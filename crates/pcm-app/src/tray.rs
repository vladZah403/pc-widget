//! Icons in the notification area: each one a reading (CPU load, a
//! temperature, memory…) drawn as a number in its scale colour. They live
//! without the widget: the program stays in the tray when the widget is
//! hidden. Clicks arrive at the widget window as WM_APP_TRAY.

use std::ptr::null_mut;

use pcm_render::text::Fonts;
use pcm_render::theme::{Rgb, Scale};
use pcm_render::widget::{Data, tray_icon};
use tiny_skia::Pixmap;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::HiDpi::{GetDpiForSystem, GetSystemMetricsForDpi};
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub const WM_APP_TRAY: u32 = 0x8000 + 3;

pub use pcm_core::config::{TRAY_KINDS as KINDS, TRAY_MAX as MAX_ICONS};

/// The text and colour of a reading ("—" when there is none).
pub fn reading(kind: &str, d: &Data, scale: &Scale) -> (String, Rgb) {
    let pct = |v: Option<f32>| match v {
        Some(v) => (format!("{:.0}", v.clamp(0.0, 100.0)), scale.at(v / 100.0)),
        None => ("—".into(), scale.at(0.0)),
    };
    let deg = |v: Option<f32>| match v {
        Some(t) => (format!("{t:.0}°"), scale.heat(t)),
        None => ("—".into(), scale.at(0.0)),
    };
    match kind {
        "cpu_temp" => deg(d.temp_cpu),
        "gpu" => pct(d.gpu_load),
        "gpu_temp" => deg(d.temp_gpu.or(if d.gpu_shared { d.temp_cpu } else { None })),
        "ram" => pct(Some(d.ram_pct)),
        _ => pct(Some(d.cpu)),
    }
}

/// The tooltip: the main readings in a few short lines (at most 127 chars).
pub fn tooltip(d: &Data) -> String {
    let mut lines = vec!["PC Monitor 2.0".to_string()];
    let mut cpu = format!("CPU {:.0}%", d.cpu);
    if let Some(t) = d.temp_cpu {
        cpu += &format!(" · {}{t:.0}°", if d.temp_cpu_approx { "≈" } else { "" });
    }
    lines.push(cpu + &format!("   RAM {:.0}%", d.ram_pct));
    if let Some(g) = d.gpu_load {
        let mut gpu = format!("GPU {g:.0}%");
        if let Some(t) = d.temp_gpu {
            gpu += &format!(" · {t:.0}°");
        }
        lines.push(gpu);
    }
    let mut tip = lines.join("\n");
    tip.truncate(tip.char_indices().nth(127).map_or(tip.len(), |(i, _)| i));
    tip
}

/// An HICON from a premultiplied RGBA pixmap (icons take straight alpha).
fn hicon(pm: &Pixmap) -> HICON {
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    unsafe {
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = w;
        bmi.bmiHeader.biHeight = -h;
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;
        let mut bits = null_mut();
        let screen = GetDC(null_mut());
        let color = CreateDIBSection(screen, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        ReleaseDC(null_mut(), screen);
        if color.is_null() || bits.is_null() {
            return null_mut();
        }
        let dst = std::slice::from_raw_parts_mut(bits as *mut u8, (w * h * 4) as usize);
        for (d, p) in dst.as_chunks_mut::<4>().0.iter_mut().zip(pm.data().as_chunks::<4>().0) {
            let a = p[3] as u32;
            let un = |c: u8| (c as u32 * 255 + a / 2).checked_div(a).map_or(0, |v| v.min(255) as u8);
            *d = [un(p[2]), un(p[1]), un(p[0]), p[3]];
        }
        let mask = CreateBitmap(w, h, 1, 1, null_mut());
        let info = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info);
        DeleteObject(mask);
        DeleteObject(color);
        icon
    }
}

fn tip_buf(text: &str) -> [u16; 128] {
    let mut buf = [0u16; 128];
    for (i, c) in text.encode_utf16().take(127).enumerate() {
        buf[i] = c;
    }
    buf
}

struct Icon {
    id: u32,
    kind: String,
    handle: HICON,
    shown: Option<(String, Rgb)>,
    added: bool,
}

pub struct Tray {
    hwnd: HWND,
    icons: Vec<Icon>,
    size: u32,
    /// "TaskbarCreated": explorer.exe restarted, the icons must be added again.
    pub taskbar_created: u32,
    tip: String,
}

impl Tray {
    pub fn new(hwnd: HWND) -> Tray {
        let name: Vec<u16> = "TaskbarCreated".encode_utf16().chain(Some(0)).collect();
        let size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem()) }.clamp(16, 64) as u32;
        let taskbar_created = unsafe { RegisterWindowMessageW(name.as_ptr()) };
        // elevated, Windows drops these from explorer.exe (lower integrity) unless let through
        for msg in [WM_APP_TRAY, taskbar_created] {
            unsafe { ChangeWindowMessageFilterEx(hwnd, msg, MSGFLT_ALLOW, null_mut()) };
        }
        Tray { hwnd, icons: Vec::new(), size, taskbar_created, tip: String::new() }
    }

    fn data(&self, icon: &Icon, flags: u32) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: icon.id,
            uFlags: flags,
            uCallbackMessage: WM_APP_TRAY,
            hIcon: icon.handle,
            szTip: tip_buf(&self.tip),
            ..Default::default()
        }
    }

    /// Show exactly these readings (in this order).
    pub fn set_kinds(&mut self, kinds: &[String]) {
        let wanted: Vec<&String> = kinds.iter().take(MAX_ICONS).collect();
        if self.icons.iter().map(|i| &i.kind).eq(wanted.iter().copied()) {
            return;
        }
        self.remove_all();
        self.icons = wanted
            .iter()
            .enumerate()
            .map(|(i, k)| Icon {
                id: i as u32 + 1,
                kind: (*k).clone(),
                handle: null_mut(),
                shown: None,
                added: false,
            })
            .collect();
    }

    /// Draw what changed and hand it to the shell.
    pub fn update(&mut self, d: &Data, fonts: &mut Fonts, scale: &Scale) {
        self.tip = tooltip(d);
        for i in 0..self.icons.len() {
            let now = reading(&self.icons[i].kind, d, scale);
            let changed = self.icons[i].shown.as_ref() != Some(&now);
            if changed && let Some(pm) = tray_icon(fonts, self.size, &now.0, now.1) {
                let new = hicon(&pm);
                if !new.is_null() {
                    let old = std::mem::replace(&mut self.icons[i].handle, new);
                    if !old.is_null() {
                        unsafe { DestroyIcon(old) };
                    }
                    self.icons[i].shown = Some(now);
                }
            }
            if self.icons[i].handle.is_null() {
                continue;
            }
            let data = self.data(&self.icons[i], NIF_ICON | NIF_TIP | NIF_MESSAGE | NIF_SHOWTIP);
            let msg = if self.icons[i].added { NIM_MODIFY } else { NIM_ADD };
            if unsafe { Shell_NotifyIconW(msg, &data) } != 0 {
                self.icons[i].added = true;
            } else if msg == NIM_MODIFY {
                self.icons[i].added = false; // the shell lost it: add it next time
            }
        }
    }

    /// explorer.exe restarted: every icon goes in again.
    pub fn readd(&mut self) {
        for i in &mut self.icons {
            i.added = false;
        }
    }

    pub fn remove_all(&mut self) {
        for i in &self.icons {
            if i.added {
                let data = self.data(i, 0);
                unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
            }
            if !i.handle.is_null() {
                unsafe { DestroyIcon(i.handle) };
            }
        }
        self.icons.clear();
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.remove_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcm_render::theme::Theme;

    #[test]
    fn readings_and_tip() {
        let s = Theme::Heat.scale();
        let d = pcm_render::widget::sample_data();
        assert_eq!(reading("cpu", &d, &s).0, "37");
        assert_eq!(reading("cpu_temp", &d, &s).0, "66°");
        assert_eq!(reading("gpu_temp", &d, &s).0, "66°", "integrated GPU shares the CPU's");
        assert_eq!(reading("ram", &d, &s).0, "64");
        let none = Data::default();
        assert_eq!(reading("cpu_temp", &none, &s).0, "—");
        let tip = tooltip(&d);
        assert!(tip.starts_with("PC Monitor 2.0\nCPU 37% · 66°") && tip.chars().count() < 128, "{tip}");
    }
}

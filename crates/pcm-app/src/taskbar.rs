//! The mini panel on the taskbar, left of the tray icons (1.8's TaskbarPanel,
//! the TrafficMonitor way): a topmost window owned by the taskbar, and the
//! task button area (MSTaskSwWClass) narrowed to make room — given back on
//! exit. Hidden for a vertical or auto-hidden taskbar, during fullscreen
//! games and on the new Windows 11 taskbar, whose tray area has no size.

use std::ptr::{null, null_mut};

use pcm_render::text::Fonts;
use pcm_render::theme::Theme;
use pcm_render::widget::{Data, taskbar_panel};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::layered::{self, Surface};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn rect(h: HWND) -> RECT {
    let mut r = RECT::default();
    unsafe { GetWindowRect(h, &mut r) };
    r
}

/// Is the taskbar light (Windows' "SystemUsesLightTheme")?
pub fn light_taskbar() -> bool {
    let (k, v) =
        (wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"), wide("SystemUsesLightTheme"));
    let mut value = 0u32;
    let mut size = 4u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            k.as_ptr(),
            v.as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            &mut value as *mut _ as *mut _,
            &mut size,
        )
    };
    rc == 0 && value == 1
}

/// (taskbar, tray area, task buttons, their parent)
fn find() -> (HWND, HWND, HWND, HWND) {
    unsafe {
        let bar = FindWindowW(wide("Shell_TrayWnd").as_ptr(), null());
        if bar.is_null() {
            return (null_mut(), null_mut(), null_mut(), null_mut());
        }
        let notify = FindWindowExW(bar, null_mut(), wide("TrayNotifyWnd").as_ptr(), null());
        let rebar = FindWindowExW(bar, null_mut(), wide("ReBarWindow32").as_ptr(), null());
        let tasks = if rebar.is_null() {
            null_mut()
        } else {
            FindWindowExW(rebar, null_mut(), wide("MSTaskSwWClass").as_ptr(), null())
        };
        (bar, notify, tasks, rebar)
    }
}

/// Resize a child so its right edge is at `right` (screen px); never blocks
/// on explorer.exe.
fn set_child_right(child: HWND, parent: HWND, right: i32) {
    let r = rect(child);
    let mut pt = POINT { x: r.left, y: r.top };
    unsafe {
        ScreenToClient(parent, &mut pt);
        SetWindowPos(
            child,
            null_mut(),
            pt.x,
            pt.y,
            (right - r.left).max(0),
            r.bottom - r.top,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
        );
    }
}

pub struct Panel {
    pub hwnd: HWND,
    surface: Option<Surface>,
    shown: bool,
    owner: HWND,
    /// The task buttons we narrowed, to give them their width back.
    narrowed: Option<(HWND, HWND)>,
    light: bool,
    checked_theme: std::time::Instant,
}

impl Panel {
    pub fn new(hwnd: HWND) -> Panel {
        Panel {
            hwnd,
            surface: None,
            shown: false,
            owner: null_mut(),
            narrowed: None,
            light: light_taskbar(),
            checked_theme: std::time::Instant::now(),
        }
    }

    pub fn hide(&mut self) {
        if self.shown {
            unsafe { ShowWindow(self.hwnd, SW_HIDE) };
            self.shown = false;
        }
        self.restore();
    }

    /// Give the task buttons their full width back.
    pub fn restore(&mut self) {
        if let Some((tasks, rebar)) = self.narrowed.take()
            && unsafe { IsWindow(tasks) } != 0
        {
            set_child_right(tasks, rebar, rect(rebar).right);
        }
    }

    /// Draw and place the panel (once a second with fresh data).
    pub fn update(&mut self, data: &Data, fonts: &mut Fonts, theme: &Theme, scale: f32, busy: bool) {
        if self.checked_theme.elapsed().as_secs() >= 10 {
            self.light = light_taskbar();
            self.checked_theme = std::time::Instant::now();
        }
        let (bar, notify, tasks, rebar) = find();
        if bar.is_null() || notify.is_null() {
            return self.hide();
        }
        let n = rect(notify);
        let b = rect(bar);
        let h = b.bottom - b.top;
        let screen_h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
        let visible = b.bottom.min(screen_h) - b.top.max(0);
        // no tray area (Windows 11), vertical, auto-hidden, or a fullscreen game
        if n.right - n.left <= 0 || b.right - b.left <= h || (visible as f32) < h as f32 * 0.8 || busy {
            return self.hide();
        }
        let Some(pm) = taskbar_panel(fonts, scale, data, theme, self.light, h as u32) else { return };
        let gap = (6.0 * scale).round() as i32;
        let w = pm.width() as i32;
        let x = n.left - gap - w;
        if self.owner != bar {
            unsafe { SetWindowLongPtrW(self.hwnd, GWLP_HWNDPARENT, bar as isize) };
            self.owner = bar;
        }
        if !tasks.is_null() && !rebar.is_null() {
            let limit = x - gap;
            let t = rect(tasks);
            if t.left < limit && (t.right - limit).abs() > 1 {
                set_child_right(tasks, rebar, limit);
            }
            self.narrowed = Some((tasks, rebar));
        }
        layered::show(self.hwnd, &mut self.surface, &pm, 1.0, Some((x, b.top)));
        if !self.shown {
            self.shown = true;
            unsafe {
                ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                SetWindowPos(self.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        self.restore();
    }
}

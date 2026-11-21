//! Layered windows: a pixmap goes to the screen with per-pixel alpha through
//! UpdateLayeredWindow (no window chrome, true transparency).

use std::ptr::{null, null_mut};

use tiny_skia::Pixmap;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::{ULW_ALPHA, UpdateLayeredWindow};

/// A DIB section kept between frames.
pub struct Surface {
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    pub w: i32,
    pub h: i32,
}

impl Surface {
    fn new(w: i32, h: i32) -> Surface {
        unsafe {
            let screen = GetDC(null_mut());
            let dc = CreateCompatibleDC(screen);
            ReleaseDC(null_mut(), screen);
            let mut bmi: BITMAPINFO = std::mem::zeroed();
            bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = w;
            bmi.bmiHeader.biHeight = -h; // top-down
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            bmi.bmiHeader.biCompression = BI_RGB;
            let mut bits = null_mut();
            let bmp = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
            let old = SelectObject(dc, bmp);
            Surface { dc, bmp, old, bits: bits as *mut u8, w, h }
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            DeleteObject(self.bmp);
            DeleteDC(self.dc);
        }
    }
}

/// Put `pm` (premultiplied RGBA) on the window at opacity `alpha` (0..1),
/// moving it to `pos` (screen px) if given; the window takes the pixmap's size.
pub fn show(hwnd: HWND, surface: &mut Option<Surface>, pm: &Pixmap, alpha: f32, pos: Option<(i32, i32)>) {
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    if surface.as_ref().is_none_or(|s| s.w != w || s.h != h) {
        *surface = Some(Surface::new(w, h));
    }
    let Some(s) = surface.as_ref() else { return };
    if s.bits.is_null() {
        return;
    }
    unsafe {
        let dst = std::slice::from_raw_parts_mut(s.bits, (w * h * 4) as usize);
        for (d, p) in dst.as_chunks_mut::<4>().0.iter_mut().zip(pm.data().as_chunks::<4>().0) {
            *d = [p[2], p[1], p[0], p[3]];
        }
        let size = SIZE { cx: w, cy: h };
        let src = POINT { x: 0, y: 0 };
        let dst_pt = pos.map(|(x, y)| POINT { x, y });
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let screen = GetDC(null_mut());
        UpdateLayeredWindow(
            hwnd,
            screen,
            dst_pt.as_ref().map_or(null(), |p| p as *const POINT),
            &size,
            s.dc,
            &src,
            0,
            &blend,
            ULW_ALPHA,
        );
        ReleaseDC(null_mut(), screen);
    }
}

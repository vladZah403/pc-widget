//! Popup menus in the program's colours, as Windows 10/11 draw their own:
//! dark or light (uxtheme's unnamed exports 135 SetPreferredAppMode and 136
//! FlushMenuThemes, which Explorer and Notepad use; Windows 10 1903 and
//! later), and an icon beside an item (a Segoe MDL2 Assets glyph drawn into
//! a 32-bit bitmap with alpha).

use std::ptr::null_mut;
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CLIP_DEFAULT_PRECIS, CreateCompatibleDC,
    CreateDIBSection, CreateFontW, DEFAULT_CHARSET, DIB_RGB_COLORS, DT_CENTER, DT_NOPREFIX, DT_SINGLELINE,
    DT_VCENTER, DeleteDC, DeleteObject, DrawTextW, FW_NORMAL, GdiFlush, HBITMAP, OUT_DEFAULT_PRECIS,
    SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetMenuItemCount, HMENU, MENUITEMINFOW, MIIM_BITMAP, SetMenuItemInfoW,
};

use crate::wide;

type SetPreferredAppMode = unsafe extern "system" fn(i32) -> i32;
type FlushMenuThemes = unsafe extern "system" fn();

/// uxtheme's two exports, on Windows 10 1903 (build 18362) and later.
fn uxtheme() -> Option<(SetPreferredAppMode, FlushMenuThemes)> {
    static FNS: OnceLock<Option<(SetPreferredAppMode, FlushMenuThemes)>> = OnceLock::new();
    *FNS.get_or_init(|| unsafe {
        if crate::system::windows_build().is_none_or(|b| b < 18362) {
            return None;
        }
        let lib = LoadLibraryW(wide("uxtheme.dll").as_ptr());
        if lib.is_null() {
            return None;
        }
        let set = GetProcAddress(lib, 135 as _)?;
        let flush = GetProcAddress(lib, 136 as _)?;
        Some((
            std::mem::transmute::<unsafe extern "system" fn() -> isize, SetPreferredAppMode>(set),
            std::mem::transmute::<unsafe extern "system" fn() -> isize, FlushMenuThemes>(flush),
        ))
    })
}

/// This process's popup menus dark or light from now on.
pub fn set_dark(dark: bool) {
    if let Some((set, flush)) = uxtheme() {
        // 2: force dark, 3: force light
        unsafe {
            set(if dark { 2 } else { 3 });
            flush();
        }
    }
}

/// `glyph` (Segoe MDL2 Assets) `px` square in `rgb`, on a transparent
/// background: a premultiplied 32-bit bitmap a menu item can show. Delete
/// it (DeleteObject) once the menu is gone.
pub fn glyph_bitmap(glyph: &str, px: i32, rgb: (u8, u8, u8)) -> Option<HBITMAP> {
    unsafe {
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: px,
            biHeight: -px, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let mut bits: *mut core::ffi::c_void = null_mut();
        let bmp = CreateDIBSection(null_mut(), &bi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        if bmp.is_null() || bits.is_null() {
            return None;
        }
        let dc = CreateCompatibleDC(null_mut());
        let old = SelectObject(dc, bmp);
        // white on black, grey edges: the grey becomes the alpha
        let face = wide("Segoe MDL2 Assets");
        let font = CreateFontW(
            -(px as f32 * 0.9).round() as i32,
            0,
            0,
            0,
            FW_NORMAL as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            ANTIALIASED_QUALITY as u32,
            0,
            face.as_ptr(),
        );
        let old_font = SelectObject(dc, font);
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, 0x00FF_FFFF);
        let text = wide(glyph);
        let mut r = RECT { left: 0, top: 0, right: px, bottom: px };
        DrawTextW(dc, text.as_ptr(), -1, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        GdiFlush();
        SelectObject(dc, old_font);
        DeleteObject(font);
        SelectObject(dc, old);
        DeleteDC(dc);
        let px_bytes = std::slice::from_raw_parts_mut(bits as *mut u8, (px * px * 4) as usize);
        for p in px_bytes.as_chunks_mut::<4>().0.iter_mut() {
            let a = p[0].max(p[1]).max(p[2]) as u16;
            let pre = |c: u8| ((c as u16 * a + 127) / 255) as u8;
            *p = [pre(rgb.2), pre(rgb.1), pre(rgb.0), a as u8]; // BGRA
        }
        Some(bmp)
    }
}

/// Show `bmp` beside the item last appended to `menu` (an HMENU, as isize:
/// nothing is read through it here).
pub fn icon_on_last(menu: isize, bmp: Option<HBITMAP>) {
    let Some(bmp) = bmp else { return };
    let menu = menu as HMENU;
    unsafe {
        let n = GetMenuItemCount(menu);
        if n <= 0 {
            return;
        }
        let mut mii: MENUITEMINFOW = std::mem::zeroed();
        mii.cbSize = size_of::<MENUITEMINFOW>() as u32;
        mii.fMask = MIIM_BITMAP;
        mii.hbmpItem = bmp;
        SetMenuItemInfoW(menu, (n - 1) as u32, 1, &mii);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_glyph_has_opaque_and_clear_pixels() {
        use windows_sys::Win32::Graphics::Gdi::{GetDC, GetDIBits, ReleaseDC};
        let bmp = glyph_bitmap("\u{E80F}", 16, (255, 255, 255)).expect("bitmap");
        let mut buf = vec![0u8; 16 * 16 * 4];
        unsafe {
            let mut bi: BITMAPINFO = std::mem::zeroed();
            bi.bmiHeader = BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: 16,
                biHeight: -16,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..std::mem::zeroed()
            };
            let dc = GetDC(null_mut());
            GetDIBits(dc, bmp, 0, 16, buf.as_mut_ptr() as _, &mut bi, DIB_RGB_COLORS);
            ReleaseDC(null_mut(), dc);
            DeleteObject(bmp);
        }
        let alpha: Vec<u8> = buf.as_chunks::<4>().0.iter().map(|p| p[3]).collect();
        assert!(alpha.iter().any(|a| *a > 200), "the glyph is drawn");
        assert!(alpha.contains(&0), "around it is clear");
    }
}

//! A program's icon as pixels (the Games page shows each game's).

use std::path::Path;
use std::ptr::null_mut;

use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, ReleaseDC,
};
use windows_sys::Win32::UI::Shell::SHDefExtractIconW;
use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

use crate::wide;

/// The first icon of `exe` at `size` px: straight (not premultiplied) RGBA,
/// rows top to bottom. None when the file has no icon.
pub fn exe_icon(exe: &Path, size: u32) -> Option<Vec<u8>> {
    let path = wide(&exe.to_string_lossy());
    let mut icon: HICON = null_mut();
    // the low word asks for the large icon's size
    let hr = unsafe { SHDefExtractIconW(path.as_ptr(), 0, 0, &mut icon, null_mut(), size & 0xFFFF) };
    if hr != 0 || icon.is_null() {
        return None;
    }
    let rgba = pixels(icon, size);
    unsafe { DestroyIcon(icon) };
    rgba
}

/// An icon's colour bitmap (and its mask, for icons without alpha) as RGBA.
fn pixels(icon: HICON, size: u32) -> Option<Vec<u8>> {
    unsafe {
        let mut ii: ICONINFO = std::mem::zeroed();
        if GetIconInfo(icon, &mut ii) == 0 {
            return None;
        }
        let read = |bmp| {
            let mut bi: BITMAPINFO = std::mem::zeroed();
            bi.bmiHeader = BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size as i32,
                biHeight: -(size as i32), // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..std::mem::zeroed()
            };
            let mut buf = vec![0u8; (size * size * 4) as usize];
            let dc = GetDC(null_mut());
            let lines = GetDIBits(dc, bmp, 0, size, buf.as_mut_ptr() as _, &mut bi, DIB_RGB_COLORS);
            ReleaseDC(null_mut(), dc);
            (lines == size as i32).then_some(buf)
        };
        let color = if ii.hbmColor.is_null() { None } else { read(ii.hbmColor) };
        let mask = if ii.hbmMask.is_null() { None } else { read(ii.hbmMask) };
        if !ii.hbmColor.is_null() {
            DeleteObject(ii.hbmColor);
        }
        if !ii.hbmMask.is_null() {
            DeleteObject(ii.hbmMask);
        }
        let mut px = color?;
        let has_alpha = px.as_chunks::<4>().0.iter().any(|p| p[3] != 0);
        for (i, p) in px.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            p.swap(0, 2); // BGRA -> RGBA
            if !has_alpha {
                // an old icon: the mask says what's transparent (white = see-through)
                let see_through = mask.as_ref().is_some_and(|m| m[i * 4] != 0);
                p[3] = if see_through { 0 } else { 255 };
            }
        }
        Some(px)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explorer_has_an_icon() {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        let px = exe_icon(Path::new(&format!(r"{windir}\explorer.exe")), 32).expect("explorer's icon");
        assert_eq!(px.len(), 32 * 32 * 4);
        assert!(px.as_chunks::<4>().0.iter().any(|p| p[3] == 255), "some opaque pixels");
        assert!(px.as_chunks::<4>().0.iter().any(|p| p[3] == 0), "a transparent edge");
    }

    #[test]
    fn no_file_no_icon() {
        assert!(exe_icon(Path::new(r"C:\no\such\game.exe"), 32).is_none());
    }
}

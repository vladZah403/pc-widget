//! A picture on the clipboard, for pasting into a chat (Ctrl+V): as a DIB,
//! which every program takes, and as "PNG", which keeps the transparency
//! for the programs that look for it (Discord, browsers, Office).

use std::path::PathBuf;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::GlobalFree;
use windows_sys::Win32::Graphics::Gdi::{BI_RGB, BITMAPINFOHEADER};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};

/// CF_DIB (winuser.h).
const CF_DIB: u32 = 8;

/// Movable global memory holding `bytes` (the clipboard owns it once set).
fn global(bytes: &[u8]) -> Option<*mut core::ffi::c_void> {
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
        if h.is_null() {
            return None;
        }
        let p = GlobalLock(h) as *mut u8;
        if p.is_null() {
            GlobalFree(h);
            return None;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        GlobalUnlock(h);
        Some(h)
    }
}

/// A DIB of straight RGBA pixels: a BITMAPINFOHEADER, then BGRA rows from the top
/// (a negative height), alpha kept.
pub fn dib(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let header = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width as i32,
        biHeight: -(height as i32),
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        biSizeImage: width * height * 4,
        biXPelsPerMeter: 0,
        biYPelsPerMeter: 0,
        biClrUsed: 0,
        biClrImportant: 0,
    };
    let mut out = Vec::with_capacity(size_of::<BITMAPINFOHEADER>() + rgba.len());
    let raw = unsafe {
        std::slice::from_raw_parts(&header as *const _ as *const u8, size_of::<BITMAPINFOHEADER>())
    };
    out.extend_from_slice(raw);
    for px in rgba.as_chunks::<4>().0 {
        out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }
    out
}

/// Put the picture on the clipboard: straight (not premultiplied) RGBA
/// pixels and the same picture as PNG bytes.
pub fn set_image(rgba: &[u8], width: u32, height: u32, png: &[u8]) -> bool {
    unsafe {
        // another program may hold the clipboard a moment
        let mut open = false;
        for _ in 0..10 {
            if OpenClipboard(null_mut()) != 0 {
                open = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        if !open {
            return false;
        }
        EmptyClipboard();
        let mut ok = false;
        if let Some(h) = global(&dib(rgba, width, height)) {
            if SetClipboardData(CF_DIB, h).is_null() {
                GlobalFree(h);
            } else {
                ok = true;
            }
        }
        let name: Vec<u16> = "PNG\0".encode_utf16().collect();
        let png_format = RegisterClipboardFormatW(name.as_ptr());
        if png_format != 0
            && let Some(h) = global(png)
            && SetClipboardData(png_format, h).is_null()
        {
            GlobalFree(h);
        }
        CloseClipboard();
        ok
    }
}

/// The user's Pictures folder (FOLDERID_Pictures).
pub fn pictures_dir() -> Option<PathBuf> {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_Pictures, SHGetKnownFolderPath};
    unsafe {
        let mut p: *mut u16 = null_mut();
        let hr = SHGetKnownFolderPath(&FOLDERID_Pictures, 0, null_mut(), &mut p);
        let path = if hr >= 0 { crate::from_wide_ptr(p) } else { None };
        if !p.is_null() {
            CoTaskMemFree(p as *const _);
        }
        path.filter(|s| !s.is_empty()).map(PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn dib_is_top_down_bgra() {
        let d = super::dib(&[1, 2, 3, 4, 5, 6, 7, 8], 2, 1);
        assert_eq!(d.len(), 40 + 8);
        assert_eq!(i32::from_le_bytes(d[8..12].try_into().unwrap()), -1, "top-down");
        assert_eq!(&d[40..], &[3, 2, 1, 4, 7, 6, 5, 8]);
    }

    #[test]
    fn pictures_folder() {
        assert!(super::pictures_dir().is_some_and(|p| p.is_absolute()));
    }
}

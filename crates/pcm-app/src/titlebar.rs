//! The PC Monitor window's title bar in the window's own colours: Windows 11
//! paints it with the accent colour when "Show accent colour on title bars"
//! is on (a green or blue strip over a dark window). DWM's caption, text and
//! border colours (Windows 11; Windows 10 keeps the dark or light theme egui
//! asks for).

use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_TEXT_COLOR,
    DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Controls::MARGINS;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumThreadWindows, GW_OWNER, GetWindow, GetWindowTextLengthW,
};

/// COLORREF of an RGB colour (0x00BBGGRR).
fn colorref((r, g, b): (u8, u8, u8)) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

/// The title bar's background, text and border colours on this thread's
/// titled top-level windows (the PC Monitor window).
pub fn set_colors(caption: (u8, u8, u8), text: (u8, u8, u8), border: (u8, u8, u8)) {
    let colors: [(i32, u32); 3] = [
        (DWMWA_CAPTION_COLOR, colorref(caption)),
        (DWMWA_TEXT_COLOR, colorref(text)),
        (DWMWA_BORDER_COLOR, colorref(border)),
    ];
    unsafe extern "system" fn each(hwnd: HWND, lp: LPARAM) -> i32 {
        unsafe {
            if GetWindowTextLengthW(hwnd) > 0 && GetWindow(hwnd, GW_OWNER).is_null() {
                let colors = &*(lp as *const [(i32, u32); 3]);
                for (attr, value) in colors {
                    // Windows 10 refuses these: nothing to do then
                    DwmSetWindowAttribute(hwnd, *attr as u32, value as *const u32 as _, 4);
                }
            }
        }
        1
    }
    unsafe {
        EnumThreadWindows(GetCurrentThreadId(), Some(each), &colors as *const _ as LPARAM);
    }
}

/// Windows 11 22H2 and later draw the Mica material behind a window.
pub fn mica_available() -> bool {
    pcm_win::system::windows_build().is_some_and(|b| b >= 22621)
}

/// Mica behind this thread's titled windows: the material through the
/// whole window (the frame extended over it), DWMSBT_MAINWINDOW.
pub fn set_mica() {
    unsafe extern "system" fn each(hwnd: HWND, _: LPARAM) -> i32 {
        unsafe {
            if GetWindowTextLengthW(hwnd) > 0 && GetWindow(hwnd, GW_OWNER).is_null() {
                let margins =
                    MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
                DwmExtendFrameIntoClientArea(hwnd, &margins);
                let mica: u32 = 2; // DWMSBT_MAINWINDOW
                DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE as u32, &mica as *const u32 as _, 4);
            }
        }
        1
    }
    unsafe {
        EnumThreadWindows(GetCurrentThreadId(), Some(each), 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colorref_is_bgr() {
        assert_eq!(colorref((0x12, 0x34, 0x56)), 0x0056_3412);
    }
}

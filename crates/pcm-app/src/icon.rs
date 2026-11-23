//! The logo on our windows: icon 1 of PC-Monitor.exe (build.rs puts
//! assets/app.ico there), each size drawn for itself, so Windows picks the
//! one it needs instead of shrinking a big one.

use std::ptr::null;

use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// The logo at `size` px (None if the exe has no icon).
fn load(size: i32) -> Option<HICON> {
    let h = unsafe { LoadImageW(GetModuleHandleW(null()), 1 as _, IMAGE_ICON, size, size, LR_DEFAULTCOLOR) };
    (!h.is_null()).then_some(h as HICON)
}

/// For a window class: the logo at the system's icon size.
pub fn class_icon() -> HICON {
    unsafe { LoadIconW(GetModuleHandleW(null()), 1 as _) }
}

/// Give `hwnd` the logo in its title bar (small) and on the taskbar and
/// Alt+Tab (big), at its monitor's DPI.
pub fn set_on(hwnd: HWND) {
    unsafe {
        let dpi = GetDpiForWindow(hwnd).max(96);
        for (which, metric) in [(ICON_SMALL, SM_CXSMICON), (ICON_BIG, SM_CXICON)] {
            if let Some(icon) = load(GetSystemMetricsForDpi(metric, dpi)) {
                SendMessageW(hwnd, WM_SETICON, which as WPARAM, icon as LPARAM);
            }
        }
    }
}

/// The logo on this thread's top-level windows with a title (the PC
/// Monitor window: eframe makes it on this thread).
pub fn set_on_this_thread() {
    unsafe extern "system" fn each(hwnd: HWND, _: LPARAM) -> i32 {
        unsafe {
            if GetWindowTextLengthW(hwnd) > 0 && GetWindow(hwnd, GW_OWNER).is_null() {
                set_on(hwnd);
            }
        }
        1
    }
    unsafe {
        EnumThreadWindows(GetCurrentThreadId(), Some(each), 0);
    }
}

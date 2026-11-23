//! Where the taskbar's buttons are. Windows 11's taskbar draws its buttons
//! itself (XAML): they can't be narrowed to make room for the mini panel as
//! on Windows 10, so UI Automation tells where they end instead.

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::System::Variant::{VARIANT, VT_I4};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationCacheRequest, IUIAutomationCondition, TreeScope_Descendants,
    UIA_AutomationIdPropertyId, UIA_BoundingRectanglePropertyId, UIA_ButtonControlTypeId,
    UIA_ClassNamePropertyId, UIA_ControlTypePropertyId,
};

/// One button of the taskbar.
#[derive(Clone, Debug, PartialEq)]
pub struct Button {
    /// "Taskbar.TaskListButtonAutomationPeer" for an app, "ToggleButton"…
    pub class: String,
    /// "StartButton", "SearchButton", an app's id…
    pub id: String,
    /// On the screen: left, top, right, bottom (px).
    pub rect: (i32, i32, i32, i32),
}

/// A UI Automation client for the taskbar. It joins its thread's COM
/// apartment as multithreaded, so it belongs on a thread of its own (the
/// answers come from explorer.exe and can take a while).
pub struct TaskbarButtons {
    uia: IUIAutomation,
    buttons: IUIAutomationCondition,
    cache: IUIAutomationCacheRequest,
}

impl TaskbarButtons {
    pub fn new() -> Option<TaskbarButtons> {
        unsafe {
            // S_FALSE (already) and RPC_E_CHANGED_MODE (an STA thread) both still work
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
            let mut v = VARIANT::default();
            (*v.Anonymous.Anonymous).vt = VT_I4;
            (*v.Anonymous.Anonymous).Anonymous.lVal = UIA_ButtonControlTypeId.0;
            let buttons = uia.CreatePropertyCondition(UIA_ControlTypePropertyId, &v).ok()?;
            // the three properties come along with the search: one call to explorer
            let cache = uia.CreateCacheRequest().ok()?;
            for p in [UIA_BoundingRectanglePropertyId, UIA_ClassNamePropertyId, UIA_AutomationIdPropertyId] {
                cache.AddProperty(p).ok()?;
            }
            Some(TaskbarButtons { uia, buttons, cache })
        }
    }

    /// The visible buttons inside the taskbar window `bar` (Shell_TrayWnd).
    pub fn list(&self, bar: *mut core::ffi::c_void) -> Option<Vec<Button>> {
        unsafe {
            let root = self.uia.ElementFromHandle(HWND(bar)).ok()?;
            let all = root.FindAllBuildCache(TreeScope_Descendants, &self.buttons, &self.cache).ok()?;
            let mut out = Vec::new();
            for i in 0..all.Length().ok()? {
                let Ok(e) = all.GetElement(i) else { continue };
                let Ok(r) = e.CachedBoundingRectangle() else { continue };
                if r.right <= r.left || r.bottom <= r.top {
                    continue; // hidden
                }
                out.push(Button {
                    class: e.CachedClassName().map(|s| s.to_string()).unwrap_or_default(),
                    id: e.CachedAutomationId().map(|s| s.to_string()).unwrap_or_default(),
                    rect: (r.left, r.top, r.right, r.bottom),
                });
            }
            Some(out)
        }
    }
}

/// The right edge of the rightmost button left of the tray (`tray_left`),
/// on the taskbar's own rows (`top`..`bottom`); None without one.
pub fn right_edge(buttons: &[Button], tray_left: i32, top: i32, bottom: i32) -> Option<i32> {
    buttons
        .iter()
        .filter(|b| b.rect.2 <= tray_left && b.rect.3 > top && b.rect.1 < bottom)
        .map(|b| b.rect.2)
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(class: &str, l: i32, r: i32) -> Button {
        Button { class: class.into(), id: String::new(), rect: (l, 1032, r, 1080) }
    }

    #[test]
    fn rightmost_button_left_of_the_tray() {
        let list = [
            b("ToggleButton", 0, 120), // weather, far left
            b("ToggleButton", 700, 748),
            b("Taskbar.TaskListButtonAutomationPeer", 748, 796),
            b("Taskbar.TaskListButtonAutomationPeer", 1100, 1148),
            b("SystemTray.NormalButton", 1500, 1530), // in the tray
        ];
        assert_eq!(right_edge(&list, 1490, 1032, 1080), Some(1148));
        assert_eq!(right_edge(&list[4..], 1490, 1032, 1080), None);
    }

    #[test]
    fn other_monitors_taskbars_dont_count() {
        // a button of another bar (above this one) is left out
        let other = Button { class: "x".into(), id: String::new(), rect: (0, 0, 1400, 48) };
        assert_eq!(right_edge(&[other, b("x", 10, 60)], 1490, 1032, 1080), Some(60));
    }

    #[test]
    #[ignore = "needs a taskbar: cargo test -p pcm-win print_taskbar_buttons -- --ignored --nocapture"]
    fn print_taskbar_buttons() {
        let t = TaskbarButtons::new().expect("UI Automation");
        let bar = unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW(
                crate::wide("Shell_TrayWnd").as_ptr(),
                std::ptr::null(),
            )
        };
        let list = t.list(bar).expect("buttons");
        for b in &list {
            println!("{b:?}");
        }
        assert!(!list.is_empty());
    }
}

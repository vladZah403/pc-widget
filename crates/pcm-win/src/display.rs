//! Monitors attached to the desktop: resolution and refresh rate.

use pcm_core::model::Display;
use windows_sys::Win32::Graphics::Gdi::{
    DEVMODEW, DISPLAY_DEVICE_ATTACHED_TO_DESKTOP, DISPLAY_DEVICEW, ENUM_CURRENT_SETTINGS,
    EnumDisplayDevicesW, EnumDisplaySettingsW,
};

use crate::from_wide;

fn device(parent: Option<&[u16]>, i: u32) -> Option<DISPLAY_DEVICEW> {
    let mut d = DISPLAY_DEVICEW { cb: size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
    let parent = parent.map_or(std::ptr::null(), |p| p.as_ptr());
    (unsafe { EnumDisplayDevicesW(parent, i, &mut d, 0) } != 0).then_some(d)
}

pub fn displays() -> Vec<Display> {
    let mut out = Vec::new();
    for i in 0..16 {
        let Some(adapter) = device(None, i) else { break };
        if adapter.StateFlags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP == 0 {
            continue;
        }
        let mut dm = DEVMODEW { dmSize: size_of::<DEVMODEW>() as u16, ..Default::default() };
        if unsafe { EnumDisplaySettingsW(adapter.DeviceName.as_ptr(), ENUM_CURRENT_SETTINGS, &mut dm) } == 0 {
            continue;
        }
        let monitor =
            device(Some(&adapter.DeviceName), 0).map(|m| from_wide(&m.DeviceString)).unwrap_or_default();
        out.push(Display {
            name: if monitor.is_empty() { from_wide(&adapter.DeviceName) } else { monitor },
            width: dm.dmPelsWidth,
            height: dm.dmPelsHeight,
            refresh_hz: dm.dmDisplayFrequency,
        });
    }
    out
}

//! The battery: charge and time left from GetSystemPowerStatus, design and
//! full capacity and the charge/discharge power from the battery driver
//! (IOCTL_BATTERY_*). No admin rights needed; LibreHardwareMonitor spends
//! ~70 ms a reading on the same data.

use std::ptr::{null, null_mut};

use pcm_core::model::Battery;
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::*;
use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

use crate::from_wide_ptr;

const IOCTL_BATTERY_QUERY_TAG: u32 = 0x0029_4040;
const IOCTL_BATTERY_QUERY_INFORMATION: u32 = 0x0029_4044;
const IOCTL_BATTERY_QUERY_STATUS: u32 = 0x0029_404C;
const BATTERY_CAPACITY_RELATIVE: u32 = 0x4000_0000;
const BATTERY_UNKNOWN_RATE: i32 = i32::MIN;

/// Paths of the battery devices.
fn devices() -> Vec<Vec<u16>> {
    let mut out = Vec::new();
    unsafe {
        let set = SetupDiGetClassDevsW(
            &GUID_DEVCLASS_BATTERY,
            null(),
            null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        );
        if set == INVALID_HANDLE_VALUE as isize {
            return out;
        }
        for i in 0..8 {
            let mut data = SP_DEVICE_INTERFACE_DATA {
                cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
                ..Default::default()
            };
            if SetupDiEnumDeviceInterfaces(set, null(), &GUID_DEVCLASS_BATTERY, i, &mut data) == 0 {
                break;
            }
            let mut need = 0u32;
            SetupDiGetDeviceInterfaceDetailW(set, &data, null_mut(), 0, &mut need, null_mut());
            if need == 0 {
                continue;
            }
            let mut buf = vec![0u64; (need as usize).div_ceil(8)];
            let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            if SetupDiGetDeviceInterfaceDetailW(set, &data, detail, need, null_mut(), null_mut()) != 0
                && let Some(path) = from_wide_ptr((*detail).DevicePath.as_ptr())
            {
                out.push(path.encode_utf16().chain(Some(0)).collect());
            }
        }
        SetupDiDestroyDeviceInfoList(set);
    }
    out
}

fn ioctl<I, O>(h: *mut core::ffi::c_void, code: u32, input: &I, output: &mut O) -> bool {
    let mut got = 0u32;
    unsafe {
        DeviceIoControl(
            h,
            code,
            input as *const I as *const _,
            size_of::<I>() as u32,
            output as *mut O as *mut _,
            size_of::<O>() as u32,
            &mut got,
            null_mut(),
        ) != 0
    }
}

#[repr(C)]
#[derive(Default)]
struct QueryInformation {
    tag: u32,
    level: u32, // BatteryInformation = 0
    at_rate: i32,
}

#[repr(C)]
#[derive(Default)]
struct Information {
    capabilities: u32,
    technology: u8,
    reserved: [u8; 3],
    chemistry: [u8; 4],
    designed_capacity: u32,
    full_charged_capacity: u32,
    default_alert1: u32,
    default_alert2: u32,
    critical_bias: u32,
    cycle_count: u32,
}

#[repr(C)]
#[derive(Default)]
struct WaitStatus {
    tag: u32,
    timeout: u32,
    power_state: u32,
    low_capacity: u32,
    high_capacity: u32,
}

#[repr(C)]
#[derive(Default)]
struct Status {
    power_state: u32,
    capacity: u32,
    voltage: u32,
    rate: i32,
}

/// (design mWh, full mWh, rate mW) of one battery device.
fn details(path: &[u16]) -> Option<(u32, u32, Option<i32>)> {
    let h = unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return None;
    }
    let result = (|| {
        let mut tag = 0u32;
        if !ioctl(h, IOCTL_BATTERY_QUERY_TAG, &0u32, &mut tag) || tag == 0 {
            return None;
        }
        let mut info = Information::default();
        if !ioctl(
            h,
            IOCTL_BATTERY_QUERY_INFORMATION,
            &QueryInformation { tag, ..Default::default() },
            &mut info,
        ) || info.capabilities & BATTERY_CAPACITY_RELATIVE != 0
        {
            return None;
        }
        let mut status = Status::default();
        let rate =
            ioctl(h, IOCTL_BATTERY_QUERY_STATUS, &WaitStatus { tag, ..Default::default() }, &mut status)
                .then_some(status.rate)
                .filter(|&r| r != BATTERY_UNKNOWN_RATE);
        Some((info.designed_capacity, info.full_charged_capacity, rate))
    })();
    unsafe { CloseHandle(h) };
    result
}

/// None when the PC has no battery.
pub fn read() -> Option<Battery> {
    let mut s = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut s) } == 0 || s.BatteryFlag & 128 != 0 || s.BatteryFlag == 255 {
        return None; // 128 = no system battery, 255 = unknown
    }
    let on_ac = s.ACLineStatus == 1;
    let mut b = Battery {
        percent: (s.BatteryLifePercent <= 100).then_some(s.BatteryLifePercent as f32),
        on_ac,
        charging: s.BatteryFlag & 8 != 0,
        // Windows' estimate; unknown right after unplugging
        seconds_left: (!on_ac && s.BatteryLifeTime != u32::MAX && s.BatteryLifeTime < 48 * 3600)
            .then_some(s.BatteryLifeTime),
        ..Default::default()
    };
    // a laptop can have two batteries: capacities and power add up
    let (mut design, mut full, mut rate, mut any) = (0u64, 0u64, 0i64, false);
    for path in devices() {
        if let Some((d, f, r)) = details(&path) {
            design += d as u64;
            full += f as u64;
            rate += r.unwrap_or(0) as i64;
            any = true;
        }
    }
    if any && design > 0 && full > 0 {
        b.design_wh = Some(design as f32 / 1000.0);
        b.full_wh = Some(full as f32 / 1000.0);
    }
    if any && rate != 0 {
        b.rate_w = Some(rate.unsigned_abs() as f32 / 1000.0);
    }
    Some(b)
}

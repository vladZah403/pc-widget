//! Facts about Windows and this process: build, admin rights, uptime,
//! the CPU's name and core counts, the PawnIO driver.

use std::ptr::null_mut;

use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows_sys::Win32::System::Registry::{HKEY, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
use windows_sys::Win32::System::SystemInformation::{
    GetLogicalProcessorInformationEx, GetTickCount64, OSVERSIONINFOW, RelationProcessorCore,
};
use windows_sys::Win32::System::Threading::{GetActiveProcessorCount, GetCurrentProcess, OpenProcessToken};

use crate::{from_wide, wide};

pub fn windows_build() -> Option<u32> {
    let mut v =
        OSVERSIONINFOW { dwOSVersionInfoSize: size_of::<OSVERSIONINFOW>() as u32, ..Default::default() };
    (unsafe { RtlGetVersion(&mut v) } == 0).then_some(v.dwBuildNumber)
}

pub fn is_windows_11() -> bool {
    windows_build().is_some_and(|b| b >= 22000)
}

/// Is this process elevated?
pub fn is_admin() -> bool {
    unsafe {
        let mut token: HANDLE = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut e = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut e as *mut _ as *mut _,
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        );
        CloseHandle(token);
        ok != 0 && e.TokenIsElevated != 0
    }
}

pub fn uptime_s() -> u64 {
    unsafe { GetTickCount64() / 1000 }
}

/// A REG_SZ value, or None.
pub(crate) fn reg_string(root: HKEY, key: &str, value: &str) -> Option<String> {
    let (k, v) = (wide(key), wide(value));
    let mut buf = [0u16; 512];
    let mut size = (buf.len() * 2) as u32;
    let rc = unsafe {
        RegGetValueW(
            root,
            k.as_ptr(),
            v.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            buf.as_mut_ptr() as _,
            &mut size,
        )
    };
    (rc == 0).then(|| from_wide(&buf).trim().to_string()).filter(|s| !s.is_empty())
}

/// "AMD Ryzen 5 5500U with Radeon Graphics" from the registry.
pub fn cpu_name() -> Option<String> {
    reg_string(HKEY_LOCAL_MACHINE, r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", "ProcessorNameString")
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// Logical processors in all groups.
pub fn threads() -> u32 {
    unsafe { GetActiveProcessorCount(0xFFFF) } // ALL_PROCESSOR_GROUPS
}

/// Physical cores.
pub fn cores() -> Option<u32> {
    let mut len = 0u32;
    unsafe { GetLogicalProcessorInformationEx(RelationProcessorCore, null_mut(), &mut len) };
    if len == 0 {
        return None;
    }
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    if unsafe { GetLogicalProcessorInformationEx(RelationProcessorCore, buf.as_mut_ptr() as _, &mut len) }
        == 0
    {
        return None;
    }
    // variable-size records: Relationship (u32), Size (u32), ...
    let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, len as usize) };
    let (mut pos, mut count) = (0usize, 0u32);
    while pos + 8 <= bytes.len() {
        let size =
            u32::from_le_bytes([bytes[pos + 4], bytes[pos + 5], bytes[pos + 6], bytes[pos + 7]]) as usize;
        if size == 0 {
            break;
        }
        count += 1;
        pos += size;
    }
    (count > 0).then_some(count)
}

/// "Windows 11 Pro", "Windows 10 Home": the build tells 10 from 11 (the
/// registry's ProductName says 10 on both), EditionID the edition.
pub fn windows_name() -> String {
    let base = if is_windows_11() { "Windows 11" } else { "Windows 10" };
    let key = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let edition = match reg_string(HKEY_LOCAL_MACHINE, key, "EditionID").as_deref() {
        Some(e) if e.starts_with("Core") => "Home",
        Some(e) if e.starts_with("Professional") => "Pro",
        Some(e) if e.starts_with("Enterprise") => "Enterprise",
        Some(e) if e.starts_with("Education") => "Education",
        _ => "",
    };
    if edition.is_empty() { base.to_string() } else { format!("{base} {edition}") }
}

/// Is the PawnIO driver (LibreHardwareMonitor's way to the CPU sensors) installed?
pub fn pawnio_installed() -> bool {
    reg_string(HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Services\PawnIO", "ImagePath").is_some()
}

/// The local time now, "2026-11-22 13:05:41".
pub fn local_time() -> String {
    let mut t = windows_sys::Win32::Foundation::SYSTEMTIME::default();
    unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t) };
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

/// Unix seconds as local time, "2026-11-22 13:05:41".
pub fn local_time_of(unix: i64) -> String {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    let ft100 = (unix.max(0) + 11_644_473_600) as u64 * 10_000_000;
    let ft = FILETIME { dwLowDateTime: ft100 as u32, dwHighDateTime: (ft100 >> 32) as u32 };
    let (mut utc, mut t) = (SYSTEMTIME::default(), SYSTEMTIME::default());
    unsafe {
        FileTimeToSystemTime(&ft, &mut utc);
        SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut t);
    }
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

/// Unix seconds now.
pub fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// The Windows display language as one of PC Monitor's: "ru", "uk", "de",
/// anything else "en" (1.8's system_language).
pub fn ui_language() -> &'static str {
    match unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() } & 0x3ff {
        0x19 => "ru",
        0x22 => "uk",
        0x07 => "de",
        _ => "en",
    }
}

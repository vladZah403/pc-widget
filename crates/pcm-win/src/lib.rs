//! PC Monitor 2.0: everything read from Windows. All `unsafe` code of the
//! program lives in this crate, behind safe functions that return the
//! `pcm-core` model; anything a reader can't get is `None`, never a panic.

pub mod battery;
pub mod cpu;
pub mod disk;
pub mod display;
pub mod frames;
pub mod gpu;
pub mod memory;
pub mod net;
pub mod nvme;
pub mod pdh;
pub mod ping;
pub mod processes;
pub mod sensors_helper;
pub mod smbios;
pub mod system;
pub mod thermal;

/// A NUL-terminated UTF-16 copy of `s` for the W functions.
pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// A Rust string from a NUL-terminated (or full) UTF-16 buffer.
pub(crate) fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// A Rust string from a NUL-terminated UTF-16 pointer (None for null).
///
/// # Safety
/// `p` must be null or point to a NUL-terminated UTF-16 string.
pub(crate) unsafe fn from_wide_ptr(p: *const u16) -> Option<String> {
    if p.is_null() {
        return None;
    }
    let mut len = 0;
    // SAFETY: the caller promises a NUL-terminated string
    unsafe {
        while *p.add(len) != 0 {
            len += 1;
        }
        Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, len)))
    }
}

/// Ticks per second of QueryPerformanceCounter.
pub(crate) fn qpc_freq() -> f64 {
    use std::sync::OnceLock;
    use windows_sys::Win32::System::Performance::QueryPerformanceFrequency;
    static FREQ: OnceLock<f64> = OnceLock::new();
    *FREQ.get_or_init(|| {
        let mut f = 0i64;
        unsafe { QueryPerformanceFrequency(&mut f) };
        f.max(1) as f64
    })
}

/// Seconds on the QueryPerformanceCounter clock (the one ETW timestamps use).
pub fn now_s() -> f64 {
    let mut c = 0i64;
    unsafe { windows_sys::Win32::System::Performance::QueryPerformanceCounter(&mut c) };
    c as f64 / qpc_freq()
}

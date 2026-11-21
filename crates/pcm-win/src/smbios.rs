//! The raw SMBIOS table (board, BIOS, RAM sticks) without WMI or admin rights.

use pcm_core::smbios::{Tables, parse};
use windows_sys::Win32::System::SystemInformation::GetSystemFirmwareTable;

const RSMB: u32 = u32::from_be_bytes(*b"RSMB");

pub fn read() -> Option<Tables> {
    let size = unsafe { GetSystemFirmwareTable(RSMB, 0, std::ptr::null_mut(), 0) };
    if size == 0 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    let got = unsafe { GetSystemFirmwareTable(RSMB, 0, buf.as_mut_ptr() as *mut _, size) };
    (got > 0 && got <= size).then(|| parse(&buf[..got as usize]))
}

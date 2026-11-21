//! RAM in use, like Task Manager's "In use".

use pcm_core::model::Memory;
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

pub fn read() -> Option<Memory> {
    let mut m = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    (unsafe { GlobalMemoryStatusEx(&mut m) } != 0 && m.ullTotalPhys > 0)
        .then(|| Memory { total: m.ullTotalPhys, used: m.ullTotalPhys - m.ullAvailPhys })
}

//! Volumes (letters, sizes; USB sticks included, CD drives not) and disk
//! activity from the PhysicalDisk counters.

use pcm_core::model::{DiskIo, Volume};
use windows_sys::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDriveStringsW, GetVolumeInformationW,
};
use windows_sys::Win32::System::Diagnostics::Debug::{SEM_FAILCRITICALERRORS, SetThreadErrorMode};

use crate::pdh::{Counter, Query};
use crate::{from_wide, wide};

const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;

/// "C:\" roots of every drive letter.
fn roots() -> Vec<String> {
    let mut buf = [0u16; 512];
    let n = unsafe { GetLogicalDriveStringsW(buf.len() as u32, buf.as_mut_ptr()) } as usize;
    if n == 0 || n > buf.len() {
        return Vec::new();
    }
    buf[..n].split(|&c| c == 0).filter(|s| !s.is_empty()).map(String::from_utf16_lossy).collect()
}

pub fn volumes() -> Vec<Volume> {
    // an empty card reader must not pop up "insert a disk"
    let mut old = 0;
    unsafe { SetThreadErrorMode(SEM_FAILCRITICALERRORS, &mut old) };
    let mut out = Vec::new();
    for root in roots() {
        if root.starts_with('A') || root.starts_with('B') {
            continue; // floppy letters: slow to ask and never interesting
        }
        let w = wide(&root);
        let kind = unsafe { GetDriveTypeW(w.as_ptr()) };
        if kind != DRIVE_FIXED && kind != DRIVE_REMOVABLE {
            continue;
        }
        let mut label = [0u16; 261];
        let mut fs = [0u16; 32];
        let has_fs = unsafe {
            GetVolumeInformationW(
                w.as_ptr(),
                label.as_mut_ptr(),
                label.len() as u32,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                fs.as_mut_ptr(),
                fs.len() as u32,
            )
        } != 0;
        if !has_fs {
            continue; // no medium or not formatted
        }
        let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
        if unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut avail, &mut total, &mut free) } == 0 || total == 0 {
            continue;
        }
        let label = from_wide(&label);
        out.push(Volume {
            letter: root.trim_end_matches('\\').to_string(),
            label: (!label.is_empty()).then_some(label),
            total,
            free,
            removable: kind == DRIVE_REMOVABLE,
        });
    }
    unsafe { SetThreadErrorMode(old, std::ptr::null_mut()) };
    out
}

pub struct DiskIoReader {
    query: Query,
    read: Counter,
    write: Counter,
    idle: Option<Counter>,
}

impl DiskIoReader {
    pub fn new() -> Option<DiskIoReader> {
        let query = Query::open()?;
        let read = query.add(r"\PhysicalDisk(_Total)\Disk Read Bytes/sec")?;
        let write = query.add(r"\PhysicalDisk(_Total)\Disk Write Bytes/sec")?;
        let idle = query.add(r"\PhysicalDisk(*)\% Idle Time");
        query.collect();
        Some(DiskIoReader { query, read, write, idle })
    }

    pub fn read(&mut self) -> Option<DiskIo> {
        if !self.query.collect() {
            return None;
        }
        let mut active: Vec<(String, f32)> = self
            .idle
            .map(|c| c.values())
            .unwrap_or_default()
            .into_iter()
            .filter(|(n, _)| n != "_Total")
            .map(|(n, idle)| (n, (100.0 - idle).clamp(0.0, 100.0) as f32))
            .collect();
        active.sort_by(|a, b| a.0.cmp(&b.0));
        Some(DiskIo {
            read_bps: self.read.value()?.max(0.0),
            write_bps: self.write.value()?.max(0.0),
            active,
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn system_drive_is_listed() {
        let v = super::volumes();
        let c = v.iter().find(|v| v.letter.eq_ignore_ascii_case("C:")).expect("C:");
        assert!(c.total > c.free && !c.removable);
    }
}

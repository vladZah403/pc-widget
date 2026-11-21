//! NVMe health through IOCTL_STORAGE_QUERY_PROPERTY on a handle opened with no
//! access rights, so it works without admin rights (a port of 1.8). Drives
//! behind Intel RST / VMD or a USB bridge refuse it; then the sensor helper's
//! reading is used.

use std::ptr::{null, null_mut};

use pcm_core::model::{DriveHealth, HealthSource};
use pcm_core::nvme::parse_health;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;

use crate::wide;

const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;
const IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS: u32 = 0x0056_0000;

struct Device(HANDLE);

impl Drop for Device {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

fn open(path: &str) -> Result<Device, u32> {
    let w = wide(path);
    let h = unsafe {
        CreateFileW(w.as_ptr(), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, null(), OPEN_EXISTING, 0, null_mut())
    };
    if h == INVALID_HANDLE_VALUE { Err(unsafe { GetLastError() }) } else { Ok(Device(h)) }
}

fn ioctl(d: &Device, code: u32, input: &[u8], out_size: usize) -> Option<Vec<u8>> {
    let mut out = vec![0u8; out_size];
    let mut got = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            d.0,
            code,
            input.as_ptr() as *const _,
            input.len() as u32,
            out.as_mut_ptr() as *mut _,
            out_size as u32,
            &mut got,
            null_mut(),
        )
    };
    (ok != 0).then(|| {
        out.truncate(got as usize);
        out
    })
}

fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(off..off + 4)?.try_into().ok()?))
}

/// Vendor + product from the storage device descriptor.
fn model(d: &Device) -> String {
    let Some(data) = ioctl(d, IOCTL_STORAGE_QUERY_PROPERTY, &[0u8; 12], 1024) else {
        return String::new();
    };
    let mut names = Vec::new();
    for off in [12, 16] {
        if let Some(o) = u32_at(&data, off).map(|o| o as usize).filter(|&o| o > 0 && o < data.len()) {
            let end = data[o..].iter().position(|&b| b == 0).map_or(data.len(), |e| o + e);
            let s = String::from_utf8_lossy(&data[o..end]).trim().to_string();
            if !s.is_empty() {
                names.push(s);
            }
        }
    }
    names.join(" ")
}

/// STORAGE_PROPERTY_QUERY (StorageDeviceProtocolSpecificProperty) +
/// STORAGE_PROTOCOL_SPECIFIC_DATA asking for NVMe log page 2, then room for it.
fn health_query() -> Vec<u8> {
    let mut q = Vec::with_capacity(8 + 44 + 512);
    for v in [50u32, 0, 3, 2, 2, 0, 44, 512, 0, 0, 0, 0, 0] {
        q.extend(v.to_le_bytes());
    }
    q.resize(8 + 44 + 512, 0);
    q
}

fn log_page(d: &Device) -> Option<Vec<u8>> {
    let q = health_query();
    let data = ioctl(d, IOCTL_STORAGE_QUERY_PROPERTY, &q, q.len())?;
    let offset = u32_at(&data, 8 + 16)? as usize;
    let length = (u32_at(&data, 8 + 20)? as usize).min(512);
    data.get(8 + offset..8 + offset + length).map(<[u8]>::to_vec)
}

/// Health of \\.\PhysicalDrive<n> if it is an NVMe drive that answers.
pub fn health(n: u32) -> Option<DriveHealth> {
    let d = open(&format!(r"\\.\PhysicalDrive{n}")).ok()?;
    let log = parse_health(&log_page(&d)?)?;
    Some(DriveHealth {
        drive: Some(n),
        model: model(&d),
        letters: Vec::new(),
        critical: log.critical,
        temp: log.temp,
        spare: Some(log.spare),
        spare_min: Some(log.spare_min),
        wear: log.wear,
        written: Some(log.written),
        hours: Some(log.hours),
        source: HealthSource::Windows,
    })
}

/// The physical drive of a volume like "C:" (its first extent).
pub fn volume_drive(letter: &str) -> Option<u32> {
    let d = open(&format!(r"\\.\{letter}")).ok()?;
    let data = ioctl(&d, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, &[], 64)?;
    (u32_at(&data, 0)? > 0).then(|| u32_at(&data, 8)).flatten()
}

/// Every NVMe drive that answers, with its letters from `volumes` ("C:").
pub fn read_all(volumes: &[String]) -> Vec<DriveHealth> {
    let mut drives: Vec<DriveHealth> = (0..16).filter_map(health).collect();
    let letters: Vec<(String, Option<u32>)> = volumes.iter().map(|l| (l.clone(), volume_drive(l))).collect();
    for d in &mut drives {
        d.letters = letters.iter().filter(|(_, n)| *n == d.drive).map(|(l, _)| l.clone()).collect();
        d.letters.sort();
    }
    drives
}

/// Letters per physical drive number, to give the helper's drives letters.
pub fn letters_of(volumes: &[String], drive: u32) -> Vec<String> {
    volumes.iter().filter(|l| volume_drive(l) == Some(drive)).cloned().collect()
}

/// What each physical drive answers, for --diag: why there is (or isn't) a
/// health row.
pub fn probe() -> String {
    let mut lines = Vec::new();
    for n in 0..16 {
        match open(&format!(r"\\.\PhysicalDrive{n}")) {
            Err(2) => {}
            Err(e) => lines.push(format!("PhysicalDrive{n}: open failed, error {e}")),
            Ok(d) => {
                let m = model(&d);
                let state = match log_page(&d) {
                    Some(l) => format!("NVMe health log {} bytes", l.len()),
                    None => format!("NVMe query refused, error {}", unsafe { GetLastError() }),
                };
                lines.push(format!("PhysicalDrive{n}: {}: {state}", if m.is_empty() { "?" } else { &m }));
            }
        }
    }
    if lines.is_empty() { "no physical drives found".into() } else { lines.join("; ") }
}

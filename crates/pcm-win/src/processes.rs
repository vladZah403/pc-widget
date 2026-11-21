//! Every process in one NtQuerySystemInformation call (~ms), instead of
//! opening each one: protected processes can't be opened, and that is slow.

use pcm_core::processes::Sample;
use windows_sys::Wdk::System::SystemInformation::{NtQuerySystemInformation, SystemProcessInformation};

const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;

/// The head of SYSTEM_PROCESS_INFORMATION; repr(C) gives the same padding
/// as the real one (x64).
#[repr(C)]
struct ProcInfo {
    next_entry_offset: u32,
    number_of_threads: u32,
    working_set_private_size: i64,
    hard_fault_count: u32,
    number_of_threads_high_watermark: u32,
    cycle_time: u64,
    create_time: i64,
    user_time: i64,
    kernel_time: i64,
    image_name_length: u16,
    image_name_maximum_length: u16,
    image_name_buffer: *const u16,
    base_priority: i32,
    unique_process_id: usize,
}

pub struct ProcessReader {
    buf: Vec<u64>,
}

impl Default for ProcessReader {
    fn default() -> Self {
        ProcessReader { buf: vec![0u64; 1 << 17] } // 1 MB
    }
}

impl ProcessReader {
    pub fn sample(&mut self) -> Vec<Sample> {
        let mut need = 0u32;
        loop {
            let bytes = (self.buf.len() * 8) as u32;
            let status = unsafe {
                NtQuerySystemInformation(
                    SystemProcessInformation,
                    self.buf.as_mut_ptr() as *mut _,
                    bytes,
                    &mut need,
                )
            };
            match status {
                0 => break,
                STATUS_INFO_LENGTH_MISMATCH if need as usize <= 64 << 20 => {
                    self.buf = vec![0u64; (need as usize * 2).div_ceil(8)];
                }
                _ => return Vec::new(),
            }
        }
        let base = self.buf.as_ptr() as *const u8;
        let mut out = Vec::with_capacity(400);
        let mut offset = 0usize;
        loop {
            // SAFETY: Windows filled the buffer with a chain of these records
            let p = unsafe { &*(base.add(offset) as *const ProcInfo) };
            let image = if p.image_name_buffer.is_null() {
                String::new()
            } else {
                let s = unsafe {
                    std::slice::from_raw_parts(p.image_name_buffer, p.image_name_length as usize / 2)
                };
                String::from_utf16_lossy(s)
            };
            out.push(Sample {
                pid: p.unique_process_id as u32,
                image,
                cpu_time: (p.user_time.max(0) + p.kernel_time.max(0)) as u64,
                private_ws: p.working_set_private_size.max(0) as u64,
                created: p.create_time,
            });
            if p.next_entry_offset == 0 {
                break;
            }
            offset += p.next_entry_offset as usize;
            if offset >= self.buf.len() * 8 {
                break;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn this_process_is_listed() {
        let list = super::ProcessReader::default().sample();
        let me = std::process::id();
        assert!(list.len() > 10);
        assert!(list.iter().any(|s| s.pid == me && s.private_ws > 0));
    }
}

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

/// The user SID of a process's token, as bytes.
fn token_user(process: windows_sys::Win32::Foundation::HANDLE) -> Option<Vec<u8>> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Security::{
        GetLengthSid, GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::System::Threading::OpenProcessToken;
    unsafe {
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(process, TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let mut buf = vec![0u64; 64];
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenUser,
            buf.as_mut_ptr() as *mut _,
            (buf.len() * 8) as u32,
            &mut len,
        );
        CloseHandle(token);
        if ok == 0 {
            return None;
        }
        let sid = (*(buf.as_ptr() as *const TOKEN_USER)).User.Sid;
        let n = GetLengthSid(sid) as usize;
        Some(std::slice::from_raw_parts(sid as *const u8, n).to_vec())
    }
}

/// This process's CPU (% of the whole PC since the last read) and private memory.
#[derive(Default)]
pub struct OwnUsage {
    last: Option<(f64, u64)>,
}

impl OwnUsage {
    pub fn read(&mut self) -> (Option<f32>, Option<u64>) {
        use windows_sys::Win32::Foundation::FILETIME;
        use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
        let ft = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
        unsafe {
            let h = GetCurrentProcess();
            let mut t = [FILETIME::default(); 4];
            let cpu = if GetProcessTimes(h, &mut t[0], &mut t[1], &mut t[2], &mut t[3]) != 0 {
                let busy = ft(t[2]) + ft(t[3]); // kernel + user, 100 ns
                let now = crate::now_s();
                let threads = crate::system::threads().max(1) as f64;
                let pct = self.last.filter(|(t0, _)| now > *t0).map(|(t0, b0)| {
                    (busy.saturating_sub(b0) as f64 / 1e7 / (now - t0) / threads * 100.0) as f32
                });
                self.last = Some((now, busy));
                pct
            } else {
                None
            };
            let mut pmc: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
            pmc.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
            let mem = (GetProcessMemoryInfo(h, &mut pmc as *mut _ as *mut _, pmc.cb) != 0)
                .then_some(pmc.PrivateUsage as u64);
            (cpu, mem)
        }
    }
}

/// The program file's name of a process, lower case without ".exe"
/// ("civilizationvi_dx12"); None when it can't be opened.
pub fn image_name(pid: u32) -> Option<String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len) != 0;
        CloseHandle(h);
        if !ok {
            return None;
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]).to_lowercase();
        let file = path.rsplit('\\').next().unwrap_or("");
        Some(file.strip_suffix(".exe").unwrap_or(file).to_string())
    }
}

/// Wait until process `pid` has exited (or `timeout` passed, or it can't be opened).
pub fn wait_for_exit(pid: u32, timeout: std::time::Duration) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if !h.is_null() {
            WaitForSingleObject(h, timeout.as_millis().min(u32::MAX as u128) as u32);
            CloseHandle(h);
        }
    }
}

/// End every process of the program `name` (without ".exe", as the list
/// shows it) that runs under this Windows user — never another user's or the
/// system's, never one of `keep`, never what can't be closed. Each process
/// is checked again by its pid, name and owner right before it is ended (a
/// pid can be reused). Returns how many were ended.
pub fn end_program(name: &str, keep: &[u32]) -> usize {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
        QueryFullProcessImageNameW, TerminateProcess,
    };
    if !pcm_core::processes::can_close(name) {
        return 0;
    }
    let Some(me) = token_user(unsafe { GetCurrentProcess() }) else { return 0 };
    let mut ended = 0;
    for s in ProcessReader::default().sample() {
        let image = s.image.strip_suffix(".exe").or(s.image.strip_suffix(".EXE")).unwrap_or(&s.image);
        if s.pid == 0 || keep.contains(&s.pid) || !image.eq_ignore_ascii_case(name) {
            continue;
        }
        unsafe {
            let h = OpenProcess(PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION, 0, s.pid);
            if h.is_null() {
                continue;
            }
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let same_name = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len) != 0 && {
                let path = String::from_utf16_lossy(&buf[..len as usize]);
                let file = path.rsplit('\\').next().unwrap_or("");
                file.strip_suffix(".exe")
                    .or(file.strip_suffix(".EXE"))
                    .unwrap_or(file)
                    .eq_ignore_ascii_case(name)
            };
            if same_name && token_user(h).as_deref() == Some(me.as_slice()) && TerminateProcess(h, 1) != 0 {
                ended += 1;
            }
            CloseHandle(h);
        }
    }
    ended
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

//! Bytes each process sends and receives, from a real-time ETW session on
//! the Kernel-Network provider (TCP and UDP, IPv4 and IPv6), as Resource
//! Monitor counts them. Only while the Processes page is open.

use std::collections::HashMap;
use std::mem::{size_of, zeroed};
use std::ptr::null;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use windows_sys::Win32::Foundation::ERROR_ALREADY_EXISTS;
use windows_sys::Win32::System::Diagnostics::Etw::*;
use windows_sys::core::GUID;

use crate::frames::Error;
use crate::wide;

pub const SESSION: &str = "PCMonitor2Net";

const KERNEL_NETWORK: GUID = GUID::from_u128(0x7DD42A49_5329_4832_8DFD_43D979153A88);
/// KERNEL_NETWORK_KEYWORD_IPV4 | _IPV6
const KEYWORDS: u64 = 0x30;
/// TCP send and receive (IPv4, IPv6), UDP send and receive (IPv4, IPv6).
const SENT: [u16; 4] = [10, 26, 42, 58];
const RECEIVED: [u16; 4] = [11, 27, 43, 59];

/// Bytes per process id: (received, sent).
type Counts = HashMap<u32, (u64, u64)>;

fn same_guid(a: &GUID, b: &GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

fn properties() -> Vec<u64> {
    let size = size_of::<EVENT_TRACE_PROPERTIES>();
    let bytes = (size + 2 * (SESSION.len() + 1) + 16).div_ceil(8) * 8;
    let mut buf = vec![0u64; bytes / 8];
    let p = buf.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES;
    unsafe {
        (*p).Wnode.BufferSize = bytes as u32;
        (*p).Wnode.Flags = WNODE_FLAG_TRACED_GUID;
        (*p).Wnode.ClientContext = 1;
        (*p).LogFileMode = EVENT_TRACE_REAL_TIME_MODE;
        (*p).FlushTimer = 1;
        (*p).BufferSize = 64; // KB: a download sends many events
        (*p).LoggerNameOffset = size as u32;
    }
    buf
}

/// Stop our session by name (also one left behind by a crashed run).
pub fn stop_session() {
    let mut buf = properties();
    let name = wide(SESSION);
    unsafe {
        ControlTraceW(
            CONTROLTRACE_HANDLE { Value: 0 },
            name.as_ptr(),
            buf.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES,
            EVENT_TRACE_CONTROL_STOP,
        );
    }
}

/// The (pid, size) every one of these events starts with.
fn pid_and_size(rec: &EVENT_RECORD) -> Option<(u32, u32)> {
    if (rec.UserDataLength as usize) < 8 || rec.UserData.is_null() {
        return None;
    }
    let d = unsafe { std::slice::from_raw_parts(rec.UserData as *const u8, 8) };
    Some((u32::from_le_bytes([d[0], d[1], d[2], d[3]]), u32::from_le_bytes([d[4], d[5], d[6], d[7]])))
}

unsafe extern "system" fn on_event(rec: *mut EVENT_RECORD) {
    let rec = unsafe { &*rec };
    let h = &rec.EventHeader;
    if !same_guid(&h.ProviderId, &KERNEL_NETWORK) {
        return;
    }
    let id = h.EventDescriptor.Id;
    let received = RECEIVED.contains(&id);
    if !received && !SENT.contains(&id) {
        return;
    }
    let Some((pid, size)) = pid_and_size(rec) else { return };
    // SAFETY: UserContext is the Mutex the session's thread keeps alive
    let counts = unsafe { &*(rec.UserContext as *const Mutex<Counts>) };
    if let Ok(mut c) = counts.lock() {
        let e = c.entry(pid).or_default();
        if received {
            e.0 += size as u64;
        } else {
            e.1 += size as u64;
        }
    }
}

pub struct NetTraffic {
    counts: Arc<Mutex<Counts>>,
    worker: Option<JoinHandle<()>>,
}

impl NetTraffic {
    /// Start the session and a thread that delivers its events. Needs admin
    /// rights or the Performance Log Users group (Error::AccessDenied).
    pub fn start() -> Result<NetTraffic, Error> {
        let name = wide(SESSION);
        let mut handle = CONTROLTRACE_HANDLE { Value: 0 };
        let mut buf = properties();
        let mut rc = unsafe { StartTraceW(&mut handle, name.as_ptr(), buf.as_mut_ptr() as *mut _) };
        if rc == ERROR_ALREADY_EXISTS {
            stop_session();
            buf = properties();
            rc = unsafe { StartTraceW(&mut handle, name.as_ptr(), buf.as_mut_ptr() as *mut _) };
        }
        match rc {
            0 => {}
            5 => return Err(Error::AccessDenied),
            _ => return Err(Error::Etw("StartTrace", rc)),
        }
        let mut params: ENABLE_TRACE_PARAMETERS = unsafe { zeroed() };
        params.Version = ENABLE_TRACE_PARAMETERS_VERSION_2;
        let rc = unsafe {
            EnableTraceEx2(
                handle,
                &KERNEL_NETWORK,
                EVENT_CONTROL_CODE_ENABLE_PROVIDER,
                TRACE_LEVEL_INFORMATION as u8,
                KEYWORDS,
                0,
                0,
                &params,
            )
        };
        if rc != 0 {
            stop_session();
            return Err(if rc == 5 {
                Error::AccessDenied
            } else {
                Error::Etw("EnableTraceEx2 (Kernel-Network)", rc)
            });
        }
        let counts = Arc::new(Mutex::new(Counts::new()));
        let context = Arc::into_raw(counts.clone()) as usize;
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("etw-net".into())
            .spawn(move || {
                let mut logger = wide(SESSION);
                let mut logfile: EVENT_TRACE_LOGFILEW = unsafe { zeroed() };
                logfile.LoggerName = logger.as_mut_ptr();
                logfile.Anonymous1.ProcessTraceMode =
                    PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD;
                logfile.Anonymous2.EventRecordCallback = Some(on_event);
                logfile.Context = context as *mut _;
                let trace = unsafe { OpenTraceW(&mut logfile) };
                let opened = trace.Value != u64::MAX;
                let _ = tx.send(opened);
                if opened {
                    unsafe {
                        ProcessTrace(&trace, 1, null(), null());
                        CloseTrace(trace);
                    }
                }
                // SAFETY: the reference handed to the callback; no events come any more
                drop(unsafe { Arc::from_raw(context as *const Mutex<Counts>) });
            })
            .map_err(|_| Error::Etw("thread", 0))?;
        if rx.recv() != Ok(true) {
            stop_session();
            let _ = worker.join();
            return Err(Error::Etw("OpenTrace", 0));
        }
        Ok(NetTraffic { counts, worker: Some(worker) })
    }

    /// Bytes per process since the last take: (received, sent).
    pub fn take(&self) -> HashMap<u32, (u64, u64)> {
        self.counts.lock().map(|mut c| std::mem::take(&mut *c)).unwrap_or_default()
    }

    pub fn stop(&mut self) {
        if let Some(w) = self.worker.take() {
            stop_session();
            let _ = w.join();
        }
    }
}

impl Drop for NetTraffic {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_or_says_it_needs_admin() {
        match NetTraffic::start() {
            Ok(mut n) => {
                // some traffic of our own: a connection attempt
                let _ = std::net::TcpStream::connect_timeout(
                    &"1.1.1.1:80".parse().unwrap(),
                    std::time::Duration::from_millis(800),
                );
                std::thread::sleep(std::time::Duration::from_millis(1500));
                let counts = n.take();
                eprintln!("{} processes with traffic", counts.len());
                n.stop();
            }
            Err(e) => {
                eprintln!("not started: {e}");
                assert!(matches!(e, Error::AccessDenied | Error::Etw(..)));
            }
        }
    }
}

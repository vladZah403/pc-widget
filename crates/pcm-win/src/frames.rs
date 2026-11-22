//! Frames per second of every program, from a real-time ETW session on the
//! DXGI (Direct3D 10-12) and DxgKrnl (any API) providers — the way 1.8 and,
//! at its simplest, PresentMon count them. Nothing is injected into games.

use std::mem::{size_of, zeroed};
use std::ptr::null;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

use pcm_core::frames::{FrameStats, Frames, Source};
use windows_sys::Win32::Foundation::ERROR_ALREADY_EXISTS;
use windows_sys::Win32::System::Diagnostics::Etw::*;
use windows_sys::core::GUID;

use crate::{qpc_freq, wide};

/// Not 1.8's "PCMonitorFPS", so both versions can run side by side.
pub const SESSION: &str = "PCMonitor2FPS";

const DXGI: GUID = GUID::from_u128(0xCA11C036_0102_4A2D_A6AD_F03CFED5D3C9);
const DXGI_PRESENT_IDS: [u16; 2] = [42, 55]; // Present_Start, PresentMultiplaneOverlay_Start
const DXGKRNL: GUID = GUID::from_u128(0x802EC45A_1E99_4B83_9920_87C98277BA9D);
const DXGKRNL_PRESENT_INFO: u16 = 184;
const DXGKRNL_BASE: u64 = 0x1; // the keyword that carries Present_Info

fn same_guid(a: &GUID, b: &GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

/// EVENT_TRACE_PROPERTIES followed by room for the session name, 8-byte aligned.
fn properties() -> Vec<u64> {
    let size = size_of::<EVENT_TRACE_PROPERTIES>();
    let bytes = (size + 2 * (SESSION.len() + 1) + 16).div_ceil(8) * 8;
    let mut buf = vec![0u64; bytes / 8];
    let p = buf.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES;
    unsafe {
        (*p).Wnode.BufferSize = bytes as u32;
        (*p).Wnode.Flags = WNODE_FLAG_TRACED_GUID;
        (*p).Wnode.ClientContext = 1; // timestamps in QPC units
        (*p).LogFileMode = EVENT_TRACE_REAL_TIME_MODE;
        (*p).FlushTimer = 1; // deliver at least once a second
        (*p).BufferSize = 16; // KB
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

/// EVENT_FILTER_EVENT_ID with N ids: only these events leave the provider.
#[repr(C)]
struct EventIdFilter<const N: usize> {
    filter_in: u8,
    reserved: u8,
    count: u16,
    events: [u16; N],
}

fn enable<const N: usize>(session: u64, provider: &GUID, keywords: u64, ids: [u16; N]) -> u32 {
    let filter = EventIdFilter { filter_in: 1, reserved: 0, count: N as u16, events: ids };
    let mut desc = EVENT_FILTER_DESCRIPTOR {
        Ptr: &filter as *const _ as u64,
        Size: size_of::<EventIdFilter<N>>() as u32,
        Type: EVENT_FILTER_TYPE_EVENT_ID,
    };
    let mut params: ENABLE_TRACE_PARAMETERS = unsafe { zeroed() };
    params.Version = ENABLE_TRACE_PARAMETERS_VERSION_2;
    params.EnableFilterDesc = &mut desc;
    params.FilterDescCount = 1;
    unsafe {
        EnableTraceEx2(
            CONTROLTRACE_HANDLE { Value: session },
            provider,
            EVENT_CONTROL_CODE_ENABLE_PROVIDER,
            TRACE_LEVEL_INFORMATION as u8,
            keywords,
            0,
            0,
            &params,
        )
    }
}

unsafe extern "system" fn on_event(rec: *mut EVENT_RECORD) {
    let rec = unsafe { &*rec };
    let h = &rec.EventHeader;
    let id = h.EventDescriptor.Id;
    let source = if same_guid(&h.ProviderId, &DXGI) {
        if !DXGI_PRESENT_IDS.contains(&id) {
            return;
        }
        Source::Dxgi
    } else if id == DXGKRNL_PRESENT_INFO && same_guid(&h.ProviderId, &DXGKRNL) {
        Source::Kernel
    } else {
        return;
    };
    // SAFETY: UserContext is the Mutex the session's thread keeps alive
    let frames = unsafe { &*(rec.UserContext as *const Mutex<Frames>) };
    if let Ok(mut f) = frames.lock() {
        f.add(h.ProcessId, source, h.TimeStamp as f64 / qpc_freq());
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// Needs admin rights (or the Performance Log Users group).
    AccessDenied,
    Etw(&'static str, u32),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Error::AccessDenied => write!(f, "an ETW session needs admin rights"),
            Error::Etw(what, rc) => write!(f, "{what} failed with {rc}"),
        }
    }
}

pub struct FrameCounter {
    frames: Arc<Mutex<Frames>>,
    worker: Option<JoinHandle<()>>,
    /// The kernel provider may be refused; Direct3D 10-12 still work then.
    pub kernel_ok: bool,
}

impl FrameCounter {
    /// Start the session and a thread that delivers its events.
    pub fn start() -> Result<FrameCounter, Error> {
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
        let rc = enable(handle.Value, &DXGI, u64::MAX, DXGI_PRESENT_IDS);
        if rc != 0 {
            stop_session();
            return Err(Error::Etw("EnableTraceEx2 (DXGI)", rc));
        }
        let kernel_ok = enable(handle.Value, &DXGKRNL, DXGKRNL_BASE, [DXGKRNL_PRESENT_INFO]) == 0;

        let frames = Arc::new(Mutex::new(Frames::default()));
        let context = Arc::into_raw(frames.clone()) as usize;
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("etw-frames".into())
            .spawn(move || {
                let mut logger = wide(SESSION);
                let mut logfile: EVENT_TRACE_LOGFILEW = unsafe { zeroed() };
                logfile.LoggerName = logger.as_mut_ptr();
                // RAW_TIMESTAMP: event times stay on the QPC clock (ClientContext 1), the
                // clock of now_s(); without it ETW turns them into system time (FILETIME),
                // which only looks right because both tick 10 MHz on Windows 10/11
                logfile.Anonymous1.ProcessTraceMode = PROCESS_TRACE_MODE_REAL_TIME
                    | PROCESS_TRACE_MODE_EVENT_RECORD
                    | PROCESS_TRACE_MODE_RAW_TIMESTAMP;
                logfile.Anonymous2.EventRecordCallback = Some(on_event);
                logfile.Context = context as *mut _;
                let trace = unsafe { OpenTraceW(&mut logfile) };
                let opened = trace.Value != u64::MAX;
                let _ = tx.send(opened);
                if opened {
                    unsafe {
                        ProcessTrace(&trace, 1, null(), null()); // until the session stops
                        CloseTrace(trace);
                    }
                }
                // SAFETY: the reference handed to the callback; no events come any more
                drop(unsafe { Arc::from_raw(context as *const Mutex<Frames>) });
            })
            .map_err(|_| Error::Etw("thread", 0))?;
        if rx.recv() != Ok(true) {
            stop_session();
            let _ = worker.join();
            return Err(Error::Etw("OpenTrace", 0));
        }
        Ok(FrameCounter { frames, worker: Some(worker), kernel_ok })
    }

    pub fn frames(&self) -> MutexGuard<'_, Frames> {
        self.frames.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// FPS, 1% low and frame times of `pid`, with the source they came from.
    pub fn stats(&self, pid: u32) -> Option<(FrameStats, Source)> {
        self.frames().stats(pid, crate::now_s())
    }

    pub fn stop(&mut self) {
        if let Some(w) = self.worker.take() {
            stop_session();
            let _ = w.join();
        }
    }
}

impl Drop for FrameCounter {
    fn drop(&mut self) {
        self.stop();
    }
}

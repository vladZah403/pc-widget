//! Real-time ETW session on the DXGI and DxgKrnl providers, ported from
//! FpsReader in PC Monitor 1.8. Every frame a game presents raises one event;
//! the callback stores its QPC time per (pid, source).

use std::mem::{size_of, zeroed};
use std::ptr::null;
use std::sync::{LazyLock, Mutex, OnceLock};

use windows_sys::Win32::Foundation::ERROR_ALREADY_EXISTS;
use windows_sys::Win32::System::Diagnostics::Etw::*;
use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows_sys::core::GUID;

use crate::stats::{Frames, Source};

/// Not 1.8's "PCMonitorFPS", so both versions can run side by side.
pub const SESSION: &str = "PCMonitor2FPS";

const DXGI: GUID = GUID::from_u128(0xCA11C036_0102_4A2D_A6AD_F03CFED5D3C9);
const DXGI_PRESENT_IDS: [u16; 2] = [42, 55]; // Present_Start, PresentMultiplaneOverlay_Start
const DXGKRNL: GUID = GUID::from_u128(0x802EC45A_1E99_4B83_9920_87C98277BA9D);
const DXGKRNL_PRESENT_INFO: u16 = 184;
const DXGKRNL_BASE: u64 = 0x1; // the keyword that carries Present_Info

pub static FRAMES: LazyLock<Mutex<Frames>> = LazyLock::new(Default::default);

fn qpc_freq() -> f64 {
    static FREQ: OnceLock<f64> = OnceLock::new();
    *FREQ.get_or_init(|| {
        let mut f = 0i64;
        unsafe { QueryPerformanceFrequency(&mut f) };
        f as f64
    })
}

/// Now on the clock of the event timestamps, in seconds.
pub fn now_s() -> f64 {
    let mut c = 0i64;
    unsafe { QueryPerformanceCounter(&mut c) };
    c as f64 / qpc_freq()
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn same_guid(a: &GUID, b: &GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

/// EVENT_TRACE_PROPERTIES followed by room for the session name, 8-byte aligned.
fn properties() -> Vec<u64> {
    let size = size_of::<EVENT_TRACE_PROPERTIES>();
    let bytes = size + 2 * (SESSION.len() + 1) + 16;
    let mut buf = vec![0u64; bytes.div_ceil(8)];
    let p = buf.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES;
    unsafe {
        (*p).Wnode.BufferSize = (bytes.div_ceil(8) * 8) as u32;
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
    let h = unsafe { &(*rec).EventHeader };
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
    let t = h.TimeStamp as f64 / qpc_freq();
    if let Ok(mut frames) = FRAMES.lock() {
        frames.add(h.ProcessId, source, t);
    }
}

#[derive(Debug)]
pub enum Error {
    NeedsAdmin,
    Etw(&'static str, u32),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Error::NeedsAdmin => write!(f, "an ETW session needs admin rights (or Performance Log Users)"),
            Error::Etw(what, rc) => write!(f, "{what} failed with {rc}"),
        }
    }
}

pub struct Session {
    trace: u64,
    /// The kernel provider may be refused; DX10-12 still work through DXGI.
    pub kernel_ok: bool,
}

/// Start the session and enable both providers.
pub fn start() -> Result<Session, Error> {
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
        5 => return Err(Error::NeedsAdmin),
        _ => return Err(Error::Etw("StartTrace", rc)),
    }
    let rc = enable(handle.Value, &DXGI, u64::MAX, DXGI_PRESENT_IDS);
    if rc != 0 {
        stop_session();
        return Err(Error::Etw("EnableTraceEx2 (DXGI)", rc));
    }
    let kernel_ok = enable(handle.Value, &DXGKRNL, DXGKRNL_BASE, [DXGKRNL_PRESENT_INFO]) == 0;

    let mut logfile: EVENT_TRACE_LOGFILEW = unsafe { zeroed() };
    let mut logger = wide(SESSION);
    logfile.LoggerName = logger.as_mut_ptr();
    logfile.Anonymous1.ProcessTraceMode = PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD;
    logfile.Anonymous2.EventRecordCallback = Some(on_event);
    let trace = unsafe { OpenTraceW(&mut logfile) };
    if trace.Value == u64::MAX {
        stop_session();
        return Err(Error::Etw("OpenTrace", u32::MAX));
    }
    Ok(Session { trace: trace.Value, kernel_ok })
}

impl Session {
    /// Deliver events to the callback; blocks until the session is stopped.
    pub fn process(&self) -> u32 {
        let handles = [PROCESSTRACE_HANDLE { Value: self.trace }];
        let rc = unsafe { ProcessTrace(handles.as_ptr(), 1, null(), null()) };
        unsafe { CloseTrace(PROCESSTRACE_HANDLE { Value: self.trace }) };
        rc
    }
}

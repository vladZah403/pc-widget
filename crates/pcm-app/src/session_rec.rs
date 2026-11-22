//! Records game sessions: every frame of the game's process from the moment
//! it is seen until it exits (or stops drawing for IDLE_END), and the
//! readings once a second; then the session file in the background.

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use pcm_core::frames::Source;
use pcm_core::session::{
    self, MIN_ACTIVE_S, Sample, Session, SessionInfo, VERSION, fill_fps, frame_times, summarize,
};
use pcm_render::widget::Data;
use pcm_win::frames::FrameCounter;
use windows_sys::Win32::Foundation::{HANDLE, HWND, RECT, WAIT_OBJECT_0};
use windows_sys::Win32::Graphics::Gdi::{
    DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST,
    MONITORINFOEXW, MonitorFromWindow,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, GetWindowTextW, PostMessageW};

use crate::game::Game;

/// A game that hasn't drawn a frame for this long is over (minimised for good).
const IDLE_END_S: f64 = 300.0;
/// Tells the window a session file was written (or turned out too short).
pub const WM_APP_SESSION: u32 = 0x8000 + 5;

const SOURCES: [Source; 2] = [Source::Dxgi, Source::Kernel];

pub struct Recording {
    pid: u32,
    info: SessionInfo,
    /// To see when the game exits.
    process: Option<OwnedHandle>,
    /// QPC seconds of the start; frame and sample times are taken from it.
    t0: f64,
    /// Present times (QPC s) per source; the one with more frames is kept.
    times: [Vec<f64>; 2],
    last_frame: f64,
    samples: Vec<Sample>,
}

fn window_title(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) }.max(0) as usize;
    String::from_utf16_lossy(&buf[..n])
}

fn refresh_hz(hwnd: HWND) -> u32 {
    unsafe {
        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFOEXW::default();
        mi.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(mon, &mut mi as *mut _ as *mut _) == 0 {
            return 0;
        }
        let mut dm = DEVMODEW { dmSize: size_of::<DEVMODEW>() as u16, ..Default::default() };
        if EnumDisplaySettingsW(mi.szDevice.as_ptr(), ENUM_CURRENT_SETTINGS, &mut dm) == 0 {
            return 0;
        }
        dm.dmDisplayFrequency
    }
}

#[derive(Default)]
pub struct Recorder {
    rec: Option<Recording>,
}

impl Recorder {
    /// The game being recorded.
    pub fn game(&self) -> Option<&str> {
        self.rec.as_ref().map(|r| r.info.game.as_str())
    }

    /// A game is in front: record it, finishing another game's session first.
    pub fn seen(&mut self, g: &Game, d: Option<&Data>) -> Option<Recording> {
        if self.rec.as_ref().is_some_and(|r| r.pid == g.pid) {
            return None;
        }
        let done = self.rec.take();
        let mut client = RECT::default();
        unsafe { GetClientRect(g.hwnd, &mut client) };
        let info = SessionInfo {
            version: VERSION,
            game: g.name.clone(),
            title: window_title(g.hwnd),
            started: pcm_win::system::local_time(),
            width: (client.right - client.left).max(0) as u32,
            height: (client.bottom - client.top).max(0) as u32,
            refresh_hz: refresh_hz(g.hwnd),
            cpu: pcm_win::system::cpu_name().unwrap_or_default(),
            gpu: d.and_then(|d| d.gpu_name.clone()).unwrap_or_default(),
            ram_gb: d.map_or(0.0, |d| d.ram_total as f32 / (1u64 << 30) as f32),
            windows: pcm_win::system::windows_build().map(|b| format!("build {b}")).unwrap_or_default(),
            ..SessionInfo::default()
        };
        let h = unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, 0, g.pid) };
        let process = (!h.is_null()).then(|| unsafe { OwnedHandle::from_raw_handle(h as _) });
        self.rec = Some(Recording {
            pid: g.pid,
            info,
            process,
            t0: pcm_win::now_s(),
            times: [Vec::new(), Vec::new()],
            last_frame: pcm_win::now_s(),
            samples: Vec::new(),
        });
        done
    }

    /// Take the game's new frames (the counter keeps ~14 s of them; this runs
    /// 4 times a second). Returns the session when the game is over.
    pub fn poll(&mut self, counter: Option<&FrameCounter>) -> Option<Recording> {
        let rec = self.rec.as_mut()?;
        let Some(counter) = counter else { return self.rec.take() };
        {
            let frames = counter.frames();
            for (i, src) in SOURCES.iter().enumerate() {
                let Some(all) = frames.map.get(&(rec.pid, *src)) else { continue };
                let last = rec.times[i].last().copied().unwrap_or(f64::MIN);
                let from = all.partition_point(|&t| t <= last);
                rec.times[i].extend_from_slice(&all[from..]);
                if let Some(&t) = all.last() {
                    rec.last_frame = rec.last_frame.max(t);
                }
            }
        }
        let exited = rec
            .process
            .as_ref()
            .is_none_or(|h| unsafe { WaitForSingleObject(h.as_raw_handle() as HANDLE, 0) } == WAIT_OBJECT_0);
        let idle = pcm_win::now_s() - rec.last_frame > IDLE_END_S;
        if exited || idle { self.rec.take() } else { None }
    }

    /// The readings of this second.
    pub fn sample(&mut self, d: &Data) {
        let Some(rec) = self.rec.as_mut() else { return };
        rec.samples.push(Sample {
            t: (pcm_win::now_s() - rec.t0) as f32,
            fps: None,
            cpu: Some(d.cpu),
            gpu: d.gpu_load,
            cpu_t: d.temp_cpu,
            gpu_t: d.temp_gpu,
            ram: Some(d.ram_pct),
        });
    }

    /// Stop now (the program exits or frame counting is switched off).
    pub fn stop(&mut self) -> Option<Recording> {
        self.rec.take()
    }
}

/// The session of a finished recording; None when it was too short to keep.
pub fn build(rec: Recording) -> Option<Session> {
    let i = if rec.times[0].len() >= rec.times[1].len() { 0 } else { 1 };
    let times: Vec<f64> = rec.times[i].iter().map(|t| t - rec.t0).collect();
    if times.len() < 2 {
        return None;
    }
    let mut info = rec.info.clone();
    let mut samples = rec.samples.clone();
    fill_fps(&mut samples, &times);
    let frame_ms = frame_times(&times);
    info.source = SOURCES[i].name().into();
    info.duration_s = times[times.len() - 1] - times[0];
    info.summary = summarize(&frame_ms, &samples);
    info.samples = samples;
    (info.summary.active_s >= MIN_ACTIVE_S).then_some(Session { info, frame_ms })
}

pub fn sessions_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("sessions")
}

/// Build and save the session off the window's thread, then tell the window
/// (WM_APP_SESSION) with the result in `done`.
pub fn finish_in_background(rec: Recording, dir: PathBuf, done: Arc<Mutex<Vec<SessionInfo>>>, hwnd: HWND) {
    let hwnd = hwnd as isize;
    std::thread::Builder::new()
        .name("session".into())
        .spawn(move || {
            if let Some(s) = build(rec) {
                let path = dir.join(session::file_name(&s.info.started, &s.info.game));
                if s.save(&path).is_ok()
                    && let Ok(mut d) = done.lock()
                {
                    d.push(s.info);
                    unsafe { PostMessageW(hwnd as HWND, WM_APP_SESSION, 0, 0) };
                }
            }
        })
        .ok();
}

/// Write a session now, on this thread (the program is closing).
pub fn finish_now(rec: Recording, dir: &Path) {
    if let Some(s) = build(rec) {
        let _ = s.save(&dir.join(session::file_name(&s.info.started, &s.info.game)));
    }
}

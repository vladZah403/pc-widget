//! pcm-sensors.exe, the LibreHardwareMonitor helper: started only from the
//! program's own folder by its full path, talked to only through its stdout.

use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use pcm_core::sensors::{Hardware, Message, PROTOCOL, parse_line};

pub const EXE: &str = "pcm-sensors.exe";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The helper next to PC-Monitor.exe: `_internal\pcm-sensors.exe` (the
/// release layout) or `pcm-sensors.exe`.
pub fn find() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    [dir.join("_internal").join(EXE), dir.join(EXE)].into_iter().find(|p| p.is_file())
}

#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    pub lhm: String,
    pub admin: bool,
    pub hardware: Vec<Hardware>,
    pub read_ms: f64,
    pub mem_mb: f64,
    pub errors: Vec<String>,
}

/// Start the helper for one reading of `parts` ("cpu,gpu,board,storage").
/// LibreHardwareMonitor takes a few seconds to open as admin.
pub fn read_once(path: &Path, parts: &str, timeout: Duration) -> Result<Reading, String> {
    let mut child = Command::new(path)
        .args(["--once", "--parts", parts])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("can't start {}: {e}", path.display()))?;
    let mut stdout = child.stdout.take().ok_or("no stdout")?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < timeout => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("no answer in {} s", timeout.as_secs()));
            }
        }
    }
    let text = reader.join().map_err(|_| "reader thread failed")?;
    let mut r = Reading {
        lhm: String::new(),
        admin: false,
        hardware: Vec::new(),
        read_ms: 0.0,
        mem_mb: 0.0,
        errors: Vec::new(),
    };
    let mut hello = false;
    for msg in text.lines().filter_map(parse_line) {
        match msg {
            Message::Hello { protocol, lhm, admin } => {
                if protocol != PROTOCOL {
                    return Err(format!("protocol {protocol}, expected {PROTOCOL}"));
                }
                hello = true;
                r.lhm = lhm;
                r.admin = admin;
            }
            Message::Sensors { hw, read_ms, mem_mb, .. } => {
                r.hardware = hw;
                r.read_ms = read_ms;
                r.mem_mb = mem_mb;
            }
            Message::Error { message } => r.errors.push(message),
        }
    }
    if !hello {
        return Err(format!("no greeting; output: {:?}", text.chars().take(200).collect::<String>()));
    }
    Ok(r)
}

/// Pauses before restarting a helper that ended: 5 s, 30 s, then 5 min.
const BACKOFF: [Duration; 3] = [Duration::from_secs(5), Duration::from_secs(30), Duration::from_secs(300)];
/// A run this long counts as healthy: the next failure starts over at 5 s.
const HEALTHY: Duration = Duration::from_secs(120);

#[derive(Default)]
struct Shared {
    latest: Option<Reading>,
    stop: bool,
    /// Why the helper isn't giving data, for the log and --diag.
    last_error: Option<String>,
    restarts: u32,
}

/// The helper running in the background, one reading every `interval_ms`.
/// Dropping it ends the helper (its stdin closes).
pub struct Helper {
    shared: std::sync::Arc<std::sync::Mutex<Shared>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Helper {
    pub fn start(path: PathBuf, parts: &str, interval_ms: u32) -> Helper {
        let shared = std::sync::Arc::new(std::sync::Mutex::new(Shared::default()));
        let s = shared.clone();
        let parts = parts.to_string();
        let worker = std::thread::Builder::new()
            .name("sensors-helper".into())
            .spawn(move || run(&path, &parts, interval_ms, &s))
            .ok();
        Helper { shared, worker }
    }

    /// The newest reading (None until the first one arrives).
    pub fn latest(&self) -> Option<Reading> {
        self.shared.lock().ok()?.latest.clone()
    }

    pub fn last_error(&self) -> Option<String> {
        self.shared.lock().ok()?.last_error.clone()
    }

    pub fn restarts(&self) -> u32 {
        self.shared.lock().map_or(0, |s| s.restarts)
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        if let Ok(mut s) = self.shared.lock() {
            s.stop = true;
        }
        // the worker notices within a second (its child ends when we close its stdin)
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

fn stopped(s: &std::sync::Mutex<Shared>) -> bool {
    s.lock().map_or(true, |s| s.stop)
}

fn run(path: &Path, parts: &str, interval_ms: u32, shared: &std::sync::Mutex<Shared>) {
    use std::io::BufRead;
    let mut failures = 0usize;
    while !stopped(shared) {
        let started = Instant::now();
        let child = Command::new(path)
            .args(["--interval", &interval_ms.to_string(), "--parts", parts])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
        let error = match child {
            Err(e) => format!("can't start {}: {e}", path.display()),
            Ok(mut child) => {
                let stdout = child.stdout.take();
                let (tx, rx) = std::sync::mpsc::channel::<String>();
                // lines arrive on their own thread so this one can watch `stop`
                let reader = stdout.map(|out| {
                    std::thread::spawn(move || {
                        for line in std::io::BufReader::new(out).lines() {
                            let Ok(line) = line else { break };
                            if tx.send(line).is_err() {
                                break;
                            }
                        }
                    })
                });
                let (mut lhm, mut admin) = (String::new(), false);
                let mut errors: Vec<String> = Vec::new();
                let mut reason = "the helper ended".to_string();
                loop {
                    if stopped(shared) {
                        break;
                    }
                    match rx.recv_timeout(Duration::from_millis(500)) {
                        Ok(line) => match parse_line(&line) {
                            Some(Message::Hello { protocol, lhm: l, admin: a }) => {
                                if protocol != PROTOCOL {
                                    reason = format!("protocol {protocol}, expected {PROTOCOL}");
                                    break;
                                }
                                (lhm, admin) = (l, a);
                            }
                            Some(Message::Sensors { hw, read_ms, mem_mb, .. }) => {
                                let r = Reading {
                                    lhm: lhm.clone(),
                                    admin,
                                    hardware: hw,
                                    read_ms,
                                    mem_mb,
                                    errors: std::mem::take(&mut errors),
                                };
                                if let Ok(mut s) = shared.lock() {
                                    s.latest = Some(r);
                                    s.last_error = None;
                                }
                            }
                            Some(Message::Error { message }) if errors.len() < 20 => errors.push(message),
                            Some(Message::Error { .. }) | None => {}
                        },
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
                drop(child.stdin.take()); // the helper quits when its stdin closes
                let deadline = Instant::now() + Duration::from_secs(3);
                while matches!(child.try_wait(), Ok(None)) && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(50));
                }
                let _ = child.kill();
                let _ = child.wait();
                if let Some(r) = reader {
                    let _ = r.join();
                }
                reason
            }
        };
        if stopped(shared) {
            break;
        }
        if started.elapsed() >= HEALTHY {
            failures = 0;
        }
        let pause = BACKOFF[failures.min(BACKOFF.len() - 1)];
        failures += 1;
        if let Ok(mut s) = shared.lock() {
            s.last_error = Some(error);
            s.restarts += 1;
        }
        let until = Instant::now() + pause;
        while Instant::now() < until && !stopped(shared) {
            std::thread::sleep(Duration::from_millis(250));
        }
    }
}

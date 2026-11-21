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

//! Autostart and admin rights (1.8's rules, ported as they are).
//!
//! Whatever runs with admin rights must not be replaceable without them, or
//! any program running as the user could swap a file (the .exe, the sensor
//! helper) and get admin rights at the next start. So in admin mode the
//! program runs from a copy in Program Files, and only that copy may
//! autostart elevated (a logon task with the highest rights: no UAC prompt).
//! Otherwise autostart is the per-user Run key.

use std::path::{Path, PathBuf};
use std::ptr::null;
use std::sync::atomic::{AtomicIsize, Ordering};

use pcm_win::autostart as os;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, GetLastError, HANDLE,
};
use windows_sys::Win32::System::Threading::CreateMutexW;

/// While 2.0 is tried out next to 1.8 it has names of its own, so it
/// neither takes over 1.8's autostart, admin task and Program Files copy nor
/// keeps 1.8 from running. The release sets this to false: the names are
/// then 1.8's, and an update from 1.8 keeps the user's autostart.
pub const BESIDE_1_8: bool = true;
pub const RUN_NAME: &str = if BESIDE_1_8 { "PCMonitor2Widget" } else { "PCMonitorWidget" };
pub const TASK_NAME: &str = RUN_NAME;
pub const INSTALL_FOLDER: &str = if BESIDE_1_8 { "PC-Monitor 2" } else { "PC-Monitor" };
const MUTEX: &str = if BESIDE_1_8 { "Local\\PCMonitor2Widget" } else { "Local\\PCMonitorWidget" };
const EXE_NAME: &str = "PC-Monitor.exe";
/// Started with Windows: quietly (no PC Monitor window), unlike by hand.
pub const AUTOSTART_ARG: &str = "--autostart";
const INTERNAL: &str = "_internal";

// ---------- one widget at a time ----------

static INSTANCE: AtomicIsize = AtomicIsize::new(0);

/// Is another widget running? Takes the single-instance lock when not.
pub fn already_running() -> bool {
    release_instance(); // our own handle would make it look taken
    let name: Vec<u16> = MUTEX.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let h = CreateMutexW(null(), 0, name.as_ptr());
        let err = GetLastError();
        INSTANCE.store(h as isize, Ordering::Relaxed);
        // "access denied": it exists, made by an elevated copy
        err == ERROR_ALREADY_EXISTS || (h.is_null() && err == ERROR_ACCESS_DENIED)
    }
}

/// Let go of the lock (a copy we start is to take it).
pub fn release_instance() {
    let h = INSTANCE.swap(0, Ordering::Relaxed);
    if h != 0 {
        unsafe { CloseHandle(h as HANDLE) };
    }
}

// ---------- where we run from ----------

pub(crate) fn this_exe() -> Option<PathBuf> {
    std::env::current_exe().ok()
}

pub(crate) fn app_dir() -> Option<PathBuf> {
    this_exe()?.parent().map(Path::to_path_buf)
}

pub fn install_dir() -> PathBuf {
    os::program_files().join(INSTALL_FOLDER)
}

fn installed_exe() -> Option<PathBuf> {
    Some(install_dir().join(EXE_NAME)).filter(|p| p.is_file())
}

/// The same file or folder, whatever the case of the letters.
fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        let p = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
        p.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase()
    };
    norm(a) == norm(b)
}

/// The protected copy in Program Files is what runs.
pub fn running_installed() -> bool {
    app_dir().is_some_and(|d| same_path(&d, &install_dir()))
}

/// A release folder (PC-Monitor.exe + _internal), not a bare build.
pub(crate) fn is_release_folder(dir: &Path) -> bool {
    dir.join(INTERNAL).is_dir()
}

/// Everything in `dir` (recursively): relative names and sizes, sorted.
fn listing(dir: &Path) -> std::io::Result<Vec<(String, u64)>> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, u64)>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                walk(root, &path, out)?;
            } else {
                let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().to_lowercase();
                out.push((rel, entry.metadata()?.len()));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out)?;
    out.sort();
    Ok(out)
}

/// Two release folders hold the same build: the same .exe byte for byte
/// (it carries the code) and the same files in _internal (1.8 compares
/// names and sizes there too).
fn same_build(a: &Path, b: &Path) -> bool {
    let exe = |d: &Path| std::fs::read(d.join(EXE_NAME)).ok();
    let (Some(ea), Some(eb)) = (exe(a), exe(b)) else { return false };
    ea == eb
        && matches!((listing(&a.join(INTERNAL)), listing(&b.join(INTERNAL))), (Ok(la), Ok(lb)) if la == lb)
}

/// The Program Files copy is this very build.
fn installed_is_current() -> bool {
    if running_installed() {
        return true;
    }
    match (app_dir(), installed_exe()) {
        (Some(here), Some(_)) => same_build(&here, &install_dir()),
        _ => false,
    }
}

pub(crate) fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// A sibling folder: "C:\Program Files\PC-Monitor" + ".new".
pub(crate) fn with_suffix(dir: &Path, suffix: &str) -> PathBuf {
    let mut s = dir.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Copy this build (the .exe and _internal) to Program Files. Needs admin
/// rights. The old copy is swapped out only once the new one is complete,
/// and put back if the swap fails (e.g. that copy is running).
fn install_copy() -> bool {
    let (Some(exe), Some(here)) = (this_exe(), app_dir()) else { return false };
    if running_installed() || !is_release_folder(&here) {
        return false;
    }
    let dest = install_dir();
    let (new, old) = (with_suffix(&dest, ".new"), with_suffix(&dest, ".old"));
    for p in [&new, &old] {
        let _ = std::fs::remove_dir_all(p);
    }
    let copied = std::fs::create_dir_all(&new)
        .and_then(|_| std::fs::copy(&exe, new.join(EXE_NAME)).map(|_| ()))
        .and_then(|_| copy_dir(&here.join(INTERNAL), &new.join(INTERNAL)));
    let swapped = copied.and_then(|_| {
        if dest.exists() {
            std::fs::rename(&dest, &old)?;
        }
        std::fs::rename(&new, &dest)
    });
    if swapped.is_err() {
        let _ = std::fs::remove_dir_all(&new);
        if !dest.exists() && old.exists() {
            let _ = std::fs::rename(&old, &dest); // the previous copy back
        }
        return false;
    }
    let _ = std::fs::remove_dir_all(&old);
    true
}

/// Start the Program Files copy (it inherits our admin rights).
fn start_installed(args: &[&str]) -> bool {
    let Some(exe) = installed_exe() else { return false };
    std::process::Command::new(exe).args(args).current_dir(install_dir()).spawn().is_ok()
}

/// Ask UAC to start us elevated: the protected copy when it is this build.
/// True if the elevated copy was started.
fn relaunch_as_admin(args: &str, owner: isize) -> bool {
    let exe = if !running_installed() && installed_is_current() { installed_exe() } else { this_exe() };
    let Some(exe) = exe else { return false };
    let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    os::start_as_admin(&exe, args, &dir, owner)
}

// ---------- autostart ----------

/// What the Run key starts: this program, quietly.
fn autostart_command() -> String {
    this_exe().map(|p| format!("\"{}\" {AUTOSTART_ARG}", p.display())).unwrap_or_default()
}

pub fn run_key_enabled() -> bool {
    os::run_entry(RUN_NAME).is_some()
}

fn set_run_key(on: bool) -> bool {
    if on {
        os::set_run_entry(RUN_NAME, Some(&autostart_command()))
    } else {
        os::set_run_entry(RUN_NAME, None)
    }
}

/// Text for XML.
fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// A logon task with the highest rights: elevated autostart without a UAC
/// prompt. Battery and time limits off (schtasks' defaults would stop it on
/// laptops). 1.8's task, word for word.
fn task_xml(user: &str, exe: &str, dir: &str) -> String {
    let (user, exe, dir) = (escape(user), escape(exe), escape(dir));
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{user}</UserId>
    <Delay>PT5S</Delay></LogonTrigger></Triggers>
  <Principals><Principal id="Author"><UserId>{user}</UserId>
    <LogonType>InteractiveToken</LogonType><RunLevel>HighestAvailable</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author"><Exec><Command>{exe}</Command>
    <Arguments>{AUTOSTART_ARG}</Arguments><WorkingDirectory>{dir}</WorkingDirectory></Exec></Actions>
</Task>"#
    )
}

/// The text of <name> in a task's XML (None: no such element).
fn tag(xml: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let start = xml.find(&open)? + open.len();
    let len = xml[start..].find(&format!("</{name}>"))?;
    Some(unescape(xml[start..start + len].trim()))
}

/// The program in a task's XML (None: no <Command>).
fn command_of(xml: &str) -> Option<String> {
    tag(xml, "Command").map(|c| c.trim_matches('"').to_string())
}

pub fn task_enabled() -> bool {
    os::task_exists(TASK_NAME)
}

/// The program the elevated autostart task starts ("" when it can't be
/// read) and its arguments, or None when there is no task.
fn task_command() -> Option<(String, String)> {
    os::task_xml(TASK_NAME)
        .map(|x| (command_of(&x).unwrap_or_default(), tag(&x, "Arguments").unwrap_or_default()))
}

/// Create or delete the task. Needs admin rights (we have them when this is
/// called). Only the protected copy may get one.
fn set_task(on: bool) -> bool {
    if !on {
        return os::delete_task(TASK_NAME) || !task_enabled();
    }
    if !running_installed() {
        return false;
    }
    let (Some(exe), dir) = (this_exe(), install_dir()) else { return false };
    let xml = task_xml(&os::user_name(), &exe.to_string_lossy(), &dir.to_string_lossy());
    // the file goes into the protected program folder, not %TEMP%: there
    // another program could swap it between our write and schtasks reading it
    let path = dir.join("autostart-task.xml");
    let mut bytes = vec![0xff, 0xfe];
    bytes.extend(xml.encode_utf16().flat_map(u16::to_le_bytes));
    let done = std::fs::write(&path, bytes).is_ok() && os::create_task(TASK_NAME, &path);
    let _ = std::fs::remove_file(&path);
    done
}

/// Does PC Monitor start with Windows (either way)?
pub fn autostart_enabled() -> bool {
    run_key_enabled() || task_enabled()
}

/// At start: keep the Run entry pointing at this program, drop an elevated
/// task that would start something an ordinary program could replace, and
/// (admin mode, protected copy) move autostart to the task, so logon shows
/// no UAC prompt.
pub fn check_autostart(prefer_task: bool) {
    if run_key_enabled() {
        set_run_key(true);
    }
    if !pcm_win::system::is_admin() {
        return;
    }
    let mut cmd = task_command();
    let installed = installed_exe();
    if let Some((c, _)) = &cmd
        && !installed.as_deref().is_some_and(|i| !c.is_empty() && same_path(Path::new(c), i))
        && set_task(false)
    {
        set_run_key(true); // still starts with Windows, just without silent admin rights
        cmd = None;
    }
    if let Some((_, args)) = &cmd
        && args != AUTOSTART_ARG
        && running_installed()
    {
        set_task(true); // 1.8's task: without the argument the window would open at logon
    }
    if prefer_task && cmd.is_none() && running_installed() && run_key_enabled() && set_task(true) {
        set_run_key(false);
    }
}

/// The autostart switch: elevated in admin mode, the task (no UAC prompt at
/// logon); otherwise the Run key. Returns the new state.
pub fn set_autostart(on: bool, run_as_admin: bool) -> bool {
    let admin = pcm_win::system::is_admin();
    if on && admin && run_as_admin && running_installed() && set_task(true) {
        set_run_key(false);
    } else {
        set_run_key(on);
        if admin {
            set_task(false);
        }
    }
    autostart_enabled()
}

// ---------- admin rights ----------

/// What starting with the admin setting comes to.
pub enum Start {
    /// Carry on here.
    Here,
    /// An elevated copy took over: leave.
    Leave,
}

/// Before the widget starts (1.8's __main__): with the admin setting on,
/// ask UAC for an elevated copy, or, elevated in an ordinary folder, copy
/// this build to Program Files and carry on from there. If UAC is refused,
/// this copy runs without admin rights. Call it holding the lock.
pub fn at_start(run_as_admin: bool, args: &[String]) -> Start {
    let wants = run_as_admin
        && pcm_win::sensors_helper::find().is_some()
        && !args.iter().any(|a| a == "--no-elevate");
    if !wants {
        return Start::Here;
    }
    // the copy we start carries on as asked: quietly (autostart) or with the settings open
    let keep: Vec<&str> =
        args.iter().map(String::as_str).filter(|a| ["--settings", AUTOSTART_ARG].contains(a)).collect();
    if !pcm_win::system::is_admin() {
        release_instance(); // the elevated copy is to take the lock
        if relaunch_as_admin(&keep.join(" "), 0) || already_running() {
            return Start::Leave; // or we lost a race while UAC was open
        }
    } else if !running_installed() && install_copy() {
        // elevated, but from a folder anyone can write to (e.g. Downloads)
        release_instance();
        if start_installed(&keep) {
            return Start::Leave;
        }
        already_running();
    }
    Start::Here
}

/// What switching admin mode on (from the settings) needs.
pub fn needs_restart(run_as_admin: bool) -> bool {
    run_as_admin && (!pcm_win::system::is_admin() || !running_installed())
}

/// Restart elevated (from the protected copy), with the settings open
/// again. True when the new copy is running (we leave); false when UAC was
/// refused or the copy failed: this one keeps running and holds the lock.
pub fn restart_elevated(owner: isize) -> bool {
    release_instance();
    let started = if pcm_win::system::is_admin() {
        install_copy() && start_installed(&["--settings"])
    } else {
        relaunch_as_admin("--settings", owner)
    };
    if !started {
        already_running(); // take the lock back
    }
    started
}

/// One line for --diag.
pub fn describe() -> String {
    format!(
        "run key: {}, task: {}, installed copy: {}, running it: {}",
        os::run_entry(RUN_NAME).unwrap_or_else(|| "none".into()),
        task_command()
            .map_or_else(|| "none".into(), |(c, a)| format!("{} {a}", if c.is_empty() { "?" } else { &c })),
        installed_exe().map_or_else(|| "none".into(), |p| p.display().to_string()),
        running_installed(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_xml_round_trip() {
        let xml =
            task_xml(r"PC\Влад & Co", r"C:\Program Files\PC-Monitor\PC-Monitor.exe", r"C:\Program Files");
        assert!(xml.contains(r"<UserId>PC\Влад &amp; Co</UserId>"));
        assert_eq!(command_of(&xml).as_deref(), Some(r"C:\Program Files\PC-Monitor\PC-Monitor.exe"));
        assert_eq!(tag(&xml, "Arguments").as_deref(), Some(AUTOSTART_ARG));
        assert_eq!(tag("<Arguments></Arguments>", "Arguments").as_deref(), Some(""));
        assert_eq!(command_of("<Command>\"C:\\a&amp;b\\x.exe\"</Command>").as_deref(), Some(r"C:\a&b\x.exe"));
        assert_eq!(command_of("<Exec></Exec>"), None);
    }

    #[test]
    fn paths_compare_without_case() {
        assert!(same_path(
            Path::new(r"C:\Program Files\PC-Monitor\"),
            Path::new("c:/program files/pc-monitor")
        ));
        assert!(!same_path(
            Path::new(r"C:\Program Files\PC-Monitor"),
            Path::new(r"C:\Program Files\PC-Monitor 2")
        ));
        let d = with_suffix(Path::new(r"C:\Program Files\PC-Monitor 2"), ".new");
        assert_eq!(d, PathBuf::from(r"C:\Program Files\PC-Monitor 2.new"));
    }

    #[test]
    fn builds_compare_by_content() {
        let root = std::env::temp_dir().join(format!("pcm-build-test-{}", std::process::id()));
        let (a, b) = (root.join("a"), root.join("b"));
        for d in [&a, &b] {
            std::fs::create_dir_all(d.join(INTERNAL).join("sub")).unwrap();
            std::fs::write(d.join(EXE_NAME), b"exe").unwrap();
            std::fs::write(d.join(INTERNAL).join("sub").join("x.dll"), b"12345").unwrap();
        }
        assert!(same_build(&a, &b));
        std::fs::write(b.join(INTERNAL).join("sub").join("x.dll"), b"123456").unwrap();
        assert!(!same_build(&a, &b));
        std::fs::write(b.join(INTERNAL).join("sub").join("x.dll"), b"12345").unwrap();
        std::fs::write(b.join(EXE_NAME), b"exf").unwrap();
        assert!(!same_build(&a, &b));
        let _ = std::fs::remove_dir_all(&root);
    }
}

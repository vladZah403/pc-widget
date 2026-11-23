//! Updates (1.8's UpdateChecker, Updater and swap, plus the signature).
//!
//! Every 6 hours (and on "Проверить сейчас") the latest release is read
//! from GitHub. On a click its PC-Monitor.zip is downloaded and must match
//! GitHub's SHA-256 and carry our minisign signature; it is unpacked next
//! to the program folder ("<folder>.new"). Then the NEW exe, started from
//! there with `--finish-update`, waits for us to exit, moves the program
//! folder aside, copies itself in and starts it; if anything fails it puts
//! the old folder back and starts that. No PowerShell (1.8 used it).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pcm_core::update::{self, Release};
use pcm_render::format::tr;
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::{APP_VERSION, elevation};

/// Something changed: a check finished, the download moved on, it's ready or failed.
pub const WM_APP_UPDATE: u32 = 0x8000 + 8;
pub const FINISH_ARG: &str = "--finish-update";
const FIRST_CHECK: Duration = Duration::from_secs(20);
const EVERY: Duration = Duration::from_secs(6 * 3600);
const EXE_NAME: &str = "PC-Monitor.exe";

fn agent() -> String {
    format!("PC-Monitor/{APP_VERSION}")
}

fn post(hwnd: isize) {
    unsafe { PostMessageW(hwnd as _, WM_APP_UPDATE, 0, 0) };
}

// ---------- the check ----------

#[derive(Clone, Default)]
pub struct CheckState {
    /// A newer release, if the last check found one.
    pub found: Option<Release>,
    /// Finished checks, successful or not.
    pub checks: u32,
    /// The last one failed (offline is normal for a laptop).
    pub failed: bool,
}

pub struct Checker {
    state: Arc<Mutex<CheckState>>,
    enabled: Arc<AtomicBool>,
    wake: Sender<()>,
}

impl Checker {
    /// The background check: first soon after the start, then every 6 hours.
    pub fn start(hwnd: isize, enabled: bool) -> Checker {
        let state = Arc::new(Mutex::new(CheckState::default()));
        let on = Arc::new(AtomicBool::new(enabled));
        let (wake, rx) = std::sync::mpsc::channel::<()>();
        let (st, on2) = (state.clone(), on.clone());
        let _ = std::thread::Builder::new().name("update-check".into()).spawn(move || {
            let mut wait = FIRST_CHECK;
            loop {
                if let Err(RecvTimeoutError::Disconnected) = rx.recv_timeout(wait) {
                    break; // the widget has gone
                }
                while rx.try_recv().is_ok() {} // several "check now" are one
                wait = EVERY;
                if !on2.load(Ordering::Relaxed) {
                    continue;
                }
                let result = check();
                if let Ok(mut s) = st.lock() {
                    s.checks += 1;
                    match result {
                        Ok(found) => (s.found, s.failed) = (found, false),
                        Err(_) => s.failed = true,
                    }
                }
                post(hwnd);
            }
        });
        Checker { state, enabled: on, wake }
    }

    pub fn check_now(&self) {
        let _ = self.wake.send(());
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
        if on {
            self.check_now();
        }
    }

    pub fn state(&self) -> CheckState {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

/// One look at the latest release.
fn check() -> Result<Option<Release>, String> {
    let body = pcm_win::http::get(
        update::LATEST_API,
        &agent(),
        &[("Accept", "application/vnd.github+json")],
        1 << 20,
        &mut |_| (),
    )?;
    let info: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    Ok(update::newer_release(&info, APP_VERSION))
}

// ---------- the update ----------

#[derive(Clone, Debug, PartialEq)]
pub enum Stage {
    Download(f32),
    Install,
    Ready,
    Error(String),
}

pub struct Updater {
    pub app_dir: PathBuf,
    pub staging: PathBuf,
    stage: Arc<Mutex<Stage>>,
}

impl Updater {
    /// Download, check and unpack `release` into the staging folder.
    pub fn start(release: &Release, (app_dir, staging): (PathBuf, PathBuf), hwnd: isize) -> Option<Updater> {
        let (zip, sig) = (release.zip.clone()?, release.sig.clone()?);
        let version = release.version.clone();
        let stage = Arc::new(Mutex::new(Stage::Download(0.0)));
        let (st, app, stg) = (stage.clone(), app_dir.clone(), staging.clone());
        let set = move |s: Stage| {
            if let Ok(mut g) = st.lock() {
                *g = s;
            }
            post(hwnd);
        };
        std::thread::Builder::new()
            .name("updater".into())
            .spawn(move || {
                let mut shown = 0u64;
                let mut progress = |n: u64| {
                    // a repaint every 2 %
                    if n * 50 / zip.size.max(1) != shown * 50 / zip.size.max(1) {
                        shown = n;
                        set(Stage::Download(n as f32 / zip.size as f32));
                    }
                };
                let result = (|| {
                    let data = pcm_win::http::get(&zip.url, &agent(), &[], zip.size, &mut progress)?;
                    if data.len() as u64 != zip.size {
                        return Err(tr("файл скачался не полностью"));
                    }
                    if pcm_win::http::sha256_hex(&data) != zip.sha256 {
                        return Err(tr("контрольная сумма не совпала"));
                    }
                    let minisig = pcm_win::http::get(&sig.url, &agent(), &[], sig.size, &mut |_| ())?;
                    if pcm_win::http::sha256_hex(&minisig) != sig.sha256 {
                        return Err(tr("контрольная сумма не совпала"));
                    }
                    update::verify_release(&data, &minisig, update::PUBLIC_KEY, &version)
                        .map_err(|_| tr("подпись не прошла проверку"))?;
                    set(Stage::Install);
                    update::unpack(&data, &stg, &app).map_err(|_| tr("в архиве нет программы"))
                })();
                set(match result {
                    Ok(()) => Stage::Ready,
                    Err(e) => {
                        let _ = std::fs::remove_dir_all(&stg);
                        Stage::Error(e)
                    }
                });
            })
            .ok()?;
        Some(Updater { app_dir, staging, stage })
    }

    pub fn stage(&self) -> Stage {
        self.stage.lock().map(|s| s.clone()).unwrap_or(Stage::Error(String::new()))
    }
}

/// (program folder, staging folder) when this copy can update itself.
/// Elevated, only the protected Program Files copy (its staging folder is
/// protected too); without admin rights, a copy in an ordinary folder.
pub fn update_target() -> Option<(PathBuf, PathBuf)> {
    let here = elevation::app_dir()?;
    if !elevation::is_release_folder(&here) {
        return None; // a bare build (cargo's folder): nothing to update
    }
    let admin = pcm_win::system::is_admin();
    if elevation::running_installed() {
        let dir = elevation::install_dir();
        return admin.then(|| (dir.clone(), elevation::with_suffix(&dir, ".new")));
    }
    if admin {
        return None; // never run a download from a user-writable folder with admin rights
    }
    Some((here.clone(), elevation::with_suffix(&here, ".new")))
}

/// Start the new exe from the staging folder to finish the job once we're gone.
pub fn start_finish(app_dir: &Path, staging: &Path) -> bool {
    std::process::Command::new(staging.join(EXE_NAME))
        .arg(FINISH_ARG)
        .arg(app_dir)
        .arg(std::process::id().to_string())
        .current_dir(pcm_win::autostart::system32())
        .spawn()
        .is_ok()
}

/// Try `f` for up to `secs` seconds (a folder stays busy while something
/// still runs from it: the sensor helper, the PC Monitor window).
fn retry(secs: u64, mut f: impl FnMut() -> bool) -> bool {
    for _ in 0..secs * 4 {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    false
}

/// `PC-Monitor.exe --finish-update <program folder> <pid>`, run by the new
/// version from the staging folder: swap the folders and start the program.
pub fn finish_update(args: &[String]) -> i32 {
    let at = args.iter().position(|a| a == FINISH_ARG).unwrap_or(0);
    let (Some(app_dir), Some(pid)) =
        (args.get(at + 1).map(PathBuf::from), args.get(at + 2).and_then(|p| p.parse::<u32>().ok()))
    else {
        return 2;
    };
    let Some(staging) = elevation::app_dir() else { return 2 };
    // only ever "<folder>.new" next to "<folder>": nothing else gets replaced
    if elevation::with_suffix(&app_dir, ".new") != staging || !staging.join(EXE_NAME).is_file() {
        return 2;
    }
    pcm_win::processes::wait_for_exit(pid, Duration::from_secs(30));
    let start = |dir: &Path| {
        std::process::Command::new(dir.join(EXE_NAME))
            .current_dir(pcm_win::autostart::system32())
            .spawn()
            .is_ok()
    };
    swap(&app_dir, &staging, 15, start)
}

/// The program folder aside ("<folder>.old"), the staging folder's files in
/// its place, then start what is there: the new version, or the old one put
/// back when the folder stays busy for `wait_s` or the copy fails.
fn swap(app_dir: &Path, staging: &Path, wait_s: u64, start: impl Fn(&Path) -> bool) -> i32 {
    let old = elevation::with_suffix(app_dir, ".old");
    let _ = std::fs::remove_dir_all(&old);
    if !retry(wait_s, || std::fs::rename(app_dir, &old).is_ok()) {
        start(app_dir); // still in use: the old version carries on
        return 1;
    }
    if elevation::copy_dir(staging, app_dir).is_err() {
        let _ = std::fs::remove_dir_all(app_dir);
        let _ = std::fs::rename(&old, app_dir); // the old version back
        start(app_dir);
        return 1;
    }
    start(app_dir);
    0
}

/// At start: drop what an update left next to the program folder (the old
/// version, a staging folder). The finishing exe may still be on its way
/// out, so this keeps trying for a while, in the background.
pub fn clean_leftovers() {
    let Some(here) = elevation::app_dir() else { return };
    if !elevation::is_release_folder(&here) || here.extension().is_some_and(|e| e == "new") {
        return;
    }
    let _ = std::thread::Builder::new().name("update-cleanup".into()).spawn(move || {
        for dir in [elevation::with_suffix(&here, ".old"), elevation::with_suffix(&here, ".new")] {
            if dir.exists() {
                retry(20, || std::fs::remove_dir_all(&dir).is_ok() || !dir.exists());
            }
        }
    });
}

/// Open one of our GitHub pages. Elevated, explorer.exe hands the link to
/// the user's normal shell, so the browser doesn't get admin rights.
pub fn open_in_browser(url: &str) {
    if !url.starts_with(&format!("https://github.com/{}", update::REPO)) {
        return;
    }
    if pcm_win::system::is_admin() {
        let windir = pcm_win::autostart::system32().parent().map(Path::to_path_buf).unwrap_or_default();
        let _ = std::process::Command::new(windir.join("explorer.exe")).arg(url).spawn();
    } else {
        pcm_win::autostart::open(url);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn folder(dir: &Path, exe: &[u8]) {
        std::fs::create_dir_all(dir.join("_internal")).unwrap();
        std::fs::write(dir.join(EXE_NAME), exe).unwrap();
        std::fs::write(dir.join("_internal").join("pcm-sensors.exe"), exe).unwrap();
    }

    #[test]
    fn swaps_in_the_new_version() {
        let root = std::env::temp_dir().join(format!("pcm-swap-{}", std::process::id()));
        let (app, staging) = (root.join("PC-Monitor"), root.join("PC-Monitor.new"));
        folder(&app, b"old");
        folder(&staging, b"new");
        let started = RefCell::new(Vec::new());
        let code = swap(&app, &staging, 1, |d| {
            started.borrow_mut().push(std::fs::read(d.join(EXE_NAME)).unwrap());
            true
        });
        assert_eq!(code, 0);
        assert_eq!(started.into_inner(), vec![b"new".to_vec()], "the new version starts");
        assert_eq!(std::fs::read(app.join("_internal").join("pcm-sensors.exe")).unwrap(), b"new");
        assert_eq!(std::fs::read(root.join("PC-Monitor.old").join(EXE_NAME)).unwrap(), b"old");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn busy_folder_keeps_the_old_version() {
        let root = std::env::temp_dir().join(format!("pcm-swap-busy-{}", std::process::id()));
        let (app, staging) = (root.join("PC-Monitor"), root.join("PC-Monitor.new"));
        folder(&app, b"old");
        folder(&staging, b"new");
        // an open file inside (as a running helper would be) keeps the folder from moving
        let busy = std::fs::File::open(app.join("_internal").join("pcm-sensors.exe")).unwrap();
        let started = RefCell::new(Vec::new());
        let code = swap(&app, &staging, 1, |d| {
            started.borrow_mut().push(std::fs::read(d.join(EXE_NAME)).unwrap());
            true
        });
        drop(busy);
        assert_eq!(code, 1);
        assert_eq!(started.into_inner(), vec![b"old".to_vec()], "the old version carries on");
        assert!(!root.join("PC-Monitor.old").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}

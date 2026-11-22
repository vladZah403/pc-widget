//! Starting with Windows and with admin rights, as 1.8 does it: the
//! per-user Run key, a logon task with the highest rights (schtasks.exe,
//! no UAC prompt at logon), the protected Program Files folder and the UAC
//! prompt itself. What to do with them is pcm-app's business.

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW};

use crate::system::reg_string;
use crate::{from_wide, wide};

/// Programs that start when this user logs on.
pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// The command of the Run value `name`, or None when there is none.
pub fn run_entry(name: &str) -> Option<String> {
    reg_string(HKEY_CURRENT_USER, RUN_KEY, name)
}

/// Set the Run value `name` to `command`, or remove it (None). Needs no
/// admin rights. True when it is done (removing a missing value is done).
pub fn set_run_entry(name: &str, command: Option<&str>) -> bool {
    let (key, value) = (wide(RUN_KEY), wide(name));
    unsafe {
        match command {
            Some(cmd) => {
                let data = wide(cmd);
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    key.as_ptr(),
                    value.as_ptr(),
                    REG_SZ,
                    data.as_ptr() as *const _,
                    (data.len() * 2) as u32,
                ) == 0
            }
            None => {
                let rc = RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr());
                rc == 0 || rc == ERROR_FILE_NOT_FOUND
            }
        }
    }
}

/// A REG_SZ value under HKEY_CURRENT_USER, or None.
pub fn user_value(key: &str, value: &str) -> Option<String> {
    reg_string(HKEY_CURRENT_USER, key, value)
}

/// Change a REG_SZ value under HKEY_CURRENT_USER, only if the key is there
/// already (RegSetKeyValueW would create it).
pub fn set_user_value(key: &str, value: &str, data: &str) -> bool {
    use windows_sys::Win32::System::Registry::{KEY_SET_VALUE, RegCloseKey, RegOpenKeyExW};
    let (k, v, d) = (wide(key), wide(value), wide(data));
    unsafe {
        let mut h = null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, k.as_ptr(), 0, KEY_SET_VALUE, &mut h) != 0 {
            return false;
        }
        let ok =
            RegSetKeyValueW(h, null(), v.as_ptr(), REG_SZ, d.as_ptr() as *const _, (d.len() * 2) as u32) == 0;
        RegCloseKey(h);
        ok
    }
}

/// The system folder from the kernel, not from an environment variable.
pub fn system32() -> PathBuf {
    let mut buf = [0u16; 260];
    let n =
        unsafe { windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW(buf.as_mut_ptr(), 260) }
            as usize;
    if n == 0 || n >= buf.len() {
        return PathBuf::from(r"C:\Windows\System32");
    }
    PathBuf::from(from_wide(&buf[..n]))
}

/// Program Files from the shell (FOLDERID_ProgramFiles): %ProgramFiles%
/// could be faked by whoever starts us.
pub fn program_files() -> PathBuf {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_ProgramFiles, SHGetKnownFolderPath};
    unsafe {
        let mut p: *mut u16 = null_mut();
        let hr = SHGetKnownFolderPath(&FOLDERID_ProgramFiles, 0, null_mut(), &mut p);
        let path = if hr >= 0 { crate::from_wide_ptr(p) } else { None };
        if !p.is_null() {
            CoTaskMemFree(p as *const _);
        }
        path.filter(|s| !s.is_empty()).map_or_else(|| PathBuf::from(r"C:\Program Files"), PathBuf::from)
    }
}

/// Run schtasks.exe (by its full path: a bare name would be looked up in the
/// current folder too); its output when it succeeds.
fn schtasks(args: &[&std::ffi::OsStr]) -> Option<Vec<u8>> {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new(system32().join("schtasks.exe"))
        .args(args)
        .stdin(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

/// Is there a task called `name`? (Any user may ask about their own tasks.)
pub fn task_exists(name: &str) -> bool {
    schtasks(&["/Query".as_ref(), "/TN".as_ref(), name.as_ref()]).is_some()
}

/// The task's definition (XML), or None when there is no such task.
pub fn task_xml(name: &str) -> Option<String> {
    let raw = schtasks(&["/Query".as_ref(), "/TN".as_ref(), name.as_ref(), "/XML".as_ref()])?;
    Some(decode(&raw))
}

/// schtasks writes UTF-16 with a BOM to a file, the ANSI code page to a pipe.
fn decode(raw: &[u8]) -> String {
    if raw.len() >= 2 && raw[..2] == [0xff, 0xfe] {
        let units: Vec<u16> = raw[2..].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        return String::from_utf16_lossy(&units);
    }
    if raw.is_ascii() {
        return String::from_utf8_lossy(raw).into_owned();
    }
    use windows_sys::Win32::Globalization::{CP_ACP, MultiByteToWideChar};
    unsafe {
        let n = MultiByteToWideChar(CP_ACP, 0, raw.as_ptr(), raw.len() as i32, null_mut(), 0);
        if n <= 0 {
            return String::from_utf8_lossy(raw).into_owned();
        }
        let mut buf = vec![0u16; n as usize];
        MultiByteToWideChar(CP_ACP, 0, raw.as_ptr(), raw.len() as i32, buf.as_mut_ptr(), n);
        String::from_utf16_lossy(&buf)
    }
}

/// Create (or replace) the task `name` from an XML file. Needs admin rights
/// for a task with the highest rights.
pub fn create_task(name: &str, xml_file: &Path) -> bool {
    let args: [&std::ffi::OsStr; 6] = [
        "/Create".as_ref(),
        "/TN".as_ref(),
        name.as_ref(),
        "/XML".as_ref(),
        xml_file.as_os_str(),
        "/F".as_ref(),
    ];
    schtasks(&args).is_some()
}

/// Delete the task `name` (admin rights for an elevated task).
pub fn delete_task(name: &str) -> bool {
    schtasks(&["/Delete".as_ref(), "/TN".as_ref(), name.as_ref(), "/F".as_ref()]).is_some()
}

/// Ask UAC to start `exe` with admin rights. `owner` should be the window
/// in front (the prompt comes up in front only for it). False when it was
/// not started, e.g. the user pressed "No". It waits for the prompt.
pub fn start_as_admin(exe: &Path, args: &str, dir: &Path, owner: isize) -> bool {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let (verb, file, params, folder) =
        (wide("runas"), wide(&exe.to_string_lossy()), wide(args), wide(&dir.to_string_lossy()));
    let rc = unsafe {
        ShellExecuteW(
            owner as _,
            verb.as_ptr(),
            file.as_ptr(),
            if args.is_empty() { null() } else { params.as_ptr() },
            folder.as_ptr(),
            SW_SHOWNORMAL,
        )
    };
    rc as isize > 32 // 32 and below: an error (ERROR_CANCELLED when refused)
}

/// Open a web page (or a file) the way Explorer would.
pub fn open(target: &str) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let (verb, file) = (wide("open"), wide(target));
    unsafe { ShellExecuteW(null_mut(), verb.as_ptr(), file.as_ptr(), null(), null(), SW_SHOWNORMAL) };
}

/// Like start_as_admin, but wait (up to `timeout`) for the elevated
/// program to finish. False when it didn't start (UAC refused).
pub fn run_as_admin_and_wait(exe: &Path, args: &str, owner: isize, timeout: std::time::Duration) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::WaitForSingleObject;
    use windows_sys::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;
    let (verb, file, params) = (wide("runas"), wide(&exe.to_string_lossy()), wide(args));
    unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS;
        info.hwnd = owner as _;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params.as_ptr();
        info.nShow = SW_HIDE;
        if ShellExecuteExW(&mut info) == 0 {
            return false;
        }
        if !info.hProcess.is_null() {
            WaitForSingleObject(info.hProcess, timeout.as_millis().min(u32::MAX as u128) as u32);
            CloseHandle(info.hProcess);
        }
        true
    }
}

/// The window in front (to own a UAC prompt).
pub fn foreground_window() -> isize {
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() as isize }
}

/// "DOMAIN\user" of this user, for a task's logon trigger (1.8 takes it
/// from the environment the same way).
pub fn user_name() -> String {
    let get = |k: &str| std::env::var(k).unwrap_or_default();
    format!("{}\\{}", get("USERDOMAIN"), get("USERNAME"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_both_kinds() {
        let text = "<Command>C:\\Program Files\\PC-Monitor\\PC-Monitor.exe</Command>";
        let mut utf16 = vec![0xff, 0xfe];
        utf16.extend(text.encode_utf16().flat_map(|u| u.to_le_bytes()));
        assert_eq!(decode(&utf16), text);
        assert_eq!(decode(text.as_bytes()), text);
    }

    #[test]
    fn folders_are_real() {
        assert!(system32().join("schtasks.exe").is_file());
        assert!(program_files().is_dir());
    }
}

//! What fills the system drive: the places that usually grow (the Recycle
//! Bin, Downloads and the other user folders, temporary files, Windows
//! Update's downloads, the page and hibernation files) and the biggest
//! folders of programs and games. Folder sizes are summed file by file,
//! junctions and links left out (they point at what is counted elsewhere).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_LocalAppData, FOLDERID_Music,
    FOLDERID_Pictures, FOLDERID_Profile, FOLDERID_ProgramFiles, FOLDERID_ProgramFilesX86,
    FOLDERID_RoamingAppData, FOLDERID_Videos, SHGetKnownFolderPath, SHQUERYRBINFO, SHQueryRecycleBinW,
};
use windows_sys::core::GUID;

use crate::wide;

/// What an item is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// The Recycle Bin of the drive.
    RecycleBin,
    /// A folder (it can be opened).
    Folder,
    /// A system file (pagefile.sys, hiberfil.sys).
    SystemFile,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// Russian, to translate ("Загрузки"), or a folder path to show as is.
    pub name: String,
    /// `name` is a text to translate.
    pub named: bool,
    pub path: PathBuf,
    pub bytes: u64,
    pub place: Place,
}

/// The folders' and files' biggest items worth showing (others: bigger than this).
pub const BIG: u64 = 300 << 20;

/// A known folder's path (Downloads, Program Files…).
fn known(id: &GUID) -> Option<PathBuf> {
    let mut p: windows_sys::core::PWSTR = std::ptr::null_mut();
    unsafe {
        let hr = SHGetKnownFolderPath(id, 0, std::ptr::null_mut(), &mut p);
        if hr != 0 || p.is_null() {
            return None;
        }
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
        CoTaskMemFree(p as _);
        Some(PathBuf::from(s))
    }
}

/// The bytes in the Recycle Bin of `root` ("C:\\").
pub fn recycle_bin(root: &str) -> Option<u64> {
    let mut info: SHQUERYRBINFO = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<SHQUERYRBINFO>() as u32;
    let r = wide(root);
    let hr = unsafe { SHQueryRecycleBinW(r.as_ptr(), &mut info) };
    (hr == 0).then_some(info.i64Size.max(0) as u64)
}

/// A junction, a symbolic link or another reparse point: not followed.
fn is_link(m: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    m.file_attributes() & 0x400 != 0 || m.file_type().is_symlink()
}

/// The bytes of every file under `dir`; `files` counts them as it goes,
/// `stop` ends it early (what was summed so far is returned).
pub fn folder_size(dir: &Path, stop: &AtomicBool, files: &AtomicU64) -> u64 {
    let mut total = 0u64;
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            // the listing's own data: no file is opened
            let Ok(m) = e.metadata() else { continue };
            if is_link(&m) {
                continue;
            }
            if m.is_dir() {
                todo.push(e.path());
            } else {
                total += m.len();
                files.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    total
}

/// The subfolders of `dir`.
fn subfolders(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.metadata().is_ok_and(|m| m.is_dir() && !is_link(&m)))
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default()
}

/// "Program Files (x86)\\Steam" of a folder under the drive's root.
fn short(path: &Path, root: &Path) -> String {
    let s = path.strip_prefix(root).unwrap_or(path).to_string_lossy().to_string();
    // a Steam game by its name: "Steam: STALCRAFT"
    const STEAM: &str = "steamapps\\common\\";
    if let Some(i) = s.to_lowercase().find(STEAM) {
        return format!("Steam: {}", &s[i + STEAM.len()..]);
    }
    // the user's folder by its last two parts: "AppData\\Local\\Discord"
    if let Some(i) = s.find("AppData\\") {
        return s[i..].to_string();
    }
    s
}

/// Everything, biggest first; `files` counts the files summed so far.
pub fn scan(stop: &AtomicBool, files: &AtomicU64) -> Vec<Item> {
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    let root = PathBuf::from(format!("{drive}\\"));
    let on_drive = |p: &Path| p.to_string_lossy().to_lowercase().starts_with(&drive.to_lowercase());
    let mut out: Vec<Item> = Vec::new();
    let mut add = |name: &str, named: bool, path: PathBuf, bytes: u64, place: Place| {
        out.push(Item { name: name.to_string(), named, path, bytes, place });
    };
    if let Some(b) = recycle_bin(&format!("{drive}\\")) {
        add("Корзина", true, PathBuf::from("shell:RecycleBinFolder"), b, Place::RecycleBin);
    }
    let user = [
        ("Загрузки", FOLDERID_Downloads),
        ("Рабочий стол", FOLDERID_Desktop),
        ("Документы", FOLDERID_Documents),
        ("Видео", FOLDERID_Videos),
        ("Изображения", FOLDERID_Pictures),
        ("Музыка", FOLDERID_Music),
    ];
    let mut counted: Vec<PathBuf> = Vec::new();
    for (name, id) in user {
        if let Some(p) = known(&id).filter(|p| on_drive(p)) {
            let b = folder_size(&p, stop, files);
            counted.push(p.clone());
            add(name, true, p, b, Place::Folder);
        }
    }
    let temp = std::env::temp_dir();
    if on_drive(&temp) {
        let b = folder_size(&temp, stop, files);
        counted.push(temp.clone());
        add("Временные файлы", true, temp, b, Place::Folder);
    }
    let windir = PathBuf::from(std::env::var("WINDIR").unwrap_or_else(|_| format!("{drive}\\Windows")));
    let updates = windir.join("SoftwareDistribution").join("Download");
    if updates.is_dir() {
        let b = folder_size(&updates, stop, files);
        add("Загрузки обновлений Windows", true, updates, b, Place::Folder);
    }
    // the page and hibernation files, as the root's listing gives their size
    if let Ok(rd) = std::fs::read_dir(&root) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_lowercase();
            let name = match n.as_str() {
                "pagefile.sys" => "Файл подкачки",
                "hiberfil.sys" => "Файл гибернации",
                "swapfile.sys" => "Файл подкачки приложений",
                _ => continue,
            };
            if let Ok(m) = e.metadata() {
                add(name, true, e.path(), m.len(), Place::SystemFile);
            }
        }
    }
    // the biggest folders of programs, games and their data
    let mut candidates: Vec<PathBuf> = Vec::new();
    for id in
        [FOLDERID_ProgramFiles, FOLDERID_ProgramFilesX86, FOLDERID_LocalAppData, FOLDERID_RoamingAppData]
    {
        if let Some(base) = known(&id).filter(|p| on_drive(p)) {
            for sub in subfolders(&base) {
                // Steam: each game on its own
                let games = sub.join("steamapps").join("common");
                if games.is_dir() {
                    candidates.extend(subfolders(&games));
                } else {
                    candidates.push(sub);
                }
            }
        }
    }
    if let Some(profile) = known(&FOLDERID_Profile).filter(|p| on_drive(p)) {
        for sub in subfolders(&profile) {
            let name = sub.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            if name != "appdata" && !counted.iter().any(|c| c == &sub) {
                candidates.push(sub);
            }
        }
    }
    let mut big: Vec<(PathBuf, u64)> = Vec::new();
    for c in candidates {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let b = folder_size(&c, stop, files);
        if b >= BIG {
            big.push((c, b));
        }
    }
    big.sort_by_key(|(_, b)| std::cmp::Reverse(*b));
    for (p, b) in big.into_iter().take(10) {
        let name = short(&p, &root);
        add(&name, false, p, b, Place::Folder);
    }
    out.sort_by_key(|i| std::cmp::Reverse(i.bytes));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folders_size() {
        let dir = std::env::temp_dir().join(format!("pcm-diskuse-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("a").join("b")).unwrap();
        std::fs::write(dir.join("one"), vec![0u8; 1000]).unwrap();
        std::fs::write(dir.join("a").join("b").join("two"), vec![0u8; 2345]).unwrap();
        let (stop, files) = (AtomicBool::new(false), AtomicU64::new(0));
        assert_eq!(folder_size(&dir, &stop, &files), 3345);
        assert_eq!(files.load(Ordering::Relaxed), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn known_places() {
        assert!(known(&FOLDERID_Downloads).is_some());
        assert!(recycle_bin("C:\\").is_some());
        let root = Path::new("C:\\");
        assert_eq!(short(Path::new(r"C:\Program Files (x86)\Steam"), root), r"Program Files (x86)\Steam");
        assert_eq!(short(Path::new(r"C:\Users\Влад\AppData\Local\Discord"), root), r"AppData\Local\Discord");
        let game = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Far Cry 4");
        assert_eq!(short(game, root), "Steam: Far Cry 4");
    }

    #[test]
    #[ignore = "scans this PC's drive: cargo test -p pcm-win print_disk_use -- --ignored --nocapture"]
    fn print_disk_use() {
        let (stop, files) = (AtomicBool::new(false), AtomicU64::new(0));
        let t = std::time::Instant::now();
        for i in scan(&stop, &files) {
            println!("{:>10.1} MB  {:?}  {}", i.bytes as f64 / 1048576.0, i.place, i.name);
        }
        println!("{} files in {:?}", files.load(Ordering::Relaxed), t.elapsed());
    }
}

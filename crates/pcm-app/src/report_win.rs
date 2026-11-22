//! The window with a game session's report: a card to drag around, × (or
//! Esc) closes it, "Сохранить CSV" writes the seconds and the frames.

use std::path::{Path, PathBuf};
use std::ptr::null_mut;

use pcm_core::session::{EXTENSION, Session};
use pcm_render::report::{ReportBoxes, ReportHover, session_report};
use pcm_render::text::Fonts;
use pcm_render::theme::Scale;
use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
};
use windows_sys::Win32::UI::Controls::Dialogs::{
    GetSaveFileNameW, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::layered::{self, Surface};
use pcm_render::format::tr;

/// What a click on the report asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Close,
    SaveCsv,
    /// Pressed on the card itself: it is dragged.
    Drag,
}

pub struct ReportWin {
    pub hwnd: HWND,
    surface: Option<Surface>,
    session: Option<Session>,
    hover: ReportHover,
    boxes: ReportBoxes,
}

fn inside(b: &[f32; 4], x: f32, y: f32) -> bool {
    x >= b[0] && x < b[2] && y >= b[1] && y < b[3]
}

impl ReportWin {
    pub fn new(hwnd: HWND) -> ReportWin {
        ReportWin {
            hwnd,
            surface: None,
            session: None,
            hover: ReportHover::default(),
            boxes: ReportBoxes::default(),
        }
    }

    fn scale(&self) -> f32 {
        unsafe { GetDpiForWindow(self.hwnd) }.max(96) as f32 / 96.0
    }

    /// Open a session file in the middle of the screen with the mouse.
    pub fn open(&mut self, path: &Path, fonts: &mut Fonts, sc: &Scale) -> bool {
        let Ok(s) = Session::load(path) else { return false };
        self.session = Some(s);
        self.hover = ReportHover::default();
        unsafe {
            let mut pt = POINT::default();
            GetCursorPos(&mut pt);
            let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            GetMonitorInfoW(mon, &mut mi);
            let wa = mi.rcWork;
            // move there first, so the DPI is that monitor's
            SetWindowPos(self.hwnd, null_mut(), wa.left + 40, wa.top + 40, 0, 0, SWP_NOSIZE | SWP_NOZORDER);
            let Some(pm) = self.picture(fonts, sc) else { return false };
            let (w, h) = (pm.width() as i32, pm.height() as i32);
            let x = wa.left + ((wa.right - wa.left - w) / 2).max(0);
            let y = wa.top + ((wa.bottom - wa.top - h) / 2).max(0);
            layered::show(self.hwnd, &mut self.surface, &pm, 1.0, Some((x, y)));
            ShowWindow(self.hwnd, SW_SHOW);
            SetForegroundWindow(self.hwnd);
        }
        true
    }

    fn picture(&mut self, fonts: &mut Fonts, sc: &Scale) -> Option<tiny_skia::Pixmap> {
        let s = self.session.as_ref()?;
        let (pm, boxes) = session_report(fonts, self.scale(), sc, s, self.hover)?;
        self.boxes = boxes;
        Some(pm)
    }

    /// Draw again in place (hover changed, a new DPI).
    pub fn redraw(&mut self, fonts: &mut Fonts, sc: &Scale) {
        if let Some(pm) = self.picture(fonts, sc) {
            layered::show(self.hwnd, &mut self.surface, &pm, 1.0, None);
        }
    }

    pub fn mouse_move(&mut self, x: f32, y: f32, fonts: &mut Fonts, sc: &Scale) {
        let hover =
            ReportHover { close: inside(&self.boxes.close, x, y), csv: inside(&self.boxes.csv, x, y) };
        if hover != self.hover {
            self.hover = hover;
            self.redraw(fonts, sc);
        }
    }

    pub fn press(&self, x: f32, y: f32) -> Action {
        if inside(&self.boxes.close, x, y) {
            Action::Close
        } else if inside(&self.boxes.csv, x, y) {
            Action::SaveCsv
        } else {
            Action::Drag
        }
    }

    /// Close it; the session's frames are let go.
    pub fn close(&mut self) {
        unsafe { ShowWindow(self.hwnd, SW_HIDE) };
        self.session = None;
        self.surface = None;
    }

    /// "civilizationvi 2026-11-22 13-05.csv"
    pub fn csv_name(&self) -> String {
        self.session.as_ref().map_or("session.csv".into(), |s| session_csv_name(&s.info))
    }

    /// The seconds to `path`, the frames beside it ("… frames.csv").
    pub fn write_csv(&self, path: &Path) -> std::io::Result<PathBuf> {
        let Some(s) = &self.session else { return Err(std::io::ErrorKind::NotFound.into()) };
        write_session_csv(s, path)
    }
}

/// A session's seconds to `path` and its frames beside it ("… frames.csv").
pub fn write_session_csv(s: &Session, path: &Path) -> std::io::Result<PathBuf> {
    std::fs::write(path, s.csv_seconds(false))?;
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let frames = path.with_file_name(format!("{stem} frames.csv"));
    std::fs::write(&frames, s.csv_frames(false))?;
    Ok(frames)
}

/// "civilizationvi 2026-11-22 13-05.csv"
pub fn session_csv_name(info: &pcm_core::session::SessionInfo) -> String {
    let stamp: String = info.started.chars().take(16).map(|c| if c == ':' { '-' } else { c }).collect();
    format!("{} {stamp}.csv", info.game)
}

/// Windows' "Save as" dialog for a CSV file.
pub fn save_dialog(owner: HWND, suggested: &str) -> Option<PathBuf> {
    let mut file = [0u16; 1024];
    for (d, s) in file.iter_mut().zip(suggested.encode_utf16().take(1000)) {
        *d = s;
    }
    let filter: Vec<u16> = "CSV\0*.csv\0\0".encode_utf16().collect();
    let ext: Vec<u16> = "csv\0".encode_utf16().collect();
    let title: Vec<u16> = (tr("Сохранить CSV") + "\0").encode_utf16().collect();
    let mut ofn = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        lpstrFilter: filter.as_ptr(),
        lpstrFile: file.as_mut_ptr(),
        nMaxFile: file.len() as u32,
        lpstrDefExt: ext.as_ptr(),
        lpstrTitle: title.as_ptr(),
        Flags: OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    if unsafe { GetSaveFileNameW(&mut ofn) } == 0 {
        return None;
    }
    let n = file.iter().position(|&c| c == 0).unwrap_or(file.len());
    Some(PathBuf::from(String::from_utf16_lossy(&file[..n])))
}

/// The newest session file in `dir` (names start with the date and time).
pub fn latest_session(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == EXTENSION))
        .max()
}

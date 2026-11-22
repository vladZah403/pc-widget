//! Games: the frame counter (ETW), which window in front is a game, and the
//! FPS panel over it (1.8's FpsReader, Widget._update_fps_overlay and
//! FpsOverlay as one piece).

use std::collections::HashMap;
use std::ptr::null_mut;
use std::time::{Duration, Instant};

use pcm_core::frames::FrameStats;
use pcm_render::overlay::{Look, OverlayInfo, fps_overlay};
use pcm_render::text::Fonts;
use pcm_render::theme::Scale;
use pcm_render::widget::Data;
use pcm_win::frames::{Error, FrameCounter};
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromWindow};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::layered::{self, Surface};

/// Processes whose frames are not a game's (browsers, chats, players, shell).
pub const FPS_IGNORE: &[&str] = &[
    // our own window (`--dashboard`, a child process) draws frames too
    "pc-monitor",
    "chrome",
    "msedge",
    "firefox",
    "opera",
    "brave",
    "vivaldi",
    "browser",
    "yandex",
    "msedgewebview2",
    "telegram",
    "discord",
    "steamwebhelper",
    "explorer",
    "applicationframehost",
    "searchapp",
    "searchhost",
    "startmenuexperiencehost",
    "shellexperiencehost",
    "textinputhost",
    "vlc",
    "mpc-hc64",
    "mpc-be64",
    "potplayermini64",
    "spotify",
    "code",
    "devenv",
    "obs64",
    "pc-monitor",
    "python",
    "pythonw",
    "dwm",
    "winword",
    "excel",
    "powerpnt",
    "outlook",
    "onenote",
    "teams",
    "ms-teams",
    "zoom",
    "slack",
    "whatsapp",
];

/// A window in front drawing this many frames a second for GAME_AFTER is a
/// game (1.8: 25 FPS on 3 one-second checks in a row); fullscreen at once.
const GAME_FPS: f64 = 25.0;
const GAME_AFTER: Duration = Duration::from_secs(2);

/// The game in front.
pub struct Game {
    pub pid: u32,
    /// The program name ("civilizationvi").
    pub name: String,
    pub hwnd: HWND,
    pub rect: RECT,
    pub stats: FrameStats,
}

#[derive(Default)]
pub struct GameWatch {
    counter: Option<FrameCounter>,
    /// Why counting can't run (shown once when it is switched on).
    pub error: Option<Error>,
    /// (pid, since when it draws GAME_FPS+)
    streak: Option<(u32, Instant)>,
    names: HashMap<u32, String>,
}

impl GameWatch {
    /// The frame counter, while it runs.
    pub fn counter(&self) -> Option<&FrameCounter> {
        self.counter.as_ref()
    }

    pub fn start(&mut self) {
        if self.counter.is_none() {
            match FrameCounter::start() {
                Ok(c) => {
                    self.counter = Some(c);
                    self.error = None;
                }
                Err(e) => self.error = Some(e),
            }
        }
    }

    pub fn stop(&mut self) {
        self.counter = None; // its Drop ends the session
        self.streak = None;
    }

    fn name(&mut self, pid: u32) -> String {
        if self.names.len() > 500 {
            self.names.clear();
        }
        self.names
            .entry(pid)
            .or_insert_with(|| pcm_win::processes::image_name(pid).unwrap_or_default())
            .clone()
    }

    /// The game in front, if the window in front is one: not ours, not in
    /// FPS_IGNORE, and fullscreen or drawing GAME_FPS+ for GAME_AFTER.
    pub fn check(&mut self, own: &[HWND], fullscreen: bool) -> Option<Game> {
        let counter = self.counter.as_ref()?;
        let fg = unsafe { GetForegroundWindow() };
        if fg.is_null() || own.contains(&fg) {
            self.streak = None;
            return None;
        }
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(fg, &mut pid) };
        if pid == 0 || pid == std::process::id() {
            return None;
        }
        let Some((stats, _)) = counter.stats(pid) else {
            self.streak = None;
            return None;
        };
        let name = self.name(pid);
        if FPS_IGNORE.contains(&name.as_str()) {
            self.streak = None;
            return None;
        }
        let now = Instant::now();
        self.streak = match self.streak {
            Some((p, since)) if p == pid && stats.fps >= GAME_FPS => Some((p, since)),
            _ if stats.fps >= GAME_FPS => Some((pid, now)),
            _ => None,
        };
        let long = self.streak.is_some_and(|(_, since)| now - since >= GAME_AFTER);
        if !(fullscreen || long) {
            return None;
        }
        let mut rect = RECT::default();
        unsafe { GetWindowRect(fg, &mut rect) };
        Some(Game { pid, name, hwnd: fg, rect, stats })
    }
}

/// What the panel shows besides the frames: the widget's latest readings.
pub fn overlay_info(g: &Game, d: Option<&Data>) -> OverlayInfo {
    let mut info = OverlayInfo {
        fps: g.stats.fps,
        low: g.stats.low1,
        frame_ms: g.stats.frame_ms.clone(),
        ..Default::default()
    };
    if let Some(d) = d {
        let shared = d.gpu_shared && d.temp_gpu.is_none() && d.temp_cpu.is_some();
        info.cpu = Some(d.cpu);
        info.cpu_t = d.temp_cpu;
        info.gpu = d.gpu_load;
        info.gpu_t = if shared { d.temp_cpu } else { d.temp_gpu };
        info.gpu_approx = shared;
        info.vram_used_mb = d.vram_used_mb;
        info.vram_total_mb = d.vram_total_mb;
        info.gpu_clock = d.gpu_clock;
        info.ping = if d.ping_ready { d.ping } else { None };
    }
    info
}

/// The panel's window: topmost, clicks go through to the game, never focused.
pub struct Overlay {
    pub hwnd: HWND,
    surface: Option<Surface>,
    visible: bool,
    /// What the picture on screen shows (and how), to skip redrawing the same.
    shown: Option<(OverlayInfo, u32, Look, u32)>,
}

impl Overlay {
    pub fn new(hwnd: HWND) -> Overlay {
        Overlay { hwnd, surface: None, visible: false, shown: None }
    }

    /// Show `info` in `corner` ("tl", "tr", "bl", "br") of the game window,
    /// `size` %, its background `bg` (0..1) solid.
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        fonts: &mut Fonts,
        sc: &Scale,
        info: &OverlayInfo,
        g: &Game,
        corner: &str,
        size: u32,
        look: Look,
        bg: f32,
    ) {
        let dpi = unsafe {
            let mon = MonitorFromWindow(g.hwnd, MONITOR_DEFAULTTONEAREST);
            let (mut x, mut y) = (96u32, 96u32);
            GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut x, &mut y);
            x.max(96)
        };
        let scale = dpi as f32 / 96.0 * size as f32 / 100.0;
        // rounded like the panel's numbers, so a steady game doesn't redraw it
        let key = OverlayInfo {
            fps: info.fps.round(),
            low: info.low.map(f64::round),
            frame_ms: info.frame_ms.iter().map(|m| m.round()).collect(),
            ..info.clone()
        };
        let skey = (scale * 100.0).round() as u32;
        let bkey = (bg * 100.0).round() as u32;
        let redraw = self.shown.as_ref() != Some(&(key.clone(), skey, look, bkey));
        let r = g.rect;
        let m = (12.0 * scale).round() as i32;
        let place = |w: i32, h: i32| {
            let x = if corner.ends_with('l') { r.left + m } else { r.right - w - m };
            let y = if corner.starts_with('t') { r.top + m } else { r.bottom - h - m };
            (x, y)
        };
        if redraw {
            let Some(pm) = fps_overlay(fonts, scale, sc, &key, look, bg) else { return };
            let pos = place(pm.width() as i32, pm.height() as i32);
            layered::show(self.hwnd, &mut self.surface, &pm, 1.0, Some(pos));
            self.shown = Some((key, skey, look, bkey));
        } else if let Some(s) = &self.surface {
            let pos = place(s.w, s.h);
            unsafe {
                SetWindowPos(
                    self.hwnd,
                    null_mut(),
                    pos.0,
                    pos.1,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                )
            };
        }
        unsafe {
            if !self.visible {
                ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                self.visible = true;
            }
            // games raise themselves; stay above
            SetWindowPos(self.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    pub fn hide(&mut self) {
        if self.visible {
            self.visible = false;
            unsafe { ShowWindow(self.hwnd, SW_HIDE) };
        }
    }
}

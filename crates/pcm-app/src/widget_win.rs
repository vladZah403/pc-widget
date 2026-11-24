//! The desktop widget: a layered window that shows pcm-render's frames
//! (true transparency, no window chrome). Drag it anywhere; drag the right
//! or bottom edge or the corner to change its shape; Ctrl + wheel changes its
//! size; the pin keeps it on the desktop (it survives Win+D); a right click
//! opens the menu.

use std::cell::RefCell;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use pcm_core::config::{self, Config};
use pcm_render::overlay::Look;
use pcm_render::text::Fonts;
use pcm_render::theme::{DARK_INK, LIGHT_INK, Rgb, Theme, hex};
use pcm_render::widget::{
    BoxPx, Boxes, Data, Edge, GRIP, Renderer, TAB_H, TAB_W, UpdateState, View, WidgetLook, alert_card_with,
    hover_card, side_tab,
};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, RegisterHotKey, ReleaseCapture, SetCapture, TME_LEAVE,
    TRACKMOUSEEVENT, TrackMouseEvent, UnregisterHotKey, VK_CONTROL, VK_ESCAPE, VK_SHIFT,
};
use windows_sys::Win32::UI::Shell::{NIN_POPUPCLOSE, NIN_POPUPOPEN};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// The mouse left the window (TrackMouseEvent); from CommCtrl.h.
const WM_MOUSELEAVE: u32 = 0x02A3;

use crate::alerts::{Alert, Alerts, Limits};
use crate::collect::{self, Shared, WM_APP_DATA};
use crate::dashboard::{FromDashboard, StartInfo, ToDashboard};
use crate::elevation;
use crate::game::{GameWatch, Overlay, overlay_info};
use crate::layered::{self, Surface};
use crate::report_win::{self, Action, ReportWin};
use crate::session_rec::{self, Recorder, Recording, WM_APP_SESSION};
use crate::taskbar::Panel;
use crate::tray::{self, Tray, WM_APP_TRAY};
use crate::updater::{self, Checker, Stage, Updater, WM_APP_UPDATE};
use pcm_render::format::{dec, fmt_mem, fmt_speed, tr, trf};

/// The right and bottom edges are this wide for dragging the shape (logical px).
const EDGE: f32 = 6.0;
/// A dragged widget sticks this far from a screen edge, within this reach.
const SNAP_GAP: i32 = 10;
const SNAP_REACH: i32 = 16;
/// Mouse travel that turns a press into a drag (physical px).
const DRAG_START: i32 = 4;
const TIMER_DISARM: usize = 1;
const TIMER_GAMES: usize = 2;
const TIMER_TAB: usize = 3;
const TIMER_SLIDE: usize = 4;
const TIMER_TOAST: usize = 5;
/// The FPS panel follows the game 4 times a second.
const TIMER_FPS: usize = 6;
/// Put the side tab beside the widget again (a move it missed while busy).
const TIMER_FOLLOW: usize = 7;
/// The mouse rests on the taskbar panel: its card comes up.
const TIMER_HOVER: usize = 8;
const HOVER_DELAY_MS: u32 = 400;

/// What the hover card is about.
#[derive(Clone, Debug, PartialEq)]
enum HoverOf {
    /// A tray icon's reading ("cpu", "ram"…), the icon at this screen point.
    Tray(String, (i32, i32)),
    /// The taskbar panel.
    Panel,
}
const HOTKEY_WIDGET: i32 = 1;
const HOTKEY_FPS: i32 = 2;
/// An alert card stays this long (ms) unless clicked.
const TOAST_MS: u32 = 9000;
/// The widget slides behind the screen edge (and back) this long.
const TUCK_MS: f32 = 180.0;
/// Opacity while a game or a fullscreen video is in front ("dim").
const DIM_ALPHA: f32 = 0.3;
/// A game in front this long switches to the "game" profile; gone this long, back.
const GAME_ON: std::time::Duration = std::time::Duration::from_secs(3);
const GAME_OFF: std::time::Duration = std::time::Duration::from_secs(10);

/// A window other than the desktop covers its whole monitor (a game, a video).
fn fullscreen_app_active(own: HWND) -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_null() || fg == own {
            return false;
        }
        let mut class = [0u16; 64];
        let n = GetClassNameW(fg, class.as_mut_ptr(), class.len() as i32) as usize;
        let class = String::from_utf16_lossy(&class[..n]);
        if ["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd"].contains(&class.as_str()) {
            return false;
        }
        let mut r = RECT::default();
        if GetWindowRect(fg, &mut r) == 0 {
            return false;
        }
        let mon = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        GetMonitorInfoW(mon, &mut mi) != 0
            && r.left <= mi.rcMonitor.left
            && r.top <= mi.rcMonitor.top
            && r.right >= mi.rcMonitor.right
            && r.bottom >= mi.rcMonitor.bottom
    }
}

/// "42 мин", "1 ч 05 мин", "45 с"
fn fmt_play_time(s: f64) -> String {
    let m = (s / 60.0).round() as u64;
    if s < 60.0 {
        trf("{v} с", &[("v", format!("{s:.0}"))])
    } else if m < 60 {
        trf("{m} мин", &[("m", m.to_string())])
    } else {
        trf("{h} ч {m} мин", &[("h", (m / 60).to_string()), ("m", format!("{:02}", m % 60))])
    }
}

/// The settings; the first time 2.0 runs, 1.8's widget_config.json is taken
/// over (and left where it is).
fn load_or_migrate(path: &std::path::Path) -> Config {
    if !path.exists()
        && let Ok(text) = std::fs::read_to_string(data_dir().join("widget_config.json"))
        && let Some(c) = Config::from_1_8_json(&text)
    {
        let _ = c.save(path);
        return c;
    }
    Config::load(path)
}

/// The history database, with 1.8's history.csv taken in the first time.
fn open_history() -> Option<pcm_core::history::History> {
    use pcm_core::history::{CSV_1_8, FILE, History};
    let dir = data_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let mut h = History::open(&dir.join(FILE)).ok()?;
    if h.count().ok()? == 0
        && let Ok(text) = std::fs::read_to_string(dir.join(CSV_1_8))
    {
        let _ = h.import_csv_1_8(&text);
    }
    Some(h)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn inside(b: &BoxPx, x: f32, y: f32) -> bool {
    x >= b[0] && x < b[2] && y >= b[1] && y < b[3]
}

/// The PC Monitor window: a child process fed through its stdin by a
/// writer thread (a slow window never blocks the widget).
struct DashboardProc {
    child: std::process::Child,
    tx: std::sync::mpsc::Sender<String>,
}

/// The window asks something: its requests wait here for the widget's thread.
const WM_APP_DASH: u32 = 0x8000 + 6;
/// PC-Monitor.exe was started again by hand: open the window (at tab wParam).
const WM_APP_OPEN: u32 = 0x8000 + 7;
const WIDGET_CLASS: &str = "PCMonitor2Widget";

impl DashboardProc {
    fn start(hwnd: HWND, inbox: Arc<Mutex<Vec<FromDashboard>>>, tab: Option<usize>) -> Option<DashboardProc> {
        use std::io::{BufRead, Write};
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let exe = std::env::current_exe().ok()?;
        let tab_args = tab.map(|t| vec!["--tab".to_string(), t.to_string()]).unwrap_or_default();
        let mut child = std::process::Command::new(exe)
            .arg("--dashboard")
            .args(tab_args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .ok()?;
        let mut stdin = child.stdin.take()?;
        let stdout = child.stdout.take()?;
        let hwnd = hwnd as isize;
        std::thread::Builder::new()
            .name("dashboard-asks".into())
            .spawn(move || {
                for line in std::io::BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    let Ok(msg) = serde_json::from_str::<FromDashboard>(&line) else { continue };
                    if let Ok(mut q) = inbox.lock() {
                        q.push(msg);
                    }
                    unsafe { PostMessageW(hwnd as HWND, WM_APP_DASH, 0, 0) };
                }
            })
            .ok()?;
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        std::thread::Builder::new()
            .name("dashboard-feed".into())
            .spawn(move || {
                for line in rx {
                    if writeln!(stdin, "{line}").and_then(|_| stdin.flush()).is_err() {
                        break; // the window was closed
                    }
                }
            })
            .ok()?;
        Some(DashboardProc { child, tx })
    }

    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn send(&self, msg: &ToDashboard) -> bool {
        serde_json::to_string(msg).is_ok_and(|line| self.tx.send(line).is_ok())
    }
}

/// A slide of the widget and its tab between two positions (screen px).
#[derive(Clone, Copy)]
struct Slide {
    start: std::time::Instant,
    widget: ((i32, i32), (i32, i32)),
    tab: ((i32, i32), (i32, i32)),
    tucking: bool,
}

/// The side tab: beside the widget while the mouse is over it; alone at the
/// screen edge (docked) while the widget is tucked behind that edge.
struct TabState {
    hwnd: HWND,
    surface: Option<Surface>,
    visible: bool,
    edge_right: bool,
    pos: (i32, i32),
    hover: bool,
    misses: u8,
    /// (mouse y, tab y, moved) while the button is down on the docked tab
    press: Option<(i32, i32, bool)>,
    slide: Option<Slide>,
    cpu_shown: Option<i32>,
}

#[derive(Clone, Copy, PartialEq)]
enum Press {
    /// Pressed on something clickable or the body; may become a move.
    Body { x: i32, y: i32 },
    /// Dragging an edge or the corner: the shape changes.
    Shape { edge: Edge, corner: bool, x: i32, y: i32, width: u32, stretch: u32 },
}

struct App {
    hwnd: HWND,
    shared: Arc<Shared>,
    cfg: Config,
    cfg_path: PathBuf,
    fonts: Fonts,
    data: Option<Data>,
    boxes: Boxes,
    surface: Option<Surface>,
    size_px: (i32, i32),
    mouse_in: bool,
    hover: (f32, f32),
    close_armed: bool,
    procs_armed: Option<String>,
    press: Option<Press>,
    /// A game or a fullscreen video is in front.
    busy: bool,
    /// Hidden because of it (the "hide" mode).
    game_hidden: bool,
    tab: TabState,
    tray: Option<Tray>,
    alerts: Alerts,
    started: std::time::Instant,
    toast: HWND,
    toast_surface: Option<Surface>,
    /// The card over a tray icon or the taskbar panel (cards look).
    hover_hwnd: HWND,
    hover_surface: Option<Surface>,
    hover_of: Option<HoverOf>,
    /// The mouse is on the taskbar panel (its leave is being tracked).
    panel_mouse: bool,
    panel: Panel,
    game: GameWatch,
    overlay: Overlay,
    /// A game is in front (counted frames), whether the panel shows or not.
    game_fps: bool,
    recorder: Recorder,
    /// Sessions written in the background, waiting for their alert card.
    sessions_done: Arc<Mutex<Vec<(PathBuf, pcm_core::session::SessionInfo)>>>,
    report: ReportWin,
    /// The alert card on screen tells about this session: a click opens its report.
    toast_session: Option<PathBuf>,
    /// The alert card on screen is about an update: a click updates or opens this page.
    toast_link: Option<ToastLink>,
    /// The card's buttons: where (physical px in the card) and what they do.
    toast_buttons: Vec<([f32; 4], ToastButton)>,
    /// Since when a game is in front / since when none is (the automatic profile).
    game_seen: Option<std::time::Instant>,
    game_gone: Option<std::time::Instant>,
    /// "game" was switched on by a game, not by hand: it goes when the game does.
    auto_switched: bool,
    back_to: String,
    dashboard: Option<DashboardProc>,
    dash_inbox: Arc<Mutex<Vec<FromDashboard>>>,
    /// The history database (None if it can't be opened) and when it got
    /// its last point / was last trimmed (unix s).
    history: Option<pcm_core::history::History>,
    history_last: i64,
    history_trimmed: i64,
    /// Points waiting for the next write (history::WRITE_EVERY at a time).
    history_buf: Vec<pcm_core::history::Point>,
    /// Starts with Windows (asked once: the admin task takes schtasks.exe).
    autostart: Option<bool>,
    /// The background look for a new version, and the update under way.
    checker: Checker,
    updater: Option<Updater>,
    /// "Проверить сейчас": the check count when asked, for the answer.
    manual_check: Option<u32>,
}

/// What a click on the alert card does besides opening a session's report.
/// A button on the alert card.
#[derive(Clone)]
enum ToastButton {
    /// "Изменить порог": Settings at "Предупреждения".
    Limits,
    /// "Не показывать час": this alert rule is quiet for an hour.
    Snooze(String),
    /// "Открыть отчёт" / "Открыть историю": what a click on the card does.
    Open,
}

enum ToastLink {
    Update,
    Page(String),
    /// "Итоги недели": the window's History for the week.
    Week,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok()?.as_mut().map(f))
}

pub(crate) fn data_dir() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("PC-Monitor")
}

/// The desktop icon view (the pinned widget's owner), as 1.8 finds it.
fn desktop_view() -> HWND {
    unsafe {
        let progman = FindWindowW(wide("Progman").as_ptr(), null());
        let view_class = wide("SHELLDLL_DefView");
        let mut view = FindWindowExW(progman, null_mut(), view_class.as_ptr(), null());
        let mut worker: HWND = null_mut();
        // with a wallpaper slideshow the view lives in a WorkerW
        while view.is_null() {
            worker = FindWindowExW(null_mut(), worker, wide("WorkerW").as_ptr(), null());
            if worker.is_null() {
                break;
            }
            view = FindWindowExW(worker, null_mut(), view_class.as_ptr(), null());
        }
        view
    }
}

fn key_down(vk: u16) -> bool {
    unsafe { GetKeyState(vk as i32) < 0 }
}

impl App {
    fn scale(&self) -> f32 {
        let dpi = unsafe { GetDpiForWindow(self.hwnd) }.max(96) as f32;
        self.cfg.size as f32 / 100.0 * dpi / 96.0
    }

    fn theme(&self) -> Theme {
        match self.cfg.theme.as_str() {
            "custom" => Theme::Custom(
                hex(&self.cfg.custom_colors[0]).unwrap_or((46, 196, 182)),
                hex(&self.cfg.custom_colors[1]).unwrap_or((232, 68, 90)),
            ),
            name => Theme::from_name(name),
        }
    }

    fn hover_on(&self, b: &BoxPx) -> bool {
        self.mouse_in && inside(b, self.hover.0, self.hover.1)
    }

    /// Which edge of the shape the mouse is on (right, bottom, or the corner).
    fn edge_at(&self, x: f32, y: f32) -> Option<(Edge, bool)> {
        let s = self.scale();
        let (w, h) = (self.size_px.0 as f32, self.size_px.1 as f32);
        // the cards look has one shape (Ctrl + wheel scales it)
        if self.cfg.compact || self.cards_look() {
            return None;
        }
        let corner = inside(&self.boxes.grip, x, y) || (x >= w - GRIP * s && y >= h - GRIP * s);
        if corner {
            return Some((Edge::Right, true));
        }
        if x >= w - EDGE * s {
            return Some((Edge::Right, false));
        }
        if y >= h - EDGE * s {
            return Some((Edge::Bottom, false));
        }
        None
    }

    /// The cards or the gauges look (a fixed width, light or dark, "›").
    fn cards_look(&self) -> bool {
        WidgetLook::from_name(&self.cfg.widget_style).modern()
    }

    fn view(&self) -> View {
        let shaping = match self.press {
            Some(Press::Shape { edge, corner, .. }) => Some((edge, corner)),
            _ => None,
        };
        let edge_hover = if self.mouse_in && self.press.is_none() {
            self.edge_at(self.hover.0, self.hover.1)
        } else {
            None
        };
        let edge = shaping.or(edge_hover);
        View {
            look: WidgetLook::from_name(&self.cfg.widget_style),
            light: self.cfg.window_light,
            open_hover: self.hover_on(&self.boxes.open),
            theme: self.theme(),
            scale: self.scale(),
            width: self.cfg.width as f32,
            stretch: self.cfg.stretch as f32,
            compact: self.cfg.compact,
            hidden: self.cfg.hidden.clone(),
            pinned: self.cfg.pinned,
            pin_hover: self.hover_on(&self.boxes.pin),
            close_hover: self.hover_on(&self.boxes.close),
            close_armed: self.close_armed,
            hover_row: if self.mouse_in && self.press.is_none() {
                self.boxes.rows.iter().position(|(_, b)| inside(b, self.hover.0, self.hover.1))
            } else {
                None
            },
            grip: match edge {
                Some((_, true)) => 2,
                _ if self.mouse_in => 1,
                _ => 0,
            },
            edge: match edge {
                Some((e, false)) => Some(e),
                _ => None,
            },
        }
    }

    /// Draw the latest data and hand it to the window (premultiplied BGRA).
    fn present(&mut self) {
        if !self.cfg.widget || self.game_hidden || (self.cfg.tucked && self.tab.slide.is_none()) {
            return; // drawn again when it comes back
        }
        let Some(mut data) = self.data.clone() else { return };
        data.procs_armed = self.procs_armed.clone();
        let view = self.view();
        let Some(frame) = Renderer::new(&mut self.fonts).render(&data, &view) else { return };
        self.boxes = frame.boxes;
        let (w, h) = (frame.pm.width() as i32, frame.pm.height() as i32);
        let alpha = self.alpha();
        layered::show(self.hwnd, &mut self.surface, &frame.pm, alpha, None);
        let resized = (w, h) != self.size_px;
        self.size_px = (w, h);
        if resized {
            self.tab_follow(); // the new height moves the tab's middle
        }
    }

    fn alpha(&self) -> f32 {
        if self.busy && self.cfg.in_games == "dim" { self.cfg.alpha.min(DIM_ALPHA) } else { self.cfg.alpha }
    }

    /// Hide or fade the widget while a game or a fullscreen video is in
    /// front; everything comes back when that window goes.
    /// "game" while a game is in front (after GAME_ON), back after GAME_OFF
    /// without one, if it was a game that switched it on.
    fn auto_profile(&mut self) {
        if !self.cfg.auto_game {
            return;
        }
        let now = std::time::Instant::now();
        if self.busy {
            let seen = *self.game_seen.get_or_insert(now);
            self.game_gone = None;
            if self.cfg.profile != "game" && now - seen >= GAME_ON {
                self.back_to = self.cfg.profile.clone();
                self.switch_profile("game", true);
                self.auto_switched = true;
            }
        } else {
            self.game_seen = None;
            let gone = *self.game_gone.get_or_insert(now);
            if self.auto_switched && self.cfg.profile == "game" && now - gone >= GAME_OFF {
                self.auto_switched = false;
                let back = self.back_to.clone();
                self.switch_profile(&back, true);
            }
        }
    }

    fn switch_profile(&mut self, name: &str, auto: bool) {
        if !self.cfg.switch_profile(name) {
            return;
        }
        if !auto {
            self.auto_switched = false;
        }
        self.save();
        // the other look: pin, panel, then the widget itself
        self.apply_pin();
        self.update_panel();
        self.present();
    }

    fn check_games(&mut self) {
        let busy = self.game_fps || fullscreen_app_active(self.hwnd);
        let hide = busy && self.cfg.in_games == "hide";
        let changed = busy != self.busy;
        self.busy = busy;
        if hide != self.game_hidden {
            self.game_hidden = hide;
            // back after the game only if it was shown before: not when closed by × or tucked
            let show = !hide && self.cfg.widget && !self.cfg.tucked;
            if hide || show {
                unsafe { ShowWindow(self.hwnd, if hide { SW_HIDE } else { SW_SHOWNOACTIVATE }) };
            }
            if show {
                self.apply_pin();
                self.present();
            }
        } else if changed {
            self.present(); // the opacity changes with "dim"
        }
        if self.cfg.widget && self.cfg.tucked && self.tab.slide.is_none() {
            let hide_tab = busy && self.cfg.in_games != "off";
            if hide_tab == self.tab.visible {
                if hide_tab {
                    self.tab_hide();
                } else {
                    self.tab_render();
                }
            }
        }
        self.auto_profile();
    }

    // --- the side tab ---

    fn tab_scale(&self) -> f32 {
        unsafe { GetDpiForWindow(self.hwnd) }.max(96) as f32 / 96.0
    }

    fn tab_size(&self) -> (i32, i32) {
        let s = self.tab_scale();
        ((TAB_W * s).round() as i32, (TAB_H * s).round() as i32)
    }

    /// The work area of the monitor the widget belongs to.
    fn work_area(&self) -> RECT {
        unsafe {
            let mut r = RECT::default();
            GetWindowRect(self.hwnd, &mut r);
            if self.cfg.tucked
                && let (Some(x), Some(y)) = (self.cfg.x, self.cfg.y)
            {
                r = RECT { left: x, top: y, right: x + self.size_px.0, bottom: y + self.size_px.1 };
            }
            let mon = MonitorFromRect(&r, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            GetMonitorInfoW(mon, &mut mi);
            mi.rcWork
        }
    }

    fn widget_rect(&self) -> RECT {
        let mut r = RECT::default();
        unsafe { GetWindowRect(self.hwnd, &mut r) };
        r
    }

    fn tab_render(&mut self) {
        let cpu = self.data.as_ref().map(|d| d.cpu);
        let docked = self.cfg.tucked;
        let light = self.cards_look() && self.cfg.window_light;
        let Some(pm) = side_tab(
            self.tab_scale(),
            self.tab.edge_right,
            docked,
            self.tab.hover,
            cpu,
            &self.theme(),
            light,
        ) else {
            return;
        };
        let pos = self.tab.pos;
        layered::show(self.tab.hwnd, &mut self.tab.surface, &pm, 1.0, Some(pos));
        self.tab.cpu_shown = cpu.map(|c| c.round() as i32);
        if !self.tab.visible {
            self.tab.visible = true;
            unsafe { ShowWindow(self.tab.hwnd, SW_SHOWNOACTIVATE) };
        }
        self.tab_raise();
    }

    /// Docked, the tab stays on top of everything; beside the widget it is
    /// as high as the widget: in front of it, behind the windows covering it.
    fn tab_raise(&self) {
        const FLAGS: SET_WINDOW_POS_FLAGS = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
        let tab = self.tab.hwnd;
        let topmost = |h: HWND| unsafe { GetWindowLongW(h, GWL_EXSTYLE) } as u32 & WS_EX_TOPMOST != 0;
        unsafe {
            if self.cfg.tucked || topmost(self.hwnd) {
                SetWindowPos(tab, HWND_TOPMOST, 0, 0, 0, 0, FLAGS);
                return;
            }
            SetWindowPos(tab, HWND_NOTOPMOST, 0, 0, 0, 0, FLAGS);
            let above = GetWindow(self.hwnd, GW_HWNDPREV);
            if !above.is_null() && above != tab && !topmost(above) {
                SetWindowPos(tab, above, 0, 0, 0, 0, FLAGS); // right below it = right above the widget
            }
        }
    }

    fn tab_hide(&mut self) {
        if self.tab.visible {
            self.tab.visible = false;
            self.tab.hover = false;
            unsafe { ShowWindow(self.tab.hwnd, SW_HIDE) };
        }
    }

    /// Where the tab sits beside the widget: on the side away from the nearer
    /// screen edge, the edge the widget would go behind.
    fn tab_beside_pos(&mut self, r: &RECT) -> (i32, i32) {
        let wa = self.work_area();
        let (tw, th) = self.tab_size();
        self.tab.edge_right = (r.left + r.right) / 2 > (wa.left + wa.right) / 2;
        let x = if self.tab.edge_right { r.left - tw } else { r.right };
        (x, r.top + (r.bottom - r.top - th) / 2)
    }

    /// The mouse is over the widget: show the tab beside it.
    fn tab_beside(&mut self) {
        if self.cfg.tucked || self.tab.slide.is_some() {
            return;
        }
        self.tab.misses = 0;
        if self.tab.visible {
            return;
        }
        let r = self.widget_rect();
        self.tab.pos = self.tab_beside_pos(&r);
        self.tab_render();
        unsafe { SetTimer(self.hwnd, TIMER_TAB, 200, None) };
    }

    /// The widget moved or changed size: the tab beside it stays attached.
    fn tab_follow(&mut self) {
        if !self.tab.visible
            || self.cfg.tucked
            || self.tab.slide.is_some()
            || unsafe { IsWindowVisible(self.hwnd) } == 0
        {
            return; // docked, sliding, or the widget is coming out from behind the edge
        }
        let edge_right = self.tab.edge_right;
        let r = self.widget_rect();
        let pos = self.tab_beside_pos(&r);
        if pos != self.tab.pos || edge_right != self.tab.edge_right {
            self.tab.pos = pos;
            self.tab_render();
        }
    }

    /// Hide the tab after the mouse has been off the widget and the tab for
    /// two checks.
    fn tab_watch(&mut self) {
        if self.cfg.tucked || self.tab.slide.is_some() {
            return;
        }
        let mut pt = POINT::default();
        unsafe { GetCursorPos(&mut pt) };
        // the window really under the mouse: over a window covering the widget is not over the widget
        let under = unsafe { WindowFromPoint(pt) };
        let (over_widget, over_tab) = (under == self.hwnd, under == self.tab.hwnd);
        if over_widget || over_tab || self.press.is_some() {
            self.tab.misses = 0;
            return;
        }
        self.tab.misses += 1;
        if self.tab.misses >= 2 {
            self.tab_hide();
            unsafe { KillTimer(self.hwnd, TIMER_TAB) };
        }
    }

    /// Off-screen position of the widget behind the tab's edge.
    fn behind_edge(&self, wa: &RECT, from: (i32, i32)) -> (i32, i32) {
        let w = self.size_px.0;
        if self.tab.edge_right { (wa.right + 2, from.1) } else { (wa.left - w - 2, from.1) }
    }

    fn docked_tab_pos(&self, wa: &RECT, y: i32) -> (i32, i32) {
        let (tw, th) = self.tab_size();
        let y = y.clamp(wa.top, wa.bottom - th);
        (if self.tab.edge_right { wa.right - tw } else { wa.left }, y)
    }

    fn start_slide(
        &mut self,
        widget: ((i32, i32), (i32, i32)),
        tab: ((i32, i32), (i32, i32)),
        tucking: bool,
    ) {
        self.tab.slide = Some(Slide { start: std::time::Instant::now(), widget, tab, tucking });
        unsafe { SetTimer(self.hwnd, TIMER_SLIDE, 15, None) };
    }

    /// Slide the widget behind the screen edge; only the tab stays.
    fn tuck(&mut self) {
        let r = self.widget_rect();
        let from = (r.left, r.top);
        self.cfg.x = Some(r.left);
        self.cfg.y = Some(r.top);
        let wa = self.work_area();
        let to = self.behind_edge(&wa, from);
        let tab_to = self.docked_tab_pos(&wa, self.tab.pos.1);
        self.cfg.tab_y = Some(tab_to.1);
        self.start_slide((from, to), (self.tab.pos, tab_to), true);
    }

    /// Bring the widget back from behind the edge.
    fn untuck(&mut self) {
        let wa = self.work_area();
        let home = (self.cfg.x.unwrap_or(wa.right - 300), self.cfg.y.unwrap_or(wa.top + 40));
        let from = self.behind_edge(&wa, home);
        unsafe {
            SetWindowPos(
                self.hwnd,
                null_mut(),
                from.0,
                from.1,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        self.cfg.tucked = false;
        self.present();
        unsafe { ShowWindow(self.hwnd, SW_SHOWNOACTIVATE) };
        let r = RECT {
            left: home.0,
            top: home.1,
            right: home.0 + self.size_px.0,
            bottom: home.1 + self.size_px.1,
        };
        let tab_to = self.tab_beside_pos(&r);
        self.start_slide((from, home), (self.tab.pos, tab_to), false);
    }

    fn slide_step(&mut self) {
        let Some(sl) = self.tab.slide else {
            unsafe { KillTimer(self.hwnd, TIMER_SLIDE) };
            return;
        };
        let t = (sl.start.elapsed().as_secs_f32() * 1000.0 / TUCK_MS).min(1.0);
        let e = 1.0 - (1.0 - t).powi(3); // ease-out
        let lerp = |a: (i32, i32), b: (i32, i32)| {
            (a.0 + ((b.0 - a.0) as f32 * e).round() as i32, a.1 + ((b.1 - a.1) as f32 * e).round() as i32)
        };
        let w = lerp(sl.widget.0, sl.widget.1);
        self.tab.pos = lerp(sl.tab.0, sl.tab.1);
        unsafe {
            SetWindowPos(self.hwnd, null_mut(), w.0, w.1, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            SetWindowPos(
                self.tab.hwnd,
                null_mut(),
                self.tab.pos.0,
                self.tab.pos.1,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        if t < 1.0 {
            return;
        }
        unsafe { KillTimer(self.hwnd, TIMER_SLIDE) };
        self.tab.slide = None;
        if sl.tucking {
            unsafe { ShowWindow(self.hwnd, SW_HIDE) };
            self.cfg.tucked = true;
            self.tab.hover = false;
            self.tab_render(); // now docked: "‹" and the CPU meter
        } else {
            self.apply_pin();
            self.tab_render();
            unsafe { SetTimer(self.hwnd, TIMER_TAB, 200, None) };
        }
        self.save();
    }

    /// Started tucked: only the docked tab at the edge.
    fn dock_at_start(&mut self) {
        let wa = self.work_area();
        let home_x = self.cfg.x.unwrap_or(wa.right);
        self.tab.edge_right = home_x + self.size_px.0 / 2 > (wa.left + wa.right) / 2;
        let y = self.cfg.tab_y.unwrap_or(wa.top + (wa.bottom - wa.top) / 2);
        self.tab.pos = self.docked_tab_pos(&wa, y);
        unsafe { ShowWindow(self.hwnd, SW_HIDE) };
        self.tab_render();
    }

    /// A new CPU reading: the docked tab's meter follows it.
    fn tab_cpu(&mut self) {
        if self.cfg.tucked && self.tab.visible && self.tab.slide.is_none() {
            let now = self.data.as_ref().map(|d| d.cpu.round() as i32);
            if now != self.tab.cpu_shown {
                self.tab_render();
            }
        }
    }

    fn tab_mouse(&mut self, msg: u32, x: i32, y: i32) {
        let mut pt = POINT { x, y };
        unsafe { ClientToScreen(self.tab.hwnd, &mut pt) };
        match msg {
            WM_MOUSEMOVE => {
                if !self.tab.hover {
                    self.tab.hover = true;
                    let mut t = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: self.tab.hwnd,
                        dwHoverTime: 0,
                    };
                    unsafe { TrackMouseEvent(&mut t) };
                    self.tab_render();
                }
                self.tab.misses = 0;
                // the docked tab moves up and down with the mouse
                if let Some((my, ty, _)) = self.tab.press
                    && self.cfg.tucked
                    && (pt.y - my).abs() > DRAG_START
                {
                    let wa = self.work_area();
                    self.tab.pos = self.docked_tab_pos(&wa, ty + pt.y - my);
                    self.tab.press = Some((my, ty, true));
                    unsafe {
                        SetWindowPos(
                            self.tab.hwnd,
                            null_mut(),
                            self.tab.pos.0,
                            self.tab.pos.1,
                            0,
                            0,
                            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                        )
                    };
                }
            }
            WM_MOUSELEAVE => {
                self.tab.hover = false;
                self.tab_render();
            }
            WM_LBUTTONDOWN => {
                self.tab.press = Some((pt.y, self.tab.pos.1, false));
                unsafe { SetCapture(self.tab.hwnd) };
            }
            WM_LBUTTONUP => {
                let press = self.tab.press.take();
                unsafe { ReleaseCapture() };
                match press {
                    Some((_, _, true)) => {
                        self.cfg.tab_y = Some(self.tab.pos.1);
                        self.save();
                    }
                    Some(_) if self.tab.slide.is_none() => {
                        if self.cfg.tucked {
                            self.untuck();
                        } else {
                            self.tuck();
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// Show or hide the widget (and its tab); the tray icons stay.
    fn set_widget(&mut self, on: bool) {
        self.cfg.widget = on;
        self.save();
        if on {
            if self.cfg.tucked {
                self.tab_render();
            } else {
                unsafe { ShowWindow(self.hwnd, SW_SHOWNOACTIVATE) };
                self.apply_pin();
                self.present();
            }
        } else {
            unsafe { ShowWindow(self.hwnd, SW_HIDE) };
            self.tab_hide();
        }
    }

    /// The biggest program by memory, for the memory alert.
    fn top_ram() -> Option<(String, u64)> {
        let mut by_name: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
        for s in pcm_win::processes::ProcessReader::default().sample() {
            if s.pid == 0 {
                continue;
            }
            let name = s.image.strip_suffix(".exe").unwrap_or(&s.image).to_string();
            *by_name.entry(name).or_insert(0) += s.private_ws;
        }
        by_name.into_iter().max_by_key(|(_, m)| *m)
    }

    fn check_alerts(&mut self) {
        if !self.cfg.alerts {
            return;
        }
        let Some(d) = self.data.clone() else { return };
        let limits = Limits {
            temp: self.cfg.alert_temp as f32,
            ram: self.cfg.alert_ram as f32,
            battery: self.cfg.alert_battery as f32,
            disk: self.cfg.alert_disk as f32,
        };
        let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        let in_game = self.busy && self.cfg.quiet_games;
        let now = self.started.elapsed().as_secs_f64();
        if let Some(alert) = self.alerts.check(now, &d, &limits, &system, in_game, Self::top_ram) {
            // what can be done about it, right on the card
            let key = self.alerts.fired_key.clone().unwrap_or_default();
            let mut buttons = Vec::new();
            if crate::alerts::WITH_LIMIT.contains(&key.as_str()) {
                buttons.push((tr("Изменить порог"), ToastButton::Limits));
            }
            if !key.is_empty() {
                buttons.push((tr("Не показывать час"), ToastButton::Snooze(key)));
            }
            self.show_toast_with(&alert, buttons);
        }
    }

    /// "Итоги недели": once in 7 days (counted from the first run), not over a
    /// game, if the PC ran an hour or more: the week's PC and game time.
    fn check_weekly(&mut self) {
        use pcm_core::week;
        if !self.cfg.weekly || self.busy {
            return;
        }
        let now = pcm_win::system::unix_now();
        if self.cfg.weekly_last <= 0 {
            self.cfg.weekly_last = now;
            self.save();
            return;
        }
        if now - self.cfg.weekly_last < week::EVERY_S {
            return;
        }
        self.cfg.weekly_last = now;
        self.save();
        let from = now - week::EVERY_S;
        let points = self.history.as_ref().and_then(|h| h.range(from, now).ok()).unwrap_or_default();
        let dir = session_rec::sessions_dir(&data_dir());
        let infos: Vec<pcm_core::session::SessionInfo> = std::fs::read_dir(&dir)
            .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            .filter(|p| p.extension().is_some_and(|e| e == pcm_core::session::EXTENSION))
            .filter_map(|p| pcm_core::session::Session::load_info(&p).ok())
            .collect();
        let w = week::summarize(&points, week::since(&infos, &pcm_win::system::local_time_of(from)));
        if w.on_s < week::MIN_ON_S {
            return;
        }
        let mut body = trf("За компьютером: {time}.", &[("time", fmt_play_time(w.on_s))]);
        if let Some(g) = w.games.first() {
            body += " ";
            body += &trf(
                "В играх: {time}, больше всего — {game} ({fps} FPS).",
                &[
                    ("time", fmt_play_time(w.play_s)),
                    ("game", g.name.clone()),
                    ("fps", format!("{:.0}", g.avg_fps)),
                ],
            );
        }
        if let Some(t) = w.cpu_t_max {
            body += " ";
            body += &trf("Процессор — до {t}°.", &[("t", format!("{t:.0}"))]);
        }
        let week = Alert { title: tr("Итоги недели"), body, hot: false };
        self.show_toast_with(&week, vec![(tr("Открыть историю"), ToastButton::Open)]);
        self.toast_link = Some(ToastLink::Week);
    }

    /// The alert card in the bottom-right corner of the work area; a click closes it.
    fn show_toast(&mut self, a: &Alert) {
        self.show_toast_with(a, Vec::new());
    }

    /// The alert card with buttons along its bottom.
    fn show_toast_with(&mut self, a: &Alert, buttons: Vec<(String, ToastButton)>) {
        self.toast_session = None;
        self.toast_link = None;
        let scale = self.tab_scale();
        let sc = self.theme().scale();
        let accent = if a.hot { sc.at(1.0) } else { sc.at(0.75) };
        let names: Vec<String> = buttons.iter().map(|(n, _)| n.clone()).collect();
        let Some((pm, rects)) = alert_card_with(&mut self.fonts, scale, &a.title, &a.body, accent, &names)
        else {
            return;
        };
        self.toast_buttons = rects.into_iter().zip(buttons.into_iter().map(|(_, b)| b)).collect();
        let mut wa = RECT::default();
        unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut wa as *mut _ as *mut _, 0) };
        let gap = (14.0 * scale).round() as i32;
        let pos = (wa.right - pm.width() as i32 - gap, wa.bottom - pm.height() as i32 - gap);
        layered::show(self.toast, &mut self.toast_surface, &pm, 1.0, Some(pos));
        unsafe {
            ShowWindow(self.toast, SW_SHOWNOACTIVATE);
            SetWindowPos(self.toast, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            // a card with buttons stays longer: there's something to choose
            let ms = if self.toast_buttons.is_empty() { TOAST_MS } else { TOAST_MS * 2 };
            SetTimer(self.hwnd, TIMER_TOAST, ms, None);
        }
    }

    fn taskbar_on(&self) -> bool {
        self.cfg.taskbar.unwrap_or_else(|| !pcm_win::system::is_windows_11())
    }

    fn update_panel(&mut self) {
        if !self.taskbar_on() {
            self.panel.hide();
            return;
        }
        let scale = self.tab_scale();
        let theme = self.theme();
        if let Some(d) = &self.data {
            self.panel.update(d, &mut self.fonts, &theme, scale, self.busy);
        }
    }

    fn update_tray(&mut self) {
        let scale = self.theme().scale();
        let cards = self.cards_look();
        if let (Some(t), Some(d)) = (&mut self.tray, &self.data) {
            t.set_kinds(&self.cfg.tray);
            t.update(d, &mut self.fonts, &scale, cards);
        }
    }

    // --- the hover card (cards look): over a tray icon or the taskbar panel ---

    /// Show (or draw again) the card about `of`.
    fn hover_show(&mut self, of: HoverOf) {
        if !self.cards_look() {
            return;
        }
        let first = self.hover_of.as_ref() != Some(&of);
        self.hover_of = Some(of);
        // "больше всего": the process list while the card is up
        self.shared.want_top.store(true, Ordering::Relaxed);
        self.hover_draw(first);
    }

    fn hover_hide(&mut self) {
        unsafe {
            KillTimer(self.hwnd, TIMER_HOVER);
            ShowWindow(self.hover_hwnd, SW_HIDE);
        }
        self.hover_of = None;
        self.shared.want_top.store(false, Ordering::Relaxed);
    }

    /// The card's lines for what it is about.
    fn hover_lines(&self, of: &HoverOf) -> (String, Vec<(String, String, Rgb)>, String) {
        let sc = self.theme().scale_dark();
        let sc = if self.cfg.window_light { sc.on_light() } else { sc };
        let text = if self.cfg.window_light { LIGHT_INK.text } else { DARK_INK.text };
        let Some(d) = &self.data else { return (String::new(), Vec::new(), String::new()) };
        let pct = |v: f32| (format!("{v:.0}%"), sc.at(v / 100.0));
        let deg = |t: f32, approx: bool| (format!("{}{t:.0}°C", if approx { "≈" } else { "" }), sc.heat(t));
        let top = |by_mem: bool| -> Option<String> {
            let list = d.procs_all.as_ref()?;
            let best = if by_mem {
                list.iter().max_by_key(|p| p.memory)?
            } else {
                list.iter().max_by(|a, b| a.cpu.total_cmp(&b.cpu))?
            };
            let v =
                if by_mem { fmt_mem(best.memory) } else { format!("{}%", dec(format!("{:.1}", best.cpu))) };
            Some(format!("{} · {v}", best.name))
        };
        let mut rows: Vec<(String, String, Rgb)> = Vec::new();
        let mut push = |label: &str, (v, c): (String, Rgb)| rows.push((tr(label), v, c));
        let kind = match of {
            HoverOf::Tray(k, _) => k.as_str(),
            HoverOf::Panel => "panel",
        };
        let title = match kind {
            "cpu" | "cpu_temp" => {
                push("Нагрузка", pct(d.cpu));
                if let Some(t) = d.temp_cpu {
                    push("Температура", deg(t, d.temp_cpu_approx));
                }
                if let Some(g) = d.cpu_clock_ghz {
                    push("Частота", (trf("{v} ГГц", &[("v", dec(format!("{g:.2}")))]), text));
                }
                push("Больше всего", (top(false).unwrap_or_else(|| "…".into()), text));
                tr("Процессор")
            }
            "gpu" | "gpu_temp" => {
                push("Нагрузка", d.gpu_load.map_or(("—".into(), text), pct));
                match (d.temp_gpu, d.gpu_shared, d.temp_cpu) {
                    (Some(t), _, _) => push("Температура", deg(t, false)),
                    (None, true, Some(t)) => push("Температура", deg(t, true)),
                    _ => {}
                }
                if let (Some(u), Some(t)) = (d.vram_used_mb, d.vram_total_mb) {
                    let mb = |v: f32| fmt_mem((v as f64 * 1048576.0) as u64);
                    push("VRAM", (format!("{} / {}", mb(u), mb(t)), text));
                }
                d.gpu_name.clone().unwrap_or_else(|| tr("Видеокарта"))
            }
            "ram" => {
                push("Занято", pct(d.ram_pct));
                push("Свободно", (fmt_mem(d.ram_total.saturating_sub(d.ram_used)), text));
                push("Больше всего", (top(true).unwrap_or_else(|| "…".into()), text));
                tr("Память")
            }
            _ => {
                let mut cpu = pct(d.cpu);
                if let Some(t) = d.temp_cpu {
                    cpu.0 = format!("{} · {}", cpu.0, deg(t, d.temp_cpu_approx).0);
                }
                push("Процессор", cpu);
                push("Память", pct(d.ram_pct));
                if let Some(g) = d.gpu_load {
                    push("Видеокарта", pct(g));
                }
                push("Сеть", (format!("↓ {}  ↑ {}", fmt_speed(d.net_down), fmt_speed(d.net_up)), text));
                if let (true, Some(ms)) = (d.ping_ready, d.ping) {
                    push("Пинг", (trf("{v} мс", &[("v", ms.to_string())]), sc.ping(Some(ms))));
                }
                "PC Monitor".into()
            }
        };
        let hint = if kind == "panel" {
            tr("Клик — показать или скрыть виджет")
        } else {
            tr("Клик — открыть PC Monitor")
        };
        (title, rows, hint)
    }

    /// Draw the card and put it beside the icon / above the panel, inside
    /// the work area of that monitor (any taskbar side).
    fn hover_draw(&mut self, place: bool) {
        let Some(of) = self.hover_of.clone() else { return };
        let (title, rows, hint) = self.hover_lines(&of);
        if title.is_empty() {
            return;
        }
        let scale = self.tab_scale();
        let light = self.cfg.window_light;
        let Some(pm) = hover_card(&mut self.fonts, scale, &title, &rows, &hint, light) else { return };
        let pos = if place {
            let anchor = match &of {
                HoverOf::Tray(_, p) => *p,
                HoverOf::Panel => {
                    let mut r = RECT::default();
                    unsafe { GetWindowRect(self.panel.hwnd, &mut r) };
                    ((r.left + r.right) / 2, (r.top + r.bottom) / 2)
                }
            };
            Some(hover_place(anchor, (pm.width() as i32, pm.height() as i32), scale))
        } else {
            None
        };
        layered::show(self.hover_hwnd, &mut self.hover_surface, &pm, 1.0, pos);
        if place {
            unsafe {
                ShowWindow(self.hover_hwnd, SW_SHOWNOACTIVATE);
                SetWindowPos(
                    self.hover_hwnd,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
        }
    }

    fn save(&self) {
        let _ = self.cfg.save(&self.cfg_path);
    }

    // --- the history ---

    /// Write the waiting points (every minute, and when the program ends).
    fn flush_history(&mut self) {
        if let Some(h) = &self.history
            && !self.history_buf.is_empty()
            && h.add_all(&self.history_buf).is_ok()
        {
            self.history_buf.clear();
        }
        // a database that can't be written: don't pile points up for ever
        if self.history_buf.len() > 360 {
            self.history_buf.clear();
        }
    }

    /// A point every STEP_S seconds: the averages of the last ones (like 1.8).
    fn record_history(&mut self) {
        use pcm_core::history::{Point, STEP_S};
        let now = pcm_win::system::unix_now();
        if now - self.history_last < STEP_S {
            return;
        }
        let (Some(_), Some(d)) = (&self.history, &self.data) else { return };
        self.history_last = now;
        let cpu_recent = &d.cpu_hist[d.cpu_hist.len().saturating_sub(STEP_S as usize)..];
        let cpu = if cpu_recent.is_empty() {
            d.cpu
        } else {
            cpu_recent.iter().sum::<f32>() / cpu_recent.len() as f32
        };
        let net = &d.net_hist[d.net_hist.len().saturating_sub(STEP_S as usize)..];
        let n = net.len().max(1) as f64;
        let (down, up) = if net.is_empty() {
            (None, None)
        } else {
            (Some(net.iter().map(|x| x.0).sum::<f64>() / n), Some(net.iter().map(|x| x.1).sum::<f64>() / n))
        };
        // an integrated GPU without its own sensor runs at the CPU's temperature
        let gpu_t = d.temp_gpu.or(if d.gpu_shared { d.temp_cpu } else { None });
        let p =
            Point { t: now, cpu, cpu_t: d.temp_cpu, gpu_t, gpu: d.gpu_load, ram: Some(d.ram_pct), down, up };
        // a minute's points in one write: the disk sees 1/6 of the writes
        self.history_buf.push(p);
        if self.history_buf.len() >= pcm_core::history::WRITE_EVERY {
            self.flush_history();
        }
        let Some(h) = &self.history else { return };
        if now - self.history_trimmed >= 3600 {
            self.history_trimmed = now;
            let _ = h.trim(now);
        }
    }

    // --- the PC Monitor window ---

    /// Open the window (at this tab), or bring it to the front if it is open.
    fn open_dashboard(&mut self, tab: Option<usize>) {
        if let Some(d) = &mut self.dashboard
            && d.alive()
        {
            unsafe { AllowSetForegroundWindow(d.child.id()) };
            d.send(&ToDashboard::Focus);
            return;
        }
        self.dashboard = DashboardProc::start(self.hwnd, self.dash_inbox.clone(), tab);
        self.feed_dashboard();
    }

    /// What the window asked for: a process list, ending a program, the
    /// settings. True: restart with admin rights (the caller does it, outside
    /// `with_app`: the UAC prompt waits there while messages keep coming).
    fn dashboard_asks(&mut self) -> bool {
        let mut elevate = false;
        let asks: Vec<FromDashboard> =
            self.dash_inbox.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
        for ask in asks {
            match ask {
                FromDashboard::WantProcs(on) => self.shared.want_procs_all.store(on, Ordering::Relaxed),
                FromDashboard::EndProgram(name) => {
                    // the same rules as the widget's ×: this user's programs, never Windows' own
                    let mut keep = self.shared.pids.lock().map(|p| p.clone()).unwrap_or_default();
                    if let Some(d) = &self.dashboard {
                        keep.push(d.child.id());
                    }
                    pcm_win::processes::end_program(&name, &keep);
                }
                FromDashboard::SetConfig(c) => self.apply_settings(*c),
                FromDashboard::ResetShape => {
                    (self.cfg.width, self.cfg.stretch) = (config::WIDTH_STD, 0);
                    self.save();
                    self.present();
                }
                FromDashboard::ResetPosition => self.reset_position(),
                FromDashboard::SaveProfile => {
                    let name = self.cfg.profile.clone();
                    let view = self.cfg.view();
                    self.cfg.profiles.insert(name, view);
                    self.save();
                }
                FromDashboard::SetAutostart(on) => {
                    self.autostart = Some(elevation::set_autostart(on, self.cfg.run_as_admin))
                }
                // posted, not DestroyWindow here: WM_DESTROY needs the app (a game session is saved)
                FromDashboard::CheckUpdates => self.check_updates_now(),
                FromDashboard::StartUpdate => self.update_clicked(),
                FromDashboard::Closed => {
                    if self.cfg.close_quits {
                        unsafe { PostMessageW(self.hwnd, WM_CLOSE, 0, 0) };
                    }
                }
                FromDashboard::SetAdmin(on) => {
                    self.cfg.run_as_admin = on;
                    self.save();
                    if elevation::needs_restart(on) {
                        elevate = true;
                    } else if self.autostart.unwrap_or_else(elevation::autostart_enabled) {
                        // autostart moves between the admin task and the Run key
                        self.autostart = Some(elevation::set_autostart(true, on));
                    }
                }
            }
        }
        self.feed_dashboard(); // the window sees the result at once
        elevate
    }

    /// UAC was refused (or the copy to Program Files failed): this copy keeps
    /// running as it is, and without UAC's yes the setting is off again.
    fn elevation_failed(&mut self) {
        if !pcm_win::system::is_admin() {
            self.cfg.run_as_admin = false;
            self.save();
        }
        self.feed_dashboard();
    }

    /// Settings from the window: only what can be set there is taken (not
    /// the position, the tab, the saved profiles), then each change is applied.
    fn apply_settings(&mut self, new: Config) {
        let old = self.cfg.clone();
        let mut c = old.clone();
        c.widget = new.widget;
        c.pinned = new.pinned;
        c.topmost = new.topmost;
        c.compact = new.compact;
        c.size = new.size;
        c.alpha = new.alpha;
        c.theme = new.theme;
        c.custom_colors = new.custom_colors;
        c.hidden = new.hidden;
        c.tray = new.tray;
        c.taskbar = new.taskbar;
        c.in_games = new.in_games;
        c.fps_panel = new.fps_panel;
        c.fps_corner = new.fps_corner;
        c.fps_size = new.fps_size;
        c.fps_style = new.fps_style;
        c.fps_rows = new.fps_rows;
        c.fps_bg = new.fps_bg;
        c.auto_game = new.auto_game;
        c.alerts = new.alerts;
        c.quiet_games = new.quiet_games;
        c.alert_temp = new.alert_temp;
        c.alert_ram = new.alert_ram;
        c.alert_battery = new.alert_battery;
        c.alert_disk = new.alert_disk;
        c.ping_host = new.ping_host;
        c.lang = new.lang;
        c.close_quits = new.close_quits;
        c.window_light = new.window_light;
        c.window_zoom = new.window_zoom;
        c.window_nav_small = new.window_nav_small;
        c.widget_style = new.widget_style;
        c.check_updates = new.check_updates;
        c.weekly = new.weekly;
        c.home_hidden = new.home_hidden;
        c.home_order = new.home_order;
        c.sanitize();
        self.cfg = c;
        self.save();
        if (self.cfg.window_light, &self.cfg.widget_style) != (old.window_light, &old.widget_style) {
            // the look: tray icons and the side tab follow the widget
            self.update_tray();
            if self.tab.visible {
                self.tab_render();
            }
        }
        if self.cfg.lang != old.lang {
            // everything with words is drawn again: the widget below, the tray, the panel
            crate::apply_language(&self.cfg.lang);
            self.update_tray();
            self.update_panel();
        }
        if self.cfg.pinned != old.pinned || self.cfg.topmost != old.topmost {
            self.apply_pin();
        }
        if self.cfg.tray != old.tray {
            self.update_tray();
        }
        if self.cfg.taskbar != old.taskbar {
            self.update_panel();
        }
        if self.cfg.ping_host != old.ping_host
            && let Ok(mut h) = self.shared.ping_host.lock()
        {
            *h = self.cfg.ping_host.clone();
        }
        if self.cfg.check_updates != old.check_updates {
            self.checker.set_enabled(self.cfg.check_updates);
            self.fill_update();
        }
        if self.cfg.in_games != old.in_games {
            self.check_games();
        }
        if new.fps != old.fps {
            self.set_fps(new.fps);
        } else if (&self.cfg.fps_corner, self.cfg.fps_size, self.cfg.fps_panel, &self.cfg.fps_style)
            != (&old.fps_corner, old.fps_size, old.fps_panel, &old.fps_style)
            || (&self.cfg.fps_rows, self.cfg.fps_bg) != (&old.fps_rows, old.fps_bg)
        {
            self.fps_tick();
        }
        if new.profile != old.profile {
            self.switch_profile(&new.profile, false);
        }
        if self.cfg.widget != old.widget {
            let on = self.cfg.widget;
            self.set_widget(on);
        } else {
            self.present();
        }
    }

    /// The widget back to the top-right corner of the screen (1.8's _reset_position).
    fn reset_position(&mut self) {
        let mut wa = RECT::default();
        unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut wa as *mut _ as *mut _, 0) };
        let (x, y) = (wa.right - self.size_px.0 - SNAP_GAP, wa.top + SNAP_GAP);
        if self.cfg.tucked {
            self.cfg.x = Some(x);
            self.cfg.y = Some(y);
        } else {
            unsafe {
                SetWindowPos(self.hwnd, null_mut(), x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE)
            };
            self.remember_position();
        }
        self.save();
    }

    /// The latest readings to the window, if it is open.
    fn feed_dashboard(&mut self) {
        let Some(d) = &mut self.dashboard else { return };
        if !d.alive() {
            self.dashboard = None; // closed: its memory is Windows' again
            self.shared.want_procs_all.store(false, Ordering::Relaxed);
            return;
        }
        let Some(data) = &self.data else { return };
        let start = StartInfo {
            autostart: *self.autostart.get_or_insert_with(elevation::autostart_enabled),
            admin: pcm_win::system::is_admin(),
            helper: pcm_win::sensors_helper::find().is_some(),
        };
        let msg = ToDashboard::Feed {
            data: Box::new(data.clone()),
            theme: self.cfg.theme.clone(),
            custom: self.cfg.custom_colors.clone(),
            cfg: Box::new(self.cfg.clone()),
            start,
            game: self.recorder.live().map(Box::new),
        };
        if !d.send(&msg) {
            self.dashboard = None;
        }
    }

    // --- games ---

    /// Is a game in front? The FPS panel goes over it (or away).
    fn fps_tick(&mut self) {
        let own = [self.hwnd, self.tab.hwnd, self.toast, self.overlay.hwnd, self.panel.hwnd];
        let game = if self.cfg.fps { self.game.check(&own, fullscreen_app_active(self.hwnd)) } else { None };
        if let Some(g) = &game
            && let Some(done) = self.recorder.seen(g, self.data.as_ref())
        {
            self.finish_session(done); // another game came to the front
        }
        if let Some(done) = self.recorder.poll(self.game.counter()) {
            self.finish_session(done);
        }
        let was = self.game_fps;
        self.game_fps = game.is_some();
        match &game {
            Some(g) if self.cfg.fps_panel => {
                let info = overlay_info(g, self.data.as_ref()).keep(&self.cfg.fps_rows);
                let sc = self.theme().scale();
                let look = Look::from_name(&self.cfg.fps_style);
                let c = &self.cfg;
                self.overlay.show(&mut self.fonts, &sc, &info, g, &c.fps_corner, c.fps_size, look, c.fps_bg);
            }
            _ => self.overlay.hide(),
        }
        if was != self.game_fps {
            self.check_games(); // hide or fade the widget now, not on the next check
        }
    }

    /// Count frames or stop; Windows may refuse (an ETW session usually needs
    /// admin rights): an alert card says so.
    fn set_fps(&mut self, on: bool) {
        self.cfg.fps = on;
        self.save();
        if on {
            self.game.start();
            if let Some(e) = &self.game.error {
                let body = match e {
                    pcm_win::frames::Error::AccessDenied => {
                        tr("Windows отказала: нужны права администратора.").to_string()
                    }
                    other => trf("Windows отказала: {error}.", &[("error", other.to_string())]),
                };
                let alert = Alert { title: tr("FPS недоступен"), body, hot: false };
                self.show_toast(&alert);
            }
        } else {
            if let Some(done) = self.recorder.stop() {
                self.finish_session(done);
            }
            self.game.stop();
        }
        self.fps_tick();
    }

    fn finish_session(&self, rec: Recording) {
        let dir = session_rec::sessions_dir(&data_dir());
        session_rec::finish_in_background(rec, dir, self.sessions_done.clone(), self.hwnd);
    }

    /// "Сессия записана": the game, how long, the average FPS and 1% low.
    fn session_written(&mut self) {
        let done: Vec<_> = self.sessions_done.lock().map(|mut d| std::mem::take(&mut *d)).unwrap_or_default();
        let Some((path, info)) = done.last() else { return };
        let s = &info.summary;
        let name = if info.title.trim().is_empty() { &info.game } else { &info.title };
        let body = trf(
            "{name}: {time}, в среднем {fps} FPS, 1% low {low}.",
            &[
                ("name", name.to_string()),
                ("time", fmt_play_time(s.active_s)),
                ("fps", format!("{:.0}", s.avg_fps)),
                ("low", format!("{:.0}", s.low1)),
            ],
        );
        let alert = Alert { title: tr("Игровая сессия записана"), body, hot: false };
        let path = path.clone();
        self.show_toast_with(&alert, vec![(tr("Открыть отчёт"), ToastButton::Open)]);
        self.toast_session = Some(path);
    }

    /// A click on the alert card at (x, y), its own physical px: one of its
    /// buttons, or the card (what its text says).
    fn toast_clicked_at(&mut self, x: i32, y: i32) {
        let (x, y) = (x as f32, y as f32);
        let hit = self.toast_buttons.iter().find(|(r, _)| x >= r[0] && x < r[2] && y >= r[1] && y < r[3]);
        match hit.map(|(_, b)| b.clone()) {
            Some(ToastButton::Limits) => {
                self.toast_session = None;
                self.toast_link = None;
                self.open_dashboard(Some(crate::dashboard::TAB_SETTINGS));
                if let Some(d) = &self.dashboard {
                    d.send(&ToDashboard::Settings("Предупреждения".into()));
                }
            }
            Some(ToastButton::Snooze(key)) => {
                self.toast_session = None;
                self.toast_link = None;
                let now = self.started.elapsed().as_secs_f64();
                self.alerts.snooze(&key, now + 3600.0);
            }
            Some(ToastButton::Open) | None => self.toast_clicked(),
        }
        self.toast_buttons.clear();
    }

    /// The alert card was clicked: a session's card opens its report.
    fn toast_clicked(&mut self) {
        if let Some(path) = self.toast_session.take() {
            self.open_session_page(&path);
        }
        match self.toast_link.take() {
            Some(ToastLink::Update) => self.update_clicked(),
            Some(ToastLink::Page(url)) => updater::open_in_browser(&url),
            Some(ToastLink::Week) => {
                self.open_dashboard(Some(crate::dashboard::TAB_HISTORY));
                if let Some(d) = &self.dashboard {
                    d.send(&ToDashboard::HistoryWeek);
                }
            }
            None => {}
        }
    }

    // --- updates (1.8's _update_notice / _update_clicked / _poll_updater) ---

    /// The update row of the widget and the window: the version found, the
    /// download's progress, whether a click updates in place.
    fn fill_update(&mut self) {
        let found = self.checker.state().found.filter(|_| self.cfg.check_updates);
        let stage = self.updater.as_ref().map(Updater::stage);
        let auto = found.as_ref().is_some_and(|r| r.installable()) && updater::update_target().is_some();
        if let Some(d) = &mut self.data {
            d.update = found.map(|r| r.version);
            d.update_state = match stage {
                Some(Stage::Download(f)) => Some(UpdateState::Download(f)),
                Some(Stage::Install | Stage::Ready) => Some(UpdateState::Install),
                _ => None,
            };
            d.update_auto = auto;
        }
    }

    /// A check finished or the update moved on.
    fn update_event(&mut self) {
        self.poll_updater();
        self.update_notice();
        self.fill_update();
        self.present();
        self.feed_dashboard();
    }

    /// An alert card once per new version, and the answer to "Проверить сейчас".
    fn update_notice(&mut self) {
        let st = self.checker.state();
        let manual = self.manual_check.is_some_and(|m| st.checks > m);
        if manual {
            self.manual_check = None;
        }
        let v = crate::APP_VERSION.to_string();
        match st.found.filter(|_| self.cfg.check_updates) {
            Some(r) if manual || r.version != self.cfg.update_seen => {
                self.cfg.update_seen = r.version.clone();
                self.save();
                let auto = r.installable() && updater::update_target().is_some();
                let body = if auto {
                    trf("У тебя {v}. Нажми, чтобы обновить.", &[("v", v)])
                } else {
                    trf("У тебя {v}. Нажми, чтобы открыть страницу загрузки.", &[("v", v)])
                };
                let title = trf("Вышла версия {v}", &[("v", r.version)]);
                self.show_toast(&Alert { title, body, hot: false });
                self.toast_link = Some(ToastLink::Update);
            }
            _ if manual && st.failed => {
                let alert = Alert {
                    title: tr("Не удалось проверить"),
                    body: tr("Нет связи с GitHub. Попробуй позже."),
                    hot: true,
                };
                self.show_toast(&alert);
            }
            _ if manual => {
                let body = trf("У тебя последняя версия, {v}.", &[("v", v)]);
                self.show_toast(&Alert { title: tr("Обновлений нет"), body, hot: false });
            }
            _ => {}
        }
    }

    /// Update in place when this copy can, otherwise open the release page.
    fn update_clicked(&mut self) {
        let Some(r) = self.checker.state().found else { return };
        if self.updater.is_some() {
            return;
        }
        match updater::update_target() {
            Some(target) if r.installable() => {
                self.updater = Updater::start(&r, target, self.hwnd as isize);
                self.fill_update();
                self.present();
            }
            _ => updater::open_in_browser(&r.page),
        }
    }

    /// Ready: the new version finishes the job after we exit. Failed: say so.
    fn poll_updater(&mut self) {
        match self.updater.as_ref().map(Updater::stage) {
            Some(Stage::Ready) => {
                let u = self.updater.take().expect("an updater");
                if updater::start_finish(&u.app_dir, &u.staging) {
                    unsafe { PostMessageW(self.hwnd, WM_CLOSE, 0, 0) };
                } else {
                    self.update_failed(tr("не удалось запустить новую версию"));
                }
            }
            Some(Stage::Error(e)) => {
                self.updater = None;
                self.update_failed(e);
            }
            _ => {}
        }
    }

    fn update_failed(&mut self, error: String) {
        let page =
            self.checker.state().found.map_or_else(|| pcm_core::update::RELEASES_URL.to_string(), |r| r.page);
        let body = trf("{error}. Нажми, чтобы открыть страницу загрузки.", &[("error", error)]);
        self.show_toast(&Alert { title: tr("Не удалось обновить"), body, hot: true });
        self.toast_link = Some(ToastLink::Page(page));
    }

    /// "Проверить сейчас" (it switches the checks on, as in 1.8).
    fn check_updates_now(&mut self) {
        self.manual_check = Some(self.checker.state().checks);
        if self.cfg.check_updates {
            self.checker.check_now();
        } else {
            self.cfg.check_updates = true;
            self.save();
            self.checker.set_enabled(true);
        }
    }

    /// A session's report on the PC Monitor window's Games page; the report
    /// window of stage 4 only when the PC Monitor window can't start.
    fn open_session_page(&mut self, path: &std::path::Path) {
        self.open_dashboard(Some(crate::dashboard::TAB_GAMES));
        let sent = self
            .dashboard
            .as_mut()
            .is_some_and(|d| d.alive() && d.send(&ToDashboard::OpenSession(path.to_path_buf())));
        if !sent {
            self.open_report(path);
        }
    }

    fn open_report(&mut self, path: &std::path::Path) {
        let sc = self.theme().scale();
        if !self.report.open(path, &mut self.fonts, &sc) {
            let body = trf("Не удалось прочитать {file}.", &[("file", path.display().to_string())]);
            self.show_toast(&Alert { title: tr("Отчёт не открылся"), body, hot: false });
        }
    }

    /// Ask where to save, outside with_app (the dialog runs its own message loop).
    fn save_report_csv(hwnd: HWND) {
        let Some(name) = with_app(|a| a.report.csv_name()) else { return };
        let Some(path) = report_win::save_dialog(hwnd, &name) else { return };
        with_app(|a| {
            let file = |p: &std::path::Path| {
                p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            };
            let alert = match a.report.write_csv(&path) {
                Ok(frames) => Alert {
                    title: tr("CSV сохранён"),
                    body: trf(
                        "{a} (по секундам) и {b} (каждый кадр).",
                        &[("a", file(&path)), ("b", file(&frames))],
                    ),
                    hot: false,
                },
                Err(e) => Alert { title: tr("CSV не сохранён"), body: e.to_string(), hot: true },
            };
            a.show_toast(&alert);
        });
    }

    fn toggle_fps_panel(&mut self) {
        self.cfg.fps_panel = !self.cfg.fps_panel;
        self.save();
        self.fps_tick();
    }

    /// On the desktop (owned by the icon view, at the bottom) or a normal
    /// floating window (topmost if asked).
    fn apply_pin(&self) {
        unsafe {
            if self.cfg.pinned {
                SetWindowLongPtrW(self.hwnd, GWLP_HWNDPARENT, desktop_view() as isize);
                SetWindowPos(self.hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                SetWindowPos(self.hwnd, HWND_BOTTOM, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            } else {
                SetWindowLongPtrW(self.hwnd, GWLP_HWNDPARENT, 0);
                let z = if self.cfg.topmost { HWND_TOPMOST } else { HWND_NOTOPMOST };
                SetWindowPos(self.hwnd, z, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }
    }

    fn remember_position(&mut self) {
        let mut r = RECT::default();
        if unsafe { GetWindowRect(self.hwnd, &mut r) } != 0 {
            self.cfg.x = Some(r.left);
            self.cfg.y = Some(r.top);
            self.save();
        }
    }

    fn click(&mut self, x: f32, y: f32) {
        let b = self.boxes.clone();
        if inside(&b.close, x, y) {
            if self.close_armed {
                self.close_armed = false;
                self.set_widget(false);
                return;
            } else {
                // the first click asks for a second one
                self.close_armed = true;
                unsafe { SetTimer(self.hwnd, TIMER_DISARM, 3000, None) };
            }
        } else if inside(&b.pin, x, y) {
            self.cfg.pinned = !self.cfg.pinned;
            self.apply_pin();
            self.save();
        } else if let Some((name, _)) = b.procs.iter().find(|(_, bx)| inside(bx, x, y)) {
            if self.procs_armed.as_deref() == Some(name.as_str()) {
                let pids = self.shared.pids.lock().map(|p| p.clone()).unwrap_or_default();
                let ended = pcm_win::processes::end_program(name, &pids);
                self.procs_armed = None;
                let _ = ended;
            } else {
                self.procs_armed = Some(name.clone());
                unsafe { SetTimer(self.hwnd, TIMER_DISARM, 3000, None) };
            }
        } else if inside(&b.update, x, y) {
            self.update_clicked();
        } else if inside(&b.open, x, y) || (self.cards_look() && b.rows.iter().any(|(_, r)| inside(r, x, y)))
        {
            // the cards look: "›" or a reading opens the PC Monitor window
            self.open_dashboard(None);
        } else if inside(&b.hero, x, y) {
            let open = !self.shared.want_procs.load(Ordering::Relaxed);
            self.shared.want_procs.store(open, Ordering::Relaxed);
            self.procs_armed = None;
            if let Some(d) = &mut self.data {
                d.procs = open.then(Vec::new); // "собираю данные…" until the next sample
            }
        }
        self.present();
    }

    fn menu(&mut self) {
        const COMPACT: usize = 1;
        const TOPMOST: usize = 2;
        const CLOSE: usize = 3;
        const WIDGET: usize = 4;
        const ALERTS: usize = 5;
        const PANEL: usize = 6;
        const TRAY: usize = 60;
        const THEME: usize = 10;
        const SIZE: usize = 20;
        const ALPHA: usize = 30;
        const GAMES: usize = 40;
        const ROWS: usize = 50;
        const FPS: usize = 7;
        const FPS_PANEL: usize = 8;
        const LAST_SESSION: usize = 9;
        const AUTO_GAME: usize = 11;
        const DASHBOARD: usize = 12;
        const PROFILE: usize = 90;
        const CORNER: usize = 70;
        const FPS_SIZE: usize = 80;
        let themes = [("heat", tr("Тепло")), ("neon", tr("Неон")), ("ice", tr("Лёд"))];
        let sizes = [80u32, 100, 125, 150, 200];
        let alphas = [100u32, 90, 80, 70, 60];
        let games = [("hide", tr("Прятать")), ("dim", tr("Делать прозрачным")), ("off", tr("Не трогать"))];
        let corners = [
            ("tl", tr("Слева сверху")),
            ("tr", tr("Справа сверху")),
            ("bl", tr("Слева снизу")),
            ("br", tr("Справа снизу")),
        ];
        // the menu in the window's colours, with an icon beside each command
        // and submenu (the switches keep their check marks)
        pcm_win::menus::set_dark(!self.cfg.window_light);
        let icon_px = unsafe {
            let dpi = windows_sys::Win32::UI::HiDpi::GetDpiForWindow(self.hwnd).max(96);
            windows_sys::Win32::UI::HiDpi::GetSystemMetricsForDpi(SM_CXSMICON, dpi)
        };
        let ink = if self.cfg.window_light { (70, 72, 78) } else { (205, 208, 214) };
        let mut bitmaps = Vec::new();
        let mut icon = |menu: HMENU, glyph: &str| {
            let b = pcm_win::menus::glyph_bitmap(glyph, icon_px, ink);
            pcm_win::menus::icon_on_last(menu as isize, b);
            bitmaps.extend(b);
        };
        unsafe {
            let m = CreatePopupMenu();
            let check = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
            let widget_text = if self.cfg.widget {
                tr("Спрятать виджет (Ctrl+Alt+W)")
            } else {
                tr("Показать виджет (Ctrl+Alt+W)")
            };
            AppendMenuW(m, MF_STRING | MF_DEFAULT, DASHBOARD, wide(&tr("Открыть PC Monitor")).as_ptr());
            icon(m, "\u{E8A7}");
            AppendMenuW(m, MF_STRING, WIDGET, wide(&widget_text).as_ptr());
            icon(m, "\u{E8A9}");
            AppendMenuW(m, MF_SEPARATOR, 0, null());
            AppendMenuW(
                m,
                MF_STRING | check(self.cfg.compact),
                COMPACT,
                wide(&tr("Компактный режим")).as_ptr(),
            );
            let top_flags = MF_STRING | check(self.cfg.topmost) | if self.cfg.pinned { MF_GRAYED } else { 0 };
            AppendMenuW(m, top_flags, TOPMOST, wide(&tr("Поверх окон")).as_ptr());
            let tm = CreatePopupMenu();
            for (i, (key, name)) in themes.iter().enumerate() {
                AppendMenuW(tm, MF_STRING | check(self.cfg.theme == *key), THEME + i, wide(name).as_ptr());
            }
            AppendMenuW(m, MF_POPUP, tm as usize, wide(&tr("Цвета")).as_ptr());
            icon(m, "\u{E790}");
            let sm = CreatePopupMenu();
            for (i, s) in sizes.iter().enumerate() {
                AppendMenuW(
                    sm,
                    MF_STRING | check(self.cfg.size == *s),
                    SIZE + i,
                    wide(&format!("{s}%")).as_ptr(),
                );
            }
            AppendMenuW(m, MF_POPUP, sm as usize, wide(&tr("Размер")).as_ptr());
            icon(m, "\u{E740}");
            let am = CreatePopupMenu();
            let alpha_now = (self.cfg.alpha * 100.0).round() as u32;
            for (i, a) in alphas.iter().enumerate() {
                let on = check(alpha_now == *a);
                AppendMenuW(am, MF_STRING | on, ALPHA + i, wide(&format!("{a}%")).as_ptr());
            }
            AppendMenuW(m, MF_POPUP, am as usize, wide(&tr("Непрозрачность")).as_ptr());
            icon(m, "\u{E706}");
            let rm = CreatePopupMenu();
            for (i, (key, name)) in config::SECTIONS.iter().enumerate() {
                let on = check(!self.cfg.hidden.iter().any(|h| h == key));
                AppendMenuW(rm, MF_STRING | on, ROWS + i, wide(&tr(name)).as_ptr());
            }
            AppendMenuW(m, MF_POPUP, rm as usize, wide(&tr("Показывать")).as_ptr());
            icon(m, "\u{E890}");
            let gm = CreatePopupMenu();
            for (i, (key, name)) in games.iter().enumerate() {
                AppendMenuW(gm, MF_STRING | check(self.cfg.in_games == *key), GAMES + i, wide(name).as_ptr());
            }
            AppendMenuW(m, MF_POPUP, gm as usize, wide(&tr("В играх и видео на весь экран")).as_ptr());
            icon(m, "\u{E7FC}");
            let pm = CreatePopupMenu();
            for (i, (key, name)) in [("work", tr("Работа")), ("game", tr("Игра"))].iter().enumerate()
            {
                AppendMenuW(
                    pm,
                    MF_STRING | check(self.cfg.profile == *key),
                    PROFILE + i,
                    wide(name).as_ptr(),
                );
            }
            AppendMenuW(pm, MF_SEPARATOR, 0, null());
            let auto_text = wide(&tr("Включать «Игру», когда запущена игра"));
            AppendMenuW(pm, MF_STRING | check(self.cfg.auto_game), AUTO_GAME, auto_text.as_ptr());
            AppendMenuW(m, MF_POPUP, pm as usize, wide(&tr("Профиль")).as_ptr());
            icon(m, "\u{E77B}");
            let fm = CreatePopupMenu();
            if let Some(game) = self.recorder.game() {
                // which game is being recorded (a line to read, not a command)
                AppendMenuW(
                    fm,
                    MF_STRING | MF_GRAYED,
                    0,
                    wide(&trf("● Идёт запись: {game}", &[("game", game.to_string())])).as_ptr(),
                );
                AppendMenuW(fm, MF_SEPARATOR, 0, null());
            }
            let last = report_win::latest_session(&session_rec::sessions_dir(&data_dir()));
            let last_flags = MF_STRING | if last.is_some() { 0 } else { MF_GRAYED };
            AppendMenuW(fm, last_flags, LAST_SESSION, wide(&tr("Отчёт о последней игре")).as_ptr());
            AppendMenuW(fm, MF_SEPARATOR, 0, null());
            AppendMenuW(fm, MF_STRING | check(self.cfg.fps), FPS, wide(&tr("Считать FPS")).as_ptr());
            let off = if self.cfg.fps { 0 } else { MF_GRAYED };
            let panel_text = wide(&tr("Показывать FPS-панель (Ctrl+Alt+F)"));
            AppendMenuW(fm, MF_STRING | off | check(self.cfg.fps_panel), FPS_PANEL, panel_text.as_ptr());
            AppendMenuW(fm, MF_SEPARATOR, 0, null());
            for (i, (key, name)) in corners.iter().enumerate() {
                let on = check(self.cfg.fps_corner == *key);
                AppendMenuW(fm, MF_STRING | off | on, CORNER + i, wide(name).as_ptr());
            }
            AppendMenuW(fm, MF_SEPARATOR, 0, null());
            for (i, sz) in config::FPS_SIZES.iter().enumerate() {
                let on = check(self.cfg.fps_size == *sz);
                AppendMenuW(
                    fm,
                    MF_STRING | off | on,
                    FPS_SIZE + i,
                    wide(&trf("Размер {v}%", &[("v", sz.to_string())])).as_ptr(),
                );
            }
            AppendMenuW(m, MF_POPUP, fm as usize, wide(&tr("FPS в играх")).as_ptr());
            icon(m, "\u{E9D2}");
            let trm = CreatePopupMenu();
            for (i, (key, name)) in tray::KINDS.iter().enumerate() {
                let on = self.cfg.tray.iter().any(|k| k == key);
                // up to 3 icons, at least one
                let full = !on && self.cfg.tray.len() >= tray::MAX_ICONS;
                let last = on && self.cfg.tray.len() == 1;
                let flags = MF_STRING | check(on) | if full || last { MF_GRAYED } else { 0 };
                AppendMenuW(trm, flags, TRAY + i, wide(&tr(name)).as_ptr());
            }
            AppendMenuW(m, MF_POPUP, trm as usize, wide(&tr("Значки в трее")).as_ptr());
            icon(m, "\u{E7C4}");
            AppendMenuW(m, MF_STRING | check(self.cfg.alerts), ALERTS, wide(&tr("Предупреждения")).as_ptr());
            AppendMenuW(
                m,
                MF_STRING | check(self.taskbar_on()),
                PANEL,
                wide(&tr("Панель на панели задач")).as_ptr(),
            );
            AppendMenuW(m, MF_SEPARATOR, 0, null());
            AppendMenuW(m, MF_STRING, CLOSE, wide(&tr("Выход")).as_ptr());
            icon(m, "\u{E7E8}");
            let mut pt = POINT::default();
            GetCursorPos(&mut pt);
            SetForegroundWindow(self.hwnd);
            let cmd =
                TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, 0, self.hwnd, null()) as usize;
            DestroyMenu(m);
            for b in bitmaps {
                windows_sys::Win32::Graphics::Gdi::DeleteObject(b);
            }
            match cmd {
                COMPACT => self.cfg.compact = !self.cfg.compact,
                FPS => {
                    let on = !self.cfg.fps;
                    self.set_fps(on);
                    return;
                }
                FPS_PANEL => {
                    self.toggle_fps_panel();
                    return;
                }
                LAST_SESSION => {
                    if let Some(p) = last {
                        self.open_session_page(&p);
                    }
                    return;
                }
                c if (PROFILE..PROFILE + config::PROFILES.len()).contains(&c) => {
                    self.switch_profile(config::PROFILES[c - PROFILE], false);
                    return;
                }
                DASHBOARD => {
                    self.open_dashboard(None);
                    return;
                }
                AUTO_GAME => {
                    self.cfg.auto_game = !self.cfg.auto_game;
                    self.save();
                    return;
                }
                c if (CORNER..CORNER + corners.len()).contains(&c) => {
                    self.cfg.fps_corner = corners[c - CORNER].0.into();
                    self.save();
                    self.fps_tick();
                    return;
                }
                c if (FPS_SIZE..FPS_SIZE + config::FPS_SIZES.len()).contains(&c) => {
                    self.cfg.fps_size = config::FPS_SIZES[c - FPS_SIZE];
                    self.save();
                    self.fps_tick();
                    return;
                }
                ALERTS => self.cfg.alerts = !self.cfg.alerts,
                PANEL => {
                    self.cfg.taskbar = Some(!self.taskbar_on());
                    self.save();
                    self.update_panel();
                    return;
                }
                WIDGET => {
                    let on = !self.cfg.widget;
                    self.set_widget(on);
                    return;
                }
                c if (TRAY..TRAY + tray::KINDS.len()).contains(&c) => {
                    let key = tray::KINDS[c - TRAY].0.to_string();
                    if let Some(i) = self.cfg.tray.iter().position(|k| *k == key) {
                        self.cfg.tray.remove(i);
                    } else {
                        self.cfg.tray.push(key);
                    }
                    self.cfg.sanitize();
                    self.save();
                    self.update_tray();
                    return;
                }
                TOPMOST => {
                    self.cfg.topmost = !self.cfg.topmost;
                    self.apply_pin();
                }
                CLOSE => {
                    // posted: WM_DESTROY must not come while the app is borrowed (it saves a game session)
                    PostMessageW(self.hwnd, WM_CLOSE, 0, 0);
                    return;
                }
                c if (THEME..THEME + themes.len()).contains(&c) => {
                    self.cfg.theme = themes[c - THEME].0.into()
                }
                c if (SIZE..SIZE + sizes.len()).contains(&c) => self.cfg.size = sizes[c - SIZE],
                c if (ALPHA..ALPHA + alphas.len()).contains(&c) => {
                    self.cfg.alpha = alphas[c - ALPHA] as f32 / 100.0
                }
                c if (GAMES..GAMES + games.len()).contains(&c) => {
                    self.cfg.in_games = games[c - GAMES].0.into();
                    self.check_games();
                }
                c if (ROWS..ROWS + config::SECTIONS.len()).contains(&c) => {
                    let key = config::SECTIONS[c - ROWS].0.to_string();
                    if let Some(i) = self.cfg.hidden.iter().position(|h| *h == key) {
                        self.cfg.hidden.remove(i);
                    } else {
                        self.cfg.hidden.push(key);
                    }
                }
                _ => return,
            }
        }
        self.save();
        self.present();
    }

    fn change_shape(&mut self, mx: i32, my: i32) {
        let Some(Press::Shape { edge, corner, x, y, width, stretch }) = self.press else { return };
        let s = self.scale();
        if corner || edge == Edge::Right {
            let w = width as f32 + (mx - x) as f32 / s;
            self.cfg.width =
                (w.round() as i64).clamp(config::WIDTH_STD as i64, config::WIDTH_MAX as i64) as u32;
        }
        if corner || edge == Edge::Bottom {
            let st = stretch as f32 + (my - y) as f32 / s;
            self.cfg.stretch = (st.round() as i64).clamp(0, config::STRETCH_MAX as i64) as u32;
        }
        self.present();
    }

    /// While the window is dragged: stick to the edges of the monitor's work
    /// area (Shift turns it off).
    fn snap(&self, r: &mut RECT) {
        if key_down(VK_SHIFT) {
            return;
        }
        unsafe {
            let mon = MonitorFromRect(r, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            if GetMonitorInfoW(mon, &mut mi) == 0 {
                return;
            }
            let wa = mi.rcWork;
            let (w, h) = (r.right - r.left, r.bottom - r.top);
            let near = |a: i32, b: i32| (a - b).abs() <= SNAP_REACH;
            if near(r.left, wa.left + SNAP_GAP) {
                r.left = wa.left + SNAP_GAP;
            } else if near(r.right, wa.right - SNAP_GAP) {
                r.left = wa.right - SNAP_GAP - w;
            }
            if near(r.top, wa.top + SNAP_GAP) {
                r.top = wa.top + SNAP_GAP;
            } else if near(r.bottom, wa.bottom - SNAP_GAP) {
                r.top = wa.bottom - SNAP_GAP - h;
            }
            r.right = r.left + w;
            r.bottom = r.top + h;
        }
    }
}

fn lparam_xy(lp: LPARAM) -> (i32, i32) {
    ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_APP_DATA => {
            with_app(|a| {
                if let Ok(mut d) = a.shared.data.lock()
                    && let Some(new) = d.take()
                {
                    a.data = Some(new);
                }
                a.fill_update();
                if let Some(d) = &a.data {
                    a.recorder.sample(d);
                }
                a.present();
                a.tab_cpu();
                a.update_tray();
                a.update_panel();
                if a.hover_of.is_some() {
                    a.hover_draw(false);
                }
                a.check_alerts();
                a.check_weekly();
                a.feed_dashboard();
                a.record_history();
            });
            0
        }
        WM_MOUSEMOVE => {
            let (x, y) = lparam_xy(lp);
            let start_move = with_app(|a| {
                if !a.mouse_in {
                    let mut t = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    unsafe { TrackMouseEvent(&mut t) };
                    a.mouse_in = true;
                }
                a.hover = (x as f32, y as f32);
                a.tab_beside();
                match a.press {
                    Some(Press::Body { x: px, y: py }) if (x - px).abs() + (y - py).abs() > DRAG_START => {
                        a.press = None;
                        return true;
                    }
                    Some(Press::Shape { .. }) => {
                        let mut pt = POINT { x, y };
                        unsafe { ClientToScreen(hwnd, &mut pt) };
                        a.change_shape(pt.x, pt.y);
                    }
                    _ => a.present(),
                }
                false
            });
            if start_move == Some(true) {
                // the press becomes a move: Windows drags the window. Outside
                // with_app, so the messages of the move loop (snap, the tab
                // following, saving the place) reach the app.
                unsafe {
                    ReleaseCapture();
                    SendMessageW(hwnd, WM_SYSCOMMAND, (SC_MOVE | 2) as usize, 0);
                }
            }
            0
        }
        WM_MOUSELEAVE => {
            with_app(|a| {
                a.mouse_in = false;
                a.present();
            });
            0
        }
        WM_LBUTTONDOWN => {
            let (x, y) = lparam_xy(lp);
            with_app(|a| {
                let mut pt = POINT { x, y };
                unsafe { ClientToScreen(hwnd, &mut pt) };
                a.press = Some(match a.edge_at(x as f32, y as f32) {
                    Some((edge, corner)) => Press::Shape {
                        edge,
                        corner,
                        x: pt.x,
                        y: pt.y,
                        width: a.cfg.width,
                        stretch: a.cfg.stretch,
                    },
                    None => Press::Body { x, y },
                });
                unsafe { SetCapture(hwnd) };
            });
            0
        }
        WM_LBUTTONUP => {
            let (x, y) = lparam_xy(lp);
            with_app(|a| {
                let press = a.press.take();
                unsafe { ReleaseCapture() };
                match press {
                    Some(Press::Body { .. }) => a.click(x as f32, y as f32),
                    Some(Press::Shape { .. }) => {
                        a.save();
                        a.present();
                    }
                    None => {}
                }
            });
            0
        }
        WM_LBUTTONDBLCLK => {
            // a double click on the corner: back to the standard shape
            let (x, y) = lparam_xy(lp);
            with_app(|a| {
                if a.edge_at(x as f32, y as f32).is_some_and(|(_, corner)| corner) {
                    a.cfg.width = config::WIDTH_STD;
                    a.cfg.stretch = 0;
                    a.save();
                    a.present();
                }
            });
            0
        }
        WM_RBUTTONUP => {
            with_app(|a| a.menu());
            0
        }
        WM_MOUSEWHEEL => {
            // Ctrl + wheel: the size, in steps of 10%
            if key_down(VK_CONTROL) {
                let delta = ((wp >> 16) & 0xFFFF) as i16;
                with_app(|a| {
                    let step = if delta > 0 { 10 } else { -10 };
                    let size =
                        (a.cfg.size as i32 + step).clamp(config::SIZE_MIN as i32, config::SIZE_MAX as i32);
                    a.cfg.size = size as u32;
                    a.save();
                    a.present();
                });
            }
            0
        }
        WM_SETCURSOR => {
            let cursor = with_app(|a| {
                if !a.mouse_in {
                    return IDC_ARROW;
                }
                let (x, y) = a.hover;
                match a.press {
                    Some(Press::Shape { corner: true, .. }) => IDC_SIZENWSE,
                    Some(Press::Shape { edge: Edge::Right, .. }) => IDC_SIZEWE,
                    Some(Press::Shape { edge: Edge::Bottom, .. }) => IDC_SIZENS,
                    _ => match a.edge_at(x, y) {
                        Some((_, true)) => IDC_SIZENWSE,
                        Some((Edge::Right, _)) => IDC_SIZEWE,
                        Some((Edge::Bottom, _)) => IDC_SIZENS,
                        None => IDC_ARROW,
                    },
                }
            })
            .unwrap_or(IDC_ARROW);
            unsafe { SetCursor(LoadCursorW(null_mut(), cursor)) };
            1
        }
        WM_MOVING => {
            with_app(|a| a.snap(unsafe { &mut *(lp as *mut RECT) }));
            1
        }
        WM_EXITSIZEMOVE => {
            with_app(|a| a.remember_position());
            0
        }
        WM_WINDOWPOSCHANGED => {
            if with_app(|a| a.tab_follow()).is_none() {
                // sent while the app is busy (a Shell call waiting on explorer, the
                // menu's loop): try again later. A timer, not a posted message: a
                // message posted again and again outruns the mouse (the menu froze)
                unsafe { SetTimer(hwnd, TIMER_FOLLOW, 50, None) };
            }
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
        WM_TIMER if wp == TIMER_FOLLOW => {
            if with_app(|a| a.tab_follow()).is_some() {
                unsafe { KillTimer(hwnd, TIMER_FOLLOW) };
            }
            0
        }
        WM_APP_TRAY => {
            // NOTIFYICON_VERSION_4: LOWORD(lParam) the event, HIWORD the icon,
            // wParam the icon's point on the screen
            let event = (lp & 0xFFFF) as u32;
            let id = ((lp >> 16) & 0xFFFF) as u32;
            let at = ((wp & 0xFFFF) as i16 as i32, ((wp >> 16) & 0xFFFF) as i16 as i32);
            match event {
                // the PC Monitor window comes first; the widget: the menu or Ctrl+Alt+W
                WM_LBUTTONUP => {
                    with_app(|a| {
                        a.hover_hide();
                        a.open_dashboard(None);
                    });
                }
                WM_RBUTTONUP => {
                    with_app(|a| {
                        a.hover_hide();
                        a.menu();
                    });
                }
                NIN_POPUPOPEN => {
                    with_app(|a| {
                        if let Some(kind) = a.tray.as_ref().and_then(|t| t.kind_of(id)) {
                            a.hover_show(HoverOf::Tray(kind, at));
                        }
                    });
                }
                NIN_POPUPCLOSE => {
                    with_app(|a| {
                        if matches!(a.hover_of, Some(HoverOf::Tray(..))) {
                            a.hover_hide();
                        }
                    });
                }
                _ => {}
            }
            0
        }
        WM_TIMER if wp == TIMER_HOVER => {
            with_app(|a| {
                unsafe { KillTimer(hwnd, TIMER_HOVER) };
                if a.panel_mouse {
                    a.hover_show(HoverOf::Panel);
                }
            });
            0
        }
        WM_HOTKEY => {
            with_app(|a| {
                if wp as i32 == HOTKEY_FPS {
                    a.toggle_fps_panel(); // Ctrl+Alt+F: the FPS panel in a game
                } else {
                    let on = !a.cfg.widget; // Ctrl+Alt+W: the widget
                    a.set_widget(on);
                }
            });
            0
        }
        WM_TIMER if wp == TIMER_FPS => {
            with_app(|a| a.fps_tick());
            0
        }
        WM_APP_SESSION => {
            with_app(|a| a.session_written());
            0
        }
        WM_APP_UPDATE => {
            with_app(|a| a.update_event());
            0
        }
        WM_APP_OPEN => {
            with_app(|a| a.open_dashboard(Some(wp)));
            0
        }
        WM_APP_DASH => {
            if with_app(|a| a.dashboard_asks()).unwrap_or(false) {
                if elevation::restart_elevated(pcm_win::autostart::foreground_window()) {
                    unsafe { DestroyWindow(hwnd) }; // the elevated copy takes over
                } else {
                    with_app(|a| a.elevation_failed());
                }
            }
            0
        }
        WM_TIMER if wp == TIMER_TOAST => {
            with_app(|a| unsafe {
                KillTimer(hwnd, TIMER_TOAST);
                ShowWindow(a.toast, SW_HIDE);
            });
            0
        }
        WM_TIMER if wp == TIMER_TAB => {
            with_app(|a| a.tab_watch());
            0
        }
        WM_TIMER if wp == TIMER_SLIDE => {
            with_app(|a| a.slide_step());
            0
        }
        WM_TIMER if wp == TIMER_GAMES => {
            with_app(|a| a.check_games());
            0
        }
        WM_TIMER if wp == TIMER_DISARM => {
            with_app(|a| {
                unsafe { KillTimer(hwnd, TIMER_DISARM) };
                a.close_armed = false;
                a.procs_armed = None;
                a.present();
            });
            0
        }
        WM_DPICHANGED => {
            with_app(|a| {
                let r = unsafe { &*(lp as *const RECT) };
                unsafe {
                    SetWindowPos(
                        hwnd,
                        null_mut(),
                        r.left,
                        r.top,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                    )
                };
                a.fonts.clear_cache();
                a.present();
            });
            0
        }
        WM_DESTROY => {
            with_app(|a| {
                a.flush_history();
                if let Some(rec) = a.recorder.stop() {
                    session_rec::finish_now(rec, &session_rec::sessions_dir(&data_dir()));
                }
                a.panel.hide();
                a.shared.stop.store(true, Ordering::Relaxed);
                a.shared.hwnd.store(0, Ordering::Relaxed);
            });
            unsafe { PostQuitMessage(0) };
            0
        }
        _ => {
            let taskbar =
                with_app(|a| a.tray.as_ref().is_some_and(|t| t.taskbar_created == msg)).unwrap_or(false);
            if taskbar {
                // explorer.exe restarted: the icons go in again, the pin finds the new desktop
                with_app(|a| {
                    if let Some(t) = &mut a.tray {
                        t.readd();
                    }
                    a.apply_pin();
                    a.update_tray();
                });
                return 0;
            }
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
    }
}

unsafe extern "system" fn panel_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEMOVE => {
            with_app(|a| {
                if !a.panel_mouse {
                    a.panel_mouse = true;
                    let mut t = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    unsafe {
                        TrackMouseEvent(&mut t);
                        SetTimer(a.hwnd, TIMER_HOVER, HOVER_DELAY_MS, None);
                    }
                }
            });
            0
        }
        WM_MOUSELEAVE => {
            with_app(|a| {
                a.panel_mouse = false;
                if a.hover_of == Some(HoverOf::Panel) || a.hover_of.is_none() {
                    a.hover_hide();
                }
            });
            0
        }
        WM_LBUTTONUP => {
            with_app(|a| {
                a.hover_hide();
                let on = !a.cfg.widget;
                a.set_widget(on);
            });
            0
        }
        WM_RBUTTONUP => {
            with_app(|a| a.menu());
            0
        }
        WM_SETCURSOR => {
            unsafe { SetCursor(LoadCursorW(null_mut(), IDC_HAND)) };
            1
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// Where a card of `size` goes for an icon or panel at `anchor`: beside the
/// taskbar the anchor is on (outside the work area), inside the work area.
fn hover_place(anchor: (i32, i32), size: (i32, i32), scale: f32) -> (i32, i32) {
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
    };
    let gap = (8.0 * scale).round() as i32;
    let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..unsafe { std::mem::zeroed() } };
    let mon = unsafe { MonitorFromPoint(POINT { x: anchor.0, y: anchor.1 }, MONITOR_DEFAULTTONEAREST) };
    if unsafe { GetMonitorInfoW(mon, &mut mi) } == 0 {
        return (anchor.0 - size.0 / 2, anchor.1 - size.1 - gap);
    }
    let wa = mi.rcWork;
    let clamp_x = |x: i32| x.clamp(wa.left + gap, (wa.right - size.0 - gap).max(wa.left + gap));
    let clamp_y = |y: i32| y.clamp(wa.top + gap, (wa.bottom - size.1 - gap).max(wa.top + gap));
    if anchor.1 >= wa.bottom {
        (clamp_x(anchor.0 - size.0 / 2), wa.bottom - size.1 - gap) // taskbar at the bottom
    } else if anchor.1 < wa.top {
        (clamp_x(anchor.0 - size.0 / 2), wa.top + gap)
    } else if anchor.0 >= wa.right {
        (wa.right - size.0 - gap, clamp_y(anchor.1 - size.1 / 2))
    } else if anchor.0 < wa.left {
        (wa.left + gap, clamp_y(anchor.1 - size.1 / 2))
    } else {
        // inside (the Windows 11 overflow menu): above the point
        (clamp_x(anchor.0 - size.0 / 2), clamp_y(anchor.1 - size.1 - gap))
    }
}

unsafe extern "system" fn toast_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_LBUTTONUP => {
            unsafe { ShowWindow(hwnd, SW_HIDE) };
            let (x, y) = lparam_xy(lp);
            with_app(|a| a.toast_clicked_at(x, y));
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

unsafe extern "system" fn report_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEMOVE => {
            let (x, y) = lparam_xy(lp);
            let mut t = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            unsafe { TrackMouseEvent(&mut t) };
            with_app(|a| {
                let sc = a.theme().scale();
                a.report.mouse_move(x as f32, y as f32, &mut a.fonts, &sc);
            });
            0
        }
        WM_MOUSELEAVE => {
            with_app(|a| {
                let sc = a.theme().scale();
                a.report.mouse_move(-1.0, -1.0, &mut a.fonts, &sc);
            });
            0
        }
        WM_LBUTTONDOWN => {
            let (x, y) = lparam_xy(lp);
            // decided inside with_app, done outside it: the drag and the
            // dialog run message loops of their own
            match with_app(|a| a.report.press(x as f32, y as f32)) {
                Some(Action::Drag) => unsafe {
                    ReleaseCapture();
                    SendMessageW(hwnd, WM_SYSCOMMAND, (SC_MOVE | 2) as usize, 0);
                },
                Some(Action::Close) => {
                    with_app(|a| a.report.close());
                }
                Some(Action::SaveCsv) => App::save_report_csv(hwnd),
                None => {}
            }
            0
        }
        WM_KEYDOWN if wp == VK_ESCAPE as usize => {
            with_app(|a| a.report.close());
            0
        }
        WM_CLOSE => {
            with_app(|a| a.report.close());
            0
        }
        WM_DPICHANGED => {
            let r = unsafe { &*(lp as *const RECT) };
            unsafe { SetWindowPos(hwnd, null_mut(), r.left, r.top, 0, 0, SWP_NOSIZE | SWP_NOZORDER) };
            with_app(|a| {
                let sc = a.theme().scale();
                a.fonts.clear_cache();
                a.report.redraw(&mut a.fonts, &sc);
            });
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

unsafe extern "system" fn tab_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEMOVE | WM_MOUSELEAVE | WM_LBUTTONDOWN | WM_LBUTTONUP => {
            let (x, y) = lparam_xy(lp);
            with_app(|a| a.tab_mouse(msg, x, y));
            0
        }
        WM_RBUTTONUP => {
            with_app(|a| a.menu());
            0
        }
        WM_SETCURSOR => {
            unsafe { SetCursor(LoadCursorW(null_mut(), IDC_HAND)) };
            1
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// The running copy is to open its window (we were started again by hand).
fn ask_running_copy(tab: usize) {
    unsafe {
        let hwnd = FindWindowW(wide(WIDGET_CLASS).as_ptr(), null());
        if !hwnd.is_null() {
            AllowSetForegroundWindow(ASFW_ANY); // its window may come to the front
            PostMessageW(hwnd, WM_APP_OPEN, tab, 0);
        }
    }
}

/// Run the widget until it is closed, with the PC Monitor window open at
/// tab `open` (None: quietly, as at autostart) or a session's report.
/// Returns the exit code.
pub fn run(report: Option<String>, open: Option<usize>) -> i32 {
    unsafe {
        // one widget at a time; started again by hand, the running one opens its window
        if elevation::already_running() {
            if let Some(tab) = open {
                ask_running_copy(tab);
            }
            return 0;
        }
        let cfg_path = data_dir().join("config-2.toml");
        let cfg = load_or_migrate(&cfg_path);
        let check_updates = cfg.check_updates;
        let args: Vec<String> = std::env::args().skip(1).collect();
        if let elevation::Start::Leave = elevation::at_start(cfg.run_as_admin, &args) {
            return 0; // an elevated copy took over
        }
        // work from the data folder, not the program folder: a process sitting in
        // it (and its children: the helper, the window) would block an update
        let _ = std::env::set_current_dir(data_dir());
        updater::clean_leftovers();
        elevation::sync_uninstall_version();
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        crate::apply_language(&cfg.lang);
        let shared = Shared::new(cfg.ping_host.clone());

        let hinst = GetModuleHandleW(null());
        let class = wide(WIDGET_CLASS);
        let wc = WNDCLASSW {
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: class.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hIcon: crate::icon::class_icon(),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
        let title = wide("PC Monitor");
        let mut work = RECT::default();
        SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut work as *mut _ as *mut _, 0);
        let (x, y) = match (cfg.x, cfg.y) {
            (Some(x), Some(y)) => (x, y),
            _ => (work.right - 300, work.top + 40),
        };
        let hwnd = CreateWindowExW(
            ex,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            x,
            y,
            264,
            600,
            null_mut(),
            null_mut(),
            hinst,
            null(),
        );
        if hwnd.is_null() {
            return 1;
        }
        shared.hwnd.store(hwnd as isize, Ordering::Relaxed);
        let tab_class = wide("PCMonitor2Tab");
        let tc = WNDCLASSW {
            lpfnWndProc: Some(tab_proc),
            hInstance: hinst,
            lpszClassName: tab_class.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_HAND),
            ..Default::default()
        };
        RegisterClassW(&tc);
        let panel_class = wide("PCMonitor2Panel");
        let pc = WNDCLASSW {
            lpfnWndProc: Some(panel_proc),
            hInstance: hinst,
            lpszClassName: panel_class.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_HAND),
            ..Default::default()
        };
        RegisterClassW(&pc);
        let panel_hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
            panel_class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            0,
            0,
            200,
            40,
            null_mut(),
            null_mut(),
            hinst,
            null(),
        );
        let toast_class = wide("PCMonitor2Toast");
        let toc = WNDCLASSW {
            lpfnWndProc: Some(toast_proc),
            hInstance: hinst,
            lpszClassName: toast_class.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_HAND),
            ..Default::default()
        };
        RegisterClassW(&toc);
        let tab_ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST;
        let toast_hwnd = CreateWindowExW(
            tab_ex,
            toast_class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            0,
            0,
            300,
            80,
            null_mut(),
            null_mut(),
            hinst,
            null(),
        );
        let tab_hwnd = CreateWindowExW(
            tab_ex,
            tab_class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            0,
            0,
            14,
            60,
            null_mut(),
            null_mut(),
            hinst,
            null(),
        );
        let overlay_class = wide("PCMonitor2Overlay");
        let oc = WNDCLASSW {
            lpfnWndProc: Some(DefWindowProcW),
            hInstance: hinst,
            lpszClassName: overlay_class.as_ptr(),
            ..Default::default()
        };
        RegisterClassW(&oc);
        // clicks go through it to the game
        let hover_hwnd = CreateWindowExW(
            tab_ex | WS_EX_TRANSPARENT,
            overlay_class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            0,
            0,
            250,
            120,
            null_mut(),
            null_mut(),
            hinst,
            null(),
        );
        let overlay_hwnd = CreateWindowExW(
            tab_ex | WS_EX_TRANSPARENT,
            overlay_class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            0,
            0,
            200,
            60,
            null_mut(),
            null_mut(),
            hinst,
            null(),
        );
        let report_class = wide("PCMonitor2Report");
        let rc = WNDCLASSW {
            lpfnWndProc: Some(report_proc),
            hInstance: hinst,
            lpszClassName: report_class.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hIcon: crate::icon::class_icon(),
            ..Default::default()
        };
        RegisterClassW(&rc);
        let report_title = wide(&tr("PC Monitor: игровая сессия"));
        let report_hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_APPWINDOW,
            report_class.as_ptr(),
            report_title.as_ptr(),
            WS_POPUP,
            0,
            0,
            780,
            480,
            null_mut(),
            null_mut(),
            hinst,
            null(),
        );
        APP.with(|a| {
            *a.borrow_mut() = Some(App {
                hwnd,
                shared: shared.clone(),
                cfg,
                cfg_path,
                fonts: Fonts::system(),
                data: None,
                boxes: Boxes::default(),
                surface: None,
                size_px: (0, 0),
                mouse_in: false,
                hover: (0.0, 0.0),
                close_armed: false,
                procs_armed: None,
                press: None,
                busy: false,
                game_hidden: false,
                tab: TabState {
                    hwnd: tab_hwnd,
                    surface: None,
                    visible: false,
                    edge_right: true,
                    pos: (0, 0),
                    hover: false,
                    misses: 0,
                    press: None,
                    slide: None,
                    cpu_shown: None,
                },
                tray: Some(Tray::new(hwnd)),
                alerts: Alerts::default(),
                started: std::time::Instant::now(),
                toast: toast_hwnd,
                toast_surface: None,
                hover_hwnd,
                hover_surface: None,
                hover_of: None,
                panel_mouse: false,
                panel: Panel::new(panel_hwnd),
                game: GameWatch::default(),
                overlay: Overlay::new(overlay_hwnd),
                game_fps: false,
                recorder: Recorder::default(),
                sessions_done: Arc::new(Mutex::new(Vec::new())),
                report: ReportWin::new(report_hwnd),
                toast_session: None,
                toast_link: None,
                toast_buttons: Vec::new(),
                game_seen: None,
                game_gone: None,
                auto_switched: false,
                back_to: "work".into(),
                dashboard: None,
                dash_inbox: Arc::new(Mutex::new(Vec::new())),
                history: open_history(),
                history_last: 0,
                history_trimmed: 0,
                history_buf: Vec::new(),
                autostart: None,
                checker: Checker::start(hwnd as isize, check_updates),
                updater: None,
                manual_check: None,
            })
        });
        collect::start(shared.clone());
        with_app(|a| a.apply_pin());
        SetTimer(hwnd, TIMER_GAMES, 1000, None);
        RegisterHotKey(hwnd, HOTKEY_WIDGET, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, 'W' as u32);
        RegisterHotKey(hwnd, HOTKEY_FPS, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, 'F' as u32);
        with_app(|a| {
            if a.cfg.fps {
                a.game.start(); // quietly: an alert card only when switched on by hand
            }
        });
        SetTimer(hwnd, TIMER_FPS, 250, None);
        let (tucked, visible) = with_app(|a| (a.cfg.tucked, a.cfg.widget)).unwrap_or((false, true));
        if visible && tucked {
            with_app(|a| a.dock_at_start());
        } else if visible {
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }

        if let Some(path) = report {
            with_app(|a| a.open_report(std::path::Path::new(&path)));
        }
        let prefer_task = with_app(|a| a.cfg.run_as_admin).unwrap_or(false);
        elevation::check_autostart(prefer_task);
        // a copy started by hand (even a normal one, when this one is elevated) may ask for the window
        ChangeWindowMessageFilterEx(hwnd, WM_APP_OPEN, MSGFLT_ALLOW, null_mut());
        if let Some(tab) = open {
            with_app(|a| a.open_dashboard(Some(tab)));
        }

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        UnregisterHotKey(hwnd, HOTKEY_WIDGET);
        UnregisterHotKey(hwnd, HOTKEY_FPS);
        APP.with(|a| a.borrow_mut().take());
        elevation::release_instance();
        0
    }
}

/// Draw the widget with live data (after a few seconds of samples) into a PNG.
pub fn png(path: &str) -> i32 {
    let cfg = Config::load(&data_dir().join("config-2.toml"));
    crate::apply_language(&cfg.lang);
    let shared = Shared::new(cfg.ping_host.clone());
    collect::start(shared.clone());
    std::thread::sleep(std::time::Duration::from_millis(3500));
    shared.stop.store(true, Ordering::Relaxed);
    let Some(data) = shared.data.lock().ok().and_then(|mut d| d.take()) else {
        eprintln!("no data");
        return 1;
    };
    let theme = Theme::from_name(&cfg.theme);
    let view = View {
        look: WidgetLook::from_name(&cfg.widget_style),
        light: cfg.window_light,
        theme,
        scale: cfg.size as f32 / 100.0,
        width: cfg.width as f32,
        stretch: cfg.stretch as f32,
        compact: cfg.compact,
        hidden: cfg.hidden.clone(),
        pinned: cfg.pinned,
        ..View::default()
    };
    let mut fonts = Fonts::system();
    match Renderer::new(&mut fonts).render(&data, &view) {
        Some(f) if f.pm.save_png(path).is_ok() => 0,
        _ => 1,
    }
}

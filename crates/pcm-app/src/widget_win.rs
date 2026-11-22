//! The desktop widget: a layered window that shows pcm-render's frames
//! (true transparency, no window chrome). Drag it anywhere; drag the right
//! or bottom edge or the corner to change its shape; Ctrl + wheel changes its
//! size; the pin keeps it on the desktop (it survives Win+D); a right click
//! opens the menu.

use std::cell::RefCell;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use pcm_core::config::{self, Config};
use pcm_render::text::Fonts;
use pcm_render::theme::{Theme, hex};
use pcm_render::widget::{
    BoxPx, Boxes, Data, Edge, GRIP, Renderer, TAB_H, TAB_W, View, alert_card, side_tab,
};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, RegisterHotKey, ReleaseCapture, SetCapture, TME_LEAVE,
    TRACKMOUSEEVENT, TrackMouseEvent, UnregisterHotKey, VK_CONTROL, VK_SHIFT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// The mouse left the window (TrackMouseEvent); from CommCtrl.h.
const WM_MOUSELEAVE: u32 = 0x02A3;
/// Put the side tab beside the widget again (a move it missed).
const WM_APP_FOLLOW: u32 = 0x8000 + 4;

use crate::alerts::{Alert, Alerts, Limits};
use crate::collect::{self, Shared, WM_APP_DATA};
use crate::layered::{self, Surface};
use crate::taskbar::Panel;
use crate::tray::{self, Tray, WM_APP_TRAY};

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
/// An alert card stays this long (ms) unless clicked.
const TOAST_MS: u32 = 9000;
/// The widget slides behind the screen edge (and back) this long.
const TUCK_MS: f32 = 180.0;
/// Opacity while a game or a fullscreen video is in front ("dim").
const DIM_ALPHA: f32 = 0.3;

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

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn inside(b: &BoxPx, x: f32, y: f32) -> bool {
    x >= b[0] && x < b[2] && y >= b[1] && y < b[3]
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
    panel: Panel,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok()?.as_mut().map(f))
}

fn data_dir() -> PathBuf {
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
        if self.cfg.compact {
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
    fn check_games(&mut self) {
        let busy = fullscreen_app_active(self.hwnd);
        let hide = busy && self.cfg.in_games == "hide";
        let changed = busy != self.busy;
        self.busy = busy;
        if hide != self.game_hidden {
            self.game_hidden = hide;
            unsafe { ShowWindow(self.hwnd, if hide { SW_HIDE } else { SW_SHOWNOACTIVATE }) };
            if !hide {
                self.apply_pin();
                self.present();
            }
        } else if changed {
            self.present(); // the opacity changes with "dim"
        }
        if self.cfg.tucked && self.tab.slide.is_none() {
            let hide_tab = busy && self.cfg.in_games != "off";
            if hide_tab == self.tab.visible {
                if hide_tab {
                    self.tab_hide();
                } else {
                    self.tab_render();
                }
            }
        }
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
        let Some(pm) =
            side_tab(self.tab_scale(), self.tab.edge_right, docked, self.tab.hover, cpu, &self.theme())
        else {
            return;
        };
        let pos = self.tab.pos;
        layered::show(self.tab.hwnd, &mut self.tab.surface, &pm, 1.0, Some(pos));
        self.tab.cpu_shown = cpu.map(|c| c.round() as i32);
        if !self.tab.visible {
            self.tab.visible = true;
            unsafe {
                ShowWindow(self.tab.hwnd, SW_SHOWNOACTIVATE);
                SetWindowPos(
                    self.tab.hwnd,
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
        let r = self.widget_rect();
        let (tw, th) = self.tab_size();
        let over_widget = pt.x >= r.left && pt.x < r.right && pt.y >= r.top && pt.y < r.bottom;
        let (tx, ty) = self.tab.pos;
        let over_tab = pt.x >= tx && pt.x < tx + tw && pt.y >= ty && pt.y < ty + th;
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
            self.show_toast(&alert);
        }
    }

    /// The alert card in the bottom-right corner of the work area; a click closes it.
    fn show_toast(&mut self, a: &Alert) {
        let scale = self.tab_scale();
        let sc = self.theme().scale();
        let accent = if a.hot { sc.at(1.0) } else { sc.at(0.75) };
        let Some(pm) = alert_card(&mut self.fonts, scale, &a.title, &a.body, accent) else { return };
        let mut wa = RECT::default();
        unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut wa as *mut _ as *mut _, 0) };
        let gap = (14.0 * scale).round() as i32;
        let pos = (wa.right - pm.width() as i32 - gap, wa.bottom - pm.height() as i32 - gap);
        layered::show(self.toast, &mut self.toast_surface, &pm, 1.0, Some(pos));
        unsafe {
            ShowWindow(self.toast, SW_SHOWNOACTIVATE);
            SetWindowPos(self.toast, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            SetTimer(self.hwnd, TIMER_TOAST, TOAST_MS, None);
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
        if let (Some(t), Some(d)) = (&mut self.tray, &self.data) {
            t.set_kinds(&self.cfg.tray);
            t.update(d, &mut self.fonts, &scale);
        }
    }

    fn save(&self) {
        let _ = self.cfg.save(&self.cfg_path);
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
        let themes = [("heat", "Тепло"), ("neon", "Неон"), ("ice", "Лёд")];
        let sizes = [80u32, 100, 125, 150, 200];
        let alphas = [100u32, 90, 80, 70, 60];
        let games = [("hide", "Прятать"), ("dim", "Делать прозрачным"), ("off", "Не трогать")];
        unsafe {
            let m = CreatePopupMenu();
            let check = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
            let widget_text = if self.cfg.widget {
                "Спрятать виджет (Ctrl+Alt+W)"
            } else {
                "Показать виджет (Ctrl+Alt+W)"
            };
            AppendMenuW(m, MF_STRING, WIDGET, wide(widget_text).as_ptr());
            AppendMenuW(m, MF_SEPARATOR, 0, null());
            AppendMenuW(m, MF_STRING | check(self.cfg.compact), COMPACT, wide("Компактный режим").as_ptr());
            let top_flags = MF_STRING | check(self.cfg.topmost) | if self.cfg.pinned { MF_GRAYED } else { 0 };
            AppendMenuW(m, top_flags, TOPMOST, wide("Поверх окон").as_ptr());
            let tm = CreatePopupMenu();
            for (i, (key, name)) in themes.iter().enumerate() {
                AppendMenuW(tm, MF_STRING | check(self.cfg.theme == *key), THEME + i, wide(name).as_ptr());
            }
            AppendMenuW(m, MF_POPUP, tm as usize, wide("Цвета").as_ptr());
            let sm = CreatePopupMenu();
            for (i, s) in sizes.iter().enumerate() {
                AppendMenuW(
                    sm,
                    MF_STRING | check(self.cfg.size == *s),
                    SIZE + i,
                    wide(&format!("{s}%")).as_ptr(),
                );
            }
            AppendMenuW(m, MF_POPUP, sm as usize, wide("Размер").as_ptr());
            let am = CreatePopupMenu();
            let alpha_now = (self.cfg.alpha * 100.0).round() as u32;
            for (i, a) in alphas.iter().enumerate() {
                let on = check(alpha_now == *a);
                AppendMenuW(am, MF_STRING | on, ALPHA + i, wide(&format!("{a}%")).as_ptr());
            }
            AppendMenuW(m, MF_POPUP, am as usize, wide("Непрозрачность").as_ptr());
            let rm = CreatePopupMenu();
            for (i, (key, name)) in config::SECTIONS.iter().enumerate() {
                let on = check(!self.cfg.hidden.iter().any(|h| h == key));
                AppendMenuW(rm, MF_STRING | on, ROWS + i, wide(name).as_ptr());
            }
            AppendMenuW(m, MF_POPUP, rm as usize, wide("Показывать").as_ptr());
            let gm = CreatePopupMenu();
            for (i, (key, name)) in games.iter().enumerate() {
                AppendMenuW(gm, MF_STRING | check(self.cfg.in_games == *key), GAMES + i, wide(name).as_ptr());
            }
            AppendMenuW(m, MF_POPUP, gm as usize, wide("В играх и видео на весь экран").as_ptr());
            let trm = CreatePopupMenu();
            for (i, (key, name)) in tray::KINDS.iter().enumerate() {
                let on = self.cfg.tray.iter().any(|k| k == key);
                // up to 3 icons, at least one
                let full = !on && self.cfg.tray.len() >= tray::MAX_ICONS;
                let last = on && self.cfg.tray.len() == 1;
                let flags = MF_STRING | check(on) | if full || last { MF_GRAYED } else { 0 };
                AppendMenuW(trm, flags, TRAY + i, wide(name).as_ptr());
            }
            AppendMenuW(m, MF_POPUP, trm as usize, wide("Значки в трее").as_ptr());
            AppendMenuW(m, MF_STRING | check(self.cfg.alerts), ALERTS, wide("Предупреждения").as_ptr());
            AppendMenuW(
                m,
                MF_STRING | check(self.taskbar_on()),
                PANEL,
                wide("Панель на панели задач").as_ptr(),
            );
            AppendMenuW(m, MF_SEPARATOR, 0, null());
            AppendMenuW(m, MF_STRING, CLOSE, wide("Выход").as_ptr());
            let mut pt = POINT::default();
            GetCursorPos(&mut pt);
            SetForegroundWindow(self.hwnd);
            let cmd =
                TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, 0, self.hwnd, null()) as usize;
            DestroyMenu(m);
            match cmd {
                COMPACT => self.cfg.compact = !self.cfg.compact,
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
                    DestroyWindow(self.hwnd);
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
                a.present();
                a.tab_cpu();
                a.update_tray();
                a.update_panel();
                a.check_alerts();
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
        WM_WINDOWPOSCHANGED | WM_APP_FOLLOW => {
            if with_app(|a| a.tab_follow()).is_none() {
                // sent while the app is busy (a Shell call waiting on explorer
                // lets it in): try again right after
                unsafe { PostMessageW(hwnd, WM_APP_FOLLOW, 0, 0) };
            }
            if msg == WM_APP_FOLLOW { 0 } else { unsafe { DefWindowProcW(hwnd, msg, wp, lp) } }
        }
        WM_APP_TRAY => {
            match lp as u32 {
                WM_LBUTTONUP => {
                    with_app(|a| {
                        let on = !a.cfg.widget;
                        a.set_widget(on);
                    });
                }
                WM_RBUTTONUP => {
                    with_app(|a| a.menu());
                }
                _ => {}
            }
            0
        }
        WM_HOTKEY => {
            // Ctrl+Alt+W: show or hide the widget
            with_app(|a| {
                let on = !a.cfg.widget;
                a.set_widget(on);
            });
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
        WM_LBUTTONUP => {
            with_app(|a| {
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

unsafe extern "system" fn toast_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_LBUTTONUP => {
            unsafe { ShowWindow(hwnd, SW_HIDE) };
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

/// Run the widget until it is closed. Returns the exit code.
pub fn run() -> i32 {
    unsafe {
        // one widget at a time (not 1.8's mutex: both may run during the alpha)
        let mutex = CreateMutexW(null(), 1, wide("Local\\PCMonitor2Widget").as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return 0;
        }
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let cfg_path = data_dir().join("config-2.toml");
        let cfg = Config::load(&cfg_path);
        let shared = Shared::new(cfg.ping_host.clone());

        let hinst = GetModuleHandleW(null());
        let class = wide("PCMonitor2Widget");
        let wc = WNDCLASSW {
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: class.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
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
                panel: Panel::new(panel_hwnd),
            })
        });
        collect::start(shared.clone());
        with_app(|a| a.apply_pin());
        SetTimer(hwnd, TIMER_GAMES, 1000, None);
        RegisterHotKey(hwnd, 1, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, 'W' as u32);
        let (tucked, visible) = with_app(|a| (a.cfg.tucked, a.cfg.widget)).unwrap_or((false, true));
        if visible && tucked {
            with_app(|a| a.dock_at_start());
        } else if visible {
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        UnregisterHotKey(hwnd, 1);
        APP.with(|a| a.borrow_mut().take());
        CloseHandle(mutex);
        0
    }
}

/// Draw the widget with live data (after a few seconds of samples) into a PNG.
pub fn png(path: &str) -> i32 {
    let cfg = Config::load(&data_dir().join("config-2.toml"));
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

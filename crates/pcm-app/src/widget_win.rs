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
use pcm_render::widget::{BoxPx, Boxes, Data, Edge, GRIP, Renderer, View};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_CONTROL,
    VK_SHIFT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// The mouse left the window (TrackMouseEvent); from CommCtrl.h.
const WM_MOUSELEAVE: u32 = 0x02A3;

use crate::collect::{self, Shared, WM_APP_DATA};

/// The right and bottom edges are this wide for dragging the shape (logical px).
const EDGE: f32 = 6.0;
/// A dragged widget sticks this far from a screen edge, within this reach.
const SNAP_GAP: i32 = 10;
const SNAP_REACH: i32 = 16;
/// Mouse travel that turns a press into a drag (physical px).
const DRAG_START: i32 = 4;
const TIMER_DISARM: usize = 1;
const TIMER_GAMES: usize = 2;
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

/// A DIB section kept between frames for UpdateLayeredWindow.
struct Surface {
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    w: i32,
    h: i32,
}

impl Surface {
    fn new(w: i32, h: i32) -> Surface {
        unsafe {
            let screen = GetDC(null_mut());
            let dc = CreateCompatibleDC(screen);
            ReleaseDC(null_mut(), screen);
            let mut bmi: BITMAPINFO = std::mem::zeroed();
            bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = w;
            bmi.bmiHeader.biHeight = -h;
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            bmi.bmiHeader.biCompression = BI_RGB;
            let mut bits = null_mut();
            let bmp = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
            let old = SelectObject(dc, bmp);
            Surface { dc, bmp, old, bits: bits as *mut u8, w, h }
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            DeleteObject(self.bmp);
            DeleteDC(self.dc);
        }
    }
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
        if self.game_hidden {
            return; // drawn again when it comes back
        }
        let Some(mut data) = self.data.clone() else { return };
        data.procs_armed = self.procs_armed.clone();
        let view = self.view();
        let Some(frame) = Renderer::new(&mut self.fonts).render(&data, &view) else { return };
        self.boxes = frame.boxes;
        let (w, h) = (frame.pm.width() as i32, frame.pm.height() as i32);
        if self.surface.as_ref().is_none_or(|s| s.w != w || s.h != h) {
            self.surface = Some(Surface::new(w, h));
        }
        let s = self.surface.as_ref().unwrap();
        unsafe {
            let dst = std::slice::from_raw_parts_mut(s.bits, (w * h * 4) as usize);
            for (d, p) in dst.as_chunks_mut::<4>().0.iter_mut().zip(frame.pm.data().as_chunks::<4>().0) {
                *d = [p[2], p[1], p[0], p[3]];
            }
            let size = SIZE { cx: w, cy: h };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: (self.alpha() * 255.0).round() as u8,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let screen = GetDC(null_mut());
            UpdateLayeredWindow(self.hwnd, screen, null(), &size, s.dc, &src, 0, &blend, ULW_ALPHA);
            ReleaseDC(null_mut(), screen);
        }
        self.size_px = (w, h);
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
                unsafe { DestroyWindow(self.hwnd) };
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
            AppendMenuW(m, MF_SEPARATOR, 0, null());
            AppendMenuW(m, MF_STRING, CLOSE, wide("Закрыть").as_ptr());
            let mut pt = POINT::default();
            GetCursorPos(&mut pt);
            SetForegroundWindow(self.hwnd);
            let cmd =
                TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, 0, self.hwnd, null()) as usize;
            DestroyMenu(m);
            match cmd {
                COMPACT => self.cfg.compact = !self.cfg.compact,
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
            });
            0
        }
        WM_MOUSEMOVE => {
            let (x, y) = lparam_xy(lp);
            with_app(|a| {
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
                match a.press {
                    Some(Press::Body { x: px, y: py }) if (x - px).abs() + (y - py).abs() > DRAG_START => {
                        // the press becomes a move: Windows drags the window
                        a.press = None;
                        unsafe {
                            ReleaseCapture();
                            SendMessageW(hwnd, WM_SYSCOMMAND, (SC_MOVE | 2) as usize, 0);
                        }
                    }
                    Some(Press::Shape { .. }) => {
                        let mut pt = POINT { x, y };
                        unsafe { ClientToScreen(hwnd, &mut pt) };
                        a.change_shape(pt.x, pt.y);
                    }
                    _ => a.present(),
                }
            });
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
                a.shared.stop.store(true, Ordering::Relaxed);
                a.shared.hwnd.store(0, Ordering::Relaxed);
            });
            unsafe { PostQuitMessage(0) };
            0
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
            })
        });
        collect::start(shared.clone());
        with_app(|a| a.apply_pin());
        SetTimer(hwnd, TIMER_GAMES, 1000, None);
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
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

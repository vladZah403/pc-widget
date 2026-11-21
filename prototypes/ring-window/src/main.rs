//! PC Monitor 2.0, stage 0: a transparent window with a ring gauge of the CPU load.
//!
//! pcm-proto-ring [--measure N] [--clickthrough] [--scale X] [--png FILE]
//!   --measure N     close after N seconds and print CPU and memory use
//!   --clickthrough  clicks go to the windows below (like the FPS overlay)
//!   --scale X       ignore the monitor's DPI and draw at X (1.0 = 100%)
//!   --png FILE      draw one card into a PNG file and exit (no window)
//!
//! Drag it with the left mouse button, close it with the right one.

mod render;

use std::cell::RefCell;
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes, GetSystemTimes};
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const TIMER_SAMPLE: usize = 1;
const TIMER_ANIM: usize = 2;
const ANIM_MS: f32 = 250.0;
const ANIM_STEP_MS: u32 = 33;

struct Args {
    measure: Option<u64>,
    clickthrough: bool,
    scale: Option<f32>,
    png: Option<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args { measure: None, clickthrough: false, scale: None, png: None };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        match arg.as_str() {
            "--measure" => a.measure = Some(value("--measure")?.parse().map_err(|_| "bad --measure")?),
            "--scale" => a.scale = Some(value("--scale")?.parse().map_err(|_| "bad --scale")?),
            "--png" => a.png = Some(value("--png")?),
            "--clickthrough" => a.clickthrough = true,
            _ => return Err(format!("unknown argument: {arg}")),
        }
    }
    Ok(a)
}

fn ft(f: FILETIME) -> u64 {
    (f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64
}

/// Total CPU load from GetSystemTimes deltas.
struct CpuLoad {
    last: Option<(u64, u64)>, // (idle, kernel + user)
}

impl CpuLoad {
    fn sample(&mut self) -> Option<f32> {
        let (mut idle, mut kernel, mut user) = unsafe { std::mem::zeroed() };
        if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
            return None;
        }
        let now = (ft(idle), ft(kernel) + ft(user)); // kernel time includes idle
        let load = self.last.and_then(|(i0, t0)| {
            let (di, dt) = (now.0.saturating_sub(i0), now.1.saturating_sub(t0));
            (dt > 0).then(|| 1.0 - di as f32 / dt as f32)
        });
        self.last = Some(now);
        load
    }
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
            bmi.bmiHeader.biHeight = -h; // top-down
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

struct App {
    card: render::Card,
    fixed_scale: bool,
    cpu: CpuLoad,
    shown: f32,
    from: f32,
    target: f32,
    anim_start: Instant,
    surface: Option<Surface>,
    started: Instant,
    measure: Option<u64>,
    renders: u64,
    render_time: Duration,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

impl App {
    /// Draw the card and hand it to the window (premultiplied BGRA).
    fn present(&mut self, hwnd: HWND) {
        let t = Instant::now();
        let pm = self.card.draw(self.shown);
        let (w, h) = (pm.width() as i32, pm.height() as i32);
        if self.surface.as_ref().is_none_or(|s| s.w != w || s.h != h) {
            self.surface = Some(Surface::new(w, h));
        }
        let s = self.surface.as_ref().unwrap();
        unsafe {
            let dst = std::slice::from_raw_parts_mut(s.bits, (w * h * 4) as usize);
            for (d, p) in dst.as_chunks_mut::<4>().0.iter_mut().zip(pm.data().as_chunks::<4>().0) {
                *d = [p[2], p[1], p[0], p[3]];
            }
            let size = SIZE { cx: w, cy: h };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let screen = GetDC(null_mut());
            UpdateLayeredWindow(hwnd, screen, null(), &size, s.dc, &src, 0, &blend, ULW_ALPHA);
            ReleaseDC(null_mut(), screen);
        }
        self.renders += 1;
        self.render_time += t.elapsed();
    }

    fn report(&self) {
        unsafe {
            let (mut c, mut e, mut k, mut u) = std::mem::zeroed();
            GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u);
            let cpu_s = (ft(k) + ft(u)) as f64 / 1e7;
            let wall = self.started.elapsed().as_secs_f64();
            let mut m: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
            m.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
            K32GetProcessMemoryInfo(GetCurrentProcess(), &mut m as *mut _ as *mut _, m.cb);
            println!(
                "{wall:.0} s: cpu {cpu_s:.2} s = {:.2}% of one core; private {:.1} MB, working set {:.1} MB; \
                 {} frames, {:.2} ms per frame (draw + copy + UpdateLayeredWindow)",
                cpu_s / wall * 100.0,
                m.PrivateUsage as f64 / 1048576.0,
                m.WorkingSetSize as f64 / 1048576.0,
                self.renders,
                self.render_time.as_secs_f64() * 1000.0 / self.renders.max(1) as f64
            );
        }
    }
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.borrow_mut().as_mut().map(f))
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => HTCAPTION as LRESULT, // drag by any point
        WM_NCRBUTTONUP | WM_RBUTTONUP => {
            unsafe { DestroyWindow(hwnd) };
            0
        }
        WM_TIMER if wp == TIMER_SAMPLE => {
            let done = with_app(|app| {
                if let Some(load) = app.cpu.sample() {
                    // no frame at all when nothing would visibly change
                    let same_number = (load * 100.0).round() == (app.shown * 100.0).round();
                    if !(same_number && (load - app.shown).abs() < 0.01) {
                        app.from = app.shown;
                        app.target = load;
                        app.anim_start = Instant::now();
                        unsafe { SetTimer(hwnd, TIMER_ANIM, ANIM_STEP_MS, None) };
                    }
                }
                app.measure.is_some_and(|m| app.started.elapsed().as_secs() >= m)
            });
            if done == Some(true) {
                with_app(|app| app.report());
                unsafe { DestroyWindow(hwnd) };
            }
            0
        }
        WM_TIMER if wp == TIMER_ANIM => {
            with_app(|app| {
                let t = (app.anim_start.elapsed().as_secs_f32() * 1000.0 / ANIM_MS).min(1.0);
                let ease = 1.0 - (1.0 - t).powi(3); // ease-out
                app.shown = app.from + (app.target - app.from) * ease;
                app.present(hwnd);
                if t >= 1.0 {
                    unsafe { KillTimer(hwnd, TIMER_ANIM) };
                }
            });
            0
        }
        WM_DPICHANGED => {
            with_app(|app| {
                if !app.fixed_scale {
                    let scale = (wp >> 16 & 0xffff) as f32 / 96.0;
                    app.card.set_scale(scale);
                    let r = unsafe { &*(lp as *const RECT) };
                    let side = (render::SIZE * scale).round() as i32;
                    unsafe { SetWindowPos(hwnd, null_mut(), r.left, r.top, side, side, SWP_NOZORDER) };
                    app.present(hwnd);
                }
            });
            0
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

fn main() {
    let args = parse_args().unwrap_or_else(|e| {
        eprintln!("{e}\nusage: pcm-proto-ring [--measure N] [--clickthrough] [--scale X] [--png FILE]");
        std::process::exit(2);
    });
    let font = render::system_font().unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    if let Some(path) = &args.png {
        let pm = render::Card::new(font, "CPU", args.scale.unwrap_or(1.0)).draw(0.37);
        pm.save_png(path).unwrap_or_else(|e| eprintln!("{e}"));
        return;
    }

    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let hinst = GetModuleHandleW(null());
        let class = wide("PCMonitor2RingProto");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: class.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);
        let mut ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE;
        if args.clickthrough {
            ex |= WS_EX_TRANSPARENT;
        }
        let mut work: RECT = std::mem::zeroed();
        SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut work as *mut _ as *mut _, 0);
        let title = wide("PC Monitor 2.0 ring prototype");
        let hwnd = CreateWindowExW(
            ex,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            work.right - 240,
            work.top + (work.bottom - work.top) / 2 - 100,
            200,
            200,
            null_mut(),
            null_mut(),
            hinst,
            null(),
        );
        if hwnd.is_null() {
            eprintln!("CreateWindowEx failed: {}", GetLastError());
            std::process::exit(1);
        }
        let fixed = args.scale.is_some();
        let scale = args.scale.unwrap_or(GetDpiForWindow(hwnd) as f32 / 96.0);
        let mut cpu = CpuLoad { last: None };
        cpu.sample();
        APP.with(|a| {
            *a.borrow_mut() = Some(App {
                card: render::Card::new(font, "CPU", scale),
                fixed_scale: fixed,
                cpu,
                shown: 0.0,
                from: 0.0,
                target: 0.0,
                anim_start: Instant::now(),
                surface: None,
                started: Instant::now(),
                measure: args.measure,
                renders: 0,
                render_time: Duration::ZERO,
            })
        });
        let side = (render::SIZE * scale).round() as i32;
        SetWindowPos(hwnd, null_mut(), 0, 0, side, side, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
        with_app(|app| app.present(hwnd));
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetTimer(hwnd, TIMER_SAMPLE, 1000, None);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

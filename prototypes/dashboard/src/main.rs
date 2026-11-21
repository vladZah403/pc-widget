//! PC Monitor 2.0, stage 0: the dashboard window in egui (eframe + glow).
//!
//! pcm-proto-dashboard [--measure N] [--reopen] [--screenshot FILE]
//!   --measure N  close the window after N seconds, print CPU and memory use
//!                while it was open and after it closed
//!   --reopen     open the window a second time after it closes (the 2.0 app
//!                lives in the tray and creates the dashboard on demand)
//!   --screenshot FILE  save the window's own pixels as PNG after 8 s and close
//!
//! The CPU card is the tiny-skia card of the ring prototype, shown as a
//! texture; the other two are plain egui, to compare the looks.

#[path = "../../ring-window/src/render.rs"]
mod render;

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// When main() started, for the time to the first frame.
static STARTED: OnceLock<Instant> = OnceLock::new();

use eframe::egui;
use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, RichText, Stroke, Vec2};
use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes, GetSystemTimes};

// 1.8 palette
const BG: Color32 = Color32::from_rgb(20, 23, 30);
const CARD: Color32 = Color32::from_rgb(28, 33, 43);
const BORDER: Color32 = Color32::from_rgb(44, 51, 64);
const TRACK: Color32 = Color32::from_rgb(40, 46, 58);
const TEXT: Color32 = Color32::from_rgb(232, 234, 239);
const DIM: Color32 = Color32::from_rgb(139, 147, 163);

const TABS: [&str; 5] = ["Обзор", "Процессы", "История", "Игровые сессии", "Настройки"];

fn ft(f: FILETIME) -> u64 {
    (f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64
}

struct CpuLoad(Option<(u64, u64)>);

impl CpuLoad {
    fn sample(&mut self) -> Option<f32> {
        let (mut idle, mut kernel, mut user) = unsafe { std::mem::zeroed() };
        if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
            return None;
        }
        let now = (ft(idle), ft(kernel) + ft(user));
        let load = self.0.and_then(|(i0, t0)| {
            let (di, dt) = (now.0.saturating_sub(i0), now.1.saturating_sub(t0));
            (dt > 0).then(|| 1.0 - di as f32 / dt as f32)
        });
        self.0 = Some(now);
        load
    }
}

/// (used, total) bytes of RAM.
fn memory() -> (u64, u64) {
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
    unsafe { GlobalMemoryStatusEx(&mut m) };
    (m.ullTotalPhys - m.ullAvailPhys, m.ullTotalPhys)
}

/// (CPU seconds so far, private MB, working set MB) of this process.
fn usage() -> (f64, f64, f64) {
    unsafe {
        let (mut c, mut e, mut k, mut u) = std::mem::zeroed();
        GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u);
        let mut m: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        m.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        K32GetProcessMemoryInfo(GetCurrentProcess(), &mut m as *mut _ as *mut _, m.cb);
        ((ft(k) + ft(u)) as f64 / 1e7, m.PrivateUsage as f64 / 1048576.0, m.WorkingSetSize as f64 / 1048576.0)
    }
}

/// Segoe UI from the Windows fonts folder (egui's own fonts are not bundled).
fn fonts() -> FontDefinitions {
    let mut defs = FontDefinitions::empty();
    let dir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    for (name, file) in [("ui", "segoeui.ttf"), ("ui-semibold", "seguisb.ttf"), ("bahn", "bahnschrift.ttf")] {
        if let Ok(data) = std::fs::read(format!(r"{dir}\Fonts\{file}")) {
            defs.font_data.insert(name.into(), Arc::new(FontData::from_owned(data)));
        }
    }
    let have = |n: &str| defs.font_data.contains_key(n).then(|| n.to_string());
    let ui: Vec<String> = [have("ui"), have("bahn")].into_iter().flatten().collect();
    defs.families.insert(FontFamily::Proportional, ui.clone());
    defs.families.insert(FontFamily::Monospace, ui.clone());
    let heading: Vec<String> = [have("ui-semibold"), have("ui")].into_iter().flatten().collect();
    defs.families.insert(FontFamily::Name("heading".into()), heading);
    defs.families
        .insert(FontFamily::Name("numbers".into()), [have("bahn")].into_iter().flatten().chain(ui).collect());
    defs
}

struct Dashboard {
    tab: usize,
    cpu: CpuLoad,
    load: f32,
    history: VecDeque<f32>,
    card: render::Card,
    texture: Option<egui::TextureHandle>,
    texture_scale: f32,
    last_sample: Instant,
    opened: Instant,
    measure: Option<u64>,
    cpu_at_open: f64,
    frames: u64,
    report: Option<String>,
    screenshot: Option<String>,
    shot_requested: bool,
}

impl Dashboard {
    fn new(cc: &eframe::CreationContext, measure: Option<u64>, screenshot: Option<String>) -> Dashboard {
        cc.egui_ctx.set_fonts(fonts());
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = BG;
        visuals.window_fill = BG;
        visuals.override_text_color = Some(TEXT);
        visuals.selection.bg_fill = BORDER; // the active tab: a quiet chip, not egui blue
        visuals.selection.stroke = Stroke::new(1.0, TEXT);
        visuals.widgets.hovered.weak_bg_fill = CARD;
        // always dark, whatever the Windows theme is (egui follows it by default)
        cc.egui_ctx.set_theme(egui::ThemePreference::Dark);
        cc.egui_ctx.set_visuals_of(egui::Theme::Dark, visuals);
        let mut cpu = CpuLoad(None);
        cpu.sample();
        let font = render::system_font().expect("Bahnschrift or Segoe UI");
        Dashboard {
            tab: 0,
            cpu,
            load: 0.0,
            history: VecDeque::new(),
            card: render::Card::new(font, "CPU", 1.0),
            texture: None,
            texture_scale: 0.0,
            last_sample: Instant::now(),
            opened: Instant::now(),
            measure,
            cpu_at_open: usage().0,
            frames: 0,
            report: None,
            screenshot,
            shot_requested: false,
        }
    }

    fn sample(&mut self) {
        if let Some(l) = self.cpu.sample() {
            self.load = l;
            self.history.push_back(l);
            if self.history.len() > 60 {
                self.history.pop_front();
            }
        }
        self.texture_scale = 0.0; // redraw the card texture
    }

    /// A card of `w` x 200 points with its content laid out top to bottom.
    fn card(ui: &mut egui::Ui, w: f32, add: impl FnOnce(&mut egui::Ui)) {
        let inner = Vec2::new(w - 32.0, 200.0 - 32.0);
        egui::Frame::new()
            .fill(CARD)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(CornerRadius::same(18))
            .inner_margin(16)
            .show(ui, |ui| {
                ui.allocate_ui_with_layout(inner, egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.set_min_size(inner);
                    add(ui)
                });
            });
    }

    fn cpu_card(&mut self, ui: &mut egui::Ui) {
        let ppp = ui.ctx().pixels_per_point();
        if self.texture.is_none() || self.texture_scale != ppp {
            // the card in physical pixels, so it stays sharp at any DPI
            self.card.set_scale(ppp);
            let pm = self.card.draw(self.load);
            let img = egui::ColorImage::from_rgba_premultiplied(
                [pm.width() as usize, pm.height() as usize],
                pm.data(),
            );
            match &mut self.texture {
                Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                None => {
                    self.texture = Some(ui.ctx().load_texture("cpu-card", img, egui::TextureOptions::LINEAR))
                }
            }
            self.texture_scale = ppp;
        }
        if let Some(t) = &self.texture {
            ui.add(egui::Image::new((t.id(), Vec2::splat(render::SIZE))));
        }
    }

    fn ram_card(&self, ui: &mut egui::Ui) {
        let (used, total) = memory();
        let gb = |b: u64| b as f64 / (1u64 << 30) as f64;
        Self::card(ui, 200.0, |ui| {
            ui.label(RichText::new("Память").color(DIM));
            ui.add_space(8.0);
            let pct = used as f32 / total.max(1) as f32;
            ui.label(
                RichText::new(format!("{:.0}%", pct * 100.0))
                    .size(40.0)
                    .color(TEXT)
                    .family(FontFamily::Name("numbers".into())),
            );
            ui.label(RichText::new(format!("{:.1} из {:.1} ГБ", gb(used), gb(total))).color(DIM));
            ui.add_space(10.0);
            let (rect, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 10.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 5.0, TRACK);
            let mut fill = rect;
            fill.set_width(rect.width() * pct);
            let c = render::heat(pct);
            ui.painter().rect_filled(fill, 5.0, Color32::from_rgb(c.0, c.1, c.2));
        });
    }

    fn history_card(&self, ui: &mut egui::Ui) {
        Self::card(ui, 260.0, |ui| {
            ui.label(RichText::new("CPU за минуту").color(DIM));
            ui.add_space(8.0);
            let (rect, _) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
            let p = ui.painter();
            p.rect_filled(rect, 8.0, TRACK.gamma_multiply(0.6));
            if self.history.len() >= 2 {
                let n = 60.0;
                let pts: Vec<egui::Pos2> = self
                    .history
                    .iter()
                    .enumerate()
                    .map(|(i, &v)| {
                        let x = rect.right() - (self.history.len() - 1 - i) as f32 / (n - 1.0) * rect.width();
                        egui::pos2(x, rect.bottom() - v * rect.height())
                    })
                    .collect();
                let c = render::heat(self.load);
                p.add(egui::Shape::line(pts, Stroke::new(2.0, Color32::from_rgb(c.0, c.1, c.2))));
            }
        });
    }
}

impl eframe::App for Dashboard {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.frames += 1;
        if self.frames == 1 {
            let ms = STARTED.get().map_or(0.0, |t| t.elapsed().as_secs_f64() * 1000.0);
            println!("first frame {ms:.0} ms after start");
        }
        if let Some(path) = self.screenshot.clone()
            && !self.shot_requested
            && self.opened.elapsed() >= Duration::from_secs(8)
        {
            self.shot_requested = true;
            let ctx = ui.ctx().clone();
            ui.ctx().request_screenshot(move |img| {
                let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                let size = tiny_skia::IntSize::from_wh(img.size[0] as u32, img.size[1] as u32);
                match size.and_then(|size| tiny_skia::Pixmap::from_vec(bytes, size)) {
                    Some(pm) => pm.save_png(&path).unwrap_or_else(|e| eprintln!("screenshot: {e}")),
                    None => eprintln!("screenshot: bad size"),
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                ctx.request_repaint();
            });
            ui.ctx().request_repaint();
        }
        if self.last_sample.elapsed() >= Duration::from_secs(1) {
            self.last_sample = Instant::now();
            self.sample();
        }
        if let Some(m) = self.measure
            && self.opened.elapsed() >= Duration::from_secs(m)
            && self.report.is_none()
        {
            let (cpu, private, ws) = usage();
            let wall = self.opened.elapsed().as_secs_f64();
            self.report = Some(format!(
                "window open {wall:.0} s: cpu {:.2}% of one core, private {private:.1} MB, working set {ws:.1} MB, \
                 {} frames",
                (cpu - self.cpu_at_open) / wall * 100.0,
                self.frames
            ));
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }

        ui.painter().rect_filled(ui.max_rect(), 0.0, BG); // the whole window, not just the content
        egui::Frame::new().fill(BG).inner_margin(20).show(ui, |ui| {
            ui.horizontal(|ui| {
                for (i, name) in TABS.iter().enumerate() {
                    let selected = self.tab == i;
                    let text = RichText::new(*name).size(15.0).color(if selected { TEXT } else { DIM });
                    if ui.add(egui::Button::selectable(selected, text)).clicked() {
                        self.tab = i;
                    }
                    ui.add_space(4.0);
                }
            });
            ui.add_space(16.0);
            match self.tab {
                0 => {
                    ui.horizontal_top(|ui| {
                        self.cpu_card(ui);
                        ui.add_space(12.0);
                        self.ram_card(ui);
                        ui.add_space(12.0);
                        self.history_card(ui);
                    });
                    ui.add_space(16.0);
                    ui.label(
                        RichText::new("Слева карточка tiny-skia (текстура), две другие нарисованы egui.")
                            .color(DIM),
                    );
                }
                _ => {
                    ui.label(RichText::new(format!("«{}» появится на этапе 5.", TABS[self.tab])).color(DIM));
                }
            }
        });
        // nothing moves between samples: one frame a second unless the mouse does something
        ui.ctx().request_repaint_after(Duration::from_millis(1000));
    }
}

fn parse_args() -> Result<(Option<u64>, bool, Option<String>), String> {
    let (mut measure, mut reopen, mut screenshot) = (None, false, None);
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--measure" => {
                measure = Some(it.next().and_then(|v| v.parse().ok()).ok_or("--measure needs seconds")?)
            }
            "--reopen" => reopen = true,
            "--screenshot" => screenshot = Some(it.next().ok_or("--screenshot needs a file")?),
            _ => return Err(format!("unknown argument: {a}")),
        }
    }
    Ok((measure, reopen, screenshot))
}

fn main() {
    STARTED.get_or_init(Instant::now);
    let (measure, reopen, screenshot) = parse_args().unwrap_or_else(|e| {
        eprintln!("{e}\nusage: pcm-proto-dashboard [--measure N] [--reopen] [--screenshot FILE]");
        std::process::exit(2);
    });
    let (_, private, ws) = usage();
    println!("before the window: private {private:.1} MB, working set {ws:.1} MB");
    for round in 1..=(if reopen { 2 } else { 1 }) {
        let report = Arc::new(std::sync::Mutex::new(None::<String>));
        let out = report.clone();
        let shot = screenshot.clone();
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([760.0, 330.0])
                .with_min_inner_size([560.0, 300.0])
                .with_title("PC Monitor 2.0 — прототип дашборда"),
            // `--features wgpu` compares Direct3D 12 (wgpu) with OpenGL (glow)
            #[cfg(feature = "wgpu")]
            renderer: eframe::Renderer::Wgpu,
            #[cfg(not(feature = "wgpu"))]
            renderer: eframe::Renderer::Glow,
            ..Default::default()
        };
        let result = eframe::run_native(
            "PC Monitor 2.0 dashboard prototype",
            options,
            Box::new(move |cc| {
                let app = Dashboard::new(cc, measure, shot);
                Ok(Box::new(ReportOnDrop { app, out }))
            }),
        );
        if let Err(e) = result {
            eprintln!("eframe: {e}");
            std::process::exit(1);
        }
        if let Some(r) = report.lock().unwrap().take() {
            println!("round {round}: {r}");
        }
        let (_, private, ws) = usage();
        println!("round {round}: after the window closed: private {private:.1} MB, working set {ws:.1} MB");
    }
}

/// Hands the measurement out of the app when eframe drops it.
struct ReportOnDrop {
    app: Dashboard,
    out: Arc<std::sync::Mutex<Option<String>>>,
}

impl eframe::App for ReportOnDrop {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.app.ui(ui, frame);
    }
}

impl Drop for ReportOnDrop {
    fn drop(&mut self) {
        *self.out.lock().unwrap() = self.app.report.take();
    }
}

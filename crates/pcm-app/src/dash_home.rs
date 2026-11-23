//! "Главная" of the PC Monitor window (beta 2, Vlad's concept "Главный
//! экран"): four summary cards with the last minute under each, the load
//! chart (1 min / 5 min from memory, 1 h / 24 h from the history database),
//! temperatures and fans, the network, the busiest programs and the game
//! session (the running one, else the last one recorded).
//!
//! Everything is painted with egui shapes, so it stays sharp at any scale
//! and the chart can answer the mouse.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{
    Align2, Color32, CursorIcon, FontFamily, FontId, Mesh, Painter, Pos2, Rect, Sense, Shape, Stroke,
    StrokeKind, Ui, pos2, vec2,
};
use pcm_core::history::{Column, FILE, History, Point, buckets};
use pcm_core::session::{EXTENSION, Session, SessionInfo};
use pcm_render::format::{dec, fmt_mem, fmt_speed, fmt_uptime, tr, trf};
use pcm_render::theme::Scale;
use pcm_render::widget::Data;

use crate::session_rec::LiveGame;

/// The window's colours; `p()` gives the dark or the light set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pal {
    /// The page behind the cards.
    pub bg: Color32,
    /// The top bar, the pages list and the status line.
    pub chrome: Color32,
    pub card: Color32,
    /// A selected page, a pressed segment, an icon's box.
    pub card_hi: Color32,
    pub border: Color32,
    pub text: Color32,
    pub dim: Color32,
    pub faint: Color32,
    pub grid: Color32,
    pub accent: Color32,
    pub good: Color32,
    pub warn: Color32,
    pub hot: Color32,
    /// The load chart's two lines: the CPU solid, the GPU dashed.
    pub cpu_line: Color32,
    pub gpu_line: Color32,
}

pub const DARK: Pal = Pal {
    bg: Color32::from_rgb(20, 23, 30),
    chrome: Color32::from_rgb(17, 19, 25),
    card: Color32::from_rgb(26, 30, 39),
    card_hi: Color32::from_rgb(36, 42, 54),
    border: Color32::from_rgb(40, 46, 58),
    text: Color32::from_rgb(232, 234, 239),
    dim: Color32::from_rgb(139, 147, 163),
    faint: Color32::from_rgb(88, 96, 112),
    grid: Color32::from_rgb(34, 39, 50),
    accent: Color32::from_rgb(92, 156, 246),
    good: Color32::from_rgb(72, 199, 142),
    warn: Color32::from_rgb(240, 177, 64),
    hot: Color32::from_rgb(232, 68, 90),
    cpu_line: Color32::from_rgb(92, 156, 246),
    gpu_line: Color32::from_rgb(178, 148, 250),
};

pub const LIGHT: Pal = Pal {
    bg: Color32::from_rgb(240, 242, 246),
    chrome: Color32::from_rgb(250, 251, 253),
    card: Color32::from_rgb(255, 255, 255),
    card_hi: Color32::from_rgb(231, 235, 242),
    border: Color32::from_rgb(220, 224, 232),
    text: Color32::from_rgb(24, 28, 36),
    dim: Color32::from_rgb(94, 102, 118),
    faint: Color32::from_rgb(146, 153, 167),
    grid: Color32::from_rgb(232, 235, 240),
    accent: Color32::from_rgb(37, 110, 214),
    good: Color32::from_rgb(22, 150, 96),
    warn: Color32::from_rgb(190, 120, 16),
    hot: Color32::from_rgb(214, 52, 74),
    cpu_line: Color32::from_rgb(37, 110, 214),
    gpu_line: Color32::from_rgb(132, 88, 222),
};

/// The colours in use: light when "Светлая тема" is on.
pub fn pal() -> &'static Pal {
    if pcm_render::theme::light() { &LIGHT } else { &DARK }
}

const GAP: f32 = 16.0;
/// Heights of the page's rows ("Главная" and its line, the four cards, the bottom three).
const HEADER_H: f32 = 50.0;
const CARDS_H: f32 = 170.0;
const BOTTOM_H: f32 = 228.0;
const R: f32 = 12.0;
/// Seconds of load kept in memory for "1 мин" and "5 мин".
const RING: usize = 300;
const RANGES: [(&str, i64); 4] = [("1 мин", 60), ("5 мин", 300), ("1 ч", 3600), ("24 ч", 86400)];
const DB_RELOAD: Duration = Duration::from_secs(10);
const SESSIONS_RELOAD: Duration = Duration::from_secs(15);

/// Icons from Segoe MDL2 Assets (the "icons" font family).
pub mod icon {
    pub const LOGO: &str = "\u{E9D9}";
    pub const HOME: &str = "\u{E80F}";
    pub const HISTORY: &str = "\u{E81C}";
    pub const GAME: &str = "\u{E7FC}";
    pub const LIST: &str = "\u{E8FD}";
    pub const DEVICES: &str = "\u{E772}";
    pub const SETTINGS: &str = "\u{E713}";
    pub const CPU: &str = "\u{E950}";
    pub const GPU: &str = "\u{E7F4}";
    pub const RAM: &str = "\u{E964}";
    pub const DISK: &str = "\u{EDA2}";
    pub const CHEVRON: &str = "\u{E70D}";
    pub const SUN: &str = "\u{E706}";
    pub const MOON: &str = "\u{E708}";
    pub const NET: &str = "\u{E839}";
    pub const POWER: &str = "\u{E945}";
}

pub fn text_font(size: f32) -> FontId {
    FontId::proportional(size)
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

pub fn icon_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("icons".into()))
}

pub fn rgb(c: (u8, u8, u8)) -> Color32 {
    Color32::from_rgb(c.0, c.1, c.2)
}

pub(crate) fn alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

/// A card: the rounded fill and its border.
pub fn panel(p: &Painter, r: Rect) {
    p.rect_filled(r, R, pal().card);
    p.rect_stroke(r, R, Stroke::new(1.0, pal().border), StrokeKind::Inside);
}

/// Pieces of text in a row from `pos` (left, baseline-ish top); returns the end x.
pub(crate) fn run(p: &Painter, pos: Pos2, parts: &[(String, FontId, Color32)]) -> f32 {
    let mut x = pos.x;
    // align the bottoms: the largest piece decides the line
    let tallest = parts.iter().map(|(_, f, _)| f.size).fold(0.0, f32::max);
    for (t, f, c) in parts {
        let g = p.layout_no_wrap(t.clone(), f.clone(), *c);
        let y = pos.y + (tallest - f.size) * 0.78;
        let w = g.size().x;
        p.galley(pos2(x, y), g, *c);
        x += w;
    }
    x
}

pub(crate) fn text_w(p: &Painter, t: &str, f: FontId) -> f32 {
    p.layout_no_wrap(t.to_string(), f, pal().text).size().x
}

/// Text cut with "…" to `max` px.
pub(crate) fn fit(p: &Painter, t: &str, f: &FontId, max: f32) -> String {
    if text_w(p, t, f.clone()) <= max {
        return t.to_string();
    }
    let mut chars: Vec<char> = t.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let s: String = chars.iter().collect::<String>() + "…";
        if text_w(p, &s, f.clone()) <= max {
            return s;
        }
    }
    "…".into()
}

/// The area under a line: the colour fading down to nothing, and the line.
fn area(p: &Painter, r: Rect, values: &[f32], max: f32, color: Color32, top_alpha: u8, width: f32) {
    if values.len() < 2 || max <= 0.0 {
        return;
    }
    let n = values.len();
    let pts: Vec<Pos2> = values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let x = r.left() + r.width() * i as f32 / (n - 1) as f32;
            pos2(x, r.bottom() - r.height() * (v / max).clamp(0.0, 1.0))
        })
        .collect();
    let mut mesh = Mesh::default();
    for (i, pt) in pts.iter().enumerate() {
        mesh.colored_vertex(*pt, alpha(color, top_alpha));
        mesh.colored_vertex(pos2(pt.x, r.bottom()), Color32::TRANSPARENT);
        if i > 0 {
            let b = (i * 2) as u32;
            mesh.add_triangle(b - 2, b - 1, b);
            mesh.add_triangle(b - 1, b + 1, b);
        }
    }
    p.add(Shape::mesh(mesh));
    p.add(Shape::line(pts, Stroke::new(width, color)));
}

/// A segmented switch ("1 мин | 5 мин | 1 ч | 24 ч"); returns the clicked index.
pub(crate) fn segments(
    ui: &mut Ui,
    right_top: Pos2,
    items: &[String],
    selected: usize,
    id: &str,
) -> Option<usize> {
    let p = ui.painter().clone();
    let f = text_font(12.5);
    let widths: Vec<f32> = items.iter().map(|t| text_w(&p, t, f.clone()) + 22.0).collect();
    let total: f32 = widths.iter().sum::<f32>() + 6.0;
    let outer = Rect::from_min_size(pos2(right_top.x - total, right_top.y), vec2(total, 30.0));
    p.rect_filled(outer, 8.0, pal().bg);
    p.rect_stroke(outer, 8.0, Stroke::new(1.0, pal().border), StrokeKind::Inside);
    let mut x = outer.left() + 3.0;
    let mut clicked = None;
    for (i, (t, w)) in items.iter().zip(&widths).enumerate() {
        let r = Rect::from_min_size(pos2(x, outer.top() + 3.0), vec2(*w, 24.0));
        let resp = ui.interact(r, ui.scope_id().with((id, i)), Sense::click());
        if i == selected {
            p.rect_filled(r, 6.0, pal().card_hi);
        } else if resp.hovered() {
            p.rect_filled(r, 6.0, alpha(pal().card_hi, 120));
        }
        let col = if i == selected { pal().text } else { pal().dim };
        let fnt = if i == selected { bold(12.5) } else { f.clone() };
        p.text(r.center(), Align2::CENTER_CENTER, t, fnt, col);
        if resp.clicked() {
            clicked = Some(i);
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        x += w;
    }
    clicked
}

/// A link-like piece of text; true when clicked.
pub(crate) fn link(ui: &mut Ui, pos: Pos2, anchor: Align2, t: &str, id: &str) -> bool {
    let p = ui.painter().clone();
    let r = p.text(pos, anchor, t, text_font(13.0), pal().accent);
    let resp = ui.interact(r.expand(3.0), ui.scope_id().with(id), Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        p.hline(r.x_range(), r.bottom(), Stroke::new(1.0, alpha(pal().accent, 160)));
    }
    resp.clicked()
}

/// What a click on the page asks the window to do.
pub enum HomeAction {
    /// Show another tab (dashboard::TAB_* index).
    Tab(usize),
    /// Show this session's report in the games tab.
    Session(PathBuf),
}

/// What the page needs from the feed.
pub struct HomeInput<'a> {
    pub data: &'a Data,
    pub sc: &'a Scale,
    pub game: Option<&'a LiveGame>,
    /// Bumped with every new feed.
    pub seq: u64,
}

#[derive(Default)]
pub struct HomeTab {
    range: usize,
    fahrenheit: bool,
    /// CPU, GPU, RAM load and disk bytes/s each second, oldest first.
    ring: VecDeque<[f32; 4]>,
    seen_seq: u64,
    db: Option<History>,
    points: Vec<Point>,
    loaded: Option<(Instant, i64)>,
    last_session: Option<(PathBuf, SessionInfo)>,
    sessions_read: Option<Instant>,
}

impl HomeTab {
    /// Take this second's readings into the ring (once per feed).
    fn remember(&mut self, d: &Data, seq: u64) {
        if seq == self.seen_seq {
            return;
        }
        self.seen_seq = seq;
        let io = (d.io_read + d.io_write) as f32;
        if self.ring.is_empty() {
            // the widget's last minute, so the chart isn't empty at first
            let n = d.cpu_hist.len().min(d.gpu_hist.len().max(d.cpu_hist.len()));
            for i in 0..n.saturating_sub(1) {
                let gpu = d.gpu_hist.get(i + d.gpu_hist.len().saturating_sub(n)).copied().unwrap_or(0.0);
                self.ring.push_back([d.cpu_hist[i], gpu, d.ram_pct, io]);
            }
        }
        self.ring.push_back([d.cpu, d.gpu_load.unwrap_or(0.0), d.ram_pct, io]);
        while self.ring.len() > RING {
            self.ring.pop_front();
        }
    }

    fn column(&self, k: usize, last: usize) -> Vec<f32> {
        let skip = self.ring.len().saturating_sub(last);
        self.ring.iter().skip(skip).map(|v| v[k]).collect()
    }

    /// The history database's points for the long ranges.
    fn load_db(&mut self, span: i64) {
        let fresh = self.loaded.is_some_and(|(t, s)| s == span && t.elapsed() < DB_RELOAD);
        if fresh {
            return;
        }
        if self.db.is_none() {
            self.db = History::open_read(&crate::widget_win::data_dir().join(FILE)).ok();
        }
        let now = pcm_win::system::unix_now();
        self.points = self.db.as_ref().and_then(|db| db.range(now - span, now).ok()).unwrap_or_default();
        self.loaded = Some((Instant::now(), span));
    }

    fn read_sessions(&mut self) {
        if self.sessions_read.is_some_and(|t| t.elapsed() < SESSIONS_RELOAD) {
            return;
        }
        self.sessions_read = Some(Instant::now());
        let dir = crate::dash_sessions::dir();
        let newest = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.extension().is_some_and(|e| e == EXTENSION))
                    .max()
            })
            .ok()
            .flatten();
        match newest {
            Some(p) if self.last_session.as_ref().is_some_and(|(q, _)| *q == p) => {}
            Some(p) => self.last_session = Session::load_info(&p).ok().map(|i| (p, i)),
            None => self.last_session = None,
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, input: Option<HomeInput>) -> Option<HomeAction> {
        let Some(input) = input else {
            ui.label(egui::RichText::new(tr("Жду данные от виджета…")).color(pal().dim));
            return None;
        };
        self.remember(input.data, input.seq);
        if input.game.is_none() {
            self.read_sessions();
        }
        let mut action = None;
        let view_h = ui.available_height();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let w = ui.available_width();
            self.header(ui, w, input.sc);
            ui.add_space(14.0);
            self.summary_row(ui, w, &input);
            ui.add_space(GAP);
            let wide = w >= 1000.0;
            if wide {
                // the chart takes what the window has left: no scrolling on a laptop
                let rest = view_h - HEADER_H - 14.0 - CARDS_H - 2.0 * GAP - BOTTOM_H - 16.0;
                let (r, _) = ui.allocate_exact_size(vec2(w, rest.clamp(280.0, 420.0)), Sense::hover());
                let temps_w = 380.0;
                let chart = Rect::from_min_size(r.min, vec2(w - temps_w - GAP, r.height()));
                let temps =
                    Rect::from_min_size(pos2(chart.right() + GAP, r.top()), vec2(temps_w, r.height()));
                self.load_chart(ui, chart, &input);
                self.temps(ui, temps, input.data, input.sc);
            } else {
                let (chart, _) = ui.allocate_exact_size(vec2(w, 300.0), Sense::hover());
                self.load_chart(ui, chart, &input);
                ui.add_space(GAP);
                let (temps, _) = ui.allocate_exact_size(vec2(w, 300.0), Sense::hover());
                self.temps(ui, temps, input.data, input.sc);
            }
            ui.add_space(GAP);
            let h = BOTTOM_H;
            let cells: Vec<Rect> = if wide {
                let (r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
                let cw = (w - 2.0 * GAP) / 3.0;
                (0..3)
                    .map(|i| {
                        Rect::from_min_size(pos2(r.left() + i as f32 * (cw + GAP), r.top()), vec2(cw, h))
                    })
                    .collect()
            } else {
                (0..3)
                    .map(|i| {
                        if i > 0 {
                            ui.add_space(GAP);
                        }
                        ui.allocate_exact_size(vec2(w, h), Sense::hover()).0
                    })
                    .collect()
            };
            self.network(ui, cells[0], input.data, input.sc);
            if let Some(a) = self.programs(ui, cells[1], input.data, input.sc) {
                action = Some(a);
            }
            if let Some(a) = self.game(ui, cells[2], input.game, input.sc) {
                action = Some(a);
            }
            ui.add_space(4.0);
        });
        action
    }

    /// "Главная", what the colours mean, the scale 0..100.
    fn header(&self, ui: &mut Ui, w: f32, sc: &Scale) {
        let (r, _) = ui.allocate_exact_size(vec2(w, HEADER_H), Sense::hover());
        let p = ui.painter();
        p.text(r.left_top(), Align2::LEFT_TOP, tr("Главная"), bold(22.0), pal().text);
        p.text(
            pos2(r.left(), r.top() + 32.0),
            Align2::LEFT_TOP,
            tr("Цвет значения показывает нагрузку: от спокойного до горячего"),
            text_font(13.0),
            pal().dim,
        );
        // the theme's scale as a bar
        let bar = Rect::from_min_size(pos2(r.right() - 190.0, r.top() + 12.0), vec2(160.0, 6.0));
        let mut mesh = Mesh::default();
        let steps = 24;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let c = rgb(sc.at(t));
            let x = bar.left() + bar.width() * t;
            mesh.colored_vertex(pos2(x, bar.top()), c);
            mesh.colored_vertex(pos2(x, bar.bottom()), c);
            if i > 0 {
                let b = (i * 2) as u32;
                mesh.add_triangle(b - 2, b - 1, b);
                mesh.add_triangle(b - 1, b + 1, b);
            }
        }
        p.add(Shape::mesh(mesh));
        p.text(pos2(bar.left() - 8.0, bar.center().y), Align2::RIGHT_CENTER, "0", text_font(12.0), pal().dim);
        p.text(
            pos2(bar.right() + 8.0, bar.center().y),
            Align2::LEFT_CENTER,
            "100",
            text_font(12.0),
            pal().dim,
        );
    }

    fn summary_row(&self, ui: &mut Ui, w: f32, input: &HomeInput) {
        let d = input.data;
        let sc = input.sc;
        let cols = if w >= 880.0 { 4 } else { 2 };
        let cw = (w - GAP * (cols - 1) as f32) / cols as f32;
        let h = CARDS_H;
        let mut cards: Vec<Card> = Vec::new();

        // the processor
        let mut sub = d.cpu_name.clone();
        if let Some(c) = d.cpu_cores {
            // Russian and Ukrainian plurals: 1 ядро, 4 ядра, 6 ядер
            let form = match (c % 10, c % 100) {
                (1, x) if x != 11 => "{n} ядро",
                (2..=4, x) if !(12..=14).contains(&x) => "{n} ядра",
                _ => "{n} ядер",
            };
            sub = format!("{sub} · {}", trf(form, &[("n", c.to_string())]));
        }
        let mut line = Vec::new();
        if let Some(ghz) = d.cpu_clock_ghz {
            line.push((trf("{v} ГГц", &[("v", dec(format!("{ghz:.2}")))]), pal().dim));
        }
        if let Some(t) = d.temp_cpu {
            let approx = if d.temp_cpu_approx { "≈" } else { "" };
            line.push((format!("{approx}{t:.0}°C"), rgb(sc.heat(t))));
        }
        // held back right now: from heat red, by the power plan yellow
        if let Some(th) = pcm_core::throttle::check(d.cpu_limit, d.temp_cpu, Some(d.cpu)) {
            let col = if th.cause == pcm_core::throttle::Cause::Heat { pal().hot } else { pal().warn };
            let text = match th.limit {
                Some(l) => trf("урезан до {l}%", &[("l", format!("{l:.0}"))]),
                None => tr("сбрасывает частоту"),
            };
            line.push((text, col));
        }
        cards.push(Card {
            icon: icon::CPU,
            title: tr("Процессор"),
            sub,
            value: format!("{:.0}", d.cpu),
            unit: "%".into(),
            color: rgb(sc.at(d.cpu / 100.0)),
            line,
            spark: self.column(0, 60),
            spark_max: 100.0,
        });

        // the graphics card
        let mut sub = d.gpu_name.clone().unwrap_or_else(|| tr("Видеокарта"));
        if let Some(t) = d.vram_total_mb.filter(|t| *t >= 1024.0) {
            sub = format!("{sub} · {}", trf("{v} ГБ", &[("v", format!("{:.0}", t / 1024.0))]));
        }
        let mut line = Vec::new();
        if let Some(u) = d.vram_used_mb {
            line.push((
                format!("VRAM {}", trf("{v} ГБ", &[("v", dec(format!("{:.1}", u / 1024.0)))])),
                pal().dim,
            ));
        }
        let (gt, approx) = match (d.temp_gpu, d.gpu_shared) {
            (Some(t), _) => (Some(t), false),
            (None, true) => (d.temp_cpu, true),
            _ => (None, false),
        };
        if let Some(t) = gt {
            line.push((format!("{}{t:.0}°C", if approx { "≈" } else { "" }), rgb(sc.heat(t))));
        }
        let g = d.gpu_load.unwrap_or(0.0);
        cards.push(Card {
            icon: icon::GPU,
            title: tr("Видеокарта"),
            sub,
            value: d.gpu_load.map_or("—".into(), |l| format!("{l:.0}")),
            unit: if d.gpu_load.is_some() { "%".into() } else { String::new() },
            color: rgb(sc.at(g / 100.0)),
            line,
            spark: self.column(1, 60),
            spark_max: 100.0,
        });

        // memory
        let gb = |b: u64| dec(format!("{:.1}", b as f64 / (1u64 << 30) as f64));
        let mut sub = String::new();
        if let Some(k) = &d.mem_kind {
            sub = match d.mem_speed {
                Some(s) => format!("{k}-{s}"),
                None => k.clone(),
            };
        }
        if !d.mem_sticks.is_empty() {
            let first = d.mem_sticks[0];
            let size = |b: u64| trf("{v} ГБ", &[("v", format!("{}", b >> 30))]);
            let sticks = if d.mem_sticks.iter().all(|s| *s == first) && d.mem_sticks.len() > 1 {
                format!("{} × {}", d.mem_sticks.len(), size(first))
            } else {
                size(d.mem_sticks.iter().sum())
            };
            sub = if sub.is_empty() { sticks } else { format!("{sub} · {sticks}") };
        }
        if sub.is_empty() {
            sub = trf("{v} ГБ", &[("v", gb(d.ram_total))]);
        }
        let line = vec![
            (trf("{used} из {total} ГБ", &[("used", gb(d.ram_used)), ("total", gb(d.ram_total))]), pal().dim),
            (trf("свободно {v} ГБ", &[("v", gb(d.ram_total.saturating_sub(d.ram_used)))]), rgb(sc.at(0.05))),
        ];
        cards.push(Card {
            icon: icon::RAM,
            title: tr("Память"),
            sub,
            value: format!("{:.0}", d.ram_pct),
            unit: "%".into(),
            color: rgb(sc.at(d.ram_pct / 100.0)),
            line,
            spark: self.column(2, 60),
            spark_max: 100.0,
        });

        // the system drive (else the first)
        let disk = d.disks.iter().find(|x| x.0.eq_ignore_ascii_case("C:")).or(d.disks.first());
        if let Some((letter, used, total, pct)) = disk {
            let drive = d.drives.iter().find(|x| x.letters.iter().any(|l| l == letter));
            let sub = drive.map(|x| x.model.clone()).filter(|m| !m.is_empty()).unwrap_or_default();
            let mut line =
                vec![(trf("{used} из {total} ГБ", &[("used", gb(*used)), ("total", gb(*total))]), pal().dim)];
            if let Some(x) = drive {
                let health = 100u8.saturating_sub(x.wear);
                let mut t = trf("здоровье {v}%", &[("v", health.to_string())]);
                if let Some(temp) = x.temp {
                    t = format!("{t} · {temp:.0}°C");
                }
                line.push((t, rgb(sc.at(x.wear as f32 / 100.0))));
            }
            let io = self.column(3, 60);
            // a little headroom, and a floor so an idle disk stays low
            let io_max = io.iter().copied().fold(10.0 * 1024.0 * 1024.0, f32::max) * 1.25;
            cards.push(Card {
                icon: icon::DISK,
                title: trf("Диск {d}", &[("d", letter.clone())]),
                sub,
                value: format!("{pct:.0}"),
                unit: tr("% занято"),
                color: rgb(sc.at(pct / 100.0)),
                line,
                spark: io,
                spark_max: io_max,
            });
        }

        for row in cards.chunks(cols) {
            let (r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
            for (i, c) in row.iter().enumerate() {
                let cr = Rect::from_min_size(pos2(r.left() + i as f32 * (cw + GAP), r.top()), vec2(cw, h));
                c.paint(ui.painter(), cr);
            }
            if cols < 4 {
                ui.add_space(GAP);
            }
        }
    }

    /// "Нагрузка": the CPU solid and the GPU dashed over the chosen range.
    fn load_chart(&mut self, ui: &mut Ui, r: Rect, input: &HomeInput) {
        let p = ui.painter().clone();
        panel(&p, r);
        p.text(r.left_top() + vec2(18.0, 16.0), Align2::LEFT_TOP, tr("Нагрузка"), bold(15.0), pal().text);
        let names: Vec<String> = RANGES.iter().map(|(n, _)| tr(n)).collect();
        if let Some(i) = segments(ui, r.right_top() + vec2(-14.0, 12.0), &names, self.range, "range") {
            self.range = i;
        }
        let span = RANGES[self.range].1;
        // the series: from memory for a minute or five, from the database for longer
        let (cpu, gpu): (Vec<Option<f32>>, Vec<Option<f32>>) = if span <= 300 {
            let n = span as usize;
            let c = self.column(0, n);
            let g = self.column(1, n);
            let pad = n.saturating_sub(c.len());
            let fill = |v: Vec<f32>| std::iter::repeat_n(None, pad).chain(v.into_iter().map(Some)).collect();
            (fill(c), fill(g))
        } else {
            self.load_db(span);
            let now = pcm_win::system::unix_now();
            let n = 240;
            let get = |col| {
                buckets(&self.points, now - span, now, n, col)
                    .into_iter()
                    .map(|b| b.map(|(avg, _)| avg as f32))
                    .collect::<Vec<_>>()
            };
            (get(Column::Cpu), get(Column::Gpu))
        };
        let last = |v: &[Option<f32>]| v.iter().rev().find_map(|x| *x);
        // the legend with the latest values
        let ly = r.top() + 58.0;
        let mut x = r.left() + 18.0;
        for (name, col, dashed, v) in [
            (tr("Процессор"), pal().cpu_line, false, last(&cpu).or(Some(input.data.cpu))),
            (tr("Видеокарта"), pal().gpu_line, true, last(&gpu).or(input.data.gpu_load)),
        ] {
            let y = ly + 8.0;
            if dashed {
                for k in 0..3 {
                    let x0 = x + k as f32 * 6.0;
                    p.line_segment([pos2(x0, y), pos2(x0 + 3.5, y)], Stroke::new(2.0, col));
                }
            } else {
                p.line_segment([pos2(x, y), pos2(x + 16.0, y)], Stroke::new(2.0, col));
            }
            x += 24.0;
            x = run(
                &p,
                pos2(x, ly),
                &[
                    (format!("{name}  "), text_font(13.0), pal().dim),
                    (v.map_or("—".into(), |v| format!("{v:.0}%")), bold(13.0), pal().text),
                ],
            ) + 26.0;
        }
        // the plot
        let plot = Rect::from_min_max(
            pos2(r.left() + 58.0, r.top() + 92.0),
            pos2(r.right() - 20.0, r.bottom() - 40.0),
        );
        for k in 0..=4 {
            let v = k as f32 * 25.0;
            let y = plot.bottom() - plot.height() * v / 100.0;
            p.hline(plot.x_range(), y, Stroke::new(1.0, pal().grid));
            p.text(
                pos2(plot.left() - 12.0, y),
                Align2::RIGHT_CENTER,
                format!("{v:.0}%"),
                text_font(11.5),
                pal().faint,
            );
        }
        let n = cpu.len().max(2);
        let xy = |i: usize, v: f32| {
            pos2(
                plot.left() + plot.width() * i as f32 / (n - 1) as f32,
                plot.bottom() - plot.height() * (v / 100.0).clamp(0.0, 1.0),
            )
        };
        for (series, col, dashed) in [(&gpu, pal().gpu_line, true), (&cpu, pal().cpu_line, false)] {
            // a line per stretch without gaps
            let mut runs: Vec<Vec<Pos2>> = vec![Vec::new()];
            for (i, v) in series.iter().enumerate() {
                match v {
                    Some(v) => runs.last_mut().unwrap().push(xy(i, *v)),
                    None if !runs.last().unwrap().is_empty() => runs.push(Vec::new()),
                    None => {}
                }
            }
            for pts in runs.into_iter().filter(|r| r.len() >= 2) {
                if dashed {
                    p.extend(Shape::dashed_line(&pts, Stroke::new(1.6, col), 5.0, 4.0));
                } else {
                    p.add(Shape::line(pts, Stroke::new(1.8, col)));
                }
            }
        }
        // time labels
        let labels = 5;
        for k in 0..labels {
            let t = k as f32 / (labels - 1) as f32;
            let ago = ((1.0 - t) * span as f32).round() as i64;
            let label = if ago == 0 {
                tr("сейчас")
            } else if span <= 60 {
                trf("−{v} с", &[("v", ago.to_string())])
            } else if span <= 300 {
                trf("−{v} мин", &[("v", dec(format!("{:.0}", ago as f32 / 60.0)))])
            } else {
                clock(pcm_win::system::unix_now() - ago, false)
            };
            let x = plot.left() + plot.width() * t;
            let anchor = match k {
                0 => Align2::LEFT_TOP,
                k if k == labels - 1 => Align2::RIGHT_TOP,
                _ => Align2::CENTER_TOP,
            };
            p.text(pos2(x, plot.bottom() + 10.0), anchor, label, text_font(11.5), pal().faint);
        }
        // the mouse: a line, the dots and a box with the values
        let resp = ui.interact(plot, ui.scope_id().with("plot"), Sense::hover());
        if let Some(m) = resp.hover_pos()
            && cpu.len() >= 2
        {
            let i = (((m.x - plot.left()) / plot.width()) * (n - 1) as f32).round().clamp(0.0, (n - 1) as f32)
                as usize;
            let x = xy(i, 0.0).x;
            p.vline(x, plot.y_range(), Stroke::new(1.0, pal().dim));
            let ago = ((n - 1 - i) as f32 / (n - 1) as f32 * span as f32).round() as i64;
            let when = clock(pcm_win::system::unix_now() - ago, span <= 300);
            let mut rows = vec![(when, None::<Color32>)];
            for (name, series, col) in [("CPU", &cpu, pal().cpu_line), ("GPU", &gpu, pal().gpu_line)] {
                if let Some(v) = series.get(i).copied().flatten() {
                    p.circle_filled(xy(i, v), 4.0, col);
                    p.circle_stroke(xy(i, v), 4.0, Stroke::new(1.5, pal().card));
                    rows.push((format!("{name}  {v:.0}%"), Some(col)));
                }
            }
            let bw = 118.0;
            let bh = 12.0 + rows.len() as f32 * 20.0;
            let bx = if x + 14.0 + bw > plot.right() { x - 14.0 - bw } else { x + 14.0 };
            let b = Rect::from_min_size(pos2(bx, plot.top() + 8.0), vec2(bw, bh));
            p.rect_filled(b, 8.0, pal().bg);
            p.rect_stroke(b, 8.0, Stroke::new(1.0, pal().border), StrokeKind::Inside);
            for (k, (t, dot)) in rows.iter().enumerate() {
                let y = b.top() + 8.0 + k as f32 * 20.0;
                let mut tx = b.left() + 12.0;
                if let Some(c) = dot {
                    p.circle_filled(pos2(tx + 4.0, y + 8.0), 4.0, *c);
                    tx += 14.0;
                }
                p.text(
                    pos2(tx, y),
                    Align2::LEFT_TOP,
                    t,
                    text_font(12.5),
                    if dot.is_some() { pal().text } else { pal().dim },
                );
            }
        }
    }

    /// "Температуры": a bar a part, then the fans.
    fn temps(&mut self, ui: &mut Ui, r: Rect, d: &Data, sc: &Scale) {
        let p = ui.painter().clone();
        panel(&p, r);
        p.text(r.left_top() + vec2(18.0, 16.0), Align2::LEFT_TOP, tr("Температуры"), bold(15.0), pal().text);
        let units = ["°C".to_string(), "°F".to_string()];
        if let Some(i) =
            segments(ui, r.right_top() + vec2(-14.0, 12.0), &units, self.fahrenheit as usize, "unit")
        {
            self.fahrenheit = i == 1;
        }
        let mut rows: Vec<(String, f32, bool)> = Vec::new();
        if let Some(t) = d.temp_cpu {
            rows.push((tr("Процессор"), t, d.temp_cpu_approx));
        }
        match (d.temp_gpu, d.gpu_shared) {
            (Some(t), _) => rows.push((tr("Видеокарта"), t, false)),
            (None, true) if d.temp_cpu.is_some() => {
                rows.push((tr("Видеокарта"), d.temp_cpu.unwrap_or(0.0), true))
            }
            _ => {}
        }
        for x in &d.drives {
            if let Some(t) = x.temp {
                let name = match x.letters.first() {
                    Some(l) => trf("Диск {d}", &[("d", l.clone())]),
                    None => x.model.clone(),
                };
                rows.push((name, t, false));
            }
        }
        let left = r.left() + 18.0;
        let right = r.right() - 18.0;
        let mut y = r.top() + 60.0;
        if rows.is_empty() {
            let hint = d.sensor_hint.clone().unwrap_or_else(|| tr("Датчики температуры не отвечают"));
            let g = p.layout(hint, text_font(13.0), pal().dim, right - left);
            p.galley(pos2(left, y), g, pal().dim);
        }
        let row_h = if rows.len() > 4 { 38.0 } else { 44.0 };
        for (name, t, approx) in rows.iter().take(6) {
            let col = rgb(sc.heat(*t));
            p.text(
                pos2(left, y),
                Align2::LEFT_TOP,
                fit(&p, name, &text_font(13.0), right - left - 70.0),
                text_font(13.0),
                pal().text,
            );
            let v = if self.fahrenheit { t * 9.0 / 5.0 + 32.0 } else { *t };
            let unit = if self.fahrenheit { "°F" } else { "°C" };
            p.text(
                pos2(right, y),
                Align2::RIGHT_TOP,
                format!("{}{v:.0}{unit}", if *approx { "≈" } else { "" }),
                bold(13.5),
                col,
            );
            let track = Rect::from_min_size(pos2(left, y + 22.0), vec2(right - left, 5.0));
            p.rect_filled(track, 3.0, pal().grid);
            let frac = ((t - 20.0) / 80.0).clamp(0.04, 1.0);
            p.rect_filled(Rect::from_min_size(track.min, vec2(track.width() * frac, 5.0)), 3.0, col);
            y += row_h;
        }
        // fans, two to a row
        let fans: Vec<(String, f32)> = d
            .fans
            .iter()
            .enumerate()
            .map(|(i, rpm)| {
                (
                    d.fan_names
                        .get(i)
                        .cloned()
                        .unwrap_or_else(|| trf("Вентилятор {n}", &[("n", (i + 1).to_string())])),
                    *rpm,
                )
            })
            .collect();
        if !fans.is_empty() {
            let fy = r.bottom() - 62.0;
            p.hline(left..=right, fy, Stroke::new(1.0, pal().border));
            let cw = (right - left) / 2.0;
            for (i, (name, rpm)) in fans.iter().take(2).enumerate() {
                let x = left + i as f32 * cw;
                p.text(
                    pos2(x, fy + 12.0),
                    Align2::LEFT_TOP,
                    fit(&p, name, &text_font(12.0), cw - 10.0),
                    text_font(12.0),
                    pal().dim,
                );
                let v = trf("{v} об/мин", &[("v", format!("{rpm:.0}"))]);
                p.text(pos2(x, fy + 30.0), Align2::LEFT_TOP, v, bold(13.5), pal().text);
            }
        }
    }

    /// "Сеть": down, up and ping, the last minute mirrored (down up, up down).
    fn network(&self, ui: &mut Ui, r: Rect, d: &Data, sc: &Scale) {
        let p = ui.painter().clone();
        panel(&p, r);
        p.text(r.left_top() + vec2(18.0, 16.0), Align2::LEFT_TOP, tr("Сеть"), bold(15.0), pal().text);
        if let Some(ip) = &d.net_ip {
            p.text(r.right_top() + vec2(-18.0, 18.0), Align2::RIGHT_TOP, ip, text_font(12.5), pal().dim);
        }
        let left = r.left() + 18.0;
        let cw = (r.width() - 36.0) / 3.0;
        let speed = |bps: f64| {
            let s = fmt_speed(bps);
            match s.rsplit_once(' ') {
                Some((v, u)) => (v.to_string(), format!(" {u}")),
                None => (s, String::new()),
            }
        };
        let ping = match (d.ping_ready, d.ping) {
            (false, _) => ("…".to_string(), String::new()),
            (true, Some(ms)) => (ms.to_string(), format!(" {}", tr("мс"))),
            (true, None) => ("—".to_string(), String::new()),
        };
        let cols = [
            (tr("Загрузка ↓"), speed(d.net_down), pal().text),
            (tr("Отдача ↑"), speed(d.net_up), pal().text),
            (trf("Пинг {host}", &[("host", d.ping_host.clone())]), ping, rgb(sc.ping(d.ping))),
        ];
        for (i, (label, (v, unit), col)) in cols.iter().enumerate() {
            let x = left + i as f32 * cw;
            p.text(
                pos2(x, r.top() + 52.0),
                Align2::LEFT_TOP,
                fit(&p, label, &text_font(12.0), cw - 6.0),
                text_font(12.0),
                pal().dim,
            );
            run(
                &p,
                pos2(x, r.top() + 70.0),
                &[(v.clone(), bold(21.0), *col), (unit.clone(), text_font(12.5), pal().dim)],
            );
        }
        let g = Rect::from_min_max(pos2(left, r.top() + 116.0), pos2(r.right() - 18.0, r.bottom() - 18.0));
        let mid = g.center().y;
        let down: Vec<f32> = d.net_hist.iter().map(|v| v.0 as f32).collect();
        let up: Vec<f32> = d.net_hist.iter().map(|v| v.1 as f32).collect();
        let max = down.iter().chain(&up).copied().fold(64.0 * 1024.0, f32::max);
        p.hline(g.x_range(), mid, Stroke::new(1.0, pal().grid));
        area(&p, Rect::from_min_max(g.min, pos2(g.right(), mid)), &down, max, pal().cpu_line, 90, 1.5);
        // upload drawn downwards: mirror the values around the middle line
        if up.len() >= 2 {
            let lower = Rect::from_min_max(pos2(g.left(), mid), g.max);
            let n = up.len();
            let pts: Vec<Pos2> = up
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    pos2(
                        lower.left() + lower.width() * i as f32 / (n - 1) as f32,
                        mid + lower.height() * (v / max).clamp(0.0, 1.0),
                    )
                })
                .collect();
            let mut mesh = Mesh::default();
            for (i, pt) in pts.iter().enumerate() {
                mesh.colored_vertex(*pt, alpha(pal().gpu_line, 90));
                mesh.colored_vertex(pos2(pt.x, mid), Color32::TRANSPARENT);
                if i > 0 {
                    let b = (i * 2) as u32;
                    mesh.add_triangle(b - 2, b - 1, b);
                    mesh.add_triangle(b - 1, b + 1, b);
                }
            }
            p.add(Shape::mesh(mesh));
            p.add(Shape::line(pts, Stroke::new(1.5, pal().gpu_line)));
        }
    }

    /// "Больше всего нагружают": the four busiest programs.
    fn programs(&self, ui: &mut Ui, r: Rect, d: &Data, sc: &Scale) -> Option<HomeAction> {
        let p = ui.painter().clone();
        panel(&p, r);
        p.text(
            r.left_top() + vec2(18.0, 16.0),
            Align2::LEFT_TOP,
            tr("Больше всего нагружают"),
            bold(15.0),
            pal().text,
        );
        let mut action = None;
        let all = match d.proc_count {
            Some(n) => trf("Все {n}", &[("n", n.to_string())]),
            None => tr("Все"),
        };
        if link(ui, r.right_top() + vec2(-18.0, 18.0), Align2::RIGHT_TOP, &all, "all-procs") {
            action = Some(HomeAction::Tab(crate::dashboard::TAB_PROCS));
        }
        let left = r.left() + 18.0;
        let right = r.right() - 18.0;
        match &d.procs_all {
            Some(list) if !list.is_empty() => {
                let mut top: Vec<_> = list.iter().collect();
                top.sort_by(|a, b| b.cpu.total_cmp(&a.cpu));
                for (i, row) in top.iter().take(4).enumerate() {
                    let y = r.top() + 56.0 + i as f32 * 34.0;
                    let mut name = row.name.clone();
                    if row.count > 1 {
                        name = format!("{name} ({})", row.count);
                    }
                    let mem_x = right - 70.0;
                    let cpu_x = mem_x - 70.0;
                    p.text(
                        pos2(left, y),
                        Align2::LEFT_TOP,
                        fit(&p, &name, &text_font(13.5), cpu_x - left - 50.0),
                        text_font(13.5),
                        pal().text,
                    );
                    let pct = format!("{}%", dec(format!("{:.1}", row.cpu)));
                    p.text(
                        pos2(cpu_x, y),
                        Align2::RIGHT_TOP,
                        pct,
                        bold(13.5),
                        rgb(sc.at(row.cpu / 100.0).max_soft()),
                    );
                    p.text(
                        pos2(right, y),
                        Align2::RIGHT_TOP,
                        fmt_mem(row.memory),
                        text_font(13.5),
                        pal().dim,
                    );
                }
            }
            _ => {
                p.text(
                    pos2(left, r.top() + 58.0),
                    Align2::LEFT_TOP,
                    tr("Собираю список программ…"),
                    text_font(13.0),
                    pal().dim,
                );
            }
        }
        let foot = format!("{} · {}", trf("Работает {t}", &[("t", fmt_uptime(d.uptime_s))]), d.windows);
        p.text(
            pos2(left, r.bottom() - 16.0),
            Align2::LEFT_BOTTOM,
            fit(&p, &foot, &text_font(12.0), right - left),
            text_font(12.0),
            pal().dim,
        );
        action
    }

    /// "Игровая сессия": the one being recorded, else the last one saved.
    fn game(&self, ui: &mut Ui, r: Rect, live: Option<&LiveGame>, sc: &Scale) -> Option<HomeAction> {
        let p = ui.painter().clone();
        panel(&p, r);
        p.text(
            r.left_top() + vec2(18.0, 16.0),
            Align2::LEFT_TOP,
            tr("Игровая сессия"),
            bold(15.0),
            pal().text,
        );
        let left = r.left() + 18.0;
        let right = r.right() - 18.0;
        let mut action = None;
        // (name line, fps, 1% low, 0.1% low, bars, link)
        let (fps, low1, low01, bars, has_link) = if let Some(g) = live {
            let secs = g.seconds as u64;
            let t = format!("{:02}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60);
            let rx = p.text(
                r.right_top() + vec2(-18.0, 18.0),
                Align2::RIGHT_TOP,
                trf("Запись · {t}", &[("t", t)]),
                text_font(12.5),
                pal().dim,
            );
            p.circle_filled(pos2(rx.left() - 9.0, rx.center().y), 4.0, pal().hot);
            let name = if g.title.is_empty() { g.game.clone() } else { g.title.clone() };
            p.text(
                pos2(left, r.top() + 42.0),
                Align2::LEFT_TOP,
                fit(&p, &name, &text_font(12.5), right - left),
                text_font(12.5),
                pal().dim,
            );
            (g.fps, g.low1, g.low01, g.fps_hist.clone(), false)
        } else if let Some((_, info)) = &self.last_session {
            let s = &info.summary;
            let name = if info.title.is_empty() { info.game.clone() } else { info.title.clone() };
            let when = info.started.get(5..16).map(|t| {
                // "11-22 21:04" -> "22.11 21:04"
                format!(
                    "{}.{} {}",
                    t.get(3..5).unwrap_or(""),
                    t.get(0..2).unwrap_or(""),
                    t.get(6..).unwrap_or("")
                )
            });
            let head =
                trf("Последняя: {name} · {when}", &[("name", name), ("when", when.unwrap_or_default())]);
            p.text(
                pos2(left, r.top() + 42.0),
                Align2::LEFT_TOP,
                fit(&p, &head, &text_font(12.5), right - left),
                text_font(12.5),
                pal().dim,
            );
            let fps: Vec<f32> = info.samples.iter().filter_map(|x| x.fps).collect();
            // the whole session squeezed into LIVE_BARS bars (averages)
            let n = crate::session_rec::LIVE_BARS;
            let bars: Vec<f32> = if fps.len() <= n {
                fps
            } else {
                (0..n)
                    .map(|i| {
                        let a = i * fps.len() / n;
                        let b = ((i + 1) * fps.len() / n).max(a + 1);
                        fps[a..b].iter().sum::<f32>() / (b - a) as f32
                    })
                    .collect()
            };
            let known = |v: f64| (v > 0.0).then_some(v);
            (known(s.avg_fps), known(s.low1), known(s.low01), bars, true)
        } else {
            let hint =
                tr("Запусти игру — сессия запишется сама: FPS, 1% low, нагрузка и температуры по секундам.");
            let g = p.layout(hint, text_font(13.0), pal().dim, right - left);
            p.galley(pos2(left, r.top() + 56.0), g, pal().dim);
            return None;
        };
        let top = r.top() + 70.0;
        let label = |x: f32, t: &str| {
            p.text(pos2(x, top), Align2::LEFT_TOP, t, text_font(12.0), pal().dim);
        };
        let fps_label = if live.is_some() { "FPS" } else { "FPS ⌀" };
        label(left, fps_label);
        label(left + 96.0, "1% low");
        label(left + 170.0, "0.1% low");
        let num = |v: Option<f64>| v.map_or("—".to_string(), |v| format!("{v:.0}"));
        p.text(pos2(left, top + 16.0), Align2::LEFT_TOP, num(fps), bold(32.0), pal().text);
        p.text(pos2(left + 96.0, top + 26.0), Align2::LEFT_TOP, num(low1), bold(20.0), pal().text);
        let warm = rgb(sc.at(0.55));
        p.text(pos2(left + 170.0, top + 26.0), Align2::LEFT_TOP, num(low01), bold(20.0), warm);
        // a bar a second (or a slice of the session), dips in red
        let area_r = Rect::from_min_max(pos2(left, top + 66.0), pos2(right, r.bottom() - 42.0));
        if !bars.is_empty() {
            let mut sorted = bars.clone();
            sorted.sort_by(f32::total_cmp);
            let med = sorted[sorted.len() / 2];
            let max = sorted.last().copied().unwrap_or(1.0).max(1.0);
            let n = crate::session_rec::LIVE_BARS as f32;
            let step = area_r.width() / n;
            let x0 = area_r.right() - step * bars.len() as f32;
            for (i, v) in bars.iter().enumerate() {
                let h = (area_r.height() * v / max).max(2.0);
                let x = x0 + i as f32 * step;
                let col = if *v < med * 0.75 { pal().hot } else { pal().cpu_line };
                p.rect_filled(
                    Rect::from_min_max(
                        pos2(x + 1.0, area_r.bottom() - h),
                        pos2(x + step - 1.5, area_r.bottom()),
                    ),
                    1.5,
                    col,
                );
            }
        }
        let link_text =
            if has_link { tr("Открыть отчёт сессии") } else { tr("Все сессии") };
        if link(ui, pos2(left, r.bottom() - 16.0), Align2::LEFT_BOTTOM, &link_text, "session-link") {
            action = Some(match (&self.last_session, has_link) {
                (Some((path, _)), true) => HomeAction::Session(path.clone()),
                _ => HomeAction::Tab(crate::dashboard::TAB_GAMES),
            });
        }
        action
    }
}

/// "14:32" (or "14:32:18") of unix seconds, local time.
fn clock(unix: i64, seconds: bool) -> String {
    let s = pcm_win::system::local_time_of(unix); // "2026-11-22 13:05:41"
    s.get(11..if seconds { 19 } else { 16 }).unwrap_or("").to_string()
}

/// One of the four cards on top.
struct Card {
    icon: &'static str,
    title: String,
    sub: String,
    value: String,
    unit: String,
    color: Color32,
    /// Pieces of the line under the value.
    line: Vec<(String, Color32)>,
    spark: Vec<f32>,
    spark_max: f32,
}

impl Card {
    fn paint(&self, p: &Painter, r: Rect) {
        panel(p, r);
        let left = r.left() + 18.0;
        let ib = Rect::from_min_size(pos2(left, r.top() + 16.0), vec2(34.0, 34.0));
        p.rect_filled(ib, 9.0, pal().card_hi);
        p.text(ib.center(), Align2::CENTER_CENTER, self.icon, icon_font(16.0), pal().dim);
        let tx = ib.right() + 12.0;
        let max = r.right() - 16.0 - tx;
        p.text(
            pos2(tx, r.top() + 16.0),
            Align2::LEFT_TOP,
            fit(p, &self.title, &bold(14.0), max),
            bold(14.0),
            pal().text,
        );
        p.text(
            pos2(tx, r.top() + 35.0),
            Align2::LEFT_TOP,
            fit(p, &self.sub, &text_font(12.0), max),
            text_font(12.0),
            pal().dim,
        );
        let vy = r.top() + 62.0;
        let end = run(p, pos2(left, vy), &[(self.value.clone(), bold(38.0), self.color)]);
        p.text(pos2(end + 4.0, vy + 22.0), Align2::LEFT_TOP, &self.unit, text_font(15.0), pal().dim);
        let mut x = left;
        for (t, c) in &self.line {
            let rr = p.text(
                pos2(x, r.top() + 112.0),
                Align2::LEFT_TOP,
                fit(p, t, &text_font(12.5), r.right() - 16.0 - x),
                text_font(12.5),
                *c,
            );
            x = rr.right() + 14.0;
            if x > r.right() - 30.0 {
                break;
            }
        }
        // inside the card's rounded corners (Vlad: the line stuck out at the edges)
        let spark = Rect::from_min_max(
            pos2(r.left() + R + 2.0, r.bottom() - 42.0),
            pos2(r.right() - R - 2.0, r.bottom() - 12.0),
        );
        area(p, spark, &self.spark, self.spark_max, self.color, 70, 1.6);
    }
}

/// A colour no darker than a readable minimum on the card (for very calm themes).
trait Soft {
    fn max_soft(self) -> (u8, u8, u8);
}

impl Soft for (u8, u8, u8) {
    fn max_soft(self) -> (u8, u8, u8) {
        let lum = self.0 as u32 * 3 + self.1 as u32 * 6 + self.2 as u32;
        if lum >= 900 { self } else { pcm_render::theme::lerp(self, (232, 234, 239), 0.35) }
    }
}

/// Seconds of ring for tests.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_starts_with_the_widget_minute_and_keeps_five() {
        let mut h = HomeTab::default();
        let d = Data { cpu: 50.0, cpu_hist: vec![10.0; 60], gpu_hist: vec![20.0; 60], ..Data::default() };
        h.remember(&d, 1);
        assert_eq!(h.ring.len(), 60, "59 from the widget + this second");
        h.remember(&d, 1);
        assert_eq!(h.ring.len(), 60, "the same feed counts once");
        for s in 2..400 {
            h.remember(&d, s);
        }
        assert_eq!(h.ring.len(), RING);
        assert_eq!(h.column(0, 60).len(), 60);
        assert_eq!(h.column(1, 1), vec![0.0], "no GPU load in this feed");
    }
}

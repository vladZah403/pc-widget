//! A game session's report in the PC Monitor window (beta 2, Vlad's
//! concept "Отчёт о сессии"): the numbers as tiles, four charts on one time
//! axis (frames per second, frame time, load, temperature), what limits the
//! FPS, the system, a note, a comparison with another session, CSV.
//!
//! Painted with egui shapes like the home page; the mouse over any chart
//! shows that moment on all of them.

use std::path::{Path, PathBuf};
use std::time::Instant;

use eframe::egui;
use egui::{
    Align2, Color32, CursorIcon, Mesh, Painter, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2,
};
use pcm_core::frames::FRAME_PAUSE_S;
use pcm_core::session::{Session, SessionInfo};
use pcm_render::format::{dec, tr, trf};
use pcm_render::theme::Scale;

use crate::dash_home::{alpha, bold, fit, link, pal, panel, rgb, text_font};
use crate::report_win::{save_dialog, session_csv_name, write_session_csv};

const GAP: f32 = 16.0;
const SIDE_W: f32 = 300.0;
/// Columns the frame-time chart is drawn in.
const FRAME_COLS: usize = 480;
/// "в первые 2 минуты": loading hitches.
const EARLY_S: f32 = 120.0;
/// Stutters as % of the frames: from 1% the tile turns yellow, from 3% red.
const STUTTER_WARN: f64 = 1.0;
const STUTTER_HOT: f64 = 3.0;
/// The GPU's load (75th percentile) from which the FPS is the GPU's limit.
/// Integrated graphics share the chip's power with the CPU and rarely show
/// more than ~85% (Vlad's Radeon in Civilization VI: 83–86%).
const GPU_BOUND: f32 = 90.0;
const IGPU_BOUND: f32 = 80.0;
const NOTE_SAVE_EVERY: f32 = 0.8;

/// The load and temperature lines: the GPU solid, the CPU dashed.
const GPU_C: Color32 = Color32::from_rgb(243, 146, 55);
const GPU_T: Color32 = Color32::from_rgb(240, 177, 64);

/// A tile: label, value, unit, the value's colour, the change against the compared session.
type Tile = (String, String, String, Color32, Option<(String, Color32)>);
/// "Видеокарта", its load's 25th and 75th percentile.
type LoadBar = (String, (f32, f32));

/// What the report asks of the page around it.
pub enum ReportAction {
    Back,
    Deleted,
}

/// Worked out once when the report opens.
struct Stats {
    /// The stutter threshold (ms), the stutters' times (s from the start).
    thr_ms: f32,
    stutters: Vec<f32>,
    /// The longest frame time per column, the average and its stutters.
    cols: Vec<(f32, f32, u32)>,
    /// A column with at least this many stutters gets a red mark.
    mark_min: u32,
    /// Frames played (pauses left out).
    frames: usize,
    gpu_range: Option<(f32, f32)>,
    cpu_range: Option<(f32, f32)>,
    gpu_t_peak: Option<f32>,
    cpu_t_peak: Option<f32>,
    /// Seconds the time axis covers.
    span: f32,
    /// Seconds the processor was held back (throttle.rs), and when: from,
    /// to (s), heat (else power).
    throttle: pcm_core::throttle::Totals,
    throttle_runs: Vec<(f32, f32, bool)>,
}

/// The stretches of throttling, a second's gap closing one.
fn throttle_runs(samples: &[pcm_core::session::Sample]) -> Vec<(f32, f32, bool)> {
    use pcm_core::throttle::{Cause, check};
    let mut runs: Vec<(f32, f32, bool)> = Vec::new();
    for x in samples {
        let Some(th) = check(x.limit, x.cpu_t, x.cpu) else { continue };
        let heat = th.cause == Cause::Heat;
        match runs.last_mut() {
            Some(r) if r.2 == heat && x.t - r.1 <= 1.5 => r.1 = x.t,
            _ => runs.push((x.t - 1.0, x.t, heat)),
        }
    }
    runs
}

fn quartiles(mut v: Vec<f32>) -> Option<(f32, f32)> {
    if v.len() < 4 {
        return None;
    }
    v.sort_by(f32::total_cmp);
    Some((v[v.len() / 4], v[v.len() * 3 / 4]))
}

fn stats(s: &Session) -> Stats {
    let info = &s.info;
    let thr_ms = pcm_core::session::stutter_ms(info.summary.median_ms);
    let pause = (FRAME_PAUSE_S * 1000.0) as f32;
    let frames_s: f32 = s.frame_ms.iter().sum::<f32>() / 1000.0;
    let last_sample = info.samples.last().map_or(0.0, |x| x.t);
    let span = (info.duration_s as f32).max(frames_s).max(last_sample).max(1.0);
    // per column: the longest frame, the sum, frames, stutters
    let mut cols = vec![(0f32, 0f32, 0u32, 0u32); FRAME_COLS];
    let mut stutters = Vec::new();
    let mut frames = 0;
    let mut t = 0.0f32;
    for &ms in &s.frame_ms {
        t += ms / 1000.0;
        if ms <= 0.0 || ms >= pause {
            continue; // a pause (alt-tab, loading screen), not a frame
        }
        frames += 1;
        let k = ((t / span) * FRAME_COLS as f32) as usize;
        let c = &mut cols[k.min(FRAME_COLS - 1)];
        if ms > thr_ms {
            stutters.push(t);
            c.3 += 1;
        }
        c.0 = c.0.max(ms);
        c.1 += ms;
        c.2 += 1;
    }
    let played = cols.iter().filter(|c| c.2 > 0).count();
    let mark_min = pcm_core::session::stutter_mark_min(stutters.len(), played);
    let cols =
        cols.into_iter().map(|(mx, sum, n, st)| (mx, if n > 0 { sum / n as f32 } else { 0.0 }, st)).collect();
    let pick = |f: fn(&pcm_core::session::Sample) -> Option<f32>| {
        info.samples.iter().filter_map(f).collect::<Vec<_>>()
    };
    let peak = |v: Vec<f32>| v.into_iter().reduce(f32::max);
    Stats {
        thr_ms,
        stutters,
        cols,
        mark_min,
        frames,
        gpu_range: quartiles(pick(|x| x.gpu)),
        cpu_range: quartiles(pick(|x| x.cpu)),
        gpu_t_peak: peak(pick(|x| x.gpu_t)),
        cpu_t_peak: peak(pick(|x| x.cpu_t)),
        span,
        throttle: pcm_core::throttle::totals(&info.samples),
        throttle_runs: throttle_runs(&info.samples),
    }
}

/// "31 с", "23 мин 17 с", "1 ч 05 мин"
fn long_time(s: f64) -> String {
    let s = s.max(0.0) as u64;
    if s < 60 {
        trf("{v} с", &[("v", s.to_string())])
    } else if s >= 3600 {
        trf("{h} ч {m} мин", &[("h", (s / 3600).to_string()), ("m", format!("{:02}", s / 60 % 60))])
    } else {
        trf("{m} мин {s} с", &[("m", (s / 60).to_string()), ("s", (s % 60).to_string())])
    }
}

/// Stutters in % of the frames played.
fn stutter_share(stutters: usize, frames: usize) -> f64 {
    if frames == 0 { 0.0 } else { stutters as f64 * 100.0 / frames as f64 }
}

/// "0,5%", "<0,1%", "0%"
fn share_text(share: f64) -> String {
    if share <= 0.0 {
        "0%".into()
    } else if share < 0.1 {
        format!("<{}%", dec("0.1".into()))
    } else {
        format!("{}%", dec(format!("{share:.1}")))
    }
}

/// "200 940"
fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push('\u{202F}');
        }
        out.push(c);
    }
    out
}

/// The note beside a session file.
fn note_path(p: &Path) -> PathBuf {
    p.with_extension("note.txt")
}

pub struct ReportPage {
    pub path: PathBuf,
    session: Session,
    stats: Stats,
    note: String,
    note_saved: (String, Instant),
    compare: Option<SessionInfo>,
    delete_armed: Option<Instant>,
    /// Came from the home page: "← Главная".
    pub from_home: bool,
    /// Test runs' screenshots of the lower part (PCM_REPORT_SCROLL=px).
    start_offset: Option<f32>,
    /// "Поделиться" was clicked (done in ui(), which has the colours).
    share_now: bool,
    /// What it did, shown under the buttons for a while: text, good, since.
    share_note: Option<(String, bool, Instant)>,
}

impl ReportPage {
    pub fn open(p: &Path) -> std::io::Result<ReportPage> {
        let session = Session::load(p)?;
        let stats = stats(&session);
        let note = std::fs::read_to_string(note_path(p)).unwrap_or_default();
        Ok(ReportPage {
            path: p.to_path_buf(),
            session,
            stats,
            note_saved: (note.clone(), Instant::now()),
            note,
            compare: None,
            delete_armed: None,
            from_home: false,
            start_offset: std::env::var("PCM_REPORT_SCROLL").ok().and_then(|v| v.parse().ok()),
            share_now: false,
            share_note: None,
        })
    }

    fn save_note(&mut self) {
        if self.note != self.note_saved.0 {
            let path = note_path(&self.path);
            let _ = if self.note.trim().is_empty() {
                std::fs::remove_file(&path)
            } else {
                std::fs::write(&path, &self.note)
            };
            self.note_saved = (self.note.clone(), Instant::now());
        }
    }

    /// `others`: the other sessions to compare with (newest first).
    pub fn ui(&mut self, ui: &mut Ui, sc: &Scale, others: &[(PathBuf, SessionInfo)]) -> Option<ReportAction> {
        if self.delete_armed.is_some_and(|t| t.elapsed().as_secs_f32() > 3.0) {
            self.delete_armed = None;
        }
        let mut action = None;
        let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]);
        if let Some(o) = self.start_offset.take() {
            area = area.vertical_scroll_offset(o);
        }
        area.show(ui, |ui| {
            let w = ui.available_width();
            if let Some(a) = self.header(ui, w, others) {
                action = Some(a);
            }
            if std::mem::take(&mut self.share_now) {
                self.share(sc);
            }
            ui.add_space(14.0);
            self.tiles(ui, w, sc);
            ui.add_space(GAP);
            let wide = w >= 980.0;
            let charts_w = if wide { w - SIDE_W - GAP } else { w };
            let chart_h = 190.0;
            let (area, _) = ui.allocate_exact_size(vec2(w, 4.0 * chart_h + 3.0 * GAP + 28.0), Sense::hover());
            let rects: Vec<Rect> = (0..4)
                .map(|i| {
                    Rect::from_min_size(
                        pos2(area.left(), area.top() + i as f32 * (chart_h + GAP)),
                        vec2(charts_w, chart_h),
                    )
                })
                .collect();
            self.charts(ui, &rects, sc);
            if wide {
                let side = Rect::from_min_size(
                    pos2(area.left() + charts_w + GAP, area.top()),
                    vec2(SIDE_W, area.height()),
                );
                self.side(ui, side, sc);
            } else {
                ui.add_space(GAP);
                let (side, _) = ui.allocate_exact_size(vec2(w, 760.0), Sense::hover());
                self.side(ui, side, sc);
            }
            ui.add_space(8.0);
        });
        if self.note != self.note_saved.0 && self.note_saved.1.elapsed().as_secs_f32() > NOTE_SAVE_EVERY {
            self.save_note();
        }
        if self.delete_armed.is_some() || self.note != self.note_saved.0 {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(900));
        }
        if matches!(action, Some(ReportAction::Back)) {
            self.save_note();
        }
        action
    }

    /// "← Все сессии", the game, when and how long; compare, CSV, delete.
    fn header(&mut self, ui: &mut Ui, w: f32, others: &[(PathBuf, SessionInfo)]) -> Option<ReportAction> {
        let info = self.session.info.clone();
        let (r, _) = ui.allocate_exact_size(vec2(w, 78.0), Sense::hover());
        let p = ui.painter().clone();
        let mut action = None;
        let back = if self.from_home { tr("← Главная") } else { tr("← Все сессии") };
        if link(ui, r.left_top(), Align2::LEFT_TOP, &back, "report-back") {
            action = Some(ReportAction::Back);
        }
        let name = if info.title.trim().is_empty() { info.game.clone() } else { info.title.clone() };
        let title = trf("Сессия: {name}", &[("name", name)]);
        // the four buttons' width (as button() lays them out) and a gap: the rest is the title's
        let right_room = {
            let cmp = match &self.compare {
                Some(c) => trf("Сравнение: {when} ✕", &[("when", short_when(&c.started))]),
                None => tr("Сравнить с другой сессией"),
            };
            let del = if self.delete_armed.is_some() {
                tr("Точно удалить?")
            } else {
                tr("Удалить")
            };
            [cmp, tr("Поделиться"), tr("Экспорт CSV"), del]
                .iter()
                .map(|t| p.layout_no_wrap(t.clone(), bold(13.0), pal().text).size().x + 32.0 + 10.0)
                .sum::<f32>()
                + 24.0
        };
        p.text(
            pos2(r.left(), r.top() + 20.0),
            Align2::LEFT_TOP,
            fit(&p, &title, &bold(22.0), w - right_room),
            bold(22.0),
            pal().text,
        );
        // "22.11.2026 · 21:04–21:27 · 23 мин 17 с · 200 940 кадров · DXGI"
        let date = info.started.get(..10).map(|d| {
            let p: Vec<&str> = d.split('-').collect();
            if p.len() == 3 { format!("{}.{}.{}", p[2], p[1], p[0]) } else { d.to_string() }
        });
        let start = info.started.get(11..16).unwrap_or("").to_string();
        let end = end_clock(&info.started, info.duration_s);
        let mut sub =
            vec![date.unwrap_or_default(), format!("{start}–{end}"), long_time(info.summary.active_s)];
        sub.push(trf("{n} кадров", &[("n", thousands(info.summary.frames))]));
        if !info.source.is_empty() {
            sub.push(if info.source == "dxgi" { "DXGI".into() } else { tr("кадры от ядра") });
        }
        // beside the buttons: cut to fit a narrow window
        let sub = fit(&p, &sub.join(" · "), &text_font(13.0), w - right_room);
        p.text(pos2(r.left(), r.top() + 52.0), Align2::LEFT_TOP, sub, text_font(13.0), pal().dim);

        // buttons, right to left: Удалить, Экспорт CSV, Сравнить
        let mut x = r.right();
        let y = r.top() + 22.0;
        let armed = self.delete_armed.is_some();
        let del = if armed { tr("Точно удалить?") } else { tr("Удалить") };
        let (b, clicked) = button(ui, x, y, &del, false, armed, "report-del");
        if clicked {
            if armed {
                self.delete_armed = None;
                if std::fs::remove_file(&self.path).is_ok() {
                    let _ = std::fs::remove_file(note_path(&self.path));
                    action = Some(ReportAction::Deleted);
                }
            } else {
                self.delete_armed = Some(Instant::now());
            }
        }
        x = b.left() - 10.0;
        let (b, clicked) = button(ui, x, y, &tr("Экспорт CSV"), true, false, "report-csv");
        if clicked && let Some(path) = save_dialog(std::ptr::null_mut(), &session_csv_name(&info)) {
            let _ = write_session_csv(&self.session, &path);
        }
        x = b.left() - 10.0;
        let (b, clicked) = button(ui, x, y, &tr("Поделиться"), false, false, "report-share");
        if clicked {
            self.share_now = true;
        }
        // what "Поделиться" did, under the buttons
        if let Some((text, good, since)) = &self.share_note {
            if since.elapsed().as_secs_f32() < 8.0 {
                let col = if *good { pal().good } else { pal().hot };
                p.text(pos2(r.right(), r.top() + 64.0), Align2::RIGHT_TOP, text, text_font(12.0), col);
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
            } else {
                self.share_note = None;
            }
        }
        x = b.left() - 10.0;
        let label = match &self.compare {
            Some(c) => trf("Сравнение: {when} ✕", &[("when", short_when(&c.started))]),
            None => tr("Сравнить с другой сессией"),
        };
        let (_, clicked) = button(ui, x, y, &label, false, false, "report-cmp");
        let popup_id = ui.make_persistent_id("compare-list");
        if clicked {
            if self.compare.is_some() {
                self.compare = None;
            } else {
                egui::Popup::toggle_id(ui.ctx(), popup_id);
            }
        }
        // the list: the same game first, then the rest, newest first
        let mut list: Vec<&(PathBuf, SessionInfo)> = others.iter().filter(|(q, _)| *q != self.path).collect();
        list.sort_by_key(|(_, i)| i.game != info.game);
        let anchor = Rect::from_min_size(pos2(x - 280.0, y + 36.0), vec2(280.0, 1.0));
        let mut chosen = None;
        egui::Popup::new(popup_id, ui.ctx().clone(), anchor, ui.layer_id())
            .open_memory(None)
            .width(280.0)
            .show(|ui| {
                if list.is_empty() {
                    ui.label(egui::RichText::new(tr("Других сессий пока нет")).color(pal().dim));
                }
                for (_, i) in list.iter().take(15) {
                    let n = if i.title.trim().is_empty() { &i.game } else { &i.title };
                    let t = format!("{} · {} · {:.0} FPS", short_when(&i.started), n, i.summary.avg_fps);
                    if ui.selectable_label(false, t).clicked() {
                        chosen = Some(i.clone());
                    }
                }
            });
        if let Some(c) = chosen {
            self.compare = Some(c);
            egui::Popup::close_id(ui.ctx(), popup_id);
        }
        // a line under the header, like the concept
        p.hline(r.x_range(), r.bottom() + 6.0, Stroke::new(1.0, pal().border));
        action
    }

    /// Средний FPS, 1% low, 0.1% low, мин / макс, время кадра, подтормаживания, пик температуры.
    fn tiles(&self, ui: &mut Ui, w: f32, sc: &Scale) {
        let s = &self.session.info.summary;
        let cmp = self.compare.as_ref().map(|c| &c.summary);
        let num = |v: f64| if v > 0.0 { format!("{v:.0}") } else { "—".into() };
        let delta = |now: f64, was: Option<f64>| -> Option<(String, Color32)> {
            let was = was.filter(|w| *w > 0.0)?;
            let d = now - was;
            let col = if d >= 0.5 {
                pal().good
            } else if d <= -0.5 {
                pal().hot
            } else {
                pal().dim
            };
            Some((format!("{}{:.0}", if d >= 0.0 { "+" } else { "−" }, d.abs()), col))
        };
        // the hotter of the two (an APU's CPU runs hotter than its graphics)
        let (peak, peak_unit) = match (self.stats.gpu_t_peak, self.stats.cpu_t_peak) {
            (Some(g), Some(c)) if c > g => (Some(c), "°C CPU"),
            (Some(g), _) => (Some(g), "°C GPU"),
            (None, Some(c)) => (Some(c), "°C CPU"),
            _ => (None, ""),
        };
        // stutters as a share of the frames: a count grows with the session's length
        let share = stutter_share(self.stats.stutters.len(), self.stats.frames);
        let tiles: Vec<Tile> = vec![
            (
                tr("Средний FPS"),
                num(s.avg_fps),
                String::new(),
                pal().text,
                delta(s.avg_fps, cmp.map(|c| c.avg_fps)),
            ),
            ("1% low".into(), num(s.low1), String::new(), pal().text, delta(s.low1, cmp.map(|c| c.low1))),
            (
                "0.1% low".into(),
                num(s.low01),
                String::new(),
                pal().warn,
                delta(s.low01, cmp.map(|c| c.low01)),
            ),
            (
                tr("Мин / макс"),
                format!("{} / {}", num(s.min_fps), num(s.max_fps)),
                String::new(),
                pal().text,
                None,
            ),
            (tr("Время кадра"), dec(format!("{:.1}", s.median_ms)), tr("мс"), pal().text, None),
            (
                tr("Подтормаживания"),
                share_text(share),
                format!("> {:.0} {}", self.stats.thr_ms, tr("мс")),
                if share >= STUTTER_HOT {
                    pal().hot
                } else if share >= STUTTER_WARN {
                    pal().warn
                } else if share > 0.0 {
                    pal().text
                } else {
                    pal().good
                },
                None,
            ),
            (
                tr("Пик температуры"),
                peak.map_or("—".into(), |t| format!("{t:.0}")),
                peak_unit.into(),
                peak.map_or(pal().text, |t| rgb(sc.heat(t))),
                None,
            ),
        ];
        let cols = if w >= 1100.0 {
            7
        } else if w >= 640.0 {
            4
        } else {
            2
        };
        let tw = (w - GAP * (cols - 1) as f32) / cols as f32;
        let th = 78.0;
        for row in tiles.chunks(cols) {
            let (r, _) = ui.allocate_exact_size(vec2(w, th), Sense::hover());
            let p = ui.painter();
            for (i, (label, value, unit, col, d)) in row.iter().enumerate() {
                let t = Rect::from_min_size(pos2(r.left() + i as f32 * (tw + GAP), r.top()), vec2(tw, th));
                panel(p, t);
                p.text(
                    t.left_top() + vec2(16.0, 13.0),
                    Align2::LEFT_TOP,
                    fit(p, label, &text_font(12.0), tw - 24.0),
                    text_font(12.0),
                    pal().dim,
                );
                let vr = p.text(t.left_top() + vec2(15.0, 32.0), Align2::LEFT_TOP, value, bold(26.0), *col);
                if !unit.is_empty() {
                    p.text(
                        pos2(vr.right() + 6.0, vr.bottom() - 6.0),
                        Align2::LEFT_BOTTOM,
                        unit,
                        text_font(12.0),
                        pal().dim,
                    );
                }
                if let Some((dt, dc)) = d {
                    p.text(
                        pos2(t.right() - 14.0, vr.bottom() - 6.0),
                        Align2::RIGHT_BOTTOM,
                        dt,
                        bold(12.5),
                        *dc,
                    );
                }
            }
            ui.add_space(GAP);
        }
    }

    /// The four charts; the mouse over one shows that moment on all.
    fn charts(&self, ui: &mut Ui, rects: &[Rect], sc: &Scale) {
        let info = &self.session.info;
        let span = self.stats.span;
        let plots: Vec<Rect> = rects
            .iter()
            .map(|r| {
                Rect::from_min_max(
                    pos2(r.left() + 52.0, r.top() + 46.0),
                    pos2(r.right() - 18.0, r.bottom() - 14.0),
                )
            })
            .collect();
        // the moment under the mouse (0..1 of the time axis)
        let hover = ui.ctx().pointer_hover_pos().and_then(|m| {
            plots.iter().find(|p| p.contains(m)).map(|p| ((m.x - p.left()) / p.width()).clamp(0.0, 1.0))
        });
        let p = ui.painter().clone();
        let at = |t: f32, plot: &Rect| plot.left() + plot.width() * (t / span).clamp(0.0, 1.0);
        let series = |f: fn(&pcm_core::session::Sample) -> Option<f32>, s: &SessionInfo| -> Vec<(f32, f32)> {
            s.samples.iter().filter_map(|x| f(x).map(|v| (x.t, v))).collect()
        };

        // 1) frames per second, the monitor's refresh dashed, the compared session dashed grey
        let fps = series(|x| x.fps, info);
        let cmp = self.compare.as_ref().map(|c| series(|x| x.fps, c)).unwrap_or_default();
        let top = fps.iter().chain(&cmp).map(|v| v.1).fold(info.refresh_hz as f32, f32::max).max(30.0);
        let hi = (top * 1.08 / 30.0).ceil() * 30.0;
        let lo = {
            let m = fps.iter().map(|v| v.1).filter(|v| *v > 0.0).fold(f32::MAX, f32::min);
            if m == f32::MAX { 0.0 } else { ((m * 0.8) / 30.0).floor() * 30.0 }
        };
        let note = if info.refresh_hz > 0 {
            trf("пунктир — частота монитора {hz} Гц", &[("hz", info.refresh_hz.to_string())])
        } else {
            String::new()
        };
        frame(&p, rects[0], &tr("Кадры в секунду"), &note);
        let y_of = |v: f32, plot: &Rect, lo: f32, hi: f32| {
            plot.bottom() - plot.height() * ((v - lo) / (hi - lo)).clamp(0.0, 1.0)
        };
        grid(&p, plots[0], lo, hi, 2, |v| format!("{v:.0}"));
        if info.refresh_hz > 0 {
            let y = y_of(info.refresh_hz as f32, &plots[0], lo, hi);
            p.extend(Shape::dashed_line(
                &[pos2(plots[0].left(), y), pos2(plots[0].right(), y)],
                Stroke::new(1.0, pal().dim),
                4.0,
                4.0,
            ));
        }
        if !cmp.is_empty() {
            let pts: Vec<Pos2> =
                cmp.iter().map(|(t, v)| pos2(at(*t, &plots[0]), y_of(*v, &plots[0], lo, hi))).collect();
            p.extend(Shape::dashed_line(&pts, Stroke::new(1.4, pal().faint), 5.0, 4.0));
        }
        let pts: Vec<Pos2> =
            fps.iter().map(|(t, v)| pos2(at(*t, &plots[0]), y_of(*v, &plots[0], lo, hi))).collect();
        fill_under(&p, &pts, plots[0].bottom(), pal().cpu_line);
        p.add(Shape::line(pts, Stroke::new(1.6, pal().cpu_line)));

        // 2) frame time: the average per column, stutters as red marks (in a long
        // session only where they bunch up), the zone above the threshold
        let thr = self.stats.thr_ms;
        let ft_hi = (thr * 1.6).max(40.0).round();
        let note = if self.stats.mark_min > 1 {
            trf("красные отметки — где кадров дольше {v} мс больше обычного", &[("v", format!("{thr:.0}"))])
        } else {
            trf("красные отметки — кадры дольше {v} мс", &[("v", format!("{thr:.0}"))])
        };
        frame(&p, rects[1], &tr("Время кадра"), &note);
        grid(&p, plots[1], 0.0, ft_hi, 2, |v| {
            if v >= ft_hi { format!("{v:.0} {}", tr("мс")) } else { format!("{v:.0}") }
        });
        let zone = Rect::from_min_max(
            pos2(plots[1].left(), plots[1].top()),
            pos2(plots[1].right(), y_of(thr, &plots[1], 0.0, ft_hi)),
        );
        p.rect_filled(zone, 0.0, alpha(pal().hot, 26));
        let n = self.stats.cols.len().max(2);
        let col_x = |i: usize, plot: &Rect| plot.left() + plot.width() * i as f32 / (n - 1) as f32;
        let mut runs: Vec<Vec<Pos2>> = vec![Vec::new()];
        for (i, (_, avg, _)) in self.stats.cols.iter().enumerate() {
            if *avg > 0.0 {
                runs.last_mut().unwrap().push(pos2(col_x(i, &plots[1]), y_of(*avg, &plots[1], 0.0, ft_hi)));
            } else if !runs.last().unwrap().is_empty() {
                runs.push(Vec::new());
            }
        }
        for r in runs.into_iter().filter(|r| r.len() >= 2) {
            p.add(Shape::line(r, Stroke::new(1.4, pal().cpu_line)));
        }
        let mut last_x = f32::MIN;
        for (i, (mx, _, st)) in self.stats.cols.iter().enumerate() {
            if *st >= self.stats.mark_min {
                let x = col_x(i, &plots[1]);
                if x - last_x >= 2.0 {
                    let y = y_of(*mx, &plots[1], 0.0, ft_hi);
                    p.line_segment([pos2(x, plots[1].bottom()), pos2(x, y)], Stroke::new(1.5, pal().hot));
                    last_x = x;
                }
            }
        }

        // 3) load and 4) temperature: the GPU solid, the CPU dashed
        for (k, title, fg, fc, (lo, hi), unit) in [
            (
                2usize,
                tr("Нагрузка"),
                (|x: &pcm_core::session::Sample| x.gpu) as fn(&pcm_core::session::Sample) -> Option<f32>,
                (|x: &pcm_core::session::Sample| x.cpu) as fn(&pcm_core::session::Sample) -> Option<f32>,
                (0.0, 100.0),
                "%",
            ),
            (
                3usize,
                tr("Температура"),
                |x: &pcm_core::session::Sample| x.gpu_t,
                |x: &pcm_core::session::Sample| x.cpu_t,
                temp_range(info),
                "°C",
            ),
        ] {
            let note = if k == 3 && !self.stats.throttle_runs.is_empty() {
                tr("сплошная — видеокарта, пунктир — процессор, полосы — троттлинг")
            } else {
                tr("сплошная — видеокарта, пунктир — процессор")
            };
            frame(&p, rects[k], &title, &note);
            if k == 3 {
                // throttling: red from heat, yellow from the power plan
                for &(t0, t1, heat) in &self.stats.throttle_runs {
                    let (x0, x1) = (at(t0, &plots[k]), at(t1, &plots[k]).max(at(t0, &plots[k]) + 2.0));
                    let col = if heat { alpha(pal().hot, 56) } else { alpha(pal().warn, 44) };
                    p.rect_filled(Rect::from_x_y_ranges(x0..=x1, plots[k].y_range()), 0.0, col);
                }
            }
            grid(
                &p,
                plots[k],
                lo,
                hi,
                2,
                |v| if v >= hi { format!("{v:.0}{unit}") } else { format!("{v:.0}") },
            );
            let col = if k == 2 { GPU_C } else { GPU_T };
            let line = |f: fn(&pcm_core::session::Sample) -> Option<f32>| -> Vec<Pos2> {
                series(f, info)
                    .iter()
                    .map(|(t, v)| pos2(at(*t, &plots[k]), y_of(*v, &plots[k], lo, hi)))
                    .collect()
            };
            let c = line(fc);
            if c.len() >= 2 {
                p.extend(Shape::dashed_line(&c, Stroke::new(1.4, pal().gpu_line), 5.0, 4.0));
            }
            let g = line(fg);
            if g.len() >= 2 {
                p.add(Shape::line(g, Stroke::new(1.6, col)));
            }
        }

        // the time axis under the last chart: minutes, the end as mm:ss
        let last = plots[3];
        let y = rects[3].bottom() + 10.0;
        let total_min = span / 60.0;
        let step = [1.0f32, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0]
            .into_iter()
            .find(|s| total_min / s <= 6.0)
            .unwrap_or(60.0);
        let mut m = 0.0;
        while m * 60.0 < span * 0.93 {
            let x = at(m * 60.0, &last);
            let label = if m == 0.0 { tr("0 мин") } else { format!("{m:.0}") };
            p.text(pos2(x, y), Align2::CENTER_TOP, label, text_font(11.5), pal().faint);
            m += step;
        }
        let end = format!("{}:{:02}", (span as u64) / 60, (span as u64) % 60);
        p.text(pos2(last.right(), y), Align2::RIGHT_TOP, end, text_font(11.5), pal().faint);

        // the moment under the mouse on every chart, the values in a box on the one under it
        if let Some(f) = hover {
            let t = f * span;
            let sample = info.samples.iter().min_by(|a, b| (a.t - t).abs().total_cmp(&(b.t - t).abs()));
            for (k, plot) in plots.iter().enumerate() {
                let x = plot.left() + plot.width() * f;
                p.vline(x, plot.y_range(), Stroke::new(1.0, pal().dim));
                let Some(sm) = sample else { continue };
                let rows: Vec<(String, Color32)> = match k {
                    0 => sm.fps.map(|v| vec![(format!("{v:.0} FPS"), pal().cpu_line)]).unwrap_or_default(),
                    1 => {
                        let i = ((f * (n - 1) as f32).round() as usize).min(n - 1);
                        let (mx, avg, _) = self.stats.cols.get(i).copied().unwrap_or_default();
                        vec![
                            (format!("⌀ {} {}", dec(format!("{avg:.1}")), tr("мс")), pal().cpu_line),
                            (
                                format!("max {} {}", dec(format!("{mx:.1}")), tr("мс")),
                                if mx > thr { pal().hot } else { pal().dim },
                            ),
                        ]
                    }
                    2 => [
                        sm.gpu.map(|v| (format!("GPU {v:.0}%"), GPU_C)),
                        sm.cpu.map(|v| (format!("CPU {v:.0}%"), pal().gpu_line)),
                    ]
                    .into_iter()
                    .flatten()
                    .collect(),
                    _ => [
                        sm.gpu_t.map(|v| (format!("GPU {v:.0}°C"), GPU_T)),
                        sm.cpu_t.map(|v| (format!("CPU {v:.0}°C"), pal().gpu_line)),
                    ]
                    .into_iter()
                    .flatten()
                    .collect(),
                };
                if rows.is_empty() || !ui.ctx().pointer_hover_pos().is_some_and(|m| plot.contains(m)) {
                    continue;
                }
                let mut rows = rows;
                rows.insert(0, (format!("{}:{:02}", (t as u64) / 60, (t as u64) % 60), pal().dim));
                let bw = 124.0;
                let bh = 10.0 + rows.len() as f32 * 19.0;
                let bx = if x + 12.0 + bw > plot.right() { x - 12.0 - bw } else { x + 12.0 };
                let b = Rect::from_min_size(pos2(bx, plot.top() + 4.0), vec2(bw, bh));
                p.rect_filled(b, 8.0, pal().bg);
                p.rect_stroke(b, 8.0, Stroke::new(1.0, pal().border), StrokeKind::Inside);
                for (j, (t, c)) in rows.iter().enumerate() {
                    p.text(
                        pos2(b.left() + 10.0, b.top() + 6.0 + j as f32 * 19.0),
                        Align2::LEFT_TOP,
                        t,
                        text_font(12.0),
                        *c,
                    );
                }
            }
        }
        let _ = sc;
    }

    /// "Что ограничивает", "Система", "Заметка к сессии".
    fn side(&mut self, ui: &mut Ui, r: Rect, sc: &Scale) {
        let p = ui.painter().clone();
        let info = self.session.info.clone();
        let left = r.left() + 18.0;
        let inner = r.width() - 36.0;

        // what limits the FPS
        let (verdict, bars) = self.verdict(&info);
        let text_g = p.layout(verdict, text_font(12.5), pal().text, inner);
        let stut = match (self.stutter_line(), self.throttle_line()) {
            (Some(a), Some(b)) => Some(format!(
                "{a}

{b}"
            )),
            (a, b) => a.or(b),
        };
        let stut_g = stut.as_ref().map(|t| p.layout(t.clone(), text_font(12.5), pal().text, inner));
        let h1 = 50.0
            + bars.len() as f32 * 44.0
            + text_g.size().y
            + stut_g.as_ref().map_or(0.0, |g| g.size().y + 12.0)
            + 18.0;
        let c1 = Rect::from_min_size(r.min, vec2(r.width(), h1));
        panel(&p, c1);
        p.text(
            c1.left_top() + vec2(18.0, 16.0),
            Align2::LEFT_TOP,
            tr("Что ограничивает"),
            bold(15.0),
            pal().text,
        );
        let mut y = c1.top() + 48.0;
        for (name, (a, b)) in &bars {
            let col = rgb(sc.at(b / 100.0));
            p.text(pos2(left, y), Align2::LEFT_TOP, name, text_font(12.5), pal().dim);
            p.text(pos2(r.right() - 18.0, y), Align2::RIGHT_TOP, format!("{a:.0}–{b:.0}%"), bold(12.5), col);
            let track = Rect::from_min_size(pos2(left, y + 22.0), vec2(inner, 6.0));
            p.rect_filled(track, 3.0, pal().grid);
            p.rect_filled(
                Rect::from_min_size(track.min, vec2(inner * (b / 100.0).clamp(0.02, 1.0), 6.0)),
                3.0,
                col,
            );
            y += 44.0;
        }
        let gh = text_g.size().y;
        p.galley(pos2(left, y), text_g, pal().text);
        y += gh + 12.0;
        if let Some(g) = stut_g {
            p.galley(pos2(left, y), g, pal().text);
        }

        // the system
        let mut rows: Vec<(String, String)> = Vec::new();
        let mut add = |k: &str, v: String| {
            if !v.trim().is_empty() {
                rows.push((tr(k), v));
            }
        };
        add("Процессор", info.cpu.clone());
        add("Видеокарта", info.gpu.clone());
        add(
            "Память",
            if info.ram_gb > 0.0 {
                trf("{v} ГБ", &[("v", format!("{:.0}", info.ram_gb))])
            } else {
                String::new()
            },
        );
        add("Windows", info.windows.clone());
        add(
            "Монитор",
            if info.refresh_hz > 0 {
                trf("{v} Гц", &[("v", info.refresh_hz.to_string())])
            } else {
                String::new()
            },
        );
        add(
            "Окно игры",
            if info.width > 0 { format!("{} × {}", info.width, info.height) } else { String::new() },
        );
        let key_w = 98.0;
        let laid: Vec<_> = rows
            .iter()
            .map(|(k, v)| (k.clone(), p.layout(v.clone(), bold(12.5), pal().text, inner - key_w)))
            .collect();
        let h2 = 52.0 + laid.iter().map(|(_, g)| g.size().y + 8.0).sum::<f32>() + 8.0;
        let c2 = Rect::from_min_size(pos2(r.left(), c1.bottom() + GAP), vec2(r.width(), h2));
        panel(&p, c2);
        p.text(c2.left_top() + vec2(18.0, 16.0), Align2::LEFT_TOP, tr("Система"), bold(15.0), pal().text);
        let mut y = c2.top() + 48.0;
        for (k, g) in laid {
            let gh = g.size().y;
            p.text(pos2(left, y), Align2::LEFT_TOP, k, text_font(12.5), pal().dim);
            p.galley(pos2(left + key_w, y), g, pal().text);
            y += gh + 8.0;
        }

        // the note: settings only the game knows, to compare sessions later
        let c3 = Rect::from_min_size(pos2(r.left(), c2.bottom() + GAP), vec2(r.width(), 210.0));
        panel(&p, c3);
        p.text(
            c3.left_top() + vec2(18.0, 16.0),
            Align2::LEFT_TOP,
            tr("Заметка к сессии"),
            bold(15.0),
            pal().text,
        );
        let hint = p.layout(
            tr("Настройки графики знает только сама игра — запиши их здесь, чтобы потом сравнивать сессии."),
            text_font(12.0),
            pal().dim,
            inner,
        );
        let hh = hint.size().y;
        p.galley(pos2(left, c3.top() + 44.0), hint, pal().dim);
        let edit =
            Rect::from_min_max(pos2(left, c3.top() + 52.0 + hh), pos2(r.right() - 18.0, c3.bottom() - 16.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(edit));
        child.add_sized(
            edit.size(),
            egui::TextEdit::multiline(&mut self.note)
                .hint_text(tr("Например: 1440p, ультра, DLSS «Качество»")),
        );
    }

    /// The text of "Что ограничивает" and the load bars (GPU, CPU: 25th–75th percentile).
    fn verdict(&self, info: &SessionInfo) -> (String, Vec<LoadBar>) {
        let mut bars = Vec::new();
        if let Some(g) = self.stats.gpu_range {
            bars.push((tr("Видеокарта"), g));
        }
        if let Some(c) = self.stats.cpu_range {
            bars.push((tr("Процессор"), c));
        }
        let gpu = self.stats.gpu_range.map(|r| r.1);
        let cpu = self.stats.cpu_range.map(|r| r.1);
        let at_refresh = info.refresh_hz > 0 && info.summary.avg_fps >= info.refresh_hz as f64 * 0.95;
        let gpu_bound = if pcm_core::sensors::is_integrated(&info.gpu) { IGPU_BOUND } else { GPU_BOUND };
        let text = if at_refresh {
            tr(
                "FPS держится у частоты монитора: его ограничивает вертикальная синхронизация или лимит кадров, а не железо.",
            )
        } else if gpu.is_some_and(|g| g >= gpu_bound) {
            tr(
                "Видеокарта загружена почти всё время, процессор — меньше. FPS упирается в видеокарту: снизить разрешение или качество графики даст больше, чем что-то с процессором.",
            )
        } else if cpu.is_some_and(|c| c >= 80.0) {
            tr(
                "Процессор загружен сильнее видеокарты. FPS упирается в процессор: помогут настройки, которые его разгружают (дальность прорисовки, толпы, физика).",
            )
        } else if gpu.is_none() && cpu.is_none() {
            tr("Нагрузку во время этой сессии записать не удалось.")
        } else {
            tr(
                "Ни видеокарта, ни процессор не загружены полностью. FPS может ограничивать сама игра (лимит кадров), один поток процессора, память или диск.",
            )
        };
        (text, bars)
    }

    /// "Поделиться": the session as a picture (pcm_render::share) on the
    /// clipboard, for Ctrl+V in a chat, and in Pictures\PC Monitor.
    fn share(&mut self, sc: &Scale) {
        let mut fonts = pcm_render::text::Fonts::system();
        let Some(pm) = pcm_render::share::share_card(&mut fonts, 2.0, sc, &self.session) else { return };
        let Ok(png) = pm.encode_png() else { return };
        // the card is opaque: its premultiplied pixels are the straight ones
        let copied = pcm_win::clipboard::set_image(pm.data(), pm.width(), pm.height(), &png);
        let name = self.path.file_stem().map_or("session".into(), |n| n.to_string_lossy().into_owned());
        let saved = pcm_win::clipboard::pictures_dir().map(|d| d.join("PC Monitor")).and_then(|dir| {
            std::fs::create_dir_all(&dir).ok()?;
            let file = dir.join(format!("{name}.png"));
            std::fs::write(&file, &png).ok().map(|_| file)
        });
        self.share_note = Some(match (copied, saved) {
            (true, Some(_)) => (
                tr("Картинка скопирована: вставь в чат (Ctrl+V). Файл — в «Изображения\\PC Monitor»."),
                true,
                Instant::now(),
            ),
            (true, None) => (tr("Картинка скопирована: вставь в чат (Ctrl+V)."), true, Instant::now()),
            (false, Some(f)) => (
                trf(
                    "Не удалось скопировать; картинка сохранена: {file}",
                    &[("file", f.display().to_string())],
                ),
                false,
                Instant::now(),
            ),
            (false, None) => (tr("Не удалось сделать картинку."), false, Instant::now()),
        });
    }

    /// "Процессор перегревался и сбрасывал частоту: 3 мин 20 с, Windows урезала его до 70%."
    fn throttle_line(&self) -> Option<String> {
        let t = self.stats.throttle;
        let mut out = Vec::new();
        if t.heat_s > 0 {
            let time = long_time(t.heat_s as f64);
            out.push(match t.heat_limit {
                Some(l) => trf(
                    "Процессор перегревался и сбрасывал частоту: {time}, Windows урезала его до {l}%.",
                    &[("time", time), ("l", format!("{l:.0}"))],
                ),
                None => trf("Процессор перегревался и сбрасывал частоту: {time}.", &[("time", time)]),
            });
        }
        if t.power_s > 0 {
            out.push(trf(
                "Режим питания ограничивал процессор: {time} (батарея или экономия энергии).",
                &[("time", long_time(t.power_s as f64))],
            ));
        }
        (!out.is_empty()).then(|| out.join(" "))
    }

    /// "12 подтормаживаний длиннее 25 мс, 9 из них — в первые 2 минуты."
    fn stutter_line(&self) -> Option<String> {
        let n = self.stats.stutters.len();
        if n == 0 {
            return None;
        }
        let thr = format!("{:.0}", self.stats.thr_ms);
        let early = self.stats.stutters.iter().filter(|t| **t <= EARLY_S).count();
        let mut s = trf("{n} подтормаживаний длиннее {v} мс", &[("n", n.to_string()), ("v", thr)]);
        if early > 0 && n > 1 {
            s += &trf(", {k} из них — в первые 2 минуты", &[("k", early.to_string())]);
        }
        Some(s + ".")
    }
}

impl Drop for ReportPage {
    fn drop(&mut self) {
        self.save_note();
    }
}

/// The temperature chart's range: 40..90 °C, wider when the session was hotter or cooler.
fn temp_range(info: &SessionInfo) -> (f32, f32) {
    let all: Vec<f32> = info.samples.iter().flat_map(|s| [s.cpu_t, s.gpu_t]).flatten().collect();
    let lo = all.iter().copied().fold(40.0, f32::min);
    let hi = all.iter().copied().fold(90.0, f32::max);
    ((lo / 5.0).floor() * 5.0, (hi / 5.0).ceil() * 5.0)
}

/// "22.11 18:52" of "2026-11-22 18:52:34".
fn short_when(started: &str) -> String {
    match (started.get(5..7), started.get(8..10), started.get(11..16)) {
        (Some(m), Some(d), Some(t)) => format!("{d}.{m} {t}"),
        _ => started.to_string(),
    }
}

/// The local clock at the end: start + duration ("21:27").
fn end_clock(started: &str, duration_s: f64) -> String {
    let h: u64 = started.get(11..13).and_then(|v| v.parse().ok()).unwrap_or(0);
    let m: u64 = started.get(14..16).and_then(|v| v.parse().ok()).unwrap_or(0);
    let s: u64 = started.get(17..19).and_then(|v| v.parse().ok()).unwrap_or(0);
    let end = (h * 3600 + m * 60 + s + duration_s.max(0.0) as u64) % 86400;
    format!("{:02}:{:02}", end / 3600, end / 60 % 60)
}

/// A chart's card: the title on the left, a note on the right.
fn frame(p: &Painter, r: Rect, title: &str, note: &str) {
    panel(p, r);
    p.text(r.left_top() + vec2(18.0, 14.0), Align2::LEFT_TOP, title, bold(14.5), pal().text);
    if !note.is_empty() {
        p.text(r.right_top() + vec2(-18.0, 16.0), Align2::RIGHT_TOP, note, text_font(12.0), pal().dim);
    }
}

/// Horizontal lines at lo, the middle and hi with their labels.
fn grid(p: &Painter, plot: Rect, lo: f32, hi: f32, steps: usize, label: impl Fn(f32) -> String) {
    for k in 0..=steps {
        let v = lo + (hi - lo) * k as f32 / steps as f32;
        let y = plot.bottom() - plot.height() * k as f32 / steps as f32;
        p.hline(plot.x_range(), y, Stroke::new(1.0, pal().grid));
        p.text(pos2(plot.left() - 10.0, y), Align2::RIGHT_CENTER, label(v), text_font(11.0), pal().faint);
    }
}

/// The area under a line, fading down.
fn fill_under(p: &Painter, pts: &[Pos2], bottom: f32, color: Color32) {
    if pts.len() < 2 {
        return;
    }
    let mut mesh = Mesh::default();
    for (i, pt) in pts.iter().enumerate() {
        mesh.colored_vertex(*pt, alpha(color, 70));
        mesh.colored_vertex(pos2(pt.x, bottom), alpha(color, 8));
        if i > 0 {
            let b = (i * 2) as u32;
            mesh.add_triangle(b - 2, b - 1, b);
            mesh.add_triangle(b - 1, b + 1, b);
        }
    }
    p.add(Shape::mesh(mesh));
}

/// A button drawn right-aligned at `right`; returns its rect and whether it was clicked.
fn button(ui: &mut Ui, right: f32, y: f32, text: &str, accent: bool, danger: bool, id: &str) -> (Rect, bool) {
    let p = ui.painter().clone();
    let font = bold(13.0);
    let w = p.layout_no_wrap(text.to_string(), font.clone(), pal().text).size().x + 32.0;
    let r = Rect::from_min_size(pos2(right - w, y), vec2(w, 36.0));
    let resp = ui.interact(r, ui.scope_id().with(id), Sense::click());
    let (fill, fg) = if accent {
        (if resp.hovered() { alpha(pal().accent, 220) } else { pal().accent }, Color32::WHITE)
    } else {
        (if resp.hovered() { pal().card_hi } else { pal().card }, if danger { pal().hot } else { pal().text })
    };
    p.rect_filled(r, 9.0, fill);
    if !accent {
        p.rect_stroke(r, 9.0, Stroke::new(1.0, pal().border), StrokeKind::Inside);
    }
    p.text(r.center(), Align2::CENTER_CENTER, text, font, fg);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    (r, resp.clicked())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stutters_and_columns() {
        let mut s = Session::default();
        s.info.summary.median_ms = 7.0;
        s.info.duration_s = 10.0;
        // 7 ms frames, two 40 ms hitches, a 2 s pause (not a stutter)
        s.frame_ms = vec![7.0; 1000];
        s.frame_ms[10] = 40.0;
        s.frame_ms[500] = 40.0;
        s.frame_ms[700] = 2000.0;
        let st = stats(&s);
        assert_eq!(st.thr_ms, 25.0, "twice the median is less: 25 ms");
        assert_eq!(st.stutters.len(), 2);
        assert_eq!(st.cols.len(), FRAME_COLS);
        assert!(st.cols.iter().any(|c| c.0 == 40.0));
        assert!(st.cols.iter().all(|c| c.0 < 2000.0), "the pause is left out");
        assert_eq!(st.frames, 999);
        assert_eq!(st.mark_min, 1, "a short session marks every stutter");
        assert_eq!(st.cols.iter().map(|c| c.2).sum::<u32>(), 2);
        assert!((stutter_share(2679, 523_000) - 0.512).abs() < 0.001);
        assert_eq!(stutter_share(5, 0), 0.0);
    }

    #[test]
    fn times_and_numbers() {
        assert_eq!(end_clock("2026-11-22 21:04:10", 23.0 * 60.0 + 17.0), "21:27");
        assert_eq!(end_clock("2026-11-22 23:50:00", 3600.0), "00:50");
        assert_eq!(short_when("2026-11-22 18:52:34"), "22.11 18:52");
        assert_eq!(thousands(200940), "200\u{202F}940");
        assert_eq!(quartiles(vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]), Some((3.0, 7.0)));
    }
}

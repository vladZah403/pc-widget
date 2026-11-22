//! The "История" tab of the PC Monitor window: the history database read
//! (the widget writes it), charts for an hour, a day, a week or 30 days,
//! and the shown points saved as CSV.

use std::time::{Duration, Instant};

use eframe::egui;

use crate::dash_home::pal;
use egui::{RichText, Vec2};
use pcm_core::history::{Column, FILE, History, Point, STEP_S, buckets, to_csv};
use pcm_render::chart::{Chart, Series, chart, columns};
use pcm_render::format::fmt_speed;
use pcm_render::format::{tr, trf};
use pcm_render::text::Fonts;
use pcm_render::theme::Theme;

const RANGES: [(&str, i64); 4] =
    [("Час", 3600), ("Сутки", 86400), ("Неделя", 7 * 86400), ("30 дней", 30 * 86400)];
const CHART_H: f32 = 170.0;
/// The open tab reads the database again this often.
const RELOAD: Duration = Duration::from_secs(10);

#[derive(Default)]
pub struct HistoryTab {
    range: usize,
    db: Option<History>,
    points: Vec<Point>,
    /// (from, to) of the points, unix s.
    span: (i64, i64),
    loaded: Option<Instant>,
    load_seq: u64,
    textures: Vec<egui::TextureHandle>,
    /// What the textures show: range, width, scale, load, theme.
    drawn: Option<(usize, u32, u32, u64, String)>,
    note: Option<String>,
}

fn pct(v: f64) -> String {
    format!("{v:.0}%")
}

fn deg(v: f64) -> String {
    format!("{v:.0}°")
}

fn speed(v: f64) -> String {
    fmt_speed(v)
}

/// "13:05" for an hour or a day, "22.11" for longer.
fn time_label(t: i64, long: bool) -> String {
    let s = pcm_win::system::local_time_of(t); // "2026-11-22 13:05:41"
    if long {
        format!("{}.{}", s.get(8..10).unwrap_or(""), s.get(5..7).unwrap_or(""))
    } else {
        s.get(11..16).unwrap_or("").to_string()
    }
}

impl HistoryTab {
    fn load(&mut self) {
        if self.db.is_none() {
            let path = crate::widget_win::data_dir().join(FILE);
            match History::open_read(&path) {
                Ok(db) => self.db = Some(db),
                Err(_) => {
                    self.note = Some(tr("Истории пока нет: виджет записывает точку раз в 10 секунд."));
                    return;
                }
            }
        }
        let now = pcm_win::system::unix_now();
        let from = now - RANGES[self.range].1;
        if let Some(db) = &self.db {
            match db.range(from, now) {
                Ok(p) => {
                    self.points = p;
                    self.note = None;
                }
                Err(e) => self.note = Some(trf("История не читается: {e}", &[("e", e.to_string())])),
            }
        }
        self.span = (from, now);
        self.loaded = Some(Instant::now());
        self.load_seq += 1;
    }

    fn charts(&self, cols: usize, sc: &pcm_render::theme::Scale) -> Vec<Chart> {
        let (from, to) = self.span;
        let b = |c: Column| buckets(&self.points, from, to, cols, c);
        let long = RANGES[self.range].1 > 86400;
        let mid = from + (to - from) / 2;
        let times = [time_label(from, long), time_label(mid, long), time_label(to, long)];
        let s = |name: &str, color, c| Series { name: name.into(), color, values: b(c) };
        let (a, z) = (sc.at(0.05), sc.at(0.55));
        vec![
            Chart {
                title: tr("Процессор, %"),
                series: vec![s("CPU", a, Column::Cpu)],
                lo: 0.0,
                hi: Some(100.0),
                min_hi: 100.0,
                times: times.clone(),
                fmt: pct,
            },
            Chart {
                title: tr("Температура"),
                series: vec![s("CPU", a, Column::CpuTemp), s("GPU", z, Column::GpuTemp)],
                lo: 20.0,
                hi: None,
                min_hi: 60.0,
                times: times.clone(),
                fmt: deg,
            },
            Chart {
                title: tr("Видеокарта, %"),
                series: vec![s("GPU", z, Column::Gpu)],
                lo: 0.0,
                hi: Some(100.0),
                min_hi: 100.0,
                times: times.clone(),
                fmt: pct,
            },
            Chart {
                title: tr("Память, %"),
                series: vec![s("RAM", a, Column::Ram)],
                lo: 0.0,
                hi: Some(100.0),
                min_hi: 100.0,
                times: times.clone(),
                fmt: pct,
            },
            Chart {
                title: tr("Сеть"),
                series: vec![s("↓", a, Column::Down), s("↑", z, Column::Up)],
                lo: 0.0,
                hi: None,
                min_hi: 64.0 * 1024.0,
                times,
                fmt: speed,
            },
        ]
    }

    fn save_csv(&self) {
        let name = format!(
            "pc-monitor-history-{}.csv",
            pcm_win::system::local_time_of(pcm_win::system::unix_now()).get(..10).unwrap_or("")
        );
        let Some(path) = crate::report_win::save_dialog(std::ptr::null_mut(), &name) else { return };
        let _ = std::fs::write(path, to_csv(&self.points, false, pcm_win::system::local_time_of));
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, fonts: &mut Fonts, theme: Option<&Theme>, theme_key: &str) {
        if self.loaded.is_none_or(|t| t.elapsed() >= RELOAD) {
            self.load();
        }
        ui.horizontal(|ui| {
            for (i, (name, _)) in RANGES.iter().enumerate() {
                let sel = self.range == i;
                let text = RichText::new(tr(name)).size(13.0).color(if sel { pal().text } else { pal().dim });
                if ui.add(egui::Button::selectable(sel, text)).clicked() && !sel {
                    self.range = i;
                    self.load();
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let enabled = !self.points.is_empty();
                if ui.add_enabled(enabled, egui::Button::new(tr("Сохранить CSV"))).clicked() {
                    self.save_csv();
                }
                ui.label(
                    RichText::new(trf("точек: {n}", &[("n", self.points.len().to_string())]))
                        .size(12.0)
                        .color(pal().dim),
                );
            });
        });
        ui.add_space(10.0);
        if let Some(n) = &self.note {
            ui.label(RichText::new(n).color(pal().dim));
            return;
        }
        let Some(theme) = theme else { return };
        let ctx = ui.ctx().clone();
        let ppp = ctx.pixels_per_point();
        let w = (ui.available_width() - 16.0).max(300.0); // room for the scroll bar
        let key = (self.range, w.round() as u32, (ppp * 100.0) as u32, self.load_seq, theme_key.to_string());
        if self.drawn.as_ref() != Some(&key) {
            let sc = theme.scale();
            // never more columns than points, or the lines fall apart into dots
            let points = ((self.span.1 - self.span.0) / STEP_S).max(10) as usize;
            let charts = self.charts(columns(w, ppp).min(points), &sc);
            let mut textures = Vec::new();
            for (i, spec) in charts.iter().enumerate() {
                let Some(pm) = chart(fonts, ppp, w, CHART_H, spec) else { continue };
                let img = egui::ColorImage::from_rgba_premultiplied(
                    [pm.width() as usize, pm.height() as usize],
                    pm.data(),
                );
                match self.textures.get_mut(i) {
                    Some(t) => {
                        t.set(img, egui::TextureOptions::LINEAR);
                        textures.push(t.clone());
                    }
                    None => textures.push(ctx.load_texture(
                        format!("chart-{i}"),
                        img,
                        egui::TextureOptions::LINEAR,
                    )),
                }
            }
            self.textures = textures;
            self.drawn = Some(key);
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for t in &self.textures {
                ui.add(egui::Image::new((t.id(), Vec2::new(w, CHART_H))));
                ui.add_space(4.0);
            }
        });
        ctx.request_repaint_after(RELOAD);
    }
}

//! The "История" tab of the PC Monitor window: the history database read
//! (the widget writes it), charts for an hour, a day, a week or 30 days,
//! and the shown points saved as CSV.
//!
//! Beta 2: in the home page's style: the range as a segmented switch, tiles
//! with the period's numbers, charts painted with egui (the mouse shows
//! the values). Then the games of the period (the "Итоги недели" card
//! opens this page for the week).

use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Rect, RichText, Sense, pos2, vec2};
use pcm_core::history::{Column, FILE, History, Point, STEP_S, buckets, to_csv};
use pcm_render::format::{dec, fmt_speed, tr, trf};
use pcm_render::theme::Theme;

use crate::dash_home::{bold, fit, icon, pal, panel, rgb, segments, text_font};
use crate::dash_kit::{
    GAP, Series, Tile, TimeChart, button, empty_state, grid_cells, stat_tiles, time_chart,
};

const RANGES: [(&str, i64); 4] =
    [("Час", 3600), ("Сутки", 86400), ("Неделя", 7 * 86400), ("30 дней", 30 * 86400)];
/// The open tab reads the database again this often.
const RELOAD: Duration = Duration::from_secs(10);
/// Columns of a chart at most.
const COLS: usize = 240;

#[derive(Default)]
pub struct HistoryTab {
    range: usize,
    db: Option<History>,
    points: Vec<Point>,
    /// (from, to) of the points, unix s.
    span: (i64, i64),
    loaded: Option<Instant>,
    note: Option<String>,
    /// The game sessions' headers, read again only when the files change.
    sessions: Vec<pcm_core::session::SessionInfo>,
    session_files: Vec<std::path::PathBuf>,
    /// The test hook's scroll was looked at.
    scrolled: bool,
    /// Windows' events of the last 30 days (read on a thread, at most once a minute).
    events: std::sync::Arc<std::sync::Mutex<Events>>,
}

/// The event logs as last read.
#[derive(Default)]
struct Events {
    read: Option<Instant>,
    reading: bool,
    list: Vec<pcm_win::eventlog::Event>,
}

/// How often the event logs are read again.
const EVENTS_EVERY: Duration = Duration::from_secs(60);
/// Rows the events card shows at most.
const EVENT_ROWS: usize = 12;

/// An event as one line, and what it means (for the tooltip).
fn event_text(e: &pcm_win::eventlog::Event) -> (String, String, egui::Color32) {
    use pcm_win::eventlog::Kind;
    let d = e.detail.clone();
    match e.kind {
        Kind::Crash => (trf("Вылетела программа: {app}", &[("app", d)]), String::new(), pal().warn),
        Kind::Hang => (trf("Программа зависла и закрыта: {app}", &[("app", d)]), String::new(), pal().dim),
        Kind::BlueScreen => (
            trf("Синий экран, код {code}", &[("code", d)]),
            tr("Код помогает найти причину: поищи его в интернете вместе со словом BSOD."),
            pal().hot,
        ),
        Kind::PowerLoss => (
            tr("Внезапное выключение или перезагрузка"),
            tr(
                "Пропало питание, ПК завис или его выключили кнопкой. Если часто — проверь блок питания и температуры.",
            ),
            pal().hot,
        ),
        Kind::DriverReset => (
            trf("Сбой видеодрайвера {driver}, восстановлен", &[("driver", d)]),
            tr(
                "Видеокарта перестала отвечать на пару секунд. Если часто — обнови драйвер, проверь температуры и разгон.",
            ),
            pal().warn,
        ),
        Kind::Firmware => (
            tr("Прошивка ограничивала частоту процессора"),
            tr("Обычно из-за перегрева или питания: ноутбук на батарее или слабый блок питания."),
            pal().dim,
        ),
        Kind::Hardware => (
            tr("Аппаратная ошибка (WHEA)"),
            tr(
                "Процессор, память или шина сообщили об ошибке. Если повторяется — проверь разгон и температуры.",
            ),
            pal().hot,
        ),
    }
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

/// "22.11 13:05" for the mouse's box.
fn when_label(t: i64) -> String {
    let s = pcm_win::system::local_time_of(t);
    format!("{}.{} {}", s.get(8..10).unwrap_or(""), s.get(5..7).unwrap_or(""), s.get(11..16).unwrap_or(""))
}

/// "45 мин", "6 ч 10 мин"
fn play_time(s: f64) -> String {
    let m = (s / 60.0).round() as u64;
    if m < 60 {
        trf("{m} мин", &[("m", m.to_string())])
    } else {
        trf("{h} ч {m} мин", &[("h", (m / 60).to_string()), ("m", format!("{:02}", m % 60))])
    }
}

/// "1,2 ГБ" of bytes.
fn amount(bytes: f64) -> String {
    let gb = bytes / (1u64 << 30) as f64;
    if gb >= 1.0 {
        trf("{v} ГБ", &[("v", dec(format!("{gb:.1}")))])
    } else {
        trf("{v} МБ", &[("v", format!("{:.0}", bytes / 1048576.0))])
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
        self.load_sessions();
        self.load_events();
    }

    /// Read the event logs again on a thread, when it's time.
    fn load_events(&self) {
        let Ok(mut e) = self.events.lock() else { return };
        if e.reading || e.read.is_some_and(|t| t.elapsed() < EVENTS_EVERY) {
            return;
        }
        e.reading = true;
        let slot = self.events.clone();
        let spawned = std::thread::Builder::new().name("event logs".into()).spawn(move || {
            let list = pcm_win::eventlog::recent(30, 300);
            if let Ok(mut e) = slot.lock() {
                e.list = list;
                e.read = Some(Instant::now());
                e.reading = false;
            }
        });
        if spawned.is_err() {
            e.reading = false;
        }
    }

    /// "События Windows": the period's crashes, blue screens, power losses…
    fn events(&self, ui: &mut egui::Ui, w: f32) {
        let (list, read) = match self.events.lock() {
            Ok(e) => (
                e.list.iter().filter(|x| x.time >= self.span.0).cloned().collect::<Vec<_>>(),
                e.read.is_some(),
            ),
            Err(_) => return,
        };
        if !read {
            return;
        }
        let shown = &list[..list.len().min(EVENT_ROWS)];
        let row_h = 30.0;
        let more = list.len() > shown.len();
        let rows = shown.len().max(1) + usize::from(more);
        let (r, _) = ui.allocate_exact_size(vec2(w, 62.0 + rows as f32 * row_h), Sense::hover());
        let p = ui.painter().clone();
        panel(&p, r);
        p.text(
            r.left_top() + vec2(18.0, 16.0),
            egui::Align2::LEFT_TOP,
            tr("События Windows"),
            bold(15.0),
            pal().text,
        );
        let mut y = r.top() + 52.0;
        if shown.is_empty() {
            p.text(
                pos2(r.left() + 18.0, y),
                egui::Align2::LEFT_TOP,
                tr("Сбоев за этот период не было"),
                text_font(13.0),
                pal().good,
            );
            return;
        }
        let long = self.span.1 - self.span.0 > 86_400;
        for (i, e) in shown.iter().enumerate() {
            let (text, why, col) = event_text(e);
            let when = {
                let s = pcm_win::system::local_time_of(e.time);
                let clock = s.get(11..16).unwrap_or("").to_string();
                if long {
                    format!("{}.{} {clock}", s.get(8..10).unwrap_or(""), s.get(5..7).unwrap_or(""))
                } else {
                    clock
                }
            };
            let tw = p.layout_no_wrap(when.clone(), text_font(12.5), pal().dim).size().x;
            p.circle_filled(pos2(r.left() + 24.0, y + 9.0), 4.0, col);
            p.text(
                pos2(r.left() + 38.0, y),
                egui::Align2::LEFT_TOP,
                fit(&p, &text, &text_font(13.0), r.width() - 74.0 - tw),
                text_font(13.0),
                pal().text,
            );
            p.text(
                pos2(r.right() - 18.0, y + 1.0),
                egui::Align2::RIGHT_TOP,
                when,
                text_font(12.5),
                pal().dim,
            );
            if !why.is_empty() {
                let row =
                    egui::Rect::from_min_size(pos2(r.left() + 12.0, y - 6.0), vec2(r.width() - 24.0, row_h));
                ui.interact(row, ui.scope_id().with(("event", i)), Sense::hover()).on_hover_text(why);
            }
            y += row_h;
        }
        if more {
            let n = (list.len() - shown.len()).to_string();
            p.text(
                pos2(r.left() + 38.0, y),
                egui::Align2::LEFT_TOP,
                trf("и ещё {n}", &[("n", n)]),
                text_font(12.5),
                pal().dim,
            );
        }
    }

    fn load_sessions(&mut self) {
        let ext = pcm_core::session::EXTENSION;
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(crate::dash_sessions::dir())
            .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect())
            .unwrap_or_default();
        files.retain(|p| p.extension().is_some_and(|e| e == ext));
        files.sort();
        if files != self.session_files {
            // only the summaries are needed here
            self.sessions = files
                .iter()
                .filter_map(|p| pcm_core::session::Session::load_info(p).ok())
                .map(|i| pcm_core::session::thinned(i, 0))
                .collect();
            self.session_files = files;
        }
    }

    /// History over 30 days (test runs).
    pub fn show_month(&mut self) {
        self.range = RANGES.len() - 1;
    }

    /// "Итоги недели" was clicked: the week.
    pub fn show_week(&mut self) {
        self.range = RANGES.iter().position(|(_, s)| *s == 7 * 86400).unwrap_or(0);
        self.load();
    }

    /// The period's games: time, sessions and FPS of each, most played first.
    fn games(&self, ui: &mut egui::Ui, w: f32) {
        let from = pcm_win::system::local_time_of(self.span.0);
        let week = pcm_core::week::summarize(&[], pcm_core::week::since(&self.sessions, &from));
        if week.games.is_empty() {
            return;
        }
        let shown = &week.games[..week.games.len().min(6)];
        let row_h = 30.0;
        let (r, _) = ui.allocate_exact_size(vec2(w, 62.0 + shown.len() as f32 * row_h), Sense::hover());
        let p = ui.painter();
        panel(p, r);
        p.text(
            r.left_top() + vec2(18.0, 16.0),
            egui::Align2::LEFT_TOP,
            tr("Игры за период"),
            bold(15.0),
            pal().text,
        );
        p.text(
            pos2(r.right() - 18.0, r.top() + 18.0),
            egui::Align2::RIGHT_TOP,
            trf("всего {time}", &[("time", play_time(week.play_s))]),
            text_font(12.5),
            pal().dim,
        );
        let mut y = r.top() + 52.0;
        for g in shown {
            let n = g.sessions.to_string();
            let right = format!(
                "{} · {} · {}",
                play_time(g.play_s),
                trf("сессий: {n}", &[("n", n)]),
                trf("{v} FPS", &[("v", format!("{:.0}", g.avg_fps))])
            );
            let rw = p.layout_no_wrap(right.clone(), text_font(12.5), pal().text).size().x;
            p.text(
                pos2(r.left() + 18.0, y),
                egui::Align2::LEFT_TOP,
                fit(p, &g.name, &text_font(13.0), r.width() - 54.0 - rw),
                text_font(13.0),
                pal().text,
            );
            p.text(pos2(r.right() - 18.0, y), egui::Align2::RIGHT_TOP, right, text_font(12.5), pal().dim);
            y += row_h;
        }
        ui.add_space(GAP);
    }

    fn save_csv(&self) {
        let name = format!(
            "pc-monitor-history-{}.csv",
            pcm_win::system::local_time_of(pcm_win::system::unix_now()).get(..10).unwrap_or("")
        );
        let Some(path) = crate::report_win::save_dialog(std::ptr::null_mut(), &name) else { return };
        let _ = std::fs::write(path, to_csv(&self.points, false, pcm_win::system::local_time_of));
    }

    /// The period's numbers: average and peak load, the hottest CPU, memory, the traffic.
    fn tiles(&self, theme: &Theme) -> Vec<Tile> {
        let sc = theme.scale();
        let col = |c: Column| self.points.iter().filter_map(|p| p.get(c)).collect::<Vec<f64>>();
        let avg = |v: &[f64]| (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64);
        let max = |v: &[f64]| v.iter().copied().reduce(f64::max);
        let cpu = col(Column::Cpu);
        let ram = col(Column::Ram);
        let cpu_t = col(Column::CpuTemp);
        // no points → "—" like the other tiles (an empty f64 sum is -0.0, shown as "-0 МБ")
        let traffic = |c: Column| {
            let v = col(c);
            if v.is_empty() {
                "—".to_string()
            } else {
                amount(v.iter().sum::<f64>().max(0.0) * STEP_S as f64)
            }
        };
        let pct = |v: Option<f64>| {
            v.map_or(("—".to_string(), pal().text), |v| (format!("{v:.0}"), rgb(sc.at(v as f32 / 100.0))))
        };
        let (a, ac) = pct(avg(&cpu));
        let (m, mc) = pct(max(&cpu));
        let (r, rc) = pct(avg(&ram));
        let (t, tc) = max(&cpu_t)
            .map_or(("—".to_string(), pal().text), |t| (format!("{t:.0}"), rgb(sc.heat(t as f32))));
        vec![
            (tr("Процессор в среднем"), a, "%".into(), ac, None),
            (tr("Пик процессора"), m, "%".into(), mc, None),
            (tr("Пик температуры CPU"), t, "°C".into(), tc, None),
            (tr("Память в среднем"), r, "%".into(), rc, None),
            (tr("Скачано"), traffic(Column::Down), String::new(), pal().text, None),
            (tr("Отдано"), traffic(Column::Up), String::new(), pal().text, None),
        ]
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, theme: Option<&Theme>) {
        if self.loaded.is_none_or(|t| t.elapsed() >= RELOAD) {
            self.load();
        }
        let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]);
        // test runs: screenshots of the lower part (PCM_HISTORY_SCROLL=px), once
        if !self.scrolled {
            // until the events are in: they make the page longer
            self.scrolled = self.events.lock().is_ok_and(|e| e.read.is_some());
            if let Some(o) = std::env::var("PCM_HISTORY_SCROLL").ok().and_then(|v| v.parse().ok()) {
                area = area.vertical_scroll_offset(o);
            }
        }
        area.show(ui, |ui| {
            let w = ui.available_width();
            // the range on the left, the points and CSV on the right
            let (bar_r, _) = ui.allocate_exact_size(vec2(w, 38.0), Sense::hover());
            let names: Vec<String> = RANGES.iter().map(|(n, _)| tr(n)).collect();
            let seg_w: f32 = names
                .iter()
                .map(|n| {
                    ui.painter()
                        .layout_no_wrap(n.clone(), crate::dash_home::text_font(12.5), pal().text)
                        .size()
                        .x
                        + 22.0
                })
                .sum::<f32>()
                + 6.0;
            if let Some(i) =
                segments(ui, pos2(bar_r.left() + seg_w, bar_r.top() + 4.0), &names, self.range, "hist-range")
                && i != self.range
            {
                self.range = i;
                self.load();
            }
            let (b, clicked) =
                button(ui, bar_r.right(), bar_r.top(), &tr("Сохранить CSV"), false, false, "hist-csv");
            if clicked && !self.points.is_empty() {
                self.save_csv();
            }
            ui.painter().text(
                pos2(b.left() - 14.0, b.center().y),
                egui::Align2::RIGHT_CENTER,
                trf("точек: {n}", &[("n", self.points.len().to_string())]),
                crate::dash_home::text_font(12.5),
                pal().dim,
            );
            ui.add_space(14.0);
            if let Some(n) = &self.note {
                if self.db.is_none() {
                    // nothing recorded yet
                    let text = tr("Виджет записывает точку раз в 10 секунд: первые графики появятся через пару минут. Хранится 30 дней.");
                    empty_state(ui, icon::HISTORY, &tr("Истории пока нет"), &text, None, "hist-empty");
                } else {
                    ui.label(RichText::new(n).color(pal().dim));
                }
                return;
            }
            if self.points.is_empty() {
                let text = tr("Здесь появятся точки, пока PC Monitor работает. Выбери период подлиннее.");
                empty_state(ui, icon::HISTORY, &tr("За этот период точек нет"), &text, None, "hist-none");
                return;
            }
            let Some(theme) = theme else { return };
            stat_tiles(ui, w, &self.tiles(theme));
            ui.add_space(GAP);
            self.games(ui, w);
            let cols = if w >= 1000.0 { 2 } else { 1 };
            let cells = grid_cells(ui, w, 4, cols, 250.0);
            self.charts(ui, &cells);
            ui.add_space(GAP);
            self.events(ui, w);
            ui.add_space(8.0);
        });
        ui.ctx().request_repaint_after(RELOAD);
    }

    fn charts(&self, ui: &mut egui::Ui, cells: &[Rect]) {
        let (from, to) = self.span;
        // never more columns than points, or the lines fall apart into dots
        let n = (((to - from) / STEP_S).max(10) as usize).min(COLS);
        let b = |c: Column| -> Vec<Option<f32>> {
            buckets(&self.points, from, to, n, c).into_iter().map(|x| x.map(|(avg, _)| avg as f32)).collect()
        };
        let long = RANGES[self.range].1 > 86400;
        let times: Vec<String> = (0..5).map(|k| time_label(from + (to - from) * k / 4, long)).collect();
        let when = |i: usize| when_label(from + ((i as f64 + 0.5) * (to - from) as f64 / n as f64) as i64);
        let s = |name: &str, values, color, dashed, fill| Series {
            name: name.into(),
            values,
            color,
            dashed,
            fill,
        };
        let pct = |v: f32| format!("{v:.0}%");
        let deg = |v: f32| format!("{v:.0}°");
        let speed = |v: f32| fmt_speed(v as f64);
        // the temperature range from the data: 30..80 at least
        let temps: Vec<f32> =
            [b(Column::CpuTemp), b(Column::GpuTemp)].concat().into_iter().flatten().collect();
        let t_lo = temps.iter().copied().fold(30.0f32, f32::min);
        let t_hi = temps.iter().copied().fold(80.0f32, f32::max);
        let down = b(Column::Down);
        let up = b(Column::Up);
        let net_hi = down.iter().chain(&up).flatten().copied().fold(64.0 * 1024.0, f32::max) * 1.1;
        let charts = [
            TimeChart {
                title: tr("Процессор и видеокарта"),
                series: vec![
                    s("CPU", b(Column::Cpu), pal().cpu_line, false, true),
                    s("GPU", b(Column::Gpu), pal().gpu_line, true, false),
                ],
                lo: 0.0,
                hi: 100.0,
                fmt: &pct,
                times: times.clone(),
                when: &when,
            },
            TimeChart {
                title: tr("Температуры"),
                series: vec![
                    s("CPU", b(Column::CpuTemp), pal().warn, false, false),
                    s("GPU", b(Column::GpuTemp), pal().gpu_line, true, false),
                ],
                lo: (t_lo / 10.0).floor() * 10.0,
                hi: (t_hi / 10.0).ceil() * 10.0,
                fmt: &deg,
                times: times.clone(),
                when: &when,
            },
            TimeChart {
                title: tr("Память"),
                series: vec![s("RAM", b(Column::Ram), pal().gpu_line, false, true)],
                lo: 0.0,
                hi: 100.0,
                fmt: &pct,
                times: times.clone(),
                when: &when,
            },
            TimeChart {
                title: tr("Сеть"),
                series: vec![s("↓", down, pal().cpu_line, false, true), s("↑", up, pal().good, true, false)],
                lo: 0.0,
                hi: net_hi,
                fmt: &speed,
                times,
                when: &when,
            },
        ];
        for (i, (c, r)) in charts.iter().zip(cells).enumerate() {
            time_chart(ui, *r, c, &format!("hist-chart-{i}"));
        }
    }
}

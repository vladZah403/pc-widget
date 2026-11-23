//! The "Игры" tab of the PC Monitor window: the recorded sessions, newest
//! first; one opens as its report (dash_report: the concept's page with
//! compare, CSV, a note and "Удалить" after a second click).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::dash_home::{bold, fit, icon, icon_font, pal, panel, text_font};
use crate::dash_kit::{GAP, Tile, empty_state, sparkline, stat_tiles};
use crate::dash_report::{ReportAction, ReportPage};
use egui::{Align2, CursorIcon, Painter, Rect, RichText, Sense, Stroke, pos2, vec2};
use pcm_core::session::{EXTENSION, Session, SessionInfo};
use pcm_render::theme::Theme;

use pcm_render::format::{tr, trf};

const RELOAD: Duration = Duration::from_secs(10);
/// Samples a listed session keeps (its sparkline; the report reads them all).
const LIST_SAMPLES: usize = 160;

#[derive(Default)]
pub struct SessionsTab {
    list: Vec<(PathBuf, SessionInfo)>,
    listed: Option<Instant>,
    page: Option<ReportPage>,
    note: Option<String>,
}

/// What the tab asks of the window.
pub enum SessionsAction {
    /// "← Главная" on a report opened from the home page.
    Home,
    /// "Включить" on the empty page: game mode on.
    GameModeOn,
}

/// "42 мин", "1 ч 05 мин", "45 с"
fn play_time(s: f64) -> String {
    let m = (s / 60.0).round() as u64;
    if s < 60.0 {
        trf("{v} с", &[("v", format!("{s:.0}"))])
    } else if m < 60 {
        trf("{m} мин", &[("m", m.to_string())])
    } else {
        trf("{h} ч {m} мин", &[("h", (m / 60).to_string()), ("m", format!("{:02}", m % 60))])
    }
}

pub(crate) fn dir() -> PathBuf {
    crate::session_rec::sessions_dir(&crate::widget_win::data_dir())
}

impl SessionsTab {
    /// A report is open (it has its own header).
    pub fn report_open(&self) -> bool {
        self.page.is_some()
    }

    /// Show this session's report (the home page's "Открыть отчёт сессии").
    pub fn open_path(&mut self, p: &std::path::Path, from_home: bool) {
        match ReportPage::open(p) {
            Ok(mut page) => {
                page.from_home = from_home;
                self.page = Some(page);
                self.note = None;
            }
            Err(e) => {
                self.page = None;
                self.note = Some(trf(
                    "Не удалось прочитать {file}: {e}",
                    &[("file", p.display().to_string()), ("e", e.to_string())],
                ));
            }
        }
    }

    fn reload(&mut self) {
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir())
            .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect())
            .unwrap_or_default();
        files.retain(|p| p.extension().is_some_and(|e| e == EXTENSION));
        files.sort();
        files.reverse(); // names start with the date: newest first
        // keep what was read already; only new files are opened
        let old = std::mem::take(&mut self.list);
        for p in files {
            if let Some(found) = old.iter().find(|(q, _)| *q == p) {
                self.list.push(found.clone());
            } else if let Ok(info) = Session::load_info(&p) {
                // a list needs only a sparkline's worth of samples
                self.list.push((p, pcm_core::session::thinned(info, LIST_SAMPLES)));
            }
        }
        self.listed = Some(Instant::now());
    }

    /// `game_mode`: sessions are being recorded (the empty page says how).
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        theme: Option<&Theme>,
        game_mode: bool,
    ) -> Option<SessionsAction> {
        if self.listed.is_none_or(|t| t.elapsed() >= RELOAD) {
            self.reload();
        }
        if let Some(page) = &mut self.page {
            let Some(theme) = theme else {
                ui.label(RichText::new(tr("Жду цвета от виджета…")).color(pal().faint));
                return None;
            };
            let from_home = page.from_home;
            match page.ui(ui, &theme.scale(), &self.list) {
                Some(ReportAction::Back) => {
                    self.page = None;
                    return from_home.then_some(SessionsAction::Home);
                }
                Some(ReportAction::Deleted) => {
                    self.page = None;
                    self.listed = None; // read the folder again
                }
                None => {}
            }
            return None;
        }
        if self.list.is_empty() {
            let on = if game_mode {
                let text = tr(
                    "Запусти игру: когда она идёт больше 30 секунд, сессия запишется сама. Потом здесь будет отчёт: FPS, 1% low, нагрузка и температуры.",
                );
                empty_state(ui, icon::GAME, &tr("Игр пока нет"), &text, None, "games-empty")
            } else {
                let text = tr(
                    "Включи «Игровой режим», и каждая игра запишется сама: FPS, просадки, нагрузка и температуры по секундам.",
                );
                let label = tr("Включить игровой режим");
                empty_state(ui, icon::GAME, &tr("Запись игр выключена"), &text, Some(&label), "games-off")
            };
            return on.then_some(SessionsAction::GameModeOn);
        }
        let mut open: Option<PathBuf> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let w = ui.available_width();
            stat_tiles(ui, w, &summary_tiles(&self.list));
            ui.add_space(GAP);
            // one card, a row per session: a click opens its report
            let row_h = 64.0;
            let (card, _) =
                ui.allocate_exact_size(vec2(w, 12.0 + self.list.len() as f32 * row_h), Sense::hover());
            let p = ui.painter().clone();
            panel(&p, card);
            for (i, (path, info)) in self.list.iter().enumerate() {
                let r = Rect::from_min_size(
                    pos2(card.left(), card.top() + 6.0 + i as f32 * row_h),
                    vec2(w, row_h),
                );
                let resp = ui.interact(r, ui.scope_id().with(("session", i)), Sense::click());
                if resp.hovered() {
                    p.rect_filled(r.shrink2(vec2(6.0, 2.0)), 9.0, pal().card_hi);
                    ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                }
                if resp.clicked() {
                    open = Some(path.clone());
                }
                if i > 0 {
                    p.hline((r.left() + 18.0)..=(r.right() - 18.0), r.top(), Stroke::new(1.0, pal().grid));
                }
                session_row(&p, r, info);
            }
        });
        if let Some(p) = open {
            self.open_path(&p, false);
        }
        if let Some(n) = &self.note {
            ui.label(RichText::new(n).color(pal().hot));
        }
        ui.ctx().request_repaint_after(RELOAD);
        None
    }
}

/// Сессий, время в играх, чаще всего, последняя.
fn summary_tiles(list: &[(PathBuf, SessionInfo)]) -> Vec<Tile> {
    let total: f64 = list.iter().map(|(_, i)| i.summary.active_s).sum();
    let mut by_game: Vec<(String, f64)> = Vec::new();
    for (_, i) in list {
        let name = if i.title.trim().is_empty() { i.game.clone() } else { i.title.clone() };
        match by_game.iter_mut().find(|(n, _)| *n == name) {
            Some(g) => g.1 += i.summary.active_s,
            None => by_game.push((name, i.summary.active_s)),
        }
    }
    let top = by_game.into_iter().max_by(|a, b| a.1.total_cmp(&b.1)).map(|g| g.0).unwrap_or_default();
    let last = list.first().map(|(_, i)| short_date(&i.started)).unwrap_or_default();
    vec![
        (tr("Сессий"), list.len().to_string(), String::new(), pal().text, None),
        (tr("Время в играх"), play_time(total), String::new(), pal().text, None),
        (tr("Чаще всего"), top, String::new(), pal().text, None),
        (tr("Последняя"), last, String::new(), pal().text, None),
    ]
}

/// "22.11 18:52" of "2026-11-22 18:52:34".
fn short_date(started: &str) -> String {
    match (started.get(5..7), started.get(8..10), started.get(11..16)) {
        (Some(m), Some(d), Some(t)) => format!("{d}.{m} {t}"),
        _ => started.to_string(),
    }
}

/// A session's row: the icon, the game and when, its FPS line, the numbers, "Отчёт ›".
fn session_row(p: &Painter, r: Rect, info: &SessionInfo) {
    let s = &info.summary;
    let left = r.left() + 18.0;
    let ib = Rect::from_min_size(pos2(left, r.center().y - 17.0), vec2(34.0, 34.0));
    p.rect_filled(ib, 9.0, pal().card_hi);
    p.text(ib.center(), Align2::CENTER_CENTER, icon::GAME, icon_font(16.0), pal().dim);
    let name = if info.title.trim().is_empty() { info.game.clone() } else { info.title.clone() };
    let right = r.right() - 18.0;
    let link_w = 74.0;
    let nums_w = 190.0;
    let spark_w = (r.width() * 0.22).clamp(80.0, 220.0);
    let text_x = ib.right() + 14.0;
    let text_room = right - link_w - nums_w - spark_w - 24.0 - text_x;
    p.text(
        pos2(text_x, r.top() + 14.0),
        Align2::LEFT_TOP,
        fit(p, &name, &bold(14.0), text_room),
        bold(14.0),
        pal().text,
    );
    let sub = format!("{} · {}", short_date(&info.started), play_time(s.active_s));
    p.text(
        pos2(text_x, r.top() + 35.0),
        Align2::LEFT_TOP,
        fit(p, &sub, &text_font(12.5), text_room),
        text_font(12.5),
        pal().dim,
    );
    // the whole session's FPS, second by second
    let fps: Vec<f32> = info.samples.iter().filter_map(|x| x.fps).collect();
    let spark = Rect::from_min_size(
        pos2(right - link_w - nums_w - spark_w, r.center().y - 12.0),
        vec2(spark_w, 24.0),
    );
    sparkline(p, spark, &fps, pal().cpu_line);
    // "47 FPS" and "1% low 18"
    let nx = right - link_w - nums_w + 24.0;
    let v = p.text(
        pos2(nx, r.center().y),
        Align2::LEFT_CENTER,
        format!("{:.0}", s.avg_fps),
        bold(20.0),
        pal().text,
    );
    p.text(pos2(v.right() + 5.0, r.center().y + 2.0), Align2::LEFT_CENTER, "FPS", text_font(12.0), pal().dim);
    let lx = nx + 84.0;
    p.text(pos2(lx, r.center().y - 9.0), Align2::LEFT_CENTER, "1% low", text_font(11.5), pal().dim);
    p.text(
        pos2(lx, r.center().y + 9.0),
        Align2::LEFT_CENTER,
        format!("{:.0}", s.low1),
        bold(13.5),
        pal().text,
    );
    p.text(pos2(right, r.center().y), Align2::RIGHT_CENTER, tr("Отчёт ›"), text_font(13.0), pal().accent);
}

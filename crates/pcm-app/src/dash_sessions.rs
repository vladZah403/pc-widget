//! The "Игры" tab of the PC Monitor window: the recorded sessions, newest
//! first; one opens as its report (dash_report: the concept's page with
//! compare, CSV, a note and "Удалить" after a second click).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::dash_home::pal;
use crate::dash_report::{ReportAction, ReportPage};
use egui::RichText;
use pcm_core::session::{EXTENSION, Session, SessionInfo};
use pcm_render::theme::Theme;

use pcm_render::format::{tr, trf};

const RELOAD: Duration = Duration::from_secs(10);

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
                self.list.push((p, info));
            }
        }
        self.listed = Some(Instant::now());
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, theme: Option<&Theme>) -> Option<SessionsAction> {
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
            ui.label(
                RichText::new(
                    tr("Сессий пока нет. Они записываются сами, когда ты играешь (нужно «Считать FPS» в меню виджета) и \
                     игра идёт больше 30 секунд."),
                )
                .color(pal().dim),
            );
            return None;
        }
        let mut open: Option<PathBuf> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("sessions").striped(true).spacing([24.0, 8.0]).show(ui, |ui| {
                for h in [
                    tr("Когда"),
                    tr("Игра"),
                    tr("Время игры"),
                    tr("FPS в среднем"),
                    "1% low".into(),
                    String::new(),
                ] {
                    ui.label(RichText::new(h).size(12.0).color(pal().dim));
                }
                ui.end_row();
                for (p, info) in &self.list {
                    let s = &info.summary;
                    ui.label(
                        RichText::new(info.started.get(..16).unwrap_or(&info.started))
                            .size(13.0)
                            .color(pal().dim),
                    );
                    let name = if info.title.trim().is_empty() { &info.game } else { &info.title };
                    ui.label(RichText::new(name).size(13.0).color(pal().text));
                    ui.label(RichText::new(play_time(s.active_s)).size(13.0).color(pal().text));
                    ui.label(RichText::new(format!("{:.0}", s.avg_fps)).size(13.0).color(pal().text));
                    ui.label(RichText::new(format!("{:.0}", s.low1)).size(13.0).color(pal().text));
                    if ui.add(egui::Button::new(RichText::new(tr("Отчёт")).size(12.0)).small()).clicked()
                    {
                        open = Some(p.clone());
                    }
                    ui.end_row();
                }
            });
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

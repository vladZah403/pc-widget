//! The "Игровые сессии" tab of the PC Monitor window: the recorded sessions,
//! newest first; one opens as its report, with "Сохранить CSV" and
//! "Удалить" (after a second click).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::dash_home::pal;
use egui::{RichText, Vec2};
use pcm_core::session::{EXTENSION, Session, SessionInfo};
use pcm_render::report::{REPORT_H, REPORT_W, ReportHover, session_report};
use pcm_render::text::Fonts;
use pcm_render::theme::Theme;

use crate::report_win::{save_dialog, session_csv_name, write_session_csv};
use pcm_render::format::{tr, trf};

const RELOAD: Duration = Duration::from_secs(10);
const ARMED: Duration = Duration::from_secs(3);

struct Open {
    path: PathBuf,
    session: Session,
    texture: Option<egui::TextureHandle>,
    /// The scale and theme the texture was drawn for.
    drawn: Option<(u32, String)>,
}

#[derive(Default)]
pub struct SessionsTab {
    list: Vec<(PathBuf, SessionInfo)>,
    listed: Option<Instant>,
    open: Option<Open>,
    delete_armed: Option<Instant>,
    note: Option<String>,
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
    pub fn open_path(&mut self, p: &std::path::Path) {
        self.delete_armed = None;
        match Session::load(p) {
            Ok(session) => {
                self.open = Some(Open { path: p.to_path_buf(), session, texture: None, drawn: None });
                self.note = None;
            }
            Err(e) => {
                self.open = None;
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

    pub fn ui(&mut self, ui: &mut egui::Ui, fonts: &mut Fonts, theme: Option<&Theme>) {
        if self.open.is_some() {
            self.report(ui, fonts, theme);
            return;
        }
        if self.listed.is_none_or(|t| t.elapsed() >= RELOAD) {
            self.reload();
        }
        if self.list.is_empty() {
            ui.label(
                RichText::new(
                    tr("Сессий пока нет. Они записываются сами, когда ты играешь (нужно «Считать FPS» в меню виджета) и \
                     игра идёт больше 30 секунд."),
                )
                .color(pal().dim),
            );
            return;
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
            match Session::load(&p) {
                Ok(session) => self.open = Some(Open { path: p, session, texture: None, drawn: None }),
                Err(e) => {
                    self.note = Some(trf(
                        "Не удалось прочитать {file}: {e}",
                        &[("file", p.display().to_string()), ("e", e.to_string())],
                    ))
                }
            }
        }
        if let Some(n) = &self.note {
            ui.label(RichText::new(n).color(pal().hot));
        }
        ui.ctx().request_repaint_after(RELOAD);
    }

    fn report(&mut self, ui: &mut egui::Ui, fonts: &mut Fonts, theme: Option<&Theme>) {
        if self.delete_armed.is_some_and(|t| t.elapsed() > ARMED) {
            self.delete_armed = None;
        }
        let mut back = false;
        let mut deleted = false;
        let Some(o) = self.open.as_mut() else { return };
        ui.horizontal(|ui| {
            if ui.button(tr("← Все сессии")).clicked() {
                back = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let armed = self.delete_armed.is_some();
                let del = if armed {
                    RichText::new(tr("Точно удалить?")).color(pal().hot)
                } else {
                    RichText::new(tr("Удалить")).color(pal().dim)
                };
                if ui.button(del).clicked() {
                    if armed {
                        deleted = std::fs::remove_file(&o.path).is_ok();
                        self.delete_armed = None;
                    } else {
                        self.delete_armed = Some(Instant::now());
                    }
                }
                if ui.button(tr("Сохранить CSV")).clicked()
                    && let Some(path) = save_dialog(std::ptr::null_mut(), &session_csv_name(&o.session.info))
                {
                    let _ = write_session_csv(&o.session, &path);
                }
            });
        });
        ui.add_space(10.0);
        if let Some(theme) = theme {
            let ctx = ui.ctx().clone();
            let ppp = ctx.pixels_per_point();
            let key = ((ppp * 100.0) as u32, format!("{theme:?}{}", pcm_render::format::language()));
            if o.drawn.as_ref() != Some(&key)
                && let Some((pm, _)) =
                    session_report(fonts, ppp, &theme.scale(), &o.session, ReportHover::default())
            {
                let img = egui::ColorImage::from_rgba_premultiplied(
                    [pm.width() as usize, pm.height() as usize],
                    pm.data(),
                );
                o.texture = Some(ctx.load_texture("session-report", img, egui::TextureOptions::LINEAR));
                o.drawn = Some(key);
            }
            if let Some(t) = &o.texture {
                // as big as it is, smaller if the window is narrower
                let k = (ui.available_width() / REPORT_W).min(1.0);
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.add(egui::Image::new((t.id(), Vec2::new(REPORT_W * k, REPORT_H * k))));
                });
            }
        } else {
            ui.label(RichText::new(tr("Жду цвета от виджета…")).color(pal().faint));
        }
        if self.delete_armed.is_some() {
            ui.ctx().request_repaint_after(ARMED);
        }
        if back || deleted {
            self.open = None;
            self.delete_armed = None;
            if deleted {
                self.listed = None; // read the folder again
            }
        }
    }
}

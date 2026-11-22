//! The "Процессы" tab of the PC Monitor window: every program with its CPU,
//! memory and number of processes; "Закрыть" ends a program after a second
//! click (the widget does it, with the same rules as its own ×).

use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Color32, RichText};
use pcm_core::processes::can_close;
use pcm_render::format::fmt_mem;
use pcm_render::theme::Scale;
use pcm_render::widget::ProcRow;

use crate::dashboard::{FromDashboard, send};
use pcm_render::format::{tr, trf};

const DIM: Color32 = Color32::from_rgb(139, 147, 163);
const TEXT: Color32 = Color32::from_rgb(232, 234, 239);
const FAINT: Color32 = Color32::from_rgb(88, 96, 112);
const HOT: Color32 = Color32::from_rgb(232, 68, 90);
/// A first click on "Закрыть" waits this long for the second.
const ARMED: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Sort {
    #[default]
    Cpu,
    Memory,
    Name,
}

#[derive(Default)]
pub struct ProcsTab {
    sort: Sort,
    armed: Option<(String, Instant)>,
    /// Told the widget to sample processes.
    asked: bool,
}

fn rgb(c: (u8, u8, u8)) -> Color32 {
    Color32::from_rgb(c.0, c.1, c.2)
}

impl ProcsTab {
    /// The tab is shown (or not): the widget samples processes only meanwhile.
    pub fn visible(&mut self, on: bool) {
        if on != self.asked {
            self.asked = on;
            send(&FromDashboard::WantProcs(on));
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, rows: Option<&Vec<ProcRow>>, sc: Option<&Scale>) {
        let Some(rows) = rows else {
            ui.label(RichText::new(tr("Собираю список программ…")).color(DIM));
            return;
        };
        if self.armed.as_ref().is_some_and(|(_, t)| t.elapsed() > ARMED) {
            self.armed = None;
        }
        let mut rows = rows.clone();
        match self.sort {
            Sort::Cpu => rows.sort_by(|a, b| b.cpu.total_cmp(&a.cpu).then(b.memory.cmp(&a.memory))),
            Sort::Memory => rows.sort_by_key(|r| std::cmp::Reverse(r.memory)),
            Sort::Name => rows.sort_by_key(|r| r.name.to_lowercase()),
        }
        let total_mem: u64 = rows.iter().map(|r| r.memory).sum();
        ui.label(
            RichText::new(trf(
                "Программ: {n}  ·  их память: {mem}",
                &[("n", rows.len().to_string()), ("mem", fmt_mem(total_mem))],
            ))
            .size(12.0)
            .color(DIM),
        );
        ui.add_space(8.0);
        let header = |ui: &mut egui::Ui, text: &str, sort: Sort, me: &mut Sort| {
            let on = *me == sort;
            let t = RichText::new(if on { format!("{text} ▾") } else { text.to_string() })
                .size(12.0)
                .color(if on { TEXT } else { DIM });
            if ui.add(egui::Label::new(t).sense(egui::Sense::click())).clicked() {
                *me = sort;
            }
        };
        let mut end: Option<String> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("procs").striped(true).min_col_width(70.0).spacing([24.0, 6.0]).show(ui, |ui| {
                header(ui, &tr("Программа"), Sort::Name, &mut self.sort);
                header(ui, "CPU", Sort::Cpu, &mut self.sort);
                header(ui, &tr("Память"), Sort::Memory, &mut self.sort);
                ui.label(RichText::new(tr("Процессов")).size(12.0).color(DIM));
                ui.label("");
                ui.end_row();
                for r in &rows {
                    ui.label(RichText::new(&r.name).size(13.0).color(TEXT));
                    let cpu_col = sc.map_or(TEXT, |s| rgb(s.at((r.cpu / 50.0).min(1.0))));
                    let cpu = format!("{:.1}%", r.cpu).replace('.', ",");
                    ui.label(RichText::new(cpu).size(13.0).color(if r.cpu < 0.05 { FAINT } else { cpu_col }));
                    ui.label(RichText::new(fmt_mem(r.memory)).size(13.0).color(TEXT));
                    ui.label(RichText::new(r.count.to_string()).size(13.0).color(DIM));
                    if can_close(&r.name) {
                        let armed = self.armed.as_ref().is_some_and(|(n, _)| *n == r.name);
                        let text = if armed {
                            RichText::new(tr("Точно закрыть?")).size(12.0).color(HOT)
                        } else {
                            RichText::new(tr("Закрыть")).size(12.0).color(DIM)
                        };
                        if ui.add(egui::Button::new(text).small()).clicked() {
                            if armed {
                                end = Some(r.name.clone());
                            } else {
                                self.armed = Some((r.name.clone(), Instant::now()));
                            }
                        }
                    } else {
                        ui.label(RichText::new(tr("системная")).size(11.0).color(FAINT));
                    }
                    ui.end_row();
                }
            });
        });
        if let Some(name) = end {
            self.armed = None;
            send(&FromDashboard::EndProgram(name));
        }
        if self.armed.is_some() {
            ui.ctx().request_repaint_after(ARMED); // the button goes back by itself
        }
    }
}

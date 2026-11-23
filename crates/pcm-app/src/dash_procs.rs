//! The "Процессы" tab of the PC Monitor window: every program with its CPU,
//! memory and number of processes; "Закрыть" ends a program after a second
//! click (the widget does it, with the same rules as its own ×).
//!
//! Beta 2: in the home page's style: tiles, a search, a table card with
//! sortable columns and bars.

use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Align2, CursorIcon, Rect, RichText, Sense, Stroke, pos2, vec2};
use pcm_core::processes::can_close;
use pcm_render::format::{dec, fmt_mem, fmt_speed, tr};
use pcm_render::theme::Scale;
use pcm_render::widget::ProcRow;

use crate::dash_home::{bold, fit, pal, panel, rgb, text_font};
use crate::dash_kit::{GAP, Tile, bar, button_at, stat_tiles};
use crate::dashboard::{FromDashboard, send};

/// A first click on "Закрыть" waits this long for the second.
const ARMED: Duration = Duration::from_secs(3);
const ROW_H: f32 = 42.0;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Sort {
    #[default]
    Cpu,
    Memory,
    Net,
    Count,
    Name,
}

#[derive(Default)]
pub struct ProcsTab {
    sort: Sort,
    armed: Option<(String, Instant)>,
    /// Told the widget to sample processes.
    asked: bool,
    search: String,
    /// Ctrl+F: the search gets the keyboard on the next frame.
    pub focus_search: bool,
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
            ui.label(RichText::new(tr("Собираю список программ…")).color(pal().dim));
            return;
        };
        if self.armed.as_ref().is_some_and(|(_, t)| t.elapsed() > ARMED) {
            self.armed = None;
        }
        let mut rows = rows.clone();
        match self.sort {
            Sort::Cpu => rows.sort_by(|a, b| b.cpu.total_cmp(&a.cpu).then(b.memory.cmp(&a.memory))),
            Sort::Memory => rows.sort_by_key(|r| std::cmp::Reverse(r.memory)),
            Sort::Net => rows.sort_by(|a, b| b.net.unwrap_or(0.0).total_cmp(&a.net.unwrap_or(0.0))),
            Sort::Count => rows.sort_by_key(|r| std::cmp::Reverse(r.count)),
            Sort::Name => rows.sort_by_key(|r| r.name.to_lowercase()),
        }
        let total_mem: u64 = rows.iter().map(|r| r.memory).sum();
        let total_cpu: f32 = rows.iter().map(|r| r.cpu).sum();
        let total_count: u32 = rows.iter().map(|r| r.count).sum();
        let max_mem = rows.iter().map(|r| r.memory).max().unwrap_or(1).max(1);
        let needle = self.search.trim().to_lowercase();
        if !needle.is_empty() {
            rows.retain(|r| r.name.to_lowercase().contains(&needle));
        }
        let mut end: Option<String> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let w = ui.available_width();
            let tiles: Vec<Tile> = vec![
                (tr("Программ"), rows.len().to_string(), String::new(), pal().text, None),
                (tr("Процессов"), total_count.to_string(), String::new(), pal().text, None),
                (tr("Их память"), fmt_mem(total_mem), String::new(), pal().text, None),
                (
                    tr("Процессор сейчас"),
                    format!("{}%", dec(format!("{total_cpu:.1}"))),
                    String::new(),
                    sc.map_or(pal().text, |s| rgb(s.at(total_cpu / 100.0))),
                    None,
                ),
            ];
            stat_tiles(ui, w, &tiles);
            ui.add_space(GAP);
            // the search
            ui.horizontal(|ui| {
                let field = ui.add_sized(
                    vec2(280.0, 30.0),
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text(tr("Найти программу (Ctrl+F)"))
                        .margin(vec2(10.0, 7.0)),
                );
                if std::mem::take(&mut self.focus_search) {
                    field.request_focus();
                }
                // Esc in the search empties it
                if field.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.search.clear();
                }
                if !self.search.is_empty() && ui.button("✕").clicked() {
                    self.search.clear();
                }
            });
            ui.add_space(12.0);
            // the table: a header with the sort, then a row a program
            let (card, _) = ui
                .allocate_exact_size(vec2(w, 48.0 + rows.len().max(1) as f32 * ROW_H + 8.0), Sense::hover());
            let p = ui.painter().clone();
            panel(&p, card);
            let left = card.left() + 18.0;
            let right = card.right() - 18.0;
            let btn_w = 128.0;
            let count_x = right - btn_w - 20.0; // right edge of "Процессов"
            // the network column when there's room for it
            let show_net = card.width() >= 860.0;
            let net_x = count_x - 200.0; // left edge of the network column
            let mem_x = if show_net { net_x - 190.0 } else { count_x - 250.0 }; // left edge of the memory column
            let cpu_x = mem_x - 180.0; // left edge of the CPU column
            // traffic needs admin rights: without them the column says how to get it
            let net_known = rows.iter().any(|r| r.net.is_some());
            let max_net = rows.iter().filter_map(|r| r.net).fold(64.0 * 1024.0, f64::max);
            let head_y = card.top() + 16.0;
            for (text, sort, x, align) in [
                (tr("Программа"), Sort::Name, left, Align2::LEFT_TOP),
                ("CPU".to_string(), Sort::Cpu, cpu_x, Align2::LEFT_TOP),
                (tr("Память"), Sort::Memory, mem_x, Align2::LEFT_TOP),
                (tr("Сеть"), Sort::Net, net_x, Align2::LEFT_TOP),
                (tr("Процессов"), Sort::Count, count_x, Align2::RIGHT_TOP),
            ] {
                if sort == Sort::Net && !show_net {
                    continue;
                }
                let on = self.sort == sort;
                let label = if on { format!("{text} ▾") } else { text };
                let font = if on { bold(12.5) } else { text_font(12.5) };
                let r = p.text(pos2(x, head_y), align, label, font, if on { pal().text } else { pal().dim });
                let resp =
                    ui.interact(r.expand(4.0), ui.scope_id().with(("sort", sort as u8)), Sense::click());
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                }
                let resp = if sort == Sort::Net && !net_known {
                    resp.on_hover_text(tr(
                        "Трафик программ Windows даёт только с правами администратора: Настройки → Запуск и закрытие",
                    ))
                } else {
                    resp
                };
                if resp.clicked() {
                    self.sort = sort;
                }
            }
            p.hline(left..=right, card.top() + 42.0, Stroke::new(1.0, pal().border));
            if rows.is_empty() {
                p.text(
                    pos2(left, card.top() + 58.0),
                    Align2::LEFT_TOP,
                    tr("Ничего не найдено"),
                    text_font(13.0),
                    pal().dim,
                );
            }
            for (i, r) in rows.iter().enumerate() {
                let top = card.top() + 46.0 + i as f32 * ROW_H;
                let row = Rect::from_min_size(pos2(card.left() + 6.0, top), vec2(card.width() - 12.0, ROW_H));
                if ui.rect_contains_pointer(row) {
                    p.rect_filled(row, 8.0, pal().card_hi);
                }
                let cy = row.center().y;
                let name = fit(&p, &r.name, &bold(13.5), cpu_x - left - 16.0);
                p.text(pos2(left, cy), Align2::LEFT_CENTER, name, bold(13.5), pal().text);
                // CPU: the number and a bar (half the PC = full)
                let cpu_col = if r.cpu < 0.05 {
                    pal().faint
                } else {
                    sc.map_or(pal().text, |s| rgb(s.at((r.cpu / 50.0).min(1.0))))
                };
                p.text(
                    pos2(cpu_x, cy),
                    Align2::LEFT_CENTER,
                    format!("{}%", dec(format!("{:.1}", r.cpu))),
                    bold(13.0),
                    cpu_col,
                );
                bar(
                    &p,
                    Rect::from_min_size(pos2(cpu_x + 56.0, cy - 2.5), vec2(90.0, 5.0)),
                    r.cpu / 50.0,
                    cpu_col,
                );
                // memory: against the biggest program
                p.text(pos2(mem_x, cy), Align2::LEFT_CENTER, fmt_mem(r.memory), text_font(13.0), pal().text);
                bar(
                    &p,
                    Rect::from_min_size(pos2(mem_x + 72.0, cy - 2.5), vec2(90.0, 5.0)),
                    r.memory as f32 / max_mem as f32,
                    pal().gpu_line,
                );
                // network: received + sent, against the busiest program
                match r.net.filter(|_| show_net) {
                    Some(bps) => {
                        let col = if bps < 1024.0 { pal().faint } else { pal().text };
                        p.text(pos2(net_x, cy), Align2::LEFT_CENTER, fmt_speed(bps), text_font(13.0), col);
                        bar(
                            &p,
                            Rect::from_min_size(pos2(net_x + 84.0, cy - 2.5), vec2(80.0, 5.0)),
                            (bps / max_net) as f32,
                            pal().cpu_line,
                        );
                    }
                    None if show_net => {
                        p.text(pos2(net_x, cy), Align2::LEFT_CENTER, "—", text_font(13.0), pal().faint);
                    }
                    None => {}
                }
                p.text(
                    pos2(count_x, cy),
                    Align2::RIGHT_CENTER,
                    r.count.to_string(),
                    text_font(13.0),
                    pal().dim,
                );
                let b = Rect::from_min_size(pos2(right - btn_w, cy - 14.0), vec2(btn_w, 28.0));
                if can_close(&r.name) {
                    let armed = self.armed.as_ref().is_some_and(|(n, _)| *n == r.name);
                    let text = if armed { tr("Точно закрыть?") } else { tr("Закрыть") };
                    if button_at(ui, b, &text, false, armed, &format!("end-{}", r.name)) {
                        if armed {
                            end = Some(r.name.clone());
                        } else {
                            self.armed = Some((r.name.clone(), Instant::now()));
                        }
                    }
                } else {
                    p.text(b.center(), Align2::CENTER_CENTER, tr("системная"), text_font(12.0), pal().faint);
                }
            }
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

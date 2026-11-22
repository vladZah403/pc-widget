//! The "Настройки" tab of the PC Monitor window: 1.8's settings window in
//! one scrolling page. The tab edits a copy of the widget's settings and
//! sends it back whole after each change; the widget takes what can be set
//! here and applies it (and feeds the result back a second later).

use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Color32, RichText};
use pcm_core::config::{self, Config, FPS_SIZES, SECTIONS, TRAY_KINDS, TRAY_MAX};

use crate::dashboard::{FromDashboard, send};

const DIM: Color32 = Color32::from_rgb(139, 147, 163);
const TEXT: Color32 = Color32::from_rgb(232, 234, 239);
const FAINT: Color32 = Color32::from_rgb(88, 96, 112);
const HOT: Color32 = Color32::from_rgb(232, 68, 90);
/// After a change here, the widget's copy (a feed behind) is not taken for this long.
const SETTLE: Duration = Duration::from_secs(3);
const PING_PRESETS: [(&str, &str); 3] =
    [("1.1.1.1", "Cloudflare"), ("8.8.8.8", "Google"), ("9.9.9.9", "Quad9")];

#[derive(Default)]
pub struct SettingsTab {
    edit: Option<Config>,
    changed: Option<Instant>,
    host_text: String,
    host_bad: bool,
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(14.0);
    ui.label(RichText::new(text).size(15.0).color(TEXT).strong());
    ui.add_space(4.0);
}

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(FAINT));
}

fn rgb_of(hex: &str) -> [u8; 3] {
    pcm_render::theme::hex(hex).map_or([128, 128, 128], |c| [c.0, c.1, c.2])
}

impl SettingsTab {
    pub fn ui(&mut self, ui: &mut egui::Ui, from_widget: Option<&Config>) {
        // the widget's settings, unless ours are newer
        if self.changed.is_none_or(|t| t.elapsed() > SETTLE)
            && let Some(c) = from_widget
        {
            if self.edit.as_ref() != Some(c) && self.host_text.is_empty() {
                self.host_text = if PING_PRESETS.iter().any(|(h, _)| *h == c.ping_host) {
                    String::new()
                } else {
                    c.ping_host.clone()
                };
            }
            self.edit = Some(c.clone());
        }
        let Some(mut c) = self.edit.clone() else {
            ui.label(RichText::new("Жду настройки от виджета…").color(DIM));
            return;
        };
        let before = c.clone();
        let mut action: Option<FromDashboard> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.set_max_width(640.0);

            ui.spacing_mut().slider_width = 240.0;
            heading(ui, "Виджет");
            ui.checkbox(&mut c.widget, "Показывать виджет (Ctrl+Alt+W)");
            ui.checkbox(&mut c.pinned, "Закрепить на рабочем столе");
            ui.add_enabled(!c.pinned, egui::Checkbox::new(&mut c.topmost, "Поверх всех окон"));
            ui.checkbox(&mut c.compact, "Компактный режим");
            ui.horizontal(|ui| {
                ui.label("Размер");
                let mut size = c.size as f32;
                let s = egui::Slider::new(&mut size, config::SIZE_MIN as f32..=config::SIZE_MAX as f32)
                    .step_by(config::SIZE_STEP as f64)
                    .fixed_decimals(0)
                    .suffix("%");
                if ui.add(s).changed() {
                    c.size = size as u32;
                }
                if ui.button("100%").clicked() {
                    c.size = 100;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Непрозрачность");
                let mut a = (c.alpha * 100.0).round();
                if ui
                    .add(egui::Slider::new(&mut a, 30.0..=100.0).step_by(5.0).fixed_decimals(0).suffix("%"))
                    .changed()
                {
                    c.alpha = a / 100.0;
                }
            });
            ui.horizontal(|ui| {
                if ui.button("Обычная форма").clicked() {
                    action = Some(FromDashboard::ResetShape);
                }
                if ui.button("В правый верхний угол").clicked() {
                    action = Some(FromDashboard::ResetPosition);
                }
            });
            hint(
                ui,
                "Размер — ещё Ctrl + колёсико над виджетом. Форма — тяни виджет за правый или нижний край.",
            );

            heading(ui, "Цвета");
            ui.horizontal(|ui| {
                for (key, name) in [("heat", "Тепло"), ("neon", "Неон"), ("ice", "Лёд"), ("custom", "Свои")]
                {
                    ui.radio_value(&mut c.theme, key.to_string(), name);
                }
            });
            if c.theme == "custom" {
                ui.horizontal(|ui| {
                    for (i, label) in ["Малая нагрузка", "Большая нагрузка"].iter().enumerate()
                    {
                        ui.label(*label);
                        let mut rgb = rgb_of(&c.custom_colors[i]);
                        if ui.color_edit_button_srgb(&mut rgb).changed() {
                            c.custom_colors[i] = format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
                        }
                        ui.add_space(12.0);
                    }
                });
            }

            heading(ui, "Показывать в виджете");
            egui::Grid::new("sections").num_columns(2).spacing([24.0, 4.0]).show(ui, |ui| {
                for (i, (key, name)) in SECTIONS.iter().enumerate() {
                    let mut on = !c.hidden.iter().any(|h| h == key);
                    if ui.checkbox(&mut on, *name).changed() {
                        if on {
                            c.hidden.retain(|h| h != key);
                        } else {
                            c.hidden.push(key.to_string());
                        }
                    }
                    if i % 2 == 1 {
                        ui.end_row();
                    }
                }
            });

            heading(ui, "Трей и панель задач");
            ui.label(RichText::new(format!("Значки в трее (1–{TRAY_MAX}):")).color(DIM));
            ui.horizontal_wrapped(|ui| {
                for (key, name) in TRAY_KINDS {
                    let mut on = c.tray.iter().any(|k| k == key);
                    let full = !on && c.tray.len() >= TRAY_MAX;
                    let last = on && c.tray.len() == 1;
                    if ui.add_enabled(!(full || last), egui::Checkbox::new(&mut on, *name)).changed() {
                        if on {
                            c.tray.push(key.to_string());
                        } else {
                            c.tray.retain(|k| k != key);
                        }
                    }
                }
            });
            let mut panel = c.taskbar.unwrap_or_else(|| !pcm_win::system::is_windows_11());
            if ui.checkbox(&mut panel, "Мини-панель на панели задач").changed() {
                c.taskbar = Some(panel);
            }

            heading(ui, "Игры");
            ui.label(RichText::new("В играх и видео на весь экран виджет:").color(DIM));
            ui.horizontal(|ui| {
                for (key, name) in [("hide", "прятать"), ("dim", "тускнеть"), ("off", "ничего")]
                {
                    ui.radio_value(&mut c.in_games, key.to_string(), name);
                }
            });
            ui.checkbox(&mut c.fps, "Считать FPS (панель, запись сессий)");
            ui.add_enabled_ui(c.fps, |ui| {
                ui.checkbox(&mut c.fps_panel, "Показывать FPS-панель (Ctrl+Alt+F)");
                ui.horizontal(|ui| {
                    ui.label("Угол панели");
                    for (key, name) in [
                        ("tl", "слева вверху"),
                        ("tr", "справа вверху"),
                        ("bl", "слева внизу"),
                        ("br", "справа внизу"),
                    ] {
                        ui.radio_value(&mut c.fps_corner, key.to_string(), name);
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Размер панели");
                    for s in FPS_SIZES {
                        ui.radio_value(&mut c.fps_size, *s, format!("{s}%"));
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label("Профиль");
                ui.radio_value(&mut c.profile, "work".to_string(), "Работа");
                ui.radio_value(&mut c.profile, "game".to_string(), "Игра");
                if ui.button("Запомнить текущий вид").clicked() {
                    action = Some(FromDashboard::SaveProfile);
                }
            });
            ui.checkbox(&mut c.auto_game, "Включать «Игру», когда запущена игра");

            heading(ui, "Предупреждения");
            ui.checkbox(&mut c.alerts, "Показывать предупреждения");
            ui.checkbox(&mut c.quiet_games, "Во время игры — показать после неё");
            ui.add_enabled_ui(c.alerts, |ui| {
                let limits: [(&str, &mut u32, [u32; 4], &str); 4] = [
                    ("Перегрев CPU от", &mut c.alert_temp, [80, 85, 90, 95], "°"),
                    ("Память занята от", &mut c.alert_ram, [80, 85, 90, 95], "%"),
                    ("Батарея ниже", &mut c.alert_battery, [10, 15, 20, 30], "%"),
                    ("Свободно на диске C: меньше", &mut c.alert_disk, [5, 10, 15, 20], "%"),
                ];
                for (title, value, choices, unit) in limits {
                    ui.horizontal(|ui| {
                        ui.label(title);
                        for v in choices {
                            ui.radio_value(value, v, format!("{v}{unit}"));
                        }
                    });
                }
            });

            heading(ui, "Пинг");
            ui.horizontal(|ui| {
                for (host, name) in PING_PRESETS {
                    if ui.radio(c.ping_host == host, format!("{host} ({name})")).clicked() {
                        c.ping_host = host.to_string();
                        self.host_text.clear();
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("Свой адрес:");
                let r = ui.add(egui::TextEdit::singleline(&mut self.host_text).desired_width(200.0));
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.button("OK").clicked() || enter {
                    match config::clean_host(&self.host_text) {
                        Some(h) => {
                            c.ping_host = h.clone();
                            self.host_text = h;
                            self.host_bad = false;
                        }
                        None => self.host_bad = true,
                    }
                }
            });
            if self.host_bad {
                ui.label(
                    RichText::new("Это не похоже на адрес (пример: eu.battle.net или 8.8.4.4).").color(HOT),
                );
            }

            ui.add_space(18.0);
            hint(
                ui,
                "Автозапуск, права администратора и обновления появятся вместе с установщиком (этап 6).",
            );
            ui.add_space(12.0);
        });
        if c != before {
            c.sanitize();
            self.changed = Some(Instant::now());
            send(&FromDashboard::SetConfig(Box::new(c.clone())));
        }
        self.edit = Some(c);
        if let Some(a) = action {
            send(&a);
        }
    }
}

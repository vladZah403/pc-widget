//! The "Настройки" tab of the PC Monitor window: 1.8's settings window in
//! one scrolling page. The tab edits a copy of the widget's settings and
//! sends it back whole after each change; the widget takes what can be set
//! here and applies it (and feeds the result back a second later).

use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Color32, RichText};
use pcm_core::config::{self, Config, FPS_SIZES, SECTIONS, TRAY_KINDS, TRAY_MAX};

use crate::dashboard::{FromDashboard, StartInfo, send};
use pcm_render::format::{tr, trf};
use pcm_render::widget::{Data, UpdateState};

const DIM: Color32 = Color32::from_rgb(139, 147, 163);
const TEXT: Color32 = Color32::from_rgb(232, 234, 239);
const FAINT: Color32 = Color32::from_rgb(88, 96, 112);
const HOT: Color32 = Color32::from_rgb(232, 68, 90);
const ACCENT: Color32 = Color32::from_rgb(38, 140, 130);
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
    /// `--scroll N` (screenshots of the lower sections): start scrolled down.
    pub start_offset: Option<f32>,
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
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        from_widget: Option<&Config>,
        start: Option<&StartInfo>,
        data: Option<&Data>,
    ) {
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
            ui.label(RichText::new(tr("Жду настройки от виджета…")).color(DIM));
            return;
        };
        let before = c.clone();
        let mut action: Option<FromDashboard> = None;
        let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]);
        if let Some(o) = self.start_offset.take() {
            area = area.vertical_scroll_offset(o);
        }
        area.show(ui, |ui| {
            ui.set_max_width(640.0);

            ui.spacing_mut().slider_width = 240.0;
            heading(ui, "Язык / Language");
            ui.horizontal_wrapped(|ui| {
                ui.radio_value(&mut c.lang, "auto".to_string(), tr("Как в Windows"));
                for (code, name) in pcm_render::format::LANGS {
                    ui.radio_value(&mut c.lang, code.to_string(), name);
                }
            });
            heading(ui, &tr("Виджет"));
            ui.checkbox(&mut c.widget, tr("Показывать виджет (Ctrl+Alt+W)"));
            ui.checkbox(&mut c.pinned, tr("Закрепить на рабочем столе"));
            ui.add_enabled(!c.pinned, egui::Checkbox::new(&mut c.topmost, tr("Поверх всех окон")));
            ui.checkbox(&mut c.compact, tr("Компактный режим"));
            ui.horizontal(|ui| {
                ui.label(tr("Размер"));
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
                ui.label(tr("Непрозрачность"));
                let mut a = (c.alpha * 100.0).round();
                if ui
                    .add(egui::Slider::new(&mut a, 30.0..=100.0).step_by(5.0).fixed_decimals(0).suffix("%"))
                    .changed()
                {
                    c.alpha = a / 100.0;
                }
            });
            ui.horizontal(|ui| {
                if ui.button(tr("Обычная форма")).clicked() {
                    action = Some(FromDashboard::ResetShape);
                }
                if ui.button(tr("В правый верхний угол")).clicked() {
                    action = Some(FromDashboard::ResetPosition);
                }
            });
            hint(
                ui,
                &tr("Размер — ещё Ctrl + колёсико над виджетом. Форма — тяни виджет за правый или нижний край."),
            );

            heading(ui, &tr("Цвета"));
            ui.horizontal(|ui| {
                for (key, name) in [("heat", tr("Тепло")), ("neon", tr("Неон")), ("ice", tr("Лёд")), ("custom", tr("Свои"))]
                {
                    ui.radio_value(&mut c.theme, key.to_string(), name);
                }
            });
            if c.theme == "custom" {
                ui.horizontal(|ui| {
                    for (i, label) in [tr("Малая нагрузка"), tr("Большая нагрузка")].iter().enumerate()
                    {
                        ui.label(label.as_str());
                        let mut rgb = rgb_of(&c.custom_colors[i]);
                        if ui.color_edit_button_srgb(&mut rgb).changed() {
                            c.custom_colors[i] = format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
                        }
                        ui.add_space(12.0);
                    }
                });
            }

            heading(ui, &tr("Показывать в виджете"));
            egui::Grid::new("sections").num_columns(2).spacing([24.0, 4.0]).show(ui, |ui| {
                for (i, (key, name)) in SECTIONS.iter().enumerate() {
                    let mut on = !c.hidden.iter().any(|h| h == key);
                    if ui.checkbox(&mut on, tr(name)).changed() {
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

            heading(ui, &tr("Трей и панель задач"));
            ui.label(RichText::new(trf("Значки в трее (1–{n}):", &[("n", TRAY_MAX.to_string())])).color(DIM));
            ui.horizontal_wrapped(|ui| {
                for (key, name) in TRAY_KINDS {
                    let mut on = c.tray.iter().any(|k| k == key);
                    let full = !on && c.tray.len() >= TRAY_MAX;
                    let last = on && c.tray.len() == 1;
                    if ui.add_enabled(!(full || last), egui::Checkbox::new(&mut on, tr(name))).changed() {
                        if on {
                            c.tray.push(key.to_string());
                        } else {
                            c.tray.retain(|k| k != key);
                        }
                    }
                }
            });
            let mut panel = c.taskbar.unwrap_or_else(|| !pcm_win::system::is_windows_11());
            if ui.checkbox(&mut panel, tr("Мини-панель на панели задач")).changed() {
                c.taskbar = Some(panel);
            }

            heading(ui, &tr("Игры"));
            ui.label(RichText::new(tr("В играх и видео на весь экран виджет:")).color(DIM));
            ui.horizontal(|ui| {
                for (key, name) in [("hide", tr("прятать")), ("dim", tr("тускнеть")), ("off", tr("ничего"))]
                {
                    ui.radio_value(&mut c.in_games, key.to_string(), name);
                }
            });
            ui.checkbox(&mut c.fps, tr("Считать FPS (панель, запись сессий)"));
            ui.add_enabled_ui(c.fps, |ui| {
                ui.checkbox(&mut c.fps_panel, tr("Показывать FPS-панель (Ctrl+Alt+F)"));
                ui.horizontal(|ui| {
                    ui.label(tr("Угол панели"));
                    for (key, name) in [
                        ("tl", tr("слева вверху")),
                        ("tr", tr("справа вверху")),
                        ("bl", tr("слева внизу")),
                        ("br", tr("справа внизу")),
                    ] {
                        ui.radio_value(&mut c.fps_corner, key.to_string(), name);
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(tr("Размер панели"));
                    for s in FPS_SIZES {
                        ui.radio_value(&mut c.fps_size, *s, format!("{s}%"));
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label(tr("Профиль"));
                ui.radio_value(&mut c.profile, "work".to_string(), tr("Работа"));
                ui.radio_value(&mut c.profile, "game".to_string(), tr("Игра"));
                if ui.button(tr("Запомнить текущий вид")).clicked() {
                    action = Some(FromDashboard::SaveProfile);
                }
            });
            ui.checkbox(&mut c.auto_game, tr("Включать «Игру», когда запущена игра"));

            heading(ui, &tr("Предупреждения"));
            ui.checkbox(&mut c.alerts, tr("Показывать предупреждения"));
            ui.checkbox(&mut c.quiet_games, tr("Во время игры — показать после неё"));
            ui.add_enabled_ui(c.alerts, |ui| {
                let limits: [(&str, &mut u32, [u32; 4], &str); 4] = [
                    (&tr("Перегрев CPU от"), &mut c.alert_temp, [80, 85, 90, 95], "°"),
                    (&tr("Память занята от"), &mut c.alert_ram, [80, 85, 90, 95], "%"),
                    (&tr("Батарея ниже"), &mut c.alert_battery, [10, 15, 20, 30], "%"),
                    (&tr("Свободно на диске C: меньше"), &mut c.alert_disk, [5, 10, 15, 20], "%"),
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

            heading(ui, &tr("Пинг"));
            ui.horizontal(|ui| {
                for (host, name) in PING_PRESETS {
                    if ui.radio(c.ping_host == host, format!("{host} ({name})")).clicked() {
                        c.ping_host = host.to_string();
                        self.host_text.clear();
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label(tr("Свой адрес:"));
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
                    RichText::new(tr("Это не похоже на адрес (пример: eu.battle.net или 8.8.4.4).")).color(HOT),
                );
            }

            heading(ui, &tr("Закрытие окна"));
            ui.radio_value(
                &mut c.close_quits,
                false,
                tr("Крестик закрывает только окно: виджет, значки в трее и мини-панель работают дальше"),
            );
            ui.radio_value(&mut c.close_quits, true, tr("Крестик полностью закрывает программу"));
            hint(ui, &tr("Выйти совсем можно через «Выход» в меню значка в трее."));

            heading(ui, &tr("Запуск"));
            if let Some(st) = start {
                let mut auto = st.autostart;
                if ui.checkbox(&mut auto, tr("Запускать вместе с Windows")).changed() {
                    action = Some(FromDashboard::SetAutostart(auto));
                }
                if st.helper {
                    let mut admin = c.run_as_admin;
                    if ui.checkbox(&mut admin, tr("Права администратора (температуры)")).changed() {
                        c.run_as_admin = admin;
                        // the widget asks UAC: its prompt may come up in front of this window
                        allow_foreground();
                        action = Some(FromDashboard::SetAdmin(admin));
                    }
                    hint(ui, &tr("Нужны для температур и напряжений CPU. Windows будет спрашивать разрешение при запуске, кроме автозапуска: он идёт без вопроса из копии в Program Files."));
                    hint(
                        ui,
                        &tr(if st.admin {
                            "Сейчас работает с правами администратора."
                        } else {
                            "Сейчас работает без прав администратора."
                        }),
                    );
                }
            }
            heading(ui, &tr("Обновления"));
            ui.checkbox(&mut c.check_updates, tr("Проверять обновления"));
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("PC Monitor {}", crate::APP_VERSION)).color(DIM));
                if ui.button(tr("Проверить сейчас")).clicked() {
                    action = Some(FromDashboard::CheckUpdates);
                }
            });
            if let Some(d) = data
                && let Some(v) = &d.update
            {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(trf("Вышла версия {v}", &[("v", v.clone())])).color(TEXT).strong());
                    match &d.update_state {
                        Some(UpdateState::Download(f)) => {
                            let text = trf("скачиваю {p}%", &[("p", format!("{:.0}", f * 100.0))]);
                            ui.add(egui::ProgressBar::new(*f).desired_width(220.0).fill(ACCENT).text(text));
                        }
                        Some(UpdateState::Install) => {
                            ui.label(tr("устанавливаю…"));
                        }
                        None => {
                            let label = if d.update_auto { tr("Обновить") } else { tr("Открыть страницу загрузки") };
                            if ui.button(label).clicked() {
                                action = Some(FromDashboard::StartUpdate);
                            }
                        }
                    }
                });
            }
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

/// Any process may take the foreground from us now (the widget's UAC prompt).
fn allow_foreground() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};
    unsafe { AllowSetForegroundWindow(ASFW_ANY) };
}

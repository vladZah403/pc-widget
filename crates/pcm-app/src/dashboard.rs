//! The PC Monitor window (stage 5): `PC-Monitor.exe --dashboard`, a child
//! process of the widget. The widget feeds it a line of JSON a second on
//! its stdin; closing the window ends the process, so its memory (egui,
//! OpenGL) goes back to Windows instead of staying in the widget.
//!
//! Beta 2 (Vlad's concept "Главный экран"): a top bar (name, profile, game
//! mode), pages on the left, a status line at the bottom; "Главная" is
//! dash_home, the old card overview is "Оборудование".

use std::io::BufRead;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui;
use egui::{
    Align, Align2, Color32, CornerRadius, CursorIcon, FontData, FontDefinitions, FontFamily, Frame, Layout,
    Margin, RichText, Sense, Stroke, StrokeKind, pos2, vec2,
};
use pcm_render::text::Fonts;
use pcm_render::theme::{Theme, hex};
use pcm_render::widget::Data;
use serde::{Deserialize, Serialize};

use crate::dash_history::HistoryTab;
use crate::dash_home::{HomeAction, HomeInput, HomeTab, bold, icon, icon_font, pal, text_font};
use crate::dash_procs::ProcsTab;
use crate::dash_sessions::SessionsTab;
use crate::dash_settings::SettingsTab;
use crate::session_rec::LiveGame;
use pcm_render::format::{dec, fmt_mem, tr};

/// What the widget sends, one per line.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ToDashboard {
    /// The latest readings and the colours to draw them in.
    Feed {
        data: Box<Data>,
        theme: String,
        custom: [String; 2],
        cfg: Box<pcm_core::config::Config>,
        #[serde(default)]
        start: StartInfo,
        /// The game session being recorded, if any.
        #[serde(default)]
        game: Option<Box<LiveGame>>,
    },
    /// "Открыть PC Monitor" again: come to the front.
    Focus,
    /// A game session's report (the "Игровая сессия записана" card, the
    /// menu's "Отчёт о последней игре"): come to the front on its page.
    OpenSession(std::path::PathBuf),
    /// "Итоги недели" was clicked: History for the week.
    HistoryWeek,
}

/// How PC Monitor starts (not in the settings file: Windows keeps it).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StartInfo {
    /// It starts with Windows (the Run key or the admin task).
    pub autostart: bool,
    /// This copy runs with admin rights.
    pub admin: bool,
    /// The sensor helper is there (admin rights are only for it).
    pub helper: bool,
}

/// What the window asks the widget, one per line on its stdout.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum FromDashboard {
    /// The process tab is open (or closed): sample every program.
    WantProcs(bool),
    /// End this program (all its processes), as the widget's × does.
    EndProgram(String),
    /// The settings as the window left them (the widget takes what can be set there).
    SetConfig(Box<pcm_core::config::Config>),
    /// The widget's standard shape / back to the top-right corner / remember the look.
    ResetShape,
    ResetPosition,
    SaveProfile,
    /// "Запускать вместе с Windows".
    SetAutostart(bool),
    /// "Права администратора (температуры)": may restart the widget elevated.
    SetAdmin(bool),
    /// "Проверить сейчас" / "Обновить" in the settings.
    CheckUpdates,
    StartUpdate,
    /// The window was closed by the user (×, Alt+F4): the widget quits too
    /// if the settings say so.
    Closed,
}

/// Ask the widget something.
pub fn send(msg: &FromDashboard) {
    use std::io::Write;
    if let Ok(line) = serde_json::to_string(msg) {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{line}").and_then(|_| out.flush());
    }
}

/// The pages, in the order of the list on the left (`--tab N`).
pub const TAB_HOME: usize = 0;
pub const TAB_HISTORY: usize = 1;
pub const TAB_GAMES: usize = 2;
pub const TAB_PROCS: usize = 3;
pub const TAB_DEVICES: usize = 4;
pub const TAB_SETTINGS: usize = 5;
/// `--settings` opens this one.
pub const SETTINGS_TAB: usize = TAB_SETTINGS;
const TABS: [(&str, &str); 6] = [
    ("Главная", icon::HOME),
    ("История", icon::HISTORY),
    ("Игры", icon::GAME),
    ("Процессы", icon::LIST),
    ("Оборудование", icon::DEVICES),
    ("Настройки", icon::SETTINGS),
];
const NAV_W: f32 = 200.0;
const BAR_H: f32 = 56.0;
const STATUS_H: f32 = 30.0;

#[derive(Default)]
struct Feed {
    data: Option<Data>,
    theme: Option<Theme>,
    cfg: Option<pcm_core::config::Config>,
    start: Option<StartInfo>,
    game: Option<LiveGame>,
    /// The theme's name and colours, to know when charts need drawing again.
    theme_key: String,
    /// The theme, its colours and the language, without the light/dark look.
    theme_name: String,
    /// Bumped with every new feed: the cards are drawn again.
    seq: u64,
    /// When the last feed came, "14:32:18".
    when: String,
    focus: bool,
    /// A session to show the report of (ToDashboard::OpenSession).
    open_session: Option<std::path::PathBuf>,
    /// Show History for the week (ToDashboard::HistoryWeek).
    history_week: bool,
    /// The widget has gone: the window closes without asking it to quit.
    gone: bool,
}

/// Segoe UI, its semibold, Segoe UI Symbol and the Segoe MDL2 Assets icons
/// from the Windows fonts folder.
fn fonts() -> FontDefinitions {
    let mut defs = FontDefinitions::empty();
    let dir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    for (name, file) in [
        ("ui", "segoeui.ttf"),
        ("ui-semibold", "seguisb.ttf"),
        ("symbols", "seguisym.ttf"),
        ("mdl2", "segmdl2.ttf"),
    ] {
        if let Ok(data) = std::fs::read(format!(r"{dir}\Fonts\{file}")) {
            defs.font_data.insert(name.into(), Arc::new(FontData::from_owned(data)));
        }
    }
    let have = |n: &str| defs.font_data.contains_key(n).then(|| n.to_string());
    let ui: Vec<String> = [have("ui"), have("symbols")].into_iter().flatten().collect();
    defs.families.insert(FontFamily::Proportional, ui.clone());
    defs.families.insert(FontFamily::Monospace, ui.clone());
    let bold: Vec<String> =
        [have("ui-semibold"), have("ui"), have("symbols")].into_iter().flatten().collect();
    defs.families.insert(FontFamily::Name("semibold".into()), bold);
    // the family must exist even without the font (egui panics on an unknown one)
    let icons: Vec<String> = [have("mdl2"), have("symbols"), have("ui")].into_iter().flatten().collect();
    defs.families.insert(FontFamily::Name("icons".into()), icons);
    defs
}

fn theme_of(name: &str, custom: &[String; 2]) -> Theme {
    match name {
        "custom" => {
            Theme::Custom(hex(&custom[0]).unwrap_or((46, 196, 182)), hex(&custom[1]).unwrap_or((232, 68, 90)))
        }
        n => Theme::from_name(n),
    }
}

struct Dashboard {
    feed: Arc<Mutex<Feed>>,
    tab: usize,
    fonts: Fonts,
    home: HomeTab,
    history: HistoryTab,
    procs: ProcsTab,
    sessions: SessionsTab,
    settings: SettingsTab,
    /// The window's own CPU and memory, read with each feed.
    own: pcm_win::processes::OwnUsage,
    own_read: (u64, Option<f32>, Option<u64>),
    /// The look in use: Some(light) once applied (the title bar too).
    look: Option<bool>,
    /// Closing the window quits PC Monitor (not in the offscreen test runs).
    quit_on_close: bool,
    /// `--screenshot FILE`: save the window's own pixels once fed, then close.
    shot: Option<(String, Option<std::time::Instant>)>,
}

impl Dashboard {
    fn new(cc: &eframe::CreationContext, feed: Arc<Mutex<Feed>>) -> Dashboard {
        cc.egui_ctx.set_fonts(fonts());
        apply_look(&cc.egui_ctx, false);
        Dashboard {
            feed,
            tab: TAB_HOME,
            fonts: Fonts::system(),
            home: HomeTab::default(),
            history: HistoryTab::default(),
            procs: ProcsTab::default(),
            sessions: SessionsTab::default(),
            settings: SettingsTab::default(),
            own: Default::default(),
            own_read: (0, None, None),
            look: None,
            quit_on_close: false,
            shot: None,
        }
    }

    /// Dark or light: the palette, egui's widgets, the title bar, and the
    /// pictures drawn by pcm-render (they are drawn again).
    fn set_look(&mut self, ctx: &egui::Context, light: bool) {
        pcm_render::theme::set_light(light);
        apply_look(ctx, light);
        let system = if light { egui::SystemTheme::Light } else { egui::SystemTheme::Dark };
        ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(system));
        self.look = Some(light);
        if let Ok(mut f) = self.feed.lock() {
            f.theme_key = theme_key(&f.theme_name, light);
        }
        ctx.request_repaint();
    }

    /// The settings with one thing changed, sent to the widget and kept
    /// here until its next feed says the same.
    fn change_config(&self, edit: impl FnOnce(&mut pcm_core::config::Config)) {
        let mut f = self.feed.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = f.cfg.as_mut() {
            edit(c);
            send(&FromDashboard::SetConfig(Box::new(c.clone())));
        }
    }

    /// Name and version on the left; the profile and game mode on the right.
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let r = ui.max_rect();
        let p = ui.painter().clone();
        let y = r.center().y;
        let x = r.left() + 22.0;
        p.text(pos2(x, y), Align2::LEFT_CENTER, icon::LOGO, icon_font(20.0), pal().accent);
        let name = p.text(pos2(x + 32.0, y), Align2::LEFT_CENTER, "PC Monitor", bold(16.0), pal().text);
        let version = crate::APP_VERSION.split('.').take(2).collect::<Vec<_>>().join(".");
        p.text(pos2(name.right() + 8.0, y + 1.0), Align2::LEFT_CENTER, version, text_font(12.5), pal().dim);

        let cfg = self.feed.lock().unwrap_or_else(|e| e.into_inner()).cfg.clone();
        let Some(cfg) = cfg else { return };
        // right to left: game mode, then the profile
        let mut right = r.right() - 20.0;
        let pill_h = 36.0;
        // "Игровой режим" with a switch
        let label = tr("Игровой режим");
        let lw = p.layout_no_wrap(label.clone(), bold(13.0), pal().text).size().x;
        let gw = 16.0 + lw + 12.0 + 38.0 + 12.0;
        let game = egui::Rect::from_min_size(pos2(right - gw, y - pill_h / 2.0), vec2(gw, pill_h));
        let resp = ui.interact(game, ui.scope_id().with("game-mode"), Sense::click());
        pill(&p, game, resp.hovered());
        p.text(pos2(game.left() + 16.0, y), Align2::LEFT_CENTER, &label, bold(13.0), pal().text);
        let sw = egui::Rect::from_min_size(pos2(game.right() - 12.0 - 38.0, y - 11.0), vec2(38.0, 22.0));
        switch(ui, &p, sw, cfg.fps, "game-switch");
        if resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        if resp.on_hover_text(tr("Счётчик FPS, панель поверх игры и запись игровых сессий")).clicked()
        {
            let on = !cfg.fps;
            self.change_config(|c| c.fps = on);
        }
        right = game.left() - 12.0;
        // "Профиль  Работа ⌄"
        let names = [("work", tr("Работа")), ("game", tr("Игра"))];
        let current =
            names.iter().find(|(k, _)| *k == cfg.profile).map_or(cfg.profile.clone(), |(_, n)| n.clone());
        let pre = tr("Профиль");
        let w1 = p.layout_no_wrap(pre.clone(), text_font(13.0), pal().dim).size().x;
        let w2 = p.layout_no_wrap(current.clone(), bold(13.0), pal().text).size().x;
        let pw = 16.0 + w1 + 8.0 + w2 + 10.0 + 12.0 + 14.0;
        let prof = egui::Rect::from_min_size(pos2(right - pw, y - pill_h / 2.0), vec2(pw, pill_h));
        let resp = ui.interact(prof, ui.scope_id().with("profile"), Sense::click());
        pill(&p, prof, resp.hovered());
        let t = p.text(pos2(prof.left() + 16.0, y), Align2::LEFT_CENTER, &pre, text_font(13.0), pal().dim);
        p.text(pos2(t.right() + 8.0, y), Align2::LEFT_CENTER, &current, bold(13.0), pal().text);
        p.text(
            pos2(prof.right() - 14.0, y + 1.0),
            Align2::RIGHT_CENTER,
            icon::CHEVRON,
            icon_font(10.0),
            pal().dim,
        );
        if resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        let mut chosen = None;
        egui::Popup::menu(&resp).width(prof.width()).show(|ui| {
            for (key, name) in &names {
                if ui.selectable_label(*key == cfg.profile, RichText::new(name).size(13.5)).clicked() {
                    chosen = Some(key.to_string());
                }
            }
        });
        if let Some(k) = chosen.filter(|k| *k != cfg.profile) {
            self.change_config(|c| c.profile = k);
        }
    }

    /// The pages; at the bottom, which parts of PC Monitor are on.
    fn nav(&mut self, ui: &mut egui::Ui) {
        let r = ui.max_rect();
        let p = ui.painter().clone();
        let mut y = r.top() + 16.0;
        for (i, (name, glyph)) in TABS.iter().enumerate() {
            let item = egui::Rect::from_min_size(pos2(r.left() + 12.0, y), vec2(r.width() - 24.0, 42.0));
            let resp = ui.interact(item, ui.scope_id().with(("nav", i)), Sense::click());
            let selected = self.tab == i;
            if selected {
                p.rect_filled(item, 9.0, pal().card_hi);
            } else if resp.hovered() {
                p.rect_filled(item, 9.0, pal().card);
            }
            let col = if selected { pal().text } else { pal().dim };
            p.text(
                pos2(item.left() + 14.0, item.center().y),
                Align2::LEFT_CENTER,
                glyph,
                icon_font(15.0),
                col,
            );
            let font = if selected { bold(14.0) } else { text_font(14.0) };
            p.text(pos2(item.left() + 42.0, item.center().y), Align2::LEFT_CENTER, tr(name), font, col);
            if resp.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            }
            if resp.clicked() {
                self.tab = i;
            }
            y += 48.0;
        }
        // "Режимы": what runs besides this window
        let cfg = self.feed.lock().unwrap_or_else(|e| e.into_inner()).cfg.clone();
        let Some(c) = cfg else { return };
        let mut on = Vec::new();
        if c.widget {
            on.push(tr("Виджет"));
        }
        if !c.tray.is_empty() {
            on.push(tr("Трей"));
        }
        if c.taskbar.unwrap_or(!pcm_win::system::is_windows_11()) {
            on.push(tr("Панель задач"));
        }
        if c.fps && c.fps_panel {
            on.push(tr("Оверлей"));
        }
        let text = if on.is_empty() { tr("Только это окно") } else { on.join(" · ") };
        let inner_w = r.width() - 24.0 - 26.0;
        let g = p.layout(text, text_font(13.0), pal().text, inner_w);
        let h = 14.0 + 18.0 + g.size().y + 8.0 + 18.0 + 14.0;
        // "Светлая тема" / "Тёмная тема" at the very bottom
        let light = c.window_light;
        let row = egui::Rect::from_min_size(
            pos2(r.left() + 12.0, r.bottom() - 12.0 - 40.0),
            vec2(r.width() - 24.0, 40.0),
        );
        let look = ui.interact(row, ui.scope_id().with("look"), Sense::click());
        if look.hovered() {
            p.rect_filled(row, 9.0, pal().card);
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        let (glyph, name) = if light {
            (icon::MOON, tr("Тёмная тема"))
        } else {
            (icon::SUN, tr("Светлая тема"))
        };
        p.text(
            pos2(row.left() + 14.0, row.center().y),
            Align2::LEFT_CENTER,
            glyph,
            icon_font(15.0),
            pal().dim,
        );
        p.text(
            pos2(row.left() + 42.0, row.center().y),
            Align2::LEFT_CENTER,
            name,
            text_font(14.0),
            pal().dim,
        );
        if look.clicked() {
            self.change_config(|c| c.window_light = !light);
            let ctx = ui.ctx().clone();
            self.set_look(&ctx, !light);
        }
        let b =
            egui::Rect::from_min_size(pos2(r.left() + 12.0, row.top() - 10.0 - h), vec2(r.width() - 24.0, h));
        p.rect_filled(b, 10.0, pal().card);
        p.rect_stroke(b, 10.0, Stroke::new(1.0, pal().border), StrokeKind::Inside);
        let lx = b.left() + 13.0;
        p.text(pos2(lx, b.top() + 14.0), Align2::LEFT_TOP, tr("Режимы"), text_font(12.0), pal().faint);
        let gy = b.top() + 32.0;
        let gh = g.size().y;
        p.galley(pos2(lx, gy), g, pal().text);
        let link =
            p.text(pos2(lx, gy + gh + 8.0), Align2::LEFT_TOP, tr("Настроить"), text_font(13.0), pal().accent);
        let resp = ui.interact(link.expand(3.0), ui.scope_id().with("modes-link"), Sense::click());
        if resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            p.hline(link.x_range(), link.bottom(), Stroke::new(1.0, pal().accent));
        }
        if resp.clicked() {
            self.tab = TAB_SETTINGS;
        }
    }

    /// Sensors, rights and the driver on the left; PC Monitor's own cost
    /// and the time of the last reading on the right.
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let r = ui.max_rect();
        let p = ui.painter().clone();
        let y = r.center().y;
        let (data, start, seq, when) = {
            let f = self.feed.lock().unwrap_or_else(|e| e.into_inner());
            (f.data.clone(), f.start.clone(), f.seq, f.when.clone())
        };
        let Some(d) = data else {
            p.text(
                pos2(r.left() + 20.0, y),
                Align2::LEFT_CENTER,
                tr("Жду данные от виджета…"),
                text_font(12.0),
                pal().dim,
            );
            return;
        };
        let admin = start.as_ref().is_some_and(|s| s.admin);
        let parts = [
            (d.sensors_online, tr("Все датчики отвечают"), tr("Датчики не отвечают")),
            (admin, tr("права администратора есть"), tr("без прав администратора")),
            (d.pawnio, tr("PawnIO установлен"), tr("PawnIO не установлен")),
        ];
        let all_good = parts.iter().all(|(ok, _, _)| *ok);
        let text =
            parts.iter().map(|(ok, yes, no)| if *ok { yes.clone() } else { no.clone() }).collect::<Vec<_>>();
        let dot = if all_good { pal().good } else { pal().warn };
        p.circle_filled(pos2(r.left() + 24.0, y), 4.0, dot);
        p.text(pos2(r.left() + 36.0, y), Align2::LEFT_CENTER, text.join(" · "), text_font(12.0), pal().dim);

        // the widget's own cost plus this window's
        if self.own_read.0 != seq {
            let (cpu, mem) = self.own.read();
            self.own_read = (seq, cpu, mem);
        }
        let cpu = match (d.own_cpu, self.own_read.1) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0.0) + b.unwrap_or(0.0)),
        };
        let mem = match (d.own_mem, self.own_read.2) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
        };
        let mut right = r.right() - 20.0;
        if !when.is_empty() {
            let t = p.text(
                pos2(right, y),
                Align2::RIGHT_CENTER,
                format!("{} {when}", tr("Обновлено")),
                text_font(12.0),
                pal().dim,
            );
            right = t.left() - 18.0;
        }
        let mut own = Vec::new();
        if let Some(c) = cpu {
            own.push(format!("CPU {}%", dec(format!("{c:.1}"))));
        }
        if let Some(m) = mem {
            own.push(fmt_mem(m));
        }
        if !own.is_empty() {
            let t = format!("{}: {}", tr("PC Monitor сам"), own.join(" · "));
            p.text(pos2(right, y), Align2::RIGHT_CENTER, t, text_font(12.0), pal().dim);
        }
    }

    /// A page's name over its content (Главная draws its own).
    /// A page's name and what it is about, as on the home page.
    fn page_title(&self, ui: &mut egui::Ui, tab: usize) {
        // a report has its own header
        if tab == TAB_GAMES && self.sessions.report_open() {
            return;
        }
        let sub = match tab {
            TAB_HISTORY => "Нагрузка, температуры и сеть: точка каждые 10 секунд, хранится 30 дней",
            TAB_GAMES => "Сессии записываются сами, пока ты играешь: FPS, нагрузка и температуры",
            TAB_PROCS => "Программы, которые работают сейчас. «Закрыть» срабатывает со второго нажатия",
            TAB_DEVICES => "Что стоит в этом компьютере и как оно себя чувствует",
            _ => "Виджет, трей, игры, предупреждения и запуск",
        };
        ui.label(RichText::new(tr(TABS[tab].0)).font(bold(22.0)).color(pal().text));
        ui.add_space(2.0);
        ui.label(RichText::new(tr(sub)).font(text_font(13.0)).color(pal().dim));
        ui.add_space(14.0);
    }

    fn page(&mut self, ui: &mut egui::Ui) {
        if self.tab != TAB_HOME {
            self.page_title(ui, self.tab);
        }
        match self.tab {
            TAB_HOME => {
                let (data, theme, game, seq) = {
                    let f = self.feed.lock().unwrap_or_else(|e| e.into_inner());
                    (f.data.clone(), f.theme.clone(), f.game.clone(), f.seq)
                };
                let sc = theme.map(|t| t.scale());
                let input = match (&data, &sc) {
                    (Some(data), Some(sc)) => Some(HomeInput { data, sc, game: game.as_ref(), seq }),
                    _ => None,
                };
                match self.home.ui(ui, input) {
                    Some(HomeAction::Tab(t)) => self.tab = t,
                    Some(HomeAction::Session(path)) => {
                        self.sessions.open_path(&path, true);
                        self.tab = TAB_GAMES;
                    }
                    None => {}
                }
            }
            TAB_HISTORY => {
                let theme = self.feed.lock().unwrap_or_else(|e| e.into_inner()).theme.clone();
                self.history.ui(ui, theme.as_ref());
            }
            TAB_GAMES => {
                let theme = self.feed.lock().unwrap_or_else(|e| e.into_inner()).theme.clone();
                if let Some(crate::dash_sessions::SessionsAction::Home) = self.sessions.ui(ui, theme.as_ref())
                {
                    self.tab = TAB_HOME;
                }
            }
            TAB_PROCS => {
                let (rows, sc) = {
                    let f = self.feed.lock().unwrap_or_else(|e| e.into_inner());
                    (f.data.as_ref().and_then(|d| d.procs_all.clone()), f.theme.as_ref().map(|t| t.scale()))
                };
                self.procs.ui(ui, rows.as_ref(), sc.as_ref());
            }
            TAB_DEVICES => {
                let (data, theme) = {
                    let f = self.feed.lock().unwrap_or_else(|e| e.into_inner());
                    (f.data.clone(), f.theme.clone())
                };
                let sc = theme.map(|t| t.scale());
                crate::dash_devices::ui(ui, data.as_ref(), sc.as_ref());
            }
            _ => {
                let (cfg, start, data, theme, key) = {
                    let f = self.feed.lock().unwrap_or_else(|e| e.into_inner());
                    (f.cfg.clone(), f.start.clone(), f.data.clone(), f.theme.clone(), f.theme_key.clone())
                };
                let (cfg, start, data) = (cfg.as_ref(), start.as_ref(), data.as_ref());
                self.settings.ui(ui, cfg, start, data, &mut self.fonts, theme.as_ref(), &key);
            }
        }
    }
}

/// What pictures are drawn for: the theme, the language and the look.
fn theme_key(name: &str, light: bool) -> String {
    format!("{name}{}", if light { "/light" } else { "/dark" })
}

/// egui's own widgets (buttons, sliders, checkboxes) in the window's colours.
fn apply_look(ctx: &egui::Context, light: bool) {
    let c = if light { &crate::dash_home::LIGHT } else { &crate::dash_home::DARK };
    let mut v = if light { egui::Visuals::light() } else { egui::Visuals::dark() };
    v.panel_fill = c.bg;
    v.window_fill = c.card;
    v.extreme_bg_color = if light { Color32::WHITE } else { c.chrome };
    v.override_text_color = Some(c.text);
    v.selection.bg_fill = if light { c.card_hi } else { c.border };
    v.selection.stroke = Stroke::new(1.0, c.text);
    v.widgets.hovered.weak_bg_fill = c.card_hi;
    v.widgets.inactive.weak_bg_fill = c.card;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, c.border);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, c.border);
    v.widgets.noninteractive.bg_fill = c.card;
    if light {
        // checkboxes and slider rails: white boxes with a border on the grey page
        v.widgets.inactive.bg_fill = Color32::WHITE;
    }
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = CornerRadius::same(8);
    }
    // ours, whatever the Windows theme
    let (pref, which) = if light {
        (egui::ThemePreference::Light, egui::Theme::Light)
    } else {
        (egui::ThemePreference::Dark, egui::Theme::Dark)
    };
    ctx.set_theme(pref);
    ctx.set_visuals_of(which, v);
}

/// A rounded box with a border, lighter under the mouse.
fn pill(p: &egui::Painter, r: egui::Rect, hovered: bool) {
    p.rect_filled(r, 9.0, if hovered { pal().card_hi } else { pal().card });
    p.rect_stroke(r, 9.0, Stroke::new(1.0, pal().border), StrokeKind::Inside);
}

/// An on/off switch that slides.
fn switch(ui: &egui::Ui, p: &egui::Painter, r: egui::Rect, on: bool, id: &str) {
    let t = ui.ctx().animate_bool_with_time(ui.scope_id().with(id), on, 0.12);
    let track = Color32::from_rgb(
        (pal().border.r() as f32 + (pal().accent.r() as f32 - pal().border.r() as f32) * t) as u8,
        (pal().border.g() as f32 + (pal().accent.g() as f32 - pal().border.g() as f32) * t) as u8,
        (pal().border.b() as f32 + (pal().accent.b() as f32 - pal().border.b() as f32) * t) as u8,
    );
    p.rect_filled(r, r.height() / 2.0, track);
    let rad = r.height() / 2.0 - 3.0;
    let x = r.left() + 3.0 + rad + (r.width() - 6.0 - 2.0 * rad) * t;
    p.circle_filled(pos2(x, r.center().y), rad, Color32::WHITE);
}

impl Dashboard {
    /// Two seconds after the first feed (the tab has drawn), take the
    /// window's own pixels: works off screen too, unlike PrintWindow.
    fn screenshot(&mut self, ctx: &egui::Context) {
        let fed = self.feed.lock().map(|f| f.seq > 0).unwrap_or(false);
        let Some((path, since)) = &mut self.shot else { return };
        if !fed {
            return;
        }
        let since = *since.get_or_insert_with(std::time::Instant::now);
        if since.elapsed() < Duration::from_secs(2) {
            ctx.request_repaint_after(Duration::from_millis(200));
            return;
        }
        let path = path.clone();
        self.shot = None;
        let c = ctx.clone();
        ctx.request_screenshot(move |img| {
            let bytes: Vec<u8> = img.pixels.iter().flat_map(|p| p.to_array()).collect();
            let size = tiny_skia::IntSize::from_wh(img.size[0] as u32, img.size[1] as u32);
            if let Some(pm) = size.and_then(|s| tiny_skia::Pixmap::from_vec(bytes, s)) {
                let _ = pm.save_png(&path);
            }
            c.send_viewport_cmd(egui::ViewportCommand::Close);
            c.request_repaint();
        });
    }
}

impl eframe::App for Dashboard {
    /// The window closes: by its × (or Alt+F4) the widget is told (it may
    /// quit too); not when the widget has gone already or in a test run.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.quit_on_close && !self.feed.lock().map(|f| f.gone).unwrap_or(true) {
            send(&FromDashboard::Closed);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // the look from the settings ("Светлая тема"), the title bar with it
        let light = self.feed.lock().map(|f| f.cfg.as_ref().is_some_and(|c| c.window_light)).unwrap_or(false);
        if self.look != Some(light) {
            self.set_look(&ctx, light);
        }
        self.screenshot(&ctx);
        let (focus, session, week) = {
            let mut f = self.feed.lock().unwrap_or_else(|e| e.into_inner());
            (std::mem::take(&mut f.focus), f.open_session.take(), std::mem::take(&mut f.history_week))
        };
        if week {
            self.history.show_week();
            self.tab = TAB_HISTORY;
        }
        if let Some(path) = session {
            self.sessions.open_path(&path, false);
            self.tab = TAB_GAMES;
        }
        if focus {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        // the home page lists the busiest programs too
        self.procs.visible(self.tab == TAB_PROCS || self.tab == TAB_HOME);
        ui.painter().rect_filled(ui.max_rect(), 0.0, pal().bg);
        let chrome = Frame::new().fill(pal().chrome);
        egui::Panel::top("bar")
            .exact_size(BAR_H)
            .resizable(false)
            .frame(chrome)
            .show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status")
            .exact_size(STATUS_H)
            .resizable(false)
            .frame(chrome)
            .show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("nav").exact_size(NAV_W).resizable(false).frame(chrome).show(ui, |ui| self.nav(ui));
        egui::CentralPanel::no_frame().show(ui, |ui| {
            Frame::new()
                .fill(pal().bg)
                .inner_margin(Margin { left: 24, right: 24, top: 20, bottom: 8 })
                .show(ui, |ui| {
                    ui.with_layout(Layout::top_down(Align::Min), |ui| self.page(ui));
                });
        });
        // new data wakes it (request_repaint from the reader); nothing else moves
        ctx.request_repaint_after(Duration::from_secs(5));
    }
}

/// The window's first size: like the concept (1440 × 960) when the screen
/// has room, else most of the work area, centred on it. Called before
/// eframe makes the process DPI aware, so Windows answers in points.
fn first_place() -> Option<([f32; 2], [f32; 2])> {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETWORKAREA, SystemParametersInfoW};
    let mut wa = RECT::default();
    if unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut wa as *mut _ as *mut _, 0) } == 0 {
        return None;
    }
    let (ww, wh) = ((wa.right - wa.left) as f32, (wa.bottom - wa.top) as f32);
    if ww < 400.0 || wh < 300.0 {
        return None;
    }
    let w = (ww - 80.0).min(1440.0);
    let h = (wh - 90.0).min(960.0); // the title bar comes on top of it
    let x = wa.left as f32 + (ww - w) / 2.0;
    let y = wa.top as f32 + ((wh - h - 32.0) / 2.0).max(0.0);
    Some(([w, h], [x, y]))
}

/// Run the window (on tab `tab`) until it is closed or the widget goes
/// away. Returns the exit code.
/// `offscreen`: far off the screen and not focused; `screenshot`: save the
/// window as PNG and close (both for tests).
pub fn run(tab: usize, offscreen: bool, screenshot: Option<String>, scroll: Option<f32>) -> i32 {
    let feed = Arc::new(Mutex::new(Feed::default()));
    let ctx_slot: Arc<Mutex<Option<egui::Context>>> = Arc::new(Mutex::new(None));
    {
        let (feed, ctx_slot) = (feed.clone(), ctx_slot.clone());
        std::thread::Builder::new()
            .name("feed".into())
            .spawn(move || {
                let stdin = std::io::stdin();
                for line in stdin.lock().lines() {
                    let Ok(line) = line else { break };
                    let Ok(msg) = serde_json::from_str::<ToDashboard>(&line) else { continue };
                    if let Ok(mut f) = feed.lock() {
                        match msg {
                            ToDashboard::Feed { data, theme, custom, cfg, start, game } => {
                                crate::apply_language(&cfg.lang);
                                f.data = Some(*data);
                                f.cfg = Some(*cfg);
                                f.start = Some(start);
                                f.game = game.map(|g| *g);
                                f.theme = Some(theme_of(&theme, &custom));
                                // the language too: charts and reports have words
                                f.theme_name = format!("{theme}{custom:?}{}", pcm_render::format::language());
                                f.theme_key = theme_key(&f.theme_name, pcm_render::theme::light());
                                f.when = pcm_win::system::local_time().get(11..).unwrap_or("").to_string();
                                f.seq += 1;
                            }
                            ToDashboard::Focus => f.focus = true,
                            ToDashboard::OpenSession(path) => {
                                f.open_session = Some(path);
                                f.focus = true;
                            }
                            ToDashboard::HistoryWeek => {
                                f.history_week = true;
                                f.focus = true;
                            }
                        }
                    }
                    if let Some(ctx) = ctx_slot.lock().ok().and_then(|c| c.clone()) {
                        ctx.request_repaint();
                    }
                }
                // the widget has gone (its end of the pipe closed): so does the window
                if let Ok(mut f) = feed.lock() {
                    f.gone = true;
                }
                if let Some(ctx) = ctx_slot.lock().ok().and_then(|c| c.clone()) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    ctx.request_repaint();
                }
            })
            .ok();
    }
    let place = if offscreen { None } else { first_place() };
    // test runs may ask for a size: PCM_DASH_SIZE=1100x760
    let asked = std::env::var("PCM_DASH_SIZE").ok().and_then(|v| {
        let (w, h) = v.split_once('x')?;
        Some([w.parse::<f32>().ok()?, h.parse::<f32>().ok()?])
    });
    let size = asked.or(place.map(|(s, _)| s)).unwrap_or([1440.0, 900.0]);
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size(size)
        .with_min_inner_size([NAV_W + 380.0, 480.0])
        .with_title("PC Monitor");
    if let Some((_, pos)) = place {
        viewport = viewport.with_position(pos);
    }
    if offscreen {
        viewport = viewport.with_position([-6000.0, -6000.0]).with_active(false);
    }
    let options = eframe::NativeOptions { viewport, renderer: eframe::Renderer::Glow, ..Default::default() };
    let result = eframe::run_native(
        "PC Monitor",
        options,
        Box::new(move |cc| {
            if let Ok(mut c) = ctx_slot.lock() {
                *c = Some(cc.egui_ctx.clone());
            }
            let mut d = Dashboard::new(cc, feed);
            d.tab = tab.min(TABS.len() - 1);
            // test runs: open a session's report (PCM_DASH_SESSION=FILE)
            if let Ok(path) = std::env::var("PCM_DASH_SESSION") {
                d.sessions.open_path(std::path::Path::new(&path), false);
                d.tab = TAB_GAMES;
            }
            d.settings.start_offset = scroll;
            d.quit_on_close = !offscreen && screenshot.is_none();
            d.shot = screenshot.map(|p| (p, None));
            Ok(Box::new(d))
        }),
    );
    if result.is_err() { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_both_ways() {
        // the widget's lines for the window and back, as they go through the pipes
        let feed = ToDashboard::Feed {
            data: Box::default(),
            theme: "ice".into(),
            custom: Default::default(),
            cfg: Box::default(),
            start: StartInfo { autostart: true, admin: false, helper: true },
            game: Some(Box::new(LiveGame {
                game: "stalzonew".into(),
                fps: Some(144.0),
                ..Default::default()
            })),
        };
        let line = serde_json::to_string(&feed).unwrap();
        assert!(!line.contains('\n'));
        assert!(matches!(serde_json::from_str(&line), Ok(ToDashboard::Feed { theme, .. }) if theme == "ice"));
        assert!(matches!(
            serde_json::from_str(&line),
            Ok(ToDashboard::Feed { game: Some(g), .. }) if g.game == "stalzonew" && g.fps == Some(144.0)
        ));
        assert_eq!(serde_json::to_string(&ToDashboard::Focus).unwrap(), "\"Focus\"");
        let p = std::path::PathBuf::from(r"C:\Users\Влад\AppData\Roaming\PC-Monitor\sessions\a.pcms");
        let open = serde_json::to_string(&ToDashboard::OpenSession(p.clone())).unwrap();
        assert!(!open.contains('\n'));
        assert!(matches!(serde_json::from_str(&open), Ok(ToDashboard::OpenSession(q)) if q == p));
        let ask = serde_json::to_string(&FromDashboard::EndProgram("chrome".into())).unwrap();
        assert_eq!(ask, r#"{"EndProgram":"chrome"}"#);
        assert!(matches!(serde_json::from_str(r#"{"WantProcs":true}"#), Ok(FromDashboard::WantProcs(true))));
        assert!(matches!(serde_json::from_str(r#"{"SetAdmin":true}"#), Ok(FromDashboard::SetAdmin(true))));
        assert!(
            matches!(serde_json::from_str(&line), Ok(ToDashboard::Feed { start, .. }) if start.autostart)
        );
        // a partial feed still reads (missing readings = defaults, no game)
        let partial =
            r##"{"Feed":{"data":{"cpu":12.5},"theme":"heat","custom":["#000000","#ffffff"],"cfg":{}}}"##;
        assert!(matches!(
            serde_json::from_str(partial),
            Ok(ToDashboard::Feed { data, game: None, .. }) if data.cpu == 12.5
        ));
    }
}

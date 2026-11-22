//! The PC Monitor window (stage 5): `PC-Monitor.exe --dashboard`, a child
//! process of the widget. The widget feeds it a line of JSON a second on
//! its stdin; closing the window ends the process, so its memory (egui,
//! OpenGL) goes back to Windows instead of staying in the widget.

use std::io::BufRead;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui;
use egui::{Color32, FontData, FontDefinitions, FontFamily, RichText, Stroke, Vec2};
use pcm_render::cards::{CARD_H, CARD_W, CARDS, card};
use pcm_render::text::Fonts;
use pcm_render::theme::{Theme, hex};
use pcm_render::widget::Data;
use serde::{Deserialize, Serialize};

use crate::dash_history::HistoryTab;
use crate::dash_procs::ProcsTab;
use crate::dash_sessions::SessionsTab;
use crate::dash_settings::SettingsTab;
use pcm_render::format::{tr, trf};

/// What the widget sends, one per line.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ToDashboard {
    /// The latest readings and the colours to draw them in.
    Feed { data: Box<Data>, theme: String, custom: [String; 2], cfg: Box<pcm_core::config::Config> },
    /// "Открыть PC Monitor" again: come to the front.
    Focus,
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
}

/// Ask the widget something.
pub fn send(msg: &FromDashboard) {
    use std::io::Write;
    if let Ok(line) = serde_json::to_string(msg) {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{line}").and_then(|_| out.flush());
    }
}

const BG: Color32 = Color32::from_rgb(20, 23, 30);
const CARD: Color32 = Color32::from_rgb(28, 33, 43);
const BORDER: Color32 = Color32::from_rgb(44, 51, 64);
const TEXT: Color32 = Color32::from_rgb(232, 234, 239);
const DIM: Color32 = Color32::from_rgb(139, 147, 163);

const TABS: [&str; 5] = ["Обзор", "Процессы", "История", "Игровые сессии", "Настройки"];
const GAP: f32 = 12.0;

#[derive(Default)]
struct Feed {
    data: Option<Data>,
    theme: Option<Theme>,
    cfg: Option<pcm_core::config::Config>,
    /// The theme's name and colours, to know when charts need drawing again.
    theme_key: String,
    /// Bumped with every new feed: the cards are drawn again.
    seq: u64,
    focus: bool,
}

/// Segoe UI and Bahnschrift from the Windows fonts folder.
fn fonts() -> FontDefinitions {
    let mut defs = FontDefinitions::empty();
    let dir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    for (name, file) in [("ui", "segoeui.ttf"), ("ui-semibold", "seguisb.ttf"), ("symbols", "seguisym.ttf")] {
        if let Ok(data) = std::fs::read(format!(r"{dir}\Fonts\{file}")) {
            defs.font_data.insert(name.into(), Arc::new(FontData::from_owned(data)));
        }
    }
    let have = |n: &str| defs.font_data.contains_key(n).then(|| n.to_string());
    let ui: Vec<String> = [have("ui"), have("symbols")].into_iter().flatten().collect();
    defs.families.insert(FontFamily::Proportional, ui.clone());
    defs.families.insert(FontFamily::Monospace, ui.clone());
    let bold: Vec<String> = [have("ui-semibold"), have("ui")].into_iter().flatten().collect();
    defs.families.insert(FontFamily::Name("semibold".into()), bold);
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
    cards: Vec<Option<egui::TextureHandle>>,
    /// The feed and the scale the textures were drawn for.
    drawn: (u64, f32),
    history: HistoryTab,
    procs: ProcsTab,
    sessions: SessionsTab,
    settings: SettingsTab,
    /// `--screenshot FILE`: save the window's own pixels once fed, then close.
    shot: Option<(String, Option<std::time::Instant>)>,
}

impl Dashboard {
    fn new(cc: &eframe::CreationContext, feed: Arc<Mutex<Feed>>) -> Dashboard {
        cc.egui_ctx.set_fonts(fonts());
        let mut v = egui::Visuals::dark();
        v.panel_fill = BG;
        v.window_fill = BG;
        v.override_text_color = Some(TEXT);
        v.selection.bg_fill = BORDER;
        v.selection.stroke = Stroke::new(1.0, TEXT);
        v.widgets.hovered.weak_bg_fill = CARD;
        // always dark, whatever the Windows theme
        cc.egui_ctx.set_theme(egui::ThemePreference::Dark);
        cc.egui_ctx.set_visuals_of(egui::Theme::Dark, v);
        Dashboard {
            feed,
            tab: 0,
            fonts: Fonts::system(),
            cards: vec![None; CARDS.len()],
            drawn: (0, 0.0),
            history: HistoryTab::default(),
            procs: ProcsTab::default(),
            sessions: SessionsTab::default(),
            settings: SettingsTab::default(),
            shot: None,
        }
    }

    /// Draw the cards again for a new feed or a new DPI.
    fn update_cards(&mut self, ctx: &egui::Context) {
        let ppp = ctx.pixels_per_point();
        let (data, theme, seq) = {
            let f = self.feed.lock().unwrap_or_else(|e| e.into_inner());
            (f.data.clone(), f.theme.clone(), f.seq)
        };
        if self.drawn == (seq, ppp) {
            return;
        }
        let (Some(d), Some(theme)) = (data, theme) else { return };
        for (i, kind) in CARDS.iter().enumerate() {
            let Some(pm) = card(&mut self.fonts, ppp, *kind, &d, &theme) else { continue };
            let img = egui::ColorImage::from_rgba_premultiplied(
                [pm.width() as usize, pm.height() as usize],
                pm.data(),
            );
            match &mut self.cards[i] {
                Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                slot => {
                    *slot = Some(ctx.load_texture(format!("card-{i}"), img, egui::TextureOptions::LINEAR))
                }
            }
        }
        self.drawn = (seq, ppp);
    }

    fn overview(&mut self, ui: &mut egui::Ui) {
        if self.cards.iter().all(|c| c.is_none()) {
            ui.label(RichText::new(tr("Жду данные от виджета…")).color(DIM));
            return;
        }
        let cols = (((ui.available_width() + GAP) / (CARD_W + GAP)).floor() as usize).max(1);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for row in self.cards.chunks(cols) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    for t in row.iter().flatten() {
                        ui.add(egui::Image::new((t.id(), Vec2::new(CARD_W, CARD_H))));
                    }
                });
                ui.add_space(GAP - ui.spacing().item_spacing.y);
            }
        });
    }
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
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.screenshot(&ctx);
        let focus = std::mem::take(&mut self.feed.lock().unwrap_or_else(|e| e.into_inner()).focus);
        if focus {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        self.update_cards(&ctx);
        self.procs.visible(self.tab == 1);
        ui.painter().rect_filled(ui.max_rect(), 0.0, BG);
        egui::Frame::new().fill(BG).inner_margin(20).show(ui, |ui| {
            ui.horizontal(|ui| {
                for (i, name) in TABS.iter().enumerate() {
                    let selected = self.tab == i;
                    let text = RichText::new(tr(name)).size(15.0).color(if selected { TEXT } else { DIM });
                    if ui.add(egui::Button::selectable(selected, text)).clicked() {
                        self.tab = i;
                    }
                    ui.add_space(4.0);
                }
            });
            ui.add_space(16.0);
            match self.tab {
                0 => self.overview(ui),
                1 => {
                    let (rows, sc) = {
                        let f = self.feed.lock().unwrap_or_else(|e| e.into_inner());
                        (
                            f.data.as_ref().and_then(|d| d.procs_all.clone()),
                            f.theme.as_ref().map(|t| t.scale()),
                        )
                    };
                    self.procs.ui(ui, rows.as_ref(), sc.as_ref());
                }
                4 => {
                    let cfg = self.feed.lock().unwrap_or_else(|e| e.into_inner()).cfg.clone();
                    self.settings.ui(ui, cfg.as_ref());
                }
                3 => {
                    let theme = self.feed.lock().unwrap_or_else(|e| e.into_inner()).theme.clone();
                    self.sessions.ui(ui, &mut self.fonts, theme.as_ref());
                }
                2 => {
                    let (theme, key) = {
                        let f = self.feed.lock().unwrap_or_else(|e| e.into_inner());
                        (f.theme.clone(), f.theme_key.clone())
                    };
                    self.history.ui(ui, &mut self.fonts, theme.as_ref(), &key);
                }
                n => {
                    ui.label(
                        RichText::new(trf("«{tab}» появится позже.", &[("tab", tr(TABS[n]))])).color(DIM),
                    );
                }
            }
        });
        // new data wakes it (request_repaint from the reader); nothing else moves
        ctx.request_repaint_after(Duration::from_secs(5));
    }
}

/// Run the window (on tab `tab`) until it is closed or the widget goes
/// away. Returns the exit code.
/// `offscreen`: far off the screen and not focused; `screenshot`: save the
/// window as PNG and close (both for tests).
pub fn run(tab: usize, offscreen: bool, screenshot: Option<String>) -> i32 {
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
                            ToDashboard::Feed { data, theme, custom, cfg } => {
                                crate::apply_language(&cfg.lang);
                                f.data = Some(*data);
                                f.cfg = Some(*cfg);
                                f.theme = Some(theme_of(&theme, &custom));
                                // the language too: charts and reports have words
                                f.theme_key = format!("{theme}{custom:?}{}", pcm_render::format::language());
                                f.seq += 1;
                            }
                            ToDashboard::Focus => f.focus = true,
                        }
                    }
                    if let Some(ctx) = ctx_slot.lock().ok().and_then(|c| c.clone()) {
                        ctx.request_repaint();
                    }
                }
                // the widget has gone (its end of the pipe closed): so does the window
                if let Some(ctx) = ctx_slot.lock().ok().and_then(|c| c.clone()) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    ctx.request_repaint();
                }
            })
            .ok();
    }
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([4.0 * CARD_W + 3.0 * GAP + 40.0, 2.0 * CARD_H + GAP + 120.0])
        .with_min_inner_size([CARD_W + 40.0, 360.0])
        .with_title("PC Monitor");
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
        };
        let line = serde_json::to_string(&feed).unwrap();
        assert!(!line.contains('\n'));
        assert!(matches!(serde_json::from_str(&line), Ok(ToDashboard::Feed { theme, .. }) if theme == "ice"));
        assert_eq!(serde_json::to_string(&ToDashboard::Focus).unwrap(), "\"Focus\"");
        let ask = serde_json::to_string(&FromDashboard::EndProgram("chrome".into())).unwrap();
        assert_eq!(ask, r#"{"EndProgram":"chrome"}"#);
        assert!(matches!(serde_json::from_str(r#"{"WantProcs":true}"#), Ok(FromDashboard::WantProcs(true))));
        // a partial feed still reads (missing readings = defaults)
        let partial =
            r##"{"Feed":{"data":{"cpu":12.5},"theme":"heat","custom":["#000000","#ffffff"],"cfg":{}}}"##;
        assert!(
            matches!(serde_json::from_str(partial), Ok(ToDashboard::Feed { data, .. }) if data.cpu == 12.5)
        );
    }
}

//! The "Настройки" tab of the PC Monitor window (beta 2, in the home page's
//! style): the settings as cards in two columns — general, the widget, what
//! it shows, the tray, startup | games, the FPS panel, alerts, ping, updates —
//! with switches, segmented choices and pills. The tab edits a copy of the
//! widget's settings and sends it back whole after each change; the widget
//! takes what can be set here and applies it (and feeds the result back a
//! second later).

use std::ops::RangeInclusive;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{
    Align, Align2, CursorIcon, Galley, Layout, Rect, Response, RichText, Sense, Stroke, StrokeKind, Ui,
    UiBuilder, pos2, vec2,
};
use pcm_core::config::{self, Config, FPS_ROWS, SECTIONS, TRAY_KINDS, TRAY_MAX};

use crate::dash_home::{alpha, bold, fit, icon, icon_font, pal, text_font, text_w};
use crate::dash_kit::{GAP, button_at, switch};
use crate::dashboard::{FromDashboard, StartInfo, send};
use pcm_render::format::{tr, trf};
use pcm_render::overlay::{Look, preview, sample_game};
use pcm_render::text::Fonts;
use pcm_render::theme::Theme;
use pcm_render::widget::{Data, UpdateState};

/// After a change here, the widget's copy (a feed behind) is not taken for this long.
const SETTLE: Duration = Duration::from_secs(3);
const PING_PRESETS: [(&str, &str); 3] =
    [("1.1.1.1", "Cloudflare"), ("8.8.8.8", "Google"), ("9.9.9.9", "Quad9")];
/// From this width the cards stand in two columns.
const TWO_COLUMNS: f32 = 900.0;
const MAX_W: f32 = 1240.0;
/// A row's least height: a label and its control.
const ROW_H: f32 = 42.0;
const SLIDER_W: f32 = 170.0;
/// A slider row's right part: the rail, its value and room for "100%".
const SLIDER_ROW_W: f32 = SLIDER_W + 8.0 + 46.0 + 8.0 + 58.0;

/// The cards' icons (Segoe MDL2 Assets) that the other pages don't have.
mod glyph {
    pub const GLOBE: &str = "\u{E774}";
    pub const WIDGET: &str = "\u{E8A9}";
    pub const EYE: &str = "\u{E890}";
    pub const TRAY: &str = "\u{E7C4}";
    pub const FPS: &str = "\u{E9D2}";
    pub const ALERT: &str = "\u{E7BA}";
    pub const START: &str = "\u{E7E8}";
    pub const UPDATE: &str = "\u{E895}";
    pub const CHECK: &str = "\u{E73E}";
}

#[derive(Default)]
pub struct SettingsTab {
    edit: Option<Config>,
    changed: Option<Instant>,
    host_text: String,
    host_bad: bool,
    /// `--scroll N` (screenshots of the lower sections): start scrolled down.
    pub start_offset: Option<f32>,
    /// "Найти настройку".
    search: String,
    /// Ctrl+F: the search gets the keyboard on the next frame.
    pub focus_search: bool,
    /// The FPS panel over a made-up game, and what it was drawn for.
    preview: Option<egui::TextureHandle>,
    preview_key: String,
}

/// What the cards read besides the settings, and what a button asks for.
struct Env<'a> {
    start: Option<&'a StartInfo>,
    data: Option<&'a Data>,
    action: Option<FromDashboard>,
}

fn rgb_of(hex: &str) -> [u8; 3] {
    pcm_render::theme::hex(hex).map_or([128, 128, 128], |c| [c.0, c.1, c.2])
}

// ---- the pieces ---------------------------------------------------------------------------

/// How long a card that was jumped to stays outlined.
const JUMP_GLOW: Duration = Duration::from_millis(1600);

/// The cards, as the page lays them out, and what each holds (untranslated):
/// the contents line and the search look here.
const INDEX: &[(&str, &[&str])] = &[
    (
        "Общие",
        &["Язык", "Language", "Цвета", "Малая нагрузка", "Большая нагрузка", "Тепло", "Неон", "Лёд", "Свои"],
    ),
    (
        "Виджет",
        &[
            "Показывать виджет (Ctrl+Alt+W)",
            "Вид",
            "Карточки",
            "Светлая тема (окно и виджет «Карточки»)",
            "Масштаб окна",
            "Закрепить на рабочем столе",
            "Поверх всех окон",
            "Компактный режим",
            "Размер",
            "Непрозрачность",
        ],
    ),
    (
        "Показывать в виджете",
        &[
            "Ядра",
            "Память",
            "Видеокарта",
            "Диски",
            "Температура",
            "Сеть",
            "График сети",
            "Пинг",
            "Чтение и запись диска",
            "Износ батареи",
            "Здоровье SSD",
            "Вентиляторы",
            "Время без перезагрузки",
        ],
    ),
    ("Трей и панель задач", &["Значки у часов и мини-панель", "Мини-панель на панели задач"]),
    (
        "Запуск и закрытие",
        &[
            "Запускать вместе с Windows",
            "Права администратора (температуры)",
            "Крестик полностью закрывает программу",
        ],
    ),
    (
        "Игры",
        &[
            "В играх и видео на весь экран виджет:",
            "Считать FPS (панель, запись сессий)",
            "Профиль",
            "Включать «Игру», когда запущена игра",
            "Игровой режим",
        ],
    ),
    (
        "FPS-панель",
        &[
            "Показывать FPS-панель (Ctrl+Alt+F)",
            "Вид панели",
            "Кроме FPS показывать:",
            "Угол панели",
            "Размер панели",
            "Фон панели",
        ],
    ),
    (
        "Предупреждения",
        &[
            "Показывать предупреждения",
            "Во время игры — показать после неё",
            "Итоги недели",
            "Перегрев CPU от",
            "Память занята от",
            "Батарея ниже",
            "Свободно на диске C: меньше",
        ],
    ),
    ("Пинг", &["Куда отправлять пинг", "Сервер", "Свой адрес:"]),
    ("Обновления", &["Проверять обновления", "Проверить сейчас"]),
];

thread_local! {
    /// What the search asks for, lower case ("" shows every card).
    static SEARCH: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    /// The card to bring into view (its title), when that was asked, and
    /// whether it has been scrolled to.
    static JUMP: std::cell::RefCell<Option<(String, Instant, bool)>> = const { std::cell::RefCell::new(None) };
}

/// Open the settings at the card titled `title` (translated, as the card
/// shows it): it is scrolled to and outlined for a moment.
pub fn jump_to(title: String) {
    JUMP.with(|j| *j.borrow_mut() = Some((title, Instant::now(), false)));
}

/// Does the search find the card titled `title` (translated) or one of its
/// rows? Russian words match in any language.
fn found(title: &str) -> bool {
    let q = SEARCH.with(|s| s.borrow().clone());
    if q.is_empty() {
        return true;
    }
    let hit = |t: &str| t.to_lowercase().contains(&q) || tr(t).to_lowercase().contains(&q);
    title.to_lowercase().contains(&q)
        || INDEX.iter().filter(|(k, _)| tr(k) == title).any(|(k, rows)| hit(k) || rows.iter().any(|r| hit(r)))
}

/// A card: the icon in its box, the title and a line under it, then the rows.
fn card(ui: &mut Ui, glyph: &str, title: &str, sub: &str, add: impl FnOnce(&mut Ui)) {
    if !found(title) {
        return;
    }
    let shown = egui::Frame::new()
        .fill(pal().card)
        .stroke(Stroke::new(1.0, pal().border))
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin { left: 18, right: 18, top: 16, bottom: 8 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::hover());
            let p = ui.painter();
            let ib = Rect::from_min_size(r.left_top(), vec2(34.0, 34.0));
            p.rect_filled(ib, 9.0, pal().card_hi);
            p.text(ib.center(), Align2::CENTER_CENTER, glyph, icon_font(16.0), pal().dim);
            let tx = ib.right() + 12.0;
            let room = r.right() - tx;
            p.text(
                pos2(tx, r.top()),
                Align2::LEFT_TOP,
                fit(p, title, &bold(14.5), room),
                bold(14.5),
                pal().text,
            );
            if !sub.is_empty() {
                let f = text_font(12.0);
                p.text(pos2(tx, r.top() + 20.0), Align2::LEFT_TOP, fit(p, sub, &f, room), f, pal().dim);
            }
            ui.add_space(8.0);
            add(ui);
        });
    // asked to jump here: into view, then the outline fades
    let jump = JUMP.with(|j| {
        let mut j = j.borrow_mut();
        let (_, at, scrolled) = j.as_mut().filter(|(t, _, _)| t == title)?;
        let first = !*scrolled;
        *scrolled = true;
        let left = JUMP_GLOW.saturating_sub(at.elapsed());
        if left.is_zero() {
            *j = None;
        }
        Some((first, left))
    });
    if let Some((first, left)) = jump {
        let r = shown.response.rect;
        if first {
            ui.scroll_to_rect(r, Some(egui::Align::TOP));
        }
        let a = (left.as_secs_f32() / JUMP_GLOW.as_secs_f32()).clamp(0.0, 1.0);
        ui.painter().rect_stroke(
            r,
            12.0,
            Stroke::new(2.0, pal().accent.gamma_multiply(a)),
            egui::StrokeKind::Inside,
        );
        ui.ctx().request_repaint();
    }
}

/// A thin line between rows.
fn sep(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(r.x_range(), r.center().y, Stroke::new(1.0, pal().grid));
}

fn texts(ui: &Ui, label: &str, hint: &str, w: f32) -> (Arc<Galley>, Option<Arc<Galley>>, f32) {
    let p = ui.painter();
    let lg = p.layout(label.to_string(), text_font(13.5), pal().text, w);
    let hg = (!hint.is_empty()).then(|| p.layout(hint.to_string(), text_font(12.0), pal().faint, w));
    let h = lg.size().y + hg.as_ref().map_or(0.0, |g| g.size().y + 2.0);
    (lg, hg, h)
}

/// A row: the label (and a hint under it) on the left, `cw` px for the control on
/// the right. A clickable row lights up under the mouse. The row and the control's rect.
fn row(ui: &mut Ui, label: &str, hint: &str, cw: f32, sense: Sense) -> (Response, Rect) {
    let w = ui.available_width();
    let lw = if cw > 0.0 { (w - cw - 16.0).max(60.0) } else { w };
    let (lg, hg, text_h) = texts(ui, label, hint, lw);
    let h = (text_h + 18.0).max(ROW_H);
    let (r, resp) = ui.allocate_exact_size(vec2(w, h), sense);
    let p = ui.painter();
    if sense.senses_click() && resp.hovered() && ui.is_enabled() {
        p.rect_filled(r.expand2(vec2(10.0, -2.0)), 8.0, alpha(pal().card_hi, 130));
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    let mut y = r.center().y - text_h / 2.0;
    let lh = lg.size().y;
    p.galley(pos2(r.left(), y), lg, pal().text);
    y += lh + 2.0;
    if let Some(g) = hg {
        p.galley(pos2(r.left(), y), g, pal().faint);
    }
    (resp, Rect::from_min_max(pos2(r.right() - cw, r.top()), r.max))
}

/// Widgets in `r`, laid out from the right.
fn place<R>(ui: &mut Ui, r: Rect, id: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    let mut child =
        ui.new_child(UiBuilder::new().id_salt(id).max_rect(r).layout(Layout::right_to_left(Align::Center)));
    child.spacing_mut().item_spacing = vec2(8.0, 0.0);
    add(&mut child)
}

/// An on/off row: a click anywhere on it flips the switch. True when flipped.
fn toggle(ui: &mut Ui, label: &str, hint: &str, on: &mut bool) -> bool {
    let active = ui.is_enabled();
    let (resp, ctrl) = row(ui, label, hint, 44.0, if active { Sense::click() } else { Sense::hover() });
    let flipped = active && resp.clicked();
    if flipped {
        *on = !*on;
    }
    let sw = Rect::from_center_size(pos2(ctrl.right() - 20.0, ctrl.center().y), vec2(40.0, 22.0));
    switch(ui, ui.painter(), sw, *on, resp.id);
    flipped
}

fn seg_widths(ui: &Ui, items: &[String]) -> (Vec<f32>, f32) {
    let widths: Vec<f32> = items.iter().map(|t| text_w(ui.painter(), t, bold(12.5)) + 22.0).collect();
    let total = widths.iter().sum::<f32>() + 6.0;
    (widths, total)
}

/// A segmented switch where the layout puts it; the clicked item.
fn seg(ui: &mut Ui, items: &[String], selected: Option<usize>, id: &str) -> Option<usize> {
    let (widths, total) = seg_widths(ui, items);
    let (outer, _) = ui.allocate_exact_size(vec2(total, 30.0), Sense::hover());
    let active = ui.is_enabled();
    let p = ui.painter().clone();
    p.rect_filled(outer, 8.0, pal().bg);
    p.rect_stroke(outer, 8.0, Stroke::new(1.0, pal().border), StrokeKind::Inside);
    let mut x = outer.left() + 3.0;
    let mut clicked = None;
    for (i, (t, w)) in items.iter().zip(&widths).enumerate() {
        let r = Rect::from_min_size(pos2(x, outer.top() + 3.0), vec2(*w, 24.0));
        let sense = if active { Sense::click() } else { Sense::hover() };
        let resp = ui.interact(r, ui.scope_id().with((id, i)), sense);
        let on = selected == Some(i);
        if on {
            p.rect_filled(r, 6.0, pal().card_hi);
            p.rect_stroke(r, 6.0, Stroke::new(1.0, alpha(pal().accent, 110)), StrokeKind::Inside);
        } else if active && resp.hovered() {
            p.rect_filled(r, 6.0, alpha(pal().card_hi, 120));
        }
        let (col, f) = if on { (pal().text, bold(12.5)) } else { (pal().dim, text_font(12.5)) };
        p.text(r.center(), Align2::CENTER_CENTER, t, f, col);
        if active && resp.clicked() {
            clicked = Some(i);
        }
        if active && resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        x += w;
    }
    clicked
}

/// One of a few: the switch beside the label when both fit, under it when not,
/// and as pills on a window too narrow for the switch. The clicked item.
fn choice(
    ui: &mut Ui,
    label: &str,
    hint: &str,
    items: &[String],
    selected: Option<usize>,
    id: &str,
) -> Option<usize> {
    let (_, sw) = seg_widths(ui, items);
    let w = ui.available_width();
    let label_w = text_w(ui.painter(), label, text_font(13.5));
    if w - sw - 16.0 >= label_w.min(170.0) {
        let (_, ctrl) = row(ui, label, hint, sw, Sense::hover());
        return place(ui, ctrl, id, |ui| seg(ui, items, selected, id));
    }
    let (lg, hg, text_h) = texts(ui, label, hint, w);
    let (r, _) = ui.allocate_exact_size(vec2(w, text_h + 18.0), Sense::hover());
    let p = ui.painter();
    let lh = lg.size().y;
    p.galley(pos2(r.left(), r.top() + 12.0), lg, pal().text);
    if let Some(g) = hg {
        p.galley(pos2(r.left(), r.top() + 14.0 + lh), g, pal().faint);
    }
    let picked = if sw <= w {
        let (r, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
        let r = Rect::from_min_size(r.min, vec2(sw, 30.0));
        place(ui, r, id, |ui| seg(ui, items, selected, id))
    } else {
        let pills: Vec<(String, bool, bool)> =
            items.iter().enumerate().map(|(i, t)| (t.clone(), selected == Some(i), true)).collect();
        chips(ui, &pills, id)
    };
    ui.add_space(12.0);
    picked
}

/// Pills that turn on and off, wrapping: (text, on, can be clicked). The clicked one.
fn chips(ui: &mut Ui, items: &[(String, bool, bool)], id: &str) -> Option<usize> {
    let mut clicked = None;
    let active = ui.is_enabled();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        for (i, (t, on, can)) in items.iter().enumerate() {
            let live = active && *can;
            let tw = text_w(ui.painter(), t, text_font(12.5));
            let w = tw + if *on { 42.0 } else { 26.0 };
            let (r, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
            let resp = ui.interact(
                r,
                ui.scope_id().with((id, i)),
                if live { Sense::click() } else { Sense::hover() },
            );
            let hover = live && resp.hovered();
            let (fill, stroke, fg) = match (*on, *can) {
                (true, true) => {
                    (alpha(pal().accent, if hover { 56 } else { 36 }), alpha(pal().accent, 150), pal().text)
                }
                (true, false) => (alpha(pal().accent, 18), alpha(pal().accent, 70), pal().dim),
                (false, true) => (if hover { pal().card_hi } else { pal().bg }, pal().border, pal().dim),
                (false, false) => (pal().bg, pal().grid, pal().faint),
            };
            let p = ui.painter();
            p.rect_filled(r, 15.0, fill);
            p.rect_stroke(r, 15.0, Stroke::new(1.0, stroke), StrokeKind::Inside);
            let mut x = r.left() + 13.0;
            if *on {
                let c = if *can { pal().accent } else { alpha(pal().accent, 120) };
                p.text(pos2(x, r.center().y), Align2::LEFT_CENTER, glyph::CHECK, icon_font(11.0), c);
                x += 16.0;
            }
            p.text(pos2(x, r.center().y), Align2::LEFT_CENTER, t, text_font(12.5), fg);
            if live && resp.clicked() {
                clicked = Some(i);
            }
            if hover {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            }
        }
    });
    clicked
}

/// A small text over pills, a hint under a row.
fn note(ui: &mut Ui, text: &str, top: f32, bottom: f32) {
    ui.add_space(top);
    ui.add(egui::Label::new(RichText::new(text).font(text_font(12.0)).color(pal().faint)).wrap());
    ui.add_space(bottom);
}

/// A button as wide as its text; true when clicked.
fn small_button(ui: &mut Ui, text: &str, accent: bool, id: &str) -> bool {
    let w = text_w(ui.painter(), text, bold(12.0)) + 26.0;
    let (r, _) = ui.allocate_exact_size(vec2(w, 28.0), Sense::hover());
    let clicked = button_at(ui, r, text, accent, false, id);
    clicked && ui.is_enabled()
}

/// A slider with its value as "N%"; with `reset`, a "100%" button beside it.
/// True when the value changed.
fn slider(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: RangeInclusive<f32>,
    step: f64,
    reset: Option<f32>,
    id: &str,
) -> bool {
    let (_, ctrl) = row(ui, label, "", SLIDER_ROW_W, Sense::hover());
    place(ui, ctrl, id, |ui| {
        let mut changed = false;
        match reset {
            Some(v) => {
                if small_button(ui, "100%", false, id) && *value != v {
                    *value = v;
                    changed = true;
                }
            }
            // the button's room, so the rails stand one under another
            None => {
                ui.add_space(text_w(ui.painter(), "100%", bold(12.0)) + 26.0 + ui.spacing().item_spacing.x)
            }
        }
        let t = RichText::new(format!("{:.0}%", value)).font(bold(13.0)).color(pal().text);
        ui.add_sized(vec2(46.0, 20.0), egui::Label::new(t));
        ui.spacing_mut().slider_width = SLIDER_W;
        ui.spacing_mut().slider_rail_height = 6.0;
        // the rail up to the handle in the accent colour
        ui.visuals_mut().selection.bg_fill = pal().accent;
        let s = egui::Slider::new(value, range).step_by(step).show_value(false).trailing_fill(true);
        changed |= ui.add(s).changed();
        changed
    })
}

/// Turns `key` on or off in a list of keys.
fn flip(list: &mut Vec<String>, key: &str) {
    if list.iter().any(|k| k == key) {
        list.retain(|k| k != key);
    } else {
        list.push(key.to_string());
    }
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|t| tr(t)).collect()
}

// ---- the page -----------------------------------------------------------------------------

impl SettingsTab {
    #[allow(clippy::too_many_arguments)]
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        from_widget: Option<&Config>,
        start: Option<&StartInfo>,
        data: Option<&Data>,
        fonts: &mut Fonts,
        theme: Option<&Theme>,
        theme_key: &str,
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
            ui.label(RichText::new(tr("Жду настройки от виджета…")).color(pal().dim));
            return;
        };
        let before = c.clone();
        let mut env = Env { start, data, action: None };
        // the search, and the cards as a line of pills that jump to them
        ui.horizontal(|ui| {
            let field = ui.add_sized(
                vec2(280.0, 30.0),
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text(tr("Найти настройку (Ctrl+F)"))
                    .margin(vec2(10.0, 7.0)),
            );
            if std::mem::take(&mut self.focus_search) {
                field.request_focus();
            }
            if field.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.search.clear();
            }
            if !self.search.is_empty() && ui.button("✕").clicked() {
                self.search.clear();
            }
        });
        // a card asked for (a link elsewhere) must not be hidden by a search
        if JUMP.with(|j| j.borrow().is_some()) {
            self.search.clear();
        }
        let query = self.search.trim().to_lowercase();
        SEARCH.with(|s| *s.borrow_mut() = query.clone());
        if query.is_empty() {
            ui.add_space(4.0);
            let pills: Vec<(String, bool, bool)> = INDEX.iter().map(|(t, _)| (tr(t), false, true)).collect();
            if let Some(i) = chips(ui, &pills, "settings-toc") {
                jump_to(tr(INDEX[i].0));
            }
        } else if !INDEX.iter().any(|(t, _)| found(&tr(t))) {
            ui.add_space(12.0);
            ui.label(RichText::new(tr("Ничего не найдено")).color(pal().dim));
        }
        ui.add_space(GAP / 2.0);
        let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]);
        if let Some(o) = self.start_offset.take() {
            area = area.vertical_scroll_offset(o);
        }
        area.show(ui, |ui| {
            let w = ui.available_width().min(MAX_W);
            ui.set_max_width(w);
            ui.spacing_mut().item_spacing = vec2(GAP, GAP);
            if w >= TWO_COLUMNS {
                ui.columns(2, |cols| {
                    let (left, right) = cols.split_at_mut(1);
                    let (l, r) = (&mut left[0], &mut right[0]);
                    general(l, &mut c);
                    widget(l, &mut c, &mut env);
                    sections(l, &mut c);
                    tray(l, &mut c);
                    startup(l, &mut c, &mut env);
                    games(r, &mut c, &mut env);
                    self.fps_panel(r, &mut c, fonts, theme, theme_key);
                    alerts(r, &mut c);
                    self.ping(r, &mut c);
                    updates(r, &mut c, &mut env);
                });
            } else {
                general(ui, &mut c);
                widget(ui, &mut c, &mut env);
                sections(ui, &mut c);
                tray(ui, &mut c);
                games(ui, &mut c, &mut env);
                self.fps_panel(ui, &mut c, fonts, theme, theme_key);
                alerts(ui, &mut c);
                self.ping(ui, &mut c);
                startup(ui, &mut c, &mut env);
                updates(ui, &mut c, &mut env);
            }
            ui.add_space(4.0);
        });
        if c != before {
            c.sanitize();
            self.changed = Some(Instant::now());
            send(&FromDashboard::SetConfig(Box::new(c.clone())));
        }
        self.edit = Some(c);
        if let Some(a) = env.action {
            send(&a);
        }
    }

    /// Start with `text` in the search (test runs' screenshots).
    pub fn search_for(&mut self, text: String) {
        self.search = text;
    }

    /// The FPS panel: on or off, its look, rows, corner, size, background and a sample.
    fn fps_panel(
        &mut self,
        ui: &mut Ui,
        c: &mut Config,
        fonts: &mut Fonts,
        theme: Option<&Theme>,
        key: &str,
    ) {
        card(
            ui,
            glyph::FPS,
            &tr("FPS-панель"),
            &tr("Поверх игры: FPS и что ещё выбрано"),
            |ui| {
                ui.add_enabled_ui(c.fps, |ui| {
                    toggle(ui, &tr("Показывать FPS-панель (Ctrl+Alt+F)"), "", &mut c.fps_panel);
                    ui.add_enabled_ui(c.fps_panel, |ui| {
                        sep(ui);
                        let looks = ["card", "classic", "line"];
                        let at = looks.iter().position(|k| *k == c.fps_style);
                        let items = strings(&["карточка", "как в 1.8", "строка"]);
                        if let Some(i) = choice(ui, &tr("Вид панели"), "", &items, at, "fps-look") {
                            c.fps_style = looks[i].to_string();
                        }
                        sep(ui);
                        note(ui, &tr("Кроме FPS показывать:"), 10.0, 8.0);
                        let pills: Vec<(String, bool, bool)> = FPS_ROWS
                            .iter()
                            .map(|(k, n)| (tr(n), c.fps_rows.iter().any(|r| r == k), true))
                            .collect();
                        if let Some(i) = chips(ui, &pills, "fps-rows") {
                            flip(&mut c.fps_rows, FPS_ROWS[i].0);
                        }
                        ui.add_space(12.0);
                        sep(ui);
                        let corners = ["tl", "tr", "bl", "br"];
                        let at = corners.iter().position(|k| *k == c.fps_corner);
                        let items =
                            strings(&["слева вверху", "справа вверху", "слева внизу", "справа внизу"]);
                        if let Some(i) = choice(ui, &tr("Угол панели"), "", &items, at, "fps-corner")
                        {
                            c.fps_corner = corners[i].to_string();
                        }
                        sep(ui);
                        let mut size = c.fps_size as f32;
                        let range = config::FPS_SIZE_MIN as f32..=config::FPS_SIZE_MAX as f32;
                        if slider(ui, &tr("Размер панели"), &mut size, range, 5.0, Some(100.0), "fps-size")
                        {
                            c.fps_size = size as u32;
                        }
                        sep(ui);
                        let mut a = (c.fps_bg * 100.0).round();
                        let range = (config::FPS_BG_MIN * 100.0)..=100.0;
                        if slider(ui, &tr("Фон панели"), &mut a, range, 5.0, None, "fps-bg") {
                            c.fps_bg = a / 100.0;
                        }
                        sep(ui);
                        note(ui, &tr("Пример — так панель будет выглядеть поверх игры:"), 10.0, 8.0);
                        self.show_preview(ui, c, fonts, theme, key);
                        ui.add_space(10.0);
                    });
                });
            },
        );
    }

    /// Where the ping goes: a preset or an address of one's own.
    fn ping(&mut self, ui: &mut Ui, c: &mut Config) {
        card(ui, icon::NET, &tr("Пинг"), &tr("Куда отправлять пинг"), |ui| {
            let items: Vec<String> = PING_PRESETS.iter().map(|(h, n)| format!("{h} ({n})")).collect();
            let at = PING_PRESETS.iter().position(|(h, _)| *h == c.ping_host);
            if let Some(i) = choice(ui, &tr("Сервер"), "", &items, at, "ping-preset") {
                c.ping_host = PING_PRESETS[i].0.to_string();
                self.host_text.clear();
                self.host_bad = false;
            }
            sep(ui);
            let (_, ctrl) = row(ui, &tr("Свой адрес:"), "", 260.0, Sense::hover());
            let ok = place(ui, ctrl, "ping-host", |ui| {
                let ok = small_button(ui, "OK", false, "ping-ok");
                let edit =
                    egui::TextEdit::singleline(&mut self.host_text).id_salt("ping-host").desired_width(190.0);
                let r = ui.add(edit);
                ok || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
            });
            if ok {
                match config::clean_host(&self.host_text) {
                    Some(h) => {
                        c.ping_host = h.clone();
                        self.host_text = h;
                        self.host_bad = false;
                    }
                    None => self.host_bad = true,
                }
            }
            if self.host_bad {
                let t = tr("Это не похоже на адрес (пример: eu.battle.net или 8.8.4.4).");
                ui.add(egui::Label::new(RichText::new(t).font(text_font(12.0)).color(pal().hot)).wrap());
                ui.add_space(10.0);
            }
        });
    }

    /// The FPS panel as the settings stand, over a made-up game picture;
    /// drawn again only when something it shows changes.
    fn show_preview(
        &mut self,
        ui: &mut Ui,
        c: &Config,
        fonts: &mut Fonts,
        theme: Option<&Theme>,
        theme_key: &str,
    ) {
        let Some(theme) = theme else { return };
        let ppp = ui.ctx().pixels_per_point();
        let key = format!(
            "{}{:?}{}{}{}{theme_key}{ppp}",
            c.fps_style, c.fps_rows, c.fps_bg, c.fps_size, c.fps_corner
        );
        if self.preview_key != key || self.preview.is_none() {
            let info = sample_game().keep(&c.fps_rows);
            let look = Look::from_name(&c.fps_style);
            let sc = theme.scale();
            let Some(pm) = preview(fonts, ppp, c.fps_size, &sc, &info, look, c.fps_bg, &c.fps_corner) else {
                return;
            };
            let img = egui::ColorImage::from_rgba_premultiplied(
                [pm.width() as usize, pm.height() as usize],
                pm.data(),
            );
            match &mut self.preview {
                Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                slot => *slot = Some(ui.ctx().load_texture("fps-preview", img, egui::TextureOptions::LINEAR)),
            }
            self.preview_key = key;
        }
        if let Some(t) = &self.preview {
            let mut size = t.size_vec2() / ppp;
            let room = ui.available_width();
            if size.x > room {
                size *= room / size.x;
            }
            ui.add(egui::Image::new((t.id(), size)).corner_radius(8.0));
        }
    }
}

// ---- the cards ----------------------------------------------------------------------------

/// The language and the colours of the values.
fn general(ui: &mut Ui, c: &mut Config) {
    card(ui, glyph::GLOBE, &tr("Общие"), &tr("Язык и цвета значений"), |ui| {
        let mut codes = vec!["auto"];
        let mut items = vec![tr("Как в Windows")];
        for (code, name) in pcm_render::format::LANGS {
            codes.push(code);
            items.push(name.to_string());
        }
        let at = codes.iter().position(|k| *k == c.lang);
        if let Some(i) = choice(ui, "Язык / Language", "", &items, at, "lang") {
            c.lang = codes[i].to_string();
        }
        sep(ui);
        let themes = ["heat", "neon", "ice", "custom"];
        let at = themes.iter().position(|k| *k == c.theme);
        let items = strings(&["Тепло", "Неон", "Лёд", "Свои"]);
        if let Some(i) = choice(ui, &tr("Цвета"), "", &items, at, "colours") {
            c.theme = themes[i].to_string();
        }
        if c.theme == "custom" {
            for (i, label) in [tr("Малая нагрузка"), tr("Большая нагрузка")].iter().enumerate()
            {
                sep(ui);
                let (_, ctrl) = row(ui, label, "", 60.0, Sense::hover());
                place(ui, ctrl, &format!("colour-{i}"), |ui| {
                    let mut rgb = rgb_of(&c.custom_colors[i]);
                    if ui.color_edit_button_srgb(&mut rgb).changed() {
                        c.custom_colors[i] = format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
                    }
                });
            }
        }
    });
}

/// The widget: shown or not, its look, its place, size and opacity.
fn widget(ui: &mut Ui, c: &mut Config, env: &mut Env) {
    card(
        ui,
        glyph::WIDGET,
        &tr("Виджет"),
        &tr("Окно со значениями на рабочем столе"),
        |ui| {
            toggle(ui, &tr("Показывать виджет (Ctrl+Alt+W)"), "", &mut c.widget);
            sep(ui);
            let looks = ["cards", "gauges", "classic"];
            let at = looks.iter().position(|k| *k == c.widget_style);
            let items = strings(&["Карточки", "Шкалы", "Классический (как в 1.8)"]);
            if let Some(i) = choice(ui, &tr("Вид"), "", &items, at, "widget-look") {
                c.widget_style = looks[i].to_string();
            }
            sep(ui);
            toggle(ui, &tr("Светлая тема (окно и виджет «Карточки»)"), "", &mut c.window_light);
            sep(ui);
            let zooms = [0.9f32, 1.0, 1.1, 1.25, 1.5];
            let at = zooms.iter().position(|z| (z - c.window_zoom).abs() < 0.01);
            let items: Vec<String> = zooms.iter().map(|z| format!("{:.0}%", z * 100.0)).collect();
            let hint = tr("Или Ctrl+плюс и Ctrl+минус");
            if let Some(i) = choice(ui, &tr("Масштаб окна"), &hint, &items, at, "zoom") {
                c.window_zoom = zooms[i];
            }
            sep(ui);
            toggle(ui, &tr("Закрепить на рабочем столе"), "", &mut c.pinned);
            sep(ui);
            ui.add_enabled_ui(!c.pinned, |ui| toggle(ui, &tr("Поверх всех окон"), "", &mut c.topmost));
            sep(ui);
            toggle(ui, &tr("Компактный режим"), "", &mut c.compact);
            sep(ui);
            let mut size = c.size as f32;
            let range = config::SIZE_MIN as f32..=config::SIZE_MAX as f32;
            if slider(
                ui,
                &tr("Размер"),
                &mut size,
                range,
                config::SIZE_STEP as f64,
                Some(100.0),
                "widget-size",
            ) {
                c.size = size as u32;
            }
            sep(ui);
            let mut a = (c.alpha * 100.0).round();
            if slider(ui, &tr("Непрозрачность"), &mut a, 30.0..=100.0, 5.0, None, "widget-alpha")
            {
                c.alpha = a / 100.0;
            }
            sep(ui);
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if small_button(ui, &tr("Обычная форма"), false, "shape") {
                    env.action = Some(FromDashboard::ResetShape);
                }
                if small_button(ui, &tr("В правый верхний угол"), false, "corner") {
                    env.action = Some(FromDashboard::ResetPosition);
                }
            });
            note(
                ui,
                &tr(
                    "Размер — ещё Ctrl + колёсико над виджетом. Форма — тяни виджет за правый или нижний край.",
                ),
                8.0,
                10.0,
            );
        },
    );
}

/// What the widget shows: a pill per section.
fn sections(ui: &mut Ui, c: &mut Config) {
    card(
        ui,
        glyph::EYE,
        &tr("Показывать в виджете"),
        &tr("Нажми, чтобы скрыть или показать"),
        |ui| {
            ui.add_space(4.0);
            let pills: Vec<(String, bool, bool)> =
                SECTIONS.iter().map(|(k, n)| (tr(n), !c.hidden.iter().any(|h| h == k), true)).collect();
            if let Some(i) = chips(ui, &pills, "sections") {
                flip(&mut c.hidden, SECTIONS[i].0);
            }
            ui.add_space(12.0);
        },
    );
}

/// The tray's icons (one to three) and the taskbar's mini panel.
fn tray(ui: &mut Ui, c: &mut Config) {
    card(
        ui,
        glyph::TRAY,
        &tr("Трей и панель задач"),
        &tr("Значки у часов и мини-панель"),
        |ui| {
            note(ui, &trf("Значки в трее (1–{n}):", &[("n", TRAY_MAX.to_string())]), 4.0, 8.0);
            let pills: Vec<(String, bool, bool)> = TRAY_KINDS
                .iter()
                .map(|(k, n)| {
                    let on = c.tray.iter().any(|t| t == k);
                    // at most TRAY_MAX, at least one
                    let can = if on { c.tray.len() > 1 } else { c.tray.len() < TRAY_MAX };
                    (tr(n), on, can)
                })
                .collect();
            if let Some(i) = chips(ui, &pills, "tray") {
                flip(&mut c.tray, TRAY_KINDS[i].0);
            }
            ui.add_space(12.0);
            sep(ui);
            let mut panel = c.taskbar.unwrap_or_else(|| !pcm_win::system::is_windows_11());
            if toggle(ui, &tr("Мини-панель на панели задач"), "", &mut panel) {
                c.taskbar = Some(panel);
            }
        },
    );
}

/// In games: what the widget does, counting FPS, the profile.
fn games(ui: &mut Ui, c: &mut Config, env: &mut Env) {
    card(
        ui,
        icon::GAME,
        &tr("Игры"),
        &tr("Что делать, когда запущена игра"),
        |ui| {
            let modes = ["hide", "dim", "off"];
            let at = modes.iter().position(|k| *k == c.in_games);
            let items = strings(&["прятать", "тускнеть", "ничего"]);
            if let Some(i) =
                choice(ui, &tr("В играх и видео на весь экран виджет:"), "", &items, at, "in-games")
            {
                c.in_games = modes[i].to_string();
            }
            sep(ui);
            toggle(ui, &tr("Считать FPS (панель, запись сессий)"), "", &mut c.fps);
            sep(ui);
            let profiles = ["work", "game"];
            let at = profiles.iter().position(|k| *k == c.profile);
            let items = strings(&["Работа", "Игра"]);
            let save = tr("Запомнить текущий вид");
            let cw = seg_widths(ui, &items).1 + 8.0 + text_w(ui.painter(), &save, bold(12.0)) + 26.0;
            let (_, ctrl) = row(ui, &tr("Профиль"), "", cw, Sense::hover());
            place(ui, ctrl, "profile", |ui| {
                if small_button(ui, &save, false, "save-profile") {
                    env.action = Some(FromDashboard::SaveProfile);
                }
                if let Some(i) = seg(ui, &items, at, "profile") {
                    c.profile = profiles[i].to_string();
                }
            });
            sep(ui);
            toggle(ui, &tr("Включать «Игру», когда запущена игра"), "", &mut c.auto_game);
        },
    );
}

/// The alerts and their limits.
fn alerts(ui: &mut Ui, c: &mut Config) {
    card(
        ui,
        glyph::ALERT,
        &tr("Предупреждения"),
        &tr("Когда предупредить"),
        |ui| {
            toggle(ui, &tr("Показывать предупреждения"), "", &mut c.alerts);
            sep(ui);
            toggle(ui, &tr("Во время игры — показать после неё"), "", &mut c.quiet_games);
            sep(ui);
            toggle(
                ui,
                &tr("Итоги недели"),
                &tr("Раз в неделю: сколько ПК работал, во что ты играл и с каким FPS"),
                &mut c.weekly,
            );
            ui.add_enabled_ui(c.alerts, |ui| {
                let limits: [(String, &mut u32, [u32; 4], &str, &str); 4] = [
                    (tr("Перегрев CPU от"), &mut c.alert_temp, [80, 85, 90, 95], "°", "alert-temp"),
                    (tr("Память занята от"), &mut c.alert_ram, [80, 85, 90, 95], "%", "alert-ram"),
                    (tr("Батарея ниже"), &mut c.alert_battery, [10, 15, 20, 30], "%", "alert-battery"),
                    (
                        tr("Свободно на диске C: меньше"),
                        &mut c.alert_disk,
                        [5, 10, 15, 20],
                        "%",
                        "alert-disk",
                    ),
                ];
                for (title, value, options, unit, id) in limits {
                    sep(ui);
                    let items: Vec<String> = options.iter().map(|v| format!("{v}{unit}")).collect();
                    let at = options.iter().position(|v| *v == *value);
                    if let Some(i) = choice(ui, &title, "", &items, at, id) {
                        *value = options[i];
                    }
                }
            });
        },
    );
}

/// Starting with Windows, the administrator's rights, what the window's × does.
fn startup(ui: &mut Ui, c: &mut Config, env: &mut Env) {
    card(
        ui,
        glyph::START,
        &tr("Запуск и закрытие"),
        &tr("Автозапуск, права и крестик окна"),
        |ui| {
            if let Some(st) = env.start {
                let mut auto = st.autostart;
                if toggle(ui, &tr("Запускать вместе с Windows"), "", &mut auto) {
                    env.action = Some(FromDashboard::SetAutostart(auto));
                }
                sep(ui);
                if st.helper {
                    let hint = format!(
                        "{}\n{}",
                        tr(
                            "Нужны для температур и напряжений CPU. Windows будет спрашивать разрешение при запуске, кроме автозапуска: он идёт без вопроса из копии в Program Files."
                        ),
                        tr(if st.admin {
                            "Сейчас работает с правами администратора."
                        } else {
                            "Сейчас работает без прав администратора."
                        })
                    );
                    let mut admin = c.run_as_admin;
                    if toggle(ui, &tr("Права администратора (температуры)"), &hint, &mut admin)
                    {
                        c.run_as_admin = admin;
                        // the widget asks UAC: its prompt may come up in front of this window
                        allow_foreground();
                        env.action = Some(FromDashboard::SetAdmin(admin));
                    }
                    sep(ui);
                }
            }
            let hint = if c.close_quits {
                String::new()
            } else {
                format!(
                    "{}\n{}",
                    tr("Крестик закрывает только окно: виджет, значки в трее и мини-панель работают дальше"),
                    tr("Выйти совсем можно через «Выход» в меню значка в трее.")
                )
            };
            toggle(ui, &tr("Крестик полностью закрывает программу"), &hint, &mut c.close_quits);
        },
    );
}

/// Checking for updates, and a new version when there is one.
fn updates(ui: &mut Ui, c: &mut Config, env: &mut Env) {
    let version = format!("PC Monitor {}", crate::APP_VERSION);
    card(ui, glyph::UPDATE, &tr("Обновления"), &version, |ui| {
        toggle(ui, &tr("Проверять обновления"), "", &mut c.check_updates);
        sep(ui);
        ui.add_space(10.0);
        if small_button(ui, &tr("Проверить сейчас"), false, "check-now") {
            env.action = Some(FromDashboard::CheckUpdates);
        }
        ui.add_space(10.0);
        if let Some(d) = env.data
            && let Some(v) = &d.update
        {
            sep(ui);
            let label = trf("Вышла версия {v}", &[("v", v.clone())]);
            let (_, ctrl) = row(ui, &label, "", 240.0, Sense::hover());
            place(ui, ctrl, "update", |ui| match &d.update_state {
                Some(UpdateState::Download(f)) => {
                    let text = trf("скачиваю {p}%", &[("p", format!("{:.0}", f * 100.0))]);
                    ui.add(egui::ProgressBar::new(*f).desired_width(220.0).fill(pal().accent).text(text));
                }
                Some(UpdateState::Install) => {
                    ui.label(RichText::new(tr("устанавливаю…")).font(text_font(13.0)).color(pal().dim));
                }
                None => {
                    let label = if d.update_auto {
                        tr("Обновить")
                    } else {
                        tr("Открыть страницу загрузки")
                    };
                    if small_button(ui, &label, true, "update-now") {
                        env.action = Some(FromDashboard::StartUpdate);
                    }
                }
            });
        }
        ui.add_space(4.0);
    });
}

/// Any process may take the foreground from us now (the widget's UAC prompt).
fn allow_foreground() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};
    unsafe { AllowSetForegroundWindow(ASFW_ANY) };
}

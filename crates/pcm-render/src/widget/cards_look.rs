//! The widget's "cards" look (beta 2, Vlad's concept "Виджет"): a ring, a
//! name, a line and the last minute for the processor, the graphics card,
//! memory and the system drive, then the network and ping; dark or light.
//! Compact: one line a reading, with a bar.
//!
//! The same readings as the PC Monitor window's home cards, smaller.
//!
//! The gauges look (WidgetLook::Gauges) shows the same readings as round
//! gauges, two a row, under the same header and over the same footer.

use super::*;
use crate::canvas::{Align, Canvas, part};
use crate::format::{dec, fmt_disk, fmt_mem, fmt_speed, tr, trf};
use crate::text::{Fonts, Style};
use crate::theme::{DARK_INK, Ink, LIGHT_INK, Rgb, lerp};
use tiny_skia::Pixmap;

/// Logical width of the card look (fixed: Ctrl + wheel scales it).
pub const CARDS_W: f32 = 280.0;
/// Logical width of the gauges look.
pub const GAUGES_W: f32 = 300.0;
const TILE_H: f32 = 122.0;
const TILE_GAP: f32 = 8.0;
pub const COMPACT_W: f32 = 264.0;
const HEAD_H: f32 = 46.0;
const ROW_H: f32 = 60.0;
const FOOT_H: f32 = 38.0;
const LINE_H: f32 = 28.0;

/// One reading of the card look.
struct Reading {
    /// History subject (rows a click opens the window from), if any.
    subject: Option<usize>,
    title: String,
    sub: String,
    /// 0..1 for the ring and the colour.
    t: f32,
    /// The ring's number, "12%".
    value: String,
    hist: Vec<f32>,
    /// The short name for the compact look ("CPU").
    short: String,
    /// The temperature under a gauge ("66°") and its colour's 0..1.
    temp: Option<(String, f32)>,
}

/// "66°" / "≈66°" and its place on the 30..100 ° scale.
fn temp_of(t: Option<f32>, approx: bool) -> Option<(String, f32)> {
    t.map(|t| (format!("{}{t:.0}°", if approx { "≈" } else { "" }), (t - 30.0) / 70.0))
}

fn readings(d: &Data, view: &View) -> Vec<Reading> {
    let hidden = |k: &str| view.hidden.iter().any(|h| h == k);
    let mut out = Vec::new();
    // the processor: always
    let mut sub = Vec::new();
    if let Some(t) = d.temp_cpu {
        sub.push(format!("{}{t:.0}°C", if d.temp_cpu_approx { "≈" } else { "" }));
    }
    if let Some(ghz) = d.cpu_clock_ghz {
        sub.push(trf("{v} ГГц", &[("v", dec(format!("{ghz:.2}")))]));
    }
    out.push(Reading {
        subject: Some(SUBJECT_CPU),
        title: tr("Процессор"),
        sub: sub.join(" · "),
        t: d.cpu / 100.0,
        value: format!("{:.0}%", d.cpu),
        hist: d.cpu_hist.clone(),
        short: "CPU".into(),
        temp: temp_of(d.temp_cpu, d.temp_cpu_approx),
    });
    if let Some(load) = d.gpu_load.filter(|_| !hidden("gpu")) {
        let mut sub = Vec::new();
        match (d.temp_gpu, d.gpu_shared, d.temp_cpu) {
            (Some(t), _, _) => sub.push(format!("{t:.0}°C")),
            (None, true, Some(t)) => sub.push(format!("≈{t:.0}°C")),
            _ => {}
        }
        if let Some(u) = d.vram_used_mb {
            sub.push(format!("VRAM {}", fmt_mem((u as f64 * 1048576.0) as u64)));
        }
        out.push(Reading {
            subject: Some(SUBJECT_GPU),
            title: tr("Видеокарта"),
            sub: sub.join(" · "),
            t: load / 100.0,
            value: format!("{load:.0}%"),
            hist: d.gpu_hist.clone(),
            short: "GPU".into(),
            temp: match (d.temp_gpu, d.gpu_shared) {
                (Some(t), _) => temp_of(Some(t), false),
                (None, true) => temp_of(d.temp_cpu, true),
                _ => None,
            },
        });
    }
    if !hidden("ram") {
        let gb = |b: u64| dec(format!("{:.1}", b as f64 / (1u64 << 30) as f64));
        let total = (d.ram_total as f64 / (1u64 << 30) as f64).round();
        out.push(Reading {
            subject: Some(SUBJECT_RAM),
            title: tr("Память"),
            sub: trf("{used} из {total} ГБ", &[("used", gb(d.ram_used)), ("total", format!("{total:.0}"))]),
            t: d.ram_pct / 100.0,
            value: format!("{:.0}%", d.ram_pct),
            hist: d.ram_hist.clone(),
            short: "RAM".into(),
            temp: None,
        });
    }
    // the system drive, else the first
    if !hidden("disks")
        && let Some((letter, used, total, pct)) =
            d.disks.iter().find(|x| x.0.eq_ignore_ascii_case("C:")).or(d.disks.first())
    {
        let drive = d.drives.iter().find(|x| x.letters.iter().any(|l| l == letter));
        // the ring says how full it is; the line: the SSD's health (it must fit)
        let sub = match drive {
            Some(x) => trf("здоровье {v}%", &[("v", 100u8.saturating_sub(x.wear).to_string())]),
            None => fmt_disk(*used, *total),
        };
        out.push(Reading {
            subject: None,
            title: trf("Диск {d}", &[("d", letter.clone())]),
            sub,
            t: pct / 100.0,
            value: format!("{pct:.0}%"),
            hist: d.io_hist.clone(),
            short: letter.clone(),
            temp: temp_of(drive.and_then(|x| x.temp), false),
        });
    }
    out
}

/// Height of the card look in logical px.
pub fn height(d: &Data, view: &View) -> f32 {
    let n = readings(d, view).len() as f32;
    if view.compact {
        let temp = d.temp_cpu.is_some() as u8 as f32;
        return 12.0 + (n + temp) * LINE_H + 8.0;
    }
    let body = if view.look == WidgetLook::Gauges {
        let rows = (n / 2.0).ceil();
        rows * TILE_H + (rows - 1.0).max(0.0) * TILE_GAP + 4.0
    } else {
        n * ROW_H
    };
    HEAD_H + body + if footer(d, view).is_some() { FOOT_H } else { 10.0 }
}

pub fn width(view: &View) -> f32 {
    if view.compact {
        COMPACT_W
    } else if view.look == WidgetLook::Gauges {
        GAUGES_W
    } else {
        CARDS_W
    }
}

/// "↓ 24,5 МБ/с · ↑ 3,2 МБ/с" and "пинг 14 мс"; None when both are hidden.
fn footer(d: &Data, view: &View) -> Option<(String, String)> {
    let hidden = |k: &str| view.hidden.iter().any(|h| h == k);
    let net = (!hidden("net")).then(|| format!("↓ {} · ↑ {}", fmt_speed(d.net_down), fmt_speed(d.net_up)));
    let ping = (!hidden("ping")).then(|| match (d.ping_ready, d.ping) {
        (false, _) => format!("{} …", tr("пинг")),
        (true, Some(ms)) => format!("{} {ms} {}", tr("пинг"), tr("мс")),
        (true, None) => format!("{} —", tr("пинг")),
    });
    (net.is_some() || ping.is_some()).then(|| (net.unwrap_or_default(), ping.unwrap_or_default()))
}

pub fn render(fonts: &mut Fonts, d: &Data, view: &View) -> Option<Frame> {
    let s = view.scale;
    let w = width(view);
    let h = height(d, view);
    let (pw, ph) = ((w * s).round() as u32, (h * s).round() as u32);
    let ink = if view.light { LIGHT_INK } else { DARK_INK };
    let scale = if view.light { view.theme.scale_dark().on_light() } else { view.theme.scale_dark() };
    let mut x = Ctx { c: Canvas::new(pw, ph, s)?, f: fonts, w, scale, boxes: Boxes::default() };
    // the card: the border, then the fill inside it
    x.c.rrect(0.0, 0.0, w, h, RADIUS, ink.border);
    x.c.rrect(1.0 / s, 1.0 / s, w - 1.0 / s, h - 1.0 / s, RADIUS - 1.0 / s, ink.card);
    let list = readings(d, view);
    if view.compact {
        x.compact_lines(d, &list, &ink);
    } else {
        x.card_header(view, &ink);
        let mut y = HEAD_H;
        if view.look == WidgetLook::Gauges {
            // two a row; an odd last one takes the whole row
            let tw = (w - 2.0 * PAD - TILE_GAP) / 2.0;
            for (i, r) in list.iter().enumerate() {
                let (col, row) = (i % 2, i / 2);
                let alone = i == list.len() - 1 && i % 2 == 0;
                let x0 = PAD + col as f32 * (tw + TILE_GAP);
                let x1 = if alone { w - PAD } else { x0 + tw };
                let y0 = HEAD_H + row as f32 * (TILE_H + TILE_GAP);
                x.gauge_tile(r, x0, y0, x1, y0 + TILE_H, view.hover_row == Some(i), &ink);
            }
            let rows = list.len().div_ceil(2) as f32;
            y += rows * TILE_H + (rows - 1.0).max(0.0) * TILE_GAP + 4.0;
        } else {
            for (i, r) in list.iter().enumerate() {
                x.card_row(r, y, view.hover_row == Some(i), &ink);
                y += ROW_H;
            }
        }
        if let Some((net, ping)) = footer(d, view) {
            x.c.rect(PAD, y + 4.0, w - PAD, y + 4.0 + 1.0 / s, ink.track);
            let base = y + 27.0;
            x.c.text(x.f, PAD, base, &net, Style::Regular, 11.5, ink.dim);
            x.c.rich(x.f, w - PAD, base, &[part(ping, Style::Regular, 11.5, ink.dim)], Align::Right);
        }
    }
    Some(Frame { pm: x.c.pm, boxes: x.boxes })
}

impl Ctx<'_> {
    /// "PC Monitor", the pin, "›" (opens the PC Monitor window) and the ×.
    fn card_header(&mut self, v: &View, ink: &Ink) {
        let w = self.w;
        if v.close_armed {
            self.c.text(
                self.f,
                PAD,
                29.0,
                &tr("Нажми ещё раз — закрыть"),
                Style::SemiBold,
                12.5,
                ink.close_hover,
            );
            self.c.rrect(
                w - PAD - 13.0,
                15.0,
                w - PAD + 5.0,
                33.0,
                5.0,
                lerp(ink.card, ink.close_hover, 0.2),
            );
        } else {
            self.c.text(self.f, PAD, 29.0, "PC Monitor", Style::SemiBold, 14.0, ink.text);
        }
        let cy = 24.0;
        let close_c = if v.close_hover || v.close_armed { ink.close_hover } else { ink.dim };
        self.close_icon(w - PAD - 4.0, cy, close_c, 4.2, 1.5);
        self.boxes.close = self.bx(w - PAD - 14.0, 12.0, w - PAD + 6.0, 36.0);
        // "›": the PC Monitor window
        let ox = w - PAD - 28.0;
        let open_c = if v.open_hover { ink.text } else { ink.dim };
        self.c.line(&[(ox - 2.2, cy - 5.0), (ox + 2.8, cy), (ox - 2.2, cy + 5.0)], 1.6, open_c);
        self.boxes.open = self.bx(ox - 10.0, 12.0, ox + 9.0, 36.0);
        let pin_x = w - PAD - 52.0;
        let mut pin_c = if v.pinned {
            self.scale.at(0.0)
        } else if v.pin_hover {
            ink.text
        } else {
            ink.faint
        };
        if v.pinned && v.pin_hover {
            pin_c = lerp(pin_c, ink.text, 0.35);
        }
        self.pin_icon(pin_x, cy, pin_c);
        self.boxes.pin = self.bx(pin_x - 10.0, 12.0, pin_x + 9.0, 36.0);
    }

    /// The ring with its number, the name and its line, the last minute.
    fn card_row(&mut self, r: &Reading, top: f32, hover: bool, ink: &Ink) {
        let w = self.w;
        if hover {
            self.c.rrect(PAD - 8.0, top + 2.0, w - PAD + 8.0, top + ROW_H - 2.0, 10.0, ink.row_hover);
        }
        if let Some(sub) = r.subject {
            self.boxes.rows.push((sub, self.bx(PAD - 8.0, top + 2.0, w - PAD + 8.0, top + ROW_H - 2.0)));
        } else {
            // the drive: no history page of its own, the window still opens
            self.boxes
                .rows
                .push((usize::MAX, self.bx(PAD - 8.0, top + 2.0, w - PAD + 8.0, top + ROW_H - 2.0)));
        }
        let t = r.t.clamp(0.0, 1.0);
        let col = self.scale.at(t);
        let (cx, cy, rad) = (PAD + 22.0, top + ROW_H / 2.0, 19.5);
        self.c.arc(cx, cy, rad, 0.0, 360.0, 3.5, false, ink.track);
        if t > 0.004 {
            self.c.arc(cx, cy, rad, -90.0, 360.0 * t, 3.5, true, col);
        }
        let num = [part(r.value.clone(), Style::SemiBold, 11.5, col)];
        self.c.rich(self.f, cx, cy + 4.2, &num, Align::Center);
        let tx = PAD + 56.0;
        let spark_x0 = w - PAD - 76.0;
        let room = spark_x0 - 10.0 - tx;
        let title = self.c.ellipsize(self.f, &r.title, Style::SemiBold, 13.0, room);
        self.c.text(self.f, tx, top + 27.0, &title, Style::SemiBold, 13.0, ink.text);
        let sub = self.c.ellipsize(self.f, &r.sub, Style::Regular, 11.5, room);
        self.c.text(self.f, tx, top + 44.0, &sub, Style::Regular, 11.5, ink.dim);
        self.sparkline(spark_x0, cy - 8.0, w - PAD, cy + 8.0, &r.hist, col);
    }

    /// A gauge tile: a 270° arc open at the bottom, the number in it, and
    /// under it the name with the temperature.
    #[allow(clippy::too_many_arguments)]
    fn gauge_tile(&mut self, r: &Reading, x0: f32, y0: f32, x1: f32, y1: f32, hover: bool, ink: &Ink) {
        let fill = if hover { ink.row_hover } else { lerp(ink.card, ink.text, 0.035) };
        self.c.rrect(x0, y0, x1, y1, 12.0, fill);
        let row = self.bx(x0, y0, x1, y1);
        self.boxes.rows.push((r.subject.unwrap_or(usize::MAX), row));
        let t = r.t.clamp(0.0, 1.0);
        let col = self.scale.at(t);
        let (cx, cy, rad) = ((x0 + x1) / 2.0, y0 + 50.0, 33.0);
        self.c.arc(cx, cy, rad, 135.0, 270.0, 7.0, true, ink.track);
        if t > 0.004 {
            self.c.arc(cx, cy, rad, 135.0, 270.0 * t, 7.0, true, col);
        }
        // "37" big, "%" small
        let (num, unit) = match r.value.strip_suffix('%') {
            Some(n) => (n.to_string(), "%"),
            None => (r.value.clone(), ""),
        };
        self.c.rich(
            self.f,
            cx,
            cy + 8.0,
            &[part(num, Style::SemiBold, 22.0, col), part(unit.to_string(), Style::Regular, 12.0, ink.dim)],
            Align::Center,
        );
        // the name and the temperature, in the arc's open bottom and under it
        let mut parts = vec![part(r.title.clone(), Style::Regular, 11.5, ink.dim)];
        if let Some((text, tt)) = &r.temp {
            parts.push(part(" · ".to_string(), Style::Regular, 11.5, ink.faint));
            parts.push(part(text.clone(), Style::SemiBold, 11.5, self.scale.at(tt.clamp(0.0, 1.0))));
        }
        self.c.rich(self.f, cx, y1 - 13.0, &parts, Align::Center);
    }

    /// The last minute as a thin line: its own range, so a calm minute still moves.
    fn sparkline(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, values: &[f32], c: Rgb) {
        if values.len() < 2 {
            self.c.line(&[(x0, (y0 + y1) / 2.0), (x1, (y0 + y1) / 2.0)], 1.4, c);
            return;
        }
        let lo = values.iter().copied().fold(f32::MAX, f32::min);
        let hi = values.iter().copied().fold(f32::MIN, f32::max);
        // at least a quarter of the peak (or 20 units) of range: noise stays small
        let span = (hi - lo).max(hi.abs() * 0.25).max(20.0);
        let mid = (hi + lo) / 2.0;
        let (lo, hi) = (mid - span / 2.0, mid + span / 2.0);
        let n = values.len();
        let pts: Vec<(f32, f32)> = values
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let x = x0 + (x1 - x0) * i as f32 / (n - 1) as f32;
                (x, y1 - (y1 - y0) * ((v - lo) / (hi - lo)).clamp(0.0, 1.0))
            })
            .collect();
        self.c.line(&pts, 1.4, c);
    }

    /// Compact: "CPU ▬▬▬▬── 12%" a line, the CPU temperature last.
    fn compact_lines(&mut self, d: &Data, list: &[Reading], ink: &Ink) {
        let w = self.w;
        let mut lines: Vec<(String, f32, String)> =
            list.iter().map(|r| (r.short.clone(), r.t, r.value.clone())).collect();
        if let Some(t) = d.temp_cpu {
            let approx = if d.temp_cpu_approx { "≈" } else { "" };
            lines.push(("CPU °C".into(), (t - 30.0) / 70.0, format!("{approx}{t:.0}°")));
        }
        let (bar0, bar1) = (PAD + 64.0, w - PAD - 52.0);
        let mut y = 12.0;
        for (label, t, value) in lines {
            let t = t.clamp(0.0, 1.0);
            let col = self.scale.at(t);
            let base = y + LINE_H / 2.0 + 4.0;
            self.c.text(self.f, PAD, base, &label, Style::Regular, 12.0, ink.dim);
            let cy = y + LINE_H / 2.0;
            self.c.rrect(bar0, cy - 3.0, bar1, cy + 3.0, 3.0, ink.track);
            if t > 0.004 {
                self.c.rrect(bar0, cy - 3.0, bar0 + (bar1 - bar0) * t.max(0.03), cy + 3.0, 3.0, col);
            }
            self.c.rich(self.f, w - PAD, base, &[part(value, Style::SemiBold, 12.5, col)], Align::Right);
            y += LINE_H;
        }
    }
}

/// Logical width of the card shown over a tray icon or the taskbar panel.
pub const HOVER_W: f32 = 250.0;

/// The card over a tray icon or the taskbar panel (cards look): a title,
/// "label ...... value" lines and a hint at the bottom.
pub fn hover_card(
    fonts: &mut Fonts,
    scale: f32,
    title: &str,
    rows: &[(String, String, Rgb)],
    hint: &str,
    light: bool,
) -> Option<Pixmap> {
    hover_card_with(fonts, scale, title, rows, hint, light, None)
}

/// Height of the hover card's graph of the last minute.
const HOVER_GRAPH_H: f32 = 40.0;

/// The hover card with the last minute under its lines (iStat Menus' way):
/// `graph` = the values (0..100, oldest first) and their colour.
pub fn hover_card_with(
    fonts: &mut Fonts,
    scale: f32,
    title: &str,
    rows: &[(String, String, Rgb)],
    hint: &str,
    light: bool,
    graph: Option<(&[f32], Rgb)>,
) -> Option<Pixmap> {
    let ink = if light { LIGHT_INK } else { DARK_INK };
    let row_h = 24.0;
    let graph = graph.filter(|(v, _)| v.len() >= 2);
    let graph_h = if graph.is_some() { HOVER_GRAPH_H + 10.0 } else { 0.0 };
    let h = 46.0 + rows.len() as f32 * row_h + graph_h + if hint.is_empty() { 6.0 } else { 30.0 };
    let w = HOVER_W;
    let mut c = Canvas::new((w * scale).round() as u32, (h * scale).round() as u32, scale)?;
    c.rrect(0.0, 0.0, w, h, 12.0, ink.border);
    c.rrect(1.0 / scale, 1.0 / scale, w - 1.0 / scale, h - 1.0 / scale, 12.0 - 1.0 / scale, ink.card);
    c.text(fonts, PAD, 30.0, title, Style::SemiBold, 13.5, ink.text);
    let mut y = 56.0;
    for (label, value, color) in rows {
        let vw = c.width(fonts, value, Style::SemiBold, 12.5);
        let label = c.ellipsize(fonts, label, Style::Regular, 12.5, w - 2.0 * PAD - vw - 10.0);
        c.text(fonts, PAD, y, &label, Style::Regular, 12.5, ink.dim);
        c.rich(fonts, w - PAD, y, &[part(value.clone(), Style::SemiBold, 12.5, *color)], Align::Right);
        y += row_h;
    }
    if let Some((values, col)) = graph {
        // the last minute: a filled line on a soft track, 0..100
        let (x0, x1) = (PAD, w - PAD);
        let (top, bottom) = (y - 6.0, y - 6.0 + HOVER_GRAPH_H);
        c.rrect(x0, top, x1, bottom, 6.0, ink.track);
        let n = values.len();
        let pts: Vec<(f32, f32)> = values
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let x = x0 + 3.0 + (x1 - x0 - 6.0) * i as f32 / (n - 1) as f32;
                (x, bottom - 3.0 - (bottom - top - 6.0) * (v / 100.0).clamp(0.0, 1.0))
            })
            .collect();
        let mut area = pts.clone();
        area.push((pts[n - 1].0, bottom - 3.0));
        area.push((pts[0].0, bottom - 3.0));
        c.polygon(&area, lerp(ink.track, col, 0.35));
        c.line(&pts, 1.5, col);
        y += HOVER_GRAPH_H + 10.0;
    }
    if !hint.is_empty() {
        c.text(fonts, PAD, y + 6.0, hint, Style::Regular, 11.5, ink.faint);
    }
    Some(c.pm)
}

/// A tray icon of the cards look: the number on a square of its colour.
pub fn tray_icon_filled(fonts: &mut Fonts, size: u32, text: &str, color: Rgb) -> Option<Pixmap> {
    let s = size as f32;
    let mut c = Canvas::new(size, size, 1.0)?;
    c.rrect(0.0, 0.0, s, s, s * 0.22, color);
    let luma = 0.299 * color.0 as f32 + 0.587 * color.1 as f32 + 0.114 * color.2 as f32;
    let ink = if luma > 150.0 { (18, 21, 28) } else { (255, 255, 255) };
    let mut px = (s * 0.8).round();
    while px > 6.0 && c.width(fonts, text, Style::SemiBoldCondensed, px) > s * 0.88 {
        px -= 1.0;
    }
    let tw = c.width(fonts, text, Style::SemiBoldCondensed, px);
    let baseline = (s + px * 0.70) / 2.0;
    c.text(fonts, (s - tw) / 2.0, baseline, text, Style::SemiBoldCondensed, px, ink);
    Some(c.pm)
}

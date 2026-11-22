//! The FPS panel over a game, in three looks: the card (big FPS and 1% low,
//! a frame-time graph, then a grid of GPU / CPU / video memory / ping), 1.8's
//! pill (FpsOverlay.render: "FPS 60   low 48" with a small graph and rows
//! under it) and a single line. The settings pick what it shows and how
//! solid its background is.

use tiny_skia::Pixmap;

use crate::canvas::{Align, Canvas, Part, part};
use crate::format::{dec, tr, trf};
use crate::text::{Fonts, Style};
use crate::theme::{BORDER, CARD, DIM, FAINT, Rgb, Scale, TEXT};

/// What the panel shows. Loads in %, temperatures in °C, memory in MB.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OverlayInfo {
    pub fps: f64,
    pub low: Option<f64>,
    /// The latest frame times in ms (up to 48 are drawn).
    pub frame_ms: Vec<f64>,
    pub cpu: Option<f32>,
    pub cpu_t: Option<f32>,
    pub gpu: Option<f32>,
    pub gpu_t: Option<f32>,
    /// The GPU's temperature is the CPU's (an integrated GPU without its own sensor).
    pub gpu_approx: bool,
    pub vram_used_mb: Option<f32>,
    pub vram_total_mb: Option<f32>,
    pub gpu_clock: Option<f32>,
    pub ping: Option<u32>,
}

impl OverlayInfo {
    /// Only what `rows` (keys of the settings' FPS_ROWS) asks for: "low",
    /// "graph", "gpu", "cpu", "vram", "clock", "ping". The FPS always stays.
    pub fn keep(mut self, rows: &[String]) -> OverlayInfo {
        let on = |k: &str| rows.iter().any(|r| r == k);
        if !on("low") {
            self.low = None;
        }
        if !on("graph") {
            self.frame_ms.clear();
        }
        if !on("cpu") {
            (self.cpu, self.cpu_t) = (None, None);
        }
        if !on("gpu") {
            (self.gpu, self.gpu_t, self.gpu_approx) = (None, None, false);
        }
        if !on("vram") {
            (self.vram_used_mb, self.vram_total_mb) = (None, None);
        }
        if !on("clock") {
            self.gpu_clock = None;
        }
        if !on("ping") {
            self.ping = None;
        }
        self
    }
}

/// How the panel looks (the settings' FPS_STYLES).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Look {
    Card,
    Classic,
    Line,
}

impl Look {
    /// "card", "classic", "line"; anything else is the card.
    pub fn from_name(name: &str) -> Look {
        match name {
            "classic" => Look::Classic,
            "line" => Look::Line,
            _ => Look::Card,
        }
    }
}

/// Frame-time graph width of the pill, logical px.
const GRAPH_W: f32 = 52.0;
const BARS: usize = 48;

const LABEL: (Style, f32) = (Style::SemiBold, 11.0);
const VALUE: (Style, f32) = (Style::SemiBoldCondensed, 17.0);
const SMALL: (Style, f32) = (Style::SemiBold, 13.0);

/// The card: width, padding, the value and second columns of the grid.
pub const CARD_W: f32 = 240.0;
const PAD: f32 = 12.0;
const VAL_X: f32 = 56.0;
const COL2: f32 = 134.0;
const ROW_H: f32 = 19.0;
const BIG: (Style, f32) = (Style::SemiBoldCondensed, 28.0);
/// The card's FPS baseline and the graph under it.
const TOP_Y: f32 = 33.0;
const GRAPH_Y0: f32 = 41.0;
const GRAPH_Y1: f32 = 65.0;

pub fn fps_color(scale: &Scale, fps: f64) -> Rgb {
    scale.at(if fps >= 55.0 {
        0.05
    } else if fps >= 30.0 {
        0.55
    } else {
        0.95
    })
}

/// The card's FPS: white while smooth, the theme's warm colours when not.
fn fps_text(scale: &Scale, fps: f64) -> Rgb {
    if fps >= 55.0 { TEXT } else { fps_color(scale, fps) }
}

fn label(t: &str) -> Part {
    part(t, LABEL.0, LABEL.1, DIM)
}

fn small(t: String, c: Rgb) -> Part {
    part(t, SMALL.0, SMALL.1, c)
}

/// The border and the card colour, at `bg` of their opacity (the text and
/// the graph drawn after it stay solid).
fn background(c: &mut Canvas, w: f32, h: f32, r: f32, bg: f32) {
    c.rrect(0.0, 0.0, w, h, r, BORDER);
    let bw = 1.0 / c.s * c.s.round().max(1.0);
    c.rrect(bw, bw, w - bw, h - bw, r - bw, CARD);
    c.fade(bg);
}

/// Frame-time bars from x0 to x1, bottoms on y1: the median reaches
/// `median_at` of the height, a spike (1.8× the median) the top, in red.
fn bars(c: &mut Canvas, frame_ms: &[f64], sc: &Scale, (x0, y0, x1, y1): (f32, f32, f32, f32), gap: f32) {
    let tail = &frame_ms[frame_ms.len().saturating_sub(BARS)..];
    if tail.is_empty() {
        return;
    }
    let mut sorted = tail.to_vec();
    sorted.sort_by(f64::total_cmp);
    let med = sorted[sorted.len() / 2];
    let median_at = if gap > 0.0 { 0.4 } else { 1.0 / 3.0 };
    let top_ms = (med / median_at).max(1.0);
    let step = (x1 - x0) / BARS as f32;
    let px = 1.0 / c.s;
    for (i, ms) in tail.iter().enumerate() {
        let hgt = ((y1 - y0) * (ms / top_ms).min(1.0) as f32).max(px);
        // the card's bars a little softer than the pill's
        let calm = if gap > 0.0 { crate::theme::lerp(sc.at(0.05), CARD, 0.25) } else { sc.at(0.05) };
        let col = if *ms > med * 1.8 { sc.at(0.95) } else { calm };
        let bx = x0 + (BARS - tail.len() + i) as f32 * step;
        // without a gap 1.8 drew each bar a pixel wider than its step: they touch
        let bw = if gap > 0.0 { (step - gap).max(px) } else { step + px };
        c.rect(bx, y1 - hgt, bx + bw, y1 + if gap > 0.0 { 0.0 } else { px }, col);
    }
}

/// "66°C" (≈ when it is the CPU's) in the temperature's colour.
fn temp_parts(t: f32, approx: bool, sc: &Scale) -> Vec<Part> {
    let mut v = Vec::new();
    if approx {
        v.push(small("≈".into(), sc.heat(t)));
    }
    v.push(small(format!("{t:.0}°C"), sc.heat(t)));
    v
}

/// GPU and CPU: (name, load, temperature), the rows of the card's grid.
fn load_rows(info: &OverlayInfo, sc: &Scale) -> Vec<(&'static str, Vec<Part>, Vec<Part>)> {
    let mut rows = Vec::new();
    for (name, load, temp, approx) in
        [("GPU", info.gpu, info.gpu_t, info.gpu_approx), ("CPU", info.cpu, info.cpu_t, false)]
    {
        if load.is_none() && temp.is_none() {
            continue;
        }
        let l = load.map(|l| vec![small(format!("{l:.0}%"), sc.at(l / 100.0))]).unwrap_or_default();
        let t = temp.map(|t| temp_parts(t, approx, sc)).unwrap_or_default();
        rows.push((name, l, t));
    }
    rows
}

/// Video memory, GPU clock, ping: (label, value), two to a row.
fn single_cells(info: &OverlayInfo, sc: &Scale) -> Vec<(String, Part)> {
    let mut cells = Vec::new();
    if let Some(used) = info.vram_used_mb {
        let v = trf("{v} ГБ", &[("v", dec(format!("{:.1}", used / 1024.0)))]);
        cells.push(("VRAM".to_string(), small(v, TEXT)));
    }
    if let Some(clock) = info.gpu_clock.filter(|c| *c > 0.0) {
        cells.push((tr("Частота"), small(trf("{v} МГц", &[("v", format!("{clock:.0}"))]), TEXT)));
    }
    if let Some(p) = info.ping {
        cells.push((tr("пинг"), small(trf("{v} мс", &[("v", p.to_string())]), sc.ping(Some(p)))));
    }
    cells
}

/// The card (the concept's look): "144 FPS        1% 92", the graph, the grid.
fn card(fonts: &mut Fonts, scale: f32, sc: &Scale, info: &OverlayInfo, bg: f32) -> Option<Pixmap> {
    let probe = Canvas::new(1, 1, scale)?;
    let loads = load_rows(info, sc);
    let cells = single_cells(info, sc);
    let grid = loads.len() + cells.len().div_ceil(2);
    let graph = !info.frame_ms.is_empty();
    let fps = format!("{:.0}", info.fps);
    let fps_w = probe.width(fonts, &fps, BIG.0, BIG.1);
    let unit_w = probe.width(fonts, "FPS", LABEL.0, LABEL.1);
    let low: Vec<Part> = info
        .low
        .map(|l| vec![label("1%  "), part(format!("{l:.0}"), Style::SemiBold, 15.0, fps_text(sc, l))])
        .unwrap_or_default();
    // the second column decides if the card has to be wider than CARD_W
    let mut col2 = loads.iter().map(|(_, _, t)| probe.parts_width(fonts, t)).fold(0.0, f32::max);
    for pair in cells.chunks(2) {
        if let Some((l, v)) = pair.get(1) {
            let lw = probe.width(fonts, l, LABEL.0, LABEL.1) + 5.0;
            col2 = col2.max(lw + probe.parts_width(fonts, std::slice::from_ref(v)));
        }
    }
    let w = if graph || grid > 0 {
        CARD_W.max((COL2 + col2 + PAD).ceil())
    } else {
        // only the numbers: as wide as they are
        let low_w = if low.is_empty() { 0.0 } else { 18.0 + probe.parts_width(fonts, &low) };
        (PAD + fps_w + 5.0 + unit_w + low_w + PAD).round()
    };
    let first = if graph { GRAPH_Y1 + 20.0 } else { TOP_Y + 22.0 };
    let h = match (grid, graph) {
        (0, false) => TOP_Y + 12.0,
        (0, true) => GRAPH_Y1 + 12.0,
        (n, _) => first + ROW_H * (n - 1) as f32 + 13.0,
    };
    let mut c = Canvas::new((w * scale).round() as u32, (h * scale).round() as u32, scale)?;
    background(&mut c, w, h, 10.0, bg);
    c.text(fonts, PAD, TOP_Y, &fps, BIG.0, BIG.1, fps_text(sc, info.fps));
    c.text(fonts, PAD + fps_w + 5.0, TOP_Y, "FPS", LABEL.0, LABEL.1, DIM);
    if !low.is_empty() {
        c.rich(fonts, w - PAD, TOP_Y, &low, Align::Right);
    }
    if graph {
        bars(&mut c, &info.frame_ms, sc, (PAD, GRAPH_Y0, w - PAD, GRAPH_Y1), 1.5);
    }
    let mut y = first;
    for (name, load, temp) in &loads {
        c.text(fonts, PAD, y, name, LABEL.0, LABEL.1, DIM);
        c.rich(fonts, VAL_X, y, load, Align::Left);
        c.rich(fonts, COL2, y, temp, Align::Left);
        y += ROW_H;
    }
    for pair in cells.chunks(2) {
        let (l, v) = &pair[0];
        c.text(fonts, PAD, y, l, LABEL.0, LABEL.1, DIM);
        c.rich(fonts, VAL_X, y, std::slice::from_ref(v), Align::Left);
        if let Some((l, v)) = pair.get(1) {
            let lw = c.width(fonts, l, LABEL.0, LABEL.1);
            c.text(fonts, COL2, y, l, LABEL.0, LABEL.1, DIM);
            c.rich(fonts, COL2 + lw + 5.0, y, std::slice::from_ref(v), Align::Left);
        }
        y += ROW_H;
    }
    Some(c.pm)
}

/// One line: "144 FPS   1% 92   GPU 97% 66°C   CPU 44% 62°C   VRAM 7,9 ГБ ▮▮▮".
fn line(fonts: &mut Fonts, scale: f32, sc: &Scale, info: &OverlayInfo, bg: f32) -> Option<Pixmap> {
    let probe = Canvas::new(1, 1, scale)?;
    let gap = || label("    ");
    let mut parts =
        vec![part(format!("{:.0}", info.fps), VALUE.0, 19.0, fps_text(sc, info.fps)), label(" FPS")];
    if let Some(l) = info.low {
        parts.extend([gap(), label("1% "), small(format!("{l:.0}"), fps_text(sc, l))]);
    }
    for (name, load, temp) in load_rows(info, sc) {
        parts.extend([gap(), label(&format!("{name} "))]);
        let both = !load.is_empty() && !temp.is_empty();
        parts.extend(load);
        if both {
            parts.push(label(" "));
        }
        parts.extend(temp);
    }
    for (l, v) in single_cells(info, sc) {
        parts.extend([gap(), label(&format!("{l} ")), v]);
    }
    let graph = !info.frame_ms.is_empty();
    let graph_w = 40.0;
    let text_w = probe.parts_width(fonts, &parts);
    let w = (10.0 + text_w + if graph { 12.0 + graph_w } else { 0.0 } + 10.0).round();
    let h = 28.0;
    let mut c = Canvas::new((w * scale).round() as u32, (h * scale).round() as u32, scale)?;
    background(&mut c, w, h, 8.0, bg);
    c.rich(fonts, 10.0, 20.0, &parts, Align::Left);
    if graph {
        let x0 = 10.0 + text_w + 12.0;
        bars(&mut c, &info.frame_ms, sc, (x0, 7.0, x0 + graph_w, 21.0), 0.0);
    }
    Some(c.pm)
}

/// "CPU 45% · 66°     GPU 23% · ≈66°"
fn system_parts(info: &OverlayInfo, sc: &Scale) -> Vec<Part> {
    let mut parts = Vec::new();
    for (name, load, temp, approx) in
        [("CPU", info.cpu, info.cpu_t, false), ("GPU", info.gpu, info.gpu_t, info.gpu_approx)]
    {
        if load.is_none() && temp.is_none() {
            continue;
        }
        if !parts.is_empty() {
            parts.push(label("     "));
        }
        parts.push(label(&format!("{name} ")));
        if let Some(l) = load {
            parts.push(small(format!("{l:.0}%"), sc.at(l / 100.0)));
        }
        if let Some(t) = temp {
            // a dot between load and temperature, so "45% 66°" don't run together
            parts.push(part(if load.is_some() { "  ·  " } else { " " }, LABEL.0, LABEL.1, FAINT));
            if approx {
                parts.push(part("≈", LABEL.0, 12.0, sc.heat(t)));
            }
            parts.push(small(format!("{t:.0}°"), sc.heat(t)));
        }
    }
    parts
}

/// The pill's rows under the FPS; the ping joins the first one when there
/// is no video memory or clock to show. Empty rows are left out.
fn system_rows(info: &OverlayInfo, sc: &Scale) -> Vec<Vec<Part>> {
    let gap = || label("     ");
    let mut cpu_gpu = system_parts(info, sc);
    let mut extra: Vec<Part> = Vec::new();
    if let (Some(used), Some(total)) = (info.vram_used_mb, info.vram_total_mb)
        && total > 0.0
    {
        extra.push(label("VRAM "));
        extra.push(small(format!("{:.1}", used / 1024.0), sc.at((used / total).min(1.0))));
        extra.push(label(&format!("/{:.1} GB", total / 1024.0)));
    }
    if let Some(clock) = info.gpu_clock.filter(|c| *c > 0.0) {
        if !extra.is_empty() {
            extra.push(gap());
        }
        extra.extend([label("GPU "), small(format!("{clock:.0}"), TEXT), label(" MHz")]);
    }
    if let Some(ping) = info.ping {
        // English like FPS / low / CPU / GPU in this pill, whatever the language
        let row = if extra.is_empty() { &mut cpu_gpu } else { &mut extra };
        if !row.is_empty() {
            row.push(gap());
        }
        row.extend([label("ping "), small(ping.to_string(), sc.ping(Some(ping))), label(" ms")]);
    }
    [cpu_gpu, extra].into_iter().filter(|r| !r.is_empty()).collect()
}

/// Draw parts from x on a baseline; the big FPS digits sit 1 px lower.
fn row(c: &mut Canvas, fonts: &mut Fonts, mut x: f32, baseline: f32, parts: &[Part]) -> f32 {
    for (t, st, sz, col) in parts {
        let y = baseline + if *sz == VALUE.1 { 1.0 } else { 0.0 };
        c.text(fonts, x, y, t, *st, *sz, *col);
        x += c.width(fonts, t, *st, *sz);
    }
    x
}

/// 1.8's pill.
fn classic(fonts: &mut Fonts, scale: f32, sc: &Scale, info: &OverlayInfo, bg: f32) -> Option<Pixmap> {
    let probe = Canvas::new(1, 1, scale)?;
    let mut top =
        vec![label("FPS "), part(format!("{:.0}", info.fps), VALUE.0, VALUE.1, fps_color(sc, info.fps))];
    if let Some(low) = info.low {
        top.push(label("   low "));
        top.push(part(format!("{low:.0}"), VALUE.0, VALUE.1, fps_color(sc, low)));
    }
    let rows = system_rows(info, sc);
    let graph = !info.frame_ms.is_empty();
    let top_w = probe.parts_width(fonts, &top) + if graph { GRAPH_W + 10.0 } else { 0.0 };
    let widest = rows.iter().map(|r| probe.parts_width(fonts, r)).fold(top_w, f32::max);
    let w = (widest + 22.0).round();
    let h = 30.0 + 18.0 * rows.len() as f32;
    let mut c = Canvas::new((w * scale).round() as u32, (h * scale).round() as u32, scale)?;
    background(&mut c, w, h, 12.0, bg);
    let x = row(&mut c, fonts, 11.0, 20.0, &top);
    if graph {
        bars(&mut c, &info.frame_ms, sc, (x + 10.0, 8.0, x + 10.0 + GRAPH_W, 22.0), 0.0);
    }
    for (i, r) in rows.iter().enumerate() {
        row(&mut c, fonts, 11.0, 39.0 + 18.0 * i as f32, r);
    }
    Some(c.pm)
}

/// The panel at `scale` (Windows' scale × the panel size from the
/// settings), its background at `bg` (0..1) of full opacity.
pub fn fps_overlay(
    fonts: &mut Fonts,
    scale: f32,
    sc: &Scale,
    info: &OverlayInfo,
    look: Look,
    bg: f32,
) -> Option<Pixmap> {
    match look {
        Look::Card => card(fonts, scale, sc, info, bg),
        Look::Classic => classic(fonts, scale, sc, info, bg),
        Look::Line => line(fonts, scale, sc, info, bg),
    }
}

/// The panel over a made-up game picture, in `corner` ("tl", "tr", "bl",
/// "br") like over a game window: the settings' preview, where the
/// background's opacity shows. `ppp`: physical px per logical px; `size`:
/// the panel's size in %.
pub fn preview(
    fonts: &mut Fonts,
    ppp: f32,
    size: u32,
    sc: &Scale,
    info: &OverlayInfo,
    look: Look,
    bg: f32,
    corner: &str,
) -> Option<Pixmap> {
    use tiny_skia::{
        Color, GradientStop, LinearGradient, Paint, PixmapPaint, Point, Rect, SpreadMode, Transform,
    };
    let scale = ppp * size as f32 / 100.0;
    let panel = fps_overlay(fonts, scale, sc, info, look, bg)?;
    let m = (12.0 * scale).round();
    let w = (440.0 * ppp).max(panel.width() as f32 + 2.0 * m).round();
    let h = (248.0 * ppp).max(panel.height() as f32 + 2.0 * m).round();
    let mut c = Canvas::new(w as u32, h as u32, ppp)?;
    let (lw, lh) = (w / ppp, h / ppp);
    // sky down to a warm horizon, a sun, two ridges, a field
    let stops = vec![
        GradientStop::new(0.0, Color::from_rgba8(36, 62, 104, 255)),
        GradientStop::new(0.55, Color::from_rgba8(104, 150, 204, 255)),
        GradientStop::new(1.0, Color::from_rgba8(236, 190, 128, 255)),
    ];
    let sky = LinearGradient::new(
        Point::from_xy(0.0, 0.0),
        Point::from_xy(0.0, h * 0.62),
        stops,
        SpreadMode::Pad,
        Transform::identity(),
    )?;
    let paint = Paint { shader: sky, anti_alias: false, ..Paint::default() };
    c.pm.fill_rect(Rect::from_xywh(0.0, 0.0, w, h)?, &paint, Transform::identity(), None);
    c.ellipse(lw * 0.68 - 26.0, lh * 0.5 - 26.0, lw * 0.68 + 26.0, lh * 0.5 + 26.0, (255, 236, 196));
    let ridge = |k: f32, y: f32| -> Vec<(f32, f32)> {
        let mut pts = vec![(0.0, lh)];
        for i in 0..=16 {
            let x = lw * i as f32 / 16.0;
            let wave = ((i as f32 * k).sin() * 0.5 + (i as f32 * k * 2.3).cos() * 0.3) * 22.0;
            pts.push((x, y + wave));
        }
        pts.push((lw, lh));
        pts
    };
    c.polygon(&ridge(0.9, lh * 0.58), (74, 92, 120));
    c.polygon(&ridge(1.7, lh * 0.7), (46, 60, 72));
    c.rect(0.0, lh * 0.82, lw, lh, (38, 52, 40));
    let (pw, ph) = (panel.width() as f32, panel.height() as f32);
    let x = if corner.ends_with('l') { m } else { w - pw - m };
    let y = if corner.starts_with('t') { m } else { h - ph - m };
    c.pm.draw_pixmap(
        x as i32,
        y as i32,
        panel.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );
    Some(c.pm)
}

/// A fixed panel for the snapshot test and the preview (1.8's numbers).
pub fn sample_overlay() -> OverlayInfo {
    let frame_ms = (0..48).map(|i| if i % 17 == 9 { 31.0 } else { 16.0 + (i % 5) as f64 * 0.6 }).collect();
    OverlayInfo {
        fps: 60.0,
        low: Some(41.0),
        frame_ms,
        cpu: Some(45.0),
        cpu_t: Some(66.0),
        gpu: Some(93.0),
        gpu_t: Some(66.0),
        gpu_approx: true,
        vram_used_mb: Some(1300.0),
        vram_total_mb: Some(2048.0),
        gpu_clock: Some(1800.0),
        ping: Some(24),
    }
}

/// The concept's numbers: a fast game on a discrete card (the settings' preview).
pub fn sample_game() -> OverlayInfo {
    let frame_ms =
        (0..48).map(|i| if i == 19 || i == 38 { 21.0 } else { 6.9 + ((i * 7) % 5) as f64 * 0.15 }).collect();
    OverlayInfo {
        fps: 144.0,
        low: Some(92.0),
        frame_ms,
        cpu: Some(44.0),
        cpu_t: Some(62.0),
        gpu: Some(97.0),
        gpu_t: Some(66.0),
        gpu_approx: false,
        vram_used_mb: Some(7.9 * 1024.0),
        vram_total_mb: Some(12.0 * 1024.0),
        gpu_clock: Some(2610.0),
        ping: Some(14),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_the_chosen_rows() {
        let all = sample_game();
        let k = all.clone().keep(&["low".into(), "ping".into()]);
        assert_eq!((k.fps, k.low, k.ping), (144.0, Some(92.0), Some(14)));
        assert!(k.frame_ms.is_empty() && k.cpu.is_none() && k.gpu_t.is_none() && k.vram_used_mb.is_none());
        let rows: Vec<String> =
            ["low", "graph", "gpu", "cpu", "vram", "clock", "ping"].map(String::from).into();
        assert_eq!(all.clone().keep(&rows), all);
        assert_eq!(Look::from_name("line"), Look::Line);
        assert_eq!(Look::from_name("?"), Look::Card);
    }

    #[test]
    fn sizes_follow_what_is_shown() {
        let mut f = Fonts::system();
        if f.width(crate::text::Font::new(Style::Regular, 12.0), "A") == 0.0 {
            return; // no Windows fonts here
        }
        let sc = crate::theme::Theme::Heat.scale();
        let rows: Vec<String> = ["low", "graph", "gpu", "cpu", "vram", "ping"].map(String::from).into();
        let shown = sample_game().keep(&rows);
        let full = fps_overlay(&mut f, 1.0, &sc, &shown, Look::Card, 1.0).unwrap();
        assert_eq!((full.width(), full.height()), (240, 136), "graph + GPU, CPU + VRAM | ping");
        let only_fps = sample_game().keep(&[]);
        let small = fps_overlay(&mut f, 1.0, &sc, &only_fps, Look::Card, 1.0).unwrap();
        assert!(small.width() < 120 && small.height() == 45, "{}x{}", small.width(), small.height());
        let line = fps_overlay(&mut f, 2.0, &sc, &sample_game(), Look::Line, 0.5).unwrap();
        assert_eq!(line.height(), 56);
        // a half-solid background: the corner pixel inside the border is half as opaque
        let px = full.pixel(6, 100).unwrap().alpha();
        let faded = fps_overlay(&mut f, 1.0, &sc, &shown, Look::Card, 0.5).unwrap();
        let half = faded.pixel(6, 100).unwrap().alpha();
        assert_eq!(px, 255);
        assert!((126..=129).contains(&half), "{half}");
    }
}

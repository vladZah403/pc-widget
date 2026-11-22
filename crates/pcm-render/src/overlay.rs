//! The FPS panel over a game (1.8's FpsOverlay.render): "FPS 60   low 48"
//! with a graph of the latest frame times, then CPU / GPU load and
//! temperature, video memory, GPU clock and ping.

use tiny_skia::Pixmap;

use crate::canvas::{Canvas, Part, part};
use crate::text::{Fonts, Style};
use crate::theme::{BORDER, CARD, DIM, FAINT, Scale, TEXT};

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

/// Frame-time graph width, logical px.
const GRAPH_W: f32 = 52.0;
const BARS: usize = 48;

const LABEL: (Style, f32) = (Style::SemiBold, 11.0);
const VALUE: (Style, f32) = (Style::SemiBoldCondensed, 17.0);
const SMALL: (Style, f32) = (Style::SemiBold, 13.0);

pub fn fps_color(scale: &Scale, fps: f64) -> crate::theme::Rgb {
    scale.at(if fps >= 55.0 {
        0.05
    } else if fps >= 30.0 {
        0.55
    } else {
        0.95
    })
}

fn label(t: &str) -> Part {
    part(t, LABEL.0, LABEL.1, DIM)
}

fn small(t: String, c: crate::theme::Rgb) -> Part {
    part(t, SMALL.0, SMALL.1, c)
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

/// The rows under the FPS; the ping joins the first one when there is no
/// video memory or clock to show. Empty rows are left out.
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

/// The panel at `scale` (Windows' scale × the panel size from the settings).
pub fn fps_overlay(fonts: &mut Fonts, scale: f32, sc: &Scale, info: &OverlayInfo) -> Option<Pixmap> {
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
    c.rrect(0.0, 0.0, w, h, 12.0, BORDER);
    let bw = 1.0 / scale * scale.round().max(1.0);
    c.rrect(bw, bw, w - bw, h - bw, 12.0 - bw, CARD);
    let x = row(&mut c, fonts, 11.0, 20.0, &top);
    if graph {
        // bars of frame time: the median is a third of the height, a spike reaches the top
        let tail = &info.frame_ms[info.frame_ms.len().saturating_sub(BARS)..];
        let mut sorted = tail.to_vec();
        sorted.sort_by(f64::total_cmp);
        let med = sorted[sorted.len() / 2];
        let top_ms = (med * 3.0).max(1.0);
        let (gx0, gy0, gy1) = (x + 10.0, 8.0, 22.0);
        let bar = GRAPH_W / BARS as f32;
        for (i, ms) in tail.iter().enumerate() {
            let hgt = ((gy1 - gy0) * (ms / top_ms).min(1.0) as f32).max(1.0 / scale);
            let spike = *ms > med * 1.8;
            let x0 = gx0 + i as f32 * bar;
            let col = if spike { sc.at(0.95) } else { sc.at(0.05) };
            // 1.8 drew each bar a pixel wider than its step: the bars touch
            c.rect(x0, gy1 - hgt, x0 + bar + 1.0 / scale, gy1 + 1.0 / scale, col);
        }
    }
    for (i, r) in rows.iter().enumerate() {
        row(&mut c, fonts, 11.0, 39.0 + 18.0 * i as f32, r);
    }
    Some(c.pm)
}

/// A fixed panel for the snapshot test and the preview.
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

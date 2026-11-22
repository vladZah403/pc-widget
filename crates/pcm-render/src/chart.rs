//! A history chart (stage 5): a panel with a title, a legend, the scale's
//! top and bottom, lines of averages over shaded peaks, time labels below.

use tiny_skia::Pixmap;

use crate::canvas::{Align, Canvas, part};
use crate::format::tr;
use crate::text::{Fonts, Style};
use crate::theme::*;

/// One line: per column the (average, peak) or None where there is no data.
pub struct Series {
    pub name: String,
    pub color: Rgb,
    pub values: Vec<Option<(f64, f64)>>,
}

pub struct Chart {
    pub title: String,
    pub series: Vec<Series>,
    /// The scale's bottom; the top is `hi` or the highest peak (at least `min_hi`).
    pub lo: f64,
    pub hi: Option<f64>,
    pub min_hi: f64,
    /// Labels under the plot: left, middle, right.
    pub times: [String; 3],
    /// How a value on the scale is written ("45%", "66°", "1,2 МБ/с").
    pub fmt: fn(f64) -> String,
}

/// Logical px around the plot.
const LEFT: f32 = 70.0;
const RIGHT: f32 = 14.0;
const TOP: f32 = 30.0;
const BOTTOM: f32 = 24.0;

/// Columns to bucket the points into for a chart `w` logical px wide at `scale`.
pub fn columns(w: f32, scale: f32) -> usize {
    (((w - LEFT - RIGHT) * scale) / 2.0).max(10.0) as usize
}

pub fn chart(fonts: &mut Fonts, scale: f32, w: f32, h: f32, spec: &Chart) -> Option<Pixmap> {
    let mut c = Canvas::new((w * scale).round() as u32, (h * scale).round() as u32, scale)?;
    c.rrect(0.0, 0.0, w, h, 12.0, HIST_TRACK);
    c.text(fonts, 14.0, 20.0, &spec.title, Style::SemiBold, 12.0, DIM);
    // the legend, right to left
    let mut x = w - RIGHT;
    for s in spec.series.iter().rev() {
        let tw = c.width(fonts, &s.name, Style::Regular, 11.0);
        c.text(fonts, x - tw, 20.0, &s.name, Style::Regular, 11.0, DIM);
        x -= tw + 6.0;
        c.rrect(x - 8.0, 12.0, x, 18.0, 2.0, s.color);
        x -= 18.0;
    }
    let (x0, y0, x1, y1) = (LEFT, TOP, w - RIGHT, h - BOTTOM);
    let peak =
        spec.series.iter().flat_map(|s| s.values.iter().flatten().map(|v| v.1)).fold(f64::MIN, f64::max);
    let hi = spec.hi.unwrap_or_else(|| peak.max(spec.min_hi) * 1.1);
    let lo = spec.lo;
    let y = |v: f64| y1 - (y1 - y0) * (((v - lo) / (hi - lo).max(1e-9)).clamp(0.0, 1.0)) as f32;
    for v in [hi, lo] {
        let yy = y(v);
        c.rect(x0, yy - 0.5 / scale, x1, yy + 0.5 / scale, TRACK);
        c.rich(fonts, x0 - 8.0, yy + 3.5, &[part((spec.fmt)(v), Style::Regular, 10.0, FAINT)], Align::Right);
    }
    let has_data = spec.series.iter().any(|s| s.values.iter().any(|v| v.is_some()));
    if !has_data {
        let msg = part(tr("нет данных за это время"), Style::Regular, 11.0, FAINT);
        c.rich(fonts, (x0 + x1) / 2.0, (y0 + y1) / 2.0 + 4.0, &[msg], Align::Center);
    }
    for s in &spec.series {
        let n = s.values.len().max(1);
        let xc = |i: usize| x0 + (x1 - x0) * (i as f32 + 0.5) / n as f32;
        let shade = lerp(HIST_TRACK, s.color, 0.28);
        // peaks: a shaded band from the average up to the peak
        let bw = ((x1 - x0) / n as f32).max(1.0 / scale);
        for (i, v) in s.values.iter().enumerate() {
            if let Some((avg, pk)) = v
                && pk > avg
            {
                let xx = xc(i) - bw / 2.0;
                c.rect(xx, y(*pk), xx + bw, y(*avg), shade);
            }
        }
        // averages: a line, broken where there is no data
        let mut run: Vec<(f32, f32)> = Vec::new();
        for (i, v) in s.values.iter().enumerate() {
            match v {
                Some((avg, _)) => run.push((xc(i), y(*avg))),
                None => {
                    flush(&mut c, &mut run, s.color);
                }
            }
        }
        flush(&mut c, &mut run, s.color);
    }
    let ty = h - 8.0;
    c.rich(fonts, x0, ty, &[part(spec.times[0].clone(), Style::Regular, 10.0, FAINT)], Align::Left);
    c.rich(
        fonts,
        (x0 + x1) / 2.0,
        ty,
        &[part(spec.times[1].clone(), Style::Regular, 10.0, FAINT)],
        Align::Center,
    );
    c.rich(fonts, x1, ty, &[part(spec.times[2].clone(), Style::Regular, 10.0, FAINT)], Align::Right);
    Some(c.pm)
}

fn flush(c: &mut Canvas, run: &mut Vec<(f32, f32)>, col: Rgb) {
    match run.len() {
        0 => {}
        1 => {
            let (x, y) = run[0];
            c.ellipse(x - 1.2, y - 1.2, x + 1.2, y + 1.2, col);
        }
        _ => c.line(run, 1.5, col),
    }
    run.clear();
}

//! A game session as one picture to share (the report's "Поделиться"): the
//! game, the FPS and its lows, stutters, the hottest temperature, the FPS
//! over the session, the PC. 1200 × 630 at scale 2 — the shape chats and
//! sites show whole.

use pcm_core::frames::FRAME_PAUSE_S;
use pcm_core::session::{Session, stutter_ms};
use tiny_skia::Pixmap;

use crate::canvas::{Align, Canvas, part};
use crate::format::{dec, tr, trf};
use crate::overlay::fps_color;
use crate::report::fmt_clock;
use crate::text::{Fonts, Style};
use crate::theme::*;

/// Logical size (× scale for pixels).
pub const SHARE_W: f32 = 600.0;
pub const SHARE_H: f32 = 315.0;
const PAD: f32 = 26.0;
const CHART_X: f32 = 230.0;

/// "2026-11-22 13:05:41" -> "22.11.2026"
fn date(started: &str) -> String {
    let d: Vec<&str> = started.get(..10).unwrap_or("").split('-').collect();
    if d.len() == 3 { format!("{}.{}.{}", d[2], d[1], d[0]) } else { started.to_string() }
}

/// Stutters (frames longer than the report's threshold) in % of the frames played.
fn stutter_share(s: &Session) -> f64 {
    let thr = stutter_ms(s.info.summary.median_ms);
    let pause = (FRAME_PAUSE_S * 1000.0) as f32;
    let played = s.frame_ms.iter().filter(|m| **m > 0.0 && **m < pause);
    let (n, bad) = played.fold((0u64, 0u64), |(n, b), m| (n + 1, b + u64::from(*m > thr)));
    if n == 0 { 0.0 } else { bad as f64 * 100.0 / n as f64 }
}

pub fn share_card(fonts: &mut Fonts, scale: f32, sc: &Scale, s: &Session) -> Option<Pixmap> {
    let (w, h) = (SHARE_W, SHARE_H);
    let mut c = Canvas::new((w * scale).round() as u32, (h * scale).round() as u32, scale)?;
    // opaque all over: a chat shows it on any background
    c.rect(0.0, 0.0, w, h, ink().card);
    c.rect(0.0, 0.0, w, 4.0, sc.at(0.05));
    let info = &s.info;
    let sum = &info.summary;

    // the game, when, how long, the window
    let title = if info.title.trim().is_empty() { info.game.clone() } else { info.title.clone() };
    let title = c.ellipsize(fonts, &title, Style::SemiBold, 21.0, w - 2.0 * PAD);
    c.text(fonts, PAD, 44.0, &title, Style::SemiBold, 21.0, ink().text);
    let mut sub = vec![date(&info.started), fmt_clock(sum.active_s)];
    if info.width > 0 {
        let mut win = format!("{}×{}", info.width, info.height);
        if info.refresh_hz > 0 {
            win += &trf(" @ {v} Гц", &[("v", info.refresh_hz.to_string())]);
        }
        sub.push(win);
    }
    c.text(fonts, PAD, 66.0, &sub.join("  ·  "), Style::Regular, 12.5, ink().dim);

    // the average, big, and the numbers under it
    c.rich(
        fonts,
        PAD,
        142.0,
        &[
            part(format!("{:.0}", sum.avg_fps), Style::SemiBoldCondensed, 64.0, fps_color(sc, sum.avg_fps)),
            part(tr("  FPS в среднем"), Style::Regular, 12.5, ink().dim),
        ],
        Align::Left,
    );
    let share = stutter_share(s);
    let share_text = if share <= 0.0 {
        "0%".to_string()
    } else if share < 0.1 {
        format!("<{}%", dec("0.1".into()))
    } else {
        format!("{}%", dec(format!("{share:.1}")))
    };
    let peak =
        |f: fn(&pcm_core::session::Sample) -> Option<f32>| info.samples.iter().filter_map(f).reduce(f32::max);
    let temp = match (peak(|x| x.cpu_t), peak(|x| x.gpu_t)) {
        (Some(c), Some(g)) if c > g => Some((c, "CPU")),
        (_, Some(g)) => Some((g, "GPU")),
        (Some(c), None) => Some((c, "CPU")),
        _ => None,
    };
    let mut rows: Vec<(String, String, Rgb)> = vec![
        ("1% low".into(), format!("{:.0}", sum.low1), fps_color(sc, sum.low1)),
        ("0.1% low".into(), format!("{:.0}", sum.low01), fps_color(sc, sum.low01)),
        (tr("Время кадра"), trf("{v} мс", &[("v", dec(format!("{:.1}", sum.median_ms)))]), ink().text),
        (tr("Подтормаживания"), share_text, if share >= 3.0 { sc.at(0.95) } else { ink().text }),
    ];
    if let Some((t, what)) = temp {
        rows.push((tr("Пик температуры"), format!("{t:.0}°C {what}"), sc.heat(t)));
    }
    let mut y = 178.0;
    let right = CHART_X - 24.0;
    for (label, value, col) in &rows {
        c.text(fonts, PAD, y, label, Style::Regular, 12.0, ink().dim);
        c.rich(fonts, right, y, &[part(value.clone(), Style::SemiBold, 12.0, *col)], Align::Right);
        y += 21.0;
    }

    // FPS over the session, the monitor's refresh rate as a line
    let (x0, y0, x1, y1) = (CHART_X, 92.0, w - PAD, 252.0);
    c.rrect(x0, y0, x1, y1, 10.0, ink().hist_track);
    c.text(fonts, x0 + 12.0, y0 + 18.0, &tr("Кадры в секунду"), Style::SemiBold, 11.5, ink().dim);
    let pts: Vec<(f32, f32)> =
        info.samples.iter().filter_map(|x| Some((x.t, x.fps.filter(|f| *f > 0.0)?))).collect();
    let top = pts.iter().map(|p| p.1).fold(info.refresh_hz as f32, f32::max).max(30.0);
    let hi = (top * 1.1 / 30.0).ceil() * 30.0;
    let span = info.duration_s.max(1.0) as f32;
    let (px0, py0, px1, py1) = (x0 + 36.0, y0 + 30.0, x1 - 12.0, y1 - 12.0);
    let xy = |t: f32, v: f32| {
        (px0 + (px1 - px0) * (t / span).clamp(0.0, 1.0), py1 - (py1 - py0) * (v / hi).clamp(0.0, 1.0))
    };
    for v in [0.0, hi / 2.0, hi] {
        let (_, yy) = xy(0.0, v);
        c.rect(px0, yy - 0.5 / scale, px1, yy + 0.5 / scale, ink().track);
        let label = format!("{v:.0}");
        let lw = c.width(fonts, &label, Style::Regular, 10.0);
        c.text(fonts, px0 - 6.0 - lw, yy + 3.5, &label, Style::Regular, 10.0, ink().faint);
    }
    if info.refresh_hz > 0 {
        let (_, yy) = xy(0.0, info.refresh_hz as f32);
        c.rect(px0, yy - 0.5 / scale, px1, yy + 0.5 / scale, ink().faint);
    }
    // at most two points per pixel column: a 3-hour session stays a line
    let step = (pts.len() / ((px1 - px0) * 2.0) as usize).max(1);
    let line: Vec<(f32, f32)> = pts
        .chunks(step)
        .map(|ch| {
            let avg = ch.iter().map(|p| p.1).sum::<f32>() / ch.len() as f32;
            xy(ch[0].0, avg)
        })
        .collect();
    if line.len() >= 2 {
        c.line(&line, 1.6, sc.at(0.05));
    }

    // the PC, and where the picture comes from
    let mut pc = vec![info.cpu.clone(), info.gpu.clone()];
    if info.ram_gb > 0.0 {
        pc.push(trf("ОЗУ {v} ГБ", &[("v", format!("{:.0}", info.ram_gb))]));
    }
    pc.retain(|v| !v.trim().is_empty());
    let brand = "PC Monitor";
    let bw = c.width(fonts, brand, Style::SemiBold, 12.0);
    let pc = c.ellipsize(fonts, &pc.join("  ·  "), Style::Regular, 11.5, w - 2.0 * PAD - bw - 24.0);
    c.text(fonts, PAD, h - 26.0, &pc, Style::Regular, 11.5, ink().dim);
    c.text(fonts, w - PAD - bw, h - 26.0, brand, Style::SemiBold, 12.0, ink().faint);
    Some(c.pm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_size_and_stutters() {
        let s = crate::report::sample_session();
        let mut fonts = Fonts::system();
        let pm = share_card(&mut fonts, 2.0, &Theme::Heat.scale(), &s).unwrap();
        assert_eq!((pm.width(), pm.height()), (1200, 630));
        // opaque everywhere: no see-through corners in a chat
        assert!(pm.pixels().iter().all(|p| p.alpha() == 255));
        assert!(stutter_share(&s) < 1.0);
        assert_eq!(date("2026-11-22 13:05:41"), "22.11.2026");
    }
}

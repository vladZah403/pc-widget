//! The report of a game session (stage 4): graphs of FPS, frame time (bad
//! frames in red), CPU/GPU load and temperature on the left, the summary
//! and the PC on the right, "Сохранить CSV" below.

use pcm_core::session::{BAD_FRAME, Session};
use tiny_skia::Pixmap;

use crate::canvas::{Align, Canvas, part};
use crate::format::{dec, tr, trf};
use crate::text::{Fonts, Style};
use crate::theme::*;

/// Logical size of the report.
pub const REPORT_W: f32 = 780.0;
pub const REPORT_H: f32 = 480.0;

const PAD: f32 = 18.0;
const SIDE_X: f32 = 540.0;
const GRAPH_X1: f32 = SIDE_X - 20.0;

/// Clickable parts, physical px: [x0, y0, x1, y1].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReportBoxes {
    pub close: [f32; 4],
    pub csv: [f32; 4],
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ReportHover {
    pub close: bool,
    pub csv: bool,
}

/// "42:05" / "1:02:05"
pub fn fmt_clock(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// "2026-11-22 13:05:41" -> "22.11.2026 13:05"
fn fmt_started(s: &str) -> String {
    let (date, time) = s.split_once(' ').unwrap_or((s, ""));
    let d: Vec<&str> = date.split('-').collect();
    let date = if d.len() == 3 { format!("{}.{}.{}", d[2], d[1], d[0]) } else { date.to_string() };
    format!("{date} {}", time.get(..5).unwrap_or(time))
}

/// A frame of a graph: the plot area and the value range.
struct Plot {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    lo: f32,
    hi: f32,
}

impl Plot {
    fn x(&self, t: f32, span: f32) -> f32 {
        self.x0 + (self.x1 - self.x0) * (t / span.max(1e-3)).clamp(0.0, 1.0)
    }
    fn y(&self, v: f32) -> f32 {
        self.y1 - (self.y1 - self.y0) * ((v - self.lo) / (self.hi - self.lo).max(1e-3)).clamp(0.0, 1.0)
    }
}

/// A rounded panel with a title, a legend and the top value; returns the plot area.
fn panel(
    c: &mut Canvas,
    fonts: &mut Fonts,
    y0: f32,
    y1: f32,
    title: &str,
    legend: &[(&str, Rgb)],
    lo: f32,
    hi: f32,
    unit: &str,
) -> Plot {
    c.rrect(PAD, y0, GRAPH_X1, y1, 10.0, ink().hist_track);
    c.text(fonts, PAD + 12.0, y0 + 17.0, title, Style::SemiBold, 12.0, ink().dim);
    let mut x = GRAPH_X1 - 12.0;
    for (name, col) in legend.iter().rev() {
        let w = c.width(fonts, name, Style::Regular, 11.0);
        c.text(fonts, x - w, y0 + 17.0, name, Style::Regular, 11.0, ink().dim);
        x -= w + 6.0;
        c.rrect(x - 8.0, y0 + 9.0, x, y0 + 15.0, 2.0, *col);
        x -= 18.0;
    }
    let plot = Plot { x0: PAD + 44.0, y0: y0 + 26.0, x1: GRAPH_X1 - 12.0, y1: y1 - 10.0, lo, hi };
    // the top and the bottom of the scale
    for v in [hi, lo] {
        let y = plot.y(v);
        c.rect(plot.x0, y - 0.5 / c.s, plot.x1, y + 0.5 / c.s, ink().track);
        let label = format!("{v:.0}{unit}");
        let w = c.width(fonts, &label, Style::Regular, 10.0);
        c.text(fonts, plot.x0 - 6.0 - w, y + 3.5, &label, Style::Regular, 10.0, ink().faint);
    }
    plot
}

fn nice_max(v: f32, step: f32) -> f32 {
    ((v / step).ceil() * step).max(step)
}

/// A line through the points (t, value); None breaks it.
fn series(c: &mut Canvas, p: &Plot, span: f32, pts: &[(f32, Option<f32>)], col: Rgb) {
    let mut run: Vec<(f32, f32)> = Vec::new();
    for &(t, v) in pts {
        match v {
            Some(v) => run.push((p.x(t, span), p.y(v))),
            None => {
                if run.len() > 1 {
                    c.line(&run, 1.4, col);
                }
                run.clear();
            }
        }
    }
    if run.len() > 1 {
        c.line(&run, 1.4, col);
    }
}

pub fn session_report(
    fonts: &mut Fonts,
    scale: f32,
    sc: &Scale,
    s: &Session,
    hover: ReportHover,
) -> Option<(Pixmap, ReportBoxes)> {
    let (w, h) = (REPORT_W, REPORT_H);
    let mut c = Canvas::new((w * scale).round() as u32, (h * scale).round() as u32, scale)?;
    c.rrect(0.0, 0.0, w, h, 14.0, ink().border);
    let bw = 1.0 / scale * scale.round().max(1.0);
    c.rrect(bw, bw, w - bw, h - bw, 14.0 - bw, ink().card);
    let info = &s.info;
    let sum = &info.summary;

    // header: the game, when, how long, the window
    let title = if info.title.trim().is_empty() { info.game.clone() } else { info.title.clone() };
    let title = c.ellipsize(fonts, &title, Style::SemiBold, 18.0, SIDE_X - PAD - 10.0);
    c.text(fonts, PAD, 36.0, &title, Style::SemiBold, 18.0, ink().text);
    let mut sub = format!("{}  ·  {}", fmt_started(&info.started), fmt_clock(info.duration_s));
    if info.width > 0 {
        sub += &format!("  ·  {}×{}", info.width, info.height);
        if info.refresh_hz > 0 {
            sub += &trf(" @ {v} Гц", &[("v", info.refresh_hz.to_string())]);
        }
    }
    c.text(fonts, PAD, 56.0, &sub, Style::Regular, 12.0, ink().dim);
    let close = [w - 42.0, 14.0, w - 14.0, 42.0];
    if hover.close {
        c.rrect(close[0], close[1], close[2], close[3], 8.0, ink().close_hover);
    }
    let (cx, cy) = ((close[0] + close[2]) / 2.0, (close[1] + close[3]) / 2.0);
    let xc = if hover.close { ink().text } else { ink().dim };
    c.line(&[(cx - 5.0, cy - 5.0), (cx + 5.0, cy + 5.0)], 1.6, xc);
    c.line(&[(cx - 5.0, cy + 5.0), (cx + 5.0, cy - 5.0)], 1.6, xc);

    // graphs
    let span = info.duration_s.max(1.0) as f32;
    let (top, bottom, gap) = (72.0, h - 52.0, 8.0);
    let gh = (bottom - top - gap * 3.0) / 4.0;
    let ys: Vec<(f32, f32)> =
        (0..4).map(|i| (top + i as f32 * (gh + gap), top + i as f32 * (gh + gap) + gh)).collect();
    let good = sc.at(0.05);
    let bad = sc.at(0.95);
    let cpu_col = sc.at(0.05);
    let gpu_col = sc.at(0.5);

    // 1. FPS each second
    let fps_max = info.samples.iter().filter_map(|x| x.fps).fold(0.0f32, f32::max);
    let p = panel(&mut c, fonts, ys[0].0, ys[0].1, "FPS", &[], 0.0, nice_max(fps_max, 30.0), "");
    let avg_y = p.y(sum.avg_fps as f32);
    c.rect(p.x0, avg_y - 0.5 / scale, p.x1, avg_y + 0.5 / scale, ink().faint);
    let pts: Vec<(f32, Option<f32>)> =
        info.samples.iter().map(|x| (x.t, x.fps.filter(|f| *f > 0.0))).collect();
    series(&mut c, &p, span, &pts, good);

    // 2. frame time: each column the range of its frames; bad frames in red
    let med = sum.median_ms as f32;
    let ms_hi = nice_max((med * 4.0).max(20.0), 10.0);
    let p = panel(
        &mut c,
        fonts,
        ys[1].0,
        ys[1].1,
        &tr("Время кадра, мс"),
        &[(&tr("плохие кадры"), bad)],
        0.0,
        ms_hi,
        "",
    );
    let bad_ms = med * BAD_FRAME;
    let bad_y = p.y(bad_ms);
    // the red zone: above twice the median
    c.rect(p.x0, p.y0, p.x1, bad_y, lerp(ink().hist_track, bad, 0.12));
    let cols = ((p.x1 - p.x0) * scale).max(1.0) as usize;
    let mut lo = vec![f32::INFINITY; cols];
    let mut hi = vec![0.0f32; cols];
    let mut n_bad = vec![0u32; cols];
    let mut t = 0.0f32;
    for &ms in &s.frame_ms {
        t += ms / 1000.0;
        if ms >= 1000.0 {
            continue; // a pause
        }
        let i = (((t / span) * cols as f32) as usize).min(cols - 1);
        lo[i] = lo[i].min(ms);
        hi[i] = hi[i].max(ms);
        if ms > bad_ms {
            n_bad[i] += 1;
        }
    }
    // red only where bad frames bunch up (a long session has some in every column)
    let played = hi.iter().filter(|h| **h > 0.0).count();
    let mark_min = pcm_core::session::stutter_mark_min(n_bad.iter().sum::<u32>() as usize, played);
    for i in 0..cols {
        if hi[i] <= 0.0 {
            continue;
        }
        let x = p.x0 + i as f32 / scale;
        let (ya, yb) = (p.y(hi[i]), p.y(lo[i]));
        let col = if n_bad[i] >= mark_min { bad } else { good };
        c.rect(x, ya, x + 1.0 / scale, yb.max(ya + 1.0 / scale), col);
    }

    // 3. load
    let p = panel(
        &mut c,
        fonts,
        ys[2].0,
        ys[2].1,
        &tr("Загрузка, %"),
        &[("CPU", cpu_col), ("GPU", gpu_col)],
        0.0,
        100.0,
        "",
    );
    let pts: Vec<_> = info.samples.iter().map(|x| (x.t, x.cpu)).collect();
    series(&mut c, &p, span, &pts, cpu_col);
    let pts: Vec<_> = info.samples.iter().map(|x| (x.t, x.gpu)).collect();
    series(&mut c, &p, span, &pts, gpu_col);

    // 4. temperature
    let temps = info.samples.iter().flat_map(|x| [x.cpu_t, x.gpu_t]).flatten();
    let (t_lo, t_hi) = temps.fold((f32::INFINITY, 0.0f32), |(a, b), v| (a.min(v), b.max(v)));
    let (t_lo, t_hi) =
        if t_hi > 0.0 { ((t_lo / 10.0).floor() * 10.0, nice_max(t_hi, 10.0)) } else { (30.0, 100.0) };
    let p = panel(
        &mut c,
        fonts,
        ys[3].0,
        ys[3].1,
        &tr("Температура, °C"),
        &[("CPU", cpu_col), ("GPU", gpu_col)],
        t_lo,
        t_hi,
        "°",
    );
    if t_hi > 0.0 && info.samples.iter().any(|x| x.cpu_t.is_some() || x.gpu_t.is_some()) {
        let pts: Vec<_> = info.samples.iter().map(|x| (x.t, x.cpu_t)).collect();
        series(&mut c, &p, span, &pts, cpu_col);
        let pts: Vec<_> = info.samples.iter().map(|x| (x.t, x.gpu_t)).collect();
        series(&mut c, &p, span, &pts, gpu_col);
    } else {
        let msg = tr("датчики температуры не отвечали");
        c.rich(
            fonts,
            (p.x0 + p.x1) / 2.0,
            (p.y0 + p.y1) / 2.0 + 4.0,
            &[part(msg, Style::Regular, 11.0, ink().faint)],
            Align::Center,
        );
    }
    // the time axis under the last graph
    c.text(fonts, PAD + 44.0, bottom + 16.0, "0:00", Style::Regular, 10.0, ink().faint);
    let end = fmt_clock(info.duration_s);
    let ew = c.width(fonts, &end, Style::Regular, 10.0);
    c.text(fonts, GRAPH_X1 - 12.0 - ew, bottom + 16.0, &end, Style::Regular, 10.0, ink().faint);

    // summary
    let x = SIDE_X;
    let r = w - PAD;
    c.rich(
        fonts,
        x,
        122.0,
        &[
            part(format!("{:.0}", sum.avg_fps), Style::SemiBoldCondensed, 46.0, fps_scale(sc, sum.avg_fps)),
            part(tr("  FPS в среднем"), Style::Regular, 12.0, ink().dim),
        ],
        Align::Left,
    );
    let pct_bad = if sum.frames > 0 { sum.bad_frames as f64 * 100.0 / sum.frames as f64 } else { 0.0 };
    let rows: Vec<(String, String, Rgb)> = vec![
        ("1% low".into(), format!("{:.0} FPS", sum.low1), fps_scale(sc, sum.low1)),
        ("0.1% low".into(), format!("{:.0} FPS", sum.low01), fps_scale(sc, sum.low01)),
        (tr("Мин / макс за секунду"), format!("{:.0} / {:.0}", sum.min_fps, sum.max_fps), ink().text),
        (tr("Медиана кадра"), trf("{v} мс", &[("v", dec(format!("{:.1}", sum.median_ms)))]), ink().text),
        (
            tr("Плохие кадры"),
            format!("{} ({}%)", sum.bad_frames, dec(format!("{pct_bad:.1}"))),
            if pct_bad >= 1.0 { bad } else { ink().text },
        ),
        (tr("Кадров"), format_count(sum.frames), ink().text),
        (tr("Игра без пауз"), fmt_clock(sum.active_s), ink().text),
    ];
    let mut y = 156.0;
    for (label, value, col) in &rows {
        c.text(fonts, x, y, label, Style::Regular, 12.0, ink().dim);
        c.rich(fonts, r, y, &[part(value.clone(), Style::SemiBold, 12.0, *col)], Align::Right);
        y += 22.0;
    }
    y += 2.0;
    c.rect(x, y - 0.5 / scale, r, y + 0.5 / scale, ink().track);
    y += 20.0;
    let mut facts: Vec<String> = Vec::new();
    for v in [&info.cpu, &info.gpu] {
        if !v.is_empty() {
            facts.push(v.clone());
        }
    }
    let mut line = Vec::new();
    if info.ram_gb > 0.0 {
        line.push(trf("ОЗУ {v} ГБ", &[("v", format!("{:.0}", info.ram_gb))]));
    }
    if !info.windows.is_empty() {
        line.push(format!("Windows {}", info.windows));
    }
    if !line.is_empty() {
        facts.push(line.join("  ·  "));
    }
    if !info.source.is_empty() {
        let src = if info.source == "dxgi" {
            "DXGI (DirectX 10–12)"
        } else {
            &tr("ядро (DX9 / OpenGL / Vulkan)")
        };
        facts.push(trf("Кадры: {src}", &[("src", src.to_string())]));
    }
    for f in &facts {
        let t = c.ellipsize(fonts, f, Style::Regular, 11.0, r - x);
        c.text(fonts, x, y, &t, Style::Regular, 11.0, ink().dim);
        y += 18.0;
    }

    // "Сохранить CSV"
    let label = tr("Сохранить CSV");
    let lw = c.width(fonts, &label, Style::SemiBold, 12.0);
    let csv = [r - lw - 28.0, h - 44.0, r, h - 16.0];
    c.rrect(csv[0], csv[1], csv[2], csv[3], 8.0, if hover.csv { ink().row_hover } else { ink().hist_track });
    c.rrect_stroke(
        csv[0],
        csv[1],
        csv[2],
        csv[3],
        8.0,
        1.0,
        if hover.csv { sc.at(0.05) } else { ink().border },
    );
    c.text(
        fonts,
        csv[0] + 14.0,
        h - 25.5,
        &label,
        Style::SemiBold,
        12.0,
        if hover.csv { ink().text } else { ink().dim },
    );

    let px = |b: [f32; 4]| b.map(|v| v * scale);
    Some((c.pm, ReportBoxes { close: px(close), csv: px(csv) }))
}

fn fps_scale(sc: &Scale, fps: f64) -> Rgb {
    crate::overlay::fps_color(sc, fps)
}

/// 123456 -> "123 456"
fn format_count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(ch);
    }
    out
}

/// A made-up 3-minute session for the snapshot test and previews.
pub fn sample_session() -> Session {
    use pcm_core::session::{Sample, SessionInfo, VERSION, fill_fps, frame_times, summarize};
    let mut times = Vec::new();
    let mut t = 0.0f64;
    let mut i = 0u64;
    while t < 180.0 {
        // ~60 FPS, a heavier stretch in the middle, a stutter now and then
        let base = if (70.0..100.0).contains(&t) { 1.0 / 45.0 } else { 1.0 / 62.0 };
        let jitter = ((i * 7919) % 13) as f64 * 0.0004;
        let stutter = if i % 997 == 500 { 0.06 } else { 0.0 };
        t += base + jitter + stutter;
        times.push(t);
        i += 1;
    }
    let mut samples: Vec<Sample> = (1..=180)
        .map(|s| {
            let f = s as f32;
            Sample {
                t: f,
                fps: None,
                cpu: Some(38.0 + 12.0 * (f / 17.0).sin().abs()),
                gpu: Some(if (70.0..100.0).contains(&f) { 97.0 } else { 78.0 + 6.0 * (f / 9.0).cos() }),
                cpu_t: Some(58.0 + f / 12.0),
                gpu_t: Some(55.0 + f / 10.0),
                ram: Some(52.0),
            }
        })
        .collect();
    fill_fps(&mut samples, &times);
    let frame_ms = frame_times(&times);
    let summary = summarize(&frame_ms, &samples);
    Session {
        info: SessionInfo {
            version: VERSION,
            game: "civilizationvi".into(),
            title: "Sid Meier's Civilization VI (DX12)".into(),
            started: "2026-11-22 13:05:41".into(),
            duration_s: times[times.len() - 1] - times[0],
            source: "dxgi".into(),
            width: 1600,
            height: 900,
            refresh_hz: 60,
            cpu: "AMD Ryzen 5 5500U with Radeon Graphics".into(),
            gpu: "AMD Radeon Graphics".into(),
            ram_gb: 21.8,
            windows: "build 19045".into(),
            summary,
            samples,
        },
        frame_ms,
    }
}

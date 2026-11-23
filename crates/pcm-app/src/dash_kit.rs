//! Pieces the PC Monitor window's pages share, in the style of the home
//! page (beta 2 concept): stat tiles, a time chart with a legend and the
//! mouse's values, buttons, a switch, a section card.

use eframe::egui;
use egui::{
    Align2, Color32, CursorIcon, Mesh, Painter, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2,
};

use crate::dash_home::{alpha, bold, fit, icon_font, pal, panel, text_font};

pub const GAP: f32 = 16.0;

/// A tile: label, value, unit, the value's colour, a change ("+12") and its colour.
pub type Tile = (String, String, String, Color32, Option<(String, Color32)>);

/// A row of tiles (wrapping on narrow windows).
pub fn stat_tiles(ui: &mut Ui, w: f32, tiles: &[Tile]) {
    if tiles.is_empty() {
        return;
    }
    let fit_cols = ((w + GAP) / (150.0 + GAP)).floor().max(2.0) as usize;
    let cols = tiles.len().min(fit_cols);
    let tw = (w - GAP * (cols - 1) as f32) / cols as f32;
    let th = 78.0;
    for (n, row) in tiles.chunks(cols).enumerate() {
        if n > 0 {
            ui.add_space(GAP);
        }
        let (r, _) = ui.allocate_exact_size(vec2(w, th), Sense::hover());
        let p = ui.painter();
        for (i, (label, value, unit, col, d)) in row.iter().enumerate() {
            let t = Rect::from_min_size(pos2(r.left() + i as f32 * (tw + GAP), r.top()), vec2(tw, th));
            panel(p, t);
            let lf = text_font(12.0);
            p.text(
                t.left_top() + vec2(16.0, 13.0),
                Align2::LEFT_TOP,
                fit(p, label, &lf, tw - 24.0),
                lf,
                pal().dim,
            );
            let unit_w = if unit.is_empty() {
                0.0
            } else {
                p.layout_no_wrap(unit.clone(), text_font(12.0), pal().dim).size().x + 8.0
            };
            let vf = bold(26.0);
            let value = fit(p, value, &vf, tw - 30.0 - unit_w);
            let vr = p.text(t.left_top() + vec2(15.0, 32.0), Align2::LEFT_TOP, value, vf, *col);
            if !unit.is_empty() {
                p.text(
                    pos2(vr.right() + 6.0, vr.bottom() - 6.0),
                    Align2::LEFT_BOTTOM,
                    unit,
                    text_font(12.0),
                    pal().dim,
                );
            }
            if let Some((dt, dc)) = d {
                p.text(pos2(t.right() - 14.0, vr.bottom() - 6.0), Align2::RIGHT_BOTTOM, dt, bold(12.5), *dc);
            }
        }
    }
}

/// A button drawn right-aligned at `right`; returns its rect and whether it was clicked.
pub fn button(
    ui: &mut Ui,
    right: f32,
    y: f32,
    text: &str,
    accent: bool,
    danger: bool,
    id: &str,
) -> (Rect, bool) {
    let font = bold(13.0);
    let w = ui.painter().layout_no_wrap(text.to_string(), font.clone(), pal().text).size().x + 32.0;
    let r = Rect::from_min_size(pos2(right - w, y), vec2(w, 36.0));
    let clicked = button_at(ui, r, text, accent, danger, id);
    (r, clicked)
}

/// A button in `r`; true when clicked.
pub fn button_at(ui: &mut Ui, r: Rect, text: &str, accent: bool, danger: bool, id: &str) -> bool {
    let p = ui.painter().clone();
    let resp = ui.interact(r, ui.scope_id().with(id), Sense::click());
    let (fill, fg) = if accent {
        (if resp.hovered() { alpha(pal().accent, 220) } else { pal().accent }, Color32::WHITE)
    } else {
        (if resp.hovered() { pal().card_hi } else { pal().card }, if danger { pal().hot } else { pal().text })
    };
    let radius = (r.height() / 4.0).min(9.0);
    p.rect_filled(r, radius, fill);
    if !accent {
        p.rect_stroke(
            r,
            radius,
            Stroke::new(1.0, if danger { alpha(pal().hot, 160) } else { pal().border }),
            StrokeKind::Inside,
        );
    }
    let size = if r.height() < 30.0 { 12.0 } else { 13.0 };
    p.text(r.center(), Align2::CENTER_CENTER, text, bold(size), fg);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    resp.clicked()
}

/// An on/off switch that slides (`r` about 38 × 22).
pub fn switch(ui: &Ui, p: &Painter, r: Rect, on: bool, id: impl std::hash::Hash + std::fmt::Debug) {
    let t = ui.ctx().animate_bool_with_time(ui.scope_id().with(id), on, 0.12);
    let (a, b) = (pal().border, pal().accent);
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    let track = Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()));
    p.rect_filled(r, r.height() / 2.0, track);
    let rad = r.height() / 2.0 - 3.0;
    let x = r.left() + 3.0 + rad + (r.width() - 6.0 - 2.0 * rad) * t;
    p.circle_filled(pos2(x, r.center().y), rad, Color32::WHITE);
}

/// A series of a time chart: values at even steps (None = no data).
pub struct Series {
    pub name: String,
    pub values: Vec<Option<f32>>,
    pub color: Color32,
    pub dashed: bool,
    /// The area under the line, fading down.
    pub fill: bool,
}

/// A chart card: the title, a legend with each series' latest value, a grid,
/// the lines and the time labels; the mouse shows a box with the values.
pub struct TimeChart<'a> {
    pub title: String,
    pub series: Vec<Series>,
    pub lo: f32,
    pub hi: f32,
    /// Values as text ("42%", "1,2 МБ/с").
    pub fmt: &'a dyn Fn(f32) -> String,
    /// The labels under the plot, left to right (evenly spread).
    pub times: Vec<String>,
    /// The moment of a column, for the mouse's box ("13:05").
    pub when: &'a dyn Fn(usize) -> String,
}

pub fn time_chart(ui: &mut Ui, r: Rect, c: &TimeChart, id: &str) {
    let p = ui.painter().clone();
    panel(&p, r);
    p.text(r.left_top() + vec2(18.0, 15.0), Align2::LEFT_TOP, &c.title, bold(14.5), pal().text);
    // the legend on the right: a line sample, the name, the latest value
    let mut x = r.right() - 18.0;
    for s in c.series.iter().rev() {
        let last = s.values.iter().rev().find_map(|v| *v);
        let v = last.map_or("—".into(), |v| (c.fmt)(v));
        let vr = p.text(pos2(x, r.top() + 17.0), Align2::RIGHT_TOP, v, bold(12.5), pal().text);
        let nr = p.text(
            pos2(vr.left() - 6.0, r.top() + 17.0),
            Align2::RIGHT_TOP,
            &s.name,
            text_font(12.5),
            pal().dim,
        );
        let y = nr.center().y;
        let x1 = nr.left() - 6.0;
        if s.dashed {
            for k in 0..3 {
                let x0 = x1 - 16.0 + k as f32 * 6.0;
                p.line_segment([pos2(x0, y), pos2(x0 + 3.5, y)], Stroke::new(2.0, s.color));
            }
        } else {
            p.line_segment([pos2(x1 - 16.0, y), pos2(x1, y)], Stroke::new(2.0, s.color));
        }
        x = x1 - 30.0;
    }
    let plot =
        Rect::from_min_max(pos2(r.left() + 64.0, r.top() + 48.0), pos2(r.right() - 18.0, r.bottom() - 32.0));
    let (lo, hi) = (c.lo, c.hi.max(c.lo + 1e-3));
    for k in 0..=2 {
        let v = lo + (hi - lo) * k as f32 / 2.0;
        let y = plot.bottom() - plot.height() * k as f32 / 2.0;
        p.hline(plot.x_range(), y, Stroke::new(1.0, pal().grid));
        p.text(pos2(plot.left() - 10.0, y), Align2::RIGHT_CENTER, (c.fmt)(v), text_font(11.0), pal().faint);
    }
    let n = c.series.iter().map(|s| s.values.len()).max().unwrap_or(0).max(2);
    let xy = |i: usize, v: f32| {
        pos2(
            plot.left() + plot.width() * i as f32 / (n - 1) as f32,
            plot.bottom() - plot.height() * ((v - lo) / (hi - lo)).clamp(0.0, 1.0),
        )
    };
    for s in &c.series {
        let mut runs: Vec<Vec<Pos2>> = vec![Vec::new()];
        for (i, v) in s.values.iter().enumerate() {
            match v {
                Some(v) => runs.last_mut().unwrap().push(xy(i, *v)),
                None if !runs.last().unwrap().is_empty() => runs.push(Vec::new()),
                None => {}
            }
        }
        for pts in runs.into_iter().filter(|r| r.len() >= 2) {
            if s.fill {
                fill_under(&p, &pts, plot.bottom(), s.color);
            }
            if s.dashed {
                p.extend(Shape::dashed_line(&pts, Stroke::new(1.5, s.color), 5.0, 4.0));
            } else {
                p.add(Shape::line(pts, Stroke::new(1.7, s.color)));
            }
        }
    }
    let k = c.times.len();
    for (i, t) in c.times.iter().enumerate() {
        let f = if k > 1 { i as f32 / (k - 1) as f32 } else { 0.0 };
        let anchor = if i == 0 {
            Align2::LEFT_TOP
        } else if i + 1 == k {
            Align2::RIGHT_TOP
        } else {
            Align2::CENTER_TOP
        };
        p.text(
            pos2(plot.left() + plot.width() * f, plot.bottom() + 9.0),
            anchor,
            t,
            text_font(11.0),
            pal().faint,
        );
    }
    // the mouse: a line, the dots and a box with the values
    let resp = ui.interact(plot, ui.scope_id().with(id), Sense::hover());
    if let Some(m) = resp.hover_pos() {
        let i = (((m.x - plot.left()) / plot.width()) * (n - 1) as f32).round().clamp(0.0, (n - 1) as f32)
            as usize;
        let x = xy(i, lo).x;
        p.vline(x, plot.y_range(), Stroke::new(1.0, pal().dim));
        let mut rows = vec![((c.when)(i), None::<Color32>)];
        for s in &c.series {
            if let Some(v) = s.values.get(i).copied().flatten() {
                p.circle_filled(xy(i, v), 4.0, s.color);
                p.circle_stroke(xy(i, v), 4.0, Stroke::new(1.5, pal().card));
                rows.push((format!("{}  {}", s.name, (c.fmt)(v)), Some(s.color)));
            }
        }
        let bw = rows
            .iter()
            .map(|(t, _)| p.layout_no_wrap(t.clone(), text_font(12.5), pal().text).size().x)
            .fold(80.0, f32::max)
            + 40.0;
        let bh = 12.0 + rows.len() as f32 * 20.0;
        let bx = if x + 14.0 + bw > plot.right() { x - 14.0 - bw } else { x + 14.0 };
        let b = Rect::from_min_size(pos2(bx, plot.top() + 4.0), vec2(bw, bh));
        p.rect_filled(b, 8.0, pal().bg);
        p.rect_stroke(b, 8.0, Stroke::new(1.0, pal().border), StrokeKind::Inside);
        for (k, (t, dot)) in rows.iter().enumerate() {
            let y = b.top() + 8.0 + k as f32 * 20.0;
            let mut tx = b.left() + 12.0;
            if let Some(col) = dot {
                p.circle_filled(pos2(tx + 4.0, y + 8.0), 4.0, *col);
                tx += 14.0;
            }
            p.text(
                pos2(tx, y),
                Align2::LEFT_TOP,
                t,
                text_font(12.5),
                if dot.is_some() { pal().text } else { pal().dim },
            );
        }
    }
}

/// The area under a line, fading down.
pub fn fill_under(p: &Painter, pts: &[Pos2], bottom: f32, color: Color32) {
    if pts.len() < 2 {
        return;
    }
    let mut mesh = Mesh::default();
    for (i, pt) in pts.iter().enumerate() {
        mesh.colored_vertex(*pt, alpha(color, 70));
        mesh.colored_vertex(pos2(pt.x, bottom), alpha(color, 6));
        if i > 0 {
            let b = (i * 2) as u32;
            mesh.add_triangle(b - 2, b - 1, b);
            mesh.add_triangle(b - 1, b + 1, b);
        }
    }
    p.add(Shape::mesh(mesh));
}

/// A thin bar: the track and the filled part (0..1).
pub fn bar(p: &Painter, r: Rect, t: f32, color: Color32) {
    p.rect_filled(r, r.height() / 2.0, pal().grid);
    if t > 0.0 {
        let w = (r.width() * t.clamp(0.0, 1.0)).max(r.height());
        p.rect_filled(Rect::from_min_size(r.min, vec2(w, r.height())), r.height() / 2.0, color);
    }
}

/// A sparkline (its own range, so a calm minute still moves).
pub fn sparkline(p: &Painter, r: Rect, values: &[f32], color: Color32) {
    if values.len() < 2 {
        return;
    }
    let lo = values.iter().copied().fold(f32::MAX, f32::min);
    let hi = values.iter().copied().fold(f32::MIN, f32::max);
    let span = (hi - lo).max(hi.abs() * 0.25).max(1e-3);
    let mid = (hi + lo) / 2.0;
    let (lo, hi) = (mid - span / 2.0, mid + span / 2.0);
    let n = values.len();
    let pts: Vec<Pos2> = values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            pos2(
                r.left() + r.width() * i as f32 / (n - 1) as f32,
                r.bottom() - r.height() * ((v - lo) / (hi - lo)).clamp(0.0, 1.0),
            )
        })
        .collect();
    p.add(Shape::line(pts, Stroke::new(1.4, color)));
}

/// The rectangles of `n` cards in rows of `cols` across `w`, each `h` tall.
pub fn grid_cells(ui: &mut Ui, w: f32, n: usize, cols: usize, h: f32) -> Vec<Rect> {
    let cols = cols.max(1);
    let cw = (w - GAP * (cols - 1) as f32) / cols as f32;
    let mut out = Vec::new();
    for row in 0..n.div_ceil(cols) {
        if row > 0 {
            ui.add_space(GAP);
        }
        let (r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
        for c in 0..cols {
            if out.len() < n {
                out.push(Rect::from_min_size(pos2(r.left() + c as f32 * (cw + GAP), r.top()), vec2(cw, h)));
            }
        }
    }
    out
}

/// A page with nothing to show yet: its icon in a circle, a title, what to
/// do, and a button when there's something to switch on. True: clicked.
pub fn empty_state(
    ui: &mut Ui,
    glyph: &str,
    title: &str,
    text: &str,
    action: Option<&str>,
    id: &str,
) -> bool {
    let w = ui.available_width();
    let col = (w - 48.0).clamp(240.0, 460.0);
    let p = ui.painter().clone();
    let mut job = egui::text::LayoutJob::simple(text.to_string(), text_font(13.5), pal().dim, col);
    job.halign = egui::Align::Center;
    let body = p.layout_job(job);
    let h = 56.0 + 68.0 + 18.0 + 24.0 + body.size().y + if action.is_some() { 56.0 } else { 0.0 } + 40.0;
    let (r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let cx = r.center().x;
    let mut y = r.top() + 56.0;
    p.circle_filled(pos2(cx, y + 34.0), 34.0, pal().card_hi);
    p.text(pos2(cx, y + 34.0), Align2::CENTER_CENTER, glyph, icon_font(26.0), pal().accent);
    y += 68.0 + 18.0;
    p.text(pos2(cx, y), Align2::CENTER_TOP, fit(&p, title, &bold(17.0), col), bold(17.0), pal().text);
    y += 28.0;
    // centred lines: the galley is laid out around x = 0
    p.galley(pos2(cx, y), body.clone(), pal().dim);
    y += body.size().y + 20.0;
    match action {
        Some(label) => {
            let lw = p.layout_no_wrap(label.to_string(), bold(13.0), pal().text).size().x;
            let b = Rect::from_center_size(pos2(cx, y + 18.0), vec2(lw + 40.0, 36.0));
            button_at(ui, b, label, true, false, id)
        }
        None => false,
    }
}

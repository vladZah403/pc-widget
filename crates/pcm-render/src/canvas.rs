//! A pixmap with logical coordinates: every call takes logical px (as 1.8's
//! Renderer does) and draws at `scale`, moved right by the column offset `ox`.

use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Rect, Stroke, Transform,
};

use crate::text::{Font, Fonts, Style};
use crate::theme::Rgb;

pub struct Canvas {
    pub pm: Pixmap,
    /// Physical px per logical px.
    pub s: f32,
    /// x offset of the column being drawn, logical px.
    pub ox: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// A piece of text with its font (logical size) and colour.
pub type Part = (String, Style, f32, Rgb);

pub fn part(text: impl Into<String>, style: Style, size: f32, color: Rgb) -> Part {
    (text.into(), style, size, color)
}

fn paint(c: Rgb) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(Color::from_rgba8(c.0, c.1, c.2, 255));
    p.anti_alias = true;
    p
}

/// Quarter circles from cubics (k = 0.5523 r).
pub fn rounded_rect(x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> Option<Path> {
    let (w, h) = (x1 - x0, y1 - y0);
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    if r < 0.01 {
        return Some(PathBuilder::from_rect(Rect::from_ltrb(x0, y0, x1, y1)?));
    }
    let k = 0.5523 * r;
    let mut pb = PathBuilder::new();
    pb.move_to(x0 + r, y0);
    pb.line_to(x1 - r, y0);
    pb.cubic_to(x1 - r + k, y0, x1, y0 + r - k, x1, y0 + r);
    pb.line_to(x1, y1 - r);
    pb.cubic_to(x1, y1 - r + k, x1 - r + k, y1, x1 - r, y1);
    pb.line_to(x0 + r, y1);
    pb.cubic_to(x0 + r - k, y1, x0, y1 - r + k, x0, y1 - r);
    pb.line_to(x0, y0 + r);
    pb.cubic_to(x0, y0 + r - k, x0 + r - k, y0, x0 + r, y0);
    pb.close();
    pb.finish()
}

impl Canvas {
    pub fn new(width: u32, height: u32, s: f32) -> Option<Canvas> {
        Some(Canvas { pm: Pixmap::new(width.max(1), height.max(1))?, s, ox: 0.0 })
    }

    /// Logical x (in the current column) -> physical.
    pub fn x(&self, v: f32) -> f32 {
        (v + self.ox) * self.s
    }

    pub fn y(&self, v: f32) -> f32 {
        v * self.s
    }

    /// Physical px of a logical length.
    pub fn px(&self, v: f32) -> f32 {
        v * self.s
    }

    pub fn font(&self, style: Style, size: f32) -> Font {
        Font::new(style, (size * self.s).round())
    }

    pub fn rrect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, r: f32, c: Rgb) {
        let (a, b, cc, d) = (self.x(x0), self.y(y0), self.x(x1), self.y(y1));
        if let Some(p) = rounded_rect(a, b, cc, d, r * self.s) {
            self.pm.fill_path(&p, &paint(c), FillRule::Winding, Transform::identity(), None);
        }
    }

    /// A rounded rectangle's outline of `width` logical px.
    pub fn rrect_stroke(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, r: f32, width: f32, c: Rgb) {
        let (a, b, cc, d) = (self.x(x0), self.y(y0), self.x(x1), self.y(y1));
        if let Some(p) = rounded_rect(a, b, cc, d, r * self.s) {
            let st = Stroke { width: width * self.s, ..Stroke::default() };
            self.pm.stroke_path(&p, &paint(c), &st, Transform::identity(), None);
        }
    }

    pub fn rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, c: Rgb) {
        // as a path: fill_rect takes tiny-skia's hairline path for thin
        // rectangles, which trips a debug assertion at some fractions
        if let Some(r) = Rect::from_ltrb(self.x(x0), self.y(y0), self.x(x1), self.y(y1)) {
            let p = PathBuilder::from_rect(r);
            self.pm.fill_path(&p, &paint(c), FillRule::Winding, Transform::identity(), None);
        }
    }

    /// A polyline of `width` logical px.
    pub fn line(&mut self, pts: &[(f32, f32)], width: f32, c: Rgb) {
        let mut pb = PathBuilder::new();
        for (i, &(x, y)) in pts.iter().enumerate() {
            let (px, py) = (self.x(x), self.y(y));
            if i == 0 {
                pb.move_to(px, py);
            } else {
                pb.line_to(px, py);
            }
        }
        if let Some(p) = pb.finish() {
            let st = Stroke {
                width: width * self.s,
                line_cap: LineCap::Butt,
                line_join: LineJoin::Miter,
                ..Stroke::default()
            };
            self.pm.stroke_path(&p, &paint(c), &st, Transform::identity(), None);
        }
    }

    pub fn polygon(&mut self, pts: &[(f32, f32)], c: Rgb) {
        let mut pb = PathBuilder::new();
        for (i, &(x, y)) in pts.iter().enumerate() {
            let (px, py) = (self.x(x), self.y(y));
            if i == 0 {
                pb.move_to(px, py);
            } else {
                pb.line_to(px, py);
            }
        }
        pb.close();
        if let Some(p) = pb.finish() {
            self.pm.fill_path(&p, &paint(c), FillRule::Winding, Transform::identity(), None);
        }
    }

    fn oval(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> Option<Path> {
        PathBuilder::from_oval(Rect::from_ltrb(self.x(x0), self.y(y0), self.x(x1), self.y(y1))?)
    }

    pub fn ellipse(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, c: Rgb) {
        if let Some(p) = self.oval(x0, y0, x1, y1) {
            self.pm.fill_path(&p, &paint(c), FillRule::Winding, Transform::identity(), None);
        }
    }

    pub fn ellipse_stroke(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, c: Rgb) {
        if let Some(p) = self.oval(x0, y0, x1, y1) {
            let st = Stroke { width: width * self.s, ..Stroke::default() };
            self.pm.stroke_path(&p, &paint(c), &st, Transform::identity(), None);
        }
    }

    /// Logical width of the text.
    pub fn width(&self, fonts: &mut Fonts, text: &str, style: Style, size: f32) -> f32 {
        fonts.width(self.font(style, size), text) / self.s
    }

    pub fn parts_width(&self, fonts: &mut Fonts, parts: &[Part]) -> f32 {
        parts.iter().map(|(t, st, sz, _)| self.width(fonts, t, *st, *sz)).sum()
    }

    /// Text with its baseline at logical y, starting at logical x.
    pub fn text(&mut self, fonts: &mut Fonts, x: f32, y: f32, text: &str, style: Style, size: f32, c: Rgb) {
        let font = self.font(style, size);
        let (px, py) = (self.x(x).round(), self.y(y).round());
        fonts.draw(&mut self.pm, font, text, px, py, c);
    }

    /// Several pieces on one baseline, aligned at logical x.
    pub fn rich(&mut self, fonts: &mut Fonts, x: f32, y: f32, parts: &[Part], align: Align) {
        let total: f32 = parts.iter().map(|(t, st, sz, _)| fonts.width(self.font(*st, *sz), t)).sum();
        let mut px = self.x(x).round()
            - match align {
                Align::Left => 0.0,
                Align::Center => total / 2.0,
                Align::Right => total,
            };
        let py = self.y(y).round();
        for (t, st, sz, c) in parts {
            let font = self.font(*st, *sz);
            px = fonts.draw(&mut self.pm, font, t, px, py, *c);
        }
    }

    /// The text shortened with "…" to `max` logical px.
    pub fn ellipsize(&self, fonts: &mut Fonts, text: &str, style: Style, size: f32, max: f32) -> String {
        fonts.ellipsize(self.font(style, size), text, max * self.s)
    }
}

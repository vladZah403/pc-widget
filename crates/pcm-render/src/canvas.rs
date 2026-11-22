//! A pixmap with logical coordinates: every call takes logical px (as 1.8's
//! Renderer does) and draws at `scale`, moved right by the column offset `ox`.

use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Rect, Stroke, Transform,
};

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::text::{Font, Fonts, Style};
use crate::theme::Rgb;

/// A filled (rounded) rectangle drawn once and blended in again on every
/// frame that has the same one: the widget draws a few hundred of them a
/// second (bar segments, graph columns, the card), and building and filling
/// each path again was most of its idle CPU. Sizes and radii in 1/8 px,
/// the position's fraction in 1/4 px (its edges are anti-aliased).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct SpriteKey {
    w: i32,
    h: i32,
    r: i32,
    fx: u8,
    fy: u8,
    color: Rgb,
}

/// Premultiplied RGBA, `w` px wide.
struct Sprite {
    data: Vec<u8>,
    w: usize,
    h: usize,
}

/// Sprites kept: dropped all at once past this many bytes (a resize or a
/// scale change leaves the old sizes unused).
const SPRITE_BYTES: usize = 8 << 20;

thread_local! {
    static SPRITES: RefCell<(HashMap<SpriteKey, Rc<Sprite>>, usize)> = RefCell::new((HashMap::new(), 0));
}

fn sprite(key: SpriteKey) -> Option<Rc<Sprite>> {
    if let Some(s) = SPRITES.with(|c| c.borrow().0.get(&key).cloned()) {
        return Some(s);
    }
    let (fx, fy) = (key.fx as f32 / 4.0, key.fy as f32 / 4.0);
    let (w, h, r) = (key.w as f32 / 8.0, key.h as f32 / 8.0, key.r as f32 / 8.0);
    let (pw, ph) = ((fx + w).ceil() as u32, (fy + h).ceil() as u32);
    let mut pm = Pixmap::new(pw.max(1), ph.max(1))?;
    let path = rounded_rect(fx, fy, fx + w, fy + h, r)?;
    pm.fill_path(&path, &paint(key.color), FillRule::Winding, Transform::identity(), None);
    let s = Rc::new(Sprite { w: pm.width() as usize, h: pm.height() as usize, data: pm.take() });
    SPRITES.with(|c| {
        let mut c = c.borrow_mut();
        if c.1 + s.data.len() > SPRITE_BYTES {
            *c = (HashMap::new(), 0);
        }
        c.1 += s.data.len();
        c.0.insert(key, s.clone());
    });
    Some(s)
}

/// Source-over of premultiplied `s` onto the pixmap at (x, y), clipped.
fn blit(pm: &mut Pixmap, s: &Sprite, x: i32, y: i32) {
    let (pw, ph) = (pm.width() as i32, pm.height() as i32);
    let (x0, x1) = (x.max(0), (x + s.w as i32).min(pw));
    let (y0, y1) = (y.max(0), (y + s.h as i32).min(ph));
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let data = pm.data_mut();
    for py in y0..y1 {
        let src = &s.data[((py - y) as usize * s.w + (x0 - x) as usize) * 4..][..(x1 - x0) as usize * 4];
        let dst = &mut data[(py * pw + x0) as usize * 4..][..(x1 - x0) as usize * 4];
        for (d, sp) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src.as_chunks::<4>().0) {
            match sp[3] {
                0 => {}
                255 => *d = *sp,
                a => {
                    let keep = 255 - a as u32;
                    for i in 0..4 {
                        d[i] = (sp[i] as u32 + (d[i] as u32 * keep + 127) / 255).min(255) as u8;
                    }
                }
            }
        }
    }
}

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
        self.fill(a, b, cc, d, r * self.s, c);
    }

    /// A filled rectangle in physical px, from the sprite cache.
    fn fill(&mut self, a: f32, b: f32, cc: f32, d: f32, r: f32, c: Rgb) {
        if cc - a <= 0.0 || d - b <= 0.0 {
            return;
        }
        let (ax, ay) = (a.floor(), b.floor());
        let key = SpriteKey {
            w: ((cc - a) * 8.0).round() as i32,
            h: ((d - b) * 8.0).round() as i32,
            r: (r.max(0.0) * 8.0).round() as i32,
            fx: (((a - ax) * 4.0).round() as u8).min(3),
            fy: (((b - ay) * 4.0).round() as u8).min(3),
            color: c,
        };
        if let Some(s) = sprite(key) {
            blit(&mut self.pm, &s, ax as i32, ay as i32);
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
        // a sprite like the rounded ones (filled as a path: fill_rect takes
        // tiny-skia's hairline path for thin rectangles, which trips a debug
        // assertion at some fractions)
        let (a, b, cc, d) = (self.x(x0), self.y(y0), self.x(x1), self.y(y1));
        self.fill(a, b, cc, d, 0.0, c);
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

    /// An arc around (cx, cy) of radius r from `from_deg` sweeping `sweep_deg`
    /// (clockwise on screen, 0° = right), `width` logical px, round or flat ends.
    pub fn arc(
        &mut self,
        cx: f32,
        cy: f32,
        r: f32,
        from_deg: f32,
        sweep_deg: f32,
        width: f32,
        round: bool,
        c: Rgb,
    ) {
        let steps = ((sweep_deg.abs() / 2.0).ceil() as usize).max(1);
        let mut pb = PathBuilder::new();
        for i in 0..=steps {
            let a = (from_deg + sweep_deg * i as f32 / steps as f32).to_radians();
            let (x, y) = (self.x(cx + r * a.cos()), self.y(cy + r * a.sin()));
            if i == 0 {
                pb.move_to(x, y);
            } else {
                pb.line_to(x, y);
            }
        }
        if let Some(p) = pb.finish() {
            let cap = if round { LineCap::Round } else { LineCap::Butt };
            let st = Stroke { width: width * self.s, line_cap: cap, ..Stroke::default() };
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

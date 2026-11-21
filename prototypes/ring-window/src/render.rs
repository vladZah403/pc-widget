//! The card, drawn into a premultiplied RGBA pixmap with no window involved
//! (so it can be tested and saved as PNG). This is the seed of pcm-render.

use std::collections::HashMap;

use tiny_skia::{Color, FillRule, LineCap, Paint, PathBuilder, Pixmap, PixmapPaint, Stroke, Transform};

/// Logical size of the card at 100%.
pub const SIZE: f32 = 200.0;

// 1.8 palette
const CARD: (u8, u8, u8) = (28, 33, 43);
const BORDER: (u8, u8, u8) = (44, 51, 64);
const TRACK: (u8, u8, u8) = (40, 46, 58);
const TEXT: (u8, u8, u8) = (232, 234, 239);
const DIM: (u8, u8, u8) = (139, 147, 163);
/// The "heat" scale of 1.8: low load -> high load.
const HEAT: [(u8, u8, u8); 5] =
    [(0x2e, 0xc4, 0xb6), (0x9c, 0xcc, 0x65), (0xf2, 0xb3, 0x3d), (0xf2, 0x7a, 0x3d), (0xe8, 0x44, 0x5a)];

/// The gauge spans 270°, with the gap at the bottom.
const START_DEG: f32 = 135.0;
const SWEEP_DEG: f32 = 270.0;

pub fn heat(t: f32) -> (u8, u8, u8) {
    let t = t.clamp(0.0, 1.0) * (HEAT.len() - 1) as f32;
    let i = (t.floor() as usize).min(HEAT.len() - 2);
    let f = t - i as f32;
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * f).round() as u8;
    let (a, b) = (HEAT[i], HEAT[i + 1]);
    (mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

fn paint(rgb: (u8, u8, u8), alpha: f32) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(Color::from_rgba8(rgb.0, rgb.1, rgb.2, (alpha * 255.0).round() as u8));
    p.anti_alias = true;
    p
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    // quarter circles from cubics (k = 0.5523 for a circle)
    let k = 0.5523 * r;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

fn arc(cx: f32, cy: f32, r: f32, from_deg: f32, sweep_deg: f32) -> Option<tiny_skia::Path> {
    let steps = ((sweep_deg.abs() / 2.0).ceil() as usize).max(1);
    let mut pb = PathBuilder::new();
    for i in 0..=steps {
        let a = (from_deg + sweep_deg * i as f32 / steps as f32).to_radians();
        let (x, y) = (cx + r * a.cos(), cy + r * a.sin());
        if i == 0 {
            pb.move_to(x, y);
        } else {
            pb.line_to(x, y);
        }
    }
    pb.finish()
}

/// Bahnschrift (as in 1.8) from the Windows fonts folder, else Segoe UI.
pub fn system_font() -> Result<fontdue::Font, String> {
    let dir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    for name in ["bahnschrift.ttf", "segoeui.ttf"] {
        if let Ok(data) = std::fs::read(format!(r"{dir}\Fonts\{name}")) {
            return Ok(fontdue::Font::from_bytes(data, fontdue::FontSettings::default())?);
        }
    }
    Err("no Bahnschrift or Segoe UI in the fonts folder".into())
}

/// (character, size in px as bits, colour)
type GlyphKey = (char, u32, (u8, u8, u8));

/// A rasterized, coloured glyph, kept between frames.
struct Glyph {
    pm: Option<Pixmap>,
    xmin: i32,
    ymin: i32,
    height: i32,
    advance: f32,
}

/// The CPU card. What never changes (background, track, label) is drawn once
/// per scale; glyphs are rasterized once; a frame copies the base and adds
/// the filled arc and the number.
pub struct Card {
    font: fontdue::Font,
    label: String,
    scale: f32,
    base: Option<Pixmap>,
    glyphs: HashMap<GlyphKey, Glyph>,
}

impl Card {
    pub fn new(font: fontdue::Font, label: &str, scale: f32) -> Card {
        Card { font, label: label.into(), scale, base: None, glyphs: HashMap::new() }
    }

    pub fn set_scale(&mut self, scale: f32) {
        if scale != self.scale {
            self.scale = scale;
            self.base = None;
            self.glyphs.clear();
        }
    }

    fn side(&self) -> f32 {
        (SIZE * self.scale).round().max(1.0)
    }

    fn center(&self) -> (f32, f32, f32) {
        let (side, s) = (self.side(), self.scale);
        (side / 2.0, side / 2.0 + 4.0 * s, 64.0 * s)
    }

    fn glyph(&mut self, c: char, px: f32, rgb: (u8, u8, u8)) -> &Glyph {
        let font = &self.font;
        self.glyphs.entry((c, px.to_bits(), rgb)).or_insert_with(|| {
            let (m, cov) = font.rasterize(c, px);
            let pm = (m.width > 0 && m.height > 0).then(|| {
                let mut pm = Pixmap::new(m.width as u32, m.height as u32).expect("glyph size");
                for (out, &a) in pm.data_mut().as_chunks_mut::<4>().0.iter_mut().zip(&cov) {
                    let pre = |v: u8| ((v as u16 * a as u16 + 127) / 255) as u8;
                    *out = [pre(rgb.0), pre(rgb.1), pre(rgb.2), a];
                }
                pm
            });
            Glyph { pm, xmin: m.xmin, ymin: m.ymin, height: m.height as i32, advance: m.advance_width }
        })
    }

    /// Draws `text` with its baseline at `y`, horizontally centred on `cx`.
    fn text(&mut self, pm: &mut Pixmap, text: &str, px: f32, cx: f32, y: f32, rgb: (u8, u8, u8)) {
        let width: f32 = text.chars().map(|c| self.glyph(c, px, rgb).advance).sum();
        let mut pen = cx - width / 2.0;
        for c in text.chars() {
            let g = self.glyph(c, px, rgb);
            if let Some(gp) = &g.pm {
                let gx = (pen + g.xmin as f32).round() as i32;
                let gy = (y - (g.height + g.ymin) as f32).round() as i32;
                pm.draw_pixmap(gx, gy, gp.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
            }
            pen += g.advance;
        }
    }

    fn draw_base(&mut self) -> Pixmap {
        let (side, s) = (self.side(), self.scale);
        let mut pm = Pixmap::new(side as u32, side as u32).expect("card size");
        // background with a 1 px border
        if let Some(p) = rounded_rect(0.5 * s, 0.5 * s, side - s, side - s, 18.0 * s) {
            pm.fill_path(&p, &paint(CARD, 0.92), FillRule::Winding, Transform::identity(), None);
            let stroke = Stroke { width: s, ..Stroke::default() };
            pm.stroke_path(&p, &paint(BORDER, 1.0), &stroke, Transform::identity(), None);
        }
        let (cx, cy, r) = self.center();
        let stroke = Stroke { width: 14.0 * s, line_cap: LineCap::Round, ..Stroke::default() };
        if let Some(p) = arc(cx, cy, r, START_DEG, SWEEP_DEG) {
            pm.stroke_path(&p, &paint(TRACK, 1.0), &stroke, Transform::identity(), None);
        }
        let label = self.label.clone();
        self.text(&mut pm, &label, 15.0 * s, cx, side - 14.0 * s, DIM);
        pm
    }

    /// One frame: the ring filled to `load` (0..1) and the number in the middle.
    pub fn draw(&mut self, load: f32) -> Pixmap {
        if self.base.is_none() {
            self.base = Some(self.draw_base());
        }
        let mut pm = self.base.clone().expect("base");
        let s = self.scale;
        let (cx, cy, r) = self.center();
        let load = load.clamp(0.0, 1.0);
        if load > 0.002 {
            // the filled part in short segments, each in its own colour of the scale
            let round = Stroke { width: 14.0 * s, line_cap: LineCap::Round, ..Stroke::default() };
            let butt = Stroke { width: 14.0 * s, line_cap: LineCap::Butt, ..Stroke::default() };
            let segs = ((SWEEP_DEG * load / 6.0).ceil() as usize).max(1);
            let seg_sweep = SWEEP_DEG * load / segs as f32;
            for i in 0..segs {
                let from = START_DEG + seg_sweep * i as f32;
                let color = heat((i as f32 + 0.5) / segs as f32 * load);
                // overlap a little so no hairline gaps show between segments
                let last = i + 1 == segs;
                let sweep = if last { seg_sweep } else { seg_sweep + 0.6 };
                let st = if i == 0 || last { &round } else { &butt };
                if let Some(p) = arc(cx, cy, r, from, sweep) {
                    pm.stroke_path(&p, &paint(color, 1.0), st, Transform::identity(), None);
                }
            }
        }
        let number = format!("{}", (load * 100.0).round() as i32);
        self.text(&mut pm, &number, 46.0 * s, cx, cy + 14.0 * s, TEXT);
        self.text(&mut pm, "%", 16.0 * s, cx, cy + 34.0 * s, DIM);
        pm
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heat_scale_ends() {
        assert_eq!(heat(0.0), HEAT[0]);
        assert_eq!(heat(1.0), HEAT[4]);
        assert_eq!(heat(2.0), HEAT[4]);
        assert_eq!(heat(-1.0), HEAT[0]);
    }

    #[test]
    fn card_has_transparent_corners_and_scales() {
        let Ok(font) = system_font() else { return };
        let mut card = Card::new(font, "CPU", 1.0);
        let pm = card.draw(0.37);
        assert_eq!(pm.width(), 200);
        assert_eq!(pm.pixel(0, 0).unwrap().alpha(), 0, "rounded corner");
        assert!(pm.pixel(100, 30).unwrap().alpha() > 200, "inside is opaque enough");
        assert_eq!(card.draw(0.37), pm, "the same frame from the caches");
        card.set_scale(1.5);
        assert_eq!(card.draw(0.37).width(), 300);
    }
}

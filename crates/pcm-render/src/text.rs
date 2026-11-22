//! Text: Bahnschrift (a variable font: weight and width axes) from the
//! Windows fonts folder, Segoe UI when it isn't there, Segoe UI Symbol for
//! arrows and marks; glyphs rasterized by swash once and cached by
//! character, and blended into the picture directly (the widget draws ~400
//! glyphs a second: looking each up in the font again and drawing it through
//! tiny-skia's pixmap pipeline was most of the idle CPU).

use std::collections::HashMap;

use swash::scale::{Render, ScaleContext, Source};
use swash::zeno::Format;
use swash::{FontRef, Setting};
use tiny_skia::Pixmap;

use crate::theme::Rgb;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Style {
    Regular,
    SemiBold,
    SemiBoldCondensed,
}

/// A style at a size in physical pixels (ppem).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Font {
    pub style: Style,
    pub px: f32,
}

impl Font {
    pub fn new(style: Style, px: f32) -> Font {
        Font { style, px }
    }
}

fn variations(style: Style) -> [(&'static str, f32); 2] {
    match style {
        Style::Regular => [("wght", 400.0), ("wdth", 100.0)],
        Style::SemiBold => [("wght", 600.0), ("wdth", 100.0)],
        Style::SemiBoldCondensed => [("wght", 600.0), ("wdth", 75.0)],
    }
}

struct Face {
    data: Vec<u8>,
    variable: bool,
}

impl Face {
    fn font(&self) -> Option<FontRef<'_>> {
        FontRef::from_index(&self.data, 0)
    }
}

/// A character in a style at a size: what a glyph is cached by.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    ch: char,
    px: u32,
    style: Style,
}

/// A rasterized glyph: its coverage (0-255 per pixel, `w` wide), placement
/// and advance, in physical pixels.
struct Glyph {
    mask: Vec<u8>,
    w: usize,
    left: i32,
    top: i32,
    advance: f32,
}

pub struct Fonts {
    faces: Vec<Face>,
    bahn: Option<usize>,
    regular: Option<usize>,
    semibold: Option<usize>,
    symbol: Option<usize>,
    ctx: ScaleContext,
    cache: HashMap<Key, Glyph>,
}

impl Fonts {
    /// Fonts from the Windows fonts folder. Never fails: text is simply not
    /// drawn when there is no usable font at all.
    pub fn system() -> Fonts {
        let dir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        Fonts::from_dir(&format!(r"{dir}\Fonts"))
    }

    pub fn from_dir(dir: &str) -> Fonts {
        let mut f = Fonts {
            faces: Vec::new(),
            bahn: None,
            regular: None,
            semibold: None,
            symbol: None,
            ctx: ScaleContext::new(),
            cache: HashMap::new(),
        };
        let mut load = |name: &str| -> Option<usize> {
            let data = std::fs::read(format!(r"{dir}\{name}")).ok()?;
            let variable = FontRef::from_index(&data, 0)?.variations().len() > 0;
            f.faces.push(Face { data, variable });
            Some(f.faces.len() - 1)
        };
        let bahn = load("bahnschrift.ttf");
        let regular = load("segoeui.ttf");
        let semibold = load("seguisb.ttf");
        let symbol = load("seguisym.ttf");
        f.bahn = bahn;
        f.regular = regular;
        f.semibold = semibold;
        f.symbol = symbol;
        f
    }

    fn primary(&self, style: Style) -> Option<usize> {
        self.bahn.or(match style {
            Style::Regular => self.regular,
            _ => self.semibold.or(self.regular),
        })
    }

    /// The face that has `ch` (the symbol font for arrows Bahnschrift lacks).
    fn resolve(&self, style: Style, ch: char) -> Option<(usize, u16)> {
        let mut tried = Vec::with_capacity(3);
        for face in [self.primary(style), self.symbol, self.regular].into_iter().flatten() {
            if tried.contains(&face) {
                continue;
            }
            tried.push(face);
            let gid = self.faces[face].font()?.charmap().map(ch);
            if gid != 0 {
                return Some((face, gid));
            }
        }
        self.primary(style).map(|f| (f, 0))
    }

    fn glyph(&mut self, font: Font, ch: char) -> Option<&Glyph> {
        let key = Key { ch, px: font.px.to_bits(), style: font.style };
        if !self.cache.contains_key(&key) {
            let (face, gid) = self.resolve(font.style, ch)?;
            let f = &self.faces[face];
            let fr = f.font()?;
            let vars = if f.variable { variations(font.style).to_vec() } else { Vec::new() };
            let settings: Vec<Setting<f32>> = vars.iter().map(|&(t, v)| (t, v).into()).collect();
            let coords: Vec<_> = fr.variations().normalized_coords(settings.iter().copied()).collect();
            let advance = fr.glyph_metrics(&coords).scale(font.px).advance_width(gid);
            let mut scaler =
                self.ctx.builder(fr).size(font.px).hint(true).variations(settings.iter().copied()).build();
            let img = Render::new(&[Source::Outline]).format(Format::Alpha).render(&mut scaler, gid);
            let (mut left, mut top, mut w, mut mask) = (0, 0, 0, Vec::new());
            if let Some(img) = img
                && img.placement.width > 0
                && img.placement.height > 0
            {
                left = img.placement.left;
                top = img.placement.top;
                w = img.placement.width as usize;
                mask = img.data;
            }
            self.cache.insert(key, Glyph { mask, w, left, top, advance });
        }
        self.cache.get(&key)
    }

    /// Width of `text` in physical pixels.
    pub fn width(&mut self, font: Font, text: &str) -> f32 {
        text.chars().map(|c| self.glyph(font, c).map_or(0.0, |g| g.advance)).sum()
    }

    /// Draws `text` from x with its baseline at y (physical pixels); returns
    /// the x after it.
    pub fn draw(&mut self, pm: &mut Pixmap, font: Font, text: &str, x: f32, y: f32, color: Rgb) -> f32 {
        let mut pen = x;
        for c in text.chars() {
            let Some(g) = self.glyph(font, c) else { continue };
            if g.w > 0 {
                let gx = (pen + g.left as f32).round() as i32;
                let gy = (y - g.top as f32).round() as i32;
                blend(pm, g, gx, gy, color);
            }
            pen += g.advance;
        }
        pen
    }

    /// The text shortened with "…" to fit `max` physical pixels.
    pub fn ellipsize(&mut self, font: Font, text: &str, max: f32) -> String {
        if self.width(font, text) <= max {
            return text.to_string();
        }
        let mut chars: Vec<char> = text.chars().collect();
        while !chars.is_empty() {
            chars.pop();
            let t: String = chars.iter().collect::<String>() + "…";
            if self.width(font, &t) <= max {
                return t;
            }
        }
        "…".into()
    }

    /// Drop cached glyphs (e.g. after a scale change makes them useless).
    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }
}

/// The glyph's coverage in `color`, source-over onto the premultiplied
/// RGBA pixmap at (x, y), clipped to it.
fn blend(pm: &mut Pixmap, g: &Glyph, x: i32, y: i32, color: Rgb) {
    let (pw, ph) = (pm.width() as i32, pm.height() as i32);
    let gh = (g.mask.len() / g.w) as i32;
    let (x0, x1) = (x.max(0), (x + g.w as i32).min(pw));
    let (y0, y1) = (y.max(0), (y + gh).min(ph));
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let rgb = [color.0 as u32, color.1 as u32, color.2 as u32];
    let data = pm.data_mut();
    for py in y0..y1 {
        let src = &g.mask[(py - y) as usize * g.w..][..g.w];
        let row = &mut data[(py * pw) as usize * 4..][..pw as usize * 4];
        for px in x0..x1 {
            let a = src[(px - x) as usize] as u32;
            if a == 0 {
                continue;
            }
            let d = &mut row[px as usize * 4..][..4];
            let keep = 255 - a;
            // (v * a + 127) / 255, the same rounding as the cached pixmaps had
            for i in 0..3 {
                d[i] = ((rgb[i] * a + 127) / 255 + (d[i] as u32 * keep + 127) / 255).min(255) as u8;
            }
            d[3] = (a + (d[3] as u32 * keep + 127) / 255).min(255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blends_over_what_is_there() {
        let mut pm = Pixmap::new(4, 1).unwrap();
        pm.data_mut().copy_from_slice(&[0, 0, 0, 0, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 0, 0]);
        let g = Glyph { mask: vec![255, 128, 0], w: 3, left: 0, top: 0, advance: 3.0 };
        blend(&mut pm, &g, 0, 0, (255, 0, 0));
        let d = pm.data();
        assert_eq!(&d[0..4], &[255, 0, 0, 255], "full coverage: the colour");
        assert_eq!(&d[4..8], &[128, 0, 127, 255], "half: half red over blue");
        assert_eq!(&d[8..12], &[0, 0, 255, 255], "none: untouched");
        blend(&mut pm, &g, 3, 0, (255, 0, 0)); // clipped at the right edge
        assert_eq!(&pm.data()[12..16], &[255, 0, 0, 255]);
    }

    #[test]
    fn measures_and_shortens() {
        let mut f = Fonts::system();
        if f.primary(Style::Regular).is_none() {
            return; // no Windows fonts here
        }
        let font = Font::new(Style::Regular, 12.0);
        let w = f.width(font, "Система");
        assert!(w > 30.0 && w < 70.0, "{w}");
        let cond = f.width(Font::new(Style::SemiBoldCondensed, 12.0), "Система");
        if f.bahn.is_some() {
            assert!(cond < w, "condensed is narrower: {cond} vs {w}");
        }
        let short = f.ellipsize(font, "AMD Radeon Graphics Super Long Name", 60.0);
        assert!(short.ends_with('…') && f.width(font, &short) <= 60.0);
        assert!(f.width(font, "↓") > 0.0, "arrows come from the symbol font");
    }
}

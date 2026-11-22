//! Colours: the fixed palette of 1.8 and the "cold -> hot" scales of the themes.

use std::sync::atomic::{AtomicBool, Ordering};

pub type Rgb = (u8, u8, u8);

pub const CARD: Rgb = (28, 33, 43);
pub const BORDER: Rgb = (44, 51, 64);
pub const TRACK: Rgb = (40, 46, 58);
pub const HIST_TRACK: Rgb = (34, 40, 51);
pub const TEXT: Rgb = (232, 234, 239);
pub const DIM: Rgb = (139, 147, 163);
pub const FAINT: Rgb = (88, 96, 112);
pub const CLOSE_HOVER: Rgb = (232, 68, 90);
/// A row under the mouse that a click opens (between CARD and TRACK).
pub const ROW_HOVER: Rgb = (35, 41, 53);
pub const PROCS_BG: Rgb = (34, 40, 52);

/// The PC Monitor window's light look ("Светлая тема"): the pictures it
/// shows (hardware cards, history charts, session reports) are drawn in
/// `ink()` colours and the theme scales darkened to read on white. Set only
/// in the window's process; the widget and the overlay stay dark.
static LIGHT: AtomicBool = AtomicBool::new(false);

pub fn set_light(on: bool) {
    LIGHT.store(on, Ordering::Relaxed);
}

pub fn light() -> bool {
    LIGHT.load(Ordering::Relaxed)
}

/// The fixed colours of the window's pictures.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ink {
    pub card: Rgb,
    pub border: Rgb,
    pub track: Rgb,
    pub hist_track: Rgb,
    pub text: Rgb,
    pub dim: Rgb,
    pub faint: Rgb,
    pub close_hover: Rgb,
    pub row_hover: Rgb,
}

pub const DARK_INK: Ink = Ink {
    card: CARD,
    border: BORDER,
    track: TRACK,
    hist_track: HIST_TRACK,
    text: TEXT,
    dim: DIM,
    faint: FAINT,
    close_hover: CLOSE_HOVER,
    row_hover: ROW_HOVER,
};

pub const LIGHT_INK: Ink = Ink {
    card: (255, 255, 255),
    border: (220, 224, 232),
    track: (228, 232, 238),
    hist_track: (248, 249, 251),
    text: (24, 28, 36),
    dim: (94, 102, 118),
    faint: (146, 153, 167),
    close_hover: (214, 52, 74),
    row_hover: (242, 244, 248),
};

pub fn ink() -> Ink {
    if light() { LIGHT_INK } else { DARK_INK }
}

pub fn lerp(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    (m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

/// "#2ec4b6" -> (46, 196, 182)
pub fn hex(s: &str) -> Option<Rgb> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some(((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

#[derive(Clone, Debug, PartialEq)]
pub enum Theme {
    Heat,
    Neon,
    Ice,
    /// A gradient between the user's two colours (low load -> high load).
    Custom(Rgb, Rgb),
}

impl Theme {
    /// The scale; in the window's light look, darkened to read on white.
    pub fn scale(&self) -> Scale {
        let s = self.scale_dark();
        if light() { s.on_light() } else { s }
    }

    /// The scale as on the dark cards (the widget, the overlay).
    pub fn scale_dark(&self) -> Scale {
        let stops = |list: &[&str]| list.iter().filter_map(|h| hex(h)).collect();
        Scale {
            stops: match self {
                Theme::Heat => stops(&["#2ec4b6", "#9ccc65", "#f2b33d", "#f27a3d", "#e8445a"]),
                Theme::Neon => stops(&["#3a86ff", "#8338ec", "#d61f9a", "#ff2e63"]),
                Theme::Ice => stops(&["#4f6a8f", "#6f9fd8", "#a9cdf5", "#eef6ff"]),
                Theme::Custom(a, b) => vec![*a, *b],
            },
        }
    }

    /// "heat", "neon", "ice" (1.8's config names); anything else is heat.
    pub fn from_name(name: &str) -> Theme {
        match name {
            "neon" => Theme::Neon,
            "ice" => Theme::Ice,
            _ => Theme::Heat,
        }
    }
}

/// Maps 0..1 to a colour along the theme's stops.
#[derive(Clone, Debug, PartialEq)]
pub struct Scale {
    stops: Vec<Rgb>,
}

/// Relative brightness 0..1 (Rec. 601 weights).
fn luma(c: Rgb) -> f32 {
    (0.299 * c.0 as f32 + 0.587 * c.1 as f32 + 0.114 * c.2 as f32) / 255.0
}

impl Scale {
    /// The same scale for a white background: every stop brighter than a
    /// mid tone is darkened to it, the hue kept (ice's near-white top turns
    /// slate, heat's amber turns ochre).
    pub fn on_light(&self) -> Scale {
        const MAX: f32 = 0.52;
        let stops = self
            .stops
            .iter()
            .map(|&c| {
                let l = luma(c);
                if l <= MAX {
                    return c;
                }
                let k = MAX / l;
                let f = |v: u8| (v as f32 * k).round() as u8;
                (f(c.0), f(c.1), f(c.2))
            })
            .collect();
        Scale { stops }
    }

    pub fn at(&self, t: f32) -> Rgb {
        let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
        let n = self.stops.len() - 1;
        let i = ((t * n as f32) as usize).min(n - 1);
        lerp(self.stops[i], self.stops[i + 1], t * n as f32 - i as f32)
    }

    /// Colour of a temperature (30..100 °C).
    pub fn heat(&self, celsius: f32) -> Rgb {
        self.at((celsius - 30.0) / 70.0)
    }

    pub fn ping(&self, ms: Option<u32>) -> Rgb {
        match ms {
            None => self.at(1.0),
            Some(m) if m < 50 => self.at(0.05),
            Some(m) if m < 100 => self.at(0.55),
            Some(_) => self.at(0.95),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_scale_reads_on_white() {
        // dark stops stay, bright ones come down to a mid tone with their hue
        let ice = Theme::Ice.scale_dark().on_light();
        assert_eq!(ice.at(0.0), (0x4f, 0x6a, 0x8f));
        let top = ice.at(1.0);
        assert!(luma(top) <= 0.53, "{top:?}");
        assert!(top.2 > top.0, "still bluish: {top:?}");
        let heat = Theme::Heat.scale_dark().on_light();
        assert!(heat.stops.iter().all(|&c| luma(c) <= 0.53));
    }

    #[test]
    fn scale_like_1_8() {
        let s = Theme::Heat.scale();
        assert_eq!(s.at(0.0), (0x2e, 0xc4, 0xb6));
        assert_eq!(s.at(1.0), (0xe8, 0x44, 0x5a));
        assert_eq!(s.at(0.5), (0xf2, 0xb3, 0x3d), "the middle stop");
        assert_eq!(s.at(-3.0), s.at(0.0));
        assert_eq!(hex("#FFFFFF"), Some((255, 255, 255)));
        assert_eq!(hex("nope"), None);
        let c = Theme::Custom((0, 0, 0), (200, 100, 0)).scale();
        assert_eq!(c.at(0.5), (100, 50, 0));
    }
}

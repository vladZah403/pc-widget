//! Colours: the fixed palette of 1.8 and the "cold -> hot" scales of the themes.

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
    pub fn scale(&self) -> Scale {
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

impl Scale {
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

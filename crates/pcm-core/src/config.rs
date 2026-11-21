//! Settings in TOML. A broken or hand-edited file never stops the program:
//! unknown keys are ignored, bad values fall back to their defaults
//! (1.8's sanitize_config).

use serde::{Deserialize, Serialize};

pub const SIZE_MIN: u32 = 80;
pub const SIZE_MAX: u32 = 200;
pub const SIZE_STEP: u32 = 5;
pub const WIDTH_STD: u32 = 264;
pub const WIDTH_MAX: u32 = 900;
pub const STRETCH_MAX: u32 = 600;
pub const THEMES: &[&str] = &["heat", "neon", "ice", "custom"];
pub const IN_GAMES: &[&str] = &["hide", "dim", "off"];
/// Rows that can be switched off, with their names in the menu (1.8's SECTIONS).
pub const SECTIONS: &[(&str, &str)] = &[
    ("cores", "Ядра"),
    ("ram", "Память"),
    ("gpu", "Видеокарта"),
    ("disks", "Диски"),
    ("temps", "Температура"),
    ("net", "Сеть"),
    ("net_graph", "График сети"),
    ("ping", "Пинг"),
    ("disk_io", "Чтение и запись диска"),
    ("battery", "Износ батареи"),
    ("ssd", "Здоровье SSD"),
    ("fans", "Вентиляторы"),
    ("uptime", "Время без перезагрузки"),
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Top-left corner on the desktop (physical px); None = place it.
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub theme: String,
    /// The "custom" theme: low load -> high load.
    pub custom_colors: [String; 2],
    /// Widget size in % (80..200, steps of 5).
    pub size: u32,
    /// Logical width (264 = standard; wider makes columns).
    pub width: u32,
    /// Extra height for the graphs.
    pub stretch: u32,
    pub compact: bool,
    /// Lives on the desktop (survives Win+D, below other windows).
    pub pinned: bool,
    /// Above other windows when not pinned.
    pub topmost: bool,
    /// Opacity 0.3..1.
    pub alpha: f32,
    pub hidden: Vec<String>,
    pub ping_host: String,
    /// While a game or a fullscreen video is in front: "hide", "dim" or "off".
    pub in_games: String,
    /// Tucked behind the screen edge (only the side tab shows).
    pub tucked: bool,
    /// The docked side tab's top (physical px).
    pub tab_y: Option<i32>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            x: None,
            y: None,
            theme: "heat".into(),
            custom_colors: ["#2ec4b6".into(), "#e8445a".into()],
            size: 100,
            width: WIDTH_STD,
            stretch: 0,
            compact: false,
            pinned: true,
            topmost: true,
            alpha: 1.0,
            hidden: Vec::new(),
            ping_host: "1.1.1.1".into(),
            in_games: "hide".into(),
            tucked: false,
            tab_y: None,
        }
    }
}

fn is_color(s: &str) -> bool {
    let s = s.trim_start_matches('#');
    s.len() == 6 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// A host name or IPv4 address, nothing else.
pub fn clean_host(text: &str) -> Option<String> {
    let mut t = text.trim().to_lowercase();
    if let Some(i) = t.find("://") {
        t = t[i + 3..].to_string();
    }
    let t = t.split('/').next()?.split(':').next()?.trim_end_matches('.').to_string();
    let ok = !t.is_empty()
        && t.len() <= 253
        && t.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        });
    ok.then_some(t)
}

impl Config {
    /// Every value back into its range; what can't be fixed gets its default.
    pub fn sanitize(&mut self) {
        let d = Config::default();
        let in_screen = |v: Option<i32>| v.filter(|v| (-32000..32000).contains(v));
        self.x = in_screen(self.x);
        self.y = in_screen(self.y);
        if self.x.is_none() || self.y.is_none() {
            (self.x, self.y) = (None, None);
        }
        if !THEMES.contains(&self.theme.as_str()) {
            self.theme = d.theme.clone();
        }
        for (c, def) in self.custom_colors.iter_mut().zip(d.custom_colors.iter()) {
            if !is_color(c) {
                *c = def.clone();
            }
        }
        self.size = ((self.size.clamp(SIZE_MIN, SIZE_MAX) + SIZE_STEP / 2) / SIZE_STEP) * SIZE_STEP;
        self.width = self.width.clamp(WIDTH_STD, WIDTH_MAX);
        self.stretch = self.stretch.min(STRETCH_MAX);
        self.alpha = if self.alpha.is_finite() { self.alpha.clamp(0.3, 1.0) } else { 1.0 };
        self.hidden.retain(|h| SECTIONS.iter().any(|(k, _)| k == h));
        if !IN_GAMES.contains(&self.in_games.as_str()) {
            self.in_games = d.in_games.clone();
        }
        self.tab_y = in_screen(self.tab_y);
        self.hidden.dedup();
        self.ping_host = clean_host(&self.ping_host).unwrap_or(d.ping_host);
    }

    /// Settings from TOML text; a broken file gives the defaults.
    pub fn parse(text: &str) -> Config {
        let mut c: Config = toml::from_str(text).unwrap_or_default();
        c.sanitize();
        c
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(self).unwrap_or_default()
    }

    pub fn load(path: &std::path::Path) -> Config {
        std::fs::read_to_string(path).map(|t| Config::parse(&t)).unwrap_or_default()
    }

    /// Write through a temporary file, so a crash mid-write can't leave half a file.
    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, self.to_toml())?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults() {
        let mut c = Config { x: Some(100), y: Some(50), compact: true, ..Config::default() };
        c.sanitize();
        assert_eq!(Config::parse(&c.to_toml()), c);
        assert_eq!(Config::parse("this is not toml"), Config::default());
        assert_eq!(Config::parse(""), Config::default());
    }

    #[test]
    fn bad_values_are_fixed() {
        let c = Config::parse(
            "x = 99999\ny = 5\ntheme = \"pink\"\nsize = 137\nwidth = 10\nstretch = 5000\nalpha = 7.0\n\
             custom_colors = [\"#zzzzzz\", \"#112233\"]\nping_host = \"https://Eu.Battle.net/x\"\nunknown = 1\n",
        );
        assert_eq!((c.x, c.y), (None, None), "half a position is no position");
        assert_eq!(c.theme, "heat");
        assert_eq!((c.size, c.width, c.stretch, c.alpha), (135, 264, 600, 1.0));
        assert_eq!(c.custom_colors, ["#2ec4b6".to_string(), "#112233".to_string()]);
        assert_eq!(c.ping_host, "eu.battle.net");
        assert_eq!(Config::parse("size = 999").size, 200);
        assert_eq!(Config::parse("ping_host = \"bad host!\"").ping_host, "1.1.1.1");
    }
}

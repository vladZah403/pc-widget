//! Settings in TOML. A broken or hand-edited file never stops the program:
//! unknown keys are ignored, bad values fall back to their defaults
//! (1.8's sanitize_config).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const SIZE_MIN: u32 = 80;
pub const SIZE_MAX: u32 = 200;
pub const SIZE_STEP: u32 = 5;
pub const WIDTH_STD: u32 = 264;
pub const WIDTH_MAX: u32 = 900;
pub const STRETCH_MAX: u32 = 600;
pub const THEMES: &[&str] = &["heat", "neon", "ice", "custom"];
pub const IN_GAMES: &[&str] = &["hide", "dim", "off"];
/// Corners of the game window for the FPS panel: top-left, top-right, bottom-left, bottom-right.
pub const FPS_CORNERS: &[&str] = &["tl", "tr", "bl", "br"];
/// FPS panel sizes in %.
pub const FPS_SIZES: &[u32] = &[100, 125, 150];
/// The interface language: "auto" = the Windows one.
pub const LANGS: &[&str] = &["auto", "ru", "en", "uk", "de"];
/// "work" and "game": each remembers how the widget looks.
pub const PROFILES: &[&str] = &["work", "game"];

/// What a profile remembers (1.8's PROFILE_KEYS).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileView {
    pub compact: bool,
    pub hidden: Vec<String>,
    pub size: u32,
    pub width: u32,
    pub stretch: u32,
    pub theme: String,
    pub alpha: f32,
    pub taskbar: Option<bool>,
    pub pinned: bool,
    pub topmost: bool,
}

impl Default for ProfileView {
    fn default() -> Self {
        Config::default().view()
    }
}
/// Readings a tray icon can show, with their menu names.
pub const TRAY_KINDS: &[(&str, &str)] = &[
    ("cpu", "Загрузка CPU"),
    ("cpu_temp", "Температура CPU"),
    ("gpu", "Загрузка видеокарты"),
    ("gpu_temp", "Температура видеокарты"),
    ("ram", "Память"),
];
pub const TRAY_MAX: usize = 3;
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
    /// The widget is shown (hidden, the program stays in the tray).
    pub widget: bool,
    /// What the tray icons show, 1 to 3 of TRAY_KINDS.
    pub tray: Vec<String>,
    /// Alert cards (overheating, memory, battery, disk space, SSD).
    pub alerts: bool,
    /// °C
    pub alert_temp: u32,
    /// % of memory in use
    pub alert_ram: u32,
    /// % of battery left
    pub alert_battery: u32,
    /// % free on the system drive
    pub alert_disk: u32,
    /// Alerts wait until a game or a fullscreen video is over.
    pub quiet_games: bool,
    /// The mini panel on the taskbar (None = on, except on Windows 11).
    pub taskbar: Option<bool>,
    /// Count frames (ETW): the FPS panel, game detection, sessions.
    pub fps: bool,
    /// The FPS panel over the game (Ctrl+Alt+F).
    pub fps_panel: bool,
    /// One of FPS_CORNERS.
    pub fps_corner: String,
    /// One of FPS_SIZES.
    pub fps_size: u32,
    /// One of PROFILES.
    pub profile: String,
    /// Switch to "game" while a game is in front, back after it.
    pub auto_game: bool,
    /// One of LANGS.
    pub lang: String,
    /// Temperatures need the sensor helper with admin rights (1.8's run_as_admin).
    pub run_as_admin: bool,
    /// Look for a new version every 6 hours.
    pub check_updates: bool,
    /// The newest version the user has been told about.
    pub update_seen: String,
    /// The saved look of each profile (last: TOML puts tables after values).
    pub profiles: BTreeMap<String, ProfileView>,
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
            widget: true,
            tray: vec!["cpu".into()],
            alerts: true,
            alert_temp: 90,
            alert_ram: 90,
            alert_battery: 15,
            alert_disk: 10,
            quiet_games: true,
            taskbar: None,
            fps: true,
            fps_panel: true,
            fps_corner: "tl".into(),
            fps_size: 100,
            profile: "work".into(),
            auto_game: true,
            lang: "auto".into(),
            run_as_admin: false,
            check_updates: true,
            update_seen: String::new(),
            profiles: BTreeMap::new(),
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
        let mut tray: Vec<String> = Vec::new();
        for k in &self.tray {
            if TRAY_KINDS.iter().any(|(t, _)| t == k) && !tray.contains(k) && tray.len() < TRAY_MAX {
                tray.push(k.clone());
            }
        }
        self.tray = if tray.is_empty() { d.tray.clone() } else { tray };
        self.alert_temp = self.alert_temp.clamp(60, 105);
        self.alert_ram = self.alert_ram.clamp(50, 99);
        self.alert_battery = self.alert_battery.clamp(5, 50);
        self.alert_disk = self.alert_disk.clamp(1, 50);
        self.hidden.dedup();
        self.ping_host = clean_host(&self.ping_host).unwrap_or(d.ping_host);
        if !FPS_CORNERS.contains(&self.fps_corner.as_str()) {
            self.fps_corner = d.fps_corner.clone();
        }
        if !FPS_SIZES.contains(&self.fps_size) {
            self.fps_size = d.fps_size;
        }
        if !LANGS.contains(&self.lang.as_str()) {
            self.lang = d.lang.clone();
        }
        if !PROFILES.contains(&self.profile.as_str()) {
            self.profile = d.profile.clone();
        }
        // each saved look is checked like the settings themselves
        let profiles = std::mem::take(&mut self.profiles);
        for (name, view) in profiles {
            if PROFILES.contains(&name.as_str()) {
                let mut c = Config::default();
                c.apply_view(&view);
                c.sanitize();
                self.profiles.insert(name, c.view());
            }
        }
    }

    /// How the widget looks now.
    pub fn view(&self) -> ProfileView {
        ProfileView {
            compact: self.compact,
            hidden: self.hidden.clone(),
            size: self.size,
            width: self.width,
            stretch: self.stretch,
            theme: self.theme.clone(),
            alpha: self.alpha,
            taskbar: self.taskbar,
            pinned: self.pinned,
            topmost: self.topmost,
        }
    }

    pub fn apply_view(&mut self, v: &ProfileView) {
        self.compact = v.compact;
        self.hidden = v.hidden.clone();
        self.size = v.size;
        self.width = v.width;
        self.stretch = v.stretch;
        self.theme = v.theme.clone();
        self.alpha = v.alpha;
        self.taskbar = v.taskbar;
        self.pinned = v.pinned;
        self.topmost = v.topmost;
    }

    /// The saved look of a profile; "game" starts as the compact widget.
    pub fn profile_view(&self, name: &str) -> ProfileView {
        if let Some(v) = self.profiles.get(name) {
            return v.clone();
        }
        let mut v = self.view();
        if name == "game" {
            v.compact = true;
        }
        v
    }

    /// Remember the current look as the current profile's and take on
    /// `name`'s. False when `name` already is the profile.
    pub fn switch_profile(&mut self, name: &str) -> bool {
        if name == self.profile || !PROFILES.contains(&name) {
            return false;
        }
        self.profiles.insert(self.profile.clone(), self.view());
        let v = self.profile_view(name);
        self.profile = name.to_string();
        self.apply_view(&v);
        self.sanitize();
        true
    }

    /// 1.8's widget_config.json as 2.0 settings (None if it isn't JSON). The
    /// same names mean the same things; 1.8's single tray icon becomes the CPU
    /// one, its taskbar on/off a definite choice. Then checked like any file.
    pub fn from_1_8_json(text: &str) -> Option<Config> {
        let v: serde_json::Value = serde_json::from_str(text).ok()?;
        let o = v.as_object()?;
        let mut c = Config::default();
        let b = |k: &str| o.get(k).and_then(|x| x.as_bool());
        let n = |k: &str| o.get(k).and_then(|x| x.as_f64());
        let s = |k: &str| o.get(k).and_then(|x| x.as_str()).map(str::to_string);
        let u = |k: &str| n(k).filter(|v| *v >= 0.0).map(|v| v.round() as u32);
        if let (Some(x), Some(y)) = (n("x"), n("y")) {
            (c.x, c.y) = (Some(x.round() as i32), Some(y.round() as i32));
        }
        macro_rules! take {
            ($($field:ident: $get:ident),* $(,)?) => {
                $(if let Some(v) = $get(stringify!($field)) { c.$field = v.into(); })*
            };
        }
        take!(
            topmost: b, pinned: b, alerts: b, compact: b, run_as_admin: b, check_updates: b, fps: b,
            fps_panel: b, auto_game: b, quiet_games: b, tucked: b,
            theme: s, ping_host: s, lang: s, in_games: s, fps_corner: s, profile: s, update_seen: s,
            alert_temp: u, alert_ram: u, alert_battery: u, alert_disk: u, size: u, width: u, stretch: u,
            fps_size: u,
        );
        if let Some(a) = n("alpha") {
            c.alpha = a as f32;
        }
        c.taskbar = b("taskbar").or(c.taskbar);
        if let Some(list) = o.get("custom_colors").and_then(|x| x.as_array())
            && let [lo, hi] = list.as_slice()
            && let (Some(lo), Some(hi)) = (lo.as_str(), hi.as_str())
        {
            c.custom_colors = [lo.to_string(), hi.to_string()];
        }
        if let Some(list) = o.get("hidden").and_then(|x| x.as_array()) {
            c.hidden = list.iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
        }
        if let Some(map) = o.get("profiles").and_then(|x| x.as_object()) {
            for (name, view) in map {
                if let Ok(v) = serde_json::from_value::<ProfileView>(view.clone()) {
                    c.profiles.insert(name.clone(), v);
                }
            }
        }
        c.sanitize();
        Some(c)
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
        let f = Config::parse("fps_corner = \"middle\"\nfps_size = 110");
        assert_eq!((f.fps_corner.as_str(), f.fps_size), ("tl", 100));
        assert_eq!(Config::parse("fps_corner = \"br\"").fps_corner, "br");
        assert_eq!(Config::parse("profile = \"party\"").profile, "work");
    }

    #[test]
    fn settings_of_1_8() {
        let json = r##"{"x": 1326, "y": 282, "topmost": true, "alpha": 0.97, "theme": "heat",
            "custom_colors": ["#00ffff", "#0000ff"], "pinned": false, "taskbar": true, "alerts": true,
            "compact": true, "run_as_admin": true, "hidden": ["uptime", "nonsense"], "alert_temp": 85,
            "alert_ram": 90, "alert_battery": 15, "check_updates": true, "update_seen": "1.8.1",
            "tray": true, "ping_host": "8.8.8.8", "fps": true, "lang": "auto", "in_games": "dim",
            "size": 125, "width": 528, "stretch": 40, "alert_disk": 10, "fps_panel": false,
            "fps_corner": "br", "fps_size": 150, "profile": "game",
            "profiles": {"game": {"compact": false, "hidden": ["uptime"], "size": 100, "width": 264,
                "stretch": 0, "theme": "neon", "alpha": 0.9, "taskbar": true, "pinned": false,
                "topmost": true}},
            "auto_game": false, "quiet_games": true, "tucked": false, "something_new": 1}"##;
        let c = Config::from_1_8_json(json).unwrap();
        assert_eq!((c.x, c.y), (Some(1326), Some(282)));
        assert_eq!((c.pinned, c.compact, c.run_as_admin, c.auto_game), (false, true, true, false));
        assert!((c.alpha - 0.97).abs() < 1e-6);
        assert_eq!(c.custom_colors, ["#00ffff".to_string(), "#0000ff".to_string()]);
        assert_eq!(c.hidden, vec!["uptime".to_string()], "unknown rows dropped");
        assert_eq!((c.alert_temp, c.size, c.width, c.stretch), (85, 125, 528, 40));
        assert_eq!((c.ping_host.as_str(), c.in_games.as_str()), ("8.8.8.8", "dim"));
        assert_eq!((c.fps_panel, c.fps_corner.as_str(), c.fps_size), (false, "br", 150));
        assert_eq!((c.profile.as_str(), c.update_seen.as_str()), ("game", "1.8.1"));
        assert_eq!(c.taskbar, Some(true));
        assert_eq!(c.profiles["game"].theme, "neon");
        assert_eq!(c.tray, vec!["cpu".to_string()]);
        assert!(Config::from_1_8_json("not json").is_none());
        let odd = Config::from_1_8_json(r#"{"size": 999, "theme": "pink", "x": 5}"#).unwrap();
        assert_eq!((odd.size, odd.theme.as_str(), odd.x), (200, "heat", None), "checked like a file");
    }

    #[test]
    fn profiles_remember_the_look() {
        let mut c = Config { size: 150, theme: "ice".into(), ..Config::default() };
        assert!(c.switch_profile("game"));
        assert!(c.compact, "game starts compact");
        assert_eq!((c.size, c.theme.as_str()), (150, "ice"), "the rest as it was");
        c.size = 100;
        assert!(!c.switch_profile("game"), "already there");
        assert!(c.switch_profile("work"));
        assert_eq!((c.compact, c.size), (false, 150), "work's look is back");
        assert!(c.switch_profile("game"));
        assert_eq!((c.compact, c.size), (true, 100), "game's saved look");
        // the profiles survive the file, and a broken saved look is fixed
        let back = Config::parse(&c.to_toml());
        assert_eq!(back, c);
        let odd =
            Config::parse("[profiles.game]\nsize = 999\ntheme = \"pink\"\n[profiles.party]\nsize = 100\n");
        assert_eq!(odd.profiles.len(), 1);
        assert_eq!((odd.profiles["game"].size, odd.profiles["game"].theme.as_str()), (200, "heat"));
    }
}

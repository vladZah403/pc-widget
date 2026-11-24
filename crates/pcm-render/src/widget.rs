//! The desktop widget, a port of 1.8's `Renderer`: a header, the CPU block,
//! then rows in three groups (resources, sensors, network) and a footer. A
//! narrow widget stacks the groups; a wide one puts them side by side in 2
//! or 3 columns. All sizes are logical px; `View::scale` makes them physical.

use serde::{Deserialize, Serialize};
use tiny_skia::Pixmap;

use crate::canvas::{Align, Canvas, Part, part};
use crate::format::*;
use crate::text::{Fonts, Style};
use crate::theme::*;

mod cards_look;
pub use cards_look::{CARDS_W, COMPACT_W, HOVER_W, hover_card, hover_card_with, tray_icon_filled};

pub const PAD: f32 = 16.0;
pub const WIDTH_STD: f32 = 264.0;
pub const WIDTH_MAX: f32 = 900.0;
pub const STRETCH_MAX: f32 = 600.0;
pub const RADIUS: f32 = 14.0;
/// Header + CPU block.
pub const HERO_H: f32 = 128.0;
/// Icon + gap before a label.
pub const ICON_W: f32 = 16.0;
pub const HISTORY_LEN: usize = 60;
/// The network graph's scale never goes below 64 KB/s.
pub const NET_GRAPH_FLOOR: f64 = 64.0 * 1024.0;
/// The resize corner.
pub const GRIP: f32 = 22.0;

/// History subjects a row opens on a click (1.8's HISTORY_ROWS).
pub const SUBJECT_CPU: usize = 0;
pub const SUBJECT_GPU: usize = 1;
pub const SUBJECT_RAM: usize = 2;
pub const SUBJECT_NET: usize = 3;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Drive {
    /// "Samsung SSD 980 PRO 1TB"
    pub model: String,
    pub letters: Vec<String>,
    pub critical: u8,
    pub temp: Option<f32>,
    pub spare: u8,
    pub spare_min: u8,
    pub wear: u8,
    pub written: Option<u64>,
}

/// A program in the PC Monitor window's process list.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProcRow {
    pub name: String,
    /// % of the whole PC.
    pub cpu: f32,
    /// Private bytes of all its processes.
    pub memory: u64,
    /// How many processes it has.
    pub count: u32,
    /// Bytes per second it sends and receives (None: unknown, no admin rights).
    #[serde(default)]
    pub net: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum UpdateState {
    Download(f32),
    Install,
}

/// Everything the widget shows (1.8's `_collect()` dict).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Data {
    pub cpu: f32,
    /// The last minute, oldest first (up to 60).
    pub cpu_hist: Vec<f32>,
    pub cores: Vec<f32>,
    pub cpu_name: String,
    pub cpu_clock_ghz: Option<f32>,
    /// Windows' "% Performance Limit" of the processor (100: not held back).
    pub cpu_limit: Option<f32>,
    pub cpu_power_w: Option<f32>,
    pub ram_pct: f32,
    pub ram_used: u64,
    pub ram_total: u64,
    /// (name "C:", used, total, percent)
    pub disks: Vec<(String, u64, u64, f32)>,
    pub io_read: f64,
    pub io_write: f64,
    pub net_down: f64,
    pub net_up: f64,
    pub net_hist: Vec<(f64, f64)>,
    /// (percent, plugged in)
    pub battery: Option<(f32, bool)>,
    pub battery_left: Option<u32>,
    pub battery_wear: Option<f32>,
    pub battery_full: Option<f32>,
    pub battery_design: Option<f32>,
    pub battery_rate: Option<f32>,
    pub uptime_s: u64,
    pub temp_cpu: Option<f32>,
    pub temp_cpu_approx: bool,
    pub temp_gpu: Option<f32>,
    /// Each temperature's [min, average, max] since PC Monitor started:
    /// "cpu", "gpu" and the drives by letter ("C:").
    #[serde(default)]
    pub temp_stats: Vec<(String, [f32; 3])>,
    /// Integrated graphics without a sensor: shows the CPU's temperature "≈".
    pub gpu_shared: bool,
    pub temp_ssd: Option<f32>,
    pub gpu_load: Option<f32>,
    pub gpu_name: Option<String>,
    /// That GPU's video memory (MB) and clock (MHz), for the FPS panel.
    pub vram_used_mb: Option<f32>,
    pub vram_total_mb: Option<f32>,
    pub gpu_clock: Option<f32>,
    pub drives: Vec<Drive>,
    pub fans: Vec<f32>,
    /// A temperature source answers (1.8's lhm_online).
    pub sensors_online: bool,
    pub sensor_hint: Option<String>,
    pub ping: Option<u32>,
    pub ping_ready: bool,
    pub ping_host: String,
    /// The process list, shown after a click on the CPU block.
    pub procs: Option<Vec<(String, f32, u64)>>,
    /// A program whose × was clicked once (the next click closes it).
    pub procs_armed: Option<String>,
    /// Every program (busiest first), while the window's process tab is open.
    pub procs_all: Option<Vec<ProcRow>>,
    /// Programs that can't be closed from the list.
    pub keep_running: Vec<String>,
    pub update: Option<String>,
    pub update_state: Option<UpdateState>,
    pub update_auto: bool,
    // --- for the PC Monitor window (the widget doesn't draw these) ---
    /// Physical cores.
    pub cpu_cores: Option<u32>,
    /// The GPU's load over the last minute, oldest first (up to 60).
    pub gpu_hist: Vec<f32>,
    /// Memory in use (%) and disk reads + writes (bytes/s) over the last minute.
    pub ram_hist: Vec<f32>,
    pub io_hist: Vec<f32>,
    /// RAM from SMBIOS: "DDR4", MT/s, each stick's size in bytes.
    pub mem_kind: Option<String>,
    pub mem_speed: Option<u32>,
    pub mem_sticks: Vec<u64>,
    /// The main adapter's IPv4 address.
    pub net_ip: Option<String>,
    /// Processes running (while the window asks for the process list).
    pub proc_count: Option<u32>,
    /// The fans' names, as `fans`.
    pub fan_names: Vec<String>,
    /// "Windows 11 Pro"
    pub windows: String,
    pub pawnio: bool,
    /// PC Monitor's own CPU (% of the PC) and private memory.
    pub own_cpu: Option<f32>,
    pub own_mem: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Right,
    Bottom,
}

/// How the widget looks: 1.8's rows, the concept's cards, or round gauges.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WidgetLook {
    #[default]
    Classic,
    Cards,
    /// Four round gauges in a grid (NZXT CAM's way), the card look's header and footer.
    Gauges,
}

impl WidgetLook {
    /// "cards" / "gauges" / "classic" (the settings' names); anything else is classic.
    pub fn from_name(name: &str) -> WidgetLook {
        match name {
            "cards" => WidgetLook::Cards,
            "gauges" => WidgetLook::Gauges,
            _ => WidgetLook::Classic,
        }
    }

    /// Cards and gauges: a fixed width, light or dark, a header with "›".
    pub fn modern(self) -> bool {
        self != WidgetLook::Classic
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub look: WidgetLook,
    /// Light colours (the cards look only; classic stays dark).
    pub light: bool,
    /// The mouse is over "›" (the cards look's way to the PC Monitor window).
    pub open_hover: bool,
    pub theme: Theme,
    pub scale: f32,
    /// Logical width, WIDTH_STD..WIDTH_MAX.
    pub width: f32,
    /// Extra height the CPU and network graphs grow by, 0..STRETCH_MAX.
    pub stretch: f32,
    pub compact: bool,
    /// Rows switched off: "cores", "ram", "gpu", "disks", "temps", "battery",
    /// "ssd", "fans", "net", "net_graph", "ping", "disk_io", "uptime".
    pub hidden: Vec<String>,
    pub pinned: bool,
    pub pin_hover: bool,
    pub close_hover: bool,
    pub close_armed: bool,
    pub hover_row: Option<usize>,
    /// 0 = no resize corner, 1 = shown, 2 = in use.
    pub grip: u8,
    pub edge: Option<Edge>,
}

impl Default for View {
    fn default() -> Self {
        View {
            look: WidgetLook::Classic,
            light: false,
            open_hover: false,
            theme: Theme::Heat,
            scale: 1.0,
            width: WIDTH_STD,
            stretch: 0.0,
            compact: false,
            hidden: Vec::new(),
            pinned: false,
            pin_hover: false,
            close_hover: false,
            close_armed: false,
            hover_row: None,
            grip: 0,
            edge: None,
        }
    }
}

/// [left, top, right, bottom] in physical px of the frame.
pub type BoxPx = [f32; 4];

/// Where the clickable things are in the frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Boxes {
    pub close: BoxPx,
    /// "›" in the cards look: open the PC Monitor window.
    pub open: BoxPx,
    pub pin: BoxPx,
    pub hero: BoxPx,
    pub update: BoxPx,
    pub grip: BoxPx,
    /// (program, its ×)
    pub procs: Vec<(String, BoxPx)>,
    /// (history subject, row) for the rows a click opens the history of
    pub rows: Vec<(usize, BoxPx)>,
}

pub struct Frame {
    pub pm: Pixmap,
    pub boxes: Boxes,
}

#[derive(Clone, Debug)]
enum Kind {
    Update,
    Procs,
    Cores,
    Ram,
    Gpu,
    Disk(usize),
    Temps,
    Battery,
    Ssd(usize, bool),
    Fans,
    Divider,
    Net,
    NetGraph(f32),
    Ping,
    DiskIo,
    Footer,
    Space,
}

#[derive(Clone, Debug)]
struct Row {
    h: f32,
    kind: Kind,
}

impl Row {
    fn new(h: f32, kind: Kind) -> Row {
        Row { h, kind }
    }

    fn subject(&self) -> Option<usize> {
        match self.kind {
            Kind::Cores | Kind::Temps => Some(SUBJECT_CPU),
            Kind::Gpu => Some(SUBJECT_GPU),
            Kind::Ram => Some(SUBJECT_RAM),
            Kind::Net | Kind::NetGraph(_) => Some(SUBJECT_NET),
            _ => None,
        }
    }
}

pub fn clean_width(w: f32) -> f32 {
    if w.is_finite() { w.round().clamp(WIDTH_STD, WIDTH_MAX) } else { WIDTH_STD }
}

pub fn clean_stretch(s: f32) -> f32 {
    if s.is_finite() { s.round().clamp(0.0, STRETCH_MAX) } else { 0.0 }
}

struct Layout {
    hero_extra: f32,
    /// (x offset, width, rows)
    cols: Vec<(f32, f32, Vec<Row>)>,
    footer: Row,
    body: f32,
}

pub struct Renderer<'a> {
    pub fonts: &'a mut Fonts,
}

/// A compact-mode reading: icon, name, value, colour, approximate, history subject.
type Cell<'a> = (&'a str, Option<&'a str>, Option<String>, Rgb, bool, Option<usize>);

/// The state of one render: the canvas, the column width, the colours.
struct Ctx<'f> {
    c: Canvas,
    f: &'f mut Fonts,
    /// Width of the column being drawn (the whole widget outside columns).
    w: f32,
    scale: Scale,
    boxes: Boxes,
}

const T: Style = Style::Regular;
const SB: Style = Style::SemiBold;
const SBC: Style = Style::SemiBoldCondensed;
// sizes of 1.8's fonts
const TITLE: f32 = 14.0;
const HERO: f32 = 46.0;
const HERO_UNIT: f32 = 20.0;
const LABEL: f32 = 12.0;
const VALUE: f32 = 12.0;
const SMALL: f32 = 11.0;
const SYM: f32 = 13.0;

fn hidden(view: &View, key: &str) -> bool {
    view.hidden.iter().any(|h| h == key)
}

fn temps_online(d: &Data) -> bool {
    d.sensors_online && (d.temp_cpu.is_some() || d.temp_gpu.is_some() || d.temp_ssd.is_some())
}

impl Renderer<'_> {
    pub fn new(fonts: &mut Fonts) -> Renderer<'_> {
        Renderer { fonts }
    }

    /// How many columns the width holds: each one at least the standard width.
    pub fn columns(width: f32) -> usize {
        ((width / WIDTH_STD) as usize).clamp(1, 3)
    }

    /// Compact mode: 2 rows of 3 readings, or all 6 in one row from twice the
    /// standard width.
    pub fn compact_rows(width: f32) -> usize {
        if width >= 2.0 * WIDTH_STD { 1 } else { 2 }
    }

    /// How the extra height is shared: (CPU graph, network graph).
    fn extras(view: &View, compact: bool) -> (f32, f32) {
        let stretch = clean_stretch(view.stretch);
        if stretch == 0.0 {
            return (0.0, 0.0);
        }
        if compact || hidden(view, "net") || hidden(view, "net_graph") {
            return (stretch, 0.0);
        }
        let hero = (stretch * 0.6).round();
        (hero, stretch - hero)
    }

    /// (resources, sensors, network, footer)
    fn groups(d: &Data, view: &View, net_extra: f32) -> (Vec<Row>, Vec<Row>, Vec<Row>, Row) {
        let mut secs = Vec::new();
        if d.update.is_some() {
            secs.push(Row::new(30.0, Kind::Update));
        }
        if d.procs.is_some() {
            secs.push(Row::new(120.0, Kind::Procs));
        }
        if !hidden(view, "cores") && !d.cores.is_empty() {
            secs.push(Row::new(38.0, Kind::Cores));
        }
        if !hidden(view, "ram") {
            secs.push(Row::new(38.0, Kind::Ram));
        }
        if !hidden(view, "gpu") {
            secs.push(Row::new(38.0, Kind::Gpu));
        }
        if !hidden(view, "disks") {
            for i in 0..d.disks.len() {
                secs.push(Row::new(38.0, Kind::Disk(i)));
            }
        }
        let mut sensors = Vec::new();
        if !hidden(view, "temps") {
            sensors.push(Row::new(40.0 + if temps_online(d) { 20.0 } else { 16.0 }, Kind::Temps));
        }
        if !hidden(view, "battery") && d.battery_wear.is_some() {
            sensors.push(Row::new(38.0, Kind::Battery));
        }
        if !hidden(view, "ssd") {
            // a single drive whose temperature the strip already shows: not twice
            let twice =
                !hidden(view, "temps") && temps_online(d) && d.temp_ssd.is_some() && d.drives.len() == 1;
            for i in 0..d.drives.len() {
                sensors.push(Row::new(38.0, Kind::Ssd(i, !twice)));
            }
        }
        if !hidden(view, "fans") && !d.fans.is_empty() {
            sensors.push(Row::new(24.0, Kind::Fans));
        }
        let mut lower = Vec::new();
        if !hidden(view, "net") {
            lower.push(Row::new(22.0, Kind::Net));
            if !hidden(view, "net_graph") {
                let gh = 26.0 + net_extra;
                lower.push(Row::new(gh, Kind::NetGraph(gh)));
            }
        }
        if !hidden(view, "ping") {
            lower.push(Row::new(22.0, Kind::Ping));
        }
        if !hidden(view, "disk_io") {
            lower.push(Row::new(22.0, Kind::DiskIo));
        }
        let footer =
            if hidden(view, "uptime") { Row::new(10.0, Kind::Space) } else { Row::new(30.0, Kind::Footer) };
        (secs, sensors, lower, footer)
    }

    /// Groups one under another, a divider between them.
    fn join(groups: &[Vec<Row>]) -> Vec<Row> {
        let mut rows: Vec<Row> = Vec::new();
        for g in groups.iter().filter(|g| !g.is_empty()) {
            if !rows.is_empty() {
                rows.push(Row::new(16.0, Kind::Divider));
            }
            rows.extend(g.iter().cloned());
        }
        rows
    }

    fn plan(d: &Data, view: &View) -> Layout {
        let width = clean_width(view.width);
        let n = Self::columns(width);
        let height = |rows: &[Row]| rows.iter().map(|r| r.h).sum::<f32>();
        if n == 1 {
            let (hero_extra, net_extra) = Self::extras(view, false);
            let (a, b, c, footer) = Self::groups(d, view, net_extra);
            let rows = Self::join(&[a, b, c]);
            let body = height(&rows);
            return Layout { hero_extra, cols: vec![(0.0, width, rows)], footer, body };
        }
        // side by side the network graph keeps its height: the CPU graph,
        // which spans the whole width, takes all the extra height
        let hero_extra = clean_stretch(view.stretch);
        let (a, b, c, footer) = Self::groups(d, view, 0.0);
        let mut groups: Vec<Vec<Row>> = [a, b, c].into_iter().filter(|g| !g.is_empty()).collect();
        if groups.is_empty() {
            groups.push(Vec::new());
        }
        let n = n.min(groups.len());
        let parts: Vec<Vec<Vec<Row>>> = if n == groups.len() {
            groups.iter().map(|g| vec![g.clone()]).collect()
        } else if n == 1 {
            vec![groups.clone()]
        } else {
            // 2 columns, 3 groups: the split with the shorter tallest column
            let tallest = |p: &[Vec<Vec<Row>>]| p.iter().map(|g| height(&Self::join(g))).fold(0.0, f32::max);
            let one = vec![vec![groups[0].clone()], groups[1..].to_vec()];
            let two = vec![groups[..2].to_vec(), vec![groups[2].clone()]];
            if tallest(&one) <= tallest(&two) { one } else { two }
        };
        let col_w = width / n as f32;
        let mut cols = Vec::new();
        for (i, p) in parts.iter().enumerate() {
            let mut rows = Self::join(p);
            if rows.first().is_some_and(|r| matches!(r.kind, Kind::Net | Kind::Ping | Kind::DiskIo)) {
                // their labels sit 2 px higher than the 38 px rows': level the column heads
                rows.insert(0, Row::new(2.0, Kind::Space));
            }
            cols.push((i as f32 * col_w, col_w, rows));
        }
        let body = cols.iter().map(|(_, _, r)| height(r)).fold(0.0, f32::max);
        Layout { hero_extra, cols, footer, body }
    }

    /// Height of the widget in logical px.
    pub fn layout_height(d: &Data, view: &View) -> f32 {
        if view.look.modern() {
            return cards_look::height(d, view);
        }
        if view.compact {
            let rows = if Self::compact_rows(clean_width(view.width)) == 2 { 50.0 } else { 28.0 };
            return HERO_H + Self::extras(view, true).0 + rows;
        }
        let l = Self::plan(d, view);
        HERO_H + l.hero_extra + l.body + l.footer.h
    }

    pub fn render(&mut self, data: &Data, view: &View) -> Option<Frame> {
        if view.look.modern() {
            return cards_look::render(self.fonts, data, view);
        }
        let s = view.scale;
        let width = clean_width(view.width);
        let mut d = data.clone();
        if view.compact {
            d.procs = None;
            d.update = None;
        }
        let h = Self::layout_height(&d, view);
        let (pw, ph) = ((width * s).round() as u32, (h * s).round() as u32);
        let mut x = Ctx {
            c: Canvas::new(pw, ph, s)?,
            f: self.fonts,
            w: width,
            scale: view.theme.scale(),
            boxes: Boxes::default(),
        };
        x.card(pw as f32, ph as f32);
        x.header(&d, view);
        let (grow, layout) = if view.compact {
            (Self::extras(view, true).0, None)
        } else {
            let l = Self::plan(&d, view);
            (l.hero_extra, Some(l))
        };
        x.hero(&d, grow, view.compact);
        match layout {
            None => x.compact_rows(&d, HERO_H + grow, view.hover_row),
            Some(l) => {
                let top = HERO_H + grow;
                // rows that open their history: a soft highlight under the hovered one
                let mut i = 0;
                for (ox, cw, rows) in &l.cols {
                    let mut y = top;
                    let start = x.boxes.rows.len();
                    let mut last: Option<(usize, f32, f32)> = None; // subject, top, bottom
                    let mut spans = Vec::new();
                    for r in rows {
                        if let Some(sub) = r.subject() {
                            match &mut last {
                                Some((ls, _, lb)) if *ls == sub && *lb == y => *lb = y + r.h,
                                _ => {
                                    if let Some(l) = last.take() {
                                        spans.push(l);
                                    }
                                    last = Some((sub, y, y + r.h));
                                }
                            }
                        }
                        y += r.h;
                    }
                    spans.extend(last);
                    let _ = start;
                    for (sub, t, b) in spans {
                        let bx = [ox + PAD - 7.0, t + 1.0, ox + cw - PAD + 7.0, b - 3.0];
                        if view.hover_row == Some(i) {
                            x.c.rrect(bx[0], bx[1], bx[2], bx[3], 8.0, ROW_HOVER);
                        }
                        x.boxes.rows.push((sub, bx.map(|v| v * s)));
                        i += 1;
                    }
                }
                for (i, (ox, cw, rows)) in l.cols.iter().enumerate() {
                    if i > 0 {
                        // a thin line between columns, like the dividers between groups
                        x.c.rect(ox - 0.5 / s, top + 6.0, ox + 0.5 / s, top + l.body - 6.0, TRACK);
                    }
                    x.c.ox = *ox;
                    x.w = *cw;
                    let mut y = top;
                    for r in rows {
                        x.row(&d, r, y);
                        y += r.h;
                    }
                    x.c.ox = 0.0;
                    x.w = width;
                }
                x.row(&d, &l.footer, top + l.body);
            }
        }
        x.grip(h, view.grip);
        x.edge_line(h, view.edge);
        Some(Frame { pm: x.c.pm, boxes: x.boxes })
    }
}

impl Ctx<'_> {
    fn card(&mut self, pw: f32, ph: f32) {
        // physical coordinates: the card fills the frame exactly
        let s = self.c.s;
        let r = (RADIUS * s).round();
        let bw = s.round().max(1.0);
        if let Some(p) = crate::canvas::rounded_rect(0.0, 0.0, pw, ph, r) {
            let mut paint = tiny_skia::Paint::default();
            paint.set_color(tiny_skia::Color::from_rgba8(BORDER.0, BORDER.1, BORDER.2, 255));
            paint.anti_alias = true;
            self.c.pm.fill_path(
                &p,
                &paint,
                tiny_skia::FillRule::Winding,
                tiny_skia::Transform::identity(),
                None,
            );
        }
        if let Some(p) = crate::canvas::rounded_rect(bw, bw, pw - bw, ph - bw, r - bw) {
            let mut paint = tiny_skia::Paint::default();
            paint.set_color(tiny_skia::Color::from_rgba8(CARD.0, CARD.1, CARD.2, 255));
            paint.anti_alias = true;
            self.c.pm.fill_path(
                &p,
                &paint,
                tiny_skia::FillRule::Winding,
                tiny_skia::Transform::identity(),
                None,
            );
        }
    }

    fn s(&self) -> f32 {
        self.c.s
    }

    fn bx(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> BoxPx {
        let s = self.s();
        [(x0 + self.c.ox) * s, y0 * s, (x1 + self.c.ox) * s, y1 * s]
    }

    fn text(&mut self, x: f32, y: f32, t: &str, style: Style, size: f32, c: Rgb) {
        self.c.text(self.f, x, y, t, style, size, c);
    }

    fn rich(&mut self, x: f32, y: f32, parts: &[Part], align: Align) {
        self.c.rich(self.f, x, y, parts, align);
    }

    fn width(&mut self, t: &str, style: Style, size: f32) -> f32 {
        self.c.width(self.f, t, style, size)
    }

    fn ellipsize(&mut self, t: &str, style: Style, size: f32, max: f32) -> String {
        self.c.ellipsize(self.f, t, style, size, max)
    }

    // --- primitives ---

    fn segment_meter(&mut self, x0: f32, y: f32, x1: f32, pct: f32) {
        // 24 on the standard width; a wider widget gets more, not fatter ones
        let segments = 24f32.max((24.0 * (x1 - x0) / (WIDTH_STD - 2.0 * PAD)).round()) as usize;
        let (h, gap) = (6.0, 2.0);
        let seg_w = (x1 - x0 - gap * (segments - 1) as f32) / segments as f32;
        let lit = (pct.clamp(0.0, 100.0) / 100.0 * segments as f32).round() as usize;
        for i in 0..segments {
            let sx = x0 + i as f32 * (seg_w + gap);
            let color = if i < lit { self.scale.at(i as f32 / (segments - 1) as f32) } else { TRACK };
            self.c.rrect(sx, y, sx + seg_w, y + h, 1.5, color);
        }
    }

    fn histogram(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, values: &[f32]) {
        let pitch = (x1 - x0) / HISTORY_LEN as f32;
        let bar_w = pitch * 0.62;
        let offset = HISTORY_LEN as isize - values.len().min(HISTORY_LEN) as isize;
        let values = &values[values.len().saturating_sub(HISTORY_LEN)..];
        for i in 0..HISTORY_LEN {
            let bx = x0 + i as f32 * pitch;
            self.c.rect(bx, y0, bx + bar_w, y1, HIST_TRACK);
            let j = i as isize - offset;
            if j < 0 {
                continue;
            }
            let v = values[j as usize].clamp(0.0, 100.0);
            let top = y1 - 1.5f32.max((y1 - y0) * v / 100.0);
            let color = self.scale.at(v / 100.0);
            self.c.rect(bx, top, bx + bar_w, y1, color);
        }
    }

    fn temp_strip(&mut self, x0: f32, y: f32, x1: f32, cpu: Option<f32>, gpu: Option<f32>, online: bool) {
        let (lo, hi) = (30.0, 100.0);
        let steps = 70;
        for i in 0..steps {
            let t = i as f32 / steps as f32;
            let c = lerp(CARD, if online { self.scale.at(t) } else { TRACK }, 0.55);
            let sx0 = x0 + (x1 - x0) * i as f32 / steps as f32;
            let sx1 = x0 + (x1 - x0) * (i + 1) as f32 / steps as f32 + 0.4;
            self.c.rect(sx0, y, sx1, y + 4.0, c);
        }
        for deg in (40..100).step_by(10) {
            let tx = x0 + (x1 - x0) * (deg as f32 - lo) / (hi - lo);
            self.c.rect(tx - 0.5, y + 6.0, tx + 0.5, y + 9.0, FAINT);
        }
        for (t, color) in [(gpu, DIM), (cpu, TEXT)] {
            if let Some(t) = t {
                let mx = x0 + (x1 - x0) * (t.clamp(lo, hi) - lo) / (hi - lo);
                self.c.rrect(mx - 2.5, y - 4.0, mx + 2.5, y + 8.0, 2.5, CARD);
                self.c.rrect(mx - 1.5, y - 3.0, mx + 1.5, y + 7.0, 1.5, color);
            }
        }
    }

    fn battery_icon(&mut self, x: f32, y: f32, pct: f32, plugged: bool) {
        // x, y = left edge, vertical centre; 18 x 9
        self.c.rrect(x, y - 4.5, x + 17.0, y + 4.5, 2.0, DIM);
        self.c.rrect(x + 1.2, y - 3.3, x + 15.8, y + 3.3, 1.2, CARD);
        self.c.rrect(x + 17.5, y - 2.0, x + 19.0, y + 2.0, 0.7, DIM);
        let fill_w = 13.2 * pct.clamp(0.0, 100.0) / 100.0;
        if fill_w > 0.5 {
            let color = if plugged { self.scale.at(0.0) } else { self.scale.at(1.0 - pct / 100.0) };
            self.c.rrect(x + 2.2, y - 2.3, x + 2.2 + fill_w, y + 2.3, 0.8, color);
        }
    }

    fn pin_icon(&mut self, cx: f32, cy: f32, color: Rgb) {
        // upright push pin, ~10 x 14
        self.c.rrect(cx - 4.0, cy - 7.0, cx + 4.0, cy - 4.6, 1.2, color);
        self.c.polygon(
            &[(cx - 2.4, cy - 4.8), (cx + 2.4, cy - 4.8), (cx + 3.4, cy + 0.6), (cx - 3.4, cy + 0.6)],
            color,
        );
        self.c.rrect(cx - 5.0, cy + 0.4, cx + 5.0, cy + 2.2, 0.8, color);
        self.c.rect(cx - 0.6, cy + 2.0, cx + 0.6, cy + 7.5, color);
    }

    fn close_icon(&mut self, cx: f32, cy: f32, color: Rgb, a: f32, width: f32) {
        self.c.line(&[(cx - a, cy - a), (cx + a, cy + a)], width, color);
        self.c.line(&[(cx - a, cy + a), (cx + a, cy - a)], width, color);
    }

    /// A ~11 px line drawing with its left edge at x, centred on cy.
    fn icon(&mut self, kind: &str, x: f32, cy: f32, color: Rgb) {
        let lw = 1.15;
        let c = &mut self.c;
        match kind {
            "cpu" => {
                c.rrect_stroke(x + 2.5, cy - 3.5, x + 9.5, cy + 3.5, 1.2, lw, color);
                c.rect(x + 4.6, cy - 1.4, x + 7.4, cy + 1.4, color);
                for k in [-2.0, 0.0, 2.0] {
                    c.line(&[(x + 6.0 + k, cy - 5.5), (x + 6.0 + k, cy - 3.5)], lw, color);
                    c.line(&[(x + 6.0 + k, cy + 3.5), (x + 6.0 + k, cy + 5.5)], lw, color);
                    c.line(&[(x + 0.5, cy + k), (x + 2.5, cy + k)], lw, color);
                    c.line(&[(x + 9.5, cy + k), (x + 11.5, cy + k)], lw, color);
                }
            }
            "cores" => {
                for gx in [0.0, 6.0] {
                    for gy in [-5.0, 1.0] {
                        c.rrect(x + 0.5 + gx, cy + gy, x + 5.0 + gx, cy + gy + 4.5, 0.8, color);
                    }
                }
            }
            "ram" => {
                c.rrect_stroke(x, cy - 4.0, x + 12.0, cy + 2.0, 1.2, lw, color);
                for k in [2.5, 6.0, 9.5] {
                    c.line(&[(x + k, cy + 2.0), (x + k, cy + 4.5)], lw, color);
                }
                c.rect(x + 2.5, cy - 2.0, x + 9.5, cy, color);
            }
            "gpu" => {
                c.rrect_stroke(x, cy - 4.0, x + 12.0, cy + 3.5, 1.2, lw, color);
                c.ellipse_stroke(x + 4.2, cy - 2.9, x + 9.8, cy + 2.7, lw, color);
                c.line(&[(x + 2.0, cy + 3.5), (x + 2.0, cy + 5.5)], lw, color);
            }
            "disk" => {
                c.rrect_stroke(x, cy - 4.0, x + 12.0, cy + 4.0, 2.0, lw, color);
                c.line(&[(x + 2.5, cy + 1.2), (x + 7.0, cy + 1.2)], lw, color);
                c.ellipse(x + 8.5, cy + 0.2, x + 10.3, cy + 2.0, color);
            }
            "temp" => {
                c.rrect_stroke(x + 4.3, cy - 5.5, x + 7.7, cy + 1.5, 1.7, lw, color);
                c.ellipse(x + 3.0, cy + 0.5, x + 9.0, cy + 6.0, color);
            }
            "battery" => {
                c.rrect_stroke(x, cy - 3.5, x + 10.5, cy + 3.5, 1.2, lw, color);
                c.rect(x + 10.8, cy - 1.5, x + 12.0, cy + 1.5, color);
                c.rect(x + 2.0, cy - 1.5, x + 6.5, cy + 1.5, color);
            }
            "fan" => {
                c.ellipse_stroke(x + 0.5, cy - 5.5, x + 11.5, cy + 5.5, lw, color);
                for ang in [90.0f32, 210.0, 330.0] {
                    let a = ang.to_radians();
                    c.line(&[(x + 6.0, cy), (x + 6.0 + 4.0 * a.cos(), cy - 4.0 * a.sin())], lw, color);
                }
            }
            "net" => {
                c.line(&[(x + 3.5, cy + 5.0), (x + 3.5, cy - 4.5)], lw, color);
                c.line(&[(x + 1.0, cy - 2.0), (x + 3.5, cy - 4.5), (x + 6.0, cy - 2.0)], lw, color);
                c.line(&[(x + 8.5, cy - 5.0), (x + 8.5, cy + 4.5)], lw, color);
                c.line(&[(x + 6.0, cy + 2.0), (x + 8.5, cy + 4.5), (x + 11.0, cy + 2.0)], lw, color);
            }
            "ping" => {
                for (i, hgt) in [3.5f32, 6.5, 10.0].iter().enumerate() {
                    let bx = x + 0.5 + i as f32 * 4.2;
                    c.rrect(bx, cy + 5.0 - hgt, bx + 2.8, cy + 5.0, 0.7, color);
                }
            }
            "clock" => {
                c.ellipse_stroke(x + 0.5, cy - 5.5, x + 11.5, cy + 5.5, lw, color);
                c.line(&[(x + 6.0, cy - 3.0), (x + 6.0, cy), (x + 8.5, cy + 1.5)], lw, color);
            }
            _ => {}
        }
    }

    /// A section label with its icon in front of it.
    fn label(&mut self, baseline: f32, text: &str, icon: &str) {
        self.icon(icon, PAD, baseline - 4.4, DIM);
        self.text(PAD + ICON_W, baseline, text, T, LABEL, TEXT);
    }

    // --- header and CPU block ---

    fn header(&mut self, d: &Data, v: &View) {
        let (p, wl) = (PAD, self.w);
        if v.close_armed {
            self.text(p, 28.0, &tr("Нажми ещё раз — закрыть"), SB, VALUE, CLOSE_HOVER);
            self.c.rrect(wl - p - 13.0, 14.0, wl - p + 5.0, 32.0, 5.0, lerp(CARD, CLOSE_HOVER, 0.25));
        } else {
            self.text(p, 28.0, &tr("Система"), SB, TITLE, TEXT);
        }
        let cc = if v.close_hover || v.close_armed { CLOSE_HOVER } else { DIM };
        self.close_icon(wl - p - 4.0, 23.0, cc, 4.2, 1.5);
        self.boxes.close = self.bx(wl - p - 14.0, 11.0, wl - p + 6.0, 35.0);
        let pin_x = wl - p - 28.0;
        let mut pin_c = if v.pinned {
            self.scale.at(0.0)
        } else if v.pin_hover {
            TEXT
        } else {
            FAINT
        };
        if v.pinned && v.pin_hover {
            pin_c = lerp(pin_c, TEXT, 0.35);
        }
        self.pin_icon(pin_x, 23.0, pin_c);
        self.boxes.pin = self.bx(pin_x - 10.0, 11.0, pin_x + 9.0, 35.0);
        if let Some((pct, plugged)) = d.battery
            && !v.close_armed
        {
            let bx = wl - p - 50.0 - 19.0;
            self.battery_icon(bx, 23.5, pct, plugged);
            let mut parts = Vec::new();
            if let Some(left) = d.battery_left {
                parts.push(part(format!("{}   ", fmt_left(left)), T, SMALL, FAINT));
            }
            parts.push(part(format!("{pct:.0}%"), T, SMALL, DIM));
            if plugged {
                parts.push(part(" ⚡", T, SYM, DIM));
            }
            self.rich(bx - 6.0, 28.0, &parts, Align::Right);
        }
    }

    fn hero(&mut self, d: &Data, grow: f32, compact: bool) {
        let (p, wl) = (PAD, self.w);
        let base = 92.0 + grow;
        let num = format!("{:.0}", d.cpu);
        let num_w = self.width(&num, SBC, HERO);
        let color = self.scale.at(d.cpu / 100.0);
        self.text(p - 1.0, base, &num, SBC, HERO, color);
        self.text(p + num_w + 2.0, base, "%", SBC, HERO_UNIT, DIM);
        let name = if d.cpu_name.is_empty() { tr("Процессор") } else { d.cpu_name.clone() };
        let name = self.ellipsize(&name, T, SMALL, 118.0);
        self.text(p, base + 18.0, &name, T, SMALL, DIM);
        self.histogram(p + 82.0, 50.0, wl - p, 100.0 + grow, &d.cpu_hist);
        let extra = cpu_extra(d.cpu_clock_ghz, d.cpu_power_w);
        if !extra.is_empty() {
            self.rich(wl - p, base + 18.0, &[part(extra, T, SMALL, DIM)], Align::Right);
        }
        if compact {
            self.boxes.hero = [0.0; 4];
            return;
        }
        let chev_x = p + self.width(&format!("{name} "), T, SMALL);
        let chev = if d.procs.is_some() { "▴" } else { "▾" };
        self.text(chev_x, base + 18.0, chev, T, SYM, DIM);
        self.boxes.hero = self.bx(p - 4.0, 42.0, wl - p + 4.0, 118.0 + grow);
    }

    /// Compact mode: readings under the CPU block, each an icon and a value.
    fn compact_rows(&mut self, d: &Data, y: f32, hover: Option<usize>) {
        let (p, wl) = (PAD, self.w);
        let per_row = 6 / Renderer::compact_rows(wl);
        let col = (wl - 2.0 * p) / per_row as f32;
        let disk_pct = d.disks.first().map(|x| x.3);
        let shared = d.gpu_shared && d.temp_gpu.is_none() && d.temp_cpu.is_some();
        let g = if shared { d.temp_cpu } else { d.temp_gpu };
        let pct = |v: Option<f32>| v.map(|v| format!("{v:.0}%"));
        let deg = |v: Option<f32>| v.map(|v| format!("{v:.0}°"));
        let heat = |sc: &Scale, t: Option<f32>| sc.heat(t.unwrap_or(30.0));
        let sc = self.scale.clone();
        let ping = if !d.ping_ready {
            None
        } else {
            Some(match d.ping {
                None => tr("нет"),
                Some(ms) => trf("{v} мс", &[("v", ms.to_string())]),
            })
        };
        // (icon, name, value, colour, approximate, history subject)
        let cells: [Cell; 6] = [
            ("ram", None, pct(Some(d.ram_pct)), sc.at(d.ram_pct / 100.0), false, Some(SUBJECT_RAM)),
            (
                "gpu",
                None,
                pct(d.gpu_load),
                sc.at(d.gpu_load.unwrap_or(0.0) / 100.0),
                false,
                Some(SUBJECT_GPU),
            ),
            ("disk", None, pct(disk_pct), sc.at(disk_pct.unwrap_or(0.0) / 100.0), false, None),
            (
                "temp",
                Some("CPU"),
                deg(d.temp_cpu),
                heat(&sc, d.temp_cpu),
                d.temp_cpu_approx,
                Some(SUBJECT_CPU),
            ),
            ("temp", Some("GPU"), deg(g), heat(&sc, g), shared, Some(SUBJECT_GPU)),
            ("ping", None, ping, sc.ping(d.ping), false, Some(SUBJECT_NET)),
        ];
        for (i, (icon, name, value, color, approx, subject)) in cells.into_iter().enumerate() {
            let (r, c) = (i / per_row, i % per_row);
            let by = y + 16.0 + r as f32 * 22.0;
            let x = p + c as f32 * col;
            if let Some(sub) = subject {
                let b = [x - 6.0, by - 14.0, x + col - 8.0, by + 6.0];
                if hover == Some(self.boxes.rows.len()) {
                    self.c.rrect(b[0], b[1], b[2], b[3], 6.0, ROW_HOVER);
                }
                let bx = self.bx(b[0], b[1], b[2], b[3]);
                self.boxes.rows.push((sub, bx));
            }
            self.icon(icon, x, by - 4.4, DIM);
            let mut parts: Vec<Part> = Vec::new();
            if let Some(n) = name {
                parts.push(part(format!("{n} "), T, SMALL, DIM));
            }
            match value {
                None => parts.push(part("—", SB, VALUE, FAINT)),
                Some(v) => {
                    if approx {
                        parts.push(part("≈", T, SYM, color));
                    }
                    parts.push(part(v, SB, VALUE, color));
                }
            }
            self.rich(x + ICON_W, by, &parts, Align::Left);
        }
    }

    fn grip(&mut self, h: f32, state: u8) {
        if state == 0 {
            return;
        }
        let wl = self.w;
        let color = if state == 2 { TEXT } else { DIM };
        let (cx, cy) = (wl - 6.0, h - 6.0);
        for k in [3.5, 7.0, 10.5] {
            self.c.line(&[(cx - k, cy), (cx, cy - k)], 1.3, color);
        }
        self.boxes.grip = self.bx(wl - GRIP, h - GRIP, wl, h);
    }

    /// A thin light line along the edge the mouse is on (or that is dragged).
    fn edge_line(&mut self, h: f32, edge: Option<Edge>) {
        let (wl, r) = (self.w, RADIUS);
        match edge {
            Some(Edge::Right) => self.c.rrect(wl - 3.0, r, wl - 1.5, h - r, 0.75, DIM),
            Some(Edge::Bottom) => self.c.rrect(r, h - 3.0, wl - r, h - 1.5, 0.75, DIM),
            None => {}
        }
    }

    // --- rows ---

    fn row(&mut self, d: &Data, r: &Row, y: f32) {
        match &r.kind {
            Kind::Update => self.update(d, y),
            Kind::Procs => self.procs(d, y),
            Kind::Cores => self.cores(d, y),
            Kind::Ram => self.ram(d, y),
            Kind::Gpu => self.gpu(d, y),
            Kind::Disk(i) => self.disk(d, *i, y),
            Kind::Temps => self.temps(d, y),
            Kind::Battery => self.battery(d, y),
            Kind::Ssd(i, show_temp) => self.ssd(d, *i, *show_temp, y),
            Kind::Fans => self.fans(d, y),
            Kind::Divider => {
                let s = self.s();
                self.c.rect(PAD, y + 6.0, self.w - PAD, y + 6.0 + 1.0 / s, TRACK);
            }
            Kind::Net => self.net(d, y),
            Kind::NetGraph(h) => self.net_graph(d, y, *h),
            Kind::Ping => self.ping(d, y),
            Kind::DiskIo => self.disk_io(d, y),
            Kind::Footer => {
                self.icon("clock", PAD, y + 14.0, FAINT);
                let t = trf("Без перезагрузки {t}", &[("t", fmt_uptime(d.uptime_s))]);
                self.text(PAD + ICON_W, y + 18.0, &t, T, SMALL, FAINT);
            }
            Kind::Space => {}
        }
    }

    fn update(&mut self, d: &Data, y: f32) {
        let (p, wl) = (PAD, self.w);
        let accent = self.scale.at(0.0);
        self.c.rrect(p - 6.0, y - 2.0, wl - p + 6.0, y + 22.0, 8.0, lerp(CARD, accent, 0.18));
        let right = match &d.update_state {
            Some(UpdateState::Download(f)) => {
                let done = p - 6.0 + (wl - 2.0 * p + 12.0) * f.max(0.04);
                self.c.rrect(p - 6.0, y - 2.0, done, y + 22.0, 8.0, lerp(CARD, accent, 0.4));
                vec![part(trf("скачиваю {p}%", &[("p", format!("{:.0}", f * 100.0))]), T, SMALL, TEXT)]
            }
            Some(UpdateState::Install) => vec![part(tr("устанавливаю…"), T, SMALL, TEXT)],
            None if d.update_auto => {
                vec![part(tr("обновить") + " ", T, SMALL, accent), part("↻", T, SYM, accent)]
            }
            None => vec![part(tr("скачать") + " ", T, SMALL, accent), part("↗", T, SYM, accent)],
        };
        let v = d.update.clone().unwrap_or_default();
        self.text(p, y + 14.0, &trf("Вышла версия {v}", &[("v", v)]), SB, VALUE, TEXT);
        self.rich(wl - p, y + 14.0, &right, Align::Right);
        self.boxes.update = self.bx(p - 6.0, y - 2.0, wl - p + 6.0, y + 22.0);
    }

    fn procs(&mut self, d: &Data, y: f32) {
        let (p, wl) = (PAD, self.w);
        let procs = d.procs.clone().unwrap_or_default();
        let (y0, y1) = (y - 4.0, y + 108.0);
        self.c.rrect(p - 6.0, y0, wl - p + 6.0, y1, 8.0, PROCS_BG);
        let col_x = wl - p - 3.0; // centre of the × that closes a program
        let (col_mem, col_cpu) = (wl - p - 14.0, wl - p - 68.0);
        self.text(p, y0 + 17.0, &tr("Программа"), T, SMALL, FAINT);
        self.rich(col_cpu, y0 + 17.0, &[part("CPU", T, SMALL, FAINT)], Align::Right);
        self.rich(col_mem, y0 + 17.0, &[part(tr("Память"), T, SMALL, FAINT)], Align::Right);
        let name_w = col_cpu - 42.0 - p;
        let hot = self.scale.at(1.0);
        for (i, (name, cpu, mem)) in procs.iter().enumerate() {
            let by = y0 + 36.0 + i as f32 * 17.0;
            let armed = d.procs_armed.as_deref() == Some(name.as_str());
            if armed {
                let t = self.ellipsize(
                    &trf("Закрыть {name}?", &[("name", name.clone())]),
                    T,
                    SMALL,
                    col_mem - 8.0 - p,
                );
                self.text(p, by, &t, T, SMALL, hot);
            } else {
                let t = self.ellipsize(name, T, SMALL, name_w);
                self.text(p, by, &t, T, SMALL, TEXT);
                let cpu_t = if *cpu < 10.0 { dec(format!("{cpu:.1}")) } else { format!("{cpu:.0}") };
                let c = self.scale.at((cpu / 50.0).min(1.0));
                self.rich(col_cpu, by, &[part(format!("{cpu_t}%"), SB, VALUE, c)], Align::Right);
                self.rich(col_mem, by, &[part(fmt_mem(*mem), T, SMALL, DIM)], Align::Right);
            }
            let closable = !d.keep_running.iter().any(|k| k.eq_ignore_ascii_case(name));
            if closable {
                self.close_icon(col_x, by - 4.0, if armed { hot } else { FAINT }, 3.0, 1.3);
                let b = self.bx(col_x - 8.0, by - 12.0, col_x + 8.0, by + 4.0);
                self.boxes.procs.push((name.clone(), b));
            }
        }
        if procs.is_empty() {
            self.text(p, y0 + 36.0, &tr("собираю данные…"), T, SMALL, FAINT);
        }
    }

    /// One bar per logical processor: shows a single overloaded core the
    /// average hides.
    fn cores(&mut self, d: &Data, y: f32) {
        let (p, wl) = (PAD, self.w);
        let top = d.cores.iter().copied().fold(0.0, f32::max);
        self.label(y + 14.0, &tr("Ядра"), "cores");
        let c = self.scale.at(top / 100.0);
        self.rich(
            wl - p,
            y + 14.0,
            &[part(tr("макс") + " ", T, SMALL, DIM), part(format!("{top:.0}%"), SB, VALUE, c)],
            Align::Right,
        );
        let n = d.cores.len();
        let gap = if n <= 16 { 2.0 } else { 1.0 };
        let bar_w = (wl - 2.0 * p - gap * (n as f32 - 1.0)) / n as f32;
        let (y0, y1) = (y + 20.0, y + 32.0);
        for (i, load) in d.cores.iter().enumerate() {
            let bx = p + i as f32 * (bar_w + gap);
            self.c.rrect(bx, y0, bx + bar_w, y1, 1.2, TRACK);
            let h = (y1 - y0) * load.clamp(0.0, 100.0) / 100.0;
            if h >= 0.6 {
                let c = self.scale.at(load / 100.0);
                self.c.rrect(bx, y1 - h, bx + bar_w, y1, 1.2, c);
            }
        }
    }

    fn ram(&mut self, d: &Data, y: f32) {
        let (p, wl) = (PAD, self.w);
        self.label(y + 14.0, &tr("Память"), "ram");
        let used =
            trf("{used} из {total} ГБ", &[("used", fmt_gb(d.ram_used)), ("total", fmt_gb(d.ram_total))]);
        self.rich(
            wl - p,
            y + 14.0,
            &[part(used + "   ", T, SMALL, DIM), part(format!("{:.0}%", d.ram_pct), SB, VALUE, TEXT)],
            Align::Right,
        );
        self.segment_meter(p, y + 22.0, wl - p, d.ram_pct);
    }

    fn gpu(&mut self, d: &Data, y: f32) {
        let (p, wl) = (PAD, self.w);
        let label = tr("Видеокарта");
        self.label(y + 14.0, &label, "gpu");
        match d.gpu_load {
            Some(load) => {
                let value = format!("{load:.0}%");
                let room = (wl - p)
                    - (p + ICON_W)
                    - self.width(&label, T, LABEL)
                    - self.width(&value, SB, VALUE)
                    - 24.0;
                let mut parts = Vec::new();
                if let Some(n) = &d.gpu_name {
                    parts.push(part(self.ellipsize(n, T, SMALL, room) + "   ", T, SMALL, DIM));
                }
                parts.push(part(value, SB, VALUE, TEXT));
                self.rich(wl - p, y + 14.0, &parts, Align::Right);
            }
            None => self.rich(wl - p, y + 14.0, &[part(tr("нет данных"), T, SMALL, FAINT)], Align::Right),
        }
        self.segment_meter(p, y + 22.0, wl - p, d.gpu_load.unwrap_or(0.0));
    }

    fn disk(&mut self, d: &Data, i: usize, y: f32) {
        let (p, wl) = (PAD, self.w);
        let (name, used, total, pct) = d.disks[i].clone();
        self.label(y + 14.0, &trf("Диск {name}", &[("name", name)]), "disk");
        self.rich(
            wl - p,
            y + 14.0,
            &[
                part(fmt_disk(used, total) + "   ", T, SMALL, DIM),
                part(format!("{pct:.0}%"), SB, VALUE, TEXT),
            ],
            Align::Right,
        );
        self.segment_meter(p, y + 22.0, wl - p, pct);
    }

    fn temps(&mut self, d: &Data, y: f32) {
        let (p, wl) = (PAD, self.w);
        let (cpu, gpu, ssd) = (d.temp_cpu, d.temp_gpu, d.temp_ssd);
        let shared = d.gpu_shared && gpu.is_none() && cpu.is_some();
        let online = temps_online(d);
        self.label(y + 14.0, &tr("Температура"), "temp");
        if !online {
            self.rich(wl - p, y + 14.0, &[part(tr("нет данных"), T, SMALL, FAINT)], Align::Right);
        }
        self.temp_strip(p, y + 26.0, wl - p, cpu, gpu, online);
        let y = y + 40.0;
        if online {
            let sc = self.scale.clone();
            let chip = |name: &str, t: Option<f32>, approx: bool| -> Vec<Part> {
                let mut v = vec![part(format!("{name} "), T, SMALL, DIM)];
                match t {
                    None => v.push(part("—", SB, VALUE, FAINT)),
                    Some(t) => {
                        let c = sc.heat(t);
                        if approx {
                            v.push(part("≈", T, SYM, c));
                        }
                        v.push(part(format!("{t:.0}°"), SB, VALUE, c));
                    }
                }
                v
            };
            let ssd_chip = chip("SSD", ssd, false);
            if cpu.is_none() && gpu.is_none() && d.sensor_hint.is_some() {
                // only the SSD answers (admin rights, but no PawnIO driver): say why
                let room = wl - 2.0 * p - 12.0 - self.c.parts_width(self.f, &ssd_chip);
                let hint = self.ellipsize(d.sensor_hint.as_deref().unwrap_or(""), T, SMALL, room);
                self.text(p, y + 8.0, &hint, T, SMALL, FAINT);
            } else {
                self.rich(p, y + 8.0, &chip("CPU", cpu, d.temp_cpu_approx), Align::Left);
                self.rich(
                    wl / 2.0,
                    y + 8.0,
                    &chip("GPU", if shared { cpu } else { gpu }, shared),
                    Align::Center,
                );
            }
            self.rich(wl - p, y + 8.0, &ssd_chip, Align::Right);
        } else {
            let hint = d.sensor_hint.clone().unwrap_or_else(|| tr("Нет данных о температуре"));
            let hint = self.ellipsize(&hint, T, SMALL, wl - 2.0 * p);
            self.text(p, y + 8.0, &hint, T, SMALL, FAINT);
        }
    }

    /// Wear (capacity lost since new) and, on battery, the power draw.
    fn battery(&mut self, d: &Data, y: f32) {
        let (p, wl) = (PAD, self.w);
        let wear = d.battery_wear.unwrap_or(0.0);
        let on_battery = d.battery.is_some_and(|(_, plugged)| !plugged);
        let info = match d.battery_rate {
            Some(r) if on_battery && r > 0.0 => {
                trf("расход {v} Вт", &[("v", dec(format!("{r:.1}")))]) + "   "
            }
            _ => {
                trf(
                    "{full} из {design} Вт·ч",
                    &[
                        ("full", fmt_wh(d.battery_full.unwrap_or(0.0))),
                        ("design", fmt_wh(d.battery_design.unwrap_or(0.0))),
                    ],
                ) + "   "
            }
        };
        self.label(y + 14.0, &tr("Батарея"), "battery");
        let c = self.scale.at((wear / 40.0).min(1.0));
        self.rich(
            wl - p,
            y + 14.0,
            &[
                part(info, T, SMALL, DIM),
                part(tr("износ") + " ", T, SMALL, DIM),
                part(format!("{wear:.0}%"), SB, VALUE, c),
            ],
            Align::Right,
        );
        self.segment_meter(p, y + 22.0, wl - p, wear);
    }

    /// An NVMe drive: wear, temperature, data written; what doesn't fit drops
    /// off, the wear always stays.
    fn ssd(&mut self, d: &Data, i: usize, show_temp: bool, y: f32) {
        let (p, wl) = (PAD, self.w);
        let drive = d.drives[i].clone();
        let label = std::iter::once("SSD".to_string())
            .chain(drive.letters.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ");
        self.label(y + 14.0, &label, "disk");
        let bad = drive.critical != 0 || drive.spare < drive.spare_min;
        let hot = self.scale.at(1.0);
        let wc = if bad { hot } else { self.scale.at((drive.wear as f32 / 100.0).min(1.0)) };
        let mut wear_parts =
            vec![part(tr("износ") + " ", T, SMALL, DIM), part(format!("{}%", drive.wear), SB, VALUE, wc)];
        if bad {
            wear_parts.insert(0, part(tr("неисправен") + "   ", T, SMALL, hot));
        }
        let temp_parts: Vec<Part> = match drive.temp.filter(|_| show_temp) {
            Some(t) => {
                vec![part(format!("{t:.0}°"), T, SMALL, self.scale.heat(t)), part("   ", T, SMALL, DIM)]
            }
            None => vec![],
        };
        let written: Vec<Part> = match drive.written {
            Some(w) => vec![part(trf("записано {v}", &[("v", fmt_written(w))]) + "   ", T, SMALL, DIM)],
            None => vec![],
        };
        let room = wl - p - p - ICON_W - 6.0 - self.width(&label, T, LABEL);
        let options = [
            [temp_parts.clone(), written.clone(), wear_parts.clone()].concat(),
            [temp_parts.clone(), wear_parts.clone()].concat(),
            wear_parts.clone(),
        ];
        let mut chosen = wear_parts.clone();
        for o in options {
            if self.c.parts_width(self.f, &o) <= room {
                chosen = o;
                break;
            }
        }
        self.rich(wl - p, y + 14.0, &chosen, Align::Right);
        self.segment_meter(p, y + 22.0, wl - p, drive.wear as f32);
    }

    fn fans(&mut self, d: &Data, y: f32) {
        let (p, wl) = (PAD, self.w);
        self.label(y + 14.0, &tr("Вентиляторы"), "fan");
        let mut parts = Vec::new();
        for (i, rpm) in d.fans.iter().enumerate() {
            if i > 0 {
                parts.push(part("  ·  ", T, SMALL, FAINT));
            }
            parts.push(part(format!("{rpm:.0}"), SB, VALUE, self.scale.at((rpm / 5000.0).min(1.0))));
        }
        parts.push(part(" ".to_string() + &tr("об/мин"), T, SMALL, DIM));
        self.rich(wl - p, y + 14.0, &parts, Align::Right);
    }

    fn net(&mut self, d: &Data, y: f32) {
        let (p, wl) = (PAD, self.w);
        let (down_c, up_c) = (self.scale.at(0.0), self.scale.at(0.75));
        self.label(y + 12.0, &tr("Сеть"), "net");
        self.rich(
            wl - p,
            y + 12.0,
            &[
                part("↓ ", T, SYM, down_c),
                part(fmt_speed(d.net_down), T, SMALL, TEXT),
                part("     ↑ ", T, SYM, up_c),
                part(fmt_speed(d.net_up), T, SMALL, TEXT),
            ],
            Align::Right,
        );
    }

    /// The last minute of traffic: download bars up from the line, upload
    /// down; square-root scale so a small upload still shows.
    fn net_graph(&mut self, d: &Data, y: f32, h: f32) {
        let (p, wl) = (PAD, self.w);
        let (down_c, up_c) = (self.scale.at(0.0), self.scale.at(0.75));
        let hist = &d.net_hist[d.net_hist.len().saturating_sub(HISTORY_LEN)..];
        let (top, bottom) = (y + 1.0, y + h - 5.0);
        let mid = top + (bottom - top) * 0.6; // download 3/5 of the height, upload 2/5
        let peak = hist.iter().map(|(a, b)| a.max(*b)).fold(NET_GRAPH_FLOOR, f64::max);
        let pitch = (wl - 2.0 * p) / HISTORY_LEN as f32;
        let bar_w = pitch * 0.62;
        let offset = HISTORY_LEN - hist.len();
        self.c.rect(p, mid - 0.4, wl - p, mid + 0.4, HIST_TRACK);
        for (i, (down, up)) in hist.iter().enumerate() {
            let bx = p + (i + offset) as f32 * pitch;
            let hd = (mid - top) * (down.max(0.0) / peak).sqrt() as f32;
            let hu = (bottom - mid) * (up.max(0.0) / peak).sqrt() as f32;
            if hd >= 0.5 {
                self.c.rect(bx, mid - hd, bx + bar_w, mid - 0.4, down_c);
            }
            if hu >= 0.5 {
                self.c.rect(bx, mid + 0.4, bx + bar_w, mid + hu, up_c);
            }
        }
    }

    fn ping(&mut self, d: &Data, y: f32) {
        let (p, wl) = (PAD, self.w);
        self.label(y + 12.0, &tr("Пинг"), "ping");
        let parts = if !d.ping_ready {
            vec![part("…", T, SMALL, FAINT)]
        } else {
            match d.ping {
                None => vec![part(tr("нет ответа"), SB, VALUE, self.scale.ping(None))],
                Some(ms) => {
                    let host = self.ellipsize(&d.ping_host, T, SMALL, 120.0);
                    vec![
                        part(trf("до {host}", &[("host", host)]) + "   ", T, SMALL, FAINT),
                        part(trf("{v} мс", &[("v", ms.to_string())]), SB, VALUE, self.scale.ping(Some(ms))),
                    ]
                }
            }
        };
        self.rich(wl - p, y + 12.0, &parts, Align::Right);
    }

    fn disk_io(&mut self, d: &Data, y: f32) {
        let (p, wl) = (PAD, self.w);
        self.label(y + 12.0, &tr("Диск"), "disk");
        self.rich(
            wl - p,
            y + 12.0,
            &[
                part(tr("чтение") + " ", T, SMALL, DIM),
                part(fmt_speed(d.io_read), T, SMALL, TEXT),
                part("   ".to_string() + &tr("запись") + " ", T, SMALL, DIM),
                part(fmt_speed(d.io_write), T, SMALL, TEXT),
            ],
            Align::Right,
        );
    }
}

/// Sample data: the same as 1.8's tests (conftest.fake_data), for snapshot
/// tests and previews.
pub fn sample_data() -> Data {
    Data {
        cpu: 37.0,
        cpu_hist: (0..60).map(|i| 30.0 + (i % 25) as f32).collect(),
        cores: vec![5.0, 12.0, 3.0, 97.0, 8.0, 20.0, 1.0, 40.0],
        cpu_name: "Процессор".into(),
        cpu_clock_ghz: Some(3.21),
        cpu_power_w: Some(12.4),
        ram_pct: 64.0,
        ram_used: (14.2 * (1u64 << 30) as f64) as u64,
        ram_total: (22.3 * (1u64 << 30) as f64) as u64,
        disks: vec![("C:".into(), 300 << 30, 476 << 30, 63.0)],
        io_read: 1.2 * 1048576.0,
        io_write: 300.0 * 1024.0,
        net_down: 2.5 * 1048576.0,
        net_up: 120.0 * 1024.0,
        net_hist: (0..60).map(|i| (3e6 * (i % 7) as f64 / 7.0, 4e5 * (i % 5) as f64 / 5.0)).collect(),
        battery: Some((87.0, false)),
        battery_left: Some(2 * 3600 + 15 * 60),
        battery_wear: Some(16.0),
        battery_full: Some(35.3),
        battery_design: Some(42.0),
        battery_rate: Some(8.2),
        uptime_s: 3 * 3600 + 12 * 60,
        temp_cpu: Some(66.0),
        temp_cpu_approx: false,
        temp_gpu: None,
        gpu_shared: true,
        temp_ssd: Some(41.0),
        gpu_load: Some(23.0),
        gpu_name: Some("AMD Radeon Graphics".into()),
        drives: vec![Drive {
            letters: vec!["C:".into()],
            critical: 0,
            temp: Some(32.0),
            spare: 100,
            spare_min: 10,
            wear: 3,
            written: Some(38_900_000_000_000),
            model: "Samsung SSD 980 1TB".into(),
        }],
        fans: vec![1450.0, 820.0],
        sensors_online: true,
        sensor_hint: None,
        ping: Some(23),
        ping_ready: true,
        ping_host: "1.1.1.1".into(),
        procs: None,
        procs_armed: None,
        keep_running: vec![],
        update: None,
        update_state: None,
        update_auto: false,
        // the minute the cards look draws beside each ring
        gpu_hist: (0..60).map(|i| 20.0 + 6.0 * ((i as f32) / 4.0).sin()).collect(),
        ram_hist: (0..60).map(|i| 41.0 + ((i as f32) / 9.0).sin()).collect(),
        io_hist: (0..60).map(|i| 4e5 * (1.0 + ((i as f32) / 3.0).cos())).collect(),
        ..Data::default()
    }
}

/// Logical size of the side tab.
pub const TAB_W: f32 = 14.0;
pub const TAB_H: f32 = 60.0;

/// The side tab (1.8's SideTab): rounded on the side away from `edge_right`'s
/// screen edge, flat on it. Beside the widget its arrow points where the
/// widget goes; docked at the screen edge it points back and shows a CPU meter.
pub fn side_tab(
    scale: f32,
    edge_right: bool,
    docked: bool,
    hover: bool,
    cpu: Option<f32>,
    theme: &Theme,
    light: bool,
) -> Option<Pixmap> {
    // light with the cards look's light theme, like the widget it belongs to
    let ink = if light { LIGHT_INK } else { DARK_INK };
    let (pw, ph) = ((TAB_W * scale).round() as u32, (TAB_H * scale).round() as u32);
    let mut c = Canvas::new(pw, ph, scale)?;
    let r = 7.0;
    // the flat side reaches past the image, so only the other side is rounded
    let (x0, x1) = if edge_right { (0.0, TAB_W + r) } else { (-r, TAB_W) };
    c.rrect(x0, 0.0, x1, TAB_H, r, ink.border);
    let bw = 1.0 / scale * scale.round().max(1.0);
    let fill = if hover { lerp(ink.card, ink.text, 0.08) } else { ink.card };
    c.rrect(x0 + bw, bw, x1 - bw, TAB_H - bw, r - bw, fill);
    let to_right = edge_right != docked;
    let dx = if to_right { 1.8 } else { -1.8 };
    let (cx, cy) = (TAB_W / 2.0, if docked { 13.0 } else { TAB_H / 2.0 });
    c.line(
        &[(cx - dx, cy - 4.0), (cx + dx, cy), (cx - dx, cy + 4.0)],
        1.6,
        if hover { ink.text } else { ink.dim },
    );
    if docked && let Some(cpu) = cpu {
        let (top, bottom) = (24.0, TAB_H - 10.0);
        c.rrect(cx - 2.0, top, cx + 2.0, bottom, 2.0, ink.track);
        let fill = 4.0f32.max((bottom - top) * cpu.clamp(0.0, 100.0) / 100.0);
        let sc = if light { theme.scale_dark().on_light() } else { theme.scale_dark() };
        c.rrect(cx - 2.0, bottom - fill, cx + 2.0, bottom, 2.0, sc.at(cpu / 100.0));
    }
    Some(c.pm)
}

/// A tray icon `size` px square: a reading ("37", "64°") in its scale
/// colour on a dark rounded plate, readable on light and dark taskbars.
pub fn tray_icon(fonts: &mut Fonts, size: u32, text: &str, color: Rgb) -> Option<Pixmap> {
    let s = size as f32;
    let mut c = Canvas::new(size, size, 1.0)?;
    c.rrect(0.0, 0.0, s, s, s * 0.22, CARD);
    // the largest size the text fits at (two digits fill the icon)
    let mut px = (s * 0.86).round();
    while px > 6.0 && c.width(fonts, text, Style::SemiBoldCondensed, px) > s * 0.94 {
        px -= 1.0;
    }
    let w = c.width(fonts, text, Style::SemiBoldCondensed, px);
    // digits are ~0.7 em tall: centre them
    let baseline = (s + px * 0.70) / 2.0;
    c.text(fonts, (s - w) / 2.0, baseline, text, Style::SemiBoldCondensed, px, color);
    Some(c.pm)
}

/// Logical width of an alert card.
pub const CARD_W: f32 = 300.0;

/// An alert card (1.8's Toast): a coloured bar, the title, the text wrapped
/// in lines.
pub fn alert_card(fonts: &mut Fonts, scale: f32, title: &str, body: &str, accent: Rgb) -> Option<Pixmap> {
    alert_card_with(fonts, scale, title, body, accent, &[]).map(|(pm, _)| pm)
}

/// An alert card with buttons along its bottom (wrapping when they don't
/// fit): the card and each button's rectangle, physical px (x0, y0, x1, y1).
pub fn alert_card_with(
    fonts: &mut Fonts,
    scale: f32,
    title: &str,
    body: &str,
    accent: Rgb,
    buttons: &[String],
) -> Option<(Pixmap, Vec<[f32; 4]>)> {
    // wrap the text at the card's inner width
    let probe = Canvas::new(1, 1, scale)?;
    let max = CARD_W - 46.0;
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in body.split_whitespace() {
        let trial = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if !line.is_empty() && probe.width(fonts, &trial, Style::Regular, 12.0) > max {
            lines.push(std::mem::replace(&mut line, word.to_string()));
        } else {
            line = trial;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    // the buttons: left to right under the text, a new row when one doesn't fit
    const BTN_H: f32 = 28.0;
    const BTN_ROW: f32 = 34.0;
    let mut placed: Vec<(f32, usize, f32)> = Vec::new();
    let (mut x, mut row) = (30.0f32, 0usize);
    for b in buttons {
        let w = probe.width(fonts, b, Style::SemiBold, 12.0) + 24.0;
        if x > 30.0 && x + w > CARD_W - 16.0 {
            x = 30.0;
            row += 1;
        }
        placed.push((x, row, w));
        x += w + 8.0;
    }
    let text_h = 46.0 + 17.0 * lines.len() as f32;
    let h = if buttons.is_empty() { text_h } else { text_h + 2.0 + (row + 1) as f32 * BTN_ROW + 8.0 };
    let mut c = Canvas::new((CARD_W * scale).round() as u32, (h * scale).round() as u32, scale)?;
    c.rrect(0.0, 0.0, CARD_W, h, 12.0, BORDER);
    let bw = 1.0 / scale * scale.round().max(1.0);
    c.rrect(bw, bw, CARD_W - bw, h - bw, 12.0 - bw, CARD);
    c.rrect(14.0, 16.0, 18.0, h - 16.0, 2.0, accent);
    c.text(fonts, 30.0, 29.0, title, Style::SemiBold, 14.0, TEXT);
    for (i, l) in lines.iter().enumerate() {
        c.text(fonts, 30.0, 48.0 + 17.0 * i as f32, l, Style::Regular, 12.0, DIM);
    }
    let mut rects = Vec::new();
    for (b, (x, row, w)) in buttons.iter().zip(&placed) {
        let y = text_h + 2.0 + *row as f32 * BTN_ROW;
        c.rrect_stroke(*x, y, x + w, y + BTN_H, 8.0, 1.0, BORDER);
        c.text(fonts, x + 12.0, y + 18.5, b, Style::SemiBold, 12.0, TEXT);
        rects.push([x * scale, y * scale, (x + w) * scale, (y + BTN_H) * scale]);
    }
    Some((c.pm, rects))
}

/// The two-line readout on the taskbar (1.8's TaskbarPanel): CPU/RAM load,
/// network speed, ping and, when there are any, CPU/GPU temperatures.
/// `height` = the taskbar's height (physical px); the background stays
/// transparent (alpha 1, so the mouse still reaches the window).
/// `max_width` (physical px, Windows 11's taskbar buttons leave only so
/// much): columns are left out, ping first, then network and load, the
/// temperatures last; None when not even one fits.
pub fn taskbar_panel(
    fonts: &mut Fonts,
    scale: f32,
    d: &Data,
    theme: &Theme,
    light: bool,
    height: u32,
    max_width: Option<u32>,
) -> Option<Pixmap> {
    let sc = theme.scale();
    let probe = Canvas::new(1, 1, scale)?;
    let w = |fonts: &mut Fonts, t: &str, st: Style| probe.width(fonts, t, st, 12.0);
    let speed_w = w(fonts, &fmt_speed(1023.0 * 1024.0), T).max(w(fonts, &fmt_speed(99.9 * 1048576.0), T));
    let col_a = w(fonts, "CPU ", T).max(w(fonts, "RAM ", T)) + w(fonts, "100%", SB);
    let col_b = w(fonts, "↓ ", T) + speed_w;
    let col_c = w(fonts, "CPU ", T).max(w(fonts, "GPU ", T)) + w(fonts, "≈", T) + w(fonts, "100°", SB);
    let col_d = w(fonts, &tr("пинг"), T).max(w(fonts, &trf("{v} мс", &[("v", "999".into())]), SB));
    let gap = 14.0;
    let temps = d.temp_cpu.is_some() || d.temp_gpu.is_some();
    // shown columns, left to right: load, network, ping, temperatures
    let mut show = [true, true, true, temps];
    let widths = [col_a, col_b, col_d, col_c];
    let total = |show: &[bool; 4]| {
        let cols = widths.iter().zip(show).filter(|(_, s)| **s).map(|(w, _)| *w);
        let n = show.iter().filter(|s| **s).count();
        cols.sum::<f32>() + gap * n.saturating_sub(1) as f32 + 2.0
    };
    if let Some(max) = max_width {
        let max = max as f32 / scale;
        for drop in [2, 1, 0, 3] {
            if total(&show) <= max {
                break;
            }
            if show.iter().filter(|s| **s).count() == 1 {
                return None;
            }
            show[drop] = false;
        }
        if total(&show) > max {
            return None;
        }
    }
    let mut xs = [0.0f32; 4];
    let mut x = 0.0;
    for i in 0..4 {
        if show[i] {
            xs[i] = x;
            x += widths[i] + gap;
        }
    }
    let [x_a, x_b, x_d, x_c] = xs;
    let width = total(&show);
    let hl = height as f32 / scale;
    let mut c = Canvas::new((width * scale).ceil() as u32, height, scale)?;
    // almost transparent: clicks still land on the window
    c.pm.fill(tiny_skia::Color::from_rgba8(0, 0, 0, 1));
    let label = if light { (88, 88, 92) } else { (158, 163, 172) };
    let text = if light { (24, 24, 24) } else { (240, 240, 242) };
    let tone = |t: f32| if light { lerp(sc.at(t), (0, 0, 0), 0.4) } else { sc.at(t) };
    // text baselines for two rows centred on the bar
    let off = 8.0f32.min(hl * 0.24);
    let rows = [hl / 2.0 - off + 4.0, hl / 2.0 + off + 4.0];
    let right = |c: &mut Canvas, fonts: &mut Fonts, x: f32, y: f32, parts: &[Part]| {
        c.rich(fonts, x, y, parts, Align::Right)
    };
    if show[0] {
        for (y, name, pct) in [(rows[0], "CPU", d.cpu), (rows[1], "RAM", d.ram_pct)] {
            c.text(fonts, x_a, y, name, T, 12.0, label);
            right(&mut c, fonts, x_a + col_a, y, &[part(format!("{pct:.0}%"), SB, 12.0, tone(pct / 100.0))]);
        }
    }
    if show[1] {
        for (y, arrow, color, speed) in
            [(rows[0], "↓", tone(0.0), d.net_down), (rows[1], "↑", tone(0.75), d.net_up)]
        {
            c.text(fonts, x_b, y, arrow, T, 12.0, color);
            right(&mut c, fonts, x_b + col_b, y, &[part(fmt_speed(speed), T, 12.0, text)]);
        }
    }
    if show[2] {
        right(&mut c, fonts, x_d + col_d, rows[0], &[part(tr("пинг"), T, 12.0, label)]);
        let (value, color) = if !d.ping_ready {
            ("…".to_string(), label)
        } else {
            match d.ping {
                None => (tr("нет"), tone(1.0)),
                Some(ms) => {
                    let t = if ms < 50 {
                        0.05
                    } else if ms < 100 {
                        0.55
                    } else {
                        0.95
                    };
                    (trf("{v} мс", &[("v", ms.to_string())]), tone(t))
                }
            }
        };
        right(&mut c, fonts, x_d + col_d, rows[1], &[part(value, SB, 12.0, color)]);
    }
    if show[3] {
        let shared = d.gpu_shared && d.temp_gpu.is_none() && d.temp_cpu.is_some();
        let gpu = if shared { d.temp_cpu } else { d.temp_gpu };
        for (y, name, t, approx) in
            [(rows[0], "CPU", d.temp_cpu, d.temp_cpu_approx), (rows[1], "GPU", gpu, shared)]
        {
            c.text(fonts, x_c, y, name, T, 12.0, label);
            let parts = match t {
                None => vec![part("—", SB, 12.0, label)],
                Some(t) => {
                    let col = tone((t - 30.0) / 70.0);
                    let mut v = Vec::new();
                    if approx {
                        v.push(part("≈", T, 12.0, col));
                    }
                    v.push(part(format!("{t:.0}°"), SB, 12.0, col));
                    v
                }
            };
            right(&mut c, fonts, x_c + col_c, y, &parts);
        }
    }
    Some(c.pm)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn heights_like_1_8() {
        let d = sample_data();
        let v = View::default();
        // 1.8 at 100%: 264 x 594 for this data (ref18-full-100.png)
        assert_eq!(Renderer::layout_height(&d, &v), 594.0);
        let compact = View { compact: true, ..View::default() };
        assert_eq!(Renderer::layout_height(&d, &compact), HERO_H + 50.0);
        assert_eq!(Renderer::columns(792.0), 3);
    }

    #[test]
    fn alert_cards_wrap() {
        let mut fonts = Fonts::system();
        let short = alert_card(&mut fonts, 1.0, "Батарея садится", "Осталось 12%.", (200, 100, 0)).unwrap();
        // buttons make it taller and say where they are
        let names = ["Изменить порог".to_string(), "Не показывать час".to_string()];
        let (with, rects) =
            alert_card_with(&mut fonts, 1.5, "Батарея садится", "Осталось 12%.", (200, 100, 0), &names)
                .unwrap();
        assert_eq!(rects.len(), 2);
        assert!(with.height() > short.height() * 3 / 2, "a row of buttons under the text");
        for r in &rects {
            assert!(r[0] >= 0.0 && r[2] <= with.width() as f32 && r[3] <= with.height() as f32, "{r:?}");
        }
        let long_text = "Свободно 9,8 ГБ (2%). Очисти корзину и «Загрузки» или удали ненужные программы.";
        let long = alert_card(&mut fonts, 1.0, "Мало места на диске C:", long_text, (200, 100, 0)).unwrap();
        assert_eq!(short.width(), 300);
        assert_eq!(short.height(), 63, "one line");
        assert!(long.height() >= 80, "the text wraps: {}", long.height());
    }

    #[test]
    fn taskbar_panel_has_the_bar_height() {
        let mut fonts = Fonts::system();
        let d = sample_data();
        let with = taskbar_panel(&mut fonts, 1.0, &d, &Theme::Heat, true, 40, None).unwrap();
        let without = taskbar_panel(
            &mut fonts,
            1.0,
            &Data { temp_cpu: None, ..d.clone() },
            &Theme::Heat,
            false,
            40,
            None,
        )
        .unwrap();
        assert_eq!(with.height(), 40);
        assert!(with.width() > without.width(), "the temperature column only with temperatures");
    }

    #[test]
    fn taskbar_panel_narrows_to_its_room() {
        let mut fonts = Fonts::system();
        let d = Data { temp_cpu: Some(61.0), ..sample_data() };
        let full = taskbar_panel(&mut fonts, 1.0, &d, &Theme::Heat, false, 48, None).unwrap().width();
        // the room it needs: the same width
        let room = taskbar_panel(&mut fonts, 1.0, &d, &Theme::Heat, false, 48, Some(full)).unwrap();
        assert_eq!(room.width(), full);
        // less room: fewer columns, never wider than the room
        let mut last = full;
        for max in [full - 1, full * 3 / 4, full / 2, full / 4] {
            let Some(pm) = taskbar_panel(&mut fonts, 1.0, &d, &Theme::Heat, false, 48, Some(max)) else {
                break;
            };
            assert!(pm.width() <= max && pm.width() <= last, "{max}: {}", pm.width());
            last = pm.width();
        }
        // the temperatures alone are the last ones
        assert!(last < full / 3);
        // not even them: none
        assert!(taskbar_panel(&mut fonts, 1.0, &d, &Theme::Heat, false, 48, Some(10)).is_none());
        // twice the scale needs twice the room
        assert!(
            taskbar_panel(&mut fonts, 2.0, &d, &Theme::Heat, false, 96, Some(full)).unwrap().width()
                < full * 2
        );
    }

    #[test]
    fn tray_icons_fit() {
        let mut fonts = Fonts::system();
        for size in [16, 20, 24, 32] {
            let pm = tray_icon(&mut fonts, size, "100", (255, 0, 0)).unwrap();
            assert_eq!((pm.width(), pm.height()), (size, size));
        }
    }

    #[test]
    fn renders_and_reports_boxes() {
        let mut fonts = Fonts::system();
        let d = sample_data();
        let mut r = Renderer::new(&mut fonts);
        let f = r.render(&d, &View { scale: 1.5, ..View::default() }).unwrap();
        assert_eq!((f.pm.width(), f.pm.height()), (396, 891));
        assert_eq!(f.pm.pixel(0, 0).unwrap().alpha(), 0, "rounded corner");
        assert!(f.boxes.close[2] > f.boxes.close[0] && f.boxes.rows.len() >= 4);
    }
}

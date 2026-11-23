//! The PC Monitor logo: a processor chip with a pulse inside, cool to hot,
//! on a dark rounded tile. From 40 px up it is drawn from one 256-unit
//! design; 16–32 px have their own on the pixel grid (two pins a side,
//! whole-pixel lines), so the taskbar and the title bar get it sharp.
//! `cargo run -p pcm-render --example logo -- assets docs/img` writes
//! assets/app.ico (the exe, the installer) and the PNGs.

use tiny_skia::{
    Color, FillRule, GradientStop, LineCap, LineJoin, LinearGradient, Paint, Path, PathBuilder, Pixmap,
    PixmapMut, Point, Shader, SpreadMode, Stroke, Transform,
};

const TEAL: (u8, u8, u8) = (46, 196, 182);
const YELLOW: (u8, u8, u8) = (244, 185, 63);
const RED: (u8, u8, u8) = (232, 68, 90);
const INK: (u8, u8, u8) = (233, 237, 243);
const TILE_TOP: (u8, u8, u8) = (38, 45, 60);
const TILE_BOTTOM: (u8, u8, u8) = (18, 21, 29);
const EDGE: (u8, u8, u8) = (58, 67, 84);

/// The sizes in the .ico: Windows picks one per DPI and place.
pub const ICO_SIZES: [u32; 9] = [16, 20, 24, 32, 40, 48, 64, 128, 256];

fn c((r, g, b): (u8, u8, u8)) -> Color {
    Color::from_rgba8(r, g, b, 255)
}

fn rrect(x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> Option<Path> {
    let k = 0.5523 * r;
    let mut p = PathBuilder::new();
    p.move_to(x0 + r, y0);
    p.line_to(x1 - r, y0);
    p.cubic_to(x1 - r + k, y0, x1, y0 + r - k, x1, y0 + r);
    p.line_to(x1, y1 - r);
    p.cubic_to(x1, y1 - r + k, x1 - r + k, y1, x1 - r, y1);
    p.line_to(x0 + r, y1);
    p.cubic_to(x0 + r - k, y1, x0, y1 - r + k, x0, y1 - r);
    p.line_to(x0, y0 + r);
    p.cubic_to(x0, y0 + r - k, x0 + r - k, y0, x0 + r, y0);
    p.close();
    p.finish()
}

fn poly(pts: &[(f32, f32)]) -> Option<Path> {
    let mut p = PathBuilder::new();
    p.move_to(pts[0].0, pts[0].1);
    for &(x, y) in &pts[1..] {
        p.line_to(x, y);
    }
    p.finish()
}

fn gradient(a: (f32, f32), b: (f32, f32), stops: Vec<GradientStop>) -> Paint<'static> {
    let mut p = Paint { anti_alias: true, ..Paint::default() };
    p.shader = LinearGradient::new(
        Point::from_xy(a.0, a.1),
        Point::from_xy(b.0, b.1),
        stops,
        SpreadMode::Pad,
        Transform::identity(),
    )
    .unwrap_or(Shader::SolidColor(c(YELLOW)));
    p
}

/// Cool to hot along x.
fn heat(x0: f32, x1: f32) -> Paint<'static> {
    gradient(
        (x0, 0.0),
        (x1, 0.0),
        vec![
            GradientStop::new(0.0, c(TEAL)),
            GradientStop::new(0.5, c(YELLOW)),
            GradientStop::new(1.0, c(RED)),
        ],
    )
}

fn solid(col: (u8, u8, u8)) -> Paint<'static> {
    let mut p = Paint { anti_alias: true, ..Paint::default() };
    p.set_color(c(col));
    p
}

fn line(w: f32, cap: LineCap) -> Stroke {
    Stroke { width: w, line_cap: cap, line_join: LineJoin::Round, ..Stroke::default() }
}

/// The dark tile from `a` to `b` (both axes), corner `r`.
fn tile(pm: &mut PixmapMut, t: Transform, a: f32, b: f32, r: f32, edge: bool) {
    let bg = gradient(
        (0.0, a),
        (0.0, b),
        vec![GradientStop::new(0.0, c(TILE_TOP)), GradientStop::new(1.0, c(TILE_BOTTOM))],
    );
    if let Some(p) = rrect(a, a, b, b, r) {
        pm.fill_path(&p, &bg, FillRule::Winding, t, None);
    }
    if edge && let Some(p) = rrect(a + 1.0, a + 1.0, b - 1.0, b - 1.0, r - 1.0) {
        pm.stroke_path(&p, &solid(EDGE), &Stroke { width: 2.0, ..Stroke::default() }, t, None);
    }
}

/// The 256-unit design (40 px and up).
fn big(pm: &mut PixmapMut, t: Transform) {
    tile(pm, t, 8.0, 248.0, 56.0, true);
    let ink = solid(INK);
    if let Some(p) = rrect(70.0, 70.0, 186.0, 186.0, 20.0) {
        pm.stroke_path(&p, &ink, &line(13.0, LineCap::Round), t, None);
    }
    for i in 0..3 {
        let p = 98.0 + 30.0 * i as f32;
        for seg in [
            [(p, 42.0), (p, 60.0)],
            [(p, 196.0), (p, 214.0)],
            [(42.0, p), (60.0, p)],
            [(196.0, p), (214.0, p)],
        ] {
            if let Some(path) = poly(&seg) {
                pm.stroke_path(&path, &ink, &line(11.0, LineCap::Round), t, None);
            }
        }
    }
    let pulse = [
        (90.0, 132.0),
        (106.0, 132.0),
        (116.0, 108.0),
        (132.0, 156.0),
        (146.0, 100.0),
        (156.0, 132.0),
        (168.0, 132.0),
    ];
    if let Some(p) = poly(&pulse) {
        pm.stroke_path(&p, &heat(90.0, 168.0), &line(11.0, LineCap::Round), t, None);
    }
}

/// 16–32 px on the pixel grid: whole-pixel lines and pins, two pins a side
/// (three from 32 px), a short pulse.
fn small(pm: &mut PixmapMut, s: u32) {
    let sf = s as f32;
    let t = Transform::identity();
    let w = if s >= 24 { 2.0 } else { 1.0 };
    tile(pm, t, 0.0, sf, (sf * 0.22).round(), false);
    let ink = solid(INK);
    // the chip's outer edge on whole pixels, the stroke inside it
    let b0 = (sf * 0.25).round();
    let b1 = sf - b0;
    let h = w / 2.0;
    if let Some(p) = rrect(b0 + h, b0 + h, b1 - h, b1 - h, (sf / 16.0).max(1.0)) {
        pm.stroke_path(&p, &ink, &line(w, LineCap::Butt), t, None);
    }
    // pins: short, as in the big design, on whole pixels
    let pins = if s >= 32 { 3 } else { 2 };
    let from = b0 - (sf * 0.09).round().max(2.0);
    let span = b1 - b0;
    for i in 0..pins {
        let at = (b0 + span * (i + 1) as f32 / (pins + 1) as f32 - h).round() + h;
        for seg in [
            [(at, from), (at, b0)],
            [(at, b1), (at, sf - from)],
            [(from, at), (b0, at)],
            [(b1, at), (sf - from, at)],
        ] {
            if let Some(p) = poly(&seg) {
                pm.stroke_path(&p, &ink, &line(w, LineCap::Butt), t, None);
            }
        }
    }
    // the pulse across the inside, one beat
    let (i0, i1) = (b0 + w, b1 - w);
    let mid = (sf / 2.0 - h).round() + h;
    let amp = ((i1 - i0) * 0.3).round().max(2.0);
    let x = |f: f32| i0 + (i1 - i0) * f;
    let pulse =
        [(i0, mid), (x(0.3), mid), (x(0.45), mid - amp), (x(0.62), mid + amp), (x(0.78), mid), (i1, mid)];
    if let Some(p) = poly(&pulse) {
        let mut st = line(w, LineCap::Butt);
        st.line_join = LineJoin::Miter;
        pm.stroke_path(&p, &heat(i0, i1), &st, t, None);
    }
}

/// The logo, `size` px square (transparent around the tile).
pub fn logo(size: u32) -> Option<Pixmap> {
    let mut pm = Pixmap::new(size, size)?;
    if size <= 32 {
        small(&mut pm.as_mut(), size);
    } else {
        let s = size as f32 / 256.0;
        big(&mut pm.as_mut(), Transform::from_scale(s, s));
    }
    Some(pm)
}

/// A Windows .ico of all `ICO_SIZES`, each a PNG (Vista and later).
pub fn ico() -> Option<Vec<u8>> {
    let pngs: Vec<(u32, Vec<u8>)> =
        ICO_SIZES.iter().map(|&s| Some((s, logo(s)?.encode_png().ok()?))).collect::<Option<_>>()?;
    let n = pngs.len() as u16;
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&n.to_le_bytes());
    let mut offset = 6 + 16 * pngs.len() as u32;
    for (s, png) in &pngs {
        let side = if *s >= 256 { 0 } else { *s as u8 };
        out.extend_from_slice(&[side, side, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits
        out.extend_from_slice(&(png.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for (_, png) in &pngs {
        out.extend_from_slice(png);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_size_draws_the_tile() {
        for s in ICO_SIZES {
            let pm = logo(s).unwrap();
            assert_eq!((pm.width(), pm.height()), (s, s));
            // the middle is covered, the corner of the tile is not
            let mid = pm.pixel(s / 2, s / 2).unwrap();
            assert_eq!(mid.alpha(), 255, "{s}");
            assert!(pm.pixel(0, 0).unwrap().alpha() < 255, "{s}");
        }
    }

    #[test]
    fn ico_lists_its_pngs() {
        let ico = ico().unwrap();
        assert_eq!(&ico[..4], &[0, 0, 1, 0]);
        assert_eq!(u16::from_le_bytes([ico[4], ico[5]]) as usize, ICO_SIZES.len());
        // the first image (16 px) is a PNG where its entry says
        let off = u32::from_le_bytes(ico[18..22].try_into().unwrap()) as usize;
        assert_eq!(ico[6], 16);
        assert_eq!(&ico[off..off + 4], b"\x89PNG");
    }
}

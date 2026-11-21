//! Snapshot tests: the widget drawn from fixed data must look like the
//! reference PNGs in tests/snapshots (1.8's layout, pixel sizes included).
//!
//! Fonts come from the Windows fonts folder, so a PC with another font
//! version draws text a little differently: a frame passes when 98% of its
//! pixels are within 32 levels of the reference. A missing reference is
//! written; `PCM_UPDATE_SNAPSHOTS=1` rewrites them all.

use std::path::PathBuf;

use pcm_render::text::Fonts;
use pcm_render::widget::{Edge, Renderer, View, sample_data};
use tiny_skia::Pixmap;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("snapshots")
}

/// Share of pixels whose channels all differ by at most `tol`.
fn similar(a: &Pixmap, b: &Pixmap, tol: u8) -> f64 {
    let close = a
        .data()
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.data().as_chunks::<4>().0.iter())
        .filter(|(p, q)| p.iter().zip(q.iter()).all(|(x, y)| x.abs_diff(*y) <= tol))
        .count();
    close as f64 / (a.data().len() / 4) as f64
}

fn check(name: &str, view: View) {
    let mut fonts = Fonts::system();
    let mut r = Renderer::new(&mut fonts);
    let frame = r.render(&sample_data(), &view).expect("render");
    let path = dir().join(format!("{name}.png"));
    let update = std::env::var("PCM_UPDATE_SNAPSHOTS").is_ok_and(|v| v == "1");
    if update || !path.exists() {
        std::fs::create_dir_all(dir()).unwrap();
        frame.pm.save_png(&path).unwrap();
        return;
    }
    let reference = Pixmap::load_png(&path).expect("reference png");
    assert_eq!(
        (frame.pm.width(), frame.pm.height()),
        (reference.width(), reference.height()),
        "{name}: the size changed"
    );
    let share = similar(&frame.pm, &reference, 32);
    if share < 0.98 {
        let got = std::env::temp_dir().join(format!("pcm-snapshot-{name}.png"));
        frame.pm.save_png(&got).unwrap();
        panic!("{name}: only {:.1}% of pixels match; this run: {}", share * 100.0, got.display());
    }
}

#[test]
fn widget_full() {
    check("widget-full-100", View::default());
}

#[test]
fn widget_full_150() {
    check("widget-full-150", View { scale: 1.5, ..View::default() });
}

#[test]
fn widget_compact() {
    check("widget-compact-100", View { compact: true, ..View::default() });
}

#[test]
fn widget_three_columns() {
    check("widget-wide-100", View { width: 792.0, ..View::default() });
}

#[test]
fn widget_states() {
    // pinned, hovered row, armed close, the resize corner and edge line
    check(
        "widget-states-100",
        View {
            pinned: true,
            close_armed: true,
            hover_row: Some(1),
            grip: 2,
            edge: Some(Edge::Right),
            stretch: 120.0,
            ..View::default()
        },
    );
}

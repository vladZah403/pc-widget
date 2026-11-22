//! Snapshot tests: the widget drawn from fixed data must look like the
//! reference PNGs in tests/snapshots (1.8's layout, pixel sizes included).
//!
//! Fonts come from the Windows fonts folder, so a PC with another font
//! version draws text a little differently: a frame passes when 98% of its
//! pixels are within 32 levels of the reference. A missing reference is
//! written; `PCM_UPDATE_SNAPSHOTS=1` rewrites them all.

use std::path::PathBuf;

use pcm_render::overlay::{Look, fps_overlay, sample_game, sample_overlay};
use pcm_render::text::Fonts;
use pcm_render::theme::Theme;
use pcm_render::widget::{Edge, Renderer, View, WidgetLook, sample_data};
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
    check_pm(name, &frame.pm);
}

fn check_pm(name: &str, pm: &Pixmap) {
    let path = dir().join(format!("{name}.png"));
    let update = std::env::var("PCM_UPDATE_SNAPSHOTS").is_ok_and(|v| v == "1");
    if update || !path.exists() {
        std::fs::create_dir_all(dir()).unwrap();
        pm.save_png(&path).unwrap();
        return;
    }
    let reference = Pixmap::load_png(&path).expect("reference png");
    assert_eq!(
        (pm.width(), pm.height()),
        (reference.width(), reference.height()),
        "{name}: the size changed"
    );
    let share = similar(pm, &reference, 32);
    if share < 0.98 {
        let got = std::env::temp_dir().join(format!("pcm-snapshot-{name}.png"));
        pm.save_png(&got).unwrap();
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
fn widget_cards_look() {
    let cards = View { look: WidgetLook::Cards, ..View::default() };
    check("widget-cards-100", cards.clone());
    check("widget-cards-light-100", View { light: true, ..cards.clone() });
    check("widget-cards-compact-100", View { compact: true, ..cards });
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

#[test]
fn fps_overlay_panel() {
    let mut fonts = Fonts::system();
    let sc = Theme::Heat.scale();
    // 1.8's pill, as it was
    let pm = fps_overlay(&mut fonts, 1.0, &sc, &sample_overlay(), Look::Classic, 1.0).expect("render");
    check_pm("overlay-100", &pm);
    let pm = fps_overlay(&mut fonts, 1.5, &sc, &sample_overlay(), Look::Classic, 1.0).expect("render");
    check_pm("overlay-150", &pm);
    // the card and the line of 2.0
    let rows: Vec<String> = ["low", "graph", "gpu", "cpu", "vram", "ping"].map(String::from).into();
    let pm = fps_overlay(&mut fonts, 1.0, &sc, &sample_game().keep(&rows), Look::Card, 0.85).expect("render");
    check_pm("overlay-card-100", &pm);
    let pm = fps_overlay(&mut fonts, 1.5, &sc, &sample_game(), Look::Line, 1.0).expect("render");
    check_pm("overlay-line-150", &pm);
}

#[test]
fn session_report_card() {
    use pcm_render::report::{ReportHover, sample_session, session_report};
    let mut fonts = Fonts::system();
    let (pm, _) =
        session_report(&mut fonts, 1.0, &Theme::Heat.scale(), &sample_session(), ReportHover::default())
            .expect("render");
    check_pm("report-100", &pm);
}

#[test]
fn dashboard_cards() {
    use pcm_render::cards::{CARDS, card};
    let mut fonts = Fonts::system();
    let mut d = sample_data();
    (d.vram_used_mb, d.vram_total_mb, d.gpu_clock) = (Some(1300.0), Some(2048.0), Some(1800.0));
    for kind in CARDS {
        let pm = card(&mut fonts, 1.0, kind, &d, &Theme::Heat).expect("render");
        check_pm(&format!("card-{}", format!("{kind:?}").to_lowercase()), &pm);
    }
}

//! The FPS panel's looks and the settings' preview into one PNG:
//! `cargo run -p pcm-render --example overlay -- OUT.png [scale]`

use pcm_render::overlay::{Look, fps_overlay, preview, sample_game};
use pcm_render::text::Fonts;
use pcm_render::theme::Theme;
use tiny_skia::{Pixmap, PixmapPaint, Transform};

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "overlay.png".into());
    let s: f32 = std::env::args().nth(2).and_then(|v| v.parse().ok()).unwrap_or(1.0);
    let mut fonts = Fonts::system();
    let sc = Theme::Heat.scale();
    let info = sample_game();
    let mut pics = Vec::new();
    for look in [Look::Card, Look::Classic, Look::Line] {
        pics.push(fps_overlay(&mut fonts, s, &sc, &info, look, 1.0).unwrap());
    }
    let few = info.clone().keep(&["low".into(), "gpu".into(), "cpu".into()]);
    pics.push(fps_overlay(&mut fonts, s, &sc, &few, Look::Card, 1.0).unwrap());
    pics.push(fps_overlay(&mut fonts, s, &sc, &info.clone().keep(&["low".into()]), Look::Card, 1.0).unwrap());
    pics.push(preview(&mut fonts, s, 100, &sc, &info, Look::Card, 0.85, "tl").unwrap());
    pics.push(preview(&mut fonts, s, 100, &sc, &info, Look::Line, 0.5, "br").unwrap());
    let gap = (16.0 * s) as u32;
    let w = pics.iter().map(|p| p.width()).max().unwrap() + 2 * gap;
    let h = pics.iter().map(|p| p.height() + gap).sum::<u32>() + gap;
    let mut sheet = Pixmap::new(w, h).unwrap();
    sheet.fill(tiny_skia::Color::from_rgba8(24, 27, 34, 255));
    let mut y = gap;
    for p in &pics {
        sheet.draw_pixmap(
            gap as i32,
            y as i32,
            p.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
        y += p.height() + gap;
    }
    sheet.save_png(&out).unwrap();
    println!("{out}");
}

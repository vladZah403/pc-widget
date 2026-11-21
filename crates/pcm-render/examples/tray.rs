//! Tray icons at common sizes into one PNG: `cargo run -p pcm-render --example tray -- OUT.png`

use pcm_render::text::Fonts;
use pcm_render::theme::Theme;
use pcm_render::widget::tray_icon;
use tiny_skia::{Pixmap, PixmapPaint, Transform};

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "tray.png".into());
    let mut fonts = Fonts::system();
    let scale = Theme::Heat.scale();
    let samples = [("7", 0.07), ("37", 0.37), ("64°", 0.6), ("100", 1.0)];
    let sizes = [16u32, 20, 24, 32];
    let mut sheet = Pixmap::new(4 * 40 + 8, 2 * 44 + 8).unwrap();
    // light and dark taskbar backgrounds
    for (row, bg) in [(0, (243u8, 243u8, 243u8)), (1, (32, 32, 32))] {
        let mut p = tiny_skia::Paint::default();
        p.set_color_rgba8(bg.0, bg.1, bg.2, 255);
        let r = tiny_skia::Rect::from_xywh(0.0, row as f32 * 44.0 + 4.0, sheet.width() as f32, 44.0).unwrap();
        sheet.fill_rect(r, &p, Transform::identity(), None);
        for (i, (text, t)) in samples.iter().enumerate() {
            let size = sizes[i];
            let icon = tray_icon(&mut fonts, size, text, scale.at(*t)).unwrap();
            sheet.draw_pixmap(
                4 + i as i32 * 40,
                row * 44 + 10,
                icon.as_ref(),
                &PixmapPaint::default(),
                Transform::identity(),
                None,
            );
        }
    }
    sheet.save_png(&out).unwrap();
}

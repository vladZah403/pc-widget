//! Renders the dashboard cards with the sample data into one PNG:
//! `cargo run -p pcm-render --example cards -- OUT_DIR`

use pcm_render::cards::{CARD_H, CARD_W, CARDS, card};
use pcm_render::text::Fonts;
use pcm_render::theme::Theme;
use pcm_render::widget::sample_data;
use tiny_skia::{Pixmap, PixmapPaint, Transform};

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let mut fonts = Fonts::system();
    let mut d = sample_data();
    d.vram_used_mb = Some(1300.0);
    d.vram_total_mb = Some(2048.0);
    d.gpu_clock = Some(1800.0);
    let scale = 1.0;
    let (cols, gap) = (4u32, 12.0);
    let rows = CARDS.len().div_ceil(cols as usize) as u32;
    let w = (cols as f32 * (CARD_W + gap) + gap) * scale;
    let h = (rows as f32 * (CARD_H + gap) + gap) * scale;
    let mut sheet = Pixmap::new(w as u32, h as u32).unwrap();
    sheet.fill(tiny_skia::Color::from_rgba8(20, 23, 30, 255));
    let t = std::time::Instant::now();
    for (i, kind) in CARDS.iter().enumerate() {
        let pm = card(&mut fonts, scale, *kind, &d, &Theme::Heat).expect("card");
        let (col, row) = (i as u32 % cols, i as u32 / cols);
        let x = (gap + col as f32 * (CARD_W + gap)) * scale;
        let y = (gap + row as f32 * (CARD_H + gap)) * scale;
        sheet.draw_pixmap(
            x as i32,
            y as i32,
            pm.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }
    println!("7 cards in {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
    sheet.save_png(format!("{out}/cards.png")).unwrap();
}

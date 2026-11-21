//! Renders the sample data into PNG files: `cargo run -p pcm-render --example sample -- OUT_DIR`

use pcm_render::text::Fonts;
use pcm_render::widget::{Renderer, View, sample_data};

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let mut fonts = Fonts::system();
    let d = sample_data();
    let views = [
        ("full-100", View::default()),
        ("full-150", View { scale: 1.5, ..View::default() }),
        ("compact-100", View { compact: true, ..View::default() }),
        ("wide-100", View { width: 792.0, ..View::default() }),
    ];
    let mut r = Renderer::new(&mut fonts);
    for (name, v) in views {
        let t = std::time::Instant::now();
        let f = r.render(&d, &v).expect("render");
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let path = format!("{out}/v2-{name}.png");
        f.pm.save_png(&path).expect("png");
        println!("{path}: {}x{} in {ms:.1} ms", f.pm.width(), f.pm.height());
    }
    // a second frame reuses the glyph cache
    let t = std::time::Instant::now();
    r.render(&d, &View::default()).expect("render");
    println!("again: {:.2} ms", t.elapsed().as_secs_f64() * 1000.0);
}

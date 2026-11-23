//! Renders the sample session's report: `cargo run -p pcm-render --example report -- OUT_DIR`
//! (and writes the session itself as OUT_DIR/sample.pcms), or a recorded one's:
//! `... -- OUT_DIR FILE.pcms`.

use pcm_render::report::{ReportHover, sample_session, session_report};
use pcm_render::text::Fonts;
use pcm_render::theme::Theme;

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let mut fonts = Fonts::system();
    let s = match std::env::args().nth(2) {
        Some(file) => {
            pcm_core::session::Session::load(std::path::Path::new(&file)).expect("read the session")
        }
        None => {
            let s = sample_session();
            s.save(std::path::Path::new(&format!("{out}/sample.pcms"))).expect("pcms");
            s
        }
    };
    for (name, scale, hover) in
        [("100", 1.0, ReportHover::default()), ("150-hover", 1.5, ReportHover { close: true, csv: true })]
    {
        let t = std::time::Instant::now();
        let (pm, _) = session_report(&mut fonts, scale, &Theme::Heat.scale(), &s, hover).expect("render");
        let path = format!("{out}/report-{name}.png");
        pm.save_png(&path).expect("png");
        println!("{path}: {}x{} in {:.1} ms", pm.width(), pm.height(), t.elapsed().as_secs_f64() * 1000.0);
    }
}

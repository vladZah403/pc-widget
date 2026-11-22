//! What drawing the widget costs, whole and per section (each hidden in
//! turn): `cargo run --release -p pcm-render --example cost`.

use std::time::Instant;

use pcm_render::text::Fonts;
use pcm_render::widget::{Renderer, View, sample_data};

const SECTIONS: [&str; 13] = [
    "cores",
    "ram",
    "gpu",
    "disks",
    "temps",
    "net",
    "net_graph",
    "ping",
    "disk_io",
    "battery",
    "ssd",
    "fans",
    "uptime",
];

fn time(fonts: &mut Fonts, view: &View) -> f64 {
    let d = sample_data();
    let mut r = Renderer::new(fonts);
    r.render(&d, view);
    let n = 50;
    let t = Instant::now();
    for _ in 0..n {
        r.render(&d, view);
    }
    t.elapsed().as_secs_f64() * 1000.0 / n as f64
}

fn main() {
    let mut fonts = Fonts::system();
    let all = time(&mut fonts, &View::default());
    println!("whole widget: {all:.2} ms");
    for s in SECTIONS {
        let v = View { hidden: vec![s.to_string()], ..View::default() };
        println!("  without {s:<10} {:.2} ms ({:+.2})", time(&mut fonts, &v), time(&mut fonts, &v) - all);
    }
    let none = View { hidden: SECTIONS.iter().map(|s| s.to_string()).collect(), ..View::default() };
    println!("header + hero only: {:.2} ms", time(&mut fonts, &none));
    let compact = View { compact: true, ..View::default() };
    println!("compact: {:.2} ms", time(&mut fonts, &compact));
}

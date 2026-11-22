//! A history chart from made-up points: `cargo run -p pcm-render --example chart -- OUT_DIR`

use pcm_core::history::{Column, Point, buckets};
use pcm_render::chart::{Chart, Series, chart, columns};
use pcm_render::text::Fonts;
use pcm_render::theme::Theme;

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let (from, to) = (0i64, 3600i64);
    // a point every 10 s with a gap (the PC asleep) in the middle
    let points: Vec<Point> = (0..360)
        .filter(|i| !(150..180).contains(i))
        .map(|i| Point {
            t: i * 10,
            cpu: 20.0 + ((i % 60) as f32 / 2.0),
            cpu_t: Some(55.0 + (i % 30) as f32 / 3.0),
            gpu_t: Some(50.0 + (i % 45) as f32 / 4.0),
            ..Point::default()
        })
        .collect();
    let (w, scale) = (1100.0, 1.0);
    let n = columns(w, scale).min(((to - from) / 10) as usize);
    let sc = Theme::Heat.scale();
    let spec = Chart {
        title: "Температура".into(),
        series: vec![
            Series {
                name: "CPU".into(),
                color: sc.at(0.05),
                values: buckets(&points, from, to, n, Column::CpuTemp),
            },
            Series {
                name: "GPU".into(),
                color: sc.at(0.55),
                values: buckets(&points, from, to, n, Column::GpuTemp),
            },
        ],
        lo: 20.0,
        hi: None,
        min_hi: 60.0,
        times: ["13:23".into(), "13:53".into(), "14:23".into()],
        fmt: |v| format!("{v:.0}°"),
    };
    let mut fonts = Fonts::system();
    chart(&mut fonts, scale, w, 170.0, &spec).expect("chart").save_png(format!("{out}/chart.png")).unwrap();
    println!("{n} columns");
}

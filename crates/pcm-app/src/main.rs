//! PC-Monitor.exe, PC Monitor 2.0.
//!
//! Without arguments it shows the desktop widget; `--diag [--out FILE]
//! [--helper PATH] [--ping HOST]` reads every source once and prints it;
//! `--png FILE` draws the widget with live data into a PNG (no window);
//! `--session FILE` prints a game session's summary; `--report FILE` starts
//! the widget with that session's report open; `--dashboard` is the PC
//! Monitor window (the widget starts it and feeds it on stdin).

mod alerts;
mod collect;
mod dashboard;
mod diag;
mod game;
mod layered;
mod report_win;
mod session_rec;
mod taskbar;
mod tray;
mod widget_win;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--dashboard") {
        std::process::exit(dashboard::run());
    }
    if args.iter().any(|a| a == "--diag") {
        std::process::exit(diag::run(&args));
    }
    if let Some(i) = args.iter().position(|a| a == "--png") {
        let Some(path) = args.get(i + 1) else {
            eprintln!("--png needs a file name");
            std::process::exit(2);
        };
        std::process::exit(widget_win::png(path));
    }
    if let Some(i) = args.iter().position(|a| a == "--session") {
        let Some(path) = args.get(i + 1) else {
            eprintln!("--session needs a file name");
            std::process::exit(2);
        };
        std::process::exit(print_session(path));
    }
    let report = args.iter().position(|a| a == "--report").and_then(|i| args.get(i + 1)).cloned();
    std::process::exit(widget_win::run(report));
}

fn print_session(path: &str) -> i32 {
    match pcm_core::session::Session::load(std::path::Path::new(path)) {
        Ok(s) => {
            let mut info = s.info.clone();
            let samples = std::mem::take(&mut info.samples);
            println!("{}", serde_json::to_string_pretty(&info).unwrap_or_default());
            println!("samples: {} (first {:?})", samples.len(), samples.first());
            println!("frames in the file: {}", s.frame_ms.len());
            0
        }
        Err(e) => {
            eprintln!("{path}: {e}");
            1
        }
    }
}

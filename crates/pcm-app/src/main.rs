//! PC-Monitor.exe, PC Monitor 2.0.
//!
//! Without arguments it shows the desktop widget; `--diag [--out FILE]
//! [--helper PATH] [--ping HOST]` reads every source once and prints it;
//! `--png FILE` draws the widget with live data into a PNG (no window).

mod collect;
mod diag;
mod layered;
mod tray;
mod widget_win;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
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
    std::process::exit(widget_win::run());
}

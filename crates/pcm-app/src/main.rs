//! PC-Monitor.exe, PC Monitor 2.0.
//!
//! Stage 1 of the plan: the data sources. `PC-Monitor.exe --diag` reads
//! every source once and prints what it got.

mod diag;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--diag") {
        std::process::exit(diag::run(&args));
    }
    eprintln!("PC Monitor 2.0 is being built: only --diag works so far");
    eprintln!("usage: PC-Monitor.exe --diag [--out FILE] [--helper PATH] [--ping HOST]");
    std::process::exit(2);
}

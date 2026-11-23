//! PC-Monitor.exe, PC Monitor 2.0.
//!
//! Without arguments it opens the PC Monitor window and runs the widget and
//! the tray icons (`--autostart`: without the window); `--diag [--out FILE]
//! [--helper PATH] [--ping HOST]` reads every source once and prints it;
//! `--png FILE` draws the widget with live data into a PNG (no window);
//! `--session FILE` prints a game session's summary; `--report FILE` starts
//! the widget with that session's report open, `--settings` with the
//! settings open (after a restart with admin rights), `--no-elevate` without
//! asking for admin rights; `--dashboard` is the PC Monitor window (the
//! widget starts it and feeds it on stdin).
//!
//! A Windows program (no console window of its own); the commands that print
//! write to the console they were started from.

// tests keep the console: their output goes there
#![cfg_attr(not(test), windows_subsystem = "windows")]

mod alerts;
mod collect;
mod dash_devices;
mod dash_history;
mod dash_home;
mod dash_kit;
mod dash_procs;
mod dash_report;
mod dash_sessions;
mod dash_settings;
mod dashboard;
mod diag;
mod elevation;
mod game;
mod icon;
mod layered;
mod report_win;
mod session_rec;
mod taskbar;
mod tray;
mod updater;
mod widget_win;

/// The interface language from the setting ("auto" = the Windows one).
pub fn apply_language(setting: &str) {
    let code = if setting == "auto" { pcm_win::system::ui_language() } else { setting };
    pcm_render::format::set_language(code);
}

/// The version of this build, "2.0.2".
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Print to the console that started us (cmd, PowerShell), when there is one
/// and our output isn't already going somewhere (a pipe, a file).
fn attach_console() {
    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_OUTPUT_HANDLE,
    };
    unsafe {
        let out = GetStdHandle(STD_OUTPUT_HANDLE);
        if out.is_null() || out == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == elevation::CLEANUP_ARG) {
        std::process::exit(elevation::cleanup()); // the uninstaller asks
    }
    if args.iter().any(|a| a == updater::FINISH_ARG) {
        std::process::exit(updater::finish_update(&args)); // the new version installs itself
    }
    if args.iter().any(|a| ["--diag", "--session", "--png", "--version"].contains(&a.as_str())) {
        attach_console();
    }
    if args.iter().any(|a| a == "--version") {
        println!("PC Monitor {APP_VERSION}");
        std::process::exit(0);
    }
    if args.iter().any(|a| a == "--dashboard") {
        let tab = args.iter().position(|a| a == "--tab").and_then(|i| args.get(i + 1)?.parse().ok());
        let offscreen = args.iter().any(|a| a == "--offscreen");
        let shot = args.iter().position(|a| a == "--screenshot").and_then(|i| args.get(i + 1)).cloned();
        let scroll = args.iter().position(|a| a == "--scroll").and_then(|i| args.get(i + 1)?.parse().ok());
        std::process::exit(dashboard::run(tab.unwrap_or(0), offscreen, shot, scroll));
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
    // by hand: the PC Monitor window opens; with Windows (or with a report): quietly
    let open = if args.iter().any(|a| a == "--settings") {
        Some(dashboard::SETTINGS_TAB)
    } else if report.is_some() || args.iter().any(|a| a == elevation::AUTOSTART_ARG) {
        None
    } else {
        Some(0)
    };
    std::process::exit(widget_win::run(report, open));
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

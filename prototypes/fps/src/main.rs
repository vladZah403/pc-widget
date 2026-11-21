//! PC Monitor 2.0, stage 0: console FPS counter on ETW.
//!
//! pcm-proto-fps [--seconds N] [--all] [--json]
//!   --seconds N  stop after N seconds (default: until Ctrl+C)
//!   --all        one line per drawing process instead of a one-line summary
//!   --json       one JSON line per second (for comparing with 1.8)

mod etw;
mod process;
mod stats;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use stats::{FrameStats, Source};
use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;

static QUIT: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn on_ctrl(_event: u32) -> i32 {
    QUIT.store(true, Ordering::SeqCst);
    1
}

struct Args {
    seconds: Option<f64>,
    all: bool,
    json: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args { seconds: None, all: false, json: false };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--seconds" => {
                let v = it.next().ok_or("--seconds needs a number")?;
                args.seconds = Some(v.parse().map_err(|_| format!("bad number: {v}"))?);
            }
            "--all" => args.all = true,
            "--json" => args.json = true,
            _ => return Err(format!("unknown argument: {a}")),
        }
    }
    Ok(args)
}

struct Row {
    pid: u32,
    st: FrameStats,
    src: Source,
}

fn json_str(s: &str) -> String {
    let mut out = String::from('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn fmt_low(st: &FrameStats) -> String {
    st.low1.map_or("-".into(), |l| format!("{l:.1}"))
}

fn main() {
    let args = parse_args().unwrap_or_else(|e| {
        eprintln!("{e}\nusage: pcm-proto-fps [--seconds N] [--all] [--json]");
        std::process::exit(2);
    });
    unsafe { SetConsoleCtrlHandler(Some(on_ctrl), 1) };

    let session = etw::start().unwrap_or_else(|e| {
        eprintln!("FPS: {e}");
        std::process::exit(1);
    });
    if !session.kernel_ok {
        eprintln!("note: the kernel graphics provider was refused, only DirectX 10-12 is counted");
    }
    let worker = std::thread::spawn(move || session.process());
    eprintln!("counting frames (session {}), Ctrl+C to stop", etw::SESSION);

    let started = Instant::now();
    let mut names: HashMap<u32, String> = HashMap::new();
    let mut name_of = |pid: u32| -> String {
        names.entry(pid).or_insert_with(|| process::name(pid).unwrap_or_else(|| format!("pid {pid}"))).clone()
    };
    let mut tick = 0u64;
    'run: loop {
        tick += 1;
        while started.elapsed() < Duration::from_secs(tick) {
            if QUIT.load(Ordering::SeqCst) {
                break 'run;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let now = etw::now_s();
        let mut rows: Vec<Row> = {
            let frames = etw::FRAMES.lock().unwrap();
            frames
                .pids()
                .into_iter()
                .filter_map(|pid| frames.stats(pid, now).map(|(st, src)| Row { pid, st, src }))
                .collect()
        };
        rows.sort_by(|a, b| b.st.fps.total_cmp(&a.st.fps));
        let fg = process::foreground_pid();
        let secs = started.elapsed().as_secs_f64();

        if args.json {
            let procs: Vec<String> = rows
                .iter()
                .map(|r| {
                    format!(
                        "{{\"pid\":{},\"name\":{},\"fps\":{:.3},\"low1\":{},\"src\":\"{}\"}}",
                        r.pid,
                        json_str(&name_of(r.pid)),
                        r.st.fps,
                        r.st.low1.map_or("null".into(), |l| format!("{l:.3}")),
                        r.src.name()
                    )
                })
                .collect();
            println!("{{\"t\":{secs:.1},\"fg\":{},\"procs\":[{}]}}", fg.unwrap_or(0), procs.join(","));
        } else if args.all {
            println!("--- {secs:>4.0}s");
            for r in &rows {
                let mark = if Some(r.pid) == fg { '>' } else { ' ' };
                println!(
                    "{mark} {:<28} {:>7.1} fps  1% low {:>6}  {}",
                    name_of(r.pid),
                    r.st.fps,
                    fmt_low(&r.st),
                    r.src.name()
                );
            }
        } else {
            let front = match fg.and_then(|p| rows.iter().find(|r| r.pid == p)) {
                Some(r) => format!(
                    "{} {:.1} fps, 1% low {}, {}",
                    name_of(r.pid),
                    r.st.fps,
                    fmt_low(&r.st),
                    r.src.name()
                ),
                None => format!("{} -", fg.map_or("?".into(), &mut name_of)),
            };
            let others: Vec<String> = rows
                .iter()
                .filter(|r| Some(r.pid) != fg)
                .take(3)
                .map(|r| format!("{} {:.0}", name_of(r.pid), r.st.fps))
                .collect();
            println!("{secs:>4.0}s  front: {front}  | {}", others.join(", "));
        }
        if args.seconds.is_some_and(|s| secs >= s) {
            break;
        }
    }

    etw::stop_session();
    let rc = worker.join().unwrap_or(u32::MAX);
    if rc != 0 {
        eprintln!("ProcessTrace ended with {rc}");
    }

    // whole-run summary: the source with more frames per process
    let totals = etw::FRAMES.lock().unwrap().totals.clone();
    let mut best: HashMap<u32, (stats::Total, Source)> = HashMap::new();
    for (&(pid, src), &t) in &totals {
        if best.get(&pid).is_none_or(|(b, _)| t.frames > b.frames) {
            best.insert(pid, (t, src));
        }
    }
    let mut list: Vec<_> = best.into_iter().filter(|(_, (t, _))| t.frames >= 30).collect();
    list.sort_by_key(|(_, (t, _))| std::cmp::Reverse(t.frames));
    if !args.json {
        println!("\nsummary ({:.0} s):", started.elapsed().as_secs_f64());
        for (pid, (t, src)) in list {
            println!(
                "  {:<28} {:>8} frames  {:>6.1} s  avg {:>7.2} fps  {}",
                name_of(pid),
                t.frames,
                t.last - t.first,
                t.avg_fps().unwrap_or(0.0),
                src.name()
            );
        }
    }
}

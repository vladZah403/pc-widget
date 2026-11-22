//! `--diag`: one reading of every source, as text — for checking a PC by
//! hand, for bug reports and for CI.
//!
//! --out FILE     also write the report to FILE
//! --helper PATH  the sensor helper to use (default: next to the exe)
//! --ping HOST    ping this host (default 1.1.1.1)

use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use pcm_core::model::{Snapshot, Temp};
use pcm_core::processes::Tracker;
use pcm_core::{merge, sensors};
use pcm_win::*;

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn gb(b: u64) -> String {
    format!("{:.1} GB", b as f64 / (1u64 << 30) as f64)
}

fn speed(bps: f64) -> String {
    if bps >= 1024.0 * 1024.0 {
        format!("{:.1} MB/s", bps / 1048576.0)
    } else {
        format!("{:.0} KB/s", bps / 1024.0)
    }
}

fn temp(t: Option<Temp>) -> String {
    t.map_or("-".into(), |t| format!("{}{:.1} °C", if t.approx { "≈" } else { "" }, t.celsius))
}

fn opt<T: std::fmt::Display>(v: Option<T>, unit: &str) -> String {
    v.map_or("-".into(), |v| format!("{v}{unit}"))
}

pub fn run(args: &[String]) -> i32 {
    let started = Instant::now();
    let mut s = Snapshot::default();
    let mut out = String::new();
    let mut notes: Vec<String> = Vec::new();

    // rates need two samples: open everything, wait, read
    let mut cpu = cpu::CpuReader::new();
    let mut gpu = gpu::GpuReader::new();
    let mut disk_io = disk::DiskIoReader::new();
    let mut net = net::NetReader::default();
    let mut procs = processes::ProcessReader::default();
    let mut tracker = Tracker::default();
    let threads = system::threads();
    net.read();
    tracker.update(&procs.sample(), now_s(), threads);
    let fps = frames::FrameCounter::start();
    std::thread::sleep(Duration::from_millis(1000));

    s.system.windows_build = system::windows_build();
    s.system.admin = system::is_admin();
    s.system.uptime_s = Some(system::uptime_s());
    s.system.pawnio = system::pawnio_installed();
    if let Some(t) = smbios::read() {
        s.system.board = Some(t.board);
        s.system.memory_modules = t.memory;
    }
    s.cpu.name = system::cpu_name();
    s.cpu.cores = system::cores();
    s.cpu.threads = Some(threads);
    if let Some(c) = cpu.as_mut() {
        let l = c.read();
        s.cpu.load = l.load;
        s.cpu.per_core = l.per_core;
        s.cpu.clock_mhz = l.clock_mhz;
    }
    let zones = thermal::ThermalZones::new();
    s.cpu.temp = zones.as_ref().and_then(|z| z.read());
    s.memory = memory::read();
    s.gpus = gpu.read();
    s.volumes = disk::volumes();
    s.disk_io = disk_io.as_mut().and_then(|d| d.read());
    let letters: Vec<String> = s.volumes.iter().map(|v| v.letter.clone()).collect();
    s.drives = nvme::read_all(&letters);
    s.net = net.read();
    let host = arg(args, "--ping").unwrap_or_else(|| "1.1.1.1".into());
    s.ping =
        ping::Pinger::new().map(|p| pcm_core::model::Ping { ms: p.ping(&host, 1000), host: host.clone() });
    s.battery = battery::read();
    s.processes = tracker.update(&procs.sample(), now_s(), threads);
    s.displays = display::displays();

    // LibreHardwareMonitor through the helper
    let helper = arg(args, "--helper").map(PathBuf::from).or_else(sensors_helper::find);
    let helper_line = match &helper {
        None => format!("not found (looked for {} next to the exe)", sensors_helper::EXE),
        Some(path) => match sensors_helper::read_once(path, "cpu,gpu,board,storage", Duration::from_secs(20))
        {
            Err(e) => format!("{}: {e}", path.display()),
            Ok(r) => {
                let sum = sensors::summarize(&r.hardware);
                merge::apply(&mut s, &sum, r.admin);
                for d in &mut s.drives {
                    if d.letters.is_empty()
                        && let Some(n) = d.drive
                    {
                        d.letters = nvme::letters_of(&letters, n);
                    }
                }
                let kinds: Vec<String> =
                    r.hardware.iter().map(|h| format!("{} ({})", h.name, h.kind)).collect();
                if !r.errors.is_empty() {
                    notes.push(format!("helper errors: {}", r.errors.join("; ")));
                }
                format!(
                    "LHM {}, admin {}, read {:.0} ms, {:.0} MB; {}",
                    r.lhm,
                    r.admin,
                    r.read_ms,
                    r.mem_mb,
                    kinds.join(", ")
                )
            }
        },
    };

    // the report
    let w = &mut out;
    let _ = writeln!(w, "PC Monitor 2.0 diag ({})", env!("CARGO_PKG_VERSION"));
    let sy = &s.system;
    let _ = writeln!(w, "windows build: {}", opt(sy.windows_build, ""));
    let _ = writeln!(w, "admin: {}", sy.admin);
    let _ = writeln!(w, "autostart: {}", crate::elevation::describe());
    let _ =
        writeln!(w, "uptime: {} h", sy.uptime_s.map_or("-".into(), |u| format!("{:.1}", u as f64 / 3600.0)));
    let _ = writeln!(w, "PawnIO: {}", if sy.pawnio { "installed" } else { "missing" });
    if let Some(b) = &sy.board {
        let j = |a: &Option<String>, b: &Option<String>| {
            [a.as_deref(), b.as_deref()].into_iter().flatten().collect::<Vec<_>>().join(" ")
        };
        let _ = writeln!(w, "system: {}", j(&b.system_maker, &b.system_product));
        let _ = writeln!(w, "board: {}", j(&b.board_maker, &b.board_product));
        let _ = writeln!(
            w,
            "bios: {} {} ({})",
            opt(b.bios_vendor.clone(), ""),
            opt(b.bios_version.clone(), ""),
            opt(b.bios_date.clone(), "")
        );
    }
    for m in &sy.memory_modules {
        let _ = writeln!(
            w,
            "ram stick: {} {} {}-{} {}",
            m.locator,
            gb(m.size),
            m.kind.as_deref().unwrap_or("?"),
            opt(m.speed_mts, ""),
            m.maker.as_deref().unwrap_or("")
        );
    }
    let c = &s.cpu;
    let _ = writeln!(
        w,
        "cpu: {} ({} cores, {} threads)",
        opt(c.name.clone(), ""),
        opt(c.cores, ""),
        opt(c.threads, "")
    );
    let _ = writeln!(
        w,
        "cpu load: {} | clock {} | temp {} | power {} | voltage {}",
        c.load.map_or("-".into(), |l| format!("{l:.1}%")),
        c.clock_mhz.map_or("-".into(), |m| format!("{m:.0} MHz")),
        temp(c.temp),
        c.power_w.map_or("-".into(), |p| format!("{p:.1} W")),
        c.voltage_v.map_or("-".into(), |v| format!("{v:.3} V"))
    );
    let cores: Vec<String> = c.per_core.iter().map(|v| format!("{v:.0}")).collect();
    let _ = writeln!(w, "cpu cores %: {}", cores.join(" "));
    if let Some(z) = &zones {
        let list: Vec<String> = z.zones().iter().map(|(n, t)| format!("{n} {t:.1}")).collect();
        let _ =
            writeln!(w, "thermal zones: {}", if list.is_empty() { "none".into() } else { list.join(", ") });
    } else {
        let _ = writeln!(w, "thermal zones: no counters");
    }
    if let Some(m) = &s.memory {
        let _ = writeln!(w, "memory: {} of {} ({:.0}%)", gb(m.used), gb(m.total), m.percent());
    }
    for g in &s.gpus {
        let _ = writeln!(
            w,
            "gpu: {}{} | load {} | dedicated {} of {} | shared {} | temp {} | clock {} | fan {}",
            g.name,
            if g.integrated { " (integrated)" } else { "" },
            g.load.map_or("-".into(), |l| format!("{l:.1}%")),
            g.dedicated_used.map_or("-".into(), gb),
            g.dedicated_total.map_or("-".into(), gb),
            g.shared_used.map_or("-".into(), gb),
            temp(g.temp),
            g.clock_mhz.map_or("-".into(), |m| format!("{m:.0} MHz")),
            opt(g.fan_rpm, " rpm")
        );
    }
    for v in &s.volumes {
        let _ = writeln!(
            w,
            "volume: {}{}{} used {} of {}",
            v.letter,
            v.label.as_deref().map_or(String::new(), |l| format!(" \"{l}\"")),
            if v.removable { " (removable)" } else { "" },
            gb(v.used()),
            gb(v.total)
        );
    }
    if let Some(d) = &s.disk_io {
        let act: Vec<String> = d.active.iter().map(|(n, a)| format!("{n} {a:.0}%")).collect();
        let _ = writeln!(
            w,
            "disk io: read {} write {} | active {}",
            speed(d.read_bps),
            speed(d.write_bps),
            act.join(", ")
        );
    }
    for d in &s.drives {
        let _ = writeln!(
            w,
            "ssd health: {} {:?} [{}] wear {}% spare {}/{} temp {} written {} hours {} critical {} ({:?})",
            opt(d.drive, ""),
            d.model,
            d.letters.join(" "),
            d.wear,
            opt(d.spare, "%"),
            opt(d.spare_min, "%"),
            d.temp.map_or("-".into(), |t| format!("{t:.0} °C")),
            d.written.map_or("-".into(), |b| format!("{:.1} TB", b as f64 / 1e12)),
            opt(d.hours, ""),
            d.critical,
            d.source
        );
    }
    let _ = writeln!(w, "drives: {}", nvme::probe());
    match &s.net {
        Some(n) => {
            let _ = writeln!(
                w,
                "net: down {} up {} | {} {}",
                speed(n.down_bps),
                speed(n.up_bps),
                opt(n.adapter.clone(), ""),
                opt(n.ipv4.clone(), "")
            );
        }
        None => {
            let _ = writeln!(w, "net: -");
        }
    }
    if let Some(p) = &s.ping {
        let _ = writeln!(w, "ping {}: {}", p.host, p.ms.map_or("no answer".into(), |m| format!("{m} ms")));
    }
    match &s.battery {
        Some(b) => {
            let _ = writeln!(
                w,
                "battery: {} | ac {} charging {} | left {} | design {} full {} wear {} | rate {}",
                b.percent.map_or("-".into(), |p| format!("{p:.0}%")),
                b.on_ac,
                b.charging,
                b.seconds_left.map_or("-".into(), |s| format!("{}:{:02}", s / 3600, s / 60 % 60)),
                b.design_wh.map_or("-".into(), |v| format!("{v:.1} Wh")),
                b.full_wh.map_or("-".into(), |v| format!("{v:.1} Wh")),
                b.wear().map_or("-".into(), |v| format!("{v:.1}%")),
                b.rate_w.map_or("-".into(), |v| format!("{v:.1} W"))
            );
        }
        None => {
            let _ = writeln!(w, "battery: none");
        }
    }
    let fans: Vec<String> = s.fans.iter().map(|f| format!("{} {:.0}", f.name, f.rpm)).collect();
    let _ = writeln!(w, "fans: {}", if fans.is_empty() { "-".into() } else { fans.join(", ") });
    for p in s.processes.iter().take(5) {
        let _ = writeln!(
            w,
            "process: {} cpu {:.1}% mem {:.0} MB ({} running)",
            p.name,
            p.cpu,
            p.memory as f64 / 1048576.0,
            p.pids.len()
        );
    }
    for d in &s.displays {
        let _ = writeln!(w, "display: {} {}x{} @ {} Hz", d.name, d.width, d.height, d.refresh_hz);
    }
    match &fps {
        Ok(f) => {
            // the first events arrive 2-3 s after the session starts
            while f.frames().map.is_empty() && started.elapsed() < Duration::from_millis(4000) {
                std::thread::sleep(Duration::from_millis(100));
            }
            let n = f.frames().map.len();
            let _ = writeln!(
                w,
                "fps: ETW session ok, kernel provider {}, {} drawing program(s) seen",
                if f.kernel_ok { "ok" } else { "refused" },
                n
            );
        }
        Err(e) => {
            let _ = writeln!(w, "fps: {e}");
        }
    }
    let _ = writeln!(w, "sensor helper: {helper_line}");
    for n in &notes {
        let _ = writeln!(w, "note: {n}");
    }
    let _ = writeln!(w, "diag took {:.1} s", started.elapsed().as_secs_f64());
    drop(fps);

    print!("{out}");
    if let Some(path) = arg(args, "--out")
        && let Err(e) = std::fs::write(&path, &out)
    {
        eprintln!("can't write {path}: {e}");
        return 1;
    }
    0
}

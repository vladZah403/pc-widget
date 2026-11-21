//! The readers run on one background thread, once a second, and leave a
//! fresh `widget::Data` for the window, which they wake with a message.
//! Slow sources have their own pace: ping every 2 s on its own thread,
//! volumes every 5 s, battery every 5 s, SSD health every 60 s.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pcm_core::model::{Snapshot, Temp};
use pcm_core::processes::{KEEP_RUNNING, Tracker};
use pcm_core::{merge, sensors};
use pcm_render::format::{short_cpu_name, short_gpu_name};
use pcm_render::widget::{Data, Drive, HISTORY_LEN};
use pcm_win::*;

/// What the window and the collector share.
pub struct Shared {
    pub data: Mutex<Option<Data>>,
    /// The window to wake (an HWND) with WM_APP_DATA.
    pub hwnd: AtomicIsize,
    /// The process list is open: sample processes.
    pub want_procs: AtomicBool,
    pub stop: AtomicBool,
    pub ping_host: Mutex<String>,
    /// The program's own PIDs (it and its helper), never offered for closing.
    pub pids: Mutex<Vec<u32>>,
}

pub const WM_APP_DATA: u32 = 0x8000 + 2;

impl Shared {
    pub fn new(ping_host: String) -> Arc<Shared> {
        Arc::new(Shared {
            data: Mutex::new(None),
            hwnd: AtomicIsize::new(0),
            want_procs: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            ping_host: Mutex::new(ping_host),
            pids: Mutex::new(vec![std::process::id()]),
        })
    }
}

fn push<T>(q: &mut VecDeque<T>, v: T) {
    q.push_back(v);
    while q.len() > HISTORY_LEN {
        q.pop_front();
    }
}

/// Ping on its own thread: a host that doesn't answer takes a whole second.
fn start_ping(shared: Arc<Shared>, out: Arc<Mutex<(bool, Option<u32>, String)>>) {
    std::thread::Builder::new()
        .name("ping".into())
        .spawn(move || {
            let Some(p) = ping::Pinger::new() else { return };
            while !shared.stop.load(Ordering::Relaxed) {
                let host = shared.ping_host.lock().map(|h| h.clone()).unwrap_or_default();
                let ms = p.ping(&host, 1000);
                if let Ok(mut o) = out.lock() {
                    *o = (true, ms, host);
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        })
        .ok();
}

pub fn start(shared: Arc<Shared>) {
    std::thread::Builder::new().name("collect".into()).spawn(move || run(shared)).ok();
}

fn run(shared: Arc<Shared>) {
    let ping = Arc::new(Mutex::new((false, None, String::new())));
    start_ping(shared.clone(), ping.clone());

    let mut cpu = cpu::CpuReader::new();
    let mut gpu = gpu::GpuReader::new();
    let mut disk_io = disk::DiskIoReader::new();
    let mut net = net::NetReader::default();
    let zones = thermal::ThermalZones::new();
    let mut procs = processes::ProcessReader::default();
    let mut tracker = Tracker::default();
    let threads = system::threads();
    let cpu_name = system::cpu_name().map(|n| short_cpu_name(&n)).unwrap_or_default();
    let helper =
        sensors_helper::find().map(|p| sensors_helper::Helper::start(p, "cpu,gpu,board,storage", 1000));

    let mut cpu_hist = VecDeque::new();
    let mut net_hist = VecDeque::new();
    let mut volumes = Vec::new();
    let mut drives = Vec::new();
    let mut battery = None;
    let (mut volumes_t, mut drives_t, mut battery_t) = (None::<Instant>, None::<Instant>, None::<Instant>);
    let mut next = Instant::now();

    while !shared.stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        let due = |t: Option<Instant>, every: u64| {
            t.is_none_or(|t| now.duration_since(t) >= Duration::from_secs(every))
        };
        let mut s = Snapshot::default();
        if let Some(c) = cpu.as_mut() {
            let l = c.read();
            s.cpu.load = l.load;
            s.cpu.per_core = l.per_core;
            s.cpu.clock_mhz = l.clock_mhz;
        }
        s.cpu.temp = zones.as_ref().and_then(|z| z.read());
        s.memory = memory::read();
        s.gpus = gpu.read();
        if due(volumes_t, 5) {
            volumes = disk::volumes();
            volumes_t = Some(now);
        }
        let letters: Vec<String> =
            volumes.iter().map(|v: &pcm_core::model::Volume| v.letter.clone()).collect();
        if due(drives_t, 60) {
            drives = nvme::read_all(&letters);
            drives_t = Some(now);
        }
        s.drives = drives.clone();
        s.disk_io = disk_io.as_mut().and_then(|d| d.read());
        s.net = net.read();
        if due(battery_t, 5) {
            battery = battery::read();
            battery_t = Some(now);
        }
        s.battery = battery.clone();
        let mut online = s.cpu.temp.is_some();
        let mut hint = None;
        if let Some(h) = &helper {
            if let Some(r) = h.latest() {
                merge::apply(&mut s, &sensors::summarize(&r.hardware), r.admin);
                online = true;
                if !r.admin {
                    hint = Some("Температуры включаются в меню (ПКМ)".to_string());
                } else if s.cpu.temp.is_none() && !system::pawnio_installed() {
                    hint = Some("CPU и GPU: нужен PawnIO".to_string());
                }
            }
        } else {
            hint = Some("Нет помощника датчиков рядом с программой".to_string());
        }
        for d in &mut s.drives {
            if d.letters.is_empty()
                && let Some(n) = d.drive
            {
                d.letters = nvme::letters_of(&letters, n);
            }
        }
        let want_procs = shared.want_procs.load(Ordering::Relaxed);
        let procs_list = if want_procs {
            let own = shared.pids.lock().map(|p| p.clone()).unwrap_or_default();
            let list = tracker.update(&procs.sample(), now_s(), threads);
            Some(
                list.into_iter()
                    .filter(|g| !g.pids.iter().all(|p| own.contains(p)))
                    .take(5)
                    .map(|g| (g.name, g.cpu, g.memory))
                    .collect(),
            )
        } else {
            None
        };

        let load = s.cpu.load.unwrap_or(0.0);
        push(&mut cpu_hist, load);
        if let Some(n) = &s.net {
            push(&mut net_hist, (n.down_bps, n.up_bps));
        }
        // the GPU worth showing: the busiest (a game's), else the first
        let g = s.gpus.iter().max_by(|a, b| a.load.unwrap_or(0.0).total_cmp(&b.load.unwrap_or(0.0)));
        let gpu_temp = g.and_then(|g| g.temp);
        let (ping_ready, ping_ms, ping_host) = ping.lock().map(|p| p.clone()).unwrap_or_default();
        let ssd_temp = s.drives.iter().filter_map(|d| d.temp).reduce(f32::max);
        let data = Data {
            cpu: load,
            cpu_hist: cpu_hist.iter().copied().collect(),
            cores: s.cpu.per_core.clone(),
            cpu_name: cpu_name.clone(),
            cpu_clock_ghz: s.cpu.clock_mhz.map(|m| m / 1000.0),
            cpu_power_w: s.cpu.power_w,
            ram_pct: s.memory.map_or(0.0, |m| m.percent()),
            ram_used: s.memory.map_or(0, |m| m.used),
            ram_total: s.memory.map_or(0, |m| m.total),
            disks: volumes
                .iter()
                .map(|v| {
                    (v.letter.clone(), v.used(), v.total, v.used() as f32 / v.total.max(1) as f32 * 100.0)
                })
                .collect(),
            io_read: s.disk_io.as_ref().map_or(0.0, |d| d.read_bps),
            io_write: s.disk_io.as_ref().map_or(0.0, |d| d.write_bps),
            net_down: s.net.as_ref().map_or(0.0, |n| n.down_bps),
            net_up: s.net.as_ref().map_or(0.0, |n| n.up_bps),
            net_hist: net_hist.iter().copied().collect(),
            battery: s.battery.as_ref().and_then(|b| Some((b.percent?, b.on_ac))),
            battery_left: s.battery.as_ref().and_then(|b| b.seconds_left),
            battery_wear: s.battery.as_ref().and_then(|b| b.wear()),
            battery_full: s.battery.as_ref().and_then(|b| b.full_wh),
            battery_design: s.battery.as_ref().and_then(|b| b.design_wh),
            battery_rate: s.battery.as_ref().and_then(|b| b.rate_w),
            uptime_s: system::uptime_s(),
            temp_cpu: s.cpu.temp.map(|t: Temp| t.celsius),
            temp_cpu_approx: s.cpu.temp.is_some_and(|t| t.approx),
            temp_gpu: gpu_temp.filter(|t| !t.approx).map(|t| t.celsius),
            gpu_shared: gpu_temp.is_some_and(|t| t.approx),
            temp_ssd: ssd_temp,
            gpu_load: g.and_then(|g| g.load),
            gpu_name: g.map(|g| short_gpu_name(&g.name)),
            drives: s
                .drives
                .iter()
                .map(|d| Drive {
                    letters: d.letters.clone(),
                    critical: d.critical,
                    temp: d.temp,
                    spare: d.spare.unwrap_or(100),
                    spare_min: d.spare_min.unwrap_or(0),
                    wear: d.wear,
                    written: d.written,
                })
                .collect(),
            fans: s.fans.iter().take(3).map(|f| f.rpm).collect(),
            sensors_online: online || gpu_temp.is_some() || ssd_temp.is_some(),
            sensor_hint: hint,
            ping: ping_ms,
            ping_ready,
            ping_host,
            procs: procs_list,
            procs_armed: None,
            keep_running: KEEP_RUNNING.iter().map(|s| s.to_string()).collect(),
            update: None,
            update_state: None,
            update_auto: false,
        };
        if let Ok(mut d) = shared.data.lock() {
            *d = Some(data);
        }
        let hwnd = shared.hwnd.load(Ordering::Relaxed);
        if hwnd != 0 {
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(hwnd as _, WM_APP_DATA, 0, 0)
            };
        }
        // once a second, on the second (no drift)
        next += Duration::from_secs(1);
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now;
        }
    }
}

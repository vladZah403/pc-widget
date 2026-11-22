//! What each reader costs per call, in CPU cycles of this thread (exact,
//! unlike process times) and wall time: `cargo run --release -p pcm-win
//! --example cost`. For keeping the idle load under 1% of one core.

use std::time::{Duration, Instant};

use pcm_win::*;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn QueryThreadCycleTime(thread: isize, cycles: *mut u64) -> i32;
    fn GetCurrentThread() -> isize;
}

fn cycles() -> u64 {
    let mut c = 0u64;
    unsafe { QueryThreadCycleTime(GetCurrentThread(), &mut c) };
    c
}

fn measure(name: &str, mut f: impl FnMut()) {
    f(); // the first call opens things
    std::thread::sleep(Duration::from_millis(300));
    let (n, mut total, mut wall) = (10u64, 0u64, Duration::ZERO);
    for _ in 0..n {
        let (c0, t0) = (cycles(), Instant::now());
        f();
        total += cycles() - c0;
        wall += t0.elapsed();
        std::thread::sleep(Duration::from_millis(200));
    }
    println!(
        "{name:<14} {:>8.2} Mcycles/call {:>8.2} ms wall",
        total as f64 / n as f64 / 1e6,
        wall.as_secs_f64() * 1e3 / n as f64
    );
}

fn main() {
    let mut cpu = cpu::CpuReader::new();
    measure("cpu", || {
        if let Some(c) = cpu.as_mut() {
            c.read();
        }
    });
    let zones = thermal::ThermalZones::new();
    measure("thermal", || {
        zones.as_ref().and_then(|z| z.read());
    });
    measure("memory", || {
        memory::read();
    });
    let mut gpu = gpu::GpuReader::new();
    measure("gpu", || {
        gpu.read();
    });
    measure("volumes", || {
        disk::volumes();
    });
    let mut io = disk::DiskIoReader::new();
    measure("disk io", || {
        io.as_mut().and_then(|d| d.read());
    });
    let mut net = net::NetReader::default();
    measure("net", || {
        net.read();
    });
    measure("battery", || {
        battery::read();
    });
    measure("uptime", || {
        system::uptime_s();
    });
}

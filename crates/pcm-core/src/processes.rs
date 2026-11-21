//! Per-program CPU and memory from process samples, as in 1.8's
//! ProcessSampler: processes of the same program are summed, CPU is a share
//! of the whole PC between two samples.

use std::collections::HashMap;

use crate::model::ProcessGroup;

/// One process as NtQuerySystemInformation lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub pid: u32,
    /// "chrome.exe"
    pub image: String,
    /// User + kernel time, 100 ns units.
    pub cpu_time: u64,
    /// Private working set, bytes.
    pub private_ws: u64,
    /// Creation time: tells a reused pid from the old process.
    pub created: i64,
}

#[derive(Debug, Default)]
pub struct Tracker {
    prev: HashMap<u32, (i64, u64)>,
    prev_t: Option<f64>,
}

impl Tracker {
    /// All programs, busiest first, for samples taken at `t` (s) on a PC with
    /// `threads` logical processors. The first call has no CPU numbers yet.
    pub fn update(&mut self, samples: &[Sample], t: f64, threads: u32) -> Vec<ProcessGroup> {
        let dt = self.prev_t.map(|p| t - p).filter(|d| *d > 0.0);
        let mut groups: HashMap<String, ProcessGroup> = HashMap::new();
        let mut current = HashMap::with_capacity(samples.len());
        for s in samples.iter().filter(|s| s.pid != 0) {
            current.insert(s.pid, (s.created, s.cpu_time));
            let name = s.image.strip_suffix(".exe").or(s.image.strip_suffix(".EXE")).unwrap_or(&s.image);
            let name = if name.is_empty() { "?" } else { name };
            let cpu = match (dt, self.prev.get(&s.pid)) {
                (Some(dt), Some(&(created, before))) if created == s.created => {
                    s.cpu_time.saturating_sub(before) as f64 / 1e7 / dt / threads.max(1) as f64 * 100.0
                }
                _ => 0.0,
            };
            let g = groups.entry(name.to_string()).or_insert_with(|| ProcessGroup {
                name: name.to_string(),
                cpu: 0.0,
                memory: 0,
                pids: Vec::new(),
            });
            g.cpu += cpu as f32;
            g.memory += s.private_ws;
            g.pids.push(s.pid);
        }
        self.prev = current;
        self.prev_t = Some(t);
        let mut list: Vec<ProcessGroup> = groups.into_values().collect();
        list.sort_by(|a, b| b.cpu.total_cmp(&a.cpu).then(b.memory.cmp(&a.memory)));
        list
    }
}

/// Never offered for closing: Windows itself (ending these logs you out or
/// crashes the PC), the antivirus, and PC Monitor.
pub const KEEP_RUNNING: &[&str] = &[
    "system",
    "registry",
    "smss",
    "csrss",
    "wininit",
    "winlogon",
    "services",
    "lsass",
    "lsaiso",
    "svchost",
    "dwm",
    "explorer",
    "fontdrvhost",
    "memory compression",
    "secure system",
    "spoolsv",
    "audiodg",
    "sihost",
    "ctfmon",
    "taskhostw",
    "conhost",
    "runtimebroker",
    "dllhost",
    "wudfhost",
    "searchindexer",
    "msmpeng",
    "nissrv",
    "securityhealthservice",
    "smartscreen",
    "pc-monitor",
    "pcm-sensors",
    "?",
];

pub fn can_close(name: &str) -> bool {
    !KEEP_RUNNING.contains(&name.to_lowercase().as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(pid: u32, image: &str, cpu_time: u64, mem: u64) -> Sample {
        Sample { pid, image: image.into(), cpu_time, private_ws: mem, created: 1 }
    }

    #[test]
    fn same_program_summed_cpu_as_share_of_pc() {
        let mut t = Tracker::default();
        let first =
            t.update(&[s(0, "", 0, 0), s(10, "chrome.exe", 0, 100), s(11, "chrome.exe", 0, 50)], 1.0, 4);
        assert_eq!(first.len(), 1, "idle (pid 0) is left out");
        assert_eq!((first[0].cpu, first[0].memory), (0.0, 150), "no CPU on the first sample");
        // 1 s later: chrome used 2 s of CPU time on a 4-thread PC = 50%
        let next = t.update(
            &[
                s(10, "chrome.exe", 1_0000000, 100),
                s(11, "chrome.exe", 1_0000000, 50),
                s(12, "game.exe", 0, 900),
            ],
            2.0,
            4,
        );
        assert_eq!(next[0].name, "chrome");
        assert!((next[0].cpu - 50.0).abs() < 0.01, "{}", next[0].cpu);
        assert_eq!(next[0].pids, vec![10, 11]);
        assert_eq!(next[1].name, "game", "then by memory");
    }

    #[test]
    fn reused_pid_is_not_a_spike() {
        let mut t = Tracker::default();
        t.update(&[s(10, "a.exe", 5_0000000, 1)], 1.0, 1);
        let mut new = s(10, "b.exe", 6_0000000, 1);
        new.created = 2;
        assert_eq!(t.update(&[new], 2.0, 1)[0].cpu, 0.0);
    }

    #[test]
    fn system_processes_cant_be_closed() {
        assert!(!can_close("svchost") && !can_close("Explorer") && !can_close("PC-Monitor"));
        assert!(can_close("chrome"));
    }
}

//! CPU load per core and the current clock, as Task Manager shows them:
//! "% Processor Utility" and "% Processor Performance" x "Processor Frequency".

use crate::pdh::{Counter, Query};

pub struct CpuReader {
    query: Query,
    total: Counter,
    cores: Counter,
    performance: Option<Counter>,
    frequency: Option<Counter>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CpuLoad {
    pub load: Option<f32>,
    pub per_core: Vec<f32>,
    pub clock_mhz: Option<f32>,
}

/// "0,11" -> (0, 11); "_Total" and "0,_Total" -> None.
fn core_key(name: &str) -> Option<(u32, u32)> {
    let (group, index) = name.split_once(',')?;
    Some((group.parse().ok()?, index.parse().ok()?))
}

impl CpuReader {
    pub fn new() -> Option<CpuReader> {
        let query = Query::open()?;
        // Utility (Windows 8+) is what Task Manager shows; Time is the older measure
        let (total, cores) = ["% Processor Utility", "% Processor Time"].iter().find_map(|name| {
            let t = query.add(&format!(r"\Processor Information(_Total)\{name}"))?;
            let c = query.add(&format!(r"\Processor Information(*)\{name}"))?;
            Some((t, c))
        })?;
        let performance = query.add(r"\Processor Information(_Total)\% Processor Performance");
        let frequency = query.add(r"\Processor Information(_Total)\Processor Frequency");
        query.collect();
        Some(CpuReader { query, total, cores, performance, frequency })
    }

    /// Load since the last call (or since new()).
    pub fn read(&mut self) -> CpuLoad {
        if !self.query.collect() {
            return CpuLoad::default();
        }
        let mut cores: Vec<((u32, u32), f32)> = self
            .cores
            .values()
            .into_iter()
            .filter_map(|(n, v)| Some((core_key(&n)?, v.clamp(0.0, 100.0) as f32)))
            .collect();
        cores.sort_by_key(|(k, _)| *k);
        let clock = match (self.performance.and_then(|c| c.value()), self.frequency.and_then(|c| c.value())) {
            (Some(perf), Some(base)) if base > 0.0 => Some((perf / 100.0 * base) as f32),
            _ => None,
        };
        CpuLoad {
            load: self.total.value().map(|v| v.clamp(0.0, 100.0) as f32),
            per_core: cores.into_iter().map(|(_, v)| v).collect(),
            clock_mhz: clock,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_instance_names() {
        assert_eq!(core_key("0,11"), Some((0, 11)));
        assert_eq!(core_key("1,0"), Some((1, 0)));
        assert_eq!(core_key("0,_Total"), None);
        assert_eq!(core_key("_Total"), None);
    }

    #[test]
    fn reads_this_pc() {
        let mut r = CpuReader::new().expect("processor counters");
        std::thread::sleep(std::time::Duration::from_millis(300));
        let l = r.read();
        assert!(l.load.is_some_and(|v| (0.0..=100.0).contains(&v)));
        assert_eq!(l.per_core.len() as u32, crate::system::threads());
    }
}

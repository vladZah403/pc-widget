//! GPU load and memory from the "GPU Engine" and "GPU Adapter Memory"
//! performance counters, computed the way Task Manager does: an engine's
//! load is the sum over processes, an adapter's load is its busiest engine.

use std::collections::HashMap;

/// An adapter LUID from "luid_0x00000000_0x0000D1E5" (high, low).
fn luid(s: &str) -> Option<u64> {
    let rest = s.split("luid_").nth(1)?;
    let mut parts = rest.split('_');
    let high = u32::from_str_radix(parts.next()?.trim_start_matches("0x"), 16).ok()?;
    let low = u32::from_str_radix(parts.next()?.trim_start_matches("0x"), 16).ok()?;
    Some((high as u64) << 32 | low as u64)
}

fn field<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let rest = s.split(key).nth(1)?;
    Some(rest.split('_').next().unwrap_or(rest))
}

/// "pid_1234_luid_0x00000000_0x0000D1E5_phys_0_eng_3_engtype_VideoDecode"
/// -> (luid, physical adapter, engine, engine type).
pub fn parse_engine(name: &str) -> Option<(u64, u32, u32, String)> {
    let l = luid(name)?;
    let phys = field(name, "_phys_")?.parse().ok()?;
    let eng = field(name, "_eng_")?.parse().ok()?;
    let kind = name.split("_engtype_").nth(1).unwrap_or("").to_string();
    Some((l, phys, eng, kind))
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AdapterLoad {
    /// 0..100: the busiest engine.
    pub load: f32,
    /// Its type ("3D", "VideoDecode", "Copy"…).
    pub busiest: String,
}

/// Utilization instances -> load per adapter LUID.
pub fn loads(instances: &[(String, f64)]) -> HashMap<u64, AdapterLoad> {
    let mut engines: HashMap<(u64, u32, u32), (f64, String)> = HashMap::new();
    for (name, value) in instances {
        if let Some((l, phys, eng, kind)) = parse_engine(name) {
            let e = engines.entry((l, phys, eng)).or_insert((0.0, kind));
            e.0 += value.max(0.0);
        }
    }
    let mut out: HashMap<u64, AdapterLoad> = HashMap::new();
    for ((l, _, _), (sum, kind)) in engines {
        let load = sum.min(100.0) as f32;
        let a = out.entry(l).or_default();
        if load > a.load || a.busiest.is_empty() {
            *a = AdapterLoad { load, busiest: kind };
        }
    }
    out
}

/// "GPU Adapter Memory" instances ("luid_..._phys_0") -> bytes per LUID.
pub fn memory(instances: &[(String, f64)]) -> HashMap<u64, u64> {
    let mut out = HashMap::new();
    for (name, value) in instances {
        if let Some(l) = luid(name) {
            *out.entry(l).or_insert(0) += value.max(0.0) as u64;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "luid_0x00000000_0x0000D1E5";

    #[test]
    fn engine_names() {
        let n = format!("pid_1234_{A}_phys_0_eng_3_engtype_VideoDecode");
        assert_eq!(parse_engine(&n), Some((0xD1E5, 0, 3, "VideoDecode".into())));
        assert_eq!(luid("luid_0x00000001_0x00000002_phys_0"), Some(1 << 32 | 2));
        assert_eq!(parse_engine("garbage"), None);
    }

    #[test]
    fn busiest_engine_summed_over_processes() {
        let i = |pid: u32, eng: u32, kind: &str, v: f64| {
            (format!("pid_{pid}_{A}_phys_0_eng_{eng}_engtype_{kind}"), v)
        };
        let l = loads(&[
            i(1, 0, "3D", 20.0),
            i(2, 0, "3D", 25.0),
            i(3, 3, "VideoDecode", 30.0),
            i(4, 1, "Copy", 70.0),
            i(5, 1, "Copy", 60.0), // the same engine: 130 -> 100
            ("pid_9_luid_0x00000000_0x00000BAD_phys_0_eng_0_engtype_3D".into(), 5.0),
        ]);
        assert_eq!(l[&0xD1E5], AdapterLoad { load: 100.0, busiest: "Copy".into() });
        assert_eq!(l[&0xBAD].load, 5.0);
        let m = memory(&[(format!("{A}_phys_0"), 1024.0), (format!("{A}_phys_1"), 1024.0)]);
        assert_eq!(m[&0xD1E5], 2048);
    }
}

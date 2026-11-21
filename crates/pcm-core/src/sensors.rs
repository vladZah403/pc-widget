//! The LibreHardwareMonitor helper (pcm-sensors.exe): its JSON lines and the
//! summary of its readings — which sensor is "the CPU temperature" etc., the
//! choices 1.8's `read_lhm` makes.

use serde::Deserialize;

use crate::model::{DriveHealth, Fan, HealthSource, Temp};

/// The protocol version this program speaks.
pub const PROTOCOL: u32 = 1;

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Sensor {
    pub id: String,
    pub name: String,
    /// LibreHardwareMonitor's SensorType: "Temperature", "Load", "Clock",
    /// "Power", "Voltage", "Fan", "Data", "SmallData", "Level"…
    pub kind: String,
    pub value: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Hardware {
    pub id: String,
    pub name: String,
    /// HardwareType: "Cpu", "GpuNvidia", "GpuAmd", "GpuIntel", "Storage",
    /// "Motherboard", "SuperIO", "Battery"…
    pub kind: String,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub sensors: Vec<Sensor>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Message {
    Hello {
        protocol: u32,
        lhm: String,
        admin: bool,
    },
    Sensors {
        seq: u64,
        #[serde(default)]
        read_ms: f64,
        #[serde(default)]
        mem_mb: f64,
        hw: Vec<Hardware>,
    },
    Error {
        message: String,
    },
}

/// One line from the helper; None for anything that isn't a message we know.
pub fn parse_line(line: &str) -> Option<Message> {
    serde_json::from_str(line.trim()).ok()
}

const CPU_PREFS: &[&str] = &["cpu package", "core (tctl/tdie)", "core average", "cpu die", "cpu"];
const GPU_PREFS: &[&str] = &["gpu core", "gpu hot spot", "gpu"];
const POWER_PREFS: &[&str] = &["package", "cpu package"];
const VOLTAGE_PREFS: &[&str] = &["core (svi2 tfn)", "cpu core", "vcore", "core"];
const STORAGE_PREFS: &[&str] = &["composite temperature", "temperature", "drive temperature"];
/// Thresholds, not readings.
const TEMP_SKIP: &[&str] = &["distance", "warning", "critical", "limit"];

/// The first sensor whose name starts with the first matching preference,
/// else the highest value.
fn pick(sensors: &[(String, f64)], prefs: &[&str]) -> Option<f64> {
    for pref in prefs {
        if let Some((_, v)) = sensors.iter().find(|(n, _)| n.starts_with(pref)) {
            return Some(*v);
        }
    }
    sensors.iter().map(|(_, v)| *v).reduce(f64::max)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Cpu,
    Gpu,
    Storage,
    Other,
}

fn hw_part(kind: &str) -> Part {
    match kind {
        "Cpu" => Part::Cpu,
        k if k.starts_with("Gpu") => Part::Gpu,
        "Storage" => Part::Storage,
        _ => Part::Other,
    }
}

/// A temperature belongs to what its sensor id says ("/amdcpu/0/..."),
/// else to its hardware.
fn sensor_part(id: &str, hw: Part) -> Part {
    let id = id.to_lowercase();
    if id.contains("cpu/") {
        Part::Cpu
    } else if id.contains("/gpu") {
        Part::Gpu
    } else if ["/nvme/", "/hdd/", "/ssd/", "/ata/"].iter().any(|k| id.contains(k)) {
        Part::Storage
    } else {
        hw
    }
}

/// Integrated graphics by name (it shares the CPU's memory and die).
pub fn is_integrated(name: &str) -> bool {
    let n = name.to_lowercase();
    n.contains("radeon(tm) graphics")
        || n.contains("radeon graphics")
        || n.contains("vega ")
        || n.contains("uhd")
        || n.contains("iris")
        || n.contains("intel")
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GpuSensors {
    pub name: String,
    pub integrated: bool,
    pub temp: Option<f32>,
    pub load: Option<f32>,
    pub clock_mhz: Option<f32>,
    /// MB, as the library counts.
    pub vram_used_mb: Option<f64>,
    pub vram_total_mb: Option<f64>,
    pub fan_rpm: Option<f32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub cpu_temp: Option<Temp>,
    pub cpu_clock_mhz: Option<f32>,
    pub cpu_power_w: Option<f32>,
    pub cpu_voltage_v: Option<f32>,
    pub gpu: Option<GpuSensors>,
    /// An integrated GPU without its own sensor: it shares the CPU's die and
    /// temperature.
    pub gpu_shared: bool,
    pub ssd_temp: Option<f32>,
    /// Spinning fans of any device, fastest first.
    pub fans: Vec<Fan>,
    /// NVMe health as the library reads it (works behind Intel RST / VMD).
    pub drives: Vec<DriveHealth>,
}

fn by_kind(hw: &Hardware, kind: &str) -> Vec<(String, f64)> {
    hw.sensors.iter().filter(|s| s.kind == kind).map(|s| (s.name.to_lowercase(), s.value)).collect()
}

fn value(list: &[(String, f64)], name: &str) -> Option<f64> {
    list.iter().find(|(n, _)| n == name).map(|(_, v)| *v)
}

/// MB from a Data (GB) or SmallData (MB) sensor of this name.
fn megabytes(hw: &Hardware, name: &str) -> Option<f64> {
    hw.sensors.iter().find(|s| s.name.eq_ignore_ascii_case(name)).and_then(|s| match s.kind.as_str() {
        "SmallData" => Some(s.value),
        "Data" => Some(s.value * 1024.0),
        _ => None,
    })
}

fn drive_number(id: &str) -> Option<u32> {
    let low = id.to_lowercase();
    for key in ["/nvme/", "/hdd/", "/ssd/", "/ata/"] {
        if let Some(rest) = low.split(key).nth(1) {
            return rest.split('/').next()?.parse().ok();
        }
    }
    None
}

fn library_health(hw: &Hardware) -> Option<DriveHealth> {
    let levels = by_kind(hw, "Level");
    let wear = value(&levels, "percentage used")?;
    let temps = by_kind(hw, "Temperature");
    let written_gb = hw.sensors.iter().find(|s| s.name.eq_ignore_ascii_case("data written")).map(|s| s.value);
    Some(DriveHealth {
        drive: drive_number(&hw.id),
        model: hw.name.clone(),
        letters: Vec::new(),
        critical: 0,
        temp: pick(&temps, STORAGE_PREFS).map(|t| t.round() as f32),
        spare: Some(value(&levels, "available spare").unwrap_or(100.0).round() as u8),
        spare_min: Some(value(&levels, "available spare threshold").unwrap_or(0.0).round() as u8),
        wear: wear.round().clamp(0.0, 100.0) as u8,
        // the library's GB are 10^9 bytes, as drive makers count
        written: written_gb.map(|gb| (gb * 1e9).round() as u64),
        hours: None,
        source: HealthSource::Library,
    })
}

pub fn summarize(hw: &[Hardware]) -> Summary {
    let mut s = Summary::default();

    // every plausible temperature, with the part it belongs to
    let mut temps: Vec<(Part, String, f64, usize)> = Vec::new();
    for (i, h) in hw.iter().enumerate() {
        for t in h.sensors.iter().filter(|t| t.kind == "Temperature") {
            let name = t.name.to_lowercase();
            if t.value > 0.0 && t.value < 150.0 && !TEMP_SKIP.iter().any(|w| name.contains(w)) {
                temps.push((sensor_part(&t.id, hw_part(&h.kind)), name, t.value, i));
            }
        }
    }
    let of = |part: Part, hw_index: Option<usize>| -> Vec<(String, f64)> {
        temps
            .iter()
            .filter(|(p, _, _, i)| *p == part && hw_index.is_none_or(|h| h == *i))
            .map(|(_, n, v, _)| (n.clone(), *v))
            .collect()
    };
    s.cpu_temp = pick(&of(Part::Cpu, None), CPU_PREFS).map(|c| Temp::exact(c as f32));

    if let Some(cpu) = hw.iter().find(|h| hw_part(&h.kind) == Part::Cpu) {
        let clocks = by_kind(cpu, "Clock");
        let cores = |effective: bool| -> Vec<f64> {
            clocks
                .iter()
                .filter(|(n, v)| {
                    n.starts_with("core #") && n.contains("effective") == effective && *v > 100.0
                })
                .map(|(_, v)| *v)
                .collect()
        };
        let mut list = cores(false);
        if list.is_empty() {
            list = cores(true);
        }
        if !list.is_empty() {
            s.cpu_clock_mhz = Some((list.iter().sum::<f64>() / list.len() as f64) as f32);
        }
        s.cpu_power_w =
            pick(&by_kind(cpu, "Power"), POWER_PREFS).filter(|p| *p > 0.0 && *p < 1000.0).map(|p| p as f32);
        s.cpu_voltage_v =
            pick(&by_kind(cpu, "Voltage"), VOLTAGE_PREFS).filter(|v| *v > 0.2 && *v < 3.0).map(|v| v as f32);
    }

    // the GPU with its own temperature, else the first one
    let gpus: Vec<usize> = (0..hw.len()).filter(|&i| hw_part(&hw[i].kind) == Part::Gpu).collect();
    let primary =
        gpus.iter().copied().find(|&i| !of(Part::Gpu, Some(i)).is_empty()).or(gpus.first().copied());
    if let Some(i) = primary {
        let g = &hw[i];
        let loads = by_kind(g, "Load");
        let main: Vec<f64> = ["gpu core", "d3d 3d"].iter().filter_map(|k| value(&loads, k)).collect();
        let load = main
            .into_iter()
            .reduce(f64::max)
            .or_else(|| loads.iter().filter(|(n, _)| n.starts_with("gpu")).map(|(_, v)| *v).reduce(f64::max));
        let clock = value(&by_kind(g, "Clock"), "gpu core").filter(|c| *c > 0.0 && *c < 10000.0);
        let mut vram = (None, None);
        for (used, total) in [
            ("gpu memory used", "gpu memory total"),
            ("d3d dedicated memory used", "d3d dedicated memory total"),
        ] {
            if let (Some(u), Some(t)) = (megabytes(g, used), megabytes(g, total))
                && t > 0.0
            {
                vram = (Some(u), Some(t));
                break;
            }
        }
        let fan = by_kind(g, "Fan")
            .into_iter()
            .map(|(_, v)| v)
            .filter(|v| *v > 0.0 && *v < 30000.0)
            .reduce(f64::max);
        let temp = pick(&of(Part::Gpu, Some(i)), GPU_PREFS);
        let integrated =
            g.sensors.iter().any(|x| x.id.to_lowercase().contains("integrated")) || is_integrated(&g.name);
        s.gpu_shared = temp.is_none() && integrated && s.cpu_temp.is_some();
        s.gpu = Some(GpuSensors {
            name: g.name.clone(),
            integrated,
            temp: temp.map(|t| t as f32),
            load: load.map(|l| l as f32),
            clock_mhz: clock.map(|c| c as f32),
            vram_used_mb: vram.0,
            vram_total_mb: vram.1,
            fan_rpm: fan.map(|f| f as f32),
        });
    }

    s.ssd_temp = (0..hw.len())
        .filter_map(|i| {
            let t = of(Part::Storage, Some(i));
            (!t.is_empty()).then(|| pick(&t, STORAGE_PREFS)).flatten()
        })
        .reduce(f64::max)
        .map(|t| t as f32);

    s.fans = hw
        .iter()
        .flat_map(|h| h.sensors.iter().filter(|x| x.kind == "Fan" && x.value > 0.0 && x.value < 30000.0))
        .map(|x| Fan { name: x.name.clone(), rpm: x.value as f32 })
        .collect();
    s.fans.sort_by(|a, b| b.rpm.total_cmp(&a.rpm));

    s.drives = hw.iter().filter(|h| hw_part(&h.kind) == Part::Storage).filter_map(library_health).collect();
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sensor(id: &str, name: &str, kind: &str, value: f64) -> Sensor {
        Sensor { id: id.into(), name: name.into(), kind: kind.into(), value }
    }

    fn hw(id: &str, name: &str, kind: &str, sensors: Vec<Sensor>) -> Hardware {
        Hardware { id: id.into(), name: name.into(), kind: kind.into(), parent: None, sensors }
    }

    #[test]
    fn real_helper_output_without_admin() {
        // pcm-sensors.exe --once on a Ryzen 5 5500U laptop, no admin rights
        let text = include_str!("../tests/fixtures/lhm-ryzen5500u-user.json");
        let msgs: Vec<Message> = text.lines().filter_map(parse_line).collect();
        assert!(matches!(&msgs[0], Message::Hello { protocol: 1, admin: false, .. }));
        let Message::Sensors { hw, .. } = &msgs[1] else { panic!("no sensors line") };
        let s = summarize(hw);
        assert_eq!(s.cpu_temp, None, "Tctl reads 0 without admin rights");
        assert_eq!(s.cpu_power_w, None, "0 W = not read");
        assert_eq!(s.cpu_voltage_v, Some(1.55));
        let g = s.gpu.unwrap();
        assert!(g.integrated && g.temp.is_none() && !s.gpu_shared);
        assert_eq!((g.load, g.clock_mhz), (Some(2.113), Some(400.0)));
        assert_eq!((g.vram_used_mb, g.vram_total_mb), (Some(363.0), Some(2048.0)));
        assert!(s.fans.is_empty() && s.drives.is_empty());
    }

    #[test]
    fn admin_readings_pick_like_1_8() {
        let cpu = hw(
            "/amdcpu/0",
            "AMD Ryzen 5 5500U",
            "Cpu",
            vec![
                sensor("/amdcpu/0/temperature/2", "Core (Tctl/Tdie)", "Temperature", 61.0),
                sensor("/amdcpu/0/temperature/3", "CCD1 (Tdie)", "Temperature", 66.0),
                sensor("/amdcpu/0/power/0", "Package", "Power", 9.5),
                sensor("/amdcpu/0/clock/1", "Core #1", "Clock", 3000.0),
                sensor("/amdcpu/0/clock/2", "Core #2", "Clock", 4000.0),
                sensor("/amdcpu/0/clock/3", "Core #1 (Effective)", "Clock", 900.0),
            ],
        );
        let gpu = hw(
            "/gpu-nvidia/0",
            "NVIDIA GeForce RTX 3060",
            "GpuNvidia",
            vec![
                sensor("/gpu-nvidia/0/temperature/0", "GPU Core", "Temperature", 55.0),
                sensor("/gpu-nvidia/0/temperature/2", "GPU Hot Spot", "Temperature", 67.0),
                sensor("/gpu-nvidia/0/load/0", "GPU Core", "Load", 97.0),
                sensor("/gpu-nvidia/0/fan/1", "GPU Fan 1", "Fan", 1450.0),
                sensor("/gpu-nvidia/0/smalldata/1", "GPU Memory Used", "SmallData", 3000.0),
                sensor("/gpu-nvidia/0/smalldata/2", "GPU Memory Total", "SmallData", 12288.0),
            ],
        );
        let nvme = hw(
            "/nvme/1",
            "KIOXIA KBG50ZNV512G",
            "Storage",
            vec![
                sensor("/nvme/1/temperature/0", "Composite Temperature", "Temperature", 41.4),
                sensor("/nvme/1/temperature/1", "Warning Temperature", "Temperature", 83.0),
                sensor("/nvme/1/level/0", "Available Spare", "Level", 100.0),
                sensor("/nvme/1/level/1", "Available Spare Threshold", "Level", 10.0),
                sensor("/nvme/1/level/2", "Percentage Used", "Level", 2.0),
                sensor("/nvme/1/data/0", "Data Written", "Data", 7000.0),
            ],
        );
        let board = hw(
            "/lpc/nct6798d/0",
            "Nuvoton NCT6798D",
            "SuperIO",
            vec![
                sensor("/lpc/nct6798d/0/fan/0", "Fan #1", "Fan", 900.0),
                sensor("/lpc/x/fan/1", "Fan #2", "Fan", 0.0),
            ],
        );
        let s = summarize(&[cpu, gpu, nvme, board]);
        assert_eq!(s.cpu_temp, Some(Temp::exact(61.0)), "Tctl/Tdie before the hotter CCD");
        assert_eq!(s.cpu_clock_mhz, Some(3500.0), "effective clocks left out");
        assert_eq!(s.cpu_power_w, Some(9.5));
        let g = s.gpu.unwrap();
        assert_eq!((g.temp, g.load, g.fan_rpm), (Some(55.0), Some(97.0), Some(1450.0)));
        assert_eq!((g.vram_used_mb, g.vram_total_mb), (Some(3000.0), Some(12288.0)));
        assert!(!g.integrated && !s.gpu_shared);
        assert_eq!(s.ssd_temp, Some(41.4), "the warning threshold is not a reading");
        assert_eq!(
            s.fans.iter().map(|f| f.rpm).collect::<Vec<_>>(),
            vec![1450.0, 900.0],
            "stopped fans left out"
        );
        let d = &s.drives[0];
        assert_eq!(
            (d.drive, d.wear, d.spare, d.spare_min, d.temp),
            (Some(1), 2, Some(100), Some(10), Some(41.0))
        );
        assert_eq!(d.written, Some(7_000_000_000_000));
    }

    #[test]
    fn integrated_gpu_shares_the_cpu_temperature() {
        let cpu = hw(
            "/amdcpu/0",
            "AMD",
            "Cpu",
            vec![sensor("/amdcpu/0/temperature/2", "Core (Tctl/Tdie)", "Temperature", 50.0)],
        );
        let gpu = hw(
            "/gpu-amd/0",
            "AMD Radeon(TM) Graphics",
            "GpuAmd",
            vec![sensor("/gpu-amd/0/load/0", "D3D 3D", "Load", 5.0)],
        );
        let s = summarize(&[cpu, gpu]);
        assert!(s.gpu_shared);
    }

    #[test]
    fn protocol_lines() {
        assert_eq!(
            parse_line(r#"{"type":"error","message":"x"}"#),
            Some(Message::Error { message: "x".into() })
        );
        assert_eq!(parse_line("not json"), None);
        assert_eq!(parse_line(r#"{"type":"future","x":1}"#), None);
    }
}

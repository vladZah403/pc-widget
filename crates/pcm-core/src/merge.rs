//! Puts what the LibreHardwareMonitor helper read into a snapshot filled by
//! Windows' own sources: the helper's numbers win where they are better
//! (real CPU temperature and clock), Windows' stay where the helper has none.

use crate::model::{Snapshot, Temp};
use crate::sensors::Summary;

/// `admin`: the helper ran with admin rights. Without them the library still
/// reports some CPU numbers (e.g. VID 1.55 V on every core) that aren't
/// readings, so only GPU data, fans and drives are taken then.
pub fn apply(s: &mut Snapshot, sum: &Summary, admin: bool) {
    if admin {
        if let Some(t) = sum.cpu_temp {
            s.cpu.temp = Some(t); // the real sensor beats an ACPI zone "≈"
        }
        if sum.cpu_clock_mhz.is_some() {
            s.cpu.clock_mhz = sum.cpu_clock_mhz; // per-core clocks beat Windows' estimate
        }
        s.cpu.power_w = sum.cpu_power_w.or(s.cpu.power_w);
        s.cpu.voltage_v = sum.cpu_voltage_v.or(s.cpu.voltage_v);
    }

    if let Some(g) = &sum.gpu {
        // the library's GPU is the Windows adapter with the same name, or the only one
        let name = g.name.to_lowercase();
        let target = match s.gpus.iter().position(|x| x.name.to_lowercase() == name) {
            Some(i) => Some(i),
            None if s.gpus.len() == 1 => Some(0),
            None => None,
        };
        if let Some(i) = target {
            let gpu = &mut s.gpus[i];
            if gpu.temp.is_none() {
                gpu.temp = g.temp.map(Temp::exact);
            }
            gpu.clock_mhz = g.clock_mhz.or(gpu.clock_mhz);
            if gpu.fan_rpm.is_none() {
                gpu.fan_rpm = g.fan_rpm.map(|f| f.round() as u32);
            }
            if gpu.load.is_none() {
                gpu.load = g.load;
            }
            // an integrated GPU without a sensor of its own sits on the CPU's die
            if gpu.temp.is_none() && sum.gpu_shared {
                gpu.temp = s.cpu.temp.map(|t| Temp { celsius: t.celsius, approx: true });
            }
        }
    }

    if !sum.fans.is_empty() {
        s.fans = sum.fans.clone();
    }

    // Windows' NVMe query gives more (hours, warnings); the library reaches
    // drives behind Intel RST / VMD that refuse it
    for d in &sum.drives {
        if !s.drives.iter().any(|x| x.drive.is_some() && x.drive == d.drive) {
            s.drives.push(d.clone());
        }
    }
    for d in &mut s.drives {
        if d.temp.is_none() {
            d.temp = sum.drives.iter().find(|x| x.drive == d.drive).and_then(|x| x.temp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DriveHealth, Gpu, HealthSource};
    use crate::sensors::GpuSensors;

    fn drive(n: u32, source: HealthSource, temp: Option<f32>) -> DriveHealth {
        DriveHealth {
            drive: Some(n),
            model: "SSD".into(),
            letters: vec![],
            critical: 0,
            temp,
            spare: Some(100),
            spare_min: Some(10),
            wear: 3,
            written: None,
            hours: None,
            source,
        }
    }

    #[test]
    fn helper_fills_what_windows_lacks() {
        let mut s = Snapshot::default();
        s.cpu.temp = Some(Temp { celsius: 70.0, approx: true });
        s.cpu.clock_mhz = Some(2100.0);
        s.gpus = vec![Gpu {
            name: "NVIDIA GeForce RTX 3060".into(),
            temp: Some(Temp::exact(50.0)),
            ..Default::default()
        }];
        s.drives = vec![drive(0, HealthSource::Windows, None)];
        let sum = Summary {
            cpu_temp: Some(Temp::exact(64.0)),
            cpu_clock_mhz: Some(3900.0),
            cpu_voltage_v: Some(1.2),
            gpu: Some(GpuSensors {
                name: "NVIDIA GeForce RTX 3060".into(),
                temp: Some(55.0),
                clock_mhz: Some(1800.0),
                fan_rpm: Some(1400.4),
                ..Default::default()
            }),
            drives: vec![
                drive(0, HealthSource::Library, Some(40.0)),
                drive(1, HealthSource::Library, Some(45.0)),
            ],
            ..Default::default()
        };
        let mut user = s.clone();
        apply(&mut s, &sum, true);
        assert_eq!(s.cpu.temp, Some(Temp::exact(64.0)));
        assert_eq!((s.cpu.clock_mhz, s.cpu.voltage_v), (Some(3900.0), Some(1.2)));
        let g = &s.gpus[0];
        assert_eq!(g.temp, Some(Temp::exact(50.0)), "Windows' own GPU temperature stays");
        assert_eq!((g.clock_mhz, g.fan_rpm), (Some(1800.0), Some(1400)));
        assert_eq!(s.drives.len(), 2, "drive 1 only the library sees");
        assert_eq!(s.drives[0].source, HealthSource::Windows);
        assert_eq!(s.drives[0].temp, Some(40.0));

        apply(&mut user, &sum, false);
        assert_eq!(user.cpu.temp, Some(Temp { celsius: 70.0, approx: true }), "no CPU numbers without admin");
        assert_eq!(user.cpu.voltage_v, None);
    }

    #[test]
    fn integrated_gpu_shows_the_cpu_temperature_approximately() {
        let mut s = Snapshot {
            gpus: vec![Gpu { name: "AMD Radeon(TM) Graphics".into(), ..Default::default() }],
            ..Default::default()
        };
        let sum = Summary {
            cpu_temp: Some(Temp::exact(60.0)),
            gpu: Some(GpuSensors {
                name: "AMD Radeon(TM) Graphics".into(),
                integrated: true,
                ..Default::default()
            }),
            gpu_shared: true,
            ..Default::default()
        };
        apply(&mut s, &sum, true);
        assert_eq!(s.gpus[0].temp, Some(Temp { celsius: 60.0, approx: true }));
    }
}

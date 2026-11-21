//! What PC Monitor knows about the PC at one moment.
//!
//! The rule for every field: a value no source could read is `None` (or an
//! empty list) and its row is hidden, never shown as 0.

/// Bytes, as all sizes are.
pub type Bytes = u64;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub system: System,
    pub cpu: Cpu,
    pub memory: Option<Memory>,
    pub gpus: Vec<Gpu>,
    pub volumes: Vec<Volume>,
    pub disk_io: Option<DiskIo>,
    pub drives: Vec<DriveHealth>,
    pub net: Option<Net>,
    pub ping: Option<Ping>,
    pub battery: Option<Battery>,
    pub fans: Vec<Fan>,
    pub processes: Vec<ProcessGroup>,
    pub displays: Vec<Display>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct System {
    pub windows_build: Option<u32>,
    pub admin: bool,
    pub uptime_s: Option<u64>,
    pub board: Option<Board>,
    pub memory_modules: Vec<MemoryModule>,
    /// The PawnIO driver (LibreHardwareMonitor needs it for CPU temperatures).
    pub pawnio: bool,
}

/// A temperature in °C; `approx` = not the part's own sensor (an ACPI
/// thermal zone standing in for the CPU), shown with "≈".
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Temp {
    pub celsius: f32,
    pub approx: bool,
}

impl Temp {
    pub fn exact(celsius: f32) -> Temp {
        Temp { celsius, approx: false }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cpu {
    pub name: Option<String>,
    pub cores: Option<u32>,
    pub threads: Option<u32>,
    /// 0..100, like Task Manager.
    pub load: Option<f32>,
    pub per_core: Vec<f32>,
    pub clock_mhz: Option<f32>,
    pub temp: Option<Temp>,
    pub power_w: Option<f32>,
    pub voltage_v: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Memory {
    pub total: Bytes,
    pub used: Bytes,
}

impl Memory {
    pub fn percent(&self) -> f32 {
        if self.total == 0 { 0.0 } else { self.used as f32 / self.total as f32 * 100.0 }
    }
}

/// One RAM stick from SMBIOS.
#[derive(Clone, Debug, PartialEq)]
pub struct MemoryModule {
    pub locator: String,
    pub size: Bytes,
    /// "DDR4", "LPDDR5", …
    pub kind: Option<String>,
    /// MT/s (what "DDR4-3200" means).
    pub speed_mts: Option<u32>,
    pub maker: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Board {
    pub system_maker: Option<String>,
    pub system_product: Option<String>,
    pub board_maker: Option<String>,
    pub board_product: Option<String>,
    pub bios_vendor: Option<String>,
    pub bios_version: Option<String>,
    pub bios_date: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Gpu {
    pub name: String,
    /// The adapter LUID (high << 32 | low), the key that ties sources together.
    pub luid: u64,
    pub integrated: bool,
    pub load: Option<f32>,
    pub dedicated_used: Option<Bytes>,
    pub dedicated_total: Option<Bytes>,
    pub shared_used: Option<Bytes>,
    pub temp: Option<Temp>,
    pub clock_mhz: Option<f32>,
    pub fan_rpm: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Volume {
    /// "C:"
    pub letter: String,
    pub label: Option<String>,
    pub total: Bytes,
    pub free: Bytes,
    pub removable: bool,
}

impl Volume {
    pub fn used(&self) -> Bytes {
        self.total.saturating_sub(self.free)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiskIo {
    pub read_bps: f64,
    pub write_bps: f64,
    /// Active time 0..100 per physical disk ("0 C:", like Task Manager).
    pub active: Vec<(String, f32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HealthSource {
    /// Windows' own NVMe query (no admin rights needed).
    Windows,
    /// LibreHardwareMonitor (needs admin; works behind Intel RST / VMD).
    Library,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DriveHealth {
    /// \\.\PhysicalDrive<n>
    pub drive: Option<u32>,
    pub model: String,
    pub letters: Vec<String>,
    /// NVMe critical warning bits (spare low, too hot, read-only…).
    pub critical: u8,
    pub temp: Option<f32>,
    /// % of spare blocks left / the drive's own limit.
    pub spare: Option<u8>,
    pub spare_min: Option<u8>,
    /// "Percentage used" of the rated endurance, 0..100.
    pub wear: u8,
    pub written: Option<Bytes>,
    pub hours: Option<u64>,
    pub source: HealthSource,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Net {
    pub down_bps: f64,
    pub up_bps: f64,
    pub adapter: Option<String>,
    pub ipv4: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ping {
    pub host: String,
    /// None = no answer.
    pub ms: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Battery {
    pub percent: Option<f32>,
    pub on_ac: bool,
    pub charging: bool,
    pub seconds_left: Option<u32>,
    pub design_wh: Option<f32>,
    pub full_wh: Option<f32>,
    /// Charge or discharge power, always positive.
    pub rate_w: Option<f32>,
}

impl Battery {
    /// Lost capacity in %, from design and full-charge capacity.
    pub fn wear(&self) -> Option<f32> {
        match (self.design_wh, self.full_wh) {
            (Some(d), Some(f)) if d > 0.0 && f > 0.0 => Some(((1.0 - f / d) * 100.0).max(0.0)),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Fan {
    pub name: String,
    pub rpm: f32,
}

/// All processes of one program, summed (like 1.8's process list).
#[derive(Clone, Debug, PartialEq)]
pub struct ProcessGroup {
    /// Without ".exe".
    pub name: String,
    /// % of the whole PC (all cores = 100).
    pub cpu: f32,
    /// Private working set, the number Task Manager shows.
    pub memory: Bytes,
    pub pids: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Display {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub refresh_hz: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battery_wear_and_memory_percent() {
        let b = Battery { design_wh: Some(42.0), full_wh: Some(35.29), ..Default::default() };
        assert!((b.wear().unwrap() - 15.976).abs() < 0.01);
        assert_eq!(Battery::default().wear(), None);
        assert_eq!(Memory { total: 200, used: 50 }.percent(), 25.0);
        assert_eq!(Memory { total: 0, used: 0 }.percent(), 0.0);
    }
}

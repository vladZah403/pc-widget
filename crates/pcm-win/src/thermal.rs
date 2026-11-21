//! The hottest ACPI thermal zone from Windows' "Thermal Zone Information"
//! counters: no driver, no admin rights. On many laptops it is the CPU as the
//! firmware sees it, so it stands in for the CPU temperature (shown with "≈")
//! when LibreHardwareMonitor has none (an Intel CPU without PawnIO).

use pcm_core::model::Temp;

use crate::pdh::{Counter, Query};

pub struct ThermalZones {
    query: Query,
    counter: Counter,
    /// High Precision Temperature is in tenths of a kelvin.
    divisor: f64,
}

impl ThermalZones {
    /// None when this PC has no such counters.
    pub fn new() -> Option<ThermalZones> {
        let query = Query::open()?;
        for (path, divisor) in [
            (r"\Thermal Zone Information(*)\High Precision Temperature", 10.0),
            (r"\Thermal Zone Information(*)\Temperature", 1.0),
        ] {
            if let Some(counter) = query.add(path) {
                return Some(ThermalZones { query, counter, divisor });
            }
        }
        None
    }

    /// (zone, °C) of every zone, plausible or not.
    pub fn zones(&self) -> Vec<(String, f32)> {
        if !self.query.collect() {
            return Vec::new();
        }
        self.counter.values().into_iter().map(|(n, v)| (n, (v / self.divisor - 273.15) as f32)).collect()
    }

    /// The hottest plausible zone, as an approximate CPU temperature.
    pub fn read(&self) -> Option<Temp> {
        self.zones()
            .into_iter()
            .map(|(_, t)| t)
            .filter(|t| *t > 10.0 && *t < 115.0)
            .reduce(f32::max)
            .map(|t| Temp { celsius: (t * 10.0).round() / 10.0, approx: true })
    }
}

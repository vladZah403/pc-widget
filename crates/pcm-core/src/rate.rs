//! Per-second rates from ever-growing counters (bytes received, read…).

#[derive(Clone, Debug, Default)]
pub struct Rate {
    last: Option<(f64, u64)>,
}

impl Rate {
    /// The counter's growth per second since the last call, at time `t` (s).
    /// None on the first call and when the counter went back (an adapter
    /// was reset or removed).
    pub fn update(&mut self, t: f64, total: u64) -> Option<f64> {
        let rate = match self.last {
            Some((t0, v0)) if t > t0 && total >= v0 => Some((total - v0) as f64 / (t - t0)),
            _ => None,
        };
        self.last = Some((t, total));
        rate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_per_second() {
        let mut r = Rate::default();
        assert_eq!(r.update(10.0, 1000), None);
        assert_eq!(r.update(12.0, 5000), Some(2000.0));
        assert_eq!(r.update(13.0, 100), None, "counter reset");
        assert_eq!(r.update(14.0, 600), Some(500.0));
        assert_eq!(r.update(14.0, 700), None, "no time passed");
    }
}

//! The lowest, average and highest of a reading since PC Monitor started
//! (HWiNFO's min / avg / max columns), kept per sensor.

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Span {
    min: f32,
    max: f32,
    sum: f64,
    n: u64,
}

impl Span {
    pub fn add(&mut self, v: f32) {
        if !v.is_finite() {
            return;
        }
        if self.n == 0 {
            (self.min, self.max) = (v, v);
        } else {
            self.min = self.min.min(v);
            self.max = self.max.max(v);
        }
        self.sum += v as f64;
        self.n += 1;
    }

    /// [min, average, max]; None before the first reading.
    pub fn get(&self) -> Option<[f32; 3]> {
        (self.n > 0).then(|| [self.min, (self.sum / self.n as f64) as f32, self.max])
    }
}

/// Spans by sensor ("cpu", "gpu", "C:").
#[derive(Default)]
pub struct Spans(BTreeMap<String, Span>);

impl Spans {
    pub fn add(&mut self, key: &str, v: Option<f32>) {
        if let Some(v) = v {
            self.0.entry(key.to_string()).or_default().add(v);
        }
    }

    /// Every sensor's [min, average, max].
    pub fn all(&self) -> Vec<(String, [f32; 3])> {
        self.0.iter().filter_map(|(k, s)| Some((k.clone(), s.get()?))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_avg_max() {
        let mut s = Spans::default();
        assert!(s.all().is_empty());
        for v in [50.0, 70.0, 60.0, f32::NAN] {
            s.add("cpu", Some(v));
        }
        s.add("gpu", None);
        assert_eq!(s.all(), vec![("cpu".to_string(), [50.0, 60.0, 70.0])]);
    }
}

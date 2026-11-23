//! Throttling: the processor gives less than it could while it is busy.
//!
//! Two signs. Windows' "% Performance Limit" (how much of its nominal
//! performance the processor is allowed, without admin rights) drops below
//! 100 when the platform holds it back: overheating, a power budget, the
//! power plan. And the temperature: AMD's laptop chips hold their clocks
//! back by themselves at their limit, without telling Windows, so a
//! processor at THROTTLE_C under load counts too. At idle neither matters
//! (a limit then is the power plan saving the battery).

/// "% Performance Limit" below this: held back.
pub const LIMIT_BELOW: f32 = 95.0;
/// At this temperature (°C) laptops' processors hold their clocks back
/// (Intel's limit is 100 °C, AMD's laptop chips 95–105 °C).
pub const THROTTLE_C: f32 = 95.0;
/// A held-back processor runs hot from this temperature: the cause is heat.
pub const HOT_C: f32 = 85.0;
/// The processor's load from which it counts (%).
pub const BUSY: f32 = 30.0;
/// How long it must last before an alert (s).
pub const HOLD_S: f64 = 10.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cause {
    /// Overheating.
    Heat,
    /// A power budget or the power plan (the battery, "Best battery life").
    Power,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Throttle {
    pub cause: Cause,
    /// How much Windows still allows, % (None: only the temperature says so).
    pub limit: Option<f32>,
}

/// Is the processor held back right now?
pub fn check(limit: Option<f32>, temp: Option<f32>, load: Option<f32>) -> Option<Throttle> {
    if load.is_none_or(|l| l < BUSY) {
        return None;
    }
    let hot = temp.is_some_and(|t| t >= HOT_C);
    match limit.filter(|l| *l < LIMIT_BELOW) {
        Some(l) => Some(Throttle { cause: if hot { Cause::Heat } else { Cause::Power }, limit: Some(l) }),
        None if temp.is_some_and(|t| t >= THROTTLE_C) => Some(Throttle { cause: Cause::Heat, limit: None }),
        None => None,
    }
}

/// Over a game session: seconds held back by heat and by power, and the
/// lowest limit Windows set while it was hot.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Totals {
    pub heat_s: u32,
    pub power_s: u32,
    pub heat_limit: Option<f32>,
}

pub fn totals(samples: &[crate::session::Sample]) -> Totals {
    let mut t = Totals::default();
    for s in samples {
        if let Some(th) = check(s.limit, s.cpu_t, s.cpu) {
            match th.cause {
                Cause::Heat => {
                    t.heat_s += 1;
                    if let Some(l) = th.limit {
                        t.heat_limit = Some(t.heat_limit.map_or(l, |m: f32| m.min(l)));
                    }
                }
                Cause::Power => t.power_s += 1,
            }
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn causes() {
        // idle: never
        assert_eq!(check(Some(50.0), Some(99.0), Some(5.0)), None);
        // busy, Windows holds it back, cool: power
        assert_eq!(check(Some(70.0), Some(60.0), Some(80.0)).map(|t| t.cause), Some(Cause::Power));
        // busy, held back and hot: heat
        assert_eq!(check(Some(70.0), Some(92.0), Some(80.0)).map(|t| t.cause), Some(Cause::Heat));
        // busy at the limit temperature, Windows says nothing (AMD): heat
        assert_eq!(
            check(Some(100.0), Some(96.0), Some(60.0)),
            Some(Throttle { cause: Cause::Heat, limit: None })
        );
        // busy, hot but below the limit, not held back: no
        assert_eq!(check(Some(100.0), Some(88.0), Some(60.0)), None);
        assert_eq!(check(None, None, Some(100.0)), None);
    }

    #[test]
    fn session_totals() {
        use crate::session::Sample;
        let s =
            |cpu, cpu_t, limit| Sample { cpu: Some(cpu), cpu_t: Some(cpu_t), limit, ..Default::default() };
        let samples =
            [s(80.0, 96.0, None), s(80.0, 90.0, Some(70.0)), s(80.0, 50.0, Some(55.0)), s(10.0, 99.0, None)];
        let t = totals(&samples);
        // the power plan's 55% is not the heat's limit
        assert_eq!((t.heat_s, t.power_s, t.heat_limit), (2, 1, Some(70.0)));
    }
}

//! Alerts, as 1.8 gives them: a condition must hold for a few seconds, the
//! same alert comes at most once in 10 minutes and only again after it
//! cleared (with a margin), and one that fires while a game is in front is
//! held until the game is over.

use std::collections::HashMap;

use pcm_render::format::{fmt_gb, fmt_left, fmt_mem, tr, trf};
use pcm_render::widget::Data;

pub const COOLDOWN_S: f64 = 600.0;
/// How far a reading must go back past the limit before the alert can fire again.
pub const HYST: f32 = 5.0;
/// SSD wear (% of its rated endurance) that is worth an alert.
pub const SSD_WEAR: u8 = 90;

#[derive(Clone, Debug, PartialEq)]
pub struct Alert {
    pub title: String,
    pub body: String,
    /// true: the hot end of the scale; false: the warm (3/4) colour.
    pub hot: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    pub temp: f32,
    pub ram: f32,
    pub battery: f32,
    /// Free space on the system drive, %.
    pub disk: f32,
}

struct State {
    armed: bool,
    since: Option<f64>,
    last: f64,
}

#[derive(Default)]
pub struct Alerts {
    states: HashMap<String, State>,
    held: Option<Alert>,
}

impl Alerts {
    /// Fire when `active` has held for `hold_s`; re-arm once `cleared`.
    fn rule(
        &mut self,
        now: f64,
        key: &str,
        active: bool,
        cleared: bool,
        hold_s: f64,
        make: impl FnOnce() -> Alert,
    ) -> Option<Alert> {
        let st = self.states.entry(key.into()).or_insert(State {
            armed: true,
            since: None,
            last: f64::NEG_INFINITY,
        });
        if active {
            let since = *st.since.get_or_insert(now);
            if st.armed && now - since >= hold_s && now - st.last >= COOLDOWN_S {
                st.armed = false;
                st.last = now;
                return Some(make());
            }
        } else {
            st.since = None;
            if cleared {
                st.armed = true;
            }
        }
        None
    }

    /// Check the readings at time `now` (s). Returns the alert to show now;
    /// while `in_game` it is held and comes after the game. `top_ram` names
    /// the program that uses the most memory (asked only when needed).
    pub fn check(
        &mut self,
        now: f64,
        d: &Data,
        limits: &Limits,
        system_drive: &str,
        in_game: bool,
        top_ram: impl FnOnce() -> Option<(String, u64)>,
    ) -> Option<Alert> {
        let mut fired: Vec<Alert> = Vec::new();
        let lim = limits.temp;
        if let Some(t) = d.temp_cpu {
            let body = trf(
                "CPU {t}°. Закрой тяжёлые программы или проверь, не забиты ли решётки.",
                &[("t", format!("{t:.0}"))],
            );
            fired.extend(self.rule(now, "cpu_temp", t >= lim, t < lim - HYST, 5.0, || Alert {
                title: tr("Процессор перегревается"),
                body,
                hot: true,
            }));
        }
        if let Some(t) = d.temp_gpu {
            let body =
                trf("GPU {t}°. Понизь настройки графики или ограничь FPS.", &[("t", format!("{t:.0}"))]);
            fired.extend(self.rule(now, "gpu_temp", t >= lim - 3.0, t < lim - 3.0 - HYST, 5.0, || Alert {
                title: tr("Видеокарта перегревается"),
                body,
                hot: true,
            }));
        }
        let pct = d.ram_pct;
        fired.extend(self.rule(now, "ram", pct >= limits.ram, pct < limits.ram - HYST, 10.0, || {
            let mut body = trf(
                "Занято {pct}% ({used} из {total} ГБ).",
                &[("pct", format!("{pct:.0}")), ("used", fmt_gb(d.ram_used)), ("total", fmt_gb(d.ram_total))],
            );
            if let Some((name, mem)) = top_ram() {
                body += " ";
                body += &trf("Больше всех: {name} — {mem}.", &[("name", name), ("mem", fmt_mem(mem))]);
            }
            Alert { title: tr("Память почти закончилась"), body, hot: true }
        }));
        if let Some((level, plugged)) = d.battery {
            let low = limits.battery;
            let mut body = trf("Осталось {level}%", &[("level", format!("{level:.0}"))]);
            if let Some(left) = d.battery_left {
                body += &trf(", примерно на {left}", &[("left", fmt_left(left))]);
            }
            body += ". ";
            body += &tr("Подключи зарядку.");
            fired.extend(self.rule(
                now,
                "battery",
                !plugged && level <= low,
                plugged || level > low + HYST,
                0.0,
                || Alert { title: tr("Батарея садится"), body, hot: false },
            ));
        }
        for (name, used, total, pct) in &d.disks {
            if !name.eq_ignore_ascii_case(system_drive) || *total == 0 {
                continue;
            }
            let free = 100.0 - pct;
            let body = trf(
                "Свободно {free} ГБ ({pct}%). Очисти корзину и «Загрузки» или удали ненужные программы.",
                &[("free", fmt_gb(total - used)), ("pct", format!("{free:.0}"))],
            );
            fired.extend(self.rule(
                now,
                "disk_space",
                free < limits.disk,
                free >= limits.disk + 2.0,
                10.0,
                || Alert {
                    title: trf("Мало места на диске {name}", &[("name", name.clone())]),
                    body,
                    hot: false,
                },
            ));
        }
        for drive in &d.drives {
            let name = std::iter::once("SSD".to_string())
                .chain(drive.letters.iter().cloned())
                .collect::<Vec<_>>()
                .join(" ");
            let failing = drive.critical != 0 || drive.spare < drive.spare_min;
            let worn = drive.wear >= SSD_WEAR;
            let key = format!("ssd_{}", drive.letters.join(""));
            fired.extend(self.rule(now, &key, failing || worn, !(failing || worn), 0.0, || Alert {
                title: if failing {
                    trf("{name} сообщает о неисправности", &[("name", name.clone())])
                } else {
                    trf("{name} изношен", &[("name", name.clone())])
                },
                body: if failing {
                    tr("Сделай копию важных файлов.")
                } else {
                    trf(
                        "Износ {wear}%. Сделай копию важных файлов и присмотри замену.",
                        &[("wear", drive.wear.to_string())],
                    )
                },
                hot: true,
            }));
        }
        if in_game {
            // never over a game: the newest one waits for the game to end
            if let Some(a) = fired.pop() {
                self.held = Some(a);
            }
            return None;
        }
        fired.pop().or_else(|| self.held.take())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: Limits = Limits { temp: 90.0, ram: 90.0, battery: 15.0, disk: 10.0 };

    fn data(cpu_temp: f32) -> Data {
        Data { temp_cpu: Some(cpu_temp), ram_pct: 50.0, ..Data::default() }
    }

    #[test]
    fn holds_cools_down_and_rearms() {
        let mut a = Alerts::default();
        let none = || None;
        assert!(a.check(0.0, &data(95.0), &LIMITS, "C:", false, none).is_none(), "must hold 5 s first");
        let fired = a.check(5.0, &data(95.0), &LIMITS, "C:", false, none).unwrap();
        assert_eq!(fired.title, "Процессор перегревается");
        assert!(fired.body.contains("95°"));
        assert!(a.check(6.0, &data(96.0), &LIMITS, "C:", false, none).is_none(), "once");
        // cooled a little (88 < 90 but not below 85): stays disarmed
        a.check(100.0, &data(88.0), &LIMITS, "C:", false, none);
        a.check(700.0, &data(95.0), &LIMITS, "C:", false, none);
        assert!(a.check(710.0, &data(95.0), &LIMITS, "C:", false, none).is_none());
        // properly cool, then hot again after the 10 minutes
        a.check(720.0, &data(80.0), &LIMITS, "C:", false, none);
        a.check(721.0, &data(95.0), &LIMITS, "C:", false, none);
        assert!(a.check(726.0, &data(95.0), &LIMITS, "C:", false, none).is_some());
    }

    #[test]
    fn held_during_a_game() {
        let mut a = Alerts::default();
        let low = Data { battery: Some((10.0, false)), battery_left: Some(1800), ..Data::default() };
        assert!(a.check(0.0, &low, &LIMITS, "C:", true, || None).is_none(), "not over the game");
        let after = a.check(60.0, &Data::default(), &LIMITS, "C:", false, || None).unwrap();
        assert_eq!(after.title, "Батарея садится");
        assert_eq!(after.body, "Осталось 10%, примерно на 30 мин. Подключи зарядку.");
    }

    #[test]
    fn memory_names_the_biggest_program() {
        let mut a = Alerts::default();
        let full = Data { ram_pct: 95.0, ram_used: 21 << 30, ram_total: 22 << 30, ..Data::default() };
        a.check(0.0, &full, &LIMITS, "C:", false, || None);
        let al = a.check(10.0, &full, &LIMITS, "C:", false, || Some(("chrome".into(), 3 << 30))).unwrap();
        assert_eq!(al.body, "Занято 95% (21,0 из 22,0 ГБ). Больше всех: chrome — 3,0 ГБ.");
    }

    #[test]
    fn system_disk_and_worn_ssd() {
        let mut a = Alerts::default();
        let d = Data {
            ram_pct: 10.0,
            disks: vec![("D:".into(), 99, 100, 99.0), ("C:".into(), 95 << 30, 100 << 30, 95.0)],
            ..Data::default()
        };
        a.check(0.0, &d, &LIMITS, "C:", false, || None);
        let al = a.check(10.0, &d, &LIMITS, "C:", false, || None).unwrap();
        assert_eq!(al.title, "Мало места на диске C:");
        let worn = Data {
            ram_pct: 10.0,
            drives: vec![pcm_render::widget::Drive {
                letters: vec!["C:".into()],
                critical: 0,
                temp: None,
                spare: 100,
                spare_min: 10,
                wear: 92,
                written: None,
            }],
            ..Data::default()
        };
        let al = Alerts::default().check(0.0, &worn, &LIMITS, "C:", false, || None).unwrap();
        assert_eq!(al.title, "SSD C: изношен");
    }
}

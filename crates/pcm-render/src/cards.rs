//! The dashboard's big cards (stage 5): one card per part of the PC, a ring
//! gauge where there is a load to show, rows of details beside it. The same
//! palette, fonts and numbers as the widget; the dashboard shows them as
//! textures, so they are tested as PNGs like the widget.

use tiny_skia::Pixmap;

use crate::canvas::{Align, Canvas, part};
use crate::format::*;
use crate::text::{Fonts, Style};
use crate::theme::*;
use crate::widget::Data;

/// Logical size of a card.
pub const CARD_W: f32 = 300.0;
pub const CARD_H: f32 = 210.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardKind {
    Cpu,
    Gpu,
    Memory,
    Disks,
    Network,
    Power,
    System,
}

/// The overview's cards, in order.
pub const CARDS: [CardKind; 7] = [
    CardKind::Cpu,
    CardKind::Gpu,
    CardKind::Memory,
    CardKind::Disks,
    CardKind::Network,
    CardKind::Power,
    CardKind::System,
];

const PAD: f32 = 18.0;
/// The ring: 270°, the gap at the bottom.
const RING_FROM: f32 = 135.0;
const RING_SWEEP: f32 = 270.0;
const RING: (f32, f32, f32) = (78.0, 132.0, 48.0);
/// The column beside the ring.
const COL_X: f32 = 152.0;

struct Card<'a> {
    c: Canvas,
    f: &'a mut Fonts,
    sc: Scale,
}

impl Card<'_> {
    fn frame(&mut self, title: &str, subtitle: &str) {
        let (w, h) = (CARD_W, CARD_H);
        let s = self.c.s;
        self.c.rrect(0.0, 0.0, w, h, 16.0, ink().border);
        let bw = 1.0 / s * s.round().max(1.0);
        self.c.rrect(bw, bw, w - bw, h - bw, 16.0 - bw, ink().card);
        self.c.text(self.f, PAD, 30.0, &tr(title), Style::SemiBold, 14.0, ink().text);
        if !subtitle.is_empty() {
            let sub = self.c.ellipsize(self.f, subtitle, Style::Regular, 11.0, w - 2.0 * PAD);
            self.c.text(self.f, PAD, 48.0, &sub, Style::Regular, 11.0, ink().dim);
        }
    }

    /// The gauge filled to `t` (0..1) in the scale's colours, `big` in the middle.
    fn ring(&mut self, t: Option<f32>, big: &str, unit: &str, hot: impl Fn(f32) -> Rgb) {
        let (cx, cy, r) = RING;
        self.c.arc(cx, cy, r, RING_FROM, RING_SWEEP, 11.0, true, ink().track);
        if let Some(t) = t.map(|t| t.clamp(0.0, 1.0)).filter(|t| *t > 0.002) {
            // short segments, each in its own colour along the way
            let segs = ((RING_SWEEP * t / 6.0).ceil() as usize).max(1);
            let step = RING_SWEEP * t / segs as f32;
            for i in 0..segs {
                let last = i + 1 == segs;
                let sweep = if last { step } else { step + 0.6 }; // no hairline gaps
                let col = hot((i as f32 + 0.5) / segs as f32 * t);
                self.c.arc(cx, cy, r, RING_FROM + step * i as f32, sweep, 11.0, i == 0 || last, col);
            }
        }
        let parts = [
            part(big, Style::SemiBoldCondensed, 32.0, ink().text),
            part(unit, Style::Regular, 14.0, ink().dim),
        ];
        self.c.rich(self.f, cx, cy + 10.0, &parts, Align::Center);
    }

    /// "label ........ value" in the column beside the ring (or across the card).
    fn row(&mut self, x0: f32, x1: f32, y: f32, label: &str, value: &str, col: Rgb) {
        self.c.text(self.f, x0, y, &tr(label), Style::Regular, 12.0, ink().dim);
        let v = part(value, Style::SemiBold, 12.0, col);
        self.c.rich(self.f, x1, y, &[v], Align::Right);
    }

    fn bar(&mut self, x0: f32, x1: f32, y: f32, t: f32, col: Rgb) {
        self.c.rrect(x0, y, x1, y + 6.0, 3.0, ink().track);
        let t = t.clamp(0.0, 1.0);
        if t > 0.0 {
            self.c.rrect(x0, y, x0 + ((x1 - x0) * t).max(6.0), y + 6.0, 3.0, col);
        }
    }

    fn temp_text(&self, t: Option<f32>, approx: bool) -> (String, Rgb) {
        match t {
            Some(t) => (format!("{}{t:.0}°", if approx { "≈" } else { "" }), self.sc.heat(t)),
            None => ("—".into(), ink().faint),
        }
    }
}

fn pct(v: f32) -> String {
    format!("{v:.0}")
}

pub fn card(fonts: &mut Fonts, scale: f32, kind: CardKind, d: &Data, theme: &Theme) -> Option<Pixmap> {
    let c = Canvas::new((CARD_W * scale).round() as u32, (CARD_H * scale).round() as u32, scale)?;
    let mut k = Card { c, f: fonts, sc: theme.scale() };
    let sc = k.sc.clone();
    let x1 = CARD_W - PAD;
    match kind {
        CardKind::Cpu => {
            k.frame("Процессор", &d.cpu_name);
            k.ring(Some(d.cpu / 100.0), &pct(d.cpu), "%", |t| sc.at(t));
            let clock = d
                .cpu_clock_ghz
                .filter(|c| *c > 0.0)
                .map(|c| trf("{v} ГГц", &[("v", dec(format!("{c:.1}")))]));
            k.row(COL_X, x1, 92.0, "Частота", &clock.unwrap_or("—".into()), ink().text);
            let (t, tc) = k.temp_text(d.temp_cpu, d.temp_cpu_approx);
            k.row(COL_X, x1, 114.0, "Температура", &t, tc);
            let power =
                d.cpu_power_w.filter(|p| *p > 0.0).map(|p| trf("{v} Вт", &[("v", format!("{p:.0}"))]));
            k.row(COL_X, x1, 136.0, "Мощность", &power.unwrap_or("—".into()), ink().text);
            // a bar per core
            let n = d.cores.len().max(1) as f32;
            let (bx0, by0, by1) = (COL_X, 152.0, 188.0);
            let step = (x1 - bx0) / n;
            for (i, &l) in d.cores.iter().enumerate() {
                let x = bx0 + i as f32 * step;
                let w = (step - 2.0).max(1.0);
                k.c.rrect(x, by0, x + w, by1, 1.5, ink().track);
                let h = ((by1 - by0) * (l / 100.0).clamp(0.0, 1.0)).max(1.5);
                k.c.rrect(x, by1 - h, x + w, by1, 1.5, sc.at(l / 100.0));
            }
        }
        CardKind::Gpu => {
            k.frame("Видеокарта", d.gpu_name.as_deref().unwrap_or(""));
            let load = d.gpu_load;
            k.ring(
                load.map(|l| l / 100.0),
                &load.map_or("—".into(), pct),
                if load.is_some() { "%" } else { "" },
                |t| sc.at(t),
            );
            let shared = d.gpu_shared && d.temp_gpu.is_none() && d.temp_cpu.is_some();
            let (t, tc) = k.temp_text(if shared { d.temp_cpu } else { d.temp_gpu }, shared);
            k.row(COL_X, x1, 92.0, "Температура", &t, tc);
            let clock = d.gpu_clock.filter(|c| *c > 0.0).map(|c| trf("{v} МГц", &[("v", format!("{c:.0}"))]));
            k.row(COL_X, x1, 114.0, "Частота", &clock.unwrap_or("—".into()), ink().text);
            match (d.vram_used_mb, d.vram_total_mb) {
                (Some(u), Some(t)) if t > 0.0 => {
                    let v = trf(
                        "{used} / {total} ГБ",
                        &[
                            ("used", dec(format!("{:.1}", u / 1024.0))),
                            ("total", dec(format!("{:.1}", t / 1024.0))),
                        ],
                    );
                    k.row(COL_X, x1, 136.0, "VRAM", &v, ink().text);
                    k.bar(COL_X, x1, 148.0, u / t, sc.at(u / t));
                }
                _ => k.row(COL_X, x1, 136.0, "VRAM", "—", ink().faint),
            }
        }
        CardKind::Memory => {
            k.frame("Память", &tr("ОЗУ"));
            k.ring(Some(d.ram_pct / 100.0), &pct(d.ram_pct), "%", |t| sc.at(t));
            k.row(COL_X, x1, 92.0, "Занято", &fmt_mem(d.ram_used), ink().text);
            k.row(COL_X, x1, 114.0, "Свободно", &fmt_mem(d.ram_total.saturating_sub(d.ram_used)), ink().text);
            k.row(COL_X, x1, 136.0, "Всего", &fmt_mem(d.ram_total), ink().text);
        }
        CardKind::Disks => {
            let io = format!(
                "{} {}  ·  {} {}",
                tr("Чтение"),
                fmt_speed(d.io_read),
                tr("запись"),
                fmt_speed(d.io_write)
            );
            k.frame("Диски", &io);
            let mut y = 76.0;
            for (letter, used, total, p) in d.disks.iter().take(3) {
                k.c.text(k.f, PAD, y, letter, Style::SemiBold, 12.0, ink().text);
                let v = part(fmt_disk(*used, *total), Style::Regular, 11.0, ink().dim);
                k.c.rich(k.f, x1, y, &[v], Align::Right);
                k.bar(PAD, x1, y + 7.0, p / 100.0, sc.at(p / 100.0));
                y += 34.0;
            }
            if let Some(dr) = d.drives.first() {
                let mut parts = vec![trf("износ {v}%", &[("v", dr.wear.to_string())])];
                if let Some(t) = dr.temp {
                    parts.push(format!("{t:.0}°"));
                }
                if let Some(w) = dr.written {
                    parts.push(trf("записано {v}", &[("v", fmt_written(w))]));
                }
                let name = if dr.letters.is_empty() {
                    "SSD".to_string()
                } else {
                    format!("SSD {}", dr.letters.join(" "))
                };
                let col = if dr.critical > 0 { sc.at(1.0) } else { ink().dim };
                let line = k.c.ellipsize(
                    k.f,
                    &format!("{name}  ·  {}", parts.join("  ·  ")),
                    Style::Regular,
                    11.0,
                    x1 - PAD,
                );
                k.c.text(k.f, PAD, CARD_H - 16.0, &line, Style::Regular, 11.0, col);
            }
        }
        CardKind::Network => {
            let ping = match (d.ping_ready, d.ping) {
                (true, Some(ms)) => {
                    trf("пинг {host}: {v} мс", &[("host", d.ping_host.clone()), ("v", ms.to_string())])
                }
                (true, None) => trf("{host} не отвечает", &[("host", d.ping_host.clone())]),
                _ => String::new(),
            };
            k.frame("Сеть", &ping);
            let down = [
                part("↓ ", Style::Regular, 14.0, ink().dim),
                part(fmt_speed(d.net_down), Style::SemiBold, 18.0, sc.at(0.05)),
            ];
            k.c.rich(k.f, PAD, 82.0, &down, Align::Left);
            let up = [
                part("↑ ", Style::Regular, 14.0, ink().dim),
                part(fmt_speed(d.net_up), Style::SemiBold, 18.0, sc.at(0.55)),
            ];
            k.c.rich(k.f, x1, 82.0, &up, Align::Right);
            // the last minute: download filled, upload as a line
            let (gx0, gy0, gx1, gy1) = (PAD, 98.0, x1, CARD_H - 16.0);
            k.c.rrect(gx0, gy0, gx1, gy1, 8.0, ink().hist_track);
            let hist = &d.net_hist;
            let top = hist.iter().map(|(a, b)| a.max(*b)).fold(64.0 * 1024.0, f64::max);
            if hist.len() >= 2 {
                let n = 59.0f32;
                let x = |i: usize| gx1 - 6.0 - (hist.len() - 1 - i) as f32 / n * (gx1 - gx0 - 12.0);
                let y = |v: f64| gy1 - 6.0 - (gy1 - gy0 - 12.0) * (v / top).min(1.0) as f32;
                let mut area: Vec<(f32, f32)> = vec![(x(0), gy1 - 6.0)];
                area.extend(hist.iter().enumerate().map(|(i, (dn, _))| (x(i), y(*dn))));
                area.push((x(hist.len() - 1), gy1 - 6.0));
                k.c.polygon(&area, lerp(ink().hist_track, sc.at(0.05), 0.35));
                let line: Vec<(f32, f32)> =
                    hist.iter().enumerate().map(|(i, (dn, _))| (x(i), y(*dn))).collect();
                k.c.line(&line, 1.5, sc.at(0.05));
                let line: Vec<(f32, f32)> =
                    hist.iter().enumerate().map(|(i, (_, upv))| (x(i), y(*upv))).collect();
                k.c.line(&line, 1.5, sc.at(0.55));
            }
        }
        CardKind::Power => match d.battery {
            Some((charge, on_ac)) => {
                let state = if !on_ac {
                    tr("От батареи")
                } else if charge >= 99.0 {
                    tr("От сети, заряжена")
                } else {
                    tr("Заряжается")
                };
                k.frame("Питание", &state);
                // little charge = the hot end of the scale
                k.ring(Some(charge / 100.0), &pct(charge), "%", |t| sc.at(1.0 - t));
                let left = d.battery_left.filter(|_| !on_ac).map(fmt_left);
                k.row(COL_X, x1, 92.0, "Осталось", &left.unwrap_or("—".into()), ink().text);
                let wear = d.battery_wear.map(|w| format!("{w:.0}%"));
                let wear_col = d.battery_wear.map_or(ink().faint, |w| sc.at(w / 40.0));
                k.row(COL_X, x1, 114.0, "Износ", &wear.unwrap_or("—".into()), wear_col);
                let rate = d
                    .battery_rate
                    .filter(|r| r.abs() > 0.05)
                    .map(|r| trf("{v} Вт", &[("v", dec(format!("{:.1}", r.abs())))]));
                k.row(COL_X, x1, 136.0, "Мощность", &rate.unwrap_or("—".into()), ink().text);
                if let (Some(full), Some(design)) = (d.battery_full, d.battery_design) {
                    let v = trf(
                        "{full} / {design} Вт·ч",
                        &[("full", format!("{full:.0}")), ("design", format!("{design:.0}"))],
                    );
                    k.row(COL_X, x1, 158.0, "Ёмкость", &v, ink().dim);
                }
            }
            None => {
                k.frame("Питание", &tr("От сети"));
                k.c.text(k.f, PAD, 112.0, &tr("Батареи нет"), Style::SemiBold, 20.0, ink().dim);
                k.row(PAD, x1, 150.0, "Без перезагрузки", &fmt_uptime(d.uptime_s), ink().text);
            }
        },
        CardKind::System => {
            k.frame("Система", &trf("без перезагрузки {v}", &[("v", fmt_uptime(d.uptime_s))]));
            let (t, tc) = k.temp_text(d.temp_cpu, d.temp_cpu_approx);
            k.row(PAD, x1, 80.0, "Процессор", &t, tc);
            let shared = d.gpu_shared && d.temp_gpu.is_none() && d.temp_cpu.is_some();
            let (t, tc) = k.temp_text(if shared { d.temp_cpu } else { d.temp_gpu }, shared);
            k.row(PAD, x1, 102.0, "Видеокарта", &t, tc);
            let (t, tc) = k.temp_text(d.temp_ssd, false);
            k.row(PAD, x1, 124.0, "SSD", &t, tc);
            let mut y = 146.0;
            if d.fans.is_empty() {
                let hint = d.sensor_hint.clone().unwrap_or_else(|| tr("Вентиляторы не видны"));
                let hint = k.c.ellipsize(k.f, &hint, Style::Regular, 11.0, x1 - PAD);
                k.c.text(k.f, PAD, y, &hint, Style::Regular, 11.0, ink().faint);
            } else {
                for (i, rpm) in d.fans.iter().take(3).enumerate() {
                    let label = trf("Вентилятор {n}", &[("n", (i + 1).to_string())]);
                    k.row(PAD, x1, y, &label, &trf("{v} об/мин", &[("v", format!("{rpm:.0}"))]), ink().text);
                    y += 22.0;
                }
            }
        }
    }
    Some(k.c.pm)
}

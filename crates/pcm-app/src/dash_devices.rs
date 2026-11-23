//! "Оборудование" of the PC Monitor window (beta 2, in the home page's
//! style): a card per part — processor (its cores), graphics card, memory,
//! drives (and their health), network, power, the system and its sensors.
//! Painted with egui; the old card textures (stage 5) are gone from here.

use eframe::egui;
use egui::{Align2, Color32, Painter, Pos2, Rect, Shape, Stroke, Ui, pos2, vec2};
use pcm_render::format::{dec, fmt_disk, fmt_left, fmt_mem, fmt_speed, fmt_uptime, tr, trf};
use pcm_render::theme::Scale;
use pcm_render::widget::Data;

use crate::dash_home::{bold, fit, icon, icon_font, pal, panel, rgb, text_font};
use crate::dash_kit::{GAP, bar, button_at, sparkline};
use pcm_win::diskuse::{Item, Place};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// "Что занимает диск C:": counted on a thread when asked.
#[derive(Default)]
struct DiskScan {
    running: bool,
    /// The items and the local time they were counted at ("21:04").
    done: Option<(Vec<Item>, String)>,
}

static SCAN: Mutex<DiskScan> = Mutex::new(DiskScan { running: false, done: None });
static SCAN_FILES: AtomicU64 = AtomicU64::new(0);
static SCAN_STOP: AtomicBool = AtomicBool::new(false);
/// Rows the card shows at most.
const DISK_ROWS: usize = 16;

fn start_scan() {
    let Ok(mut s) = SCAN.lock() else { return };
    if s.running {
        return;
    }
    s.running = true;
    SCAN_FILES.store(0, Ordering::Relaxed);
    let spawned = std::thread::Builder::new().name("disk use".into()).spawn(|| {
        let items = pcm_win::diskuse::scan(&SCAN_STOP, &SCAN_FILES);
        let when = pcm_win::system::local_time().get(11..16).unwrap_or("").to_string();
        if let Ok(mut s) = SCAN.lock() {
            s.done = Some((items, when));
            s.running = false;
        }
    });
    if spawned.is_err() {
        s.running = false;
    }
}

/// "12,4 ГБ" or "850 МБ".
fn size(bytes: u64) -> String {
    let gb = bytes as f64 / (1u64 << 30) as f64;
    if gb >= 1.0 {
        trf("{v} ГБ", &[("v", dec(format!("{gb:.1}")))])
    } else {
        trf("{v} МБ", &[("v", format!("{:.0}", bytes as f64 / 1048576.0))])
    }
}

/// The card: a button that counts, then the biggest places with "Открыть".
fn disk_use(ui: &mut Ui, w: f32) {
    let (running, done) = match SCAN.lock() {
        Ok(s) => (s.running, s.done.clone()),
        Err(_) => return,
    };
    let rows: Vec<Item> = done
        .as_ref()
        .map(|(items, _)| items.iter().filter(|i| i.bytes >= 1 << 20).take(DISK_ROWS).cloned().collect())
        .unwrap_or_default();
    let row_h = 36.0;
    let h = 76.0 + rows.len() as f32 * row_h + if rows.is_empty() { 0.0 } else { 8.0 };
    let (r, _) = ui.allocate_exact_size(vec2(w, h), egui::Sense::hover());
    // test runs: count right away and show the card (PCM_DISK_SCAN=1)
    if std::env::var_os("PCM_DISK_SCAN").is_some() {
        if !running && done.is_none() {
            start_scan();
        }
        ui.scroll_to_rect(r, Some(egui::Align::BOTTOM));
    }
    let p = ui.painter().clone();
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    let sub = if running {
        trf("Считаю… файлов: {n}", &[("n", SCAN_FILES.load(Ordering::Relaxed).to_string())])
    } else if let Some((_, when)) = &done {
        trf("Посчитано в {time}", &[("time", when.clone())])
    } else {
        tr("Нажми «Посчитать»: это займёт до минуты")
    };
    head(&p, r, icon::DISK, &trf("Что занимает диск {d}", &[("d", drive)]), &sub);
    let label = if done.is_some() { tr("Посчитать заново") } else { tr("Посчитать") };
    let bw = p.layout_no_wrap(label.clone(), bold(13.0), pal().text).size().x + 32.0;
    let b = Rect::from_min_size(pos2(r.right() - 18.0 - bw, r.top() + 18.0), vec2(bw, 32.0));
    if running {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(300));
    } else if button_at(ui, b, &label, done.is_none(), false, "disk-scan") {
        start_scan();
    }
    let max = rows.first().map_or(1, |i| i.bytes.max(1));
    let (x0, x1) = (r.left() + 18.0, r.right() - 18.0);
    let open_w = 96.0;
    let size_x = x1 - open_w - 16.0; // right edge of the size
    let bar_w = ((x1 - x0) * 0.22).clamp(80.0, 220.0);
    let bar_x = size_x - 84.0 - bar_w;
    let mut y = r.top() + 72.0;
    for (k, it) in rows.iter().enumerate() {
        let cy = y + row_h / 2.0;
        let name = if it.named { tr(&it.name) } else { it.name.clone() };
        p.text(
            pos2(x0, cy),
            Align2::LEFT_CENTER,
            fit(&p, &name, &text_font(13.0), bar_x - x0 - 16.0),
            text_font(13.0),
            pal().text,
        );
        bar(
            &p,
            Rect::from_min_size(pos2(bar_x, cy - 2.5), vec2(bar_w, 5.0)),
            it.bytes as f32 / max as f32,
            pal().gpu_line,
        );
        p.text(pos2(size_x, cy), Align2::RIGHT_CENTER, size(it.bytes), bold(13.0), pal().text);
        let ob = Rect::from_min_size(pos2(x1 - open_w, cy - 14.0), vec2(open_w, 28.0));
        match it.place {
            Place::Folder | Place::RecycleBin => {
                if button_at(ui, ob, &tr("Открыть"), false, false, &format!("disk-open-{k}")) {
                    pcm_win::autostart::open(&it.path.to_string_lossy());
                }
            }
            Place::SystemFile => {
                let tip = if it.path.to_string_lossy().to_lowercase().ends_with("hiberfil.sys") {
                    tr("Если гибернация не нужна, её выключает команда powercfg /h off (от администратора)")
                } else {
                    tr("Размер задаётся в «Параметры быстродействия → Дополнительно → Виртуальная память»")
                };
                let t =
                    p.text(ob.center(), Align2::CENTER_CENTER, tr("системный"), text_font(12.0), pal().faint);
                ui.interact(t.expand(6.0), ui.scope_id().with(("disk-tip", k)), egui::Sense::hover())
                    .on_hover_text(tip);
            }
        }
        y += row_h;
    }
}

/// A card's head: the icon in its box, the title and a line under it.
fn head(p: &Painter, r: Rect, glyph: &str, title: &str, sub: &str) {
    panel(p, r);
    let ib = Rect::from_min_size(pos2(r.left() + 18.0, r.top() + 16.0), vec2(34.0, 34.0));
    p.rect_filled(ib, 9.0, pal().card_hi);
    p.text(ib.center(), Align2::CENTER_CENTER, glyph, icon_font(16.0), pal().dim);
    let tx = ib.right() + 12.0;
    let room = r.right() - 18.0 - tx;
    p.text(
        pos2(tx, r.top() + 16.0),
        Align2::LEFT_TOP,
        fit(p, title, &bold(14.5), room),
        bold(14.5),
        pal().text,
    );
    if !sub.is_empty() {
        p.text(
            pos2(tx, r.top() + 36.0),
            Align2::LEFT_TOP,
            fit(p, sub, &text_font(12.0), room),
            text_font(12.0),
            pal().dim,
        );
    }
}

/// A ring: the track, the value's arc from the top, the number inside.
fn ring(p: &Painter, c: Pos2, rad: f32, t: Option<f32>, color: Color32, big: &str, unit: &str) {
    p.circle_stroke(c, rad, Stroke::new(8.0, pal().grid));
    if let Some(t) = t.map(|t| t.clamp(0.0, 1.0)).filter(|t| *t > 0.003) {
        let steps = ((t * 120.0).ceil() as usize).max(2);
        let pts: Vec<Pos2> = (0..=steps)
            .map(|i| {
                let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * t * i as f32 / steps as f32;
                pos2(c.x + rad * a.cos(), c.y + rad * a.sin())
            })
            .collect();
        p.add(Shape::line(pts, Stroke::new(8.0, color)));
    }
    let g = p.layout_no_wrap(big.to_string(), bold(26.0), color);
    let u = p.layout_no_wrap(unit.to_string(), text_font(12.0), pal().dim);
    let w = g.size().x + u.size().x + 2.0;
    let x = c.x - w / 2.0;
    let gh = g.size().y;
    p.galley(pos2(x, c.y - gh / 2.0), g, color);
    p.text(pos2(x + w, c.y + gh / 2.0 - 7.0), Align2::RIGHT_BOTTOM, unit, text_font(12.0), pal().dim);
}

/// "label ........ value" from x0 to x1.
fn kv(p: &Painter, x0: f32, x1: f32, y: f32, label: &str, value: &str, color: Color32) {
    p.text(pos2(x0, y), Align2::LEFT_TOP, label, text_font(12.5), pal().dim);
    let lw = p.layout_no_wrap(label.to_string(), text_font(12.5), pal().dim).size().x;
    let room = (x1 - x0 - lw - 12.0).max(20.0);
    p.text(pos2(x1, y), Align2::RIGHT_TOP, fit(p, value, &bold(12.5), room), bold(12.5), color);
}

/// A note in the card's empty space ("Батареи нет", a sensor hint).
fn note(p: &Painter, x: f32, y: f32, w: f32, text: &str) -> f32 {
    let g = p.layout(text.to_string(), text_font(12.5), pal().dim, w);
    let h = g.size().y;
    p.galley(pos2(x, y), g, pal().dim);
    h
}

fn gb(b: u64) -> String {
    dec(format!("{:.1}", b as f64 / (1u64 << 30) as f64))
}

fn temp(t: Option<f32>, approx: bool, sc: &Scale) -> (String, Color32) {
    match t {
        Some(t) => (format!("{}{t:.0}°C", if approx { "≈" } else { "" }), rgb(sc.heat(t))),
        None => ("—".into(), pal().faint),
    }
}

/// The cards' heights for this data (drives and fans make them grow).
fn heights(d: &Data) -> [f32; 7] {
    let drives = d.disks.len() as f32 * 46.0 + d.drives.len() as f32 * 44.0;
    let fans = d.fans.len().min(4) as f32 * 24.0;
    [270.0, 270.0, 240.0, (100.0 + drives).max(240.0), 240.0, 240.0, 220.0 + fans]
}

pub fn ui(ui: &mut Ui, d: Option<&Data>, sc: Option<&Scale>) {
    let (Some(d), Some(sc)) = (d, sc) else {
        ui.label(egui::RichText::new(tr("Жду данные от виджета…")).color(pal().dim));
        return;
    };
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let w = ui.available_width();
        let cols = if w >= 980.0 { 2 } else { 1 };
        let cw = (w - GAP * (cols - 1) as f32) / cols as f32;
        let hs = heights(d);
        let painters: [fn(&Painter, Rect, &Data, &Scale); 7] =
            [cpu, gpu, memory, drives, network, power, system];
        for (row, idx) in (0..7).collect::<Vec<_>>().chunks(cols).enumerate() {
            if row > 0 {
                ui.add_space(GAP);
            }
            let h = idx.iter().map(|&i| hs[i]).fold(0.0, f32::max);
            let (r, _) = ui.allocate_exact_size(vec2(w, h), egui::Sense::hover());
            let p = ui.painter();
            for (k, &i) in idx.iter().enumerate() {
                let cell = Rect::from_min_size(pos2(r.left() + k as f32 * (cw + GAP), r.top()), vec2(cw, h));
                painters[i](p, cell, d, sc);
            }
        }
        ui.add_space(GAP);
        disk_use(ui, w);
        ui.add_space(8.0);
    });
}

fn cpu(p: &Painter, r: Rect, d: &Data, sc: &Scale) {
    let mut sub = Vec::new();
    if let Some(c) = d.cpu_cores {
        sub.push(trf("ядер: {n}", &[("n", c.to_string())]));
    }
    if !d.cores.is_empty() {
        sub.push(trf("потоков: {n}", &[("n", d.cores.len().to_string())]));
    }
    let name = if d.cpu_name.is_empty() { tr("Процессор") } else { d.cpu_name.clone() };
    head(p, r, icon::CPU, &name, &sub.join(" · "));
    let col = rgb(sc.at(d.cpu / 100.0));
    let c = pos2(r.left() + 18.0 + 52.0, r.top() + 72.0 + 52.0);
    ring(p, c, 46.0, Some(d.cpu / 100.0), col, &format!("{:.0}", d.cpu), "%");
    // beside the ring; not across a wide card
    let (x0, x1) = (r.left() + 150.0, (r.right() - 18.0).min(r.left() + 470.0));
    let mut y = r.top() + 80.0;
    let clock = d.cpu_clock_ghz.map_or("—".into(), |g| trf("{v} ГГц", &[("v", dec(format!("{g:.2}")))]));
    kv(p, x0, x1, y, &tr("Частота"), &clock, pal().text);
    y += 26.0;
    let (t, tc) = temp(d.temp_cpu, d.temp_cpu_approx, sc);
    kv(p, x0, x1, y, &tr("Температура"), &t, tc);
    y += 26.0;
    let power = d.cpu_power_w.map_or("—".into(), |w| trf("{v} Вт", &[("v", dec(format!("{w:.1}")))]));
    kv(p, x0, x1, y, &tr("Мощность"), &power, pal().text);
    // the threads' load as bars along the bottom
    if !d.cores.is_empty() {
        let top = r.bottom() - 70.0;
        p.text(pos2(r.left() + 18.0, top - 2.0), Align2::LEFT_BOTTOM, tr("Ядра"), text_font(12.0), pal().dim);
        let max = d.cores.iter().copied().fold(0.0, f32::max);
        p.text(
            pos2(r.right() - 18.0, top - 2.0),
            Align2::RIGHT_BOTTOM,
            trf("макс {v}%", &[("v", format!("{max:.0}"))]),
            text_font(12.0),
            pal().dim,
        );
        let n = d.cores.len() as f32;
        let area =
            Rect::from_min_max(pos2(r.left() + 18.0, top + 6.0), pos2(r.right() - 18.0, r.bottom() - 18.0));
        let step = area.width() / n;
        for (i, v) in d.cores.iter().enumerate() {
            let x = area.left() + i as f32 * step;
            let col = Rect::from_min_max(pos2(x + 1.5, area.top()), pos2(x + step - 1.5, area.bottom()));
            p.rect_filled(col, 3.0, pal().grid);
            let h = (col.height() * (v / 100.0).clamp(0.0, 1.0)).max(2.0);
            p.rect_filled(
                Rect::from_min_max(pos2(col.left(), col.bottom() - h), col.max),
                3.0,
                rgb(sc.at(v / 100.0)),
            );
        }
    }
}

fn gpu(p: &Painter, r: Rect, d: &Data, sc: &Scale) {
    let name = d.gpu_name.clone().unwrap_or_else(|| tr("Видеокарта"));
    let sub = if d.gpu_shared { tr("встроенная") } else { tr("отдельная") };
    head(p, r, icon::GPU, &name, &sub);
    let t = d.gpu_load.map(|l| l / 100.0);
    let col = rgb(sc.at(t.unwrap_or(0.0)));
    let c = pos2(r.left() + 18.0 + 52.0, r.top() + 72.0 + 52.0);
    ring(
        p,
        c,
        46.0,
        t,
        col,
        &d.gpu_load.map_or("—".into(), |l| format!("{l:.0}")),
        if t.is_some() { "%" } else { "" },
    );
    // beside the ring; not across a wide card
    let (x0, x1) = (r.left() + 150.0, (r.right() - 18.0).min(r.left() + 470.0));
    let mut y = r.top() + 80.0;
    let (tt, tc) = match (d.temp_gpu, d.gpu_shared) {
        (Some(t), _) => temp(Some(t), false, sc),
        (None, true) => temp(d.temp_cpu, true, sc),
        _ => temp(None, false, sc),
    };
    kv(p, x0, x1, y, &tr("Температура"), &tt, tc);
    y += 26.0;
    let clock = d.gpu_clock.map_or("—".into(), |c| trf("{v} МГц", &[("v", format!("{c:.0}"))]));
    kv(p, x0, x1, y, &tr("Частота"), &clock, pal().text);
    y += 26.0;
    let mb = |v: f32| fmt_mem((v as f64 * 1048576.0) as u64);
    match (d.vram_used_mb, d.vram_total_mb) {
        (Some(u), Some(t)) if t > 0.0 => {
            kv(p, x0, x1, y, "VRAM", &format!("{} / {}", mb(u), mb(t)), pal().text);
            bar(p, Rect::from_min_size(pos2(x0, y + 24.0), vec2(x1 - x0, 6.0)), u / t, rgb(sc.at(u / t)));
        }
        (Some(u), _) => kv(p, x0, x1, y, "VRAM", &mb(u), pal().text),
        _ => {}
    }
    // the last minute of its load
    let spark = Rect::from_min_max(
        pos2(r.left() + 18.0, r.bottom() - 54.0),
        pos2(r.right() - 18.0, r.bottom() - 18.0),
    );
    p.text(
        pos2(spark.left(), spark.top() - 6.0),
        Align2::LEFT_BOTTOM,
        tr("Последняя минута"),
        text_font(12.0),
        pal().dim,
    );
    sparkline(p, spark, &d.gpu_hist, col);
}

fn memory(p: &Painter, r: Rect, d: &Data, sc: &Scale) {
    let mut sub = String::new();
    if let Some(k) = &d.mem_kind {
        sub = match d.mem_speed {
            Some(s) => format!("{k}-{s}"),
            None => k.clone(),
        };
    }
    if !d.mem_sticks.is_empty() {
        let first = d.mem_sticks[0];
        let size = |b: u64| trf("{v} ГБ", &[("v", (b >> 30).to_string())]);
        let sticks = if d.mem_sticks.len() > 1 && d.mem_sticks.iter().all(|s| *s == first) {
            format!("{} × {}", d.mem_sticks.len(), size(first))
        } else {
            d.mem_sticks.iter().map(|s| size(*s)).collect::<Vec<_>>().join(" + ")
        };
        sub = if sub.is_empty() { sticks } else { format!("{sub} · {sticks}") };
    }
    head(p, r, icon::RAM, &tr("Память"), &sub);
    let col = rgb(sc.at(d.ram_pct / 100.0));
    let c = pos2(r.left() + 18.0 + 52.0, r.top() + 72.0 + 52.0);
    ring(p, c, 46.0, Some(d.ram_pct / 100.0), col, &format!("{:.0}", d.ram_pct), "%");
    // beside the ring; not across a wide card
    let (x0, x1) = (r.left() + 150.0, (r.right() - 18.0).min(r.left() + 470.0));
    let mut y = r.top() + 80.0;
    kv(p, x0, x1, y, &tr("Занято"), &trf("{v} ГБ", &[("v", gb(d.ram_used))]), pal().text);
    y += 26.0;
    kv(
        p,
        x0,
        x1,
        y,
        &tr("Свободно"),
        &trf("{v} ГБ", &[("v", gb(d.ram_total.saturating_sub(d.ram_used)))]),
        pal().text,
    );
    y += 26.0;
    kv(p, x0, x1, y, &tr("Всего"), &trf("{v} ГБ", &[("v", gb(d.ram_total))]), pal().text);
    let spark = Rect::from_min_max(
        pos2(r.left() + 18.0, r.bottom() - 46.0),
        pos2(r.right() - 18.0, r.bottom() - 18.0),
    );
    sparkline(p, spark, &d.ram_hist, col);
}

fn drives(p: &Painter, r: Rect, d: &Data, sc: &Scale) {
    let sub =
        format!("{} {} · {} {}", tr("чтение"), fmt_speed(d.io_read), tr("запись"), fmt_speed(d.io_write));
    head(p, r, icon::DISK, &tr("Диски"), &sub);
    let (x0, x1) = (r.left() + 18.0, r.right() - 18.0);
    let mut y = r.top() + 70.0;
    for (letter, used, total, pct) in &d.disks {
        let col = rgb(sc.at(pct / 100.0));
        p.text(pos2(x0, y), Align2::LEFT_TOP, letter, bold(13.0), pal().text);
        p.text(
            pos2(x1, y),
            Align2::RIGHT_TOP,
            format!("{} · {pct:.0}%", fmt_disk(*used, *total)),
            text_font(12.5),
            pal().dim,
        );
        bar(p, Rect::from_min_size(pos2(x0, y + 22.0), vec2(x1 - x0, 6.0)), pct / 100.0, col);
        y += 46.0;
    }
    for x in &d.drives {
        y += 4.0;
        p.hline(x0..=x1, y - 4.0, Stroke::new(1.0, pal().grid));
        let name = if x.model.is_empty() {
            x.letters.join(" ")
        } else {
            format!("{} ({})", x.model, x.letters.join(" "))
        };
        p.text(
            pos2(x0, y + 4.0),
            Align2::LEFT_TOP,
            fit(p, &name, &bold(12.5), x1 - x0),
            bold(12.5),
            pal().text,
        );
        let health = 100u8.saturating_sub(x.wear);
        let mut parts = vec![trf("здоровье {v}%", &[("v", health.to_string())])];
        if let Some(t) = x.temp {
            parts.push(format!("{t:.0}°C"));
        }
        if let Some(w) = x.written {
            parts.push(trf("записано {v}", &[("v", fmt_tb(w))]));
        }
        if x.critical > 0 {
            parts.push(tr("есть предупреждения"));
        }
        let col = if x.critical > 0 || health < 50 { pal().hot } else { pal().good };
        p.text(pos2(x0, y + 23.0), Align2::LEFT_TOP, parts.join(" · "), text_font(12.0), col);
        y += 44.0;
    }
}

fn fmt_tb(bytes: u64) -> String {
    let tb = bytes as f64 / 1e12;
    if tb >= 1.0 {
        trf("{v} ТБ", &[("v", dec(format!("{tb:.1}")))])
    } else {
        trf("{v} ГБ", &[("v", format!("{:.0}", bytes as f64 / 1e9))])
    }
}

fn network(p: &Painter, r: Rect, d: &Data, sc: &Scale) {
    let sub = d.net_ip.clone().unwrap_or_default();
    head(p, r, icon::NET, &tr("Сеть"), &sub);
    let (x0, x1) = (r.left() + 18.0, r.right() - 18.0);
    let cw = (x1 - x0) / 3.0;
    let ping = match (d.ping_ready, d.ping) {
        (false, _) => ("…".to_string(), pal().dim),
        (true, Some(ms)) => (trf("{v} мс", &[("v", ms.to_string())]), rgb(sc.ping(Some(ms)))),
        (true, None) => ("—".to_string(), pal().hot),
    };
    for (i, (label, value, col)) in [
        (tr("Загрузка ↓"), fmt_speed(d.net_down), pal().text),
        (tr("Отдача ↑"), fmt_speed(d.net_up), pal().text),
        (trf("Пинг {host}", &[("host", d.ping_host.clone())]), ping.0, ping.1),
    ]
    .into_iter()
    .enumerate()
    {
        let x = x0 + i as f32 * cw;
        p.text(
            pos2(x, r.top() + 70.0),
            Align2::LEFT_TOP,
            fit(p, &label, &text_font(12.0), cw - 6.0),
            text_font(12.0),
            pal().dim,
        );
        p.text(
            pos2(x, r.top() + 88.0),
            Align2::LEFT_TOP,
            fit(p, &value, &bold(18.0), cw - 6.0),
            bold(18.0),
            col,
        );
    }
    let g = Rect::from_min_max(pos2(x0, r.top() + 150.0), pos2(x1, r.bottom() - 18.0));
    let down: Vec<f32> = d.net_hist.iter().map(|v| v.0 as f32).collect();
    let up: Vec<f32> = d.net_hist.iter().map(|v| v.1 as f32).collect();
    p.text(
        pos2(g.left(), g.top() - 4.0),
        Align2::LEFT_BOTTOM,
        tr("Последняя минута"),
        text_font(12.0),
        pal().dim,
    );
    sparkline(p, g, &down, pal().cpu_line);
    sparkline(p, g, &up, pal().good);
}

fn power(p: &Painter, r: Rect, d: &Data, sc: &Scale) {
    let Some((pct, plugged)) = d.battery else {
        head(p, r, icon::POWER, &tr("Питание"), &tr("От сети"));
        let (x0, x1) = (r.left() + 18.0, r.right() - 18.0);
        let h = note(
            p,
            x0,
            r.top() + 72.0,
            x1 - x0,
            &tr("Батареи нет: это настольный компьютер или батарея не видна Windows."),
        );
        kv(p, x0, x1, r.top() + 84.0 + h, &tr("Без перезагрузки"), &fmt_uptime(d.uptime_s), pal().text);
        return;
    };
    let sub = if plugged { tr("Заряжается от сети") } else { tr("От батареи") };
    head(p, r, icon::POWER, &tr("Батарея"), &sub);
    let col = rgb(sc.at(1.0 - pct / 100.0));
    let c = pos2(r.left() + 18.0 + 52.0, r.top() + 72.0 + 52.0);
    ring(p, c, 46.0, Some(pct / 100.0), col, &format!("{pct:.0}"), "%");
    // beside the ring; not across a wide card
    let (x0, x1) = (r.left() + 150.0, (r.right() - 18.0).min(r.left() + 470.0));
    let mut y = r.top() + 80.0;
    if let Some(left) = d.battery_left {
        kv(p, x0, x1, y, &tr("Осталось"), &fmt_left(left), pal().text);
        y += 26.0;
    }
    if let Some(w) = d.battery_wear {
        kv(p, x0, x1, y, &tr("Износ"), &format!("{w:.0}%"), rgb(sc.at(w / 100.0)));
        y += 26.0;
    }
    if let (Some(full), Some(design)) = (d.battery_full, d.battery_design) {
        kv(
            p,
            x0,
            x1,
            y,
            &tr("Ёмкость"),
            &trf("{a} из {b} Вт·ч", &[("a", dec(format!("{full:.1}"))), ("b", dec(format!("{design:.1}")))]),
            pal().text,
        );
        y += 26.0;
    }
    if let Some(rate) = d.battery_rate.filter(|r| r.abs() > 0.05) {
        kv(
            p,
            x0,
            x1,
            y,
            &tr("Мощность"),
            &trf("{v} Вт", &[("v", dec(format!("{:.1}", rate.abs())))]),
            pal().text,
        );
    }
}

fn system(p: &Painter, r: Rect, d: &Data, sc: &Scale) {
    head(p, r, icon::DEVICES, &tr("Система"), &d.windows);
    let (x0, x1) = (r.left() + 18.0, r.right() - 18.0);
    let mut y = r.top() + 72.0;
    kv(p, x0, x1, y, &tr("Без перезагрузки"), &fmt_uptime(d.uptime_s), pal().text);
    y += 26.0;
    let (s, sc_col) = if d.sensors_online {
        (tr("отвечают"), pal().good)
    } else {
        (tr("не отвечают"), pal().warn)
    };
    kv(p, x0, x1, y, &tr("Датчики"), &s, sc_col);
    y += 26.0;
    let (pw, pc) = if d.pawnio {
        (tr("установлен"), pal().good)
    } else {
        (tr("не установлен"), pal().dim)
    };
    kv(p, x0, x1, y, "PawnIO", &pw, pc);
    y += 26.0;
    let (t, tc) = temp(d.temp_ssd, false, sc);
    kv(p, x0, x1, y, "SSD", &t, tc);
    y += 26.0;
    if let Some(h) = d.sensor_hint.as_ref().filter(|_| !d.sensors_online) {
        y += 4.0;
        y += note(p, x0, y, x1 - x0, h) + 6.0;
    }
    for (i, rpm) in d.fans.iter().take(4).enumerate() {
        let name = d
            .fan_names
            .get(i)
            .cloned()
            .unwrap_or_else(|| trf("Вентилятор {n}", &[("n", (i + 1).to_string())]));
        kv(p, x0, x1, y, &name, &trf("{v} об/мин", &[("v", format!("{rpm:.0}"))]), pal().text);
        y += 24.0;
    }
}

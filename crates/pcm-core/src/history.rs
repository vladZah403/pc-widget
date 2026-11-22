//! The history (stage 5): a point every STEP_S seconds, kept KEEP_DAYS days
//! in SQLite (1.8 kept a day in history.csv, which is imported once). The
//! widget writes; the PC Monitor window reads the same file (WAL lets both).

use std::path::Path;

use rusqlite::{Connection, OpenFlags, params};

pub const STEP_S: i64 = 10;
pub const KEEP_DAYS: i64 = 30;
pub const FILE: &str = "history.db";
/// 1.8's file, imported into the database the first time.
pub const CSV_1_8: &str = "history.csv";

/// One point: unix seconds and the readings (averages over the step).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub t: i64,
    pub cpu: f32,
    pub cpu_t: Option<f32>,
    pub gpu_t: Option<f32>,
    pub gpu: Option<f32>,
    pub ram: Option<f32>,
    /// Bytes per second.
    pub down: Option<f64>,
    pub up: Option<f64>,
}

/// The columns a chart can show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    Cpu,
    CpuTemp,
    GpuTemp,
    Gpu,
    Ram,
    Down,
    Up,
}

impl Point {
    pub fn get(&self, c: Column) -> Option<f64> {
        match c {
            Column::Cpu => Some(self.cpu as f64),
            Column::CpuTemp => self.cpu_t.map(f64::from),
            Column::GpuTemp => self.gpu_t.map(f64::from),
            Column::Gpu => self.gpu.map(f64::from),
            Column::Ram => self.ram.map(f64::from),
            Column::Down => self.down,
            Column::Up => self.up,
        }
    }
}

pub struct History {
    db: Connection,
}

fn finite(v: Option<f64>) -> Option<f64> {
    v.filter(|v| v.is_finite())
}

impl History {
    /// Open (or make) the database for writing.
    pub fn open(path: &Path) -> rusqlite::Result<History> {
        let db = Connection::open(path)?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "NORMAL")?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS points (
                t INTEGER PRIMARY KEY, cpu REAL NOT NULL, cpu_t REAL, gpu_t REAL,
                gpu REAL, ram REAL, down REAL, up REAL)",
        )?;
        Ok(History { db })
    }

    /// Open for reading only (the window, while the widget writes).
    pub fn open_read(path: &Path) -> rusqlite::Result<History> {
        let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        Ok(History { db })
    }

    pub fn in_memory() -> rusqlite::Result<History> {
        let db = Connection::open_in_memory()?;
        db.execute_batch(
            "CREATE TABLE points (t INTEGER PRIMARY KEY, cpu REAL NOT NULL, cpu_t REAL, gpu_t REAL,
                gpu REAL, ram REAL, down REAL, up REAL)",
        )?;
        Ok(History { db })
    }

    pub fn add(&self, p: &Point) -> rusqlite::Result<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO points VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![p.t, p.cpu, p.cpu_t, p.gpu_t, p.gpu, p.ram, finite(p.down), finite(p.up)],
        )?;
        Ok(())
    }

    /// Forget points older than KEEP_DAYS before `now`. Returns how many went.
    pub fn trim(&self, now: i64) -> rusqlite::Result<usize> {
        self.db.execute("DELETE FROM points WHERE t < ?1", params![now - KEEP_DAYS * 86400])
    }

    /// Points from `from` to `to` (unix seconds), oldest first.
    pub fn range(&self, from: i64, to: i64) -> rusqlite::Result<Vec<Point>> {
        let mut q = self.db.prepare_cached(
            "SELECT t, cpu, cpu_t, gpu_t, gpu, ram, down, up FROM points WHERE t >= ?1 AND t <= ?2 ORDER BY t",
        )?;
        let rows = q.query_map(params![from, to], |r| {
            Ok(Point {
                t: r.get(0)?,
                cpu: r.get(1)?,
                cpu_t: r.get(2)?,
                gpu_t: r.get(3)?,
                gpu: r.get(4)?,
                ram: r.get(5)?,
                down: r.get(6)?,
                up: r.get(7)?,
            })
        })?;
        rows.collect()
    }

    pub fn count(&self) -> rusqlite::Result<i64> {
        self.db.query_row("SELECT COUNT(*) FROM points", [], |r| r.get(0))
    }

    /// 1.8's history.csv ("t,cpu,cpu_t,gpu_t[,gpu,ram,down,up]", empty = none).
    /// Points already there stay. Returns how many lines were taken in.
    pub fn import_csv_1_8(&mut self, text: &str) -> rusqlite::Result<usize> {
        let num = |s: &str| s.trim().parse::<f64>().ok().filter(|v| v.is_finite());
        let tx = self.db.transaction()?;
        let mut n = 0;
        {
            let mut q = tx.prepare("INSERT OR IGNORE INTO points VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)")?;
            for line in text.lines() {
                let f: Vec<&str> = line.split(',').collect();
                if f.len() != 4 && f.len() != 8 {
                    continue;
                }
                let (Some(t), Some(cpu)) = (num(f[0]), num(f[1])) else { continue };
                let get = |i: usize| f.get(i).and_then(|s| num(s));
                let f32_ = |i: usize| get(i).map(|v| v as f32);
                n += q.execute(params![
                    t as i64,
                    cpu as f32,
                    f32_(2),
                    f32_(3),
                    f32_(4),
                    f32_(5),
                    get(6),
                    get(7)
                ])?;
            }
        }
        tx.commit()?;
        Ok(n)
    }
}

/// Average and peak of a column in `n` equal slices of [start, end]
/// (None = no data there), 1.8's history_buckets.
pub fn buckets(points: &[Point], start: i64, end: i64, n: usize, col: Column) -> Vec<Option<(f64, f64)>> {
    let mut sums = vec![0.0; n];
    let mut counts = vec![0usize; n];
    let mut peaks = vec![f64::MIN; n];
    let span = (end - start).max(1) as f64 / n as f64;
    for p in points {
        let Some(v) = p.get(col) else { continue };
        if p.t < start || p.t > end {
            continue;
        }
        let i = (((p.t - start) as f64 / span) as usize).min(n - 1);
        sums[i] += v;
        counts[i] += 1;
        peaks[i] = peaks[i].max(v);
    }
    (0..n).map(|i| (counts[i] > 0).then(|| (sums[i] / counts[i] as f64, peaks[i]))).collect()
}

/// CSV for Excel like 1.8's write_history_csv: local time, ';' and decimal
/// commas unless English. `local` turns unix seconds into "2026-11-22 13:05:41".
pub fn to_csv(points: &[Point], english: bool, local: impl Fn(i64) -> String) -> String {
    let sep = if english { "," } else { ";" };
    let num = |v: Option<f64>| match v {
        Some(v) if english => format!("{v:.1}"),
        Some(v) => format!("{v:.1}").replace('.', ","),
        None => String::new(),
    };
    let mut out = String::from("\u{feff}");
    out += &[
        "time",
        "cpu_load_%",
        "cpu_temp_C",
        "gpu_temp_C",
        "gpu_load_%",
        "ram_%",
        "download_bytes_s",
        "upload_bytes_s",
    ]
    .join(sep);
    out += "\r\n";
    for p in points {
        let int = |v: Option<f64>| v.map_or(String::new(), |v| format!("{:.0}", v.max(0.0)));
        let cells = [
            local(p.t),
            num(Some(p.cpu as f64)),
            num(p.cpu_t.map(f64::from)),
            num(p.gpu_t.map(f64::from)),
            num(p.gpu.map(f64::from)),
            num(p.ram.map(f64::from)),
            int(p.down),
            int(p.up),
        ];
        out += &cells.join(sep);
        out += "\r\n";
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(t: i64, cpu: f32) -> Point {
        Point { t, cpu, ram: Some(50.0), ..Point::default() }
    }

    #[test]
    fn store_range_trim() {
        let h = History::in_memory().unwrap();
        let now = 2_000_000_000;
        for i in 0..10 {
            h.add(&point(now - i * 10, i as f32)).unwrap();
        }
        h.add(&point(now - 40 * 86400, 1.0)).unwrap();
        assert_eq!(h.count().unwrap(), 11);
        assert_eq!(h.trim(now).unwrap(), 1, "older than 30 days");
        let r = h.range(now - 30, now).unwrap();
        assert_eq!(r.iter().map(|p| p.cpu).collect::<Vec<_>>(), vec![3.0, 2.0, 1.0, 0.0]);
        h.add(&point(now, 99.0)).unwrap();
        assert_eq!(h.range(now, now).unwrap()[0].cpu, 99.0, "the same second replaced");
    }

    #[test]
    fn import_1_8_csv() {
        let mut h = History::in_memory().unwrap();
        let csv = "1763800000,12.5,55.0,\n1763800010,20.0,56.0,48.0,30.0,61.2,1024.0,512.0\nbad line\n\
                   1763800020,x,1,2\n";
        assert_eq!(h.import_csv_1_8(csv).unwrap(), 2);
        let r = h.range(0, i64::MAX).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!((r[0].cpu, r[0].cpu_t, r[0].gpu_t, r[0].gpu), (12.5, Some(55.0), None, None));
        assert_eq!((r[1].ram, r[1].down, r[1].up), (Some(61.2), Some(1024.0), Some(512.0)));
        assert_eq!(h.import_csv_1_8(csv).unwrap(), 0, "already there");
    }

    #[test]
    fn buckets_and_csv() {
        let pts = [point(0, 10.0), point(5, 30.0), point(60, 50.0)];
        let b = buckets(&pts, 0, 100, 2, Column::Cpu);
        assert_eq!(b, vec![Some((20.0, 30.0)), Some((50.0, 50.0))]);
        assert_eq!(buckets(&pts, 0, 100, 2, Column::Gpu), vec![None, None]);
        let csv = to_csv(&pts[..1], false, |t| format!("T{t}"));
        assert!(csv.contains("\r\nT0;10,0;;;;50,0;;\r\n"), "{csv:?}");
    }
}

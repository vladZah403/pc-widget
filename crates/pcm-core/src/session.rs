//! Game sessions: every frame of one game run and the once-a-second
//! readings beside it, the summary a report shows, and the file they are
//! kept in: zstd over a JSON header line, then the frame times as
//! little-endian u32 microseconds.

use std::io::{Read, Write};

use serde::{Deserialize, Serialize};

use crate::frames::FRAME_PAUSE_S;

pub const VERSION: u32 = 1;
pub const EXTENSION: &str = "pcms";
/// A run with less play than this is not kept (a launcher, a menu, a false start).
pub const MIN_ACTIVE_S: f64 = 30.0;
/// A frame this many times longer than the median is a "bad" one (a stutter).
pub const BAD_FRAME: f32 = 2.0;

/// The readings of one second.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sample {
    /// Seconds from the session start.
    pub t: f32,
    /// Frames in the second before `t` (from the frames themselves).
    pub fps: Option<f32>,
    pub cpu: Option<f32>,
    pub gpu: Option<f32>,
    pub cpu_t: Option<f32>,
    pub gpu_t: Option<f32>,
    pub ram: Option<f32>,
    /// Windows' "% Performance Limit" of the processor, only when it held
    /// it back (below 99.5): see throttle.rs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<f32>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Summary {
    pub frames: u64,
    /// Seconds of play: frame gaps of FRAME_PAUSE_S+ (alt-tab, loading) left out.
    pub active_s: f64,
    pub avg_fps: f64,
    /// The slowest and the fastest full second.
    pub min_fps: f64,
    pub max_fps: f64,
    /// 1 / the 99th and the 99.9th percentile frame time.
    pub low1: f64,
    pub low01: f64,
    pub median_ms: f64,
    /// Frames BAD_FRAME times longer than the median.
    pub bad_frames: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionInfo {
    pub version: u32,
    /// The game's program name ("civilizationvi").
    pub game: String,
    /// Its window title.
    pub title: String,
    /// Local time of the start, "2026-11-22 13:05:41".
    pub started: String,
    /// From the first frame to the last, pauses included.
    pub duration_s: f64,
    /// Where the frames came from: "dxgi" or "krnl".
    pub source: String,
    /// The game window's inner size and its monitor's refresh rate.
    pub width: u32,
    pub height: u32,
    pub refresh_hz: u32,
    pub cpu: String,
    pub gpu: String,
    pub ram_gb: f32,
    pub windows: String,
    pub summary: Summary,
    pub samples: Vec<Sample>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Session {
    pub info: SessionInfo,
    /// Every frame time in ms, in order (pauses as they were).
    pub frame_ms: Vec<f32>,
}

/// Frame times (ms) from present timestamps (s).
pub fn frame_times(times: &[f64]) -> Vec<f32> {
    times.windows(2).map(|w| ((w[1] - w[0]) * 1000.0) as f32).collect()
}

/// Frames in the second before each sample (times and `t` both from the
/// session start).
pub fn fill_fps(samples: &mut [Sample], times: &[f64]) {
    for s in samples.iter_mut() {
        let t = s.t as f64;
        if t < 1.0 {
            continue;
        }
        let from = times.partition_point(|&x| x <= t - 1.0);
        let to = times.partition_point(|&x| x <= t);
        s.fps = Some((to - from) as f32);
    }
}

fn percentile(sorted: &[f32], p: f64) -> f32 {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

pub fn summarize(frame_ms: &[f32], samples: &[Sample]) -> Summary {
    let pause = (FRAME_PAUSE_S * 1000.0) as f32;
    let mut play: Vec<f32> = frame_ms.iter().copied().filter(|&m| m > 0.0 && m < pause).collect();
    if play.is_empty() {
        return Summary::default();
    }
    let active_ms: f64 = play.iter().map(|&m| m as f64).sum();
    play.sort_by(f32::total_cmp);
    let median = percentile(&play, 0.5);
    // seconds without frames (pauses) are not slow seconds
    let seconds: Vec<f32> = samples.iter().filter_map(|s| s.fps).filter(|&f| f > 0.0).collect();
    Summary {
        frames: frame_ms.len() as u64 + 1,
        active_s: active_ms / 1000.0,
        avg_fps: play.len() as f64 / (active_ms / 1000.0),
        min_fps: seconds.iter().copied().reduce(f32::min).unwrap_or(0.0) as f64,
        max_fps: seconds.iter().copied().reduce(f32::max).unwrap_or(0.0) as f64,
        low1: 1000.0 / percentile(&play, 0.99) as f64,
        low01: 1000.0 / percentile(&play, 0.999) as f64,
        median_ms: median as f64,
        bad_frames: play.iter().filter(|&&m| m > median * BAD_FRAME).count() as u64,
    }
}

impl Session {
    /// The file's bytes.
    pub fn encode(&self) -> std::io::Result<Vec<u8>> {
        let mut raw = serde_json::to_vec(&self.info).map_err(std::io::Error::other)?;
        raw.push(b'\n');
        for &ms in &self.frame_ms {
            let us = (ms as f64 * 1000.0).round().clamp(0.0, u32::MAX as f64) as u32;
            raw.extend_from_slice(&us.to_le_bytes());
        }
        let mut enc = zstd::Encoder::new(Vec::new(), 3)?;
        enc.write_all(&raw)?;
        enc.finish()
    }

    pub fn decode(bytes: &[u8]) -> std::io::Result<Session> {
        let mut raw = Vec::new();
        zstd::Decoder::new(bytes)?.read_to_end(&mut raw)?;
        let nl = raw.iter().position(|&b| b == b'\n').ok_or(std::io::ErrorKind::InvalidData)?;
        let info: SessionInfo = serde_json::from_slice(&raw[..nl]).map_err(std::io::Error::other)?;
        let frame_ms =
            raw[nl + 1..].as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c) as f32 / 1000.0).collect();
        Ok(Session { info, frame_ms })
    }

    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // whole or nothing: a half-written file never takes the name
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, self.encode()?)?;
        std::fs::rename(&tmp, path)
    }

    /// Only the header (the summary and the readings), not the frames.
    pub fn load_info(path: &std::path::Path) -> std::io::Result<SessionInfo> {
        use std::io::BufRead;
        let file = std::fs::File::open(path)?;
        let mut r = std::io::BufReader::new(zstd::Decoder::new(file)?);
        let mut line = Vec::new();
        r.read_until(b'\n', &mut line)?;
        serde_json::from_slice(line.strip_suffix(b"\n").unwrap_or(&line)).map_err(std::io::Error::other)
    }

    pub fn load(path: &std::path::Path) -> std::io::Result<Session> {
        Session::decode(&std::fs::read(path)?)
    }
}

/// CSV for Excel: ';' and decimal commas, unless English (',' and points).
struct Csv {
    sep: &'static str,
    comma: bool,
    out: String,
}

impl Csv {
    fn new(english: bool, header: &[&str]) -> Csv {
        let sep = if english { "," } else { ";" };
        // a BOM, so Excel reads it as UTF-8
        let mut out = String::from("\u{feff}");
        out += &header.join(sep);
        out.push_str("\r\n");
        Csv { sep, comma: !english, out }
    }

    fn row(&mut self, cells: &[Option<String>]) {
        let line: Vec<String> = cells
            .iter()
            .map(|c| {
                let v = c.clone().unwrap_or_default();
                if self.comma { v.replace('.', ",") } else { v }
            })
            .collect();
        self.out += &line.join(self.sep);
        self.out.push_str("\r\n");
    }
}

fn num(v: Option<f32>, digits: usize) -> Option<String> {
    v.map(|v| format!("{v:.digits$}"))
}

impl Session {
    /// One row a second: FPS, load, temperatures, memory.
    pub fn csv_seconds(&self, english: bool) -> String {
        let mut c = Csv::new(
            english,
            &["time_s", "fps", "cpu_load_%", "gpu_load_%", "cpu_temp_C", "gpu_temp_C", "ram_%"],
        );
        for s in &self.info.samples {
            c.row(&[
                num(Some(s.t), 1),
                num(s.fps, 0),
                num(s.cpu, 1),
                num(s.gpu, 1),
                num(s.cpu_t, 1),
                num(s.gpu_t, 1),
                num(s.ram, 1),
            ]);
        }
        c.out
    }

    /// One row a frame: its number, when it ended (s from the first frame), its time in ms.
    pub fn csv_frames(&self, english: bool) -> String {
        let mut c = Csv::new(english, &["frame", "time_s", "frame_ms"]);
        let mut t = 0.0f64;
        for (i, &ms) in self.frame_ms.iter().enumerate() {
            t += ms as f64 / 1000.0;
            c.row(&[Some((i + 1).to_string()), Some(format!("{t:.4}")), Some(format!("{ms:.3}"))]);
        }
        c.out
    }
}

/// Frames longer than this are stutters in the report: twice the median,
/// at least 25 ms (at 200 FPS a 10 ms frame is no stutter one can see).
pub fn stutter_ms(median_ms: f64) -> f32 {
    25f32.max(median_ms as f32 * 2.0).round()
}

/// A frame-time chart squeezes many frames into one column; in a long
/// session nearly every column holds a stutter or two, and marking them all
/// paints the chart red (Vlad's 3 h Civilization VI). So a column is marked
/// only with at least three times its share of the stutters: in a short
/// session that is still every one of them, in a long one the bursts.
/// `stutters` and `columns` count only the columns that have frames.
pub fn stutter_mark_min(stutters: usize, columns: usize) -> u32 {
    if columns == 0 {
        return 1;
    }
    ((stutters as f64 / columns as f64) * 3.0).ceil().max(1.0) as u32
}

/// "2026-11-22 13:05:41" + "civilizationvi" -> "2026-11-22_13-05-41_civilizationvi.pcms"
pub fn file_name(started: &str, game: &str) -> String {
    let stamp: String = started
        .chars()
        .map(|c| {
            if c == ' ' {
                '_'
            } else if c == ':' {
                '-'
            } else {
                c
            }
        })
        .collect();
    let game: String = game
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .take(40)
        .collect();
    format!("{stamp}_{game}.{EXTENSION}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steady(fps: f64, seconds: f64) -> Vec<f64> {
        let n = (fps * seconds) as usize;
        (0..=n).map(|i| i as f64 / fps).collect()
    }

    #[test]
    fn summary_of_a_steady_run_with_a_stutter_and_a_pause() {
        let mut times = steady(60.0, 40.0);
        // five 100 ms stutters (0.2% of the frames) from 10 s, a 5 s pause (alt-tab) at 20 s
        for k in [600, 700, 800, 900, 1000] {
            for t in times.iter_mut().skip(k) {
                *t += 0.1 - 1.0 / 60.0;
            }
        }
        for t in times.iter_mut().skip(1200) {
            *t += 5.0;
        }
        let mut samples: Vec<Sample> =
            (1..=45).map(|s| Sample { t: s as f32, ..Sample::default() }).collect();
        fill_fps(&mut samples, &times);
        assert_eq!(samples[4].fps, Some(60.0));
        assert_eq!(samples[22].fps, Some(0.0), "in the pause");
        let ms = frame_times(&times);
        let s = summarize(&ms, &samples);
        assert_eq!(s.frames, times.len() as u64);
        // 40 s + the stutters' extra 5 x 83 ms - the frame that ended in the pause
        assert!((s.active_s - 40.4).abs() < 0.001, "pause left out: {}", s.active_s);
        assert!((s.avg_fps - 59.38).abs() < 0.01, "{}", s.avg_fps);
        assert!((s.median_ms - 16.667).abs() < 0.01);
        assert_eq!(s.bad_frames, 5, "the stutters");
        assert!((s.low01 - 10.0).abs() < 0.1, "the stutters are the worst 0.1%: {}", s.low01);
        assert!((s.low1 - 60.0).abs() < 0.5);
        assert!((60.0..=61.0).contains(&s.max_fps), "{}", s.max_fps);
        assert!(s.min_fps > 0.0 && s.min_fps < 60.0);
    }

    #[test]
    fn file_round_trip() {
        let times = steady(144.0, 3.0);
        let session = Session {
            info: SessionInfo {
                version: VERSION,
                game: "game".into(),
                started: "2026-11-22 13:05:41".into(),
                samples: vec![Sample { t: 1.0, fps: Some(144.0), cpu: Some(40.0), ..Sample::default() }],
                ..SessionInfo::default()
            },
            frame_ms: frame_times(&times),
        };
        let bytes = session.encode().unwrap();
        let back = Session::decode(&bytes).unwrap();
        assert_eq!(back.info, session.info);
        assert_eq!(back.frame_ms.len(), session.frame_ms.len());
        assert!(back.frame_ms.iter().zip(&session.frame_ms).all(|(a, b)| (a - b).abs() < 0.001));
        assert!(Session::decode(b"not zstd").is_err());
        let path = std::env::temp_dir().join(format!("pcm-test-{}.pcms", std::process::id()));
        session.save(&path).unwrap();
        assert_eq!(Session::load_info(&path).unwrap(), session.info, "the header alone");
        let _ = std::fs::remove_file(&path);
        assert_eq!(file_name("2026-11-22 13:05:41", "Civ VI/dx12"), "2026-11-22_13-05-41_Civ_VI_dx12.pcms");
    }

    #[test]
    fn csv_for_excel() {
        let s = Session {
            info: SessionInfo {
                samples: vec![Sample { t: 1.0, fps: Some(60.0), cpu: Some(40.5), ..Sample::default() }],
                ..SessionInfo::default()
            },
            frame_ms: vec![16.667, 16.5],
        };
        let ru = s.csv_seconds(false);
        assert!(ru.starts_with("\u{feff}time_s;fps;cpu_load_%"));
        assert!(ru.contains("\r\n1,0;60;40,5;;;;\r\n"), "{ru:?}");
        assert!(s.csv_seconds(true).contains("\r\n1.0,60,40.5,,,,\r\n"));
        let frames = s.csv_frames(false);
        assert!(frames.contains("\r\n2;0,0332;16,500\r\n"), "{frames:?}");
    }

    #[test]
    fn an_hour_at_144_fps_is_small() {
        // ~520 000 frames with a little jitter: the file stays a few MB
        let times: Vec<f64> = (0..518_400).map(|i| i as f64 / 144.0 + (i % 7) as f64 * 0.0002).collect();
        let s = Session { info: SessionInfo::default(), frame_ms: frame_times(&times) };
        let bytes = s.encode().unwrap();
        assert!(bytes.len() < 2_000_000, "{} bytes", bytes.len());
        assert_eq!(Session::decode(&bytes).unwrap().frame_ms.len(), 518_399);
    }

    #[test]
    fn stutter_marks() {
        // a short session: every stutter is marked
        assert_eq!(stutter_mark_min(12, 480), 1);
        assert_eq!(stutter_mark_min(0, 480), 1);
        assert_eq!(stutter_mark_min(5, 0), 1);
        // Vlad's 3 h Civilization VI: 2679 stutters over 480 columns, ~5.6 each
        assert_eq!(stutter_mark_min(2679, 480), 17);
    }
}

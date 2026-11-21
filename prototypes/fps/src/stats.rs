//! Frame statistics, a straight port of `frame_stats` / `fps_from_times` /
//! `best_stats` and `FpsReader.add_frame` from PC Monitor 1.8 (widget.pyw).
//! Times are QPC seconds of present events.

use std::collections::HashMap;

/// Frames are averaged over this much of the latest events.
pub const FPS_WINDOW_S: f64 = 2.0;
/// Events arrive in batches up to ~1 s late.
pub const STALE_S: f64 = 2.5;
/// 1% low is taken over this many seconds of frames.
pub const FRAME_STATS_S: f64 = 10.0;
/// A longer gap is a pause (alt-tab, loading), not a stutter.
pub const FRAME_PAUSE_S: f64 = 1.0;
/// Frame times older than this are not needed any more.
pub const KEEP_S: f64 = FRAME_STATS_S + 4.0;
pub const PRUNE_EVERY_S: f64 = 30.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    /// Microsoft-Windows-DXGI Present_Start (Direct3D 10-12).
    Dxgi,
    /// Microsoft-Windows-DxgKrnl Present_Info (any API: DX9, OpenGL, Vulkan).
    Kernel,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Source::Dxgi => "dxgi",
            Source::Kernel => "krnl",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrameStats {
    pub fps: f64,
    /// 1 / 99th percentile frame time; None until there are 20 frames.
    pub low1: Option<f64>,
    /// The latest frame times in ms (up to 48), for the frame-time graph.
    pub frame_ms: Vec<f64>,
}

/// Frames per second, or None when the process hasn't presented lately.
pub fn fps_from_times(times: &[f64], now: f64) -> Option<f64> {
    let &last = times.last()?;
    if times.len() < 2 || now - last > STALE_S + FPS_WINDOW_S {
        return None;
    }
    let recent: Vec<f64> = times.iter().copied().filter(|&t| t >= last - FPS_WINDOW_S).collect();
    if recent.len() < 2 || recent[recent.len() - 1] <= recent[0] {
        return None;
    }
    Some((recent.len() - 1) as f64 / (recent[recent.len() - 1] - recent[0]))
}

pub fn frame_stats(times: &[f64], now: f64) -> Option<FrameStats> {
    let fps = fps_from_times(times, now)?;
    let last = times[times.len() - 1];
    let recent: Vec<f64> = times.iter().copied().filter(|&t| t >= last - FRAME_STATS_S).collect();
    let gaps: Vec<f64> =
        recent.windows(2).map(|w| w[1] - w[0]).filter(|&g| g > 0.0 && g < FRAME_PAUSE_S).collect();
    let ms = |g: &[f64]| g.iter().map(|x| x * 1000.0).collect::<Vec<_>>();
    if gaps.len() < 20 {
        return Some(FrameStats { fps, low1: None, frame_ms: ms(&gaps) });
    }
    let mut sorted = gaps.clone();
    sorted.sort_by(f64::total_cmp);
    let worst = sorted[(gaps.len() - 1).min((gaps.len() as f64 * 0.99) as usize)];
    let tail = &gaps[gaps.len().saturating_sub(48)..];
    Some(FrameStats { fps, low1: Some(1.0 / worst), frame_ms: ms(tail) })
}

/// Stats of the source that saw more frames (DX10-12 presents reach both).
pub fn best_stats(sources: &[&[f64]], now: f64) -> Option<(FrameStats, usize)> {
    let mut best: Option<(FrameStats, usize)> = None;
    for (i, times) in sources.iter().enumerate() {
        if let Some(st) = frame_stats(times, now)
            && best.as_ref().is_none_or(|(b, _)| st.fps > b.fps)
        {
            best = Some((st, i));
        }
    }
    best
}

/// Whole-run totals of one (pid, source), for the summary at exit.
#[derive(Clone, Copy, Debug, Default)]
pub struct Total {
    pub frames: u64,
    pub first: f64,
    pub last: f64,
}

impl Total {
    pub fn avg_fps(&self) -> Option<f64> {
        (self.frames > 1 && self.last > self.first)
            .then(|| (self.frames - 1) as f64 / (self.last - self.first))
    }
}

/// Recent present times per (pid, source), like FpsReader.frames in 1.8.
#[derive(Default)]
pub struct Frames {
    pub map: HashMap<(u32, Source), Vec<f64>>,
    pub totals: HashMap<(u32, Source), Total>,
    pruned: f64,
}

impl Frames {
    pub fn add(&mut self, pid: u32, source: Source, t: f64) {
        let times = self.map.entry((pid, source)).or_default();
        times.push(t);
        if times.len() > 64 && times[0] < t - KEEP_S {
            let half = times.len() / 2;
            times.drain(..half);
        }
        if t - self.pruned > PRUNE_EVERY_S {
            // forget programs that stopped drawing or have ended
            self.pruned = t;
            self.map.retain(|_, v| v.last().is_some_and(|&l| l >= t - KEEP_S));
        }
        let total = self.totals.entry((pid, source)).or_insert(Total { frames: 0, first: t, last: t });
        total.frames += 1;
        total.first = total.first.min(t);
        total.last = total.last.max(t);
    }

    /// Best stats of one process, with the source they came from.
    pub fn stats(&self, pid: u32, now: f64) -> Option<(FrameStats, Source)> {
        const SOURCES: [Source; 2] = [Source::Dxgi, Source::Kernel];
        let lists: Vec<&[f64]> =
            SOURCES.iter().map(|s| self.map.get(&(pid, *s)).map_or(&[][..], |v| v.as_slice())).collect();
        best_stats(&lists, now).map(|(st, i)| (st, SOURCES[i]))
    }

    pub fn pids(&self) -> Vec<u32> {
        let mut pids: Vec<u32> = self.map.keys().map(|(p, _)| *p).collect();
        pids.sort_unstable();
        pids.dedup();
        pids
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // the same cases as test_fps_and_one_percent_low / test_best_source_wins /
    // test_fps_reader_forgets_programs_that_stopped_drawing in 1.8

    #[test]
    fn fps_and_one_percent_low() {
        let mut times = Vec::new();
        let mut t = 0.0;
        for i in 0..700 {
            t += if i % 97 == 0 { 0.050 } else { 1.0 / 60.0 };
            times.push(t);
        }
        let st = frame_stats(&times, times[699] + 0.3).unwrap();
        assert!(55.0 < st.fps && st.fps < 62.0, "{}", st.fps);
        let low = st.low1.unwrap();
        assert!(15.0 < low && low < 25.0, "{low}");
        assert_eq!(st.frame_ms.len(), 48);
        assert!(fps_from_times(&times, times[699] + 10.0).is_none(), "stopped drawing");
        let pause: Vec<f64> =
            (0..300).map(|i| i as f64 / 60.0).chain((0..300).map(|i| 8.0 + i as f64 / 60.0)).collect();
        assert!(frame_stats(&pause, pause[599]).unwrap().low1.unwrap() > 55.0, "alt-tab is not a stutter");
    }

    #[test]
    fn best_source_wins() {
        let frames: Vec<f64> = (0..600).map(|i| i as f64 / 60.0).collect();
        let now = frames[599];
        let (st, _) = best_stats(&[&frames, &frames], now).unwrap();
        assert!((st.fps - 60.0).abs() < 0.1, "same frames, no double count");
        let (st, i) = best_stats(&[&[], &frames], now).unwrap();
        assert!((st.fps - 60.0).abs() < 0.1 && i == 1, "DX9 / OpenGL: kernel only");
        assert!(best_stats(&[&[], &[]], 1.0).is_none());
    }

    #[test]
    fn forgets_programs_that_stopped_drawing() {
        let mut f = Frames::default();
        for i in 0..200 {
            f.add(1, Source::Kernel, 10.0 + i as f64 / 60.0);
        }
        assert!(f.map.contains_key(&(1, Source::Kernel)));
        let t = 10.0 + 200.0 / 60.0 + KEEP_S + PRUNE_EVERY_S + 1.0;
        f.add(2, Source::Dxgi, t);
        assert!(!f.map.contains_key(&(1, Source::Kernel)) && f.map.contains_key(&(2, Source::Dxgi)));
        for i in 0..3600 {
            f.add(3, Source::Kernel, t + i as f64 / 60.0);
        }
        assert!((f.map[&(3, Source::Kernel)].len() as f64) < 60.0 * 2.0 * (KEEP_S + 1.0));
        // totals survive pruning
        assert_eq!(f.totals[&(1, Source::Kernel)].frames, 200);
        let avg = f.totals[&(3, Source::Kernel)].avg_fps().unwrap();
        assert!((avg - 60.0).abs() < 0.01, "{avg}");
    }
}

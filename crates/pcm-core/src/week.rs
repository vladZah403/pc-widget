//! A period's summary ("Итоги недели"): how long the PC ran, the games
//! played (time and FPS), the load, the hottest the processor got, the
//! traffic. From the history's points and the game sessions of the period.

use crate::history::{Column, Point, STEP_S};
use crate::session::SessionInfo;

/// Seconds between two "Итоги недели" cards.
pub const EVERY_S: i64 = 7 * 86400;
/// Less PC time than this in a week is not worth a card (s).
pub const MIN_ON_S: f64 = 3600.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameTime {
    /// The window title ("Sid Meier's Civilization VI (DX11)"), or the program.
    pub name: String,
    pub play_s: f64,
    pub sessions: u32,
    /// Over all its frames of the period.
    pub avg_fps: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Week {
    /// The PC ran (PC Monitor recorded), seconds.
    pub on_s: f64,
    pub cpu_avg: Option<f64>,
    pub cpu_t_max: Option<f64>,
    pub down_bytes: f64,
    pub up_bytes: f64,
    /// Most played first.
    pub games: Vec<GameTime>,
    pub play_s: f64,
}

/// The summary of these points and sessions (the caller picks the period).
pub fn summarize<'a>(points: &[Point], sessions: impl IntoIterator<Item = &'a SessionInfo>) -> Week {
    let col = |c: Column| points.iter().filter_map(|p| p.get(c)).collect::<Vec<f64>>();
    let cpu = col(Column::Cpu);
    let mut w = Week {
        on_s: points.len() as f64 * STEP_S as f64,
        cpu_avg: (!cpu.is_empty()).then(|| cpu.iter().sum::<f64>() / cpu.len() as f64),
        cpu_t_max: col(Column::CpuTemp).into_iter().reduce(f64::max),
        down_bytes: col(Column::Down).iter().sum::<f64>().max(0.0) * STEP_S as f64,
        up_bytes: col(Column::Up).iter().sum::<f64>().max(0.0) * STEP_S as f64,
        ..Week::default()
    };
    // per game: the time and the frames, for one FPS over all its sessions
    let mut by_game: Vec<(String, GameTime, u64)> = Vec::new();
    for s in sessions {
        let key = s.game.to_lowercase();
        let i = match by_game.iter().position(|g| g.0 == key) {
            Some(i) => i,
            None => {
                let name = if s.title.trim().is_empty() { s.game.clone() } else { s.title.clone() };
                by_game.push((key, GameTime { name, ..GameTime::default() }, 0));
                by_game.len() - 1
            }
        };
        let g = &mut by_game[i];
        g.1.play_s += s.summary.active_s;
        g.1.sessions += 1;
        g.2 += s.summary.frames;
    }
    w.games = by_game
        .into_iter()
        .map(|(_, mut g, frames)| {
            g.avg_fps = if g.play_s > 0.0 { frames as f64 / g.play_s } else { 0.0 };
            g
        })
        .collect();
    w.games.sort_by(|a, b| b.play_s.total_cmp(&a.play_s));
    w.play_s = w.games.iter().map(|g| g.play_s).sum();
    w
}

/// Sessions that started at or after `from_local` ("2026-11-16 18:00:00",
/// the same local-time format SessionInfo::started has).
pub fn since<'a>(sessions: &'a [SessionInfo], from_local: &str) -> impl Iterator<Item = &'a SessionInfo> {
    let from = from_local.to_string();
    sessions.iter().filter(move |s| s.started.as_str() >= from.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(game: &str, title: &str, started: &str, active_s: f64, frames: u64) -> SessionInfo {
        let mut s = SessionInfo {
            game: game.into(),
            title: title.into(),
            started: started.into(),
            ..Default::default()
        };
        s.summary.active_s = active_s;
        s.summary.frames = frames;
        s
    }

    #[test]
    fn games_and_load() {
        let points: Vec<Point> = (0..360)
            .map(|i| Point {
                t: i * 10,
                cpu: if i < 180 { 10.0 } else { 30.0 },
                cpu_t: Some(50.0 + (i % 40) as f32),
                down: Some(1000.0),
                up: Some(100.0),
                ..Default::default()
            })
            .collect();
        let all = [
            session("stalzonew", "STALCRAFT", "2026-11-20 20:00:00", 3600.0, 3600 * 60),
            session("civilizationvi", "Civilization VI", "2026-11-21 13:00:00", 7200.0, 7200 * 44),
            session("stalzonew", "STALCRAFT", "2026-11-22 21:00:00", 1800.0, 1800 * 90),
            session("terraria", "", "2026-11-10 10:00:00", 600.0, 600 * 60),
        ];
        let w = summarize(&points, since(&all, "2026-11-16 00:00:00"));
        assert_eq!(w.on_s, 3600.0);
        assert_eq!(w.cpu_avg, Some(20.0));
        assert_eq!(w.cpu_t_max, Some(89.0));
        assert_eq!(w.down_bytes, 3_600_000.0);
        assert_eq!(w.games.len(), 2, "terraria is older than the week");
        assert_eq!(w.games[0].name, "Civilization VI");
        assert_eq!(w.games[1].sessions, 2);
        assert_eq!(w.games[1].avg_fps, 70.0, "(3600*60 + 1800*90) frames / 5400 s");
        assert_eq!(w.play_s, 12600.0);
    }
}

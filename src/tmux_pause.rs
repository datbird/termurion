//! tmux pause mode (tmux 3.2 and later; iTerm2's iTermTmuxBufferSizeMonitor and the
//! session's pause warning): each pane's output age from `%extended-output` is fitted to a
//! line, and a pane that will reach the age limit soon is warned about before tmux pauses it.

use std::collections::{HashMap, VecDeque};

/// A pane's recent (time, age) samples, in seconds.
#[derive(Clone, Debug)]
pub struct Series {
    data: VecDeque<(f64, f64)>,
    last_update: f64,
    last_ttl: f64,
    red: u32,
}

const CAPACITY: usize = 4;

impl Default for Series {
    fn default() -> Series {
        Series { data: VecDeque::new(), last_update: 0.0, last_ttl: f64::INFINITY, red: 0 }
    }
}

impl Series {
    fn add(&mut self, t: f64, age: f64) {
        self.last_update = t;
        self.data.push_back((t, age));
        while self.data.len() > CAPACITY {
            self.data.pop_front();
        }
    }

    /// Least squares: (slope, offset) of age over time; None with fewer than two samples.
    pub fn fit(&self) -> Option<(f64, f64)> {
        if self.data.len() < 2 {
            return None;
        }
        let n = self.data.len() as f64;
        let (mut sx, mut sx2, mut sy, mut sxy) = (0.0, 0.0, 0.0, 0.0);
        for (t, a) in &self.data {
            sx += t;
            sx2 += t * t;
            sy += a;
            sxy += t * a;
        }
        let b = (n * sxy - sx * sy) / (n * sx2 - sx * sx);
        Some((b, (sy - b * sx) / n))
    }

    /// When the age reaches `target` (infinite when it does not grow).
    pub fn time_for_age(&self, target: f64) -> f64 {
        match self.fit() {
            Some((slope, offset)) if slope > 0.0 => (target - offset) / slope,
            _ => f64::INFINITY,
        }
    }
}

/// The monitor for one tmux session.
#[derive(Clone, Debug)]
pub struct Monitor {
    pub age_limit: f64,
    series: HashMap<u32, Series>,
}

impl Monitor {
    pub fn new(age_limit: f64) -> Monitor {
        Monitor { age_limit, series: HashMap::new() }
    }

    /// A latency sample; kept when the last one is over half a second old.
    pub fn set_latency(&mut self, pane: u32, age: f64, now: f64) {
        let s = self.series.entry(pane).or_default();
        if now - s.last_update > 0.5 {
            s.add(now, age);
        }
    }

    /// A pane paused: its samples start over.
    pub fn reset(&mut self, pane: u32) {
        self.series.remove(&pane);
    }

    /// The once-a-second check: (pane, seconds left, red zone) for each pane to warn about
    /// or to clear a warning for.
    pub fn update(&mut self, now: f64) -> Vec<(u32, f64, bool)> {
        let half = self.age_limit / 2.0;
        let limit = self.age_limit;
        let mut out = Vec::new();
        let mut panes: Vec<u32> = self.series.keys().copied().collect();
        panes.sort_unstable();
        for pane in panes {
            let s = self.series.get_mut(&pane).expect("series");
            if s.data.len() < CAPACITY {
                continue;
            }
            let ttl = if now - s.last_update > 2.0 {
                s.data.clear();
                s.last_update = now;
                f64::INFINITY
            } else {
                s.time_for_age(limit) - now
            };
            let red = ttl < half;
            let was_red = s.last_ttl < half;
            s.last_ttl = ttl;
            if !red && !was_red {
                s.red = 0;
                continue;
            }
            // Five red ticks before the first warning, then every tick while red.
            if red {
                s.red += 1;
                if s.red < 5 {
                    continue;
                }
            } else {
                s.red = 0;
            }
            out.push((pane, ttl, red));
        }
        out
    }
}

/// What the pane's warning bar does.
#[derive(Clone, Debug, PartialEq)]
pub enum Bar {
    Show(String),
    Dismiss,
}

/// The shown seconds' hysteresis (iTerm2's _tmuxTTLLowerThreshold and upper).
#[derive(Clone, Copy, Debug, Default)]
pub struct Warning {
    band: Option<(f64, f64)>,
}

impl Warning {
    /// A pane paused: the next warning starts fresh.
    pub fn reset(&mut self) {
        self.band = None;
    }

    /// iTerm2's tmuxControllerSessionSetTTL; None leaves the bar as it is.
    pub fn update(&mut self, ttl: f64, red: bool, paused: bool, warn: bool) -> Option<Bar> {
        if paused || !warn {
            return None;
        }
        self.band = Some(match self.band {
            Some((lo, hi)) if ttl > lo && ttl < hi => return None,
            Some((lo, _)) if ttl <= lo => (ttl - 1.0, ttl + 1.5),
            Some(_) => (ttl - 1.5, ttl + 1.0),
            None => (ttl - 1.0, ttl + 1.0),
        });
        if !red {
            return Some(Bar::Dismiss);
        }
        let rounded = ttl.round();
        let secs = if rounded.is_nan() || rounded > i64::MAX as f64 { i64::MAX } else { (rounded as i64).max(1) };
        Some(Bar::Show(format!("This session will pause in about {secs} seconds because it is buffering too much data.")))
    }
}

/// Turns pause mode on (or changes the age), at least one second.
pub fn enable_command(age_limit: f64) -> String {
    format!("refresh-client -fpause-after={}", age_limit.round().max(1.0) as i64)
}

pub fn pause_command(panes: &[u32]) -> String {
    let adjust: Vec<String> = panes.iter().map(|p| format!("%{p}:pause")).collect();
    format!("refresh-client -A '{}'", adjust.join(" "))
}

/// The bar when a pane paused: tmux did it (a notification), or the person did.
pub fn paused_text(by_tmux: bool) -> &'static str {
    if by_tmux { "tmux paused this session because too much output was buffered." } else { "Session paused." }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn series_fit_like_iterm2() {
        let mut m = Monitor::new(10.0);
        for t in 0..4 {
            m.set_latency(1, t as f64, 1.0 + t as f64);
        }
        m.set_latency(1, 99.0, 4.2);
        let s = &m.series[&1];
        assert_eq!(s.data.len(), 4, "a sample within half a second is dropped");
        let (slope, offset) = s.fit().unwrap();
        assert!((slope - 1.0).abs() < 1e-9 && (offset + 1.0).abs() < 1e-9);
        assert!((s.time_for_age(10.0) - 11.0).abs() < 1e-9);
        let flat = Series { data: [(0.0, 1.0), (1.0, 1.0)].into(), ..Default::default() };
        assert_eq!(flat.time_for_age(10.0), f64::INFINITY, "not growing");
    }

    #[test]
    fn warnings_follow_iterm2() {
        let mut m = Monitor::new(20.0);
        let mut emitted = Vec::new();
        for t in 0..=20 {
            let now = t as f64;
            m.set_latency(1, now, now);
            for (p, ttl, red) in m.update(now) {
                emitted.push((p, now, ttl.round() as i64, red));
            }
        }
        assert_eq!(emitted.first(), Some(&(1, 15.0, 5, true)), "five red ticks first (red below 10 s)");
        assert_eq!(emitted.get(1), Some(&(1, 16.0, 4, true)), "then every tick while it stays red");
        assert_eq!(emitted.len(), 6);
        // Samples older than two seconds start over.
        let mut m = Monitor::new(20.0);
        for t in 1..=4 {
            m.set_latency(1, 0.0, t as f64);
        }
        assert!(m.update(10.0).is_empty());
        assert!(m.series[&1].data.is_empty());

        let mut w = Warning::default();
        let text = |n: i64| Bar::Show(format!("This session will pause in about {n} seconds because it is buffering too much data."));
        assert_eq!(w.update(5.0, true, false, true), Some(text(5)));
        assert_eq!(w.update(4.5, true, false, true), None, "inside the band");
        assert_eq!(w.update(3.9, true, false, true), Some(text(4)));
        assert_eq!(w.update(6.0, false, false, true), Some(Bar::Dismiss));
        assert_eq!(w.update(0.2, true, false, true), Some(text(1)), "never below one second");
        assert_eq!(w.update(1.0, true, true, true), None, "paused panes are not warned");
        assert_eq!(w.update(1.0, true, false, false), None, "Warn before pausing off");
    }

    #[test]
    fn commands_follow_iterm2() {
        assert_eq!(enable_command(120.0), "refresh-client -fpause-after=120");
        assert_eq!(enable_command(0.2), "refresh-client -fpause-after=1");
        assert_eq!(pause_command(&[3, 4]), "refresh-client -A '%3:pause %4:pause'");
        assert_eq!(paused_text(true), "tmux paused this session because too much output was buffered.");
        assert_eq!(paused_text(false), "Session paused.");
    }
}

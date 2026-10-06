//! iTerm2's precise timers for the drawing statistics (`iTermPreciseTimer.m`), as plain numbers.
//! `render` fills a `Timers` while `LogDrawingPerformance` is on (nothing is measured while it
//! is off) and logs the table every `DRAW_EVERY`. Not to be confused with `perf_stats`, which is
//! Copy Performance Stats.

use std::time::{Duration, Instant};

/// `iTermTextDrawingHelper.m:342`: the drawing statistics log every five seconds.
pub const DRAW_EVERY: Duration = Duration::from_secs(5);

/// One named timer: samples in seconds, and the events each covered.
#[derive(Clone, Debug, Default)]
pub struct Stat {
    pub name: &'static str,
    pub n: u64,
    pub sum: f64,
    pub sum_sq: f64,
    pub events: u64,
}

impl Stat {
    pub fn add(&mut self, secs: f64, events: u64) {
        self.n += 1;
        self.sum += secs;
        self.sum_sq += secs * secs;
        self.events += events;
    }

    pub fn mean_ms(&self) -> f64 {
        if self.n == 0 { 0.0 } else { self.sum / self.n as f64 * 1000.0 }
    }

    /// The sample standard deviation; NaN below two samples (`PTM:161-168`).
    pub fn stddev_ms(&self) -> f64 {
        if self.n < 2 {
            return f64::NAN;
        }
        let n = self.n as f64;
        let m2 = (self.sum_sq - self.sum * self.sum / n).max(0.0);
        (m2 / (n - 1.0)).sqrt() * 1000.0
    }

    fn reset(&mut self) {
        *self = Stat { name: self.name, ..Stat::default() };
    }
}

#[derive(Clone, Debug)]
pub struct Timers {
    pub stats: Vec<Stat>,
    last_log: Option<Instant>,
}

impl Timers {
    pub fn new(names: &[&'static str]) -> Timers {
        Timers { stats: names.iter().map(|n| Stat { name: n, ..Stat::default() }).collect(), last_log: None }
    }

    /// True the first time, then once `every` has passed since the last true.
    pub fn due(&mut self, now: Instant, every: Duration) -> bool {
        match self.last_log {
            Some(last) if now.saturating_duration_since(last) < every => false,
            _ => {
                self.last_log = Some(now);
                true
            }
        }
    }
}

/// A timer table as `log_string` made it: numbers and the stats' static names only, so it may go to
/// the debug log (a `LogSafe` value only this module can make).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table(String);

impl std::fmt::Display for Table {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl crate::dlog::LogSafe for Table {}

impl Table {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// `log_string` for the log: the same table, as a value `dlog!` accepts.
pub fn log_table(identifier: &str, stats: &mut [Stat]) -> Table {
    Table(log_string(identifier, stats))
}

/// `PTM:194-264`, the form without histograms: iTerm2's duration emoji is left out and its en dash
/// is written "-" (Decision). A stat with no samples prints nothing; every stat is reset.
pub fn log_string(identifier: &str, stats: &mut [Stat]) -> String {
    let mut log = format!("-- Precise Timers for {identifier} --\n");
    for s in stats.iter_mut() {
        if s.n > 0 {
            let (mean, sd, n) = (s.mean_ms(), s.stddev_ms(), s.n as f64);
            log.push_str(&format!(
                "{:>20}: \u{3bc}={mean:.3}ms \u{3c3}={sd:.3}ms (95% CI \u{2245} {:.3}ms-{:.3}ms) \u{3a3}={:.2}ms N={} avg. events={:.0}\n",
                s.name,
                0f64.max(mean - sd),
                mean + sd,
                n * mean,
                s.n,
                s.events as f64 / n
            ));
        }
        s.reset();
    }
    log
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// `PTM:194-264`: the no-histogram line with iTerm2's numbers (sample standard deviation), the
    /// emoji left out and the en dash written as "-" (Decision); the stats are reset after.
    #[test]
    fn timer_log_follows_iterm2() {
        let mut draw = Stat { name: "draw", ..Stat::default() };
        draw.add(0.002, 1);
        draw.add(0.004, 1);
        assert!((draw.mean_ms() - 3.0).abs() < 1e-9);
        let mut stats = [draw];
        assert_eq!(
            log_string("render", &mut stats),
            "-- Precise Timers for render --\n                draw: \u{3bc}=3.000ms \u{3c3}=1.414ms (95% CI \u{2245} 1.586ms-4.414ms) \u{3a3}=6.00ms N=2 avg. events=1\n"
        );
        assert_eq!((stats[0].n, stats[0].events, stats[0].sum), (0, 0, 0.0), "reset after the log");
        assert_eq!(stats[0].name, "draw", "the name stays");
        let mut one = [Stat { name: "scene", ..Stat::default() }];
        one[0].add(0.001, 0);
        assert!(log_string("render", &mut one).contains("\u{3c3}=NaNms"), "one sample has no deviation");
        let mut none = [Stat { name: "idle", ..Stat::default() }];
        assert_eq!(log_string("render", &mut none), "-- Precise Timers for render --\n", "a stat with no samples prints no line");
    }

    /// `iTermTextDrawingHelper.m:342`: the first call is due, then every `DRAW_EVERY`.
    #[test]
    fn timers_are_due_every_five_seconds() {
        assert_eq!(DRAW_EVERY, Duration::from_secs(5));
        let mut t = Timers::new(&["frame", "scene"]);
        assert_eq!(t.stats.iter().map(|s| s.name).collect::<Vec<_>>(), ["frame", "scene"]);
        let t0 = Instant::now();
        assert!(t.due(t0, DRAW_EVERY));
        assert!(!t.due(t0 + Duration::from_secs(4), DRAW_EVERY));
        assert!(t.due(t0 + Duration::from_secs(5), DRAW_EVERY));
        assert!(!t.due(t0 + Duration::from_secs(6), DRAW_EVERY));
    }
}

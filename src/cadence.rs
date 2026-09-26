//! How often Instant Replay may record a pane: iTerm2 saves a frame at each screen update
//! (`TV:2910-2917`), and the update timer's period follows the session
//! (`iTermUpdateCadenceController.m`, `UC` below). Pure, so the rules are tested here.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// A visible, busy session at a low byte rate: 60 frames a second (`activeUpdateCadence` and
/// `maximumFrameRate`, 60).
pub const FAST_GAP: Duration = Duration::from_millis(16);
/// A visible session receiving at least `THROUGHPUT_LIMIT` bytes a second: `slowFrameRate`
/// (15 fps).
pub const SLOW_GAP: Duration = Duration::from_nanos(1_000_000_000 / 15);
/// A session in no visible tab, or idle: `kBackgroundUpdateCadence` (`UC:20`).
pub const BACKGROUND_GAP: Duration = Duration::from_secs(1);
/// `adaptiveFrameRateThroughputThreshold` (10000 bytes a second).
pub const THROUGHPUT_LIMIT: i64 = 10_000;
/// `isIdle` (`PS:6584-6587`): no output for more than `idleTimeSeconds` (2) plus one.
pub const IDLE_AFTER: Duration = Duration::from_secs(3);

/// `changeCadenceIfNeeded:` (`UC:145-191`) on the non-Metal path with iTerm2's defaults:
/// idle or not in a visible tab is the background period; in an alternate screen the
/// adaptive rate is off (`disableAdaptiveFrameRateInInteractiveApps`, `PS:22126-22128`), so
/// the fast one; else fast only while `0 < throughput < THROUGHPUT_LIMIT`, slow otherwise.
pub fn frame_gap(visible: bool, idle: bool, alt_screen: bool, throughput: i64) -> Duration {
    if idle || !visible {
        return BACKGROUND_GAP;
    }
    if alt_screen || (throughput > 0 && throughput < THROUGHPUT_LIMIT) {
        return FAST_GAP;
    }
    SLOW_GAP
}

/// iTerm2's `iTermThroughputEstimator` as the token executor makes it
/// (`TokenExecutor.swift:370-371`): five buckets of 1/30 s, newer buckets weighted double,
/// the current one scaled up for the part of it still to come (at most 10 times).
#[derive(Clone, Debug)]
pub struct Throughput {
    start: Option<Instant>,
    buckets: VecDeque<u64>,
    last_index: i64,
}

const BUCKETS: usize = 5;
const BUCKET_SECS: f64 = 1.0 / 30.0;

impl Default for Throughput {
    fn default() -> Self {
        Throughput { start: None, buckets: VecDeque::from(vec![0; BUCKETS]), last_index: 0 }
    }
}

impl Throughput {
    /// Starts the clock at `start` (iTerm2's creation time).
    pub fn new(start: Instant) -> Throughput {
        Throughput { start: Some(start), ..Throughput::default() }
    }

    /// `eraseBucketsIfNeeded`: seconds since the start, with elapsed buckets emptied.
    fn erase(&mut self, now: Instant) -> f64 {
        let start = *self.start.get_or_insert(now);
        let delta = now.saturating_duration_since(start).as_secs_f64();
        let index = (delta / BUCKET_SECS).floor() as i64;
        let n = (index - self.last_index).clamp(0, BUCKETS as i64) as usize;
        self.last_index = self.last_index.max(index);
        for _ in 0..n {
            self.buckets.pop_front();
            self.buckets.push_back(0);
        }
        delta
    }

    /// `addByteCount:`.
    pub fn add(&mut self, now: Instant, bytes: usize) {
        self.erase(now);
        if let Some(b) = self.buckets.back_mut() {
            *b += bytes as u64;
        }
    }

    /// `estimatedThroughput`, in bytes a second.
    pub fn estimate(&mut self, now: Instant) -> i64 {
        let delta = self.erase(now);
        let spent = delta % BUCKET_SECS;
        let current = (BUCKET_SECS / spent).min(10.0);
        let (mut sum, mut weight) = (0.0, 1.0);
        for (i, b) in self.buckets.iter().enumerate() {
            let mut value = *b as f64;
            if i == BUCKETS - 1 {
                value *= current;
            }
            sum += value * weight;
            weight *= 2.0;
        }
        (sum / (weight - 1.0) / BUCKET_SECS) as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cadence_follows_iterm2() {
        assert_eq!(SLOW_GAP, Duration::from_nanos(66_666_666));
        assert_eq!(frame_gap(false, false, false, 500), BACKGROUND_GAP, "not in a visible tab");
        assert_eq!(frame_gap(true, true, false, 500), BACKGROUND_GAP, "idle");
        assert_eq!(frame_gap(false, true, true, 0), BACKGROUND_GAP);
        assert_eq!(frame_gap(true, false, false, 500), FAST_GAP);
        assert_eq!(frame_gap(true, false, false, 9_999), FAST_GAP);
        assert_eq!(frame_gap(true, false, false, 10_000), SLOW_GAP, "at the threshold");
        assert_eq!(frame_gap(true, false, false, 50_000), SLOW_GAP);
        assert_eq!(frame_gap(true, false, false, 0), SLOW_GAP, "UC:179 fast only above 0");
        assert_eq!(frame_gap(true, false, true, 50_000), FAST_GAP, "no adaptive rate in an alternate screen");
    }

    #[test]
    fn throughput_follows_iterm2() {
        let t0 = Instant::now();
        let mut t = Throughput::new(t0);
        assert_eq!(t.estimate(t0), 0);
        t.add(t0, 300);
        // At the bucket's start its weight is the cap: 300 * 10 * 16 / 31 * 30.
        assert_eq!(t.estimate(t0), 46_451);
        // 50 ms on, the bucket is one older and the current one empty: 300 * 8 / 31 * 30.
        assert_eq!(t.estimate(t0 + Duration::from_millis(50)), 2_322);
        // Five buckets on, it is forgotten.
        assert_eq!(t.estimate(t0 + Duration::from_millis(200)), 0);
        t.add(t0 + Duration::from_millis(1_000), 30_000);
        assert!(t.estimate(t0 + Duration::from_millis(1_010)) >= THROUGHPUT_LIMIT);
    }
}

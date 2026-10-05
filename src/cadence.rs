//! How often Instant Replay may record a pane: iTerm2 saves a frame at each screen update
//! (`TV:2910-2917`), and the update timer's period follows the session
//! (`iTermUpdateCadenceController.m`, `UC` below). Pure, so the rules are tested here.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::globals::Globals;

/// A visible, busy session at a low byte rate: 60 frames a second (`activeUpdateCadence` and
/// `maximumFrameRate`, 60).
pub const FAST_GAP: Duration = Duration::from_millis(16);
/// A visible session receiving at least `THROUGHPUT_LIMIT` bytes a second. Tandem draws on
/// the GPU like iTerm2's default Metal path, so this is `metalSlowFrameRate` (30 fps,
/// `iTermAdvancedSettingsModel.m:570`), not the non-Metal `slowFrameRate` (15).
pub const SLOW_GAP: Duration = Duration::from_nanos(1_000_000_000 / 30);
/// A session in no visible tab, or idle: `kBackgroundUpdateCadence` (`UC:20`).
pub const BACKGROUND_GAP: Duration = Duration::from_secs(1);
/// `adaptiveFrameRateThroughputThreshold` (10000 bytes a second).
pub const THROUGHPUT_LIMIT: i64 = 10_000;
/// `isIdle` (`PS:6584-6587`): no output for more than `idleTimeSeconds` (2) plus one.
pub const IDLE_AFTER: Duration = Duration::from_secs(3);

/// `frame_gap` with iTerm2's defaults, on the visible, normal-power path: what Instant
/// Replay's tests were written against.
#[allow(dead_code)]
pub fn frame_gap(visible: bool, idle: bool, alt_screen: bool, throughput: i64) -> Duration {
    interval(State { active: false, idle, visible, alt_screen, throughput, low_power: false }, &Prefs::default())
}

/// The update cadence settings (`iTermAdvancedSettingsModel`, `iTermPreferences`). The two
/// non-Metal keys (`UseAdaptiveFrameRate`, `DisableAdaptiveFrameRateInInteractiveApps`) are
/// read only on iTerm2's CPU path, so they are not here (ruling R2).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Prefs {
    /// `metalMaximizeThroughput` (General, "Maximize throughput"): the adaptive rate.
    pub maximize_throughput: bool,
    /// `ActiveUpdateCadence`: the rate when not adaptive (the alternate screen).
    pub active_update_cadence: f64,
    /// `MaximumFrameRate`: adaptive and light output.
    pub maximum_frame_rate: f64,
    /// `MetalSlowFrameRate`: adaptive and heavy output.
    pub metal_slow_frame_rate: f64,
    /// `AdaptiveFrameRateThroughputThreshold`, bytes a second.
    pub threshold: i64,
    /// `LowPowerModeFrameRate`.
    pub low_power_frame_rate: f64,
}

impl Default for Prefs {
    fn default() -> Prefs {
        Prefs { maximize_throughput: true, active_update_cadence: 60.0, maximum_frame_rate: 60.0, metal_slow_frame_rate: 30.0, threshold: THROUGHPUT_LIMIT, low_power_frame_rate: 30.0 }
    }
}

impl Prefs {
    /// Reads the settings, each falling back to iTerm2's default.
    pub fn from_globals(g: &Globals) -> Prefs {
        let d = Prefs::default();
        let num = |key: &str, default: f64| g.number(key).unwrap_or(default);
        Prefs {
            maximize_throughput: g.optional_bool("metalMaximizeThroughput").unwrap_or(d.maximize_throughput),
            active_update_cadence: num("ActiveUpdateCadence", d.active_update_cadence),
            maximum_frame_rate: num("MaximumFrameRate", d.maximum_frame_rate),
            metal_slow_frame_rate: num("MetalSlowFrameRate", d.metal_slow_frame_rate),
            threshold: g.get("AdaptiveFrameRateThroughputThreshold").and_then(|v| v.as_integer()).unwrap_or(d.threshold),
            low_power_frame_rate: num("LowPowerModeFrameRate", d.low_power_frame_rate),
        }
    }
}

/// `iTermUpdateCadenceState` for one pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct State {
    /// Needs periodic redraws (a cursor blinking or sliding).
    pub active: bool,
    /// No output for a while (`isIdle`).
    pub idle: bool,
    /// In the visible tab of a window on screen.
    pub visible: bool,
    pub alt_screen: bool,
    /// Bytes a second, `Throughput::estimate`.
    pub throughput: i64,
    /// Windows battery saver (`lowPowerMode`).
    pub low_power: bool,
}

/// Typed echoes draw at once under this many bytes a second (`UC:84`).
pub const TYPING_THROUGHPUT: i64 = 1024;
/// A keystroke is recent for this long (`UC:90-94`).
pub const TYPING_WINDOW: Duration = Duration::from_millis(100);

/// `didHandleInputWithThroughput:` (`UC:81-94`): light output right after a key.
pub fn typing_now(throughput: i64, last_key: Option<Instant>, now: Instant) -> bool {
    throughput < TYPING_THROUGHPUT && last_key.is_some_and(|k| now.saturating_duration_since(k) < TYPING_WINDOW)
}

/// `changeCadenceIfNeeded:` (`UC:124-181`) on the GPU path, with `PS:22107-22140`'s state and
/// the millisecond knocked off at `UC:239-243`.
pub fn interval(s: State, p: &Prefs) -> Duration {
    let rate = |fps: f64| 1.0 / fps.max(1.0);
    let low_power = |secs: f64| if s.low_power { secs.max(rate(p.low_power_frame_rate)) } else { secs };
    let secs = if !(s.active || !s.idle) || !s.visible {
        1.0
    } else if !p.maximize_throughput || s.alt_screen {
        low_power(rate(p.active_update_cadence))
    } else if s.throughput > 0 && s.throughput < p.threshold {
        low_power(rate(p.maximum_frame_rate))
    } else {
        low_power(1.0 / p.metal_slow_frame_rate)
    };
    Duration::from_secs_f64(secs.max(0.002) - 0.001)
}

/// A window has one surface, so it draws at the fastest rate any of its visible panes asks for
/// (none: the background period).
pub fn window_interval(states: &[State], p: &Prefs) -> Duration {
    states.iter().map(|s| interval(*s, p)).min().unwrap_or_else(|| interval(State { active: false, idle: true, visible: false, alt_screen: false, throughput: 0, low_power: false }, p))
}

/// When a window may draw again after pane output: iTerm2 updates the display on a timer at
/// the cadence, and at once for a typed echo. One per window.
#[derive(Clone, Debug, Default)]
pub struct Pacer {
    last: Option<Instant>,
    due: Option<Instant>,
}

impl Pacer {
    /// Output arrived. True: draw now. False: the due time is remembered for `take_due`.
    pub fn output(&mut self, now: Instant, gap: Duration, typing: bool) -> bool {
        let next = self.last.map(|l| l + gap);
        if typing || next.is_none_or(|n| now >= n) {
            self.due = None;
            return true;
        }
        self.due = next;
        false
    }

    /// A frame was drawn (or tried): any wait for it is over.
    pub fn drew(&mut self, now: Instant) {
        self.last = Some(now);
        self.due = None;
    }

    pub fn due(&self) -> Option<Instant> {
        self.due
    }

    /// When the last frame was drawn.
    pub fn last(&self) -> Option<Instant> {
        self.last
    }

    /// True once, at or after the due time.
    pub fn take_due(&mut self, now: Instant) -> bool {
        if self.due.is_some_and(|d| now >= d) {
            self.due = None;
            return true;
        }
        false
    }
}

/// The next forced frame (`iTermMTKView.swift:58-91`): a GPU view redraws at least every
/// `MetalRedrawPeriod`, so a window that last drew at `last` is due at `last + period`; one
/// that never drew is due now. None when the period is not positive (nothing is forced, so
/// an idle window is never woken).
pub fn period_due(last: Option<Instant>, period: Duration, now: Instant) -> Option<Instant> {
    if period.is_zero() {
        return None;
    }
    Some(last.map_or(now, |l| l + period))
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

    fn us(d: Duration) -> i64 {
        (d.as_secs_f64() * 1e6).round() as i64
    }

    fn state() -> State {
        State { active: false, idle: false, visible: true, alt_screen: false, throughput: 500, low_power: false }
    }

    #[test]
    fn cadence_follows_iterm2() {
        let p = Prefs::default();
        let s = state();
        assert_eq!(us(interval(s, &p)), 15_667, "1/60 s minus 1 ms");
        assert_eq!(us(interval(State { idle: true, ..s }, &p)), 999_000, "idle and not active");
        assert_eq!(us(interval(State { idle: true, active: true, ..s }, &p)), 15_667, "blinking keeps the visible rule");
        assert_eq!(us(interval(State { visible: false, ..s }, &p)), 999_000, "not visible");
        assert_eq!(us(interval(State { throughput: 10_000, ..s }, &p)), 32_333, "at the threshold, 30 fps");
        assert_eq!(us(interval(State { throughput: 9_999, ..s }, &p)), 15_667);
        assert_eq!(us(interval(State { throughput: 0, ..s }, &p)), 32_333, "UC:179 fast only above 0");
        assert_eq!(us(interval(State { alt_screen: true, throughput: 50_000, ..s }, &p)), 15_667, "ActiveUpdateCadence");
        let fast = Prefs { maximum_frame_rate: 120.0, ..p };
        assert_eq!(us(interval(State { alt_screen: true, throughput: 50_000, ..s }, &fast)), 15_667, "the alternate screen ignores MaximumFrameRate");
        assert_eq!(us(interval(s, &fast)), 7_333, "adaptive and light: MaximumFrameRate");
        let off = Prefs { maximize_throughput: false, ..p };
        assert_eq!(us(interval(State { throughput: 50_000, ..s }, &off)), 15_667, "not adaptive");
        let zero = Prefs { active_update_cadence: 0.0, ..p };
        assert_eq!(us(interval(State { alt_screen: true, ..s }, &zero)), 999_000, "MAX(1, rate)");
        let huge = Prefs { maximum_frame_rate: 100_000.0, ..p };
        assert_eq!(us(interval(s, &huge)), 1_000, "never below 1 ms");
    }

    #[test]
    fn low_power_caps_the_rate() {
        let p = Prefs::default();
        let s = State { low_power: true, ..state() };
        assert_eq!(us(interval(s, &p)), 32_333, "at least 1/30 s");
        assert_eq!(us(interval(State { throughput: 10_000, ..s }, &p)), 32_333);
        assert_eq!(us(interval(State { visible: false, ..s }, &p)), 999_000, "background stays 1 s");
        assert_eq!(us(interval(State { idle: true, ..s }, &p)), 999_000);
        assert_eq!(us(interval(s, &Prefs { low_power_frame_rate: 10.0, ..p })), 99_000);
        assert_eq!(us(interval(State { alt_screen: true, ..s }, &p)), 32_333, "the alternate screen is capped too");
    }

    #[test]
    fn non_metal_keys_are_inert() {
        let mut g = crate::globals::Globals::empty(std::path::Path::new("none.toml"));
        let base = Prefs::from_globals(&g);
        assert_eq!(base, Prefs::default());
        g.set("UseAdaptiveFrameRate", toml::Value::Boolean(false));
        g.set("DisableAdaptiveFrameRateInInteractiveApps", toml::Value::Boolean(false));
        assert_eq!(Prefs::from_globals(&g), base);
        g.set("ActiveUpdateCadence", toml::Value::Float(30.0));
        g.set("metalMaximizeThroughput", toml::Value::Boolean(false));
        let p = Prefs::from_globals(&g);
        assert_eq!((p.active_update_cadence, p.maximize_throughput), (30.0, false));
    }

    #[test]
    fn typing_draws_at_once() {
        let t = Instant::now();
        let ms = Duration::from_millis;
        assert!(typing_now(1023, Some(t), t + ms(99)));
        assert!(!typing_now(1024, Some(t), t + ms(99)));
        assert!(!typing_now(10, Some(t), t + ms(100)));
        assert!(!typing_now(10, None, t));
    }

    #[test]
    fn pacer_follows_iterm2() {
        let t = Instant::now();
        let ms = Duration::from_millis;
        let mut p = Pacer::default();
        assert!(p.output(t, ms(16), false), "a new pacer draws");
        p.drew(t);
        assert!(!p.output(t + ms(5), ms(16), false));
        assert_eq!(p.due(), Some(t + ms(16)));
        assert!(!p.take_due(t + ms(10)));
        assert!(p.take_due(t + ms(16)));
        assert_eq!(p.due(), None, "taking clears it");
        assert!(p.output(t + ms(5), ms(16), true), "a typed echo draws at once");
        assert!(p.output(t + ms(20), ms(16), false));
        p.drew(t + ms(30));
        assert!(!p.output(t + ms(31), ms(16), false));
        p.drew(t + ms(32));
        assert_eq!(p.due(), None, "a frame drawn clears the due time");
        assert!(!p.take_due(t + ms(100)));
    }

    #[test]
    fn window_interval_is_the_smallest() {
        let p = Prefs::default();
        let busy = State { throughput: 50_000, ..state() };
        let quiet = State { idle: true, ..state() };
        assert_eq!(us(window_interval(&[busy, quiet], &p)), 32_333);
        assert_eq!(us(window_interval(&[busy, state()], &p)), 15_667);
        assert_eq!(us(window_interval(&[], &p)), 999_000, "no visible pane: background");
    }

    /// `iTermMTKView.swift:58-91`: a view redraws at least every `MetalRedrawPeriod`.
    #[test]
    fn redraw_period_follows_iterm2() {
        let t = Instant::now();
        let ms = Duration::from_millis;
        assert_eq!(period_due(Some(t), ms(500), t + ms(100)), Some(t + ms(500)));
        assert_eq!(period_due(None, ms(500), t), Some(t), "never drawn: draw");
        assert_eq!(period_due(Some(t), ms(0), t + ms(100)), None, "a period of 0 never forces a frame");
        assert_eq!(period_due(Some(t), Duration::ZERO, t), None);
        // The pacer's last frame feeds it.
        let mut p = Pacer::default();
        assert_eq!(p.last(), None);
        p.drew(t);
        assert_eq!(p.last(), Some(t));
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

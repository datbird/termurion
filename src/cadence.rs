//! How often Instant Replay may record a pane: iTerm2 saves a frame at each screen update
//! (`TV:2910-2917`), and the update timer's period follows the session
//! (`iTermUpdateCadenceController.m`, `UC` below). Pure, so the rules are tested here.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::globals::Globals;

/// A visible, busy session at a low byte rate: 60 frames a second (`activeUpdateCadence` and
/// `maximumFrameRate`, 60).
pub const FAST_GAP: Duration = Duration::from_millis(16);
/// A visible session receiving at least `THROUGHPUT_LIMIT` bytes a second. Termurion draws on
/// the GPU like iTerm2's default Metal path, so this is `metalSlowFrameRate` (30 fps,
/// `iTermAdvancedSettingsModel.m:570`), not the non-Metal `slowFrameRate` (15).
pub const SLOW_GAP: Duration = Duration::from_nanos(1_000_000_000 / 30);
/// A session in no visible tab, or idle: `kBackgroundUpdateCadence` (`UC:20`).
pub const BACKGROUND_GAP: Duration = Duration::from_secs(1);
/// `adaptiveFrameRateThroughputThreshold` (10000 bytes a second).
pub const THROUGHPUT_LIMIT: i64 = 10_000;
/// `isIdle` (`PS:6584-6587`): a session is idle after no output for more than its
/// `idleTimeSeconds` plus one second.
pub fn idle_after(idle: Duration) -> Duration {
    idle + Duration::from_secs(1)
}

/// `idleTimeSeconds` (`AS:488`, default 2), read once when a session starts (`PS:849`); a
/// missing, negative or non-finite value is the default.
pub fn idle_time(seconds: Option<f64>) -> Duration {
    match seconds {
        Some(s) if s.is_finite() && s >= 0.0 => Duration::try_from_secs_f64(s).unwrap_or(crate::activity::DEFAULT_IDLE),
        _ => crate::activity::DEFAULT_IDLE,
    }
}

/// `frame_gap` with iTerm2's defaults, on the visible, normal-power path: what Instant
/// Replay's tests were written against.
#[allow(dead_code)]
pub fn frame_gap(visible: bool, idle: bool, alt_screen: bool, throughput: i64) -> Duration {
    interval(State { active: false, idle, visible, alt_screen, throughput, low_power: false, worker_parses: false, display_hz: None }, &Prefs::default())
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
    /// Termurion's `ParseWhileDrawing`: output is parsed while a frame draws, so heavy output
    /// draws at `maximum_frame_rate` instead of `metal_slow_frame_rate`. Off in `default()`,
    /// which is iTerm2's cadence; the setting itself is on by default.
    pub parse_while_drawing: bool,
    /// Termurion's `MatchDisplayRefreshRate`: the 60 fps rates follow the display. Off in
    /// `default()` (iTerm2's cadence); the setting itself is on by default.
    pub match_display: bool,
}

impl Default for Prefs {
    fn default() -> Prefs {
        Prefs { maximize_throughput: true, active_update_cadence: 60.0, maximum_frame_rate: 60.0, metal_slow_frame_rate: 30.0, threshold: THROUGHPUT_LIMIT, low_power_frame_rate: 30.0, parse_while_drawing: false, match_display: false }
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
            parse_while_drawing: g.bool("ParseWhileDrawing"),
            match_display: g.bool("MatchDisplayRefreshRate"),
        }
    }
}

/// `iTermUpdateCadenceState` for one pane.
#[derive(Clone, Copy, Debug, PartialEq)]
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
    /// Parse While Drawing can take this pane's output (`Pane::parses_alone`): its flood may
    /// draw at the full rate. A tmux, SSH integration or coprocess pane, and Instant Replay's
    /// capture, keep the slow rate.
    pub worker_parses: bool,
    /// The refresh rate of the monitor the window is on, when the OS says. Instant Replay's
    /// capture leaves it `None`: it records at the cadence, not the display's rate.
    pub display_hz: Option<f64>,
}

/// The rate a 60 fps setting draws at: the display's (30 to 240) while the setting still has
/// its default of 60 and Match Display Refresh Rate is on. A value set by hand wins.
fn matched(setting: f64, display_hz: Option<f64>, p: &Prefs) -> f64 {
    match display_hz {
        Some(hz) if p.match_display && setting == 60.0 && hz.is_finite() => hz.round().clamp(30.0, 240.0),
        _ => setting,
    }
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
    let rate = |fps: f64| 1.0 / matched(fps, s.display_hz, p).max(1.0);
    let low_power = |secs: f64| if s.low_power { secs.max(1.0 / p.low_power_frame_rate.max(1.0)) } else { secs };
    let secs = if !(s.active || !s.idle) || !s.visible {
        1.0
    } else if !p.maximize_throughput || s.alt_screen {
        low_power(rate(p.active_update_cadence))
    } else if s.throughput > 0 && s.throughput < p.threshold {
        low_power(rate(p.maximum_frame_rate))
    } else if p.parse_while_drawing && s.worker_parses {
        low_power(rate(p.maximum_frame_rate))
    } else {
        low_power(1.0 / p.metal_slow_frame_rate)
    };
    Duration::from_secs_f64(secs.max(0.002) - 0.001)
}

/// The shortest gap `interval` gives any state under `p` on a display of `display_hz` (low power
/// only lengthens it).
pub fn shortest_interval(p: &Prefs, display_hz: Option<f64>) -> Duration {
    let rate = |fps: f64| 1.0 / matched(fps, display_hz, p).max(1.0);
    // Each branch's gap as `interval` finishes it, so an odd setting (NaN) floors the same way.
    // The adaptive rates only count with Maximize Throughput on.
    // Parse While Drawing gives heavy output `maximum_frame_rate`, already counted, but only in
    // panes the worker parses: the others keep `metal_slow_frame_rate`.
    let adaptive = if p.maximize_throughput { [rate(p.maximum_frame_rate), 1.0 / p.metal_slow_frame_rate] } else { [1.0, 1.0] };
    [1.0, rate(p.active_update_cadence)]
        .into_iter()
        .chain(adaptive)
        .map(|secs: f64| Duration::from_secs_f64(secs.max(0.002) - 0.001))
        .min()
        .unwrap_or_default()
}

/// A window has one surface, so it draws at the fastest rate any of its visible panes asks for
/// (none: the background period).
pub fn window_interval(states: &[State], p: &Prefs) -> Duration {
    states.iter().map(|s| interval(*s, p)).min().unwrap_or_else(|| interval(State { active: false, idle: true, visible: false, alt_screen: false, throughput: 0, low_power: false, worker_parses: false, display_hz: None }, p))
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
        State { active: false, idle: false, visible: true, alt_screen: false, throughput: 500, low_power: false, worker_parses: true, display_hz: None }
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

    /// The replay recorder skips a pane captured less than this ago without working out its
    /// gap, so no state may give a shorter one.
    #[test]
    fn no_state_is_faster_than_the_shortest_interval() {
        let base = Prefs::default();
        let prefs = [
            base,
            Prefs { maximum_frame_rate: 120.0, ..base },
            Prefs { active_update_cadence: 240.0, ..base },
            Prefs { metal_slow_frame_rate: 500.0, ..base },
            Prefs { metal_slow_frame_rate: f64::NAN, ..base },
            Prefs { maximum_frame_rate: 100_000.0, maximize_throughput: false, ..base },
            Prefs { active_update_cadence: 0.0, maximum_frame_rate: 0.5, metal_slow_frame_rate: 2.0, ..base },
            Prefs { metal_slow_frame_rate: 500.0, parse_while_drawing: true, ..base },
            Prefs { match_display: true, ..base },
            Prefs { match_display: true, parse_while_drawing: true, ..base },
        ];
        for (p, hz) in prefs.into_iter().flat_map(|p| [None, Some(120.0), Some(1000.0)].map(|hz| (p, hz))) {
            let floor = shortest_interval(&p, hz);
            let mut fastest = Duration::MAX;
            for bits in 0..64u32 {
                for throughput in [0, 500, 10_000, 50_000] {
                    let s = State { active: bits & 1 != 0, idle: bits & 2 != 0, visible: bits & 4 != 0, alt_screen: bits & 8 != 0, throughput, low_power: bits & 16 != 0, worker_parses: bits & 32 != 0, display_hz: hz };
                    fastest = fastest.min(interval(s, &p));
                }
            }
            assert!(floor <= fastest, "{p:?}: floor {floor:?} above the fastest state {fastest:?}");
            assert_eq!(floor, fastest, "{p:?}: the floor is the fastest state");
        }
    }

    #[test]
    fn a_flood_draws_at_the_full_rate_only_with_the_switch() {
        let s = State { throughput: 50_000, ..state() };
        let on = Prefs { parse_while_drawing: true, ..Prefs::default() };
        assert_eq!(us(interval(s, &on)), 15_667, "MaximumFrameRate while the worker parses");
        assert_eq!(us(interval(s, &Prefs { parse_while_drawing: false, ..on })), 32_333, "MetalSlowFrameRate without it");
        assert_eq!(us(interval(State { low_power: true, ..s }, &on)), 32_333, "battery saver still caps it");
        assert_eq!(us(interval(State { worker_parses: false, ..s }, &on)), 32_333, "a pane the worker cannot parse keeps MetalSlowFrameRate");
        let window = [State { worker_parses: false, ..s }, State { throughput: 0, idle: true, ..s }];
        assert_eq!(us(window_interval(&window, &on)), 32_333, "a tmux flood beside an idle plain pane");
        let mut g = crate::globals::Globals::empty(std::path::Path::new("none.toml"));
        assert!(Prefs::from_globals(&g).parse_while_drawing, "on by default");
        g.set("ParseWhileDrawing", toml::Value::Boolean(false));
        assert!(!Prefs::from_globals(&g).parse_while_drawing);
    }

    fn on(hz: Option<f64>) -> (Prefs, State) {
        (Prefs { match_display: true, ..Prefs::default() }, State { display_hz: hz, ..state() })
    }

    #[test]
    fn a_120_hz_display_draws_at_120() {
        let (p, s) = on(Some(120.0));
        assert_eq!(us(interval(s, &p)), 7_333, "1/120 s minus 1 ms");
        let alt = State { alt_screen: true, ..s };
        assert_eq!(us(interval(alt, &p)), 7_333, "ActiveUpdateCadence follows too");
        assert_eq!(us(interval(State { display_hz: Some(1000.0), ..s }, &p)), 3_167, "clamped to 240");
        assert_eq!(us(interval(State { display_hz: Some(10.0), ..s }, &p)), 32_333, "clamped to 30");
        assert_eq!(shortest_interval(&p, Some(120.0)), interval(s, &p));
    }

    #[test]
    fn a_flood_on_120_hz_draws_at_120_with_parse_while_drawing() {
        let (p, s) = on(Some(120.0));
        let flood = State { throughput: 50_000, ..s };
        assert_eq!(us(interval(flood, &p)), 32_333, "without the switch a flood stays at 30");
        let p = Prefs { parse_while_drawing: true, ..p };
        assert_eq!(us(interval(flood, &p)), 7_333);
        assert_eq!(us(interval(State { worker_parses: false, ..flood }, &p)), 32_333, "a pane the worker cannot parse");
        let slow = Prefs { metal_slow_frame_rate: 20.0, ..p };
        assert_eq!(us(interval(State { worker_parses: false, ..flood }, &slow)), 49_000, "MetalSlowFrameRate never follows");
    }

    #[test]
    fn battery_saver_keeps_30_on_any_display() {
        for hz in [60.0, 120.0, 240.0] {
            let (p, s) = on(Some(hz));
            let s = State { low_power: true, ..s };
            assert_eq!(us(interval(s, &p)), 32_333, "{hz} Hz");
            assert_eq!(us(interval(State { alt_screen: true, ..s }, &p)), 32_333, "{hz} Hz alt screen");
            let flood = Prefs { parse_while_drawing: true, ..p };
            assert_eq!(us(interval(State { throughput: 50_000, ..s }, &flood)), 32_333, "{hz} Hz flood");
        }
    }

    #[test]
    fn a_hand_set_maximum_wins() {
        let (p, s) = on(Some(120.0));
        assert_eq!(us(interval(s, &Prefs { maximum_frame_rate: 45.0, ..p })), 21_222);
        let flood = State { throughput: 50_000, ..s };
        assert_eq!(us(interval(flood, &Prefs { maximum_frame_rate: 45.0, parse_while_drawing: true, ..p })), 21_222);
        // ActiveUpdateCadence is its own setting: set by hand, it wins on its own.
        let alt = State { alt_screen: true, ..s };
        assert_eq!(us(interval(alt, &Prefs { active_update_cadence: 30.0, ..p })), 32_333);
    }

    #[test]
    fn a_hand_set_low_power_rate_never_follows_the_display() {
        let (p, s) = on(Some(120.0));
        let low = State { low_power: true, ..s };
        // Battery saver at a hand-set 60 on a 120 Hz display: 60, not 120 (and not faster than that).
        assert_eq!(us(interval(low, &Prefs { low_power_frame_rate: 60.0, ..p })), 15_667);
        assert_eq!(us(interval(State { alt_screen: true, ..low }, &Prefs { low_power_frame_rate: 60.0, ..p })), 15_667);
        assert_eq!(us(interval(low, &Prefs { low_power_frame_rate: 120.0, ..p })), 7_333);
    }

    #[test]
    fn a_fractional_rate_rounds_to_the_nearest_hz() {
        let p = Prefs { match_display: true, ..Prefs::default() };
        let hz = |hz: f64| interval(State { display_hz: Some(hz), ..state() }, &p);
        assert_eq!(hz(59.94), hz(60.0));
        assert_eq!(us(hz(59.94)), 15_667);
        assert_eq!(hz(119.88), hz(120.0));
        assert_eq!(shortest_interval(&p, Some(59.94)), shortest_interval(&p, Some(60.0)));
    }

    #[test]
    fn no_display_rate_keeps_60() {
        let (p, s) = on(None);
        assert_eq!(us(interval(s, &p)), 15_667);
        assert_eq!(shortest_interval(&p, None), interval(s, &p));
    }

    #[test]
    fn the_switch_off_keeps_60() {
        let s = State { display_hz: Some(120.0), ..state() };
        assert_eq!(us(interval(s, &Prefs::default())), 15_667);
        let mut g = crate::globals::Globals::empty(std::path::Path::new("none.toml"));
        assert!(Prefs::from_globals(&g).match_display, "on by default");
        g.set("MatchDisplayRefreshRate", toml::Value::Boolean(false));
        assert!(!Prefs::from_globals(&g).match_display);
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
        // Termurion's own switch is on by default; `Prefs::default()` is iTerm2's cadence.
        g.set("ParseWhileDrawing", toml::Value::Boolean(false));
        g.set("MatchDisplayRefreshRate", toml::Value::Boolean(false));
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

    /// `idleTimeSeconds` (`AS:488`, `PS:849`): a missing, negative or non-finite value is the
    /// default 2 s; a pane is idle after that plus one second (`PS:6586`).
    #[test]
    fn idle_time_reads_the_setting() {
        assert_eq!(idle_time(Some(0.5)), Duration::from_millis(500));
        assert_eq!(idle_time(Some(10.0)), Duration::from_secs(10));
        for bad in [None, Some(f64::NAN), Some(-1.0), Some(f64::INFINITY)] {
            assert_eq!(idle_time(bad), Duration::from_secs(2), "{bad:?}");
        }
        assert_eq!(idle_after(Duration::from_secs(2)), Duration::from_secs(3));
        assert_eq!(idle_after(Duration::from_millis(500)), Duration::from_millis(1500));
    }
}

//! iTerm2's status bar metrics math (milestone 9i, Task 2): the CPU/memory/network/
//! battery samplers' formulas, the byte formatters, the battery texts, and the sampling
//! schedule over a `Samplers` trait. Pure code: no Win32. `metrics_win.rs` (Task 3)
//! implements `Samplers` with Win32 calls; `src/app/metrics_ui.rs` (Task 3) owns a
//! `Meters` per pane and calls `tick` from the status bar's own per-frame clock.
//!
//! Citations use the abbreviations from the milestone 9i brief's Resume notes, paths under
//! `~/gitrepos/iterm2-ref/sources/`: `PUB` = `Infrastructure/iTermPublisher.m`, `CPUS` =
//! `StatusBar/Components/iTermCPUUtilization.m`, `MEMS` =
//! `StatusBar/Components/iTermMemoryUtilization.m`, `NETS` =
//! `StatusBar/Components/iTermNetworkUtilization.m`, `BAT` =
//! `StatusBar/Components/iTermStatusBarBatteryComponent.m`, `PWR` =
//! `System/iTermPowerManager.m`, `CPU`/`MEM`/`NET` = the matching
//! `iTermStatusBar*UtilizationComponent.m` files. `format_bytes`/`format_bytes_compact`
//! cite `Categories/NSStringITerm.m:2265-2331`; `duration_string` cites
//! `Categories/NSDateFormatterExtras.m:125-136`.

use std::collections::{BTreeSet, VecDeque};
use std::time::{Duration, Instant};

/// `PUB`'s publisher capacity for every metrics source (`CPUS:30`, `MEMS`, `NETS:76`,
/// `PWR`).
pub const HISTORY: usize = 120;
/// The 1 s repeating timer every metrics source but Battery samples on (`CPUS:53-58`,
/// `MEMS`, `NETS:158-161`).
pub const CADENCE: Duration = Duration::from_secs(1);
/// Battery's own repeating cadence (`PWR:224-227` approximately; the Resume notes give the
/// value directly).
pub const BATTERY_CADENCE: Duration = Duration::from_secs(60);

/// A metric's rolling history: the last `HISTORY` samples, oldest first.
#[derive(Clone, Debug, Default)]
pub struct Series {
    values: VecDeque<f64>,
}

impl Series {
    pub fn new() -> Self {
        Series { values: VecDeque::new() }
    }

    pub fn push(&mut self, value: f64) {
        self.values.push_back(value);
        while self.values.len() > HISTORY {
            self.values.pop_front();
        }
    }

    pub fn values(&self) -> Vec<f64> {
        self.values.iter().copied().collect()
    }

    pub fn last(&self) -> Option<f64> {
        self.values.back().copied()
    }
}

/// One CPU tick reading: idle ticks and the total of every state (`CPUS:18-20`). Windows'
/// `metrics_win.rs` fills this from `GetSystemTimes`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CpuTicks {
    pub idle: f64,
    pub total: f64,
}

/// CPU utilization: `1 - idle/total` of the ticks since the previous sample.
#[derive(Clone, Debug, Default)]
pub struct CpuMeter {
    pub samples: Series,
    last: CpuTicks,
}

impl CpuMeter {
    pub fn new() -> Self {
        CpuMeter::default()
    }

    /// `CPUS:66-86`: `delta = current - _last; _last = current; value = (_last.total == 0)
    /// ? 0 : 1 - delta.idle / delta.total`, published unless it is NaN. `_last` is
    /// reassigned to `current` BEFORE the zero check, so the check is against the reading
    /// JUST TAKEN, not the prior one: a failed read (`current` is `None`, treated as ticks
    /// 0,0 per `-sample`'s own failure return, `CPUS:88-104`) always makes `_last.total`
    /// zero afterward, so it always publishes exactly 0 and resets the baseline; the next
    /// real reading after that again publishes its own since-boot average. Correction
    /// beyond the Resume notes: the brief's failed-read test bullet gives a nonzero
    /// formula, but the source (both reassignment order and the zero check target) gives
    /// 0 every time; this file follows the source, per the implementer rules.
    /// True when a value was published.
    pub fn sample(&mut self, current: Option<CpuTicks>) -> bool {
        let current = current.unwrap_or_default();
        let delta_idle = current.idle - self.last.idle;
        let delta_total = current.total - self.last.total;
        self.last = current;
        let value = if self.last.total == 0.0 { 0.0 } else { 1.0 - delta_idle / delta_total };
        if value.is_nan() {
            return false;
        }
        self.samples.push(value);
        true
    }
}

/// `CPU:78-152`'s left-text smoothing: `x` starts at the last sample, then each of the
/// last `min(4, n)` samples (oldest first, so the last sample is folded in twice) updates
/// `x = x * (1 - alpha) + s * alpha`; the percent is `(int)(x * 100)`. `1.0 - ALPHA` is
/// computed, not written as the literal `0.3`: iTerm2's own `x *= (1.0 - alpha)` rounds
/// `1.0 - 0.7` to `0.30000000000000004`, one ULP off the literal, and that ULP is exactly
/// what keeps `cpu_estimate(&[0.0, 1.0])` at 79 rather than 78 (verified against the
/// source's exact operation order).
pub fn cpu_estimate(samples: &[f64]) -> i32 {
    const ALPHA: f64 = 0.7;
    let Some(&last) = samples.last() else {
        return 0;
    };
    let take = samples.len().min(4);
    let mut x = last;
    for &s in &samples[samples.len() - take..] {
        x *= 1.0 - ALPHA;
        x += s * ALPHA;
    }
    (x * 100.0) as i32
}

/// `CPU:78-91`'s padding for fewer than 60 samples: the last (up to) 60 samples,
/// left-padded with zeros to 60. The app pads only a history shorter than 60 this way and
/// then, like every graph, drops the oldest value and keeps the last 60
/// (`statusbar_sparkline::graph_values`, `SPK:196-200`), so 59 values draw until 61 are
/// stored.
pub fn cpu_graph(samples: &[f64]) -> Vec<f64> {
    let take = samples.len().min(60);
    let mut out = vec![0.0; 60 - take];
    out.extend_from_slice(&samples[samples.len() - take..]);
    out
}

/// A memory reading's fraction of physical memory used, plus the total (`MEMS:64-120`).
#[derive(Clone, Debug, Default)]
pub struct MemoryMeter {
    pub samples: Series,
    pub total: u64,
}

impl MemoryMeter {
    pub fn new() -> Self {
        MemoryMeter::default()
    }

    /// `MEMS:64-120`: fraction = used / physical memory; a failed read (`None`) is used 0,
    /// over whatever total was last known (a failed read does not report a total either).
    /// Always publishes (a failed read is used 0); true for symmetry with the others.
    pub fn sample(&mut self, used_total: Option<(u64, u64)>) -> bool {
        match used_total {
            Some((used, total)) => {
                self.total = total;
                let fraction = if total == 0 { 0.0 } else { used as f64 / total as f64 };
                self.samples.push(fraction);
            }
            None => self.samples.push(0.0),
        }
        true
    }
}

/// `MEM:64-123`: `""` when `(long long)(last * total)` is 0, else `format_bytes` of it.
pub fn memory_text(last: Option<f64>, total: u64) -> String {
    let bytes = (last.unwrap_or(0.0) * total as f64) as i64;
    if bytes == 0 {
        String::new()
    } else {
        format_bytes(bytes as f64)
    }
}

/// `NSStringITerm.m:2265-2296`: the first limit `bytes` is under, 1000-based, each with its
/// own divisor and decimal count. iTerm2's rounding at the 1M boundary (999999 bytes prints
/// as `"1000 kB"`) is kept, not fixed.
pub fn format_bytes(bytes: f64) -> String {
    const K: f64 = 1000.0;
    const MB: f64 = K * K;
    const GB: f64 = MB * K;
    const TB: f64 = GB * K;
    const PB: f64 = TB * K;
    // (limit, divisor, decimals, suffix).
    let units: [(f64, f64, usize, &str); 12] = [
        (1.0, 1.0, 0, "bytes"),
        (K, 1.0, 0, "bytes"),
        (10.0 * K, K, 1, "kB"),
        (MB, K, 0, "kB"),
        (10.0 * MB, MB, 1, "MB"),
        (GB, MB, 0, "MB"),
        (10.0 * GB, GB, 1, "GB"),
        (TB, GB, 0, "GB"),
        (10.0 * TB, TB, 1, "TB"),
        (PB, TB, 0, "TB"),
        (10.0 * PB, PB, 1, "PB"),
        (K * PB, PB, 0, "PB"),
    ];
    for (limit, divisor, decimals, suffix) in units {
        if bytes < limit {
            let v = bytes / divisor;
            return if decimals == 0 { format!("{:.0} {}", v, suffix) } else { format!("{:.1} {}", v, suffix) };
        }
    }
    "\u{221E}".to_string()
}

/// `NSStringITerm.m:2298-2331`: the compact, fixed-width form the graphs' left/right text
/// uses. Each unit has three buckets (one decimal, then two space-padded whole-number
/// widths); the fallback is five spaces plus the infinity sign.
pub fn format_bytes_compact(bytes: f64) -> String {
    const K: f64 = 1000.0;
    const MB: f64 = K * K;
    const GB: f64 = MB * K;
    const TB: f64 = GB * K;
    // (limit, divisor, width, decimals, suffix); width 0 means no minimum width.
    let units: [(f64, f64, usize, usize, &str); 12] = [
        (10.0 * K, K, 0, 1, "kB"),
        (100.0 * K, K, 2, 0, "kB"),
        (MB, K, 3, 0, "kB"),
        (10.0 * MB, MB, 0, 1, "MB"),
        (100.0 * MB, MB, 2, 0, "MB"),
        (GB, MB, 3, 0, "MB"),
        (10.0 * GB, GB, 0, 1, "GB"),
        (100.0 * GB, GB, 2, 0, "GB"),
        (TB, GB, 3, 0, "GB"),
        (10.0 * TB, TB, 0, 1, "TB"),
        (100.0 * TB, TB, 2, 0, "TB"),
        (TB * K, TB, 3, 0, "TB"),
    ];
    for (limit, divisor, width, decimals, suffix) in units {
        if bytes < limit {
            let v = bytes / divisor;
            let number = match (width, decimals) {
                (0, 1) => format!("{:.1}", v),
                (2, 0) => format!(" {:2.0}", v),
                (3, 0) => format!("{:3.0}", v),
                _ => unreachable!(),
            };
            return format!("{} {}", number, suffix);
        }
    }
    "     \u{221E}".to_string()
}

/// One network reading: cumulative down/up bytes and the interface set that produced them
/// (`NETS:69-97`; the set's identity is name plus MAC on macOS, LUID plus MAC on Windows,
/// `metrics_win.rs`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NetCounters {
    pub down: f64,
    pub up: f64,
    pub interfaces: BTreeSet<Vec<u8>>,
}

/// Network throughput: down/up bytes-per-second rate pairs.
#[derive(Clone, Debug, Default)]
pub struct NetMeter {
    pub samples: VecDeque<(f64, f64)>,
    last: Option<(NetCounters, Instant)>,
}

impl NetMeter {
    pub fn new() -> Self {
        NetMeter::default()
    }

    /// `NETS:98-172`, `PUB:154-156`. `current` `None` (a failed read) publishes nothing and
    /// keeps the stored base (`NETS:104-106`: the early return happens before `_last` is
    /// updated). With no previous sample the rate is 0 (iTerm2 divides by
    /// `timeIntervalSinceLastUpdate`'s `INFINITY`, `PUB:154-156`, so a real delta over an
    /// infinite time is 0 too; this is written as an explicit branch for clarity). When the
    /// interface set changed since the last real sample, the previous published rate pair
    /// (or 0,0 with none yet) is republished instead of a real delta, so a VPN connecting
    /// or an adapter appearing never spikes (`NETS:104-114`); either way `current` becomes
    /// the new base. Rates are not clamped: a counter reset gives a negative rate, kept.
    /// True when a rate pair was published.
    pub fn sample(&mut self, current: Option<NetCounters>, now: Instant) -> bool {
        let Some(current) = current else {
            return false;
        };
        let rates = match &self.last {
            None => (0.0, 0.0),
            Some((last, last_time)) => {
                if last.interfaces != current.interfaces {
                    self.samples.back().copied().unwrap_or((0.0, 0.0))
                } else {
                    let secs = now.duration_since(*last_time).as_secs_f64();
                    ((current.down - last.down) / secs, (current.up - last.up) / secs)
                }
            }
        };
        self.last = Some((current, now));
        self.samples.push_back(rates);
        while self.samples.len() > HISTORY {
            self.samples.pop_front();
        }
        true
    }
}

/// Both series' combined ceiling over the whole stored history (`NET:139-141,203-206`;
/// corrections to the research: over all up to 120 stored samples, not the 60 drawn).
pub fn net_ceiling(samples: &VecDeque<(f64, f64)>) -> f64 {
    samples.iter().fold(0.0_f64, |m, &(d, u)| m.max(d).max(u))
}

/// One power reading (`PWR:148-212`). `time` is seconds, -1 when unknown.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PowerState {
    pub percentage: Option<i32>,
    pub charging: bool,
    pub time: f64,
}

/// Battery history and the once-per-process has-a-battery fact (`BAT:59-79`).
#[derive(Clone, Debug, Default)]
pub struct BatteryMeter {
    pub states: VecDeque<PowerState>,
    pub has_battery: Option<bool>,
}

impl BatteryMeter {
    pub fn new() -> Self {
        BatteryMeter::default()
    }

    /// An AC change or a wake only redraws; they publish no sample, so `None` here pushes
    /// nothing (`BAT:245-247`, `PWR:96-106,277-279`).
    /// True when a state was published.
    pub fn sample(&mut self, state: Option<PowerState>) -> bool {
        let Some(state) = state else { return false };
        self.states.push_back(state);
        while self.states.len() > HISTORY {
            self.states.pop_front();
        }
        true
    }
}

/// `NSDateFormatterExtras.m:125-136`, English separator only (no locale support here since
/// Tandem has no non-English UI yet).
pub fn duration_string(seconds: f64) -> String {
    let total_seconds = seconds as i64; // C `int` truncates toward zero.
    let minutes = total_seconds / 60;
    let hours = minutes / 60;
    let remainder_minutes = minutes - hours * 60;
    format!("{}:{:02}", hours, remainder_minutes)
}

/// What the Battery component draws (`BAT:110-234`).
#[derive(Clone, Debug, PartialEq)]
pub struct BatteryView {
    pub left: String,
    pub right: String,
    pub left_reserve: &'static str,
    pub right_reserve: &'static str,
    pub right_icon_reserved: bool,
    pub charging_icon: bool,
}

/// `BAT:110-234`. `current` is the live power state (`None` when there is none at all);
/// `connected` is AC power present; `has_battery` is the once-per-process machine check;
/// `show_percentage`/`show_time` are the `ShowPercentage`/`ShowTime` knobs.
///
/// `isCharging` = `has_battery && connected` (`BAT:184-186`). `showTime` is NO when the
/// estimate is 100 and charging, or the state's time is under 1 s, or the state's own
/// `charging` disagrees with `isCharging`; else the knob (`BAT:203-217`). Left reserve is
/// `"100%"` only when `show_percentage` (the time-as-left-text gets no left inset, kept,
/// `BAT:160-166`). Right reserve reserves the charging icon's room even when not charging,
/// kept (`BAT:176-183`).
pub fn battery_view(current: Option<&PowerState>, connected: bool, has_battery: bool, show_percentage: bool, show_time: bool) -> BatteryView {
    let is_charging = has_battery && connected;
    // `currentEstimate` reads the percentage as 0 when unset (`doubleValue` on nil is 0,
    // `BAT:132-135`); only reachable here when `current` is `Some` with no percentage.
    let estimate = current.and_then(|s| s.percentage).unwrap_or(0);
    let has_percentage = current.is_some_and(|s| s.percentage.is_some());

    let show_time_computed = match current {
        None => false,
        Some(state) => {
            if estimate == 100 && is_charging {
                false
            } else if state.time < 1.0 {
                false
            } else if state.charging != is_charging {
                false
            } else {
                show_time
            }
        }
    };
    let show_time_on_right = show_time_computed && show_percentage;

    let left = if !has_percentage {
        String::new()
    } else if show_percentage {
        format!("{}%", estimate)
    } else if show_time_computed {
        duration_string(current.unwrap().time)
    } else {
        String::new()
    };
    let right = if !has_percentage {
        String::new()
    } else if show_time_on_right {
        duration_string(current.unwrap().time)
    } else {
        String::new()
    };

    let left_reserve = if show_percentage { "100%" } else { "" };
    let (right_reserve, right_icon_reserved) = if show_time_on_right { ("55:55", is_charging) } else { ("", true) };

    BatteryView { left, right, left_reserve, right_reserve, right_icon_reserved, charging_icon: is_charging }
}

/// What sampling wants running right now. Set by the app from which of the four graph
/// kinds are on screen (Task 3).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Wanted {
    pub cpu: bool,
    pub memory: bool,
    pub network: bool,
    pub battery: bool,
}

/// Reads live data for one metric each; `Samplers` so `metrics_win.rs`'s Win32
/// implementation can be swapped for a fake in tests. `None` is a failed read.
pub trait Samplers {
    fn cpu(&mut self) -> Option<CpuTicks>;
    fn memory(&mut self) -> Option<(u64, u64)>;
    fn network(&mut self, exclude_tunnels: bool) -> Option<NetCounters>;
    fn power(&mut self) -> Option<PowerState>;
    fn on_ac(&mut self) -> bool;
}

/// The four meters plus the AC line, sampled on iTerm2's own schedule
/// (`CPUS:38-107`, `MEMS:36-136`, `NETS:69-190`, `PWR:215-273`, Resume notes' Samplers and
/// schedule section): Memory and Network sample at once when first wanted, then every
/// `CADENCE`; CPU takes no sample on first wanted, only after `CADENCE` has elapsed;
/// Battery samples at once, then every `BATTERY_CADENCE`; the AC line is read every
/// `CADENCE` while Battery is wanted, independent of Battery's own 60 s cadence, and a
/// change alone is reported without forcing a new Battery sample. An unwanted meter's
/// schedule goes idle (its stored history is untouched).
/// The last publish time of each meter.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sampled {
    pub cpu: Option<Instant>,
    pub memory: Option<Instant>,
    pub network: Option<Instant>,
    pub battery: Option<Instant>,
}

#[derive(Debug, Default)]
pub struct Meters {
    pub cpu: CpuMeter,
    pub memory: MemoryMeter,
    pub network: NetMeter,
    pub battery: BatteryMeter,
    pub on_ac: bool,
    /// When each meter last published a sample: each graph's slide animation starts from
    /// its own (`SPK:158-251`: a component animates when its own series updates). An AC
    /// line change sets none (`BAT:245-247`: it only redraws).
    pub sampled: Sampled,
    cpu_next: Option<Instant>,
    memory_next: Option<Instant>,
    network_next: Option<Instant>,
    battery_next: Option<Instant>,
    ac_next: Option<Instant>,
}

impl Meters {
    pub fn new() -> Self {
        Meters::default()
    }

    /// Runs whichever of the four schedules are due, and the AC line's while Battery is
    /// wanted. Returns true when a new sample was published or the AC line changed (the
    /// app's cue to redraw).
    pub fn tick(&mut self, now: Instant, wanted: Wanted, exclude_tunnels: bool, s: &mut dyn Samplers) -> bool {
        let mut changed = false;

        if wanted.memory {
            if self.memory_next.is_none_or(|due| now >= due) {
                let v = s.memory();
                if self.memory.sample(v) {
                    self.sampled.memory = Some(now);
                }
                self.memory_next = Some(now + CADENCE);
                changed = true;
            }
        } else {
            self.memory_next = None;
        }

        if wanted.network {
            if self.network_next.is_none_or(|due| now >= due) {
                let v = s.network(exclude_tunnels);
                if self.network.sample(v, now) {
                    self.sampled.network = Some(now);
                }
                self.network_next = Some(now + CADENCE);
                changed = true;
            }
        } else {
            self.network_next = None;
        }

        if wanted.cpu {
            match self.cpu_next {
                None => self.cpu_next = Some(now + CADENCE),
                Some(due) if now >= due => {
                    let v = s.cpu();
                    if self.cpu.sample(v) {
                        self.sampled.cpu = Some(now);
                    }
                    self.cpu_next = Some(now + CADENCE);
                    changed = true;
                }
                Some(_) => {}
            }
        } else {
            self.cpu_next = None;
        }

        if wanted.battery {
            if self.battery_next.is_none_or(|due| now >= due) {
                let v = s.power();
                if self.battery.sample(v) {
                    self.sampled.battery = Some(now);
                }
                self.battery_next = Some(now + BATTERY_CADENCE);
                changed = true;
            }
        } else {
            self.battery_next = None;
        }

        if wanted.battery {
            if self.ac_next.is_none_or(|due| now >= due) {
                let ac = s.on_ac();
                if ac != self.on_ac {
                    self.on_ac = ac;
                    changed = true;
                }
                self.ac_next = Some(now + CADENCE);
            }
        } else {
            self.ac_next = None;
        }

        changed
    }

    /// The earliest of the active schedules' due times; `None` while nothing is wanted.
    pub fn next_due(&self) -> Option<Instant> {
        [self.cpu_next, self.memory_next, self.network_next, self.battery_next, self.ac_next].into_iter().flatten().min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_follows_iterm2() {
        let mut meter = CpuMeter::new();
        // The first tick after start publishes the since-boot average, not 0
        // (`CPUS:66-86`: `_last` starts at {0,0}, so delta == current).
        meter.sample(Some(CpuTicks { idle: 30.0, total: 100.0 }));
        assert_eq!(meter.samples.values(), vec![0.7]);

        // A tick with equal ticks (delta 0/0) publishes nothing (NaN).
        meter.sample(Some(CpuTicks { idle: 30.0, total: 100.0 }));
        assert_eq!(meter.samples.values(), vec![0.7]);

        // A real delta after that publishes normally.
        meter.sample(Some(CpuTicks { idle: 40.0, total: 200.0 }));
        // delta = (10, 100); value = 1 - 10/100 = 0.9.
        assert_eq!(meter.samples.values(), vec![0.7, 0.9]);

        // A failed read (None) is ticks 0,0; `_last` becomes {0,0} too (total == 0), so the
        // source publishes exactly 0 (a correction to the brief's failed-read formula; see
        // this file's `CpuMeter::sample` doc comment).
        meter.sample(None);
        assert_eq!(meter.samples.values(), vec![0.7, 0.9, 0.0]);

        // After a failed read the baseline is zeroed, so the next real reading again
        // publishes its own since-boot average.
        meter.sample(Some(CpuTicks { idle: 5.0, total: 20.0 }));
        assert_eq!(meter.samples.last(), Some(0.75));

        assert_eq!(cpu_estimate(&[0.5]), 50);
        assert_eq!(cpu_estimate(&[0.0, 1.0]), 79);
        assert_eq!(cpu_estimate(&[0.2, 0.4, 0.6, 0.8, 1.0]), 92);

        assert_eq!(cpu_graph(&[0.1, 0.2, 0.3]), {
            let mut v = vec![0.0; 57];
            v.extend_from_slice(&[0.1, 0.2, 0.3]);
            v
        });
    }

    #[test]
    fn memory_follows_iterm2() {
        let mut meter = MemoryMeter::new();
        meter.sample(Some((3_100_000_000, 8_000_000_000)));
        assert_eq!(meter.samples.last(), Some(0.3875));
        assert_eq!(memory_text(meter.samples.last(), meter.total), "3.1 GB");

        assert_eq!(memory_text(Some(0.0), 8_000_000_000), "");

        meter.sample(None);
        assert_eq!(meter.samples.last(), Some(0.0));
    }

    #[test]
    fn bytes_format_like_iterm2() {
        assert_eq!(format_bytes(0.0), "0 bytes");
        assert_eq!(format_bytes(999.0), "999 bytes");
        assert_eq!(format_bytes(1500.0), "1.5 kB");
        // iTerm2's own rounding at the 1M boundary, kept.
        assert_eq!(format_bytes(999999.0), "1000 kB");
        assert_eq!(format_bytes(3.1e9), "3.1 GB");
        assert_eq!(format_bytes(12e9), "12 GB");
        assert_eq!(format_bytes(1e18), "\u{221E}");

        assert_eq!(format_bytes_compact(0.0), "0.0 kB");
        assert_eq!(format_bytes_compact(5000.0), "5.0 kB");
        assert_eq!(format_bytes_compact(55000.0), " 55 kB");
        assert_eq!(format_bytes_compact(555000.0), "555 kB");
        assert_eq!(format_bytes_compact(2.5e6), "2.5 MB");
        assert_eq!(format_bytes_compact(1e15), "     \u{221E}");
    }

    #[test]
    fn network_rates_follow_iterm2() {
        let mut meter = NetMeter::new();
        let t0 = Instant::now();
        let mut interfaces = BTreeSet::new();
        interfaces.insert(b"eth0".to_vec());

        // The first sample publishes (0, 0); equivalently, with no previous sample.
        meter.sample(Some(NetCounters { down: 1_000_000.0, up: 500_000.0, interfaces: interfaces.clone() }), t0);
        assert_eq!(meter.samples.back(), Some(&(0.0, 0.0)));

        // 1 s later, 1000 more bytes down and 500 up: (1000, 500).
        let t1 = t0 + Duration::from_secs(1);
        meter.sample(Some(NetCounters { down: 1_001_000.0, up: 500_500.0, interfaces: interfaces.clone() }), t1);
        assert_eq!(meter.samples.back(), Some(&(1000.0, 500.0)));

        // A sample whose interface set differs republishes the previous rates (1000, 500)
        // and becomes the new base.
        let mut interfaces2 = BTreeSet::new();
        interfaces2.insert(b"eth0".to_vec());
        interfaces2.insert(b"vpn0".to_vec());
        let t2 = t1 + Duration::from_secs(1);
        meter.sample(Some(NetCounters { down: 999_999_999_999.0, up: 999_999_999_999.0, interfaces: interfaces2.clone() }), t2);
        assert_eq!(meter.samples.back(), Some(&(1000.0, 500.0)));

        // A failed read publishes nothing and keeps the base: the next real sample against
        // the same (now t2) base still reads as a normal delta.
        meter.sample(None, t2 + Duration::from_secs(1));
        assert_eq!(meter.samples.len(), 3);
        let t3 = t2 + Duration::from_secs(2);
        meter.sample(Some(NetCounters { down: 1_000_000_000_001_999.0, up: 999_999_999_999.0, interfaces: interfaces2 }), t3);
        // down delta = 1_000_000_000_001_999 - 999_999_999_999 = 999_000_000_002_000 over 2s.
        assert_eq!(meter.samples.back(), Some(&(499_500_000_001_000.0, 0.0)));

        assert_eq!(net_ceiling(&meter.samples), 499_500_000_001_000.0);

        // With no previous sample at all, a fresh meter also publishes (0, 0).
        let mut fresh = NetMeter::new();
        fresh.sample(Some(NetCounters { down: 5.0, up: 5.0, interfaces: BTreeSet::new() }), Instant::now());
        assert_eq!(fresh.samples.back(), Some(&(0.0, 0.0)));
    }

    #[test]
    fn battery_texts_follow_iterm2() {
        let state = PowerState { percentage: Some(57), charging: false, time: -1.0 };
        let view = battery_view(Some(&state), false, true, true, false);
        assert_eq!(view.left, "57%");
        assert_eq!(view.right, "");
        assert_eq!(view.left_reserve, "100%");
        assert_eq!(view.right_reserve, "");
        assert!(view.right_icon_reserved);
        assert!(!view.charging_icon);

        let state_time = PowerState { percentage: Some(57), charging: false, time: 3725.0 };
        let view = battery_view(Some(&state_time), false, true, true, true);
        assert_eq!(view.right, "1:02");
        assert_eq!(view.right_reserve, "55:55");
        assert!(!view.right_icon_reserved);

        let full_charging = PowerState { percentage: Some(100), charging: true, time: 3725.0 };
        let view = battery_view(Some(&full_charging), true, true, true, true);
        assert_eq!(view.left, "100%");
        assert_eq!(view.right, "");
        assert!(view.charging_icon);

        let mismatched = PowerState { percentage: Some(57), charging: false, time: 3725.0 };
        let view = battery_view(Some(&mismatched), true, true, false, true);
        assert_eq!(view.left, "");
        assert_eq!(view.right, "");

        let time_only = PowerState { percentage: Some(57), charging: false, time: 3725.0 };
        let view = battery_view(Some(&time_only), false, true, false, true);
        assert_eq!(view.left, "1:02");
        assert_eq!(view.left_reserve, "");

        let view = battery_view(None, false, true, true, true);
        assert_eq!(view.left, "");
        assert_eq!(view.right, "");

        assert_eq!(duration_string(59.0), "0:00");
    }

    struct FakeSamplers {
        cpu_calls: usize,
        memory_calls: usize,
        network_calls: usize,
        power_calls: usize,
        ac_calls: usize,
        ac_value: bool,
    }

    impl Samplers for FakeSamplers {
        fn cpu(&mut self) -> Option<CpuTicks> {
            self.cpu_calls += 1;
            Some(CpuTicks { idle: 1.0, total: 2.0 })
        }
        fn memory(&mut self) -> Option<(u64, u64)> {
            self.memory_calls += 1;
            Some((1, 2))
        }
        fn network(&mut self, _exclude_tunnels: bool) -> Option<NetCounters> {
            self.network_calls += 1;
            Some(NetCounters::default())
        }
        fn power(&mut self) -> Option<PowerState> {
            self.power_calls += 1;
            Some(PowerState { percentage: Some(50), charging: false, time: 100.0 })
        }
        fn on_ac(&mut self) -> bool {
            self.ac_calls += 1;
            self.ac_value
        }
    }

    /// Each graph animates from its own meter's last sample (`SPK:158-251`): CPU's first
    /// sample at 1 s leaves Battery's start at 0 s; an AC change alone moves none, though it
    /// still asks for a redraw (`BAT:245-247`).
    #[test]
    fn each_meter_keeps_its_own_sample_time() {
        let mut meters = Meters::new();
        let mut fake = FakeSamplers { cpu_calls: 0, memory_calls: 0, network_calls: 0, power_calls: 0, ac_calls: 0, ac_value: false };
        let t0 = Instant::now();
        let wanted = Wanted { cpu: true, memory: false, network: false, battery: true };
        meters.tick(t0, wanted, false, &mut fake);
        assert_eq!(meters.sampled, Sampled { cpu: None, memory: None, network: None, battery: Some(t0) });

        let t1 = t0 + Duration::from_secs(1);
        meters.tick(t1, wanted, false, &mut fake);
        assert_eq!(meters.sampled.cpu, Some(t1));
        assert_eq!(meters.sampled.battery, Some(t0), "sampling CPU leaves Battery's start alone");

        let before = meters.sampled;
        fake.ac_value = true;
        let t2 = t1 + Duration::from_millis(1500);
        assert!(meters.tick(t2, Wanted { cpu: false, ..wanted }, false, &mut fake), "an AC change redraws");
        assert_eq!(meters.sampled, before, "an AC change moves no animation start");
    }

    #[test]
    fn meters_sample_on_iterm2s_schedule() {
        let mut meters = Meters::new();
        let mut fake = FakeSamplers { cpu_calls: 0, memory_calls: 0, network_calls: 0, power_calls: 0, ac_calls: 0, ac_value: false };
        let t0 = Instant::now();
        let wanted = Wanted { cpu: true, memory: true, network: true, battery: true };

        // Memory, network and battery sample at once; CPU does not.
        let changed = meters.tick(t0, wanted, false, &mut fake);
        assert!(changed);
        assert_eq!(fake.memory_calls, 1);
        assert_eq!(fake.network_calls, 1);
        assert_eq!(fake.power_calls, 1);
        assert_eq!(fake.cpu_calls, 0);
        assert_eq!(fake.ac_calls, 1);

        // Under 1 s later: nothing new.
        let t_half = t0 + Duration::from_millis(500);
        let changed = meters.tick(t_half, wanted, false, &mut fake);
        assert!(!changed);
        assert_eq!(fake.cpu_calls, 0);
        assert_eq!(fake.memory_calls, 1);

        // At 1 s: CPU's first sample, plus memory/network/AC's next.
        let t1 = t0 + Duration::from_secs(1);
        let changed = meters.tick(t1, wanted, false, &mut fake);
        assert!(changed);
        assert_eq!(fake.cpu_calls, 1);
        assert_eq!(fake.memory_calls, 2);
        assert_eq!(fake.network_calls, 2);
        assert_eq!(fake.ac_calls, 2);
        // Battery does not sample again until 60 s.
        assert_eq!(fake.power_calls, 1);

        // At 60 s: battery samples again.
        let t60 = t0 + Duration::from_secs(60);
        meters.tick(t60, wanted, false, &mut fake);
        assert_eq!(fake.power_calls, 2);

        // An AC change alone reports changed with no new battery sample.
        fake.ac_value = true;
        let t61 = t60 + Duration::from_secs(1);
        let power_calls_before = fake.power_calls;
        let changed = meters.tick(t61, wanted, false, &mut fake);
        assert!(changed);
        assert_eq!(fake.power_calls, power_calls_before);
        assert!(meters.on_ac);

        // Unwanted meters stop sampling and keep their history.
        let memory_history_len = meters.memory.samples.values().len();
        let memory_calls_before = fake.memory_calls;
        let none_wanted = Wanted::default();
        let t62 = t61 + Duration::from_secs(1);
        meters.tick(t62, none_wanted, false, &mut fake);
        let t63 = t62 + Duration::from_secs(1);
        meters.tick(t63, none_wanted, false, &mut fake);
        assert_eq!(fake.memory_calls, memory_calls_before);
        assert_eq!(meters.memory.samples.values().len(), memory_history_len);
        assert_eq!(meters.next_due(), None);

        // Wanting again picks the schedule back up (memory samples at once again).
        let t64 = t63 + Duration::from_secs(1);
        meters.tick(t64, wanted, false, &mut fake);
        assert_eq!(fake.memory_calls, memory_calls_before + 1);

        // `next_due` is the earliest due time among the active schedules.
        let due = meters.next_due().unwrap();
        assert!(due > t64);
    }
}

//! Prevent Sleep (13p): which sessions ask for it, the battery gate, the status text and the one
//! request the app holds while any session asks (`SPC:106-188`, `PSV:465-482`).

use std::time::{Duration, Instant};

/// How often the power source is looked at while a session asks (winit has no power broadcast).
pub const RECHECK: Duration = Duration::from_secs(5);

/// `SPC:106-127`: sessions (live or buried) whose profile has Prevent Sleep and that have not
/// exited. Each is (profile has Prevent Sleep, exited).
pub fn requesting(panes: &[(bool, bool)]) -> usize {
    panes.iter().filter(|(wants, exited)| *wants && !exited).count()
}

/// `SPC:133-148`: the count gated by the power source.
pub fn preventing(requesters: usize, on_power: bool, on_battery_too: bool) -> usize {
    if on_power || on_battery_too { requesters } else { 0 }
}

/// The Session tab's line (`PSV:465-482`, `ProfilesSession.*PreventingSleep`).
pub fn status_text(holding: usize, requesting: usize) -> String {
    match (holding, requesting) {
        (0, 0) => "No sessions are currently preventing sleep.".to_string(),
        (0, n) => format!("{n} {} prevent sleep, but it is disabled while on battery.", if n == 1 { "session would" } else { "sessions would" }),
        (n, _) => format!("{n} {} currently preventing sleep.", if n == 1 { "session is" } else { "sessions are" }),
    }
}

/// What the last look saw; a change means the request must be set again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fingerprint {
    pub requesting: usize,
    pub on_power: bool,
    pub battery_too: bool,
}

/// The answer of a look that found something new.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decision {
    pub holding: usize,
    pub requesting: usize,
}

/// Compares each look with the last one, so the request is set only when something changed, and
/// samples the power source at most once per `RECHECK`.
#[derive(Debug, Default)]
pub struct Tracker {
    last: Option<Fingerprint>,
    checked: Option<Instant>,
    on_power: bool,
}

impl Tracker {
    /// One look. `sample` reads the power line (true on power); it is called only while a session
    /// asks and the last reading is `RECHECK` old. Some when the fingerprint changed.
    pub fn look(&mut self, now: Instant, requesting: usize, battery_too: bool, sample: impl FnOnce() -> bool) -> Option<Decision> {
        if requesting == 0 {
            self.checked = None;
        } else if self.checked.is_none_or(|t| now >= t + RECHECK) {
            self.checked = Some(now);
            self.on_power = sample();
        }
        // With nobody asking the power line does not matter: keep one fingerprint for that.
        let on_power = requesting == 0 || self.on_power;
        let fp = Fingerprint { requesting, on_power, battery_too };
        if self.last == Some(fp) {
            return None;
        }
        self.last = Some(fp);
        Some(Decision { holding: preventing(requesting, on_power, battery_too), requesting })
    }

    /// When the power source is due to be looked at again; None while no session asks.
    pub fn due(&self) -> Option<Instant> {
        self.checked.map(|t| t + RECHECK)
    }
}

/// The one reference-counted request: held exactly while the count is above zero. Windows
/// keeps it per thread, so it is only ever called from the UI thread; Drop releases it.
#[derive(Debug, Default)]
pub struct Guard {
    held: bool,
}

impl Guard {
    /// Takes or drops the request; true when that changed it.
    pub fn set(&mut self, want: bool) -> bool {
        if want == self.held {
            return false;
        }
        apply(want);
        self.held = want;
        true
    }

    pub fn held(&self) -> bool {
        self.held
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.set(false);
    }
}

/// `SetThreadExecutionState`: system required (idle sleep off, the display may still sleep, which
/// is iTerm2's `idleSystemSleepDisabled`), or back to nothing.
#[cfg(windows)]
fn apply(want: bool) {
    use windows_sys::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED};
    unsafe { SetThreadExecutionState(if want { ES_CONTINUOUS | ES_SYSTEM_REQUIRED } else { ES_CONTINUOUS }) };
}

#[cfg(not(windows))]
fn apply(_want: bool) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review Focus 8: only live sessions with the profile key count; the battery gate.
    #[test]
    fn sleep_count_follows_iterm2() {
        assert_eq!(requesting(&[(true, false), (true, true), (false, false)]), 1);
        assert_eq!(preventing(2, true, false), 2);
        assert_eq!(preventing(2, false, false), 0);
        assert_eq!(preventing(2, false, true), 2);
        assert_eq!(preventing(0, true, true), 0);
    }

    /// The three texts with iTerm2's one and other forms.
    #[test]
    fn sleep_status_follows_iterm2() {
        assert_eq!(status_text(0, 0), "No sessions are currently preventing sleep.");
        assert_eq!(status_text(1, 1), "1 session is currently preventing sleep.");
        assert_eq!(status_text(3, 3), "3 sessions are currently preventing sleep.");
        assert_eq!(status_text(0, 1), "1 session would prevent sleep, but it is disabled while on battery.");
        assert_eq!(status_text(0, 2), "2 sessions would prevent sleep, but it is disabled while on battery.");
    }

    /// P16: the tracker reports a change once, samples the power line only while a session asks
    /// and at most every `RECHECK`, and sees a plug or unplug.
    #[test]
    fn sleep_tracker_follows_the_fingerprint() {
        let t0 = Instant::now();
        let mut t = Tracker::default();
        let mut samples = 0;
        let look = |t: &mut Tracker, at: Instant, n: usize, bat: bool, on: bool, samples: &mut i32| {
            t.look(at, n, bat, || {
                *samples += 1;
                on
            })
        };
        // Nothing asks: a first look reports, no sample is taken, no wake is due.
        assert_eq!(look(&mut t, t0, 0, false, true, &mut samples), Some(Decision { holding: 0, requesting: 0 }));
        assert_eq!((samples, t.due()), (0, None));
        assert_eq!(look(&mut t, t0, 0, false, true, &mut samples), None);
        // A session asks on power: held, sampled, a wake is due RECHECK ahead.
        assert_eq!(look(&mut t, t0, 1, false, true, &mut samples), Some(Decision { holding: 1, requesting: 1 }));
        assert_eq!((samples, t.due()), (1, Some(t0 + RECHECK)));
        // Unplugged inside the RECHECK: not seen yet and no sample.
        assert_eq!(look(&mut t, t0 + Duration::from_secs(2), 1, false, false, &mut samples), None);
        assert_eq!(samples, 1);
        // Due: the unplug is seen, the request drops, the line says battery.
        assert_eq!(look(&mut t, t0 + RECHECK, 1, false, false, &mut samples), Some(Decision { holding: 0, requesting: 1 }));
        assert_eq!(samples, 2);
        // The battery key turns it back on without a new sample.
        assert_eq!(look(&mut t, t0 + RECHECK, 1, true, false, &mut samples), Some(Decision { holding: 1, requesting: 1 }));
        assert_eq!(samples, 2);
        // The last session ends: released, and the wake stops.
        assert_eq!(look(&mut t, t0 + RECHECK, 0, true, false, &mut samples), Some(Decision { holding: 0, requesting: 0 }));
        assert_eq!(t.due(), None);
    }

    /// Three overlapping sessions are one request: it is held once and dropped once.
    #[test]
    fn overlapping_sessions_share_one_request() {
        let mut g = Guard::default();
        let wants = [true, true, true, false];
        let changes: Vec<bool> = wants.iter().map(|w| g.set(*w)).collect();
        assert_eq!(changes, [true, false, false, true]);
        assert!(!g.held());
    }

    #[cfg(windows)]
    mod win {
        use super::*;
        use windows_sys::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED};

        /// The state Windows holds for this thread, read by setting ES_CONTINUOUS alone (which
        /// clears it) and restoring it.
        fn state() -> u32 {
            unsafe { SetThreadExecutionState(ES_CONTINUOUS) }
        }

        #[test]
        fn execution_state_follows_the_guard() {
            let mut g = Guard::default();
            assert!(g.set(true) && g.held());
            assert!(!g.set(true));
            assert_ne!(state() & ES_SYSTEM_REQUIRED, 0, "held: the probe sees the request");
            // The probe cleared it; take it again through the guard's own path.
            g.held = false;
            assert!(g.set(true));
            assert!(g.set(false) && !g.held());
            assert_eq!(state() & ES_SYSTEM_REQUIRED, 0, "released: no system required");
            let mut g = Guard::default();
            g.set(true);
            drop(g);
            assert_eq!(state() & ES_SYSTEM_REQUIRED, 0, "Drop releases");
        }
    }
}

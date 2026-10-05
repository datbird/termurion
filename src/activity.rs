//! Output and idle per session, with iTerm2's timings: idle after the idle time (2 s) plus one
//! quiet second, one
//! new-output and one idle event per burst, and output right after a resize ignored.

use std::time::{Duration, Instant};

/// Output this soon after a resize is the program redrawing, not activity.
const RESIZE_JIGGLE: Duration = Duration::from_millis(250);
/// No idle or new-output events this soon after a resize.
const AFTER_RESIZE: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityEvent {
    /// A burst of output went quiet.
    Idle,
    /// A burst of output began.
    NewOutput,
    /// Output after an idle spell (the Activity After Idle event trigger).
    ActivityAfterIdle,
}

/// One session's output and idle state.
#[derive(Clone, Debug)]
pub struct Activity {
    /// `idleTimeSeconds`, read when the session started.
    idle: Duration,
    last_output: Option<Instant>,
    last_resize: Option<Instant>,
    /// Output since the last Idle.
    burst: bool,
    new_output_posted: bool,
    idle_posted: bool,
    pending: Vec<ActivityEvent>,
    /// Output the user has not looked at (the tab's indicators).
    unseen: bool,
}

/// `idleTimeSeconds` default (`AS:488`).
pub const DEFAULT_IDLE: Duration = Duration::from_secs(2);

impl Default for Activity {
    fn default() -> Activity {
        Activity::new(DEFAULT_IDLE)
    }
}

impl Activity {
    /// A tracker for a session whose `idleTimeSeconds` is `idle`.
    pub fn new(idle: Duration) -> Activity {
        Activity { idle, last_output: None, last_resize: None, burst: false, new_output_posted: false, idle_posted: false, pending: Vec::new(), unseen: false }
    }

    /// The session's idle time, read when it starts (a restarted session keeps its tracker).
    pub fn set_idle(&mut self, idle: Duration) {
        self.idle = idle;
    }

    pub fn output(&mut self, now: Instant) {
        if self.last_resize.is_some_and(|r| now.saturating_duration_since(r) <= RESIZE_JIGGLE) {
            return;
        }
        if self.last_output.is_some_and(|l| now.saturating_duration_since(l) > crate::cadence::idle_after(self.idle)) {
            self.pending.push(ActivityEvent::ActivityAfterIdle);
        }
        self.last_output = Some(now);
        self.burst = true;
        self.idle_posted = false;
        self.unseen = true;
    }

    pub fn resized(&mut self, now: Instant) {
        self.last_resize = Some(now);
    }

    /// Output less than the idle time ago (`PS:6580`).
    pub fn is_processing(&self, now: Instant) -> bool {
        self.last_output.is_some_and(|l| now.saturating_duration_since(l) < self.idle)
    }

    /// Output once, none for more than the idle time plus one second (`PS:6586`).
    pub fn is_idle(&self, now: Instant) -> bool {
        self.last_output.is_some_and(|l| now.saturating_duration_since(l) > crate::cadence::idle_after(self.idle))
    }

    pub fn last_output(&self) -> Option<Instant> {
        self.last_output
    }

    /// A burst of output has not gone idle yet (the window checks back).
    pub fn in_burst(&self) -> bool {
        self.burst
    }

    pub fn unseen(&self) -> bool {
        self.unseen
    }

    /// The user looked at the session.
    pub fn seen(&mut self) {
        self.unseen = false;
    }

    /// Events since the last tick: one New Output and one Idle per burst, none within 5 s
    /// of a resize.
    pub fn tick(&mut self, now: Instant) -> Vec<ActivityEvent> {
        let mut out = std::mem::take(&mut self.pending);
        let calm = self.last_resize.is_none_or(|r| now.saturating_duration_since(r) > AFTER_RESIZE);
        if self.burst && !self.new_output_posted && calm {
            out.push(ActivityEvent::NewOutput);
            self.new_output_posted = true;
        }
        if self.burst && self.is_idle(now) && !self.idle_posted && calm {
            out.push(ActivityEvent::Idle);
            self.idle_posted = true;
            self.burst = false;
            self.new_output_posted = false;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn ms(t0: Instant, n: u64) -> Instant {
        t0 + Duration::from_millis(n)
    }

    #[test]
    fn idle_after_three_quiet_seconds() {
        let t0 = Instant::now() + Duration::from_secs(100);
        let mut a = Activity::default();
        a.output(t0);
        assert_eq!(a.tick(ms(t0, 100)), vec![ActivityEvent::NewOutput]);
        assert!(a.is_processing(ms(t0, 1900)));
        assert!(!a.is_idle(ms(t0, 2900)));
        assert_eq!(a.tick(ms(t0, 2900)), vec![]);
        assert_eq!(a.tick(ms(t0, 3100)), vec![ActivityEvent::Idle]);
        assert_eq!(a.tick(ms(t0, 4000)), vec![], "once");
        a.output(ms(t0, 10_000));
        assert_eq!(a.tick(ms(t0, 10_100)), vec![ActivityEvent::ActivityAfterIdle, ActivityEvent::NewOutput]);
    }

    #[test]
    fn new_output_once_per_burst() {
        let t0 = Instant::now() + Duration::from_secs(100);
        let mut a = Activity::default();
        let mut events = Vec::new();
        for n in [0, 500, 1000, 1500] {
            a.output(ms(t0, n));
            events.extend(a.tick(ms(t0, n + 10)));
        }
        assert_eq!(events, vec![ActivityEvent::NewOutput]);
        assert!(a.unseen(), "the tab shows new output until the user looks");
        a.seen();
        assert!(!a.unseen());
    }

    #[test]
    fn resize_output_is_not_activity() {
        let t0 = Instant::now() + Duration::from_secs(100);
        let mut a = Activity::default();
        a.resized(t0);
        a.output(ms(t0, 100));
        assert!((1..100).all(|i| a.tick(ms(t0, 100 * i)).is_empty()), "output 0.1 s after a resize is the program redrawing");
        let mut b = Activity::default();
        b.resized(t0);
        b.output(ms(t0, 1000));
        assert!(b.tick(ms(t0, 1100)).is_empty() && b.tick(ms(t0, 4500)).is_empty(), "nothing within 5 s of a resize");
        assert!(!b.tick(ms(t0, 5500)).is_empty(), "after that, the burst counts");
    }

    /// `idleTimeSeconds` is read per session (`PS:849`): processing is output less than the
    /// idle time ago (`PS:6580`), idle is no output for more than the idle time plus one
    /// second (`PS:6586`); the default tracker keeps 2 s and 3 s.
    #[test]
    fn idle_time_follows_iterm2() {
        let t0 = Instant::now() + Duration::from_secs(100);
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut a = Activity::new(Duration::from_secs(5));
        a.output(t0);
        assert!(a.is_processing(at(4900)));
        assert!(!a.is_processing(at(5000)));
        assert!(!a.is_idle(at(6000)));
        assert!(a.is_idle(at(6001)));
        assert_eq!(a.tick(at(100)), vec![ActivityEvent::NewOutput]);
        assert_eq!(a.tick(at(3100)), vec![], "3 s is not idle for a 5 s tracker");
        assert_eq!(a.tick(at(6000)), vec![]);
        assert_eq!(a.tick(at(6001)), vec![ActivityEvent::Idle]);
        // Output after the idle spell is Activity After Idle only past the idle time plus one.
        let mut b = Activity::new(Duration::from_secs(5));
        b.output(t0);
        b.output(at(4000));
        assert!(b.tick(at(4100)).iter().all(|e| *e != ActivityEvent::ActivityAfterIdle));
        b.output(at(11_000));
        assert!(b.tick(at(11_100)).contains(&ActivityEvent::ActivityAfterIdle));
        // A tracker made with the default, then told the pane's idle time.
        let mut c = Activity::default();
        c.set_idle(Duration::from_secs(5));
        c.output(t0);
        assert!(c.is_processing(at(4900)) && !c.is_idle(at(6000)));
        // The default tracker.
        let mut d = Activity::default();
        d.output(t0);
        assert!(d.is_processing(at(1900)) && !d.is_processing(at(2100)));
        assert!(!d.is_idle(at(3000)) && d.is_idle(at(3001)));
        assert_eq!(DEFAULT_IDLE, Duration::from_secs(2));
    }
}

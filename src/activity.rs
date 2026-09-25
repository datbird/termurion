//! Output and idle per session, with iTerm2's timings: idle after 3 quiet seconds, one
//! new-output and one idle event per burst, and output right after a resize ignored.

use std::time::{Duration, Instant};

/// iTerm2's idleTimeSeconds (2) plus one: quiet this long is idle.
const IDLE_AFTER: Duration = Duration::from_secs(3);
const PROCESSING: Duration = Duration::from_secs(2);
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
#[derive(Clone, Debug, Default)]
pub struct Activity {
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

impl Activity {
    pub fn output(&mut self, now: Instant) {
        if self.last_resize.is_some_and(|r| now.saturating_duration_since(r) <= RESIZE_JIGGLE) {
            return;
        }
        if self.last_output.is_some_and(|l| now.saturating_duration_since(l) > IDLE_AFTER) {
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

    /// Output within the last 2 s.
    pub fn is_processing(&self, now: Instant) -> bool {
        self.last_output.is_some_and(|l| now.saturating_duration_since(l) <= PROCESSING)
    }

    /// Output once, none for more than 3 s.
    pub fn is_idle(&self, now: Instant) -> bool {
        self.last_output.is_some_and(|l| now.saturating_duration_since(l) > IDLE_AFTER)
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
}

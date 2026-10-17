//! Switching tabs with a horizontal swipe, and the note about it that iTerm2 shows while mouse
//! reporting takes the swipe (`PTYSession showHorizontalScrollInfo`, `PTYSession.m:21689-21726`).
//!
//! iTerm2's swipe is macOS's own "swipe between pages". Windows has no such gesture, so
//! Termurion gives a horizontal wheel or touchpad pan over a pane (one the program is not
//! taking) the same job: one swipe moves one tab, fingers moving left to the next tab. The
//! note's text is then true here too.

/// The note's bar identifier (`PTYSession.m:21698`).
pub const BAR_ID: &str = "HorizontalScrollWarning";
/// Dismissing the note in any way, or its button, sets this for good (`PTYSession.m:21695`).
pub const NEVER_KEY: &str = "NoSyncScrollingHorizontally";
pub const TEXT: &str = "When mouse reporting is on, horizontal scrolling does not switch tabs.\nHold Alt while swiping or disable horizontal scroll reporting to switch tabs.";
pub const SETTINGS: &str = "Settings\u{2026}";
/// The setting the button reveals (`kPreferenceKeyReportHorizontalScrollEvents`).
pub const SETTING_KEY: &str = "ReportHorizontalScrollEvents";

/// A touchpad pan has to travel this many lines to count as a swipe.
pub const PAN_LINES: f64 = 5.0;
/// A pause this long (seconds) ends a swipe, so the next one counts again. A touchpad's
/// momentum after the fingers lift stays inside one swipe.
pub const PAUSE: f64 = 0.35;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Previous,
    Next,
}

#[derive(Clone, Debug, Default)]
pub struct Swipe {
    travelled: f64,
    last: Option<f64>,
    switched: bool,
}

impl Swipe {
    /// A horizontal wheel event at `now` seconds (`dx` positive moves the content right, the
    /// fingers right). A wheel notch switches at once; a pan after `PAN_LINES`. One swipe
    /// switches once.
    pub fn feed(&mut self, now: f64, dx: f64, precise: bool) -> Option<Step> {
        if dx == 0.0 {
            return None;
        }
        if self.last.is_none_or(|t| now - t > PAUSE) {
            self.travelled = 0.0;
            self.switched = false;
        }
        self.last = Some(now);
        if self.switched {
            return None;
        }
        self.travelled += dx;
        let needed = if precise { PAN_LINES } else { f64::MIN_POSITIVE };
        if self.travelled.abs() < needed {
            return None;
        }
        self.switched = true;
        Some(if self.travelled < 0.0 { Step::Next } else { Step::Previous })
    }
}

/// The tab a step leads to from `active` among `count` tabs; none past either end.
pub fn target(active: usize, count: usize, step: Step) -> Option<usize> {
    match step {
        Step::Next => (active + 1 < count).then_some(active + 1),
        Step::Previous => active.checked_sub(1),
    }
}

/// Whether the note is due: a horizontal report went to the program, the window has more than
/// one tab, and the person has not dismissed it (`PTYSession.m:21690-21697`).
pub fn note_due(reported: bool, tabs: usize, dismissed: bool, showing: bool) -> bool {
    reported && tabs >= 2 && !dismissed && !showing
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wheel_notch_switches_once_per_swipe() {
        let mut s = Swipe::default();
        assert_eq!(s.feed(0.0, -3.0, false), Some(Step::Next));
        // More notches inside the swipe do nothing; after a pause the next swipe counts.
        assert_eq!(s.feed(0.1, -3.0, false), None);
        assert_eq!(s.feed(0.2, -3.0, false), None);
        assert_eq!(s.feed(1.0, 3.0, false), Some(Step::Previous));
    }

    #[test]
    fn a_pan_has_to_travel_and_its_momentum_does_not_switch_again() {
        let mut s = Swipe::default();
        for i in 0..4 {
            assert_eq!(s.feed(i as f64 * 0.02, 1.0, true), None, "{i}");
        }
        assert_eq!(s.feed(0.1, 1.5, true), Some(Step::Previous));
        // Momentum keeps arriving within the pause: no second switch.
        for i in 0..20 {
            assert_eq!(s.feed(0.2 + i as f64 * 0.1, 2.0, true), None);
        }
        // A pause ends it.
        assert_eq!(s.feed(10.0, -6.0, true), Some(Step::Next));
    }

    #[test]
    fn a_tiny_pan_that_pauses_never_adds_up() {
        let mut s = Swipe::default();
        for i in 0..10 {
            assert_eq!(s.feed(i as f64, 1.0, true), None);
        }
    }

    #[test]
    fn targets_stop_at_the_ends() {
        assert_eq!(target(0, 3, Step::Previous), None);
        assert_eq!(target(0, 3, Step::Next), Some(1));
        assert_eq!(target(2, 3, Step::Next), None);
        assert_eq!(target(2, 3, Step::Previous), Some(1));
    }

    #[test]
    fn the_note_needs_a_report_two_tabs_and_no_dismissal() {
        assert!(note_due(true, 2, false, false));
        assert!(!note_due(false, 2, false, false));
        assert!(!note_due(true, 1, false, false));
        assert!(!note_due(true, 2, true, false));
        assert!(!note_due(true, 2, false, true));
    }

    #[test]
    fn the_text_is_iterm2s_with_alt_for_option() {
        assert_eq!(TEXT, "When mouse reporting is on, horizontal scrolling does not switch tabs.\nHold Alt while swiping or disable horizontal scroll reporting to switch tabs.");
        assert_eq!(NEVER_KEY, "NoSyncScrollingHorizontally");
        assert_eq!(BAR_ID, "HorizontalScrollWarning");
    }
}

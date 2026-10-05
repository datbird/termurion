//! Cursor time: the blink phase and its move rule, and each pane's `Motion`. Pure.

use std::time::{Duration, Instant};

use crate::layout::Rect;

/// How long a moved cursor stays solid: iTerm2 blinks only once the cursor has been still for
/// more than this (`iTermTextDrawingHelper.m:4195`).
pub const MOVE_HOLD: Duration = Duration::from_millis(500);

/// The half period of the blink for "TimeBetweenBlinks" seconds (`PTYTextView.m:3649`): a value
/// that is not a positive finite number gives iTerm2's default, half a second.
pub fn half_period(seconds: f64) -> Duration {
    if seconds.is_finite() && seconds > 0.0 { Duration::try_from_secs_f64(seconds).unwrap_or(MOVE_HOLD) } else { MOVE_HOLD }
}

/// Whether the cursor blinks now (`iTermTextDrawingHelper.m:4190-4197`): it is set to blink, it
/// is the key window's focused pane's, and it has not moved for more than `MOVE_HOLD`.
pub fn blink_condition(blinking: bool, key_window_focused_pane: bool, since_moved: Duration) -> bool {
    blinking && key_window_focused_pane && since_moved > MOVE_HOLD
}

/// The blink phase, `blinkingItemsVisible`: one clock for every blinking thing, flipped each
/// half period whether or not anything uses it (`PTYTextView.m:3645-3656`).
#[derive(Clone, Debug)]
pub struct Blink {
    pub visible: bool,
    pub flipped: Instant,
}

impl Blink {
    pub fn new(now: Instant) -> Blink {
        Blink { visible: true, flipped: now }
    }

    /// Flips when a half period has passed. Returns (changed, the next flip).
    pub fn tick(&mut self, now: Instant, half: Duration) -> (bool, Instant) {
        let changed = now.saturating_duration_since(self.flipped) >= half;
        if changed {
            self.visible = !self.visible;
            self.flipped += half;
            // Asleep for several periods: do not replay each missed flip.
            if now.saturating_duration_since(self.flipped) >= half {
                self.flipped = now;
            }
        }
        (changed, self.flipped + half)
    }
}

/// A slide of the cursor between two cells (Task 7).
#[derive(Clone, Debug, Default)]
pub struct Slide {}

/// A smear of the cursor between two positions (Task 8).
#[derive(Clone, Debug, Default)]
pub struct Smear {}

/// What one pane remembers about its cursor from frame to frame.
#[derive(Clone, Debug, Default)]
pub struct Motion {
    /// The screen-relative grid cell (row, column) the cursor was last seen at.
    pub cell: Option<(i64, usize)>,
    pub moved: Option<Instant>,
    pub last_rect: Option<Rect>,
    pub slide: Option<Slide>,
    pub smear: Option<Smear>,
}

impl Motion {
    /// Notes the cursor's screen cell for a frame. True when it moved. A hidden cursor (None)
    /// leaves the stored cell alone, so hiding and showing it in place is no move.
    pub fn note(&mut self, cell: Option<(i64, usize)>, now: Instant) -> bool {
        let Some(c) = cell else { return false };
        if self.cell == Some(c) {
            return false;
        }
        self.cell = Some(c);
        self.moved = Some(now);
        true
    }

    /// How long the cursor has been where it is (`Duration::MAX` when it never moved).
    pub fn since_moved(&self, now: Instant) -> Duration {
        self.moved.map_or(Duration::MAX, |t| now.saturating_duration_since(t))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn blink_period_follows_iterm2() {
        assert_eq!(half_period(0.5), ms(500));
        assert_eq!(half_period(0.25), ms(250));
        assert_eq!(half_period(0.0), ms(500));
        assert_eq!(half_period(-1.0), ms(500));
        assert_eq!(half_period(f64::NAN), ms(500));
        assert_eq!(half_period(f64::INFINITY), ms(500));
        let t0 = Instant::now();
        let mut b = Blink::new(t0);
        assert!(b.visible);
        assert_eq!(b.tick(t0 + ms(499), ms(500)), (false, t0 + ms(500)));
        assert!(b.visible);
        assert_eq!(b.tick(t0 + ms(500), ms(500)), (true, t0 + ms(1000)));
        assert!(!b.visible);
        assert_eq!(b.tick(t0 + ms(1000), ms(500)), (true, t0 + ms(1500)));
        assert!(b.visible);
    }

    #[test]
    fn blink_condition_follows_iterm2() {
        assert!(blink_condition(true, true, ms(501)));
        assert!(!blink_condition(true, true, ms(500)));
        assert!(!blink_condition(false, true, Duration::from_secs(10)));
        assert!(!blink_condition(true, false, Duration::from_secs(10)));
        assert!(blink_condition(true, true, Duration::MAX));
        let t0 = Instant::now();
        let mut m = Motion::default();
        assert_eq!(m.since_moved(t0), Duration::MAX);
        assert!(m.note(Some((5, 3)), t0));
        assert!(!m.note(Some((5, 3)), t0 + ms(100)));
        assert_eq!(m.since_moved(t0 + ms(200)), ms(200));
        // A hidden cursor is not a move, and showing it again at the same cell is not one.
        assert!(!m.note(None, t0 + ms(300)));
        assert!(!m.note(Some((5, 3)), t0 + ms(400)));
        assert_eq!(m.since_moved(t0 + ms(400)), ms(400));
        // The screen cell is compared: the same cell after output scrolled a line is no move.
        assert!(m.note(Some((5, 4)), t0 + ms(500)));
        assert!(!m.note(Some((5, 4)), t0 + ms(900)));
        assert_eq!(m.since_moved(t0 + ms(900)), ms(400));
    }
}

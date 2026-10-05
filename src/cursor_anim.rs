//! Cursor time: the blink phase and its move rule, and each pane's `Motion`. Pure.

use std::time::{Duration, Instant};

use crate::layout::Rect;
use crate::profile::Profile;

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

/// An easing curve of the smooth blink (`iTermCursorBlinkFadeAnimator.m:106-126`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Curve {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
}

impl Curve {
    /// The stored value 0 to 3; anything else is the default, ease in-out.
    pub fn from_i64(v: i64) -> Curve {
        match v {
            0 => Curve::Linear,
            1 => Curve::EaseIn,
            2 => Curve::EaseOut,
            _ => Curve::EaseInOut,
        }
    }

    /// The stored value.
    pub fn index(self) -> i64 {
        self as i64
    }
}

/// `t` (0 to 1) along `curve`.
pub fn eased(curve: Curve, t: f64) -> f64 {
    match curve {
        Curve::Linear => t,
        Curve::EaseIn => t * t,
        Curve::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
        Curve::EaseInOut => {
            if t < 0.5 {
                2.0 * t * t
            } else {
                1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
            }
        }
    }
}

/// The six smooth-blink timings, seconds and curves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FadeTimes {
    pub fade_in: f64,
    pub fade_out: f64,
    pub in_curve: Curve,
    pub out_curve: Curve,
    pub visible_dwell: f64,
    pub hidden_dwell: f64,
}

impl FadeTimes {
    pub fn from_profile(p: &Profile) -> FadeTimes {
        FadeTimes {
            fade_in: p.float("Cursor Blink Fade In Duration"),
            fade_out: p.float("Cursor Blink Fade Out Duration"),
            in_curve: Curve::from_i64(p.int("Cursor Blink Fade In Curve")),
            out_curve: Curve::from_i64(p.int("Cursor Blink Fade Out Curve")),
            visible_dwell: p.float("Cursor Blink Visible Dwell"),
            hidden_dwell: p.float("Cursor Blink Hidden Dwell"),
        }
    }
}

/// iTerm2's five presets in tag order (`iTermCursorBlinkFadePreset.m`).
pub const PRESETS: [(&str, FadeTimes); 5] = [
    ("Breathing", FadeTimes { fade_in: 0.9, fade_out: 0.9, in_curve: Curve::EaseInOut, out_curve: Curve::EaseInOut, visible_dwell: 0.5, hidden_dwell: 0.5 }),
    ("Linear", FadeTimes { fade_in: 0.4, fade_out: 0.4, in_curve: Curve::Linear, out_curve: Curve::Linear, visible_dwell: 0.7, hidden_dwell: 0.5 }),
    ("Fast", FadeTimes { fade_in: 0.2, fade_out: 0.2, in_curve: Curve::EaseInOut, out_curve: Curve::EaseInOut, visible_dwell: 0.7, hidden_dwell: 0.2 }),
    ("Slow", FadeTimes { fade_in: 0.5, fade_out: 0.75, in_curve: Curve::EaseInOut, out_curve: Curve::EaseInOut, visible_dwell: 1.0, hidden_dwell: 0.2 }),
    ("Subtle", FadeTimes { fade_in: 0.15, fade_out: 0.15, in_curve: Curve::EaseInOut, out_curve: Curve::EaseInOut, visible_dwell: 0.9, hidden_dwell: 0.2 }),
];

/// The preset whose values the timings match: durations within 0.001, curves exact.
pub fn preset_matching(t: &FadeTimes) -> Option<usize> {
    let near = |a: f64, b: f64| (a - b).abs() < 0.001;
    PRESETS.iter().position(|(_, p)| {
        near(p.fade_in, t.fade_in)
            && near(p.fade_out, t.fade_out)
            && near(p.visible_dwell, t.visible_dwell)
            && near(p.hidden_dwell, t.hidden_dwell)
            && p.in_curve == t.in_curve
            && p.out_curve == t.out_curve
    })
}

/// The smooth blink's clock (`iTermCursorBlinkFadeAnimator.m:44-104`): a cycle of [visible
/// dwell][fade out][hidden dwell][fade in], started at the visible dwell when blinking begins.
#[derive(Clone, Debug, Default)]
pub struct Fade {
    start: Option<Instant>,
}

impl Fade {
    /// The cursor's alpha at `now`, and how long until the next frame is needed: `Some(0)`
    /// while fading, the time to the next fade while dwelling, None when not blinking or the
    /// period is 0 (the cursor stays solid).
    pub fn alpha(&mut self, blinking: bool, now: Instant, t: &FadeTimes) -> (f32, Option<Duration>) {
        if !blinking {
            self.start = None;
            return (1.0, None);
        }
        // Whole nanoseconds, so the cycle's points are exact.
        let ns = |s: f64| if s.is_finite() && s > 0.0 { (s * 1e9).round() as u64 } else { 0 };
        let (vd, fo, hd, fi) = (ns(t.visible_dwell), ns(t.fade_out), ns(t.hidden_dwell), ns(t.fade_in));
        let period = vd + fo + hd + fi;
        if period == 0 {
            return (1.0, None);
        }
        let start = *self.start.get_or_insert(now);
        let e = (now.saturating_duration_since(start).as_nanos() % u128::from(period)) as u64;
        let until = |edge: u64| Some(Duration::from_nanos(edge - e));
        if e < vd {
            (1.0, until(vd))
        } else if e < vd + fo {
            let x = (e - vd) as f64 / fo as f64;
            ((1.0 - eased(t.out_curve, x)) as f32, Some(Duration::ZERO))
        } else if e < vd + fo + hd {
            (0.0, until(vd + fo + hd))
        } else {
            let x = (e - vd - fo - hd) as f64 / fi as f64;
            (eased(t.in_curve, x) as f32, Some(Duration::ZERO))
        }
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

    fn times() -> FadeTimes {
        FadeTimes { fade_in: 0.2, fade_out: 0.2, in_curve: Curve::EaseInOut, out_curve: Curve::EaseInOut, visible_dwell: 0.3, hidden_dwell: 0.3 }
    }

    #[test]
    fn fade_cycle_follows_iterm2() {
        let t = times();
        let t0 = Instant::now();
        let mut f = Fade::default();
        assert_eq!(f.alpha(true, t0, &t), (1.0, Some(ms(300))));
        assert_eq!(f.alpha(true, t0 + ms(400), &t), (0.5, Some(Duration::ZERO)));
        assert_eq!(f.alpha(true, t0 + ms(600), &t), (0.0, Some(ms(200))));
        assert_eq!(f.alpha(true, t0 + ms(850), &t), (0.125, Some(Duration::ZERO)));
        assert_eq!(f.alpha(true, t0 + ms(1000), &t), (1.0, Some(ms(300))));
        // Not blinking: solid, no frames, and the next blink starts a new cycle.
        let t1 = t0 + ms(1100);
        assert_eq!(f.alpha(false, t1, &t), (1.0, None));
        assert_eq!(f.alpha(true, t1 + ms(10), &t), (1.0, Some(ms(300))));
        assert_eq!(f.alpha(true, t1 + ms(10) + ms(400), &t), (0.5, Some(Duration::ZERO)));
        // A zero period stays solid; negative durations count as 0.
        let zero = FadeTimes { fade_in: 0.0, fade_out: 0.0, visible_dwell: 0.0, hidden_dwell: 0.0, ..t };
        let mut f = Fade::default();
        assert_eq!(f.alpha(true, t0, &zero), (1.0, None));
        let neg = FadeTimes { fade_in: -1.0, fade_out: -2.0, visible_dwell: -0.5, hidden_dwell: -3.0, ..t };
        assert_eq!(f.alpha(true, t0 + ms(50), &neg), (1.0, None));
    }

    #[test]
    fn curves_follow_iterm2() {
        assert_eq!(eased(Curve::Linear, 0.3), 0.3);
        assert_eq!(eased(Curve::EaseIn, 0.5), 0.25);
        assert_eq!(eased(Curve::EaseOut, 0.5), 0.75);
        assert_eq!(eased(Curve::EaseInOut, 0.25), 0.125);
        assert_eq!(eased(Curve::EaseInOut, 0.5), 0.5);
        assert_eq!(eased(Curve::EaseInOut, 0.75), 0.875);
        assert_eq!(Curve::from_i64(0), Curve::Linear);
        assert_eq!(Curve::from_i64(1), Curve::EaseIn);
        assert_eq!(Curve::from_i64(2), Curve::EaseOut);
        assert_eq!(Curve::from_i64(3), Curve::EaseInOut);
        assert_eq!(Curve::from_i64(9), Curve::EaseInOut);
        assert_eq!(Curve::from_i64(-1), Curve::EaseInOut);
    }

    #[test]
    fn presets_follow_iterm2() {
        use Curve::*;
        let want = [
            ("Breathing", 0.9, 0.9, EaseInOut, EaseInOut, 0.5, 0.5),
            ("Linear", 0.4, 0.4, Linear, Linear, 0.7, 0.5),
            ("Fast", 0.2, 0.2, EaseInOut, EaseInOut, 0.7, 0.2),
            ("Slow", 0.5, 0.75, EaseInOut, EaseInOut, 1.0, 0.2),
            ("Subtle", 0.15, 0.15, EaseInOut, EaseInOut, 0.9, 0.2),
        ];
        for (i, (name, fi, fo, ic, oc, vd, hd)) in want.into_iter().enumerate() {
            assert_eq!(PRESETS[i].0, name);
            assert_eq!(PRESETS[i].1, FadeTimes { fade_in: fi, fade_out: fo, in_curve: ic, out_curve: oc, visible_dwell: vd, hidden_dwell: hd });
        }
        assert_eq!(preset_matching(&times()), None);
        let fast = PRESETS[2].1;
        assert_eq!(preset_matching(&fast), Some(2));
        assert_eq!(preset_matching(&FadeTimes { fade_in: 0.2005, ..fast }), Some(2));
        assert_eq!(preset_matching(&FadeTimes { fade_in: 0.202, ..fast }), None);
        assert_eq!(preset_matching(&FadeTimes { in_curve: Curve::Linear, ..fast }), None);
    }
}

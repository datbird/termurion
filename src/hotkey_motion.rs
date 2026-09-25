//! How a hotkey window moves when it shows and hides: the edge styles slide in from outside
//! their edge (top, bottom, and right, as in iTerm2; left does not), over 0.25 s with CSS
//! ease-in, unless the hidden frame would lie on another monitor.

use std::time::Duration;

use crate::frames;
use crate::layout::Rect;

/// iTerm2's hotkeyTermAnimationDuration default.
pub const DURATION: Duration = Duration::from_millis(250);

/// Whether a window style slides (the others show and hide at once).
pub fn slides(style: i64) -> bool {
    matches!(style, frames::TOP_PERCENT | frames::TOP_CELLS | frames::BOTTOM_PERCENT | frames::BOTTOM_CELLS | frames::RIGHT_PERCENT | frames::RIGHT_CELLS)
}

/// Where a shown frame `shown` sits while hidden, for its style, on the monitor work area
/// `work` (Windows coordinates, y down).
pub fn hidden_origin(style: i64, shown: Rect, work: Rect) -> (f32, f32) {
    match style {
        frames::TOP_PERCENT | frames::TOP_CELLS => (shown.x, work.y - shown.h),
        frames::BOTTOM_PERCENT | frames::BOTTOM_CELLS => (shown.x, work.y + work.h),
        frames::RIGHT_PERCENT | frames::RIGHT_CELLS => (work.x + work.w, shown.y),
        _ => (shown.x, shown.y),
    }
}

/// A slide is allowed unless the hidden frame overlaps a monitor other than `own`.
pub fn slide_allowed(hidden: Rect, own: Rect, monitors: &[Rect]) -> bool {
    let overlaps = |a: Rect, b: Rect| a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
    !monitors.iter().any(|m| *m != own && overlaps(hidden, *m))
}

/// CSS ease-in, cubic-bezier(0.42, 0, 1, 1): progress at time fraction `t`.
pub fn ease_in(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t == 0.0 || t == 1.0 {
        return t;
    }
    let bezier = |s: f64, p1: f64, p2: f64| 3.0 * (1.0 - s) * (1.0 - s) * s * p1 + 3.0 * (1.0 - s) * s * s * p2 + s * s * s;
    // x(s) rises from 0 to 1: find s where x(s) = t, then y(s).
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for _ in 0..40 {
        let mid = (lo + hi) / 2.0;
        if bezier(mid, 0.42, 1.0) < t as f64 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bezier((lo + hi) / 2.0, 0.0, 1.0) as f32
}

/// A window moving from one origin to another.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    pub from: (f32, f32),
    pub to: (f32, f32),
    pub duration: Duration,
}

impl Motion {
    /// The origin `elapsed` into the motion, and whether it is over.
    pub fn at(&self, elapsed: Duration) -> ((f32, f32), bool) {
        if self.duration.is_zero() || elapsed >= self.duration {
            return (self.to, true);
        }
        let p = ease_in(elapsed.as_secs_f32() / self.duration.as_secs_f32());
        ((self.from.0 + (self.to.0 - self.from.0) * p, self.from.1 + (self.to.1 - self.from.1) * p), false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1040.0 };

    #[test]
    fn hidden_origins_follow_iterm2() {
        let top = Rect { x: 0.0, y: 0.0, w: 1920.0, h: 400.0 };
        assert!(slides(frames::TOP_PERCENT) && slides(frames::TOP_CELLS) && slides(frames::BOTTOM_PERCENT) && slides(frames::RIGHT_CELLS));
        assert!(!slides(frames::LEFT_PERCENT) && !slides(frames::LEFT_CELLS), "left windows do not slide in iTerm2");
        assert!(!slides(frames::NORMAL) && !slides(frames::MAXIMIZED) && !slides(frames::CENTERED) && !slides(frames::FULL_SCREEN));
        assert_eq!(hidden_origin(frames::TOP_PERCENT, top, WORK), (0.0, -400.0));
        let bottom = Rect { x: 0.0, y: 640.0, w: 1920.0, h: 400.0 };
        assert_eq!(hidden_origin(frames::BOTTOM_CELLS, bottom, WORK), (0.0, 1040.0));
        let right = Rect { x: 1420.0, y: 0.0, w: 500.0, h: 1040.0 };
        assert_eq!(hidden_origin(frames::RIGHT_PERCENT, right, WORK), (1920.0, 0.0));
        let normal = Rect { x: 100.0, y: 100.0, w: 800.0, h: 600.0 };
        assert_eq!(hidden_origin(frames::NORMAL, normal, WORK), (100.0, 100.0));
    }

    #[test]
    fn slides_stay_on_their_monitor() {
        let upper = Rect { x: 0.0, y: -1080.0, w: 1920.0, h: 1080.0 };
        let lower = Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 };
        // A top window on the lower monitor: hidden above it, which is the upper monitor.
        let hidden = Rect { x: 0.0, y: -400.0, w: 1920.0, h: 400.0 };
        assert!(!slide_allowed(hidden, lower, &[upper, lower]));
        assert!(slide_allowed(hidden, lower, &[lower]), "nothing above: it slides");
        let beside = Rect { x: 1920.0, y: 0.0, w: 1920.0, h: 1080.0 };
        assert!(slide_allowed(hidden, lower, &[lower, beside]), "a monitor beside is not in the way");
    }

    #[test]
    fn ease_in_matches_the_curve() {
        assert_eq!(ease_in(0.0), 0.0);
        assert_eq!(ease_in(1.0), 1.0);
        assert!((ease_in(0.5) - 0.3153).abs() < 0.002, "{}", ease_in(0.5));
        assert!((ease_in(0.25) - 0.0935).abs() < 0.002, "{}", ease_in(0.25));
        let mut last = 0.0;
        for i in 1..=100 {
            let v = ease_in(i as f32 / 100.0);
            assert!(v >= last, "never goes back");
            last = v;
        }
        assert_eq!((ease_in(-1.0), ease_in(2.0)), (0.0, 1.0));
    }

    #[test]
    fn motion_ends_at_the_target() {
        let m = Motion { from: (0.0, -400.0), to: (0.0, 0.0), duration: DURATION };
        assert_eq!(m.at(Duration::ZERO), ((0.0, -400.0), false));
        let (mid, done) = m.at(Duration::from_millis(125));
        assert!(!done && mid.1 > -400.0 && mid.1 < -200.0, "ease-in starts slowly: {mid:?}");
        assert_eq!(m.at(DURATION), ((0.0, 0.0), true));
        assert_eq!(m.at(Duration::from_secs(5)), ((0.0, 0.0), true));
        let instant = Motion { from: (1.0, 1.0), to: (5.0, 5.0), duration: Duration::ZERO };
        assert_eq!(instant.at(Duration::ZERO), ((5.0, 5.0), true));
    }
}

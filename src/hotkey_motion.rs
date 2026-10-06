//! How a hotkey window moves when it shows and hides: the edge styles slide in from outside
//! their edge (top, bottom, and right, as in iTerm2; left does not), over 0.25 s with CSS
//! ease-in, unless the hidden frame would lie on another monitor. Every style
//! also fades (opacity 0 to 1 on a show, 1 to 0 on a hide) over the same time.

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
    /// The window's opacity at the start and at the end (a show runs 0 to 1, a hide 1 to 0).
    pub alpha: (f32, f32),
    /// Whether position and opacity ease in (the style's curve) or run linearly.
    pub ease: bool,
}

impl Motion {
    /// The origin and the opacity `elapsed` into the motion, and whether it is over.
    pub fn at(&self, elapsed: Duration) -> ((f32, f32), f32, bool) {
        if self.duration.is_zero() || elapsed >= self.duration {
            return (self.to, self.alpha.1, true);
        }
        // An edge or left style eases in (position and opacity together, even when the slide is refused); the others are linear.
        let t = elapsed.as_secs_f32() / self.duration.as_secs_f32();
        let p = if self.ease { ease_in(t) } else { t };
        ((self.from.0 + (self.to.0 - self.from.0) * p, self.from.1 + (self.to.1 - self.from.1) * p), self.alpha.0 + (self.alpha.1 - self.alpha.0) * p, false)
    }
}

/// Whether a window style fades while it shows and hides (every style: iTerm2 fades traditional
/// full screen too, `PHK:747-757`; only Lion full screen asserts, and Tandem has none).
pub fn fades(_style: i64) -> bool {
    true
}

/// Whether a style's motion eases in: the edge styles and the left styles, whether or not the
/// slide runs (`PHK:513-535`, `:726-768`); the others are linear.
pub fn eases(style: i64) -> bool {
    slides(style) || matches!(style, frames::LEFT_PERCENT | frames::LEFT_CELLS)
}

/// The pacing of a style's motion.
pub fn curve(style: i64) -> fn(f32) -> f32 {
    if eases(style) { ease_in } else { |t| t }
}

/// The two ways a window can fade, by the surface it presents on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FadePath {
    /// A layered window with `LWA_ALPHA` (a window on the opaque swap chain).
    Layered,
    /// The renderer scales the premultiplied target (a window on the alpha surface).
    Composed,
}

/// Step 0's results (`fade_probe.rs`, `m13u-rulings.txt`). The layered path ships off until a
/// desktop-session check on EXTREME (Intel and NVIDIA) shows non-blank frames (checklist row);
/// flip it to true then. An opaque hotkey window shows and hides at once until it is.
pub const FADE_LAYERED: bool = false;
pub const FADE_COMPOSED: bool = true;

/// How a window fades, or None to show and hide it at once.
pub fn fade_path(alpha_surface: bool, layered_ok: bool, composed_ok: bool, present_failed: bool) -> Option<FadePath> {
    match (present_failed, alpha_surface) {
        (true, _) => None,
        (false, true) => composed_ok.then_some(FadePath::Composed),
        (false, false) => layered_ok.then_some(FadePath::Layered),
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
        let m = Motion { from: (0.0, -400.0), to: (0.0, 0.0), duration: DURATION, alpha: (0.0, 1.0), ease: true };
        assert_eq!(m.at(Duration::ZERO), ((0.0, -400.0), 0.0, false));
        let (mid, _, done) = m.at(Duration::from_millis(125));
        assert!(!done && mid.1 > -400.0 && mid.1 < -200.0, "ease-in starts slowly: {mid:?}");
        assert_eq!(m.at(DURATION), ((0.0, 0.0), 1.0, true));
        assert_eq!(m.at(Duration::from_secs(5)), ((0.0, 0.0), 1.0, true));
        let instant = Motion { from: (1.0, 1.0), to: (5.0, 5.0), duration: Duration::ZERO, alpha: (0.0, 1.0), ease: true };
        assert_eq!(instant.at(Duration::ZERO), ((5.0, 5.0), 1.0, true));
    }

    #[test]
    fn fade_allowed_follows_the_probe() {
        // An opaque-surface window takes the layered path, an alpha-surface window the composed one.
        assert_eq!(fade_path(false, true, true, false), Some(FadePath::Layered));
        assert_eq!(fade_path(true, true, true, false), Some(FadePath::Composed));
        // Each path follows its own probe.
        assert_eq!(fade_path(false, false, true, false), None);
        assert_eq!(fade_path(true, true, false, false), None);
        assert_eq!(fade_path(false, true, false, false), Some(FadePath::Layered));
        assert_eq!(fade_path(true, false, true, false), Some(FadePath::Composed));
        // A present error during a fade ends fading for the rest of the run.
        assert_eq!(fade_path(false, true, true, true), None);
        assert_eq!(fade_path(true, true, true, true), None);
    }

    #[test]
    fn opaque_fade_ships_off_until_confirmed() {
        // Ruling: the layered path is unproven on a desktop session (Intel and NVIDIA), so it ships off.
        assert!(!FADE_LAYERED);
    }

    #[test]
    fn left_and_off_monitor_fades_ease_in() {
        // iTerm2 eases the left styles in too, and an edge style whose slide is refused keeps its curve.
        for s in [frames::LEFT_PERCENT, frames::LEFT_CELLS, frames::TOP_PERCENT, frames::BOTTOM_CELLS, frames::RIGHT_PERCENT] {
            assert!((curve(s)(0.25) - ease_in(0.25)).abs() < 1e-6, "{s}");
            let m = Motion { from: (5.0, 5.0), to: (5.0, 5.0), duration: DURATION, alpha: (0.0, 1.0), ease: eases(s) };
            assert!((m.at(Duration::from_millis(125)).1 - ease_in(0.5)).abs() < 1e-4, "{s}");
        }
        for s in [frames::NORMAL, frames::CENTERED, frames::MAXIMIZED, frames::FULL_SCREEN] {
            assert_eq!(curve(s)(0.25), 0.25, "{s}");
        }
    }

    #[test]
    fn fade_follows_iterm2() {
        let at = |m: &Motion, ms: u64| m.at(Duration::from_millis(ms));
        // A top-style show: alpha 0 to 1 and the origin moving with ease-in, together.
        let top = Motion { from: (0.0, -400.0), to: (0.0, 0.0), duration: DURATION, alpha: (0.0, 1.0), ease: true };
        assert_eq!(at(&top, 0), ((0.0, -400.0), 0.0, false));
        assert_eq!(at(&top, 250), ((0.0, 0.0), 1.0, true));
        let (pos, alpha, _) = at(&top, 125);
        assert!((alpha - ease_in(0.5)).abs() < 1e-4 && (pos.1 + 400.0 * (1.0 - ease_in(0.5))).abs() < 1e-3, "{pos:?} {alpha}");
        // A normal-style show: the origin stays, the alpha is linear.
        let normal = Motion { from: (100.0, 100.0), to: (100.0, 100.0), duration: DURATION, alpha: (0.0, 1.0), ease: false };
        let (pos, alpha, done) = at(&normal, 125);
        assert_eq!((pos, done), ((100.0, 100.0), false));
        assert!((alpha - 0.5).abs() < 1e-4, "{alpha}");
        // A hide runs the alpha from 1 to 0.
        let hide = Motion { alpha: (1.0, 0.0), ..normal };
        assert_eq!(at(&hide, 0).1, 1.0);
        assert!((at(&hide, 125).1 - 0.5).abs() < 1e-4);
        assert_eq!(at(&hide, 250), ((100.0, 100.0), 0.0, true));
        // A left style shows from where it already is, and fades.
        let shown = Rect { x: 0.0, y: 0.0, w: 500.0, h: 1040.0 };
        assert_eq!(hidden_origin(frames::LEFT_PERCENT, shown, WORK), (0.0, 0.0), "the left styles hide in place");
        assert!(fades(frames::LEFT_PERCENT) && fades(frames::LEFT_CELLS));
        // Every style fades; the edges still slide and the others do not.
        for s in [frames::TOP_PERCENT, frames::TOP_CELLS, frames::BOTTOM_PERCENT, frames::BOTTOM_CELLS, frames::RIGHT_PERCENT, frames::RIGHT_CELLS, frames::NORMAL, frames::MAXIMIZED, frames::CENTERED] {
            assert!(fades(s));
        }
        assert!(fades(frames::FULL_SCREEN), "traditional full screen fades (PHK:747-757)");
        assert!(slides(frames::TOP_CELLS) && slides(frames::BOTTOM_PERCENT) && slides(frames::RIGHT_CELLS) && !slides(frames::LEFT_PERCENT));
        // The edges and the left styles ease in, the others are linear.
        assert!((curve(frames::TOP_PERCENT)(0.5) - ease_in(0.5)).abs() < 1e-6);
        assert_eq!(curve(frames::NORMAL)(0.5), 0.5);
        assert!((curve(frames::LEFT_CELLS)(0.25) - ease_in(0.25)).abs() < 1e-6);
    }
}

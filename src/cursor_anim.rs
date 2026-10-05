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

/// `1 - (1 - t)^2`, the slide's easing (`iTermCursorSlideAnimator.m:258-260`).
pub fn ease_out(t: f64) -> f64 {
    1.0 - (1.0 - t) * (1.0 - t)
}

/// Whether a cursor moving from `from` to `to` slides (`PTYTextView.m:1555-1565`): the rows
/// match within one pixel and the horizontal distance is more than 0 and at most
/// `max_cells` cells plus half a cell (the source takes the absolute distance).
pub fn should_slide(from: Rect, to: Rect, cell_w: f32, max_cells: i64) -> bool {
    if (from.y - to.y).abs() > 1.0 {
        return false;
    }
    let dx = (to.x - from.x).abs();
    dx > 0.0 && dx <= cell_w * (max_cells as f32 + 0.5)
}

/// A slide of the cursor between two pixel rects (`iTermCursorSlideAnimator`, Metal path).
#[derive(Clone, Debug, PartialEq)]
pub struct Slide {
    pub from: Rect,
    pub to: Rect,
    pub start: Instant,
}

impl Slide {
    /// How far the drawn cursor is from the end rect at `now`: None once `duration` has
    /// passed (or when it is not positive).
    pub fn offset(&self, now: Instant, duration: Duration) -> Option<(f32, f32)> {
        let p = now.saturating_duration_since(self.start).as_secs_f64() / duration.as_secs_f64();
        if !(p < 1.0) {
            return None;
        }
        let left = 1.0 - ease_out(p.max(0.0)) as f32;
        Some(((self.from.x - self.to.x) * left, (self.from.y - self.to.y) * left))
    }
}

/// What a cursor's pixel rect depends on besides the cell it sits in. When any of it changes
/// the cursor snaps instead of sliding from a stale rect (a zoom, a DPI change, a resize or
/// move of the pane, a scroll).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub cell: (f32, f32),
    pub base: (f32, f32),
    pub scale: f32,
    pub pane: Rect,
    pub scrolled: usize,
}

/// CoreAnimation's easeInEaseOut: the cubic Bezier (0.42, 0) (0.58, 1), solved for x
/// (`CursorSmearView.swift:62`).
pub fn ease_in_out_ca(t: f64) -> f64 {
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    let bez = |a: f64, b: f64, u: f64| 3.0 * (1.0 - u) * (1.0 - u) * u * a + 3.0 * (1.0 - u) * u * u * b + u * u * u;
    let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
    for _ in 0..40 {
        let mid = (lo + hi) / 2.0;
        if bez(0.42, 0.58, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bez(0.0, 1.0, (lo + hi) / 2.0)
}

/// Whether two cursor rects are at least `min_distance_px` apart, origin to origin
/// (`CursorSmearView.swift:31`).
pub fn far_enough(a: Rect, b: Rect, min_distance_px: f32) -> bool {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    dx * dx + dy * dy >= min_distance_px * min_distance_px
}

/// The smear's three hexagon keyframes (`CursorSmearView.swift:77-110`), by the quadrant of
/// `b`'s center from `a`'s (y down, `dx >= 0 && dy >= 0` first).
pub fn smear_keyframes(a: Rect, b: Rect) -> [[(f32, f32); 6]; 3] {
    let corners = |r: Rect| ((r.x, r.y), (r.x + r.w, r.y), (r.x + r.w, r.y + r.h), (r.x, r.y + r.h));
    let (a_tl, a_tr, a_br, a_bl) = corners(a);
    let (b_tl, b_tr, b_br, b_bl) = corners(b);
    let dx = (b.x + b.w / 2.0) - (a.x + a.w / 2.0);
    let dy = (b.y + b.h / 2.0) - (a.y + a.h / 2.0);
    if dx >= 0.0 && dy >= 0.0 {
        [[a_tl, a_tr, a_tr, a_br, a_bl, a_bl], [a_tl, a_tr, b_tr, b_br, b_bl, a_bl], [b_tl, b_tr, b_tr, b_br, b_bl, b_bl]]
    } else if dx < 0.0 && dy >= 0.0 {
        [[a_tr, a_br, a_br, a_bl, a_tl, a_tl], [a_tr, a_br, b_br, b_bl, b_tl, a_tl], [b_tr, b_br, b_br, b_bl, b_tl, b_tl]]
    } else if dx < 0.0 {
        [[a_tl, a_tr, a_tr, a_br, a_bl, a_bl], [b_tl, b_tr, a_tr, a_br, a_bl, b_bl], [b_tl, b_tr, b_tr, b_br, b_bl, b_bl]]
    } else {
        [[a_bl, a_tl, a_tl, a_tr, a_br, a_br], [a_bl, a_tl, b_tl, b_tr, b_br, a_br], [b_bl, b_tl, b_tl, b_tr, b_br, b_br]]
    }
}

/// The hexagon at `progress` (0 to 1) along the keyframes, which sit at 0, 0.5 and 1.
pub fn smear_at(keys: &[[(f32, f32); 6]; 3], progress: f64) -> [(f32, f32); 6] {
    let p = progress.clamp(0.0, 1.0);
    let (from, to, f) = if p < 0.5 { (&keys[0], &keys[1], p * 2.0) } else { (&keys[1], &keys[2], (p - 0.5) * 2.0) };
    let f = f as f32;
    let mut out = *from;
    for i in 0..6 {
        out[i] = (from[i].0 + (to[i].0 - from[i].0) * f, from[i].1 + (to[i].1 - from[i].1) * f);
    }
    out
}

/// A smear of the cursor between two pixel rects (`CursorSmearView`).
#[derive(Clone, Debug, PartialEq)]
pub struct Smear {
    pub keys: [[(f32, f32); 6]; 3],
    pub start: Instant,
}

impl Smear {
    /// The hexagon to fill at `now`: None once `duration` has passed (or when it is not
    /// positive). The timing is easeInEaseOut.
    pub fn polygon(&self, now: Instant, duration: Duration) -> Option<[(f32, f32); 6]> {
        let p = now.saturating_duration_since(self.start).as_secs_f64() / duration.as_secs_f64();
        if !(p < 1.0) {
            return None;
        }
        Some(smear_at(&self.keys, ease_in_out_ca(p)))
    }
}

/// What decides a move's animation: the profile's switches and the advanced limits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoveRule {
    pub slide_ok: bool,
    pub max_cells: i64,
    pub cell_w: f32,
    pub smear_ok: bool,
    pub min_distance: f32,
}

/// The animation a move starts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Started {
    Slide(Rect, Rect),
    Smear(Rect, Rect),
}

fn inside(r: Rect, visible: Rect) -> bool {
    r.x >= visible.x && r.y >= visible.y && r.x + r.w <= visible.x + visible.w && r.y + r.h <= visible.y + visible.h
}

/// What one pane remembers about its cursor from frame to frame.
#[derive(Clone, Debug, Default)]
pub struct Motion {
    /// The screen-relative grid cell (row, column) the cursor was last seen at.
    pub cell: Option<(i64, usize)>,
    pub moved: Option<Instant>,
    pub last_rect: Option<Rect>,
    pub slide: Option<Slide>,
    pub smear: Option<Smear>,
    /// What `last_rect` was measured with.
    pub metrics: Option<Metrics>,
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

    /// Notes the metrics a frame's rects are measured with. When they differ from the last
    /// frame's, the remembered rect and any slide or smear are dropped; returns whether it did.
    pub fn metrics_changed(&mut self, m: Metrics) -> bool {
        let changed = self.metrics.is_some_and(|old| old != m);
        if changed {
            self.last_rect = None;
            self.slide = None;
            self.smear = None;
        }
        self.metrics = Some(m);
        changed
    }

    /// Notes the cursor's pixel rect for a frame and answers the (from, to) pair of a move
    /// that slides (`PTYTextView.m:1590-1644`): both rects fully inside `visible`, different,
    /// a previous rect known, sliding allowed and `should_slide`. `last_rect` always becomes
    /// `rect`.
    pub fn moved_to(&mut self, rect: Rect, visible: Rect, slide_ok: bool, max_cells: i64, cell_w: f32, _now: Instant) -> Option<(Rect, Rect)> {
        let prev = self.last_rect.replace(rect)?;
        (slide_ok && prev != rect && inside(prev, visible) && inside(rect, visible) && should_slide(prev, rect, cell_w, max_cells)).then_some((prev, rect))
    }

    /// Notes the pixel rect and answers the animation the move starts: a slide when it
    /// qualifies, else a smear when allowed and far enough (`PTYTextView.m:1620-1641`: the
    /// slide wins when both apply).
    pub fn classify(&mut self, rect: Rect, visible: Rect, rule: MoveRule) -> Option<Started> {
        let prev = self.last_rect;
        if let Some((from, to)) = self.moved_to(rect, visible, rule.slide_ok, rule.max_cells, rule.cell_w, Instant::now()) {
            return Some(Started::Slide(from, to));
        }
        let prev = prev?;
        (rule.smear_ok && prev != rect && inside(prev, visible) && inside(rect, visible) && far_enough(prev, rect, rule.min_distance)).then_some(Started::Smear(prev, rect))
    }

    /// Starts a smear; it replaces a running one.
    pub fn begin_smear(&mut self, from: Rect, to: Rect, now: Instant) {
        self.smear = Some(Smear { keys: smear_keyframes(from, to), start: now });
    }

    /// Starts a slide. One still running hands over where it is drawn now as the start
    /// (`currentAnimatedPositionFrom:`).
    pub fn begin_slide(&mut self, from: Rect, to: Rect, now: Instant, duration: Duration) {
        let from = match self.slide.as_ref().and_then(|s| s.offset(now, duration).map(|o| (s, o))) {
            Some((s, (dx, dy))) => Rect { x: s.to.x + dx, y: s.to.y + dy, ..s.to },
            None => from,
        };
        self.slide = Some(Slide { from, to, start: now });
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

    fn r(x: f32, y: f32) -> Rect {
        Rect { x, y, w: 1.0, h: 20.0 }
    }

    #[test]
    fn slide_rule_follows_iterm2() {
        let from = Rect { x: 20.0, y: 26.0, w: 1.0, h: 20.0 };
        assert!(should_slide(from, Rect { x: 30.0, ..from }, 10.0, 1));
        assert!(!should_slide(from, Rect { x: 40.0, ..from }, 10.0, 1), "20 is past 15");
        assert!(should_slide(from, Rect { x: 40.0, ..from }, 10.0, 2));
        assert!(should_slide(from, Rect { x: 30.0, y: 27.0, ..from }, 10.0, 1), "within 1 pixel");
        assert!(!should_slide(from, Rect { x: 30.0, y: 50.0, ..from }, 10.0, 1));
        assert!(!should_slide(from, Rect { x: 30.0, y: 27.5, ..from }, 10.0, 1));
        assert!(!should_slide(from, from, 10.0, 1), "no move");
        // iTerm2 takes the absolute distance (`PTYTextView.m:1561`): a step left slides too.
        assert!(should_slide(from, Rect { x: 10.0, ..from }, 10.0, 1));
        assert!(!should_slide(from, Rect { x: 0.0, ..from }, 10.0, 1));
    }

    #[test]
    fn slide_offset_follows_iterm2() {
        let t0 = Instant::now();
        let s = Slide { from: r(20.0, 26.0), to: r(30.0, 26.0), start: t0 };
        let d = ms(50);
        assert_eq!(s.offset(t0, d), Some((-10.0, 0.0)));
        assert_eq!(s.offset(t0 + ms(25), d), Some((-2.5, 0.0)));
        assert_eq!(s.offset(t0 + ms(50), d), None);
        assert_eq!(s.offset(t0 + ms(80), d), None);
        assert_eq!(s.offset(t0, Duration::ZERO), None, "a zero duration never slides");
        assert_eq!(ease_out(0.0), 0.0);
        assert_eq!(ease_out(0.5), 0.75);
        assert_eq!(ease_out(1.0), 1.0);
        // A step left, one pixel off the row.
        let s = Slide { from: r(30.0, 27.0), to: r(20.0, 26.0), start: t0 };
        assert_eq!(s.offset(t0, d), Some((10.0, 1.0)));
    }

    #[test]
    fn motion_starts_a_slide() {
        let t0 = Instant::now();
        let visible = Rect { x: 0.0, y: 0.0, w: 100.0, h: 100.0 };
        let mut m = Motion::default();
        let a = r(20.0, 26.0);
        let b = r(30.0, 26.0);
        assert_eq!(m.moved_to(a, visible, true, 1, 10.0, t0), None, "the first rect has no previous one");
        assert_eq!(m.last_rect, Some(a));
        assert_eq!(m.moved_to(b, visible, true, 1, 10.0, t0), Some((a, b)));
        assert_eq!(m.last_rect, Some(b));
        // The same rect is no move.
        assert_eq!(m.moved_to(b, visible, true, 1, 10.0, t0), None);
        // A setting or cursor type that does not slide answers None and still records the rect.
        assert_eq!(m.moved_to(a, visible, false, 1, 10.0, t0), None);
        assert_eq!(m.last_rect, Some(a));
        // A jump past the maximum does not slide.
        assert_eq!(m.moved_to(r(60.0, 26.0), visible, true, 1, 10.0, t0), None);
        // A rect partly outside the visible area answers None and becomes last_rect, and the
        // move back in is not a slide either.
        let out = r(95.0, 26.0);
        let out = Rect { w: 10.0, ..out };
        assert_eq!(m.moved_to(r(90.0, 26.0), visible, true, 1, 10.0, t0), None);
        assert_eq!(m.moved_to(out, visible, true, 1, 10.0, t0), None);
        assert_eq!(m.last_rect, Some(out));
        assert_eq!(m.moved_to(r(90.0, 26.0), visible, true, 1, 10.0, t0), None);
    }

    #[test]
    fn a_new_slide_continues_from_the_drawn_position() {
        let t0 = Instant::now();
        let d = ms(50);
        let mut m = Motion::default();
        m.begin_slide(r(20.0, 26.0), r(30.0, 26.0), t0, d);
        assert_eq!(m.slide.as_ref().unwrap().offset(t0, d), Some((-10.0, 0.0)));
        // Halfway (drawn at 27.5), the cursor moves on to 40: the new slide starts there.
        m.begin_slide(r(30.0, 26.0), r(40.0, 26.0), t0 + ms(25), d);
        let s = m.slide.clone().unwrap();
        assert_eq!(s.from, r(27.5, 26.0));
        assert_eq!(s.to, r(40.0, 26.0));
        assert_eq!(s.start, t0 + ms(25));
        // A finished slide does not carry over.
        m.begin_slide(r(40.0, 26.0), r(50.0, 26.0), t0 + ms(200), d);
        assert_eq!(m.slide.unwrap().from, r(40.0, 26.0));
    }

    #[test]
    fn a_change_of_metrics_snaps() {
        let t0 = Instant::now();
        let d = ms(50);
        let mut m = Motion::default();
        let k = Metrics { cell: (10.0, 20.0), base: (10.0, 17.0), scale: 1.0, pane: r(0.0, 0.0), scrolled: 0 };
        assert!(!m.metrics_changed(k), "the first frame has nothing to snap from");
        m.last_rect = Some(r(20.0, 26.0));
        m.begin_slide(r(20.0, 26.0), r(30.0, 26.0), t0, d);
        assert!(!m.metrics_changed(k));
        assert!(m.slide.is_some());
        // A zoom, a DPI change, a resize or move of the pane, and a scroll each clear both.
        for k2 in [
            Metrics { cell: (12.0, 24.0), ..k },
            Metrics { base: (12.0, 20.0), ..k },
            Metrics { scale: 1.5, ..k },
            Metrics { pane: Rect { x: 5.0, ..k.pane }, ..k },
            Metrics { pane: Rect { w: 9.0, ..k.pane }, ..k },
            Metrics { scrolled: 3, ..k },
        ] {
            m.metrics_changed(k);
            m.last_rect = Some(r(20.0, 26.0));
            m.begin_slide(r(20.0, 26.0), r(30.0, 26.0), t0, d);
            assert!(m.metrics_changed(k2), "{k2:?}");
            assert_eq!((m.last_rect, m.slide.is_some()), (None, false));
        }
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

    #[test]
    fn ease_in_out_ca_follows_core_animation() {
        let near = |a: f64, b: f64| assert!((a - b).abs() < 0.0005, "{a} vs {b}");
        assert_eq!(ease_in_out_ca(0.0), 0.0);
        assert_eq!(ease_in_out_ca(1.0), 1.0);
        near(ease_in_out_ca(0.5), 0.5);
        near(ease_in_out_ca(0.25), 0.1292);
        near(ease_in_out_ca(0.75), 0.8708);
        near(ease_in_out_ca(0.1), 0.0197);
    }

    fn rect(x: f32, y: f32) -> Rect {
        Rect { x, y, w: 10.0, h: 20.0 }
    }

    #[test]
    fn smear_keyframes_follow_iterm2() {
        let a = rect(0.0, 0.0);
        let k = smear_keyframes(a, rect(100.0, 200.0));
        assert_eq!(k[0], [(0.0, 0.0), (10.0, 0.0), (10.0, 0.0), (10.0, 20.0), (0.0, 20.0), (0.0, 20.0)]);
        assert_eq!(k[1], [(0.0, 0.0), (10.0, 0.0), (110.0, 200.0), (110.0, 220.0), (100.0, 220.0), (0.0, 20.0)]);
        assert_eq!(k[2], [(100.0, 200.0), (110.0, 200.0), (110.0, 200.0), (110.0, 220.0), (100.0, 220.0), (100.0, 220.0)]);
        // Up left.
        let k = smear_keyframes(a, rect(-100.0, -200.0));
        assert_eq!(k[0], [(0.0, 0.0), (10.0, 0.0), (10.0, 0.0), (10.0, 20.0), (0.0, 20.0), (0.0, 20.0)]);
        assert_eq!(k[1], [(-100.0, -200.0), (-90.0, -200.0), (10.0, 0.0), (10.0, 20.0), (0.0, 20.0), (-100.0, -180.0)]);
        assert_eq!(k[2], [(-100.0, -200.0), (-90.0, -200.0), (-90.0, -200.0), (-90.0, -180.0), (-100.0, -180.0), (-100.0, -180.0)]);
        // Down left: [aTR, aBR, aBR, aBL, aTL, aTL] / [aTR, aBR, bBR, bBL, bTL, aTL] / [bTR, bBR, bBR, bBL, bTL, bTL].
        let k = smear_keyframes(a, rect(-100.0, 200.0));
        assert_eq!(k[0], [(10.0, 0.0), (10.0, 20.0), (10.0, 20.0), (0.0, 20.0), (0.0, 0.0), (0.0, 0.0)]);
        assert_eq!(k[1], [(10.0, 0.0), (10.0, 20.0), (-90.0, 220.0), (-100.0, 220.0), (-100.0, 200.0), (0.0, 0.0)]);
        assert_eq!(k[2], [(-90.0, 200.0), (-90.0, 220.0), (-90.0, 220.0), (-100.0, 220.0), (-100.0, 200.0), (-100.0, 200.0)]);
        // Up right: [aBL, aTL, aTL, aTR, aBR, aBR] / [aBL, aTL, bTL, bTR, bBR, aBR] / [bBL, bTL, bTL, bTR, bBR, bBR].
        let k = smear_keyframes(a, rect(100.0, -200.0));
        assert_eq!(k[0], [(0.0, 20.0), (0.0, 0.0), (0.0, 0.0), (10.0, 0.0), (10.0, 20.0), (10.0, 20.0)]);
        assert_eq!(k[1], [(0.0, 20.0), (0.0, 0.0), (100.0, -200.0), (110.0, -200.0), (110.0, -180.0), (10.0, 20.0)]);
        assert_eq!(k[2], [(100.0, -180.0), (100.0, -200.0), (100.0, -200.0), (110.0, -200.0), (110.0, -180.0), (110.0, -180.0)]);
        // The path runs through the keyframes at progress 0, 0.5 and 1.
        let k = smear_keyframes(a, rect(100.0, 200.0));
        assert_eq!(smear_at(&k, 0.0), k[0]);
        assert_eq!(smear_at(&k, 0.5), k[1]);
        assert_eq!(smear_at(&k, 1.0), k[2]);
        let half = |x: [(f32, f32); 6], y: [(f32, f32); 6]| {
            let mut o = x;
            for i in 0..6 {
                o[i] = ((x[i].0 + y[i].0) / 2.0, (x[i].1 + y[i].1) / 2.0);
            }
            o
        };
        assert_eq!(smear_at(&k, 0.25), half(k[0], k[1]));
        assert_eq!(smear_at(&k, 0.75), half(k[1], k[2]));
    }

    #[test]
    fn smear_rule_follows_iterm2() {
        let a = rect(0.0, 0.0);
        assert!(far_enough(a, rect(150.0, 0.0), 150.0));
        assert!(!far_enough(a, rect(149.9, 0.0), 150.0));
        assert!(far_enough(a, rect(90.0, 120.0), 150.0), "the distance is Euclidean");
        let t0 = Instant::now();
        let b = rect(100.0, 200.0);
        let keys = smear_keyframes(a, b);
        let s = Smear { keys, start: t0 };
        let d = ms(100);
        assert_eq!(s.polygon(t0, d), Some(keys[0]));
        assert_eq!(s.polygon(t0 + ms(50), d), Some(keys[1]));
        assert_eq!(s.polygon(t0 + ms(100), d), None);
        assert_eq!(s.polygon(t0, Duration::ZERO), None);
    }

    #[test]
    fn motion_starts_a_smear() {
        let t0 = Instant::now();
        let visible = Rect { x: 0.0, y: 0.0, w: 1000.0, h: 1000.0 };
        let rule = MoveRule { slide_ok: true, max_cells: 1, cell_w: 10.0, smear_ok: true, min_distance: 150.0 };
        let mut m = Motion::default();
        let a = rect(20.0, 20.0);
        assert_eq!(m.classify(a, visible, rule), None, "no previous rect");
        // A one-cell step on a sliding cursor slides, even with the smear enabled.
        assert_eq!(m.classify(rect(30.0, 20.0), visible, rule), Some(Started::Slide(rect(20.0, 20.0), rect(30.0, 20.0))));
        // A near move that does not slide does not smear.
        assert_eq!(m.classify(rect(80.0, 20.0), visible, rule), None);
        // A far move smears, from the previous rect.
        assert_eq!(m.classify(rect(400.0, 300.0), visible, rule), Some(Started::Smear(rect(80.0, 20.0), rect(400.0, 300.0))));
        // Smear off: nothing. A move out of the visible area: nothing.
        assert_eq!(m.classify(rect(0.0, 0.0), visible, MoveRule { smear_ok: false, ..rule }), None);
        assert_eq!(m.classify(rect(2000.0, 0.0), visible, rule), None);
        assert_eq!(m.classify(rect(0.0, 0.0), visible, rule), None, "the previous rect was outside");
        // A new smear replaces a running one.
        m.begin_smear(a, rect(400.0, 300.0), t0);
        let first = m.smear.clone().unwrap();
        m.begin_smear(rect(400.0, 300.0), a, t0 + ms(30));
        assert_ne!(m.smear.unwrap(), first);
    }
}

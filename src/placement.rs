//! Where a new window goes: iTerm2's four placement policies (WindowInitialPositioner),
//! the per-profile saved slots, smart layout, and the Screen setting.


use std::collections::{BTreeSet, HashMap};

pub use crate::layout::Rect;

/// `WindowPlacement`: Windows decides; Smart Window Placement; Remember the size and
/// position; Remember the position and use the profile's size (the default).
pub const SYSTEM: i64 = 0;
pub const SMART: i64 = 1;
pub const SIZE_AND_POSITION: i64 = 2;
pub const POSITION: i64 = 3;

/// The key a window's saved frame is stored under: "<profile guid> <slot>".
pub fn slot_key(guid: &str, slot: u32) -> String {
    format!("{guid} {slot}")
}

/// Slot numbers in use per profile; each open window of a profile holds the lowest free.
#[derive(Clone, Debug, Default)]
pub struct Slots {
    taken: HashMap<String, BTreeSet<u32>>,
}

impl Slots {
    pub fn take(&mut self, guid: &str) -> u32 {
        let used = self.taken.entry(guid.to_string()).or_default();
        let n = (0..).find(|n| !used.contains(n)).unwrap_or(0);
        used.insert(n);
        n
    }

    pub fn give(&mut self, guid: &str, slot: u32) {
        if let Some(u) = self.taken.get_mut(guid) {
            u.remove(&slot);
        }
    }
}

fn overlap(a: Rect, b: Rect) -> f32 {
    let w = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
    let h = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
    if w > 0.0 && h > 0.0 { w * h } else { 0.0 }
}

/// iTerm2's smartLayout: the top-left point on a 50-pixel grid over the work area with the
/// least overlap with other windows, then the nearest the center.
pub fn smart(work: Rect, size: (f32, f32), others: &[Rect]) -> (f32, f32) {
    const STRIDE: f32 = 50.0;
    let scan_w = (work.w - size.0).max(1.0);
    let scan_h = (work.h - size.1).max(1.0);
    let center = (work.x + work.w / 2.0, work.y + work.h / 2.0);
    let max_d = (work.w * work.w + work.h * work.h).sqrt().max(1.0);
    let mut best = (f32::INFINITY, (work.x, work.y));
    let mut dx = 0.0;
    while dx < scan_w {
        let mut dy = 0.0;
        while dy < scan_h {
            let r = Rect { x: work.x + dx, y: work.y + dy, w: size.0, h: size.1 };
            let c = (r.x + r.w / 2.0, r.y + r.h / 2.0);
            let d = ((c.0 - center.0).powi(2) + (c.1 - center.1).powi(2)).sqrt() / max_d;
            let cost = others.iter().map(|o| overlap(r, *o)).sum::<f32>() + d;
            if cost < best.0 {
                best = (cost, (r.x, r.y));
            }
            dy += STRIDE;
        }
        dx += STRIDE;
    }
    best.1
}

/// Monitors with the primary one (the one holding (0, 0)) first, as iTerm2 numbers screens.
pub fn primary_first(mut monitors: Vec<Rect>) -> Vec<Rect> {
    if let Some(i) = monitors.iter().position(|m| m.x <= 0.0 && m.y <= 0.0 && m.x + m.w > 0.0 && m.y + m.h > 0.0) {
        let p = monitors.remove(i);
        monitors.insert(0, p);
    }
    monitors
}

/// A frame on no monitor moves onto the first one, keeping its size as
/// far as it fits (restoreWindowsWithinScreens).
pub fn clamp_to_monitors(r: Rect, monitors: &[Rect]) -> Rect {
    // Any overlap counts: Windows puts a maximized or snapped frame a few pixels outside.
    if monitors.iter().any(|m| overlap(r, *m) > 0.0) {
        return r;
    }
    let Some(m) = monitors.first() else { return r };
    let (w, h) = (r.w.min(m.w), r.h.min(m.h));
    Rect { x: r.x.clamp(m.x, m.x + m.w - w), y: r.y.clamp(m.y, m.y + m.h - h), w, h }
}

/// restoreWindowsWithinScreens for a restored window: a frame on no monitor moves onto the
/// first one, then, as iTerm2 does, a frame sticking out of the box around all monitors
/// moves in (never resized; the left and top edges win when it is larger).
pub fn within_screens(r: Rect, monitors: &[Rect]) -> Rect {
    let r = clamp_to_monitors(r, monitors);
    let Some(first) = monitors.first() else { return r };
    let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.x + first.w, first.y + first.h);
    for m in monitors {
        (x0, y0, x1, y1) = (x0.min(m.x), y0.min(m.y), x1.max(m.x + m.w), y1.max(m.y + m.h));
    }
    let fit = |p: f32, size: f32, lo: f32, hi: f32| {
        if p < lo {
            lo
        } else if p + size > hi {
            p - (p + size - hi).min(p - lo)
        } else {
            p
        }
    };
    Rect { x: fit(r.x, r.w, x0, x1), y: fit(r.y, r.h, y0, y1), ..r }
}

/// The frame for a new window, or None to leave it where Windows put it. `saved` is this
/// profile slot's last frame; `current` holds the profile's size.
pub fn place(policy: i64, saved: Option<Rect>, windows: usize, work: Rect, others: &[Rect], current: Rect, monitors: &[Rect]) -> Option<Rect> {
    let smart_rect = || {
        let (x, y) = smart(work, (current.w, current.h), others);
        Rect { x, y, w: current.w, h: current.h }
    };
    let at_saved = |s: Rect| clamp_to_monitors(Rect { x: s.x, y: s.y, w: current.w, h: current.h }, monitors);
    match policy {
        SYSTEM => None,
        SIZE_AND_POSITION => saved.map(|s| clamp_to_monitors(s, monitors)),
        SMART if windows != 1 => Some(smart_rect()),
        SMART | POSITION => Some(saved.map_or_else(smart_rect, at_saved)),
        _ => None,
    }
}

/// The profile's Screen setting: -1 the window's monitor, -2 the monitor with the cursor,
/// n the n-th monitor (the primary first); a missing one falls back to the window's.
pub fn screen_for(pref: i64, monitors: &[Rect], window_monitor: usize, cursor_monitor: usize) -> Rect {
    let pick = |i: usize| monitors.get(i).copied();
    let fallback = pick(window_monitor).or_else(|| monitors.first().copied()).unwrap_or(Rect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 });
    match pref {
        -2 => pick(cursor_monitor).unwrap_or(fallback),
        n if n >= 0 => pick(n as usize).unwrap_or(fallback),
        _ => fallback,
    }
}

/// The monitor a window is anchored to for a Screen preference (`PT:567-578`, `PT:15306-15315`):
/// the n-th monitor when in range, the cursor's for -2, none otherwise.
pub fn anchor(pref: i64, monitors: usize, cursor_monitor: usize) -> Option<usize> {
    match pref {
        -2 => (cursor_monitor < monitors).then_some(cursor_monitor),
        n if n >= 0 && (n as usize) < monitors => Some(n as usize),
        _ => None,
    }
}

/// The monitor an anchored window should go back to (`PT:5393-5403`): its anchor when that
/// monitor still exists and the window is on another one.
pub fn chase(anchor: Option<usize>, monitors: usize, window_monitor: usize) -> Option<usize> {
    anchor.filter(|a| *a < monitors && *a != window_monitor)
}

/// `UpdateScreenParamsDelay`: how long a burst of display messages settles before windows refit.
pub fn display_delay(seconds: f64) -> std::time::Duration {
    if seconds.is_nan() {
        return std::time::Duration::from_secs(1);
    }
    std::time::Duration::from_secs_f64(seconds.clamp(0.0, 3600.0))
}

/// Window > Arrange Windows Horizontally: side by side across the work area, left to right
/// in the order the windows stood, at least 400 wide (more rows when they do not fit);
/// each keeps its height up to its row's share.
pub fn arrange_horizontally(work: Rect, windows: &[Rect]) -> Vec<Rect> {
    const MIN_W: f32 = 400.0;
    let n = windows.len();
    if n == 0 {
        return Vec::new();
    }
    let mut w = (work.w / n as f32).floor();
    let per_row = if w < MIN_W { ((work.w / MIN_W).floor() as usize).max(1) } else { n };
    if w < MIN_W {
        w = (work.w / per_row as f32).floor();
    }
    let rows = n.div_ceil(per_row);
    let row_h = (work.h / rows as f32).floor();
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|a, b| windows[*a].x.total_cmp(&windows[*b].x));
    let mut out = vec![Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 }; n];
    for (slot, i) in order.into_iter().enumerate() {
        let (row, col) = (slot / per_row, slot % per_row);
        out[i] = Rect { x: work.x + col as f32 * w, y: work.y + row as f32 * row_h, w, h: windows[i].h.min(row_h) };
    }
    out
}

/// A frame moved to another monitor at the same offset from its top-left.
pub fn move_to(r: Rect, from: Rect, to: Rect) -> Rect {
    clamp_to_monitors(Rect { x: to.x + (r.x - from.x), y: to.y + (r.y - from.y), ..r }, &[to])
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect { x: 0.0, y: 0.0, w: 1000.0, h: 800.0 };
    const SIZE: (f32, f32) = (400.0, 300.0);

    #[test]
    fn policies_follow_iterm2() {
        let saved = Some(Rect { x: 50.0, y: 60.0, w: 700.0, h: 500.0 });
        let current = Rect { x: 0.0, y: 0.0, w: SIZE.0, h: SIZE.1 };
        let mons = [WORK];
        assert_eq!(place(SYSTEM, saved, 1, WORK, &[], current, &mons), None, "Windows decides");
        assert_eq!(place(SIZE_AND_POSITION, saved, 1, WORK, &[], current, &mons), saved);
        assert_eq!(place(POSITION, saved, 1, WORK, &[], current, &mons), Some(Rect { x: 50.0, y: 60.0, w: SIZE.0, h: SIZE.1 }), "position only, the profile's size");
        assert!(place(POSITION, None, 1, WORK, &[], current, &mons).is_some(), "nothing saved: smart layout");
        assert_eq!(place(SMART, saved, 1, WORK, &[], current, &mons).map(|r| (r.x, r.y)), Some((50.0, 60.0)), "the only window: its saved spot");
        // iTerm2 scans x below (work width - window width), 0 to 550 here.
        let other = [Rect { x: 0.0, y: 0.0, w: 500.0, h: 800.0 }];
        let r = place(SMART, saved, 2, WORK, &other, current, &mons).unwrap();
        assert!(r.x >= 500.0, "away from the other window: {r:?}");
    }

    #[test]
    fn smart_layout_avoids_overlap() {
        assert_eq!(smart(WORK, SIZE, &[]), (300.0, 250.0), "nothing else: nearest the center on the 50 px grid");
        let left = [Rect { x: 0.0, y: 0.0, w: 500.0, h: 800.0 }];
        let (x, _) = smart(WORK, SIZE, &left);
        assert!(x >= 500.0, "{x}");
    }

    #[test]
    fn frames_shift_into_the_screens() {
        let right = Rect { x: 1000.0, y: 0.0, w: 800.0, h: 600.0 };
        let mons = [WORK, right];
        // Sticking out on the right of the second monitor: moved left, size kept.
        let r = within_screens(Rect { x: 1500.0, y: 50.0, w: 400.0, h: 300.0 }, &mons);
        assert_eq!(r, Rect { x: 1400.0, y: 50.0, w: 400.0, h: 300.0 });
        // Above the top: moved down.
        assert_eq!(within_screens(Rect { x: 10.0, y: -40.0, w: 100.0, h: 100.0 }, &mons).y, 0.0);
        // Larger than all monitors: left edge in, size kept.
        let big = within_screens(Rect { x: -50.0, y: 0.0, w: 3000.0, h: 500.0 }, &mons);
        assert_eq!((big.x, big.w), (0.0, 3000.0));
        // On no monitor at all: today's move onto the first monitor still happens.
        let lost = within_screens(Rect { x: 5000.0, y: 5000.0, w: 100.0, h: 100.0 }, &mons);
        assert!(lost.x < 1000.0 && lost.y < 800.0, "{lost:?}");
        let fine = Rect { x: 10.0, y: 10.0, w: 100.0, h: 100.0 };
        assert_eq!(within_screens(fine, &mons), fine);
    }

    #[test]
    fn saved_frames_stay_on_screen() {
        let gone = Rect { x: 3000.0, y: 100.0, w: 400.0, h: 300.0 };
        let r = clamp_to_monitors(gone, &[WORK]);
        assert!(r.x >= 0.0 && r.x + r.w <= 1000.0 && r.y + r.h <= 800.0, "{r:?}");
        let fine = Rect { x: 10.0, y: 10.0, w: 100.0, h: 100.0 };
        assert_eq!(clamp_to_monitors(fine, &[WORK]), fine);
        let mut s = Slots::default();
        assert_eq!((s.take("g"), s.take("g"), s.take("h")), (0, 1, 0));
        s.give("g", 0);
        assert_eq!(s.take("g"), 0, "the lowest free slot per profile");
        assert_eq!(slot_key("g", 1), "g 1");
    }

    #[test]
    fn screen_preference_moves_the_frame() {
        let right = Rect { x: 1000.0, y: 0.0, w: 800.0, h: 600.0 };
        let mons = [WORK, right];
        assert_eq!(screen_for(-1, &mons, 0, 1), WORK, "no preference: the window's monitor");
        assert_eq!(screen_for(-2, &mons, 0, 1), right, "the monitor with the cursor");
        assert_eq!(screen_for(1, &mons, 0, 0), right);
        assert_eq!(screen_for(7, &mons, 0, 0), WORK, "a missing monitor: the window's");
        let moved = move_to(Rect { x: 100.0, y: 50.0, w: 300.0, h: 200.0 }, WORK, right);
        assert_eq!(moved, Rect { x: 1100.0, y: 50.0, w: 300.0, h: 200.0 }, "the same offset on the other monitor");
    }

    #[test]
    fn arrange_horizontally_tiles_by_x() {
        let work = Rect { x: 0.0, y: 0.0, w: 1500.0, h: 900.0 };
        let r = arrange_horizontally(work, &[Rect { x: 900.0, y: 0.0, w: 300.0, h: 400.0 }, Rect { x: 10.0, y: 0.0, w: 300.0, h: 400.0 }, Rect { x: 400.0, y: 0.0, w: 300.0, h: 400.0 }]);
        assert_eq!(r.iter().map(|x| x.x).collect::<Vec<_>>(), vec![1000.0, 0.0, 500.0], "left to right in the order they stood");
        assert!(r.iter().all(|x| x.w == 500.0 && x.y == 0.0 && x.h == 400.0));
        let five = arrange_horizontally(work, &[Rect { x: 0.0, y: 0.0, w: 300.0, h: 800.0 }; 5]);
        assert!(five.iter().all(|x| x.w == 500.0), "never under 400 wide: three a row");
        assert_eq!(five.iter().filter(|x| x.y > 0.0).count(), 2, "a second row");
        assert!(five.iter().all(|x| x.y + x.h <= 900.0));
    }

    #[test]
    fn a_frame_on_a_monitor_edge_stays() {
        let left = Rect { x: -1920.0, y: 0.0, w: 1920.0, h: 1040.0 };
        let right = Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1040.0 };
        // Windows puts a maximized or snapped window's outer frame a few pixels outside.
        let snapped = Rect { x: -1927.0, y: -7.0, w: 974.0, h: 1054.0 };
        assert_eq!(clamp_to_monitors(snapped, &[right, left]), snapped, "it still overlaps its own monitor");
        let far = Rect { x: 5000.0, y: 0.0, w: 300.0, h: 200.0 };
        assert!(clamp_to_monitors(far, &[right, left]).x < 1920.0);
    }

    #[test]
    fn the_primary_monitor_comes_first() {
        let left = Rect { x: -1920.0, y: 0.0, w: 1920.0, h: 1040.0 };
        let primary = Rect { x: 0.0, y: 0.0, w: 2560.0, h: 1400.0 };
        assert_eq!(primary_first(vec![left, primary]), vec![primary, left], "the primary monitor holds (0, 0)");
    }

    /// `PT:567-578`, `PT:15306-15315`: a Screen preference in range anchors the window, -2
    /// anchors it to the cursor's monitor, anything else clears the anchor.
    #[test]
    fn anchor_follows_iterm2() {
        assert_eq!(anchor(1, 2, 0), Some(1));
        assert_eq!(anchor(0, 2, 1), Some(0));
        assert_eq!(anchor(3, 2, 0), None);
        assert_eq!(anchor(-1, 2, 0), None);
        assert_eq!(anchor(-2, 2, 1), Some(1));
    }

    /// `PT:5393-5403`: only an anchored window on another monitor moves, to its anchor.
    #[test]
    fn chase_follows_iterm2() {
        assert_eq!(chase(Some(1), 2, 0), Some(1));
        assert_eq!(chase(Some(1), 2, 1), None);
        assert_eq!(chase(Some(2), 2, 0), None, "the anchored screen is gone");
        assert_eq!(chase(None, 2, 0), None);
    }

    /// `AS:485`: `UpdateScreenParamsDelay`, default 1 second; a negative delay is 0.
    #[test]
    fn display_delay_follows_iterm2() {
        use std::time::Duration;
        assert_eq!(display_delay(1.0), Duration::from_secs(1));
        assert_eq!(display_delay(0.25), Duration::from_millis(250));
        assert_eq!(display_delay(-3.0), Duration::ZERO);
        assert_eq!(display_delay(f64::NAN), Duration::from_secs(1));
    }
}

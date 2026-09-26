//! iTerm2's sparkline graph geometry (milestone 9i, Task 2): the content rect, the path
//! points for a stroked polyline, the baseline, the slide animation offset, and a polyline
//! rasterized to pixel quads (the renderer draws only quads, `src/scene.rs:1008`). Pure
//! geometry: pixels in, pixels out. `src/app/metrics_ui.rs` (Task 3) turns each graph
//! kind's stored history into points here and hands the quads to `draw_widget`
//! (`src/app/statusbar_ui.rs:921`).
//!
//! Citations use the abbreviation from the milestone 9i brief's Resume notes: `SPK` =
//! `~/gitrepos/iterm2-ref/sources/StatusBar/Components/iTermStatusBarSparklinesComponent.m`.

use crate::layout::Rect;
use std::time::Duration;

/// The number of samples drawn (`SPK:369-371`); the stored history keeps
/// `statusbar_metrics::HISTORY` (120).
pub const SHOWN: usize = 60;
/// How long a new sample's slide animation takes (`SPK:158-251`).
pub const SLIDE: Duration = Duration::from_secs(1);
/// The animation's frame interval: 5 frames a second (`SPK:158-251`; macOS Low Power
/// Mode's 1 fps has no Windows analog, per the Resume notes' decisions).
pub const FRAME: Duration = Duration::from_millis(200);

/// The content rect a graph's path is drawn in (`SPK:447-543`): inset 4 from each edge
/// that has a text reserve, else 0; y 2.5; height `view_h - 5`.
pub fn graph_rect(view_w: f32, view_h: f32, left_reserve: f32, right_reserve: f32) -> Rect {
    let left_inset = if left_reserve > 0.0 { 4.0 + left_reserve } else { 0.0 };
    let right_inset = if right_reserve > 0.0 { 4.0 + right_reserve } else { 0.0 };
    Rect { x: left_inset, y: 2.5, w: view_w - left_inset - right_inset, h: view_h - 5.0 }
}

/// `SPK:196-200`: the drawn values drop the history's single oldest stored sample, then
/// keep the last `SHOWN`. With fewer than `SHOWN + 1` stored, fewer are returned (CPU's own
/// left-padding, `statusbar_metrics::cpu_graph`, is a separate rule for that component).
pub fn graph_values(history: &[f64]) -> Vec<f64> {
    if history.is_empty() {
        return Vec::new();
    }
    let dropped = &history[1..];
    let take = dropped.len().min(SHOWN);
    dropped[dropped.len() - take..].to_vec()
}

/// `bar_w = rect_w / (SHOWN - 1)` (`SPK:267-306`).
pub fn bar_width(rect_w: f32) -> f32 {
    rect_w / (SHOWN as f32 - 1.0)
}

/// The larger of `1.0` and the largest value across every given history (`SPK:291,332-343`:
/// `MAX(1, ceiling)`, the base ceiling being the largest stored value of any series).
pub fn ceiling(histories: &[&[f64]]) -> f64 {
    let max = histories.iter().flat_map(|h| h.iter()).fold(0.0_f64, |m, &v| m.max(v));
    max.max(1.0)
}

/// The path for one series (`SPK:267-306`): `n` values right-aligned in `rect`, y-down
/// (`y = rect.bottom - 2.5 - v * (rect.h - 4) / ceiling`), plus one closing point at
/// `(rect.max_x + 1, rect.bottom - 2.5)` so the line always reaches the rect's right edge.
/// `shift` (from `slide`) offsets every x by that many points, for the slide animation.
pub fn path(values: &[f64], rect: Rect, ceiling: f64, shift: f32) -> Vec<(f32, f32)> {
    if values.is_empty() {
        return Vec::new();
    }
    let bar_w = bar_width(rect.w);
    let bottom = rect.y + rect.h;
    let max_x = rect.x + rect.w;
    let n = values.len();
    let x0 = max_x - (n as f32 - 1.0) * bar_w;
    let mut points: Vec<(f32, f32)> = Vec::with_capacity(n + 1);
    for (i, &v) in values.iter().enumerate() {
        let x = x0 + i as f32 * bar_w + shift;
        let y = bottom - 2.5 - (v as f32) * (rect.h - 4.0) / (ceiling as f32);
        points.push((x, y));
    }
    points.push((max_x + 1.0 + shift, bottom - 2.5));
    points
}

/// The baseline band (`SPK:436-445`): the text color at alpha 0.5 (the app's job to apply),
/// the content rect's full width, 1 point tall, its top 3 points above the rect's bottom.
pub fn baseline(rect: Rect) -> Rect {
    let bottom = rect.y + rect.h;
    Rect { x: rect.x, y: bottom - 3.0, w: rect.w, h: 1.0 }
}

/// The slide animation's x offset (`SPK:158-251`): with animation off, or no sample yet,
/// the new path draws in place (0). Otherwise the path starts shifted right by a full
/// `bar_w` and eases back to 0 over `SLIDE`, so the net effect is the whole graph sliding
/// left by one bar per sample.
pub fn slide(since_sample: Option<Duration>, animate: bool, bar_w: f32) -> f32 {
    if !animate {
        return 0.0;
    }
    let Some(elapsed) = since_sample else {
        return 0.0;
    };
    let frac = (elapsed.as_secs_f32() / SLIDE.as_secs_f32()).clamp(0.0, 1.0);
    bar_w * (1.0 - frac)
}

/// Rasterizes a stroked polyline (the sparkline path, stroke width in points) into pixel
/// quads, clipped to `clip` (`SPK:267-306`'s `masksToBounds`; the renderer has no lines,
/// only quads, `src/scene.rs:1008`, so this stands in for `CAShapeLayer` stroking). One
/// quad per integer x column the polyline crosses; a column's quad spans every y row the
/// line passes through in it, widened by half the stroke width on each side.
pub fn polyline_rects(points: &[(f32, f32)], width: f32, clip: Rect) -> Vec<Rect> {
    let mut out = Vec::new();
    for pair in points.windows(2) {
        out.extend(segment_rects(pair[0], pair[1], width));
    }
    out.into_iter().filter_map(|r| clip_rect(r, clip)).collect()
}

fn segment_rects((x0, y0): (f32, f32), (x1, y1): (f32, f32), width: f32) -> Vec<Rect> {
    let half = width / 2.0;
    if x0 == x1 {
        let (ylo, yhi) = if y0 <= y1 { (y0, y1) } else { (y1, y0) };
        return vec![Rect { x: x0.floor(), y: ylo - half, w: 1.0, h: (yhi - ylo) + width }];
    }
    let (xa, ya, xb, yb) = if x0 <= x1 { (x0, y0, x1, y1) } else { (x1, y1, x0, y0) };
    let slope = (yb - ya) / (xb - xa);
    let start_col = xa.floor() as i64;
    let end_col = xb.floor() as i64;
    let mut out = Vec::new();
    for c in start_col..=end_col {
        let col = c as f32;
        let cx0 = col.max(xa);
        let cx1 = (col + 1.0).min(xb);
        if cx1 <= cx0 {
            continue;
        }
        let y_at = |x: f32| ya + slope * (x - xa);
        let (ya_c, yb_c) = (y_at(cx0), y_at(cx1));
        let (ylo, yhi) = if ya_c <= yb_c { (ya_c, yb_c) } else { (yb_c, ya_c) };
        out.push(Rect { x: col, y: ylo - half, w: 1.0, h: (yhi - ylo) + width });
    }
    out
}

fn clip_rect(r: Rect, clip: Rect) -> Option<Rect> {
    let x0 = r.x.max(clip.x);
    let y0 = r.y.max(clip.y);
    let x1 = (r.x + r.w).min(clip.x + clip.w);
    let y1 = (r.y + r.h).min(clip.y + clip.h);
    if x1 <= x0 || y1 <= y0 { None } else { Some(Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) {
        assert!((a - b).abs() < 1e-4, "{a} != {b}");
    }

    #[test]
    fn sparkline_geometry_follows_iterm2() {
        let rect = graph_rect(120.0, 21.0, 30.0, 0.0);
        approx(rect.x, 34.0);
        approx(rect.y, 2.5);
        approx(rect.w, 86.0);
        approx(rect.h, 16.0);

        approx(bar_width(59.0), 1.0);

        let content = Rect { x: 0.0, y: 0.0, w: 59.0, h: 14.0 };
        let base = baseline(content);
        approx(base.h, 1.0);
        approx(base.y, content.y + content.h - 3.0);
    }

    #[test]
    fn sparkline_path_follows_iterm2() {
        let rect = Rect { x: 0.0, y: 0.0, w: 59.0, h: 14.0 };
        let points = path(&[0.0, 1.0], rect, 1.0, 0.0);
        assert_eq!(points.len(), 3);
        approx(points[0].0, 58.0);
        approx(points[0].1, 11.5);
        approx(points[1].0, 59.0);
        approx(points[1].1, 1.5);
        approx(points[2].0, 60.0);
        approx(points[2].1, 11.5);

        let history: Vec<f64> = (0..70).map(|i| i as f64).collect();
        let drawn = graph_values(&history);
        assert_eq!(drawn.len(), 60);
        assert_eq!(drawn[0], 10.0);
        assert_eq!(*drawn.last().unwrap(), 69.0);

        assert_eq!(ceiling(&[&[0.0, 0.0, 0.0]]), 1.0);
        assert_eq!(ceiling(&[&[0.2, 3.0]]), 3.0);
    }

    #[test]
    fn sparkline_slides_one_bar_per_second() {
        assert_eq!(slide(Some(Duration::from_secs(0)), true, 2.0), 2.0);
        assert_eq!(slide(Some(Duration::from_millis(500)), true, 2.0), 1.0);
        assert_eq!(slide(Some(Duration::from_secs(1)), true, 2.0), 0.0);
        assert_eq!(slide(Some(Duration::from_secs(2)), true, 2.0), 0.0);
        assert_eq!(slide(Some(Duration::from_secs(0)), false, 2.0), 0.0);
        assert_eq!(slide(None, true, 2.0), 0.0);
    }

    #[test]
    fn polyline_rects_cover_the_line() {
        let clip = Rect { x: 0.0, y: 0.0, w: 100.0, h: 100.0 };
        let flat = polyline_rects(&[(0.0, 5.0), (10.0, 5.0)], 1.0, clip);
        assert_eq!(flat.len(), 10);
        for (i, r) in flat.iter().enumerate() {
            approx(r.x, i as f32);
            approx(r.w, 1.0);
            approx(r.y, 4.5);
            approx(r.h, 1.0);
        }

        // A steep segment covers every row it crosses in its column.
        let steep = polyline_rects(&[(0.0, 0.0), (1.0, 20.0)], 1.0, clip);
        assert!(!steep.is_empty());
        let total_h: f32 = steep.iter().map(|r| r.h).sum();
        assert!(total_h >= 20.0);
        for r in &steep {
            assert!(r.y >= -0.5 - 1e-4);
            assert!(r.y + r.h <= 20.5 + 1e-4);
        }

        // Nothing lands outside clip.
        let tight = Rect { x: 2.0, y: 4.0, w: 4.0, h: 4.0 };
        let clipped = polyline_rects(&[(0.0, 5.0), (10.0, 5.0)], 1.0, tight);
        for r in &clipped {
            assert!(r.x >= tight.x - 1e-4);
            assert!(r.x + r.w <= tight.x + tight.w + 1e-4);
            assert!(r.y >= tight.y - 1e-4);
            assert!(r.y + r.h <= tight.y + tight.h + 1e-4);
        }
        assert!(!clipped.is_empty());
    }
}

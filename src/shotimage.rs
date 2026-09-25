//! Make Screenshot's pixel work on RGBA images: blur or fill the redacted rectangles, draw
//! the highlight outline and glow, and pick the lines the minimap shows.

use crate::layout::Rect;

/// A rectangle's pixel bounds inside a `w` x `h` image: (x0, y0, x1, y1), exclusive ends.
fn bounds(r: &Rect, w: usize, h: usize) -> (usize, usize, usize, usize) {
    let x0 = (r.x.floor().max(0.0) as usize).min(w);
    let y0 = (r.y.floor().max(0.0) as usize).min(h);
    let x1 = ((r.x + r.w).ceil().max(0.0) as usize).min(w);
    let y1 = ((r.y + r.h).ceil().max(0.0) as usize).min(h);
    (x0, y0, x1, y1)
}

/// Blurs the image (a Gaussian, radius as its sigma, edges clamped) and copies the blurred
/// pixels into `rects` only.
pub fn blur_rects(rgba: &mut [u8], w: usize, h: usize, rects: &[Rect], radius: f32) {
    if radius <= 0.0 || w == 0 || h == 0 || rgba.len() < w * h * 4 {
        return;
    }
    let reach = ((3.0 * radius).ceil() as usize).clamp(1, 600);
    let kernel: Vec<f32> = {
        let k: Vec<f32> = (0..=2 * reach).map(|i| (-((i as f32 - reach as f32).powi(2)) / (2.0 * radius * radius)).exp()).collect();
        let sum: f32 = k.iter().sum();
        k.into_iter().map(|v| v / sum).collect()
    };
    let src = rgba.to_vec();
    for r in rects {
        let (x0, y0, x1, y1) = bounds(r, w, h);
        if x0 >= x1 || y0 >= y1 {
            continue;
        }
        // Rows the vertical pass reads, blurred across first (edges clamped to the image).
        let (ty0, ty1) = (y0.saturating_sub(reach), (y1 + reach).min(h));
        let cols = x1 - x0;
        let mut across = vec![[0f32; 4]; (ty1 - ty0) * cols];
        for y in ty0..ty1 {
            for x in x0..x1 {
                let mut acc = [0f32; 4];
                for (i, k) in kernel.iter().enumerate() {
                    let sx = (x + i).saturating_sub(reach).min(w - 1);
                    let p = (y * w + sx) * 4;
                    for c in 0..4 {
                        acc[c] += src[p + c] as f32 * k;
                    }
                }
                across[(y - ty0) * cols + (x - x0)] = acc;
            }
        }
        for y in y0..y1 {
            for x in x0..x1 {
                let mut acc = [0f32; 4];
                for (i, k) in kernel.iter().enumerate() {
                    let sy = (y + i).saturating_sub(reach).clamp(ty0, ty1 - 1);
                    let v = across[(sy - ty0) * cols + (x - x0)];
                    for c in 0..4 {
                        acc[c] += v[c] * k;
                    }
                }
                let p = (y * w + x) * 4;
                for c in 0..4 {
                    rgba[p + c] = acc[c].round().clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
}

/// Fills `rects` with `color` (blended by its alpha).
pub fn fill_rects(rgba: &mut [u8], w: usize, h: usize, rects: &[Rect], color: [u8; 4]) {
    let a = color[3] as f32 / 255.0;
    for r in rects {
        let (x0, y0, x1, y1) = bounds(r, w, h);
        for y in y0..y1 {
            for x in x0..x1 {
                let p = (y * w + x) * 4;
                for c in 0..3 {
                    rgba[p + c] = (color[c] as f32 * a + rgba[p + c] as f32 * (1.0 - a)).round() as u8;
                }
                rgba[p + 3] = 255;
            }
        }
    }
}

/// iTerm2's highlight around one group of rectangles: a 3 px yellow outline and a 20 px
/// glow (white on a dark background, black on a light one), never inside the group.
pub fn highlight(rgba: &mut [u8], w: usize, h: usize, group: &[Rect], dark_bg: bool) {
    const STROKE: f32 = 1.5;
    const GLOW: f32 = 20.0;
    const YELLOW: [f32; 3] = [255.0, 204.0, 0.0];
    let glow = if dark_bg { [255.0f32; 3] } else { [0.0f32; 3] };
    let Some(first) = group.first() else { return };
    let (mut bx0, mut by0, mut bx1, mut by1) = (first.x, first.y, first.x + first.w, first.y + first.h);
    for r in group {
        bx0 = bx0.min(r.x);
        by0 = by0.min(r.y);
        bx1 = bx1.max(r.x + r.w);
        by1 = by1.max(r.y + r.h);
    }
    let pad = STROKE + GLOW + 2.0;
    let area = Rect { x: bx0 - pad, y: by0 - pad, w: bx1 - bx0 + 2.0 * pad, h: by1 - by0 + 2.0 * pad };
    let (x0, y0, x1, y1) = bounds(&area, w, h);
    for y in y0..y1 {
        for x in x0..x1 {
            let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
            let d = group
                .iter()
                .map(|r| {
                    let dx = (r.x - cx).max(cx - (r.x + r.w)).max(0.0);
                    let dy = (r.y - cy).max(cy - (r.y + r.h)).max(0.0);
                    (dx * dx + dy * dy).sqrt()
                })
                .fold(f32::MAX, f32::min);
            // Nothing inside the group changes.
            if d <= 0.0 || d > STROKE + GLOW {
                continue;
            }
            let (color, a) = if d <= STROKE { (YELLOW, 1.0) } else { (glow, 0.8 * (-((d - STROKE).powi(2)) / (2.0 * 7.0 * 7.0)).exp()) };
            let p = (y * w + x) * 4;
            for c in 0..3 {
                rgba[p + c] = (color[c] * a + rgba[p + c] as f32 * (1.0 - a)).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

/// Which buffer line each minimap row shows: every line when they fit, else the middle
/// line of each bucket.
/// The minimap's line count: at most 296 (iTerm2), and few enough that the image fits a
/// texture of `max_px`.
pub fn minimap_rows(total: usize, line_px: usize, max_px: usize) -> usize {
    total.min(296).min(max_px / line_px.max(1)).max(1)
}

pub fn minimap_lines(total: usize, rows: usize) -> Vec<usize> {
    if total <= rows {
        return (0..total).collect();
    }
    (0..rows).map(|i| (i * total / rows + (i + 1) * total / rows) / 2).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(w: usize, h: usize, f: impl Fn(usize, usize) -> [u8; 4]) -> Vec<u8> {
        let mut v = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                v.extend_from_slice(&f(x, y));
            }
        }
        v
    }

    fn px(img: &[u8], w: usize, x: usize, y: usize) -> [u8; 4] {
        let i = (y * w + x) * 4;
        [img[i], img[i + 1], img[i + 2], img[i + 3]]
    }

    #[test]
    fn blur_stays_inside_rects() {
        let (w, h) = (40, 10);
        let src = image(w, h, |x, _| if x < 20 { [255, 0, 0, 255] } else { [0, 0, 255, 255] });
        let mut img = src.clone();
        blur_rects(&mut img, w, h, &[Rect { x: 15.0, y: 0.0, w: 10.0, h: 10.0 }], 3.0);
        let mid = px(&img, w, 19, 5);
        assert!(mid[0] > 40 && mid[2] > 40, "colors mix at the edge inside the rect: {mid:?}");
        assert_eq!(px(&img, w, 14, 5), px(&src, w, 14, 5), "outside the rect is untouched");
        assert_eq!(px(&img, w, 25, 5), px(&src, w, 25, 5));
        // A rect at the image's edge does not darken (edges are clamped, not black).
        let mut edge = src.clone();
        blur_rects(&mut edge, w, h, &[Rect { x: 0.0, y: 0.0, w: 5.0, h: 10.0 }], 3.0);
        assert_eq!(px(&edge, w, 0, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn solid_fill() {
        let (w, h) = (10, 10);
        let mut img = image(w, h, |_, _| [9, 9, 9, 255]);
        fill_rects(&mut img, w, h, &[Rect { x: 2.0, y: 3.0, w: 4.0, h: 2.0 }], [0, 0, 0, 255]);
        assert_eq!(px(&img, w, 2, 3), [0, 0, 0, 255]);
        assert_eq!(px(&img, w, 5, 4), [0, 0, 0, 255]);
        assert_eq!(px(&img, w, 6, 4), [9, 9, 9, 255]);
        assert_eq!(px(&img, w, 2, 5), [9, 9, 9, 255]);
    }

    #[test]
    fn highlight_leaves_the_inside() {
        let (w, h) = (100, 100);
        let src = image(w, h, |_, _| [20, 20, 20, 255]);
        let mut img = src.clone();
        let group = [Rect { x: 40.0, y: 40.0, w: 20.0, h: 10.0 }];
        highlight(&mut img, w, h, &group, true);
        assert_eq!(px(&img, w, 45, 45), px(&src, w, 45, 45), "inside is untouched");
        let edge = px(&img, w, 39, 45);
        assert!(edge[0] > 200 && edge[1] > 150 && edge[2] < 80, "a yellow outline: {edge:?}");
        let glow = px(&img, w, 30, 45);
        assert!(glow[0] > 20 && glow[2] > 20, "a white glow on a dark background: {glow:?}");
        assert_eq!(px(&img, w, 5, 5), px(&src, w, 5, 5), "far away is untouched");
        let mut light = image(w, h, |_, _| [240, 240, 240, 255]);
        highlight(&mut light, w, h, &group, false);
        assert!(px(&light, w, 30, 45)[2] < 240, "a dark glow on a light background");
    }

    #[test]
    fn minimap_samples_lines() {
        assert_eq!(minimap_lines(5, 10), vec![0, 1, 2, 3, 4]);
        assert_eq!(minimap_lines(100, 10), vec![5, 15, 25, 35, 45, 55, 65, 75, 85, 95]);
        assert_eq!(minimap_lines(0, 10), Vec::<usize>::new());
    }

    #[test]
    fn minimap_fits_the_texture_limit() {
        assert_eq!(minimap_rows(1000, 29, 8192), 282, "296 rows of 29 px would pass 8192");
        assert_eq!(minimap_rows(100, 20, 8192), 100);
        assert_eq!(minimap_rows(1000, 10, 8192), 296);
        assert_eq!(minimap_rows(1000, 20000, 8192), 1, "never 0");
    }
}

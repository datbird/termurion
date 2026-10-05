//! The room around a pane's text and the alternate screen's margin color (iTerm2's
//! `TerminalMargin`, `TerminalVMargin`, `PTYTextView.m:2965-3070`, `iTermTextDrawingHelper.m:619-647`).

use crate::palette::Rgb;

/// Side margin in points (`iTermPreferences.m:112`).
pub const DEFAULT_SIDE: f32 = 5.0;
/// Top and bottom margin in points (`iTermPreferences.m:113`).
pub const DEFAULT_TOP_BOTTOM: f32 = 2.0;

/// A margin setting in points as device pixels at `scale`; below zero counts as zero (iTerm2
/// does not bound it).
pub fn px(points: i64, scale: f32) -> f32 {
    points.max(0) as f32 * scale
}

/// The color at least `threshold` of the values share, by a majority vote then the check
/// (`PTYTextView.m:3014-3070`): `Some(color)` when one does (`Some(None)` for the default
/// background), None when none does or there are no values.
pub fn dominant(colors: &[Option<Rgb>], threshold: f64) -> Option<Option<Rgb>> {
    if colors.is_empty() {
        return None;
    }
    let (mut candidate, mut counter) = (None, 0usize);
    for c in colors {
        if counter == 0 {
            (candidate, counter) = (*c, 1);
        } else if candidate == *c {
            counter += 1;
        } else {
            counter -= 1;
        }
    }
    let freq = colors.iter().filter(|c| **c == candidate).count();
    (freq >= (colors.len() as f64 * threshold).ceil() as usize).then_some(candidate)
}

/// The margin colors of one pane: the strips above and below, and each row's left and right.
#[derive(Clone, Debug, PartialEq)]
pub struct Fill {
    pub default: Rgb,
    pub rows: Vec<(Rgb, Rgb)>,
}

/// The pane's margin colors. `edges` is each visible row's first and last cell background when
/// not default; only an alternate screen with `extend` takes colors from them: the strips
/// above and below take the color 80% of the edge cells share, and each row's sides take that
/// row's own edge color when it has one (`iTermTextDrawingHelper.m:619-647`).
pub fn fill(edges: &[(Option<Rgb>, Option<Rgb>)], pane_bg: Rgb, extend: bool, alt_screen: bool) -> Fill {
    if !(extend && alt_screen) {
        return Fill { default: pane_bg, rows: vec![(pane_bg, pane_bg); edges.len()] };
    }
    let values: Vec<Option<Rgb>> = edges.iter().flat_map(|(l, r)| [*l, *r]).collect();
    let default = dominant(&values, 0.8).flatten().unwrap_or(pane_bg);
    Fill { default, rows: edges.iter().map(|(l, r)| (l.unwrap_or(default), r.unwrap_or(default))).collect() }
}

/// The columns and rows `rect` (width, height) holds past the margins `(side, top/bottom)`:
/// today's floors of two columns and one row.
pub fn grid_size(rect: (f32, f32), cell: (f32, f32), margins: (f32, f32)) -> (u16, u16) {
    let (w, h) = ((rect.0 - 2.0 * margins.0).max(0.0), (rect.1 - 2.0 * margins.1).max(0.0));
    ((w / cell.0).floor().max(2.0) as u16, (h / cell.1).floor().max(1.0) as u16)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::rgb;

    const BLUE: Rgb = rgb(0, 0, 255);
    const RED: Rgb = rgb(255, 0, 0);
    const BLACK: Rgb = rgb(0, 0, 0);

    /// R3, `02:194`, `02:195`: 5 and 2 points, scaled, never below zero.
    #[test]
    fn margins_follow_iterm2() {
        assert_eq!(px(DEFAULT_SIDE as i64, 1.0), 5.0);
        assert_eq!(px(DEFAULT_SIDE as i64, 1.5), 7.5);
        assert_eq!(px(DEFAULT_TOP_BOTTOM as i64, 2.0), 4.0);
        assert_eq!(px(-3, 1.0), 0.0);
    }

    /// `02:1069`: the alternate screen's margin takes the 80% edge color; rows keep their own.
    #[test]
    fn margin_color_follows_iterm2() {
        let ten = |blue: usize| -> Vec<Option<Rgb>> { (0..10).map(|i| (i < blue).then_some(BLUE)).collect() };
        assert_eq!(dominant(&ten(9), 0.8), Some(Some(BLUE)));
        assert_eq!(dominant(&ten(8), 0.8), Some(Some(BLUE)), "exactly 80% counts");
        assert_eq!(dominant(&ten(7), 0.8), None);
        assert_eq!(dominant(&ten(0), 0.8), Some(None), "all default");
        assert_eq!(dominant(&[], 0.8), None);

        // Nine rows with a blue first cell and a default last cell: the left is every row's own
        // blue, the right the dominant color of the last cells (default, so the pane color).
        let mut edges: Vec<(Option<Rgb>, Option<Rgb>)> = (0..10).map(|_| (Some(BLUE), None)).collect();
        edges[3] = (Some(RED), None);
        let f = fill(&edges, BLACK, true, true);
        assert_eq!(f.rows[3], (RED, BLACK));
        assert_eq!(f.rows[0], (BLUE, BLACK));
        assert_eq!(f.rows.len(), 10);
        // The strips above and below take the dominant color of the first and last cells.
        let both: Vec<(Option<Rgb>, Option<Rgb>)> = (0..10).map(|_| (Some(BLUE), Some(BLUE))).collect();
        assert_eq!(fill(&both, BLACK, true, true).default, BLUE);
        // A row whose last cell is default takes the dominant color there.
        let mut edges: Vec<(Option<Rgb>, Option<Rgb>)> = (0..10).map(|_| (Some(BLUE), Some(BLUE))).collect();
        edges[1] = (Some(RED), None);
        assert_eq!(fill(&edges, BLACK, true, true).rows[1], (RED, BLUE));
        // Main screen, or the setting off: the pane color everywhere.
        for (extend, alt) in [(true, false), (false, true), (false, false)] {
            let f = fill(&both, BLACK, extend, alt);
            assert_eq!(f.default, BLACK);
            assert!(f.rows.iter().all(|r| *r == (BLACK, BLACK)), "extend {extend} alt {alt}");
        }
    }

    /// `PS:2778-2779`: the grid is what fits past the margins, with today's floors.
    #[test]
    fn grid_size_uses_the_margins() {
        let (side, vert) = (px(DEFAULT_SIDE as i64, 1.0), px(DEFAULT_TOP_BOTTOM as i64, 1.0));
        assert_eq!(grid_size((810.0, 400.0), (8.0, 16.0), (side, vert)), (100, 24));
        assert_eq!(grid_size((810.0, 400.0), (8.0, 16.0), (0.0, 0.0)), (101, 25));
        assert_eq!(grid_size((10.0, 10.0), (8.0, 16.0), (side, vert)), (2, 1));
    }
}

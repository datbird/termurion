//! The room around a pane's text and the alternate screen's margin color (iTerm2's
//! `TerminalMargin`, `TerminalVMargin`, `PTYTextView.m:2965-3070`, `iTermTextDrawingHelper.m:619-647`).

use crate::layout::Rect;
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
/// row's own edge color when it has one, else the pane's default background (`iTermTextDrawingHelper.m:619-647`).
pub fn fill(edges: &[(Option<Rgb>, Option<Rgb>)], pane_bg: Rgb, extend: bool, alt_screen: bool) -> Fill {
    if !(extend && alt_screen) {
        return Fill { default: pane_bg, rows: vec![(pane_bg, pane_bg); edges.len()] };
    }
    let values: Vec<Option<Rgb>> = edges.iter().flat_map(|(l, r)| [*l, *r]).collect();
    let default = dominant(&values, 0.8).flatten().unwrap_or(pane_bg);
    Fill { default, rows: edges.iter().map(|(l, r)| (l.unwrap_or(pane_bg), r.unwrap_or(pane_bg))).collect() }
}

/// The columns and rows `rect` (width, height) holds past the margins `(side, top/bottom)`:
/// today's floors of two columns and one row.
pub fn grid_size(rect: (f32, f32), cell: (f32, f32), margins: (f32, f32)) -> (u16, u16) {
    let (w, h) = ((rect.0 - 2.0 * margins.0).max(0.0), (rect.1 - 2.0 * margins.1).max(0.0));
    ((w / cell.0).floor().max(2.0) as u16, (h / cell.1).floor().max(1.0) as u16)
}

/// The pane's rectangle less `room` at the right edge: what the scrollbar strip and the
/// Adjacent timestamp column take (iTerm2 lays them out beside the text view, then the margins
/// sit inside what is left). The margin painter and the grid both start from this.
pub fn text_outer(rect: Rect, room: f32) -> Rect {
    Rect { w: (rect.w - room).max(0.0), ..rect }
}

/// The text area: `text_outer` inset by the margins. The grid, the pointer, every overlay that
/// sits on the text and `grid_size` agree on it.
pub fn text_rect(rect: Rect, margins: (f32, f32), room: f32) -> Rect {
    text_outer(rect, room).inset_xy(margins.0, margins.1)
}

/// The columns and rows a pane of `rect` (width, height) holds once it gives up `room` pixels
/// at its right edge: `grid_size` of what is left, the one path every sizing site takes.
pub fn pane_grid(rect: (f32, f32), cell: (f32, f32), margins: (f32, f32), room: f32) -> (u16, u16) {
    grid_size((rect.0 - room, rect.1), cell, margins)
}

/// The columns a tmux tab reports: its area less the side margins and the room the panes of
/// its widest row give up (`row_room`, the sum of that row's panes), floored at two.
pub fn tmux_cols(area_w: f32, cw: f32, side: f32, row_room: f32) -> u16 {
    grid_size((area_w - row_room, 0.0), (cw, 1.0), (side, 0.0)).0
}

/// What a pane gives up at its right edge: the legacy scrollbar (unless hidden) and the
/// Adjacent timestamp column (`stamps` is the pane's mode while its timestamps show).
pub fn pane_room(style: crate::scrollbar::Style, hidden: bool, stamps: Option<crate::stamps::Mode>, scale: f32) -> f32 {
    crate::scrollbar::room(style, hidden, scale) + stamps.map_or(0.0, |m| crate::stamps::room(m, scale))
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
        // A row whose last cell is default takes the pane's default background there.
        let mut edges: Vec<(Option<Rgb>, Option<Rgb>)> = (0..10).map(|_| (Some(BLUE), Some(BLUE))).collect();
        edges[1] = (Some(RED), None);
        assert_eq!(fill(&edges, BLACK, true, true).rows[1], (RED, BLACK), "a default edge cell is the pane's default background (iTerm2: regularVerticalColor), not the dominant color");
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

    /// 13m Task 7 (Review Focus 3): the legacy scrollbar takes its 15 points from the grid, the
    /// overlay style and `HideScrollbar` take nothing, and a tmux tab reports the narrower width.
    #[test]
    fn panes_make_room_for_the_scrollbar() {
        use crate::scrollbar::{room, Style};
        let m = (px(DEFAULT_SIDE as i64, 1.0), px(DEFAULT_TOP_BOTTOM as i64, 1.0));
        let cell = (8.0, 16.0);
        let legacy = pane_room(Style::Legacy, false, None, 1.0);
        assert_eq!(legacy, room(Style::Legacy, false, 1.0));
        assert_eq!(pane_grid((1000.0, 400.0), cell, m, legacy).0, grid_size((1000.0 - 15.0, 400.0), cell, m).0);
        assert_eq!(pane_grid((1000.0, 400.0), cell, m, legacy).0, 121, "(1000 - 10 - 15) / 8");
        for none in [pane_room(Style::Legacy, true, None, 1.0), pane_room(Style::Overlay, false, None, 1.0)] {
            assert_eq!(none, 0.0);
            assert_eq!(pane_grid((1000.0, 400.0), cell, m, none), grid_size((1000.0, 400.0), cell, m));
            assert_eq!(pane_grid((1000.0, 400.0), cell, m, none).0, 123, "(1000 - 10) / 8");
        }
        assert_eq!(tmux_cols(1000.0, 8.0, m.0, legacy), 121, "tmux is told the narrower width");
        assert_eq!(tmux_cols(1000.0, 8.0, m.0, 0.0), 123);
        // Two panes side by side each give up the strip: the widest row's rooms add.
        assert_eq!(tmux_cols(1000.0, 8.0, m.0, 2.0 * legacy), 120, "(1000 - 10 - 30) / 8");
        assert_eq!(tmux_cols(5.0, 8.0, 5.0, 100.0), 2, "never below two columns");
        // The rect the text sits in leaves the room at the right and the margins inside it.
        let pane = Rect { x: 10.0, y: 20.0, w: 1000.0, h: 400.0 };
        let t = text_rect(pane, m, legacy);
        assert_eq!((t.x, t.y, t.w, t.h), (15.0, 22.0, 1000.0 - 10.0 - 15.0, 396.0));
        assert_eq!(text_outer(pane, legacy).w, 985.0);
    }

    /// 13m Task 9 (Review Focus 6): an Adjacent column narrows its pane's grid by 100 points
    /// times the scale (with the scrollbar's room), Over and Hover keep the width, and a
    /// neighbor's grid is its own.
    #[test]
    fn adjacent_timestamps_make_room() {
        use crate::scrollbar::{room, Style};
        use crate::stamps::{room as stamp_room, Mode};
        let scale = 1.5;
        let m = (px(DEFAULT_SIDE as i64, scale), px(DEFAULT_TOP_BOTTOM as i64, scale));
        let cell = (12.0, 24.0);
        let full = pane_room(Style::Overlay, false, None, scale);
        let adjacent = pane_room(Style::Overlay, false, Some(Mode::Adjacent), scale);
        assert_eq!(adjacent, 150.0);
        assert_eq!(pane_room(Style::Overlay, false, Some(Mode::Over), scale), 0.0);
        assert_eq!(pane_room(Style::Overlay, false, Some(Mode::Hover), scale), 0.0);
        assert_eq!(pane_grid((900.0, 600.0), cell, m, adjacent), grid_size((900.0 - 150.0, 600.0), cell, m));
        assert!(pane_grid((900.0, 600.0), cell, m, adjacent).0 < pane_grid((900.0, 600.0), cell, m, full).0);
        // With a legacy scrollbar too: the order on screen is grid, column, scrollbar.
        let both = pane_room(Style::Legacy, false, Some(Mode::Adjacent), scale);
        assert_eq!(both, room(Style::Legacy, false, scale) + stamp_room(Mode::Adjacent, scale));
        assert_eq!(pane_grid((900.0, 600.0), cell, m, both), grid_size((900.0 - both, 600.0), cell, m));
        // One pane's column leaves its neighbor's grid alone, and tmux reports the widest row.
        assert_eq!(pane_grid((450.0, 600.0), cell, m, full), grid_size((450.0, 600.0), cell, m));
        assert_eq!(tmux_cols(900.0, 12.0, m.0, adjacent + full), tmux_cols(900.0, 12.0, m.0, adjacent));
        assert!(tmux_cols(900.0, 12.0, m.0, adjacent) < tmux_cols(900.0, 12.0, m.0, 0.0));
    }
}

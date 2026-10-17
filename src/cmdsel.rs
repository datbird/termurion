//! Command selection (13s Task 9): clicking a command selects it, restricts Find, Filter and
//! Select All to it, shades everything else and outlines it (`PTYSession.m:21613-21648`,
//! `iTermTextDrawingHelper.m:986-1060`). Pure rules; the app side lives in `app/selection_ui.rs`.

use crate::layout::Rect;
use crate::palette::Rgb;

/// The outline's room outside the region, in points (`commandRegionOutlineThickness`).
pub const OUTLINE: f32 = 2.0;

/// The first-selection notice's text (`PTYSession.CommandSelectionInfo`); the link follows it.
pub const INFO: &str = "You\u{2019}ve selected a command by clicking on it. This restricts Find, Filter, and Select All to the content of the command. You can turn this feature off in Settings > General > Selection.";

/// The notice's link, which turns `ClickToSelectCommand` off (iTerm2's `iterm2:disable-command-selection`).
pub const DISABLE_LINK: &str = "Click here to disable this feature.";

/// What the find bar says while a command restricts it (`MinimalFindView.xib:106`).
pub const NOTICE: &str = "Searching only selected command";

/// The command a click selects (`textViewSelectCommandRegionAtCoord:`): the clicked command's
/// mark, unless it is the selected one already or the key is off (both deselect).
pub fn toggle(clicked: Option<u32>, selected: Option<u32>, allowed: bool) -> Option<u32> {
    if !allowed || clicked == selected {
        return None;
    }
    clicked
}

/// A selection that would take in the mutable screen of the alternate screen is refused
/// (`selectCommandWithMarkIfSafe:`): `region` and `screen_rows` are inclusive buffer rows.
pub fn refused(region: (usize, usize), alt_screen: bool, screen_rows: (usize, usize)) -> bool {
    alt_screen && region.0 <= screen_rows.1 && screen_rows.0 <= region.1
}

/// The grid lines Select All takes (`selectAll:`, `PTYTextView.m:3793-3814`): the search range
/// when a command is selected (buffer rows, inclusive), else the whole buffer.
pub fn select_all_lines(region: Option<(usize, usize)>, history: usize, screen_lines: usize) -> (i32, i32) {
    match region {
        Some((first, last)) => (first as i32 - history as i32, last as i32 - history as i32),
        None => (-(history as i32), screen_lines as i32 - 1),
    }
}

/// Whether a background reads as dark (`NSColor isDark`: perceived brightness under one half).
fn is_dark(bg: Rgb) -> bool {
    0.299 * f32::from(bg.r) + 0.587 * f32::from(bg.g) + 0.114 * f32::from(bg.b) < 128.0
}

/// The shade over everything outside the command: white on a dark background, black on a light
/// one, at `alpha` (`shadeColor`).
pub fn shade_color(default_bg: Rgb, alpha: f32) -> [f32; 4] {
    let c = if is_dark(default_bg) { crate::palette::rgb(255, 255, 255) } else { crate::palette::rgb(0, 0, 0) };
    crate::scene::rgba(c, alpha)
}

/// The outline's two frames, outer first: the selection color, then it blended 25% toward the
/// background (`selectedCommandOutlineColors`).
pub fn outline_colors(selection: Rgb, bg: Rgb) -> [Rgb; 2] {
    [selection, crate::scene::blend(selection, bg, 0.25)]
}

/// The visible part of `region` as (first, last) view rows, None when none of it shows.
fn visible_part(visible_top: usize, rows: usize, region: (usize, usize)) -> Option<(usize, usize)> {
    let (first, last) = (region.0.max(visible_top), region.1.min(visible_top + rows.checked_sub(1)?));
    (first <= last).then(|| (first - visible_top, last - visible_top))
}

/// The shade's rectangles over the pane's `rows` view rows (the first is buffer row
/// `visible_top`): above and below the region with the outline's room each side
/// (`drawShadeOverNonSelectedCommands:`); the whole pane when the region is off screen.
pub fn shade_rects(pane: Rect, cell_h: f32, visible_top: usize, rows: usize, region: (usize, usize), scale: f32) -> Vec<Rect> {
    let room = OUTLINE * scale;
    let Some((first, last)) = visible_part(visible_top, rows, region) else {
        return vec![pane];
    };
    let top_h = (first as f32 * cell_h - room).max(0.0);
    let selected_h = ((last - first + 1) as f32 * cell_h + room * 2.0).max(0.0);
    let bottom_y = top_h + selected_h;
    let total = rows as f32 * cell_h;
    let mut out = Vec::new();
    if top_h > 0.0 {
        out.push(Rect { x: pane.x, y: pane.y, w: pane.w, h: top_h });
    }
    if total - bottom_y > 0.0 {
        out.push(Rect { x: pane.x, y: pane.y + bottom_y, w: pane.w, h: total - bottom_y });
    }
    out
}

/// The outline's frames, outer first, each as the four edge rectangles of a one-point frame:
/// the region's rows widened by the room, the second frame one point inside the first
/// (`drawOutlineAroundSelectedCommand:`). The rows are the region's own, clipped to the pane
/// only by the caller's drawing; empty when none of the region shows.
pub fn outline_edges(pane: Rect, cell_h: f32, visible_top: usize, rows: usize, region: (usize, usize), scale: f32) -> Vec<(usize, Rect)> {
    let Some((first, last)) = visible_part(visible_top, rows, region) else { return Vec::new() };
    let room = OUTLINE * scale;
    // Rows above the screen or below it are not drawn: the frame closes at the pane's edge.
    let mut frame = Rect { x: pane.x, y: pane.y + first as f32 * cell_h - room, w: pane.w, h: (last - first + 1) as f32 * cell_h + room * 2.0 };
    let one = scale.max(1.0);
    let mut out = Vec::new();
    for layer in 0..2 {
        let f = frame;
        let inner_h = (f.h - 2.0 * one).max(0.0);
        out.push((layer, Rect { h: one, ..f }));
        out.push((layer, Rect { y: f.y + f.h - one, h: one, ..f }));
        out.push((layer, Rect { y: f.y + one, w: one, h: inner_h, ..f }));
        out.push((layer, Rect { x: f.x + f.w - one, y: f.y + one, w: one, h: inner_h }));
        frame = Rect { x: f.x + one, y: f.y + one, w: (f.w - 2.0 * one).max(0.0), h: (f.h - 2.0 * one).max(0.0) };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::rgb;

    /// `PTYSession.m:21613-21648`.
    #[test]
    fn command_click_follows_iterm2() {
        assert_eq!(toggle(Some(7), None, true), Some(7));
        assert_eq!(toggle(Some(7), Some(7), true), None);
        assert_eq!(toggle(Some(7), None, false), None);
        assert_eq!(toggle(None, Some(7), true), None);
        assert_eq!(toggle(Some(8), Some(7), true), Some(8), "another command replaces the selected one");
        assert!(refused((10, 30), true, (20, 43)));
        assert!(!refused((10, 15), true, (20, 43)));
        assert!(!refused((10, 30), false, (20, 43)));
        assert!(!refused((10, 15), false, (20, 43)));
    }

    #[test]
    fn select_all_takes_the_region() {
        assert_eq!(select_all_lines(Some((3, 6)), 0, 12), (3, 6));
        assert_eq!(select_all_lines(Some((13, 16)), 10, 12), (3, 6), "buffer rows to grid lines");
        assert_eq!(select_all_lines(None, 10, 12), (-10, 11));
    }

    /// `iTermTextDrawingHelper.m:986-1060`.
    #[test]
    fn shade_follows_iterm2() {
        assert_eq!(shade_color(rgb(0, 0, 0), 0.25), crate::scene::rgba(rgb(255, 255, 255), 0.25));
        assert_eq!(shade_color(rgb(255, 255, 255), 0.25), crate::scene::rgba(rgb(0, 0, 0), 0.25));
        let sel = rgb(40, 80, 200);
        let bg = rgb(0, 0, 0);
        assert_eq!(outline_colors(sel, bg), [sel, rgb(30, 60, 150)]);
        let pane = Rect { x: 0.0, y: 0.0, w: 100.0, h: 200.0 };
        let r = shade_rects(pane, 20.0, 0, 10, (3, 5), 1.0);
        assert_eq!(r, vec![Rect { x: 0.0, y: 0.0, w: 100.0, h: 58.0 }, Rect { x: 0.0, y: 122.0, w: 100.0, h: 78.0 }]);
        // Scrolled: buffer row 13 is view row 3.
        assert_eq!(shade_rects(pane, 20.0, 10, 10, (13, 15), 1.0), r);
        // The region touching the top or the bottom leaves no rectangle there.
        assert_eq!(shade_rects(pane, 20.0, 0, 10, (0, 9), 1.0), vec![]);
        // Off screen: everything is shaded.
        assert_eq!(shade_rects(pane, 20.0, 0, 10, (30, 40), 1.0), vec![pane]);
        // The outline is two one-point frames, the second inside the first.
        let edges = outline_edges(pane, 20.0, 0, 10, (3, 5), 1.0);
        assert_eq!(edges.len(), 8);
        assert_eq!(edges[0], (0, Rect { x: 0.0, y: 58.0, w: 100.0, h: 1.0 }));
        assert_eq!(edges[4], (1, Rect { x: 1.0, y: 59.0, w: 98.0, h: 1.0 }));
        assert!(outline_edges(pane, 20.0, 0, 10, (30, 40), 1.0).is_empty());
    }
}

//! Where the tab bar goes (milestone 13d): the edge it sits on, how much room it takes, a
//! side bar's width and its drag handle, and whether the bar shows at all. Pure code; the
//! app reads the settings and draws.
//!
//! Citations: `P` = `sources/Settings/iTermPreferences.m`, `RTV` =
//! `sources/TerminalView/iTermRootTerminalView.m`, `PSMH` =
//! `ThirdParty/PSMTabBarControl/source/PSMTabBarControl.h` under `~/gitrepos/iterm2-ref`.

use crate::layout::Rect;

/// The tab bar's edge: `TabViewType` (`P:110`), iTerm2's `PSMTabPosition` (`PSMH:202-207`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabPosition {
    Top,
    Bottom,
    Left,
    Right,
}

/// The stored `TabViewType`: 0 Top (the default, `P:812`), 1 Bottom, 2 Left, 3 Right,
/// anything else Top.
pub fn from_value(v: i64) -> TabPosition {
    match v {
        1 => TabPosition::Bottom,
        2 => TabPosition::Left,
        3 => TabPosition::Right,
        _ => TabPosition::Top,
    }
}

/// A side bar: its tabs stack down the window's left or right edge.
pub fn vertical(p: TabPosition) -> bool {
    matches!(p, TabPosition::Left | TabPosition::Right)
}

/// `LeftTabBarWidth`'s default in points (`P:916`): a side bar's preferred width.
pub const DEFAULT_SIDE_WIDTH: f32 = 150.0;

/// A side bar's narrowest width in points for the Light, Dark, Automatic and High Contrast
/// styles (`RTV minimumTabBarWidth`, `RTV:2030-2044`).
pub const MIN_SIDE_WIDTH: f32 = 50.0;
/// A side bar's narrowest width in points for the Minimal and Compact styles.
pub const MIN_SIDE_WIDTH_MINIMAL: f32 = 114.0;

/// A side bar's width: the preferred width (`LeftTabBarWidth`) no wider than the content
/// less both side margins less 10 (and at least 1), and no narrower than `minimum`, which
/// wins over a maximum below it (`RTV leftTabBarWidthForPreferredWidth:contentWidth:`,
/// `RTV:2046-2050`).
pub fn side_width(preferred: f32, content_w: f32, side_margin: f32, minimum: f32) -> f32 {
    let maximum = (content_w - side_margin * 2.0 - 10.0).max(1.0);
    preferred.min(maximum).max(minimum)
}

/// Splits the window's room into the bar's rect (`thickness` high on the top or bottom
/// edge, wide on the left or right) and the rest.
pub fn split(room: Rect, p: TabPosition, thickness: f32) -> (Rect, Rect) {
    let Rect { x, y, w, h } = room;
    let t = thickness.max(0.0);
    match p {
        TabPosition::Top => (Rect { x, y, w, h: t }, Rect { x, y: y + t, w, h: (h - t).max(0.0) }),
        TabPosition::Bottom => (Rect { x, y: y + h - t, w, h: t }, Rect { x, y, w, h: (h - t).max(0.0) }),
        TabPosition::Left => (Rect { x, y, w: t, h }, Rect { x: x + t, y, w: (w - t).max(0.0), h }),
        TabPosition::Right => (Rect { x: x + w - t, y, w: t, h }, Rect { x, y, w: (w - t).max(0.0), h }),
    }
}

/// A window's room (above any banner) shared out as iTerm2 does (`iTermLayoutCalculator.m:305-330`,
/// `:412-436`): the toolbelt, `belt` wide, takes the far right at the room's full height first;
/// the tab bar, when it shows, takes its `(edge, thickness)` from what is left; the sessions get
/// the rest. Answers the bar's rect, the sessions' rect and the toolbelt's rect.
pub fn window_split(room: Rect, belt: Option<f32>, bar: Option<(TabPosition, f32)>) -> (Option<Rect>, Rect, Option<Rect>) {
    let (rest, belt) = crate::toolbelt_layout::split_window(room, belt.is_some(), belt.unwrap_or(0.0));
    match bar {
        Some((p, thickness)) => {
            let (bar, sessions) = split(rest, p, thickness);
            (Some(bar), sessions, belt)
        }
        None => (None, rest, belt),
    }
}

/// The tab bar shows (`RTV:1534-1556`): a flashing bar always; in full screen only with
/// Show tab bar in fullscreen (`ShowFullScreenTabBar`); then always when configured to be
/// always visible; else with more than one tab.
pub fn bar_visible(full_screen: bool, show_in_full_screen: bool, flashing: bool, always: bool, tabs: usize) -> bool {
    if flashing {
        return true;
    }
    if full_screen && !show_in_full_screen {
        return false;
    }
    always || tabs > 1
}

/// The bar is always visible with `HideTab` off (`PT tabBarAlwaysVisible`; the check box
/// "Show tab bar even when there is only one tab" shows the key inverted).
pub fn always_visible(hide_tab: bool) -> bool {
    !hide_tab
}

/// The width in points of a side bar's drag handle (`RTV:1866`, `:1886`).
pub const DRAG_HANDLE: f32 = 3.0;

/// A side bar's drag handle: a `DRAG_HANDLE`-wide strip on the bar's inner edge, the right
/// edge of a left bar and the left edge of a right bar (`RTV:1860-1895`). A top or bottom
/// bar has none.
pub fn drag_handle(bar: Rect, p: TabPosition) -> Option<Rect> {
    match p {
        TabPosition::Left => Some(Rect { x: bar.x + bar.w - DRAG_HANDLE, y: bar.y, w: DRAG_HANDLE, h: bar.h }),
        TabPosition::Right => Some(Rect { x: bar.x, y: bar.y, w: DRAG_HANDLE, h: bar.h }),
        TabPosition::Top | TabPosition::Bottom => None,
    }
}

/// How far a side bar of `rows` rows `row_h` high scrolls in a bar `bar_h` high: until the
/// last row's bottom meets the bar's bottom, 0 when every row fits (`PSM maximumScrollOffset`).
pub fn max_scroll(rows: usize, row_h: f32, bar_h: f32) -> f32 {
    (rows as f32 * row_h - bar_h).max(0.0)
}

/// The scroll that shows row `active` whole: unchanged when it already shows, else the least
/// move that brings its top or its bottom to the bar's edge (`PSM scrollSelectedTabIntoView`).
pub fn scroll_to_show(active: usize, row_h: f32, bar_h: f32, scroll: f32) -> f32 {
    let top = active as f32 * row_h;
    if top < scroll {
        top
    } else if top + row_h > scroll + bar_h {
        top + row_h - bar_h
    } else {
        scroll
    }
}

/// A side bar's row height: `row_h` on a bar that scrolls (`ScrollableSideTabBar`), else the
/// rows shrink so all of them fit, `min(row_h, bar_h / rows)`.
pub fn fit_rows(rows: usize, row_h: f32, bar_h: f32, scrollable: bool) -> f32 {
    if scrollable || rows == 0 {
        return row_h;
    }
    row_h.min(bar_h / rows as f32)
}

/// Each tab's rect in pixels, window coordinates, from a horizontal bar's cells (tab index and
/// rect in points from the bar's left): scrolled left by `scroll` points, scaled, and moved to
/// the bar's `origin` in pixels. A tab without a cell (in the overflow menu, or collapsed)
/// gets a zero-size rect, so the result has `tabs` rects in tab order.
pub fn rects_from_cells(cells: &[(usize, Rect)], tabs: usize, origin: (f32, f32), scroll: f32, scale: f32) -> Vec<Rect> {
    let mut out = vec![Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 }; tabs];
    for &(i, r) in cells {
        if let Some(slot) = out.get_mut(i) {
            let px = crate::tab_layout::to_px(Rect { x: r.x - scroll, ..r }, scale);
            *slot = Rect { x: px.x + origin.0, y: px.y + origin.1, ..px };
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use TabPosition::*;

    fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn tab_position_follows_iterm2() {
        assert_eq!([0, 1, 2, 3].map(from_value), [Top, Bottom, Left, Right]);
        assert_eq!(from_value(9), Top);
        assert!(vertical(Left) && vertical(Right));
        assert!(!vertical(Top) && !vertical(Bottom));
        let room = r(0.0, 0.0, 800.0, 600.0);
        assert_eq!(split(room, Top, 26.0), (r(0.0, 0.0, 800.0, 26.0), r(0.0, 26.0, 800.0, 574.0)));
        assert_eq!(split(room, Bottom, 26.0), (r(0.0, 574.0, 800.0, 26.0), r(0.0, 0.0, 800.0, 574.0)));
        let (left, rest) = split(room, Left, 150.0);
        assert_eq!((left, rest), (r(0.0, 0.0, 150.0, 600.0), r(150.0, 0.0, 650.0, 600.0)));
        let (right, rest) = split(room, Right, 150.0);
        assert_eq!((right, rest), (r(650.0, 0.0, 150.0, 600.0), r(0.0, 0.0, 650.0, 600.0)));
        assert_eq!(drag_handle(left, Left), Some(r(147.0, 0.0, 3.0, 600.0)));
        assert_eq!(drag_handle(right, Right), Some(r(650.0, 0.0, 3.0, 600.0)));
        assert_eq!(drag_handle(r(0.0, 0.0, 800.0, 26.0), Top), None);
    }

    #[test]
    fn hide_tab_reads_the_key() {
        assert!(!always_visible(true));
        assert!(always_visible(false));
        // HideTab off shows one tab's bar.
        assert!(bar_visible(false, true, false, always_visible(false), 1));
    }

    #[test]
    fn bar_visible_follows_iterm2() {
        assert!(!bar_visible(false, true, false, false, 1));
        assert!(bar_visible(false, true, false, false, 2));
        // Always visible.
        assert!(bar_visible(false, true, false, true, 1));
        // Full screen with Show Tabs in Fullscreen off.
        assert!(!bar_visible(true, false, false, false, 3));
        // A flashing bar shows even then.
        assert!(bar_visible(true, false, true, false, 1));
        assert!(bar_visible(true, true, false, false, 2));
    }

    #[test]
    fn side_width_follows_iterm2() {
        assert_eq!(side_width(150.0, 800.0, 5.0, 50.0), 150.0);
        assert_eq!(side_width(20.0, 800.0, 5.0, 50.0), 50.0);
        assert_eq!(side_width(20.0, 800.0, 5.0, 114.0), 114.0);
        // The content 800 less 2 x 5 less 10.
        assert_eq!(side_width(900.0, 800.0, 5.0, 50.0), 780.0);
        // The minimum wins over a maximum below it (`MAX(MIN(max, pref), min)`).
        assert_eq!(side_width(150.0, 30.0, 5.0, 50.0), 50.0);
        assert_eq!(side_width(150.0, 0.0, 5.0, 50.0), 50.0);
    }

    #[test]
    fn toolbelt_takes_the_far_right_first() {
        let room = r(0.0, 0.0, 800.0, 600.0);
        let belt = Some(r(600.0, 0.0, 200.0, 600.0));
        // The toolbelt is full height beside every edge's bar; the bar lays out in the rest.
        assert_eq!(window_split(room, Some(200.0), Some((Top, 26.0))), (Some(r(0.0, 0.0, 600.0, 26.0)), r(0.0, 26.0, 600.0, 574.0), belt));
        assert_eq!(window_split(room, Some(200.0), Some((Bottom, 26.0))), (Some(r(0.0, 574.0, 600.0, 26.0)), r(0.0, 0.0, 600.0, 574.0), belt));
        assert_eq!(window_split(room, Some(200.0), Some((Left, 150.0))), (Some(r(0.0, 0.0, 150.0, 600.0)), r(150.0, 0.0, 450.0, 600.0), belt));
        // A right bar sits left of the toolbelt (`tabBarMaxX = contentViewWidth - toolbeltWidth`).
        assert_eq!(window_split(room, Some(200.0), Some((Right, 150.0))), (Some(r(450.0, 0.0, 150.0, 600.0)), r(0.0, 0.0, 450.0, 600.0), belt));
        // No toolbelt, no bar.
        assert_eq!(window_split(room, None, Some((Right, 150.0))), (Some(r(650.0, 0.0, 150.0, 600.0)), r(0.0, 0.0, 650.0, 600.0), None));
        assert_eq!(window_split(room, Some(200.0), None), (None, r(0.0, 0.0, 600.0, 600.0), belt));
    }

    #[test]
    fn side_scroll_follows_iterm2() {
        assert_eq!(max_scroll(10, 24.0, 100.0), 140.0);
        assert_eq!(max_scroll(3, 24.0, 100.0), 0.0);
        // The last row's bottom at the bar's bottom.
        assert_eq!(scroll_to_show(9, 24.0, 100.0, 0.0), 140.0);
        assert_eq!(scroll_to_show(0, 24.0, 100.0, 140.0), 0.0);
        // Already shown.
        assert_eq!(scroll_to_show(4, 24.0, 100.0, 50.0), 50.0);
        assert_eq!(fit_rows(10, 24.0, 100.0, true), 24.0);
        assert_eq!(fit_rows(10, 24.0, 100.0, false), 10.0);
        // Rows that fit keep their height; no rows divide nothing.
        assert_eq!(fit_rows(3, 24.0, 100.0, false), 24.0);
        assert_eq!(fit_rows(0, 24.0, 100.0, false), 24.0);
    }

    #[test]
    fn bar_tab_rects_follow_the_bar() {
        let cells = [(0, r(0.0, 0.0, 100.0, 24.0)), (2, r(100.0, 0.0, 100.0, 24.0))];
        assert_eq!(
            rects_from_cells(&cells, 3, (0.0, 30.0), 20.0, 2.0),
            vec![r(-40.0, 30.0, 200.0, 48.0), r(0.0, 0.0, 0.0, 0.0), r(160.0, 30.0, 200.0, 48.0)]
        );
        // A cell for a tab that is gone is dropped.
        assert_eq!(rects_from_cells(&cells, 1, (0.0, 0.0), 0.0, 1.0), vec![r(0.0, 0.0, 100.0, 24.0)]);
    }
}

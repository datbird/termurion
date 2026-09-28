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
}

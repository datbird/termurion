//! Dragging a tab without the app (milestone 13d): when a drag begins, where in a bar a tab
//! drops, the pinned zone a reorder stays in, and where a drop lands (its own bar, another
//! window's bar, a new window or nowhere). Pure code; `app::tab_drag_ui` applies it.
//!
//! Citations: `PT` = `sources/TerminalView/PseudoTerminal.m`, `PSM` =
//! `ThirdParty/PSMTabBarControl/source/PSMTabBarControl.m`, `AS` =
//! `sources/Settings/iTermAdvancedSettingsModel.m` under `~/gitrepos/iterm2-ref`.

use crate::layout::Rect;
use crate::tab_layout::Release;

/// A press moved far enough to start a drag: the Euclidean distance from `start` to `now`
/// is at least `min_distance` (`MinimumTabDragDistance`, `AS:316`; `PSM:3775`, `:3814`).
pub fn began(start: (f32, f32), now: (f32, f32), min_distance: f32) -> bool {
    let (dx, dy) = (now.0 - start.0, now.1 - start.1);
    (dx * dx + dy * dy).sqrt() >= min_distance
}

/// The slot a tab dropped at (`x`, `y`) takes in a bar whose tabs are `rects` (in tab order;
/// a tab not shown is zero-size and never counts): the slot before the first tab whose
/// center is past the pointer on the bar's axis (x, or y on a `vertical` bar), else the end.
/// A slot counts the dragged tab too (0 to `rects.len()`).
pub fn drop_index(rects: &[Rect], x: f32, y: f32, vertical: bool) -> usize {
    let pointer = if vertical { y } else { x };
    let center = |r: &Rect| if vertical { r.y + r.h / 2.0 } else { r.x + r.w / 2.0 };
    rects.iter().position(|r| r.w > 0.0 && r.h > 0.0 && center(r) > pointer).unwrap_or(rects.len())
}

/// The drop marker for `slot` in a bar `bar` whose tabs are `rects` (pixels): a line 2 pixels
/// thick across the bar at the slot's edge, the start of the tab at `slot` or the end of the
/// last tab shown. None when no tab shows.
pub fn marker(rects: &[Rect], slot: usize, bar: Rect, vertical: bool) -> Option<Rect> {
    let shown = |r: &&Rect| r.w > 0.0 && r.h > 0.0;
    let edge = match rects.get(slot).filter(shown) {
        Some(r) => if vertical { r.y } else { r.x },
        None => {
            let last = rects.iter().filter(shown).last()?;
            if vertical { last.y + last.h } else { last.x + last.w }
        }
    };
    Some(if vertical { Rect { x: bar.x, y: edge - 1.0, w: bar.w, h: 2.0 } } else { Rect { x: edge - 1.0, y: bar.y, w: 2.0, h: bar.h } })
}

/// `moveTabAtIndex:toIndex:` (`PT:12296-12332`): the index a tab moving from `from` to `to`
/// ends at, kept in its zone. `pinned` is each tab's pinned flag before the move. A pinned
/// tab stops at the last pinned slot; an unpinned tab stops at the first unpinned slot.
pub fn clamp_to_zone(from: usize, to: usize, pinned: &[bool]) -> usize {
    let last_pinned = pinned.iter().rposition(|p| *p);
    match (pinned.get(from).copied().unwrap_or(false), last_pinned) {
        // A pinned tab cannot move past the unpinned zone.
        (true, Some(last)) if to > last => last,
        // An unpinned tab cannot move into the pinned zone.
        (false, Some(last)) if to <= last => last + 1,
        _ => to,
    }
}

/// Where a dragged tab drops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// Its own bar, at this slot.
    OwnBar(usize),
    /// Another window's bar (its registry key), at this slot.
    OtherWindow(u64, usize),
    /// Outside every bar: a new window at the drop point.
    NewWindow,
    /// The drop does nothing.
    Nowhere,
}

/// `shouldDropTabViewItem:inTabBar:` (`PT:7653-7670`): `own_bar_hit` is the slot when the
/// pointer is on the tab's own bar, `other` another window's bar and slot there. The only tab
/// of a window never drops into its own bar (its window would disappear); another window's
/// bar always accepts; outside every bar a new window opens only with
/// `AllowDragOfTabIntoNewWindow` (`allow_new_window`). A solitary tab torn off opens a new
/// window too (13e moves the window instead under Minimal and Compact).
pub fn target(own_bar_hit: Option<usize>, other: Option<(u64, usize)>, tabs_in_window: usize, allow_new_window: bool) -> Target {
    match (own_bar_hit, other) {
        (Some(_), _) if tabs_in_window < 2 => Target::Nowhere,
        (Some(slot), _) => Target::OwnBar(slot),
        (None, Some((window, slot))) => Target::OtherWindow(window, slot),
        (None, None) if allow_new_window => Target::NewWindow,
        (None, None) => Target::Nowhere,
    }
}

/// The release that ends a press on a tab: nothing once the drag began (no select, close or
/// title edit follows a drag), else 13c's release `r`.
pub fn after_drag(began: bool, r: Release) -> Release {
    if began {
        Release::Nothing
    } else {
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn drag_threshold_follows_iterm2() {
        assert!(began((0.0, 0.0), (6.0, 8.0), 10.0), "distance 10");
        assert!(!began((0.0, 0.0), (6.0, 7.0), 10.0));
        assert!(began((5.0, 5.0), (5.0, 5.0), 0.0));
    }

    #[test]
    fn drop_target_follows_iterm2() {
        assert_eq!(target(Some(2), None, 3, true), Target::OwnBar(2));
        // The only tab never drops into its own bar (`PT:7665-7668`).
        assert_eq!(target(Some(0), None, 1, true), Target::Nowhere);
        assert_eq!(target(None, Some((7, 1)), 3, true), Target::OtherWindow(7, 1));
        assert_eq!(target(None, None, 3, true), Target::NewWindow);
        assert_eq!(target(None, None, 3, false), Target::Nowhere);
        // A solitary tab torn off moves to a new window.
        assert_eq!(target(None, None, 1, true), Target::NewWindow);
    }

    #[test]
    fn drag_consumes_release() {
        assert_eq!(after_drag(true, Release::Select(1)), Release::Nothing);
        assert_eq!(after_drag(true, Release::Close(1)), Release::Nothing);
        assert_eq!(after_drag(false, Release::Select(1)), Release::Select(1));
        assert_eq!(after_drag(false, Release::EditTitle(0)), Release::EditTitle(0));
    }

    #[test]
    fn drop_index_follows_iterm2() {
        let row = [r(0.0, 0.0, 100.0, 26.0), r(100.0, 0.0, 100.0, 26.0), r(200.0, 0.0, 100.0, 26.0)];
        assert_eq!(drop_index(&row, 10.0, 5.0, false), 0);
        assert_eq!(drop_index(&row, 60.0, 5.0, false), 1);
        assert_eq!(drop_index(&row, 160.0, 5.0, false), 2);
        assert_eq!(drop_index(&row, 290.0, 5.0, false), 3);
        let column = [r(0.0, 0.0, 150.0, 24.0), r(0.0, 24.0, 150.0, 24.0), r(0.0, 48.0, 150.0, 24.0)];
        assert_eq!(drop_index(&column, 10.0, 30.0, true), 1);
        // A tab not shown (zero-size) never counts as past the pointer.
        let hidden = [r(0.0, 0.0, 100.0, 26.0), r(0.0, 0.0, 0.0, 0.0), r(100.0, 0.0, 100.0, 26.0)];
        assert_eq!(drop_index(&hidden, 60.0, 5.0, false), 2);
        // Unpinned cannot enter the pinned zone.
        assert_eq!(clamp_to_zone(3, 0, &[true, false, false, false]), 1);
        // A pinned tab stops at the last pinned slot.
        assert_eq!(clamp_to_zone(0, 3, &[true, true, false, false]), 1);
        assert_eq!(clamp_to_zone(2, 3, &[false; 4]), 3);
    }

    #[test]
    fn drop_marker_sits_at_the_slot() {
        let bar = r(0.0, 0.0, 400.0, 26.0);
        let row = [r(0.0, 0.0, 100.0, 26.0), r(100.0, 0.0, 100.0, 26.0), r(0.0, 0.0, 0.0, 0.0)];
        assert_eq!(marker(&row, 1, bar, false), Some(r(99.0, 0.0, 2.0, 26.0)));
        // Past the last tab shown: at its end.
        assert_eq!(marker(&row, 3, bar, false), Some(r(199.0, 0.0, 2.0, 26.0)));
        let side = r(0.0, 0.0, 150.0, 300.0);
        let column = [r(0.0, 0.0, 150.0, 24.0), r(0.0, 24.0, 150.0, 24.0)];
        assert_eq!(marker(&column, 1, side, true), Some(r(0.0, 23.0, 150.0, 2.0)));
        assert_eq!(marker(&[], 0, bar, false), None);
    }
}

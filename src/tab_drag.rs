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
            let last = rects.iter().rfind(shown)?;
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
/// `AllowDragOfTabIntoNewWindow` (`allow_new_window`) and never for a pinned tab
/// (`PSMTabDragAssistant shouldCreateNewWindowOnDrop:`, `:1153-1156`). A solitary tab torn
/// off opens a new window too (13e moves the window instead under Minimal and Compact).
pub fn target(own_bar_hit: Option<usize>, other: Option<(u64, usize)>, tabs_in_window: usize, pinned: bool, allow_new_window: bool) -> Target {
    match (own_bar_hit, other) {
        (Some(_), _) if tabs_in_window < 2 => Target::Nowhere,
        (Some(slot), _) => Target::OwnBar(slot),
        (None, Some((window, slot))) => Target::OtherWindow(window, slot),
        (None, None) if allow_new_window && !pinned => Target::NewWindow,
        (None, None) => Target::Nowhere,
    }
}

/// What a left press on the bar arms for the pointer's drag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arm {
    /// The tab's drag (which may move the window instead when it begins).
    Tab,
    /// The group's drag by its chip (never the window).
    Chip,
    /// The "+" button's drag, which moves the window (`AllowDragOnAddTabButton`).
    AddButton,
    /// Only the window's drag: the press itself still clicks.
    Window,
    None,
}

/// What a left press at `hit` arms (`PSM mouseDragged:`, `PSM:3766-3796`): a chip its group's
/// drag; any other point of the bar asks the window drag conversion before a cell is looked up,
/// so the empty bar, a close button and a tab Lock Layout keeps in place (`reorder`: the tab may
/// move, `PT:7610-7616`) arm the window's drag, and a movable tab its own. The "+" and the
/// overflow button are buttons of their own: the "+" drags the window with `add_drags`.
pub fn press_arm(hit: crate::tab_layout::Hit, reorder: bool, add_drags: bool) -> Arm {
    use crate::tab_layout::Hit;
    match hit {
        Hit::Tab(_) if reorder => Arm::Tab,
        Hit::Tab(_) | Hit::Close(_) | Hit::Bar => Arm::Window,
        Hit::Chip(_) => Arm::Chip,
        Hit::Add if add_drags => Arm::AddButton,
        Hit::Add | Hit::Overflow => Arm::None,
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

/// A collapsed group's chip as drawn in a bar (a "pill"), for a tab dropped on it: its rect,
/// its group, the run of its hidden tabs, and whether another pill is right beside it on
/// each side (`PSMTabDragAssistant collapsedGroupChipAtIndex:adjacentPillChipIndexInCells:`).
#[derive(Clone, Debug, PartialEq)]
pub struct Pill {
    pub rect: Rect,
    pub group: String,
    pub run: std::ops::Range<usize>,
    pub left_pill: bool,
    pub right_pill: bool,
}

/// The pills among a bar's drawn chips (`chips`: rect, group, run of tabs, collapsed). The
/// neighbor on a side is a pill when the tab just outside the run there is hidden in a
/// collapsed group (`hidden`, each tab's flag). The tab being dragged (`dragged`) is a gap,
/// not a neighbor: the look goes past it (`collapsedGroupChipAtIndex:adjacentPillChipIndexInCells:`
/// skips placeholder cells, `PSMTabDragAssistant:2872-2876`).
pub fn pills(chips: &[(Rect, String, std::ops::Range<usize>, bool)], hidden: &[bool], dragged: Option<usize>) -> Vec<Pill> {
    let is_hidden = |i: usize| hidden.get(i).copied().unwrap_or(false);
    let before = |start: usize| start.checked_sub(1).and_then(|i| if Some(i) == dragged { i.checked_sub(1) } else { Some(i) });
    let after = |end: usize| if Some(end) == dragged { end + 1 } else { end };
    chips
        .iter()
        .filter(|c| c.3)
        .map(|(rect, group, run, _)| Pill {
            rect: *rect,
            group: group.clone(),
            run: run.clone(),
            left_pill: before(run.start).is_some_and(is_hidden),
            right_pill: is_hidden(after(run.end)),
        })
        .collect()
}

/// `PSM keepCollapsedAfterInBarDropOf:` (`PSMTabBarControl.m:1177-1210`): after a tab
/// (`dropped`) lands in a group, the tab to select instead of it so a collapsed group stays
/// collapsed, or None to select the dropped tab. Only a tab that lands hidden
/// (`dropped_hidden`) and was not its window's selected tab before the press (`before_press`,
/// recorded at the press before the press selects, `PSMTabBarControl.m:3683-3703`) keeps it
/// collapsed; `restore` is the tab to select then (in its own window the one selected before
/// the press, in another window that window's own selection), and it must still be there and
/// shown (`shown`).
pub fn keep_collapsed(dropped: u64, dropped_hidden: bool, before_press: Option<u64>, restore: Option<u64>, shown: &dyn Fn(u64) -> bool) -> Option<u64> {
    if !dropped_hidden || before_press.is_none_or(|b| b == dropped) {
        return None;
    }
    restore.filter(|&r| r != dropped && shown(r))
}

/// Where a tab dropped on a pill goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnPill {
    /// Into the group (`collapsedGroupJoinTargetIdentifier`).
    Join,
    /// Between this pill and the one before it: at the start of this run.
    Before,
    /// Between this pill and the one after it: at the end of this run.
    After,
}

/// The pill's thirds (`PSMTabDragAssistant` chip hover, `:2036-2080`): the middle third joins;
/// an edge third drops between when another pill is on that side, and otherwise joins too.
pub fn on_pill(p: &Pill, x: f32, y: f32, vertical: bool) -> OnPill {
    let (lo, len, along) = if vertical { (p.rect.y, p.rect.h, y) } else { (p.rect.x, p.rect.w, x) };
    if along < lo + len / 3.0 && p.left_pill {
        OnPill::Before
    } else if along >= lo + 2.0 * len / 3.0 && p.right_pill {
        OnPill::After
    } else {
        OnPill::Join
    }
}

/// Where a dragged tab drops in a bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Drop {
    /// At this slot (`drop_index`'s count).
    Slot(usize),
    /// Into this collapsed group.
    Join(String),
}

/// A dragged tab over a bar whose tabs are `rects` and whose pills are `pills`: over a pill
/// it joins that group or drops at the run's edge (`on_pill`); elsewhere it drops at
/// `drop_index`'s slot.
pub fn tab_drop(rects: &[Rect], pills: &[Pill], x: f32, y: f32, vertical: bool) -> Drop {
    match pills.iter().find(|p| p.rect.contains(x, y)) {
        Some(p) => match on_pill(p, x, y, vertical) {
            OnPill::Join => Drop::Join(p.group.clone()),
            OnPill::Before => Drop::Slot(p.run.start),
            OnPill::After => Drop::Slot(p.run.end),
        },
        None => Drop::Slot(drop_index(rects, x, y, vertical)),
    }
}

/// `PSMTabDragAssistant groupDropTargetAvoidingOtherGroups:` (`:2951-2990`): a whole group
/// dragged by its chip never drops inside another group's run. `slot` (`drop_index`'s, over
/// every tab) strictly inside another group's run moves to that run's start when the pointer
/// is before the middle of its tabs shown, else past its end. `groups` is each tab's group id,
/// `dragged` the dragged group, whose own tabs are skipped.
pub fn group_drop_slot(groups: &[Option<&str>], dragged: &str, rects: &[Rect], slot: usize, x: f32, y: f32, vertical: bool) -> usize {
    let other = |i: usize| groups.get(i).copied().flatten().filter(|g| *g != dragged);
    let before = (0..slot.min(groups.len())).rev().find(|&i| groups[i] != Some(dragged));
    let after = (slot..groups.len()).find(|&i| groups[i] != Some(dragged));
    let (Some(b), Some(a)) = (before, after) else { return slot };
    let Some(h) = other(b).filter(|h| other(a) == Some(*h)) else { return slot };
    let start = (0..=b).rev().take_while(|&i| groups[i] == Some(h) || groups[i] == Some(dragged)).last().unwrap_or(b);
    let start = (start..=b).find(|&i| groups[i] == Some(h)).unwrap_or(b);
    let end = (a..groups.len()).take_while(|&i| groups[i] == Some(h) || groups[i] == Some(dragged)).last().unwrap_or(a);
    let end = (a..=end).rev().find(|&i| groups[i] == Some(h)).unwrap_or(a) + 1;
    let shown: Vec<&Rect> = (start..end).filter(|&i| groups[i] == Some(h)).filter_map(|i| rects.get(i)).filter(|r| r.w > 0.0 && r.h > 0.0).collect();
    let (Some(first), Some(last)) = (shown.first(), shown.last()) else { return slot };
    let (mid, along) = if vertical { ((first.y + last.y + last.h) / 2.0, y) } else { ((first.x + last.x + last.w) / 2.0, x) };
    if along < mid { start } else { end }
}

/// `PT reorderGroupTabs:beforeTabViewItem:` (`PT:7725-7760`): the order (old indices) with
/// the group's tabs taken out and put back together, in their order, just before the first
/// other tab at or after `slot`, or at the end.
pub fn group_order(groups: &[Option<&str>], dragged: &str, slot: usize) -> Vec<usize> {
    let members: Vec<usize> = (0..groups.len()).filter(|&i| groups[i] == Some(dragged)).collect();
    let anchor = (slot..groups.len()).find(|&i| groups[i] != Some(dragged));
    let mut order = Vec::with_capacity(groups.len());
    for i in (0..groups.len()).filter(|&i| groups[i] != Some(dragged)) {
        if Some(i) == anchor {
            order.extend(&members);
        }
        order.push(i);
    }
    if anchor.is_none() {
        order.extend(&members);
    }
    order
}

/// Where a group dragged by its chip drops (`PSMTabDragAssistant performGroupDropOntoBar`,
/// `draggedImageEndedAt:operation:`): in its own bar or another window's at the slot;
/// outside every bar a new window opens only with `allow_new_window`
/// (`shouldCreateNewWindowOnDrop:`), never for a pinned first member, and only when the
/// window has another tab (`it_moveTabToNewWindow:` refuses a window's last tab).
pub fn group_target(own_bar_hit: Option<usize>, other: Option<(u64, usize)>, tabs_in_window: usize, first_pinned: bool, allow_new_window: bool) -> Target {
    match (own_bar_hit, other) {
        (Some(slot), _) => Target::OwnBar(slot),
        (None, Some((window, slot))) => Target::OtherWindow(window, slot),
        (None, None) if allow_new_window && !first_pinned && tabs_in_window >= 2 => Target::NewWindow,
        (None, None) => Target::Nowhere,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a left press on the bar arms (`PSM mouseDragged:`, `PSM:3788-3796`): the window
    /// drag conversion is asked before any cell is looked up, so a press on the empty bar, on a
    /// close button, or on a tab that Lock Layout keeps in place still moves the window on a
    /// drag (Alt, the edge strip, a solitary tab); only a group chip never does.
    #[test]
    fn bar_press_arms_the_window_drag_anywhere() {
        use crate::tab_layout::Hit;
        assert_eq!(press_arm(Hit::Tab(0), true, false), Arm::Tab);
        assert_eq!(press_arm(Hit::Tab(0), false, false), Arm::Window, "a layout lock keeps the tab in place");
        assert_eq!(press_arm(Hit::Close(0), true, false), Arm::Window);
        assert_eq!(press_arm(Hit::Bar, true, false), Arm::Window);
        assert_eq!(press_arm(Hit::Chip(0), true, false), Arm::Chip);
        assert_eq!(press_arm(Hit::Chip(0), false, false), Arm::Chip, "Lock Layout refuses a chip's drop instead");
        assert_eq!(press_arm(Hit::Add, true, true), Arm::AddButton);
        assert_eq!(press_arm(Hit::Add, true, false), Arm::None);
        assert_eq!(press_arm(Hit::Overflow, true, true), Arm::None);
    }

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
        assert_eq!(target(Some(2), None, 3, false, true), Target::OwnBar(2));
        // The only tab never drops into its own bar (`PT:7665-7668`).
        assert_eq!(target(Some(0), None, 1, false, true), Target::Nowhere);
        assert_eq!(target(None, Some((7, 1)), 3, false, true), Target::OtherWindow(7, 1));
        assert_eq!(target(None, None, 3, false, true), Target::NewWindow);
        assert_eq!(target(None, None, 3, false, false), Target::Nowhere);
        // A solitary tab torn off moves to a new window.
        assert_eq!(target(None, None, 1, false, true), Target::NewWindow);
        // A pinned tab never tears off (`shouldCreateNewWindowOnDrop:`), but moves between bars.
        assert_eq!(target(None, None, 3, true, true), Target::Nowhere);
        assert_eq!(target(None, Some((7, 0)), 3, true, true), Target::OtherWindow(7, 0));
        assert_eq!(target(Some(1), None, 3, true, true), Target::OwnBar(1));
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

    #[test]
    fn collapsed_drop_joins_like_iterm2() {
        const N: bool = false;
        const H: bool = true;
        // Tabs: a | g1 g2 (collapsed g) | h1 (collapsed h) | b. Chips drawn: g at 100..190,
        // h at 190..280.
        let hidden = [N, H, H, H, N];
        let chips = vec![(r(100.0, 0.0, 90.0, 24.0), "g".to_string(), 1..3, true), (r(190.0, 0.0, 90.0, 24.0), "h".to_string(), 3..4, true), (r(0.0, 0.0, 10.0, 24.0), "x".to_string(), 0..0, false)];
        let p = pills(&chips, &hidden, None);
        assert_eq!(p.len(), 2, "an expanded chip is no pill");
        assert_eq!((p[0].left_pill, p[0].right_pill), (false, true));
        assert_eq!((p[1].left_pill, p[1].right_pill), (true, false));
        // g: its left third joins (a tab is on that side), its right third drops between g
        // and h, at the end of g's run.
        assert_eq!(on_pill(&p[0], 105.0, 5.0, false), OnPill::Join);
        assert_eq!(on_pill(&p[0], 145.0, 5.0, false), OnPill::Join);
        assert_eq!(on_pill(&p[0], 185.0, 5.0, false), OnPill::After);
        // h: its left third is between g and h, at the start of h's run.
        assert_eq!(on_pill(&p[1], 195.0, 5.0, false), OnPill::Before);
        assert_eq!(on_pill(&p[1], 275.0, 5.0, false), OnPill::Join);
        let rects = [r(0.0, 0.0, 100.0, 24.0), r(0.0, 0.0, 0.0, 0.0), r(0.0, 0.0, 0.0, 0.0), r(0.0, 0.0, 0.0, 0.0), r(280.0, 0.0, 100.0, 24.0)];
        assert_eq!(tab_drop(&rects, &p, 145.0, 5.0, false), Drop::Join("g".into()));
        assert_eq!(tab_drop(&rects, &p, 185.0, 5.0, false), Drop::Slot(3));
        assert_eq!(tab_drop(&rects, &p, 195.0, 5.0, false), Drop::Slot(3));
        assert_eq!(tab_drop(&rects, &p, 300.0, 5.0, false), Drop::Slot(4), "off the pills, drop_index");
        // A side bar splits the pill's height.
        let side = Pill { rect: r(0.0, 24.0, 150.0, 24.0), group: "g".into(), run: 0..2, left_pill: true, right_pill: true };
        assert_eq!(on_pill(&side, 5.0, 26.0, true), OnPill::Before);
        assert_eq!(on_pill(&side, 5.0, 36.0, true), OnPill::Join);
        assert_eq!(on_pill(&side, 5.0, 47.0, true), OnPill::After);
        // The dragged tab between two pills is a gap: g's right and h's left neighbor are
        // still pills (a | g1 g2 | dragged | h1 | b).
        let hidden = [N, H, H, N, H, N];
        let chips = vec![(r(100.0, 0.0, 90.0, 24.0), "g".to_string(), 1..3, true), (r(290.0, 0.0, 90.0, 24.0), "h".to_string(), 4..5, true)];
        let p = pills(&chips, &hidden, Some(3));
        assert_eq!((p[0].right_pill, p[1].left_pill), (true, true));
        let p = pills(&chips, &hidden, None);
        assert_eq!((p[0].right_pill, p[1].left_pill), (false, false), "a tab shown between");
        // The dragged tab first in the bar leaves nothing before a pill after it.
        let p = pills(&[(r(0.0, 0.0, 90.0, 24.0), "g".to_string(), 1..2, true)], &[N, H], Some(0));
        assert!(!p[0].left_pill);
    }

    #[test]
    fn background_drop_keeps_group_collapsed() {
        // The press path: the selected tab is recorded, then the press selects the dragged
        // tab (with SelectsTabsOnMouseDown on, the default), then the drop asks.
        let (selected, dragged) = (10u64, 20u64);
        let before_press = Some(selected);
        let selected_now = if crate::tab_layout::press_selects(true, false, 1) { dragged } else { selected };
        assert_eq!(selected_now, dragged, "the press selected the dragged tab");
        let shown = |id: u64| id != 30;
        // A background tab that lands hidden: the tab selected before the press comes back,
        // so the group stays collapsed.
        assert_eq!(keep_collapsed(dragged, true, before_press, before_press, &shown), Some(selected));
        // The dragged tab was the selected one: select it (the group expands).
        assert_eq!(keep_collapsed(dragged, true, Some(dragged), Some(dragged), &shown), None);
        // It did not land hidden (an expanded group): select it.
        assert_eq!(keep_collapsed(dragged, false, before_press, before_press, &shown), None);
        // Nothing valid to restore: select the dropped tab.
        assert_eq!(keep_collapsed(dragged, true, before_press, Some(30), &shown), None, "hidden");
        assert_eq!(keep_collapsed(dragged, true, before_press, None, &shown), None, "gone");
        assert_eq!(keep_collapsed(dragged, true, None, Some(selected), &shown), None, "no press recorded");
        // Another window: its own selection before the drop is restored.
        assert_eq!(keep_collapsed(dragged, true, before_press, Some(40), &shown), Some(40));
    }

    #[test]
    fn group_drag_reorders_like_iterm2() {
        let (g, h) = (Some("g"), Some("h"));
        // a g g b: dragged to the end, to the front, or back where it was.
        let groups = [None, g, g, None];
        assert_eq!(group_order(&groups, "g", 4), vec![0, 3, 1, 2]);
        assert_eq!(group_order(&groups, "g", 0), vec![1, 2, 0, 3]);
        assert_eq!(group_order(&groups, "g", 1), vec![0, 1, 2, 3], "a slot inside its own run");
        assert_eq!(group_order(&groups, "g", 3), vec![0, 1, 2, 3]);
        // Never inside another group: a slot inside h's run goes to the near edge by the
        // middle of h's tabs (100..300, middle 200).
        let groups = [g, h, h, None];
        let rects = [r(0.0, 0.0, 100.0, 24.0), r(100.0, 0.0, 100.0, 24.0), r(200.0, 0.0, 100.0, 24.0), r(300.0, 0.0, 100.0, 24.0)];
        assert_eq!(group_drop_slot(&groups, "g", &rects, 2, 190.0, 5.0, false), 1);
        assert_eq!(group_drop_slot(&groups, "g", &rects, 2, 210.0, 5.0, false), 3);
        assert_eq!(group_drop_slot(&groups, "g", &rects, 3, 310.0, 5.0, false), 3, "after h's run is outside it");
        assert_eq!(group_drop_slot(&groups, "g", &rects, 1, 90.0, 5.0, false), 1, "before h's run is outside it");
        assert_eq!(group_order(&groups, "g", 3), vec![1, 2, 0, 3]);
        // A collapsed run shows no tabs: nothing to split, the slot stays.
        let none = [r(0.0, 0.0, 100.0, 24.0), r(0.0, 0.0, 0.0, 0.0), r(0.0, 0.0, 0.0, 0.0), r(100.0, 0.0, 100.0, 24.0)];
        assert_eq!(group_drop_slot(&groups, "g", &none, 2, 90.0, 5.0, false), 2);
        // Targets: its own bar always, a new window only when allowed, for an unpinned first
        // member of a window with another tab.
        assert_eq!(group_target(Some(2), None, 2, false, true), Target::OwnBar(2));
        assert_eq!(group_target(None, Some((7, 1)), 2, false, true), Target::OtherWindow(7, 1));
        assert_eq!(group_target(None, None, 3, false, true), Target::NewWindow);
        assert_eq!(group_target(None, None, 3, true, true), Target::Nowhere, "a pinned group");
        assert_eq!(group_target(None, None, 3, false, false), Target::Nowhere);
        assert_eq!(group_target(None, None, 1, false, true), Target::Nowhere, "a window's last tab");
    }
}

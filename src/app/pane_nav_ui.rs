//! Ordered panes and last use in the app: the one order every Next Pane, Previous Pane,
//! pane number, swap-with-next and Status ordinal reads, and the activity stamps behind
//! last-use order (`PTYTab orderedSessions`, `setActiveSession:`).

use super::{App, Tab};
use crate::layout::{most_recent, Axis, Dir, PaneId, Tree};
use crate::pane_order;

/// The stamps a focus change takes: the pane that was seen and the pane focused now both get
/// fresh counters (`setActiveSession:`, `TAB:907-914`). None when the focus did not change.
/// With nothing seen yet the new pane takes both (the caller's last write wins).
pub(super) fn activity_step(seen: Option<PaneId>, focused: PaneId, seq: u64) -> Option<[(PaneId, u64); 2]> {
    if seen == Some(focused) {
        return None;
    }
    Some([(seen.unwrap_or(focused), seq + 1), (focused, seq + 2)])
}

/// The pane a direction key picks: the most recently used adjacent pane, else with WrapFocus
/// the most recently used pane at the far edge (`sessionAdjacentTo:`, `TAB:1541-1567`).
pub(super) fn pick_direction(adjacent: &[PaneId], wrap: &[PaneId], wrap_focus: bool, activity: &dyn Fn(PaneId) -> u64) -> Option<PaneId> {
    most_recent(adjacent, activity).or_else(|| if wrap_focus { most_recent(wrap, activity) } else { None })
}

/// Whether a new split pane takes focus (`PT:11365-11370`): only when the split pane was the
/// active one, and then unless focus follows mouse is on with
/// FocusNewSplitPaneWithFocusFollowsMouse off.
pub(super) fn focus_new_split(focus_follows_mouse: bool, key: bool, target_is_active: bool) -> bool {
    target_is_active && (!focus_follows_mouse || key)
}

/// Where a split's profile comes from (`profileForSplit`, `PS:2915-2953`).
#[derive(Debug, PartialEq, Eq)]
pub(super) enum SplitFrom {
    /// The pane's own copy, local changes and all (UseDivorcedProfileToSplit, tested first).
    Divorced,
    /// The saved profile made an SSH one to the pane's host (14b).
    Conductor,
    /// The pane's saved profile.
    Saved,
}

/// The rule behind `ssh_split_profile`: divorced first, then the conductor, then saved.
pub(super) fn split_from(use_divorced: bool, is_copy: bool, has_conductor: bool) -> SplitFrom {
    if use_divorced && is_copy {
        SplitFrom::Divorced
    } else if has_conductor {
        SplitFrom::Conductor
    } else {
        SplitFrom::Saved
    }
}

/// A divider's thickness in pixels (`PTYSplitView dividerThickness`, `PTYSplitView.m:102-119`):
/// SplitPaneDividerWidth when above 0, capped at 5 points, else 1 point; scaled, rounded, at
/// least one pixel.
pub(super) fn divider_px(width_pt: f64, scale: f32) -> f32 {
    let points = if width_pt > 0.0 { width_pt.min(5.0) } else { 1.0 };
    (points as f32 * scale).round().max(1.0)
}

/// The divider color: a valid `#` color from SplitPaneColor overrides `theme`, the color the
/// window's decoration control gives (`PTYSplitView.m:110-119`).
pub(super) fn divider_rgb(custom: &str, theme: crate::palette::Rgb) -> crate::palette::Rgb {
    crate::tab_layout::hex_color(custom.trim()).filter(|_| custom.trim().starts_with('#')).unwrap_or(theme)
}

/// Which order the key picks: reading order with NavigatePanesInReadingOrder, else last use.
pub(super) fn order_source(reading: bool) -> &'static str {
    if reading { "reading" } else { "activity" }
}

impl App {
    /// The divider thickness in pixels: the one gap drawing, hit testing, tmux sizes and the API
    /// share (`SplitPaneDividerWidth`).
    pub(super) fn gap(&self) -> f32 {
        divider_px(self.globals.number("SplitPaneDividerWidth").unwrap_or(0.0), self.scale())
    }

    /// The tab's panes in the order Next Pane, Previous Pane, pane numbers, swap with next
    /// and the Status tool's ordinals share (`PTYTab orderedSessions`).
    pub(super) fn ordered_panes(&self, tab: &Tab) -> Vec<PaneId> {
        if order_source(self.globals.bool("NavigatePanesInReadingOrder")) == "reading" {
            let side_by_side = matches!(tab.layout.tree(), Tree::Split { axis: Axis::Row, .. });
            pane_order::reading_order(&tab.layout.rects(self.content_area_for(tab), self.gap()), side_by_side)
        } else {
            pane_order::activity_order(&tab.layout.panes(), &|id| self.panes.get(&id).map_or(0, |p| p.activity))
        }
    }

    /// The pane `offset` places from the tab's focus in the one order, wrapping.
    pub(super) fn pane_in_order(&self, tab: &Tab, offset: isize) -> Option<PaneId> {
        pane_order::step(&self.ordered_panes(tab), tab.focused, offset)
    }

    /// The pane a number key picks (9 is the last pane).
    pub(super) fn pane_by_number(&self, tab: &Tab, digit: u8) -> Option<PaneId> {
        pane_order::by_number(&self.ordered_panes(tab), digit)
    }

    /// The pane in direction `dir` from the tab's focus (`sessionAdjacentTo:`): the most recent
    /// adjacent pane, else the far edge with WrapFocus. The margin is 1 point times the scale.
    pub(super) fn pane_in_direction(&self, tab: &Tab, dir: Dir) -> Option<PaneId> {
        let (area, margin) = (self.content_area_for(tab), self.scale());
        let activity = |id: PaneId| self.panes.get(&id).map_or(0, |p| p.activity);
        let adjacent = tab.layout.adjacent(tab.focused, dir, area, self.gap(), margin);
        let wrap = if adjacent.is_empty() { tab.layout.wraparound(tab.focused, dir, area, self.gap(), margin) } else { Vec::new() };
        pick_direction(&adjacent, &wrap, self.globals.bool("WrapFocus"), &activity)
    }

    /// Stamps every focus change of this window's tabs since the last pass (every click,
    /// directional pick, new split and API select stamps; quiet moves record themselves).
    pub(super) fn sync_pane_activity(&mut self) {
        for i in 0..self.tabs.len() {
            let (tab_id, focused) = (self.tabs[i].id, self.tabs[i].focused);
            if let Some(stamps) = activity_step(self.pane_seen.get(&tab_id).copied(), focused, self.activity_seq) {
                for (id, n) in stamps {
                    if let Some(p) = self.panes.get_mut(&id) {
                        p.activity = n;
                    }
                }
                self.activity_seq += 2;
                self.pane_seen.insert(tab_id, focused);
            }
        }
    }

    /// Focuses a pane without a stamp (iTerm2's `updateActivityCounter:NO`): the tab's focus
    /// changes, a zoomed tab unzooms, and the pane is recorded as seen.
    pub(super) fn focus_quietly(&mut self, tab: usize, pane: PaneId) {
        let Some(t) = self.tabs.get_mut(tab) else { return };
        t.focused = pane;
        t.zoomed = None;
        let id = t.id;
        self.pane_seen.insert(id, pane);
        self.refresh_route();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane_order::by_number;

    #[test]
    fn direction_pick_follows_iterm2() {
        let stamps = |id: PaneId| if id == 2 { 5 } else { 9 };
        assert_eq!(pick_direction(&[2, 3], &[1], true, &stamps), Some(3), "the adjacent pick wins");
        assert_eq!(pick_direction(&[], &[2, 3], true, &stamps), Some(3), "wrap with WrapFocus");
        assert_eq!(pick_direction(&[], &[2, 3], false, &stamps), None, "no wrap without it");
        assert_eq!(pick_direction(&[], &[], true, &stamps), None);
        let (actions, menu, builtin, app) =
            (include_str!("actions.rs"), include_str!("menu.rs"), include_str!("builtin_ui.rs"), include_str!("mod.rs"));
        for (name, src) in [("actions.rs", actions), ("menu.rs", menu), ("builtin_ui.rs", builtin), ("mod.rs", app)] {
            assert!(!src.contains("layout.neighbor("), "{name} still picks by center distance");
        }
        assert!(actions.contains("pane_in_direction"), "select_pane_dir and swap 53..=56");
        assert!(builtin.contains("pane_in_direction"), "the script API pick");
        assert!(menu.contains("select_pane_dir("), "the menu shares select_pane_dir");
        assert!(app.contains("select_pane_dir("), "Action::Focus shares select_pane_dir");
    }

    #[test]
    fn split_focus_follows_iterm2() {
        assert!(focus_new_split(false, true, true));
        assert!(focus_new_split(true, true, true));
        assert!(!focus_new_split(true, false, true));
        assert!(focus_new_split(false, false, true));
        assert!(!focus_new_split(false, true, false), "a split of a pane that is not active never takes focus");
        let (app, api) = (include_str!("mod.rs"), include_str!("api_requests.rs"));
        assert!(app.contains("focus_new_split("), "split uses the rule");
        assert!(api.contains("focus_new_split("), "the API split uses the rule");
    }

    #[test]
    fn split_profile_follows_iterm2() {
        // B1: divorced first (PS:2916-2921), then 14b's conductor arm, then the saved profile.
        assert_eq!(split_from(true, true, false), SplitFrom::Divorced);
        assert_eq!(split_from(true, true, true), SplitFrom::Divorced, "a divorced SSH pane splits with its own profile");
        assert_eq!(split_from(false, true, true), SplitFrom::Conductor, "key off: the ssh profile");
        assert_eq!(split_from(false, true, false), SplitFrom::Saved);
        assert_eq!(split_from(true, false, true), SplitFrom::Conductor);
        assert_eq!(split_from(true, false, false), SplitFrom::Saved);
    }

    #[test]
    fn ordered_panes_drive_every_path() {
        // The old pane and the new one both take fresh stamps (TAB:907-914).
        assert_eq!(activity_step(Some(1), 2, 7), Some([(1, 8), (2, 9)]));
        assert_eq!(activity_step(Some(2), 2, 7), None);
        assert_eq!(activity_step(None, 2, 7), Some([(2, 8), (2, 9)]));
        assert_eq!(order_source(true), "reading");
        assert_eq!(order_source(false), "activity");
        // Pane numbers (P3: one digit rule, 9 is the last pane).
        let eight: Vec<u64> = (1..=8).collect();
        assert_eq!(by_number(&eight, 8), Some(8));
        assert_eq!(by_number(&eight, 9), Some(8));
        assert_eq!(by_number(&eight, 1), Some(1));
        let ten: Vec<u64> = (1..=10).collect();
        assert_eq!(by_number(&ten, 9), Some(10));
        assert_eq!(by_number(&ten, 8), Some(8));
        let three: Vec<u64> = (1..=3).collect();
        assert_eq!(by_number(&three, 5), None);
        assert_eq!(by_number(&three, 9), Some(3));
        // No second order is left (B2): the stepping callers all read the one order.
        let actions = include_str!("actions.rs");
        let menu = include_str!("menu.rs");
        let app = include_str!("mod.rs");
        for (name, src) in [("actions.rs", actions), ("menu.rs", menu), ("mod.rs", app)] {
            assert!(!src.contains("layout.next("), "{name} still steps in tree order");
        }
        assert!(actions.contains("pane_in_order"), "step_pane and swap 67/68 use pane_in_order");
        assert!(menu.contains("step_pane("), "the menu calls step_pane");
        assert!(app.contains("pane_by_number("), "Route::SelectPane uses pane_by_number");
        assert!(app.contains("self.step_pane("), "Action::NextPane|PrevPane use step_pane");
        let status = include_str!("toolbelt_more_ui.rs");
        assert!(status.contains("ordered_panes("), "the Status tool ordinal reads ordered_panes");
    }

    #[test]
    fn divider_follows_iterm2() {
        // PTYSplitView.m:102-119: a width above 0 is capped at 5 points, else 1 point.
        assert_eq!(divider_px(0.0, 1.0), 1.0);
        assert_eq!(divider_px(0.0, 1.5), 2.0, "1.5 rounds up");
        assert_eq!(divider_px(3.0, 1.0), 3.0);
        assert_eq!(divider_px(9.0, 1.0), 5.0);
        assert_eq!(divider_px(-2.0, 1.0), 1.0);
        assert_eq!(divider_px(2.5, 2.0), 5.0);
        assert_eq!(divider_px(0.1, 1.0), 1.0, "never below one pixel");
        let t = crate::palette::rgb(170, 170, 170);
        assert_eq!(divider_rgb("#ff0000", t), crate::palette::rgb(255, 0, 0));
        assert_eq!(divider_rgb("", t), t);
        assert_eq!(divider_rgb("ff0000", t), t, "only a # color overrides");
        assert_eq!(divider_rgb("#zzz", t), t);
    }

    #[test]
    fn one_gap_for_every_reader() {
        // S6/S7/P13: the constant is gone, so a reader that kept it does not compile.
        let app = include_str!("mod.rs");
        assert!(!app.contains("const GAP"), "mod.rs still defines GAP");
        for (name, src) in [
            ("mod.rs", app),
            ("menu.rs", include_str!("menu.rs")),
            ("statusbar_ui.rs", include_str!("statusbar_ui.rs")),
            ("clippings_ui.rs", include_str!("clippings_ui.rs")),
            ("api_requests.rs", include_str!("api_requests.rs")),
            ("inline_chat_ui.rs", include_str!("inline_chat_ui.rs")),
        ] {
            let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
            let reads = src.lines().filter(|l| !l.trim_start().starts_with("//")).any(|l| {
                l.match_indices("GAP").any(|(i, _)| !word(l[..i].chars().last()) && !word(l[i + 3..].chars().next()))
            });
            assert!(!reads, "{name} still reads GAP");
        }
        assert!(app.contains("self.gap()"), "the grab zone and dividers read gap()");
        assert!(app.contains("divider_rgb("), "divider_color answers SplitPaneColor");
        let settings = include_str!("mod.rs");
        assert!(settings.contains("relayout_changed_windows"), "a settings change relays out every window");
    }
}

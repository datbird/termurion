//! Ordered panes and last use in the app: the one order every Next Pane, Previous Pane,
//! pane number, swap-with-next and Status ordinal reads, and the activity stamps behind
//! last-use order (`PTYTab orderedSessions`, `setActiveSession:`).

use super::{App, Tab, GAP};
use crate::layout::{Axis, PaneId, Tree};
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

/// Which order the key picks: reading order with NavigatePanesInReadingOrder, else last use.
pub(super) fn order_source(reading: bool) -> &'static str {
    if reading { "reading" } else { "activity" }
}

impl App {
    /// The tab's panes in the order Next Pane, Previous Pane, pane numbers, swap with next
    /// and the Status tool's ordinals share (`PTYTab orderedSessions`).
    pub(super) fn ordered_panes(&self, tab: &Tab) -> Vec<PaneId> {
        if order_source(self.globals.bool("NavigatePanesInReadingOrder")) == "reading" {
            let side_by_side = matches!(tab.layout.tree(), Tree::Split { axis: Axis::Row, .. });
            pane_order::reading_order(&tab.layout.rects(self.content_area_for(tab), GAP), side_by_side)
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
}

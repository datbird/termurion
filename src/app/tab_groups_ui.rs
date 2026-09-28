//! Pinned and grouped tabs in a window (milestone 13d): Pin Tab and Unpin Tab
//! (`PTS togglePinTab`, `PTS:355-416`), a tab's group membership, and Move Tab Left/Right
//! (`PT moveCurrentTabByOffset:`, `PT:12242-12295`). The rules are `crate::tab_groups`'s.

use super::*;
use crate::tab_groups::{self, TabGroup};
use crate::winmodel::LayoutOp;

impl App {
    /// Pin Tab or Unpin Tab for the tab `tab` (its id). A tmux tab never pins. A grouped tab
    /// pins or unpins its whole group, refused whole when any member is a tmux tab, so the
    /// group never straddles the pinned boundary. Each tab whose state changes moves as a
    /// lone tab does, to one past the last pinned tab, and the order settles after each move
    /// (`tab_groups::apply_pin`, `PTS:372-392`).
    #[allow(dead_code)] // Task 9's tab menu calls it.
    pub(super) fn toggle_pin(&mut self, tab: u64) {
        let Some(at) = self.tabs.iter().position(|t| t.id == tab) else { return };
        if self.tmux_of(self.tabs[at].focused).is_some() || !self.layout_allowed(LayoutOp::ReorderTab) {
            return;
        }
        let pinned = !self.tabs[at].pinned;
        let members: Vec<usize> = match self.tabs[at].group.as_ref().map(|g| g.id.clone()).filter(|g| !g.is_empty()) {
            Some(gid) => (0..self.tabs.len()).filter(|&i| self.tabs[i].group.as_ref().is_some_and(|g| g.id == gid)).collect(),
            None => vec![at],
        };
        if members.iter().any(|&i| self.tmux_of(self.tabs[i].focused).is_some()) {
            return;
        }
        let order = {
            let groups: Vec<Option<&str>> = self.tabs.iter().map(|t| t.group.as_ref().map(|g| g.id.as_str())).collect();
            let flags: Vec<bool> = self.tabs.iter().map(|t| t.pinned).collect();
            tab_groups::apply_pin(&groups, &flags, &members, pinned)
        };
        for &i in &members {
            self.tabs[i].pinned = pinned;
        }
        let active = self.tabs.get(self.active).map(|t| t.id);
        let mut old: Vec<Option<Tab>> = std::mem::take(&mut self.tabs).into_iter().map(Some).collect();
        self.tabs = order.into_iter().filter_map(|i| old.get_mut(i).and_then(Option::take)).collect();
        if let Some(a) = active.and_then(|a| self.tabs.iter().position(|t| t.id == a)) {
            self.active = a;
        }
        self.settle_tabs();
        self.tmux_reorder();
        self.relayout();
        self.redraw();
    }

    /// A pinned tab's width in pixels: `PinnedTabWidth` points at the window's scale.
    #[allow(dead_code)] // 13h reads it.
    pub(super) fn pinned_width(&self) -> f32 {
        tab_layout::Settings::from_globals(&self.globals).pinned_width * self.scale()
    }

    /// The tab at `at` joins the group `id` (None: no group) and takes its definition and pin
    /// state from the other members (`tab_groups::reconcile`, `PT reconcileTabGroupDefinitionForTab:`).
    /// The order is left to the caller to settle.
    pub(super) fn set_tab_group(&mut self, at: usize, id: Option<String>) {
        let Some(tab) = self.tabs.get(at) else { return };
        let mut group = id.map(|id| TabGroup { id, name: String::new(), color: tab_groups::PALETTE[0], collapsed: false });
        let mut pinned = tab.pinned;
        let others: Vec<(Option<&TabGroup>, bool)> = self.tabs.iter().enumerate().filter(|(i, _)| *i != at).map(|(_, t)| (t.group.as_ref(), t.pinned)).collect();
        tab_groups::reconcile(&mut group, &mut pinned, &others);
        let tab = &mut self.tabs[at];
        tab.group = group;
        tab.pinned = pinned;
    }

    /// Move Tab Left (`offset` -1) or Right (+1) for the selected tab, one step by
    /// `tab_groups::single_tab_move`: inside a group it moves within, at a group's edge it
    /// leaves the group in place, next to a group an ungrouped tab joins it in place, and
    /// the ends wrap. The order then settles, which keeps the tab in its pinned or unpinned
    /// zone (`PT applyTabOrder:`). Lock Layout refuses.
    pub(super) fn move_active_tab(&mut self, offset: i32) {
        if self.tabs.len() < 2 || !self.layout_allowed(LayoutOp::ReorderTab) {
            return;
        }
        let step = {
            let order: Vec<Option<&str>> = self.tabs.iter().map(|t| t.group.as_ref().map(|g| g.id.as_str())).collect();
            tab_groups::single_tab_move(&order, self.active, offset)
        };
        if step.changes_membership {
            self.set_tab_group(self.active, step.new_group);
        } else {
            let id = self.tabs[self.active].id;
            let mut old: Vec<Option<Tab>> = std::mem::take(&mut self.tabs).into_iter().map(Some).collect();
            self.tabs = step.order.iter().filter_map(|&i| old.get_mut(i).and_then(Option::take)).collect();
            self.active = self.tabs.iter().position(|t| t.id == id).unwrap_or(0);
        }
        self.settle_tabs();
        self.tmux_reorder();
    }
}

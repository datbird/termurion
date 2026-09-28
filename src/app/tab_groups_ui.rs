//! Pinned and grouped tabs in a window (milestone 13d): Pin Tab and Unpin Tab
//! (`PTS togglePinTab`, `PTS:355-416`), a tab's group membership, Move Tab Left/Right
//! (`PT moveCurrentTabByOffset:`, `PT:12242-12295`), the tab group commands (`PT:8953-9910`),
//! Duplicate Tab (`PT:14414-14535`) and the tab and group context menus (`PT:8253-8500`,
//! `PT:9427-9478`). The rules are `crate::tab_groups`'s and `crate::tab_menu`'s.

use super::*;
use crate::prompt_ui::{PromptResult, PromptWindow};
use crate::tab_groups::{self, CloseTabs, TabGroup};
use crate::tab_menu::{self, TabCtx};
use crate::winmodel::LayoutOp;
use std::path::Path;

/// What the tab group name window names.
#[derive(Clone, Debug)]
pub(super) enum GroupPrompt {
    /// Add Tab to New Group for the tab (id) of the window.
    New { window: WindowId, tab: u64 },
    /// Rename Group for the group (id) of the window.
    Rename { window: WindowId, group: String },
}

impl App {
    /// Pin Tab or Unpin Tab for the tab `tab` (its id). A tmux tab never pins. A grouped tab
    /// pins or unpins its whole group, refused whole when any member is a tmux tab, so the
    /// group never straddles the pinned boundary. Each tab whose state changes moves as a
    /// lone tab does, to one past the last pinned tab, and the order settles after each move
    /// (`tab_groups::apply_pin`, `PTS:372-392`).
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
        self.apply_order(&order);
        self.settle_tabs();
        self.tmux_reorder();
        self.relayout();
        self.redraw();
    }

    /// A pinned tab's width in pixels: `PinnedTabWidth` points at the window's scale.
    #[allow(dead_code)] // 13h reads it.
    pub(super) fn pinned_width(&self) -> f32 {
        self.tab_settings().pinned_width * self.scale()
    }

    /// The tab at `at` joins the group `id` (None: no group) and takes its definition and pin
    /// state from the other members (`tab_groups::join`, `PT reconcileTabGroupDefinitionForTab:`);
    /// the sole carrier of an id keeps its own. The order is left to the caller to settle.
    pub(super) fn set_tab_group(&mut self, at: usize, id: Option<String>) {
        let Some(tab) = self.tabs.get(at) else { return };
        let others: Vec<(Option<&TabGroup>, bool)> = self.tabs.iter().enumerate().filter(|(i, _)| *i != at).map(|(_, t)| (t.group.as_ref(), t.pinned)).collect();
        let (group, pinned) = tab_groups::join(tab.group.as_ref(), id, tab.pinned, &others);
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
            self.apply_order(&step.order);
        }
        self.settle_tabs();
        self.tmux_reorder();
    }

    /// The tabs take iTerm2's canonical order (`tab_groups::canonical_order`: pinned tabs
    /// first, each group's members together), keeping the selected tab by id
    /// (`PT enforceTabGroupContiguityInvariant`), then each group one collapsed state, never
    /// the selected tab's (`tab_groups::normalized_collapsed`,
    /// `PT normalizeTabGroupCollapsedState`). Every path that inserts or moves a tab ends
    /// here. True when the order changed.
    pub(super) fn settle_tabs(&mut self) -> bool {
        let (order, collapsed, active) = {
            let pinned: Vec<bool> = self.tabs.iter().map(|t| t.pinned).collect();
            let flags: Vec<bool> = self.tabs.iter().map(|t| t.group.as_ref().is_some_and(|g| g.collapsed)).collect();
            settle_plan(&self.group_order(), &pinned, &flags, self.active)
        };
        let moved = order.iter().enumerate().any(|(k, i)| k != *i);
        if moved {
            self.apply_order(&order);
        }
        if !self.tabs.is_empty() {
            self.active = active;
        }
        for (t, c) in self.tabs.iter_mut().zip(collapsed) {
            if let Some(g) = t.group.as_mut() {
                g.collapsed = c;
            }
        }
        moved
    }

    /// The tabs take `order` (indices of the tabs as they are, each once), the selected tab
    /// kept by id.
    pub(super) fn apply_order(&mut self, order: &[usize]) {
        let active = self.tabs.get(self.active).map(|t| t.id);
        let mut old: Vec<Option<Tab>> = std::mem::take(&mut self.tabs).into_iter().map(Some).collect();
        self.tabs = order.iter().filter_map(|&i| old.get_mut(i).and_then(Option::take)).collect();
        if let Some(a) = active.and_then(|a| self.tabs.iter().position(|t| t.id == a)) {
            self.active = a;
        }
    }

    /// Each tab's group id, in tab order (None when ungrouped).
    pub(super) fn group_order(&self) -> Vec<Option<&str>> {
        self.tabs.iter().map(|t| t.group.as_ref().map(|g| g.id.as_str()).filter(|g| !g.is_empty())).collect()
    }

    /// The indices of the group's members, in tab order (`PT tabsInGroup:`).
    pub(super) fn members(&self, group: &str) -> Vec<usize> {
        (0..self.tabs.len()).filter(|&i| self.tabs[i].group.as_ref().is_some_and(|g| g.id == group)).collect()
    }

    /// The ids of the tabs at `indices`.
    pub(super) fn tab_ids(&self, indices: &[usize]) -> Vec<u64> {
        indices.iter().filter_map(|&i| self.tabs.get(i)).map(|t| t.id).collect()
    }

    /// After a membership change: the order settles, tmux hears a changed order, and the
    /// window lays out again (`PT finalizeTabGroupMembershipChangeForTab:`).
    pub(super) fn group_changed(&mut self) {
        if self.settle_tabs() {
            self.tmux_reorder();
        }
        self.relayout();
        self.redraw();
    }

    /// The selected tab's group expands when it is collapsed, so the selected tab never hides
    /// (`PT expandTabGroupIfSelectedTabIsCollapsed`).
    pub(super) fn expand_selected_group(&mut self) {
        if expand_selected(&mut self.tabs, self.active) {
            self.relayout();
            self.redraw();
        }
    }

    /// Add Tab to New Group... (`PT addTab:toNewGroupWithDefaultName:`, `PT:8953-8967`): the
    /// name window, seeded with the tab's label; OK makes the tab the sole member of a new
    /// group with the palette's next color, Cancel does nothing.
    pub(super) fn add_tab_to_new_group(&mut self, event_loop: &ActiveEventLoop, tab: u64) {
        let Some(t) = self.tabs.iter().find(|t| t.id == tab) else { return };
        let look = self.tab_look(t);
        let name = tab_groups::default_name(&requests_ui::tab_label(&look.title, &look.subtitle));
        let Some(window) = self.current_window() else { return };
        self.open_group_prompt(event_loop, tab_groups::NEW_GROUP_TITLE, &name, GroupPrompt::New { window, tab });
    }

    /// Rename Group... (`PT renameTabGroupWithID:`, `PT:9742-9760`): the name window, seeded
    /// with the group's name; OK names every member.
    pub(super) fn rename_group(&mut self, event_loop: &ActiveEventLoop, group: &str) {
        let Some(&first) = self.members(group).first() else { return };
        let name = self.tabs[first].group.as_ref().map(|g| g.name.clone()).unwrap_or_default();
        let Some(window) = self.current_window() else { return };
        self.open_group_prompt(event_loop, tab_groups::RENAME_GROUP_TITLE, &name, GroupPrompt::Rename { window, group: group.to_string() });
    }

    /// The tab group name window (`PT promptForTabGroupName:title:`, `PT:8993-9022`): OK and
    /// Cancel, the text trimmed; an empty name is allowed.
    fn open_group_prompt(&mut self, event_loop: &ActiveEventLoop, title: &str, text: &str, what: GroupPrompt) {
        if let Some((w, _)) = &self.group_prompt {
            return w.focus();
        }
        let Some(g) = self.gpu.as_ref() else { return };
        match PromptWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, title, tab_groups::GROUP_PROMPT, text.trim()) {
            Ok(w) => self.group_prompt = Some((w, what)),
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// Events for the tab group name window. False when `window` is not it.
    pub(super) fn group_prompt_event(&mut self, window: WindowId, event: &WindowEvent) -> bool {
        let Some((w, _)) = self.group_prompt.as_mut() else { return false };
        if w.id() != window {
            return false;
        }
        match event {
            WindowEvent::CloseRequested => self.group_prompt = None,
            WindowEvent::RedrawRequested => match w.redraw() {
                PromptResult::Open | PromptResult::Extra(_) => {}
                PromptResult::Cancel => self.group_prompt = None,
                PromptResult::Ok(text) => {
                    let Some((_, what)) = self.group_prompt.take() else { return true };
                    let name = text.trim().to_string();
                    match what {
                        GroupPrompt::New { window, tab } => {
                            if self.enter(window) {
                                self.make_group(tab, name);
                            }
                        }
                        GroupPrompt::Rename { window, group } => {
                            if self.enter(window) {
                                for i in self.members(&group) {
                                    if let Some(g) = self.tabs[i].group.as_mut() {
                                        g.name = name.clone();
                                    }
                                }
                                // Saved with the session, like every other group change.
                                self.mark_state_dirty();
                                self.redraw();
                            }
                        }
                    }
                }
            },
            other => {
                w.event(other);
            }
        }
        true
    }

    /// The tab `tab` becomes the sole member of a new group named `name`, colored by the
    /// number of groups the window has (`tab_groups::next_color`).
    fn make_group(&mut self, tab: u64, name: String) {
        let Some(at) = self.tabs.iter().position(|t| t.id == tab) else { return };
        let count = tab_groups::groups_in_window(self.tabs.iter().map(|t| t.group.as_ref())).len();
        self.tabs[at].group = Some(TabGroup { id: tab_groups::fresh_id(), name, color: tab_groups::next_color(count), collapsed: false });
        self.set_tab_group(at, self.tabs[at].group.as_ref().map(|g| g.id.clone()));
        self.group_changed();
    }

    /// Add Tab to Group > a group (`PT addTabToExistingGroup:`, `PT:8969-8984`): the tab
    /// joins it, taking its name, color, collapsed and pinned state, and moves into its run.
    pub(super) fn add_tab_to_group(&mut self, tab: u64, group: &str) {
        let Some(at) = self.tabs.iter().position(|t| t.id == tab) else { return };
        if self.members(group).is_empty() {
            return;
        }
        self.set_tab_group(at, Some(group.to_string()));
        self.group_changed();
    }

    /// Remove Tab from Group (`PT removeTabFromGroup:label:`, `PT:9048-9061`): the tab leaves
    /// its group and lands just after it.
    pub(super) fn remove_tab_from_group(&mut self, tab: u64) {
        let Some(at) = self.tabs.iter().position(|t| t.id == tab) else { return };
        if self.tabs[at].group.is_none() {
            return;
        }
        self.set_tab_group(at, None);
        self.group_changed();
    }

    /// Remove All Tabs from Group (`PT ungroupTabGroup:`, `PT:9762-9769`).
    pub(super) fn ungroup(&mut self, group: &str) {
        let members = self.members(group);
        if members.is_empty() {
            return;
        }
        for i in members {
            self.set_tab_group(i, None);
        }
        self.group_changed();
    }

    /// Group Color (`PT setTabGroupColor:forGroupID:`, `PT:9511-9523`): every member.
    pub(super) fn set_group_color(&mut self, group: &str, color: crate::palette::Rgb) {
        for i in self.members(group) {
            if let Some(g) = self.tabs[i].group.as_mut() {
                g.color = color;
            }
        }
        self.mark_state_dirty();
        self.redraw();
    }

    /// Collapse Group (`PT collapseTabGroup:animated:`, `PT:9538-9593`): selection first
    /// leaves the group (`collapse_plan`), then every member hides in the bar but stays in the
    /// window. Refused silently for a group that is the whole window.
    pub(super) fn collapse_group(&mut self, group: &str) {
        let plan = {
            let flags: Vec<bool> = self.tabs.iter().map(|t| t.group.as_ref().is_some_and(|g| g.collapsed)).collect();
            collapse_plan(&self.group_order(), &flags, self.active, group)
        };
        let Some((select, expand)) = plan else { return };
        if let Some(g) = expand {
            self.expand_group(&g);
        }
        if let Some(i) = select {
            self.select_tab(i);
        }
        for i in self.members(group) {
            if let Some(g) = self.tabs[i].group.as_mut() {
                g.collapsed = true;
            }
        }
        self.relayout();
        self.redraw();
    }

    /// Expand Group (`PT expandTabGroup:animated:`, `PT:9599-9619`); an expanded group stays.
    pub(super) fn expand_group(&mut self, group: &str) {
        let mut changed = false;
        for i in self.members(group) {
            if let Some(g) = self.tabs[i].group.as_mut().filter(|g| g.collapsed) {
                g.collapsed = false;
                changed = true;
            }
        }
        if changed {
            self.relayout();
            self.redraw();
        }
    }

    /// A click on a group's chip (`PT toggleTabGroupCollapsed:`, `PT:9621-9637`): a collapsed
    /// group expands, an expanded one collapses unless it is the whole window.
    pub(super) fn toggle_group_collapsed(&mut self, group: &str) {
        let Some(&first) = self.members(group).first() else { return };
        if self.tabs[first].group.as_ref().is_some_and(|g| g.collapsed) {
            self.expand_group(group);
        } else if !tab_groups::whole_window(&self.group_order(), group) {
            self.collapse_group(group);
        }
    }

    /// Select Next Tab Group (`offset` 1) or Select Previous Tab Group (-1)
    /// (`PT selectTabGroupByOffset:`, `PT:2930-2950`).
    pub(super) fn select_tab_group(&mut self, offset: i32) {
        if let Some(i) = tab_groups::select_by_offset(&self.group_order(), self.active, offset) {
            self.select_tab(i);
        }
    }

    /// Close Group (`PT closeTabGroup:`, `PT:9845-9851`): every member, pinned ones too.
    pub(super) fn close_group(&mut self, group: &str) {
        let ids = self.tab_ids(&self.members(group));
        self.close_tabs(ids, CloseTabs::Group);
    }

    /// The group menu's Close Other Tabs (`PT closeTabsOutsideGroup:`, `PT:9853-9862`): every
    /// tab outside the group but the pinned ones.
    pub(super) fn close_tabs_outside_group(&mut self, group: &str) {
        let outside: Vec<usize> = (0..self.tabs.len()).filter(|&i| self.tabs[i].group.as_ref().is_none_or(|g| g.id != group) && !self.tabs[i].pinned).collect();
        let ids = self.tab_ids(&outside);
        self.close_tabs(ids, CloseTabs::Outside);
    }

    /// The group menu's Close Tabs to the Right, Close Tabs Below on a side bar
    /// (`PT closeTabsRightOfGroup:`, `PT:9864-9880`): every tab after the group's last member
    /// but the pinned ones.
    pub(super) fn close_tabs_right_of_group(&mut self, group: &str) {
        let Some(&last) = self.members(group).last() else { return };
        let right: Vec<usize> = (last + 1..self.tabs.len()).filter(|&i| !self.tabs[i].pinned).collect();
        let ids = self.tab_ids(&right);
        self.close_tabs(ids, CloseTabs::Right);
    }

    /// Closes the tabs `ids` after one question (`PT closeTabs:confirmWith:skippingPinned:`,
    /// `PT:9890-9917`): iTerm2's persistent warning, in the alert queue, asked once while the
    /// same question waits; each tab then closes as `closeTab:` does, a tmux tab asking tmux.
    /// Lock Layout refuses.
    fn close_tabs(&mut self, ids: Vec<u64>, what: CloseTabs) {
        if ids.is_empty() || !self.layout_allowed(LayoutOp::CloseTab) {
            return;
        }
        let Some(window) = self.current_window() else { return };
        let vertical = tab_position::vertical(self.tab_position());
        let ask = close_tabs_ask(window, what, vertical, ids);
        if !self.alert_waits(|then| *then == ask.then) {
            self.ask_scripts(ask);
        }
    }

    /// The Close Tabs question answered (`Then::CloseTabs`): Close (0) closes the tabs of
    /// `ids` still in `window` on the loop's next pass (a close needs the event loop); Cancel,
    /// or closing the alert, closes nothing.
    pub(super) fn close_tabs_answered(&mut self, window: WindowId, ids: Vec<u64>, index: usize) {
        if index == 0 {
            self.pending_tab_closes.push((window, ids));
        }
    }

    /// Runs the closes Close Tabs answers left, each in the window it was asked from (gone
    /// with it: nothing); Lock Layout, turned on meanwhile, refuses.
    pub(super) fn finish_tab_closes(&mut self, event_loop: &ActiveEventLoop) {
        for (window, ids) in std::mem::take(&mut self.pending_tab_closes) {
            if self.enter(window) && self.layout_allowed(LayoutOp::CloseTab) {
                self.close_each(ids, event_loop);
            }
        }
    }

    /// Closes the tabs `ids` as `closeTab:` does, asking nothing (a tmux tab asks tmux).
    fn close_each(&mut self, ids: Vec<u64>, event_loop: &ActiveEventLoop) {
        let Some(window) = self.current_window() else { return };
        for id in ids {
            // The window goes with its last tab.
            if !self.enter(window) {
                break;
            }
            let Some(i) = self.tabs.iter().position(|t| t.id == id) else { continue };
            if !self.tmux_close_tab(i) {
                self.close_tab_now(i, event_loop);
            }
        }
    }

    /// Move Group to New Window (`PT moveTabGroupToNewWindow:`, `PT:9771-9780`): the members
    /// go to a new window keeping their group; nothing happens when the group is the whole
    /// window. A tab holding a loaded recording never moves, so neither does its group.
    pub(super) fn move_group_to_new_window(&mut self, group: &str) {
        let members = self.members(group);
        if members.is_empty() || members.len() == self.tabs.len() || members.iter().any(|&i| self.holds_recording(&self.tabs[i])) || !self.layout_allowed(LayoutOp::MoveTab) {
            return;
        }
        // Lock Layout beeps here; `take_group` keeps the selection, or moves it to the nearest
        // tab outside the group.
        let Some((moving, _)) = self.take_group(group) else { return };
        self.pending_windows.push(windows_ui::NewWindow::Tabs(moving));
        self.group_changed();
    }

    /// Save Group as Window Arrangement... (`PT saveTabGroupAsWindowArrangement:`,
    /// `PT:9782-9796`): the arrangement name window, saving only the members.
    pub(super) fn save_group_arrangement(&mut self, event_loop: &ActiveEventLoop, group: &str) {
        let ids = self.tab_ids(&self.members(group));
        let Some(window) = self.current_window().filter(|_| !ids.is_empty()) else { return };
        self.open_save_arrangement_for(event_loop, arrange_ui::SaveWhat::Tabs(window, ids));
    }

    /// New Tab in Group (`PT newTabInGroup:`, `PT:14346-14357`): a new tab of the default
    /// profile just after the group's last member, joining the group.
    pub(super) fn new_tab_in_group(&mut self, group: &str) {
        let Some(&last) = self.members(group).last() else { return };
        let before = self.tabs.len();
        self.new_tab(None);
        let Some(at) = move_new_tab(&mut self.tabs, before, last) else { return };
        self.active = at;
        self.set_tab_group(at, Some(group.to_string()));
        self.group_changed();
    }

    /// A New Tab just opened at `at` joins the group the selected tab `from` was in when it
    /// landed right after it (`tab_groups::inherited_group`), then the order settles (beside
    /// a pinned tab it goes after the pinned ones).
    pub(super) fn new_tab_joins_group(&mut self, at: usize, from: usize, group: Option<String>) {
        if let Some(g) = tab_groups::inherited_group(from, group.as_deref(), at) {
            self.set_tab_group(at, Some(g));
        }
        if self.settle_tabs() {
            self.tmux_reorder();
        }
    }

    /// Duplicate Tab (`PT duplicateTab:`, `PT createDuplicateOfTab:`, `PT:14414-14535`): a
    /// copy of the tab at `at` with every pane, split, profile and folder, new sessions in
    /// them. It opens where a new tab of the current profile would: in a new window it opens
    /// alone with no group; in this window it goes where `duplicate_placement` says, joins the
    /// original's group and is selected. Lock Layout refuses.
    pub(super) fn duplicate_tab(&mut self, at: usize) {
        if at >= self.tabs.len() || !self.layout_allowed(LayoutOp::AddTab) {
            return;
        }
        let guid = self.focused_guid();
        if self.new_tab_opens_window(guid.as_deref()) {
            let Some(value) = self.snapshot_tab(at) else { return };
            match self.build_tab(&value, None) {
                Ok(mut tab) => {
                    tab.group = None;
                    self.pending_windows.push(windows_ui::NewWindow::Tab(tab));
                }
                Err(e) => self.banners.set("arrangement", format!("Could not duplicate the tab: {e}")),
            }
            return self.redraw();
        }
        if let Some(i) = self.duplicate_here(at).and_then(|id| self.tabs.iter().position(|t| t.id == id)) {
            self.select_tab(i);
        }
        self.group_changed();
    }

    /// Duplicate Group (`PT duplicateTabGroup:`, `PT:9798-9843`): each member is duplicated in
    /// this window, the copies unpinned and in a new group named "<name> copy" with the
    /// group's color, expanded; the order then settles, the copies after the group. Lock
    /// Layout refuses.
    pub(super) fn duplicate_group(&mut self, group: &str) {
        let members = self.members(group);
        let Some(first) = members.first().and_then(|&i| self.tabs[i].group.clone()) else { return };
        if !self.layout_allowed(LayoutOp::AddTab) {
            return;
        }
        let copy = TabGroup { id: tab_groups::fresh_id(), name: tab_groups::copy_name(&first.name), color: first.color, collapsed: false };
        let mut last = None;
        for id in self.tab_ids(&members) {
            let Some(at) = self.tabs.iter().position(|t| t.id == id) else { continue };
            let Some(made) = self.duplicate_here(at) else { continue };
            if let Some(t) = self.tabs.iter_mut().find(|t| t.id == made) {
                t.pinned = false;
                t.group = Some(copy.clone());
            }
            last = Some(made);
        }
        if let Some(i) = last.and_then(|id| self.tabs.iter().position(|t| t.id == id)) {
            self.select_tab(i);
        }
        self.group_changed();
    }

    /// A copy of the tab at `at` in this window (`snapshot_tab`, then `build_tab`), placed by
    /// `duplicate_placement` and joined to the original's group. Its id; None when it could
    /// not be made (a banner says why).
    fn duplicate_here(&mut self, at: usize) -> Option<u64> {
        let value = self.snapshot_tab(at)?;
        let tmux = self.tmux_of(self.tabs[at].focused).is_some();
        let (place, group) = duplicate_placement(at, self.tabs.len(), self.globals.bool("DuplicatedTabsOpenAdjacentToOriginal"), tmux, true, self.tabs[at].group.as_ref());
        let tab = match self.build_tab(&value, None) {
            Ok(t) => t,
            Err(e) => {
                self.banners.set("arrangement", format!("Could not duplicate the tab: {e}"));
                return None;
            }
        };
        let id = tab.id;
        let i = place.unwrap_or(self.tabs.len()).min(self.tabs.len());
        if i <= self.active && !self.tabs.is_empty() {
            self.active += 1;
        }
        self.tabs.insert(i, tab);
        self.set_tab_group(i, group.map(|g| g.id));
        Some(id)
    }

    /// The tab context menu for the tab `tab` (its id) at the pointer
    /// (`PT tabView:menuForTabViewItem:`); the picks come back as `tab-menu:` ids.
    pub(super) fn show_tab_menu(&mut self, tab: u64) {
        let Some(index) = self.tabs.iter().position(|t| t.id == tab) else { return };
        let html = self.globals.bool("HTMLTabTitles");
        // Each tab's title as plain text (a menu item has one line).
        let labels = self.tabs.iter().map(|t| tab_layout::plain_title(&tab_layout::limit_title(&self.tab_look(t).title), html)).collect();
        let groups = tab_groups::groups_in_window(self.tabs.iter().map(|t| t.group.as_ref())).into_iter().map(|g| (g.id, g.name)).collect();
        let t = &self.tabs[index];
        let ctx = TabCtx {
            tab,
            labels,
            ids: self.tabs.iter().map(|t| t.id).collect(),
            index,
            pinned: self.tabs.iter().map(|t| t.pinned).collect(),
            tmux: self.tmux_of(t.focused).is_some(),
            vertical: tab_position::vertical(self.tab_position()),
            group: t.group.as_ref().map(|g| g.id.clone()).filter(|g| !g.is_empty()),
            groups,
            layout_locked: self.win_locks.layout,
        };
        self.show_entries(&tab_menu::tab_menu(&ctx), "Tab menu");
    }

    /// A group chip's context menu (`PT tabView:menuForTabGroup:`); the picks come back as
    /// `tab-group-menu:` ids. Nothing for a group with no members.
    pub(super) fn show_group_menu(&mut self, group: &str) {
        let Some(&first) = self.members(group).first() else { return };
        let collapsed = self.tabs[first].group.as_ref().is_some_and(|g| g.collapsed);
        let whole = tab_groups::whole_window(&self.group_order(), group);
        let vertical = tab_position::vertical(self.tab_position());
        self.show_entries(&tab_menu::group_menu(group, collapsed, whole, vertical), "Tab group menu");
    }

    /// A pick from the tab or group context menu runs it (`PT:8253-8500`, `PT:9427-9478`).
    /// False when `id` is neither menu's.
    pub(super) fn tab_menu_picked(&mut self, id: &str, event_loop: &ActiveEventLoop) -> bool {
        let Some((action, target)) = tab_menu::parse(id) else { return false };
        if id.starts_with(tab_menu::GROUP_PREFIX) {
            self.group_menu_action(&action, &target, event_loop);
        } else {
            let (tab, group) = target.split_once(':').map_or((target.as_str(), None), |(t, g)| (t, Some(g)));
            if let Ok(tab) = tab.parse::<u64>() {
                self.tab_menu_action(&action, tab, group, event_loop);
            }
        }
        self.redraw();
        true
    }

    /// Runs the tab menu's `action` for the tab `tab` (its id; nothing when it has closed
    /// since); `group` is the group a join picked.
    fn tab_menu_action(&mut self, action: &str, tab: u64, group: Option<&str>, event_loop: &ActiveEventLoop) {
        let Some(at) = self.tabs.iter().position(|t| t.id == tab) else { return };
        match action {
            "select" => self.select_tab(at),
            // `PT editSession:`: the tab's focused session.
            "edit-session" => {
                let pane = self.tabs[at].focused;
                self.edit_session(Some(pane), event_loop);
            }
            "new-tab" => self.new_tab_beside(at),
            "duplicate" => self.duplicate_tab(at),
            "close" => self.close_tab_at(at, event_loop),
            // `PT closeOtherTabs:` and `closeTabsToTheRight:`: the unpinned tabs, each closed
            // as `closeTab:` does, asking nothing; Lock Layout refuses silently.
            "close-others" | "close-right" if self.layout_allowed_quiet(LayoutOp::CloseTab) => {
                let from = if action == "close-right" { at + 1 } else { 0 };
                let close: Vec<usize> = (from..self.tabs.len()).filter(|&i| i != at && !self.tabs[i].pinned).collect();
                let ids = self.tab_ids(&close);
                self.close_each(ids, event_loop);
            }
            "move-window" if self.layout_allowed(LayoutOp::MoveTab) => self.move_tab_at_to_new_window(at),
            "pin" => self.toggle_pin(tab),
            "new-group" => self.add_tab_to_new_group(event_loop, tab),
            "join" => {
                if let Some(g) = group {
                    self.add_tab_to_group(tab, g);
                }
            }
            "rename-group" => {
                if let Some(g) = self.tabs[at].group.as_ref().map(|g| g.id.clone()) {
                    self.rename_group(event_loop, &g);
                }
            }
            "remove-group" => self.remove_tab_from_group(tab),
            // `PT saveTabAsWindowArrangement:`: the arrangement name window, saving this tab.
            "save" => {
                if let Some(window) = self.current_window() {
                    self.open_save_arrangement_for(event_loop, arrange_ui::SaveWhat::Tabs(window, vec![tab]));
                }
            }
            "color" => {
                let swatches = tab_layout::tab_colors(&self.globals.string("TabColorMenuOptions"));
                self.open_color_picker(menu::ColorTarget::Tab(tab), swatches);
            }
            _ => {}
        }
    }

    /// Runs the group menu's `action` for the group `group`.
    fn group_menu_action(&mut self, action: &str, group: &str, event_loop: &ActiveEventLoop) {
        match action {
            "new-tab" => self.new_tab_in_group(group),
            "collapse" => self.collapse_group(group),
            "expand" => self.expand_group(group),
            "rename" => self.rename_group(event_loop, group),
            "color" => self.open_color_picker(menu::ColorTarget::Group(group.to_string()), tab_groups::PALETTE.to_vec()),
            "duplicate" => self.duplicate_group(group),
            "move-window" => self.move_group_to_new_window(group),
            "save" => self.save_group_arrangement(event_loop, group),
            "ungroup" => self.ungroup(group),
            "close" => self.close_group(group),
            "close-others" => self.close_tabs_outside_group(group),
            "close-right" => self.close_tabs_right_of_group(group),
            _ => {}
        }
    }

    /// New Tab to the Right (Below on a side bar) for the tab at `at`
    /// (`PT newTabToTheRight:`, `PT:14329-14343`): a tab of the default profile just after
    /// it, joining its group; the order then settles. In a tmux session it first asks what
    /// kind of tab (`AD newTabAtIndex:didMakeSession:`, `possiblyTmuxValueForWindow:`), in
    /// the alert queue: a new tmux tab opens where tmux places it, Use Default Profile opens
    /// this tab beside it (`tab_beside`), and Cancel opens nothing.
    fn new_tab_beside(&mut self, at: usize) {
        let Some(beside) = self.tabs.get(at).map(|t| t.id) else { return };
        if self.tmux_new_beside(false, true, Some(beside)).is_some() {
            return;
        }
        self.tab_beside(at);
    }

    /// New Tab to the Right's local tab: the default profile's tab just after the tab at
    /// `at`, joining its group, and selected; the order then settles.
    pub(super) fn tab_beside(&mut self, at: usize) {
        let group = self.tabs.get(at).and_then(|t| t.group.as_ref()).map(|g| g.id.clone()).filter(|g| !g.is_empty());
        let before = self.tabs.len();
        self.new_tab(None);
        let Some(new) = move_new_tab(&mut self.tabs, before, at) else { return };
        self.active = new;
        if group.is_some() {
            self.set_tab_group(new, group);
        }
        self.group_changed();
    }
}

/// The group close commands' question for the tabs `ids` of `window` (`PT:9899-9907`): "<question>
/// (N tabs)" under "Close Tabs", Close or Cancel, the persistent `NoSyncCloseTabGroup`. Closing
/// the alert presses Cancel.
pub(super) fn close_tabs_ask(window: WindowId, what: CloseTabs, vertical: bool, ids: Vec<u64>) -> super::scripts_alerts::Ask {
    let text = tab_groups::close_question(what, vertical, ids.len());
    super::scripts_alerts::ask(tab_groups::CLOSE_TABS_HEADING, &text, &tab_groups::CLOSE_TABS_BUTTONS, Some(tab_groups::CLOSE_TABS_ID), crate::warnings::Kind::Persistent, super::scripts_alerts::Then::CloseTabs { window, ids })
}

/// Collapse Group's plan (`PT collapseTabGroup:animated:`, `PT:9538-9580`) for `group` over
/// the window's group ids `order`, collapsed flags and selected tab `active`: the tab to
/// select first and a group to expand first, so the selected tab is never hidden. The
/// selected tab outside the group needs nothing; inside it, the nearest visible tab outside
/// is selected; when every tab outside is hidden in other collapsed groups, the nearest of
/// them is selected and its group expanded. None when refused: no such group, or the group
/// is the whole window.
pub(super) fn collapse_plan(order: &[Option<&str>], collapsed: &[bool], active: usize, group: &str) -> Option<(Option<usize>, Option<String>)> {
    if !order.contains(&Some(group)) {
        return None;
    }
    if order.get(active).copied().flatten() != Some(group) {
        return Some((None, None));
    }
    if let Some(outside) = tab_groups::nearest_outside(order, collapsed, group) {
        return Some((Some(outside), None));
    }
    if tab_groups::whole_window(order, group) {
        return None;
    }
    // Every tab outside is a hidden member of another collapsed group: land on the nearest
    // with no tab skipped, and expand its group.
    let hidden = tab_groups::nearest_outside(order, &[], group)?;
    Some((Some(hidden), order[hidden].map(str::to_string)))
}

/// `App::expand_selected_group`'s rule over a window's `tabs` with `active` selected: when
/// the selected tab is hidden in a collapsed group, every member of that group expands
/// (`PT expandTabGroupIfSelectedTabIsCollapsed`). True when a flag changed. Every path that
/// changes the selected tab without settling ends here, as a selection does in iTerm2
/// (`PT tabView:didSelectTabViewItem:`).
pub(super) fn expand_selected(tabs: &mut [Tab], active: usize) -> bool {
    let Some(group) = tabs.get(active).and_then(|t| t.group.as_ref()).filter(|g| g.collapsed).map(|g| g.id.clone()) else { return false };
    for g in tabs.iter_mut().filter_map(|t| t.group.as_mut()).filter(|g| g.id == group) {
        g.collapsed = false;
    }
    true
}

/// `App::settle_tabs`'s rule over a window's group ids, pinned and collapsed flags and
/// selected tab `active`: the canonical order (input indices; `tab_groups::canonical_order`),
/// each tab's collapsed flag in that order with every group one state and never the selected
/// tab's (`tab_groups::normalized_collapsed`), and the selected tab's index in that order.
pub(super) fn settle_plan(groups: &[Option<&str>], pinned: &[bool], collapsed: &[bool], active: usize) -> (Vec<usize>, Vec<bool>, usize) {
    let order = tab_groups::canonical_order(groups, pinned);
    let active = order.iter().position(|&i| i == active).unwrap_or(0);
    let groups: Vec<Option<&str>> = order.iter().map(|&i| groups[i]).collect();
    let flags: Vec<bool> = order.iter().map(|&i| collapsed.get(i).copied().unwrap_or(false)).collect();
    let flags = tab_groups::normalized_collapsed(&groups, &flags, active);
    (order, flags, active)
}

/// Where Duplicate Tab puts the copy of the tab at `source` among `tabs` tabs
/// (`PT createDuplicateOfTab:inTerminal:`, `PT:14436-14535`): just after the original
/// (Some(index)) in the same window when `DuplicatedTabsOpenAdjacentToOriginal` (`adjacent`)
/// is on and the tab is not a tmux tab, else at the end (None); and the group it keeps: the
/// original's in the same window, none in a new window.
pub(super) fn duplicate_placement(source: usize, tabs: usize, adjacent: bool, tmux: bool, same_window: bool, group: Option<&TabGroup>) -> (Option<usize>, Option<TabGroup>) {
    if !same_window {
        return (None, None);
    }
    let at = (adjacent && !tmux).then(|| (source + 1).min(tabs));
    (at, group.cloned())
}

/// What a pane's tab icon was worked out from (`PS tabGraphicForProfile:`): the profile's Icon
/// and Custom Icon Path, the job's lookup chain and fallback name, and
/// `DefaultIconsUsingLetters`. A change works the icon out again.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct IconKey {
    setting: i64,
    custom: String,
    chain: Vec<Vec<String>>,
    fallback: Option<String>,
    letters: bool,
}

/// A decoded icon picture: a bundled icon under its tint, a user PNG or a custom icon file.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum ImageId {
    Tinted(String, [u8; 3]),
    File(PathBuf),
}

/// Pictures read from files are kept at most this many pixels square: 16 points at twice
/// the size (`iTermCacheableImage imageAtPath:ofSize:`, 16 by 16).
const ICON_FILE_PX: u32 = 32;

/// `%APPDATA%\Tandem`, where the user's icon maps and PNGs live (iTerm2's Application Support).
fn icon_dir() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("Tandem")
}

/// A user file's bytes; None when it is absent, unreadable, not a regular file, larger than
/// `tab_icons::MAX_FILE_BYTES`, a device, or on the network or reached through a link to it
/// (never probed, `tab_icons::readable_path`). Only a regular file is opened, so a device or
/// pipe never blocks the window.
fn read_user_file(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    use crate::tab_icons::MAX_FILE_BYTES;
    if !crate::tab_icons::readable_path(path, &crate::scripts::read_link) || !std::fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return None;
    }
    // The file may grow after the size was read: never read past the cap.
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= MAX_FILE_BYTES).then_some(bytes)
}

/// The icon maps with the user's `graphic_icons.json` and `graphic_colors.json` merged over
/// the bundled ones (`GS reloadGraphicMaps`); `previous` is kept for a file that no longer
/// parses.
pub(super) fn user_tab_icon_maps(previous: Option<&crate::tab_icons::Maps>) -> crate::tab_icons::Maps {
    use crate::tab_icons::{COLORS_FILE, ICONS_FILE, load_maps};
    let dir = icon_dir();
    let icons = read_user_file(&dir.join(ICONS_FILE));
    let colors = read_user_file(&dir.join(COLORS_FILE));
    load_maps(icons.as_deref(), colors.as_deref(), previous)
}

/// A picture as the tab bar draws it, keyed for the texture cache apart from inline images
/// (small counters) and background images (top bit set).
fn picture(id: &ImageId, pixels: Vec<u8>, size: (u32, u32)) -> scene::TabImage {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    let key = (h.finish() >> 2) | (1 << 62);
    scene::TabImage::Picture { key, pixels: std::sync::Arc::new(pixels), size }
}

/// An image file decoded to RGBA within `tab_icons::decode_limited`'s bounds, shrunk to
/// `ICON_FILE_PX` square when larger; None when it cannot be read (`read_user_file`) or
/// decoded.
fn decode_icon_file(path: &Path) -> Option<(Vec<u8>, (u32, u32))> {
    let bytes = read_user_file(path)?;
    let mut img = crate::tab_icons::decode_limited(&bytes)?;
    if img.width() > ICON_FILE_PX || img.height() > ICON_FILE_PX {
        img = image::imageops::resize(&img, ICON_FILE_PX, ICON_FILE_PX, image::imageops::FilterType::Triangle);
    }
    let size = img.dimensions();
    Some((img.into_raw(), size))
}

impl App {
    /// Reads the user's icon maps again (`GS reloadGraphicMaps`; 13y2 Task 9a calls it after it
    /// pulls them): when the maps changed, every icon is worked out again and the bars redrawn.
    #[allow(dead_code)] // 13y2 Task 9a is its caller.
    pub(super) fn reload_tab_icon_maps(&mut self) {
        let maps = user_tab_icon_maps(Some(&self.tab_icon_maps));
        if maps == self.tab_icon_maps {
            return;
        }
        self.tab_icon_maps = maps;
        self.tab_icon_cache.get_mut().clear();
        self.tab_icon_images.get_mut().clear();
        self.redraw();
    }

    /// The icon picture a pane's tab shows (`PS tabGraphicForProfile:`): none, the job's
    /// built-in icon or letter (`GS imageForProcessID:`; a tmux pane by tmux's job name,
    /// `updateImageForJobName:`), or the custom image. Kept per pane until what it came from
    /// changes; a pane whose program ended keeps its last icon.
    pub(super) fn tab_icon_for(&self, pane: PaneId) -> Option<scene::TabImage> {
        use crate::tab_icons::{ancestor_chain, candidates, fallback_name};
        let p = self.panes.get(&pane)?;
        let profile = self.store.get(&p.profile)?;
        let (chain, fallback) = match p.tmux_io() {
            Some(io) => {
                let job = io.info.borrow().job.clone();
                if job.is_empty() { (Vec::new(), None) } else { (vec![candidates(&job, None, None)], Some(fallback_name(&job, None, None))) }
            }
            None => {
                let shell = &p.own().shell;
                match &shell.job {
                    Some(j) => {
                        let mut chain = vec![candidates(&j.name, Some(&j.title), Some(&j.cmdline))];
                        // The ancestors start with the job itself.
                        chain.extend(ancestor_chain(shell.job_ancestors.get(1..).unwrap_or_default()));
                        (chain, Some(fallback_name(&j.name, Some(&j.title), Some(&j.cmdline))))
                    }
                    None => (Vec::new(), None),
                }
            }
        };
        let key = IconKey { setting: profile.int("Icon"), custom: profile.string("Custom Icon Path"), chain, fallback, letters: self.globals.bool("DefaultIconsUsingLetters") };
        if let Some((k, image)) = self.tab_icon_cache.borrow().get(&pane) {
            if *k == key || p.exited.is_some() {
                return image.clone();
            }
        }
        let icon = crate::tab_icons::resolve(&self.tab_icon_maps, key.setting, &key.custom, &key.chain, key.fallback.as_deref(), key.letters, &|l| self.has_icon_picture(l));
        let image = match icon {
            crate::tab_icons::Icon::None => None,
            crate::tab_icons::Icon::Picture(logical, tint) => self.icon_picture(&logical, tint),
            crate::tab_icons::Icon::Letter(letter, color) => Some(scene::TabImage::Letter { letter, color }),
            crate::tab_icons::Icon::Custom(path) => self.icon_file(&path),
        };
        let mut cache = self.tab_icon_cache.borrow_mut();
        cache.insert(pane, (key, image.clone()));
        if cache.len() > self.panes.len() + 16 {
            cache.retain(|id, _| self.panes.contains_key(id));
        }
        image
    }

    /// Whether a logical name has a picture (`GS:380-388`): the bundled PNG, else the user's
    /// `graphic_<name>.png`.
    fn has_icon_picture(&self, logical: &str) -> bool {
        crate::tab_icons::png(logical).is_some() || (crate::tab_icons::plain_logical(logical) && self.icon_file(&crate::tab_icons::user_png_path(&icon_dir(), logical)).is_some())
    }

    /// A logical name's picture painted `tint` (`GS:380-402`), made once per logical name and
    /// tint; None when it has none.
    fn icon_picture(&self, logical: &str, tint: crate::palette::Rgb) -> Option<scene::TabImage> {
        let id = ImageId::Tinted(logical.to_string(), [tint.r, tint.g, tint.b]);
        if let Some(found) = self.tab_icon_images.borrow().get(&id) {
            return found.clone();
        }
        let source = match crate::tab_icons::png(logical) {
            Some(bytes) => image::load_from_memory(bytes).ok().map(|i| i.to_rgba8()).map(|i| (i.dimensions(), i.into_raw())),
            None if crate::tab_icons::plain_logical(logical) => match self.icon_file(&crate::tab_icons::user_png_path(&icon_dir(), logical)) {
                Some(scene::TabImage::Picture { pixels, size, .. }) => Some((size, pixels.to_vec())),
                _ => None,
            },
            None => None,
        };
        let image = source.map(|(size, pixels)| picture(&id, crate::tab_icons::tinted(&pixels, tint), size));
        self.tab_icon_images.borrow_mut().insert(id, image.clone());
        image
    }

    /// An image file as a picture (a custom icon, a user PNG), read once; None when it is
    /// missing, unreadable or on the network (`iTermCacheableImage` gives nil).
    fn icon_file(&self, path: &Path) -> Option<scene::TabImage> {
        let id = ImageId::File(path.to_path_buf());
        if let Some(found) = self.tab_icon_images.borrow().get(&id) {
            return found.clone();
        }
        let image = decode_icon_file(path).map(|(pixels, size)| picture(&id, pixels, size));
        self.tab_icon_images.borrow_mut().insert(id, image.clone());
        image
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Option<&str> = Some("a");
    const B: Option<&str> = Some("b");

    /// Close Tabs goes through the alert queue (13b): iTerm2's text and buttons, persistent;
    /// closing the alert presses Cancel; a second pick while it waits asks nothing more, and
    /// the question for other tabs still asks.
    #[test]
    fn close_tabs_question_is_an_alert() {
        use super::super::scripts_alerts::{on_close, waits, Answer, Then};
        let window = WindowId::from(4);
        let ask = close_tabs_ask(window, CloseTabs::Right, true, vec![7, 8]);
        assert_eq!((ask.spec.heading.as_str(), ask.spec.text.as_str()), ("Close Tabs", "Close all tabs below this group? (2 tabs)"));
        assert_eq!((ask.spec.labels.clone(), ask.spec.cancel), (vec!["Close".to_string(), "Cancel".to_string()], Some(1)));
        assert_eq!((ask.spec.id.as_deref(), ask.spec.kind), (Some("NoSyncCloseTabGroup"), crate::warnings::Kind::Persistent));
        let close = crate::prompt_ui::close_index(&["Close", "Cancel"]);
        assert_eq!(on_close(&ask.then, close), Answer::Button(1), "a close is Cancel, never Close");
        let mut queued = std::collections::VecDeque::new();
        queued.push_back(close_tabs_ask(window, CloseTabs::Right, true, vec![7, 8]));
        assert!(waits(None, &queued, |t| *t == ask.then));
        assert!(!waits(None, &queued, |t| *t == Then::CloseTabs { window, ids: vec![9] }));
    }

    #[test]
    fn collapse_follows_iterm2() {
        assert_eq!(collapse_plan(&[None, A, A], &[], 1, "a"), Some((Some(0), None)));
        assert_eq!(collapse_plan(&[A, A], &[], 0, "a"), None, "the whole window");
        assert_eq!(collapse_plan(&[B, A], &[true, false], 1, "a"), Some((Some(0), Some("b".into()))), "every tab outside is hidden in b: land on it, expanding b first");
        assert_eq!(collapse_plan(&[None, A], &[], 0, "a"), Some((None, None)), "the selected tab is outside");
        assert_eq!(collapse_plan(&[None, A], &[], 0, "z"), None, "no such group");
    }

    /// Each path that inserts or moves a tab ends with `settle_tabs`; here the orders those
    /// paths leave, settled.
    #[test]
    fn every_insertion_settles() {
        const N: Option<&str> = None;
        let settle = |g: &[Option<&str>], p: &[bool], active: usize| {
            let (order, _, active) = settle_plan(g, p, &[], active);
            (order, active)
        };
        // New Tab beside the first of two pinned tabs (`move_new_tab` puts it at 1): it goes
        // after the pinned ones, and stays selected.
        assert_eq!(settle(&[N, N, N, N], &[true, false, true, false], 1), (vec![0, 2, 1, 3], 2));
        // New Tab inside a group, beside a member that is not the last: it joined the group
        // (`groupIDToInheritForNewTabAtIndex:`), so nothing moves.
        assert_eq!(settle(&[A, A, A], &[false; 3], 1), (vec![0, 1, 2], 1));
        // The API moving an unpinned tab in front of a pinned one.
        assert_eq!(settle(&[N, N], &[false, true], 0), (vec![1, 0], 1));
        // Merge All Windows appending another window's pinned tab and a split group.
        assert_eq!(settle(&[N, B, N, B], &[false, false, true, false], 3), (vec![2, 0, 1, 3], 3));
        // A loaded recording's tab inserted at 0 in front of a pinned tab.
        assert_eq!(settle(&[N, N, N], &[false, true, false], 0), (vec![1, 0, 2], 1));
        // Undo Close putting a grouped tab back away from its group.
        assert_eq!(settle(&[A, N, A], &[false; 3], 2), (vec![0, 2, 1], 1));
        // A collapsed group holding the selected tab expands; another stays collapsed.
        let (_, flags, _) = settle_plan(&[A, A, B, B], &[false; 4], &[true, true, true, true], 0);
        assert_eq!(flags, vec![false, false, true, true]);
    }

    /// Paths that pick the selected tab without settling (Bury, a drag out or tear-off, the
    /// kept tabs of Merge All Windows, Unbury and Undo Close into a tab through `select_tab`)
    /// expand the group it is hidden in (`PT expandTabGroupIfSelectedTabIsCollapsed`).
    #[test]
    fn a_selected_tab_never_hides() {
        let tab = |id: u64, group: Option<(&str, bool)>| {
            let mut t = Tab::new(id, Layout::new(id), id);
            t.group = group.map(|(g, collapsed)| TabGroup { id: g.into(), name: "Build".into(), color: tab_groups::PALETTE[0], collapsed });
            t
        };
        let collapsed = |tabs: &[Tab]| tabs.iter().map(|t| t.group.as_ref().is_some_and(|g| g.collapsed)).collect::<Vec<_>>();
        // [A, B | G1, G2 (collapsed)] with B selected; Bury takes B and its neighbor G1 is
        // selected: the group expands.
        let mut tabs = vec![tab(1, None), tab(2, None), tab(3, Some(("g", true))), tab(4, Some(("g", true))), tab(5, Some(("h", true))), tab(6, Some(("h", true)))];
        tabs.remove(1);
        let active = crate::close_rules::active_after_close(1, 1, tabs.len(), false);
        assert_eq!(active, 1);
        assert!(expand_selected(&mut tabs, active));
        assert_eq!(collapsed(&tabs), vec![false, false, false, true, true], "only the selected tab's group expands");
        // A selected tab already shown, or in no group, changes nothing.
        assert!(!expand_selected(&mut tabs, 1));
        assert!(!expand_selected(&mut tabs, 0));
        assert!(!expand_selected(&mut tabs, 9));
        assert_eq!(collapsed(&tabs), vec![false, false, false, true, true]);
    }

    #[test]
    fn tab_icon_files_follow_iterm2() {
        let dir = std::env::temp_dir().join(format!("tandem-tab-icons-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let save = |name: &str, side: u32| {
            let path = dir.join(name);
            image::RgbaImage::from_pixel(side, side, image::Rgba([1, 2, 3, 200])).save(&path).unwrap();
            path
        };
        // A large custom icon is shrunk to 16 points at twice the size; a small one is kept.
        let (pixels, size) = decode_icon_file(&save("big.png", 64)).expect("big");
        assert_eq!((size, pixels.len()), ((32, 32), 32 * 32 * 4));
        assert_eq!(decode_icon_file(&save("small.png", 8)).expect("small").1, (8, 8));
        // Missing, not a picture, or on the network: nothing (`iTermCacheableImage` gives nil).
        assert_eq!(decode_icon_file(&dir.join("missing.png")), None);
        std::fs::write(dir.join("text.png"), b"not a picture").unwrap();
        assert_eq!(decode_icon_file(&dir.join("text.png")), None);
        assert_eq!(decode_icon_file(Path::new(r"\\server\share\i.png")), None);
        assert_eq!(read_user_file(Path::new(r"\\?\UNC\server\share\graphic_icons.json")), None);
        assert_eq!(read_user_file(Path::new(r"\/server/share/graphic_icons.json")), None);
        // A device or a folder is never opened, only a regular file (m13d ruling).
        for p in ["NUL", "nul.png", "CON", "COM1", "LPT1.png", r"\\.\NUL", r"\\.\COM1"] {
            assert_eq!(read_user_file(Path::new(p)), None, "{p}");
        }
        assert_eq!(read_user_file(&dir.join("NUL.png")), None);
        assert_eq!(read_user_file(&dir), None, "a folder");
        // A file past the size cap is not read; one at the cap is.
        let at_cap = dir.join("cap.bin");
        std::fs::write(&at_cap, vec![0u8; crate::tab_icons::MAX_FILE_BYTES as usize]).unwrap();
        assert_eq!(read_user_file(&at_cap).map(|b| b.len() as u64), Some(crate::tab_icons::MAX_FILE_BYTES));
        std::fs::write(&at_cap, vec![0u8; crate::tab_icons::MAX_FILE_BYTES as usize + 1]).unwrap();
        assert_eq!(read_user_file(&at_cap), None);
        // A picture wider than the decode limit draws nothing.
        let wide = dir.join("wide.png");
        image::RgbaImage::new(crate::tab_icons::MAX_DECODE_SIDE + 1, 1).save(&wide).unwrap();
        assert_eq!(decode_icon_file(&wide), None);
        // Tinted and file pictures are keyed apart from inline and background images.
        let key = |id: &ImageId| match picture(id, vec![0; 4], (1, 1)) {
            scene::TabImage::Picture { key, .. } => key,
            _ => unreachable!(),
        };
        for id in [ImageId::Tinted("git".into(), [1, 2, 3]), ImageId::File(dir.join("big.png"))] {
            assert_eq!(key(&id) >> 62, 1, "{id:?}");
        }
        assert_ne!(key(&ImageId::Tinted("git".into(), [1, 2, 3])), key(&ImageId::Tinted("git".into(), [1, 2, 4])));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn duplicate_tab_follows_iterm2() {
        let g = TabGroup { id: "g".into(), name: "Build".into(), color: tab_groups::PALETTE[2], collapsed: false };
        assert_eq!(duplicate_placement(1, 4, true, false, true, Some(&g)), (Some(2), Some(g.clone())));
        assert_eq!(duplicate_placement(1, 4, false, false, true, None), (None, None));
        assert_eq!(duplicate_placement(1, 4, true, true, true, None), (None, None), "tmux tabs append");
        assert_eq!(duplicate_placement(1, 4, true, false, false, Some(&g)), (None, None), "a new window drops the group");
    }
}

//! Dragging tabs (milestone 13d): a left press on a tab becomes a drag once the pointer moves
//! `MinimumTabDragDistance` pixels (`PSM:3775`, `:3814`); the tab then follows the pointer
//! along its bar over a drop marker, and the release reorders it in its own bar, moves it
//! into another window's bar, or tears it off into a new window at the drop point
//! (`PT shouldDropTabViewItem:`, `PT:7653-7670`). A tab dropped on a collapsed group's chip
//! joins that group, which is framed in the accent color meanwhile
//! (`collapsedGroupJoinTargetIdentifier`). A press on a group chip drags the whole group
//! (`PSM:3763-3786`): to another place in its bar, another window, or a new window. A press
//! on the "+" button that moves as far drags the window instead (`AllowDragOnAddTabButton`,
//! `PT:10259-10261`). The rules are `crate::tab_drag`'s.

use super::*;
use crate::tab_drag::{Arm, Drop, Target};
use crate::tab_groups;
use crate::winmodel::LayoutOp;

impl App {
    /// A left press on the tab bar may start a drag (`tab_drag::press_arm`): on a tab unless
    /// Lock Layout forbids reordering (no beep: the press still selects, `PT:7610-7616`); on a
    /// group chip, whose drop Lock Layout refuses instead (`PT:7781`, `:7813`); on the "+" button
    /// with `AllowDragOnAddTabButton`. Anywhere else on the bar (a close button, the empty bar,
    /// a locked tab) only the window's drag is armed, which Alt, the edge strip or a solitary tab
    /// converts on the move (`PSM:3788-3794`, asked before any cell is looked up).
    pub(super) fn tab_drag_press(&mut self, hit: tab_layout::Hit) {
        let start = self.cursor;
        self.tab_drop_marker = None;
        let reorder = matches!(hit, tab_layout::Hit::Tab(_)) && self.layout_allowed_quiet(LayoutOp::ReorderTab);
        let add_drags = self.globals.bool("AllowDragOnAddTabButton") && self.new_tab_button_rect().is_some_and(|r| r.contains(start.0, start.1));
        match (tab_drag::press_arm(hit, reorder, add_drags), hit) {
            (Arm::Tab, tab_layout::Hit::Tab(from)) => {
                // Called before the press selects the tab: this is the selection before it.
                let before = self.tabs.get(self.active).map(|t| t.id);
                self.drag = Drag::Tab { from, start, began: false, before };
            }
            (Arm::Chip, _) => {
                if let Some(group) = self.bar_chip_at(start.0, start.1) {
                    self.drag = Drag::Chip { group, start, began: false };
                }
            }
            (Arm::AddButton, _) => self.drag = Drag::AddButton { start },
            (Arm::Window, _) => self.drag = Drag::Window { start },
            _ => {}
        }
    }

    /// The size in pixels of the current window's edge strip that drags it (`caption::edge_strip`
    /// of the theme's key, times the scale); 0 outside Minimal and Compact and while the window
    /// is full screen or maximized, and in a window type that cannot be dragged (Centered).
    pub(super) fn edge_strip_px(&self) -> f32 {
        let number = |key: &str, default: f32| self.globals.number(key).map_or(default, |v| v as f32);
        // Centered and edge-docked windows are not draggable (`PT:2106-2135`, `:10127-10129`).
        let draggable = crate::caption::window_movable(self.win_style, self.full_screen()) && !self.gpu.as_ref().is_some_and(|g| g.window.is_maximized());
        let strip = crate::caption::edge_strip(self.tab_style(), number("MinimalEdgeDragSize", 12.0), number("CompactEdgeDragSize", 10.0), draggable);
        if self.compact_window() { strip.max(0.0) * self.scale() } else { 0.0 }
    }

    /// Whether the drag of a tab that just began (or of a press that only a window drag can
    /// take, `Drag::Window`), with the pointer now at the window point `at`, moves the window
    /// instead (`caption::drag_to_window`, judged on the move only, `PSM:3788-3794`): Alt held
    /// now, the window's theme, its No Title Bar type, its tabs (collapsed group members count;
    /// only the overflow menu stops the count, `numberOfVisibleTabs`) and whether the drag event
    /// is in the edge strip (`event.locationInWindow`, `PT:10232-10233`) decide it. A group
    /// chip's drag is never asked (`PSM:3766-3795`).
    pub(super) fn tab_drag_moves_window(&self, at: (f32, f32)) -> bool {
        use crate::frames;
        let no_title_bar = matches!(frames::unthemed(self.win_style), frames::NO_TITLE_BAR | frames::CENTERED_NO_TITLE_BAR);
        let visible = crate::caption::visible_tab_count(self.tabs.len(), self.tab_bar.hidden.iter().copied().min());
        let in_strip = self.tab_bar_rect().is_some_and(|b| crate::caption::in_edge_strip(b, self.tab_position(), self.edge_strip_px(), at.0, at.1));
        let convert = self.globals.bool("ConvertTabDragToWindowDragForSolitaryTabInCompactOrMinimalTheme");
        crate::caption::drag_to_window(self.mods.alt, self.compact_window(), no_title_bar, visible, in_strip, convert)
    }

    /// Hands the pointer to the OS to move the window; the press that led here is forgotten (it
    /// selects, closes or drops nothing after the OS loop returns), as `add_button_drag` does.
    /// A window that cannot be dragged (`caption::window_movable`: Centered, docked to an edge,
    /// full screen) moves nothing, and the press is forgotten all the same (`iTermWindowImpl.m:163-167`).
    pub(super) fn begin_window_drag(&mut self) {
        self.drag = Drag::None;
        self.tab_press = None;
        self.end_tab_drag();
        self.drag_window_if_movable();
        self.redraw();
    }

    /// The OS moves the window with the pointer, where the window may be dragged.
    fn drag_window_if_movable(&self) {
        if !crate::caption::window_movable(self.win_style, self.full_screen()) {
            return;
        }
        if let Some(g) = &self.gpu {
            let _ = g.window.drag_window();
        }
    }

    /// A tab drag, or a group drag by its chip, has begun (the pointer moved far enough from
    /// the press).
    pub(super) fn tab_drag_began(&self) -> bool {
        matches!(self.drag, Drag::Tab { began: true, .. } | Drag::Chip { began: true, .. })
    }

    /// The pointer moved during a press on a tab or a group chip: the drag begins at
    /// `MinimumTabDragDistance`, then the drop marker follows the slot under the pointer in
    /// the own bar and the tab (or the group) is drawn at the pointer. Over a collapsed
    /// group's chip, in this bar or another window's, a dragged tab would join the group,
    /// which is framed instead. A press that went (the tab moved away under it) ends it.
    pub(super) fn tab_drag_move(&mut self, x: f32, y: f32) {
        // A press that can only move the window: asked again at each move (Alt may come later).
        if let Drag::Window { start } = self.drag {
            if self.tab_press.is_none() {
                return self.end_tab_drag();
            }
            if tab_drag::began(start, (x, y), self.min_tab_drag) && self.tab_drag_moves_window((x, y)) {
                self.hide_tab_tip();
                self.begin_window_drag();
            }
            return;
        }
        let (Drag::Tab { start, began, .. } | Drag::Chip { start, began, .. }) = &mut self.drag else { return };
        let (start, was) = (*start, *began);
        if self.tab_press.is_none() || self.tab_press_moved() {
            self.end_tab_drag();
            return self.redraw();
        }
        if !was {
            if !tab_drag::began(start, (x, y), self.min_tab_drag) {
                return;
            }
            // Minimal and Compact windows (and No Title Bar ones) move instead of tearing off.
            if matches!(self.drag, Drag::Tab { .. }) && self.tab_drag_moves_window((x, y)) {
                self.hide_tab_tip();
                return self.begin_window_drag();
            }
            if let Drag::Tab { began, .. } | Drag::Chip { began, .. } = &mut self.drag {
                *began = true;
            }
            self.hide_tab_tip();
        }
        let join = match &self.drag {
            Drag::Chip { group, .. } => {
                let group = group.clone();
                self.tab_drop_marker = self.own_bar_group_slot(&group, x, y);
                None
            }
            Drag::Tab { from, .. } => match self.own_bar_drop(x, y, *from) {
                Some(Drop::Slot(slot)) => {
                    self.tab_drop_marker = Some(slot);
                    None
                }
                Some(Drop::Join(group)) => {
                    self.tab_drop_marker = None;
                    self.current_window().map(|w| (w, group))
                }
                None => {
                    self.tab_drop_marker = None;
                    match cursor_screen().and_then(|p| self.window_bar_drop(p)) {
                        Some((k, Drop::Join(group))) => self.window_id_of_key(k).map(|w| (w, group)),
                        _ => None,
                    }
                }
            },
            _ => None,
        };
        self.set_tab_join(join);
        self.redraw();
    }

    /// The press that dragged a tab or a group came up: a tab drops where `tab_drag::target`
    /// says, a group where `tab_drag::group_target` says, with the pointer's screen point for
    /// another window's bar or a new window.
    pub(super) fn tab_drag_release(&mut self, event_loop: &ActiveEventLoop) {
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        self.end_tab_drag();
        match drag {
            Drag::Tab { from, began: true, before, .. } => self.drop_tab(from, before, event_loop),
            Drag::Chip { group, began: true, .. } => self.drop_group(&group, event_loop),
            _ => {}
        }
        self.redraw();
    }

    /// A dragged tab's release: in its own bar it moves, or joins the collapsed group whose
    /// chip is under it; in another window's bar it moves there (joining a collapsed group
    /// there the same way); outside every bar it tears off, unless it is pinned. `before` is
    /// this window's selected tab before the press.
    fn drop_tab(&mut self, from: usize, before: Option<u64>, event_loop: &ActiveEventLoop) {
        if from >= self.tabs.len() {
            return;
        }
        let (x, y) = self.cursor;
        let own = self.own_bar_drop(x, y, from);
        let screen = cursor_screen();
        let other = if own.is_none() { screen.and_then(|p| self.window_bar_drop(p)) } else { None };
        // A join has no slot of its own; the target rules only need where the pointer is.
        let slot_of = |d: &Drop| match d {
            Drop::Slot(slot) => *slot,
            Drop::Join(_) => from,
        };
        let own_slot = own.as_ref().map(slot_of);
        let other_slot = other.as_ref().map(|(k, d)| (*k, slot_of(d)));
        match tab_drag::target(own_slot, other_slot, self.tabs.len(), self.tabs[from].pinned, self.globals.bool("AllowDragOfTabIntoNewWindow")) {
            Target::OwnBar(slot) => match own {
                Some(Drop::Join(group)) => self.join_in_own_bar(from, group, before),
                _ => self.drop_in_own_bar(from, slot),
            },
            Target::OtherWindow(window, slot) => {
                let join = match other {
                    Some((_, Drop::Join(group))) => Some(group),
                    _ => None,
                };
                self.drop_in_window(from, window, slot, join, before);
            }
            Target::NewWindow => {
                if let Some(p) = screen {
                    self.tear_off(from, p, event_loop);
                }
            }
            Target::Nowhere => {}
        }
    }

    /// Another window's tab bar under the screen point `screen` (the topmost window there),
    /// and the slot a tab dropped there takes; None over no other window's bar.
    pub(super) fn window_bar_hit(&self, screen: (i32, i32)) -> Option<(u64, usize)> {
        let vertical = tab_position::vertical(self.tab_position());
        let (k, x, y) = self.window_bar_point(screen)?;
        let (_, rects, _) = self.parked_bar(k)?;
        Some((k, tab_drag::drop_index(rects, x, y, vertical)))
    }

    /// Another window's tab bar under the screen point `screen`, and where a dragged tab drops
    /// there: a slot, or the collapsed group whose chip is under it (`tab_drag::tab_drop`).
    fn window_bar_drop(&self, screen: (i32, i32)) -> Option<(u64, Drop)> {
        let vertical = tab_position::vertical(self.tab_position());
        let (k, x, y) = self.window_bar_point(screen)?;
        let (_, rects, pills) = self.parked_bar(k)?;
        Some((k, tab_drag::tab_drop(rects, pills, x, y, vertical)))
    }

    /// The parked window `k`'s tab bar as last drawn.
    fn parked_bar(&self, k: u64) -> Option<&(Rect, Vec<Rect>, Vec<tab_drag::Pill>)> {
        self.windows.parked().find(|(key, _)| **key == k).and_then(|(_, s)| s.tab_bar_drawn.as_ref())
    }

    /// Another window's tab bar under the screen point `screen` (the topmost window there):
    /// its registry key and the point in its client pixels; None over no other window's bar.
    fn window_bar_point(&self, screen: (i32, i32)) -> Option<(u64, f32, f32)> {
        use windows_sys::Win32::Foundation::{HWND, POINT};
        use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetAncestor, WindowFromPoint, GA_ROOT};
        let under = unsafe { GetAncestor(WindowFromPoint(POINT { x: screen.0, y: screen.1 }), GA_ROOT) } as isize;
        if under == 0 {
            return None;
        }
        self.windows.parked().find_map(|(k, s)| {
            let hwnd = s.gpu.as_ref().and_then(|g| super::menu::hwnd_of(&g.window)).filter(|h| *h == under)?;
            let (bar, _, _) = s.tab_bar_drawn.as_ref()?;
            let mut p = POINT { x: screen.0, y: screen.1 };
            if unsafe { ScreenToClient(hwnd as HWND, &mut p) } == 0 {
                return None;
            }
            let (x, y) = (p.x as f32, p.y as f32);
            bar.contains(x, y).then_some((*k, x, y))
        })
    }

    /// Ends a press on a tab, a chip or the "+" (dropped, cancelled, or never dragged): no
    /// marker, and no group framed as a join target.
    pub(super) fn end_tab_drag(&mut self) {
        if matches!(self.drag, Drag::Tab { .. } | Drag::Chip { .. } | Drag::AddButton { .. } | Drag::Window { .. }) {
            self.drag = Drag::None;
        }
        self.tab_drop_marker = None;
        self.set_tab_join(None);
    }

    /// The collapsed group a dragged tab would join changes: the windows showing the old and
    /// the new one redraw.
    fn set_tab_join(&mut self, join: Option<(WindowId, String)>) {
        if join == self.tab_join {
            return;
        }
        for (w, _) in [self.tab_join.take(), join.clone()].into_iter().flatten() {
            self.redraw_window(w);
        }
        self.tab_join = join;
    }

    /// Asks the window `id` (this one or a parked one) to draw again.
    fn redraw_window(&self, id: WindowId) {
        if self.current_window() == Some(id) {
            return self.redraw();
        }
        if let Some(g) = self.windows.parked().find_map(|(_, s)| s.gpu.as_ref().filter(|g| g.window.id() == id)) {
            g.window.request_redraw();
        }
    }

    /// The pointer moved during a press on the "+": once it passes `MinimumTabDragDistance`
    /// the window moves with it (where it may be dragged) and the press is forgotten, so no tab
    /// opens.
    pub(super) fn add_button_drag(&mut self, start: (f32, f32), x: f32, y: f32) {
        if !tab_drag::began(start, (x, y), self.min_tab_drag) {
            return;
        }
        self.drag = Drag::None;
        self.tab_press = None;
        self.drag_window_if_movable();
        self.redraw();
    }

    /// Where the tab at `from`, dropped at the window point (`x`, `y`), goes in this window's
    /// tab bar: a slot, or the collapsed group whose chip is under it (the dragged tab is a
    /// gap between pills); None off the bar.
    fn own_bar_drop(&self, x: f32, y: f32, from: usize) -> Option<Drop> {
        self.tab_bar_rect().filter(|b| b.contains(x, y))?;
        Some(tab_drag::tab_drop(&self.bar_tab_rects(), &self.bar_pills(Some(from)), x, y, tab_position::vertical(self.tab_position())))
    }

    /// The slot the group `group` dragged by its chip drops at under the window point
    /// (`x`, `y`) in this window's tab bar, never inside another group
    /// (`tab_drag::group_drop_slot`); None off the bar.
    fn own_bar_group_slot(&self, group: &str, x: f32, y: f32) -> Option<usize> {
        self.tab_bar_rect().filter(|b| b.contains(x, y))?;
        let (rects, vertical) = (self.bar_tab_rects(), tab_position::vertical(self.tab_position()));
        let slot = tab_drag::drop_index(&rects, x, y, vertical);
        Some(tab_drag::group_drop_slot(&self.group_order(), group, &rects, slot, x, y, vertical))
    }

    /// A tab dropped on a collapsed group's chip in its own bar joins that group where the tab
    /// is (the drop gap stays at the dragged tab's origin, `PSMTabDragAssistant:2045-2050`);
    /// the order then settles, which brings it next to the group's other tabs. A tab that
    /// was not selected before the press (`before`, the press itself selected it) gives the
    /// selection back to that tab, so the group stays collapsed; the tab that was selected
    /// stays selected and expands it (`tab_drag::keep_collapsed`).
    fn join_in_own_bar(&mut self, from: usize, group: String, before: Option<u64>) {
        let Some(dropped) = self.tabs.get(from).filter(|t| t.group.as_ref().is_none_or(|g| g.id != group)).map(|t| t.id) else { return };
        self.set_tab_group(from, Some(group));
        if let Some(i) = self.kept_collapsed(dropped, before, before) {
            self.select_tab(i);
        }
        self.group_changed();
    }

    /// `tab_drag::keep_collapsed` for the tab `dropped` just joined to a group in this
    /// window: the index of the tab to select instead of it, if any.
    fn kept_collapsed(&self, dropped: u64, before: Option<u64>, restore: Option<u64>) -> Option<usize> {
        let hidden_at = |i: usize| self.tabs[i].group.as_ref().is_some_and(|g| g.collapsed);
        let index = |id: u64| self.tabs.iter().position(|t| t.id == id);
        let dropped_hidden = index(dropped).is_some_and(hidden_at);
        let shown = |id: u64| index(id).is_some_and(|i| !hidden_at(i));
        tab_drag::keep_collapsed(dropped, dropped_hidden, before, restore, &shown).and_then(index)
    }

    /// A drop in the tab's own bar at `slot`: it moves there (`slot` counts the tab itself),
    /// kept in its pinned or unpinned zone (`PT moveTabAtIndex:toIndex:`). A drop that leaves
    /// it where it was changes nothing.
    fn drop_in_own_bar(&mut self, from: usize, slot: usize) {
        let to = if slot > from { slot - 1 } else { slot };
        let pinned: Vec<bool> = self.tabs.iter().map(|t| t.pinned).collect();
        if tab_drag::clamp_to_zone(from, to.min(self.tabs.len() - 1), &pinned) == from {
            return;
        }
        self.place_dropped_tab(from, to);
        self.tmux_reorder();
        self.relayout();
    }

    /// Moves the tab at `from` to `to`, kept in its zone (`tab_drag::clamp_to_zone`), then
    /// its group follows where it landed (`PT resolveDroppedTabGroupMembership:`,
    /// `tab_groups::resolved_group`, then `reconcile`) and the order settles
    /// (`PT tabsDidReorder`). The selected tab stays selected. Returns the tab's id.
    fn place_dropped_tab(&mut self, from: usize, to: usize) -> u64 {
        let pinned: Vec<bool> = self.tabs.iter().map(|t| t.pinned).collect();
        let to = tab_drag::clamp_to_zone(from, to.min(self.tabs.len() - 1), &pinned);
        let active = self.tabs.get(self.active).map(|t| t.id);
        let tab = self.tabs.remove(from);
        let id = tab.id;
        self.tabs.insert(to, tab);
        self.resolve_dropped_group(to);
        if let Some(a) = active.and_then(|a| self.tabs.iter().position(|t| t.id == a)) {
            self.active = a;
        }
        self.settle_tabs();
        id
    }

    /// The group of the tab just dropped at `at` follows from its neighbors
    /// (`tab_groups::resolved_group`): strictly inside a group's run it joins that group and
    /// takes its definition and pin state (`tab_groups::reconcile`); anywhere else it leaves
    /// any group. Only the dropped tab changes.
    fn resolve_dropped_group(&mut self, at: usize) {
        let resolved = {
            let order: Vec<Option<&str>> = self.tabs.iter().map(|t| t.group.as_ref().map(|g| g.id.as_str())).collect();
            tab_groups::resolved_group(at, &order)
        };
        let Some(tab) = self.tabs.get(at) else { return };
        if resolved.as_deref() == tab.group.as_ref().map(|g| g.id.as_str()) {
            return;
        }
        self.set_tab_group(at, resolved);
    }

    /// A drop in another window's bar (`window`, its registry key) at `slot`: the tab leaves
    /// this window (which closes when it was its last tab), goes into that one kept in its
    /// zone, takes its group there, and is selected in the window, which comes to the front
    /// (`PT didDonateTab:`). Dropped on a collapsed group's chip (`join`) it joins that group
    /// as its first tab; a tab that was not its window's selected tab before the press
    /// (`before`) then leaves the group collapsed and the window's selection as it was
    /// (`tab_drag::keep_collapsed`).
    /// A tab holding a loaded recording never moves, and a window under Lock Layout takes no
    /// tab. A tmux tab moves whole; its session hears which tmux windows share each Tandem
    /// window (`saveAffinities`), as the API's ReorderTabs does.
    fn drop_in_window(&mut self, from: usize, window: u64, slot: usize, join: Option<String>, before: Option<u64>) {
        if self.tabs.get(from).is_none_or(|t| self.holds_recording(t)) {
            return;
        }
        let Some(dest) = self.window_id_of_key(window) else { return };
        if self.windows.parked().any(|(k, s)| *k == window && s.locks().forbids(LayoutOp::AddTab)) {
            return;
        }
        let moving = self.take_tab(from);
        let gateway = self.tmux_of(moving.focused).map(|(gw, _, _)| gw);
        // Its browser views move to `dest` before this window can close (final review I4).
        self.browser_views_follow(&moving.layout.panes(), dest);
        if self.tabs.is_empty() {
            self.drop_current_window();
        } else {
            self.active = crate::close_rules::active_after_close(self.active, from, self.tabs.len(), self.globals.bool("moveLeftAfterClosingTab"));
            // The neighbor selected may be hidden in a collapsed group.
            self.expand_selected_group();
            self.tmux_reorder();
            self.relayout();
            self.redraw();
        }
        // `dest` is a terminal window, so the tab always lands there.
        self.enter(dest);
        let dest_selected = self.tabs.get(self.active).map(|t| t.id);
        let join = join.and_then(|g| self.members(&g).first().map(|&at| (g, at)));
        let slot = join.as_ref().map_or(slot, |(_, at)| *at);
        self.tabs.push(moving);
        let id = self.place_dropped_tab(self.tabs.len() - 1, slot);
        let mut kept = None;
        if let Some((group, _)) = join {
            if let Some(i) = self.tabs.iter().position(|t| t.id == id) {
                self.set_tab_group(i, Some(group));
            }
            kept = self.kept_collapsed(id, before, dest_selected);
            if let Some(i) = kept {
                self.active = i;
            }
            self.settle_tabs();
        }
        if let (None, Some(i)) = (kept, self.tabs.iter().position(|t| t.id == id)) {
            self.select_tab(i);
        }
        self.tmux_reorder();
        self.relayout();
        self.focus_terminal_window(dest);
        self.redraw();
        if let Some(gw) = gateway {
            self.tmux_save_affinities(gw);
        }
    }

    /// A drop outside every tab bar: the tab leaves its group (`PT:8538-8551`) and opens in a
    /// new window whose top-left corner is the drop point `screen`, which comes to the front;
    /// a window whose last tab left closes. A tab holding a loaded recording never moves (Move
    /// Tab to New Window's rule). A tmux tab moves whole, as to another window.
    fn tear_off(&mut self, from: usize, screen: (i32, i32), event_loop: &ActiveEventLoop) {
        if self.tabs.get(from).is_none_or(|t| self.holds_recording(t)) {
            return;
        }
        let Some(source) = self.current_window() else { return };
        let mut tab = self.take_tab(from);
        tab.group = None;
        let gateway = self.tmux_of(tab.focused).map(|(gw, _, _)| gw);
        if !self.tabs.is_empty() {
            self.active = crate::close_rules::active_after_close(self.active, from, self.tabs.len(), self.globals.bool("moveLeftAfterClosingTab"));
            self.expand_selected_group();
        }
        // Opened now, not with the pending windows, so a window whose only tab left closes
        // after its tab has a window (a failed open puts the tab back here).
        self.open_pending(event_loop, windows_ui::NewWindow::TornOff(tab, screen));
        let Some(new) = self.current_window().filter(|w| *w != source) else { return };
        if self.enter(source) {
            if self.tabs.is_empty() {
                self.drop_current_window();
            } else {
                self.tmux_reorder();
                self.relayout();
                self.redraw();
            }
        }
        self.focus_terminal_window(new);
        if let Some(gw) = gateway {
            self.tmux_save_affinities(gw);
        }
    }

    /// A group dragged by its chip came up (`PSMTabDragAssistant performGroupDropOntoBar`,
    /// `draggedImageEndedAt:operation:`): in its own bar it moves as a block, in another
    /// window's bar it moves there, outside every bar it tears off into a new window.
    fn drop_group(&mut self, group: &str, event_loop: &ActiveEventLoop) {
        let members = self.members(group);
        let Some(&first) = members.first() else { return };
        let (x, y) = self.cursor;
        let own = self.own_bar_group_slot(group, x, y);
        let screen = cursor_screen();
        let other = if own.is_none() { screen.and_then(|p| self.window_bar_point(p)) } else { None };
        let first_pinned = self.tabs[first].pinned;
        match tab_drag::group_target(own, other.map(|(k, _, _)| (k, 0)), self.tabs.len(), first_pinned, self.globals.bool("AllowDragOfTabIntoNewWindow")) {
            Target::OwnBar(slot) => self.reorder_group(group, slot),
            Target::OtherWindow(window, _) => {
                if let Some((_, px, py)) = other {
                    self.move_group_to_window(group, window, (px, py));
                }
            }
            Target::NewWindow => {
                if let Some(p) = screen {
                    self.tear_off_group(group, p, event_loop);
                }
            }
            Target::Nowhere => {}
        }
    }

    /// The group's tabs move together to just before the first other tab at or after `slot`
    /// (`PT reorderGroupTabs:beforeTabViewItem:`, `tab_drag::group_order`); membership and a
    /// collapsed state stay, the selected tab stays selected, and the order settles. Lock
    /// Layout refuses (`PT:7781`).
    fn reorder_group(&mut self, group: &str, slot: usize) {
        if !self.layout_allowed_quiet(LayoutOp::ReorderTab) {
            return;
        }
        let order = tab_drag::group_order(&self.group_order(), group, slot);
        if order.iter().enumerate().all(|(k, i)| k == *i) {
            return;
        }
        self.apply_order(&order);
        self.settle_tabs();
        self.tmux_reorder();
        self.relayout();
        self.redraw();
    }

    /// Takes the group's tabs out of this window, in order, for a move elsewhere: the
    /// selection stays, or goes to the nearest tab outside the group. None when a tab holds a
    /// loaded recording or Lock Layout forbids it (`PT:7781`, `:7813`). Also answers each tmux
    /// session the tabs belong to.
    pub(super) fn take_group(&mut self, group: &str) -> Option<(Vec<Tab>, Vec<PaneId>)> {
        let members = self.members(group);
        if members.is_empty() || members.iter().any(|&i| self.holds_recording(&self.tabs[i])) || !self.layout_allowed_quiet(LayoutOp::MoveTab) {
            return None;
        }
        let stay = if members.contains(&self.active) { tab_groups::nearest_outside(&self.group_order(), &[], group) } else { Some(self.active) };
        let stay = stay.and_then(|i| self.tabs.get(i)).map(|t| t.id);
        let mut moving = Vec::new();
        for id in self.tab_ids(&members) {
            if let Some(i) = self.tabs.iter().position(|t| t.id == id) {
                moving.push(self.take_tab(i));
            }
        }
        self.active = stay.and_then(|a| self.tabs.iter().position(|t| t.id == a)).unwrap_or(0);
        let mut gateways: Vec<PaneId> = moving.iter().filter_map(|t| self.tmux_of(t.focused).map(|(gw, _, _)| gw)).collect();
        gateways.sort_unstable();
        gateways.dedup();
        Some((moving, gateways))
    }

    /// This window after its tabs left: it closes when it has none, else it settles and lays
    /// out again.
    fn after_tabs_left(&mut self) {
        if self.tabs.is_empty() {
            self.drop_current_window();
        } else {
            self.settle_tabs();
            self.tmux_reorder();
            self.relayout();
            self.redraw();
        }
    }

    /// A group dropped in another window's bar (`window`, its registry key) at the client
    /// point `at` (`PT moveTabGroup:toTerminal:atIndex:`, `PT:7826-7880`): its tabs go there
    /// together, never inside another group, keeping the group. A collapsed group stays
    /// collapsed and that window keeps its selection; an expanded one's last tab is selected.
    /// The window comes to the front; this one closes when it has no tab left. A window under
    /// Lock Layout takes no tab.
    fn move_group_to_window(&mut self, group: &str, window: u64, at: (f32, f32)) {
        let Some(dest) = self.window_id_of_key(window) else { return };
        if self.windows.parked().any(|(k, s)| *k == window && s.locks().forbids(LayoutOp::AddTab)) {
            return;
        }
        let Some((moving, gateways)) = self.take_group(group) else { return };
        let collapsed = moving.first().and_then(|t| t.group.as_ref()).is_some_and(|g| g.collapsed);
        let panes: Vec<PaneId> = moving.iter().flat_map(|t| t.layout.panes()).collect();
        self.browser_views_follow(&panes, dest);
        self.after_tabs_left();
        self.enter(dest);
        let vertical = tab_position::vertical(self.tab_position());
        let rects = self.tab_bar_drawn.as_ref().map(|(_, r, _)| r.clone()).unwrap_or_default();
        let slot = tab_drag::drop_index(&rects, at.0, at.1, vertical);
        let slot = tab_drag::group_drop_slot(&self.group_order(), group, &rects, slot, at.0, at.1, vertical).min(self.tabs.len());
        let kept = self.tabs.get(self.active).map(|t| t.id);
        let last = moving.last().map(|t| t.id);
        self.tabs.splice(slot..slot, moving);
        let selected = if collapsed { kept } else { last };
        self.active = selected.and_then(|a| self.tabs.iter().position(|t| t.id == a)).unwrap_or(0);
        self.settle_tabs();
        self.tmux_reorder();
        self.relayout();
        self.focus_terminal_window(dest);
        self.redraw();
        for gw in gateways {
            self.tmux_save_affinities(gw);
        }
    }

    /// A group dropped outside every tab bar (`PT tearOffTabGroup:atScreenPoint:`,
    /// `PT:7882-7926`): its tabs open together in a new window whose top-left corner is the
    /// drop point `screen`, keeping the group, with its last tab selected (each tab inserted
    /// is selected in turn, `PT:7898-7909`); this window closes when it has no tab left. A
    /// failed open puts the tabs back.
    fn tear_off_group(&mut self, group: &str, screen: (i32, i32), event_loop: &ActiveEventLoop) {
        let Some(source) = self.current_window() else { return };
        let Some((mut moving, gateways)) = self.take_group(group) else { return };
        if moving.is_empty() {
            return;
        }
        let first = moving.remove(0);
        self.open_pending(event_loop, windows_ui::NewWindow::TornOff(first, screen));
        let Some(new) = self.current_window().filter(|w| *w != source) else {
            // The first tab came back; the rest follow it, and the order settles.
            self.tabs.extend(moving);
            if self.settle_tabs() {
                self.tmux_reorder();
            }
            self.relayout();
            return;
        };
        let last = moving.last().map(|t| t.id);
        self.tabs.extend(moving);
        self.active = last.and_then(|id| self.tabs.iter().position(|t| t.id == id)).unwrap_or(0);
        self.settle_tabs();
        self.relayout();
        self.redraw();
        if self.enter(source) {
            self.after_tabs_left();
        }
        self.focus_terminal_window(new);
        for gw in gateways {
            self.tmux_save_affinities(gw);
        }
    }

    /// The bar `bar` (`area` in pixels) with a dragged tab, or a group dragged by its chip
    /// (the chip and its tabs), moved along it, or down a side bar, to follow the pointer and
    /// drawn last, over the others (their looks marked `dragged` so they have a background);
    /// None while no drag is under way. The move stays inside the tabs' area.
    pub(super) fn dragged_bar(&self, bar: &tab_layout::Bar, looks: &mut [scene::TabLook], area: Rect) -> Option<tab_layout::Bar> {
        let (moving, start, chip): (Vec<usize>, (f32, f32), Option<usize>) = match &self.drag {
            Drag::Tab { from, start, began: true, .. } => (vec![*from], *start, None),
            Drag::Chip { group, start, began: true } => {
                let k = bar.chips.iter().position(|(id, _)| id == group)?;
                (bar.chip_texts.get(k)?.run.clone().collect(), *start, Some(k))
            }
            _ => return None,
        };
        let ks: Vec<usize> = bar.cells.iter().enumerate().filter(|(_, (i, _, _))| moving.contains(i)).map(|(k, _)| k).collect();
        if ks.is_empty() && chip.is_none() {
            return None;
        }
        // The block moved: its extent along the bar, clamped to the tabs' room.
        let rects: Vec<Rect> = ks.iter().map(|&k| bar.cells[k].2).chain(chip.map(|k| bar.chips[k].1)).collect();
        let lo = rects.iter().map(|r| if bar.vertical { r.y } else { r.x }).fold(f32::MAX, f32::min);
        let hi = rects.iter().map(|r| if bar.vertical { r.y + r.h } else { r.x + r.w }).fold(f32::MIN, f32::max);
        let scale = self.scale();
        let room = if bar.vertical { area.h / scale } else { tab_layout::tab_area_width(bar, area.w / scale) };
        let pointer = if bar.vertical { self.cursor.1 - start.1 } else { self.cursor.0 - start.0 };
        let d = (pointer / scale).clamp(-lo, (room - hi).max(-lo));
        let mut out = bar.clone();
        let mut shifted = Vec::new();
        for &k in ks.iter().rev() {
            let (i, mut c, mut r) = out.cells.remove(k);
            if bar.vertical {
                // A row's inside is placed from its top, so only the row moves.
                r.y += d;
            } else {
                let shift = |o: &mut Option<Rect>| {
                    if let Some(q) = o.as_mut() {
                        q.x += d;
                    }
                };
                shift(&mut c.close);
                shift(&mut c.icon);
                shift(&mut c.indicator);
                shift(&mut c.counter);
                shift(&mut c.graphic);
                c.label.x += d;
                r.x += d;
            }
            // The per-cell texts go last with their cell.
            let texts = (
                (k < out.titles.len()).then(|| out.titles.remove(k)),
                (k < out.subtitles.len()).then(|| out.subtitles.remove(k)),
                (k < out.texts.len()).then(|| out.texts.remove(k)),
            );
            shifted.push(((i, c, r), texts));
        }
        for (cell, (title, subtitle, text)) in shifted.into_iter().rev() {
            out.cells.push(cell);
            out.titles.extend(title);
            out.subtitles.extend(subtitle);
            out.texts.extend(text);
        }
        if let Some(k) = chip {
            let r = &mut out.chips[k].1;
            if bar.vertical {
                r.y += d;
            } else {
                r.x += d;
            }
        }
        for i in moving {
            if let Some(l) = looks.get_mut(i) {
                l.dragged = true;
            }
        }
        Some(out)
    }

    /// The drop marker: a 2-pixel line at the slot a dragged tab would drop at in this
    /// window's bar, as last drawn.
    pub(super) fn draw_drop_marker(&self, frame: &mut Frame) {
        let (Some(slot), true) = (self.tab_drop_marker, self.tab_drag_began()) else { return };
        let Some((bar, rects, _)) = &self.tab_bar_drawn else { return };
        if let Some(rect) = tab_drag::marker(rects, slot, *bar, tab_position::vertical(self.tab_position())) {
            frame.quads.push(scene::Quad { rect, color: rgba(scene::LIST_SELECTED, 1.0) });
        }
    }
}

/// The pointer's screen point (`GetCursorPos`).
pub(super) fn cursor_screen() -> Option<(i32, i32)> {
    let mut p = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
    (unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p) } != 0).then_some((p.x, p.y))
}

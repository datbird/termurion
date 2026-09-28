//! Dragging tabs (milestone 13d): a left press on a tab becomes a drag once the pointer moves
//! `MinimumTabDragDistance` pixels (`PSM:3775`, `:3814`); the tab then follows the pointer
//! along its bar over a drop marker, and the release reorders it in its own bar, moves it
//! into another window's bar, or tears it off into a new window at the drop point
//! (`PT shouldDropTabViewItem:`, `PT:7653-7670`). A press on the "+" button that moves as far
//! drags the window instead (`AllowDragOnAddTabButton`, `PT:10259-10261`). The rules are
//! `crate::tab_drag`'s.

use super::*;
use crate::tab_drag::Target;
use crate::tab_groups;
use crate::winmodel::LayoutOp;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

impl App {
    /// A left press on the tab bar may start a drag: on a tab unless Lock Layout forbids
    /// reordering (no beep: the press still selects, `PT:7610-7616`); on the "+" button with
    /// `AllowDragOnAddTabButton`.
    pub(super) fn tab_drag_press(&mut self, hit: tab_layout::Hit) {
        let start = self.cursor;
        self.tab_drop_marker = None;
        match hit {
            tab_layout::Hit::Tab(from) if self.layout_allowed_quiet(LayoutOp::ReorderTab) => {
                self.drag = Drag::Tab { from, start, began: false };
            }
            tab_layout::Hit::Add if self.globals.bool("AllowDragOnAddTabButton") && self.new_tab_button_rect().is_some_and(|r| r.contains(start.0, start.1)) => {
                self.drag = Drag::AddButton { start };
            }
            _ => {}
        }
    }

    /// A tab drag has begun (the pointer moved far enough from the press).
    pub(super) fn tab_drag_began(&self) -> bool {
        matches!(self.drag, Drag::Tab { began: true, .. })
    }

    /// The pointer moved during a press on a tab: the drag begins at `MinimumTabDragDistance`,
    /// then the drop marker follows the slot under the pointer in the tab's own bar and the
    /// tab is drawn at the pointer. A press that went (the tab moved away under it) ends it.
    pub(super) fn tab_drag_move(&mut self, x: f32, y: f32) {
        let Drag::Tab { from, start, began } = self.drag else { return };
        if self.tab_press.is_none() || self.tab_press_moved() {
            self.end_tab_drag();
            return self.redraw();
        }
        if !began {
            if !tab_drag::began(start, (x, y), self.min_tab_drag) {
                return;
            }
            self.drag = Drag::Tab { from, start, began: true };
            self.hide_tab_tip();
        }
        self.tab_drop_marker = self.own_bar_slot(x, y);
        self.redraw();
    }

    /// The press that dragged a tab came up: the tab drops where `tab_drag::target` says,
    /// with the pointer's screen point for another window's bar or a new window.
    pub(super) fn tab_drag_release(&mut self, event_loop: &ActiveEventLoop) {
        let Drag::Tab { from, began: true, .. } = self.drag else { return self.end_tab_drag() };
        self.end_tab_drag();
        if from >= self.tabs.len() {
            return;
        }
        let (x, y) = self.cursor;
        let own = self.own_bar_slot(x, y);
        let screen = cursor_screen();
        let other = if own.is_none() { screen.and_then(|p| self.window_bar_hit(p)) } else { None };
        match tab_drag::target(own, other, self.tabs.len(), self.globals.bool("AllowDragOfTabIntoNewWindow")) {
            Target::OwnBar(slot) => self.drop_in_own_bar(from, slot),
            Target::OtherWindow(window, slot) => self.drop_in_window(from, window, slot),
            Target::NewWindow => {
                if let Some(p) = screen {
                    self.tear_off(from, p, event_loop);
                }
            }
            Target::Nowhere => {}
        }
        self.redraw();
    }

    /// Another window's tab bar under the screen point `screen` (the topmost window there),
    /// and the slot a tab dropped there takes; None over no other window's bar.
    pub(super) fn window_bar_hit(&self, screen: (i32, i32)) -> Option<(u64, usize)> {
        use windows_sys::Win32::Foundation::{HWND, POINT};
        use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetAncestor, WindowFromPoint, GA_ROOT};
        let under = unsafe { GetAncestor(WindowFromPoint(POINT { x: screen.0, y: screen.1 }), GA_ROOT) } as isize;
        if under == 0 {
            return None;
        }
        let vertical = tab_position::vertical(self.tab_position());
        self.windows.parked().find_map(|(k, s)| {
            let hwnd = s.gpu.as_ref().and_then(|g| hwnd_of(&g.window)).filter(|h| *h == under)?;
            let (bar, rects) = s.tab_bar_drawn.as_ref()?;
            let mut p = POINT { x: screen.0, y: screen.1 };
            if unsafe { ScreenToClient(hwnd as HWND, &mut p) } == 0 {
                return None;
            }
            let (x, y) = (p.x as f32, p.y as f32);
            bar.contains(x, y).then(|| (*k, tab_drag::drop_index(rects, x, y, vertical)))
        })
    }

    /// Ends a press on a tab or on the "+" (dropped, cancelled, or never dragged).
    pub(super) fn end_tab_drag(&mut self) {
        if matches!(self.drag, Drag::Tab { .. } | Drag::AddButton { .. }) {
            self.drag = Drag::None;
        }
        self.tab_drop_marker = None;
    }

    /// The pointer moved during a press on the "+": once it passes `MinimumTabDragDistance`
    /// the window moves with it and the press is forgotten, so no tab opens.
    pub(super) fn add_button_drag(&mut self, start: (f32, f32), x: f32, y: f32) {
        if !tab_drag::began(start, (x, y), self.min_tab_drag) {
            return;
        }
        self.drag = Drag::None;
        self.tab_press = None;
        if let Some(g) = &self.gpu {
            let _ = g.window.drag_window();
        }
        self.redraw();
    }

    /// The slot under the window point (`x`, `y`) in this window's tab bar; None off it.
    fn own_bar_slot(&self, x: f32, y: f32) -> Option<usize> {
        self.tab_bar_rect().filter(|b| b.contains(x, y))?;
        Some(tab_drag::drop_index(&self.bar_tab_rects(), x, y, tab_position::vertical(self.tab_position())))
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
    /// (`PT didDonateTab:`). A tab holding a loaded recording never moves, and a window under
    /// Lock Layout takes no tab. A tmux tab moves whole; its session hears which tmux windows
    /// share each Tandem window (`saveAffinities`), as the API's ReorderTabs does.
    fn drop_in_window(&mut self, from: usize, window: u64, slot: usize) {
        if self.tabs.get(from).is_none_or(|t| self.holds_recording(t)) {
            return;
        }
        let Some(dest) = self.window_id_of_key(window) else { return };
        if self.windows.parked().any(|(k, s)| *k == window && s.locks().forbids(LayoutOp::AddTab)) {
            return;
        }
        let moving = self.take_tab(from);
        let gateway = self.tmux_of(moving.focused).map(|(gw, _, _)| gw);
        if self.tabs.is_empty() {
            self.drop_current_window();
        } else {
            self.active = crate::close_rules::active_after_close(self.active, from, self.tabs.len(), self.globals.bool("moveLeftAfterClosingTab"));
            self.tmux_reorder();
            self.relayout();
            self.redraw();
        }
        // `dest` is a terminal window, so the tab always lands there.
        self.enter(dest);
        self.tabs.push(moving);
        let id = self.place_dropped_tab(self.tabs.len() - 1, slot);
        if let Some(i) = self.tabs.iter().position(|t| t.id == id) {
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

    /// The bar `bar` (`area` in pixels) with a dragged tab moved along it, or down a side bar,
    /// to follow the pointer and drawn last, over the others (its look marked `dragged` so it
    /// has a background); None while no tab drag is under way. The move stays inside the
    /// tabs' area.
    pub(super) fn dragged_bar(&self, bar: &tab_layout::Bar, looks: &mut [scene::TabLook], area: Rect) -> Option<tab_layout::Bar> {
        let Drag::Tab { from, start, began: true } = self.drag else { return None };
        let k = bar.cells.iter().position(|(i, _, _)| *i == from)?;
        let scale = self.scale();
        let mut out = bar.clone();
        let (i, mut c, mut r) = out.cells.remove(k);
        if bar.vertical {
            // A row's inside is placed from its top, so only the row moves.
            let room = area.h / scale;
            r.y += ((self.cursor.1 - start.1) / scale).clamp(-r.y, (room - r.h - r.y).max(-r.y));
        } else {
            let room = tab_layout::tab_area_width(bar, area.w / scale);
            let dx = ((self.cursor.0 - start.0) / scale).clamp(-r.x, (room - r.w - r.x).max(-r.x));
            let shift = |o: &mut Option<Rect>| {
                if let Some(q) = o.as_mut() {
                    q.x += dx;
                }
            };
            shift(&mut c.close);
            shift(&mut c.icon);
            shift(&mut c.indicator);
            shift(&mut c.counter);
            c.label.x += dx;
            r.x += dx;
        }
        out.cells.push((i, c, r));
        // The per-cell texts go last with their cell.
        if k < out.titles.len() {
            let t = out.titles.remove(k);
            out.titles.push(t);
        }
        if k < out.subtitles.len() {
            let t = out.subtitles.remove(k);
            out.subtitles.push(t);
        }
        if k < out.texts.len() {
            let t = out.texts.remove(k);
            out.texts.push(t);
        }
        if let Some(l) = looks.get_mut(from) {
            l.dragged = true;
        }
        Some(out)
    }

    /// The drop marker: a 2-pixel line at the slot a dragged tab would drop at in this
    /// window's bar, as last drawn.
    pub(super) fn draw_drop_marker(&self, frame: &mut Frame) {
        let (Some(slot), true) = (self.tab_drop_marker, self.tab_drag_began()) else { return };
        let Some((bar, rects)) = &self.tab_bar_drawn else { return };
        if let Some(rect) = tab_drag::marker(rects, slot, *bar, tab_position::vertical(self.tab_position())) {
            frame.quads.push(scene::Quad { rect, color: rgba(scene::LIST_SELECTED, 1.0) });
        }
    }
}

/// The pointer's screen point (`GetCursorPos`).
fn cursor_screen() -> Option<(i32, i32)> {
    let mut p = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
    (unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p) } != 0).then_some((p.x, p.y))
}

/// A window's Win32 handle.
fn hwnd_of(window: &Window) -> Option<isize> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

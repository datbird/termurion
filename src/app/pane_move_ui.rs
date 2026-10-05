//! Move Session to Split Pane and Swap With Session: the pick mode that spans every window
//! (`MovePaneController.m`, `SplitSelectionView.m`), the drop that moves or swaps panes, and
//! the overlay's drawing. The model (halves, looks, drop plan, tmux commands) is `pane_move`.

use super::*;
use crate::layout::visible_rects;
use crate::pane_move::{self, Half};

/// The pick mode (P24: app-wide, never in `swap_live`). The hover is a pane and its half; the
/// pane's window is found by `pane_window` at each use (P10), never kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PaneMove {
    /// The session being moved or swapped.
    pub(super) source: PaneId,
    /// Swap With Session rather than Move Session to Split Pane.
    pub(super) swap: bool,
    /// The target pane under the pointer and the half it shows.
    pub(super) hover: Option<(PaneId, Half)>,
    /// A title bar drag (Task 8) rather than the pick: it drops on the release, not a press.
    pub(super) drag: bool,
}

/// What `apply_drop` did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Applied {
    /// The pane moved; `emptied_tab` is the index (before the removal) of the tab it left
    /// empty, which `apply_drop` removed.
    Moved { emptied_tab: Option<usize> },
    Swapped,
    Refused,
}

/// What can end a pick without a drop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PickEvent {
    /// A click on a pane.
    Click(PaneId),
    /// A session was started; only the person's cancels (P15), not a script's or the API's.
    NewSession { by_person: bool },
    /// A session ended.
    Ended(PaneId),
}

/// The pick ends when the source is clicked (`MovePaneController.m:226-232`, `:367-370`), a
/// new session starts (`PTYSession.m:851-853`) or the source ends (`PTYSession.m:3940-3942`).
pub(super) fn cancel_on(event: PickEvent, source: PaneId) -> bool {
    match event {
        PickEvent::Click(p) | PickEvent::Ended(p) => p == source,
        PickEvent::NewSession { by_person } => by_person,
    }
}

/// The tmux window a pane dropped on a tab bar goes next to (`PseudoTerminal.m:8651-8659`
/// breaks it out into the destination window): the tab at the drop `index` when it belongs to
/// tmux session `gw`, else the nearest such tab at or after it, else the nearest before.
/// `tabs` is each destination tab's tmux session and window (None: not a tmux tab). None when
/// the window has no tab of that session (the drop is refused).
pub(super) fn tmux_drop_sibling(tabs: &[Option<(PaneId, u32)>], index: usize, gw: PaneId) -> Option<u32> {
    let mine = |t: &Option<(PaneId, u32)>| t.filter(|(g, _)| *g == gw).map(|(_, w)| w);
    let at = index.min(tabs.len());
    tabs[at..].iter().find_map(mine).or_else(|| tabs[..at].iter().rev().find_map(mine))
}

/// What can end a title bar drag without its release (iTerm2's drag session ends on Esc and
/// when the window resigns key; a lost mouse capture shows as a move with the button up).
#[derive(Clone, Copy)]
pub(super) enum DragLost {
    FocusLost,
    Escape,
    Moved { left_down: bool },
}

/// Whether `event` ends a title bar drag.
pub(super) fn drag_lost(event: DragLost) -> bool {
    match event {
        DragLost::FocusLost | DragLost::Escape => true,
        DragLost::Moved { left_down } => !left_down,
    }
}

/// Ends a title bar drag with nothing moved: the drag and its pick overlay go. True when
/// anything was on.
pub(super) fn end_title_drag(drag: &mut Drag, pick: &mut Option<PaneMove>) -> bool {
    let was_drag = matches!(drag, Drag::Title { .. });
    if was_drag {
        *drag = Drag::None;
    }
    let was_pick = pick.as_ref().is_some_and(|pm| pm.drag);
    if was_pick {
        *pick = None;
    }
    was_drag || was_pick
}

/// A title bar drag begins once the pointer is this many points from the press
/// (`SessionTitleView mouseDragged:`; the system's drag distance, scaled).
pub(super) const DRAG_THRESHOLD: f32 = 4.0;

/// Whether a title bar drag may start: Alt is required only with the Advanced key
/// `RequireOptionToDragSplitPaneTitleBar` (`SessionTitleView.m:377-384`).
pub(super) fn title_drag_allowed(require_alt: bool, alt: bool) -> bool {
    !require_alt || alt
}

/// Whether the pointer at `to` is far enough from the press at `from` (pixels at `scale`).
pub(super) fn drag_began(from: (f32, f32), to: (f32, f32), scale: f32) -> bool {
    (to.0 - from.0).hypot(to.1 - from.1) >= DRAG_THRESHOLD * scale
}

/// What a released title bar drag does. A lone pane never gets here: it moves the window when the
/// drag begins (P6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DragOutcome {
    /// Dropped on another pane's half (`Half::None` when the pointer sits where no half shows).
    Drop(PaneId, Half),
    /// Dropped on a window's tab bar (registry key, slot): a tab there (`PseudoTerminal.m:8648-8677`).
    NewTab(u64, usize),
    /// Dropped outside every Tandem window (`MovePaneController.m:234-237`).
    NewWindow,
    /// Dropped on itself, or on a window's other chrome.
    Nothing,
}

/// The outcome of a release: over another pane is a drop, over the source itself nothing
/// (`MovePaneController.m:226-232`), over a tab bar a new tab, over a window's other parts
/// nothing, outside every window a new window.
pub(super) fn drag_outcome(over_window: bool, over_bar: Option<(u64, usize)>, over_pane: Option<(PaneId, Half)>, source: PaneId) -> DragOutcome {
    if !over_window {
        return DragOutcome::NewWindow;
    }
    match (over_pane, over_bar) {
        (Some((p, _)), _) if p == source => DragOutcome::Nothing,
        (Some((p, half)), _) => DragOutcome::Drop(p, half),
        (None, Some((window, slot))) => DragOutcome::NewTab(window, slot),
        (None, None) => DragOutcome::Nothing,
    }
}

/// Where a pane dropped at bar slot `slot` takes its tab (P12): out of the pinned zone (the new
/// tab is unpinned) and never inside a group's run (`groups` is each tab's group id), where a
/// collapsed group would hide it: it goes after the group.
pub(super) fn new_tab_slot(groups: &[Option<&str>], pinned: &[bool], slot: usize) -> usize {
    let slot = crate::tab_drag::clamp_to_zone(pinned.len(), slot.min(groups.len()), pinned);
    match slot.checked_sub(1).and_then(|b| groups.get(b).copied().flatten()) {
        Some(g) if groups.get(slot).copied().flatten() == Some(g) => groups[slot..].iter().position(|x| *x != Some(g)).map_or(groups.len(), |n| slot + n),
        _ => slot,
    }
}

/// The panes that left a window in a drop (`Broadcast::forget` is for those only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Left {
    /// Left the source's window.
    pub(super) source_window: Vec<PaneId>,
    /// Left the target's window.
    pub(super) target_window: Vec<PaneId>,
}

/// A move or swap within one window takes nobody out of it; across windows the source left its
/// window, and in a swap the target left its own.
pub(super) fn left_windows(source: PaneId, target: PaneId, same_window: bool, swap: bool) -> Left {
    match (same_window, swap) {
        (true, _) => Left { source_window: vec![], target_window: vec![] },
        (false, false) => Left { source_window: vec![source], target_window: vec![] },
        (false, true) => Left { source_window: vec![source], target_window: vec![target] },
    }
}

/// The panes a drop on a window's tab bar takes out of the broadcast: the pane only when it
/// changes window (the Task 7 rule: broadcast keeps panes that stay).
pub(super) fn tab_drop_left(pane: PaneId, same_window: bool) -> Vec<PaneId> {
    left_windows(pane, pane, same_window, false).source_window
}

/// Whether the primary (logical left) mouse button is down, given the physical buttons and
/// whether the buttons are swapped (`SM_SWAPBUTTON`): swapped, the primary is the right one.
pub(super) fn primary_down(swapped: bool, left: bool, right: bool) -> bool {
    if swapped { right } else { left }
}

/// `from` takes `to`'s id in the tab's layout, focus and zoom.
fn replace_pane(tab: &mut Tab, from: PaneId, to: PaneId) {
    tab.layout = Layout::from_tree(tab.layout.tree().map(&mut |p| if p == from { to } else { p }));
    if tab.focused == from {
        tab.focused = to;
    }
    if tab.zoomed == Some(from) {
        tab.zoomed = Some(to);
    }
}

/// What leaving a tab did to it.
pub(super) enum Detached {
    Refused,
    Kept,
    /// The tab had only that pane: it was removed from the list.
    Emptied(usize, Tab),
}

/// Takes `source` out of its tab: the tab's focus goes to its first pane and its zoom ends
/// (`move_session_to_tab`); a tab left empty leaves the list.
pub(super) fn detach(tabs: &mut Vec<Tab>, source: PaneId) -> Detached {
    let Some(at) = tabs.iter().position(|t| t.layout.panes().contains(&source)) else { return Detached::Refused };
    if tabs[at].layout.panes().len() < 2 {
        return Detached::Emptied(at, tabs.remove(at));
    }
    let tab = &mut tabs[at];
    if !tab.layout.remove(source) {
        return Detached::Refused;
    }
    tab.focused = tab.layout.panes()[0];
    tab.zoomed = None;
    Detached::Kept
}

/// Splits `target` of `tab` with `source` (West and North put it first) and focuses it.
fn attach(tab: &mut Tab, source: PaneId, target: PaneId, axis: Axis, before: bool) -> bool {
    if !tab.layout.split(target, axis, source) {
        return false;
    }
    if before {
        tab.layout.swap(source, target);
    }
    tab.focused = source;
    tab.zoomed = None;
    true
}

/// One window's tabs after a drop of `source` on `target` (`MovePaneController.m:222-306`):
/// a move splits the target's tab and focuses the moved pane, closing the tab it emptied; a
/// swap exchanges the two panes' places, in two tabs when they are in two.
pub(super) fn apply_drop(tabs: &mut Vec<Tab>, source: PaneId, target: PaneId, plan: pane_move::Drop) -> Applied {
    if source == target {
        return Applied::Refused;
    }
    let find = |tabs: &[Tab], p: PaneId| tabs.iter().position(|t| t.layout.panes().contains(&p));
    let (Some(ts), Some(tt)) = (find(tabs, source), find(tabs, target)) else { return Applied::Refused };
    match plan {
        pane_move::Drop::Cancel => Applied::Refused,
        pane_move::Drop::Swap if ts == tt => {
            if tabs[ts].layout.swap(source, target) {
                Applied::Swapped
            } else {
                Applied::Refused
            }
        }
        pane_move::Drop::Swap => {
            replace_pane(&mut tabs[ts], source, target);
            replace_pane(&mut tabs[tt], target, source);
            Applied::Swapped
        }
        pane_move::Drop::Split { axis, before } => {
            let emptied_tab = match detach(tabs, source) {
                Detached::Refused => return Applied::Refused,
                Detached::Kept => None,
                Detached::Emptied(at, _) => Some(at),
            };
            let Some(tt) = find(tabs, target) else { return Applied::Refused };
            if attach(&mut tabs[tt], source, target, axis, before) {
                Applied::Moved { emptied_tab }
            } else {
                Applied::Refused
            }
        }
    }
}

impl App {
    /// Move Session to Split Pane (`move_`) or Swap With Session from `pane`
    /// (`MovePaneController startWithSession:move:`): declined while a pick is on or under
    /// Lock Layout; every window's Instant Replay closes (`PT:12223-12236`).
    pub(super) fn start_pane_move(&mut self, pane: PaneId, move_: bool, event_loop: &ActiveEventLoop) {
        if self.pane_move.is_some() {
            return;
        }
        if self.pane_move_refused(pane) {
            find_ui::beep();
            return;
        }
        self.close_all_replays(event_loop);
        self.pane_move = Some(PaneMove { source: pane, swap: !move_, hover: None, drag: false });
        self.redraw();
        self.redraw_all();
    }

    /// The pick ends in every window.
    pub(super) fn end_pane_move(&mut self) {
        if self.pane_move.take().is_some() {
            self.redraw();
            self.redraw_all();
        }
    }

    /// A title bar drag whose release will not come (`drag_lost`): the drag and every overlay
    /// go and nothing moves. True when a drag was on.
    pub(super) fn title_drag_lost(&mut self, event: DragLost) -> bool {
        if !drag_lost(event) || !end_title_drag(&mut self.drag, &mut self.pane_move) {
            return false;
        }
        self.redraw();
        self.redraw_all();
        true
    }

    /// An event that may end the pick (`cancel_on`): a new session by the person (P15), the
    /// source ending.
    pub(super) fn pane_move_event(&mut self, event: PickEvent) {
        if self.pane_move.as_ref().is_some_and(|pm| cancel_on(event, pm.source)) {
            self.end_pane_move();
        }
    }

    /// The pane may not be moved: Lock Layout of its window (13i adds the pane lock), a loaded
    /// recording, or a tmux gateway pane (P17, `compatible` cannot see it).
    pub(super) fn pane_move_refused(&self, pane: PaneId) -> bool {
        self.pane_window_forbids(pane, crate::winmodel::LayoutOp::MovePane) || self.panes.get(&pane).is_some_and(|p| p.dvr.read_only) || self.tmux.contains_key(&pane)
    }

    /// The panes of `tabs` showing a live Instant Replay.
    fn live_replays(&self, tabs: &[Tab]) -> Vec<PaneId> {
        tabs.iter().flat_map(|t| t.layout.panes()).filter(|id| self.panes.get(id).is_some_and(|p| p.replay.is_some() && !p.dvr.read_only)).collect()
    }

    /// Every window's Instant Replay closes (B5); the window the call started in is current again.
    fn close_all_replays(&mut self, event_loop: &ActiveEventLoop) {
        let home = self.current_window();
        let others: Vec<WindowId> = self.windows.parked().filter(|(_, s)| !self.live_replays(&s.tabs).is_empty()).filter_map(|(_, s)| s.gpu.as_ref().map(|g| g.window.id())).collect();
        for id in others {
            if self.enter(id) {
                for p in self.live_replays(&self.tabs) {
                    self.close_replay_panel(Some(p), event_loop);
                }
            }
        }
        if let Some(home) = home {
            self.enter(home);
        }
        for p in self.live_replays(&self.tabs) {
            self.close_replay_panel(Some(p), event_loop);
        }
    }

    /// The current window's shown panes with their whole rectangles, title bars included (the
    /// overlay covers the whole session view).
    fn pick_rects(&self) -> Vec<(PaneId, Rect)> {
        let Some(tab) = self.tabs.get(self.active) else { return Vec::new() };
        visible_rects(&tab.layout, tab.zoomed, self.content_area_for(tab), GAP)
    }

    /// The rectangles the pick's looks are drawn in while a pick is on (they hold every event
    /// from the program, whatever the button).
    pub(super) fn pane_move_hit_rects(&self) -> Vec<Rect> {
        if self.pane_move.is_none() {
            return Vec::new();
        }
        self.pick_rects().into_iter().map(|(_, r)| r).collect()
    }

    /// A press while the pick is on. Left presses are all taken: on a pane it drops with the
    /// half shown (or the half at the press), on the source it cancels. Right and middle pass.
    pub(super) fn pane_move_press(&mut self, button: MouseButton, x: f32, y: f32, event_loop: &ActiveEventLoop) -> bool {
        let Some(pm) = self.pane_move.clone() else { return false };
        if button != MouseButton::Left || pm.drag {
            return false;
        }
        let Some((pane, rect)) = self.pick_rects().into_iter().find(|(_, r)| r.contains(x, y)) else { return true };
        if cancel_on(PickEvent::Click(pane), pm.source) {
            self.end_pane_move();
            return true;
        }
        let shown = pm.hover.filter(|(p, _)| *p == pane).map(|(_, h)| h);
        let half = shown.unwrap_or_else(|| pane_move::half_at(Half::None, pm.swap, rect.w, rect.h, x - rect.x, y - rect.y, self.scale()));
        self.drop_pane(pane, half, event_loop);
        true
    }

    /// The target pane under the window point and the half it shows, in the current window. A
    /// target in a Lock Layout window shows none (P5); the source is no target.
    fn hover_at(&self, source: PaneId, swap: bool, old: Option<(PaneId, Half)>, x: f32, y: f32) -> Option<(PaneId, Half)> {
        let (p, r) = self.pick_rects().into_iter().find(|(_, r)| r.contains(x, y))?;
        if p == source || self.pane_window_forbids(p, crate::winmodel::LayoutOp::MovePane) {
            return None;
        }
        let current = old.filter(|(hp, _)| *hp == p).map_or(Half::None, |(_, h)| h);
        let half = pane_move::half_at(current, swap, r.w, r.h, x - r.x, y - r.y, self.scale());
        (half != Half::None).then_some((p, half))
    }

    /// The pointer moved: the half under it in the pane under it.
    pub(super) fn pane_move_hover(&mut self, x: f32, y: f32) {
        let Some(pm) = self.pane_move.as_ref().filter(|pm| !pm.drag) else { return };
        let new = self.hover_at(pm.source, pm.swap, pm.hover, x, y);
        self.set_hover(new);
    }

    /// The hover changed: every window redraws.
    fn set_hover(&mut self, new: Option<(PaneId, Half)>) {
        if self.pane_move.as_ref().is_some_and(|pm| pm.hover != new) {
            if let Some(pm) = self.pane_move.as_mut() {
                pm.hover = new;
            }
            self.redraw();
            self.redraw_all();
        }
    }

    /// Runs `f` with window `w` current, then the window this was current in again. None when
    /// `w` cannot be entered. `enter` swaps only fields, so it is cheap; nothing returns between
    /// the two `enter` calls, which keeps this window's `Drag::Title` (a per-window field).
    fn in_window<R>(&mut self, w: WindowId, f: impl FnOnce(&mut Self) -> R) -> Option<R> {
        let home = self.current_window()?;
        if w == home {
            return Some(f(self));
        }
        if !self.enter(w) {
            return None;
        }
        let out = f(self);
        self.enter(home);
        Some(out)
    }

    /// The Tandem terminal window under the cursor and the cursor in its client pixels (P18:
    /// matched only against Tandem's own windows, never by title or class).
    pub(super) fn terminal_window_at_cursor(&self) -> Option<(WindowId, (f32, f32))> {
        use windows_sys::Win32::Foundation::{HWND, POINT};
        use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetAncestor, WindowFromPoint, GA_ROOT};
        let screen = tab_drag_ui::cursor_screen()?;
        let under = unsafe { GetAncestor(WindowFromPoint(POINT { x: screen.0, y: screen.1 }), GA_ROOT) } as isize;
        if under == 0 {
            return None;
        }
        let windows = self.gpu.iter().chain(self.windows.parked().filter_map(|(_, s)| s.gpu.as_ref()));
        let (id, hwnd) = windows.filter_map(|g| menu::hwnd_of(&g.window).map(|h| (g.window.id(), h))).find(|(_, h)| *h == under)?;
        let mut p = POINT { x: screen.0, y: screen.1 };
        (unsafe { ScreenToClient(hwnd as HWND, &mut p) } != 0).then_some((id, (p.x as f32, p.y as f32)))
    }

    /// A press on a title bar's pane moved: the drag begins at the threshold (a lone pane moves
    /// the window instead, P6), then the hover follows the pointer over any Tandem window.
    pub(super) fn title_drag_move(&mut self, pane: PaneId, from: (f32, f32), x: f32, y: f32) {
        let began = self.pane_move.as_ref().is_some_and(|pm| pm.drag && pm.source == pane);
        if !began {
            if !drag_began(from, (x, y), self.scale()) || !title_drag_allowed(self.globals.bool("RequireOptionToDragSplitPaneTitleBar"), self.mods.alt) {
                return;
            }
            let lone = self.tabs.iter().find(|t| t.layout.panes().contains(&pane)).is_none_or(|t| t.layout.panes().len() < 2);
            if lone {
                // Windows moves the window live (iTerm2 moves it by the drop offset).
                return self.begin_window_drag();
            }
            // A drag exits any pick first (`MovePaneController.m:313-326`).
            self.end_pane_move();
            if self.pane_move_refused(pane) {
                self.drag = Drag::None;
                find_ui::beep();
                return;
            }
            self.pane_move = Some(PaneMove { source: pane, swap: false, hover: None, drag: true });
            self.redraw_all();
        }
        let old = self.pane_move.as_ref().and_then(|pm| pm.hover);
        let new = self.terminal_window_at_cursor().and_then(|(w, (cx, cy))| self.in_window(w, |a| a.hover_at(pane, false, old, cx, cy)).flatten());
        self.set_hover(new);
        self.redraw();
    }

    /// The title bar drag's release (`drag_outcome`): a drop on a half, a new tab at a bar's
    /// slot, a new window at the cursor; every outcome ends the drag. True when a drag was on.
    pub(super) fn title_drag_release(&mut self, pane: PaneId, event_loop: &ActiveEventLoop) -> bool {
        let Some(pm) = self.pane_move.clone().filter(|pm| pm.drag && pm.source == pane) else { return false };
        let screen = tab_drag_ui::cursor_screen();
        let here = self.current_window();
        let at = self.terminal_window_at_cursor();
        let (mut over_pane, mut over_bar) = (None, None);
        if let Some((w, (cx, cy))) = at {
            let swap = pm.swap;
            over_pane = self.in_window(w, |a| {
                let (p, r) = a.pick_rects().into_iter().find(|(_, r)| r.contains(cx, cy))?;
                if p == pane {
                    return Some((p, Half::None));
                }
                // No half shown still drops, below (the pick's rule).
                let shown = pm.hover.filter(|(hp, _)| *hp == p).map(|(_, h)| h);
                Some((p, shown.unwrap_or_else(|| pane_move::half_at(Half::None, swap, r.w, r.h, cx - r.x, cy - r.y, a.scale()))))
            }).flatten();
            over_bar = if Some(w) == here { self.own_bar_slot(cx, cy) } else { screen.and_then(|s| self.window_bar_hit(s)) };
        }
        match drag_outcome(at.is_some(), over_bar, over_pane, pane) {
            DragOutcome::Drop(target, half) => {
                self.drop_pane(target, half, event_loop);
                // A drop in another window brings that window to the front.
                if let Some(w) = self.pane_window_id(pane).filter(|w| Some(*w) != here) {
                    self.focus_terminal_window(w);
                }
            }
            DragOutcome::NewTab(key, slot) => {
                if let Some(window) = self.window_id_of_key(key) {
                    self.move_session_to_new_tab(pane, window, slot, event_loop);
                }
            }
            DragOutcome::NewWindow => {
                if let Some(point) = screen {
                    self.move_session_to_window_at(pane, point, event_loop);
                }
            }
            DragOutcome::Nothing => {}
        }
        self.end_pane_move();
        self.redraw();
        true
    }

    /// The slot a tab dropped at the window point takes in this window's own tab bar; None
    /// off the bar. The key is this window's registry key.
    fn own_bar_slot(&self, x: f32, y: f32) -> Option<(u64, usize)> {
        self.tab_bar_rect().filter(|b| b.contains(x, y))?;
        let vertical = tab_position::vertical(self.tab_position());
        Some((self.windows.current()?, crate::tab_drag::drop_index(&self.bar_tab_rects(), x, y, vertical)))
    }

    /// Drops the source on `target`'s `half`. False when nothing happened and the pick stays
    /// (an incompatible or locked target, a half that plans nothing); true when the pick ended.
    pub(super) fn drop_pane(&mut self, target: PaneId, half: Half, event_loop: &ActiveEventLoop) -> bool {
        let _ = event_loop;
        let Some(pm) = self.pane_move.clone() else { return false };
        let source = pm.source;
        // P10: a pane that left its window cancels the pick.
        let (Some(sid), Some(tid)) = (self.pane_window_id(source), self.pane_window_id(target)) else {
            self.end_pane_move();
            return true;
        };
        let plan = pane_move::drop_plan(half, pm.swap, source == target);
        if plan == pane_move::Drop::Cancel {
            if source == target {
                self.end_pane_move();
                return true;
            }
            return false;
        }
        // `PTYSession isCompatibleWith:` (a drop on an incompatible session does nothing, the
        // pick stays); a tmux gateway pane is no target (P17).
        let gateway = |p: PaneId| self.tmux_of(p).map(|(gw, _, _)| gw);
        if !pane_move::compatible(gateway(source), gateway(target)) || self.tmux.contains_key(&target) {
            return false;
        }
        // Lock Layout of both windows (P5).
        if self.pane_move_refused(source) || self.pane_window_forbids(target, crate::winmodel::LayoutOp::MovePane) {
            find_ui::beep();
            return false;
        }
        if self.tmux_of(source).is_some() {
            // tmux moves the panes; its layout change shows them (P17).
            let sent = match plan {
                pane_move::Drop::Swap => self.tmux_swap(source, target),
                other => self.tmux_move_pane(source, target, other),
            };
            self.end_pane_move();
            return sent;
        }
        let done = if sid == tid { self.move_in_window(source, target, plan) } else { self.move_across_windows(source, target, plan, (sid, tid)) };
        self.end_pane_move();
        self.redraw_all();
        done
    }

    /// A tab that left: its MRU entry, broadcast setting and variables go.
    fn forget_tab(&mut self, id: u64) {
        self.mru.forget(id);
        self.broadcast_tabs.remove(&id);
        self.api_vars.forget(crate::api_vars::Scope::Tab, &id.to_string());
    }

    /// Both panes are in the current window.
    fn move_in_window(&mut self, source: PaneId, target: PaneId, plan: pane_move::Drop) -> bool {
        let ids: Vec<u64> = self.tabs.iter().map(|t| t.id).collect();
        let same_tab = self.tabs.iter().any(|t| {
            let panes = t.layout.panes();
            panes.contains(&source) && panes.contains(&target)
        });
        if !same_tab {
            self.end_replays(&[source, target]);
        }
        let applied = apply_drop(&mut self.tabs, source, target, plan);
        if applied == Applied::Refused {
            return false;
        }
        for id in ids {
            if !self.tabs.iter().any(|t| t.id == id) {
                self.forget_tab(id);
            }
        }
        if matches!(applied, Applied::Moved { .. }) {
            if let Some(at) = self.tabs.iter().position(|t| t.layout.panes().contains(&source)) {
                self.active = at;
            }
        }
        self.active = self.active.min(self.tabs.len().saturating_sub(1));
        self.refresh_route();
        self.relayout();
        self.redraw();
        true
    }

    /// The panes are in two windows: the source leaves its window (which closes when it was
    /// the last pane) and joins the target's tab, or the two exchange windows.
    fn move_across_windows(&mut self, source: PaneId, target: PaneId, plan: pane_move::Drop, (sid, tid): (WindowId, WindowId)) -> bool {
        // A loaded recording never changes window.
        if self.panes.get(&target).is_some_and(|p| p.dvr.read_only) && plan == pane_move::Drop::Swap {
            return false;
        }
        if !self.enter(sid) {
            return false;
        }
        self.end_replays(&[source]);
        let left = left_windows(source, target, false, plan == pane_move::Drop::Swap);
        self.broadcast.forget(&left.source_window);
        let selected = self.tabs.get(self.active).map(|t| t.id);
        match plan {
            pane_move::Drop::Swap => {
                if let Some(t) = self.tabs.iter_mut().find(|t| t.layout.panes().contains(&source)) {
                    replace_pane(t, source, target);
                }
                self.relayout();
                self.redraw();
                if !self.enter(tid) {
                    return false;
                }
                self.end_replays(&[target]);
                self.broadcast.forget(&left.target_window);
                if let Some(t) = self.tabs.iter_mut().find(|t| t.layout.panes().contains(&target)) {
                    replace_pane(t, target, source);
                }
                self.refresh_route();
                self.relayout();
                self.redraw();
                true
            }
            pane_move::Drop::Split { axis, before } => {
                match detach(&mut self.tabs, source) {
                    Detached::Refused => return false,
                    Detached::Kept => {}
                    Detached::Emptied(_, tab) => self.forget_tab(tab.id),
                }
                if self.tabs.is_empty() {
                    self.drop_current_window();
                } else {
                    self.active = selected.and_then(|s| self.tabs.iter().position(|t| t.id == s)).unwrap_or(self.active).min(self.tabs.len() - 1);
                    self.relayout();
                    self.redraw();
                }
                let joined = self.enter(tid) && self.tabs.iter().position(|t| t.layout.panes().contains(&target)).is_some_and(|at| {
                    let ok = attach(&mut self.tabs[at], source, target, axis, before);
                    if ok {
                        self.active = at;
                    }
                    ok
                });
                if !joined {
                    // The target went away mid drop: the session keeps a tab of its own.
                    let tab = Tab::new(self.next_tab_id, Layout::new(source), source);
                    self.next_tab_id += 1;
                    self.tabs.push(tab);
                    self.active = self.tabs.len() - 1;
                }
                self.refresh_route();
                self.relayout();
                self.redraw();
                joined
            }
            pane_move::Drop::Cancel => false,
        }
    }

    /// The pick's overlay in the current window, over every other text (P22): the source
    /// pane with the source look and message, the hovered half of a target with the target's.
    pub(super) fn draw_pane_move(&self, frame: &mut scene::Frame, cell: (f32, f32)) {
        let Some(pm) = &self.pane_move else { return };
        let colors = pane_move::colors(&|key| self.globals.string(key));
        let scale = self.scale();
        for (id, rect) in self.pick_rects() {
            if id == pm.source {
                let text = if pm.swap { pane_move::SOURCE_SWAP } else { pane_move::SOURCE_MOVE };
                draw_look(frame, rect, [colors.source_fill, colors.source_border, colors.source_inner], text, cell, scale);
            } else if let Some((_, half)) = pm.hover.filter(|(p, _)| *p == id) {
                let text = if pm.swap { pane_move::TARGET_SWAP } else { pane_move::TARGET_MOVE };
                if let Some(r) = pane_move::half_rect(rect, half) {
                    draw_look(frame, r, [colors.target_fill, colors.target_border, colors.target_inner], text, cell, scale);
                }
            }
        }
    }
}

/// One look (`SplitSelectionView drawSourceWithMessage:`): the fill at `ALPHA`, an outer border
/// and one point in an inner one, and the message white over a black shadow, centered, its
/// bottom a third of the way down. The text under the box is cut out.
fn draw_look(frame: &mut scene::Frame, rect: Rect, [fill, border, inner]: [crate::palette::Rgb; 3], text: &str, cell: (f32, f32), scale: f32) {
    use crate::palette::rgb;
    scene::cut_out(frame, rect, cell.1);
    let a = pane_move::ALPHA;
    frame.popups.push(scene::Quad { rect, color: scene::rgba(fill, a) });
    let t = scale.max(1.0);
    for (r, color) in [(rect, border), (rect.inset(t), inner)] {
        let sides = [
            Rect { h: t, ..r },
            Rect { y: r.y + r.h - t, h: t, ..r },
            Rect { w: t, ..r },
            Rect { x: r.x + r.w - t, w: t, ..r },
        ];
        for side in sides {
            frame.popups.push(scene::Quad { rect: side, color: scene::rgba(color, a) });
        }
    }
    let at_one = text.chars().count() as f32 * cell.0;
    let k = pane_move::message_scale(at_one, rect.w, cell.1);
    let (w, line_h) = (at_one * k, cell.1 * k);
    let x = rect.x + ((rect.w - w) / 2.0).max(0.0);
    let y = (rect.y + rect.h / 3.0 - line_h).max(rect.y);
    let span = |color| scene::Span { text: text.to_string(), color, bold: false, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false };
    frame.rows.push(scene::TextRow { x: x + 2.0 * t, y: y + 2.0 * t, spans: vec![span(rgb(0, 0, 0))], clip: rect, scale: k });
    frame.rows.push(scene::TextRow { x, y, spans: vec![span(rgb(255, 255, 255))], clip: rect, scale: k });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Layout, Tree};
    use crate::pane_move::Drop as Plan;

    #[test]
    fn tmux_drop_goes_beside_a_tab_of_its_session() {
        // Destination tabs: a plain tab, session 7's window 3, session 9's window 4, session 7's window 5.
        let tabs = [None, Some((7, 3)), Some((9, 4)), Some((7, 5))];
        assert_eq!(tmux_drop_sibling(&tabs, 1, 7), Some(3), "the tab at the slot");
        assert_eq!(tmux_drop_sibling(&tabs, 2, 7), Some(5), "the nearest after it");
        assert_eq!(tmux_drop_sibling(&tabs, 4, 7), Some(5), "past the end: the last before");
        assert_eq!(tmux_drop_sibling(&tabs, 0, 9), Some(4));
        assert_eq!(tmux_drop_sibling(&tabs, 0, 8), None, "no tab of that session: refused");
        assert_eq!(tmux_drop_sibling(&[], 0, 7), None);
    }

    #[test]
    fn a_tab_bar_drop_forgets_broadcast_only_when_the_window_changes() {
        assert_eq!(tab_drop_left(1, true), Vec::<PaneId>::new());
        assert_eq!(tab_drop_left(1, false), vec![1]);
    }

    #[test]
    fn the_primary_button_follows_the_swap_setting() {
        assert!(primary_down(false, true, false));
        assert!(!primary_down(false, false, true));
        assert!(primary_down(true, false, true), "swapped: the physical right button is the primary");
        assert!(!primary_down(true, true, false));
    }

    #[test]
    fn a_title_drag_ends_cleanly_when_its_release_is_lost() {
        assert!(drag_lost(DragLost::FocusLost));
        assert!(drag_lost(DragLost::Escape));
        assert!(drag_lost(DragLost::Moved { left_down: false }), "capture lost: a move with the button up");
        assert!(!drag_lost(DragLost::Moved { left_down: true }));
        let mut drag = Drag::Title { pane: 1, from: (0.0, 0.0) };
        let mut pick = Some(PaneMove { source: 1, swap: false, hover: Some((2, Half::East)), drag: true });
        assert!(end_title_drag(&mut drag, &mut pick));
        assert!(matches!(drag, Drag::None) && pick.is_none(), "no drag and no overlay left");
        // A pick (not a drag) is not ended by it, and an idle state reports nothing.
        let mut idle = Drag::None;
        let mut pick = Some(PaneMove { source: 1, swap: false, hover: None, drag: false });
        assert!(!end_title_drag(&mut idle, &mut pick));
        assert!(pick.is_some(), "a click-pick stays");
        // A drag that never got past the threshold has only the Drag.
        let mut drag = Drag::Title { pane: 1, from: (0.0, 0.0) };
        let mut none = None;
        assert!(end_title_drag(&mut drag, &mut none) && matches!(drag, Drag::None));
    }

    /// Tab A holds panes 1 and 2 side by side, tab B pane 3.
    fn window() -> Vec<Tab> {
        let mut a = Layout::new(1);
        a.split(1, Axis::Row, 2);
        vec![Tab::new(10, a, 1), Tab::new(11, Layout::new(3), 3)]
    }

    #[test]
    fn pick_mode_moves_and_swaps() {
        // 1 dropped on 3's east half: A keeps pane 2, B is 3 | 1 with the focus on 1.
        let mut tabs = window();
        tabs[1].zoomed = Some(3);
        let got = apply_drop(&mut tabs, 1, 3, Plan::Split { axis: Axis::Row, before: false });
        assert_eq!(got, Applied::Moved { emptied_tab: None });
        assert_eq!(tabs[0].layout.panes(), vec![2]);
        assert_eq!((tabs[0].focused, tabs[0].zoomed), (2, None), "the source tab's focus moves off the pane that left");
        assert_eq!(tabs[1].layout.panes(), vec![3, 1]);
        assert_eq!((tabs[1].focused, tabs[1].zoomed), (1, None), "the moved pane is focused and the target tab unzoomed");

        // 3 dropped on 1's north half: B is empty and gone; A's left side is 3 over 1.
        let mut tabs = window();
        let got = apply_drop(&mut tabs, 3, 1, Plan::Split { axis: Axis::Column, before: true });
        assert_eq!(got, Applied::Moved { emptied_tab: Some(1) });
        assert_eq!(tabs.len(), 1);
        let left = Tree::Split { axis: Axis::Column, ratio: 0.5, a: Box::new(Tree::Leaf(3)), b: Box::new(Tree::Leaf(1)) };
        assert_eq!(tabs[0].layout.tree(), Tree::Split { axis: Axis::Row, ratio: 0.5, a: Box::new(left), b: Box::new(Tree::Leaf(2)) });
        assert_eq!(tabs[0].focused, 3);

        // The west half puts the moved pane first.
        let mut tabs = window();
        apply_drop(&mut tabs, 1, 3, Plan::Split { axis: Axis::Row, before: true });
        assert_eq!(tabs[1].layout.panes(), vec![1, 3]);

        // Within one tab: 1 moved to 2's south half.
        let mut tabs = window();
        let got = apply_drop(&mut tabs, 1, 2, Plan::Split { axis: Axis::Column, before: false });
        assert_eq!(got, Applied::Moved { emptied_tab: None });
        assert_eq!(tabs[0].layout.panes(), vec![2, 1]);
        assert_eq!(tabs[0].focused, 1);

        // A swap in one tab keeps the shape.
        let mut tabs = window();
        assert_eq!(apply_drop(&mut tabs, 1, 2, Plan::Swap), Applied::Swapped);
        assert_eq!(tabs[0].layout.panes(), vec![2, 1]);
        assert_eq!(tabs.len(), 2);

        // A swap across tabs exchanges their places in both, and each tab's focus follows.
        let mut tabs = window();
        assert_eq!(apply_drop(&mut tabs, 1, 3, Plan::Swap), Applied::Swapped);
        assert_eq!(tabs[0].layout.panes(), vec![3, 2]);
        assert_eq!(tabs[1].layout.panes(), vec![1]);
        assert_eq!((tabs[0].focused, tabs[1].focused), (3, 1));

        // A move onto itself, a missing pane and a cancel do nothing.
        let mut tabs = window();
        let before = tabs.clone().iter().map(|t| t.layout.panes()).collect::<Vec<_>>();
        assert_eq!(apply_drop(&mut tabs, 1, 1, Plan::Split { axis: Axis::Row, before: false }), Applied::Refused);
        assert_eq!(apply_drop(&mut tabs, 1, 9, Plan::Split { axis: Axis::Row, before: false }), Applied::Refused);
        assert_eq!(apply_drop(&mut tabs, 9, 1, Plan::Swap), Applied::Refused);
        assert_eq!(apply_drop(&mut tabs, 1, 3, Plan::Cancel), Applied::Refused);
        assert_eq!(tabs.iter().map(|t| t.layout.panes()).collect::<Vec<_>>(), before);
    }

    #[test]
    fn title_drag_follows_iterm2() {
        // `SessionTitleView.m:377-384`: Alt is required only with the Advanced key.
        assert!(title_drag_allowed(false, false));
        assert!(!title_drag_allowed(true, false));
        assert!(title_drag_allowed(true, true));
        assert!(title_drag_allowed(false, true));

        // The drag begins once the pointer is DRAG_THRESHOLD points (times the scale) away.
        assert_eq!(DRAG_THRESHOLD, 4.0);
        assert!(!drag_began((10.0, 10.0), (12.0, 12.0), 1.0));
        assert!(drag_began((10.0, 10.0), (14.0, 10.0), 1.0));
        assert!(!drag_began((0.0, 0.0), (5.0, 5.0), 2.0), "7.07 pixels is under 8");
        assert!(drag_began((0.0, 0.0), (0.0, 8.0), 2.0));

        // The release: another pane's half, its own pane, a tab bar, outside every window, a
        // window's other chrome (`MovePaneController.m:226-237`, `PseudoTerminal.m:8648-8677`).
        use crate::pane_move::Half;
        let on = |over_window, bar, pane| drag_outcome(over_window, bar, pane, 1);
        assert_eq!(on(true, None, Some((2, Half::West))), DragOutcome::Drop(2, Half::West));
        assert_eq!(on(true, None, Some((2, Half::None))), DragOutcome::Drop(2, Half::None), "no half still drops: below, as the pick");
        assert_eq!(on(true, None, Some((1, Half::None))), DragOutcome::Nothing, "dropped on itself");
        assert_eq!(on(true, Some((7, 2)), None), DragOutcome::NewTab(7, 2));
        assert_eq!(on(false, None, None), DragOutcome::NewWindow);
        assert_eq!(on(true, None, None), DragOutcome::Nothing, "a status bar or the margin");
        assert_eq!(on(false, Some((7, 2)), None), DragOutcome::NewWindow, "no window means no bar");
    }

    #[test]
    fn dropped_pane_tab_slot_keeps_zones() {
        // P12: the slot is clamped out of the pinned zone and out of a group's run.
        assert_eq!(new_tab_slot(&[None, None, None], &[false; 3], 2), 2);
        assert_eq!(new_tab_slot(&[None, None, None], &[true, false, false], 0), 1, "not among the pinned tabs");
        assert_eq!(new_tab_slot(&[None, None, None], &[true, true, false], 1), 2);
        let groups = [None, Some("g"), Some("g"), Some("g"), None];
        let loose = [false; 5];
        assert_eq!(new_tab_slot(&groups, &loose, 1), 1, "before the group's first tab");
        assert_eq!(new_tab_slot(&groups, &loose, 2), 4, "inside the group: after it");
        assert_eq!(new_tab_slot(&groups, &loose, 3), 4);
        assert_eq!(new_tab_slot(&groups, &loose, 4), 4, "after the group's last tab");
        assert_eq!(new_tab_slot(&groups, &loose, 9), 5, "past the end is the end");
        let two = [Some("a"), Some("a"), Some("b"), Some("b")];
        assert_eq!(new_tab_slot(&two, &[false; 4], 2), 2, "between two groups belongs to neither");
        assert_eq!(new_tab_slot(&[], &[], 0), 0);
    }

    #[test]
    fn broadcast_forgets_only_panes_that_left_a_window() {
        // Within one window nothing left, move or swap, across tabs or not.
        assert_eq!(left_windows(1, 3, true, false), Left { source_window: vec![], target_window: vec![] });
        assert_eq!(left_windows(1, 3, true, true), Left { source_window: vec![], target_window: vec![] });
        // A move to another window: the source left its window, the target never moved.
        assert_eq!(left_windows(1, 3, false, false), Left { source_window: vec![1], target_window: vec![] });
        // A swap across windows: each left the window it was in.
        assert_eq!(left_windows(1, 3, false, true), Left { source_window: vec![1], target_window: vec![3] });
    }

    #[test]
    fn pick_mode_cancels() {
        assert!(cancel_on(PickEvent::Click(4), 4), "clicking the source cancels");
        assert!(!cancel_on(PickEvent::Click(5), 4), "clicking another pane drops, it does not cancel");
        assert!(cancel_on(PickEvent::NewSession { by_person: true }, 4), "a new session cancels");
        assert!(!cancel_on(PickEvent::NewSession { by_person: false }, 4), "a script's or the API's session does not (P15)");
        assert!(cancel_on(PickEvent::Ended(4), 4), "the source ending cancels");
        assert!(!cancel_on(PickEvent::Ended(5), 4), "another pane ending does not");
    }
}

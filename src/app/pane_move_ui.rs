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
    /// A session was started by the person.
    NewSession,
    /// A session ended.
    Ended(PaneId),
}

/// The pick ends when the source is clicked (`MovePaneController.m:226-232`, `:367-370`), a
/// new session starts (`PTYSession.m:851-853`) or the source ends (`PTYSession.m:3940-3942`).
pub(super) fn cancel_on(event: PickEvent, source: PaneId) -> bool {
    match event {
        PickEvent::Click(p) | PickEvent::Ended(p) => p == source,
        PickEvent::NewSession => true,
    }
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
enum Detached {
    Refused,
    Kept,
    /// The tab had only that pane: it was removed from the list.
    Emptied(usize, Tab),
}

/// Takes `source` out of its tab: the tab's focus goes to its first pane and its zoom ends
/// (`move_session_to_tab`); a tab left empty leaves the list.
fn detach(tabs: &mut Vec<Tab>, source: PaneId) -> Detached {
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

    /// The pointer moved: the half under it in the pane under it. A target in a Lock Layout
    /// window shows none (P5).
    pub(super) fn pane_move_hover(&mut self, x: f32, y: f32) {
        let Some(pm) = self.pane_move.as_ref().filter(|pm| !pm.drag) else { return };
        let (source, swap, old) = (pm.source, pm.swap, pm.hover);
        let hit = self.pick_rects().into_iter().find(|(_, r)| r.contains(x, y));
        let new = match hit {
            Some((p, r)) if p != source && !self.pane_window_forbids(p, crate::winmodel::LayoutOp::MovePane) => {
                let current = old.filter(|(hp, _)| *hp == p).map_or(Half::None, |(_, h)| h);
                let half = pane_move::half_at(current, swap, r.w, r.h, x - r.x, y - r.y, self.scale());
                (half != Half::None).then_some((p, half))
            }
            _ => None,
        };
        if new != old {
            if let Some(pm) = self.pane_move.as_mut() {
                pm.hover = new;
            }
            self.redraw();
            self.redraw_all();
        }
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
        if !same_tab {
            self.broadcast.forget(&[source, target]);
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
        self.broadcast.forget(&[source, target]);
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
    fn pick_mode_cancels() {
        assert!(cancel_on(PickEvent::Click(4), 4), "clicking the source cancels");
        assert!(!cancel_on(PickEvent::Click(5), 4), "clicking another pane drops, it does not cancel");
        assert!(cancel_on(PickEvent::NewSession, 4), "a new session cancels");
        assert!(cancel_on(PickEvent::Ended(4), 4), "the source ending cancels");
        assert!(!cancel_on(PickEvent::Ended(5), 4), "another pane ending does not");
    }
}

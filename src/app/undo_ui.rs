//! Undo Close in the window: closed sessions, tabs, and windows keep their shells running,
//! not drawn, until Session Close Undo Timeout passes; Shell > Undo Close brings the
//! newest back.

use std::time::{Duration, Instant};

use super::*;
use crate::undo_close::{self, Kind, Where};

/// A closed thing waiting in the undo stack.
pub(super) enum Undone {
    Session { pane: Pane, tab: u64, window: Option<WindowId> },
    Tab { tab: Tab, panes: Vec<Pane>, index: usize, window: Option<WindowId> },
    Window { tabs: Vec<Tab>, panes: Vec<Pane> },
}

/// A close in progress: the panes it takes, collected by `close_pane`.
pub(super) struct Capture {
    kind: Kind,
    tabs: Vec<(Tab, usize)>,
    tab_id: u64,
    window: Option<WindowId>,
    pub(super) panes: Vec<Pane>,
    timeout: Duration,
}

impl App {
    /// Starts collecting what a person's close takes (nothing when the profile's timeout is 0).
    pub(super) fn begin_undo(&mut self, kind: Kind, panes: &[PaneId]) {
        let secs = panes.first().and_then(|id| self.panes.get(id)).and_then(|p| self.store.get(&p.profile)).map_or(5.0, |p| p.float("Session Close Undo Timeout"));
        // A git command's short-lived window has an undo timeout of 0 (`openSingleUseWindow`).
        let secs = if panes.first().is_some_and(|id| self.short_lived(*id)) { 0.0 } else { secs };
        let Some(timeout) = undo_close::timeout(secs) else {
            self.undo_capture = None;
            return;
        };
        let tabs: Vec<(Tab, usize)> = self.tabs.iter().enumerate().filter(|(_, t)| t.layout.panes().iter().any(|p| panes.contains(p))).map(|(i, t)| (t.clone(), i)).collect();
        let tab_id = tabs.first().map_or(0, |(t, _)| t.id);
        self.undo_capture = Some(Capture { kind, tabs, tab_id, window: self.current_window(), panes: Vec::new(), timeout });
    }

    /// Ends the collection and puts the closed thing on the stack.
    pub(super) fn end_undo(&mut self) {
        let Some(c) = self.undo_capture.take() else { return };
        if c.panes.is_empty() {
            return;
        }
        let deadline = Instant::now() + c.timeout;
        let held: Vec<PaneId> = c.panes.iter().map(|p| p.id).collect();
        let c = Capture { tabs: keep_held(c.tabs, &held), ..c };
        let entry = match c.kind {
            Kind::Session => {
                let Some(pane) = c.panes.into_iter().next() else { return };
                Undone::Session { pane, tab: c.tab_id, window: c.window }
            }
            Kind::Tab => {
                let Some((tab, index)) = c.tabs.into_iter().next() else { return };
                Undone::Tab { tab, panes: c.panes, index, window: c.window }
            }
            Kind::Window => Undone::Window { tabs: c.tabs.into_iter().map(|(t, _)| t).collect(), panes: c.panes },
        };
        self.undo.push(entry, deadline);
    }

    /// The shells of closes past their deadline end now.
    pub(super) fn expire_undo(&mut self) -> Option<Instant> {
        for gone in self.undo.expire(Instant::now()) {
            self.end_undone(gone);
        }
        self.undo.next_deadline()
    }

    /// Quit: every waiting shell ends.
    pub(super) fn clear_undo(&mut self) {
        for gone in self.undo.clear() {
            self.end_undone(gone);
        }
    }

    fn end_undone(&mut self, gone: Undone) {
        let panes = match gone {
            Undone::Session { pane, .. } => vec![pane],
            Undone::Tab { panes, .. } | Undone::Window { panes, .. } => panes,
        };
        for mut p in panes {
            let guid = p.profile.clone();
            // A paused reader would hold up the pty's shutdown: let it drain first.
            p.set_paused(false);
            drop(p);
            self.forget_session_profile(&guid);
        }
    }

    fn window_alive(&self, id: Option<WindowId>) -> bool {
        id.is_some_and(|w| self.registry.order().contains(&windows_ui::key(w)))
    }

    fn put_back(&mut self, mut pane: Pane) {
        if let Some(pid) = pane.shell_pid() {
            self.jobs.watch(pid);
            if pane.owns_console() {
                self.jobs.watch_console(pid);
            }
        }
        pane.set_paused(false);
        self.panes.insert(pane.id, pane);
    }

    /// Brings the window an undone close went back into to the front.
    fn show_restored(&mut self) {
        self.bring_forward();
        if let Some(w) = self.current_window() {
            self.last_window = Some(w);
        }
    }

    /// Shell > Undo Close.
    pub(super) fn undo_close(&mut self) {
        let Some(entry) = self.undo.pop(Instant::now()) else { return };
        match entry {
            Undone::Session { pane, tab, window } => {
                let id = pane.id;
                let alive = self.window_alive(window);
                let tab_alive = alive && window.is_some_and(|w| self.enter(w)) && self.tabs.iter().any(|t| t.id == tab);
                self.put_back(pane);
                match undo_close::target(Kind::Session, alive, tab_alive) {
                    Where::SameTab => {
                        if let Some(i) = self.tabs.iter().position(|t| t.id == tab) {
                            let t = &mut self.tabs[i];
                            t.layout.split(t.focused, Axis::Row, id);
                            t.focused = id;
                            t.zoomed = None;
                            self.active = i;
                        }
                    }
                    _ => {
                        if let Some(w) = self.last_window {
                            self.enter(w);
                        }
                        self.tabs.push(Tab { id: self.next_tab_id, layout: Layout::new(id), focused: id, zoomed: None, title: None });
                        self.next_tab_id += 1;
                        self.active = self.tabs.len() - 1;
                    }
                }
            }
            Undone::Tab { tab, panes, index, window } => {
                for p in panes {
                    self.put_back(p);
                }
                if self.window_alive(window) && window.is_some_and(|w| self.enter(w)) {
                    let at = index.min(self.tabs.len());
                    self.tabs.insert(at, tab);
                    self.active = at;
                } else {
                    self.pending_windows.push(windows_ui::NewWindow::Tabs(vec![tab]));
                }
            }
            Undone::Window { tabs, panes } => {
                for p in panes {
                    self.put_back(p);
                }
                self.pending_windows.push(windows_ui::NewWindow::Tabs(tabs));
            }
        }
        // Profile or theme changes made while it waited reach it now.
        self.refresh_panes();
        self.show_restored();
        self.relayout();
        self.redraw();
    }
}

/// The closed tabs with only the panes the close held (a pane that is not undoable, such as
/// a loaded recording's, leaves its tab); a tab left with none does not come back.
fn keep_held(tabs: Vec<(Tab, usize)>, held: &[PaneId]) -> Vec<(Tab, usize)> {
    tabs.into_iter()
        .filter_map(|(t, i)| {
            let tree = t.layout.tree().retain(&mut |id| held.contains(id))?;
            let layout = Layout::from_tree(tree);
            let panes = layout.panes();
            let focused = if panes.contains(&t.focused) { t.focused } else { panes[0] };
            let zoomed = t.zoomed.filter(|z| panes.contains(z));
            Some((Tab { layout, focused, zoomed, ..t }, i))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_keeps_only_the_panes_it_holds() {
        // A loaded recording's pane is never undoable (`PS:3931`): the tab comes back
        // without it, and a tab of recordings only does not come back.
        let mut layout = Layout::new(1);
        layout.split(1, Axis::Row, 2);
        let mixed = Tab { id: 7, layout, focused: 2, zoomed: Some(2), title: None };
        let only = Tab { id: 8, layout: Layout::new(3), focused: 3, zoomed: None, title: None };
        let kept = keep_held(vec![(mixed, 0), (only, 1)], &[1]);
        assert_eq!(kept.len(), 1);
        let (t, i) = &kept[0];
        assert_eq!((t.id, *i), (7, 0));
        assert_eq!(t.layout.panes(), vec![1]);
        assert_eq!((t.focused, t.zoomed), (1, None));
    }
}

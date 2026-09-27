//! The Python API's requests of milestone 11b: `api_request` is the one match over them,
//! each carried out from a pure module's plan. Ids resolve through `api_ids` (`H:1615-1644`);
//! SendText types through `user_write`, Inject feeds output, Activate and Close follow
//! iTerm2's order (`api_session`).

use winit::event_loop::ActiveEventLoop;

use super::close_ui::Closing;
use super::App;
use crate::api_proto::{self, pb};
use crate::api_session::{self, CloseStep, Snapshot};
use crate::broadcast::Write;
use crate::layout::PaneId;
use pb::client_originated_message::Submessage as Req;
use pb::server_originated_message::Submessage as Resp;

/// A request's answer: sent now, or later by whatever finishes it (tmux replies).
pub(super) enum Answer {
    Now(pb::ServerOriginatedMessage),
    #[allow(dead_code)]
    Later,
}

/// The reply to `req` carrying `sub`.
fn reply(req: &pb::ClientOriginatedMessage, sub: Resp) -> Answer {
    let mut m = api_proto::reply_to(req);
    m.submessage = Some(sub);
    Answer::Now(m)
}

impl App {
    /// What ids name now: every window by number with its tabs (the current window's live
    /// fields, parked slots otherwise), the buried sessions, the exited ones, and the focused
    /// session of the current window (the last one used while it is open, as Focus reports).
    pub(super) fn api_ids(&self) -> Snapshot {
        let mut windows = Vec::new();
        for k in self.registry.order() {
            let (Some(guid), Some((tabs, _))) = (self.registry.guid(k), self.window_tabs(k)) else { continue };
            windows.push((k, guid.to_string(), tabs.iter().map(|t| (t.id, t.layout.panes())).collect()));
        }
        let active = self
            .last_window
            .map(super::windows_ui::key)
            .filter(|k| windows.iter().any(|(w, _, _)| w == k))
            .and_then(|k| self.window_tabs(k))
            .and_then(|(tabs, a)| tabs.get(a))
            .filter(|t| t.layout.panes().contains(&t.focused))
            .map(|t| t.focused);
        Snapshot {
            windows,
            buried: self.buried.items().into_iter().map(|(p, _)| p).collect(),
            exited: self.panes.iter().filter(|(_, p)| p.exited.is_some()).map(|(id, _)| *id).collect(),
            active,
        }
    }

    /// The app's answer to a request of 11b. A request no task handles yet answers iTerm2's
    /// malformed error. The window current before is current again after, as `api_model`
    /// leaves it (a raised window becomes current through `last_window`).
    pub(super) fn api_request(&mut self, _conn: u64, req: &pb::ClientOriginatedMessage, event_loop: &ActiveEventLoop) -> Answer {
        let back = self.current_window();
        let answer = match &req.submessage {
            Some(Req::SendTextRequest(r)) => reply(req, Resp::SendTextResponse(self.api_send_text(r))),
            Some(Req::InjectRequest(r)) => reply(req, Resp::InjectResponse(self.api_inject(r))),
            Some(Req::ActivateRequest(r)) => reply(req, Resp::ActivateResponse(self.api_activate(r, event_loop))),
            Some(Req::CloseRequest(r)) => reply(req, Resp::CloseResponse(self.api_close(r, event_loop))),
            _ => Answer::Now(api_proto::malformed(req)),
        };
        if let Some(b) = back {
            self.enter(b);
        }
        answer
    }

    /// SendText: typed into each session through `user_write` from its own window, so
    /// broadcast domains, tmux panes and gateways take it as typing; a session whose program
    /// exited is skipped ("all" names it too).
    fn api_send_text(&mut self, r: &pb::SendTextRequest) -> pb::SendTextResponse {
        let (resp, writes) = api_session::send_text(r, &self.api_ids());
        for (pane, bytes, broadcast) in writes {
            if bytes.is_empty() || self.panes.get(&pane).is_none_or(|p| p.exited.is_some()) {
                continue;
            }
            // A buried session is in no window: `user_write` then writes to it alone.
            self.enter_pane_window(pane);
            self.user_write(pane, &bytes, if broadcast { Write::SendText } else { Write::SendTextLocal });
        }
        resp
    }

    /// Inject: the bytes go to each session's screen as its program's output (`PS:17295-17300`).
    fn api_inject(&mut self, r: &pb::InjectRequest) -> pb::InjectResponse {
        let (resp, panes) = api_session::inject(r, &self.api_ids());
        let data = r.data.clone().unwrap_or_default();
        if panes.is_empty() || data.is_empty() {
            return resp;
        }
        for pane in panes {
            if let Some(p) = self.panes.get_mut(&pane) {
                p.feed_output(&data);
            }
        }
        self.take_requests();
        self.check_marks();
        self.redraw();
        resp
    }

    /// Restores a buried session where it was, opening the window it needs now.
    fn api_disinter(&mut self, pane: PaneId, event_loop: &ActiveEventLoop) {
        self.disinter(pane);
        for w in std::mem::take(&mut self.pending_windows) {
            self.open_pending(event_loop, w);
            self.tmux_apply_geometry();
        }
    }

    /// Activate, in iTerm2's order (`H:3011-3104`): a buried session is restored and its tab
    /// found (BAD_IDENTIFIER when it has none), the app is brought forward, then the window is
    /// raised, the tab selected and the session focused, stopping at the plan's error.
    fn api_activate(&mut self, r: &pb::ActivateRequest, event_loop: &ActiveEventLoop) -> pb::ActivateResponse {
        use pb::activate_response::Status;
        let answer = |s: Status| pb::ActivateResponse { status: Some(s as i32) };
        let mut plan = api_session::activate(r, &self.api_ids());
        if plan.error == Some(Status::BadIdentifier) {
            return answer(Status::BadIdentifier);
        }
        if plan.unbury {
            let Some(pane) = plan.pane else { return answer(Status::BadIdentifier) };
            self.api_disinter(pane, event_loop);
            let s = self.api_ids();
            plan.tab = s.pane_tab(pane);
            plan.window = plan.tab.and_then(|t| s.tab_window(t));
            if plan.tab.is_none() {
                return answer(Status::BadIdentifier);
            }
        }
        let window = plan.window.and_then(|k| self.window_id_of_key(k));
        if plan.activate_app {
            // `raise_all_windows` and `ignoring_other_apps` have no Windows meaning.
            // The target window, else the last one used; the current one when that is gone.
            if let Some(w) = window.or(self.last_window) {
                self.enter(w);
            }
            self.bring_forward();
        }
        if plan.raise {
            if let Some(w) = window {
                self.focus_terminal_window(w);
            }
        }
        if plan.select_tab {
            if let Some(tab) = plan.tab {
                if window.is_some_and(|w| self.enter(w)) {
                    if let Some(i) = self.tabs.iter().position(|t| t.id == tab) {
                        self.active = i;
                        self.relayout();
                    }
                }
            }
        }
        if plan.select_session {
            if let Some(pane) = plan.pane.filter(|p| self.enter_pane_window(*p)) {
                if let Some(t) = self.tabs.iter_mut().find(|t| t.layout.panes().contains(&pane)) {
                    // A maximized tab stays maximized, on the new session.
                    if t.zoomed.is_some() {
                        t.zoomed = Some(pane);
                    }
                    t.focused = pane;
                }
                self.relayout();
            }
        }
        self.redraw();
        answer(plan.error.unwrap_or(Status::Ok))
    }

    /// Close, one status per id (`H:4189-4255`). A target an earlier id of the same request
    /// already closed is NOT_FOUND, as iTerm2 no longer finds it.
    fn api_close(&mut self, r: &pb::CloseRequest, event_loop: &ActiveEventLoop) -> pb::CloseResponse {
        use pb::close_response::Status;
        let force = r.force.unwrap_or(false);
        let steps = api_session::close_steps(r, &self.api_ids());
        let mut results = Vec::new();
        for step in steps {
            let now = self.api_ids();
            let done = match step {
                CloseStep::Window(k) if now.windows.iter().any(|(w, _, _)| *w == k) => Some(self.api_close_window(k, force, event_loop)),
                CloseStep::Tab(t) if now.tab_window(t).is_some() => Some(self.api_close_tab(t, force, event_loop)),
                CloseStep::Session { pane, .. } if now.pane(&pane.to_string()).is_some() => Some(self.api_close_session(pane, force, event_loop)),
                _ => None,
            };
            results.push(match done {
                Some(true) => Status::Ok,
                Some(false) => Status::UserDeclined,
                None => Status::NotFound,
            });
        }
        api_session::close_response(&results)
    }

    /// Closes a window by its Registry key, undoable like Close Window. Without `force`,
    /// Tandem's confirmation and then tmux's question (which then carries the close out);
    /// false when the person declined.
    pub(super) fn api_close_window(&mut self, key: u64, force: bool, event_loop: &ActiveEventLoop) -> bool {
        let Some(id) = self.window_id_of_key(key) else { return false };
        if !self.enter(id) {
            return false;
        }
        if !force {
            if !self.confirm_close(Closing::Window) {
                return false;
            }
            if self.tmux_close_window() {
                return true;
            }
        }
        self.close_window_now(event_loop);
        true
    }

    /// Closes a tab by its id, undoable like Close Tab. Without `force`, tmux's question for a
    /// tmux tab (which then carries the close out), else Tandem's confirmation; false when the
    /// person declined.
    pub(super) fn api_close_tab(&mut self, tab: u64, force: bool, event_loop: &ActiveEventLoop) -> bool {
        let Some(k) = self.registry.order().into_iter().find(|k| self.window_tabs(*k).is_some_and(|(tabs, _)| tabs.iter().any(|t| t.id == tab))) else { return false };
        if !self.window_id_of_key(k).is_some_and(|w| self.enter(w)) {
            return false;
        }
        let Some(index) = self.tabs.iter().position(|t| t.id == tab) else { return false };
        if !force {
            if self.tmux_close_tab(index) {
                return true;
            }
            if !self.confirm_close(Closing::Tab(index)) {
                return false;
            }
        }
        self.close_tab_now(index, event_loop);
        true
    }

    /// Closes a session, undoable like Close Session (alone in its tab it closes as the tab);
    /// a buried one is restored first. Without `force`, Tandem's confirmation; false when the
    /// person declined.
    pub(super) fn api_close_session(&mut self, pane: PaneId, force: bool, event_loop: &ActiveEventLoop) -> bool {
        if self.buried.contains(pane) {
            self.api_disinter(pane, event_loop);
        }
        if !self.enter_pane_window(pane) {
            return false;
        }
        if !force && !self.confirm_close(Closing::Session(pane)) {
            return false;
        }
        self.close_session_undoable(pane, event_loop);
        true
    }
}

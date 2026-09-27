//! The Python API's requests of milestone 11b: `api_request` is the one match over them,
//! each carried out from a pure module's plan. Ids resolve through `api_ids` (`H:1615-1644`);
//! SendText types through `user_write`, Inject feeds output, Activate and Close follow
//! iTerm2's order (`api_session`); CreateTab and SplitPane start sessions from a
//! request's profile and custom properties (`api_create`).

use winit::event_loop::ActiveEventLoop;

use winit::window::WindowId;

use super::close_ui::Closing;
use super::{App, Rect, GAP};
use crate::api_create::{self, SplitResult};
use crate::api_proto::{self, pb};
use crate::api_session::{self, CloseStep, Snapshot};
use crate::broadcast::Write;
use crate::layout::{Axis, PaneId};
use crate::winmodel::LayoutOp;
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
            Some(Req::CreateTabRequest(r)) => reply(req, Resp::CreateTabResponse(self.api_create_tab(r, event_loop))),
            Some(Req::SplitPaneRequest(r)) => reply(req, Resp::SplitPaneResponse(self.api_split_pane(r))),
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
        let mut fed = Vec::new();
        for pane in panes {
            if let Some(p) = self.panes.get_mut(&pane) {
                p.feed_output(&data);
                if !fed.contains(&pane) {
                    fed.push(pane);
                }
            }
        }
        // The same step real output runs.
        self.after_output(&fed, std::time::Instant::now());
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
                self.order_front(w);
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
                CloseStep::Session { pane, unbury } if now.pane(&pane.to_string()).is_some() => {
                    // A buried session is restored before it closes (`H:4238-4240`).
                    if unbury {
                        self.api_disinter(pane, event_loop);
                    }
                    Some(self.api_close_session(pane, force, event_loop))
                }
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

    /// Closes a tab by its id, undoable like Close Tab. Without `force`, Lock Layout refuses
    /// it, then tmux's question for a tmux tab (which then carries the close out), else
    /// Tandem's confirmation; false when refused or declined.
    pub(super) fn api_close_tab(&mut self, tab: u64, force: bool, event_loop: &ActiveEventLoop) -> bool {
        let Some(k) = self.registry.order().into_iter().find(|k| self.window_tabs(*k).is_some_and(|(tabs, _)| tabs.iter().any(|t| t.id == tab))) else { return false };
        if !self.window_id_of_key(k).is_some_and(|w| self.enter(w)) {
            return false;
        }
        let Some(index) = self.tabs.iter().position(|t| t.id == tab) else { return false };
        if api_session::lock_refuses(CloseStep::Tab(tab), force, &self.win_locks) {
            return false;
        }
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

    /// Closes a session, undoable like Close Session (alone in its tab it closes as the tab).
    /// The caller restores a buried one first (`CloseStep::Session::unbury`); a session in no
    /// window is not closed. Without `force`, Lock Layout refuses it, else Tandem's
    /// confirmation; false when refused or declined.
    pub(super) fn api_close_session(&mut self, pane: PaneId, force: bool, event_loop: &ActiveEventLoop) -> bool {
        if !self.enter_pane_window(pane) {
            return false;
        }
        if api_session::lock_refuses(CloseStep::Session { pane, unbury: false }, force, &self.win_locks) {
            return false;
        }
        if !force && !self.confirm_close(Closing::Session(pane)) {
            return false;
        }
        self.close_session_undoable(pane, event_loop);
        true
    }

    /// The Guid of the profile a request names: the default one, else the first saved
    /// profile with that name; None when no profile has it.
    fn api_profile_guid(&self, name: Option<&str>) -> Option<String> {
        let by_name = |n: &str| self.store.profiles.iter().find(|p| p.name() == n).map(|p| p.guid());
        api_create::profile_guid(name, &by_name, &self.store.default_profile().guid())
    }

    /// A session copy (`store.sessions`) of the profile `guid` with `values` set, made before
    /// a session starts from it so a Command or folder applies; its Guid.
    pub(super) fn api_session_copy(&mut self, guid: &str, values: &[(String, Option<toml::Value>)]) -> String {
        let shared = self.store.get(guid).cloned().unwrap_or_else(|| self.store.default_profile().clone());
        let mut copy = crate::session_profile::divorce(&shared, uuid::Uuid::new_v4().to_string().to_uppercase());
        copy.set(values, &shared);
        let new_guid = copy.profile.guid();
        self.store.sessions.push(copy);
        new_guid
    }

    /// The profile a request starts its sessions from: the named (or default) profile, or a
    /// session copy of it when the request customizes it. Err with the name's status or the
    /// custom properties' status.
    fn api_start_profile<S>(&mut self, name: Option<&str>, props: &[pb::ProfileProperty], no_name: S, bad_props: S) -> Result<(String, Vec<(String, Option<toml::Value>)>), S> {
        let base = self.api_profile_guid(name).ok_or(no_name)?;
        let values = api_create::custom_values(props).ok_or(bad_props)?;
        Ok((base, values))
    }

    /// CreateTab (`H:2465-2588`): the window, then the profile, then a tab made in the window
    /// (or a new window when there is none, or the profile never opens in a tab), then the
    /// requested index.
    fn api_create_tab(&mut self, r: &pb::CreateTabRequest, event_loop: &ActiveEventLoop) -> pb::CreateTabResponse {
        use pb::create_tab_response::Status;
        let fail = |s: Status| pb::CreateTabResponse { status: Some(s as i32), ..Default::default() };
        let window = match &r.window_id {
            Some(id) => match self.api_ids().window_key(id).and_then(|k| self.window_id_of_key(k)) {
                Some(w) => Some(w),
                None => return fail(Status::InvalidWindowId),
            },
            None => None,
        };
        // A custom property whose JSON does not parse leaves iTerm2 no profile, so no session.
        let (base, values) = match self.api_start_profile(r.profile_name.as_deref(), &r.custom_profile_properties, Status::InvalidProfileName, Status::MissingSubstitution) {
            Ok(p) => p,
            Err(s) => return fail(s),
        };
        let guid = if values.is_empty() { base } else { self.api_session_copy(&base, &values) };
        let Some((w, pane)) = self.api_start_tab(window, &guid, r.select_tab.unwrap_or(true), event_loop) else {
            self.forget_session_profile(&guid);
            return fail(Status::MissingSubstitution);
        };
        // The tab's window is current now.
        let Some(index) = self.tabs.iter().position(|t| t.layout.panes().contains(&pane)) else { return fail(Status::MissingSubstitution) };
        let tab_id = self.tabs[index].id;
        let landed = match r.tab_index {
            Some(i) if (i as usize) < self.tabs.len() => {
                let to = i as usize;
                if to != index && self.layout_allowed(LayoutOp::ReorderTab) {
                    let active = self.tabs.get(self.active).map(|t| t.id);
                    let tab = self.tabs.remove(index);
                    self.tabs.insert(to, tab);
                    if let Some(a) = active.and_then(|a| self.tabs.iter().position(|t| t.id == a)) {
                        self.active = a;
                    }
                    self.tmux_reorder();
                    self.relayout();
                    self.redraw();
                }
                self.tabs.iter().position(|t| t.id == tab_id)
            }
            _ => None,
        };
        pb::CreateTabResponse {
            status: Some(api_create::tab_index_status(r.tab_index, self.tabs.len(), landed) as i32),
            window_id: self.registry.guid(super::windows_ui::key(w)).map(str::to_string),
            tab_id: Some(tab_id as i32),
            session_id: Some(pane.to_string()),
        }
    }

    /// Makes CreateTab's tab with profile `guid`: in `window` unless its profile never opens
    /// in a tab, else in a new window (which comes to the front). In an existing window
    /// `select` false keeps the tab in front and does not raise the window. The window it is
    /// in (current on return) and its session; None when no session started.
    fn api_start_tab(&mut self, window: Option<WindowId>, guid: &str, select: bool, event_loop: &ActiveEventLoop) -> Option<(WindowId, PaneId)> {
        if let Some(w) = window {
            if !self.enter(w) {
                return None;
            }
            if !self.new_tab_opens_window(Some(guid)) {
                let (front, count) = (self.active, self.tabs.len());
                self.new_tab(Some(guid));
                if self.tabs.len() == count {
                    return None;
                }
                let pane = self.tabs.last()?.focused;
                if select {
                    self.focus_terminal_window(w);
                } else {
                    self.active = front;
                    self.relayout();
                }
                self.redraw();
                return Some((w, pane));
            }
        }
        match self.open_window(event_loop, Some(guid)) {
            Ok(w) => {
                self.last_window = Some(w);
                Some((w, self.tabs.first()?.focused))
            }
            Err(e) => {
                self.banners.set("window", format!("Could not open a window: {e}"));
                self.redraw();
                None
            }
        }
    }

    /// SplitPane (`H:2608-2677`): every target must be in a window (SESSION_NOT_FOUND), then
    /// the profile and its custom properties, then one split per target, each new session
    /// with its own session copy.
    fn api_split_pane(&mut self, r: &pb::SplitPaneRequest) -> pb::SplitPaneResponse {
        use pb::split_pane_response::Status;
        let fail = |s: Status| pb::SplitPaneResponse { status: Some(s as i32), session_id: Vec::new() };
        let ids = self.api_ids();
        let session = r.session.as_deref().unwrap_or_default();
        let targets = if session == api_session::ALL {
            ids.all_panes()
        } else {
            match ids.pane(session) {
                Some(p) => vec![p],
                None => return fail(Status::SessionNotFound),
            }
        };
        // A buried session is in no window.
        if targets.iter().any(|p| ids.pane_tab(*p).is_none()) {
            return fail(Status::SessionNotFound);
        }
        let (base, values) = match self.api_start_profile(r.profile_name.as_deref(), &r.custom_profile_properties, Status::InvalidProfileName, Status::MalformedCustomProfileProperty) {
            Ok(p) => p,
            Err(s) => return fail(s),
        };
        let axis = api_create::split_axis(r.split_direction.unwrap_or(0));
        let before = r.before.unwrap_or(false);
        let mut results = Vec::new();
        for target in targets {
            let guid = if values.is_empty() { base.clone() } else { self.api_session_copy(&base, &values) };
            let result = self.api_split(target, axis, before, &guid);
            if !matches!(result, SplitResult::Made(_)) {
                self.forget_session_profile(&guid);
            }
            results.push(result);
        }
        self.redraw();
        api_create::split_response(&results)
    }

    /// Splits `target` with a new session of profile `guid`, the new one first when `before`
    /// (a split, then a swap of the two). iTerm2's order (`PT:11414-11470`): Lock Layout
    /// refuses first (a tmux pane's refusal is still no CANNOT_SPLIT), a tmux pane asks tmux,
    /// then Instant Replay refuses (`PT:11124-11129`). Focus moves to the new session, as
    /// Split does.
    pub(super) fn api_split(&mut self, target: PaneId, axis: Axis, before: bool, guid: &str) -> SplitResult {
        if !self.enter_pane_window(target) {
            return SplitResult::Refused;
        }
        if !self.layout_allowed(LayoutOp::Split) {
            return if self.tmux_of(target).is_some() { SplitResult::Tmux } else { SplitResult::Refused };
        }
        if self.tmux_split(target, axis == Axis::Row) {
            return SplitResult::Tmux;
        }
        if crate::replay::on_event(self.in_instant_replay(), crate::replay::ReplayEvent::Split) == crate::replay::Close::Refuse {
            return SplitResult::Refused;
        }
        let Some(index) = self.tabs.iter().position(|t| t.layout.panes().contains(&target)) else { return SplitResult::Refused };
        let area = self.content_area_for(&self.tabs[index]);
        let Some((_, rect)) = self.tabs[index].layout.rects(area, GAP).into_iter().find(|(id, _)| *id == target) else { return SplitResult::Refused };
        let half = match axis {
            Axis::Row => Rect { w: rect.w / 2.0, ..rect },
            Axis::Column => Rect { h: rect.h / 2.0, ..rect },
        };
        self.start_tab = Some(index);
        let started = self.start_pane(half, Some(guid));
        self.start_tab = None;
        let Some(id) = started else { return SplitResult::Refused };
        let tab = &mut self.tabs[index];
        tab.layout.split(target, axis, id);
        if before {
            tab.layout.swap(target, id);
        }
        tab.focused = id;
        tab.zoomed = None;
        self.relayout();
        SplitResult::Made(id)
    }
}

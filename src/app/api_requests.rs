//! The Python API's requests of milestone 11b: `api_request` is the one match over them,
//! each carried out from a pure module's plan. Ids resolve through `api_ids` (`H:1615-1644`);
//! SendText types through `user_write`, Inject feeds output, Activate and Close follow
//! iTerm2's order (`api_session`); CreateTab and SplitPane start sessions from a
//! request's profile and custom properties (`api_create`); GetBuffer, GetPrompt and
//! ListPrompts read a session's own screen, Screenshot renders its visible lines
//! (`api_buffer`); VariableRequest and the VARIABLE_CHANGE subscriptions read the objects
//! ids name (`api_vars`); GetProperty and SetProperty read and change a window's frame and
//! full screen and a session's grid size and burial (`api_props`); GetProfileProperty,
//! SetProfileProperty, ListProfiles and Preferences read and change profiles and settings
//! (`api_profile`); SavedArrangement, RestartSession, SetTabLayout and ReorderTabs save and
//! restore arrangements, start sessions again, set split sizes and move tabs (`api_layout`);
//! MenuItem finds a menu item by its XIB identifier and runs it as the menu does (`menus`).

use winit::event_loop::ActiveEventLoop;

use winit::window::WindowId;

use super::close_ui::Closing;
use super::{App, Rect, GAP};
use crate::api_buffer;
use crate::api_create::{self, SplitResult};
use crate::api_layout::{self, Arrange};
use crate::api_profile;
use crate::api_props::{self, BuryStep, Prop};
use crate::api_proto::{self, pb};
use crate::api_session::{self, CloseStep, Snapshot};
use crate::api_vars::{self, Objects, Scope, Values};
use crate::broadcast::Write;
use crate::layout::{Axis, PaneId};
use crate::menus;
use crate::settings_ui::Edit;
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
    pub(super) fn api_request(&mut self, conn: u64, req: &pb::ClientOriginatedMessage, event_loop: &ActiveEventLoop) -> Answer {
        let back = self.current_window();
        let answer = match &req.submessage {
            Some(Req::SendTextRequest(r)) => reply(req, Resp::SendTextResponse(self.api_send_text(r))),
            Some(Req::InjectRequest(r)) => reply(req, Resp::InjectResponse(self.api_inject(r))),
            Some(Req::ActivateRequest(r)) => reply(req, Resp::ActivateResponse(self.api_activate(r, event_loop))),
            Some(Req::CloseRequest(r)) => reply(req, Resp::CloseResponse(self.api_close(r, event_loop))),
            Some(Req::CreateTabRequest(r)) => reply(req, Resp::CreateTabResponse(self.api_create_tab(r, event_loop))),
            Some(Req::SplitPaneRequest(r)) => reply(req, Resp::SplitPaneResponse(self.api_split_pane(r))),
            Some(Req::GetBufferRequest(r)) => self.api_get_buffer(req, r),
            Some(Req::GetPromptRequest(r)) => reply(req, Resp::GetPromptResponse(self.api_get_prompt(r))),
            Some(Req::ListPromptsRequest(r)) => reply(req, Resp::ListPromptsResponse(self.api_list_prompts(r))),
            Some(Req::ScreenshotRequest(r)) => reply(req, Resp::ScreenshotResponse(self.api_screenshot(r))),
            Some(Req::VariableRequest(r)) => reply(req, Resp::VariableResponse(self.api_variable(r))),
            Some(Req::GetPropertyRequest(r)) => reply(req, Resp::GetPropertyResponse(self.api_get_property(r))),
            Some(Req::SetPropertyRequest(r)) => {
                let status = self.api_set_property(r, event_loop);
                reply(req, Resp::SetPropertyResponse(pb::SetPropertyResponse { status: Some(status as i32) }))
            }
            Some(Req::GetProfilePropertyRequest(r)) => reply(req, Resp::GetProfilePropertyResponse(self.api_get_profile_property(r))),
            Some(Req::SetProfilePropertyRequest(r)) => {
                let status = self.api_set_profile_property(r);
                reply(req, Resp::SetProfilePropertyResponse(pb::SetProfilePropertyResponse { status: Some(status as i32) }))
            }
            Some(Req::ListProfilesRequest(r)) => reply(req, Resp::ListProfilesResponse(api_profile::list_profiles(&self.store.profiles, &r.properties, &r.guids))),
            Some(Req::PreferencesRequest(r)) => reply(req, Resp::PreferencesResponse(self.api_preferences(r))),
            Some(Req::SavedArrangementRequest(r)) => reply(req, Resp::SavedArrangementResponse(self.api_saved_arrangement(r))),
            Some(Req::RestartSessionRequest(r)) => {
                let status = self.api_restart_session(r);
                reply(req, Resp::RestartSessionResponse(pb::RestartSessionResponse { status: Some(status as i32) }))
            }
            Some(Req::SetTabLayoutRequest(r)) => {
                let status = self.api_set_tab_layout(r);
                reply(req, Resp::SetTabLayoutResponse(pb::SetTabLayoutResponse { status: Some(status as i32) }))
            }
            Some(Req::ReorderTabsRequest(r)) => {
                let status = self.api_reorder_tabs(r);
                reply(req, Resp::ReorderTabsResponse(pb::ReorderTabsResponse { status: Some(status as i32) }))
            }
            Some(Req::MenuItemRequest(r)) => reply(req, Resp::MenuItemResponse(self.api_menu_item(r, event_loop))),
            Some(Req::NotificationRequest(r)) if r.notification_type == Some(pb::NotificationType::NotifyOnVariableChange as i32) => {
                let status = self.api_variable_subscribe(conn, r);
                reply(req, Resp::NotificationResponse(pb::NotificationResponse { status: Some(status as i32) }))
            }
            _ => Answer::Now(api_proto::malformed(req)),
        };
        if let Some(b) = back {
            self.enter(b);
        }
        answer
    }

    /// The objects ids name now, their values read when asked for.
    pub(super) fn api_objects(&self) -> ApiObjects<'_> {
        ApiObjects { app: self, ids: self.api_ids() }
    }

    /// VariableRequest (`H:3106-3267`): the plan's sets go to the store, the only place a
    /// script writes variables (user variables alone, as iTerm2 allows).
    fn api_variable(&mut self, r: &pb::VariableRequest) -> pb::VariableResponse {
        let (resp, sets) = api_vars::answer(r, &self.api_objects());
        if sets.is_empty() {
            return resp;
        }
        for (scope, id, name, value) in sets {
            // `answer` passes only legal "user." names, which the store always takes.
            let _ = self.api_vars.set_user(scope, &id, &name, value);
        }
        // Titles and status bars may interpolate them.
        self.redraw();
        self.redraw_all();
        resp
    }

    /// A VARIABLE_CHANGE (un)subscribe (`H:1804-1864`). The objects a new subscription
    /// watches take their baseline now, so a change the script makes right after is posted.
    fn api_variable_subscribe(&mut self, conn: u64, r: &pb::NotificationRequest) -> pb::notification_response::Status {
        let mut subs = std::mem::take(&mut self.api_var_subs);
        let objects = self.api_objects();
        let status = subs.request(conn, r, &objects);
        let mut now = Vec::new();
        if status == pb::notification_response::Status::Ok && r.subscribe == Some(true) {
            for (s, id) in subs.watched(&objects) {
                let v = objects.values(s, &id);
                now.push((s, id, v));
            }
        }
        self.api_var_subs = subs;
        for (s, id, v) in now {
            self.api_vars.baseline(s, &id, v);
        }
        status
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

    /// The session a request names ("all" names none), buried ones too.
    fn api_session_pane(&self, session: Option<&str>) -> Option<PaneId> {
        self.api_ids().pane(session.unwrap_or_default()).filter(|p| self.panes.contains_key(p))
    }

    /// GetBuffer (`PS:22416-22488`) from the session's own screen (also while a view or
    /// Instant Replay shows, `api_buffer::get_buffer`). An invalid range answers with no
    /// GetBuffer response (`S:739-751`).
    fn api_get_buffer(&mut self, req: &pb::ClientOriginatedMessage, r: &pb::GetBufferRequest) -> Answer {
        let Some(p) = self.api_session_pane(r.session.as_deref()).and_then(|id| self.panes.get(&id)) else {
            let status = Some(pb::get_buffer_response::Status::SessionNotFound as i32);
            return reply(req, Resp::GetBufferResponse(pb::GetBufferResponse { status, ..Default::default() }));
        };
        match api_buffer::get_buffer(p.own(), r.line_range.as_ref(), r.include_styles.unwrap_or(false)) {
            Some(resp) => reply(req, Resp::GetBufferResponse(resp)),
            None => Answer::Now(api_proto::reply_to(req)),
        }
    }

    /// GetPrompt (`PS:22500-22574`): the prompt mark with the given id, else the newest one;
    /// an id that names no prompt mark is PROMPT_UNAVAILABLE.
    fn api_get_prompt(&self, r: &pb::GetPromptRequest) -> pb::GetPromptResponse {
        let Some(p) = self.api_session_pane(r.session.as_deref()).and_then(|id| self.panes.get(&id)) else {
            return pb::GetPromptResponse { status: Some(pb::get_prompt_response::Status::SessionNotFound as i32), ..Default::default() };
        };
        let screen = p.own();
        let facts = match r.unique_prompt_id.as_deref() {
            Some(s) => s.parse::<u32>().ok().filter(|id| id.to_string() == s).and_then(|id| screen.prompt_facts(Some(id))),
            None => screen.prompt_facts(None),
        };
        api_buffer::prompt_response(facts.as_ref())
    }

    /// ListPrompts (`PS:22490-22498`): the prompt ids, status left unset as iTerm2 leaves it.
    fn api_list_prompts(&mut self, r: &pb::ListPromptsRequest) -> pb::ListPromptsResponse {
        let Some(p) = self.api_session_pane(r.session.as_deref()).and_then(|id| self.panes.get_mut(&id)) else {
            return pb::ListPromptsResponse { status: Some(pb::list_prompts_response::Status::SessionNotFound as i32), ..Default::default() };
        };
        let ids = p.own_mut().prompt_ids();
        pb::ListPromptsResponse { status: None, unique_prompt_id: api_buffer::prompt_ids(&ids, r.first_unique_id.as_deref(), r.last_unique_id.as_deref()) }
    }

    /// Screenshot (`H:1658-1676`, `PS:9118-9124`): the lines the session's own screen shows
    /// (`rangeOfVisibleLines`) with its cursor (`showCursor:YES`), rendered alone by its
    /// window (a buried session by the current one) as PNG; INTERNAL_ERROR when there is no
    /// window or rendering fails.
    fn api_screenshot(&mut self, r: &pb::ScreenshotRequest) -> pb::ScreenshotResponse {
        use alacritty_terminal::grid::Dimensions;
        use pb::screenshot_response::Status;
        let answer = |s: Status, png: Option<Vec<u8>>| pb::ScreenshotResponse { status: Some(s as i32), png };
        let Some(pane) = self.api_session_pane(r.session.as_deref()) else { return answer(Status::SessionNotFound, None) };
        self.enter_pane_window(pane);
        let Some(p) = self.panes.get(&pane) else { return answer(Status::SessionNotFound, None) };
        let visible = crate::screenshot::visible(p.own());
        let rows = visible.term().screen_lines();
        match self.render_lines(pane, &visible, 0, rows.max(1) - 1, true).and_then(|(w, h, px)| crate::screenshot::png(w, h, px)) {
            Ok(png) => answer(Status::Ok, Some(png)),
            Err(_) => answer(Status::InternalError, None),
        }
    }

    /// GetProperty (`H:2852-2984`): a window's frame (physical pixels, y down) and full
    /// screen, a session's grid size (one less each way, kept), burial and line counts from its
    /// own screen; a target not found (session "all" too) is INVALID_TARGET, a name the target
    /// does not have UNRECOGNIZED_NAME.
    fn api_get_property(&mut self, r: &pb::GetPropertyRequest) -> pb::GetPropertyResponse {
        use alacritty_terminal::grid::Dimensions;
        use pb::get_property_request::Identifier;
        use pb::get_property_response::Status;
        let answer = |s: Status, json_value: Option<String>| pb::GetPropertyResponse { status: Some(s as i32), json_value };
        let name = r.name.as_deref().unwrap_or_default();
        let json = match &r.identifier {
            None => return answer(Status::InvalidTarget, None),
            Some(Identifier::WindowId(id)) => {
                let Some(w) = self.api_ids().window_key(id).and_then(|k| self.window_id_of_key(k)) else { return answer(Status::InvalidTarget, None) };
                if !self.enter(w) {
                    return answer(Status::InvalidTarget, None);
                }
                let Some(g) = self.gpu.as_ref() else { return answer(Status::InvalidTarget, None) };
                match api_props::window_prop(name) {
                    Some(Prop::Frame) => {
                        let p = g.window.outer_position().unwrap_or_default();
                        let s = g.window.outer_size();
                        Some(api_props::frame_json(p.x.into(), p.y.into(), s.width.into(), s.height.into()))
                    }
                    Some(Prop::Fullscreen) => Some(g.window.fullscreen().is_some().to_string()),
                    _ => None,
                }
            }
            Some(Identifier::SessionId(id)) => {
                let Some(pane) = self.api_session_pane(Some(id)) else { return answer(Status::InvalidTarget, None) };
                let Some(p) = self.panes.get(&pane) else { return answer(Status::InvalidTarget, None) };
                let screen = p.own();
                let g = screen.term().grid();
                match api_props::session_prop(name, false) {
                    Some(Prop::GridSize) => Some(api_props::grid_json(g.columns(), g.screen_lines())),
                    Some(Prop::Buried) => Some(api_props::is_buried(self.buried.contains(pane), self.api_tmux_hidden(pane)).to_string()),
                    Some(Prop::NumberOfLines) => {
                        let (off, history) = (g.scrolled_off() as i64, g.history_size() as i64);
                        Some(api_props::lines_json(off - history, g.screen_lines() as i64, history, off - g.display_offset() as i64))
                    }
                    _ => None,
                }
            }
        };
        match json {
            Some(j) => answer(Status::Ok, Some(j)),
            None => answer(Status::UnrecognizedName, None),
        }
    }

    /// SetProperty (`H:2690-2850`): a value that is not JSON is INVALID_VALUE before
    /// anything else, then the target (INVALID_TARGET), then the name (UNRECOGNIZED_NAME),
    /// then the value's shape (INVALID_VALUE). Session "all" sets the first session only
    /// (`H:2723-2729`, kept).
    fn api_set_property(&mut self, r: &pb::SetPropertyRequest, event_loop: &ActiveEventLoop) -> pb::set_property_response::Status {
        use pb::set_property_request::Identifier;
        use pb::set_property_response::Status;
        let Some(value) = api_props::parse_value(r.json_value.as_deref().unwrap_or_default()) else { return Status::InvalidValue };
        let name = r.name.as_deref().unwrap_or_default();
        let ids = self.api_ids();
        match &r.identifier {
            None => Status::InvalidTarget,
            Some(Identifier::WindowId(id)) => {
                let Some(w) = ids.window_key(id).and_then(|k| self.window_id_of_key(k)) else { return Status::InvalidTarget };
                if !self.enter(w) {
                    return Status::InvalidTarget;
                }
                let Some(g) = self.gpu.as_ref() else { return Status::InvalidTarget };
                match api_props::window_prop(name) {
                    // iTerm2 answers INVALID_VALUE and then sets a zero frame; one answer here.
                    Some(Prop::Frame) => {
                        let Some((x, y, w, h)) = api_props::parse_frame(&value) else { return Status::InvalidValue };
                        // The outer size asked for, less the window's frame thickness.
                        let (outer, inner) = (g.window.outer_size(), g.window.inner_size());
                        let (dw, dh) = (f64::from(outer.width) - f64::from(inner.width), f64::from(outer.height) - f64::from(inner.height));
                        g.window.set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
                        let size = winit::dpi::PhysicalSize::new((w - dw).round().max(1.0) as u32, (h - dh).round().max(1.0) as u32);
                        let _ = g.window.request_inner_size(size);
                        Status::Ok
                    }
                    Some(Prop::Fullscreen) => {
                        let Some(on) = api_props::as_bool(&value) else { return Status::InvalidValue };
                        if g.window.fullscreen().is_some() != on {
                            super::menu::toggle_fullscreen(Some(&*g.window));
                        }
                        Status::Ok
                    }
                    _ => Status::UnrecognizedName,
                }
            }
            Some(Identifier::SessionId(id)) => {
                let pane = if id == api_session::ALL {
                    // With no session iTerm2 falls through to INVALID_TARGET.
                    match ids.all_panes().first() {
                        Some(p) => *p,
                        None => return Status::InvalidTarget,
                    }
                } else {
                    match self.api_session_pane(Some(id)) {
                        Some(p) => p,
                        None => return Status::InvalidTarget,
                    }
                };
                match api_props::session_prop(name, true) {
                    Some(Prop::GridSize) => match api_props::parse_size(&value) {
                        Some((cols, rows)) => self.api_set_grid_size(pane, cols, rows),
                        None => Status::InvalidValue,
                    },
                    Some(Prop::Buried) => {
                        let Some(bury) = api_props::as_bool(&value) else { return Status::InvalidValue };
                        match api_props::bury_step(bury, self.buried.contains(pane), self.api_tmux_hidden(pane)) {
                            BuryStep::Nothing => {}
                            // A replay view is never buried (`bury`'s `_synthetic`); a tmux pane
                            // hides its tmux window. iTerm2 answers OK either way.
                            BuryStep::Bury => {
                                if self.enter_pane_window(pane) && !self.bury_refused(pane) {
                                    self.bury_pane(pane);
                                }
                            }
                            BuryStep::Restore => self.api_disinter(pane, event_loop),
                            // As Session > Buried Sessions shows a hidden tmux window.
                            BuryStep::ShowTmux => {
                                if let Some((gw, _, window)) = self.tmux_of(pane) {
                                    if let Some(t) = self.tmux.get_mut(&gw) {
                                        t.ctl.open_windows(&[window], false, &mut t.gw);
                                    }
                                    self.tmux_flush(gw);
                                }
                            }
                        }
                        self.redraw();
                        Status::Ok
                    }
                    _ => Status::UnrecognizedName,
                }
            }
        }
    }

    /// GetProfileProperty (`H:2354-2365`, `PS:22595-22615`): the keys asked (all when none
    /// are) from the profile the session uses, its own copy when it has one.
    fn api_get_profile_property(&self, r: &pb::GetProfilePropertyRequest) -> pb::GetProfilePropertyResponse {
        use pb::get_profile_property_response::Status;
        let Some(p) = self.api_session_pane(r.session.as_deref()).and_then(|id| self.panes.get(&id)) else {
            return pb::GetProfilePropertyResponse { status: Some(Status::SessionNotFound as i32), properties: Vec::new() };
        };
        let profile = self.store.get(&p.profile).unwrap_or_else(|| self.store.default_profile());
        pb::GetProfilePropertyResponse { status: Some(Status::Ok as i32), properties: api_profile::properties(profile, &r.keys) }
    }

    /// SetProfileProperty (`H:2245-2352`), in iTerm2's order: the target (a guid list's
    /// unknown guid BAD_GUID before anything is set, a session SESSION_NOT_FOUND), then every
    /// value's JSON, then its legality (REQUEST_MALFORMED); with nothing targeted nothing is
    /// checked further. A session's values go to its own copy of its profile, a saved
    /// profile's through the same edit the Settings window makes.
    fn api_set_profile_property(&mut self, r: &pb::SetProfilePropertyRequest) -> pb::set_profile_property_response::Status {
        use pb::set_profile_property_request::Target;
        use pb::set_profile_property_response::Status;
        let (guids, panes) = match &r.target {
            None => return Status::RequestMalformed,
            Some(Target::GuidList(list)) => {
                if list.guids.iter().any(|g| self.store.get_mut(g).is_none()) {
                    return Status::BadGuid;
                }
                (list.guids.clone(), Vec::new())
            }
            Some(Target::Session(s)) if s == api_session::ALL => (Vec::new(), self.api_ids().all_panes()),
            Some(Target::Session(s)) => match self.api_session_pane(Some(s)) {
                Some(p) => (Vec::new(), vec![p]),
                None => return Status::SessionNotFound,
            },
        };
        let Some(parsed) = api_profile::assignments(r) else { return Status::RequestMalformed };
        // iTerm2 checks legality per object, so with none nothing is refused.
        if guids.is_empty() && panes.is_empty() {
            return Status::Ok;
        }
        let Some(values) = api_profile::legal(&parsed) else { return Status::RequestMalformed };
        if let Some(first) = guids.first().cloned() {
            for g in &guids {
                if let Some(p) = self.store.get_mut(g) {
                    for (k, v) in &values {
                        p.set(k, v.clone());
                    }
                }
            }
            self.apply_edit(Edit::Changed(first));
        }
        let session_values: Vec<(String, Option<toml::Value>)> = values.into_iter().map(|(k, v)| (k, Some(v))).collect();
        for pane in panes {
            self.set_session_values(pane, &session_values);
        }
        self.redraw();
        Status::Ok
    }

    /// Preferences (`H:3802-3881`): one result per request, in order. Settings that changed
    /// are saved and applied once, after the last request; "Enable Python API" is acted on
    /// when the dispatch loop settles, after the answer went out. Stored consents and
    /// program paths are refused and `setCookie` on waits for the Confirm alert
    /// (`api_profile::preference_step`); the default profile must be a saved profile
    /// (BAD_GUID).
    fn api_preferences(&mut self, r: &pb::PreferencesRequest) -> pb::PreferencesResponse {
        use api_profile::PrefStep;
        use pb::preferences_request::request::Request as Q;
        use pb::preferences_response::result::{self as res, Result as A};
        let mut results = Vec::new();
        let mut globals_changed = false;
        for q in &r.requests {
            let result = match &q.request {
                None => A::UnrecognizedRequest(res::UnrecognizedResult {}),
                Some(Q::GetPreferenceRequest(g)) => {
                    let key = g.key.as_deref().unwrap_or_default();
                    A::GetPreferenceResult(res::GetPreferenceResult { json_value: api_profile::preference_json(self.globals.get(key).as_ref()) })
                }
                Some(Q::SetPreferenceRequest(s)) => {
                    use res::set_preference_result::Status;
                    let key = s.key.as_deref().unwrap_or_default();
                    let status = match api_profile::preference_value(s.json_value.as_deref().unwrap_or_default()) {
                        Err(status) => status,
                        Ok(v) => match api_profile::preference_step(key, &v, self.globals.bool(super::api_ui::SET_COOKIE_KEY)) {
                            PrefStep::Refuse => Status::InvalidValue,
                            PrefStep::Confirm => {
                                self.api_confirm_cookie();
                                Status::Ok
                            }
                            PrefStep::Store => {
                                self.globals.set(key, v);
                                globals_changed = true;
                                Status::Ok
                            }
                        },
                    };
                    A::SetPreferenceResult(res::SetPreferenceResult { status: Some(status as i32) })
                }
                Some(Q::GetDefaultProfileRequest(_)) => A::GetDefaultProfileResult(res::GetDefaultProfileResult { guid: Some(self.store.default_profile().guid()) }),
                Some(Q::SetDefaultProfileRequest(d)) => {
                    use res::set_default_profile_result::Status;
                    let guid = d.guid.clone().unwrap_or_default();
                    let status = if self.store.get_mut(&guid).is_some() {
                        self.store.default_guid = guid.clone();
                        self.apply_edit(Edit::Changed(guid));
                        Status::Ok
                    } else {
                        Status::BadGuid
                    };
                    A::SetDefaultProfileResult(res::SetDefaultProfileResult { status: Some(status as i32) })
                }
            };
            results.push(pb::preferences_response::Result { result: Some(result) });
        }
        if globals_changed {
            // As `apply_edit(Edit::GlobalsChanged)`, with the API settings left to the settle.
            if let Some(w) = self.last_window {
                self.enter(w);
            }
            self.globals_changed(false);
            self.relayout();
            self.relayout_changed_windows();
        }
        self.redraw();
        pb::PreferencesResponse { results }
    }

    /// SavedArrangement (`H:3269-3336`): a save replaces without asking (`IC:493-523`), after
    /// entering the window named (then only it is saved); a restore needs a saved arrangement
    /// with at least one window (`IC:579-589`) and restores as tabs in the window named; a list
    /// has the names in the store's order.
    fn api_saved_arrangement(&mut self, r: &pb::SavedArrangementRequest) -> pb::SavedArrangementResponse {
        use pb::saved_arrangement_response::Status;
        let answer = |s: Status, names: Vec<String>| pb::SavedArrangementResponse { status: Some(s as i32), names };
        let ids = self.api_ids();
        let window = |id: &Option<String>| id.as_deref().and_then(|w| ids.window_key(w)).and_then(|k| self.window_id_of_key(k));
        match api_layout::arrange(r, &|w| ids.window_key(w).is_some()) {
            Err(s) => answer(s, Vec::new()),
            Ok(Arrange::List) => answer(Status::Ok, self.arrangements().names().iter().map(|n| n.to_string()).collect()),
            Ok(Arrange::Save { name, window: w }) => {
                if let Some(id) = window(&w) {
                    self.enter(id);
                }
                self.store_arrangement(&name, w.is_none());
                answer(Status::Ok, Vec::new())
            }
            Ok(Arrange::Restore { name, window: w }) => {
                if !self.arrangements().get(&name).is_some_and(|windows| !windows.is_empty()) {
                    return answer(Status::ArrangementNotFound, Vec::new());
                }
                if let Some(id) = window(&w) {
                    self.enter(id);
                }
                self.restore_arrangement(&name, w.is_some());
                answer(Status::Ok, Vec::new())
            }
        }
    }

    /// RestartSession (`H:3497-3522`): buried sessions are found; tmux panes, gateways and
    /// loaded recordings have no program to run again (`isRestartable`). The session's window
    /// is entered first, so the new shell gets that window's numbers.
    fn api_restart_session(&mut self, r: &pb::RestartSessionRequest) -> pb::restart_session_response::Status {
        use pb::restart_session_response::Status;
        let pane = self.api_session_pane(r.session_id.as_deref());
        let (restartable, exited) = pane.and_then(|p| self.panes.get(&p)).map_or((false, false), |p| (p.tmux_io().is_none() && !p.gateway && !p.dvr.read_only, p.exited.is_some()));
        let status = api_layout::restart_status(pane.is_some(), restartable, r.only_if_exited.unwrap_or(false), exited);
        if let (Status::Ok, Some(p)) = (status, pane) {
            self.enter_pane_window(p);
            self.restart_pane(p);
            self.redraw();
        }
        status
    }

    /// SetTabLayout (`H:3575-3596`): the tab's tree as `node_of` flattens it must be the
    /// requested one's shape (WRONG_TREE); then the split ratios come from the requested grid
    /// sizes and the window keeps its size. A tmux tab, and a tab with a maximized pane (whose
    /// tree shows that pane alone), answer OK and change nothing.
    fn api_set_tab_layout(&mut self, r: &pb::SetTabLayoutRequest) -> pb::set_tab_layout_response::Status {
        use pb::set_tab_layout_response::Status;
        let ids = self.api_ids();
        let Some(tab) = r.tab_id.as_deref().and_then(|t| ids.tab_by_id(t)) else { return Status::BadTabId };
        let Some(w) = ids.tab_window(tab).and_then(|k| self.window_id_of_key(k)) else { return Status::BadTabId };
        if !self.enter(w) {
            return Status::BadTabId;
        }
        let Some(index) = self.tabs.iter().position(|t| t.id == tab) else { return Status::BadTabId };
        let t = &self.tabs[index];
        let blank = |p: PaneId| crate::api_model::ApiSession { id: p.to_string(), frame: None, grid: None, title: String::new() };
        let before = crate::api_model::split_node(&crate::api_model::node_of(&t.layout.tree(), t.zoomed, &blank).0);
        let requested = r.root.clone().unwrap_or_default();
        if !api_layout::isomorphic(&before, &requested) {
            return Status::WrongTree;
        }
        if t.zoomed.is_some() || self.panes.get(&t.focused).is_some_and(|p| p.tmux_io().is_some()) {
            return Status::Ok;
        }
        for (path, ratio) in api_layout::ratios(&requested, &t.layout.tree()) {
            self.tabs[index].layout.set_ratio(&path, ratio);
        }
        self.relayout();
        self.redraw();
        Status::Ok
    }

    /// ReorderTabs (`H:3731-3800`): iTerm2's checks, then a tab holding a loaded recording that
    /// would change windows refuses the whole request (INVALID_ASSIGNMENT: it never leaves
    /// its window); then each assignment in order.
    fn api_reorder_tabs(&mut self, r: &pb::ReorderTabsRequest) -> pb::reorder_tabs_response::Status {
        use pb::reorder_tabs_response::Status;
        let ids = self.api_ids();
        let status = api_layout::reorder_status(r, &ids);
        if status != Status::Ok {
            return status;
        }
        let mut plan = Vec::new();
        for a in &r.assignments {
            let Some(dest) = ids.window_key(a.window_id.as_deref().unwrap_or_default()) else { return Status::InvalidWindowId };
            let tabs: Vec<u64> = a.tab_ids.iter().filter_map(|t| ids.tab_by_id(t)).collect();
            for &t in &tabs {
                let Some(src) = ids.tab_window(t) else { continue };
                let recording = self.window_tabs(src).and_then(|(all, _)| all.iter().find(|x| x.id == t)).is_some_and(|x| self.holds_recording(x));
                if src != dest && recording {
                    return Status::InvalidAssignment;
                }
            }
            plan.push((dest, tabs));
        }
        for (dest, tabs) in plan {
            self.api_reorder_assignment(dest, &tabs);
        }
        Status::Ok
    }

    /// `performReorderAssignment:` (`H:3759-3786`): the tabs go to index 0, 1, 2... of window
    /// `dest` in order. A tab already there moves (refused silently under Lock Layout, as
    /// `moveTabAtIndex:toIndex:`, `PT:12296-12300`); one from another window leaves it (its
    /// replays end, the window closing when it has no tab left) and is inserted and selected
    /// (`insertTab:atIndex:` with `automaticallySelectNewTabs`). Every window touched then
    /// sends its tabs' order to tmux and lays out again.
    fn api_reorder_assignment(&mut self, dest: u64, tabs: &[u64]) {
        let mut touched = vec![dest];
        for (index, &tab) in tabs.iter().enumerate() {
            // A window emptied by an earlier move of this request is gone.
            let Some(dest_id) = self.window_id_of_key(dest) else { return };
            let Some(src) = self.registry.order().into_iter().find(|k| self.window_tabs(*k).is_some_and(|(all, _)| all.iter().any(|t| t.id == tab))) else { continue };
            if src == dest {
                if !self.enter(dest_id) || self.win_locks.forbids(LayoutOp::ReorderTab) {
                    continue;
                }
                let Some(from) = self.tabs.iter().position(|t| t.id == tab) else { continue };
                let active = self.tabs.get(self.active).map(|t| t.id);
                let moving = self.tabs.remove(from);
                self.tabs.insert(index.min(self.tabs.len()), moving);
                if let Some(a) = active.and_then(|a| self.tabs.iter().position(|t| t.id == a)) {
                    self.active = a;
                }
                continue;
            }
            let Some(src_id) = self.window_id_of_key(src) else { continue };
            if !self.enter(src_id) {
                continue;
            }
            let Some(at) = self.tabs.iter().position(|t| t.id == tab) else { continue };
            let panes = self.tabs[at].layout.panes();
            self.end_replays(&panes);
            let moving = self.tabs.remove(at);
            self.mru.forget(moving.id);
            self.active = crate::close_rules::active_after_close(self.active, at, self.tabs.len(), self.globals.bool("moveLeftAfterClosingTab"));
            if self.tabs.is_empty() {
                self.drop_current_window();
                touched.retain(|k| *k != src);
            } else if !touched.contains(&src) {
                touched.push(src);
            }
            // `dest` is known to exist, so the tab always lands there.
            self.enter(dest_id);
            let to = index.min(self.tabs.len());
            self.tabs.insert(to, moving);
            self.active = to;
        }
        for k in touched {
            if self.window_id_of_key(k).is_some_and(|id| self.enter(id)) {
                self.tmux_reorder();
                self.relayout();
                self.redraw();
            }
        }
    }

    /// MenuItem (`H:3524-3547`): the menu is brought up to date first, as iTerm2 updates it;
    /// the item runs in the window a menu pick would use, through the menu's own path.
    fn api_menu_item(&mut self, r: &pb::MenuItemRequest, event_loop: &ActiveEventLoop) -> pb::MenuItemResponse {
        if let Some(w) = self.last_window {
            self.enter(w);
        }
        self.sync_checks();
        let item = menus::by_identifier(r.identifier.as_deref().unwrap_or(""));
        let enabled = item.is_some_and(App::menu_enabled);
        let checked = item.is_some_and(|i| self.menu_checked(i.label()));
        let (answer, run) = menus::menu_answer(item, enabled, checked, r.query_only.unwrap_or(false));
        if let (true, Some(item)) = (run, item) {
            self.run_command(item, event_loop);
        }
        answer
    }

    /// For a tmux pane, whether its tmux window is hidden (how Tandem and iTerm2 bury a tmux
    /// session); None for any other session.
    fn api_tmux_hidden(&self, pane: PaneId) -> Option<bool> {
        let (gw, _, window) = self.tmux_of(pane)?;
        Some(self.tmux.get(&gw).is_some_and(|t| t.ctl.hidden().contains(&window)))
    }

    /// SetProperty `grid_size` (`sessionInitiatedResize:`, `PT:7126-7160`): IMPOSSIBLE in
    /// full screen, with Lock Size, for a tmux pane or a session in no window; otherwise the
    /// window's inner size grows or shrinks by the cells between the session's grid and the
    /// one asked for (at least 20 by 2, rows capped by the monitor, `safelySetSessionSize:`),
    /// so the other panes of its tab change too. An exited session keeps its size (OK).
    pub(super) fn api_set_grid_size(&mut self, pane: PaneId, cols: i64, rows: i64) -> pb::set_property_response::Status {
        use alacritty_terminal::grid::Dimensions;
        use pb::set_property_response::Status;
        let in_window = self.enter_pane_window(pane);
        let (fullscreen, locked, tmux) = match self.gpu.as_ref().filter(|_| in_window) {
            Some(g) => (g.window.fullscreen().is_some(), self.win_locks.size, self.tmux_of(pane).is_some()),
            None => (false, false, self.tmux_of(pane).is_some()),
        };
        let status = api_props::grid_status(fullscreen, locked, tmux, in_window);
        if status != Status::Ok {
            return status;
        }
        let Some(p) = self.panes.get(&pane).filter(|p| p.exited.is_none()) else { return status };
        let (cur_cols, cur_rows) = {
            let g = p.own().term().grid();
            (g.columns() as i64, g.screen_lines() as i64)
        };
        let (cw, ch) = self.cell();
        let Some(g) = self.gpu.as_ref() else { return status };
        let (outer, inner) = (g.window.outer_size(), g.window.inner_size());
        let max_new_rows = match g.window.current_monitor() {
            Some(m) => ((f64::from(m.size().height) - f64::from(outer.height)) / f64::from(ch.max(1.0))).floor() as i64,
            None => i64::MAX,
        };
        let (want_cols, want_rows) = api_props::wanted_grid(cols, rows, cur_rows, max_new_rows);
        let w = f64::from(inner.width) + (want_cols - cur_cols) as f64 * f64::from(cw);
        let h = f64::from(inner.height) + (want_rows - cur_rows) as f64 * f64::from(ch);
        let _ = g.window.request_inner_size(winit::dpi::PhysicalSize::new(w.round().max(1.0) as u32, h.round().max(1.0) as u32));
        status
    }
}

/// The sessions, tabs and windows ids name (`api_ids`), and their variables as the session
/// standing for each shows them: a tab's focused session, a window's current tab's focused
/// session, any session for the app.
pub(super) struct ApiObjects<'a> {
    app: &'a App,
    ids: Snapshot,
}

impl Objects for ApiObjects<'_> {
    fn id(&self, scope: Scope, id: &str) -> Option<String> {
        match scope {
            Scope::Session => self.ids.pane(id).map(|p| p.to_string()),
            Scope::Tab => self.ids.tab_by_id(id).map(|t| t.to_string()),
            Scope::Window => self.ids.window_key(id).map(|_| id.to_string()),
            Scope::App => Some(String::new()),
        }
    }

    fn all(&self, scope: Scope) -> Vec<String> {
        match scope {
            Scope::Session => self.ids.all_panes().iter().map(u64::to_string).collect(),
            Scope::Tab => self.ids.all_tabs().iter().map(u64::to_string).collect(),
            Scope::Window => self.ids.window_guids(),
            Scope::App => vec![String::new()],
        }
    }

    fn values(&self, scope: Scope, id: &str) -> Values {
        let app = self.app;
        let pane = match scope {
            Scope::Session => id.parse::<PaneId>().ok(),
            Scope::Tab => id.parse::<u64>().ok().and_then(|t| {
                let (tabs, _) = app.window_tabs(self.ids.tab_window(t)?)?;
                tabs.iter().find(|x| x.id == t).map(|x| x.focused)
            }),
            Scope::Window => self.ids.window_key(id).and_then(|k| app.window_tabs(k)).and_then(|(tabs, a)| tabs.get(a)).map(|t| t.focused),
            Scope::App => app.panes.keys().next().copied(),
        };
        let vars = pane.map(|p| app.variables_with(p, false)).unwrap_or_default();
        let mut out = api_vars::scope_values(&vars, scope);
        api_vars::typed_users(&app.api_vars, &vars, scope, id, &mut out);
        out
    }
}

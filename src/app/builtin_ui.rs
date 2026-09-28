//! Built-in functions and methods with side effects (`BF/`, `PS:23440-23578`,
//! `PTYTab.m:7893-7953`, `PseudoTerminal.m:16220-16225`), run by the invocation host
//! (`script_ui`). A built-in that waits (a dialog, a tmux reply, a new window) answers
//! `Started` with a token, and its answer later reaches the run through `api_rpc_done`, as a
//! script's result does.

use std::collections::BTreeMap;

use serde_json::json;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::App;
use crate::builtin_fns::{self as bf, Builtin, On};
use crate::expr::{Failure, Value};
use crate::layout::PaneId;
use crate::prompt_ui::{ConfirmResult, ConfirmWindow, PromptResult, PromptWindow};
use crate::script_eval::HostOutcome;

/// A dialog waiting to open: `alert`'s (`BF/iTermAlertBuiltInFunction.m:22-76`) or
/// `get_string`'s (`:80-140`), with the token its answer is delivered under.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Dialog {
    Alert { token: u64, title: String, subtitle: String, buttons: Vec<String> },
    Text { token: u64, title: String, subtitle: String, placeholder: String, default: String },
}

/// The most identifiers `set_status_bar_component_unread_count` keeps, app-wide and per
/// session (Tandem's bound; iTerm2 keeps every one). Session entries go with their session.
pub(super) const MAX_UNREAD_IDS: usize = 64;

/// The failure of a dialog dropped from a full queue (Tandem's; iTerm2 shows every one).
pub(super) const DIALOG_DROPPED: &str = "Too many dialogs were waiting, so this one was not shown.";

/// The dialog showing.
pub(super) enum Open {
    Alert { window: ConfirmWindow, token: u64, buttons: Vec<String> },
    Text { window: PromptWindow, token: u64 },
}

/// The buttons of an alert: the strings among `buttons`, else NSAlert's lone OK.
fn alert_buttons(buttons: Option<&Value>) -> Vec<String> {
    let list: Vec<String> = buttons.and_then(Value::as_array).into_iter().flatten().filter_map(|b| b.as_str().map(str::to_string)).collect();
    if list.is_empty() { vec![crate::api_auth::OK.to_string()] } else { list }
}

/// `alert`'s answer for the button at `index` (NSAlertFirstButtonReturn plus the index).
fn alert_answer(index: usize) -> Value {
    json!(bf::ALERT_FIRST_BUTTON + index as i64)
}

/// Closing an alert: the button Escape presses ("Cancel"), else the first; closing never
/// picks an action the person did not (`prompt_ui::close_index`).
fn alert_closed(buttons: &[String]) -> Value {
    let labels: Vec<&str> = buttons.iter().map(String::as_str).collect();
    alert_answer(crate::prompt_ui::close_index(&labels))
}

/// A numeric argument, a boolean as 1 or 0 (NSNumber's `doubleValue`).
fn num_arg(args: &BTreeMap<String, Value>, name: &str) -> Option<f64> {
    args.get(name).and_then(|a| a.as_f64().or_else(|| a.as_bool().map(f64::from)))
}

/// An unread count (`integerValue`: truncated, a boolean 1 or 0).
fn count_arg(args: &BTreeMap<String, Value>) -> i64 {
    num_arg(args, "count").unwrap_or(0.0) as i64
}

fn str_arg<'a>(args: &'a BTreeMap<String, Value>, name: &str) -> Option<&'a str> {
    args.get(name).and_then(Value::as_str)
}

fn failed(reason: &str) -> HostOutcome {
    HostOutcome::Failed(Failure::plain(reason))
}

fn done(r: Result<Value, String>) -> HostOutcome {
    match r {
        Ok(v) => HostOutcome::Done(v),
        Err(e) => HostOutcome::Failed(Failure::plain(e)),
    }
}

/// `set_status`'s fields as tab status pairs (`BF/SetStatusBuiltInFunction.swift:40-104`):
/// an empty value clears its field, a color must be `#rrggbb` (or `#rgb`, `#rrrrggggbbbb`);
/// `background_tasks` is accepted and ignored. Nothing applies when a color is bad.
fn status_pairs(args: &BTreeMap<String, Value>) -> Result<Vec<(String, String)>, String> {
    let mut pairs = Vec::new();
    for (arg, key, bad) in [("status", "status", None), ("text_color", "status-color", Some(bf::INVALID_TEXT_COLOR)), ("dot_color", "indicator", Some(bf::INVALID_DOT_COLOR)), ("detail", "detail", None)] {
        let Some(v) = str_arg(args, arg) else { continue };
        if let Some(text) = bad.filter(|_| !v.is_empty() && !bf::is_hash_color(v)) {
            return Err(text.to_string());
        }
        pairs.push((key.to_string(), v.to_string()));
    }
    Ok(pairs)
}

/// An id argument as text (a number from a tab scope's `id` too).
fn id_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

impl App {
    /// Runs a built-in whose arguments are type-checked, amended and without nulls; a method
    /// runs on `receiver`. With side effects off a function with a placeholder answers it
    /// (`BIF:264-267`); a method never does, as iTerm2's method path never asks
    /// (`iTermObject.m:73-91`). A synchronous evaluation cannot wait, so a built-in that
    /// waits for an answer answers null there without acting.
    pub(super) fn builtin(&mut self, b: &'static Builtin, args: BTreeMap<String, Value>, receiver: Option<(On, u64)>, side_effects: bool, sync: bool) -> HostOutcome {
        if !side_effects && receiver.is_none() {
            if let Some(p) = b.placeholder {
                return HostOutcome::Done(json!(p));
            }
        }
        let waits = matches!((b.on, b.name), (On::Function, "alert" | "get_string" | "move_tab_to_window") | (On::Session, "run_tmux_command"));
        if sync && waits {
            return HostOutcome::Done(Value::Null);
        }
        let back = self.current_window();
        let out = match (b.on, receiver) {
            (On::Function, _) => self.builtin_function(b.name, &args),
            (On::Session, Some((_, pane))) => self.session_method(b.name, pane, &args),
            (On::Tab, Some((_, tab))) => self.tab_method(b.name, tab, &args),
            (On::Window, Some((_, key))) => self.window_method(key, &args),
            _ => HostOutcome::Done(Value::Null),
        };
        // Only `focus` changes the window the person uses.
        if b.name != "focus" {
            if let Some(w) = back {
                self.enter(w);
            }
        }
        out
    }

    /// A method receiver's object (`SFC:365-381`): a window by its `pty-` id, else a session
    /// or a tab by decimal id, the one whose catalog has `sig` (`builtin_fns::receiver_kind`).
    pub(super) fn receiver(&self, id: &str, sig: &str) -> Option<(On, u64)> {
        let ids = self.api_ids();
        let window = if id.starts_with("pty-") { self.registry.by_guid(id).filter(|k| self.window_id_of_key(*k).is_some()) } else { None };
        let number = id.parse::<u64>().ok().filter(|n| n.to_string() == id);
        let pane = number.filter(|p| ids.pane_tab(*p).is_some() || ids.buried(*p));
        let tab = number.filter(|t| ids.tab_window(*t).is_some());
        let on = bf::receiver_kind(id, pane.is_some(), tab.is_some(), window.is_some(), sig)?;
        let key = match on {
            On::Window => window,
            On::Session => pane,
            On::Tab => tab,
            On::Function => None,
        }?;
        Some((on, key))
    }

    /// A token for a built-in's later answer.
    fn builtin_token(&mut self) -> u64 {
        let token = self.script_next;
        self.script_next += 1;
        token
    }

    /// The session a `session_id` (or `session`) default names (`anySessionWithGUID:`, buried
    /// ones too unless `shown`).
    fn session_arg(&self, args: &BTreeMap<String, Value>, name: &str, shown: bool) -> Result<PaneId, &'static str> {
        let s = str_arg(args, name).ok_or(bf::MISSING_SESSION_ID)?;
        let ids = self.api_ids();
        s.parse::<u64>().ok().filter(|p| p.to_string() == s).filter(|p| ids.pane_tab(*p).is_some() || (!shown && ids.buried(*p))).ok_or(bf::NO_SUCH_SESSION)
    }

    fn builtin_function(&mut self, name: &str, args: &BTreeMap<String, Value>) -> HostOutcome {
        match name {
            "count" => done(bf::count(args.get("array"))),
            "urlEncode" => done(bf::url_encode(args.get("string"))),
            "with_alpha" => match (str_arg(args, "color"), num_arg(args, "alpha")) {
                (Some(c), Some(a)) => done(bf::with_alpha(c, a)),
                _ => failed("with_alpha requires a color string and a numeric alpha"),
            },
            "alert" => {
                let token = self.builtin_token();
                let title = str_arg(args, "title").unwrap_or_default().to_string();
                let subtitle = str_arg(args, "subtitle").unwrap_or_default().to_string();
                self.queue_dialog(Dialog::Alert { token, title, subtitle, buttons: alert_buttons(args.get("buttons")) });
                HostOutcome::Started(token)
            }
            "get_string" => {
                let token = self.builtin_token();
                let text = |n: &str| str_arg(args, n).unwrap_or_default().to_string();
                self.queue_dialog(Dialog::Text { token, title: text("title"), subtitle: text("subtitle"), placeholder: text("placeholder"), default: text("defaultValue") });
                HostOutcome::Started(token)
            }
            "focus" => match self.session_arg(args, "session_id", false) {
                Ok(pane) => {
                    self.builtin_focus(pane);
                    HostOutcome::Done(Value::Null)
                }
                Err(e) => failed(e),
            },
            "paste" => match self.session_arg(args, "session_id", true) {
                Ok(pane) => {
                    if let Some(text) = self.clipboard_text() {
                        self.user_paste(pane, &text, crate::broadcast::Write::Paste);
                    }
                    HostOutcome::Done(Value::Null)
                }
                Err(e) => failed(e),
            },
            "add_clipping" => {
                let pane = match self.session_arg(args, "session", false) {
                    Ok(p) => p,
                    Err(e) => return failed(e),
                };
                let (Some(kind), Some(title), Some(detail)) = (str_arg(args, "type"), str_arg(args, "title"), str_arg(args, "detail")) else { return failed(bf::MISSING_ARGUMENT) };
                let clip = crate::clippings::Clipping { kind: kind.to_string(), title: title.to_string(), detail: detail.to_string() };
                self.clips.entry(pane).or_default().model.add(clip);
                self.mark_state_dirty();
                // The panel shows in the session's window.
                if self.enter_pane_window(pane) {
                    self.relayout();
                }
                self.sync_checks();
                self.redraw_all();
                HostOutcome::Done(Value::Null)
            }
            "archive_clippings" => {
                let pane = match self.session_arg(args, "session", false) {
                    Ok(p) => p,
                    Err(e) => return failed(e),
                };
                if let Some(c) = self.clips.get_mut(&pane) {
                    if c.model.archive_all() {
                        c.selected.clear();
                        c.scroll = 0.0;
                    }
                }
                self.mark_state_dirty();
                self.redraw_all();
                HostOutcome::Done(Value::Null)
            }
            "get_profile_property" => {
                let pane = match self.session_arg(args, "session_id", false) {
                    Ok(p) => p,
                    Err(e) => return failed(e),
                };
                let key = str_arg(args, "key").unwrap_or_default().to_string();
                let Some(p) = self.panes.get(&pane) else { return failed(bf::NO_SUCH_SESSION) };
                let profile = self.store.get(&p.profile).unwrap_or_else(|| self.store.default_profile());
                let json = crate::api_profile::properties(profile, &[key]).into_iter().next().and_then(|p| p.json_value);
                HostOutcome::Done(json.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or(Value::Null))
            }
            "set_status" => {
                let pane = match self.session_arg(args, "session_id", false) {
                    Ok(p) => p,
                    Err(e) => return failed(e),
                };
                let pairs = match status_pairs(args) {
                    Ok(p) => p,
                    Err(e) => return failed(&e),
                };
                if let Some(p) = self.panes.get_mut(&pane) {
                    p.own_mut().apply_tab_status(&pairs);
                }
                self.redraw_all();
                self.redraw();
                HostOutcome::Done(Value::Null)
            }
            "set_status_bar_component_unread_count" => {
                let id = str_arg(args, "identifier").unwrap_or_default().to_string();
                let n = count_arg(args);
                // A new identifier past the cap is ignored (a trigger could pass one per match).
                if self.unread_counts.contains_key(&id) || self.unread_counts.len() < MAX_UNREAD_IDS {
                    self.unread_counts.insert(id, n);
                }
                self.redraw_all();
                self.redraw();
                HostOutcome::Done(Value::Null)
            }
            "move_tab_to_window" => self.builtin_move_tab(args),
            _ => HostOutcome::Done(Value::Null),
        }
    }

    /// `focus` (`BF/FocusBuiltInFunction.swift`): reveal (a buried session comes back), then
    /// the window comes forward with the session's tab and the session focused.
    fn builtin_focus(&mut self, pane: PaneId) {
        if self.buried.contains(pane) {
            self.disinter(pane);
        }
        if !self.enter_pane_window(pane) {
            return;
        }
        if let Some(i) = self.tabs.iter().position(|t| t.layout.panes().contains(&pane)) {
            self.select_tab(i);
            let t = &mut self.tabs[i];
            // A maximized tab stays maximized, on this session.
            if t.zoomed.is_some() {
                t.zoomed = Some(pane);
            }
            t.focused = pane;
        }
        if let Some(w) = self.current_window() {
            self.focus_terminal_window(w);
        }
        self.relayout();
        self.redraw();
    }

    /// `move_tab_to_window` (`BF/iTermMoveTabToWindowBuiltInFunction.m`): no `tab_id` is "No
    /// such tab"; an id naming no tab finds no window ("Tab has no window", as iTerm2's nil
    /// tab does); the window's only tab stays. The answer is the new window's id, once the
    /// window opens.
    fn builtin_move_tab(&mut self, args: &BTreeMap<String, Value>) -> HostOutcome {
        let Some(id) = args.get("tab_id").and_then(id_text) else { return failed(bf::NO_SUCH_TAB) };
        let ids = self.api_ids();
        let Some((tab, k)) = ids.tab_by_id(&id).and_then(|t| Some((t, ids.tab_window(t)?))) else { return failed(bf::TAB_HAS_NO_WINDOW) };
        if !self.window_id_of_key(k).is_some_and(|w| self.enter(w)) {
            return failed(bf::TAB_HAS_NO_WINDOW);
        }
        if self.tabs.len() < 2 {
            return failed(bf::ONLY_ONE_TAB);
        }
        let Some(at) = self.tabs.iter().position(|t| t.id == tab) else { return failed(bf::TAB_HAS_NO_WINDOW) };
        if self.holds_recording(&self.tabs[at]) {
            return failed(bf::WINDOW_FAILED);
        }
        let front = self.tabs.get(self.active).map(|t| t.id);
        self.active = at;
        self.move_tab_to_new_window();
        // The window keeps the tab it had in front, when that was another.
        if let Some(i) = front.filter(|f| *f != tab).and_then(|f| self.tabs.iter().position(|t| t.id == f)) {
            self.active = i;
            self.relayout();
        }
        let token = self.builtin_token();
        self.script_moved.push((tab, token));
        HostOutcome::Started(token)
    }

    /// Answers the `move_tab_to_window` calls whose tab reached a window: its id, or "Failed
    /// to create new window" when the tab is in no window and none is waiting to open. True
    /// while some still wait.
    pub(super) fn script_moved_tabs(&mut self) -> bool {
        if self.script_moved.is_empty() {
            return false;
        }
        let ids = self.api_ids();
        let pending: Vec<u64> = self
            .pending_windows
            .iter()
            .flat_map(|w| match w {
                super::windows_ui::NewWindow::Tab(t) => std::slice::from_ref(t),
                super::windows_ui::NewWindow::Tabs(ts) => ts.as_slice(),
                _ => &[],
            })
            .map(|t| t.id)
            .collect();
        let mut waiting = Vec::new();
        for (tab, token) in std::mem::take(&mut self.script_moved) {
            let guid = ids.tab_window(tab).and_then(|k| self.registry.guid(k)).map(str::to_string);
            match guid {
                Some(g) => self.api_rpc_done.push_back((token, Ok(Value::String(g)))),
                None if pending.contains(&tab) => waiting.push((tab, token)),
                None => self.api_rpc_done.push_back((token, Err(Failure::plain(bf::WINDOW_FAILED)))),
            }
        }
        self.script_moved = waiting;
        !self.script_moved.is_empty()
    }

    /// Session methods (`PS:23440-23578`, `PS:23705-23748`).
    fn session_method(&mut self, name: &str, pane: PaneId, args: &BTreeMap<String, Value>) -> HostOutcome {
        match name {
            // `PS:23744-23748`: the session's own profile Name. As a local name it replaces a
            // name a program or trigger set (`PS:3196-3203`), so the pane's title goes.
            "set_name" => {
                let name = str_arg(args, "name").unwrap_or_default().to_string();
                if let Some(p) = self.panes.get_mut(&pane) {
                    p.own_mut().set_title(None);
                }
                self.set_session_values(pane, &[("Name".to_string(), Some(toml::Value::String(name)))]);
                self.redraw_all();
                self.redraw();
                HostOutcome::Done(Value::Null)
            }
            "run_tmux_command" => {
                // A tmux window's session or the gateway's own (both have a controller).
                let gw = self.panes.get(&pane).and_then(|p| p.tmux_io()).map(|io| io.gateway).or_else(|| self.tmux.contains_key(&pane).then_some(pane));
                let Some(gw) = gw else { return failed(bf::NOT_TMUX) };
                // Sent tolerating errors (`PS:23723-23727`): a bad command answers "tmux error".
                let tolerant = crate::tmux_gateway::Flags { tolerate: true, ..Default::default() };
                let Some(tag) = self.tmux_api_send(gw, str_arg(args, "command").unwrap_or_default(), tolerant) else { return failed(bf::NOT_TMUX) };
                let token = self.builtin_token();
                self.script_tmux.insert((gw, tag), token);
                HostOutcome::Started(token)
            }
            "set_status_bar_component_unread_count" => {
                let id = str_arg(args, "identifier").unwrap_or_default().to_string();
                let n = count_arg(args);
                let key = (pane, id);
                if self.unread_session_counts.contains_key(&key) || self.unread_session_counts.keys().filter(|(p, _)| *p == pane).count() < MAX_UNREAD_IDS {
                    self.unread_session_counts.insert(key, n);
                }
                self.redraw_all();
                self.redraw();
                HostOutcome::Done(Value::Null)
            }
            "stop_coprocess" => {
                let running = self.panes.get(&pane).is_some_and(|p| p.coprocess_command().is_some());
                if running {
                    self.stop_coprocess(pane);
                }
                HostOutcome::Done(Value::Bool(running))
            }
            "get_coprocess" => HostOutcome::Done(self.panes.get(&pane).and_then(|p| p.coprocess_command()).map_or(Value::Null, |c| json!(c))),
            "run_coprocess" => {
                if self.panes.get(&pane).is_some_and(|p| p.coprocess_command().is_some()) {
                    return HostOutcome::Done(Value::Bool(false));
                }
                let mute = args.get("mute").is_some_and(|m| m.as_bool().unwrap_or_else(|| m.as_f64().is_some_and(|n| n != 0.0)));
                self.start_coprocess(pane, str_arg(args, "commandLine").unwrap_or_default(), mute, crate::coprocess::Entry::Api);
                HostOutcome::Done(Value::Bool(true))
            }
            _ => HostOutcome::Done(Value::Null),
        }
    }

    /// Tab methods (`PTYTab.m:7921-7953`): the title override as Edit Tab Title sets it, and
    /// a move to the neighboring session.
    fn tab_method(&mut self, name: &str, tab: u64, args: &BTreeMap<String, Value>) -> HostOutcome {
        let window = self.api_ids().tab_window(tab).and_then(|k| self.window_id_of_key(k));
        if !window.is_some_and(|w| self.enter(w)) {
            return HostOutcome::Done(Value::Null);
        }
        match name {
            "set_title" => {
                let text = Some(str_arg(args, "title").unwrap_or_default().to_string()).filter(|t| !t.is_empty());
                self.apply_tab_title(tab, text);
                self.redraw();
                HostOutcome::Done(Value::Null)
            }
            "select_pane_in_direction" => {
                let Some(dir) = bf::direction(str_arg(args, "direction").unwrap_or_default()) else { return failed(bf::INVALID_DIRECTION) };
                let Some(i) = self.tabs.iter().position(|t| t.id == tab) else { return HostOutcome::Done(Value::Null) };
                let area = self.content_area_for(&self.tabs[i]);
                let t = &mut self.tabs[i];
                let Some(next) = t.layout.neighbor(t.focused, dir, area) else { return HostOutcome::Done(Value::Null) };
                t.focused = next;
                t.zoomed = None;
                self.relayout();
                self.redraw();
                HostOutcome::Done(json!(next.to_string()))
            }
            _ => HostOutcome::Done(Value::Null),
        }
    }

    /// The window method `set_title` (`PseudoTerminal.m:16220-16225`): Edit Window Title's
    /// text; empty clears it.
    fn window_method(&mut self, key: u64, args: &BTreeMap<String, Value>) -> HostOutcome {
        if self.window_id_of_key(key).is_some_and(|w| self.enter(w)) {
            self.win_title = Some(str_arg(args, "title").unwrap_or_default().to_string()).filter(|t| !t.is_empty());
            self.redraw();
        }
        HostOutcome::Done(Value::Null)
    }

    /// Queues an `alert` or `get_string` dialog. At most `MAX_QUEUED` wait (a trigger can call
    /// them once per match); the oldest waiting one goes, its call failing with
    /// `DIALOG_DROPPED` so its run does not wait forever.
    fn queue_dialog(&mut self, d: Dialog) {
        if let Some(old) = super::script_ui::push_capped(&mut self.script_dialogs, d) {
            self.api_rpc_done.push_back((dialog_token(&old), Err(Failure::plain(DIALOG_DROPPED.to_string()))));
        }
    }

    /// Opens the next queued dialog when none shows; a dialog whose run finished is dropped.
    pub(super) fn open_script_dialogs(&mut self, event_loop: &ActiveEventLoop) {
        if self.script_dialog.is_some() {
            return;
        }
        while self.script_dialogs.front().is_some_and(|d| !self.script_tokens.contains_key(&dialog_token(d))) {
            self.script_dialogs.pop_front();
        }
        let Some(g) = self.gpu.as_ref() else { return };
        let Some(d) = self.script_dialogs.pop_front() else { return };
        let token = dialog_token(&d);
        let opened = match d {
            Dialog::Alert { title, subtitle, buttons, .. } => {
                let labels: Vec<&str> = buttons.iter().map(String::as_str).collect();
                ConfirmWindow::actions(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, &title, &subtitle, &labels, false).map(|window| Open::Alert { window, token, buttons: buttons.clone() })
            }
            Dialog::Text { title, subtitle, placeholder, default, .. } => {
                PromptWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, &title, &subtitle, &default).map(|w| Open::Text { window: w.with_hint(&placeholder).ok_only(), token })
            }
        };
        match opened {
            Ok(open) => self.script_dialog = Some(open),
            Err(e) => {
                self.banners.set("window", format!("Could not open the window: {e}"));
                self.api_rpc_done.push_back((token, Err(Failure::plain(format!("Could not open the window: {e}")))));
            }
        }
    }

    /// Events for the showing dialog. An alert answers with its button (closing it is
    /// `alert_closed`'s); a text dialog with the field's text whatever closes it. False for
    /// other windows.
    pub(super) fn script_dialog_event(&mut self, window: WindowId, event: &WindowEvent) -> bool {
        let answer = match self.script_dialog.as_mut() {
            Some(Open::Alert { window: w, token, buttons }) if w.id() == window => {
                let value = match event {
                    WindowEvent::CloseRequested => Some(alert_closed(buttons)),
                    WindowEvent::RedrawRequested => match w.redraw() {
                        ConfirmResult::Action { index, .. } => Some(alert_answer(index)),
                        _ => None,
                    },
                    other => {
                        w.event(other);
                        None
                    }
                };
                value.map(|v| (*token, v))
            }
            Some(Open::Text { window: w, token }) if w.id() == window => {
                let text = match event {
                    WindowEvent::CloseRequested => Some(w.text().to_string()),
                    WindowEvent::RedrawRequested => match w.redraw() {
                        PromptResult::Ok(t) | PromptResult::Extra(t) => Some(t),
                        PromptResult::Cancel => Some(w.text().to_string()),
                        PromptResult::Open => None,
                    },
                    other => {
                        w.event(other);
                        None
                    }
                };
                text.map(|t| (*token, Value::String(t)))
            }
            _ => return false,
        };
        if let Some((token, v)) = answer {
            self.script_dialog = None;
            self.api_rpc_done.push_back((token, Ok(v)));
        }
        true
    }
}

fn dialog_token(d: &Dialog) -> u64 {
    match d {
        Dialog::Alert { token, .. } | Dialog::Text { token, .. } => *token,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_dialogs_follow_iterm2() {
        assert_eq!(alert_buttons(Some(&json!(["Yes", 3, "No"]))), ["Yes", "No"], "only strings become buttons");
        assert_eq!(alert_buttons(Some(&json!([]))), ["OK"], "NSAlert's lone OK");
        assert_eq!(alert_answer(0), json!(1000));
        assert_eq!(alert_answer(2), json!(1002));
        // Closing never picks an action: Escape's "Cancel", else the first.
        let labels = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<String>>();
        assert_eq!(alert_closed(&labels(&["Keep", "Delete"])), json!(1000));
        assert_eq!(alert_closed(&labels(&["Delete", "Cancel"])), json!(1001));
        assert_eq!(alert_closed(&labels(&["OK"])), json!(1000));
        // Counts as `integerValue`: a boolean is 1, a fraction truncated.
        let count = |v: Value| count_arg(&BTreeMap::from([("count".to_string(), v)]));
        assert_eq!((count(json!(true)), count(json!(false)), count(json!(2.7)), count(json!(3))), (1, 0, 2, 3));
        assert_eq!(num_arg(&BTreeMap::from([("alpha".to_string(), json!(true))]), "alpha"), Some(1.0));
        let args: BTreeMap<String, Value> = [("status", json!("Working")), ("text_color", json!("")), ("dot_color", json!("#0f0")), ("background_tasks", json!(2))].into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        assert_eq!(status_pairs(&args), Ok(vec![("status".to_string(), "Working".to_string()), ("status-color".to_string(), String::new()), ("indicator".to_string(), "#0f0".to_string())]));
        let bad: BTreeMap<String, Value> = [("status", json!("x")), ("dot_color", json!("red"))].into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        assert_eq!(status_pairs(&bad), Err(bf::INVALID_DOT_COLOR.to_string()));
        let bad: BTreeMap<String, Value> = [("text_color", json!("#12345"))].into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        assert_eq!(status_pairs(&bad), Err(bf::INVALID_TEXT_COLOR.to_string()));
        assert_eq!(id_text(&json!(5)), Some("5".to_string()));
        assert_eq!(id_text(&json!("5")), Some("5".to_string()));
        assert_eq!(id_text(&json!([])), None);
    }
}

//! Running invocations (`SFC:203-276`, `PS:11657-11673`): each is a `Run` of the pure
//! `script_eval::Invocation` with the scope it reads and where its result goes. The host
//! routes a call to a built-in (Task 4) or, while the Python API runs, to the script that
//! registered it (`script_fn`); script results and timeouts come back through
//! `api_rpc_done` and `script_tick` steps the runs they finish. Failures reported to the
//! person show iTerm2's "Function Call Failed" alert (`H:566-591`), one at a time.

use std::time::Instant;

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::App;
use crate::api_proto::{self, pb};
use crate::api_vars::{self, Objects, Scope, Values};
use crate::expr::{self, Call, FailKind, Failure, Value};
use crate::layout::PaneId;
use crate::prompt_ui::{ConfirmResult, ConfirmWindow};
use crate::script_eval::{self, Host, HostOutcome, Invocation, Layered, Step, Timeout};
use crate::script_fn;

/// The scope an invocation reads: a session's with a frame of temporary values on top (a
/// trigger's, a component's `__knobs`), a tab's by id, a window's by Registry key, the
/// app's, or a method call's receiver by its id.
#[derive(Clone, Debug)]
pub(super) enum ScopeSpec {
    Session { pane: PaneId, frame: Values },
    Tab(u64),
    Window(u64),
    App,
    Receiver(String),
}

/// Where an invocation's result goes: an InvokeFunctionRequest's answer, the error alert
/// with the origin's name, or one of the callers of Tasks 5 to 9; `Quiet` drops it.
#[derive(Clone, Debug)]
pub(super) enum Origin {
    Api {
        conn: u64,
        req: pb::ClientOriginatedMessage,
    },
    // Constructed by the callers of Tasks 5 to 9.
    #[allow(dead_code)]
    Report(String),
    #[allow(dead_code)]
    OpenQuickly,
    #[allow(dead_code)]
    Title(PaneId),
    #[allow(dead_code)]
    Component { pane: PaneId, id: String },
    #[allow(dead_code)]
    Click,
    #[allow(dead_code)]
    Template { pane: PaneId, key: String },
    #[allow(dead_code)]
    Quiet,
}

/// A running invocation.
pub(super) struct Run {
    pub inv: Invocation,
    pub scope: ScopeSpec,
    pub origin: Origin,
}

/// An alert waiting to show: a failed call's heading and text, or an invocation's result
/// (Open Quickly, Task 5).
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Alert {
    Failed {
        heading: String,
        text: String,
    },
    #[allow(dead_code)]
    Result(String),
}

/// The host of one run's calls: built-ins first (Task 4), then scripts (`SFC:510-560`).
struct ScriptHost<'a> {
    app: &'a mut App,
    run: u64,
    scope: &'a dyn expr::Scope,
}

impl Host for ScriptHost<'_> {
    fn call(&mut self, call: &Call, args: &std::collections::BTreeMap<String, Value>, sync: bool, _side_effects: bool) -> HostOutcome {
        // A synchronous evaluation never runs a script (`SFC:527-532`).
        if sync {
            return HostOutcome::Done(Value::Null);
        }
        let sig = call.signature();
        if !self.app.api_on {
            return HostOutcome::Failed(Failure { reason: script_fn::disabled_text(&sig), detail: None, kind: FailKind::Disabled, conn: None });
        }
        let fq = call.fq_name();
        let Some((conn, params)) = self.app.api_fns.resolve(&fq, args, self.scope) else {
            let reason = script_fn::unregistered_text(&sig, self.app.api_fns.similar(&fq).as_deref());
            return HostOutcome::Failed(Failure { reason, detail: None, kind: FailKind::Unregistered, conn: None });
        };
        let token = self.app.script_next;
        self.app.script_next += 1;
        match self.app.api_fns.start(conn, &fq, &params, token, Instant::now()) {
            Ok((_, n)) => {
                self.app.api_reply(conn, &api_proto::notification(n));
                self.app.script_tokens.insert(token, self.run);
                HostOutcome::Started(token)
            }
            Err(f) => HostOutcome::Failed(f),
        }
    }
}

impl App {
    /// Starts an invocation (`callFunction`, `SFC:203-276`) and takes its first step. The
    /// run's id while it waits; None when it finished at once (its result already went to
    /// its origin), including a syntax error or a receiver that names no object.
    pub(super) fn invoke(&mut self, invocation: &str, scope: ScopeSpec, timeout: Timeout, side_effects: bool, origin: Origin) -> Option<u64> {
        let inv = match Invocation::new(invocation, timeout, side_effects, Instant::now()) {
            Ok(inv) => inv,
            Err(f) => {
                self.script_answer(&origin, invocation, Err(f));
                return None;
            }
        };
        if let ScopeSpec::Receiver(id) = &scope {
            if self.script_values(&scope).is_none() {
                self.script_answer(&origin, invocation, Err(Failure::plain(script_eval::object_not_found(id))));
                return None;
            }
        }
        let id = self.script_next;
        self.script_next += 1;
        self.script_runs.insert(id, Run { inv, scope, origin });
        self.script_step(id);
        self.script_runs.contains_key(&id).then_some(id)
    }

    /// The values a scope reads (11b's `scope_values` with the API's typed user values): a
    /// session's with its selection, a tab's focused session's in tab scope, a window's
    /// current tab's focused session's in window scope, any session's app scope (else an
    /// empty one). A receiver is a window id when it starts `pty-`, else a session, else a
    /// tab. None when the object is gone.
    pub(super) fn script_values(&self, spec: &ScopeSpec) -> Option<Values> {
        let session = |pane: PaneId| -> Option<Values> {
            if !self.panes.contains_key(&pane) {
                return None;
            }
            let vars = self.variables_with(pane, true);
            let mut out = api_vars::scope_values(&vars, Scope::Session);
            api_vars::typed_users(&self.api_vars, &vars, Scope::Session, &pane.to_string(), &mut out);
            Some(out)
        };
        let objects = self.api_objects();
        let ids = self.api_ids();
        let window = |k: u64| -> Option<Values> {
            let guid = ids.windows.iter().find(|(w, _, _)| *w == k).map(|(_, g, _)| g.clone())?;
            Some(objects.values(Scope::Window, &guid))
        };
        let tab = |t: u64| -> Option<Values> { ids.tab_window(t).map(|_| objects.values(Scope::Tab, &t.to_string())) };
        match spec {
            ScopeSpec::Session { pane, .. } => session(*pane),
            ScopeSpec::Tab(t) => tab(*t),
            ScopeSpec::Window(k) => window(*k),
            ScopeSpec::App => Some(objects.values(Scope::App, "")),
            ScopeSpec::Receiver(id) if id.starts_with("pty-") => ids.window_key(id).and_then(window),
            ScopeSpec::Receiver(id) => match ids.pane(id) {
                Some(p) => session(p),
                None => ids.tab_by_id(id).and_then(tab),
            },
        }
    }

    /// Steps a run with its scope as it is now; a finished run goes to its origin. The run is
    /// out of `script_runs` while it steps (the host borrows the App).
    fn script_step(&mut self, id: u64) {
        let Some(mut run) = self.script_runs.remove(&id) else { return };
        // An object gone since the start reads as an empty scope.
        let base = self.script_values(&run.scope).unwrap_or_default();
        let empty = Values::new();
        let frame = match &run.scope {
            ScopeSpec::Session { frame, .. } => frame.clone(),
            _ => empty,
        };
        let scope = Layered { frame: &frame, base: &base };
        let step = run.inv.step(&scope, &mut ScriptHost { app: self, run: id, scope: &scope });
        match step {
            Step::Waiting => {
                self.script_runs.insert(id, run);
            }
            Step::Done(v) => self.script_finished(run, Ok(v)),
            Step::Failed(f) => self.script_finished(run, Err(f)),
        }
    }

    /// Once per loop pass: script results (and calls past the registry's deadline) reach
    /// their runs, the runs they finish step, runs past their own timer fail with its text
    /// (written to the waiting call's connection log, `SFC:396-401`). When the next timer
    /// fires, the registry's included.
    pub(super) fn script_tick(&mut self) -> Option<Instant> {
        let now = Instant::now();
        for (token, f) in self.api_fns.expire(now) {
            self.api_rpc_done.push_back((token, Err(f)));
        }
        let mut touched = std::collections::BTreeSet::new();
        while let Some((token, r)) = self.api_rpc_done.pop_front() {
            let Some(id) = self.script_tokens.remove(&token) else { continue };
            if self.script_runs.get_mut(&id).is_some_and(|run| run.inv.deliver(token, r)) {
                touched.insert(id);
            }
        }
        for id in touched {
            self.script_step(id);
        }
        let late: Vec<u64> = self.script_runs.iter().filter(|(_, r)| r.inv.deadline().is_some_and(|d| d <= now)).map(|(id, _)| *id).collect();
        for id in late {
            let Some(run) = self.script_runs.remove(&id) else { continue };
            let mut f = run.inv.timed_out();
            f.conn = run.inv.returning().and_then(|t| self.api_fns.conn_of(t));
            self.script_log(f.conn, f.reason.clone());
            self.script_finished(run, Err(f));
        }
        self.script_runs.values().filter_map(|r| r.inv.deadline()).chain(self.api_fns.next_deadline()).min()
    }

    /// A finished run: its calls still running are forgotten (their results are dropped),
    /// and the result goes to its origin.
    pub(super) fn script_finished(&mut self, run: Run, r: Result<Value, Failure>) {
        for token in run.inv.tokens() {
            self.script_tokens.remove(&token);
        }
        self.script_answer(&run.origin, run.inv.text(), r);
    }

    /// Where a result goes: an InvokeFunctionResponse (`H:4376-4394`), the error alert with
    /// the origin's name, or nowhere.
    fn script_answer(&mut self, origin: &Origin, invocation: &str, r: Result<Value, Failure>) {
        match origin {
            Origin::Api { conn, req } => {
                let mut m = api_proto::reply_to(req);
                m.submessage = Some(pb::server_originated_message::Submessage::InvokeFunctionResponse(script_eval::invoke_response(&r)));
                self.api_reply(*conn, &m);
            }
            Origin::Report(name) => {
                if let Err(f) = r {
                    self.report_function_error(name, invocation, &f);
                }
            }
            // The callers of Tasks 5 to 9 take their results there.
            Origin::OpenQuickly | Origin::Title(_) | Origin::Component { .. } | Origin::Click | Origin::Template { .. } | Origin::Quiet => {}
        }
    }

    /// iTerm2's `reportFunctionCallError` (`H:566-591`): the console text in the log of the
    /// connection that failed, and the alert queued (OK only; no suppress box).
    pub(super) fn report_function_error(&mut self, origin: &str, invocation: &str, f: &Failure) {
        self.script_log(f.conn, script_eval::console_text(invocation, &f.reason, f.detail.as_deref().unwrap_or_default()));
        self.script_alerts.push_back(Alert::Failed { heading: script_eval::report_heading(origin), text: script_eval::report_text(invocation, &f.reason) });
    }

    /// A line for a connection's Script Console entry; nothing without a connection (iTerm2
    /// finds no entry for a nil key).
    pub(super) fn script_log(&mut self, conn: Option<u64>, text: String) {
        if let Some(conn) = conn {
            self.api_log_conn(conn, text);
        }
    }

    /// Opens the next queued alert when none shows.
    pub(super) fn open_script_alerts(&mut self, event_loop: &ActiveEventLoop) {
        if self.script_alert.is_some() {
            return;
        }
        let Some(alert) = self.script_alerts.front() else { return };
        let Some(g) = self.gpu.as_ref() else { return };
        let (heading, text) = match alert {
            Alert::Failed { heading, text } => (heading.as_str(), text.as_str()),
            Alert::Result(text) => (script_eval::RESULT_HEADING, text.as_str()),
        };
        let opened = ConfirmWindow::actions(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, heading, text, &[crate::api_auth::OK], false);
        let Some(alert) = self.script_alerts.pop_front() else { return };
        match opened {
            Ok(w) => self.script_alert = Some((w, alert)),
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// Events for the showing alert; OK (or closing it) dismisses it. False for other windows.
    pub(super) fn script_alert_event(&mut self, window: WindowId, event: &WindowEvent) -> bool {
        let Some((w, _)) = self.script_alert.as_mut().filter(|(w, _)| w.id() == window) else { return false };
        let answer = match event {
            WindowEvent::CloseRequested => w.close_answer(),
            WindowEvent::RedrawRequested => w.redraw(),
            other => {
                w.event(other);
                ConfirmResult::Open
            }
        };
        if !matches!(answer, ConfirmResult::Open) {
            self.script_alert = None;
        }
        true
    }
}

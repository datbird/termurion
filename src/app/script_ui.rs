//! Running invocations (`SFC:203-276`, `PS:11657-11673`): each is a `Run` of the pure
//! `script_eval::Invocation` with the scope it reads and where its result goes. The host
//! routes a call to a built-in (`builtin_fns`, run by `builtin_ui`), a method invocation's
//! calls to its receiver, and the rest, while the Python API runs, to the script that
//! registered it (`script_fn`); script results and timeouts come back through
//! `api_rpc_done` and `script_tick` steps the runs they finish. Failures reported to the
//! person show iTerm2's "Function Call Failed" alert (`H:566-591`), one at a time.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
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
use crate::builtin_fns;

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

/// A running invocation, with the values its scope had when it started: iTerm2 reads every
/// variable once, when it parses the invocation (`SFC:211-212`, `EP:425`).
pub(super) struct Run {
    pub inv: Invocation,
    pub scope: ScopeSpec,
    pub base: Values,
    pub origin: Origin,
    /// A method invocation's receiver id: its calls are the receiver's methods.
    pub receiver: Option<String>,
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

impl Alert {
    /// The alert's heading and text.
    fn form(&self) -> (&str, &str) {
        match self {
            Alert::Failed { heading, text } => (heading.as_str(), text.as_str()),
            Alert::Result(text) => (script_eval::RESULT_HEADING, text.as_str()),
        }
    }
}

/// What a script call answers before any script is asked (`SFC:527-545`): a synchronous
/// evaluation null, and with the API off iTerm2's undefined function text.
fn script_gate(sig: &str, sync: bool, api_on: bool) -> Option<HostOutcome> {
    if sync {
        return Some(HostOutcome::Done(Value::Null));
    }
    (!api_on).then(|| HostOutcome::Failed(Failure { reason: script_fn::disabled_text(sig), detail: None, kind: FailKind::Disabled, conn: None }))
}

/// Script results reach the runs waiting on them; the runs that got one. A result whose run
/// finished is dropped.
fn deliver_results(runs: &mut BTreeMap<u64, Run>, tokens: &mut HashMap<u64, u64>, done: &mut VecDeque<(u64, Result<Value, Failure>)>) -> BTreeSet<u64> {
    let mut touched = BTreeSet::new();
    while let Some((token, r)) = done.pop_front() {
        let Some(id) = tokens.remove(&token) else { continue };
        if runs.get_mut(&id).is_some_and(|run| run.inv.deliver(token, r)) {
            touched.insert(id);
        }
    }
    touched
}

/// The runs whose timer fired by `now`.
fn late_runs(runs: &BTreeMap<u64, Run>, now: Instant) -> Vec<u64> {
    runs.iter().filter(|(_, r)| r.inv.deadline().is_some_and(|d| d <= now)).map(|(id, _)| *id).collect()
}

/// When the next timer fires: a run's, or the registry's for a call it waits on.
fn next_wake(runs: &BTreeMap<u64, Run>, registry: Option<Instant>) -> Option<Instant> {
    runs.values().filter_map(|r| r.inv.deadline()).chain(registry).min()
}

/// The host of one run's calls: a method invocation's calls on its receiver, built-ins
/// first, then scripts (`SFC:510-560`).
struct ScriptHost<'a> {
    app: &'a mut App,
    run: u64,
    scope: &'a dyn expr::Scope,
    /// A method invocation's receiver id, and the sites of the calls it receives (the
    /// sequence's, not their arguments').
    receiver: Option<&'a str>,
    methods: BTreeSet<usize>,
}

/// The arguments a built-in runs with: nulls dropped (`BIF:268`).
fn without_nulls(args: BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    args.into_iter().filter(|(_, v)| !v.is_null()).collect()
}

impl ScriptHost<'_> {
    /// A built-in's answer; a later answer's token belongs to this run.
    fn started(&mut self, out: HostOutcome) -> HostOutcome {
        if let HostOutcome::Started(token) = out {
            self.app.script_tokens.insert(token, self.run);
        }
        out
    }

    /// A method call (`SFC:512-521`, `iTermObject.m:79-91`): the receiver is looked up for
    /// each call (`object_not_found` when it names nothing now), then the method its catalog
    /// has for the signature, else "No method found ...", as unregistered.
    fn method(&mut self, id: &str, sig: &str, args: &BTreeMap<String, Value>, sync: bool, side_effects: bool) -> HostOutcome {
        let Some((on, key)) = self.app.receiver(id, sig) else { return HostOutcome::Failed(Failure::plain(script_eval::object_not_found(id))) };
        let Some(b) = builtin_fns::find(on, sig) else {
            return HostOutcome::Failed(Failure { reason: builtin_fns::no_method(on, id, sig), detail: None, kind: FailKind::Unregistered, conn: None });
        };
        if let Err(e) = builtin_fns::type_check(b, args) {
            return HostOutcome::Failed(Failure::plain(e));
        }
        let out = self.app.builtin(b, without_nulls(args.clone()), Some((on, key)), side_effects, sync);
        self.started(out)
    }
}

impl Host for ScriptHost<'_> {
    fn call(&mut self, call: &Call, args: &BTreeMap<String, Value>, sync: bool, side_effects: bool) -> HostOutcome {
        let sig = call.signature();
        if let Some(id) = self.receiver.filter(|_| self.methods.contains(&call.site)) {
            return self.method(id, &sig, args, sync, side_effects);
        }
        // Built-ins first (`SFC:510-525`, `BIF:237-268`): the explicit arguments type-checked,
        // then the defaults filled from the scope.
        if let Some(b) = builtin_fns::find(builtin_fns::On::Function, &sig) {
            if let Err(e) = builtin_fns::type_check(b, args) {
                return HostOutcome::Failed(Failure::plain(e));
            }
            let amended = builtin_fns::amend(b, args, self.scope);
            let out = self.app.builtin(b, without_nulls(amended), None, side_effects, sync);
            return self.started(out);
        }
        if let Some(answer) = script_gate(&sig, sync, self.app.api_on) {
            return answer;
        }
        let fq = call.fq_name();
        let Some((conn, params)) = self.app.api_fns.resolve(&fq, args, self.scope) else {
            let similar = self.app.api_fns.similar(&fq).or_else(|| builtin_fns::similar(builtin_fns::On::Function, &fq));
            let reason = script_fn::unregistered_text(&sig, similar.as_deref());
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

/// The scope of a method's receiver.
fn receiver_scope(on: builtin_fns::On, key: u64) -> ScopeSpec {
    match on {
        builtin_fns::On::Session => ScopeSpec::Session { pane: key, frame: Values::new() },
        builtin_fns::On::Tab => ScopeSpec::Tab(key),
        builtin_fns::On::Window => ScopeSpec::Window(key),
        builtin_fns::On::Function => ScopeSpec::App,
    }
}

impl App {
    /// Starts an invocation (`callFunction`, `SFC:203-276`) and takes its first step. The
    /// run's id while it waits; None when it finished at once (its result already went to
    /// its origin), including a syntax error or a receiver that names no object.
    pub(super) fn invoke(&mut self, invocation: &str, scope: ScopeSpec, timeout: Timeout, side_effects: bool, origin: Origin) -> Option<u64> {
        let lax = self.globals.bool("laxNilPolicyInInterpolatedStrings");
        let inv = match Invocation::new(invocation, timeout, side_effects, lax, Instant::now()) {
            Ok(inv) => inv,
            Err(f) => {
                self.script_answer(&origin, invocation, Err(f));
                return None;
            }
        };
        // A method invocation runs in its receiver's scope, the object whose catalog has the
        // first call's method.
        let (scope, receiver) = match scope {
            ScopeSpec::Receiver(id) => {
                let sig = inv.calls().first().map(|c| c.signature()).unwrap_or_default();
                let Some((on, key)) = self.receiver(&id, &sig) else {
                    self.script_answer(&origin, invocation, Err(Failure::plain(script_eval::object_not_found(&id))));
                    return None;
                };
                (receiver_scope(on, key), Some(id))
            }
            other => (other, None),
        };
        // An object already gone reads as an empty scope.
        let base = self.script_values(&scope).unwrap_or_default();
        let id = self.script_next;
        self.script_next += 1;
        self.script_runs.insert(id, Run { inv, scope, base, origin, receiver });
        self.script_step(id);
        self.script_runs.contains_key(&id).then_some(id)
    }

    /// The values a scope reads (11b's `scope_values` with the API's typed user values): a
    /// session's with its selection, a tab's focused session's in tab scope, a window's
    /// current tab's focused session's in window scope, any session's app scope (else an
    /// empty one). A receiver is its object's (`receiver`). None when the object is gone.
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
            ScopeSpec::Receiver(id) => match self.receiver(id, "")? {
                (builtin_fns::On::Session, p) => session(p),
                (builtin_fns::On::Tab, t) => tab(t),
                (builtin_fns::On::Window, k) => window(k),
                (builtin_fns::On::Function, _) => None,
            },
        }
    }

    /// Steps a run against the values its scope had when it started; a finished run goes to
    /// its origin. The run is out of `script_runs` while it steps (the host borrows the App).
    fn script_step(&mut self, id: u64) {
        let Some(mut run) = self.script_runs.remove(&id) else { return };
        let base = std::mem::take(&mut run.base);
        let frame = match &run.scope {
            ScopeSpec::Session { frame, .. } => frame.clone(),
            _ => Values::new(),
        };
        let scope = Layered { frame: &frame, base: &base };
        let receiver = run.receiver.clone();
        let methods = if receiver.is_some() { run.inv.calls().iter().map(|c| c.site).collect() } else { BTreeSet::new() };
        let step = run.inv.step(&scope, &mut ScriptHost { app: self, run: id, scope: &scope, receiver: receiver.as_deref(), methods });
        run.base = base;
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
        let moving = self.script_moved_tabs();
        for (token, f) in self.api_fns.expire(now) {
            self.api_rpc_done.push_back((token, Err(f)));
        }
        for id in deliver_results(&mut self.script_runs, &mut self.script_tokens, &mut self.api_rpc_done) {
            self.script_step(id);
        }
        for id in late_runs(&self.script_runs, now) {
            let Some(run) = self.script_runs.remove(&id) else { continue };
            let mut f = run.inv.timed_out();
            // A method invocation's calls carry the requester's connection (`H:4336`).
            let requester = match (&run.receiver, &run.origin) {
                (Some(_), Origin::Api { conn, .. }) => Some(*conn),
                _ => None,
            };
            f.conn = run.inv.returning().and_then(|t| self.api_fns.conn_of(t)).or(requester);
            self.script_log(f.conn, f.reason.clone());
            self.script_finished(run, Err(f));
        }
        // A tab moved to a new window is answered once the window opens, on the next pass.
        let soon = moving.then_some(now);
        next_wake(&self.script_runs, self.api_fns.next_deadline()).into_iter().chain(soon).min()
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
        let (heading, text) = alert.form();
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    fn run(text: &str, timeout: Timeout, now: Instant) -> Run {
        Run { inv: Invocation::new(text, timeout, true, true, now).unwrap(), scope: ScopeSpec::App, base: Values::new(), origin: Origin::Quiet, receiver: None }
    }

    /// Starts every script call with the next token.
    struct Starter(u64);

    impl Host for Starter {
        fn call(&mut self, _call: &Call, _args: &BTreeMap<String, Value>, _sync: bool, _side_effects: bool) -> HostOutcome {
            self.0 += 1;
            HostOutcome::Started(self.0)
        }
    }

    #[test]
    fn script_tick_follows_iterm2() {
        let t0 = Instant::now();
        let mut runs = BTreeMap::new();
        let mut a = run("f()", Timeout::After(Duration::from_secs(5)), t0);
        let mut b = run("g()", Timeout::Never, t0);
        let empty = Values::new();
        assert_eq!(a.inv.step(&empty, &mut Starter(10)), Step::Waiting);
        assert_eq!(b.inv.step(&empty, &mut Starter(20)), Step::Waiting);
        runs.insert(1, a);
        runs.insert(2, b);
        let mut tokens = HashMap::from([(11, 1), (21, 2)]);

        // A result reaches its run; a result for a forgotten token or a finished run is dropped.
        let mut done = VecDeque::from([(99, Ok(json!(0))), (21, Ok(json!("g"))), (21, Ok(json!("again")))]);
        assert_eq!(deliver_results(&mut runs, &mut tokens, &mut done), BTreeSet::from([2]));
        assert!(done.is_empty() && !tokens.contains_key(&21));
        assert_eq!(runs.get_mut(&2).unwrap().inv.step(&empty, &mut Starter(30)), Step::Done(json!("g")));

        // The run with a timer is late at its deadline, not before; the untimed one never.
        assert!(late_runs(&runs, t0 + Duration::from_secs(4)).is_empty());
        assert_eq!(late_runs(&runs, t0 + Duration::from_secs(5)), [1]);
        // The next wake is the earliest of the run timers and the registry's.
        let five = t0 + Duration::from_secs(5);
        assert_eq!(next_wake(&runs, None), Some(five));
        assert_eq!(next_wake(&runs, Some(t0 + Duration::from_secs(1))), Some(t0 + Duration::from_secs(1)));
        assert_eq!(next_wake(&BTreeMap::new(), None), None);
    }

    #[test]
    fn script_host_follows_iterm2() {
        assert_eq!(script_gate("f()", true, true), Some(HostOutcome::Done(Value::Null)), "a synchronous evaluation runs no script");
        assert_eq!(script_gate("f()", true, false), Some(HostOutcome::Done(Value::Null)));
        assert_eq!(
            script_gate("f(a)", false, false),
            Some(HostOutcome::Failed(Failure { reason: "Undefined function with signature \u{201c}f(a)\u{201d}".into(), detail: None, kind: FailKind::Disabled, conn: None }))
        );
        assert_eq!(script_gate("f()", false, true), None, "the registry is asked");
    }

    #[test]
    fn script_alerts_follow_iterm2() {
        let failed = Alert::Failed { heading: script_eval::report_heading("Trigger"), text: script_eval::report_text("f()", "boom") };
        assert_eq!(failed.form(), ("Trigger Function Call Failed", "Error running \u{201c}f()\u{201d}:\nboom"));
        assert_eq!(Alert::Result("f() returned:\n3".into()).form(), ("Function Call Result", "f() returned:\n3"));
    }
}

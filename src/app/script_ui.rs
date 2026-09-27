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
    Report(String),
    OpenQuickly,
    // Constructed by the callers of Tasks 7 to 9.
    #[allow(dead_code)]
    Title(PaneId),
    #[allow(dead_code)]
    Component { pane: PaneId, id: String },
    #[allow(dead_code)]
    Click,
    /// A status bar string's evaluation: its cache entry (`script_templates`) and the
    /// evaluation's number there, so an older one never replaces a newer one's value.
    Template { pane: PaneId, key: String, nth: u64 },
    #[allow(dead_code)]
    Quiet,
}

/// A status bar string with function calls (`iTermGenericEvaluator.m`): the paths its parse
/// names (or its syntax error), parsed once; the value shown (empty before the first
/// evaluation finishes), the evaluation still running, the dependencies it started with, the
/// newest evaluation whose value was applied, the functions it found unregistered, the
/// registry generation it last checked them against, and the last bar layout that asked for
/// it (an entry no layout asks for any more goes).
#[derive(Clone, Debug, Default)]
pub(super) struct Template {
    pub paths: Option<Result<BTreeSet<String>, String>>,
    pub value: Option<Result<String, String>>,
    pub running: Option<u64>,
    pub deps: String,
    pub applied: u64,
    pub missing: BTreeSet<String>,
    pub generation: u64,
    pub pass: u64,
}

impl Template {
    /// Whether an evaluation starts now (`reevaluateIfNeeded`, `dependencyDidChange`,
    /// `registeredFunctionsDidChange:`): nothing ran yet, the dependencies changed since the
    /// last start, or, once per change of the registered functions (`generation`), a
    /// function it missed is registered now; those alone leave `missing`. Never while its own
    /// evaluation runs with the same dependencies.
    fn needs_start(&mut self, deps: &str, generation: u64, registered: impl Fn(&str) -> bool) -> bool {
        let mut start = (self.value.is_none() && self.running.is_none()) || self.deps != deps;
        if self.generation != generation {
            self.generation = generation;
            let before = self.missing.len();
            self.missing.retain(|s| !registered(s));
            start |= self.missing.len() != before;
        }
        start
    }

    /// An evaluation finished (`evaluateSynchronously:sideEffectsAllowed:`): dropped when a
    /// newer one's value was applied, else its value replaces the shown one. True when the
    /// shown value changed.
    fn finish(&mut self, nth: u64, missing: &BTreeSet<String>, out: &expr::Outcome) -> bool {
        if nth < self.applied {
            return false;
        }
        self.applied = nth;
        if self.running == Some(nth) {
            self.running = None;
        }
        self.missing.extend(missing.iter().cloned());
        let Some(value) = script_eval::template_outcome(out) else { return false };
        let changed = self.value.as_ref() != Some(&value);
        self.value = Some(value);
        changed
    }
}

/// Drops `pane`'s entries not stamped with `pass`: its bar's layout no longer asks for them.
fn sweep<T>(map: &mut HashMap<(PaneId, String), T>, pane: PaneId, pass: u64, pass_of: impl Fn(&T) -> u64) {
    map.retain(|(p, _), t| *p != pane || pass_of(t) == pass);
}

/// Status bar strings have iTerm2's interpolated string timeout (`iTermGenericEvaluator.m`,
/// 30 s asynchronously).
const TEMPLATE_TIMEOUT_SECS: f64 = 30.0;

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

/// The origin names of the callers (`PS:12220-12224`, `PS:13431-13433`, `PS:25443-25452`,
/// `OQW:492`).
pub(super) const TRIGGER: &str = "Trigger";
pub(super) const KEY_BINDING: &str = "Key Binding";
pub(super) const POINTER_ACTION: &str = "Pointer action";
pub(super) const OPEN_QUICKLY: &str = "Open Quickly";

/// An alert waiting to show: a failed call's heading and text, or an invocation's result
/// (Open Quickly, Task 5).
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Alert {
    Failed {
        heading: String,
        text: String,
    },
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

/// Queues a failure's alert (`reportFunctionCallError`, `H:566-591`): none while the
/// temporary silence holds (`iTermWarning.m` `preempt:`), and none when the same heading and
/// text is already queued or showing, so a burst of one failure (a trigger matching many
/// times) shows once.
fn queue_alert(queue: &mut VecDeque<Alert>, showing: Option<&Alert>, alert: Alert, silenced: bool) {
    if silenced || showing == Some(&alert) || queue.contains(&alert) {
        return;
    }
    queue.push_back(alert);
}

/// Unix seconds now, the clock of the alert's silence.
fn unix_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
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

/// A method call's target (`SFC:512-521`, `iTermObject.m:55-91`): the receiver looked up
/// again for each call with that call's signature ("No object with ID" when it names nothing
/// now), then the method its catalog has for the signature ("No method found ...", as
/// unregistered), its explicit arguments type-checked.
fn method_target(lookup: &mut dyn FnMut(&str, &str) -> Option<(builtin_fns::On, u64)>, id: &str, sig: &str, args: &BTreeMap<String, Value>) -> Result<((builtin_fns::On, u64), &'static builtin_fns::Builtin), Failure> {
    let (on, key) = lookup(id, sig).ok_or_else(|| Failure::plain(builtin_fns::no_object(id)))?;
    let b = builtin_fns::find(on, sig).ok_or_else(|| Failure { reason: builtin_fns::no_method(on, id, sig), detail: None, kind: FailKind::Unregistered, conn: None })?;
    builtin_fns::type_check(b, args).map_err(Failure::plain)?;
    Ok(((on, key), b))
}

/// The connection a timed-out run's failure carries (`SFC:396-401`): the one serving the call
/// it waits to return from, else for a method invocation the requester's (`H:4331-4336`).
fn timeout_conn(run: &Run, conn_of: impl Fn(u64) -> Option<u64>) -> Option<u64> {
    let requester = match (&run.receiver, &run.origin) {
        (Some(_), Origin::Api { conn, .. }) => Some(*conn),
        _ => None,
    };
    run.inv.returning().and_then(conn_of).or(requester)
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

    /// A method call: its target (`method_target`, the receiver looked up for this call),
    /// then the built-in method runs.
    fn method(&mut self, id: &str, sig: &str, args: &BTreeMap<String, Value>, sync: bool, side_effects: bool) -> HostOutcome {
        let app = &*self.app;
        match method_target(&mut |i, s| app.receiver(i, s), id, sig, args) {
            Ok((receiver, b)) => {
                let out = self.app.builtin(b, without_nulls(args.clone()), Some(receiver), side_effects, sync);
                self.started(out)
            }
            Err(f) => HostOutcome::Failed(f),
        }
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
    /// The session scope of `pane`, else the app's (key action 60 with no session,
    /// `PS:11799-11813`; Open Quickly with no current session, `OQM:950-1003`).
    pub(super) fn session_or_app(pane: Option<PaneId>) -> ScopeSpec {
        pane.map_or(ScopeSpec::App, |pane| ScopeSpec::Session { pane, frame: Values::new() })
    }

    /// Starts an invocation (`callFunction`, `SFC:203-276`) and takes its first step. The
    /// run's id while it waits; None when it finished at once (its result already went to
    /// its origin), including a syntax error or a receiver that names no object.
    pub(super) fn invoke(&mut self, invocation: &str, scope: ScopeSpec, timeout: Timeout, side_effects: bool, origin: Origin) -> Option<u64> {
        let lax = self.globals.bool("laxNilPolicyInInterpolatedStrings");
        let inv = Invocation::new(invocation, timeout, side_effects, lax, Instant::now());
        self.start_run(invocation, inv, scope, origin)
    }

    /// Starts an invocation already parsed (a call sequence, or an interpolated string);
    /// as `invoke`.
    fn start_run(&mut self, invocation: &str, inv: Result<Invocation, Failure>, scope: ScopeSpec, origin: Origin) -> Option<u64> {
        let inv = match inv {
            Ok(inv) => inv,
            Err(f) => {
                self.script_answer(&origin, invocation, Err(f), &BTreeSet::new());
                return None;
            }
        };
        // A method invocation runs in its receiver's scope, the object whose catalog has the
        // first call's method.
        let (scope, receiver) = match scope {
            ScopeSpec::Receiver(id) => {
                let sig = inv.calls().first().map(|c| c.signature()).unwrap_or_default();
                let Some((on, key)) = self.receiver(&id, &sig) else {
                    self.script_answer(&origin, invocation, Err(Failure::plain(script_eval::object_not_found(&id))), &BTreeSet::new());
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
        let session = |pane: PaneId| self.session_values(pane, true);
        // The object catalog only for the scopes that read it (a status bar string reads a
        // session's scope every frame).
        let window = |k: u64| -> Option<Values> {
            let guid = self.api_ids().windows.iter().find(|(w, _, _)| *w == k).map(|(_, g, _)| g.clone())?;
            Some(self.api_objects().values(Scope::Window, &guid))
        };
        let tab = |t: u64| -> Option<Values> { self.api_ids().tab_window(t).map(|_| self.api_objects().values(Scope::Tab, &t.to_string())) };
        match spec {
            ScopeSpec::Session { pane, .. } => session(*pane),
            ScopeSpec::Tab(t) => tab(*t),
            ScopeSpec::Window(k) => window(*k),
            ScopeSpec::App => Some(self.api_objects().values(Scope::App, "")),
            ScopeSpec::Receiver(id) => match self.receiver(id, "")? {
                (builtin_fns::On::Session, p) => session(p),
                (builtin_fns::On::Tab, t) => tab(t),
                (builtin_fns::On::Window, k) => window(k),
                (builtin_fns::On::Function, _) => None,
            },
        }
    }

    /// A session's scope values (`script_values`), with its selection only when asked (a
    /// copy of the selection is costly and a status bar string reads it every frame). None
    /// when the pane is gone.
    pub(super) fn session_values(&self, pane: PaneId, selection: bool) -> Option<Values> {
        if !self.panes.contains_key(&pane) {
            return None;
        }
        let vars = self.variables_with(pane, selection);
        let mut out = api_vars::scope_values(&vars, Scope::Session);
        api_vars::typed_users(&self.api_vars, &vars, Scope::Session, &pane.to_string(), &mut out);
        Some(out)
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
            f.conn = timeout_conn(&run, |t| self.api_fns.conn_of(t));
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
        self.script_answer(&run.origin, run.inv.text(), r, run.inv.missing());
    }

    /// Where a result goes: an InvokeFunctionResponse (`H:4376-4394`), the error alert with
    /// the origin's name, a status bar string's cache (with the functions it missed), or
    /// nowhere.
    fn script_answer(&mut self, origin: &Origin, invocation: &str, r: Result<Value, Failure>, missing: &BTreeSet<String>) {
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
            // `OQW:481-503`: a failure reported as Open Quickly's, a value shown.
            Origin::OpenQuickly => match r {
                Ok(v) => self.script_alerts.push_back(Alert::Result(script_eval::result_text(invocation, &v))),
                Err(f) => self.report_function_error(OPEN_QUICKLY, invocation, &f),
            },
            Origin::Template { pane, key, nth } => self.template_finished(*pane, key, *nth, missing, r),
            // The callers of Tasks 7 to 9 take their results there.
            Origin::Title(_) | Origin::Component { .. } | Origin::Click | Origin::Quiet => {}
        }
    }

    /// A status bar string's value (Interpolated String, a Custom Action's title): one with no
    /// function call evaluates at once, as before; one with calls shows its cached value
    /// (empty before the first) and starts an evaluation when `Template::needs_start` says
    /// so. The first evaluation is preceded by a synchronous one, script calls answering
    /// nothing (`reevaluateIfNeeded`); evaluations have no side effects
    /// (`SWS:117-119`, `ACT:72-74`) and time out after 30 s. The string is parsed once, and
    /// its dependencies read the selection only when it names it.
    pub(super) fn template(&mut self, pane: PaneId, template: &str) -> Result<String, String> {
        let key = (pane, template.to_string());
        if !self.script_templates.contains_key(&key) {
            if !crate::interp::has_calls(template) {
                return self.evaluate_for(pane, template);
            }
            let paths = expr::parse_template(template).map(|parts| script_eval::template_paths(&parts));
            self.script_templates.insert(key.clone(), Template { paths: Some(paths), ..Template::default() });
        }
        let pass = self.script_pass;
        let Some(entry) = self.script_templates.get_mut(&key) else { return Ok(String::new()) };
        entry.pass = pass;
        let paths = match entry.paths.as_ref() {
            Some(Ok(paths)) => paths.clone(),
            Some(Err(e)) => return Err(e.clone()),
            None => BTreeSet::new(),
        };
        let selection = paths.iter().any(|p| crate::prompted::names_selection(p));
        let deps = script_eval::deps_key(&paths, &self.session_values(pane, selection).unwrap_or_default());
        let fns = &self.api_fns;
        let Some(entry) = self.script_templates.get_mut(&key) else { return Ok(String::new()) };
        if entry.needs_start(&deps, fns.generation(), |sig| fns.has_signature(sig)) {
            let first = entry.value.is_none();
            entry.deps = deps;
            if first {
                self.start_template(pane, template, Timeout::Sync);
            }
            self.start_template(pane, template, Timeout::from_secs(TEMPLATE_TIMEOUT_SECS));
        }
        self.script_templates.get(&key).and_then(|t| t.value.clone()).unwrap_or(Ok(String::new()))
    }

    /// A bar's layout is about to ask for its strings: entries it asks for are stamped with
    /// this pass.
    pub(super) fn begin_script_pass(&mut self) -> u64 {
        self.script_pass += 1;
        self.script_pass
    }

    /// After a bar asked for its strings: the pane's entries it did not ask for (an edited
    /// text, a removed component) go, with any evaluation still running for them.
    pub(super) fn end_script_pass(&mut self, pane: PaneId, pass: u64) {
        sweep(&mut self.script_templates, pane, pass, |t| t.pass);
    }

    /// Starts one evaluation of a status bar string, recorded as its entry's running one.
    fn start_template(&mut self, pane: PaneId, template: &str, timeout: Timeout) {
        let nth = self.script_next;
        self.script_next += 1;
        if let Some(t) = self.script_templates.get_mut(&(pane, template.to_string())) {
            t.running = Some(nth);
        }
        let lax = self.globals.bool("laxNilPolicyInInterpolatedStrings");
        let inv = Invocation::for_template(template, timeout, false, lax, Instant::now());
        let origin = Origin::Template { pane, key: template.to_string(), nth };
        self.start_run(template, inv, ScopeSpec::Session { pane, frame: Values::new() }, origin);
    }

    /// A status bar string's evaluation finished: its entry takes the value (unless a newer
    /// evaluation's came first) and the bars redraw when it changed. A failure goes to the
    /// failing connection's log (`iTermGenericEvaluator.m`). An entry gone with its pane
    /// takes nothing.
    fn template_finished(&mut self, pane: PaneId, key: &str, nth: u64, missing: &BTreeSet<String>, r: Result<Value, Failure>) {
        if let Err(f) = &r {
            self.script_log(f.conn, script_eval::template_failed_text(key, &f.reason));
        }
        let out = match r {
            Ok(v) => expr::Outcome::Done(v),
            Err(f) => expr::Outcome::Failed(f),
        };
        let Some(t) = self.script_templates.get_mut(&(pane, key.to_string())) else { return };
        if t.finish(nth, missing, &out) {
            self.redraw();
            self.redraw_all();
        }
    }

    /// iTerm2's `reportFunctionCallError` (`H:566-591`): the console text in the log of the
    /// connection that failed, and the alert queued (`queue_alert`: OK, temporarily
    /// silenceable under `NoSyncFunctionCallError`, the same one merged).
    pub(super) fn report_function_error(&mut self, origin: &str, invocation: &str, f: &Failure) {
        self.script_log(f.conn, script_eval::console_text(invocation, &f.reason, f.detail.as_deref().unwrap_or_default()));
        let alert = Alert::Failed { heading: script_eval::report_heading(origin), text: script_eval::report_text(invocation, &f.reason) };
        let silenced = self.function_errors_silenced();
        let showing = self.script_alert.as_ref().map(|(_, a)| a);
        queue_alert(&mut self.script_alerts, showing, alert, silenced);
    }

    /// Whether the person silenced the function call error alert for ten minutes.
    fn function_errors_silenced(&self) -> bool {
        let until = self.globals.get(script_eval::SILENCE_UNTIL_KEY).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)));
        script_eval::silenced(until, unix_now())
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
        // Failures queued before the person silenced the alert are not shown either (each
        // warning checks the silence when it would show, `preempt:`).
        if self.function_errors_silenced() {
            self.script_alerts.retain(|a| !matches!(a, Alert::Failed { .. }));
        }
        let Some(alert) = self.script_alerts.front() else { return };
        let Some(g) = self.gpu.as_ref() else { return };
        let (heading, text) = alert.form();
        let failed = matches!(alert, Alert::Failed { .. });
        let opened = ConfirmWindow::actions(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, heading, text, &[crate::api_auth::OK], false).map(|mut w| {
            if failed {
                w.set_checkbox_label(ConfirmWindow::SUPPRESS_TEN_MINUTES);
            }
            w
        });
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
        match answer {
            ConfirmResult::Open => return true,
            // "Suppress this message for ten minutes" checked on a failure's OK
            // (`iTermWarning.m:577-593`): the silence and the remembered selection.
            ConfirmResult::Action { suppress: true, .. } if matches!(self.script_alert, Some((_, Alert::Failed { .. }))) => {
                self.globals.set(script_eval::SILENCE_UNTIL_KEY, toml::Value::Float(unix_now() + script_eval::TEMPORARY_SILENCE_SECS));
                self.globals.set(script_eval::SELECTION_KEY, toml::Value::Integer(0));
                self.save_globals();
            }
            _ => {}
        }
        self.script_alert = None;
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

    /// Runs a method invocation's calls through `method_target`, recording each lookup; the
    /// receiver answers as a session for the first `alive` lookups, then is gone.
    struct Methods {
        lookups: Vec<(String, String)>,
        alive: usize,
    }

    impl Host for Methods {
        fn call(&mut self, call: &Call, args: &BTreeMap<String, Value>, _sync: bool, _side_effects: bool) -> HostOutcome {
            let (lookups, alive) = (&mut self.lookups, self.alive);
            let mut lookup = |id: &str, sig: &str| {
                lookups.push((id.to_string(), sig.to_string()));
                (lookups.len() <= alive).then_some((builtin_fns::On::Session, 7))
            };
            match method_target(&mut lookup, "7", &call.signature(), args) {
                Ok(_) => HostOutcome::Done(Value::Null),
                Err(f) => HostOutcome::Failed(f),
            }
        }
    }

    #[test]
    fn method_calls_follow_iterm2() {
        let empty = Values::new();
        // The receiver is looked up again for each call, with that call's signature.
        let mut r = run("iterm2.set_name(name: \"a\"); iterm2.stop_coprocess()", Timeout::Never, Instant::now());
        let mut host = Methods { lookups: Vec::new(), alive: 9 };
        assert_eq!(r.inv.step(&empty, &mut host), Step::Done(Value::Null));
        assert_eq!(host.lookups, [("7".to_string(), "iterm2.set_name(name)".to_string()), ("7".to_string(), "iterm2.stop_coprocess()".to_string())]);
        // Gone by the second call: "No object with ID" (`iTermObject.m:61-67`), a plain failure.
        let mut r = run("iterm2.set_name(name: \"a\"); iterm2.stop_coprocess()", Timeout::Never, Instant::now());
        let mut host = Methods { lookups: Vec::new(), alive: 1 };
        let step = r.inv.step(&empty, &mut host);
        assert!(matches!(&step, Step::Failed(f) if f.reason == "No object with ID 7" && f.kind != FailKind::Unregistered), "{step:?}");
        // A method the receiver lacks: unregistered; a bad argument type: the mismatch.
        let mut none = |_: &str, _: &str| Some((builtin_fns::On::Tab, 3));
        let f = method_target(&mut none, "3", "iterm2.set_name(name)", &BTreeMap::new()).unwrap_err();
        assert_eq!((f.reason.as_str(), f.kind), ("No method found on <Tab id=3> with signature iterm2.set_name(name)", FailKind::Unregistered));
        let mut session = |_: &str, _: &str| Some((builtin_fns::On::Session, 7));
        assert!(method_target(&mut session, "7", "iterm2.set_name(name)", &BTreeMap::from([("name".to_string(), json!(3))])).is_err());

        // A timeout carries the connection serving the waited call, else a method
        // invocation's requester, else none.
        let t0 = Instant::now();
        let mut r = run("f()", Timeout::After(Duration::from_secs(1)), t0);
        assert_eq!(r.inv.step(&empty, &mut Starter(40)), Step::Waiting);
        assert_eq!(timeout_conn(&r, |_| None), None, "no connection and no requester");
        assert_eq!(timeout_conn(&r, |t| (t == 41).then_some(5)), Some(5));
        r.receiver = Some("7".into());
        r.origin = Origin::Api { conn: 9, req: pb::ClientOriginatedMessage::default() };
        assert_eq!(timeout_conn(&r, |_| None), Some(9), "the requester's");
        assert_eq!(timeout_conn(&r, |t| (t == 41).then_some(5)), Some(5), "the serving connection first");
        r.receiver = None;
        assert_eq!(timeout_conn(&r, |_| None), None, "an InvokeFunction that is not a method");
    }

    /// The status bar string cache (`iTermGenericEvaluator.m`): the first evaluation, a
    /// dependency change, a missed function registered; never a second start with the same
    /// dependencies; an older evaluation's value never replaces a newer one's.
    #[test]
    fn template_cache_follows_iterm2() {
        let none = |_: &str| false;
        let mut t = Template::default();
        assert!(t.needs_start("d", 0, none), "nothing ran yet");
        t.deps = "d".into();
        t.running = Some(5);
        assert!(!t.needs_start("d", 0, none), "its evaluation runs");
        assert!(t.needs_start("e", 0, none), "a dependency changed");
        // The synchronous pass (4) then the asynchronous one (5).
        assert!(t.finish(4, &BTreeSet::new(), &expr::Outcome::Done(json!("sync"))));
        assert_eq!((t.value.clone(), t.running), (Some(Ok("sync".into())), Some(5)));
        assert!(t.finish(5, &BTreeSet::from(["f()".to_string()]), &expr::Outcome::Failed(Failure::plain("No function"))));
        assert_eq!((t.value.clone(), t.running), (Some(Err("No function".into())), None));
        assert!(!t.needs_start("d", 0, none));
        // Review finding 4: once per change of the registered functions, and only the newly
        // registered names leave `missing`.
        t.missing.insert("g()".into());
        assert!(!t.needs_start("d", 0, |s| s == "f()"), "no registration happened");
        assert!(t.needs_start("d", 1, |s| s == "f()"), "the missed function is registered now");
        assert_eq!(t.missing, BTreeSet::from(["g()".to_string()]));
        assert!(!t.needs_start("d", 1, |s| s == "f()" || s == "g()"), "once per registration");
        assert!(!t.needs_start("d", 2, |s| s == "f()"), "another registration, none it missed");
        // Two running: the newer finishes first, then the older is dropped.
        t.running = Some(8);
        assert!(t.finish(8, &BTreeSet::new(), &expr::Outcome::Done(json!("new"))));
        assert!(!t.finish(7, &BTreeSet::new(), &expr::Outcome::Done(json!("old"))));
        assert_eq!(t.value, Some(Ok("new".into())));
        // An older one finishing while a newer runs is shown until the newer's comes.
        t.running = Some(10);
        assert!(t.finish(9, &BTreeSet::new(), &expr::Outcome::Done(json!("nine"))));
        assert_eq!((t.value.clone(), t.running), (Some(Ok("nine".into())), Some(10)));
        assert!(!t.finish(10, &BTreeSet::new(), &expr::Outcome::Done(json!("nine"))), "the same value");
        assert_eq!(t.running, None);

        // Review finding 5: a pane's entries its layout did not ask for this pass go; other
        // panes' stay.
        let stamp = |pass| Template { pass, ..Template::default() };
        let mut map = HashMap::from([((1, "old".to_string()), stamp(3)), ((1, "new".to_string()), stamp(4)), ((2, "other".to_string()), stamp(1))]);
        sweep(&mut map, 1, 4, |t| t.pass);
        let mut keys: Vec<_> = map.keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, [(1, "new".to_string()), (2, "other".to_string())]);
    }

    /// Review finding 1: an Invoke Script Function trigger whose function is not registered
    /// fails on every match; 1000 matches queue one alert, none while silenced, none while
    /// the same one shows; another failure still queues.
    #[test]
    fn trigger_alerts_are_bounded() {
        let failure = |sig: &str| Alert::Failed { heading: script_eval::report_heading(TRIGGER), text: script_eval::report_text(sig, &script_fn::unregistered_text(sig, None)) };
        let mut queue = VecDeque::new();
        for _ in 0..1000 {
            queue_alert(&mut queue, None, failure("f()"), false);
        }
        assert_eq!(queue.len(), 1);
        queue_alert(&mut queue, None, failure("g()"), false);
        assert_eq!(queue.len(), 2, "a different failure");
        let showing = queue.pop_front();
        queue_alert(&mut queue, showing.as_ref(), failure("f()"), false);
        assert_eq!(queue.len(), 1, "the one showing");
        let mut quiet = VecDeque::new();
        queue_alert(&mut quiet, None, failure("f()"), true);
        assert!(quiet.is_empty(), "silenced");
        // The silence: ten minutes from the OK (`iTermWarning.m:13`, `:632-636`).
        assert_eq!((script_eval::SILENCE_UNTIL_KEY, script_eval::TEMPORARY_SILENCE_SECS), ("NoSyncFunctionCallError_SilenceUntil", 600.0));
        assert!(script_eval::silenced(Some(100.0), 99.0));
        assert!(!script_eval::silenced(Some(100.0), 100.0));
        assert!(!script_eval::silenced(None, 0.0));
        assert_eq!(crate::prompt_ui::ConfirmWindow::SUPPRESS_TEN_MINUTES, "Suppress this message for ten minutes");
    }

    #[test]
    fn script_alerts_follow_iterm2() {
        let failed = Alert::Failed { heading: script_eval::report_heading("Trigger"), text: script_eval::report_text("f()", "boom") };
        assert_eq!(failed.form(), ("Trigger Function Call Failed", "Error running \u{201c}f()\u{201d}:\nboom"));
        assert_eq!(Alert::Result("f() returned:\n3".into()).form(), ("Function Call Result", "f() returned:\n3"));
    }
}

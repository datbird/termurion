//! Running invocations (`SFC:203-276`, `PS:11657-11673`): each is a `Run` of the pure
//! `script_eval::Invocation` with the scope it reads and where its result goes. The host
//! routes a call to a built-in (`builtin_fns`, run by `builtin_ui`), a method invocation's
//! calls to its receiver, and the rest, while the Python API runs, to the script that
//! registered it (`script_fn`); script results and timeouts come back through
//! `api_rpc_done` and `script_tick` steps the runs they finish. Failures reported to the
//! person show iTerm2's "Function Call Failed" alert (`H:566-591`), one at a time.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::rc::Rc;
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
use crate::script_statusbar;
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
/// with the origin's name, or one of the callers (triggers, bindings, the status bar, title
/// providers, menus).
#[derive(Clone, Debug)]
pub(super) enum Origin {
    Api {
        conn: u64,
        req: pb::ClientOriginatedMessage,
    },
    Report(String),
    OpenQuickly,
    /// A session title provider's evaluation: the session and the evaluation's number there
    /// (`script_titles`).
    Title { pane: PaneId, nth: u64 },
    /// A script status bar component's evaluation: its entry (`script_components`), the
    /// evaluation's number there, and the registration it ran (for the failure text).
    Component { pane: PaneId, id: String, nth: u64, reg: Box<pb::RpcRegistrationRequest> },
    /// A script component's click (`RPC:560-571`): a failure goes to the console log.
    Click,
    /// A status bar string's evaluation: its cache entry (`script_templates`) and the
    /// evaluation's number there, so an older one never replaces a newer one's value.
    Template { pane: PaneId, key: String, nth: u64 },
}

/// A status bar string with function calls (`iTermGenericEvaluator.m`): the paths its parse
/// names (or its syntax error), parsed once; the value shown (empty before the first
/// evaluation finishes), the evaluation still running, the dependencies it started with, the
/// newest evaluation whose value was applied, the functions it found unregistered, the
/// registry generation it last checked them against, and the last bar layout that asked for
/// it (an entry no layout asks for any more goes). A string with no call keeps an entry with
/// `plain` set, so it is not parsed for calls again each frame.
#[derive(Clone, Debug, Default)]
pub(super) struct Template {
    pub plain: bool,
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

/// A session's title from a script's title provider (`SNC:98-228`): the provider's unique id
/// it evaluates, the value shown (a result as `provider_title` makes it; empty before the
/// first), the newest evaluation running, the invocation it last evaluated and the
/// dependencies it started with (the invocation and the values of the paths it names; empty:
/// never started), the title provider generation it last saw
/// (`Registry::title_generation`), and the newest evaluation whose value was applied (older
/// ones finishing later are dropped, `_appliedCount`).
#[derive(Clone, Debug, Default)]
pub(super) struct TitleState {
    pub unique_id: String,
    pub value: String,
    pub running: Option<u64>,
    pub invocation: String,
    pub deps: String,
    pub generation: u64,
    pub applied: u64,
}

/// Whether a title evaluation starts now.
#[derive(Clone, Copy, Debug, PartialEq)]
enum TitleStart {
    /// A synchronous pass first (it records the references and shows a script call's value as
    /// blank), then the asynchronous one (`setNeedsUpdate`, `updateIfNeeded` with no cached
    /// value): the first time, when the Title Function names another provider, and when a
    /// title provider registered (`didRegisterSessionTitleFunc:` clears the cached
    /// evaluation, `SNC:451-455`).
    Fresh,
    /// The asynchronous one alone (`setNeedsReevaluation`): a dependency changed.
    Again,
}

impl TitleState {
    /// Whether an evaluation starts now for `unique_id` with these dependencies at this
    /// title provider generation. A fresh start forgets the old value's evaluations but
    /// keeps the value until one replaces it.
    fn needs_start(&mut self, unique_id: &str, deps: &str, generation: u64) -> Option<TitleStart> {
        if self.deps.is_empty() || self.unique_id != unique_id || self.generation != generation {
            let applied = self.applied;
            let invocation = std::mem::take(&mut self.invocation);
            *self = TitleState { unique_id: unique_id.to_string(), value: std::mem::take(&mut self.value), invocation, deps: deps.to_string(), generation, applied, running: None };
            return Some(TitleStart::Fresh);
        }
        if self.deps != deps {
            self.deps = deps.to_string();
            return Some(TitleStart::Again);
        }
        None
    }

    /// An evaluation starts: the newest running one; a fresh start's first evaluation is the
    /// floor older results cannot pass.
    fn start(&mut self, nth: u64, fresh: bool) {
        self.running = Some(nth);
        if fresh {
            self.applied = nth;
        }
    }

    /// An evaluation finished with the title `value`: dropped when a newer one's value was
    /// applied. True when the title changed.
    fn finish(&mut self, nth: u64, value: String) -> bool {
        if nth < self.applied {
            return false;
        }
        self.applied = nth;
        if self.running == Some(nth) {
            self.running = None;
        }
        let changed = self.value != value;
        self.value = value;
        changed
    }
}

/// Title providers as the Title Function popup lists them: (display name, unique id).
fn provider_items(providers: &[script_fn::TitleProvider]) -> Vec<(String, String)> {
    providers.iter().map(|p| (p.display_name.clone(), p.unique_id.clone())).collect()
}

/// What a Custom session's title evaluates this pass: its registered provider's invocation
/// (true); with none registered, the invocation it last evaluated for this unique id, whose
/// references still decide when it evaluates again and then finds no provider (false); None
/// when it never evaluated one (the title shows the ellipsis and waits for a registration).
fn title_invocation(provider: Option<&str>, entry: Option<&TitleState>, unique_id: &str) -> Option<(String, bool)> {
    match (provider, entry) {
        (Some(inv), _) => Some((inv.to_string(), true)),
        (None, Some(t)) if t.unique_id == unique_id && !t.deps.is_empty() => Some((t.invocation.clone(), false)),
        (None, _) => None,
    }
}

/// A title provider's evaluation times out after 30 s (`SNC:128`).
const TITLE_TIMEOUT_SECS: f64 = 30.0;

/// Drops `pane`'s entries not stamped with `pass`: its bar's layout no longer asks for them.
fn sweep<T>(map: &mut HashMap<(PaneId, String), T>, pane: PaneId, pass: u64, pass_of: impl Fn(&T) -> u64) {
    map.retain(|(p, _), t| *p != pane || pass_of(t) == pass);
}

/// A script status bar component in one session (`RPC:145-155`): the variants shown (none
/// before the first evaluation, the ladybug after an error), an HTML component's variants as
/// text (made once per result), the error text a click shows, the evaluation running, the
/// newest evaluation whose result was applied, the dependencies (the invocation's paths with
/// `__knobs`) it last started with (empty: evaluate again), when its cadence comes due, a
/// beat that came while an evaluation ran (it evaluates again when that one finishes), the
/// functions it found unregistered, the registry generation it last checked them against,
/// and the last bar layout that asked for it.
#[derive(Clone, Debug, Default)]
pub(super) struct Component {
    pub variants: Vec<String>,
    pub texts: Option<Vec<String>>,
    pub error: Option<String>,
    pub running: Option<u64>,
    pub applied: u64,
    pub deps: String,
    pub due: Option<Instant>,
    pub beat: bool,
    pub missing: BTreeSet<String>,
    pub generation: u64,
    pub pass: u64,
}

/// What a script component's saved configuration gives, parsed once (the cache empties
/// whenever the registrations change, `script_registry_changed`): its identifier, the
/// registration it runs (the newest of its unique id), the invocation, the paths it names and
/// whether they name the selection.
#[derive(Clone, Debug)]
pub(super) struct Parsed {
    pub id: String,
    pub reg: pb::RpcRegistrationRequest,
    pub invocation: String,
    pub paths: BTreeSet<String>,
    pub selection: bool,
}

impl Component {
    /// Whether an evaluation starts now (`RPC:200-222`, `RPC:456-473`): the dependencies
    /// changed (the first time, a session variable it reads, its knobs, a cadence beat or its
    /// registration again, which empty `deps`), or, once per change of the registered
    /// functions, a function it missed is registered now (those alone leave `missing`).
    fn needs_start(&mut self, deps: &str, generation: u64, registered: impl Fn(&str) -> bool) -> bool {
        let mut start = self.deps != deps;
        if self.generation != generation {
            self.generation = generation;
            let before = self.missing.len();
            self.missing.retain(|s| !registered(s));
            start |= self.missing.len() != before;
        }
        start
    }

    /// An evaluation starts: its number is the newest running one, and the first one also
    /// becomes the floor older results (of an entry swept and made again) cannot pass.
    fn start(&mut self, nth: u64) {
        self.running = Some(nth);
        if self.applied == 0 {
            self.applied = nth;
        }
    }

    /// An evaluation finished: dropped when a newer one's result was already applied (Task
    /// 6's rule), else it replaces what shows (`RPC:413-449`). A beat that came while it ran
    /// empties the dependencies, so the next frame evaluates again. True when what shows
    /// changed or an evaluation is due.
    fn finish(&mut self, nth: u64, shown: Result<Vec<String>, String>, missing: Option<&BTreeSet<String>>) -> bool {
        if nth < self.applied {
            return false;
        }
        self.applied = nth;
        let mut due = false;
        if self.running == Some(nth) {
            self.running = None;
            if std::mem::take(&mut self.beat) {
                self.deps.clear();
                due = true;
            }
        }
        let (variants, error) = match shown {
            Ok(v) => (v, None),
            Err(e) => (vec![super::statusbar_content::LADYBUG.to_string()], Some(e)),
        };
        // A failure keeps the functions it missed (`RPC:447`); a success leaves them.
        if let Some(m) = missing {
            self.missing = m.clone();
        }
        let changed = self.variants != variants || self.error != error;
        if self.variants != variants {
            self.texts = None;
        }
        self.variants = variants;
        self.error = error;
        changed || due
    }

    /// A cadence beat (`RPC:475-481`; iTerm2's timer fires whatever runs,
    /// `iTermStatusBarContainerView.m:57`): the dependencies empty so the next frame that
    /// draws it evaluates again; while an evaluation runs, the beat waits for it to finish.
    /// The beat is used up either way (the next start sets the next one). True when the
    /// bars should redraw now.
    fn beat(&mut self) -> bool {
        self.due = None;
        if self.running.is_some() {
            self.beat = true;
            return false;
        }
        self.deps.clear();
        true
    }
}

/// A script component's click times out after 30 s (`RPC:561`).
const CLICK_TIMEOUT_SECS: f64 = 30.0;

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
    /// A failed call: the heading names its origin, `invocation` is what failed (with the
    /// heading, what merges repeats), `text` the reason as shown.
    Failed {
        heading: String,
        invocation: String,
        text: String,
    },
    Result(String),
    /// A failed script status bar component's text (`RPC:533-555`): persistent, OK only.
    Component(String),
}

impl Alert {
    /// The alert's heading and text.
    fn form(&self) -> (&str, &str) {
        match self {
            Alert::Failed { heading, text, .. } => (heading.as_str(), text.as_str()),
            Alert::Result(text) => (script_eval::RESULT_HEADING, text.as_str()),
            Alert::Component(text) => (script_statusbar::ERROR_HEADING, text.as_str()),
        }
    }
}

/// The most script alerts, or script dialogs, that wait in their queue (the one showing is
/// not counted): output-driven callers (a trigger matching many times) cannot grow either.
pub(super) const MAX_QUEUED: usize = 16;

/// Adds to a bounded queue; the oldest waiting item goes when it is full, and is returned.
pub(super) fn push_capped<T>(queue: &mut VecDeque<T>, item: T) -> Option<T> {
    queue.push_back(item);
    if queue.len() > MAX_QUEUED { queue.pop_front() } else { None }
}

/// Queues a failure's alert (`reportFunctionCallError`, `H:566-591`): none while the
/// temporary silence holds (`iTermWarning.m` `preempt:`); a failure of the same origin and
/// invocation as one showing is dropped, as one queued replaces that one's text (so a burst
/// of one failure, a trigger matching many times, shows once with the newest reason); other
/// alerts merge only when identical. At most `MAX_QUEUED` wait.
fn queue_alert(queue: &mut VecDeque<Alert>, showing: Option<&Alert>, alert: Alert, silenced: bool) {
    if silenced {
        return;
    }
    if let Alert::Failed { heading, invocation, text } = &alert {
        let same = |a: &Alert| matches!(a, Alert::Failed { heading: h, invocation: i, .. } if h == heading && i == invocation);
        if showing.is_some_and(same) {
            return;
        }
        if let Some(Alert::Failed { text: queued, .. }) = queue.iter_mut().find(|a| same(a)) {
            queued.clone_from(text);
            return;
        }
    } else if showing == Some(&alert) || queue.contains(&alert) {
        return;
    }
    push_capped(queue, alert);
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
                Ok(v) => {
                    push_capped(&mut self.script_alerts, Alert::Result(script_eval::result_text(invocation, &v)));
                }
                Err(f) => self.report_function_error(OPEN_QUICKLY, invocation, &f),
            },
            Origin::Template { pane, key, nth } => self.template_finished(*pane, key, *nth, missing, r),
            Origin::Component { pane, id, nth, reg } => self.component_finished(*pane, id, *nth, reg, invocation, missing, r),
            // `RPC:566-570`: "Error in onclick handler" in the console.
            Origin::Click => {
                if let Err(f) = r {
                    self.script_log(f.conn, script_statusbar::onclick_error(&f.reason, f.detail.as_deref().unwrap_or_default()));
                }
            }
            Origin::Title { pane, nth } => self.title_finished(*pane, *nth, invocation, r),
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
        let pass = self.script_pass;
        if !self.script_templates.contains_key(&key) {
            let entry = if crate::interp::has_calls(template) {
                let paths = expr::parse_template(template).map(|parts| script_eval::template_paths(&parts));
                Template { paths: Some(paths), ..Template::default() }
            } else {
                Template { plain: true, ..Template::default() }
            };
            self.script_templates.insert(key.clone(), entry);
        }
        let Some(entry) = self.script_templates.get_mut(&key) else { return Ok(String::new()) };
        entry.pass = pass;
        if entry.plain {
            return self.evaluate_for(pane, template);
        }
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
        sweep(&mut self.script_components, pane, pass, |c| c.pass);
    }

    /// A script component on a bar (`RPC:351-385`): its entry, after starting an evaluation
    /// when `Component::needs_start` says so. The invocation (`script_statusbar::invocation`,
    /// the newest registration of its unique id) runs in the session's scope with `__knobs`
    /// (the knob values' JSON) in a frame, side effects off (a Tandem ruling: iTerm2 allows
    /// them, `RPC:373`, but a component comes from profile data), with the registration's timeout
    /// else none. None when the configuration holds no registration (iTerm2 makes no
    /// component, `RPC:172-183`). With it, the component's identifier, the texts it shows (an
    /// HTML component's variants as text, `html`) and its error.
    pub(super) fn script_component(&mut self, pane: PaneId, c: &crate::statusbar::Component, html: bool) -> Option<(String, Vec<String>, Option<String>)> {
        let parsed = self.parsed_component(&c.config)?;
        let id = parsed.id.clone();
        let frame = Values::from([(script_statusbar::KNOBS_PATH.to_string(), Value::String(script_statusbar::knobs_json(&c.knobs)))]);
        let base = self.session_values(pane, parsed.selection).unwrap_or_default();
        let deps = script_eval::deps_key(&parsed.paths, &Layered { frame: &frame, base: &base });
        let (pass, now) = (self.script_pass, Instant::now());
        let fns = &self.api_fns;
        let entry = self.script_components.entry((pane, id.clone())).or_default();
        entry.pass = pass;
        if entry.needs_start(&deps, fns.generation(), |sig| fns.has_signature(sig)) {
            let nth = self.script_next;
            self.script_next += 1;
            entry.deps = deps;
            entry.start(nth);
            let reg = parsed.reg.clone();
            entry.due = script_statusbar::cadence(&reg).map(|d| now + d);
            // `timeout ?: distantFuture` (`RPC:372`): none or 0 waits forever.
            let timeout = reg.timeout.filter(|t| *t != 0.0).map_or(Timeout::Never, |t| Timeout::from_secs(f64::from(t)));
            let origin = Origin::Component { pane, id: id.clone(), nth, reg: Box::new(reg) };
            // Side effects off: a status bar component never runs a built-in that acts.
            self.invoke(&parsed.invocation, ScopeSpec::Session { pane, frame }, timeout, false, origin);
        }
        let entry = self.script_components.get_mut(&(pane, id.clone()))?;
        let texts = if html { entry.texts.get_or_insert_with(|| entry.variants.iter().map(|v| script_statusbar::html_text(v)).collect()).clone() } else { entry.variants.clone() };
        Some((id, texts, entry.error.clone()))
    }

    /// A script component's configuration parsed (`Parsed`), from the cache until the
    /// registrations change: the saved registration decoded once, its identifier the saved
    /// one's (`RPC:241-244`), the registration it runs the newest of that unique id. None when
    /// the configuration holds no registration, or one whose invocation would hold more than
    /// its own parts (`script_statusbar::invocation`): saved profile data never runs anything.
    fn parsed_component(&mut self, config: &toml::Table) -> Option<Rc<Parsed>> {
        let key = config.get(script_statusbar::REQUEST_KEY)?.as_str()?;
        if let Some(p) = self.script_parsed.get(key) {
            return p.clone();
        }
        let parsed = script_statusbar::request_of(config).and_then(|saved| {
            let id = script_statusbar::unique_id(&saved).unwrap_or_default().to_string();
            let reg = self.api_fns.status_bar_component(&id).cloned().unwrap_or(saved);
            let invocation = script_statusbar::invocation(&reg)?;
            let paths = script_statusbar::invocation_paths(&invocation);
            let selection = paths.iter().any(|p| crate::prompted::names_selection(p));
            Some(Rc::new(Parsed { id, reg, invocation, paths, selection }))
        });
        self.script_parsed.insert(key.to_string(), parsed.clone());
        parsed
    }

    /// The registrations changed (a subscribe or unsubscribe, a connection's go): the one
    /// place the status bar's copy of the live component registrations
    /// (`script_statusbar::set_live`) and the parsed configurations follow the registry.
    pub(super) fn script_registry_changed(&mut self) {
        script_statusbar::set_live(self.api_fns.status_bar_components());
        self.script_parsed.clear();
    }

    /// A script component's evaluation finished (`RPC:413-449`): a string or a list of strings
    /// shows, anything else and a failure show the ladybug with iTerm2's text, written to the
    /// log of the connection that registered the function. The bars redraw when it changed.
    #[allow(clippy::too_many_arguments)]
    fn component_finished(&mut self, pane: PaneId, id: &str, nth: u64, reg: &pb::RpcRegistrationRequest, invocation: &str, missing: &BTreeSet<String>, r: Result<Value, Failure>) {
        let (shown, missed) = match r {
            Ok(v) => (script_statusbar::variants(invocation, &v), None),
            Err(f) => (Err(script_statusbar::failed_text(reg, invocation, &f.reason, f.detail.as_deref())), Some(missing)),
        };
        if let Err(text) = &shown {
            let owner = self.api_fns.owner(&script_fn::reg_signature(reg));
            self.script_log(owner, text.clone());
        }
        let Some(entry) = self.script_components.get_mut(&(pane, id.to_string())) else { return };
        if entry.finish(nth, shown, missed) {
            self.redraw();
            self.redraw_all();
        }
    }

    /// A script component's click (`RPC:533-572`): with an error, iTerm2's alert (OK; Launch
    /// Script and Reveal wait for 11d); else its script's click handler, in the session's
    /// scope, with a 30 s timeout, side effects off. A unique id that is not one runs nothing.
    pub(super) fn script_component_click(&mut self, pane: PaneId, id: &str) {
        let Some(invocation) = script_statusbar::click_invocation(id, &pane.to_string()) else { return };
        let scope = ScopeSpec::Session { pane, frame: Values::new() };
        // Side effects off, as the component's evaluation.
        self.invoke(&invocation, scope, Timeout::from_secs(CLICK_TIMEOUT_SECS), false, Origin::Click);
    }

    /// A pick of a script context menu provider's item (`CMH:1377-1398`): the registration
    /// with that unique id now (none, or the session closed while the menu was open: nothing
    /// happens), its invocation from its defaults, in the session's scope, with the
    /// registration's timeout else 30 s, side effects on; a failure reported as the menu item's.
    pub(super) fn context_provider_picked(&mut self, pane: PaneId, unique_id: &str, title: &str) {
        if !self.panes.contains_key(&pane) {
            return;
        }
        let items = self.api_fns.context_menu_items();
        let Some(reg) = items.iter().find(|r| script_fn::context_attrs(r).is_some_and(|(_, id)| id == unique_id)) else { return };
        let Some(invocation) = script_fn::reg_invocation(reg) else { return };
        let timeout = Timeout::from_secs(script_fn::context_menu_timeout_secs(reg));
        let scope = ScopeSpec::Session { pane, frame: Values::new() };
        self.invoke(&invocation, scope, timeout, true, Origin::Report(script_eval::menu_origin(title)));
    }

    /// A failed script component's click: its error text under "Status Bar Script Error",
    /// through the one alert queue (never silenced; the same one merged).
    pub(super) fn script_component_error(&mut self, text: &str) {
        let showing = self.script_alert.as_ref().map(|(_, a)| a);
        queue_alert(&mut self.script_alerts, showing, Alert::Component(text.to_string()), false);
    }

    /// A status bar component registration (`didRegisterStatusBarComponent:`, `RPC:456-462`):
    /// that unique id's components forget what they missed and evaluate again.
    pub(super) fn script_component_registered(&mut self, id: &str) {
        for ((_, i), c) in self.script_components.iter_mut() {
            if i == id {
                c.missing.clear();
                c.deps.clear();
            }
        }
        self.redraw();
        self.redraw_all();
    }

    /// The cadences (`RPC:475-481`, `CV:57`): a component whose beat came evaluates again on
    /// the next frame that draws it (a beat while it still runs waits for it to finish,
    /// `Component::beat`). When the next beat comes.
    pub(super) fn tick_script_components(&mut self, now: Instant) -> Option<Instant> {
        let mut fired = false;
        for c in self.script_components.values_mut() {
            if c.due.is_some_and(|d| d <= now) {
                fired |= c.beat();
            }
        }
        if fired {
            self.redraw();
            self.redraw_all();
        }
        self.script_components.values().filter_map(|c| c.due).min()
    }

    /// The registered title providers as the settings window's Title Function popup lists
    /// them: (display name, unique id).
    pub(super) fn title_provider_items(&self) -> Vec<(String, String)> {
        provider_items(&self.api_fns.title_providers())
    }

    /// Once per loop pass: each session whose Title Components is exactly Custom with a
    /// Title Function evaluates its provider's invocation when `TitleState::needs_start` says
    /// so, in its scope, side effects off, with a 30 s timeout (`SNC:98-170`); the others'
    /// entries go. A provider gone keeps its last title until the next evaluation, which
    /// finds none and shows the ellipsis (`SNC:120-124`). The settings window's popup follows
    /// the providers.
    pub(super) fn tick_script_titles(&mut self) {
        // The providers are listed again only when the registrations changed.
        let revision = self.api_fns.revision();
        let providers = match &self.script_title_providers {
            Some((r, list)) if *r == revision => list.clone(),
            _ => {
                let list = Rc::new(self.api_fns.title_providers());
                self.script_title_providers = Some((revision, list.clone()));
                if let Some(s) = self.settings.as_mut() {
                    let items = provider_items(&list);
                    if s.title_providers != items {
                        s.title_providers = items;
                    }
                }
                list
            }
        };
        let generation = self.api_fns.title_generation();
        let panes: Vec<PaneId> = self.panes.keys().copied().collect();
        for pane in panes {
            let Some(Some(id)) = self.title_provider_id(pane) else {
                self.script_titles.remove(&pane);
                continue;
            };
            let provider = providers.iter().find(|p| p.unique_id == id).map(|p| p.invocation.as_str());
            let Some((invocation, registered)) = title_invocation(provider, self.script_titles.get(&pane), &id) else { continue };
            let paths = script_statusbar::invocation_paths(&invocation);
            let selection = paths.iter().any(|p| crate::prompted::names_selection(p));
            let values = self.session_values(pane, selection).unwrap_or_default();
            let deps = format!("{invocation}\n{}", script_eval::deps_key(&paths, &values));
            let entry = self.script_titles.entry(pane).or_default();
            let Some(start) = entry.needs_start(&id, &deps, generation) else { continue };
            entry.invocation = invocation.clone();
            if !registered {
                let nth = self.script_next;
                self.script_next += 1;
                entry.start(nth, true);
                if entry.finish(nth, script_eval::TITLE_UNREGISTERED.to_string()) {
                    self.redraw();
                    self.redraw_all();
                }
                continue;
            }
            if start == TitleStart::Fresh {
                self.start_title(pane, &invocation, Timeout::Sync, true);
            }
            self.start_title(pane, &invocation, Timeout::from_secs(TITLE_TIMEOUT_SECS), false);
        }
    }

    /// Starts one evaluation of a session's title provider, recorded as its entry's running
    /// one.
    fn start_title(&mut self, pane: PaneId, invocation: &str, timeout: Timeout, fresh: bool) {
        let nth = self.script_next;
        self.script_next += 1;
        if let Some(t) = self.script_titles.get_mut(&pane) {
            t.start(nth, fresh);
        }
        self.invoke(invocation, ScopeSpec::Session { pane, frame: Values::new() }, timeout, false, Origin::Title { pane, nth });
    }

    /// A title provider's evaluation finished (`SNC:131-146`, `SNC:188-228`): a failure is
    /// logged to the failing connection ("Invoked ... to compute name for session") and to
    /// the connection that registered the invocation's signature (the unregistered
    /// explanation, else the reason); the entry takes the title (unless a newer evaluation's
    /// came first) and the windows redraw when it changed. An entry gone takes nothing.
    fn title_finished(&mut self, pane: PaneId, nth: u64, invocation: &str, r: Result<Value, Failure>) {
        if let Err(f) = &r {
            self.script_log(f.conn, script_eval::title_failed_text(invocation, &f.reason, f.detail.as_deref()));
            let owner = expr::parse_calls(invocation).ok().and_then(|c| c.first().map(|c| self.api_fns.owner(&c.signature()))).flatten();
            let text = if f.kind == FailKind::Unregistered { script_eval::title_unregistered_text(invocation) } else { f.reason.clone() };
            self.script_log(owner, text);
        }
        let value = script_eval::provider_title(&r);
        let Some(t) = self.script_titles.get_mut(&pane) else { return };
        if t.finish(nth, value) {
            self.redraw();
            self.redraw_all();
        }
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
        let alert = Alert::Failed { heading: script_eval::report_heading(origin), invocation: invocation.to_string(), text: script_eval::report_text(invocation, &f.reason) };
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
        Run { inv: Invocation::new(text, timeout, true, true, now).unwrap(), scope: ScopeSpec::App, base: Values::new(), origin: Origin::Click, receiver: None }
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

    /// A script component's entry (`RPC:200-222`, `RPC:413-473`): it starts when its
    /// dependencies change (emptied for a cadence beat or a new registration), once per
    /// registration change when a missed function is registered; a result replaces what shows
    /// unless a newer one already finished; a failure shows the ladybug with its text and
    /// keeps what it missed.
    #[test]
    fn script_component_cache_follows_iterm2() {
        let none = |_: &str| false;
        let mut c = Component::default();
        assert!(c.needs_start("d", 0, none), "never started");
        c.deps = "d".into();
        c.running = Some(3);
        assert!(!c.needs_start("d", 0, none));
        assert!(c.needs_start("e", 0, none), "a variable or a knob changed");
        assert!(c.finish(3, Ok(vec!["12:00".into()]), None));
        assert_eq!((c.variants.clone(), c.error.clone(), c.running), (vec!["12:00".to_string()], None, None));
        assert!(!c.finish(2, Ok(vec!["old".into()]), None), "an older one after the newest finished");
        // A failure: the ladybug, its text, the functions it missed.
        c.running = Some(5);
        let missed = BTreeSet::from(["f()".to_string()]);
        assert!(c.finish(5, Err("boom".into()), Some(&missed)));
        assert_eq!((c.variants.clone(), c.error.clone()), (vec![super::super::statusbar_content::LADYBUG.to_string()], Some("boom".to_string())));
        assert!(!c.needs_start("d", 0, |s| s == "f()"), "no registration happened");
        assert!(c.needs_start("d", 1, |s| s == "f()"), "the missed function is registered");
        assert!(c.missing.is_empty());
        // An older evaluation finishing while a newer runs shows until the newer's comes.
        c.running = Some(9);
        assert!(c.finish(8, Ok(vec!["a".into(), "b".into()]), None));
        assert_eq!(c.running, Some(9));
        assert!(!c.finish(9, Ok(vec!["a".into(), "b".into()]), None), "the same variants");
        assert_eq!(c.running, None);
        // Review finding 2 (Task 6's applied rule): start 20 and 21, 21 finishes, start 22;
        // 20 finishing then is older than what shows.
        c.start(20);
        c.start(21);
        assert!(c.finish(21, Ok(vec!["21".into()]), None));
        c.start(22);
        assert!(!c.finish(20, Ok(vec!["20".into()]), None));
        assert_eq!(c.variants, ["21"]);
        assert!(c.finish(22, Ok(vec!["22".into()]), None));
        // An entry made again after a sweep: its first start is the floor.
        let mut fresh = Component::default();
        fresh.start(40);
        assert!(!fresh.finish(39, Ok(vec!["late".into()]), None), "a result for the swept entry");
        // Review finding 1: a beat while an evaluation runs waits for it; when it finishes the
        // dependencies empty, so the next frame starts one (which sets the next beat).
        c.start(30);
        c.deps = "x".into();
        c.due = Some(Instant::now());
        assert!(!c.beat(), "nothing to redraw while it runs");
        assert_eq!((c.due, c.beat, c.deps.as_str()), (None, true, "x"));
        assert!(!c.needs_start("x", 1, none));
        assert!(c.finish(30, Ok(vec!["22".into()]), None), "the same variants, but a beat is due");
        assert!(!c.beat && c.deps.is_empty());
        assert!(c.needs_start("x", 1, none));
        // A beat with nothing running evaluates on the next frame.
        c.deps = "x".into();
        assert!(c.beat());
        assert!(c.needs_start("x", 1, none));
        // An HTML component's texts are made again only for new variants.
        c.texts = Some(vec!["t".into()]);
        c.start(31);
        c.finish(31, Ok(vec!["22".into()]), None);
        assert!(c.texts.is_some());
        c.start(32);
        c.finish(32, Ok(vec!["new".into()]), None);
        assert!(c.texts.is_none());
        assert_eq!(script_statusbar::ERROR_HEADING, Alert::Component("x".into()).form().0);
        assert_eq!(CLICK_TIMEOUT_SECS, 30.0);
    }

    /// A title provider's entry (`SNC:98-170`, `SNC:419-455`): the first evaluation, one for
    /// another unique id and one after a title provider registered are a synchronous pass then
    /// an asynchronous one; a dependency change the asynchronous one alone; an older result
    /// after a newer one was applied is dropped, and results from before a fresh start never
    /// show.
    #[test]
    fn title_cache_follows_iterm2() {
        let mut t = TitleState::default();
        assert_eq!(t.needs_start("com.a", "d", 0), Some(TitleStart::Fresh));
        t.start(1, true);
        assert!(t.finish(1, " ".into()), "the synchronous pass shows a blank");
        t.start(2, false);
        assert_eq!(t.needs_start("com.a", "d", 0), None);
        assert!(t.finish(2, "hi".into()));
        assert_eq!((t.value.as_str(), t.running), ("hi", None));
        assert_eq!(t.needs_start("com.a", "e", 0), Some(TitleStart::Again), "a dependency changed");
        assert_eq!(t.needs_start("com.a", "e", 0), None);
        t.start(3, false);
        assert_eq!(t.needs_start("com.a", "e", 1), Some(TitleStart::Fresh), "a title provider registered");
        assert_eq!((t.value.as_str(), t.running), ("hi", None), "the value stays until an evaluation replaces it");
        assert_eq!(t.needs_start("com.a", "e", 1), None);
        t.start(4, true);
        assert!(!t.finish(3, "stale".into()), "a result from before the registration");
        assert!(t.finish(4, " ".into()));
        // Start 5 and 6; 6 finishes; 5 finishing later is dropped.
        t.start(5, false);
        t.start(6, false);
        assert!(t.finish(6, "six".into()));
        assert!(!t.finish(5, "five".into()));
        // Another provider: a fresh start whose pass is the floor.
        t.start(7, false);
        assert_eq!(t.needs_start("com.b", "e", 1), Some(TitleStart::Fresh));
        assert_eq!(t.unique_id, "com.b");
        t.start(8, true);
        assert!(!t.finish(7, "old provider".into()), "a result from before the fresh start");
        assert!(t.finish(8, " ".into()));
        assert_eq!(TITLE_TIMEOUT_SECS, 30.0);
    }

    /// The tick's start rules (`SNC:98-170`, `SNC:451-455`): a registered provider's
    /// invocation evaluates; with it gone, a session that evaluated it keeps its title and
    /// re-evaluates the last invocation (to the ellipsis) only when its references change;
    /// a session that never evaluated one waits.
    #[test]
    fn title_ticks_follow_iterm2() {
        assert_eq!(title_invocation(Some("title.a.f(x:id)"), None, "com.a"), Some(("title.a.f(x:id)".to_string(), true)));
        assert_eq!(title_invocation(None, None, "com.a"), None);
        let mut t = TitleState::default();
        assert_eq!(title_invocation(None, Some(&t), "com.a"), None, "never started");
        assert_eq!(t.needs_start("com.a", "d", 1), Some(TitleStart::Fresh));
        t.invocation = "title.a.f(x:id)".into();
        t.start(1, true);
        t.finish(1, "hi".into());
        assert_eq!(title_invocation(None, Some(&t), "com.a"), Some(("title.a.f(x:id)".to_string(), false)));
        assert_eq!(title_invocation(None, Some(&t), "com.b"), None, "another provider");
        assert_eq!(title_invocation(Some("title.a.g()"), Some(&t), "com.a"), Some(("title.a.g()".to_string(), true)), "the registered one wins");
        // Gone: nothing starts until a reference changes; the title stays.
        assert_eq!(t.needs_start("com.a", "d", 1), None);
        assert_eq!(t.value, "hi");
        assert_eq!(t.needs_start("com.a", "e", 1), Some(TitleStart::Again));
    }

    /// Review finding 1: an Invoke Script Function trigger whose function is not registered
    /// fails on every match; 1000 matches queue one alert, none while silenced, none while
    /// the same one shows; another failure still queues.
    #[test]
    fn trigger_alerts_are_bounded() {
        let failure = |sig: &str| Alert::Failed { heading: script_eval::report_heading(TRIGGER), invocation: sig.to_string(), text: script_eval::report_text(sig, &script_fn::unregistered_text(sig, None)) };
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
        // Final review: a failure whose text varies per match (the reason echoes the match)
        // merges by origin and invocation, keeping the newest text; different invocations
        // stop at MAX_QUEUED, the oldest going first.
        let varying = |inv: &str, reason: &str| Alert::Failed { heading: script_eval::report_heading(TRIGGER), invocation: inv.to_string(), text: script_eval::report_text(inv, reason) };
        let mut queue = VecDeque::new();
        for i in 0..1000 {
            queue_alert(&mut queue, None, varying("f(x: matches[1])", &format!("bad {i}")), false);
        }
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].form().1, script_eval::report_text("f(x: matches[1])", "bad 999"));
        let mut queue = VecDeque::new();
        for i in 0..1000 {
            queue_alert(&mut queue, None, varying(&format!("f(x: \"{i}\")"), &format!("bad {i}")), false);
        }
        assert_eq!(queue.len(), MAX_QUEUED);
        assert_eq!(queue[0].form().1, script_eval::report_text("f(x: \"984\")", "bad 984"), "the oldest went");
        let showing = varying("g()", "one");
        let mut queue = VecDeque::new();
        queue_alert(&mut queue, Some(&showing), varying("g()", "two"), false);
        assert!(queue.is_empty(), "the same failure as the one showing");
        let mut dialogs: VecDeque<u64> = VecDeque::new();
        let dropped: Vec<u64> = (0..1000).filter_map(|i| push_capped(&mut dialogs, i)).collect();
        assert_eq!((dialogs.len(), dropped.len(), dropped[0], dialogs[0]), (MAX_QUEUED, 1000 - MAX_QUEUED, 0, 984));
        // The silence: ten minutes from the OK (`iTermWarning.m:13`, `:632-636`).
        assert_eq!((script_eval::SILENCE_UNTIL_KEY, script_eval::TEMPORARY_SILENCE_SECS), ("NoSyncFunctionCallError_SilenceUntil", 600.0));
        assert!(script_eval::silenced(Some(100.0), 99.0));
        assert!(!script_eval::silenced(Some(100.0), 100.0));
        assert!(!script_eval::silenced(None, 0.0));
        assert_eq!(crate::prompt_ui::ConfirmWindow::SUPPRESS_TEN_MINUTES, "Suppress this message for ten minutes");
    }

    #[test]
    fn script_alerts_follow_iterm2() {
        let failed = Alert::Failed { heading: script_eval::report_heading("Trigger"), invocation: "f()".into(), text: script_eval::report_text("f()", "boom") };
        assert_eq!(failed.form(), ("Trigger Function Call Failed", "Error running \u{201c}f()\u{201d}:\nboom"));
        assert_eq!(Alert::Result("f() returned:\n3".into()).form(), ("Function Call Result", "f() returned:\n3"));
    }
}

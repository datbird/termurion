//! Evaluating invocations (`SFC:203-450`, `SFC:586-679`, `EV:315-400`): an `Invocation`
//! runs a call, a call sequence or an interpolated string against a `Host` that answers
//! built-ins at once and starts script calls. A started call is `Pending` for the evaluator;
//! when its result is delivered the invocation replays its parsed tree against a memo of the
//! finished calls, so no call is asked twice. It owns iTerm2's timeout texts, the function
//! call error report (`H:566-591`), and InvokeFunctionRequest's targets and answers
//! (`H:4257-4394`).

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use crate::api_proto::pb;
use crate::api_vars::Values;
use crate::expr::{self, Arg, Call, Expr, FailKind, Failure, Outcome, Part, Value};

/// An invocation's timeout (`SFC:407-450`): 0 evaluates synchronously (script calls answer
/// nothing), a positive time starts a timer per call of a sequence, anything else waits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Timeout {
    Sync,
    After(Duration),
    Never,
}

/// The longest timeout kept as one; a longer one never fires.
const YEAR: f64 = 365.0 * 24.0 * 3600.0;

impl Timeout {
    /// A timeout in seconds: 0 synchronous, a finite positive time up to a year a timer, any
    /// other (infinite, longer, negative or NaN: `timeout > 0` fails and it is not 0) none.
    pub fn from_secs(s: f64) -> Timeout {
        if s == 0.0 {
            Timeout::Sync
        } else if s > 0.0 && s <= YEAR {
            Timeout::After(Duration::from_secs_f64(s))
        } else {
            Timeout::Never
        }
    }
}

/// What a host answers for one call.
#[derive(Clone, Debug, PartialEq)]
pub enum HostOutcome {
    Done(Value),
    Failed(Failure),
    /// The call runs; its result is delivered later under this token.
    Started(u64),
}

/// Runs the calls of an invocation: built-ins, then scripts.
pub trait Host {
    /// A call whose arguments are known. `sync` is a synchronous evaluation: a script call
    /// answers `Done(Null)` without running.
    fn call(&mut self, call: &Call, args: &BTreeMap<String, Value>, sync: bool, side_effects: bool) -> HostOutcome;
}

/// Where an invocation stands after a step.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Done(Value),
    Failed(Failure),
    Waiting,
}

#[derive(Clone, Debug)]
enum Body {
    /// A call sequence, each call an `Expr::Call`.
    Calls(Vec<Expr>),
    /// An interpolated string.
    Template(Vec<Part>),
}

/// What a waiting evaluation waits on, for the timeout text: the arguments of its outermost
/// call still evaluating, or that call itself.
#[derive(Clone, Debug)]
enum Progress {
    Args { fq: String, names: Vec<String> },
    Return { name: String, site: usize },
}

/// A memo key: a call's site and the JSON of its arguments.
type Key = (usize, String);

/// One invocation being evaluated.
#[derive(Clone, Debug)]
pub struct Invocation {
    text: String,
    calls: Body,
    lax: bool,
    timeout: Timeout,
    side_effects: bool,
    /// When the current call of a sequence (or the whole string) started: its timer's start.
    started: Instant,
    /// The next call of a sequence to finish, and the last finished call's value.
    next: usize,
    last: Value,
    /// Finished calls, and those of them that finished only after waiting.
    memo: BTreeMap<Key, Outcome>,
    waited: BTreeSet<Key>,
    /// Started calls by token, with their signatures by site.
    waiting: BTreeMap<u64, Key>,
    signatures: BTreeMap<usize, String>,
    /// Signatures of calls that found no registered function.
    missing: BTreeSet<String>,
    progress: Option<Progress>,
    finished: Option<Step>,
}

impl Invocation {
    /// A call or call sequence (iTerm2's call parser); a syntax error is the failure.
    /// Interpolated strings inside the arguments use iTerm2's default lax policy.
    pub fn new(text: &str, timeout: Timeout, side_effects: bool, now: Instant) -> Result<Invocation, Failure> {
        let calls = expr::parse_calls(text).map_err(Failure::plain)?;
        Ok(Invocation::with(text, Body::Calls(calls.into_iter().map(Expr::Call).collect()), true, timeout, side_effects, now))
    }

    /// An interpolated string whose result is its text; `lax` is
    /// `laxNilPolicyInInterpolatedStrings`.
    pub fn for_template(template: &str, timeout: Timeout, side_effects: bool, lax: bool, now: Instant) -> Result<Invocation, Failure> {
        let parts = expr::parse_template(template).map_err(Failure::plain)?;
        Ok(Invocation::with(template, Body::Template(parts), lax, timeout, side_effects, now))
    }

    fn with(text: &str, calls: Body, lax: bool, timeout: Timeout, side_effects: bool, now: Instant) -> Invocation {
        Invocation {
            text: text.to_string(),
            calls,
            lax,
            timeout,
            side_effects,
            started: now,
            next: 0,
            last: Value::Null,
            memo: BTreeMap::new(),
            waited: BTreeSet::new(),
            waiting: BTreeMap::new(),
            signatures: BTreeMap::new(),
            missing: BTreeSet::new(),
            progress: None,
            finished: None,
        }
    }

    /// Evaluates as far as the finished calls allow. A sequence runs its calls in order and
    /// stops at the first failure (`SFC:278-350`); each call's timer starts when the call
    /// does (`SFC:407-424`).
    pub fn step(&mut self, scope: &dyn expr::Scope, host: &mut dyn Host) -> Step {
        if let Some(done) = &self.finished {
            return done.clone();
        }
        let sync = self.timeout == Timeout::Sync;
        let cx = expr::Ctx { invocation: &self.text, lax: self.lax };
        let mut replay = Replay {
            memo: &mut self.memo,
            waited: &mut self.waited,
            waiting: &mut self.waiting,
            signatures: &mut self.signatures,
            missing: &mut self.missing,
            host,
            sync,
            side_effects: self.side_effects,
            last_waited: false,
            pending: BTreeSet::new(),
        };
        let step = match &self.calls {
            Body::Template(parts) => match expr::eval_template(parts, scope, &mut replay, &cx) {
                Outcome::Done(v) => Step::Done(v),
                Outcome::Failed(f) => Step::Failed(f),
                Outcome::Pending => {
                    self.progress = parts.iter().find_map(|p| match p {
                        Part::Expr(x) => outermost(x, &replay.pending),
                        Part::Text(_) => None,
                    }).map(|c| progress_of(c, &replay.pending));
                    Step::Waiting
                }
            },
            Body::Calls(calls) => loop {
                let Some(call) = calls.get(self.next) else { break Step::Done(self.last.clone()) };
                match expr::eval(call, scope, &mut replay, &cx) {
                    Outcome::Done(v) => {
                        self.last = v;
                        self.next += 1;
                        if self.next < calls.len() {
                            self.started = Instant::now();
                        }
                    }
                    Outcome::Failed(f) => break Step::Failed(f),
                    Outcome::Pending => {
                        self.progress = outermost(call, &replay.pending).map(|c| progress_of(c, &replay.pending));
                        break Step::Waiting;
                    }
                }
            },
        };
        if step != Step::Waiting {
            self.progress = None;
            self.finished = Some(step.clone());
        }
        step
    }

    /// A started call's result; false for a token this invocation does not wait on.
    pub fn deliver(&mut self, token: u64, r: Result<Value, Failure>) -> bool {
        if self.finished.is_some() {
            return false;
        }
        let Some(key) = self.waiting.remove(&token) else { return false };
        let outcome = match r {
            Ok(v) => Outcome::Done(v),
            Err(f) => {
                if f.kind == FailKind::Unregistered {
                    if let Some(sig) = self.signatures.get(&key.0) {
                        self.missing.insert(sig.clone());
                    }
                }
                Outcome::Failed(f)
            }
        };
        self.waited.insert(key.clone());
        self.memo.insert(key, outcome);
        true
    }

    /// When the current call's timer fires; none for a synchronous or untimed invocation, or
    /// a finished one.
    pub fn deadline(&self) -> Option<Instant> {
        match (self.timeout, &self.finished) {
            (Timeout::After(d), None) => Some(self.started + d),
            _ => None,
        }
    }

    /// The timer's failure (`SFC:388-405`), kind Timeout: while the outermost waiting call's
    /// arguments evaluate, their names; else waiting for it to return.
    pub fn timed_out(&self) -> Failure {
        let secs = match self.timeout {
            Timeout::After(d) => d.as_secs_f64(),
            _ => 0.0,
        };
        let reason = match &self.progress {
            Some(Progress::Args { fq, names }) => timeout_args_text(secs, &self.text, fq, names),
            Some(Progress::Return { name, .. }) => timeout_return_text(secs, &self.text, name),
            None => timeout_return_text(secs, &self.text, &self.current_name()),
        };
        Failure { reason, detail: None, kind: FailKind::Timeout, conn: None }
    }

    /// The name of the sequence's current call (the string's first), for a timeout text with
    /// nothing waiting.
    fn current_name(&self) -> String {
        let e = match &self.calls {
            Body::Calls(calls) => calls.get(self.next),
            Body::Template(parts) => parts.iter().find_map(|p| match p {
                Part::Expr(x) => Some(x),
                Part::Text(_) => None,
            }),
        };
        match e {
            Some(Expr::Call(c)) => c.name.clone(),
            _ => String::new(),
        }
    }

    /// The token of the outermost call when the invocation waits for it to return (its
    /// connection is the one a timeout is logged to).
    pub fn returning(&self) -> Option<u64> {
        let Some(Progress::Return { site, .. }) = &self.progress else { return None };
        self.waiting.iter().find(|(_, (s, _))| s == site).map(|(t, _)| *t)
    }

    /// Signatures of calls that found no registered function (`SFC:552-556`).
    pub fn missing(&self) -> &BTreeSet<String> {
        &self.missing
    }

    /// The tokens of the started calls not yet delivered.
    pub fn tokens(&self) -> Vec<u64> {
        self.waiting.keys().copied().collect()
    }

    pub fn text(&self) -> &str {
        &self.text
    }
}

/// The evaluator's calls: a finished call from the memo, a started one pending, any other
/// asked of the host once.
struct Replay<'a> {
    memo: &'a mut BTreeMap<Key, Outcome>,
    waited: &'a mut BTreeSet<Key>,
    waiting: &'a mut BTreeMap<u64, Key>,
    signatures: &'a mut BTreeMap<usize, String>,
    missing: &'a mut BTreeSet<String>,
    host: &'a mut dyn Host,
    sync: bool,
    side_effects: bool,
    /// The call just answered from the memo finished after waiting.
    last_waited: bool,
    /// The sites found running in this pass.
    pending: BTreeSet<usize>,
}

impl expr::Calls for Replay<'_> {
    fn call(&mut self, call: &Call, args: &BTreeMap<String, Value>) -> Outcome {
        let key: Key = (call.site, serde_json::to_string(args).unwrap_or_default());
        self.last_waited = false;
        if let Some(o) = self.memo.get(&key) {
            self.last_waited = self.waited.contains(&key);
            return o.clone();
        }
        if self.waiting.values().any(|k| *k == key) {
            self.pending.insert(call.site);
            return Outcome::Pending;
        }
        let outcome = match self.host.call(call, args, self.sync, self.side_effects) {
            HostOutcome::Done(v) => Outcome::Done(v),
            HostOutcome::Failed(f) => {
                if f.kind == FailKind::Unregistered {
                    self.missing.insert(call.signature());
                }
                Outcome::Failed(f)
            }
            HostOutcome::Started(token) => {
                self.waiting.insert(token, key);
                self.signatures.insert(call.site, call.signature());
                self.pending.insert(call.site);
                return Outcome::Pending;
            }
        };
        self.memo.insert(key, outcome.clone());
        outcome
    }

    fn waited(&self, _call: &Call) -> bool {
        self.last_waited
    }
}

/// True when a call at one of `sites` is inside `e`.
fn has_site(e: &Expr, sites: &BTreeSet<usize>) -> bool {
    match e {
        Expr::Num(_) | Expr::Bool(_) | Expr::Null | Expr::Path { .. } => false,
        Expr::Call(c) => sites.contains(&c.site) || c.args.iter().any(|(_, a)| matches!(a, Arg::Value(x) if has_site(x, sites))),
        Expr::Str(parts) => parts.iter().any(|p| matches!(p, Part::Expr(x) if has_site(x, sites))),
        Expr::Index { index, .. } => has_site(index, sites),
        Expr::Array(items) => items.iter().any(|x| has_site(x, sites)),
        Expr::Not(x) | Expr::Neg(x) | Expr::Opt(x) => has_site(x, sites),
        Expr::Bin(l, _, r) => has_site(l, sites) || has_site(r, sites),
        Expr::Cond(c, t, f) => has_site(c, sites) || has_site(t, sites) || has_site(f, sites),
    }
}

/// The first outermost call in `e` that waits on one of `sites`.
fn outermost<'e>(e: &'e Expr, sites: &BTreeSet<usize>) -> Option<&'e Call> {
    if !has_site(e, sites) {
        return None;
    }
    match e {
        Expr::Call(c) => Some(c),
        Expr::Str(parts) => parts.iter().find_map(|p| match p {
            Part::Expr(x) => outermost(x, sites),
            Part::Text(_) => None,
        }),
        Expr::Index { index, .. } => outermost(index, sites),
        Expr::Array(items) => items.iter().find_map(|x| outermost(x, sites)),
        Expr::Not(x) | Expr::Neg(x) | Expr::Opt(x) => outermost(x, sites),
        Expr::Bin(l, _, r) => outermost(l, sites).or_else(|| outermost(r, sites)),
        Expr::Cond(c, t, f) => outermost(c, sites).or_else(|| outermost(t, sites)).or_else(|| outermost(f, sites)),
        Expr::Num(_) | Expr::Bool(_) | Expr::Null | Expr::Path { .. } => None,
    }
}

/// What a waiting call waits on: itself when it runs, else its arguments still evaluating
/// (iTerm2's `_remainingArgs`, sorted here since iTerm2's set has no order).
fn progress_of(c: &Call, sites: &BTreeSet<usize>) -> Progress {
    if sites.contains(&c.site) {
        return Progress::Return { name: c.name.clone(), site: c.site };
    }
    let mut names: Vec<String> = c.args.iter().filter(|(_, a)| matches!(a, Arg::Value(x) if has_site(x, sites))).map(|(n, _)| n.clone()).collect();
    names.sort();
    Progress::Args { fq: c.fq_name(), names }
}

/// A map of values as a scope (a path is a key).
impl expr::Scope for Values {
    fn value(&self, path: &str) -> Option<Value> {
        self.get(path).cloned()
    }
}

/// A scope that records the paths an evaluation read, for its dependencies.
pub struct Recorder<'a> {
    pub inner: &'a dyn expr::Scope,
    pub read: RefCell<BTreeSet<String>>,
}

impl expr::Scope for Recorder<'_> {
    fn value(&self, path: &str) -> Option<Value> {
        self.read.borrow_mut().insert(path.to_string());
        self.inner.value(path)
    }

    fn writable(&self, path: &str) -> bool {
        self.inner.writable(path)
    }
}

/// The values of `paths` now, as one text: it changes when any of them does.
pub fn deps_key(paths: &BTreeSet<String>, scope: &dyn expr::Scope) -> String {
    let now: Vec<(&str, Option<Value>)> = paths.iter().map(|p| (p.as_str(), scope.value(p))).collect();
    serde_json::to_string(&now).unwrap_or_default()
}

/// A frame of temporary values (a trigger's, a component's `__knobs`) over a scope.
pub struct Layered<'a> {
    pub frame: &'a Values,
    pub base: &'a dyn expr::Scope,
}

impl expr::Scope for Layered<'_> {
    fn value(&self, path: &str) -> Option<Value> {
        self.frame.get(path).cloned().or_else(|| self.base.value(path))
    }

    fn writable(&self, path: &str) -> bool {
        self.base.writable(path)
    }
}

/// Seconds as NSNumber prints them (`30`, `2.5`).
fn secs_text(secs: f64) -> String {
    expr::to_text(&serde_json::json!(secs))
}

/// The timer fired while arguments evaluated (`SFC:390-392`): the fully qualified name.
pub fn timeout_args_text(secs: f64, invocation: &str, fq: &str, args: &[String]) -> String {
    format!(
        "Timeout ({} sec) while evaluating invocation \u{201c}{invocation}\u{201d}. The timeout occurred while evaluating the following arguments to {fq}: {}",
        secs_text(secs),
        args.join(", ")
    )
}

/// The timer fired while the call ran (`SFC:393-395`): the relative name.
pub fn timeout_return_text(secs: f64, invocation: &str, name: &str) -> String {
    format!("Timeout ({} sec) while evaluating invocation \u{201c}{invocation}\u{201d}. The timeout occurred while waiting for {name} to return.", secs_text(secs))
}

/// A method call's receiver names no object (`SFC:365-381`).
pub fn object_not_found(id: &str) -> String {
    format!("Object with identifier \u{201c}{id}\u{201d} not found")
}

/// The function call error alert's heading (`H:583`).
pub fn report_heading(origin: &str) -> String {
    format!("{origin} Function Call Failed")
}

/// The function call error alert's text (`H:567`).
pub fn report_text(invocation: &str, reason: &str) -> String {
    format!("Error running \u{201c}{invocation}\u{201d}:\n{reason}")
}

/// The failure as the connection's console entry gets it (`H:574-577`).
pub fn console_text(invocation: &str, reason: &str, traceback: &str) -> String {
    format!("An error occurred while running the function invocation \u{201c}{invocation}\u{201d}:\n{reason}\n\nTraceback:\n{traceback}")
}

/// The function call error alert's identifier (`H:580`).
pub const ERROR_ID: &str = "NoSyncFunctionCallError";
/// The error alert's second button when there is a traceback (`H:570`); 11d.
pub const REVEAL_IN_CONSOLE: &str = "Reveal in Script Console";
/// The heading of an invocation's result shown by Open Quickly (`OQW:494`).
pub const RESULT_HEADING: &str = "Function Call Result";

/// What an InvokeFunctionRequest runs in: the app, a tab by id, a window by Registry key, a
/// session by pane, or a method's receiver id.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    App,
    Tab(u64),
    Window(u64),
    Session(u64),
    Method(String),
}

fn invoke_error(status: pb::invoke_function_response::Status, reason: Option<&str>) -> pb::InvokeFunctionResponse {
    pb::InvokeFunctionResponse {
        disposition: Some(pb::invoke_function_response::Disposition::Error(pb::invoke_function_response::Error {
            status: Some(status as i32),
            error_reason: reason.map(str::to_string),
        })),
    }
}

/// InvokeFunctionRequest's context and timeout (`H:4257-4374`): a tab, window or session
/// that ids name no object INVALID_ID; a method with no receiver or no context
/// REQUEST_MALFORMED; the timeout `>= 0` as given, else 30 (the proto default is -1).
pub fn invoke_target(r: &pb::InvokeFunctionRequest, s: &crate::api_session::Snapshot) -> Result<(Target, Timeout), pb::InvokeFunctionResponse> {
    use pb::invoke_function_request::Context;
    use pb::invoke_function_response::Status;
    let invalid = |reason: &str| invoke_error(Status::InvalidId, Some(reason));
    let target = match &r.context {
        Some(Context::App(_)) => Target::App,
        Some(Context::Tab(t)) => Target::Tab(s.tab_by_id(t.tab_id.as_deref().unwrap_or_default()).ok_or_else(|| invalid("No such tab"))?),
        Some(Context::Window(w)) => Target::Window(s.window_key(w.window_id.as_deref().unwrap_or_default()).ok_or_else(|| invalid("No such window"))?),
        Some(Context::Session(x)) => Target::Session(s.pane(x.session_id.as_deref().unwrap_or_default()).ok_or_else(|| invalid("No such session"))?),
        Some(Context::Method(m)) => match &m.receiver {
            Some(receiver) => Target::Method(receiver.clone()),
            None => return Err(invoke_error(Status::RequestMalformed, Some("No receiver"))),
        },
        None => return Err(invoke_error(Status::RequestMalformed, Some("Invalid context"))),
    };
    let t = r.timeout.unwrap_or(-1.0);
    Ok((target, Timeout::from_secs(if t >= 0.0 { t } else { 30.0 })))
}

/// InvokeFunctionResponse for a finished invocation (`H:4376-4394`): the invocation's timer
/// TIMEOUT with no reason, any other failure FAILED with it, a value its JSON.
pub fn invoke_response(r: &Result<Value, Failure>) -> pb::InvokeFunctionResponse {
    use pb::invoke_function_response::{Disposition, Status, Success};
    match r {
        Ok(v) => pb::InvokeFunctionResponse { disposition: Some(Disposition::Success(Success { json_result: Some(expr::json_text(v)) })) },
        Err(f) if f.kind == FailKind::Timeout => invoke_error(Status::Timeout, None),
        Err(f) => invoke_error(Status::Failed, Some(&f.reason)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Records every call; answers built-ins named `b` at once (their `x`), fails the names
    /// in `unregistered` as unregistered, and starts scripts with tokens 1, 2, 3 (a
    /// synchronous call answers them null).
    #[derive(Default)]
    struct Fake {
        asked: Vec<(String, BTreeMap<String, Value>, bool)>,
        unregistered: BTreeSet<String>,
        next: u64,
    }

    impl Host for Fake {
        fn call(&mut self, call: &Call, args: &BTreeMap<String, Value>, sync: bool, _side_effects: bool) -> HostOutcome {
            self.asked.push((call.name.clone(), args.clone(), sync));
            if call.name == "b" {
                return HostOutcome::Done(args.get("x").cloned().unwrap_or(Value::Null));
            }
            if self.unregistered.contains(&call.name) {
                return HostOutcome::Failed(Failure { reason: format!("no {}", call.name), detail: None, kind: FailKind::Unregistered, conn: None });
            }
            if sync {
                return HostOutcome::Done(Value::Null);
            }
            self.next += 1;
            HostOutcome::Started(self.next)
        }
    }

    impl Fake {
        fn names(&self) -> Vec<&str> {
            self.asked.iter().map(|a| a.0.as_str()).collect()
        }
    }

    fn scope() -> Values {
        Values::from([("columns".to_string(), json!(80)), ("user.x".to_string(), json!("hi"))])
    }

    fn inv(text: &str, timeout: Timeout) -> Invocation {
        Invocation::new(text, timeout, true, Instant::now()).unwrap_or_else(|f| panic!("{text}: {f:?}"))
    }

    const FOREVER: Timeout = Timeout::Never;

    #[test]
    fn invocations_follow_iterm2() {
        let s = scope();

        // One script call: started once, then its result.
        let mut host = Fake::default();
        let mut i = inv("f(a: 1)", FOREVER);
        assert_eq!(i.step(&s, &mut host), Step::Waiting);
        assert_eq!(host.asked, vec![("f".to_string(), BTreeMap::from([("a".to_string(), json!(1))]), false)]);
        assert_eq!(i.tokens(), [1]);
        assert_eq!(i.step(&s, &mut host), Step::Waiting);
        assert_eq!(host.asked.len(), 1, "a started call is not asked again");
        assert!(!i.deliver(9, Ok(json!(0))), "an unknown token");
        assert!(i.deliver(1, Ok(json!(3))));
        assert_eq!(i.step(&s, &mut host), Step::Done(json!(3)));
        assert_eq!(host.asked.len(), 1);
        assert!(i.tokens().is_empty());

        // A nested call finishes before its caller, which gets its value.
        let mut host = Fake::default();
        let mut i = inv("f(a: g(b: 2))", FOREVER);
        assert_eq!(i.step(&s, &mut host), Step::Waiting);
        assert_eq!(host.names(), ["g"]);
        assert!(i.deliver(1, Ok(json!("gv"))));
        assert_eq!(i.step(&s, &mut host), Step::Waiting);
        assert_eq!(host.names(), ["g", "f"]);
        assert_eq!(host.asked[1].1, BTreeMap::from([("a".to_string(), json!("gv"))]));
        assert!(i.deliver(2, Ok(json!(5))));
        for _ in 0..3 {
            assert_eq!(i.step(&s, &mut host), Step::Done(json!(5)));
        }
        assert_eq!(host.names(), ["g", "f"], "each site once however many steps");

        // A sequence runs in order; its result is the last call's.
        let mut host = Fake::default();
        let mut i = inv("f(); g()", FOREVER);
        assert_eq!(i.step(&s, &mut host), Step::Waiting);
        assert_eq!(host.names(), ["f"]);
        assert!(i.deliver(1, Ok(json!("fv"))));
        assert_eq!(i.step(&s, &mut host), Step::Waiting);
        assert_eq!(host.names(), ["f", "g"]);
        assert!(i.deliver(2, Ok(json!("gv"))));
        assert_eq!(i.step(&s, &mut host), Step::Done(json!("gv")));

        // A failed dependency: "In call to", its traceback and connection kept.
        let mut host = Fake::default();
        let mut i = inv("f(a: g())", FOREVER);
        assert_eq!(i.step(&s, &mut host), Step::Waiting);
        assert!(i.deliver(1, Err(Failure { reason: "boom".into(), detail: Some("tb".into()), kind: FailKind::Plain, conn: Some(7) })));
        assert_eq!(i.step(&s, &mut host), Step::Failed(Failure { reason: "In call to g: boom".into(), detail: Some("tb".into()), kind: FailKind::Plain, conn: Some(7) }));
        assert_eq!(host.names(), ["g"], "the caller never runs");

        // An unregistered dependency passes through unchanged and is missing.
        let unregistered = Failure { reason: "no g".into(), detail: None, kind: FailKind::Unregistered, conn: None };
        let mut host = Fake::default();
        let mut i = inv("f(a: g())", FOREVER);
        assert_eq!(i.step(&s, &mut host), Step::Waiting);
        assert!(i.deliver(1, Err(unregistered.clone())));
        assert_eq!(i.step(&s, &mut host), Step::Failed(unregistered.clone()));
        assert_eq!(i.missing(), &BTreeSet::from(["g()".to_string()]));
        let mut host = Fake { unregistered: BTreeSet::from(["g".to_string()]), ..Fake::default() };
        let mut i = inv("f(a: g())", FOREVER);
        assert_eq!(i.step(&s, &mut host), Step::Failed(unregistered));
        assert_eq!(i.missing(), &BTreeSet::from(["g()".to_string()]));

        // A built-in finishes in one step, with no token, and is never asked twice.
        let mut host = Fake::default();
        let mut i = inv("b(x: 1)", FOREVER);
        assert_eq!(i.step(&s, &mut host), Step::Done(json!(1)));
        assert!(i.tokens().is_empty());
        let mut host = Fake::default();
        let mut i = inv("f(a: b(x: columns), c: g())", FOREVER);
        for _ in 0..4 {
            assert_eq!(i.step(&s, &mut host), Step::Waiting);
        }
        assert_eq!(host.names(), ["b", "g"]);
        assert!(i.deliver(1, Ok(json!(2))));
        assert_eq!(i.step(&s, &mut host), Step::Waiting);
        assert!(i.deliver(2, Ok(json!("done"))));
        assert_eq!(i.step(&s, &mut host), Step::Done(json!("done")));
        assert_eq!(host.names(), ["b", "g", "f"], "the side-effect built-in ran once");
        assert_eq!(host.asked[2].1, BTreeMap::from([("a".to_string(), json!(80)), ("c".to_string(), json!(2))]));

        // An operator error keeps "In <invocation>: " only when no operand waited.
        let mut host = Fake::default();
        let text = "f(a: b(x: user.x) * 2)";
        let mut i = inv(text, FOREVER);
        assert_eq!(i.step(&s, &mut host), Step::Failed(Failure::plain(format!("In {text}: Arithmetic operations require numeric operands"))));
        let mut host = Fake::default();
        let mut i = inv("f(a: g() * 2)", FOREVER);
        assert_eq!(i.step(&s, &mut host), Step::Waiting);
        assert!(i.deliver(1, Ok(json!("word"))));
        assert_eq!(i.step(&s, &mut host), Step::Failed(Failure::plain("Arithmetic operations require numeric operands")));

        // An interpolated string's result is its text.
        let mut host = Fake::default();
        let mut i = Invocation::for_template("a\\(g())z\\(bogus)", FOREVER, false, true, Instant::now()).unwrap();
        assert_eq!(i.step(&s, &mut host), Step::Waiting);
        assert!(i.deliver(1, Ok(json!(3))));
        assert_eq!(i.step(&s, &mut host), Step::Done(json!("a3z")));

        // Syntax errors are the call parser's.
        assert_eq!(Invocation::new("x", FOREVER, true, Instant::now()).unwrap_err(), Failure::plain(expr::parse_calls("x").unwrap_err()));
    }

    #[test]
    fn timeouts_follow_iterm2() {
        let s = scope();
        let t0 = Instant::now();
        let thirty = Timeout::After(Duration::from_secs(30));

        let mut host = Fake::default();
        let mut i = Invocation::new("f(a: g())", thirty, true, t0).unwrap();
        assert_eq!(i.deadline(), Some(t0 + Duration::from_secs(30)));
        assert_eq!(i.step(&s, &mut host), Step::Waiting);
        let f = i.timed_out();
        assert_eq!(f.kind, FailKind::Timeout);
        assert_eq!(f.reason, "Timeout (30 sec) while evaluating invocation \u{201c}f(a: g())\u{201d}. The timeout occurred while evaluating the following arguments to f: a");
        assert_eq!(i.returning(), None);

        let mut host = Fake::default();
        let mut i = Invocation::new("f(a: 1)", thirty, true, t0).unwrap();
        assert_eq!(i.step(&s, &mut host), Step::Waiting);
        assert_eq!(i.timed_out().reason, "Timeout (30 sec) while evaluating invocation \u{201c}f(a: 1)\u{201d}. The timeout occurred while waiting for f to return.");
        assert_eq!(i.returning(), Some(1));
        assert!(i.deliver(1, Ok(json!(1))));
        assert_eq!(i.step(&s, &mut host), Step::Done(json!(1)));
        assert_eq!(i.deadline(), None, "a finished invocation has no timer");

        // Only the arguments still evaluating are named, the fully qualified name first.
        let mut host = Fake::default();
        let mut i = Invocation::new("ns.f(z: g(), a: h(), m: 1)", Timeout::After(Duration::from_millis(2500)), true, t0).unwrap();
        assert_eq!(i.step(&s, &mut host), Step::Waiting);
        assert!(i.deliver(1, Ok(json!(1))));
        assert_eq!(i.step(&s, &mut host), Step::Waiting);
        assert_eq!(
            i.timed_out().reason,
            "Timeout (2.5 sec) while evaluating invocation \u{201c}ns.f(z: g(), a: h(), m: 1)\u{201d}. The timeout occurred while evaluating the following arguments to ns.f: a"
        );
        assert_eq!(timeout_args_text(30.0, "i", "f", &["a".into(), "b".into()]), "Timeout (30 sec) while evaluating invocation \u{201c}i\u{201d}. The timeout occurred while evaluating the following arguments to f: a, b");

        // Synchronous: built-ins run, script calls answer null without running, and nothing
        // is missing (`SFC:527-532` passes only the arguments' missing set).
        let mut host = Fake::default();
        let mut i = Invocation::new("f(a: 1)", Timeout::Sync, true, t0).unwrap();
        assert_eq!(i.step(&s, &mut host), Step::Done(Value::Null));
        assert!(i.missing().is_empty());
        assert!(i.tokens().is_empty());
        assert_eq!(i.deadline(), None);
        let mut host = Fake::default();
        let mut i = Invocation::new("b(x: 4)", Timeout::Sync, true, t0).unwrap();
        assert_eq!(i.step(&s, &mut host), Step::Done(json!(4)));
        assert!(host.asked.iter().all(|a| a.2), "asked synchronously");

        let i = Invocation::new("f()", Timeout::Never, true, t0).unwrap();
        assert_eq!(i.deadline(), None);
        assert_eq!(Timeout::from_secs(0.0), Timeout::Sync);
        assert_eq!(Timeout::from_secs(5.0), Timeout::After(Duration::from_secs(5)));
        assert_eq!(Timeout::from_secs(f64::INFINITY), Timeout::Never);
        assert_eq!(Timeout::from_secs(2.0 * YEAR), Timeout::Never);
        assert_eq!(Timeout::from_secs(-1.0), Timeout::Never);
        assert_eq!(Timeout::from_secs(f64::NAN), Timeout::Never);
    }

    /// 11b's Snapshot fixture (`api_session`'s tests): window key 1 "pty-A" with tabs 5
    /// (panes 1, 2) and 6 (pane 3), window key 2 "pty-B" with tab 9 (pane 7); pane 8 buried;
    /// pane 3 exited; pane 2 active.
    fn snapshot() -> crate::api_session::Snapshot {
        crate::api_session::Snapshot {
            windows: vec![(1, "pty-A".into(), vec![(5, vec![1, 2]), (6, vec![3])]), (2, "pty-B".into(), vec![(9, vec![7])])],
            buried: vec![8],
            exited: BTreeSet::from([3]),
            active: Some(2),
        }
    }

    #[test]
    fn invoke_targets_follow_iterm2() {
        use pb::invoke_function_request::{App, Context, Method, Session, Tab, Window};
        use pb::invoke_function_response::{Disposition, Error, Status};
        let s = snapshot();
        let req = |context: Option<Context>, timeout: Option<f64>| pb::InvokeFunctionRequest { invocation: Some("f()".into()), timeout, context };
        let err = |status: Status, reason: &str| {
            Err(pb::InvokeFunctionResponse { disposition: Some(Disposition::Error(Error { status: Some(status as i32), error_reason: Some(reason.into()) })) })
        };
        let thirty = Timeout::After(Duration::from_secs(30));
        assert_eq!(invoke_target(&req(Some(Context::App(App {})), None), &s), Ok((Target::App, thirty)));
        assert_eq!(invoke_target(&req(Some(Context::App(App {})), Some(-1.0)), &s), Ok((Target::App, thirty)));
        assert_eq!(invoke_target(&req(Some(Context::App(App {})), Some(0.0)), &s), Ok((Target::App, Timeout::Sync)));
        assert_eq!(invoke_target(&req(Some(Context::App(App {})), Some(2.0)), &s), Ok((Target::App, Timeout::After(Duration::from_secs(2)))));
        assert_eq!(invoke_target(&req(Some(Context::Tab(Tab { tab_id: Some("6".into()) })), None), &s), Ok((Target::Tab(6), thirty)));
        assert_eq!(invoke_target(&req(Some(Context::Tab(Tab { tab_id: Some("77".into()) })), None), &s), err(Status::InvalidId, "No such tab"));
        assert_eq!(invoke_target(&req(Some(Context::Window(Window { window_id: Some("pty-B".into()) })), None), &s), Ok((Target::Window(2), thirty)));
        assert_eq!(invoke_target(&req(Some(Context::Window(Window { window_id: Some("pty-Z".into()) })), None), &s), err(Status::InvalidId, "No such window"));
        assert_eq!(invoke_target(&req(Some(Context::Session(Session { session_id: Some("99".into()) })), None), &s), err(Status::InvalidId, "No such session"));
        assert_eq!(invoke_target(&req(Some(Context::Session(Session { session_id: Some("8".into()) })), None), &s), Ok((Target::Session(8), thirty)));
        assert_eq!(invoke_target(&req(Some(Context::Session(Session { session_id: Some("active".into()) })), None), &s), Ok((Target::Session(2), thirty)));
        assert_eq!(invoke_target(&req(Some(Context::Method(Method { receiver: None })), None), &s), err(Status::RequestMalformed, "No receiver"));
        assert_eq!(invoke_target(&req(Some(Context::Method(Method { receiver: Some("7".into()) })), None), &s), Ok((Target::Method("7".into()), thirty)));
        assert_eq!(invoke_target(&req(None, None), &s), err(Status::RequestMalformed, "Invalid context"));
    }

    #[test]
    fn reports_follow_iterm2() {
        use pb::invoke_function_response::{Disposition, Error, Status, Success};
        assert_eq!(report_heading("Trigger"), "Trigger Function Call Failed");
        assert_eq!(report_text("f()", "boom"), "Error running \u{201c}f()\u{201d}:\nboom");
        assert_eq!(console_text("f()", "boom", "tb"), "An error occurred while running the function invocation \u{201c}f()\u{201d}:\nboom\n\nTraceback:\ntb");
        assert_eq!(object_not_found("pty-X"), "Object with identifier \u{201c}pty-X\u{201d} not found");
        assert_eq!((ERROR_ID, REVEAL_IN_CONSOLE, RESULT_HEADING), ("NoSyncFunctionCallError", "Reveal in Script Console", "Function Call Result"));
        assert_eq!(invoke_response(&Ok(json!(3))), pb::InvokeFunctionResponse { disposition: Some(Disposition::Success(Success { json_result: Some("3".into()) })) });
        assert_eq!(invoke_response(&Ok(json!(["a", 1]))).disposition, Some(Disposition::Success(Success { json_result: Some(r#"["a",1]"#.into()) })));
        let timeout = Failure { reason: "Timeout (30 sec) ...".into(), detail: None, kind: FailKind::Timeout, conn: None };
        assert_eq!(invoke_response(&Err(timeout)), pb::InvokeFunctionResponse { disposition: Some(Disposition::Error(Error { status: Some(Status::Timeout as i32), error_reason: None })) });
        assert_eq!(
            invoke_response(&Err(Failure::plain("boom"))),
            pb::InvokeFunctionResponse { disposition: Some(Disposition::Error(Error { status: Some(Status::Failed as i32), error_reason: Some("boom".into()) })) }
        );

        // Dependencies: the values of the paths read, nothing else.
        let mut v = scope();
        let rec = Recorder { inner: &v, read: RefCell::default() };
        let e = expr::parse_expression("columns + 1").unwrap();
        assert_eq!(expr::eval(&e, &rec, &mut expr::NoCalls, &expr::Ctx { invocation: "", lax: true }), Outcome::Done(json!(81)));
        let read = rec.read.into_inner();
        assert!(read.contains("columns") && !read.contains("user.x"));
        let before = deps_key(&read, &v);
        v.insert("user.x".into(), json!("changed"));
        assert_eq!(deps_key(&read, &v), before, "an unread path");
        v.insert("columns".into(), json!(81));
        assert_ne!(deps_key(&read, &v), before, "a read path");

        // A frame shadows its scope.
        let base = scope();
        let frame = Values::from([("columns".to_string(), json!(1)), ("__knobs".to_string(), json!("{}"))]);
        let l = Layered { frame: &frame, base: &base };
        assert_eq!(expr::Scope::value(&l, "columns"), Some(json!(1)));
        assert_eq!(expr::Scope::value(&l, "user.x"), Some(json!("hi")));
        assert_eq!(expr::Scope::value(&l, "__knobs"), Some(json!("{}")));
    }
}

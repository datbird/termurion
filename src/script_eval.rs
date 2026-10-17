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

/// A memo key: a call's site. The language has no loops, so each site runs at most once
/// per invocation (iTerm2 evaluates each parsed call once, `SFC:211-212`).
type Key = usize;

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
    /// `lax` is `laxNilPolicyInInterpolatedStrings`, for strings inside the arguments.
    pub fn new(text: &str, timeout: Timeout, side_effects: bool, lax: bool, now: Instant) -> Result<Invocation, Failure> {
        let calls = expr::parse_calls(text).map_err(Failure::plain)?;
        Ok(Invocation::with(text, Body::Calls(calls.into_iter().map(Expr::Call).collect()), lax, timeout, side_effects, now))
    }

    /// An expression (a variable, a literal, an operator, a call) whose result is its value
    /// (`ExpressionBindableView`'s bound expression); a syntax error is the failure.
    pub fn for_expression(text: &str, timeout: Timeout, side_effects: bool, lax: bool, now: Instant) -> Result<Invocation, Failure> {
        let e = expr::parse_expression(text).map_err(Failure::plain)?;
        Ok(Invocation::with(text, Body::Calls(vec![e]), lax, timeout, side_effects, now))
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
                if f.kind == FailKind::Unregistered
                    && let Some(sig) = self.signatures.get(&key) {
                        self.missing.insert(sig.clone());
                    }
                Outcome::Failed(f)
            }
        };
        self.waited.insert(key);
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
        self.waiting.iter().find(|(_, s)| *s == site).map(|(t, _)| *t)
    }

    /// Signatures of calls that found no registered function (`SFC:552-556`).
    pub fn missing(&self) -> &BTreeSet<String> {
        &self.missing
    }

    /// The calls of a sequence, in order (none for an interpolated string): a method
    /// invocation's calls go to its receiver (`SFC:512-521`).
    pub fn calls(&self) -> Vec<&Call> {
        match &self.calls {
            Body::Calls(calls) => calls.iter().filter_map(|e| if let Expr::Call(c) = e { Some(c) } else { None }).collect(),
            Body::Template(_) => Vec::new(),
        }
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
/// asked of the host once. A call is known by its site alone, so a value that changes
/// between steps never runs a call again.
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
        let key: Key = call.site;
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

/// Every path an expression names, in every branch and every call's arguments: iTerm2 reads
/// each variable when it parses (`EP:409-445`), and those reads are the recorded references
/// (`iTermGenericEvaluator.m`, `iTermVariableRecordingScope`).
pub fn paths_in(e: &Expr, out: &mut BTreeSet<String>) {
    match e {
        Expr::Num(_) | Expr::Bool(_) | Expr::Null => {}
        Expr::Path { path, .. } => {
            out.insert(path.clone());
        }
        Expr::Index { path, index } => {
            out.insert(path.clone());
            paths_in(index, out);
        }
        Expr::Str(parts) => template_paths_into(parts, out),
        Expr::Array(items) => items.iter().for_each(|x| paths_in(x, out)),
        Expr::Call(c) => call_paths(c, out),
        Expr::Not(x) | Expr::Neg(x) | Expr::Opt(x) => paths_in(x, out),
        Expr::Bin(l, _, r) => {
            paths_in(l, out);
            paths_in(r, out);
        }
        Expr::Cond(c, t, f) => {
            paths_in(c, out);
            paths_in(t, out);
            paths_in(f, out);
        }
    }
}

/// The paths a call's argument values name (`paths_in`).
pub fn call_paths(c: &Call, out: &mut BTreeSet<String>) {
    for (_, a) in &c.args {
        if let Arg::Value(x) = a {
            paths_in(x, out);
        }
    }
}

fn template_paths_into(parts: &[Part], out: &mut BTreeSet<String>) {
    for p in parts {
        if let Part::Expr(x) = p {
            paths_in(x, out);
        }
    }
}

/// Every path an interpolated string names (`paths_in`).
pub fn template_paths(parts: &[Part]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    template_paths_into(parts, &mut out);
    out
}

/// An interpolated string's dependencies now (`iTermGenericEvaluator.m`'s recorded
/// references): the values of every path it names, as one text.
pub fn template_deps(parts: &[Part], scope: &dyn expr::Scope) -> String {
    deps_key(&template_paths(parts), scope)
}

/// What a status bar string shows after an evaluation (`iTermGenericEvaluator.m`
/// `setEvaluatedResult:error:`): the text of its value, or its error; None while it waits
/// (the previous value stays).
pub fn template_outcome(value: &Outcome) -> Option<Result<String, String>> {
    match value {
        Outcome::Done(v) => Some(Ok(expr::to_text(v))),
        Outcome::Failed(f) => Some(Err(f.reason.clone())),
        Outcome::Pending => None,
    }
}

/// "Invocation of ... failed with error" (`iTermGenericEvaluator.m`): a failed status bar
/// string, in the failing connection's log.
pub fn template_failed_text(template: &str, reason: &str) -> String {
    format!("Invocation of \u{201c}{template}\u{201d} failed with error:\n{reason}\n")
}

/// What a title shows while its Title Function names no registered provider, or when the
/// provider's script is gone (`SNC:122-126`, `SNC:226`).
pub const TITLE_UNREGISTERED: &str = "\u{2026}";
/// What a failed evaluation shows (the status bar's too, `SNC:226`).
pub const LADYBUG: &str = "\u{1F41E}";

/// iTerm2's `whitespaceCharacterSet`: tab and the space separators (no line breaks).
fn is_title_space(c: char) -> bool {
    matches!(c, '\t' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}')
}

/// A title provider's result as the title (`SNC:215-228`): the value's text trimmed of
/// spaces and tabs, blank as one space; an unregistered function's failure the ellipsis,
/// any other failure the ladybug.
pub fn provider_title(r: &Result<Value, Failure>) -> String {
    match r {
        Ok(v) => {
            let text = expr::to_text(v);
            let t = text.trim_matches(is_title_space);
            if t.is_empty() { " ".to_string() } else { t.to_string() }
        }
        Err(f) if f.kind == FailKind::Unregistered => TITLE_UNREGISTERED.to_string(),
        Err(_) => LADYBUG.to_string(),
    }
}

/// A failed title evaluation, in the failing connection's log (`SNC:192-199`), with the
/// script's traceback when it gave one.
pub fn title_failed_text(invocation: &str, reason: &str, detail: Option<&str>) -> String {
    let mut s = format!("Invoked \u{201c}{invocation}\u{201d} to compute name for session. Failed with error:\n{reason}\n");
    if let Some(d) = detail {
        s.push_str(&format!("{d}\n"));
    }
    s
}

/// An unregistered title provider, in the log of the connection that registered the
/// invocation's signature, if any (`SNC:206-208`).
pub fn title_unregistered_text(invocation: &str) -> String {
    format!("Could not make a function call into a script. Either its unique identifier changed (in which case you should update your settings in Settings > Profiles > General > Title) or the script is not running. The failed invocation was:\n{invocation}")
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

/// A context menu provider's origin in the error alert (`PS:14863`).
pub fn menu_origin(title: &str) -> String {
    format!("Menu Item \u{201c}{title}\u{201d}")
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
/// The error alert's buttons (`H:568-571`): OK, and "Reveal in Script Console" when the
/// failure has a traceback.
pub fn report_buttons(traceback: bool) -> Vec<&'static str> {
    let mut out = vec![crate::api_auth::OK];
    if traceback {
        out.push(REVEAL_IN_CONSOLE);
    }
    out
}
/// The heading of an invocation's result shown by Open Quickly (`OQW:494`).
pub const RESULT_HEADING: &str = "Function Call Result";

/// The body of Open Quickly's result alert (`OQW:495`): the value as NSObject's
/// `description` prints it (a top-level null is nil, "(null)", `H:3481`).
pub fn result_text(invocation: &str, v: &Value) -> String {
    format!("{invocation} returned:\n{}", description_text(v))
}

/// A value as `%@` prints it (NSObject's `description`; nil "(null)").
pub fn description_text(v: &Value) -> String {
    if v.is_null() { "(null)".to_string() } else { description(v, 0, true) }
}

/// Foundation's `description` of a decoded JSON value: a string itself (quoted inside a
/// collection when it is not plain alphanumerics, `"` `\\` and controls escaped, characters
/// outside ASCII as `\\Uxxxx`), a number as NSNumber prints it, NSNull `<null>`, an array
/// `(` one element per line `)`, a dictionary `{` `key = value;` per line in key order `}`,
/// each level indented four spaces; a nested collection's brackets carry its own level's
/// indent too (`b =     (`).
fn description(v: &Value, level: usize, top: bool) -> String {
    let pad = |n: usize| "    ".repeat(n);
    match v {
        Value::String(s) if top => s.clone(),
        Value::String(s) => quoted(s),
        Value::Null => "<null>".to_string(),
        Value::Array(items) => {
            let body: Vec<String> = items.iter().map(|x| format!("{}{}", pad(level + 1), description(x, level + 1, false))).collect();
            let body = if body.is_empty() { String::new() } else { format!("{}\n", body.join(",\n")) };
            format!("{}(\n{body}{})", pad(level), pad(level))
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let body: String = keys.iter().map(|k| format!("{}{} = {};\n", pad(level + 1), quoted(k), description(&map[*k], level + 1, false))).collect();
            format!("{}{{\n{body}{}}}", pad(level), pad(level))
        }
        other => expr::to_text(other),
    }
}

/// A string inside a collection's description: bare when it is ASCII letters and digits,
/// else quoted and escaped.
fn quoted(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric()) {
        return s.to_string();
    }
    let mut out = String::from("\"");
    for u in s.encode_utf16() {
        match u {
            0x22 => out.push_str("\\\""),
            0x5c => out.push_str("\\\\"),
            0x0a => out.push_str("\\n"),
            0x09 => out.push_str("\\t"),
            0x0d => out.push_str("\\r"),
            0x20..=0x7e => out.push(char::from(u as u8)),
            _ => out.push_str(&format!("\\U{u:04x}")),
        }
    }
    out.push('"');
    out
}

/// The temporary values an Invoke Script Function trigger adds to the session's scope
/// (`Triggers/iTermRPCTrigger.m:55-78`, `PS:25443-25452`): its captured strings, their
/// ranges as `[location, length]` pairs, the line's text and its absolute number, and the
/// captures again as `matches` (the backreferences).
pub fn trigger_frame(captures: &[String], ranges: &[(usize, usize)], input: &str, line: i64) -> Values {
    let pairs: Vec<Value> = ranges.iter().map(|(at, len)| serde_json::json!([at, len])).collect();
    Values::from([
        ("trigger.captured_strings".to_string(), serde_json::json!(captures)),
        ("trigger.captured_ranges".to_string(), Value::Array(pairs)),
        ("trigger.input".to_string(), Value::from(input)),
        ("trigger.line_number".to_string(), Value::from(line)),
        ("matches".to_string(), serde_json::json!(captures)),
    ])
}

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

pub fn invoke_error(status: pb::invoke_function_response::Status, reason: Option<&str>) -> pb::InvokeFunctionResponse {
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
        Invocation::new(text, timeout, true, true, Instant::now()).unwrap_or_else(|f| panic!("{text}: {f:?}"))
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

        // A value that changes while a call waits never runs a call again: each site runs
        // once (iTerm2 reads every variable once, when it parses, `SFC:211-212`).
        let mut host = Fake::default();
        let mut i = inv("f(a: g(x: columns), b: h())", FOREVER);
        assert_eq!(i.step(&s, &mut host), Step::Waiting);
        assert_eq!(host.names(), ["g", "h"]);
        assert!(i.deliver(2, Ok(json!("hv"))));
        let mut changed = scope();
        changed.insert("columns".into(), json!(81));
        assert_eq!(i.step(&changed, &mut host), Step::Waiting);
        assert_eq!(host.names(), ["g", "h"], "g is not asked again with the new value");
        assert!(i.deliver(1, Ok(json!("gv"))));
        assert_eq!(i.step(&changed, &mut host), Step::Waiting);
        assert_eq!(host.names(), ["g", "h", "f"]);
        assert_eq!(host.asked[2].1, BTreeMap::from([("a".to_string(), json!("gv")), ("b".to_string(), json!("hv"))]));

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

        // Strings inside call arguments follow `laxNilPolicyInInterpolatedStrings`.
        let mut host = Fake::default();
        let mut i = Invocation::new("b(x: \"a\\(bogus)z\")", FOREVER, true, true, Instant::now()).unwrap();
        assert_eq!(i.step(&s, &mut host), Step::Done(json!("az")));
        let mut i = Invocation::new("b(x: \"a\\(bogus)z\")", FOREVER, true, false, Instant::now()).unwrap();
        assert_eq!(i.step(&s, &mut host), Step::Failed(Failure::plain(expr::undefined("bogus"))));

        // Syntax errors are the call parser's.
        assert_eq!(Invocation::new("x", FOREVER, true, true, Instant::now()).unwrap_err(), Failure::plain(expr::parse_calls("x").unwrap_err()));
    }

    #[test]
    fn timeouts_follow_iterm2() {
        let s = scope();
        let t0 = Instant::now();
        let thirty = Timeout::After(Duration::from_secs(30));

        let mut host = Fake::default();
        let mut i = Invocation::new("f(a: g())", thirty, true, true, t0).unwrap();
        assert_eq!(i.deadline(), Some(t0 + Duration::from_secs(30)));
        assert_eq!(i.step(&s, &mut host), Step::Waiting);
        let f = i.timed_out();
        assert_eq!(f.kind, FailKind::Timeout);
        assert_eq!(f.reason, "Timeout (30 sec) while evaluating invocation \u{201c}f(a: g())\u{201d}. The timeout occurred while evaluating the following arguments to f: a");
        assert_eq!(i.returning(), None);

        let mut host = Fake::default();
        let mut i = Invocation::new("f(a: 1)", thirty, true, true, t0).unwrap();
        assert_eq!(i.step(&s, &mut host), Step::Waiting);
        assert_eq!(i.timed_out().reason, "Timeout (30 sec) while evaluating invocation \u{201c}f(a: 1)\u{201d}. The timeout occurred while waiting for f to return.");
        assert_eq!(i.returning(), Some(1));
        assert!(i.deliver(1, Ok(json!(1))));
        assert_eq!(i.step(&s, &mut host), Step::Done(json!(1)));
        assert_eq!(i.deadline(), None, "a finished invocation has no timer");

        // Only the arguments still evaluating are named, the fully qualified name first.
        let mut host = Fake::default();
        let mut i = Invocation::new("ns.f(z: g(), a: h(), m: 1)", Timeout::After(Duration::from_millis(2500)), true, true, t0).unwrap();
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
        let mut i = Invocation::new("f(a: 1)", Timeout::Sync, true, true, t0).unwrap();
        assert_eq!(i.step(&s, &mut host), Step::Done(Value::Null));
        assert!(i.missing().is_empty());
        assert!(i.tokens().is_empty());
        assert_eq!(i.deadline(), None);
        let mut host = Fake::default();
        let mut i = Invocation::new("b(x: 4)", Timeout::Sync, true, true, t0).unwrap();
        assert_eq!(i.step(&s, &mut host), Step::Done(json!(4)));
        assert!(host.asked.iter().all(|a| a.2), "asked synchronously");

        let i = Invocation::new("f()", Timeout::Never, true, true, t0).unwrap();
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
    fn trigger_invocation_follows_iterm2() {
        let caps = ["ab".to_string(), "b".to_string()];
        let f = trigger_frame(&caps, &[(0, 2), (1, 1)], "xab", 7);
        assert_eq!(f.get("trigger.captured_strings"), Some(&json!(["ab", "b"])));
        assert_eq!(f.get("trigger.captured_ranges"), Some(&json!([[0, 2], [1, 1]])));
        assert_eq!(f.get("trigger.input"), Some(&json!("xab")));
        assert_eq!(f.get("trigger.line_number"), Some(&json!(7)));
        assert_eq!(f.get("matches"), Some(&json!(["ab", "b"])));
        assert_eq!(f.len(), 5);
        // Through an invocation, the frame over a scope: `matches[1]` is the first group.
        let base = scope();
        let l = Layered { frame: &f, base: &base };
        for (text, want) in [("matches[1]", json!("b")), ("trigger.line_number + columns", json!(87)), ("trigger.captured_ranges[1]", json!([1, 1]))] {
            let e = expr::parse_expression(text).unwrap();
            assert_eq!(expr::eval(&e, &l, &mut expr::NoCalls, &expr::Ctx { invocation: "", lax: true }), Outcome::Done(want), "{text}");
        }
        assert_eq!(result_text("f()", &json!(3)), "f() returned:\n3");
        assert_eq!(result_text("f()", &Value::Null), "f() returned:\n(null)");
        assert_eq!(result_text("f()", &json!("hi")), "f() returned:\nhi");
        assert_eq!(result_text("f()", &json!(true)), "f() returned:\n1");
        // Collections as Foundation describes them (review Minor 3).
        assert_eq!(result_text("f()", &json!([1, 2])), "f() returned:\n(\n    1,\n    2\n)");
        assert_eq!(result_text("f()", &json!([])), "f() returned:\n(\n)");
        assert_eq!(result_text("f()", &json!(["ab1", "a b", "", null, 0.5])), "f() returned:\n(\n    ab1,\n    \"a b\",\n    \"\",\n    <null>,\n    0.5\n)");
        assert_eq!(result_text("f()", &json!({"b": [true], "a": "x\u{e9}\"q"})), "f() returned:\n{\n    a = \"x\\U00e9\\\"q\";\n    b =     (\n        1\n    );\n}");
        assert_eq!(result_text("f()", &json!({})), "f() returned:\n{\n}");
        assert_eq!(result_text("f()", &json!([[1]])), "f() returned:\n(\n        (\n        1\n    )\n)");
    }

    #[test]
    fn reports_follow_iterm2() {
        use pb::invoke_function_response::{Disposition, Error, Status, Success};
        assert_eq!(report_heading("Trigger"), "Trigger Function Call Failed");
        assert_eq!(report_text("f()", "boom"), "Error running \u{201c}f()\u{201d}:\nboom");
        assert_eq!(console_text("f()", "boom", "tb"), "An error occurred while running the function invocation \u{201c}f()\u{201d}:\nboom\n\nTraceback:\ntb");
        assert_eq!(object_not_found("pty-X"), "Object with identifier \u{201c}pty-X\u{201d} not found");
        assert_eq!((ERROR_ID, REVEAL_IN_CONSOLE, RESULT_HEADING), ("NoSyncFunctionCallError", "Reveal in Script Console", "Function Call Result"));
        // The second button only with a traceback (`H:568-571`).
        assert_eq!(report_buttons(false), ["OK"]);
        assert_eq!(report_buttons(true), ["OK", "Reveal in Script Console"]);
        assert_eq!(invoke_response(&Ok(json!(3))), pb::InvokeFunctionResponse { disposition: Some(Disposition::Success(Success { json_result: Some("3".into()) })) });
        // A synchronous InvokeFunction's script call answers null: "null" (`H:4376-4394`).
        assert_eq!(invoke_response(&Ok(Value::Null)).disposition, Some(Disposition::Success(Success { json_result: Some("null".into()) })));
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

    /// A title provider's result (`SNC:215-228`): trimmed of spaces and tabs, blank as one
    /// space, a value's text; an unregistered failure the ellipsis, any other the ladybug.
    /// The console texts (`SNC:192-212`).
    #[test]
    fn title_providers_follow_iterm2() {
        assert_eq!(provider_title(&Ok(json!("  hi  "))), "hi");
        assert_eq!(provider_title(&Ok(json!(""))), " ");
        assert_eq!(provider_title(&Ok(json!(3))), "3");
        assert_eq!(provider_title(&Ok(Value::Null)), " ", "a synchronous pass answers nothing");
        assert_eq!(provider_title(&Ok(json!("\ta b\u{3000}"))), "a b");
        assert_eq!(provider_title(&Ok(json!("\nx\n"))), "\nx\n", "newlines are not in the whitespace set");
        let unregistered = Failure { reason: "no".into(), detail: None, kind: FailKind::Unregistered, conn: None };
        assert_eq!(provider_title(&Err(unregistered)), "\u{2026}");
        assert_eq!(provider_title(&Err(Failure::plain("boom"))), "\u{1F41E}");
        assert_eq!(TITLE_UNREGISTERED, "\u{2026}");
        assert_eq!(title_failed_text("title.x.f()", "boom", None), "Invoked \u{201c}title.x.f()\u{201d} to compute name for session. Failed with error:\nboom\n");
        assert_eq!(title_failed_text("title.x.f()", "boom", Some("tb")), "Invoked \u{201c}title.x.f()\u{201d} to compute name for session. Failed with error:\nboom\ntb\n");
        assert_eq!(
            title_unregistered_text("title.x.f()"),
            "Could not make a function call into a script. Either its unique identifier changed (in which case you should update your settings in Settings > Profiles > General > Title) or the script is not running. The failed invocation was:\ntitle.x.f()"
        );
    }

    /// A status bar string's value (`iTermGenericEvaluator.m`): a finished evaluation its
    /// text or its error; a pending one keeps the previous value. Its dependencies are the
    /// paths the string reads, the arguments of calls still waiting included.
    #[test]
    fn templates_follow_iterm2() {
        assert_eq!(template_outcome(&Outcome::Done(json!("x"))), Some(Ok("x".to_string())));
        assert_eq!(template_outcome(&Outcome::Done(json!(3))), Some(Ok("3".to_string())));
        assert_eq!(template_outcome(&Outcome::Failed(Failure::plain("boom"))), Some(Err("boom".to_string())));
        assert_eq!(template_outcome(&Outcome::Pending), None);

        let mut v = scope();
        let parts = expr::parse_template("a\\(f(n: columns))\\(user.x)").unwrap();
        let before = template_deps(&parts, &v);
        v.insert("user.other".into(), json!(1));
        assert_eq!(template_deps(&parts, &v), before, "an unread path");
        v.insert("columns".into(), json!(81));
        let after = template_deps(&parts, &v);
        assert_ne!(after, before, "a call's argument");
        v.insert("user.x".into(), json!("changed"));
        assert_ne!(template_deps(&parts, &v), after, "a part after a waiting call");

        // Review finding 2: every path of the parse, also in a branch a call decides
        // (`EP:409-445` reads each path at parse time), in an index and in a nested string.
        let parts = expr::parse_template("\\(f() ? a : b)\\(m[g(x: i)])\\(h(s: \"\\(q?)\"))").unwrap();
        assert_eq!(template_paths(&parts), BTreeSet::from(["a", "b", "m", "i", "q"].map(String::from)));
        let before = template_deps(&parts, &v);
        v.insert("a".into(), json!(1));
        assert_ne!(template_deps(&parts, &v), before, "a path in a call-decided branch");
        assert_eq!(template_failed_text("\\(f())", "boom"), "Invocation of \u{201c}\\(f())\u{201d} failed with error:\nboom\n");
    }
}

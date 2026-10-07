//! Tandem's isolated JavaScript world (milestone 13x2, Task 0): pure. iTerm2 runs its page
//! scripts in `.defaultClient`, a WebKit content world that shares the DOM with the page and
//! nothing else. WebView2 has no such API, but Chromium's DevTools protocol does: a named
//! isolated world, a binding exposed to that world only, and the context events that say which
//! world a call came from. This file holds the calls that set it up (in order), the table of
//! contexts that decides whether a binding call is Tandem's, and the parameters of every later
//! call. Nothing here touches COM or a page.

use std::collections::HashMap;

use serde_json::{Value, json};

/// The isolated world's name: the name its contexts carry (`.defaultClient` in iTerm2).
pub const WORLD_NAME: &str = "tandem-client";
/// The function the world's scripts call (`Runtime.addBinding`); it exists only in that world.
pub const BINDING: &str = "__tandemClient";
/// The DevTools events a view subscribes to.
pub const EVENTS: &[&str] = &[
    "Runtime.executionContextCreated",
    "Runtime.executionContextDestroyed",
    "Runtime.executionContextsCleared",
    "Runtime.bindingCalled",
    "Target.attachedToTarget",
    "Target.detachedFromTarget",
    "Page.frameNavigated",
];
/// Contexts kept at once (a page that makes frames without end cannot grow the table).
pub const MAX_CONTEXTS: usize = 256;
/// Frame ids interned for a view's life.
pub const MAX_FRAME_KEYS: usize = 1024;

pub type Call = (&'static str, Value);

fn common(client_sources: &[String]) -> Vec<Call> {
    let mut v: Vec<Call> = vec![
        ("Page.enable", json!({})),
        ("Runtime.enable", json!({})),
        // Without `executionContextName` the binding would exist in the page's own world too.
        ("Runtime.addBinding", json!({"name": BINDING, "executionContextName": WORLD_NAME})),
    ];
    for s in client_sources {
        v.push(("Page.addScriptToEvaluateOnNewDocument", json!({"source": s, "worldName": WORLD_NAME})));
    }
    v.push(("Target.setAutoAttach", json!({"autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true})));
    v
}

/// The calls that make the world for a view's own target, in order; each waits for the previous answer.
pub fn setup_calls(client_sources: &[String]) -> Vec<Call> {
    let mut v = common(client_sources);
    v.push(("Page.getFrameTree", json!({})));
    v
}

/// The same for a child session (an out-of-process frame); `Runtime.runIfWaitingForDebugger` is
/// last, and is sent even when an earlier call failed (the frame then has no Tandem world).
pub fn session_calls(client_sources: &[String]) -> Vec<Call> {
    let mut v = common(client_sources);
    v.push(("Runtime.runIfWaitingForDebugger", json!({})));
    v
}

/// Makes the world in a document that already exists (`addScript...` reaches only new ones).
pub fn create_world_call(frame_id: &str) -> Call {
    // `grantUniveralAccess` is the protocol's own spelling.
    ("Page.createIsolatedWorld", json!({"frameId": frame_id, "worldName": WORLD_NAME, "grantUniveralAccess": false}))
}

/// `Runtime.evaluate` in one context, named by its unique id: a numeric context id is reused when a
/// frame changes process, and CDP's own guard against evaluating in another context is the unique id.
pub fn evaluate_params(unique_context_id: &str, expression: &str) -> Value {
    json!({"expression": expression, "uniqueContextId": unique_context_id, "awaitPromise": true, "returnByValue": true, "userGesture": false})
}

/// The value of a `Runtime.evaluate` answer; `undefined` is null, an exception is the error text.
pub fn evaluate_result(json: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(json).map_err(|_| "the answer is not JSON".to_string())?;
    if let Some(d) = v.get("exceptionDetails") {
        let text = d.pointer("/exception/description").and_then(Value::as_str).or_else(|| d.get("text").and_then(Value::as_str)).unwrap_or("exception");
        return Err(text.to_string());
    }
    Ok(v.pointer("/result/value").cloned().unwrap_or(Value::Null))
}

/// Child sessions of one view that get a world; the next one is resumed and left alone.
pub const MAX_SESSIONS: usize = 64;
/// The call that lets an auto-attached target run (it waits for a debugger otherwise).
pub const RESUME: &str = "Runtime.runIfWaitingForDebugger";

/// Every DevTools method Tandem ever calls (P22): a new one is added here with its reviewer.
pub const ALLOWED: &[&str] = &[
    "Page.enable",
    "Page.getFrameTree",
    "Page.addScriptToEvaluateOnNewDocument",
    "Page.removeScriptToEvaluateOnNewDocument",
    "Page.createIsolatedWorld",
    "Runtime.enable",
    "Runtime.addBinding",
    "Runtime.evaluate",
    "Target.setAutoAttach",
    "Runtime.runIfWaitingForDebugger",
    // B11: a target that is not a frame is let go once it is resumed.
    "Target.detachFromTarget",
    // Review I1 (reviewer: the controller): clears the back list after the isolation check's
    // about:blank, so Back on a new pane never reaches it. Keeps only the current entry.
    "Page.resetNavigationHistory",
    "Emulation.setFocusEmulationEnabled",
    "Input.insertText",
    "Input.dispatchKeyEvent",
    "Input.dispatchMouseEvent",
];

/// Clears the back-forward list but the current entry.
pub const RESET_HISTORY: &str = "Page.resetNavigationHistory";

/// The isolation check navigates to about:blank before the first real load; that entry is cleared
/// once the load that follows has committed (also after a restart's setup).
#[derive(Default, Debug)]
pub struct HistoryReset {
    armed: bool,
}

impl HistoryReset {
    /// A load follows a check.
    pub fn arm(&mut self) {
        self.armed = true;
    }

    /// A page loaded: true once per arming, when the list is to be cleared now.
    pub fn loaded(&mut self) -> bool {
        std::mem::take(&mut self.armed)
    }
}

pub fn method_allowed(method: &str) -> bool {
    ALLOWED.contains(&method)
}

/// What an attached target gets (B11).
#[derive(Clone, Debug, PartialEq)]
pub enum Attach {
    /// An iframe within the session cap: `SessionSetup` makes its world, and resumes it last.
    Frame { session: String },
    /// Anything else (workers, a frame over the cap): resumed at once on its own session, then
    /// detached with `Target.detachFromTarget` on the parent's session.
    Resume { session: String },
    Ignore,
}

/// The plan for one `Target.attachedToTarget` body; `live` is the child sessions the view has now.
pub fn attached(json: &str, live: usize) -> Attach {
    let Ok(v) = serde_json::from_str::<Value>(json) else { return Attach::Ignore };
    let Some(session) = v.get("sessionId").and_then(Value::as_str).filter(|s| !s.is_empty() && s.len() <= 256) else { return Attach::Ignore };
    let session = session.to_string();
    if v.pointer("/targetInfo/type").and_then(Value::as_str) == Some("iframe") && live < MAX_SESSIONS {
        Attach::Frame { session }
    } else {
        Attach::Resume { session }
    }
}

/// The parameters of `Target.detachFromTarget`.
pub fn detach_params(session: &str) -> Value {
    json!({"sessionId": session})
}

/// One child session's calls, one at a time: after a failed call only the resume still goes (the
/// frame then has no Tandem world, but it is never left paused).
#[derive(Debug)]
pub struct SessionSetup {
    session: String,
    queue: std::collections::VecDeque<Call>,
    in_flight: bool,
}

impl SessionSetup {
    pub fn new(session: &str, client_sources: &[String]) -> SessionSetup {
        SessionSetup { session: session.to_string(), queue: session_calls(client_sources).into(), in_flight: false }
    }

    pub fn session(&self) -> &str {
        &self.session
    }

    pub fn next_call(&mut self) -> Option<Call> {
        if self.in_flight {
            return None;
        }
        let c = self.queue.pop_front()?;
        self.in_flight = true;
        Some(c)
    }

    pub fn answered(&mut self, r: Result<String, String>) {
        self.in_flight = false;
        if r.is_err() {
            self.queue.retain(|(m, _)| *m == RESUME);
        }
    }

    pub fn done(&self) -> bool {
        self.queue.is_empty() && !self.in_flight
    }
}

/// Whether a view has a Tandem world.
#[derive(Clone, Debug, PartialEq)]
pub enum ClientWorld {
    Setting,
    Ready,
    /// No Client script is injected into this view (never moved to the page world).
    Unavailable(String),
}

/// The run-time check, one call at a time: the setup calls, then a world in every frame of the
/// tree. Any failed answer makes the view's world `Unavailable`.
#[derive(Debug)]
pub struct Setup {
    queue: std::collections::VecDeque<Call>,
    in_flight: Option<&'static str>,
    state: ClientWorld,
    main: Option<String>,
    /// The isolation checks still to run (empty for `unchecked`), and the one in flight.
    checks: std::collections::VecDeque<Check>,
    check_in_flight: Option<Check>,
}

/// The marker a Client source sets in the world the scripts really run in: seeing it only there
/// proves `worldName` was honored.
pub const CANARY: &str = "globalThis.__tandemCanary = 1;";
/// The expression both worlds answer: the binding's type and the marker's.
pub const CHECK_EXPRESSION: &str = "typeof __tandemClient + '|' + typeof __tandemCanary";

/// One isolation check, run once every setup call has answered. The app runs it and answers with
/// `Setup::checked`.
#[derive(Clone, Debug, PartialEq)]
pub enum Check {
    /// Navigate the view to `about:blank` (a new document, so the new-document scripts have run) and
    /// answer when it finished.
    Reload,
    /// Evaluate in the PAGE world (`ExecuteScript`): the answer is the JSON text of the value.
    Page(&'static str),
    /// Evaluate in the main frame's Tandem world: the answer is the JSON text of the value.
    World(&'static str),
}

impl Setup {
    /// The setup with the isolation checks: a canary source goes first, and the view is Ready only
    /// when the checks prove the page world holds neither the binding nor the canary.
    pub fn new(client_sources: &[String]) -> Setup {
        let mut sources = vec![CANARY.to_string()];
        sources.extend(client_sources.iter().cloned());
        let mut s = Setup::unchecked(&sources);
        s.checks = [Check::Reload, Check::Page(CHECK_EXPRESSION), Check::World(CHECK_EXPRESSION)].into();
        s
    }

    /// The calls alone, with no isolation check (the probe's own harness proves isolation itself).
    pub fn unchecked(client_sources: &[String]) -> Setup {
        Setup { queue: setup_calls(client_sources).into(), in_flight: None, state: ClientWorld::Setting, main: None, checks: Default::default(), check_in_flight: None }
    }

    /// Ends the setup as Unavailable (a deadline passed): nothing more is sent.
    pub fn fail(&mut self, why: &str) {
        if self.state == ClientWorld::Setting {
            self.state = ClientWorld::Unavailable(why.to_string());
        }
        self.queue.clear();
        self.checks.clear();
    }

    /// The next isolation check, once every call has answered; None while one is in flight, while
    /// calls remain, when done, or after a failure.
    pub fn next_check(&mut self) -> Option<Check> {
        if self.check_in_flight.is_some() || self.in_flight.is_some() || !self.queue.is_empty() || self.state != ClientWorld::Setting {
            return None;
        }
        let c = self.checks.pop_front()?;
        self.check_in_flight = Some(c.clone());
        Some(c)
    }

    /// The answer to the check in flight (see `Check`): anything but the exact expected text fails closed.
    pub fn checked(&mut self, r: Result<String, String>) {
        let Some(check) = self.check_in_flight.take() else { return };
        let text = |r: Result<String, String>| -> Option<String> { serde_json::from_str::<Value>(&r.ok()?).ok()?.as_str().map(str::to_string) };
        let ok = match check {
            Check::Reload => r.is_ok(),
            Check::Page(_) => text(r).as_deref() == Some("undefined|undefined"),
            Check::World(_) => text(r).as_deref() == Some("function|number"),
        };
        if !ok {
            self.state = ClientWorld::Unavailable(format!("the isolation check failed ({})", match check { Check::Reload => "reload", Check::Page(_) => "page world", Check::World(_) => "tandem world" }));
        } else if self.checks.is_empty() {
            self.state = ClientWorld::Ready;
        }
    }

    pub fn state(&self) -> &ClientWorld {
        &self.state
    }

    /// The main frame's id, once the tree is read.
    pub fn main_frame(&self) -> Option<&str> {
        self.main.as_deref()
    }

    /// The next call to send; None while one is in flight, when done, or after a failure.
    pub fn next_call(&mut self) -> Option<Call> {
        if self.in_flight.is_some() || self.state != ClientWorld::Setting {
            return None;
        }
        let c = self.queue.pop_front()?;
        self.in_flight = Some(c.0);
        Some(c)
    }

    /// The answer to the call in flight.
    pub fn answered(&mut self, r: Result<String, String>) {
        let Some(method) = self.in_flight.take() else { return };
        match r {
            Err(e) => self.state = ClientWorld::Unavailable(format!("{method} failed: {}", e.chars().take(120).collect::<String>())),
            Ok(body) => {
                if method == "Page.getFrameTree" {
                    match frame_ids(&body) {
                        Some((main, all)) => {
                            for f in &all {
                                self.queue.push_back(create_world_call(f));
                            }
                            self.main = Some(main);
                        }
                        None => self.state = ClientWorld::Unavailable("the frame tree did not parse".into()),
                    }
                }
            }
        }
        if self.state == ClientWorld::Setting && self.queue.is_empty() && self.checks.is_empty() {
            self.state = ClientWorld::Ready;
        }
    }
}

/// The main frame id and every frame id of a `Page.getFrameTree` answer (the main frame first).
pub fn frame_ids(json: &str) -> Option<(String, Vec<String>)> {
    fn walk(n: &Value, out: &mut Vec<String>) {
        if out.len() >= MAX_FRAME_KEYS {
            return;
        }
        if let Some(id) = n.pointer("/frame/id").and_then(Value::as_str) {
            out.push(id.to_string());
        }
        if let Some(kids) = n.get("childFrames").and_then(Value::as_array) {
            for k in kids {
                walk(k, out);
            }
        }
    }
    let v: Value = serde_json::from_str(json).ok()?;
    let tree = v.get("frameTree")?;
    let main = tree.pointer("/frame/id")?.as_str()?.to_string();
    let mut all = Vec::new();
    walk(tree, &mut all);
    Some((main, all))
}

/// A view's Tandem world is usable only when it is Ready and the main frame's context is known.
pub fn world_usable(state: &ClientWorld, contexts: &Contexts) -> bool {
    *state == ClientWorld::Ready && contexts.context_for(None).is_some()
}

#[derive(Debug)]
struct Ctx {
    frame: String,
    origin: String,
    unique: String,
    /// Creation order: the newest context of a frame is the live one.
    seq: u64,
}

/// The execution contexts that are Tandem's world, keyed by session and context id (ids are per
/// session and die with their document).
#[derive(Default, Debug)]
pub struct Contexts {
    main: Option<String>,
    table: HashMap<(Option<String>, i64), Ctx>,
    keys: HashMap<String, u64>,
    frame_names: Vec<String>,
    seq: u64,
}

impl Contexts {
    pub fn set_main_frame(&mut self, frame_id: &str) {
        self.main = Some(frame_id.to_string());
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    /// Records a context only when it is an isolated world named `WORLD_NAME`: a page cannot make one
    /// (its own contexts are "default").
    pub fn created(&mut self, session: Option<&str>, json: &str) {
        let Ok(v) = serde_json::from_str::<Value>(json) else { return };
        let Some(c) = v.get("context") else { return };
        if c.get("name").and_then(Value::as_str) != Some(WORLD_NAME) || c.pointer("/auxData/type").and_then(Value::as_str) != Some("isolated") {
            return;
        }
        // No unique id, no entry: nothing is ever evaluated by the reusable numeric id.
        let (Some(id), Some(frame), Some(unique)) = (c.get("id").and_then(Value::as_i64), c.pointer("/auxData/frameId").and_then(Value::as_str), c.get("uniqueId").and_then(Value::as_str)) else { return };
        let origin = c.get("origin").and_then(Value::as_str).unwrap_or_default().chars().take(crate::webhost::MAX_URL).collect();
        let key = (session.map(str::to_string), id);
        if !self.table.contains_key(&key) && self.table.len() >= MAX_CONTEXTS {
            return;
        }
        self.seq += 1;
        self.table.insert(key, Ctx { frame: frame.to_string(), origin, unique: unique.chars().take(128).collect(), seq: self.seq });
    }

    pub fn destroyed(&mut self, session: Option<&str>, json: &str) {
        let Ok(v) = serde_json::from_str::<Value>(json) else { return };
        let key = |id: i64| (session.map(str::to_string), id);
        // A late Destroyed of an old document names its unique id: it must not remove the new context
        // that reuses the numeric id.
        if let Some(u) = v.get("executionContextUniqueId").and_then(Value::as_str) {
            self.table.retain(|k, c| !(k.0.as_deref() == session && c.unique == u));
            return;
        }
        if let Some(id) = v.get("executionContextId").and_then(Value::as_i64) {
            self.table.remove(&key(id));
        }
    }

    pub fn cleared(&mut self, session: Option<&str>) {
        self.table.retain(|(s, _), _| s.as_deref() != session);
    }

    /// `Page.frameNavigated`: a frame with no parent in the view's own target is the main frame (its
    /// id can change when the main frame changes process).
    pub fn navigated(&mut self, session: Option<&str>, json: &str) {
        if session.is_some() {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(json) else { return };
        if v.pointer("/frame/parentId").is_none() {
            if let Some(id) = v.pointer("/frame/id").and_then(Value::as_str) {
                self.main = Some(id.to_string());
            }
        }
    }

    pub fn detached(&mut self, session: &str) {
        self.cleared(Some(session));
    }

    /// None for the main frame; any other frame id gets a key that is stable for the view's life
    /// (None too when the table of keys is full).
    pub fn frame_key(&mut self, _session: Option<&str>, frame_id: &str) -> Option<u64> {
        if self.main.as_deref() == Some(frame_id) {
            return None;
        }
        if let Some(k) = self.keys.get(frame_id) {
            return Some(*k);
        }
        if self.frame_names.len() >= MAX_FRAME_KEYS {
            return None;
        }
        let k = self.frame_names.len() as u64 + 1;
        self.keys.insert(frame_id.to_string(), k);
        self.frame_names.push(frame_id.to_string());
        Some(k)
    }

    /// The session and context id of a frame's Tandem world (None for the main frame's own).
    pub fn context_for(&self, frame: Option<u64>) -> Option<(Option<String>, i64)> {
        let name = match frame {
            None => self.main.as_deref()?,
            Some(k) => self.frame_names.get((k as usize).checked_sub(1)?)?.as_str(),
        };
        self.table.iter().filter(|(_, c)| c.frame == name).max_by_key(|(_, c)| c.seq).map(|((s, id), _)| (s.clone(), *id))
    }

    /// The session and unique context id of a frame's Tandem world: what `Runtime.evaluate` names.
    pub fn unique_for(&self, frame: Option<u64>) -> Option<(Option<String>, String)> {
        let name = match frame {
            None => self.main.as_deref()?,
            Some(k) => self.frame_names.get((k as usize).checked_sub(1)?)?.as_str(),
        };
        self.table.iter().filter(|(_, c)| c.frame == name).max_by_key(|(_, c)| c.seq).map(|((s, _), c)| (s.clone(), c.unique.clone()))
    }

    /// The keys of the non-main frames that have a Tandem world.
    pub fn frames(&self) -> Vec<u64> {
        let mut v: Vec<u64> = self.table.values().filter_map(|c| self.keys.get(&c.frame).copied()).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// `Runtime.bindingCalled`: Some((frame, origin, payload)) only for `BINDING` from a recorded
    /// Tandem context of that session; every other call is None.
    pub fn binding_called(&mut self, session: Option<&str>, json: &str) -> Option<(Option<u64>, String, String)> {
        let v: Value = serde_json::from_str(json).ok()?;
        if v.get("name").and_then(Value::as_str) != Some(BINDING) {
            return None;
        }
        let payload = v.get("payload").and_then(Value::as_str)?;
        if payload.len() > crate::webhost::MAX_MESSAGE {
            return None;
        }
        let id = v.get("executionContextId").and_then(Value::as_i64)?;
        let c = self.table.get(&(session.map(str::to_string), id))?;
        let (frame, origin) = (c.frame.clone(), c.origin.clone());
        let key = if self.main.as_deref() == Some(frame.as_str()) && session.is_none() { None } else { Some(self.frame_key(session, &frame)?) };
        Some((key, origin, payload.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn names(c: &[(&'static str, serde_json::Value)]) -> Vec<&'static str> {
        c.iter().map(|(n, _)| *n).collect()
    }

    /// Review Focus 0: the setup follows the protocol's own method names, in order.
    #[test]
    fn world_setup_follows_cdp() {
        let calls = setup_calls(&["A".into(), "B".into()]);
        assert_eq!(
            names(&calls),
            ["Page.enable", "Runtime.enable", "Runtime.addBinding", "Page.addScriptToEvaluateOnNewDocument", "Page.addScriptToEvaluateOnNewDocument", "Target.setAutoAttach", "Page.getFrameTree"]
        );
        assert_eq!(calls[2].1, json!({"name": "__tandemClient", "executionContextName": "tandem-client"}));
        assert!(calls[2].1.get("executionContextName").is_some(), "a binding without a context name is exposed to the page");
        assert_eq!(calls[3].1, json!({"source": "A", "worldName": "tandem-client"}));
        assert_eq!(calls[4].1, json!({"source": "B", "worldName": "tandem-client"}));
        assert!(calls[3].1.get("runImmediately").is_none());
        assert_eq!(calls[5].1, json!({"autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true}));
        let s = session_calls(&["A".into()]);
        assert_eq!(names(&s), ["Page.enable", "Runtime.enable", "Runtime.addBinding", "Page.addScriptToEvaluateOnNewDocument", "Target.setAutoAttach", "Runtime.runIfWaitingForDebugger"]);
        let (m, p) = create_world_call("F");
        assert_eq!(m, "Page.createIsolatedWorld");
        assert_eq!(p, json!({"frameId": "F", "worldName": "tandem-client", "grantUniveralAccess": false}));
        let e = evaluate_params("U5", "1");
        assert_eq!((e["uniqueContextId"].as_str(), e["awaitPromise"].as_bool(), e["returnByValue"].as_bool(), e["userGesture"].as_bool()), (Some("U5"), Some(true), Some(true), Some(false)));
        assert!(e.get("contextId").is_none(), "never the reusable numeric id");
        assert_eq!(e["expression"], "1");
        assert_eq!(evaluate_result(r#"{"result":{"type":"number","value":42}}"#), Ok(json!(42)));
        assert_eq!(evaluate_result(r#"{"result":{"type":"undefined"}}"#), Ok(serde_json::Value::Null));
        assert_eq!(evaluate_result(r#"{"result":{"type":"object"},"exceptionDetails":{"text":"Uncaught","exception":{"description":"Error: E"}}}"#), Err("Error: E".into()));
        assert_eq!(evaluate_result(r#"{"result":{},"exceptionDetails":{"text":"Uncaught"}}"#), Err("Uncaught".into()));
        assert!(evaluate_result("not json").is_err());
    }

    fn created(id: i64, name: &str, kind: &str, frame: &str, origin: &str) -> String {
        json!({"context": {"id": id, "origin": origin, "name": name, "uniqueId": format!("u{id}"), "auxData": {"frameId": frame, "isDefault": false, "type": kind}}}).to_string()
    }

    #[test]
    fn contexts_follow_cdp() {
        let mut c = Contexts::default();
        c.set_main_frame("M");
        c.created(None, &created(3, "tandem-client", "isolated", "M", "https://a.invalid"));
        assert_eq!(c.context_for(None), Some((None, 3)));
        // A page cannot name its own world: a default context, or any other name, is ignored.
        c.created(None, &created(4, "tandem-client", "default", "M", "https://evil.invalid"));
        c.created(None, &created(5, "other", "isolated", "M", "https://evil.invalid"));
        assert_eq!(c.context_for(None), Some((None, 3)));
        let k = c.frame_key(Some("S1"), "C").expect("a child frame has a key");
        assert_eq!(c.frame_key(Some("S1"), "C"), Some(k));
        assert_eq!(c.frame_key(None, "M"), None);
        c.created(Some("S1"), &created(7, "tandem-client", "isolated", "C", "https://b.invalid"));
        assert_eq!(c.context_for(Some(k)), Some((Some("S1".into()), 7)));
        assert_eq!(c.frames(), vec![k]);
        let call = |name: &str, id: i64| json!({"name": name, "payload": "{}", "executionContextId": id}).to_string();
        assert_eq!(c.binding_called(None, &call("__tandemClient", 3)), Some((None, "https://a.invalid".into(), "{}".into())));
        assert_eq!(c.binding_called(Some("S1"), &call("__tandemClient", 7)), Some((Some(k), "https://b.invalid".into(), "{}".into())));
        // The page's own contexts, an unknown id, another session's id and another name are not Tandem's.
        assert_eq!(c.binding_called(None, &call("__tandemClient", 4)), None);
        assert_eq!(c.binding_called(None, &call("__tandemClient", 5)), None);
        assert_eq!(c.binding_called(None, &call("__tandemClient", 99)), None);
        assert_eq!(c.binding_called(Some("S1"), &call("__tandemClient", 3)), None);
        assert_eq!(c.binding_called(None, &call("__tandemClient", 7)), None);
        assert_eq!(c.binding_called(None, &call("other", 3)), None);
        assert_eq!(c.binding_called(None, "garbage"), None);
        assert_eq!(c.binding_called(None, &json!({"name": "__tandemClient", "executionContextId": 3}).to_string()), None);
        c.destroyed(None, &json!({"executionContextId": 3}).to_string());
        assert_eq!(c.context_for(None), None);
        assert_eq!(c.binding_called(None, &call("__tandemClient", 3)), None);
        c.created(None, &created(8, "tandem-client", "isolated", "M", "https://a.invalid"));
        assert_eq!(c.context_for(None), Some((None, 8)));
        c.cleared(None);
        assert_eq!(c.context_for(None), None);
        assert_eq!(c.context_for(Some(k)), Some((Some("S1".into()), 7)), "another session's contexts survive");
        c.detached("S1");
        assert_eq!(c.context_for(Some(k)), None);
        // The main frame is the one frame with no parent in the view's own target.
        c.navigated(Some("S1"), r#"{"frame":{"id":"Z"}}"#);
        c.navigated(None, r#"{"frame":{"id":"Q","parentId":"M"}}"#);
        c.created(None, &created(9, "tandem-client", "isolated", "M", "o"));
        assert_eq!(c.context_for(None), Some((None, 9)));
        c.navigated(None, r#"{"frame":{"id":"N"}}"#);
        assert_eq!(c.context_for(None), None);
        assert_eq!(c.frame_key(Some("S1"), "C"), Some(k), "the key is stable for the view's life");
    }

    /// Fix round 1, item 1: a process swap reuses numeric ids; the stale context is never picked and a
    /// late Destroyed of the old one never removes the new one. Evaluation names the unique id.
    #[test]
    fn a_reused_id_never_picks_the_stale_context() {
        let mut c = Contexts::default();
        c.set_main_frame("M");
        let made = |id: i64, unique: &str, frame: &str| json!({"context": {"id": id, "origin": "o", "name": "tandem-client", "uniqueId": unique, "auxData": {"frameId": frame, "type": "isolated"}}}).to_string();
        c.created(None, &made(3, "OLD", "M"));
        // The old document's Destroyed was lost; the new renderer reuses id 3 and a lower id 2 is stale.
        c.created(None, &made(2, "STALE", "M"));
        c.created(None, &made(3, "NEW", "M"));
        assert_eq!(c.unique_for(None), Some((None, "NEW".to_string())), "the newest context wins, never the lowest id");
        // A late Destroyed that names the old unique id removes only the old one.
        c.destroyed(None, &json!({"executionContextId": 3, "executionContextUniqueId": "OLD"}).to_string());
        assert_eq!(c.unique_for(None), Some((None, "NEW".to_string())));
        let call = json!({"name": "__tandemClient", "payload": "{}", "executionContextId": 3}).to_string();
        assert!(c.binding_called(None, &call).is_some());
        c.destroyed(None, &json!({"executionContextId": 3, "executionContextUniqueId": "NEW"}).to_string());
        assert!(c.binding_called(None, &call).is_none());
        // A context with no unique id is never recorded.
        let mut d = Contexts::default();
        d.set_main_frame("M");
        d.created(None, &json!({"context": {"id": 1, "origin": "o", "name": "tandem-client", "auxData": {"frameId": "M", "type": "isolated"}}}).to_string());
        assert!(d.is_empty());
    }

    #[test]
    fn the_table_is_bounded() {
        let mut c = Contexts::default();
        for i in 0..(MAX_CONTEXTS as i64 + 50) {
            c.created(None, &created(i, "tandem-client", "isolated", &format!("F{i}"), "https://x.invalid"));
        }
        assert!(c.len() <= MAX_CONTEXTS);
        for i in 0..(MAX_FRAME_KEYS as u32 + 50) {
            let _ = c.frame_key(None, &format!("G{i}"));
        }
        assert_eq!(c.frame_key(None, "G-new-one-too-many"), None);
    }

    /// Drives a setup to its end: every call answers `{}` (the tree for `getFrameTree`), then the checks.
    fn drive(s: &mut Setup, page: Result<&str, &str>, world: Result<&str, &str>) -> Vec<String> {
        let tree = r#"{"frameTree":{"frame":{"id":"M"},"childFrames":[{"frame":{"id":"K"}}]}}"#;
        let mut sent = Vec::new();
        while let Some((m, _)) = s.next_call() {
            sent.push(m.to_string());
            assert!(s.next_call().is_none(), "one call at a time");
            s.answered(Ok(if m == "Page.getFrameTree" { tree.to_string() } else { "{}".to_string() }));
        }
        let owned = |r: Result<&str, &str>| r.map(str::to_string).map_err(str::to_string);
        while let Some(check) = s.next_check() {
            assert!(s.next_check().is_none(), "one check at a time");
            sent.push(format!("{check:?}"));
            match check {
                Check::Reload => s.checked(Ok(String::new())),
                Check::Page(_) => s.checked(owned(page)),
                Check::World(_) => s.checked(owned(world)),
            }
        }
        sent
    }

    /// The run-time check, one call at a time: the setup calls, then a world in every frame of the
    /// tree, then the isolation checks. A failed answer makes the world unavailable.
    #[test]
    fn setup_decides_per_view() {
        let mut s = Setup::new(&["A".into()]);
        assert_eq!(*s.state(), ClientWorld::Setting);
        let sent = drive(&mut s, Ok("\"undefined|undefined\""), Ok("\"function|number\""));
        assert_eq!(*s.state(), ClientWorld::Ready);
        assert_eq!(s.main_frame(), Some("M"));
        assert_eq!(&sent[..8], &["Page.enable", "Runtime.enable", "Runtime.addBinding", "Page.addScriptToEvaluateOnNewDocument", "Page.addScriptToEvaluateOnNewDocument", "Target.setAutoAttach", "Page.getFrameTree", "Page.createIsolatedWorld"]);
        assert_eq!(&sent[9..], &["Reload", "Page(\"typeof __tandemClient + '|' + typeof __tandemCanary\")", "World(\"typeof __tandemClient + '|' + typeof __tandemCanary\")"], "{sent:?}");
        assert_eq!(sent.len(), 12, "{sent:?}");

        let mut f = Setup::new(&["A".into()]);
        let _ = f.next_call();
        f.answered(Err("no".into()));
        assert!(matches!(f.state(), ClientWorld::Unavailable(_)));
        assert!(f.next_call().is_none() && f.next_check().is_none(), "nothing more is sent after a failure");
        let mut g = Setup::new(&[]);
        while let Some((m, _)) = g.next_call() {
            g.answered(Ok(if m == "Page.getFrameTree" { "not a tree".into() } else { "{}".into() }));
        }
        assert!(matches!(g.state(), ClientWorld::Unavailable(_)), "a frame tree that does not parse is a failure");
    }

    /// Fix round 1, item 2: answers alone prove nothing (CDP ignores parameters it does not know), so
    /// the view is Ready only when the page world has neither the binding nor the world's marker and
    /// the isolated world has both. Any doubt is Unavailable.
    #[test]
    fn setup_fails_closed_when_the_world_is_not_isolated() {
        let case = |page: Result<&str, &str>, world: Result<&str, &str>| {
            let mut s = Setup::new(&[]);
            drive(&mut s, page, world);
            s.state().clone()
        };
        let (p, w) = ("\"undefined|undefined\"", "\"function|number\"");
        assert_eq!(case(Ok(p), Ok(w)), ClientWorld::Ready);
        for (name, page, world) in [
            ("the binding exists in the page world", Ok("\"function|undefined\""), Ok(w)),
            ("the marker exists in the page world", Ok("\"undefined|number\""), Ok(w)),
            ("the binding is missing in the world", Ok(p), Ok("\"undefined|number\"")),
            ("the marker is missing in the world", Ok(p), Ok("\"function|undefined\"")),
            ("the page check failed", Err("boom"), Ok(w)),
            ("the world check failed", Ok(p), Err("boom")),
            ("the page answer is not a string", Ok("5"), Ok(w)),
            ("the world answer is not JSON", Ok(p), Ok("nope")),
        ] {
            assert!(matches!(case(page, world), ClientWorld::Unavailable(_)), "{name}");
        }
        // A deadline ends it, and nothing more is sent.
        let mut d = Setup::new(&[]);
        let _ = d.next_call();
        d.fail("late");
        assert!(matches!(d.state(), ClientWorld::Unavailable(_)) && d.next_call().is_none() && d.next_check().is_none());
        // The probe's own harness runs the calls without the checks.
        let mut u = Setup::unchecked(&[]);
        let mut n = 0;
        while let Some((m, _)) = u.next_call() {
            n += 1;
            u.answered(Ok(if m == "Page.getFrameTree" { r#"{"frameTree":{"frame":{"id":"M"}}}"#.into() } else { "{}".into() }));
        }
        assert!(n > 0);
        assert_eq!(*u.state(), ClientWorld::Ready);
        assert!(u.next_check().is_none());
    }


    fn attach_json(session: &str, ty: &str, waiting: bool) -> String {
        json!({"sessionId": session, "targetInfo": {"targetId": "T", "type": ty, "url": "https://x.invalid/"}, "waitingForDebugger": waiting}).to_string()
    }

    /// B11/P11: every auto-attached target ends with exactly one `Runtime.runIfWaitingForDebugger` on
    /// its own session, whatever its type and even when its setup fails; a target that is not an
    /// iframe is then detached (on the parent's session); a view has at most 64 child sessions.
    #[test]
    fn attached_targets_are_resumed() {
        let sources = vec!["A".to_string()];
        for ty in ["iframe", "worker", "service_worker", "shared_worker", "page", "other"] {
            for fail_first in [false, true] {
                let mut sent: Vec<(String, &str)> = Vec::new();
                match attached(&attach_json("S1", ty, true), 0) {
                    Attach::Frame { session } => {
                        assert_eq!(ty, "iframe");
                        let mut s = SessionSetup::new(&session, &sources);
                        let mut first = true;
                        while let Some((m, _)) = s.next_call() {
                            sent.push((session.clone(), m));
                            s.answered(if first && fail_first { Err("boom".into()) } else { Ok("{}".into()) });
                            first = false;
                        }
                    }
                    Attach::Resume { session } => {
                        assert_ne!(ty, "iframe");
                        sent.push((session.clone(), RESUME));
                        sent.push(("parent".into(), "Target.detachFromTarget"));
                    }
                    Attach::Ignore => panic!("{ty} is ignored"),
                }
                let resumes: Vec<_> = sent.iter().filter(|(_, m)| *m == RESUME).collect();
                assert_eq!(resumes.len(), 1, "{ty} fail_first={fail_first}: {sent:?}");
                assert_eq!(resumes[0].0, "S1");
                if ty == "iframe" {
                    assert_eq!(sent.last().map(|(_, m)| *m), Some(RESUME), "the resume is last: {sent:?}");
                    assert!(!sent.iter().any(|(_, m)| *m == "Target.detachFromTarget"), "a frame keeps its session");
                } else {
                    assert!(sent.iter().any(|(_, m)| *m == "Target.detachFromTarget"));
                }
            }
        }
        // The 65th session is resumed and never set up, even an iframe.
        assert_eq!(attached(&attach_json("S65", "iframe", true), MAX_SESSIONS), Attach::Resume { session: "S65".into() });
        assert!(matches!(attached(&attach_json("S64", "iframe", true), MAX_SESSIONS - 1), Attach::Frame { .. }));
        // No session id, no target info or a garbled body: nothing to resume.
        assert_eq!(attached("{}", 0), Attach::Ignore);
        assert_eq!(attached("nope", 0), Attach::Ignore);
        assert_eq!(detach_params("S1"), json!({"sessionId": "S1"}));
    }

    /// P22: the DevTools protocol is Rust-only and a closed set of methods; no debugging port or
    /// origin flag exists anywhere in the program.
    #[test]
    fn devtools_methods_are_a_closed_set() {
        for m in ["Page.enable", "Page.getFrameTree", "Page.addScriptToEvaluateOnNewDocument", "Page.removeScriptToEvaluateOnNewDocument", "Page.createIsolatedWorld", "Runtime.enable", "Runtime.addBinding", "Runtime.evaluate", "Target.setAutoAttach", "Runtime.runIfWaitingForDebugger", "Target.detachFromTarget", "Page.resetNavigationHistory", "Emulation.setFocusEmulationEnabled", "Input.insertText", "Input.dispatchKeyEvent", "Input.dispatchMouseEvent"] {
            assert!(method_allowed(m), "{m}");
        }
        assert_eq!(ALLOWED.len(), 16);
        for m in ["Page.navigate", "Network.getCookies", "Runtime.callFunctionOn", "Browser.close", "Storage.getCookies", "Target.createTarget", "Fetch.enable", "Page.enable ", "page.enable", ""] {
            assert!(!method_allowed(m), "{m}");
        }
        // Every setup and session call is allowed.
        for (m, _) in setup_calls(&["x".into()]).into_iter().chain(session_calls(&["x".into()])) {
            assert!(method_allowed(m), "{m}");
        }
        // The host refuses anything else before the call is made, and no flag opens a port.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let host = std::fs::read_to_string(root.join("webhost_win.rs")).unwrap();
        let host = host.split("#[cfg(test)]\nmod tests").next().unwrap();
        assert!(host.contains("browser_world::method_allowed(method)"), "the one DevTools sender checks the allow-list");
        let mut dirs = vec![root];
        let flag = ["remote", "-debugging"].concat();
        let origin = ["remote", "-allow-origins"].concat();
        while let Some(d) = dirs.pop() {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    dirs.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    let t = std::fs::read_to_string(&p).unwrap();
                    let live = t.split("#[cfg(test)]").next().unwrap();
                    assert!(!live.contains(&flag) && !live.contains(&origin), "{p:?} names a debugging flag");
                }
            }
        }
    }

    #[test]
    fn a_world_is_only_usable_with_a_context() {
        let mut c = Contexts::default();
        c.set_main_frame("M");
        assert!(!world_usable(&ClientWorld::Ready, &c));
        c.created(None, &created(3, "tandem-client", "isolated", "M", "o"));
        assert!(world_usable(&ClientWorld::Ready, &c));
        assert!(!world_usable(&ClientWorld::Setting, &c));
        assert!(!world_usable(&ClientWorld::Unavailable("x".into()), &c));
    }

    /// Review I1: the isolation check's about:blank is not a history entry the person can go back to:
    /// after the first load that follows a check the back list is cleared, once.
    #[test]
    fn the_check_leaves_no_history_entry() {
        let mut h = HistoryReset::default();
        assert!(!h.loaded(), "nothing is cleared before a check ran");
        h.arm();
        assert!(h.loaded(), "the first load after the check clears the list");
        assert!(!h.loaded(), "only once");
        h.arm();
        h.arm();
        assert!(h.loaded() && !h.loaded());
        assert!(method_allowed(RESET_HISTORY) && RESET_HISTORY == "Page.resetNavigationHistory");
    }
}

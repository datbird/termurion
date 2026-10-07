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

/// `Runtime.evaluate` in one context.
pub fn evaluate_params(context_id: i64, expression: &str) -> Value {
    json!({"expression": expression, "contextId": context_id, "awaitPromise": true, "returnByValue": true, "userGesture": false})
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
}

impl Setup {
    pub fn new(client_sources: &[String]) -> Setup {
        Setup { queue: setup_calls(client_sources).into(), in_flight: None, state: ClientWorld::Setting, main: None }
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
        if self.state == ClientWorld::Setting && self.queue.is_empty() {
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
}

/// The execution contexts that are Tandem's world, keyed by session and context id (ids are per
/// session and die with their document).
#[derive(Default, Debug)]
pub struct Contexts {
    main: Option<String>,
    table: HashMap<(Option<String>, i64), Ctx>,
    keys: HashMap<String, u64>,
    frame_names: Vec<String>,
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
        let (Some(id), Some(frame)) = (c.get("id").and_then(Value::as_i64), c.pointer("/auxData/frameId").and_then(Value::as_str)) else { return };
        let origin = c.get("origin").and_then(Value::as_str).unwrap_or_default().chars().take(crate::webhost::MAX_URL).collect();
        let key = (session.map(str::to_string), id);
        if !self.table.contains_key(&key) && self.table.len() >= MAX_CONTEXTS {
            return;
        }
        self.table.insert(key, Ctx { frame: frame.to_string(), origin });
    }

    pub fn destroyed(&mut self, session: Option<&str>, json: &str) {
        let Some(id) = serde_json::from_str::<Value>(json).ok().and_then(|v| v.get("executionContextId").and_then(Value::as_i64)) else { return };
        self.table.remove(&(session.map(str::to_string), id));
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
        self.table.iter().filter(|(_, c)| c.frame == name).map(|((s, id), _)| (s.clone(), *id)).min_by_key(|(_, id)| *id)
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
        let e = evaluate_params(5, "1");
        assert_eq!((e["contextId"].as_i64(), e["awaitPromise"].as_bool(), e["returnByValue"].as_bool(), e["userGesture"].as_bool()), (Some(5), Some(true), Some(true), Some(false)));
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

    /// The run-time check: the setup runs call by call; a failed answer makes the world unavailable
    /// and no Client script is injected.
    #[test]
    fn setup_decides_per_view() {
        let mut s = Setup::new(&["A".into()]);
        assert_eq!(*s.state(), ClientWorld::Setting);
        let mut sent = Vec::new();
        let tree = r#"{"frameTree":{"frame":{"id":"M"},"childFrames":[{"frame":{"id":"K"}}]}}"#;
        while let Some((m, _)) = s.next_call() {
            sent.push(m);
            assert!(s.next_call().is_none(), "one call at a time");
            s.answered(Ok(if m == "Page.getFrameTree" { tree.to_string() } else { "{}".to_string() }));
        }
        assert_eq!(*s.state(), ClientWorld::Ready);
        assert_eq!(s.main_frame(), Some("M"));
        assert_eq!(&sent[..7], &["Page.enable", "Runtime.enable", "Runtime.addBinding", "Page.addScriptToEvaluateOnNewDocument", "Target.setAutoAttach", "Page.getFrameTree", "Page.createIsolatedWorld"]);
        assert_eq!(sent.len(), 8, "one world per frame of the tree after the setup: {sent:?}");

        let mut f = Setup::new(&["A".into()]);
        let _ = f.next_call();
        f.answered(Err("no".into()));
        assert!(matches!(f.state(), ClientWorld::Unavailable(_)));
        assert!(f.next_call().is_none(), "nothing more is sent after a failure");
        let mut g = Setup::new(&[]);
        for _ in 0..4 {
            let _ = g.next_call();
            g.answered(Ok("{}".into()));
        }
        let _ = g.next_call();
        g.answered(Ok("not a tree".into()));
        assert!(matches!(g.state(), ClientWorld::Unavailable(_)), "a frame tree that does not parse is a failure");
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
}

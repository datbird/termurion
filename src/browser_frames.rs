//! iTerm2's frame graph bookkeeping (milestone 13x2, Task 2; `iTermBrowserGraphDiscoveryHandler.swift`).
//! `graph-discovery.js` runs only in Termurion's isolated world and tells native which frames exist;
//! this module decides what Termurion evaluates and where. Web content is untrusted: a message that
//! is not the isolated world's makes nothing run and registers nothing, and only code a Client
//! script supplied is ever evaluated, in frames registered from the Client world.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::browser_js::{self, Inject, Routed, UserScript, World};

/// The message handler of the graph script (`GDH:5`).
pub const HANDLER: &str = "iTermGraphDiscovery";

/// Frames registered per view, and the longest id or request id read.
const MAX_FRAMES: usize = 512;
const MAX_ID: usize = 256;
/// Reports and evaluations waiting for an answer.
const MAX_WAITING: usize = 256;

/// The script, injected in the isolated world only, in every frame (`BM:252-265`).
pub fn user_script() -> UserScript {
    UserScript {
        id: "GraphDiscovery",
        code: browser_js::fill_template(browser_js::resource("graph-discovery.js").unwrap_or_default(), &[]),
        inject: Inject::DocumentStart,
        main_frame_only: false,
        world: World::Client,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Msg {
    Register(String),
    RequestReport(String),
    Eval { request: String, target: String, sender: String, code: String },
    DomChanged,
}

fn text(body: &Value, key: &str) -> Option<String> {
    body.get(key).and_then(Value::as_str).map(str::to_string)
}

fn id_text(body: &Value, key: &str) -> Option<String> {
    text(body, key).filter(|s| s.len() <= MAX_ID)
}

/// `GDH:20-76`. A report without a request id gets `fresh_id()` so it does not stall.
pub fn parse(body: &Value, fresh_id: &mut dyn FnMut() -> String) -> Option<Msg> {
    match body.get("type")?.as_str()? {
        "REGISTER_FRAME" => Some(Msg::Register(id_text(body, "frameId")?)),
        "REQUEST_REPORT" => Some(Msg::RequestReport(id_text(body, "requestId").unwrap_or_else(fresh_id))),
        // iTerm2 spells this one requestID (`GDH:43`).
        "EVAL_IN_FRAME" => Some(Msg::Eval { target: id_text(body, "target")?, sender: id_text(body, "sender")?, request: id_text(body, "requestID")?, code: text(body, "code")? }),
        "DOM_CHANGED" => Some(Msg::DomChanged),
        _ => None,
    }
}

/// `GDH:134-146`: the code's value as JSON text, or null when it throws.
pub fn eval_wrapper(code: &str) -> String {
    format!(
        "(() => {{ \n    try {{ \n        const raw = {code};\n        const value = JSON.stringify(raw); \n        return value;\n    }} catch (e) {{ \n        console.error(\"[eval] Eval failed\", e.toString(), e);\n        return null;\n    }} \n}})()"
    )
}

/// `GDH:83-89`; `value` is JSON text (see `unwrap_result`).
pub fn eval_response_js(request: &str, value: Option<&str>) -> String {
    format!("window[Symbol.for('iTermGraphDiscovery')].handleEvalResponse({}, {});", browser_js::js_string(request), value.unwrap_or("null"))
}

pub fn prepare_js(request: &str) -> String {
    format!("window[Symbol.for('iTermGraphDiscovery')].prepareForReport({})", browser_js::js_string(request))
}

pub fn report_js(request: &str) -> String {
    format!("window[Symbol.for('iTermGraphDiscovery')].report({})", browser_js::js_string(request))
}

/// `GDH:71`.
pub const REDISCOVER_JS: &str = "window[Symbol.for('iTermGraphDiscovery')].rediscover();";

/// What `eval_wrapper` answers: a string of JSON text, or null. Only text that parses as JSON is
/// returned, and it is returned re-serialized, so what goes into a script is data and nothing else.
pub fn unwrap_result(value: &Value) -> Option<String> {
    let s = value.as_str()?;
    serde_json::from_str::<Value>(s).ok().map(|v| v.to_string())
}

/// One evaluation Termurion makes in the Termurion world of `frame` (None is the main frame); `code` is
/// already wrapped. The answer comes back to `Frames::answered` under `token`.
#[derive(Clone, Debug, PartialEq)]
pub struct Act {
    pub token: u64,
    pub frame: Option<u64>,
    pub code: String,
}

#[derive(Default, Debug)]
pub struct Frames {
    /// Registration order, as iTerm2's dictionary would be walked.
    order: Vec<(String, Option<u64>)>,
    /// Reports in their prepare phase: the request and the tokens still out.
    reports: Vec<(String, Vec<u64>)>,
    evals: HashMap<u64, (Option<u64>, String)>,
    /// Calls whose answer is ignored.
    loose: HashSet<u64>,
    next: u64,
}

impl Frames {
    /// An id belongs to the frame that registered it until that frame is gone (13x3 Task 3): another
    /// frame naming a taken id is refused, so no frame can take over another's id.
    pub fn register(&mut self, id: &str, frame: Option<u64>) {
        if self.order.iter().any(|(k, _)| k == id) {
            // Known: nothing moves.
        } else if self.order.len() < MAX_FRAMES {
            self.order.push((id.to_string(), frame));
        }
    }

    /// Outer None: unknown. Inner None: the main frame.
    pub fn lookup(&self, id: &str) -> Option<Option<u64>> {
        self.order.iter().find(|(k, _)| k == id).map(|(_, f)| *f)
    }

    /// `GDH:16-18`: a navigation forgets every frame and every wait.
    pub fn will_navigate(&mut self) {
        self.order.clear();
        self.reports.clear();
        self.evals.clear();
        self.loose.clear();
    }

    /// A frame's world is gone.
    pub fn gone(&mut self, frame: u64) {
        self.order.retain(|(_, f)| *f != Some(frame));
    }

    pub fn begin_report(&mut self, request: &str, tokens: Vec<u64>) {
        if self.reports.len() < MAX_WAITING && !tokens.is_empty() {
            self.reports.push((request.to_string(), tokens));
        }
    }

    /// When the last prepare token returns: the request and every registered frame for the report phase.
    pub fn phase_done(&mut self, token: u64) -> Option<(String, Vec<Option<u64>>)> {
        let at = self.reports.iter().position(|(_, t)| t.contains(&token))?;
        self.reports[at].1.retain(|t| *t != token);
        if !self.reports[at].1.is_empty() {
            return None;
        }
        let (request, _) = self.reports.remove(at);
        Some((request, self.order.iter().map(|(_, f)| *f).collect()))
    }

    pub fn track_eval(&mut self, token: u64, sender: Option<u64>, request: String) {
        if self.evals.len() < MAX_WAITING {
            self.evals.insert(token, (sender, request));
        }
    }

    pub fn take_eval(&mut self, token: u64) -> Option<(Option<u64>, String)> {
        self.evals.remove(&token)
    }

    fn token(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    fn loose_act(&mut self, frame: Option<u64>, code: String) -> Act {
        let token = self.token();
        if self.loose.len() < MAX_WAITING {
            self.loose.insert(token);
        }
        Act { token, frame, code }
    }

    /// One message of the graph script: what to evaluate. Nothing for a message that is not the
    /// isolated world's, nothing while the world is not ready, and the message is not read first.
    pub fn handle(&mut self, r: &Routed, ready: bool, fresh_id: &mut dyn FnMut() -> String) -> Vec<Act> {
        if r.world != World::Client || !ready {
            return Vec::new();
        }
        let Some(msg) = parse(&r.body, fresh_id) else { return Vec::new() };
        match msg {
            Msg::Register(id) => {
                self.register(&id, r.frame);
                Vec::new()
            }
            Msg::RequestReport(request) => {
                let frames: Vec<Option<u64>> = self.order.iter().map(|(_, f)| *f).collect();
                let acts: Vec<Act> = frames.into_iter().map(|f| Act { token: self.token(), frame: f, code: eval_wrapper(&prepare_js(&request)) }).collect();
                self.begin_report(&request, acts.iter().map(|a| a.token).collect());
                acts
            }
            Msg::Eval { request, target, sender, code } => {
                // The sender is the frame the message came from (the host knows it); a body naming
                // another frame's id as its own is dropped (13x3 Task 3).
                let Some(sender_frame) = self.lookup(&sender).filter(|f| *f == r.frame) else { return Vec::new() };
                match self.lookup(&target) {
                    None => vec![self.loose_act(sender_frame, eval_wrapper(&eval_response_js(&request, None)))],
                    Some(target_frame) => {
                        let token = self.token();
                        self.track_eval(token, sender_frame, request);
                        vec![Act { token, frame: target_frame, code: eval_wrapper(&code) }]
                    }
                }
            }
            Msg::DomChanged => vec![self.loose_act(None, eval_wrapper(REDISCOVER_JS))],
        }
    }

    /// The answer to an evaluation: whether `token` was the graph's, and what to evaluate next.
    pub fn answered(&mut self, token: u64, result: &Result<Value, String>) -> (bool, Vec<Act>) {
        if let Some((sender, request)) = self.take_eval(token) {
            let value = result.as_ref().ok().and_then(unwrap_result);
            let act = self.loose_act(sender, eval_wrapper(&eval_response_js(&request, value.as_deref())));
            return (true, vec![act]);
        }
        let in_phase = self.reports.iter().any(|(_, t)| t.contains(&token));
        if in_phase {
            let acts = match self.phase_done(token) {
                Some((request, frames)) => frames.into_iter().map(|f| self.loose_act(f, eval_wrapper(&report_js(&request)))).collect(),
                None => Vec::new(),
            };
            return (true, acts);
        }
        (self.loose.remove(&token), Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser_js::{Routed, World};
    use serde_json::json;

    fn fresh() -> impl FnMut() -> String {
        let mut n = 0;
        move || {
            n += 1;
            format!("fresh{n}")
        }
    }

    fn routed(world: World, frame: Option<u64>, body: serde_json::Value) -> Routed {
        Routed { handler: HANDLER.into(), world, frame, source: "https://a.test/".into(), body }
    }

    #[test]
    fn graph_messages_follow_iterm2() {
        let mut f = fresh();
        assert_eq!(parse(&json!({"type": "REGISTER_FRAME", "frameId": "f1"}), &mut f), Some(Msg::Register("f1".into())));
        assert_eq!(parse(&json!({"type": "REQUEST_REPORT", "requestId": "r"}), &mut f), Some(Msg::RequestReport("r".into())));
        assert_eq!(parse(&json!({"type": "REQUEST_REPORT"}), &mut f), Some(Msg::RequestReport("fresh1".into())));
        assert_eq!(
            parse(&json!({"type": "EVAL_IN_FRAME", "target": "t", "sender": "s", "requestID": "q", "code": "1"}), &mut f),
            Some(Msg::Eval { request: "q".into(), target: "t".into(), sender: "s".into(), code: "1".into() })
        );
        // iTerm2's spelling here is requestID, not requestId.
        assert_eq!(parse(&json!({"type": "EVAL_IN_FRAME", "target": "t", "sender": "s", "requestId": "q", "code": "1"}), &mut f), None);
        assert_eq!(parse(&json!({"type": "EVAL_IN_FRAME", "target": "t", "sender": "s", "requestID": "q"}), &mut f), None);
        assert_eq!(parse(&json!({"type": "REGISTER_FRAME"}), &mut f), None);
        assert_eq!(parse(&json!({"type": "NOPE"}), &mut f), None);
        assert_eq!(parse(&json!("REGISTER_FRAME"), &mut f), None);
        assert_eq!(parse(&json!({"type": "DOM_CHANGED"}), &mut f), Some(Msg::DomChanged));
        let wrapped = eval_wrapper("1+1");
        assert!(wrapped.contains("const raw = 1+1;") && wrapped.contains("JSON.stringify(raw)") && wrapped.contains("return null;"));
        assert_eq!(eval_response_js("q", None), "window[Symbol.for('iTermGraphDiscovery')].handleEvalResponse(\"q\", null);");
        assert_eq!(eval_response_js("q", Some("[1]")), "window[Symbol.for('iTermGraphDiscovery')].handleEvalResponse(\"q\", [1]);");
        assert_eq!(prepare_js("r"), "window[Symbol.for('iTermGraphDiscovery')].prepareForReport(\"r\")");
        assert_eq!(report_js("r"), "window[Symbol.for('iTermGraphDiscovery')].report(\"r\")");
        assert_eq!(REDISCOVER_JS, "window[Symbol.for('iTermGraphDiscovery')].rediscover();");
        assert_eq!(unwrap_result(&json!("[1,2]")), Some("[1,2]".to_string()));
        assert_eq!(unwrap_result(&json!(null)), None);
        assert_eq!(unwrap_result(&json!(5)), None);
        assert_eq!(unwrap_result(&json!("not json {")), None, "only JSON text is ever put into a script");
        // A request id is JSON-escaped into the script.
        assert!(prepare_js("a\"b").contains("\"a\\\"b\""));
    }

    #[test]
    fn report_phases_follow_iterm2() {
        let mut g = Frames::default();
        g.register("m", None);
        g.register("a", Some(7));
        assert_eq!(g.lookup("a"), Some(Some(7)));
        assert_eq!(g.lookup("m"), Some(None));
        assert_eq!(g.lookup("zz"), None);
        g.begin_report("r", vec![1, 2]);
        assert_eq!(g.phase_done(1), None);
        assert_eq!(g.phase_done(2), Some(("r".to_string(), vec![None, Some(7)])));
        assert_eq!(g.phase_done(2), None, "a phase ends once");
        g.gone(7);
        assert_eq!(g.lookup("a"), None);
        assert_eq!(g.lookup("m"), Some(None));
        g.register("a", Some(7));
        g.track_eval(9, Some(7), "q".into());
        assert_eq!(g.take_eval(9), Some((Some(7), "q".to_string())));
        assert_eq!(g.take_eval(9), None);
        g.will_navigate();
        assert_eq!(g.lookup("a"), None);
        assert_eq!(g.lookup("m"), None);
    }

    #[test]
    fn the_graph_is_bounded() {
        let mut g = Frames::default();
        for i in 0..600 {
            g.register(&format!("f{i}"), Some(i));
        }
        assert_eq!(g.lookup("f0"), Some(Some(0)));
        assert_eq!(g.lookup("f599"), None, "past 512 a frame is not registered");
        g.register("f0", Some(5));
        assert_eq!(g.lookup("f0"), Some(Some(0)), "a known id stays with its frame");
    }

    /// The whole flow, with no web view: what each message makes Termurion evaluate, and where.
    #[test]
    fn graph_flow_follows_iterm2() {
        let mut g = Frames::default();
        let mut f = fresh();
        let reg = |id: &str| json!({"type": "REGISTER_FRAME", "frameId": id});
        assert!(g.handle(&routed(World::Client, None, reg("m")), true, &mut f).is_empty());
        assert!(g.handle(&routed(World::Client, Some(7), reg("a")), true, &mut f).is_empty());
        // A report: prepare in every frame, then report in every frame once all prepared.
        let acts = g.handle(&routed(World::Client, None, json!({"type": "REQUEST_REPORT", "requestId": "r"})), true, &mut f);
        assert_eq!(acts.iter().map(|a| a.frame).collect::<Vec<_>>(), vec![None, Some(7)]);
        assert!(acts.iter().all(|a| a.code.contains("prepareForReport(\\\"r\\\")") || a.code.contains("prepareForReport(\"r\")")));
        let (o1, next) = g.answered(acts[0].token, &Ok(json!("null")));
        assert!(o1 && next.is_empty());
        let (o2, next) = g.answered(acts[1].token, &Err("no context".into()));
        assert!(o2, "a failed frame counts as answered");
        assert_eq!(next.iter().map(|a| a.frame).collect::<Vec<_>>(), vec![None, Some(7)]);
        assert!(next.iter().all(|a| a.code.contains("report(")));
        for a in &next {
            let (own, more) = g.answered(a.token, &Ok(json!(null)));
            assert!(own && more.is_empty(), "report results are ignored");
        }
        // An eval: the target runs the wrapped code, the sender gets the answer.
        let eval = json!({"type": "EVAL_IN_FRAME", "target": "a", "sender": "m", "requestID": "q", "code": "document.title"});
        let acts = g.handle(&routed(World::Client, None, eval.clone()), true, &mut f);
        assert_eq!(acts.len(), 1);
        assert_eq!(acts[0].frame, Some(7));
        assert!(acts[0].code.contains("const raw = document.title;"));
        let (own, next) = g.answered(acts[0].token, &Ok(json!("\"T\"")));
        assert!(own && next.len() == 1 && next[0].frame.is_none());
        assert!(next[0].code.contains("handleEvalResponse(\"q\", \"T\")"));
        // An unknown target answers the sender at once with null; an unknown sender is dropped.
        let none = json!({"type": "EVAL_IN_FRAME", "target": "zz", "sender": "a", "requestID": "q2", "code": "1"});
        let acts = g.handle(&routed(World::Client, Some(7), none), true, &mut f);
        assert_eq!(acts.len(), 1);
        assert_eq!(acts[0].frame, Some(7));
        assert!(acts[0].code.contains("handleEvalResponse(\"q2\", null)"));
        let nosender = json!({"type": "EVAL_IN_FRAME", "target": "a", "sender": "zz", "requestID": "q3", "code": "1"});
        assert!(g.handle(&routed(World::Client, None, nosender), true, &mut f).is_empty());
        // DOM_CHANGED rediscovers in the main frame.
        let acts = g.handle(&routed(World::Client, Some(7), json!({"type": "DOM_CHANGED"})), true, &mut f);
        assert_eq!(acts.len(), 1);
        assert!(acts[0].frame.is_none() && acts[0].code.contains("rediscover();"));
        // A script that threw answers the sender null.
        let acts = g.handle(&routed(World::Client, None, eval), true, &mut f);
        let (_, next) = g.answered(acts[0].token, &Err("exception".into()));
        assert!(next[0].code.contains("handleEvalResponse(\"q\", null)"));
        // A navigation ends every wait.
        let acts = g.handle(&routed(World::Client, None, json!({"type": "REQUEST_REPORT"})), true, &mut f);
        g.will_navigate();
        let (_, next) = g.answered(acts[0].token, &Ok(json!(null)));
        assert!(next.is_empty());
    }

    /// 13x3 Task 3: a frame graph built from messages must not let one frame name or take another
    /// frame's id. The frame of a Client message comes from the host, never the body: an id stays
    /// with the frame that registered it, and an evaluation is answered only for the frame whose
    /// message it came in.
    #[test]
    fn graph_ids_cannot_be_spoofed() {
        let mut g = Frames::default();
        let mut f = fresh();
        let reg = |id: &str| json!({"type": "REGISTER_FRAME", "frameId": id});
        g.handle(&routed(World::Client, None, reg("m")), true, &mut f);
        g.handle(&routed(World::Client, Some(7), reg("a")), true, &mut f);
        // Another frame registering a taken id does not move it; the owner may repeat itself.
        g.handle(&routed(World::Client, Some(9), reg("a")), true, &mut f);
        assert_eq!(g.lookup("a"), Some(Some(7)), "a taken id stays with its frame");
        g.handle(&routed(World::Client, Some(9), reg("m")), true, &mut f);
        assert_eq!(g.lookup("m"), Some(None), "the main frame's id cannot be taken");
        g.handle(&routed(World::Client, Some(7), reg("a")), true, &mut f);
        assert_eq!(g.lookup("a"), Some(Some(7)));
        // Frame 9 is unregistered under its own id; it cannot send as "a" or "m".
        let as_sender = |s: &str| json!({"type": "EVAL_IN_FRAME", "target": "m", "sender": s, "requestID": "q", "code": "1"});
        assert!(g.handle(&routed(World::Client, Some(9), as_sender("a")), true, &mut f).is_empty(), "frame 9 is not frame a");
        assert!(g.handle(&routed(World::Client, Some(9), as_sender("m")), true, &mut f).is_empty());
        assert!(g.handle(&routed(World::Client, Some(7), as_sender("m")), true, &mut f).is_empty(), "nor frame 7 the main frame");
        // The real sender still works.
        assert_eq!(g.handle(&routed(World::Client, Some(7), as_sender("a")), true, &mut f).len(), 1);
        // A page-world message cannot take or name an id either.
        g.handle(&routed(World::Page, Some(9), reg("a")), true, &mut f);
        assert_eq!(g.lookup("a"), Some(Some(7)));
        assert!(g.handle(&routed(World::Page, Some(7), as_sender("a")), true, &mut f).is_empty());
        // Ids that look like script are data: they are only compared, and JSON-encoded when composed.
        let evil = "a\"); alert(1); (\"";
        g.handle(&routed(World::Client, Some(9), reg(evil)), true, &mut f);
        let acts = g.handle(&routed(World::Client, Some(9), json!({"type": "EVAL_IN_FRAME", "target": "zz", "sender": evil, "requestID": evil, "code": "1"})), true, &mut f);
        assert_eq!(acts.len(), 1);
        assert!(acts[0].code.contains(&browser_js::js_string(evil)) && !acts[0].code.contains(&format!("({evil}")), "{}", acts[0].code);
    }

    /// R13x23-2, C2: a page-world message, or any message while the world is not ready, makes
    /// Termurion evaluate nothing and register nothing.
    #[test]
    fn graph_refuses_page_world() {
        let mut g = Frames::default();
        let mut f = fresh();
        g.handle(&routed(World::Client, None, json!({"type": "REGISTER_FRAME", "frameId": "m"})), true, &mut f);
        g.handle(&routed(World::Client, Some(3), json!({"type": "REGISTER_FRAME", "frameId": "t"})), true, &mut f);
        let eval = json!({"type": "EVAL_IN_FRAME", "target": "t", "sender": "m", "requestID": "q", "code": "alert(1)"});
        assert!(g.handle(&routed(World::Page, None, eval.clone()), true, &mut f).is_empty(), "a page frame makes nothing run");
        assert!(g.handle(&routed(World::Page, Some(9), json!({"type": "REGISTER_FRAME", "frameId": "evil"})), true, &mut f).is_empty());
        assert_eq!(g.lookup("evil"), None, "a page registers nothing");
        assert!(g.handle(&routed(World::Page, None, json!({"type": "REQUEST_REPORT"})), true, &mut f).is_empty());
        assert!(g.handle(&routed(World::Page, None, json!({"type": "DOM_CHANGED"})), true, &mut f).is_empty());
        assert!(g.handle(&routed(World::Client, None, eval.clone()), false, &mut f).is_empty(), "no world, no evaluation");
        assert_eq!(g.handle(&routed(World::Client, None, eval), true, &mut f).len(), 1);
        // The script itself is the isolated world's, never the page's.
        let s = user_script();
        assert_eq!((s.id, s.world, s.inject, s.main_frame_only), ("GraphDiscovery", World::Client, crate::browser_js::Inject::DocumentStart, false));
        assert_eq!(s.code, std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/resources/browser/graph-discovery.js")).unwrap());
    }
}

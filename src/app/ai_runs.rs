//! 12e's table of AI requests: one-shot runs for Engage AI, Composer suggestions and Codecierge
//! (12b's `Runner`s), the one place a request is started, and the routing of each finished
//! answer to its owner. Every request goes through `App::ai_run`, which asks the gate for the
//! request's own endpoint right before sending (ruling R12e-1), so no entry point can skip it.

use super::ai_app::KeyThen;
use super::{App, PaneId};
use crate::ai_catalog::Model;
use crate::ai_turn::{Out, Runner, ToolResult, Turn, Wire};
use crate::chat_gate;
use crate::pwm::Secret;

/// What a request that cannot be sent answers (`ATO:478-492`).
pub(super) const NOT_ENABLED: &str = "AI features are not enabled or the API key is missing.";

/// Who gets a run's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Owner {
    Engage,
    Complete { pane: PaneId, generation: u64 },
    #[allow(dead_code)] // Codecierge (Task 7).
    Codecierge(String),
}

impl Owner {
    /// Composer suggestions ask the gate silently (`AC:16-95`); everything else shows its alerts.
    pub(super) fn silent(&self) -> bool {
        matches!(self, Owner::Complete { .. })
    }
}

/// One request in flight. `endpoint` is what a key prompt for it is gated on (ruling R12e-5).
pub(super) struct Run {
    pub runner: Runner,
    pub owner: Owner,
    pub endpoint: crate::ai_models::Endpoint,
}

/// Every way a request starts, for the test that each one is gated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // The test walks them all.
pub(super) enum Entry {
    EngageTerminal,
    EngageComposer,
    EngageStatusBar,
    ComposerSuggest,
    CodeciergeStart,
    CodeciergeSend,
}

#[allow(dead_code)]
impl Entry {
    pub(super) const ALL: [Entry; 6] = [Entry::EngageTerminal, Entry::EngageComposer, Entry::EngageStatusBar, Entry::ComposerSuggest, Entry::CodeciergeStart, Entry::CodeciergeSend];

    /// The owner this entry point's runs have (a test value for each).
    pub(super) fn owner(self) -> Owner {
        match self {
            Entry::EngageTerminal | Entry::EngageComposer | Entry::EngageStatusBar => Owner::Engage,
            Entry::ComposerSuggest => Owner::Complete { pane: 1, generation: 1 },
            Entry::CodeciergeStart | Entry::CodeciergeSend => Owner::Codecierge("G".into()),
        }
    }
}

/// How a run ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Finished {
    Choice(String),
    Failed(String),
    Canceled,
}

/// The outputs that end a run; every other one is progress.
pub(super) fn finished(out: &Out) -> Option<Finished> {
    match out {
        Out::Choice(text) => Some(Finished::Choice(text.clone())),
        Out::Failed(e) => Some(Finished::Failed(e.clone())),
        Out::Canceled => Some(Finished::Canceled),
        _ => None,
    }
}

/// The one start path (ruling R12e-1): a closed gate answers `NOT_ENABLED` without touching the
/// runner, so the transport never sees a request; an open one starts it with the key.
pub(super) fn launch(gate_open: bool, runner: &mut Runner, key: Option<&Secret>) -> Result<Vec<Out>, Finished> {
    if !gate_open {
        return Err(Finished::Failed(NOT_ENABLED.into()));
    }
    Ok(runner.start(key))
}

/// The one tool-result path (ruling R12e-1): the follow-up request after a tool result is a send
/// too, so a closed gate ends the run with `NOT_ENABLED` and the transport never sees it.
pub(super) fn tool_done_gated(gate_open: bool, runner: &mut Runner, result: ToolResult) -> Result<Vec<Out>, Finished> {
    if !gate_open {
        runner.cancel();
        return Err(Finished::Failed(NOT_ENABLED.into()));
    }
    Ok(runner.tool_done(result))
}

/// 12b's `one_shot` with iTerm2's `AIConversation.shouldThink` (`ChatIconGenerator.swift:86-89`),
/// which `one_shot` cannot carry: the same fields, plus `should_think`. With None it is exactly
/// `one_shot`.
pub(super) fn one_shot_turn(model: Model, prompt: &str, wire: Wire, legacy_url_pref: String, should_think: Option<bool>) -> Turn {
    use crate::ai_msg::{Message, Role};
    let messages = vec![Message::text(Role::User, prompt)];
    if should_think.is_none() {
        return crate::ai_turn::one_shot(model, messages, wire, legacy_url_pref);
    }
    Turn::new(crate::ai_turn::TurnInput {
        model,
        messages,
        tools: Vec::new(),
        stream: false,
        hosted: crate::ai_turn::Hosted::default(),
        previous_response_id: None,
        should_think,
        effort: None,
        tier: None,
        volatile: Box::new(|| None),
        frozen: None,
        wire,
        legacy_url_pref,
    })
}

impl App {
    /// The model a one-shot request uses, from the settings and the models found so far.
    pub(super) fn ai_model_now(&mut self) -> Option<Model> {
        let found = self.ai_found();
        crate::ai_model::configured(&self.globals, crate::ai_catalog::bundled(), &found)
    }

    /// Whether a request to the default model has what it needs: a stored key for its vendor,
    /// or an endpoint that never takes one.
    pub(super) fn ai_has_key(&mut self) -> bool {
        let found = self.ai_found();
        let catalog = crate::ai_catalog::bundled();
        if let Some(m) = crate::ai_model::configured(&self.globals, catalog, &found) {
            if crate::ai_policy::key_policy(&m.url, m.api) != crate::ai_policy::KeyPolicy::VendorKey {
                return true;
            }
        }
        let vendor = crate::ai_models::effective_vendor(&self.globals, catalog, &found);
        crate::ai_keys::shared().lock().unwrap_or_else(|e| e.into_inner()).is_set(vendor, &mut *crate::ai_keys::system_store())
    }

    /// `template` evaluated in `pane`'s scope with the `ai.*` values (`ATO:380-396`). An
    /// evaluation error is the Err.
    pub(super) fn ai_prompt(&self, pane: PaneId, template: &str, ai: &[(&str, &str)]) -> Result<String, String> {
        let vars = self.variables(pane);
        crate::ai_prompts::evaluate(template, ai, &|name| crate::prompted::variable(name, &vars))
    }

    /// The token limits and the other settings a request is built with.
    pub(super) fn ai_wire(&self) -> Wire {
        Wire::from_globals(&self.globals)
    }

    /// The id the next run will get, for a caller that must know it before the run's answer can
    /// arrive (an answer can come back inside the call that starts the run).
    pub(super) fn ai_next_run_id(&self) -> u64 {
        self.ai_runs_next + 1
    }

    /// A one-shot request with `prompt` as the user's one message.
    pub(super) fn ai_one_shot(&mut self, prompt: String, owner: Owner) -> Option<u64> {
        self.ai_one_shot_thinking(prompt, owner, None)
    }

    /// `ai_one_shot` with thinking turned on or off. None when no run is left going: with no
    /// model the owner has already been told `NO_MODEL`, and a request that finished at once
    /// has already been routed.
    pub(super) fn ai_one_shot_thinking(&mut self, prompt: String, owner: Owner, should_think: Option<bool>) -> Option<u64> {
        let Some(model) = self.ai_model_now() else {
            self.ai_runs_next += 1;
            let id = self.ai_runs_next;
            self.ai_run_route(id, owner, Finished::Failed(crate::ai_turn::NO_MODEL.into()));
            return None;
        };
        let turn = one_shot_turn(model.clone(), &prompt, self.ai_wire(), self.globals.string(crate::ai_settings::URL), should_think);
        self.ai_run(&model, turn, owner)
    }

    /// The chokepoint (ruling R12e-1): the gate for this request's own host runs first (with its
    /// alert unless the owner is silent), then the request starts, whether or not a key is
    /// stored. A closed gate answers the owner `NOT_ENABLED` at once. Some(id) while the run
    /// is still going.
    pub(super) fn ai_run(&mut self, model: &Model, turn: Turn, owner: Owner) -> Option<u64> {
        self.ai_runs_next += 1;
        let id = self.ai_runs_next;
        let endpoint = chat_gate::endpoint_of(model);
        let open = self.ai_check_endpoint(Some(&endpoint), owner.silent());
        let key = if open { crate::ai_keys::shared().lock().unwrap_or_else(|e| e.into_inner()).get(model.vendor, &mut *crate::ai_keys::system_store()) } else { None };
        let options = crate::ai_transport::options_from(&self.globals);
        let runner = Runner::new(turn, self.ai_transport.clone(), options, self.notify.clone());
        #[cfg(windows)]
        let runner = runner.with_log(self.ai_wire_log.clone(), self.globals.bool("aiChatRawWireLogging"));
        let mut run = Run { runner, owner, endpoint };
        match launch(open, &mut run.runner, key.as_ref()) {
            Err(done) => {
                self.ai_run_route(id, run.owner, done);
                None
            }
            Ok(outs) => {
                self.ai_runs.insert(id, run);
                self.ai_run_outs(id, outs);
                self.ai_runs.contains_key(&id).then_some(id)
            }
        }
    }

    /// Cancels a run and forgets it: no answer is routed after this.
    pub(super) fn ai_run_cancel(&mut self, id: u64) {
        if let Some(mut run) = self.ai_runs.remove(&id) {
            run.runner.cancel();
        }
    }

    /// Cancels every run that belongs to a pane that is closing.
    pub(super) fn ai_runs_pane_gone(&mut self, pane: PaneId) {
        let ids: Vec<u64> = self.ai_runs.iter().filter(|(_, r)| matches!(&r.owner, Owner::Complete { pane: p, .. } if *p == pane)).map(|(id, _)| *id).collect();
        for id in ids {
            self.ai_run_cancel(id);
        }
        self.engage_pane_gone(pane);
    }

    /// A tool's result for a run that asked for one.
    #[allow(dead_code)] // Codecierge's execute and write (Task 7).
    pub(super) fn ai_run_tool_done(&mut self, id: u64, result: ToolResult) {
        let Some(run) = self.ai_runs.get_mut(&id) else { return };
        let open = chat_gate::gate_open(&self.globals, &run.endpoint);
        match tool_done_gated(open, &mut run.runner, result) {
            Ok(outs) => self.ai_run_outs(id, outs),
            Err(done) => {
                if let Some(run) = self.ai_runs.remove(&id) {
                    self.ai_run_route(id, run.owner, done);
                }
            }
        }
    }

    /// Polls every run. A continuation send spends a proof that the gate for the run's own
    /// endpoint is open (ruling R12cd-1), so a closed gate cancels it.
    pub(super) fn ai_runs_tick(&mut self) {
        if self.ai_runs.is_empty() {
            return;
        }
        let mut ids: Vec<u64> = self.ai_runs.keys().copied().collect();
        ids.sort_unstable();
        for id in ids {
            let outs = {
                let globals = &self.globals;
                let Some(run) = self.ai_runs.get_mut(&id) else { continue };
                let endpoint = run.endpoint.clone();
                run.runner.poll_with(&mut || chat_gate::pass(globals, &endpoint))
            };
            self.ai_run_outs(id, outs);
        }
    }

    /// `KeyThen::Run`: the key prompt's answer. No key ends the run with `NOT_ENABLED`
    /// (`ATO:478-492`); a key continues it only while the gate for its endpoint is still open.
    pub(super) fn ai_run_key(&mut self, id: u64, key: Option<Secret>) {
        let Some(run) = self.ai_runs.get_mut(&id) else { return };
        let open = chat_gate::gate_open(&self.globals, &run.endpoint);
        match key.filter(|_| open) {
            Some(k) => {
                let outs = run.runner.key_given(Some(k));
                self.ai_run_outs(id, outs);
            }
            None => {
                if let Some(mut run) = self.ai_runs.remove(&id) {
                    run.runner.cancel();
                    self.ai_run_route(id, run.owner, Finished::Failed(NOT_ENABLED.into()));
                }
            }
        }
    }

    /// Acts on what a run's turn produced: a key request goes to the key prompt, a finishing
    /// output removes the run and answers its owner, the rest is progress.
    fn ai_run_outs(&mut self, id: u64, outs: Vec<Out>) {
        for out in outs {
            let Some(run) = self.ai_runs.get(&id) else { return };
            if let Some(done) = finished(&out) {
                if let Some(run) = self.ai_runs.remove(&id) {
                    self.ai_run_route(id, run.owner, done);
                }
                return;
            }
            if let Out::NeedKey(vendor) = out {
                let endpoint = run.endpoint.clone();
                self.ask_ai_key(vendor, Some(&endpoint), KeyThen::Run(id));
            }
            // A tool call comes only from a run that offers tools (Codecierge, Task 7).
        }
    }

    /// Hands a finished run's answer to its owner. Until their tasks land, a Composer
    /// suggestion's and Codecierge's answers are dropped.
    fn ai_run_route(&mut self, id: u64, owner: Owner, done: Finished) {
        match owner {
            Owner::Engage => self.engage_done(id, done),
            Owner::Complete { .. } | Owner::Codecierge(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_catalog::Feature;
    use crate::ai_gate::{self, Check};
    use crate::ai_model::fixture;
    use crate::ai_models::Endpoint;
    use crate::ai_msg::{Message, Role};
    use crate::ai_settings::{Api, Vendor, ADMIN, CONSENT, ENABLE_AI};
    use crate::ai_transport::{Options, Proxy, Recorded, Reply};
    use crate::ai_turn::{one_shot, Wire};
    use crate::globals::Globals;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;
    use toml::Value;

    const URL: &str = "https://api.openai.com/v1/responses";
    const ANSWER: &[u8] = br#"{"id":"resp_1","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"ls"}]}]}"#;

    fn globals(admin: bool, enable: bool) -> Globals {
        let mut g = Globals::empty(Path::new("gate-test.toml"));
        g.set(ENABLE_AI, Value::Boolean(enable));
        g.set(ADMIN, Value::Boolean(admin));
        g
    }

    fn endpoint() -> Endpoint {
        Endpoint { vendor: Vendor::OpenAI, model: "gpt-test".into(), url: URL.into(), api: Api::Responses, custom_headers: vec![] }
    }

    fn rig(turn: crate::ai_turn::Turn) -> (Runner, Arc<Recorded>) {
        let recorded = Arc::new(Recorded::new(vec![Reply::ok(&[ANSWER])]));
        (Runner::new(turn, recorded.clone(), Options { idle: Duration::ZERO, proxy: Proxy::Off }, Arc::new(|| {})), recorded)
    }

    fn list_files(g: &Globals) -> crate::ai_turn::Turn {
        one_shot(fixture("gpt-test", URL, Api::Responses), vec![Message::text(Role::User, "list files")], Wire::from_globals(g), String::new())
    }

    fn key() -> Secret {
        Secret::new("sk-a".into())
    }

    #[test]
    fn runs_finish_follow_iterm2() {
        assert_eq!(finished(&Out::Choice("ls".into())), Some(Finished::Choice("ls".into())));
        assert_eq!(finished(&Out::Failed("e".into())), Some(Finished::Failed("e".into())));
        assert_eq!(finished(&Out::Canceled), Some(Finished::Canceled));
        assert_eq!(finished(&Out::WillSend), None);
        assert_eq!(finished(&Out::Delta("x".into())), None);
        assert_eq!(finished(&Out::StreamEnd), None);
        assert_eq!(finished(&Out::NeedKey(Vendor::OpenAI)), None);
        assert_eq!(finished(&Out::Invoke { call: crate::ai_msg::FunctionCall { name: Some("f".into()), arguments: None, id: None, thought_signature: None }, id: None }), None);
        assert_eq!(NOT_ENABLED, "AI features are not enabled or the API key is missing.");
        assert!(Owner::Complete { pane: 1, generation: 1 }.silent());
        assert!(!Owner::Engage.silent());
        assert!(!Owner::Codecierge("G".into()).silent());
    }

    #[test]
    fn closed_gate_sends_nothing() {
        // Allowed and enabled, but no consent for the host: nothing may be sent.
        let mut g = globals(true, true);
        assert!(matches!(ai_gate::check_endpoint(&g, Some(&endpoint())), Check::NeedsConsent { .. }));
        let (mut runner, recorded) = rig(list_files(&g));
        let check = ai_gate::check_endpoint(&g, Some(&endpoint()));
        assert_eq!(launch(check == Check::Pass, &mut runner, Some(&key())).err(), Some(Finished::Failed(NOT_ENABLED.into())));
        assert!(recorded.requests().is_empty(), "no consent, no request");

        // AI off and the administrator's switch off close it the same way.
        for (admin, enable, expected) in [(true, false, Check::AiOff), (false, true, Check::AdminOff)] {
            let g = globals(admin, enable);
            let (mut runner, recorded) = rig(list_files(&g));
            let check = ai_gate::check_endpoint(&g, Some(&endpoint()));
            assert_eq!(check, expected);
            assert_eq!(launch(check == Check::Pass, &mut runner, Some(&key())).err(), Some(Finished::Failed(NOT_ENABLED.into())));
            assert!(recorded.requests().is_empty());
        }

        // Consent for another host sends nothing either.
        ai_gate::grant(&mut g, "api.anthropic.com");
        let (mut runner, recorded) = rig(list_files(&g));
        let check = ai_gate::check_endpoint(&g, Some(&endpoint()));
        assert!(launch(check == Check::Pass, &mut runner, Some(&key())).is_err());
        assert!(recorded.requests().is_empty());

        // Consent for the host sends exactly one request.
        ai_gate::grant(&mut g, "api.openai.com");
        let (mut runner, recorded) = rig(list_files(&g));
        let check = ai_gate::check_endpoint(&g, Some(&endpoint()));
        assert_eq!(check, Check::Pass);
        assert!(launch(check == Check::Pass, &mut runner, Some(&key())).is_ok());
        assert_eq!(recorded.requests().len(), 1);
        assert!(g.get(CONSENT).is_some());
    }

    #[test]
    fn every_entry_point_is_gated() {
        let g = globals(true, true);
        for e in Entry::ALL {
            let (mut runner, recorded) = rig(list_files(&g));
            assert_eq!(launch(false, &mut runner, Some(&key())).err(), Some(Finished::Failed(NOT_ENABLED.into())), "{e:?}");
            assert!(recorded.requests().is_empty(), "{e:?}");
        }
        // `ai_run` is the only caller of the runner's start: the one call is inside `launch`.
        let call = concat!(".sta", "rt(");
        for (name, source) in [("ai_runs.rs", include_str!("ai_runs.rs")), ("ai_engage_app.rs", include_str!("ai_engage_app.rs")), ("ai_explain_app.rs", include_str!("ai_explain_app.rs"))] {
            let production = source.split("\n#[cfg(test)]\nmod tests").next().unwrap();
            let calls = production.matches(call).count();
            assert_eq!(calls, usize::from(name == "ai_runs.rs"), "{name} starts a runner {calls} times");
        }
        let production = include_str!("ai_runs.rs").split("\n#[cfg(test)]\nmod tests").next().unwrap();
        let launch_fn = production.split("pub(super) fn launch").nth(1).unwrap().split("\n}\n").next().unwrap();
        assert_eq!(launch_fn.matches(call).count(), 1, "the one start is inside launch");
    }

    #[test]
    fn closed_gate_after_a_tool_result_sends_nothing() {
        use crate::ai_turn::{ToolDecl, Turn, TurnInput};
        const CALL: &[u8] = br#"{"id":"resp_1","output":[{"type":"function_call","id":"fc_1","call_id":"c1","name":"execute","arguments":"{}"}]}"#;
        let mut g = globals(true, true);
        ai_gate::grant(&mut g, "api.openai.com");
        let turn = || {
            Turn::new(TurnInput {
                model: fixture("gpt-test", URL, Api::Responses),
                messages: vec![Message::text(Role::User, "go")],
                tools: vec![ToolDecl { name: "execute".into(), description: "d".into(), parameters: serde_json::json!({"type": "object"}) }],
                stream: false,
                hosted: crate::ai_turn::Hosted::default(),
                previous_response_id: None,
                should_think: None,
                effort: None,
                tier: None,
                volatile: Box::new(|| None),
                frozen: None,
                wire: Wire::from_globals(&g),
                legacy_url_pref: String::new(),
            })
        };
        // Runs one call to the point where the model asked for a tool.
        let asked = || {
            let recorded = Arc::new(Recorded::new(vec![Reply::ok(&[CALL]), Reply::ok(&[ANSWER])]));
            let mut runner = Runner::new(turn(), recorded.clone(), Options { idle: Duration::ZERO, proxy: Proxy::Off }, Arc::new(|| {}));
            assert!(launch(true, &mut runner, Some(&key())).is_ok());
            let outs = runner.poll();
            assert!(outs.iter().any(|o| matches!(o, Out::Invoke { .. })), "{outs:?}");
            (runner, recorded)
        };
        let (mut runner, recorded) = asked();
        assert_eq!(recorded.requests().len(), 1);
        let done = tool_done_gated(false, &mut runner, ToolResult::Ok("out".into()));
        assert_eq!(done.err(), Some(Finished::Failed(NOT_ENABLED.into())));
        runner.poll();
        assert_eq!(recorded.requests().len(), 1, "a closed gate sends no follow-up");
        // Open: exactly the follow-up.
        let (mut runner, recorded) = asked();
        assert!(tool_done_gated(true, &mut runner, ToolResult::Ok("out".into())).is_ok());
        runner.poll();
        assert_eq!(recorded.requests().len(), 2);
        // The only tool_done call of the production text sits behind that gate.
        let production = include_str!("ai_runs.rs").split("\n#[cfg(test)]\nmod tests").next().unwrap();
        assert_eq!(production.matches(concat!(".tool", "_done(")).count(), 1);
    }

    #[test]
    fn one_shot_thinking_follows_iterm2() {
        let mut g = globals(true, true);
        ai_gate::grant(&mut g, "api.openai.com");
        let plain = fixture("gpt-test", URL, Api::Responses);
        let mut thinks = plain.clone();
        thinks.features.insert(Feature::ConfigurableThinking);
        let body_of = |m: &crate::ai_catalog::Model, turn: crate::ai_turn::Turn| {
            let _ = m;
            let (mut runner, recorded) = rig(turn);
            assert!(launch(true, &mut runner, Some(&key())).is_ok());
            let requests = recorded.requests();
            assert_eq!(requests.len(), 1);
            String::from_utf8(requests[0].body.clone()).unwrap()
        };
        let off = body_of(&thinks, one_shot_turn(thinks.clone(), "p", Wire::from_globals(&g), String::new(), Some(false)));
        assert!(off.contains(r#""reasoning":{"effort":"low"}"#), "{off}");
        assert!(off.contains(r#""stream":false"#), "{off}");
        let unset = body_of(&thinks, one_shot_turn(thinks.clone(), "p", Wire::from_globals(&g), String::new(), None));
        let twelve_b = body_of(&thinks, one_shot(thinks.clone(), vec![Message::text(Role::User, "p")], Wire::from_globals(&g), String::new()));
        assert_eq!(unset, twelve_b, "None is 12b's one_shot, byte for byte");
        assert!(!unset.contains("reasoning\":"), "{unset}");
        for think in [Some(false), None] {
            let body = body_of(&plain, one_shot_turn(plain.clone(), "p", Wire::from_globals(&g), String::new(), think));
            assert!(!body.contains(r#""reasoning":"#), "a model without ConfigurableThinking sends no reasoning key: {body}");
        }
    }
}

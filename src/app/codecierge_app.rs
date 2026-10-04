//! Codecierge's conversation (`CG:62-370`): the system prompt and the first message at Start, one
//! request at a time, `execute` and `write` answered by the next finished command, the feed of
//! each finished command, the usage warning, and a real stop (ruling R12e-3). Every request goes
//! through `App::ai_run`, which asks the gate for the request's own endpoint right before the
//! send (ruling R12e-1), and every byte typed goes through `user_write(.., Write::Agent)`.

use super::ai_runs::{Finished, Owner};
use super::chat_app::pane_for_ref;
use super::{App, PaneId};
use crate::ai_catalog::Model;
use crate::ai_msg::{Message, Role};
use crate::ai_turn::{ToolResult, Turn, TurnInput, Wire};
use crate::broadcast::Write;
use crate::codecierge::{self as cc, Fed, Invoke, Reaction, Stage, WritePlan};
use crate::toolbelt::ToolId;

/// The turn for one request: the conversation so far and the new text, with `execute` and `write`
/// offered only when commands run automatically (`CG:80-146`).
pub(super) fn request_turn(model: &Model, messages: &[Message], text: &str, auto: bool, wire: Wire, legacy_url_pref: String) -> Turn {
    let mut all = messages.to_vec();
    all.push(Message::text(Role::User, text));
    Turn::new(TurnInput {
        model: model.clone(),
        messages: all,
        tools: if auto { cc::functions() } else { Vec::new() },
        stream: false,
        hosted: crate::ai_turn::Hosted::default(),
        previous_response_id: None,
        should_think: None,
        effort: None,
        tier: None,
        volatile: Box::new(|| None),
        frozen: None,
        wire,
        legacy_url_pref,
    })
}

/// One string argument of a function call.
fn argument(arguments: &str, key: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(arguments).ok()?.get(key)?.as_str().map(str::to_string)
}

impl App {
    /// Whether Codecierge is shown in the current window's toolbelt (`sessionEnabled`,
    /// `CG:628-630`): sessions feed only then.
    fn codecierge_shown(&self) -> bool {
        self.belt.visible && self.belt.split.tools.contains(&ToolId::Codecierge)
    }

    /// Every pane's `codecierge_watch` follows the sessions that are running.
    pub(super) fn sync_codecierge_watch(&mut self) {
        for p in self.panes.values_mut() {
            let on = self.codecierge.get(&p.session_guid).is_some_and(|s| s.stage == Stage::Running);
            if p.own().codecierge_watch != on {
                p.own_mut().codecierge_watch = on;
            }
        }
    }

    /// Start (`CG:62-160`, `CG:455-462`): the checkbox (ticked only after Danger! was answered
    /// OK) with function calling becomes the auto-run flag, the history
    /// clears, the conversation is the system message built from the goal and the context, and
    /// the first message goes.
    pub(super) fn codecierge_start(&mut self, guid: &str) {
        if !self.codecierge.contains_key(guid) {
            return;
        }
        // Nothing of an earlier task survives.
        self.codecierge_stop(guid, Stage::GoalSetting);
        let (shell, uname) = match pane_for_ref(&self.panes, guid) {
            Some(pane) => {
                let vars = self.variables_with(pane, false);
                (vars.session.get("shell").cloned(), vars.session.get("uname").cloned())
            }
            None => (None, None),
        };
        let (_, calling) = self.codecierge_gate();
        let Some(s) = self.codecierge.get_mut(guid) else { return };
        s.begin_auto(calling);
        let key = if s.auto { cc::GHOST_RIDING_KEY } else { cc::REGULAR_KEY };
        let template = self.globals.string(key);
        let s = self.codecierge.get_mut(guid).expect("checked");
        s.messages = vec![Message::text(Role::System, &cc::system_prompt(&template, &s.goal, &cc::context(shell.as_deref(), uname.as_deref())))];
        s.transcript = cc::THINKING.to_string();
        s.stage = Stage::Running;
        s.reply.clear();
        s.commands = 0;
        s.queue.push_back((cc::GET_STARTED_MESSAGE.to_string(), false));
        self.belt.cc_scroll = 0.0;
        self.sync_codecierge_watch();
        self.tools_changed();
        self.redraw_all();
        self.codecierge_pump(guid);
    }

    /// Stops the session's work for real (ruling R12e-3): `Session::stop`, then the pending call
    /// is answered canceled and the run in flight is cancelled, so a late `execute` finds
    /// nothing to act on.
    pub(super) fn codecierge_stop(&mut self, guid: &str, to: Stage) {
        let Some(s) = self.codecierge.get_mut(guid) else { return };
        let stopped = s.stop(to);
        if let Some((id, _)) = stopped.answer {
            self.ai_run_tool_done(id, ToolResult::Canceled(crate::ai_exec::CANCELED_MODEL.to_string()));
        }
        if let Some(id) = stopped.cancel {
            self.ai_run_cancel(id);
        }
        self.sync_codecierge_watch();
        self.tools_changed();
        self.redraw_all();
    }

    /// The closed gate, once per loop pass (ruling R12e-3, `CG:476-480`): every session not
    /// already in onboarding stops there before any polled call is routed. The models are the
    /// ones found so far: no probe on a loop pass (ruling P2).
    pub(super) fn codecierge_gate_tick(&mut self) {
        if self.codecierge.values().all(|s| s.stage == Stage::Onboarding) {
            return;
        }
        let (open, _) = self.codecierge_gate();
        for guid in cc::to_stop(self.codecierge.values(), open) {
            self.codecierge_stop(&guid, Stage::Onboarding);
        }
    }

    /// A reply from the person: queued, sent when no request is in flight (`CG:268-340`).
    pub(super) fn codecierge_send(&mut self, guid: &str, text: String) {
        let Some(s) = self.codecierge.get_mut(guid) else { return };
        if s.stage != Stage::Running {
            return;
        }
        s.queue.push_back((text, true));
        self.codecierge_pump(guid);
    }

    /// Sends the next queued text when nothing is in flight. The id the run will get is known
    /// first, because a closed gate answers inside `ai_run`.
    fn codecierge_pump(&mut self, guid: &str) {
        let Some(s) = self.codecierge.get(guid) else { return };
        if s.stage != Stage::Running || s.request.is_some() || s.queue.is_empty() {
            return;
        }
        let (auto, messages) = (s.auto, s.messages.clone());
        let model = self.ai_model_now();
        let id = self.ai_next_run_id();
        let Some(s) = self.codecierge.get_mut(guid) else { return };
        let Some((text, append)) = s.queue.pop_front() else { return };
        s.request = Some(id);
        s.busy = true;
        s.sent = Some((text.clone(), append));
        let Some(model) = model else {
            return self.codecierge_done(guid, id, Finished::Failed(crate::ai_turn::NO_MODEL.to_string()));
        };
        let turn = request_turn(&model, &messages, &text, auto, self.ai_wire(), self.globals.string(crate::ai_settings::URL));
        self.ai_run(&model, turn, Owner::Codecierge(guid.to_string()));
        self.tools_changed();
        self.redraw_all();
    }

    /// A run ended (`CG:283-311`): an answer goes into the conversation and the transcript (a
    /// feed or the first message replaces it, a reply appends); a refusal of the gate stops the
    /// session in onboarding; any other failure shows `error_text` the same way. Then the next
    /// queued text goes. A run that is no longer the session's request is dropped.
    pub(super) fn codecierge_done(&mut self, guid: &str, id: u64, finished: Finished) {
        let Some(s) = self.codecierge.get_mut(guid) else { return };
        if s.request != Some(id) {
            return;
        }
        s.request = None;
        s.busy = false;
        if s.pending.as_ref().is_some_and(|(p, _)| *p == id) {
            s.pending = None;
            s.pending_done = false;
        }
        let sent = s.sent.take();
        let append = sent.as_ref().is_some_and(|(_, a)| *a);
        let show = |s: &mut cc::Session, text: &str| {
            if append {
                s.transcript.push_str(&cc::assistant_more(text));
            } else {
                s.transcript = cc::assistant(text);
            }
        };
        match finished {
            Finished::Canceled => {}
            Finished::Choice(text) => {
                if let Some((user, _)) = sent {
                    if !user.is_empty() {
                        s.messages.push(Message::text(Role::User, &user));
                    }
                    s.messages.push(Message::text(Role::Assistant, &text));
                }
                show(s, &text);
            }
            Finished::Failed(e) => match cc::done_reaction(&e) {
                Reaction::StopToOnboarding => return self.codecierge_stop(guid, Stage::Onboarding),
                Reaction::ShowError(t) => show(s, &t),
            },
        }
        if self.belt.cc_guid.as_deref() == Some(guid) {
            self.belt.cc_scroll = 0.0;
        }
        self.tools_changed();
        self.redraw_all();
        self.codecierge_pump(guid);
    }

    /// What a `write` knows of the pane's shell (final residual ruling): `ai_shell_of`'s kind
    /// and the detector's answer, the launch program, the job tree, and whether a framing
    /// conductor's chain was vouched for by Tandem's origin token.
    fn codecierge_write_shell(&self, pane: PaneId) -> cc::WriteShell {
        let (kind, host_is_local, _) = self.ai_shell_of(pane);
        let name = self.variables_with(pane, false).session.get("shell").cloned();
        let Some(p) = self.panes.get(&pane) else { return cc::WriteShell::unknown() };
        let s = p.own();
        let launch = s.shell.job_root.clone().or_else(|| Some(crate::proctree::argv0_title(&p.command).to_string()).filter(|n| !n.is_empty()));
        let link = match (crate::conductor::level(p.conductor.as_deref()), p.conductor.as_deref()) {
            (0, _) | (_, None) => cc::Link::None,
            (2, Some(c)) => cc::Link::Framing { vouched: c.root_is_local() },
            _ => cc::Link::Unframed,
        };
        cc::WriteShell { kind, name, launch, host_is_local, ancestors: s.shell.job_ancestors.clone(), link }
    }

    /// The model called a function of run `id` (`CG:80-146`). Nothing at all happens for a call
    /// that `invoke_plan` drops: after End Task, Stop or a closed gate, or for a run that is no
    /// longer the session's request (ruling R12e-3).
    pub(super) fn codecierge_invoke(&mut self, guid: &str, id: u64, call: crate::ai_msg::Message) {
        let (name, arguments) = call.function_call().map(|(c, _)| (c.name.clone().unwrap_or_default(), c.arguments.clone().unwrap_or_default())).unwrap_or_default();
        let (name, arguments) = (name.as_str(), arguments.as_str());
        if let Some(s) = self.codecierge.get_mut(guid) {
            if s.request == Some(id) {
                s.call = Some(call);
            }
        }
        let Some(s) = self.codecierge.get(guid) else { return };
        let (open, _) = self.codecierge_gate();
        let Some(pane) = pane_for_ref(&self.panes, guid) else {
            // The session is gone: only a call that would have acted is answered.
            let would = cc::invoke_plan(s, id, open, name, arguments, &cc::WriteShell::unknown());
            if would != Invoke::Drop {
                self.ai_run_tool_done(id, ToolResult::Failed(cc::SESSION_GONE.to_string()));
            }
            return;
        };
        // The one shell detector decides the kind; a write types only where Tandem knows a
        // POSIX shell reads it (final residual ruling).
        let shell = self.codecierge_write_shell(pane);
        let Some(s) = self.codecierge.get(guid) else { return };
        let plan = cc::invoke_plan(s, id, open, name, arguments, &shell);
        match plan {
            Invoke::Drop => {}
            Invoke::Answer(text) => self.codecierge_answer_now(guid, id, ToolResult::Failed(text)),
            Invoke::Type(parts) => {
                let command = argument(arguments, "command").unwrap_or_default();
                self.codecierge_note(guid, &cc::running(&command));
                for part in &parts {
                    self.codecierge_type(pane, part);
                }
                self.codecierge_wait(guid, id, name, false);
            }
            Invoke::Write(plan) => {
                let filename = argument(arguments, "filename").unwrap_or_default();
                match plan {
                    WritePlan::Refuse(text) => self.codecierge_answer_now(guid, id, ToolResult::Failed(text)),
                    WritePlan::Typed(parts) => {
                        self.codecierge_note(guid, &cc::writing(&filename));
                        for part in &parts {
                            self.codecierge_type(pane, part);
                        }
                        self.codecierge_wait(guid, id, name, true);
                    }
                }
            }
        }
    }

    /// A call answered at once: the exchange joins the conversation, then the answer goes.
    fn codecierge_answer_now(&mut self, guid: &str, id: u64, result: ToolResult) {
        let text = match &result {
            ToolResult::Ok(t) | ToolResult::Failed(t) | ToolResult::Canceled(t) => t.clone(),
        };
        if let Some(s) = self.codecierge.get_mut(guid) {
            s.answer_call(&text);
        }
        self.ai_run_tool_done(id, result);
    }

    /// Text the transcript gains as an appended section.
    fn codecierge_note(&mut self, guid: &str, text: &str) {
        if let Some(s) = self.codecierge.get_mut(guid) {
            s.transcript.push_str(&format!("\n\n{text}"));
        }
        self.tools_changed();
        self.redraw_all();
    }

    /// The call waits for the next finished command.
    fn codecierge_wait(&mut self, guid: &str, id: u64, name: &str, done: bool) {
        if let Some(s) = self.codecierge.get_mut(guid) {
            s.pending = Some((id, name.to_string()));
            s.pending_done = done;
        }
    }

    /// Types one part at the pane as the agent: never into broadcast siblings, no paste warnings
    /// (ruling R12e-4). Text left on the prompt line is staged as 12c's typing stages it, and a
    /// part that ends in Enter submits it (ruling P15).
    fn codecierge_type(&mut self, pane: PaneId, part: &str) {
        self.user_write(pane, part.as_bytes(), Write::Agent);
        if part.contains(['\r', '\n']) {
            self.ai_staged.remove(&pane);
        } else if !part.chars().any(char::is_control) {
            let prompts = self.panes.get(&pane).map_or(0, |p| p.own().prompts_seen());
            self.ai_staged.entry(pane).or_default().push(prompts, part);
        }
    }

    /// A command finished in `pane` (`CG:183-266`), only while the tool is shown in the pane's
    /// window. A pending call is answered with the feed, else the feed is queued; each command
    /// counts toward the usage warning while the session runs.
    pub(super) fn codecierge_command_exited(&mut self, pane: PaneId, mark: Option<u32>, code: i32) {
        let Some(guid) = self.panes.get(&pane).map(|p| p.session_guid.clone()) else { return };
        if !self.codecierge.get(&guid).is_some_and(|s| s.stage == Stage::Running) || !self.codecierge_shown() {
            return;
        }
        let Some(f) = self.panes.get(&pane).and_then(|p| p.own().finished_command(mark)) else { return };
        let fed = Fed { user: f.user, host: f.host, directory: f.directory, command: f.command, output: f.output.unwrap_or_default().trim_end().to_string(), code };
        let limit = self.globals.int(cc::WARNING_COUNT_KEY);
        let Some(s) = self.codecierge.get_mut(&guid) else { return };
        s.transcript.push_str(&cc::terminal(&fed.command));
        s.transcript.push_str(cc::ASSISTANT_THINKING);
        s.commands += 1;
        let due = cc::usage_due(s.commands, limit);
        if due {
            s.commands = 0;
        }
        let text = cc::feed(&fed, s.auto);
        let mut answer = None;
        match s.pending.take() {
            Some((id, _)) => {
                // The call's answer is the feed, or "Done" for a typed write.
                let text = if s.pending_done { "Done".to_string() } else { text };
                s.answer_call(&text);
                answer = Some((id, text));
                s.pending_done = false;
            }
            None => s.queue.push_back((text, false)),
        }
        self.tools_changed();
        self.redraw_all();
        match answer {
            Some((id, text)) => self.ai_run_tool_done(id, ToolResult::Ok(text)),
            None => self.codecierge_pump(&guid),
        }
        if due {
            // Last: a remembered answer acts at once and may stop the session.
            self.ask_ai(crate::ai_gate::Alert {
                heading: cc::USAGE_HEADING.to_string(),
                text: cc::USAGE_TEXT.to_string(),
                buttons: vec![cc::KEEP_GOING, cc::STOP],
                silence: Some(cc::USAGE_ID.to_string()),
                then: crate::ai_gate::Then::CodeciergeUsage(guid),
                gate: false,
                close: None,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_gate::{self, Check};
    use crate::ai_model::fixture;
    use crate::ai_models::Endpoint;
    use crate::ai_settings::{Api, Vendor, ADMIN, ENABLE_AI};
    use crate::ai_transport::{Options, Proxy, Recorded, Reply};
    use crate::app::ai_runs::{launch, NOT_ENABLED};
    use crate::globals::Globals;
    use crate::ai_turn::Runner;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;
    use toml::Value;

    const URL: &str = "https://api.openai.com/v1/responses";
    const ANSWER: &[u8] = br#"{"id":"resp_1","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"ls"}]}]}"#;

    fn endpoint() -> Endpoint {
        Endpoint { vendor: Vendor::OpenAI, model: "gpt-test".into(), url: URL.into(), api: Api::Responses, custom_headers: vec![] }
    }

    fn globals() -> Globals {
        let mut g = Globals::empty(Path::new("cc-app-test.toml"));
        g.set(ENABLE_AI, Value::Boolean(true));
        g.set(ADMIN, Value::Boolean(true));
        g
    }

    fn rig(g: &Globals, auto: bool) -> (Runner, Arc<Recorded>) {
        let model = fixture("gpt-test", URL, Api::Responses);
        let system = vec![Message::text(Role::System, "sys")];
        let turn = request_turn(&model, &system, cc::GET_STARTED_MESSAGE, auto, Wire::from_globals(g), String::new());
        let recorded = Arc::new(Recorded::new(vec![Reply::ok(&[ANSWER])]));
        (Runner::new(turn, recorded.clone(), Options { idle: Duration::ZERO, proxy: Proxy::Off }, Arc::new(|| {})), recorded)
    }

    #[test]
    fn codecierge_sends_are_gated() {
        // The first message, a reply and the follow-up of a feed are all built by `request_turn`
        // and started through `launch`: a closed gate sends no byte and the session stops in
        // onboarding instead of showing an error.
        let mut g = globals();
        for auto in [true, false] {
            let (mut runner, recorded) = rig(&g, auto);
            let check = ai_gate::check_endpoint(&g, Some(&endpoint()));
            assert!(matches!(check, Check::NeedsConsent { .. }));
            let refused = launch(check == Check::Pass, &mut runner, Some(&crate::pwm::Secret::new("sk-a".into()))).err();
            assert_eq!(refused, Some(Finished::Failed(NOT_ENABLED.into())));
            assert!(recorded.requests().is_empty(), "no consent, no request (auto {auto})");
            assert_eq!(cc::done_reaction(NOT_ENABLED), Reaction::StopToOnboarding);
        }
        // With consent exactly one request goes, and it offers the functions only when auto.
        ai_gate::grant(&mut g, "api.openai.com");
        for (auto, offers) in [(true, true), (false, false)] {
            let (mut runner, recorded) = rig(&g, auto);
            let check = ai_gate::check_endpoint(&g, Some(&endpoint()));
            assert!(launch(check == Check::Pass, &mut runner, Some(&crate::pwm::Secret::new("sk-a".into()))).is_ok());
            let sent = recorded.requests();
            assert_eq!(sent.len(), 1);
            let body = String::from_utf8_lossy(&sent[0].body).to_string();
            assert_eq!(body.contains("\"execute\""), offers, "{body}");
            assert!(body.contains(cc::GET_STARTED_MESSAGE));
        }
        // The source: no start, key, tool-result or runner of its own, one `ai_run` for requests.
        let production = include_str!("codecierge_app.rs").split("\n#[cfg(test)]\nmod tests").next().unwrap();
        for forbidden in [concat!(".sta", "rt("), concat!("Runner", "::new"), concat!(".tool_", "done("), concat!("key_", "given(")] {
            assert_eq!(production.matches(forbidden).count(), 0, "{forbidden}");
        }
        assert_eq!(production.matches("self.ai_run(").count(), 1);
    }
}

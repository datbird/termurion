//! Orchestration's dispatcher on the App side (12f Task 10, `OrchestratorDispatcher.swift`):
//! every orchestration tool call and `session_*` mirror, the per-chat claims, the one typed-text
//! chokepoint (`orch_type_gated`, rulings R12f-1 and R12f-2), the safety checks (ruling P6) and
//! everything parked on the person. Every tool call is the model's untrusted output: a session
//! is named only by its stable id (ruling P3), typing reaches only a live, shown pane (ruling
//! P8), and whatever waits dies with the chat's turn (ruling P7, `orch_chat_ended`).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::App;
use crate::ai_catalog::Model;
use crate::ai_classifier::{self, Entry};
use crate::ai_models::Endpoint;
use crate::ai_msg::{Message as WireMessage, Role};
use crate::ai_shell::ShellKind;
use crate::ai_tools::Call;
use crate::ai_transport::{Notify, Options, Transport};
use crate::ai_turn::{self, Runner, ToolResult, Wire};
use crate::broadcast::Write;
use crate::chat_gate::Passed;
use crate::chat_message::{ClientLocal, Content, Message, Participant};
use crate::layout::PaneId;
use crate::orch::{self, OrchLocal, Outcome};
use crate::orch_gate::{self, Plan, Route, TypedState, Typing, Verdict};
use crate::orch_tools::{self as ot, OrchError};
use crate::pwm::Secret;

/// The pause between a bracketed paste and its CR (`DI:2869-2871`).
pub(super) const CR_DELAY: Duration = Duration::from_millis(75);
/// A mirror's limit (`DI:1365-1386`).
pub(super) const MIRROR_TIMEOUT: Duration = Duration::from_secs(30);
/// Ruling P15's spawn cap: per chat, per minute and alive at once.
pub(super) const SPAWNS_PER_MINUTE: usize = 4;
pub(super) const SPAWNS_LIVE: usize = 16;
/// The most wheel notches one scroll_wheel sends (Tandem's bound on a model's number).
pub(super) const MAX_NOTCHES: i64 = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CheckKind {
    Command,
    FileWrite,
    Keystroke,
}

/// Whether a classifier's block is a "potentially dangerous" deny: not for a keystroke into a
/// TUI, whose holds are often only unclear screens (`OSG:147-156`).
pub(super) fn block_is_deny(kind: CheckKind) -> bool {
    kind != CheckKind::Keystroke
}

/// What one typed send carries through its check (rulings R12f-1, R12f-2): what the plan saw,
/// so the race guard can compare after the verdict.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct TypedCheck {
    pub pane: PaneId,
    /// The model's reference, for its answers.
    pub guid: String,
    pub prior: TypedState,
    pub plan: Plan,
    pub text: String,
    pub append_newline: bool,
}

/// What runs once a call is allowed, by the gate or by the person.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Then {
    /// send_text's typing.
    SendText(TypedCheck),
    /// session_execute_command's typing, through 12c's dispatcher.
    Execute { typed: TypedCheck, command: String },
    Spawn(ot::StartSession),
    /// A mirror that runs as it is once allowed (create_file, restart_session).
    Mirror { pane: PaneId, guid: String, call: Call },
}

impl Then {
    /// A mirror answers plain text, the others the result envelope.
    fn mirror(&self) -> bool {
        matches!(self, Then::Execute { .. } | Then::Mirror { .. })
    }
}

/// What a check judges.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Proposed {
    Line { line: String, shell: ShellKind },
    File { path: String, content: String },
    Keystroke { keystroke: String, screen: String },
}

/// A check's progress.
pub(super) enum Run {
    Running(Runner, Endpoint),
    Done(Verdict),
}

/// One running safety check (ruling P6: orchestration's own runner, which keeps the verdict).
pub(super) struct Check {
    pub chat: String,
    pub call_id: String,
    pub kind: CheckKind,
    pub then: Then,
    /// What the approval bubble says the agent wants.
    pub summary: String,
    /// One per form judged (a command is judged alone and joined with its held text).
    pub runs: Vec<Run>,
}

/// The verdict of a check once every form's run is done: the worst of them (any refusal
/// refuses); None while one still runs.
pub(super) fn runs_verdict(runs: &[Run]) -> Option<Verdict> {
    let mut vs = Vec::new();
    for r in runs {
        match r {
            Run::Done(v) => vs.push(v.clone()),
            Run::Running(..) => return None,
        }
    }
    Some(orch_gate::worst(&vs))
}

/// A call parked on the person, by request id (`orch_parks` maps the id to the chat and call).
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Wait {
    /// A claim; once granted the call is dispatched again from the start.
    Claim { scope: String, name: String, args: Value, mirror_guid: Option<String> },
    /// A verdict that was not an allow, or a call that always asks.
    Approve { then: Then, reason: String, denied: bool },
    /// A session-bound register_watch waiting for the one-time screen-read consent; once
    /// approved it is registered again from the start with every permission re-read.
    WatchConsent { args: Value },
}

/// A CR due after a bracketed paste; the call is answered once it is sent.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PendingCr {
    pub due: Instant,
    pub pane: PaneId,
    pub chat: String,
    pub call_id: String,
}

/// A start_session allowed and waiting for the event loop (a window needs it).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Spawn {
    pub chat: String,
    pub call_id: String,
    pub args: ot::StartSession,
}

/// What can stop a pane from taking the model's typing (ruling P8).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct PaneState {
    pub exited: bool,
    pub buried: bool,
    pub password: bool,
    pub gateway: bool,
}

/// Typing goes only into a live pane in a window, never at a password prompt or into a tmux
/// gateway (ruling P8).
pub(super) fn typeable(guid: &str, s: PaneState) -> Result<(), OrchError> {
    if s.exited {
        return Err(OrchError::TargetGone(guid.to_string()));
    }
    let why = if s.buried {
        ot::BURIED
    } else if s.password {
        ot::AT_PASSWORD
    } else if s.gateway {
        ot::GATEWAY
    } else {
        return Ok(());
    };
    Err(OrchError::Unsupported(ot::not_typeable(guid, why)))
}

/// The pane a model's reference names: a canonical stable id of a pane, nothing else (ruling P3:
/// a decimal pane id or 12d's per-launch uuid names no session).
pub(super) fn resolve<'a>(reference: &str, panes: impl IntoIterator<Item = (PaneId, &'a str)>) -> Result<PaneId, OrchError> {
    crate::stable_id::find(reference, panes).ok_or_else(|| OrchError::UnknownSession(reference.to_string()))
}

/// The CRs due at `now`, taken from the queue.
pub(super) fn take_due(crs: &mut Vec<PendingCr>, now: Instant) -> Vec<PendingCr> {
    let (due, rest): (Vec<PendingCr>, Vec<PendingCr>) = crs.drain(..).partition(|c| c.due <= now);
    *crs = rest;
    due
}

/// A start_session's folder: a local absolute folder, else none (the profile's); a network path
/// is refused and never probed (ruling P15).
pub(super) fn spawn_cwd(cwd: Option<&str>) -> Result<Option<String>, OrchError> {
    let Some(c) = cwd.map(str::trim).filter(|c| !c.is_empty()) else { return Ok(None) };
    let sep = |ch: char| ch == '\\' || ch == '/';
    let mut two = c.chars();
    if two.next().is_some_and(sep) && two.next().is_some_and(sep) {
        return Err(OrchError::Unsupported(ot::NETWORK_CWD.to_string()));
    }
    let b = c.as_bytes();
    let drive = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && sep(char::from(b[2]));
    Ok(drive.then(|| c.to_string()))
}

/// What a start_session does (ruling P15): refuse it, start it at once (nothing differs from
/// the defaults), or ask the person, after a check of `command` when there is one. The
/// command is the model's, else the one the chosen profile runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SpawnStep {
    Refuse(OrchError),
    Start,
    Ask { command: Option<String> },
}

pub(super) fn spawn_step(args: &ot::StartSession, profile_found: bool, profile_command: Option<String>, recent: usize, live: usize) -> SpawnStep {
    if let Some(p) = args.profile.as_deref().filter(|_| !profile_found) {
        return SpawnStep::Refuse(OrchError::Malformed(ot::no_profile(p)));
    }
    if let Err(e) = spawn_cwd(args.cwd.as_deref()) {
        return SpawnStep::Refuse(e);
    }
    if recent >= SPAWNS_PER_MINUTE || live >= SPAWNS_LIVE {
        return SpawnStep::Refuse(OrchError::Unsupported(ot::TOO_MANY_SPAWNS.to_string()));
    }
    let command = args.command.clone().or(profile_command);
    if command.is_none() && args.profile.is_none() && args.cwd.is_none() {
        return SpawnStep::Start;
    }
    SpawnStep::Ask { command }
}

/// A mirror's first steps, in iTerm2's order (`DI:1226-1290`): the plain-text answer of a call
/// that goes no further, else (raw name, the parsed call, the model's session reference).
pub(super) fn mirror_call(name: &str, args: &Value) -> Result<(String, Call, String), String> {
    let raw = name.strip_prefix("session_").unwrap_or(name);
    if !crate::ai_tools::ALL_NAMES.contains(&raw) {
        return Err(ot::unknown_session_tool(name));
    }
    match raw {
        "insert_text_at_cursor" | "delete_current_line" => return Err(ot::not_in_orchestration(name)),
        "get_screen_contents" => return Err(ot::use_workgroup_screen(name)),
        _ => {}
    }
    let Some(obj) = args.as_object() else { return Err(ot::ARGS_NOT_OBJECT.to_string()) };
    let Some(guid) = obj.get("session_guid").and_then(Value::as_str) else { return Err(ot::MISSING_GUID.to_string()) };
    let mut rest = obj.clone();
    rest.remove("session_guid");
    let call = crate::ai_tools::parse(raw, &Value::Object(rest)).map_err(|e| ot::args_error(&e))?;
    Ok((raw.to_string(), call, guid.to_string()))
}

/// The verdict of a check whose model cannot run (ruling P6, fail closed); None when it runs.
pub(super) fn backend_verdict(b: &super::ai_safety_app::Backend) -> Option<Verdict> {
    use super::ai_safety_app::Backend;
    match b {
        Backend::Run(..) => None,
        Backend::Refused(_) => Some(Verdict::RequireApproval(orch_gate::CHECK_FAILED.to_string())),
        Backend::NoModel => Some(Verdict::RequireApproval(ai_classifier::NO_MODEL.to_string())),
    }
}

/// A model's reply as a verdict; any failure asks the person.
pub(super) fn reply_verdict(reply: Result<String, String>, deny: bool) -> Verdict {
    match reply {
        Ok(text) => orch_gate::verdict_of(&ai_classifier::parse(&text), deny),
        Err(_) => Verdict::RequireApproval(orch_gate::CHECK_FAILED.to_string()),
    }
}

/// The two messages of a check (`classifyAction`, `classifyTUIKeystroke`).
pub(super) fn check_messages(entries: &[Entry], proposed: &Proposed, sentinel: &str) -> Vec<WireMessage> {
    match proposed {
        Proposed::Line { line, .. } => super::ai_safety_app::messages_with(entries, &orch_gate::command_entry(line)),
        Proposed::File { path, content } => super::ai_safety_app::messages_with(entries, &orch_gate::file_write_entry(path, content)),
        Proposed::Keystroke { keystroke, screen } => {
            let recent = super::ai_safety_app::recent_entries(entries);
            vec![WireMessage::text(Role::System, &orch_gate::tui_system_prompt()), WireMessage::text(Role::User, &orch_gate::tui_user_message(&recent, screen, keystroke, sentinel))]
        }
    }
}

/// Starts a check's one-shot (12c's runner, the reply kept as a verdict). The first send spends
/// `_pass`, the proof the backend took (ruling R12cd-1).
#[allow(clippy::too_many_arguments)]
pub(super) fn run_start(model: Model, endpoint: Endpoint, _pass: Passed, msgs: Vec<WireMessage>, wire: Wire, transport: Arc<dyn Transport>, options: Options, notify: Notify, key: Option<&Secret>, deny: bool) -> Run {
    let wire = Wire { response_limit: i64::from(ai_classifier::HISTORY_RESPONSE_ROOM), ..wire };
    let mut runner = Runner::new(ai_turn::one_shot(model, msgs, wire, String::new()), transport, options, notify);
    let outs = runner.start(key);
    match super::ai_safety_app::reply_of(&outs) {
        Some(reply) => Run::Done(reply_verdict(reply, deny)),
        None => Run::Running(runner, endpoint),
    }
}

/// One step of a running check: the gate first (a closed gate cancels the request and the
/// check asks the person, ruling R12cd-1), then the transport.
pub(super) fn run_advance(run: Run, mut pass: impl FnMut(&Endpoint) -> Option<Passed>, deny: bool) -> Run {
    let Run::Running(mut runner, endpoint) = run else { return run };
    if pass(&endpoint).is_none() {
        runner.cancel();
        return Run::Done(Verdict::RequireApproval(orch_gate::CHECK_FAILED.to_string()));
    }
    match super::ai_safety_app::reply_of(&runner.poll_with(&mut || pass(&endpoint))) {
        Some(reply) => Run::Done(reply_verdict(reply, deny)),
        None => Run::Running(runner, endpoint),
    }
}

/// A fresh sentinel for the screen's fence (`makeSentinel`): chosen after the program wrote
/// the screen, so it cannot be forged.
fn sentinel() -> String {
    let mut b = [0u8; 16];
    let _ = getrandom::fill(&mut b);
    format!("SCREEN-{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>())
}

fn done(text: String) -> Outcome {
    Outcome::Done(text)
}

/// What the typed chokepoint runs after its verdict.
pub(super) enum After {
    SendText,
    Execute(String),
}

impl App {
    /// What may stop a pane from taking typing now.
    pub(super) fn orch_pane_state(&self, pane: PaneId) -> PaneState {
        let p = self.panes.get(&pane);
        PaneState {
            exited: p.is_none_or(|p| p.exited.is_some()),
            buried: self.buried.contains(pane),
            password: p.is_some_and(|p| p.password_input || p.ssh_password_prompt()),
            gateway: p.is_some_and(|p| p.gateway),
        }
    }

    /// A session's name as the person reads it, else its stable id.
    pub(super) fn orch_name(&self, pane: PaneId) -> String {
        let n = crate::inert_files::shown_name(&self.session_label(pane).0);
        if n.is_empty() { self.panes.get(&pane).map(|p| p.stable_id.clone()).unwrap_or_default() } else { n }
    }

    pub(super) fn orch_resolve(&self, guid: &str) -> Result<PaneId, OrchError> {
        resolve(guid, self.panes.iter().map(|(k, p)| (*k, p.stable_id.as_str())))
    }

    pub(super) fn orch_stable(&self, pane: PaneId) -> String {
        self.panes.get(&pane).map(|p| p.stable_id.clone()).unwrap_or_default()
    }

    /// One orchestration tool call: an answer now, an answer later (`Running`), or parked on the
    /// person. A chat that left orchestration mode answers every tool with the mode's error.
    pub(super) fn orch_dispatch(&mut self, chat_id: &str, call_id: &str, name: &str, args: &Value) -> Outcome {
        if !self.chat_row(chat_id).is_some_and(|c| c.orchestration_enabled) {
            return done(ot::error_json(&OrchError::Unsupported(ot::LEFT_ORCHESTRATION.to_string())));
        }
        if name.starts_with("session_") {
            return self.orch_mirror(chat_id, call_id, name, args);
        }
        match self.orch_tool(chat_id, call_id, name, args) {
            Ok(o) => o,
            Err(e) => done(ot::error_json(&e)),
        }
    }

    fn orch_tool(&mut self, chat: &str, call_id: &str, name: &str, args: &Value) -> Result<Outcome, OrchError> {
        match name {
            "list_workgroups" => {
                let groups = self.orch_groups();
                Ok(done(ot::result_json("workgroups", serde_json::to_value(groups).unwrap_or_default())))
            }
            "get_state" => {
                let pane = self.orch_resolve(&ot::parse_guid(args)?)?;
                Ok(done(ot::result_json("session_state", self.orch_state_value(pane))))
            }
            "get_screen_contents" => {
                let a = ot::parse_get_screen_contents(args)?;
                let pane = self.orch_resolve(&a.guid)?;
                let v = self.orch_screen_value(pane, a.lines);
                Ok(done(orch::model_json(&v)))
            }
            "scroll_wheel" => {
                let a = ot::parse_scroll_wheel(args)?;
                let pane = self.orch_resolve(&a.guid)?;
                typeable(&a.guid, self.orch_pane_state(pane))?;
                self.orch_scroll(pane, &a)?;
                Ok(done(ot::ack_json()))
            }
            "list_workgroup_clippings" => {
                let a = ot::parse_list_workgroup_clippings(args)?;
                let (_, home, _) = self.orch_clippings_home(&a.workgroup_id)?;
                let list = self.clips.get(&home).map(|c| c.model.live.clone()).unwrap_or_default();
                Ok(done(ot::result_json("clippings", ot::clippings_value(&list, a.type_filter.as_deref()))))
            }
            "send_text" => {
                let s = ot::parse_send_text(args)?;
                let pane = self.orch_resolve(&s.guid)?;
                typeable(&s.guid, self.orch_pane_state(pane))?;
                let text = ot::decode_escapes(&s.text)?;
                let scope = self.claim_scope_of(pane);
                if let Some(o) = self.orch_claim(chat, call_id, &scope, name, args, None) {
                    return Ok(o);
                }
                Ok(self.orch_type_gated(chat, call_id, pane, &s.guid, &text, s.newline, After::SendText))
            }
            "interrupt" => {
                let guid = ot::parse_guid(args)?;
                let pane = self.orch_resolve(&guid)?;
                typeable(&guid, self.orch_pane_state(pane))?;
                let scope = self.claim_scope_of(pane);
                if let Some(o) = self.orch_claim(chat, call_id, &scope, name, args, None) {
                    return Ok(o);
                }
                // Ctrl-C clears the line: the accumulator and its contamination go (`DI:2913-2918`).
                self.ai_staged.remove(&pane);
                self.orch_held.remove(&pane);
                self.enter_pane_window(pane);
                self.user_write(pane, b"\x03", Write::Agent);
                Ok(done(ot::ack_json()))
            }
            "add_workgroup_clipping" => {
                let a = ot::parse_add_workgroup_clipping(args)?;
                let (target, home, scope) = self.orch_clippings_home(&a.workgroup_id)?;
                let stable = self.orch_stable(target);
                if self.orch_pane_state(target).exited {
                    return Err(OrchError::TargetGone(stable));
                }
                if let Some(o) = self.orch_claim(chat, call_id, &scope, name, args, None) {
                    return Ok(o);
                }
                if !self.clips.entry(home).or_default().model.add(crate::clippings::Clipping { kind: a.kind, title: a.title, detail: a.detail }) {
                    self.banners.set("clippings", format!("This session has {} clippings; archive or remove some to add more.", crate::clippings::MAX_LIVE));
                }
                self.redraw();
                Ok(done(ot::ack_json()))
            }
            "start_code_review" => {
                // The session must be a Code Review member of a workgroup (`WorkgroupIntrospection`): the call
                // runs a review on the workgroup's code review member. Any other session keeps the error.
                let a = ot::parse_start_code_review(args)?;
                let pane = self.orch_resolve(&a.guid)?;
                if self.orch_pane_state(pane).exited {
                    return Err(OrchError::TargetGone(a.guid));
                }
                // A claim first, like send_text: a model starts a review only where the person let it act.
                // The launch itself is the member's own restart (`workgroup_launch`), which runs only a
                // command in the list the workgroup was entered with; nothing here is the person's answer.
                if let Some(o) = self.orch_claim(chat, call_id, &self.claim_scope_of(pane), name, args, None) {
                    return Ok(o);
                }
                let instance = self.workgroup_of(pane).map(|i| i.id.clone());
                let is_review = self.member_config(pane).is_some_and(|(i, c)| self.workgroups.get(&i).and_then(|inst| inst.workgroup.session(&c)).is_some_and(|s| s.mode == crate::workgroup::Mode::CodeReview));
                // With no prompt overlay up, the review restarts its program: only from idle (`DI:3177-3189`),
                // so a model never kills a review under way.
                if is_review {
                    if let Some((i, c)) = self.member_config(pane) {
                        let overlay = self.workgroups.get(&i).is_some_and(|inst| inst.review.overlays.contains_key(&c));
                        let state = self.panes.get(&pane).map_or(crate::session_state::State::Unknown, |p| crate::session_state::state(&p.own().tab_status));
                        if !overlay && state != crate::session_state::State::Idle {
                            let inst = self.workgroups.get(&i);
                            let role = inst.and_then(|x| x.workgroup.session(&c)).map(|s| if s.display_name.is_empty() { c.clone() } else { s.display_name.clone() }).unwrap_or_else(|| c.clone());
                            let wg = inst.map(|x| if x.workgroup.name.is_empty() { x.id.clone() } else { x.workgroup.name.clone() }).unwrap_or_default();
                            let (role, wg) = (crate::inert_files::shown_name(&role), crate::inert_files::shown_name(&wg));
                            return Err(OrchError::Unsupported(ot::review_busy(&role, &wg, state.word())));
                        }
                    }
                }
                match instance.filter(|_| is_review) {
                    Some(i) if self.request_review(&i) => Ok(done(ot::ack_json())),
                    Some(_) => Err(OrchError::Unsupported("The code review session could not start a review now (it needs to be live and restartable).".to_string())),
                    None => Err(OrchError::Unsupported(ot::not_code_review("default", &self.orch_name(pane)))),
                }
            }
            "start_session" => self.orch_start_session(chat, call_id, args),
            "register_watch" | "register_timer" | "unregister_watch" | "list_watches" => Ok(self.orch_watch_call(chat, call_id, name, args)),
            _ => Err(OrchError::UnknownTool(name.to_string())),
        }
    }

    /// get_state's payload (`SessionStateInfo`): the snapshot's fields of the session, and its
    /// last message (the status detail, else the status).
    fn orch_state_value(&self, pane: PaneId) -> Value {
        let stable = self.orch_stable(pane);
        let facts = self.orch_facts().into_iter().find(|f| f.stable_id == stable);
        let Some(f) = facts else { return json!({}) };
        let groups = orch::all_workgroups(&self.orch_live_groups(), std::slice::from_ref(&f));
        let Some((g, s)) = groups.iter().find_map(|g| g.sessions.iter().find(|s| s.session_guid == stable).map(|s| (g, s))) else { return json!({}) };
        let last = f.status.detail.clone().filter(|d| !d.trim().is_empty()).or_else(|| f.status.status.clone()).map(|t| crate::inert_files::shown_name(&t));
        let mut v = json!({"workgroup_id": g.workgroup_id, "workgroup_name": g.workgroup_name, "role_id": s.role_id, "role_name": s.role_name, "kind": s.kind, "status": s.status, "status_source": s.status_source});
        for (k, val) in [("screen_last_changed", &s.screen_last_changed), ("current_command", &s.current_command), ("last_message", &last), ("provenance", &s.provenance)] {
            if let Some(t) = val {
                v[k] = Value::from(t.clone());
            }
        }
        v
    }

    /// get_screen_contents' payload through 12c's `Screen::screen_contents` (the trailing
    /// lines on the primary screen, the grid on the alternate one).
    pub(super) fn orch_screen_value(&self, pane: PaneId, lines: Option<i64>) -> Value {
        let Some(p) = self.panes.get(&pane) else { return json!({}) };
        let s = p.own();
        let claude = s.shell.job.as_ref().is_some_and(|j| crate::proctree::same_job(&j.name, "claude")) || s.shell.job_ancestors.iter().any(|a| crate::proctree::same_job(a, "claude"));
        let kind = crate::ai_terminal::kind_of(claude.then_some("claude"), s.alt_screen());
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
        let changed = s.newest_visible_change().map(|t| now - f64::from(t)).filter(|age| *age >= 0.0).map(|age| format!("{} ago", crate::ai_terminal::compact_age(age)));
        s.screen_contents(Some(crate::ai_terminal::clamp_lines(lines.unwrap_or(0))), kind, changed)
    }

    /// The screen text a keystroke check judges.
    fn orch_screen_text(&self, pane: PaneId) -> String {
        self.orch_screen_value(pane, None)["screen_contents"]["text"].as_str().unwrap_or_default().to_string()
    }

    /// scroll_wheel: wheel reports only to a program that asked for the mouse (`DI:1961-1985`).
    fn orch_scroll(&mut self, pane: PaneId, a: &ot::ScrollWheel) -> Result<(), OrchError> {
        let name = self.orch_name(pane);
        let modes = self.modes(pane);
        if modes.tracking == crate::mouse::Tracking::Off {
            return Err(OrchError::Unsupported(ot::no_mouse_reporting(&name, &name)));
        }
        let Some(p) = self.panes.get(&pane) else { return Err(OrchError::TargetGone(a.guid.clone())) };
        let (cols, rows) = {
            use alacritty_terminal::grid::Dimensions;
            let g = p.own().term().grid();
            (g.columns() as u32, g.screen_lines() as u32)
        };
        let n = a.lines.clamp(1, MAX_NOTCHES) as i32;
        let lines = if a.up { n } else { -n };
        // N reports for N notches: no accumulator, profile gate, arrows or stuck detector (an
        // agent's injected report is not a person's wheel).
        let bytes = crate::input::wheel_reports(modes, lines, cols / 2, rows / 2);
        if bytes.is_empty() {
            return Err(OrchError::Unsupported(format!("The scroll wheel could not be sent to {name} ({name}); mouse reporting appears to be off.")));
        }
        self.enter_pane_window(pane);
        self.user_write(pane, &bytes, Write::Agent);
        Ok(())
    }

    /// (The pane named, the pane whose clippings a clipping tool reads or writes, the scope its claim
    /// lives under): for a `wg-` id the live instance's group (the leader's, `workgroup_clippings`) and the
    /// instance; for a `session:` id the session's group and its own scope. An id no live
    /// session or instance has is `UnknownWorkgroup`.
    fn orch_clippings_home(&self, workgroup_id: &str) -> Result<(PaneId, PaneId, String), OrchError> {
        let unknown = || OrchError::UnknownWorkgroup(workgroup_id.to_string());
        match ot::clipping_target(workgroup_id)? {
            ot::ClippingTarget::Session(id) => {
                let pane = self.pane_by_reference(id).ok_or_else(unknown)?;
                Ok((pane, self.clip_home(pane), ot::session_scope(&self.orch_stable(pane))))
            }
            ot::ClippingTarget::Workgroup(id) => {
                let home = self.workgroup_clippings(id).ok_or_else(unknown)?;
                let inst = self.workgroups.get(id).ok_or_else(unknown)?;
                Ok((inst.leader, home, inst.id.clone()))
            }
        }
    }

    /// The claim a write needs (`ensureClaim`, `DI:1586-1660`): None when the chat holds the
    /// scope; else the call is denied (refused earlier in this chat) or parked on the claim
    /// bubble. `mirror_guid` makes the denial a mirror's plain text.
    pub(super) fn orch_claim(&mut self, chat_id: &str, call_id: &str, scope: &str, name: &str, args: &Value, mirror_guid: Option<&str>) -> Option<Outcome> {
        let claimed = self.chat_row(chat_id).map(|c| c.claimed_scopes).unwrap_or_default();
        let denied = self.orch_denied.get(chat_id).cloned().unwrap_or_default();
        match ot::claim_needed(&claimed, scope, &denied) {
            ot::Claim::Granted => None,
            ot::Claim::Denied => Some(done(Self::claim_refusal(mirror_guid))),
            ot::Claim::Ask => {
                let shown = self.orch_scope_name(scope);
                let wait = Wait::Claim { scope: scope.to_string(), name: name.to_string(), args: args.clone(), mirror_guid: mirror_guid.map(str::to_string) };
                Some(self.orch_ask(chat_id, call_id, scope, &shown, &ot::claim_text(&shown), wait))
            }
        }
    }

    fn claim_refusal(mirror_guid: Option<&str>) -> String {
        match mirror_guid {
            Some(g) => ot::declined_session(g),
            None => ot::error_json(&OrchError::PermissionDenied),
        }
    }

    /// A scope's name for the person: the session's name, else the scope itself.
    pub(super) fn orch_scope_name(&self, scope: &str) -> String {
        if let Some(inst) = self.workgroups.get(scope) {
            return Some(crate::inert_files::shown_name(&inst.workgroup.name)).filter(|n| !n.is_empty()).unwrap_or_else(|| inst.id.clone());
        }
        match scope.strip_prefix(orch::SYNTHETIC_PREFIX).and_then(|id| self.pane_by_reference(id)) {
            Some(pane) => self.orch_name(pane),
            None => scope.to_string(),
        }
    }

    /// Publishes a permission bubble and parks the call on it (`awaitPermission`). A bubble that
    /// cannot be shown answers as a refusal: the model always gets an answer.
    pub(super) fn orch_ask(&mut self, chat: &str, call_id: &str, workgroup_id: &str, workgroup_name: &str, summary: &str, wait: Wait) -> Outcome {
        let request_id = crate::chat_message::new_id();
        orch::park(&mut self.orch_parks, &request_id, chat, call_id);
        self.orch_waits.insert(request_id.clone(), wait);
        let local = OrchLocal::PermissionRequest { request_id: request_id.clone(), workgroup_id: workgroup_id.to_string(), workgroup_name: workgroup_name.to_string(), summary: summary.to_string() };
        let m = Message::new(chat, Participant::Agent, Content::ClientLocal(ClientLocal::Other(local.to_value())), super::warnings_app::unix_now());
        if self.chat_publish(chat, m, false).is_ok() {
            return Outcome::Parked;
        }
        orch::take_park(&mut self.orch_parks, &request_id, chat);
        match self.orch_waits.remove(&request_id) {
            Some(w) => done(Self::declined(&w)),
            None => done(ot::error_json(&OrchError::PermissionDenied)),
        }
    }

    /// What a refused bubble answers (`enforce`, `DI:2786-2800`; a claim's refusal).
    fn declined(w: &Wait) -> String {
        match w {
            Wait::Claim { mirror_guid, .. } => Self::claim_refusal(mirror_guid.as_deref()),
            Wait::Approve { then: Then::Spawn(_), .. } => ot::error_json(&OrchError::PermissionDenied),
            Wait::WatchConsent { .. } => ot::error_json(&OrchError::Unsupported(crate::orch_watch::SCREEN_DECLINED.to_string())),
            Wait::Approve { then, reason, denied } => {
                let e = OrchError::SafetyBlocked(if *denied { reason.clone() } else { ot::DECLINED_APPROVAL.to_string() });
                if then.mirror() { e.message() } else { ot::error_json(&e) }
            }
        }
    }

    /// A command-approval bubble for a verdict that was not an allow (`promptForCommandApproval`).
    fn orch_ask_approval(&mut self, chat: &str, call_id: &str, then: Then, action: &str, reason: &str, denied: bool) -> Outcome {
        let summary = ot::safety_summary(action, reason, denied);
        self.orch_ask(chat, call_id, orch::COMMAND_APPROVAL_ID, orch::COMMAND_APPROVAL_NAME, &summary, Wait::Approve { then, reason: reason.to_string(), denied })
    }

    /// The spawn bubble (`promptForSpawn`), with the check's reason when it flagged the command.
    fn orch_ask_spawn(&mut self, chat: &str, call_id: &str, args: ot::StartSession, command: Option<&str>, flagged: Option<(&str, bool)>) -> Outcome {
        let base = ot::start_prompt(ot::placement(args.window.as_deref()), command);
        let (summary, reason, denied) = match flagged {
            Some((r, d)) => (ot::safety_summary(&base, r, d), r.to_string(), d),
            None => (base, String::new(), false),
        };
        self.orch_ask(chat, call_id, orch::SPAWN_ID, orch::SPAWN_NAME, &summary, Wait::Approve { then: Then::Spawn(args), reason, denied })
    }

    /// The person's answer to a permission bubble. One for a request not parked for this chat
    /// (answered, stopped, forged, another chat's) does nothing (ruling P7).
    pub(super) fn orch_claim_answered(&mut self, chat_id: &str, request_id: &str, approved: bool) {
        if !self.orch_waits.contains_key(request_id) {
            return;
        }
        let Some(call_id) = orch::take_park(&mut self.orch_parks, request_id, chat_id) else { return };
        let Some(wait) = self.orch_waits.remove(request_id) else { return };
        if let Some(p) = self.chat_agents.get_mut(chat_id).and_then(|a| a.orch_pending.as_mut()) {
            p.1 = false;
        }
        let o = match (wait, approved) {
            (Wait::Claim { scope, name, args, mirror_guid }, true) => {
                let saved = self.chat_update(chat_id, |c| {
                    if !c.claimed_scopes.contains(&scope) {
                        c.claimed_scopes.push(scope.clone());
                    }
                });
                if let Some(d) = self.orch_denied.get_mut(chat_id) {
                    d.retain(|s| *s != scope);
                }
                self.chat_buttons_refresh();
                // The call starts over: the session is resolved and checked again.
                if saved { self.orch_dispatch(chat_id, &call_id, &name, &args) } else { done(Self::claim_refusal(mirror_guid.as_deref())) }
            }
            (w @ Wait::Claim { .. }, false) => {
                if let Wait::Claim { scope, .. } = &w {
                    let d = self.orch_denied.entry(chat_id.to_string()).or_default();
                    if !d.contains(scope) {
                        d.push(scope.clone());
                    }
                }
                done(Self::declined(&w))
            }
            (Wait::WatchConsent { args }, approved) => self.orch_watch_consent_answered(chat_id, &call_id, &args, approved),
            (Wait::Approve { then, .. }, true) => self.orch_allowed(chat_id, &call_id, then),
            (w, false) => done(Self::declined(&w)),
        };
        self.orch_settle(chat_id, &call_id, o);
    }

    /// Answers a call or marks it waiting, as its outcome says.
    pub(super) fn orch_settle(&mut self, chat: &str, call_id: &str, o: Outcome) {
        match o {
            Outcome::Done(text) => self.orch_resume(chat, call_id, ToolResult::Ok(text)),
            Outcome::Failed(e) => self.orch_resume(chat, call_id, ToolResult::Failed(e)),
            Outcome::Running => {}
            Outcome::Parked => {
                // The turn waits on the person: no spinner (iTerm2's typingParkOnUser), and a
                // new message of theirs cancels the wedged turn.
                self.chat_broker.typing(chat, false, Participant::Agent);
                if let Some(p) = self.chat_agents.get_mut(chat).and_then(|a| a.orch_pending.as_mut()) {
                    p.1 = true;
                }
            }
        }
    }

    /// The person's @-picks in a message they sent grant control of those sessions (ruling P9:
    /// only `chat_input::send_grants`, never text a model or a program wrote).
    pub(super) fn orch_grant_mentions(&mut self, chat_id: &str, grants: &[String]) {
        if !self.chat_row(chat_id).is_some_and(|c| c.orchestration_enabled) {
            return;
        }
        for id in grants {
            let Some(pane) = crate::stable_id::canonical(id).and_then(|c| self.pane_by_reference(&c)) else { continue };
            // A bare pick of a workgroup member claims its instance (`claimScope(forMention:)`).
            let Some(scope) = crate::mention::claim_scope(None, id, &|_| Some(self.claim_scope_of(pane))) else { continue };
            let mut added = false;
            self.chat_update(chat_id, |c| {
                if !c.claimed_scopes.contains(&scope) {
                    c.claimed_scopes.push(scope.clone());
                    added = true;
                }
            });
            if !added {
                continue;
            }
            if let Some(d) = self.orch_denied.get_mut(chat_id) {
                d.retain(|s| *s != scope);
            }
            let local = OrchLocal::Granted { scope: scope.clone(), name: self.orch_scope_name(&scope) };
            let m = Message::new(chat_id, Participant::Agent, Content::ClientLocal(ClientLocal::Other(local.to_value())), super::warnings_app::unix_now());
            let _ = self.chat_publish(chat_id, m, false);
        }
    }

    /// Types `text` (and its CR) into `pane`, after the gate allowed it: the only writer of the
    /// model's typing, always `Write::Agent`. A bracketed-paste program gets prose as a paste
    /// and its CR later (`DI:2874-2900`); true when that CR is still due.
    pub(super) fn orch_type(&mut self, pane: PaneId, text: &str, newline: bool) -> bool {
        let Some(p) = self.panes.get(&pane) else { return false };
        let bracketed = p.own().term().mode().contains(alacritty_terminal::term::TermMode::BRACKETED_PASTE);
        let keys = text.chars().any(|c| (c as u32) < 0x20 && c != '\n');
        self.enter_pane_window(pane);
        if bracketed && !keys {
            self.user_write(pane, &crate::pane::paste_bytes(text, true), Write::Agent);
            return newline;
        }
        let mut bytes = text.as_bytes().to_vec();
        if newline {
            bytes.push(b'\r');
        }
        self.user_write(pane, &bytes, Write::Agent);
        false
    }

    /// THE chokepoint before the model's text reaches a pane's prompt line (ruling R12f-1):
    /// send_text and session_execute_command both come here. The line is planned over 12c's
    /// one accumulator (`ai_staged`), then classified, accumulated or asked; nothing is typed
    /// and the accumulator does not move before the verdict (ruling R12f-2).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn orch_type_gated(&mut self, chat_id: &str, call_id: &str, pane: PaneId, guid: &str, text: &str, append_newline: bool, after: After) -> Outcome {
        let Some(p) = self.panes.get(&pane) else { return done(ot::error_json(&OrchError::TargetGone(guid.to_string()))) };
        let s = p.own();
        let prompts = s.prompts_seen();
        let prior = TypedState::of(&self.ai_staged.get(&pane).cloned().unwrap_or_default(), prompts);
        let job = s.shell.job.as_ref().map(|j| j.name.clone());
        let screen_aware = s.alt_screen() || !crate::session_state::is_shell(job.as_deref());
        let typed = if s.at_prompt() == Some(true) { s.typed_command() } else { None };
        let plan = orch_gate::plan(prior.buffer.as_deref().unwrap_or(""), text, append_newline, screen_aware, typed.as_deref(), typed.as_deref(), prior.contaminated);
        let t = TypedCheck { pane, guid: guid.to_string(), prior, plan: plan.clone(), text: text.to_string(), append_newline };
        let then = match after {
            After::SendText => Then::SendText(t),
            After::Execute(command) => Then::Execute { typed: t, command },
        };
        match plan.route {
            Route::AccumulateOnly => self.orch_typed_finish(chat_id, call_id, then, None),
            Route::ClassifyLine(line) => {
                let shell = self.ai_shell_of(pane).0;
                // The shell may join this line with text a model already submitted (a trailing
                // pipe, its own continuation character, an open quote): the line alone and
                // each joined form are judged, and any refusal refuses (Task 10 fix round 2).
                let held = orch_gate::live_lines(self.orch_held.get(&pane), prompts, prompts > 0, super::warnings_app::unix_now(), shell);
                let forms = orch_gate::forms(&held, &line, shell);
                let summary = ot::run_summary(forms.get(1).unwrap_or(&line));
                let proposed = forms.into_iter().map(|line| Proposed::Line { line, shell }).collect();
                self.orch_check_start(chat_id, call_id, CheckKind::Command, proposed, summary, then)
            }
            Route::ScreenAware => {
                let keystroke = format!("{text}{}", if append_newline { "\r" } else { "" });
                if keystroke.is_empty() {
                    return self.orch_typed_finish(chat_id, call_id, then, None);
                }
                let screen = self.orch_screen_text(pane);
                let summary = ot::keystroke_summary(&keystroke);
                self.orch_check_start(chat_id, call_id, CheckKind::Keystroke, vec![Proposed::Keystroke { keystroke, screen }], summary, then)
            }
            Route::FailClosed(reason) => self.orch_ask_approval(chat_id, call_id, then, ot::FAILCLOSED_SUMMARY, &reason, false),
        }
    }

    /// A typed send after its verdict: `refused` is the reason when it was not allowed. The race
    /// guard runs, then the bytes go (or not) and the accumulator moves only for an allowed send.
    fn orch_typed_finish(&mut self, chat: &str, call_id: &str, then: Then, refused: Option<String>) -> Outcome {
        let mirror = then.mirror();
        let fail = |e: OrchError| done(if mirror { e.message() } else { ot::error_json(&e) });
        let (t, command) = match &then {
            Then::SendText(t) => (t.clone(), None),
            Then::Execute { typed, command } => (typed.clone(), Some(command.clone())),
            _ => return fail(OrchError::Malformed("not a typed send".to_string())),
        };
        // The pane may have closed, gone to the burial ground or reached a password prompt
        // while the check ran.
        if !self.panes.contains_key(&t.pane) {
            return if mirror { done(ot::no_session(&t.guid)) } else { fail(OrchError::TargetGone(t.guid)) };
        }
        if let Err(e) = typeable(&t.guid, self.orch_pane_state(t.pane)) {
            return fail(e);
        }
        let prompts = self.panes.get(&t.pane).map_or(0, |p| p.own().prompts_seen());
        let observed = TypedState::of(&self.ai_staged.get(&t.pane).cloned().unwrap_or_default(), prompts);
        let submitting = t.append_newline || orch_gate::contains_submit_newline(&t.text);
        let r = orch_gate::resolve_concurrent_send(&t.prior, &observed, submitting, &t.plan);
        let (state, typing) = orch_gate::after_check(&observed, refused.as_deref(), &r);
        if let Typing::Block(reason) = typing {
            self.ai_staged.insert(t.pane, state.staged(prompts, 0));
            return fail(OrchError::SafetyBlocked(reason));
        }
        // Allowed and sent: what it submitted stays as context for a continuation.
        if let Route::ClassifyLine(l) = &t.plan.route {
            let shell = self.ai_shell_of(t.pane).0;
            let h = orch_gate::record_held(self.orch_held.get(&t.pane), prompts, prompts > 0, super::warnings_app::unix_now(), shell, l, chat);
            self.orch_held.insert(t.pane, h);
        }
        match command {
            None => {
                let cr_due = self.orch_type(t.pane, &t.text, t.append_newline);
                self.ai_staged.insert(t.pane, state.staged(prompts, orch_gate::submits(&t.text, t.append_newline)));
                if cr_due {
                    self.orch_pending_cr.push(PendingCr { due: Instant::now() + CR_DELAY, pane: t.pane, chat: chat.to_string(), call_id: call_id.to_string() });
                    return Outcome::Running;
                }
                done(ot::ack_json())
            }
            Some(command) => {
                // 12c's staged form (ruling R12cd-2): the whole checked line is the call, the
                // model's command is what is typed, and `still_staged` refuses a changed prompt.
                let line = match &t.plan.route {
                    Route::ClassifyLine(l) => l.clone(),
                    _ => {
                        let (si, own) = self.ai_prompt_text(t.pane);
                        crate::ai_actions::staged_line(si.as_deref(), &own) + &command
                    }
                };
                self.orch_mirrors.insert((chat.to_string(), call_id.to_string()), (t.pane, Instant::now()));
                self.ai_run_tool(chat, call_id, t.pane, &Call::ExecuteCommand { command: line }, Some(command.clone()));
                if self.ai_execs.get(&t.pane).is_some_and(|r| r.chat == chat && r.request_id == call_id) {
                    self.ai_staged.insert(t.pane, state.staged(prompts, orch_gate::submits(&command, true)));
                }
                Outcome::Running
            }
        }
    }

    /// Runs what an allow (the gate's or the person's) lets go.
    fn orch_allowed(&mut self, chat: &str, call_id: &str, then: Then) -> Outcome {
        match then {
            Then::SendText(_) | Then::Execute { .. } => self.orch_typed_finish(chat, call_id, then, None),
            Then::Spawn(args) => {
                self.orch_spawns.push(Spawn { chat: chat.to_string(), call_id: call_id.to_string(), args });
                (self.notify)();
                Outcome::Running
            }
            Then::Mirror { pane, guid, call } => {
                if !self.panes.contains_key(&pane) {
                    return done(ot::no_session(&guid));
                }
                self.orch_run_mirror(chat, call_id, pane, &call)
            }
        }
    }

    /// A verdict reached: an allow runs, anything else asks the person. A spawn that differs
    /// from the defaults always asks (ruling P15).
    fn orch_verdict(&mut self, chat: &str, call_id: &str, then: Then, summary: &str, v: Verdict) -> Outcome {
        if let Then::Spawn(args) = then {
            let command = args.command.clone().or_else(|| self.orch_profile_command(args.profile.as_deref()));
            let flagged = match &v {
                Verdict::Allow => None,
                Verdict::RequireApproval(r) => Some((r.as_str(), false)),
                Verdict::Deny(r) => Some((r.as_str(), true)),
            };
            return self.orch_ask_spawn(chat, call_id, args, command.as_deref(), flagged);
        }
        match v {
            Verdict::Allow => self.orch_allowed(chat, call_id, then),
            Verdict::RequireApproval(r) => self.orch_ask_approval(chat, call_id, then, summary, &r, false),
            Verdict::Deny(r) => self.orch_ask_approval(chat, call_id, then, summary, &r, true),
        }
    }

    /// Starts one safety check (ruling P6): always on (the AI Safety Check setting and its nag
    /// are 12c's and do not apply), hard rules first for a shell line only, then the model
    /// behind the gate for its own host; anything that cannot run asks the person.
    fn orch_check_start(&mut self, chat: &str, call_id: &str, kind: CheckKind, proposed: Vec<Proposed>, summary: String, then: Then) -> Outcome {
        let deny = block_is_deny(kind);
        let mut runs = Vec::new();
        for p in &proposed {
            let pre = match p {
                Proposed::Line { line, shell } => ai_classifier::pre_check(&orch_gate::command_entry(line), false, *shell).map(|d| orch_gate::verdict_of(&d, deny)),
                // A keystroke and a file's content are not shell lines (`OSG:66-77`, `TUISafetyPrompt`).
                Proposed::File { .. } | Proposed::Keystroke { .. } => None,
            };
            runs.push(match pre {
                Some(v) => Run::Done(v),
                None => self.orch_check_run(chat, p, deny),
            });
        }
        self.orch_checks.push(Check { chat: chat.to_string(), call_id: call_id.to_string(), kind, then, summary, runs });
        // A check done at once is finished by the next tick.
        (self.notify)();
        Outcome::Running
    }

    fn orch_check_run(&mut self, chat: &str, proposed: &Proposed, deny: bool) -> Run {
        let found = self.ai_found();
        let catalog = crate::ai_catalog::bundled();
        let economy = crate::ai_model::economy(&self.globals, catalog, &found);
        let configured = crate::ai_model::configured(&self.globals, catalog, &found);
        let chosen = super::ai_safety_app::backend(&self.globals, economy, configured);
        if let super::ai_safety_app::Backend::Refused(e) = &chosen {
            self.ai_check_endpoint(Some(e), false);
        }
        if let Some(v) = backend_verdict(&chosen) {
            return Run::Done(v);
        }
        let super::ai_safety_app::Backend::Run(model, endpoint, proof) = chosen else { return Run::Done(Verdict::RequireApproval(orch_gate::CHECK_FAILED.to_string())) };
        let entries = self.chat_db.as_ref().and_then(|db| db.messages(chat).ok()).map(|m| ai_classifier::project(&m)).unwrap_or_default();
        let key = crate::ai_keys::shared().lock().unwrap_or_else(|e| e.into_inner()).get(model.vendor, &mut *crate::ai_keys::system_store());
        let msgs = check_messages(&entries, proposed, &sentinel());
        run_start(model, endpoint, proof, msgs, Wire::from_globals(&self.globals), self.ai_transport.clone(), crate::ai_transport::options_from(&self.globals), self.notify.clone(), key.as_ref(), deny)
    }

    /// Advances the running checks (each asks the gate first) and continues each finished one;
    /// then the mirror timeouts and the due CRs.
    pub(super) fn orch_check_tick(&mut self) {
        let checks = std::mem::take(&mut self.orch_checks);
        let mut finished = Vec::new();
        {
            let g = &self.globals;
            for mut c in checks {
                let deny = block_is_deny(c.kind);
                c.runs = std::mem::take(&mut c.runs).into_iter().map(|r| run_advance(r, |e| crate::chat_gate::pass(g, e), deny)).collect();
                match runs_verdict(&c.runs) {
                    Some(v) => finished.push((c.chat, c.call_id, c.then, c.summary, v)),
                    None => self.orch_checks.push(c),
                }
            }
        }
        for (chat, call_id, then, summary, v) in finished {
            let o = self.orch_verdict(&chat, &call_id, then, &summary, v);
            self.orch_settle(&chat, &call_id, o);
        }
        self.orch_mirror_timeouts();
        self.orch_cr_tick();
    }

    /// The soonest time a tick has work: a due CR or a mirror's limit.
    pub(super) fn orch_due(&self) -> Option<Instant> {
        let crs = self.orch_pending_cr.iter().map(|c| c.due);
        let mirrors = self.orch_mirrors.values().map(|(_, at)| *at + MIRROR_TIMEOUT);
        crs.chain(mirrors).min()
    }

    /// Each CR due after its paste, sent only while its call is still the chat's pending one
    /// (Stop, the mode off or an answered call drop it, ruling P7) and the pane can still take
    /// it; the call is answered after the CR.
    fn orch_cr_tick(&mut self) {
        for cr in take_due(&mut self.orch_pending_cr, Instant::now()) {
            let pending = self.chat_agents.get(&cr.chat).is_some_and(|a| a.in_turn && a.orch_pending.as_ref().is_some_and(|(c, _)| *c == cr.call_id));
            if !pending {
                continue;
            }
            let guid = self.orch_stable(cr.pane);
            let answer = match typeable(&guid, self.orch_pane_state(cr.pane)) {
                Ok(()) => {
                    self.enter_pane_window(cr.pane);
                    self.user_write(cr.pane, b"\r", Write::Agent);
                    ot::ack_json()
                }
                Err(e) => ot::error_json(&e),
            };
            self.orch_resume(&cr.chat, &cr.call_id, ToolResult::Ok(answer));
        }
    }

    /// A mirror past its limit answers the timeout; its command or Get-Help is forgotten.
    fn orch_mirror_timeouts(&mut self) {
        let now = Instant::now();
        let late: Vec<(String, String)> = self.orch_mirrors.iter().filter(|(_, (_, at))| now >= *at + MIRROR_TIMEOUT).map(|(k, _)| k.clone()).collect();
        for (chat, call_id) in late {
            self.orch_forget_mirror(&chat, &call_id);
            self.orch_resume(&chat, &call_id, ToolResult::Ok(ot::SESSION_TIMEOUT.to_string()));
        }
    }

    /// Drops a mirror and the 12c command or Get-Help running for it (only its own).
    fn orch_forget_mirror(&mut self, chat: &str, call_id: &str) {
        let Some((pane, _)) = self.orch_mirrors.remove(&(chat.to_string(), call_id.to_string())) else { return };
        if self.ai_execs.get(&pane).is_some_and(|r| r.chat == chat && r.request_id == call_id) {
            self.ai_exec_cancel(pane);
        }
        if self.ai_pending_tools.get(&pane).is_some_and(|(c, r)| c == chat && r == call_id) {
            self.ai_help_cancel(pane);
        }
    }

    /// A mirror's answer from 12c's one return path; false when the request is not a mirror.
    pub(super) fn orch_mirror_finished(&mut self, chat: &str, request_id: &str, model: &str) -> bool {
        if self.orch_mirrors.remove(&(chat.to_string(), request_id.to_string())).is_none() {
            return false;
        }
        // Program output reaches the model inert (Task 9 fix round 1).
        self.orch_resume(chat, request_id, ToolResult::Ok(orch::inert_json(model)));
        true
    }

    /// Runs a mirror through 12c's one dispatcher; its answer returns through
    /// `ai_tool_finished`, which hands it to `orch_resume`.
    fn orch_run_mirror(&mut self, chat: &str, call_id: &str, pane: PaneId, call: &Call) -> Outcome {
        self.orch_mirrors.insert((chat.to_string(), call_id.to_string()), (pane, Instant::now()));
        self.ai_run_tool(chat, call_id, pane, call, None);
        Outcome::Running
    }

    /// A `session_*` mirror (`handleSessionToolCall`, `DI:1226-1390`), answered in plain text.
    fn orch_mirror(&mut self, chat: &str, call_id: &str, name: &str, args: &Value) -> Outcome {
        let (_, call, guid) = match mirror_call(name, args) {
            Ok(c) => c,
            Err(text) => return done(text),
        };
        let Ok(pane) = self.orch_resolve(&guid) else { return done(ot::no_session(&guid)) };
        let category = call.category();
        // No browser pane before milestone 13: nothing to claim for.
        if category.is_browser() {
            return done(orch::UNAVAILABLE.to_string());
        }
        if ot::mirror_needs_claim(category) {
            if let Err(e) = typeable(&guid, self.orch_pane_state(pane)) {
                return done(e.message());
            }
            let scope = self.claim_scope_of(pane);
            if let Some(o) = self.orch_claim(chat, call_id, &scope, name, args, Some(&guid)) {
                return o;
            }
        }
        match call {
            // Ruling R12f-1: the same chokepoint as send_text, never classified on its own.
            Call::ExecuteCommand { .. } => match ot::typed_payload(name, args) {
                Ok(Some((command, newline))) => self.orch_type_gated(chat, call_id, pane, &guid, &command, newline, After::Execute(command.clone())),
                Ok(None) => done(ot::NEEDS_APPROVAL.to_string()),
                Err(e) => done(ot::args_error(&e.message())),
            },
            // Ruling P16: a restart stops every program in the session, so every one asks.
            Call::RestartSession => {
                let summary = ot::restart_summary(&self.orch_name(pane));
                self.orch_ask_approval(chat, call_id, Then::Mirror { pane, guid, call }, &summary, "restarting stops every program running in the session", false)
            }
            Call::CreateFile { ref filename, ref content } => {
                let summary = ot::file_summary(filename);
                let proposed = Proposed::File { path: filename.clone(), content: content.clone() };
                let then = Then::Mirror { pane, guid, call: call.clone() };
                match orch_gate::file_route(content) {
                    Some(v) => self.orch_verdict(chat, call_id, then, &summary, v),
                    None => self.orch_check_start(chat, call_id, CheckKind::FileWrite, vec![proposed], summary, then),
                }
            }
            ref c if c.needs_safety_check() => done(ot::NEEDS_APPROVAL.to_string()),
            _ => self.orch_run_mirror(chat, call_id, pane, &call),
        }
    }

    /// The command a profile runs instead of its shell, if any (ruling P15: a profile's
    /// custom or SSH command counts as running a command).
    fn orch_profile_command(&self, name: Option<&str>) -> Option<String> {
        let p = match name {
            Some(n) => self.store.profiles.iter().find(|p| p.name() == n)?,
            None => self.store.default_profile(),
        };
        matches!(p.string("Custom Command").as_str(), "Yes" | "Custom Shell" | "SSH").then(|| crate::profile::command_line(p, &self.shell, None)).filter(|c| !c.trim().is_empty())
    }

    /// start_session (`gateSpawn`, `doStartSession`) under ruling P15.
    fn orch_start_session(&mut self, chat: &str, call_id: &str, args: &Value) -> Result<Outcome, OrchError> {
        let a = ot::parse_start_session(args)?;
        let found = a.profile.as_deref().is_none_or(|n| self.store.profiles.iter().any(|p| p.name() == n));
        let now = Instant::now();
        let spawned = self.orch_spawned.get(chat).cloned().unwrap_or_default();
        let recent = spawned.iter().filter(|(at, _)| now.duration_since(*at) < Duration::from_secs(60)).count();
        let live = spawned.iter().filter(|(_, p)| self.panes.get(p).is_some_and(|q| q.exited.is_none())).count();
        let profile_command = if found { self.orch_profile_command(a.profile.as_deref()) } else { None };
        match spawn_step(&a, found, profile_command, recent, live) {
            SpawnStep::Refuse(e) => Err(e),
            SpawnStep::Start => Ok(self.orch_allowed(chat, call_id, Then::Spawn(a))),
            SpawnStep::Ask { command: Some(c) } => {
                let summary = ot::start_prompt(ot::placement(a.window.as_deref()), Some(&c));
                Ok(self.orch_check_start(chat, call_id, CheckKind::Command, vec![Proposed::Line { line: c, shell: ShellKind::Unknown }], summary, Then::Spawn(a)))
            }
            SpawnStep::Ask { command: None } => Ok(self.orch_ask_spawn(chat, call_id, a, None, None)),
        }
    }

    /// Opens each allowed spawn (the event loop is needed for a window) and answers its call
    /// with the new session's stable id (ruling P3). A spawn whose call is no longer pending
    /// (Stop, the mode off) never opens.
    pub(super) fn orch_spawn_tick(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        for s in std::mem::take(&mut self.orch_spawns) {
            let pending = self.chat_agents.get(&s.chat).is_some_and(|a| a.in_turn && a.orch_pending.as_ref().is_some_and(|(c, _)| *c == s.call_id));
            if !pending {
                continue;
            }
            let answer = match self.orch_spawn_now(event_loop, &s) {
                Ok(stable) => ot::result_json("started_session", json!({"session_guid": stable})),
                Err(e) => ot::error_json(&e),
            };
            self.orch_resume(&s.chat, &s.call_id, ToolResult::Ok(answer));
        }
    }

    fn orch_spawn_now(&mut self, event_loop: &winit::event_loop::ActiveEventLoop, s: &Spawn) -> Result<String, OrchError> {
        let a = &s.args;
        let guid = match &a.profile {
            Some(n) => Some(self.store.profiles.iter().find(|p| p.name() == *n).map(|p| p.guid()).ok_or_else(|| OrchError::Malformed(ot::no_profile(n)))?),
            None => None,
        };
        let cwd = spawn_cwd(a.cwd.as_deref())?;
        if let Some(d) = &cwd {
            if !crate::localfs::is_local_dir(std::path::Path::new(d)) {
                return Err(OrchError::Malformed(format!("cwd is not a folder: {d}")));
            }
        }
        let before: HashSet<PaneId> = self.panes.keys().copied().collect();
        self.start_command_override = a.command.clone();
        self.start_dir_override = cwd.map(crate::arrangement::Folder::Forced);
        let opened = match a.window.as_deref() {
            Some("current") if self.tabs.is_empty() => Err(OrchError::Unsupported(ot::NO_WINDOW_FOR_CURRENT.to_string())),
            Some("current") => {
                self.split_as(crate::layout::Axis::Row, guid.as_deref(), false);
                Ok(())
            }
            Some("new") => self.open_window(event_loop, guid.as_deref()).map(|_| ()).map_err(|_| OrchError::Unsupported(ot::LAUNCH_FAILED.to_string())),
            _ if self.tabs.is_empty() || self.new_tab_opens_window(guid.as_deref()) => self.open_window(event_loop, guid.as_deref()).map(|_| ()).map_err(|_| OrchError::Unsupported(ot::LAUNCH_FAILED.to_string())),
            _ => {
                self.new_tab_as(guid.as_deref(), false);
                Ok(())
            }
        };
        self.start_command_override = None;
        self.start_dir_override = None;
        opened?;
        let new = self.panes.keys().copied().find(|p| !before.contains(p)).ok_or_else(|| OrchError::Unsupported(ot::LAUNCH_FAILED.to_string()))?;
        if self.panes.get(&new).is_none_or(|p| p.exited.is_some()) {
            return Err(OrchError::Unsupported("The new session was created but is no longer reachable. The command likely exited immediately (e.g. binary not on PATH or a non-zero startup script) and the session ended.".to_string()));
        }
        let title = self.chat_row(&s.chat).map(|c| c.title);
        self.orch_provenance.insert(new, ot::provenance(title.as_deref(), &s.chat));
        self.orch_spawned.entry(s.chat.clone()).or_default().push((Instant::now(), new));
        self.redraw();
        Ok(self.orch_stable(new))
    }

    /// Everything orchestration holds for `chat` ends (ruling P7): its parked bubbles and their
    /// calls, running checks (their requests cancelled), delayed CRs, mirrors and their 12c
    /// commands, queued spawns.
    pub(super) fn orch_drop_chat(&mut self, chat: &str) {
        // The held text of the panes this chat sent to is cleared, except a continuation the
        // pane's shell still waits on (final review ruling 2); other chats keep theirs.
        let shells: std::collections::HashMap<PaneId, ShellKind> = self.orch_held.keys().map(|p| (*p, self.ai_shell_of(*p).0)).collect();
        orch_gate::after_chat_end(&mut self.orch_held, chat, |p| shells.get(p).copied().unwrap_or(ShellKind::Unknown));
        let requests: Vec<String> = self.orch_parks.iter().filter(|(_, (c, _))| c == chat).map(|(r, _)| r.clone()).collect();
        for r in requests {
            self.orch_waits.remove(&r);
        }
        for c in std::mem::take(&mut self.orch_checks) {
            if c.chat == chat {
                for r in c.runs {
                    if let Run::Running(mut runner, _) = r {
                        runner.cancel();
                    }
                }
            } else {
                self.orch_checks.push(c);
            }
        }
        self.orch_pending_cr.retain(|c| c.chat != chat);
        let mirrors: Vec<(String, String)> = self.orch_mirrors.keys().filter(|(c, _)| c == chat).cloned().collect();
        for (c, call) in mirrors {
            self.orch_forget_mirror(&c, &call);
        }
        self.orch_spawns.retain(|s| s.chat != chat);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "ptys_0123456789ABW";

    /// The dispatcher's code (no tests) and one function's body in it.
    fn code() -> &'static str {
        include_str!("orch_dispatch.rs").split("#[cfg(test)]").next().unwrap()
    }

    fn body(code: &'static str, f: &str) -> &'static str {
        let at = code.find(f).unwrap_or_else(|| panic!("{f}"));
        let rest = &code[at + f.len()..];
        let end = ["\n    fn ", "\n    pub(super) fn ", "\n    #[allow", "\n    /// "].iter().filter_map(|p| rest.find(p)).min().unwrap_or(rest.len());
        &rest[..end]
    }

    fn spawn(profile: Option<&str>, command: Option<&str>, cwd: Option<&str>) -> ot::StartSession {
        ot::StartSession { profile: profile.map(str::to_string), command: command.map(str::to_string), cwd: cwd.map(str::to_string), window: None }
    }

    /// Guard rows 1, 2, 4, 5, 11: typing reaches only a live pane in a window (ruling P8).
    #[test]
    fn typing_needs_a_live_shown_pane() {
        assert_eq!(typeable(ID, PaneState::default()), Ok(()));
        assert_eq!(typeable(ID, PaneState { exited: true, ..Default::default() }), Err(OrchError::TargetGone(ID.into())));
        for (s, why) in [(PaneState { buried: true, ..Default::default() }, ot::BURIED), (PaneState { password: true, ..Default::default() }, ot::AT_PASSWORD), (PaneState { gateway: true, ..Default::default() }, ot::GATEWAY)] {
            assert_eq!(typeable(ID, s), Err(OrchError::Unsupported(ot::not_typeable(ID, why))));
        }
    }

    /// Ruling P3: only a canonical stable id names a session; a decimal pane id or 12d's
    /// per-launch uuid names nothing.
    #[test]
    fn references_are_stable_ids_only() {
        let panes = [(7u64, ID), (8, "ptys_ZZZZZZZZZZZZZ")];
        assert_eq!(resolve(ID, panes), Ok(7));
        assert_eq!(resolve("ptys_0123456789abw", panes), Ok(7), "canonical form");
        for bad in ["7", "8", "6F9619FF-8B86-D011-B42D-00C04FC964FF", "ptys_0123456789ABX", "session:ptys_0123456789ABW"] {
            assert_eq!(resolve(bad, panes), Err(OrchError::UnknownSession(bad.into())), "{bad}");
        }
        // Every session-naming path of the dispatcher resolves through it.
        let src = include_str!("orch_dispatch.rs");
        let code = src.split("#[cfg(test)]").next().unwrap();
        assert!(code.contains("resolve(guid, self.panes.iter()") && !code.contains("pane_by_reference(&guid)") && !code.contains(".parse::<PaneId>"));
    }

    /// Guard row 3 and ruling P7: a CR waits its 75 ms, and a chat's Stop drops it.
    #[test]
    fn stop_drops_pending_cr() {
        let t = Instant::now();
        let cr = |chat: &str, ms: u64| PendingCr { due: t + Duration::from_millis(ms), pane: 1, chat: chat.into(), call_id: "c".into() };
        let mut q = vec![cr("A", 75), cr("B", 75), cr("A", 500)];
        assert!(take_due(&mut q, t).is_empty(), "nothing before its time");
        q.retain(|c| c.chat != "A");
        assert_eq!(take_due(&mut q, t + CR_DELAY).iter().map(|c| c.chat.as_str()).collect::<Vec<_>>(), ["B"]);
        assert!(q.is_empty());
        // The dispatcher's own drop does exactly this for the chat (`orch_drop_chat`).
        assert!(body(code(), "fn orch_drop_chat(").contains("self.orch_pending_cr.retain(|c| c.chat != chat);"));
        assert!(body(code(), "fn orch_cr_tick(").contains("let pending = self.chat_agents.get(&cr.chat).is_some_and(|a| a.in_turn && a.orch_pending"), "a CR goes only while its call is pending");
    }

    /// Guard rows 6 (ruling P15): start_session refuses a network folder and an unknown profile,
    /// is capped, asks for anything but the defaults, and a profile's own command counts.
    #[test]
    fn start_session_follows_p15() {
        assert_eq!(spawn_step(&spawn(None, None, None), true, None, 0, 0), SpawnStep::Start);
        assert_eq!(spawn_step(&spawn(None, Some("make"), None), true, None, 0, 0), SpawnStep::Ask { command: Some("make".into()) });
        assert_eq!(spawn_step(&spawn(Some("ssh box"), None, None), true, Some("ssh box.example".into()), 0, 0), SpawnStep::Ask { command: Some("ssh box.example".into()) });
        assert_eq!(spawn_step(&spawn(Some("nope"), None, None), false, None, 0, 0), SpawnStep::Refuse(OrchError::Malformed(ot::no_profile("nope"))));
        for unc in ["\\\\host\\share", "//host/share", "\\\\?\\C:\\x", "/\\host"] {
            assert_eq!(spawn_step(&spawn(None, None, Some(unc)), true, None, 0, 0), SpawnStep::Refuse(OrchError::Unsupported(ot::NETWORK_CWD.into())), "{unc}");
        }
        assert_eq!(spawn_step(&spawn(None, None, Some("C:\\src")), true, None, 0, 0), SpawnStep::Ask { command: None });
        assert_eq!(spawn_cwd(Some("relative\\dir")), Ok(None), "a relative folder is the profile's");
        assert_eq!(spawn_step(&spawn(None, None, None), true, None, SPAWNS_PER_MINUTE, 0), SpawnStep::Refuse(OrchError::Unsupported(ot::TOO_MANY_SPAWNS.into())));
        assert_eq!(spawn_step(&spawn(None, None, None), true, None, 0, SPAWNS_LIVE), SpawnStep::Refuse(OrchError::Unsupported(ot::TOO_MANY_SPAWNS.into())));
    }

    /// Guard rows 7, 8, 9, 10, 19: the mirrors' order and claims.
    #[test]
    fn mirrors_follow_iterm2() {
        let g = |v: Value| mirror_call("session_execute_command", &v);
        assert_eq!(mirror_call("session_x", &json!({})), Err(ot::unknown_session_tool("session_x")));
        assert_eq!(mirror_call("session_insert_text_at_cursor", &json!({})), Err(ot::not_in_orchestration("session_insert_text_at_cursor")));
        assert_eq!(mirror_call("session_delete_current_line", &json!({})), Err(ot::not_in_orchestration("session_delete_current_line")));
        assert_eq!(mirror_call("session_get_screen_contents", &json!({})), Err(ot::use_workgroup_screen("session_get_screen_contents")));
        assert_eq!(g(json!([1])), Err(ot::ARGS_NOT_OBJECT.into()));
        assert_eq!(g(json!({"command": "ls"})), Err(ot::MISSING_GUID.into()));
        assert!(matches!(g(json!({"session_guid": ID})), Err(e) if e.starts_with("Error decoding arguments: ")));
        let (raw, call, guid) = g(json!({"session_guid": ID, "command": "ls"})).unwrap();
        assert_eq!((raw.as_str(), call, guid.as_str()), ("execute_command", Call::ExecuteCommand { command: "ls".into() }, ID));
        let claim = |n: &str, v: Value| ot::mirror_needs_claim(mirror_call(n, &v).unwrap().1.category());
        assert!(claim("session_restart_session", json!({"session_guid": ID})), "row 7");
        assert!(claim("session_set_clipboard", json!({"session_guid": ID, "text": "x"})), "row 8");
        assert!(claim("session_create_file", json!({"session_guid": ID, "filename": "a", "content": "b"})), "row 9");
        assert!(!claim("session_get_man_page", json!({"session_guid": ID, "cmd": "ls"})), "row 10: no claim (iTerm2)");
        assert!(mirror_call("session_load_url", &json!({"session_guid": ID, "url": "https://x"})).unwrap().1.category().is_browser(), "row 19: answered unavailable");
        // The dispatcher's mirror arms (ruling P16, R12f-1).
        let mirror = body(code(), "fn orch_mirror(");
        assert!(mirror.contains("Call::RestartSession => {") && mirror.contains("self.orch_ask_approval(chat, call_id, Then::Mirror"), "a restart always asks");
        assert!(mirror.contains("Call::ExecuteCommand { .. } => match ot::typed_payload(name, args)") && mirror.contains("self.orch_type_gated("), "execute_command goes through the chokepoint");
        let claim_at = mirror.find("self.orch_claim(").unwrap();
        assert!(claim_at < mirror.find("match call {").unwrap(), "the claim comes before any action");
    }

    /// Guard row 17 and ruling P7: an answer acts once, only for its own chat.
    #[test]
    fn answer_twice_types_once() {
        let mut parks = orch::Parks::new();
        orch::park(&mut parks, "R", "A", "c1");
        assert_eq!(orch::take_park(&mut parks, "R", "B"), None, "another chat's button does nothing");
        assert_eq!(orch::take_park(&mut parks, "R", "A"), Some("c1".into()));
        assert_eq!(orch::take_park(&mut parks, "R", "A"), None, "a second press does nothing");
        let answered = body(code(), "fn orch_claim_answered(");
        let take = answered.find("orch::take_park(").expect("the park is taken");
        assert!(take < answered.find("self.orch_allowed(").unwrap() && take < answered.find("self.orch_dispatch(").unwrap(), "nothing runs before the park is taken");
    }

    /// Ruling P7: a late approval never types; Stop ends the checks, bubbles, CRs and mirrors.
    #[test]
    fn late_approval_never_types() {
        let drop = body(code(), "fn orch_drop_chat(");
        for part in ["self.orch_waits.remove(&r)", "runner.cancel()", "self.orch_pending_cr.retain", "self.orch_forget_mirror(", "self.orch_spawns.retain"] {
            assert!(drop.contains(part), "{part}");
        }
        let app = include_str!("orch_app.rs");
        assert!(app.contains("self.orch_drop_chat(chat);"), "every halt reaches it");
        // A wait whose park is gone answers nothing: the waits are keyed by request id and
        // `orch_claim_answered` takes the park first.
        let answered = body(code(), "fn orch_claim_answered(");
        assert!(answered.starts_with("&mut self, chat_id: &str, request_id: &str, approved: bool) {\n        if !self.orch_waits.contains_key(request_id) {\n            return;"));
    }

    fn reply(text: &str) -> crate::ai_transport::Reply {
        let body = json!({"choices": [{"index": 0, "message": {"role": "assistant", "content": text}, "finish_reason": "stop"}]});
        crate::ai_transport::Reply::status(200, body.to_string().as_bytes())
    }

    fn check(reply: crate::ai_transport::Reply, proposed: &Proposed, gate: bool, deny: bool) -> (Run, Arc<crate::ai_transport::Recorded>) {
        use crate::ai_transport::{Proxy, Recorded};
        let rec = Arc::new(Recorded::new(vec![reply]));
        let m = crate::ai_model::fixture("gpt-x", "https://api.openai.com/v1/chat/completions", crate::ai_settings::Api::ChatCompletions);
        let wire = Wire { token_limit: 128000, response_limit: 8192, zdr: false, ollama_num_ctx: 0, ollama_keep_alive: "30m".into() };
        let msgs = check_messages(&[Entry::UserText("please".into())], proposed, "SCREEN-1");
        let key = Secret::new("sk-a".to_string());
        let started = run_start(m.clone(), crate::chat_gate::endpoint_of(&m), Passed::for_test(), msgs, wire, rec.clone(), Options { idle: Duration::ZERO, proxy: Proxy::Off }, Arc::new(|| {}), Some(&key), deny);
        (run_advance(started, |_| gate.then(Passed::for_test), deny), rec)
    }

    fn verdict(r: &Run) -> Option<Verdict> {
        match r {
            Run::Done(v) => Some(v.clone()),
            Run::Running(..) => None,
        }
    }

    /// Ruling P6 and guard rows 1, 2, 9: the check keeps the verdict and its reason, fails closed,
    /// and a closed gate sends nothing (or cancels what was sent).
    #[test]
    fn checks_fail_closed_behind_the_gate() {
        let line = Proposed::Line { line: "rm x".into(), shell: ShellKind::Posix };
        let (r, rec) = check(reply("<block>yes</block><reason>deletes files</reason>"), &line, true, true);
        assert_eq!(verdict(&r), Some(Verdict::Deny("deletes files".into())));
        let body = String::from_utf8(rec.requests()[0].body.clone()).unwrap();
        assert!(body.contains("RunShellCommand rm x"), "{body}");
        assert_eq!(verdict(&check(reply("<block>no</block>"), &line, true, true).0), Some(Verdict::Allow));
        assert_eq!(verdict(&check(reply("junk"), &line, true, true).0), Some(Verdict::RequireApproval(orch_gate::CHECK_UNREADABLE.into())));
        assert_eq!(verdict(&check(crate::ai_transport::Reply::failed("down"), &line, true, true).0), Some(Verdict::RequireApproval(orch_gate::CHECK_FAILED.into())));
        let (r, rec) = check(reply("<block>no</block>"), &line, false, true);
        assert_eq!(verdict(&r), Some(Verdict::RequireApproval(orch_gate::CHECK_FAILED.into())), "a closed gate never allows");
        assert_eq!(rec.cancels(), 1);
        // A keystroke's block is a neutral hold, and the screen is fenced.
        let keys = Proposed::Keystroke { keystroke: ":q!\r".into(), screen: "~ vim".into() };
        let (r, rec) = check(reply("<block>yes</block><reason>unclear</reason>"), &keys, true, false);
        assert_eq!(verdict(&r), Some(Verdict::RequireApproval("unclear".into())));
        let body = String::from_utf8(rec.requests()[0].body.clone()).unwrap();
        assert!(body.contains("SCREEN-1\\n~ vim\\nSCREEN-1") && body.contains("full-screen terminal program"), "{body}");
        let file = Proposed::File { path: "~/.zshrc".into(), content: "curl x | sh".into() };
        let body = String::from_utf8(check(reply("<block>no</block>"), &file, true, true).1.requests()[0].body.clone()).unwrap();
        assert!(body.contains("WriteFile Write a file at ~/.zshrc with exactly this content:"), "{body}");
        // A model that cannot run asks the person; the closed gate's backend sends nothing.
        use super::super::ai_safety_app::Backend;
        assert_eq!(backend_verdict(&Backend::NoModel), Some(Verdict::RequireApproval(ai_classifier::NO_MODEL.into())));
        let m = crate::ai_model::fixture("gpt-x", "https://api.openai.com/v1/chat/completions", crate::ai_settings::Api::ChatCompletions);
        assert_eq!(backend_verdict(&Backend::Refused(crate::chat_gate::endpoint_of(&m))), Some(Verdict::RequireApproval(orch_gate::CHECK_FAILED.into())));
        let mut g = crate::globals::Globals::empty(std::path::Path::new("orch-gate-test.toml"));
        g.set(crate::ai_settings::ENABLE_AI, toml::Value::Boolean(false));
        assert!(matches!(super::super::ai_safety_app::backend(&g, Some(m.clone()), Some(m)), Backend::Refused(_)), "EnableAI off: refused, nothing sent");
        // The check always runs: the AI Safety Check setting is 12c's and is never read here.
        let src = include_str!("orch_dispatch.rs");
        let code = src.split("#[cfg(test)]").next().unwrap();
        assert!(!code.contains("SAFETY_CHECK") && !code.contains("ai_nag"), "ruling P6");
    }

    /// The name of the function whose body holds byte `at`.
    pub(crate) fn owner(code: &str, at: usize) -> &str {
        let head = &code[..at];
        let f = ["\n    fn ", "\n    pub(super) fn ", "\nfn ", "\npub(super) fn "].iter().filter_map(|p| head.rfind(p).map(|i| i + p.len())).max().unwrap();
        code[f..].split(['(', '<']).next().unwrap()
    }

    /// The source rules the security review names: one chokepoint, one accumulator.
    #[test]
    fn the_chokepoint_is_the_only_typist() {
        let src = include_str!("orch_dispatch.rs");
        let code = src.split("#[cfg(test)]").next().unwrap();
        // Only orch_type, the delayed CR, interrupt's Ctrl-C and the wheel write to a pane.
        let writers: Vec<&str> = code.match_indices("self.user_write(").map(|(i, _)| owner(code, i)).collect();
        for w in &writers {
            assert!(["orch_type", "orch_cr_tick", "orch_tool", "orch_scroll"].contains(w), "{w} writes to a pane");
        }
        assert_eq!(writers.len(), 5, "{writers:?}");
        // orch_type is called only after a verdict.
        let calls: Vec<usize> = code.match_indices("self.orch_type(").map(|(i, _)| i).collect();
        assert_eq!(calls.len(), 1);
        assert_eq!(owner(code, calls[0]), "orch_typed_finish");
        // A command is classified only inside the chokepoint and start_session.
        for (at, _) in code.match_indices("CheckKind::Command,") {
            assert!(["orch_type_gated", "orch_start_session"].contains(&owner(code, at)), "{}", owner(code, at));
        }
    }

    /// Task 10 fix rounds 1 and 2: every form of a split line is judged, the held text is
    /// recorded only after an allowed send, and a chat's end clears its panes' held text.
    #[test]
    fn the_held_text_is_joined_recorded_and_cleared() {
        let src = include_str!("orch_dispatch.rs");
        let code = src.split("#[cfg(test)]").next().unwrap();
        let at = |needle: &str| code.match_indices(needle).map(|(i, _)| owner(code, i)).collect::<Vec<_>>();
        assert_eq!(at("orch_gate::forms("), ["orch_type_gated"], "the classified text is every form");
        assert_eq!(at("orch_gate::record_held("), ["orch_typed_finish"], "recorded only after an allowed send");
        assert_eq!(at("orch_gate::after_chat_end(&mut self.orch_held, chat, "), ["orch_drop_chat"], "only the ending chat's panes");
        assert_eq!(at("runs_verdict(&"), ["orch_check_tick"], "a check's verdict is the worst of its forms");
        assert!(include_str!("orch_app.rs").split("#[cfg(test)]").next().unwrap().contains("self.orch_drop_chat(chat)"));
    }

    /// Task 10 fix round 2: every path that ends what the held text stands for clears it.
    /// Stop, EnableAI off, the mode off and a delete reach `orch_chat_ended` (Task 10); a gate
    /// that closes halts the turn (`chat_engine::ai_off_stops_the_chat`, `answers_need_the_gate`)
    /// with `CancelChecks`, which App maps to `orch_chat_ended`; a pane gone for good, a
    /// restart's new shell and the person's own writes clear it directly.
    #[test]
    fn every_halt_and_close_clears_the_held_text() {
        let code = |s: &'static str| s.split("#[cfg(test)]").next().unwrap();
        let fn_body = |src: &'static str, name: &str| {
            let c = code(src);
            let i = c.find(name).unwrap_or_else(|| panic!("{name} is missing"));
            let rest = &c[i..];
            rest[..rest.find("\n    }\n").unwrap()].to_string()
        };
        let apply = fn_body(include_str!("chat_app.rs"), "EngineEffect::CancelChecks(chat) =>");
        assert!(apply.contains("self.orch_chat_ended(&chat);"), "a halt (and so a closed gate) ends the chat");
        assert!(fn_body(include_str!("orch_app.rs"), "fn orch_chat_ended(").contains("self.orch_drop_chat(chat);"));
        assert!(fn_body(include_str!("mod.rs"), "fn pane_gone(").contains("self.orch_held.remove(&id);"), "a pane closed for good");
        assert!(fn_body(include_str!("mod.rs"), "fn restart_confirmed(").contains("self.orch_held.remove(&id);"), "a restarted pane runs a new shell");
        let write = fn_body(include_str!("broadcast_ui.rs"), "fn write_to_domain(");
        assert!(write.contains("crate::orch_gate::person_writes(what)") && write.contains("self.orch_held.remove(&t);"), "the person's typing and pastes");
    }

    #[test]
    fn a_check_waits_for_every_form() {
        assert_eq!(runs_verdict(&[Run::Done(Verdict::Allow), Run::Done(Verdict::Allow)]), Some(Verdict::Allow));
        assert_eq!(runs_verdict(&[Run::Done(Verdict::Allow), Run::Done(Verdict::Deny("d".into()))]), Some(Verdict::Deny("d".into())));
        assert!(matches!(runs_verdict(&[]), Some(Verdict::RequireApproval(_))));
    }

    /// `WorkgroupIntrospection.swift:366-389`: a `wg-` id names a live instance's group; the model's
    /// actions on a workgroup go through the claim and the one command list, never the person's answer.
    #[test]
    fn workgroup_clippings_follow_iterm2() {
        let wg = "wg-6F1C2A3B-0000-4000-8000-000000000002";
        assert_eq!(ot::clipping_target(wg), Ok(ot::ClippingTarget::Workgroup(wg)));
        assert_eq!(ot::clipping_target("session:ptys_0123456789ABW"), Ok(ot::ClippingTarget::Session("ptys_0123456789ABW")), "session: ids keep 12f's behavior");
        assert_eq!(ot::clipping_target("wg-nope"), Err(OrchError::UnknownWorkgroup("wg-nope".into())));
        let c = code();
        // Both tools resolve through one function; its wg- arm asks the app for the live instance's group,
        // and an id that names none is UnknownWorkgroup.
        let home = body(c, "fn orch_clippings_home(");
        assert!(home.contains("ot::ClippingTarget::Workgroup(id) =>") && home.contains("self.workgroup_clippings(id).ok_or_else(unknown)?"));
        assert!(home.contains("ot::ClippingTarget::Session(id) =>") && home.contains("self.pane_by_reference(id).ok_or_else(unknown)?"));
        let list = body(c, "\"list_workgroup_clippings\" => {");
        assert!(list.contains("self.orch_clippings_home(&a.workgroup_id)?") && list.contains("ot::clippings_value(&list, a.type_filter.as_deref())"), "filtered by type_filter");
        let add = body(c, "\"add_workgroup_clipping\" => {");
        assert!(add.contains("self.orch_clippings_home(&a.workgroup_id)?") && add.contains("model.add("), "appends to the group's clippings");
        assert!(add.find("self.orch_claim(").unwrap() < add.find("model.add(").unwrap(), "the claim comes first");
        let app = include_str!("workgroup_app.rs");
        let wc = body_of(app, "pub(super) fn workgroup_clippings(");
        assert!(wc.contains("self.clip_home(inst.leader)") && wc.contains("self.workgroups.get(id)"), "the leader's group of a live instance");
        // Claims by workgroup: a member's writes claim its instance.
        for tool in ["\"send_text\" => {", "\"interrupt\" => {", "\"start_code_review\" => {"] {
            assert!(body(c, tool).contains("self.claim_scope_of(pane)"), "{tool}");
        }
        assert!(body(c, "fn orch_mirror(").contains("self.claim_scope_of(pane)"));
        // start_code_review needs the claim first, then only the member's own restart runs.
        let review = body(c, "\"start_code_review\" => {");
        assert!(review.find("self.orch_claim(").unwrap() < review.find("self.request_review(").unwrap(), "a model starts a review only where the person let it act");
        // Task 10 review Important 1: with no overlay up, a review member that is not idle is refused
        // before anything restarts it (`DI:3177-3189`), after the claim.
        let busy = review.find("ot::review_busy(").expect("the busy refusal");
        assert!(review.find("self.orch_claim(").unwrap() < busy && busy < review.find("self.request_review(").unwrap(), "claim, then the idle check, then the restart");
        assert!(review.contains("session_state::State::Idle") && review.contains("review.overlays.contains_key("), "idle unless the overlay is up");
        // Nothing an orchestration tool does can enter a workgroup, mark one trusted, or answer its question.
        for banned in ["enter_workgroup", "workgroup_picked", "Approved::of", "Mechanism::", "enter_workgroup_now", "Trust", "restore_answered"] {
            assert!(!c.contains(banned), "orchestration must not reach {banned}");
        }
        // The launch a review makes is the member's restart, which runs only a command in the list the
        // workgroup was entered with.
        assert!(app.contains("i.approved.allows(&template)"));
    }

    fn body_of(code: &'static str, f: &str) -> &'static str {
        body(code, f)
    }
}

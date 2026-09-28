//! The chat turns, App-free (ruling R12cd-4): `Engine` answers `Effect::ToService`, the
//! person's answers to requests, tool results and the ticks, each touching only the `Ctx` it is
//! lent and returning the `EngineEffect`s the App applies after the borrow ends. The App's side
//! of those effects (the dispatcher `ai_run_tool`, the live drivers) is `chat_app.rs`.

use std::collections::HashMap;
use std::time::Instant;

use super::ai_app::KeyThen;
use super::chat_app::{publish_in, set_permission_in};
use crate::ai_catalog::{Feature, Model, VectorStore};
use crate::ai_files::{FileOut, Job};
use crate::ai_model::has;
use crate::ai_models::Endpoint;
use crate::ai_msg::{self, Attachment, CallId, FunctionCall, Role};
use crate::ai_permissions::{self, Category, Gate, Key, Overrides, Permission};
use crate::ai_settings::Vendor;
use crate::ai_terminal::Outcome;
use crate::ai_tools::Call;
use crate::ai_turn::{Hosted, Out, ToolResult, TurnInput, Wire};
use crate::chat_agent::{self, Agent, FileDriver, FileFlow, Park, TurnDriver};
use crate::chat_broker::{Broker, Effect};
use crate::chat_gate::{self, endpoint_of, files_endpoint, Passed};
use crate::chat_message::{locks_provider, ClientLocal, Content, Message, Participant, Payload, RemoteCommand, Subpart, TerminalCommand, TurnEvent, UserCommand};
use crate::chat_store::{Chat, Store};
use crate::globals::Globals;
use crate::layout::PaneId;
use crate::pwm::Secret;

/// The person's answer to a parked request (`CVC:1826-1896`).
#[allow(dead_code)] // Made by the chat window's buttons (12d).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    AllowOnce,
    AlwaysAllow,
    DenyOnce,
    AlwaysDeny,
}

/// What the engine borrows from the App for one call (ruling R12cd-4): the four App fields 12d
/// reads by name, the globals, and the closures that answer what only the App knows.
pub(super) struct Ctx<'a> {
    pub store: &'a mut Store,
    pub broker: &'a mut Broker,
    pub overrides: &'a mut Overrides,
    pub agents: &'a mut HashMap<String, Agent>,
    pub globals: &'a Globals,
    /// The chat's model: `named(chat.model_name)`, else the configured one.
    pub model_for: &'a dyn Fn(&Chat) -> Option<Model>,
    /// A session guid to its live pane.
    pub pane_of: &'a dyn Fn(&str) -> Option<PaneId>,
    pub new_turn: &'a mut dyn FnMut(&Model) -> Box<dyn TurnDriver>,
    pub new_files: &'a mut dyn FnMut(&Model, Job) -> Box<dyn FileDriver>,
    /// Seconds since 1970, for the messages published.
    pub now: f64,
}

/// What the App does once the engine's borrow ends, in order.
#[derive(Debug, PartialEq)]
pub(super) enum EngineEffect {
    /// The one dispatch of a tool call, after the gate answered Run or the person allowed it.
    RunTool { chat: String, request_id: String, pane: PaneId, call: Call, send: Option<String> },
    /// Forget the command or Get-Help running on the pane, unanswered (Stop, Cancel).
    CancelExec(PaneId),
    CancelChecks(String),
    /// Compute the staged line, then check `command` (or not) and come back through `checked`.
    Check { chat: String, pane: PaneId, command: String, request: Message },
    GateRefused { endpoint: Endpoint, silent: bool },
    AskKey { vendor: Vendor, endpoint: Endpoint, then: KeyThen },
    Link { chat: String, pane: PaneId },
    Republish { chat: String, request_id: String },
    /// A request carried the terminal context: commit its watermark (minor m2).
    VolatileSent(String),
    /// An Always Allow or Always Deny holds for this run but its row was not written: tell the
    /// person, as `ai_set_permission` does (Task 3 ruling).
    PermissionNotSaved,
}

type Fx = Vec<EngineEffect>;

/// The chat turns, App-free (ruling R12cd-4): each function touches only the `Ctx` it is lent
/// and answers the effects the App applies after the borrow ends.
pub(super) struct Engine;

/// The one send of each kind, each spending a proof that the gate passed just before.
fn send_start(d: &mut dyn TurnDriver, _: Passed, input: TurnInput) -> Vec<Out> {
    d.start(input)
}

fn send_tool_done(d: &mut dyn TurnDriver, _: Passed, r: ToolResult) -> Vec<Out> {
    d.tool_done(r)
}

fn send_key(d: &mut dyn TurnDriver, _: Passed, key: Option<Secret>) -> Vec<Out> {
    d.key_given(key)
}

fn send_file_job(d: &mut dyn FileDriver, _: Passed, key: Option<&Secret>) -> Vec<FileOut> {
    d.start(key)
}

/// A call's argument text as JSON: empty or missing is `{}` (Anthropic sends nothing), and a
/// truncated text is completed as iTerm2 does (`parseTruncatedJSON`).
fn arguments_json(text: Option<&str>) -> Result<serde_json::Value, String> {
    let text = text.filter(|t| !t.is_empty()).unwrap_or("{}");
    serde_json::from_str(text).or_else(|_| serde_json::from_str(&complete_json(text))).map_err(|e| format!("While parsing a function call request: {e}"))
}

/// `completeJSON` (`JSONSerialization+iTerm.swift:13-62`): an open string closed, then the open
/// brackets and braces closed in reverse order.
fn complete_json(json: &str) -> String {
    let (mut stack, mut in_string, mut escaped) = (Vec::new(), false, false);
    for c in json.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
        } else {
            match c {
                '"' => in_string = true,
                '{' | '[' => stack.push(c),
                '}' if stack.last() == Some(&'{') => {
                    stack.pop();
                }
                ']' if stack.last() == Some(&'[') => {
                    stack.pop();
                }
                _ => {}
            }
        }
    }
    let mut out = json.to_string();
    if in_string {
        out.push('"');
    }
    while let Some(open) = stack.pop() {
        out.push(if open == '{' { '}' } else { ']' });
    }
    out
}

/// The classic request's command, if the message is one.
fn classic(m: &Message) -> Option<&RemoteCommand> {
    match &m.content {
        Content::RemoteCommandRequest { payload: Payload::Classic(rc), .. } => Some(rc),
        _ => None,
    }
}

impl Engine {
    /// A message published without handing it to the service (the engine's own).
    fn put(cx: &mut Ctx, chat: &str, m: Message, partial: bool) {
        // iTerm2 `try?`: a failed write loses the message, never the turn.
        let _ = publish_in(cx.store, cx.broker, chat, m, partial);
    }

    fn agent_says(cx: &mut Ctx, chat: &str, content: Content) {
        let m = Message::new(chat, Participant::Agent, content, cx.now);
        Self::put(cx, chat, m, false);
    }

    fn notice(cx: &mut Ctx, chat: &str, text: &str) {
        Self::agent_says(cx, chat, Content::ClientLocal(ClientLocal::Notice(text.to_string())));
    }

    /// The chat's agent, made on first use; until it has seen a permission set it takes the
    /// chat's latest stored one (the App may have made it first, for the context snapshot).
    fn ensure_agent(cx: &mut Ctx, chat: &str) {
        if cx.agents.get(chat).is_some_and(|a| a.allowed.is_some()) {
            return;
        }
        let allowed = cx.store.messages(chat).unwrap_or_default().into_iter().rev().find_map(|m| if let Content::SetPermissions(set) = m.content { Some(set.into_iter().collect()) } else { None });
        cx.agents.entry(chat.to_string()).or_default().allowed = Some(allowed.unwrap_or_default());
    }

    fn in_turn(cx: &Ctx, chat: &str) -> bool {
        cx.agents.get(chat).is_some_and(|a| a.in_turn)
    }

    /// `ChatBroker.publish` and the service's hand-off (`ChatService.swift:116-178`).
    pub(super) fn publish(cx: &mut Ctx, chat: &str, m: Message, partial: bool) -> Result<Fx, String> {
        Self::ensure_agent(cx, chat);
        let handoff = publish_in(cx.store, cx.broker, chat, m, partial)?;
        let mut fx = Vec::new();
        for e in handoff {
            if let Effect::ToService(m) = e {
                fx.extend(Self::service(cx, chat, m));
            }
        }
        Ok(fx)
    }

    fn service(cx: &mut Ctx, chat: &str, m: Message) -> Fx {
        match m.content {
            Content::SetPermissions(set) => {
                // An orchestration chat's agent is 12f's.
                let orchestrating = cx.store.chat(chat).ok().flatten().is_some_and(|c| c.orchestration_enabled);
                if let (false, Some(a)) = (orchestrating, cx.agents.get_mut(chat)) {
                    a.allowed = Some(set.into_iter().collect());
                }
                Vec::new()
            }
            Content::UserCommand(UserCommand::Stop) => Self::stop(cx, chat),
            // A tool's response is the engine's own; the others drive nothing.
            Content::UserCommand(_) | Content::RemoteCommandResponse { .. } | Content::Unsupported(_) => Vec::new(),
            _ => {
                let mut fx = Vec::new();
                // A turn wedged on the person's answer is cancelled for the new message
                // (`isAwaitingUserApproval`, `CA:1249-1282`); a working turn keeps it queued.
                if cx.agents.get(chat).is_some_and(Agent::is_awaiting_approval) {
                    fx.extend(Self::halt(cx, chat));
                }
                fx.extend(Self::next(cx, chat));
                fx
            }
        }
    }

    /// Starts the chat's next queued turn when it has none.
    fn next(cx: &mut Ctx, chat: &str) -> Fx {
        if cx.agents.get(chat).is_some_and(|a| a.in_turn || a.held.is_some() || a.files.is_some()) {
            return Vec::new();
        }
        match cx.broker.next_turn(chat) {
            Some(m) => Self::begin(cx, chat, m, true),
            None => Vec::new(),
        }
    }

    /// Ends the turn every way a turn ends: the reply committed, typing off, `Turn(Ended)`, the
    /// queue moving on.
    fn end_turn(cx: &mut Ctx, chat: &str) -> Fx {
        Self::commit(cx, chat);
        if let Some(a) = cx.agents.get_mut(chat) {
            a.in_turn = false;
            a.driver = None;
            a.question = None;
            a.reasoning = None;
            a.held = None;
            a.model = None;
            if let Ok(mut v) = a.volatile.lock() {
                *v = None;
            }
        }
        cx.broker.typing(chat, false, Participant::Agent);
        cx.broker.turn(chat, TurnEvent::Ended);
        cx.broker.turn_done(chat);
        Self::next(cx, chat)
    }

    /// The streamed reply ends: its commit, and the next reply starts a new message.
    fn commit(cx: &mut Ctx, chat: &str) {
        let Some(a) = cx.agents.get_mut(chat) else { return };
        a.attachment_ids.clear();
        if let Some(id) = a.streaming_id.take() {
            Self::agent_says(cx, chat, Content::Commit(id));
        }
    }

    /// A turn for `user` (`CA:1323-1409`): the gate for the model's own endpoint first, the
    /// provider binding, the files' vector-store flow, then the request.
    fn begin(cx: &mut Ctx, chat: &str, user: Message, allow_files: bool) -> Fx {
        Self::ensure_agent(cx, chat);
        let Some(row) = cx.store.chat(chat).ok().flatten() else {
            cx.broker.turn_done(chat);
            return Vec::new();
        };
        let body = match &user.content {
            Content::Multipart { parts, .. } => Some(chat_agent::multipart_body(parts)),
            c => chat_agent::user_text(c).map(ai_msg::Body::Text),
        };
        let Some(body) = body else {
            // Not an LLM message here (12e and 12f add theirs): the turn closes at once.
            cx.broker.turn_done(chat);
            return Self::next(cx, chat);
        };
        if let Some(a) = cx.agents.get_mut(chat) {
            if !a.in_turn {
                a.in_turn = true;
                a.gate_refused = false;
                a.files_failed = false;
                a.question = Some(user.content.clone());
                cx.broker.typing(chat, true, Participant::Agent);
                cx.broker.turn(chat, TurnEvent::Started);
            }
        }
        let Some(model) = (cx.model_for)(&row) else {
            Self::agent_says(cx, chat, Content::Markdown(crate::ai_turn::NO_MODEL.to_string()));
            return Self::end_turn(cx, chat);
        };
        if let Some(a) = cx.agents.get_mut(chat) {
            a.model = Some(model.clone());
        }
        let endpoint = endpoint_of(&model);
        let Some(passed) = chat_gate::pass(cx.globals, &endpoint) else {
            let mut fx = vec![EngineEffect::GateRefused { endpoint, silent: false }];
            fx.extend(Self::end_turn(cx, chat));
            return fx;
        };
        // The whole stored log but this message and the ones still queued behind it: a reply
        // and tool rows stored while this message waited are part of its history (iTerm2's
        // `historyExcluding`, `ChatService.swift:517-525`).
        let queued = cx.broker.queued_ids(chat);
        let before: Vec<Message> = cx.store.messages(chat).unwrap_or_default().into_iter().filter(|m| m.unique_id != user.unique_id && !queued.contains(&m.unique_id)).collect();
        // `ChatProviderBinding`: a chat is bound to its first model's provider.
        match &row.model_name {
            None => {
                let bound = Chat { model_name: Some(model.name.clone()), ..row.clone() };
                let _ = cx.store.update_chat(&bound);
            }
            Some(bound) if *bound != model.name && crate::ai_models::vendor_for_model_name(bound).is_some_and(|v| v != model.vendor) && before.iter().any(|m| locks_provider(&m.content)) => {
                Self::agent_says(cx, chat, Content::Markdown(chat_agent::provider_lock_text(bound, &model.name)));
                return Self::end_turn(cx, chat);
            }
            Some(_) => {}
        }
        if allow_files && model.vector_store == VectorStore::OpenAI {
            let files = chat_agent::attached_files(&user.content);
            if !files.is_empty() {
                if let Some(a) = cx.agents.get_mut(chat) {
                    a.held = Some(user);
                }
                return Self::files_start(cx, chat, files);
            }
        }
        let Some(a) = cx.agents.get_mut(chat) else { return Vec::new() };
        let allowed = a.allowed.clone().unwrap_or_default();
        let functions = has(&model, Feature::FunctionCalling);
        let key = chat_agent::prompt_key(functions, &allowed);
        a.system_dirty = false;
        a.update_system(chat_agent::system_message(&cx.globals.string(key.global_key())));
        let mut messages = vec![ai_msg::Message::text(Role::System, &a.system)];
        messages.extend(chat_agent::history(&before));
        messages.push(ai_msg::Message::new(Some(Role::User), body));
        let options = chat_agent::turn_options(cx.globals, &model);
        let stores = if model.vector_store == VectorStore::OpenAI && !a.files_failed { row.vector_store.clone().into_iter().collect() } else { Vec::new() };
        let volatile = a.volatile.clone();
        let input = TurnInput {
            model: model.clone(),
            messages,
            tools: if functions { crate::ai_tools::tool_defs(&allowed) } else { Vec::new() },
            stream: has(&model, Feature::Streaming),
            hosted: Hosted { file_search_vector_stores: stores, web_search: options.web_search, code_interpreter: has(&model, Feature::HostedCodeInterpreter) },
            previous_response_id: None,
            should_think: options.should_think,
            effort: options.effort,
            tier: options.tier,
            volatile: Box::new(move || volatile.lock().map(|v| v.clone()).unwrap_or(None)),
            frozen: None,
            wire: Wire::from_globals(cx.globals),
            legacy_url_pref: cx.globals.string(crate::ai_settings::URL),
        };
        let mut driver = (cx.new_turn)(&model);
        let outs = send_start(&mut *driver, passed, input);
        if let Some(a) = cx.agents.get_mut(chat) {
            a.driver = Some(driver);
        }
        Self::outs(cx, chat, outs)
    }

    /// What a turn's driver said.
    fn outs(cx: &mut Ctx, chat: &str, outs: Vec<Out>) -> Fx {
        let mut fx = Vec::new();
        for o in outs {
            // A stopped turn reads nothing more.
            if !Self::in_turn(cx, chat) {
                break;
            }
            match o {
                Out::NeedKey(vendor) => {
                    if let Some(model) = cx.agents.get(chat).and_then(|a| a.model.as_ref()) {
                        fx.push(EngineEffect::AskKey { vendor, endpoint: endpoint_of(model), then: KeyThen::Turn(chat.to_string()) });
                    }
                }
                // The driver sends its own requests.
                Out::Send(_) => {}
                Out::WillSend => fx.push(EngineEffect::VolatileSent(chat.to_string())),
                Out::Delta(text) => Self::chunk(cx, chat, Chunk::Text(text)),
                Out::Attachment(a) => Self::chunk(cx, chat, Chunk::Attachment(a)),
                Out::Reasoning(text) => Self::reasoning(cx, chat, &text),
                Out::Invoke { call, id } => {
                    Self::commit(cx, chat);
                    fx.extend(Self::invoke(cx, chat, call, id));
                }
                Out::Choice(text) => {
                    Self::reply(cx, chat, text);
                    fx.extend(Self::end_turn(cx, chat));
                    break;
                }
                Out::StreamEnd | Out::Canceled => {
                    fx.extend(Self::end_turn(cx, chat));
                    break;
                }
                Out::Failed(e) => {
                    Self::commit(cx, chat);
                    Self::agent_says(cx, chat, Content::Markdown(e));
                    fx.extend(Self::end_turn(cx, chat));
                    break;
                }
            }
        }
        fx
    }

    /// A streamed chunk: the first publishes the reply, later ones append to it.
    fn chunk(cx: &mut Ctx, chat: &str, chunk: Chunk) {
        let Some(a) = cx.agents.get_mut(chat) else { return };
        let streaming = a.streaming_id.clone();
        let (fresh, more) = match chunk {
            Chunk::Text(text) => match &streaming {
                None => (Some((chat_agent::reply_content(a.question.as_ref(), &text, true, ""), a.reasoning.take())), None),
                Some(id) => (None, Some(Content::Append { text, uuid: id.clone() })),
            },
            Chunk::Attachment(att) => {
                let json = match &att {
                    Attachment::Status { id, inline, .. } => chat_agent::encode_wire_attachment(id, *inline, &att),
                    _ => {
                        let id = chat_agent::attachment_id(a, chat_agent::attachment_kind(&att));
                        chat_agent::encode_wire_attachment(&id, chat_agent::attachment_inline(&att), &att)
                    }
                };
                match &streaming {
                    None => (Some((Content::Multipart { parts: vec![Subpart::Attachment(json)], vector_store_id: None }, a.reasoning.take())), None),
                    Some(id) => (None, Some(Content::AppendAttachment { attachment: json, uuid: id.clone() })),
                }
            }
        };
        if let Some((content, reasoning)) = fresh {
            let m = Message { agent_reasoning: reasoning, ..Message::new(chat, Participant::Agent, content, cx.now) };
            a.streaming_id = Some(m.unique_id.clone());
            Self::put(cx, chat, m, true);
        } else if let Some(content) = more {
            let m = Message::new(chat, Participant::Agent, content, cx.now);
            Self::put(cx, chat, m, true);
        }
    }

    /// The reply's reasoning, kept on its message (ruling I-3).
    fn reasoning(cx: &mut Ctx, chat: &str, text: &str) {
        let Some(a) = cx.agents.get_mut(chat) else { return };
        match a.streaming_id.clone() {
            Some(id) => {
                let _ = cx.store.append_reasoning(&id, text);
            }
            None => a.reasoning.get_or_insert_with(String::new).push_str(text),
        }
    }

    /// A reply that did not stream.
    fn reply(cx: &mut Ctx, chat: &str, text: String) {
        let Some(a) = cx.agents.get_mut(chat) else { return };
        let content = chat_agent::reply_content(a.question.as_ref(), &text, false, "");
        let m = Message { agent_reasoning: a.reasoning.take(), ..Message::new(chat, Participant::Agent, content, cx.now) };
        Self::put(cx, chat, m, false);
    }

    /// The model called a tool: parsed, then its request routed (`runRemoteCommand`,
    /// `CA:2110-2162`). A call that does not parse is the tool's failure, which 12b counts.
    fn invoke(cx: &mut Ctx, chat: &str, call: FunctionCall, id: Option<CallId>) -> Fx {
        let name = call.name.clone().unwrap_or_default();
        let parsed = arguments_json(call.arguments.as_deref()).and_then(|args| crate::ai_tools::parse(&name, &args));
        let tool = match parsed {
            Ok(tool) => tool,
            Err(e) => return Self::continue_turn(cx, chat, ToolResult::Failed(e)),
        };
        // Reasoning that came before the call belongs to the call's message, not the next reply.
        let reasoning = cx.agents.get_mut(chat).and_then(|a| a.reasoning.take());
        let request = Message {
            agent_reasoning: reasoning,
            ..Message::new(
                chat,
                Participant::Agent,
                Content::RemoteCommandRequest { payload: Payload::Classic(RemoteCommand { llm_message: chat_agent::call_json(&call, id.as_ref()), call: tool }), safe: None },
                cx.now,
            )
        };
        Self::route(cx, chat, request)
    }

    /// The session a call acts in: the chat's linked guid for its kind and its live pane.
    fn linked(cx: &Ctx, chat: &str, call: &Call) -> (Option<String>, Option<PaneId>) {
        let row = cx.store.chat(chat).ok().flatten();
        let guid = row.and_then(|r| if call.category().is_browser() { r.browser_session_guid } else { r.terminal_session_guid });
        let pane = guid.as_deref().and_then(|g| (cx.pane_of)(g));
        (guid, pane)
    }

    /// An `execute_command` is refused for a control key first, then checked on its pane's
    /// whole staged line (the App's `Check` arm); every other call is gated at once.
    fn route(cx: &mut Ctx, chat: &str, request: Message) -> Fx {
        let Some(rc) = classic(&request) else { return Vec::new() };
        if let Call::ExecuteCommand { command } = &rc.call {
            if let Some(refused) = crate::ai_exec::refusal(command) {
                let id = request.unique_id.clone();
                Self::publish_or_replace(cx, chat, request);
                return Self::answer(cx, chat, &id, refused, false);
            }
            let command = command.clone();
            if let (_, Some(pane)) = Self::linked(cx, chat, &rc.call) {
                return vec![EngineEffect::Check { chat: chat.to_string(), pane, command, request }];
            }
        }
        Self::checked(cx, chat, request, None, None)
    }

    /// A request published once; a republished one keeps its row.
    fn publish_or_replace(cx: &mut Ctx, chat: &str, m: Message) {
        if cx.store.message(&m.unique_id).ok().flatten().is_some() {
            let _ = cx.store.replace_content(&m.unique_id, &m.content);
        } else {
            Self::put(cx, chat, m, false);
        }
    }

    /// The check's verdict (or none): the request is published with it, then the permission
    /// gate decides (`processRemoteCommandRequest`, `CC:126-181`).
    pub(super) fn checked(cx: &mut Ctx, chat: &str, mut request: Message, send: Option<String>, safe: Option<bool>) -> Fx {
        if !Self::in_turn(cx, chat) {
            return Vec::new();
        }
        let Content::RemoteCommandRequest { payload: Payload::Classic(rc), .. } = &request.content else { return Vec::new() };
        let rc = rc.clone();
        request.content = Content::RemoteCommandRequest { payload: Payload::Classic(rc.clone()), safe };
        let request_id = request.unique_id.clone();
        Self::publish_or_replace(cx, chat, request.clone());
        let (guid, pane) = Self::linked(cx, chat, &rc.call);
        let g = cx.globals;
        let global = |c: Category| g.int(c.global_key());
        let p = cx.overrides.get(chat, guid.as_deref().unwrap_or(""), rc.call.category(), &global);
        let call_id = chat_agent::call_from_json(&rc.llm_message).and_then(|(c, id)| id.map(|i| i.call_id).or(c.id)).unwrap_or_default();
        let park = |guid: String| Park { call_id: call_id.clone(), request_id: request_id.clone(), call: rc.call.clone(), safe, cleared_typing: true, guid, send: send.clone() };
        match (ai_permissions::gate(pane.is_some(), p, safe), guid, pane) {
            (Gate::Run, Some(guid), Some(pane)) => Self::run(cx, chat, &request_id, guid, pane, rc, send),
            (Gate::Deny, ..) => Self::answer(cx, chat, &request_id, Outcome { model: ai_permissions::DENIED_MODEL.to_string(), notice: Some(ai_permissions::DENIED_NOTICE.to_string()) }, false),
            (Gate::Park, Some(guid), _) => {
                if let Some(a) = cx.agents.get_mut(chat) {
                    a.parks.push(park(guid));
                }
                cx.broker.typing(chat, false, Participant::Agent);
                Vec::new()
            }
            // No live session of the call's kind: the person picks one.
            _ => {
                let terminal = !rc.call.category().is_browser();
                Self::agent_says(cx, chat, Content::SelectSessionRequest { original: Box::new(request), terminal });
                if let Some(a) = cx.agents.get_mut(chat) {
                    a.parks.push(park(String::new()));
                }
                cx.broker.typing(chat, false, Participant::Agent);
                Vec::new()
            }
        }
    }

    /// The one dispatch: the tool runs on the chat's pane (`performRemoteCommand`,
    /// `CC:204-231`), with its status notice, or for a command the executing bubble.
    fn run(cx: &mut Ctx, chat: &str, request_id: &str, guid: String, pane: PaneId, rc: RemoteCommand, send: Option<String>) -> Fx {
        // Every dispatch passes the turn's gate here, whichever way it got here (the permission
        // gate, a button, a session picker's answer): nothing runs while it is closed.
        let Some(model) = cx.agents.get(chat).and_then(|a| a.model.clone()) else { return Vec::new() };
        let endpoint = endpoint_of(&model);
        if !chat_gate::gate_open(cx.globals, &endpoint) {
            return Self::refuse(cx, chat, endpoint);
        }
        if let Some(a) = cx.agents.get_mut(chat) {
            a.running = Some((request_id.to_string(), guid));
        }
        cx.broker.typing(chat, true, Participant::Agent);
        match rc.call.status_notice() {
            Some(text) => Self::notice(cx, chat, &text),
            None => Self::agent_says(cx, chat, Content::ClientLocal(ClientLocal::ExecutingCommand(rc.clone()))),
        }
        vec![EngineEffect::RunTool { chat: chat.to_string(), request_id: request_id.to_string(), pane, call: rc.call, send }]
    }

    /// The function name and call id a response quotes, from its stored request.
    fn response_meta(cx: &Ctx, request_id: &str) -> (String, Option<serde_json::Value>) {
        let rc = cx.store.message(request_id).ok().flatten().and_then(|m| classic(&m).cloned());
        let Some(rc) = rc else { return ("Unknown function call name".to_string(), None) };
        match chat_agent::call_from_json(&rc.llm_message) {
            Some((call, id)) => {
                let id = id.or_else(|| call.id.clone().map(|i| CallId { call_id: i.clone(), item_id: i }));
                (call.name.unwrap_or_else(|| rc.call.name().to_string()), id.as_ref().map(chat_agent::response_call_id))
            }
            None => (rc.call.name().to_string(), None),
        }
    }

    /// A request's answer: its notice, the hidden response, then the turn goes on
    /// (`respondSuccessfullyToRemoteCommandRequest`, `CC:232-252`).
    fn answer(cx: &mut Ctx, chat: &str, request_id: &str, o: Outcome, resume_typing: bool) -> Fx {
        if let Some(n) = &o.notice {
            Self::notice(cx, chat, n);
        }
        let (function_name, function_call_id) = Self::response_meta(cx, request_id);
        let response = Content::RemoteCommandResponse { result: Ok(o.model.clone()), request_id: request_id.to_string(), function_name, function_call_id };
        let m = Message::new(chat, Participant::User, response, cx.now);
        Self::put(cx, chat, m, false);
        if resume_typing && Self::in_turn(cx, chat) {
            cx.broker.typing(chat, true, Participant::Agent);
        }
        Self::continue_turn(cx, chat, ToolResult::Ok(o.model))
    }

    /// The tool's result to the model, after the turn's gate (ruling R12cd-1).
    fn continue_turn(cx: &mut Ctx, chat: &str, r: ToolResult) -> Fx {
        let Some(model) = cx.agents.get(chat).filter(|a| a.in_turn).and_then(|a| a.model.clone()) else { return Vec::new() };
        let endpoint = endpoint_of(&model);
        let Some(passed) = chat_gate::pass(cx.globals, &endpoint) else { return Self::refuse(cx, chat, endpoint) };
        let outs = match cx.agents.get_mut(chat).and_then(|a| a.driver.as_mut()) {
            Some(d) => send_tool_done(&mut **d, passed, r),
            None => Vec::new(),
        };
        Self::outs(cx, chat, outs)
    }

    /// A closed gate mid-turn: the chat stops as Stop does, and the refusal is told once,
    /// silently (the App shows nothing more).
    fn refuse(cx: &mut Ctx, chat: &str, endpoint: Endpoint) -> Fx {
        let first = cx.agents.get_mut(chat).is_some_and(|a| !std::mem::replace(&mut a.gate_refused, true));
        let mut fx = Self::stop(cx, chat);
        if first {
            fx.push(EngineEffect::GateRefused { endpoint, silent: true });
        }
        fx
    }

    /// A tool's result from the App (`ai_tool_finished`); one for a request that is not
    /// running (an orphan after Stop) is dropped.
    pub(super) fn tool_finished(cx: &mut Ctx, chat: &str, request_id: &str, o: Outcome) -> Fx {
        let Some(a) = cx.agents.get_mut(chat) else { return Vec::new() };
        if a.running.as_ref().is_none_or(|(r, _)| r != request_id) {
            return Vec::new();
        }
        a.running = None;
        Self::answer(cx, chat, request_id, o, false)
    }

    /// A button on a parked request (`CVC:1826-1896`). The park runs only in the session it
    /// was asked for (minor m5), and only while the turn's gate is open (R13x8ai item 2).
    pub(super) fn answer_request(cx: &mut Ctx, chat: &str, request_id: &str, c: Choice) -> Fx {
        // The row is read before the park is taken, so a failed read leaves the park to answer.
        let Some(row) = cx.store.chat(chat).ok().flatten() else { return Vec::new() };
        let Some(park) = cx.agents.get_mut(chat).and_then(|a| a.take_park(request_id)) else { return Vec::new() };
        let category = park.call.category();
        let current = if category.is_browser() { row.browser_session_guid } else { row.terminal_session_guid };
        let live = current.and_then(|g| (cx.pane_of)(&g).map(|p| (g, p)));
        let Some((guid, pane)) = live else {
            Self::notice(cx, chat, ai_permissions::NOT_LINKED_NOTICE);
            let o = Outcome { model: ai_permissions::NOT_LINKED_MODEL.to_string(), notice: Some(ai_permissions::NOT_LINKED_USER_NOTICE.to_string()) };
            return Self::answer(cx, chat, request_id, o, true);
        };
        if guid != park.guid {
            let o = Outcome { model: chat_agent::RELINKED_MODEL.to_string(), notice: Some(chat_agent::RELINKED_NOTICE.to_string()) };
            return Self::answer(cx, chat, request_id, o, true);
        }
        // Always Allow and Always Deny write the chat's own override (never the globals).
        let always = match c {
            Choice::AlwaysAllow => Some(Permission::Always),
            Choice::AlwaysDeny => Some(Permission::Never),
            _ => None,
        };
        // A change that holds for this run but was not written is reported to the person.
        let mut fx = Vec::new();
        if let Some(p) = always {
            if set_permission_in(cx.store, cx.overrides, Key { chat: chat.to_string(), guid: guid.clone(), category }, p).is_some_and(|(_, saved)| !saved) {
                fx.push(EngineEffect::PermissionNotSaved);
            }
        }
        fx.extend(match c {
            Choice::AllowOnce | Choice::AlwaysAllow => {
                let rc = cx.store.message(request_id).ok().flatten().and_then(|m| classic(&m).cloned());
                let rc = RemoteCommand { call: park.call, ..rc.unwrap_or(RemoteCommand { llm_message: serde_json::json!({}), call: Call::IsAtPrompt }) };
                Self::run(cx, chat, request_id, guid, pane, rc, park.send)
            }
            Choice::DenyOnce | Choice::AlwaysDeny => Self::answer(cx, chat, request_id, Outcome { model: ai_permissions::DECLINED_MODEL.to_string(), notice: None }, true),
        });
        fx
    }

    /// The session picker's answer (`CVC:1770-1805`). Linking is the App's (minor m14): the
    /// request is published again once the link is applied.
    pub(super) fn session_selected(cx: &mut Ctx, chat: &str, request_id: &str, pane: Option<PaneId>) -> Fx {
        if cx.agents.get_mut(chat).and_then(|a| a.take_park(request_id)).is_none() {
            return Vec::new();
        }
        match pane {
            None => Self::answer(cx, chat, request_id, Outcome { model: ai_permissions::PICKER_DECLINED_MODEL.to_string(), notice: None }, true),
            Some(pane) => vec![EngineEffect::Link { chat: chat.to_string(), pane }, EngineEffect::Republish { chat: chat.to_string(), request_id: request_id.to_string() }],
        }
    }

    /// A request routed again after its chat was linked: it follows the new session's gate.
    pub(super) fn republish(cx: &mut Ctx, chat: &str, request_id: &str) -> Fx {
        if !Self::in_turn(cx, chat) {
            return Vec::new();
        }
        let Some(request) = cx.store.message(request_id).ok().flatten().filter(|m| classic(m).is_some()) else { return Vec::new() };
        cx.broker.typing(chat, true, Participant::Agent);
        Self::route(cx, chat, request)
    }

    /// The executing bubble's Cancel (`cancelRemoteCommand`, `PS:476-491`): the command is
    /// forgotten and answered as canceled; the turn goes on.
    pub(super) fn cancel_tool(cx: &mut Ctx, chat: &str) -> Fx {
        let Some((request_id, guid)) = cx.agents.get_mut(chat).and_then(|a| a.running.take()) else { return Vec::new() };
        let mut fx: Fx = (cx.pane_of)(&guid).map(EngineEffect::CancelExec).into_iter().collect();
        fx.extend(Self::answer(cx, chat, &request_id, crate::ai_exec::canceled_outcome(), false));
        fx
    }

    /// Stop halts the whole pipeline: the queued backlog goes, then the turn
    /// (`ChatService.swift:307-333`).
    pub(super) fn stop(cx: &mut Ctx, chat: &str) -> Fx {
        cx.broker.clear_queue(chat);
        Self::halt(cx, chat)
    }

    /// The turn cancelled (`ChatAgent.stop`, `CA:1225-1306`): the request, every park, a
    /// running command and the checks; nothing it was waiting for is used afterwards.
    fn halt(cx: &mut Ctx, chat: &str) -> Fx {
        let Some(a) = cx.agents.get_mut(chat) else { return Vec::new() };
        let busy = a.in_turn || a.driver.is_some() || a.files.is_some() || a.held.is_some();
        if let Some(mut d) = a.driver.take() {
            let _ = d.cancel();
        }
        if let Some((_, mut f)) = a.files.take() {
            f.cancel();
        }
        a.stop();
        a.held = None;
        let running = a.running.take();
        let mut fx: Fx = running.and_then(|(_, guid)| (cx.pane_of)(&guid)).map(EngineEffect::CancelExec).into_iter().collect();
        fx.push(EngineEffect::CancelChecks(chat.to_string()));
        if busy {
            fx.extend(Self::end_turn(cx, chat));
        }
        fx
    }

    /// A deleted chat: stopped, and its agent dropped before it can publish again.
    pub(super) fn drop_chat(cx: &mut Ctx, chat: &str) -> Fx {
        let fx = Self::stop(cx, chat);
        cx.agents.remove(chat);
        fx
    }

    /// 12d's "Send Commands & Output to AI Automatically": a user message that starts a turn.
    pub(super) fn send_terminal_command(cx: &mut Ctx, chat: &str, t: TerminalCommand) -> Fx {
        let m = Message::new(chat, Participant::User, Content::TerminalCommand(t), cx.now);
        Self::publish(cx, chat, m, false).unwrap_or_default()
    }

    /// The key prompt's answer for the turn, after the turn's gate.
    pub(super) fn key_given(cx: &mut Ctx, chat: &str, key: Option<Secret>) -> Fx {
        let Some(model) = cx.agents.get(chat).filter(|a| a.in_turn && a.driver.is_some()).and_then(|a| a.model.clone()) else { return Vec::new() };
        let endpoint = endpoint_of(&model);
        let Some(passed) = chat_gate::pass(cx.globals, &endpoint) else { return Self::refuse(cx, chat, endpoint) };
        let outs = match cx.agents.get_mut(chat).and_then(|a| a.driver.as_mut()) {
            Some(d) => send_key(&mut **d, passed, key),
            None => Vec::new(),
        };
        Self::outs(cx, chat, outs)
    }

    /// Starts the chat's vector-store flow for `files` (ruling I-3, piece 4): the store is
    /// created once per chat, each file uploaded, then added.
    pub(super) fn files_start(cx: &mut Ctx, chat: &str, files: Vec<(String, Vec<u8>)>) -> Fx {
        Self::ensure_agent(cx, chat);
        let Some(row) = cx.store.chat(chat).ok().flatten() else { return Vec::new() };
        let model = cx.agents.get(chat).and_then(|a| a.model.clone()).or_else(|| (cx.model_for)(&row));
        let Some(model) = model else { return Self::files_end(cx, chat, false) };
        if let Some(a) = cx.agents.get_mut(chat) {
            a.model = Some(model);
        }
        Self::files_job(cx, chat, FileFlow { store: row.vector_store, queue: files, uploaded: Vec::new() })
    }

    /// The flow's next job, after the gate for api.openai.com (ruling R12cd-1).
    fn files_job(cx: &mut Ctx, chat: &str, flow: FileFlow) -> Fx {
        let title = cx.store.chat(chat).ok().flatten().map(|c| c.title).unwrap_or_default();
        let Some(model) = cx.agents.get(chat).and_then(|a| a.model.clone()) else { return Self::files_end(cx, chat, false) };
        let Some(job) = flow.next_job(&title) else { return Self::files_end(cx, chat, true) };
        let endpoint = files_endpoint(&model);
        let Some(passed) = chat_gate::pass(cx.globals, &endpoint) else {
            let mut fx = vec![EngineEffect::GateRefused { endpoint, silent: false }];
            fx.extend(Self::files_end(cx, chat, false));
            return fx;
        };
        let mut driver = (cx.new_files)(&model, job);
        let outs = send_file_job(&mut *driver, passed, None);
        if let Some(a) = cx.agents.get_mut(chat) {
            a.files = Some((flow, driver));
        }
        Self::file_outs(cx, chat, outs)
    }

    /// A job's results: a new store is published once, the next job starts, and the end of
    /// the flow releases the turn that waited for it.
    fn file_outs(cx: &mut Ctx, chat: &str, outs: Vec<FileOut>) -> Fx {
        let mut fx = Vec::new();
        for o in outs {
            let Some((mut flow, driver)) = cx.agents.get_mut(chat).and_then(|a| a.files.take()) else { break };
            match &o {
                FileOut::NeedKey(vendor) => {
                    let endpoint = cx.agents.get(chat).and_then(|a| a.model.as_ref()).map(files_endpoint);
                    if let Some(a) = cx.agents.get_mut(chat) {
                        a.files = Some((flow, driver));
                    }
                    if let Some(endpoint) = endpoint {
                        fx.push(EngineEffect::AskKey { vendor: *vendor, endpoint, then: KeyThen::Files(chat.to_string()) });
                    }
                }
                FileOut::Created(_) | FileOut::Uploaded(_) => {
                    drop(driver);
                    if let Some(id) = flow.advance(&o) {
                        Self::agent_says(cx, chat, Content::VectorStoreCreated(id));
                    }
                    fx.extend(Self::files_job(cx, chat, flow));
                }
                FileOut::Added => fx.extend(Self::files_end(cx, chat, true)),
                FileOut::Failed(e) => {
                    Self::notice(cx, chat, e);
                    if e == crate::ai_files::INGEST_ERROR {
                        Self::notice(cx, chat, crate::ai_files::NOT_WELL_FORMED);
                    }
                    fx.extend(Self::files_end(cx, chat, false));
                }
            }
        }
        fx
    }

    /// The flow ended; the turn that waited for it goes on (without the store when it failed).
    fn files_end(cx: &mut Ctx, chat: &str, ok: bool) -> Fx {
        let Some(a) = cx.agents.get_mut(chat) else { return Vec::new() };
        a.files = None;
        if !ok {
            a.files_failed = true;
        }
        match a.held.take() {
            Some(held) => Self::begin(cx, chat, held, false),
            None => Vec::new(),
        }
    }

    /// The key prompt's answer for a file job, after the gate for api.openai.com.
    pub(super) fn files_key(cx: &mut Ctx, chat: &str, key: Option<Secret>) -> Fx {
        let Some((flow, mut driver)) = cx.agents.get_mut(chat).and_then(|a| a.files.take()) else { return Vec::new() };
        let Some(model) = cx.agents.get(chat).and_then(|a| a.model.clone()) else { return Self::files_end(cx, chat, false) };
        let Some(key) = key else {
            Self::notice(cx, chat, crate::ai_turn::NO_KEY);
            return Self::files_end(cx, chat, false);
        };
        let endpoint = files_endpoint(&model);
        let Some(passed) = chat_gate::pass(cx.globals, &endpoint) else {
            let mut fx = vec![EngineEffect::GateRefused { endpoint, silent: true }];
            fx.extend(Self::files_end(cx, chat, false));
            return fx;
        };
        let outs = send_file_job(&mut *driver, passed, Some(&key));
        if let Some(a) = cx.agents.get_mut(chat) {
            a.files = Some((flow, driver));
        }
        Self::file_outs(cx, chat, outs)
    }

    /// Reads what every turn and file job has received. A file job whose gate closed is
    /// cancelled first and starts nothing more.
    pub(super) fn poll(cx: &mut Ctx) -> Fx {
        let mut fx = Vec::new();
        let chats: Vec<String> = cx.agents.keys().cloned().collect();
        for chat in chats {
            let (g, endpoint) = (cx.globals, cx.agents.get(&chat).and_then(|a| a.model.as_ref()).map(endpoint_of));
            let outs = cx.agents.get_mut(&chat).and_then(|a| a.driver.as_mut()).map(|d| d.poll(&mut || endpoint.as_ref().and_then(|e| chat_gate::pass(g, e)))).unwrap_or_default();
            fx.extend(Self::outs(cx, &chat, outs));
            let Some(model) = cx.agents.get(&chat).filter(|a| a.files.is_some()).and_then(|a| a.model.clone()) else { continue };
            let endpoint = files_endpoint(&model);
            if !chat_gate::gate_open(cx.globals, &endpoint) {
                fx.extend(Self::refuse(cx, &chat, endpoint));
                continue;
            }
            let outs = cx.agents.get_mut(&chat).and_then(|a| a.files.as_mut()).map(|(_, d)| d.poll(Instant::now())).unwrap_or_default();
            fx.extend(Self::file_outs(cx, &chat, outs));
        }
        fx
    }

    /// Ruling R12cd-1: every chat with a turn or a file flow whose gate closed (the setting, the
    /// administrator's switch, a forgotten consent) stops, its request cancelled.
    pub(super) fn gate_tick(cx: &mut Ctx) -> Fx {
        let mut fx = Vec::new();
        let chats: Vec<String> = cx.agents.keys().cloned().collect();
        for chat in chats {
            let Some(a) = cx.agents.get(&chat) else { continue };
            if !(a.in_turn || a.driver.is_some() || a.files.is_some()) {
                continue;
            }
            let Some(model) = a.model.clone() else { continue };
            let endpoint = if a.files.is_some() { files_endpoint(&model) } else { endpoint_of(&model) };
            if !chat_gate::gate_open(cx.globals, &endpoint) {
                fx.extend(Self::refuse(cx, &chat, endpoint));
            }
        }
        fx
    }
}

/// One streamed piece of a reply.
enum Chunk {
    Text(String),
    Attachment(Attachment),
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::chat_app::create_in;

    fn ask(_: Category) -> i64 {
        0
    }

    // The engine's tests (ruling R12cd-4): a `Ctx` over an in-memory store, with `FakeDriver`
    // and `FakeFiles` as the only doubles. Nothing reaches a pane or the network; each test reads
    // the effects the App would apply and feeds results back as the App would.
    use crate::ai_catalog::{Effort, Feature, Model, VectorStore};
    use crate::ai_files::{FileOut, Job};
    use crate::ai_msg::{Attachment, Body, CallId, FunctionCall, Status};
    use crate::ai_settings::{Api, Vendor, ADMIN, ENABLE_AI};
    use crate::ai_turn::{Hosted, Out, ToolResult, TurnInput};
    use crate::chat_broker::Update;
    use crate::chat_message::Subpart;
    use std::cell::RefCell;
    use std::collections::BTreeSet;
    use std::collections::VecDeque;
    use std::rc::Rc;
    use toml::Value as T;

    #[derive(Clone, Debug, PartialEq)]
    enum Ev {
        Start,
        Poll,
        Key(Option<String>),
        Tool(ToolResult),
        Cancel,
        FileStart(Option<String>),
        FileCancel,
    }

    /// What one `start` received.
    struct Started {
        messages: Vec<crate::ai_msg::Message>,
        tools: Vec<String>,
        hosted: Hosted,
        should_think: Option<bool>,
        effort: Option<Effort>,
    }

    #[derive(Default)]
    struct Log {
        events: Vec<Ev>,
        inputs: Vec<Started>,
        turns_made: usize,
        on_start: VecDeque<Vec<Out>>,
        on_poll: VecDeque<Vec<Out>>,
        on_tool: VecDeque<Vec<Out>>,
        jobs: Vec<Job>,
        file_start: VecDeque<Vec<FileOut>>,
        file_poll: VecDeque<Vec<FileOut>>,
    }

    struct FakeDriver(Rc<RefCell<Log>>);

    impl TurnDriver for FakeDriver {
        fn start(&mut self, input: TurnInput) -> Vec<Out> {
            let mut l = self.0.borrow_mut();
            l.events.push(Ev::Start);
            l.inputs.push(Started { messages: input.messages, tools: input.tools.iter().map(|t| t.name.clone()).collect(), hosted: input.hosted, should_think: input.should_think, effort: input.effort });
            l.on_start.pop_front().unwrap_or_default()
        }
        fn poll(&mut self, _: &mut dyn FnMut() -> Option<Passed>) -> Vec<Out> {
            let mut l = self.0.borrow_mut();
            l.events.push(Ev::Poll);
            l.on_poll.pop_front().unwrap_or_default()
        }
        fn key_given(&mut self, key: Option<Secret>) -> Vec<Out> {
            self.0.borrow_mut().events.push(Ev::Key(key.map(|k| k.expose().to_string())));
            Vec::new()
        }
        fn tool_done(&mut self, r: ToolResult) -> Vec<Out> {
            let mut l = self.0.borrow_mut();
            l.events.push(Ev::Tool(r));
            l.on_tool.pop_front().unwrap_or_default()
        }
        fn cancel(&mut self) -> Vec<Out> {
            self.0.borrow_mut().events.push(Ev::Cancel);
            vec![Out::Canceled]
        }
    }

    struct FakeFiles(Rc<RefCell<Log>>);

    impl FileDriver for FakeFiles {
        fn start(&mut self, key: Option<&Secret>) -> Vec<FileOut> {
            let mut l = self.0.borrow_mut();
            l.events.push(Ev::FileStart(key.map(|k| k.expose().to_string())));
            l.file_start.pop_front().unwrap_or_default()
        }
        fn poll(&mut self, _: Instant) -> Vec<FileOut> {
            self.0.borrow_mut().file_poll.pop_front().unwrap_or_default()
        }
        fn cancel(&mut self) {
            self.0.borrow_mut().events.push(Ev::FileCancel);
        }
    }

    struct H {
        store: Store,
        broker: Broker,
        overrides: Overrides,
        agents: HashMap<String, Agent>,
        g: Globals,
        log: Rc<RefCell<Log>>,
        model: Model,
        pinned: Model,
        live: Vec<PaneId>,
    }

    impl H {
        fn new() -> H {
            let mut g = Globals::empty(&std::env::temp_dir().join("tandem-chat-engine-test.toml"));
            g.set(ENABLE_AI, T::Boolean(true));
            crate::ai_gate::grant(&mut g, "api.openai.com");
            H {
                store: Store::open_in_memory().unwrap(),
                broker: Broker::new(),
                overrides: Overrides::default(),
                agents: HashMap::new(),
                g,
                log: Rc::new(RefCell::new(Log::default())),
                model: crate::ai_model::fixture("gpt-t", "https://api.openai.com/v1/responses", Api::Responses),
                pinned: crate::ai_model::fixture("p", "https://pinned.test/v1/chat/completions", Api::ChatCompletions),
                live: vec![7, 8, 9],
            }
        }

        fn run<R>(&mut self, f: impl FnOnce(&mut Ctx) -> R) -> R {
            let (model, pinned) = (self.model.clone(), self.pinned.clone());
            let model_for = move |c: &Chat| Some(if c.model_name.as_deref() == Some(pinned.name.as_str()) { pinned.clone() } else { model.clone() });
            let live = self.live.clone();
            let pane_of = move |g: &str| g.parse::<PaneId>().ok().filter(|p| live.contains(p));
            let log = self.log.clone();
            let mut new_turn = move |_: &Model| -> Box<dyn TurnDriver> {
                log.borrow_mut().turns_made += 1;
                Box::new(FakeDriver(log.clone()))
            };
            let log = self.log.clone();
            let mut new_files = move |_: &Model, job: Job| -> Box<dyn FileDriver> {
                log.borrow_mut().jobs.push(job);
                Box::new(FakeFiles(log.clone()))
            };
            let mut cx = Ctx {
                store: &mut self.store,
                broker: &mut self.broker,
                overrides: &mut self.overrides,
                agents: &mut self.agents,
                globals: &self.g,
                model_for: &model_for,
                pane_of: &pane_of,
                new_turn: &mut new_turn,
                new_files: &mut new_files,
                now: 10.0,
            };
            f(&mut cx)
        }

        fn chat_with(&mut self, guid: Option<&str>, model: Option<&str>) -> String {
            let g = &self.g;
            let global = |c: Category| g.int(c.global_key());
            create_in(&mut self.store, &mut self.broker, &mut self.overrides, "T", guid, None, "", vec![], model.map(str::to_string), &global, 1.0).unwrap()
        }

        fn chat(&mut self, guid: Option<&str>) -> String {
            self.chat_with(guid, None)
        }

        fn send(&mut self, chat: &str, content: Content) -> Vec<EngineEffect> {
            let m = Message::new(chat, Participant::User, content, 5.0);
            self.run(|cx| Engine::publish(cx, chat, m, false).unwrap())
        }

        fn say(&mut self, chat: &str, text: &str) -> Vec<EngineEffect> {
            self.send(chat, Content::PlainText { text: text.into(), context: None })
        }

        fn set(&mut self, chat: &str, guid: &str, c: Category, p: Permission) {
            set_permission_in(&mut self.store, &mut self.overrides, Key { chat: chat.into(), guid: guid.into(), category: c }, p);
        }

        fn permission(&self, chat: &str, guid: &str, c: Category) -> Permission {
            let g = &self.g;
            self.overrides.get(chat, guid, c, &|c: Category| g.int(c.global_key()))
        }

        fn relink(&mut self, chat: &str, guid: Option<&str>) {
            let row = self.store.chat(chat).unwrap().unwrap();
            self.store.update_chat(&Chat { terminal_session_guid: guid.map(str::to_string), ..row }).unwrap();
        }

        fn messages(&self, chat: &str) -> Vec<Message> {
            self.store.messages(chat).unwrap()
        }

        fn notices(&self, chat: &str) -> Vec<String> {
            self.messages(chat).into_iter().filter_map(|m| if let Content::ClientLocal(ClientLocal::Notice(n)) = m.content { Some(n) } else { None }).collect()
        }

        /// (request id, result text) of each stored response.
        fn responses(&self, chat: &str) -> Vec<(String, String)> {
            self.messages(chat)
                .into_iter()
                .filter_map(|m| if let Content::RemoteCommandResponse { result: Ok(t), request_id, .. } = m.content { Some((request_id, t)) } else { None })
                .collect()
        }

        fn requests(&self, chat: &str) -> Vec<Message> {
            self.messages(chat).into_iter().filter(|m| matches!(m.content, Content::RemoteCommandRequest { .. })).collect()
        }

        fn agent_text(&self, chat: &str) -> Vec<String> {
            self.messages(chat).into_iter().filter(|m| m.author == Participant::Agent).filter_map(|m| if let Content::Markdown(t) = m.content { Some(t) } else { None }).collect()
        }

        fn script(&self, outs: Vec<Out>) {
            self.log.borrow_mut().on_start.push_back(outs);
        }

        fn after_tool(&self, outs: Vec<Out>) {
            self.log.borrow_mut().on_tool.push_back(outs);
        }

        fn events(&self) -> Vec<Ev> {
            self.log.borrow().events.clone()
        }

        fn count(&self, e: &Ev) -> usize {
            self.log.borrow().events.iter().filter(|x| *x == e).count()
        }

        fn park_ids(&self, chat: &str) -> Vec<String> {
            self.agents.get(chat).map(|a| a.parks.iter().map(|p| p.request_id.clone()).collect()).unwrap_or_default()
        }

        fn in_turn(&self, chat: &str) -> bool {
            self.agents.get(chat).is_some_and(|a| a.in_turn)
        }

        fn off(&mut self) {
            self.g.set(ENABLE_AI, T::Boolean(false));
        }

        fn on(&mut self) {
            self.g.set(ENABLE_AI, T::Boolean(true));
        }
    }

    fn cid(s: &str) -> CallId {
        CallId { call_id: s.into(), item_id: s.into() }
    }

    fn fcall(name: &str, args: &str) -> FunctionCall {
        FunctionCall { name: Some(name.into()), arguments: Some(args.into()), ..Default::default() }
    }

    fn invoke(name: &str, args: &str, id: &str) -> Out {
        Out::Invoke { call: fcall(name, args), id: Some(cid(id)) }
    }

    fn screen(id: &str) -> Out {
        invoke("get_screen_contents", "{\"lines\":0}", id)
    }

    fn ran(model: &str, notice: Option<&str>) -> Outcome {
        Outcome { model: model.into(), notice: notice.map(str::to_string) }
    }

    fn run_ids(fx: &[EngineEffect]) -> Vec<(String, PaneId, Call, Option<String>)> {
        fx.iter().filter_map(|e| if let EngineEffect::RunTool { request_id, pane, call, send, .. } = e { Some((request_id.clone(), *pane, call.clone(), send.clone())) } else { None }).collect()
    }

    fn check_of(fx: &[EngineEffect]) -> (PaneId, String, Message) {
        match fx {
            [EngineEffect::Check { pane, command, request, .. }] => (*pane, command.clone(), request.clone()),
            other => panic!("one Check expected: {other:?}"),
        }
    }

    fn silent_refusal(fx: &[EngineEffect]) -> bool {
        fx.iter().any(|e| matches!(e, EngineEffect::GateRefused { silent: true, .. }))
    }

    #[test]
    fn tool_calls_pass_the_gate() {
        let mut h = H::new();
        let chat = h.chat(Some("7"));
        h.script(vec![invoke("execute_command", "{\"command\":\"ls\"}", "c1")]);
        let fx = h.say(&chat, "hi");
        let (pane, command, request) = check_of(&fx);
        assert_eq!((pane, command.as_str()), (7, "ls"));
        assert_eq!(classic(&request).unwrap().llm_message, chat_agent::call_json(&fcall("execute_command", "{\"command\":\"ls\"}"), Some(&cid("c1"))), "ruling 12c-1");
        assert!(h.requests(&chat).is_empty(), "nothing is published before the check");

        let rid = request.unique_id.clone();
        assert_eq!(h.run(|cx| Engine::checked(cx, &chat, request, None, None)), vec![], "Ask parks");
        assert!(matches!(h.requests(&chat)[0].content, Content::RemoteCommandRequest { safe: None, .. }));
        assert_eq!(h.agents[&chat].parks[0].guid, "7");
        let fx = h.run(|cx| Engine::answer_request(cx, &chat, &rid, Choice::AllowOnce));
        assert_eq!(fx, vec![EngineEffect::RunTool { chat: chat.clone(), request_id: rid.clone(), pane: 7, call: Call::ExecuteCommand { command: "ls".into() }, send: None }]);

        h.after_tool(vec![invoke("execute_command", "{\"command\":\"pwd\"}", "c2")]);
        let fx = h.run(|cx| Engine::tool_finished(cx, &chat, &rid, ran("out", Some("Ran ls"))));
        assert!(h.events().contains(&Ev::Tool(ToolResult::Ok("out".into()))));
        let response = h.messages(&chat).into_iter().find(|m| matches!(m.content, Content::RemoteCommandResponse { .. })).unwrap();
        assert!(crate::chat_message::hidden(&response.content));
        assert_eq!(
            response.content,
            Content::RemoteCommandResponse { result: Ok("out".into()), request_id: rid.clone(), function_name: "execute_command".into(), function_call_id: Some(serde_json::json!({"callID":"c1","itemID":"c1"})) }
        );
        assert_eq!(h.notices(&chat), vec!["Ran ls".to_string()]);

        // At Always with no verdict the request runs at once and nothing parks.
        h.set(&chat, "7", Category::RunCommands, Permission::Always);
        let (_, _, request) = check_of(&fx);
        let rid2 = request.unique_id.clone();
        let fx = h.run(|cx| Engine::checked(cx, &chat, request, None, None));
        assert_eq!(run_ids(&fx), vec![(rid2.clone(), 7, Call::ExecuteCommand { command: "pwd".into() }, None)]);
        assert!(h.park_ids(&chat).is_empty());

        // Always Allow writes Always for (chat, "7", Run Commands) into the chat row.
        h.set(&chat, "7", Category::RunCommands, Permission::Ask);
        h.after_tool(vec![invoke("execute_command", "{\"command\":\"id\"}", "c3")]);
        let fx = h.run(|cx| Engine::tool_finished(cx, &chat, &rid2, ran("/", None)));
        let (_, _, request) = check_of(&fx);
        let rid3 = request.unique_id.clone();
        assert_eq!(h.run(|cx| Engine::checked(cx, &chat, request, None, None)), vec![]);
        let fx = h.run(|cx| Engine::answer_request(cx, &chat, &rid3, Choice::AlwaysAllow));
        assert_eq!(run_ids(&fx).len(), 1);
        assert_eq!(h.permission(&chat, "7", Category::RunCommands), Permission::Always);
        let mut stored = Overrides::default();
        stored.merge_encoded(&chat, &h.store.chat(&chat).unwrap().unwrap().permissions);
        assert_eq!(stored.get(&chat, "7", Category::RunCommands, &ask), Permission::Always);
    }

    #[test]
    fn denied_tools_never_run() {
        let mut h = H::new();
        let chat = h.chat(Some("7"));
        h.set(&chat, "7", Category::ViewContents, Permission::Never);
        h.script(vec![screen("c1")]);
        let fx = h.say(&chat, "hi");
        assert!(run_ids(&fx).is_empty());
        assert_eq!(h.events().last(), Some(&Ev::Tool(ToolResult::Ok(ai_permissions::DENIED_MODEL.into()))), "the denial is the tool's text");
        assert_eq!(h.notices(&chat), vec![ai_permissions::DENIED_NOTICE.to_string()]);
        assert_eq!(h.requests(&chat).len(), 1, "the request is kept before its answer");

        for (choice, stored) in [(Choice::DenyOnce, Permission::Ask), (Choice::AlwaysDeny, Permission::Never)] {
            let mut h = H::new();
            let chat = h.chat(Some("7"));
            h.script(vec![screen("c1")]);
            assert_eq!(h.say(&chat, "hi"), vec![]);
            let rid = h.park_ids(&chat)[0].clone();
            let fx = h.run(|cx| Engine::answer_request(cx, &chat, &rid, choice));
            assert!(run_ids(&fx).is_empty());
            assert_eq!(h.events().last(), Some(&Ev::Tool(ToolResult::Ok(ai_permissions::DECLINED_MODEL.into()))));
            assert!(h.notices(&chat).is_empty(), "no notice for a declined request");
            assert_eq!(h.permission(&chat, "7", Category::ViewContents), stored, "{choice:?}");
        }

        // No linked session: the person picks one.
        let mut h = H::new();
        let chat = h.chat(None);
        h.script(vec![screen("c1")]);
        assert_eq!(h.say(&chat, "hi"), vec![]);
        let request = h.requests(&chat).remove(0);
        let picker = h.messages(&chat).into_iter().find_map(|m| if let Content::SelectSessionRequest { original, terminal } = m.content { Some((original, terminal)) } else { None });
        assert_eq!(picker, Some((Box::new(Message { seq: 0, ..request.clone() }), true)), "the request as published");
        let fx = h.run(|cx| Engine::session_selected(cx, &chat, &request.unique_id, None));
        assert!(run_ids(&fx).is_empty());
        assert_eq!(h.events().last(), Some(&Ev::Tool(ToolResult::Ok(ai_permissions::PICKER_DECLINED_MODEL.into()))));

        // A picked session is linked by the App (minor m14), then the request is gated there.
        let mut h = H::new();
        let chat = h.chat(None);
        h.set(&chat, "7", Category::ViewContents, Permission::Always);
        h.script(vec![screen("c1")]);
        h.say(&chat, "hi");
        let rid = h.requests(&chat)[0].unique_id.clone();
        let fx = h.run(|cx| Engine::session_selected(cx, &chat, &rid, Some(9)));
        assert_eq!(fx, vec![EngineEffect::Link { chat: chat.clone(), pane: 9 }, EngineEffect::Republish { chat: chat.clone(), request_id: rid.clone() }]);
        h.relink(&chat, Some("9"));
        assert_eq!(h.run(|cx| Engine::republish(cx, &chat, &rid)), vec![], "session 9 is at its default, Ask, not 7's Always");
        assert_eq!(h.agents[&chat].parks[0].guid, "9");
    }

    #[test]
    fn flagged_always_parks() {
        let mut h = H::new();
        let chat = h.chat(Some("7"));
        h.set(&chat, "7", Category::RunCommands, Permission::Always);
        h.script(vec![invoke("execute_command", "{\"command\":\"rm x\"}", "c1")]);
        let (_, _, request) = check_of(&h.say(&chat, "hi"));
        assert_eq!(h.run(|cx| Engine::checked(cx, &chat, request, None, Some(false))), vec![], "a flagged command parks at Always");
        assert!(matches!(h.requests(&chat)[0].content, Content::RemoteCommandRequest { safe: Some(false), .. }));
        assert_eq!(h.agents[&chat].parks[0].safe, Some(false));

        let other = h.chat(Some("7"));
        h.set(&other, "7", Category::RunCommands, Permission::Always);
        h.script(vec![invoke("execute_command", "{\"command\":\"ls\"}", "c1")]);
        let (_, _, request) = check_of(&h.say(&other, "hi"));
        assert_eq!(run_ids(&h.run(|cx| Engine::checked(cx, &other, request, None, Some(true)))).len(), 1);

        let third = h.chat(Some("7"));
        h.set(&third, "7", Category::ViewContents, Permission::Always);
        h.script(vec![screen("c1")]);
        let fx = h.say(&third, "hi");
        assert!(!fx.iter().any(|e| matches!(e, EngineEffect::Check { .. })), "only execute_command is checked");
        assert_eq!(run_ids(&fx).len(), 1);
    }

    #[test]
    fn agent_never_broadcasts() {
        // Pane 8 is live beside 7 (in a broadcast domain the App knows); the engine names only
        // the linked pane. `user_write(.., Write::Agent)` is Task 6's.
        let mut h = H::new();
        let chat = h.chat(Some("7"));
        h.set(&chat, "7", Category::ControlTerminal, Permission::Always);
        h.set(&chat, "7", Category::RunCommands, Permission::Always);
        h.script(vec![invoke("insert_text_at_cursor", "{\"text\":\"x\"}", "c1")]);
        let fx = h.say(&chat, "hi");
        assert_eq!(fx.len(), 1);
        assert_eq!(run_ids(&fx)[0].1, 7);
        let rid = run_ids(&fx)[0].0.clone();
        h.after_tool(vec![invoke("execute_command", "{\"command\":\"ls\"}", "c2")]);
        let fx = h.run(|cx| Engine::tool_finished(cx, &chat, &rid, ran("Inserted 1 characters.", None)));
        let (pane, _, request) = check_of(&fx);
        assert_eq!(pane, 7);
        let fx = h.run(|cx| Engine::checked(cx, &chat, request, None, Some(true)));
        assert_eq!(run_ids(&fx).iter().map(|r| r.1).collect::<Vec<_>>(), vec![7]);
        assert!(!format!("{fx:?}").contains("pane: 8"));
    }

    #[test]
    fn stop_follows_iterm2() {
        let mut h = H::new();
        let chat = h.chat(Some("7"));
        h.script(vec![screen("c1")]);
        h.say(&chat, "hi");
        let parked = h.park_ids(&chat)[0].clone();
        h.agents.get_mut(&chat).unwrap().running = Some(("R2".into(), "7".into()));
        let sub = h.broker.subscribe(Some(&chat));
        let fx = h.run(|cx| Engine::stop(cx, &chat));
        assert_eq!(fx, vec![EngineEffect::CancelExec(7), EngineEffect::CancelChecks(chat.clone())]);
        assert_eq!(h.count(&Ev::Cancel), 1);
        assert_eq!(h.events().last(), Some(&Ev::Cancel), "nothing after the cancel");
        let a = &h.agents[&chat];
        assert!(a.parks.is_empty() && a.running.is_none() && !a.in_turn);
        assert!(!h.broker.is_typing(&chat, Participant::Agent));
        assert!(h.broker.drain(sub).iter().any(|(_, u)| *u == Update::Turn(TurnEvent::Ended)));

        let before = h.messages(&chat).len();
        assert_eq!(h.run(|cx| Engine::tool_finished(cx, &chat, "R2", ran("late", None))), vec![], "an orphan is dropped");
        assert_eq!(h.run(|cx| Engine::answer_request(cx, &chat, &parked, Choice::AllowOnce)), vec![]);
        h.log.borrow_mut().on_poll.push_back(vec![Out::Delta("late".into())]);
        assert_eq!(h.run(|cx| Engine::poll(cx)), vec![]);
        assert_eq!(h.messages(&chat).len(), before, "no second write");
        assert_eq!(h.events().last(), Some(&Ev::Cancel));
    }

    #[test]
    fn wedged_turn_follows_iterm2() {
        let mut h = H::new();
        let chat = h.chat(Some("7"));
        h.script(vec![screen("c1")]);
        h.say(&chat, "first");
        assert!(h.agents[&chat].is_awaiting_approval());
        h.say(&chat, "second");
        assert_eq!(h.count(&Ev::Cancel), 1, "the wedged turn is cancelled");
        assert_eq!(h.count(&Ev::Start), 2);
        assert_eq!(h.log.borrow().inputs[1].messages.last(), Some(&crate::ai_msg::Message::text(Role::User, "second")));

        // A working turn (no park) keeps the new message queued.
        let mut h = H::new();
        let chat = h.chat(Some("7"));
        h.say(&chat, "a");
        h.say(&chat, "b");
        assert_eq!((h.count(&Ev::Start), h.count(&Ev::Cancel)), (1, 0));
        h.log.borrow_mut().on_poll.push_back(vec![Out::StreamEnd]);
        h.run(|cx| Engine::poll(cx));
        assert_eq!(h.count(&Ev::Start), 2);
        assert_eq!(h.log.borrow().inputs[1].messages.last(), Some(&crate::ai_msg::Message::text(Role::User, "b")));
    }

    #[test]
    fn queued_turn_sees_the_running_turns_reply() {
        // Task 11 review Important 1 (`historyExcluding`, `ChatService.swift:517-525`): the
        // reply and the tool rows stored after "b" was queued belong to b's history.
        let mut h = H::new();
        let chat = h.chat(Some("7"));
        h.script(vec![screen("c1")]);
        h.set(&chat, "7", Category::ViewContents, Permission::Always);
        let fx = h.say(&chat, "a");
        let rid = run_ids(&fx)[0].0.clone();
        h.say(&chat, "b");
        h.after_tool(vec![Out::Choice("A-reply".into())]);
        h.run(|cx| Engine::tool_finished(cx, &chat, &rid, ran("the screen", None)));
        assert_eq!(h.count(&Ev::Start), 2);
        let started = h.log.borrow();
        let messages = &started.inputs[1].messages;
        let has = |m: crate::ai_msg::Message| messages.contains(&m);
        assert!(has(crate::ai_msg::Message::text(Role::User, "a")), "{messages:?}");
        assert!(has(crate::ai_msg::Message::text(Role::Assistant, "A-reply")), "the previous reply: {messages:?}");
        assert_eq!(messages.last(), Some(&crate::ai_msg::Message::text(Role::User, "b")));
        assert_eq!(messages.iter().filter(|m| **m == crate::ai_msg::Message::text(Role::User, "b")).count(), 1, "the message itself only once");
        let pos = |m: crate::ai_msg::Message| messages.iter().position(|x| *x == m);
        assert!(pos(crate::ai_msg::Message::text(Role::User, "a")) < pos(crate::ai_msg::Message::text(Role::Assistant, "A-reply")));
    }

    #[test]
    fn a_still_queued_message_stays_out_of_the_history() {
        let mut h = H::new();
        let chat = h.chat(Some("7"));
        h.say(&chat, "a");
        h.say(&chat, "b");
        h.say(&chat, "c");
        h.log.borrow_mut().on_poll.push_back(vec![Out::Choice("A".into())]);
        h.run(|cx| Engine::poll(cx));
        let started = h.log.borrow();
        let messages = &started.inputs[1].messages;
        assert_eq!(messages.last(), Some(&crate::ai_msg::Message::text(Role::User, "b")));
        assert!(!messages.contains(&crate::ai_msg::Message::text(Role::User, "c")), "c is still queued: {messages:?}");
    }

    #[test]
    fn the_picker_answer_needs_the_gate() {
        // Task 11 review Minor 1: Link, Republish, route and checked reach `run` with no
        // other gate check, so `run` checks it (ruling R12cd-1).
        let mut h = H::new();
        let chat = h.chat(None);
        h.set(&chat, "7", Category::ViewContents, Permission::Always);
        h.script(vec![screen("c1")]);
        h.say(&chat, "hi");
        let rid = h.requests(&chat)[0].unique_id.clone();
        h.run(|cx| Engine::session_selected(cx, &chat, &rid, Some(7)));
        h.relink(&chat, Some("7"));
        h.off();
        let fx = h.run(|cx| Engine::republish(cx, &chat, &rid));
        assert!(run_ids(&fx).is_empty(), "nothing runs while the gate is closed: {fx:?}");
        assert!(silent_refusal(&fx));
        assert!(!h.in_turn(&chat));
    }

    #[test]
    fn a_failed_row_read_keeps_the_park() {
        // Task 11 review Minor 3: the park is taken only once the chat row is read.
        let mut h = H::new();
        let chat = h.chat(Some("7"));
        h.script(vec![screen("c1")]);
        h.say(&chat, "hi");
        let rid = h.park_ids(&chat)[0].clone();
        h.store.delete_chat(&chat).unwrap();
        assert_eq!(h.run(|cx| Engine::answer_request(cx, &chat, &rid, Choice::AllowOnce)), vec![]);
        assert_eq!(h.park_ids(&chat), vec![rid], "the park is still there to answer");
    }

    #[test]
    fn a_failed_always_save_is_reported() {
        // Final review Minor 3: Always from a request bubble tells the person an unsaved change.
        for (choice, fail) in [(Choice::AlwaysDeny, true), (Choice::AlwaysAllow, true), (Choice::AlwaysDeny, false), (Choice::DenyOnce, true)] {
            let mut h = H::new();
            let chat = h.chat(Some("7"));
            h.script(vec![screen("c1")]);
            h.say(&chat, "hi");
            let rid = h.park_ids(&chat)[0].clone();
            if fail {
                h.store.refuse_chat_updates();
            }
            let fx = h.run(|cx| Engine::answer_request(cx, &chat, &rid, choice));
            let told = fx.contains(&EngineEffect::PermissionNotSaved);
            assert_eq!(told, fail && choice != Choice::DenyOnce, "{choice:?} fail={fail}");
        }
    }

    #[test]
    fn reasoning_before_a_tool_call_stays_with_that_call() {
        // Task 11 review Minor 5.
        let mut h = H::new();
        let chat = h.chat(Some("7"));
        h.script(vec![Out::Reasoning("think".into()), screen("c1")]);
        h.say(&chat, "hi");
        let rid = h.park_ids(&chat)[0].clone();
        h.set(&chat, "7", Category::ViewContents, Permission::Always);
        h.after_tool(vec![Out::Delta("text".into()), Out::StreamEnd]);
        h.run(|cx| Engine::answer_request(cx, &chat, &rid, Choice::AllowOnce));
        h.run(|cx| Engine::tool_finished(cx, &chat, &rid, ran("s", None)));
        let request = h.requests(&chat).remove(0);
        assert_eq!(request.agent_reasoning.as_deref(), Some("think"), "the call's own message");
        let reply = h.messages(&chat).into_iter().find(|m| matches!(m.content, Content::Markdown(_))).unwrap();
        assert_eq!(reply.agent_reasoning, None, "not the next text reply");
    }

    #[test]
    fn streaming_follows_iterm2() {
        let mut h = H::new();
        let chat = h.chat(Some("7"));
        let sub = h.broker.subscribe(Some(&chat));
        h.script(vec![Out::Delta("He".into()), Out::Delta("llo".into()), Out::StreamEnd]);
        h.say(&chat, "q");
        assert_eq!(h.agent_text(&chat), vec!["Hello".to_string()]);
        let updates: Vec<Update> = h.broker.drain(sub).into_iter().map(|(_, u)| u).collect();
        let kinds: Vec<&str> = updates
            .iter()
            .filter_map(|u| match u {
                Update::Delivery { message, .. } if message.author == Participant::Agent => Some(match message.content {
                    Content::Markdown(_) => "first",
                    Content::Append { .. } => "append",
                    Content::Commit(_) => "commit",
                    _ => "other",
                }),
                Update::Typing { on: false, .. } => Some("typing off"),
                Update::Turn(TurnEvent::Ended) => Some("ended"),
                _ => None,
            })
            .collect();
        assert_eq!(kinds, ["first", "append", "commit", "typing off", "ended"]);

        h.script(vec![Out::Choice("x".into())]);
        h.say(&chat, "q2");
        assert_eq!(h.agent_text(&chat), vec!["Hello".to_string(), "x".to_string()]);
        h.script(vec![Out::Failed("E".into())]);
        h.say(&chat, "q3");
        assert_eq!(h.agent_text(&chat).last().map(String::as_str), Some("E"));
        assert!(!h.in_turn(&chat));
        h.script(vec![Out::WillSend]);
        assert_eq!(h.say(&chat, "q4"), vec![EngineEffect::VolatileSent(chat.clone())], "minor m2");

        let mut h = H::new();
        let chat = h.chat(Some("7"));
        let t = TerminalCommand { username: None, hostname: None, directory: None, command: "c".into(), output: "o".into(), exit_code: 0, url: String::new() };
        h.run(|cx| Engine::send_terminal_command(cx, &chat, t.clone()));
        assert!(h.messages(&chat).iter().any(|m| m.author == Participant::User && m.content == Content::TerminalCommand(t.clone())));
        assert_eq!(h.log.borrow().inputs[0].messages.last(), Some(&crate::ai_msg::Message::text(Role::User, &chat_agent::terminal_command_prompt(&t))));
    }

    #[test]
    fn permissions_rebuild_the_prompt() {
        let mut h = H::new();
        let chat = h.chat(Some("7"));
        h.script(vec![Out::StreamEnd]);
        h.say(&chat, "a");
        h.send(&chat, Content::SetPermissions(vec![Category::ViewContents]));
        h.script(vec![Out::StreamEnd]);
        h.say(&chat, "b");
        let log = h.log.borrow();
        let (first, second) = (&log.inputs[0], &log.inputs[1]);
        let only: BTreeSet<Category> = [Category::ViewContents].into_iter().collect();
        assert_eq!(second.tools, crate::ai_tools::tool_defs(&only).into_iter().map(|t| t.name).collect::<Vec<_>>());
        assert_ne!(first.tools, second.tools);
        assert_ne!(first.messages[0], second.messages[0], "a new system message");
        let ro = chat_agent::system_message(&h.g.string(chat_agent::PromptKey::ReadOnlyTerminal.global_key()));
        assert_eq!(second.messages[0], crate::ai_msg::Message::text(Role::System, &ro));
        assert!(h.agents[&chat].system_dirty);
        let first = Started { messages: vec![], tools: first.tools.clone(), hosted: Hosted::default(), should_think: None, effort: None };
        drop(log);

        // An agent the App made first (for the context snapshot) still takes the chat's set.
        let made = h.chat(Some("7"));
        h.agents.insert(made.clone(), Agent::default());
        h.script(vec![Out::StreamEnd]);
        h.say(&made, "c");
        assert_eq!(h.log.borrow().inputs[2].tools, first.tools.clone());

        // An orchestration chat's agent is 12f's: its set is not taken here.
        let orch = h.chat(Some("7"));
        let row = h.store.chat(&orch).unwrap().unwrap();
        h.store.update_chat(&Chat { orchestration_enabled: true, ..row }).unwrap();
        h.send(&orch, Content::SetPermissions(vec![]));
        let before = h.agents[&orch].allowed.clone();
        h.send(&orch, Content::SetPermissions(vec![Category::RunCommands]));
        assert_eq!(h.agents[&orch].allowed, before);
    }

    fn file_json() -> serde_json::Value {
        serde_json::json!({"inline":false,"id":"f1","type":{"file":{"_0":{"name":"a.txt","content":"aGk=","mimeType":"text/plain"}}}})
    }

    fn with_file() -> Content {
        Content::Multipart { parts: vec![Subpart::PlainText("hi".into()), Subpart::Attachment(file_json())], vector_store_id: None }
    }

    #[test]
    fn attachments_and_reasoning_stream_follows_iterm2() {
        let mut h = H::new();
        let chat = h.chat(Some("7"));
        let sub = h.broker.subscribe(Some(&chat));
        let status = Attachment::Status { id: "w1".into(), inline: true, status: Status::WebSearchStarted };
        h.script(vec![
            Out::Attachment(status.clone()),
            Out::Delta("Hi".into()),
            Out::Attachment(Attachment::Code("x".into())),
            Out::Reasoning("hmm".into()),
            Out::Reasoning("!".into()),
            Out::StreamEnd,
        ]);
        h.say(&chat, "q");
        let delivered: Vec<Message> = h.broker.drain(sub).into_iter().filter_map(|(_, u)| if let Update::Delivery { message, .. } = u { Some(message) } else { None }).filter(|m| m.author == Participant::Agent).collect();
        let first = &delivered[0];
        assert_eq!(first.content, Content::Multipart { parts: vec![Subpart::Attachment(chat_agent::encode_wire_attachment("w1", true, &status))], vector_store_id: None });
        let id = first.unique_id.clone();
        assert_eq!(delivered[1].content, Content::Append { text: "Hi".into(), uuid: id.clone() });
        let Content::AppendAttachment { attachment, uuid } = &delivered[2].content else { panic!("{:?}", delivered[2]) };
        assert_eq!(uuid, &id);
        let code_id = attachment["id"].as_str().unwrap().to_string();
        assert_eq!(*attachment, chat_agent::encode_wire_attachment(&code_id, true, &Attachment::Code("x".into())));
        assert_eq!(delivered[3].content, Content::Commit(id.clone()));
        assert_eq!(h.store.message(&id).unwrap().unwrap().agent_reasoning.as_deref(), Some("hmm!"));
        assert!(!h.broker.is_typing(&chat, Participant::Agent));
        assert!(!h.broker.turn_in_progress(&chat));
    }

    #[test]
    fn attached_files_reach_the_request() {
        let mut h = H::new();
        let chat = h.chat(Some("7"));
        h.send(&chat, with_file());
        let Content::Multipart { parts, .. } = with_file() else { unreachable!() };
        assert_eq!(h.log.borrow().inputs[0].messages.last(), Some(&crate::ai_msg::Message::new(Some(Role::User), chat_agent::multipart_body(&parts))));
    }

    #[test]
    fn turn_uses_the_options_struct() {
        let mut h = H::new();
        h.g.set(chat_agent::HIGH_EFFORT_KEY, T::Boolean(true));
        h.g.set(chat_agent::EFFORT_KEY, T::String("high".into()));
        h.g.set(chat_agent::WEB_SEARCH_KEY, T::Boolean(true));
        let plain = h.model.clone();
        h.model.features.extend([Feature::ConfigurableThinking, Feature::HostedWebSearch]);
        h.model.reasoning_efforts = vec![Effort::High];
        let chat = h.chat(Some("7"));
        h.say(&chat, "q");
        {
            let log = h.log.borrow();
            assert_eq!((log.inputs[0].should_think, log.inputs[0].effort, log.inputs[0].hosted.web_search), (Some(true), Some(Effort::High), true));
        }
        h.model = plain;
        let other = h.chat(Some("7"));
        h.say(&other, "q");
        let log = h.log.borrow();
        assert_eq!((log.inputs[1].should_think, log.inputs[1].effort, log.inputs[1].hosted.web_search), (None, None, false));
    }

    fn store_harness() -> H {
        let mut h = H::new();
        h.model.vector_store = VectorStore::OpenAI;
        h
    }

    #[test]
    fn vector_store_created_once_per_chat() {
        let mut h = store_harness();
        let chat = h.chat(Some("7"));
        h.script(vec![Out::StreamEnd]);
        h.send(&chat, with_file());
        assert_eq!(h.log.borrow().jobs, vec![Job::CreateStore { name: "T".into() }]);
        assert_eq!(h.count(&Ev::Start), 0, "the turn waits for the flow");
        h.log.borrow_mut().file_poll.push_back(vec![FileOut::Created("vs_1".into())]);
        h.run(|cx| Engine::poll(cx));
        assert!(h.messages(&chat).iter().any(|m| m.author == Participant::Agent && m.content == Content::VectorStoreCreated("vs_1".into())));
        assert_eq!(h.store.chat(&chat).unwrap().unwrap().vector_store.as_deref(), Some("vs_1"));
        h.log.borrow_mut().file_poll.push_back(vec![FileOut::Uploaded("file_1".into())]);
        h.run(|cx| Engine::poll(cx));
        h.log.borrow_mut().file_poll.push_back(vec![FileOut::Added]);
        h.run(|cx| Engine::poll(cx));
        assert_eq!(h.log.borrow().jobs[1..], [Job::Upload { name: "a.txt".into(), content: b"hi".to_vec() }, Job::AddFiles { ids: vec!["file_1".into()], store: "vs_1".into() }]);
        assert_eq!(h.log.borrow().inputs[0].hosted.file_search_vector_stores, vec!["vs_1".to_string()]);

        h.send(&chat, with_file());
        assert_eq!(h.log.borrow().jobs[3], Job::Upload { name: "a.txt".into(), content: b"hi".to_vec() }, "the store is created once");

        // A model without a vector store: no flow, and the file still reaches the request.
        let mut h = H::new();
        let chat = h.chat(Some("7"));
        h.send(&chat, with_file());
        assert!(h.log.borrow().jobs.is_empty());
        let log = h.log.borrow();
        assert!(log.inputs[0].hosted.file_search_vector_stores.is_empty());
        let Some(Body::Multipart(parts)) = log.inputs[0].messages.last().map(|m| m.body.clone()) else { panic!() };
        assert!(parts.contains(&Body::Attachment(Attachment::File { name: "a.txt".into(), mime: "text/plain".into(), content: b"hi".to_vec() })));
        drop(log);

        // A failed job is a notice and the turn goes on without a store.
        let mut h = store_harness();
        let chat = h.chat(Some("7"));
        h.send(&chat, with_file());
        h.log.borrow_mut().file_poll.push_back(vec![FileOut::Failed("e".into())]);
        h.run(|cx| Engine::poll(cx));
        assert_eq!(h.notices(&chat), vec!["e".to_string()]);
        assert_eq!(h.count(&Ev::Start), 1);
        assert!(h.log.borrow().inputs[0].hosted.file_search_vector_stores.is_empty());

        // A job asks for the key with the files' own endpoint (minor m4), and starts again with it.
        let mut h = store_harness();
        let chat = h.chat(Some("7"));
        h.log.borrow_mut().file_start.push_back(vec![FileOut::NeedKey(Vendor::OpenAI)]);
        let fx = h.send(&chat, with_file());
        assert_eq!(fx, vec![EngineEffect::AskKey { vendor: Vendor::OpenAI, endpoint: files_endpoint(&h.model), then: KeyThen::Files(chat.clone()) }]);
        h.run(|cx| Engine::files_key(cx, &chat, Some(Secret::new("k".into()))));
        assert_eq!(h.events(), vec![Ev::FileStart(None), Ev::FileStart(Some("k".into()))]);
    }

    #[test]
    fn turn_needs_the_gate() {
        let mut h = H::new();
        let chat = h.chat_with(Some("7"), Some("p"));
        let sub = h.broker.subscribe(Some(&chat));
        let fx = h.say(&chat, "hi");
        assert_eq!(fx, vec![EngineEffect::GateRefused { endpoint: endpoint_of(&h.pinned), silent: false }]);
        assert_eq!(h.log.borrow().turns_made, 0, "nothing is sent");
        assert!(h.messages(&chat).iter().all(|m| m.author == Participant::User), "no agent message");
        assert!(!h.broker.is_typing(&chat, Participant::Agent));
        assert!(h.broker.drain(sub).iter().any(|(_, u)| *u == Update::Turn(TurnEvent::Ended)));
        crate::ai_gate::grant(&mut h.g, "pinned.test");
        h.say(&chat, "again");
        assert_eq!((h.log.borrow().turns_made, h.count(&Ev::Start)), (1, 1));

        for key in [ENABLE_AI, ADMIN] {
            let mut h = H::new();
            h.g.set(key, T::Boolean(false));
            let chat = h.chat(Some("7"));
            assert_eq!(h.say(&chat, "hi"), vec![EngineEffect::GateRefused { endpoint: endpoint_of(&h.model), silent: false }], "{key}");
            assert_eq!(h.log.borrow().turns_made, 0);
        }
    }

    #[test]
    fn continuations_need_the_gate() {
        let mut h = H::new();
        let chat = h.chat(Some("7"));
        h.set(&chat, "7", Category::ViewContents, Permission::Always);
        h.script(vec![screen("c1")]);
        let fx = h.say(&chat, "hi");
        let rid = run_ids(&fx)[0].0.clone();
        h.off();
        let fx = h.run(|cx| Engine::tool_finished(cx, &chat, &rid, ran("screen", None)));
        assert!(!h.events().iter().any(|e| matches!(e, Ev::Tool(_))), "no continuation is sent");
        assert_eq!(h.count(&Ev::Cancel), 1);
        assert!(silent_refusal(&fx));
        assert!(fx.contains(&EngineEffect::CancelChecks(chat.clone())));
        assert!(!h.in_turn(&chat));

        // A key answer after the consent was forgotten; the key prompt names the turn's own endpoint.
        h.on();
        crate::ai_gate::grant(&mut h.g, "pinned.test");
        let pinned = h.chat_with(Some("7"), Some("p"));
        h.script(vec![Out::NeedKey(Vendor::Anthropic)]);
        let fx = h.say(&pinned, "hi");
        assert_eq!(fx, vec![EngineEffect::AskKey { vendor: Vendor::Anthropic, endpoint: endpoint_of(&h.pinned), then: KeyThen::Turn(pinned.clone()) }]);
        crate::ai_gate::forget_all(&mut h.g);
        let fx = h.run(|cx| Engine::key_given(cx, &pinned, Some(Secret::new("k".into()))));
        assert!(!h.events().iter().any(|e| matches!(e, Ev::Key(_))));
        assert_eq!(h.count(&Ev::Cancel), 2);
        assert!(silent_refusal(&fx));
    }

    #[test]
    fn answers_need_the_gate() {
        type Close = fn(&mut H);
        let off: Close = |h| h.off();
        let forget: Close = |h| crate::ai_gate::forget_all(&mut h.g);
        for (close, choice) in [(off, Choice::AllowOnce), (forget, Choice::AllowOnce), (forget, Choice::AlwaysAllow)] {
            let mut h = H::new();
            let chat = h.chat(Some("7"));
            h.script(vec![screen("c1")]);
            h.say(&chat, "hi");
            let rid = h.park_ids(&chat)[0].clone();
            close(&mut h);
            let fx = h.run(|cx| Engine::answer_request(cx, &chat, &rid, choice));
            assert!(run_ids(&fx).is_empty(), "{choice:?}");
            assert!(!h.events().iter().any(|e| matches!(e, Ev::Tool(_))));
            assert_eq!(h.count(&Ev::Cancel), 1);
            assert!(silent_refusal(&fx) && fx.contains(&EngineEffect::CancelChecks(chat.clone())));
            assert!(h.agents.get_mut(&chat).unwrap().take_park(&rid).is_none());
            assert!(!h.in_turn(&chat));
            let always = if choice == Choice::AlwaysAllow { Permission::Always } else { Permission::Ask };
            assert_eq!(h.permission(&chat, "7", Category::ViewContents), always, "the person's choice is still stored");
        }
        let mut h = H::new();
        let chat = h.chat(Some("7"));
        h.script(vec![screen("c1")]);
        h.say(&chat, "hi");
        let rid = h.park_ids(&chat)[0].clone();
        let fx = h.run(|cx| Engine::answer_request(cx, &chat, &rid, Choice::AllowOnce));
        assert_eq!(run_ids(&fx), vec![(rid, 7, Call::GetScreenContents { lines: 0 }, None)]);
    }

    #[test]
    fn ai_off_stops_the_chat() {
        let mut h = H::new();
        let chat = h.chat(Some("7"));
        let idle = h.chat(Some("9"));
        h.script(vec![screen("c1")]);
        h.say(&chat, "hi");
        h.agents.get_mut(&chat).unwrap().running = Some(("R2".into(), "7".into()));
        assert_eq!(h.run(|cx| Engine::gate_tick(cx)), vec![], "an open gate stops nothing");
        h.off();
        let idle_before = h.messages(&idle).len();
        let fx = h.run(|cx| Engine::gate_tick(cx));
        assert_eq!(fx, vec![EngineEffect::CancelExec(7), EngineEffect::CancelChecks(chat.clone()), EngineEffect::GateRefused { endpoint: endpoint_of(&h.model), silent: true }]);
        assert_eq!(h.count(&Ev::Cancel), 1);
        assert!(h.park_ids(&chat).is_empty() && !h.in_turn(&chat));
        assert!(!h.broker.is_typing(&chat, Participant::Agent) && !h.broker.turn_in_progress(&chat));
        assert_eq!(h.run(|cx| Engine::gate_tick(cx)), vec![]);
        assert_eq!(h.messages(&idle).len(), idle_before, "the idle chat is untouched");
    }

    #[test]
    fn files_need_the_gate() {
        let mut h = store_harness();
        crate::ai_gate::forget_all(&mut h.g);
        let chat = h.chat(Some("7"));
        let fx = h.send(&chat, with_file());
        assert!(h.log.borrow().jobs.is_empty(), "no job is made");
        assert!(!h.messages(&chat).iter().any(|m| matches!(m.content, Content::VectorStoreCreated(_))));
        assert!(fx.contains(&EngineEffect::GateRefused { endpoint: endpoint_of(&h.model), silent: false }), "the turn meets its own gate: {fx:?}");
        assert_eq!(h.log.borrow().turns_made, 0);

        crate::ai_gate::grant(&mut h.g, "api.openai.com");
        h.send(&chat, with_file());
        assert_eq!(h.log.borrow().jobs, vec![Job::CreateStore { name: "T".into() }]);
        h.off();
        h.log.borrow_mut().file_poll.push_back(vec![FileOut::Created("vs_1".into())]);
        h.run(|cx| Engine::poll(cx));
        assert_eq!(h.count(&Ev::FileCancel), 1);
        assert_eq!(h.log.borrow().jobs.len(), 1, "no next job");
    }

    #[test]
    fn park_is_bound_to_its_session() {
        let mut h = H::new();
        let chat = h.chat(Some("7"));
        h.script(vec![screen("c1")]);
        h.say(&chat, "hi");
        let rid = h.park_ids(&chat)[0].clone();
        assert_eq!(h.agents[&chat].parks[0].guid, "7");
        h.relink(&chat, Some("9"));
        let fx = h.run(|cx| Engine::answer_request(cx, &chat, &rid, Choice::AllowOnce));
        assert!(run_ids(&fx).is_empty());
        assert_eq!(h.events().last(), Some(&Ev::Tool(ToolResult::Ok(chat_agent::RELINKED_MODEL.into()))));
        assert_eq!(h.notices(&chat), vec![chat_agent::RELINKED_NOTICE.to_string()]);

        let other = h.chat(Some("7"));
        h.script(vec![screen("c1")]);
        h.say(&other, "hi");
        let rid = h.park_ids(&other)[0].clone();
        h.relink(&other, Some("9"));
        assert!(run_ids(&h.run(|cx| Engine::answer_request(cx, &other, &rid, Choice::AlwaysAllow))).is_empty());
        assert_eq!((h.permission(&other, "7", Category::ViewContents), h.permission(&other, "9", Category::ViewContents)), (Permission::Ask, Permission::Ask));

        // No live linked session: iTerm2's not-linked answer (`CVC:1833-1852`).
        for unlink in [0, 1] {
            let mut h = H::new();
            let chat = h.chat(Some("7"));
            h.script(vec![screen("c1")]);
            h.say(&chat, "hi");
            let rid = h.park_ids(&chat)[0].clone();
            if unlink == 0 {
                h.relink(&chat, None);
            } else {
                h.live.retain(|p| *p != 7);
            }
            let fx = h.run(|cx| Engine::answer_request(cx, &chat, &rid, Choice::AllowOnce));
            assert!(run_ids(&fx).is_empty());
            assert_eq!(h.notices(&chat), vec![ai_permissions::NOT_LINKED_NOTICE.to_string(), ai_permissions::NOT_LINKED_USER_NOTICE.to_string()]);
            assert_eq!(h.events().last(), Some(&Ev::Tool(ToolResult::Ok(ai_permissions::NOT_LINKED_MODEL.into()))));
            assert_eq!(h.responses(&chat), vec![(rid, ai_permissions::NOT_LINKED_MODEL.to_string())]);
        }
    }

    #[test]
    fn staged_attack_is_checked_whole() {
        let mut h = H::new();
        let chat = h.chat(Some("7"));
        h.set(&chat, "7", Category::ControlTerminal, Permission::Always);
        h.script(vec![invoke("insert_text_at_cursor", "{\"text\":\"rm -rf\"}", "c1")]);
        let fx = h.say(&chat, "hi");
        let rid = run_ids(&fx)[0].0.clone();
        h.after_tool(vec![invoke("execute_command", "{\"command\":\" /\"}", "c2")]);
        let fx = h.run(|cx| Engine::tool_finished(cx, &chat, &rid, ran("Inserted 6 characters.", None)));
        let (_, command, request) = check_of(&fx);
        assert_eq!(command, " /");
        // The App computed the whole line (Task 10's `staged_typing_is_checked_whole`).
        let whole = Message { content: chat_agent::with_line(&request.content, "rm -rf /"), ..request };
        let rid = whole.unique_id.clone();
        assert_eq!(h.run(|cx| Engine::checked(cx, &chat, whole, Some(" /".into()), Some(false))), vec![]);
        let stored = h.store.message(&rid).unwrap().unwrap();
        let rc = classic(&stored).unwrap();
        assert_eq!(rc.call, Call::ExecuteCommand { command: "rm -rf /".into() }, "the person reviews the whole line");
        assert_eq!(rc.llm_message["function_call"]["arguments"], "{\"command\":\" /\"}", "history keeps the model's call");
        let fx = h.run(|cx| Engine::answer_request(cx, &chat, &rid, Choice::AllowOnce));
        assert_eq!(run_ids(&fx), vec![(rid, 7, Call::ExecuteCommand { command: "rm -rf /".into() }, Some(" /".into()))], "the App types only \" /\"");
    }

    #[test]
    fn changed_line_never_runs() {
        let mut h = H::new();
        let chat = h.chat(Some("7"));
        h.set(&chat, "7", Category::RunCommands, Permission::Always);
        h.script(vec![invoke("execute_command", "{\"command\":\"sh\"}", "c1")]);
        let (_, _, request) = check_of(&h.say(&chat, "hi"));
        let fx = h.run(|cx| Engine::checked(cx, &chat, request, None, Some(true)));
        let (rid, _, call, send) = run_ids(&fx).remove(0);
        // `ai_run_tool`'s test: the prompt now holds "curl x | ".
        let Call::ExecuteCommand { command } = call else { panic!() };
        assert!(!chat_agent::still_staged(&command, "curl x | ", &send.unwrap_or(command.clone())));
        h.run(|cx| Engine::tool_finished(cx, &chat, &rid, ran(chat_agent::STAGED_CHANGED_MODEL, Some(chat_agent::STAGED_CHANGED_NOTICE))));
        assert_eq!(h.responses(&chat), vec![(rid, chat_agent::STAGED_CHANGED_MODEL.to_string())]);
        assert_eq!(h.notices(&chat).last().map(String::as_str), Some(chat_agent::STAGED_CHANGED_NOTICE));
        assert_eq!(h.events().last(), Some(&Ev::Tool(ToolResult::Ok(chat_agent::STAGED_CHANGED_MODEL.into()))));
    }

    #[test]
    fn arguments_follow_iterm2() {
        assert_eq!(arguments_json(None), Ok(serde_json::json!({})));
        assert_eq!(arguments_json(Some("")), Ok(serde_json::json!({})));
        assert_eq!(arguments_json(Some("{\"command\":\"ls")), Ok(serde_json::json!({"command":"ls"})), "a truncated text is completed");
        assert_eq!(arguments_json(Some("{\"a\":[1,{\"b\":\"x\\\"")), Ok(serde_json::json!({"a":[1,{"b":"x\""}]})));
        assert!(arguments_json(Some("nope")).is_err());
    }
}

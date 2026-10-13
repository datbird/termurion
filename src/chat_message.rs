//! A chat message and its Swift-shaped JSON (`AITerm/Message.swift`, `Categories/Result+iTerm.swift`,
//! `AITerm/TerminalCommand.swift`), so a message iTerm2 wrote into `chatdb.sqlite` reads back and
//! a message Tandem writes has iTerm2's shape. Pure.
//!
//! Swift's synthesized enum coding: each case is a one-key object named by the case, its
//! unlabeled values are `_0`, `_1`, ... and its labeled ones use their labels, a `nil` value
//! is left out. Types other parts interpret (explanations, watcher events, orchestration
//! actions and commands, attachments, the model's own message) ride as opaque JSON.

use serde_json::{Map, Value};

use crate::ai_msg::Status;
use crate::ai_permissions::Category;
use crate::ai_tools::{truncated, Call};

/// Shown in place of a message this build cannot read (`CVC:2569-2572`).
pub const UNSUPPORTED_TEXT: &str = "This message requires a newer version of Tandem to view.";

/// The chat list's snippet length (`MSG:455-459`).
pub const SNIPPET_MAX: usize = 40;

/// Seconds from 1970-01-01 to Swift's reference date, 2001-01-01.
const REFERENCE_DATE: f64 = 978_307_200.0;

/// A fresh id in Swift's `UUID().uuidString` form: uppercase, hyphenated, version 4.
pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string().to_uppercase()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Participant {
    User,
    Agent,
}

impl Participant {
    pub fn as_str(self) -> &'static str {
        match self {
            Participant::User => "user",
            Participant::Agent => "agent",
        }
    }

    pub fn parse(s: &str) -> Option<Participant> {
        match s {
            "user" => Some(Participant::User),
            "agent" => Some(Participant::Agent),
            _ => None,
        }
    }
}

/// The start and genuine end of an agent turn (`MSG:62-81`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnEvent {
    Started,
    Ended,
}

/// Swift `AIError`: `type` is 0 (generic) or 1 (request too large).
#[derive(Clone, Debug, PartialEq)]
pub struct AiError {
    pub message: String,
    pub kind: i64,
}

/// A tool call the model asked for: the model's own message (12b's raw assistant JSON) and
/// the call.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteCommand {
    pub llm_message: Value,
    pub call: Call,
    /// Tandem's (final review I1): for a browser call, the host its pane showed when the approval
    /// was asked (`ai_browser::Site::host`), written as `termurionSite`; None for every other call.
    pub site: Option<String>,
}

impl RemoteCommand {
    pub fn of(llm_message: Value, call: Call) -> RemoteCommand {
        RemoteCommand { llm_message, call, site: None }
    }

    /// The text of the parked request: a browser call asked about a site gets Tandem's text naming
    /// the action and the site (`ai_browser::approval_text`); every other call keeps
    /// `consent_description`.
    pub fn consent_text(&self) -> String {
        match &self.site {
            Some(host) if self.call.category().is_browser() => crate::ai_browser::approval_text(&self.call, host),
            _ => self.call.consent_description(),
        }
    }
}

/// A classic call is written bare; an orchestration call carries `"kind":"external"` (`RC:29-102`).
#[derive(Clone, Debug, PartialEq)]
pub enum Payload {
    Classic(RemoteCommand),
    External(Value),
}

#[derive(Clone, Debug, PartialEq)]
pub struct TerminalCommand {
    pub username: Option<String>,
    pub hostname: Option<String>,
    pub directory: Option<String>,
    pub command: String,
    pub output: String,
    pub exit_code: i32,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Subpart {
    PlainText(String),
    Markdown(String),
    /// iTerm2's `LLM.Message.Attachment` object.
    Attachment(Value),
    /// Extra context a user message adds for the model.
    Context(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Streaming {
    Stopped,
    Active,
    StoppedAutomatically,
}

impl Streaming {
    fn raw(self) -> &'static str {
        match self {
            Streaming::Stopped => "stopped",
            Streaming::Active => "active",
            Streaming::StoppedAutomatically => "stoppedAutomatically",
        }
    }

    fn parse(raw: &str) -> Option<Streaming> {
        [Streaming::Stopped, Streaming::Active, Streaming::StoppedAutomatically].into_iter().find(|s| s.raw() == raw)
    }
}

/// `ClientLocal.Action` (`MSG:131-215`). The three orchestration actions are known to iTerm2
/// and ride as `Other`, keeping their one-key object (12f).
#[derive(Clone, Debug, PartialEq)]
pub enum ClientLocal {
    PickingSession,
    ExecutingCommand(RemoteCommand),
    Notice(String),
    StreamingChanged(Streaming),
    OfferLink { terminal: bool, guid: String, name: Option<String> },
    OfferOrchestration,
    Permissions { terminal: bool, guid: String },
    Other(Value),
}

/// iTerm2's orchestration actions, carried unread.
const ORCHESTRATION_ACTIONS: [&str; 3] = ["workgroupPermissionRequest", "enableOrchestrationRequest", "orchestrationPermissionGranted"];

/// `UserCommand` (`MSG:217-240`); the orchestration answers ride as `Other`, keeping their
/// one-key object (12f).
#[derive(Clone, Debug, PartialEq)]
pub enum UserCommand {
    Stop,
    Other(Value),
}

const ORCHESTRATION_COMMANDS: [&str; 3] = ["workgroupPermissionResponse", "enableOrchestrationResponse", "revokeOrchestrationPermission"];

/// `Message.Content` (`MSG:286-336`). The opaque cases hold the case's whole body object.
#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    PlainText { text: String, context: Option<String> },
    Markdown(String),
    ExplanationRequest(Value),
    ExplanationResponse(Value),
    RemoteCommandRequest { payload: Payload, safe: Option<bool> },
    WatcherEvent(Value),
    RemoteCommandResponse { result: Result<String, AiError>, request_id: String, function_name: String, function_call_id: Option<Value> },
    SelectSessionRequest { original: Box<Message>, terminal: bool },
    ClientLocal(ClientLocal),
    RenameChat(String),
    Append { text: String, uuid: String },
    AppendAttachment { attachment: Value, uuid: String },
    Commit(String),
    UserCommand(UserCommand),
    SetPermissions(Vec<Category>),
    VectorStoreCreated(String),
    TerminalCommand(TerminalCommand),
    Multipart { parts: Vec<Subpart>, vector_store_id: Option<String> },
    /// A message this build cannot read, kept as its raw JSON text and never rewritten.
    Unsupported(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    /// The database's sequence number; 0 until stored.
    pub seq: i64,
    pub unique_id: String,
    pub author: Participant,
    pub chat_id: String,
    pub content: Content,
    /// Seconds since 1970.
    pub sent_date: f64,
    pub response_id: Option<String>,
    pub agent_reasoning: Option<String>,
    pub first_blob_ref: Option<String>,
}

impl Message {
    pub fn new(chat_id: &str, author: Participant, content: Content, now: f64) -> Message {
        Message { seq: 0, unique_id: new_id(), author, chat_id: chat_id.to_string(), content, sent_date: now, response_id: None, agent_reasoning: None, first_blob_ref: None }
    }
}

/// `{name: body}`.
pub(crate) fn case(name: &str, body: Value) -> Value {
    let mut map = Map::new();
    map.insert(name.to_string(), body);
    Value::Object(map)
}

/// The one key and value of a Swift enum's object; None for anything else.
pub(crate) fn one_key(v: &Value) -> Option<(&str, &Value)> {
    let map = v.as_object()?;
    if map.len() != 1 {
        return None;
    }
    map.iter().next().map(|(k, v)| (k.as_str(), v))
}

/// An object of the given keys in order, leaving out the `None` ones (`encodeIfPresent`).
fn obj<const N: usize>(pairs: [(&str, Option<Value>); N]) -> Value {
    let mut map = Map::new();
    for (key, value) in pairs {
        if let Some(value) = value {
            map.insert(key.to_string(), value);
        }
    }
    Value::Object(map)
}

fn text(s: &str) -> Option<Value> {
    Some(Value::String(s.to_string()))
}

fn empty() -> Value {
    Value::Object(Map::new())
}

/// A case body being read.
struct Body<'a>(&'a Map<String, Value>);

impl<'a> Body<'a> {
    fn of(v: &'a Value) -> Option<Body<'a>> {
        v.as_object().map(Body)
    }

    fn get(&self, key: &str) -> Option<&'a Value> {
        self.0.get(key)
    }

    fn string(&self, key: &str) -> Option<String> {
        self.0.get(key)?.as_str().map(str::to_string)
    }

    /// Missing or null: `Some(None)`; a string: `Some(Some)`; anything else fails.
    fn opt_string(&self, key: &str) -> Option<Option<String>> {
        match self.0.get(key) {
            None | Some(Value::Null) => Some(None),
            Some(Value::String(s)) => Some(Some(s.clone())),
            Some(_) => None,
        }
    }

    fn bool(&self, key: &str) -> Option<bool> {
        self.0.get(key)?.as_bool()
    }

    fn opt_bool(&self, key: &str) -> Option<Option<bool>> {
        match self.0.get(key) {
            None | Some(Value::Null) => Some(None),
            Some(Value::Bool(b)) => Some(Some(*b)),
            Some(_) => None,
        }
    }
}

fn command_value(rc: &RemoteCommand) -> Value {
    obj([("llmMessage", Some(rc.llm_message.clone())), ("content", Some(rc.call.encode())), ("termurionSite", rc.site.clone().map(Value::String))])
}

fn command_of(v: &Value) -> Option<RemoteCommand> {
    let body = Body::of(v)?;
    Some(RemoteCommand { llm_message: body.get("llmMessage")?.clone(), call: Call::decode(body.get("content")?)?, site: body.opt_string("termurionSite")? })
}

fn action_value(action: &ClientLocal) -> Value {
    match action {
        ClientLocal::PickingSession => case("pickingSession", empty()),
        ClientLocal::ExecutingCommand(rc) => case("executingCommand", case("_0", command_value(rc))),
        ClientLocal::Notice(s) => case("notice", obj([("_0", text(s))])),
        ClientLocal::StreamingChanged(state) => case("streamingChanged", obj([("_0", text(state.raw()))])),
        ClientLocal::OfferLink { terminal, guid, name } => case("offerLink", obj([("terminal", Some(Value::Bool(*terminal))), ("guid", text(guid)), ("name", name.as_deref().and_then(text))])),
        ClientLocal::OfferOrchestration => case("offerOrchestration", empty()),
        ClientLocal::Permissions { terminal, guid } => case("permissions", obj([("terminal", Some(Value::Bool(*terminal))), ("guid", text(guid))])),
        ClientLocal::Other(v) => v.clone(),
    }
}

/// An unknown action fails, which makes the whole content Unsupported (`MSG:201-215`).
fn action_of(v: &Value) -> Option<ClientLocal> {
    let (name, raw) = one_key(v)?;
    let body = Body::of(raw)?;
    Some(match name {
        "pickingSession" => ClientLocal::PickingSession,
        "executingCommand" => ClientLocal::ExecutingCommand(command_of(body.get("_0")?)?),
        "notice" => ClientLocal::Notice(body.string("_0")?),
        "streamingChanged" => ClientLocal::StreamingChanged(Streaming::parse(&body.string("_0")?)?),
        "offerLink" => ClientLocal::OfferLink { terminal: body.bool("terminal")?, guid: body.string("guid")?, name: body.opt_string("name")? },
        "offerOrchestration" => ClientLocal::OfferOrchestration,
        "permissions" => ClientLocal::Permissions { terminal: body.bool("terminal")?, guid: body.string("guid")? },
        _ if ORCHESTRATION_ACTIONS.contains(&name) => ClientLocal::Other(v.clone()),
        _ => return None,
    })
}

fn subpart_value(part: &Subpart) -> Value {
    match part {
        Subpart::PlainText(s) => case("plainText", obj([("_0", text(s))])),
        Subpart::Markdown(s) => case("markdown", obj([("_0", text(s))])),
        Subpart::Attachment(v) => case("attachment", case("_0", v.clone())),
        Subpart::Context(s) => case("context", obj([("_0", text(s))])),
    }
}

fn subpart_of(v: &Value) -> Option<Subpart> {
    let (name, raw) = one_key(v)?;
    let body = Body::of(raw)?;
    Some(match name {
        "plainText" => Subpart::PlainText(body.string("_0")?),
        "markdown" => Subpart::Markdown(body.string("_0")?),
        "attachment" => Subpart::Attachment(body.get("_0")?.clone()),
        "context" => Subpart::Context(body.string("_0")?),
        _ => return None,
    })
}

/// A content as a JSON value; an Unsupported text that is not JSON rides as a JSON string
/// (Decision: never happens for a text read from iTerm2's database, which always holds JSON).
fn content_value(c: &Content) -> Value {
    match c {
        Content::PlainText { text: s, context } => case("plainText", obj([("_0", text(s)), ("context", context.as_deref().and_then(text))])),
        Content::Markdown(s) => case("markdown", obj([("_0", text(s))])),
        Content::ExplanationRequest(v) => case("explanationRequest", v.clone()),
        Content::ExplanationResponse(v) => case("explanationResponse", v.clone()),
        Content::RemoteCommandRequest { payload, safe } => {
            let payload = match payload {
                Payload::Classic(rc) => command_value(rc),
                Payload::External(v) => v.clone(),
            };
            case("remoteCommandRequest", obj([("_0", Some(payload)), ("safe", safe.map(Value::Bool))]))
        }
        Content::WatcherEvent(v) => case("watcherEvent", v.clone()),
        Content::RemoteCommandResponse { result, request_id, function_name, function_call_id } => {
            let result = match result {
                Ok(s) => case("success", Value::String(s.clone())),
                Err(e) => case("failure", obj([("message", text(&e.message)), ("type", Some(e.kind.into()))])),
            };
            case("remoteCommandResponse", obj([("_0", Some(result)), ("_1", text(request_id)), ("_2", text(function_name)), ("_3", function_call_id.clone())]))
        }
        Content::SelectSessionRequest { original, terminal } => case("selectSessionRequest", obj([("_0", Some(encode_message(original))), ("terminal", Some(Value::Bool(*terminal)))])),
        Content::ClientLocal(action) => case("clientLocal", case("_0", case("action", action_value(action)))),
        Content::RenameChat(s) => case("renameChat", obj([("_0", text(s))])),
        Content::Append { text: s, uuid } => case("append", obj([("string", text(s)), ("uuid", text(uuid))])),
        Content::AppendAttachment { attachment, uuid } => case("appendAttachment", obj([("attachment", Some(attachment.clone())), ("uuid", text(uuid))])),
        Content::Commit(s) => case("commit", obj([("_0", text(s))])),
        Content::UserCommand(command) => {
            let command = match command {
                UserCommand::Stop => case("stop", empty()),
                UserCommand::Other(v) => v.clone(),
            };
            case("userCommand", case("_0", command))
        }
        Content::SetPermissions(categories) => {
            let mut sorted = categories.clone();
            sorted.sort();
            sorted.dedup();
            case("setPermissions", case("_0", Value::Array(sorted.iter().map(|c| Value::String(c.raw().to_string())).collect())))
        }
        Content::VectorStoreCreated(id) => case("vectorStoreCreated", obj([("id", text(id))])),
        Content::TerminalCommand(tc) => case(
            "terminalCommand",
            case(
                "_0",
                obj([
                    ("username", tc.username.as_deref().and_then(text)),
                    ("hostname", tc.hostname.as_deref().and_then(text)),
                    ("directory", tc.directory.as_deref().and_then(text)),
                    ("command", text(&tc.command)),
                    ("output", text(&tc.output)),
                    ("exitCode", Some(tc.exit_code.into())),
                    ("url", text(&tc.url)),
                ]),
            ),
        ),
        Content::Multipart { parts, vector_store_id } => {
            case("multipart", obj([("_0", Some(Value::Array(parts.iter().map(subpart_value).collect()))), ("vectorStoreID", vector_store_id.as_deref().and_then(text))]))
        }
        Content::Unsupported(t) => serde_json::from_str(t).unwrap_or_else(|_| Value::String(t.clone())),
    }
}

/// Reads a content; None for an unknown case or a known case whose body does not decode.
fn content_of(v: &Value) -> Option<Content> {
    let (name, raw) = one_key(v)?;
    let body = Body::of(raw)?;
    Some(match name {
        "plainText" => Content::PlainText { text: body.string("_0")?, context: body.opt_string("context")? },
        "markdown" => Content::Markdown(body.string("_0")?),
        "explanationRequest" => Content::ExplanationRequest(raw.clone()),
        "explanationResponse" => Content::ExplanationResponse(raw.clone()),
        "remoteCommandRequest" => {
            let payload = body.get("_0")?;
            let payload = if payload.get("kind").and_then(Value::as_str) == Some("external") { Payload::External(payload.clone()) } else { Payload::Classic(command_of(payload)?) };
            Content::RemoteCommandRequest { payload, safe: body.opt_bool("safe")? }
        }
        "watcherEvent" => Content::WatcherEvent(raw.clone()),
        "remoteCommandResponse" => {
            let result = Body::of(body.get("_0")?)?;
            let result = if let Some(success) = result.get("success") {
                Ok(success.as_str()?.to_string())
            } else {
                let failure = Body::of(result.get("failure")?)?;
                let kind = match failure.get("type") {
                    None | Some(Value::Null) => 0,
                    Some(kind) => kind.as_i64()?,
                };
                Err(AiError { message: failure.string("message")?, kind })
            };
            let function_call_id = body.get("_3").filter(|v| !v.is_null()).cloned();
            Content::RemoteCommandResponse { result, request_id: body.string("_1")?, function_name: body.string("_2")?, function_call_id }
        }
        "selectSessionRequest" => Content::SelectSessionRequest { original: Box::new(decode_message(body.get("_0")?)?), terminal: body.bool("terminal")? },
        "clientLocal" => Content::ClientLocal(action_of(Body::of(body.get("_0")?)?.get("action")?)?),
        "renameChat" => Content::RenameChat(body.string("_0")?),
        "append" => Content::Append { text: body.string("string")?, uuid: body.string("uuid")? },
        "appendAttachment" => Content::AppendAttachment { attachment: body.get("attachment")?.clone(), uuid: body.string("uuid")? },
        "commit" => Content::Commit(body.string("_0")?),
        "userCommand" => {
            let command = body.get("_0")?;
            let (name, inner) = one_key(command)?;
            inner.as_object()?;
            match name {
                "stop" => Content::UserCommand(UserCommand::Stop),
                _ if ORCHESTRATION_COMMANDS.contains(&name) => Content::UserCommand(UserCommand::Other(command.clone())),
                _ => return None,
            }
        }
        "setPermissions" => {
            let mut categories = body.get("_0")?.as_array()?.iter().map(|c| Category::parse(c.as_str()?)).collect::<Option<Vec<_>>>()?;
            categories.sort();
            categories.dedup();
            Content::SetPermissions(categories)
        }
        "vectorStoreCreated" => Content::VectorStoreCreated(body.string("id")?),
        "terminalCommand" => {
            let tc = Body::of(body.get("_0")?)?;
            Content::TerminalCommand(TerminalCommand {
                username: tc.opt_string("username")?,
                hostname: tc.opt_string("hostname")?,
                directory: tc.opt_string("directory")?,
                command: tc.string("command")?,
                output: tc.string("output")?,
                exit_code: i32::try_from(tc.get("exitCode")?.as_i64()?).ok()?,
                url: tc.string("url")?,
            })
        }
        "multipart" => Content::Multipart {
            parts: body.get("_0")?.as_array()?.iter().map(subpart_of).collect::<Option<Vec<_>>>()?,
            vector_store_id: body.opt_string("vectorStoreID")?,
        },
        _ => return None,
    })
}

/// The JSON text stored in the Message table. An Unsupported content is its text, unchanged.
pub fn encode_content(c: &Content) -> String {
    match c {
        Content::Unsupported(t) => t.clone(),
        other => content_value(other).to_string(),
    }
}

/// Never fails: a text that is not JSON, an unknown case, or a known case whose body does
/// not decode gives `Unsupported(text)` (iTerm2 throws for the last; Decision).
pub fn decode_content(text: &str) -> Content {
    serde_json::from_str::<Value>(text).ok().and_then(|v| content_of(&v)).unwrap_or_else(|| Content::Unsupported(text.to_string()))
}

/// The Swift `Message` object (`MSG:707-758`): `sentDate` is seconds since 2001-01-01;
/// `inResponseTo` and `configuration` are not kept.
pub fn encode_message(m: &Message) -> Value {
    obj([
        ("chatID", text(&m.chat_id)),
        ("author", text(m.author.as_str())),
        ("content", Some(content_value(&m.content))),
        ("sentDate", Some((m.sent_date - REFERENCE_DATE).into())),
        ("uniqueID", text(&m.unique_id)),
        ("responseID", m.response_id.as_deref().and_then(text)),
        ("agentReasoning", m.agent_reasoning.as_deref().and_then(text)),
        ("firstBlobRef", m.first_blob_ref.as_deref().and_then(text)),
    ])
}

/// Reads a Swift `Message` (seq 0); an unreadable content alone degrades to Unsupported.
pub fn decode_message(v: &Value) -> Option<Message> {
    let body = Body::of(v)?;
    let content = body.get("content")?;
    Some(Message {
        seq: 0,
        unique_id: body.string("uniqueID")?,
        author: Participant::parse(&body.string("author")?)?,
        chat_id: body.string("chatID")?,
        content: content_of(content).unwrap_or_else(|| Content::Unsupported(content.to_string())),
        sent_date: body.get("sentDate")?.as_f64()? + REFERENCE_DATE,
        response_id: body.opt_string("responseID")?,
        agent_reasoning: body.opt_string("agentReasoning")?,
        first_blob_ref: body.opt_string("firstBlobRef")?,
    })
}

/// Not shown as a message of its own (`hiddenFromClient`, `MSG:548-562`).
pub fn hidden(c: &Content) -> bool {
    matches!(
        c,
        Content::RemoteCommandResponse { .. } | Content::RenameChat(_) | Content::Commit(_) | Content::SetPermissions(_) | Content::VectorStoreCreated(_) | Content::UserCommand(_)
    )
}

/// Whether the message fixes the chat's provider (`messageLocksProvider`, `CVC:807-817`).
pub fn locks_provider(c: &Content) -> bool {
    !matches!(
        c,
        Content::ClientLocal(_) | Content::RenameChat(_) | Content::Commit(_) | Content::UserCommand(_) | Content::SetPermissions(_) | Content::VectorStoreCreated(_)
    )
}

fn payload_description(payload: &Payload) -> String {
    match payload {
        Payload::Classic(rc) => rc.call.markdown_description(),
        Payload::External(v) => v.get("markdownDescription").and_then(Value::as_str).unwrap_or_default().to_string(),
    }
}

/// A hosted tool's status update (`LLM.Message.StatusUpdate`).
pub(crate) fn status_of(v: &Value) -> Option<Status> {
    let (name, body) = one_key(v)?;
    let first = body.get("_0");
    Some(match name {
        "webSearchStarted" => Status::WebSearchStarted,
        "webSearchFinished" => Status::WebSearchFinished(first.and_then(Value::as_str).map(str::to_string)),
        "codeInterpreterStarted" => Status::CodeInterpreterStarted,
        "codeInterpreterFinished" => Status::CodeInterpreterFinished,
        "reasoningSummaryUpdate" => Status::ReasoningSummary(first?.as_str()?.to_string()),
        "multipart" => Status::Multipart(first?.as_array()?.iter().map(status_of).collect::<Option<Vec<_>>>()?),
        _ => return None,
    })
}

/// One subpart's preview (`previewAndLabel`, `MSG:258-283`); None for context and for an
/// attachment this build cannot read. A file shows its name without iTerm2's emoji prefix.
fn subpart_preview(part: &Subpart, max: usize) -> Option<String> {
    match part {
        Subpart::PlainText(s) | Subpart::Markdown(s) => Some(truncated(s, max)),
        Subpart::Context(_) => None,
        Subpart::Attachment(v) => {
            let (kind, body) = one_key(v.get("type")?)?;
            match kind {
                "code" => Some(truncated(body.get("_0")?.as_str()?, max)),
                "statusUpdate" => Some(status_of(body.get("_0")?)?.display()),
                "file" => Some(body.get("_0")?.get("name")?.as_str()?.to_string()),
                "fileID" => Some(body.get("name")?.as_str()?.to_string()),
                _ => None,
            }
        }
    }
}

/// The chat list's snippet (`snippetText`, `MSG:461-505`); "" where iTerm2 has none.
pub fn snippet(c: &Content, max: usize) -> String {
    let field = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
    match c {
        Content::PlainText { text, .. } | Content::Markdown(text) => truncated(text, max),
        Content::ExplanationRequest(v) => {
            let request = v.get("request").cloned().unwrap_or(Value::Null);
            match request.get("command").and_then(Value::as_str) {
                Some(command) => format!("Explain {command}"),
                None if !field(&request, "question").is_empty() => field(&request, "question"),
                None => format!("Explain {}", field(&request, "subjectMatter")),
            }
        }
        Content::ExplanationResponse(v) => truncated(&field(v, "markdown"), max),
        Content::RemoteCommandRequest { payload, .. } => payload_description(payload),
        Content::SelectSessionRequest { .. } => "Selecting session\u{2026}".to_string(),
        Content::ClientLocal(action) => match action {
            ClientLocal::ExecutingCommand(rc) => rc.call.markdown_description(),
            ClientLocal::PickingSession => "Selecting session\u{2026}".to_string(),
            ClientLocal::Notice(s) => s.clone(),
            ClientLocal::StreamingChanged(Streaming::Active) => "Sending commands to AI automatically".to_string(),
            ClientLocal::StreamingChanged(_) => "Stopped sending commands to AI".to_string(),
            ClientLocal::OfferLink { .. } | ClientLocal::OfferOrchestration | ClientLocal::Permissions { .. } | ClientLocal::Other(_) => String::new(),
        },
        Content::RenameChat(_)
        | Content::Append { .. }
        | Content::AppendAttachment { .. }
        | Content::Commit(_)
        | Content::SetPermissions(_)
        | Content::VectorStoreCreated(_)
        | Content::UserCommand(_)
        | Content::Unsupported(_) => String::new(),
        Content::WatcherEvent(v) => truncated(&v.get("_0").map(|u| field(u, "detail")).unwrap_or_default(), max),
        Content::RemoteCommandResponse { .. } => "Finished executing command".to_string(),
        Content::TerminalCommand(tc) => format!("Ran `{}`", truncated(&tc.command, max.saturating_sub(4))),
        Content::Multipart { parts, .. } => parts.iter().rev().find_map(|p| subpart_preview(p, max)).unwrap_or_else(|| "Empty message".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_permissions::Category;
    use crate::ai_tools::Call;
    use serde_json::json;

    fn round(c: Content, want: &str) {
        let text = encode_content(&c);
        assert_eq!(text, want);
        assert_eq!(decode_content(&text), c, "{want}");
    }

    fn classic(call: Call) -> Payload {
        Payload::Classic(RemoteCommand::of(json!({"role":"assistant"}), call))
    }

    fn all_calls() -> Vec<Call> {
        vec![
            Call::IsAtPrompt,
            Call::ExecuteCommand { command: "ls".into() },
            Call::GetLastExitStatus,
            Call::GetCommandHistory { limit: 5 },
            Call::GetLastCommand,
            Call::GetCommandBeforeCursor,
            Call::SearchCommandHistory { query: "q".into() },
            Call::GetCommandOutput { id: "7".into() },
            Call::GetScreenContents { lines: 12 },
            Call::GetTerminalSize,
            Call::GetShellType,
            Call::DetectSshSession,
            Call::GetRemoteHostname,
            Call::GetUserIdentity,
            Call::GetCurrentDirectory,
            Call::SetClipboard { text: "t".into() },
            Call::InsertTextAtCursor { text: "i".into() },
            Call::DeleteCurrentLine,
            Call::GetManPage { cmd: "ls".into() },
            Call::CreateFile { filename: "f".into(), content: "c".into() },
            Call::FindOnPage { query: "f".into() },
            Call::LoadUrl { url: "https://a".into() },
            Call::WebSearch { query: "w".into() },
            Call::GetUrl,
            Call::ReadWebPage { starting_line_number: 3, number_of_lines: 4 },
            Call::RestartSession,
        ]
    }

    #[test]
    fn content_codec_follows_swift() {
        round(Content::PlainText { text: "hi".into(), context: None }, r#"{"plainText":{"_0":"hi"}}"#);
        round(Content::PlainText { text: "hi".into(), context: Some("c".into()) }, r#"{"plainText":{"_0":"hi","context":"c"}}"#);
        round(Content::Markdown("m".into()), r#"{"markdown":{"_0":"m"}}"#);
        round(Content::Commit("A1".into()), r#"{"commit":{"_0":"A1"}}"#);
        round(Content::Append { text: "x".into(), uuid: "A1".into() }, r#"{"append":{"string":"x","uuid":"A1"}}"#);
        round(Content::RenameChat("t".into()), r#"{"renameChat":{"_0":"t"}}"#);
        round(Content::SetPermissions(vec![Category::RunCommands, Category::ViewContents]), r#"{"setPermissions":{"_0":["Run Commands","View Contents"]}}"#);
        assert_eq!(encode_content(&Content::SetPermissions(vec![Category::ViewContents, Category::RunCommands])), r#"{"setPermissions":{"_0":["Run Commands","View Contents"]}}"#);
        round(
            Content::RemoteCommandResponse { result: Ok("out".into()), request_id: "A1".into(), function_name: "execute_command".into(), function_call_id: None },
            r#"{"remoteCommandResponse":{"_0":{"success":"out"},"_1":"A1","_2":"execute_command"}}"#,
        );
        round(
            Content::RemoteCommandResponse { result: Err(AiError { message: "no".into(), kind: 0 }), request_id: "A1".into(), function_name: "f".into(), function_call_id: Some(json!({"callID":"c","itemID":"i"})) },
            r#"{"remoteCommandResponse":{"_0":{"failure":{"message":"no","type":0}},"_1":"A1","_2":"f","_3":{"callID":"c","itemID":"i"}}}"#,
        );
        round(
            Content::RemoteCommandRequest { payload: classic(Call::ExecuteCommand { command: "ls".into() }), safe: Some(false) },
            r#"{"remoteCommandRequest":{"_0":{"llmMessage":{"role":"assistant"},"content":{"executeCommand":{"_0":{"command":"ls"}}}},"safe":false}}"#,
        );
        round(
            Content::RemoteCommandRequest { payload: classic(Call::IsAtPrompt), safe: None },
            r#"{"remoteCommandRequest":{"_0":{"llmMessage":{"role":"assistant"},"content":{"isAtPrompt":{"_0":{}}}}}}"#,
        );
        round(
            Content::RemoteCommandRequest { payload: classic(Call::GetCommandHistory { limit: 5 }), safe: Some(true) },
            r#"{"remoteCommandRequest":{"_0":{"llmMessage":{"role":"assistant"},"content":{"getCommandHistory":{"_0":{"limit":5}}}},"safe":true}}"#,
        );
        round(Content::ClientLocal(ClientLocal::Notice("n".into())), r#"{"clientLocal":{"_0":{"action":{"notice":{"_0":"n"}}}}}"#);
        round(Content::ClientLocal(ClientLocal::PickingSession), r#"{"clientLocal":{"_0":{"action":{"pickingSession":{}}}}}"#);
        round(Content::ClientLocal(ClientLocal::OfferOrchestration), r#"{"clientLocal":{"_0":{"action":{"offerOrchestration":{}}}}}"#);
        round(Content::ClientLocal(ClientLocal::StreamingChanged(Streaming::StoppedAutomatically)), r#"{"clientLocal":{"_0":{"action":{"streamingChanged":{"_0":"stoppedAutomatically"}}}}}"#);
        round(
            Content::ClientLocal(ClientLocal::OfferLink { terminal: true, guid: "7".into(), name: None }),
            r#"{"clientLocal":{"_0":{"action":{"offerLink":{"terminal":true,"guid":"7"}}}}}"#,
        );
        round(
            Content::ClientLocal(ClientLocal::OfferLink { terminal: false, guid: "7".into(), name: Some("n".into()) }),
            r#"{"clientLocal":{"_0":{"action":{"offerLink":{"terminal":false,"guid":"7","name":"n"}}}}}"#,
        );
        round(Content::ClientLocal(ClientLocal::Permissions { terminal: true, guid: "7".into() }), r#"{"clientLocal":{"_0":{"action":{"permissions":{"terminal":true,"guid":"7"}}}}}"#);
        round(
            Content::ClientLocal(ClientLocal::ExecutingCommand(RemoteCommand::of(json!({}), Call::GetShellType))),
            r#"{"clientLocal":{"_0":{"action":{"executingCommand":{"_0":{"llmMessage":{},"content":{"getShellType":{"_0":{}}}}}}}}}"#,
        );
        round(Content::UserCommand(UserCommand::Stop), r#"{"userCommand":{"_0":{"stop":{}}}}"#);
        round(
            Content::UserCommand(UserCommand::Other(json!({"revokeOrchestrationPermission":{"scope":"s"}}))),
            r#"{"userCommand":{"_0":{"revokeOrchestrationPermission":{"scope":"s"}}}}"#,
        );
        let tc = TerminalCommand { username: Some("u".into()), hostname: Some("h".into()), directory: Some("d".into()), command: "c".into(), output: "o".into(), exit_code: 1, url: "file:///x".into() };
        round(
            Content::TerminalCommand(tc.clone()),
            r#"{"terminalCommand":{"_0":{"username":"u","hostname":"h","directory":"d","command":"c","output":"o","exitCode":1,"url":"file:///x"}}}"#,
        );
        round(
            Content::TerminalCommand(TerminalCommand { username: None, ..tc }),
            r#"{"terminalCommand":{"_0":{"hostname":"h","directory":"d","command":"c","output":"o","exitCode":1,"url":"file:///x"}}}"#,
        );
        round(Content::VectorStoreCreated("v".into()), r#"{"vectorStoreCreated":{"id":"v"}}"#);
        round(Content::Multipart { parts: vec![Subpart::PlainText("a".into())], vector_store_id: None }, r#"{"multipart":{"_0":[{"plainText":{"_0":"a"}}]}}"#);
        round(
            Content::Multipart {
                parts: vec![Subpart::Markdown("m".into()), Subpart::Attachment(json!({"inline":true,"id":"x","type":{"code":{"_0":"c"}}})), Subpart::Context("k".into())],
                vector_store_id: Some("vs".into()),
            },
            r#"{"multipart":{"_0":[{"markdown":{"_0":"m"}},{"attachment":{"_0":{"inline":true,"id":"x","type":{"code":{"_0":"c"}}}}},{"context":{"_0":"k"}}],"vectorStoreID":"vs"}}"#,
        );
        round(
            Content::AppendAttachment { attachment: json!({"inline":false,"id":"a","type":{"fileID":{"id":"f","name":"n"}}}), uuid: "A1".into() },
            r#"{"appendAttachment":{"attachment":{"inline":false,"id":"a","type":{"fileID":{"id":"f","name":"n"}}},"uuid":"A1"}}"#,
        );
        round(Content::WatcherEvent(json!({"_0":{"detail":"d"}})), r#"{"watcherEvent":{"_0":{"detail":"d"}}}"#);
        round(Content::ExplanationRequest(json!({"request":{"question":"q"}})), r#"{"explanationRequest":{"request":{"question":"q"}}}"#);
        round(Content::ExplanationResponse(json!({"_0":{},"markdown":"m"})), r#"{"explanationResponse":{"_0":{},"markdown":"m"}}"#);
        let original = Message {
            seq: 0,
            unique_id: "U1".into(),
            author: Participant::User,
            chat_id: "C1".into(),
            content: Content::Markdown("m".into()),
            sent_date: 978307201.0,
            response_id: None,
            agent_reasoning: None,
            first_blob_ref: None,
        };
        round(
            Content::SelectSessionRequest { original: Box::new(original), terminal: true },
            r#"{"selectSessionRequest":{"_0":{"chatID":"C1","author":"user","content":{"markdown":{"_0":"m"}},"sentDate":1.0,"uniqueID":"U1"},"terminal":true}}"#,
        );
        // Every call rides a request and reads back.
        for call in all_calls() {
            let c = Content::RemoteCommandRequest { payload: classic(call.clone()), safe: None };
            assert_eq!(decode_content(&encode_content(&c)), c, "{call:?}");
        }
        assert_eq!(
            encode_content(&Content::RemoteCommandRequest { payload: classic(Call::ReadWebPage { starting_line_number: 3, number_of_lines: 4 }), safe: None }),
            r#"{"remoteCommandRequest":{"_0":{"llmMessage":{"role":"assistant"},"content":{"readWebPage":{"_0":{"startingLineNumber":3,"numberOfLines":4}}}}}}"#,
        );
        assert_eq!(
            encode_content(&Content::RemoteCommandRequest { payload: classic(Call::DetectSshSession), safe: None }),
            r#"{"remoteCommandRequest":{"_0":{"llmMessage":{"role":"assistant"},"content":{"detectSSHSession":{"_0":{}}}}}}"#,
        );
        // Arguments decode leniently: a missing field takes the Swift default, a float a whole number.
        let lenient = |content: &str| match decode_content(&format!(r#"{{"remoteCommandRequest":{{"_0":{{"llmMessage":{{}},"content":{content}}}}}}}"#)) {
            Content::RemoteCommandRequest { payload: Payload::Classic(rc), .. } => Some(rc.call),
            _ => None,
        };
        assert_eq!(lenient(r#"{"getCommandHistory":{"_0":{}}}"#), Some(Call::GetCommandHistory { limit: 100 }));
        assert_eq!(lenient(r#"{"getCommandHistory":{"_0":{"limit":5.0}}}"#), Some(Call::GetCommandHistory { limit: 5 }));
        assert_eq!(lenient(r#"{"executeCommand":{"_0":{}}}"#), Some(Call::ExecuteCommand { command: String::new() }));
        assert_eq!(lenient(r#"{"getScreenContents":{"_0":{"lines":null}}}"#), Some(Call::GetScreenContents { lines: 0 }));
        assert_eq!(lenient(r#"{"executeCommand":{"_0":{"command":5}}}"#), None);
    }

    #[test]
    fn unknown_cases_are_unsupported() {
        let future = r#"{"futureThing":{"_0":1}}"#;
        assert_eq!(decode_content(future), Content::Unsupported(future.to_string()));
        assert_eq!(decode_content("not json"), Content::Unsupported("not json".to_string()));
        let action = r#"{"clientLocal":{"_0":{"action":{"futureAction":{}}}}}"#;
        assert_eq!(decode_content(action), Content::Unsupported(action.to_string()));
        assert_eq!(decode_content(r#"{"clientLocal":{"_0":{"action":{"offerOrchestration":{}}}}}"#), Content::ClientLocal(ClientLocal::OfferOrchestration));
        let enable = r#"{"clientLocal":{"_0":{"action":{"enableOrchestrationRequest":{"requestID":"R"}}}}}"#;
        assert_eq!(decode_content(enable), Content::ClientLocal(ClientLocal::Other(json!({"enableOrchestrationRequest":{"requestID":"R"}}))));
        assert_eq!(encode_content(&decode_content(enable)), enable);
        let external = r#"{"remoteCommandRequest":{"_0":{"kind":"external","name":"send_text"}}}"#;
        assert_eq!(
            decode_content(external),
            Content::RemoteCommandRequest { payload: Payload::External(json!({"kind":"external","name":"send_text"})), safe: None }
        );
        assert_eq!(encode_content(&decode_content(external)), external);
        match decode_content(r#"{"remoteCommandRequest":{"_0":{"llmMessage":{},"content":{"loadURL":{"_0":{"url":"https://a"}}}}}}"#) {
            Content::RemoteCommandRequest { payload: Payload::Classic(rc), safe: None } => assert_eq!(rc.call, Call::LoadUrl { url: "https://a".into() }),
            other => panic!("{other:?}"),
        }
        // A known case whose body does not decode, an unknown tool, an unknown category, two keys.
        for bad in [
            r#"{"markdown":{"_0":5}}"#,
            r#"{"plainText":{}}"#,
            r#"{"remoteCommandRequest":{"_0":{"llmMessage":{},"content":{"futureTool":{"_0":{}}}}}}"#,
            r#"{"setPermissions":{"_0":["Run Commands","Fly"]}}"#,
            r#"{"markdown":{"_0":"a"},"commit":{"_0":"b"}}"#,
            r#"{"userCommand":{"_0":{"futureCommand":{}}}}"#,
            r#"{"clientLocal":{"_0":{"action":{"streamingChanged":{"_0":"sideways"}}}}}"#,
            r#"{"unsupported":{}}"#,
            "[]",
        ] {
            assert_eq!(decode_content(bad), Content::Unsupported(bad.to_string()), "{bad}");
        }
        assert_eq!(decode_content(r#"{"setPermissions":{"_0":["View History"]}}"#), Content::SetPermissions(vec![Category::ViewContents]));
        assert_eq!(UNSUPPORTED_TEXT, "This message requires a newer version of Tandem to view.");
        let t = r#"{"futureThing":  {"_0" : 1}}"#;
        assert_eq!(encode_content(&Content::Unsupported(t.to_string())), t);
    }

    #[test]
    fn message_json_follows_swift() {
        let mut m = Message {
            seq: 9,
            unique_id: "U1".into(),
            author: Participant::Agent,
            chat_id: "C1".into(),
            content: Content::PlainText { text: "hi".into(), context: None },
            sent_date: 978307200.5,
            response_id: None,
            agent_reasoning: None,
            first_blob_ref: None,
        };
        let v = encode_message(&m);
        assert_eq!(v.to_string(), r#"{"chatID":"C1","author":"agent","content":{"plainText":{"_0":"hi"}},"sentDate":0.5,"uniqueID":"U1"}"#);
        m.seq = 0;
        assert_eq!(decode_message(&v), Some(m.clone()));
        m.response_id = Some("r".into());
        m.agent_reasoning = Some("a".into());
        m.first_blob_ref = Some("b".into());
        let v = encode_message(&m);
        assert_eq!(
            v.to_string(),
            r#"{"chatID":"C1","author":"agent","content":{"plainText":{"_0":"hi"}},"sentDate":0.5,"uniqueID":"U1","responseID":"r","agentReasoning":"a","firstBlobRef":"b"}"#
        );
        assert_eq!(decode_message(&v), Some(m.clone()));
        // Unknown content inside a message degrades alone; a missing field fails the message.
        let odd = json!({"chatID":"C1","author":"user","content":{"future":{}},"sentDate":1,"uniqueID":"U2","inResponseTo":"x","configuration":{}});
        let read = decode_message(&odd).unwrap();
        assert_eq!(read.content, Content::Unsupported(r#"{"future":{}}"#.to_string()));
        assert_eq!(read.sent_date, 978307201.0);
        assert_eq!(read.author, Participant::User);
        assert_eq!(decode_message(&json!({"author":"user","content":{},"sentDate":1,"uniqueID":"U2"})), None);
        assert_eq!(decode_message(&json!({"chatID":"C","author":"robot","content":{},"sentDate":1,"uniqueID":"U2"})), None);
        let fresh = Message::new("C9", Participant::User, Content::Markdown("m".into()), 5.0);
        assert_eq!((fresh.seq, fresh.chat_id.as_str(), fresh.sent_date, fresh.response_id.clone()), (0, "C9", 5.0, None));
        assert_ne!(fresh.unique_id, Message::new("C9", Participant::User, Content::Markdown("m".into()), 5.0).unique_id);
    }

    #[test]
    fn ids_follow_swift() {
        let id = new_id();
        assert_eq!(id.len(), 36);
        assert_eq!(id, id.to_uppercase());
        assert_eq!(id.chars().filter(|&c| c == '-').count(), 4);
        assert_eq!(&id[14..15], "4");
        assert_eq!(Participant::parse("user"), Some(Participant::User));
        assert_eq!(Participant::parse("agent"), Some(Participant::Agent));
        assert_eq!(Participant::parse("User"), None);
        assert_eq!((Participant::User.as_str(), Participant::Agent.as_str()), ("user", "agent"));
    }

    fn every_content() -> Vec<Content> {
        let rc = RemoteCommand::of(json!({}), Call::IsAtPrompt);
        let m = Message::new("C", Participant::User, Content::Markdown("m".into()), 0.0);
        vec![
            Content::PlainText { text: "p".into(), context: None },
            Content::Markdown("m".into()),
            Content::ExplanationRequest(json!({})),
            Content::ExplanationResponse(json!({})),
            Content::RemoteCommandRequest { payload: Payload::Classic(rc.clone()), safe: None },
            Content::WatcherEvent(json!({})),
            Content::RemoteCommandResponse { result: Ok(String::new()), request_id: "A".into(), function_name: "f".into(), function_call_id: None },
            Content::SelectSessionRequest { original: Box::new(m), terminal: true },
            Content::ClientLocal(ClientLocal::PickingSession),
            Content::RenameChat("t".into()),
            Content::Append { text: "x".into(), uuid: "A".into() },
            Content::AppendAttachment { attachment: json!({}), uuid: "A".into() },
            Content::Commit("A".into()),
            Content::UserCommand(UserCommand::Stop),
            Content::SetPermissions(vec![]),
            Content::VectorStoreCreated("v".into()),
            Content::TerminalCommand(TerminalCommand { username: None, hostname: None, directory: None, command: "c".into(), output: String::new(), exit_code: 0, url: String::new() }),
            Content::Multipart { parts: vec![], vector_store_id: None },
            Content::Unsupported("{}".into()),
        ]
    }

    fn case_name(c: &Content) -> String {
        encode_content(c).split('"').nth(1).unwrap_or("unsupported").to_string()
    }

    #[test]
    fn message_rules_follow_iterm2() {
        let hidden_names: Vec<String> = every_content().iter().filter(|c| hidden(c)).map(case_name).collect();
        assert_eq!(hidden_names, ["remoteCommandResponse", "renameChat", "commit", "userCommand", "setPermissions", "vectorStoreCreated"]);
        let free: Vec<String> = every_content().iter().filter(|c| !locks_provider(c)).map(case_name).collect();
        assert_eq!(free, ["clientLocal", "renameChat", "commit", "userCommand", "setPermissions", "vectorStoreCreated"]);

        assert_eq!(SNIPPET_MAX, 40);
        assert_eq!(snippet(&Content::PlainText { text: "a".repeat(50), context: None }, 40), format!("{}\u{2026}", "a".repeat(39)));
        assert_eq!(snippet(&Content::PlainText { text: "a".repeat(40), context: None }, 40), "a".repeat(40));
        let m = Message::new("C", Participant::User, Content::Markdown("m".into()), 0.0);
        assert_eq!(snippet(&Content::SelectSessionRequest { original: Box::new(m), terminal: true }, 40), "Selecting session\u{2026}");
        assert_eq!(snippet(&Content::ClientLocal(ClientLocal::PickingSession), 40), "Selecting session\u{2026}");
        assert_eq!(snippet(&Content::ClientLocal(ClientLocal::Notice("n".into())), 40), "n");
        assert_eq!(snippet(&Content::ClientLocal(ClientLocal::StreamingChanged(Streaming::Active)), 40), "Sending commands to AI automatically");
        assert_eq!(snippet(&Content::ClientLocal(ClientLocal::StreamingChanged(Streaming::Stopped)), 40), "Stopped sending commands to AI");
        assert_eq!(snippet(&Content::ClientLocal(ClientLocal::StreamingChanged(Streaming::StoppedAutomatically)), 40), "Stopped sending commands to AI");
        let ls = RemoteCommand::of(json!({}), Call::ExecuteCommand { command: "ls".into() });
        assert_eq!(snippet(&Content::RemoteCommandRequest { payload: Payload::Classic(ls.clone()), safe: None }, 40), "Executing `ls`");
        assert_eq!(snippet(&Content::ClientLocal(ClientLocal::ExecutingCommand(ls)), 40), "Executing `ls`");
        assert_eq!(
            snippet(&Content::RemoteCommandRequest { payload: Payload::External(json!({"kind":"external","markdownDescription":"Sending text"})), safe: None }, 40),
            "Sending text"
        );
        let tc = TerminalCommand { username: None, hostname: None, directory: None, command: "c".into(), output: String::new(), exit_code: 0, url: String::new() };
        assert_eq!(snippet(&Content::TerminalCommand(tc.clone()), 40), "Ran `c`");
        assert_eq!(snippet(&Content::TerminalCommand(TerminalCommand { command: "c".repeat(50), ..tc }), 40), format!("Ran `{}\u{2026}`", "c".repeat(35)));
        assert_eq!(
            snippet(&Content::RemoteCommandResponse { result: Ok(String::new()), request_id: "A".into(), function_name: "f".into(), function_call_id: None }, 40),
            "Finished executing command"
        );
        assert_eq!(snippet(&Content::Multipart { parts: vec![], vector_store_id: None }, 40), "Empty message");
        assert_eq!(snippet(&Content::Multipart { parts: vec![Subpart::Context("k".into())], vector_store_id: None }, 40), "Empty message");
        let parts = vec![Subpart::PlainText("first".into()), Subpart::Attachment(json!({"inline":false,"id":"a","type":{"file":{"_0":{"name":"notes.txt","content":"","mimeType":"text/plain"}}}})), Subpart::Context("k".into())];
        assert_eq!(snippet(&Content::Multipart { parts, vector_store_id: None }, 40), "notes.txt");
        let code = vec![Subpart::Attachment(json!({"inline":true,"id":"a","type":{"code":{"_0":"x".repeat(45)}}}))];
        assert_eq!(snippet(&Content::Multipart { parts: code, vector_store_id: None }, 40), format!("{}\u{2026}", "x".repeat(39)));
        let status = vec![Subpart::Attachment(json!({"inline":true,"id":"a","type":{"statusUpdate":{"_0":{"webSearchFinished":{"_0":"cats"}}}}}))];
        assert_eq!(snippet(&Content::Multipart { parts: status, vector_store_id: None }, 40), "Finished searching the web for cats.");
        assert_eq!(snippet(&Content::ExplanationRequest(json!({"request":{"command":"ls","question":"","subjectMatter":"s"}})), 40), "Explain ls");
        assert_eq!(snippet(&Content::ExplanationRequest(json!({"request":{"question":"why","subjectMatter":"s"}})), 40), "why");
        assert_eq!(snippet(&Content::ExplanationRequest(json!({"request":{"question":"","subjectMatter":"s"}})), 40), "Explain s");
        assert_eq!(snippet(&Content::ExplanationResponse(json!({"_0":{},"markdown":"md"})), 40), "md");
        assert_eq!(snippet(&Content::WatcherEvent(json!({"_0":{"detail":"went idle"}})), 40), "went idle");
        for none in [Content::RenameChat("t".into()), Content::Commit("A".into()), Content::Unsupported("{}".into()), Content::ClientLocal(ClientLocal::OfferOrchestration)] {
            assert_eq!(snippet(&none, 40), "", "{none:?}");
        }
    }

    /// Final review I1: a browser request keeps the site it was asked about (`termurionSite`), and a
    /// request without one is written as before.
    #[test]
    fn a_browser_request_keeps_its_site() {
        let rc = RemoteCommand { site: Some("a.test".into()), ..RemoteCommand::of(json!({}), Call::GetUrl) };
        let c = Content::RemoteCommandRequest { payload: Payload::Classic(rc.clone()), safe: None };
        assert_eq!(decode_content(&encode_content(&c)), c);
        assert!(rc.consent_text().contains("a\\.test"), "{}", rc.consent_text());
        let plain = Content::RemoteCommandRequest { payload: Payload::Classic(RemoteCommand::of(json!({}), Call::GetUrl)), safe: None };
        assert!(!encode_content(&plain).contains("termurionSite"));
        assert_eq!(RemoteCommand::of(json!({}), Call::GetUrl).consent_text(), Call::GetUrl.consent_description());
    }
}

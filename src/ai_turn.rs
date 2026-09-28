//! One AI request turn: the key, the request built by the model's wire format (`Dialect`), the
//! streamed or whole answer, the tool calls it asks for and the continuations that answer them,
//! and iTerm2's failure texts. `Turn` is the pure state machine; `Runner` joins it to a
//! `Transport` (and, on Windows, the wire log). Ports `AITerm/AITerm.swift` (`T:83-219`,
//! `T:360-704`, `T:928-1481`) and `LLMRequestBuilder.swift:48-53`.

use crate::ai_catalog::{Effort, Feature, Model, ServiceTier};
use crate::ai_model::{display_name, has, supports_previous_response_id};
use crate::ai_msg::{context_limit, truncate, Attachment, Body, CallId, FunctionCall, Message, Role};
use crate::ai_policy::{self, KeyPolicy, SELF_HOSTED_PLACEHOLDER};
use crate::ai_settings::{self as ai, Api, Vendor};
use crate::ai_sse::{self, DONE};
use crate::ai_transport::{self, Call, HttpEvent, HttpRequest, Notify, Options, Transport};
use crate::globals::Globals;
use crate::pwm::Secret;
use std::sync::Arc;

/// The App Review key: a turn with exactly this key contacts nothing and answers
/// `REVIEW_RESPONSE` (`T:596-634`).
pub const REVIEW_KEY: &str = "placeholder";
/// Decision 11: "your PC" for "your Mac".
pub const REVIEW_RESPONSE: &str = "This is a demonstration response from the Tandem AI assistant. Tandem is configured with a placeholder key for App Review, so no external AI service is contacted and no data leaves your PC. To get real answers, set your AI provider\u{2019}s API key in Tandem Settings > AI.";
/// Consecutive tool failures (or unknown tool names) that end a turn (`T:70-71`).
pub const MAX_TOOL_FAILURES: u32 = 6;

pub const NO_MODEL: &str = "No AI model configured in settings.";
pub const NO_KEY: &str = "Could not obtain a valid API key registration for this model\u{2019}s provider.";
pub const UNKNOWN_FUNCTION: &str = "There is no registered function by that name. Try again.";
pub const TOKEN_LIMIT: &str = "AI token limit exceeded because the conversation reached its maximum length";
pub const UNEXPECTED_EOF: &str = "Unexpected end of file from server";
pub const EMPTY_RESPONSE: &str = "Empty response from server";
/// Ruling 7.
pub const APPLE_UNAVAILABLE: &str = "Apple Intelligence is not available on Windows.";

/// `pref` is `AitermURL`, as iTerm2's text names it (`T:688`).
pub fn invalid_provider_url(pref: &str) -> String {
    format!("Invalid URL for AI provider of {pref}")
}

/// "Error from <provider>: <error>", then the server's reason after one space when it has one
/// (`T:372-379`).
pub fn provider_error(provider: &str, error: &str, reason: Option<&str>) -> String {
    match reason.filter(|r| !r.is_empty()) {
        Some(reason) => format!("Error from {provider}: {error} {reason}"),
        None => format!("Error from {provider}: {error}"),
    }
}

/// Streamed when a streaming request fails (`T:576-583`); `details` is already indented.
pub fn request_failed(details: &str) -> String {
    format!("\n\n**Request failed**\n\n{details}")
}

/// Sent back to the model when a tool fails (`T:1428`).
pub fn tool_failed(error: &str) -> String {
    format!("Tool call failed: {error}")
}

pub fn could_not_decode(reason: &str) -> String {
    format!("Could not decode response: {reason}")
}

/// `data` is the body as received (already lossy UTF-8, so iTerm2's hex form has no case).
pub fn failed_to_decode(error: &str, data: &str) -> String {
    format!("Failed to decode API response: {error}. Data is: {data}")
}

/// The server's reason in an error body (`LLMErrorParser`, `LegacyOpenAI.swift:148-181`):
/// `{"error":{"message":...}}` (OpenAI, Anthropic, Google) or `{"error":"..."}` (Ollama).
pub fn error_reason(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    let error = v.get("error")?;
    error.get("message").and_then(|m| m.as_str()).or_else(|| error.as_str()).map(str::to_string)
}

/// A tool the model may call.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolDecl {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// The vendor-hosted tools a request enables (`HostedTools`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hosted {
    pub file_search_vector_stores: Vec<String>,
    pub web_search: bool,
    pub code_interpreter: bool,
}

/// The settings a request body reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wire {
    pub token_limit: i64,
    pub response_limit: i64,
    pub zdr: bool,
    pub ollama_num_ctx: i64,
    pub ollama_keep_alive: String,
}

impl Wire {
    pub fn from_globals(g: &Globals) -> Wire {
        Wire {
            token_limit: g.int(ai::TOKEN_LIMIT),
            response_limit: g.int(ai::RESPONSE_TOKEN_LIMIT),
            zdr: g.bool(ai::ZERO_DATA_RETENTION),
            ollama_num_ctx: g.int("ollamaNumCtx"),
            ollama_keep_alive: g.string("ollamaKeepAlive"),
        }
    }
}

/// Everything a wire format needs to build one request.
pub struct Build<'a> {
    pub model: &'a Model,
    pub key: &'a str,
    pub messages: &'a [Message],
    pub tools: &'a [ToolDecl],
    pub stream: bool,
    pub hosted: &'a Hosted,
    pub previous_response_id: Option<&'a str>,
    pub should_think: Option<bool>,
    pub effort: Option<Effort>,
    pub tier: Option<ServiceTier>,
    /// Per-request context sent after the cache breakpoint.
    pub volatile: Option<&'a str>,
    /// The chat's frozen history, spliced into the body as it was sent before.
    pub frozen: Option<&'a [u8]>,
    pub wire: &'a Wire,
}

/// One parsed answer or streamed event (iTerm2's `AnyResponse` / `AnyStreamingResponse`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Parsed {
    pub messages: Vec<Message>,
    pub response_id: Option<String>,
    pub prompt_tokens: Option<i64>,
    /// A status event nobody shows; its usage still counts.
    pub ignore: bool,
    /// A mid-stream error event's message (ruling 12b-3): the turn fails with it.
    pub error: Option<String>,
}

/// A wire format: the request body and URL, and the parsers of its answers.
pub trait Dialect: Send + Sync {
    fn url(&self, b: &Build) -> String {
        b.model.url.clone()
    }
    fn body(&self, b: &Build) -> Result<Vec<u8>, String>;
    /// A whole answer; Ok(None) is iTerm2's "no response".
    fn parse(&self, body: &[u8]) -> Result<Option<Parsed>, String>;
    fn split(&self, buffer: &str) -> (Option<String>, String) {
        ai_sse::split_sse(buffer)
    }
    /// One streamed event; Ok(None) ends the stream.
    fn parse_event(&self, json: &str) -> Result<Option<Parsed>, String>;
}

/// The wire format of an API; Tasks 4 to 8 add one arm each, and Apple Intelligence has none
/// (ruling 7).
pub fn dialect_for(api: Api) -> Option<Box<dyn Dialect>> {
    match api {
        Api::Responses => Some(Box::new(crate::ai_responses::Responses)),
        Api::ChatCompletions => Some(Box::new(crate::ai_chat::ChatCompletions)),
        Api::EarlyO1 => Some(Box::new(crate::ai_chat::EarlyO1)),
        Api::Completions => Some(Box::new(crate::ai_chat::Completions)),
        Api::Anthropic => Some(Box::new(crate::ai_anthropic::Anthropic)),
        Api::Gemini => Some(Box::new(crate::ai_gemini::Gemini)),
        Api::Llama | Api::DeepSeek | Api::AppleIntelligence => None,
    }
}

/// The request's headers (`RB:48-53`): the key's, the JSON content type, then the model's custom
/// headers, which replace a built-in one that differs only in case.
pub fn headers(b: &Build) -> Vec<(String, String)> {
    let mut base = ai_policy::auth_headers(&b.model.url, b.model.api, b.key);
    base.push(("Content-Type".to_string(), "application/json".to_string()));
    ai_policy::merged_headers(&base, &b.model.custom_headers)
}

/// The response token budget for this request (`P:303-317`): the query is every message's
/// content joined with "\n", a message without content (a call, a file) counting as "" with its
/// separator (`Body.content`, `LLM.swift:220-222`); the functions count as their declarations'
/// JSON only when the model has function calling (Swift escapes "/" and writes its own schema
/// encoding, so a length can differ by those bytes).
pub fn max_tokens_for(b: &Build) -> i64 {
    let query = b.messages.iter().map(|m| m.content().unwrap_or_default()).collect::<Vec<_>>().join("\n");
    let functions = if b.tools.is_empty() || !has(b.model, Feature::FunctionCalling) {
        String::new()
    } else {
        let decls: Vec<serde_json::Value> = b.tools.iter().map(|t| serde_json::json!({"name": t.name, "description": t.description, "parameters": t.parameters})).collect();
        serde_json::Value::Array(decls).to_string()
    };
    crate::ai_model::max_tokens(b.model, b.wire.token_limit, &query, &functions)
}

pub fn request(b: &Build, d: &dyn Dialect) -> Result<HttpRequest, String> {
    Ok(HttpRequest { method: "POST", url: d.url(b), headers: headers(b), body: d.body(b)?, stream: b.stream })
}

/// What a turn asks of its owner.
#[derive(Clone, Debug, PartialEq)]
pub enum Out {
    /// Ask the person for this vendor's key, then call `key_given`.
    NeedKey(Vendor),
    /// Start this request and feed its events to `http` (the `Runner` does it).
    Send(HttpRequest),
    WillSend,
    /// Streamed answer text.
    Delta(String),
    Attachment(Attachment),
    /// The answer's reasoning, before `StreamEnd` or `Choice` (DeepSeek replays it).
    Reasoning(String),
    /// Run this tool, then call `tool_done`; `id` is the call message's `CallId` (ruling 12c-1).
    Invoke { call: FunctionCall, id: Option<CallId> },
    /// The whole answer of a request that did not stream.
    Choice(String),
    StreamEnd,
    Failed(String),
    Canceled,
}

/// How a tool call ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolResult {
    Ok(String),
    Failed(String),
    /// The person stopped it (iTerm2's `PendingCommandCanceled`): the turn ends at once.
    Canceled(String),
}

pub struct TurnInput {
    pub model: Model,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDecl>,
    pub stream: bool,
    pub hosted: Hosted,
    pub previous_response_id: Option<String>,
    pub should_think: Option<bool>,
    pub effort: Option<Effort>,
    pub tier: Option<ServiceTier>,
    /// Called once per request, continuations included (`T:280-290`).
    pub volatile: Box<dyn FnMut() -> Option<String> + Send>,
    pub frozen: Option<Vec<u8>>,
    pub wire: Wire,
    /// `AitermURL`, named only in the invalid-URL text.
    pub legacy_url_pref: String,
}

/// A streamed answer so far: the message accumulating, the text not yet split into events, and
/// whether an end marker was seen.
struct StreamState {
    message: Message,
    rest: String,
    ended: bool,
}

impl StreamState {
    fn new() -> StreamState {
        StreamState { message: Message::new(None, Body::Uninitialized), rest: String::new(), ended: false }
    }
}

enum State {
    Ground,
    AwaitingKey { messages: Vec<Message> },
    /// A request is out; `stream` is Some when it streams.
    Sent { messages: Vec<Message>, stream: Option<StreamState> },
    /// A tool runs; `messages` end with the message that called it.
    AwaitingTool { messages: Vec<Message>, call: FunctionCall, id: Option<CallId> },
}

/// The key a request uses.
enum Key<'a> {
    Review,
    Use(&'a str),
}

pub struct Turn {
    model: Model,
    messages: Vec<Message>,
    tools: Vec<ToolDecl>,
    stream: bool,
    hosted: Hosted,
    previous_response_id: Option<String>,
    should_think: Option<bool>,
    effort: Option<Effort>,
    tier: Option<ServiceTier>,
    volatile: Box<dyn FnMut() -> Option<String> + Send>,
    frozen: Option<Vec<u8>>,
    wire: Wire,
    legacy_url_pref: String,
    dialect: Option<Arc<dyn Dialect>>,
    key: Option<Secret>,
    prompt_tokens: Option<i64>,
    failures: u32,
    /// The answer text streamed during the current request.
    streamed: String,
    started: bool,
    state: State,
}

impl Turn {
    pub fn new(input: TurnInput) -> Turn {
        let dialect = dialect_for(input.model.api).map(Arc::from);
        Turn::build(input, dialect)
    }

    #[cfg(test)]
    pub fn with_dialect(input: TurnInput, dialect: Box<dyn Dialect>) -> Turn {
        Turn::build(input, Some(Arc::from(dialect)))
    }

    fn build(input: TurnInput, dialect: Option<Arc<dyn Dialect>>) -> Turn {
        // Tools are defined only for a model with function calling (`T:343-348`).
        let tools = if has(&input.model, Feature::FunctionCalling) { input.tools } else { Vec::new() };
        Turn {
            model: input.model,
            messages: input.messages,
            tools,
            stream: input.stream,
            hosted: input.hosted,
            previous_response_id: input.previous_response_id,
            should_think: input.should_think,
            effort: input.effort,
            tier: input.tier,
            volatile: input.volatile,
            frozen: input.frozen,
            wire: input.wire,
            legacy_url_pref: input.legacy_url_pref,
            dialect,
            key: None,
            prompt_tokens: None,
            failures: 0,
            streamed: String::new(),
            started: false,
            state: State::Ground,
        }
    }

    /// The server's response id for the next turn's `previous_response_id`.
    pub fn previous_response_id(&self) -> Option<&str> {
        self.previous_response_id.as_deref()
    }

    /// The prompt tokens the vendor last reported.
    pub fn prompt_tokens(&self) -> Option<i64> {
        self.prompt_tokens
    }

    pub fn is_idle(&self) -> bool {
        matches!(self.state, State::Ground)
    }

    fn waiting_for_http(&self) -> bool {
        matches!(self.state, State::Sent { .. })
    }

    /// Starts the turn with the stored key, if any; a turn starts once.
    pub fn start(&mut self, key: Option<&Secret>) -> Vec<Out> {
        if self.started {
            return Vec::new();
        }
        self.started = true;
        self.key = key.map(Secret::duplicate);
        let messages = std::mem::take(&mut self.messages);
        match self.key() {
            None => {
                self.state = State::AwaitingKey { messages };
                vec![Out::NeedKey(self.model.vendor)]
            }
            Some(_) => self.send(messages),
        }
    }

    /// The key the turn's requests carry, for the wire log to hide; None for the self-hosted
    /// placeholder, which is no secret.
    pub fn wire_key(&self) -> Option<&str> {
        match self.key() {
            Some(Key::Use(key)) if key != SELF_HOSTED_PLACEHOLDER => Some(key),
            _ => None,
        }
    }

    /// The person's answer to `NeedKey`.
    pub fn key_given(&mut self, key: Option<Secret>) -> Vec<Out> {
        if !matches!(self.state, State::AwaitingKey { .. }) {
            return Vec::new();
        }
        let State::AwaitingKey { messages } = std::mem::replace(&mut self.state, State::Ground) else {
            return Vec::new();
        };
        self.key = key;
        if self.key().is_none() {
            return vec![Out::Failed(NO_KEY.to_string())];
        }
        self.send(messages)
    }

    /// iTerm2's `registration` (`T:83-96`, `T:200-219`): the review key wins over everything;
    /// then a model that must not receive the vendor key gets the placeholder; then the given
    /// key, when it is not blank.
    fn key(&self) -> Option<Key<'_>> {
        if self.key.as_ref().is_some_and(|k| k.expose() == REVIEW_KEY) {
            return Some(Key::Review);
        }
        if ai_policy::key_policy(&self.model.url, self.model.api) != KeyPolicy::VendorKey {
            return Some(Key::Use(SELF_HOSTED_PLACEHOLDER));
        }
        self.key.as_ref().filter(|k| !k.expose().trim_start().is_empty()).map(|k| Key::Use(k.expose()))
    }

    /// `requestCompletion` (`T:648-704`).
    fn send(&mut self, messages: Vec<Message>) -> Vec<Out> {
        self.state = State::Ground;
        if self.model.api == Api::AppleIntelligence {
            return vec![Out::Failed(APPLE_UNAVAILABLE.to_string())];
        }
        if matches!(self.key(), Some(Key::Review)) {
            self.failures = 0;
            return vec![Out::Choice(REVIEW_RESPONSE.to_string())];
        }
        let Some(dialect) = self.dialect.clone() else {
            // Every API but Apple Intelligence has a wire format once Tasks 4 to 8 are in.
            return vec![Out::Failed(NO_MODEL.to_string())];
        };
        let volatile = (self.volatile)();
        if ai_transport::parse_url(&self.model.url).is_err() {
            return vec![Out::Failed(invalid_provider_url(&self.legacy_url_pref))];
        }
        let built = {
            let Some(Key::Use(key)) = self.key() else {
                return vec![Out::Failed(NO_KEY.to_string())];
            };
            let m = &self.model;
            let b = Build {
                model: m,
                key,
                messages: &messages,
                tools: &self.tools,
                stream: self.stream,
                hosted: &self.hosted,
                previous_response_id: self.previous_response_id.as_deref().filter(|_| supports_previous_response_id(m, self.wire.zdr)),
                should_think: self.should_think.filter(|_| has(m, Feature::ConfigurableThinking)),
                effort: self.effort.filter(|e| m.reasoning_efforts.contains(e)),
                tier: self.tier.filter(|t| m.service_tiers.contains(t)),
                volatile: volatile.as_deref(),
                frozen: self.frozen.as_deref(),
                wire: &self.wire,
            };
            request(&b, dialect.as_ref())
        };
        match built {
            Err(e) => vec![Out::Failed(e)],
            // A dialect that builds its own URL (Gemini's "about:empty") can still make one that
            // is not sent (`P:68-70`).
            Ok(req) if ai_transport::parse_url(&req.url).is_err() => vec![Out::Failed(invalid_provider_url(&self.legacy_url_pref))],
            Ok(req) => {
                self.streamed.clear();
                self.state = State::Sent { messages, stream: self.stream.then(StreamState::new) };
                vec![Out::WillSend, Out::Send(req)]
            }
        }
    }

    /// Ends the turn with `text`; a streaming request that was out also streams it
    /// (`T:572-585`), unless the person canceled a tool.
    fn fail(&mut self, text: String, streamed: bool) -> Vec<Out> {
        self.state = State::Ground;
        let mut out = Vec::new();
        if streamed && self.stream {
            let details = text.split('\n').map(|l| format!("    {l}")).collect::<Vec<_>>().join("\n");
            out.push(Out::Delta(request_failed(&details)));
        }
        out.push(Out::Failed(text));
        out
    }

    /// An event of the request that is out; anything else is ignored.
    pub fn http(&mut self, event: HttpEvent) -> Vec<Out> {
        let State::Sent { stream, .. } = &self.state else {
            return Vec::new();
        };
        let streaming = stream.is_some();
        match event {
            HttpEvent::Chunk(text) if streaming => self.chunk(&text),
            HttpEvent::Chunk(_) => Vec::new(),
            HttpEvent::Done { body, error: Some(error) } if !error.is_empty() => {
                // `T:545-552`, with the withheld-key advice for the model's endpoint.
                let message = provider_error(display_name(&self.model), &error, error_reason(body.as_bytes()).as_deref());
                let message = ai_policy::amended(&message, &error, &self.model.url, self.model.api, &self.model.custom_headers);
                self.fail(message, true)
            }
            HttpEvent::Done { body, .. } if streaming => self.stream_end(&body),
            HttpEvent::Done { body, .. } => self.whole(&body),
        }
    }

    /// `parseNonStreamingResponse` (`T:1197-1251`).
    fn whole(&mut self, body: &str) -> Vec<Out> {
        let Some(dialect) = self.dialect.clone() else {
            return Vec::new();
        };
        let parsed = match dialect.parse(body.as_bytes()) {
            Err(e) => {
                let text = match error_reason(body.as_bytes()) {
                    Some(reason) => could_not_decode(&reason),
                    None => failed_to_decode(&e, body),
                };
                return self.fail(text, true);
            }
            Ok(None) => return self.fail(UNEXPECTED_EOF.to_string(), false),
            Ok(Some(p)) => p,
        };
        if let Some(id) = parsed.response_id {
            self.previous_response_id = Some(id);
        }
        if let Some(t) = parsed.prompt_tokens {
            self.prompt_tokens = Some(t);
        }
        let Some(message) = parsed.messages.into_iter().next() else {
            return self.fail(EMPTY_RESPONSE.to_string(), false);
        };
        if let Some((call, _)) = message.function_call() {
            let call = call.clone();
            return self.dispatch(message, call);
        }
        let mut out = Vec::new();
        if let Some(r) = message.reasoning_content.as_ref().filter(|r| !r.is_empty()) {
            out.push(Out::Reasoning(r.clone()));
        }
        self.failures = 0;
        self.state = State::Ground;
        out.push(Out::Choice(message.trimmed_text().unwrap_or_default()));
        out
    }

    /// One streamed chunk (`T:1005-1137`): events are split off the buffer and merged into the
    /// accumulating message; a part that does not merge drains what came before. Accumulated
    /// text and attachments drain at the end of every chunk.
    fn chunk(&mut self, text: &str) -> Vec<Out> {
        let (Some(dialect), State::Sent { stream: Some(st), .. }) = (self.dialect.clone(), &mut self.state) else {
            return Vec::new();
        };
        let mut st = std::mem::replace(st, StreamState::new());
        let mut out = Vec::new();
        let mut buffer = std::mem::take(&mut st.rest) + text;
        let mut call = None;
        while !st.ended && call.is_none() {
            let (first, rest) = dialect.split(&buffer);
            buffer = rest;
            let Some(event) = first else {
                break;
            };
            if event == DONE {
                st.ended = true;
                break;
            }
            match dialect.parse_event(&event) {
                Ok(None) => st.ended = true,
                Err(_) => call = drain(&mut st.message, &mut out, &mut self.streamed),
                Ok(Some(p)) => {
                    // Usage counts even on an ignored event.
                    if let Some(t) = p.prompt_tokens {
                        self.prompt_tokens = Some(t);
                    }
                    if let Some(error) = p.error {
                        // Text that came before the error still shows; a call it cut short
                        // does not run.
                        if matches!(st.message.body, Body::Text(_) | Body::Attachment(_)) {
                            drain(&mut st.message, &mut out, &mut self.streamed);
                        }
                        out.extend(self.fail(provider_error(display_name(&self.model), &error, None), true));
                        return out;
                    }
                    if p.ignore {
                        continue;
                    }
                    if let Some(id) = p.response_id {
                        self.previous_response_id = Some(id);
                    }
                    for choice in p.messages {
                        accumulate(&mut st.message, &choice);
                        if !st.message.try_append(&choice.body) {
                            call = drain(&mut st.message, &mut out, &mut self.streamed);
                            if call.is_some() {
                                break;
                            }
                            st.message = Message {
                                response_id: self.previous_response_id.clone(),
                                role: choice.role,
                                body: choice.body,
                                reasoning_content: st.message.reasoning_content.take(),
                                reasoning_items: std::mem::take(&mut st.message.reasoning_items),
                            };
                        }
                    }
                }
            }
        }
        if let Some(message) = call {
            // A call that another part followed is complete: it runs now and the rest of this
            // answer is dropped (iTerm2 would run every call at once).
            let call = message.function_call().map(|(c, _)| c.clone()).unwrap_or_default();
            out.extend(self.dispatch(message, call));
            return out;
        }
        if matches!(st.message.body, Body::Text(_) | Body::Attachment(_)) {
            drain(&mut st.message, &mut out, &mut self.streamed);
        }
        st.rest = if st.ended { String::new() } else { buffer };
        if let State::Sent { stream: Some(slot), .. } = &mut self.state {
            *slot = st;
        }
        out
    }

    /// The streamed request completed (`T:936-1004`): the whole body is parsed again, and its
    /// call wins when it is longer or the live stream saw none; a text answer gets any tail the
    /// live stream missed, then `StreamEnd`.
    fn stream_end(&mut self, body: &str) -> Vec<Out> {
        let (Some(dialect), State::Sent { stream: Some(st), .. }) = (self.dialect.clone(), &mut self.state) else {
            return Vec::new();
        };
        let live = std::mem::replace(&mut st.message, Message::new(None, Body::Uninitialized));
        let whole = reparse(dialect.as_ref(), body, self.previous_response_id.as_deref());
        let live_call = live.function_call().map(|(c, _)| c.clone());
        let whole_call = whole.as_ref().and_then(|m| m.function_call()).map(|(c, _)| c.clone());
        let arguments = |c: &FunctionCall| c.arguments.as_deref().unwrap_or("").chars().count();
        let prefer_whole = match (&whole_call, &live_call) {
            (Some(w), Some(l)) => arguments(w) > arguments(l),
            (Some(_), None) => true,
            (None, _) => false,
        };
        if let (true, Some(message), Some(call)) = (prefer_whole, whole.clone(), whole_call) {
            return self.dispatch(message, call);
        }
        if let Some(call) = live_call {
            return self.dispatch(live, call);
        }
        let mut out = Vec::new();
        let reasoning = whole.as_ref().and_then(|m| m.reasoning_content.clone()).or(live.reasoning_content);
        if let Some(r) = reasoning.filter(|r| !r.is_empty()) {
            out.push(Out::Reasoning(r));
        }
        if let Some(full) = whole.and_then(|m| m.content()) {
            let seen = self.streamed.chars().count();
            let tail: String = full.chars().skip(seen).collect();
            if !tail.is_empty() {
                self.streamed.push_str(&tail);
                out.push(Out::Delta(tail));
            }
        }
        self.failures = 0;
        self.state = State::Ground;
        out.push(Out::StreamEnd);
        out
    }

    /// `doFunctionCall` (`T:1341-1481`): a registered tool is invoked; an unknown name is told
    /// so and asked again, sharing the failure bound.
    fn dispatch(&mut self, message: Message, call: FunctionCall) -> Vec<Out> {
        if !self.waiting_for_http() {
            return Vec::new();
        }
        let State::Sent { messages, .. } = std::mem::replace(&mut self.state, State::Ground) else {
            return Vec::new();
        };
        let mut message = message;
        if message.response_id.is_none() {
            message.response_id = self.previous_response_id.clone();
        }
        let id = message.call_id().cloned();
        let mut amended = messages;
        amended.push(message);
        if self.tools.iter().any(|t| call.name.as_deref() == Some(t.name.as_str())) {
            self.state = State::AwaitingTool { messages: amended, call: call.clone(), id: id.clone() };
            return vec![Out::Invoke { call, id }];
        }
        self.failures += 1;
        if self.failures >= MAX_TOOL_FAILURES {
            self.failures = 0;
            self.previous_response_id = None;
            return self.fail(UNKNOWN_FUNCTION.to_string(), true);
        }
        match output_id(id, &call) {
            Some(id) => amended.push(function_message(call.name, UNKNOWN_FUNCTION, Some(id))),
            None => amended.push(Message::text(Role::User, UNKNOWN_FUNCTION)),
        }
        self.send(amended)
    }

    /// The tool's result: sent back to the model (truncated to fit), or the end of the turn.
    pub fn tool_done(&mut self, result: ToolResult) -> Vec<Out> {
        if !matches!(self.state, State::AwaitingTool { .. }) {
            return Vec::new();
        }
        let State::AwaitingTool { mut messages, call, id } = std::mem::replace(&mut self.state, State::Ground) else {
            return Vec::new();
        };
        let id = output_id(id, &call);
        let content = match result {
            ToolResult::Ok(output) => {
                self.failures = 0;
                output
            }
            ToolResult::Canceled(text) => {
                self.previous_response_id = None;
                self.failures = 0;
                return self.fail(text, false);
            }
            ToolResult::Failed(error) => {
                self.failures += 1;
                if self.failures >= MAX_TOOL_FAILURES {
                    self.failures = 0;
                    self.previous_response_id = None;
                    return self.fail(error, true);
                }
                tool_failed(&error)
            }
        };
        messages.push(function_message(call.name, &content, id));
        let messages = truncate(&messages, context_limit(self.wire.token_limit, self.wire.response_limit));
        self.send(messages)
    }

    /// Stops the turn: nothing it was waiting for is used afterwards.
    pub fn cancel(&mut self) -> Vec<Out> {
        match std::mem::replace(&mut self.state, State::Ground) {
            State::Ground => Vec::new(),
            _ => vec![Out::Canceled],
        }
    }
}

/// A streamed choice's role, reasoning and reasoning items go on the accumulating message
/// (`T:1080-1103`).
fn accumulate(message: &mut Message, choice: &Message) {
    if choice.role.is_some() {
        message.role = choice.role;
    }
    if let Some(r) = choice.reasoning_content.as_ref().filter(|r| !r.is_empty()) {
        message.reasoning_content = Some(format!("{}{r}", message.reasoning_content.as_deref().unwrap_or("")));
    }
    message.reasoning_items.extend(choice.reasoning_items.iter().cloned());
}

/// Hands on what the message accumulated (`T:1017-1041`): text as a `Delta`, an attachment as
/// itself; a call is returned for dispatch. The body starts over.
fn drain(message: &mut Message, out: &mut Vec<Out>, streamed: &mut String) -> Option<Message> {
    let mut call = None;
    match &message.body {
        Body::Text(s) if !s.is_empty() => {
            streamed.push_str(s);
            out.push(Out::Delta(s.clone()));
        }
        Body::Attachment(a) => out.push(Out::Attachment(a.clone())),
        Body::FunctionCall(..) => call = Some(message.clone()),
        // A server never streams tool output or a multipart part.
        _ => {}
    }
    message.body = Body::Uninitialized;
    call
}

/// The whole streamed body as the one message it ends with, without side effects
/// (`T:1144-1195`); a failed event is skipped.
fn reparse(dialect: &dyn Dialect, body: &str, response_id: Option<&str>) -> Option<Message> {
    let mut rest = body.to_string();
    let mut message = Message::new(None, Body::Uninitialized);
    let mut started = false;
    loop {
        let (first, next) = dialect.split(&rest);
        rest = next;
        let Some(event) = first else {
            break;
        };
        if event == DONE {
            break;
        }
        let Ok(Some(p)) = dialect.parse_event(&event) else {
            continue;
        };
        if p.ignore {
            continue;
        }
        for choice in p.messages {
            accumulate(&mut message, &choice);
            if !started {
                message.body = choice.body;
                started = true;
            } else if !message.try_append(&choice.body) {
                // Earlier parts were shown live; only the last logical part counts here.
                message = Message {
                    response_id: response_id.map(str::to_string),
                    role: choice.role,
                    body: choice.body,
                    reasoning_content: message.reasoning_content.take(),
                    reasoning_items: std::mem::take(&mut message.reasoning_items),
                };
            }
        }
    }
    started.then_some(message)
}

/// The ids a tool's answer quotes: the call message's, else the call's own id as both.
fn output_id(id: Option<CallId>, call: &FunctionCall) -> Option<CallId> {
    id.or_else(|| call.id.clone().map(|i| CallId { call_id: i.clone(), item_id: i }))
}

/// iTerm2's `Message(role: .function, content:, name:, functionCallID:)`: tool output when the
/// call has a name, else plain text.
fn function_message(name: Option<String>, content: &str, id: Option<CallId>) -> Message {
    let body = match name {
        Some(name) => Body::FunctionOutput { name, output: content.to_string(), id },
        None => Body::Text(content.to_string()),
    };
    Message::new(Some(Role::Function), body)
}

/// Joins a `Turn` to a `Transport`: each `Send` starts a call, and `poll` feeds the call's events
/// to the turn.
pub struct Runner {
    turn: Turn,
    transport: Arc<dyn Transport>,
    options: Options,
    notify: Notify,
    call: Option<Box<dyn Call>>,
    #[cfg(windows)]
    log: Option<(Arc<std::sync::Mutex<crate::ai_wire_log::WireLog>>, bool)>,
    /// The live call's wire log id and start.
    #[cfg(windows)]
    logged: Option<(String, std::time::Instant)>,
}

impl Runner {
    pub fn new(turn: Turn, transport: Arc<dyn Transport>, options: Options, notify: Notify) -> Runner {
        Runner {
            turn,
            transport,
            options,
            notify,
            call: None,
            #[cfg(windows)]
            log: None,
            #[cfg(windows)]
            logged: None,
        }
    }

    /// Writes the wire log's records while `enabled` (`aiChatRawWireLogging`).
    #[cfg(windows)]
    pub fn with_log(mut self, log: Arc<std::sync::Mutex<crate::ai_wire_log::WireLog>>, enabled: bool) -> Runner {
        self.log = Some((log, enabled));
        self
    }

    pub fn turn(&self) -> &Turn {
        &self.turn
    }

    pub fn start(&mut self, key: Option<&Secret>) -> Vec<Out> {
        let out = self.turn.start(key);
        self.route(out)
    }

    pub fn key_given(&mut self, key: Option<Secret>) -> Vec<Out> {
        let out = self.turn.key_given(key);
        self.route(out)
    }

    pub fn tool_done(&mut self, result: ToolResult) -> Vec<Out> {
        let out = self.turn.tool_done(result);
        self.route(out)
    }

    pub fn cancel(&mut self) -> Vec<Out> {
        if let Some(mut call) = self.call.take() {
            call.cancel();
        }
        self.turn.cancel()
    }

    /// Feeds the live call's events to the turn; never returns `Send`.
    pub fn poll(&mut self) -> Vec<Out> {
        let mut out = Vec::new();
        while let Some(event) = self.call.as_mut().and_then(|c| c.try_event()) {
            let done = matches!(event, HttpEvent::Done { .. });
            self.log_event(&event);
            if done {
                self.call = None;
            }
            let outs = self.turn.http(event);
            out.extend(self.route(outs));
            // A call the turn no longer reads (a tool runs, or it failed) stops.
            if !done && !self.turn.waiting_for_http() {
                if let Some(mut call) = self.call.take() {
                    call.cancel();
                }
            }
        }
        out
    }

    fn route(&mut self, outs: Vec<Out>) -> Vec<Out> {
        let mut kept = Vec::new();
        for o in outs {
            match o {
                Out::Send(request) => self.send(request),
                other => kept.push(other),
            }
        }
        kept
    }

    fn send(&mut self, request: HttpRequest) {
        if let Some(mut old) = self.call.take() {
            old.cancel();
        }
        self.log_request(&request);
        self.call = Some(self.transport.start(request, self.options.clone(), self.notify.clone()));
    }

    #[cfg(windows)]
    fn log_request(&mut self, request: &HttpRequest) {
        use crate::ai_wire_log::{iso_stamp, now_millis, request_record};
        if let Some((log, enabled)) = &self.log {
            let id = uuid::Uuid::new_v4().to_string().to_uppercase();
            let record = request_record(&id, &iso_stamp(now_millis()), request, self.turn.wire_key());
            log.lock().unwrap_or_else(|e| e.into_inner()).log(*enabled, &record);
            self.logged = Some((id, std::time::Instant::now()));
        }
    }

    #[cfg(not(windows))]
    fn log_request(&mut self, _request: &HttpRequest) {}

    #[cfg(windows)]
    fn log_event(&mut self, event: &HttpEvent) {
        use crate::ai_wire_log::{chunk_record, iso_stamp, now_millis, response_record};
        let (Some((log, enabled)), Some((id, started))) = (&self.log, &self.logged) else {
            return;
        };
        let stamp = iso_stamp(now_millis());
        let record = match event {
            HttpEvent::Chunk(text) => chunk_record(id, &stamp, text, self.turn.wire_key()),
            HttpEvent::Done { body, error } => response_record(id, &stamp, started.elapsed(), body, error.as_deref(), self.turn.wire_key()),
        };
        log.lock().unwrap_or_else(|e| e.into_inner()).log(*enabled, &record);
    }

    #[cfg(not(windows))]
    fn log_event(&mut self, _event: &HttpEvent) {}
}

/// Whether two runs of the same answer delivered in different chunks had the same outcome: the
/// same streamed text, the same status updates and other attachments' content, and the same
/// other outputs in the same order. How many `Delta`s there are, and how status updates merged,
/// follow the chunking (iTerm2 drains at the end of every chunk).
#[cfg(test)]
pub fn same_outcome(a: &[Out], b: &[Out]) -> bool {
    use crate::ai_msg::Status;
    fn text(o: &[Out]) -> String {
        o.iter().filter_map(|x| if let Out::Delta(s) = x { Some(s.as_str()) } else { None }).collect()
    }
    fn attachments(o: &[Out]) -> (Vec<Status>, String, Vec<Attachment>) {
        let (mut statuses, mut code, mut other) = (Vec::new(), String::new(), Vec::new());
        for x in o {
            match x {
                Out::Attachment(Attachment::Status { status, .. }) => statuses.extend(status.exploded()),
                Out::Attachment(Attachment::Code(c)) => code.push_str(c),
                Out::Attachment(a) => other.push(a.clone()),
                _ => {}
            }
        }
        (statuses, code, other)
    }
    fn rest(o: &[Out]) -> Vec<Out> {
        o.iter().filter(|x| !matches!(x, Out::Delta(_) | Out::Attachment(_))).cloned().collect()
    }
    text(a) == text(b) && attachments(a) == attachments(b) && rest(a) == rest(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_model::fixture;
    use crate::ai_msg::{Status, TRUNCATED};
    use crate::ai_transport::{transport_error, Proxy, Recorded, Reply};
    use serde_json::{json, Value};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;

    /// What the test dialect saw of each request it built.
    #[derive(Clone, Debug, PartialEq)]
    struct Seen {
        messages: Vec<Message>,
        tools: usize,
        stream: bool,
        previous: Option<String>,
        think: Option<bool>,
        effort: Option<Effort>,
        tier: Option<ServiceTier>,
        volatile: Option<String>,
    }

    type Log = Arc<Mutex<Vec<Seen>>>;

    /// `body` writes `{"n":<message count>,"stream":<bool>}`; the parsers read `{"text"}`,
    /// `{"call":{"name","arguments","id"}}` (the id as both halves of the `CallId`) or
    /// `{"status"}`, with optional `reasoning`, `id`, `tokens` and `ignore`; `{"end":true}` ends
    /// the stream and `{"empty":true}` has no messages.
    struct TestDialect {
        seen: Log,
        fail_body: bool,
    }

    fn test_parse(v: Value) -> Result<Option<Parsed>, String> {
        if v["end"] == json!(true) {
            return Ok(None);
        }
        let mut p = Parsed {
            messages: Vec::new(),
            response_id: v["id"].as_str().map(str::to_string),
            prompt_tokens: v["tokens"].as_i64(),
            ignore: v["ignore"].as_bool().unwrap_or(false),
            error: None,
        };
        if v["empty"] == json!(true) {
            return Ok(Some(p));
        }
        let body = if let Some(t) = v["text"].as_str() {
            Body::Text(t.to_string())
        } else if let Some(c) = v.get("call") {
            let s = |k: &str| c[k].as_str().map(str::to_string);
            let id = s("id").map(|i| CallId { call_id: i.clone(), item_id: i });
            Body::FunctionCall(FunctionCall { name: s("name"), arguments: s("arguments"), id: s("id"), thought_signature: None }, id)
        } else if let Some(s) = v["status"].as_str() {
            Body::Attachment(Attachment::Status { id: "st".into(), inline: true, status: Status::ReasoningSummary(s.to_string()) })
        } else {
            return Err("unknown event".to_string());
        };
        let mut m = Message::new(Some(Role::Assistant), body);
        m.reasoning_content = v["reasoning"].as_str().map(str::to_string);
        p.messages.push(m);
        Ok(Some(p))
    }

    impl Dialect for TestDialect {
        fn body(&self, b: &Build) -> Result<Vec<u8>, String> {
            self.seen.lock().unwrap().push(Seen {
                messages: b.messages.to_vec(),
                tools: b.tools.len(),
                stream: b.stream,
                previous: b.previous_response_id.map(str::to_string),
                think: b.should_think,
                effort: b.effort,
                tier: b.tier,
                volatile: b.volatile.map(str::to_string),
            });
            if self.fail_body {
                return Err(TOKEN_LIMIT.to_string());
            }
            Ok(format!("{{\"n\":{},\"stream\":{}}}", b.messages.len(), b.stream).into_bytes())
        }

        fn parse(&self, body: &[u8]) -> Result<Option<Parsed>, String> {
            test_parse(serde_json::from_slice(body).map_err(|e| e.to_string())?)
        }

        fn parse_event(&self, json: &str) -> Result<Option<Parsed>, String> {
            test_parse(serde_json::from_str(json).map_err(|e| e.to_string())?)
        }
    }

    fn wire() -> Wire {
        Wire { token_limit: 128000, response_limit: 8192, zdr: false, ollama_num_ctx: 0, ollama_keep_alive: "30m".into() }
    }

    fn tool(name: &str) -> ToolDecl {
        ToolDecl { name: name.into(), description: String::new(), parameters: json!({"type": "object"}) }
    }

    fn input(model: Model, stream: bool) -> TurnInput {
        TurnInput {
            model,
            messages: vec![Message::text(Role::User, "hi")],
            tools: vec![tool("f")],
            stream,
            hosted: Hosted::default(),
            previous_response_id: None,
            should_think: None,
            effort: None,
            tier: None,
            volatile: Box::new(|| None),
            frozen: None,
            wire: wire(),
            legacy_url_pref: String::new(),
        }
    }

    fn turn_of(input: TurnInput) -> (Turn, Log) {
        let seen = Log::default();
        (Turn::with_dialect(input, Box::new(TestDialect { seen: seen.clone(), fail_body: false })), seen)
    }

    fn openai() -> Model {
        fixture("gpt", "https://api.openai.com/v1/chat/completions", Api::ChatCompletions)
    }

    fn anthropic() -> Model {
        fixture("claude", "https://api.anthropic.com/v1/messages", Api::Anthropic)
    }

    fn key(s: &str) -> Secret {
        Secret::new(s.to_string())
    }

    /// A turn sent with key "sk-a"; its `WillSend` and `Send` are checked and dropped.
    fn sent(model: Model, stream: bool) -> (Turn, Log) {
        let (mut t, seen) = turn_of(input(model, stream));
        let out = t.start(Some(&key("sk-a")));
        assert!(matches!(&out[..], [Out::WillSend, Out::Send(_)]), "{out:?}");
        (t, seen)
    }

    /// A turn sent with a response id from an earlier turn.
    fn sent_after(model: Model, stream: bool, previous: &str) -> Turn {
        let mut i = input(model, stream);
        i.previous_response_id = Some(previous.into());
        let (mut t, _) = turn_of(i);
        assert!(matches!(&t.start(Some(&key("sk-a")))[..], [Out::WillSend, Out::Send(_)]));
        t
    }

    fn sse(events: &[&str]) -> String {
        events.iter().map(|e| format!("data: {e}\n\n")).collect()
    }

    fn text(s: &str) -> String {
        json!({"text": s}).to_string()
    }

    fn call_event(name: &str, arguments: &str, id: Option<&str>) -> String {
        match id {
            Some(id) => json!({"call": {"name": name, "arguments": arguments, "id": id}}).to_string(),
            None => json!({"call": {"name": name, "arguments": arguments}}).to_string(),
        }
    }

    const END: &str = r#"{"end":true}"#;

    /// Each chunk, then `Done` with `body` (the chunks joined when None).
    fn feed(t: &mut Turn, chunks: &[String], body: Option<String>) -> Vec<Out> {
        let mut out = Vec::new();
        for c in chunks {
            out.extend(t.http(HttpEvent::Chunk(c.clone())));
        }
        out.extend(t.http(HttpEvent::Done { body: body.unwrap_or_else(|| chunks.concat()), error: None }));
        out
    }

    fn invoke(name: &str, arguments: &str, id: Option<&str>) -> Out {
        Out::Invoke {
            call: FunctionCall { name: Some(name.into()), arguments: Some(arguments.into()), id: id.map(str::to_string), thought_signature: None },
            id: id.map(|i| CallId { call_id: i.into(), item_id: i.into() }),
        }
    }

    fn delta(s: &str) -> Out {
        Out::Delta(s.to_string())
    }

    fn failed(s: &str) -> Out {
        Out::Failed(s.to_string())
    }

    fn options() -> Options {
        Options { idle: Duration::ZERO, proxy: Proxy::Off }
    }

    fn runner(t: Turn, replies: Vec<Reply>) -> (Runner, Arc<Recorded>) {
        let rec = Arc::new(Recorded::new(replies));
        (Runner::new(t, rec.clone(), options(), Arc::new(|| {})), rec)
    }

    /// A turn for `model` answered by `reply`: everything after `WillSend`.
    fn answered(model: Model, stream: bool, reply: Reply) -> Vec<Out> {
        let (t, _) = turn_of(input(model, stream));
        let (mut r, _) = runner(t, vec![reply]);
        assert_eq!(r.start(Some(&key("sk-a"))), [Out::WillSend]);
        r.poll()
    }

    fn with_build<R>(m: &Model, key: &str, f: impl FnOnce(&Build) -> R) -> R {
        let (hosted, wire) = (Hosted::default(), wire());
        let messages = [Message::text(Role::User, "hi")];
        f(&Build {
            model: m,
            key,
            messages: &messages,
            tools: &[],
            stream: false,
            hosted: &hosted,
            previous_response_id: None,
            should_think: None,
            effort: None,
            tier: None,
            volatile: None,
            frozen: None,
            wire: &wire,
        })
    }

    fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect()
    }

    #[test]
    fn max_tokens_for_follows_iterm2() {
        let (hosted, small) = (Hosted::default(), Wire { token_limit: 100, ..wire() });
        let budget = |m: &Model, messages: &[Message], tools: &[ToolDecl]| {
            max_tokens_for(&Build {
                model: m,
                key: "sk-a",
                messages,
                tools,
                stream: false,
                hosted: &hosted,
                previous_response_id: None,
                should_think: None,
                effort: None,
                tier: None,
                volatile: None,
                frozen: None,
                wire: &small,
            })
        };
        // A tool continuation: the call has no content but still counts as "" with its "\n",
        // so the query is "ab\n\ncd", 6 bytes, 3 tokens.
        let call = Message::new(Some(Role::Assistant), Body::FunctionCall(FunctionCall { name: Some("f".into()), arguments: Some("{}".into()), id: None, thought_signature: None }, None));
        let output = Message::new(Some(Role::Function), Body::FunctionOutput { name: "f".into(), output: "cd".into(), id: None });
        let continuation = [Message::text(Role::User, "ab"), call, output];
        let m = openai();
        assert_eq!(budget(&m, &continuation, &[]), 97);
        // The declarations `[{"name":"f","description":"","parameters":{"type":"object"}}]`
        // are 62 bytes, 31 tokens, counted only with function calling.
        assert_eq!(budget(&m, &continuation, &[tool("f")]), 66);
        let mut plain = openai();
        plain.features.remove(&Feature::FunctionCalling);
        assert_eq!(budget(&plain, &continuation, &[tool("f")]), 97);
        let mut short = openai();
        short.max_response_tokens = 50;
        assert_eq!(budget(&short, &continuation, &[]), 50, "the model's own cap");
    }

    #[test]
    fn wire_reads_the_globals() {
        let g = Globals::empty(&std::env::temp_dir().join("tandem-ai-turn-test.toml"));
        assert_eq!(Wire::from_globals(&g), wire());
    }

    #[test]
    fn headers_follow_iterm2() {
        let responses = fixture("gpt", "https://api.openai.com/v1/responses", Api::Responses);
        let got = with_build(&responses, " sk-a \n", headers);
        assert!(got == h(&[("Authorization", "Bearer sk-a"), ("Content-Type", "application/json")]), "OpenAI: Authorization then Content-Type");
        let azure = fixture("gpt", "https://me.openai.azure.com/x", Api::ChatCompletions);
        let got = with_build(&azure, " sk-a \n", headers);
        assert!(got == h(&[("api-key", "sk-a"), ("Content-Type", "application/json")]), "Azure: api-key");
        let got = with_build(&anthropic(), "sk-a", headers);
        assert!(got == h(&[("x-api-key", "sk-a"), ("anthropic-version", "2023-06-01"), ("Content-Type", "application/json")]), "Anthropic: x-api-key and the version");

        let mut custom = responses.clone();
        custom.custom_headers = h(&[("authorization", "Bearer mine"), ("Bad Name", "x"), ("X-Ok", "a\r\nb"), ("X-Team", "t")]);
        let got = with_build(&custom, "sk-a", headers);
        assert!(got == h(&[("Content-Type", "application/json"), ("authorization", "Bearer mine"), ("X-Team", "t")]), "custom headers replace case-insensitively; invalid ones are skipped");
        let seen = Log::default();
        let req = with_build(&custom, "sk-a", |b| request(b, &TestDialect { seen, fail_body: false })).expect("the request builds");
        assert_eq!((req.method, req.url.as_str(), req.body.as_slice(), req.stream), ("POST", "https://api.openai.com/v1/responses", br#"{"n":1,"stream":false}"#.as_slice(), false));
        let text = format!("{req:?}");
        for secret in ["sk-a", "Bearer mine", "application/json"] {
            assert!(!text.contains(secret), "the request's Debug shows no header value");
        }
    }

    #[test]
    fn key_policy_on_the_wire() {
        let private = fixture("m", "http://192.168.1.5:8000/v1", Api::ChatCompletions);
        let (mut t, _) = turn_of(input(private, false));
        let out = t.start(None);
        let [Out::WillSend, Out::Send(req)] = &out[..] else { panic!("{out:?}") };
        assert!(req.headers.contains(&("Authorization".to_string(), format!("Bearer {SELF_HOSTED_PLACEHOLDER}"))), "the placeholder, never NeedKey");

        let (mut t, _) = turn_of(input(openai(), false));
        assert_eq!(t.start(None), [Out::NeedKey(Vendor::OpenAI)]);
        assert_eq!(t.key_given(None), [failed(NO_KEY)]);
        assert!(t.is_idle());
        assert_eq!(t.start(Some(&key("sk-a"))), [], "a turn starts once");

        let (mut t, _) = turn_of(input(openai(), false));
        assert_eq!(t.start(None), [Out::NeedKey(Vendor::OpenAI)]);
        assert_eq!(t.key_given(Some(key(" \n "))), [failed(NO_KEY)], "whitespace is no key");

        let (mut t, _) = turn_of(input(openai(), false));
        assert_eq!(t.start(None), [Out::NeedKey(Vendor::OpenAI)]);
        let out = t.key_given(Some(key("sk-b")));
        let [Out::WillSend, Out::Send(req)] = &out[..] else { panic!("{out:?}") };
        assert!(req.headers.contains(&("Authorization".to_string(), "Bearer sk-b".to_string())), "the given key is sent");

        let (mut t, _) = turn_of(input(openai(), false));
        let out = t.start(Some(&key("sk-a")));
        let [Out::WillSend, Out::Send(req)] = &out[..] else { panic!("{out:?}") };
        assert!(req.headers.contains(&("Authorization".to_string(), "Bearer sk-a".to_string())), "the Bearer header");
        assert_eq!(t.start(Some(&key("sk-a"))), [], "a turn starts once");
    }

    #[test]
    fn review_key_contacts_nothing() {
        for model in [openai(), fixture("m", "http://192.168.1.5:8000/v1", Api::ChatCompletions)] {
            let (t, _) = turn_of(input(model, true));
            let (mut r, rec) = runner(t, vec![]);
            assert_eq!(r.start(Some(&key(REVIEW_KEY))), [Out::Choice(REVIEW_RESPONSE.to_string())]);
            assert!(rec.requests().is_empty(), "nothing is contacted");
            assert!(r.turn().is_idle());
        }
        let (mut t, _) = turn_of(input(openai(), false));
        assert!(matches!(&t.start(Some(&key("placeholder ")))[..], [Out::WillSend, Out::Send(_)]), "only the exact key");
    }

    #[test]
    fn provider_errors_follow_iterm2() {
        let refused = br#"{"error":{"message":"invalid x-api-key"}}"#;
        let text = "Error from Anthropic: HTTP request failed with status 401. invalid x-api-key";
        assert_eq!(answered(anthropic(), false, Reply::status(401, refused)), [failed(text)]);
        assert_eq!(answered(anthropic(), true, Reply::status(401, refused)), [delta(&format!("\n\n**Request failed**\n\n    {text}")), failed(text)]);
        assert_eq!(answered(anthropic(), false, Reply::status(404, br#"{"error":"model not found"}"#)), [failed("Error from Anthropic: HTTP request failed with status 404. model not found")]);
        assert_eq!(answered(anthropic(), false, Reply::status(400, br#"{"error":{"message":"m","code":1}}"#)), [failed("Error from Anthropic: HTTP request failed with status 400. m")]);
        assert_eq!(answered(anthropic(), false, Reply::status(500, b"oops")), [failed("Error from Anthropic: HTTP request failed with status 500.")]);
        assert_eq!(answered(openai(), false, Reply::failed("The request timed out.")), [failed("Error from OpenAI: HTTP request failed with The request timed out.")]);

        // A private host that got no credential: iTerm2's advice; its name is unknown.
        let private = fixture("m", "http://192.168.1.5:8000/v1", Api::ChatCompletions);
        let plain = "Error from Unknown Platform: HTTP request failed with status 401.";
        let advised = ai_policy::amended(plain, &crate::ai_transport::status_error(401), &private.url, private.api, &[]);
        assert!(advised.starts_with(&format!("{plain}\n\n")) && advised.len() > plain.len() + 2, "{advised}");
        assert_eq!(answered(private.clone(), false, Reply::status(401, b"")), [failed(&advised)]);
        let indented = advised.split('\n').map(|l| format!("    {l}")).collect::<Vec<_>>().join("\n");
        assert_eq!(answered(private, true, Reply::status(401, b"")), [delta(&request_failed(&indented)), failed(&advised)], "every line indented");

        assert_eq!(error_reason(br#"{"error":{"message":""}}"#).as_deref(), Some(""));
        assert_eq!(provider_error("X", "e", Some("")), "Error from X: e");
        assert_eq!(error_reason(b"[1]"), None);
        assert_eq!(transport_error("x"), "HTTP request failed with x");
    }

    #[test]
    fn stream_accumulation_follows_iterm2() {
        let events = [text("He"), text("llo"), call_event("f", "{}", Some("c1")), END.to_string()];
        let per_event: Vec<String> = events.iter().map(|e| sse(&[e])).collect();
        let (mut t, _) = sent(openai(), true);
        assert_eq!(feed(&mut t, &per_event, None), [delta("He"), delta("llo"), invoke("f", "{}", Some("c1"))]);
        let (mut t, _) = sent(openai(), true);
        let refs: Vec<&str> = events.iter().map(String::as_str).collect();
        assert_eq!(feed(&mut t, &[sse(&refs)], None), [delta("Hello"), invoke("f", "{}", Some("c1"))], "one chunk: text drains when the call arrives");

        let (mut t, _) = sent(openai(), true);
        assert_eq!(feed(&mut t, &[sse(&[&text("He")]), sse(&[&text("llo")]), sse(&[END])], None), [delta("He"), delta("llo"), Out::StreamEnd]);
        assert!(t.is_idle());

        let (mut t, _) = sent(openai(), true);
        let chunks = [format!("{}data: [DONE]\n\n{}", sse(&[&text("a")]), sse(&[&text("b")])), sse(&[&text("c")])];
        assert_eq!(feed(&mut t, &chunks, None), [delta("a"), Out::StreamEnd], "[DONE] stops parsing");

        let (mut t, _) = sent(openai(), true);
        let chunk = format!("{}data: {{bad\n\n{}", sse(&[&text("a")]), sse(&[&text("b")]));
        assert_eq!(feed(&mut t, &[chunk], None), [delta("a"), delta("b"), Out::StreamEnd], "a bad event drains, and parsing continues");

        // Usage counts even on an ignored event; the response id only on a used one (iTerm2).
        let (mut t, _) = sent(openai(), true);
        let events = [r#"{"text":"x","tokens":5,"id":"r1"}"#, r#"{"text":"","tokens":9,"id":"r2","ignore":true}"#];
        assert_eq!(feed(&mut t, &[sse(&events)], None), [delta("x"), Out::StreamEnd]);
        assert_eq!((t.prompt_tokens(), t.previous_response_id()), (Some(9), Some("r1")));
    }

    #[test]
    fn end_of_stream_reparse_follows_iterm2() {
        // The race iTerm2 repairs: the body holds a fragment the chunks never delivered.
        let (t, _) = turn_of(input(openai(), true));
        let first = sse(&[&call_event("f", "{\"a\"", Some("c1"))]);
        let late = sse(&[r#"{"call":{"arguments":":1}"}}"#]);
        let (mut r, _) = runner(t, vec![Reply::raced(&[first.as_bytes()], late.as_bytes())]);
        assert_eq!(r.start(Some(&key("sk-a"))), [Out::WillSend]);
        assert_eq!(r.poll(), [invoke("f", "{\"a\":1}", Some("c1"))], "the body's longer call");

        let (mut t, _) = sent(openai(), true);
        let live = sse(&[&call_event("f", "{\"b\":2}", Some("c1"))]);
        assert_eq!(feed(&mut t, &[live], Some(sse(&[&call_event("f", "{\"a\":1}", Some("c1"))]))), [invoke("f", "{\"b\":2}", Some("c1"))], "as long: the live call");

        let (t, _) = turn_of(input(openai(), true));
        let hi = sse(&[&text("Hi")]);
        let call = sse(&[&call_event("f", "{}", Some("c2"))]);
        let (mut r, _) = runner(t, vec![Reply::raced(&[hi.as_bytes()], call.as_bytes())]);
        r.start(Some(&key("sk-a")));
        assert_eq!(r.poll(), [delta("Hi"), invoke("f", "{}", Some("c2"))], "the live stream saw no call");

        let (t, _) = turn_of(input(openai(), true));
        let (hel, lo) = (sse(&[&text("Hel")]), sse(&[&text("lo")]));
        let (mut r, _) = runner(t, vec![Reply::raced(&[hel.as_bytes()], lo.as_bytes())]);
        r.start(Some(&key("sk-a")));
        assert_eq!(r.poll(), [delta("Hel"), delta("lo"), Out::StreamEnd], "the untransmitted tail");
    }

    /// Everything after `WillSend` for `body` delivered in `chunks`.
    fn delivered(chunks: &[&[u8]]) -> Vec<Out> {
        let (t, _) = turn_of(input(openai(), true));
        let (mut r, _) = runner(t, vec![Reply::ok(chunks)]);
        r.start(Some(&key("sk-a")));
        r.poll()
    }

    #[test]
    fn any_chunking_parses_the_same() {
        let event = |e: &str| format!("event: message\r\ndata: {e}\r\n\r\n");
        let with_call = [
            event(&text("Hel")),
            ": keep-alive\r\n\r\n".to_string(),
            event(&text("lo, caf\u{e9} \u{1F600}")),
            event(r#"{"status":"s1"}"#),
            event(r#"{"status":"s2"}"#),
            event(&call_event("f", "{\"a\":1}", Some("c1"))),
        ]
        .concat();
        let text_only = [
            event(r#"{"text":"Hel","reasoning":"r1"}"#),
            ": keep-alive\r\n\r\n".to_string(),
            event(&text("lo, caf\u{e9} \u{1F600}")),
            event(r#"{"status":"s1"}"#),
            event(r#"{"text":"!","reasoning":"r2"}"#),
            "data: [DONE]\r\n\r\n".to_string(),
        ]
        .concat();
        for (body, expected) in [
            (with_call, vec![invoke("f", "{\"a\":1}", Some("c1"))]),
            (text_only, vec![Out::Reasoning("r1r2".into()), Out::StreamEnd]),
        ] {
            let bytes = body.as_bytes();
            let whole = delivered(&[bytes]);
            let rest: Vec<Out> = whole.iter().filter(|o| !matches!(o, Out::Delta(_) | Out::Attachment(_))).cloned().collect();
            assert_eq!(rest, expected);
            let streamed: String = whole.iter().filter_map(|o| if let Out::Delta(s) = o { Some(s.as_str()) } else { None }).collect();
            assert!(streamed.starts_with("Hello, caf\u{e9} \u{1F600}"), "{streamed}");
            for i in 0..=bytes.len() {
                let split = delivered(&[&bytes[..i], &bytes[i..]]);
                assert!(same_outcome(&split, &whole), "split at {i}: {split:?}");
            }
            for i in 0..64 {
                for j in i..64 {
                    let split = delivered(&[&bytes[..i], &bytes[i..j], &bytes[j..]]);
                    assert!(same_outcome(&split, &whole), "split at {i} and {j}: {split:?}");
                }
            }
        }
        assert!(!same_outcome(&[delta("a")], &[delta("b")]));
        assert!(!same_outcome(&[Out::StreamEnd], &[]));
    }

    /// The last request's messages.
    fn last(seen: &Log) -> Seen {
        seen.lock().unwrap().last().cloned().expect("a request was built")
    }

    fn call_message(name: &str, arguments: &str, id: Option<&str>) -> Message {
        Message::new(
            Some(Role::Assistant),
            Body::FunctionCall(
                FunctionCall { name: Some(name.into()), arguments: Some(arguments.into()), id: id.map(str::to_string), thought_signature: None },
                id.map(|i| CallId { call_id: i.into(), item_id: i.into() }),
            ),
        )
    }

    /// Streams a call to `name` and checks the turn asks for it.
    fn ask(t: &mut Turn, name: &str, id: Option<&str>) -> Vec<Out> {
        feed(t, &[sse(&[&call_event(name, "{}", id)])], None)
    }

    #[test]
    fn tool_loop_follows_iterm2() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let mut i = input(openai(), true);
        i.volatile = Box::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            Some("v".into())
        });
        let (mut t, seen) = turn_of(i);
        assert!(matches!(&t.start(Some(&key("sk-a")))[..], [Out::WillSend, Out::Send(_)]));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let id = CallId { call_id: "c1".into(), item_id: "c1".into() };
        let out = feed(&mut t, &[sse(&[&call_event("f", "{\"x\":1}", Some("c1"))])], None);
        assert_eq!(out, [invoke("f", "{\"x\":1}", Some("c1"))]);
        let [Out::Invoke { id: got, .. }] = &out[..] else { panic!() };
        assert_eq!(got.as_ref(), Some(&id), "ruling 12c-1: the call's CallId");

        let out = t.tool_done(ToolResult::Ok("42".into()));
        let [Out::WillSend, Out::Send(req)] = &out[..] else { panic!("{out:?}") };
        assert!(req.stream, "the same stream flag");
        let s = last(&seen);
        assert_eq!(s.messages, [Message::text(Role::User, "hi"), call_message("f", "{\"x\":1}", Some("c1")), Message::new(Some(Role::Function), Body::FunctionOutput { name: "f".into(), output: "42".into(), id: Some(id.clone()) })]);
        assert_eq!((s.volatile.as_deref(), calls.load(Ordering::SeqCst)), (Some("v"), 2), "the volatile text is asked for once per request");

        // Five failures in a row each re-send; the sixth ends the turn and forgets the
        // response id.
        let mut t = sent_after(openai(), true, "resp_0");
        for n in 1..=5 {
            assert_eq!(ask(&mut t, "f", Some("c1")), [invoke("f", "{}", Some("c1"))]);
            assert!(matches!(&t.tool_done(ToolResult::Failed("boom".into()))[..], [Out::WillSend, Out::Send(_)]), "failure {n} re-sends");
        }
        assert_eq!(t.previous_response_id(), Some("resp_0"));
        assert_eq!(ask(&mut t, "f", Some("c1")), [invoke("f", "{}", Some("c1"))]);
        assert_eq!(t.tool_done(ToolResult::Failed("boom".into())), [delta(&request_failed("    boom")), failed("boom")]);
        assert!(t.is_idle());
        assert_eq!(t.previous_response_id(), None);

        // The failure text goes back to the model; a success resets the count.
        let (mut t, seen) = sent(openai(), false);
        let ask_whole = |t: &mut Turn, name: &str, id: Option<&str>| t.http(HttpEvent::Done { body: call_event(name, "{}", id), error: None });
        for _ in 0..5 {
            ask_whole(&mut t, "f", Some("c1"));
            t.tool_done(ToolResult::Failed("boom".into()));
        }
        assert_eq!(last(&seen).messages.last(), Some(&Message::new(Some(Role::Function), Body::FunctionOutput { name: "f".into(), output: "Tool call failed: boom".into(), id: Some(id.clone()) })));
        assert!(!last(&seen).stream);
        ask_whole(&mut t, "f", Some("c1"));
        assert!(matches!(&t.tool_done(ToolResult::Ok("ok".into()))[..], [Out::WillSend, Out::Send(_)]));
        for _ in 0..5 {
            ask_whole(&mut t, "f", Some("c1"));
            assert!(matches!(&t.tool_done(ToolResult::Failed("boom".into()))[..], [Out::WillSend, Out::Send(_)]), "counted from the success");
        }

        // An unknown name is answered as a tool output when the call has an id, else as the
        // user, and shares the bound.
        let (mut t, seen) = sent(openai(), true);
        assert!(matches!(&ask(&mut t, "nope", Some("c9"))[..], [Out::WillSend, Out::Send(_)]));
        assert_eq!(last(&seen).messages.last(), Some(&Message::new(Some(Role::Function), Body::FunctionOutput { name: "nope".into(), output: UNKNOWN_FUNCTION.into(), id: Some(CallId { call_id: "c9".into(), item_id: "c9".into() }) })));
        assert!(matches!(&ask(&mut t, "nope", None)[..], [Out::WillSend, Out::Send(_)]));
        assert_eq!(last(&seen).messages.last(), Some(&Message::text(Role::User, UNKNOWN_FUNCTION)));
        for _ in 0..2 {
            ask(&mut t, "f", Some("c1"));
            t.tool_done(ToolResult::Failed("boom".into()));
        }
        assert!(matches!(&ask(&mut t, "nope", None)[..], [Out::WillSend, Out::Send(_)]), "the fifth");
        assert_eq!(ask(&mut t, "nope", None), [delta(&request_failed(&format!("    {UNKNOWN_FUNCTION}"))), failed(UNKNOWN_FUNCTION)], "the sixth");

        // Canceled ends at once, unstreamed, resets the count and forgets the response id.
        let mut t = sent_after(openai(), true, "resp_0");
        for _ in 0..5 {
            ask(&mut t, "f", Some("c1"));
            t.tool_done(ToolResult::Failed("boom".into()));
        }
        ask(&mut t, "f", Some("c1"));
        assert_eq!(t.tool_done(ToolResult::Canceled("Canceled".into())), [failed("Canceled")]);
        assert_eq!(t.previous_response_id(), None);

        // The continuation is truncated to the context limit.
        let (mut t, seen) = sent(openai(), false);
        t.http(HttpEvent::Done { body: call_event("f", "{}", Some("c1")), error: None });
        let big = format!("{}{}", "a".repeat(150000), "b".repeat(150000));
        t.tool_done(ToolResult::Ok(big));
        let s = last(&seen);
        assert_eq!(context_limit(128000, 8192), 119808);
        assert_eq!(s.messages.len(), 1, "older messages dropped first, as iTerm2's");
        assert_eq!(s.messages[0].content().unwrap(), format!("{}{TRUNCATED}{}", "a".repeat(75000), "b".repeat(75000)));
    }

    #[test]
    fn cancel_follows_iterm2() {
        let (mut t, _) = sent(openai(), true);
        assert_eq!(t.cancel(), [Out::Canceled]);
        assert_eq!(t.http(HttpEvent::Chunk(sse(&[&text("x")]))), []);
        assert_eq!(t.http(HttpEvent::Done { body: String::new(), error: Some("HTTP request failed with status 500.".into()) }), []);
        assert!(t.is_idle());
        assert_eq!(t.cancel(), [], "idle: nothing");

        let (mut t, _) = sent(openai(), true);
        ask(&mut t, "f", Some("c1"));
        assert_eq!(t.cancel(), [Out::Canceled]);
        assert_eq!(t.tool_done(ToolResult::Ok("late".into())), [], "a tool that ends after the cancel is ignored");

        let (mut t, _) = turn_of(input(openai(), true));
        assert_eq!(t.cancel(), []);
        assert_eq!(t.start(None), [Out::NeedKey(Vendor::OpenAI)]);
        assert_eq!(t.cancel(), [Out::Canceled]);
        assert_eq!(t.key_given(Some(key("sk-a"))), []);

        // An answer that comes at the wrong time leaves the request alone.
        let (mut t, _) = sent(openai(), true);
        assert_eq!(t.tool_done(ToolResult::Ok("x".into())), []);
        assert_eq!(t.key_given(Some(key("sk-b"))), []);
        assert_eq!(feed(&mut t, &[sse(&[&text("a")])], None), [delta("a"), Out::StreamEnd], "still streaming");
    }

    #[test]
    fn send_rules_follow_iterm2() {
        let mut i = input(fixture("m", "about:empty", Api::ChatCompletions), false);
        i.legacy_url_pref = "x".into();
        let (mut t, _) = turn_of(i);
        assert_eq!(t.start(Some(&key("sk-a"))), [failed("Invalid URL for AI provider of x")]);
        let (mut t, _) = turn_of(input(fixture("apple", "", Api::AppleIntelligence), false));
        assert_eq!(t.start(None), [failed(APPLE_UNAVAILABLE)], "ruling 7");
        let mut t = Turn::with_dialect(input(openai(), false), Box::new(TestDialect { seen: Log::default(), fail_body: true }));
        assert_eq!(t.start(Some(&key("sk-a"))), [failed(TOKEN_LIMIT)]);
        assert!(t.is_idle());

        // What reaches the builder.
        let responses = || {
            let mut m = fixture("gpt", "https://api.openai.com/v1/responses", Api::Responses);
            m.reasoning_efforts = vec![Effort::Low, Effort::High];
            m.service_tiers = vec![ServiceTier::Flex];
            m
        };
        let seen_with = |m: Model, zdr: bool, effort: Effort, tier: ServiceTier| {
            let mut i = input(m, false);
            i.previous_response_id = Some("r0".into());
            i.should_think = Some(true);
            i.effort = Some(effort);
            i.tier = Some(tier);
            i.wire.zdr = zdr;
            let (mut t, seen) = turn_of(i);
            t.start(Some(&key("sk-a")));
            last(&seen)
        };
        let s = seen_with(responses(), false, Effort::High, ServiceTier::Flex);
        assert_eq!((s.previous.as_deref(), s.think, s.effort, s.tier), (Some("r0"), None, Some(Effort::High), Some(ServiceTier::Flex)));
        let s = seen_with(responses(), true, Effort::Medium, ServiceTier::Priority);
        assert_eq!((s.previous, s.effort, s.tier), (None, None, None), "not under ZDR; unlisted effort and tier");
        assert_eq!(seen_with(openai(), false, Effort::High, ServiceTier::Flex).previous, None, "Responses only");
        let mut thinking = responses();
        thinking.features.insert(Feature::ConfigurableThinking);
        assert_eq!(seen_with(thinking, false, Effort::High, ServiceTier::Flex).think, Some(true));
        let mut no_tools = openai();
        no_tools.features.remove(&Feature::FunctionCalling);
        let (mut t, seen) = turn_of(input(no_tools, false));
        t.start(Some(&key("sk-a")));
        assert_eq!(last(&seen).tools, 0, "tools only with function calling");

        // A whole answer.
        let whole = |body: &str| {
            let (mut t, _) = sent(openai(), false);
            t.http(HttpEvent::Done { body: body.to_string(), error: None })
        };
        assert_eq!(whole(r#"{"text":"  hi"}"#), [Out::Choice("hi".into())]);
        assert_eq!(whole(r#"{"text":"hi","reasoning":"r"}"#), [Out::Reasoning("r".into()), Out::Choice("hi".into())]);
        assert_eq!(whole(END), [failed(UNEXPECTED_EOF)]);
        assert_eq!(whole(r#"{"empty":true}"#), [failed(EMPTY_RESPONSE)]);
        assert_eq!(whole(r#"{"error":{"message":"bad"}}"#), [failed("Could not decode response: bad")]);
        let out = whole("nonsense");
        let [Out::Failed(text)] = &out[..] else { panic!("{out:?}") };
        assert!(text.starts_with("Failed to decode API response: ") && text.ends_with(". Data is: nonsense"), "{text}");
        let (mut t, _) = sent(openai(), false);
        t.http(HttpEvent::Done { body: r#"{"text":"x","id":"r9","tokens":12}"#.into(), error: None });
        assert_eq!((t.previous_response_id(), t.prompt_tokens()), (Some("r9"), Some(12)));
        assert_eq!(t.http(HttpEvent::Chunk("x".into())), [], "idle");
    }

    #[test]
    fn runner_follows_the_turn() {
        let (t, seen) = turn_of(input(openai(), false));
        let (mut r, rec) = runner(t, vec![Reply::ok(&[call_event("f", "{}", Some("c1")).as_bytes()]), Reply::ok(&[text("done").as_bytes()])]);
        assert_eq!(r.start(Some(&key("sk-a"))), [Out::WillSend]);
        assert_eq!(r.poll(), [invoke("f", "{}", Some("c1"))]);
        assert_eq!(r.poll(), []);
        assert_eq!(r.tool_done(ToolResult::Ok("42".into())), [Out::WillSend]);
        assert_eq!(r.poll(), [Out::Choice("done".into())]);
        let requests = rec.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].body, br#"{"n":3,"stream":false}"#);
        assert_eq!(last(&seen).messages.len(), 3);
        assert!(r.turn().is_idle());

        let (t, _) = turn_of(input(openai(), true));
        let (a, b) = (sse(&[&text("a")]), sse(&[&text("b")]));
        let (mut r, _) = runner(t, vec![Reply::ok(&[a.as_bytes(), b.as_bytes()])]);
        assert_eq!(r.start(Some(&key("sk-a"))), [Out::WillSend]);
        assert_eq!(r.cancel(), [Out::Canceled]);
        assert_eq!(r.poll(), [], "the live call was cancelled");
        assert!(r.turn().is_idle());
    }

    /// The Decision "only the first of two streamed calls runs": a call that another part
    /// follows is dispatched once, mid-stream, and the rest of the answer is dropped with its
    /// HTTP call cancelled; in one chunk or one event per chunk.
    #[test]
    fn mid_stream_call_dispatches_once() {
        let status = json!({"status": "s"}).to_string();
        for second in [call_event("f", "{\"x\":1}", Some("c2")), status] {
            let events = [call_event("f", "{}", Some("c1")), second, text("after"), END.to_string()];
            let whole = sse(&events.iter().map(String::as_str).collect::<Vec<_>>());
            let each: Vec<String> = events.iter().map(|e| sse(&[e])).collect();
            for chunks in [vec![whole], each] {
                let (t, _) = turn_of(input(openai(), true));
                let (mut r, rec) = runner(t, vec![Reply::ok(&chunks.iter().map(String::as_bytes).collect::<Vec<_>>())]);
                assert_eq!(r.start(Some(&key("sk-a"))), [Out::WillSend]);
                assert_eq!(r.poll(), [invoke("f", "{}", Some("c1"))], "{chunks:?}");
                assert_eq!(r.poll(), [], "nothing after the dispatch, and no StreamEnd");
                assert_eq!(rec.cancels(), 1, "the stream was cancelled");
                assert_eq!(rec.requests().len(), 1);
                assert_eq!(r.tool_done(ToolResult::Canceled("stopped".into())), [Out::Failed("stopped".into())], "the call was waiting for its tool");
            }
        }

        // An unknown name: the model is told so once, the first stream stays cancelled, and the
        // new request is read in the same poll.
        // (Text would merge into the call as its arguments, as Anthropic streams them.)
        let first = sse(&[&call_event("nope", "{}", Some("c1")), &json!({"status": "s"}).to_string(), &text("after")]);
        let (t, seen) = turn_of(input(openai(), true));
        let (mut r, rec) = runner(t, vec![Reply::ok(&[first.as_bytes()]), Reply::ok(&[sse(&[&text("ok")]).as_bytes()])]);
        assert_eq!(r.start(Some(&key("sk-a"))), [Out::WillSend]);
        assert_eq!(r.poll(), [Out::WillSend, delta("ok"), Out::StreamEnd]);
        assert_eq!(r.poll(), []);
        assert_eq!((rec.cancels(), rec.requests().len()), (1, 2));
        assert_eq!(last(&seen).messages.last().and_then(Message::content).as_deref(), Some(UNKNOWN_FUNCTION));
        assert!(r.turn().is_idle());
    }
}

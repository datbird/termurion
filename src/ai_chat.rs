//! OpenAI's Chat Completions API and the two older wire formats beside it: the request bodies
//! (iTerm2 does not sort these keys), the `CompletionsMessage` conversion every chat-shaped
//! dialect shares (Ollama's native shapes included), and the parsers of whole and streamed
//! answers. Pure; ports `AITerm/CompletionsOpenAI.swift` (`CO:8-67`), `LLMModernProtocol.swift`
//! (`MP:9-675`), `O1OpenAI.swift` (`O1:9-46`) and `LegacyOpenAI.swift` (`LO:18-146`).

use crate::ai_json::splice_at_key;
use crate::ai_model::{dynamic_models_supported, mime_is_textual};
use crate::ai_msg::{Attachment, Body, CallId, FunctionCall, Message, Role};
use crate::ai_sse;
use crate::ai_turn::{max_tokens_for, Build, Dialect, Parsed, TOKEN_LIMIT};
use base64::Engine;
use serde_json::{json, Map, Value};

/// Chat Completions (`/v1/chat/completions` and every server that copies it).
pub struct ChatCompletions;
/// The first o1 models: no system role, no tools, no streaming options.
pub struct EarlyO1;
/// The legacy Completions API (`/v1/completions`): one prompt string.
pub struct Completions;

fn base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Strict base64, as Swift's `Data(base64Encoded:)`; empty when it does not decode.
fn unbase64(s: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(s).unwrap_or_default()
}

/// "A file named <name> (content unavailable)", a file id sent as text.
pub fn unavailable(name: &str) -> String {
    format!("A file named {name} (content unavailable)")
}

/// A file sent as text inside iTerm2's wrapper (the wire string keeps its name).
pub fn attachment_text(name: &str, mime: &str, content: &[u8]) -> String {
    format!("<iterm2:attachment file=\"{name}\" type=\"{mime}\">\n{}\n</iterm2:attachment>", String::from_utf8_lossy(content))
}

fn text_part(text: &str) -> Value {
    json!({"type": "text", "text": text})
}

/// `openAIAudioFormat` (`MP:508-517`): the two formats `input_audio` takes.
fn audio_format(mime: &str) -> Option<&'static str> {
    match mime {
        "audio/wav" | "audio/x-wav" => Some("wav"),
        "audio/mpeg" | "audio/mp3" => Some("mp3"),
        _ => None,
    }
}

/// `contentPart(forFile:)` (`MP:481-504`): an image as `image_url`, WAV or MP3 as
/// `input_audio`, a PDF as `file`; text (SVG included) and anything else in the text wrapper.
pub fn content_part(name: &str, mime: &str, content: &[u8]) -> Value {
    if !mime_is_textual(mime) {
        if mime.starts_with("image/") {
            return json!({"type": "image_url", "image_url": {"url": format!("data:{mime};base64,{}", base64(content))}});
        }
        if let Some(format) = audio_format(mime) {
            return json!({"type": "input_audio", "input_audio": {"data": base64(content), "format": format}});
        }
        if mime == "application/pdf" {
            return json!({"type": "file", "file": {"file_data": format!("data:{mime};base64,{}", base64(content)), "filename": name}});
        }
    }
    text_part(&attachment_text(name, mime, content))
}

fn role_name(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Function => "function",
    }
}

/// `CompletionsMessage.Content`: a string or content parts.
enum Content {
    Text(String),
    Parts(Vec<Value>),
}

impl Content {
    fn value(&self) -> Value {
        match self {
            Content::Text(s) => Value::String(s.clone()),
            Content::Parts(parts) => Value::Array(parts.clone()),
        }
    }
}

/// iTerm2's `CompletionsMessage` as `init?(_ llmMessage:)` fills it (`MP:277-371`).
struct Chat {
    role: Option<Role>,
    content: Option<Content>,
    /// The `name` key: a legacy call's or tool output's function name.
    name: Option<String>,
    function_call: Option<FunctionCall>,
    /// The one `tool_calls` entry: its id and call.
    tool_call: Option<(String, FunctionCall)>,
    tool_call_id: Option<String>,
}

impl Chat {
    fn of(m: &Message) -> Option<Chat> {
        let mut c = Chat { role: m.role, content: None, name: None, function_call: None, tool_call: None, tool_call_id: None };
        match &m.body {
            Body::Uninitialized => return None,
            Body::Text(text) => c.content = Some(Content::Text(text.clone())),
            Body::Attachment(a) => {
                c.content = Some(match a {
                    Attachment::Code(code) => Content::Text(code.clone()),
                    Attachment::Status { status, .. } => Content::Text(status.display()),
                    Attachment::File { name, mime, content } if !mime_is_textual(mime) && (mime.starts_with("image/") || audio_format(mime).is_some()) => {
                        Content::Parts(vec![content_part(name, mime, content)])
                    }
                    Attachment::File { content, .. } => Content::Text(String::from_utf8_lossy(content).into_owned()),
                    Attachment::FileId { name, .. } => Content::Text(unavailable(name)),
                })
            }
            Body::FunctionCall(call, id) => c.set_call(call, id.as_ref()),
            Body::FunctionOutput { name, output, id } => {
                match id {
                    Some(id) => c.tool_call_id = Some(id.call_id.clone()),
                    None => c.name = Some(name.clone()),
                }
                c.content = Some(Content::Text(output.clone()));
            }
            Body::Multipart(parts) => {
                let mut content = Vec::new();
                for part in parts {
                    match part {
                        Body::Text(text) => content.push(text_part(text)),
                        // Only the first call goes (the others are not expected).
                        Body::FunctionCall(call, id) if c.tool_call.is_none() && c.function_call.is_none() => c.set_call(call, id.as_ref()),
                        Body::Attachment(Attachment::Code(code)) => content.push(text_part(code)),
                        Body::Attachment(Attachment::File { name, mime, content: bytes }) => content.push(content_part(name, mime, bytes)),
                        Body::Attachment(Attachment::FileId { name, .. }) => content.push(text_part(&unavailable(name))),
                        // Statuses are not sent; outputs and nested parts are not expected.
                        _ => {}
                    }
                }
                c.content = (!content.is_empty()).then_some(Content::Parts(content));
            }
        }
        Some(c)
    }

    /// Modern servers take `tool_calls` with an id; without one, the legacy `function_call`.
    fn set_call(&mut self, call: &FunctionCall, id: Option<&CallId>) {
        match id.map(|i| i.call_id.clone()).or_else(|| call.id.clone()) {
            Some(id) => self.tool_call = Some((id, call.clone())),
            None => {
                self.name = call.name.clone();
                self.function_call = Some(call.clone());
            }
        }
    }

    /// `encode(to:)` (`MP:93-149`): `role` and `content` always (null when absent).
    fn encode(&self, ollama: bool) -> Value {
        if ollama && let Some(v) = self.ollama_tool_message().or_else(|| self.ollama_image_message()) {
            return v;
        }
        let mut v = Map::new();
        let role = match (&self.tool_call_id, self.role) {
            (Some(_), _) => json!("tool"),
            (None, Some(role)) => json!(role_name(role)),
            (None, None) => Value::Null,
        };
        v.insert("role".into(), role);
        if let (None, Some(name)) = (&self.tool_call_id, &self.name) {
            v.insert("name".into(), json!(name));
        }
        let content = if ollama { self.ollama_scalar_content() } else { self.content.as_ref().map(Content::value) };
        v.insert("content".into(), content.unwrap_or(Value::Null));
        if let Some(call) = &self.function_call {
            v.insert("function_call".into(), function_json(call));
        }
        if let Some((id, call)) = &self.tool_call {
            v.insert("tool_calls".into(), json!([{"id": id, "type": "function", "function": function_json(call)}]));
        }
        if let Some(id) = &self.tool_call_id {
            v.insert("tool_call_id".into(), json!(id));
        }
        Value::Object(v)
    }

    /// `encodeOllamaToolMessage` (`MP:154-178`): a tool result names its tool; a call without
    /// an id goes as `tool_calls` with its arguments as an object.
    fn ollama_tool_message(&self) -> Option<Value> {
        if self.function_call.is_none()
            && self.tool_call.is_none()
            && let (Some(name), Some(content)) = (&self.name, &self.content)
        {
            return Some(json!({"role": "tool", "tool_name": name, "content": content.value()}));
        }
        let call = self.function_call.as_ref().filter(|_| self.tool_call.is_none())?;
        let arguments = call.arguments.as_deref().and_then(|a| serde_json::from_str::<Map<String, Value>>(a).ok()).unwrap_or_default();
        let mut function = Map::new();
        if let Some(name) = &call.name {
            function.insert("name".into(), json!(name));
        }
        function.insert("arguments".into(), Value::Object(arguments));
        let mut v = Map::new();
        v.insert("role".into(), json!("assistant"));
        v.insert("tool_calls".into(), json!([{"type": "function", "function": function}]));
        if let Some(content) = self.ollama_scalar_content() {
            v.insert("content".into(), content);
        }
        Some(Value::Object(v))
    }

    /// `encodeOllamaImageMessage` (`MP:231-249`): images go out of band as bare base64.
    fn ollama_image_message(&self) -> Option<Value> {
        let Some(Content::Parts(parts)) = &self.content else {
            return None;
        };
        let images: Vec<String> = parts.iter().filter_map(|p| p["image_url"]["url"].as_str().filter(|_| p["type"] == "image_url")).map(|url| data_url(url).map_or(url, |(_, payload)| payload).to_string()).collect();
        if images.is_empty() {
            return None;
        }
        let role = self.role.map_or(Value::Null, |r| json!(role_name(r)));
        Some(json!({"role": role, "content": joined_text(parts), "images": images}))
    }

    /// Ollama takes only a string `content`: parts become their joined text.
    fn ollama_scalar_content(&self) -> Option<Value> {
        match &self.content {
            Some(Content::Parts(parts)) => Some(json!(joined_text(parts))),
            other => other.as_ref().map(Content::value),
        }
    }
}

fn joined_text(parts: &[Value]) -> String {
    parts.iter().filter(|p| p["type"] == "text").filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n")
}

/// "data:<meta>,<payload>" split at the first comma (`MP:254-261`).
fn data_url(s: &str) -> Option<(&str, &str)> {
    let rest = s.strip_prefix("data:")?;
    rest.split_once(',')
}

/// `LLM.FunctionCall` as Swift encodes it: the fields that are present.
pub fn function_json(call: &FunctionCall) -> Value {
    let mut v = Map::new();
    for (key, field) in [("name", &call.name), ("arguments", &call.arguments), ("id", &call.id), ("thoughtSignature", &call.thought_signature)] {
        if let Some(s) = field {
            v.insert(key.into(), json!(s));
        }
    }
    Value::Object(v)
}

/// The message as Chat Completions takes it; None for a message with no body.
pub fn chat_message(m: &Message) -> Option<Value> {
    chat_message_for(m, false)
}

/// The message as Chat Completions takes it, or with `ollama` as Ollama's native `/api/chat`
/// takes it (tool messages, images and string content differ; `MP:96-106`).
pub fn chat_message_for(m: &Message, ollama: bool) -> Option<Value> {
    Chat::of(m).map(|c| c.encode(ollama))
}

/// The message's content as Ollama's builder counts it (`joinText`, `LL:430-457`): adjacent
/// text parts merged with "\n", a lone text part as a string; None when it has no content.
pub fn ollama_joined_content(m: &Message) -> Option<Value> {
    match Chat::of(m)?.content? {
        Content::Text(text) => Some(Value::String(text)),
        Content::Parts(parts) => {
            let mut combined: Vec<Value> = Vec::new();
            for part in parts {
                match combined.last_mut() {
                    Some(last) if last["type"] == "text" && part["type"] == "text" => {
                        *last = text_part(&format!("{}\n{}", last["text"].as_str().unwrap_or_default(), part["text"].as_str().unwrap_or_default()));
                    }
                    _ => combined.push(part),
                }
            }
            match combined.as_slice() {
                [only] if only["type"] == "text" => Some(only["text"].clone()),
                _ => Some(Value::Array(combined)),
            }
        }
    }
}

/// The messages a request sends: the volatile text, when there is some, as a last user message
/// (`messagesWithVolatile`, `RB:41-46`).
pub fn with_volatile(b: &Build) -> Vec<Message> {
    let mut messages = b.messages.to_vec();
    if let Some(v) = b.volatile.filter(|v| !v.is_empty()) {
        messages.push(Message::text(Role::User, v));
    }
    messages
}

/// How many system messages lead the list; the frozen history is spliced after them.
pub fn systems(messages: &[Message]) -> usize {
    messages.iter().filter(|m| m.role == Some(Role::System)).count()
}

/// A request that leaves the answer fewer than 2 tokens is refused (`CO:56-58`).
pub fn budget(max: i64) -> Result<i64, String> {
    if max < 2 {
        Err(TOKEN_LIMIT.to_string())
    } else {
        Ok(max)
    }
}

fn to_bytes(body: Map<String, Value>) -> Vec<u8> {
    serde_json::to_vec(&Value::Object(body)).expect("a JSON value always encodes")
}

impl Dialect for ChatCompletions {
    /// `ModernBodyRequestBuilder.body()` (`CO:36-67`), plus `parallel_tool_calls: false` with
    /// tools to OpenAI's own host (m12b ruling: Tandem runs only the first call of an answer;
    /// the field is OpenAI's, so every other host gets iTerm2's bytes, even one 12a files under
    /// the OpenAI vendor).
    fn body(&self, b: &Build) -> Result<Vec<u8>, String> {
        let messages = with_volatile(b);
        let max = budget(max_tokens_for(&Build { messages: &messages, ..*b }))?;
        let mut body = Map::new();
        if dynamic_models_supported(b.model) {
            body.insert("model".into(), json!(b.model.name));
        }
        body.insert("messages".into(), Value::Array(messages.iter().filter_map(chat_message).collect()));
        body.insert("max_tokens".into(), json!(max));
        if b.model.supports_temperature {
            body.insert("temperature".into(), json!(0));
        }
        if !b.tools.is_empty() {
            let tools: Vec<Value> = b.tools.iter().map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.parameters}})).collect();
            body.insert("tools".into(), Value::Array(tools));
            body.insert("tool_choice".into(), json!("auto"));
            if crate::ai_policy::host_is_openai(&b.model.url) {
                body.insert("parallel_tool_calls".into(), json!(false));
            }
        }
        body.insert("stream".into(), json!(b.stream));
        splice_at_key(&to_bytes(body), "messages", systems(&messages), b.frozen)
    }

    fn parse(&self, body: &[u8]) -> Result<Option<Parsed>, String> {
        parse_modern(body)
    }

    fn parse_event(&self, json: &str) -> Result<Option<Parsed>, String> {
        parse_modern_event(json)
    }
}

impl Dialect for EarlyO1 {
    /// `O1BodyRequestBuilder.body()` (`O1:21-46`): system messages become user messages in
    /// place; no tools, temperature or `stream`; the model is always named.
    fn body(&self, b: &Build) -> Result<Vec<u8>, String> {
        let messages = with_volatile(b);
        let max = budget(max_tokens_for(&Build { messages: &messages, tools: &[], ..*b }))?;
        let converted: Vec<Value> = messages
            .iter()
            .filter_map(|m| {
                let mut m = m.clone();
                if m.role == Some(Role::System) {
                    m.role = Some(Role::User);
                }
                chat_message(&m)
            })
            .collect();
        let mut body = Map::new();
        body.insert("model".into(), json!(b.model.name));
        body.insert("messages".into(), Value::Array(converted));
        body.insert("max_completion_tokens".into(), json!(max));
        splice_at_key(&to_bytes(body), "messages", systems(&messages), b.frozen)
    }

    fn parse(&self, body: &[u8]) -> Result<Option<Parsed>, String> {
        parse_modern(body)
    }

    fn parse_event(&self, json: &str) -> Result<Option<Parsed>, String> {
        parse_modern_event(json)
    }
}

impl Dialect for Completions {
    /// `LegacyBodyRequestBuilder.body()` (`LO:30-50`): every message's content joined into one
    /// prompt; no frozen history.
    fn body(&self, b: &Build) -> Result<Vec<u8>, String> {
        let messages = with_volatile(b);
        let max = budget(max_tokens_for(&Build { messages: &messages, tools: &[], ..*b }))?;
        let prompt = messages.iter().map(|m| m.content().unwrap_or_default()).collect::<Vec<_>>().join("\n");
        let mut body = Map::new();
        if dynamic_models_supported(b.model) {
            body.insert("model".into(), json!(b.model.name));
        }
        body.insert("prompt".into(), json!(prompt));
        body.insert("max_tokens".into(), json!(max));
        Ok(to_bytes(body))
    }

    /// `LegacyResponse` (`LO:52-94`): the first choice's text.
    fn parse(&self, body: &[u8]) -> Result<Option<Parsed>, String> {
        let v = object(body)?;
        let choices = v["choices"].as_array().ok_or_else(|| "the response has no choices".to_string())?;
        let messages = choices.first().map(|c| Message::text(Role::Assistant, c["text"].as_str().unwrap_or_default())).into_iter().collect();
        Ok(Some(Parsed { messages, response_id: None, prompt_tokens: v["usage"]["prompt_tokens"].as_i64(), ignore: false, error: None }))
    }

    fn split(&self, buffer: &str, at: &mut ai_sse::Cursor) -> Option<String> {
        ai_sse::split_lines(buffer, at)
    }

    /// `LegacyStreamingResponse` (`LO:95-118`), the `{model, created_at, response, done}` shape
    /// iTerm2 expects from a legacy model with streaming on (kept); `done` ends the stream.
    fn parse_event(&self, json: &str) -> Result<Option<Parsed>, String> {
        let v = object(json.as_bytes())?;
        if v["done"].as_bool() == Some(true) {
            return Ok(None);
        }
        let response = v["response"].as_str().ok_or_else(|| "the event has no response".to_string())?;
        Ok(Some(Parsed { messages: vec![Message::text(Role::Assistant, response)], ..Parsed::default() }))
    }
}

fn object(bytes: &[u8]) -> Result<Value, String> {
    let v: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if v.is_object() {
        Ok(v)
    } else {
        Err("the response is not an object".to_string())
    }
}

/// The first of a response's `choices`, which must be there (iTerm2 never asks for more).
fn first_choice(v: &Value) -> Result<Option<&Value>, String> {
    Ok(v["choices"].as_array().ok_or_else(|| "the response has no choices".to_string())?.first())
}

fn role_of(v: &Value) -> Result<Option<Role>, String> {
    match v {
        Value::Null => Ok(None),
        Value::String(s) => match s.as_str() {
            "system" => Ok(Some(Role::System)),
            "user" => Ok(Some(Role::User)),
            "assistant" => Ok(Some(Role::Assistant)),
            "function" => Ok(Some(Role::Function)),
            other => Err(format!("unknown role {other}")),
        },
        _ => Err("the role is not a string".to_string()),
    }
}

/// A decoded content part (`ContentPart`, `MP:711-800`).
enum Part {
    Text(String),
    File { data: String, filename: String },
    Image(String),
    Audio { data: String, format: String },
}

fn content_of(v: &Value) -> Result<Option<Result<String, Vec<Part>>>, String> {
    let s = |v: &Value| v.as_str().map(str::to_string);
    match v {
        Value::Null => Ok(None),
        Value::String(text) => Ok(Some(Ok(text.clone()))),
        Value::Array(parts) => {
            let parts = parts
                .iter()
                .map(|p| {
                    let part = match p["type"].as_str() {
                        Some("text") => s(&p["text"]).map(Part::Text),
                        Some("file") => s(&p["file"]["file_data"]).zip(s(&p["file"]["filename"])).map(|(data, filename)| Part::File { data, filename }),
                        Some("image_url") => s(&p["image_url"]["url"]).map(Part::Image),
                        Some("input_audio") => s(&p["input_audio"]["data"]).zip(s(&p["input_audio"]["format"])).map(|(data, format)| Part::Audio { data, format }),
                        _ => None,
                    };
                    part.ok_or_else(|| "unknown content part".to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Some(Err(parts)))
        }
        _ => Err("the content is neither text nor parts".to_string()),
    }
}

fn call_of(v: &Value) -> FunctionCall {
    let s = |k: &str| v[k].as_str().map(str::to_string);
    FunctionCall { name: s("name"), arguments: s("arguments"), id: s("id"), thought_signature: s("thoughtSignature") }
}

/// The first `tool_calls` entry, when it has a function: the call, and its id as both halves.
fn first_tool_call(message: &Value) -> Option<(FunctionCall, Option<CallId>)> {
    let entry = message["tool_calls"].as_array()?.first().filter(|t| t["function"].is_object())?;
    let id = entry["id"].as_str().map(|i| CallId { call_id: i.to_string(), item_id: i.to_string() });
    Some((call_of(&entry["function"]), id))
}

fn file(name: &str, mime: &str, content: Vec<u8>) -> Body {
    Body::Attachment(Attachment::File { name: name.to_string(), mime: mime.to_string(), content })
}

/// `decodeDataURL` (`MP:520-531`): its MIME and bytes; characters outside base64 are skipped.
fn decoded_data_url(url: &str) -> Option<(String, Vec<u8>)> {
    let (meta, payload) = data_url(url)?;
    let clean: String = payload.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=')).collect();
    let bytes = base64::engine::general_purpose::STANDARD.decode(clean).ok()?;
    let mime = meta.split(';').next().unwrap_or("application/octet-stream");
    Some((mime.to_string(), bytes))
}

/// `llmMessage` (`MP:372-452`): a named string is a tool's output; otherwise the text, then the
/// legacy call or the first tool call, one body or a multipart.
fn whole_message(v: &Value) -> Result<Message, String> {
    let role = role_of(&v["role"])?;
    let content = content_of(&v["content"])?;
    let reasoning = v["reasoning_content"].as_str().map(str::to_string);
    if let (Some(name), Some(Ok(output))) = (v["name"].as_str(), &content) {
        let mut m = Message::new(role, Body::FunctionOutput { name: name.to_string(), output: output.clone(), id: None });
        m.reasoning_content = reasoning;
        return Ok(m);
    }
    let mut bodies = Vec::new();
    match content {
        Some(Ok(text)) if !text.is_empty() => bodies.push(Body::Text(text)),
        Some(Ok(_)) | None => {}
        Some(Err(parts)) => {
            let mut parts: Vec<Body> = parts
                .into_iter()
                .map(|p| match p {
                    Part::Text(t) => Body::Text(t),
                    Part::File { data, filename } => file(&filename, "application/octet-stream", unbase64(&data)),
                    Part::Image(url) => match decoded_data_url(&url) {
                        Some((mime, bytes)) => file("image", &mime, bytes),
                        None => Body::Text(url),
                    },
                    Part::Audio { data, format } => file("audio", if format == "wav" { "audio/wav" } else { "audio/mpeg" }, unbase64(&data)),
                })
                .collect();
            match parts.len() {
                0 => {}
                1 => bodies.push(parts.remove(0)),
                _ => bodies.push(Body::Multipart(parts)),
            }
        }
    }
    if v["function_call"].is_object() {
        bodies.push(Body::FunctionCall(call_of(&v["function_call"]), None));
    } else if let Some((call, id)) = first_tool_call(v) {
        bodies.push(Body::FunctionCall(call, id));
    }
    let body = match bodies.len() {
        0 => Body::Uninitialized,
        1 => bodies.remove(0),
        _ => Body::Multipart(bodies),
    };
    let mut m = Message::new(role, body);
    m.reasoning_content = reasoning;
    Ok(m)
}

/// A whole Chat Completions answer (`ModernResponse`, `MP:534-579`): the first choice's
/// message and `usage.prompt_tokens`. Also DeepSeek's first try.
pub fn parse_modern(body: &[u8]) -> Result<Option<Parsed>, String> {
    let v = object(body)?;
    let messages = match first_choice(&v)? {
        Some(choice) => vec![whole_message(&choice["message"])?],
        None => Vec::new(),
    };
    Ok(Some(Parsed { messages, response_id: None, prompt_tokens: v["usage"]["prompt_tokens"].as_i64(), ignore: false, error: None }))
}

/// `coercedContentString` (`MP:454-475`): streamed content as one string.
fn coerced(content: Option<Result<String, Vec<Part>>>) -> String {
    match content {
        None => String::new(),
        Some(Ok(text)) => text,
        Some(Err(parts)) => parts
            .iter()
            .map(|p| match p {
                Part::Text(t) => t.clone(),
                Part::File { data, filename } => format!("<iterm2:attachment file=\"{filename}\">\n{}\n</iterm2:attachment>", String::from_utf8_lossy(&unbase64(data))),
                Part::Image(_) => "[image]".to_string(),
                Part::Audio { .. } => "[audio]".to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// One streamed Chat Completions chunk (`ModernStreamingResponse`, `MP:581-662`): the first
/// choice's delta. A finish-only trailer after a call gives nothing; a call (with any text
/// beside it) is the first tool call or the legacy `function_call`; else the text, even empty.
/// `reasoning_content` is dropped, as iTerm2's. Also DeepSeek's fallback.
pub fn parse_modern_event(json: &str) -> Result<Option<Parsed>, String> {
    let v = object(json.as_bytes())?;
    let mut p = Parsed::default();
    let Some(choice) = first_choice(&v)? else {
        return Ok(Some(p));
    };
    let delta = &choice["delta"];
    let role = role_of(&delta["role"])?;
    let content = content_of(&delta["content"])?;
    let finish = choice["finish_reason"].as_str();
    let no_calls = delta["tool_calls"].as_array().is_none_or(Vec::is_empty);
    let trailer = matches!(finish, Some("function_call" | "tool_calls")) && role.is_none() && content.is_none() && delta["name"].is_null() && delta["function_call"].is_null() && no_calls;
    if trailer {
        return Ok(Some(p));
    }
    let text = coerced(content);
    let call = first_tool_call(delta).or_else(|| delta["function_call"].is_object().then(|| (call_of(&delta["function_call"]), None)));
    let body = match call {
        Some((call, id)) if text.is_empty() => Body::FunctionCall(call, id),
        Some((call, id)) => Body::Multipart(vec![Body::Text(text), Body::FunctionCall(call, id)]),
        None => Body::Text(text),
    };
    p.messages.push(Message::new(Some(Role::Assistant), body));
    Ok(Some(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_catalog::Model;
    use crate::ai_model::fixture;
    use crate::ai_msg::{Attachment, Body, CallId, FunctionCall, Message, Role, Status};
    use crate::ai_settings::{Api, Vendor};
    use crate::ai_transport::{Options, Proxy, Recorded, Reply};
    use crate::ai_turn::{headers, same_outcome, Build, Dialect, Hosted, Out, ToolDecl, Turn, TurnInput, Wire, EMPTY_RESPONSE, TOKEN_LIMIT};
    use crate::pwm::Secret;
    use serde_json::{json, Value};
    use std::sync::Arc;
    use std::time::Duration;

    const CHAT_STREAM: &str = concat!(
        r#"data: {"id":"c","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","content":"Hel"}}]}"#,
        "\r\n\r\n",
        ": keep-alive\r\n\r\n",
        r#"data: {"choices":[{"index":0,"delta":{"content":"lo"}}]}"#,
        "\r\n\r\n",
        r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"f","arguments":""}}]}}]}"#,
        "\r\n\r\n",
        r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"a\":1}"}}]}}]}"#,
        "\r\n\r\n",
        r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
        "\r\n\r\n",
        "data: [DONE]\r\n\r\n",
    );

    /// The legacy "streaming" shape iTerm2's parser expects (`LO:95-118`), one line per event.
    const LEGACY_STREAM: &str = concat!(
        r#"{"model":"davinci-test","created_at":"t","response":"Hel","done":false}"#,
        "\n",
        r#"data: {"model":"davinci-test","created_at":"t","response":"lo","done":false}"#,
        "\n",
        r#"{"model":"davinci-test","created_at":"t","response":"","done":true}"#,
        "\n",
    );

    fn chat_model() -> Model {
        fixture("gpt-test", "https://api.openai.com/v1/chat/completions", Api::ChatCompletions)
    }

    const AZURE: &str = "https://me.openai.azure.com/openai/deployments/d/chat/completions?api-version=2024-10-21";

    fn wire() -> Wire {
        Wire { token_limit: 128000, response_limit: 8192, zdr: false, ollama_num_ctx: 0, ollama_keep_alive: "30m".into() }
    }

    fn tool() -> ToolDecl {
        ToolDecl { name: "f".into(), description: "d".into(), parameters: json!({"type": "object", "properties": {}, "required": []}) }
    }

    fn build<'a>(m: &'a Model, messages: &'a [Message], tools: &'a [ToolDecl], hosted: &'a Hosted, wire: &'a Wire) -> Build<'a> {
        Build {
            model: m,
            key: "sk-a",
            messages,
            tools,
            stream: true,
            hosted,
            previous_response_id: None,
            should_think: None,
            effort: None,
            tier: None,
            volatile: None,
            frozen: None,
            wire,
        }
    }

    fn sh() -> Vec<Message> {
        vec![Message::text(Role::System, "S"), Message::text(Role::User, "hi")]
    }

    fn value(d: &dyn Dialect, b: &Build) -> Value {
        serde_json::from_slice(&d.body(b).expect("body builds")).expect("the body is JSON")
    }

    fn header_names(b: &Build) -> Vec<String> {
        headers(b).into_iter().map(|(n, _)| n).collect()
    }

    #[test]
    fn parallel_tool_calls_only_to_the_openai_host() {
        let (w, none, tools) = (wire(), Hosted::default(), [tool()]);
        let messages = sh();
        assert_eq!(value(&ChatCompletions, &build(&chat_model(), &messages, &tools, &none, &w))["parallel_tool_calls"], json!(false), "api.openai.com runs one call at a time");
        // Every other host gets iTerm2's bytes, even one 12a files under the OpenAI vendor (the
        // field is OpenAI's; m12b ruling).
        let mut local = fixture("llama3.3", "http://localhost:8080/v1/chat/completions", Api::ChatCompletions);
        local.vendor = Vendor::OpenAI;
        for other in [local, fixture("chat", "https://api.deepseek.com/chat/completions", Api::ChatCompletions), fixture("mine", "https://llm.example.com/v1/chat/completions", Api::ChatCompletions)] {
            let name = other.name.clone();
            let v = value(&ChatCompletions, &build(&other, &messages, &tools, &none, &w));
            assert_eq!(v.get("parallel_tool_calls"), None, "{name}");
            assert_eq!(v["tool_choice"], json!("auto"), "{name}");
        }
    }

    #[test]
    fn chat_body_follows_iterm2() {
        let (m, w, none) = (chat_model(), wire(), Hosted::default());
        let messages = sh();
        let tools = [tool()];
        let b = build(&m, &messages, &tools, &none, &w);
        assert_eq!(
            value(&ChatCompletions, &b),
            json!({"model":"gpt-test","messages":[{"role":"system","content":"S"},{"role":"user","content":"hi"}],"max_tokens":8192,"temperature":0,"tools":[{"type":"function","function":{"name":"f","description":"d","parameters":{"type":"object","properties":{},"required":[]}}}],"tool_choice":"auto","parallel_tool_calls":false,"stream":true})
        );
        let bytes = String::from_utf8(ChatCompletions.body(&b).unwrap()).unwrap();
        assert!(bytes.contains(r#""temperature":0,"#), "an integer: {bytes}");
        assert_eq!(header_names(&b), ["Authorization", "Content-Type"], "OpenAI authorizes with a bearer token");

        let plain = value(&ChatCompletions, &build(&m, &messages, &[], &none, &w));
        assert_eq!(plain, json!({"model":"gpt-test","messages":[{"role":"system","content":"S"},{"role":"user","content":"hi"}],"max_tokens":8192,"temperature":0,"stream":true}), "no tools: no tools, tool_choice or parallel_tool_calls");

        let mut whole = build(&m, &messages, &[], &none, &w);
        whole.stream = false;
        assert_eq!(value(&ChatCompletions, &whole)["stream"], json!(false), "always sent");

        let mut cold = chat_model();
        cold.supports_temperature = false;
        assert!(value(&ChatCompletions, &build(&cold, &messages, &[], &none, &w)).get("temperature").is_none());

        let azure = fixture("gpt-test", AZURE, Api::ChatCompletions);
        let b = build(&azure, &messages, &[], &none, &w);
        assert!(value(&ChatCompletions, &b).get("model").is_none(), "Azure names the model in its URL");
        assert_eq!(header_names(&b), ["api-key", "Content-Type"], "Azure authorizes with api-key");

        let mut named = chat_model();
        named.wire_model_name = Some("wire".into());
        assert_eq!(value(&ChatCompletions, &build(&named, &messages, &[], &none, &w))["model"], "gpt-test", "the name, not the wire name");

        // "S\n0123456789abcdef" is 18 bytes, 9 tokens: 10 - 9 is under 2.
        let long = vec![Message::text(Role::System, "S"), Message::text(Role::User, "0123456789abcdef")];
        let tight = Wire { token_limit: 10, ..wire() };
        assert_eq!(ChatCompletions.body(&build(&m, &long, &[], &none, &tight)), Err(TOKEN_LIMIT.to_string()));

        let mut frozen = build(&m, &messages, &[], &none, &w);
        frozen.frozen = Some(br#"{"role":"user","content":"old"}"#);
        assert_eq!(value(&ChatCompletions, &frozen)["messages"], json!([{"role":"system","content":"S"},{"role":"user","content":"old"},{"role":"user","content":"hi"}]), "after the system message");

        // The volatile text is one more user message (`RB:41-46`), counted in the budget:
        // "S\nhi\nV" is 6 bytes, 3 tokens.
        let small = Wire { token_limit: 100, ..wire() };
        let mut volatile = build(&m, &messages, &[], &none, &small);
        volatile.volatile = Some("V");
        let v = value(&ChatCompletions, &volatile);
        assert_eq!(v["messages"][2], json!({"role":"user","content":"V"}));
        assert_eq!(v["max_tokens"], 97);
        volatile.volatile = Some("");
        assert_eq!(value(&ChatCompletions, &volatile)["messages"].as_array().unwrap().len(), 2, "empty volatile text is not sent");
    }

    fn call(id: Option<&str>) -> FunctionCall {
        FunctionCall { name: Some("f".into()), arguments: Some("{}".into()), id: id.map(str::to_string), thought_signature: None }
    }

    fn ids(id: &str) -> Option<CallId> {
        Some(CallId { call_id: id.into(), item_id: id.into() })
    }

    fn file(name: &str, mime: &str, content: &[u8]) -> Body {
        Body::Attachment(Attachment::File { name: name.into(), mime: mime.into(), content: content.to_vec() })
    }

    fn user(body: Body) -> Message {
        Message::new(Some(Role::User), body)
    }

    #[test]
    fn chat_messages_follow_iterm2() {
        let assistant = |body: Body| Message::new(Some(Role::Assistant), body);
        // iTerm2 echoes a call's own id inside `function` (kept).
        assert_eq!(
            chat_message(&assistant(Body::FunctionCall(call(Some("call_1")), ids("call_1")))),
            Some(json!({"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"f","arguments":"{}","id":"call_1"}}]}))
        );
        assert_eq!(
            chat_message(&assistant(Body::FunctionCall(call(None), ids("call_1")))),
            Some(json!({"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"f","arguments":"{}"}}]})),
            "the CallId alone"
        );
        assert_eq!(chat_message(&assistant(Body::FunctionCall(call(None), None))), Some(json!({"role":"assistant","content":null,"name":"f","function_call":{"name":"f","arguments":"{}"}})));
        let output = |id| Message::new(Some(Role::Function), Body::FunctionOutput { name: "f".into(), output: "42".into(), id });
        assert_eq!(chat_message(&output(ids("call_1"))), Some(json!({"role":"tool","content":"42","tool_call_id":"call_1"})));
        assert_eq!(chat_message(&output(None)), Some(json!({"role":"function","name":"f","content":"42"})));

        let multipart = user(Body::Multipart(vec![
            Body::Text("look".into()),
            file("a.png", "image/png", &[0, 1, 2]),
            file("v.wav", "audio/wav", b"RIFF"),
            file("r.pdf", "application/pdf", b"%PDF"),
            file("a.zip", "application/zip", b"PK"),
            Body::Attachment(Attachment::FileId { id: "file_1".into(), name: "x.bin".into() }),
            Body::Attachment(Attachment::Status { id: "s".into(), inline: true, status: Status::WebSearchStarted }),
        ]));
        assert_eq!(
            chat_message(&multipart),
            Some(json!({"role":"user","content":[
                {"type":"text","text":"look"},
                {"type":"image_url","image_url":{"url":"data:image/png;base64,AAEC"}},
                {"type":"input_audio","input_audio":{"data":"UklGRg==","format":"wav"}},
                {"type":"file","file":{"file_data":"data:application/pdf;base64,JVBERg==","filename":"r.pdf"}},
                {"type":"text","text":"<iterm2:attachment file=\"a.zip\" type=\"application/zip\">\nPK\n</iterm2:attachment>"},
                {"type":"text","text":"A file named x.bin (content unavailable)"}
            ]})),
            "a status update is dropped"
        );
        assert_eq!(content_part("m.mp3", "audio/mpeg", b"ID3"), json!({"type":"input_audio","input_audio":{"data":"SUQz","format":"mp3"}}));
        assert_eq!(content_part("s.svg", "image/svg+xml", b"<svg/>"), json!({"type":"text","text":"<iterm2:attachment file=\"s.svg\" type=\"image/svg+xml\">\n<svg/>\n</iterm2:attachment>"}), "textual images are text");
        assert_eq!(attachment_text("a.zip", "application/zip", b"PK"), "<iterm2:attachment file=\"a.zip\" type=\"application/zip\">\nPK\n</iterm2:attachment>");
        assert_eq!(unavailable("x.bin"), "A file named x.bin (content unavailable)");

        // A top-level attachment: images and audio as one part, everything else as its text.
        assert_eq!(chat_message(&user(file("a.png", "image/png", &[0, 1, 2]))), Some(json!({"role":"user","content":[{"type":"image_url","image_url":{"url":"data:image/png;base64,AAEC"}}]})));
        assert_eq!(chat_message(&user(file("n.txt", "text/plain", b"hey"))), Some(json!({"role":"user","content":"hey"})));
        assert_eq!(chat_message(&user(file("r.pdf", "application/pdf", b"%PDF"))), Some(json!({"role":"user","content":"%PDF"})), "a top-level PDF is its bytes, as iTerm2's");
        assert_eq!(chat_message(&user(Body::Attachment(Attachment::FileId { id: "file_1".into(), name: "x.bin".into() }))), Some(json!({"role":"user","content":"A file named x.bin (content unavailable)"})));
        assert_eq!(chat_message(&assistant(Body::Attachment(Attachment::Code("x=1".into())))), Some(json!({"role":"assistant","content":"x=1"})));
        assert_eq!(chat_message(&assistant(Body::Attachment(Attachment::Status { id: "s".into(), inline: true, status: Status::WebSearchStarted }))), Some(json!({"role":"assistant","content":"Searching the web\u{2026}"})));
        assert_eq!(chat_message(&user(Body::Uninitialized)), None);
        assert_eq!(chat_message(&Message::new(None, Body::Text("x".into()))), Some(json!({"role":null,"content":"x"})), "role and content are always written");

        // A preamble with a call: its text parts and the first call.
        let mixed = assistant(Body::Multipart(vec![Body::Text("Let me".into()), Body::FunctionCall(call(None), ids("call_1")), Body::FunctionCall(call(None), ids("call_2")), Body::Attachment(Attachment::Code("c".into()))]));
        assert_eq!(
            chat_message(&mixed),
            Some(json!({"role":"assistant","content":[{"type":"text","text":"Let me"},{"type":"text","text":"c"}],"tool_calls":[{"id":"call_1","type":"function","function":{"name":"f","arguments":"{}"}}]}))
        );
        assert_eq!(chat_message(&assistant(Body::Multipart(vec![Body::FunctionCall(call(None), None)]))), Some(json!({"role":"assistant","content":null,"name":"f","function_call":{"name":"f","arguments":"{}"}})));

        // Ollama's native shapes (`MP:150-249`).
        let ollama = |m: &Message| chat_message_for(m, true);
        assert_eq!(ollama(&output(None)), Some(json!({"role":"tool","tool_name":"f","content":"42"})));
        let args = FunctionCall { arguments: Some("{\"a\":1}".into()), ..call(None) };
        assert_eq!(ollama(&assistant(Body::FunctionCall(args, None))), Some(json!({"role":"assistant","tool_calls":[{"type":"function","function":{"name":"f","arguments":{"a":1}}}]})));
        let broken = FunctionCall { arguments: Some("{".into()), ..call(None) };
        assert_eq!(ollama(&assistant(Body::FunctionCall(broken, None)))
            .map(|v| v["tool_calls"][0]["function"]["arguments"].clone()), Some(json!({})), "unparseable arguments are an empty object");
        assert_eq!(ollama(&user(Body::Multipart(vec![Body::Text("look".into()), file("a.png", "image/png", &[0, 1, 2])]))), Some(json!({"role":"user","content":"look","images":["AAEC"]})));
        assert_eq!(ollama(&assistant(Body::Multipart(vec![Body::Text("a".into()), Body::Text("b".into())]))), Some(json!({"role":"assistant","content":"a\nb"})), "content is one string");
        assert_eq!(ollama(&Message::text(Role::User, "hi")), chat_message(&Message::text(Role::User, "hi")));
    }

    #[test]
    fn early_o1_body_follows_iterm2() {
        let (w, none) = (wire(), Hosted::default());
        let m = fixture("o1-test", "https://api.openai.com/v1/chat/completions", Api::EarlyO1);
        let messages = sh();
        let tools = [tool()];
        let b = build(&m, &messages, &tools, &none, &w);
        assert_eq!(value(&EarlyO1, &b), json!({"model":"o1-test","messages":[{"role":"user","content":"S"},{"role":"user","content":"hi"}],"max_completion_tokens":8192}));
        let azure = fixture("o1-test", AZURE, Api::EarlyO1);
        assert_eq!(value(&EarlyO1, &build(&azure, &messages, &[], &none, &w))["model"], "o1-test", "sent even on Azure");
        let long = vec![Message::text(Role::System, "S"), Message::text(Role::User, "0123456789abcdef")];
        let tight = Wire { token_limit: 10, ..wire() };
        assert_eq!(EarlyO1.body(&build(&m, &long, &[], &none, &tight)), Err(TOKEN_LIMIT.to_string()));
        let mut frozen = build(&m, &messages, &[], &none, &w);
        frozen.frozen = Some(br#"{"role":"user","content":"old"}"#);
        assert_eq!(value(&EarlyO1, &frozen)["messages"], json!([{"role":"user","content":"S"},{"role":"user","content":"old"},{"role":"user","content":"hi"}]), "after the converted system message");
    }

    #[test]
    fn legacy_body_follows_iterm2() {
        let (w, none) = (wire(), Hosted::default());
        let m = fixture("davinci-test", "https://api.openai.com/v1/completions", Api::Completions);
        let messages = sh();
        let tools = [tool()];
        assert_eq!(value(&Completions, &build(&m, &messages, &tools, &none, &w)), json!({"model":"davinci-test","prompt":"S\nhi","max_tokens":8192}));
        let azure = fixture("davinci-test", AZURE, Api::Completions);
        assert_eq!(value(&Completions, &build(&azure, &messages, &[], &none, &w)), json!({"prompt":"S\nhi","max_tokens":8192}));
        let mut volatile = build(&m, &messages, &[], &none, &w);
        volatile.volatile = Some("V");
        assert_eq!(value(&Completions, &volatile)["prompt"], "S\nhi\nV");
        let long = vec![Message::text(Role::System, "S"), Message::text(Role::User, "0123456789abcdef")];
        let tight = Wire { token_limit: 10, ..wire() };
        assert_eq!(Completions.body(&build(&m, &long, &[], &none, &tight)), Err(TOKEN_LIMIT.to_string()));

        let (mut r, rec) = runner(Turn::new(TurnInput { model: m.clone(), stream: false, ..turn_input() }), vec![Reply::ok(&[br#"{"choices":[{"text":" hi","finish_reason":"stop"}]}"#])]);
        assert_eq!(r.start(Some(&key())), [Out::WillSend]);
        assert_eq!(r.poll(), [Out::Choice("hi".into())]);
        assert_eq!(rec.requests()[0].url, "https://api.openai.com/v1/completions");
        let p = Completions.parse(br#"{"choices":[{"text":"a","index":0,"finish_reason":"stop"},{"text":"b"}],"usage":{"prompt_tokens":4,"total_tokens":5}}"#).unwrap().unwrap();
        assert_eq!((p.messages, p.prompt_tokens), (vec![Message::text(Role::Assistant, "a")], Some(4)), "only the first choice");
        assert!(Completions.parse(br#"{"choices":[]}"#).unwrap().unwrap().messages.is_empty());
        assert!(Completions.parse(b"nope").is_err());
    }

    #[test]
    fn legacy_stream_follows_iterm2() {
        let m = fixture("davinci-test", "https://api.openai.com/v1/completions", Api::Completions);
        let lines: Vec<&[u8]> = LEGACY_STREAM.split_inclusive('\n').map(str::as_bytes).collect();
        let out = streamed(&m, &lines);
        assert_eq!(out, [Out::Delta("Hel".into()), Out::Delta("lo".into()), Out::StreamEnd]);
        let whole = LEGACY_STREAM.as_bytes();
        for at in 1..whole.len() {
            let split = streamed(&m, &[&whole[..at], &whole[at..]]);
            assert!(same_outcome(&split, &out), "split at {at}: {split:?}");
        }
        assert_eq!(Completions.parse_event(r#"{"model":"m","created_at":"t","response":"","done":true}"#), Ok(None), "done ends the stream");
        assert!(Completions.parse_event("nope").is_err());
        let mut at = ai_sse::Cursor::default();
        assert_eq!(Completions.split("data: {\"a\":1}\nrest", &mut at).as_deref(), Some("{\"a\":1}"));
        assert_eq!(at.rest("data: {\"a\":1}\nrest"), "rest");
    }

    fn key() -> Secret {
        Secret::new("sk-a".to_string())
    }

    fn turn_input() -> TurnInput {
        TurnInput {
            model: chat_model(),
            messages: sh(),
            tools: vec![tool()],
            stream: true,
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

    fn runner(t: Turn, replies: Vec<Reply>) -> (crate::ai_turn::Runner, Arc<Recorded>) {
        let rec = Arc::new(Recorded::new(replies));
        (crate::ai_turn::Runner::new(t, rec.clone(), Options { idle: Duration::ZERO, proxy: Proxy::Off }, Arc::new(|| {})), rec)
    }

    /// `m`'s streamed answer in `chunks`: everything after `WillSend`.
    fn streamed(m: &Model, chunks: &[&[u8]]) -> Vec<Out> {
        let (mut r, _) = runner(Turn::new(TurnInput { model: m.clone(), ..turn_input() }), vec![Reply::ok(chunks)]);
        assert_eq!(r.start(Some(&key())), [Out::WillSend]);
        r.poll()
    }

    fn events() -> Vec<&'static [u8]> {
        CHAT_STREAM.split_inclusive("\r\n\r\n").map(str::as_bytes).collect()
    }

    fn one(json: &str) -> Vec<Message> {
        let p = parse_modern_event(json).expect("parses").expect("does not end the stream");
        assert!(!p.ignore && p.response_id.is_none(), "{json}");
        p.messages
    }

    #[test]
    fn chat_stream_follows_iterm2() {
        let out = streamed(&chat_model(), &events());
        assert_eq!(
            out,
            [
                Out::Delta("Hel".into()),
                Out::Delta("lo".into()),
                Out::Invoke { call: FunctionCall { name: Some("f".into()), arguments: Some("{\"a\":1}".into()), id: None, thought_signature: None }, id: ids("call_1") },
            ]
        );
        // The trailer names the finish reason only.
        assert!(one(r#"{"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#).is_empty());
        assert!(one(r#"{"choices":[{"index":0,"delta":{"role":null},"finish_reason":"function_call"}]}"#).is_empty());
        assert_eq!(one(r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#), [Message::text(Role::Assistant, "")], "other finishes are empty text");
        assert_eq!(one(r#"{"choices":[{"index":0,"delta":{"reasoning_content":"r","content":"x"}}]}"#), [Message::text(Role::Assistant, "x")], "reasoning_content is dropped, as iTerm2's");
        assert_eq!(
            one(r#"{"choices":[{"index":0,"delta":{"content":"a","tool_calls":[{"index":0,"id":"call_1","function":{"name":"f","arguments":""}}]}}]}"#),
            [Message::new(Some(Role::Assistant), Body::Multipart(vec![Body::Text("a".into()), Body::FunctionCall(FunctionCall { arguments: Some(String::new()), ..call(None) }, ids("call_1"))]))],
            "text and a call together"
        );
        assert_eq!(
            one(r#"{"choices":[{"index":0,"delta":{"function_call":{"name":"f","arguments":"{}"}}}]}"#),
            [Message::new(Some(Role::Assistant), Body::FunctionCall(call(None), None))],
            "the legacy function_call"
        );
        assert_eq!(
            one(r#"{"choices":[{"index":0,"delta":{"content":[{"type":"text","text":"t"},{"type":"image_url","image_url":{"url":"u"}},{"type":"input_audio","input_audio":{"data":"","format":"wav"}},{"type":"file","file":{"file_data":"UEs=","filename":"a.zip"}}]}}]}"#),
            [Message::text(Role::Assistant, "t\n[image]\n[audio]\n<iterm2:attachment file=\"a.zip\">\nPK\n</iterm2:attachment>")]
        );
        assert_eq!(one(r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1"},{"index":1,"function":{"name":"g"}}]}}]}"#), [Message::text(Role::Assistant, "")], "only the first entry counts");
        assert!(one(r#"{"choices":[]}"#).is_empty());
        assert!(parse_modern_event("nope").is_err());
        assert!(parse_modern_event(r#"{"id":"x"}"#).is_err(), "no choices");
    }

    #[test]
    fn chat_stream_any_chunking() {
        let expected = streamed(&chat_model(), &events());
        let whole = CHAT_STREAM.as_bytes();
        assert!(same_outcome(&streamed(&chat_model(), &[whole]), &expected), "one chunk");
        for at in 1..whole.len() {
            let out = streamed(&chat_model(), &[&whole[..at], &whole[at..]]);
            assert!(same_outcome(&out, &expected), "split at {at}: {out:?}");
        }
    }

    fn answered(body: &str) -> Vec<Out> {
        let (mut r, _) = runner(Turn::new(TurnInput { stream: false, ..turn_input() }), vec![Reply::ok(&[body.as_bytes()])]);
        assert_eq!(r.start(Some(&key())), [Out::WillSend]);
        r.poll()
    }

    #[test]
    fn chat_parse_follows_iterm2() {
        let hi = r#"{"choices":[{"index":0,"message":{"role":"assistant","content":"Hi"},"finish_reason":"stop"}],"usage":{"prompt_tokens":9,"total_tokens":12}}"#;
        let p = parse_modern(hi.as_bytes()).unwrap().unwrap();
        assert_eq!((p.messages, p.prompt_tokens, p.response_id), (vec![Message::text(Role::Assistant, "Hi")], Some(9), None));
        assert_eq!(answered(hi), [Out::Choice("Hi".into())]);

        let called = r#"{"choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"f","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#;
        assert_eq!(answered(called), [Out::Invoke { call: call(None), id: ids("call_1") }]);
        let p = parse_modern(br#"{"choices":[{"index":0,"message":{"role":"assistant","content":"Let me","tool_calls":[{"id":"call_1","function":{"name":"f","arguments":"{}"}},{"id":"call_2","function":{"name":"g","arguments":"{}"}}]}}]}"#).unwrap().unwrap();
        assert_eq!(p.messages, [Message::new(Some(Role::Assistant), Body::Multipart(vec![Body::Text("Let me".into()), Body::FunctionCall(call(None), ids("call_1"))]))], "text and the first call");
        let legacy = parse_modern(br#"{"choices":[{"index":0,"message":{"role":"assistant","content":null,"function_call":{"name":"f","arguments":"{}"}},"finish_reason":"function_call"}]}"#).unwrap().unwrap();
        assert_eq!(legacy.messages, [Message::new(Some(Role::Assistant), Body::FunctionCall(call(None), None))]);

        assert_eq!(answered(r#"{"choices":[]}"#), [Out::Failed(EMPTY_RESPONSE.into())]);
        let output = parse_modern(br#"{"choices":[{"index":0,"message":{"role":"function","name":"f","content":"42"},"finish_reason":"stop"}]}"#).unwrap().unwrap();
        assert_eq!(output.messages, [Message::new(Some(Role::Function), Body::FunctionOutput { name: "f".into(), output: "42".into(), id: None })], "iTerm2's llmMessage");

        let reasoned = parse_modern(br#"{"choices":[{"index":0,"message":{"role":"assistant","content":"x","reasoning_content":"r"}}]}"#).unwrap().unwrap();
        assert_eq!(reasoned.messages[0].reasoning_content.as_deref(), Some("r"), "a whole answer keeps its reasoning");
        let empty = parse_modern(br#"{"choices":[{"index":0,"message":{"role":"assistant","content":""}}]}"#).unwrap().unwrap();
        assert_eq!(empty.messages[0].body, Body::Uninitialized);
        let parts = parse_modern(br#"{"choices":[{"index":0,"message":{"role":"assistant","content":[{"type":"text","text":"a"},{"type":"image_url","image_url":{"url":"data:image/png;base64,AAEC"}},{"type":"image_url","image_url":{"url":"https://x/y.png"}},{"type":"input_audio","input_audio":{"data":"UklGRg==","format":"wav"}}]}}]}"#).unwrap().unwrap();
        assert_eq!(
            parts.messages[0].body,
            Body::Multipart(vec![Body::Text("a".into()), file("image", "image/png", &[0, 1, 2]), Body::Text("https://x/y.png".into()), file("audio", "audio/wav", b"RIFF")])
        );
        assert!(parse_modern(b"nope").is_err());
        assert!(parse_modern(br#"{"model":"m","message":{"content":"Hi"},"done":true}"#).is_err(), "no choices: not this shape");
        assert!(parse_modern(br#"{"choices":[{"index":0,"message":{"role":"assistant","content":7}}]}"#).is_err(), "content is text or parts");
    }
}

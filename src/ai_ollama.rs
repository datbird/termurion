//! Ollama's native `/api/chat`: the request body with its context sizing (`num_ctx`,
//! `num_predict`), keep-alive and thinking switches, and the parsers of whole answers and streamed
//! NDJSON lines. Pure; ports `AITerm/Llama.swift` (`LL:10-543`).

use crate::ai_chat::{chat_message_for, ollama_joined_content, systems, with_volatile};
use crate::ai_json::splice_at_key;
use crate::ai_model::{dynamic_models_supported, effective_name, tokens, DEFAULT_CONTEXT};
use crate::ai_msg::{Attachment, Body, FunctionCall, Message, Role, Status};
use crate::ai_sse;
use crate::ai_turn::{max_tokens_for, Build, Dialect, Parsed, ToolDecl, TOKEN_LIMIT};
use base64::Engine;
use serde_json::{json, Map, Value};

pub struct Ollama;

/// The status attachment of streamed thinking (`LL:150-155`).
pub const REASONING_ID: &str = "ollama-reasoning";

/// Room kept in `num_ctx` beyond the prompt and the reserved answer (`LL:248`).
const HEADROOM: i64 = 256;
/// The smallest `num_ctx` sent (`LL:249`).
const MIN_NUM_CTX: i64 = 4096;
/// Less room than this for the answer refuses the request (`LL:260`).
const MIN_RESPONSE_BUDGET: i64 = 128;
/// The most answer space reserved in `num_ctx` (`LL:264`).
const MAX_RESPONSE_RESERVE: i64 = 8192;
const IMAGE_TOKEN_FLOOR: i64 = 85;
const IMAGE_TOKEN_CAP: i64 = 12288;

/// The least power of two at or above `n` (1 for n below 2), i64::MAX past the largest.
fn power_of_two_at_least(n: i64) -> i64 {
    if n < 2 {
        return 1;
    }
    (n as u64).checked_next_power_of_two().and_then(|p| i64::try_from(p).ok()).unwrap_or(i64::MAX)
}

/// `computedNumCtx` (`LL:279-295`): a positive `ollamaNumCtx` override as it is; else the prompt
/// plus a reserved answer plus headroom, rounded up to a power of two of at least 4096, capped at
/// the model's window (32768 when unknown).
pub fn num_ctx(prompt_tokens: i64, num_predict: i64, context_window: i64, override_ctx: i64) -> i64 {
    if override_ctx > 0 {
        return override_ctx;
    }
    let window = if context_window > 0 { context_window } else { DEFAULT_CONTEXT };
    let reserve = num_predict.clamp(512, MAX_RESPONSE_RESERVE);
    let desired = prompt_tokens + reserve + HEADROOM;
    power_of_two_at_least(desired.max(MIN_NUM_CTX)).min(window)
}

/// `num_ctx` and the `num_predict` that fits in it (`LL:489-510`), or the token-limit error with
/// its detail when the prompt leaves under 128 tokens for the answer.
pub fn budget(prompt_tokens: i64, num_predict: i64, context_window: i64, override_ctx: i64) -> Result<(i64, i64), String> {
    let n = num_ctx(prompt_tokens, num_predict, context_window, override_ctx);
    let room = n - prompt_tokens;
    if room < MIN_RESPONSE_BUDGET {
        return Err(if override_ctx > 0 { over_override(prompt_tokens, override_ctx) } else { over_ctx(prompt_tokens, n) });
    }
    Ok((n, num_predict.min(room - 64).max(1)))
}

/// `imageTokenEstimate` (`LL:390-403`): pixel area / 750 rounded up, between 85 and 12288.
pub fn image_tokens(width: u32, height: u32) -> i64 {
    if width == 0 || height == 0 {
        return IMAGE_TOKEN_FLOOR;
    }
    let estimate = (f64::from(width) * f64::from(height) / 750.0).ceil() as i64;
    estimate.clamp(IMAGE_TOKEN_FLOOR, IMAGE_TOKEN_CAP)
}

/// The refusal when the prompt does not fit a pinned `ollamaNumCtx` (`LL:503-504`).
pub fn over_override(prompt: i64, override_ctx: i64) -> String {
    format!("{TOKEN_LIMIT} (prompt \u{2248}{prompt} tokens exceeds the ollamaNumCtx override of {override_ctx})")
}

/// The refusal when the prompt does not fit the sized `num_ctx` (`LL:505`).
pub fn over_ctx(prompt: i64, num_ctx: i64) -> String {
    format!("{TOKEN_LIMIT} (prompt \u{2248}{prompt} tokens needs more than num_ctx {num_ctx})")
}

/// An image's estimate from a data URL or bare base64 (`LL:390-423`): its pixel size read from
/// the header, or the floor when it does not decode.
fn image_tokens_of(base64_or_data_url: &str) -> i64 {
    let payload = base64_or_data_url.strip_prefix("data:").and_then(|rest| rest.split_once(',')).map_or(base64_or_data_url, |(_, payload)| payload);
    // Characters outside base64 are skipped, as `.ignoreUnknownCharacters` does.
    let clean: String = payload.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=')).collect();
    let size = base64::engine::general_purpose::STANDARD
        .decode(clean)
        .ok()
        .and_then(|bytes| image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?.into_dimensions().ok());
    match size {
        Some((width, height)) => image_tokens(width, height),
        None => IMAGE_TOKEN_FLOOR,
    }
}

/// `estimatedTokens(for:)` (`LL:317-338`): a message's text as bytes / 2 plus one, and an image
/// by its area instead of its base64 length.
fn message_tokens(m: &Message) -> i64 {
    match ollama_joined_content(m) {
        Some(Value::String(text)) => tokens(&text) + 1,
        Some(Value::Array(parts)) => {
            let s = |v: &Value| v.as_str().unwrap_or_default().to_string();
            1 + parts
                .iter()
                .map(|p| match p["type"].as_str() {
                    Some("image_url") => image_tokens_of(p["image_url"]["url"].as_str().unwrap_or_default()),
                    Some("text") => tokens(&s(&p["text"])) + 1,
                    Some("file") => tokens(&(s(&p["file"]["file_data"]) + &s(&p["file"]["filename"]))) + 1,
                    Some("input_audio") => tokens(&s(&p["input_audio"]["data"])) + 1,
                    _ => 0,
                })
                .sum::<i64>()
        }
        _ => 0,
    }
}

/// `estimatedFrozenTokens` (`LL:347-369`): each spliced message's JSON as bytes / 2, its images
/// by area; bytes that do not parse count as they are.
fn frozen_tokens(frozen: &[u8]) -> i64 {
    let as_bytes = || tokens(&String::from_utf8_lossy(frozen));
    let wrapped = [b"[".as_slice(), frozen, b"]"].concat();
    let Ok(Value::Array(messages)) = serde_json::from_slice::<Value>(&wrapped) else {
        return as_bytes();
    };
    if !messages.iter().all(Value::is_object) {
        return as_bytes();
    }
    let mut total = 0;
    for mut message in messages {
        let images: Option<Vec<String>> = message["images"].as_array().and_then(|images| images.iter().map(|i| i.as_str().map(str::to_string)).collect());
        if let (Some(images), Some(map)) = (images, message.as_object_mut()) {
            total += images.iter().map(|i| image_tokens_of(i)).sum::<i64>();
            map.remove("images");
        }
        total += tokens(&message.to_string());
    }
    total
}

/// The tools as Ollama takes them (`LlamaFunctionDeclaration`, `LL:536-543`).
fn declarations(tools: &[ToolDecl]) -> Vec<Value> {
    tools.iter().map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.parameters}})).collect()
}

/// The prompt estimate `num_ctx` is sized for (`LL:297-308`, `LL:485-488`): the messages, the
/// tools' JSON and the frozen history spliced in after.
pub fn prompt_tokens(messages: &[Message], tools: &[ToolDecl], frozen: Option<&[u8]>) -> i64 {
    let mut total: i64 = messages.iter().map(message_tokens).sum();
    if !tools.is_empty() {
        total += tokens(&Value::Array(declarations(tools)).to_string());
    }
    if let Some(frozen) = frozen.filter(|f| !f.is_empty()) {
        total += frozen_tokens(frozen);
    }
    total
}

fn object(bytes: &[u8]) -> Result<Value, String> {
    let v: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if !v["message"].is_object() {
        return Err("the response has no message".to_string());
    }
    if !v["done"].is_boolean() {
        return Err("the response has no done flag".to_string());
    }
    Ok(v)
}

/// The first tool call (`LL:92-95`), its arguments as JSON (`{}` when absent).
fn first_call(message: &Value) -> Option<FunctionCall> {
    let entry = message["tool_calls"].as_array()?.first().filter(|e| e["function"].is_object())?;
    let function = &entry["function"];
    let arguments = match &function["arguments"] {
        v @ Value::Object(_) => v.to_string(),
        _ => "{}".to_string(),
    };
    Some(FunctionCall { name: function["name"].as_str().map(str::to_string), arguments: Some(arguments), id: None, thought_signature: None })
}

fn assistant(body: Body) -> Message {
    Message::new(Some(Role::Assistant), body)
}

impl Dialect for Ollama {
    /// `LlamaBodyRequestBuilder.body()` (`LL:459-533`): declaration order, not sorted.
    fn body(&self, b: &Build) -> Result<Vec<u8>, String> {
        let messages = with_volatile(b);
        let num_predict = max_tokens_for(&Build { messages: &messages, ..*b });
        if num_predict < 2 {
            return Err(TOKEN_LIMIT.to_string());
        }
        let prompt = prompt_tokens(&messages, b.tools, b.frozen);
        let (num_ctx, num_predict) = budget(prompt, num_predict, b.model.context_window_tokens, b.wire.ollama_num_ctx)?;
        let mut body = Map::new();
        if dynamic_models_supported(b.model) {
            body.insert("model".into(), json!(effective_name(b.model)));
        }
        body.insert("messages".into(), Value::Array(messages.iter().filter_map(|m| chat_message_for(m, true)).collect()));
        if !b.tools.is_empty() {
            body.insert("tools".into(), Value::Array(declarations(b.tools)));
        }
        body.insert("stream".into(), json!(b.stream));
        if let Some(think) = b.should_think {
            body.insert("think".into(), json!(think));
        }
        if !b.wire.ollama_keep_alive.is_empty() {
            body.insert("keep_alive".into(), json!(b.wire.ollama_keep_alive));
        }
        body.insert("options".into(), json!({"num_ctx": num_ctx, "num_predict": num_predict}));
        let bytes = serde_json::to_vec(&Value::Object(body)).expect("a JSON value always encodes");
        splice_at_key(&bytes, "messages", systems(&messages), b.frozen)
    }

    /// A whole answer (`LL:113-143`): one assistant message carrying the text, the call (both
    /// as a multipart) and the thinking; thinking stands in for an empty answer. No usage.
    fn parse(&self, body: &[u8]) -> Result<Option<Parsed>, String> {
        let v = object(body)?;
        let message = &v["message"];
        let content = message["content"].as_str().unwrap_or_default();
        let thinking = message["thinking"].as_str().filter(|t| !t.is_empty());
        let (body, reasoning) = match first_call(message) {
            Some(call) if content.is_empty() => (Body::FunctionCall(call, None), thinking),
            Some(call) => (Body::Multipart(vec![Body::Text(content.to_string()), Body::FunctionCall(call, None)]), thinking),
            None if !content.is_empty() => (Body::Text(content.to_string()), thinking),
            // The answer came only as thinking: shown once, as the answer.
            None => (Body::Text(thinking.unwrap_or(content).to_string()), None),
        };
        let mut m = assistant(body);
        m.reasoning_content = reasoning.map(str::to_string);
        Ok(Some(Parsed { messages: vec![m], ..Parsed::default() }))
    }

    fn split(&self, buffer: &str, at: &mut ai_sse::Cursor) -> Option<String> {
        ai_sse::split_ndjson(buffer, at)
    }

    /// One streamed line (`LL:96-176`, `LL:187-189`): the done line gives nothing (a call it
    /// repeats already ran); else the thinking as a status, the text, then the first call. A
    /// line with none of them is ignored.
    fn parse_event(&self, json: &str) -> Result<Option<Parsed>, String> {
        let v = object(json.as_bytes())?;
        let message = &v["message"];
        let content = message["content"].as_str().unwrap_or_default();
        let thinking = message["thinking"].as_str().filter(|t| !t.is_empty());
        let mut p = Parsed { ignore: content.is_empty() && message["tool_calls"].is_null() && thinking.is_none(), ..Parsed::default() };
        if v["done"] == true {
            return Ok(Some(p));
        }
        if let Some(thinking) = thinking {
            let status = Attachment::Status { id: REASONING_ID.to_string(), inline: true, status: Status::ReasoningSummary(thinking.to_string()) };
            let mut m = assistant(Body::Attachment(status));
            m.reasoning_content = Some(thinking.to_string());
            p.messages.push(m);
        }
        if !content.is_empty() {
            p.messages.push(assistant(Body::Text(content.to_string())));
        }
        if let Some(call) = first_call(message) {
            p.messages.push(assistant(Body::FunctionCall(call, None)));
        } else if p.messages.is_empty() {
            p.messages.push(assistant(Body::Text(content.to_string())));
        }
        Ok(Some(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_catalog::{Feature, Model};
    use crate::ai_model::{fixture, tokens};
    use crate::ai_msg::{Attachment, Body, FunctionCall, Message, Role, Status};
    use crate::ai_settings::Api;
    use crate::ai_transport::{Options, Proxy, Recorded, Reply};
    use crate::ai_turn::{headers, same_outcome, Hosted, Out, ToolDecl, ToolResult, Turn, TurnInput, Wire, TOKEN_LIMIT};
    use crate::pwm::Secret;
    use base64::Engine;
    use serde_json::{json, Value};
    use std::sync::Arc;
    use std::time::Duration;

    const OLLAMA_STREAM: &str = concat!(
        r#"{"model":"llama3.3","created_at":"t","message":{"role":"assistant","content":"","thinking":"hmm"},"done":false}"#,
        "\n",
        r#"{"model":"llama3.3","created_at":"t","message":{"role":"assistant","content":"Hi"},"done":false}"#,
        "\n",
        r#"{"model":"llama3.3","created_at":"t","message":{"role":"assistant","content":""},"done":true,"total_duration":1}"#,
        "\n",
    );

    const URL: &str = "http://localhost:11434/api/chat";

    fn model() -> Model {
        Model { context_window_tokens: 32768, ..fixture("llama3.3", URL, Api::Llama) }
    }

    fn wire() -> Wire {
        Wire { token_limit: 128000, response_limit: 8192, zdr: false, ollama_num_ctx: 0, ollama_keep_alive: "30m".into() }
    }

    fn tool() -> ToolDecl {
        ToolDecl { name: "f".into(), description: "d".into(), parameters: json!({"type": "object", "properties": {}, "required": []}) }
    }

    fn build<'a>(m: &'a Model, messages: &'a [Message], tools: &'a [ToolDecl], hosted: &'a Hosted, wire: &'a Wire) -> Build<'a> {
        Build {
            model: m,
            key: "Placeholder for self-hosted",
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

    fn body(b: &Build) -> String {
        String::from_utf8(Ollama.body(b).expect("body builds")).expect("UTF-8")
    }

    fn sh() -> Vec<Message> {
        vec![Message::text(Role::System, "S"), Message::text(Role::User, "hi")]
    }

    fn wire_message(m: Message) -> Value {
        let (model, w, none) = (model(), wire(), Hosted::default());
        let messages = [m];
        let v: Value = serde_json::from_slice(&Ollama.body(&build(&model, &messages, &[], &none, &w)).expect("body builds")).expect("JSON");
        v["messages"][0].clone()
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageLuma8(image::GrayImage::new(width, height)).write_to(&mut out, image::ImageFormat::Png).expect("encodes");
        out.into_inner()
    }

    fn file(name: &str, mime: &str, content: &[u8]) -> Body {
        Body::Attachment(Attachment::File { name: name.into(), mime: mime.into(), content: content.to_vec() })
    }

    #[test]
    fn power_of_two_never_loops() {
        assert_eq!([-5, 0, 1, 2, 3, 4096, 4097].map(power_of_two_at_least), [1, 1, 1, 2, 4, 4096, 8192]);
        assert_eq!(power_of_two_at_least(1 << 62), 1 << 62);
        assert_eq!(power_of_two_at_least((1 << 62) + 1), i64::MAX, "past the largest, where the loop wrapped to 0");
        assert_eq!(power_of_two_at_least(i64::MAX), i64::MAX);
    }

    #[test]
    fn ollama_body_follows_iterm2() {
        let (m, w, none) = (model(), wire(), Hosted::default());
        let messages = sh();
        let tools = [tool()];
        let b = build(&m, &messages, &tools, &none, &w);
        assert_eq!(
            body(&b),
            r#"{"model":"llama3.3","messages":[{"role":"system","content":"S"},{"role":"user","content":"hi"}],"tools":[{"type":"function","function":{"name":"f","description":"d","parameters":{"type":"object","properties":{},"required":[]}}}],"stream":true,"keep_alive":"30m","options":{"num_ctx":16384,"num_predict":8192}}"#
        );
        assert_eq!(
            body(&build(&m, &messages, &[], &none, &w)),
            r#"{"model":"llama3.3","messages":[{"role":"system","content":"S"},{"role":"user","content":"hi"}],"stream":true,"keep_alive":"30m","options":{"num_ctx":16384,"num_predict":8192}}"#,
            "no tools"
        );
        for (think, text) in [(Some(true), r#""stream":true,"think":true,"keep_alive""#), (Some(false), r#""stream":true,"think":false,"keep_alive""#), (None, r#""stream":true,"keep_alive""#)] {
            let b = Build { should_think: think, ..build(&m, &messages, &[], &none, &w) };
            assert!(body(&b).contains(text), "{think:?}: {}", body(&b));
        }
        let forever = Wire { ollama_keep_alive: String::new(), ..wire() };
        assert!(body(&build(&m, &messages, &[], &none, &forever)).contains(r#""stream":true,"options""#), "an empty keep-alive is omitted");
        let tagged = Model { wire_model_name: Some("llama3.3:latest".into()), ..model() };
        assert!(body(&build(&tagged, &messages, &[], &none, &w)).starts_with(r#"{"model":"llama3.3:latest","#), "the wire name goes on the wire");
        let pinned = Wire { ollama_num_ctx: 2048, ..wire() };
        assert!(body(&build(&m, &messages, &[], &none, &pinned)).ends_with(r#""options":{"num_ctx":2048,"num_predict":1981}}"#), "the override is sent as it is");

        // No key header ever, whatever the key and the host (`AUTH:79-80`).
        for url in [URL, "https://ollama.example.com/api/chat"] {
            let remote = Model { url: url.into(), ..model() };
            let b = Build { key: "sk-ABCDEFGHIJKLMNOPQRSTUV", ..build(&remote, &messages, &[], &none, &w) };
            let names: Vec<String> = headers(&b).into_iter().map(|(n, _)| n).collect();
            assert_eq!(names, ["Content-Type"], "{url}");
        }

        // Ollama's native message shapes (`MP:150-249`).
        let call = FunctionCall { name: Some("f".into()), arguments: Some("{\"a\":1}".into()), id: None, thought_signature: None };
        assert_eq!(wire_message(Message::new(Some(Role::Assistant), Body::FunctionCall(call, None))), json!({"role":"assistant","tool_calls":[{"type":"function","function":{"name":"f","arguments":{"a":1}}}]}));
        assert_eq!(wire_message(Message::new(Some(Role::Function), Body::FunctionOutput { name: "f".into(), output: "42".into(), id: None })), json!({"role":"tool","content":"42","tool_name":"f"}));
        assert_eq!(wire_message(Message::new(Some(Role::User), Body::Multipart(vec![Body::Text("look".into()), file("a.png", "image/png", &[0, 1, 2])]))), json!({"role":"user","content":"look","images":["AAEC"]}));

        // The volatile text is a last user message; the frozen history goes after the system.
        let mut v = build(&m, &messages, &[], &none, &w);
        v.volatile = Some("V");
        assert!(body(&v).contains(r#"{"role":"user","content":"hi"},{"role":"user","content":"V"}]"#), "{}", body(&v));
        let mut frozen = build(&m, &messages, &[], &none, &w);
        frozen.frozen = Some(br#"{"role":"user","content":"old"}"#);
        assert!(body(&frozen).contains(r#""messages":[{"role":"system","content":"S"},{"role":"user","content":"old"},{"role":"user","content":"hi"}]"#), "{}", body(&frozen));

        // The two refusals: the token limit, and a prompt the context cannot hold.
        let tight = Wire { token_limit: 10, ..wire() };
        assert_eq!(Ollama.body(&build(&m, &messages, &tools, &none, &tight)), Err(TOKEN_LIMIT.to_string()));
        let big = [Message::text(Role::User, &"x".repeat(70000))];
        let small = Model { context_window_tokens: 32768, ..model() };
        assert_eq!(Ollama.body(&build(&small, &big, &[], &none, &w)), Err(over_ctx(35001, 32768)));
        assert_eq!(Ollama.body(&build(&small, &big, &[], &none, &pinned)), Err(over_override(35001, 2048)));
    }

    #[test]
    fn ollama_num_ctx_follows_iterm2() {
        assert_eq!(num_ctx(5000, 4096, 32768, 0), 16384);
        assert_eq!(budget(5000, 4096, 32768, 0), Ok((16384, 4096)));
        assert_eq!(num_ctx(1000, 64, 32768, 0), 4096);
        assert_eq!(num_ctx(30000, 8192, 32768, 0), 32768, "capped at the window");
        assert_eq!(budget(30000, 8192, 32768, 0), Ok((32768, 2704)));
        assert_eq!(num_ctx(10, 10, 0, 0), 4096);
        assert_eq!(num_ctx(30000, 10, 0, 0), 32768, "an unknown window is 32768");
        assert_eq!(num_ctx(30000, 10, -1, 0), 32768);
        assert_eq!(num_ctx(10, 10, 32768, 2048), 2048);
        assert_eq!(num_ctx(100000, 10, 32768, 65536), 65536, "the override is sent as it is");
        assert_eq!(budget(10, 5000, 32768, 2048), Ok((2048, 1974)), "num_predict fits the room left");
        assert_eq!(budget(1900, 5000, 32768, 2048), Ok((2048, 84)));
        assert_eq!(
            budget(2000, 100, 32768, 2048),
            Err("AI token limit exceeded because the conversation reached its maximum length (prompt \u{2248}2000 tokens exceeds the ollamaNumCtx override of 2048)".to_string())
        );
        assert_eq!(budget(32700, 100, 32768, 0), Err("AI token limit exceeded because the conversation reached its maximum length (prompt \u{2248}32700 tokens needs more than num_ctx 32768)".to_string()));
        assert_eq!(over_override(2000, 2048), format!("{TOKEN_LIMIT} (prompt \u{2248}2000 tokens exceeds the ollamaNumCtx override of 2048)"));
        assert_eq!(over_ctx(32700, 32768), format!("{TOKEN_LIMIT} (prompt \u{2248}32700 tokens needs more than num_ctx 32768)"));
        assert_eq!(image_tokens(100, 100), 85);
        assert_eq!(image_tokens(1000, 1000), 1334);
        assert_eq!(image_tokens(10000, 10000), 12288);
        assert_eq!(image_tokens(0, 100), 85);
    }

    #[test]
    fn ollama_prompt_estimate_follows_iterm2() {
        let tools = [tool()];
        let decls = json!([{"type":"function","function":{"name":"f","description":"d","parameters":{"type":"object","properties":{},"required":[]}}}]).to_string();
        assert_eq!(prompt_tokens(&sh(), &tools, None), 1 + 2 + tokens(&decls), "text as bytes / 2 + 1 each, plus the tools");
        assert_eq!(prompt_tokens(&sh(), &[], None), 3);
        let joined = [Message::new(Some(Role::Assistant), Body::Multipart(vec![Body::Text("a".into()), Body::Text("b".into())]))];
        assert_eq!(prompt_tokens(&joined, &[], None), 2, "adjacent texts count as one string");
        let call = [Message::new(Some(Role::Assistant), Body::FunctionCall(FunctionCall::default(), None))];
        assert_eq!(prompt_tokens(&call, &[], None), 0, "a call has no content");

        // An image counts by its pixel area, not its base64 length (`LL:310-338`).
        let picture = png(1000, 1000);
        let seen = [Message::new(Some(Role::User), Body::Multipart(vec![Body::Text("look".into()), file("a.png", "image/png", &picture)]))];
        assert_eq!(prompt_tokens(&seen, &[], None), 1 + 3 + 1334);
        let unreadable = [Message::new(Some(Role::User), Body::Multipart(vec![Body::Text("look".into()), file("a.png", "image/png", &[0, 1, 2])]))];
        assert_eq!(prompt_tokens(&unreadable, &[], None), 1 + 3 + 85, "an unreadable image is the floor");

        // The frozen history counts too, its images by area (`LL:347-369`).
        let b64 = base64::engine::general_purpose::STANDARD.encode(&picture);
        let frozen = format!(r#"{{"role":"user","content":"x","images":["{b64}"]}},{{"role":"assistant","content":"y"}}"#);
        let rest = tokens(r#"{"role":"user","content":"x"}"#) + tokens(r#"{"role":"assistant","content":"y"}"#);
        assert_eq!(prompt_tokens(&[], &[], Some(frozen.as_bytes())), 1334 + rest);
        assert_eq!(prompt_tokens(&[], &[], Some(b"not json")), tokens("not json"), "unparseable: its bytes");
        assert_eq!(prompt_tokens(&sh(), &[], Some(b"")), 3, "an empty history counts nothing");
    }

    fn key() -> Secret {
        Secret::new("sk-ABCDEFGHIJKLMNOPQRSTUV".to_string())
    }

    fn turn_input() -> TurnInput {
        TurnInput {
            model: model(),
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

    fn lines(stream: &str) -> Vec<&[u8]> {
        stream.split_inclusive('\n').map(str::as_bytes).collect()
    }

    fn streamed(chunks: &[&[u8]]) -> Vec<Out> {
        let (mut r, _) = runner(Turn::new(turn_input()), vec![Reply::ok(chunks)]);
        assert_eq!(r.start(Some(&key())), [Out::WillSend]);
        r.poll()
    }

    #[test]
    fn ollama_stream_follows_iterm2() {
        let thinking = Out::Attachment(Attachment::Status { id: REASONING_ID.into(), inline: true, status: Status::ReasoningSummary("hmm".into()) });
        // The reasoning is surfaced again before the end, as for every provider (`T:965-972`).
        assert_eq!(streamed(&lines(OLLAMA_STREAM)), [thinking, Out::Delta("Hi".into()), Out::Reasoning("hmm".into()), Out::StreamEnd]);

        let p = |json: &str| Ollama.parse_event(json).expect("parses").expect("an event");
        assert!(p(r#"{"model":"m","created_at":"t","message":{"role":"assistant","content":""},"done":false}"#).ignore, "nothing in it");
        assert!(!p(r#"{"model":"m","created_at":"t","message":{"role":"assistant","content":"x"},"done":false}"#).ignore);
        let done = p(r#"{"model":"m","created_at":"t","message":{"role":"assistant","content":"x","tool_calls":[{"function":{"name":"f","arguments":{}}}]},"done":true}"#);
        assert!(done.messages.is_empty() && !done.ignore, "the done line gives nothing, even a repeated call");
        let preamble = p(r#"{"model":"m","created_at":"t","message":{"role":"assistant","content":"pre","tool_calls":[{"function":{"name":"f"}}]},"done":false}"#).messages;
        let bare = FunctionCall { name: Some("f".into()), arguments: Some("{}".into()), id: None, thought_signature: None };
        assert_eq!(preamble.iter().map(|m| m.body.clone()).collect::<Vec<_>>(), [Body::Text("pre".into()), Body::FunctionCall(bare, None)], "text before a call; no arguments are {{}}");
        assert!(Ollama.parse_event(r#"{"error":"model not found"}"#).is_err());
        assert!(Ollama.parse_event("nope").is_err());

        // A call runs with its arguments as JSON; the done line's repeat does not run it twice.
        let call_stream = concat!(
            r#"{"model":"m","created_at":"t","message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"f","arguments":{"a":1}}}]},"done":false}"#,
            "\n",
            r#"{"model":"m","created_at":"t","message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"f","arguments":{"a":1}}}]},"done":true}"#,
            "\n",
        );
        let (mut r, rec) = runner(Turn::new(turn_input()), vec![Reply::ok(&lines(call_stream)), Reply::ok(&[])]);
        assert_eq!(r.start(Some(&key())), [Out::WillSend]);
        let call = FunctionCall { name: Some("f".into()), arguments: Some("{\"a\":1}".into()), id: None, thought_signature: None };
        assert_eq!(r.poll(), [Out::Invoke { call, id: None }]);
        assert_eq!(r.tool_done(ToolResult::Ok("42".into())), [Out::WillSend]);
        let sent: Value = serde_json::from_slice(&rec.requests()[1].body).expect("JSON");
        assert_eq!(sent["messages"][2], json!({"role":"assistant","tool_calls":[{"type":"function","function":{"name":"f","arguments":{"a":1}}}]}));
        assert_eq!(sent["messages"][3], json!({"role":"tool","tool_name":"f","content":"42"}));
        assert!(rec.requests().iter().all(|q| q.headers.iter().all(|(n, _)| !n.eq_ignore_ascii_case("authorization"))), "no Authorization header");
    }

    #[test]
    fn ollama_stream_any_chunking() {
        let expected = streamed(&lines(OLLAMA_STREAM));
        let whole = OLLAMA_STREAM.as_bytes();
        assert!(same_outcome(&streamed(&[whole]), &expected), "one chunk");
        for at in 1..whole.len() {
            let out = streamed(&[&whole[..at], &whole[at..]]);
            assert!(same_outcome(&out, &expected), "split at {at}: {out:?}");
        }
    }

    fn answered(body: &str) -> (Vec<Out>, Option<i64>) {
        let (mut r, _) = runner(Turn::new(TurnInput { stream: false, ..turn_input() }), vec![Reply::ok(&[body.as_bytes()])]);
        assert_eq!(r.start(Some(&key())), [Out::WillSend]);
        let out = r.poll();
        (out, r.turn().prompt_tokens())
    }

    #[test]
    fn ollama_parse_follows_iterm2() {
        let whole = |message: &str| format!(r#"{{"model":"m","created_at":"t","message":{message},"done":true}}"#);
        assert_eq!(answered(&whole(r#"{"role":"assistant","content":"","thinking":"t"}"#)), (vec![Out::Choice("t".into())], None), "thinking stands in for no answer");
        assert_eq!(answered(&whole(r#"{"role":"assistant","content":"Hi"}"#)), (vec![Out::Choice("Hi".into())], None));
        assert_eq!(answered(&whole(r#"{"role":"assistant","content":"Hi","thinking":"t"}"#)).0, [Out::Reasoning("t".into()), Out::Choice("Hi".into())]);
        let call = FunctionCall { name: Some("f".into()), arguments: Some("{\"a\":1}".into()), id: None, thought_signature: None };
        assert_eq!(answered(&whole(r#"{"role":"assistant","content":"","tool_calls":[{"function":{"name":"f","arguments":{"a":1}}}]}"#)).0, [Out::Invoke { call: call.clone(), id: None }]);
        let p = Ollama.parse(whole(r#"{"role":"assistant","content":"pre","tool_calls":[{"function":{"name":"f","arguments":{"a":1}}}]}"#).as_bytes()).unwrap().unwrap();
        assert_eq!(p.messages[0].body, Body::Multipart(vec![Body::Text("pre".into()), Body::FunctionCall(call, None)]), "a preamble and a call");
        assert_eq!(p.prompt_tokens, None);
        assert!(Ollama.parse(br#"{"error":"model not found"}"#).is_err());
        assert!(Ollama.parse(b"nope").is_err());
    }

    #[test]
    fn ollama_think_needs_configurable_thinking() {
        // The turn sends `think` only to a model that has the switch (`T:648-704`).
        let body_of = |m: Model| {
            let (mut r, rec) = runner(Turn::new(TurnInput { model: m, should_think: Some(true), ..turn_input() }), vec![Reply::ok(&[])]);
            r.start(Some(&key()));
            String::from_utf8(rec.requests()[0].body.clone()).unwrap()
        };
        assert!(!body_of(model()).contains("think"));
        let mut thinking = model();
        thinking.features.insert(Feature::ConfigurableThinking);
        assert!(body_of(thinking).contains(r#""think":true"#));
    }
}

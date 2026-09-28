//! Anthropic's Messages API: the request body (sorted keys, for Anthropic's prompt cache), the
//! conversion of a conversation into its messages with iTerm2's three repairs, the rolling cache
//! breakpoint, and the parsers of streamed events and whole answers. Pure; ports
//! `AITerm/CompletionsAnthropic.swift` (`AN:43-183`, `AN:533-790`, `AN:792-1188`).

use crate::ai_json::{sorted, splice_at_key};
use crate::ai_model::mime_is_textual;
use crate::ai_msg::{Attachment, Body, CallId, FunctionCall, Message, Role};
use crate::ai_turn::{max_tokens_for, Build, Dialect, Parsed, TOKEN_LIMIT};
use base64::Engine;
use serde_json::{json, Map, Value};

pub struct Anthropic;

/// The ephemeral prompt-cache marker (`AnthropicCacheControl.ephemeral`).
fn ephemeral() -> Value {
    json!({"type": "ephemeral"})
}

fn text_block(text: &str) -> Value {
    json!({"text": text, "type": "text"})
}

fn base64_block(kind: &str, mime: &str, content: &[u8]) -> Value {
    let data = base64::engine::general_purpose::STANDARD.encode(content);
    json!({"source": {"data": data, "media_type": mime, "type": "base64"}, "type": kind})
}

fn tool_use(call: &FunctionCall, id: &str) -> Value {
    json!({"id": id, "input": arguments(call.arguments.as_deref()), "name": call.name.as_deref().unwrap_or(""), "type": "tool_use"})
}

fn tool_result(id: &str, output: &str) -> Value {
    json!({"content": output, "tool_use_id": id, "type": "tool_result"})
}

/// `parseArguments` (`AN:524-531`): the arguments as a JSON object, else an empty one.
fn arguments(arguments: Option<&str>) -> Value {
    match arguments.and_then(|a| serde_json::from_str::<Value>(a).ok()) {
        Some(v @ Value::Object(_)) => v,
        _ => json!({}),
    }
}

/// A file: textual types (SVG included) as text, other images and PDFs as base64 blocks, and
/// anything else as lossy text (the attachment rules refuse those upstream). `Err` is text.
fn file_block(mime: &str, content: &[u8]) -> Result<Value, String> {
    if mime_is_textual(mime) {
        Err(String::from_utf8_lossy(content).into_owned())
    } else if mime.starts_with("image/") {
        Ok(base64_block("image", mime, content))
    } else if mime == "application/pdf" {
        Ok(base64_block("document", mime, content))
    } else {
        Err(String::from_utf8_lossy(content).into_owned())
    }
}

fn file_id_text(name: &str) -> String {
    format!("A file named {name} (content unavailable)")
}

fn message(role: &str, content: Value) -> Value {
    json!({"content": content, "role": role})
}

/// `AnthropicMessage.init?(_:)` (`AN:43-183`): one message, or None for a system, roleless or
/// empty one. `content` is a string or an array of blocks.
fn converted(m: &Message) -> Option<Value> {
    let role = match (&m.body, m.role?) {
        (Body::FunctionOutput { id: Some(_), .. }, _) => "user",
        (_, Role::System) => return None,
        (_, Role::User | Role::Function) => "user",
        (_, Role::Assistant) => "assistant",
    };
    let content = match &m.body {
        Body::Uninitialized => return None,
        Body::Text(text) => json!(text),
        Body::Attachment(Attachment::Code(code)) => json!(code),
        Body::Attachment(Attachment::Status { status, .. }) => json!(status.display()),
        Body::Attachment(Attachment::File { mime, content, .. }) => match file_block(mime, content) {
            Ok(block) => json!([block]),
            Err(text) => json!(text),
        },
        Body::Attachment(Attachment::FileId { name, .. }) => json!(file_id_text(name)),
        Body::FunctionCall(call, _) => match &call.id {
            Some(id) => json!([tool_use(call, id)]),
            None => json!(format!("Function call: {}", call.name.as_deref().unwrap_or("(nil)"))),
        },
        Body::FunctionOutput { name, output, id } => match id {
            Some(id) => json!([tool_result(&id.call_id, output)]),
            None => json!(format!("Function {name} output: {output}")),
        },
        Body::Multipart(parts) => Value::Array(parts.iter().filter_map(part_block).collect()),
    };
    Some(message(role, content))
}

/// One part of a multipart message (`AN:125-181`).
fn part_block(part: &Body) -> Option<Value> {
    match part {
        Body::Uninitialized | Body::Multipart(_) => None,
        Body::Text(text) => Some(text_block(text)),
        Body::FunctionCall(call, _) => call.id.as_deref().map(|id| tool_use(call, id)),
        Body::FunctionOutput { output, id, .. } => Some(match id {
            Some(id) => tool_result(&id.call_id, output),
            None => text_block(output),
        }),
        Body::Attachment(Attachment::Code(code)) => Some(text_block(code)),
        Body::Attachment(Attachment::Status { .. }) => None,
        Body::Attachment(Attachment::File { mime, content, .. }) => Some(file_block(mime, content).unwrap_or_else(|text| text_block(&text))),
        Body::Attachment(Attachment::FileId { name, .. }) => Some(text_block(&file_id_text(name))),
    }
}

fn is_role(m: &Value, role: &str) -> bool {
    m["role"] == role
}

/// The conversation as Anthropic messages (`convertMessages`, `AN:859-939`), with iTerm2's
/// three repairs: a tool's output answers the latest pending call, even written as text; a
/// result is moved up to follow its call; consecutive assistant messages merge.
pub fn convert(messages: &[Message]) -> Vec<Value> {
    let mut out = Vec::new();
    let mut pending: Vec<String> = Vec::new();
    for m in messages {
        if m.role == Some(Role::System) {
            continue;
        }
        if let (Some(Role::Function), Body::FunctionOutput { output, .. }) = (m.role, &m.body) {
            if let Some(id) = pending.pop() {
                out.push(message("user", json!([tool_result(&id, output)])));
                continue;
            }
        }
        let Some(converted) = converted(m) else {
            continue;
        };
        if is_role(&converted, "assistant") {
            for block in converted["content"].as_array().into_iter().flatten() {
                if block["type"] == "tool_use" {
                    pending.push(block["id"].as_str().unwrap_or_default().to_string());
                }
            }
        }
        if is_role(&converted, "user") {
            if let Some(text) = converted["content"].as_str() {
                // A tool's output written as text.
                if text.starts_with("Function ") {
                    if let (Some(at), Some(id)) = (text.find(" output: "), pending.last()) {
                        out.push(message("user", json!([tool_result(id, &text[at + " output: ".len()..])])));
                        pending.pop();
                        continue;
                    }
                }
                pending.clear();
            }
        }
        out.push(converted);
    }
    coalesce_assistants(tool_results_follow_calls(out))
}

/// `coalesceConsecutiveAssistantMessages` (`AN:941-954`): the roles must alternate.
fn coalesce_assistants(messages: Vec<Value>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for m in messages {
        match out.last_mut() {
            Some(last) if is_role(last, "assistant") && is_role(&m, "assistant") => {
                let mut merged = blocks(&last["content"]);
                merged.extend(blocks(&m["content"]));
                last["content"] = Value::Array(merged);
            }
            _ => out.push(m),
        }
    }
    out
}

fn blocks(content: &Value) -> Vec<Value> {
    match content {
        Value::String(s) => vec![text_block(s)],
        Value::Array(blocks) => blocks.clone(),
        _ => Vec::new(),
    }
}

/// `enforceToolUseAdjacency` (`AN:974-1027`): each assistant message's tool_use blocks are
/// followed at once by one user message holding their tool_result blocks, gathered from later
/// user messages; the messages in between keep their order after it.
fn tool_results_follow_calls(messages: Vec<Value>) -> Vec<Value> {
    let mut input = messages;
    let mut out = Vec::new();
    let mut i = 0;
    while i < input.len() {
        let current = input[i].clone();
        i += 1;
        let mut ids: Vec<String> = if is_role(&current, "assistant") {
            current["content"].as_array().into_iter().flatten().filter(|b| b["type"] == "tool_use").map(|b| b["id"].as_str().unwrap_or_default().to_string()).collect()
        } else {
            Vec::new()
        };
        out.push(current);
        let mut results = Vec::new();
        let mut k = i;
        while k < input.len() && !ids.is_empty() {
            let candidate = &input[k];
            let Some(blocks) = candidate["content"].as_array().filter(|_| is_role(candidate, "user")) else {
                k += 1;
                continue;
            };
            let (mut consumed, mut leftover) = (Vec::new(), Vec::new());
            for block in blocks {
                let matched = (block["type"] == "tool_result").then(|| ids.iter().position(|id| block["tool_use_id"] == id.as_str())).flatten();
                match matched {
                    Some(at) => {
                        ids.remove(at);
                        consumed.push(block.clone());
                    }
                    None => leftover.push(block.clone()),
                }
            }
            if consumed.is_empty() {
                k += 1;
                continue;
            }
            results.extend(consumed);
            if leftover.is_empty() {
                input.remove(k);
            } else {
                input[k] = message("user", Value::Array(leftover));
                k += 1;
            }
        }
        if !results.is_empty() {
            out.push(message("user", Value::Array(results)));
        }
    }
    out
}

/// The rolling history breakpoint (`markLastMessageForCaching`, `AN:1058-1075`): the last block
/// of the last message is marked, a string content first becoming one text block, so the whole
/// conversation up to it is cached for the next turn.
pub fn mark_cache(messages: &mut Vec<Value>) {
    let Some(last) = messages.last_mut() else {
        return;
    };
    match &mut last["content"] {
        Value::String(text) => {
            let mut block = text_block(text);
            block["cache_control"] = ephemeral();
            last["content"] = json!([block]);
        }
        Value::Array(blocks) => {
            if let Some(block) = blocks.last_mut() {
                block["cache_control"] = ephemeral();
            }
        }
        _ => {}
    }
}

/// `input_tokens` counts only the uncached input; the prompt is the sum (`AN:588-598`).
fn prompt_tokens(usage: &Value) -> Option<i64> {
    let input = usage["input_tokens"].as_i64()?;
    Some(input + usage["cache_read_input_tokens"].as_i64().unwrap_or(0) + usage["cache_creation_input_tokens"].as_i64().unwrap_or(0))
}

fn string(v: &Value) -> Option<String> {
    v.as_str().map(str::to_string)
}

/// The reason of an error event that gives no message.
const UNKNOWN_ERROR: &str = "Unknown error";

impl Dialect for Anthropic {
    /// `AnthropicRequestBuilder.body()` (`AN:1077-1188`).
    fn body(&self, b: &Build) -> Result<Vec<u8>, String> {
        let mut messages = convert(b.messages);
        mark_cache(&mut messages);
        // Per-request context goes after the breakpoint, unmarked, so the cached prefix holds.
        if let Some(v) = b.volatile.filter(|v| !v.is_empty()) {
            messages.push(message("user", json!([text_block(v)])));
        }
        let max_tokens = max_tokens_for(b);
        if max_tokens < 2 {
            return Err(TOKEN_LIMIT.to_string());
        }
        let mut body = Map::new();
        body.insert("model".into(), json!(b.model.name));
        body.insert("messages".into(), Value::Array(messages));
        body.insert("max_tokens".into(), json!(max_tokens));
        // Breakpoints on the system block and the last tool (the tools-only fallback segment).
        if let Some(system) = b.messages.iter().filter(|m| m.role == Some(Role::System)).find_map(Message::content) {
            let mut block = text_block(&system);
            block["cache_control"] = ephemeral();
            body.insert("system".into(), json!([block]));
        }
        if b.model.supports_temperature {
            body.insert("temperature".into(), json!(0));
        }
        if b.stream {
            body.insert("stream".into(), json!(true));
        }
        if !b.tools.is_empty() {
            let mut tools: Vec<Value> = b.tools.iter().map(|t| json!({"description": t.description, "input_schema": t.parameters, "name": t.name})).collect();
            if let Some(last) = tools.last_mut() {
                last["cache_control"] = ephemeral();
            }
            body.insert("tools".into(), Value::Array(tools));
            body.insert("tool_choice".into(), json!({"disable_parallel_tool_use": true, "type": "auto"}));
        }
        // Sorted keys keep the bytes, and so the prompt cache prefix, stable.
        let bytes = sorted(&Value::Object(body)).into_bytes();
        splice_at_key(&bytes, "messages", 0, b.frozen)
    }

    /// A whole answer (`AN:533-653`): its text and tool_use blocks collapse into one assistant
    /// message, a multipart when there are several.
    fn parse(&self, body: &[u8]) -> Result<Option<Parsed>, String> {
        let v: Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
        if !v.is_object() {
            return Err("the response is not an object".to_string());
        }
        let content = v["content"].as_array().map(Vec::as_slice).unwrap_or_default();
        let mut bodies = Vec::new();
        for block in content {
            match block["type"].as_str().unwrap_or_default() {
                "text" => {
                    if let Some(text) = block["text"].as_str().filter(|t| !t.is_empty()) {
                        bodies.push(Body::Text(text.to_string()));
                    }
                }
                "tool_use" => {
                    if let (Some(name), Some(id), Some(input)) = (block["name"].as_str(), block["id"].as_str(), block["input"].as_object()) {
                        let call = FunctionCall { name: Some(name.to_string()), arguments: Some(Value::Object(input.clone()).to_string()), id: Some(id.to_string()), thought_signature: None };
                        bodies.push(Body::FunctionCall(call, Some(CallId { call_id: id.to_string(), item_id: id.to_string() })));
                    }
                }
                _ => {}
            }
        }
        let body = match bodies.len() {
            0 => Body::Text(content.iter().filter_map(|b| b["text"].as_str()).collect()),
            1 => bodies.remove(0),
            _ => Body::Multipart(bodies),
        };
        let mut m = Message::new(Some(Role::Assistant), body);
        m.response_id = string(&v["id"]);
        Ok(Some(Parsed { messages: vec![m], response_id: None, prompt_tokens: prompt_tokens(&v["usage"]), ignore: false, error: None }))
    }

    /// One streamed event (`AN:655-790`): usage from `message_start`, a call from a tool_use
    /// block's start, text and argument fragments from deltas (arguments arrive as text and
    /// merge into the call); an `error` event fails the turn (ruling 12b-3; iTerm2 drops it).
    fn parse_event(&self, json: &str) -> Result<Option<Parsed>, String> {
        let event: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let kind = event["type"].as_str().ok_or("the event has no type")?;
        let mut p = Parsed { prompt_tokens: prompt_tokens(&event["message"]["usage"]), ..Parsed::default() };
        if kind == "error" {
            p.error = Some(event["error"]["message"].as_str().unwrap_or(UNKNOWN_ERROR).to_string());
            return Ok(Some(p));
        }
        let block = &event["content_block"];
        if block["type"] == "tool_use" {
            // The start's `input` is a placeholder; the arguments stream as deltas.
            if let (Some(name), Some(id)) = (block["name"].as_str(), block["id"].as_str()) {
                let call = FunctionCall { name: Some(name.to_string()), arguments: Some(String::new()), id: Some(id.to_string()), thought_signature: None };
                let ids = CallId { call_id: id.to_string(), item_id: id.to_string() };
                p.messages.push(Message::new(Some(Role::Assistant), Body::FunctionCall(call, Some(ids))));
            }
            return Ok(Some(p));
        }
        let delta = &event["delta"];
        let text = block["text"].as_str().or_else(|| delta["text"].as_str()).or_else(|| delta["partial_json"].as_str());
        if let Some(text) = text.filter(|t| !t.is_empty()) {
            p.messages.push(Message::text(Role::Assistant, text));
        }
        Ok(Some(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_catalog::Model;
    use crate::ai_json::sorted;
    use crate::ai_model::fixture;
    use crate::ai_msg::{Attachment, Body, CallId, FunctionCall, Role};
    use crate::ai_settings::Api;
    use crate::ai_transport::{Options, Proxy, Recorded, Reply};
    use crate::ai_turn::{headers, same_outcome, Hosted, Out, ToolDecl, ToolResult, Turn, TurnInput, Wire, TOKEN_LIMIT};
    use crate::pwm::Secret;
    use serde_json::json;
    use std::sync::Arc;
    use std::time::Duration;

    const ANTHROPIC_STREAM: &str = concat!(
        "event: message_start\n",
        r#"data: {"type":"message_start","message":{"id":"m","content":[],"usage":{"input_tokens":10,"cache_read_input_tokens":2,"cache_creation_input_tokens":3}}}"#,
        "\n\n",
        "event: content_block_start\n",
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        "\n\n",
        "event: ping\n",
        r#"data: {"type":"ping"}"#,
        "\n\n",
        "event: content_block_delta\n",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#,
        "\n\n",
        "event: content_block_stop\n",
        r#"data: {"type":"content_block_stop","index":0}"#,
        "\n\n",
        "event: content_block_start\n",
        r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"f","input":{}}}"#,
        "\n\n",
        "event: content_block_delta\n",
        r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"a\":"}}"#,
        "\n\n",
        "event: content_block_delta\n",
        r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"1}"}}"#,
        "\n\n",
        "event: message_delta\n",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":5}}"#,
        "\n\n",
        "event: message_stop\n",
        r#"data: {"type":"message_stop"}"#,
        "\n\n",
    );

    fn model() -> Model {
        fixture("claude-test", "https://api.anthropic.com/v1/messages", Api::Anthropic)
    }

    fn wire() -> Wire {
        Wire { token_limit: 128000, response_limit: 8192, zdr: false, ollama_num_ctx: 0, ollama_keep_alive: "30m".into() }
    }

    fn tool_named(name: &str) -> ToolDecl {
        ToolDecl { name: name.into(), description: "d".into(), parameters: json!({"type": "object", "properties": {}, "required": []}) }
    }

    fn tool() -> ToolDecl {
        tool_named("f")
    }

    fn build<'a>(m: &'a Model, messages: &'a [Message], tools: &'a [ToolDecl], hosted: &'a Hosted, wire: &'a Wire) -> Build<'a> {
        Build {
            model: m,
            key: "sk-ant-a",
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
        String::from_utf8(Anthropic.body(b).expect("body builds")).expect("UTF-8")
    }

    fn sh() -> Vec<Message> {
        vec![Message::text(Role::System, "S"), Message::text(Role::User, "hi")]
    }

    const HI_MARKED: &str = r#"{"content":[{"cache_control":{"type":"ephemeral"},"text":"hi","type":"text"}],"role":"user"}"#;
    const SYSTEM_S: &str = r#""system":[{"cache_control":{"type":"ephemeral"},"text":"S","type":"text"}]"#;

    #[test]
    fn anthropic_body_follows_iterm2() {
        let (m, w, none) = (model(), wire(), Hosted::default());
        let messages = sh();
        let tools = [tool()];
        let b = build(&m, &messages, &tools, &none, &w);
        assert_eq!(
            body(&b),
            r#"{"max_tokens":8192,"messages":[{"content":[{"cache_control":{"type":"ephemeral"},"text":"hi","type":"text"}],"role":"user"}],"model":"claude-test","stream":true,"system":[{"cache_control":{"type":"ephemeral"},"text":"S","type":"text"}],"temperature":0,"tool_choice":{"disable_parallel_tool_use":true,"type":"auto"},"tools":[{"cache_control":{"type":"ephemeral"},"description":"d","input_schema":{"properties":{},"required":[],"type":"object"},"name":"f"}]}"#
        );
        let h = headers(&b);
        let want = [("x-api-key", "sk-ant-a"), ("anthropic-version", "2023-06-01"), ("Content-Type", "application/json")];
        assert!(h.len() == 3 && h.iter().zip(want).all(|((n, v), (wn, wv))| n == wn && v == wv), "x-api-key, anthropic-version and Content-Type headers");

        let mut whole = build(&m, &messages, &tools, &none, &w);
        whole.stream = false;
        assert!(!body(&whole).contains(r#""stream""#), "non-streaming omits stream");
        let cold = Model { supports_temperature: false, ..model() };
        assert!(!body(&build(&cold, &messages, &tools, &none, &w)).contains("temperature"), "no temperature");
        assert_eq!(
            body(&build(&m, &messages, &[], &none, &w)),
            format!(r#"{{"max_tokens":8192,"messages":[{HI_MARKED}],"model":"claude-test","stream":true,{SYSTEM_S},"temperature":0}}"#),
            "no tools: no tools or tool_choice"
        );
        let no_system = [Message::text(Role::User, "hi")];
        assert_eq!(body(&build(&m, &no_system, &[], &none, &w)), format!(r#"{{"max_tokens":8192,"messages":[{HI_MARKED}],"model":"claude-test","stream":true,"temperature":0}}"#));
        let two_systems = [Message::text(Role::System, "S"), Message::text(Role::System, "T"), Message::text(Role::User, "hi")];
        let two = body(&build(&m, &two_systems, &[], &none, &w));
        assert!(two.contains(SYSTEM_S) && !two.contains(r#""text":"T""#), "only the first system message: {two}");

        let two_tools = [tool_named("f"), tool_named("g")];
        let marked = body(&build(&m, &messages, &two_tools, &none, &w));
        let schema = r#""input_schema":{"properties":{},"required":[],"type":"object"}"#;
        assert!(
            marked.contains(&format!(r#""tools":[{{"description":"d",{schema},"name":"f"}},{{"cache_control":{{"type":"ephemeral"}},"description":"d",{schema},"name":"g"}}]"#)),
            "only the last tool is marked: {marked}"
        );

        let mut v = build(&m, &messages, &[], &none, &w);
        v.volatile = Some("V");
        assert!(body(&v).contains(&format!(r#""messages":[{HI_MARKED},{{"content":[{{"text":"V","type":"text"}}],"role":"user"}}],"#)), "{}", body(&v));
        v.volatile = Some("");
        assert!(body(&v).contains(&format!(r#""messages":[{HI_MARKED}],"#)), "an empty volatile text adds nothing");

        let tight = Wire { token_limit: 10, ..wire() };
        assert_eq!(Anthropic.body(&build(&m, &messages, &tools, &none, &tight)), Err(TOKEN_LIMIT.to_string()));

        let mut frozen = build(&m, &messages, &[], &none, &w);
        frozen.frozen = Some(br#"{"content":"old","role":"user"}"#);
        assert!(body(&frozen).starts_with(&format!(r#"{{"max_tokens":8192,"messages":[{{"content":"old","role":"user"}},{HI_MARKED}],"#)), "{}", body(&frozen));
    }

    fn call(arguments: &str, id: Option<&str>) -> Message {
        let call = FunctionCall { name: Some("f".into()), arguments: Some(arguments.into()), id: id.map(str::to_string), thought_signature: None };
        let ids = id.map(|i| CallId { call_id: i.into(), item_id: i.into() });
        Message::new(Some(Role::Assistant), Body::FunctionCall(call, ids))
    }

    fn output(id: Option<&str>) -> Message {
        let ids = id.map(|i| CallId { call_id: i.into(), item_id: i.into() });
        Message::new(Some(Role::Function), Body::FunctionOutput { name: "f".into(), output: "42".into(), id: ids })
    }

    fn file(name: &str, mime: &str, content: &[u8]) -> Body {
        Body::Attachment(Attachment::File { name: name.into(), mime: mime.into(), content: content.to_vec() })
    }

    fn file_id() -> Body {
        Body::Attachment(Attachment::FileId { id: "file-1".into(), name: "x".into() })
    }

    fn user(body: Body) -> Message {
        Message::new(Some(Role::User), body)
    }

    fn converted(messages: &[Message]) -> Vec<String> {
        convert(messages).iter().map(sorted).collect()
    }

    const TOOL_USE: &str = r#"{"content":[{"id":"toolu_1","input":{"a":1},"name":"f","type":"tool_use"}],"role":"assistant"}"#;
    const TOOL_RESULT: &str = r#"{"content":[{"content":"42","tool_use_id":"toolu_1","type":"tool_result"}],"role":"user"}"#;
    const IMAGE: &str = r#"{"source":{"data":"AAEC","media_type":"image/png","type":"base64"},"type":"image"}"#;
    const PDF: &str = r#"{"source":{"data":"AAEC","media_type":"application/pdf","type":"base64"},"type":"document"}"#;

    #[test]
    fn anthropic_messages_follow_iterm2() {
        assert_eq!(converted(&[call("{\"a\":1}", Some("toolu_1"))]), [TOOL_USE]);
        assert!(converted(&[call("not json", Some("toolu_1"))])[0].contains(r#""input":{}"#), "arguments that are not JSON");
        assert!(converted(&[call("[1]", Some("toolu_1"))])[0].contains(r#""input":{}"#), "arguments that are not an object");
        assert_eq!(converted(&[output(Some("toolu_1"))]), [TOOL_RESULT]);

        assert_eq!(converted(&[user(file("p.png", "image/png", &[0, 1, 2]))]), [format!(r#"{{"content":[{IMAGE}],"role":"user"}}"#)]);
        assert_eq!(converted(&[user(file("d.pdf", "application/pdf", &[0, 1, 2]))]), [format!(r#"{{"content":[{PDF}],"role":"user"}}"#)]);
        assert_eq!(converted(&[user(file("a.txt", "text/plain", b"hello"))]), [r#"{"content":"hello","role":"user"}"#]);
        assert_eq!(converted(&[user(file("i.svg", "image/svg+xml", b"<svg/>"))]), [r#"{"content":"<svg/>","role":"user"}"#], "SVG is text");
        assert_eq!(converted(&[user(file_id())]), [r#"{"content":"A file named x (content unavailable)","role":"user"}"#]);
        assert_eq!(converted(&[call("{}", None)]), [r#"{"content":"Function call: f","role":"assistant"}"#]);
        assert_eq!(converted(&[output(None)]), [r#"{"content":"Function f output: 42","role":"user"}"#]);
        assert_eq!(converted(&[user(Body::Attachment(Attachment::Code("x = 1".into())))]), [r#"{"content":"x = 1","role":"user"}"#]);

        // The same encodings as parts of a multipart message.
        let parts = Body::Multipart(vec![
            Body::Text("look".into()),
            file("p.png", "image/png", &[0, 1, 2]),
            file("d.pdf", "application/pdf", &[0, 1, 2]),
            file("a.txt", "text/plain", b"hello"),
            file_id(),
            Body::Attachment(Attachment::Code("x = 1".into())),
            Body::Uninitialized,
        ]);
        assert_eq!(
            converted(&[user(parts.clone())]),
            [format!(
                r#"{{"content":[{{"text":"look","type":"text"}},{IMAGE},{PDF},{{"text":"hello","type":"text"}},{{"text":"A file named x (content unavailable)","type":"text"}},{{"text":"x = 1","type":"text"}}],"role":"user"}}"#
            )]
        );
        assert!(converted(&[Message::text(Role::System, "S"), Message::new(None, Body::Text("x".into())), user(Body::Uninitialized)]).is_empty(), "system, roleless and empty messages are skipped");

        // The breakpoint goes on the last block of the last message only.
        let mut marked = convert(&[Message::text(Role::User, "a"), user(parts)]);
        mark_cache(&mut marked);
        let last = marked[1]["content"].as_array().unwrap();
        assert_eq!(last.last().unwrap()["cache_control"], json!({"type": "ephemeral"}));
        assert!(last[..last.len() - 1].iter().all(|b| b.get("cache_control").is_none()));
        assert_eq!(marked[0], json!({"content": "a", "role": "user"}));
        let mut none: Vec<Value> = Vec::new();
        mark_cache(&mut none);
        assert!(none.is_empty());
    }

    #[test]
    fn anthropic_repairs_follow_iterm2() {
        let use_ = || call("{\"a\":1}", Some("toolu_1"));
        let hello = r#"{"content":"hello","role":"user"}"#;
        // A tool result moves up to follow its call.
        assert_eq!(converted(&[use_(), Message::text(Role::User, "hello"), output(Some("toolu_1"))]), [TOOL_USE, TOOL_RESULT, hello]);
        // Consecutive assistant messages merge into one block array.
        assert_eq!(
            converted(&[Message::text(Role::Assistant, "a"), Message::text(Role::Assistant, "b")]),
            [r#"{"content":[{"text":"a","type":"text"},{"text":"b","type":"text"}],"role":"assistant"}"#]
        );
        // A tool output written as text answers the pending call.
        assert_eq!(converted(&[use_(), Message::text(Role::User, "Function f output: 42")]), [TOOL_USE, TOOL_RESULT]);
        // A plain user string clears the pending call.
        assert_eq!(
            converted(&[use_(), Message::text(Role::User, "hello"), Message::text(Role::User, "Function f output: 42")]),
            [TOOL_USE, hello, r#"{"content":"Function f output: 42","role":"user"}"#]
        );
    }

    fn key() -> Secret {
        Secret::new("sk-ant-a".to_string())
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

    fn streamed(chunks: &[&[u8]]) -> Vec<Out> {
        let (mut r, _) = runner(Turn::new(turn_input()), vec![Reply::ok(chunks)]);
        assert_eq!(r.start(Some(&key())), [Out::WillSend]);
        r.poll()
    }

    fn events(stream: &str) -> Vec<&[u8]> {
        stream.split_inclusive("\n\n").map(str::as_bytes).collect()
    }

    fn invoke() -> Out {
        Out::Invoke {
            call: FunctionCall { name: Some("f".into()), arguments: Some("{\"a\":1}".into()), id: Some("toolu_1".into()), thought_signature: None },
            id: Some(CallId { call_id: "toolu_1".into(), item_id: "toolu_1".into() }),
        }
    }

    #[test]
    fn anthropic_stream_follows_iterm2() {
        let (mut r, rec) = runner(Turn::new(turn_input()), vec![Reply::ok(&events(ANTHROPIC_STREAM)), Reply::ok(&[])]);
        assert_eq!(r.start(Some(&key())), [Out::WillSend]);
        assert_eq!(r.poll(), [Out::Delta("Hi".into()), invoke()]);
        assert_eq!(r.turn().prompt_tokens(), Some(15), "input + cache read + cache creation");

        // The call and its result go back as a tool_use and its tool_result.
        assert_eq!(r.tool_done(ToolResult::Ok("42".into())), [Out::WillSend]);
        let sent = String::from_utf8(rec.requests()[1].body.clone()).unwrap();
        let replay = concat!(
            r#""messages":[{"content":"hi","role":"user"},"#,
            r#"{"content":[{"id":"toolu_1","input":{"a":1},"name":"f","type":"tool_use"}],"role":"assistant"},"#,
            r#"{"content":[{"cache_control":{"type":"ephemeral"},"content":"42","tool_use_id":"toolu_1","type":"tool_result"}],"role":"user"}],"#,
        );
        assert!(sent.contains(replay), "{sent}");

        for quiet in [
            r#"{"type":"ping"}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":5}}"#,
            r#"{"type":"message_stop"}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        ] {
            let p = Anthropic.parse_event(quiet).expect("parses").expect("does not end the stream");
            assert!(p.messages.is_empty() && p.prompt_tokens.is_none() && p.error.is_none(), "{quiet}");
        }
        let start = Anthropic.parse_event(r#"{"type":"message_start","message":{"id":"m","content":[{"type":"text","text":"x"}],"usage":{"input_tokens":4}}}"#).unwrap().unwrap();
        assert_eq!((start.messages.len(), start.prompt_tokens), (0, Some(4)), "usage is kept whatever the content holds");
        assert!(Anthropic.parse_event("not json").is_err(), "an undecodable event drains");
    }

    const ERROR_EVENT: &str = concat!("event: error\n", r#"data: {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#, "\n\n");
    const LATER: &str = concat!("event: content_block_delta\n", r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"later"}}"#, "\n\n");

    #[test]
    fn anthropic_error_event_surfaces() {
        let head: Vec<&str> = ANTHROPIC_STREAM.split_inclusive("\n\n").take(5).collect();
        let whole = format!("{}{ERROR_EVENT}{LATER}", head.concat());
        let expected = [
            Out::Delta("Hi".into()),
            Out::Delta("\n\n**Request failed**\n\n    Error from Anthropic: Overloaded".into()),
            Out::Failed("Error from Anthropic: Overloaded".into()),
        ];
        let (mut r, _) = runner(Turn::new(turn_input()), vec![Reply::ok(&events(&whole))]);
        assert_eq!(r.start(Some(&key())), [Out::WillSend]);
        assert_eq!(r.poll(), expected, "later events are ignored");
        assert!(r.turn().is_idle());
        // Text still waiting in the same chunk streams before the failure.
        assert_eq!(streamed(&[whole.as_bytes()]), expected);
        // A call the error cuts short does not run.
        let cut: Vec<&str> = ANTHROPIC_STREAM.split_inclusive("\n\n").take(7).collect();
        let out = streamed(&events(&format!("{}{ERROR_EVENT}", cut.concat())));
        assert_eq!(out.last(), Some(&Out::Failed("Error from Anthropic: Overloaded".into())));
        assert!(!out.iter().any(|o| matches!(o, Out::Invoke { .. })), "{out:?}");
    }

    #[test]
    fn anthropic_stream_any_chunking() {
        let expected = streamed(&events(ANTHROPIC_STREAM));
        let whole = ANTHROPIC_STREAM.as_bytes();
        assert!(same_outcome(&streamed(&[whole]), &expected), "one chunk");
        for at in 1..whole.len() {
            let out = streamed(&[&whole[..at], &whole[at..]]);
            assert!(same_outcome(&out, &expected), "split at {at}: {out:?}");
        }
    }

    #[test]
    fn anthropic_parse_follows_iterm2() {
        let whole = |body: &str| Anthropic.parse(body.as_bytes()).expect("parses").expect("a response");
        let usage = r#""usage":{"input_tokens":10,"cache_read_input_tokens":2,"cache_creation_input_tokens":3,"output_tokens":1}"#;
        let p = whole(&format!(r#"{{"id":"m","type":"message","role":"assistant","content":[{{"type":"text","text":"Hi"}}],{usage}}}"#));
        assert_eq!(p.prompt_tokens, Some(15));
        assert_eq!(p.messages.len(), 1);
        assert_eq!((p.messages[0].role, &p.messages[0].body), (Some(Role::Assistant), &Body::Text("Hi".into())));
        assert!(Anthropic.parse(b"nope").is_err());
        assert_eq!(whole(r#"{"id":"m","content":[]}"#).messages[0].body, Body::Text(String::new()), "no blocks: empty text");

        let tool_use = r#"{"type":"tool_use","id":"toolu_1","name":"f","input":{"a":1}}"#;
        let two = whole(&format!(r#"{{"id":"m","content":[{{"type":"text","text":"Let me"}},{tool_use}]}}"#));
        let call = FunctionCall { name: Some("f".into()), arguments: Some("{\"a\":1}".into()), id: Some("toolu_1".into()), thought_signature: None };
        assert_eq!(two.messages[0].body, Body::Multipart(vec![Body::Text("Let me".into()), Body::FunctionCall(call, Some(CallId { call_id: "toolu_1".into(), item_id: "toolu_1".into() }))]));

        let one = |body: String| {
            let (mut r, _) = runner(Turn::new(TurnInput { stream: false, ..turn_input() }), vec![Reply::ok(&[body.as_bytes()])]);
            assert_eq!(r.start(Some(&key())), [Out::WillSend]);
            (r.poll(), r.turn().prompt_tokens())
        };
        assert_eq!(one(format!(r#"{{"id":"m","type":"message","role":"assistant","content":[{{"type":"text","text":"Hi"}}],{usage}}}"#)), (vec![Out::Choice("Hi".into())], Some(15)));
        assert_eq!(one(format!(r#"{{"id":"m","content":[{{"type":"text","text":"Let me"}},{tool_use}]}}"#)).0, [invoke()]);
    }
}

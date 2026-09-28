//! OpenAI's Responses API: the request body (sorted keys, for OpenAI's prompt cache), the input
//! items a conversation replays, the function and hosted tools, and the parsers of streamed
//! events and whole answers. Pure; ports `AITerm/ResponsesAPIRequest.swift` (`RQ:1661-1966`),
//! `ResponsesResponseStreamingParser.swift` (`RS:516-682`) and `ResponsesAPIResponse.swift`
//! (`RR:1335-1399`, `RR:1507-1520`).

use crate::ai_catalog::Feature;
use crate::ai_json::{sorted, splice_at_key};
use crate::ai_model::{has, mime_is_textual};
use crate::ai_msg::{Attachment, Body, CallId, FunctionCall, Message, ReasoningItem, Role, Status};
use crate::ai_turn::{max_tokens_for, Build, Dialect, Hosted, Parsed, ToolDecl};
use base64::Engine;
use serde_json::{json, Map, Value};

/// "Something went wrong: <message>", a `response.failed` event's text.
pub fn something_went_wrong(message: &str) -> String {
    format!("Something went wrong: {message}")
}

/// The reason of a `response.failed` event that names none.
pub const UNKNOWN_ERROR: &str = "Unknown error";

/// A refusal in a whole answer.
pub fn refused(text: &str) -> String {
    format!("The request was refused: {text}")
}

pub struct Responses;

fn base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// An `input` message item; `content` is a string or a list of content parts.
fn message_item(role: &str, content: Value) -> Value {
    json!({"content": content, "role": role, "type": "message"})
}

/// The input items one message replays as (`RQ:1661-1757`): reasoning items before the calls
/// they produced, a tool's output, system and user messages with their attachments. A message
/// with no role, or a shape the API does not take, gives none.
pub fn input_items(m: &Message, hosted: &Hosted) -> Vec<Value> {
    match m.role {
        Some(Role::Assistant) => replayed(m),
        Some(Role::Function) if m.function_call().is_some() => replayed(m),
        Some(Role::Function) => match (m.call_id(), m.content()) {
            (Some(id), Some(output)) => vec![json!({"call_id": id.call_id, "output": output, "type": "function_call_output"})],
            _ => Vec::new(),
        },
        Some(Role::System) => m.content().map(|c| message_item("system", Value::String(c))).into_iter().collect(),
        Some(Role::User) => match &m.body {
            Body::Text(text) => vec![message_item("user", Value::String(text.clone()))],
            Body::Multipart(parts) => {
                let content: Vec<Value> = parts.iter().filter_map(|p| user_part(p, hosted)).collect();
                vec![message_item("user", Value::Array(content))]
            }
            Body::Uninitialized | Body::FunctionCall(..) | Body::FunctionOutput { .. } | Body::Attachment(_) => Vec::new(),
        },
        None => Vec::new(),
    }
}

/// One part of a user's multipart message (`RQ:1701-1745`).
fn user_part(part: &Body, hosted: &Hosted) -> Option<Value> {
    let Body::Attachment(a) = part else {
        return match part {
            Body::Text(text) => Some(json!({"text": text, "type": "input_text"})),
            _ => None,
        };
    };
    match a {
        Attachment::Code(code) => Some(json!({"file_data": format!("data:text/plain;base64,{}", base64(code.as_bytes())), "type": "input_file"})),
        Attachment::Status { .. } => None,
        Attachment::File { mime, content, .. } if mime_is_textual(mime) => Some(json!({"text": String::from_utf8_lossy(content), "type": "input_text"})),
        // The API refuses an image sent as a file.
        Attachment::File { mime, content, .. } if mime.starts_with("image/") => {
            Some(json!({"detail": "auto", "image_url": format!("data:{mime};base64,{}", base64(content)), "type": "input_image"}))
        }
        Attachment::File { name, mime, content } => Some(json!({"file_data": format!("data:{mime};base64,{}", base64(content)), "filename": name, "type": "input_file"})),
        // With the code interpreter, file ids go on its tool instead.
        Attachment::FileId { .. } if hosted.code_interpreter => None,
        Attachment::FileId { id, .. } => Some(json!({"file_id": id, "type": "input_file"})),
    }
}

/// An assistant turn, or a function-role call: its reasoning items, then its parts. A call
/// keeps its item id only beside its reasoning (the API pairs them).
fn replayed(m: &Message) -> Vec<Value> {
    let mut out: Vec<Value> = m.reasoning_items.iter().map(reasoning_item).collect();
    out.extend(assistant_items(&m.body, !m.reasoning_items.is_empty()));
    out
}

fn reasoning_item(r: &ReasoningItem) -> Value {
    let mut v = json!({
        "id": r.id,
        "summary": r.summary.iter().map(|s| json!({"text": s, "type": "summary_text"})).collect::<Vec<_>>(),
        "type": "reasoning",
    });
    if let Some(e) = &r.encrypted_content {
        v["encrypted_content"] = Value::String(e.clone());
    }
    v
}

/// `assistantEntries` (`RQ:1771-1826`): assistant messages take only text, so attachments
/// become text placeholders and statuses are dropped.
fn assistant_items(body: &Body, keep_item_ids: bool) -> Vec<Value> {
    let text = |s: String| vec![message_item("assistant", Value::String(s))];
    match body {
        Body::Text(s) => text(s.clone()),
        Body::FunctionCall(call, id) => {
            let (Some(arguments), Some(id), Some(name)) = (&call.arguments, id, &call.name) else {
                return Vec::new();
            };
            let mut v = json!({"arguments": arguments, "call_id": id.call_id, "name": name, "type": "function_call"});
            if keep_item_ids {
                v["id"] = Value::String(id.item_id.clone());
            }
            vec![v]
        }
        Body::Multipart(parts) => parts.iter().flat_map(|p| assistant_items(p, keep_item_ids)).collect(),
        Body::Attachment(Attachment::Code(s)) => text(s.clone()),
        Body::Attachment(Attachment::Status { .. }) => Vec::new(),
        Body::Attachment(Attachment::File { name, mime, .. }) => text(format!("[file attachment: {name} ({mime})]")),
        Body::Attachment(Attachment::FileId { name, .. }) => text(format!("[file attachment: {name}]")),
        Body::FunctionOutput { .. } | Body::Uninitialized => body.content().map(text).unwrap_or_default(),
    }
}

/// A function the model may call (`RQ:1882-1889`): strict, with `additionalProperties: false`
/// on its parameters (Decision 9).
pub fn function_tool(t: &ToolDecl) -> Value {
    let mut parameters = t.parameters.clone();
    if let Value::Object(schema) = &mut parameters {
        schema.insert("additionalProperties".to_string(), Value::Bool(false));
    }
    json!({"description": t.description, "name": t.name, "parameters": parameters, "strict": true, "type": "function"})
}

/// The vendor-hosted tools (`RQ:1845-1880`), in iTerm2's order; the code interpreter gets the
/// file ids of the `last` message.
pub fn hosted_tools(h: &Hosted, last: Option<&Message>) -> Vec<Value> {
    let mut out = Vec::new();
    if !h.file_search_vector_stores.is_empty() {
        out.push(json!({"filters": [], "type": "file_search", "vector_store_ids": h.file_search_vector_stores}));
    }
    if h.web_search {
        out.push(json!({"search_context_size": "medium", "type": "web_search_preview", "user_location": {"type": "approximate"}}));
    }
    if h.code_interpreter {
        let file_ids: Vec<&str> = match last.map(|m| &m.body) {
            Some(Body::Multipart(parts)) => parts
                .iter()
                .filter_map(|p| match p {
                    Body::Attachment(Attachment::FileId { id, .. }) => Some(id.as_str()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        out.push(json!({"container": {"file_ids": file_ids, "type": "auto"}, "type": "code_interpreter"}));
    }
    out
}

/// The status update an event shows (`RS:546-651`), if it shows one.
pub fn status_of(event: &Value) -> Option<Status> {
    let item_type = event["item"]["type"].as_str();
    match event["type"].as_str()? {
        "response.output_item.added" if item_type == Some("web_search_call") => Some(Status::WebSearchStarted),
        "response.output_item.done" if item_type == Some("web_search_call") => event["item"]["action"]["query"].as_str().map(|q| Status::WebSearchFinished(Some(q.to_string()))),
        "response.web_search_call.completed" => Some(Status::WebSearchFinished(None)),
        "response.reasoning_summary_text.done" => event["text"].as_str().map(|t| Status::ReasoningSummary(t.to_string())),
        "response.code_interpreter_call.interpreting" => Some(Status::CodeInterpreterStarted),
        "response.code_interpreter_call.completed" => Some(Status::CodeInterpreterFinished),
        _ => None,
    }
}

/// A status attachment shown inline, with a fresh id as iTerm2's.
fn status_message(status: Status) -> Message {
    let id = uuid::Uuid::new_v4().to_string().to_uppercase();
    Message::new(Some(Role::Assistant), Body::Attachment(Attachment::Status { id, inline: true, status }))
}

fn string(v: &Value) -> Option<String> {
    v.as_str().map(str::to_string)
}

/// Summary texts, the empty ones dropped (`RS:579`, `RR:1375`).
fn summaries(item: &Value) -> Vec<String> {
    item["summary"].as_array().into_iter().flatten().filter_map(|s| s["text"].as_str()).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

fn reasoning_of(item: &Value) -> ReasoningItem {
    ReasoningItem { id: item["id"].as_str().unwrap_or_default().to_string(), encrypted_content: string(&item["encrypted_content"]), summary: summaries(item) }
}

impl Responses {
    /// One streamed event (`RS:516-682`); anything iTerm2 does not know, or cannot decode, is
    /// ignored, and `response.done` ends the stream.
    fn event(&self, event: &Value) -> Option<Parsed> {
        let mut p = Parsed { ignore: true, ..Parsed::default() };
        let item = &event["item"];
        let shown = |p: &mut Parsed, m: Message| {
            p.messages.push(m);
            p.ignore = false;
        };
        match event["type"].as_str().unwrap_or_default() {
            "response.failed" => {
                let reason = event["response"]["error"]["message"].as_str().unwrap_or(UNKNOWN_ERROR);
                shown(&mut p, Message::text(Role::Assistant, &something_went_wrong(reason)));
            }
            "response.output_text.delta" => {
                if let Some(delta) = event["delta"].as_str() {
                    shown(&mut p, Message::text(Role::Assistant, delta));
                }
            }
            "response.output_item.added" if item["type"] == "function_call" => {
                let call = FunctionCall { name: string(&item["name"]), arguments: string(&item["arguments"]), id: None, thought_signature: None };
                let id = CallId { call_id: item["call_id"].as_str().unwrap_or_default().to_string(), item_id: item["id"].as_str().unwrap_or_default().to_string() };
                shown(&mut p, Message::new(Some(Role::Function), Body::FunctionCall(call, Some(id))));
            }
            "response.output_item.done" if item["type"] == "web_search_call" => {
                p.ignore = false;
                if let Some(status) = status_of(event) {
                    p.messages.push(status_message(status));
                }
            }
            // The encrypted payload rides on a bodiless message until the turn's call takes it.
            "response.output_item.done" if item["type"] == "reasoning" => {
                let mut m = Message::new(Some(Role::Assistant), Body::Uninitialized);
                m.reasoning_items.push(reasoning_of(item));
                shown(&mut p, m);
            }
            // Streamed arguments carry no call id; the item id matches them to their call.
            "response.function_call_arguments.delta" => {
                if let Some(delta) = event["delta"].as_str() {
                    let call = FunctionCall { name: None, arguments: Some(delta.to_string()), id: None, thought_signature: None };
                    let id = CallId { call_id: String::new(), item_id: event["item_id"].as_str().unwrap_or_default().to_string() };
                    shown(&mut p, Message::new(Some(Role::Function), Body::FunctionCall(call, Some(id))));
                }
            }
            "response.created" => {
                p.response_id = string(&event["response"]["id"]);
                p.ignore = false;
            }
            "response.done" => return None,
            "response.code_interpreter_call_code.delta" => {
                if let Some(delta) = event["delta"].as_str() {
                    shown(&mut p, Message::new(Some(Role::Assistant), Body::Attachment(Attachment::Code(delta.to_string()))));
                }
            }
            _ => {
                if let Some(status) = status_of(event) {
                    shown(&mut p, status_message(status));
                }
            }
        }
        Some(p)
    }
}

impl Dialect for Responses {
    /// `body()` (`RQ:1891-1966`).
    fn body(&self, b: &Build) -> Result<Vec<u8>, String> {
        // With a previous response id the server holds the history: only the newest message
        // goes, and the volatile text after it.
        let base = match b.previous_response_id {
            Some(_) if !b.messages.is_empty() => &b.messages[b.messages.len() - 1..],
            _ => b.messages,
        };
        let volatile = b.volatile.filter(|v| !v.is_empty()).map(|v| Message::text(Role::User, v));
        let input: Vec<Value> = base.iter().chain(volatile.as_ref()).flat_map(|m| input_items(m, b.hosted)).collect();
        let mut tools: Vec<Value> = b.tools.iter().map(function_tool).collect();
        tools.extend(hosted_tools(b.hosted, b.messages.last()));
        let mut body = Map::new();
        body.insert("input".into(), Value::Array(input));
        body.insert("model".into(), json!(b.model.name));
        body.insert("max_output_tokens".into(), json!(max_tokens_for(b)));
        if let Some(id) = b.previous_response_id {
            body.insert("previous_response_id".into(), json!(id));
        }
        body.insert("stream".into(), json!(b.stream));
        body.insert("tool_choice".into(), json!(if tools.is_empty() { "none" } else { "auto" }));
        body.insert("tools".into(), Value::Array(tools));
        let summary = |on: bool| on.then_some("auto");
        let reasoning = match (b.effort, b.should_think) {
            (Some(effort), think) => Some((effort, summary(think == Some(true) && effort != crate::ai_catalog::Effort::None))),
            (None, Some(think)) => Some((if think { b.model.thinking_on_effort } else { b.model.thinking_off_effort }, summary(think))),
            (None, None) => None,
        };
        if let Some((effort, summary)) = reasoning {
            let mut r = json!({"effort": effort.as_str()});
            if let Some(s) = summary {
                r["summary"] = json!(s);
            }
            body.insert("reasoning".into(), r);
        }
        // Only reasoning models accept the include.
        if has(b.model, Feature::ConfigurableThinking) {
            body.insert("include".into(), json!(["reasoning.encrypted_content"]));
        }
        if let Some(tier) = b.tier {
            body.insert("service_tier".into(), json!(tier.as_str()));
        }
        // Zero Data Retention: nothing stored server-side (the default is to store).
        if b.wire.zdr {
            body.insert("store".into(), json!(false));
        }
        let bytes = sorted(&Value::Object(body)).into_bytes();
        if b.previous_response_id.is_some() {
            return Ok(bytes);
        }
        let systems = b.messages.iter().filter(|m| m.role == Some(Role::System)).count();
        splice_at_key(&bytes, "input", systems, b.frozen)
    }

    /// A whole answer (`RR:1335-1399`): its output items collapse into one assistant message,
    /// a multipart when there are several parts.
    fn parse(&self, body: &[u8]) -> Result<Option<Parsed>, String> {
        let v: Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
        if !v.is_object() {
            return Err("the response is not an object".to_string());
        }
        let mut bodies = Vec::new();
        let mut reasoning = Vec::new();
        for item in v["output"].as_array().into_iter().flatten() {
            match item["type"].as_str().unwrap_or_default() {
                "function_call" => {
                    let call = FunctionCall { name: string(&item["name"]), arguments: string(&item["arguments"]), id: None, thought_signature: None };
                    let id = CallId { call_id: item["call_id"].as_str().unwrap_or_default().to_string(), item_id: item["id"].as_str().unwrap_or_default().to_string() };
                    bodies.push(Body::FunctionCall(call, Some(id)));
                }
                "message" => {
                    for content in item["content"].as_array().into_iter().flatten() {
                        match content["type"].as_str().unwrap_or_default() {
                            "output_text" => bodies.push(Body::Text(content["text"].as_str().unwrap_or_default().to_string())),
                            "refusal" => bodies.push(Body::Text(refused(content["refusal"].as_str().unwrap_or_default()))),
                            _ => {}
                        }
                    }
                }
                "reasoning" => reasoning.push(reasoning_of(item)),
                _ => {}
            }
        }
        let body = match bodies.len() {
            0 if reasoning.is_empty() => None,
            0 => Some(Body::Uninitialized),
            1 => bodies.pop(),
            _ => Some(Body::Multipart(bodies)),
        };
        let messages = body
            .map(|body| {
                let mut m = Message::new(Some(Role::Assistant), body);
                m.reasoning_items = reasoning;
                m
            })
            .into_iter()
            .collect();
        Ok(Some(Parsed { messages, response_id: string(&v["id"]), prompt_tokens: v["usage"]["input_tokens"].as_i64(), ignore: false }))
    }

    fn parse_event(&self, json: &str) -> Result<Option<Parsed>, String> {
        match serde_json::from_str::<Value>(json) {
            Ok(event) => Ok(self.event(&event)),
            // iTerm2 ignores an event it cannot decode.
            Err(_) => Ok(Some(Parsed { ignore: true, ..Parsed::default() })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_catalog::{Effort, Feature, Model, ServiceTier};
    use crate::ai_model::fixture;
    use crate::ai_msg::{Attachment, Body, CallId, FunctionCall, Message, ReasoningItem, Role, Status};
    use crate::ai_settings::Api;
    use crate::ai_transport::{Options, Proxy, Recorded, Reply};
    use crate::ai_turn::{headers, same_outcome, Build, Dialect, Hosted, Out, ToolDecl, ToolResult, Turn, TurnInput, Wire};
    use crate::pwm::Secret;
    use serde_json::{json, Value};
    use std::sync::Arc;
    use std::time::Duration;

    const RESPONSES_STREAM: &str = concat!(
        "event: response.created\n",
        r#"data: {"type":"response.created","response":{"id":"resp_1","status":"in_progress"}}"#,
        "\n\n",
        "event: response.output_item.added\n",
        r#"data: {"type":"response.output_item.added","item":{"type":"web_search_call","id":"ws_1"}}"#,
        "\n\n",
        r#"data: {"type":"response.output_text.delta","item_id":"msg_1","delta":"Hel"}"#,
        "\n\n",
        r#"data: {"type":"response.output_text.delta","item_id":"msg_1","delta":"lo"}"#,
        "\n\n",
        r#"data: {"type":"response.output_item.done","item":{"type":"web_search_call","id":"ws_1","action":{"query":"rust"}}}"#,
        "\n\n",
        r#"data: {"type":"response.reasoning_summary_text.done","text":"thinking"}"#,
        "\n\n",
        r#"data: {"type":"response.output_item.done","item":{"type":"reasoning","id":"rs_1","encrypted_content":"E","summary":[{"type":"summary_text","text":"s"}]}}"#,
        "\n\n",
        r#"data: {"type":"response.output_item.added","item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"f","arguments":""}}"#,
        "\n\n",
        r#"data: {"type":"response.function_call_arguments.delta","item_id":"fc_1","delta":"{\"a\":"}"#,
        "\n\n",
        r#"data: {"type":"response.function_call_arguments.delta","item_id":"fc_1","delta":"1}"}"#,
        "\n\n",
        "event: response.completed\n",
        r#"data: {"type":"response.completed","response":{"id":"resp_1","status":"completed"}}"#,
        "\n\n",
    );

    fn model() -> Model {
        fixture("gpt-test", "https://api.openai.com/v1/responses", Api::Responses)
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

    fn body(b: &Build) -> String {
        String::from_utf8(Responses.body(b).expect("body builds")).expect("UTF-8")
    }

    fn sh() -> Vec<Message> {
        vec![Message::text(Role::System, "S"), Message::text(Role::User, "hi")]
    }

    /// Each item written as the body writes it.
    fn items(m: &Message, hosted: &Hosted) -> Vec<String> {
        input_items(m, hosted).iter().map(sorted).collect()
    }

    #[test]
    fn responses_body_follows_iterm2() {
        let (m, w, none) = (model(), wire(), Hosted::default());
        let messages = sh();
        let b = build(&m, &messages, &[], &none, &w);
        assert_eq!(
            body(&b),
            r#"{"input":[{"content":"S","role":"system","type":"message"},{"content":"hi","role":"user","type":"message"}],"max_output_tokens":8192,"model":"gpt-test","stream":true,"tool_choice":"none","tools":[]}"#
        );
        assert!(headers(&b) == [("Authorization".to_string(), "Bearer sk-a".to_string()), ("Content-Type".to_string(), "application/json".to_string())], "Authorization and Content-Type headers");

        let tools = [tool()];
        let with_tool = body(&build(&m, &messages, &tools, &none, &w));
        assert!(with_tool.contains(r#""tool_choice":"auto","tools":[{"description":"d","name":"f","parameters":{"additionalProperties":false,"properties":{},"required":[],"type":"object"},"strict":true,"type":"function"}]"#), "{with_tool}");

        let four = vec![Message::text(Role::System, "S"), Message::text(Role::User, "a"), Message::text(Role::Assistant, "b"), Message::text(Role::User, "c")];
        let small = Wire { token_limit: 100, ..wire() };
        let mut b = build(&m, &four, &[], &none, &small);
        b.previous_response_id = Some("resp_1");
        b.volatile = Some("V");
        let delta = body(&b);
        assert!(delta.starts_with(r#"{"input":[{"content":"c","role":"user","type":"message"},{"content":"V","role":"user","type":"message"}],"#), "{delta}");
        assert!(delta.contains(r#""previous_response_id":"resp_1""#), "{delta}");
        // "S\na\nb\nc" is 7 bytes, 3 tokens: 100 - 3, not 100 - 0 for "c" alone.
        assert!(delta.contains(r#""max_output_tokens":97,"#), "{delta}");
        b.volatile = Some("");
        assert!(body(&b).starts_with(r#"{"input":[{"content":"c","role":"user","type":"message"}],"#), "empty volatile text is not sent");

        let zdr = Wire { zdr: true, ..wire() };
        assert!(body(&build(&m, &messages, &[], &none, &zdr)).ends_with(r#""store":false,"stream":true,"tool_choice":"none","tools":[]}"#));

        let reasoning = |effort: Option<Effort>, think: Option<bool>| {
            let mut b = build(&m, &messages, &[], &none, &w);
            b.effort = effort;
            b.should_think = think;
            let v: Value = serde_json::from_str(&body(&b)).unwrap();
            v.get("reasoning").map(sorted)
        };
        assert_eq!(reasoning(Some(Effort::High), Some(true)).as_deref(), Some(r#"{"effort":"high","summary":"auto"}"#));
        assert_eq!(reasoning(Some(Effort::High), None).as_deref(), Some(r#"{"effort":"high"}"#));
        assert_eq!(reasoning(Some(Effort::None), Some(true)).as_deref(), Some(r#"{"effort":"none"}"#));
        assert_eq!(reasoning(None, Some(false)).as_deref(), Some(r#"{"effort":"low"}"#));
        assert_eq!(reasoning(None, Some(true)).as_deref(), Some(r#"{"effort":"medium","summary":"auto"}"#));
        assert_eq!(reasoning(None, None), None);

        let mut thinking = model();
        thinking.features.insert(Feature::ConfigurableThinking);
        thinking.thinking_off_effort = Effort::Minimal;
        let b = build(&thinking, &messages, &[], &none, &w);
        assert!(body(&b).contains(r#""include":["reasoning.encrypted_content"],"input":"#), "whatever the reasoning settings");
        let mut off = build(&thinking, &messages, &[], &none, &w);
        off.should_think = Some(false);
        assert!(body(&off).contains(r#""include":["reasoning.encrypted_content"],"input":"#));
        assert!(body(&off).contains(r#""reasoning":{"effort":"minimal"}"#), "the model's own off effort");

        let mut tier = build(&m, &messages, &[], &none, &w);
        tier.tier = Some(ServiceTier::Flex);
        assert!(body(&tier).contains(r#""service_tier":"flex","stream":true"#));

        // No token-limit check: the budget goes negative and is sent anyway.
        let tiny = Wire { token_limit: 1, ..wire() };
        assert!(body(&build(&m, &messages, &[], &none, &tiny)).contains(r#""max_output_tokens":-1,"#));

        let mut plain = build(&m, &messages, &[], &none, &w);
        plain.stream = false;
        assert!(body(&plain).contains(r#""stream":false"#));
        let mut named = model();
        named.wire_model_name = Some("wire".into());
        assert!(body(&build(&named, &messages, &[], &none, &w)).contains(r#""model":"gpt-test""#), "the name, not the wire name");
    }

    fn user_multipart() -> Message {
        Message::new(
            Some(Role::User),
            Body::Multipart(vec![
                Body::Text("look".into()),
                Body::Attachment(Attachment::File { name: "a.png".into(), mime: "image/png".into(), content: vec![0, 1, 2] }),
                Body::Attachment(Attachment::Code("x=1".into())),
                Body::Attachment(Attachment::File { name: "n.txt".into(), mime: "text/plain".into(), content: b"hey".to_vec() }),
                Body::Attachment(Attachment::File { name: "r.pdf".into(), mime: "application/pdf".into(), content: b"%PDF".to_vec() }),
                Body::Attachment(Attachment::FileId { id: "file_1".into(), name: "f.pdf".into() }),
            ]),
        )
    }

    fn call_message(role: Role, reasoning: Vec<ReasoningItem>) -> Message {
        let mut m = Message::new(
            Some(role),
            Body::FunctionCall(FunctionCall { name: Some("f".into()), arguments: Some("{}".into()), id: None, thought_signature: None }, Some(CallId { call_id: "call_1".into(), item_id: "fc_1".into() })),
        );
        m.reasoning_items = reasoning;
        m
    }

    #[test]
    fn responses_history_follows_iterm2() {
        let none = Hosted::default();
        assert_eq!(items(&call_message(Role::Assistant, vec![]), &none), [r#"{"arguments":"{}","call_id":"call_1","name":"f","type":"function_call"}"#]);
        let rs = ReasoningItem { id: "rs_1".into(), encrypted_content: Some("E".into()), summary: vec!["s".into()] };
        let with_reasoning = [
            r#"{"encrypted_content":"E","id":"rs_1","summary":[{"text":"s","type":"summary_text"}],"type":"reasoning"}"#,
            r#"{"arguments":"{}","call_id":"call_1","id":"fc_1","name":"f","type":"function_call"}"#,
        ];
        assert_eq!(items(&call_message(Role::Assistant, vec![rs.clone()]), &none), with_reasoning);
        assert_eq!(items(&call_message(Role::Function, vec![rs]), &none), with_reasoning, "a function-role call replays the same way");
        let bare = ReasoningItem { id: "rs_2".into(), encrypted_content: None, summary: vec![] };
        assert_eq!(items(&call_message(Role::Assistant, vec![bare]), &none)[0], r#"{"id":"rs_2","summary":[],"type":"reasoning"}"#, "summary is always sent");

        for broken in [
            FunctionCall { name: None, arguments: Some("{}".into()), id: None, thought_signature: None },
            FunctionCall { name: Some("f".into()), arguments: None, id: None, thought_signature: None },
        ] {
            let m = Message::new(Some(Role::Assistant), Body::FunctionCall(broken, Some(CallId::default())));
            assert!(items(&m, &none).is_empty(), "a call missing its name or arguments is dropped");
        }
        let no_id = Message::new(Some(Role::Assistant), Body::FunctionCall(FunctionCall { name: Some("f".into()), arguments: Some("{}".into()), id: None, thought_signature: None }, None));
        assert!(items(&no_id, &none).is_empty(), "a call missing its id is dropped");

        let output = Message::new(Some(Role::Function), Body::FunctionOutput { name: "f".into(), output: "42".into(), id: Some(CallId { call_id: "call_1".into(), item_id: "fc_1".into() }) });
        assert_eq!(items(&output, &none), [r#"{"call_id":"call_1","output":"42","type":"function_call_output"}"#]);
        let orphan = Message::new(Some(Role::Function), Body::FunctionOutput { name: "f".into(), output: "42".into(), id: None });
        assert!(items(&orphan, &none).is_empty(), "an output without a call id is dropped");

        assert_eq!(items(&Message::text(Role::Assistant, "b"), &none), [r#"{"content":"b","role":"assistant","type":"message"}"#]);
        let mixed = Message::new(Some(Role::Assistant), Body::Multipart(vec![Body::Text("t".into()), call_message(Role::Assistant, vec![]).body]));
        assert_eq!(items(&mixed, &none), [r#"{"content":"t","role":"assistant","type":"message"}"#, r#"{"arguments":"{}","call_id":"call_1","name":"f","type":"function_call"}"#]);
        let attachments = |a: Attachment| items(&Message::new(Some(Role::Assistant), Body::Attachment(a)), &none);
        assert_eq!(attachments(Attachment::Code("c".into())), [r#"{"content":"c","role":"assistant","type":"message"}"#]);
        assert!(attachments(Attachment::Status { id: "x".into(), inline: true, status: Status::WebSearchStarted }).is_empty());
        assert_eq!(attachments(Attachment::File { name: "a.txt".into(), mime: "text/plain".into(), content: vec![] }), [r#"{"content":"[file attachment: a.txt (text/plain)]","role":"assistant","type":"message"}"#]);
        assert_eq!(attachments(Attachment::FileId { id: "file_1".into(), name: "r.pdf".into() }), [r#"{"content":"[file attachment: r.pdf]","role":"assistant","type":"message"}"#]);

        assert_eq!(items(&Message::text(Role::System, "S"), &none), [r#"{"content":"S","role":"system","type":"message"}"#]);
        assert!(items(&Message::new(Some(Role::User), Body::Uninitialized), &none).is_empty());
        assert!(items(&Message::new(None, Body::Text("x".into())), &none).is_empty(), "no role, no item");
        assert!(items(&Message::new(Some(Role::User), Body::Attachment(Attachment::Code("x".into()))), &none).is_empty(), "a bare user attachment is dropped, as iTerm2's");

        let parts = [
            r#"{"text":"look","type":"input_text"}"#,
            r#"{"detail":"auto","image_url":"data:image/png;base64,AAEC","type":"input_image"}"#,
            r#"{"file_data":"data:text/plain;base64,eD0x","type":"input_file"}"#,
            r#"{"text":"hey","type":"input_text"}"#,
            r#"{"file_data":"data:application/pdf;base64,JVBERg==","filename":"r.pdf","type":"input_file"}"#,
            r#"{"file_id":"file_1","type":"input_file"}"#,
        ];
        assert_eq!(items(&user_multipart(), &none), [format!(r#"{{"content":[{}],"role":"user","type":"message"}}"#, parts.join(","))]);
        let interpreter = Hosted { code_interpreter: true, ..Hosted::default() };
        assert_eq!(items(&user_multipart(), &interpreter), [format!(r#"{{"content":[{}],"role":"user","type":"message"}}"#, parts[..5].join(","))], "the file id rides on the code interpreter tool");
    }

    #[test]
    fn responses_hosted_tools_follow_iterm2() {
        let file_id = Message::new(Some(Role::User), Body::Multipart(vec![Body::Text("t".into()), Body::Attachment(Attachment::FileId { id: "file_1".into(), name: "n".into() })]));
        let all = Hosted { file_search_vector_stores: vec!["vs_1".into()], web_search: true, code_interpreter: true };
        let tools: Vec<String> = hosted_tools(&all, Some(&file_id)).iter().map(sorted).collect();
        assert_eq!(
            tools,
            [
                r#"{"filters":[],"type":"file_search","vector_store_ids":["vs_1"]}"#,
                r#"{"search_context_size":"medium","type":"web_search_preview","user_location":{"type":"approximate"}}"#,
                r#"{"container":{"file_ids":["file_1"],"type":"auto"},"type":"code_interpreter"}"#,
            ]
        );
        let interpreter = Hosted { code_interpreter: true, ..Hosted::default() };
        assert_eq!(hosted_tools(&interpreter, Some(&Message::text(Role::User, "t"))).iter().map(sorted).collect::<Vec<_>>(), [r#"{"container":{"file_ids":[],"type":"auto"},"type":"code_interpreter"}"#]);
        assert_eq!(hosted_tools(&interpreter, None).iter().map(sorted).collect::<Vec<_>>(), [r#"{"container":{"file_ids":[],"type":"auto"},"type":"code_interpreter"}"#]);
        assert!(hosted_tools(&Hosted::default(), Some(&file_id)).is_empty());
        assert_eq!(sorted(&function_tool(&tool())), r#"{"description":"d","name":"f","parameters":{"additionalProperties":false,"properties":{},"required":[],"type":"object"},"strict":true,"type":"function"}"#);

        let (m, w) = (model(), wire());
        let messages = vec![Message::text(Role::System, "S"), file_id];
        let fns = [tool()];
        let v: Value = serde_json::from_slice(&Responses.body(&build(&m, &messages, &fns, &all, &w)).unwrap()).unwrap();
        let types: Vec<&str> = v["tools"].as_array().unwrap().iter().map(|t| t["type"].as_str().unwrap()).collect();
        assert_eq!(types, ["function", "file_search", "web_search_preview", "code_interpreter"], "functions first");
        assert_eq!(v["tool_choice"], "auto");
        let web = Hosted { web_search: true, ..Hosted::default() };
        assert!(body(&build(&m, &messages, &[], &web, &w)).contains(r#""tool_choice":"auto""#), "hosted tools alone");
    }

    #[test]
    fn responses_splice_follows_iterm2() {
        let (m, w, none) = (model(), wire(), Hosted::default());
        let messages = sh();
        let old = br#"{"content":"old","role":"user","type":"message"}"#;
        let mut b = build(&m, &messages, &[], &none, &w);
        b.frozen = Some(old);
        assert!(body(&b).starts_with(r#"{"input":[{"content":"S","role":"system","type":"message"},{"content":"old","role":"user","type":"message"},{"content":"hi","role":"user","type":"message"}],"#), "{}", body(&b));
        b.previous_response_id = Some("resp_1");
        assert!(!body(&b).contains("old"), "the server holds the history");
    }

    fn key() -> Secret {
        Secret::new("sk-a".to_string())
    }

    fn turn(zdr: bool) -> Turn {
        Turn::new(TurnInput { wire: Wire { zdr, ..wire() }, ..turn_input() })
    }

    fn runner(t: Turn, replies: Vec<Reply>) -> (crate::ai_turn::Runner, Arc<Recorded>) {
        let rec = Arc::new(Recorded::new(replies));
        (crate::ai_turn::Runner::new(t, rec.clone(), Options { idle: Duration::ZERO, proxy: Proxy::Off }, Arc::new(|| {})), rec)
    }

    /// The fixture run through a turn in `chunks`: everything after `WillSend`.
    fn streamed(chunks: &[&[u8]]) -> Vec<Out> {
        let (mut r, _) = runner(turn(false), vec![Reply::ok(chunks)]);
        assert_eq!(r.start(Some(&key())), [Out::WillSend]);
        r.poll()
    }

    fn events() -> Vec<&'static [u8]> {
        RESPONSES_STREAM.split_inclusive("\n\n").map(str::as_bytes).collect()
    }

    fn status_out(o: &Out) -> Option<Status> {
        match o {
            Out::Attachment(Attachment::Status { status, inline: true, .. }) => Some(status.clone()),
            _ => None,
        }
    }

    fn event(json: &str) -> Parsed {
        Responses.parse_event(json).expect("parses").expect("does not end the stream")
    }

    fn shown(json: &str) -> String {
        let p = event(json);
        assert!(!p.ignore, "{json}");
        match &p.messages[..] {
            [Message { body: Body::Attachment(Attachment::Status { status, .. }), .. }] => status.display(),
            [Message { body: Body::Text(t), .. }] => t.clone(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn responses_stream_follows_iterm2() {
        let (mut r, rec) = runner(turn(true), vec![Reply::ok(&events()), Reply::ok(&[])]);
        assert_eq!(r.start(Some(&key())), [Out::WillSend]);
        let out = r.poll();
        assert_eq!(out.len(), 6, "{out:?}");
        assert_eq!(status_out(&out[0]), Some(Status::WebSearchStarted));
        assert_eq!(status_out(&out[0]).unwrap().display(), "Searching the web\u{2026}");
        assert_eq!(out[1..3], [Out::Delta("Hel".into()), Out::Delta("lo".into())]);
        assert_eq!(status_out(&out[3]), Some(Status::WebSearchFinished(Some("rust".into()))));
        assert_eq!(status_out(&out[3]).unwrap().display(), "Finished searching the web for rust.");
        assert_eq!(status_out(&out[4]), Some(Status::ReasoningSummary("thinking".into())));
        assert_eq!(
            out[5],
            Out::Invoke {
                call: FunctionCall { name: Some("f".into()), arguments: Some("{\"a\":1}".into()), id: None, thought_signature: None },
                id: Some(CallId { call_id: "call_1".into(), item_id: "fc_1".into() }),
            }
        );
        assert_eq!(r.turn().previous_response_id(), Some("resp_1"));

        // The reasoning item rides on the call message: the continuation replays it before the
        // call (ZDR: a full replay).
        assert_eq!(r.tool_done(ToolResult::Ok("42".into())), [Out::WillSend]);
        let sent = String::from_utf8(rec.requests()[1].body.clone()).unwrap();
        let replay = concat!(
            r#"{"content":"hi","role":"user","type":"message"},"#,
            r#"{"encrypted_content":"E","id":"rs_1","summary":[{"text":"s","type":"summary_text"}],"type":"reasoning"},"#,
            r#"{"arguments":"{\"a\":1}","call_id":"call_1","id":"fc_1","name":"f","type":"function_call"},"#,
            r#"{"call_id":"call_1","output":"42","type":"function_call_output"}]"#,
        );
        assert!(sent.contains(replay), "{sent}");

        assert_eq!(shown(r#"{"type":"response.code_interpreter_call.interpreting","item_id":"ci_1"}"#), "Executing code\u{2026}");
        assert_eq!(shown(r#"{"type":"response.code_interpreter_call.completed","item_id":"ci_1"}"#), "Finished executing code");
        assert_eq!(shown(r#"{"type":"response.web_search_call.completed","item_id":"ws_1"}"#), "Finished searching the web.");
        let code = event(r#"{"type":"response.code_interpreter_call_code.delta","item_id":"ci_1","delta":"print(1)"}"#);
        assert_eq!(code.messages.iter().map(|m| m.body.clone()).collect::<Vec<_>>(), [Body::Attachment(Attachment::Code("print(1)".into()))]);
        assert_eq!(shown(r#"{"type":"response.failed","response":{"error":{"message":"quota"}}}"#), "Something went wrong: quota");
        assert_eq!(shown(r#"{"type":"response.failed","response":{}}"#), "Something went wrong: Unknown error");
        assert_eq!(something_went_wrong(UNKNOWN_ERROR), "Something went wrong: Unknown error");

        for ignored in [
            r#"{"type":"response.completed","response":{"id":"resp_9","status":"completed"}}"#,
            r#"{"type":"response.something_new"}"#,
            r#"{"type":"response.output_text.done","text":"Hello"}"#,
            r#"{"type":"response.function_call_arguments.done","item_id":"fc_1","arguments":"{}"}"#,
            r#"{"type":"response.web_search_call.in_progress","item_id":"ws_1"}"#,
            r#"{"type":"response.code_interpreter_call.in_progress","item_id":"ci_1"}"#,
            r#"{"type":"response.output_item.added","item":{"type":"code_interpreter_call","id":"ci_1"}}"#,
            r#"{"type":"response.output_item.done","item":{"type":"function_call","id":"fc_1"}}"#,
            "not json",
        ] {
            let p = event(ignored);
            assert!(p.ignore && p.messages.is_empty() && p.response_id.is_none(), "{ignored}");
        }
        let unqueried = event(r#"{"type":"response.output_item.done","item":{"type":"web_search_call","id":"ws_1"}}"#);
        assert!(!unqueried.ignore && unqueried.messages.is_empty(), "a search without a query shows nothing");
        assert_eq!(Responses.parse_event(r#"{"type":"response.done","response":{"id":"r"}}"#), Ok(None), "response.done ends the stream");
        assert_eq!(status_of(&json!({"type":"response.output_item.added","item":{"type":"web_search_call","id":"w"}})), Some(Status::WebSearchStarted));
        assert_eq!(status_of(&json!({"type":"response.output_text.delta","delta":"x"})), None);

        // The stream ends when the body ends: "response.completed" changes nothing.
        let text_only: Vec<&[u8]> = events().into_iter().take(4).collect();
        let out = streamed(&text_only);
        assert_eq!(out.last(), Some(&Out::StreamEnd), "{out:?}");
    }

    #[test]
    fn responses_stream_any_chunking() {
        let expected = streamed(&events());
        let whole = RESPONSES_STREAM.as_bytes();
        assert!(same_outcome(&streamed(&[whole]), &expected), "one chunk");
        for at in 1..whole.len() {
            let out = streamed(&[&whole[..at], &whole[at..]]);
            assert!(same_outcome(&out, &expected), "split at {at}: {out:?}");
        }
    }

    #[test]
    fn responses_parse_follows_iterm2() {
        let whole = |body: &str| Responses.parse(body.as_bytes()).expect("parses").expect("a response");
        let p = whole(r#"{"id":"resp_2","output":[{"type":"message","content":[{"type":"output_text","text":"Hi"}]}],"usage":{"input_tokens":12,"output_tokens":1,"total_tokens":13}}"#);
        assert_eq!((p.response_id.as_deref(), p.prompt_tokens), (Some("resp_2"), Some(12)));
        assert_eq!(p.messages, [Message::text(Role::Assistant, "Hi")]);

        let call = r#"{"type":"function_call","name":"f","arguments":"{}","call_id":"call_1","id":"fc_1"}"#;
        let p = whole(&format!(r#"{{"id":"r","output":[{call}]}}"#));
        assert_eq!(p.prompt_tokens, None);
        assert_eq!(p.messages, [call_message(Role::Assistant, vec![])]);
        let refused_text = whole(r#"{"id":"r","output":[{"type":"message","content":[{"type":"refusal","refusal":"no"}]}]}"#);
        assert_eq!(refused_text.messages, [Message::text(Role::Assistant, "The request was refused: no")]);
        assert_eq!(refused("no"), "The request was refused: no");
        let reasoning = whole(r#"{"id":"r","output":[{"type":"reasoning","id":"rs_1","encrypted_content":"E","summary":[{"type":"summary_text","text":""}]}]}"#);
        assert_eq!(reasoning.messages[0].body, Body::Uninitialized);
        assert_eq!(reasoning.messages[0].reasoning_items, [ReasoningItem { id: "rs_1".into(), encrypted_content: Some("E".into()), summary: vec![] }]);
        assert!(whole(r#"{"id":"r","output":[{"type":"web_search_call","id":"ws"}]}"#).messages.is_empty());
        assert!(Responses.parse(b"nope").is_err());

        // Text plus a call: the turn dispatches the call.
        let (mut r, _) = runner(
            Turn::new(TurnInput { stream: false, ..turn_input() }),
            vec![Reply::ok(&[format!(r#"{{"id":"resp_2","output":[{{"type":"message","content":[{{"type":"output_text","text":"Let me"}}]}},{call}]}}"#).as_bytes()])],
        );
        assert_eq!(r.start(Some(&key())), [Out::WillSend]);
        let out = r.poll();
        assert!(matches!(&out[..], [Out::Invoke { call, .. }] if call.name.as_deref() == Some("f")), "{out:?}");
        let (mut r, _) = runner(Turn::new(TurnInput { stream: false, ..turn_input() }), vec![Reply::ok(&[br#"{"id":"resp_2","output":[{"type":"message","content":[{"type":"output_text","text":"Hi"}]}]}"#])]);
        r.start(Some(&key()));
        assert_eq!(r.poll(), [Out::Choice("Hi".into())]);
        assert_eq!(r.turn().previous_response_id(), Some("resp_2"));
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
}

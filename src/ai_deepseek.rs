//! DeepSeek's chat API: Chat Completions with the reasoning of an assistant turn echoed back as
//! `reasoning_content`, the `function_call: "auto"` switch, the `thinking` block of a model with
//! configurable thinking, and streamed reasoning shown as a status. Pure; ports
//! `AITerm/DeepSeek.swift` (`DS:8-354`).

use crate::ai_catalog::Feature;
use crate::ai_chat::{attachment_text, budget, function_json, parse_modern, systems, unavailable, with_volatile};
use crate::ai_json::splice_at_key;
use crate::ai_model::{dynamic_models_supported, has};
use crate::ai_msg::{Attachment, Body, CallId, FunctionCall, Message, Role, Status};
use crate::ai_turn::{max_tokens_for, Build, Dialect, Parsed};
use serde_json::{json, Map, Value};

pub struct DeepSeek;

/// The status attachment of streamed reasoning (`DS:317-321`).
pub const REASONING_ID: &str = "deepseek-reasoning";

/// One `tool_calls` entry (`Message.ToolCall`, `DS:52-56`): the id when there is one.
fn tool_call(id: Option<&str>, call: &FunctionCall) -> Value {
    let mut v = Map::new();
    if let Some(id) = id {
        v.insert("id".into(), json!(id));
    }
    v.insert("type".into(), json!("function"));
    v.insert("function".into(), function_json(call));
    Value::Object(v)
}

/// `DeepSeekRequestBuilder.Message.init` (`DS:64-180`): keys in declaration order, absent ones
/// omitted. An assistant turn echoes its reasoning; a part of a multipart turn is text, except a
/// call.
pub fn message(m: &Message) -> Value {
    let role = m.role.map(|r| match r {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
        Role::Function => "tool",
    });
    let reasoning = m.reasoning_content.as_deref().filter(|r| m.role == Some(Role::Assistant) && !r.is_empty());
    let mut tool_call_id = None;
    let mut content = None;
    let mut tool_calls = Vec::new();
    match &m.body {
        Body::Uninitialized => {}
        Body::Text(text) => content = Some(text.clone()),
        Body::FunctionCall(call, id) => tool_calls.push(tool_call(id.as_ref().map(|i| i.call_id.as_str()), call)),
        Body::FunctionOutput { output, id, .. } => {
            tool_call_id = id.as_ref().map(|i| i.call_id.clone());
            content = Some(output.clone());
        }
        Body::Attachment(_) => content = Some(String::new()),
        Body::Multipart(parts) => {
            let mut texts = Vec::new();
            let mut output_id = None;
            for part in parts {
                match part {
                    Body::Uninitialized | Body::Multipart(_) | Body::Attachment(Attachment::Status { .. }) => {}
                    Body::Text(text) | Body::Attachment(Attachment::Code(text)) => texts.push(text.clone()),
                    Body::FunctionCall(call, id) => tool_calls.push(tool_call(id.as_ref().map(|i| i.call_id.as_str()).or(call.id.as_deref()), call)),
                    Body::FunctionOutput { output, id, .. } => {
                        if output_id.is_none() {
                            output_id = id.as_ref().map(|i| i.call_id.clone());
                        }
                        texts.push(output.clone());
                    }
                    Body::Attachment(Attachment::File { name, mime, content }) => texts.push(attachment_text(name, mime, content)),
                    Body::Attachment(Attachment::FileId { name, .. }) => texts.push(unavailable(name)),
                }
            }
            // A message is a call or an output, not both; the call wins (`DS:152-163`).
            if tool_calls.is_empty() {
                tool_call_id = output_id;
            }
            if !texts.is_empty() {
                content = Some(texts.join("\n"));
            }
        }
    }
    // A reasoning-only turn keeps the conversation alternating (`DS:170-179`).
    if m.role == Some(Role::Assistant) && content.is_none() && tool_calls.is_empty() && reasoning.is_some() {
        content = Some(String::new());
    }
    let mut v = Map::new();
    if let Some(role) = role {
        v.insert("role".into(), json!(role));
    }
    if let Some(id) = tool_call_id {
        v.insert("tool_call_id".into(), json!(id));
    }
    if let Some(content) = content {
        v.insert("content".into(), json!(content));
    }
    if !tool_calls.is_empty() {
        v.insert("tool_calls".into(), Value::Array(tool_calls));
    }
    if let Some(reasoning) = reasoning {
        v.insert("reasoning_content".into(), json!(reasoning));
    }
    Value::Object(v)
}

impl Dialect for DeepSeek {
    /// `DeepSeekRequestBuilder.body()` (`DS:185-229`): declaration order, not sorted.
    fn body(&self, b: &Build) -> Result<Vec<u8>, String> {
        let messages = with_volatile(b);
        let max = budget(max_tokens_for(&Build { messages: &messages, ..*b }))?;
        let mut body = Map::new();
        if dynamic_models_supported(b.model) {
            body.insert("model".into(), json!(b.model.name));
        }
        body.insert("messages".into(), Value::Array(messages.iter().map(message).collect()));
        body.insert("max_tokens".into(), json!(max));
        if b.model.supports_temperature {
            body.insert("temperature".into(), json!(0));
        }
        if !b.tools.is_empty() {
            let tools: Vec<Value> = b.tools.iter().map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.parameters}})).collect();
            body.insert("tools".into(), Value::Array(tools));
            body.insert("function_call".into(), json!("auto"));
        }
        body.insert("stream".into(), json!(b.stream));
        // A model with configurable thinking always says which, so the server's default
        // (enabled) never applies unasked; other models never get the field.
        if has(b.model, Feature::ConfigurableThinking) {
            let mode = if b.should_think == Some(true) { "enabled" } else { "disabled" };
            body.insert("thinking".into(), json!({"type": mode}));
        }
        let bytes = serde_json::to_vec(&Value::Object(body)).expect("a JSON value always encodes");
        splice_at_key(&bytes, "messages", systems(&messages), b.frozen)
    }

    /// `DeepSeekResponseParser` (`DS:232-253`): the Chat Completions shape, then Ollama's; when
    /// neither parses, the last parser's error.
    fn parse(&self, body: &[u8]) -> Result<Option<Parsed>, String> {
        parse_modern(body).or_else(|_| crate::ai_ollama::Ollama.parse(body))
    }

    /// `DeepSeekStreamingResponse` (`DS:255-354`): the first choice's delta. The end of a call
    /// gives nothing; reasoning is a status that also carries the text for the replay; the first
    /// tool call has its id as the call id and no item id; else the text, or an empty message.
    fn parse_event(&self, json: &str) -> Result<Option<Parsed>, String> {
        let v: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let choices = v["choices"].as_array().ok_or_else(|| "the event has no choices".to_string())?;
        let mut p = Parsed::default();
        let Some(choice) = choices.first() else {
            return Ok(Some(p));
        };
        if choice["finish_reason"] == "tool_calls" {
            return Ok(Some(p));
        }
        let delta = &choice["delta"];
        let content = delta["content"].as_str();
        if let Some(reasoning) = delta["reasoning_content"].as_str().filter(|r| !r.is_empty()) {
            let status = Attachment::Status { id: REASONING_ID.to_string(), inline: true, status: Status::ReasoningSummary(reasoning.to_string()) };
            let mut m = Message::new(Some(Role::Assistant), Body::Attachment(status));
            m.reasoning_content = Some(reasoning.to_string());
            p.messages.push(m);
        }
        if let Some(entry) = delta["tool_calls"].as_array().and_then(|calls| calls.first()) {
            let s = |v: &Value| v.as_str().map(str::to_string);
            let id = s(&entry["id"]);
            let call = FunctionCall { name: s(&entry["function"]["name"]), arguments: s(&entry["function"]["arguments"]), id: id.clone(), thought_signature: None };
            let ids = id.map(|call_id| CallId { call_id, item_id: String::new() });
            p.messages.push(Message::new(Some(Role::Assistant), Body::FunctionCall(call, ids)));
        } else if p.messages.is_empty() || content.is_some() {
            let body = content.map_or(Body::Uninitialized, |c| Body::Text(c.to_string()));
            p.messages.push(Message::new(Some(Role::Assistant), body));
        }
        Ok(Some(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_catalog::{Feature, Model};
    use crate::ai_model::fixture;
    use crate::ai_msg::{Attachment, Body, CallId, FunctionCall, Message, Role, Status};
    use crate::ai_settings::Api;
    use crate::ai_transport::{Options, Proxy, Recorded, Reply};
    use crate::ai_turn::{headers, same_outcome, Hosted, Out, ToolDecl, ToolResult, Turn, TurnInput, Wire, TOKEN_LIMIT};
    use crate::pwm::Secret;
    use serde_json::{json, Value};
    use std::sync::Arc;
    use std::time::Duration;

    const DEEPSEEK_STREAM: &str = concat!(
        r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","reasoning_content":"hmm"}}]}"#,
        "\n\n",
        r#"data: {"choices":[{"index":0,"delta":{"content":"Hi"}}]}"#,
        "\n\n",
        r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
        "\n\n",
        "data: [DONE]\n\n",
    );

    const CALL_STREAM: &str = concat!(
        r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"f","arguments":""}}]}}]}"#,
        "\n\n",
        r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"a\":"}}]}}]}"#,
        "\n\n",
        r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"1}"}}]}}]}"#,
        "\n\n",
        r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
        "\n\n",
        "data: [DONE]\n\n",
    );

    const URL: &str = "https://api.deepseek.com/chat/completions";
    const KEY: &str = "sk-deepseek-test";

    fn model() -> Model {
        fixture("deepseek-test", URL, Api::DeepSeek)
    }

    fn thinking_model() -> Model {
        let mut m = model();
        m.features.insert(Feature::ConfigurableThinking);
        m
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
            key: KEY,
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
        String::from_utf8(DeepSeek.body(b).expect("body builds")).expect("UTF-8")
    }

    fn sh() -> Vec<Message> {
        vec![Message::text(Role::System, "S"), Message::text(Role::User, "hi")]
    }

    /// The one message a lone message becomes on the wire.
    fn wire_message(m: Message) -> String {
        let (model, w, none) = (model(), wire(), Hosted::default());
        let messages = [m];
        let v: Value = serde_json::from_slice(&DeepSeek.body(&build(&model, &messages, &[], &none, &w)).expect("body builds")).expect("JSON");
        v["messages"][0].to_string()
    }

    fn assistant(body: Body) -> Message {
        Message::new(Some(Role::Assistant), body)
    }

    fn ids(id: &str) -> Option<CallId> {
        Some(CallId { call_id: id.into(), item_id: id.into() })
    }

    #[test]
    fn deepseek_body_follows_iterm2() {
        let (m, w, none) = (model(), wire(), Hosted::default());
        let messages = sh();
        let tools = [tool()];
        let b = build(&m, &messages, &tools, &none, &w);
        assert_eq!(
            body(&b),
            r#"{"model":"deepseek-test","messages":[{"role":"system","content":"S"},{"role":"user","content":"hi"}],"max_tokens":8192,"temperature":0,"tools":[{"type":"function","function":{"name":"f","description":"d","parameters":{"type":"object","properties":{},"required":[]}}}],"function_call":"auto","stream":true}"#
        );
        assert_eq!(
            body(&build(&m, &messages, &[], &none, &w)),
            r#"{"model":"deepseek-test","messages":[{"role":"system","content":"S"},{"role":"user","content":"hi"}],"max_tokens":8192,"temperature":0,"stream":true}"#,
            "no tools: no tools and no function_call"
        );
        let cold = Model { supports_temperature: false, ..model() };
        assert!(!body(&build(&cold, &messages, &[], &none, &w)).contains("temperature"), "a model that rejects temperature");
        let mut once = build(&m, &messages, &[], &none, &w);
        once.stream = false;
        assert!(body(&once).ends_with(r#""stream":false}"#));

        // Configurable thinking always says which; other models never do (`DS:195-207`).
        let t = thinking_model();
        for (think, word) in [(Some(true), "enabled"), (Some(false), "disabled"), (None, "disabled")] {
            let b = Build { should_think: think, ..build(&t, &messages, &[], &none, &w) };
            assert!(body(&b).ends_with(&format!(r#""stream":true,"thinking":{{"type":"{word}"}}}}"#)), "{think:?}");
        }
        let b = Build { should_think: Some(true), ..build(&m, &messages, &[], &none, &w) };
        assert!(!body(&b).contains("thinking"), "without configurable thinking");

        // The reasoning goes back on assistant turns only.
        let mut reasoned = Message::text(Role::Assistant, "x");
        reasoned.reasoning_content = Some("r".into());
        assert_eq!(wire_message(reasoned), r#"{"role":"assistant","content":"x","reasoning_content":"r"}"#);
        let mut user = Message::text(Role::User, "x");
        user.reasoning_content = Some("r".into());
        assert_eq!(wire_message(user), r#"{"role":"user","content":"x"}"#);
        let mut empty = Message::text(Role::Assistant, "x");
        empty.reasoning_content = Some(String::new());
        assert_eq!(wire_message(empty), r#"{"role":"assistant","content":"x"}"#, "empty reasoning is not sent");
        let mut only = assistant(Body::Uninitialized);
        only.reasoning_content = Some("r".into());
        assert_eq!(wire_message(only), r#"{"role":"assistant","content":"","reasoning_content":"r"}"#, "a reasoning-only turn gets empty content");
        assert_eq!(wire_message(assistant(Body::Uninitialized)), r#"{"role":"assistant"}"#);

        // Calls and their outputs (`DS:77-83`).
        let call = FunctionCall { name: Some("f".into()), arguments: Some("{}".into()), id: Some("call_1".into()), thought_signature: None };
        assert_eq!(
            wire_message(assistant(Body::FunctionCall(call.clone(), ids("call_1")))),
            r#"{"role":"assistant","tool_calls":[{"id":"call_1","type":"function","function":{"name":"f","arguments":"{}","id":"call_1"}}]}"#
        );
        let bare = FunctionCall { id: None, ..call.clone() };
        assert_eq!(
            wire_message(assistant(Body::FunctionCall(bare, None))),
            r#"{"role":"assistant","tool_calls":[{"type":"function","function":{"name":"f","arguments":"{}"}}]}"#,
            "no id: none on the wire"
        );
        let output = Message::new(Some(Role::Function), Body::FunctionOutput { name: "f".into(), output: "42".into(), id: ids("call_1") });
        assert_eq!(wire_message(output), r#"{"role":"tool","tool_call_id":"call_1","content":"42"}"#);
        let unnamed = Message::new(Some(Role::Function), Body::FunctionOutput { name: "f".into(), output: "42".into(), id: None });
        assert_eq!(wire_message(unnamed), r#"{"role":"tool","content":"42"}"#);

        // Attachments: a whole-message one is empty text; parts are text (`DS:84-150`).
        let png = Attachment::File { name: "a.png".into(), mime: "image/png".into(), content: vec![0, 1, 2] };
        assert_eq!(wire_message(Message::new(Some(Role::User), Body::Attachment(png))), r#"{"role":"user","content":""}"#);
        let parts = Body::Multipart(vec![
            Body::Text("a".into()),
            Body::Attachment(Attachment::Code("b".into())),
            Body::Attachment(Attachment::File { name: "x.txt".into(), mime: "text/plain".into(), content: b"hello".to_vec() }),
            Body::Attachment(Attachment::FileId { id: "file-1".into(), name: "y".into() }),
            Body::Attachment(Attachment::Status { id: "s".into(), inline: true, status: Status::WebSearchStarted }),
            Body::Uninitialized,
            Body::FunctionCall(FunctionCall { name: Some("f".into()), arguments: Some("{}".into()), id: Some("c2".into()), thought_signature: None }, None),
        ]);
        assert_eq!(
            wire_message(assistant(parts)),
            r#"{"role":"assistant","content":"a\nb\n<iterm2:attachment file=\"x.txt\" type=\"text/plain\">\nhello\n</iterm2:attachment>\nA file named y (content unavailable)","tool_calls":[{"id":"c2","type":"function","function":{"name":"f","arguments":"{}","id":"c2"}}]}"#,
            "a call without ids takes the call's own id"
        );
        let answered = Body::Multipart(vec![Body::Text("a".into()), Body::FunctionOutput { name: "f".into(), output: "42".into(), id: ids("call_9") }]);
        assert_eq!(wire_message(Message::new(Some(Role::Function), answered)), r#"{"role":"tool","tool_call_id":"call_9","content":"a\n42"}"#);
        let both = Body::Multipart(vec![
            Body::FunctionOutput { name: "f".into(), output: "42".into(), id: ids("call_9") },
            Body::FunctionCall(call, ids("call_1")),
        ]);
        assert!(!wire_message(assistant(both)).contains("tool_call_id"), "a call wins over an output's id");

        // The volatile text is a last user message; the frozen history goes after the system.
        let mut v = build(&m, &messages, &[], &none, &w);
        v.volatile = Some("V");
        assert!(body(&v).contains(r#"{"role":"user","content":"hi"},{"role":"user","content":"V"}]"#), "{}", body(&v));
        let mut frozen = build(&m, &messages, &[], &none, &w);
        frozen.frozen = Some(br#"{"role":"user","content":"old"}"#);
        assert!(body(&frozen).contains(r#""messages":[{"role":"system","content":"S"},{"role":"user","content":"old"},{"role":"user","content":"hi"}]"#), "{}", body(&frozen));

        let tight = Wire { token_limit: 10, ..wire() };
        assert_eq!(DeepSeek.body(&build(&m, &messages, &tools, &none, &tight)), Err(TOKEN_LIMIT.to_string()));

        let h = headers(&b);
        let names: Vec<&str> = h.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["Authorization", "Content-Type"]);
        assert!(h[0].1 == format!("Bearer {KEY}"), "Authorization carries the key as a bearer token");
    }

    fn key() -> Secret {
        Secret::new(KEY.to_string())
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

    fn events(stream: &str) -> Vec<&[u8]> {
        stream.split_inclusive("\n\n").map(str::as_bytes).collect()
    }

    fn streamed(chunks: &[&[u8]]) -> Vec<Out> {
        let (mut r, _) = runner(Turn::new(turn_input()), vec![Reply::ok(chunks)]);
        assert_eq!(r.start(Some(&key())), [Out::WillSend]);
        r.poll()
    }

    fn reasoning(text: &str) -> Out {
        Out::Attachment(Attachment::Status { id: REASONING_ID.into(), inline: true, status: Status::ReasoningSummary(text.into()) })
    }

    #[test]
    fn deepseek_stream_follows_iterm2() {
        assert_eq!(streamed(&events(DEEPSEEK_STREAM)), [reasoning("hmm"), Out::Delta("Hi".into()), Out::Reasoning("hmm".into()), Out::StreamEnd]);

        // A call's fragments join; the call id rides on the first only (`DS:323-331`).
        let (mut r, rec) = runner(Turn::new(turn_input()), vec![Reply::ok(&events(CALL_STREAM)), Reply::ok(&[])]);
        assert_eq!(r.start(Some(&key())), [Out::WillSend]);
        let call = FunctionCall { name: Some("f".into()), arguments: Some("{\"a\":1}".into()), id: Some("call_1".into()), thought_signature: None };
        let id = Some(CallId { call_id: "call_1".into(), item_id: String::new() });
        assert_eq!(r.poll(), [Out::Invoke { call, id }]);
        assert_eq!(r.tool_done(ToolResult::Ok("42".into())), [Out::WillSend]);
        let sent: Value = serde_json::from_slice(&rec.requests()[1].body).expect("JSON");
        assert_eq!(sent["messages"][2], json!({"role":"assistant","tool_calls":[{"id":"call_1","type":"function","function":{"name":"f","arguments":"{\"a\":1}","id":"call_1"}}]}));
        assert_eq!(sent["messages"][3], json!({"role":"tool","tool_call_id":"call_1","content":"42"}));

        let p = |json: &str| DeepSeek.parse_event(json).expect("parses").expect("an event");
        assert!(p(r#"{"choices":[{"index":0,"delta":{"content":"x"},"finish_reason":"tool_calls"}]}"#).messages.is_empty(), "the end of a call gives nothing");
        assert!(p(r#"{"choices":[]}"#).messages.is_empty());
        let both = p(r#"{"choices":[{"index":0,"delta":{"reasoning_content":"r","content":"c"}}]}"#).messages;
        assert_eq!(both.len(), 2, "reasoning and text in one delta");
        assert_eq!(both[0].reasoning_content.as_deref(), Some("r"));
        assert_eq!(both[1].body, Body::Text("c".into()));
        let placeholder = p(r#"{"choices":[{"index":0,"delta":{}}]}"#).messages;
        assert_eq!((placeholder.len(), &placeholder[0].body), (1, &Body::Uninitialized), "an empty delta is an empty message");
        let with_text = p(r#"{"choices":[{"index":0,"delta":{"content":"pre","tool_calls":[{"index":0,"id":"c","function":{"name":"f"}}]}}]}"#).messages;
        assert!(matches!(&with_text[0].body, Body::FunctionCall(..)), "a call drops the text beside it");
        assert!(DeepSeek.parse_event(r#"{"error":{"message":"m"}}"#).is_err(), "no choices");
        assert!(DeepSeek.parse_event("nope").is_err());
    }

    #[test]
    fn deepseek_stream_any_chunking() {
        let expected = streamed(&events(DEEPSEEK_STREAM));
        let whole = DEEPSEEK_STREAM.as_bytes();
        assert!(same_outcome(&streamed(&[whole]), &expected), "one chunk");
        for at in 1..whole.len() {
            let out = streamed(&[&whole[..at], &whole[at..]]);
            assert!(same_outcome(&out, &expected), "split at {at}: {out:?}");
        }
    }

    fn answered(body: &str) -> Vec<Out> {
        let (mut r, _) = runner(Turn::new(TurnInput { stream: false, ..turn_input() }), vec![Reply::ok(&[body.as_bytes()])]);
        assert_eq!(r.start(Some(&key())), [Out::WillSend]);
        r.poll()
    }

    #[test]
    fn deepseek_parse_follows_iterm2() {
        let (mut r, _) = runner(Turn::new(TurnInput { stream: false, ..turn_input() }), vec![Reply::ok(&[br#"{"choices":[{"message":{"role":"assistant","content":"Hi","reasoning_content":"r"}}],"usage":{"prompt_tokens":5}}"#])]);
        assert_eq!(r.start(Some(&key())), [Out::WillSend]);
        assert_eq!(r.poll(), [Out::Reasoning("r".into()), Out::Choice("Hi".into())]);
        assert_eq!(r.turn().prompt_tokens(), Some(5));

        assert_eq!(answered(r#"{"model":"m","created_at":"t","message":{"role":"assistant","content":"Hi"},"done":true}"#), [Out::Choice("Hi".into())], "the Ollama shape");

        // Neither parses: the last parser's error, as `DS:233-247` throws.
        let neither = br#"{"unexpected":true}"#;
        let e = DeepSeek.parse(neither).expect_err("does not parse");
        assert_eq!(Some(e), crate::ai_ollama::Ollama.parse(neither).err());
        assert!(crate::ai_chat::parse_modern(neither).is_err());
    }
}

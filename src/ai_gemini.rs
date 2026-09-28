//! Google Gemini's generateContent API: the request URL (the key and method ride in it), the
//! body with its schema sanitizer and thought signatures, and the parsers of whole answers and
//! streamed events (each SSE event is a whole response). Pure; ports `AITerm/Gemini.swift`
//! (`GE:8-479`) and `LLMProvider.url` (`P:18-39`).

use crate::ai_json::{arguments, splice_at_key};
use crate::ai_model::mime_is_textual;
use crate::ai_msg::{Attachment, Body, FunctionCall, Message, Role};
use crate::ai_policy;
use crate::ai_turn::{Build, Dialect, Parsed};
use base64::Engine;
use serde_json::{json, Map, Value};

pub struct Gemini;

/// Google's documented stand-in for a call made without a signature (`GE:70`); a wire string.
pub const THOUGHT_SIGNATURE: &str = "context_engineering_is_the_way_to_go";

/// A failed candidate that broke Gemini's safety rules (`GE:404-406`).
pub const SAFETY: &str = "The request violated Gemini's safety rules.";

/// A candidate that failed for any other reason (`GE:407-409`).
pub fn failed_reason(reason: &str) -> String {
    format!("Failed to generate a response with reason: {reason}.")
}

/// `requiresThoughtSignature` (`GE:81-91`): generation 3 and later want a signature on every
/// replayed call; only a name whose generation parses as under 3 opts out, so an alias such as
/// "gemini-flash-latest" or a manual name gets one.
pub fn needs_signature(model_name: &str) -> bool {
    let lowered = model_name.to_lowercase();
    let Some(at) = lowered.find("gemini-") else {
        return true;
    };
    let digits: String = lowered[at + "gemini-".len()..].chars().take_while(|c| c.is_numeric()).collect();
    !matches!(digits.parse::<i64>(), Ok(major) if major < 3)
}

const ABOUT_EMPTY: &str = "about:empty";

/// `%XX` for every byte of `text` that `keep` does not allow.
fn percent_encoded(text: &str, keep: impl Fn(u8) -> bool) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if keep(b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"-._~".contains(&b)
}

/// `LLMProvider.url` for Gemini (`P:18-39`): the method suffix is stripped, `{{MODEL}}` becomes
/// the model name, the method for `stream` is appended, and the query is replaced by the key
/// (and `alt=sse` when streaming). "about:empty" when Foundation would make no URL.
pub fn url_for(model_url: &str, model_name: &str, key: &str, stream: bool) -> String {
    let Some(parts) = ai_policy::url_parts(model_url) else {
        return ABOUT_EMPTY.to_string();
    };
    let origin = &model_url[..model_url.len() - parts.rest.len()];
    let (before_fragment, fragment) = match parts.rest.find('#') {
        Some(at) => parts.rest.split_at(at),
        None => (parts.rest, ""),
    };
    let path = before_fragment.split('?').next().unwrap_or_default();
    let path = path.strip_suffix(":streamGenerateContent").unwrap_or(path);
    let path = path.strip_suffix(":generateContent").unwrap_or(path);
    // URLComponents percent-encodes what a path cannot hold.
    let name = percent_encoded(model_name, |b| unreserved(b) || b"!$&'()*+,;=:@/".contains(&b));
    let mut path = path.replace("{{MODEL}}", &name);
    path.push_str(if stream { ":streamGenerateContent" } else { ":generateContent" });
    // A path that does not start with "/" after an authority makes no URL.
    if !path.starts_with('/') {
        return ABOUT_EMPTY.to_string();
    }
    let mut url = format!("{origin}{path}?key={}", percent_encoded(key, unreserved));
    if stream {
        url.push_str("&alt=sse");
    }
    url.push_str(fragment);
    url
}

/// `sanitizingFunctionParameterSchema` (`GE:120-175`): drops `additionalProperties`, turns a
/// type list into its first non-null type plus `nullable`, and a string `items` into a schema.
/// Member names under `properties`, `$defs`, `definitions` and `patternProperties` are kept
/// verbatim, and literal values (`enum`, `const`, `default`, `examples`) are copied.
pub fn sanitize(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, value) in map {
                match key.as_str() {
                    "additionalProperties" => {}
                    "type" => match value {
                        Value::Array(types) => {
                            let names: Vec<&str> = types.iter().filter_map(Value::as_str).collect();
                            if let Some(name) = names.iter().find(|n| **n != "null") {
                                out.insert(key.clone(), json!(name));
                            }
                            if names.contains(&"null") {
                                out.insert("nullable".into(), json!(true));
                            }
                        }
                        other => {
                            out.insert(key.clone(), other.clone());
                        }
                    },
                    "items" => {
                        let items = match value {
                            Value::String(name) => json!({"type": name}),
                            other => sanitize(other),
                        };
                        out.insert(key.clone(), items);
                    }
                    "properties" | "$defs" | "definitions" | "patternProperties" => {
                        let members = match value {
                            Value::Object(members) => Value::Object(members.iter().map(|(name, s)| (name.clone(), sanitize(s))).collect()),
                            other => sanitize(other),
                        };
                        out.insert(key.clone(), members);
                    }
                    "enum" | "const" | "default" | "examples" => {
                        out.insert(key.clone(), value.clone());
                    }
                    _ => {
                        out.insert(key.clone(), sanitize(value));
                    }
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(sanitize).collect()),
        other => other.clone(),
    }
}

/// A call's part, or None for a call without a name; a call without a captured signature gets
/// `fallback` (`GE:241-248`).
fn call_part(call: &FunctionCall, fallback: Option<&str>) -> Option<Value> {
    let name = call.name.as_deref()?;
    let mut part = json!({"functionCall": {"name": name, "args": arguments(call.arguments.as_deref())}});
    if let Some(signature) = call.thought_signature.as_deref().or(fallback) {
        part["thoughtSignature"] = json!(signature);
    }
    Some(part)
}

fn response_part(name: &str, output: &str) -> Value {
    json!({"functionResponse": {"name": name, "response": {"output": output}}})
}

/// One part of a multipart message (`GE:262-310`): textual files (SVG included) as text, other
/// files as inline data; statuses, file ids and nested multiparts are dropped.
fn part(body: &Body, fallback: Option<&str>) -> Option<Value> {
    match body {
        Body::Uninitialized | Body::Multipart(_) => None,
        Body::Text(text) => Some(json!({"text": text})),
        Body::FunctionCall(call, _) => call_part(call, fallback),
        Body::FunctionOutput { name, output, .. } => Some(response_part(name, output)),
        Body::Attachment(Attachment::Code(code)) => Some(json!({"text": code})),
        Body::Attachment(Attachment::Status { .. } | Attachment::FileId { .. }) => None,
        Body::Attachment(Attachment::File { mime, content, .. }) => Some(if mime_is_textual(mime) {
            json!({"text": String::from_utf8_lossy(content)})
        } else {
            json!({"inlineData": {"mime_type": mime, "data": base64::engine::general_purpose::STANDARD.encode(content)}})
        }),
    }
}

/// One message's content (`GE:219-317`), or None for a system, roleless or empty one. A
/// function message is the model's when it makes a call, else the user's answer.
fn content(m: &Message, fallback: Option<&str>) -> Option<Value> {
    let role = match m.role? {
        Role::User => "user",
        Role::Assistant => "model",
        Role::System => return None,
        Role::Function if m.function_call().is_some() => "model",
        Role::Function => "user",
    };
    let parts: Vec<Value> = match &m.body {
        Body::Uninitialized | Body::Attachment(_) => Vec::new(),
        Body::Text(text) => vec![json!({"text": text})],
        Body::FunctionCall(call, _) => call_part(call, fallback).into_iter().collect(),
        Body::FunctionOutput { name, output, .. } => vec![response_part(name, output)],
        Body::Multipart(bodies) => bodies.iter().filter_map(|b| part(b, fallback)).collect(),
    };
    if parts.is_empty() {
        return None;
    }
    Some(json!({"role": role, "parts": parts}))
}

fn has_part(content: &Value, key: &str) -> bool {
    content["parts"].as_array().is_some_and(|parts| parts.iter().any(|p| p.get(key).is_some()))
}

/// `coalescingConsecutiveRoles` (`GE:330-344`): adjacent contents of one role merge, except a
/// functionResponse followed by text (Gemini answers that merge with nothing).
fn coalesced(contents: Vec<Value>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for c in contents {
        match out.last_mut() {
            Some(last) if last["role"] == c["role"] && !(has_part(last, "functionResponse") && has_part(&c, "text")) => {
                let more = c["parts"].as_array().cloned().unwrap_or_default();
                if let Some(parts) = last["parts"].as_array_mut() {
                    parts.extend(more);
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// A whole response or one streamed event (`GE:356-411`): the first candidate's parts collapse
/// into one message; with no usable part, a failure reason other than a normal finish becomes
/// the text. `candidates` must be an array, as iTerm2's decoder requires; every other field is
/// read leniently.
fn parsed(v: &Value) -> Result<Parsed, String> {
    let candidates = v["candidates"].as_array().ok_or("the response has no candidates")?;
    let prompt_tokens = v["usageMetadata"]["promptTokenCount"].as_i64();
    let mut p = Parsed { prompt_tokens, ..Parsed::default() };
    let Some(candidate) = candidates.first() else {
        return Ok(p);
    };
    let content = candidate.get("content").filter(|c| c.is_object());
    // No content: failed, probably because of safety.
    let role = match content {
        Some(c) if c["role"] != "model" => Role::User,
        _ => Role::Assistant,
    };
    let parts = content.and_then(|c| c["parts"].as_array()).map(Vec::as_slice).unwrap_or_default();
    let mut bodies = Vec::new();
    for part in parts {
        if let Some(text) = part["text"].as_str().filter(|t| !t.is_empty()) {
            bodies.push(Body::Text(text.to_string()));
        } else if let Some(name) = part["functionCall"]["name"].as_str() {
            let args = match &part["functionCall"]["args"] {
                v @ Value::Object(_) => v.to_string(),
                _ => "{}".to_string(),
            };
            let signature = part["thoughtSignature"].as_str().map(str::to_string);
            bodies.push(Body::FunctionCall(FunctionCall { name: Some(name.to_string()), arguments: Some(args), id: None, thought_signature: signature }, None));
        }
    }
    let body = match bodies.len() {
        0 => match candidate["finishReason"].as_str() {
            None | Some("STOP" | "MAX_TOKENS" | "FINISH_REASON_UNSPECIFIED") => return Ok(p),
            Some("SAFETY") => Body::Text(SAFETY.to_string()),
            Some(reason) => Body::Text(failed_reason(reason)),
        },
        1 => bodies.remove(0),
        _ => Body::Multipart(bodies),
    };
    p.messages.push(Message::new(Some(role), body));
    Ok(p)
}

impl Dialect for Gemini {
    /// The key and the method ride in the URL; no header carries the key (`AUTH:79-80`).
    fn url(&self, b: &Build) -> String {
        url_for(&b.model.url, &b.model.name, b.key, b.stream)
    }

    /// `GeminiRequestBuilder` (`GE:177-352`): declaration order, absent fields omitted; no
    /// model, token limit or temperature. The frozen history goes first in `contents`.
    fn body(&self, b: &Build) -> Result<Vec<u8>, String> {
        let fallback = needs_signature(&b.model.name).then_some(THOUGHT_SIGNATURE);
        let mut messages = b.messages.to_vec();
        if let Some(v) = b.volatile.filter(|v| !v.is_empty()) {
            messages.push(Message::text(Role::User, v));
        }
        let mut body = Map::new();
        if let Some(system) = messages.iter().find(|m| m.role == Some(Role::System)).and_then(Message::content) {
            body.insert("system_instruction".into(), json!({"parts": [{"text": system}]}));
        }
        let contents = coalesced(messages.iter().filter_map(|m| content(m, fallback)).collect());
        body.insert("contents".into(), Value::Array(contents));
        // Gemini takes functions or hosted tools in one call, not both (`GE:197-213`).
        if !b.tools.is_empty() {
            let declarations: Vec<Value> = b.tools.iter().map(|t| json!({"name": t.name, "description": t.description, "parameters": sanitize(&t.parameters)})).collect();
            body.insert("tools".into(), json!([{"functionDeclarations": declarations}]));
        } else if b.hosted.code_interpreter {
            body.insert("tools".into(), json!([{"code_execution": {}}]));
        }
        let bytes = serde_json::to_vec(&Value::Object(body)).expect("a JSON value always encodes");
        splice_at_key(&bytes, "contents", 0, b.frozen)
    }

    fn parse(&self, body: &[u8]) -> Result<Option<Parsed>, String> {
        let v: Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
        parsed(&v).map(Some)
    }

    /// Each SSE event is a whole response (`GE:454-479`); calls carry no id.
    fn parse_event(&self, json: &str) -> Result<Option<Parsed>, String> {
        let v: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
        parsed(&v).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_catalog::Model;
    use crate::ai_model::fixture;
    use crate::ai_msg::{Attachment, Body, FunctionCall, Message, Role};
    use crate::ai_settings::Api;
    use crate::ai_transport::{Options, Proxy, Recorded, Reply};
    use crate::ai_turn::{headers, request, same_outcome, Hosted, Out, ToolDecl, ToolResult, Turn, TurnInput, Wire, EMPTY_RESPONSE};
    use crate::pwm::Secret;
    use serde_json::json;
    use std::sync::Arc;
    use std::time::Duration;

    const GEMINI_STREAM: &str = concat!(
        r#"data: {"candidates":[{"content":{"role":"model","parts":[{"text":"Hel"}]}}]}"#,
        "\r\n\r\n",
        r#"data: {"candidates":[{"content":{"role":"model","parts":[{"text":"lo"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":7}}"#,
        "\r\n\r\n",
    );

    const URL: &str = "https://generativelanguage.googleapis.com/v1beta/models/{{MODEL}}";
    const KEY: &str = "AIzaK";

    fn model() -> Model {
        fixture("gemini-3.7-flash", URL, Api::Gemini)
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
        String::from_utf8(Gemini.body(b).expect("body builds")).expect("UTF-8")
    }

    fn sh() -> Vec<Message> {
        vec![Message::text(Role::System, "S"), Message::text(Role::User, "hi")]
    }

    #[test]
    fn gemini_url_follows_iterm2() {
        let base = "https://generativelanguage.googleapis.com/v1beta/models";
        let at = |url: &str, stream: bool| url_for(url, "gemini-3.7-flash", KEY, stream);
        assert_eq!(at(URL, true), format!("{base}/gemini-3.7-flash:streamGenerateContent?key=AIzaK&alt=sse"));
        assert_eq!(at(URL, false), format!("{base}/gemini-3.7-flash:generateContent?key=AIzaK"));
        for suffix in [":generateContent", ":streamGenerateContent"] {
            assert_eq!(at(&format!("{URL}{suffix}"), true), format!("{base}/gemini-3.7-flash:streamGenerateContent?key=AIzaK&alt=sse"), "{suffix}");
            assert_eq!(at(&format!("{URL}{suffix}"), false), format!("{base}/gemini-3.7-flash:generateContent?key=AIzaK"), "{suffix}");
        }
        assert_eq!(at(&format!("{URL}?x=1"), false), format!("{base}/gemini-3.7-flash:generateContent?key=AIzaK"), "the query is replaced");
        assert_eq!(at(&format!("{base}/pinned"), false), format!("{base}/pinned:generateContent?key=AIzaK"), "no placeholder: the path as written");
        assert_eq!(url_for(URL, "m", "Placeholder for self-hosted", false), format!("{base}/m:generateContent?key=Placeholder%20for%20self-hosted"), "the key is encoded");
        assert_eq!(at("not a url", true), "about:empty");
        assert_eq!(at("https://generativelanguage.googleapis.com", false), "about:empty", "no path: Foundation makes no URL");

        // The dialect builds the URL from the model, the key and the streaming flag.
        let (m, w, none) = (model(), wire(), Hosted::default());
        let messages = sh();
        let mut b = build(&m, &messages, &[], &none, &w);
        assert!(Gemini.url(&b) == format!("{base}/gemini-3.7-flash:streamGenerateContent?key=AIzaK&alt=sse"), "streaming URL");
        b.stream = false;
        assert!(Gemini.url(&b) == format!("{base}/gemini-3.7-flash:generateContent?key=AIzaK"), "non-streaming URL");

        // A URL that does not parse fails the turn with iTerm2's text.
        let broken = TurnInput { model: fixture("gemini-3.7-flash", "not a url", Api::Gemini), legacy_url_pref: "x".into(), ..turn_input() };
        assert_eq!(Turn::new(broken).start(Some(&key())), [Out::Failed("Invalid URL for AI provider of x".into())]);
        let pathless = TurnInput { model: fixture("gemini-3.7-flash", "https://generativelanguage.googleapis.com", Api::Gemini), legacy_url_pref: "y".into(), ..turn_input() };
        assert_eq!(Turn::new(pathless).start(Some(&key())), [Out::Failed("Invalid URL for AI provider of y".into())], "about:empty is not sent");
    }

    #[test]
    fn gemini_body_follows_iterm2() {
        let (m, w, none) = (model(), wire(), Hosted::default());
        let messages = sh();
        let tools = [tool()];
        let b = build(&m, &messages, &tools, &none, &w);
        assert_eq!(
            body(&b),
            r#"{"system_instruction":{"parts":[{"text":"S"}]},"contents":[{"role":"user","parts":[{"text":"hi"}]}],"tools":[{"functionDeclarations":[{"name":"f","description":"d","parameters":{"type":"object","properties":{},"required":[]}}]}]}"#
        );
        let code = Hosted { code_interpreter: true, ..Hosted::default() };
        assert!(body(&build(&m, &messages, &[], &code, &w)).ends_with(r#""tools":[{"code_execution":{}}]}"#), "code interpreter alone");
        assert!(!body(&build(&m, &messages, &tools, &code, &w)).contains("code_execution"), "functions win over the code interpreter");
        assert_eq!(body(&build(&m, &messages, &[], &none, &w)), r#"{"system_instruction":{"parts":[{"text":"S"}]},"contents":[{"role":"user","parts":[{"text":"hi"}]}]}"#, "neither: no tools");
        assert_eq!(body(&build(&m, &messages[1..], &[], &none, &w)), r#"{"contents":[{"role":"user","parts":[{"text":"hi"}]}]}"#, "no system message");

        let h = headers(&b);
        assert!(h.len() == 1 && h[0].0 == "Content-Type" && h[0].1 == "application/json", "only Content-Type");
        let custom = Model { custom_headers: vec![("X-Goog-User-Project".into(), "p".into())], ..model() };
        let names: Vec<String> = headers(&build(&custom, &messages, &[], &none, &w)).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["Content-Type", "X-Goog-User-Project"], "then the custom headers");

        let mut v = build(&m, &messages, &[], &none, &w);
        v.volatile = Some("V");
        assert!(body(&v).contains(r#""contents":[{"role":"user","parts":[{"text":"hi"},{"text":"V"}]}]"#), "the volatile text is a last user message: {}", body(&v));

        let mut frozen = build(&m, &messages, &[], &none, &w);
        frozen.frozen = Some(br#"{"role":"model","parts":[{"text":"old"}]}"#);
        assert!(body(&frozen).contains(r#""contents":[{"role":"model","parts":[{"text":"old"}]},{"role":"user","parts":[{"text":"hi"}]}]"#), "{}", body(&frozen));
    }

    fn call(signature: Option<&str>) -> Message {
        let call = FunctionCall { name: Some("f".into()), arguments: Some("{\"a\":1}".into()), id: None, thought_signature: signature.map(str::to_string) };
        Message::new(Some(Role::Assistant), Body::FunctionCall(call, None))
    }

    fn output() -> Message {
        Message::new(Some(Role::Function), Body::FunctionOutput { name: "f".into(), output: "42".into(), id: None })
    }

    fn file(mime: &str, content: &[u8]) -> Body {
        Body::Attachment(Attachment::File { name: "x".into(), mime: mime.into(), content: content.to_vec() })
    }

    fn user(body: Body) -> Message {
        Message::new(Some(Role::User), body)
    }

    /// The `contents` array of the body for these messages under a model name.
    fn contents_for(name: &str, messages: &[Message]) -> String {
        let (m, w, none) = (fixture(name, URL, Api::Gemini), wire(), Hosted::default());
        let v: Value = serde_json::from_slice(&Gemini.body(&build(&m, messages, &[], &none, &w)).expect("body builds")).expect("JSON");
        v["contents"].to_string()
    }

    fn contents(messages: &[Message]) -> String {
        contents_for("gemini-3.7-flash", messages)
    }

    #[test]
    fn gemini_contents_follow_iterm2() {
        let signed = |s: &str| format!(r#"[{{"role":"model","parts":[{{"functionCall":{{"name":"f","args":{{"a":1}}}},"thoughtSignature":"{s}"}}]}}]"#);
        assert_eq!(contents(&[call(None)]), signed(THOUGHT_SIGNATURE));
        assert_eq!(contents(&[call(Some("sig"))]), signed("sig"), "a captured signature");
        assert_eq!(contents_for("gemini-2.5-flash", &[call(None)]), r#"[{"role":"model","parts":[{"functionCall":{"name":"f","args":{"a":1}}}]}]"#, "pre-3: no signature");
        assert_eq!(contents_for("gemini-2.5-flash", &[call(Some("sig"))]), signed("sig"), "a captured signature always goes back");
        for name in ["gemini-3.7-flash", "gemini-flash-latest", "my-model", "Gemini-3-pro", "models/gemini-10-x"] {
            assert!(needs_signature(name), "{name}");
        }
        for name in ["gemini-1.5-pro", "gemini-2.0-flash", "GEMINI-2.5-flash"] {
            assert!(!needs_signature(name), "{name}");
        }
        let bad = Message::new(Some(Role::Assistant), Body::FunctionCall(FunctionCall { name: Some("f".into()), arguments: Some("nope".into()), ..FunctionCall::default() }, None));
        assert!(contents(&[bad]).contains(r#""args":{}"#), "arguments that are not JSON");
        let nameless = Message::new(Some(Role::Assistant), Body::FunctionCall(FunctionCall::default(), None));
        assert_eq!(contents(&[nameless]), "[]", "a call without a name is dropped");
        let called = Message::new(Some(Role::Function), call(None).body);
        assert!(contents(&[called]).starts_with(r#"[{"role":"model""#), "a function message making a call is the model's");

        assert_eq!(contents(&[output()]), r#"[{"role":"user","parts":[{"functionResponse":{"name":"f","response":{"output":"42"}}}]}]"#);

        let parts = Body::Multipart(vec![
            Body::Text("look".into()),
            file("image/png", &[0, 1, 2]),
            file("text/plain", b"hello"),
            file("image/svg+xml", b"<svg/>"),
            Body::Attachment(Attachment::FileId { id: "file-1".into(), name: "x".into() }),
            Body::Attachment(Attachment::Code("x = 1".into())),
            Body::Uninitialized,
        ]);
        assert_eq!(
            contents(&[user(parts)]),
            r#"[{"role":"user","parts":[{"text":"look"},{"inlineData":{"mime_type":"image/png","data":"AAEC"}},{"text":"hello"},{"text":"<svg/>"},{"text":"x = 1"}]}]"#
        );
        assert_eq!(contents(&[user(file("image/png", &[0, 1, 2]))]), "[]", "a top-level attachment is dropped");
        assert_eq!(contents(&[user(Body::Attachment(Attachment::FileId { id: "file-1".into(), name: "x".into() }))]), "[]");
        assert_eq!(contents(&[Message::new(None, Body::Text("x".into())), user(Body::Uninitialized)]), "[]", "roleless and empty messages are skipped");

        // Consecutive contents of one role merge, except a functionResponse and a user's text.
        assert_eq!(contents(&[Message::text(Role::User, "a"), Message::text(Role::User, "b")]), r#"[{"role":"user","parts":[{"text":"a"},{"text":"b"}]}]"#);
        assert_eq!(
            contents(&[output(), Message::text(Role::User, "b")]),
            r#"[{"role":"user","parts":[{"functionResponse":{"name":"f","response":{"output":"42"}}}]},{"role":"user","parts":[{"text":"b"}]}]"#
        );
        assert_eq!(contents(&[Message::text(Role::User, "b"), output()]).matches(r#""role":"user""#).count(), 1, "text then a response merge");
        assert_eq!(contents(&[call(Some("s")), Message::text(Role::Assistant, "done")]).matches(r#""role":"model""#).count(), 1, "two model turns merge");
    }

    #[test]
    fn gemini_sanitizer_follows_iterm2() {
        let schema = json!({"type":"object","additionalProperties":false,"properties":{"a":{"type":["string","null"]},"b":{"type":"array","items":"string"},"c":{"enum":["x"],"default":"x"}}});
        assert_eq!(
            sanitize(&schema).to_string(),
            r#"{"type":"object","properties":{"a":{"type":"string","nullable":true},"b":{"type":"array","items":{"type":"string"}},"c":{"enum":["x"],"default":"x"}}}"#
        );
        let nested = json!({"type":"object","properties":{"o":{"type":"object","additionalProperties":true,"properties":{}}},"$defs":{"d":{"type":"object","additionalProperties":false}}});
        assert_eq!(sanitize(&nested).to_string(), r#"{"type":"object","properties":{"o":{"type":"object","properties":{}}},"$defs":{"d":{"type":"object"}}}"#);
        let keyword_names = json!({"properties":{"additionalProperties":{"type":"string"},"type":{"type":"integer"}}});
        assert_eq!(sanitize(&keyword_names), keyword_names, "member names are kept verbatim");
        let literal = json!({"default":{"additionalProperties":1,"type":["a","null"]},"const":{"additionalProperties":2}});
        assert_eq!(sanitize(&literal), literal, "literal values are copied");
        assert_eq!(sanitize(&json!({"anyOf":[{"type":["null"]},{"items":{"type":"integer","additionalProperties":false}}]})).to_string(), r#"{"anyOf":[{"nullable":true},{"items":{"type":"integer"}}]}"#);

        // The tool's parameters go out sanitized.
        let (m, w, none) = (model(), wire(), Hosted::default());
        let messages = sh();
        let tools = [ToolDecl { parameters: schema, ..tool() }];
        assert!(body(&build(&m, &messages, &tools, &none, &w)).contains(r#""parameters":{"type":"object","properties":{"a":{"type":"string","nullable":true}"#));
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

    fn streamed(chunks: &[&[u8]]) -> Vec<Out> {
        let (mut r, _) = runner(Turn::new(turn_input()), vec![Reply::ok(chunks)]);
        assert_eq!(r.start(Some(&key())), [Out::WillSend]);
        r.poll()
    }

    fn events(stream: &str) -> Vec<&[u8]> {
        stream.split_inclusive("\r\n\r\n").map(str::as_bytes).collect()
    }

    const CALL_STREAM: &str = concat!(
        r#"data: {"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"f","args":{"a":1}},"thoughtSignature":"sig"}]}}]}"#,
        "\r\n\r\n",
        r#"data: {"candidates":[{"content":{"role":"model","parts":[{"text":""}]},"finishReason":"STOP"}]}"#,
        "\r\n\r\n",
    );

    #[test]
    fn gemini_stream_follows_iterm2() {
        let (mut r, rec) = runner(Turn::new(turn_input()), vec![Reply::ok(&events(GEMINI_STREAM))]);
        assert_eq!(r.start(Some(&key())), [Out::WillSend]);
        assert_eq!(r.poll(), [Out::Delta("Hel".into()), Out::Delta("lo".into()), Out::StreamEnd]);
        assert_eq!(r.turn().prompt_tokens(), Some(7));
        assert!(rec.requests()[0].url.ends_with(":streamGenerateContent?key=AIzaK&alt=sse"), "streaming URL");

        let (mut r, rec) = runner(Turn::new(turn_input()), vec![Reply::ok(&events(CALL_STREAM)), Reply::ok(&[])]);
        assert_eq!(r.start(Some(&key())), [Out::WillSend]);
        let call = FunctionCall { name: Some("f".into()), arguments: Some("{\"a\":1}".into()), id: None, thought_signature: Some("sig".into()) };
        assert_eq!(r.poll(), [Out::Invoke { call, id: None }], "no call id");
        // The call goes back with its signature, and its result as a functionResponse.
        assert_eq!(r.tool_done(ToolResult::Ok("42".into())), [Out::WillSend]);
        let sent = String::from_utf8(rec.requests()[1].body.clone()).unwrap();
        let replay = r#""contents":[{"role":"user","parts":[{"text":"hi"}]},{"role":"model","parts":[{"functionCall":{"name":"f","args":{"a":1}},"thoughtSignature":"sig"}]},{"role":"user","parts":[{"functionResponse":{"name":"f","response":{"output":"42"}}}]}]"#;
        assert!(sent.contains(replay), "{sent}");

        assert!(Gemini.parse_event(r#"{"error":{"message":"m"}}"#).is_err(), "no candidates: the event does not decode");
        assert!(Gemini.parse_event("not json").is_err());
    }

    #[test]
    fn gemini_stream_any_chunking() {
        let expected = streamed(&events(GEMINI_STREAM));
        let whole = GEMINI_STREAM.as_bytes();
        assert!(same_outcome(&streamed(&[whole]), &expected), "one chunk");
        for at in 1..whole.len() {
            let out = streamed(&[&whole[..at], &whole[at..]]);
            assert!(same_outcome(&out, &expected), "split at {at}: {out:?}");
        }
    }

    fn one(body: &str) -> Vec<Out> {
        let (mut r, _) = runner(Turn::new(TurnInput { stream: false, ..turn_input() }), vec![Reply::ok(&[body.as_bytes()])]);
        assert_eq!(r.start(Some(&key())), [Out::WillSend]);
        r.poll()
    }

    #[test]
    fn gemini_parse_follows_iterm2() {
        let (mut r, rec) = runner(Turn::new(TurnInput { stream: false, ..turn_input() }), vec![Reply::ok(&[br#"{"candidates":[{"content":{"role":"model","parts":[{"text":"Hi"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":7}}"#])]);
        assert_eq!(r.start(Some(&key())), [Out::WillSend]);
        assert_eq!(r.poll(), [Out::Choice("Hi".into())]);
        assert_eq!(r.turn().prompt_tokens(), Some(7));
        assert!(rec.requests()[0].url.ends_with(":generateContent?key=AIzaK"), "non-streaming URL");

        assert_eq!(one(r#"{"candidates":[{"finishReason":"SAFETY"}]}"#), [Out::Choice(SAFETY.into())]);
        assert_eq!(one(r#"{"candidates":[{"finishReason":"RECITATION"}]}"#), [Out::Choice("Failed to generate a response with reason: RECITATION.".into())]);
        assert_eq!(failed_reason("OTHER"), "Failed to generate a response with reason: OTHER.");
        for reason in ["STOP", "MAX_TOKENS", "FINISH_REASON_UNSPECIFIED"] {
            assert_eq!(one(&format!(r#"{{"candidates":[{{"content":{{"role":"model","parts":[]}},"finishReason":"{reason}"}}]}}"#)), [Out::Failed(EMPTY_RESPONSE.into())], "{reason}");
        }
        assert_eq!(one(r#"{"candidates":[]}"#), [Out::Failed(EMPTY_RESPONSE.into())], "no candidates");
        assert_eq!(
            one(r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"first"}]}},{"content":{"role":"model","parts":[{"text":"second"}]}}]}"#),
            [Out::Choice("first".into())],
            "only the first candidate counts"
        );

        let p = Gemini.parse(br#"{"candidates":[{"content":{"role":"model","parts":[{"text":"Let me"},{"text":""},{"functionCall":{"name":"f","args":{"a":1}},"thoughtSignature":"sig"}]}}]}"#).unwrap().unwrap();
        let call = FunctionCall { name: Some("f".into()), arguments: Some("{\"a\":1}".into()), id: None, thought_signature: Some("sig".into()) };
        assert_eq!(p.messages.len(), 1);
        assert_eq!((p.messages[0].role, &p.messages[0].body), (Some(Role::Assistant), &Body::Multipart(vec![Body::Text("Let me".into()), Body::FunctionCall(call, None)])));
        assert_eq!(Gemini.parse(br#"{"candidates":[{"content":{"role":"user","parts":[{"text":"x"}]}}]}"#).unwrap().unwrap().messages[0].role, Some(Role::User));
        assert!(Gemini.parse(b"nope").is_err());
        assert!(Gemini.parse(br#"{"error":{"message":"m"}}"#).is_err(), "no candidates: the answer does not decode");
    }

    #[test]
    fn gemini_errors_hide_the_key() {
        let refused = br#"{"error":{"message":"API key not valid"}}"#;
        let text = "Error from Google: HTTP request failed with status 400. API key not valid";
        for stream in [false, true] {
            let (mut r, _) = runner(Turn::new(TurnInput { stream, ..turn_input() }), vec![Reply::status(400, refused)]);
            assert_eq!(r.start(Some(&key())), [Out::WillSend]);
            let out = r.poll();
            assert_eq!(out.last(), Some(&Out::Failed(text.into())), "stream {stream}");
            assert!(!format!("{out:?}").contains(KEY), "no key in the outputs (stream {stream})");
        }

        // The wire log fingerprints a real-shaped key in the URL line.
        let long = format!("AIza{}", "0123456789abcdefghijABCDEFGHIJ_-xyz");
        assert_eq!(long.len(), 39);
        let (m, w, none) = (model(), wire(), Hosted::default());
        let messages = sh();
        let b = Build { key: &long, ..build(&m, &messages, &[], &none, &w) };
        let req = request(&b, &Gemini).expect("builds");
        assert!(req.url.contains(&long), "the key rides in the URL");
        let record = crate::ai_wire_log::request_record("c", "t", &req, Some(&long));
        let url_line = record.lines().nth(1).unwrap_or_default().to_string();
        assert!(!url_line.contains(&long), "the URL line hides the key");
        assert!(url_line.contains("key=AIza0123\u{2026}-xyz [redacted 39 chars]&alt=sse"), "the key is fingerprinted");
        assert!(!format!("{req:?}").contains(&long), "Debug hides the query");

        // A key of any other shape is hidden too (Task 7 review), known or not.
        for odd in [KEY, "not-a-google-key"] {
            let b = Build { key: odd, ..build(&m, &messages, &[], &none, &w) };
            let req = request(&b, &Gemini).expect("builds");
            for known in [Some(odd), None] {
                let record = crate::ai_wire_log::request_record("c", "t", &req, known);
                assert!(!record.contains(odd), "the record hides a {}-character key (known: {})", odd.len(), known.is_some());
                assert!(record.contains("?key=[redacted "), "fingerprinted");
            }
        }

        // The runner logs with the key the request carried.
        let (mut r, _) = runner(Turn::new(turn_input()), vec![Reply::ok(&[])]);
        r.start(Some(&key()));
        assert!(r.turn().wire_key() == Some(KEY), "the turn names the key it sent");
        let local = TurnInput { model: fixture("gemini-3.7-flash", "http://localhost:8080/v1beta/models/{{MODEL}}", Api::Gemini), ..turn_input() };
        let (mut r, _) = runner(Turn::new(local), vec![Reply::ok(&[])]);
        r.start(Some(&key()));
        assert_eq!(r.turn().wire_key(), None, "the self-hosted placeholder is no secret");
    }
}

//! A message to or from a language model: its bodies, the rules that merge streamed parts, the
//! token estimate and the truncation of tool continuations. Pure; ports `AITerm/LLM.swift`
//! (`LLM.Message`, `LLM.swift:180-552`), `LLM+Mac.swift:14-18` and `truncate` (`AITerm.swift:1492-1533`).

use crate::ai_model::tokens;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
    Function,
}

/// A tool call's ids: the call id the answer must quote, and the item id streamed parts carry.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct CallId {
    pub call_id: String,
    pub item_id: String,
}

/// A tool call; every field can be missing from a streamed part.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct FunctionCall {
    pub name: Option<String>,
    pub arguments: Option<String>,
    pub id: Option<String>,
    /// Gemini's per-call signature, echoed back unchanged.
    pub thought_signature: Option<String>,
}

/// What a hosted tool is doing (`StatusUpdate`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    WebSearchStarted,
    WebSearchFinished(Option<String>),
    CodeInterpreterStarted,
    CodeInterpreterFinished,
    ReasoningSummary(String),
    /// Updates merged while streaming (`LLM.swift:113`).
    Multipart(Vec<Status>),
}

impl Status {
    /// The single updates, in order.
    pub fn exploded(&self) -> Vec<Status> {
        match self {
            Status::Multipart(parts) => parts.iter().flat_map(Status::exploded).collect(),
            other => vec![other.clone()],
        }
    }

    /// The text shown for the update (`LLM.swift:534-552`); merged updates show the last
    /// reasoning summary or finished search or finished code run and everything after it.
    pub fn display(&self) -> String {
        match self {
            Status::WebSearchStarted => "Searching the web\u{2026}".to_string(),
            Status::WebSearchFinished(Some(query)) => format!("Finished searching the web for {query}."),
            Status::WebSearchFinished(None) => "Finished searching the web.".to_string(),
            Status::CodeInterpreterStarted => "Executing code\u{2026}".to_string(),
            Status::CodeInterpreterFinished => "Finished executing code".to_string(),
            Status::ReasoningSummary(text) => text.clone(),
            Status::Multipart(_) => {
                let mut parts = self.exploded();
                let keep = parts.iter().rposition(|s| matches!(s, Status::ReasoningSummary(_) | Status::WebSearchFinished(_) | Status::CodeInterpreterFinished));
                if let Some(start) = keep {
                    parts.drain(..start);
                }
                parts.iter().map(Status::display).collect::<Vec<_>>().join("\n")
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Attachment {
    File { name: String, mime: String, content: Vec<u8> },
    FileId { id: String, name: String },
    Code(String),
    Status { id: String, inline: bool, status: Status },
}

impl Attachment {
    /// `Attachment.appending` (`LLM.swift:146-181`): status updates always merge (so the window
    /// is not spammed with them) and code accumulates; files and file ids come whole. Merges in
    /// place: a status already merged extends its own parts vector instead of exploding both
    /// sides into a fresh one, and code appends to the existing string, so a stream of many
    /// parts costs its total length once, not once per part.
    fn append(&mut self, other: &Attachment) -> bool {
        match (self, other) {
            (Attachment::Status { status, .. }, Attachment::Status { status: more, .. }) => {
                if !matches!(status, Status::Multipart(_)) {
                    let first = std::mem::replace(status, Status::Multipart(Vec::new()));
                    let Status::Multipart(parts) = status else { unreachable!() };
                    parts.push(first);
                }
                let Status::Multipart(parts) = status else { unreachable!() };
                parts.extend(more.exploded());
                true
            }
            (Attachment::Code(a), Attachment::Code(b)) => {
                a.push_str(b);
                true
            }
            _ => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    Uninitialized,
    Text(String),
    FunctionCall(FunctionCall, Option<CallId>),
    FunctionOutput { name: String, output: String, id: Option<CallId> },
    Attachment(Attachment),
    Multipart(Vec<Body>),
}

impl Body {
    /// iTerm2's `maybeContent` (`LLM.swift:202-218`): text, a tool's output and code; the parts'
    /// contents joined with "\n"; None for calls, files, file ids and statuses.
    pub fn content(&self) -> Option<String> {
        match self {
            Body::Multipart(parts) => Some(parts.iter().filter_map(Body::content).collect::<Vec<_>>().join("\n")),
            Body::Text(s) | Body::FunctionOutput { output: s, .. } | Body::Attachment(Attachment::Code(s)) => Some(s.clone()),
            Body::Attachment(_) | Body::FunctionCall(..) | Body::Uninitialized => None,
        }
    }

    /// `Body.tryAppend` (`LLM.swift:255-342`): merges a streamed part into this one. Never makes
    /// a multipart, but always succeeds on one.
    pub fn try_append(&mut self, other: &Body) -> bool {
        match self {
            Body::Uninitialized => false,
            Body::Text(original) => match other {
                Body::Text(more) => {
                    original.push_str(more);
                    true
                }
                _ => false,
            },
            Body::FunctionCall(original, original_id) => match other {
                // Only item ids are compared: OpenAI streams arguments without the call id, and
                // DeepSeek sends ids only with a call's first part.
                Body::FunctionCall(more, id) if id.is_none() || id.as_ref().map(|i| &i.item_id) == original_id.as_ref().map(|i| &i.item_id) => {
                    // Appended in place, so a call streamed in many parts costs its length once.
                    if original.id.is_some() || more.id.is_some() {
                        // The ids concatenate, as iTerm2's.
                        original.id.get_or_insert_with(String::new).push_str(more.id.as_deref().unwrap_or(""));
                    }
                    original.name.get_or_insert_with(String::new).push_str(more.name.as_deref().unwrap_or(""));
                    original.arguments.get_or_insert_with(String::new).push_str(more.arguments.as_deref().unwrap_or(""));
                    if original.thought_signature.is_none() {
                        original.thought_signature = more.thought_signature.clone();
                    }
                    true
                }
                // Anthropic streams a call's arguments as text.
                Body::Text(more) => {
                    original.name = Some(original.name.take().unwrap_or_default());
                    original.arguments.get_or_insert_with(String::new).push_str(more);
                    true
                }
                _ => false,
            },
            Body::FunctionOutput { name, output, id } => match other {
                Body::FunctionOutput { name: more_name, output: more, id: more_id } if more_id == id => {
                    name.push_str(more_name);
                    output.push_str(more);
                    true
                }
                _ => false,
            },
            Body::Attachment(original) => match other {
                Body::Attachment(more) => original.append(more),
                _ => false,
            },
            Body::Multipart(parts) => {
                let merged = parts.last_mut().is_some_and(|last| last.try_append(other));
                if !merged {
                    parts.push(other.clone());
                }
                true
            }
        }
    }
}

/// One reasoning item of an OpenAI Responses turn, replayed before its calls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReasoningItem {
    pub id: String,
    pub encrypted_content: Option<String>,
    pub summary: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub role: Option<Role>,
    pub body: Body,
    pub response_id: Option<String>,
    /// DeepSeek's reasoning, echoed back on later requests.
    pub reasoning_content: Option<String>,
    pub reasoning_items: Vec<ReasoningItem>,
}

impl Message {
    pub fn text(role: Role, s: &str) -> Message {
        Message::new(Some(role), Body::Text(s.to_string()))
    }

    pub fn new(role: Option<Role>, body: Body) -> Message {
        Message { role, body, response_id: None, reasoning_content: None, reasoning_items: Vec::new() }
    }

    pub fn try_append(&mut self, other: &Body) -> bool {
        self.body.try_append(other)
    }

    /// The call and its ids; for a multipart body the first call part (`LLM.swift:347-365`).
    pub fn function_call(&self) -> Option<(&FunctionCall, Option<&CallId>)> {
        match &self.body {
            Body::FunctionCall(call, id) => Some((call, id.as_ref())),
            Body::Multipart(parts) => parts.iter().find_map(|p| match p {
                Body::FunctionCall(call, id) => Some((call, id.as_ref())),
                _ => None,
            }),
            _ => None,
        }
    }

    /// The ids of the call or the output this message carries, the first part's for a
    /// multipart body (`LLM.swift:366-380`).
    pub fn call_id(&self) -> Option<&CallId> {
        match &self.body {
            Body::FunctionCall(_, id) | Body::FunctionOutput { id, .. } => id.as_ref(),
            Body::Multipart(parts) => parts.iter().find_map(|p| match p {
                Body::FunctionCall(_, id) | Body::FunctionOutput { id, .. } => id.as_ref(),
                _ => None,
            }),
            _ => None,
        }
    }

    /// `approximateTokenCount` (`LLM+Mac.swift:14`).
    pub fn approx_tokens(&self) -> i64 {
        tokens(&self.content().unwrap_or_default()) + 1
    }

    pub fn content(&self) -> Option<String> {
        self.body.content()
    }

    /// `trimmedString` (`LLM+Mac.swift:16-18`): the content (or "") without leading
    /// whitespace; iTerm2's is never nil.
    pub fn trimmed_text(&self) -> Option<String> {
        Some(self.content().unwrap_or_default().trim_start().to_string())
    }
}

/// Marks the middle cut out of a message too long to send.
pub const TRUNCATED: &str = "\u{2026}[truncated]\u{2026}";

/// The tokens a tool continuation may use (`AIConversation.swift:137-147`): the token limit less
/// the response's share, at most half of it.
pub fn context_limit(token_limit: i64, response_limit: i64) -> i64 {
    token_limit - (token_limit / 2).min(response_limit)
}

/// The first half of the characters and the rest.
fn halved(s: &str) -> (String, String) {
    let middle = s.chars().count() / 2;
    let at = s.char_indices().nth(middle).map_or(s.len(), |(i, _)| i);
    (s[..at].to_string(), s[at..].to_string())
}

/// iTerm2's `truncate` (`AITerm.swift:1492-1533`): while over `max_tokens`, drop the oldest
/// messages that are not system messages; the last one is instead halved repeatedly around
/// `TRUNCATED`. Where iTerm2 would loop forever (a last message that cannot shrink), it stops.
pub fn truncate(messages: &[Message], max_tokens: i64) -> Vec<Message> {
    let mut tokens: i64 = messages.iter().map(Message::approx_tokens).sum();
    let mut out = messages.to_vec();
    let mut j = 0;
    for (i, m) in messages.iter().enumerate() {
        if tokens < max_tokens {
            break;
        }
        if m.role == Some(Role::System) {
            j += 1;
            continue;
        }
        if i + 1 < messages.len() {
            tokens -= m.approx_tokens();
            out.remove(j);
            continue;
        }
        let (mut head, mut tail) = halved(&out[j].content().unwrap_or_default());
        while tokens >= max_tokens {
            let before = out[j].clone();
            head = halved(&head).0;
            tail = halved(&tail).1;
            let cut = format!("{head}{TRUNCATED}{tail}");
            tokens -= out[j].approx_tokens();
            match &mut out[j].body {
                Body::Text(_) | Body::Attachment(_) | Body::Multipart(_) => out[j].body = Body::Text(cut),
                Body::FunctionOutput { output, .. } => *output = cut,
                Body::Uninitialized | Body::FunctionCall(..) => {}
            }
            tokens += out[j].approx_tokens();
            if out[j] == before {
                break;
            }
        }
        j += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: Option<&str>, args: &str, id: Option<&str>) -> FunctionCall {
        FunctionCall { name: name.map(str::to_string), arguments: Some(args.to_string()), id: id.map(str::to_string), thought_signature: None }
    }

    fn ids(item: &str) -> Option<CallId> {
        Some(CallId { call_id: format!("call-{item}"), item_id: item.to_string() })
    }

    fn status(s: Status) -> Body {
        Body::Attachment(Attachment::Status { id: "st".into(), inline: true, status: s })
    }

    #[test]
    fn try_append_follows_iterm2() {
        let mut b = Body::Text("a".into());
        assert!(b.try_append(&Body::Text("b".into())));
        assert_eq!(b, Body::Text("ab".into()));
        assert!(!b.try_append(&Body::FunctionCall(call(Some("f"), "", None), None)), "text + call");

        let mut c = Body::FunctionCall(call(Some("f"), "", Some("x")), ids("i1"));
        assert!(c.try_append(&Body::FunctionCall(call(None, "{\"a\"", Some("y")), Some(CallId { call_id: String::new(), item_id: "i1".into() }))));
        assert_eq!(c, Body::FunctionCall(FunctionCall { name: Some("f".into()), arguments: Some("{\"a\"".into()), id: Some("xy".into()), thought_signature: None }, ids("i1")), "the first CallId is kept, the ids concatenate");
        assert!(!c.try_append(&Body::FunctionCall(call(None, "z", None), ids("i2"))), "another item");
        assert!(c.try_append(&Body::FunctionCall(FunctionCall { thought_signature: Some("sig".into()), ..call(None, "", None) }, None)), "no CallId merges");
        assert!(c.try_append(&Body::Text(":1}".into())), "Anthropic does this");
        assert!(c.try_append(&Body::FunctionCall(FunctionCall { thought_signature: Some("later".into()), ..call(None, "", None) }, None)));
        assert_eq!(c, Body::FunctionCall(FunctionCall { name: Some("f".into()), arguments: Some("{\"a\":1}".into()), id: Some("xy".into()), thought_signature: Some("sig".into()) }, ids("i1")));

        let mut plain = Body::FunctionCall(call(Some("g"), "", None), None);
        assert!(plain.try_append(&Body::FunctionCall(call(None, "{}", None), None)));
        assert_eq!(plain, Body::FunctionCall(call(Some("g"), "{}", None), None), "no ids stay none");

        let mut u = Body::Uninitialized;
        assert!(!u.try_append(&Body::Text("x".into())));
        assert_eq!(u, Body::Uninitialized);

        let mut m = Body::Multipart(vec![Body::Text("a".into())]);
        assert!(m.try_append(&Body::Text("b".into())));
        assert!(m.try_append(&Body::FunctionCall(call(Some("f"), "", None), None)));
        assert_eq!(m, Body::Multipart(vec![Body::Text("ab".into()), Body::FunctionCall(call(Some("f"), "", None), None)]));
        let mut empty = Body::Multipart(vec![]);
        assert!(empty.try_append(&Body::Text("x".into())));
        assert_eq!(empty, Body::Multipart(vec![Body::Text("x".into())]));

        let mut o = Body::FunctionOutput { name: "f".into(), output: "1".into(), id: ids("i") };
        assert!(o.try_append(&Body::FunctionOutput { name: String::new(), output: "2".into(), id: ids("i") }));
        assert!(!o.try_append(&Body::FunctionOutput { name: String::new(), output: "3".into(), id: None }));
        assert_eq!(o, Body::FunctionOutput { name: "f".into(), output: "12".into(), id: ids("i") });

        let mut s = status(Status::WebSearchStarted);
        assert!(s.try_append(&status(Status::WebSearchFinished(Some("rust".into())))));
        assert!(s.try_append(&status(Status::ReasoningSummary("r".into()))));
        let Body::Attachment(Attachment::Status { status: merged, .. }) = &s else { panic!("{s:?}") };
        assert_eq!(merged.exploded(), [Status::WebSearchStarted, Status::WebSearchFinished(Some("rust".into())), Status::ReasoningSummary("r".into())]);
        assert!(!s.try_append(&Body::Attachment(Attachment::Code("x".into()))));
        let mut code = Body::Attachment(Attachment::Code("a".into()));
        assert!(code.try_append(&Body::Attachment(Attachment::Code("b".into()))));
        assert_eq!(code, Body::Attachment(Attachment::Code("ab".into())));
        let mut file = Body::Attachment(Attachment::FileId { id: "f".into(), name: "n".into() });
        assert!(!file.try_append(&Body::Attachment(Attachment::FileId { id: "f".into(), name: "n".into() })));
    }

    /// A small merge, checked against exactly what the pre-fix rebuild-per-part algorithm
    /// produced (final-rereview N1): five statuses still explode to the same five, in order.
    #[test]
    fn status_merge_matches_previous_result() {
        let mut s = status(Status::ReasoningSummary("a".into()));
        for text in ["b", "c", "d", "e"] {
            assert!(s.try_append(&status(Status::ReasoningSummary(text.into()))));
        }
        let Body::Attachment(Attachment::Status { status: merged, .. }) = &s else { panic!("{s:?}") };
        assert_eq!(
            merged.exploded(),
            [Status::ReasoningSummary("a".into()), Status::ReasoningSummary("b".into()), Status::ReasoningSummary("c".into()), Status::ReasoningSummary("d".into()), Status::ReasoningSummary("e".into())]
        );
    }

    /// N1: Ollama streams one thinking token per NDJSON line, each becoming a status merge.
    /// The pre-fix code exploded and rebuilt the whole list on every merge (O(k^2) clones for
    /// k lines); merging in place keeps a realistic thinking stream well under a second.
    #[test]
    fn status_merge_of_30000_thinking_lines_is_fast() {
        let mut s = status(Status::ReasoningSummary("0".into()));
        let start = std::time::Instant::now();
        for i in 1..30_000 {
            assert!(s.try_append(&status(Status::ReasoningSummary(i.to_string()))));
        }
        let elapsed = start.elapsed();
        assert!(elapsed < std::time::Duration::from_secs(2), "30,000 thinking lines took {elapsed:?}, the merge is quadratic again");
        let Body::Attachment(Attachment::Status { status: merged, .. }) = &s else { panic!("{s:?}") };
        let exploded = merged.exploded();
        assert_eq!(exploded.len(), 30_000);
        assert_eq!(exploded[0], Status::ReasoningSummary("0".into()));
        assert_eq!(exploded[29_999], Status::ReasoningSummary("29999".into()));
    }

    /// A small merge, checked against exactly what the pre-fix `format!("{a}{b}")` concat
    /// produced: the parts joined in order.
    #[test]
    fn code_merge_matches_previous_result() {
        let mut c = Body::Attachment(Attachment::Code("a".into()));
        for text in ["b", "c", "d", "e"] {
            assert!(c.try_append(&Body::Attachment(Attachment::Code(text.into()))));
        }
        assert_eq!(c, Body::Attachment(Attachment::Code("abcde".into())));
    }

    /// The same rebuild shape as N1 (`format!("{a}{b}")` copies the whole accumulated string on
    /// every code-interpreter delta); merging in place keeps it fast.
    #[test]
    fn code_merge_of_30000_deltas_is_fast() {
        let mut c = Body::Attachment(Attachment::Code(String::new()));
        let start = std::time::Instant::now();
        for _ in 0..30_000 {
            assert!(c.try_append(&Body::Attachment(Attachment::Code("x".into()))));
        }
        let elapsed = start.elapsed();
        assert!(elapsed < std::time::Duration::from_secs(2), "30,000 code deltas took {elapsed:?}, the merge is quadratic again");
        assert_eq!(c, Body::Attachment(Attachment::Code("x".repeat(30_000))));
    }

    #[test]
    fn content_and_calls_follow_iterm2() {
        let m = Message::new(
            Some(Role::Assistant),
            Body::Multipart(vec![
                Body::Text("pre".into()),
                status(Status::WebSearchStarted),
                Body::FunctionCall(call(Some("f"), "{}", Some("c")), ids("i")),
                Body::Attachment(Attachment::Code("x=1".into())),
            ]),
        );
        assert_eq!(m.content().as_deref(), Some("pre\nx=1"));
        assert_eq!(m.function_call(), Some((&call(Some("f"), "{}", Some("c")), ids("i").as_ref())));
        assert_eq!(m.call_id(), ids("i").as_ref());
        assert_eq!(m.approx_tokens(), 3 + 1);
        let out = Message::new(Some(Role::Function), Body::FunctionOutput { name: "f".into(), output: "42".into(), id: ids("o") });
        assert_eq!(out.content().as_deref(), Some("42"));
        assert_eq!(out.call_id(), ids("o").as_ref());
        assert_eq!(out.function_call(), None);
        assert_eq!(Message::text(Role::User, "  \n hi ").trimmed_text().as_deref(), Some("hi "));
        let file = Message::new(Some(Role::User), Body::Attachment(Attachment::File { name: "a.txt".into(), mime: "text/plain".into(), content: b"abc".to_vec() }));
        assert_eq!(file.content(), None);
        assert_eq!(file.trimmed_text().as_deref(), Some(""), "never nil, as iTerm2's");
        assert_eq!(file.approx_tokens(), 1);
    }

    #[test]
    fn status_texts_follow_iterm2() {
        assert_eq!(Status::WebSearchStarted.display(), "Searching the web\u{2026}");
        assert_eq!(Status::WebSearchFinished(Some("rust".into())).display(), "Finished searching the web for rust.");
        assert_eq!(Status::WebSearchFinished(None).display(), "Finished searching the web.");
        assert_eq!(Status::CodeInterpreterStarted.display(), "Executing code\u{2026}");
        assert_eq!(Status::CodeInterpreterFinished.display(), "Finished executing code");
        assert_eq!(Status::ReasoningSummary("thinking".into()).display(), "thinking");
        let merged = Status::Multipart(vec![Status::WebSearchStarted, Status::ReasoningSummary("r".into()), Status::Multipart(vec![Status::CodeInterpreterStarted])]);
        assert_eq!(merged.display(), "r\nExecuting code\u{2026}", "from the last long update on");
        assert_eq!(Status::Multipart(vec![Status::WebSearchStarted, Status::CodeInterpreterStarted]).display(), "Searching the web\u{2026}\nExecuting code\u{2026}");
    }

    #[test]
    fn truncate_follows_iterm2() {
        assert_eq!(context_limit(128000, 8192), 119808);
        assert_eq!(context_limit(10000, 8000), 5000);
        let long = format!("{}{}", "a".repeat(200), "b".repeat(200));
        let messages = vec![Message::text(Role::System, "S"), Message::text(Role::User, &"u".repeat(200)), Message::text(Role::Assistant, &"x".repeat(200)), Message::text(Role::User, &long)];
        assert_eq!(messages.iter().map(Message::approx_tokens).collect::<Vec<_>>(), [1, 101, 101, 201]);

        // 404 tokens against 250: dropping the first user and the assistant is enough.
        assert_eq!(truncate(&messages, 250), vec![messages[0].clone(), messages[3].clone()]);

        // Against 150 the last is halved too: 200 a's and 200 b's become 100 of each.
        let cut = truncate(&messages, 150);
        assert_eq!(cut.len(), 2);
        assert_eq!(cut[0], messages[0]);
        assert_eq!(cut[1], Message::text(Role::User, &format!("{}{TRUNCATED}{}", "a".repeat(100), "b".repeat(100))));

        // Halved repeatedly until under the limit.
        let cut = truncate(&messages, 40);
        assert_eq!(cut[1], Message::text(Role::User, &format!("{}{TRUNCATED}{}", "a".repeat(25), "b".repeat(25))));
        assert!(cut.iter().map(Message::approx_tokens).sum::<i64>() < 40);

        assert_eq!(truncate(&messages, 1000), messages, "under the limit: unchanged");

        let output = vec![Message::text(Role::System, "S"), Message::new(Some(Role::Function), Body::FunctionOutput { name: "f".into(), output: long.clone(), id: ids("i") })];
        assert_eq!(
            truncate(&output, 150)[1],
            Message::new(Some(Role::Function), Body::FunctionOutput { name: "f".into(), output: format!("{}{TRUNCATED}{}", "a".repeat(100), "b".repeat(100)), id: ids("i") }),
            "a tool's output is halved in place"
        );

        let stuck = vec![Message::new(Some(Role::Assistant), Body::FunctionCall(call(Some("f"), "{}", None), None))];
        assert_eq!(truncate(&stuck, 0), stuck, "a message that cannot shrink ends the loop");
        let multibyte = vec![Message::text(Role::User, &"\u{e9}".repeat(100))];
        let cut = truncate(&multibyte, 60);
        assert_eq!(cut[0].content().unwrap(), format!("{}{TRUNCATED}{}", "\u{e9}".repeat(25), "\u{e9}".repeat(25)), "characters, not bytes");
    }
}

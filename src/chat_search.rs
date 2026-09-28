//! Search across chats (`ChatSearchResultsViewController`, `CSR`): the query tokens, the text a
//! message contributes, iTerm2's word filter, the 200-character snippet and its highlight, and
//! the streamed work (a debounce, then the hits in 10 ms slices). Pure.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crate::chat_json::{self, Attachment};
use crate::chat_message::{Content, Message, Participant, Subpart};
use crate::palette::Palette;
use crate::porthole::detect::Language;

/// The longest snippet (`CSR:117`).
pub const SNIPPET_LENGTH: usize = 200;
/// The pause between two slices of work (`CSR:32-55`).
pub const TICK: Duration = Duration::from_millis(20);
/// The most time one slice spends (`CSR:100-113`).
pub const SLICE: Duration = Duration::from_millis(10);
/// The wait after the last keystroke before the store is asked (ruling F28).
pub const DEBOUNCE: Duration = Duration::from_millis(200);

/// One result: a message and the snippet that shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub chat: String,
    pub message: String,
    pub user: bool,
    pub snippet: String,
}

/// `query.components(separatedBy: .whitespacesAndNewlines)`: empties are kept.
pub fn filter_tokens(query: &str) -> Vec<String> {
    query.split(char::is_whitespace).map(str::to_string).collect()
}

/// A markdown text as the person reads it (`AttributedStringForGPTMarkdown(..).string`).
pub fn markdown_text(md: &str) -> String {
    crate::porthole::render::render(md, &Language::Markdown, &Palette::default(), true).plain()
}

/// What a message contributes to the search (`indexableString`, `CSR:199-241`). A model-only
/// context part is never indexed (ruling F8); the explanation arms are 12e's (ruling 12c-3).
pub fn indexable(content: &Content) -> Option<String> {
    match content {
        Content::PlainText { text, .. } | Content::Append { text, .. } => Some(text.clone()),
        Content::AppendAttachment { attachment, .. } => Some(match chat_json::attachment(attachment) {
            Some((_, Attachment::Code(code))) => code,
            _ => String::new(),
        }),
        Content::Multipart { parts, .. } => {
            let texts: Vec<String> = parts
                .iter()
                .filter_map(|p| match p {
                    Subpart::PlainText(t) | Subpart::Markdown(t) => Some(t.clone()),
                    Subpart::Attachment(v) => match chat_json::attachment(v) {
                        Some((_, Attachment::Code(code))) => Some(code),
                        _ => None,
                    },
                    Subpart::Context(_) => None,
                })
                .collect();
            Some(texts.join("\n"))
        }
        Content::Markdown(md) => Some(markdown_text(md)),
        Content::RemoteCommandResponse { result, .. } => result.as_ref().ok().cloned(),
        Content::TerminalCommand(tc) => Some(tc.command.clone()),
        Content::WatcherEvent(v) => chat_json::watcher(v).map(|(_, detail)| detail),
        Content::ExplanationRequest(_)
        | Content::ExplanationResponse(_)
        | Content::RemoteCommandRequest { .. }
        | Content::SelectSessionRequest { .. }
        | Content::ClientLocal(_)
        | Content::RenameChat(_)
        | Content::Commit(_)
        | Content::SetPermissions(_)
        | Content::VectorStoreCreated(_)
        | Content::UserCommand(_)
        | Content::Unsupported(_) => None,
    }
}

/// `components(separatedBy: .whitespaces)` without the empties: Swift's set is the space
/// separators and the tab, not the line breaks.
fn words(text: &str) -> Vec<&str> {
    let space = |c: char| c == '\t' || (c.is_whitespace() && !matches!(c, '\n' | '\r' | '\u{b}' | '\u{c}' | '\u{85}' | '\u{2028}' | '\u{2029}'));
    text.split(space).filter(|w| !w.is_empty()).collect()
}

/// Every token is in some word of the text (`CSR:250-257`): the word lowercased and the token
/// not (a kept quirk).
pub fn validate(text: &str, tokens: &[String]) -> bool {
    let words = words(text);
    tokens.iter().all(|t| words.iter().any(|w| w.to_lowercase().contains(t.as_str())))
}

/// At most `max` characters around the matching words (`CSR:259-335`), highlighted: two words
/// each side of a match, overlapping or adjacent stretches merged, stretches added with an
/// ellipsis between while they fit.
pub fn snippetize(text: &str, tokens: &[String], max: usize) -> String {
    if text.chars().count() <= max {
        return highlight(text, tokens);
    }
    let words = words(text);
    let lowered: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
    let mut ranges: Vec<(usize, usize)> = lowered
        .iter()
        .enumerate()
        .filter(|(_, w)| tokens.iter().any(|t| w.contains(t.as_str())))
        .map(|(i, _)| (i.saturating_sub(2), (i + 2).min(words.len() - 1)))
        .collect();
    ranges.sort_by_key(|r| r.0);
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for r in ranges {
        match merged.last_mut() {
            Some(last) if r.0 <= last.1 + 1 => last.1 = last.1.max(r.1),
            _ => merged.push(r),
        }
    }
    let mut pieces: Vec<String> = Vec::new();
    let mut length = 0;
    for (i, (start, end)) in merged.into_iter().enumerate() {
        let piece = words[start..=end].join(" ");
        let extra = usize::from(i > 0) + piece.chars().count();
        if length + extra > max {
            break;
        }
        if i > 0 {
            pieces.push("\u{2026}".to_string());
            length += 1;
        }
        length += piece.chars().count();
        pieces.push(piece);
    }
    highlight(&pieces.join(" "), tokens)
}

/// The lowercased text with each token's matches bold (`CSR:337-360`), one token after the
/// other; a match that touches the one before it joins it.
pub fn highlight(text: &str, tokens: &[String]) -> String {
    let mut result = text.to_lowercase();
    for token in tokens {
        let token = token.to_lowercase();
        if token.is_empty() {
            continue;
        }
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        let mut from = 0;
        while let Some(at) = result[from..].find(&token) {
            let (start, end) = (from + at, from + at + token.len());
            match ranges.last_mut() {
                Some(last) if last.1 >= start => last.1 = end,
                _ => ranges.push((start, end)),
            }
            from = end;
        }
        for (start, end) in ranges.into_iter().rev() {
            let bold = format!("**{}**", &result[start..end]);
            result.replace_range(start..end, &bold);
        }
    }
    result
}

/// The snippet of a message for a query; None unless it passes the filter and is not empty
/// (`CSR:115-129`).
pub fn result_snippet(content: &Content, query: &str) -> Option<String> {
    let text = indexable(content)?;
    let tokens = filter_tokens(query);
    if !validate(&text, &tokens) {
        return None;
    }
    Some(snippetize(&text, &tokens, SNIPPET_LENGTH)).filter(|s| !s.is_empty())
}

/// A snippet cut at its `**` marks: (text, bold) runs, for drawing the highlight.
pub fn bold_runs(snippet: &str) -> Vec<(String, bool)> {
    snippet.split("**").enumerate().filter(|(_, t)| !t.is_empty()).map(|(i, t)| (t.to_string(), i % 2 == 1)).collect()
}

/// What one `tick` did.
#[derive(Debug, Default, PartialEq)]
pub struct Tick {
    /// The hits or the searching flag changed.
    pub changed: bool,
    /// When to call `tick` again; None when nothing is left to do.
    pub next: Option<Instant>,
    /// The store could not search.
    pub error: Option<String>,
}

/// The search as the app runs it: the query, its debounce, the pairs still to turn into hits
/// and the messages of each chat loaded once.
#[derive(Default)]
pub struct State {
    pub query: String,
    due: Option<Instant>,
    pending: VecDeque<(String, String)>,
    running: bool,
    hits: Vec<Hit>,
    loaded: HashMap<String, HashMap<String, (Content, bool)>>,
}

impl State {
    /// A new query drops the old work; an empty one ends the search. True when it changed.
    pub fn set_query(&mut self, query: &str, now: Instant) -> bool {
        if query == self.query {
            return false;
        }
        self.query = query.to_string();
        self.hits.clear();
        self.pending.clear();
        self.loaded.clear();
        self.running = false;
        self.due = (!query.is_empty()).then_some(now + DEBOUNCE);
        true
    }

    pub fn hits(&self) -> &[Hit] {
        &self.hits
    }

    /// Work is waiting for the debounce or for a slice.
    pub fn searching(&self) -> bool {
        self.due.is_some() || !self.pending.is_empty()
    }

    /// Runs what is due: the store's search after the debounce, then at most `SLICE` of hits
    /// (at least one pair), asking for another pass after `TICK` while any remain.
    pub fn tick(&mut self, now: Instant, search: &mut dyn FnMut(&str) -> Result<Vec<(String, String)>, String>, messages: &mut dyn FnMut(&str) -> Vec<Message>) -> Tick {
        let mut out = Tick::default();
        if let Some(due) = self.due {
            if now < due {
                out.next = Some(due);
                return out;
            }
            self.due = None;
            match search(&self.query) {
                Ok(found) => self.pending = found.into(),
                Err(e) => out.error = Some(e),
            }
            out.changed = true;
        }
        let start = Instant::now();
        while let Some((chat, id)) = self.pending.pop_front() {
            if !self.loaded.contains_key(&chat) {
                let map = messages(&chat).into_iter().map(|m| (m.unique_id, (m.content, m.author == Participant::User))).collect();
                self.loaded.insert(chat.clone(), map);
            }
            if let Some((content, user)) = self.loaded.get(&chat).and_then(|m| m.get(&id)) {
                if let Some(snippet) = result_snippet(content, &self.query) {
                    self.hits.push(Hit { chat, message: id, user: *user, snippet });
                    out.changed = true;
                }
            }
            if start.elapsed() >= SLICE {
                break;
            }
        }
        if !self.pending.is_empty() {
            out.next = Some(now + TICK);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat_message::{Payload, RemoteCommand, Subpart, TerminalCommand};
    use serde_json::json;

    fn toks(t: &[&str]) -> Vec<String> {
        t.iter().map(|s| s.to_string()).collect()
    }

    fn plain(s: &str) -> Content {
        Content::PlainText { text: s.into(), context: None }
    }

    #[test]
    fn search_filter_follows_iterm2() {
        assert!(validate("Hello World foo", &toks(&["wor"])));
        assert!(!validate("Hello World foo", &toks(&["Wor"])), "the query token is not lowercased");
        assert!(validate("Hello World foo", &toks(&["wor", ""])));
        assert!(!validate("Hello World foo", &toks(&["zzz"])));
        assert!(validate("a\nbc", &toks(&["bc"])), "a newline does not split words");
        assert!(!validate("   ", &toks(&[""])), "no words, nothing matches");
        assert_eq!(filter_tokens("a  b"), toks(&["a", "", "b"]));
        assert_eq!(filter_tokens("a\nb\tc"), toks(&["a", "b", "c"]));

        assert_eq!(indexable(&plain("x")).as_deref(), Some("x"));
        assert_eq!(indexable(&Content::Markdown("**b**".into())).as_deref(), Some("b"));
        let terminal = TerminalCommand { username: None, hostname: None, directory: None, command: "ls -l".into(), output: "out".into(), exit_code: 0, url: String::new() };
        assert_eq!(indexable(&Content::TerminalCommand(terminal)).as_deref(), Some("ls -l"));
        assert_eq!(indexable(&Content::WatcherEvent(json!({"_0":{"reason":"stateReached","detail":"went idle"}}))).as_deref(), Some("went idle"));
        assert_eq!(indexable(&Content::ExplanationRequest(json!({"subjectMatter":"s","question":"q"}))), None, "12e Task 3 fills this arm (ruling 12c-3)");
        let parts = vec![
            Subpart::PlainText("p".into()),
            Subpart::Attachment(json!({"inline":true,"id":"c","type":{"code":{"_0":"c"}}})),
            Subpart::Attachment(json!({"inline":false,"id":"f","type":{"file":{"_0":{"name":"a","content":"aGk=","mimeType":"text/plain"}}}})),
            Subpart::Context("model only".into()),
        ];
        assert_eq!(indexable(&Content::Multipart { parts, vector_store_id: None }).as_deref(), Some("p\nc"));
        let md = Content::Multipart { parts: vec![Subpart::Markdown("**raw**".into())], vector_store_id: None };
        assert_eq!(indexable(&md).as_deref(), Some("**raw**"), "a multipart markdown part is indexed as written");
        let rc = Payload::Classic(RemoteCommand { llm_message: json!({}), call: crate::ai_tools::Call::IsAtPrompt });
        assert_eq!(indexable(&Content::RemoteCommandRequest { payload: rc, safe: None }), None);
        assert_eq!(indexable(&Content::RenameChat("n".into())), None);
        assert_eq!(indexable(&Content::Unsupported("{}".into())), None);
        let response = |result| Content::RemoteCommandResponse { result, request_id: "r".into(), function_name: "f".into(), function_call_id: None };
        assert_eq!(indexable(&response(Ok("ok".into()))).as_deref(), Some("ok"));
        assert_eq!(indexable(&response(Err(crate::chat_message::AiError { message: "bad".into(), kind: 0 }))), None);
    }

    #[test]
    fn snippets_follow_iterm2() {
        assert_eq!(highlight("Hello World", &toks(&["wor"])), "hello **wor**ld");
        // The search resumes after each match, so "aaa" is one match and a rest; touching matches join.
        assert_eq!(highlight("aaa", &toks(&["aa"])), "**aa**a");
        assert_eq!(highlight("aaaa", &toks(&["aa"])), "**aaaa**");
        assert_eq!(highlight("a b", &toks(&["a", "b"])), "**a** **b**");
        assert_eq!(bold_runs("x **y** z"), vec![("x ".to_string(), false), ("y".to_string(), true), (" z".to_string(), false)]);
        assert_eq!(highlight("abc", &toks(&[""])), "abc", "an empty token matches nothing");
        let long = "one two three four five six seven eight nine ten";
        assert_eq!(snippetize("Hello World", &toks(&["wor"]), 200), highlight("Hello World", &toks(&["wor"])));
        assert_eq!(snippetize(long, &toks(&["five"]), 30), "three four **five** six seven");
        assert_eq!(snippetize(long, &toks(&["two", "nine"]), 40), "one **two** three four \u{2026} seven eight **nine** ten");
        assert_eq!(snippetize(long, &toks(&["five"]), 10), "");
        assert_eq!(result_snippet(&plain("Hello World foo"), "Wor"), None, "fails the filter");
        assert_eq!(result_snippet(&plain("nothing"), "zzz"), None);
        assert!(result_snippet(&plain("it's here"), "it's").is_some(), "the quote reaches the filter unharmed");
        assert_eq!(result_snippet(&plain("Hello World"), "wor").as_deref(), Some("hello **wor**ld"));
        assert_eq!(result_snippet(&Content::RenameChat("wor".into()), "wor"), None);
    }

    fn message(chat: &str, id: &str, author: Participant, text: &str) -> Message {
        let mut m = Message::new(chat, author, plain(text), 1.0);
        m.unique_id = id.to_string();
        m
    }

    #[test]
    fn search_work_streams_and_a_new_query_drops_it() {
        let t0 = Instant::now();
        let mut st = State::default();
        assert!(!st.set_query("", t0), "an empty query is no change");
        assert!(st.set_query("wor", t0));
        assert!(!st.set_query("wor", t0), "the same query is no change");
        assert!(st.searching());
        let mut loads: Vec<String> = Vec::new();
        let mut search = |q: &str| -> Result<Vec<(String, String)>, String> {
            assert_eq!(q, "wor");
            Ok(vec![("c1".into(), "m1".into()), ("c1".into(), "m2".into()), ("c2".into(), "m3".into()), ("c2".into(), "gone".into())])
        };
        let mut messages = |chat: &str| -> Vec<Message> {
            loads.push(chat.to_string());
            match chat {
                "c1" => vec![message("c1", "m1", Participant::User, "Hello World"), message("c1", "m2", Participant::Agent, "no match here")],
                _ => vec![message("c2", "m3", Participant::Agent, "a wordy reply")],
            }
        };
        let early = st.tick(t0, &mut search, &mut messages);
        assert_eq!(early, Tick { changed: false, next: Some(t0 + DEBOUNCE), error: None }, "nothing runs before the debounce");
        assert!(st.hits().is_empty());
        let done = st.tick(t0 + DEBOUNCE, &mut search, &mut messages);
        assert!(done.changed && done.error.is_none());
        assert_eq!(done.next, None, "all four pairs fit one slice");
        assert!(!st.searching());
        assert_eq!(st.hits().len(), 2, "a failed filter and a missing message are dropped");
        assert_eq!(st.hits()[0], Hit { chat: "c1".into(), message: "m1".into(), user: true, snippet: "hello **wor**ld".into() });
        assert_eq!((st.hits()[1].chat.as_str(), st.hits()[1].user), ("c2", false));
        assert_eq!(loads, vec!["c1".to_string(), "c2".to_string()], "each chat's messages load once");

        assert!(st.set_query("other", t0));
        assert!(st.hits().is_empty(), "a new query drops the old hits");
        assert!(st.searching());
        assert!(st.set_query("", t0));
        assert!(!st.searching() && st.hits().is_empty());
    }

    #[test]
    fn a_slice_always_does_one_pair_and_reports_the_rest() {
        let t0 = Instant::now();
        let mut st = State::default();
        st.set_query("a", t0);
        let many: Vec<(String, String)> = (0..50_000).map(|i| ("c".to_string(), format!("m{i}"))).collect();
        let mut search = |_: &str| Ok(many.clone());
        let mut messages = |_: &str| (0..50_000).map(|i| message("c", &format!("m{i}"), Participant::User, "a b c")).collect::<Vec<_>>();
        let t = st.tick(t0 + DEBOUNCE, &mut search, &mut messages);
        assert!(t.changed && !st.hits().is_empty());
        if st.searching() {
            let now = Instant::now();
            assert!(t.next.is_some_and(|n| n >= now.checked_sub(SLICE).unwrap_or(now)), "more work asks for another pass after TICK");
        }
        let mut search_err = |_: &str| Err("db".to_string());
        let mut st2 = State::default();
        st2.set_query("a", t0);
        let e = st2.tick(t0 + DEBOUNCE, &mut search_err, &mut messages);
        assert_eq!(e.error.as_deref(), Some("db"));
        assert!(!st2.searching());
    }
}

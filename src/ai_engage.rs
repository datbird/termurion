//! Engage Artificial Intelligence's rules (iTerm2's `PTYSession.m:21095-21209`, the large
//! Composer's `LC:306-343`): what the dialog asks and shows, when the skip key sends at once,
//! and how an answer is trimmed. Pure; the windows are `ai_engage_ui`.

/// The global that skips the dialog when set (a global, `PS:21125-21129`).
pub const SKIP_KEY: &str = "NoSyncBypassConfirmAIPrompt";
pub const DIALOG_TEXT: &str = "Describe the command you want to run in plain English. Press Shift+Enter to send.";
pub const SKIP_LABEL: &str = "Skip this dialog in the future and send the prompt immediately.";
pub const LONG_REASON: &str = "The selected text was rather long.";
pub const THINKING: &str = "Thinking\u{2026}";
pub const ERROR_HEADING: &str = "AI Error";
/// The `ai.prompt` variable of the `AI Prompt` template.
pub const PROMPT_VAR: &str = "prompt";
/// The dialog's text field, in points (`PS:21160`).
pub const FIELD_W: f32 = 400.0;
pub const FIELD_H: f32 = 200.0;

/// What the dialog opens with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    pub prefill: String,
    /// The small label under the field.
    pub reason: Option<&'static str>,
    /// Whether the skip checkbox is offered.
    pub skippable: bool,
}

/// NSString's length.
pub fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// The longest prefill, in UTF-16 units: `AiResponseMaxTokens / 8`, 0 when negative.
pub fn max_length(response_tokens: i64) -> usize {
    usize::try_from(response_tokens / 8).unwrap_or(0)
}

/// The selection when there is one, else the current command, else nothing.
pub fn query_source(selection: Option<&str>, current_command: Option<&str>) -> String {
    selection.filter(|s| !s.is_empty()).or(current_command).unwrap_or("").to_string()
}

/// iTerm2's three openings: empty and not skippable; at or past the limit only the last
/// `max_length` units with the reason, not skippable; else prefilled and skippable. The cut
/// walks back over whole characters, so it never splits a surrogate pair.
pub fn ask_for(query: &str, response_tokens: i64) -> Ask {
    if query.is_empty() {
        return Ask { prefill: String::new(), reason: None, skippable: false };
    }
    let limit = max_length(response_tokens);
    if utf16_len(query) < limit {
        return Ask { prefill: query.to_string(), reason: None, skippable: true };
    }
    let mut units = 0;
    let mut start = query.len();
    for (i, c) in query.char_indices().rev() {
        units += c.len_utf16();
        if units > limit {
            break;
        }
        start = i;
    }
    Ask { prefill: query[start..].to_string(), reason: Some(LONG_REASON), skippable: false }
}

/// The key is set and the prefill is skippable: no dialog.
pub fn send_at_once(ask: &Ask, skip_set: bool) -> bool {
    skip_set && ask.skippable
}

/// The `ai.prompt` value.
pub fn trimmed_query(text: &str) -> String {
    text.trim().to_string()
}

/// `acceptSuggestion:` (`LC:336-343`): whitespace, newlines and backticks trimmed from both ends
/// (a command's own trailing backtick goes too, as iTerm2).
pub fn accept_text(choice: &str) -> String {
    choice.trim_matches(|c: char| c.is_whitespace() || c == '`').to_string()
}

/// The large Composer's query: the selection when non-empty, else the text after the prompt
/// prefix and before the suggestion.
pub fn composer_query(text_after_prefix_before_ghost: &str, selected: Option<&str>) -> String {
    selected.filter(|s| !s.is_empty()).unwrap_or(text_after_prefix_before_ghost).to_string()
}

/// The status bar Composer's AI button works only when generative AI is allowed and the field
/// holds something besides whitespace.
pub fn status_bar_query_enabled(generative_ai_allowed: bool, field: &str) -> bool {
    generative_ai_allowed && !field.trim().is_empty()
}

/// The field is erased after an answer only if it still holds the query.
pub fn erase_status_bar(current: &str, query: &str) -> bool {
    current == query
}

/// One row of the dialog, top to bottom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Text(&'static str),
    Field(String),
    Check(&'static str),
    Reason(&'static str),
    Buttons(Vec<&'static str>),
}

pub fn dialog_rows(ask: &Ask) -> Vec<Row> {
    let mut rows = vec![Row::Text(DIALOG_TEXT), Row::Field(ask.prefill.clone())];
    if ask.skippable {
        rows.push(Row::Check(SKIP_LABEL));
    }
    if let Some(r) = ask.reason {
        rows.push(Row::Reason(r));
    }
    rows.push(Row::Buttons(vec!["OK", "Cancel"]));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn long_ask(units: usize) -> Ask {
        ask_for(&"a".repeat(units), 8192)
    }

    #[test]
    fn engage_input_follows_iterm2() {
        assert_eq!(query_source(Some("ls -la"), Some("git st")), "ls -la");
        assert_eq!(query_source(None, Some("git st")), "git st");
        assert_eq!(query_source(None, None), "");
        assert_eq!(max_length(8192), 1024);
        assert_eq!(max_length(-5), 0);
        assert_eq!(ask_for("", 8192), Ask { prefill: String::new(), reason: None, skippable: false });
        let a = ask_for("find big files", 8192);
        assert_eq!(a, Ask { prefill: "find big files".into(), reason: None, skippable: true });
        let q: String = (0..1024).map(|i| (b'a' + (i % 26) as u8) as char).collect();
        let a = ask_for(&q, 8192);
        assert_eq!(a.prefill, q);
        assert_eq!(a.reason, Some(LONG_REASON));
        assert!(!a.skippable);
        let q: String = (0..1030).map(|i| (b'a' + (i % 26) as u8) as char).collect();
        let a = ask_for(&q, 8192);
        assert_eq!(a.prefill, q[6..]);
        assert_eq!(utf16_len(&a.prefill), 1024);
        let q = "\u{1F600}".repeat(600);
        assert_eq!(utf16_len(&q), 1200);
        let a = ask_for(&q, 8192);
        assert_eq!(a.prefill.chars().count(), 512);
        assert_eq!(utf16_len(&a.prefill), 1024);
        // An odd limit never splits a pair: 3 units keep one emoji and one letter.
        let a = ask_for("x\u{1F600}\u{1F600}", 24);
        assert_eq!(a.prefill, "\u{1F600}");
        assert!(send_at_once(&ask_for("find big files", 8192), true));
        assert!(!send_at_once(&ask_for("find big files", 8192), false));
        assert!(!send_at_once(&ask_for("", 8192), true));
        assert!(!send_at_once(&long_ask(1024), true));
        assert_eq!(trimmed_query("  \n list files \n"), "list files");
    }

    #[test]
    fn dialog_rows_follow_iterm2() {
        let ok = Row::Buttons(vec!["OK", "Cancel"]);
        let a = ask_for("find big files", 8192);
        assert_eq!(dialog_rows(&a), vec![Row::Text(DIALOG_TEXT), Row::Field("find big files".into()), Row::Check(SKIP_LABEL), ok.clone()]);
        let l = long_ask(1100);
        assert_eq!(dialog_rows(&l), vec![Row::Text(DIALOG_TEXT), Row::Field(l.prefill.clone()), Row::Reason(LONG_REASON), ok.clone()]);
        assert_eq!(dialog_rows(&ask_for("", 8192)), vec![Row::Text(DIALOG_TEXT), Row::Field(String::new()), ok]);
        assert_eq!(DIALOG_TEXT, "Describe the command you want to run in plain English. Press Shift+Enter to send.");
        assert_eq!(SKIP_LABEL, "Skip this dialog in the future and send the prompt immediately.");
        assert_eq!(THINKING, "Thinking\u{2026}");
        assert_eq!(ERROR_HEADING, "AI Error");
        assert_eq!(SKIP_KEY, "NoSyncBypassConfirmAIPrompt");
        assert_eq!(PROMPT_VAR, "prompt");
    }

    #[test]
    fn accept_text_follows_iterm2() {
        assert_eq!(accept_text("```\nls -la\n```"), "ls -la");
        assert_eq!(accept_text(" `rm *.log` \n"), "rm *.log");
        assert_eq!(accept_text("echo `date`"), "echo `date");
        assert_eq!(composer_query("git st", None), "git st");
        assert_eq!(composer_query("git st", Some("st")), "st");
        assert_eq!(composer_query("git st", Some("")), "git st");
    }

    #[test]
    fn status_bar_query_follows_iterm2() {
        assert!(status_bar_query_enabled(true, "list files  "));
        assert!(!status_bar_query_enabled(true, "   "));
        assert!(!status_bar_query_enabled(false, "list"));
        assert!(erase_status_bar("list", "list"));
        assert!(!erase_status_bar("list more", "list"));
    }
}

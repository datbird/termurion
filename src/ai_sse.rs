//! Splitting a streamed answer into its events: server-sent events, the legacy line format and
//! newline-delimited JSON. Each returns the first event's JSON, if a whole one is there, and the
//! rest of the input to keep for the next chunk. Pure; ports `AITerm/SSE.swift`,
//! `LegacyOpenAI.swift:120-144` and `Llama.swift:196-208`.

/// The stream's end marker (`[DONE]`).
pub const DONE: &str = "[DONE]";

/// Swift's `CharacterSet.whitespaces`: tab and the Unicode space separators, not newlines.
fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}')
}

/// The first line and what follows its line end, splitting at the first "\r\n" in the input, or
/// else at the first "\n" (iTerm2's order, kept).
fn first_line<'a>(input: &'a str, crlf_first: bool) -> Option<(&'a str, &'a str)> {
    let crlf = if crlf_first { input.find("\r\n").map(|i| (i, 2)) } else { None };
    let (at, len) = crlf.or_else(|| input.find('\n').map(|i| (i, 1)))?;
    Some((&input[..at], &input[at + len..]))
}

/// `SplitServerSentEvents` (`SSE.swift`): leading whitespace is dropped; `event:`, `id:`,
/// `retry:`, comment and blank lines are skipped; the first `data:` line is the event; any other
/// line stops the split (as iTerm2's does), leaving the input as it is.
pub fn split_sse(input: &str) -> (Option<String>, String) {
    let mut input = input.trim_start();
    loop {
        let Some((line, rest)) = first_line(input, true) else {
            return (None, input.to_string());
        };
        if line.starts_with("event:") || line.starts_with("id:") || line.starts_with("retry:") || line.starts_with(':') || line.trim_matches(is_space).is_empty() {
            input = rest.trim_start();
            continue;
        }
        return match line.strip_prefix("data:") {
            Some(json) => (Some(json.trim_matches(is_space).to_string()), rest.to_string()),
            None => (None, input.to_string()),
        };
    }
}

/// The legacy streaming splitter (`LegacyOpenAI.swift:120-144`): one event per "\n"-ended line,
/// with an optional `data:` prefix.
pub fn split_lines(input: &str) -> (Option<String>, String) {
    let input = input.trim_start();
    let Some((line, rest)) = first_line(input, false) else {
        return (None, input.to_string());
    };
    let prefix = if line.starts_with("data:") { "data:" } else { "" };
    let json = line[prefix.len()..].trim_matches(is_space);
    (Some(json.strip_prefix(prefix).unwrap_or(json).to_string()), rest.to_string())
}

/// `SplitNDJSON` (`Llama.swift:196-208`): one event per "\n"-ended line.
pub fn split_ndjson(input: &str) -> (Option<String>, String) {
    let input = input.trim_start();
    match first_line(input, false) {
        Some((line, rest)) => (Some(line.to_string()), rest.to_string()),
        None => (None, input.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(json: Option<&str>, rest: &str) -> (Option<String>, String) {
        (json.map(str::to_string), rest.to_string())
    }

    #[test]
    fn splitters_follow_iterm2() {
        assert_eq!(split_sse("event: x\r\ndata: {\"a\":1}\r\n\r\nrest"), pair(Some("{\"a\":1}"), "\r\nrest"));
        assert_eq!(split_sse(": ping\n\ndata: b\n"), pair(Some("b"), ""));
        assert_eq!(split_sse("data: partial"), pair(None, "data: partial"));
        assert_eq!(split_sse("junk\ndata: x\n"), pair(None, "junk\ndata: x\n"), "a non-data line stops, as iTerm2");
        assert_eq!(split_sse("  \n"), pair(None, ""));
        assert_eq!(split_sse("id: 1\nretry: 5\n \t\ndata:[DONE]\n"), pair(Some(DONE), ""));
        assert_eq!(split_sse("event: x\r\nda"), pair(None, "da"), "a cut line waits");
        assert_eq!(split_sse("\r"), pair(None, ""));
        assert_eq!(split_sse("data: a\r"), pair(None, "data: a\r"), "a cut line end waits");
        assert_eq!(split_lines("data: {}\nnext"), pair(Some("{}"), "next"));
        assert_eq!(split_lines("{}\n"), pair(Some("{}"), ""));
        assert_eq!(split_lines("data:data:{}\n"), pair(Some("{}"), ""), "the prefix is removed twice, as iTerm2's");
        assert_eq!(split_lines("{\"a\":1}"), pair(None, "{\"a\":1}"));
        assert_eq!(split_ndjson("{\"a\":1}\n{\"b\""), pair(Some("{\"a\":1}"), "{\"b\""));
        assert_eq!(split_ndjson("\n\n{\"a\":1}\r\n"), pair(Some("{\"a\":1}\r"), ""));
    }
}

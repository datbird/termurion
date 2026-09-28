//! Splitting a streamed answer into its events: server-sent events, the legacy line format and
//! newline-delimited JSON. Each returns the first event's JSON, if a whole one is there, and moves
//! a `Cursor` past what it consumed, so a body of any size is split in one pass: nothing after the
//! event is copied or scanned again (final review, ruling "SSE parsing is linear"). Pure; ports
//! `AITerm/SSE.swift`, `LegacyOpenAI.swift:120-144` and `Llama.swift:196-208`.

/// The stream's end marker (`[DONE]`).
pub const DONE: &str = "[DONE]";

/// Where the next event starts in a buffer, plus how far line-end searches already looked. The
/// buffer may grow between calls (a stream's chunks append to it); `compact` drops what was
/// consumed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cursor {
    /// The byte offset of what is left.
    pub pos: usize,
    /// No "\n" is in `pos..lf_clear`.
    lf_clear: usize,
    /// No "\r\n" starts in `pos..crlf_clear`.
    crlf_clear: usize,
}

impl Cursor {
    /// What is left of `buffer`.
    pub fn rest<'a>(&self, buffer: &'a str) -> &'a str {
        &buffer[self.pos..]
    }

    /// Removes the consumed front of `buffer`; the cursor then points at its start.
    pub fn compact(&mut self, buffer: &mut String) {
        buffer.drain(..self.pos);
        self.lf_clear = self.lf_clear.saturating_sub(self.pos);
        self.crlf_clear = self.crlf_clear.saturating_sub(self.pos);
        self.pos = 0;
    }

    /// Moves past leading whitespace and newlines.
    fn skip_space(&mut self, input: &str) {
        let rest = &input[self.pos..];
        self.pos += rest.len() - rest.trim_start().len();
    }

    /// The next "\n" at or after `pos`, searching only what no earlier call searched.
    fn next_lf(&mut self, input: &str) -> Option<usize> {
        let from = self.pos.max(self.lf_clear);
        match input[from..].find('\n') {
            Some(i) => {
                self.lf_clear = from + i;
                Some(from + i)
            }
            None => {
                self.lf_clear = input.len();
                None
            }
        }
    }

    /// The next "\r\n" at or after `pos`, searching only what no earlier call searched. A last
    /// "\r" stays unsearched: the next chunk may end it.
    fn next_crlf(&mut self, input: &str) -> Option<usize> {
        let from = self.pos.max(self.crlf_clear);
        match input[from..].find("\r\n") {
            Some(i) => {
                self.crlf_clear = from + i;
                Some(from + i)
            }
            None => {
                let tail = usize::from(input.ends_with('\r'));
                self.crlf_clear = (input.len() - tail).max(from);
                None
            }
        }
    }

    /// The next line after `pos` and the offset just past its line end, splitting at the first
    /// "\r\n" in what is left, or else at the first "\n" (iTerm2's order, kept).
    fn line<'a>(&mut self, input: &'a str, crlf_first: bool) -> Option<(&'a str, usize)> {
        let crlf = if crlf_first { self.next_crlf(input).map(|i| (i, 2)) } else { None };
        let (at, len) = match crlf {
            Some(found) => found,
            None => (self.next_lf(input)?, 1),
        };
        Some((&input[self.pos..at], at + len))
    }
}

/// Swift's `CharacterSet.whitespaces`: tab and the Unicode space separators, not newlines.
fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}')
}

/// `SplitServerSentEvents` (`SSE.swift`): leading whitespace is dropped; `event:`, `id:`,
/// `retry:`, comment and blank lines are skipped; the first `data:` line is the event; any other
/// line stops the split (as iTerm2's does), leaving the cursor on it.
pub fn split_sse(input: &str, at: &mut Cursor) -> Option<String> {
    loop {
        at.skip_space(input);
        let (line, next) = at.line(input, true)?;
        if line.starts_with("event:") || line.starts_with("id:") || line.starts_with("retry:") || line.starts_with(':') || line.trim_matches(is_space).is_empty() {
            at.pos = next;
            continue;
        }
        let json = line.strip_prefix("data:")?.trim_matches(is_space).to_string();
        at.pos = next;
        return Some(json);
    }
}

/// The legacy streaming splitter (`LegacyOpenAI.swift:120-144`): one event per "\n"-ended line,
/// with an optional `data:` prefix.
pub fn split_lines(input: &str, at: &mut Cursor) -> Option<String> {
    at.skip_space(input);
    let (line, next) = at.line(input, false)?;
    at.pos = next;
    let prefix = if line.starts_with("data:") { "data:" } else { "" };
    let json = line[prefix.len()..].trim_matches(is_space);
    Some(json.strip_prefix(prefix).unwrap_or(json).to_string())
}

/// `SplitNDJSON` (`Llama.swift:196-208`): one event per "\n"-ended line.
pub fn split_ndjson(input: &str, at: &mut Cursor) -> Option<String> {
    at.skip_space(input);
    let (line, next) = at.line(input, false)?;
    at.pos = next;
    Some(line.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(json: Option<&str>, rest: &str) -> (Option<String>, String) {
        (json.map(str::to_string), rest.to_string())
    }

    /// One split from the start of `input`: the event and what is left.
    fn once(split: fn(&str, &mut Cursor) -> Option<String>, input: &str) -> (Option<String>, String) {
        let mut at = Cursor::default();
        let json = split(input, &mut at);
        (json, at.rest(input).to_string())
    }

    #[test]
    fn splitters_follow_iterm2() {
        assert_eq!(once(split_sse, "event: x\r\ndata: {\"a\":1}\r\n\r\nrest"), pair(Some("{\"a\":1}"), "\r\nrest"));
        assert_eq!(once(split_sse, ": ping\n\ndata: b\n"), pair(Some("b"), ""));
        assert_eq!(once(split_sse, "data: partial"), pair(None, "data: partial"));
        assert_eq!(once(split_sse, "junk\ndata: x\n"), pair(None, "junk\ndata: x\n"), "a non-data line stops, as iTerm2");
        assert_eq!(once(split_sse, "  \n"), pair(None, ""));
        assert_eq!(once(split_sse, "id: 1\nretry: 5\n \t\ndata:[DONE]\n"), pair(Some(DONE), ""));
        assert_eq!(once(split_sse, "event: x\r\nda"), pair(None, "da"), "a cut line waits");
        assert_eq!(once(split_sse, "\r"), pair(None, ""));
        assert_eq!(once(split_sse, "data: a\r"), pair(None, "data: a\r"), "a cut line end waits");
        assert_eq!(once(split_sse, "data: a\ndata: b\r\n"), pair(Some("a\ndata: b"), ""), "a later CRLF wins, as iTerm2's");
        assert_eq!(once(split_lines, "data: {}\nnext"), pair(Some("{}"), "next"));
        assert_eq!(once(split_lines, "{}\n"), pair(Some("{}"), ""));
        assert_eq!(once(split_lines, "data:data:{}\n"), pair(Some("{}"), ""), "the prefix is removed twice, as iTerm2's");
        assert_eq!(once(split_lines, "{\"a\":1}"), pair(None, "{\"a\":1}"));
        assert_eq!(once(split_ndjson, "{\"a\":1}\n{\"b\""), pair(Some("{\"a\":1}"), "{\"b\""));
        assert_eq!(once(split_ndjson, "\n\n{\"a\":1}\r\n"), pair(Some("{\"a\":1}\r"), ""));
    }

    #[test]
    fn a_cursor_splits_a_growing_buffer() {
        let mut buffer = String::new();
        let mut at = Cursor::default();
        let mut events = Vec::new();
        // Byte by byte, CRLF and LF lines mixed, with a compaction after each chunk.
        for b in "event: e\r\ndata: 1\r\n\r\ndata: 2\n\n: c\ndata: 3\r\n".chars() {
            buffer.push(b);
            while let Some(e) = split_sse(&buffer, &mut at) {
                events.push(e);
            }
            at.compact(&mut buffer);
        }
        assert_eq!(events, ["1", "2", "3"]);
        assert_eq!(buffer, "");
        // A line with no end waits, and its end in a later chunk is found.
        let mut buffer = String::from("{\"a\":");
        let mut at = Cursor::default();
        assert_eq!(split_ndjson(&buffer, &mut at), None);
        buffer.push_str("1}\n");
        assert_eq!(split_ndjson(&buffer, &mut at).as_deref(), Some("{\"a\":1}"));
        // A "\r" at the end of one chunk and "\n" at the start of the next make a CRLF.
        let mut buffer = String::from("data: y\r");
        let mut at = Cursor::default();
        assert_eq!(split_sse(&buffer, &mut at), None);
        buffer.push('\n');
        assert_eq!(split_sse(&buffer, &mut at).as_deref(), Some("y"), "not \"y\\r\"");
    }

    #[test]
    fn a_large_body_splits_in_linear_time() {
        // About 20 MB in 64,000 events: a quadratic split would copy or scan terabytes.
        let event = format!("data: {{\"delta\":\"{}\"}}\n\n", "x".repeat(300));
        let body = event.repeat(64_000);
        for split in [split_sse as fn(&str, &mut Cursor) -> Option<String>, split_lines, split_ndjson] {
            let started = std::time::Instant::now();
            let mut at = Cursor::default();
            let mut n = 0;
            while split(&body, &mut at).is_some() {
                n += 1;
            }
            assert_eq!(n, 64_000);
            assert!(started.elapsed() < std::time::Duration::from_secs(5), "took {:?}", started.elapsed());
        }
    }
}

//! In-conversation find (iTerm2's `ChatFindController` and the find bar's texts): the matches
//! over each message's rendered text, stepping between them from the current match or from a
//! click, and the counter. Pure; offsets are byte offsets into a segment's text.

use std::ops::Range;

use regex::Regex;

use crate::chat_view::{Body, Part};
use crate::porthole::render::Doc;
use crate::find::{pattern, Mode};

/// The bar's mode popup, in iTerm2's order (`CFB:43-56`).
pub const MODES: [(&str, Mode); 4] = [("Smart Case", Mode::SmartCase), ("Ignore Case", Mode::CaseInsensitive), ("Match Case", Mode::CaseSensitive), ("Regular Expression", Mode::RegexInsensitive)];
pub const PLACEHOLDER: &str = "Find in Conversation";
pub const NO_RESULTS: &str = "No results";
/// The bar is at most this wide, in points.
pub const MAX_WIDTH: f32 = 640.0;
/// The other matches' highlight; the current one is solid (`CVC:2921-2922`).
pub const HIGHLIGHT_ALPHA: f32 = 0.45;

/// The popup item a mode shows: both regex modes are the one "Regular Expression" item
/// (`CFB:76-85`).
pub fn popup_index(mode: Mode) -> usize {
    match mode {
        Mode::SmartCase => 0,
        Mode::CaseInsensitive => 1,
        Mode::CaseSensitive => 2,
        Mode::RegexSensitive | Mode::RegexInsensitive => 3,
    }
}

/// One match: the message, its row among the conversation's rows, the findable text within
/// the message, and the byte range in that text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub message: String,
    pub item: usize,
    pub segment: usize,
    pub range: Range<usize>,
}

/// A place in the conversation (a click): the next step starts from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub item: usize,
    pub segment: usize,
    pub at: usize,
}

/// The message id, its row, and its findable texts.
pub type Segments = (String, usize, Vec<String>);

/// Every non-overlapping, non-empty match of `query` in `text`; an invalid pattern finds
/// nothing (`CFC:204-258`).
pub fn ranges(query: &str, text: &str, mode: Mode) -> Vec<Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    let Ok(re) = Regex::new(&pattern(query, mode)) else { return Vec::new() };
    re.find_iter(text).filter(|m| !m.is_empty()).map(|m| m.range()).collect()
}

#[derive(Clone, Debug, Default)]
pub struct Finder {
    pub matches: Vec<Match>,
    pub current: Option<usize>,
    pub query: String,
    pub mode: Mode,
    cursor: Option<Cursor>,
}

impl Finder {
    /// A new query: every match, the first current, the click position forgotten.
    pub fn search(&mut self, query: &str, mode: Mode, segments: &[Segments]) {
        self.query = query.to_string();
        self.mode = mode;
        self.cursor = None;
        self.matches = self.scan(segments);
        self.current = (!self.matches.is_empty()).then_some(0);
    }

    fn scan(&self, segments: &[Segments]) -> Vec<Match> {
        let mut found = Vec::new();
        for (message, item, texts) in segments {
            for (segment, text) in texts.iter().enumerate() {
                for range in ranges(&self.query, text, self.mode) {
                    found.push(Match { message: message.clone(), item: *item, segment, range });
                }
            }
        }
        found
    }

    /// The conversation changed: the same query again, keeping the current match when its
    /// message, segment and range survive, else the first.
    pub fn rescan(&mut self, segments: &[Segments]) {
        let kept = self.current_match().cloned();
        self.matches = self.scan(segments);
        self.current = if self.matches.is_empty() {
            None
        } else {
            Some(kept.and_then(|k| self.matches.iter().position(|m| m.message == k.message && m.segment == k.segment && m.range == k.range)).unwrap_or(0))
        };
    }

    /// The first match at or after the click, else the one after the current, wrapping.
    #[allow(clippy::should_implement_trait)] // a cursor-style method with its own return type, not an Iterator
    pub fn next(&mut self) -> Option<&Match> {
        if self.matches.is_empty() {
            self.current = None;
            return None;
        }
        let n = self.matches.len();
        self.current = Some(match self.cursor.take() {
            Some(c) => self.matches.iter().position(|m| !before(m, &c)).unwrap_or(0),
            None => self.current.map_or(0, |i| (i + 1) % n),
        });
        self.current_match()
    }

    /// The last match strictly before the click, else the one before the current, wrapping.
    pub fn previous(&mut self) -> Option<&Match> {
        if self.matches.is_empty() {
            self.current = None;
            return None;
        }
        let n = self.matches.len();
        self.current = Some(match self.cursor.take() {
            Some(c) => self.matches.iter().rposition(|m| before(m, &c)).unwrap_or(n - 1),
            None => self.current.map_or(n - 1, |i| (i + n - 1) % n),
        });
        self.current_match()
    }

    pub fn set_cursor(&mut self, c: Option<Cursor>) {
        self.cursor = c;
    }

    pub fn clear(&mut self) {
        self.query.clear();
        self.matches.clear();
        self.current = None;
        self.cursor = None;
    }

    /// The bar's counter (`CFB:87-98`).
    pub fn counter(&self) -> String {
        if self.query.is_empty() {
            String::new()
        } else if self.matches.is_empty() {
            NO_RESULTS.to_string()
        } else if let Some(i) = self.current {
            format!("{} of {}", i + 1, self.matches.len())
        } else {
            self.matches.len().to_string()
        }
    }

    pub fn current_match(&self) -> Option<&Match> {
        self.current.and_then(|i| self.matches.get(i))
    }
}

/// Whether a match starts before the click position (item, then segment, then offset).
fn before(m: &Match, c: &Cursor) -> bool {
    (m.item, m.segment, m.range.start) < (c.item, c.segment, c.at)
}

/// The findable texts of a bubble's body, in the order the window draws them: the text of each
/// text item, and each part of a multipart; file parts have none. `markdown_plain` turns a
/// markdown source into the rendered document's plain text.
pub fn body_segments(body: &Body, markdown_plain: &dyn Fn(&str) -> String) -> Vec<String> {
    match body {
        Body::Plain(t) => vec![t.clone()],
        Body::Markdown(md) => vec![markdown_plain(md)],
        Body::Command { command, .. } => vec![command.clone()],
        Body::Parts(parts) => parts
            .iter()
            .flat_map(|p| match p {
                Part::Text(b) => body_segments(b, markdown_plain),
                Part::Code(c) => vec![c.clone()],
                Part::Status(s) => vec![markdown_plain(s)],
                Part::File { .. } | Part::RemoteFile { .. } => Vec::new(),
            })
            .collect(),
    }
}

/// Where each block of `doc` starts in `Doc::plain` (blocks joined by one newline; a rule is an
/// empty line), and its length in bytes.
pub fn block_spans(doc: &Doc) -> Vec<Range<usize>> {
    let mut at = 0;
    doc.blocks
        .iter()
        .map(|b| {
            let len = if b.rule { 0 } else { b.runs.iter().map(|r| r.text.len()).sum() };
            let span = at..at + len;
            at += len + 1;
            span
        })
        .collect()
}

/// A highlighted stretch inside one block: its bytes relative to the block, and whether it is
/// the current match.
pub type Mark = (Range<usize>, bool);

/// The matches of one segment that touch the block at `span`, clipped to it and made relative
/// to its start. `current` is the index of the current match in `matches`.
pub fn marks(matches: &[Match], current: Option<usize>, message: &str, segment: usize, span: &Range<usize>) -> Vec<Mark> {
    matches
        .iter()
        .enumerate()
        .filter(|(_, m)| m.message == message && m.segment == segment && m.range.start < span.end && m.range.end > span.start)
        .map(|(i, m)| (m.range.start.max(span.start) - span.start..m.range.end.min(span.end) - span.start, current == Some(i)))
        .collect()
}

/// Cuts one run (its bytes relative to the block) at the marks' edges: each piece with
/// `Some(current)` when a mark covers it.
pub fn cut_run(run: Range<usize>, marks: &[Mark]) -> Vec<(Range<usize>, Option<bool>)> {
    let mut pieces = Vec::new();
    let mut at = run.start;
    while at < run.end {
        // Inside a mark: up to its end; else up to the next mark's start (or the run's end).
        let inside = marks.iter().find(|(r, _)| r.start <= at && at < r.end);
        let end = match inside {
            Some((r, _)) => r.end.min(run.end),
            None => marks.iter().map(|(r, _)| r.start).filter(|s| *s > at).min().unwrap_or(run.end).min(run.end),
        };
        pieces.push((at..end, inside.map(|(_, c)| *c)));
        at = end;
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(id: &str, item: usize, texts: &[&str]) -> Segments {
        (id.to_string(), item, texts.iter().map(|t| t.to_string()).collect())
    }

    fn three() -> Finder {
        let mut f = Finder::default();
        f.search("a", Mode::SmartCase, &[seg("m1", 1, &["a b a"]), seg("m2", 3, &["x", "a"])]);
        f
    }

    #[test]
    fn find_modes_follow_iterm2() {
        let titles: Vec<&str> = MODES.iter().map(|m| m.0).collect();
        assert_eq!(titles, ["Smart Case", "Ignore Case", "Match Case", "Regular Expression"]);
        assert_eq!(popup_index(Mode::RegexSensitive), 3);
        assert_eq!(popup_index(Mode::RegexInsensitive), 3);
        assert_eq!(popup_index(Mode::CaseSensitive), 2);
        assert_eq!(popup_index(Mode::CaseInsensitive), 1);
        assert_eq!(popup_index(Mode::SmartCase), 0);
        assert_eq!(PLACEHOLDER, "Find in Conversation");
    }

    #[test]
    fn find_matches_follow_iterm2() {
        assert_eq!(ranges("hello", "Hello hello", Mode::SmartCase), vec![0..5, 6..11]);
        assert_eq!(ranges("Hello", "Hello hello", Mode::SmartCase), vec![0..5]);
        assert!(ranges("hello", "HELLO", Mode::CaseSensitive).is_empty());
        assert_eq!(ranges("aa", "aaaa", Mode::CaseInsensitive), vec![0..2, 2..4]);
        assert_eq!(ranges("h.l+o", "HELLO", Mode::RegexInsensitive), vec![0..5]);
        assert!(ranges("(", "x", Mode::RegexInsensitive).is_empty());
        assert!(ranges("x*", "abc", Mode::RegexInsensitive).is_empty());
        assert!(ranges("", "abc", Mode::SmartCase).is_empty());
        // Byte offsets, on character boundaries.
        assert_eq!(ranges("b", "\u{e9}b", Mode::SmartCase), vec![2..3]);
    }

    #[test]
    fn find_steps_follow_iterm2() {
        let mut f = three();
        assert_eq!(f.matches.len(), 3);
        assert_eq!(f.current, Some(0));
        assert_eq!(f.next().map(|m| m.range.clone()), Some(4..5));
        assert_eq!(f.current, Some(1));
        f.next();
        assert_eq!(f.current, Some(2));
        f.next();
        assert_eq!(f.current, Some(0), "wraps");
        f.previous();
        assert_eq!(f.current, Some(2), "wraps back");

        // No current match: next gives the first, previous the last.
        let mut f = three();
        f.current = None;
        f.next();
        assert_eq!(f.current, Some(0));
        f.current = None;
        f.previous();
        assert_eq!(f.current, Some(2));

        // A click between matches 0 and 1: next gives 1, then stepping resumes from it.
        let mut f = three();
        f.set_cursor(Some(Cursor { item: 1, segment: 0, at: 3 }));
        f.next();
        assert_eq!(f.current, Some(1));
        f.next();
        assert_eq!(f.current, Some(2));
        // A click after the last: next wraps to the first.
        let mut f = three();
        f.set_cursor(Some(Cursor { item: 5, segment: 0, at: 0 }));
        f.next();
        assert_eq!(f.current, Some(0));
        // Previous with a click: the last match strictly before it.
        let mut f = three();
        f.set_cursor(Some(Cursor { item: 3, segment: 1, at: 0 }));
        f.previous();
        assert_eq!(f.current, Some(1));
        let mut f = three();
        f.set_cursor(Some(Cursor { item: 1, segment: 0, at: 0 }));
        f.previous();
        assert_eq!(f.current, Some(2), "nothing before it: wraps to the last");
        // A click exactly at a match's start: next takes that match, previous does not.
        let mut f = three();
        f.set_cursor(Some(Cursor { item: 1, segment: 0, at: 4 }));
        f.next();
        assert_eq!(f.current, Some(1));

        // A rescan after a streamed chunk keeps the same match current.
        let mut f = three();
        f.next();
        assert_eq!(f.current, Some(1));
        f.rescan(&[seg("m1", 1, &["a b a"]), seg("m2", 3, &["x", "a a"])]);
        assert_eq!(f.matches.len(), 4);
        assert_eq!(f.current, Some(1));
        // A rescan that removes it makes the first current.
        f.rescan(&[seg("m2", 3, &["x", "a a"])]);
        assert_eq!(f.matches.len(), 2);
        assert_eq!(f.current, Some(0));
        // No match at all: no current.
        f.rescan(&[seg("m2", 3, &["x"])]);
        assert_eq!(f.current, None);
    }

    #[test]
    fn find_counter_follows_iterm2() {
        let mut f = Finder::default();
        assert_eq!(f.counter(), "");
        f.search("zzz", Mode::SmartCase, &[seg("m1", 1, &["a"])]);
        assert_eq!(f.counter(), "No results");
        let mut f = three();
        assert_eq!(f.counter(), "1 of 3");
        f.next();
        assert_eq!(f.counter(), "2 of 3");
        f.current = None;
        assert_eq!(f.counter(), "3");
        f.clear();
        assert_eq!((f.counter().as_str(), f.matches.len(), f.current), ("", 0, None));
    }

    #[test]
    fn segments_follow_what_the_window_draws() {
        let md = |s: &str| format!("<{s}>");
        assert_eq!(body_segments(&Body::Plain("p".into()), &md), ["p"]);
        assert_eq!(body_segments(&Body::Markdown("m".into()), &md), ["<m>"]);
        assert_eq!(body_segments(&Body::Command { command: "ls".into(), url: "u".into() }, &md), ["ls"]);
        let parts = Body::Parts(vec![
            Part::Text(Body::Plain("t".into())),
            Part::File { id: "1".into(), name: "f.txt".into() },
            Part::Code("c".into()),
            Part::Status("s".into()),
            Part::RemoteFile { name: "r".into() },
            Part::Text(Body::Markdown("m".into())),
        ]);
        assert_eq!(body_segments(&parts, &md), ["t", "c", "<s>", "<m>"]);
    }

    #[test]
    fn highlights_follow_the_rendered_text() {
        use crate::palette::Palette;
        use crate::porthole::detect::Language;
        let doc = crate::porthole::render::render("# Title\n\nsome **bold** text\n\n---\n\n- one\n- two\n", &Language::Markdown, &Palette::default(), false);
        let plain = doc.plain();
        let spans = block_spans(&doc);
        assert_eq!(spans.len(), doc.blocks.len());
        for (b, span) in doc.blocks.iter().zip(&spans) {
            let text: String = if b.rule { String::new() } else { b.runs.iter().map(|r| r.text.as_str()).collect() };
            assert_eq!(&plain[span.clone()], text);
        }
        // A match found in the plain text lands inside the right block, relative to it.
        let at = plain.find("bold").unwrap();
        let m = Match { message: "m".into(), item: 0, segment: 2, range: at..at + 4 };
        let hit: Vec<_> = spans.iter().map(|sp| marks(std::slice::from_ref(&m), Some(0), "m", 2, sp)).collect();
        let (i, found) = hit.iter().enumerate().find(|(_, v)| !v.is_empty()).unwrap();
        assert_eq!(found.len(), 1);
        let text: String = doc.blocks[i].runs.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(&text[found[0].0.clone()], "bold");
        assert!(found[0].1);
    }

    #[test]
    fn marks_clip_to_their_block_and_segment() {
        let m = |segment: usize, r: Range<usize>| Match { message: "m".into(), item: 0, segment, range: r };
        let all = [m(0, 2..6), m(1, 0..3), m(0, 12..14)];
        // A block covering 4..10 of segment 0: the first match is clipped, the third is outside.
        assert_eq!(marks(&all, Some(2), "m", 0, &(4..10)), vec![(0..2, false)]);
        // A match ending exactly at the block's start does not touch it.
        assert!(marks(&all, None, "m", 0, &(6..9)).is_empty());
        // Another message's match is not drawn.
        assert!(marks(&all, None, "other", 0, &(0..20)).is_empty());
        assert_eq!(marks(&all, Some(2), "m", 0, &(10..20)), vec![(2..4, true)]);
    }

    #[test]
    fn runs_are_cut_at_the_marks() {
        let marks = [(2..4, false), (6..8, true)];
        assert_eq!(cut_run(0..10, &marks), vec![(0..2, None), (2..4, Some(false)), (4..6, None), (6..8, Some(true)), (8..10, None)]);
        // A run wholly inside a mark, and one wholly outside.
        assert_eq!(cut_run(2..4, &marks), vec![(2..4, Some(false))]);
        assert_eq!(cut_run(4..6, &marks), vec![(4..6, None)]);
        // A run that starts inside a mark and leaves it.
        assert_eq!(cut_run(3..5, &marks), vec![(3..4, Some(false)), (4..5, None)]);
        // An empty run makes no piece.
        assert!(cut_run(5..5, &marks).is_empty());
    }
}

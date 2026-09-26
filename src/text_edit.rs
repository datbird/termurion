//! The Notes tool's native multi-line editor, pure model: text, caret and selection as byte
//! offsets on char boundaries, the Windows edit-control keys (typing, Enter, Tab, Backspace,
//! Delete, arrows across soft-wrapped lines, Home, End, Shift to select, Ctrl+A/C/X/V/Z/Y and
//! Ctrl+Shift+Z), clicks and drags, undo, and the soft wrap it draws with, measured by a width
//! closure (as `tool_list::truncate_tail` is). iTerm2's Notes is an `NSTextView`
//! (`Toolbelt/ToolNotes.m`, `TN`); the app owns the clipboard, the double-click time and the
//! drawing (the Notes tool, `src/app/toolbelt_more_ui.rs`).

use std::ops::Range;

/// A key the focused editor acts on, already decoded from the raw key event (`ctrl_key` reads
/// the Ctrl chords; the app fills `Paste` with the clipboard text).
#[derive(Clone, Debug, PartialEq)]
pub enum EditKey {
    Text(String),
    Enter,
    Tab,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    SelectAll,
    Copy,
    Cut,
    Paste(String),
    Undo,
    Redo,
}

/// What a key did. `Copied` carries the text for the clipboard; after a Cut the text has also
/// changed, so the caller saves as it does after `Changed`.
#[derive(Clone, Debug, PartialEq)]
pub enum EditOut {
    Changed,
    Moved,
    Copied(String),
    Kept,
}

/// One drawn line: the byte range of `text` it shows. A hard line's `end` stops before its
/// `\n`; a soft-wrapped line keeps its trailing spaces (they hang past the right edge, as in an
/// `NSTextView`), so its `end` is the next line's `start`.
#[derive(Clone, Debug, PartialEq)]
pub struct VisualLine {
    pub start: usize,
    pub end: usize,
}

/// One undo step: at `at`, `removed` was replaced by `inserted`; the caret and anchor before.
#[derive(Clone, Debug)]
struct Step {
    at: usize,
    removed: String,
    inserted: String,
    caret: usize,
    anchor: Option<usize>,
}

/// The editor: `text`, the `caret`, and the selection's other end (`anchor`, `None` or equal
/// to the caret when nothing is selected).
#[derive(Clone, Debug, Default)]
pub struct TextEdit {
    pub text: String,
    pub caret: usize,
    pub anchor: Option<usize>,
    /// The x that Up and Down keep while they repeat; any other key drops it.
    goal_x: Option<f32>,
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// The last plain click (index, time), for double-click detection.
    last_click: Option<(usize, u128)>,
}

impl TextEdit {
    /// Loads new text (a mode switch or the note changed elsewhere): caret at the end, nothing
    /// selected, a fresh undo history (`TN` `loadGlobalNote` removes all undo actions). The
    /// same text changes nothing, so a refresh keeps the caret.
    pub fn set_text(&mut self, t: &str) {
        if self.text == t {
            return;
        }
        self.text = t.to_string();
        self.caret = t.len();
        self.anchor = None;
        self.goal_x = None;
        self.undo.clear();
        self.redo.clear();
    }

    /// The selected byte range, if any.
    pub fn selection(&self) -> Option<Range<usize>> {
        let a = self.anchor?;
        (a != self.caret).then(|| a.min(self.caret)..a.max(self.caret))
    }

    /// One key. `lines` is `wrap` of the current text at the editor's width and `width` the
    /// same measure, for the moves that follow visual lines (Up, Down, Home, End). `shift`
    /// extends the selection on a move.
    pub fn key(&mut self, k: EditKey, shift: bool, lines: &[VisualLine], width: &dyn Fn(&str) -> f32) -> EditOut {
        if !matches!(k, EditKey::Up | EditKey::Down) {
            self.goal_x = None;
        }
        match k {
            EditKey::Text(s) | EditKey::Paste(s) if s.is_empty() => EditOut::Kept,
            EditKey::Text(s) | EditKey::Paste(s) => self.replace(&plain_line_ends(&s)),
            EditKey::Enter => self.replace("\n"),
            EditKey::Tab => self.replace("\t"),
            EditKey::Backspace => {
                let r = match self.selection() {
                    Some(r) => r,
                    None if self.caret == 0 => return EditOut::Kept,
                    None => prev_boundary(&self.text, self.caret)..self.caret,
                };
                self.edit(r, "")
            }
            EditKey::Delete => {
                let r = match self.selection() {
                    Some(r) => r,
                    None if self.caret == self.text.len() => return EditOut::Kept,
                    None => self.caret..next_boundary(&self.text, self.caret),
                };
                self.edit(r, "")
            }
            EditKey::Left => match self.selection() {
                Some(r) if !shift => self.move_to(r.start, false),
                _ => self.move_to(prev_boundary(&self.text, self.caret), shift),
            },
            EditKey::Right => match self.selection() {
                Some(r) if !shift => self.move_to(r.end, false),
                _ => self.move_to(next_boundary(&self.text, self.caret), shift),
            },
            EditKey::Up | EditKey::Down => {
                let (line, x) = caret_pos(&self.text, lines, self.caret, width);
                let goal = *self.goal_x.get_or_insert(x);
                let to = if matches!(k, EditKey::Up) {
                    if line == 0 { 0 } else { index_at(&self.text, lines, line - 1, goal, width) }
                } else if line + 1 >= lines.len() {
                    self.text.len()
                } else {
                    index_at(&self.text, lines, line + 1, goal, width)
                };
                let out = self.move_to(to, shift);
                self.goal_x = Some(goal);
                out
            }
            EditKey::Home => {
                let (line, _) = caret_pos(&self.text, lines, self.caret, width);
                let to = lines.get(line).map_or(0, |l| l.start);
                self.move_to(to, shift)
            }
            EditKey::End => {
                let (line, _) = caret_pos(&self.text, lines, self.caret, width);
                let to = lines.get(line).map_or(self.text.len(), |_| line_limit(&self.text, lines, line));
                self.move_to(to, shift)
            }
            EditKey::SelectAll => {
                self.anchor = Some(0);
                self.caret = self.text.len();
                EditOut::Moved
            }
            EditKey::Copy => match self.selection() {
                Some(r) => EditOut::Copied(self.text[r].to_string()),
                None => EditOut::Kept,
            },
            EditKey::Cut => match self.selection() {
                Some(r) => {
                    let cut = self.text[r].to_string();
                    self.replace("");
                    EditOut::Copied(cut)
                }
                None => EditOut::Kept,
            },
            EditKey::Undo => match self.undo.pop() {
                Some(step) => {
                    let (at, len) = (step.at, step.inserted.len());
                    self.text.replace_range(at..at + len, &step.removed);
                    self.caret = step.caret;
                    self.anchor = step.anchor;
                    self.redo.push(step);
                    EditOut::Changed
                }
                None => EditOut::Kept,
            },
            EditKey::Redo => match self.redo.pop() {
                Some(step) => {
                    let (at, len) = (step.at, step.removed.len());
                    self.text.replace_range(at..at + len, &step.inserted);
                    self.caret = at + step.inserted.len();
                    self.anchor = None;
                    self.undo.push(step);
                    EditOut::Changed
                }
                None => EditOut::Kept,
            },
        }
    }

    /// A press at byte index `at` (from `index_at`). Shift extends the selection; a second
    /// plain press at the same index within `double_ms` selects the word there (a run of word
    /// characters, of spaces, or one other character, as the Windows edit control does); a
    /// third press is a plain press again.
    pub fn click(&mut self, at: usize, shift: bool, now_ms: u128, double_ms: u128) {
        let at = floor_boundary(&self.text, at);
        self.goal_x = None;
        if shift {
            self.anchor = Some(self.anchor.unwrap_or(self.caret));
            self.caret = at;
            self.last_click = None;
            return;
        }
        let double = matches!(self.last_click, Some((i, t)) if i == at && now_ms.saturating_sub(t) <= double_ms);
        if double {
            let r = word_at(&self.text, at);
            self.anchor = Some(r.start);
            self.caret = r.end;
            self.last_click = None;
        } else {
            self.caret = at;
            self.anchor = None;
            self.last_click = Some((at, now_ms));
        }
    }

    /// A drag after a press, to byte index `at`: the selection runs from the press to `at`.
    pub fn drag_to(&mut self, at: usize) {
        let at = floor_boundary(&self.text, at);
        self.anchor = Some(self.anchor.unwrap_or(self.caret));
        self.caret = at;
    }

    /// Replaces the selection (or inserts at the caret) with `s`.
    fn replace(&mut self, s: &str) -> EditOut {
        let r = self.selection().unwrap_or(self.caret..self.caret);
        self.edit(r, s)
    }

    /// Replaces `text[r]` with `s` as one undo step (`TN:585` breaks undo coalescing after
    /// every change, so each edit undoes on its own); the caret lands after `s`.
    fn edit(&mut self, r: Range<usize>, s: &str) -> EditOut {
        self.undo.push(Step {
            at: r.start,
            removed: self.text[r.clone()].to_string(),
            inserted: s.to_string(),
            caret: self.caret,
            anchor: self.anchor,
        });
        self.redo.clear();
        self.text.replace_range(r.clone(), s);
        self.caret = r.start + s.len();
        self.anchor = None;
        EditOut::Changed
    }

    /// Moves the caret, extending the selection when `extend`, else dropping it.
    fn move_to(&mut self, to: usize, extend: bool) -> EditOut {
        if extend {
            self.anchor = Some(self.anchor.unwrap_or(self.caret));
        } else {
            self.anchor = None;
        }
        self.caret = to;
        EditOut::Moved
    }
}

/// `text` broken into visual lines no wider than `max_w`: at each `\n`, then at spaces (a
/// line's trailing spaces hang past the edge and do not count), and inside a word only when
/// the word alone is too wide (as the toolbelt's `wrap_words` breaks a label). Every line
/// holds at least one character, and empty text is one empty line.
pub fn wrap(text: &str, max_w: f32, width: &dyn Fn(&str) -> f32) -> Vec<VisualLine> {
    let mut lines = Vec::new();
    let mut hard = 0;
    for part in text.split('\n') {
        wrap_hard(text, hard, hard + part.len(), max_w, width, &mut lines);
        hard += part.len() + 1;
    }
    lines
}

/// One hard line `text[start..end]` into `lines`.
fn wrap_hard(text: &str, start: usize, end: usize, max_w: f32, width: &dyn Fn(&str) -> f32, lines: &mut Vec<VisualLine>) {
    let mut line = start;
    loop {
        if width(text[line..end].trim_end_matches(' ')) <= max_w {
            lines.push(VisualLine { start: line, end });
            return;
        }
        // The last space-ended word boundary whose text (less its spaces) still fits.
        let mut cut = None;
        let mut i = line;
        while let Some(off) = text[i..end].find(' ') {
            let space = i + off;
            let after = space + text[space..end].len() - text[space..end].trim_start_matches(' ').len();
            if space == line {
                // Leading spaces belong to the first word.
                i = after;
                if after >= end {
                    break;
                }
            } else if width(&text[line..space]) <= max_w {
                cut = Some(after);
                i = after;
                if after >= end {
                    break;
                }
            } else {
                break;
            }
        }
        let cut = cut.unwrap_or_else(|| {
            // One word too wide: the longest char prefix that fits, at least one character.
            let mut c = next_boundary(text, line);
            while c < end {
                let n = next_boundary(text, c);
                if width(&text[line..n]) > max_w {
                    break;
                }
                c = n;
            }
            c
        });
        lines.push(VisualLine { start: line, end: cut });
        if cut >= end {
            // A last character too wide for the line on its own.
            return;
        }
        line = cut;
    }
}

/// The byte index on visual line `line` whose caret sits nearest `x` (an earlier index wins a
/// tie); past a soft-wrapped line's end it stops before the line's last character so the
/// caret stays on that line.
pub fn index_at(text: &str, lines: &[VisualLine], line: usize, x: f32, width: &dyn Fn(&str) -> f32) -> usize {
    let Some(l) = lines.get(line) else { return text.len() };
    let limit = line_limit(text, lines, line);
    let mut best = (l.start, (width("") - x).abs());
    let mut i = l.start;
    while i < limit {
        i = next_boundary(text, i);
        let d = (width(&text[l.start..i]) - x).abs();
        if d < best.1 {
            best = (i, d);
        }
    }
    best.0
}

/// The visual line holding byte index `at` and the caret's x on it. An index where a soft
/// wrap breaks belongs to the line it starts.
pub fn caret_pos(text: &str, lines: &[VisualLine], at: usize, width: &dyn Fn(&str) -> f32) -> (usize, f32) {
    let line = lines.iter().rposition(|l| l.start <= at).unwrap_or(0);
    let Some(l) = lines.get(line) else { return (0, 0.0) };
    let at = floor_boundary(text, at.clamp(l.start, text.len()));
    (line, width(&text[l.start..at]))
}

/// Whether a focused editor takes a key (Review Focus 6): every plain key; with Ctrl only its
/// edit chords (Ctrl+A/C/X/V/Z/Y, Ctrl+Shift+Z); never an Alt or Win chord (Win is Cmd).
pub fn takes(ctrl: bool, alt: bool, win: bool, key: &EditKey) -> bool {
    if alt || win {
        return false;
    }
    if ctrl {
        return matches!(
            key,
            EditKey::SelectAll | EditKey::Copy | EditKey::Cut | EditKey::Paste(_) | EditKey::Undo | EditKey::Redo
        );
    }
    true
}

/// The edit key a Ctrl+letter chord means (the Windows edit control's): A select all, C copy,
/// X cut, V paste (the app fills in the clipboard text), Z undo, Shift+Z and Y redo.
pub fn ctrl_key(c: char, shift: bool) -> Option<EditKey> {
    match (c.to_ascii_lowercase(), shift) {
        ('a', false) => Some(EditKey::SelectAll),
        ('c', false) => Some(EditKey::Copy),
        ('x', false) => Some(EditKey::Cut),
        ('v', false) => Some(EditKey::Paste(String::new())),
        ('z', false) => Some(EditKey::Undo),
        ('z', true) | ('y', false) => Some(EditKey::Redo),
        _ => None,
    }
}

/// Windows line ends (`\r\n`, a lone `\r`) as `\n`, the editor's only line end.
fn plain_line_ends(s: &str) -> String {
    if s.contains('\r') { s.replace("\r\n", "\n").replace('\r', "\n") } else { s.to_string() }
}

/// The furthest index the caret may take on visual line `line`: its end, or before its last
/// character when a soft wrap follows (the next line starts where this one ends).
fn line_limit(text: &str, lines: &[VisualLine], line: usize) -> usize {
    let l = &lines[line];
    match lines.get(line + 1) {
        Some(n) if n.start == l.end && l.end > l.start => prev_boundary(text, l.end),
        _ => l.end,
    }
}

fn prev_boundary(text: &str, i: usize) -> usize {
    text[..i].char_indices().next_back().map_or(0, |(j, _)| j)
}

fn next_boundary(text: &str, i: usize) -> usize {
    text[i..].chars().next().map_or(i, |c| i + c.len_utf8())
}

/// `i` clamped to the text and moved back onto a char boundary.
fn floor_boundary(text: &str, i: usize) -> usize {
    let mut i = i.min(text.len());
    while !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// The double-click word around byte index `at`: the run of characters of the class there
/// (word characters: alphanumerics and `_`; spaces and tabs; anything else is one character
/// alone). At a line end or the text end it is the character before.
fn word_at(text: &str, at: usize) -> Range<usize> {
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            1
        } else if c == ' ' || c == '\t' {
            2
        } else {
            0
        }
    };
    let here = text[at..].chars().next().filter(|&c| c != '\n');
    let (start, c) = match here {
        Some(c) => (at, c),
        None => match text[..at].chars().next_back() {
            Some(c) if c != '\n' => (at - c.len_utf8(), c),
            _ => return at..at,
        },
    };
    let k = class(c);
    if k == 0 {
        return start..start + c.len_utf8();
    }
    let begin = text[..start].char_indices().rev().take_while(|&(_, d)| class(d) == k).last().map_or(start, |(j, _)| j);
    let end = text[start..].char_indices().find(|&(_, d)| class(d) != k).map_or(text.len(), |(j, _)| start + j);
    begin..end
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ten units per character, so positions read as plain column counts.
    fn w(s: &str) -> f32 {
        s.chars().count() as f32 * 10.0
    }

    fn typed(e: &mut TextEdit, s: &str) {
        for c in s.chars() {
            let lines = wrap(&e.text, 1000.0, &w);
            e.key(EditKey::Text(c.to_string()), false, &lines, &w);
        }
    }

    fn press(e: &mut TextEdit, k: EditKey, shift: bool, max_w: f32) -> EditOut {
        let lines = wrap(&e.text, max_w, &w);
        e.key(k, shift, &lines, &w)
    }

    fn vl(start: usize, end: usize) -> VisualLine {
        VisualLine { start, end }
    }

    #[test]
    fn wrap_breaks_at_spaces_then_mid_word() {
        assert_eq!(wrap("aaaa bbbb", 40.0, &w), vec![vl(0, 5), vl(5, 9)]);
        assert_eq!(wrap("abcdefghij", 40.0, &w), vec![vl(0, 4), vl(4, 8), vl(8, 10)]);
        assert_eq!(wrap("", 40.0, &w), vec![vl(0, 0)]);
        assert_eq!(wrap("a\n", 40.0, &w), vec![vl(0, 1), vl(2, 2)]);
        assert_eq!(wrap("ab cd ef", 60.0, &w), vec![vl(0, 6), vl(6, 8)]);
        // Byte offsets on char boundaries: each e-acute is two bytes.
        assert_eq!(wrap("\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}", 40.0, &w), vec![vl(0, 8), vl(8, 10)]);
        // Too narrow for one character: still one character per line.
        assert_eq!(wrap("ab", 5.0, &w), vec![vl(0, 1), vl(1, 2)]);
    }

    #[test]
    fn text_edit_follows_windows_edit_rules() {
        // Typing, Enter and Tab.
        let mut e = TextEdit::default();
        typed(&mut e, "ab");
        assert_eq!(press(&mut e, EditKey::Enter, false, 1000.0), EditOut::Changed);
        typed(&mut e, "c");
        assert_eq!(e.text, "ab\nc");
        assert_eq!(press(&mut e, EditKey::Tab, false, 1000.0), EditOut::Changed);
        assert_eq!(e.text, "ab\nc\t");

        // Backspace and Delete around the caret, then over a selection.
        let mut e = TextEdit::default();
        e.set_text("abcd");
        assert_eq!(e.caret, 4);
        press(&mut e, EditKey::Left, false, 1000.0);
        press(&mut e, EditKey::Left, false, 1000.0);
        assert_eq!(press(&mut e, EditKey::Backspace, false, 1000.0), EditOut::Changed);
        assert_eq!((e.text.as_str(), e.caret), ("acd", 1));
        assert_eq!(press(&mut e, EditKey::Delete, false, 1000.0), EditOut::Changed);
        assert_eq!((e.text.as_str(), e.caret), ("ad", 1));
        press(&mut e, EditKey::Right, false, 1000.0);
        assert_eq!(press(&mut e, EditKey::Delete, false, 1000.0), EditOut::Kept);
        e.set_text("abcdef");
        e.click(1, false, 0, 500);
        assert_eq!(press(&mut e, EditKey::Right, true, 1000.0), EditOut::Moved);
        press(&mut e, EditKey::Right, true, 1000.0);
        assert_eq!(e.selection(), Some(1..3));
        assert_eq!(press(&mut e, EditKey::Backspace, false, 1000.0), EditOut::Changed);
        assert_eq!((e.text.as_str(), e.caret, e.selection()), ("adef", 1, None));
        e.click(1, false, 10_000, 500);
        e.drag_to(3);
        assert_eq!(e.selection(), Some(1..3));
        press(&mut e, EditKey::Delete, false, 1000.0);
        assert_eq!(e.text, "af");

        // Backspace never splits a character.
        e.set_text("a\u{e9}");
        press(&mut e, EditKey::Backspace, false, 1000.0);
        assert_eq!((e.text.as_str(), e.caret), ("a", 1));

        // Left at a selection collapses to its start, Right to its end.
        e.set_text("abcdef");
        e.click(4, false, 20_000, 500);
        press(&mut e, EditKey::Left, true, 1000.0);
        press(&mut e, EditKey::Left, true, 1000.0);
        assert_eq!(e.selection(), Some(2..4));
        assert_eq!(press(&mut e, EditKey::Left, false, 1000.0), EditOut::Moved);
        assert_eq!((e.caret, e.selection()), (2, None));
        press(&mut e, EditKey::Right, true, 1000.0);
        press(&mut e, EditKey::Right, false, 1000.0);
        assert_eq!((e.caret, e.selection()), (3, None));

        // Up and Down keep the x through soft-wrapped lines; a short line in between does not
        // lose it.
        let text = "aaaa bbbb\nc\ndddd";
        assert_eq!(wrap(text, 40.0, &w), vec![vl(0, 5), vl(5, 9), vl(10, 11), vl(12, 16)]);
        e.set_text(text);
        e.click(3, false, 30_000, 500);
        press(&mut e, EditKey::Down, false, 40.0);
        assert_eq!(e.caret, 8);
        press(&mut e, EditKey::Down, false, 40.0);
        assert_eq!(e.caret, 11);
        press(&mut e, EditKey::Down, false, 40.0);
        assert_eq!(e.caret, 15);
        press(&mut e, EditKey::Up, false, 40.0);
        assert_eq!(e.caret, 11);
        press(&mut e, EditKey::Up, false, 40.0);
        assert_eq!(e.caret, 8);
        press(&mut e, EditKey::Up, false, 40.0);
        assert_eq!(e.caret, 3);
        // Up on the first line goes to the start, Down on the last to the end.
        press(&mut e, EditKey::Up, false, 40.0);
        assert_eq!(e.caret, 0);
        e.click(13, false, 40_000, 500);
        press(&mut e, EditKey::Down, false, 40.0);
        assert_eq!(e.caret, 16);
        // Shift+Down selects.
        e.click(2, false, 50_000, 500);
        press(&mut e, EditKey::Down, true, 40.0);
        assert_eq!(e.selection(), Some(2..7));

        // Home and End go to the visual line's ends; End on a soft-wrapped line stops before
        // its hanging space so the caret stays on that line.
        e.click(7, false, 60_000, 500);
        press(&mut e, EditKey::Home, false, 40.0);
        assert_eq!(e.caret, 5);
        press(&mut e, EditKey::End, false, 40.0);
        assert_eq!(e.caret, 9);
        e.click(2, false, 70_000, 500);
        press(&mut e, EditKey::End, false, 40.0);
        assert_eq!(e.caret, 4);
        press(&mut e, EditKey::Home, true, 40.0);
        assert_eq!(e.selection(), Some(0..4));

        // SelectAll then Cut copies everything and empties; Copy without a selection keeps.
        let mut e = TextEdit::default();
        e.set_text("hello world");
        assert_eq!(press(&mut e, EditKey::SelectAll, false, 1000.0), EditOut::Moved);
        assert_eq!(press(&mut e, EditKey::Copy, false, 1000.0), EditOut::Copied("hello world".into()));
        assert_eq!(press(&mut e, EditKey::Cut, false, 1000.0), EditOut::Copied("hello world".into()));
        assert_eq!((e.text.as_str(), e.caret), ("", 0));
        assert_eq!(press(&mut e, EditKey::Copy, false, 1000.0), EditOut::Kept);
        assert_eq!(press(&mut e, EditKey::Cut, false, 1000.0), EditOut::Kept);

        // Paste replaces the selection, with Windows line ends made plain.
        e.set_text("one two");
        e.click(4, false, 80_000, 500);
        press(&mut e, EditKey::End, true, 1000.0);
        assert_eq!(press(&mut e, EditKey::Paste("2\r\n3".into()), false, 1000.0), EditOut::Changed);
        assert_eq!((e.text.as_str(), e.caret), ("one 2\n3", 7));
        assert_eq!(press(&mut e, EditKey::Paste(String::new()), false, 1000.0), EditOut::Kept);

        // Undo and Redo: one step per edit (ToolNotes breaks undo coalescing after every
        // change, `TN:585`); an edit after an Undo drops the Redo.
        let mut e = TextEdit::default();
        typed(&mut e, "ab");
        assert_eq!(press(&mut e, EditKey::Undo, false, 1000.0), EditOut::Changed);
        assert_eq!((e.text.as_str(), e.caret), ("a", 1));
        press(&mut e, EditKey::Undo, false, 1000.0);
        assert_eq!(e.text, "");
        assert_eq!(press(&mut e, EditKey::Undo, false, 1000.0), EditOut::Kept);
        assert_eq!(press(&mut e, EditKey::Redo, false, 1000.0), EditOut::Changed);
        assert_eq!((e.text.as_str(), e.caret), ("a", 1));
        typed(&mut e, "z");
        assert_eq!(press(&mut e, EditKey::Redo, false, 1000.0), EditOut::Kept);
        assert_eq!(e.text, "az");
        // Undo of a Cut brings the text and its selection back.
        press(&mut e, EditKey::SelectAll, false, 1000.0);
        press(&mut e, EditKey::Cut, false, 1000.0);
        press(&mut e, EditKey::Undo, false, 1000.0);
        assert_eq!((e.text.as_str(), e.selection()), ("az", Some(0..2)));
        press(&mut e, EditKey::Redo, false, 1000.0);
        assert_eq!(e.text, "");
        // Loading new text starts a fresh undo history; the same text changes nothing.
        e.set_text("loaded");
        assert_eq!(press(&mut e, EditKey::Undo, false, 1000.0), EditOut::Kept);
        e.click(2, false, 90_000, 500);
        e.set_text("loaded");
        assert_eq!(e.caret, 2);

        // A click places the caret by index_at (nearest boundary); a double-click selects a
        // word; Shift+click extends.
        let text = "aaaa bbbb";
        let lines = wrap(text, 40.0, &w);
        assert_eq!(index_at(text, &lines, 1, 24.0, &w), 7);
        assert_eq!(index_at(text, &lines, 1, 26.0, &w), 8);
        assert_eq!(index_at(text, &lines, 1, 999.0, &w), 9);
        // Past a soft-wrapped line's end: before its hanging space.
        assert_eq!(index_at(text, &lines, 0, 999.0, &w), 4);
        assert_eq!(caret_pos(text, &lines, 5, &w), (1, 0.0));
        assert_eq!(caret_pos(text, &lines, 4, &w), (0, 40.0));
        assert_eq!(caret_pos(text, &lines, 9, &w), (1, 40.0));
        let mut e = TextEdit::default();
        e.set_text(text);
        e.click(index_at(text, &lines, 1, 24.0, &w), false, 100_000, 500);
        assert_eq!((e.caret, e.selection()), (7, None));
        e.click(7, false, 100_300, 500);
        assert_eq!(e.selection(), Some(5..9));
        // A third quick click is a plain click again.
        e.click(7, false, 100_400, 500);
        assert_eq!((e.caret, e.selection()), (7, None));
        // Too slow for a double-click.
        e.click(7, false, 101_000, 500);
        assert_eq!(e.selection(), None);
        e.click(2, true, 102_000, 500);
        assert_eq!(e.selection(), Some(2..7));
        // A double-click on a space selects the run of spaces.
        e.set_text("ab   cd");
        e.click(3, false, 200_000, 500);
        e.click(3, false, 200_100, 500);
        assert_eq!(e.selection(), Some(2..5));

        // Keys a focused editor takes: every plain key, its Ctrl edit chords; never another
        // Ctrl chord, an Alt chord or a Win chord.
        assert!(takes(false, false, false, &EditKey::Text("x".into())));
        assert!(takes(false, false, false, &EditKey::Enter));
        assert!(takes(false, false, false, &EditKey::Tab));
        for k in [EditKey::SelectAll, EditKey::Copy, EditKey::Cut, EditKey::Paste("p".into()), EditKey::Undo, EditKey::Redo] {
            assert!(takes(true, false, false, &k), "{k:?}");
        }
        assert!(takes(true, false, false, &EditKey::Copy));
        assert!(!takes(true, false, false, &EditKey::Text("x".into())));
        assert!(!takes(true, false, false, &EditKey::Enter));
        assert!(!takes(false, true, false, &EditKey::Copy));
        assert!(!takes(false, true, false, &EditKey::Text("x".into())));
        assert!(!takes(false, false, true, &EditKey::Copy));
        assert!(!takes(false, false, true, &EditKey::Text("x".into())));

        // The Ctrl chords, as the Windows edit control reads them.
        assert_eq!(ctrl_key('a', false), Some(EditKey::SelectAll));
        assert_eq!(ctrl_key('C', false), Some(EditKey::Copy));
        assert_eq!(ctrl_key('x', false), Some(EditKey::Cut));
        assert_eq!(ctrl_key('v', false), Some(EditKey::Paste(String::new())));
        assert_eq!(ctrl_key('z', false), Some(EditKey::Undo));
        assert_eq!(ctrl_key('z', true), Some(EditKey::Redo));
        assert_eq!(ctrl_key('y', false), Some(EditKey::Redo));
        assert_eq!(ctrl_key('b', false), None);
        assert_eq!(ctrl_key('a', true), None);
    }
}

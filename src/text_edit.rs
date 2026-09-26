//! The Notes tool's native multi-line editor, pure model: text, caret and selection as byte
//! offsets on char boundaries, the Windows edit-control keys (typing, Enter, Tab, Backspace,
//! Delete, arrows across soft-wrapped lines, Home, End, Page Up and Page Down, the Ctrl word
//! and document moves and word deletes, Shift to select, Ctrl+A/C/X/V/Z/Y and Ctrl+Shift+Z),
//! clicks and drags, undo, and the soft wrap it draws with, measured by a width closure (as
//! `tool_list::truncate_tail` is). iTerm2's Notes is an `NSTextView` (`Toolbelt/ToolNotes.m`,
//! `TN`); the app owns the clipboard, the double-click time and the drawing (the Notes tool,
//! `src/app/toolbelt_more_ui.rs`).
//!
//! Every function taking `lines` expects `wrap` of the editor's current text (at the width it
//! is drawn at); stale lines give wrong moves and positions.

use std::ops::Range;

/// A key the focused editor acts on, already decoded from the raw key event (`ctrl_key` reads
/// the Ctrl+letter chords and `ctrl_nav` the Ctrl+arrow family; the app fills `Paste` with the
/// clipboard text).
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
    PageUp,
    PageDown,
    /// Ctrl+Left and Ctrl+Right: to the start of the previous or next word (`word_left`,
    /// `word_right`).
    WordLeft,
    WordRight,
    /// Ctrl+Backspace and Ctrl+Delete: delete back to `word_left` or on to `word_right` (or
    /// the selection).
    DeleteWordBack,
    DeleteWordForward,
    /// Ctrl+Home and Ctrl+End: the start or end of the text.
    TextStart,
    TextEnd,
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
    /// Caret affinity: where a soft wrap breaks, the caret index is both the end of one
    /// visual line and the start of the next; true draws it at the end of the earlier line
    /// (set by End, a click or drag past a line's end, and Up, Down, Page Up and Page Down
    /// landing there; every other move and every edit clears it).
    pub at_line_end: bool,
    /// Visual lines the editor shows at once, for Page Up and Page Down (at least 1 is used).
    pub page_lines: usize,
    /// The x that Up, Down, Page Up and Page Down keep while they repeat; any other key drops it.
    goal_x: Option<f32>,
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// The last plain click (index, time), for double-click detection.
    last_click: Option<(usize, u128)>,
}

impl TextEdit {
    /// Shows `t` (Windows line ends made `\n`), keeping the caret, selection and undo when
    /// it is already the text (a `tools_changed` refresh); otherwise as `load`.
    pub fn set_text(&mut self, t: &str) {
        let t = plain_line_ends(t);
        if self.text != t {
            self.load(&t);
        }
    }

    /// Loads `t` (Windows line ends made `\n`) as a new note, even when the text is the same
    /// (a mode switch between two notes): caret at the end, nothing selected, a fresh undo
    /// history (`TN` `loadGlobalNote` removes all undo actions).
    pub fn load(&mut self, t: &str) {
        self.text = plain_line_ends(t);
        self.caret = self.text.len();
        self.anchor = None;
        self.at_line_end = false;
        self.goal_x = None;
        self.undo.clear();
        self.redo.clear();
        self.last_click = None;
    }

    /// The selected byte range, if any.
    pub fn selection(&self) -> Option<Range<usize>> {
        let a = self.anchor?;
        (a != self.caret).then(|| a.min(self.caret)..a.max(self.caret))
    }

    /// The caret's visual line and x (`caret_pos` with this editor's affinity). `lines` must be
    /// `wrap` of the current text.
    pub fn caret_line(&self, lines: &[VisualLine], width: &dyn Fn(&str) -> f32) -> (usize, f32) {
        caret_pos(&self.text, lines, self.caret, self.at_line_end, width)
    }

    /// One key. `lines` must be `wrap` of the current text at the editor's width and `width`
    /// the same measure, for the moves that follow visual lines (Up, Down, Home, End, Page Up,
    /// Page Down). `shift` extends the selection on a move.
    pub fn key(&mut self, k: EditKey, shift: bool, lines: &[VisualLine], width: &dyn Fn(&str) -> f32) -> EditOut {
        if !matches!(k, EditKey::Up | EditKey::Down | EditKey::PageUp | EditKey::PageDown) {
            self.goal_x = None;
        }
        match k {
            EditKey::Text(s) | EditKey::Paste(s) if s.is_empty() => EditOut::Kept,
            EditKey::Text(s) | EditKey::Paste(s) => self.replace(&plain_line_ends(&s)),
            EditKey::Enter => self.replace("\n"),
            EditKey::Tab => self.replace("\t"),
            EditKey::Backspace => {
                let from = prev_boundary(&self.text, self.caret);
                self.delete_to(from)
            }
            EditKey::Delete => {
                let to = next_boundary(&self.text, self.caret);
                self.delete_to(to)
            }
            EditKey::DeleteWordBack => {
                let from = word_left(&self.text, self.caret);
                self.delete_to(from)
            }
            EditKey::DeleteWordForward => {
                let to = word_right(&self.text, self.caret);
                self.delete_to(to)
            }
            EditKey::Left => match self.selection() {
                Some(r) if !shift => self.move_to(r.start, false),
                _ => self.move_to(prev_boundary(&self.text, self.caret), shift),
            },
            EditKey::Right => match self.selection() {
                Some(r) if !shift => self.move_to(r.end, false),
                _ => self.move_to(next_boundary(&self.text, self.caret), shift),
            },
            EditKey::WordLeft => self.move_to(word_left(&self.text, self.caret), shift),
            EditKey::WordRight => self.move_to(word_right(&self.text, self.caret), shift),
            EditKey::TextStart => self.move_to(0, shift),
            EditKey::TextEnd => self.move_to(self.text.len(), shift),
            EditKey::Up => self.vertical(-1, shift, lines, width),
            EditKey::Down => self.vertical(1, shift, lines, width),
            EditKey::PageUp => self.vertical(-(self.page_lines.max(1) as isize), shift, lines, width),
            EditKey::PageDown => self.vertical(self.page_lines.max(1) as isize, shift, lines, width),
            EditKey::Home => {
                let (line, _) = self.caret_line(lines, width);
                let to = lines.get(line).map_or(0, |l| l.start);
                self.move_to(to, shift)
            }
            EditKey::End => {
                let (line, _) = self.caret_line(lines, width);
                let to = lines.get(line).map_or(self.text.len(), |l| l.end);
                let out = self.move_to(to, shift);
                self.at_line_end = ends_line(lines, line, to);
                out
            }
            EditKey::SelectAll => {
                self.anchor = Some(0);
                self.caret = self.text.len();
                self.at_line_end = false;
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
                    self.at_line_end = false;
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
                    self.at_line_end = false;
                    self.undo.push(step);
                    EditOut::Changed
                }
                None => EditOut::Kept,
            },
        }
    }

    /// A press at byte index `at` (from `index_at`, with `at_line_end` from `ends_line` for the
    /// line pressed on). Shift extends the selection; a second plain press at the same index
    /// within `double_ms` selects the word there (a run of word characters, of spaces, or one
    /// other character, as the Windows edit control does); a third press is a plain press again.
    pub fn click(&mut self, at: usize, at_line_end: bool, shift: bool, now_ms: u128, double_ms: u128) {
        let at = floor_boundary(&self.text, at);
        self.goal_x = None;
        if shift {
            self.anchor = Some(self.anchor.unwrap_or(self.caret));
            self.caret = at;
            self.at_line_end = at_line_end;
            self.last_click = None;
            return;
        }
        let double = matches!(self.last_click, Some((i, t)) if i == at && now_ms.saturating_sub(t) <= double_ms);
        if double {
            let r = word_at(&self.text, at);
            self.anchor = Some(r.start);
            self.caret = r.end;
            self.at_line_end = false;
            self.last_click = None;
        } else {
            self.caret = at;
            self.anchor = None;
            self.at_line_end = at_line_end;
            self.last_click = Some((at, now_ms));
        }
    }

    /// A drag after a press, to byte index `at` (and its affinity, as `click` takes): the
    /// selection runs from the press to `at`.
    pub fn drag_to(&mut self, at: usize, at_line_end: bool) {
        let at = floor_boundary(&self.text, at);
        self.anchor = Some(self.anchor.unwrap_or(self.caret));
        self.caret = at;
        self.at_line_end = at_line_end;
    }

    /// Up (`by` < 0) or Down by `by` visual lines, keeping the goal x. Past the first line it
    /// goes to the start of the text, past the last to its end (NSTextView).
    fn vertical(&mut self, by: isize, shift: bool, lines: &[VisualLine], width: &dyn Fn(&str) -> f32) -> EditOut {
        let (line, x) = self.caret_line(lines, width);
        let goal = *self.goal_x.get_or_insert(x);
        let target = line as isize + by;
        let (to, end) = if target < 0 || (by < 0 && line == 0) {
            (0, false)
        } else if by > 0 && line + 1 >= lines.len() {
            (self.text.len(), false)
        } else {
            let target = (target as usize).min(lines.len().saturating_sub(1));
            let to = index_at(&self.text, lines, target, goal, width);
            (to, ends_line(lines, target, to))
        };
        let out = self.move_to(to, shift);
        self.at_line_end = end;
        self.goal_x = Some(goal);
        out
    }

    /// Deletes the selection, or else from the caret to `to` (either side); nothing to delete
    /// changes nothing.
    fn delete_to(&mut self, to: usize) -> EditOut {
        let r = match self.selection() {
            Some(r) => r,
            None if to == self.caret => return EditOut::Kept,
            None => self.caret.min(to)..self.caret.max(to),
        };
        self.edit(r, "")
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
        self.at_line_end = false;
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
        self.at_line_end = false;
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
/// tie), from the line's start to its end. `lines` must be `wrap` of `text`. Past a
/// soft-wrapped line's end this is the line's `end`, which is also the next line's start:
/// pass `ends_line` for it to `TextEdit::click` so the caret stays on this line.
pub fn index_at(text: &str, lines: &[VisualLine], line: usize, x: f32, width: &dyn Fn(&str) -> f32) -> usize {
    let Some(l) = lines.get(line) else { return text.len() };
    let mut best = (l.start, (width("") - x).abs());
    let mut i = l.start;
    while i < l.end {
        i = next_boundary(text, i);
        let d = (width(&text[l.start..i]) - x).abs();
        if d < best.1 {
            best = (i, d);
        }
    }
    best.0
}

/// Whether byte index `at` on visual line `line` is that line's end where a soft wrap breaks
/// (the next line starts there), so the caret there needs `at_line_end` to draw on `line`.
pub fn ends_line(lines: &[VisualLine], line: usize, at: usize) -> bool {
    match (lines.get(line), lines.get(line + 1)) {
        (Some(l), Some(n)) => at == l.end && n.start == l.end && l.end > l.start,
        _ => false,
    }
}

/// The visual line holding byte index `at` and the caret's x on it. `lines` must be `wrap`
/// of `text`. An index where a soft wrap breaks belongs to the line it starts, unless
/// `at_line_end` (the caret's affinity) puts it at the end of the line before.
pub fn caret_pos(text: &str, lines: &[VisualLine], at: usize, at_line_end: bool, width: &dyn Fn(&str) -> f32) -> (usize, f32) {
    let mut line = lines.iter().rposition(|l| l.start <= at).unwrap_or(0);
    if at_line_end && line > 0 && ends_line(lines, line - 1, at) {
        line -= 1;
    }
    let Some(l) = lines.get(line) else { return (0, 0.0) };
    let at = floor_boundary(text, at.clamp(l.start, text.len()));
    (line, width(&text[l.start..at]))
}

/// Whether a focused editor takes a key (Review Focus 6): every plain key; with Ctrl only its
/// edit chords (Ctrl+A/C/X/V/Z/Y, Ctrl+Shift+Z) and the Ctrl word, document and page moves
/// and word deletes; never an Alt or Win chord (Win is Cmd).
pub fn takes(ctrl: bool, alt: bool, win: bool, key: &EditKey) -> bool {
    if alt || win {
        return false;
    }
    if ctrl {
        return matches!(
            key,
            EditKey::SelectAll
                | EditKey::Copy
                | EditKey::Cut
                | EditKey::Paste(_)
                | EditKey::Undo
                | EditKey::Redo
                | EditKey::WordLeft
                | EditKey::WordRight
                | EditKey::DeleteWordBack
                | EditKey::DeleteWordForward
                | EditKey::TextStart
                | EditKey::TextEnd
                | EditKey::PageUp
                | EditKey::PageDown
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

/// The edit key a named key means with Ctrl held (the Windows edit control's): Left and Right
/// move by words, Backspace and Delete delete words, Home and End go to the text's ends, Page
/// Up and Page Down page as without Ctrl. Any other key has no Ctrl meaning.
pub fn ctrl_nav(key: &EditKey) -> Option<EditKey> {
    match key {
        EditKey::Left => Some(EditKey::WordLeft),
        EditKey::Right => Some(EditKey::WordRight),
        EditKey::Backspace => Some(EditKey::DeleteWordBack),
        EditKey::Delete => Some(EditKey::DeleteWordForward),
        EditKey::Home => Some(EditKey::TextStart),
        EditKey::End => Some(EditKey::TextEnd),
        EditKey::PageUp => Some(EditKey::PageUp),
        EditKey::PageDown => Some(EditKey::PageDown),
        _ => None,
    }
}

/// A character's class for word moves and double-clicks: 1 a word character (alphanumerics
/// and `_`), 2 a space or tab, 3 a line end, 0 anything else (punctuation).
fn class(c: char) -> u8 {
    if c.is_alphanumeric() || c == '_' {
        1
    } else if c == ' ' || c == '\t' {
        2
    } else if c == '\n' {
        3
    } else {
        0
    }
}

/// Ctrl+Right's stop from `at` (Windows edit rules): past the run of word characters or of
/// punctuation there, then past the spaces and tabs after it; a line end is a stop of its own
/// (at one, the move goes just past it).
pub fn word_right(text: &str, at: usize) -> usize {
    let mut chars = text[at..].char_indices().map(|(j, c)| (at + j, c)).peekable();
    let Some(&(_, first)) = chars.peek() else { return at };
    if class(first) == 3 {
        return at + 1;
    }
    let mut i = at;
    if class(first) != 2 {
        while let Some(&(j, c)) = chars.peek() {
            if class(c) != class(first) {
                break;
            }
            i = j + c.len_utf8();
            chars.next();
        }
    }
    while let Some(&(j, c)) = chars.peek() {
        if class(c) != 2 {
            break;
        }
        i = j + c.len_utf8();
        chars.next();
    }
    i
}

/// Ctrl+Left's stop from `at` (Windows edit rules): back past spaces and tabs, then back to
/// the start of the run of word characters or of punctuation before; a line end is a stop of
/// its own (just before it).
pub fn word_left(text: &str, at: usize) -> usize {
    let mut chars = text[..at].char_indices().rev().peekable();
    let mut i = at;
    while let Some(&(j, c)) = chars.peek() {
        if class(c) != 2 {
            break;
        }
        i = j;
        chars.next();
    }
    let Some(&(j, first)) = chars.peek() else { return i };
    if class(first) == 3 {
        return if i == at { j } else { i };
    }
    while let Some(&(j, c)) = chars.peek() {
        if class(c) != class(first) {
            break;
        }
        i = j;
        chars.next();
    }
    i
}

/// Windows line ends (`\r\n`, a lone `\r`) as `\n`, the editor's only line end.
fn plain_line_ends(s: &str) -> String {
    if s.contains('\r') { s.replace("\r\n", "\n").replace('\r', "\n") } else { s.to_string() }
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
        e.click(1, false, false, 0, 500);
        assert_eq!(press(&mut e, EditKey::Right, true, 1000.0), EditOut::Moved);
        press(&mut e, EditKey::Right, true, 1000.0);
        assert_eq!(e.selection(), Some(1..3));
        assert_eq!(press(&mut e, EditKey::Backspace, false, 1000.0), EditOut::Changed);
        assert_eq!((e.text.as_str(), e.caret, e.selection()), ("adef", 1, None));
        e.click(1, false, false, 10_000, 500);
        e.drag_to(3, false);
        assert_eq!(e.selection(), Some(1..3));
        press(&mut e, EditKey::Delete, false, 1000.0);
        assert_eq!(e.text, "af");

        // Backspace never splits a character.
        e.set_text("a\u{e9}");
        press(&mut e, EditKey::Backspace, false, 1000.0);
        assert_eq!((e.text.as_str(), e.caret), ("a", 1));

        // Left at a selection collapses to its start, Right to its end.
        e.set_text("abcdef");
        e.click(4, false, false, 20_000, 500);
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
        e.click(3, false, false, 30_000, 500);
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
        e.click(13, false, false, 40_000, 500);
        press(&mut e, EditKey::Down, false, 40.0);
        assert_eq!(e.caret, 16);
        // Shift+Down selects.
        e.click(2, false, false, 50_000, 500);
        press(&mut e, EditKey::Down, true, 40.0);
        assert_eq!(e.selection(), Some(2..7));

        // Home and End go to the visual line's ends. End on a soft-wrapped line goes past its
        // hanging space and keeps the caret drawn on that line (affinity); Home from there
        // stays on it too.
        e.click(7, false, false, 60_000, 500);
        press(&mut e, EditKey::Home, false, 40.0);
        assert_eq!(e.caret, 5);
        press(&mut e, EditKey::End, false, 40.0);
        assert_eq!((e.caret, e.at_line_end), (9, false));
        e.click(2, false, false, 70_000, 500);
        press(&mut e, EditKey::End, false, 40.0);
        assert_eq!((e.caret, e.at_line_end), (5, true));
        assert_eq!(e.caret_line(&wrap(text, 40.0, &w), &w), (0, 50.0));
        press(&mut e, EditKey::Home, true, 40.0);
        assert_eq!((e.selection(), e.at_line_end), (Some(0..5), false));

        // A line broken mid-word: End reaches the line's true end (after its last character),
        // typing there inserts after it, Shift+End selects it, and a click past the end lands
        // there drawn on that line.
        let text = "abcdefghij";
        let lines = wrap(text, 40.0, &w);
        assert_eq!(lines, vec![vl(0, 4), vl(4, 8), vl(8, 10)]);
        e.load(text);
        e.click(1, false, false, 75_000, 500);
        press(&mut e, EditKey::End, false, 40.0);
        assert_eq!((e.caret, e.at_line_end), (4, true));
        assert_eq!(e.caret_line(&lines, &w), (0, 40.0));
        // End again stays put (still on line 0), and Home goes back to line 0's start.
        press(&mut e, EditKey::End, false, 40.0);
        assert_eq!((e.caret, e.at_line_end), (4, true));
        press(&mut e, EditKey::Home, false, 40.0);
        assert_eq!(e.caret, 0);
        press(&mut e, EditKey::End, true, 40.0);
        assert_eq!(e.selection(), Some(0..4));
        press(&mut e, EditKey::Right, false, 40.0);
        assert_eq!((e.caret, e.at_line_end), (4, false));
        assert_eq!(e.caret_line(&lines, &w), (1, 0.0));
        e.click(0, false, false, 76_000, 500);
        press(&mut e, EditKey::End, false, 40.0);
        press(&mut e, EditKey::Text("X".into()), false, 40.0);
        assert_eq!((e.text.as_str(), e.caret, e.at_line_end), ("abcdXefghij", 5, false));
        e.load(text);
        let at = index_at(text, &lines, 0, 999.0, &w);
        assert_eq!((at, ends_line(&lines, 0, at)), (4, true));
        assert!(!ends_line(&lines, 1, 4));
        assert!(!ends_line(&lines, 2, 10));
        e.click(at, ends_line(&lines, 0, at), false, 77_000, 500);
        assert_eq!(e.caret_line(&lines, &w), (0, 40.0));
        // Down from there keeps x 40: the end of line 1, drawn on line 1.
        press(&mut e, EditKey::Down, false, 40.0);
        assert_eq!((e.caret, e.at_line_end), (8, true));
        assert_eq!(e.caret_line(&lines, &w), (1, 40.0));
        // A drag past line 1's end selects through its last character.
        e.click(5, false, false, 78_000, 500);
        e.drag_to(8, true);
        assert_eq!((e.selection(), e.caret_line(&lines, &w)), (Some(5..8), (1, 40.0)));

        // Ctrl word moves (Windows rules: past the word or punctuation run, then its spaces;
        // a line end is a stop of its own), each extending with Shift.
        let text = "foo  bar.baz\nqux";
        e.load(text);
        e.click(0, false, false, 79_000, 500);
        let mut stops = Vec::new();
        for _ in 0..7 {
            press(&mut e, EditKey::WordRight, false, 1000.0);
            stops.push(e.caret);
        }
        assert_eq!(stops, vec![5, 8, 9, 12, 13, 16, 16]);
        let mut stops = Vec::new();
        for _ in 0..7 {
            press(&mut e, EditKey::WordLeft, false, 1000.0);
            stops.push(e.caret);
        }
        assert_eq!(stops, vec![13, 12, 9, 8, 5, 0, 0]);
        press(&mut e, EditKey::WordRight, true, 1000.0);
        press(&mut e, EditKey::WordRight, true, 1000.0);
        assert_eq!(e.selection(), Some(0..8));
        e.click(8, false, false, 80_000, 500);
        press(&mut e, EditKey::WordLeft, true, 1000.0);
        assert_eq!(e.selection(), Some(5..8));

        // Ctrl+Backspace and Ctrl+Delete delete a word (or the selection).
        e.load("one two three");
        e.click(8, false, false, 81_000, 500);
        assert_eq!(press(&mut e, EditKey::DeleteWordBack, false, 1000.0), EditOut::Changed);
        assert_eq!((e.text.as_str(), e.caret), ("one three", 4));
        assert_eq!(press(&mut e, EditKey::DeleteWordForward, false, 1000.0), EditOut::Changed);
        assert_eq!((e.text.as_str(), e.caret), ("one ", 4));
        assert_eq!(press(&mut e, EditKey::DeleteWordForward, false, 1000.0), EditOut::Kept);
        e.click(1, false, false, 82_000, 500);
        e.drag_to(2, false);
        press(&mut e, EditKey::DeleteWordBack, false, 1000.0);
        assert_eq!(e.text, "oe ");
        e.click(0, false, false, 83_000, 500);
        assert_eq!(press(&mut e, EditKey::DeleteWordBack, false, 1000.0), EditOut::Kept);

        // Ctrl+Home and Ctrl+End go to the text's ends; Shift selects.
        e.load("ab\ncd\nef");
        e.click(4, false, false, 84_000, 500);
        press(&mut e, EditKey::TextStart, false, 1000.0);
        assert_eq!(e.caret, 0);
        press(&mut e, EditKey::TextEnd, true, 1000.0);
        assert_eq!(e.selection(), Some(0..8));
        e.click(4, false, false, 85_000, 500);
        press(&mut e, EditKey::TextStart, true, 1000.0);
        assert_eq!(e.selection(), Some(0..4));

        // Page Up and Page Down move by the visible line count keeping x; past either end they
        // go to the text's ends; Shift selects.
        let text = "l0\nl1\nl2\nl3\nl4\nl5";
        e.load(text);
        e.page_lines = 2;
        e.click(1, false, false, 86_000, 500);
        press(&mut e, EditKey::PageDown, false, 1000.0);
        assert_eq!(e.caret, 7);
        press(&mut e, EditKey::PageDown, true, 1000.0);
        assert_eq!(e.selection(), Some(7..13));
        press(&mut e, EditKey::PageDown, false, 1000.0);
        assert_eq!(e.caret, 16);
        press(&mut e, EditKey::PageDown, false, 1000.0);
        assert_eq!(e.caret, 17);
        press(&mut e, EditKey::PageUp, false, 1000.0);
        assert_eq!(e.caret, 10);
        press(&mut e, EditKey::PageUp, false, 1000.0);
        assert_eq!(e.caret, 4);
        press(&mut e, EditKey::PageUp, false, 1000.0);
        assert_eq!(e.caret, 0);
        // No page size set: one line.
        e.page_lines = 0;
        e.click(1, false, false, 87_000, 500);
        press(&mut e, EditKey::PageDown, false, 1000.0);
        assert_eq!(e.caret, 4);
        press(&mut e, EditKey::PageUp, true, 1000.0);
        assert_eq!(e.selection(), Some(1..4));

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
        e.click(4, false, false, 80_000, 500);
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
        // Loading new text starts a fresh undo history; setting the same text changes
        // nothing, and Windows line ends read as the same text; `load` resets even then.
        e.set_text("loaded");
        assert_eq!(press(&mut e, EditKey::Undo, false, 1000.0), EditOut::Kept);
        e.click(2, false, false, 90_000, 500);
        e.set_text("loaded");
        assert_eq!(e.caret, 2);
        e.set_text("a\r\nb\rc");
        assert_eq!((e.text.as_str(), e.caret), ("a\nb\nc", 5));
        typed(&mut e, "d");
        e.set_text("a\nb\ncd");
        assert_eq!(press(&mut e, EditKey::Undo, false, 1000.0), EditOut::Changed);
        typed(&mut e, "d");
        e.load("a\nb\ncd");
        assert_eq!((e.caret, press(&mut e, EditKey::Undo, false, 1000.0)), (6, EditOut::Kept));

        // A click places the caret by index_at (nearest boundary); a double-click selects a
        // word; Shift+click extends.
        let text = "aaaa bbbb";
        let lines = wrap(text, 40.0, &w);
        assert_eq!(index_at(text, &lines, 1, 24.0, &w), 7);
        assert_eq!(index_at(text, &lines, 1, 26.0, &w), 8);
        assert_eq!(index_at(text, &lines, 1, 999.0, &w), 9);
        // Past a soft-wrapped line's end: past its hanging space, the next line's start,
        // drawn on this line only with the affinity.
        assert_eq!(index_at(text, &lines, 0, 999.0, &w), 5);
        assert_eq!(caret_pos(text, &lines, 5, false, &w), (1, 0.0));
        assert_eq!(caret_pos(text, &lines, 5, true, &w), (0, 50.0));
        assert_eq!(caret_pos(text, &lines, 4, false, &w), (0, 40.0));
        assert_eq!(caret_pos(text, &lines, 4, true, &w), (0, 40.0));
        assert_eq!(caret_pos(text, &lines, 9, false, &w), (1, 40.0));
        let mut e = TextEdit::default();
        e.set_text(text);
        e.click(index_at(text, &lines, 1, 24.0, &w), false, false, 100_000, 500);
        assert_eq!((e.caret, e.selection()), (7, None));
        e.click(7, false, false, 100_300, 500);
        assert_eq!(e.selection(), Some(5..9));
        // A third quick click is a plain click again.
        e.click(7, false, false, 100_400, 500);
        assert_eq!((e.caret, e.selection()), (7, None));
        // Too slow for a double-click.
        e.click(7, false, false, 101_000, 500);
        assert_eq!(e.selection(), None);
        e.click(2, false, true, 102_000, 500);
        assert_eq!(e.selection(), Some(2..7));
        // A double-click on a space selects the run of spaces.
        e.set_text("ab   cd");
        e.click(3, false, false, 200_000, 500);
        e.click(3, false, false, 200_100, 500);
        assert_eq!(e.selection(), Some(2..5));

        // Keys a focused editor takes: every plain key, its Ctrl edit chords; never another
        // Ctrl chord, an Alt chord or a Win chord.
        assert!(takes(false, false, false, &EditKey::Text("x".into())));
        assert!(takes(false, false, false, &EditKey::Enter));
        assert!(takes(false, false, false, &EditKey::Tab));
        for k in [EditKey::SelectAll, EditKey::Copy, EditKey::Cut, EditKey::Paste("p".into()), EditKey::Undo, EditKey::Redo] {
            assert!(takes(true, false, false, &k), "{k:?}");
        }
        for k in [
            EditKey::WordLeft,
            EditKey::WordRight,
            EditKey::DeleteWordBack,
            EditKey::DeleteWordForward,
            EditKey::TextStart,
            EditKey::TextEnd,
            EditKey::PageUp,
            EditKey::PageDown,
        ] {
            assert!(takes(true, false, false, &k), "Ctrl {k:?}");
            assert!(takes(false, false, false, &k), "{k:?}");
            assert!(!takes(false, true, false, &k), "Alt {k:?}");
            assert!(!takes(false, false, true, &k), "Win {k:?}");
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
        assert_eq!(ctrl_nav(&EditKey::Left), Some(EditKey::WordLeft));
        assert_eq!(ctrl_nav(&EditKey::Right), Some(EditKey::WordRight));
        assert_eq!(ctrl_nav(&EditKey::Backspace), Some(EditKey::DeleteWordBack));
        assert_eq!(ctrl_nav(&EditKey::Delete), Some(EditKey::DeleteWordForward));
        assert_eq!(ctrl_nav(&EditKey::Home), Some(EditKey::TextStart));
        assert_eq!(ctrl_nav(&EditKey::End), Some(EditKey::TextEnd));
        assert_eq!(ctrl_nav(&EditKey::PageDown), Some(EditKey::PageDown));
        assert_eq!(ctrl_nav(&EditKey::Up), None);
    }
}

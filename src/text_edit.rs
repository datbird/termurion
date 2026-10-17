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
//!
//! The composer (milestone 9l) adds iTerm2's multiple carets (`MultiCursor/MultiCursorTextView
//! .swift`, `MCTV`): Ctrl+Shift+Up and Down, Ctrl+Shift+click and Alt+drag add carets, every
//! typed key and delete acts at each caret as one undo step, and a multi-caret copy pastes back
//! one group per caret. With one caret the editor behaves as before (the Notes tool).

use std::ops::Range;

/// Spaces a multi-caret Tab inserts (`MCTV:88` `tabStop`).
const TAB_STOP: usize = 4;

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

/// A caret other than the primary: its byte index and its selection's other end (`None` or
/// equal to `at` when nothing is selected).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Caret {
    pub at: usize,
    pub anchor: Option<usize>,
}

impl Caret {
    /// The caret's selected byte range (empty for a plain caret).
    fn range(&self) -> Range<usize> {
        let a = self.anchor.unwrap_or(self.at);
        a.min(self.at)..a.max(self.at)
    }
}

/// One replacement in an undo step: at `at` (an index in the text before the step), `removed`
/// was replaced by `inserted`.
#[derive(Clone, Debug)]
struct Part {
    at: usize,
    removed: String,
    inserted: String,
}

/// Every caret at one moment: the primary and the others.
#[derive(Clone, Debug)]
struct Carets {
    caret: usize,
    anchor: Option<usize>,
    extra: Vec<Caret>,
}

/// One undo step: its replacements in text order (one per caret that changed the text) and
/// the carets before and after.
#[derive(Clone, Debug)]
struct Step {
    parts: Vec<Part>,
    before: Carets,
    after: Carets,
}

/// The editor: `text`, the `caret`, and the selection's other end (`anchor`, `None` or equal
/// to the caret when nothing is selected).
#[derive(Clone, Debug, Default)]
pub struct TextEdit {
    pub text: String,
    pub caret: usize,
    pub anchor: Option<usize>,
    /// The other carets, in text order; `caret` and `anchor` are the primary, the one added
    /// last (`MCTV` adds a caret from the last range, `handleSpecialKeyDown`). Empty with one
    /// caret. Only the primary has an affinity.
    pub extra: Vec<Caret>,
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
        self.extra.clear();
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
    /// With other carets, typing and deleting act at each caret as one undo step, moves move
    /// each (Up and Down as `MCTV` does, dropping a caret with no line to go to), Copy and Cut
    /// take every caret's piece joined by line ends, and Select All leaves one selection.
    pub fn key(&mut self, k: EditKey, shift: bool, lines: &[VisualLine], width: &dyn Fn(&str) -> f32) -> EditOut {
        if !self.extra.is_empty()
            && let Some(out) = self.key_multi(&k, shift, lines, width) {
                return out;
            }
        self.key_one(k, shift, lines, width)
    }

    /// `key` for the primary caret alone.
    fn key_one(&mut self, k: EditKey, shift: bool, lines: &[VisualLine], width: &dyn Fn(&str) -> f32) -> EditOut {
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
                self.extra.clear();
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
                    // Each part's place in the text after the step, undone from the last back.
                    let mut shift = 0isize;
                    let now: Vec<usize> = step
                        .parts
                        .iter()
                        .map(|p| {
                            let at = (p.at as isize + shift) as usize;
                            shift += p.inserted.len() as isize - p.removed.len() as isize;
                            at
                        })
                        .collect();
                    for (p, &at) in step.parts.iter().zip(&now).rev() {
                        self.text.replace_range(at..at + p.inserted.len(), &p.removed);
                    }
                    self.restore(&step.before);
                    self.redo.push(step);
                    EditOut::Changed
                }
                None => EditOut::Kept,
            },
            EditKey::Redo => match self.redo.pop() {
                Some(step) => {
                    for p in step.parts.iter().rev() {
                        self.text.replace_range(p.at..p.at + p.removed.len(), &p.inserted);
                    }
                    self.restore(&step.after);
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
        self.extra.clear();
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
        self.edit_all(vec![(r, s.to_string())], 0)
    }

    /// Replaces each range with its string as one undo step. The ranges are in text order and
    /// do not overlap (two may start at one index; the earlier's string goes first). Every
    /// part leaves a plain caret after its string; part `primary` is the primary. Nothing to
    /// replace changes nothing.
    fn edit_all(&mut self, parts: Vec<(Range<usize>, String)>, primary: usize) -> EditOut {
        if parts.iter().all(|(r, s)| r.is_empty() && s.is_empty()) {
            return EditOut::Kept;
        }
        let before = self.snapshot();
        let mut done = Vec::new();
        let mut carets = Vec::new();
        let mut shift = 0isize;
        for (i, (r, s)) in parts.iter().enumerate() {
            let at = (r.start as isize + shift) as usize + s.len();
            carets.push((Caret { at, anchor: None }, i == primary));
            shift += s.len() as isize - r.len() as isize;
            if !(r.is_empty() && s.is_empty()) {
                done.push(Part { at: r.start, removed: self.text[r.clone()].to_string(), inserted: s.clone() });
            }
        }
        // From the last back, so the earlier indexes stay valid.
        for (r, s) in parts.iter().rev() {
            self.text.replace_range(r.clone(), s);
        }
        self.set_all(carets);
        self.at_line_end = false;
        let after = self.snapshot();
        self.undo.push(Step { parts: done, before, after });
        self.redo.clear();
        EditOut::Changed
    }

    fn snapshot(&self) -> Carets {
        Carets { caret: self.caret, anchor: self.anchor, extra: self.extra.clone() }
    }

    fn restore(&mut self, c: &Carets) {
        self.caret = c.caret;
        self.anchor = c.anchor;
        self.extra = c.extra.clone();
        self.at_line_end = false;
    }

    /// Every caret with whether it is the primary, in text order.
    fn all(&self) -> Vec<(Caret, bool)> {
        let mut v: Vec<(Caret, bool)> = self.extra.iter().map(|&c| (c, false)).collect();
        v.push((Caret { at: self.caret, anchor: self.anchor }, true));
        v.sort_by_key(|(c, _)| {
            let r = c.range();
            (r.start, r.end)
        });
        v
    }

    /// Sets every caret from `v` (any order): carets that overlap or start at one index merge
    /// into one spanning both (keeping the primary's direction, else the longer one's); the
    /// one marked primary is the primary, or the last in text order when none is. An empty
    /// `v` leaves only the primary.
    fn set_all(&mut self, mut v: Vec<(Caret, bool)>) {
        if v.is_empty() {
            self.extra.clear();
            return;
        }
        v.sort_by_key(|(c, _)| {
            let r = c.range();
            (r.start, r.end)
        });
        let mut out: Vec<(Caret, bool)> = Vec::new();
        for (c, p) in v {
            if let Some((l, lp)) = out.last_mut() {
                let (lr, r) = (l.range(), c.range());
                if r.start < lr.end || r.start == lr.start {
                    let keep = if *lp {
                        *l
                    } else if p || r.len() > lr.len() {
                        c
                    } else {
                        *l
                    };
                    let u = lr.start.min(r.start)..lr.end.max(r.end);
                    *l = if u.is_empty() {
                        keep
                    } else if keep.anchor.is_some_and(|a| a > keep.at) {
                        Caret { at: u.start, anchor: Some(u.end) }
                    } else {
                        Caret { at: u.end, anchor: Some(u.start) }
                    };
                    *lp |= p;
                    continue;
                }
            }
            out.push((c, p));
        }
        let i = out.iter().rposition(|(_, p)| *p).unwrap_or(out.len() - 1);
        let (primary, _) = out.remove(i);
        self.caret = primary.at;
        self.anchor = primary.anchor;
        self.extra = out.into_iter().map(|(c, _)| c).collect();
    }

    /// `key` with other carets; `None` for the keys that act as with one caret (Undo, Redo,
    /// Select All).
    fn key_multi(&mut self, k: &EditKey, shift: bool, lines: &[VisualLine], width: &dyn Fn(&str) -> f32) -> Option<EditOut> {
        self.goal_x = None;
        // The piece a delete takes at caret `c`: its selection, else from it to `to`.
        fn del(c: &Caret, to: usize) -> (Range<usize>, String) {
            let r = c.range();
            (if r.is_empty() { c.at.min(to)..c.at.max(to) } else { r }, String::new())
        }
        Some(match k {
            EditKey::Text(s) | EditKey::Paste(s) if s.is_empty() => EditOut::Kept,
            EditKey::Text(s) | EditKey::Paste(s) => {
                let s = plain_line_ends(s);
                self.edit_each(|_, c| (c.range(), s.clone()))
            }
            EditKey::Enter => self.edit_each(|_, c| (c.range(), "\n".to_string())),
            // `MCTV:1655-1657` `insertTab`: `tabStop` (4, `MCTV:88`) spaces at each caret.
            EditKey::Tab => self.edit_each(|_, c| (c.range(), " ".repeat(TAB_STOP))),
            // `MCTV:942-963`: each caret one character left or right of its range's start,
            // selected or not (the one-caret Windows collapse does not apply).
            EditKey::Left if !shift => self.move_each(|t, c| prev_boundary(t, c.range().start)),
            EditKey::Right if !shift => self.move_each(|t, c| next_boundary(t, c.range().start)),
            EditKey::Backspace => self.edit_each(|t, c| del(c, prev_boundary(t, c.at))),
            EditKey::Delete => self.edit_each(|t, c| del(c, next_boundary(t, c.at))),
            EditKey::DeleteWordBack => self.edit_each(|t, c| del(c, word_left(t, c.at))),
            EditKey::DeleteWordForward => self.edit_each(|t, c| del(c, word_right(t, c.at))),
            // `MCTV:1901-1931`: every caret's piece, even an empty one.
            EditKey::Copy => EditOut::Copied(self.pieces().join("\n")),
            EditKey::Cut => {
                let cut = self.pieces().join("\n");
                self.edit_each(|_, c| (c.range(), String::new()));
                EditOut::Copied(cut)
            }
            EditKey::Up | EditKey::Down => self.vertical_each(matches!(k, EditKey::Down), shift, lines, width),
            EditKey::Undo | EditKey::Redo | EditKey::SelectAll => return None,
            _ => {
                // Every other move: the one-caret move at each caret.
                let end = self.at_line_end;
                let mut moved = Vec::new();
                let mut primary_end = false;
                for (c, p) in self.all() {
                    self.caret = c.at;
                    self.anchor = c.anchor;
                    self.at_line_end = p && end;
                    self.goal_x = None;
                    self.key_one(k.clone(), shift, lines, width);
                    if p {
                        primary_end = self.at_line_end;
                    }
                    moved.push((Caret { at: self.caret, anchor: self.anchor }, p));
                }
                self.set_all(moved);
                self.at_line_end = primary_end;
                self.goal_x = None;
                EditOut::Moved
            }
        })
    }

    /// Each caret's piece of text (empty for a plain caret), in text order.
    fn pieces(&self) -> Vec<String> {
        self.all().iter().map(|(c, _)| self.text[c.range()].to_string()).collect()
    }

    /// One edit at every caret, from `f` (the text and the caret give the range to replace and
    /// its replacement), as one undo step. Delete ranges that overlap merge (`MCTV`
    /// `deleteRanges` clips them), and so do their carets.
    fn edit_each(&mut self, f: impl Fn(&str, &Caret) -> (Range<usize>, String)) -> EditOut {
        let mut parts: Vec<(Range<usize>, String, bool)> = Vec::new();
        for (c, p) in self.all() {
            let (mut r, s) = f(&self.text, &c);
            let mut p = p;
            while let Some(last) = parts.last() {
                if r.start >= last.0.end {
                    break;
                }
                let (lr, _, lp) = parts.pop().expect("last");
                r = lr.start.min(r.start)..lr.end.max(r.end);
                p |= lp;
            }
            parts.push((r, s, p));
        }
        let primary = parts.iter().position(|(_, _, p)| *p).unwrap_or(parts.len() - 1);
        self.edit_all(parts.into_iter().map(|(r, s, _)| (r, s)).collect(), primary)
    }

    /// Every caret to the plain caret `f` gives for it (`MCTV:942-963`).
    fn move_each(&mut self, f: impl Fn(&str, &Caret) -> usize) -> EditOut {
        let moved = self.all().iter().map(|(c, p)| (Caret { at: f(&self.text, c), anchor: None }, *p)).collect();
        self.set_all(moved);
        self.at_line_end = false;
        EditOut::Moved
    }

    /// Up or Down with other carets (`MCTV:1123-1131,1192-1216,1256-1296`): each caret to the
    /// visual line above or below at its own x (no goal x), Shift extending its selection; a
    /// caret with no line there is dropped. When every caret is dropped the primary stays,
    /// alone. The primary keeps its affinity as the one-caret move does.
    fn vertical_each(&mut self, down: bool, shift: bool, lines: &[VisualLine], width: &dyn Fn(&str) -> f32) -> EditOut {
        let mut moved = Vec::new();
        let mut primary_end = None;
        for (c, p) in self.all() {
            let (line, x) = caret_pos(&self.text, lines, c.at, p && self.at_line_end, width);
            let target = if down { Some(line + 1).filter(|&t| t < lines.len()) } else { line.checked_sub(1) };
            let Some(target) = target else { continue };
            let at = index_at(&self.text, lines, target, x, width);
            let anchor = if shift { Some(c.anchor.unwrap_or(c.at)) } else { None };
            if p {
                primary_end = Some((at, ends_line(lines, target, at)));
            }
            moved.push((Caret { at, anchor }, p));
        }
        if moved.is_empty() {
            self.extra.clear();
            return EditOut::Moved;
        }
        self.set_all(moved);
        // A merge can move the primary off its target; then it has no affinity.
        self.at_line_end = matches!(primary_end, Some((at, end)) if end && at == self.caret);
        EditOut::Moved
    }

    /// Every caret or selection in text order, an empty range for a plain caret.
    pub fn carets(&self) -> Vec<Range<usize>> {
        self.all().iter().map(|(c, _)| c.range()).collect()
    }

    /// Whether there is more than one caret.
    pub fn has_extra(&self) -> bool {
        !self.extra.is_empty()
    }

    /// Every caret's range for a mutating `k` (`key_one`'s and `key_multi`'s own choice of
    /// what to replace): its selection when it has one, else where `k` reaches from it, empty
    /// at the caret for an insert (`Text`, `Enter`, `Tab`, `Paste`), `prev_boundary`/
    /// `next_boundary`/`word_left`/`word_right` for the delete keys. Lets a caller (the
    /// composer's prefix guard) check a key against a protected byte range before performing
    /// it, without duplicating the boundary walk.
    pub fn reach(&self, k: &EditKey) -> Vec<Range<usize>> {
        self.all()
            .iter()
            .map(|(c, _)| {
                let sel = c.range();
                if !sel.is_empty() {
                    return sel;
                }
                match k {
                    EditKey::Backspace => prev_boundary(&self.text, c.at)..c.at,
                    EditKey::Delete => c.at..next_boundary(&self.text, c.at),
                    EditKey::DeleteWordBack => word_left(&self.text, c.at)..c.at,
                    EditKey::DeleteWordForward => c.at..word_right(&self.text, c.at),
                    _ => c.at..c.at,
                }
            })
            .collect()
    }

    /// Drops all undo and redo history: for a caller (the composer's `take`) that removes
    /// several ranges directly rather than through one caret's edit, so there is nothing
    /// coherent left to undo.
    pub fn clear_undo(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    /// Ctrl+Shift+Down (`below`) or Ctrl+Shift+Up (`MCTV:1163-1183`): a caret on the visual line
    /// below or above the primary's start, at its x, which becomes the primary. False (and
    /// nothing added) when there is no such line.
    pub fn add_caret(&mut self, below: bool, lines: &[VisualLine], width: &dyn Fn(&str) -> f32) -> bool {
        let start = self.anchor.map_or(self.caret, |a| a.min(self.caret));
        let (line, x) = caret_pos(&self.text, lines, start, self.at_line_end && start == self.caret, width);
        let target = if below { Some(line + 1).filter(|&t| t < lines.len()) } else { line.checked_sub(1) };
        let Some(target) = target else { return false };
        let at = index_at(&self.text, lines, target, x, width);
        let mut all: Vec<(Caret, bool)> = self.all().into_iter().map(|(c, _)| (c, false)).collect();
        all.push((Caret { at, anchor: None }, true));
        self.set_all(all);
        self.at_line_end = self.caret == at && ends_line(lines, target, at);
        self.goal_x = None;
        true
    }

    /// Ctrl+Shift+click at byte index `at` (`MCTV:352-368`): adds a plain caret there, the
    /// others staying; the last caret in text order is then the primary (`MCTV` sorts them).
    pub fn click_add(&mut self, at: usize) {
        let at = floor_boundary(&self.text, at);
        let mut all: Vec<(Caret, bool)> = self.all().into_iter().map(|(c, _)| (c, false)).collect();
        all.push((Caret { at, anchor: None }, false));
        self.set_all(all);
        self.at_line_end = false;
        self.goal_x = None;
        self.last_click = None;
    }

    /// Alt+drag from `from` to `to`, each a (visual line, x) (`MCTV:316-335`): replaces every
    /// caret with one range per visual line between the two, from `index_at` of the start x to
    /// `index_at` of the end x (a caret where they meet); the last line's is the primary.
    pub fn box_select(&mut self, from: (usize, f32), to: (usize, f32), lines: &[VisualLine], text_width: &dyn Fn(&str) -> f32) {
        let Some(last) = lines.len().checked_sub(1) else { return };
        let (top, bottom) = (from.0.min(to.0).min(last), from.0.max(to.0).min(last));
        let v = (top..=bottom)
            .map(|line| {
                let a = index_at(&self.text, lines, line, from.1, text_width);
                let b = index_at(&self.text, lines, line, to.1, text_width);
                (Caret { at: b, anchor: (a != b).then_some(a) }, false)
            })
            .collect();
        self.set_all(v);
        self.at_line_end = false;
        self.goal_x = None;
        self.last_click = None;
    }

    /// Only the primary caret stays (Esc, `MCTV:1933-1940` keeps the last).
    pub fn collapse_extra(&mut self) {
        self.extra.clear();
    }

    /// Sets every caret (any order, clamped to the text); the last given is the primary. An
    /// empty slice changes nothing.
    pub fn set_carets(&mut self, carets: &[Caret]) {
        if carets.is_empty() {
            return;
        }
        let n = carets.len();
        let v = carets
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let at = floor_boundary(&self.text, c.at);
                let anchor = c.anchor.map(|a| floor_boundary(&self.text, a));
                (Caret { at, anchor }, i + 1 == n)
            })
            .collect();
        self.set_all(v);
        self.at_line_end = false;
        self.goal_x = None;
    }

    /// A multi-caret copy (`MCTV:1901-1919`): every caret's piece joined with "\n", and each
    /// piece's line count, for `paste_groups`. `None` with one caret.
    pub fn copy_groups(&self) -> Option<(String, Vec<usize>)> {
        if self.extra.is_empty() {
            return None;
        }
        let pieces = self.pieces();
        let counts = pieces.iter().map(|p| p.split('\n').count()).collect();
        Some((pieces.join("\n"), counts))
    }

    /// Pastes a multi-caret copy (`MCTV:1832-1899`) as one undo step: `text`'s lines, taken
    /// `groups[i]` at a time, replace caret `i`'s selection in text order; carets past the
    /// groups keep their text and collapse to their ends; groups past the carets go in at the
    /// visual line below the last caret's start at its x, each further one on the line below
    /// that at the x it landed at, and on a line appended to the text when no line is left.
    /// Every caret lands after its group; with groups added the last is the primary. Groups
    /// that do not add up to `text`'s lines paste `text` at each caret as plain text.
    pub fn paste_groups(&mut self, text: &str, groups: &[usize], lines: &[VisualLine], width: &dyn Fn(&str) -> f32) -> EditOut {
        let text = plain_line_ends(text);
        let all_lines: Vec<&str> = text.split('\n').collect();
        if groups.is_empty() || groups.contains(&0) || groups.iter().sum::<usize>() != all_lines.len() {
            return self.key(EditKey::Paste(text), false, lines, width);
        }
        let mut values = Vec::new();
        let mut i = 0;
        for &n in groups {
            values.push(all_lines[i..i + n].join("\n"));
            i += n;
        }
        self.goal_x = None;
        let all = self.all();
        let mut primary = all.iter().position(|(_, p)| *p).unwrap_or(0);
        let mut parts: Vec<(Range<usize>, String)> = all
            .iter()
            .enumerate()
            .map(|(i, (c, _))| match values.get(i) {
                Some(v) => (c.range(), v.clone()),
                None => (c.range().end..c.range().end, String::new()),
            })
            .collect();
        if values.len() > all.len() {
            let last = all[all.len() - 1].0.range();
            let (mut line, mut x) = caret_pos(&self.text, lines, last.start, false, width);
            let mut floor = last.end;
            for v in &values[all.len()..] {
                let mut placed = None;
                while line + 1 < lines.len() {
                    line += 1;
                    let at = index_at(&self.text, lines, line, x, width);
                    if at >= floor {
                        placed = Some(at);
                        break;
                    }
                }
                match placed {
                    Some(at) => {
                        x = width(&self.text[lines[line].start..at]);
                        floor = at;
                        parts.push((at..at, v.clone()));
                    }
                    None => {
                        let end = self.text.len();
                        parts.push((end..end, format!("\n{v}")));
                    }
                }
            }
            primary = parts.len() - 1;
        }
        self.edit_all(parts, primary)
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
    wrap_indented(text, max_w, 0.0, false, width)
}

/// `wrap` with the first visual line starting `first_indent` in (the composer's prompt
/// prefix), so it has `max_w - first_indent` of room; every later line has `max_w`. The
/// caller adds the indent to line 0's x when it draws or hit-tests (`caret_pos` and
/// `index_at` measure from each line's start).
///
/// iTerm2 keeps the prompt prefix as text in the same storage (`CTV:475-505`), so the prefix
/// and the first word wrap together: when the prefix ends in a break opportunity (whitespace,
/// `prefix_breaks`) and no word of the first line fits the indented room, the first visual
/// line is empty (`start == end == 0`, the prefix alone) and the text starts on line 1; without
/// one, the first word breaks mid-word at the room, as a too-wide word does.
pub fn wrap_indented(text: &str, max_w: f32, first_indent: f32, prefix_breaks: bool, width: &dyn Fn(&str) -> f32) -> Vec<VisualLine> {
    let mut lines = Vec::new();
    let mut hard = 0;
    for (i, part) in text.split('\n').enumerate() {
        let indent = if i == 0 { first_indent } else { 0.0 };
        wrap_hard(text, hard, hard + part.len(), max_w, indent, prefix_breaks && i == 0, width, &mut lines);
        hard += part.len() + 1;
    }
    lines
}

/// `wrap` remembered between calls (the Notes tool wraps on every frame, key and drag move):
/// the lines are reused while the text, the width and the font (`font`, a key the caller
/// derives from the measure it passes) are unchanged, else wrapped again.
#[derive(Clone, Debug, Default)]
pub struct WrapCache {
    key: Option<(String, u32, u64)>,
    lines: Vec<VisualLine>,
}

impl WrapCache {
    /// `wrap(text, max_w, width)`, from the last call when nothing it depends on changed.
    pub fn wrap(&mut self, text: &str, max_w: f32, font: u64, width: &dyn Fn(&str) -> f32) -> Vec<VisualLine> {
        let hit = self.key.as_ref().is_some_and(|(t, w, f)| *w == max_w.to_bits() && *f == font && t == text);
        if !hit {
            self.lines = wrap(text, max_w, width);
            self.key = Some((text.to_string(), max_w.to_bits(), font));
        }
        self.lines.clone()
    }
}

/// One hard line `text[start..end]` into `lines`, its first visual line `indent` in; with
/// `break_before`, that line may be empty when no word fits it (`wrap_indented`).
///
/// Each visual line measures only text near its own length (`fit_end`), never the rest of the
/// paragraph, so a long pasted line wraps in O(n log n) measuring rather than O(n^2). This
/// relies on the width growing with the text (a longer prefix is never narrower).
fn wrap_hard(text: &str, start: usize, end: usize, max_w: f32, indent: f32, break_before: bool, width: &dyn Fn(&str) -> f32, lines: &mut Vec<VisualLine>) {
    let bytes = text.as_bytes();
    let mut line = start;
    let mut room = max_w - indent;
    loop {
        // A line's trailing spaces hang past the edge and do not count.
        let trimmed = line + text[line..end].trim_end_matches(' ').len();
        let reach = if trimmed == line { line } else { fit_end(text, line, end, room, width) };
        let fits = if trimmed == line { width("") <= room } else { reach >= trimmed };
        if fits {
            lines.push(VisualLine { start: line, end });
            return;
        }
        // The last space-ended word boundary whose text (less its spaces) still fits: the last
        // run of spaces starting at or before `reach`. Leading spaces belong to the first word.
        let lead = line + text[line..end].len() - text[line..end].trim_start_matches(' ').len();
        let mut cut = None;
        if lead < reach {
            let upto = if bytes.get(reach) == Some(&b' ') { reach + 1 } else { reach };
            if let Some(off) = text[lead..upto].rfind(' ') {
                let space = lead + off;
                let after = space + text[space..end].len() - text[space..end].trim_start_matches(' ').len();
                cut = Some(after);
            }
        }
        if cut.is_none() && break_before && line == start && room < max_w && line < end {
            // The prefix's own break: the first word moves down whole.
            lines.push(VisualLine { start: line, end: line });
            room = max_w;
            continue;
        }
        // One word too wide: the longest char prefix that fits, at least one character.
        let cut = cut.unwrap_or_else(|| reach.max(next_boundary(text, line)));
        lines.push(VisualLine { start: line, end: cut });
        if cut >= end {
            // A last character too wide for the line on its own.
            return;
        }
        line = cut;
        room = max_w;
    }
}

/// The furthest char boundary `b` in `line..=end` with `text[line..b]` no wider than `room`
/// (`line` itself when not even one character fits). Gallops out from `line` in doubling steps
/// and then halves back, so it makes O(log k) measures of at most about 3k bytes, k the bytes
/// that fit, however far `end` is.
fn fit_end(text: &str, line: usize, end: usize, room: f32, width: &dyn Fn(&str) -> f32) -> usize {
    let probe = |lo: usize, to: usize| {
        let p = floor_boundary(text, to.min(end));
        if p <= lo { next_boundary(text, lo) } else { p }
    };
    let mut lo = line;
    let mut step = 16;
    let mut hi = loop {
        if lo >= end {
            return end;
        }
        let p = probe(lo, lo + step);
        if width(&text[line..p]) > room {
            break p;
        }
        lo = p;
        step *= 2;
    };
    // `lo` fits and `hi` does not.
    while next_boundary(text, lo) < hi {
        let mid = probe(lo, lo + (hi - lo) / 2);
        if width(&text[line..mid]) > room {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    lo
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

/// The pieces of a selection to paint: (visual line index, x offset of its start, width) for
/// each of `show` (indexes into `lines`) the selection touches. A selected line end shows as a
/// space's width past the text. Shared by the Notes tool and the inline chat's input.
pub fn selection_spans(sel: Option<Range<usize>>, text: &str, lines: &[VisualLine], show: Range<usize>, measure: &dyn Fn(&str) -> f32) -> Vec<(usize, f32, f32)> {
    let mut out = Vec::new();
    let Some(r) = sel else { return out };
    let space = measure(" ");
    for i in show.start..show.end.min(lines.len()) {
        let v = &lines[i];
        if r.end < v.start || r.start > v.end || (r.end == v.start && v.start != v.end) {
            continue;
        }
        let (a, b) = (r.start.max(v.start), r.end.min(v.end));
        let x0 = measure(&text[v.start..a]);
        let x1 = measure(&text[v.start..b]) + if r.end > v.end && lines.get(i + 1).is_some_and(|n| n.start > v.end) { space } else { 0.0 };
        if x1 > x0 {
            out.push((i, x0, x1 - x0));
        }
    }
    out
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

    #[test]
    fn selection_spans_follow_the_notes_rule() {
        // "ab cd" wrapped as "ab " / "cd" (one char = 1 unit); the selection 1..4 spans both lines.
        let lines = vec![VisualLine { start: 0, end: 3 }, VisualLine { start: 3, end: 5 }];
        let measure = |t: &str| t.chars().count() as f32;
        let spans = selection_spans(Some(1..4), "ab cd", &lines, 0..2, &measure);
        assert_eq!(spans, vec![(0, 1.0, 2.0), (1, 0.0, 1.0)]);
        assert!(selection_spans(None, "ab cd", &lines, 0..2, &measure).is_empty());
        // Only the shown lines are painted.
        assert_eq!(selection_spans(Some(1..4), "ab cd", &lines, 1..2, &measure), vec![(1, 0.0, 1.0)]);
    }

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
    fn reach_finds_what_a_delete_key_would_touch() {
        let mut e = TextEdit::default();
        e.set_text("ab cd");
        e.caret = 3;
        assert_eq!(e.reach(&EditKey::Backspace), vec![2..3]);
        assert_eq!(e.reach(&EditKey::Delete), vec![3..4]);
        assert_eq!(e.reach(&EditKey::DeleteWordBack), vec![0..3]);
        assert_eq!(e.reach(&EditKey::Text("x".to_string())), vec![3..3]);
        e.anchor = Some(1);
        assert_eq!(e.reach(&EditKey::Backspace), vec![1..3]);
        assert_eq!(e.reach(&EditKey::Text("x".to_string())), vec![1..3]);

        e.anchor = None;
        e.caret = 5;
        e.set_carets(&[Caret { at: 5, anchor: None }, Caret { at: 2, anchor: None }]);
        assert_eq!(e.reach(&EditKey::Backspace), vec![1..2, 4..5]);
    }

    #[test]
    fn clear_undo_drops_history() {
        let mut e = TextEdit::default();
        e.set_text("a");
        press(&mut e, EditKey::Text("b".to_string()), false, 1000.0);
        e.clear_undo();
        assert_eq!(press(&mut e, EditKey::Undo, false, 1000.0), EditOut::Kept);
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

    #[test]
    fn carets_follow_iterm2() {
        // Ctrl+Shift+Down twice from the caret after "a": one caret per line at its x
        // (`MCTV:1163-1183`); none past the last line.
        let mut e = TextEdit::default();
        e.load("ab\ncd\nef");
        e.click(1, false, false, 0, 500);
        let lines = wrap(&e.text, 1000.0, &w);
        assert!(!e.has_extra());
        assert!(e.add_caret(true, &lines, &w));
        assert!(e.add_caret(true, &lines, &w));
        assert_eq!(e.carets(), vec![1..1, 4..4, 7..7]);
        assert!(e.has_extra());
        assert_eq!(e.extra, vec![Caret { at: 1, anchor: None }, Caret { at: 4, anchor: None }]);
        // The primary is the last added.
        assert_eq!((e.caret, e.anchor), (7, None));
        assert!(!e.add_caret(true, &lines, &w));
        assert_eq!(e.carets().len(), 3);

        // Typing, Backspace, Left and Shift+Right act at every caret.
        assert_eq!(press(&mut e, EditKey::Text("X".into()), false, 1000.0), EditOut::Changed);
        assert_eq!(e.text, "aXb\ncXd\neXf");
        assert_eq!(e.carets(), vec![2..2, 6..6, 10..10]);
        assert_eq!(press(&mut e, EditKey::Backspace, false, 1000.0), EditOut::Changed);
        assert_eq!(e.text, "ab\ncd\nef");
        assert_eq!(e.carets(), vec![1..1, 4..4, 7..7]);
        assert_eq!(press(&mut e, EditKey::Left, false, 1000.0), EditOut::Moved);
        assert_eq!(e.carets(), vec![0..0, 3..3, 6..6]);
        press(&mut e, EditKey::Right, true, 1000.0);
        assert_eq!(e.carets(), vec![0..1, 3..4, 6..7]);
        assert_eq!(press(&mut e, EditKey::Copy, false, 1000.0), EditOut::Copied("a\nc\ne".into()));
        // A plain Left or Right moves each caret one character from its range's start,
        // selected or not (`MCTV:942-963`).
        press(&mut e, EditKey::Right, false, 1000.0);
        assert_eq!(e.carets(), vec![1..1, 4..4, 7..7]);
        e.set_carets(&[Caret { at: 0, anchor: Some(2) }, Caret { at: 5, anchor: Some(3) }]);
        press(&mut e, EditKey::Right, false, 1000.0);
        assert_eq!(e.carets(), vec![1..1, 4..4]);
        e.set_carets(&[Caret { at: 0, anchor: Some(1) }, Caret { at: 5, anchor: Some(3) }]);
        press(&mut e, EditKey::Left, false, 1000.0);
        assert_eq!(e.carets(), vec![0..0, 2..2]);
        e.set_carets(&[Caret { at: 1, anchor: None }, Caret { at: 4, anchor: None }, Caret { at: 7, anchor: None }]);
        // Carets meeting merge: Backspace twice from 1 and 2 leaves one caret at 0.
        e.set_carets(&[Caret { at: 1, anchor: None }, Caret { at: 2, anchor: None }]);
        press(&mut e, EditKey::Backspace, false, 1000.0);
        assert_eq!((e.text.as_str(), e.carets()), ("\ncd\nef", vec![0..0]));
        assert!(!e.has_extra());
        press(&mut e, EditKey::Undo, false, 1000.0);
        assert_eq!((e.text.as_str(), e.carets()), ("ab\ncd\nef", vec![1..1, 2..2]));

        // A plain Up or Down with extras moves each by a visual line at its own x; a caret with
        // no line to go to is dropped (`MCTV:1123-1131,1192-1216`, `modifyRanges` compactMap).
        let lines = wrap(&e.text, 1000.0, &w);
        e.set_carets(&[Caret { at: 1, anchor: None }, Caret { at: 4, anchor: None }]);
        assert_eq!(press(&mut e, EditKey::Down, false, 1000.0), EditOut::Moved);
        assert_eq!(e.carets(), vec![4..4, 7..7]);
        press(&mut e, EditKey::Up, false, 1000.0);
        assert_eq!(e.carets(), vec![1..1, 4..4]);
        press(&mut e, EditKey::Down, false, 1000.0);
        press(&mut e, EditKey::Down, false, 1000.0);
        assert_eq!((e.carets(), e.has_extra()), (vec![7..7], false));
        // The primary keeps its affinity on Up and Down, as with one caret: past a soft-wrapped
        // line's end it stays drawn on that line.
        let text = "abcdefghij\nxy";
        e.load(text);
        let wl = wrap(text, 40.0, &w);
        assert_eq!(wl, vec![vl(0, 4), vl(4, 8), vl(8, 10), vl(11, 13)]);
        e.set_carets(&[Caret { at: 12, anchor: None }, Caret { at: 4, anchor: None }]);
        e.at_line_end = true;
        assert_eq!(e.caret_line(&wl, &w), (0, 40.0));
        press(&mut e, EditKey::Down, false, 40.0);
        assert_eq!((e.caret, e.at_line_end), (8, true));
        assert_eq!(e.caret_line(&wl, &w), (1, 40.0));
        assert_eq!(e.carets(), vec![8..8]);
        e.load("ab\ncd\nef");

        // Ctrl+Shift+Up on the first line adds nothing; a plain click collapses to one caret.
        e.click(1, false, false, 1_000, 500);
        assert!(!e.add_caret(false, &lines, &w));
        assert!(e.add_caret(true, &lines, &w));
        assert!(e.add_caret(false, &lines, &w));
        assert_eq!(e.carets(), vec![1..1, 4..4]);
        assert_eq!(e.caret, 1);
        e.click(5, false, false, 2_000, 500);
        assert_eq!((e.carets(), e.has_extra()), (vec![5..5], false));

        // Ctrl+Shift+click adds a caret without moving the others; the last in text order
        // is the primary (`MCTV:352-368` sorts).
        e.click(1, false, false, 3_000, 500);
        e.click_add(7);
        assert_eq!(e.carets(), vec![1..1, 7..7]);
        e.click_add(4);
        assert_eq!(e.carets(), vec![1..1, 4..4, 7..7]);
        assert_eq!(e.caret, 7);
        e.click_add(4);
        assert_eq!(e.carets().len(), 3);

        // Enter and Tab insert at each caret; one Undo takes back a whole multi-caret edit.
        press(&mut e, EditKey::Enter, false, 1000.0);
        assert_eq!(e.text, "a\nb\nc\nd\ne\nf");
        assert_eq!(e.carets(), vec![2..2, 6..6, 10..10]);
        // Tab puts `tabStop` (4) spaces at each caret (`MCTV:1655-1657`).
        press(&mut e, EditKey::Tab, false, 1000.0);
        assert_eq!(e.text, "a\n    b\nc\n    d\ne\n    f");
        assert_eq!(e.carets(), vec![6..6, 14..14, 22..22]);
        assert_eq!(press(&mut e, EditKey::Undo, false, 1000.0), EditOut::Changed);
        assert_eq!((e.text.as_str(), e.carets()), ("a\nb\nc\nd\ne\nf", vec![2..2, 6..6, 10..10]));
        press(&mut e, EditKey::Undo, false, 1000.0);
        assert_eq!((e.text.as_str(), e.carets()), ("ab\ncd\nef", vec![1..1, 4..4, 7..7]));
        assert_eq!(press(&mut e, EditKey::Redo, false, 1000.0), EditOut::Changed);
        assert_eq!((e.text.as_str(), e.carets()), ("a\nb\nc\nd\ne\nf", vec![2..2, 6..6, 10..10]));

        // Cut takes every selection, joined by line ends; Esc's collapse keeps the primary.
        e.load("ab\ncd");
        e.set_carets(&[Caret { at: 1, anchor: Some(0) }, Caret { at: 4, anchor: Some(3) }]);
        assert_eq!(press(&mut e, EditKey::Cut, false, 1000.0), EditOut::Copied("a\nc".into()));
        assert_eq!((e.text.as_str(), e.carets()), ("b\nd", vec![0..0, 2..2]));
        e.collapse_extra();
        assert_eq!((e.carets(), e.caret), (vec![2..2], 2));
        // Select All leaves one selection.
        e.set_carets(&[Caret { at: 0, anchor: None }, Caret { at: 2, anchor: None }]);
        press(&mut e, EditKey::SelectAll, false, 1000.0);
        assert_eq!((e.carets(), e.has_extra()), (vec![0..3], false));
    }

    #[test]
    fn box_select_and_groups_follow_iterm2() {
        // Alt+drag from line 0 at one cell to line 2 at two cells: one range per visual line
        // between the two x (`MCTV:316-335`, `split`).
        let mut e = TextEdit::default();
        e.load("ab\ncd\nef");
        let lines = wrap(&e.text, 1000.0, &w);
        e.box_select((0, 10.0), (2, 20.0), &lines, &w);
        assert_eq!(e.carets(), vec![1..2, 4..5, 7..8]);
        // A multi-caret copy: the pieces joined with "\n" and each piece's line count
        // (`MCTV:1901-1919`); one caret has no groups.
        assert_eq!(e.copy_groups(), Some(("b\nd\nf".to_string(), vec![1, 1, 1])));
        assert_eq!(press(&mut e, EditKey::Copy, false, 1000.0), EditOut::Copied("b\nd\nf".into()));
        // Pasting them back replaces each selection in order, one undo step.
        assert_eq!(e.paste_groups("1\n2\n3", &[1, 1, 1], &lines, &w), EditOut::Changed);
        assert_eq!((e.text.as_str(), e.carets()), ("a1\nc2\ne3", vec![2..2, 5..5, 8..8]));
        press(&mut e, EditKey::Undo, false, 1000.0);
        assert_eq!((e.text.as_str(), e.carets()), ("ab\ncd\nef", vec![1..2, 4..5, 7..8]));
        e.click(0, false, false, 0, 500);
        assert_eq!(e.copy_groups(), None);
        // Dragging from right to left and bottom to top selects the same box.
        e.box_select((2, 20.0), (0, 10.0), &lines, &w);
        assert_eq!(e.carets(), vec![1..2, 4..5, 7..8]);
        // A zero-width box is a column of carets.
        e.box_select((0, 10.0), (1, 10.0), &lines, &w);
        assert_eq!(e.carets(), vec![1..1, 4..4]);

        // More groups than carets: the rest go on the lines below the last caret at its x,
        // then on lines appended at the end (`MCTV:1866-1896`).
        e.load("ab\ncd\nef");
        e.box_select((0, 10.0), (1, 20.0), &lines, &w);
        e.paste_groups("1\n2\n3\n4", &[1, 1, 1, 1], &lines, &w);
        assert_eq!(e.text, "a1\nc2\ne3f\n4");
        assert_eq!(e.carets(), vec![2..2, 5..5, 8..8, 11..11]);
        assert_eq!(e.caret, 11);
        press(&mut e, EditKey::Undo, false, 1000.0);
        assert_eq!((e.text.as_str(), e.carets()), ("ab\ncd\nef", vec![1..2, 4..5]));
        // Fewer groups: the other carets keep their text and collapse to their ends.
        e.box_select((0, 10.0), (2, 20.0), &lines, &w);
        e.paste_groups("1\n2", &[1, 1], &lines, &w);
        assert_eq!((e.text.as_str(), e.carets()), ("a1\nc2\nef", vec![2..2, 5..5, 8..8]));
        // A group of two lines goes to one caret.
        e.load("ab\ncd");
        let lines = wrap(&e.text, 1000.0, &w);
        e.box_select((0, 10.0), (1, 20.0), &lines, &w);
        assert_eq!(e.copy_groups(), Some(("b\nd".to_string(), vec![1, 1])));
        e.paste_groups("x\ny\nz", &[2, 1], &lines, &w);
        assert_eq!(e.text, "ax\ny\ncz");
        e.set_carets(&[Caret { at: 0, anchor: Some(4) }, Caret { at: 6, anchor: Some(7) }]);
        assert_eq!(e.copy_groups(), Some(("ax\ny\nz".to_string(), vec![2, 1])));
        // Groups that do not add up to the text's lines paste it as plain text at each caret.
        e.load("ab\ncd");
        e.box_select((0, 10.0), (1, 10.0), &lines, &w);
        e.paste_groups("q", &[1, 1], &lines, &w);
        assert_eq!(e.text, "aqb\ncqd");
    }

    #[test]
    fn indented_wrap_follows_notes_rules() {
        // The first line starts two characters in, leaving room for two. A prefix ending in a
        // break opportunity wraps with the text (`CTV:475-505`): the first word does not fit,
        // so it moves down whole and the first line is the prefix alone.
        assert_eq!(wrap_indented("aaaa bbbb", 40.0, 20.0, true, &w), vec![vl(0, 0), vl(0, 5), vl(5, 9)]);
        // Without a break opportunity the first word breaks after two characters' room.
        assert_eq!(wrap_indented("aaaa bbbb", 40.0, 20.0, false, &w), vec![vl(0, 2), vl(2, 5), vl(5, 9)]);
        // A first word that fits stays on the first line either way.
        assert_eq!(wrap_indented("ab cd", 40.0, 10.0, true, &w), vec![vl(0, 3), vl(3, 5)]);
        assert_eq!(wrap_indented("ab cd", 40.0, 10.0, false, &w), vec![vl(0, 3), vl(3, 5)]);
        // Only the first line is indented, not the first line of each paragraph.
        assert_eq!(wrap_indented("aaaa\nbbbb", 40.0, 20.0, false, &w), vec![vl(0, 2), vl(2, 4), vl(5, 9)]);
        assert_eq!(wrap_indented("aaaa\nbbbb", 40.0, 20.0, true, &w), vec![vl(0, 0), vl(0, 4), vl(5, 9)]);
        // No room at all: one character, or the whole word moved down.
        assert_eq!(wrap_indented("ab", 40.0, 50.0, false, &w), vec![vl(0, 1), vl(1, 2)]);
        assert_eq!(wrap_indented("ab", 40.0, 50.0, true, &w), vec![vl(0, 0), vl(0, 2)]);
        // Empty text is one empty line.
        assert_eq!(wrap_indented("", 40.0, 50.0, true, &w), vec![vl(0, 0)]);
        // The caret at the start draws on the line the text starts on.
        let lines = wrap_indented("aaaa bbbb", 40.0, 20.0, true, &w);
        assert_eq!(caret_pos("aaaa bbbb", &lines, 0, false, &w), (1, 0.0));
        for t in ["aaaa bbbb", "abcdefghij", "", "a\n", "ab cd ef", "aaaa bbbb\nc\ndddd"] {
            assert_eq!(wrap_indented(t, 40.0, 0.0, false, &w), wrap(t, 40.0, &w), "{t:?}");
            assert_eq!(wrap_indented(t, 40.0, 0.0, true, &w), wrap(t, 40.0, &w), "{t:?}");
        }
    }

    #[test]
    fn wrapping_a_long_line_measures_in_n_log_n() {
        // A 100 KiB paste (one word, then words): each visual line measures only text near its
        // own length, so the calls and the bytes measured grow as n log n, not n squared.
        let calls = std::cell::Cell::new(0usize);
        let bytes = std::cell::Cell::new(0usize);
        let counted = |s: &str| {
            calls.set(calls.get() + 1);
            bytes.set(bytes.get() + s.len());
            w(s)
        };
        for text in ["x".repeat(100 * 1024), "abcdefg ".repeat(100 * 1024 / 8)] {
            calls.set(0);
            bytes.set(0);
            let lines = wrap(&text, 800.0, &counted);
            let n = text.len();
            assert!(lines.len() >= n / 80, "{} lines", lines.len());
            assert!(calls.get() <= 20 * lines.len(), "{} width calls for {} lines", calls.get(), lines.len());
            assert!(bytes.get() <= 40 * n, "{} bytes measured for {n}", bytes.get());
        }
    }

    #[test]
    fn wrap_matches_the_whole_paragraph_measure() {
        // The bounded measure breaks exactly where measuring the whole paragraph did.
        let texts = [
            "aaaa bbbb", "abcdefghij", "", "a\n", "ab cd ef", "aaaa bbbb\nc\ndddd", "  lead  spaces  here  ",
            "one two three four five six seven", "a  b   c    d     e", "\u{e9}\u{e9}\u{e9} \u{e9}\u{e9}\u{e9}\u{e9}\u{e9}",
            "word\n\n  \nxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx yy", "ab    ", "    ", "x y",
        ];
        for t in texts {
            for max_w in [5.0, 10.0, 25.0, 40.0, 60.0, 95.0, 400.0] {
                for (indent, breaks) in [(0.0, false), (20.0, false), (20.0, true), (50.0, true), (50.0, false)] {
                    let mut want = Vec::new();
                    let mut hard = 0;
                    for (i, part) in t.split('\n').enumerate() {
                        let ind = if i == 0 { indent } else { 0.0 };
                        old_wrap_hard(t, hard, hard + part.len(), max_w, ind, breaks && i == 0, &w, &mut want);
                        hard += part.len() + 1;
                    }
                    assert_eq!(wrap_indented(t, max_w, indent, breaks, &w), want, "{t:?} {max_w} {indent} {breaks}");
                }
            }
        }
    }

    /// The wrap as it was before the bounded measure, the reference for the test above.
    fn old_wrap_hard(text: &str, start: usize, end: usize, max_w: f32, indent: f32, break_before: bool, width: &dyn Fn(&str) -> f32, lines: &mut Vec<VisualLine>) {
        let mut line = start;
        let mut room = max_w - indent;
        loop {
            if width(text[line..end].trim_end_matches(' ')) <= room {
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
                } else if width(&text[line..space]) <= room {
                    cut = Some(after);
                    i = after;
                    if after >= end {
                        break;
                    }
                } else {
                    break;
                }
            }
            if cut.is_none() && break_before && line == start && room < max_w && line < end {
                // The prefix's own break: the first word moves down whole.
                lines.push(VisualLine { start: line, end: line });
                room = max_w;
                continue;
            }
            let cut = cut.unwrap_or_else(|| {
                // One word too wide: the longest char prefix that fits, at least one character.
                let mut c = next_boundary(text, line);
                while c < end {
                    let n = next_boundary(text, c);
                    if width(&text[line..n]) > room {
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
            room = max_w;
        }
    }

    #[test]
    fn wrap_cache_rewraps_only_on_change() {
        let calls = std::cell::Cell::new(0usize);
        let counted = |s: &str| {
            calls.set(calls.get() + 1);
            w(s)
        };
        let mut c = WrapCache::default();
        assert_eq!(c.wrap("aaaa bbbb", 40.0, 1, &counted), wrap("aaaa bbbb", 40.0, &w));
        let first = calls.get();
        assert!(first > 0);
        assert_eq!(c.wrap("aaaa bbbb", 40.0, 1, &counted), wrap("aaaa bbbb", 40.0, &w));
        assert_eq!(calls.get(), first, "a hit measures nothing");
        c.wrap("aaaa bbbb", 60.0, 1, &counted);
        assert!(calls.get() > first, "a new width wraps again");
        let n = calls.get();
        c.wrap("aaaa bbbb", 60.0, 2, &counted);
        assert!(calls.get() > n, "a new font wraps again");
        let n = calls.get();
        assert_eq!(c.wrap("aaaa bbbbc", 60.0, 2, &counted), wrap("aaaa bbbbc", 60.0, &w));
        assert!(calls.get() > n, "new text wraps again");
    }
}

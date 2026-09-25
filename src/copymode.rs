//! iTerm2's Copy Mode: a keyboard cursor over the buffer with vi-style moves, counts, and
//! character, line, and box selection (iTermCopyModeHandler.m and its key table).

use crate::find::Pos;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Char,
    Line,
    Box,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    None,
    Moved,
    /// Scroll the view this many lines (negative is up) without moving the cursor.
    Scroll(i32),
    /// Copy the selection, then leave Copy Mode.
    Copy,
    /// Leave Copy Mode; `clear` drops the selection too.
    Exit { clear: bool },
    /// Open the find bar.
    Find,
}

/// What the engine needs to know about the buffer. Rows count from the oldest line.
pub struct Ctx<'a> {
    pub rows: usize,
    pub cols: usize,
    pub view_top: usize,
    pub view_rows: usize,
    /// Rows with a mark, oldest first.
    pub marks: &'a [usize],
    /// A row's characters, `cols` long.
    pub line: &'a dyn Fn(usize) -> Vec<char>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub cursor: Pos,
    /// Where the selection started, while selecting.
    pub anchor: Option<Pos>,
    pub kind: Kind,
    count: usize,
}

impl State {
    pub fn new(cursor: Pos) -> State {
        State { cursor, anchor: None, kind: Kind::Char, count: 0 }
    }

    /// The selection, start first, with its kind.
    pub fn selection(&self) -> Option<(Pos, Pos, Kind)> {
        let a = self.anchor?;
        let (lo, hi) = if a <= self.cursor { (a, self.cursor) } else { (self.cursor, a) };
        Some((lo, hi, self.kind))
    }

    /// iTerm2's v, V, and Ctrl+V: selecting turns on or off, in this kind.
    fn toggle(&mut self, kind: Kind) {
        self.anchor = match self.anchor {
            Some(_) => None,
            None => Some(self.cursor),
        };
        self.kind = kind;
    }

    /// One key: a character ("h", "$", " "), or a name ("Left", "Home", "PageUp", "Esc",
    /// "C-b" for Ctrl+B, "C-Space").
    pub fn key(&mut self, key: &str, ctx: &Ctx) -> Outcome {
        // Counts: 1-9 start one, 0 continues one (else it is the line start).
        if let Some(d) = key.chars().next().filter(|_| key.len() == 1).and_then(|c| c.to_digit(10)) {
            if d != 0 || self.count > 0 {
                self.count = (self.count * 10 + d as usize).min(100_000);
                return Outcome::None;
            }
        }
        let n = std::mem::take(&mut self.count).max(1);
        let last_row = ctx.rows.saturating_sub(1);
        let last_col = ctx.cols.saturating_sub(1);
        let c = self.cursor;
        let at = |row: usize, col: usize| Pos { row: row.min(last_row), col: col.min(last_col) };
        let text_end = |row: usize| (ctx.line)(row).iter().rposition(|c| *c != ' ').unwrap_or(0);
        let text_start = |row: usize| (ctx.line)(row).iter().position(|c| *c != ' ').unwrap_or(0);
        let half = (ctx.view_rows / 2).max(1);
        let to = match key {
            "h" | "Left" => at(c.row, c.col.saturating_sub(n)),
            "l" | "Right" => at(c.row, c.col + n),
            "k" | "Up" => at(c.row.saturating_sub(n), c.col),
            "j" | "Down" => at(c.row + n, c.col),
            "w" | "W" | "b" | "B" => {
                let mut p = c;
                for _ in 0..n {
                    p = word_move(p, key, ctx);
                }
                p
            }
            "0" => at(c.row, 0),
            "^" => at(c.row, text_start(c.row)),
            "$" => at(c.row, text_end(c.row)),
            "g" | "Home" => at(0, 0),
            "G" | "End" => at(last_row, 0),
            "H" => at(ctx.view_top, c.col),
            "M" => at(ctx.view_top + ctx.view_rows / 2, c.col),
            "L" => at(ctx.view_top + ctx.view_rows.saturating_sub(1), c.col),
            "C-b" | "PageUp" => at(c.row.saturating_sub(ctx.view_rows * n), c.col),
            "C-f" | "PageDown" => at(c.row + ctx.view_rows * n, c.col),
            "C-u" => at(c.row.saturating_sub(half * n), c.col),
            "C-d" => at(c.row + half * n, c.col),
            "[" => at(ctx.marks.iter().rev().find(|m| **m < c.row).copied().unwrap_or(c.row), 0),
            "]" => at(ctx.marks.iter().find(|m| **m > c.row).copied().unwrap_or(c.row), 0),
            "C-y" => return Outcome::Scroll(-(n as i32)),
            "C-e" => return Outcome::Scroll(n as i32),
            "v" | " " => {
                self.toggle(Kind::Char);
                return Outcome::Moved;
            }
            "V" => {
                self.toggle(Kind::Line);
                return Outcome::Moved;
            }
            "C-v" | "C-Space" => {
                self.toggle(Kind::Box);
                return Outcome::Moved;
            }
            "o" => {
                if let Some(a) = self.anchor.replace(self.cursor) {
                    self.cursor = a;
                }
                return Outcome::Moved;
            }
            "y" | "C-k" => return Outcome::Copy,
            "q" | "Esc" => {
                self.anchor = None;
                return Outcome::Exit { clear: true };
            }
            "C-c" | "C-g" => return Outcome::Exit { clear: false },
            "/" => return Outcome::Find,
            _ => return Outcome::None,
        };
        self.cursor = to;
        Outcome::Moved
    }
}

/// Shift+arrow (alone, or with one of Alt, Ctrl, Win) enters Copy Mode when a selection
/// exists. Returns whether the key moves backward, so the cursor starts at the selection's
/// start; None when the key does not enter.
pub fn auto_enter(key: &str, shift: bool, alt: bool, ctrl: bool, win: bool) -> Option<bool> {
    let backward = match key {
        "Left" | "Up" | "Home" | "PageUp" => true,
        "Right" | "Down" | "End" | "PageDown" => false,
        _ => return None,
    };
    (shift && (alt as u8 + ctrl as u8 + win as u8) <= 1).then_some(backward)
}

/// Key action 73's parameter, a vim-style key string ("3j<C-v>$y"), as keys for `State::key`.
pub fn parse_vim_keys(s: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '<' {
            out.push(c.to_string());
            continue;
        }
        let mut name = String::new();
        let mut closed = false;
        for n in chars.by_ref() {
            if n == '>' {
                closed = true;
                break;
            }
            name.push(n);
        }
        if !closed || name.is_empty() {
            return Err(format!("Unclosed <{name} in the Copy Mode keys."));
        }
        out.push(match name.as_str() {
            "lt" => "<".into(),
            "Space" => " ".into(),
            "CR" | "Enter" | "Return" => "Enter".into(),
            n if n.len() > 2 && (n.starts_with("C-") || n.starts_with("c-")) => format!("C-{}", &n[2..]),
            n => n.to_string(),
        });
    }
    Ok(out)
}

/// vi's word classes: 0 blank, 1 word characters, 2 other; for W and B, 1 for any non-blank.
fn class(c: char, big: bool) -> u8 {
    if c == ' ' || c == '\0' {
        0
    } else if big || c.is_alphanumeric() || c == '_' {
        1
    } else {
        2
    }
}

/// One w, W, b, or B move, across rows (a row's end counts as blank).
fn word_move(from: Pos, key: &str, ctx: &Ctx) -> Pos {
    let big = key == "W" || key == "B";
    let cols = ctx.cols.max(1);
    let total = ctx.rows * cols;
    if total == 0 {
        return from;
    }
    let mut lines: std::collections::HashMap<usize, Vec<char>> = std::collections::HashMap::new();
    let mut ch = |i: usize| {
        let row = i / cols;
        let l = lines.entry(row).or_insert_with(|| (ctx.line)(row));
        l.get(i % cols).copied().unwrap_or(' ')
    };
    let mut i = from.row * cols + from.col;
    if key == "w" || key == "W" {
        let start = class(ch(i), big);
        if start != 0 {
            while i + 1 < total && class(ch(i), big) == start {
                i += 1;
            }
        }
        while i + 1 < total && class(ch(i), big) == 0 {
            i += 1;
        }
    } else {
        if i == 0 {
            return from;
        }
        i -= 1;
        while i > 0 && class(ch(i), big) == 0 {
            i -= 1;
        }
        let k = class(ch(i), big);
        while i > 0 && class(ch(i - 1), big) == k {
            i -= 1;
        }
    }
    Pos { row: i / cols, col: i % cols }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(row: usize, col: usize) -> Pos {
        Pos { row, col }
    }

    fn buf(lines: &[&str]) -> Vec<Vec<char>> {
        lines.iter().map(|l| format!("{l:<20}").chars().collect()).collect()
    }

    fn run(state: &mut State, lines: &[Vec<char>], keys: &[&str]) -> Outcome {
        let line = |r: usize| lines[r].clone();
        let ctx = Ctx { rows: lines.len(), cols: 20, view_top: 0, view_rows: 4, marks: &[1, 5], line: &line };
        let mut out = Outcome::None;
        for k in keys {
            out = state.key(k, &ctx);
        }
        out
    }

    #[test]
    fn hjkl_and_counts_move() {
        let b = buf(&["one two", "three", "four", "five", "six", "seven"]);
        let mut s = State::new(at(0, 0));
        run(&mut s, &b, &["l", "l", "j"]);
        assert_eq!(s.cursor, at(1, 2));
        run(&mut s, &b, &["3", "j"]);
        assert_eq!(s.cursor, at(4, 2));
        run(&mut s, &b, &["1", "0", "h"]);
        assert_eq!(s.cursor, at(4, 0), "10h: 0 continues a count");
        run(&mut s, &b, &["l", "0"]);
        assert_eq!(s.cursor, at(4, 0), "0 alone is the line start");
        run(&mut s, &b, &["$"]);
        assert_eq!(s.cursor, at(4, 2));
        run(&mut s, &b, &["[", "["]);
        assert_eq!(s.cursor, at(1, 0));
        run(&mut s, &b, &["]"]);
        assert_eq!(s.cursor, at(5, 0));
        run(&mut s, &b, &["H"]);
        assert_eq!(s.cursor.row, 0);
        run(&mut s, &b, &["L"]);
        assert_eq!(s.cursor.row, 3);
        run(&mut s, &b, &["C-u"]);
        assert_eq!(s.cursor.row, 1);
        assert_eq!(run(&mut s, &b, &["C-e"]), Outcome::Scroll(1));
    }

    #[test]
    fn moves_clamp_at_the_buffer_ends() {
        let lines: Vec<String> = (0..10_000).map(|i| format!("line {i}")).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let b = buf(&refs);
        let mut s = State::new(at(5, 3));
        run(&mut s, &b, &["9", "9", "9", "k"]);
        assert_eq!(s.cursor, at(0, 3));
        run(&mut s, &b, &["G"]);
        assert_eq!(s.cursor.row, 9_999);
        run(&mut s, &b, &["5", "j", "C-f", "C-d", "l", "9", "9", "l"]);
        assert_eq!(s.cursor, at(9_999, 19));
        run(&mut s, &b, &["g"]);
        assert_eq!(s.cursor, at(0, 0));
        run(&mut s, &b, &["C-b", "h", "b"]);
        assert_eq!(s.cursor, at(0, 0));
    }

    #[test]
    fn word_moves_follow_vi() {
        let b = buf(&["foo.bar baz", "  qux"]);
        let mut s = State::new(at(0, 0));
        run(&mut s, &b, &["w"]);
        assert_eq!(s.cursor, at(0, 3), "w stops at punctuation");
        run(&mut s, &b, &["w"]);
        assert_eq!(s.cursor, at(0, 4));
        run(&mut s, &b, &["w", "w"]);
        assert_eq!(s.cursor, at(1, 2), "w crosses to the next line");
        run(&mut s, &b, &["b"]);
        assert_eq!(s.cursor, at(0, 8));
        let mut s = State::new(at(0, 0));
        run(&mut s, &b, &["W"]);
        assert_eq!(s.cursor, at(0, 8), "W skips punctuation");
        run(&mut s, &b, &["B"]);
        assert_eq!(s.cursor, at(0, 0));
        run(&mut s, &b, &["j", "^"]);
        assert_eq!(s.cursor, at(1, 2));
    }

    #[test]
    fn v_shift_v_and_ctrl_v_select() {
        let b = buf(&["abc", "def"]);
        let mut s = State::new(at(0, 1));
        run(&mut s, &b, &["v", "j"]);
        assert_eq!(s.selection(), Some((at(0, 1), at(1, 1), Kind::Char)));
        run(&mut s, &b, &["v"]);
        assert_eq!(s.selection(), None, "v again stops selecting");
        run(&mut s, &b, &["V", "k"]);
        assert_eq!(s.selection(), Some((at(0, 1), at(1, 1), Kind::Line)));
        run(&mut s, &b, &["V", "C-v"]);
        assert_eq!(s.selection().map(|x| x.2), Some(Kind::Box));
        let mut s = State::new(at(1, 2));
        run(&mut s, &b, &[" ", "h"]);
        assert_eq!(s.selection(), Some((at(1, 1), at(1, 2), Kind::Char)));
    }

    #[test]
    fn o_swaps_the_ends() {
        let b = buf(&["abcdef"]);
        let mut s = State::new(at(0, 1));
        run(&mut s, &b, &["v", "l", "l", "o"]);
        assert_eq!((s.cursor, s.anchor), (at(0, 1), Some(at(0, 3))));
    }

    #[test]
    fn y_copies_and_exits() {
        let b = buf(&["abc"]);
        let mut s = State::new(at(0, 0));
        assert_eq!(run(&mut s, &b, &["v", "l", "y"]), Outcome::Copy);
        assert_eq!(run(&mut s, &b, &["C-k"]), Outcome::Copy);
        assert_eq!(run(&mut s, &b, &["q"]), Outcome::Exit { clear: true });
        assert_eq!(run(&mut s, &b, &["Esc"]), Outcome::Exit { clear: true });
        assert_eq!(run(&mut s, &b, &["C-c"]), Outcome::Exit { clear: false });
        assert_eq!(run(&mut s, &b, &["C-g"]), Outcome::Exit { clear: false });
        assert_eq!(run(&mut s, &b, &["/"]), Outcome::Find);
    }

    #[test]
    fn shift_arrows_enter_copy_mode() {
        assert_eq!(auto_enter("Left", true, false, false, false), Some(true));
        assert_eq!(auto_enter("PageDown", true, true, false, false), Some(false));
        assert_eq!(auto_enter("Up", true, false, true, false), Some(true));
        assert_eq!(auto_enter("End", true, false, false, true), Some(false));
        assert_eq!(auto_enter("Left", false, false, false, false), None);
        assert_eq!(auto_enter("Left", true, true, true, false), None, "two extra modifiers");
        assert_eq!(auto_enter("a", true, false, false, false), None);
    }

    #[test]
    fn vim_key_strings_parse() {
        assert_eq!(parse_vim_keys("3j<C-v>$y").unwrap(), vec!["3", "j", "C-v", "$", "y"]);
        assert_eq!(parse_vim_keys("<Esc><Space><lt>").unwrap(), vec!["Esc", " ", "<"]);
        assert!(parse_vim_keys("<C-v").is_err());
        assert!(parse_vim_keys("a<b").is_err(), "a later > elsewhere does not close it");
    }
}

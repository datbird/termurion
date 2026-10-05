//! The text around a Semantic History click: iTerm2's `wrappedLocatedStringAt:` (the logical
//! line, or the rows around the click when hard newlines are ignored), the byte limit each
//! way, and the filename part of the prefix and suffix (`UAF:437-452`). Pure; the rows come
//! from `find::rows_text`.

use crate::find::{Pos, RowText};
use crate::url_guess::CharSet;

/// Rows each way from the click that are joined when hard newlines are not respected
/// (`WTE:wrappedLocatedStringAt`).
pub const ROW_CLAMP: usize = 10;

/// The text before and after the click, and where each character of `before + after` sits.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Context {
    pub before: String,
    pub after: String,
    /// Byte offset of each character of `before + after` and its cell.
    pub cells: Vec<(usize, Pos)>,
}

impl Context {
    /// The bytes of `before + after` from the first character at or after cell `a` to the last
    /// at or before cell `b` (both ends inclusive); None when no character lies between them.
    pub fn range_between(&self, a: Pos, b: Pos) -> Option<std::ops::Range<usize>> {
        let first = self.cells.iter().position(|(_, p)| *p >= a)?;
        let last = self.cells.iter().rposition(|(_, p)| *p <= b)?;
        if last < first {
            return None;
        }
        let end = self.cells.get(last + 1).map_or(self.before.len() + self.after.len(), |(o, _)| *o);
        Some(self.cells[first].0..end)
    }

    /// The cell of the character holding byte `offset` of `before + after`.
    pub fn pos_at(&self, offset: usize) -> Option<Pos> {
        let i = self.cells.partition_point(|(o, _)| *o <= offset);
        self.cells.get(i.checked_sub(1)?).map(|(_, p)| *p)
    }
}

/// The cell of `row` the click at `col` lands on: its own, or the character before it when
/// that is a wide one (the spacer cell has no entry).
fn click_cell(row: &RowText, col: usize) -> Option<usize> {
    use unicode_width::UnicodeWidthChar;
    let exact = row.cells.iter().position(|(_, p)| p.col == col);
    exact.or_else(|| {
        let i = row.cells.iter().position(|(_, p)| col > 0 && p.col == col - 1)?;
        let c = row.text[row.cells[i].0..].chars().next()?;
        (c.width() == Some(2)).then_some(i)
    })
}

/// The text around column `click_col` of `rows[click_row]`: the logical line when hard
/// newlines are respected, else rows `click_row - 10 ..= click_row + 10` joined with no
/// separator; at most `max` bytes each side, cut on character boundaries. Nothing when the
/// click is not on a character.
pub fn build(rows: &[RowText], click_row: usize, click_col: usize, respect_hard: bool, max: usize) -> Context {
    let Some(row) = rows.get(click_row) else { return Context::default() };
    let Some(ci) = click_cell(row, click_col) else { return Context::default() };
    let (lo, hi) = if respect_hard {
        let mut lo = click_row;
        while lo > 0 && rows[lo - 1].wraps {
            lo -= 1;
        }
        let mut hi = click_row;
        while hi + 1 < rows.len() && rows[hi].wraps {
            hi += 1;
        }
        (lo, hi)
    } else {
        (click_row.saturating_sub(ROW_CLAMP), (click_row + ROW_CLAMP).min(rows.len() - 1))
    };
    let mut text = String::new();
    let mut cells: Vec<(usize, Pos)> = Vec::new();
    let mut click = 0;
    for (i, r) in rows[lo..=hi].iter().enumerate() {
        let base = text.len();
        if lo + i == click_row {
            click = base + row.cells[ci].0;
        }
        cells.extend(r.cells.iter().map(|(o, p)| (o + base, *p)));
        text.push_str(&r.text);
    }
    let mut start = click.saturating_sub(max);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let mut end = click.saturating_add(max).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let cells = cells.into_iter().filter(|(o, _)| (start..end).contains(o)).map(|(o, p)| (o - start, p)).collect();
    Context { before: text[start..click].to_string(), after: text[click..end].to_string(), cells }
}

/// Whether hard newlines end the text around a click (`UAH:50-55`, `WTE:3-5`): not with the
/// setting, in the alternate screen (`TVA:1442-1444`) or with a column window.
pub fn respect_hard(ignore_setting: bool, alt_screen: bool, windowed: bool) -> bool {
    !(ignore_setting || alt_screen || windowed)
}

/// The prefix's trailing run of filename characters and the suffix's leading run of them plus
/// `"` and `,` (`UAF:437-452`).
pub fn filename_parts(before: &str, after: &str, set: &CharSet) -> (String, String) {
    let start = before.char_indices().rev().take_while(|(_, c)| set.contains(*c)).last().map_or(before.len(), |(i, _)| i);
    let end = after.char_indices().find(|(_, c)| !(set.contains(*c) || *c == '"' || *c == ',')).map_or(after.len(), |(i, _)| i);
    (before[start..].to_string(), after[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::url_guess;

    /// Rows from strings: a trailing `\` means the row wraps. Cells are numbered by character.
    fn rows(lines: &[&str]) -> Vec<RowText> {
        lines
            .iter()
            .enumerate()
            .map(|(r, l)| {
                let (text, wraps) = l.strip_suffix('\\').map_or((*l, false), |t| (t, true));
                let mut cells = Vec::new();
                for (col, (o, _)) in text.char_indices().enumerate() {
                    cells.push((o, Pos { row: r, col }));
                }
                RowText { text: text.to_string(), cells, wraps }
            })
            .collect()
    }

    fn filename_set() -> CharSet {
        url_guess::filename(&url_guess::CharSet::default(), url_guess::DEFAULT_FILENAME_CHARS)
    }

    #[test]
    fn a_command_range_is_a_byte_range_of_the_context() {
        // "PS> dir x" with the click on "dir": the prompt is cells 0..4.
        let r = rows(&["PS> dir x"]);
        let c = build(&r, 0, 5, true, 2000);
        let at = |col| Pos { row: 0, col };
        assert_eq!(c.range_between(at(4), at(8)), Some(4..9));
        assert_eq!(c.range_between(at(4), at(6)), Some(4..7));
        // An end past the text still ends at the last character.
        assert_eq!(c.range_between(at(4), at(39)), Some(4..9));
        assert_eq!(c.range_between(at(9), at(39)), None);
        assert_eq!(c.range_between(at(6), at(4)), None);
        // A command on the next row of a wrapped prompt.
        let r = rows(&["PS> ab\\", "cd"]);
        let c = build(&r, 0, 4, true, 2000);
        assert_eq!(c.range_between(Pos { row: 0, col: 4 }, Pos { row: 1, col: 1 }), Some(4..8));
    }

    #[test]
    fn filename_parts_follow_iterm2() {
        let set = filename_set();
        assert_eq!(filename_parts("see <C:\\a b\\x.txt", ">, line 3", &set), ("C:\\a b\\x.txt".to_string(), String::new()));
        assert_eq!(
            filename_parts("at \"C:\\p\\f.py", "\", line 12, in x", &set),
            ("C:\\p\\f.py".to_string(), "\", line 12, in x".to_string())
        );
        assert_eq!(filename_parts("{a", "b}", &set), ("a".to_string(), "b".to_string()));
    }

    #[test]
    fn context_rows_follow_iterm2() {
        let r = rows(&["aaaa\\", "bbbb", "cccc"]);
        let c = build(&r, 1, 1, true, 2000);
        assert_eq!((c.before.as_str(), c.after.as_str()), ("aaaab", "bbb"), "the logical line aaaabbbb");
        let c = build(&r, 1, 1, false, 2000);
        assert_eq!((c.before.as_str(), c.after.as_str()), ("aaaab", "bbbcccc"), "every row joined");

        // The plus or minus 10 row clamp.
        let letters: Vec<String> = (0..30).map(|i| char::from(b'a' + (i % 26) as u8).to_string()).collect();
        let refs: Vec<&str> = letters.iter().map(String::as_str).collect();
        let r = rows(&refs);
        let c = build(&r, 15, 0, false, 2000);
        let expect: String = letters[5..15].concat();
        assert_eq!(c.before, expect, "rows 5 to 14");
        assert_eq!(c.after, letters[15..=25].concat(), "rows 15 to 25");

        // The byte limit.
        let r = rows(&["abcdefgh"]);
        let c = build(&r, 0, 4, true, 3);
        assert_eq!((c.before.as_str(), c.after.as_str()), ("bcd", "efg"));

        // A multi-byte character straddling the limit is dropped whole.
        let r = rows(&["a\u{e9}\u{e9}b\u{e9}\u{e9}c"]);
        let c = build(&r, 0, 3, true, 3);
        assert_eq!((c.before.as_str(), c.after.as_str()), ("\u{e9}", "b\u{e9}"), "3 bytes keeps one e-acute each side");

        // Every byte of before + after maps back to its row and column.
        let r = rows(&["ab\\", "c\u{e9}d"]);
        let c = build(&r, 1, 1, true, 2000);
        let joined = format!("{}{}", c.before, c.after);
        assert_eq!(joined, "abc\u{e9}d");
        let at = |o: usize| c.pos_at(o).map(|p| (p.row, p.col));
        assert_eq!(
            [at(0), at(1), at(2), at(3), at(4), at(5)],
            [Some((0, 0)), Some((0, 1)), Some((1, 0)), Some((1, 1)), Some((1, 1)), Some((1, 2))]
        );
        assert_eq!(c.cells.len(), 5, "one entry per character");
    }

    #[test]
    fn click_off_the_text_gives_nothing() {
        let r = rows(&["ab"]);
        assert_eq!(build(&r, 0, 5, true, 2000), Context::default());
        assert_eq!(build(&r, 3, 0, true, 2000), Context::default());
    }

    #[test]
    fn alt_screen_ignores_hard_newlines() {
        assert!(respect_hard(false, false, false));
        assert!(!respect_hard(true, false, false));
        assert!(!respect_hard(false, true, false), "R3: full-screen apps");
        assert!(!respect_hard(false, false, true));
    }
}

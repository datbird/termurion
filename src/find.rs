//! iTerm2's find: the five search modes, every match in the buffer, and moving between
//! matches. Find Next goes up (toward older text), as iTerm2 does by default.

use crate::screen::Screen;

/// iTerm2's `findMode_iTerm` values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    SmartCase = 0,
    CaseSensitive = 1,
    CaseInsensitive = 2,
    RegexSensitive = 3,
    RegexInsensitive = 4,
}

impl Mode {
    pub const ALL: [Mode; 5] =
        [Mode::SmartCase, Mode::CaseSensitive, Mode::CaseInsensitive, Mode::RegexSensitive, Mode::RegexInsensitive];

    pub fn from_i64(v: i64) -> Mode {
        Mode::ALL.into_iter().find(|m| *m as i64 == v).unwrap_or(Mode::SmartCase)
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::SmartCase => "Smart Case",
            Mode::CaseSensitive => "Case Sensitive",
            Mode::CaseInsensitive => "Case Insensitive",
            Mode::RegexSensitive => "Case-Sensitive Regex",
            Mode::RegexInsensitive => "Case-Insensitive Regex",
        }
    }
}

/// A place in the buffer: `row` counts from the oldest line of scrollback.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub row: usize,
    pub col: usize,
}

/// One match, both ends inclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Match {
    pub start: Pos,
    pub end: Pos,
}

/// The regular expression for a query in a mode.
pub fn pattern(query: &str, mode: Mode) -> String {
    let (is_regex, insensitive) = match mode {
        Mode::SmartCase => (false, !query.chars().any(char::is_uppercase)),
        Mode::CaseSensitive => (false, false),
        Mode::CaseInsensitive => (false, true),
        Mode::RegexSensitive => (true, false),
        Mode::RegexInsensitive => (true, true),
    };
    let body = if is_regex { query.to_string() } else { regex::escape(query) };
    if insensitive { format!("(?i){body}") } else { body }
}

/// One logical line: its text and, for each character, its byte offset and cell.
pub struct Logical {
    pub text: String,
    pub cells: Vec<(usize, Pos)>,
}

impl Logical {
    /// The cell of the character at byte `offset`, or of the last one before it.
    pub fn pos_before(&self, offset: usize) -> Pos {
        let i = self.cells.partition_point(|(o, _)| *o <= offset);
        self.cells[i.max(1) - 1].1
    }
}

/// The buffer as logical lines, oldest first: rows joined where they wrap, wide-character
/// spacers left out, trailing spaces trimmed.
pub fn logical_lines(screen: &Screen) -> Vec<Logical> {
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::{Column, Line};
    use alacritty_terminal::term::cell::Flags;
    let grid = screen.term().grid();
    let history = grid.history_size() as i32;
    let cols = grid.columns();
    let mut out = Vec::new();
    let mut cur = Logical { text: String::new(), cells: Vec::new() };
    for l in -history..grid.screen_lines() as i32 {
        let row = &grid[Line(l)];
        let r = (l + history) as usize;
        for c in 0..cols {
            let cell = &row[Column(c)];
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            cur.cells.push((cur.text.len(), Pos { row: r, col: c }));
            cur.text.push(cell.c);
            for z in cell.zerowidth().into_iter().flatten() {
                cur.text.push(*z);
            }
        }
        if !row[Column(cols - 1)].flags.contains(Flags::WRAPLINE) {
            let kept = cur.text.trim_end_matches(' ').len();
            cur.text.truncate(kept);
            cur.cells.retain(|(o, _)| *o < kept);
            out.push(std::mem::replace(&mut cur, Logical { text: String::new(), cells: Vec::new() }));
        }
    }
    if !cur.cells.is_empty() {
        out.push(cur);
    }
    out
}

/// Every match of `pattern` in the buffer, top to bottom. Matches do not cross logical
/// lines, and a match of no text is skipped.
pub fn find_all(screen: &Screen, pattern: &str) -> Result<Vec<Match>, String> {
    if pattern.is_empty() {
        return Ok(Vec::new());
    }
    let re = regex::Regex::new(pattern).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for line in logical_lines(screen) {
        for m in re.find_iter(&line.text) {
            if m.is_empty() {
                continue;
            }
            out.push(Match { start: line.pos_before(m.start()), end: line.pos_before(m.end() - 1) });
        }
    }
    Ok(out)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Results {
    pub matches: Vec<Match>,
    pub current: Option<usize>,
}

impl Results {
    /// Moves to the next match up (`up`) or down, wrapping at the ends. Returns the match
    /// and whether the move wrapped.
    pub fn step(&mut self, up: bool) -> (Option<&Match>, bool) {
        let n = self.matches.len();
        if n == 0 {
            self.current = None;
            return (None, false);
        }
        let (next, wrapped) = match (self.current, up) {
            (None, true) => (n - 1, false),
            (None, false) => (0, false),
            (Some(0), true) => (n - 1, true),
            (Some(i), true) => (i - 1, false),
            (Some(i), false) if i + 1 >= n => (0, true),
            (Some(i), false) => (i + 1, false),
        };
        self.current = Some(next);
        (self.matches.get(next), wrapped)
    }

    /// Makes the nearest match at or above `from` current.
    pub fn start_at(&mut self, from: Pos) {
        let above = self.matches.partition_point(|m| m.start <= from);
        self.current = if self.matches.is_empty() { None } else { Some(above.max(1) - 1) };
    }

    /// New matches for the same query: the current match stays on the same text.
    pub fn update(&mut self, matches: Vec<Match>) {
        let was = self.current.and_then(|i| self.matches.get(i)).map(|m| m.start);
        self.matches = matches;
        self.current = was.and_then(|start| self.matches.iter().position(|m| m.start == start));
    }

    /// "N", or "i/N" with a current match, i counted from the bottom.
    pub fn label(&self) -> String {
        let n = self.matches.len();
        match self.current {
            Some(i) if i < n => format!("{}/{n}", n - i),
            _ => n.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(text: &str) -> Screen {
        let mut s = Screen::with_scrollback(20, 4, 1000);
        s.feed(text.replace('\n', "\r\n").as_bytes());
        s
    }

    fn at(row: usize, col: usize) -> Pos {
        Pos { row, col }
    }

    #[test]
    fn smart_case_follows_uppercase() {
        let s = screen("foo Foo FOO");
        let lower = find_all(&s, &pattern("foo", Mode::SmartCase)).unwrap();
        assert_eq!(lower.len(), 3);
        let upper = find_all(&s, &pattern("Foo", Mode::SmartCase)).unwrap();
        assert_eq!(upper, vec![Match { start: at(0, 4), end: at(0, 6) }]);
        assert_eq!(find_all(&s, &pattern("foo", Mode::CaseSensitive)).unwrap().len(), 1);
        assert_eq!(find_all(&s, &pattern("Foo", Mode::CaseInsensitive)).unwrap().len(), 3);
        assert_eq!(find_all(&s, &pattern("f.o", Mode::RegexSensitive)).unwrap().len(), 1);
        assert_eq!(find_all(&s, &pattern("f.o", Mode::RegexInsensitive)).unwrap().len(), 3);
    }

    #[test]
    fn substring_modes_escape_the_query() {
        let s = screen("a.c abc (x)");
        assert_eq!(find_all(&s, &pattern("a.c", Mode::SmartCase)).unwrap().len(), 1);
        assert_eq!(find_all(&s, &pattern("(x)", Mode::CaseSensitive)).unwrap().len(), 1);
        assert_eq!(find_all(&s, &pattern("a.c", Mode::RegexSensitive)).unwrap().len(), 2);
        assert!(find_all(&s, &pattern("(x", Mode::RegexSensitive)).is_err());
    }

    #[test]
    fn finds_every_match_top_to_bottom() {
        // A 20-column screen: "ab" wraps onto the second row inside one logical line.
        let s = screen("xx ab\n..................ab\nab");
        let m = find_all(&s, &pattern("ab", Mode::SmartCase)).unwrap();
        assert_eq!(
            m,
            vec![
                Match { start: at(0, 3), end: at(0, 4) },
                Match { start: at(1, 18), end: at(1, 19) },
                Match { start: at(2, 0), end: at(2, 1) },
            ]
        );
        let wrapped = screen("..................abcd");
        let m = find_all(&wrapped, &pattern("abcd", Mode::SmartCase)).unwrap();
        assert_eq!(m, vec![Match { start: at(0, 18), end: at(1, 1) }]);
    }

    #[test]
    fn wide_characters_take_two_cells() {
        let s = screen("\u{4e2d}ab");
        let m = find_all(&s, &pattern("ab", Mode::SmartCase)).unwrap();
        assert_eq!(m, vec![Match { start: at(0, 2), end: at(0, 3) }]);
    }

    #[test]
    fn empty_matches_are_skipped() {
        let s = screen("axxb");
        let m = find_all(&s, &pattern("x*", Mode::RegexSensitive)).unwrap();
        assert_eq!(m, vec![Match { start: at(0, 1), end: at(0, 2) }]);
        assert!(find_all(&s, "").unwrap().is_empty());
    }

    fn three() -> Results {
        let m = |r| Match { start: at(r, 0), end: at(r, 1) };
        Results { matches: vec![m(0), m(1), m(2)], current: None }
    }

    #[test]
    fn find_next_goes_up_and_wraps() {
        let mut r = three();
        r.start_at(at(2, 5));
        assert_eq!(r.current, Some(2));
        assert_eq!(r.step(true).0.unwrap().start.row, 1);
        let first = r.matches[0];
        assert_eq!(r.step(true), (Some(&first), false));
        let (m, wrapped) = r.step(true);
        assert_eq!((m.unwrap().start.row, wrapped), (2, true));
        let (m, wrapped) = r.step(false);
        assert_eq!((m.unwrap().start.row, wrapped), (0, true));
        let mut none = three();
        assert_eq!(none.step(true).0.unwrap().start.row, 2);
        let mut empty = Results::default();
        assert_eq!(empty.step(true), (None, false));
    }

    #[test]
    fn start_at_above_the_first_match_takes_the_first() {
        let mut r = three();
        r.matches.remove(0);
        r.start_at(at(0, 0));
        assert_eq!(r.current, Some(0));
    }

    #[test]
    fn the_count_counts_from_the_bottom() {
        let mut r = three();
        assert_eq!(r.label(), "3");
        r.current = Some(2);
        assert_eq!(r.label(), "1/3");
        r.current = Some(0);
        assert_eq!(r.label(), "3/3");
        assert_eq!(Results::default().label(), "0");
    }

    #[test]
    fn new_output_keeps_matches_in_order() {
        let mut s = screen("ab\nab\n");
        let mut r = Results { matches: find_all(&s, "ab").unwrap(), current: None };
        r.current = Some(0);
        s.feed(b"x\r\nab\r\nab\r\nab\r\n");
        r.update(find_all(&s, "ab").unwrap());
        assert_eq!(r.matches.len(), 5);
        assert!(r.matches.windows(2).all(|w| w[0].start < w[1].start));
        assert_eq!(r.current, Some(0));
        assert_eq!(r.matches[0].start, at(0, 0));
        r.update(Vec::new());
        assert_eq!(r.current, None);
    }

    #[test]
    fn modes_round_trip() {
        for m in Mode::ALL {
            assert_eq!(Mode::from_i64(m as i64), m);
        }
        assert_eq!(Mode::from_i64(9), Mode::SmartCase);
        assert_eq!(Mode::SmartCase.label(), "Smart Case");
    }
}

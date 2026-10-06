//! iTerm2's find: the five search modes, every match in the buffer, and moving between
//! matches. Find Next goes up (toward older text), as iTerm2 does by default.

use crate::screen::Screen;

/// The cells the text builders below have read on this thread: the work counter the bound tests
/// watch (a huge wrapped line must cost the same as a short one).
#[cfg(test)]
pub(crate) mod work {
    thread_local!(static CELLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) });
    pub(crate) fn add(n: usize) {
        CELLS.with(|c| c.set(c.get() + n));
    }
    pub(crate) fn take() -> usize {
        CELLS.with(|c| c.replace(0))
    }
}

/// iTerm2's `findMode_iTerm` values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
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

    /// The status bar Search and Filter Tools' mode menu items (`iTermMiniSearchField`).
    pub fn menu_title(self) -> &'static str {
        match self {
            Mode::SmartCase => "Smart Case Substring",
            Mode::CaseSensitive => "Case-Sensitive Substring",
            Mode::CaseInsensitive => "Case-Insensitive Substring",
            Mode::RegexSensitive => "Case-Sensitive Regex",
            Mode::RegexInsensitive => "Case-Insensitive Regex",
        }
    }
}

/// A place in the buffer: `row` counts from the oldest line of scrollback.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
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
    pub fn clone_line(&self) -> Logical {
        Logical { text: self.text.clone(), cells: self.cells.clone() }
    }

    /// Lines joined into one with `sep` between them.
    pub fn join(lines: &[Logical], sep: &str) -> Logical {
        let mut out = Logical { text: String::new(), cells: Vec::new() };
        for (i, l) in lines.iter().enumerate() {
            if i > 0 {
                out.text.push_str(sep);
            }
            let base = out.text.len();
            out.cells.extend(l.cells.iter().map(|(o, p)| (o + base, *p)));
            out.text.push_str(&l.text);
        }
        out
    }

    /// The byte offset of the character in cell `at`.
    pub fn offset_of(&self, at: Pos) -> Option<usize> {
        self.cells.iter().find(|(_, p)| *p == at).map(|(o, _)| *o)
    }

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
    let grid = screen.term().grid();
    logical_between(screen, -(grid.history_size() as i32), grid.screen_lines() as i32 - 1)
}

pub(crate) fn wraps(screen: &Screen, line: i32) -> bool {
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::{Column, Line};
    let grid = screen.term().grid();
    grid[Line(line)][Column(grid.columns() - 1)].flags.contains(alacritty_terminal::term::cell::Flags::WRAPLINE)
}

/// The logical line holding buffer row `row`, with up to `radius` more each way, and the
/// index of the one holding `row`.
pub fn logical_lines_around(screen: &Screen, row: usize, radius: usize) -> (Vec<Logical>, usize) {
    use alacritty_terminal::grid::Dimensions;
    let grid = screen.term().grid();
    let (top, bottom) = (-(grid.history_size() as i32), grid.screen_lines() as i32 - 1);
    let line = (row as i32 + top).clamp(top, bottom);
    let start_of = |mut l: i32| {
        while l > top && wraps(screen, l - 1) {
            l -= 1;
        }
        l
    };
    let end_of = |mut l: i32| {
        while l < bottom && wraps(screen, l) {
            l += 1;
        }
        l
    };
    let own = start_of(line);
    let mut first = own;
    let mut above = 0;
    while above < radius && first > top {
        first = start_of(first - 1);
        above += 1;
    }
    let mut last = end_of(line);
    for _ in 0..radius {
        if last >= bottom {
            break;
        }
        last = end_of(last + 1);
    }
    (logical_between(screen, first, last), above)
}

/// The logical line holding buffer row `row`, built from at most `up` cells of rows above it and
/// `down` cells of rows below it (a wrapped line of any length costs no more than that), or only
/// that row's cells inside the columns `window`. None past the buffer.
pub fn logical_around(screen: &Screen, row: usize, window: Option<(usize, usize)>, up: usize, down: usize) -> Option<Logical> {
    use alacritty_terminal::grid::Dimensions;
    let grid = screen.term().grid();
    let (top, bottom) = (-(grid.history_size() as i32), grid.screen_lines() as i32 - 1);
    let line = row as i32 + top;
    if line < top || line > bottom {
        return None;
    }
    if window.is_some() {
        let r = rows_text(screen, line, line, window).into_iter().next()?;
        return Some(Logical { text: r.text, cells: r.cells });
    }
    let cols = grid.columns().max(1);
    let (rows_up, rows_down) = ((up / cols + 1) as i32, (down / cols + 1) as i32);
    let (mut first, mut last) = (line, line);
    while first > top && line - first < rows_up && wraps(screen, first - 1) {
        first -= 1;
    }
    while last < bottom && last - line < rows_down && wraps(screen, last) {
        last += 1;
    }
    logical_between(screen, first, last).into_iter().next()
}

/// The text of every logical line, oldest first, without the cell map (for Filter).
pub fn logical_texts(screen: &Screen) -> Vec<String> {
    use alacritty_terminal::grid::Dimensions;
    let grid = screen.term().grid();
    texts_between(screen, -(grid.history_size() as i32), grid.screen_lines() as i32 - 1)
}

/// The logical lines on screen, widened over the lines that run past it by at most `reach` cells
/// each way (a visible line can be a whole scrollback of wrapped rows, P10).
pub fn visible_lines(screen: &Screen, reach: usize) -> Vec<Logical> {
    use alacritty_terminal::grid::Dimensions;
    let grid = screen.term().grid();
    let (top, bottom) = (-(grid.history_size() as i32), grid.screen_lines() as i32 - 1);
    let (a0, b0) = (-(grid.display_offset() as i32), (-(grid.display_offset() as i32) + grid.screen_lines() as i32 - 1).min(bottom));
    let room = (reach / grid.columns().max(1) + 1) as i32;
    let (mut a, mut b) = (a0, b0);
    while a > top && a0 - a < room && wraps(screen, a - 1) {
        a -= 1;
    }
    while b < bottom && b - b0 < room && wraps(screen, b) {
        b += 1;
    }
    logical_between(screen, a, b)
}

/// The buffer rows on screen.
pub fn visible_rows(screen: &Screen) -> std::ops::RangeInclusive<usize> {
    use alacritty_terminal::grid::Dimensions;
    let grid = screen.term().grid();
    let top = grid.history_size() - grid.display_offset();
    top..=top + grid.screen_lines() - 1
}

/// The text of the logical lines over buffer rows `first..=last`, widened to whole lines.
pub fn logical_texts_over(screen: &Screen, first: usize, last: usize) -> Vec<String> {
    lines_over(screen, first, last).into_iter().map(|(text, _, _)| text).collect()
}

/// `logical_texts_over` with each line's first and last grid line.
pub(crate) fn lines_over(screen: &Screen, first: usize, last: usize) -> Vec<(String, i32, i32)> {
    use alacritty_terminal::grid::Dimensions;
    let grid = screen.term().grid();
    let (top, bottom) = (-(grid.history_size() as i32), grid.screen_lines() as i32 - 1);
    let mut a = (first as i32 + top).clamp(top, bottom);
    let mut b = (last as i32 + top).clamp(top, bottom);
    while a > top && wraps(screen, a - 1) {
        a -= 1;
    }
    while b < bottom && wraps(screen, b) {
        b += 1;
    }
    lines_between(screen, a, b)
}

/// One cell's text as it is shown: its characters in the profile's normalization form, so a
/// search matches what the pane draws (never across cells, so a match still covers whole cells).
fn push_cell_text(out: &mut String, screen: &Screen, cell: &alacritty_terminal::term::cell::Cell) {
    // Nothing to normalize: the common case adds no allocation.
    if screen.normalization == crate::encoding::Form::None || (cell.c.is_ascii() && cell.zerowidth().is_none()) {
        out.push(cell.c);
        out.extend(cell.zerowidth().into_iter().flatten());
        return;
    }
    let mut text = String::from(cell.c);
    text.extend(cell.zerowidth().into_iter().flatten());
    crate::encoding::normalize_in_place(&mut text, screen.normalization);
    out.push_str(&text);
}

fn texts_between(screen: &Screen, top: i32, bottom: i32) -> Vec<String> {
    lines_between(screen, top, bottom).into_iter().map(|(text, _, _)| text).collect()
}

/// The text of the logical lines over grid lines `top..=bottom` (which must start a logical
/// line), each with its first and last grid line.
pub(crate) fn lines_between(screen: &Screen, top: i32, bottom: i32) -> Vec<(String, i32, i32)> {
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::{Column, Line};
    use alacritty_terminal::term::cell::Flags;
    let grid = screen.term().grid();
    let cols = grid.columns();
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut first = top;
    for l in top..=bottom {
        let row = &grid[Line(l)];
        let wrapped = row[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
        let end = if wrapped { cols } else { (0..cols).rev().find(|&c| row[Column(c)].c != ' ').map_or(0, |c| c + 1) };
        for c in 0..end {
            let cell = &row[Column(c)];
            if !cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                push_cell_text(&mut cur, screen, cell);
            }
        }
        if !wrapped || l == bottom {
            let kept = cur.trim_end_matches(' ').len();
            cur.truncate(kept);
            out.push((std::mem::take(&mut cur), first, l));
            first = l + 1;
        }
    }
    out
}

/// Where a chunk that starts at grid line `start` (a logical line's first) ends: the last row
/// of the `max_lines`th logical line, or of the first line that takes the chunk past
/// `max_cells` cells, or `end`. A line is never split, so a chunk can pass `max_cells` by one.
pub(crate) fn chunk_end(screen: &Screen, start: i32, end: i32, max_lines: usize, max_cells: usize) -> i32 {
    use alacritty_terminal::grid::Dimensions;
    let cols = screen.term().columns();
    let (mut l, mut lines, mut cells) = (start, 0usize, 0usize);
    loop {
        while l < end && wraps(screen, l) {
            l += 1;
            cells += cols;
        }
        lines += 1;
        cells += cols;
        if l >= end || lines >= max_lines || cells >= max_cells {
            return l.min(end);
        }
        l += 1;
    }
}

/// Logical lines over grid lines `top..=bottom`, which must start a logical line.
fn logical_between(screen: &Screen, top: i32, bottom: i32) -> Vec<Logical> {
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::{Column, Line};
    use alacritty_terminal::term::cell::Flags;
    let grid = screen.term().grid();
    let history = grid.history_size() as i32;
    let cols = grid.columns();
    let mut out = Vec::new();
    let mut cur = Logical { text: String::new(), cells: Vec::new() };
    for l in top..=bottom {
        let row = &grid[Line(l)];
        let r = (l + history) as usize;
        let wrapped = row[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
        let end = if wrapped { cols } else { (0..cols).rev().find(|&c| row[Column(c)].c != ' ').map_or(0, |c| c + 1) };
        #[cfg(test)]
        work::add(end);
        for c in 0..end {
            let cell = &row[Column(c)];
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            cur.cells.push((cur.text.len(), Pos { row: r, col: c }));
            push_cell_text(&mut cur.text, screen, cell);
        }
        if !wrapped || l == bottom {
            let kept = cur.text.trim_end_matches(' ').len();
            cur.text.truncate(kept);
            cur.cells.retain(|(o, _)| *o < kept);
            out.push(std::mem::replace(&mut cur, Logical { text: String::new(), cells: Vec::new() }));
        }
    }
    out
}

/// One screen row's text for the Semantic History context: its characters inside the column
/// window with their cells, and whether the row wraps into the next.
#[derive(Clone, Debug, PartialEq)]
pub struct RowText {
    pub text: String,
    pub cells: Vec<(usize, Pos)>,
    pub wraps: bool,
}

/// The rows over grid lines `first..=last` (clamped to the buffer), each limited to the columns
/// of `window` (inclusive) when given, trailing blank cells dropped.
pub fn rows_text(screen: &Screen, first: i32, last: i32, window: Option<(usize, usize)>) -> Vec<RowText> {
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::{Column, Line};
    use alacritty_terminal::term::cell::Flags;
    let grid = screen.term().grid();
    let (top, bottom) = (-(grid.history_size() as i32), grid.screen_lines() as i32 - 1);
    let history = grid.history_size() as i32;
    let cols = grid.columns();
    let (from, to) = window.map_or((0, cols - 1), |(a, b)| (a.min(cols - 1), b.min(cols - 1)));
    let mut out = Vec::new();
    for l in first.max(top)..=last.min(bottom) {
        let row = &grid[Line(l)];
        let wraps = row[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
        let end = if wraps { to + 1 } else { (from..=to).rev().find(|&c| row[Column(c)].c != ' ').map_or(from, |c| c + 1) };
        let mut r = RowText { text: String::new(), cells: Vec::new(), wraps };
        #[cfg(test)]
        work::add(end.saturating_sub(from));
        for c in from..end {
            let cell = &row[Column(c)];
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            r.cells.push((r.text.len(), Pos { row: (l + history) as usize, col: c }));
            push_cell_text(&mut r.text, screen, cell);
        }
        out.push(r);
    }
    out
}

/// Every cell's character over grid lines `first..=last` (clamped), for finding dividers.
pub fn cell_rows(screen: &Screen, first: i32, last: i32) -> Vec<Vec<char>> {
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::{Column, Line};
    let grid = screen.term().grid();
    let (top, bottom) = (-(grid.history_size() as i32), grid.screen_lines() as i32 - 1);
    (first.max(top)..=last.min(bottom)).map(|l| (0..grid.columns()).map(|c| grid[Line(l)][Column(c)].c).collect()).collect()
}

/// Every match of `pattern` in the buffer, top to bottom. Matches do not cross logical
/// lines, and a match of no text is skipped.
pub fn find_all(screen: &Screen, pattern: &str) -> Result<Vec<Match>, String> {
    Ok(find_hits(screen, pattern)?.into_iter().map(|(m, _, _)| m).collect())
}

/// `find_all` over buffer rows `rows` (first, last inclusive) only, the selected command's
/// (`updateSearchRange`, `PS:21819-21834`): a match counts when it starts on one of them. None
/// searches the whole buffer.
pub fn find_all_in(screen: &Screen, pattern: &str, rows: Option<(usize, usize)>) -> Result<Vec<Match>, String> {
    Ok(find_hits_in(screen, pattern, rows)?.into_iter().map(|(m, _, _)| m).collect())
}

/// Like `find_all`, with each match's logical line and its byte range there.
pub fn find_hits(screen: &Screen, pattern: &str) -> Result<Vec<(Match, String, std::ops::Range<usize>)>, String> {
    find_hits_in(screen, pattern, None)
}

/// `find_hits` restricted to the matches that start on buffer rows `rows`; only the logical lines
/// over those rows are read.
pub fn find_hits_in(screen: &Screen, pattern: &str, rows: Option<(usize, usize)>) -> Result<Vec<(Match, String, std::ops::Range<usize>)>, String> {
    use alacritty_terminal::grid::Dimensions;
    if pattern.is_empty() {
        return Ok(Vec::new());
    }
    let re = regex::Regex::new(pattern).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let lines = match rows {
        None => logical_lines(screen),
        Some((first, last)) => {
            let grid = screen.term().grid();
            let (top, bottom) = (-(grid.history_size() as i32), grid.screen_lines() as i32 - 1);
            let (a, b) = ((first as i32 + top).clamp(top, bottom), (last as i32 + top).clamp(top, bottom));
            let (mut a, mut b) = (a, b);
            while a > top && wraps(screen, a - 1) {
                a -= 1;
            }
            while b < bottom && wraps(screen, b) {
                b += 1;
            }
            logical_between(screen, a, b)
        }
    };
    for line in lines {
        let found: Vec<_> = re.find_iter(&line.text).filter(|m| !m.is_empty()).map(|m| m.range()).collect();
        for r in found {
            let m = Match { start: line.pos_before(r.start), end: line.pos_before(r.end - 1) };
            if rows.is_some_and(|(first, last)| m.start.row < first || m.start.row > last) {
                continue;
            }
            out.push((m, line.text.clone(), r));
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
    /// `anchor` is where the current match's text is now (the selection, which moves with
    /// the text); without it the old position is used.
    pub fn update(&mut self, matches: Vec<Match>, anchor: Option<Pos>) {
        let was = self.current.and_then(|i| self.matches.get(i)).map(|m| m.start);
        self.matches = matches;
        self.current = anchor.or(was).and_then(|start| self.matches.iter().position(|m| m.start == start));
    }

    /// "N", or "i/N" with a current match, i counted from the bottom with `swap` on (iTerm2's
    /// default) and from the top with it off (`iTermSearchFieldCell.m:299`).
    pub fn label(&self, swap: bool) -> String {
        let n = self.matches.len();
        match self.current {
            Some(i) if i < n => format!("{}/{n}", if swap { n - i } else { i + 1 }),
            _ => n.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    /// A chunk ends on a logical line's last row, at the line budget or the cell budget (P7).
    #[test]
    fn chunks_end_on_whole_lines() {
        let mut s = Screen::with_scrollback(10, 4, 100);
        // Rows 0 to 2 are one logical line (25 characters), then "a" and "b".
        s.feed(format!("{}\r\na\r\nb", "x".repeat(25)).as_bytes());
        use alacritty_terminal::grid::Dimensions;
        let top = -(s.term().grid().history_size() as i32);
        assert_eq!(chunk_end(&s, top, top + 4, 1, usize::MAX), top + 2, "the budget in lines");
        assert_eq!(chunk_end(&s, top, top + 4, 2, usize::MAX), top + 3);
        assert_eq!(chunk_end(&s, top, top + 4, usize::MAX, 10), top + 2, "a line is never split by the cell budget");
        assert_eq!(chunk_end(&s, top, top + 4, usize::MAX, usize::MAX), top + 4, "no budget reads to the end");
    }

    use super::*;

    #[test]
    fn bar_mode_titles_follow_iterm2() {
        let titles: Vec<&str> = Mode::ALL.iter().map(|m| m.menu_title()).collect();
        assert_eq!(titles, ["Smart Case Substring", "Case-Sensitive Substring", "Case-Insensitive Substring", "Case-Sensitive Regex", "Case-Insensitive Regex"]);
    }

    fn screen(text: &str) -> Screen {
        let mut s = Screen::with_scrollback(20, 4, 1000);
        s.feed(text.replace('\n', "\r\n").as_bytes());
        s
    }

    fn at(row: usize, col: usize) -> Pos {
        Pos { row, col }
    }

    /// Find matches what is shown: an NFC profile finds "caf\u{e9}" in cells that hold e and a
    /// combining acute, and the match covers those cells.
    #[test]
    fn find_follows_the_normalization_form() {
        let look = |form| crate::screen::Look { normalization: form, ..crate::screen::Look::default() };
        let mut s = Screen::with_look(20, 4, crate::palette::Palette::default(), look(crate::encoding::Form::Nfc));
        s.feed("cafe\u{301} au lait".as_bytes());
        let hits = find_all(&s, "caf\u{e9}").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!((hits[0].start, hits[0].end), (at(0, 0), at(0, 3)));
        let mut plain = Screen::with_look(20, 4, crate::palette::Palette::default(), look(crate::encoding::Form::None));
        plain.feed("cafe\u{301} au lait".as_bytes());
        assert!(find_all(&plain, "caf\u{e9}").unwrap().is_empty());
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

    /// `iTermSearchFieldCell.m:299`: with the swap on i counts from the bottom, off from the top.
    #[test]
    fn label_follows_swap() {
        let m = |r| Match { start: at_pos(r, 0), end: at_pos(r, 1) };
        let r = Results { matches: (0..5).map(m).collect(), current: Some(1) };
        assert_eq!(r.label(true), "4/5");
        assert_eq!(r.label(false), "2/5");
        assert_eq!(Results::default().label(false), "0");
    }

    #[test]
    fn the_count_counts_from_the_bottom() {
        let mut r = three();
        assert_eq!(r.label(true), "3");
        r.current = Some(2);
        assert_eq!(r.label(true), "1/3");
        r.current = Some(0);
        assert_eq!(r.label(true), "3/3");
        assert_eq!(Results::default().label(true), "0");
    }

    #[test]
    fn new_output_keeps_matches_in_order() {
        let mut s = screen("ab\nab\n");
        let mut r = Results { matches: find_all(&s, "ab").unwrap(), current: None };
        r.current = Some(0);
        s.feed(b"x\r\nab\r\nab\r\nab\r\n");
        r.update(find_all(&s, "ab").unwrap(), None);
        assert_eq!(r.matches.len(), 5);
        assert!(r.matches.windows(2).all(|w| w[0].start < w[1].start));
        assert_eq!(r.current, Some(0));
        assert_eq!(r.matches[0].start, at(0, 0));
        r.update(Vec::new(), None);
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

    #[test]
    fn lines_around_a_row_follow_wrapping() {
        // 20 columns: the second line wraps onto two rows.
        let s = screen("l0\nl1\n..................wrap\nl3\nl4\nl5");
        let (lines, at) = logical_lines_around(&s, 3, 1);
        let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, vec!["l1", "..................wrap", "l3"]);
        assert_eq!(at, 1);
        let joined = Logical::join(&lines, "\n");
        assert_eq!(joined.text, "l1\n..................wrap\nl3");
        let o = joined.offset_of(at_pos(3, 1)).unwrap();
        assert_eq!(&joined.text[o..o + 1], "p");
        assert_eq!(joined.pos_before(o), at_pos(3, 1));
    }

    fn at_pos(row: usize, col: usize) -> Pos {
        Pos { row, col }
    }

    #[test]
    fn the_current_match_follows_its_text_when_rows_drift() {
        let m = |r| Match { start: at_pos(r, 0), end: at_pos(r, 1) };
        let mut r = Results { matches: vec![m(5), m(6), m(7)], current: Some(1) };
        // Scrollback is full: 3 lines dropped off the top, so every row moved up by 3.
        r.update(vec![m(2), m(3), m(4), m(8)], Some(at_pos(3, 0)));
        assert_eq!(r.current, Some(1));
        assert_eq!(r.label(true), "3/4");
    }

    #[test]
    fn rows_text_follows_the_window_and_wrap() {
        let mut s = Screen::with_scrollback(10, 4, 100);
        s.feed(b"abcdefghij");
        s.feed(b"klm\r\nxy|zw  \r\n");
        let history = 0usize;
        let rows = rows_text(&s, 0, 3, None);
        assert_eq!(rows.len(), 4);
        assert_eq!((rows[0].text.as_str(), rows[0].wraps), ("abcdefghij", true));
        assert_eq!((rows[1].text.as_str(), rows[1].wraps), ("klm", false));
        assert_eq!(rows[2].text, "xy|zw", "trailing blanks dropped");
        assert_eq!(rows[2].cells[3], (3, at(history + 2, 3)));
        let win = rows_text(&s, 2, 2, Some((3, 4)));
        assert_eq!((win[0].text.as_str(), win[0].cells.clone()), ("zw", vec![(0, at(2, 3)), (1, at(2, 4))]));
        let chars = cell_rows(&s, 1, 2);
        assert_eq!(chars.len(), 2);
        assert_eq!(chars[1][2], '|');
        assert_eq!(chars[1].len(), 10);
        assert_eq!(rows_text(&s, -5, 99, None).len(), 4, "clamped to the buffer");
    }

    #[test]
    fn find_over_a_large_buffer_is_quick() {
        let n = 100_000;
        let mut s = Screen::with_scrollback(80, 24, n);
        let mut text = String::new();
        for i in 0..n {
            text.push_str(&format!("line {i} {}\r\n", if i % 1000 == 0 { "needle" } else { "hay" }));
        }
        s.feed(text.as_bytes());
        let t = std::time::Instant::now();
        let hits = find_all(&s, "needle").unwrap();
        let took = t.elapsed();
        assert_eq!(hits.len(), 100);
        assert!(took < std::time::Duration::from_millis(500), "{took:?}");
    }
}

//! iTerm2's synthetic sessions: Filter shows only the lines that match, and Zoom In on
//! Selection shows only the selected lines. Both are read-only copies of the text.

use crate::find;
use crate::screen::{Look, Screen};

/// One logical line of a synthetic view: its plain text (what a filter matches and what a
/// repeat compares) and the same text with its SGR codes (what the view is fed).
#[derive(Clone, Debug, PartialEq)]
pub struct Styled {
    pub text: String,
    pub sgr: String,
}

/// Logical lines a chunked filter reads per step (P7).
pub const CHUNK: usize = 20_000;
/// Cells a step reads at most: one logical line can be a whole scrollback of wrapped rows.
pub const CHUNK_CELLS: usize = 8 * 1024 * 1024;

/// The styled line over grid lines `first..=last`: SGR text with no control characters of the
/// cells' own (`styled_copy::sgr_over_clean`, the writer Copy with Control Sequences uses).
fn styled_line(screen: &Screen, text: String, first: i32, last: i32) -> Styled {
    use alacritty_terminal::grid::Dimensions;
    let cols = screen.term().columns();
    let spans: Vec<(i32, usize, usize)> = (first..=last).map(|l| (l, 0, cols - 1)).collect();
    Styled { text, sgr: crate::styled_copy::sgr_over_clean(screen, &spans) }
}

/// The logical lines that match `pattern`, oldest first. An empty pattern keeps them all.
pub fn filter_lines(screen: &Screen, pattern: &str) -> Result<Vec<Styled>, String> {
    filter_lines_in(screen, pattern, None)
}

/// `filter_lines` over buffer rows `rows` only (the selected command's); None reads them all.
pub fn filter_lines_in(screen: &Screen, pattern: &str, rows: Option<(usize, usize)>) -> Result<Vec<Styled>, String> {
    let mut c = Chunked::start_in(screen, pattern, rows)?;
    loop {
        if let Some(lines) = c.step_with(screen, usize::MAX, usize::MAX) {
            return Ok(lines);
        }
    }
}

/// The logical lines over buffer rows `first..=last`.
pub fn zoom_lines(screen: &Screen, first: usize, last: usize) -> Vec<Styled> {
    find::lines_over(screen, first, last).into_iter().map(|(text, a, b)| styled_line(screen, text, a, b)).collect()
}

/// A filter that reads the buffer a chunk at a time, so a big scrollback never holds the event
/// loop (`CHUNK` logical lines or `CHUNK_CELLS` cells a step). Its place is a line number that
/// counts the lines the buffer dropped, so output arriving between steps moves nothing: the run
/// reads on to the buffer's end as it is at each step, which is its catch-up.
#[derive(Clone, Debug)]
pub struct Chunked {
    pattern: String,
    re: Option<regex::Regex>,
    /// The first line still to read, numbered as `dropped + history row`.
    start: i64,
    next: i64,
    /// The last line to read (numbered the same way); None reads to the buffer's end.
    last: Option<i64>,
    cols: usize,
    out: Vec<Styled>,
    progress: f32,
    /// Logical lines read so far (the work counter the tests watch).
    read: usize,
    /// Lines of the view that come before this run's first output line (a resumed run extends them).
    base: usize,
    /// Where the whole filter began (a resize reads from here again).
    origin: i64,
    /// The last logical line read (numbered like `next`) and how many output lines come before it:
    /// the place a later run resumes, since that line may still be growing.
    last_start: i64,
    keep: usize,
}

/// Where a finished run stopped, for the next run to resume (final review fix 3).
#[derive(Clone, Debug, PartialEq)]
pub struct Tail {
    pattern: String,
    origin: i64,
    start: i64,
    keep: usize,
    cols: usize,
}

impl Chunked {
    pub fn start(screen: &Screen, pattern: &str) -> Result<Chunked, String> {
        Chunked::start_in(screen, pattern, None)
    }

    /// A run over buffer rows `rows` (the selected command's), or all of them.
    pub fn start_in(screen: &Screen, pattern: &str, rows: Option<(usize, usize)>) -> Result<Chunked, String> {
        // The `regex` crate: linear time, so a pattern on program output cannot stall a step (B6).
        let re = regex::Regex::new(pattern).map_err(|e| e.to_string())?;
        use alacritty_terminal::grid::Dimensions;
        let dropped = screen.term().grid().dropped() as i64;
        let (start, last) = match rows {
            Some((first, last)) => (dropped + first as i64, Some(dropped + last as i64)),
            None => (dropped, None),
        };
        Ok(Chunked { pattern: pattern.to_string(), re: (!pattern.is_empty()).then_some(re), start, next: start, last, cols: screen.term().columns(), out: Vec::new(), progress: 0.0, read: 0, base: 0, origin: start, last_start: start, keep: 0 })
    }

    /// A run over the buffer's new end only: it starts at the line the last run stopped in and
    /// reads to the buffer's end. None when that no longer fits (the width changed, the line left
    /// the scrollback, another pattern): the caller starts a whole run then.
    pub fn resume(screen: &Screen, pattern: &str, tail: &Tail) -> Option<Chunked> {
        use alacritty_terminal::grid::Dimensions;
        if tail.pattern != pattern || tail.cols != screen.term().columns() || tail.start < screen.term().grid().dropped() as i64 {
            return None;
        }
        let mut c = Chunked::start_in(screen, pattern, None).ok()?;
        c.origin = tail.origin;
        c.start = tail.start;
        c.next = tail.start;
        c.last_start = tail.start;
        c.base = tail.keep;
        c.keep = tail.keep;
        Some(c)
    }

    /// Where this run stopped; None for a run over a command's rows only.
    pub fn tail(&self) -> Option<Tail> {
        self.last.is_none().then(|| Tail { pattern: self.pattern.clone(), origin: self.origin, start: self.last_start, keep: self.keep, cols: self.cols })
    }

    /// The view lines before this run's output: the finished lines are `shown[..base]` plus the
    /// result of the last `step`.
    pub fn base(&self) -> usize {
        self.base
    }

    /// Reads one chunk. None until the run is done, then every matching line.
    pub fn step(&mut self, screen: &Screen) -> Option<Vec<Styled>> {
        self.step_with(screen, CHUNK, CHUNK_CELLS)
    }

    fn step_with(&mut self, screen: &Screen, max_lines: usize, max_cells: usize) -> Option<Vec<Styled>> {
        use alacritty_terminal::grid::Dimensions;
        let grid = screen.term().grid();
        // A resize rewraps everything: read it again from the start.
        if grid.columns() != self.cols {
            self.cols = grid.columns();
            self.start = self.origin;
            self.next = self.origin;
            self.base = 0;
            self.out.clear();
        }
        let (dropped, history) = (grid.dropped() as i64, grid.history_size() as i64);
        let bottom = grid.screen_lines() as i32 - 1;
        let to_line = |n: i64| (n - dropped - history).clamp(-history, bottom as i64) as i32;
        let mut end = self.last.map_or(bottom, to_line);
        while end < bottom && find::wraps(screen, end) {
            end += 1;
        }
        let mut from = to_line(self.next.max(self.start));
        while from > -(history as i32) && find::wraps(screen, from - 1) {
            from -= 1;
        }
        let total = (end as i64 + history + 1 - to_line(self.start) as i64 - history).max(1);
        if from <= end {
            let stop = find::chunk_end(screen, from, end, max_lines, max_cells);
            for (text, a, b) in find::lines_between(screen, from, stop) {
                self.read += 1;
                (self.last_start, self.keep) = (a as i64 + dropped + history, self.base + self.out.len());
                if self.re.as_ref().is_none_or(|re| re.is_match(&text)) {
                    self.out.push(styled_line(screen, text, a, b));
                }
            }
            self.next = stop as i64 + 1 + dropped + history;
            let done = stop as i64 + 1 - to_line(self.start) as i64;
            self.progress = (done as f32 / total as f32).clamp(0.0, 1.0);
            if stop < end {
                return None;
            }
        }
        self.progress = 1.0;
        Some(std::mem::take(&mut self.out))
    }

    /// The pattern the run filters with.
    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    /// Logical lines this run has read.
    pub fn lines_read(&self) -> usize {
        self.read
    }

    /// How much of the buffer has been read, 0 to 1.
    pub fn progress(&self) -> f32 {
        self.progress
    }
}

/// The lines `new` adds after `old`, when `new` only grew at the end.
pub fn appended<'a>(old: &[Styled], new: &'a [Styled]) -> Option<&'a [Styled]> {
    (new.len() >= old.len() && new[..old.len()].iter().zip(old).all(|(a, b)| a.text == b.text)).then(|| &new[old.len()..])
}

/// A screen showing `lines`, scrolled to the bottom, with room for all of them.
pub fn view(lines: &[Styled], like: &Screen) -> Screen {
    use alacritty_terminal::grid::Dimensions;
    let (cols, rows) = (like.term().columns(), like.term().screen_lines());
    // Room for every line, and for Filter to add more as output arrives.
    let wrap = 1 + lines.iter().map(|l| l.text.chars().count()).max().unwrap_or(0) / cols.max(1);
    let look = Look { scrollback: lines.len() * wrap + rows + 10_000, ..Look::default() };
    let mut s = Screen::with_look(cols, rows, like.palette.clone(), look);
    // No cursor: the view is read-only.
    s.feed(b"\x1b[?25l");
    s.feed(lines.iter().map(|l| l.sgr.as_str()).collect::<Vec<_>>().join("\r\n").as_bytes());
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::index::{Column, Line};
    use alacritty_terminal::term::cell::Flags;
    use alacritty_terminal::vte::ansi::{Color, NamedColor};

    fn screen(text: &str, scrollback: usize) -> Screen {
        let mut s = Screen::with_scrollback(20, 4, scrollback);
        s.feed(text.replace('\n', "\r\n").as_bytes());
        s
    }

    fn texts(lines: &[Styled]) -> Vec<&str> {
        lines.iter().map(|l| l.text.as_str()).collect()
    }

    fn styled(xs: &[&str]) -> Vec<Styled> {
        xs.iter().map(|x| Styled { text: x.to_string(), sgr: x.to_string() }).collect()
    }

    #[test]
    fn filter_keeps_whole_matching_lines() {
        let s = screen("apple pie\nbanana\n..................apple wraps\ncherry", 100);
        assert_eq!(texts(&filter_lines(&s, "apple").unwrap()), vec!["apple pie", "..................apple wraps"]);
        assert_eq!(filter_lines(&s, "").unwrap().len(), 4);
        assert!(filter_lines(&s, "(").is_err());
    }

    #[test]
    fn filtering_a_large_buffer_is_quick() {
        let n = 100_000;
        let mut s = Screen::with_scrollback(80, 24, n);
        let mut text = String::new();
        for i in 0..n {
            text.push_str(&format!("line {i} {}\r\n", if i % 1000 == 0 { "needle" } else { "hay" }));
        }
        s.feed(text.as_bytes());
        let t = std::time::Instant::now();
        let hits = filter_lines(&s, "needle").unwrap();
        let took = t.elapsed();
        assert_eq!(hits.len(), 100);
        assert!(took < std::time::Duration::from_millis(500), "{took:?}");
    }

    #[test]
    fn zoom_copies_the_selected_lines() {
        let s = screen("one\ntwo\nthree\nfour", 100);
        assert_eq!(texts(&zoom_lines(&s, 1, 2)), vec!["two", "three"]);
        let v = view(&zoom_lines(&s, 1, 2), &s);
        assert_eq!(v.all_text(), "two\nthree\n");
        assert_eq!(v.term().grid().display_offset(), 0);
        let many: Vec<Styled> = (0..50).map(|i| Styled { text: format!("l{i}"), sgr: format!("l{i}") }).collect();
        let v = view(&many, &s);
        assert!(v.all_text().starts_with("l0\nl1\n"), "the view keeps every line");
    }

    #[test]
    fn new_output_appends_to_the_filter_view() {
        let old = styled(&["a1", "a2"]);
        assert_eq!(appended(&old, &styled(&["a1", "a2", "a3"])), Some(&styled(&["a3"])[..]));
        assert_eq!(appended(&old, &styled(&["a1", "a2"])), Some(&[][..]));
        assert_eq!(appended(&old, &styled(&["a2", "a3"])), None);
        assert_eq!(appended(&old, &styled(&["a1"])), None);
        // The comparison is of text: a line whose colors changed is still the same line.
        let recolored = vec![Styled { text: "a1".into(), sgr: "\x1b[31ma1".into() }, Styled { text: "a2".into(), sgr: "a2".into() }];
        assert_eq!(appended(&old, &recolored), Some(&[][..]));
    }

    /// Review Focus 9: Filter copies the screen's characters with their attributes.
    #[test]
    fn filtered_lines_keep_colors() {
        let s = screen("\x1b[31mred apple\x1b[0m\nplain", 100);
        let hits = filter_lines(&s, "apple").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text, "red apple");
        assert!(hits[0].sgr.contains("31"), "{:?}", hits[0].sgr);
        let v = view(&hits, &s);
        let cell = &v.term().grid()[Line(0)][Column(0)];
        assert_eq!(cell.c, 'r');
        assert_eq!(cell.fg, Color::Named(NamedColor::Red));
        assert_eq!(v.all_text(), "red apple\n");
    }

    #[test]
    fn zoom_keeps_colors() {
        let s = screen("one\n\x1b[1mtwo bold\x1b[0m\nthree", 100);
        let lines = zoom_lines(&s, 1, 1);
        assert_eq!(texts(&lines), vec!["two bold"]);
        let v = view(&lines, &s);
        assert!(v.term().grid()[Line(0)][Column(0)].flags.contains(Flags::BOLD));
        assert!(!v.term().grid()[Line(1)][Column(0)].flags.contains(Flags::BOLD));
    }

    /// The text of a filtered line is not a way to write to the view: a control character in a
    /// cell never reaches the parser (P7).
    #[test]
    fn a_cell_cannot_inject_a_sequence() {
        let mut s = Screen::with_scrollback(30, 4, 100);
        s.feed(b"keep\r\nmark\r\n");
        for (i, c) in ['\u{1b}', '[', '3', '1', 'm', '\u{9b}', '\u{7f}', '\u{7}'].into_iter().enumerate() {
            s.term_mut().grid_mut()[Line(1)][Column(4 + i)].c = c;
        }
        let hits = filter_lines(&s, "mark").unwrap();
        assert_eq!(hits.len(), 1, "{:?}", filter_lines(&s, "").unwrap());
        assert!(!hits[0].sgr.chars().any(|c| c.is_control() && c != '\u{1b}'), "{:?}", hits[0].sgr);
        let v = view(&hits, &s);
        // The cell's own "[31m" is text now, not a sequence: nothing turned red.
        assert_eq!(hits[0].sgr.matches("\u{1b}[31m").count(), 0, "{:?}", hits[0].sgr);
        assert!(v.all_text().contains("[31m"), "{:?}", v.all_text());
        assert_ne!(v.term().grid()[Line(0)][Column(4)].fg, Color::Named(NamedColor::Red));
    }

    fn big(n: usize) -> Screen {
        let mut s = Screen::with_scrollback(40, 24, n + 100);
        let mut text = String::new();
        for i in 0..n {
            text.push_str(&format!("line {i}{}\r\n", if i % 10_000 == 5 { " needle" } else { "" }));
        }
        s.feed(text.as_bytes());
        s
    }

    #[test]
    fn chunked_filter_reports_progress() {
        let s = big(50_000);
        let mut c = Chunked::start(&s, "needle").unwrap();
        assert!(c.step(&s).is_none());
        assert!((c.progress() - 0.4).abs() < 0.02, "{}", c.progress());
        assert!(c.step(&s).is_none());
        assert!((c.progress() - 0.8).abs() < 0.02, "{}", c.progress());
        let hits = c.step(&s).expect("the third chunk ends the run");
        assert_eq!(texts(&hits), ["line 5 needle", "line 10005 needle", "line 20005 needle", "line 30005 needle", "line 40005 needle"]);
        assert_eq!(c.progress(), 1.0);
        // A small buffer finishes at once.
        let small = big(100);
        let mut c = Chunked::start(&small, "needle").unwrap();
        assert_eq!(texts(&c.step(&small).unwrap()), ["line 5 needle"]);
        assert!(Chunked::start(&small, "(").is_err());
    }

    /// New output while a run is under way is read by the run itself; the lines it adds are not
    /// lost and the run still ends (P7).
    #[test]
    fn chunked_filter_reads_output_that_arrives_mid_run() {
        let mut s = big(30_000);
        let mut c = Chunked::start(&s, "needle").unwrap();
        assert!(c.step(&s).is_none());
        s.feed(b"fresh needle one\r\nfresh needle two\r\n");
        let hits = c.step(&s).expect("done");
        let t = texts(&hits);
        assert_eq!(t[0], "line 5 needle");
        assert!(t.contains(&"fresh needle one") && t.contains(&"fresh needle two"), "{t:?}");
        // The same lines as one pass over the final buffer.
        assert_eq!(hits, filter_lines(&s, "needle").unwrap());
    }
}

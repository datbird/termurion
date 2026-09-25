//! A porthole's record: its text, how it was detected, and the grid rows it replaced (put
//! back when it closes). The rows it stands on are blank and tagged with its id.

use std::collections::HashSet;

use alacritty_terminal::grid::{Dimensions, Row};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::TermMode;

use super::detect::{self, Detected};
use crate::screen::Screen;

/// Where a porthole's text came from: the type and file name given, and Wide asked for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Source {
    pub hint: Option<String>,
    pub filename: Option<String>,
    pub wide: bool,
}

#[derive(Clone, Debug)]
pub struct Porthole {
    pub id: u32,
    pub text: String,
    pub detected: Detected,
    pub filename: Option<String>,
    /// The shell's folder when the porthole was made (relative markdown links).
    pub base_dir: Option<String>,
    pub wide: bool,
    /// Rows it stands on.
    pub rows: u32,
    /// The rows it replaced, put back on close.
    pub saved: Vec<Row<Cell>>,
    /// Its rows were counted with the window's fonts (not the terminal's estimate).
    pub sized: bool,
}

/// Rows a laid-out document needs: its height, the margins, and the control strip.
pub fn rows_needed(height: f32, cell_h: f32) -> u32 {
    ((height + 2.0 * crate::scene::PORTHOLE_MARGIN + cell_h) / cell_h.max(1.0)).ceil().max(1.0) as u32
}

/// The rows a document needs, measured as if every face were the terminal's font (the
/// window measures again with its fonts when it first draws the porthole).
pub fn estimate_rows(p: &Porthole, palette: &crate::palette::Palette, dark: bool, cols: usize, cell: (f32, f32)) -> u32 {
    use super::render::{self, Face};
    use unicode_width::UnicodeWidthStr;
    let lang = p.detected.language.clone().unwrap_or(detect::Language::Plain);
    let doc = render::render(&p.text, &lang, palette, dark);
    let measure = |t: &str, _: Face, s: f32| t.width() as f32 * cell.0 * s;
    let width = (cols as f32 * cell.0 - 2.0 * crate::scene::PORTHOLE_MARGIN).max(cell.0);
    rows_needed(render::layout(&doc, width, cell.1, &measure, p.wide).height, cell.1)
}

fn blank_rows(cols: usize, n: u32, id: u32) -> Vec<Row<Cell>> {
    (0..n.max(1))
        .map(|_| {
            let mut r: Row<Cell> = Row::new(cols);
            r.set_porthole(id);
            r
        })
        .collect()
}

impl Screen {
    /// Replaces the main screen's lines `first..=last` with a porthole of their text. The
    /// cursor, when inside, first moves to the line below. `rows_for` says how many rows the
    /// rendered document needs. None on the alternate screen or for lines out of the buffer.
    pub fn render_range(&mut self, first: i32, last: i32, source: Source, rows_for: &dyn Fn(&Porthole) -> u32) -> Option<u32> {
        use alacritty_terminal::vte::ansi::Handler;
        if self.term.mode().contains(TermMode::ALT_SCREEN) {
            return None;
        }
        let (mut first, mut last) = (first, last);
        let in_buffer = |s: &Screen, a: i32, b: i32| {
            let g = s.term.primary_grid();
            a <= b && a >= -(g.history_size() as i32) && b < g.screen_lines() as i32
        };
        if !in_buffer(self, first, last) {
            return None;
        }
        // The cursor goes below the range; a line feed at the bottom scrolls it up.
        let mut steps = 0;
        while (first..=last).contains(&self.term.grid().cursor.point.line.0) && steps <= last - first + 1 {
            let before = self.term.grid().cursor.point.line.0;
            self.term.linefeed();
            self.term.carriage_return();
            if self.term.grid().cursor.point.line.0 == before {
                first -= 1;
                last -= 1;
            }
            steps += 1;
        }
        if !in_buffer(self, first, last) {
            return None;
        }
        let text = {
            let g = self.term.primary_grid();
            let mut lines = Vec::new();
            let mut line = String::new();
            for l in first..=last {
                let row = &g[Line(l)];
                line.extend((0..g.columns()).map(|c| &row[Column(c)]).filter(|c| !c.flags.contains(Flags::WIDE_CHAR_SPACER)).map(|c| c.c));
                if !(l < last && row[Column(g.columns() - 1)].flags.contains(Flags::WRAPLINE)) {
                    lines.push(line.trim_end().to_string());
                    line.clear();
                }
            }
            lines.join("\n")
        };
        self.next_porthole += 1;
        let id = self.next_porthole;
        let detected = detect::detect(&text, source.hint.as_deref(), source.filename.as_deref());
        let mut p = Porthole { id, text, detected, filename: source.filename, base_dir: self.shell.cwd.clone(), wide: source.wide, rows: 0, saved: Vec::new(), sized: false };
        p.rows = rows_for(&p).max(1);
        let mut rows = blank_rows(self.term.columns(), p.rows, id);
        // A rendered block's rows still belong to it (hover, copy, fold).
        let block = self.term.primary_grid()[Line(first)].block();
        for r in rows.iter_mut() {
            r.set_block(block);
        }
        p.saved = self.splice_rows(first, last, rows);
        self.portholes.insert(id, p);
        Some(id)
    }

    /// `render_range` with the terminal's own estimate of the rows (output-made portholes).
    pub fn render_range_estimated(&mut self, first: i32, last: i32, source: Source) -> Option<u32> {
        let (palette, dark, cols) = (self.palette.clone(), self.is_dark(), self.term.columns());
        let (cw, ch) = self.cell_px();
        let cell = (cw.max(1) as f32, ch.max(1) as f32);
        self.render_range(first, last, source, &|p| estimate_rows(p, &palette, dark, cols, cell))
    }

    /// Selects text in a porthole (one selection at a time: the terminal's goes).
    pub fn select_in_porthole(&mut self, id: u32, anchor: super::render::Pos, head: super::render::Pos) {
        self.term.selection = None;
        self.porthole_selection = Some((id, anchor, head));
    }

    pub fn clear_porthole_selection(&mut self) {
        self.porthole_selection = None;
    }

    /// The grid lines a porthole stands on.
    pub fn porthole_rows(&self, id: u32) -> Option<(i32, i32)> {
        let g = self.term.primary_grid();
        let (top, bottom) = (-(g.history_size() as i32), g.screen_lines() as i32 - 1);
        let first = (top..=bottom).find(|l| g[Line(*l)].porthole() == id)?;
        let mut last = first;
        while last < bottom && g[Line(last + 1)].porthole() == id {
            last += 1;
        }
        Some((first, last))
    }

    /// Close: the saved rows come back (at the current width) in the porthole's place.
    pub fn close_porthole(&mut self, id: u32) -> bool {
        if self.term.mode().contains(TermMode::ALT_SCREEN) {
            return false;
        }
        let Some((first, last)) = self.porthole_rows(id) else { return false };
        let Some(p) = self.portholes.remove(&id) else { return false };
        self.splice_rows(first, last, p.saved);
        true
    }

    /// A new height (a language change, Wide).
    pub fn set_porthole_rows(&mut self, id: u32, n: u32) -> bool {
        if self.term.mode().contains(TermMode::ALT_SCREEN) || !self.portholes.contains_key(&id) {
            return false;
        }
        let Some((first, last)) = self.porthole_rows(id) else { return false };
        let rows = blank_rows(self.term.columns(), n, id);
        self.splice_rows(first, last, rows);
        if let Some(p) = self.portholes.get_mut(&id) {
            p.rows = n.max(1);
        }
        true
    }

    /// Drops portholes whose rows left the buffer (rows a fold keeps still count).
    pub(crate) fn prune_portholes(&mut self) {
        let g = self.term.primary_grid();
        let top = -(g.history_size() as i32);
        let mut live: HashSet<u32> = (top..g.screen_lines() as i32).map(|l| g[Line(l)].porthole()).collect();
        live.extend(self.folds.values().flatten().map(|r| r.porthole()));
        self.portholes.retain(|id, _| live.contains(id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::Screen;
    use alacritty_terminal::grid::Dimensions;

    fn two_rows(_: &Porthole) -> u32 {
        2
    }

    #[test]
    fn render_range_reserves_tagged_rows() {
        let mut s = Screen::new(20, 8);
        s.feed(b"# Title\r\nbody  \r\nmore\r\n$ ");
        let id = s.render_range(0, 2, Source::default(), &two_rows).expect("a porthole");
        let p = &s.portholes[&id];
        assert_eq!(p.text, "# Title\nbody\nmore", "rows' text, trailing spaces trimmed, no last newline");
        assert_eq!(p.detected.language, Some(crate::porthole::detect::Language::Markdown));
        assert_eq!(p.rows, 2);
        assert_eq!(s.porthole_rows(id), Some((1, 2)), "the cursor row stays; rows above slide");
        assert_eq!(s.line_text(1).trim(), "", "blank rows");
        assert_eq!(s.line_text(3).trim_end(), "$");
        assert!(s.set_porthole_rows(id, 4));
        let (first, last) = s.porthole_rows(id).unwrap();
        assert_eq!((last, last - first), (2, 3));
        assert_eq!(s.portholes[&id].rows, 4);
    }

    #[test]
    fn cursor_moves_below() {
        let mut s = Screen::new(20, 8);
        s.feed(b"a\r\nb");
        let id = s.render_range(0, 1, Source::default(), &two_rows).unwrap();
        assert_eq!(s.portholes[&id].text, "a\nb");
        let (first, last) = s.porthole_rows(id).unwrap();
        let cursor = s.term().grid().cursor.point;
        assert!(cursor.line.0 > last && cursor.column.0 == 0, "cursor at {cursor:?}, porthole {first}..{last}");
    }

    #[test]
    fn close_restores_rows_at_the_new_width() {
        let mut s = Screen::with_scrollback(10, 6, 100);
        s.feed(b"abcdefghij\r\nk\r\n$ ");
        let id = s.render_range(0, 1, Source::default(), &two_rows).unwrap();
        s.resize(5, 6);
        let cursor = s.term().grid().cursor.point.line;
        assert!(s.close_porthole(id));
        assert!(s.portholes.is_empty());
        assert_eq!(s.term().grid().cursor.point.line, cursor, "the cursor keeps its row");
        let (top, bottom) = (-(s.term().grid().history_size() as i32), 5);
        let text: Vec<String> = (top..=bottom).map(|l| s.line_text(l).trim_end().to_string()).collect();
        assert!(text.iter().any(|t| t == "abcdefghij"), "re-wrapped at the new width: {text:?}");
        assert!(!s.close_porthole(id), "closed once");
    }

    #[test]
    fn portholes_pruned_with_scrollback() {
        let mut s = Screen::with_scrollback(20, 4, 3);
        s.feed(b"x\r\n");
        let id = s.render_range(0, 0, Source::default(), &two_rows).unwrap();
        for i in 0..20 {
            s.feed(format!("line {i}\r\n").as_bytes());
        }
        s.prune_tags();
        assert!(s.portholes.is_empty());
        assert!(!s.close_porthole(id));
        assert_eq!(s.porthole_rows(id), None);
    }

    #[test]
    fn folds_carry_portholes() {
        let mut s = Screen::with_scrollback(20, 8, 100);
        s.feed(b"p\r\nq\r\nr\r\n$ ");
        let id = s.render_range(1, 1, Source::default(), &two_rows).unwrap();
        let (a, b) = s.porthole_rows(id).unwrap();
        let fold = s.fold(a, b + 1).expect("folded");
        assert_eq!(s.porthole_rows(id), None);
        s.prune_tags();
        assert!(s.portholes.contains_key(&id), "a folded porthole is kept");
        assert!(s.unfold(fold));
        assert_eq!(s.porthole_rows(id), Some((a, b)));
    }

    #[test]
    fn alt_screen_refuses() {
        let mut s = Screen::new(20, 5);
        s.feed(b"a\r\n\x1b[?1049hb");
        assert_eq!(s.render_range(0, 0, Source::default(), &two_rows), None);
    }
}

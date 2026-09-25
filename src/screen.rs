//! Runs app output through the terminal core and collects the replies the app expects.

use std::sync::mpsc::{self, Receiver, Sender};

use alacritty_terminal::Term;
use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Row};
use alacritty_terminal::term::cell::Cell;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::{Config, TermMode};
use alacritty_terminal::vte::ansi::{ClearMode, CursorShape, CursorStyle, Handler, Processor};

use crate::mouse::{Modes, Tracking};
use crate::palette::Palette;

#[derive(Clone)]
pub struct Listener(Sender<Event>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let _ = self.0.send(event);
    }
}

struct GridSize {
    cols: usize,
    rows: usize,
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

/// The profile settings the terminal core needs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub cursor_shape: CursorShape,
    pub cursor_blink: bool,
    pub scrollback: usize,
}

impl Look {
    pub fn from_profile(p: &crate::profile::Profile) -> Look {
        let cursor_shape = match p.int("Cursor Type") {
            0 => CursorShape::Underline,
            1 => CursorShape::Beam,
            _ => CursorShape::Block,
        };
        // alacritty_terminal keeps at most 100,000 lines; that is Tandem's "unlimited".
        let scrollback = if p.bool("Unlimited Scrollback") { 100_000 } else { p.int("Scrollback Lines").clamp(0, 100_000) as usize };
        Look { cursor_shape, cursor_blink: p.bool("Blinking Cursor"), scrollback }
    }
}

impl Default for Look {
    fn default() -> Self {
        Look::from_profile(&crate::profile::Profile::stock())
    }
}

fn config(look: Look) -> Config {
    Config {
        scrolling_history: look.scrollback,
        default_cursor_style: CursorStyle { shape: look.cursor_shape, blinking: look.cursor_blink },
        ..Config::default()
    }
}

pub struct Screen {
    pub(crate) term: Term<Listener>,
    parser: Processor,
    events: Receiver<Event>,
    pub palette: Palette,
    title: Option<String>,
    cell_px: (u16, u16),
    osc: crate::osc::Scanner,
    pub marks: crate::marks::Marks,
    pub shell: crate::marks::Shell,
    /// The profile's Place Prompt at First Column.
    pub place_prompt_first_column: bool,
    /// Output comes through ConPTY, whose own screen must match this one, so the terminal
    /// never moves the cursor on its own (no fresh line for a prompt).
    pub pty_backed: bool,
    /// A fixed clock for row stamps (tests); None uses the time.
    clock: Option<u32>,
    /// Inline images, by id.
    pub images: crate::images::Store,
    /// Files the terminal was sent to save; the app asks before writing them.
    pub downloads: Vec<crate::inline_file::Download>,
    /// A MultipartFile in progress.
    pub multipart: Option<crate::inline_file::Multipart>,
    /// Kitty graphics images and transfers.
    pub kitty: crate::kitty::Kitty,
    /// Answers the terminal owes the program (kitty graphics), sent with `feed`'s reply.
    pub replies: Vec<u8>,
    /// Annotations, by id.
    pub notes: crate::annotations::Notes,
    /// Folded lines, by fold id.
    folds: std::collections::HashMap<u32, Vec<Row<Cell>>>,
    next_fold: u32,
    feeds: u32,
}

impl Screen {
    pub fn new(cols: usize, rows: usize) -> Self {
        Screen::with_look(cols, rows, Palette::default(), Look::default())
    }

    pub fn with_look(cols: usize, rows: usize, palette: Palette, look: Look) -> Self {
        let (tx, events) = mpsc::channel();
        let term = Term::new(config(look), &GridSize { cols, rows }, Listener(tx));
        Self {
            term,
            parser: Processor::new(),
            events,
            palette,
            title: None,
            cell_px: (0, 0),
            osc: crate::osc::Scanner::default(),
            marks: crate::marks::Marks::default(),
            shell: crate::marks::Shell::default(),
            place_prompt_first_column: true,
            pty_backed: false,
            clock: None,
            images: crate::images::Store::default(),
            downloads: Vec::new(),
            multipart: None,
            kitty: crate::kitty::Kitty::default(),
            replies: Vec::new(),
            notes: crate::annotations::Notes::default(),
            folds: std::collections::HashMap::new(),
            next_fold: 0,
            feeds: 0,
        }
    }

    /// A screen with `lines` of scrollback.
    pub fn with_scrollback(cols: usize, rows: usize, lines: usize) -> Self {
        Screen::with_look(cols, rows, Palette::default(), Look { scrollback: lines, ..Look::default() })
    }

    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
    }

    pub fn set_look(&mut self, look: Look) {
        self.term.set_options(config(look));
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.term.resize(GridSize { cols, rows });
    }

    pub fn term(&self) -> &Term<Listener> {
        &self.term
    }

    pub fn term_mut(&mut self) -> &mut Term<Listener> {
        &mut self.term
    }

    pub fn set_cell_px(&mut self, w: u16, h: u16) {
        self.cell_px = (w, h);
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Feeds app output. Returns the bytes the terminal must send back, such as
    /// answers to device, color, and size queries. An app that gets no answer may hang.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        // Shell-integration OSCs act where they appear in the stream, so the output is fed in
        // pieces that end at each one.
        let events = self.osc.scan(bytes);
        let mut reply = Vec::new();
        let mut at = 0;
        for (end, osc) in events {
            reply.extend(self.feed_raw(&bytes[at..end]));
            at = end;
            self.apply(osc);
            reply.append(&mut self.replies);
        }
        reply.extend(self.feed_raw(&bytes[at..]));
        reply
    }

    /// The cell size in pixels, as the app set it.
    pub fn cell_px(&self) -> (u16, u16) {
        self.cell_px
    }

    /// Fixes the clock that stamps written rows (seconds), for tests; None uses the time.
    pub fn set_clock(&mut self, secs: Option<u32>) {
        self.clock = secs;
    }

    /// When a row (grid line) was last written, in Unix seconds; 0 for never.
    pub fn row_time(&self, line: i32) -> u32 {
        let grid = self.term.grid();
        if line < -(grid.history_size() as i32) || line >= grid.screen_lines() as i32 {
            return 0;
        }
        grid[Line(line)].time()
    }

    /// Replaces the main screen's rows `first..=last` (grid lines) with `with`, returning
    /// the removed rows, top first. Rows above slide down when fewer come back.
    pub fn splice_rows(&mut self, first: i32, last: i32, with: Vec<Row<Cell>>) -> Vec<Row<Cell>> {
        // Edits go to the main screen, whose rows a full-screen app's screen hides.
        if self.alt_screen() {
            return Vec::new();
        }
        self.term.selection = None;
        self.term.primary_grid_mut().splice(Line(first), Line(last), with)
    }

    /// Rows holding `lines` as plain text; a line longer than the width wraps onto more rows.
    pub fn text_rows(&self, lines: &[String]) -> Vec<Row<Cell>> {
        use alacritty_terminal::term::cell::Flags;
        use unicode_width::UnicodeWidthChar;
        let cols = self.term.columns();
        let mut out = Vec::new();
        for l in lines {
            let mut row: Row<Cell> = Row::new(cols);
            let mut c = 0;
            for ch in l.chars() {
                let w = ch.width().unwrap_or(0);
                if w == 0 || w > cols {
                    continue;
                }
                if c + w > cols {
                    row[Column(cols - 1)].flags.insert(Flags::WRAPLINE);
                    out.push(std::mem::replace(&mut row, Row::new(cols)));
                    c = 0;
                }
                row[Column(c)].c = ch;
                if w == 2 {
                    row[Column(c)].flags.insert(Flags::WIDE_CHAR);
                    row[Column(c + 1)].flags.insert(Flags::WIDE_CHAR_SPACER);
                }
                c += w;
            }
            out.push(row);
        }
        out
    }

    /// Folds grid lines `first..=last` of the main screen into one summary line. None
    /// when the alternate screen is up or the range holds the cursor.
    pub fn fold(&mut self, first: i32, last: i32) -> Option<u32> {
        self.fold_as(first, last, None)
    }

    /// `fold` with a caption of its own for the summary line (Fold All's).
    pub fn fold_as(&mut self, first: i32, last: i32, caption: Option<String>) -> Option<u32> {
        use alacritty_terminal::term::cell::Flags;
        let grid = self.term.primary_grid();
        let (top, bottom) = (-(grid.history_size() as i32), grid.screen_lines() as i32 - 1);
        let cursor = grid.cursor.point.line.0;
        if self.term.mode().contains(TermMode::ALT_SCREEN) || first > last || first < top || last > bottom || (first..=last).contains(&cursor) {
            return None;
        }
        let text = |l: i32| (0..grid.columns()).map(|c| grid[Line(l)][Column(c)].c).collect::<String>();
        let summary = caption.unwrap_or_else(|| crate::fold::summary(&text(first), &text(last), (last - first + 1) as usize, grid.columns()));
        self.next_fold += 1;
        let id = self.next_fold;
        let mut rows = self.text_rows(&[summary]);
        rows.truncate(1);
        for row in rows.iter_mut() {
            for c in 0..row.len() {
                row[Column(c)].flags.insert(Flags::DIM | Flags::ITALIC);
            }
            row.set_fold(id);
        }
        let removed = self.splice_rows(first, last, rows);
        self.folds.insert(id, removed);
        Some(id)
    }

    /// Puts a fold's lines back. False when no row stands for it.
    pub fn unfold(&mut self, id: u32) -> bool {
        use alacritty_terminal::term::cell::Flags;
        if self.alt_screen() {
            return false;
        }
        let grid = self.term.primary_grid();
        let (top, bottom) = (-(grid.history_size() as i32), grid.screen_lines() as i32 - 1);
        let Some(line) = (top..=bottom).find(|l| grid[Line(*l)].fold() == id) else { return false };
        // A narrower pane may have wrapped the summary line onto more rows.
        let mut last = line;
        while last < bottom && grid[Line(last)][Column(grid.columns() - 1)].flags.contains(Flags::WRAPLINE) {
            last += 1;
        }
        let Some(rows) = self.folds.remove(&id) else { return false };
        self.splice_rows(line, last, rows);
        true
    }

    /// How many folds keep lines.
    pub fn fold_count(&self) -> usize {
        self.folds.len()
    }

    /// Drops folds and notes whose rows left the buffer (cleared, or trimmed off the top of
    /// the scrollback), so their saved lines do not stay in memory.
    pub fn prune_tags(&mut self) {
        let grid = self.term.primary_grid();
        let top = -(grid.history_size() as i32);
        let mut folds = std::collections::HashSet::new();
        let mut notes = std::collections::HashSet::new();
        for l in top..grid.screen_lines() as i32 {
            let row = &grid[Line(l)];
            folds.insert(row.fold());
            notes.insert(row.note());
        }
        self.folds.retain(|id, _| folds.contains(id));
        self.notes.by_id.retain(|_, n| notes.contains(&n.row_key));
        self.prune_images();
    }

    fn alt_screen(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    /// The fold a row stands for.
    pub fn fold_at(&self, line: i32) -> Option<u32> {
        if self.alt_screen() {
            return None;
        }
        let grid = self.term.primary_grid();
        if line < -(grid.history_size() as i32) || line >= grid.screen_lines() as i32 {
            return None;
        }
        Some(grid[Line(line)].fold()).filter(|id| *id != 0)
    }

    pub(crate) fn feed_raw(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.term.stamp = self.clock.unwrap_or_else(|| {
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as u32)
        });
        self.parser.advance(&mut self.term, bytes);
        // Now and then, forget folds and notes whose rows scrolled out of the buffer.
        self.feeds = self.feeds.wrapping_add(1);
        if self.feeds % 256 == 0 && (!self.folds.is_empty() || !self.notes.by_id.is_empty() || !self.images.by_id.is_empty()) {
            self.prune_tags();
        }
        let mut reply = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::PtyWrite(text) => reply.extend_from_slice(text.as_bytes()),
                Event::ColorRequest(index, answer) => {
                    let color = self.term.colors()[index].unwrap_or_else(|| match index {
                        0..=255 => self.palette.indexed(index as u8),
                        257 => self.palette.bg,
                        258 => self.palette.cursor,
                        _ => self.palette.fg,
                    });
                    reply.extend_from_slice(answer(color).as_bytes());
                }
                Event::TextAreaSizeRequest(answer) => {
                    let size = WindowSize {
                        num_lines: self.term.screen_lines() as u16,
                        num_cols: self.term.columns() as u16,
                        cell_width: self.cell_px.0,
                        cell_height: self.cell_px.1,
                    };
                    reply.extend_from_slice(answer(size).as_bytes());
                }
                Event::Title(title) => self.title = Some(title),
                Event::ResetTitle => self.title = None,
                _ => {}
            }
        }
        reply
    }

    /// Puts a mark id on a row (grid line: 0 is the top of the screen, negative is history).
    pub fn set_row_mark(&mut self, line: i32, id: u32) {
        let grid = self.term.primary_grid_mut();
        if line >= -(grid.history_size() as i32) && line < grid.screen_lines() as i32 {
            grid[Line(line)].set_mark(id);
        }
    }

    /// Puts a manual mark id on a row, beside any prompt mark there.
    pub fn set_row_manual(&mut self, line: i32, id: u32) {
        let grid = self.term.primary_grid_mut();
        if line >= -(grid.history_size() as i32) && line < grid.screen_lines() as i32 {
            grid[Line(line)].set_manual(id);
        }
    }

    /// Every mark on a row, top first: (grid line, mark id). A row's prompt mark comes
    /// before its manual mark.
    pub fn row_marks(&self) -> Vec<(i32, u32)> {
        // Marks live on the main screen, also while a full-screen app has the alternate one.
        let grid = self.term.primary_grid();
        let top = -(grid.history_size() as i32);
        (top..grid.screen_lines() as i32)
            .flat_map(|l| {
                let row = &grid[Line(l)];
                [(l, row.mark()), (l, row.manual())]
            })
            .filter(|(_, id)| *id != 0)
            .collect()
    }

    /// One logical line's text starting at `line`: its row and the rows it wraps onto,
    /// trailing spaces trimmed.
    pub fn line_text(&self, line: i32) -> String {
        let grid = self.term.grid();
        let mut out = String::new();
        let mut l = line;
        while l < grid.screen_lines() as i32 {
            let row = &grid[Line(l)];
            out.extend((0..grid.columns()).map(|c| row[Column(c)].c));
            let wrapped = row[Column(grid.columns() - 1)].flags.contains(alacritty_terminal::term::cell::Flags::WRAPLINE);
            if !wrapped {
                break;
            }
            l += 1;
        }
        out.trim_end().to_string()
    }

    /// The whole buffer as text, oldest line first, trailing spaces trimmed.
    pub fn all_text(&self) -> String {
        let grid = self.term.grid();
        let top = -(grid.history_size() as i32);
        let mut out = String::new();
        for line in top..grid.screen_lines() as i32 {
            let row = &grid[Line(line)];
            let text: String = (0..grid.columns()).map(|c| row[Column(c)].c).collect();
            out.push_str(text.trim_end());
            out.push('\n');
        }
        let trimmed = out.trim_end_matches('\n').len();
        out.truncate(trimmed);
        out.push('\n');
        out
    }

    /// iTerm2's Clear Scrollback Buffer (`scrollback_only`) or Clear Buffer (everything but
    /// the cursor line, which moves to the top, as iTerm2 keeps the prompt).
    pub fn clear(&mut self, scrollback_only: bool) {
        // Scrolling and clearing push lines into scrollback, so the screen goes first,
        // then the scrollback.
        if !scrollback_only {
            let cursor = self.term.grid().cursor.point;
            if cursor.line.0 > 0 {
                self.term.scroll_up(cursor.line.0 as usize);
            }
            if self.term.screen_lines() > 1 {
                self.term.goto(1, 0);
                self.term.clear_screen(ClearMode::Below);
            }
            self.term.goto(0, cursor.column.0);
        }
        self.term.clear_screen(ClearMode::Saved);
        self.prune_tags();
    }

    /// iTerm2's Reset (a soft reset, DECSTR): modes, attributes, margins, and charset go back
    /// to their defaults and the text stays. alacritty has no DECSTR, so the equivalent
    /// sequences are fed in.
    pub fn soft_reset(&mut self) {
        const DECSTR: &[u8] = b"\x1b[?25h\x1b[?1l\x1b>\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1005l\x1b[?1006l\
            \x1b[?2004l\x1b[?1004l\x1b[4l\x1b[0m\x1b[r\x1b[?6l\x1b[?7h\x1b(B";
        self.feed(DECSTR);
    }

    pub fn mouse_modes(&self) -> Modes {
        let mode = self.term.mode();
        let tracking = if mode.contains(TermMode::MOUSE_MOTION) {
            Tracking::Any
        } else if mode.contains(TermMode::MOUSE_DRAG) {
            Tracking::Drag
        } else if mode.contains(TermMode::MOUSE_REPORT_CLICK) {
            Tracking::Click
        } else {
            Tracking::Off
        };
        Modes { tracking, sgr: mode.contains(TermMode::SGR_MOUSE) }
    }

    pub fn text(&self) -> String {
        let grid = self.term.grid();
        let mut out = String::new();
        for row in 0..grid.screen_lines() {
            for col in 0..grid.columns() {
                out.push(grid[Line(row as i32)][Column(col)].c);
            }
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_background_color_query() {
        let mut s = Screen::new(80, 24);
        let reply = String::from_utf8(s.feed(b"\x1b]11;?\x07")).unwrap();
        let bg = s.palette.bg;
        let want = format!("rgb:{0:02x}{0:02x}/{1:02x}{1:02x}/{2:02x}{2:02x}", bg.r, bg.g, bg.b);
        assert!(reply.contains(&want), "{reply:?} vs {want}");
    }

    #[test]
    fn answers_text_area_size_query() {
        let mut s = Screen::new(80, 24);
        s.set_cell_px(9, 18);
        let reply = String::from_utf8(s.feed(b"\x1b[14t")).unwrap();
        assert!(reply.contains("4;432;720t"), "{reply:?}");
    }

    #[test]
    fn tracks_title() {
        let mut s = Screen::new(80, 24);
        s.feed(b"\x1b]0;hello\x07");
        assert_eq!(s.title(), Some("hello"));
    }

    #[test]
    fn resize_changes_the_grid() {
        let mut s = Screen::new(80, 24);
        s.resize(100, 30);
        assert_eq!((s.term().columns(), s.term().screen_lines()), (100, 30));
    }

    #[test]
    fn look_follows_the_profile() {
        let mut p = crate::profile::Profile::default();
        p.set("Cursor Type", toml::Value::Integer(1));
        p.set("Blinking Cursor", toml::Value::Boolean(true));
        p.set("Scrollback Lines", toml::Value::Integer(5000));
        let look = Look::from_profile(&p);
        assert_eq!((look.cursor_shape, look.cursor_blink, look.scrollback), (CursorShape::Beam, true, 5000));
        p.set("Unlimited Scrollback", toml::Value::Boolean(true));
        assert_eq!(Look::from_profile(&p).scrollback, 100_000);
    }

    #[test]
    fn the_profile_cursor_shape_is_used() {
        let mut p = crate::profile::Profile::default();
        p.set("Cursor Type", toml::Value::Integer(0));
        let s = Screen::with_look(80, 24, Palette::default(), Look::from_profile(&p));
        assert_eq!(s.term().renderable_content().cursor.shape, CursorShape::Underline);
    }

    #[test]
    fn all_text_includes_scrollback() {
        let mut s = Screen::new(20, 3);
        for i in 0..6 {
            s.feed(format!("line{i}\r\n").as_bytes());
        }
        let t = s.all_text();
        assert!(t.starts_with("line0\nline1\n"), "{t:?}");
        assert!(t.contains("line5"));
    }

    #[test]
    fn clear_scrollback_keeps_the_screen() {
        let mut s = Screen::new(20, 3);
        for i in 0..6 {
            s.feed(format!("line{i}\r\n").as_bytes());
        }
        s.clear(true);
        let t = s.all_text();
        assert!(!t.contains("line0") && t.contains("line5"), "{t:?}");
        s.clear(false);
        assert!(!s.all_text().contains("line5"));
    }

    #[test]
    fn a_mark_moves_into_history_with_its_row() {
        let mut s = Screen::new(20, 3);
        s.feed(b"prompt> ");
        s.set_row_mark(0, 7);
        for i in 0..5 {
            s.feed(format!("\r\nout{i}").as_bytes());
        }
        let marks = s.row_marks();
        assert_eq!(marks.len(), 1);
        let (line, id) = marks[0];
        assert_eq!(id, 7);
        assert!(line < 0, "{line}");
        assert!(s.line_text(line).starts_with("prompt>"));
    }

    #[test]
    fn a_mark_survives_reflow() {
        let mut s = Screen::new(10, 5);
        s.feed(b"$ ");
        s.set_row_mark(0, 3);
        s.feed(b"abcdefghijklmnop\r\nnext");
        s.resize(6, 5);
        s.resize(30, 5);
        let (line, id) = s.row_marks()[0];
        assert_eq!(id, 3);
        assert!(s.line_text(line).starts_with("$ abcdefghijklmnop"), "{:?}", s.line_text(line));
    }

    #[test]
    fn a_cleared_row_loses_its_mark() {
        let mut s = Screen::new(20, 3);
        s.set_row_mark(1, 5);
        s.feed(b"\x1b[H\x1b[J");
        assert!(s.row_marks().is_empty());
    }

    #[test]
    fn clear_buffer_keeps_the_cursor_line_at_the_top() {
        let mut s = Screen::new(20, 5);
        s.feed(b"old1\r\nold2\r\nold3\r\nprompt> ");
        s.clear(false);
        assert_eq!(s.all_text(), "prompt>\n");
        let cursor = s.term().grid().cursor.point;
        assert_eq!((cursor.line.0, cursor.column.0), (0, 8));
    }

    #[test]
    fn soft_reset_keeps_the_text_and_drops_the_modes() {
        let mut s = Screen::new(20, 5);
        s.feed(b"keep me\r\n\x1b[?1002h\x1b[?1006h\x1b[?25l\x1b[?2004h\x1b[1;31m");
        s.soft_reset();
        assert!(s.all_text().contains("keep me"));
        assert_eq!(s.mouse_modes(), Modes { tracking: Tracking::Off, sgr: false });
        let mode = *s.term().mode();
        assert!(mode.contains(TermMode::SHOW_CURSOR) && !mode.contains(TermMode::BRACKETED_PASTE));
    }

    #[test]
    fn tracks_drag_and_sgr() {
        let mut s = Screen::new(80, 24);
        s.feed(b"\x1b[?1002h\x1b[?1006h");
        assert_eq!(s.mouse_modes(), Modes { tracking: Tracking::Drag, sgr: true });
    }

    #[test]
    fn any_motion_wins() {
        let mut s = Screen::new(80, 24);
        s.feed(b"\x1b[?1000h\x1b[?1003h");
        assert_eq!(s.mouse_modes().tracking, Tracking::Any);
    }

    #[test]
    fn reset_turns_tracking_off() {
        let mut s = Screen::new(80, 24);
        s.feed(b"\x1b[?1002h\x1b[?1002l");
        assert_eq!(s.mouse_modes().tracking, Tracking::Off);
    }

    #[test]
    fn answers_device_attributes() {
        let mut s = Screen::new(80, 24);
        let reply = s.feed(b"\x1b[c");
        assert!(reply.starts_with(b"\x1b[?"), "{:?}", String::from_utf8_lossy(&reply));
    }

    #[test]
    fn text_shows_output() {
        let mut s = Screen::new(80, 24);
        s.feed(b"hello\r\nworld");
        let text = s.text();
        assert!(text.contains("hello") && text.contains("world"), "{text}");
    }

    #[test]
    fn output_stamps_the_rows_it_writes() {
        let mut s = Screen::new(20, 6);
        s.set_clock(Some(100));
        s.feed(b"a\r\nb");
        s.set_clock(Some(200));
        s.feed(b"\r\nc");
        assert_eq!((s.row_time(0), s.row_time(1), s.row_time(2), s.row_time(3)), (100, 100, 200, 0));
    }

    #[test]
    fn row_slots_survive_reflow() {
        let mut s = Screen::new(20, 6);
        s.set_clock(Some(100));
        s.feed(b"0123456789012345678901234\r\n");
        s.set_clock(Some(200));
        s.feed(b"x");
        // Every row by its text, oldest first.
        let rows = |s: &Screen| {
            let top = -(s.term().grid().history_size() as i32);
            (top..6)
                .map(|l| {
                    let g = s.term().grid();
                    let text: String = (0..g.columns()).map(|c| g[Line(l)][Column(c)].c).collect();
                    (text.trim_end().to_string(), s.row_time(l))
                })
                .filter(|(t, _)| !t.is_empty())
                .collect::<Vec<_>>()
        };
        s.resize(10, 6);
        let t = |xs: &[(&str, u32)]| xs.iter().map(|(a, b)| (a.to_string(), *b)).collect::<Vec<_>>();
        assert_eq!(rows(&s), t(&[("0123456789", 100), ("0123456789", 100), ("01234", 100), ("x", 200)]));
        s.resize(30, 6);
        assert_eq!(rows(&s), t(&[("0123456789012345678901234", 100), ("x", 200)]));
    }

    #[test]
    fn splice_replaces_rows_and_keeps_the_cursor_row() {
        let mut s = Screen::new(20, 6);
        s.feed(b"l0\r\nl1\r\nl2\r\nl3\r\nl4\r\nprompt");
        let cursor = s.term().grid().cursor.point.line.0;
        let placeholder = s.text_rows(&["folded".to_string()]);
        let removed = s.splice_rows(1, 3, placeholder);
        assert_eq!(removed.len(), 3);
        // Rows above slide down; with no history, blank rows fill in at the top.
        assert_eq!(s.all_text().trim_start_matches('\n'), "l0\nfolded\nl4\nprompt\n");
        assert_eq!(s.term().grid().cursor.point.line.0, cursor);
        assert_eq!(s.line_text(cursor), "prompt");
        let line = (0..6).find(|l| s.line_text(*l) == "folded").unwrap();
        s.splice_rows(line, line, removed);
        assert_eq!(s.all_text().trim_start_matches('\n'), "l0\nl1\nl2\nl3\nl4\nprompt\n");
        assert_eq!(s.line_text(s.term().grid().cursor.point.line.0), "prompt");
    }

    #[test]
    fn text_rows_wrap_long_lines() {
        let s = Screen::new(4, 3);
        let rows = s.text_rows(&["abcdefg".to_string(), "h".to_string()]);
        let text = |r: &Row<Cell>| (0..4).map(|c| r[Column(c)].c).collect::<String>();
        assert_eq!(rows.iter().map(text).collect::<Vec<_>>(), vec!["abcd", "efg ", "h   "]);
        use alacritty_terminal::term::cell::Flags;
        assert!(rows[0][Column(3)].flags.contains(Flags::WRAPLINE));
        assert!(!rows[1][Column(3)].flags.contains(Flags::WRAPLINE));
    }

    fn lines_text(s: &Screen) -> String {
        s.all_text().trim_start_matches('\n').to_string()
    }

    #[test]
    fn fold_then_unfold_is_lossless() {
        let mut s = Screen::with_scrollback(20, 6, 100);
        s.feed(b"l0\r\nl1\r\nl2\r\nl3\r\n$ ");
        s.set_row_mark(1, 7);
        let id = s.fold(1, 3).unwrap();
        assert_eq!(lines_text(&s), "l0\nl1 \u{2026}3 lines\u{2026} l3\n$\n");
        let line = (0..6).find(|l| s.fold_at(*l) == Some(id)).unwrap();
        assert_eq!(s.line_text(line), "l1 \u{2026}3 lines\u{2026} l3");
        assert!(s.unfold(id));
        assert_eq!(lines_text(&s), "l0\nl1\nl2\nl3\n$\n");
        assert!(s.row_marks().iter().any(|(_, m)| *m == 7), "the mark came back with its row");
        assert!(!s.unfold(id));
    }

    #[test]
    fn unfold_restores_lines_after_scrolling() {
        let mut s = Screen::with_scrollback(20, 4, 100);
        s.feed(b"a\r\nb\r\nc\r\n$ ");
        let id = s.fold(0, 1).unwrap();
        for i in 0..10 {
            s.feed(format!("\r\nout{i}").as_bytes());
        }
        assert!(s.unfold(id));
        let text = lines_text(&s);
        assert!(text.starts_with("a\nb\nc\n$\nout0\n"), "{text}");
        assert!(text.ends_with("out9\n"), "{text}");
    }

    #[test]
    fn fold_refuses_the_cursor_row() {
        let mut s = Screen::with_scrollback(20, 4, 100);
        s.feed(b"a\r\nb\r\n$ ");
        assert_eq!(s.fold(1, 2), None);
        s.feed(b"\x1b[?1049h");
        assert_eq!(s.fold(0, 0), None, "not on the alternate screen");
    }

    #[test]
    fn unfold_after_widening_fits_the_rows() {
        let mut s = Screen::with_scrollback(20, 5, 100);
        s.feed(b"l0\r\nl1\r\nl2\r\n$ ");
        let id = s.fold(0, 1).unwrap();
        s.resize(30, 5);
        assert!(s.unfold(id));
        let text = lines_text(&s);
        assert!(text.starts_with("l0\nl1\nl2\n$"), "{text}");
        let mut f = crate::scene::Frame::default();
        crate::scene::pane(&mut f, &s, crate::layout::Rect { x: 0.0, y: 0.0, w: 300.0, h: 100.0 }, (10.0, 20.0), true, true);
    }

    #[test]
    fn unfold_after_narrowing_leaves_no_summary() {
        let mut s = Screen::with_scrollback(30, 5, 100);
        s.feed(b"first line here\r\nmiddle\r\nlast line here\r\n$ ");
        let id = s.fold(0, 2).unwrap();
        s.resize(10, 5);
        assert!(s.unfold(id));
        let text = lines_text(&s);
        assert!(!text.contains('\u{2026}'), "{text}");
        assert!(text.contains("middle"), "{text}");
    }

    #[test]
    fn buffer_edits_refuse_the_alternate_screen() {
        let mut s = Screen::with_scrollback(20, 5, 100);
        s.feed(b"a\r\nb\r\nc\r\n$ ");
        let id = s.fold(0, 1).unwrap();
        s.feed(b"\x1b[?1049h");
        assert!(s.splice_rows(0, 0, Vec::new()).is_empty());
        assert!(!s.unfold(id), "the fold waits for the main screen");
        assert_eq!(s.add_note(0, 0, 0, 1, "x", false), 0);
        s.feed(b"\x1b[?1049l");
        assert!(s.unfold(id));
    }

    #[test]
    fn clearing_forgets_folds_and_notes() {
        let mut s = Screen::with_scrollback(20, 5, 100);
        s.feed(b"a\r\nb\r\nc\r\n$ ");
        s.fold(0, 1).unwrap();
        s.add_note(2, 0, 2, 0, "n", false);
        s.clear(false);
        assert_eq!(s.fold_count(), 0);
        assert!(s.notes.by_id.is_empty());
    }
}

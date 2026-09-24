//! Runs app output through the terminal core and collects the replies the app expects.

use std::sync::mpsc::{self, Receiver, Sender};

use alacritty_terminal::Term;
use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::Dimensions;
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
    term: Term<Listener>,
    parser: Processor,
    events: Receiver<Event>,
    pub palette: Palette,
    title: Option<String>,
    cell_px: (u16, u16),
}

impl Screen {
    pub fn new(cols: usize, rows: usize) -> Self {
        Screen::with_look(cols, rows, Palette::default(), Look::default())
    }

    pub fn with_look(cols: usize, rows: usize, palette: Palette, look: Look) -> Self {
        let (tx, events) = mpsc::channel();
        let term = Term::new(config(look), &GridSize { cols, rows }, Listener(tx));
        Self { term, parser: Processor::new(), events, palette, title: None, cell_px: (0, 0) }
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
        self.parser.advance(&mut self.term, bytes);
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

    /// iTerm2's Clear Scrollback Buffer (`scrollback_only`) or Clear Buffer (everything).
    pub fn clear(&mut self, scrollback_only: bool) {
        // Clearing the screen pushes its lines into scrollback (xterm behavior), so clear
        // the screen first, then the scrollback.
        if !scrollback_only {
            self.term.clear_screen(ClearMode::All);
            self.term.goto(0, 0);
        }
        self.term.clear_screen(ClearMode::Saved);
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
}

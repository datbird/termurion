//! What the AI chat's tools read from a screen (`WorkgroupIntrospection.swift`
//! `screenContents`, `extractScreenText`; `PTYSession.swift` `contentAfter`,
//! `getCommandOutputRemoteCommand`): absolute lines, the marked-up text, the screen JSON,
//! and the commands with their output. Pure; an `impl Screen` block as `marks.rs` is.

use alacritty_terminal::grid::{Dimensions, Row};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::TermMode;

use crate::ai_terminal::{DEFAULT_SCREEN_LINES, MAX_SCREEN_LINES, OUTPUT_CAP};
use crate::marks::Kind;
use crate::screen::Screen;

// The markup tokens (`WI:680-683`); the brackets are U+27E8 and U+27E9.
const DIM_OPEN: &str = "\u{27E8}dim\u{27E9}";
const DIM_CLOSE: &str = "\u{27E8}/dim\u{27E9}";
const CURSOR: &str = "\u{27E8}cursor\u{27E9}";
const IMAGE: &str = "\u{27E8}image\u{27E9}";

/// One piece of extracted text: a cell's characters, a dim cell's, an image tile, or a line
/// break.
#[derive(Clone, Debug, PartialEq)]
enum Piece {
    Text(String, bool),
    Image(u32),
    Break,
}

/// Which grid a line reads from: the screen shown (the main screen's history above the current
/// grid, iTerm2's shared line buffer), or the main screen alone (where the marks live).
#[derive(Clone, Copy, PartialEq)]
enum Source {
    Shown,
    Main,
}

/// What `finished_command` reads.
#[derive(Clone, Debug, PartialEq)]
pub struct Finished {
    pub mark: u32,
    pub command: String,
    pub directory: Option<String>,
    pub host: Option<String>,
    pub user: Option<String>,
    /// None when the ended mark is no longer in the buffer.
    pub output: Option<String>,
}

impl Screen {
    /// The row at grid line `line`; None past either end.
    fn tool_row(&self, line: i32, source: Source) -> Option<&Row<Cell>> {
        let main = self.term.primary_grid();
        let g = if line < 0 || source == Source::Main { main } else { self.term.grid() };
        (line >= -(main.history_size() as i32) && line < g.screen_lines() as i32).then(|| &g[Line(line)])
    }

    /// The grid line of absolute line `abs` (lost lines plus history count from 0, as
    /// `api_buffer::above`).
    fn grid_line(&self, abs: i64) -> i64 {
        abs - crate::api_buffer::above(self.term.primary_grid())
    }

    /// The pieces of the text from `start` (grid line, column) to `end` (exclusive), iTerm2's
    /// extractor with trailing whitespace trimmed at each hard line end and a soft-wrapped
    /// row joined to the next with nothing. With `cursor`, also the index of the piece the
    /// cursor marker goes before (`cursorMarkupIndex`: the first piece at or after the cursor
    /// on its row, else the end of its row's text).
    fn pieces(&self, start: (i32, usize), end: (i32, usize), source: Source, cursor: bool) -> (Vec<Piece>, Option<usize>) {
        let at = self.term.grid().cursor.point;
        let at = cursor.then_some((at.line.0, at.column.0));
        let last = if end.1 == 0 { end.0 - 1 } else { end.0 };
        let mut out = Vec::new();
        let mut marker = None;
        for line in start.0.max(-(self.term.primary_grid().history_size() as i32))..=last {
            let Some(row) = self.tool_row(line, source) else { break };
            let cols = row.len();
            let from = if line == start.0 { start.1 } else { 0 };
            let to = if line == end.0 { end.1.min(cols) } else { cols };
            let wrapped = to == cols && cols > 0 && row[Column(cols - 1)].flags.contains(Flags::WRAPLINE) && line < last;
            let mut cells: Vec<(usize, Piece)> = Vec::new();
            for c in from..to {
                let cell = &row[Column(c)];
                if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                    continue;
                }
                let piece = match cell.image() {
                    Some(tile) => Piece::Image(tile.image),
                    None => {
                        let mut text = cell.c.to_string();
                        text.extend(cell.zerowidth().unwrap_or_default());
                        Piece::Text(text, cell.flags.contains(Flags::DIM))
                    }
                };
                cells.push((c, piece));
            }
            if !wrapped {
                while cells.last().is_some_and(|(_, p)| matches!(p, Piece::Text(t, _) if t.chars().all(char::is_whitespace))) {
                    cells.pop();
                }
            }
            for (c, piece) in cells {
                if marker.is_none() && at.is_some_and(|(l, col)| l == line && c >= col) {
                    marker = Some(out.len());
                }
                out.push(piece);
            }
            if marker.is_none() && at.is_some_and(|(l, _)| l == line) {
                marker = Some(out.len());
            }
            if !wrapped && line < last {
                out.push(Piece::Break);
            }
        }
        (out, marker)
    }

    /// The plain text of `pieces` (iTerm2's extractor without an attribute provider: images
    /// have no text).
    fn plain(&self, start: (i32, usize), end: (i32, usize), source: Source) -> String {
        let (pieces, _) = self.pieces(start, end, source, false);
        pieces
            .iter()
            .map(|p| match p {
                Piece::Text(t, _) => t.as_str(),
                Piece::Image(_) => "",
                Piece::Break => "\n",
            })
            .collect()
    }

    /// The cursor's absolute line (`numberOfScrollbackLines + cursorY - 1 +
    /// totalScrollbackOverflow`).
    pub fn abs_cursor_line(&self) -> i64 {
        crate::api_buffer::above(self.term.primary_grid()) + self.term.grid().cursor.point.line.0 as i64
    }

    /// The absolute line of grid line `grid` (a mark's line).
    pub fn abs_line(&self, grid: i32) -> i64 {
        crate::api_buffer::above(self.term.primary_grid()) + grid as i64
    }

    /// The absolute number of the oldest line the main screen still keeps.
    pub fn oldest_abs_line(&self) -> i64 {
        self.term.primary_grid().dropped() as i64
    }

    /// The text of absolute lines `from` to `to_exclusive`, as `contentAfter` extracts it:
    /// trailing whitespace trimmed per line, lines joined with "\n", a soft-wrapped row joined
    /// to the next with nothing. None when `from` was dropped from the scrollback.
    pub fn abs_text(&self, from: i64, to_exclusive: i64) -> Option<String> {
        let (start, end) = (self.grid_line(from), self.grid_line(to_exclusive));
        if start < -(self.term.primary_grid().history_size() as i64) {
            return None;
        }
        let clamp = |l: i64| l.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
        Some(self.plain((clamp(start), 0), (clamp(end), 0), Source::Shown))
    }

    /// Grid lines `from_line` to `to_line` (inclusive) with iTerm2's markup
    /// (`extractScreenText`): dim runs in \u{27E8}dim\u{27E9}..\u{27E8}/dim\u{27E9}, the cursor
    /// as \u{27E8}cursor\u{27E9} (with `cursor`), each image run on a row as
    /// \u{27E8}image\u{27E9}; trailing blank lines dropped.
    pub fn marked_text(&self, from_line: i32, to_line: i32, cursor: bool) -> String {
        let (pieces, marker) = self.pieces((from_line, 0), (to_line.saturating_add(1), 0), Source::Shown, cursor);
        // `renderAttributed`.
        let mut out = String::new();
        let (mut dim, mut image) = (false, None);
        for (i, piece) in pieces.iter().enumerate() {
            let want = matches!(piece, Piece::Text(_, true));
            if want != dim {
                out.push_str(if want { DIM_OPEN } else { DIM_CLOSE });
                dim = want;
            }
            if marker == Some(i) {
                out.push_str(CURSOR);
            }
            match piece {
                Piece::Image(id) => {
                    if image != Some(*id) {
                        out.push_str(IMAGE);
                        image = Some(*id);
                    }
                }
                Piece::Text(t, _) => {
                    image = None;
                    // A program printing the markup's own brackets cannot fake a token: they
                    // read as plain angle brackets (Tandem; iTerm2 relies on their rarity).
                    out.extend(t.chars().map(|c| match c {
                        '\u{27E8}' => '<',
                        '\u{27E9}' => '>',
                        c => c,
                    }));
                }
                Piece::Break => {
                    image = None;
                    out.push('\n');
                }
            }
        }
        if dim {
            out.push_str(DIM_CLOSE);
        }
        if marker == Some(pieces.len()) {
            out.push_str(CURSOR);
        }
        // `stripTrailingBlankLines`.
        let mut lines: Vec<&str> = out.split('\n').collect();
        while lines.last().is_some_and(|l| l.trim().is_empty()) {
            lines.pop();
        }
        lines.join("\n")
    }

    /// The session-bound `get_screen_contents` result (`PS:745-753`, `WI:304-337`):
    /// `{"screen_contents": {...}}` with the last `lines` of the main screen (None: 100;
    /// clamped 1 to 2000), or on the alternate screen its visible grid only as a snapshot.
    /// `pending_action` is never sent (Tandem has no overlay); `changed` is
    /// `screen_last_changed`, omitted when None.
    pub fn screen_contents(&self, lines: Option<i64>, kind: &str, changed: Option<String>) -> serde_json::Value {
        let alternate = self.term.mode().contains(TermMode::ALT_SCREEN);
        let rows = self.term.grid().screen_lines() as i32;
        let text = if alternate {
            self.marked_text(0, rows - 1, true)
        } else {
            let n = lines.map_or(DEFAULT_SCREEN_LINES, |n| n.clamp(1, MAX_SCREEN_LINES)) as i32;
            let top = -(self.term.primary_grid().history_size() as i32);
            self.marked_text((rows - n).max(top), rows - 1, true)
        };
        let mut v = serde_json::Map::new();
        v.insert("text".into(), text.into());
        v.insert("kind".into(), kind.into());
        v.insert("is_snapshot".into(), alternate.into());
        v.insert("screen".into(), if alternate { "alternate" } else { "primary" }.into());
        v.insert("mouse_reporting".into(), (self.mouse_modes().tracking != crate::mouse::Tracking::Off).into());
        if let Some(c) = changed {
            v.insert("screen_last_changed".into(), c.into());
        }
        serde_json::json!({ "screen_contents": v })
    }

    /// When a visible row was last written (Unix seconds), for `screen_last_changed`; None
    /// when none was.
    pub fn newest_visible_change(&self) -> Option<u32> {
        let rows = self.term.grid().screen_lines() as i32;
        (0..rows).map(|l| self.row_time(l)).max().filter(|t| *t > 0)
    }

    /// The prompt marks still in the buffer that hold a command, oldest first: (mark id,
    /// command), the history `get_command_history` lists (`enumeratePrompts`).
    pub fn prompt_commands(&self) -> Vec<(u32, String)> {
        self.row_marks()
            .into_iter()
            .filter_map(|(_, id)| self.marks.records.get(&id))
            .filter(|m| m.kind == Kind::Prompt && !m.command.is_empty())
            .map(|m| (m.id, m.command.clone()))
            .collect()
    }

    /// `get_command_output`'s text (`PS:699-735`): prompt mark `id`'s output when it is in the
    /// buffer, else the last command's; from where its output began to the next prompt, or to
    /// the cursor while nothing follows it; the last `OUTPUT_CAP` characters. None when the
    /// output is gone.
    pub fn command_output(&self, id: Option<u32>) -> Option<String> {
        let marks = self.row_marks();
        let is_prompt = |id: &u32| self.marks.records.get(id).is_some_and(|m| m.kind == Kind::Prompt);
        let chosen = id.filter(|id| marks.iter().any(|(_, m)| m == id) && is_prompt(id)).or(self.marks.last_command)?;
        let mark = self.marks.records.get(&chosen)?;
        let start = self.resolve(mark.output_start?)?;
        let line = marks.iter().find(|(_, m)| *m == chosen)?.0;
        let cursor = self.term.primary_grid().cursor.point;
        let end = marks.iter().find(|(l, m)| *l > line && is_prompt(m)).map_or((cursor.line.0, cursor.column.0), |(l, _)| (*l, 0));
        let text = self.plain((start.line.0, start.column.0), end, Source::Main);
        let n = text.chars().count();
        Some(if n > OUTPUT_CAP { text.chars().skip(n - OUTPUT_CAP).collect() } else { text })
    }

    /// A finished command for a chat that streams (`streamLastCommand`, `CVC:2401-2455`): the
    /// ended mark's own command, directory, the host and user the shell reported, and its output
    /// only while that mark is still a prompt in the buffer (ruling F32: `command_output` would
    /// answer the last command's otherwise). None without a mark or a command text.
    pub fn finished_command(&self, mark: Option<u32>) -> Option<Finished> {
        let id = mark?;
        let record = self.marks.records.get(&id).filter(|m| m.kind == Kind::Prompt)?;
        if record.command.is_empty() {
            return None;
        }
        let in_buffer = self.row_marks().iter().any(|(_, m)| *m == id);
        Some(Finished {
            mark: id,
            command: record.command.clone(),
            directory: record.directory.clone(),
            host: self.shell.host.clone(),
            user: self.shell.user.clone(),
            output: if in_buffer { self.command_output(Some(id)) } else { None },
        })
    }

    /// The last command's exit status: None with no command mark, Some(None) while it runs.
    pub fn last_exit(&self) -> Option<Option<i32>> {
        self.last_command_mark().map(|m| m.code)
    }
}

#[cfg(test)]
mod tests {
    use crate::screen::Screen;

    const PROMPT: &[u8] = b"\x1b]133;A\x07$ \x1b]133;B\x07";

    fn run(s: &mut Screen, command: &str) {
        s.feed(PROMPT);
        s.feed(command.as_bytes());
        s.feed(b"\r\n\x1b]133;C\x07");
    }

    fn picture() -> crate::images::Image {
        let frame = crate::images::Frame { rgba: std::sync::Arc::new(vec![0; 400]), delay_ms: 0 };
        crate::images::Image { width_px: 10, height_px: 10, cols: 3, rows: 2, frames: vec![frame], broken: false, keep_aspect: true, uid: 0 }
    }

    #[test]
    fn markup_follows_iterm2() {
        let mut s = Screen::new(20, 5);
        s.feed(b"ab\x1b[2mcd\x1b[0me");
        assert_eq!(s.marked_text(0, 0, true), "ab\u{27E8}dim\u{27E9}cd\u{27E8}/dim\u{27E9}e\u{27E8}cursor\u{27E9}");
        assert_eq!(s.marked_text(0, 0, false), "ab\u{27E8}dim\u{27E9}cd\u{27E8}/dim\u{27E9}e");
        // The cursor on the first dim cell: the run opens, then the cursor (renderAttributed).
        s.feed(b"\x1b[3D");
        assert_eq!(s.marked_text(0, 0, true), "ab\u{27E8}dim\u{27E9}\u{27E8}cursor\u{27E9}cd\u{27E8}/dim\u{27E9}e");
        // Printed markup brackets are plain brackets: no fake token reaches the model.
        let mut fake = Screen::new(40, 3);
        fake.feed("a\u{27E8}/dim\u{27E9}\u{27E8}cursor\u{27E9}".as_bytes());
        assert_eq!(fake.marked_text(0, 0, false), "a</dim><cursor>");
        // Trailing blanks and blank rows go; a dim run closes at its line's end.
        let mut t = Screen::new(20, 5);
        t.feed(b"x   \r\n\x1b[2mdim  \x1b[0m\r\n");
        assert_eq!(t.marked_text(0, 4, false), "x\n\u{27E8}dim\u{27E9}dim\u{27E8}/dim\u{27E9}");
        // The cursor past the text of its row sits at the row's end; on an empty row, alone.
        assert_eq!(t.marked_text(0, 4, true), "x\n\u{27E8}dim\u{27E9}dim\u{27E8}/dim\u{27E9}\n\u{27E8}cursor\u{27E9}");
        // One marker per image run on each row; the cells are not trimmed as blanks.
        let mut i = Screen::new(20, 6);
        i.feed(b"ab");
        i.place_image(picture(), true);
        assert_eq!(i.marked_text(0, 1, false), "ab\u{27E8}image\u{27E9}\n  \u{27E8}image\u{27E9}");
        // A soft-wrapped row joins the next with nothing.
        let mut w = Screen::new(4, 5);
        w.feed(b"abcdef");
        assert_eq!(w.marked_text(0, 1, false), "abcdef");
    }

    #[test]
    fn screen_contents_follows_iterm2() {
        let mut s = Screen::new(20, 5);
        let lines: Vec<String> = (1..=13).map(|n| format!("l{n}")).collect();
        s.feed(lines.join("\r\n").as_bytes());
        let all = format!("{}\u{27E8}cursor\u{27E9}", lines.join("\n"));
        let v = s.screen_contents(None, "other", None);
        assert_eq!(v, serde_json::json!({"screen_contents": {"text": all, "kind": "other", "is_snapshot": false, "screen": "primary", "mouse_reporting": false}}));
        assert_eq!(
            serde_json::to_string(&v).unwrap(),
            format!(r#"{{"screen_contents":{{"text":"{}","kind":"other","is_snapshot":false,"screen":"primary","mouse_reporting":false}}}}"#, all.replace('\n', "\\n")),
            "the keys in iTerm2's order; pending_action and screen_last_changed omitted when None"
        );
        let text = |v: &serde_json::Value| v["screen_contents"]["text"].as_str().unwrap().to_string();
        assert_eq!(text(&s.screen_contents(Some(3), "other", None)), "l11\nl12\nl13\u{27E8}cursor\u{27E9}");
        assert_eq!(text(&s.screen_contents(Some(0), "other", None)), "l13\u{27E8}cursor\u{27E9}");
        assert_eq!(text(&s.screen_contents(Some(-1), "other", None)), "l13\u{27E8}cursor\u{27E9}");
        assert_eq!(text(&s.screen_contents(Some(i64::MAX), "other", None)), all);
        let aged = s.screen_contents(None, "other", Some("< 1 min ago".into()));
        assert_eq!(aged["screen_contents"]["screen_last_changed"], "< 1 min ago");
        assert!(aged["screen_contents"].get("pending_action").is_none());

        // The alternate screen: the visible grid only, a snapshot.
        s.feed(b"\x1b[?1049h\x1b[Hvis");
        let alt = s.screen_contents(Some(100), "tui", None);
        assert_eq!(alt, serde_json::json!({"screen_contents": {"text": "vis\u{27E8}cursor\u{27E9}", "kind": "tui", "is_snapshot": true, "screen": "alternate", "mouse_reporting": false}}));
        s.feed(b"\x1b[?1000h");
        assert_eq!(s.screen_contents(None, "tui", None)["screen_contents"]["mouse_reporting"], true);
        assert!(s.newest_visible_change().is_some());
    }

    #[test]
    fn a_finished_command_reads_its_own_mark() {
        let mut s = Screen::new(20, 10);
        s.feed(b"\x1b]7;file://box/home/me\x07");
        s.feed(b"\x1b]1337;RemoteHost=me@box\x07");
        run(&mut s, "ls");
        s.feed(b"a\r\nb\r\n\x1b]133;D;0\x07");
        let ls = s.marks.last_command.unwrap();
        run(&mut s, "pwd");
        s.feed(b"/home\r\n\x1b]133;D;0\x07");
        let f = s.finished_command(Some(ls)).unwrap();
        assert_eq!((f.mark, f.command.as_str(), f.output.as_deref()), (ls, "ls", Some("a\nb")), "the ended mark's own output, not the last command's");
        assert_eq!((f.host.as_deref(), f.user.as_deref()), (Some("box"), Some("me")));
        assert_eq!(f.directory.as_deref(), Some("/home/me"));
        // A mark that is no longer in the buffer: the command is known if the record is, the
        // output is not, and the last command's output is never substituted (F32).
        let last = s.marks.last_command.unwrap();
        s.marks.records.get_mut(&ls).unwrap().output_start = None;
        assert_eq!(s.finished_command(Some(ls)).unwrap().output, None);
        assert_eq!(s.finished_command(Some(last)).unwrap().output.as_deref(), Some("/home"));
        assert_eq!(s.finished_command(Some(999)), None, "an unknown mark");
        assert_eq!(s.finished_command(None), None, "no mark");
        let scrolled = {
            let mut t = Screen::with_scrollback(20, 3, 0);
            run(&mut t, "first");
            t.feed(b"one\r\n\x1b]133;D;0\x07");
            let first = t.marks.last_command.unwrap();
            for c in ["b", "c", "d", "e"] {
                run(&mut t, c);
                t.feed(b"x\r\n\x1b]133;D;0\x07");
            }
            (t.finished_command(Some(first)), t.command_output(Some(first)))
        };
        assert!(scrolled.0.is_none_or(|f| f.output.is_none()), "the scrolled-away mark has no output of its own");
        assert!(scrolled.1.is_some(), "while command_output falls back to another command's (the reason for the rule)");
    }

    #[test]
    fn commands_and_output_follow_iterm2() {
        let mut none = Screen::new(20, 10);
        none.feed(b"plain\r\n");
        assert_eq!(none.last_exit(), None, "no command mark");
        assert_eq!(none.prompt_commands(), vec![]);
        assert_eq!(none.command_output(None), None);

        let mut s = Screen::new(20, 10);
        run(&mut s, "ls");
        s.feed(b"a\r\nb\r\n\x1b]133;D;0\x07");
        assert_eq!(s.last_exit(), Some(Some(0)));
        run(&mut s, "pwd");
        s.feed(b"/home\r\n");
        let commands = s.prompt_commands();
        assert_eq!(commands.iter().map(|(_, c)| c.as_str()).collect::<Vec<_>>(), ["ls", "pwd"]);
        let (ls, pwd) = (commands[0].0, commands[1].0);
        assert!(ls < pwd);
        assert_eq!(s.last_exit(), Some(None), "still running");
        assert_eq!(s.command_output(Some(ls)).as_deref(), Some("a\nb"));
        assert_eq!(s.command_output(Some(pwd)).as_deref(), Some("/home"));
        assert_eq!(s.command_output(None).as_deref(), Some("/home"), "the running command's output so far");
        assert_eq!(s.command_output(Some(999)).as_deref(), Some("/home"), "an unknown id reads the last command's");
        s.feed(b"\x1b]133;D;3\x07");
        assert_eq!(s.last_exit(), Some(Some(3)));

        // Output past the cap keeps its last OUTPUT_CAP characters (`truncateTail: false`).
        let mut big = Screen::with_scrollback(100, 10, 1000);
        run(&mut big, "cat");
        let body = format!("y{}", "x".repeat(19_999));
        big.feed(body.as_bytes());
        big.feed(b"\r\n\x1b]133;D;0\x07");
        let out = big.command_output(None).unwrap();
        assert_eq!(out.chars().count(), crate::ai_terminal::OUTPUT_CAP);
        assert!(out.chars().all(|c| c == 'x'), "the head is dropped");

        // A line that scrolled out of the scrollback is gone.
        let mut lost = Screen::with_scrollback(20, 5, 10);
        for n in 0..40 {
            lost.feed(format!("n{n}\r\n").as_bytes());
        }
        let cursor = lost.abs_cursor_line();
        assert_eq!(cursor, 40);
        assert_eq!(lost.abs_text(0, 1), None);
        assert_eq!(lost.abs_text(cursor - 2, cursor + 1).as_deref(), Some("n38\nn39\n"));
        assert_eq!(lost.abs_text(cursor - 14, cursor - 13).as_deref(), Some("n26"), "the oldest line kept");
        assert_eq!(lost.abs_text(cursor - 15, cursor - 14), None);
    }

    #[test]
    fn abs_text_joins_soft_wraps() {
        // Twenty "a" fill the first row, which wraps into the marker text: one line.
        let mut s = Screen::new(20, 5);
        s.feed(b"aaaaaaaaaaaaaaaaaaaa-- FINISHED U\r\n");
        let c = s.abs_cursor_line();
        assert_eq!(s.abs_text(c - 2, c).as_deref(), Some("aaaaaaaaaaaaaaaaaaaa-- FINISHED U"));
        // A real line break after the twenty: two lines, the marker at a line start.
        let mut t = Screen::new(20, 5);
        t.feed(b"aaaaaaaaaaaaaaaaaaaa\r\n-- FINISHED U\r\n");
        let c = t.abs_cursor_line();
        assert_eq!(t.abs_text(c - 2, c).as_deref(), Some("aaaaaaaaaaaaaaaaaaaa\n-- FINISHED U"));
    }
}

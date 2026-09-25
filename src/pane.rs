//! One terminal pane: a shell in a pty, and its screen.

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::term::TermMode;

use crate::conpty::Api;
use crate::layout::PaneId;
use crate::pty::{self, Notify, Pty, Read};
use crate::screen::Screen;

/// The bytes to send for a paste. Newlines become carriage returns, as a typed
/// Enter would. Bracketed paste wraps the text so the app knows it was pasted,
/// and removes every ESC from it, so no end marker can form inside and a paste
/// cannot break out and run commands.
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if !bracketed {
        return text.into_bytes();
    }
    format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', "")).into_bytes()
}

/// Shell > Toggle Buffer Input: while on, what the user sends waits here.
#[derive(Default)]
pub struct InputBuffer {
    pub on: bool,
    queue: Vec<u8>,
}

impl InputBuffer {
    /// The bytes to send now, or None when they wait.
    pub fn send(&mut self, bytes: &[u8]) -> Option<Vec<u8>> {
        if self.on {
            self.queue.extend_from_slice(bytes);
            None
        } else {
            Some(bytes.to_vec())
        }
    }

    /// Turns buffering off and returns what waited, in order.
    pub fn release(&mut self) -> Vec<u8> {
        self.on = false;
        std::mem::take(&mut self.queue)
    }
}

/// Where a pane's shell runs: in Tandem, in the session host (it outlives Tandem), or in
/// tmux (a pane of a tmux window, reached through its gateway).
pub enum Io {
    Local(Pty),
    Hosted(crate::host_client::HostedPty),
    Tmux(TmuxIo),
}

/// A tmux pane's pty stand-in: `%output` waits in `inbox`, typing waits in `outbox` for the
/// gateway to turn into send-keys.
pub struct TmuxIo {
    pub gateway: PaneId,
    pub pane: u32,
    inbox: std::cell::RefCell<Vec<u8>>,
    outbox: std::cell::RefCell<Vec<u8>>,
    ended: std::cell::Cell<Option<u32>>,
    /// What tmux's monitors say about the pane.
    pub info: std::cell::RefCell<TmuxInfo>,
    /// This client answers the pane's OSC 4 color queries (tmux 3.6 and later elect one).
    pub answers_osc4: std::cell::Cell<bool>,
}

/// A tmux pane's title, foreground job, and working directory (remote paths), from tmux.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TmuxInfo {
    pub title: String,
    pub job: String,
    pub cwd: String,
}

impl TmuxIo {
    pub fn new(gateway: PaneId, pane: u32) -> TmuxIo {
        TmuxIo { gateway, pane, inbox: Default::default(), outbox: Default::default(), ended: Default::default(), info: Default::default(), answers_osc4: Default::default() }
    }

    /// Output from tmux for this pane.
    pub fn push_output(&self, bytes: &[u8]) {
        self.inbox.borrow_mut().extend_from_slice(bytes);
    }

    /// What was typed since the last call.
    pub fn take_input(&self) -> Vec<u8> {
        std::mem::take(&mut self.outbox.borrow_mut())
    }

    /// The gateway detached or tmux closed the pane.
    pub fn end(&self) {
        self.ended.set(Some(0));
    }
}

impl Io {
    pub fn try_read(&self) -> Read {
        match self {
            Io::Local(p) => p.try_read(),
            Io::Hosted(p) => p.try_read(),
            Io::Tmux(t) => {
                let data = std::mem::take(&mut *t.inbox.borrow_mut());
                if data.is_empty() { Read::Empty } else { Read::Data(data) }
            }
        }
    }

    pub fn write(&self, bytes: &[u8]) -> Result<(), String> {
        match self {
            Io::Local(p) => p.write(bytes),
            Io::Hosted(p) => p.write(bytes),
            Io::Tmux(t) => {
                t.outbox.borrow_mut().extend_from_slice(bytes);
                Ok(())
            }
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        match self {
            Io::Local(p) => p.resize(cols, rows),
            Io::Hosted(p) => p.resize(cols, rows),
            // tmux sets its panes' sizes (milestone 8c sends Tandem's).
            Io::Tmux(_) => Ok(()),
        }
    }

    pub fn clear(&self) -> Result<(), String> {
        match self {
            Io::Local(p) => p.clear(),
            Io::Hosted(p) => p.clear(),
            Io::Tmux(_) => Ok(()),
        }
    }

    pub fn set_paused(&self, paused: bool) {
        match self {
            Io::Local(p) => p.set_paused(paused),
            Io::Hosted(p) => p.set_paused(paused),
            Io::Tmux(_) => {}
        }
    }

    pub fn pending(&self) -> usize {
        match self {
            Io::Local(p) => p.pending(),
            Io::Hosted(p) => p.pending(),
            Io::Tmux(t) => t.outbox.borrow().len(),
        }
    }

    pub fn exit_code(&self) -> Option<u32> {
        match self {
            Io::Local(p) => p.exit_code(),
            Io::Hosted(p) => p.exit_code(),
            Io::Tmux(t) => t.ended.get(),
        }
    }

    /// The shell's process id; a tmux pane's process is on tmux's side.
    pub fn pid(&self) -> Option<u32> {
        match self {
            Io::Local(p) => Some(p.pid()),
            Io::Hosted(p) => Some(p.pid()),
            Io::Tmux(_) => None,
        }
    }
}

pub struct Pane {
    pub id: PaneId,
    pub screen: Screen,
    pty: Option<Io>,
    pub exited: Option<u32>,
    pub error: Option<String>,
    /// The Guid of the profile this pane runs.
    pub profile: String,
    /// The pane's own screen while `screen` shows a Filter or Zoom view. Output keeps
    /// going here.
    pub live: Option<Screen>,
    /// View > Show Session Note's text.
    pub session_note: String,
    /// Shell > Toggle Buffer Input.
    pub buffer: InputBuffer,
    /// Make Screenshot's frozen copy is showing: views and resizes leave it alone, so the
    /// panel's lines and redactions stay where they were chosen.
    pub frozen: bool,
    /// The bell offer's Suppress All Output: output is dropped until a key is typed.
    pub suppress_output: bool,
    /// Output arrived since the window last looked (idle and new-output tracking).
    pub got_output: bool,
    /// Session > Run Coprocess: it reads the raw output and types into the shell.
    coprocess: Option<crate::coproc_win::Coprocess>,
    co_out: crate::coprocess::OutQueue,
    /// A coprocess that ended: its command and stderr, for the window.
    co_end: Option<(String, String)>,
    /// tmux control mode's entry and line reader.
    tmux_hook: crate::tmux_hook::Hook,
    /// What the hook found, in order, for the window's tmux gateway. Output behind a tmux
    /// line waits here too, so "Detached" lands before what followed `%exit`.
    tmux_q: Vec<crate::tmux_hook::Out>,
    /// A screen-like TERM: italics as reverse video (iTerm2's translateFromScreenTerminal).
    pub screen_filter: Option<crate::screenterm::Filter>,
    /// A tmux gateway: its pty carries tmux commands only, so no input reaches it.
    pub gateway: bool,
    /// A tmux pane's size is tmux's, whatever its rect (milestone 8c asks tmux for others).
    tmux_size: Option<(u16, u16)>,
}

impl Pane {
    pub fn start(id: PaneId, api: &Api, cmdline: &str, cwd: Option<&std::path::Path>, cols: u16, rows: u16, notify: Notify) -> Result<Pane, String> {
        let (cols, rows) = (cols.max(2), rows.max(1));
        let pty = pty::spawn_notify(api, cmdline, cwd, cols, rows, notify)?;
        Ok(Pane::with_io(id, Io::Local(pty), cols, rows))
    }

    /// A pane whose shell the session host runs.
    pub fn start_hosted(id: PaneId, host: &crate::host_client::Client, cmdline: &str, cwd: Option<&std::path::Path>, cols: u16, rows: u16, notify: Notify) -> Result<Pane, String> {
        let (cols, rows) = (cols.max(2), rows.max(1));
        let pty = host.launch(cmdline, cwd, cols, rows, notify)?;
        Ok(Pane::with_io(id, Io::Hosted(pty), cols, rows))
    }

    /// A pane for a session the host already runs (a reattach).
    pub fn attached(id: PaneId, pty: crate::host_client::HostedPty, cols: u16, rows: u16) -> Pane {
        let (cols, rows) = (cols.max(2), rows.max(1));
        let mut pane = Pane::with_io(id, Io::Hosted(pty), cols, rows);
        // The host's pty may have another size (a resize just before a crash, another
        // monitor now): it takes the pane's, as iTerm2 sets the grid size after an attach.
        if let Some(io) = &pane.pty {
            if let Err(e) = io.resize(cols, rows) {
                pane.error = Some(e);
            }
        }
        pane
    }

    /// A pane of a tmux window: no ConPTY, its size is tmux's.
    pub fn tmux(id: PaneId, gateway: PaneId, pane: u32, cols: u16, rows: u16) -> Pane {
        let mut p = Pane::with_io(id, Io::Tmux(TmuxIo::new(gateway, pane)), cols.max(1), rows.max(1));
        p.screen.pty_backed = false;
        p.tmux_size = Some((cols, rows));
        p
    }

    /// tmux gave this pane a new size.
    pub fn set_tmux_size(&mut self, cols: u16, rows: u16) {
        self.tmux_size = Some((cols, rows));
        self.resize(cols, rows);
    }

    pub fn tmux_io(&self) -> Option<&TmuxIo> {
        match &self.pty {
            Some(Io::Tmux(t)) => Some(t),
            _ => None,
        }
    }

    /// The host session this pane runs in, and the output offset read so far.
    pub fn host_session(&self) -> Option<(u64, u64)> {
        match &self.pty {
            Some(Io::Hosted(p)) => Some((p.id(), p.offset())),
            _ => None,
        }
    }

    /// A forced repaint: the pty one column wider, then back (a reattach whose output was
    /// cut short, or an adopted orphan).
    pub fn jiggle(&mut self) {
        let (cols, rows) = (self.screen.term().columns() as u16, self.screen.term().screen_lines() as u16);
        if let Some(io) = &self.pty {
            for (c, r) in crate::reattach::jiggle(cols, rows) {
                if let Err(e) = io.resize(c, r) {
                    self.error = Some(e);
                }
            }
        }
    }

    /// Quit with jobs kept: the shell stays in the host when the pane goes.
    pub fn detach(&mut self) {
        if let Some(Io::Hosted(p)) = &mut self.pty {
            p.detach();
        }
    }

    fn with_io(id: PaneId, io: Io, cols: u16, rows: u16) -> Pane {
        let mut screen = Screen::new(cols as usize, rows as usize);
        screen.pty_backed = true;
        Pane { id, screen, pty: Some(io), exited: None, error: None, profile: String::new(), live: None, session_note: String::new(), buffer: InputBuffer::default(), frozen: false, suppress_output: false, got_output: false, coprocess: None, co_out: Default::default(), co_end: None, tmux_hook: Default::default(), tmux_q: Vec::new(), gateway: false, tmux_size: None, screen_filter: None }
    }

    /// Feeds all waiting output into the screen. Returns true if anything changed.
    pub fn pump(&mut self) -> bool {
        let Some(pty) = &self.pty else { return false };
        let mut changed = false;
        while let Read::Data(chunk) = pty.try_read() {
            changed = true;
            self.got_output = true;
            // The coprocess gets every byte, before parsing (iTerm2 tees the raw read).
            if let Some(c) = self.coprocess.as_mut() {
                c.write(&chunk);
            }
            // tmux control mode is found before anything can drop output.
            let dropping = self.suppress_output || self.coprocess.as_ref().is_some_and(|c| c.mute);
            for out in self.tmux_hook.feed(&chunk) {
                match out {
                    crate::tmux_hook::Out::Screen(_) if dropping => {}
                    crate::tmux_hook::Out::Screen(bytes) if self.tmux_q.is_empty() => {
                        let bytes = match self.screen_filter.as_mut() {
                            Some(f) => f.feed(&bytes),
                            None => bytes,
                        };
                        let reply = self.live.as_mut().unwrap_or(&mut self.screen).feed(&bytes);
                        // tmux answers queries for its panes itself, but OSC 4 when this
                        // client is the elected one.
                        let reply = match pty {
                            Io::Tmux(t) if t.answers_osc4.get() => crate::tmux_clients::osc4_answers(&reply),
                            Io::Tmux(_) => Vec::new(),
                            _ => reply,
                        };
                        if !reply.is_empty() {
                            if let Err(e) = pty.write(&reply) {
                                self.error = Some(e);
                            }
                        }
                    }
                    other => self.tmux_q.push(other),
                }
            }
        }
        if let Some(c) = self.coprocess.as_mut() {
            // Its output goes to the shell as typed, while the shell keeps up.
            loop {
                let budget = crate::coprocess::budget(pty.pending());
                if budget == 0 {
                    break;
                }
                if self.co_out.is_empty() {
                    match c.try_output() {
                        Some(o) => self.co_out.push(o),
                        None => break,
                    }
                }
                let bytes = self.co_out.take(budget);
                if let Err(e) = pty.write(&bytes) {
                    self.error = Some(e);
                    break;
                }
            }
            if self.co_out.is_empty() && c.finished() {
                self.co_end = Some((c.command.clone(), c.stderr()));
                self.coprocess = None;
                changed = true;
            }
        }
        if self.exited.is_none() {
            if let Some(code) = pty.exit_code() {
                self.exited = Some(code);
                changed = true;
            }
        }
        // The session ended: its coprocess ends too (iTerm2's mainProcessDidTerminate).
        if self.exited.is_some() && self.coprocess.is_some() {
            self.stop_coprocess();
        }
        changed
    }

    /// What the tmux hook found since the last call.
    pub fn take_tmux(&mut self) -> Vec<crate::tmux_hook::Out> {
        std::mem::take(&mut self.tmux_q)
    }

    pub fn has_tmux(&self) -> bool {
        !self.tmux_q.is_empty()
    }

    /// Output that waited behind a tmux line.
    pub fn feed_output(&mut self, bytes: &[u8]) {
        let reply = self.own_mut().feed(bytes);
        if !reply.is_empty() && self.tmux_io().is_none() {
            self.reply(&reply);
        }
    }

    /// Ends tmux control mode here without an `%exit`.
    pub fn unhook_tmux(&mut self) {
        self.tmux_hook.unhook();
    }

    /// What the user sends: keys, pastes. Held while Buffer Input is on.
    pub fn input(&mut self, bytes: &[u8]) {
        if self.gateway {
            return;
        }
        self.suppress_output = false;
        if let Some(now) = self.buffer.send(bytes) {
            self.reply(&now);
        }
    }

    /// Turns Buffer Input on or off; off sends what waited.
    pub fn set_buffering(&mut self, on: bool) {
        if on {
            self.buffer.on = true;
        } else {
            let waited = self.buffer.release();
            if !waited.is_empty() {
                self.reply(&waited);
            }
        }
    }

    /// Copy with Control Sequences of the selection the user made (in a Filter or Zoom
    /// view, the view's).
    pub fn sgr_copy(&self) -> String {
        crate::styled_copy::sgr_text(&self.screen)
    }

    /// The selection's styled rows, for Copy with Styles.
    pub fn styled_rows(&self) -> Vec<crate::styled_copy::Row> {
        crate::styled_copy::runs(&self.screen)
    }

    /// Focus reporting (DECSET 1004): a report, so Buffer Input never holds it.
    pub fn focus_report(&mut self, gained: bool) {
        self.reply(if gained { b"\x1b[I" } else { b"\x1b[O" });
    }

    /// Bytes Buffer Input holds now.
    pub fn waiting(&self) -> usize {
        self.buffer.queue.len()
    }

    /// Undo Close: a waiting pane stops reading its program's output.
    pub fn set_paused(&mut self, paused: bool) {
        if let Some(pty) = &self.pty {
            pty.set_paused(paused);
        }
    }

    /// Starts a coprocess. One at a time: a second is an error.
    pub fn run_coprocess(&mut self, command: &str, mute: bool, cwd: &std::path::Path, notify: Notify) -> Result<(), String> {
        if self.coprocess.is_some() {
            return Err("a coprocess is already running in this session".into());
        }
        if self.pty.is_none() {
            return Err("the session has ended".into());
        }
        self.coprocess = Some(crate::coproc_win::Coprocess::start(command, mute, cwd, notify)?);
        self.co_out = Default::default();
        Ok(())
    }

    /// Session > Stop Coprocess (and pane close, through Drop).
    pub fn stop_coprocess(&mut self) {
        if let Some(mut c) = self.coprocess.take() {
            c.stop();
        }
        self.co_out = Default::default();
    }

    pub fn coprocess_command(&self) -> Option<&str> {
        self.coprocess.as_ref().map(|c| c.command.as_str())
    }

    /// A coprocess that ended since the last call: (command, stderr).
    pub fn take_coprocess_end(&mut self) -> Option<(String, String)> {
        self.co_end.take()
    }

    /// Terminal output a coprocess did not read in time and lost.
    pub fn coprocess_dropped(&self) -> usize {
        self.coprocess.as_ref().map_or(0, |c| c.dropped())
    }

    /// The shell's process id, while it has a pty.
    pub fn shell_pid(&self) -> Option<u32> {
        self.pty.as_ref().and_then(|p| p.pid())
    }

    /// What the terminal answers the program (reports): never held by Buffer Input.
    pub fn reply(&mut self, bytes: &[u8]) {
        if let Some(pty) = &self.pty {
            if let Err(e) = pty.write(bytes) {
                self.error = Some(e);
            }
        }
    }

    pub fn paste(&mut self, text: &str) {
        let bytes = self.paste_payload(text);
        self.input(&bytes);
    }

    /// Resizes the screen and the pty. A pane squeezed to nothing keeps 2x1. True when the
    /// size changed.
    pub fn resize(&mut self, cols: u16, rows: u16) -> bool {
        let (cols, rows) = self.tmux_size.unwrap_or((cols, rows));
        let (cols, rows) = (cols.max(2), rows.max(1));
        if (self.own().term().columns(), self.own().term().screen_lines()) == (cols as usize, rows as usize) {
            return false;
        }
        if !self.frozen {
            self.screen.resize(cols as usize, rows as usize);
        }
        if let Some(live) = &mut self.live {
            live.resize(cols as usize, rows as usize);
        }
        if let Some(pty) = &self.pty {
            if let Err(e) = pty.resize(cols, rows) {
                self.error = Some(e);
            }
        }
        true
    }

    /// iTerm2's Clear to Last Mark: clears from after the last mark above the prompt, then
    /// clears ConPTY's buffer too (keeping the cursor row), so both agree.
    pub fn clear_to_last_mark(&mut self) {
        self.end_view();
        let lines: Vec<i32> = self.screen.marks_in_order().iter().map(|(l, _)| *l).collect();
        let cursor = self.screen.term().grid().cursor.point.line.0;
        let start = crate::marks::clear_start(&lines, self.screen.current_prompt_line(), cursor);
        self.screen.clear_from(start);
        self.screen.term_mut().selection = None;
        if let Some(pty) = &self.pty {
            if let Err(e) = pty.clear() {
                self.error = Some(e);
            }
        }
    }

    /// iTerm2's Clear Buffer: the screen and ConPTY both clear, keeping the cursor line.
    pub fn clear(&mut self) {
        self.end_view();
        self.screen.clear(false);
        if let Some(pty) = &self.pty {
            if let Err(e) = pty.clear() {
                self.error = Some(e);
            }
        }
    }

    pub fn scroll(&mut self, scroll: Scroll) {
        self.screen.term_mut().scroll_display(scroll);
    }

    /// What a paste sends: bracketed when the pane's own screen asks for it.
    pub fn paste_payload(&self, text: &str) -> Vec<u8> {
        let bracketed = self.own().term().mode().contains(TermMode::BRACKETED_PASTE);
        paste_bytes(text, bracketed)
    }

    /// New colors and cursor for the pane, on its own screen and any view.
    pub fn restyle(&mut self, palette: crate::palette::Palette, look: crate::screen::Look) {
        if let Some(live) = &mut self.live {
            live.set_palette(palette.clone());
            live.set_look(look.clone());
        }
        self.screen.set_palette(palette);
        self.screen.set_look(look);
        // A background that crossed between dark and light is reported (mode 2031).
        let reply = self.screen.take_replies();
        if !reply.is_empty() {
            self.reply(&reply);
        }
    }

    /// When the pane's synchronized update must be drawn anyway.
    pub fn sync_deadline(&self) -> Option<std::time::Instant> {
        self.live.as_ref().unwrap_or(&self.screen).sync_deadline()
    }

    /// Draws a synchronized update whose time ran out. True when one was.
    pub fn flush_due_sync(&mut self, now: std::time::Instant) -> bool {
        let screen = self.live.as_mut().unwrap_or(&mut self.screen);
        if !screen.sync_deadline().is_some_and(|t| t <= now) {
            return false;
        }
        let reply = screen.flush_sync();
        if !reply.is_empty() {
            self.reply(&reply);
        }
        true
    }

    pub fn set_cell_px(&mut self, w: u16, h: u16) {
        if let Some(live) = &mut self.live {
            live.set_cell_px(w, h);
        }
        self.screen.set_cell_px(w, h);
    }

    /// The pane's own screen, also while a view shows.
    pub fn own(&self) -> &Screen {
        self.live.as_ref().unwrap_or(&self.screen)
    }

    pub fn own_mut(&mut self) -> &mut Screen {
        self.live.as_mut().unwrap_or(&mut self.screen)
    }

    /// Shows a read-only view in place of the pane's screen.
    pub fn show_view(&mut self, view: Screen) {
        if self.frozen {
            return;
        }
        let own = std::mem::replace(&mut self.screen, view);
        if self.live.is_none() {
            self.live = Some(own);
        }
    }

    /// Make Screenshot: `snap` shows in place of any view until `thaw`.
    pub fn freeze(&mut self, snap: Screen) {
        self.frozen = false;
        self.end_view();
        self.show_view(snap);
        self.frozen = true;
    }

    /// The panel closed: back to the pane's own screen.
    pub fn thaw(&mut self) {
        self.frozen = false;
        self.end_view();
    }

    /// Back to the pane's own screen, at the scroll position it had.
    pub fn end_view(&mut self) {
        if self.frozen {
            return;
        }
        if let Some(own) = self.live.take() {
            self.screen = own;
        }
    }
}

impl Drop for Pane {
    /// Closing a pty can take about 2 s on the Windows copy of ConPTY (milestone 0),
    /// so it happens on its own thread.
    fn drop(&mut self) {
        if let Some(pty) = self.pty.take() {
            std::thread::spawn(move || drop(pty));
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn buffered_input_waits_for_release() {
        let mut b = super::InputBuffer::default();
        assert_eq!(b.send(b"ls\r"), Some(b"ls\r".to_vec()), "off: sent at once");
        b.on = true;
        assert_eq!(b.send(b"echo "), None);
        assert_eq!(b.send(b"hi\r"), None);
        assert_eq!(b.release(), b"echo hi\r");
        assert!(!b.on);
        assert_eq!(b.send(b"x"), Some(b"x".to_vec()));
        assert_eq!(b.release(), b"", "nothing waited");
    }

    use super::*;
    use crate::conpty::{self, Source, dev_vendor_dir};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn tmux_io_queues() {
        let mut p = Pane::tmux(5, 1, 3, 20, 4);
        let io = p.tmux_io().unwrap();
        assert_eq!((io.gateway, io.pane), (1, 3));
        io.push_output(b"hi");
        assert!(p.pump());
        assert_eq!(p.screen.term().grid()[alacritty_terminal::index::Line(0)][alacritty_terminal::index::Column(1)].c, 'i');
        p.input(b"ls\r");
        assert_eq!(p.tmux_io().unwrap().take_input(), b"ls\r", "typing waits for send-keys");
        assert!(p.tmux_io().unwrap().take_input().is_empty());
        assert_eq!(p.shell_pid(), None, "tmux's process is not Tandem's");
        assert!(!p.pump(), "nothing new");
        assert!(!p.resize(100, 50), "a tmux pane keeps tmux's size");
        p.set_tmux_size(30, 5);
        assert_eq!((p.screen.term().columns(), p.screen.term().screen_lines()), (30, 5));
        p.tmux_io().unwrap().end();
        p.pump();
        assert_eq!(p.exited, Some(0));
    }

    #[test]
    fn tmux_panes_do_not_answer_queries() {
        let mut p = Pane::tmux(5, 1, 3, 20, 4);
        p.tmux_io().unwrap().push_output(b"\x1b[c\x1b[6n");
        p.pump();
        p.feed_output(b"\x1b[c");
        assert!(p.tmux_io().unwrap().take_input().is_empty(), "tmux answers for its panes; a second answer would be typed");
    }

    #[test]
    fn gateway_panes_take_no_input() {
        let mut p = Pane { id: 1, screen: Screen::new(10, 4), pty: None, exited: None, error: None, profile: String::new(), live: None, session_note: String::new(), buffer: InputBuffer::default(), frozen: false, suppress_output: false, got_output: false, coprocess: None, co_out: Default::default(), co_end: None, tmux_hook: Default::default(), tmux_q: Vec::new(), gateway: false, tmux_size: None, screen_filter: None };
        p.buffer.on = true;
        p.input(b"a");
        p.gateway = true;
        p.input(b"b");
        assert_eq!(p.buffer.release(), b"a", "a tmux gateway's pty carries only tmux commands");
    }

    #[test]
    fn frozen_pane_keeps_its_copy() {
        let mut own = Screen::new(10, 4);
        own.feed(b"live");
        let mut p = Pane { id: 1, screen: own, pty: None, exited: None, error: None, profile: String::new(), live: None, session_note: String::new(), buffer: InputBuffer::default(), frozen: false, suppress_output: false, got_output: false, coprocess: None, co_out: Default::default(), co_end: None, tmux_hook: Default::default(), tmux_q: Vec::new(), gateway: false, tmux_size: None, screen_filter: None };
        let mut snap = Screen::new(10, 4);
        snap.feed(b"frozen");
        p.freeze(snap);
        p.resize(6, 3);
        assert_eq!(p.screen.term().columns(), 10, "the frozen copy keeps its size (redactions stay on their lines)");
        assert_eq!(p.own().term().columns(), 6, "the live screen follows the pane");
        p.show_view(Screen::new(10, 4));
        p.end_view();
        assert!(p.screen.line_text(0).starts_with("frozen"), "Filter and Zoom leave the frozen copy alone");
        p.thaw();
        assert!(p.screen.line_text(0).starts_with("live"));
        assert!(!p.frozen);
    }

    fn start(cmd: &str) -> Pane {
        let api = conpty::load(&Source::Shipped(dev_vendor_dir())).unwrap();
        Pane::start(1, &api, cmd, None, 80, 24, Arc::new(|| {})).unwrap()
    }

    fn pump_until(p: &mut Pane, done: impl Fn(&Pane) -> bool) -> bool {
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            p.pump();
            if done(p) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn clear_buffer_keeps_conpty_in_step() {
        let mut p = start("cmd.exe");
        assert!(pump_until(&mut p, |p| p.screen.text().contains('>')), "no prompt");
        p.input(b"for /l %i in (1,1,30) do @echo L%i\r");
        assert!(pump_until(&mut p, |p| {
            let t = p.screen.all_text();
            t.contains("L30\n") && t.trim_end().ends_with('>')
        }), "no output");
        p.clear();
        assert!(!p.screen.all_text().contains("L29"));
        // ConPTY must clear too, or it keeps drawing where its own cursor was: near the
        // bottom of the screen.
        // The clear reaches ConPTY on its signal pipe, not in order with typed input, so
        // give it a moment as a person would.
        std::thread::sleep(Duration::from_millis(300));
        p.input(b"echo MARK\r");
        assert!(pump_until(&mut p, |p| p.screen.all_text().matches("MARK").count() >= 2), "no echo");
        let rows: Vec<usize> = p.screen.text().lines().enumerate().filter(|(_, l)| l.contains("MARK")).map(|(i, _)| i).collect();
        assert!(rows.iter().all(|&r| r < 4), "MARK on rows {rows:?}:\n{}", p.screen.all_text());
        assert!(p.error.is_none(), "{:?}", p.error);
    }

    #[test]
    fn the_pwsh_script_emits_prompt_marks() {
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/shell_integration/tandem_shell_integration.ps1");
        // Windows PowerShell 5.1 is on every Windows machine, and is the older of the two
        // PowerShells the script supports.
        let cmd = format!("powershell.exe -NoProfile -NoLogo -Command \". '{}'; prompt; Start-Sleep -Milliseconds 300\"", script.display());
        let mut p = start(&cmd);
        assert!(pump_until(&mut p, |p| p.exited.is_some()), "pwsh did not finish");
        p.pump();
        assert_eq!(p.screen.marks_in_order().len(), 1, "{}", p.screen.all_text());
        assert!(p.screen.shell.cwd.as_deref().is_some_and(|c| c.contains(':')), "{:?}", p.screen.shell.cwd);
        assert_eq!(p.screen.shell.shell.as_deref(), Some("powershell"));
    }

    #[test]
    fn clear_to_last_mark_keeps_conpty_in_step() {
        let mut p = start("cmd.exe");
        assert!(pump_until(&mut p, |p| p.screen.text().contains('>')), "no prompt");
        p.input(b"for /l %i in (1,1,30) do @echo L%i\r");
        assert!(pump_until(&mut p, |p| {
            let t = p.screen.all_text();
            t.contains("L30\n") && t.trim_end().ends_with('>')
        }), "no output");
        let top = -(p.screen.term().grid().history_size() as i32);
        let line = (top..24).find(|l| p.screen.line_text(*l) == "L20").expect("L20 row");
        p.screen.set_manual_mark(line, None);
        p.clear_to_last_mark();
        let t = p.screen.all_text();
        assert!(t.contains("L20\n") && !t.contains("L21"), "{t}");
        std::thread::sleep(Duration::from_millis(300));
        p.input(b"echo MARK\r");
        assert!(pump_until(&mut p, |p| p.screen.all_text().matches("MARK").count() >= 2), "no echo");
        let rows: Vec<usize> = p.screen.text().lines().enumerate().filter(|(_, l)| l.contains("MARK")).map(|(i, _)| i).collect();
        assert!(rows.iter().all(|&r| r < 4), "MARK on rows {rows:?}:\n{}", p.screen.all_text());
    }

    #[test]
    fn the_pwsh_script_keeps_the_prompt_state() {
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/shell_integration/tandem_shell_integration.ps1");
        // A prompt that shows the last status, as starship and oh-my-posh do; a failed
        // command; then the prompt drawn twice with no command between (PSReadLine's
        // InvokePrompt does that).
        let body = format!(
            "function prompt {{ if ($?) {{ 'ok> ' }} else {{ 'fail> ' }} }}; . '{}'; Write-Error x -ErrorAction SilentlyContinue; prompt; prompt; Start-Sleep -Milliseconds 300",
            script.display()
        );
        let mut p = start(&format!("powershell.exe -NoProfile -NoLogo -Command \"{body}\""));
        assert!(pump_until(&mut p, |p| p.exited.is_some()), "powershell did not finish");
        p.pump();
        let text = p.screen.all_text();
        assert!(text.contains("fail>"), "the original prompt lost the failure: {text}");
        // The second drawing sent no command end, so the first prompt has no code.
        let marks = p.screen.marks_in_order();
        assert!(marks.iter().all(|(_, m)| m.code.is_none()), "{marks:?}");
    }

    #[test]
    fn plain_paste_uses_carriage_returns() {
        assert_eq!(paste_bytes("a\r\nb\nc", false), b"a\rb\rc");
    }

    #[test]
    fn bracketed_paste_wraps() {
        assert_eq!(paste_bytes("hi", true), b"\x1b[200~hi\x1b[201~");
    }

    #[test]
    fn bracketed_paste_strips_the_end_marker() {
        assert_eq!(paste_bytes("a\x1b[201~rm -rf x\n", true), b"\x1b[200~a[201~rm -rf x\r\x1b[201~");
    }

    #[test]
    fn bracketed_paste_strips_a_nested_end_marker() {
        // One pass of removal would join the halves into a live end marker.
        assert_eq!(paste_bytes("\x1b[20\x1b[201~1~rm -rf x", true), b"\x1b[200~[20[201~1~rm -rf x\x1b[201~");
    }

    #[test]
    fn pump_shows_output() {
        let mut p = start("cmd.exe /q /k echo pane-ok");
        assert!(pump_until(&mut p, |p| p.screen.text().contains("pane-ok")), "{}", p.screen.text());
    }

    #[test]
    fn exit_code_is_reported() {
        let mut p = start("cmd.exe /c exit 4");
        assert!(pump_until(&mut p, |p| p.exited.is_some()));
        assert_eq!(p.exited, Some(4));
    }

    #[test]
    fn resize_to_zero_keeps_a_minimum() {
        let mut p = start("cmd.exe /q /k");
        p.resize(0, 0);
        assert_eq!(p.error, None);
        assert_eq!((p.screen.term().columns(), p.screen.term().screen_lines()), (2, 1));
    }

    #[test]
    fn drop_does_not_block() {
        let p = start("cmd.exe /q /k");
        let t = Instant::now();
        drop(p);
        assert!(t.elapsed() < Duration::from_millis(500), "{:?}", t.elapsed());
    }

    #[test]
    fn a_view_keeps_the_panes_own_modes_and_style() {
        let mut p = start("cmd.exe");
        p.own_mut().feed(b"\x1b[?2004h");
        let view = crate::synthetic::view(&["x".to_string()], p.own());
        p.show_view(view);
        assert!(p.paste_payload("a\nb").starts_with(b"\x1b[200~"));
        let mut red = crate::palette::Palette::default();
        red.bg = crate::palette::rgb(0x80, 0, 0);
        p.restyle(red.clone(), crate::screen::Look::default());
        p.end_view();
        assert_eq!(p.screen.palette.bg, red.bg);
    }
    #[test]
    fn copies_read_the_screen_the_user_selects_in() {
        use alacritty_terminal::index::{Column, Line, Point, Side};
        use alacritty_terminal::selection::{Selection, SelectionType};
        let mut p = start("cmd /c ping -n 3 127.0.0.1 >nul");
        let mut view = Screen::new(20, 4);
        view.feed(b"from the view");
        let mut sel = Selection::new(SelectionType::Simple, Point::new(Line(0), Column(0)), Side::Left);
        sel.update(Point::new(Line(0), Column(12)), Side::Right);
        view.term_mut().selection = Some(sel);
        let own = std::mem::replace(&mut p.screen, view);
        p.live = Some(own);
        assert!(p.sgr_copy().contains("from the view"), "{:?}", p.sgr_copy());
        assert_eq!(p.styled_rows().len(), 1);
    }

    #[test]
    fn focus_reports_are_not_held() {
        let mut p = start("cmd /c ping -n 3 127.0.0.1 >nul");
        p.set_buffering(true);
        p.focus_report(true);
        assert_eq!(p.waiting(), 0);
        p.input(b"x");
        assert_eq!(p.waiting(), 1);
    }

    #[test]
    fn coprocess_types_into_the_shell() {
        let mut p = start("cmd.exe");
        assert!(pump_until(&mut p, |p| p.screen.text().contains('>')), "no prompt");
        p.run_coprocess("echo echo FROM^^CO", false, &std::env::temp_dir(), Arc::new(|| {})).unwrap();
        assert!(p.run_coprocess("echo x", false, &std::env::temp_dir(), Arc::new(|| {})).is_err(), "one at a time");
        assert!(pump_until(&mut p, |p| p.screen.all_text().contains("FROMCO")), "{}", p.screen.all_text());
        assert!(pump_until(&mut p, |p| p.coprocess_command().is_none()), "it ends");
        assert_eq!(p.take_coprocess_end().map(|e| e.0), Some("echo echo FROM^^CO".to_string()));
    }

    #[test]
    fn mute_hides_output_from_the_screen() {
        let mut p = start("cmd.exe");
        assert!(pump_until(&mut p, |p| p.screen.text().contains('>')), "no prompt");
        p.run_coprocess("ping -n 30 127.0.0.1 >NUL", true, &std::env::temp_dir(), Arc::new(|| {})).unwrap();
        p.input(b"echo HID^DEN\r");
        let shown = pump_until(&mut p, |p| p.screen.all_text().contains("HIDDEN"));
        assert!(!shown, "a silent coprocess hides the session's output");
        p.stop_coprocess();
        assert!(p.coprocess_command().is_none());
    }

    #[test]
    fn a_paused_pane_stops_reading() {
        // ping prints a line a second; a paused pane takes none of them until it resumes.
        let mut p = start("ping -n 8 127.0.0.1");
        std::thread::sleep(Duration::from_millis(1200));
        let drain = |p: &Pane| {
            let mut n = 0;
            if let Some(pty) = &p.pty {
                while let Read::Data(c) = pty.try_read() {
                    n += c.len();
                }
            }
            n
        };
        drain(&p);
        p.set_paused(true);
        std::thread::sleep(Duration::from_millis(100));
        drain(&p);
        std::thread::sleep(Duration::from_millis(3000));
        let while_paused = drain(&p);
        p.set_paused(false);
        std::thread::sleep(Duration::from_millis(2500));
        let after = drain(&p);
        assert_eq!(while_paused, 0, "nothing read while paused");
        assert!(after > 0, "reading resumes");
    }
}

//! One terminal pane: a shell in a pty, and its screen.

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::term::TermMode;

use std::time::{Duration, Instant};

use crate::composer;
use crate::conpty::Api;
use crate::dvr;
use crate::layout::PaneId;
use crate::pty::{self, Notify, Pty, Read};
use crate::ssh_transport::Transport;
use crate::replay;
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

/// Instant Replay records at most one frame per pane in this time (a display refresh);
/// damage in between waits for the next frame. The shortest of `cadence::frame_gap`'s.
pub const FRAME_GAP: Duration = crate::cadence::FAST_GAP;

/// The recorder's capacity for `IRMemory` megabytes (`S:121-122`), within the setting's
/// 0 to 1000.
pub fn capacity_for(megabytes: i64) -> u64 {
    megabytes.clamp(0, 1000) as u64 * 1024 * 1024
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
            // A pause waits out a console mode read in flight, so none runs on a paused console
            // (the job watcher checks the pause under `CONSOLE`), as tandem-host does.
            Io::Local(p) if paused => p.pause_after_reads(),
            Io::Local(p) => p.set_paused(false),
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
    /// The pane's own reference in a chat: unique across panes and launches, unlike `id`, which
    /// restarts every launch (a chat bound to a reused id would control an unrelated pane).
    pub session_guid: String,
    pub screen: Screen,
    pty: Option<Io>,
    pub exited: Option<u32>,
    pub error: Option<String>,
    /// The Guid of the profile this pane runs.
    pub profile: String,
    /// The command line the pane started with (its profile's or an override), for the tab
    /// tooltip; empty for a pane Tandem did not start a command in.
    pub command: String,
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
    /// SSH integration's entry (the DCS 2000p hook), ahead of the tmux hook.
    ssh_hook: crate::ssh_hook::Hook,
    /// What the SSH hook found, in order, for the app. Output behind a conductor token waits
    /// here too, so it lands after what the token does.
    ssh_q: Vec<crate::ssh_hook::Out>,
    /// The innermost SSH conductor; its parents hang off it (iTerm2's `_conductor`).
    pub conductor: Option<Box<crate::conductor::Conductor>>,
    /// Session writes held while the conductor sets up (`_sshWriteQueue`, `PS:4191-4197`),
    /// sent when it unhooks.
    ssh_held: Vec<u8>,
    /// A screen-like TERM: italics as reverse video (iTerm2's translateFromScreenTerminal).
    pub screen_filter: Option<crate::screenterm::Filter>,
    /// A tmux gateway: its pty carries tmux commands only, so no input reaches it.
    pub gateway: bool,
    /// A tmux pane's size is tmux's, whatever its rect (milestone 8c asks tmux for others).
    tmux_size: Option<(u16, u16)>,
    /// When a newline (CR or LF) was last sent to the session (`activityInfo.lastNewline`).
    pub last_newline: Option<std::time::Instant>,
    /// The Composer (milestone 9l): a native multi-line command editor for this session.
    pub composer: composer::Composer,
    /// Instant Replay's recording of the pane's own screen, sized from `IRMemory` when the
    /// pane is installed.
    pub dvr: dvr::Dvr,
    /// When the last frame was captured.
    pub replay_at: Option<Instant>,
    /// Output bytes a second, for the recording cadence (iTerm2's token executor estimate).
    pub throughput: crate::cadence::Throughput,
    /// Instant Replay's session while this pane shows a frame from its DVR instead of
    /// its live screen.
    pub replay: Option<replay::Replay>,
    /// The password manager's echo probe (`iTermEchoProbe`): fed the raw output before the
    /// screen sees it, once tmux control mode is found (a gateway never probes).
    pub echo_probe: Option<crate::echo_probe::Probe>,
    /// Focus reports are held while this is above 0 (`incrementDisableFocusReporting:`): the
    /// password manager opening and closing, a password being entered.
    pub no_focus_reports: u32,
    /// The shell's console at a password prompt, debounced over real samples (the job watcher
    /// for a local pane, tandem-host for a hosted one): the key cursor, the click on it, and
    /// the automatic opening follow it. The live session's state, whatever view shows.
    pub password_input: bool,
    /// The Python API cookie this session's shell got in `ITERM2_COOKIE` (a Session cookie in
    /// the app's jar, removed when the pane is dropped for good). A secret: never logged.
    pub cookie: Option<String>,
    /// Reading is paused while an inline file waits for the person to allow its display.
    inline_paused: bool,
    /// Watches for a reported drag followed by Copy (iTerm2's frustration detector).
    pub frustration: crate::mouse_frustration::Detector,
    /// The named arrangement this pane was restored from and its session's id there, for
    /// the arrangement's Repair and Assign Profile (`_originatingArrangementName`).
    pub arrangement: Option<(String, String)>,
    /// SSH integration: the connecting queue (milestone 14).
    pub ssh: crate::ssh_session::PaneSsh,
}

/// A fresh session reference for a pane.
pub fn new_session_guid() -> String {
    uuid::Uuid::new_v4().to_string().to_uppercase()
}

impl Pane {
    /// Takes `guid` as the pane's reference: a restored pane keeps the one it saved, and a
    /// restarted session keeps its own (iTerm2 keeps a session's guid, `PTYSession.m:2403-2420`).
    pub fn keep_reference(&mut self, guid: &str) {
        self.session_guid = guid.to_string();
    }

    pub fn start(id: PaneId, api: &Api, cmdline: &str, cwd: Option<&std::path::Path>, env: &[String], cols: u16, rows: u16, notify: Notify) -> Result<Pane, String> {
        Pane::start_transport(id, api, Transport::Conpty, cmdline, cwd, env, cols, rows, notify)
    }

    /// `start` on an SSH transport (`ssh_transport`); `Transport::Conpty` is every other pane's.
    #[allow(clippy::too_many_arguments)]
    pub fn start_transport(id: PaneId, api: &Api, t: Transport, cmdline: &str, cwd: Option<&std::path::Path>, env: &[String], cols: u16, rows: u16, notify: Notify) -> Result<Pane, String> {
        let (cols, rows) = (cols.max(2), rows.max(1));
        let pty = pty::spawn_transport(api, t, cmdline, cwd, env, cols, rows, notify)?;
        Ok(Pane::with_io(id, Io::Local(pty), cols, rows))
    }

    /// A pane whose shell the session host runs.
    pub fn start_hosted(id: PaneId, host: &crate::host_client::Client, cmdline: &str, cwd: Option<&std::path::Path>, env: &[String], cols: u16, rows: u16, notify: Notify) -> Result<Pane, String> {
        Pane::start_hosted_transport(id, host, Transport::Conpty, cmdline, cwd, env, cols, rows, notify)
    }

    /// `start_hosted` on an SSH transport; a host too old for it gives
    /// `Err(host_client::HOST_TOO_OLD)`.
    #[allow(clippy::too_many_arguments)]
    pub fn start_hosted_transport(id: PaneId, host: &crate::host_client::Client, t: Transport, cmdline: &str, cwd: Option<&std::path::Path>, env: &[String], cols: u16, rows: u16, notify: Notify) -> Result<Pane, String> {
        let (cols, rows) = (cols.max(2), rows.max(1));
        let pty = host.launch_transport(t, cmdline, cwd, env, cols, rows, notify)?;
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

    /// A loaded Instant Replay recording's pane (iTerm2's synthetic session, `RC:113-114`):
    /// no pty, so nothing runs in it, it never exits and it reads no output.
    pub fn recording(id: PaneId, cols: u16, rows: u16) -> Pane {
        Pane::with(id, None, cols.max(2), rows.max(1))
    }

    fn with_io(id: PaneId, io: Io, cols: u16, rows: u16) -> Pane {
        Pane::with(id, Some(io), cols, rows)
    }

    fn with(id: PaneId, pty: Option<Io>, cols: u16, rows: u16) -> Pane {
        let mut screen = Screen::new(cols as usize, rows as usize);
        screen.pty_backed = pty.is_some();
        Pane { id, session_guid: new_session_guid(), screen, pty, exited: None, error: None, profile: String::new(), command: String::new(), live: None, session_note: String::new(), buffer: InputBuffer::default(), frozen: false, suppress_output: false, got_output: false, coprocess: None, co_out: Default::default(), co_end: None, tmux_hook: Default::default(), tmux_q: Vec::new(), ssh_hook: Default::default(), ssh_q: Vec::new(), conductor: None, ssh_held: Vec::new(), gateway: false, tmux_size: None, screen_filter: None, last_newline: None, composer: composer::Composer::default(), dvr: dvr::Dvr::new(capacity_for(4)), replay_at: None, throughput: Default::default(), replay: None, echo_probe: None, no_focus_reports: 0, password_input: false, cookie: None, inline_paused: false, frustration: Default::default(), arrangement: None, ssh: Default::default() }
    }

    /// Feeds all waiting output into the screen. Returns true if anything changed.
    pub fn pump(&mut self) -> bool {
        let Some(pty) = &self.pty else { return false };
        let mut chunks = Vec::new();
        while let Read::Data(chunk) = pty.try_read() {
            self.got_output = true;
            self.throughput.add(Instant::now(), chunk.len());
            // The coprocess gets every byte, before parsing (iTerm2 tees the raw read).
            if let Some(c) = self.coprocess.as_mut() {
                c.write(&chunk);
            }
            chunks.push(chunk);
        }
        let mut changed = !chunks.is_empty();
        // SSH integration, then tmux control mode, are found before anything can drop output.
        for chunk in chunks {
            self.hook_output(&chunk);
        }
        if self.coprocess.is_some() {
            // Its output goes to the shell as typed, while the shell keeps up.
            loop {
                let Some(pty) = &self.pty else { break };
                let budget = crate::coprocess::budget(pty.pending());
                if budget == 0 {
                    break;
                }
                if self.co_out.is_empty() {
                    match self.coprocess.as_mut().and_then(|c| c.try_output()) {
                        Some(o) => self.co_out.push(o),
                        None => break,
                    }
                }
                let bytes = self.co_out.take(budget);
                if !self.coprocess_output(&bytes) {
                    break;
                }
            }
            if let Some(c) = self.coprocess.as_mut() {
                if self.co_out.is_empty() && c.finished() {
                    self.co_end = Some((c.command.clone(), c.stderr()));
                    self.coprocess = None;
                    changed = true;
                }
            }
        }
        let Some(pty) = &self.pty else { return changed };
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

    /// One read through the hooks. The SSH hook sees it first, unless tmux control mode is on:
    /// iTerm2's DCS parser is hooked by the tmux parser for the whole control session, so a
    /// `DCS 2000p` inside it (a reply line printing a user option) starts nothing
    /// (`DP:513-538`). A read that starts control mode goes to the SSH hook only through the
    /// tmux start; the rest is tmux's. While a conductor is hooked it takes everything, a
    /// tmux start included, as iTerm2's conductor parser does.
    fn hook_output(&mut self, chunk: &[u8]) {
        let start = crate::tmux_hook::START;
        let cut = match self.ssh_hook.active() || self.tmux_hook.active() {
            true => chunk.len(),
            false => chunk.windows(start.len()).position(|w| w == start).map_or(chunk.len(), |k| k + start.len()),
        };
        let (head, rest) = chunk.split_at(cut);
        for part in [head, rest].into_iter().filter(|part| !part.is_empty()) {
            if self.tmux_hook.active() && !self.ssh_hook.active() && self.ssh_q.is_empty() {
                let mut bytes = self.ssh_hook.take_held();
                bytes.extend_from_slice(part);
                self.route_output(bytes);
            } else {
                for out in self.ssh_hook.feed(part) {
                    match out {
                        crate::ssh_hook::Out::Screen(bytes) if self.ssh_q.is_empty() => self.route_output(bytes),
                        other => self.ssh_q.push(other),
                    }
                }
            }
        }
    }

    /// Output the SSH hook let through: tmux control mode is looked for, then the screen gets
    /// it unless output is dropped, and its reports go back to the session.
    fn route_output(&mut self, bytes: Vec<u8>) {
        let dropping = self.suppress_output || self.coprocess.as_ref().is_some_and(|c| c.mute);
        for out in self.tmux_hook.feed(&bytes) {
            match out {
                crate::tmux_hook::Out::Screen(_) if dropping => {}
                crate::tmux_hook::Out::Screen(bytes) if self.tmux_q.is_empty() => {
                    if let Some(p) = self.echo_probe.as_mut() {
                        p.output(&bytes);
                    }
                    let bytes = match self.screen_filter.as_mut() {
                        Some(f) => f.feed(&bytes),
                        None => bytes,
                    };
                    let reply = self.live.as_mut().unwrap_or(&mut self.screen).feed(&bytes);
                    // A report is a write to the session: it cancels the composer's clear
                    // probe (`PS:16212-16221` to `writeTaskImpl`, `PS:4154-4158`), whether
                    // or not a tmux pane then drops it (iTerm2 cancels before routing).
                    if !reply.is_empty() {
                        self.composer.probe = None;
                    }
                    // tmux answers queries for its panes itself, but OSC 4 when this
                    // client is the elected one.
                    let reply = match &self.pty {
                        Some(Io::Tmux(t)) if t.answers_osc4.get() => crate::tmux_clients::osc4_answers(&reply),
                        Some(Io::Tmux(_)) => Vec::new(),
                        _ => reply,
                    };
                    // A report passes the connecting queue (`PS:16212-16220`) but is held
                    // while the conductor sets up (`PS:4191-4197`).
                    if !reply.is_empty() {
                        self.reply(&reply);
                    }
                }
                other => self.tmux_q.push(other),
            }
        }
    }

    /// The time between this pane's frames now (`cadence::frame_gap`): `visible` when it is
    /// in the tab in front of a window on screen; `last_output` as the activity tracker has
    /// it (output right after a resize ignored), with output since its last tick counting
    /// as now.
    pub fn replay_gap(&mut self, now: Instant, visible: bool, last_output: Option<Instant>) -> Duration {
        let idle = !self.got_output && last_output.is_none_or(|t| now.saturating_duration_since(t) > crate::cadence::IDLE_AFTER);
        let alt = self.own().alt_screen();
        crate::cadence::frame_gap(visible, idle, alt, self.throughput.estimate(now))
    }

    /// Records a frame of the pane's own screen, never a view (`PS:1470-1480`), when it
    /// changed and `gap` has passed since the last capture. True when one was stored. A frame
    /// the recorder could not hold is not captured at all (its damage is dropped). Stamps
    /// never go backward. A tmux gateway has no screen.
    pub fn record_frame(&mut self, now: Instant, now_us: i64, gap: Duration) -> bool {
        if self.gateway || self.replay_at.is_some_and(|t| now.saturating_duration_since(t) < gap) {
            return false;
        }
        let (cols, rows) = (self.own().term().columns(), self.own().term().screen_lines());
        if self.dvr.read_only || dvr::key_bytes(cols, rows) > self.dvr.ring.capacity / 2 {
            self.own_mut().replay_discard();
            return false;
        }
        let Some(c) = self.own_mut().replay_capture() else { return false };
        self.replay_at = Some(now);
        let stamp = if self.dvr.is_empty() { now_us } else { now_us.max(self.dvr.last_timestamp() + 1) };
        matches!(self.dvr.append(&c.rows, &c.clean, c.cursor, stamp), dvr::Appended::Key | dvr::Appended::Diff)
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
        if let Some(p) = self.echo_probe.as_mut() {
            p.output(bytes);
        }
        let reply = self.own_mut().feed(bytes);
        if !reply.is_empty() && self.tmux_io().is_none() {
            self.reply(&reply);
        }
    }

    /// Reading stops while an inline file waits for the person (a tmux pane cannot stop; its
    /// held output is capped by the screen instead).
    pub fn pause_for_inline(&mut self) {
        self.inline_paused = true;
        self.set_paused(true);
    }

    /// The person answered inline file number `serial` (`Screen::inline_serial`): if it still
    /// waits, the screen shows it or drops it and reads the output held after it (an answer
    /// to a file already dropped does nothing). Reading resumes unless another one waits.
    pub fn inline_answered(&mut self, serial: u64, allow: bool) {
        if self.own().inline_serial() == Some(serial) {
            let reply = self.own_mut().inline_answered(allow);
            if !reply.is_empty() && self.tmux_io().is_none() {
                self.reply(&reply);
            }
        }
        self.resume_after_inline();
    }

    /// Reading resumes once no inline file waits: answered, or dropped by the screen when too
    /// much output waited behind it.
    pub fn resume_after_inline(&mut self) {
        if self.inline_paused && !self.own().holding_inline() {
            self.inline_paused = false;
            self.set_paused(false);
        }
    }

    /// Undo Close brought the pane back: reading resumes unless an inline file still waits.
    /// `question_waits` is whether that file's question is still queued or showing; when it
    /// is not, it went while the pane was closed, and the file is asked about again.
    pub fn undo_close_resume(&mut self, question_waits: bool) {
        if self.inline_paused && self.own().holding_inline() {
            if !question_waits {
                self.own_mut().ask_inline_again();
            }
            return;
        }
        self.inline_paused = false;
        self.set_paused(false);
    }

    /// Ends tmux control mode here without an `%exit`.
    pub fn unhook_tmux(&mut self) {
        self.tmux_hook.unhook();
    }

    /// A tmux integration pane's `%output` (`tmux_pump`'s Output arm): pushed and read at
    /// once, since nothing else wakes the loop for it. True when the SSH hook found something
    /// the app must run now (`App::ssh_pump_pane`): the conductor waits on Tandem.
    pub fn tmux_output(&mut self, data: &[u8], answers_osc4: bool) -> bool {
        if let Some(io) = self.tmux_io() {
            io.answers_osc4.set(answers_osc4);
            io.push_output(data);
        }
        self.pump();
        self.has_ssh() || self.ssh_effects_pending()
    }

    /// What the SSH hook found since the last call.
    pub fn take_ssh(&mut self) -> Vec<crate::ssh_hook::Out> {
        std::mem::take(&mut self.ssh_q)
    }

    pub fn has_ssh(&self) -> bool {
        !self.ssh_q.is_empty()
    }

    /// Whether the pane's conductors asked for something not yet applied (keys a coprocess
    /// or a write handed to `send_keys`).
    pub fn ssh_effects_pending(&self) -> bool {
        self.conductor.as_ref().is_some_and(|c| c.has_effects())
    }

    /// Output that waited behind a conductor token.
    pub fn feed_after_ssh(&mut self, bytes: &[u8]) {
        self.route_output(bytes.to_vec());
    }

    /// Where a write to the session goes now (`conductor::route`).
    pub fn ssh_route(&self) -> crate::conductor::Route {
        crate::conductor::route(self.conductor.as_deref())
    }

    /// A typed write to the session in iTerm2's order (`PS:4411-4432`, then `PS:4191-4197`):
    /// the conductor's `send_keys` when it handles keystrokes (the app applies its effects in
    /// `ssh_pump`), else the connecting queue, else held while the conductor sets up, else
    /// the pty.
    pub fn write_session(&mut self, bytes: &[u8]) {
        if self.ssh_route() == crate::conductor::Route::Conductor {
            if let Some(c) = self.conductor.as_mut() {
                c.send_keys(bytes);
            }
        } else if !self.ssh.queue(bytes) {
            self.write_unless_held(bytes);
        }
    }

    /// Held while the conductor sets up, else to the pty, past the connecting queue (iTerm2's
    /// `writeTaskImpl`, `PS:4191-4197`).
    pub fn write_unless_held(&mut self, bytes: &[u8]) {
        if self.ssh_route() != crate::conductor::Route::Direct {
            self.composer.probe = None;
            self.ssh_held.extend_from_slice(bytes);
        } else {
            self.write_no_broadcast(bytes);
        }
    }

    /// A coprocess's output: to the conductor's `send_keys` while the pane has one (`PK:354-361`,
    /// `PS:4639-4642`: held during setup, framed from 14c), else to the pty as typed. False when
    /// the pty refused it.
    pub fn coprocess_output(&mut self, bytes: &[u8]) -> bool {
        if let Some(c) = self.conductor.as_mut() {
            c.send_keys(bytes);
            return true;
        }
        if let Some(pty) = &self.pty {
            if let Err(e) = pty.write(bytes) {
                self.error = Some(e);
                return false;
            }
        }
        true
    }

    /// The writes held while the conductor set up, in order.
    pub fn take_ssh_held(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.ssh_held)
    }

    /// Leaves the conductor without an `unhook`: what follows is output again.
    pub fn unhook_ssh(&mut self) {
        self.ssh_hook.unhook();
    }

    /// The pane's SSH integration level (`conductor::level`).
    pub fn ssh_level(&self) -> u8 {
        crate::conductor::level(self.conductor.as_deref())
    }

    /// unhookSSHConductor (`PS:20179-20204`) for the screen: the innermost conductor goes,
    /// its saved state is restored, and its parent becomes current. The hops are the caller's.
    /// False when there was none.
    pub fn pop_conductor(&mut self) -> bool {
        let Some(c) = crate::conductor::pop(&mut self.conductor) else { return false };
        if let Some(saved) = &c.saved {
            self.own_mut().restore_ssh_saved(saved);
        }
        true
    }

    /// screenEndSSH (`PS:20253-20275`) for the pane: the hops end through the innermost one
    /// with `unique_id`, then one conductor per ended hop is popped, innermost first, so the
    /// outermost popped one's saved state is what remains. The count comes before the end,
    /// which removes the hops it counts. When any hop ended, `END_WRITE` is written and the
    /// banner to show is returned. What was typed while connecting is dropped (14a's
    /// `PaneSsh::end_ssh`, `PS:20270-20273`); so are the writes a popped setup held, unless a
    /// remaining conductor still sets up (controller rulings: dropped as iTerm2 drops them,
    /// never sent to the local shell after ssh exited).
    pub fn end_ssh(&mut self, unique_id: &str) -> Option<String> {
        let n = self.ssh.chain.count_to(unique_id);
        let banner = self.ssh.end_ssh(unique_id);
        for _ in 0..n {
            self.pop_conductor();
        }
        if banner.is_some() {
            // Keys typed for a remote shell that is gone. A remaining conductor that still sets
            // up keeps them for its own unhook, as iTerm2 keeps `_sshWriteQueue` for
            // `screenDidUnhookSSHConductor` (`PS:20176-20180`): they are for that remote.
            if !self.conductor.as_ref().is_some_and(|c| c.queue_writes()) {
                self.ssh_held.clear();
            }
            // writeTaskNoBroadcast (`PS:20269`): held behind them while a conductor sets up.
            self.reply(crate::ssh_session::END_WRITE);
        }
        banner
    }

    /// Drops the pane's innermost conductor and its hop, keeping the two chains in step:
    /// every path that drops a conductor but EndSSH comes here (`PS:20179-20204`).
    pub fn ssh_unhook_conductor(&mut self) {
        if self.pop_conductor() {
            self.ssh.chain.pop();
        }
        debug_assert!(self.ssh_chain_in_step(), "the hops follow the conductors");
    }

    /// conductorAbort (`PS:25587-25601`): the error, the reason and the end banner, then the
    /// conductor and its hop go. Then Tandem's part: the DCS hook lets go, so later output
    /// shows (ruling 14b-3, iTerm2 bugs B4 and B13); the held writes (the raw
    /// `exec_login_shell` line among them) are dropped (iTerm2 strands them); connecting ends
    /// and what was typed meanwhile is dropped (controller ruling: those keys were for the
    /// remote shell, and conductor.sh may still be reading command lines). Typing after this
    /// goes straight to the pty.
    pub fn abort_ssh(&mut self, reason: &str) {
        let location = self.conductor.as_ref().and_then(|c| c.parent.as_ref()).map(|p| p.identity().compact(&crate::ssh_args::local_user()));
        let screen = self.own_mut();
        screen.print_message(crate::conductor::SETUP_ERROR);
        screen.print_message(&crate::conductor::shown(reason));
        screen.append_banner(&crate::ssh_handshake::end_banner(1, location.as_deref()));
        self.ssh_unhook_conductor();
        self.unhook_ssh();
        self.ssh_held.clear();
        // Connecting ends so later typing is not queued behind a dead conductor; its queue goes.
        self.ssh.stop_queueing();
    }

    /// Applies what the pane's conductors asked for, until they ask nothing more. True when
    /// the pane changed in a way the window must redraw.
    pub fn apply_ssh_effects(&mut self) -> bool {
        use crate::conductor::Effect;
        let mut redraw = false;
        loop {
            let effects = self.conductor.as_mut().map(|c| c.take_effects()).unwrap_or_default();
            if effects.is_empty() {
                break;
            }
            'batch: for effect in effects {
                match effect {
                    Effect::Write(bytes) => self.write_no_broadcast(&bytes),
                    Effect::SessionWrite(bytes) => self.write_unless_held(&bytes),
                    // Unframed sendKeys is the session's conductorWrite, which is
                    // writeTaskNoBroadcast (`CD:1854-1862`, `PS:25519-25522`): held while setup
                    // runs, never the connecting queue. A conductor that handles keystrokes
                    // sends them framed (14c); unframed, `reply` would hand them straight back.
                    Effect::Keys(bytes) => {
                        debug_assert!(self.ssh_route() != crate::conductor::Route::Conductor, "unframed keys from a conductor that handles keystrokes");
                        self.reply(&bytes)
                    }
                    // `PS:25614-25617`: what was typed while connecting goes through sendKeys,
                    // ahead of later typing; its effects are applied by the next pass.
                    Effect::StopQueueingInput => {
                        let queued = self.ssh.stop_queueing();
                        if let Some(c) = self.conductor.as_mut() {
                            c.send_keys(&queued);
                        }
                    }
                    // Decision: Tandem sends no Initial Text anywhere yet; it goes here.
                    Effect::SendInitialText => {}
                    Effect::StateChanged => redraw = true,
                    Effect::Notice(text) => {
                        self.own_mut().append_banner(&text);
                        redraw = true;
                    }
                    // The rest of the batch came from the failed setup (keys for its remote,
                    // its writes) and goes with it: after the pop the route is Direct, so
                    // applying it would send those bytes to whatever runs now. A remaining
                    // parent's own later effects come in the next pass.
                    Effect::Abort(reason) => {
                        self.abort_ssh(&reason);
                        redraw = true;
                        break 'batch;
                    }
                }
            }
        }
        redraw
    }

    /// The command timer of the pane's current conductor at `now`, then its effects. True
    /// when the pane must be redrawn.
    pub fn tick_ssh(&mut self, now: std::time::Instant) -> bool {
        let Some(c) = self.conductor.as_mut() else { return false };
        c.tick(now);
        self.apply_ssh_effects()
    }

    /// When the pane's current conductor times out if nothing happens first.
    pub fn ssh_deadline(&self) -> Option<std::time::Instant> {
        self.conductor.as_ref().and_then(|c| c.deadline())
    }

    /// Whether the hop ids are the conductor chain's unique ids, outermost first.
    pub fn ssh_chain_in_step(&self) -> bool {
        let mut ids = Vec::new();
        let mut c = self.conductor.as_deref();
        while let Some(conductor) = c {
            ids.push(conductor.setup.client_unique_id.as_str());
            c = conductor.parent.as_deref();
        }
        ids.reverse();
        self.ssh.chain.hops.iter().map(|h| h.unique_id.as_str()).eq(ids)
    }

    /// What the user sends: keys, pastes. Held while Buffer Input is on. Nothing reaches a
    /// replaying pane: iTerm2's tab then shows the replay's synthetic session, which has no
    /// task (`PS:1457-1467`).
    pub fn input(&mut self, bytes: &[u8]) {
        if self.replay.is_some() {
            return;
        }
        self.live_input(bytes);
    }

    /// What the live session itself sends (a trigger's Send Text, an upload its program
    /// asked for): a replay view does not stop it, as the live session keeps running.
    pub fn live_input(&mut self, bytes: &[u8]) {
        if self.gateway {
            return;
        }
        // While SSH integration connects, writes wait for the conductor, ahead of Buffer
        // Input and before `writeTaskImpl`'s effects (`PS:4411-4430`).
        if self.ssh.queue(bytes) {
            return;
        }
        // Any write cancels the composer's clear probe (`PS:4154-4158`), held by Buffer
        // Input or not.
        self.composer.probe = None;
        self.suppress_output = false;
        // `PTYSession.m:4217-4227`: git state polls again after a newline goes out.
        if bytes.iter().any(|&b| b == b'\r' || b == b'\n') {
            self.last_newline = Some(std::time::Instant::now());
        }
        if let Some(now) = self.buffer.send(bytes) {
            self.send_typed(&now);
        }
    }

    /// Turns Buffer Input on or off; off sends what waited.
    pub fn set_buffering(&mut self, on: bool) {
        if on {
            self.buffer.on = true;
        } else {
            let waited = self.buffer.release();
            if !waited.is_empty() {
                self.send_typed(&waited);
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

    /// Focus reporting (DECSET 1004): a report, so Buffer Input never holds it. None while
    /// `no_focus_reports` holds them (iTerm2's `setFocused:` returns early).
    pub fn focus_report(&mut self, gained: bool) {
        if self.no_focus_reports > 0 {
            return;
        }
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

    /// Tandem owns this pane's console (a local ConPTY): only then may it attach to read it.
    pub fn owns_console(&self) -> bool {
        matches!(self.pty, Some(Io::Local(_)))
    }

    /// A local pane's pause flag, which a console reader checks under `CONSOLE`
    /// (`pty::console_mode_unless`); None for other panes.
    pub fn console_pause_flag(&self) -> Option<std::sync::Arc<std::sync::atomic::AtomicBool>> {
        match &self.pty {
            Some(Io::Local(p)) => Some(p.pause_flag()),
            _ => None,
        }
    }

    /// Send Signal's Ctrl+C (or Ctrl+Break) to a hosted pane's console, through tandem-host;
    /// None for other panes (a local pane's goes from Tandem).
    pub fn hosted_signal(&self, ctrl_break: bool) -> Option<Result<(), String>> {
        match &self.pty {
            Some(Io::Hosted(p)) => Some(p.signal(ctrl_break)),
            _ => None,
        }
    }

    /// A hosted pane's password prompt state as tandem-host last said; None for other panes.
    pub fn hosted_password_input(&self) -> Option<bool> {
        match &self.pty {
            Some(Io::Hosted(p)) => Some(p.password_input()),
            _ => None,
        }
    }

    /// The shell's process id, while it has a pty.
    pub fn shell_pid(&self) -> Option<u32> {
        self.pty.as_ref().and_then(|p| p.pid())
    }

    /// What the terminal answers the program (reports), and Tandem's own writes that iTerm2
    /// makes with `broadcastAllowed:NO` (the idle code, upload answers, tmux commands):
    /// never held by Buffer Input nor the connecting queue (`PS:4364-4376`, `PS:4276-4294`),
    /// but routed as `writeTaskNoBroadcast` routes them: through the conductor's `send_keys`
    /// when it handles keystrokes, else held while it sets up (`PS:4191-4197`), else the pty.
    pub fn reply(&mut self, bytes: &[u8]) {
        self.composer.probe = None;
        if self.ssh_route() == crate::conductor::Route::Conductor {
            if let Some(c) = self.conductor.as_mut() {
                c.send_keys(bytes);
            }
        } else {
            self.write_unless_held(bytes);
        }
    }

    /// Typed bytes Buffer Input let go: the session's typed route (`write_session`).
    fn send_typed(&mut self, bytes: &[u8]) {
        self.write_session(bytes);
    }

    /// iTerm2's `writeTaskNoBroadcast:`: straight to the pty, past the connecting queue and
    /// Buffer Input. The conductor's own bytes (its bootstrap, the `^C` line that ends it) go
    /// this way.
    pub fn write_no_broadcast(&mut self, bytes: &[u8]) {
        self.composer.probe = None;
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

    /// The Auto Composer's Ctrl+L (`clearForComposer`, `MS:2371-2417`): while a command is
    /// being entered on the main screen, the rows above the prompt move into history and
    /// ConPTY clears too (keeping the cursor row), as Clear to Last Mark does. Keyed on the
    /// prompt mark's one-line interval, whose end is the line after the mark: nothing only when
    /// that end is at or above the screen's top (`MS:2391-2396`), so a prompt on row 0 still
    /// clears. Decision: `Screen::clear_from` at the prompt line stands in for iTerm2's
    /// scroll-and-move - cost if wrong: the upper lines of a multi-line prompt go with the rows
    /// above it.
    pub fn clear_for_composer(&mut self) {
        let s = self.own();
        if !s.prompt_state.entering() || s.alt_screen() {
            return;
        }
        let Some(end) = s.current_prompt_line().map(|l| l + 1) else { return };
        if end <= 0 {
            return;
        }
        let line = end - 1;
        self.end_view();
        self.screen.clear_from(line);
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

    /// At a password prompt, showing its own screen: no view or Instant Replay stands in for
    /// it (a replaying pane drops input, and its frames have their own cursor).
    pub fn at_password_prompt(&self) -> bool {
        self.password_input && self.live.is_none() && self.replay.is_none()
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
    fn a_terminal_reply_cancels_the_clear_probe() {
        // `screenSendReportData` goes through `writeTaskImpl` (`PS:16212-16221`,
        // `PS:4142-4165`): a DA answer during the probe window cancels it; plain output does
        // not.
        let mut p = Pane::tmux(5, 1, 3, 20, 4);
        p.composer.probe = crate::prompt_state::ClearProbe::start(&[0x15, 0x0b], Instant::now());
        p.tmux_io().unwrap().push_output(b"hi");
        assert!(p.pump());
        assert!(p.composer.probe.is_some(), "output alone keeps it");
        p.tmux_io().unwrap().push_output(b"\x1b[c");
        assert!(p.pump());
        assert!(p.composer.probe.is_none(), "the DA reply cancels it");
    }

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

    /// `S:121-122`: the capacity is `IRMemory` megabytes; `TV:2880-2917`: a frame per
    /// refresh with damage, from the session's own screen (`PS:1470-1480`: a replay view
    /// records nothing while the live screen keeps recording).
    #[test]
    fn recording_follows_iterm2() {
        use crate::dvr::{Decoder, Dvr};
        assert_eq!(capacity_for(4), 4194304);
        assert_eq!(capacity_for(-1), 0);
        assert_eq!(capacity_for(5000), 1048576000);
        let newest = |p: &Pane| {
            let mut d = Decoder::default();
            assert!(d.seek(&p.dvr, p.dvr.last_timestamp()));
            d.rows.iter().map(|r| (0..r.len()).map(|c| r[alacritty_terminal::index::Column(c)].c).collect::<String>().trim_end().to_string()).collect::<Vec<_>>()
        };
        let mut p = Pane::tmux(5, 1, 3, 20, 4);
        p.dvr = Dvr::new(capacity_for(4));
        let t0 = Instant::now();
        p.tmux_io().unwrap().push_output(b"hi");
        p.pump();
        assert!(p.record_frame(t0, 1_000, FRAME_GAP));
        assert_eq!(p.dvr.timestamps(), vec![1_000]);
        p.tmux_io().unwrap().push_output(b" more");
        p.pump();
        assert!(!p.record_frame(t0 + Duration::from_millis(10), 2_000, FRAME_GAP), "within 16 ms of the last frame");
        assert_eq!(p.dvr.timestamps(), vec![1_000]);
        assert!(p.record_frame(t0 + FRAME_GAP, 3_000, FRAME_GAP), "the damage waited");
        assert_eq!(p.dvr.timestamps(), vec![1_000, 3_000]);
        assert_eq!(newest(&p)[0], "hi more");
        assert!(!p.record_frame(t0 + FRAME_GAP * 2, 3_500, FRAME_GAP), "nothing new");
        assert_eq!(p.dvr.timestamps(), vec![1_000, 3_000]);
        // A view in front: the own screen records, the view never does.
        let mut view = Screen::new(20, 4);
        view.feed(b"VIEW");
        p.show_view(view);
        let t1 = t0 + FRAME_GAP * 4;
        assert!(!p.record_frame(t1, 4_000, FRAME_GAP), "showing a view is not output");
        p.tmux_io().unwrap().push_output(b"\r\nlive");
        p.pump();
        assert!(p.record_frame(t1 + FRAME_GAP, 5_000, FRAME_GAP));
        let rows = newest(&p);
        assert_eq!(&rows[..2], &["hi more".to_string(), "live".to_string()]);
        assert!(rows.iter().all(|r| !r.contains("VIEW")));
        assert!(p.screen.replay_capture().is_some(), "the view's own damage was never read");
        // No memory, no frames.
        let mut q = Pane::tmux(6, 1, 4, 20, 4);
        q.dvr = Dvr::new(capacity_for(0));
        q.tmux_io().unwrap().push_output(b"hi");
        q.pump();
        assert!(!q.record_frame(t0, 1_000, FRAME_GAP));
        assert!(q.dvr.is_empty());
        // A tmux gateway shows no screen of its own.
        let mut g = Pane::tmux(7, 1, 5, 20, 4);
        g.gateway = true;
        g.tmux_io().unwrap().push_output(b"hi");
        g.pump();
        assert!(!g.record_frame(t0, 1_000, FRAME_GAP));
        assert!(g.dvr.is_empty());
    }

    /// The cadence a pane records at (`UC:145-191`) and the recorder's clock: frames never
    /// go back in time, and a pane with no room for a frame copies nothing.
    #[test]
    fn recording_cadence_follows_iterm2() {
        use crate::cadence::{BACKGROUND_GAP, FAST_GAP};
        let t0 = Instant::now();
        let mut p = Pane::tmux(5, 1, 3, 20, 4);
        assert_eq!(p.replay_gap(t0, true, None), BACKGROUND_GAP, "no output yet is idle");
        p.tmux_io().unwrap().push_output(b"hi");
        p.pump();
        let now = Instant::now();
        assert_eq!(p.replay_gap(now, true, None), FAST_GAP, "output since the last tick, a few bytes");
        assert_eq!(p.replay_gap(now, false, None), BACKGROUND_GAP, "not in a visible tab");
        p.got_output = false;
        assert_eq!(p.replay_gap(now, true, Some(now)), FAST_GAP);
        assert_eq!(p.replay_gap(now + Duration::from_secs(4), true, Some(now)), BACKGROUND_GAP, "idle 3 s");
        assert!(p.record_frame(t0, 5_000, FAST_GAP));
        p.tmux_io().unwrap().push_output(b"!");
        p.pump();
        assert!(!p.record_frame(t0 + Duration::from_millis(500), 6_000, BACKGROUND_GAP), "a background pane waits 1 s");
        assert!(p.record_frame(t0 + BACKGROUND_GAP, 4_000, BACKGROUND_GAP), "a clock set back");
        assert_eq!(p.dvr.timestamps(), vec![5_000, 5_001], "timestamps never go backward");
        // No room for a frame: nothing is captured.
        p.dvr = crate::dvr::Dvr::new(capacity_for(0));
        p.tmux_io().unwrap().push_output(b"?");
        p.pump();
        assert!(!p.record_frame(t0 + BACKGROUND_GAP * 3, 9_000, FAST_GAP));
        assert!(p.dvr.is_empty());
        assert_eq!(p.replay_at, Some(t0 + BACKGROUND_GAP), "no capture happened");
    }

    #[test]
    fn connecting_input_is_queued() {
        let mut p = Pane::with(1, None, 80, 24);
        p.ssh.begin(true);
        p.live_input(b"ls\r");
        assert_eq!(p.ssh.connecting, Some(b"ls\r".to_vec()));
        // Reports pass the queue, as iTerm2's `writeTaskNoBroadcast:` (`PS:16212-16220`).
        p.reply(b"\x1b[0n");
        p.focus_report(true);
        p.feed_output(b"\x1b[5n");
        assert_eq!(p.ssh.connecting, Some(b"ls\r".to_vec()), "reports are not queued");
        p.write_no_broadcast(b"x");
        assert_eq!(p.ssh.connecting, Some(b"ls\r".to_vec()), "the conductor's bytes pass the queue");
        // Buffer Input does not hold what connecting queues.
        p.buffer.on = true;
        p.live_input(b"z");
        assert_eq!(p.ssh.connecting, Some(b"ls\rz".to_vec()));
        // What Buffer Input held is typing: released while connecting, it is queued.
        p.ssh.connecting = None;
        p.live_input(b"w");
        p.ssh.begin(true);
        p.set_buffering(false);
        assert_eq!(p.ssh.connecting, Some(b"w".to_vec()));
        p.ssh.stop_queueing();
        p.live_input(b"y");
        assert_eq!(p.ssh.connecting, None);
    }

    fn setting_up() -> Option<Box<crate::conductor::Conductor>> {
        Some(Box::new(crate::conductor::Conductor::new(Default::default(), None, "U")))
    }

    #[test]
    fn held_writes_flush_in_order() {
        let mut p = Pane::tmux(5, 1, 3, 20, 4);
        p.conductor = setting_up();
        p.write_session(b"a");
        p.write_no_broadcast(b"C");
        p.write_session(b"b");
        assert_eq!(p.tmux_io().unwrap().take_input(), b"C", "the conductor's own bytes are never held");
        assert_eq!(p.take_ssh_held(), b"ab");
        assert!(p.take_ssh_held().is_empty());
        p.conductor.as_mut().unwrap().handle_unhook();
        p.write_session(b"c");
        assert_eq!(p.tmux_io().unwrap().take_input(), b"c");
        assert!(p.take_ssh_held().is_empty());
    }

    #[test]
    fn popping_a_conductor_restores_its_saved_state() {
        use crate::conductor::{Conductor, SavedState, Setup};
        use crate::ssh_session::Hop;
        let at = |id: &str| Setup { client_unique_id: id.into(), ..Setup::default() };
        let hop = |id: &str| Hop { unique_id: id.into(), location: String::new() };
        let mut p = Pane::with(5, None, 80, 24);
        assert_eq!(p.ssh_level(), 0);
        assert!(p.ssh_chain_in_step());
        assert!(!p.pop_conductor(), "nothing to pop");

        let local = SavedState { cwd: Some("/local".into()), host: Some("laptop".into()), user: Some("me".into()) };
        let first_host = SavedState { cwd: Some("/srv".into()), host: Some("a".into()), user: Some("u".into()) };
        let mut a = Conductor::new(at("a"), None, "U");
        a.saved = Some(local.clone());
        a.handle_unhook();
        let mut b = Conductor::new(at("b"), Some(Box::new(a)), "V");
        b.saved = Some(first_host.clone());
        p.conductor = Some(Box::new(b));
        p.ssh.chain.push(hop("a"));
        assert!(!p.ssh_chain_in_step(), "one hop for two conductors");
        p.ssh.chain.push(hop("b"));
        assert!(p.ssh_chain_in_step());
        assert_eq!(p.ssh_level(), 1);
        p.own_mut().restore_ssh_saved(&SavedState { cwd: Some("/far".into()), host: Some("b".into()), user: None });

        assert!(p.pop_conductor());
        assert_eq!(p.own().ssh_saved(), first_host, "the popped conductor's saved state");
        assert_eq!(p.conductor.as_ref().map(|c| c.setup.client_unique_id.as_str()), Some("a"));
        assert_eq!(p.ssh.chain.hops.len(), 2, "the hops are not touched");
        p.conductor.as_mut().unwrap().framed_pid = Some(1);
        assert_eq!(p.ssh_level(), 2);
        assert!(p.pop_conductor());
        assert_eq!(p.own().ssh_saved(), local);
        assert!(p.conductor.is_none());
        assert_eq!(p.ssh_level(), 0);
    }

    /// A pane running A, then B inside it, then C inside B, each conductor with its own saved
    /// state and hop.
    fn three_hops() -> Pane {
        use crate::conductor::{Conductor, SavedState, Setup};
        use crate::ssh_session::Hop;
        let saved = |at: &str| SavedState { cwd: Some(format!("/{at}")), host: Some(at.into()), user: Some("u".into()) };
        let mut p = Pane::with(5, None, 80, 24);
        let mut chain = None;
        for (id, at) in [("a", "local"), ("b", "alpha"), ("c", "beta")] {
            let mut c = Conductor::new(Setup { client_unique_id: id.into(), ..Setup::default() }, chain.take(), "U");
            c.saved = Some(saved(at));
            c.handle_unhook();
            chain = Some(Box::new(c));
            p.ssh.chain.push(Hop { unique_id: id.into(), location: format!("{id}-host") });
        }
        p.conductor = chain;
        p.own_mut().restore_ssh_saved(&saved("gamma"));
        assert!(p.ssh_chain_in_step());
        p
    }

    fn current_id(p: &Pane) -> Option<&str> {
        p.conductor.as_ref().map(|c| c.setup.client_unique_id.as_str())
    }

    #[test]
    fn end_ssh_pops_the_matching_conductors() {
        let hops = |p: &Pane| p.ssh.chain.hops.iter().map(|h| h.unique_id.clone()).collect::<Vec<_>>();

        // EndSSH for the middle hop: B and C go, innermost first, so B's saved state remains.
        let mut p = three_hops();
        assert_eq!(p.end_ssh("b").as_deref(), Some("2 ssh sessions ended. \u{2014} now at a-host."));
        assert_eq!(current_id(&p), Some("a"));
        assert_eq!(hops(&p), ["a"]);
        assert_eq!(p.own().ssh_saved().host.as_deref(), Some("alpha"), "the outermost popped one's saved state");
        assert!(p.ssh_chain_in_step());

        // An id no hop has: nothing ends, nothing is popped.
        let mut p = three_hops();
        assert_eq!(p.end_ssh("zzz"), None);
        assert_eq!(current_id(&p), Some("c"));
        assert_eq!(hops(&p), ["a", "b", "c"]);
        assert_eq!(p.own().ssh_saved().host.as_deref(), Some("gamma"));

        // The innermost: one pop, C's saved state.
        let mut p = three_hops();
        assert_eq!(p.end_ssh("c").as_deref(), Some("ssh exited \u{2014} now at b-host."));
        assert_eq!(current_id(&p), Some("b"));
        assert_eq!(hops(&p), ["a", "b"]);
        assert_eq!(p.own().ssh_saved().host.as_deref(), Some("beta"));
        assert!(p.ssh_chain_in_step());

        // The outermost: everything goes, the local state is back.
        let mut p = three_hops();
        assert_eq!(p.end_ssh("a").as_deref(), Some("3 ssh sessions ended.."), "iTerm2's text (VT100ScreenMutableState.m:7472-7483)");
        assert_eq!(current_id(&p), None);
        assert!(hops(&p).is_empty());
        assert_eq!(p.own().ssh_saved().host.as_deref(), Some("local"));
    }

    /// A pane whose DCS hook is hooked, running conductor "a" (at a.example) started at
    /// `t0` with its first writes taken. `nested`: inside a level 1 session at o.example,
    /// whose unhooked conductor holds nothing; else the pane's first conductor, which holds
    /// typing while it sets up.
    fn hooked_setup(t0: std::time::Instant, nested: bool) -> Pane {
        use crate::conductor::{Conductor, Setup};
        use crate::ssh_session::Hop;
        let at = |id: &str| Setup { client_unique_id: id.into(), parsed: crate::ssh_args::ParsedArgs { hostname: format!("{id}.example"), port: 22, ..Default::default() }, ..Setup::default() };
        let mut p = Pane::tmux(5, 1, 3, 120, 10);
        p.ssh.begin(true);
        p.tmux_io().unwrap().push_output(b"\x1bP2000pnone a YQ== - Yg==\n");
        p.pump();
        p.take_ssh();
        p.tmux_io().unwrap().push_output(b"hidden");
        p.pump();
        assert!(!p.screen.all_text().contains("hidden"), "the hook takes the output");
        let parent = nested.then(|| {
            let mut outer = Conductor::new(at("o"), None, "O");
            outer.handle_unhook();
            p.ssh.chain.push(Hop { unique_id: "o".into(), location: "o.example".into() });
            Box::new(outer)
        });
        let mut c = Conductor::new(at("a"), parent, "U");
        c.fake_now = Some(t0);
        c.start();
        p.conductor = Some(Box::new(c));
        p.ssh.chain.push(Hop { unique_id: "a".into(), location: "a.example".into() });
        assert!(!p.apply_ssh_effects());
        assert!(p.tmux_io().unwrap().take_input().starts_with(b"\x03U\n"), "the prefix, then getshell");
        assert_eq!(p.conductor.as_ref().unwrap().queue_writes(), !nested);
        p
    }

    #[test]
    fn abort_leaves_nothing_held() {
        let mut p = hooked_setup(std::time::Instant::now(), false);
        p.write_unless_held(b"exec_login_shell\n");
        p.ssh.stop_queueing();
        p.write_session(b"ls\r");
        assert_eq!(p.ssh_held, b"exec_login_shell\nls\r");

        p.abort_ssh("boom");
        assert!(p.tmux_io().unwrap().take_input().is_empty(), "the held bytes are dropped, never sent");
        assert!(p.ssh_held.is_empty());
        assert!(p.conductor.is_none());
        assert!(p.ssh.chain.hops.is_empty(), "the hop goes with its conductor");
        let text = p.screen.all_text();
        assert!(text.contains("An error occurred while setting up the SSH environment:\nboom\n"), "{text}");
        assert!(text.contains("ssh exited."), "{text}");
        p.write_session(b"x");
        assert_eq!(p.tmux_io().unwrap().take_input(), b"x", "typing goes straight to the pty");
        assert!(p.ssh_held.is_empty());

        // Nested: the parent becomes current and the banner names where the pane is now.
        let mut p = hooked_setup(std::time::Instant::now(), true);
        p.abort_ssh("boom");
        assert_eq!(p.conductor.as_ref().map(|c| c.setup.client_unique_id.as_str()), Some("o"));
        assert_eq!(p.ssh.chain.hops.len(), 1);
        assert!(p.ssh_chain_in_step());
        assert!(p.screen.all_text().contains("ssh exited \u{2014} now at o.example."));
        p.write_session(b"x");
        assert_eq!(p.tmux_io().unwrap().take_input(), b"x");

        // A remote-sized reason (a command naming the remote's ssh args) is cut.
        let mut p = hooked_setup(std::time::Instant::now(), false);
        p.abort_ssh(&"r".repeat(1 << 20));
        let shown = p.screen.all_text().matches('r').count();
        assert!(shown <= crate::conductor::MAX_SHOWN + 16, "{shown}");
    }

    #[test]
    fn a_failed_setup_never_holds_typing() {
        let t0 = std::time::Instant::now();
        let secs = std::time::Duration::from_secs;
        let mut p = hooked_setup(t0, false);
        p.write_session(b"early");
        assert_eq!(p.ssh.connecting.as_deref(), Some(&b"early"[..]), "typed while connecting");
        assert!(!p.tick_ssh(t0 + secs(29)));
        assert_eq!(p.ssh_deadline(), Some(t0 + secs(30)));

        assert!(p.tick_ssh(t0 + secs(31)));
        assert!(p.tmux_io().unwrap().take_input().is_empty(), "neither the raw exec_login_shell nor the typing (dropped, as iTerm2)");
        assert_eq!(p.ssh.connecting, None, "connecting ended");
        assert!(p.ssh_held.is_empty());
        assert!(p.conductor.is_none());
        assert_eq!(p.ssh_deadline(), None);
        assert!(p.screen.all_text().contains("getshell: No response after 30 seconds"));
        p.write_session(b"ls\r");
        assert_eq!(p.tmux_io().unwrap().take_input(), b"ls\r");
        // iTerm2 bug B13 fixed: output shows again.
        p.tmux_io().unwrap().push_output(b"visible");
        p.pump();
        assert!(!p.has_ssh());
        assert!(p.screen.all_text().contains("visible"));
    }

    #[test]
    fn a_stalled_ground_never_holds_typing() {
        // iTerm2 bug B6: getshell's non-zero status leaves setup in ground, nothing in flight.
        let t0 = std::time::Instant::now();
        let secs = std::time::Duration::from_secs;
        let mut p = hooked_setup(t0, false);
        p.ssh.stop_queueing();
        let c = p.conductor.as_mut().unwrap();
        c.fake_now = Some(t0 + secs(5));
        c.handle_begin("1", 0);
        c.handle_end("1", "r", 1, 0);
        assert!(!p.apply_ssh_effects());
        p.write_session(b"ls\r");
        assert_eq!(p.ssh_held, b"ls\r", "held while setup waits");
        assert_eq!(p.ssh_deadline(), Some(t0 + secs(35)));
        assert!(!p.tick_ssh(t0 + secs(34)));

        assert!(p.tick_ssh(t0 + secs(36)));
        assert!(p.tmux_io().unwrap().take_input().is_empty());
        assert!(p.ssh_held.is_empty());
        assert!(p.screen.all_text().contains("getshell: No response after 30 seconds"));
        p.write_session(b"pwd\r");
        assert_eq!(p.tmux_io().unwrap().take_input(), b"pwd\r");
    }

    #[test]
    fn end_ssh_drops_held_typing() {
        // Controller ruling: EndSSH ending a conductor that still sets up drops what it held;
        // only END_WRITE goes out, never the keys meant for the remote shell.
        let mut p = hooked_setup(std::time::Instant::now(), false);
        p.ssh.stop_queueing();
        p.write_session(b"ls\r");
        assert_eq!(p.ssh_held, b"ls\r");
        assert_eq!(p.end_ssh("a").as_deref(), Some("ssh exited."));
        assert_eq!(p.tmux_io().unwrap().take_input(), b"\x03\n");
        assert!(p.ssh_held.is_empty());

        // An id no hop has: nothing ends (`PS:20255-20259`), so they stay held.
        let mut p = hooked_setup(std::time::Instant::now(), false);
        p.ssh.stop_queueing();
        p.write_session(b"ls\r");
        assert_eq!(p.end_ssh("zzz"), None);
        assert!(p.tmux_io().unwrap().take_input().is_empty());
        assert_eq!(p.ssh_held, b"ls\r");
    }

    #[test]
    fn end_ssh_while_connecting_drops_typing() {
        // The connecting queue and the held writes both go; only END_WRITE is written.
        let mut p = hooked_setup(std::time::Instant::now(), false);
        p.write_session(b"early");
        assert_eq!(p.ssh.connecting.as_deref(), Some(&b"early"[..]));
        p.ssh_held.extend_from_slice(b"held");
        assert_eq!(p.end_ssh("a").as_deref(), Some("ssh exited."));
        assert_eq!(p.tmux_io().unwrap().take_input(), b"\x03\n");
        assert_eq!(p.ssh.connecting, None);
        assert!(p.ssh_held.is_empty());

        // A remaining conductor still sets up (nested case, controller ruling): the connecting
        // queue goes, while what it held stays for its own unhook, with END_WRITE held behind
        // it (writeTaskNoBroadcast, `PS:20176-20180`, `PS:20269`). Nothing reaches the pty,
        // where conductor.sh reads command lines.
        let mut p = hooked_setup(std::time::Instant::now(), false);
        let inner = crate::conductor::Conductor::new(crate::conductor::Setup { client_unique_id: "b".into(), ..Default::default() }, p.conductor.take(), "V");
        p.conductor = Some(Box::new(inner));
        p.ssh.chain.push(crate::ssh_session::Hop { unique_id: "b".into(), location: "b.example".into() });
        p.write_session(b"early");
        p.ssh_held.extend_from_slice(b"held");
        assert_eq!(p.end_ssh("b").as_deref(), Some("ssh exited \u{2014} now at a.example."));
        assert!(p.tmux_io().unwrap().take_input().is_empty());
        assert_eq!(p.ssh.connecting, None);
        assert_eq!(p.ssh_held, b"held\x03\n");
    }

    #[test]
    fn keys_after_an_abort_in_one_batch_are_dropped() {
        // A coprocess (or StopQueueingInput) handed keys to the conductor after it failed:
        // the batch is SessionWrite, Abort, Keys.
        let mut p = hooked_setup(std::time::Instant::now(), false);
        p.ssh.stop_queueing();
        let c = p.conductor.as_mut().unwrap();
        c.fail("boom");
        c.send_keys(b"typed");
        assert!(p.apply_ssh_effects());
        assert!(p.conductor.is_none());
        assert!(p.tmux_io().unwrap().take_input().is_empty(), "the failed setup's keys and writes never reach the pty");
        assert!(p.screen.all_text().contains("boom"));
    }

    #[test]
    fn reports_and_keys_are_held_during_setup() {
        // Not the tmux fake: a tmux pane drops screen replies (`feed_output`).
        let mut p = Pane::with(5, None, 80, 24);
        let da = Pane::with(6, None, 80, 24).own_mut().feed(b"\x1b[c");
        assert!(!da.is_empty());
        p.conductor = setting_up();
        p.input(b"k");
        p.reply(b"\x1b[?1;2c");
        p.feed_after_ssh(b"\x1b[c");
        let mut want = b"k\x1b[?1;2c".to_vec();
        want.extend_from_slice(&da);
        assert_eq!(p.take_ssh_held(), want, "a key, a report, then the screen's report, in order");
        p.conductor.as_mut().unwrap().handle_unhook();
        p.input(b"z");
        assert!(p.take_ssh_held().is_empty(), "not held once unhooked");
    }

    #[test]
    fn coprocess_output_goes_to_the_conductor() {
        use crate::conductor::Effect;
        let mut p = Pane::tmux(5, 1, 3, 20, 4);
        p.conductor = setting_up();
        assert!(p.coprocess_output(b"co"));
        assert_eq!(p.conductor.as_mut().unwrap().take_effects(), [Effect::Keys(b"co".to_vec())], "sendKeys (PS:4639-4642)");
        assert!(p.tmux_io().unwrap().take_input().is_empty());
        assert!(p.ssh_held.is_empty());
        p.conductor.as_mut().unwrap().send_keys(b"co");
        assert!(!p.apply_ssh_effects());
        assert_eq!(p.take_ssh_held(), b"co", "held while setup runs");
        assert!(p.tmux_io().unwrap().take_input().is_empty());

        // Level 1 while a nested it2ssh connects: sendKeys writes through
        // writeTaskNoBroadcast (`CD:1854-1862`, `PS:25519-25522`), past the connecting queue.
        p.conductor.as_mut().unwrap().handle_unhook();
        p.ssh.begin(true);
        assert!(p.coprocess_output(b"lvl1"));
        assert!(!p.apply_ssh_effects());
        assert_eq!(p.tmux_io().unwrap().take_input(), b"lvl1");
        assert_eq!(p.ssh.connecting.as_deref(), Some(&b""[..]), "not queued behind the nested setup");
        p.ssh.stop_queueing();

        p.conductor = None;
        assert!(p.coprocess_output(b"out"));
        assert_eq!(p.tmux_io().unwrap().take_input(), b"out", "no conductor: to the pty as before");
    }

    #[test]
    fn conductor_route_reaches_send_keys() {
        use crate::conductor::Effect;
        let mut p = Pane::tmux(5, 1, 3, 20, 4);
        p.conductor = setting_up();
        p.conductor.as_mut().unwrap().framed_pid = Some(1);
        p.write_session(b"q");
        assert_eq!(p.conductor.as_mut().unwrap().take_effects(), [Effect::Keys(b"q".to_vec())]);
        assert!(p.tmux_io().unwrap().take_input().is_empty(), "nothing written");
        assert!(p.ssh_held.is_empty(), "nothing held");
    }

    #[test]
    fn write_layers_follow_iterm2() {
        let mut p = Pane::with(5, None, 80, 24);
        p.conductor = setting_up();
        p.ssh.begin(true);
        p.write_session(b"x");
        assert_eq!(p.ssh.connecting, Some(b"x".to_vec()), "the connecting queue comes first");
        assert!(p.ssh_held.is_empty());
        p.ssh.stop_queueing();
        p.write_session(b"y");
        assert_eq!(p.ssh_held, b"y");
    }

    #[test]
    fn ssh_output_waits_behind_its_token() {
        use crate::conductor_parser::Token;
        use crate::ssh_hook::Out;
        let mut p = Pane::tmux(5, 1, 3, 20, 4);
        p.tmux_io().unwrap().push_output(b"before\x1bP2000pnone 1 YQ== - Yg==\n");
        p.tmux_io().unwrap().push_output(b"\x1b]135;:unhook\x1b\\after");
        assert!(p.pump());
        assert!(p.has_ssh());
        let outs = p.take_ssh();
        assert!(matches!(&outs[0], Out::Ssh(Token::Init(i), 0) if i.unique_id == "1"), "{outs:?}");
        assert_eq!(outs[1..], [Out::Ssh(Token::Unhook, 0), Out::Screen(b"after".to_vec())]);
        assert!(!p.has_ssh());
        assert!(p.screen.all_text().contains("before"));
        assert!(!p.screen.all_text().contains("after"), "output after a token waits for it");
        p.feed_after_ssh(b"after");
        assert!(p.screen.all_text().contains("beforeafter"));
        // Suppress All Output drops output, never the hook's view of it.
        p.suppress_output = true;
        p.tmux_io().unwrap().push_output(b"x\x1bP2000pnone 1 YQ== - Yg==\n");
        p.pump();
        assert!(matches!(&p.take_ssh()[..], [Out::Ssh(Token::Init(_), 0)]));
        assert!(!p.screen.all_text().contains("afterx"));
    }

    #[test]
    fn tmux_output_hands_the_hook_over_at_once() {
        use crate::conductor_parser::Token;
        use crate::ssh_hook::Out;
        let mut p = Pane::tmux(5, 1, 3, 20, 4);
        assert!(!p.tmux_output(b"plain", true));
        assert!(p.tmux_io().unwrap().answers_osc4.get());
        assert!(p.tmux_output(b"x\x1bP2000pnone 1 YQ== - Yg==\n", false), "the app must run the hook now");
        assert!(!p.tmux_io().unwrap().answers_osc4.get());
        assert!(matches!(&p.take_ssh()[..], [Out::Ssh(Token::Init(_), 0)]));
        assert!(p.screen.all_text().contains("plainx"));
        assert!(p.tmux_output(b"\x1b]135;:unhook\x1b\\", false), "every later token too");
        assert_eq!(p.take_ssh(), [Out::Ssh(Token::Unhook, 0)]);
    }

    #[test]
    fn a_dcs_2000p_inside_tmux_control_mode_starts_nothing() {
        use crate::tmux_hook::Out;
        let dcs = b"\x1bP2000pnone 1 YQ== - Yg==";
        let line = |bytes: &[u8]| Out::Line(bytes.to_vec());
        let reply = |p: &mut Pane, bytes: &[u8]| {
            p.tmux_io().unwrap().push_output(bytes);
            p.pump();
        };
        // The start and a reply block in one read: the reply line carries the raw DCS.
        let mut p = Pane::tmux(5, 1, 3, 20, 4);
        let mut read = b"sh\x1bP1000p%begin 1 1 0\n".to_vec();
        read.extend_from_slice(dcs);
        read.extend_from_slice(b"\n%end 1 1 0\n");
        reply(&mut p, &read);
        assert!(!p.has_ssh(), "no conductor over the tmux stream");
        assert_eq!(p.take_tmux(), [Out::Enter, line(b"%begin 1 1 0"), line(dcs), line(b"%end 1 1 0")]);
        assert!(p.screen.all_text().contains("sh"));

        // In a later read, split across reads: the same.
        reply(&mut p, b"%begin 2 2 0\n\x1bP20");
        reply(&mut p, b"00pnone 1 YQ== - Yg==\n%end 2 2 0\n");
        assert!(!p.has_ssh());
        assert_eq!(p.take_tmux(), [line(b"%begin 2 2 0"), line(dcs), line(b"%end 2 2 0")]);

        // After %exit the pane is a plain one again, and a DCS 2000p hooks.
        reply(&mut p, b"%exit\n");
        p.take_tmux();
        let mut read = dcs.to_vec();
        read.push(b'\n');
        reply(&mut p, &read);
        assert!(p.has_ssh());
    }

    #[test]
    fn tmux_panes_do_not_answer_queries() {
        let mut p = Pane::tmux(5, 1, 3, 20, 4);
        p.tmux_io().unwrap().push_output(b"\x1b[c\x1b[6n");
        p.pump();
        p.feed_output(b"\x1b[c");
        assert!(p.tmux_io().unwrap().take_input().is_empty(), "tmux answers for its panes; a second answer would be typed");
    }

    /// Undo Close brings a pane back paused while its inline file waits, and asks about the
    /// file again when its question went while the pane was closed.
    #[test]
    fn undo_close_keeps_a_waiting_inline_file_paused() {
        let mut p = Pane::with(1, None, 20, 6);
        p.feed_output(b"\x1b]1337;File=inline=1:aGVsbG8=\x07after");
        assert!(p.own_mut().inline_waiting().is_some());
        p.pause_for_inline();
        p.undo_close_resume(true);
        assert!(p.inline_paused, "the question still waits");
        assert!(p.own_mut().inline_waiting().is_none(), "and is not asked twice");
        p.undo_close_resume(false);
        assert!(p.inline_paused, "the question went");
        assert!(p.own_mut().inline_waiting().is_some(), "so it is asked again");
        let serial = p.own().inline_serial().unwrap();
        p.inline_answered(serial, false);
        assert!(!p.inline_paused);
        p.pause_for_inline();
        p.undo_close_resume(false);
        assert!(!p.inline_paused, "nothing waits: reading resumes");
    }

    #[test]
    fn gateway_panes_take_no_input() {
        let mut p = Pane { id: 1, session_guid: String::new(), screen: Screen::new(10, 4), pty: None, exited: None, error: None, profile: String::new(), command: String::new(), live: None, session_note: String::new(), buffer: InputBuffer::default(), frozen: false, suppress_output: false, got_output: false, coprocess: None, co_out: Default::default(), co_end: None, tmux_hook: Default::default(), tmux_q: Vec::new(), ssh_hook: Default::default(), ssh_q: Vec::new(), conductor: None, ssh_held: Vec::new(), gateway: false, tmux_size: None, screen_filter: None, last_newline: None, composer: composer::Composer::default(), dvr: dvr::Dvr::new(capacity_for(4)), replay_at: None, throughput: Default::default(), replay: None, echo_probe: None, no_focus_reports: 0, password_input: false, cookie: None, inline_paused: false, frustration: Default::default(), arrangement: None, ssh: Default::default() };
        p.buffer.on = true;
        p.input(b"a");
        p.gateway = true;
        p.input(b"b");
        assert_eq!(p.buffer.release(), b"a", "a tmux gateway's pty carries only tmux commands");
    }

    #[test]
    fn replaying_pane_takes_no_user_input() {
        // During a replay iTerm2's tab shows the task-less synthetic session, so nothing
        // the user sends reaches the live shell; the live session's own writes (a
        // trigger's Send Text) still do.
        let mut p = Pane { id: 1, session_guid: String::new(), screen: Screen::new(10, 4), pty: None, exited: None, error: None, profile: String::new(), command: String::new(), live: None, session_note: String::new(), buffer: InputBuffer::default(), frozen: false, suppress_output: false, got_output: false, coprocess: None, co_out: Default::default(), co_end: None, tmux_hook: Default::default(), tmux_q: Vec::new(), ssh_hook: Default::default(), ssh_q: Vec::new(), conductor: None, ssh_held: Vec::new(), gateway: false, tmux_size: None, screen_filter: None, last_newline: None, composer: composer::Composer::default(), dvr: dvr::Dvr::new(capacity_for(4)), replay_at: None, throughput: Default::default(), replay: None, echo_probe: None, no_focus_reports: 0, password_input: false, cookie: None, inline_paused: false, frustration: Default::default(), arrangement: None, ssh: Default::default() };
        p.buffer.on = true;
        p.input(b"a");
        p.replay = Some(replay::Replay { decoder: None, read_only: false });
        p.input(b"b");
        p.live_input(b"c");
        p.replay = None;
        p.input(b"d");
        assert_eq!(p.buffer.release(), b"acd");
    }

    #[test]
    fn frozen_pane_keeps_its_copy() {
        let mut own = Screen::new(10, 4);
        own.feed(b"live");
        let mut p = Pane { id: 1, session_guid: String::new(), screen: own, pty: None, exited: None, error: None, profile: String::new(), command: String::new(), live: None, session_note: String::new(), buffer: InputBuffer::default(), frozen: false, suppress_output: false, got_output: false, coprocess: None, co_out: Default::default(), co_end: None, tmux_hook: Default::default(), tmux_q: Vec::new(), ssh_hook: Default::default(), ssh_q: Vec::new(), conductor: None, ssh_held: Vec::new(), gateway: false, tmux_size: None, screen_filter: None, last_newline: None, composer: composer::Composer::default(), dvr: dvr::Dvr::new(capacity_for(4)), replay_at: None, throughput: Default::default(), replay: None, echo_probe: None, no_focus_reports: 0, password_input: false, cookie: None, inline_paused: false, frustration: Default::default(), arrangement: None, ssh: Default::default() };
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
        Pane::start(1, &api, cmd, None, &[], 80, 24, Arc::new(|| {})).unwrap()
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

    #[test]
    fn a_kept_reference_replaces_the_fresh_one() {
        let old = Pane::recording(1, 80, 24);
        let mut restarted = Pane::recording(1, 80, 24);
        assert_ne!(old.session_guid, restarted.session_guid, "every pane starts with a fresh reference");
        restarted.keep_reference(&old.session_guid);
        assert_eq!(old.session_guid, restarted.session_guid);
        let split = Pane::recording(2, 80, 24);
        assert_ne!(old.session_guid, split.session_guid, "a split or duplicate is a new session");
    }
}

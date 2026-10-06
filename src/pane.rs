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

/// The most keystroke bytes a framed conductor may hold queued before the coprocess waits (F20).
const COPROCESS_BACKLOG: usize = crate::conductor::PIPELINE_BYTES * 64;

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

impl TmuxInfo {
    /// What a tmux monitor reported. The job is another program's text and joins
    /// `session.jobName` and the titles, so it is stored as the person reads it.
    pub fn store(&mut self, what: crate::tmux_controller::Info, value: String) {
        match what {
            crate::tmux_controller::Info::Title => self.title = value,
            crate::tmux_controller::Info::Job => self.job = crate::inert_files::clean_name(&value),
            crate::tmux_controller::Info::Cwd => self.cwd = value,
        }
    }
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

// Test-only: how many times this thread reached a pty (a write, a resize or a clear), so a
// test can prove an ended pane touches its dead console never (13q B8).
#[cfg(test)]
thread_local! {
    pub static PTY_TOUCHES: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn touched() {
    PTY_TOUCHES.with(|c| c.set(c.get() + 1));
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
        #[cfg(test)]
        touched();
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
        #[cfg(test)]
        touched();
        match self {
            Io::Local(p) => p.resize(cols, rows),
            Io::Hosted(p) => p.resize(cols, rows),
            // tmux sets its panes' sizes (milestone 8c sends Tandem's).
            Io::Tmux(_) => Ok(()),
        }
    }

    pub fn clear(&self) -> Result<(), String> {
        #[cfg(test)]
        touched();
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

/// How long after an exit its end waits for the output to drain (13q P6).
const END_DRAIN: Duration = Duration::from_millis(50);
/// The most the end waits beyond that for a stream that will not stop.
const END_DRAIN_CAP: Duration = Duration::from_millis(200);

/// The most `%notif search` texts a pane holds between two passes of the app.
const SEARCH_QUEUE_MAX: usize = 1024;

pub struct Pane {
    pub id: PaneId,
    /// A new session still waits for Force keyboard's once-per-session layout (`needsNewTerminalKeyboardForced`).
    pub needs_forced_layout: bool,
    /// Last-use stamp from the app-wide sequence (`activityCounter`); 0 until focused.
    pub activity: u64,
    /// Lock Pane (`PTYSession locked`): refuses moves and swaps and asks before closing.
    pub locked: bool,
    /// The prompt mark of the command a click selected (13s Task 9 sets it; 13r's buttons read it).
    pub selected_command: Option<u32>,
    /// Lock Split Pane Width (`PS:21756-21764`): the width in
    /// points the pane is capped at on a window resize.
    pub pinned_width: Option<f32>,
    /// The pane's own reference in a chat: unique across panes and launches, unlike `id`, which
    /// restarts every launch (a chat bound to a reused id would control an unrelated pane).
    pub session_guid: String,
    /// The id a model names this session by (`stable_id`); minted in `Pane::with`, restored only
    /// by the launch's own session restoration.
    pub stable_id: String,
    pub screen: Screen,
    /// The session's Windows code page (0 for UTF-8, which needs no conversion).
    pub encoding: u32,
    /// Decodes the session's output to UTF-8 while `encoding` is set.
    pub decoder: Option<crate::encoding::Decoder>,
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
    /// Changes to the app-wide connected hosts, for the app to apply in order after it pumps
    /// the pane's SSH (`App::ssh_apply`): every conductor that frames or leaves the pane.
    ssh_registry_out: Vec<crate::ssh_registry::Change>,
    /// Answers to script calls (`get_time_offset`) the conductors finished, for the app to
    /// deliver by token.
    ssh_answers: Vec<(u64, Result<crate::ssh_clock::Offset, String>)>,
    /// File requests answered (id, content, status), for the app to route (14d, P6).
    ssh_files_out: Vec<(u64, String, i32)>,
    /// `%notif search` texts the remote sent, unchecked, for the app to parse (14d, P18);
    /// bounded: a flood past `SEARCH_QUEUE_MAX` between two passes is dropped.
    ssh_search_out: Vec<String>,
    /// The Clock's view of the remote's time (`CLK`): what the bar shows for this session.
    pub ssh_clock: crate::ssh_clock::Clock,
    /// Session writes held while the conductor sets up (`_sshWriteQueue`, `PS:4191-4197`),
    /// sent when it unhooks.
    ssh_held: Vec<u8>,
    /// The hops this pane still has to reconnect through (`_pendingJumps`, `PS:20139-20169`): a
    /// hook uses up the first (its directory) and the rest start the jumping. Set for a
    /// Duplicate Session or a restored arrangement's session.
    pub pending_jumps: Vec<crate::ssh_jumps::Reconnection>,
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
    /// Reduce Flicker: this pane's last live picture (kept each live frame while the profile's
    /// switch is on, dropped when it is off), and when the hold now showing it ends.
    pub held: Option<crate::flicker::Held>,
    pub held_until: Option<Instant>,
    /// Instant Replay's session while this pane shows a frame from its DVR instead of
    /// its live screen.
    pub replay: Option<replay::Replay>,
    /// The password manager's echo probe (`iTermEchoProbe`): fed the raw output before the
    /// screen sees it, once tmux control mode is found (a gateway never probes).
    pub echo_probe: Option<crate::echo_probe::Probe>,
    /// The stuck mouse reporting detector, armed by each mouse report written to the program.
    pub stuck: Option<crate::stuck_mouse::Detector>,
    /// The detector found a report echoed back; `App::stuck_tick` takes it.
    pub stuck_seen: Option<crate::stuck_mouse::Turd>,
    /// A conductor logged in and the profile's Initial Text may now be typed (`PS:25565`);
    /// `App::ssh_apply` takes it.
    pub initial_text_due: bool,
    /// The session's Initial Text went (or was refused for good): a conductor's repeated
    /// `SendInitialText` (a hop, a reconnect) never types it twice.
    pub initial_text_sent: bool,
    /// `idleTimeSeconds`, read once when the session starts (`PS:849`).
    pub idle_time: Duration,
    /// The OSC 9;4 bar's animation clock (`progress::Anim`), kept in fractions of the bar.
    pub progress_anim: crate::progress::Anim,
    /// Test hook: every size `jiggle` sent to the pty.
    #[cfg(test)]
    pub resizes_for_test: Vec<(u16, u16)>,
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
    /// The scroll accumulators (iTerm2 keeps them per session).
    pub wheel: crate::wheel::PaneWheel,
    /// The named arrangement this pane was restored from and its session's id there, for
    /// the arrangement's Repair and Assign Profile (`_originatingArrangementName`).
    pub arrangement: Option<(String, String)>,
    /// SSH integration: the connecting queue (milestone 14).
    pub ssh: crate::ssh_session::PaneSsh,
    /// The cursor's motion between frames (13k): where it was, and when it moved.
    pub motion: crate::cursor_anim::Motion,
    /// The session ended and its end was handled (the divider is drawn, the end action ran): the
    /// pane stays, as iTerm2's does, and takes no input (13q R3, `PS:5214`). The one flag for
    /// every kept-open pane, a REPL's included.
    pub ended: bool,
    /// When the session was created (`_creationDate`): the clock of the short-lived warning, the
    /// log file name's `creationTimeString`; a restart keeps it.
    pub created: std::time::SystemTime,
    pub created_at: Instant,
    /// A random number for the log file name's `autoLogId` (`PS:885-897`); a restart keeps it.
    pub auto_log_id: u32,
    /// A REPL window or a dependency run: it stays when its program ends, and is never offered a
    /// restart (iTerm2 gives them no CloseOnTermination, `IC:2189-2191`).
    pub hold_open: bool,
    /// Programs the shell left running when it exited, as (pid, creation time): they stay
    /// attached to the console, and closing the pane ends them (13q B7).
    pub survivors: Vec<(u32, u64)>,
    /// When the exit was first seen: the end is handled once the output has drained.
    exit_seen: Option<Instant>,
    /// The shell's pid at its exit, for the survivor survey only (`shell_pid` is None then).
    exit_pid: Option<u32>,
    /// The local folder the shell started in (it passed 13p's start-folder rules and network
    /// filter): a restart starts there again (P13, `PS:4890-4896`). None for a pane with no
    /// local shell (tmux, a test).
    pub start_dir: Option<std::path::PathBuf>,
    /// The session's log (13q Task 7): written from `pump` (raw reads and cooked screen text),
    /// stopped when the session ends or the pane closes. A restart starts its own.
    pub log: Option<crate::session_log::Logger>,
    /// When logging started: asciicast's clock.
    pub log_started: Option<Instant>,
    /// `smartLoggingWithAutoComposer` and View > Auto Composer, set by the app before each pump
    /// (`PS:15607-15616`).
    pub log_compact: (bool, bool),
    /// A restored archive's pane (13q Task 8): no shell, the saved text; Restart makes it live.
    pub archived: bool,
    /// Which shell the pane holds: bumped when the shell exits (or is ended) and again when a
    /// restart gives the pane a new one, so a question asked about one shell is never answered
    /// for another (13q final fix 2).
    pub shell_gen: u64,
}

/// A random u32 for a session's `autoLogId`.
fn new_auto_log_id() -> u32 {
    let mut b = [0u8; 4];
    let _ = getrandom::fill(&mut b);
    u32::from_le_bytes(b)
}

/// A fresh session reference for a pane.
pub fn new_session_guid() -> String {
    uuid::Uuid::new_v4().to_string().to_uppercase()
}

impl Pane {
    /// The pane's inline file question is gone with its program: reading is no longer paused.
    pub fn inline_paused_off(&mut self) {
        self.inline_paused = false;
    }

    /// The session ended: every SSH conductor goes (the hop list stays, so a restart reconnects
    /// through the profile), what was held for a setup is dropped, and registry changes not yet
    /// applied are discarded (13q P5).
    pub fn drop_ssh_at_exit(&mut self) {
        for _ in 0..16 {
            if self.conductor.is_none() {
                break;
            }
            self.ssh_unhook_conductor();
        }
        self.ssh_hook.unhook();
        self.ssh_held.clear();
        self.ssh_registry_out.clear();
    }

    /// A restart in place (13q Task 4, `resetForRelaunch`, `PS:6734-6741`): `fresh` is a new
    /// pane for the same id with a new shell. The pane that comes back is `fresh`, so every
    /// field of the shell (its pty, cookie, SSH and decoder state, coprocess, held input, the
    /// profile's flags) is new by construction; the screen and what belongs to the session
    /// itself move in from `old`: the text, scrollback and marks, the lock, pinned width, last
    /// use, references, note, arrangement, Instant Replay's recording, the composer, the wheel
    /// and the creation time and log id (a restart keeps them, `PS:846`, `PS:892-897`). The old
    /// shell goes with `old`'s pty. The caller has dropped the app-wide state of the old shell
    /// (jobs, cookie, SSH registry).
    pub fn relaunch(mut old: Pane, mut fresh: Pane) -> Pane {
        fresh.shell_gen = old.shell_gen + 1;
        // A Filter or Zoom view or a frozen copy gives way to the pane's own screen.
        old.frozen = false;
        old.end_view();
        old.replay = None;
        // The program is gone: its modes, its open strings and its state go; the text stays.
        old.screen.program_ended();
        old.screen.soft_reset();
        let want = (fresh.screen.term().columns() as u16, fresh.screen.term().screen_lines() as u16);
        std::mem::swap(&mut fresh.screen, &mut old.screen);
        fresh.screen.pty_backed = true;
        fresh.session_note = std::mem::take(&mut old.session_note);
        fresh.locked = old.locked;
        fresh.pinned_width = old.pinned_width;
        fresh.activity = old.activity;
        fresh.session_guid = std::mem::take(&mut old.session_guid);
        fresh.stable_id = std::mem::take(&mut old.stable_id);
        fresh.arrangement = old.arrangement.take();
        std::mem::swap(&mut fresh.dvr, &mut old.dvr);
        fresh.replay_at = old.replay_at;
        fresh.held = old.held.take();
        fresh.held_until = old.held_until;
        std::mem::swap(&mut fresh.composer, &mut old.composer);
        fresh.composer.probe = None;
        std::mem::swap(&mut fresh.wheel, &mut old.wheel);
        fresh.motion = std::mem::take(&mut old.motion);
        fresh.created = old.created;
        fresh.created_at = old.created_at;
        fresh.auto_log_id = old.auto_log_id;
        // The shell started at the size the layout gives now; the kept screen follows it.
        fresh.resize(want.0, want.1);
        fresh
    }

    /// Takes `guid` as the pane's reference: a restored pane keeps the one it saved, and a
    /// restarted session keeps its own (iTerm2 keeps a session's guid, `PTYSession.m:2403-2420`).
    pub fn keep_reference(&mut self, guid: &str) {
        self.session_guid = guid.to_string();
    }

    /// Takes `id` as the pane's stable id (a restarted session keeps its own; a restored one
    /// adopts it through `stable_id::adopt` only).
    pub fn keep_stable_id(&mut self, id: &str) {
        self.stable_id = id.to_string();
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
        pane.needs_forced_layout = false;
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
        if self.exited.is_some() {
            return;
        }
        let (cols, rows) = (self.screen.term().columns() as u16, self.screen.term().screen_lines() as u16);
        if let Some(io) = &self.pty {
            for (c, r) in crate::reattach::jiggle(cols, rows) {
                #[cfg(test)]
                self.resizes_for_test.push((c, r));
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
        let mut p = Pane::with(id, None, cols.max(2), rows.max(1));
        p.needs_forced_layout = false;
        p
    }

    /// A restored archive's pane (`PS:3577-3580`): no shell, ended with status 0, the saved text.
    pub fn archived(id: PaneId, cols: u16, rows: u16) -> Pane {
        let mut p = Pane::with(id, None, cols.max(2), rows.max(1));
        p.exited = Some(0);
        p.ended = true;
        p.archived = true;
        p.needs_forced_layout = false;
        p
    }

    fn with_io(id: PaneId, io: Io, cols: u16, rows: u16) -> Pane {
        Pane::with(id, Some(io), cols, rows)
    }

    fn with(id: PaneId, pty: Option<Io>, cols: u16, rows: u16) -> Pane {
        let mut screen = Screen::new(cols as usize, rows as usize);
        screen.pty_backed = pty.is_some();
        Pane { id, needs_forced_layout: true, activity: 0, locked: false, selected_command: None, pinned_width: None, session_guid: new_session_guid(), stable_id: crate::stable_id::generate(), screen, encoding: 0, decoder: None, pty, exited: None, error: None, profile: String::new(), command: String::new(), live: None, session_note: String::new(), buffer: InputBuffer::default(), frozen: false, suppress_output: false, got_output: false, coprocess: None, co_out: Default::default(), co_end: None, tmux_hook: Default::default(), tmux_q: Vec::new(), ssh_hook: Default::default(), ssh_q: Vec::new(), conductor: None, ssh_registry_out: Vec::new(), ssh_answers: Vec::new(), ssh_files_out: Vec::new(), ssh_search_out: Vec::new(), ssh_clock: Default::default(), ssh_held: Vec::new(), pending_jumps: Vec::new(), gateway: false, tmux_size: None, screen_filter: None, last_newline: None, composer: composer::Composer::default(), dvr: dvr::Dvr::new(capacity_for(4)), replay_at: None, throughput: Default::default(), held: None, held_until: None, replay: None, echo_probe: None, stuck: None, stuck_seen: None, initial_text_due: false, initial_text_sent: false, idle_time: crate::activity::DEFAULT_IDLE, progress_anim: Default::default(), #[cfg(test)] resizes_for_test: Vec::new(), no_focus_reports: 0, password_input: false, cookie: None, inline_paused: false, frustration: Default::default(), wheel: crate::wheel::PaneWheel::new(0.1), arrangement: None, ssh: Default::default(), motion: Default::default(), ended: false, created: std::time::SystemTime::now(), created_at: Instant::now(), auto_log_id: new_auto_log_id(), hold_open: false, survivors: Vec::new(), exit_seen: None, exit_pid: None, start_dir: None, log: None, log_started: None, log_compact: (false, false), archived: false, shell_gen: 0 }
    }

    /// Feeds all waiting output into the screen. Returns true if anything changed.
    pub fn pump(&mut self) -> bool {
        let Some(pty) = &self.pty else { return false };
        let mut chunks = Vec::new();
        while let Read::Data(chunk) = pty.try_read() {
            self.got_output = true;
            self.throughput.add(Instant::now(), chunk.len());
            chunks.push(chunk);
        }
        let mut changed = !chunks.is_empty();
        // SSH integration, then tmux control mode, are found before anything can drop output.
        for chunk in chunks {
            self.log_raw(&chunk);
            self.hook_output(&chunk);
        }
        if self.coprocess.is_some() {
            // Its output goes to the shell as typed, while the shell keeps up.
            loop {
                let Some(pty) = &self.pty else { break };
                let budget = crate::coprocess::budget(pty.pending());
                // A framed link answers slowly: what is queued for it holds the coprocess back
                // until replies come (F20), so its output cannot grow the queue without end.
                if budget == 0 || self.coprocess_backlogged() {
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
                self.shell_gen += 1;
                self.exit_pid = pty.pid();
                self.exit_seen = Some(Instant::now());
                // What the shell left running is taken now, not when the end is handled: a close or
                // a restart inside the drain wait must still count it (13q B7).
                self.survey_survivors();
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
                self.tee_raw(part);
                let mut bytes = self.ssh_hook.take_held();
                bytes.extend_from_slice(part);
                self.route_output(bytes);
            } else {
                let outs = self.ssh_hook.feed(part);
                // The coprocess gets the raw read while SSH integration plays no part in it
                // (iTerm2 tees the raw read, `PK:900`); with a conductor only its depth 0
                // output (`Out::Raw`) goes, and a level 1 session's coprocess gets nothing.
                if outs.iter().all(|o| matches!(o, crate::ssh_hook::Out::Screen(_))) {
                    self.tee_raw(part);
                }
                for out in outs {
                    match out {
                        // At once, not behind `ssh_q`: it is the coprocess's own copy.
                        crate::ssh_hook::Out::Raw(bytes) => self.tee_coprocess(&bytes),
                        crate::ssh_hook::Out::Screen(bytes) if self.ssh_q.is_empty() => self.route_output(bytes),
                        other => self.ssh_q.push(other),
                    }
                }
            }
        }
    }

    /// Whether the raw read may go to the coprocess: no conductor, no hook, nothing waiting.
    fn coprocess_gets_raw(&self) -> bool {
        self.conductor.is_none() && !self.ssh_hook.active() && self.ssh_q.is_empty()
    }

    fn tee_raw(&mut self, bytes: &[u8]) {
        if self.coprocess_gets_raw() {
            self.tee_coprocess(bytes);
        }
    }

    /// Output is looked at for a mouse report echoed back (`PTYSession.m:14102-14120`).
    fn watch_stuck(&mut self, bytes: &[u8]) {
        if let Some(d) = self.stuck.as_mut() {
            if d.output(bytes, Instant::now()) {
                self.stuck_seen = Some(d.kind());
            }
        }
    }

    /// Depth 0 output of a conductor's session reaches the coprocess only while it has one.
    fn tee_coprocess(&mut self, bytes: &[u8]) {
        if let Some(c) = self.coprocess.as_mut() {
            c.write(bytes);
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
                    let bytes = self.decode_output(bytes);
                    self.log_cooked(&bytes);
                    self.got_output = true;
                    if let Some(p) = self.echo_probe.as_mut() {
                        p.output(&bytes);
                    }
                    self.watch_stuck(&bytes);
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

    /// The time between this pane's frames now (`cadence::interval`, `prefs` and `low_power` as
    /// the window's cadence reads them): `visible` when it is
    /// in the tab in front of a window on screen; `last_output` as the activity tracker has
    /// it (output right after a resize ignored), with output since its last tick counting
    /// as now.
    pub fn replay_gap(&mut self, now: Instant, visible: bool, last_output: Option<Instant>, prefs: &crate::cadence::Prefs, low_power: bool) -> Duration {
        let idle = !self.got_output && last_output.is_none_or(|t| now.saturating_duration_since(t) > crate::cadence::idle_after(self.idle_time));
        let alt = self.own().alt_screen();
        let state = crate::cadence::State { active: false, idle, visible, alt_screen: alt, throughput: self.throughput.estimate(now), low_power };
        crate::cadence::interval(state, prefs)
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
        let bytes = &self.decode_output(bytes.to_vec())[..];
        self.log_cooked(bytes);
        if let Some(p) = self.echo_probe.as_mut() {
            p.output(bytes);
        }
        self.got_output = true;
        self.watch_stuck(bytes);
        let reply = self.own_mut().feed(bytes);
        if !reply.is_empty() && self.tmux_io().is_none() {
            self.reply(&reply);
        }
    }

    /// Output the screen is about to see, in UTF-8: decoded from the session's code page while
    /// the profile names one. Output is decoded where the screen consumes it, in order, so a
    /// character cut by a read waits for the next; a tmux gateway's control stream and a tmux
    /// pane's output are always UTF-8 and never decoded (the coprocess's raw copy is taken
    /// earlier, undecoded).
    fn decode_output(&mut self, bytes: Vec<u8>) -> Vec<u8> {
        if self.gateway || self.tmux_io().is_some() {
            return bytes;
        }
        match self.decoder.as_mut() {
            Some(d) => d.feed(&bytes, crate::encoding::system()),
            None => bytes,
        }
    }

    /// Bytes bound for the session with the text in them in the session's encoding.
    fn encode_text<'a>(&self, bytes: &'a [u8]) -> std::borrow::Cow<'a, [u8]> {
        // tmux control mode is UTF-8 (`PTYSession.m:10958`): a tmux pane and its gateway never encode.
        match self.encoding {
            0 => std::borrow::Cow::Borrowed(bytes),
            _ if self.gateway || self.tmux_io().is_some() => std::borrow::Cow::Borrowed(bytes),
            cp => std::borrow::Cow::Owned(crate::encoding::encode_stream(cp, bytes, crate::encoding::system())),
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
        !self.ssh_files_out.is_empty() || !self.ssh_search_out.is_empty() || self.conductor.as_ref().is_some_and(|c| c.has_effects())
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
    /// a conductor's `send_keys` when one sends for the session (the app applies its effects
    /// in `ssh_pump`), else the connecting queue, else `send_routed`.
    pub fn write_session(&mut self, bytes: &[u8]) {
        if self.exited.is_some() {
            return;
        }
        if self.ssh_route() == crate::conductor::Route::Conductor(0) || !self.ssh.queue(bytes) {
            self.send_routed(bytes);
        }
    }

    /// THE route of every byte for the session's transport but the conductors' own
    /// (`Effect::Write`, `write_no_broadcast`): `conductor::route` picks it. Framed by the
    /// conductor that sends for the session (the current one when it handles keystrokes, else
    /// the nearest ancestor whose framer runs), held while the current one sets up, else to
    /// the pty (iTerm2's `writeTaskNoBroadcast`, `PS:4276-4294`, `PS:4191-4197`). False when
    /// the pty refused it.
    pub fn send_routed(&mut self, bytes: &[u8]) -> bool {
        use crate::conductor::Route;
        // An ended session takes nothing: its console is dead, and `ssh_held` would only grow
        // (13q B8).
        if bytes.is_empty() || self.exited.is_some() {
            return true;
        }
        self.composer.probe = None;
        match self.ssh_route() {
            Route::Conductor(hops) => {
                if let Some(c) = self.conductor.as_deref_mut().and_then(|c| c.ancestor_mut(hops)) {
                    c.send_keys(bytes);
                }
                true
            }
            Route::Held => {
                self.ssh_held.extend_from_slice(bytes);
                true
            }
            Route::Direct => self.write_pty(bytes),
        }
    }

    /// iTerm2's `conductorWrite` outside `write()` (a failed setup's `exec_login_shell` line):
    /// to the pty when nothing frames; through the framer above when the conductor is still
    /// setting up under one (its conductor.sh reads the tty that framer's child owns, which is
    /// where iTerm2's raw write lands); otherwise held, and the abort that follows drops it (a
    /// framing conductor has no conductor.sh to read it, an unhooked one's is gone).
    fn write_unless_held(&mut self, bytes: &[u8]) {
        use crate::conductor::Route;
        if self.exited.is_some() {
            return;
        }
        let setting_up = self.conductor.as_deref().is_some_and(|c| !c.framing() && !c.unhooked());
        let above = self.conductor.as_deref().and_then(|c| c.framing_ancestor());
        match self.ssh_route() {
            // Setting up under a framer that runs: the route holds typing for the setup, but
            // this line is for the setup's own conductor.sh, which reads the tty of that
            // framer's child: framed through it (and every framer above).
            Route::Held if setting_up && above.is_some() => {
                // Sent from that framer itself, which the abort that follows does not pop.
                if let Some(c) = above.and_then(|h| self.conductor.as_deref_mut().and_then(|c| c.ancestor_mut(h))) {
                    c.send_keys(bytes);
                }
            }
            Route::Direct => {
                self.write_pty(bytes);
            }
            Route::Conductor(hops) if hops >= 1 && setting_up => {
                self.send_routed(bytes);
            }
            _ => {
                self.composer.probe = None;
                self.ssh_held.extend_from_slice(bytes);
            }
        }
    }

    /// Whether the queue of keystrokes of the conductor that sends for the session is too
    /// long for the coprocess to add to (F20): the pump stops draining it until replies come
    /// back.
    pub fn coprocess_backlogged(&self) -> bool {
        match self.ssh_route() {
            // Setup holds the bytes here: the same limit, or a coprocess grows them without end.
            crate::conductor::Route::Held => self.ssh_held.len() > COPROCESS_BACKLOG,
            crate::conductor::Route::Conductor(hops) => self.conductor.as_deref().and_then(|c| c.ancestor(hops)).is_some_and(|c| c.queued_bytes() > COPROCESS_BACKLOG),
            crate::conductor::Route::Direct => false,
        }
    }

    /// A coprocess's output (`PK:354-361`, `PS:4639-4642`): `send_routed`, held during setup,
    /// framed while a conductor sends for the session. False when the pty refused it.
    pub fn coprocess_output(&mut self, bytes: &[u8]) -> bool {
        self.send_routed(bytes)
    }

    /// The conductor bootstrap for it2ssh's `SendConductor` (`PS:20290-20300`), when the hook
    /// exists: `writeTaskNoBroadcast`, so framed when it2ssh runs in a framed shell (`PS:4280-4285`),
    /// never typed into framer.py's stdin.
    pub fn ssh_send_conductor(&mut self, h: &crate::ssh_handshake::Handshake) {
        if let Some(bytes) = crate::ssh_session::conductor_bytes(h, crate::ssh_session::CONDUCTOR_READY) {
            self.reply(&bytes);
        }
    }

    /// The writes held while the conductor set up, in order.
    pub fn take_ssh_held(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.ssh_held)
    }

    /// The pane is an ssh session (F27): an OSC 134 outside the hook may now start an in-band
    /// recovery, and the pane saves the fact. Called only when a hook Tandem's origin token
    /// vouched for hooks (`note_hook`) and for a restored pane that saved the fact; never from
    /// output alone (the it2ssh OSC, an unvouched hook, a restored conductor), so a hostile
    /// file cannot make Tandem type into a shell that never ran a vouched ssh (ruling). The
    /// right ends when EndSSH empties the hop list (`end_ssh`, final review I2), so it does not
    /// hold at the local prompt after ssh exits either.
    pub fn note_ssh_session(&mut self) {
        self.ssh.was_ssh = true;
        self.ssh_hook.allow_recovery();
    }

    /// it2ssh announced itself (OSC 1337 it2ssh, `T:4524-4527`): input typed now waits for
    /// the conductor.
    /// Any program can print it, so it gives no right to recover (F27).
    pub fn ssh_will_begin(&mut self) {
        self.ssh.begin(crate::ssh_session::CONDUCTOR_READY);
    }

    /// A conductor hooked; `vouched` is whether its hook carried Tandem's origin token. Only
    /// a vouched one makes the pane an ssh session (F27, ruling).
    pub fn note_hook(&mut self, vouched: bool) {
        if vouched {
            self.note_ssh_session();
        }
    }

    /// Whether `note_ssh_session` was called.
    pub fn was_ssh_session(&self) -> bool {
        self.ssh.was_ssh
    }

    /// Leaves the conductor without an `unhook`: what follows is output again.
    pub fn unhook_ssh(&mut self) {
        self.ssh_hook.unhook();
    }

    /// The session started: whether it is an SSH profile's, and so waits for a login.
    pub fn start_as_profile(&mut self, ssh: bool) {
        self.ssh.profile_ssh = ssh;
        self.ssh.initial_text_waits = ssh;
    }

    /// The it2ssh hook took the session over (`PS:20129-20132`): it is no longer "from a profile".
    pub fn ssh_hook_took_over(&mut self) {
        self.ssh.profile_ssh = false;
    }

    /// A conductor logged in: an SSH profile's Initial Text is now due, once per session. A
    /// session that is not an SSH profile's (a typed `ssh` an it2ssh hook took over) already
    /// sent its text at start and never gets it again.
    pub fn conductor_login_text(&mut self) {
        if self.ssh.initial_text_waits && !self.initial_text_sent {
            self.initial_text_due = true;
        }
    }

    /// True once when the Initial Text is due; the caller types it.
    pub fn take_initial_text_due(&mut self) -> bool {
        let due = std::mem::take(&mut self.initial_text_due);
        self.initial_text_sent |= due;
        due
    }

    /// The changes to the connected hosts since the last call, in order.
    pub fn take_ssh_registry(&mut self) -> Vec<crate::ssh_registry::Change> {
        std::mem::take(&mut self.ssh_registry_out)
    }

    /// The guids of the pane's conductors, innermost first: what leaves the connected hosts
    /// when the pane goes.
    pub fn ssh_guids(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut c = self.conductor.as_deref();
        while let Some(conductor) = c {
            out.push(conductor.guid.clone());
            c = conductor.parent.as_deref();
        }
        out
    }

    /// Whether the innermost conductor's remote tty is at a password prompt (`PS:7659`).
    pub fn ssh_password_prompt(&self) -> bool {
        self.conductor.as_ref().is_some_and(|c| c.at_password_prompt())
    }

    /// The pane's SSH integration level (`conductor::level`).
    pub fn ssh_level(&self) -> u8 {
        crate::conductor::level(self.conductor.as_deref())
    }

    /// Whether the current conductor's framer is running to answer a request. Not the level
    /// (`ssh_level` stays 2 after the framer unhooks, as iTerm2's `sshIntegrationLevel` does):
    /// a request queued on an unhooked conductor would wait for EndSSH.
    pub fn ssh_frames(&self) -> bool {
        self.conductor.as_deref().is_some_and(|c| c.frames_keys())
    }

    /// unhookSSHConductor (`PS:20179-20204`) for the screen: the innermost conductor goes,
    /// its saved state is restored, and its parent becomes current. The hops are the caller's.
    /// False when there was none.
    pub fn pop_conductor(&mut self) -> bool {
        let Some(mut c) = crate::conductor::pop(&mut self.conductor) else { return false };
        // Nothing reads its effects again: the file requests it still owes are answered (-1).
        c.abort_files();
        for effect in c.take_effects() {
            if let crate::conductor::Effect::FileDone { id, content, status } = effect {
                self.ssh_files_out.push((id, content, status));
            }
        }
        // The next job comes from the parent's remote or the local shell again.
        self.ssh.jobs_dirty = true;
        self.ssh_registry_out.push(crate::ssh_registry::Change::Remove(c.guid.clone()));
        // Script calls it still owed an answer get one: nothing reads its effects again.
        for token in c.abandoned_time_tokens() {
            self.ssh_answers.push((token, Err("Failed to fetch time: the connection ended".to_string())));
        }
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
        // Back at the local prompt: the pane is no longer an ssh session, so output may not
        // start a recovery there, and nothing saves the right for after a restart (final
        // review I2, ruling). Only a new vouched hook gives it again.
        if self.ssh.chain.hops.is_empty() {
            self.ssh.was_ssh = false;
            self.ssh_hook.forbid_recovery();
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

    /// A `Token::Unhook` from the hook at `depth` (`PS:20172-20177`, which has no depth): the
    /// conductor at that depth unhooks and the held writes go out raw. Beyond iTerm2, every
    /// conductor deeper than `depth` is popped first, innermost first: their remote sits
    /// behind the process that just ended, and a conductor left framing keys would type them
    /// into the host above (`cat evil; exit` inside a deep session). An unhook for a depth
    /// with no conductor (it is gone) is dropped, so it never unhooks a healthy shallower one.
    pub fn unhook_conductor_at(&mut self, depth: i32) {
        let mut popped_setup = false;
        while let Some(c) = self.conductor.as_deref().filter(|c| c.depth() > depth) {
            popped_setup |= c.queue_writes() && !c.handles_keystrokes();
            self.ssh_unhook_conductor();
        }
        if popped_setup {
            // Keys held for a setup that is gone were for its remote.
            self.ssh_held.clear();
        }
        let Some(c) = self.conductor.as_mut().filter(|c| c.depth() == depth) else { return };
        c.handle_unhook();
        // Then routed as any write: to a framer above when one runs (Tandem-side guard: iTerm2's
        // `writeData:` types them raw, `PS:20176-20180`).
        let held = self.take_ssh_held();
        self.send_routed(&held);
    }

    /// terminalBeginFramerRecoveryForChildOfConductorAtDepth and screenBeginFramerRecovery
    /// (`TD:4067-4071`, `PS:20307-20342`): "Recovering ssh connection..." on the screen; a
    /// depth below 0 unhooks every conductor first; then a shim takes the previous innermost
    /// as its parent and asks the framer to recover. A depth the conductors do not have (the
    /// shim would sit where the hook has no owner for it) is not started: nothing is written.
    pub fn begin_framer_recovery(&mut self, parent_depth: i32) {
        if parent_depth >= 0 && crate::conductor::child_depth(self.conductor.as_deref()) != parent_depth + 1 {
            self.ssh_hook.end_recovery();
            return;
        }
        self.own_mut().append_banner(crate::ssh_recovery::RECOVERING);
        // The it2 state of the conductor being recovered rides the shim (`PS:20307-20336`,
        // `adoptIT2RecoveryStateFrom:`); Tandem's is the inbox kept for 14e (ruling 3).
        let it2 = self.conductor.as_ref().map(|c| c.it2_inbox.clone()).unwrap_or_default();
        // A root-level recovery replaces every hop; the outermost one's id goes to the shim and
        // the recovered conductor, so it2ssh's EndSSH, which names it, still ends the session
        // whatever id the remote reports (final review I2, ruling).
        self.ssh.recovery_replaces = None;
        if parent_depth < 0 {
            self.ssh.recovery_replaces = self.ssh.chain.hops.first().map(|h| h.unique_id.clone());
            while self.conductor.is_some() {
                self.ssh_unhook_conductor();
            }
        }
        let mut shim = crate::conductor::Conductor::recovery_shim(self.conductor.take());
        if let Some(id) = &self.ssh.recovery_replaces {
            shim.setup.client_unique_id = id.clone();
        }
        shim.it2_inbox = it2;
        shim.start_recovery();
        let location = self.ssh.chain.location().unwrap_or_default().to_string();
        self.ssh.chain.push(crate::ssh_session::Hop { unique_id: shim.setup.client_unique_id.clone(), location });
        self.conductor = Some(Box::new(shim));
        debug_assert!(self.ssh_chain_in_step(), "the hops follow the conductors");
        self.apply_ssh_effects();
    }

    /// screenHandleFramerRecoveryString (`PS:20344-20370`): one recovery line for the shim. When
    /// the last one finishes the recovery the shim is replaced by the recovered conductor, its
    /// hop and registry entry, the hook follows its tree, "ssh connection recovered!" is shown
    /// and it resynchronizes (`TD:4074-4090`); true then, and the rest of the batch is dropped
    /// as iTerm2 drops the tokens read before the parser was replaced. A recovery that ended
    /// without a login (the shim quit) leaves recovery mode for the hook. Only a shim in
    /// recovery reads a line.
    pub fn framer_recovery_line(&mut self, line: &str) -> bool {
        use crate::conductor::State;
        let Some(shim) = self.conductor.as_mut().filter(|c| matches!(c.state(), State::Recovery(_))) else {
            self.ssh_hook.end_recovery();
            return false;
        };
        let Some(finished) = shim.handle_recovery_line(line) else {
            self.apply_ssh_effects();
            if !self.conductor.as_ref().is_some_and(|c| matches!(c.state(), State::Recovery(_))) {
                self.ssh_hook.end_recovery();
            }
            return false;
        };
        let (parent, it2) = self.conductor.take().map(|mut shim| (shim.parent.take(), std::mem::take(&mut shim.it2_inbox))).unwrap_or_default();
        self.ssh.chain.pop();
        let mut recovered = crate::conductor::Conductor::from_recovery(&finished, parent);
        if let Some(id) = self.ssh.recovery_replaces.take() {
            recovered.setup.client_unique_id = id;
        }
        // The shim's it2 state goes on (`handleRecovery` reads it off the shim).
        recovered.it2_inbox = it2;
        self.conductor = Some(Box::new(recovered));
        self.adopt_recovered(&finished.dcs_id, &finished.tree());
        self.own_mut().append_banner(crate::ssh_recovery::RECOVERED);
        true
    }

    /// The conductor just made from a recovery (already the pane's innermost) gets its hop and
    /// registry entry, the hook follows `tree` (in recovery mode), and it resynchronizes. What
    /// was typed meanwhile goes out through it.
    fn adopt_recovered(&mut self, dcs_id: &str, tree: &crate::ssh_recovery::Tree) {
        let Some(c) = self.conductor.as_ref() else { return };
        let hop = crate::ssh_session::Hop { unique_id: c.setup.client_unique_id.clone(), location: c.identity().compact(&crate::ssh_args::local_user()) };
        self.ssh.chain.push(hop);
        self.ssh_registry_out.push(crate::ssh_registry::Change::Add(c.guid.clone(), c.identity()));
        self.ssh_hook.start_conductor_recovery(dcs_id, tree);
        if let Some(c) = self.conductor.as_mut() {
            c.did_resynchronize();
        }
        debug_assert!(self.ssh_chain_in_step(), "the hops follow the conductors");
        self.apply_ssh_effects();
        let held = self.take_ssh_held();
        self.send_routed(&held);
    }

    /// The session's conductor chain from its saved state (`PS:2284-2298`), for a pane that
    /// re-attached to its hosted session: the conductors come from `json` (untrusted: false
    /// when it does not read), a hop and a registry entry for each framing one, the hook in
    /// recovery mode with `tree`, and the framers reset at once (Tandem's hook swap is
    /// synchronous, so iTerm2's boundary token is not needed). False, and nothing changed, when
    /// there is nothing usable to restore. The conductors are unvouched, so they give no right
    /// to recover in band: only the saved "SSH Session" flag does (F27, applied by the caller
    /// after this, since the hook is replaced here).
    pub fn restore_conductor(&mut self, json: &str, dcs_id: &str, tree: &crate::ssh_recovery::Tree) -> bool {
        if self.conductor.is_some() || !tree.valid() || tree.is_empty() {
            return false;
        }
        let Some(restored) = crate::conductor::Conductor::from_json(json) else { return false };
        let mut hops = Vec::new();
        let mut adds = Vec::new();
        let mut c = Some(&restored);
        while let Some(conductor) = c {
            hops.push(crate::ssh_session::Hop { unique_id: conductor.setup.client_unique_id.clone(), location: conductor.identity().compact(&crate::ssh_args::local_user()) });
            if conductor.framing() {
                adds.push(crate::ssh_registry::Change::Add(conductor.guid.clone(), conductor.identity()));
            }
            c = conductor.parent.as_deref();
        }
        hops.reverse();
        adds.reverse();
        self.ssh.chain.hops = hops;
        self.ssh_registry_out.extend(adds);
        self.conductor = Some(Box::new(restored));
        self.ssh_hook = crate::ssh_hook::Hook::recovering(dcs_id, tree, 0);
        if let Some(c) = self.conductor.as_mut() {
            c.did_resynchronize();
        }
        debug_assert!(self.ssh_chain_in_step(), "the hops follow the conductors");
        self.apply_ssh_effects();
        true
    }

    /// conductorQuit (`PS:25603-25612`): "Disconnected from <identity>" on the screen, then the
    /// innermost conductor and its hop leave (`unhookSSHConductor`, which pops the hop too, so
    /// the EndSSH that follows finds none and writes nothing), and the held writes are cleared.
    fn ssh_quit(&mut self) {
        let Some((identity, named)) = self.conductor.as_ref().map(|c| (c.identity().description(), !c.setup.parsed.hostname.is_empty())) else { return };
        // A recovery shim that quit has no host to name.
        if named {
            self.own_mut().append_banner(&crate::conductor_framer::disconnected_banner(&identity));
        }
        self.ssh_unhook_conductor();
        self.take_ssh_held();
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
    /// conductor and its hop go. The conductor that aborted is the outermost one `fail` marked
    /// (a parent's abort reaches the pane through its child's effects), else the innermost;
    /// every conductor inside it goes first, innermost first, so its saved state is what
    /// remains. Then Tandem's part: the DCS hook that owns its depth lets go, and every hook
    /// under it, so later output shows (ruling 14b-3, iTerm2 bugs B4 and B13), while the hooks
    /// above keep parsing for their conductors (iTerm2 never force-unhooks the parser); the
    /// held writes (the raw `exec_login_shell` line among them) are dropped (iTerm2 strands
    /// them); connecting ends and what was typed meanwhile is dropped (controller ruling:
    /// those keys were for the remote shell, and conductor.sh may still be reading command
    /// lines). Typing after this goes straight to the pty.
    pub fn abort_ssh(&mut self, reason: &str) {
        let mut chain = Vec::new();
        let mut c = self.conductor.as_deref();
        while let Some(conductor) = c {
            chain.push(conductor);
            c = conductor.parent.as_deref();
        }
        let at = chain.iter().rposition(|c| c.failed()).unwrap_or(0);
        let depth = chain.get(at).map_or(0, |c| c.depth());
        let location = chain.get(at).and_then(|c| c.parent.as_ref()).map(|p| p.identity().compact(&crate::ssh_args::local_user()));
        let screen = self.own_mut();
        screen.print_message(crate::conductor::SETUP_ERROR);
        screen.print_message(&crate::conductor::shown(reason));
        screen.append_banner(&crate::ssh_handshake::end_banner(1, location.as_deref()));
        for _ in 0..=at {
            self.ssh_unhook_conductor();
        }
        if depth == 0 {
            self.ssh_hook.unhook();
        } else {
            self.ssh_hook.let_go_owner(depth);
        }
        self.ssh_held.clear();
        // Connecting ends so later typing is not queued behind a dead conductor; its queue goes.
        self.ssh.stop_queueing();
    }

    /// Whether a conductor hooked at `depth` sits at that depth in the pane's chain
    /// (`conductor::child_depth`). When not (a hook deeper or shallower than the conductors
    /// allow, or a second one at a framed conductor's own depth), the hook that read the init
    /// line lets go and no conductor is made (Tandem's: iTerm2 chains it at the depth its
    /// parent gives, and routing by depth then reaches the wrong conductor).
    pub fn ssh_init_fits(&mut self, depth: i32) -> bool {
        if crate::conductor::child_depth(self.conductor.as_deref()) == depth {
            return true;
        }
        self.ssh_hook.let_go_owner(depth);
        false
    }

    /// A parser at `depth` lost the remote's output or was refused more of it: every conductor
    /// at that depth or deeper fails, innermost first, each aborting through `abort_ssh`. With
    /// none at that depth or below (the conductor is already gone) the error is dropped, so it
    /// never fails a healthy shallower one. True when the pane must be redrawn.
    pub fn ssh_error(&mut self, depth: i32, reason: &str) -> bool {
        let mut redraw = false;
        for _ in 0..=crate::ssh_hook::MAX_DEPTH + 1 {
            match self.conductor.as_mut() {
                Some(c) if c.depth() >= depth => c.fail(reason),
                _ => break,
            }
            redraw |= self.apply_ssh_effects();
        }
        redraw
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
            let mut effects = effects.into_iter();
            while let Some(effect) = effects.next() {
                match effect {
                    Effect::Write(bytes) => self.write_no_broadcast(&bytes),
                    Effect::SessionWrite(bytes) => self.write_unless_held(&bytes),
                    // Unframed sendKeys is the session's conductorWrite, which is
                    // writeTaskNoBroadcast (`CD:1854-1862`, `PS:25519-25522`): held while setup
                    // runs, never the connecting queue. A conductor that handles keystrokes
                    // sends them framed (14c); unframed, `reply` would hand them straight back.
                    Effect::Keys(bytes) => {
                        debug_assert!(self.ssh_route() != crate::conductor::Route::Conductor(0), "unframed keys from a conductor that handles keystrokes");
                        self.reply(&bytes)
                    }
                    // `PS:25614-25617`: what was typed while connecting goes through sendKeys,
                    // ahead of later typing (routed: the conductor that sends for the session, or
                    // held while the current one sets up); its effects are applied by the next pass.
                    Effect::StopQueueingInput => {
                        let queued = self.ssh.stop_queueing();
                        self.send_routed(&queued);
                    }
                    // `PS:25565`: after the conductor's login, an SSH profile's Initial Text
                    // is typed (the app does it, in the pass that drains the answers).
                    Effect::SendInitialText => self.conductor_login_text(),
                    Effect::StateChanged => redraw = true,
                    // conductorQuit (`PS:25603-25612`): the banner, the conductor and its hop
                    // go, and the writes it held are dropped. Its own quit was written first.
                    Effect::Quit => {
                        self.ssh_quit();
                        redraw = true;
                    }
                    Effect::ProcessesChanged => self.ssh.jobs_dirty = true,
                    Effect::Registered { guid, identity } => self.ssh_registry_out.push(crate::ssh_registry::Change::Add(guid, identity)),
                    Effect::Deregistered { guid } => self.ssh_registry_out.push(crate::ssh_registry::Change::Remove(guid)),
                    Effect::TimeOffset { token, result } => redraw |= self.time_offset(token, result),
                    Effect::FileDone { id, content, status } => self.ssh_files_out.push((id, content, status)),
                    Effect::SearchNotif(text) => {
                        if self.ssh_search_out.len() < SEARCH_QUEUE_MAX {
                            self.ssh_search_out.push(text);
                        }
                    }
                    Effect::Notice(text) => {
                        self.own_mut().append_banner(&text);
                        redraw = true;
                    }
                    // The rest of the batch came from the failed setup (keys for its remote,
                    // its writes) and goes with it: after the pop the route is Direct, so
                    // applying it would send those bytes to whatever runs now. Only the
                    // registry changes survive: a conductor that is still here registers, and
                    // a deregistration always applies (the parents' effects are drained ahead
                    // of the child's, so nothing of a live conductor may be lost here).
                    Effect::Abort(reason) => {
                        self.abort_ssh(&reason);
                        redraw = true;
                        let live = self.ssh_guids();
                        for rest in effects.by_ref() {
                            match rest {
                                Effect::Registered { guid, identity } if live.contains(&guid) => self.ssh_registry_out.push(crate::ssh_registry::Change::Add(guid, identity)),
                                Effect::Deregistered { guid } => self.ssh_registry_out.push(crate::ssh_registry::Change::Remove(guid)),
                                // A script call is answered even when its batch is dropped.
                                Effect::TimeOffset { token: Some(t), result } => self.ssh_answers.push((t, result)),
                                // A file request is answered even when its batch is dropped (P5).
                                Effect::FileDone { id, content, status } => self.ssh_files_out.push((id, content, status)),
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
        redraw
    }

    /// A clock reply: a script call's is kept for the app to deliver, the Clock's own sets the
    /// pane's Clock. True when the bar's text may change.
    fn time_offset(&mut self, token: Option<u64>, result: Result<crate::ssh_clock::Offset, String>) -> bool {
        match token {
            Some(t) => {
                self.ssh_answers.push((t, result));
                false
            }
            None => {
                self.ssh_clock.set(result);
                true
            }
        }
    }

    /// The script calls this pane's conductors still owe an answer (fetching, or finished
    /// with the effect not yet taken). A pane that is dropped answers them with an error.
    pub fn owed_answers(&self) -> Vec<u64> {
        let mut tokens = Vec::new();
        let mut c = self.conductor.as_deref();
        while let Some(conductor) = c {
            tokens.extend(conductor.abandoned_time_tokens());
            c = conductor.parent.as_deref();
        }
        tokens
    }

    /// The file requests answered since the last call, in order: (id, content, status).
    pub fn take_ssh_files(&mut self) -> Vec<(u64, String, i32)> {
        std::mem::take(&mut self.ssh_files_out)
    }

    /// The `%notif search` texts received since the last call, in order.
    pub fn take_ssh_search(&mut self) -> Vec<String> {
        std::mem::take(&mut self.ssh_search_out)
    }

    /// The answers to script calls the conductors finished, taken.
    pub fn take_ssh_answers(&mut self) -> Vec<(u64, Result<crate::ssh_clock::Offset, String>)> {
        std::mem::take(&mut self.ssh_answers)
    }

    /// Asks the pane's Clock what to do at `now` for its SSH level and host name (`CLK:52-99`)
    /// and does it: at level 2 the current conductor fetches the remote's time. Returns what
    /// it decided.
    pub fn ssh_clock_tick(&mut self, now: std::time::Instant, host: Option<&str>) -> crate::ssh_clock::Refresh {
        use crate::ssh_clock::Refresh;
        // A framer that has unhooked answers nothing: the Clock is local, as below level 2.
        let level = if self.ssh_frames() { i32::from(self.ssh_level()) } else { i32::from(self.ssh_level().min(1)) };
        let refresh = self.ssh_clock.due(level, host, now);
        if refresh == Refresh::Fetch {
            let wall = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
            if let Some(c) = self.conductor.as_mut() {
                c.fetch_time_offset(wall, crate::ssh_clock::monotonic(std::time::Instant::now()), None);
            }
            self.apply_ssh_effects();
        }
        refresh
    }

    /// When the Clock next fetches (level 2 only).
    pub fn clock_due(&self) -> Option<std::time::Instant> {
        self.ssh_clock.next_due()
    }

    /// The command timer of the pane's current conductor at `now`, then its effects. True
    /// when the pane must be redrawn.
    pub fn tick_ssh(&mut self, now: std::time::Instant) -> bool {
        let Some(c) = self.conductor.as_mut() else { return false };
        c.tick(now);
        self.apply_ssh_effects()
    }

    /// The remote job and ancestors of the innermost conductor that frames, once its process
    /// list has arrived; None shows the local watcher's values (`PS:25574-25581`).
    pub fn remote_job(&self) -> Option<(Option<crate::proctree::Job>, Vec<String>)> {
        self.conductor.as_deref()?.innermost_framing()?.remote_job()
    }

    /// The names of the remote programs for the close prompt (`CD:684-694`), once a process
    /// list has arrived; None asks the local tree.
    pub fn remote_process_names(&self) -> Option<Vec<String>> {
        let c = self.conductor.as_deref()?.innermost_framing()?;
        c.remote_job()?;
        Some(c.transitive_process_names())
    }

    /// The remote CPU values of the innermost framing conductor, in place of the local meter.
    pub fn remote_cpu(&self) -> Option<&crate::statusbar_metrics::Series> {
        self.conductor.as_deref()?.innermost_framing()?.procs.as_ref().map(|p| &p.cpu_series)
    }

    /// A conductor hooked: the directory the pane has now is not the remote's, so only a later
    /// change is told to the conductor.
    pub fn note_ssh_cwd(&mut self) {
        self.ssh.cwd_seen = self.own().shell.cwd.clone();
    }

    /// didUpdateCurrentDirectory (`PS:19373-19376`): when the shell's directory changed since
    /// last looked at, the innermost conductor learns it as its current directory, for a later
    /// reconnection. The remote chose the text, so a directory that is not one line is not
    /// kept. True when the directory changed.
    pub fn track_ssh_cwd(&mut self) -> bool {
        let Some(conductor) = self.conductor.as_mut() else { return false };
        let cwd = self.live.as_ref().unwrap_or(&self.screen).shell.cwd.clone();
        if cwd == self.ssh.cwd_seen {
            return false;
        }
        if let Some(dir) = cwd.as_deref().and_then(crate::ssh_jumps::clean_directory) {
            conductor.current_directory = Some(dir);
        }
        self.ssh.cwd_seen = cwd;
        true
    }

    /// Whether the job must be read again (taken).
    pub fn take_jobs_dirty(&mut self) -> bool {
        std::mem::take(&mut self.ssh.jobs_dirty)
    }

    /// The once-a-second publish of every framing conductor's CPU value (making each process
    /// list at first use), then the effects that caused. True when a value was pushed.
    pub fn tick_procs(&mut self, now: std::time::Instant) -> bool {
        let framing = self.conductor.as_ref().is_some_and(|c| c.frames_anywhere());
        if !framing {
            self.ssh.procs_due = None;
            return false;
        }
        if self.ssh.procs_due.is_some_and(|due| now < due) {
            return false;
        }
        self.ssh.procs_due = Some(now + std::time::Duration::from_secs(1));
        let pushed = self.conductor.as_mut().is_some_and(|c| c.tick_processes());
        self.apply_ssh_effects();
        pushed
    }

    /// When the process lists next publish.
    pub fn procs_due(&self) -> Option<std::time::Instant> {
        self.ssh.procs_due
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
        // An ended session takes no input (`insertText:` returns when exited, `PS:5214`).
        if self.gateway || self.exited.is_some() {
            return;
        }
        // Typed text, pastes and a trigger's Send Text leave in the session's encoding
        // (iTerm2's `dataUsingEncoding:self.encoding`); reports are ASCII and pass as they are.
        let encoded = self.encode_text(bytes).into_owned();
        let bytes = &encoded[..];
        // While SSH integration connects, writes wait for the conductor, ahead of Buffer
        // Input and before `writeTaskImpl`'s effects (`PS:4411-4430`) but after a conductor
        // that handles keystrokes: a nested hop's password reaches the outer framer (F9,
        // `PS:4418-4426`).
        // Only the current conductor's own keys skip the queue: with a hop connecting from
        // a level 1 host (route `Conductor(n)`, n >= 1) typing waits, as iTerm2 queues unless the
        // current conductor handles keystrokes.
        let routed = matches!(self.ssh_route(), crate::conductor::Route::Conductor(_));
        if self.ssh_route() != crate::conductor::Route::Conductor(0) && self.ssh.queue(bytes) {
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
        // A conductor's keys go before any buffering (`PS:4417-4431`): Buffer Input never
        // holds them.
        if routed {
            self.send_routed(bytes);
        } else if let Some(now) = self.buffer.send(bytes) {
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

    /// What the program wrote, as read: Raw and asciicast logs take it (`TK:888-893`). A tmux
    /// client's reads are `%output` and only a Raw log takes them (`PS:10983-10985`).
    fn log_raw(&mut self, bytes: &[u8]) {
        let tmux = self.tmux_io().is_some();
        let (Some(log), Some(started)) = (self.log.as_mut(), self.log_started) else { return };
        if tmux && log.style() != crate::session_log::Style::Raw {
            return;
        }
        log.raw(bytes, started.elapsed().as_secs_f64());
    }

    /// Text the screen is about to take, for plain text and HTML logs (`logCooked:`, `PS:15592`):
    /// never a tmux gateway's, and with compact logging none at the prompt while the Auto
    /// Composer is on.
    fn log_cooked(&mut self, bytes: &[u8]) {
        if self.gateway || self.log.is_none() {
            return;
        }
        let at_prompt = self.own().at_prompt() == Some(true);
        let (smart, auto) = self.log_compact;
        if crate::session_log::skip_compact(smart, auto, at_prompt) {
            return;
        }
        let (parts, millis) = crate::session_log::now_parts();
        // The pane's own palette, not a Filter or Zoom view's.
        let own_palette = self.own().palette.clone();
        if let Some(log) = self.log.as_mut() {
            log.cooked(bytes, &own_palette, (&parts, millis));
        }
    }

    /// Compact logging's own text (the revealed prompt, a sent command): plain text and HTML only.
    pub fn log_text(&mut self, text: &str) {
        let own_palette = self.own().palette.clone();
        let Some(log) = self.log.as_mut() else { return };
        let (parts, millis) = crate::session_log::now_parts();
        log.cooked_text(text, &own_palette, (&parts, millis));
    }

    /// Ends the log: nothing is written after this (`PS:4728`, the close). What was queued still
    /// reaches the file on the writer's thread.
    pub fn stop_log(&mut self) {
        if let Some(mut log) = self.log.take() {
            log.stop();
        }
        self.log_started = None;
    }

    /// `stop_log` at quit: waits up to `wait` for the queued bytes to reach the file.
    pub fn close_log(&mut self, wait: Duration) {
        if let Some(mut log) = self.log.take() {
            log.close(wait);
        }
        self.log_started = None;
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

    /// The shell's process id, while it runs: None once it exited, since Windows reuses pids and
    /// the job watcher, the close prompt and the process waits would read a stranger's tree
    /// (13q B7).
    pub fn shell_pid(&self) -> Option<u32> {
        if self.exited.is_some() {
            return None;
        }
        self.pty.as_ref().and_then(|p| p.pid())
    }

    /// A restart over a live shell ends it first (`PS:3863-3871`): its console goes (the shell and
    /// what it runs end, which the person confirmed) and the pane is an ended one, so a failed
    /// start leaves an ended pane.
    pub fn end_live_shell(&mut self) {
        if self.exited.is_some() {
            return;
        }
        self.pty = None;
        self.exited = Some(0);
        self.shell_gen += 1;
        self.ended = true;
        self.survivors.clear();
    }

    /// The pid the shell had when it exited, for the survivor survey only.
    pub fn exit_pid(&self) -> Option<u32> {
        self.exit_pid
    }

    /// Takes the survey of what the exited shell left running (once, at the end), by process id
    /// and creation time: the creation time guards against a reused pid (13q B7).
    pub fn survey_survivors(&mut self) {
        let Some(pid) = self.exit_pid.filter(|_| self.owns_console() || self.host_session().is_some()) else { return };
        if let Ok(left) = crate::jobwatch::orphans_of(pid) {
            self.survivors = left.into_iter().filter(|p| !p.gui).map(|p| (p.pid, p.created)).filter(|(_, made)| *made != 0).collect();
        }
    }

    /// What a restart question asks about now: this shell and the programs it left running.
    pub fn restart_subject(&self) -> crate::session_end::AskedAbout {
        crate::session_end::AskedAbout { generation: self.shell_gen, survivors: if self.exited.is_some() { self.live_survivors() } else { Vec::new() } }
    }

    /// The names of the programs the exited shell left that still run; they end when the pane
    /// closes (the console goes), so the close asks.
    pub fn live_survivors(&self) -> Vec<String> {
        if self.survivors.is_empty() {
            return Vec::new();
        }
        crate::jobwatch::still_running(&self.survivors)
    }

    /// When the end of an exited session is due: once its output has drained (a short wait after
    /// the exit is seen, `P6`); None when it is handled or running.
    pub fn end_due_at(&self) -> Option<Instant> {
        if self.exited.is_none() || self.ended {
            return None;
        }
        self.exit_seen.map(|t| t + END_DRAIN)
    }

    /// Whether the end may be handled at `now`: the drain wait has passed and the output is
    /// quiet, or the hard cap passed.
    pub fn end_ready(&mut self, now: Instant) -> bool {
        let Some(due) = self.end_due_at() else { return false };
        if now < due {
            return false;
        }
        // One more read: output still arriving holds the end back, up to the cap.
        let more = self.pump();
        !more || now >= due + END_DRAIN_CAP
    }

    /// What the terminal answers the program (reports), and Tandem's own writes that iTerm2
    /// makes with `broadcastAllowed:NO` (the idle code, upload answers, tmux commands, the
    /// conductor bootstrap): never held by Buffer Input nor the connecting queue
    /// (`PS:4364-4376`, `PS:4276-4294`), and routed as `writeTaskNoBroadcast` routes them
    /// (`send_routed`).
    pub fn reply(&mut self, bytes: &[u8]) {
        if self.exited.is_some() {
            return;
        }
        self.composer.probe = None;
        // Replies are raw: a report, a title report (UTF-8, `PTYSession.m:18338`), a tmux command,
        // a key the SSH queue already encoded. Only typed text, pastes and the answerback take
        // the session's encoding (`PTYSession.m:4364-4376`).
        self.send_routed(bytes);
    }

    /// Typed bytes Buffer Input let go: the session's typed route (`write_session`).
    fn send_typed(&mut self, bytes: &[u8]) {
        self.write_session(bytes);
    }

    /// The conductors' own bytes (`Effect::Write`: conductor.sh command lines and framer
    /// commands, already framed by every framer they pass through): straight to the pty, past
    /// the connecting queue, Buffer Input and `send_routed`. Nothing else writes this way.
    fn write_no_broadcast(&mut self, bytes: &[u8]) {
        // A dead console takes nothing, and the probe is about bytes that were really sent.
        if self.exited.is_some() {
            return;
        }
        self.composer.probe = None;
        self.write_pty(bytes);
    }

    /// The pty write itself. False when the pty refused it.
    fn write_pty(&mut self, bytes: &[u8]) -> bool {
        if self.exited.is_some() {
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

    pub fn paste(&mut self, text: &str) {
        let bytes = self.paste_payload(text);
        self.input(&bytes);
    }

    /// DEC mode 2048's report for a resize just done, when the program asked for one and the
    /// pane's size is ours to report (a tmux pane's is tmux's). The caller writes it.
    pub fn resize_report_if_on(&mut self, rows: u16, cols: u16, px: (u32, u32)) -> Option<Vec<u8>> {
        resize_report_bytes(self.own().resize_reports(), self.tmux_io().is_some(), rows, cols, px)
    }

    /// The grid a request for `cols` by `rows` really gets: tmux's size for a tmux pane, and a
    /// pane squeezed to nothing keeps 2x1.
    pub fn clamped_size(&self, cols: u16, rows: u16) -> (u16, u16) {
        let (cols, rows) = self.tmux_size.unwrap_or((cols, rows));
        (cols.max(2), rows.max(1))
    }

    /// Resizes the screen and the pty. A pane squeezed to nothing keeps 2x1. True when the
    /// size changed.
    pub fn resize(&mut self, cols: u16, rows: u16) -> bool {
        let (cols, rows) = self.clamped_size(cols, rows);
        if (self.own().term().columns(), self.own().term().screen_lines()) == (cols as usize, rows as usize) {
            return false;
        }
        if !self.frozen {
            self.screen.resize(cols as usize, rows as usize);
        }
        if let Some(live) = &mut self.live {
            live.resize(cols as usize, rows as usize);
        }
        if let Some(pty) = self.pty.as_ref().filter(|_| self.exited.is_none()) {
            if let Err(e) = pty.resize(cols, rows) {
                self.error = Some(e);
            }
        }
        if let (Some(log), Some(started)) = (self.log.as_mut(), self.log_started) {
            log.resize(cols, rows, started.elapsed().as_secs_f64());
        }
        true
    }

    /// Every write to the selection goes through here, so the live mode and the column window
    /// stay consistent with it.
    pub fn set_selection(&mut self, sel: Option<alacritty_terminal::selection::Selection>, mode: crate::seltext::Mode) {
        self.screen.set_selection(sel, mode);
    }

    pub fn clear_selection(&mut self) {
        self.screen.clear_selection();
    }

    /// iTerm2's Clear to Last Mark: clears from after the last mark above the prompt, then
    /// clears ConPTY's buffer too (keeping the cursor row), so both agree.
    pub fn clear_to_last_mark(&mut self) {
        self.end_view();
        let lines: Vec<i32> = self.screen.marks_in_order().iter().map(|(l, _)| *l).collect();
        let cursor = self.screen.term().grid().cursor.point.line.0;
        let start = crate::marks::clear_start(&lines, self.screen.current_prompt_line(), cursor);
        self.screen.clear_from(start);
        self.screen.clear_selection();
        if let Some(pty) = self.pty.as_ref().filter(|_| self.exited.is_none()) {
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
        self.screen.clear_selection();
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
        // A dead console has nothing to clear (13q B8).
        if let Some(pty) = self.pty.as_ref().filter(|_| self.exited.is_none()) {
            if let Err(e) = pty.clear() {
                self.error = Some(e);
            }
        }
    }

    pub fn scroll(&mut self, scroll: Scroll) {
        self.screen.term_mut().scroll_display(scroll);
        // Back at the live view the user's scroll has ended: its remainder goes (`PTYScrollView.m:117-121`).
        if self.screen.term().grid().display_offset() == 0 {
            self.wheel.scroll.reset();
        }
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

    /// The selected command's region in the pane's own screen, in buffer rows, inclusive (13s
    /// P15). None with no command selected and while the alternate screen shows. A mark that is
    /// gone ends the selection.
    pub fn own_region(&mut self) -> Option<(usize, usize)> {
        let mark = self.selected_command?;
        let own = self.own_mut();
        if own.alt_screen() {
            return None;
        }
        let region = own.command_region(mark);
        if region.is_none() {
            self.selected_command = None;
        }
        region
    }

    /// `own_region` for what the pane shows: None in a Filter or Zoom view, whose rows are not
    /// the buffer's.
    pub fn selected_region(&mut self) -> Option<(usize, usize)> {
        if self.live.is_some() {
            return None;
        }
        self.own_region()
    }

    /// The rows Find, Filter and Select All work in (`updateSearchRange`, `PS:21819-21834`): the
    /// region, one row higher for a line-style mark. In a view the own screen's rows.
    pub fn search_region(&mut self) -> Option<(usize, usize)> {
        let (first, last) = self.own_region()?;
        let line_style = self.selected_command.and_then(|m| self.own().command_record(m)).is_some_and(|m| m.line_style);
        Some((if line_style { first.saturating_sub(1) } else { first }, last))
    }

    /// The pane's own screen, also while a view shows.
    pub fn own(&self) -> &Screen {
        self.live.as_ref().unwrap_or(&self.screen)
    }

    pub fn own_mut(&mut self) -> &mut Screen {
        self.live.as_mut().unwrap_or(&mut self.screen)
    }

    /// The Advanced emulation rules and, when given, the profile's Terminal Type, on the pane's
    /// own screen and any view.
    pub fn set_emu(&mut self, rules: crate::emu::Rules, term: Option<&str>) {
        for screen in self.live.iter_mut().chain(std::iter::once(&mut self.screen)) {
            screen.set_rules(rules);
            if let Some(t) = term {
                screen.set_term_name(t);
            }
        }
    }

    /// The profile's Character Encoding (iTerm2's NSStringEncoding). UTF-8, and a value the
    /// table lacks, take no decoder: bytes reach the screen unchanged. An unchanged code page
    /// keeps the decoder's held bytes.
    pub fn set_encoding(&mut self, ns_encoding: i64) {
        let cp = crate::encoding::code_page(ns_encoding).unwrap_or(0);
        if cp != self.encoding {
            self.encoding = cp;
            self.decoder = (cp != 0).then(|| crate::encoding::Decoder::new(cp));
            // The answerback is the one reply in the session's encoding.
            for screen in self.live.iter_mut().chain(std::iter::once(&mut self.screen)) {
                screen.set_answerback_encoding(cp);
            }
        }
    }

    /// The profile's Answerback String, on the pane's own screen and any view.
    pub fn set_answerback(&mut self, text: &str) {
        for screen in self.live.iter_mut().chain(std::iter::once(&mut self.screen)) {
            screen.set_answerback(text.to_string());
        }
    }

    /// The right-to-left display rules, on the pane's own screen and any view.
    pub fn set_bidi_rules(&mut self, rules: crate::bidi::Rules) {
        if let Some(live) = &mut self.live {
            live.bidi_rules = rules;
        }
        self.screen.bidi_rules = rules;
    }

    /// What a copy does with whitespace, on the pane's own screen and any view.
    pub fn set_copy_options(&mut self, options: crate::seltext::Options) {
        if let Some(live) = &mut self.live {
            live.copy_options = options;
        }
        self.screen.copy_options = options;
    }

    /// Shows a read-only view in place of the pane's screen.
    pub fn show_view(&mut self, mut view: Screen) {
        if self.frozen {
            return;
        }
        view.bidi_rules = self.screen.bidi_rules;
        view.copy_options = self.screen.copy_options;
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

/// Clear Buffer on one pane: clears it, then (`JiggleTTYSizeOnClearBuffer`, `PS:7077-7079`)
/// resizes the pty one column wider and back so the program redraws. A tmux pane has no pty
/// and nothing jiggles.
pub(crate) fn clear_then_jiggle(p: &mut Pane, jiggle: bool) {
    p.clear();
    if jiggle {
        p.jiggle();
    }
}

/// The bytes of a resize report: only while the mode is on, and never for a tmux pane (its size
/// is tmux's, and tmux answers for it). `px` is (height, width) of the text area.
pub fn resize_report_bytes(on: bool, tmux: bool, rows: u16, cols: u16, px: (u32, u32)) -> Option<Vec<u8>> {
    (on && !tmux).then(|| crate::emu::resize_report(rows, cols, px.0, px.1).into_bytes())
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
    fn every_pane_has_a_stable_id() {
        let (a, b) = (Pane::recording(1, 80, 24), Pane::tmux(2, 1, 0, 80, 24));
        assert!(crate::stable_id::is_valid(&a.stable_id) && crate::stable_id::is_valid(&b.stable_id));
        assert_ne!(a.stable_id, b.stable_id);
        assert_ne!(a.stable_id, a.session_guid, "the stable id and the per-launch reference are separate");
    }

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

    /// `PS:6586`: a pane is idle after its own idle time plus one second, so the frame gap
    /// follows the pane's `idle_time`, not a constant.
    #[test]
    fn replay_gap_reads_the_idle_time() {
        use crate::cadence::frame_gap;
        let prefs = crate::cadence::Prefs::default();
        let (fast, bg) = (frame_gap(true, false, false, 0), frame_gap(true, true, false, 0));
        let now = Instant::now() + Duration::from_secs(100);
        let last = Some(now - Duration::from_secs(5));
        let mut p = Pane::tmux(5, 1, 3, 20, 4);
        p.idle_time = Duration::from_secs(10);
        assert_eq!(p.replay_gap(now, true, last, &prefs, false), fast, "5 s quiet is not idle after 10 s");
        p.idle_time = Duration::from_secs(2);
        assert_eq!(p.replay_gap(now, true, last, &prefs, false), bg, "5 s quiet is idle after 2 s");
    }

    /// The cadence a pane records at (`UC:145-191`) and the recorder's clock: frames never
    /// go back in time, and a pane with no room for a frame copies nothing.
    #[test]
    fn recording_cadence_follows_iterm2() {
        use crate::cadence::frame_gap;
        let prefs = crate::cadence::Prefs::default();
        let (fast, bg) = (frame_gap(true, false, false, 500), frame_gap(false, false, false, 500));
        let us = |d: Duration| (d.as_secs_f64() * 1e6).round() as u64;
        assert_eq!((us(fast), us(bg)), (15_667, 999_000), "UC:239-243: 1/60 s and 1 s, a millisecond off each");
        let t0 = Instant::now();
        let mut p = Pane::tmux(5, 1, 3, 20, 4);
        assert_eq!(p.replay_gap(t0, true, None, &prefs, false), bg, "no output yet is idle");
        p.tmux_io().unwrap().push_output(b"hi");
        p.pump();
        let now = Instant::now();
        assert_eq!(p.replay_gap(now, true, None, &prefs, false), fast, "output since the last tick, a few bytes");
        assert_eq!(p.replay_gap(now, false, None, &prefs, false), bg, "not in a visible tab");
        p.got_output = false;
        assert_eq!(p.replay_gap(now, true, Some(now), &prefs, false), fast);
        assert_eq!(p.replay_gap(now + Duration::from_secs(4), true, Some(now), &prefs, false), bg, "idle 3 s");
        assert!(p.record_frame(t0, 5_000, fast));
        p.tmux_io().unwrap().push_output(b"!");
        p.pump();
        assert!(!p.record_frame(t0 + Duration::from_millis(500), 6_000, bg), "a background pane waits 1 s");
        assert!(p.record_frame(t0 + bg, 4_000, bg), "a clock set back");
        assert_eq!(p.dvr.timestamps(), vec![5_000, 5_001], "timestamps never go backward");
        // No room for a frame: nothing is captured.
        p.dvr = crate::dvr::Dvr::new(capacity_for(0));
        p.tmux_io().unwrap().push_output(b"?");
        p.pump();
        assert!(!p.record_frame(t0 + bg * 3, 9_000, fast));
        assert!(p.dvr.is_empty());
        assert_eq!(p.replay_at, Some(t0 + bg), "no capture happened");
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

    /// Character Encoding (13o Task 8): a session's output is decoded before the screen sees
    /// it, a character cut by a read decodes once, and UTF-8 keeps its bytes.
    #[test]
    fn output_is_decoded_with_the_session_encoding() {
        let mut p = Pane::recording(1, 20, 3);
        p.set_encoding(5);
        p.feed_output(b"caf\xe9 \x1b[1mx");
        assert!(p.screen.text().starts_with("caf\u{e9} x"), "{:?}", p.screen.text());
        let mut j = Pane::recording(2, 20, 3);
        j.set_encoding(8);
        j.feed_output(b"a\x82");
        j.feed_output(b"\xa0b");
        assert!(j.screen.text().starts_with("a\u{3042} b"), "{:?}", j.screen.text());
        let mut u = Pane::recording(3, 20, 3);
        u.feed_output("caf\u{e9}".as_bytes());
        assert!(u.screen.text().starts_with("caf\u{e9}"));
        assert!(u.decoder.is_none() && u.encoding == 0);
        // A value the table lacks, and UTF-8 (4), take no decoder.
        u.set_encoding(9999);
        assert!(u.decoder.is_none());
        u.set_encoding(4);
        assert!(u.decoder.is_none());
        // The same code page again keeps the held half character; a new one starts over.
        j.set_encoding(8);
        j.feed_output(b"\x82");
        j.set_encoding(8);
        j.feed_output(b"\xa0");
        assert!(j.screen.text().starts_with("a\u{3042} b\u{3042}"), "{:?}", j.screen.text());
        j.set_encoding(5);
        assert_eq!(j.encoding, 28591);
    }

    /// S4 (13o x 13p): Initial Text (its text, then Enter) goes through `input` and so leaves
    /// in the session's encoding like typing, in every non-UTF-8 choice; a UTF-8 session sends
    /// it as written.
    #[test]
    fn initial_text_leaves_in_the_session_encoding() {
        let text = |ns: i64| {
            let mut p = Pane::with(1, None, 10, 4);
            p.conductor = setting_up();
            p.set_encoding(ns);
            p.input("cd caf\u{e9}\r".as_bytes());
            p.take_ssh_held()
        };
        assert_eq!(text(5), b"cd caf\xe9\r");
        assert_eq!(text(4), "cd caf\u{e9}\r".as_bytes());
        // UTF-16: the text is paired, the Enter leaves as the one byte a typed Enter does
        // (13o's stream encoder passes C0 controls as they are).
        let mut utf16 = crate::encoding::utf16le_encode("cd caf\u{e9}");
        utf16.push(b'\r');
        assert_eq!(text(10), utf16);
    }

    /// Typed text and pastes leave in the session's encoding; a tmux pane and its gateway are
    /// always UTF-8 (control mode, `PTYSession.m:10958`).
    #[test]
    fn input_uses_the_session_encoding_but_tmux_is_utf8() {
        let mut p = Pane::tmux(5, 1, 3, 20, 4);
        p.set_encoding(5);
        p.input("caf\u{e9}\r".as_bytes());
        assert_eq!(p.tmux_io().unwrap().take_input(), "caf\u{e9}\r".as_bytes(), "a tmux pane sends UTF-8");
        p.paste("na\u{ef}ve");
        let sent = p.tmux_io().unwrap().take_input();
        assert!(sent.windows(6).any(|w| w == "na\u{ef}ve".as_bytes()), "{sent:?}");
        // A gateway's tmux commands are UTF-8 whatever the profile says.
        let mut g = Pane::with(7, None, 10, 4);
        g.set_encoding(5);
        g.gateway = true;
        assert_eq!(g.encode_text("send -lt %1 \u{e9}".as_bytes()).as_ref(), "send -lt %1 \u{e9}".as_bytes());
        // Any other pane encodes its text.
        let mut q = Pane::with(8, None, 10, 4);
        q.set_encoding(5);
        assert_eq!(q.encode_text("caf\u{e9}".as_bytes()).as_ref(), b"caf\xe9");
        let mut u = Pane::tmux(6, 1, 4, 20, 4);
        u.input("caf\u{e9}".as_bytes());
        assert_eq!(u.tmux_io().unwrap().take_input(), "caf\u{e9}".as_bytes());
    }

    /// Fix round 1 (Important 1): what the terminal answers and what Tandem itself writes
    /// (`Pane::reply`) leaves as the bytes it was given, in every encoding; only typed text,
    /// pastes and the answerback are encoded, once.
    #[test]
    fn replies_leave_raw_and_text_is_encoded_once() {
        let held = |p: &mut Pane| p.take_ssh_held();
        for ns in [5, 10] {
            let mut p = Pane::with(1, None, 10, 4);
            p.conductor = setting_up();
            p.set_encoding(ns);
            // A title report (UTF-8 text), a DA answer and a 1005 mouse report are raw.
            p.reply("\x1bP1;caf\u{e9}\x1b\\".as_bytes());
            p.reply(b"\x1b[?1;2c");
            p.reply(b"\x1b[M \xc4\x80!");
            let mut want = "\x1bP1;caf\u{e9}\x1b\\".as_bytes().to_vec();
            want.extend_from_slice(b"\x1b[?1;2c\x1b[M \xc4\x80!");
            assert_eq!(held(&mut p), want, "encoding {ns}");
        }
        // Typing: text is encoded, key escapes are not (also in UTF-16).
        let mut l = Pane::with(2, None, 10, 4);
        l.conductor = setting_up();
        l.set_encoding(5);
        l.live_input("caf\u{e9}\x1b[A".as_bytes());
        assert_eq!(held(&mut l), b"caf\xe9\x1b[A");
        let mut u = Pane::with(3, None, 10, 4);
        u.conductor = setting_up();
        u.set_encoding(10);
        u.live_input(b"\x1b[A");
        assert_eq!(held(&mut u), b"\x1b[A", "an arrow key is 3 bytes in UTF-16");
        // Text typed while SSH connects is encoded when typed and sent as it is queued (no second pass).
        let mut q = Pane::with(4, None, 10, 4);
        q.conductor = setting_up();
        q.set_encoding(5);
        q.reply("\u{e9}".as_bytes());
        assert_eq!(held(&mut q), "\u{e9}".as_bytes(), "reply does not encode");
        // The answerback is converted whole, once, where it is set and when the encoding changes.
        let mut a = Pane::with(5, None, 10, 4);
        a.conductor = setting_up();
        a.set_answerback("caf\u{e9}");
        a.set_encoding(5);
        let r = a.own_mut().feed(b"\x05");
        a.reply(&r);
        assert_eq!(held(&mut a), b"caf\xe9");
        a.set_encoding(10);
        let r = a.own_mut().feed(b"\x05");
        a.reply(&r);
        assert_eq!(held(&mut a), vec![b'c', 0, b'a', 0, b'f', 0, 0xe9, 0]);
        a.set_encoding(0);
        let r = a.own_mut().feed(b"\x05");
        a.reply(&r);
        assert_eq!(held(&mut a), "caf\u{e9}".as_bytes());
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

    /// `three_hops` with every conductor framed; the registry as the app keeps it after the
    /// pane applied what the conductors asked for.
    fn framed_three() -> (Pane, crate::ssh_registry::Registry, Vec<String>) {
        let mut p = three_hops();
        let mut guids = Vec::new();
        let mut c = p.conductor.as_deref_mut();
        while let Some(x) = c {
            x.setup.parsed.hostname = format!("{}.example", x.setup.client_unique_id);
            guids.insert(0, x.guid.clone());
            x.set_framed_pid(Some(10));
            c = x.parent.as_deref_mut();
        }
        let mut registry = crate::ssh_registry::Registry::default();
        p.apply_ssh_effects();
        registry.apply(p.take_ssh_registry());
        (p, registry, guids)
    }

    fn ask_files(p: &mut Pane, ids: &[u64]) {
        use crate::conductor::{Command, Handler};
        use crate::ssh_files::FileSub;
        let c = p.conductor.as_deref_mut().unwrap();
        for id in ids {
            c.send(Command::FramerFile(FileSub::Stat { path: "/tmp".into() }), Handler::File { id: *id, lines: Vec::new() }, false);
        }
        p.apply_ssh_effects();
        assert!(p.take_ssh_files().is_empty(), "nothing answered yet");
    }

    #[test]
    fn a_popped_conductor_answers_its_file_requests() {
        // P5: a written one and a queued one; the pop tells both -1, once.
        let (mut p, _r, _g) = framed_three();
        ask_files(&mut p, &[1, 2]);
        assert!(p.pop_conductor());
        assert_eq!(p.take_ssh_files(), [(1, String::new(), -1), (2, String::new(), -1)]);
        assert!(p.take_ssh_files().is_empty());
    }

    #[test]
    fn a_failed_conductor_answers_its_file_requests() {
        let (mut p, _r, _g) = framed_three();
        ask_files(&mut p, &[3, 4]);
        p.conductor.as_deref_mut().unwrap().fail("boom");
        p.apply_ssh_effects();
        assert_eq!(p.take_ssh_files(), [(3, String::new(), -1), (4, String::new(), -1)], "each once, though the abort pops it");
        assert!(!p.ssh_effects_pending());
    }

    #[test]
    fn every_conductor_leaves_the_registry_with_its_pane_state() {
        // Registered comes with the conductor that framed, even from a parent's queue.
        let (mut p, mut r, guids) = framed_three();
        assert_eq!(r.identities().iter().map(|i| i.hostname.as_str()).collect::<Vec<_>>(), ["a.example", "b.example", "c.example"]);
        assert_eq!(p.ssh_guids(), [guids[2].clone(), guids[1].clone(), guids[0].clone()], "innermost first");

        // EndSSH for the middle hop: B and C leave.
        p.end_ssh("b");
        r.apply(p.take_ssh_registry());
        assert_eq!(r.identities().len(), 1);
        p.end_ssh("a");
        r.apply(p.take_ssh_registry());
        assert!(!r.any(), "the last framed conductor is gone");

        // Abort: the failed conductor's Deregistered and its pop; the parents stay.
        let (mut p, mut r, _) = framed_three();
        p.conductor.as_mut().unwrap().fail("boom");
        p.apply_ssh_effects();
        r.apply(p.take_ssh_registry());
        assert_eq!(r.identities().iter().map(|i| i.hostname.as_str()).collect::<Vec<_>>(), ["a.example", "b.example"]);

        // Unhook and a plain pop.
        p.ssh_unhook_conductor();
        r.apply(p.take_ssh_registry());
        assert_eq!(r.identities().len(), 1);
        assert!(p.pop_conductor());
        r.apply(p.take_ssh_registry());
        assert!(!r.any());
        assert!(p.take_ssh_registry().is_empty());
    }

    #[test]
    fn an_abort_keeps_the_registry_changes_of_the_same_pass() {
        // Every conductor framed in one pass and the innermost fails in that pass: the
        // parents' Registered is applied, the failed one's is undone.
        let mut p = three_hops();
        let mut c = p.conductor.as_deref_mut();
        while let Some(x) = c {
            x.setup.parsed.hostname = format!("{}.example", x.setup.client_unique_id);
            x.set_framed_pid(Some(10));
            c = x.parent.as_deref_mut();
        }
        p.conductor.as_mut().unwrap().fail("boom");
        p.apply_ssh_effects();
        let mut r = crate::ssh_registry::Registry::default();
        r.apply(p.take_ssh_registry());
        assert_eq!(r.identities().iter().map(|i| i.hostname.as_str()).collect::<Vec<_>>(), ["a.example", "b.example"]);
        // The outermost failing takes the whole chain: nothing stays registered.
        let mut p = three_hops();
        let mut c = p.conductor.as_deref_mut();
        while let Some(x) = c {
            x.setup.parsed.hostname = format!("{}.example", x.setup.client_unique_id);
            x.set_framed_pid(Some(10));
            c = x.parent.as_deref_mut();
        }
        let mut outer = p.conductor.as_deref_mut().unwrap();
        while outer.parent.is_some() {
            outer = outer.parent.as_deref_mut().unwrap();
        }
        outer.fail("boom");
        p.apply_ssh_effects();
        let mut r = crate::ssh_registry::Registry::default();
        r.apply(p.take_ssh_registry());
        assert!(!r.any(), "{:?}", r.identities());
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

    /// A pane whose root hook is hooked, in a framing session (conductor "o", depth 0) with a
    /// nested framing conductor "a" inside it (depth 1), both with their hops.
    fn nested_framing() -> Pane {
        use crate::conductor::{Conductor, Setup};
        use crate::ssh_session::Hop;
        let at = |id: &str| Setup { client_unique_id: id.into(), parsed: crate::ssh_args::ParsedArgs { hostname: format!("{id}.example"), port: 22, ..Default::default() }, ..Setup::default() };
        let mut p = Pane::tmux(5, 1, 3, 120, 10);
        p.ssh.begin(true);
        p.tmux_io().unwrap().push_output(b"\x1bP2000pnone o YQ== - Yg==\n");
        p.pump();
        p.take_ssh();
        let mut outer = Conductor::new(at("o"), None, "O");
        outer.framed_pid = Some(100);
        p.ssh.chain.push(Hop { unique_id: "o".into(), location: "o.example".into() });
        let mut inner = Conductor::new(at("a"), Some(Box::new(outer)), "U");
        inner.framed_pid = Some(200);
        assert_eq!(inner.depth(), 1);
        p.ssh.chain.push(Hop { unique_id: "a".into(), location: "a.example".into() });
        p.conductor = Some(Box::new(inner));
        p
    }

    fn still_parses(p: &mut Pane) -> bool {
        use crate::conductor_parser::Token;
        p.tmux_io().unwrap().push_output(&crate::ssh_fixtures::reply("9.1", &["x"], 0, 'f'));
        p.pump();
        p.take_ssh().iter().any(|o| matches!(o, crate::ssh_hook::Out::Ssh(Token::Begin(id), 0) if id == "9.1"))
    }

    #[test]
    fn a_nested_abort_keeps_the_parent_hooked() {
        // A depth 1 conductor fails: only its depth is let go, the framing parent goes on.
        let mut p = nested_framing();
        p.abort_ssh("boom");
        assert_eq!(p.conductor.as_ref().map(|c| c.setup.client_unique_id.as_str()), Some("o"));
        assert!(p.ssh_hook.active(), "the depth 0 parser is still hooked");
        assert!(still_parses(&mut p), "and still parses the parent's replies");
        // The outermost conductor failing lets the whole hook go, as before.
        p.abort_ssh("boom");
        assert!(p.conductor.is_none() && !p.ssh_hook.active());
    }

    #[test]
    fn an_abort_fails_the_conductor_that_aborted() {
        // The parent fails (a reply routed to it through the child): the parent and the child
        // go, and the depth 0 hook lets go with them.
        let mut p = nested_framing();
        p.conductor.as_mut().unwrap().via_parent(|outer| outer.fail("boom"));
        p.apply_ssh_effects();
        assert!(p.conductor.is_none(), "the failed parent is gone, and its child with it");
        assert!(p.ssh.chain.hops.is_empty());
        assert!(!p.ssh_hook.active(), "depth 0 let go");
        assert!(p.screen.all_text().contains("boom"));
        // The child fails: only the child goes, depth 0 still parses.
        let mut p = nested_framing();
        p.conductor.as_mut().unwrap().fail("boom");
        p.apply_ssh_effects();
        assert_eq!(p.conductor.as_ref().map(|c| c.setup.client_unique_id.as_str()), Some("o"));
        assert!(still_parses(&mut p));
    }

    /// Feeds `bytes` as the pane's output and returns the depths of the conductor tokens the
    /// hook found.
    fn ssh_depths(p: &mut Pane, bytes: &[u8]) -> Vec<i32> {
        p.tmux_io().unwrap().push_output(bytes);
        p.pump();
        p.take_ssh().iter().filter_map(|o| if let crate::ssh_hook::Out::Ssh(_, d) = o { Some(*d) } else { None }).collect()
    }

    fn nested_init() -> Vec<u8> {
        use crate::ssh_fixtures::{dcs_hook, output, pre_reply};
        output("n", 200, -1, 0, &[dcs_hook("", "u", "YQ==", "Yg=="), pre_reply("1.1", &["sh"])].concat())
    }

    #[test]
    fn a_conductor_takes_its_hooks_depth() {
        use crate::ssh_fixtures::{output, pre_reply};
        // Under the framing depth 0 conductor alone, a conductor fits at depth 1 only.
        let mut p = nested_framing();
        p.ssh_unhook_conductor();
        assert_eq!(ssh_depths(&mut p, &nested_init()), [1, 1, 1, 1]);
        assert!(!p.ssh_init_fits(2));
        assert!(p.ssh_hook.owned(1), "a refusal at a depth nobody owns lets nothing go");
        assert!(p.ssh_init_fits(1));
        // Depth 0 again (the root hooked anew): it is the whole hook that lets go.
        assert!(!p.ssh_init_fits(0));
        assert!(!p.ssh_hook.active() && !p.ssh_hook.owned(1));
        // Under the framing depth 1 conductor, a second depth 1 hook does not fit: it lets go,
        // and its process's replies are plain output after it; depth 0 still parses.
        let mut p = nested_framing();
        assert_eq!(ssh_depths(&mut p, &nested_init()), [1, 1, 1, 1]);
        assert!(!p.ssh_init_fits(1));
        assert!(!p.ssh_hook.owned(1) && p.ssh_hook.owned(0));
        assert_eq!(ssh_depths(&mut p, &output("m", 200, -1, 0, &pre_reply("1.2", &["x"]))), Vec::<i32>::new());
        assert!(still_parses(&mut p));
    }

    #[test]
    fn an_unhook_takes_the_deeper_conductors_first() {
        let name = |p: &Pane| p.conductor.as_ref().map(|c| (c.setup.client_unique_id.clone(), c.unhooked()));
        // Beyond iTerm2: an unhook at depth 0 while a depth 1 conductor frames pops that one
        // (its keys must never go into a host that is no longer there), then unhooks depth 0.
        let mut p = nested_framing();
        let inner = p.conductor.as_ref().unwrap().guid.clone();
        p.unhook_conductor_at(0);
        assert_eq!(name(&p), Some(("o".into(), true)));
        assert_eq!(p.ssh.chain.hops.len(), 1, "the hop of the popped conductor went with it");
        assert!(p.take_ssh_registry().contains(&crate::ssh_registry::Change::Remove(inner)));
        assert!(p.ssh_chain_in_step());
        // Typing now goes straight to the pty, never framed into a gone remote.
        assert_eq!(p.ssh_route(), crate::conductor::Route::Direct);
        // An unhook at the innermost depth takes that conductor only.
        let mut p = nested_framing();
        p.unhook_conductor_at(1);
        assert_eq!(name(&p), Some(("a".into(), true)));
        assert_eq!(p.ssh.chain.hops.len(), 2);
        // An unhook for a depth with no conductor (it is gone) does not unhook a healthy
        // shallower one.
        let mut p = nested_framing();
        p.ssh_unhook_conductor();
        p.unhook_conductor_at(1);
        assert_eq!(name(&p), Some(("o".into(), false)));
        assert!(p.conductor.as_ref().unwrap().handles_keystrokes());
    }

    #[test]
    fn a_nested_abort_lets_its_own_hook_go() {
        // The depth 1 conductor fails: the depth 1 hook lets go, the depth 0 one parses on.
        let mut p = nested_framing();
        assert_eq!(ssh_depths(&mut p, &nested_init()), [1, 1, 1, 1]);
        p.conductor.as_mut().unwrap().fail("boom");
        p.apply_ssh_effects();
        assert_eq!(p.conductor.as_ref().map(|c| c.depth()), Some(0));
        assert!(!p.ssh_hook.owned(1) && p.ssh_hook.owned(0));
        assert!(still_parses(&mut p));
        // The depth 0 conductor fails through its child: every hook lets go.
        let mut p = nested_framing();
        assert_eq!(ssh_depths(&mut p, &nested_init()), [1, 1, 1, 1]);
        p.conductor.as_mut().unwrap().via_parent(|outer| outer.fail("boom"));
        p.apply_ssh_effects();
        assert!(p.conductor.is_none() && !p.ssh_hook.owned(1) && !p.ssh_hook.active());
    }

    #[test]
    fn errors_go_to_the_conductor_at_their_depth() {
        let name = |p: &Pane| p.conductor.as_ref().map(|c| c.setup.client_unique_id.clone());
        // A depth 1 error fails the depth 1 conductor only.
        let mut p = nested_framing();
        p.ssh_error(1, "boom");
        assert_eq!(name(&p).as_deref(), Some("o"));
        assert!(still_parses(&mut p));
        // A depth 1 error with no conductor at depth 1 (it is gone) is dropped: the healthy
        // depth 0 conductor is not failed by it.
        p.ssh_error(1, "boom");
        assert_eq!(name(&p).as_deref(), Some("o"));
        assert!(still_parses(&mut p));
        // A depth 0 error ends every conductor at or below depth 0, innermost first.
        let mut p = nested_framing();
        p.ssh_error(0, "boom");
        assert!(p.conductor.is_none());
        assert!(p.ssh.chain.hops.is_empty());
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
        assert!(p.conductor.as_mut().unwrap().take_effects().is_empty());
        assert!(p.tmux_io().unwrap().take_input().is_empty());
        assert_eq!(p.take_ssh_held(), b"co", "sendKeys (PS:4639-4642) of a conductor setting up: held");
        // The conductor's own unframed sendKeys lands the same way.
        p.conductor.as_mut().unwrap().send_keys(b"co");
        assert_eq!(p.conductor.as_ref().unwrap().effects, [Effect::Keys(b"co".to_vec())]);
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
    fn coprocess_raw_tee_follows_iterm2() {
        // PK:900: with SSH integration in play the raw read never feeds the coprocess.
        let mut p = Pane::tmux(5, 1, 3, 20, 4);
        assert!(p.coprocess_gets_raw(), "plain output");
        p.hook_output(b"hello");
        assert!(p.coprocess_gets_raw());
        p.hook_output(b"\x1bP2000pnone 1 YQ== - Yg==\n");
        assert!(!p.coprocess_gets_raw(), "a hook is active");
        p.take_ssh();
        assert!(!p.coprocess_gets_raw(), "still hooked");
        p.ssh_hook.unhook();
        assert!(p.coprocess_gets_raw());
        p.conductor = setting_up();
        assert!(!p.coprocess_gets_raw(), "a conductor (level 1 included) has no raw tee");
    }

    #[test]
    fn conductor_route_reaches_send_keys() {
        use crate::conductor::Effect;
        let mut p = Pane::tmux(5, 1, 3, 20, 4);
        p.conductor = setting_up();
        p.conductor.as_mut().unwrap().framed_pid = Some(1);
        p.write_session(b"q");
        // Framed: a send to the framed pid, written by the conductor (never raw keys).
        assert_eq!(p.conductor.as_mut().unwrap().take_effects(), [Effect::Write(b"c2VuZA==\nMQ==\nY1E9PQ==\n\n".to_vec())]);
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

    /// A framing conductor's process list gives the pane its job, close-prompt names and CPU
    /// series once rows have arrived; the values go when the conductor does.
    #[test]
    fn remote_processes_belong_to_the_framing_pane() {
        use crate::conductor::{Conductor, Setup};
        let mut p = Pane::with(1, None, 20, 6);
        assert!(p.remote_job().is_none() && p.remote_process_names().is_none() && p.remote_cpu().is_none());
        let mut c = Conductor::new(Setup { client_unique_id: "A".into(), ..Setup::default() }, None, "P");
        c.framed_pid = Some(10);
        p.conductor = Some(Box::new(c));
        let t0 = std::time::Instant::now();
        // The first tick makes the process list; nothing is known until rows come.
        assert!(!p.tick_procs(t0));
        assert!(p.remote_job().is_none() && p.remote_process_names().is_none(), "no rows: the local values stay");
        assert_eq!(p.procs_due(), Some(t0 + std::time::Duration::from_secs(1)));
        let procs = p.conductor.as_mut().unwrap().processes().unwrap();
        procs.handle("$begin ps\n+10 1 Ss Mon Sep 27 10:00:00 2026 -bash\n+11 10 S+ Mon Sep 27 10:00:01 2026 vim x\n$begin cpu\n=50\n");
        let (job, ancestors) = p.remote_job().unwrap();
        assert_eq!((job.unwrap().name, ancestors), ("vim".to_string(), vec!["vim".to_string()]));
        assert_eq!(p.remote_process_names(), Some(vec!["vim".to_string()]));
        // Once a second, and only when a value is known.
        assert!(!p.tick_procs(t0 + std::time::Duration::from_millis(500)));
        assert!(p.tick_procs(t0 + std::time::Duration::from_secs(1)));
        assert_eq!(p.remote_cpu().unwrap().values(), [0.5]);
        // The conductor going marks the job for a fresh read from the local watcher.
        p.take_jobs_dirty();
        assert!(p.pop_conductor());
        assert!(p.take_jobs_dirty());
        assert!(p.remote_job().is_none() && p.remote_cpu().is_none());
        assert!(!p.tick_procs(t0 + std::time::Duration::from_secs(5)));
        assert_eq!(p.procs_due(), None);
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
    fn cwd_tracking_follows_iterm2() {
        use crate::conductor::{Conductor, Setup};
        let mut p = Pane::with(1, None, 80, 24);
        p.conductor = Some(Box::new(Conductor::new(Setup::default(), None, "P")));
        p.feed_output(b"\x1b]7;file://localhost/local/dir\x07");
        p.note_ssh_cwd();
        assert!(!p.track_ssh_cwd(), "the directory the pane had when the conductor hooked is not the remote's");
        assert_eq!(p.conductor.as_ref().unwrap().current_directory, None);
        p.feed_output(b"\x1b]7;file://h/home/u/x\x07");
        assert!(p.track_ssh_cwd());
        assert_eq!(p.conductor.as_ref().unwrap().current_directory.as_deref(), Some("/home/u/x"));
        assert!(!p.track_ssh_cwd(), "once per change");
        // A directory that is not one line of text never reaches the conductor.
        p.feed_output(b"\x1b]7;file://h/a%0Ab\x07");
        assert!(p.track_ssh_cwd());
        assert_eq!(p.conductor.as_ref().unwrap().current_directory.as_deref(), Some("/home/u/x"));
        // With no conductor there is nothing to tell.
        p.conductor = None;
        p.feed_output(b"\x1b]7;file://h/other\x07");
        assert!(!p.track_ssh_cwd());
    }

    /// 13o Task 6 (R8): the resize report is written only while DEC mode 2048 is on, never for tmux.
    #[test]
    fn resize_reports_are_written_only_when_on() {
        assert_eq!(resize_report_bytes(true, false, 24, 80, (400, 640)), Some(b"\x1b[48;24;80;400;640t".to_vec()));
        assert_eq!(resize_report_bytes(false, false, 24, 80, (400, 640)), None);
        assert_eq!(resize_report_bytes(true, true, 24, 80, (400, 640)), None);
        let mut p = Pane::with(1, None, 20, 4);
        assert_eq!(p.resize_report_if_on(4, 20, (64, 160)), None);
        p.screen.feed(b"\x1b[?2048h");
        assert_eq!(p.resize_report_if_on(4, 20, (64, 160)), Some(b"\x1b[48;4;20;64;160t".to_vec()));
    }

    /// 13o final review (Task 6): a pane squeezed below 2x1 reports the grid it really has.
    #[test]
    fn a_squeezed_pane_reports_its_clamped_grid() {
        let p = Pane::with(1, None, 20, 4);
        assert_eq!(p.clamped_size(0, 0), (2, 1));
        assert_eq!(p.clamped_size(1, 0), (2, 1));
        assert_eq!(p.clamped_size(80, 24), (80, 24));
        let (cols, rows) = p.clamped_size(0, 0);
        assert_eq!(resize_report_bytes(true, false, rows, cols, (16, 32)), Some(b"\x1b[48;1;2;16;32t".to_vec()));
    }

    /// 13o Task 6 fix: the report follows the session's own screen while a view or a freeze is up.
    #[test]
    fn resize_reports_follow_the_own_screen_during_a_view() {
        let mut p = Pane::with(1, None, 20, 4);
        p.screen.feed(b"\x1b[?2048h");
        p.show_view(Screen::new(20, 4));
        assert_eq!(p.resize_report_if_on(4, 20, (64, 160)), Some(b"\x1b[48;4;20;64;160t".to_vec()), "view");
        p.freeze(Screen::new(20, 4));
        assert_eq!(p.resize_report_if_on(4, 20, (64, 160)), Some(b"\x1b[48;4;20;64;160t".to_vec()), "freeze");
        p.thaw();
        let mut p = Pane::with(1, None, 20, 4);
        p.show_view(Screen::new(20, 4));
        p.screen.feed(b"\x1b[?2048h");
        assert_eq!(p.resize_report_if_on(4, 20, (64, 160)), None, "the view's own mode is not the program's");
    }

    /// An echoed mouse report sets `stuck_seen` once per arming; output before any report does not.
    #[test]
    fn echoed_mouse_reports_set_stuck_seen_once() {
        let mut p = Pane::with(1, None, 20, 4);
        p.route_output(b"0;12;5M0;12;5m".to_vec());
        assert_eq!(p.stuck_seen, None, "nothing was armed");
        let mut d = crate::stuck_mouse::Detector::default();
        let now = std::time::Instant::now();
        d.armed(b"\x1b[<0;12;5M", now);
        d.armed(b"\x1b[<0;12;5m", now);
        p.stuck = Some(d);
        p.route_output(b"$ 0;12;5M0;12;5m".to_vec());
        assert_eq!(p.stuck_seen, Some(crate::stuck_mouse::Turd::Mouse));
        p.stuck_seen = None;
        p.route_output(b"$ 0;12;5M0;12;5m".to_vec());
        assert_eq!(p.stuck_seen, None, "one occurrence, one notice");
    }

    /// 13l Task 4 fix: output a tmux or ssh pump feeds counts as output for the pacer, as a
    /// pty read does (the Wake handler routes by `got_output`).
    #[test]
    fn pump_fed_output_counts_as_got_output() {
        let mut p = Pane::with(1, None, 80, 24);
        assert!(!p.got_output);
        p.feed_output(b"hi");
        assert!(p.got_output, "feed_output");
        p.got_output = false;
        p.feed_after_ssh(b"hi");
        assert!(p.got_output, "feed_after_ssh");
    }

    #[test]
    fn gateway_panes_take_no_input() {
        let mut p = Pane { id: 1, needs_forced_layout: true, activity: 0, locked: false, selected_command: None, pinned_width: None, session_guid: String::new(), stable_id: String::new(), screen: Screen::new(10, 4), encoding: 0, decoder: None, pty: None, exited: None, error: None, profile: String::new(), command: String::new(), live: None, session_note: String::new(), buffer: InputBuffer::default(), frozen: false, suppress_output: false, got_output: false, coprocess: None, co_out: Default::default(), co_end: None, tmux_hook: Default::default(), tmux_q: Vec::new(), ssh_hook: Default::default(), ssh_q: Vec::new(), conductor: None, ssh_registry_out: Vec::new(), ssh_answers: Vec::new(), ssh_files_out: Vec::new(), ssh_search_out: Vec::new(), ssh_clock: Default::default(), ssh_held: Vec::new(), pending_jumps: Vec::new(), gateway: false, tmux_size: None, screen_filter: None, last_newline: None, composer: composer::Composer::default(), dvr: dvr::Dvr::new(capacity_for(4)), replay_at: None, throughput: Default::default(), held: None, held_until: None, replay: None, echo_probe: None, stuck: None, stuck_seen: None, initial_text_due: false, initial_text_sent: false, idle_time: crate::activity::DEFAULT_IDLE, progress_anim: Default::default(), #[cfg(test)] resizes_for_test: Vec::new(), no_focus_reports: 0, password_input: false, cookie: None, inline_paused: false, frustration: Default::default(), wheel: crate::wheel::PaneWheel::new(0.1), arrangement: None, ssh: Default::default(), motion: Default::default(), ended: false, created: std::time::SystemTime::UNIX_EPOCH, created_at: Instant::now(), auto_log_id: 0, hold_open: false, survivors: Vec::new(), exit_seen: None, exit_pid: None, start_dir: None, log: None, log_started: None, log_compact: (false, false), archived: false, shell_gen: 0 };
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
        let mut p = Pane { id: 1, needs_forced_layout: true, activity: 0, locked: false, selected_command: None, pinned_width: None, session_guid: String::new(), stable_id: String::new(), screen: Screen::new(10, 4), encoding: 0, decoder: None, pty: None, exited: None, error: None, profile: String::new(), command: String::new(), live: None, session_note: String::new(), buffer: InputBuffer::default(), frozen: false, suppress_output: false, got_output: false, coprocess: None, co_out: Default::default(), co_end: None, tmux_hook: Default::default(), tmux_q: Vec::new(), ssh_hook: Default::default(), ssh_q: Vec::new(), conductor: None, ssh_registry_out: Vec::new(), ssh_answers: Vec::new(), ssh_files_out: Vec::new(), ssh_search_out: Vec::new(), ssh_clock: Default::default(), ssh_held: Vec::new(), pending_jumps: Vec::new(), gateway: false, tmux_size: None, screen_filter: None, last_newline: None, composer: composer::Composer::default(), dvr: dvr::Dvr::new(capacity_for(4)), replay_at: None, throughput: Default::default(), held: None, held_until: None, replay: None, echo_probe: None, stuck: None, stuck_seen: None, initial_text_due: false, initial_text_sent: false, idle_time: crate::activity::DEFAULT_IDLE, progress_anim: Default::default(), #[cfg(test)] resizes_for_test: Vec::new(), no_focus_reports: 0, password_input: false, cookie: None, inline_paused: false, frustration: Default::default(), wheel: crate::wheel::PaneWheel::new(0.1), arrangement: None, ssh: Default::default(), motion: Default::default(), ended: false, created: std::time::SystemTime::UNIX_EPOCH, created_at: Instant::now(), auto_log_id: 0, hold_open: false, survivors: Vec::new(), exit_seen: None, exit_pid: None, start_dir: None, log: None, log_started: None, log_compact: (false, false), archived: false, shell_gen: 0 };
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
        let mut p = Pane { id: 1, needs_forced_layout: true, activity: 0, locked: false, selected_command: None, pinned_width: None, session_guid: String::new(), stable_id: String::new(), screen: own, encoding: 0, decoder: None, pty: None, exited: None, error: None, profile: String::new(), command: String::new(), live: None, session_note: String::new(), buffer: InputBuffer::default(), frozen: false, suppress_output: false, got_output: false, coprocess: None, co_out: Default::default(), co_end: None, tmux_hook: Default::default(), tmux_q: Vec::new(), ssh_hook: Default::default(), ssh_q: Vec::new(), conductor: None, ssh_registry_out: Vec::new(), ssh_answers: Vec::new(), ssh_files_out: Vec::new(), ssh_search_out: Vec::new(), ssh_clock: Default::default(), ssh_held: Vec::new(), pending_jumps: Vec::new(), gateway: false, tmux_size: None, screen_filter: None, last_newline: None, composer: composer::Composer::default(), dvr: dvr::Dvr::new(capacity_for(4)), replay_at: None, throughput: Default::default(), held: None, held_until: None, replay: None, echo_probe: None, stuck: None, stuck_seen: None, initial_text_due: false, initial_text_sent: false, idle_time: crate::activity::DEFAULT_IDLE, progress_anim: Default::default(), #[cfg(test)] resizes_for_test: Vec::new(), no_focus_reports: 0, password_input: false, cookie: None, inline_paused: false, frustration: Default::default(), wheel: crate::wheel::PaneWheel::new(0.1), arrangement: None, ssh: Default::default(), motion: Default::default(), ended: false, created: std::time::SystemTime::UNIX_EPOCH, created_at: Instant::now(), auto_log_id: 0, hold_open: false, survivors: Vec::new(), exit_seen: None, exit_pid: None, start_dir: None, log: None, log_started: None, log_compact: (false, false), archived: false, shell_gen: 0 };
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

    /// `PS:7077-7079`: with `JiggleTTYSizeOnClearBuffer` on, Clear Buffer briefly widens the
    /// pty and restores it so the program redraws; off, the pty is not resized.
    #[test]
    fn clear_buffer_jiggles_when_asked() {
        let mut p = start("cmd.exe");
        clear_then_jiggle(&mut p, false);
        assert!(p.resizes_for_test.is_empty(), "off: no resize");
        clear_then_jiggle(&mut p, true);
        assert_eq!(p.resizes_for_test, vec![(81, 24), (80, 24)]);
    }

    #[test]
    fn ended_pane_keeps_its_screen() {
        let mut p = start("cmd.exe /c echo tandem-end");
        assert!(pump_until(&mut p, |p| p.exited.is_some()), "no exit");
        // The rest of the output may still be on its way.
        assert!(pump_until(&mut p, |p| p.screen.all_text().contains("tandem-end")), "{:?}", p.screen.all_text());
        assert_eq!(p.exited, Some(0));
        p.own_mut().append_end_message("Session Ended", "");
        let text = p.screen.all_text();
        assert!(text.contains("tandem-end"), "{text:?}");
        assert!(text.contains(" Session Ended "), "{text:?}");
    }

    #[test]
    fn relaunch_keeps_the_screen() {
        let mut a = start("cmd.exe /c echo first-run");
        assert!(pump_until(&mut a, |p| p.exited.is_some()), "no exit");
        assert!(pump_until(&mut a, |p| p.screen.all_text().contains("first-run")), "{:?}", a.screen.all_text());
        a.ended = true;
        a.own_mut().feed(b"\x1b[?1000h");
        assert_ne!(a.screen.mouse_modes().tracking, crate::mouse::Tracking::Off);
        let b = start("cmd.exe /c echo second-run");
        let mut a = Pane::relaunch(a, b);
        assert_eq!(a.exited, None);
        assert!(!a.ended);
        assert!(a.screen.pty_backed);
        assert_eq!(a.screen.mouse_modes().tracking, crate::mouse::Tracking::Off, "a restart resets the modes");
        assert!(pump_until(&mut a, |p| p.exited.is_some() && p.screen.all_text().contains("second-run")), "{:?}", a.screen.all_text());
        let text = a.screen.all_text();
        assert!(text.contains("first-run") && text.contains("second-run"), "{text:?}");
    }

    /// The shell generation moves with every exit and every restart (a question asked about one
    /// shell is not answered for another).
    #[test]
    fn the_shell_generation_moves_with_exit_and_restart() {
        let mut a = start("cmd.exe /c echo gen");
        let first = a.shell_gen;
        assert!(pump_until(&mut a, |p| p.exited.is_some()), "no exit");
        let ended = a.shell_gen;
        assert!(ended > first, "an exit is a new state of the pane");
        let b = start("cmd.exe /q /k");
        let mut c = Pane::relaunch(a, b);
        assert!(c.shell_gen > ended, "a restart is a new shell");
        let live = c.shell_gen;
        c.end_live_shell();
        assert!(c.shell_gen > live, "ending a live shell is an exit");
    }

    /// Item 17: an ended pane's screen still resizes and clears, but nothing reaches its dead
    /// console (B8's ruling counts resizes).
    #[test]
    fn an_ended_pane_resizes_its_screen_not_its_console() {
        let mut p = start("cmd.exe /c exit 0");
        assert!(pump_until(&mut p, |p| p.exited.is_some()), "no exit");
        let before = touches();
        assert!(p.resize(40, 9), "the size changed");
        assert_eq!((p.own().term().columns(), p.own().term().screen_lines()), (40, 9), "the screen followed");
        p.clear_to_last_mark();
        assert_eq!(touches(), before, "no resize or clear reaches a dead console");
        assert!(p.error.is_none(), "{:?}", p.error);
    }

    /// Item 22: a write that a dead pane refuses leaves the composer's probe alone.
    #[test]
    fn a_refused_write_keeps_the_composer_probe() {
        let mut p = start("cmd.exe /c exit 0");
        assert!(pump_until(&mut p, |p| p.exited.is_some()), "no exit");
        p.composer.probe = crate::prompt_state::ClearProbe::start(b"\x08", Instant::now());
        assert!(p.composer.probe.is_some());
        p.write_no_broadcast(b"x");
        assert!(p.composer.probe.is_some(), "the guard comes before the probe is cleared");
    }

    /// Every field of a Pane is classified here: a new field fails to compile until it is named,
    /// and the test says which pane it comes from after a relaunch.
    #[test]
    fn relaunch_takes_every_shell_field_from_the_fresh_pane() {
        fn classify(p: &Pane) {
            let Pane { id: _, needs_forced_layout: _, activity: _, locked: _, pinned_width: _, session_guid: _, stable_id: _, screen: _, encoding: _, decoder: _, pty: _, exited: _, error: _, profile: _, command: _, live: _, session_note: _, buffer: _, frozen: _, suppress_output: _, got_output: _, coprocess: _, co_out: _, co_end: _, tmux_hook: _, tmux_q: _, ssh_hook: _, ssh_q: _, conductor: _, ssh_registry_out: _, ssh_answers: _, ssh_files_out: _, ssh_search_out: _, ssh_clock: _, ssh_held: _, pending_jumps: _, screen_filter: _, gateway: _, tmux_size: _, last_newline: _, composer: _, dvr: _, replay_at: _, throughput: _, held: _, held_until: _, replay: _, echo_probe: _, stuck: _, stuck_seen: _, no_focus_reports: _, password_input: _, cookie: _, inline_paused: _, frustration: _, wheel: _, arrangement: _, ssh: _, motion: _, ended: _, created: _, created_at: _, auto_log_id: _, hold_open: _, survivors: _, exit_seen: _, exit_pid: _,
                // 13p, all from the fresh pane (the merge's classification): Initial Text's state (a
                // restarted SSH shell types once after its own login, S2), the idle time (read
                // again by apply_session_start, S4), the progress bar's animation (a restart
                // clears the bar, S8), the test-only resize log, and the start folder (the fresh
                // shell was started in the old one's, P13).
                initial_text_due: _, initial_text_sent: _, idle_time: _, progress_anim: _, resizes_for_test: _, start_dir: _,
                // 13q Task 7: a restart starts its own log (the old one stopped at the exit).
                log: _, log_started: _, log_compact: _,
                // 13q Task 8: a restart makes the archived pane live, so the fresh pane's flag.
                archived: _,
                // 13r Task 8: a restart selects no command (the fresh pane's None).
                selected_command: _,
                // 13q final fix 2: the new shell's generation is set from the old pane's.
                shell_gen: _ } = p;
        }
        let mut old = Pane::recording(1, 10, 3);
        classify(&old);
        old.own_mut().feed(b"kept text");
        old.locked = true;
        old.activity = 7;
        old.pinned_width = Some(120.0);
        old.session_guid = "OLD-GUID".into();
        old.stable_id = "old-stable".into();
        old.session_note = "note".into();
        old.arrangement = Some(("arr".into(), "sess".into()));
        old.created = std::time::SystemTime::UNIX_EPOCH;
        let old_created_at = Instant::now().checked_sub(Duration::from_secs(5)).unwrap();
        old.created_at = old_created_at;
        old.auto_log_id = 99;
        old.composer.probe = crate::prompt_state::ClearProbe::start(b"\x08", Instant::now());
        // The dead shell's state.
        old.exited = Some(3);
        old.ended = true;
        old.error = Some("boom".into());
        old.password_input = true;
        old.cookie = Some("old-cookie".into());
        old.ssh_held = b"queued".to_vec();
        old.pending_jumps = Vec::new();
        old.co_end = Some(("c".into(), "e".into()));
        old.encoding = 932;
        old.gateway = true;
        old.echo_probe = None;
        old.hold_open = true;
        // 13p's per-shell state of the dead shell.
        (old.initial_text_due, old.initial_text_sent) = (true, true);
        old.idle_time = std::time::Duration::from_secs(99);
        old.start_dir = Some("C:\\old".into());
        old.own_mut().feed(b"\x1b]9;4;1;40\x07");
        assert!(old.screen.progress.visible(), "the dead program left a bar");
        let mut fresh = Pane::recording(1, 10, 3);
        fresh.cookie = Some("new-cookie".into());
        fresh.session_guid = "NEW-GUID".into();
        fresh.start_dir = Some("C:\\fresh".into());
        let new = Pane::relaunch(old, fresh);
        assert!(!new.initial_text_due && !new.initial_text_sent, "a restarted shell gets its own Initial Text once (P14, S2)");
        assert_eq!(new.idle_time, crate::activity::DEFAULT_IDLE, "the idle time is read again for the new session (S4)");
        assert_eq!(new.start_dir.as_deref(), Some(std::path::Path::new("C:\\fresh")), "the fresh shell carries the folder it started in");
        assert!(!new.screen.progress.visible(), "a restart clears the dead program's progress bar (S8)");
        // From the old pane.
        assert!(new.screen.all_text().contains("kept text"));
        assert!(new.locked);
        assert_eq!(new.activity, 7);
        assert_eq!(new.pinned_width, Some(120.0));
        assert_eq!(new.session_guid, "OLD-GUID");
        assert_eq!(new.stable_id, "old-stable");
        assert_eq!(new.session_note, "note");
        assert_eq!(new.arrangement, Some(("arr".into(), "sess".into())));
        assert_eq!(new.created, std::time::SystemTime::UNIX_EPOCH, "a restart keeps the creation time (B6)");
        assert_eq!(new.created_at, old_created_at, "and the clock of the short-lived warning");
        assert_eq!(new.auto_log_id, 99);
        assert!(new.composer.probe.is_none());
        // From the fresh one.
        assert_eq!(new.exited, None);
        assert!(!new.ended);
        assert_eq!(new.error, None);
        assert!(!new.password_input);
        assert_eq!(new.cookie.as_deref(), Some("new-cookie"));
        assert!(new.ssh_held.is_empty());
        assert!(new.co_end.is_none());
        assert_eq!(new.encoding, 0);
        assert!(!new.gateway);
        assert!(!new.hold_open);
        assert!(new.screen.pty_backed);
    }

    fn touches() -> u32 {
        PTY_TOUCHES.with(|c| c.get())
    }

    #[test]
    fn archived_pane_has_no_shell() {
        let mut p = Pane::archived(1, 80, 24);
        assert_eq!(p.shell_pid(), None);
        assert_eq!(p.exited, Some(0));
        assert!(p.ended && p.archived);
        assert!(p.pty.is_none() && !p.screen.pty_backed);
        let before = p.screen.all_text();
        p.input(b"x");
        p.paste("y");
        assert!(!p.pump(), "nothing to read");
        assert_eq!(p.screen.all_text(), before, "input is ignored");
        assert!(p.log.is_none() && p.error.is_none());
        let live = Pane::recording(2, 80, 24);
        assert!(!live.archived && live.exited.is_none() && !live.ended, "only an archive starts ended");
    }

    #[test]
    fn ended_pane_takes_no_input() {
        let mut p = start("cmd.exe /c exit 0");
        assert!(pump_until(&mut p, |p| p.exited.is_some()), "no exit");
        // Buffer Input holds what is typed: an ended pane holds nothing.
        p.buffer.on = true;
        let before = touches();
        p.input(b"x");
        p.paste("y");
        p.reply(b"z");
        assert!(p.send_routed(b"w"));
        p.write_no_broadcast(b"v");
        p.write_unless_held(b"u");
        p.write_session(b"t");
        p.live_input(b"s");
        p.buffer.on = false;
        p.input(b"r");
        // The routes of a trigger's Send Text, a broadcast, the API's text, a report and the
        // composer all end in these.
        p.own_mut().feed(b"\x1b[?1000h\x1b[?1006h");
        p.reply(b"\x1b[<0;1;1M");
        p.send_routed(b"q");
        assert!(p.error.is_none(), "{:?}", p.error);
        assert!(p.ssh_held.is_empty(), "nothing queues for a dead console");
        assert!(p.buffer.release().is_empty());
        assert_eq!(touches(), before, "no byte reaches a dead console");
    }

    #[test]
    fn ended_pane_has_no_shell_pid() {
        let mut p = start("cmd.exe /c exit 0");
        assert!(p.shell_pid().is_some(), "a running shell has its pid");
        assert!(pump_until(&mut p, |p| p.exited.is_some()), "no exit");
        assert_eq!(p.shell_pid(), None, "Windows reuses pids");
        assert!(p.exit_pid().is_some(), "kept for the survivor survey only");
    }

    #[test]
    fn close_asks_for_survivors_of_an_ended_pane() {
        // `start /b` leaves ping running and attached to the console after cmd exits.
        let mut p = start("cmd.exe /c start /b ping -n 30 127.0.0.1");
        assert!(pump_until(&mut p, |p| p.exited.is_some()), "no exit");
        p.survey_survivors();
        let names = p.live_survivors();
        let pids: Vec<u32> = p.survivors.iter().map(|(pid, _)| *pid).collect();
        assert!(names.iter().any(|n| n.eq_ignore_ascii_case("ping")), "{names:?} {:?}", p.survivors);
        // Only what this test started is stopped.
        for pid in pids {
            let _ = std::process::Command::new("taskkill").args(["/PID", &pid.to_string(), "/F"]).output();
        }
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end && !p.live_survivors().is_empty() {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(p.live_survivors().is_empty(), "a stopped program is no survivor");
    }

    #[test]
    fn clear_buffer_skips_an_ended_pane() {
        let mut p = start("cmd.exe /c echo gone");
        assert!(pump_until(&mut p, |p| p.exited.is_some()), "no exit");
        let before = touches();
        p.clear();
        p.jiggle();
        assert!(p.error.is_none(), "{:?}", p.error);
        assert_eq!(touches(), before, "Clear Buffer's resize and clear never reach a dead console");
    }

    #[test]
    fn a_live_shell_ends_before_its_restart() {
        let mut p = start("cmd.exe /q /k");
        assert!(p.pty.is_some() && p.exited.is_none(), "a running shell");
        p.end_live_shell();
        assert!(p.exited.is_some(), "the pane is ended");
        assert!(p.ended, "its end is handled: nothing reports it again");
        assert!(p.pty.is_none(), "its console is gone");
    }

    #[test]
    fn survivors_are_taken_at_the_exit() {
        // A close or restart before the end is handled (the drain wait) still sees what the
        // shell left running; so does the pid kept for the job watcher.
        let mut p = start("cmd.exe /c start /b ping -n 30 127.0.0.1");
        assert!(pump_until(&mut p, |p| p.exited.is_some()), "no exit");
        assert!(!p.ended, "the end is not handled yet");
        let names = p.live_survivors();
        let pids: Vec<u32> = p.survivors.iter().map(|(pid, _)| *pid).collect();
        assert!(names.iter().any(|n| n.eq_ignore_ascii_case("ping")), "{names:?} {:?}", p.survivors);
        assert!(p.exit_pid().is_some());
        for pid in pids {
            let _ = std::process::Command::new("taskkill").args(["/PID", &pid.to_string(), "/F"]).output();
        }
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
    fn a_tmux_job_is_stored_without_hidden_characters() {
        let mut info = TmuxInfo::default();
        info.store(crate::tmux_controller::Info::Job, "vi\u{202e}m\u{7}\u{200b}".into());
        assert_eq!(info.job, "vim");
        info.store(crate::tmux_controller::Info::Job, "x".repeat(300));
        assert_eq!(info.job.chars().count(), 300, "a name is not cut here (a command line must not be)");
    }

    #[test]
    fn initial_text_is_due_after_the_hook_cleared_the_profile_flag() {
        let mut p = Pane::with(1, None, 80, 24);
        p.start_as_profile(true);
        p.ssh_hook_took_over();
        p.conductor_login_text();
        assert!(p.take_initial_text_due(), "the SSH profile's text is due at login, hook or not");
        assert!(!p.take_initial_text_due());
        let mut typed = Pane::with(2, None, 80, 24);
        typed.start_as_profile(false);
        typed.ssh_hook_took_over();
        typed.conductor_login_text();
        assert!(!typed.take_initial_text_due(), "a typed ssh never gets it");
    }

    #[test]
    fn initial_text_is_due_only_for_an_ssh_profile_after_login_and_once() {
        let mut p = Pane::with(1, None, 80, 24);
        // Not an SSH profile's session: a conductor never makes its text due.
        p.conductor_login_text();
        assert!(!p.take_initial_text_due());
        // An SSH profile's: nothing before the conductor logs in.
        p.start_as_profile(true);
        assert!(!p.take_initial_text_due(), "nothing before the login");
        p.conductor_login_text();
        p.conductor_login_text();
        assert!(p.take_initial_text_due(), "due after the login");
        assert!(!p.take_initial_text_due(), "taken once");
        p.conductor_login_text();
        assert!(!p.take_initial_text_due(), "a repeated effect never types it again");
    }

    #[cfg(windows)]
    #[test]
    fn injected_powershell_emits_marks() {
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/shell_integration/tandem_shell_integration.ps1");
        let cmd = crate::shell_inject::inject("powershell.exe -NoProfile -NoLogo", &script).expect("a PowerShell line is injected");
        assert!(!cmd.contains("Set-ExecutionPolicy"), "{cmd}");
        let mut p = start(&cmd);
        // The script is read and run before the first prompt; typing then runs one more.
        assert!(pump_until(&mut p, |p| p.screen.shell.shell.as_deref() == Some("powershell")), "{}", p.screen.all_text());
        p.input(b"prompt; exit\r");
        assert!(pump_until(&mut p, |p| p.exited.is_some()), "powershell did not exit");
        p.pump();
        assert!(!p.screen.marks_in_order().is_empty(), "{}", p.screen.all_text());
        assert_eq!(p.screen.shell.shell.as_deref(), Some("powershell"));
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
        let view = crate::synthetic::view(&[crate::synthetic::Styled { text: "x".into(), sgr: "x".into() }], p.own());
        p.show_view(view);
        assert!(p.paste_payload("a\nb").starts_with(b"\x1b[200~"));
        let mut red = crate::palette::Palette::default();
        red.bg = crate::palette::rgb(0x80, 0, 0);
        p.restyle(red.clone(), crate::screen::Look::default());
        p.end_view();
        assert_eq!(p.screen.palette.bg, red.bg);
    }
    #[test]
    fn selection_writes_keep_state() {
        use alacritty_terminal::index::{Column, Line, Point, Side};
        use alacritty_terminal::selection::{Selection, SelectionType};
        let mut p = start("cmd /c ping -n 3 127.0.0.1 >nul");
        p.screen.feed(b"one\r\ntwo");
        let sel = || Selection::new(SelectionType::Simple, Point::new(Line(0), Column(0)), Side::Left);
        p.set_selection(Some(sel()), crate::seltext::Mode::Word);
        assert_eq!(p.screen.sel.live_mode, crate::seltext::Mode::Word);
        p.screen.push_extra(crate::seltext::Sub { start: crate::find::Pos { row: 1, col: 0 }, end: crate::find::Pos { row: 1, col: 2 }, mode: crate::seltext::Mode::Character, window: None, connected: false });
        p.screen.sel.window = Some((1, 2));
        assert_eq!(p.screen.sel.extras.len(), 1);
        // A new selection starts the extras and the window over.
        p.set_selection(Some(sel()), crate::seltext::Mode::Character);
        assert!(p.screen.sel.extras.is_empty() && p.screen.sel.window.is_none());
        p.screen.push_extra(crate::seltext::Sub { start: crate::find::Pos { row: 1, col: 0 }, end: crate::find::Pos { row: 1, col: 2 }, mode: crate::seltext::Mode::Character, window: None, connected: false });
        p.clear_selection();
        assert!(p.screen.sel.extras.is_empty());
        // What alacritty does on output: the selection goes, and the extras with it.
        p.set_selection(Some(sel()), crate::seltext::Mode::Character);
        p.screen.push_extra(crate::seltext::Sub { start: crate::find::Pos { row: 1, col: 0 }, end: crate::find::Pos { row: 1, col: 2 }, mode: crate::seltext::Mode::Character, window: None, connected: false });
        p.screen.term_mut().selection = None;
        p.screen.sync_selection();
        assert!(p.screen.sel.extras.is_empty());
    }

    /// B1: the extras travel with the screen a Zoom or Filter view swaps out.
    #[test]
    fn extras_ride_with_the_screen() {
        use alacritty_terminal::index::{Column, Line, Point, Side};
        use alacritty_terminal::selection::{Selection, SelectionType};
        let mut p = start("cmd /c ping -n 3 127.0.0.1 >nul");
        p.screen.feed(b"one\r\ntwo");
        p.set_selection(Some(Selection::new(SelectionType::Simple, Point::new(Line(0), Column(0)), Side::Left)), crate::seltext::Mode::Character);
        p.screen.push_extra(crate::seltext::Sub { start: crate::find::Pos { row: 1, col: 0 }, end: crate::find::Pos { row: 1, col: 2 }, mode: crate::seltext::Mode::Character, window: None, connected: false });
        p.show_view(Screen::new(20, 4));
        assert!(p.screen.sel.extras.is_empty(), "the view shows none");
        p.end_view();
        assert_eq!(p.screen.sel.extras.len(), 1, "the own screen gets them back");
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

    fn framing_conductor(pid: i32) -> Box<crate::conductor::Conductor> {
        let mut c = crate::conductor::Conductor::new(Default::default(), None, "U");
        c.framed_pid = Some(pid);
        Box::new(c)
    }

    fn sends(p: &Pane) -> Vec<Vec<u8>> {
        use crate::conductor::Command;
        p.conductor.as_ref().unwrap().pending().into_iter().filter_map(|c| match c {
            Command::FramerSend { data, pid: 4242 } => Some(data.clone()),
            _ => None,
        }).collect()
    }

    #[test]
    fn framed_writes_go_through_send() {
        let mut p = Pane::with(7, None, 80, 24);
        p.conductor = Some(framing_conductor(4242));
        p.live_input(b"a");
        p.reply(b"\x1b[?1;2c");
        assert!(p.coprocess_output(b"co"));
        assert_eq!(sends(&p), [b"a".to_vec(), b"\x1b[?1;2c".to_vec(), b"co".to_vec()]);
        assert!(p.take_ssh_held().is_empty(), "nothing raw is held for the session");

        // A level 1 conductor (writes are not queued) sends nothing framed.
        let mut p = Pane::with(8, None, 80, 24);
        let mut c = crate::conductor::Conductor::new(Default::default(), None, "U");
        c.handle_unhook();
        p.conductor = Some(Box::new(c));
        p.live_input(b"a");
        p.reply(b"r");
        assert!(p.coprocess_output(b"co"));
        p.apply_ssh_effects();
        assert!(sends(&p).is_empty());
        assert!(p.take_ssh_held().is_empty(), "straight to the pty: no conductor above frames");
    }

    #[test]
    fn connecting_input_goes_through_send() {
        let mut p = Pane::with(7, None, 80, 24);
        p.conductor = setting_up();
        p.ssh.begin(true);
        p.live_input(b"ls\r");
        p.live_input(b"x");
        // The framer's login command in flight: still queued, nothing sent before its reply.
        login(&mut p);
        p.live_input(b"y");
        assert!(sends(&p).is_empty(), "queued while connecting, not sent before the login");
        assert_eq!(p.ssh.connecting.as_deref(), Some(&b"ls\rxy"[..]));
        // The login's reply (the shell's pid) frames the conductor and stops the queueing.
        login_reply(&mut p);
        assert_eq!(sends(&p), [b"ls\rxy".to_vec()], "one send for everything typed while connecting");
        assert!(p.ssh.connecting.is_none());
        // Nothing typed: nothing sent (F18).
        let mut p = Pane::with(7, None, 80, 24);
        p.conductor = setting_up();
        p.ssh.begin(true);
        login(&mut p);
        login_reply(&mut p);
        assert!(sends(&p).is_empty());
        assert!(p.ssh.connecting.is_none());
    }

    /// The framer's `login` command written (`CD:1926-1929`).
    fn login(p: &mut Pane) {
        use crate::conductor::{Command, Handler};
        let c = p.conductor.as_mut().unwrap();
        c.send(Command::FramerLogin { cwd: "$HOME".into(), args: Vec::new() }, Handler::FramerLogin(Vec::new()), false);
        c.take_effects();
    }

    /// framer.py's reply to `login`: the login shell's pid, 4242 (`CD:2398-2411`).
    fn login_reply(p: &mut Pane) {
        let c = p.conductor.as_mut().unwrap();
        c.handle_begin("l", 0);
        c.handle_line("4242", 0);
        c.handle_end("l", "f", 0, 0);
        assert_eq!(c.framed_pid, Some(4242));
        p.apply_ssh_effects();
    }

    #[test]
    fn typing_while_a_nested_hop_connects_reaches_the_outer_send() {
        // F9: iTerm2 asks handlesKeystrokes before the connecting queue (`PS:4418-4426`).
        let mut p = Pane::with(7, None, 80, 24);
        p.conductor = Some(framing_conductor(4242));
        p.ssh.begin(true);
        p.live_input(b"secret\r");
        assert_eq!(sends(&p), [b"secret\r".to_vec()]);
        assert_eq!(p.ssh.connecting.as_deref(), Some(&b""[..]), "nothing waits in the connecting queue");
    }

    #[test]
    fn a_coprocess_cannot_flood_the_queue() {
        // F20: a slow link must not grow the queue without end.
        let mut p = Pane::with(7, None, 80, 24);
        let mut c = framing_conductor(4242);
        c.send(crate::conductor::Command::FramerRun("x".into()), crate::conductor::Handler::FireAndForget, false);
        p.conductor = Some(c);
        assert!(!p.coprocess_backlogged());
        for _ in 0..80 {
            assert!(p.coprocess_output(&[b'z'; 1024]));
        }
        assert!(p.coprocess_backlogged(), "over 64 KiB waits");
        // Replies come back and the queue drains into the wire.
        let c = p.conductor.as_mut().unwrap();
        for i in 0..200 {
            c.handle_end("a", "f", 0, 0);
            if !c.has_command(&|x| matches!(x, crate::conductor::Command::FramerSend { .. })) {
                break;
            }
            assert!(i < 199);
        }
        assert!(!p.coprocess_backlogged());
    }

    // Routing under nested conductors (task 6 fix round, ruling: input never goes to the pty
    // while any ancestor conductor frames; the nearest framing ancestor sends it).

    /// A pane in a framing session: the root hook hooked, conductor "o" at depth 0 framing
    /// pid 100 in ground, with its hop.
    fn framing_root() -> Pane {
        let mut p = framed_nested();
        p.ssh_unhook_conductor();
        assert_eq!(p.conductor.as_ref().map(|c| (c.depth(), c.framing())), Some((0, true)));
        p
    }

    /// `nested_framing` once connecting ended (the login's StopQueueingInput).
    fn framed_nested() -> Pane {
        let mut p = nested_framing();
        p.ssh.stop_queueing();
        p
    }

    /// What the pty got once the conductors' effects are applied.
    fn wire(p: &mut Pane) -> Vec<u8> {
        p.apply_ssh_effects();
        p.tmux_io().unwrap().take_input()
    }

    /// `data` framed for pid `pid` at depth 0: a `send` framer command.
    fn sent_to(pid: i32, data: &[u8]) -> Vec<u8> {
        use base64::Engine;
        crate::ssh_fixtures::framer_cmd(&["send", &pid.to_string(), &base64::engine::general_purpose::STANDARD.encode(data)])
    }

    /// A conductor for `init` chained onto the pane's current one, as `App::ssh_hooked` does.
    fn hook_child(p: &mut Pane, init: &crate::conductor_parser::Init, depth: i32) {
        use crate::conductor::{setup_for_hook, Conductor};
        assert!(p.ssh_init_fits(depth), "a conductor fits at depth {depth}");
        let setup = setup_for_hook(init, false, &[], &[], None, false, &crate::ssh_args::NoConfig);
        let mut c = Conductor::new(setup, p.conductor.take(), "E");
        c.start();
        p.ssh.chain.push(crate::ssh_session::Hop { unique_id: c.setup.client_unique_id.clone(), location: "x".into() });
        p.conductor = Some(Box::new(c));
    }

    /// Feeds `bytes` as output and runs the hook's Init and Unhook tokens as `App::ssh_pump_pane`
    /// does.
    fn run_hook(p: &mut Pane, bytes: &[u8]) {
        use crate::conductor_parser::Token;
        use crate::ssh_hook::Out;
        p.tmux_io().unwrap().push_output(bytes);
        p.pump();
        for out in p.take_ssh() {
            match out {
                Out::Ssh(Token::Init(init), d) => hook_child(p, &init, d),
                Out::Ssh(Token::Unhook, d) => p.unhook_conductor_at(d),
                _ => {}
            }
        }
    }

    /// Every way into the session, each asserted framed for `pid` with nothing raw.
    fn every_path_is_framed(p: &mut Pane, pid: i32) {
        every_path_is_framed_by(p, &|data| sent_to(pid, data));
    }

    /// Every way into the session, each asserted to reach the wire as `framed(data)`.
    fn every_path_is_framed_by(p: &mut Pane, framed: &dyn Fn(&[u8]) -> Vec<u8>) {
        wire(p);
        p.live_input(b"k");
        assert_eq!(wire(p), framed(b"k"), "typing");
        p.input(b"\r");
        assert_eq!(wire(p), framed(b"\r"), "Enter");
        p.paste("a\nb");
        assert_eq!(wire(p), framed(b"a\rb"), "a paste");
        p.own_mut().feed(b"\x1b[?2004h");
        p.paste("c");
        assert_eq!(wire(p), framed(b"\x1b[200~c\x1b[201~"), "a bracketed paste");
        p.own_mut().feed(b"\x1b[?2004l");
        p.reply(b"\x1b[?1;2c");
        assert_eq!(wire(p), framed(b"\x1b[?1;2c"), "a report");
        p.focus_report(true);
        assert_eq!(wire(p), framed(b"\x1b[I"), "a focus report");
        assert!(p.coprocess_output(b"co"));
        assert_eq!(wire(p), framed(b"co"), "coprocess output");
        p.write_session(b"w");
        assert_eq!(wire(p), framed(b"w"), "a session write");
        p.set_buffering(true);
        p.live_input(b"b");
        assert_eq!(wire(p), framed(b"b"), "Buffer Input never holds a conductor's keys (PS:4417-4431)");
        p.set_buffering(false);
        assert!(p.take_ssh_held().is_empty(), "nothing held");
    }

    #[test]
    fn a_nested_bootstrap_is_framed() {
        // it2ssh typed in a framed shell: its bootstrap goes through the framer as
        // writeTaskNoBroadcast does (`PS:20298-20300`, `PS:4280-4285`), never into its stdin.
        let mut p = framing_root();
        wire(&mut p);
        let h = crate::ssh_handshake::Handshake { token: "a".into(), unique_id: "b".into(), encoded_ba: "c".into(), ssh_args: "d".into() };
        p.ssh_send_conductor(&h);
        let bytes = crate::ssh_session::conductor_bytes(&h, true).unwrap();
        assert_eq!(wire(&mut p), sent_to(100, &bytes));
        // With no conductor it is typed as before.
        let mut p = Pane::tmux(5, 1, 3, 120, 10);
        p.ssh_send_conductor(&h);
        assert_eq!(p.tmux_io().unwrap().take_input(), bytes);
    }

    #[test]
    fn a_level_1_hop_under_a_framer_is_framed_by_it() {
        use crate::ssh_fixtures::{dcs_hook, output, pre_reply};
        // A nested it2ssh to a host without Python 3.7: a depth 1 conductor that sets up, then
        // unhooks for good. Its ssh runs in the framed shell of pid 100.
        let mut p = framing_root();
        run_hook(&mut p, &output("n", 100, -1, 0, &dcs_hook("", "u", "YQ==", "Yg==")));
        assert_eq!(p.conductor.as_ref().map(|c| c.depth()), Some(1));
        let setup = wire(&mut p);
        assert!(setup.starts_with(&sent_to(100, b"\x03E\n")), "the child's setup is framed (F5)");
        // Typed while it sets up: held for its unhook.
        p.live_input(b"early");
        assert_eq!(wire(&mut p), b"");
        // conductor.sh answers and then unhooks (the login shell ran without the framer).
        run_hook(&mut p, &output("n", 100, -1, 0, &[pre_reply("1.1", &["sh"]), crate::ssh_fixtures::osc135("unhook")].concat()));
        assert!(p.conductor.as_ref().unwrap().unhooked() && p.conductor.as_ref().unwrap().depth() == 1);
        assert_eq!(wire(&mut p), sent_to(100, b"early"), "the unhook's held bytes are framed");
        every_path_is_framed(&mut p, 100);
    }

    #[test]
    fn a_nested_exit_before_endssh_is_framed_by_the_parent() {
        // The depth 1 framer ends (its unhook) and EndSSH has not come yet: whatever is typed
        // meanwhile is for the depth 0 framed shell that ran that ssh.
        let mut p = framed_nested();
        wire(&mut p);
        p.unhook_conductor_at(1);
        assert_eq!(p.conductor.as_ref().map(|c| (c.depth(), c.unhooked())), Some((1, true)));
        every_path_is_framed(&mut p, 100);
        // EndSSH pops it; the end line goes to the same framed shell.
        p.end_ssh("a");
        assert_eq!(wire(&mut p), sent_to(100, crate::ssh_session::END_WRITE));
    }

    #[test]
    fn a_catd_conductor_under_a_framer_is_framed_by_it() {
        use crate::ssh_fixtures::{dcs_hook, osc135, output};
        // `cat evil` in the framed shell: DCS 2000p, an init line and an unhook at depth 1.
        let mut p = framing_root();
        run_hook(&mut p, &output("n", 100, -1, 0, &[dcs_hook("", "evil", "YQ==", "Yg=="), osc135("unhook")].concat()));
        assert_eq!(p.conductor.as_ref().map(|c| (c.depth(), c.unhooked())), Some((1, true)));
        every_path_is_framed(&mut p, 100);
    }

    #[test]
    fn a_depth_2_hop_is_framed_at_depth_1() {
        use crate::ssh_fixtures::{dcs_hook, osc135, output};
        // The same at depth 2: the nearest framing ancestor (depth 1, pid 200) sends, and its
        // send is itself framed by depth 0.
        let mut p = framed_nested();
        assert_eq!(ssh_depths(&mut p, &nested_init()), [1, 1, 1, 1], "the depth 1 hook");
        wire(&mut p);
        let evil = output("k", 200, -1, 1, &[dcs_hook("", "evil", "YQ==", "Yg=="), osc135("unhook")].concat());
        // Through the process whose output holds the depth 1 hook (`nested_init`'s).
        run_hook(&mut p, &output("n", 200, -1, 0, &evil));
        assert_eq!(p.conductor.as_ref().map(|c| (c.depth(), c.unhooked())), Some((2, true)));
        wire(&mut p);
        p.live_input(b"k");
        let inner = sent_to(200, b"k");
        assert_eq!(wire(&mut p), sent_to(100, &inner));
    }

    #[test]
    fn a_failing_level_1_hop_writes_nothing_raw() {
        // An error at depth 1 fails the level 1 conductor: its `exec_login_shell` line is never
        // typed into the depth 0 framer.
        let mut p = framed_nested();
        wire(&mut p);
        p.unhook_conductor_at(1);
        p.ssh_error(1, "lost");
        assert_eq!(wire(&mut p), b"");
        assert_eq!(p.conductor.as_ref().map(|c| c.depth()), Some(0));
    }

    /// A framing conductor's clock effects: a tokenless reply is the pane's Clock, a script
    /// call's reply is an answer for the app, and both are read only while the pane is at
    /// level 2.
    #[test]
    fn time_offsets_reach_the_clock_and_the_script_tokens() {
        use crate::conductor::{Conductor, Effect, Setup};
        use crate::ssh_clock::{Offset, Refresh};
        let mut p = Pane::with(1, None, 20, 6);
        let t0 = std::time::Instant::now();
        // No conductor: local time, once.
        assert_eq!(p.ssh_clock_tick(t0, Some("h")), Refresh::Local);
        assert_eq!(p.ssh_clock_tick(t0, Some("h")), Refresh::Nothing);
        let mut c = Conductor::new(Setup { client_unique_id: "A".into(), ..Setup::default() }, None, "P");
        c.framed_pid = Some(10);
        p.conductor = Some(Box::new(c));
        // Level 2: a fetch is sent (through the framer, as a runpy command), once.
        assert_eq!(p.ssh_clock_tick(t0, Some("h")), Refresh::Fetch);
        assert!(matches!(p.conductor.as_ref().unwrap().in_flight(), Some(crate::conductor::Command::FramerExecPythonStatements(_))));
        assert_eq!(p.ssh_clock_tick(t0, Some("h")), Refresh::Nothing);
        assert_eq!(p.clock_due(), Some(t0 + crate::ssh_clock::REFRESH));
        let good = Offset { offset: 11.5, tz: "CDT".into(), zone: Some(-18000) };
        p.conductor.as_mut().unwrap().effects.push(Effect::TimeOffset { token: None, result: Ok(good.clone()) });
        p.conductor.as_mut().unwrap().effects.push(Effect::TimeOffset { token: Some(7), result: Ok(good.clone()) });
        p.conductor.as_mut().unwrap().effects.push(Effect::TimeOffset { token: Some(8), result: Err("Failed to fetch time: x".into()) });
        assert!(p.apply_ssh_effects(), "the bar shows a new time");
        assert_eq!((p.ssh_clock.offset, p.ssh_clock.tz.as_deref(), p.ssh_clock.zone), (11.5, Some("CDT"), Some(-18000)));
        assert_eq!(p.take_ssh_answers(), [(7, Ok(good)), (8, Err("Failed to fetch time: x".to_string()))]);
        assert!(p.take_ssh_answers().is_empty());
        // The conductor leaves: level 0 is local time again.
        assert!(p.pop_conductor());
        assert_eq!(p.ssh_clock_tick(t0, Some("h")), Refresh::Local);
        assert_eq!((p.ssh_clock.offset, p.ssh_clock.tz.clone()), (0.0, None));
    }

    /// A script call whose conductor is dropped, or fails, is still answered.
    #[test]
    fn a_dropped_conductor_answers_its_time_offset_tokens() {
        use crate::conductor::{Conductor, Setup};
        let framing = |id: &str| {
            let mut c = Conductor::new(Setup { client_unique_id: id.into(), ..Setup::default() }, None, "P");
            c.framed_pid = Some(10);
            c
        };
        // Popped with the fetch in flight.
        let mut p = Pane::with(1, None, 20, 6);
        p.conductor = Some(Box::new(framing("A")));
        p.conductor.as_mut().unwrap().fetch_time_offset(1.0, 0.0, Some(5));
        assert!(p.pop_conductor());
        let answers = p.take_ssh_answers();
        assert!(matches!(&answers[..], [(5, Err(_))]), "{answers:?}");
        // The connection ends (unhook) and the batch is applied.
        let mut p = Pane::with(1, None, 20, 6);
        p.conductor = Some(Box::new(framing("A")));
        p.conductor.as_mut().unwrap().fetch_time_offset(1.0, 0.0, Some(6));
        p.unhook_conductor_at(0);
        p.apply_ssh_effects();
        let answers = p.take_ssh_answers();
        assert!(matches!(&answers[..], [(6, Err(_))]), "{answers:?}");
        // A failure's dropped batch still answers the token.
        let mut p = Pane::with(1, None, 20, 6);
        p.conductor = Some(Box::new(framing("A")));
        p.conductor.as_mut().unwrap().fetch_time_offset(1.0, 0.0, Some(7));
        p.conductor.as_mut().unwrap().fail("boom");
        p.apply_ssh_effects();
        let answers = p.take_ssh_answers();
        assert!(matches!(&answers[..], [(7, Err(_))]), "{answers:?}");
    }

    /// A conductor chained under the pane's current one as `App::ssh_hooked` does, at `depth`
    /// (the hook fits), framing `pid` in ground when given (a conductor whose setup is done).
    fn chain(p: &mut Pane, id: &str, depth: i32, pid: Option<i32>) {
        use crate::conductor::{Conductor, Setup};
        assert!(p.ssh_init_fits(depth), "a conductor fits at depth {depth}");
        let mut c = Conductor::new(Setup { client_unique_id: id.into(), ..Setup::default() }, p.conductor.take(), "X");
        c.framed_pid = pid;
        assert_eq!(c.depth(), depth);
        p.ssh.chain.push(crate::ssh_session::Hop { unique_id: id.into(), location: "x".into() });
        p.conductor = Some(Box::new(c));
    }

    /// A `cat`'d hook: an unframed conductor that unhooked at once.
    fn chain_catd(p: &mut Pane, id: &str, depth: i32) {
        chain(p, id, depth, None);
        p.conductor.as_mut().unwrap().handle_unhook();
    }

    #[test]
    fn a_framing_hop_under_a_catd_hook_is_framed_by_every_framer_above() {
        use crate::ssh_fixtures::{dcs_hook, osc135, output};
        // `cat evil` in the framed shell (pid 100) leaves an unhooked, unframed depth 1
        // conductor; the person then runs it2ssh to a host with Python and its framer frames
        // at depth 1. Nothing typed may reach that framer's stdin raw.
        let mut p = framing_root();
        run_hook(&mut p, &output("n", 100, -1, 0, &[dcs_hook("", "evil", "YQ==", "Yg=="), osc135("unhook")].concat()));
        assert_eq!(p.conductor.as_ref().map(|c| (c.depth(), c.unhooked())), Some((1, true)));
        chain(&mut p, "x", 1, Some(300));
        assert_eq!(crate::conductor::route(p.conductor.as_deref()), crate::conductor::Route::Conductor(0));
        every_path_is_framed_by(&mut p, &|d| sent_to(100, &sent_to(300, d)));

        // At depth 3: framing d0 (100), d1 (200), d2 (300), a cat'd unhooked d3, then a framing
        // hop at depth 3 (400): each send is framed by every framer above it. A fresh pane per
        // path: nothing answers here, and four framings of a few writes fill the pipeline.
        let deep = || {
            let mut p = framed_nested();
            chain(&mut p, "c", 2, Some(300));
            chain_catd(&mut p, "evil", 3);
            chain(&mut p, "x", 3, Some(400));
            assert_eq!(p.conductor.as_ref().map(|c| c.depth()), Some(3));
            wire(&mut p);
            p
        };
        let framed = |d: &[u8]| sent_to(100, &sent_to(200, &sent_to(300, &sent_to(400, d))));
        let mut p = deep();
        p.live_input(b"k");
        assert_eq!(wire(&mut p), framed(b"k"), "typing");
        let mut p = deep();
        p.paste("a\nb");
        assert_eq!(wire(&mut p), framed(b"a\rb"), "a paste");
        let mut p = deep();
        p.reply(b"\x1b[?1;2c");
        assert_eq!(wire(&mut p), framed(b"\x1b[?1;2c"), "a report");
        let mut p = deep();
        assert!(p.coprocess_output(b"co"));
        assert_eq!(wire(&mut p), framed(b"co"), "coprocess output");
    }

    #[test]
    fn a_framing_hop_after_a_hop_without_python_is_framed() {
        use crate::ssh_fixtures::{dcs_hook, output, pre_reply};
        // A hop without Python 3.7 (unframed, unhooked, depth 1) inside the framed session, then
        // a further hop from that host that does frame.
        let mut p = framing_root();
        run_hook(&mut p, &output("n", 100, -1, 0, &dcs_hook("", "u", "YQ==", "Yg==")));
        run_hook(&mut p, &output("n", 100, -1, 0, &[pre_reply("1.1", &["sh"]), crate::ssh_fixtures::osc135("unhook")].concat()));
        assert!(p.conductor.as_ref().unwrap().unhooked() && p.conductor.as_ref().unwrap().depth() == 1);
        chain(&mut p, "x", 1, Some(300));
        every_path_is_framed_by(&mut p, &|d| sent_to(100, &sent_to(300, d)));
    }

    #[test]
    fn typing_while_connecting_from_a_level_1_hop_waits_in_the_queue() {
        // iTerm2 queues while connecting unless the CURRENT conductor handles keystrokes
        // (`PS:4417-4426`); only a route through the current conductor skips the queue.
        let mut p = framed_nested();
        p.unhook_conductor_at(1);
        assert_eq!(crate::conductor::route(p.conductor.as_deref()), crate::conductor::Route::Conductor(1));
        wire(&mut p);
        p.ssh.begin(true);
        p.live_input(b"pass");
        p.write_session(b"word");
        assert_eq!(wire(&mut p), b"", "nothing is typed at the outer shell yet");
        assert_eq!(p.ssh.connecting.as_deref(), Some(&b"password"[..]));
        // The release goes through the route.
        let queued = p.ssh.stop_queueing();
        p.send_routed(&queued);
        assert_eq!(wire(&mut p), sent_to(100, b"password"));
    }

    #[test]
    fn a_failed_setup_under_an_unhooked_parent_reaches_its_conductor_sh() {
        use crate::ssh_fixtures::{dcs_hook, osc135, output};
        // The new hop's ssh runs in the framed shell; conductor.sh is still reading its tty when
        // setup fails, so `exec_login_shell` goes through the framer to it (iTerm2 writes it
        // to the pty, which is that tty).
        let mut p = framing_root();
        run_hook(&mut p, &output("n", 100, -1, 0, &[dcs_hook("", "evil", "YQ==", "Yg=="), osc135("unhook")].concat()));
        chain(&mut p, "x", 1, None);
        wire(&mut p);
        p.conductor.as_mut().unwrap().fail("boom");
        assert_eq!(wire(&mut p), sent_to(100, b"exec_login_shell\n"));
        // A framing conductor's own failure (nothing reads a line at its end) or an unhooked
        // one (its conductor.sh is gone) types nothing.
        let mut p = framed_nested();
        wire(&mut p);
        p.conductor.as_mut().unwrap().fail("boom");
        assert_eq!(wire(&mut p), b"");
    }

    #[test]
    fn a_coprocess_cannot_flood_the_held_bytes_during_setup() {
        let mut p = Pane::with(7, None, 80, 24);
        p.conductor = setting_up();
        assert!(!p.coprocess_backlogged());
        for _ in 0..80 {
            assert!(p.coprocess_output(&[b'z'; 1024]));
        }
        assert!(p.coprocess_backlogged(), "over 64 KiB held waits");
        // The unhook releases them and the pump runs again.
        p.conductor.as_mut().unwrap().handle_unhook();
        p.take_ssh_held();
        assert!(!p.coprocess_backlogged());
    }

    /// A pane whose root hook is hooked, with one framing conductor "o" (pid 100) and its hop,
    /// connecting over.
    fn framed_root() -> Pane {
        use crate::conductor::{Conductor, Setup};
        use crate::ssh_session::Hop;
        let at = Setup { client_unique_id: "o".into(), parsed: crate::ssh_args::ParsedArgs { hostname: "o.example".into(), port: 22, ..Default::default() }, ..Setup::default() };
        let mut p = Pane::tmux(5, 1, 3, 120, 10);
        p.ssh.begin(true);
        p.tmux_io().unwrap().push_output(b"\x1bP2000pnone o YQ== - Yg==\n");
        p.pump();
        p.take_ssh();
        let mut c = Conductor::new(at, None, "O");
        c.framed_pid = Some(100);
        p.ssh.chain.push(Hop { unique_id: "o".into(), location: "o.example".into() });
        p.conductor = Some(Box::new(c));
        p.ssh.stop_queueing();
        p
    }

    #[test]
    fn disconnect_follows_iterm2() {
        use crate::ssh_fixtures::pre_cmd;
        // `PS:25603-25612`: the banner names the identity, the conductor and its hop go, what
        // was held is dropped, and the write is iTerm2's pre-framer quit with the prefix.
        let mut p = framed_root();
        let identity = p.conductor.as_ref().unwrap().identity().description();
        let guid = p.conductor.as_ref().unwrap().guid.clone();
        p.ssh_held.extend_from_slice(b"held");
        p.conductor.as_mut().unwrap().quit();
        assert_eq!(wire(&mut p), pre_cmd("\u{3}O", &["quit"]));
        assert_eq!(p.screen.all_text().matches(&format!("Disconnected from {identity}")).count(), 1, "{}", p.screen.all_text());
        assert!(p.conductor.is_none() && p.ssh.chain.hops.is_empty() && p.ssh_chain_in_step());
        assert!(p.ssh_held.is_empty(), "the write queue is cleared");
        assert_eq!(p.take_ssh_registry(), [crate::ssh_registry::Change::Remove(guid.clone()), crate::ssh_registry::Change::Remove(guid)]);
        // Typing after it goes straight to the pty.
        p.write_session(b"k");
        assert_eq!(p.tmux_io().unwrap().take_input(), b"k");
    }

    #[test]
    fn late_unhook_is_harmless() {
        use crate::ssh_fixtures::{osc135, reply};
        // `C:266-271`: the framer answers the quit and prints `unhook` after its conductor
        // has left. The unhook finds no conductor: no second banner and no write.
        let mut p = framed_root();
        p.conductor.as_mut().unwrap().quit();
        wire(&mut p);
        let text = p.screen.all_text();
        run_hook(&mut p, &[reply("q", &[], 0, 'r'), osc135("unhook")].concat());
        assert!(p.conductor.is_none());
        assert_eq!(wire(&mut p), b"");
        assert_eq!(p.screen.all_text(), text, "no second banner");
        assert!(!p.ssh_hook.active());
        // A parent survives its child's late unhook: a depth 1 unhook does not touch depth 0.
        let mut p = framed_nested();
        p.conductor.as_mut().unwrap().quit();
        assert_eq!(wire(&mut p), sent_to(100, &crate::ssh_fixtures::pre_cmd("\u{3}U", &["quit"])));
        assert_eq!(p.conductor.as_ref().map(|c| c.setup.client_unique_id.as_str()), Some("o"));
        let text = p.screen.all_text();
        p.unhook_conductor_at(1);
        assert_eq!(p.conductor.as_ref().map(|c| c.setup.client_unique_id.as_str()), Some("o"));
        assert_eq!(wire(&mut p), b"");
        assert_eq!(p.screen.all_text(), text);
        assert!(still_parses(&mut p), "the parent's hook goes on");
    }

    #[test]
    fn a_failed_setup_under_a_hooked_framer_is_framed() {
        // Task 8 findings item 2: the new hop's ssh runs in the framer's shell, so its
        // conductor.sh is waiting on that tty and `exec_login_shell` must reach it, framed
        // (held and dropped it would leave the nested ssh sitting in conductor.sh).
        let mut p = framed_root();
        chain(&mut p, "x", 1, None);
        wire(&mut p);
        p.conductor.as_mut().unwrap().fail("boom");
        assert_eq!(wire(&mut p), sent_to(100, b"exec_login_shell\n"));
        assert!(p.ssh_held.is_empty());
        assert_eq!(p.conductor.as_ref().map(|c| c.setup.client_unique_id.as_str()), Some("o"), "the abort popped the hop");
        // Two hooked framers: the nearest one frames it, and the outer one frames that.
        let mut p = framed_nested();
        chain(&mut p, "y", 2, None);
        wire(&mut p);
        p.conductor.as_mut().unwrap().fail("boom");
        assert_eq!(wire(&mut p), sent_to(100, &sent_to(200, b"exec_login_shell\n")));
    }

    #[test]
    fn an_unhooked_framer_answers_the_clock_at_once() {
        // Task 8 findings item 1: the level stays 2 after the framer unhooks, but nothing reads
        // a fetch any more, so the Clock is local and no command waits for EndSSH.
        let mut p = framed_root();
        p.conductor.as_mut().unwrap().handle_unhook();
        assert_eq!(p.ssh_level(), 2);
        assert!(!p.ssh_frames());
        let now = Instant::now();
        assert_eq!(p.ssh_clock_tick(now, Some("h")), crate::ssh_clock::Refresh::Local);
        assert_eq!(p.conductor.as_ref().unwrap().written_and_queued().count(), 0, "nothing queued");
        // A framing conductor still fetches.
        let mut p = framed_root();
        assert!(p.ssh_frames());
        assert_eq!(p.ssh_clock_tick(now, Some("h")), crate::ssh_clock::Refresh::Fetch);
    }

    /// What `App::ssh_pump_pane` does with the recovery tokens: the pane's output goes through
    /// the hook, then each token runs. True when a recovery finished (the rest of the read is
    /// dropped there).
    fn recover_from(p: &mut Pane, bytes: &[u8]) -> bool {
        use crate::conductor_parser::Token;
        use crate::ssh_hook::Out;
        p.tmux_io().unwrap().push_output(bytes);
        p.pump();
        for out in p.take_ssh() {
            match out {
                Out::Ssh(Token::BeginFramerRecovery(d), _) => p.begin_framer_recovery(d),
                Out::Ssh(Token::RecoveryLine(line), _) => {
                    if p.framer_recovery_line(&line) {
                        return true;
                    }
                }
                Out::Screen(bytes) => p.feed_after_ssh(&bytes),
                _ => {}
            }
        }
        false
    }

    /// What a live framer prints once it is told to recover: a stale block's header and end
    /// first (Tandem restarted mid-stream), then the recovery lines.
    fn stale_then_recovery(login: i32) -> Vec<u8> {
        stale_then_recovery_of(login, "99")
    }

    /// `stale_then_recovery` with the framer naming `id` as its hop's clientUniqueID.
    fn stale_then_recovery_of(login: i32, id: &str) -> Vec<u8> {
        use crate::ssh_fixtures::{osc134, recovery};
        let keys = [("dcsID", "D0"), ("sshargs", "-p 22 h"), ("boolArgs", "46"), ("clientUniqueID", id)];
        [osc134("%output 1 2 -1 0"), b"stale".to_vec(), osc134("%end 1"), recovery(login, &keys, &[5000])].concat()
    }

    /// Final review I2 (ruling): the right to recover in band ends when EndSSH empties the hop
    /// list, and nothing gives it back but a new vouched hook: a file `cat`ed at the local
    /// prompt after ssh exited makes Tandem type nothing, and nothing is saved for a restart.
    #[test]
    fn the_recovery_right_ends_with_the_last_hop() {
        let mut p = framed_root();
        p.note_hook(true);
        assert!(p.ssh_hook.recovery_allowed());
        p.unhook_ssh();
        p.end_ssh("o");
        p.tmux_io().unwrap().take_input();
        assert!(p.conductor.is_none() && p.ssh.chain.hops.is_empty());
        assert!(!p.ssh_hook.recovery_allowed() && !p.was_ssh_session(), "the right is gone, and so is the saved fact");
        assert!(!recover_from(&mut p, &stale_then_recovery(4242)));
        assert_eq!(p.tmux_io().unwrap().take_input(), b"", "nothing typed into the local shell");
        assert!(p.conductor.is_none());
        // An EndSSH that leaves a hop keeps the right.
        let mut p = framed_root();
        p.note_hook(true);
        chain(&mut p, "i", 1, Some(200));
        p.end_ssh("i");
        assert!(p.ssh_hook.recovery_allowed() && p.was_ssh_session());
    }

    /// Final review I2 (ruling): a root-level recovery replaces the hops it pops, and the
    /// recovered conductor takes the replaced hop's id, so it2ssh's EndSSH (which names the
    /// original id) ends it whatever id the remote reported.
    #[test]
    fn a_recovered_conductor_ends_with_the_hop_it_replaced() {
        let mut p = framed_root();
        p.note_hook(true);
        // The framer unhooks, then a root-level recovery naming a foreign id.
        p.conductor.as_mut().unwrap().handle_unhook();
        p.unhook_ssh();
        assert!(!recover_from(&mut p, &crate::ssh_fixtures::osc134("%output 1 2 -1 0")));
        let first = crate::ssh_fixtures::osc134("%output 1 2 -1 0").len();
        assert!(recover_from(&mut p, &stale_then_recovery_of(4242, "evil")[first..]));
        let c = p.conductor.as_ref().expect("recovered");
        assert_eq!(c.setup.client_unique_id, "o", "the id of the hop it replaced");
        assert!(p.ssh_chain_in_step());
        p.end_ssh("o");
        assert!(p.conductor.is_none() && p.ssh.chain.hops.is_empty(), "EndSSH of the original hop ends it");
        // With no hop to replace, the framer's own id stands (`framer_recovery_follows_iterm2`).
    }

    #[test]
    fn a_plain_pane_never_recovers() {
        // F27: a hostile file cat'd in a local pane, recovery lines and all, makes Tandem type
        // nothing into the local shell.
        let mut p = Pane::tmux(5, 1, 3, 120, 10);
        let bytes = stale_then_recovery(4242);
        assert!(!recover_from(&mut p, &bytes));
        assert_eq!(p.tmux_io().unwrap().take_input(), b"", "nothing was written");
        assert!(p.conductor.is_none() && !p.ssh_hook.active() && p.ssh.chain.hops.is_empty());
    }

    /// F27 (ruling): only a hook Tandem's origin token vouched for gives a pane the right to
    /// recover in band. A hostile file cat'd in a local pane, with an it2ssh OSC (not
    /// trust-gated) and a fake DCS hook before the recovery block, makes Tandem type nothing,
    /// and the pane saves no right for after a restart; an unvouched conductor restored from a
    /// state file gets none either.
    #[test]
    fn a_hostile_file_never_turns_recovery_on() {
        use crate::conductor_parser::Token;
        use crate::ssh_hook::Out;
        use crate::screen::Request;
        // As the App does: each request and token in order; the fake hook fails the token check.
        let run = |p: &mut Pane, bytes: &[u8]| {
            p.tmux_io().unwrap().push_output(bytes);
            p.pump();
            for out in p.take_ssh() {
                match out {
                    Out::Screen(bytes) => {
                        p.feed_after_ssh(&bytes);
                        for r in p.own_mut().take_requests() {
                            if r == Request::SshWillBegin {
                                p.ssh_will_begin();
                            }
                        }
                    }
                    Out::Ssh(Token::Init(_), _) => p.note_hook(false),
                    Out::Ssh(Token::BeginFramerRecovery(d), _) => p.begin_framer_recovery(d),
                    Out::Ssh(Token::RecoveryLine(line), _) => {
                        p.framer_recovery_line(&line);
                    }
                    _ => {}
                }
            }
        };
        let mut p = Pane::tmux(5, 1, 3, 120, 10);
        // The file arrives in two reads, so the second is scanned after the first was handled.
        run(&mut p, &[b"\x1b]1337;it2ssh=a b c d\x07".to_vec(), crate::ssh_fixtures::dcs_hook("", "U", "NDY=", "ZXZpbA=="), b"\x1b]135;:unhook\x1b\\".to_vec()].concat());
        run(&mut p, &stale_then_recovery(4242));
        let typed = String::from_utf8_lossy(&p.tmux_io().unwrap().take_input()).into_owned();
        assert!(!typed.contains("cmVjb3Zlcg=="), "no recover line is typed: {typed:?}");
        assert!(p.conductor.is_none() && !p.ssh_hook.recovery_allowed());
        assert!(!p.was_ssh_session(), "nothing to save for after a restart");
        // A vouched hook does turn it on.
        let mut v = Pane::tmux(5, 1, 3, 120, 10);
        v.note_hook(true);
        assert!(v.ssh_hook.recovery_allowed() && v.was_ssh_session());
        // After a restart: a conductor restored from the state file is unvouched (it may have
        // come from a hook a file printed), so it brings no right with it.
        let mut c = crate::conductor::Conductor::new(crate::conductor::Setup { client_unique_id: "a".into(), dcs_id: "D0".into(), ..Default::default() }, None, "U");
        c.framed_pid = Some(200);
        c.framer_version = Some(2);
        let mut r = Pane::tmux(5, 1, 3, 120, 10);
        assert!(r.restore_conductor(&c.json(), "D0", &c.tree()));
        assert!(!r.ssh_hook.recovery_allowed() && !r.was_ssh_session(), "the saved flag alone gives it (only a vouched pane saves it)");
    }

    #[test]
    fn framer_recovery_follows_iterm2() {
        use crate::ssh_fixtures::output;
        let mut p = Pane::tmux(5, 1, 3, 120, 10);
        p.note_ssh_session();
        // The stale block starts recovery: "Recovering ssh connection..." and the recover
        // line, with a shim in recovery.
        assert!(!recover_from(&mut p, &crate::ssh_fixtures::osc134("%output 1 2 -1 0")));
        assert_eq!(p.tmux_io().unwrap().take_input(), b"\ncmVjb3Zlcg==\n\n\n");
        assert!(matches!(p.conductor.as_ref().map(|c| c.state().clone()), Some(crate::conductor::State::Recovery(_))));
        assert!(p.ssh_chain_in_step() && p.ssh.chain.hops.len() == 1);
        assert!(p.own().all_text().contains(crate::ssh_recovery::RECOVERING), "{}", p.own().all_text());
        // The lines finish it: the shim is replaced by the recovered conductor, the hook follows
        // the tree, the framer is reset, the registry hears of it and the banner says so.
        let first = crate::ssh_fixtures::osc134("%output 1 2 -1 0").len();
        let bytes = [stale_then_recovery(4242)[first..].to_vec(), b"dropped".to_vec()].concat();
        assert!(recover_from(&mut p, &bytes), "the last line finished it");
        let c = p.conductor.as_ref().expect("recovered");
        assert_eq!((c.framed_pid, c.depth(), c.setup.client_unique_id.as_str(), c.setup.dcs_id.as_str()), (Some(4242), 0, "99", "D0"));
        assert!(p.ssh_chain_in_step() && p.ssh.chain.hops.len() == 1 && p.ssh.chain.hops[0].unique_id == "99");
        assert!(p.ssh_hook.active() && p.ssh_hook.owned(0));
        assert!(p.own().all_text().contains(crate::ssh_recovery::RECOVERED));
        let changes = p.take_ssh_registry();
        assert!(matches!(&changes[..], [crate::ssh_registry::Change::Add(_, _)]), "{changes:?}");
        p.apply_ssh_effects();
        let sent = p.tmux_io().unwrap().take_input();
        assert!(sent.starts_with(b"cmVzZXQy\n"), "reset2 first: {:?}", String::from_utf8_lossy(&sent));
        // Output of the framed shell renders again through the recovered hook.
        p.tmux_io().unwrap().push_output(&output("2", 4242, -1, 0, b"hello"));
        p.pump();
        for o in p.take_ssh() {
            if let crate::ssh_hook::Out::Screen(b) = o {
                p.feed_after_ssh(&b);
            }
        }
        assert!(p.own().all_text().contains("hello"));
    }

    #[test]
    fn a_recovery_without_a_login_ends() {
        use crate::ssh_fixtures::osc134;
        let mut p = Pane::tmux(5, 1, 3, 120, 10);
        p.note_ssh_session();
        recover_from(&mut p, &osc134("junk"));
        p.tmux_io().unwrap().take_input();
        // begin, version, end: no login. The shim quits and goes; the hook stops recovering, so
        // the next stale OSC starts one again.
        let bytes = [osc134("begin-recovery"), osc134("recovery: version 2"), osc134("end-recovery")].concat();
        assert!(!recover_from(&mut p, &bytes));
        assert!(p.conductor.is_none() && p.ssh.chain.hops.is_empty(), "the shim is gone");
        recover_from(&mut p, &osc134("junk again"));
        assert!(p.conductor.is_some(), "a new recovery began");
    }

    #[test]
    fn a_nested_recovery_keeps_its_parent() {
        use crate::ssh_fixtures::{osc134, output, recovery};
        let mut p = framing_root();
        p.note_ssh_session();
        assert!(p.ssh_hook.active());
        wire(&mut p);
        // The inner framer (a child of pid 100's shell) is found mid-stream: recovery begins for
        // the child of the depth 0 conductor, which stays as the shim's parent.
        assert!(!recover_from(&mut p, &output("a", 100, -1, 0, &osc134("%output 1 2 -1 0"))));
        assert_eq!(p.conductor.as_ref().map(|c| c.depth()), Some(1));
        assert!(p.ssh_chain_in_step() && p.ssh.chain.hops.len() == 2);
        // The recover line went into the outer framer's shell, framed.
        assert_eq!(wire(&mut p), sent_to(100, b"\ncmVjb3Zlcg==\n\n\n"));
        let keys = [("dcsID", "D9"), ("sshargs", "h2"), ("boolArgs", ""), ("clientUniqueID", "77")];
        assert!(recover_from(&mut p, &output("b", 100, -1, 0, &recovery(300, &keys, &[]))));
        let c = p.conductor.as_ref().unwrap();
        assert_eq!((c.framed_pid, c.depth(), c.parent.as_ref().map(|q| q.framed_pid)), (Some(300), 1, Some(Some(100))));
        assert!(p.ssh_chain_in_step() && p.ssh.chain.hops.len() == 2);
        assert!(p.ssh_hook.owned(0) && p.ssh_hook.owned(1) && !p.ssh_hook.owned(2), "the hook follows the tree of both");
    }

    #[test]
    fn a_recovery_at_the_wrong_depth_is_ignored() {
        // A remote cannot start a recovery for a depth the conductors do not have: nothing is
        // written and the chain is left alone.
        let mut p = framing_root();
        p.note_ssh_session();
        wire(&mut p);
        p.begin_framer_recovery(5);
        assert_eq!(wire(&mut p), b"");
        assert_eq!(p.conductor.as_ref().map(|c| (c.depth(), c.framed_pid)), Some((0, Some(100))));
        assert!(p.ssh_chain_in_step());
    }

    #[test]
    fn restore_conductor_follows_iterm2() {
        use crate::conductor::{Conductor, Setup};
        let at = |id: &str, dcs: &str| Setup { client_unique_id: id.into(), dcs_id: dcs.into(), sshargs: format!("{id}.example"), parsed: crate::ssh_args::parse(&format!("{id}.example"), "", &crate::ssh_args::NoConfig), ..Setup::default() };
        let mut outer = Conductor::new(at("o", "D0"), None, "O");
        outer.framed_pid = Some(100);
        outer.framer_version = Some(2);
        let mut inner = Conductor::new(at("a", "D1"), Some(Box::new(outer)), "U");
        inner.framed_pid = Some(200);
        inner.framer_version = Some(2);
        let (json, tree) = (inner.json(), inner.tree());
        let mut p = Pane::tmux(5, 1, 3, 120, 10);
        assert!(p.restore_conductor(&json, "D1", &tree));
        // The chain comes back, one hop per conductor, outermost first; the hook follows the
        // saved tree; the framers are reset and the registry hears of both.
        let ids: Vec<&str> = p.ssh.chain.hops.iter().map(|h| h.unique_id.as_str()).collect();
        assert_eq!(ids, ["o", "a"]);
        assert!(p.ssh_chain_in_step());
        assert_eq!(p.ssh_level(), 2);
        assert!(p.ssh_hook.owned(0) && p.ssh_hook.owned(1) && !p.ssh_hook.owned(2));
        assert_eq!(p.take_ssh_registry().len(), 2);
        assert!(!p.ssh.was_ssh, "restored conductors are unvouched: only the saved flag gives the right (F27)");
        let sent = p.tmux_io().unwrap().take_input();
        assert!(sent.starts_with(b"cmVzZXQy\n"), "the outer framer is reset first: {:?}", String::from_utf8_lossy(&sent));
        // Nothing usable: no conductor and no hook.
        let mut p = Pane::tmux(5, 1, 3, 120, 10);
        assert!(!p.restore_conductor("{}", "D1", &tree));
        assert!(p.conductor.is_none() && !p.ssh_hook.active() && p.ssh.chain.hops.is_empty());
        assert!(!p.restore_conductor(&json, "D1", &crate::ssh_recovery::Tree::default()), "no tree: nothing to parse with");
        assert!(p.conductor.is_none());
    }

    /// The it2 state of the conductor being recovered rides the shim into the recovered one
    /// (`PS:20307-20342` `adoptIT2RecoveryStateFrom:`, `CD+IT2:485-509`): Tandem's is the
    /// inbox kept for 14e (ruling 3: nothing reads it yet).
    #[test]
    fn recovery_keeps_the_it2_state() {
        let mut p = Pane::tmux(5, 1, 3, 120, 10);
        p.note_hook(true);
        let mut old = crate::conductor::Conductor::new(crate::conductor::Setup { client_unique_id: "old".into(), ..Default::default() }, None, "U");
        old.it2_inbox = vec!["3 open".into()];
        p.ssh.chain.push(crate::ssh_session::Hop { unique_id: "old".into(), location: "h".into() });
        p.conductor = Some(Box::new(old));
        p.begin_framer_recovery(-1);
        assert_eq!(p.conductor.as_ref().map(|c| c.it2_inbox.clone()), Some(vec!["3 open".to_string()]), "the shim carries it");
        let first = crate::ssh_fixtures::osc134("%output 1 2 -1 0").len();
        assert!(recover_from(&mut p, &stale_then_recovery(4242)[first..]));
        assert_eq!(p.conductor.as_ref().map(|c| c.it2_inbox.clone()), Some(vec!["3 open".to_string()]), "and the recovered conductor");
    }

    /// iTerm2 marks a conductor waiting to resynchronize only when it is made by a recovery
    /// (`CD:593`, `CD:1791`), never when it comes from saved JSON: after a restart of a two-hop
    /// session the outer host's process list works as the inner one's does.
    #[test]
    fn a_restored_chain_lists_every_hosts_processes() {
        use crate::conductor::{Conductor, Setup};
        let at = |id: &str, dcs: &str| Setup { client_unique_id: id.into(), dcs_id: dcs.into(), sshargs: format!("{id}.example"), parsed: crate::ssh_args::parse(&format!("{id}.example"), "", &crate::ssh_args::NoConfig), ..Setup::default() };
        let mut outer = Conductor::new(at("o", "D0"), None, "O");
        outer.framed_pid = Some(100);
        outer.framer_version = Some(2);
        let mut inner = Conductor::new(at("a", "D1"), Some(Box::new(outer)), "U");
        inner.framed_pid = Some(200);
        inner.framer_version = Some(2);
        let mut p = Pane::tmux(5, 1, 3, 120, 10);
        assert!(p.restore_conductor(&inner.json(), "D1", &inner.tree()));
        let c = p.conductor.as_mut().unwrap();
        assert!(c.processes().is_some(), "the inner host's");
        assert!(c.parent.as_mut().unwrap().processes().is_some(), "the outer host's too");
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

    /// Task 4 fix: back at the bottom, the history accumulator's remainder is dropped
    /// (`PTYScrollView.m:117-121`).
    #[test]
    fn returning_to_the_bottom_resets_the_scroll_remainder() {
        let prefs = crate::wheel::Prefs { acceleration: 1.0, fast_trackpad: true, modern: true, sensitive: false };
        let mut p = Pane::recording(1, 80, 24);
        assert_eq!(p.wheel.scroll.delta(0.4, true, &prefs), 0.0);
        p.scroll(Scroll::Bottom);
        assert_eq!(p.wheel.scroll.delta(0.7, true, &prefs), 0.0, "the 0.4 was dropped, 0.7 alone is not a line");
    }
    struct MemSink(Arc<std::sync::Mutex<Vec<u8>>>);
    impl crate::session_log::Sink for MemSink {
        fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(())
        }
    }

    fn logged(p: &mut Pane, style: crate::session_log::Style) -> Arc<std::sync::Mutex<Vec<u8>>> {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        p.log = Some(crate::session_log::Logger::new(style, Box::new(MemSink(seen.clone())), None, None, None));
        p.log_started = Some(Instant::now());
        seen
    }

    fn text(seen: &Arc<std::sync::Mutex<Vec<u8>>>) -> String {
        String::from_utf8_lossy(&seen.lock().unwrap()).into_owned()
    }

    #[test]
    fn a_log_takes_what_the_screen_takes() {
        // Plain text: the cooked text of what reaches the screen; Raw: the bytes as read.
        let mut p = Pane::recording(1, 20, 4);
        let plain = logged(&mut p, crate::session_log::Style::PlainText);
        p.feed_output(b"\x1b[31mred\x1b[0m line\r\nnext\r\n");
        assert_eq!(text(&plain), "red line\nnext\n");
        let mut q = Pane::recording(2, 20, 4);
        let raw = logged(&mut q, crate::session_log::Style::Raw);
        q.log_raw(b"\x1b[31mred\x1b[0m");
        q.feed_output(b"not logged twice");
        assert_eq!(text(&raw), "\x1b[31mred\x1b[0m", "raw takes the read, not the screen text");
    }

    #[test]
    fn a_tmux_pane_logs_raw_only_and_a_gateway_never_cooked() {
        let mut t = Pane::tmux(1, 9, 0, 20, 4);
        let ascii = logged(&mut t, crate::session_log::Style::Asciicast);
        t.log_raw(b"%output text");
        assert_eq!(text(&ascii), "", "asciicast takes no tmux pane output (PS:10983-10985)");
        let mut r = Pane::tmux(2, 9, 1, 20, 4);
        let raw = logged(&mut r, crate::session_log::Style::Raw);
        r.log_raw(b"abc");
        assert_eq!(text(&raw), "abc");
        let mut g = Pane::recording(3, 20, 4);
        g.gateway = true;
        let plain = logged(&mut g, crate::session_log::Style::PlainText);
        g.feed_output(b"control stream\r\n");
        assert_eq!(text(&plain), "", "a tmux gateway is never cooked-logged (PS:15592)");
    }

    #[test]
    fn a_stopped_log_writes_nothing_more() {
        let mut p = Pane::recording(1, 20, 4);
        let seen = logged(&mut p, crate::session_log::Style::PlainText);
        p.feed_output(b"before\r\n");
        p.stop_log();
        p.feed_output(b"after\r\n");
        p.log_text("after too");
        assert!(p.log.is_none() && p.log_started.is_none());
        assert_eq!(text(&seen), "before\n");
    }

    #[test]
    fn compact_logging_logs_its_own_text_and_skips_only_at_the_prompt() {
        let mut p = Pane::recording(1, 20, 4);
        p.log_compact = (true, true);
        let seen = logged(&mut p, crate::session_log::Style::PlainText);
        // No shell integration: not at a prompt, so output is logged.
        p.feed_output(b"output\r\n");
        p.log_text("$ ");
        p.log_text("ls -l");
        assert_eq!(text(&seen), "output\n$ ls -l");
        // Raw takes no compact text.
        let mut q = Pane::recording(2, 20, 4);
        let raw = logged(&mut q, crate::session_log::Style::Raw);
        q.log_text("$ ");
        assert_eq!(text(&raw), "");
    }

    #[test]
    fn a_resize_is_an_asciicast_record() {
        let mut p = Pane::recording(1, 20, 4);
        let seen = logged(&mut p, crate::session_log::Style::Asciicast);
        p.resize(30, 6);
        assert!(text(&seen).contains("\"r\"") && text(&seen).contains("30x6"), "{:?}", text(&seen));
    }
}

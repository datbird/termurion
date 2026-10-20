//! The tmux gateway (iTerm2's TmuxGateway): commands and their replies (matched by the
//! order of `%begin` blocks), command lists, the rules for errors, notifications, and the
//! first reply, and the write gate that holds everything until `%session-changed`.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::tmux_proto::{self, Note};

pub type Tag = u64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flags {
    /// An error reply does not abort tmux mode.
    pub tolerate: bool,
    /// The reply is wanted as raw bytes too.
    pub wants_data: bool,
    /// A slow reply offers Force Detach (detach, kill-pane, kill-window, kill-session).
    pub laggy: bool,
    /// An error reply keeps tmux's text: the reason a refusal shows.
    pub wants_error: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Reply { tag: Tag, ok: bool, text: String, data: Vec<u8> },
    Note(Note),
    /// tmux mode must end: this message in "tmux Reported a Problem".
    Abort(String),
    /// The first reply came (iTerm2's tmuxInitialCommandDidCompleteSuccessfully).
    Initialized,
    InitialFailed(String),
    /// A line for the gateway session's screen.
    Print(String),
    /// tmux left (or the stream was not tmux): the gateway is disconnected.
    Exit,
    /// psmux sends no `%exit` when the attached session is killed: the gateway is
    /// disconnected, and the app must end the client itself.
    Gone,
}

struct Cmd {
    text: String,
    tag: Option<Tag>,
    flags: Flags,
    in_list: bool,
    last_in_list: bool,
    initial: bool,
    sent: Instant,
}

struct Current {
    cmd: Option<Cmd>,
    id: String,
    text: String,
    data: Vec<u8>,
}

/// The Command Menu characters in typing: a key counts by its first character (iTerm2 reads
/// the key event's characters, where arrows and function keys are private-use codes), so an
/// encoded key that starts with ESC and has more after it is not Esc; typed text counts
/// each character.
pub fn menu_chars(bytes: &[u8], key: bool) -> Vec<char> {
    let text = String::from_utf8_lossy(bytes);
    if !key {
        return text.chars().collect();
    }
    if bytes.len() > 1 && bytes[0] == 0x1b {
        return Vec::new();
    }
    text.chars().take(1).collect()
}

/// How long a laggy command may wait before Force Detach is offered.
pub const LAG: Duration = Duration::from_secs(5);

pub struct Gateway {
    newline: String,
    queue: VecDeque<Cmd>,
    current: Option<Current>,
    out: Vec<u8>,
    held: Vec<u8>,
    can_write: bool,
    accept_notes: bool,
    initialized: bool,
    detach_sent: bool,
    disconnected: bool,
    stray: String,
    pub logging: bool,
    /// tolerateUnrecognizedTmuxCommands: text that is not tmux is kept for `%exit`.
    pub tolerate_unrecognized: bool,
    /// A divider drag runs: tmux's lines wait (iTerm2's pauseTokenExecution).
    paused: bool,
    stash: Vec<Vec<u8>>,
    /// tmux 3.2 or later (the wait-exit handshake).
    pub v3_2: bool,
    /// The server is psmux (native Windows tmux): its `phony-command` succeeds.
    pub psmux: bool,
    /// The attached session, from `%session-changed`.
    session: Option<(u32, String)>,
    /// psmux: a detach was asked for, and the app must end the client.
    left: bool,
}

/// Whether `cmd` ends the attached session `session`: `kill-server`, or `kill-session` of it
/// (by `$id` or name, or with no target). `kill-session -a` keeps the attached one.
pub fn kills_session(cmd: &str, session: Option<&(u32, String)>) -> bool {
    let words: Vec<&str> = cmd.split_whitespace().collect();
    match words.first() {
        Some(&"kill-server") => return true,
        Some(&"kill-session") => {}
        _ => return false,
    }
    if words.contains(&"-a") {
        return false;
    }
    let Some(at) = words.iter().position(|w| *w == "-t") else { return true };
    if words.get(at + 1).is_none() {
        return true;
    }
    // The name may hold spaces: take the rest of the command, minus quotes.
    let target = words[at + 1..].join(" ");
    let target = target.trim_matches(|c| c == '"' || c == '\'');
    let Some((id, name)) = session else { return false };
    target == format!("${id}") || target == name.as_str()
}

impl Gateway {
    pub fn new(newline: &str) -> Gateway {
        Gateway { newline: newline.into(), queue: VecDeque::new(), current: None, out: Vec::new(), held: Vec::new(), can_write: false, accept_notes: false, initialized: false, detach_sent: false, disconnected: false, stray: String::new(), logging: false, tolerate_unrecognized: false, paused: false, stash: Vec::new(), v3_2: false, psmux: false, session: None, left: false }
    }

    /// Sends one command. With no tag its errors are tolerated (iTerm2 does the same).
    pub fn send(&mut self, cmd: &str, tag: Option<Tag>, flags: Flags) {
        if self.detach_sent || self.disconnected {
            return;
        }
        let flags = Flags { tolerate: flags.tolerate || tag.is_none(), ..flags };
        self.queue.push_back(Cmd { text: cmd.into(), tag, flags, in_list: false, last_in_list: false, initial: false, sent: Instant::now() });
        let line = format!("{cmd}{}", self.newline);
        self.write(line.as_bytes());
    }

    /// Sends commands as one line joined by "; ". An error fails the rest of the list;
    /// `initial`: the last reply turns notifications on.
    pub fn send_list(&mut self, cmds: Vec<(String, Option<Tag>, Flags)>, initial: bool) {
        if self.detach_sent || self.disconnected || cmds.is_empty() {
            return;
        }
        let n = cmds.len();
        let mut line = String::new();
        for (i, (text, tag, flags)) in cmds.into_iter().enumerate() {
            if i > 0 {
                line.push_str("; ");
            }
            line.push_str(&text);
            let last = i + 1 == n;
            self.queue.push_back(Cmd { text, tag, flags, in_list: true, last_in_list: last, initial: initial && last, sent: Instant::now() });
        }
        line.push_str(&self.newline);
        self.write(line.as_bytes());
    }

    fn write(&mut self, bytes: &[u8]) {
        if self.can_write {
            self.out.extend_from_slice(bytes);
        } else {
            self.held.extend_from_slice(bytes);
        }
    }

    /// Esc in the gateway session: `detach`, and nothing more after it.
    pub fn detach(&mut self) {
        if self.psmux {
            // psmux's client leaves on `detach` only when it reads more input, and the server
            // is its own process: dropping the client is the detach.
            self.detach_sent = true;
            self.left = true;
            self.force_disconnect();
            return;
        }
        self.send("detach", None, Flags { laggy: true, tolerate: true, ..Default::default() });
        self.detach_sent = true;
    }

    /// psmux only: true once after a detach, when the app must end the client.
    pub fn take_left(&mut self) -> bool {
        std::mem::take(&mut self.left)
    }

    /// Shell > tmux > Detach (iTerm2's requestDetach): false when a detach already went out,
    /// so the caller asks whether to force it.
    pub fn request_detach(&mut self) -> bool {
        if self.detach_sent {
            return false;
        }
        self.detach();
        true
    }

    /// After an Abort: `detach`, then nothing more (iTerm2's abortWithErrorMessage).
    pub fn abort(&mut self) {
        self.detach();
        self.force_disconnect();
    }

    /// X in the gateway session, or an abort: let go without telling tmux.
    pub fn force_disconnect(&mut self) {
        self.disconnected = true;
        self.queue.clear();
    }

    /// No window opened on attach: notifications start now (iTerm2 sets acceptNotifications).
    pub fn accept_notifications(&mut self) {
        self.accept_notes = true;
    }

    pub fn enable_writes(&mut self) {
        self.can_write = true;
        let held = std::mem::take(&mut self.held);
        self.out.extend(held);
    }

    /// Bytes to write to the gateway pane now.
    pub fn take_writes(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }

    pub fn detach_sent(&self) -> bool {
        self.detach_sent
    }

    pub fn disconnected(&self) -> bool {
        self.disconnected
    }

    /// A laggy command (detach, kill-*) still waits, and some command waited over `LAG`.
    pub fn hung(&self, now: Instant) -> bool {
        let cmds = || self.current.iter().filter_map(|c| c.cmd.as_ref()).chain(self.queue.iter());
        cmds().any(|c| c.flags.laggy) && cmds().any(|c| now.saturating_duration_since(c.sent) > LAG)
    }

    /// When `hung` may next turn true: while a laggy command waits, the oldest command's
    /// time plus `LAG`.
    pub fn next_hung_check(&self) -> Option<Instant> {
        let cmds = || self.current.iter().filter_map(|c| c.cmd.as_ref()).chain(self.queue.iter());
        if !cmds().any(|c| c.flags.laggy) {
            return None;
        }
        cmds().map(|c| c.sent + LAG).min()
    }

    /// Holds tmux's lines until `resume`.
    pub fn pause(&mut self) {
        self.paused = true;
    }

    /// Runs the held lines.
    pub fn resume(&mut self) -> Vec<Event> {
        self.paused = false;
        let held = std::mem::take(&mut self.stash);
        held.iter().flat_map(|l| self.line(l)).collect()
    }

    /// One line from the hook.
    pub fn line(&mut self, raw: &[u8]) -> Vec<Event> {
        if self.paused {
            self.stash.push(raw.to_vec());
            return Vec::new();
        }
        let mut ev = Vec::new();
        let text = String::from_utf8_lossy(raw).into_owned();
        if self.logging {
            ev.push(Event::Print(format!("< {text}")));
        }
        let is_exit = text == "%exit" || text.starts_with("%exit ");
        // tmux 1.8 can print %exit with no end guard: finish the reply first.
        if self.current.is_some() && is_exit {
            self.finish(false, &mut ev);
        }
        if let Some(cur) = &mut self.current {
            if text.starts_with(&format!("%end {}", cur.id)) {
                self.finish(false, &mut ev);
            } else if text.starts_with(&format!("%error {}", cur.id)) {
                self.finish(true, &mut ev);
            } else {
                cur.text.push_str(&text);
                cur.text.push('\n');
                cur.data.extend_from_slice(raw);
                cur.data.push(b'\n');
            }
            return ev;
        }
        // Lines only read once notifications are on (after the first window list).
        let gated = ["%output ", "%extended-output ", "%layout-change ", "%window-add", "%window-close", "%unlinked-window-close", "%window-renamed", "%unlinked-window-renamed", "%unlinked-window-add", "%session-renamed", "%sessions-changed", "%window-pane-changed", "%subscription-changed ", "%session-window-changed", "%client-session-changed ", "%client-detached ", "%paste-buffer-changed "];
        if gated.iter().any(|g| text.starts_with(g)) && !self.accept_notes {
            return ev;
        }
        let parsed = if self.psmux { tmux_proto::parse_psmux(raw, self.session.as_ref().map(|s| s.0)) } else { tmux_proto::parse(raw) };
        match parsed {
            Err(e) => ev.push(Event::Abort(e)),
            Ok(Note::Begin { time, flags, .. }) => self.begin(time, flags, &mut ev),
            Ok(Note::End { .. } | Note::Error { .. } | Note::Continue) => {}
            Ok(Note::SessionChanged { session, name }) => {
                self.session = Some((session, name.clone()));
                ev.push(Event::Note(Note::SessionChanged { session, name }));
                if !self.can_write {
                    self.enable_writes();
                }
            }
            Ok(Note::Exit(reason)) => {
                if !self.stray.is_empty() {
                    ev.push(Event::Print(String::new()));
                    ev.push(Event::Print("** ERROR **".into()));
                    ev.push(Event::Print("tmux exited with message:".into()));
                    for l in self.stray.lines().filter(|l| !l.is_empty()) {
                        ev.push(Event::Print(l.to_string()));
                    }
                    ev.push(Event::Print("*".repeat(80)));
                } else if reason.is_some() {
                    ev.push(Event::Print("tmux exited unexpectedly.".into()));
                    ev.push(Event::Print(text.clone()));
                }
                if self.v3_2 {
                    let nl = self.newline.clone();
                    self.out.extend_from_slice(nl.as_bytes());
                }
                self.disconnected = true;
                ev.push(Event::Exit);
            }
            Ok(Note::Text(_)) if self.tolerate_unrecognized => {
                self.stray.push_str(&text);
                self.stray.push('\n');
            }
            Ok(Note::Text(_)) => {
                ev.push(Event::Print("Unrecognized command from tmux. Did your ssh session die? The command was:".into()));
                ev.push(Event::Print(text.clone()));
                self.disconnected = true;
                ev.push(Event::Exit);
            }
            // %noop, %pane-mode-changed, and notifications Termurion does not know are logged only.
            Ok(Note::Other(_)) => {}
            Ok(n) => {
                if let Note::SessionRenamed { session, name } = &n
                    && self.session.as_ref().is_some_and(|(s, _)| s == session) {
                        self.session = Some((*session, name.clone()));
                    }
                ev.push(Event::Note(n));
            }
        }
        ev
    }

    /// `%begin`: a reply to the next queued command, or (flags without bit 1) a block
    /// tmux sent on its own.
    fn begin(&mut self, time: String, flags: i64, ev: &mut Vec<Event>) {
        if self.current.is_some() {
            ev.push(Event::Abort("%begin without %end".into()));
            return;
        }
        if flags & 1 == 0 {
            self.current = Some(Current { cmd: None, id: time, text: String::new(), data: Vec::new() });
            return;
        }
        let Some(cmd) = self.queue.pop_front() else {
            ev.push(Event::Abort("%begin with empty command queue".into()));
            return;
        };
        if self.logging {
            ev.push(Event::Print(format!("[Begin response for {}]", cmd.text)));
        }
        self.current = Some(Current { cmd: Some(cmd), id: time, text: String::new(), data: Vec::new() });
    }

    /// The reply is complete (iTerm2's currentCommandResponseFinishedWithError).
    fn finish(&mut self, mut error: bool, ev: &mut Vec<Event>) {
        loop {
            let Some(mut cur) = self.current.take() else { return };
            if cur.text.ends_with('\n') {
                cur.text.pop();
                cur.data.pop();
            }
            let flags = cur.cmd.as_ref().map(|c| c.flags).unwrap_or_default();
            if error {
                if self.logging {
                    ev.push(Event::Print(format!("[Error \u{201c}{}\u{201d} in response to \u{201c}{}\u{201d}]", cur.text, cur.cmd.as_ref().map_or("", |c| c.text.as_str()))));
                }
                if !flags.tolerate && (self.initialized || cur.cmd.is_some()) {
                    ev.push(Event::Abort(format!("Error: {}.\n\nInfo:\n{}", cur.text, cur.cmd.as_ref().map_or("", |c| c.text.as_str()))));
                    return;
                }
            }
            if let Some(tag) = cur.cmd.as_ref().and_then(|c| c.tag) {
                let (text, data) = if error && !flags.wants_error { (String::new(), Vec::new()) } else { (cur.text.clone(), cur.data.clone()) };
                ev.push(Event::Reply { tag, ok: !error, text, data });
            }
            if !error && self.logging
                && let Some(c) = &cur.cmd {
                    ev.push(Event::Print(format!("[Normal response to \u{201c}{}\u{201d}]", c.text)));
                }
            // psmux answers the kill of the attached session and says nothing more.
            if !error && self.psmux && !self.disconnected && cur.cmd.as_ref().is_some_and(|c| kills_session(&c.text, self.session.as_ref())) {
                self.disconnected = true;
                self.queue.clear();
                ev.push(Event::Gone);
                return;
            }
            let fail_next = error && cur.cmd.as_ref().is_some_and(|c| c.in_list && !c.last_in_list);
            if cur.cmd.as_ref().is_some_and(|c| c.initial) {
                self.accept_notes = true;
            }
            if !self.initialized {
                self.initialized = true;
                ev.push(if error && !flags.tolerate { Event::InitialFailed(cur.text.clone()) } else { Event::Initialized });
            }
            if !fail_next {
                return;
            }
            // The next command of the list fails the same way.
            let Some(next) = self.queue.pop_front() else { return };
            self.current = Some(Current { cmd: Some(next), id: "n/a".into(), text: String::new(), data: Vec::new() });
            error = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(g: &mut Gateway, lines: &[&str]) -> Vec<Event> {
        lines.iter().flat_map(|l| g.line(l.as_bytes())).collect()
    }

    fn tol() -> Flags {
        Flags { tolerate: true, ..Default::default() }
    }

    fn started() -> Gateway {
        let mut g = Gateway::new("\r");
        let ev = feed(&mut g, &["%begin 1 1 0", "%end 1 1 0", "%session-changed $0 main"]);
        assert!(ev.contains(&Event::Initialized));
        g
    }

    #[test]
    fn replies_match_by_begin() {
        let mut g = started();
        g.send("list-sessions", Some(1), Flags::default());
        assert_eq!(g.take_writes(), b"list-sessions\r");
        let ev = feed(&mut g, &["%begin 100 5 1", "a", "b", "%end 100 5 1"]);
        assert_eq!(ev, vec![Event::Reply { tag: 1, ok: true, text: "a\nb".into(), data: b"a\nb".to_vec() }]);
        // A server block (flags 0) needs no command.
        assert!(feed(&mut g, &["%begin 7 7 0", "x", "%end 7 7 0"]).is_empty());
        assert_eq!(feed(&mut g, &["%begin 9 9 1"]), vec![Event::Abort("%begin with empty command queue".into())]);
    }

    #[test]
    fn writes_wait_for_session_changed() {
        let mut g = Gateway::new("\n");
        g.send("phony-command", None, Flags::default());
        assert!(g.take_writes().is_empty(), "held until tmux names the session");
        let ev = feed(&mut g, &["%begin 1 1 0", "%end 1 1 0", "%session-changed $3 work"]);
        assert!(ev.contains(&Event::Note(Note::SessionChanged { session: 3, name: "work".into() })));
        assert_eq!(g.take_writes(), b"phony-command\n", "Tmux Newline");
    }

    #[test]
    fn list_errors_fail_the_rest() {
        let mut g = started();
        g.send_list(vec![("a".into(), Some(1), tol()), ("b".into(), Some(2), tol()), ("c".into(), Some(3), tol())], false);
        assert_eq!(g.take_writes(), b"a; b; c\r");
        let ev = feed(&mut g, &["%begin 5 5 1", "oops", "%error 5 5 1"]);
        let fails: Vec<Tag> = ev.iter().filter_map(|e| if let Event::Reply { tag, ok: false, .. } = e { Some(*tag) } else { None }).collect();
        assert_eq!(fails, vec![1, 2, 3], "the rest of the list fails the same way");
        g.send("bad", Some(4), Flags::default());
        let ev = feed(&mut g, &["%begin 6 6 1", "no such thing", "%error 6 6 1"]);
        assert_eq!(ev, vec![Event::Abort("Error: no such thing.\n\nInfo:\nbad".into())]);
    }

    #[test]
    fn notifications_wait_for_the_initial_reply() {
        let mut g = started();
        assert!(feed(&mut g, &["%output %1 early"]).is_empty(), "not yet");
        g.send_list(vec![("list-windows".into(), Some(1), tol())], true);
        feed(&mut g, &["%begin 2 2 1", "%end 2 2 1"]);
        assert_eq!(feed(&mut g, &["%output %1 now"]), vec![Event::Note(Note::Output { pane: 1, data: b"now".to_vec() })]);
        assert_eq!(feed(&mut g, &["%window-add x"]), vec![Event::Abort("Malformed command (expected %window-add id): \"%window-add x\"".into())]);
    }

    #[test]
    fn unrecognized_text_aborts() {
        let mut g = started();
        assert_eq!(
            feed(&mut g, &["bash: tmux: command not found"]),
            vec![Event::Print("Unrecognized command from tmux. Did your ssh session die? The command was:".into()), Event::Print("bash: tmux: command not found".into()), Event::Exit]
        );
        let mut g = started();
        g.v3_2 = true;
        assert_eq!(feed(&mut g, &["%exit server exited"]), vec![Event::Print("tmux exited unexpectedly.".into()), Event::Print("%exit server exited".into()), Event::Exit]);
        assert_eq!(g.take_writes(), b"\r", "tmux 3.2's wait-exit wants a newline");
        assert!(g.disconnected());
    }

    #[test]
    fn only_a_bare_escape_detaches() {
        assert_eq!(menu_chars(b"\x1b", true), vec!['\u{1b}']);
        for key in [&b"\x1b[A"[..], b"\x1b[15~", b"\x1bx", b"\x1bOP"] {
            assert!(menu_chars(key, true).is_empty(), "{key:?}");
        }
        assert_eq!(menu_chars(b"Lx", true), vec!['L'], "a key counts by its first character");
        assert_eq!(menu_chars(b"LC", false), vec!['L', 'C'], "typed text counts each one");
    }

    #[test]
    fn second_detach_offers_force() {
        let mut g = started();
        assert!(g.request_detach());
        assert_eq!(g.take_writes(), b"detach\r");
        assert!(!g.request_detach(), "a second press asks to force it");
        assert!(g.take_writes().is_empty());
    }

    #[test]
    fn paused_lines_wait() {
        let mut g = started();
        g.send("x", Some(1), tol());
        g.pause();
        assert!(feed(&mut g, &["%begin 3 3 1", "r", "%end 3 3 1"]).is_empty(), "held while a divider drags");
        let ev = g.resume();
        assert!(matches!(&ev[..], [Event::Reply { tag: 1, .. }]), "{ev:?}");
        assert!(g.resume().is_empty());
    }

    #[test]
    fn abort_detaches_then_disconnects() {
        let mut g = started();
        g.send("bad", Some(4), Flags::default());
        g.take_writes();
        let ev = feed(&mut g, &["%begin 6 6 1", "no such thing", "%error 6 6 1"]);
        assert!(matches!(&ev[..], [Event::Abort(_)]));
        g.abort();
        assert_eq!(g.take_writes(), b"detach\r", "iTerm2 detaches before it lets go");
        assert!(g.disconnected());
        g.send("list-windows", None, Flags::default());
        assert!(g.take_writes().is_empty());
    }

    #[test]
    fn tolerated_text_waits_for_exit() {
        let mut g = started();
        g.tolerate_unrecognized = true;
        assert!(feed(&mut g, &["sh: 1: oops"]).is_empty());
        let ev = feed(&mut g, &["%exit"]);
        let star = "*".repeat(80);
        let want: Vec<Event> = ["", "** ERROR **", "tmux exited with message:", "sh: 1: oops", &star].iter().map(|s| Event::Print(s.to_string())).chain([Event::Exit]).collect();
        assert_eq!(ev, want);
    }

    #[test]
    fn slow_detach_offers_force() {
        let mut g = started();
        let t0 = Instant::now();
        assert_eq!(g.next_hung_check(), None);
        g.detach();
        assert!(g.next_hung_check().is_some_and(|t| t >= t0 + LAG));
        assert!(g.detach_sent());
        assert!(!g.hung(t0));
        assert!(g.hung(t0 + LAG + Duration::from_millis(1)));
        g.send("list-windows", None, Flags::default());
        assert!(g.take_writes().starts_with(b"detach\r"), "nothing goes out after detach");
    }

    #[test]
    fn killing_the_attached_session_is_found_by_target() {
        let me = (3u32, "work".to_string());
        let k = |c: &str| kills_session(c, Some(&me));
        assert!(k("kill-session -t $3"));
        assert!(k("kill-session -t \"$3\""));
        assert!(k("kill-session -t work"));
        assert!(k("kill-session"));
        assert!(k("kill-server"));
        assert!(!k("kill-session -t $4"));
        assert!(!k("kill-session -t other"));
        assert!(!k("kill-session -a -t $3"), "-a keeps the named session");
        assert!(!k("kill-window -t @1"));
        assert!(!k("display-message kill-session"));
        assert!(!kills_session("kill-session -t work", None), "no attached session known");
        let spaced = (5u32, "my work".to_string());
        assert!(kills_session("kill-session -t \"my work\"", Some(&spaced)));
    }

    fn psmux_started() -> Gateway {
        let mut g = started();
        g.psmux = true;
        g
    }

    #[test]
    fn psmux_kill_of_the_attached_session_ends_the_gateway() {
        let mut g = psmux_started();
        g.send("kill-session -t $0", Some(1), tol());
        g.take_writes();
        let ev = feed(&mut g, &["%begin 9 9 1", "%end 9 9 1"]);
        assert_eq!(ev, vec![Event::Reply { tag: 1, ok: true, text: String::new(), data: Vec::new() }, Event::Gone]);
        assert!(g.disconnected());
        g.send("list-windows", None, Flags::default());
        assert!(g.take_writes().is_empty(), "nothing goes out after");
        // The same command on real tmux waits for %exit, as before.
        let mut g = started();
        g.send("kill-session -t $0", Some(1), tol());
        let ev = feed(&mut g, &["%begin 9 9 1", "%end 9 9 1"]);
        assert_eq!(ev, vec![Event::Reply { tag: 1, ok: true, text: String::new(), data: Vec::new() }]);
        assert!(!g.disconnected());
        // Another session's kill, or a failed one, does not end psmux's gateway.
        let mut g = psmux_started();
        g.send("kill-session -t $7", Some(1), tol());
        assert_eq!(feed(&mut g, &["%begin 9 9 1", "%end 9 9 1"]).len(), 1);
        g.send("kill-session -t $0", Some(2), tol());
        assert!(!feed(&mut g, &["%begin 9 9 1", "no such", "%error 9 9 1"]).contains(&Event::Gone));
    }

    #[test]
    fn psmux_session_rename_without_an_id_is_the_attached_one() {
        let mut g = psmux_started();
        g.accept_notifications();
        let ev = feed(&mut g, &["%session-renamed renamed"]);
        assert_eq!(ev, vec![Event::Note(Note::SessionRenamed { session: 0, name: "renamed".into() })]);
        let mut t = started();
        t.accept_notifications();
        assert!(matches!(&feed(&mut t, &["%session-renamed renamed"])[..], [Event::Abort(_)]), "real tmux keeps its parser");
    }

    #[test]
    fn psmux_detach_drops_the_client_and_sends_nothing() {
        let mut g = psmux_started();
        g.take_writes();
        assert!(g.request_detach());
        assert!(g.take_writes().is_empty(), "psmux's client would sit on a detach");
        assert!(g.disconnected() && g.detach_sent());
        assert!(g.take_left());
        assert!(!g.take_left(), "told once");
        assert!(!g.request_detach(), "a second request asks to force it");
        // Real tmux sends `detach` and waits for %exit.
        let mut t = started();
        t.take_writes();
        assert!(t.request_detach());
        assert_eq!(t.take_writes(), b"detach\r");
        assert!(!t.take_left() && !t.disconnected());
    }
}

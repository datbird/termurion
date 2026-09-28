//! The framer half of the Conductor (`CR`, milestone 14c): iTerm2 splits
//! `Conductor+SSHCommandRunning.swift` from `Conductor.swift` the same way.

use crate::conductor::{Command, Conductor, Effect, Handler};
use crate::conductor_parser::{AUTOPOLL_PID, NOTIF_PID};
use crate::framer_script::framer_code;

/// How many `%it2` frames a conductor keeps.
const IT2_INBOX_MAX: usize = 16;
/// The most of one `%it2` frame it keeps (F30).
pub(crate) const IT2_ENTRY_MAX: usize = 64 * 1024;

/// The most background jobs one conductor keeps; the remote answers each `run` with pid
/// lines, so a hostile one could add without end.
const MAX_JOBS: usize = 256;
/// The most output one job keeps (1 MiB, the parser's own cap on a block); the rest is dropped.
const JOB_OUTPUT_MAX: usize = 1024 * 1024;
/// The longest uname, home directory or host name kept (a path's limit); a longer value is
/// treated as unusable.
const VALUE_MAX: usize = 4096;

/// What a background command's result is for (`CR` completion closures). 14c Task 8 adds
/// the clock's time offset.
#[derive(Clone, Debug, PartialEq)]
pub enum Purpose {
    Uname,
    Home,
    Hostname,
    Ignore,
}

/// A running background command (`handleBackgroundJob`): its output pieces, channel 1 only.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub command: Command,
    pub purpose: Purpose,
    pub output: Vec<String>,
}

/// The remote tty (`CD:672-679`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TtyState {
    pub echo: bool,
    pub icanon: bool,
}

impl Default for TtyState {
    fn default() -> Self {
        TtyState { echo: true, icanon: true }
    }
}

impl TtyState {
    /// Echo off with line input on.
    pub fn at_password_prompt(&self) -> bool {
        !self.echo && self.icanon
    }
}

impl Conductor {
    /// runRemoteCommand (`CR:57-69`): `run` on the framer; its reply is the child's pid, and
    /// the result reaches `complete` when the child ends. A framed pid of 0 answers at once.
    pub fn run_remote_command(&mut self, command: &str, purpose: Purpose) {
        if self.framed_pid == Some(0) {
            self.complete(&purpose, b"", -1);
            return;
        }
        self.send(Command::FramerRun(command.to_string()), Handler::RunRemote(purpose), false);
    }

    /// The pids of the running background commands.
    pub fn background_pids(&self) -> Vec<i32> {
        self.background_jobs.keys().copied().collect()
    }

    /// atPasswordPrompt (`CD:2700`).
    pub fn at_password_prompt(&self) -> bool {
        self.tty.at_password_prompt()
    }

    /// addBackgroundJob (`CR:51-55`) for a `run` reply line (`CD:2259-2271`): only an integer
    /// is a pid. The framer's own pseudo pids are never a job, and at most `MAX_JOBS` run.
    pub(crate) fn add_background_job(&mut self, line: &str, command: Command, purpose: Purpose) {
        let Ok(pid) = line.parse::<i32>() else { return };
        if pid == AUTOPOLL_PID || pid == NOTIF_PID {
            return;
        }
        if self.background_jobs.len() >= MAX_JOBS && !self.background_jobs.contains_key(&pid) {
            return;
        }
        self.background_jobs.insert(pid, Job { command, purpose, output: Vec::new() });
    }

    /// The completion closures of `exfiltrateUsefulFramerInfo`, the uname command and
    /// `handleGetHostname` (`CD:1912-1917`, `CD:1934-1940`, `CD:2205-2210`).
    fn complete(&mut self, purpose: &Purpose, data: &[u8], status: i32) {
        #[cfg(test)]
        self.completed.push((purpose.clone(), data.to_vec(), status));
        let text = String::from_utf8_lossy(data);
        match purpose {
            Purpose::Uname | Purpose::Home if status == 0 => {
                let value = Some(text.trim().to_string()).filter(|t| t.len() <= VALUE_MAX);
                if *purpose == Purpose::Uname {
                    self.uname = value;
                } else {
                    self.home_directory = value;
                }
                self.effects.push(Effect::StateChanged);
            }
            Purpose::Hostname if status == 0 && !text.is_empty() && text.trim().len() <= VALUE_MAX => {
                self.discovered_hostname = Some(text.trim().to_string());
            }
            _ => {}
        }
    }

    /// handleTTYNotif (`CD:2650-2679`): each word `-name` or `+name` (longer than one
    /// character) sets a flag; anything else is skipped. A repeated flag: the last one wins.
    fn handle_tty_notif(&mut self, message: &str) {
        for word in message.split(' ') {
            if word.chars().count() <= 1 {
                continue;
            }
            let enabled = match word.as_bytes()[0] {
                b'-' => false,
                b'+' => true,
                _ => continue,
            };
            match &word[1..] {
                "echo" => self.tty.echo = enabled,
                "icanon" => self.tty.icanon = enabled,
                _ => {}
            }
        }
    }

    /// handleNotif (`CD:2638-2648`): `%notif tty ` is the tty state; `%notif search ` is 14d's.
    fn handle_notif(&mut self, message: &str) {
        if let Some(rest) = message.strip_prefix("%notif tty ") {
            self.handle_tty_notif(rest);
        }
    }

    /// The framer path after python is found (`CD:2130-2161`). Task 5 completes it.
    pub fn frame(&mut self) {
        self.exec_framer();
    }

    /// execFramer (`CD:2044-2063`): `runpython` with the framer as its program, written on
    /// success; the version is set at once, before any reply.
    pub(crate) fn exec_framer(&mut self) {
        let code = framer_code(self.depth());
        self.send(Command::RunPython(code.clone()), Handler::WriteOnSuccess(code), false);
        self.framer_version = Some(2);
    }

    /// handleTerminate (`CD:2544-2575`): a process of a deeper conductor is the parent's. The
    /// framed shell ending sends the pre-framer `quit` (fire and forget; once, a remote that
    /// repeats the terminate queues no more); a job completes with its output and `rc`.
    pub fn handle_terminate(&mut self, pid: i32, rc: i32, depth: i32) {
        if depth != self.depth() {
            self.via_parent(|parent| parent.handle_terminate(pid, rc, depth));
            return;
        }
        if Some(pid) == self.framed_pid {
            if !self.has_command(&|c| *c == Command::Quit) {
                self.send(Command::Quit, Handler::FireAndForget, false);
            }
        } else if let Some(job) = self.background_jobs.remove(&pid) {
            self.complete(&job.purpose, job.output.concat().as_bytes(), rc);
        }
    }

    /// handleSideChannelOutput (`CD:2582-2636`): routed by depth like the rest. The autopoll
    /// pid is Task 7's; the notif pid is the tty rule; a job keeps channel 1 and any other
    /// channel completes it at once with -2 (it stays and completes again at its end).
    pub fn handle_side(&mut self, text: &str, pid: i32, channel: i32, depth: i32) {
        if depth != self.depth() {
            self.via_parent(|parent| parent.handle_side(text, pid, channel, depth));
            return;
        }
        if pid == AUTOPOLL_PID {
            return;
        }
        if pid == NOTIF_PID {
            self.handle_notif(text);
        }
        let Some(job) = self.background_jobs.get_mut(&pid) else { return };
        if channel == 1 {
            let kept: usize = job.output.iter().map(String::len).sum();
            let room = JOB_OUTPUT_MAX.saturating_sub(kept);
            let mut end = text.len().min(room);
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            if end > 0 {
                job.output.push(text[..end].to_string());
            }
        } else {
            let purpose = job.purpose.clone();
            self.complete(&purpose, b"", -2);
        }
    }

    /// handleIT2 (`CD:2593-2600`): a `%it2` frame is kept by the conductor that owns its
    /// depth, the newest 16, each cut at 64 KiB (F30). It raises no prompt and writes nothing
    /// (F29): no proxy reads it.
    pub fn handle_it2(&mut self, text: &str, depth: i32) {
        if depth != self.depth() {
            self.via_parent(|parent| parent.handle_it2(text, depth));
            return;
        }
        let mut end = text.len().min(IT2_ENTRY_MAX);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        self.it2_inbox.push(text[..end].to_string());
        if self.it2_inbox.len() > IT2_INBOX_MAX {
            self.it2_inbox.remove(0);
        }
    }

    /// The framer protocol version, None until the framer is launched.
    pub fn framer_version(&self) -> Option<u8> {
        self.framer_version
    }
}

#[cfg(test)]
mod tests {
    use super::{Job, Purpose, TtyState, IT2_ENTRY_MAX};
    use crate::conductor_parser::{AUTOPOLL_PID, NOTIF_PID};
    use crate::ssh_fixtures::{framer_cmd, pre_cmd};
    use crate::conductor::{python_status, Command, Conductor, Effect, Handler, Setup, State};
    use crate::conductor_parser::Token;
    use crate::framer_script::framer_code;

    fn feed(c: &mut Conductor, tokens: &[Token]) {
        for t in tokens {
            match t {
                Token::Begin(id) => c.handle_begin(id, 0),
                Token::Line(l) => c.handle_line(l, 0),
                Token::End { id, status, kind } => c.handle_end(id, kind, *status, 0),
                _ => unreachable!(),
            }
        }
    }

    fn end(id: &str, status: u8, kind: &str) -> Token {
        Token::End { id: id.into(), status, kind: kind.into() }
    }

    /// A conductor driven through the recorded getshell reply to the pythonversion step
    /// (F31), its writes taken; the recorded pythonversion reply then goes in.
    fn at_runpython() -> Conductor {
        let mut c = Conductor::new(Setup { client_unique_id: "A".into(), ..Setup::default() }, None, "P");
        c.start();
        feed(&mut c, &[Token::Begin("1.1".into()), Token::Line("/bin/sh".into()), Token::Line("\n/home/u".into()), end("1.1", 0, "r")]);
        assert_eq!(c.in_flight(), Some(&Command::PythonVersion));
        c.take_effects();
        feed(&mut c, &[Token::Begin("2.1".into()), Token::Line("Python 3.11.4".into()), end("2.1", 0, "r")]);
        c
    }

    fn job(purpose: Purpose) -> Job {
        Job { command: Command::FramerRun("x".into()), purpose, output: Vec::new() }
    }

    /// A framing conductor (framed pid 100, depth 0, line prefix "\u{3}P").
    fn framing() -> Conductor {
        let mut c = Conductor::new(Setup { client_unique_id: "A".into(), ..Setup::default() }, None, "P");
        c.framed_pid = Some(100);
        c
    }

    /// `run <command>` sent and answered with `pid` (`F:1475-1491`): the job exists after it.
    fn start_job(c: &mut Conductor, command: &str, purpose: Purpose, pid: &str) {
        c.run_remote_command(command, purpose);
        c.take_effects();
        feed(c, &[Token::Begin("a".into()), Token::Line(pid.into()), end("a", 0, "f")]);
        c.take_effects();
    }

    #[test]
    fn background_jobs_follow_iterm2() {
        let mut c = framing();
        c.run_remote_command("uname -s", Purpose::Uname);
        assert_eq!(c.take_effects(), [Effect::Write(framer_cmd(&["run", "uname -s"]))]);
        feed(&mut c, &[Token::Begin("a".into()), Token::Line("4243".into()), end("a", 0, "f")]);
        assert_eq!(c.background_pids(), [4243]);
        assert_eq!(c.background_jobs[&4243].command, Command::FramerRun("uname -s".into()));
        c.take_effects();
        c.handle_side("Linux\r\n", 4243, 1, 0);
        assert!(c.completed.is_empty() && c.uname.is_none());
        c.handle_terminate(4243, 0, 0);
        assert_eq!(c.uname.as_deref(), Some("Linux"));
        assert_eq!(c.take_effects(), [Effect::StateChanged]);
        assert!(c.background_pids().is_empty());

        // Pieces are joined with nothing; a non-zero status sets nothing.
        start_job(&mut c, "echo $HOME", Purpose::Home, "4300");
        c.handle_side("/ho", 4300, 1, 0);
        c.handle_side("me/u\n", 4300, 1, 0);
        c.handle_terminate(4300, 0, 0);
        assert_eq!(c.home_directory.as_deref(), Some("/home/u"));
        start_job(&mut c, "echo $HOME", Purpose::Home, "4301");
        c.handle_side("/elsewhere", 4301, 1, 0);
        c.handle_terminate(4301, 1, 0);
        assert_eq!(c.home_directory.as_deref(), Some("/home/u"));

        // stderr (any channel but 1) completes the job at once with -2 and it stays; it
        // completes again at its terminate (CD:2371-2380, kept quirk). Uname ignores both.
        let mut c = framing();
        start_job(&mut c, "uname -s", Purpose::Uname, "4244");
        c.handle_side("oops", 4244, 2, 0);
        assert_eq!(c.completed, [(Purpose::Uname, Vec::new(), -2)]);
        assert_eq!(c.background_pids(), [4244]);
        c.handle_side("Linux", 4244, 1, 0);
        c.handle_terminate(4244, 0, 0);
        assert_eq!(c.completed.len(), 2);
        assert_eq!(c.completed[1], (Purpose::Uname, b"Linux".to_vec(), 0));
        assert_eq!(c.uname.as_deref(), Some("Linux"));

        // Hostname: status 0 and non-empty only, trimmed.
        let mut c = framing();
        start_job(&mut c, "hostname -f", Purpose::Hostname, "5000");
        c.handle_side("box.example.com\n", 5000, 1, 0);
        c.handle_terminate(5000, 0, 0);
        assert_eq!(c.discovered_hostname.as_deref(), Some("box.example.com"));
        assert_eq!(c.take_effects(), [], "the hostname does not change the state");
        start_job(&mut c, "hostname -f", Purpose::Hostname, "5001");
        c.handle_terminate(5001, 0, 0);
        start_job(&mut c, "hostname -f", Purpose::Hostname, "5002");
        c.handle_side("other", 5002, 1, 0);
        c.handle_terminate(5002, 1, 0);
        assert_eq!(c.discovered_hostname.as_deref(), Some("box.example.com"));

        // Unknown pids change nothing.
        let mut c = framing();
        c.handle_side("x", 9, 1, 0);
        c.handle_terminate(9, 0, 0);
        assert!(c.completed.is_empty() && c.background_pids().is_empty() && c.take_effects().is_empty());

        // A pid line that is not an integer adds nothing; an abort does nothing.
        start_job(&mut c, "uname -s", Purpose::Uname, "oops");
        start_job(&mut c, "uname -s", Purpose::Uname, "");
        assert!(c.background_pids().is_empty());
        c.run_remote_command("uname -s", Purpose::Uname);
        c.handle_unhook();
        assert!(c.background_pids().is_empty() && c.completed.is_empty());

        // A framed pid of 0 completes at once with -1 and writes nothing (CR:73-76).
        let mut c = framing();
        c.framed_pid = Some(0);
        c.run_remote_command("uname -s", Purpose::Uname);
        assert_eq!(c.completed, [(Purpose::Uname, Vec::new(), -1)]);
        assert_eq!(c.take_effects(), []);
        assert_eq!(c.in_flight(), None);
    }

    #[test]
    fn background_jobs_are_bounded() {
        // The autopoll and notif pids are the framer's own channels, never a job.
        let mut c = framing();
        start_job(&mut c, "x", Purpose::Ignore, &AUTOPOLL_PID.to_string());
        start_job(&mut c, "x", Purpose::Ignore, &NOTIF_PID.to_string());
        assert!(c.background_pids().is_empty());
        // A remote that answers with pid lines forever keeps 256 jobs.
        for pid in 1..=300 {
            start_job(&mut c, "x", Purpose::Ignore, &pid.to_string());
        }
        assert_eq!(c.background_pids().len(), 256);
        // One job's output stops growing at 1 MiB; the job still completes.
        let mut c = framing();
        start_job(&mut c, "x", Purpose::Ignore, "7");
        let piece = "a".repeat(64 * 1024);
        for _ in 0..40 {
            c.handle_side(&piece, 7, 1, 0);
        }
        c.handle_terminate(7, 0, 0);
        assert_eq!(c.completed.len(), 1);
        assert_eq!(c.completed[0].1.len(), 1024 * 1024);
    }

    #[test]
    fn framed_pid_termination_quits() {
        let mut c = framing();
        c.handle_terminate(100, 0, 0);
        assert_eq!(c.take_effects(), [Effect::Write(pre_cmd("\u{3}P", &["quit"]))]);
        // A remote repeating the terminate does not queue a second quit.
        c.handle_terminate(100, 0, 0);
        assert_eq!(c.take_effects(), []);
        assert_eq!(c.in_flight(), Some(&Command::Quit));
    }

    #[test]
    fn tty_notifications_follow_iterm2() {
        let mut c = framing();
        assert_eq!(c.tty, TtyState { echo: true, icanon: true });
        assert!(!c.at_password_prompt());
        c.handle_side("%notif tty -echo +icanon", NOTIF_PID, 1, 0);
        assert!(c.at_password_prompt());
        c.handle_side("%notif tty +echo", NOTIF_PID, 1, 0);
        assert!(!c.at_password_prompt());
        c.handle_side("%notif tty -echo", NOTIF_PID, 1, 0);
        assert!(c.at_password_prompt());
        // -icanon leaves echo as it was and turns the prompt off.
        c.handle_side("%notif tty -icanon", NOTIF_PID, 1, 0);
        assert_eq!(c.tty, TtyState { echo: false, icanon: false });
        assert!(!c.at_password_prompt());
        // Malformed flags are skipped one by one (CD:2650-2679).
        c.handle_side("%notif tty +icanon x +", NOTIF_PID, 1, 0);
        assert_eq!(c.tty, TtyState { echo: false, icanon: true });
        c.handle_side("%notif tty +echo x +", NOTIF_PID, 1, 0);
        assert_eq!(c.tty, TtyState { echo: true, icanon: true });
        c.handle_side("%notif tty -echo x +", NOTIF_PID, 1, 0);
        assert_eq!(c.tty, TtyState { echo: false, icanon: true });
        // A repeated flag does not crash (iTerm2's dictionary would); the last one wins.
        c.handle_side("%notif tty +echo -echo +echo", NOTIF_PID, 1, 0);
        assert!(c.tty.echo);
        // Not a tty notification: search is 14d's, the rest is ignored.
        c.handle_side("%notif tty -echo", NOTIF_PID, 1, 0);
        c.handle_side("%notif search 1 {}", NOTIF_PID, 1, 0);
        c.handle_side("%notif ttyx +echo", NOTIF_PID, 1, 0);
        c.handle_side("%notif", NOTIF_PID, 1, 0);
        assert_eq!(c.tty, TtyState { echo: false, icanon: true });
        assert!(c.completed.is_empty() && c.take_effects().is_empty());
        // Nothing here writes or runs anything, and the autopoll pid is Task 7's.
        c.handle_side("EOF", AUTOPOLL_PID, 1, 0);
        assert_eq!(c.take_effects(), []);
    }

    #[test]
    fn exec_framer_follows_iterm2() {
        let mut c = at_runpython();
        assert_eq!(c.take_effects(), [Effect::Write(b"\x03PcnVucHl0aG9u\n\n".to_vec())]);
        assert_eq!(c.framer_version(), Some(2));
        assert_eq!(c.in_flight(), Some(&Command::RunPython(framer_code(0))));
        feed(&mut c, &[Token::Begin("3.1".into()), Token::Line("31337".into()), end("3.1", 0, "0")]);
        assert_eq!(c.take_effects(), [Effect::Write((framer_code(0) + "\nEOF\n\n").into_bytes())]);
    }

    #[test]
    fn python_failure_follows_iterm2() {
        let mut c = at_runpython();
        c.take_effects();
        feed(&mut c, &[Token::Begin("3.1".into()), Token::Line("31337".into()), end("3.1", 1, "0")]);
        assert_eq!(c.take_effects(), [Effect::SessionWrite(b"exec_login_shell\n".to_vec()), Effect::Abort(python_status(1))]);
        assert_eq!(python_status(1), "Status 1 when running python code");
    }

    /// A framing parent (pid 100, depth 0) with a poll in flight and a framing child (pid 200,
    /// depth 1) with a poll in flight, the child the session's innermost conductor.
    fn nest() -> Conductor {
        let mut parent = Conductor::new(Setup { client_unique_id: "A".into(), ..Setup::default() }, None, "P");
        parent.framed_pid = Some(100);
        parent.send(Command::FramerPoll, Handler::FireAndForget, false);
        parent.send(Command::FramerPoll, Handler::FireAndForget, false);
        let mut child = Conductor::new(Setup { client_unique_id: "B".into(), ..Setup::default() }, Some(Box::new(parent)), "V");
        child.framed_pid = Some(200);
        child.send(Command::FramerPoll, Handler::FireAndForget, false);
        assert_eq!((child.depth(), child.parent.as_ref().unwrap().depth()), (1, 0));
        child.take_effects();
        child
    }

    fn parent_of(c: &Conductor) -> &Conductor {
        c.parent.as_ref().unwrap()
    }

    #[test]
    fn depth_routing_follows_iterm2() {
        // A line of depth 0 reaches the parent's in-flight command, depth 1 the child's.
        let mut c = nest();
        assert!(matches!(parent_of(&c).state(), State::WillExecute(_)) && matches!(c.state(), State::WillExecute(_)));
        c.handle_line("L", 0);
        assert!(matches!(parent_of(&c).state(), State::Executing(..)), "the parent heard it");
        assert!(matches!(c.state(), State::WillExecute(_)), "the child did not");
        c.handle_line("L", 1);
        assert!(matches!(c.state(), State::Executing(..)));
        // An end of depth 0 goes to the parent, which writes its next command; the app drains
        // only the innermost conductor, so the child's take_effects has it.
        c.handle_end("f", "f", 0, 0);
        assert_eq!(parent_of(&c).in_flight(), Some(&Command::FramerPoll), "the parent's second poll");
        assert!(matches!(c.state(), State::Executing(..)), "the child's is still in flight");
        assert!(!parent_of(&c).has_effects());
        assert_eq!(c.take_effects(), [Effect::Write(b"cG9sbA==\n\n".to_vec())]);
        assert_eq!(c.take_effects(), []);
        c.handle_end("f", "f", 0, 1);
        assert_eq!(c.state(), &State::Ground);
        // A non-framing child whose in-flight command is not a framer command passes an end
        // of kind "f" up and handles kind "r" itself (CD:2482-2491).
        let plain = || {
            let mut parent = Conductor::new(Setup { client_unique_id: "A".into(), ..Setup::default() }, None, "P");
            parent.framed_pid = Some(100);
            parent.send(Command::FramerPoll, Handler::FireAndForget, false);
            let mut child = Conductor::new(Setup { client_unique_id: "B".into(), ..Setup::default() }, Some(Box::new(parent)), "V");
            child.send(Command::Cd("/a".into()), Handler::FireAndForget, false);
            child.take_effects();
            child
        };
        let mut c = plain();
        assert!(!c.framing() && c.in_flight() == Some(&Command::Cd("/a".into())));
        c.handle_end("f", "f", 0, 0);
        assert_eq!(parent_of(&c).in_flight(), None, "the parent took the framer end");
        assert_eq!(c.in_flight(), Some(&Command::Cd("/a".into())));
        let mut c = plain();
        c.handle_end("r", "r", 0, 1);
        assert_eq!(c.in_flight(), None, "an r end is the child's own");
        assert_eq!(parent_of(&c).in_flight(), Some(&Command::FramerPoll));
    }

    #[test]
    fn routing_of_the_rest_follows_iterm2() {
        // A job of the parent's (depth 0) and one of the child's (depth 1).
        let mut c = nest();
        c.parent.as_mut().unwrap().background_jobs.insert(7, job(Purpose::Ignore));
        c.background_jobs.insert(8, job(Purpose::Ignore));
        c.handle_side("x", 7, 1, 0);
        assert_eq!(parent_of(&c).background_jobs[&7].output, ["x"]);
        assert!(c.background_jobs[&8].output.is_empty());
        c.handle_side("y", 8, 1, 1);
        assert_eq!(c.background_jobs[&8].output, ["y"]);
        c.handle_terminate(7, 0, 0);
        assert!(parent_of(&c).background_jobs.is_empty() && c.background_pids() == [8]);
        assert_eq!(parent_of(&c).completed, [(Purpose::Ignore, b"x".to_vec(), 0)]);
        c.handle_terminate(8, 3, 1);
        assert_eq!(c.completed, [(Purpose::Ignore, b"y".to_vec(), 3)]);
        assert!(c.background_pids().is_empty());
    }

    #[test]
    fn it2_frames_are_kept_and_never_acted_on() {
        let mut c = nest();
        c.handle_it2("3 open", 0);
        assert_eq!(parent_of(&c).it2_inbox, ["3 open"]);
        assert!(c.it2_inbox.is_empty());
        c.handle_it2("3 open", 1);
        assert_eq!(c.it2_inbox, ["3 open"]);
        // No prompt, no write, no state change (F29): nothing is queued for the app.
        assert_eq!(c.take_effects(), []);
        assert!(matches!(c.state(), State::WillExecute(_)));
        // Twenty entries keep the newest sixteen.
        for n in 0..20 {
            c.handle_it2(&format!("{n} e"), 1);
        }
        assert_eq!(c.it2_inbox.len(), 16);
        assert_eq!((c.it2_inbox[0].as_str(), c.it2_inbox[15].as_str()), ("4 e", "19 e"));
        // An entry is cut at 64 KiB, on a character boundary (F30).
        c.handle_it2(&"\u{e9}".repeat(100_000), 1);
        let last = c.it2_inbox.last().unwrap();
        assert!(last.len() <= IT2_ENTRY_MAX && last.len() > IT2_ENTRY_MAX - 4, "{}", last.len());
        assert!(last.chars().all(|ch| ch == '\u{e9}'));
        assert!(c.it2_inbox.iter().map(String::len).sum::<usize>() <= 16 * IT2_ENTRY_MAX);
    }
}

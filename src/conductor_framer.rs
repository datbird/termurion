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
/// The most output all of one conductor's jobs keep together (16 MiB); a hostile remote
/// could otherwise hold 256 jobs of 1 MiB, and more at each hook depth.
pub(crate) const JOBS_OUTPUT_MAX: usize = 16 * 1024 * 1024;
/// The most a login reply or a getenv value keeps (F13): a pid or a short failure text, a
/// PATH. More fails the login and drops the value.
pub const REPLY_MAX: usize = 16 * 1024;
/// The command whose output is the remote's host name (`CD:244`).
pub const DISCOVER_HOSTNAME: &str = "hostname -f";
/// The message when the remote's login answer is not a pid (this plan's; 14b has no such text).
pub fn invalid_pid(text: &str) -> String {
    format!("Invalid process ID from remote: {text}")
}

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

/// A running background command (`handleBackgroundJob`): its output, channel 1 only. iTerm2
/// keeps the pieces and joins them with nothing at the end; Tandem appends each piece as it
/// comes, so the kept length is known at once (a remote streaming lone newlines sends a
/// piece per byte).
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub command: Command,
    pub purpose: Purpose,
    pub output: String,
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
    /// is a pid. The framer's own pseudo pids are never a job, nor the framed shell's pid (its
    /// terminate is the shell's `quit`, so the job would never complete), and at most
    /// `MAX_JOBS` run.
    pub(crate) fn add_background_job(&mut self, line: &str, command: Command, purpose: Purpose) {
        let Ok(pid) = line.parse::<i32>() else { return };
        if pid == AUTOPOLL_PID || pid == NOTIF_PID || Some(pid) == self.framed_pid {
            return;
        }
        if self.background_jobs.len() >= MAX_JOBS && !self.background_jobs.contains_key(&pid) {
            return;
        }
        if let Some(old) = self.background_jobs.insert(pid, Job { command, purpose, output: String::new() }) {
            self.job_output = self.job_output.saturating_sub(old.output.len());
        }
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

    /// doFraming (`CD:1901-1930`), the framer path after python is found (`CD:2130-2161`).
    /// Every command is queued now and goes out after the previous one's reply. The it2
    /// socket listen is n/a (ruling 3, Tandem never sends `it2listen`). The `save` pairs go in
    /// a fixed order (iTerm2's dictionary order is arbitrary).
    pub fn frame(&mut self) {
        self.exec_framer();
        let pairs = [
            ("dcsID", self.setup.dcs_id.clone()),
            ("sshargs", self.setup.sshargs.clone()),
            ("boolArgs", self.setup.bool_args.clone()),
            ("clientUniqueID", self.setup.client_unique_id.clone()),
        ];
        self.send(Command::FramerSave(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()), Handler::FireAndForget, false);
        let uname = self.uname_command.clone();
        self.run_remote_command(&uname, Purpose::Uname);
        // exfiltrateUsefulFramerInfo (`CD:1932-1940`).
        self.run_remote_command("echo $HOME", Purpose::Home);
        self.send(Command::FramerGetenv("PATH".into()), Handler::Getenv("PATH".into(), None), false);
        // Task 10 adds the jump branch here and in the login handler.
        let cwd = self.setup.initial_directory.clone().unwrap_or_else(|| "$HOME".to_string());
        let args = self.modified_command_args.clone().unwrap_or_else(|| self.setup.parsed.command_args.clone());
        self.send(Command::FramerLogin { cwd, args }, Handler::FramerLogin(Vec::new()), false);
        if self.autopoll_enabled {
            self.send(Command::FramerAutopoll, Handler::FireAndForget, false);
        }
        // handleGetHostname: its pid line adds a Hostname job (`CD:2198-2214`).
        self.send(Command::FramerRun(DISCOVER_HOSTNAME.into()), Handler::RunRemote(Purpose::Hostname), false);
        self.effects.push(Effect::StateChanged);
    }

    /// finalizeFraming (`CD:2398-2411`): the login's answer is the shell's pid, or why there is
    /// none. The pieces join with nothing (StringArray).
    pub(crate) fn finalize_framing(&mut self, status: u8, lines: &[String]) {
        let text = lines.concat();
        if status != 0 {
            self.fail(&text);
            return;
        }
        let Ok(pid) = text.parse::<i32>() else {
            self.fail(&invalid_pid(&text));
            return;
        };
        self.set_framed_pid(Some(pid));
        self.effects.push(Effect::SendInitialText);
        self.effects.push(Effect::StateChanged);
        self.effects.push(Effect::StopQueueingInput);
    }

    /// The framedPID setter (`CD:293-302`): a pid joins the connected hosts.
    pub(crate) fn set_framed_pid(&mut self, pid: Option<i32>) {
        self.framed_pid = pid;
        if pid.is_some() {
            self.effects.push(Effect::Registered { guid: self.guid.clone(), identity: self.identity() });
        }
    }

    /// What `getenv` learned about a remote variable.
    pub fn environment_variable(&self, name: &str) -> Option<&str> {
        self.environment_variables.get(name).map(String::as_str)
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
            self.job_output = self.job_output.saturating_sub(job.output.len());
            self.complete(&job.purpose, job.output.as_bytes(), rc);
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
        let left = JOBS_OUTPUT_MAX.saturating_sub(self.job_output);
        let Some(job) = self.background_jobs.get_mut(&pid) else { return };
        if channel == 1 {
            let room = JOB_OUTPUT_MAX.saturating_sub(job.output.len()).min(left);
            let mut end = text.len().min(room);
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            // Grow by doubling but never past the job's cap, so its allocation stays within
            // what the cap counts.
            let len = job.output.len();
            if job.output.capacity() - len < end {
                job.output.reserve_exact((len + end).next_power_of_two().min(JOB_OUTPUT_MAX) - len);
            }
            job.output.push_str(&text[..end]);
            self.job_output += end;
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
    use super::{invalid_pid, Job, Purpose, TtyState, DISCOVER_HOSTNAME, IT2_ENTRY_MAX};
    use crate::conductor_parser::{AUTOPOLL_PID, NOTIF_PID};
    use crate::ssh_fixtures::{framer_cmd, pre_cmd};
    use crate::conductor::{python_status, Command, Conductor, Effect, Handler, Setup, State};
    use crate::ssh_args::ParsedArgs;
    use std::time::{Duration, Instant};
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
        Job { command: Command::FramerRun("x".into()), purpose, output: String::new() }
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
    fn job_output_has_a_total_per_conductor() {
        let mut c = framing();
        for pid in 1..=20 {
            start_job(&mut c, "x", Purpose::Ignore, &pid.to_string());
        }
        let piece = "a".repeat(1024 * 1024);
        for pid in 1..=20 {
            c.handle_side(&piece, pid, 1, 0);
        }
        let held: usize = c.background_jobs.values().map(|j| j.output.len()).sum();
        assert_eq!(held, super::JOBS_OUTPUT_MAX, "16 of the 20 MiB are kept");
        assert_eq!(c.background_jobs[&17].output.len(), 0, "the jobs past the total keep nothing");
        // No job holds more room than its share: a job at its cap owns exactly the cap.
        assert!(c.background_jobs[&1].output.capacity() <= super::JOB_OUTPUT_MAX);
        // A job that ends gives its share back.
        c.handle_terminate(1, 0, 0);
        c.handle_side("xyz", 17, 1, 0);
        assert_eq!(c.background_jobs[&17].output, "xyz");
        // The totals stay right when a job is replaced by a new one with its pid.
        c.handle_side("q", 2, 1, 0);
        let before: usize = c.background_jobs.values().map(|j| j.output.len()).sum();
        assert!(before <= super::JOBS_OUTPUT_MAX);
    }

    #[test]
    fn job_output_is_counted_as_it_comes() {
        // A remote that streams lone newlines on a job: each is a piece of its own.
        let mut c = framing();
        start_job(&mut c, "x", Purpose::Ignore, "7");
        let n = (1 << 20) + 4096;
        let t0 = Instant::now();
        for _ in 0..n {
            c.handle_side("\n", 7, 1, 0);
        }
        assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());
        c.handle_terminate(7, 0, 0);
        assert_eq!(c.completed[0].1, vec![b'\n'; 1024 * 1024], "1 MiB kept, the rest dropped");
    }

    #[test]
    fn the_framed_pid_is_never_a_job() {
        // A `run` answered with the framed shell's pid would be a job that never completes
        // (its terminate is the shell's `quit`).
        let mut c = framing();
        start_job(&mut c, "x", Purpose::Ignore, "100");
        assert!(c.background_pids().is_empty());
        start_job(&mut c, "x", Purpose::Ignore, "101");
        assert_eq!(c.background_pids(), [101]);
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

    /// A conductor driven through getshell, any cd, and pythonversion to a launched framer
    /// (`tweak` runs first, as 14b's injection would set `modified_command_args`); the
    /// effects the runpython reply causes come back with it.
    fn launched(setup: Setup, tweak: impl FnOnce(&mut Conductor)) -> (Conductor, Vec<Effect>) {
        let mut c = Conductor::new(setup, None, "P");
        tweak(&mut c);
        c.start();
        feed(&mut c, &[Token::Begin("1.1".into()), Token::Line("/bin/sh".into()), Token::Line("\n/home/u".into()), end("1.1", 0, "r")]);
        for _ in 0..4 {
            if c.in_flight() == Some(&Command::PythonVersion) {
                break;
            }
            feed(&mut c, &[Token::Begin("1.2".into()), end("1.2", 0, "r")]);
        }
        assert_eq!(c.in_flight(), Some(&Command::PythonVersion));
        c.take_effects();
        feed(&mut c, &[Token::Begin("2.1".into()), Token::Line("Python 3.11.4".into()), end("2.1", 0, "r")]);
        assert_eq!(c.take_effects(), [Effect::StateChanged, Effect::Write(b"\x03PcnVucHl0aG9u\n\n".to_vec())], "frame ends with a state change");
        feed(&mut c, &[Token::Begin("3.1".into()), Token::Line("31337".into()), end("3.1", 0, "0")]);
        let fx = c.take_effects();
        (c, fx)
    }

    /// The framer's reply to the command in flight; what that causes comes back.
    fn answer(c: &mut Conductor, lines: &[&str], status: u8, kind: &str) -> Vec<Effect> {
        let mut tokens = vec![Token::Begin("a".into())];
        tokens.extend(lines.iter().map(|l| Token::Line(l.to_string())));
        tokens.push(end("a", status, kind));
        feed(c, &tokens);
        c.take_effects()
    }

    fn sequence_setup() -> Setup {
        Setup { dcs_id: "D0".into(), sshargs: "-p 22 h".into(), bool_args: "46".into(), client_unique_id: "99".into(), ..Setup::default() }
    }

    #[test]
    fn framing_sequence_follows_iterm2() {
        let (mut c, fx) = launched(sequence_setup(), |_| {});
        assert_eq!(c.uname_command, "uname -s");
        assert!(c.autopoll_enabled);
        assert_eq!(fx, [Effect::Write((framer_code(0) + "\nEOF\n\n").into_bytes()), Effect::Write(framer_cmd(&["save", "dcsID=D0", "sshargs=-p 22 h", "boolArgs=46", "clientUniqueID=99"]))]);
        // Each command goes out only after the previous one's reply (none pipelines).
        assert_eq!(answer(&mut c, &[], 0, "r"), [Effect::Write(framer_cmd(&["run", "uname -s"]))]);
        assert_eq!(answer(&mut c, &["501"], 0, "r"), [Effect::Write(framer_cmd(&["run", "echo $HOME"]))]);
        assert_eq!(answer(&mut c, &["502"], 0, "r"), [Effect::Write(framer_cmd(&["getenv", "PATH"]))]);
        assert_eq!(c.background_pids(), [501, 502]);
        assert_eq!(answer(&mut c, &["/usr/bin:/bin"], 0, "r"), [Effect::Write(framer_cmd(&["login", "$HOME"]))]);
        assert_eq!(c.environment_variable("PATH"), Some("/usr/bin:/bin"));
        assert!(!c.framing());
        let fx = answer(&mut c, &["4242"], 0, "f");
        assert!(c.framing());
        assert_eq!(c.framed_pid, Some(4242));
        assert_eq!(fx.len(), 5, "{fx:?}");
        assert!(matches!(&fx[0], Effect::Registered { guid, identity } if *guid == c.guid && *identity == c.identity()), "{fx:?}");
        assert_eq!(fx[1..], [Effect::SendInitialText, Effect::StateChanged, Effect::StopQueueingInput, Effect::Write(framer_cmd(&["autopoll"]))]);
        assert_eq!(answer(&mut c, &[], 0, "f"), [Effect::Write(framer_cmd(&["run", "hostname -f"]))]);
        assert_eq!(DISCOVER_HOSTNAME, "hostname -f");
        assert_eq!(answer(&mut c, &["503"], 0, "f"), []);
        c.handle_side("h.example.com\n", 503, 1, 0);
        c.handle_terminate(503, 0, 0);
        assert_eq!(c.discovered_hostname.as_deref(), Some("h.example.com"));
        // The uname and home jobs finish as the remote's shell answers.
        c.handle_side("Linux\n", 501, 1, 0);
        c.handle_terminate(501, 0, 0);
        c.handle_side("/home/u\n", 502, 1, 0);
        c.handle_terminate(502, 0, 0);
        assert_eq!((c.uname.as_deref(), c.home_directory.as_deref()), (Some("Linux"), Some("/home/u")));

        // A configured uname command is the one that runs.
        let (mut c, _) = launched(sequence_setup(), |c| c.uname_command = "uname -a".into());
        assert_eq!(answer(&mut c, &[], 0, "r"), [Effect::Write(framer_cmd(&["run", "uname -a"]))]);
    }

    /// The login command a conductor writes for `setup` (and 14b's modified arguments).
    fn login_written(setup: Setup, modified: Option<Vec<String>>) -> Vec<u8> {
        let (mut c, _) = launched(setup, |c| c.modified_command_args = modified);
        answer(&mut c, &[], 0, "r");
        answer(&mut c, &["1"], 0, "r");
        answer(&mut c, &["2"], 0, "r");
        let mut fx = answer(&mut c, &[""], 0, "r");
        assert_eq!(fx.len(), 1, "{fx:?}");
        match fx.remove(0) {
            Effect::Write(bytes) => bytes,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_login_names_its_directory_and_arguments() {
        let parsed = ParsedArgs { command_args: vec!["top".into(), "-d".into(), "1".into()], ..ParsedArgs::default() };
        let with = |dir: Option<&str>| Setup { initial_directory: dir.map(String::from), parsed: parsed.clone(), ..sequence_setup() };
        assert_eq!(login_written(with(Some("/srv")), None), framer_cmd(&["login", "/srv", "top", "-d", "1"]));
        assert_eq!(login_written(with(None), None), framer_cmd(&["login", "$HOME", "top", "-d", "1"]));
        // 14b's modified arguments replace the parsed ones (`CD:1924-1925`).
        let modified = Some(vec!["-l".to_string()]);
        assert_eq!(login_written(with(Some("/srv")), modified), framer_cmd(&["login", "/srv", "-l"]));
        assert_eq!(login_written(sequence_setup(), Some(Vec::new())), framer_cmd(&["login", "$HOME"]));
    }

    #[test]
    fn login_failure_follows_iterm2() {
        let at_login = || {
            let (mut c, _) = launched(sequence_setup(), |_| {});
            for lines in [&[][..], &["1"], &["2"], &["/bin"]] {
                answer(&mut c, lines, 0, "r");
            }
            assert_eq!(c.in_flight(), Some(&Command::FramerLogin { cwd: "$HOME".into(), args: Vec::new() }));
            c
        };
        let text = "Command failed: [Errno 2] No such file or directory: '/bin/nosh'";
        let mut c = at_login();
        let fx = answer(&mut c, &[text], 1, "r");
        assert_eq!(fx, [Effect::SessionWrite(b"exec_login_shell\n".to_vec()), Effect::Abort(text.into())]);
        assert!(!c.framing());
        // Pieces of the message join with nothing (StringArray.string).
        let mut c = at_login();
        let fx = answer(&mut c, &["Command failed: ", "boom"], 1, "r");
        assert_eq!(fx.last(), Some(&Effect::Abort("Command failed: boom".into())));

        let mut c = at_login();
        let fx = answer(&mut c, &["abc"], 0, "f");
        assert_eq!(fx, [Effect::SessionWrite(b"exec_login_shell\n".to_vec()), Effect::Abort("Invalid process ID from remote: abc".into())]);
        assert_eq!(invalid_pid("abc"), "Invalid process ID from remote: abc");
        assert!(!c.framing() && c.framed_pid.is_none());
        // Nothing registered, and no pid line at all is not a pid either.
        let mut c = at_login();
        let fx = answer(&mut c, &[], 0, "f");
        assert!(!fx.iter().any(|e| matches!(e, Effect::Registered { .. })) && c.framed_pid.is_none(), "{fx:?}");
    }

    #[test]
    fn remote_replies_are_bounded() {
        // A hostile login reply cannot grow the message without end.
        let (mut c, _) = launched(sequence_setup(), |_| {});
        for lines in [&[][..], &["1"], &["2"], &["/bin"]] {
            answer(&mut c, lines, 0, "r");
        }
        let piece = "x".repeat(64 * 1024);
        let many: Vec<&str> = (0..40).map(|_| piece.as_str()).collect();
        let fx = answer(&mut c, &many, 1, "r");
        let Some(Effect::Abort(reason)) = fx.last() else { panic!("{fx:?}") };
        assert!(reason.len() <= super::REPLY_MAX, "{}", reason.len());
        // A getenv value is kept only when it is a sane size.
        let (mut c, _) = launched(sequence_setup(), |_| {});
        for lines in [&[][..], &["1"], &["2"]] {
            answer(&mut c, lines, 0, "r");
        }
        answer(&mut c, &[piece.as_str(), piece.as_str()], 0, "r");
        assert_eq!(c.environment_variable("PATH"), None);
        // Only the first line is the value: an oversize first line gives nothing, never the
        // next line (`F:744-750` answers one line).
        let (mut c, _) = launched(sequence_setup(), |_| {});
        for lines in [&[][..], &["1"], &["2"]] {
            answer(&mut c, lines, 0, "r");
        }
        answer(&mut c, &[piece.as_str(), "/bin"], 0, "r");
        assert_eq!(c.environment_variable("PATH"), None);
    }

    #[test]
    fn the_login_reply_is_capped_at_64_lines() {
        let (mut c, _) = launched(sequence_setup(), |_| {});
        for lines in [&[][..], &["1"], &["2"], &["/bin"]] {
            answer(&mut c, lines, 0, "r");
        }
        // 64 short lines still fit; the 65th fails the step.
        let lines: Vec<&str> = vec!["y"; 64];
        let fx = answer(&mut c, &lines, 1, "r");
        let Some(Effect::Abort(reason)) = fx.last() else { panic!("{fx:?}") };
        assert!(!reason.contains("too long"), "{reason}");
        let (mut c, _) = launched(sequence_setup(), |_| {});
        for lines in [&[][..], &["1"], &["2"], &["/bin"]] {
            answer(&mut c, lines, 0, "r");
        }
        let lines: Vec<&str> = vec!["y"; 65];
        let fx = answer(&mut c, &lines, 1, "r");
        let Some(Effect::Abort(reason)) = fx.last() else { panic!("{fx:?}") };
        assert!(reason.contains("too long"), "{reason}");
    }

    #[test]
    fn deregistration_follows_iterm2() {
        let mut c = framing();
        c.fail("x");
        let fx = c.take_effects();
        assert!(matches!(&fx[0], Effect::Deregistered { guid } if *guid == c.guid), "{fx:?}");
        assert_eq!(fx[1..], [Effect::SessionWrite(b"exec_login_shell\n".to_vec()), Effect::Abort("x".into())]);
        // A conductor that never framed was never registered.
        let mut c = Conductor::new(Setup::default(), None, "P");
        c.fail("x");
        assert_eq!(c.take_effects(), [Effect::SessionWrite(b"exec_login_shell\n".to_vec()), Effect::Abort("x".into())]);
        // Each conductor has its own guid.
        assert_ne!(Conductor::new(Setup::default(), None, "P").guid, Conductor::new(Setup::default(), None, "P").guid);
    }

    #[test]
    fn framed_timeout_follows_the_rulings() {
        // Ruling M14-I2, with F19: commands written after the framed pid is set are untimed.
        let t0 = Instant::now();
        let mut c = framing();
        c.fake_now = Some(t0);
        c.send(Command::FramerGetenv("PATH".into()), Handler::Getenv("PATH".into(), None), false);
        c.take_effects();
        assert_eq!(c.deadline(), None);
        c.tick(t0 + Duration::from_secs(31));
        assert!(c.take_effects().is_empty() && c.framing());
        // The framed shell's output changes nothing about the clock.
        c.handle_side("out", 100, 1, 0);
        assert_eq!(c.deadline(), None);
        feed(&mut c, &[Token::Begin("a".into()), Token::Line("/usr/bin".into()), end("a", 0, "r")]);
        assert_eq!(c.environment_variable("PATH"), Some("/usr/bin"));
        // The same for a run; its reply still completes it.
        c.fake_now = Some(t0 + Duration::from_secs(40));
        c.run_remote_command("uname -s", Purpose::Uname);
        c.take_effects();
        assert_eq!(c.deadline(), None);
        c.tick(t0 + Duration::from_secs(80));
        assert!(c.take_effects().is_empty() && c.framing());
        feed(&mut c, &[Token::Begin("b".into()), Token::Line("601".into()), end("b", 0, "r")]);
        assert_eq!(c.background_pids(), [601]);
        // Only a command that says so is timed once framed: none of 14c's does.
        for cmd in [Command::FramerRun("x".into()), Command::FramerGetenv("x".into()), Command::FramerAutopoll, Command::FramerSave(Vec::new())] {
            assert!(!cmd.timed_when_framed(), "{cmd:?}");
        }
    }

    #[test]
    fn exec_framer_follows_iterm2() {
        let mut c = at_runpython();
        assert_eq!(c.take_effects(), [Effect::StateChanged, Effect::Write(b"\x03PcnVucHl0aG9u\n\n".to_vec())]);
        assert_eq!(c.framer_version(), Some(2));
        assert_eq!(c.in_flight(), Some(&Command::RunPython(framer_code(0))));
        feed(&mut c, &[Token::Begin("3.1".into()), Token::Line("31337".into()), end("3.1", 0, "0")]);
        // The framer's code, then the first command of the framing sequence.
        assert_eq!(
            c.take_effects(),
            [Effect::Write((framer_code(0) + "\nEOF\n\n").into_bytes()), Effect::Write(framer_cmd(&["save", "dcsID=", "sshargs=", "boolArgs=", "clientUniqueID=A"]))]
        );
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
        // The parent took the framer end: its poll is done and the child's write, which waited
        // behind it as a send, is now in flight.
        assert!(matches!(parent_of(&c).in_flight(), Some(Command::FramerSend { pid: 100, .. })), "{:?}", parent_of(&c).in_flight());
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
        assert_eq!(parent_of(&c).background_jobs[&7].output, "x");
        assert!(c.background_jobs[&8].output.is_empty());
        c.handle_side("y", 8, 1, 1);
        assert_eq!(c.background_jobs[&8].output, "y");
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

    /// A framing conductor for pid 4242, depth 0, ground state.
    fn framing_4242() -> Conductor {
        let mut c = framing();
        c.framed_pid = Some(4242);
        c
    }

    fn send_of(data: &[u8]) -> Command {
        Command::FramerSend { data: data.to_vec(), pid: 4242 }
    }

    #[test]
    fn framed_keys_follow_iterm2() {
        let mut c = framing_4242();
        assert!(c.handles_keystrokes());
        c.send_keys(b"ls\r");
        assert_eq!(c.take_effects(), [Effect::Write(b"c2VuZA==\nNDI0Mg==\nYkhNTg==\n\n".to_vec())]);
        // A second one before any reply goes out at once (the pipeline is topped up).
        c.send_keys(b"x");
        assert_eq!(c.take_effects(), [Effect::Write(framer_cmd(&["send", "4242", "eA=="]))]);
        assert_eq!(c.pending(), [&send_of(b"ls\r"), &send_of(b"x")]);
        // Nothing to send sends nothing (F18: the login's empty connecting queue).
        c.send_keys(b"");
        assert!(c.take_effects().is_empty() && c.pending().len() == 2);
        feed(&mut c, &[end("a", 0, "f"), end("b", 0, "f")]);
        assert!(c.pending().is_empty());

        // A run does not pipeline: keys wait for its end.
        c.send(Command::FramerRun("x".into()), Handler::FireAndForget, false);
        c.take_effects();
        c.send_keys(b"y");
        assert!(c.take_effects().is_empty(), "written only after the run ends");
        assert_eq!(c.pending(), [&Command::FramerRun("x".into()), &send_of(b"y")]);
        feed(&mut c, &[end("a", 0, "f")]);
        assert_eq!(c.take_effects(), [Effect::Write(framer_cmd(&["send", "4242", "eQ=="]))]);

        // Not framing (a level 1 conductor that unhooked): the session writes it raw.
        let mut plain = Conductor::new(Setup::default(), None, "P");
        plain.handle_unhook();
        assert!(!plain.handles_keystrokes());
        plain.send_keys(b"ls\r");
        assert_eq!(plain.take_effects(), [Effect::Keys(b"ls\r".to_vec())]);
        // Framing but unhooked: not routed here.
        let mut gone = framing_4242();
        gone.handle_unhook();
        assert!(!gone.handles_keystrokes());
    }

    #[test]
    fn nested_writes_follow_iterm2() {
        // A child under a framing parent: everything it writes is the parent's send of those
        // bytes, never a raw write into the outer framer's stdin.
        let parent = framing_4242();
        let mut child = Conductor::new(Setup { client_unique_id: "B".into(), ..Setup::default() }, Some(Box::new(parent)), "V");
        child.start();
        let prefix_line = format!("{}\n", child.line_prefix());
        let got = child.take_effects();
        let first = crate::conductor::encode(&send_of(prefix_line.as_bytes()), "");
        assert_eq!(got[0], Effect::Write((first + "\n").into_bytes()), "the prefix line rides in a send");
        // The getshell command carries the child's prefix inside its own send.
        let getshell = crate::conductor::encode(&Command::GetShell, child.line_prefix()) + "\n";
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(got[1], Effect::Write((crate::conductor::encode(&send_of(getshell.as_bytes()), "") + "\n").into_bytes()));
        assert_eq!(child.parent.as_ref().unwrap().pending(), [&send_of(prefix_line.as_bytes()), &send_of(getshell.as_bytes())]);
        // The launch write of Task 1 is one of them too: the code and the framer's EOF line.
        let parent = framing_4242();
        let mut child = Conductor::new(Setup { client_unique_id: "B".into(), ..Setup::default() }, Some(Box::new(parent)), "V");
        child.exec_framer();
        child.take_effects();
        child.handle_begin("q", 1);
        child.handle_end("q", "r", 0, 1);
        let code = framer_code(child.depth()) + "\nEOF\n\n";
        let want = crate::conductor::encode(&send_of(code.as_bytes()), "") + "\n";
        assert_eq!(child.take_effects(), [Effect::Write(want.into_bytes())]);
    }

}

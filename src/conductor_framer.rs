//! The framer half of the Conductor (`CR`, milestone 14c): iTerm2 splits
//! `Conductor+SSHCommandRunning.swift` from `Conductor.swift` the same way.

use crate::conductor::{Command, Conductor, Effect, Handler, RecoveryState, Setup, State};
use crate::ssh_recovery::{Finished, RecoveryInfo};
use crate::conductor_parser::{AUTOPOLL_PID, NOTIF_PID};
use crate::framer_script::framer_code;
use crate::ssh_jumps::Reconnection;

/// The most autopoll text kept between EOF pieces (the parser's own cap for one block).
const AUTOPOLL_TEXT_MAX: usize = crate::conductor_parser::MAX_AUTOPOLL;
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
/// The banner conductorQuit posts (`PS:25603-25612`); `identity` is the identity's
/// `description`. It lives here, with the effect that asks for it, because the pane applies it.
pub fn disconnected_banner(identity: &str) -> String {
    format!("Disconnected from {identity}")
}

/// The command whose output is the remote's host name (`CD:244`).
pub const DISCOVER_HOSTNAME: &str = "hostname -f";
/// The message when the remote's login answer is not a pid (this plan's; 14b has no such text).
pub fn invalid_pid(text: &str) -> String {
    format!("Invalid process ID from remote: {text}")
}

/// The longest uname, home directory or host name kept (a path's limit); a longer value is
/// treated as unusable.
const VALUE_MAX: usize = 4096;

/// The error a clock fetch gets when its connection goes before the reply.
const LOST: &str = "Failed to fetch time: the connection ended";

/// What a background command's result is for (`CR` completion closures), and the clock's
/// time offset: when its fetch began on the monotonic clock (`ssh_clock::monotonic`) and the
/// script call that asked, if any.
#[derive(Clone, Debug, PartialEq)]
pub enum Purpose {
    Uname,
    Home,
    Hostname,
    Ignore,
    TimeOffset { started: f64, token: Option<u64> },
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
        self.exfiltrate_useful_framer_info();
        if self.my_jump.is_some() {
            // framerJump (`CD:1970-1972`): the jump script replaces the login.
            let script = self.jump_script();
            self.send(Command::FramerEval(script), Handler::Jump(Vec::new()), false);
        } else {
            let cwd = self.setup.initial_directory.clone().unwrap_or_else(|| "$HOME".to_string());
            let args = self.modified_command_args.clone().unwrap_or_else(|| self.setup.parsed.command_args.clone());
            self.send(Command::FramerLogin { cwd, args }, Handler::FramerLogin(Vec::new()), false);
        }
        if self.autopoll_enabled {
            self.send(Command::FramerAutopoll, Handler::FireAndForget, false);
        }
        // handleGetHostname: its pid line adds a Hostname job (`CD:2198-2214`).
        self.send(Command::FramerRun(DISCOVER_HOSTNAME.into()), Handler::RunRemote(Purpose::Hostname), false);
        self.effects.push(Effect::StateChanged);
    }

    /// exfiltrateUsefulFramerInfo (`CD:1932-1940`): the remote's home and PATH.
    fn exfiltrate_useful_framer_info(&mut self) {
        self.run_remote_command("echo $HOME", Purpose::Home);
        self.send(Command::FramerGetenv("PATH".into()), Handler::Getenv("PATH".into(), None), false);
    }

    /// jumpScript (`CD:1754-1770`): the script for `my_jump`, which it uses up. The hop's
    /// arguments are text an earlier hop reported and the script runs on that same host.
    fn jump_script(&mut self) -> String {
        let jump = self.my_jump.take();
        crate::ssh_jumps::jump_script(crate::ssh_jumps::IT2SSH, jump.as_ref().map_or("", |j| j.sshargs.as_str()))
    }

    /// jumpWithEval (`CD:1772-1774`): a host without a usable Python evals the script through
    /// conductor.sh, fire and forget.
    pub(crate) fn jump_with_eval(&mut self) {
        use base64::Engine;
        let script = self.jump_script();
        let code = base64::engine::general_purpose::STANDARD.encode(script.as_bytes());
        self.send(Command::Eval(code), Handler::FireAndForget, false);
    }

    /// childDidBeginJumping (`CD:1776-1778`): the next hop's conductor took this one's jumps.
    pub fn child_did_begin_jumping(&mut self) {
        self.my_jump = None;
    }

    /// reconnectionInfo (`CD:737-741`): the current directory wins over the initial one.
    pub fn reconnection_info(&self) -> Reconnection {
        Reconnection {
            sshargs: self.setup.sshargs.clone(),
            initial_directory: self.current_directory.clone().or_else(|| self.setup.initial_directory.clone()),
            boolargs: self.setup.bool_args.clone(),
        }
    }

    /// The hops from the outermost to this conductor (`PS:2956-2966`).
    pub fn sequence(&self) -> Vec<Reconnection> {
        let mut out = self.parent.as_ref().map(|p| p.sequence()).unwrap_or_default();
        out.push(self.reconnection_info());
        out
    }

    /// Whether the chain's first hook was vouched for by Tandem's own origin token. Only then
    /// is the first hop's ssh text something the person typed; a hook any program can print
    /// says nothing about what to run locally.
    pub fn root_is_local(&self) -> bool {
        self.parent.as_ref().map_or(self.setup.local_origin, |p| p.root_is_local())
    }

    /// The host a split of this session connects to (`PS:2947-2952`), when it may go into a
    /// local command line. Beyond iTerm2 (no check there): only when Tandem's own token vouched
    /// for THIS conductor's hook (ruling). The chain's first hook vouching is not enough: a
    /// nested hook is text the hop above printed and could name any user, port or host. And
    /// only names `identity_is_safe` takes.
    pub fn split_identity(&self) -> Option<crate::ssh_args::Identity> {
        Some(self.identity()).filter(|_| self.setup.local_origin).filter(crate::ssh_jumps::identity_is_safe)
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

    /// quit (`CD:1822-1843`), Shell > ssh > Disconnect: queued and in-flight work is canceled,
    /// the state is ground, and the PRE-framer `quit` is sent, so it carries the line prefix
    /// (`CD:1837`, `CD:2871-2879`; a restored conductor has none). Then the conductor leaves
    /// the connected hosts and tells the pane (which posts the banner and unhooks it).
    pub fn quit(&mut self) {
        self.cancel_all();
        self.send(Command::Quit, Handler::FireAndForget, false);
        self.effects.push(Effect::Deregistered { guid: self.guid.clone() });
        self.effects.push(Effect::Quit);
        self.effects.push(Effect::StateChanged);
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
    /// pid feeds the process list; the notif pid is the tty rule; a job keeps channel 1 and any other
    /// channel completes it at once with -2 (it stays and completes again at its end).
    pub fn handle_side(&mut self, text: &str, pid: i32, channel: i32, depth: i32) {
        if depth != self.depth() {
            self.via_parent(|parent| parent.handle_side(text, pid, channel, depth));
            return;
        }
        if pid == AUTOPOLL_PID {
            // "EOF" hands the text to the process list and re-arms the poll; any other piece
            // joins the text (`CD:2600-2611`), up to the cap. Tandem-side guard: the poll is
            // re-armed only when none is queued or in flight (iTerm2 adds one per EOF, so a
            // remote that never ends the command grows the queue until the timeout).
            if text == "EOF" {
                let polled = std::mem::take(&mut self.autopoll);
                let changed = self.processes().is_some_and(|p| p.handle(&polled));
                if !self.has_command(&|c| *c == Command::FramerAutopoll) {
                    self.send(Command::FramerAutopoll, Handler::FireAndForget, false);
                }
                if changed {
                    self.effects.push(Effect::ProcessesChanged);
                }
            } else {
                let mut end = text.len().min(AUTOPOLL_TEXT_MAX.saturating_sub(self.autopoll.len()));
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                self.autopoll.push_str(&text[..end]);
            }
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

    /// The remote process list (`CD:461-470`): made at first use once the shell is framed,
    /// it registers the framed pid with the framer (fire and forget) and tracks it. None
    /// while not framing.
    pub fn processes(&mut self) -> Option<&mut crate::ssh_procs::Provider> {
        // None until a recovered conductor has resynchronized (`CD:721`).
        if self.waiting_to_resync {
            return None;
        }
        let pid = self.framed_pid?;
        if self.procs.is_none() {
            let mut provider = crate::ssh_procs::Provider::new(pid);
            provider.track(pid);
            self.procs = Some(provider);
            self.send(Command::FramerRegister(pid), Handler::FireAndForget, false);
        }
        self.procs.as_mut()
    }

    /// transitiveProcesses (`CD:684-694`): the names of this conductor's shell's descendants
    /// then its parent's, as the remotes spell them. A conductor that is not framing has
    /// none, and a list that has not arrived gives none (never an error).
    pub fn transitive_process_names(&self) -> Vec<String> {
        let Some(pid) = self.framed_pid else { return Vec::new() };
        let mut names = self.procs.as_ref().map(|p| p.descendant_names(pid)).unwrap_or_default();
        if let Some(parent) = &self.parent {
            names.extend(parent.transitive_process_names());
        }
        names
    }

    /// The job and its ancestors on this conductor's remote, once a process list has arrived.
    pub fn remote_job(&self) -> Option<(Option<crate::proctree::Job>, Vec<String>)> {
        let provider = self.procs.as_ref().filter(|p| !p.rows().is_empty())?;
        Some(provider.job(self.framed_pid?))
    }

    /// Once a second, for every framing conductor in the chain (parents first): make its
    /// process list if it has none and publish its CPU value. True when a value was pushed.
    pub fn tick_processes(&mut self) -> bool {
        let mut pushed = self.parent.as_mut().is_some_and(|p| p.tick_processes());
        if self.framed_pid.is_some() {
            pushed |= self.processes().is_some_and(|p| p.tick());
        }
        pushed
    }

    /// Whether this conductor or a parent frames (has a process list to publish).
    pub fn frames_anywhere(&self) -> bool {
        self.framing() || self.parent.as_ref().is_some_and(|p| p.frames_anywhere())
    }

    /// The innermost conductor that frames: a session shows its remote's values, and while its
    /// newest hop is still setting up, the hop under it.
    pub fn innermost_framing(&self) -> Option<&Conductor> {
        if self.framing() { Some(self) } else { self.parent.as_deref().and_then(Conductor::innermost_framing) }
    }

    /// fetchTimeOffset (`CD:619-649`): the clock script through the framer's `runpy`. Its
    /// answer is an `Effect::TimeOffset` carrying `token`, when a script call asked.
    /// `started` is the fetch's time on the monotonic clock, `local_now` the wall clock the
    /// script subtracts.
    pub fn fetch_time_offset(&mut self, local_now: f64, started: f64, token: Option<u64>) {
        let script = crate::ssh_clock::script(local_now);
        self.send(Command::FramerExecPythonStatements(script), Handler::Ephemeral(Purpose::TimeOffset { started, token }, Vec::new()), false);
    }

    /// The reply of an ephemeral command: the time offset's is parsed with its round trip.
    pub(crate) fn finish_ephemeral(&mut self, purpose: &Purpose, lines: &[String], status: u8) {
        if let Purpose::TimeOffset { started, token } = purpose {
            let latency = crate::ssh_clock::monotonic(self.now()) - started;
            let result = crate::ssh_clock::parse_reply(status == 0, &lines.concat(), latency);
            self.effects.push(Effect::TimeOffset { token: *token, result });
        }
    }

    /// An ephemeral command that will never be answered (an abort, a cancel): the time offset
    /// says so, which resets the Clock and unblocks a script.
    pub(crate) fn time_offset_lost(&mut self, handler: &Handler) {
        if let Handler::Ephemeral(Purpose::TimeOffset { token, .. }, _) = handler {
            self.effects.push(Effect::TimeOffset { token: *token, result: Err(LOST.to_string()) });
        }
    }

    /// The tokens of the script calls this conductor still owes an answer: fetches written or
    /// queued, and answers not yet taken as effects. A pane that drops the conductor answers
    /// them (the conductor's effects are never read again).
    pub fn abandoned_time_tokens(&self) -> Vec<u64> {
        let effects = self.effects.iter().filter_map(|e| match e {
            Effect::TimeOffset { token: Some(t), .. } => Some(*t),
            _ => None,
        });
        let asked = self.written_and_queued().filter_map(|c| match &c.handler {
            Handler::Ephemeral(Purpose::TimeOffset { token: Some(t), .. }, _) => Some(*t),
            _ => None,
        });
        asked.chain(effects).collect()
    }

    /// The framer protocol version, None until the framer is launched.
    pub fn framer_version(&self) -> Option<u8> {
        self.framer_version
    }

    /// The conductor `startRecovery` runs on (`PS:20321-20334`): empty fields, no framed pid,
    /// no line prefix, and an id of its own that no EndSSH can name.
    pub fn recovery_shim(parent: Option<Box<Conductor>>) -> Conductor {
        let setup = Setup { client_unique_id: uuid::Uuid::new_v4().hyphenated().to_string(), ..Setup::default() };
        let mut c = Conductor::new(setup, parent, "");
        c.line_prefix = String::new();
        c
    }

    /// startRecovery (`CD:1789-1794`): "\n", base64("recover") and "\n\n" plus write's newline
    /// ask a running framer for its state; the lines that answer come to `handle_recovery_line`.
    pub fn start_recovery(&mut self) {
        use base64::Engine;
        let recover = base64::engine::general_purpose::STANDARD.encode("recover");
        self.write(&format!("\n{recover}\n\n"));
        self.waiting_to_resync = true;
        self.state = State::Recovery(RecoveryState::Ground);
        // A recovery the remote never finishes fails at the command timeout.
        self.last_activity = Some(self.now());
        self.effects.push(Effect::StateChanged);
    }

    /// The framed pid and dcs id of this conductor and each above it, outermost first (0: not
    /// framing), the parentage a recovery's tree starts with (`CD:2731-2733`, whose own order is
    /// innermost first and so wrong past two conductors).
    fn nesting(&self) -> Vec<(i32, String)> {
        let mut out = self.parent.as_ref().map(|p| p.nesting()).unwrap_or_default();
        out.push((self.framed_pid.unwrap_or(0), self.setup.dcs_id.clone()));
        out
    }

    /// handleRecovery(line:) (`CD:2713-2800`) for one recovery line, the OSC string with its
    /// leading colon. Only a conductor in recovery reads them (Tandem-side: iTerm2's
    /// `:begin-recovery` would put any conductor in it). The block ends at `:end-recovery`:
    /// without a login the conductor quits and gives None; else it has the framed pid and the
    /// finished information comes back for the app to build the recovered conductor from.
    pub fn handle_recovery_line(&mut self, raw: &str) -> Option<Finished> {
        let line = raw.trim_end_matches(['\r', '\n']);
        if !line.starts_with(':') || !matches!(self.state, State::Recovery(_)) {
            return None;
        }
        if line == ":begin-recovery" {
            self.state = State::Recovery(RecoveryState::Building(RecoveryInfo::default()));
            self.last_activity = Some(self.now());
            return None;
        }
        // Background jobs are not restored.
        if line.starts_with(":recovery: process ") {
            return None;
        }
        let State::Recovery(RecoveryState::Building(info)) = &self.state else { return None };
        if line.starts_with(":end-recovery") {
            let Some(finished) = info.finished(self.parent.as_ref().map(|p| p.nesting()).unwrap_or_default()) else {
                self.quit();
                return None;
            };
            self.framed_pid = Some(finished.login);
            self.state = State::Ground;
            self.effects.push(Effect::StateChanged);
            return Some(finished);
        }
        let (key, value) = line.strip_prefix(":recovery: ")?.split_once(' ')?;
        let mut next = info.clone();
        if next.set(key, value) {
            self.state = State::Recovery(RecoveryState::Building(next));
            self.last_activity = Some(self.now());
        }
        None
    }

    /// init(recovery:) (`CD:553-593`): a conductor for a framer found by recovery. It has the
    /// framed pid and the framer's version, no line prefix and nothing else the remote's
    /// lines did not give; it waits to resynchronize. Its arguments are text the remote sent,
    /// so it is never a local origin, and its host is the name as given (no ssh_config).
    pub fn from_recovery(f: &Finished, parent: Option<Box<Conductor>>) -> Conductor {
        let setup = Setup {
            sshargs: f.sshargs.clone(),
            bool_args: f.bool_args.clone(),
            dcs_id: f.dcs_id.clone(),
            client_unique_id: f.client_unique_id.clone(),
            parsed: crate::ssh_args::parse(&f.sshargs, &f.bool_args, &crate::ssh_args::NoConfig),
            ..Setup::default()
        };
        let mut c = Conductor::new(setup, parent, "");
        c.line_prefix = String::new();
        c.framed_pid = Some(f.login);
        c.state = State::Recovered;
        c.framer_version = u8::try_from(f.version).ok();
        c.waiting_to_resync = true;
        c
    }

    /// didResynchronize (`CD:1809-1820`): tokens may have been dropped, so the conductor and its
    /// parents are reset and the framer's home and PATH read again. Nothing waits for the
    /// parser's boundary token here (Decision: Tandem swaps its hook at once).
    pub fn did_resynchronize(&mut self) {
        self.waiting_to_resync = false;
        self.force_ground();
        self.reset_transitively();
        self.exfiltrate_useful_framer_info();
    }

    /// resetTransitively (`CD:3078-3081`): the conductors above are reset first, outermost
    /// first (iTerm2 resets only the parent).
    pub fn reset_transitively(&mut self) {
        if let Some(parent) = self.parent.as_mut() {
            parent.reset_transitively();
        }
        self.reset();
    }

    /// reset (`CR:22-44`) with a fresh code.
    pub fn reset(&mut self) {
        let code = self.reset_code.take().unwrap_or_else(|| uuid::Uuid::new_v4().hyphenated().to_string().to_uppercase());
        self.reset_with(&code);
    }

    /// reset (`CR:22-44`) with `code`: back to ground (this conductor's own; the parents' resets
    /// were sent before it and grounding them again would drop those), `reset2` with the code
    /// (`reset` for framer version 1 or none), then autopoll and the framed pid registered
    /// again. The process list is made here if it was not (it registers the pid itself).
    pub fn reset_with(&mut self, code: &str) {
        self.ground_self();
        if self.framer_version == Some(2) {
            self.send(Command::FramerReset2(code.to_string()), Handler::Reset { code: code.to_string(), lines: Vec::new() }, false);
        } else {
            self.send(Command::FramerReset1, Handler::FireAndForget, false);
        }
        if self.autopoll_enabled {
            self.send(Command::FramerAutopoll, Handler::FireAndForget, false);
            if let Some(pid) = self.framed_pid {
                self.processes();
                self.send(Command::FramerRegister(pid), Handler::FireAndForget, false);
            }
        }
    }
}

/// The most a remote's home directory, shell or uname shows in a variable.
const VARIABLE_MAX: usize = 1024;

/// The session's SSH variables (`PS:25624-25660`) for its current conductor: the level, and
/// the home directory, shell and uname. With no conductor the local ones stand in (a uname of
/// `Windows_NT`); at level 1 they are cleared; at level 2 the conductor's are used, each only
/// when it has one. The remote's values are hostile text and are made safe.
pub fn session_values(c: Option<&Conductor>, local_home: &str, local_shell: &str) -> Vec<(&'static str, String)> {
    let level = crate::conductor::level(c);
    let mut out = vec![("sshIntegrationLevel", level.to_string())];
    let (home, shell, uname) = match (level, c) {
        (0, _) => (Some(local_home.to_string()), Some(local_shell.to_string()), Some(crate::ai_prompts::WINDOWS_UNAME.to_string())),
        (2, Some(c)) => (c.home_directory.clone(), c.shell.clone(), c.uname.clone()),
        _ => (None, None, None),
    };
    for (name, value) in [("homeDirectory", home), ("shell", shell), ("uname", uname)] {
        if let Some(text) = value.map(|v| crate::ssh_procs::clean(&v, VARIABLE_MAX)).filter(|v| !v.is_empty()) {
            out.push((name, text));
        }
    }
    out
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
    fn quit_follows_iterm2() {
        // `CD:1822-1843`: queued and in-flight work is canceled without a stale handler, the
        // state is ground, and the PRE-framer `quit` goes out behind the line prefix.
        let mut c = framing();
        c.send_keys(b"x");
        c.take_effects();
        c.run_remote_command("uname -s", Purpose::Uname);
        assert!(c.written_and_queued().count() >= 2, "one in flight, one behind it");
        c.quit();
        assert_eq!(
            c.take_effects(),
            [
                Effect::Write(pre_cmd("\u{3}P", &["quit"])),
                Effect::Deregistered { guid: c.guid.clone() },
                Effect::Quit,
                Effect::StateChanged,
            ]
        );
        assert_eq!(c.written_and_queued().map(|x| x.command.clone()).collect::<Vec<_>>(), [Command::Quit], "only the quit is left");
        assert!(c.completed.is_empty(), "the Uname handler is not fired");
        assert_eq!(c.take_effects(), []);
        // A restored conductor has no line prefix.
        let mut c = framing();
        c.line_prefix = String::new();
        c.quit();
        assert_eq!(c.take_effects()[0], Effect::Write(pre_cmd("", &["quit"])));
        assert_eq!(pre_cmd("", &["quit"]), b"cXVpdA==\n\n");
    }

    #[test]
    fn cancel_all_tells_every_context_canceled() {
        use crate::conductor::Partial;
        let mut c = framing();
        c.send_keys(b"x");
        c.send_keys(b"y");
        c.run_remote_command("uname -s", Purpose::Uname);
        c.take_effects();
        c.cancel_all();
        assert_eq!(c.written_and_queued().count(), 0);
        assert_eq!(c.state(), &State::Ground);
        let canceled = c.updates.iter().filter(|(_, r, _)| *r == Partial::Canceled).count();
        assert_eq!(canceled, 3, "{:?}", c.updates);
        assert!(c.completed.is_empty());
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
        assert!(c.completed.is_empty() && c.take_effects().is_empty(), "nothing here writes or runs anything");
    }

    /// The recorded autopoll block, fed through a hook that is past its init line, reaches
    /// `handle_side` piece by piece.
    fn poll_pieces(body: &str) -> Vec<(String, i32)> {
        let mut h = crate::ssh_hook::Hook::default();
        h.feed(&crate::ssh_fixtures::dcs_hook("", "A", "YQ==", "Yg=="));
        h.feed(&crate::ssh_fixtures::autopoll("9", body)).into_iter().map(|o| match o {
            crate::ssh_hook::Out::Side { text, pid, .. } => (text, pid),
            other => panic!("{other:?}"),
        }).collect()
    }

    #[test]
    fn autopoll_follows_iterm2() {
        let mut c = Conductor::new(Setup { client_unique_id: "A".into(), ..Setup::default() }, None, "P");
        assert!(c.processes().is_none(), "not framing: no provider");
        assert_eq!(c.take_effects(), []);
        c.framed_pid = Some(4242);
        // The provider is made at first use and registers the framed pid once (`CD:461-470`).
        assert!(c.processes().is_some());
        assert_eq!(c.take_effects(), [Effect::Write(framer_cmd(&["register", "4242"]))]);
        feed(&mut c, &[Token::Begin("a".into()), end("a", 0, "f")]);
        assert!(c.processes().is_some());
        assert_eq!(c.take_effects(), [], "registered once");
        // The text fills the provider only when the "EOF" piece arrives (`CD:2600-2611`), and
        // that re-arms the poll.
        let pieces = poll_pieces("$begin ps\n+4242 1 Ss Mon Sep 27 10:00:00 2026 -zsh\n");
        assert!(pieces.iter().all(|(_, pid)| *pid == AUTOPOLL_PID));
        let eof = pieces.iter().position(|(t, _)| t == "EOF").expect("EOF is a piece of its own");
        assert_eq!(eof, pieces.len() - 2, "the parser's line end follows it");
        for (text, pid) in &pieces[..eof] {
            c.handle_side(text, *pid, 1, 0);
        }
        assert!(c.processes().unwrap().rows().is_empty());
        assert_eq!(c.take_effects(), []);
        c.handle_side("EOF", AUTOPOLL_PID, 1, 0);
        assert_eq!(c.processes().unwrap().rows().len(), 1);
        assert_eq!(c.take_effects(), [Effect::Write(framer_cmd(&["autopoll"])), Effect::ProcessesChanged]);
        assert!(c.autopoll.is_empty(), "the text is taken");
        feed(&mut c, &[Token::Begin("b".into()), end("b", 0, "f")]);
        // The line end after EOF starts the next block's text (iTerm2's quirk; empty lines
        // are ignored when it is read).
        c.handle_side(&pieces[eof + 1].0, AUTOPOLL_PID, 1, 0);
        assert_eq!(c.autopoll, "\n");
        // A second block starts from nothing.
        c.handle_side("$begin cpu\n=5", AUTOPOLL_PID, 1, 0);
        c.handle_side("EOF", AUTOPOLL_PID, 1, 0);
        assert_eq!(c.take_effects(), [Effect::Write(framer_cmd(&["autopoll"]))], "a cpu change alone does not change the jobs");
        assert_eq!(c.processes().unwrap().cpu(), Some(0.05));
        // A hostile block cannot grow the text without bound.
        let chunk = "x".repeat(1 << 20);
        for _ in 0..10 {
            c.handle_side(&chunk, AUTOPOLL_PID, 1, 0);
        }
        assert!(c.autopoll.len() <= super::AUTOPOLL_TEXT_MAX);
        c.handle_side("EOF", AUTOPOLL_PID, 1, 0);
        assert!(c.autopoll.is_empty());
    }

    #[test]
    fn autopoll_rearms_only_once() {
        // A hostile remote sends EOF pieces and never ends the poll command: one autopoll
        // waits at most, never one per EOF.
        let count = |c: &Conductor| c.pending().into_iter().filter(|x| **x == Command::FramerAutopoll).count();
        let mut c = Conductor::new(Setup { client_unique_id: "A".into(), ..Setup::default() }, None, "P");
        c.framed_pid = Some(4242);
        for _ in 0..1000 {
            c.handle_side("EOF", AUTOPOLL_PID, 1, 0);
        }
        assert_eq!(count(&c), 1);
        // Once it ended, the next EOF arms it again (normal operation is unchanged).
        c.cancel_enqueued(&|x| *x == Command::FramerAutopoll);
        while c.in_flight().is_some() {
            feed(&mut c, &[Token::Begin("a".into()), end("a", 0, "f")]);
        }
        assert_eq!(count(&c), 0);
        c.handle_side("EOF", AUTOPOLL_PID, 1, 0);
        assert_eq!(count(&c), 1);
    }

    #[test]
    fn transitive_names_join_the_parents() {
        let row = |pid: i32, ppid: i32, name: &str| format!("+{pid} {ppid} S+ Mon Sep 27 10:00:00 2026 {name}\n");
        let mut parent = Conductor::new(Setup { client_unique_id: "A".into(), ..Setup::default() }, None, "P");
        parent.framed_pid = Some(10);
        parent.processes().unwrap().handle("$begin ps\n+10 1 Ss Mon Sep 27 10:00:00 2026 -bash\n+11 10 S+ Mon Sep 27 10:00:00 2026 ssh\n");
        let mut child = Conductor::new(Setup { client_unique_id: "B".into(), ..Setup::default() }, Some(Box::new(parent)), "Q");
        assert!(child.transitive_process_names().is_empty(), "a conductor that does not frame has none (`CD:685`)");
        child.framed_pid = Some(20);
        child.processes().unwrap().handle(&format!("$begin ps\n+20 1 Ss Mon Sep 27 10:00:00 2026 -zsh\n{}", row(21, 20, "/usr/bin/htop")));
        assert_eq!(child.transitive_process_names(), ["htop", "ssh"], "this conductor's, then its parent's (`CD:684-694`)");
        assert!(child.remote_job().is_some());
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

    fn b64(text: &str) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(text.as_bytes())
    }

    /// A framing conductor whose clock reads `secs` after the monotonic epoch.
    fn at(c: &mut Conductor, secs: u64) -> f64 {
        let now = crate::ssh_clock::epoch() + Duration::from_secs(secs);
        c.fake_now = Some(now);
        crate::ssh_clock::monotonic(now)
    }

    fn offset(secs: f64, tz: &str, zone: Option<i32>) -> crate::ssh_clock::Offset {
        crate::ssh_clock::Offset { offset: secs, tz: tz.into(), zone }
    }

    #[test]
    fn fetch_time_offset_follows_iterm2() {
        let mut c = framing();
        let started = at(&mut c, 100);
        c.fetch_time_offset(1790000000.5, started, None);
        let script = crate::ssh_clock::script(1790000000.5);
        assert_eq!(c.take_effects(), [Effect::Write(framer_cmd(&["runpy", &b64(&script)]))]);
        assert_eq!(c.in_flight(), Some(&Command::FramerExecPythonStatements(script)));
        // The reply arrives two seconds later: the offset is the difference less half of it.
        at(&mut c, 102);
        feed(&mut c, &[Token::Begin("t".into()), Token::Line("12.5\nCDT\n-18000".into()), end("t", 0, "f")]);
        assert_eq!(c.take_effects(), [Effect::TimeOffset { token: None, result: Ok(offset(11.5, "CDT", Some(-18000))) }]);
        // A script call carries its token to the answer.
        let started = at(&mut c, 200);
        c.fetch_time_offset(1.0, started, Some(7));
        c.take_effects();
        at(&mut c, 200);
        feed(&mut c, &[Token::Begin("t".into()), Token::Line("Traceback".into()), end("t", 1, "f")]);
        assert_eq!(c.take_effects(), [Effect::TimeOffset { token: Some(7), result: Err("Failed to fetch time: Traceback".into()) }]);
        // The reply's pieces join with nothing; a reply that is not the script's is invalid.
        c.fetch_time_offset(1.0, started, None);
        c.take_effects();
        feed(&mut c, &[Token::Begin("t".into()), Token::Line("5\nU".into()), Token::Line("TC".into()), end("t", 0, "f")]);
        assert!(matches!(&c.take_effects()[..], [Effect::TimeOffset { token: None, result: Ok(o) }] if o.tz == "UTC" && o.zone.is_none()));
        c.fetch_time_offset(1.0, started, None);
        c.take_effects();
        feed(&mut c, &[Token::Begin("t".into()), Token::Line("x".into()), end("t", 0, "f")]);
        assert_eq!(c.take_effects(), [Effect::TimeOffset { token: None, result: Err("Invalid output: x".into()) }]);
    }

    #[test]
    fn a_time_offset_that_never_arrives_answers_its_token() {
        // The connection ends with the fetch in flight: the token still gets an answer.
        let mut c = framing();
        let started = at(&mut c, 1);
        c.fetch_time_offset(1.0, started, Some(9));
        c.take_effects();
        c.fetch_time_offset(1.0, started, Some(10));
        assert_eq!(c.abandoned_time_tokens(), [9, 10], "in flight and queued");
        c.handle_unhook();
        let fx = c.take_effects();
        assert_eq!(fx.len(), 2, "{fx:?}");
        assert!(fx.iter().all(|e| matches!(e, Effect::TimeOffset { token: Some(_), result: Err(_) })), "{fx:?}");
        // Once their effects are taken nothing is left to answer.
        assert!(c.abandoned_time_tokens().is_empty());
        // A canceled fetch is answered too.
        let mut c = framing();
        c.send(Command::FramerRun("x".into()), Handler::FireAndForget, false);
        c.fetch_time_offset(1.0, 0.0, Some(11));
        c.cancel_enqueued(&|cmd| matches!(cmd, Command::FramerExecPythonStatements(_)));
        feed(&mut c, &[Token::Begin("a".into()), end("a", 0, "f")]);
        assert!(matches!(&c.take_effects()[..], [.., Effect::TimeOffset { token: Some(11), result: Err(_) }]));
    }

    #[test]
    fn a_time_offset_reply_is_bounded() {
        // A hostile remote streams lines: only the first 16 KiB are kept, and the reply is
        // invalid rather than the conductor failed.
        let mut c = framing();
        c.fetch_time_offset(1.0, 0.0, None);
        c.take_effects();
        let piece = "y".repeat(1024);
        let mut tokens = vec![Token::Begin("t".into())];
        tokens.extend((0..5000).map(|_| Token::Line(piece.clone())));
        tokens.push(end("t", 0, "f"));
        feed(&mut c, &tokens);
        let fx = c.take_effects();
        assert!(!c.failed(), "{fx:?}");
        let [Effect::TimeOffset { token: None, result: Err(text) }] = &fx[..] else { panic!("{fx:?}") };
        assert!(text.chars().count() < 300, "{}", text.len());
    }

    #[test]
    fn ssh_variables_follow_iterm2() {
        use super::session_values;
        let v = |pairs: &[(&'static str, &str)]| pairs.iter().map(|(k, x)| (*k, x.to_string())).collect::<Vec<_>>();
        assert_eq!(
            session_values(None, r"C:\Users\u", "pwsh"),
            v(&[("sshIntegrationLevel", "0"), ("homeDirectory", r"C:\Users\u"), ("shell", "pwsh"), ("uname", "Windows_NT")])
        );
        // Nothing known locally: only what is known.
        assert_eq!(session_values(None, "", ""), v(&[("sshIntegrationLevel", "0"), ("uname", "Windows_NT")]));
        // Level 1: the three are cleared (`PS:25638-25642`).
        let mut plain = Conductor::new(Setup::default(), None, "P");
        plain.handle_unhook();
        assert_eq!(session_values(Some(&plain), "/h", "zsh"), v(&[("sshIntegrationLevel", "1")]));
        // Level 2 copies them from the conductor.
        let mut c = framing();
        c.home_directory = Some("/home/u".into());
        c.shell = Some("zsh".into());
        c.uname = Some("Linux".into());
        assert_eq!(session_values(Some(&c), "/h", "pwsh"), v(&[("sshIntegrationLevel", "2"), ("homeDirectory", "/home/u"), ("shell", "zsh"), ("uname", "Linux")]));
        let bare = framing();
        assert_eq!(session_values(Some(&bare), "/h", "pwsh"), v(&[("sshIntegrationLevel", "2")]));
    }

    #[test]
    fn remote_variable_values_are_made_safe() {
        use super::session_values;
        let mut c = framing();
        c.home_directory = Some(format!("/home/\u{1b}]0;x\u{7}{}", "a".repeat(9000)));
        c.shell = Some("\u{202e}zsh\n".into());
        c.uname = Some("Li\u{200b}nux".into());
        let values = session_values(Some(&c), "", "");
        let get = |k: &str| values.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone());
        let home = get("homeDirectory").unwrap();
        assert!(home.chars().count() <= 1024 && !home.chars().any(char::is_control), "{}", home.len());
        assert_eq!(get("shell").as_deref(), Some("zsh"));
        assert_eq!(get("uname").as_deref(), Some("Li nux"));
        // A value that is only invisible is left out.
        c.shell = Some("\u{200b}\u{202e}".into());
        assert!(session_values(Some(&c), "", "").iter().all(|(n, _)| *n != "shell"));
    }
    fn reconnection(args: &str, dir: Option<&str>) -> crate::ssh_jumps::Reconnection {
        crate::ssh_jumps::Reconnection { sshargs: args.into(), initial_directory: dir.map(String::from), boolargs: "46".into() }
    }

    /// A jumping conductor driven to the point the framing sequence sends its login step:
    /// getshell answered, python 3.11, the runpython reply, then uname, home and getenv.
    fn jumping_to_login(jumps: Vec<crate::ssh_jumps::Reconnection>) -> (Conductor, Vec<Effect>) {
        let mut c = Conductor::new(sequence_setup(), None, "P");
        c.start_jumping(jumps);
        let started = c.take_effects();
        feed(&mut c, &[Token::Begin("1.1".into()), Token::Line("/bin/sh".into()), Token::Line("\n/home/u".into()), end("1.1", 0, "r")]);
        for _ in 0..4 {
            if c.in_flight() == Some(&Command::PythonVersion) {
                break;
            }
            feed(&mut c, &[Token::Begin("1.2".into()), end("1.2", 0, "r")]);
        }
        c.take_effects();
        feed(&mut c, &[Token::Begin("2.1".into()), Token::Line("Python 3.11.4".into()), end("2.1", 0, "r")]);
        c.take_effects();
        feed(&mut c, &[Token::Begin("3.1".into()), Token::Line("31337".into()), end("3.1", 0, "0")]);
        c.take_effects();
        answer(&mut c, &[], 0, "r");
        answer(&mut c, &["501"], 0, "r");
        answer(&mut c, &["502"], 0, "r");
        (c, started)
    }

    #[test]
    fn jumps_follow_iterm2() {
        use crate::ssh_jumps::{jump_script, IT2SSH};
        use base64::Engine;
        let (a, b) = (reconnection("-p 1 a", Some("/da")), reconnection("-p 2 b", None));
        let mut first = Conductor::new(sequence_setup(), None, "P");
        first.start_jumping(vec![a.clone(), b.clone()]);
        assert_eq!(first.my_jump, Some(a.clone()));
        assert_eq!(first.subsequent_jumps, [b.clone()]);
        let started = first.take_effects();
        assert!(matches!(started.first(), Some(Effect::Write(w)) if w.starts_with(b"\x03P")), "start writes the line prefix first: {started:?}");
        // The framing sequence queues the jump at once and uses the jump up.
        let (mut c, _) = jumping_to_login(vec![a.clone(), b.clone()]);
        assert_eq!(c.my_jump, None);
        // The framing path: the eval of the jump script replaces the login.
        let script = base64::engine::general_purpose::STANDARD.encode(jump_script(IT2SSH, "-p 1 a"));
        let fx = answer(&mut c, &["/usr/bin:/bin"], 0, "r");
        assert_eq!(fx, [Effect::Write(framer_cmd(&["eval", &script]))]);
        assert_eq!(c.subsequent_jumps, [b.clone()], "the rest wait for the next hop");
        // Its reply finalizes framing exactly as a login's.
        let fx = answer(&mut c, &["4300"], 0, "f");
        assert!(c.framing());
        assert_eq!(c.framed_pid, Some(4300));
        assert!(matches!(&fx[0], Effect::Registered { .. }), "{fx:?}");
        assert_eq!(fx[1..], [Effect::SendInitialText, Effect::StateChanged, Effect::StopQueueingInput, Effect::Write(framer_cmd(&["autopoll"]))]);
        // A failing jump fails the conductor with the reply's text.
        let (mut c, _) = jumping_to_login(vec![a.clone()]);
        answer(&mut c, &["/bin"], 0, "r");
        let fx = answer(&mut c, &["Command failed: boom"], 1, "r");
        assert_eq!(fx.last(), Some(&Effect::Abort("Command failed: boom".into())));
        // childDidBeginJumping (`CD:1776-1778`) clears the jump.
        let mut p = Conductor::new(sequence_setup(), None, "P");
        p.start_jumping(vec![a.clone()]);
        p.child_did_begin_jumping();
        assert_eq!(p.my_jump, None);

        // Python below 3.7: the pre-framer eval, fire and forget, in place of the login shell.
        let mut c = Conductor::new(sequence_setup(), None, "P");
        c.start_jumping(vec![a.clone(), b.clone()]);
        c.take_effects();
        feed(&mut c, &[Token::Begin("1.1".into()), Token::Line("/bin/sh".into()), Token::Line("\n/home/u".into()), end("1.1", 0, "r")]);
        for _ in 0..4 {
            if c.in_flight() == Some(&Command::PythonVersion) {
                break;
            }
            feed(&mut c, &[Token::Begin("1.2".into()), end("1.2", 0, "r")]);
        }
        c.take_effects();
        feed(&mut c, &[Token::Begin("2.1".into()), Token::Line("Python 3.6.9".into()), end("2.1", 0, "r")]);
        let eval = format!("eval {}", base64::engine::general_purpose::STANDARD.encode(jump_script(IT2SSH, "-p 1 a")));
        assert_eq!(c.take_effects(), [Effect::Write(pre_cmd("\u{3}P", &[&eval]))]);
        assert_eq!(c.my_jump, None);
        assert!(!c.python_ok);
        // No jump pending: the login shell, as before.
        let mut c = Conductor::new(sequence_setup(), None, "P");
        c.start();
        c.take_effects();
        feed(&mut c, &[Token::Begin("1.1".into()), Token::Line("/bin/sh".into()), Token::Line("\n/home/u".into()), end("1.1", 0, "r")]);
        for _ in 0..4 {
            if c.in_flight() == Some(&Command::PythonVersion) {
                break;
            }
            feed(&mut c, &[Token::Begin("1.2".into()), end("1.2", 0, "r")]);
        }
        c.take_effects();
        feed(&mut c, &[Token::Begin("2.1".into()), Token::Line("Python 3.6.9".into()), end("2.1", 0, "r")]);
        assert_eq!(c.take_effects()[0], Effect::StopQueueingInput);
    }

    #[test]
    fn jump_reply_lines_are_bounded() {
        let (mut c, _) = jumping_to_login(vec![reconnection("a", None)]);
        answer(&mut c, &["/bin"], 0, "r");
        let piece = "x".repeat(64 * 1024);
        let many: Vec<&str> = (0..40).map(|_| piece.as_str()).collect();
        let fx = answer(&mut c, &many, 1, "r");
        let Some(Effect::Abort(reason)) = fx.last() else { panic!("{fx:?}") };
        assert!(reason.len() <= super::REPLY_MAX, "{}", reason.len());
    }

    #[test]
    fn sequence_follows_iterm2() {
        let mut outer = Conductor::new(Setup { sshargs: "h1".into(), bool_args: "b1".into(), initial_directory: Some("/a".into()), client_unique_id: "A".into(), ..Setup::default() }, None, "P");
        outer.framed_pid = Some(10);
        let mut inner = Conductor::new(Setup { sshargs: "h2".into(), bool_args: "b2".into(), initial_directory: Some("/ignored".into()), client_unique_id: "B".into(), ..Setup::default() }, Some(Box::new(outer)), "Q");
        inner.current_directory = Some("/b".into());
        let r = |args: &str, dir: &str, bool_args: &str| crate::ssh_jumps::Reconnection { sshargs: args.into(), initial_directory: Some(dir.into()), boolargs: bool_args.into() };
        assert_eq!(inner.sequence(), [r("h1", "/a", "b1"), r("h2", "/b", "b2")]);
        assert_eq!(inner.reconnection_info(), r("h2", "/b", "b2"));
        // The current directory wins over the initial one; with neither there is none.
        inner.current_directory = None;
        assert_eq!(inner.reconnection_info().initial_directory.as_deref(), Some("/ignored"));
        let bare = Conductor::new(Setup { sshargs: "h".into(), ..Setup::default() }, None, "P");
        assert_eq!(bare.reconnection_info().initial_directory, None);
    }

    #[test]
    fn only_a_local_origin_root_may_reconnect_from_here() {
        let mut root = Conductor::new(Setup { client_unique_id: "A".into(), ..Setup::default() }, None, "P");
        let child = |root: Conductor| Conductor::new(Setup { client_unique_id: "B".into(), ..Setup::default() }, Some(Box::new(root)), "Q");
        assert!(!child(Conductor::new(Setup::default(), None, "P")).root_is_local(), "a hook the local secret did not vouch for");
        root.setup.local_origin = true;
        assert!(root.root_is_local());
        assert!(child(root).root_is_local(), "the chain's first hook decides");
    }

    /// A split runs a LOCAL ssh to the innermost host, so only a hop Tandem's own token vouched
    /// for names it (ruling). A hostile first hop prints a nested hook naming its own user, port
    /// and host: the split must not connect there.
    #[test]
    fn a_split_takes_only_a_vouched_identity() {
        let hook = |sshargs: &str, local: bool, parent: Option<Box<Conductor>>| {
            let init = crate::conductor_parser::Init { token: String::new(), unique_id: "U".into(), bool_args: "46".into(), sshargs: sshargs.into(), dcs_id: "D".into() };
            Conductor::new(crate::conductor::setup_for_hook(&init, local, &[], &[], None, false, &crate::ssh_args::NoConfig), parent, "P")
        };
        let root = hook("-l me -p 22 home.lan ", true, None);
        let id = root.split_identity().expect("the vouched first hop names its host");
        assert_eq!((id.hostname.as_str(), id.username.as_deref()), ("home.lan", Some("me")));
        let mut hop0 = root;
        hop0.framed_pid = Some(10);
        let fake = hook("-l evil -p 2222 attacker.example ", false, Some(Box::new(hop0)));
        assert_eq!(fake.split_identity(), None, "a nested hook the remote printed names no local host");
        assert_eq!(hook("h", false, None).split_identity(), None, "an unvouched first hop either");
    }

    use crate::ssh_recovery::{Finished, Tree};

    fn finished(login: i32, parentage: Vec<(i32, String)>) -> Finished {
        Finished { login, dcs_id: "D0".into(), sshargs: "-p 22 h".into(), bool_args: "46".into(), client_unique_id: "99".into(), version: 2, parentage }
    }

    /// The recovery lines framer.py prints for a login shell (`F:1300-1321`), through the last.
    const LINES: [&str; 9] = [
        ":begin-recovery",
        ":recovery: version 2",
        ":recovery: login 4242",
        ":recovery: dcsID D0",
        ":recovery: sshargs -p 22 h",
        ":recovery: boolArgs 46",
        ":recovery: clientUniqueID 99",
        ":recovery: process 5000",
        ":end-recovery",
    ];

    #[test]
    fn recovery_lines_follow_iterm2() {
        // A shim (empty fields, no parent) starts recovery: "\n" + base64("recover") + "\n\n"
        // plus write's newline (`CD:1789-1794`).
        let mut c = Conductor::recovery_shim(None);
        c.start_recovery();
        assert_eq!(c.take_effects(), [Effect::Write(b"\ncmVjb3Zlcg==\n\n\n".to_vec()), Effect::StateChanged]);
        assert!(c.waiting_to_resync);
        assert!(matches!(c.state(), State::Recovery(_)));
        for line in &LINES[..8] {
            assert_eq!(c.handle_recovery_line(line), None, "{line}");
        }
        assert_eq!(c.handle_recovery_line(LINES[8]), Some(finished(4242, vec![])));
        assert_eq!(c.framed_pid, Some(4242));
        assert_eq!(c.state(), &State::Ground);

        // No login line: the conductor quits (Task 9's effects) and nothing is returned.
        let mut c = Conductor::recovery_shim(None);
        c.start_recovery();
        c.take_effects();
        for line in [":begin-recovery", ":recovery: version 2", ":end-recovery"] {
            assert_eq!(c.handle_recovery_line(line), None);
        }
        assert!(c.take_effects().contains(&Effect::Quit));
        assert_eq!(c.framed_pid, None);

        // A line without the leading colon changes nothing; so does an unknown key, a login that
        // is not a pid and a line before recovery began.
        let mut c = Conductor::recovery_shim(None);
        c.start_recovery();
        assert_eq!(c.handle_recovery_line("begin-recovery"), None);
        assert_eq!(c.state(), &State::Recovery(crate::conductor::RecoveryState::Ground));
        assert_eq!(c.handle_recovery_line(":recovery: login 1"), None);
        assert_eq!(c.state(), &State::Recovery(crate::conductor::RecoveryState::Ground), "the begin line is what opens the block");
        assert_eq!(c.handle_recovery_line(":begin-recovery"), None);
        let building = c.state().clone();
        assert_eq!(c.handle_recovery_line(":recovery: bogus 1"), None);
        assert_eq!(c.handle_recovery_line(":recovery: login -1000"), None);
        assert_eq!(c.handle_recovery_line(":recovery: version x"), None);
        assert_eq!(c.handle_recovery_line(":recovery: login"), None);
        assert_eq!(c.state(), &building);
        // Lines when no recovery runs are ignored.
        let mut c = framing();
        assert_eq!(c.handle_recovery_line(":begin-recovery"), None);
        assert_eq!(c.state(), &State::Ground);
        assert_eq!(c.handle_recovery_line(":end-recovery"), None);

        // The conductors above are the parentage, outermost first (framing ones by their pid).
        let outer = framing();
        let mut shim = Conductor::recovery_shim(Some(Box::new(outer)));
        shim.start_recovery();
        shim.take_effects();
        for line in &LINES[..8] {
            shim.handle_recovery_line(line);
        }
        let f = shim.handle_recovery_line(LINES[8]).expect("finished");
        assert_eq!(f.parentage, [(100, String::new())]);
        assert_eq!(f.tree(), Tree(std::collections::BTreeMap::from([(100, (String::new(), Tree(std::collections::BTreeMap::from([(4242, ("D0".to_string(), Tree::default()))]))))])));
    }

    #[test]
    fn a_recovery_never_ends_by_itself() {
        // Tandem-side guard: a remote that begins recovery and stops is failed by the command
        // clock rather than held forever.
        let mut c = Conductor::recovery_shim(None);
        let t0 = Instant::now();
        c.fake_now = Some(t0);
        c.start_recovery();
        assert_eq!(c.deadline(), Some(t0 + crate::conductor::COMMAND_TIMEOUT));
        c.take_effects();
        c.tick(t0 + crate::conductor::COMMAND_TIMEOUT);
        assert!(c.take_effects().iter().any(|e| matches!(e, Effect::Abort(_))));
    }

    /// Feeds one recorded framer reply to the conductor.
    fn reply_to(c: &mut Conductor, id: &str, lines: &[&str], status: u8) {
        let mut tokens = vec![Token::Begin(id.into())];
        tokens.extend(lines.iter().map(|l| Token::Line(l.to_string())));
        tokens.push(end(id, status, "f"));
        feed(c, &tokens);
    }

    #[test]
    fn resynchronize_follows_iterm2() {
        let f = finished(4242, vec![]);
        let mut c = Conductor::from_recovery(&f, None);
        assert_eq!((c.depth(), c.framed_pid, c.line_prefix(), c.framer_version()), (0, Some(4242), "", Some(2)));
        assert_eq!(c.state(), &State::Recovered);
        assert!(c.waiting_to_resync);
        assert_eq!((c.setup.dcs_id.as_str(), c.setup.sshargs.as_str(), c.setup.bool_args.as_str(), c.setup.client_unique_id.as_str()), ("D0", "-p 22 h", "46", "99"));
        assert_eq!((c.setup.parsed.hostname.as_str(), c.setup.parsed.port), ("h", 22), "parsed from the args");
        assert!(!c.setup.local_origin, "text the remote sent never vouches for a local command");
        // Depth: a framing parent's plus one; a non-framing parent's own.
        assert_eq!(Conductor::from_recovery(&f, Some(Box::new(framing()))).depth(), 1);
        let mut quiet = Conductor::new(Setup { client_unique_id: "B".into(), ..Setup::default() }, Some(Box::new(framing())), "Q");
        quiet.framed_pid = None;
        assert_eq!(quiet.depth(), 1);
        assert_eq!(Conductor::from_recovery(&f, Some(Box::new(quiet))).depth(), 1);

        // Resynchronizing: only the reset goes out at once; autopoll and the two registers
        // (the process list's own and reset's) wait behind it, then the home and PATH.
        c.reset_code = Some("C0DE".into());
        c.did_resynchronize();
        assert!(!c.waiting_to_resync);
        assert_eq!(c.take_effects(), [Effect::Write(framer_cmd(&["reset2", "C0DE"]))]);
        assert_eq!(
            c.pending(),
            [
                &Command::FramerReset2("C0DE".into()),
                &Command::FramerAutopoll,
                &Command::FramerRegister(4242),
                &Command::FramerRegister(4242),
                &Command::FramerRun("echo $HOME".into()),
                &Command::FramerGetenv("PATH".into())
            ]
        );
        // A stale end before the code arrives is ignored: still waiting.
        reply_to(&mut c, "x", &["stale"], 0);
        assert_eq!(c.in_flight(), Some(&Command::FramerReset2("C0DE".into())));
        assert_eq!(c.take_effects(), []);
        // The code arrives: ground, and the rest go out one per end.
        reply_to(&mut c, "y", &["C0DE"], 0);
        assert_eq!(c.take_effects(), [Effect::Write(framer_cmd(&["autopoll"]))]);
        reply_to(&mut c, "a", &[], 0);
        assert_eq!(c.take_effects(), [Effect::Write(framer_cmd(&["register", "4242"]))]);
        reply_to(&mut c, "b", &[], 0);
        assert_eq!(c.take_effects(), [Effect::Write(framer_cmd(&["register", "4242"]))]);
        reply_to(&mut c, "c", &[], 0);
        assert_eq!(c.take_effects(), [Effect::Write(framer_cmd(&["run", "echo $HOME"]))]);
        reply_to(&mut c, "d", &["5100"], 0);
        assert_eq!(c.take_effects(), [Effect::Write(framer_cmd(&["getenv", "PATH"]))]);

        // The parent is reset first: its reset2 is the outer framer's own write, the child's
        // reset rides inside it as a framed send behind.
        let mut outer = framing();
        outer.framer_version = Some(2);
        let mut c = Conductor::from_recovery(&f, Some(Box::new(outer)));
        c.reset_transitively();
        let fx = c.take_effects();
        let Some(Effect::Write(first)) = fx.first() else { panic!("{fx:?}") };
        assert!(first.starts_with(b"cmVzZXQy\n"), "the parent's reset2 goes to the pty first");
        assert_eq!(fx.len(), 1, "the child's reset waits behind it");
        assert!(matches!(c.parent.as_ref().and_then(|p| p.pending().first().copied()), Some(Command::FramerReset2(_))));
        // The reset waits like every framed command (M14-I2): a slept laptop's late answer counts.
        let mut c = Conductor::from_recovery(&f, None);
        let t0 = Instant::now();
        c.fake_now = Some(t0);
        c.did_resynchronize();
        c.take_effects();
        assert_eq!(c.deadline(), None);
    }

    #[test]
    fn conductor_json_follows_iterm2() {
        let mut outer = Conductor::new(Setup { sshargs: "h1".into(), bool_args: "46".into(), dcs_id: "D0".into(), client_unique_id: "A".into(), local_origin: true, ..Setup::default() }, None, "P");
        outer.framed_pid = Some(100);
        outer.home_directory = Some("/home/u".into());
        let mut inner = Conductor::new(Setup { sshargs: "-p 2222 u@h2".into(), bool_args: "46".into(), dcs_id: "D1".into(), client_unique_id: "B".into(), initial_directory: Some("/srv".into()), ..Setup::default() }, Some(Box::new(outer)), "Q");
        inner.setup.parsed = crate::ssh_args::parse("-p 2222 u@h2", "46", &crate::ssh_args::NoConfig);
        inner.framed_pid = Some(200);
        inner.shell = Some("/bin/zsh".into());
        inner.uname = Some("Linux".into());
        inner.framer_version = Some(2);
        // Secrets never reach the state file (F26): environment values, Files to Copy.
        inner.setup.vars_to_send = vec![("TOKEN".into(), "hunter2-secret".into())];
        inner.setup.client_vars = vec![("PASS".into(), "hunter2-secret".into())];
        inner.modified_vars = Some(vec![("K".into(), "hunter2-secret".into())]);
        inner.payloads.push(crate::ssh_inject::Payload { source: crate::ssh_inject::Source::Disk("/x".into()), destination: "/y".into() });
        let json = inner.json();
        assert!(!json.contains("hunter2-secret") && !json.contains("TOKEN") && !json.contains("PASS"), "{json}");
        // It round trips: identity, ids, pids, values, the chain, a fresh guid, no line prefix,
        // a conductor that waits to resynchronize.
        let back = Conductor::from_json(&json).expect("restores");
        assert_eq!((back.setup.sshargs.as_str(), back.setup.dcs_id.as_str(), back.setup.client_unique_id.as_str()), ("-p 2222 u@h2", "D1", "B"));
        assert_eq!((back.framed_pid, back.shell.as_deref(), back.uname.as_deref(), back.framer_version()), (Some(200), Some("/bin/zsh"), Some("Linux"), Some(2)));
        assert_eq!(back.setup.initial_directory.as_deref(), Some("/srv"));
        assert_eq!(back.setup.parsed, inner.setup.parsed);
        assert_eq!((back.depth(), back.line_prefix(), back.state(), back.waiting_to_resync), (1, "", &State::Ground, false));
        assert!(back.setup.vars_to_send.is_empty() && back.payloads.is_empty() && back.modified_vars.is_none());
        let parent = back.parent.as_ref().expect("the chain comes back");
        assert_eq!((parent.framed_pid, parent.home_directory.as_deref(), parent.setup.client_unique_id.as_str(), parent.depth()), (Some(100), Some("/home/u"), "A", 0));
        assert!(!parent.setup.local_origin && !back.root_is_local(), "a restored chain is unvouched");
        assert!(!json.contains("local_origin"), "{json}");
        assert_ne!(back.guid, inner.guid);
        assert_eq!(back.tree(), inner.tree());
        assert_eq!(inner.tree().0.keys().copied().collect::<Vec<_>>(), [100]);
        // A saved conductor has an empty queue and ground state, whatever it was doing.
        inner.send(Command::Cd("/tmp".into()), Handler::FireAndForget, false);
        assert_eq!(Conductor::from_json(&inner.json()).unwrap().pending(), Vec::<&Command>::new());
    }

    /// A state file can be edited, so what it says of a hop's origin is never believed: a
    /// restored conductor is unvouched (ruling), and Duplicate Session and splits of it build
    /// no local command from it.
    #[test]
    fn a_restored_conductor_is_never_vouched() {
        let mut root = Conductor::new(Setup { sshargs: "-p 22 me@home.lan ".into(), client_unique_id: "A".into(), local_origin: true, ..Setup::default() }, None, "P");
        root.setup.parsed = crate::ssh_args::parse("-p 22 me@home.lan ", "46", &crate::ssh_args::NoConfig);
        root.framed_pid = Some(100);
        let back = Conductor::from_json(&root.json()).expect("restores");
        assert!(!back.setup.local_origin && !back.root_is_local() && back.split_identity().is_none(), "Tandem's own file vouches for nothing");
        // An edited file claiming the origin at every level is not believed either.
        let mut v: serde_json::Value = serde_json::from_str(&root.json()).unwrap();
        v["local_origin"] = true.into();
        let forged = Conductor::from_json(&v.to_string()).expect("restores");
        assert!(!forged.setup.local_origin && !forged.root_is_local(), "{v}");
        let child = Conductor::new(Setup { sshargs: "-l evil attacker.example".into(), client_unique_id: "B".into(), ..Setup::default() }, Some(Box::new(root)), "Q");
        let mut v: serde_json::Value = serde_json::from_str(&child.json()).unwrap();
        v["local_origin"] = true.into();
        v["parent"]["local_origin"] = true.into();
        let forged = Conductor::from_json(&v.to_string()).expect("restores");
        assert!(!forged.setup.local_origin && !forged.root_is_local() && forged.split_identity().is_none(), "{v}");
    }

    #[test]
    fn restored_json_is_checked() {
        let mut c = framing();
        c.setup.dcs_id = "D0".into();
        let good = c.json();
        assert!(Conductor::from_json(&good).is_some());
        // Not JSON, the wrong shape, an id that is not text, a chain deeper than any hook, text
        // with controls, and an enormous value are all refused.
        assert!(Conductor::from_json("").is_none());
        assert!(Conductor::from_json("{}").is_none());
        assert!(Conductor::from_json("[1,2]").is_none());
        assert!(Conductor::from_json(&good.replace("\"D0\"", "5")).is_none());
        assert!(Conductor::from_json(&good.replace("\"D0\"", "\"D\\u0007x\"")).is_none(), "a control character in an id");
        assert!(Conductor::from_json(&good.replace("\"D0\"", &format!("\"{}\"", "d".repeat(70_000)))).is_none());
        let mut deep = good.clone();
        for _ in 0..20 {
            deep = good.replacen("\"parent\":null", &format!("\"parent\":{deep}"), 1);
        }
        assert!(Conductor::from_json(&deep).is_none(), "more conductors than a hook tree can follow");
        // A pid that names no process is refused.
        assert!(Conductor::from_json(&good.replace("\"framed_pid\":100", "\"framed_pid\":-1000")).is_none());
    }
}

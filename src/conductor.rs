//! iTerm2's Conductor (`CD`, milestone 14b): the commands it sends to conductor.sh and
//! framer.py with their wire encoding, and the conductor object with its parent chain.
//! Pure: it talks to the app only through the `Effect`s it queues (the tmux gateway's
//! `take_writes` pattern).

use crate::ssh_args::{Identity, ParsedArgs};
use crate::ssh_inject::{expand_local_tilde, injection_payloads, last_component, modify_remote, shell_supports_injection, tarballs, tweak_destination, Payload, Source, INTEGRATION_SUBDIR};
use std::path::Path;
use base64::Engine;

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// A conductor command (`CD:912-958`). Every framer variant is built here with its wire
/// form; milestone 14c sends them and 14d adds the file commands. The it2 proxy commands
/// are not built (ruling 3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    ExecLoginShell(Vec<String>),
    Setenv { key: String, value: String },
    /// Replace the conductor with this command.
    Run(String),
    /// Reads the python program and executes it.
    RunPython(String),
    /// Shell out to this command and then return to the conductor.
    Shell(String),
    PythonVersion,
    GetShell,
    Write { data: Vec<u8>, dest: String },
    Cd(String),
    Quit,
    /// Base64-encoded bash code.
    Eval(String),
    FramerRun(String),
    FramerLogin { cwd: String, args: Vec<String> },
    FramerEval(String),
    FramerSend { data: Vec<u8>, pid: i32 },
    FramerKill(i32),
    FramerQuit,
    FramerRegister(i32),
    FramerDeregister(i32),
    FramerPoll,
    FramerReset1,
    FramerReset2(String),
    FramerAutopoll,
    FramerSave(Vec<(String, String)>),
    FramerGetenv(String),
    FramerExecPythonStatements(String),
}

impl Command {
    /// stringValue (`CD:969-1032`).
    pub fn string_value(&self) -> String {
        let lines = |words: &[&str]| words.join("\n");
        match self {
            Command::ExecLoginShell(args) => std::iter::once("exec_login_shell").chain(args.iter().map(String::as_str)).collect::<Vec<_>>().join("\n"),
            Command::Setenv { key, value } => format!("setenv {key} {}", bash_escape(value)),
            Command::Run(cmd) => format!("run {cmd}"),
            Command::RunPython(_) => "runpython".into(),
            Command::Shell(cmd) => format!("shell {cmd}"),
            Command::PythonVersion => "pythonversion".into(),
            Command::GetShell => "getshell".into(),
            Command::Write { data, dest } => format!("write {} {dest}", b64(data)),
            Command::Cd(dir) => format!("cd {dir}"),
            Command::Quit => "quit".into(),
            Command::Eval(code) => format!("eval {code}"),
            Command::FramerRun(cmd) => lines(&["run", cmd]),
            Command::FramerLogin { cwd, args } => ["login", cwd.as_str()].into_iter().chain(args.iter().map(String::as_str)).collect::<Vec<_>>().join("\n"),
            Command::FramerEval(script) => lines(&["eval", &b64(script.as_bytes())]),
            Command::FramerSend { data, pid } => lines(&["send", &pid.to_string(), &b64(data)]),
            Command::FramerKill(pid) => lines(&["kill", &pid.to_string()]),
            Command::FramerQuit => "quit".into(),
            Command::FramerRegister(pid) => lines(&["register", &pid.to_string()]),
            // iTerm2's typo, kept for wire parity (ruling 14c-3).
            Command::FramerDeregister(pid) => lines(&["dereigster", &pid.to_string()]),
            Command::FramerPoll => "poll".into(),
            Command::FramerReset1 => "reset".into(),
            Command::FramerReset2(code) => lines(&["reset2", code]),
            Command::FramerAutopoll => "autopoll".into(),
            // iTerm2 walks a dictionary (any order); Tandem keeps the order given.
            Command::FramerSave(pairs) => std::iter::once("save".to_string()).chain(pairs.iter().map(|(k, v)| format!("{k}={v}"))).collect::<Vec<_>>().join("\n"),
            Command::FramerGetenv(name) => lines(&["getenv", name]),
            Command::FramerExecPythonStatements(statements) => lines(&["runpy", &b64(statements.as_bytes())]),
        }
    }

    /// operationDescription (`CD:1034-1100`).
    pub fn description(&self) -> String {
        match self {
            Command::ExecLoginShell(args) => format!("starting login shell with args {}", args.join(" ")),
            Command::Setenv { key, value } => format!("setting {key}={value}"),
            Command::Run(cmd) => format!("running \u{201c}{cmd}\u{201d}"),
            Command::Shell(cmd) => format!("running in shell \u{201c}{cmd}\u{201d}"),
            Command::PythonVersion => "running pythonversion".into(),
            Command::RunPython(_) => "running Python code".into(),
            Command::Write { dest, .. } => format!("copying files to {dest}"),
            Command::Cd(dir) => format!("changing directory to {dir}"),
            Command::Quit => "quitting".into(),
            Command::Eval(_) => "evaling".into(),
            Command::GetShell => "getshell".into(),
            Command::FramerRun(cmd) => format!("run \u{201c}{cmd}\u{201d}"),
            Command::FramerLogin { cwd, args } => format!("login cwd={cwd} args={}", swift_array(args)),
            Command::FramerEval(_) => "eval".into(),
            Command::FramerSave(pairs) => format!("save {}", pairs.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(", ")),
            Command::FramerSend { data, pid } => format!("send {} to {pid}", semi_verbose(data)),
            Command::FramerKill(pid) => format!("kill {pid}"),
            Command::FramerGetenv(name) => format!("getenv {name}"),
            Command::FramerExecPythonStatements(statements) => format!("exec python statements {statements}"),
            Command::FramerQuit => "quit".into(),
            Command::FramerRegister(pid) => format!("register {pid}"),
            Command::FramerDeregister(pid) => format!("dereigster {pid}"),
            Command::FramerPoll => "poll".into(),
            Command::FramerReset1 => "reset".into(),
            Command::FramerReset2(code) => format!("reset2 {code}"),
            Command::FramerAutopoll => "autopoll".into(),
        }
    }

    /// isFramer (`CD:957-967`).
    pub fn is_framer(&self) -> bool {
        !matches!(
            self,
            Command::ExecLoginShell(_)
                | Command::Setenv { .. }
                | Command::Run(_)
                | Command::RunPython(_)
                | Command::Shell(_)
                | Command::PythonVersion
                | Command::GetShell
                | Command::Write { .. }
                | Command::Cd(_)
                | Command::Quit
                | Command::Eval(_)
        )
    }

    /// A command whose answer may take longer than the command timeout without anything
    /// being wrong. framer.py answers every command here at once (`F:653-1430`); 14d's
    /// `file zip` streams.
    pub fn streams(&self) -> bool {
        false
    }
}

/// Swift's description of a `[String]` (`["a", "b"]`): each element's debugDescription
/// (`UnicodeScalar.swift` `_escaped(asASCII: false)`). Swift's extra escaping of a
/// combining mark that would join the quote is not ported.
fn swift_array(items: &[String]) -> String {
    let quoted: Vec<String> = items
        .iter()
        .map(|item| {
            let mut out = String::from("\"");
            for c in item.chars() {
                match c {
                    '\\' => out.push_str("\\\\"),
                    '\'' => out.push_str("\\'"),
                    '"' => out.push_str("\\\""),
                    ' '..='~' => out.push(c),
                    '\0' => out.push_str("\\0"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    c if (c as u32) < 128 => out.push_str(&format!("\\u{{{:02X}}}", c as u32)),
                    c => out.push(c),
                }
            }
            out.push('"');
            out
        })
        .collect();
    format!("[{}]", quoted.join(", "))
}

/// semiVerboseDescription (`Data+iTerm.swift:32-41`): over 32 bytes the first and last 16
/// around an ellipsis; UTF-8 text in curly quotes with controls and backslash escaped
/// (`NS:766-780`); anything else as lowercase hex.
fn semi_verbose(data: &[u8]) -> String {
    if data.len() > 32 {
        return format!("{}\u{2026}{}", semi_verbose(&data[..16]), semi_verbose(&data[data.len() - 16..]));
    }
    match std::str::from_utf8(data) {
        Ok(text) => {
            let mut safe = String::new();
            for c in text.chars() {
                match c {
                    '\\' => safe.push_str("\\\\"),
                    '\n' => safe.push_str("\\n"),
                    '\r' => safe.push_str("\\r"),
                    '\t' => safe.push_str("\\t"),
                    c if (c as u32) < 32 => safe.push_str(&format!("\\x{:04x}", c as u32)),
                    c => safe.push(c),
                }
            }
            format!("\u{201c}{safe}\u{201d}")
        }
        Err(_) => data.iter().map(|b| format!("{b:02x}")).collect(),
    }
}

/// shellEscapableCharacters (`NS:160-162`).
const SHELL_ESCAPABLE: &str = "\\ ()\"&'!$<>;|*?[]#`\t{}^+=@~";

/// stringEscapedForBash (`NS:171-175`, `NS:3251-3287`): unchanged unless an escapable
/// character, CR or LF is present; then `$'...'` with backslash doubled, `'` as `\x27`, CR
/// as `\r` and LF as `\n` (a tab stays itself).
pub fn bash_escape(s: &str) -> String {
    if !s.chars().any(|c| c == '\r' || c == '\n' || SHELL_ESCAPABLE.contains(c)) {
        return s.to_string();
    }
    let inner = s.replace('\\', "\\\\").replace('\'', "\\x27").replace('\r', "\\r").replace('\n', "\\n");
    format!("$'{inner}'")
}

/// The text a command goes out as, before `write()`'s final `\n` (`CD:2871-2879`): each
/// line base64'd, the lines joined with `\n`, cut into 128-character chunks
/// (`String+iTerm.swift:508-523`); a pre-framer chunk gets the line prefix, a framer chunk
/// a trailing `\` unless it is the last; the chunks joined with `\n`, plus `\n`.
pub fn encode(cmd: &Command, line_prefix: &str) -> String {
    let framer = cmd.is_framer();
    let text = cmd.string_value().split('\n').map(|line| b64(line.as_bytes())).collect::<Vec<_>>().join("\n");
    // Base64 and "\n" are ASCII, so characters are bytes.
    let pieces: Vec<&str> = text.as_bytes().chunks(128).map(|c| std::str::from_utf8(c).unwrap_or_default()).collect();
    let last = pieces.len().saturating_sub(1);
    let chunks: Vec<String> = pieces
        .iter()
        .enumerate()
        .map(|(i, piece)| match (framer, i < last) {
            (true, true) => format!("{piece}\\"),
            (true, false) => piece.to_string(),
            (false, _) => format!("{line_prefix}{piece}"),
        })
        .collect();
    chunks.join("\n") + "\n"
}

/// What the conductor asks of the app.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// The conductor's own bytes, through `write()`: never held.
    Write(Vec<u8>),
    /// iTerm2's `conductorWrite` outside `write()`: held like typing while writes queue.
    SessionWrite(Vec<u8>),
    /// Bytes sent through `send_keys`: held while setup runs.
    Keys(Vec<u8>),
    StopQueueingInput,
    SendInitialText,
    StateChanged,
    Abort(String),
}

/// What an unhook restores (Decision: the remote host and the working directory).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SavedState {
    pub cwd: Option<String>,
    pub host: Option<String>,
    pub user: Option<String>,
}

/// What the hook knows when a conductor starts (`PS:20118-20170`).
#[derive(Clone, Debug, Default)]
pub struct Setup {
    pub sshargs: String,
    pub bool_args: String,
    pub dcs_id: String,
    pub client_unique_id: String,
    pub vars_to_send: Vec<(String, String)>,
    pub client_vars: Vec<(String, String)>,
    pub initial_directory: Option<String>,
    pub inject: bool,
    pub parsed: ParsedArgs,
}

/// How a command's results are handled (`CD:1205-1221`). The reply lines a handler keeps
/// are its own; 14c adds handlers holding an `f64`, so no `Eq`.
#[derive(Clone, Debug, PartialEq)]
pub enum Handler {
    FailIfNonzero,
    FireAndForget,
    NonFramerLogin,
    WriteOnSuccess(String),
    GetShell(Vec<String>),
    CheckForPython(Vec<String>),
}

/// A command with its handler (ExecutionContext, `CD:1159-1164`).
#[derive(Clone, Debug, PartialEq)]
pub struct Context {
    pub command: Command,
    pub handler: Handler,
    canceled: bool,
}

/// The conductor's state (`CD:1483-1507`). 14c adds `Recovery` and `Recovered`.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// Nothing written, nothing expected.
    Ground,
    /// Written, its begin not yet parsed. Never empty.
    WillExecute(Vec<Context>),
    /// The first has had a line; the rest were written behind it.
    Executing(Context, Vec<Context>),
    /// Not using the conductor: input and output are direct.
    Unhooked,
}

/// What a handler is told (PartialResult, `CD:1509-1515`; side-channel lines are 14c).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Partial {
    Line(String),
    /// The exit status.
    End(u8),
    /// The command could not be sent, or the conductor went away.
    Abort,
    Canceled,
}

/// The failure text for a command that ended with an unexpected status (`CD:2115,2124`,
/// iTerm2's typo kept). A setenv shows its name only: the value may be a secret.
pub fn unexpected_status(cmd: &Command, status: u8) -> String {
    let shown = match cmd {
        Command::Setenv { key, .. } => format!("setenv {key}"),
        cmd => cmd.string_value(),
    };
    format!("{shown}: Unepected status {status}")
}

/// A command's operationDescription as the person sees it: a setenv names the variable
/// only, since the value may be a secret.
fn shown_description(cmd: &Command) -> String {
    match cmd {
        Command::Setenv { key, .. } => format!("setting {key}"),
        cmd => cmd.description(),
    }
}

/// The failure text for python code that did not run (`CD:2255`).
pub fn python_status(status: u8) -> String {
    format!("Status {status} when running python code")
}

/// The oldest remote python that runs framer.py (`CD:2077-2078`).
pub const MIN_PYTHON: (u32, u32) = (3, 7);

/// The version checkForPython reads from pythonversion's reply (`CD:2143-2151`): the regex
/// `^Python ([0-9]\.[0-9][0-9]*)` at the start of the text, a minor too big to count as 0.
pub fn python_version(output: &str) -> Option<(u32, u32)> {
    let re = regex::Regex::new(r"^Python ([0-9])\.([0-9][0-9]*)").ok()?;
    let groups = re.captures(output)?;
    Some((groups[1].parse().unwrap_or(0), groups[2].parse().unwrap_or(0)))
}

/// Where getshell's reply starts (iTerm2 bug B15 guarded): conductor.sh's guess_login_shell
/// can print a shell error ("sh: 160: using_passwd: not found") before the shell, so the
/// first line that starts a pair of absolute paths (shell, then home) is taken; with no such
/// pair, line 0 as iTerm2 (conductor.sh's fallback shell is the bare "sh").
fn shell_line(parts: &[&str]) -> usize {
    parts.windows(2).position(|pair| pair[0].starts_with('/') && pair[1].starts_with('/')).unwrap_or(0)
}

/// One conductor.sh (or framer.py) on the other side of an ssh (`CD`).
pub struct Conductor {
    pub setup: Setup,
    pub parent: Option<Box<Conductor>>,
    pub framed_pid: Option<i32>,
    pub payloads: Vec<Payload>,
    pub modified_vars: Option<Vec<(String, String)>>,
    pub modified_command_args: Option<Vec<String>>,
    pub home_directory: Option<String>,
    pub shell: Option<String>,
    pub saved: Option<SavedState>,
    pub python_ok: bool,
    line_prefix: String,
    depth: i32,
    effects: Vec<Effect>,
    state: State,
    queue: Vec<Context>,
    /// Set while a handler runs and while commands are written: `send` only enqueues, and
    /// the transition's own dequeue sends it next (`CD:384-391`, `CD:2964-2969`).
    transition: bool,
    /// Every handler call with the state at that moment, for the tests.
    #[cfg(test)]
    updates: Vec<(Command, Partial, State)>,
}

impl Conductor {
    /// A conductor for a hook (`CD:484-517`): `\u{3}` plus `prefix_uuid` starts each
    /// pre-framer line; the depth is the parent's, one more when the parent frames (an
    /// unframed parent does not wrap input for it).
    pub fn new(setup: Setup, parent: Option<Box<Conductor>>, prefix_uuid: &str) -> Conductor {
        let depth = match &parent {
            Some(p) if p.framing() => p.depth + 1,
            Some(p) => p.depth,
            None => 0,
        };
        Conductor {
            setup,
            parent,
            framed_pid: None,
            payloads: Vec::new(),
            modified_vars: None,
            modified_command_args: None,
            home_directory: None,
            shell: None,
            saved: None,
            python_ok: false,
            line_prefix: format!("\u{3}{prefix_uuid}"),
            depth,
            effects: Vec::new(),
            state: State::Ground,
            queue: Vec::new(),
            transition: false,
            #[cfg(test)]
            updates: Vec::new(),
        }
    }

    /// add(path:destination:) (`CD:1590-1604`): a Files to Copy entry, its local `~` being
    /// `local_home`.
    pub fn add_path(&mut self, path: &str, destination: &str, local_home: &Path) {
        self.payloads.push(Payload { source: Source::Disk(expand_local_tilde(path, local_home)), destination: tweak_destination(destination) });
    }

    /// start (`CD:1606-1611`): the line prefix conductor.sh reads first, then getshell.
    pub fn start(&mut self) {
        if !self.line_prefix.is_empty() {
            let prefix = self.line_prefix.clone();
            self.write(&prefix);
        }
        self.send(Command::GetShell, Handler::GetShell(Vec::new()), false);
    }

    /// The end of getshell's reply (`CD:2303-2369` without the it2 proxy, ruling 3).
    fn did_get_shell(&mut self, lines: &[String]) {
        let joined = lines.concat();
        let all: Vec<&str> = joined.split('\n').map(|p| p.trim_matches([' ', '\t'])).collect();
        let parts = &all[shell_line(&all)..];
        let command_args = self.setup.parsed.command_args.clone();
        let shell = command_args.first().cloned().unwrap_or_else(|| parts[0].to_string());
        let home = parts.get(1).copied().unwrap_or("");
        let version = parts.get(2..).map(|v| v.join("\n")).unwrap_or_default();
        if !shell.is_empty() && !home.is_empty() && self.setup.inject && shell_supports_injection(last_component(&shell), &version) {
            let dir = format!("{home}/{INTEGRATION_SUBDIR}");
            let argv = command_args.get(1..).unwrap_or_default();
            let (vars, mut args) = modify_remote(&dir, home, &self.setup.vars_to_send, &shell, argv);
            if let Some(first) = command_args.first() {
                // An explicit command keeps its first word as argv[0].
                args.insert(0, first.clone());
            } else if !args.is_empty() {
                // bash's --posix: the shell must be named for its arguments to count.
                args.insert(0, shell.clone());
            }
            self.modified_vars = Some(vars);
            self.modified_command_args = Some(args);
            self.payloads.extend(injection_payloads());
        }
        self.shell = Some(shell);
        self.effects.push(Effect::StateChanged);
        self.did_finish_get_shell();
    }

    /// didFinishGetShell (`CD:1780-1787`, `CD:2014-2026`, `CD:1942-1951`): each setenv, each
    /// tarball, the directory, then the python check. Inside a handler these only queue.
    fn did_finish_get_shell(&mut self) {
        let vars = self.modified_vars.clone().unwrap_or_else(|| self.setup.vars_to_send.clone());
        for (key, value) in vars {
            self.send(Command::Setenv { key, value }, Handler::FailIfNonzero, false);
        }
        for (data, dest) in tarballs(&self.payloads) {
            self.send(Command::Write { data, dest }, Handler::FailIfNonzero, false);
        }
        if let Some(dir) = self.setup.initial_directory.clone() {
            self.send(Command::Cd(dir), Handler::FailIfNonzero, false);
        }
        self.send(Command::PythonVersion, Handler::CheckForPython(Vec::new()), false);
    }

    /// The end of pythonversion's reply (`CD:2130-2161`): a usable python is ready; anything
    /// else starts the login shell (a pending jump's eval is 14c).
    fn did_check_for_python(&mut self, lines: &[String], status: u8) {
        match python_version(&lines.join("\n")) {
            Some(version) if status == 0 && version >= MIN_PYTHON => self.python_ready(),
            _ => self.exec_login_shell(),
        }
    }

    /// A remote python of `MIN_PYTHON` or later. 14c Task 1 replaces the login shell here
    /// with execFramer (ruling 14b-6).
    fn python_ready(&mut self) {
        self.python_ok = true;
        self.exec_login_shell();
    }

    /// execLoginShell (`CD:2028-2038`): input stops queueing in the connecting pane, then an
    /// empty injected argv or no command starts the login shell, else the command runs. A
    /// non-empty injected argv is dropped (iTerm2 bug B7, kept).
    pub fn exec_login_shell(&mut self) {
        self.effects.push(Effect::StopQueueingInput);
        let command_args = &self.setup.parsed.command_args;
        if self.modified_command_args.as_ref().is_some_and(Vec::is_empty) || command_args.is_empty() {
            self.send(Command::ExecLoginShell(Vec::new()), Handler::NonFramerLogin, false);
        } else {
            let command = command_args.join(" ");
            self.send(Command::Run(command), Handler::FailIfNonzero, false);
        }
    }

    pub fn line_prefix(&self) -> &str {
        &self.line_prefix
    }

    pub fn depth(&self) -> i32 {
        self.depth
    }

    pub fn framing(&self) -> bool {
        self.framed_pid.is_some()
    }

    /// ancestryContains (`CD:1850-1852`).
    pub fn ancestry_contains(&self, unique_id: &str) -> bool {
        self.setup.client_unique_id == unique_id || self.parent.as_ref().is_some_and(|p| p.ancestry_contains(unique_id))
    }

    pub fn identity(&self) -> Identity {
        self.setup.parsed.identity()
    }

    /// Everything queued for the app: the parents' effects first, outermost first, then
    /// this conductor's (the app drains only a pane's innermost conductor).
    pub fn take_effects(&mut self) -> Vec<Effect> {
        let mut out = self.parent.as_mut().map(|p| p.take_effects()).unwrap_or_default();
        out.append(&mut self.effects);
        out
    }

    /// sendKeys (`CD:1854-1862`), unframed: the bytes go to the session, which holds them
    /// while setup runs. Milestone 14c adds the framed branch.
    pub fn send_keys(&mut self, data: &[u8]) {
        self.effects.push(Effect::Keys(data.to_vec()));
    }

    /// The conductor's own bytes as a parent carries them for a child's `write()`: an
    /// unframed parent writes them raw (sendKeys with `_queueWrites` cleared, never held).
    fn write_through(&mut self, bytes: Vec<u8>) {
        self.effects.push(Effect::Write(bytes));
    }

    /// Runs `f` on the parent, if any, and moves what the parent queues meanwhile into this
    /// conductor's queue after its own, so the effects keep the order they were made in.
    fn via_parent(&mut self, f: impl FnOnce(&mut Conductor)) {
        if let Some(parent) = self.parent.as_deref_mut() {
            let before = parent.effects.len();
            f(parent);
            self.effects.extend(parent.effects.drain(before..));
        }
    }

    /// write (`CD:3039-3058`): `text` plus `\n`, through the parent when there is one.
    /// `_queueWrites` is not toggled: an `Effect::Write` is never held.
    pub(crate) fn write(&mut self, text: &str) {
        let bytes = format!("{text}\n").into_bytes();
        if self.parent.is_some() {
            self.via_parent(|parent| parent.write_through(bytes));
        } else {
            self.effects.push(Effect::Write(bytes));
        }
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn unhooked(&self) -> bool {
        self.state == State::Unhooked
    }

    /// The command written and not yet ended, for the command timer (Task 9) and tests.
    pub fn in_flight(&self) -> Option<&Command> {
        match &self.state {
            State::WillExecute(contexts) => contexts.first().map(|c| &c.command),
            State::Executing(context, _) => Some(&context.command),
            State::Ground | State::Unhooked => None,
        }
    }

    /// queueWrites (`CD:656-670`): the session holds its writes while this is true. Only
    /// `write()` clears iTerm2's own flag, and Tandem's writes are never held, so the flag
    /// is the state; a conductor under an unhooked parent never holds writes.
    pub fn queue_writes(&self) -> bool {
        !self.unhooked() && self.parent.as_ref().is_none_or(|p| p.queue_writes())
    }

    /// handlesKeystrokes (`CD:700-703`): route writes through `send_keys`.
    pub fn handles_keystrokes(&self) -> bool {
        self.framing() && self.queue_writes()
    }

    /// currentOperationDescription (`CD:3060-3083`), with setenv values hidden.
    pub fn description(&self) -> String {
        match &self.state {
            State::Ground => "waiting".into(),
            State::Executing(context, pending) if pending.is_empty() => shown_description(&context.command),
            State::Executing(context, pending) => format!("{} and {} more", shown_description(&context.command), pending.len()),
            State::WillExecute(contexts) => {
                let first = format!("{} (preparation stage)", shown_description(&contexts[0].command));
                match contexts.len() {
                    1 => first,
                    n => format!("{first} and {} more", n - 1),
                }
            }
            State::Unhooked => "unhooked".into(),
        }
    }

    /// send (`CD:2802-2846`): enqueue (first when `high_priority`); from ground the command
    /// is written at once. During a transition it only enqueues. In flight, iTerm2 tops up
    /// a pipeline, which no 14b command joins (14c). Unhooked, it stays queued, unsent.
    pub fn send(&mut self, cmd: Command, handler: Handler, high_priority: bool) {
        let context = Context { command: cmd, handler, canceled: false };
        if high_priority {
            self.queue.insert(0, context);
        } else {
            self.queue.push(context);
        }
        if self.transition {
            return;
        }
        if self.state == State::Ground {
            self.dequeue();
        }
    }

    /// cancelEnqueuedRequests (`CD:2848-2858`): matching queued commands are dropped with
    /// `Partial::Canceled` when their turn comes.
    pub fn cancel_enqueued(&mut self, pred: &dyn Fn(&Command) -> bool) {
        for context in self.queue.iter_mut().filter(|c| pred(&c.command)) {
            context.canceled = true;
        }
    }

    /// dequeue and extendPipeline from ground (`CD:2860-2869`, `CD:2888-2960`), one context
    /// at a time (14c pipelines): canceled contexts are dropped, the next one becomes the
    /// state before it is written, and the dropped handlers run once the state is
    /// consistent.
    fn dequeue(&mut self) {
        debug_assert_eq!(self.state, State::Ground);
        let mut dropped = Vec::new();
        while self.queue.first().is_some_and(|c| c.canceled) {
            dropped.push((self.queue.remove(0), Partial::Canceled));
        }
        if !self.queue.is_empty() {
            let context = self.queue.remove(0);
            let text = encode(&context.command, &self.line_prefix);
            self.in_transition(|c| {
                c.state = State::WillExecute(vec![context]);
                c.write(&text);
            });
        }
        for (context, result) in dropped {
            self.update(&context, result);
        }
    }

    /// withPipelineTransition (`CD:2964-2969`): nested transitions restore the flag.
    fn in_transition(&mut self, f: impl FnOnce(&mut Conductor)) {
        let saved = self.transition;
        self.transition = true;
        f(self);
        self.transition = saved;
    }

    /// handleCommandBegin (`CD:2468-2472`): iTerm2 only logs it.
    pub fn handle_begin(&mut self, _id: &str, _depth: i32) {}

    /// handle(line:depth:) (`CD:2413-2433`).
    pub fn handle_line(&mut self, line: &str, depth: i32) {
        if depth != self.depth && self.framing() {
            self.via_parent(|parent| parent.handle_line(line, depth));
            return;
        }
        let context = match std::mem::replace(&mut self.state, State::Ground) {
            State::WillExecute(mut contexts) => {
                let first = contexts.remove(0);
                self.state = State::Executing(first.clone(), contexts);
                first
            }
            State::Executing(context, pending) => {
                self.state = State::Executing(context.clone(), pending);
                context
            }
            // Ground and unhooked tolerate stray input.
            other => {
                self.state = other;
                return;
            }
        };
        self.update(&context, Partial::Line(line.to_string()));
    }

    /// handleCommandEnd (`CD:2475-2537`): `kind` is "f" for framer.py, "r" for conductor.sh.
    pub fn handle_end(&mut self, id: &str, kind: &str, status: u8, depth: i32) {
        let expect_framing = self.framing() || matches!(&self.state, State::Executing(c, _) if c.command.is_framer());
        if (!expect_framing && kind == "f") || (self.framing() && depth != self.depth) {
            self.via_parent(|parent| parent.handle_end(id, kind, status, depth));
            return;
        }
        let (context, pending) = match &self.state {
            State::WillExecute(contexts) => (contexts[0].clone(), contexts[1..].to_vec()),
            State::Executing(context, pending) => (context.clone(), pending.clone()),
            // Ground and unhooked tolerate stray input.
            State::Ground | State::Unhooked => return,
        };
        self.in_transition(|c| c.update(&context, Partial::End(status)));
        if pending.is_empty() {
            self.state = State::Ground;
            self.dequeue();
        } else {
            // amendPipeline: nothing joins a 14b pipeline, so only the state changes.
            self.state = State::WillExecute(pending);
        }
    }

    /// handleUnhook (`CD:2435-2466`): the in-flight and queued contexts are taken, the
    /// state is set, and only then their handlers hear `Abort`.
    pub fn handle_unhook(&mut self) {
        let mut aborted: Vec<Context> = match std::mem::replace(&mut self.state, State::Unhooked) {
            State::WillExecute(mut contexts) => vec![contexts.remove(0)],
            State::Executing(context, _) => vec![context],
            State::Ground | State::Unhooked => Vec::new(),
        };
        aborted.append(&mut self.queue);
        for context in aborted {
            self.update(&context, Partial::Abort);
        }
        self.state = State::Unhooked;
    }

    /// forceReturnToGroundState (`CD:3085-3093`): ground, the queue aborted (the in-flight
    /// command is dropped unheard), then the parent's. The handlers run in a transition and
    /// the queue is cleared after them, so what an abort handler sends (CheckForPython's
    /// login shell) is dropped too, never written after the failure. An unhooked conductor
    /// stays unhooked (iTerm2 bug B17 fixed): iTerm2 grounds every parent, so a failed
    /// nested setup makes the unhooked level 1 parent hold typing forever.
    pub fn force_ground(&mut self) {
        if !self.unhooked() {
            self.state = State::Ground;
        }
        let aborted = self.queue.clone();
        self.in_transition(|c| {
            for context in aborted {
                c.update(&context, Partial::Abort);
            }
        });
        self.queue.clear();
        self.via_parent(Conductor::force_ground);
    }

    /// fail (`CD:3095-3103`): back to ground, then the raw `exec_login_shell` line through
    /// the session (outside `write()`, so it is held like typing; without the line prefix,
    /// base64 or the blank line conductor.sh needs: iTerm2 bug B3, kept), then the abort.
    pub fn fail(&mut self, reason: &str) {
        self.force_ground();
        let line = Command::ExecLoginShell(Vec::new()).string_value() + "\n";
        self.effects.push(Effect::SessionWrite(line.into_bytes()));
        self.effects.push(Effect::Abort(reason.to_string()));
    }

    /// The in-flight command's reply lines, kept by its handler (iTerm2's StringArray is a
    /// reference; the context the handler is given is a copy).
    fn collect_line(&mut self, line: String) {
        if let State::Executing(context, _) = &mut self.state {
            if let Handler::GetShell(lines) | Handler::CheckForPython(lines) = &mut context.handler {
                lines.push(line);
            }
        }
    }

    /// update(executionContext:result:) (`CD:2106-2396`) for the 14b handlers.
    fn update(&mut self, context: &Context, result: Partial) {
        #[cfg(test)]
        self.updates.push((context.command.clone(), result.clone(), self.state.clone()));
        let status = match result {
            Partial::End(status) => status,
            Partial::Line(line) => {
                self.collect_line(line);
                return;
            }
            Partial::Abort | Partial::Canceled => {
                if matches!(context.handler, Handler::CheckForPython(_)) {
                    self.exec_login_shell();
                }
                return;
            }
        };
        match &context.handler {
            Handler::FireAndForget => {}
            Handler::FailIfNonzero => {
                if status != 0 {
                    self.fail(&unexpected_status(&context.command, status));
                }
            }
            Handler::NonFramerLogin => {
                if status == 0 {
                    self.effects.push(Effect::SendInitialText);
                } else {
                    self.fail(&unexpected_status(&context.command, status));
                }
            }
            Handler::WriteOnSuccess(code) => {
                if status == 0 {
                    self.write(&format!("{code}\nEOF\n"));
                } else {
                    self.fail(&python_status(status));
                }
            }
            // A non-zero status stalls setup with nothing queued (iTerm2 bug B6, kept;
            // unreachable because conductor.sh always reports 0, B1).
            Handler::GetShell(lines) => {
                if status == 0 {
                    self.did_get_shell(lines);
                }
            }
            Handler::CheckForPython(lines) => self.did_check_for_python(lines, status),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn pre_framer() -> Vec<Command> {
        vec![
            Command::ExecLoginShell(vec![]),
            Command::ExecLoginShell(s(&["a", "b"])),
            Command::Setenv { key: "FOO".into(), value: "bar".into() },
            Command::Run("ls -l".into()),
            Command::RunPython("print(1)".into()),
            Command::Shell("x".into()),
            Command::PythonVersion,
            Command::GetShell,
            Command::Write { data: vec![1, 2, 3], dest: "/$HOME/x".into() },
            Command::Cd("/tmp".into()),
            Command::Quit,
            Command::Eval("ZWNobw==".into()),
        ]
    }

    fn framer() -> Vec<Command> {
        vec![
            Command::FramerRun("uname -s".into()),
            Command::FramerLogin { cwd: "$HOME".into(), args: s(&["top", "-d"]) },
            Command::FramerEval("echo".into()),
            Command::FramerSend { data: b"ls\r".to_vec(), pid: 4242 },
            Command::FramerKill(7),
            Command::FramerQuit,
            Command::FramerRegister(4242),
            Command::FramerDeregister(4242),
            Command::FramerPoll,
            Command::FramerReset1,
            Command::FramerReset2("C0DE".into()),
            Command::FramerAutopoll,
            Command::FramerSave(vec![("dcsID".into(), "D0".into()), ("sshargs".into(), "h".into())]),
            Command::FramerGetenv("PATH".into()),
            Command::FramerExecPythonStatements("x=1".into()),
        ]
    }

    #[test]
    fn commands_follow_iterm2() {
        let pre: Vec<String> = pre_framer().iter().map(Command::string_value).collect();
        assert_eq!(
            pre,
            [
                "exec_login_shell",
                "exec_login_shell\na\nb",
                "setenv FOO bar",
                "run ls -l",
                "runpython",
                "shell x",
                "pythonversion",
                "getshell",
                "write AQID /$HOME/x",
                "cd /tmp",
                "quit",
                "eval ZWNobw==",
            ]
        );
        let framed: Vec<String> = framer().iter().map(Command::string_value).collect();
        assert_eq!(
            framed,
            [
                "run\nuname -s",
                "login\n$HOME\ntop\n-d",
                "eval\nZWNobw==",
                "send\n4242\nbHMN",
                "kill\n7",
                "quit",
                "register\n4242",
                // iTerm2's typo, kept for wire parity (ruling 14c-3).
                "dereigster\n4242",
                "poll",
                "reset",
                "reset2\nC0DE",
                "autopoll",
                "save\ndcsID=D0\nsshargs=h",
                "getenv\nPATH",
                "runpy\neD0x",
            ]
        );
        assert_eq!(pre_framer().len(), 12, "eleven variants, ExecLoginShell twice");
        for c in pre_framer() {
            assert!(!c.is_framer(), "{c:?}");
        }
        for c in framer() {
            assert!(c.is_framer(), "{c:?}");
        }
        for c in pre_framer().into_iter().chain(framer()) {
            assert!(!c.streams(), "{c:?}");
        }

        let mut framer_descriptions: Vec<String> = framer().iter().map(Command::description).collect();
        framer_descriptions[3] = Command::FramerSend { data: b"ls".to_vec(), pid: 4242 }.description();
        assert_eq!(
            framer_descriptions,
            [
                "run \u{201c}uname -s\u{201d}",
                "login cwd=$HOME args=[\"top\", \"-d\"]",
                "eval",
                "send \u{201c}ls\u{201d} to 4242",
                "kill 7",
                "quit",
                "register 4242",
                "dereigster 4242",
                "poll",
                "reset",
                "reset2 C0DE",
                "autopoll",
                "save dcsID, sshargs",
                "getenv PATH",
                "exec python statements x=1",
            ]
        );
        let pre: Vec<String> = pre_framer().iter().skip(1).map(Command::description).collect();
        assert_eq!(
            pre,
            [
                "starting login shell with args a b",
                "setting FOO=bar",
                "running \u{201c}ls -l\u{201d}",
                "running Python code",
                "running in shell \u{201c}x\u{201d}",
                "running pythonversion",
                "getshell",
                "copying files to /$HOME/x",
                "changing directory to /tmp",
                "quitting",
                "evaling",
            ]
        );
        assert_eq!(Command::ExecLoginShell(vec![]).description(), "starting login shell with args ");
    }

    #[test]
    fn semi_verbose_follows_iterm2() {
        let send = |data: &[u8]| Command::FramerSend { data: data.to_vec(), pid: 1 }.description();
        assert_eq!(send(b"ls\r"), "send \u{201c}ls\\r\u{201d} to 1");
        assert_eq!(send(b"a\\b\t\n\x01"), "send \u{201c}a\\\\b\\t\\n\\x0001\u{201d} to 1");
        assert_eq!(send(b""), "send \u{201c}\u{201d} to 1");
        assert_eq!(send(&[0xff, 0x00, 0x7f]), "send ff007f to 1", "not UTF-8: hex");
        let long: Vec<u8> = (b'a'..=b'z').chain(b'A'..=b'J').collect();
        assert_eq!(long.len(), 36);
        assert_eq!(send(&long), "send \u{201c}abcdefghijklmnop\u{201d}\u{2026}\u{201c}uvwxyzABCDEFGHIJ\u{201d} to 1");
        let exactly = &long[..32];
        assert_eq!(send(exactly), format!("send \u{201c}{}\u{201d} to 1", std::str::from_utf8(exactly).unwrap()));
        // A 16-byte half that cuts a character in two is hex.
        let mut split = vec![b'x'; 15];
        split.extend("\u{e9}".as_bytes());
        split.extend(vec![b'y'; 17]);
        assert_eq!(send(&split), format!("send {}c3\u{2026}\u{201c}{}\u{201d} to 1", "78".repeat(15), "y".repeat(16)));

        let login = |args: &[&str]| Command::FramerLogin { cwd: "/".into(), args: s(args) }.description();
        assert_eq!(login(&[]), "login cwd=/ args=[]");
        assert_eq!(login(&["it's", "a\"b", "c\\d", "\t\n\r\0", "\u{1}\u{7f}", "\u{e9}"]), "login cwd=/ args=[\"it\\'s\", \"a\\\"b\", \"c\\\\d\", \"\\t\\n\\r\\0\", \"\\u{01}\\u{7F}\", \"\u{e9}\"]");
    }

    #[test]
    fn bash_escape_follows_iterm2() {
        for plain in ["abc", "/home/u/.iterm2", ""] {
            assert_eq!(bash_escape(plain), plain);
        }
        assert_eq!(bash_escape("x y"), "$'x y'");
        assert_eq!(bash_escape("a=b"), "$'a=b'");
        assert_eq!(bash_escape("~"), "$'~'");
        assert_eq!(bash_escape("it's"), "$'it\\x27s'");
        assert_eq!(bash_escape("a\\b"), "$'a\\\\b'");
        assert_eq!(bash_escape("a\nb"), "$'a\\nb'");
        assert_eq!(bash_escape("a\rb"), "$'a\\rb'");
        // A tab is escapable but stays itself inside the quotes (NS:3251-3287 replaces only CR and LF).
        assert_eq!(bash_escape("a\tb"), "$'a\tb'");
        assert_eq!(bash_escape("\u{e9}"), "\u{e9}");
        assert_eq!(Command::Setenv { key: "A".into(), value: "x y".into() }.string_value(), "setenv A $'x y'");
    }

    #[test]
    fn encoding_follows_iterm2() {
        let p = "\u{3}ABC";
        assert_eq!(encode(&Command::GetShell, p), "\u{3}ABCZ2V0c2hlbGw=\n");
        assert_eq!(encode(&Command::ExecLoginShell(s(&["x"])), p), "\u{3}ABCZXhlY19sb2dpbl9zaGVsbA==\neA==\n");

        // "write " + 216 base64 characters + " /x" is 225 characters, 300 in base64.
        let write = Command::Write { data: vec![7; 162], dest: "/x".into() };
        assert_eq!(write.string_value().len(), 225);
        let out = encode(&write, p);
        let lines: Vec<&str> = out.strip_suffix('\n').unwrap().split('\n').collect();
        assert_eq!(lines.len(), 3);
        for line in &lines {
            assert!(line.starts_with(p), "{line}");
            assert!(!line.ends_with('\\'), "{line}");
        }
        assert_eq!(lines.iter().map(|l| l.len() - p.len()).collect::<Vec<_>>(), [128, 128, 44]);
        assert_eq!(lines.iter().map(|l| &l[p.len()..]).collect::<String>(), b64(write.string_value().as_bytes()));

        let setenv = Command::Setenv { key: "A".into(), value: "x\ny".into() };
        assert_eq!(encode(&setenv, p), format!("{p}{}\n", b64(b"setenv A $'x\\ny'")));

        assert_eq!(encode(&Command::FramerSend { data: b"ls\r".to_vec(), pid: 4242 }, p), "c2VuZA==\nNDI0Mg==\nYkhNTg==\n");
        let runpy = Command::FramerExecPythonStatements("a".repeat(200));
        let out = encode(&runpy, p);
        let body = out.strip_suffix('\n').unwrap();
        // Three chunks joined by "\n": the first two are 128 characters plus "\", the last
        // 113. The first chunk holds the join between the runpy line's base64 (8) and the
        // statements' base64 base64'd again (360).
        let chunks: Vec<&str> = body.split_inclusive("\\\n").collect();
        assert_eq!(chunks.iter().map(|c| c.len()).collect::<Vec<_>>(), [130, 130, 113]);
        assert!(chunks[..2].iter().all(|c| c.ends_with("\\\n")) && !chunks[2].contains('\\'));
        assert_eq!(&chunks[0][..9], "cnVucHk=\n");
        assert_eq!(body.len(), 369 + 2 + 2, "369 characters, two continuations, three joins");
        let text = body.replace("\\\n", "");
        assert_eq!(text.len(), 369);
        assert_eq!(text, format!("cnVucHk=\n{}", b64(b64(&[b'a'; 200]).as_bytes())));
        assert!(!body.contains(p));
    }

    #[test]
    fn encode_matches_recording() {
        let recorded = include_bytes!("../tests/fixtures/ssh/level1-nopython.in.bin");
        let first = recorded.iter().position(|&b| b == b'\n').unwrap();
        let prefix = std::str::from_utf8(&recorded[..first]).unwrap();
        assert!(prefix.starts_with('\u{3}'));
        let payload = include_bytes!("../tests/fixtures/ssh/payload.tgz").to_vec();
        let commands = [
            Command::GetShell,
            Command::Setenv { key: "FOO".into(), value: "bar".into() },
            Command::Write { data: payload, dest: "/$HOME/.t".into() },
            Command::Cd("/tmp".into()),
            Command::PythonVersion,
            Command::ExecLoginShell(vec![]),
        ];
        let mut want = format!("{prefix}\n");
        for c in &commands {
            want += &encode(c, prefix);
            want += "\n";
        }
        // After the unhook the recorder typed two lines of its own.
        want += "echo hi\nexit\n";
        assert_eq!(String::from_utf8_lossy(recorded), want);
        assert_eq!(recorded.as_slice(), want.as_bytes());
    }

    fn setup(id: &str) -> Setup {
        Setup { client_unique_id: id.into(), ..Setup::default() }
    }

    #[test]
    fn conductor_basics_follow_iterm2() {
        let root = Conductor::new(setup("P"), None, "U");
        assert_eq!((root.depth(), root.line_prefix()), (0, "\u{3}U"));
        assert!(!root.framing());

        let child = Conductor::new(setup("C"), Some(Box::new(root)), "V");
        assert_eq!(child.depth(), 0, "an unframed parent: the same depth");
        assert_eq!(child.line_prefix(), "\u{3}V");
        assert!(child.ancestry_contains("C") && child.ancestry_contains("P"));
        assert!(!child.ancestry_contains("X"));
        assert!(!child.parent.as_ref().unwrap().ancestry_contains("C"));

        let mut framed = Conductor::new(setup("F"), None, "W");
        framed.framed_pid = Some(1);
        assert!(framed.framing());
        let deeper = Conductor::new(setup("D"), Some(Box::new(framed)), "Y");
        assert_eq!(deeper.depth(), 1, "a framed parent: one deeper");
        let deepest = Conductor::new(setup("E"), Some(Box::new(deeper)), "Z");
        assert_eq!(deepest.depth(), 1, "under an unframed depth-1 parent");

        let parsed = crate::ssh_args::ParsedArgs { host: "h".into(), hostname: "h.example".into(), username: Some("u".into()), port: 2222, ..Default::default() };
        let c = Conductor::new(Setup { parsed: parsed.clone(), ..Setup::default() }, None, "U");
        assert_eq!(c.identity(), parsed.identity());

        let mut root = Conductor::new(setup("P"), None, "U");
        root.write("x");
        assert_eq!(root.take_effects(), [Effect::Write(b"x\n".to_vec())]);
        assert!(root.take_effects().is_empty());
        root.send_keys(b"k");
        assert_eq!(root.take_effects(), [Effect::Keys(b"k".to_vec())]);

        let mut child = Conductor::new(setup("C"), Some(Box::new(root)), "V");
        child.write("y");
        assert_eq!(child.effects, [Effect::Write(b"y\n".to_vec())], "an unframed parent writes raw, into the child's queue");
        assert!(child.parent.as_ref().unwrap().effects.is_empty());
        child.effects.clear();

        child.parent.as_mut().unwrap().effects.push(Effect::StateChanged);
        child.write("z");
        assert_eq!(child.take_effects(), [Effect::StateChanged, Effect::Write(b"z\n".to_vec())]);
        assert!(child.effects.is_empty() && child.parent.as_ref().unwrap().effects.is_empty());
    }

    /// The concatenated `Effect::Write` bytes taken so far.
    fn wire(c: &mut Conductor) -> Vec<u8> {
        c.take_effects()
            .into_iter()
            .filter_map(|e| match e {
                Effect::Write(bytes) => Some(bytes),
                _ => None,
            })
            .flatten()
            .collect()
    }

    /// What `write()` puts on the wire for a command of a conductor with prefix `U`.
    fn sent(cmd: &Command) -> Vec<u8> {
        (encode(cmd, "\u{3}U") + "\n").into_bytes()
    }

    fn cd(dir: &str) -> Command {
        Command::Cd(dir.into())
    }

    fn root() -> Conductor {
        Conductor::new(setup("A"), None, "U")
    }

    #[test]
    fn queue_follows_iterm2() {
        let mut c = root();
        assert_eq!((c.state(), c.description().as_str()), (&State::Ground, "waiting"));
        c.send(cd("/a"), Handler::FailIfNonzero, false);
        assert_eq!(wire(&mut c), sent(&cd("/a")), "ground writes at once");
        assert_eq!(c.description(), "changing directory to /a (preparation stage)");
        assert_eq!(c.in_flight(), Some(&cd("/a")));
        c.send(cd("/b"), Handler::FailIfNonzero, false);
        assert!(wire(&mut c).is_empty(), "one in flight: the next waits");

        let before = c.state().clone();
        c.handle_begin("1", 0);
        assert_eq!(c.state(), &before, "a begin is only logged (CD:2468-2472)");
        assert_eq!(c.description(), "changing directory to /a (preparation stage)");
        c.handle_line("out", 0);
        assert!(matches!(c.state(), State::Executing(ctx, rest) if ctx.command == cd("/a") && rest.is_empty()));
        assert_eq!(c.description(), "changing directory to /a");

        c.handle_end("1", "r", 0, 0);
        assert_eq!(wire(&mut c), sent(&cd("/b")));
        assert_eq!(c.description(), "changing directory to /b (preparation stage)");
        c.handle_end("2", "r", 0, 0);
        assert_eq!(c.description(), "waiting");
        assert_eq!((c.state(), c.in_flight()), (&State::Ground, None));
        assert!(wire(&mut c).is_empty());

        // A high-priority send goes before older queued commands.
        c.send(cd("/c"), Handler::FireAndForget, false);
        c.send(cd("/d"), Handler::FireAndForget, false);
        c.send(cd("/e"), Handler::FireAndForget, true);
        assert_eq!(wire(&mut c), sent(&cd("/c")));
        c.handle_end("3", "r", 0, 0);
        assert_eq!(wire(&mut c), sent(&cd("/e")));
        c.handle_end("4", "r", 0, 0);
        assert_eq!(wire(&mut c), sent(&cd("/d")));
        c.handle_end("5", "r", 0, 0);

        // During a transition a send only enqueues; the transition's own dequeue sends it.
        c.transition = true;
        c.send(cd("/f"), Handler::FireAndForget, false);
        assert!(wire(&mut c).is_empty());
        assert_eq!(c.state(), &State::Ground);
        c.transition = false;
        c.dequeue();
        assert_eq!(wire(&mut c), sent(&cd("/f")));
    }

    #[test]
    fn lines_follow_iterm2() {
        let mut c = root();
        c.send(Command::GetShell, Handler::FireAndForget, false);
        c.take_effects();
        assert!(matches!(c.state(), State::WillExecute(v) if v.len() == 1));
        c.handle_line("/bin/zsh", 0);
        assert!(matches!(c.state(), State::Executing(..)));
        c.handle_line("/home/u", 0);
        let got: Vec<(Command, Partial)> = c.updates.iter().map(|(cmd, p, _)| (cmd.clone(), p.clone())).collect();
        assert_eq!(got, [(Command::GetShell, Partial::Line("/bin/zsh".into())), (Command::GetShell, Partial::Line("/home/u".into()))]);
        assert!(matches!(&c.updates[0].2, State::Executing(..)), "the state moves before the handler sees the line");

        // Ground tolerates lines and ends.
        let mut g = root();
        g.handle_line("stray", 0);
        g.handle_begin("7", 0);
        g.handle_end("7", "r", 1, 0);
        assert_eq!(g.state(), &State::Ground);
        assert!(g.take_effects().is_empty() && g.updates.is_empty());

        // So does unhooked.
        g.handle_unhook();
        g.handle_line("stray", 0);
        g.handle_end("7", "r", 1, 0);
        assert_eq!(g.state(), &State::Unhooked);
        assert!(g.take_effects().is_empty() && g.updates.is_empty());
    }

    #[test]
    fn end_passes_to_the_parent_follows_iterm2() {
        let child_under_busy_parent = || {
            let mut parent = root();
            parent.framed_pid = Some(1);
            parent.send(Command::FramerPoll, Handler::FireAndForget, false);
            assert_eq!(wire(&mut parent), b"cG9sbA==\n\n");
            Conductor::new(setup("C"), Some(Box::new(parent)), "V")
        };

        let mut child = child_under_busy_parent();
        child.handle_end("9", "f", 0, 0);
        let parent = child.parent.as_ref().unwrap();
        assert_eq!((parent.state(), parent.in_flight()), (&State::Ground, None), "a framer end goes to the parent (CD:2487-2496)");
        assert_eq!(parent.updates.iter().map(|(_, p, _)| p.clone()).collect::<Vec<_>>(), [Partial::End(0)]);
        assert_eq!(child.state(), &State::Ground);
        assert!(child.updates.is_empty());

        let mut child = child_under_busy_parent();
        child.handle_end("9", "r", 0, 0);
        let parent = child.parent.as_ref().unwrap();
        assert_eq!(parent.in_flight(), Some(&Command::FramerPoll), "a regular end stays with the child");
        assert!(parent.updates.is_empty());
    }

    #[test]
    fn unhook_follows_iterm2() {
        let mut c = root();
        c.send(cd("/a"), Handler::FailIfNonzero, false);
        c.send(cd("/b"), Handler::FailIfNonzero, false);
        c.take_effects();
        assert!(c.queue_writes());
        c.handle_unhook();
        assert!(c.unhooked());
        assert!(!c.queue_writes());
        assert!(c.queue.is_empty());
        assert_eq!(c.description(), "unhooked");
        assert_eq!(c.in_flight(), None);
        assert_eq!(
            c.updates,
            [(cd("/a"), Partial::Abort, State::Unhooked), (cd("/b"), Partial::Abort, State::Unhooked)],
            "the in-flight command first, each handler after the state is set"
        );
        assert!(c.take_effects().is_empty(), "an aborted FailIfNonzero writes nothing");
        c.send(cd("/c"), Handler::FailIfNonzero, false);
        assert!(wire(&mut c).is_empty());
        assert_eq!(c.state(), &State::Unhooked);
    }

    #[test]
    fn queue_writes_follow_iterm2() {
        let c = root();
        assert!(c.queue_writes());
        assert!(!c.handles_keystrokes(), "unframed");

        let mut parent = root();
        parent.handle_unhook();
        let child = Conductor::new(setup("C"), Some(Box::new(parent)), "V");
        assert!(!child.unhooked());
        assert!(!child.queue_writes(), "transitive: the parent is unhooked (CD:656-670)");

        let mut framed = root();
        framed.framed_pid = Some(7);
        assert!(framed.handles_keystrokes(), "framing and writes queue (CD:700-703)");
        framed.handle_unhook();
        assert!(!framed.handles_keystrokes());

        let mut parent = root();
        parent.handle_unhook();
        let mut child = Conductor::new(setup("C"), Some(Box::new(parent)), "V");
        child.framed_pid = Some(8);
        assert!(!child.handles_keystrokes(), "the parent's unhook stops the child's queueing");
    }

    #[test]
    fn fail_follows_iterm2() {
        let mut parent = root();
        parent.send(cd("/p1"), Handler::FailIfNonzero, false);
        parent.send(cd("/p2"), Handler::FailIfNonzero, false);
        let mut c = Conductor::new(setup("C"), Some(Box::new(parent)), "V");
        c.send(cd("/a"), Handler::FailIfNonzero, false);
        c.send(cd("/b"), Handler::FailIfNonzero, false);
        c.take_effects();

        c.fail("boom");
        assert_eq!(c.take_effects(), [Effect::SessionWrite(b"exec_login_shell\n".to_vec()), Effect::Abort("boom".into())]);
        assert_eq!((c.state(), c.in_flight()), (&State::Ground, None));
        assert!(c.queue.is_empty());
        assert_eq!(c.updates, [(cd("/b"), Partial::Abort, State::Ground)], "the queue is aborted, the in-flight command dropped (CD:3085-3093)");
        assert!(c.queue_writes(), "fail writes outside write(): writes still queue");
        let parent = c.parent.as_ref().unwrap();
        assert_eq!((parent.state(), parent.queue.is_empty()), (&State::Ground, true));
        assert_eq!(parent.updates, [(cd("/p2"), Partial::Abort, State::Ground)], "and the parent's");
    }

    #[test]
    fn fail_keeps_an_unhooked_parent_unhooked() {
        // iTerm2 bug B17 fixed: a nested it2ssh that fails must not make the outer
        // conductor, unhooked at level 1, hold typing again.
        let mut grandparent = root();
        grandparent.send(cd("/g"), Handler::FailIfNonzero, false);
        let mut parent = Conductor::new(setup("P"), Some(Box::new(grandparent)), "W");
        parent.handle_unhook();
        let mut c = Conductor::new(setup("C"), Some(Box::new(parent)), "V");
        c.send(cd("/a"), Handler::FailIfNonzero, false);
        c.take_effects();
        assert!(!c.queue_writes(), "under an unhooked parent writes never queue");

        c.fail("boom");
        let parent = c.parent.as_ref().unwrap();
        assert_eq!(parent.state(), &State::Unhooked, "an unhooked parent stays unhooked");
        let grandparent = parent.parent.as_ref().unwrap();
        assert_eq!(grandparent.state(), &State::Ground, "the force still reaches past it");
        assert!(!c.queue_writes(), "so typing is still not held");
        assert_eq!(c.state(), &State::Ground);
    }

    #[test]
    fn fail_drops_what_abort_handlers_send() {
        // CheckForPython's abort arm sends exec_login_shell; iTerm2 clears the queue after
        // the abort loop (CD:3085-3093), so nothing is written after the failure.
        let mut c = Conductor::new(Setup { initial_directory: Some("/srv".into()), ..setup("A") }, None, "U");
        c.start();
        c.take_effects();
        c.handle_end("1", "r", 0, 0);
        c.take_effects();
        assert_eq!(c.in_flight(), Some(&cd("/srv")));
        assert_eq!(c.queue.iter().map(|q| q.command.clone()).collect::<Vec<_>>(), [Command::PythonVersion]);
        c.handle_end("2", "r", 1, 0);
        assert_eq!(
            c.take_effects(),
            [Effect::StopQueueingInput, Effect::SessionWrite(b"exec_login_shell\n".to_vec()), Effect::Abort("cd /srv: Unepected status 1".into())]
        );
        assert_eq!((c.state(), c.in_flight()), (&State::Ground, None));
        assert!(c.queue.is_empty());

        // The same from outside a handler.
        let mut c = Conductor::new(Setup { initial_directory: Some("/srv".into()), ..setup("A") }, None, "U");
        c.start();
        c.handle_end("1", "r", 0, 0);
        c.take_effects();
        c.fail("boom");
        assert_eq!(c.take_effects(), [Effect::StopQueueingInput, Effect::SessionWrite(b"exec_login_shell\n".to_vec()), Effect::Abort("boom".into())]);
        assert_eq!((c.state(), c.in_flight()), (&State::Ground, None));
        assert!(c.queue.is_empty());
    }

    #[test]
    fn handlers_follow_iterm2() {
        // FailIfNonzero: 0 does nothing, then the next command goes.
        let mut c = root();
        c.send(cd("/a"), Handler::FailIfNonzero, false);
        c.send(cd("/b"), Handler::FailIfNonzero, false);
        c.take_effects();
        c.handle_end("1", "r", 0, 0);
        assert_eq!(c.take_effects(), [Effect::Write(sent(&cd("/b")))]);

        // Non-zero fails, and the fail wins over the dequeue that follows the handler.
        let mut c = root();
        c.send(cd("/a"), Handler::FailIfNonzero, false);
        c.send(cd("/b"), Handler::FailIfNonzero, false);
        c.take_effects();
        c.handle_end("1", "r", 1, 0);
        assert_eq!(unexpected_status(&cd("/a"), 1), "cd /a: Unepected status 1");
        assert_eq!(c.take_effects(), [Effect::SessionWrite(b"exec_login_shell\n".to_vec()), Effect::Abort("cd /a: Unepected status 1".into())]);
        assert_eq!(c.state(), &State::Ground);
        assert!(c.queue.is_empty());

        // NonFramerLogin.
        let login = Command::ExecLoginShell(vec![]);
        let mut c = root();
        c.send(login.clone(), Handler::NonFramerLogin, false);
        c.take_effects();
        c.handle_line("ignored", 0);
        c.handle_end("1", "r", 0, 0);
        assert_eq!(c.take_effects(), [Effect::SendInitialText]);
        c.send(login.clone(), Handler::NonFramerLogin, false);
        c.take_effects();
        c.handle_end("2", "r", 3, 0);
        assert_eq!(c.take_effects(), [Effect::SessionWrite(b"exec_login_shell\n".to_vec()), Effect::Abort("exec_login_shell: Unepected status 3".into())]);

        // WriteOnSuccess: the code, "\nEOF\n" and write()'s own newline (CD:2253, CD:3039-3058).
        let py = Command::RunPython("print(1)".into());
        let mut c = root();
        c.send(py.clone(), Handler::WriteOnSuccess("code".into()), false);
        c.take_effects();
        c.handle_end("1", "r", 0, 0);
        assert_eq!(c.take_effects(), [Effect::Write(b"code\nEOF\n\n".to_vec())]);
        c.send(py.clone(), Handler::WriteOnSuccess("code".into()), false);
        c.take_effects();
        c.handle_end("2", "r", 2, 0);
        assert_eq!(python_status(2), "Status 2 when running python code");
        assert_eq!(c.take_effects(), [Effect::SessionWrite(b"exec_login_shell\n".to_vec()), Effect::Abort("Status 2 when running python code".into())]);

        // Nested under an unframed parent: the parent carries it raw.
        let mut c = Conductor::new(setup("C"), Some(Box::new(root())), "V");
        c.send(py.clone(), Handler::WriteOnSuccess("code".into()), false);
        c.take_effects();
        c.handle_end("1", "r", 0, 0);
        assert_eq!(c.take_effects(), [Effect::Write(b"code\nEOF\n\n".to_vec())]);

        // FireAndForget ignores everything.
        let mut c = root();
        for status in [0, 5] {
            c.send(cd("/x"), Handler::FireAndForget, false);
            c.take_effects();
            c.handle_line("l", 0);
            c.handle_end("1", "r", status, 0);
            assert!(c.take_effects().is_empty());
            assert_eq!(c.state(), &State::Ground);
        }
        c.send(cd("/x"), Handler::FireAndForget, false);
        c.take_effects();
        c.handle_unhook();
        assert!(c.take_effects().is_empty());
    }

    #[test]
    fn cancel_follows_iterm2() {
        let mut c = root();
        c.send(cd("/a"), Handler::FailIfNonzero, false);
        c.send(cd("/b"), Handler::FailIfNonzero, false);
        c.send(Command::Run("x".into()), Handler::FailIfNonzero, false);
        c.take_effects();
        c.cancel_enqueued(&|cmd| matches!(cmd, Command::Cd(_)));
        assert_eq!(c.in_flight(), Some(&cd("/a")), "only queued commands are canceled");
        c.handle_end("1", "r", 0, 0);
        assert_eq!(wire(&mut c), sent(&Command::Run("x".into())), "the canceled command is dropped, the next written");
        let run_in_flight = State::WillExecute(vec![Context { command: Command::Run("x".into()), handler: Handler::FailIfNonzero, canceled: false }]);
        assert_eq!(
            c.updates,
            [(cd("/a"), Partial::End(0), State::WillExecute(vec![Context { command: cd("/a"), handler: Handler::FailIfNonzero, canceled: false }])), (cd("/b"), Partial::Canceled, run_in_flight)],
            "the canceled handler runs once the state is consistent"
        );
    }

    #[test]
    fn shown_texts_hide_environment_values() {
        let setenv = Command::Setenv { key: "TOKEN".into(), value: "s3cret".into() };
        assert_eq!(unexpected_status(&setenv, 1), "setenv TOKEN: Unepected status 1");
        let mut c = root();
        c.send(setenv.clone(), Handler::FailIfNonzero, false);
        assert_eq!(c.description(), "setting TOKEN (preparation stage)");
        c.handle_line("x", 0);
        assert_eq!(c.description(), "setting TOKEN");
        c.take_effects();
        c.handle_end("1", "r", 1, 0);
        let effects = c.take_effects();
        assert_eq!(effects.last(), Some(&Effect::Abort("setenv TOKEN: Unepected status 1".into())));
        assert!(!format!("{effects:?}").contains("s3cret"));
    }

    fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    /// One reply: begin, each line, then the end with `status`.
    fn reply(c: &mut Conductor, lines: &[&str], status: u8) {
        c.handle_begin("1", 0);
        for line in lines {
            c.handle_line(line, 0);
        }
        c.handle_end("1", "r", status, 0);
    }

    /// A started conductor, its start writes taken.
    fn started(setup: Setup) -> Conductor {
        let mut c = Conductor::new(setup, None, "U");
        c.start();
        c.take_effects();
        c
    }

    fn with_command(setup: Setup, command: &[&str]) -> Setup {
        Setup { parsed: ParsedArgs { command_args: s(command), ..ParsedArgs::default() }, ..setup }
    }

    fn zsh_injected() -> Setup {
        Setup { inject: true, ..setup("A") }
    }

    /// The remote home as the injection used it.
    fn home_seen(c: &Conductor) -> Option<String> {
        let vars = c.modified_vars.as_ref()?;
        let (_, dir) = vars.iter().find(|(k, _)| k == "ZDOTDIR")?;
        dir.strip_suffix("/.iterm2/shell-integration").map(str::to_string)
    }

    #[test]
    fn start_follows_iterm2() {
        let mut c = Conductor::new(setup("A"), None, "U");
        c.start();
        assert_eq!(c.take_effects(), [Effect::Write(b"\x03U\n".to_vec()), Effect::Write(sent(&Command::GetShell))]);
        assert_eq!(c.in_flight(), Some(&Command::GetShell));
    }

    #[test]
    fn getshell_follows_iterm2() {
        let base = Setup { vars_to_send: pairs(&[("FOO", "bar"), ("BAZ", "x y")]), initial_directory: Some("/srv".into()), inject: false, ..setup("A") };
        let mut c = started(base.clone());
        reply(&mut c, &["/bin/bash\n/home/u\nGNU bash, version 5.2"], 0);
        assert_eq!(c.shell.as_deref(), Some("/bin/bash"));
        let foo = Command::Setenv { key: "FOO".into(), value: "bar".into() };
        assert_eq!(c.take_effects(), [Effect::StateChanged, Effect::Write(sent(&foo))]);
        // The rest were queued inside the handler; the guard sends them one at a time.
        let baz = Command::Setenv { key: "BAZ".into(), value: "x y".into() };
        assert_eq!(baz.string_value(), "setenv BAZ $'x y'");
        for next in [baz, cd("/srv"), Command::PythonVersion] {
            reply(&mut c, &[], 0);
            assert_eq!(c.take_effects(), [Effect::Write(sent(&next))]);
        }

        // Lines are joined with nothing between them (CD:2323-2325).
        let mut c = started(setup("A"));
        reply(&mut c, &["/bin/sh\n/ho", "me/u\n"], 0);
        assert_eq!(c.shell.as_deref(), Some("/bin/sh"));
        let mut c = started(zsh_injected());
        reply(&mut c, &["/bin/zsh\n/ho", "me/u\n"], 0);
        assert_eq!(home_seen(&c).as_deref(), Some("/home/u"));

        // Spaces and tabs are trimmed.
        let mut c = started(setup("A"));
        reply(&mut c, &[" /bin/sh \n/home/u"], 0);
        assert_eq!(c.shell.as_deref(), Some("/bin/sh"));
        let mut c = started(zsh_injected());
        reply(&mut c, &[" /bin/zsh \n\t/home/u "], 0);
        assert_eq!((c.shell.as_deref(), home_seen(&c).as_deref()), (Some("/bin/zsh"), Some("/home/u")));

        // An explicit command names the shell whatever line 0 says (surprise 17).
        let mut c = started(with_command(setup("A"), &["/usr/local/bin/zsh"]));
        reply(&mut c, &["/bin/bash\n/home/u"], 0);
        assert_eq!(c.shell.as_deref(), Some("/usr/local/bin/zsh"));

        // A non-zero status sends nothing further (iTerm2 bug B6, unreachable because of B1).
        let mut c = started(base);
        reply(&mut c, &["/bin/bash\n/home/u"], 1);
        assert!(c.take_effects().is_empty());
        assert_eq!((c.state(), c.shell.as_deref()), (&State::Ground, None));
    }

    #[test]
    fn getshell_skips_error_lines() {
        // iTerm2 bug B15: conductor.sh calls an undefined using_passwd, and the shell's error
        // comes first in getshell's reply; Tandem takes the first line that starts a shell and
        // home pair of absolute paths.
        let mut c = started(zsh_injected());
        reply(&mut c, &["sh: 160: using_passwd: not found\n/bin/zsh\n/home/user\n"], 0);
        assert_eq!((c.shell.as_deref(), home_seen(&c).as_deref()), (Some("/bin/zsh"), Some("/home/user")));

        // conductor.sh's own fallback "sh" is not an error line.
        let mut c = started(setup("A"));
        reply(&mut c, &["sh\n/home/u\nGNU bash, version 5.2"], 0);
        assert_eq!(c.shell.as_deref(), Some("sh"));
        // With no pair, line 0 is the shell, as iTerm2.
        let mut c = started(setup("A"));
        reply(&mut c, &["sh: 160: using_passwd: not found\nsh\n/home/u"], 0);
        assert_eq!(c.shell.as_deref(), Some("sh: 160: using_passwd: not found"));

        // The real recording, through the parser.
        let recorded = include_bytes!("../tests/fixtures/ssh/level1-nopython.bin");
        let start = b"\x1bP2000p";
        assert!(recorded.starts_with(start));
        let mut parser = crate::conductor_parser::Parser::new("D");
        let mut c = started(Setup { vars_to_send: pairs(&[("FOO", "bar")]), ..setup("A") });
        for token in parser.feed(&recorded[start.len()..]).tokens {
            use crate::conductor_parser::Token;
            match token {
                Token::Begin(id) => c.handle_begin(&id, 0),
                Token::Line(line) => c.handle_line(&line, 0),
                Token::End { id, status, kind } => {
                    c.handle_end(&id, &kind, status, 0);
                    break;
                }
                Token::Init(_) | Token::Unhook => {}
            }
        }
        assert_eq!(c.shell.as_deref(), Some("/bin/sh"));
        let foo = Command::Setenv { key: "FOO".into(), value: "bar".into() };
        assert_eq!(c.take_effects(), [Effect::StateChanged, Effect::Write(sent(&foo))]);
    }

    #[test]
    fn python_rule_follows_iterm2() {
        assert_eq!(MIN_PYTHON, (3, 7));
        assert_eq!(python_version("Python 3.6.9"), Some((3, 6)));
        assert_eq!(python_version("Python 3.10.4"), Some((3, 10)));
        assert_eq!(python_version("Python 4.0"), Some((4, 0)));
        assert_eq!(python_version("Python 2.7.18"), Some((2, 7)));
        assert_eq!(python_version(""), None);
        assert_eq!(python_version("python 3.9"), None);
        assert_eq!(python_version("x\nPython 3.9"), None, "^ is the start of the text");

        let login = Command::ExecLoginShell(vec![]);
        // Reaches pythonversion with nothing else queued.
        let at_python = || {
            let mut c = started(setup("A"));
            reply(&mut c, &["/bin/sh\n/home/u"], 0);
            c.take_effects();
            assert_eq!(c.in_flight(), Some(&Command::PythonVersion));
            c
        };
        let goes_to_login = |c: &mut Conductor, why: &str| {
            assert_eq!(c.take_effects(), [Effect::StopQueueingInput, Effect::Write(sent(&login))], "{why}");
            assert!(!c.python_ok, "{why}");
        };

        let mut c = at_python();
        reply(&mut c, &["Python 3.6.9"], 0);
        goes_to_login(&mut c, "3.6");
        let mut c = at_python();
        reply(&mut c, &["Python 3.12.1"], 1);
        goes_to_login(&mut c, "status 1");
        let mut c = at_python();
        reply(&mut c, &[], 0);
        goes_to_login(&mut c, "no line");

        // Until 14c Task 1 adds execFramer, a usable python also goes to the login shell.
        for ok in ["Python 3.12.1", "Python 3.7"] {
            let mut c = at_python();
            reply(&mut c, &[ok], 0);
            assert!(c.python_ok, "{ok}");
            assert_eq!(c.take_effects(), [Effect::StopQueueingInput, Effect::Write(sent(&login))], "{ok}");
        }

        // Lines are joined with "\n" before the regex (CD:2143-2145).
        let mut c = at_python();
        reply(&mut c, &["Python 3.", "9"], 0);
        assert!(!c.python_ok, "\"Python 3.\\n9\" has no minor digit");
        let mut c = at_python();
        reply(&mut c, &["Python 3.9", "junk"], 0);
        assert!(c.python_ok);

        // An abort goes to the login shell too; unhooked, it stays queued.
        let mut c = at_python();
        c.handle_unhook();
        assert_eq!(c.take_effects(), [Effect::StopQueueingInput]);
        assert_eq!(c.queue.iter().map(|q| q.command.clone()).collect::<Vec<_>>(), [login.clone()]);
    }

    #[test]
    fn exec_login_shell_follows_iterm2() {
        let run = |setup: Setup, modified: Option<Vec<String>>| {
            let mut c = Conductor::new(setup, None, "U");
            c.modified_command_args = modified;
            c.exec_login_shell();
            let effects = c.take_effects();
            let handler = c.in_flight().and_then(|_| match c.state() {
                State::WillExecute(v) => Some(v[0].handler.clone()),
                _ => None,
            });
            (effects, c.in_flight().cloned(), handler)
        };
        let login = Command::ExecLoginShell(vec![]);
        for modified in [None, Some(vec![]), Some(s(&["/bin/bash", "--posix"]))] {
            let (effects, cmd, handler) = run(setup("A"), modified.clone());
            assert_eq!(effects, [Effect::StopQueueingInput, Effect::Write(sent(&login))], "{modified:?}");
            assert_eq!((cmd, handler), (Some(login.clone()), Some(Handler::NonFramerLogin)), "{modified:?}");
        }
        let (effects, cmd, handler) = run(with_command(setup("A"), &["top"]), None);
        let top = Command::Run("top".into());
        assert_eq!(effects, [Effect::StopQueueingInput, Effect::Write(sent(&top))]);
        assert_eq!((cmd, handler), (Some(top), Some(Handler::FailIfNonzero)));
        let (_, cmd, _) = run(with_command(setup("A"), &["ls", "-l"]), None);
        assert_eq!(cmd, Some(Command::Run("ls -l".into())));
        // An empty injected argv wins over the command (CD:2030-2032).
        let (_, cmd, _) = run(with_command(setup("A"), &["top"]), Some(vec![]));
        assert_eq!(cmd, Some(login));
    }

    /// The commands written from now on, answering each with an empty reply and status 0
    /// `replies` times; a write shows its destination only.
    fn commands_sent(c: &mut Conductor, replies: usize) -> Vec<String> {
        let mut out = Vec::new();
        for i in 0..=replies {
            for e in c.take_effects() {
                let Effect::Write(bytes) = e else { continue };
                let text = String::from_utf8(bytes).unwrap();
                let body: String = text.lines().filter_map(|l| l.strip_prefix("\u{3}U")).collect();
                let decoded = String::from_utf8(base64::engine::general_purpose::STANDARD.decode(body).unwrap()).unwrap();
                out.push(match decoded.strip_prefix("write ") {
                    Some(rest) => format!("write {}", rest.split(' ').nth(1).unwrap()),
                    None => decoded,
                });
            }
            if i < replies {
                reply(c, &[], 0);
            }
        }
        out
    }

    #[test]
    fn injection_branch_follows_iterm2() {
        let base = Setup { vars_to_send: pairs(&[("FOO", "bar")]), inject: true, ..setup("A") };
        let mut c = started(base.clone());
        reply(&mut c, &["/usr/bin/zsh\n/home/u\nzsh 5.9"], 0);
        let want_vars = pairs(&[("FOO", "bar"), ("ZDOTDIR", "/home/u/.iterm2/shell-integration"), ("ITERM_INJECT_SHELL_INTEGRATION", "1")]);
        assert_eq!(c.modified_vars.as_ref(), Some(&want_vars));
        assert_eq!(c.modified_command_args, Some(vec![]));
        assert!(!format!("{:?}", c.modified_vars).contains("IT2_SOCK") && !format!("{:?}", c.modified_vars).contains("IT2_NONCE"), "no it2 proxy (ruling 3)");
        assert_eq!(
            commands_sent(&mut c, 6),
            [
                "setenv FOO bar",
                "setenv ZDOTDIR /home/u/.iterm2/shell-integration",
                "setenv ITERM_INJECT_SHELL_INTEGRATION 1",
                "write /$HOME/.iterm2/shell-integration",
                "write /$HOME/.iterm2/shell-integration/fish/vendor_conf.d",
                "pythonversion",
                "exec_login_shell",
            ]
        );

        // bash: --posix goes in and the shell becomes argv[0]; HISTFILE is the remote home's.
        let bash = "GNU bash, version 5.1.16(1)-release (x86_64-pc-linux-gnu)";
        let mut c = started(base.clone());
        reply(&mut c, &[&format!("/bin/bash\n/home/u\n{bash}")], 0);
        assert_eq!(c.modified_command_args, Some(s(&["/bin/bash", "--posix"])));
        let vars = c.modified_vars.clone().unwrap();
        assert!(vars.contains(&("HISTFILE".into(), "/home/u/.bash_history".into())), "{vars:?}");
        assert_eq!(vars[0], ("FOO".into(), "bar".into()));
        // With a command, its first word is put back as argv[0] (CD:2329-2333).
        let mut c = started(with_command(base.clone(), &["/bin/bash", "-l"]));
        reply(&mut c, &[&format!("/usr/bin/zsh\n/home/u\n{bash}")], 0);
        assert_eq!(c.shell.as_deref(), Some("/bin/bash"));
        assert_eq!(c.modified_command_args, Some(s(&["/bin/bash", "--posix", "-l"])));

        // No injection: macOS bash, tcsh, no home, or injection off.
        let cases = [
            (base.clone(), "/bin/bash\n/Users/u\nGNU bash, version 3.2.57(1)-release (arm64-apple-darwin23)"),
            (base.clone(), "/bin/tcsh\n/home/u\ntcsh 6.24"),
            (base.clone(), "/usr/bin/zsh\n\nzsh 5.9"),
            (Setup { inject: false, ..base.clone() }, "/usr/bin/zsh\n/home/u\nzsh 5.9"),
        ];
        for (setup, text) in cases {
            let mut c = started(setup);
            reply(&mut c, &[text], 0);
            assert_eq!((&c.modified_vars, &c.modified_command_args), (&None, &None), "{text}");
            assert!(c.payloads.is_empty(), "{text}");
            let foo = Command::Setenv { key: "FOO".into(), value: "bar".into() };
            assert_eq!(c.take_effects(), [Effect::StateChanged, Effect::Write(sent(&foo))], "{text}");
        }
    }

    #[test]
    fn payload_order_follows_iterm2() {
        let home = std::env::temp_dir().join(format!("tandem-conductor-payloads-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join(".vimrc"), b"set nu\n").unwrap();

        let mut c = Conductor::new(Setup { inject: true, ..setup("A") }, None, "U");
        c.add_path("~/.vimrc", "~/", &home);
        c.add_path("~/.vimrc", "x/y/", &home);
        assert_eq!(
            c.payloads,
            [
                Payload { source: Source::Disk(home.join(".vimrc")), destination: "/$HOME".into() },
                Payload { source: Source::Disk(home.join(".vimrc")), destination: "/$HOME/y".into() },
            ],
            "tweak_destination (iTerm2 bug B8 kept) and expand_local_tilde"
        );
        c.payloads.pop();
        c.start();
        c.take_effects();
        reply(&mut c, &["/usr/bin/zsh\n/home/u\nzsh 5.9"], 0);
        let order: Vec<String> = commands_sent(&mut c, 6).into_iter().filter(|w| w.starts_with("write ")).collect();
        assert_eq!(order, ["write /$HOME", "write /$HOME/.iterm2/shell-integration", "write /$HOME/.iterm2/shell-integration/fish/vendor_conf.d"]);
        let _ = std::fs::remove_dir_all(&home);
    }
}

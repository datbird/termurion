//! iTerm2's Conductor (`CD`, milestone 14b): the commands it sends to conductor.sh and
//! framer.py with their wire encoding, and the conductor object with its parent chain.
//! Pure: it talks to the app only through the `Effect`s it queues (the tmux gateway's
//! `take_writes` pattern).

use crate::conductor_framer::{Job, Purpose, TtyState, REPLY_MAX};
use crate::conductor_parser::Init;
use crate::ssh_args::{HostnameFinder, Identity, ParsedArgs};
use crate::ssh_inject::{expand_local_tilde, injection_payloads, last_component, modify_remote, shell_supports_injection, tarballs, tweak_destination, Payload, Source, INTEGRATION_SUBDIR};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};
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

    /// supportsPipelining (`CD:1375-1386`): several may be written before the first answers.
    /// 14d adds the file fetch; iTerm2's `framerIT2Send` is not built (ruling 3).
    pub fn supports_pipelining(&self) -> bool {
        matches!(self, Command::FramerSend { .. })
    }

    /// size (`CD:1388-1416`): about the bytes on the wire, from the base64 form. Only a
    /// command that pipelines has a size; the others are `usize::MAX`.
    pub fn size(&self) -> usize {
        match self {
            Command::FramerSend { .. } => self.string_value().chars().count() * 4 / 3,
            _ => usize::MAX,
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

    /// Whether a command written while framing keeps the command timeout (ruling M14-I2: a
    /// sleeping laptop must not end SSH integration). None in 14b; 14d's file commands but
    /// `file zip` add the true arm.
    pub fn timed_when_framed(&self) -> bool {
        false
    }
}

/// How long a timed command may go without a begin, a line or its end (Tandem's; iTerm2
/// waits forever, ruling 2), and how long a setup stalled in ground may wait.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// Seconds added per square MiB of a command's encoded bytes. conductor.sh reads a command
/// one 128-character line at a time and appends each to the text so far
/// (`line="${line}${part}"`, `C:440-456`), so reading it costs the square of its size, and
/// its begin comes only after the last line. Measured with conductor.sh's own loop: dash
/// about 10 s and bash about 142 s per square MiB (a 1 MiB file is 1.78 MiB encoded: 29 s
/// and 451 s; a 4 MiB file, 7.1 MiB: 549 s under dash); this is about twice bash's, for
/// slower hosts.
pub const WRITE_COST_SECS: f64 = 300.0;

/// The timeout of a command `encoded_len` bytes long on the wire: `COMMAND_TIMEOUT` plus
/// `WRITE_COST_SECS` per square MiB, whole seconds rounded down, saturating (the cast and the
/// sum) rather than overflowing. Only Files to Copy's writes grow past a few KiB.
pub fn command_timeout(encoded_len: usize) -> Duration {
    let mib = encoded_len as f64 / (1024.0 * 1024.0);
    COMMAND_TIMEOUT.saturating_add(Duration::from_secs((WRITE_COST_SECS * mib * mib) as u64))
}

/// When a command timed from `t` for `timeout` is due: never (None) when that is past what an
/// `Instant` holds, rather than a panic.
fn due_at(t: Instant, timeout: Duration) -> Option<Instant> {
    t.checked_add(timeout)
}

/// The first line of the abort text (`PS:25594`).
pub const SETUP_ERROR: &str = "An error occurred while setting up the SSH environment:";

/// The failure text for a command the remote stopped answering for `timeout` (Tandem's),
/// built from its description so a tarball never lands on the screen, and a setenv value
/// never does.
pub fn timeout_text(cmd: &Command, timeout: Duration) -> String {
    format!("{}: No response after {} seconds", shown_description(cmd), timeout.as_secs())
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
    /// A message for the pane (a Files to Copy job that was skipped).
    Notice(String),
    Abort(String),
    /// The conductor framed and joins the connected hosts (`CD:293-302`). The effect carries
    /// its guid and identity because a parent's effects surface through its child's queue.
    Registered { guid: String, identity: Identity },
    /// A framed conductor failed and leaves the connected hosts (`CD:3095-3103`).
    Deregistered { guid: String },
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
    /// A framer `run`: its reply line is the child's pid, which becomes a background job
    /// (`CD:2259-2271`); the end, an abort and a cancel do nothing.
    RunRemote(Purpose),
    /// The framer's `login` reply: its lines are the shell's pid, or the failure text
    /// (`CD:2216-2225`); the end finishes framing.
    FramerLogin(Vec<String>),
    /// A framer `getenv`: the first line is the value of the named variable (`CD:2236-2246`).
    /// The variable's name and the first reply line: `None` before one arrived, `Some(None)`
    /// when it was too long to keep.
    Getenv(String, Option<Option<String>>),
}

/// The most a pipeline holds before it stops taking commands (`CD:2986-3010`).
pub const PIPELINE_BYTES: usize = 1024;

/// A command with its handler (ExecutionContext, `CD:1159-1164`).
#[derive(Clone, Debug, PartialEq)]
pub struct Context {
    pub command: Command,
    pub handler: Handler,
    canceled: bool,
    /// Whether the conductor was framing when this command was written.
    framed_at_write: bool,
    /// Its timeout (`command_timeout` of its size), set when it is written.
    timeout: Duration,
}

impl Context {
    /// Whether the command timeout applies while it is in flight (ruling M14-I2): written
    /// before framing and not streaming, or written while framing and timed even then.
    fn timed(&self) -> bool {
        if self.framed_at_write {
            self.command.timed_when_framed()
        } else {
            !self.command.streams()
        }
    }
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
/// iTerm2's typo kept). A setenv shows its name only: the value may be a secret. A write
/// shows its destination only, never the base64 tarball (up to about 85 MiB of the
/// person's files, where iTerm2 prints it all).
pub fn unexpected_status(cmd: &Command, status: u8) -> String {
    let shown = match cmd {
        Command::Setenv { key, .. } => format!("setenv {key}"),
        Command::Write { dest, .. } => format!("write {dest}"),
        Command::ExecLoginShell(args) if !args.is_empty() => Command::ExecLoginShell(shell_by_name(args)).string_value(),
        cmd => cmd.string_value(),
    };
    format!("{shown}: Unepected status {status}")
}

/// A command's operationDescription as the person sees it: a setenv names the variable
/// only, since the value may be a secret, and a login shell's argv[0] is the shell's name
/// only (the remote reported the path).
fn shown_description(cmd: &Command) -> String {
    match cmd {
        Command::Setenv { key, .. } => format!("setting {key}"),
        Command::ExecLoginShell(args) if !args.is_empty() => Command::ExecLoginShell(shell_by_name(args)).description(),
        cmd => cmd.description(),
    }
}

/// A login shell's argv with argv[0] cut to the shell's name (its last path component).
fn shell_by_name(args: &[String]) -> Vec<String> {
    let mut args = args.to_vec();
    if let Some(shell) = args.first_mut() {
        *shell = last_component(shell).to_string();
    }
    args
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

/// What getshell and pythonversion keep of a reply, in bytes and lines (Tandem's: iTerm2 keeps
/// it all). The remote is not trusted, and the shell and version sit in the first lines; past
/// either limit the step fails.
pub const GETSHELL_LIMIT: (usize, usize) = (64 * 1024, 256);
pub const PYTHON_LIMIT: (usize, usize) = (4 * 1024, 64);
/// The login reply is a pid, or a short failure text (Tandem's).
pub const LOGIN_LIMIT: (usize, usize) = (REPLY_MAX, 64);

/// The failure text for a reply past its limit (Tandem's).
pub fn reply_too_long(cmd: &Command) -> String {
    format!("{}: The reply is too long", cmd.string_value())
}

/// Whether getshell's `line` can be a shell: an absolute path or a bare command name, with no
/// whitespace, quotes, control characters or `=` (controller ruling). A shell error from the
/// real it2ssh starts with its `sh -c` script, which holds the origin token and the base64
/// environment, so such a line is never taken; nor is the shell value shown or logged.
fn plausible_shell(line: &str) -> bool {
    let bad = |c: char| c.is_whitespace() || c.is_control() || matches!(c, '"' | '\'' | '`' | '=');
    !line.is_empty() && !line.chars().any(bad) && (line.starts_with('/') || !line.contains('/'))
}

/// Where getshell's reply starts (iTerm2 bug B15 guarded): conductor.sh's guess_login_shell
/// can print a shell error ("sh: 160: using_passwd: not found") before the shell, so the
/// first line that starts a pair of absolute paths (a plausible shell, then home) is taken;
/// with no such pair, line 0 as iTerm2 (conductor.sh's fallback shell is the bare "sh").
fn shell_line(parts: &[&str]) -> usize {
    parts.windows(2).position(|pair| pair[0].starts_with('/') && plausible_shell(pair[0]) && pair[1].starts_with('/')).unwrap_or(0)
}

/// A hook's setup (`PS:20118-20170`): `env` is every variable Env reported, in stored order
/// (`Screen::exfiltrated(None)`), `env_names` the profile's Environment Variables to Copy. A
/// variable reported twice keeps its first place and its last value, as iTerm2's dictionary
/// (`VT100Screen.m:1941-1949`); only a local origin sends any, in the configured order
/// (Decision: iTerm2's dictionary order is arbitrary).
pub fn setup_for_hook(init: &Init, local: bool, env: &[(String, String)], env_names: &[String], directory: Option<String>, inject: bool, finder: &dyn HostnameFinder) -> Setup {
    let mut client_vars: Vec<(String, String)> = Vec::new();
    for (name, value) in env {
        match client_vars.iter_mut().find(|(n, _)| n == name) {
            Some(kept) => kept.1 = value.clone(),
            None => client_vars.push((name.clone(), value.clone())),
        }
    }
    let mut vars_to_send: Vec<(String, String)> = Vec::new();
    if local {
        for name in env_names {
            if vars_to_send.iter().any(|(n, _)| n == name) {
                continue;
            }
            if let Some(var) = client_vars.iter().find(|(n, _)| n == name) {
                vars_to_send.push(var.clone());
            }
        }
    }
    Setup {
        sshargs: init.sshargs.clone(),
        bool_args: init.bool_args.clone(),
        dcs_id: init.dcs_id.clone(),
        client_unique_id: init.unique_id.clone(),
        vars_to_send,
        client_vars,
        initial_directory: directory,
        inject,
        parsed: crate::ssh_args::parse(&init.sshargs, &init.bool_args, finder),
    }
}

/// The banner a hook appends (`TD:3860`), cut to `MAX_SHOWN` (the remote sends the args).
pub fn hook_banner(sshargs: &str) -> String {
    shown(&format!("ssh {sshargs}"))
}

/// The most characters of a remote-sized text (the hook banner, a failure reason naming the
/// remote's ssh args) Tandem shows (Tandem's: iTerm2 shows it all, up to its 1 MiB DCS cap).
pub const MAX_SHOWN: usize = 1024;

/// `text` cut to `MAX_SHOWN` characters, with an ellipsis when cut.
pub fn shown(text: &str) -> String {
    match text.char_indices().nth(MAX_SHOWN) {
        Some((at, _)) => format!("{}\u{2026}", &text[..at]),
        None => text.to_string(),
    }
}

/// The depth of a conductor hooked while `parent` is the pane's current one (`CD:512-518`):
/// the parent's, one more when the parent frames (an unframed parent does not wrap input
/// for it); 0 with none.
pub fn child_depth(parent: Option<&Conductor>) -> i32 {
    match parent {
        Some(p) if p.framing() => p.depth + 1,
        Some(p) => p.depth,
        None => 0,
    }
}

/// Takes the pane's innermost conductor; its parent becomes current (`PS:20199`).
pub fn pop(chain: &mut Option<Box<Conductor>>) -> Option<Box<Conductor>> {
    let mut innermost = chain.take()?;
    *chain = innermost.parent.take();
    Some(innermost)
}

/// The SSH integration level of a pane whose current conductor is `c` (`PS:25624-25636`):
/// 0 with none, 1 while not framing, 2 while framing.
pub fn level(c: Option<&Conductor>) -> u8 {
    match c {
        None => 0,
        Some(c) if c.framing() => 2,
        Some(_) => 1,
    }
}

/// Where a write to the session goes while `c` is the pane's current conductor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// To the pty.
    Direct,
    /// Held until the conductor unhooks (`_sshWriteQueue`).
    Held,
    /// Through the conductor's `send_keys` (framed from milestone 14c).
    Conductor,
}

/// handlesKeystrokes gives Conductor (`PS:4278-4293`, `PS:4411-4432`), queueWrites gives
/// Held (`PS:4191-4197`), else Direct.
pub fn route(c: Option<&Conductor>) -> Route {
    match c {
        Some(c) if c.handles_keystrokes() => Route::Conductor,
        Some(c) if c.queue_writes() => Route::Held,
        _ => Route::Direct,
    }
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
    /// Set by `exec_framer` (14c, `conductor_framer.rs`).
    pub(crate) framer_version: Option<u8>,
    /// The `%it2` frames this conductor owns, the newest 16, each cut at `IT2_ENTRY_MAX`.
    /// Nothing reads them: the it2 proxy is n/a (ruling 3) and Tandem never sends `it2listen`.
    pub it2_inbox: Vec<String>,
    /// The remote's `uname` output, trimmed (`CD:1912-1917`).
    pub uname: Option<String>,
    /// The name `hostname -f` printed, trimmed (`CD:2198-2214`).
    pub discovered_hostname: Option<String>,
    /// The commands running in the background on the remote, by pid (`CD:471`); not restorable.
    pub background_jobs: BTreeMap<i32, Job>,
    /// The remote tty's echo and icanon, from `%notif tty` (`CD:672-679`).
    pub tty: TtyState,
    /// This conductor's own id in the connected hosts (iTerm2's `guid`).
    pub guid: String,
    /// What `getenv` learned about the remote's environment (`CD:2236-2246`).
    pub(crate) environment_variables: BTreeMap<String, String>,
    /// Whether framing starts the remote process list (`CD:381`, always true).
    pub autopoll_enabled: bool,
    /// The command whose output is the remote's `uname` (the `unameCommand` setting).
    pub uname_command: String,
    /// Every background job completion, for the tests.
    #[cfg(test)]
    pub(crate) completed: Vec<(Purpose, Vec<u8>, i32)>,
    line_prefix: String,
    depth: i32,
    /// Set by `fail`: the pane's abort takes this conductor, and every one inside it, out.
    failed: bool,
    pub(crate) effects: Vec<Effect>,
    state: State,
    queue: Vec<Context>,
    /// Set while a handler runs and while commands are written: `send` only enqueues, and
    /// the transition's own dequeue sends it next (`CD:384-391`, `CD:2964-2969`).
    transition: bool,
    /// When the conductor last wrote a command or heard a begin, a line or an end for one.
    /// None before the first write and after a failure.
    last_activity: Option<Instant>,
    /// The timeout text of the last command written, for a setup stalled in ground.
    last_written: Option<String>,
    /// The clock for tests: `now()` reads this when set.
    pub(crate) fake_now: Option<Instant>,
    /// Every handler call with the state at that moment, for the tests.
    #[cfg(test)]
    updates: Vec<(Command, Partial, State)>,
}

impl Conductor {
    /// A conductor for a hook (`CD:484-517`): `\u{3}` plus `prefix_uuid` starts each
    /// pre-framer line; the depth is the parent's, one more when the parent frames (an
    /// unframed parent does not wrap input for it).
    pub fn new(setup: Setup, parent: Option<Box<Conductor>>, prefix_uuid: &str) -> Conductor {
        let depth = child_depth(parent.as_deref());
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
            framer_version: None,
            it2_inbox: Vec::new(),
            uname: None,
            discovered_hostname: None,
            background_jobs: BTreeMap::new(),
            tty: TtyState::default(),
            guid: uuid::Uuid::new_v4().hyphenated().to_string(),
            environment_variables: BTreeMap::new(),
            autopoll_enabled: true,
            uname_command: "uname -s".to_string(),
            #[cfg(test)]
            completed: Vec::new(),
            line_prefix: format!("\u{3}{prefix_uuid}"),
            depth,
            failed: false,
            effects: Vec::new(),
            state: State::Ground,
            queue: Vec::new(),
            transition: false,
            last_activity: None,
            last_written: None,
            fake_now: None,
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
        // A line that cannot be a shell leaves it unknown: nothing is injected.
        let Some(shell) = command_args.first().cloned().or_else(|| plausible_shell(parts[0]).then(|| parts[0].to_string())) else {
            self.effects.push(Effect::StateChanged);
            self.did_finish_get_shell();
            return;
        };
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
        let (packed, skipped) = tarballs(&self.payloads);
        self.effects.extend(skipped.into_iter().map(Effect::Notice));
        for (data, dest) in packed {
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

    /// A remote python of `MIN_PYTHON` or later starts the framer (ruling 14b-6).
    fn python_ready(&mut self) {
        self.python_ok = true;
        self.frame();
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

    /// Whether this conductor failed (`fail`) and waits for the pane's abort.
    pub fn failed(&self) -> bool {
        self.failed
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

    /// Whether `take_effects` would give anything.
    pub fn has_effects(&self) -> bool {
        !self.effects.is_empty() || self.parent.as_ref().is_some_and(|p| p.has_effects())
    }

    /// sendKeys (`CD:1854-1862`): framed, `send <pid> <base64>` fire and forget, pipelined
    /// (`framerSend`); unframed, the bytes go to the session, which holds them while setup
    /// runs. Nothing to send sends nothing (F18: the login's empty connecting queue).
    pub fn send_keys(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        match self.framed_pid {
            Some(pid) => self.send(Command::FramerSend { data: data.to_vec(), pid }, Handler::FireAndForget, false),
            None => self.effects.push(Effect::Keys(data.to_vec())),
        }
    }

    /// The conductor's own bytes as a parent carries them for a child's `write()`
    /// (`CD:3039-3058`: the parent's `sendKeys`): a framing parent sends them to its framed
    /// shell; an unframed one writes them raw (`_queueWrites` cleared, never held).
    fn write_through(&mut self, bytes: Vec<u8>) {
        if self.framing() {
            self.send_keys(&bytes);
        } else {
            self.effects.push(Effect::Write(bytes));
        }
    }

    /// Runs `f` on the parent, if any, and moves what the parent queues meanwhile into this
    /// conductor's queue after its own, so the effects keep the order they were made in.
    pub(crate) fn via_parent(&mut self, f: impl FnOnce(&mut Conductor)) {
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

    /// Whether `pred` holds for the command in flight or one queued.
    pub(crate) fn has_command(&self, pred: &dyn Fn(&Command) -> bool) -> bool {
        self.in_flight().is_some_and(pred) || self.queue.iter().any(|c| pred(&c.command))
    }

    /// The commands written and not yet ended (a pipeline, first to last), then the queued
    /// ones; `in_flight` is the first of them.
    pub fn pending(&self) -> Vec<&Command> {
        let written: Vec<&Context> = match &self.state {
            State::WillExecute(contexts) => contexts.iter().collect(),
            State::Executing(context, rest) => std::iter::once(context).chain(rest).collect(),
            State::Ground | State::Unhooked => Vec::new(),
        };
        written.into_iter().chain(&self.queue).map(|c| &c.command).collect()
    }

    /// The bytes of keystrokes waiting in the queue (not yet written), for a coprocess's flow
    /// control.
    pub fn queued_bytes(&self) -> usize {
        self.queue
            .iter()
            .map(|c| match &c.command {
                Command::FramerSend { data, .. } => data.len(),
                _ => 0,
            })
            .sum()
    }

    /// The command written and not yet ended, for the command timer (Task 9) and tests.
    pub fn in_flight(&self) -> Option<&Command> {
        self.in_flight_context().map(|c| &c.command)
    }

    fn in_flight_context(&self) -> Option<&Context> {
        match &self.state {
            State::WillExecute(contexts) => contexts.first(),
            State::Executing(context, _) => Some(context),
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
    /// is written at once. During a transition it only enqueues. In flight, the pipeline is
    /// topped up with what can ride along (`extend`): a keystroke need not wait for the
    /// round trip of the one before it. Unhooked, it stays queued, unsent.
    pub fn send(&mut self, cmd: Command, handler: Handler, high_priority: bool) {
        let context = Context { command: cmd, handler, canceled: false, framed_at_write: false, timeout: COMMAND_TIMEOUT };
        if high_priority {
            self.queue.insert(0, context);
        } else {
            self.queue.push(context);
        }
        if self.transition {
            return;
        }
        match self.state.clone() {
            State::Ground => self.dequeue(),
            State::WillExecute(written) => self.extend(written, |c, contexts| c.state = State::WillExecute(contexts)),
            // The first has had its begin, so it stays the executing one and the new
            // commands join those behind it.
            State::Executing(first, rest) => {
                let written = std::iter::once(first).chain(rest).collect();
                self.extend(written, |c, contexts| {
                    let mut contexts = contexts.into_iter();
                    if let Some(first) = contexts.next() {
                        c.state = State::Executing(first, contexts.collect());
                    }
                });
            }
            State::Unhooked => {}
        }
    }

    /// cancelEnqueuedRequests (`CD:2848-2858`): matching queued commands are dropped with
    /// `Partial::Canceled` when their turn comes.
    pub fn cancel_enqueued(&mut self, pred: &dyn Fn(&Command) -> bool) {
        for context in self.queue.iter_mut().filter(|c| pred(&c.command)) {
            context.canceled = true;
        }
    }

    /// dequeue from ground (`CD:2860-2869`): a pipeline of what is queued.
    fn dequeue(&mut self) {
        debug_assert_eq!(self.state, State::Ground);
        self.extend(Vec::new(), |c, contexts| c.state = State::WillExecute(contexts));
    }

    /// extendPipeline (`CD:2888-2960`) and takeNextContextPipeline (`CD:2986-3010`): takes
    /// what can ride along behind the `written` pipeline (nothing when a written command does
    /// not pipeline; from ground, the first command whatever it is), while the total size is
    /// under `PIPELINE_BYTES` and each next command pipelines. `set` puts the whole pipeline
    /// in the state before anything is written, inside the transition guard; canceled contexts
    /// are dropped and their handlers told last, once the state is consistent.
    fn extend(&mut self, written: Vec<Context>, set: impl FnOnce(&mut Conductor, Vec<Context>)) {
        if !written.iter().all(|c| c.command.supports_pipelining()) {
            return;
        }
        let from_ground = written.is_empty();
        let mut size: usize = written.iter().map(|c| c.command.size()).sum();
        let mut pipeline = written;
        let already = pipeline.len();
        let mut dropped = Vec::new();
        let mut texts = Vec::new();
        while size < PIPELINE_BYTES {
            while self.queue.first().is_some_and(|c| c.canceled) {
                dropped.push((self.queue.remove(0), Partial::Canceled));
            }
            let Some(next) = self.queue.first() else { break };
            if !pipeline.is_empty() && !next.command.supports_pipelining() {
                break;
            }
            let mut context = self.queue.remove(0);
            context.framed_at_write = self.framing();
            let text = encode(&context.command, &self.line_prefix);
            context.timeout = command_timeout(text.len());
            // A stall in ground after it waits COMMAND_TIMEOUT, whatever its size.
            self.last_written = Some(timeout_text(&context.command, COMMAND_TIMEOUT));
            let pipelines = context.command.supports_pipelining();
            if pipelines {
                size += context.command.size();
            }
            pipeline.push(context);
            texts.push(text);
            if !pipelines {
                break;
            }
        }
        if pipeline.len() > already {
            if from_ground {
                self.last_activity = Some(self.now());
            }
            self.in_transition(|c| {
                set(c, pipeline);
                for text in &texts {
                    c.write(text);
                }
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

    /// handleCommandBegin (`CD:2468-2472`): iTerm2 only logs it; Tandem's command clock
    /// restarts. A begin from a deeper framed conductor is its parent's, as a line.
    pub fn handle_begin(&mut self, id: &str, depth: i32) {
        if depth != self.depth && self.framing() {
            self.via_parent(|parent| parent.handle_begin(id, depth));
            return;
        }
        if self.in_flight().is_some() {
            self.last_activity = Some(self.now());
        }
    }

    /// handle(line:depth:) (`CD:2413-2433`).
    pub fn handle_line(&mut self, line: &str, depth: i32) {
        if depth != self.depth && self.framing() {
            self.via_parent(|parent| parent.handle_line(line, depth));
            return;
        }
        match std::mem::replace(&mut self.state, State::Ground) {
            State::WillExecute(mut contexts) => {
                let first = contexts.remove(0);
                self.state = State::Executing(first, contexts);
            }
            executing @ State::Executing(..) => self.state = executing,
            // Ground and unhooked tolerate stray input.
            other => {
                self.state = other;
                return;
            }
        }
        self.last_activity = Some(self.now());
        #[cfg(test)]
        if let State::Executing(context, _) = &self.state {
            self.updates.push((context.command.clone(), Partial::Line(line.to_string()), self.state.clone()));
        }
        self.collect_line(line);
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
        // Before the handler, so a failure it causes clears the clock.
        self.last_activity = Some(self.now());
        self.in_transition(|c| c.update(&context, Partial::End(status)));
        if pending.is_empty() {
            self.state = State::Ground;
            self.dequeue();
        } else {
            // amendPipeline: what was behind is still written; more may ride along.
            self.state = State::WillExecute(pending.clone());
            self.extend(pending, |c, contexts| c.state = State::WillExecute(contexts));
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
        self.failed = true;
        self.force_ground();
        // Nothing more is expected of a failed conductor: no deadline, one abort.
        self.last_activity = None;
        // A framed one leaves the connected hosts first (`CD:3095-3103`).
        if self.framing() {
            self.effects.push(Effect::Deregistered { guid: self.guid.clone() });
        }
        let line = Command::ExecLoginShell(Vec::new()).string_value() + "\n";
        self.effects.push(Effect::SessionWrite(line.into_bytes()));
        self.effects.push(Effect::Abort(reason.to_string()));
    }

    fn now(&self) -> Instant {
        self.fake_now.unwrap_or_else(Instant::now)
    }

    /// When `tick` fails the conductor if nothing happens first: the in-flight command's
    /// last activity plus its timeout (`command_timeout`) when it is timed; in ground, the
    /// last activity plus `COMMAND_TIMEOUT` while the conductor does not frame (iTerm2 bugs B5
    /// and B6: a setup stalled there holds typing, or its hook swallows output, forever);
    /// else none.
    pub fn deadline(&self) -> Option<Instant> {
        let timeout = match &self.state {
            State::WillExecute(_) | State::Executing(..) => self.in_flight_context().filter(|c| c.timed()).map(|c| c.timeout),
            // A nested setup under an unhooked parent holds no writes, but the pane's DCS hook
            // still takes its output (final review Minor 1): it is timed as well.
            State::Ground => (!self.framing()).then_some(COMMAND_TIMEOUT),
            State::Unhooked => None,
        };
        self.last_activity.zip(timeout).and_then(|(t, timeout)| due_at(t, timeout))
    }

    /// The command timer: at or past the deadline, setup fails with the timeout text of the
    /// command in flight, or in ground of the last command written. A pre-framer command's
    /// late `end` would otherwise be credited to the next command.
    pub fn tick(&mut self, now: Instant) {
        if !self.deadline().is_some_and(|due| now >= due) {
            return;
        }
        let reason = match self.in_flight_context() {
            Some(context) => timeout_text(&context.command, context.timeout),
            None => self.last_written.clone().unwrap_or_default(),
        };
        self.fail(&reason);
    }

    /// The in-flight command's reply line, kept in place by its handler (iTerm2's StringArray
    /// is a reference), up to the handler's limit; past it the step fails.
    fn collect_line(&mut self, line: &str) {
        let State::Executing(context, _) = &mut self.state else { return };
        if let Handler::RunRemote(purpose) = &context.handler {
            let (purpose, command) = (purpose.clone(), context.command.clone());
            self.add_background_job(line, command, purpose);
            return;
        }
        let (lines, (max_bytes, max_lines)) = match &mut context.handler {
            Handler::GetShell(lines) => (lines, GETSHELL_LIMIT),
            Handler::CheckForPython(lines) => (lines, PYTHON_LIMIT),
            Handler::FramerLogin(lines) => (lines, LOGIN_LIMIT),
            // Only the first line is the value; a value past the limit is not kept (F13).
            Handler::Getenv(_, first) => {
                if first.is_none() {
                    *first = Some((line.len() <= REPLY_MAX).then(|| line.to_string()));
                }
                return;
            }
            _ => return,
        };
        let kept: usize = lines.iter().map(String::len).sum();
        if lines.len() < max_lines && kept + line.len() <= max_bytes {
            lines.push(line.to_string());
            return;
        }
        let reason = reply_too_long(&context.command);
        self.fail(&reason);
    }

    /// update(executionContext:result:) (`CD:2106-2396`) for the 14b handlers.
    fn update(&mut self, context: &Context, result: Partial) {
        #[cfg(test)]
        self.updates.push((context.command.clone(), result.clone(), self.state.clone()));
        let status = match result {
            Partial::End(status) => status,
            // Lines are kept by `collect_line`, never through a copy of the context.
            Partial::Line(_) => return,
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
            Handler::RunRemote(_) => {}
            Handler::FramerLogin(lines) => self.finalize_framing(status, lines),
            Handler::Getenv(name, first) => {
                if let Some(Some(line)) = first {
                    self.environment_variables.insert(name.clone(), line.clone());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Command::Cd;

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
    fn routes_follow_iterm2() {
        assert_eq!(route(None), Route::Direct);
        let mut c = root();
        assert_eq!(route(Some(&c)), Route::Held, "setting up");
        c.framed_pid = Some(1);
        assert_eq!(route(Some(&c)), Route::Conductor, "framing with writes queued");
        c.handle_unhook();
        assert_eq!(route(Some(&c)), Route::Direct, "unhooked");
        let mut parent = root();
        parent.handle_unhook();
        let child = Conductor::new(setup("B"), Some(Box::new(parent)), "V");
        assert_eq!(route(Some(&child)), Route::Direct, "a child setting up under an unhooked parent");
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
        let run_in_flight = State::WillExecute(vec![Context { command: Command::Run("x".into()), handler: Handler::FailIfNonzero, canceled: false, framed_at_write: false, timeout: COMMAND_TIMEOUT }]);
        assert_eq!(
            c.updates,
            [(cd("/a"), Partial::End(0), State::WillExecute(vec![Context { command: cd("/a"), handler: Handler::FailIfNonzero, canceled: false, framed_at_write: false, timeout: COMMAND_TIMEOUT }])), (cd("/b"), Partial::Canceled, run_in_flight)],
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
        // With no pair, line 0 is the shell as iTerm2, but only when it can be a shell: an
        // error line leaves the shell unknown (controller ruling, Task 6 review).
        let mut c = started(setup("A"));
        reply(&mut c, &["sh: 160: using_passwd: not found\nsh\n/home/u"], 0);
        assert_eq!(c.shell, None);

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
                Token::Init(_) | Token::Unhook | Token::Error(_) | Token::Output { .. } | Token::Terminate { .. } | Token::It2(_) => {}
            }
        }
        assert_eq!(c.shell.as_deref(), Some("/bin/sh"));
        let foo = Command::Setenv { key: "FOO".into(), value: "bar".into() };
        assert_eq!(c.take_effects(), [Effect::StateChanged, Effect::Write(sent(&foo))]);
    }

    #[test]
    fn getshell_never_takes_a_script_as_the_shell() {
        for ok in ["/bin/sh", "/usr/local/bin/zsh", "sh", "bash", "-zsh"] {
            assert!(plausible_shell(ok), "{ok}");
        }
        for bad in ["", "a b", "sh\t", "x=y", "bin/zsh", "'sh'", "\"sh\"", "`sh`", "sh\u{1b}", "/bin/sh\u{7}", "sh: 160: using_passwd: not found"] {
            assert!(!plausible_shell(bad), "{bad:?}");
        }
        // The real it2ssh runs `sh -c <sanitized script>`, so a shell error starts with the
        // script: the origin token and the base64 environment.
        let script = "printf '\\033]1337;it2ssh=0123abcd\\007\\033]1337;Env=report=all:QUJDPTE=\\007'; exec sh -c 'main': 1: eval: using_passwd: not found";
        let mut c = started(zsh_injected());
        reply(&mut c, &[&format!("{script}\n/bin/zsh\n/home/user\n")], 0);
        assert_eq!((c.shell.as_deref(), home_seen(&c).as_deref()), (Some("/bin/zsh"), Some("/home/user")));
        let mut c = started(zsh_injected());
        reply(&mut c, &[&format!("{script}\nsh\n/home/user\n")], 0);
        assert_eq!(c.shell, None, "the script is never the shell");
        assert_eq!(c.modified_vars, None, "an unknown shell is not injected");
        let written: Vec<u8> = c.take_effects().into_iter().flat_map(|e| if let Effect::Write(b) = e { b } else { Vec::new() }).collect();
        let written = String::from_utf8_lossy(&written);
        assert!(!written.contains("0123abcd") && !written.contains("QUJD"), "{written}");
        // A shell with a pair of absolute paths but no plausible name is skipped as well.
        let mut c = started(setup("A"));
        reply(&mut c, &["/bin/sh -c x\n/bin/zsh\n/home/u"], 0);
        assert_eq!(c.shell.as_deref(), Some("/bin/zsh"));
    }

    #[test]
    fn reply_lines_are_capped() {
        let failed = |c: &mut Conductor| c.take_effects().into_iter().find_map(|e| if let Effect::Abort(r) = e { Some(r) } else { None });
        // Up to the line limit a getshell reply is kept.
        let mut c = started(setup("A"));
        c.handle_begin("1", 0);
        for _ in 0..GETSHELL_LIMIT.1 {
            c.handle_line("x", 0);
        }
        assert_eq!(failed(&mut c), None);
        c.handle_line("x", 0);
        assert_eq!(failed(&mut c).as_deref(), Some("getshell: The reply is too long"));
        assert_eq!(c.state(), &State::Ground);
        c.handle_end("1", "r", 0, 0);
        assert!(c.take_effects().is_empty(), "the end of a failed reply is ignored");
        // The byte limit.
        let mut c = started(setup("A"));
        c.handle_line(&"y".repeat(GETSHELL_LIMIT.0), 0);
        assert_eq!(failed(&mut c), None);
        c.handle_line("y", 0);
        assert_eq!(failed(&mut c).as_deref(), Some("getshell: The reply is too long"));
        // pythonversion keeps less.
        let mut c = started(setup("A"));
        reply(&mut c, &["/bin/sh\n/home/u"], 0);
        c.take_effects();
        assert_eq!(c.in_flight(), Some(&Command::PythonVersion));
        c.handle_line(&"z".repeat(PYTHON_LIMIT.0 + 1), 0);
        assert_eq!(failed(&mut c).as_deref(), Some("pythonversion: The reply is too long"));
        assert_eq!(c.state(), &State::Ground);
        assert!(c.take_effects().is_empty(), "no login shell after the failure");
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

        // A usable python launches the framer (14c Task 1), not the login shell.
        let runpython = |c: &Conductor| c.in_flight() == Some(&Command::RunPython(crate::framer_script::framer_code(0)));
        for ok in ["Python 3.12.1", "Python 3.7"] {
            let mut c = at_python();
            reply(&mut c, &[ok], 0);
            assert!(c.python_ok, "{ok}");
            assert!(runpython(&c), "{ok}");
            assert!(!c.take_effects().contains(&Effect::StopQueueingInput), "{ok}");
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

    struct Hosts;

    impl crate::ssh_args::HostnameFinder for Hosts {
        fn hostname(&self, host: &str) -> String {
            format!("{host}.internal")
        }
    }

    fn env_pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn setup_for_hook_follows_iterm2() {
        let init = crate::conductor_parser::Init { token: "t".into(), unique_id: "12345".into(), bool_args: "46AaCf".into(), sshargs: "-p 2222 example.com ".into(), dcs_id: "D1".into() };
        let env = env_pairs(&[("A", "1"), ("B", "2"), ("C", "3"), ("A", "4")]);
        let names = s(&["C", "A", "X"]);
        let local = setup_for_hook(&init, true, &env, &names, Some("/srv".into()), true, &Hosts);
        assert_eq!(local.vars_to_send, env_pairs(&[("C", "3"), ("A", "4")]), "configured order, missing names skipped, the last value");
        assert_eq!(local.client_vars, env_pairs(&[("A", "4"), ("B", "2"), ("C", "3")]), "first appearance, last value");
        assert_eq!(local.sshargs, "-p 2222 example.com ");
        assert_eq!(local.bool_args, "46AaCf");
        assert_eq!(local.dcs_id, "D1");
        assert_eq!(local.client_unique_id, "12345");
        assert_eq!(local.initial_directory.as_deref(), Some("/srv"));
        assert!(local.inject);
        assert_eq!(local.parsed, crate::ssh_args::parse("-p 2222 example.com ", "46AaCf", &Hosts));
        assert_eq!(local.parsed.hostname, "example.com.internal");
        let remote = setup_for_hook(&init, false, &env, &names, None, false, &Hosts);
        assert_eq!(remote.vars_to_send, vec![], "a remote origin sends nothing (`PS:20137-20146`)");
        assert_eq!(remote.client_vars, local.client_vars);
        assert_eq!(remote.initial_directory, None);
        assert!(!remote.inject);
        let twice = setup_for_hook(&init, true, &env, &s(&["A", "A"]), None, false, &Hosts);
        assert_eq!(twice.vars_to_send, env_pairs(&[("A", "4")]), "a name listed twice is sent once, as a dictionary");
        assert_eq!(hook_banner("example.com "), "ssh example.com ");
        let long = hook_banner(&"\u{e9}".repeat(1 << 20));
        assert_eq!(long.chars().count(), MAX_SHOWN + 1, "cut, with an ellipsis");
        assert!(long.starts_with("ssh \u{e9}") && long.ends_with("\u{e9}\u{2026}"));
        assert_eq!(shown(&"x".repeat(MAX_SHOWN)), "x".repeat(MAX_SHOWN), "not cut at the limit");
    }

    #[test]
    fn skipped_files_reach_the_pane() {
        let missing = std::env::temp_dir().join(format!("tandem-conductor-missing-{}", std::process::id())).join("nothing.txt");
        let mut c = Conductor::new(setup("A"), None, "U");
        c.add_path(&missing.to_string_lossy(), "~/", Path::new("C:\\"));
        c.start();
        c.take_effects();
        reply(&mut c, &["/bin/sh\n/home/u\n"], 0);
        let notices: Vec<String> = c.take_effects().into_iter().filter_map(|e| if let Effect::Notice(t) = e { Some(t) } else { None }).collect();
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].starts_with("A Files to Copy tarball for /$HOME was skipped: "), "{}", notices[0]);
    }

    /// The recorder's line prefix UUID (`scripts/record-conductor.py`).
    const RECORDER_PREFIX: &str = "8C3A5E1F-0000-4000-8000-00000000A11E";

    /// Feeds `outs` to the pane's conductor as `ssh_pump` does (an Init hooks a conductor
    /// with `hook`), collecting every effect in the order it was taken.
    fn dispatch(current: &mut Option<Box<Conductor>>, outs: Vec<crate::ssh_hook::Out>, hook: &dyn Fn(&Init, Option<Box<Conductor>>) -> Conductor) -> Vec<Effect> {
        use crate::conductor_parser::Token;
        use crate::ssh_hook::Out;
        let mut effects = Vec::new();
        for out in outs {
            match out {
                Out::Screen(_) | Out::Side { .. } | Out::Raw(_) => {}
                Out::Ssh(Token::Init(init), _) => {
                    let mut c = hook(&init, current.take());
                    c.start();
                    *current = Some(Box::new(c));
                }
                Out::Ssh(token, depth) => {
                    let Some(c) = current.as_mut() else { continue };
                    match token {
                        Token::Begin(id) => c.handle_begin(&id, depth),
                        Token::Line(line) => c.handle_line(&line, depth),
                        Token::End { id, status, kind } => c.handle_end(&id, &kind, status, depth),
                        Token::Unhook => c.handle_unhook(),
                        Token::Error(reason) => c.fail(&reason),
                        Token::Init(_) | Token::Output { .. } | Token::Terminate { .. } | Token::It2(_) => {}
                    }
                }
            }
            loop {
                let taken = current.as_mut().map(|c| c.take_effects()).unwrap_or_default();
                if taken.is_empty() {
                    break;
                }
                for effect in taken {
                    // What was typed while connecting (nothing here) goes through sendKeys.
                    if effect == Effect::StopQueueingInput {
                        if let Some(c) = current.as_mut() {
                            c.send_keys(b"");
                        }
                    }
                    effects.push(effect);
                }
            }
        }
        effects
    }

    /// The command a pre-framer write carries: its chunks after the prefix, joined and
    /// base64-decoded.
    fn decoded_command(bytes: &[u8], prefix: &str) -> String {
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        let body: String = text.lines().filter_map(|l| l.strip_prefix(prefix)).collect();
        String::from_utf8(base64::engine::general_purpose::STANDARD.decode(body).unwrap()).unwrap()
    }

    #[test]
    fn a_long_begin_id_fails_setup_at_once() {
        // The hook, then a begin whose id is past conductor_parser::MAX_ID: the conductor
        // fails with the reason now, not 30 seconds later, and the output after it shows.
        let hook = |init: &Init, parent: Option<Box<Conductor>>| Conductor::new(setup_for_hook(init, false, &[], &[], None, false, &crate::ssh_args::NoConfig), parent, "U");
        let mut ssh = crate::ssh_hook::Hook::with_ids(|| "D".into());
        let mut current = None;
        let mut bytes = b"\x1bP2000pnone a YQ== - Yg==\n".to_vec();
        bytes.extend_from_slice(format!("\x1b]135;:begin {}\x1b\\$ ", "1".repeat(1 << 16)).as_bytes());
        let outs = ssh.feed(&bytes);
        assert!(matches!(outs.last(), Some(crate::ssh_hook::Out::Screen(b)) if b == b"$ "), "output again");
        let effects = dispatch(&mut current, outs, &hook);
        assert_eq!(effects.last(), Some(&Effect::Abort(crate::conductor_parser::ID_TOO_LONG.into())));
        assert_eq!(current.as_deref().unwrap().deadline(), None);
    }

    #[test]
    fn level_one_follows_iterm2() {
        let dir = std::env::temp_dir().join(format!("tandem-conductor-level1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let hello = dir.join("hello.txt");
        std::fs::write(&hello, b"hi\n").unwrap();
        let hello_path = hello.to_string_lossy().to_string();

        let env = env_pairs(&[("FOO", "bar")]);
        let hook = |init: &Init, parent: Option<Box<Conductor>>| {
            let setup = setup_for_hook(init, true, &env, &s(&["FOO"]), Some("/tmp".into()), true, &crate::ssh_args::NoConfig);
            let mut c = Conductor::new(setup, parent, RECORDER_PREFIX);
            c.add_path(&hello_path, "~/.t", &dir);
            c
        };
        let recording = std::fs::read("tests/fixtures/ssh/level1-nopython.bin").unwrap();
        let mut ssh = crate::ssh_hook::Hook::with_ids(|| "D".into());
        let mut current = None;
        let effects = dispatch(&mut current, ssh.feed(&recording), &hook);
        let c = current.as_deref().unwrap();

        // The commands, decoded.
        let prefix = format!("\u{3}{RECORDER_PREFIX}");
        let writes: Vec<Vec<u8>> = effects.iter().filter_map(|e| if let Effect::Write(b) = e { Some(b.clone()) } else { None }).collect();
        assert_eq!(writes[0], format!("{prefix}\n").into_bytes(), "the line prefix first");
        let (packed, skipped) = tarballs(&[Payload { source: Source::Disk(hello.clone()), destination: "/$HOME/.t".into() }]);
        assert!(skipped.is_empty(), "{skipped:?}");
        let write = Command::Write { data: packed[0].0.clone(), dest: "/$HOME/.t".into() }.string_value();
        let commands: Vec<String> = writes[1..].iter().map(|w| decoded_command(w, &prefix)).collect();
        assert_eq!(commands, ["getshell", "setenv FOO bar", write.as_str(), "cd /tmp", "pythonversion", "exec_login_shell"]);

        // Every line but the write's is the recording's, byte for byte.
        let recorded = std::fs::read("tests/fixtures/ssh/level1-nopython.in.bin").unwrap();
        let first = recorded.iter().position(|&b| b == b'\n').unwrap() + 1;
        let mut segments = vec![recorded[..first].to_vec()];
        let mut at = first;
        while let Some(end) = recorded[at..].windows(2).position(|w| w == b"\n\n") {
            segments.push(recorded[at..at + end + 2].to_vec());
            at += end + 2;
        }
        assert_eq!(&recorded[at..], b"echo hi\nexit\n", "typed after the unhook, not the conductor's");
        assert_eq!(segments.len(), writes.len());
        for (i, (ours, theirs)) in writes.iter().zip(&segments).enumerate() {
            if i == 3 {
                assert_eq!(decoded_command(theirs, &prefix).split(' ').nth(2), Some("/$HOME/.t"), "the recorded write");
                continue;
            }
            assert_eq!(ours, theirs, "command {i}");
        }

        // The login shell: input stops queueing just before it, Initial Text after its end.
        let login = sent_with(&Command::ExecLoginShell(vec![]), &prefix);
        let at_login = effects.iter().position(|e| *e == Effect::Write(login.clone())).unwrap();
        assert_eq!(effects[at_login - 1], Effect::StopQueueingInput);
        assert!(effects[at_login + 1..].contains(&Effect::SendInitialText));
        assert!(!effects[..at_login].contains(&Effect::SendInitialText));
        assert!(effects.iter().all(|e| !matches!(e, Effect::Abort(_))), "{effects:?}");

        assert!(c.unhooked());
        assert_eq!(level(Some(c)), 1);
        assert!(!c.queue_writes());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What `write()` puts on the wire for a command under `prefix`.
    fn sent_with(cmd: &Command, prefix: &str) -> Vec<u8> {
        (encode(cmd, prefix) + "\n").into_bytes()
    }

    #[test]
    fn end_ssh_pops_the_chain() {
        use crate::ssh_session::{Chain, Hop, PaneSsh};
        let at = |id: &str, host: &str| {
            let parsed = ParsedArgs { host: host.into(), hostname: host.into(), port: 22, ..ParsedArgs::default() };
            Setup { client_unique_id: id.into(), parsed, ..Setup::default() }
        };
        let hop = |c: &Conductor| Hop { unique_id: c.setup.client_unique_id.clone(), location: c.identity().compact("me") };
        let a = Conductor::new(at("a", "alpha.example"), None, "U");
        let a_compact = a.identity().compact("me");
        let b = Conductor::new(at("b", "beta.example"), Some(Box::new(a)), "V");
        let c = Conductor::new(at("c", "gamma.example"), Some(Box::new(b)), "W");
        let hops = {
            let b = c.parent.as_deref().unwrap();
            vec![hop(b.parent.as_deref().unwrap()), hop(b), hop(&c)]
        };
        let mut current = Some(Box::new(c));
        let mut ssh = PaneSsh { chain: Chain { hops: hops.clone() }, ..PaneSsh::default() };

        let n = ssh.chain.count_to("b");
        assert_eq!(n, 2);
        for _ in 0..n {
            assert!(pop(&mut current).is_some());
        }
        assert_eq!(current.as_ref().map(|c| c.setup.client_unique_id.as_str()), Some("a"));
        assert!(current.as_ref().unwrap().parent.is_none());
        assert_eq!(ssh.end_ssh("b"), Some(format!("2 ssh sessions ended. \u{2014} now at {a_compact}.")));
        assert_eq!(a_compact, "alpha.example");
        assert_eq!(ssh.chain.hops, hops[..1]);

        // An id no conductor has: nothing counted, ended or popped.
        let a = Conductor::new(at("a", "alpha.example"), None, "U");
        let b = Conductor::new(at("b", "beta.example"), Some(Box::new(a)), "V");
        let mut ssh = PaneSsh { chain: Chain { hops: hops[..2].to_vec() }, ..PaneSsh::default() };
        let mut current = Some(Box::new(b));
        assert_eq!(ssh.chain.count_to("zzz"), 0);
        assert_eq!(ssh.end_ssh("zzz"), None);
        assert_eq!(ssh.chain.hops, hops[..2]);
        assert_eq!(current.as_ref().map(|c| c.setup.client_unique_id.as_str()), Some("b"));
        assert_eq!(Chain { hops: hops.clone() }.count_to("c"), 1);

        // Popping the last leaves none; popping none gives none.
        assert_eq!(pop(&mut current).map(|c| c.setup.client_unique_id.clone()).as_deref(), Some("b"));
        assert_eq!(pop(&mut current).map(|c| c.setup.client_unique_id.clone()).as_deref(), Some("a"));
        assert!(current.is_none());
        assert!(pop(&mut current).is_none());

        assert_eq!(level(None), 0);
        let mut framed = Conductor::new(setup("F"), None, "U");
        assert_eq!(level(Some(&framed)), 1);
        framed.framed_pid = Some(1);
        assert_eq!(level(Some(&framed)), 2);
    }

    #[test]
    fn nested_setup_follows_iterm2() {
        // A level 1 session: set up, then unhooked.
        let mut first = started(setup("A"));
        first.handle_unhook();
        first.take_effects();
        assert_eq!(level(Some(&first)), 1);
        assert!(!first.queue_writes());

        // it2ssh typed at its prompt.
        let mut second = Conductor::new(setup("B"), Some(Box::new(first)), "V");
        assert_eq!(second.depth(), 0, "an unframed parent: the same depth");
        assert!(!second.queue_writes(), "an unhooked parent never holds typing");
        second.start();
        assert!(!second.queue_writes());
        assert_eq!(
            second.effects,
            [Effect::Write(b"\x03V\n".to_vec()), Effect::Write(sent_with(&Command::GetShell, "\u{3}V"))],
            "written through the parent, in the child's effects"
        );
        assert!(second.parent.as_ref().unwrap().effects.is_empty());
        assert_eq!(level(Some(&second)), 1);
    }

    /// A conductor with its clock at `t`: the next call reads `t` as now.
    fn at(c: &mut Conductor, t: Instant) -> &mut Conductor {
        c.fake_now = Some(t);
        c
    }

    const SECS: fn(u64) -> Duration = Duration::from_secs;

    #[test]
    fn stalled_command_times_out() {
        let t0 = Instant::now();
        let mut c = Conductor::new(setup("A"), None, "U");
        at(&mut c, t0).start();
        c.take_effects();
        assert_eq!(c.deadline(), Some(t0 + SECS(30)));
        c.tick(t0 + SECS(29));
        assert!(c.take_effects().is_empty(), "not yet");
        assert_eq!(c.in_flight(), Some(&Command::GetShell));
        c.send(cd("/queued"), Handler::FailIfNonzero, false);
        c.tick(t0 + SECS(31));
        assert_eq!(c.take_effects(), [Effect::SessionWrite(b"exec_login_shell\n".to_vec()), Effect::Abort("getshell: No response after 30 seconds".into())]);
        assert_eq!((c.state(), c.in_flight()), (&State::Ground, None));
        assert!(c.queue.is_empty());
        assert_eq!(c.deadline(), None, "a failed conductor waits for nothing");
        c.tick(t0 + SECS(3600));
        assert!(c.take_effects().is_empty(), "it fails once");
        c.handle_unhook();
        assert_eq!(c.deadline(), None);

        // An unhooked conductor has no deadline, even with a command in flight before it.
        let mut c = Conductor::new(setup("A"), None, "U");
        at(&mut c, t0).start();
        c.handle_unhook();
        assert_eq!(c.deadline(), None);
        c.tick(t0 + SECS(31));
        assert!(!c.take_effects().iter().any(|e| matches!(e, Effect::Abort(_))));

        // No 14b command streams, so every one written before framing is timed.
        for cmd in pre_framer().into_iter().chain(framer()) {
            assert!(!cmd.streams(), "{cmd:?}");
        }

        // The recorded stall: quirks.bin cut right after its first begin.
        let recording = std::fs::read("tests/fixtures/ssh/quirks.bin").unwrap();
        let begin = recording.windows(8).position(|w| w == b":begin 0").unwrap();
        let cut = begin + recording[begin..].windows(2).position(|w| w == b"\x1b\\").unwrap() + 2;
        let hook = |init: &Init, parent: Option<Box<Conductor>>| {
            let mut c = Conductor::new(setup_for_hook(init, false, &[], &[], None, false, &crate::ssh_args::NoConfig), parent, RECORDER_PREFIX);
            c.fake_now = Some(t0);
            c
        };
        let mut ssh = crate::ssh_hook::Hook::with_ids(|| "D".into());
        let mut current = None;
        dispatch(&mut current, ssh.feed(&recording[..cut]), &hook);
        let c = current.as_deref_mut().unwrap();
        assert_eq!(c.in_flight(), Some(&Command::GetShell), "its begin came, nothing more");
        assert_eq!(c.deadline(), Some(t0 + SECS(30)));
        c.tick(t0 + SECS(30));
        assert_eq!(c.take_effects().last(), Some(&Effect::Abort("getshell: No response after 30 seconds".into())));

        // The silent stall (iTerm2 bugs B5 and B6): getshell's non-zero status leaves setup in
        // ground with nothing in flight while typing is held.
        let mut c = Conductor::new(setup("A"), None, "U");
        at(&mut c, t0).start();
        reply(at(&mut c, t0 + SECS(5)), &["/bin/sh\n/home/u"], 1);
        c.take_effects();
        assert_eq!((c.state(), c.in_flight()), (&State::Ground, None));
        assert!(c.queue_writes());
        assert_eq!(c.deadline(), Some(t0 + SECS(35)), "30 s after the last activity");
        c.tick(t0 + SECS(34));
        assert!(c.take_effects().is_empty());
        c.tick(t0 + SECS(36));
        assert_eq!(c.take_effects(), [Effect::SessionWrite(b"exec_login_shell\n".to_vec()), Effect::Abort("getshell: No response after 30 seconds".into())], "the last command written");
        assert_eq!(c.deadline(), None);

        // A silent unhook of the hook (garbage, an unknown word) leaves a command in flight:
        // it times out as a stalled command does. Ground stalls are only for a conductor
        // that does not frame.
        let mut framing = Conductor::new(setup("A"), None, "U");
        at(&mut framing, t0).start();
        reply(at(&mut framing, t0), &["/bin/sh\n/home/u"], 1);
        framing.framed_pid = Some(4242);
        assert_eq!(framing.deadline(), None, "a framing conductor has no ground deadline");
        let mut unhooked = started(setup("A"));
        unhooked.handle_unhook();
        assert_eq!(unhooked.deadline(), None);
        let mut parent = started(setup("P"));
        parent.handle_unhook();
        let mut child = Conductor::new(setup("C"), Some(Box::new(parent)), "V");
        at(&mut child, t0).start();
        assert_eq!(child.deadline(), Some(t0 + SECS(30)), "its own command is timed");
        reply(at(&mut child, t0), &["/bin/sh\n/home/u"], 1);
        assert!(!child.queue_writes());
        assert_eq!(child.state(), &State::Ground);
        assert_eq!(child.deadline(), Some(t0 + SECS(30)), "nothing is held, but the pane's hook swallows output until it ends");
        child.tick(t0 + SECS(30));
        assert_eq!(child.take_effects().last(), Some(&Effect::Abort("getshell: No response after 30 seconds".into())));
    }

    #[test]
    fn framed_commands_are_not_timed_out() {
        // Ruling M14-I2: a sleeping laptop must not end SSH integration.
        let t0 = Instant::now();
        let mut c = root();
        c.framed_pid = Some(4242);
        at(&mut c, t0).send(Command::FramerGetenv("PATH".into()), Handler::FireAndForget, false);
        c.take_effects();
        assert_eq!(c.in_flight(), Some(&Command::FramerGetenv("PATH".into())));
        assert_eq!(c.deadline(), None);
        for later in [SECS(31), SECS(3600)] {
            c.tick(t0 + later);
            assert!(c.take_effects().is_empty());
            assert!(c.framing());
            assert_eq!(c.in_flight(), Some(&Command::FramerGetenv("PATH".into())));
        }
        for cmd in pre_framer().into_iter().chain(framer()) {
            assert!(!cmd.timed_when_framed(), "{cmd:?}");
        }

        // Written before framing began: its 30 s stand.
        let mut c = root();
        at(&mut c, t0).send(cd("/a"), Handler::FailIfNonzero, false);
        c.framed_pid = Some(4242);
        assert_eq!(c.deadline(), Some(t0 + SECS(30)));
        c.take_effects();
        c.tick(t0 + SECS(31));
        assert_eq!(c.take_effects().last(), Some(&Effect::Abort("changing directory to /a: No response after 30 seconds".into())));
    }

    #[test]
    fn a_write_has_time_for_its_size() {
        assert_eq!(command_timeout(0), COMMAND_TIMEOUT);
        assert_eq!(command_timeout(16 << 10), COMMAND_TIMEOUT, "every command but a large write: 30 s");
        assert_eq!(command_timeout(1 << 20), SECS(330));
        assert_eq!(command_timeout(4 << 20), SECS(30 + 4800));
        // Past any real write (Files to Copy is capped far below this), the sum saturates
        // rather than overflowing, and a deadline past what an Instant holds is never due.
        assert_eq!(command_timeout(usize::MAX), Duration::MAX);
        assert_eq!(due_at(Instant::now(), Duration::MAX), None);

        // Files to Copy of 4 MiB of incompressible data (its gzip tarball is as large): about
        // 7.5 MB on the wire, over four hours, where 30 s failed it (conductor.sh takes about
        // 9 minutes under dash, 2 hours under bash).
        let t0 = Instant::now();
        let mut c = root();
        let write = Command::Write { data: vec![0xa5; 4 << 20], dest: "/$HOME".into() };
        at(&mut c, t0).send(write, Handler::FailIfNonzero, false);
        let len = wire(&mut c).len();
        let due = c.deadline().unwrap();
        assert_eq!(due, t0 + command_timeout(len));
        assert!(due > t0 + SECS(4 * 3600), "{:?}", due - t0);
        c.tick(due - SECS(1));
        assert!(c.take_effects().is_empty());
        at(&mut c, t0 + SECS(60)).handle_begin("1", 0);
        assert_eq!(c.deadline(), Some(t0 + SECS(60) + command_timeout(len)), "each restart gets the write's time");
        c.tick(t0 + SECS(60) + command_timeout(len));
        let text = format!("copying files to /$HOME: No response after {} seconds", command_timeout(len).as_secs());
        assert_eq!(c.take_effects().last(), Some(&Effect::Abort(text)));
    }

    #[test]
    fn activity_restarts_the_clock() {
        let t0 = Instant::now();
        let mut c = root();
        at(&mut c, t0).send(cd("/a"), Handler::FailIfNonzero, false);
        c.send(cd("/b"), Handler::FailIfNonzero, false);
        assert_eq!(c.deadline(), Some(t0 + SECS(30)));
        at(&mut c, t0 + SECS(20)).handle_begin("1", 0);
        assert_eq!(c.deadline(), Some(t0 + SECS(50)), "a begin");
        at(&mut c, t0 + SECS(40)).handle_line("out", 0);
        assert_eq!(c.deadline(), Some(t0 + SECS(70)), "a line");
        c.tick(t0 + SECS(69));
        c.take_effects();
        assert_eq!(c.in_flight(), Some(&cd("/a")));
        at(&mut c, t0 + SECS(45)).handle_end("1", "r", 0, 0);
        assert_eq!(c.in_flight(), Some(&cd("/b")));
        assert_eq!(c.deadline(), Some(t0 + SECS(75)), "the next command's clock starts at its write");
        c.tick(t0 + SECS(76));
        assert_eq!(c.take_effects().last(), Some(&Effect::Abort("changing directory to /b: No response after 30 seconds".into())));

        // Stray input in ground restarts nothing.
        let mut c = started(setup("A"));
        reply(at(&mut c, t0), &["/bin/sh\n/home/u"], 1);
        at(&mut c, t0 + SECS(20)).handle_begin("2", 0);
        at(&mut c, t0 + SECS(20)).handle_line("x", 0);
        at(&mut c, t0 + SECS(20)).handle_end("2", "r", 0, 0);
        assert_eq!(c.deadline(), Some(t0 + SECS(30)));
    }

    #[test]
    fn texts_follow_iterm2() {
        assert_eq!(SETUP_ERROR, "An error occurred while setting up the SSH environment:");
        assert_eq!(COMMAND_TIMEOUT, Duration::from_secs(30));
        assert_eq!(unexpected_status(&Cd("/a".into()), 1), "cd /a: Unepected status 1");
        assert_eq!(timeout_text(&Command::Write { data: vec![0; 3], dest: "/$HOME/x".into() }, COMMAND_TIMEOUT), "copying files to /$HOME/x: No response after 30 seconds");
        assert_eq!(timeout_text(&Command::GetShell, Duration::from_secs(331)), "getshell: No response after 331 seconds");
        // Never an environment value, and a shell by its name only.
        let secret = Command::Setenv { key: "TOKEN".into(), value: "s3cret".into() };
        assert_eq!(timeout_text(&secret, COMMAND_TIMEOUT), "setting TOKEN: No response after 30 seconds");
        let login = Command::ExecLoginShell(s(&["/opt/secret/bin/bash", "--posix"]));
        assert_eq!(timeout_text(&login, COMMAND_TIMEOUT), "starting login shell with args bash --posix: No response after 30 seconds");
        assert_eq!(timeout_text(&Command::ExecLoginShell(vec![]), COMMAND_TIMEOUT), "starting login shell with args : No response after 30 seconds");
        assert_eq!(unexpected_status(&login, 1), "exec_login_shell\nbash\n--posix: Unepected status 1", "the status text trims the shell too");
        assert_eq!(unexpected_status(&Command::ExecLoginShell(vec![]), 1), "exec_login_shell: Unepected status 1");
        // A write never puts its tarball on the screen, only where it goes.
        let write = Command::Write { data: vec![7; 4096], dest: "/$HOME/.config".into() };
        assert_eq!(unexpected_status(&write, 1), "write /$HOME/.config: Unepected status 1");
    }

    fn send_of(len: usize) -> Command {
        Command::FramerSend { data: vec![b'a'; len], pid: 4242 }
    }

    fn wire_writes(c: &mut Conductor) -> usize {
        c.take_effects().iter().filter(|e| matches!(e, Effect::Write(_))).count()
    }

    #[test]
    fn pipeline_follows_iterm2() {
        // 300 bytes: 400 base64 characters plus "send\n4242\n" is 410 characters, times 4 / 3.
        let big = send_of(300);
        assert_eq!(big.size(), 546);
        assert!(big.supports_pipelining());
        let run = Command::FramerRun("x".into());
        assert!(!run.supports_pipelining());
        assert_eq!(run.size(), usize::MAX);
        assert_eq!(PIPELINE_BYTES, 1024);

        let mut c = Conductor::new(setup("P"), None, "U");
        c.framed_pid = Some(4242);
        for _ in 0..3 {
            c.send(send_of(300), Handler::FireAndForget, false);
        }
        // 546, then 1092 reaches the limit: two are written at once, the third waits.
        assert_eq!(wire_writes(&mut c), 2);
        assert_eq!(c.pending().len(), 3);
        assert!(matches!(c.state(), State::WillExecute(v) if v.len() == 2));
        let first = big.description();
        assert_eq!(c.description(), format!("{first} (preparation stage) and 1 more"));
        // The first has begun (its first line arrived).
        c.handle_line("", 0);
        assert_eq!(c.description(), format!("{first} and 1 more"));
        // An end for the first tops the pipeline up: the third goes out.
        c.handle_end("a", "f", 0, 0);
        assert_eq!(wire_writes(&mut c), 1);
        assert!(matches!(c.state(), State::WillExecute(v) if v.len() == 2));
        c.handle_end("b", "f", 0, 0);
        c.handle_end("c", "f", 0, 0);
        assert!(c.pending().is_empty() && *c.state() == State::Ground);

        // A run queued behind a pipeline is written only when the pipeline has ended.
        for _ in 0..3 {
            c.send(send_of(1), Handler::FireAndForget, false);
        }
        c.send(Command::FramerRun("x".into()), Handler::FireAndForget, false);
        assert_eq!(wire_writes(&mut c), 3);
        c.handle_end("a", "f", 0, 0);
        c.handle_end("b", "f", 0, 0);
        assert_eq!(wire_writes(&mut c), 0, "the run still waits for the last send");
        c.handle_end("c", "f", 0, 0);
        assert_eq!(wire_writes(&mut c), 1);
        assert_eq!(c.in_flight(), Some(&run));
        // A send behind a run waits for it, and a send after keystrokes joins them.
        c.send(send_of(1), Handler::FireAndForget, false);
        assert_eq!(wire_writes(&mut c), 0);
        c.handle_end("d", "f", 0, 0);
        assert_eq!(wire_writes(&mut c), 1);
    }
}

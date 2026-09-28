//! iTerm2's Conductor (`CD`, milestone 14b): the commands it sends to conductor.sh and
//! framer.py with their wire encoding, and the conductor object with its parent chain.
//! Pure: it talks to the app only through the `Effect`s it queues (the tmux gateway's
//! `take_writes` pattern).

use crate::ssh_args::{Identity, ParsedArgs};
use crate::ssh_inject::Payload;
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
    #[cfg_attr(not(test), allow(dead_code))] // Task 5's state machine is the first caller.
    fn write_through(&mut self, bytes: Vec<u8>) {
        self.effects.push(Effect::Write(bytes));
    }

    /// write (`CD:3039-3058`): `text` plus `\n`, through the parent when there is one. What
    /// the parent queues for it moves into this conductor's queue, after its own.
    #[cfg_attr(not(test), allow(dead_code))] // Task 5's state machine is the first caller.
    pub(crate) fn write(&mut self, text: &str) {
        let bytes = format!("{text}\n").into_bytes();
        match self.parent.as_mut() {
            Some(parent) => {
                let before = parent.effects.len();
                parent.write_through(bytes);
                self.effects.extend(parent.effects.drain(before..));
            }
            None => self.effects.push(Effect::Write(bytes)),
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
}

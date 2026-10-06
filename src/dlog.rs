//! The debug log (iTerm2's `DebugLogging.m`): `dlog!` lines kept in memory while Toggle Debug
//! Logging is on, written as one file when it goes off. The file is the one place Tandem writes
//! what it did, so it holds no secret: every argument of `dlog!` must be a `LogSafe` value (a
//! number, a flag, a literal, or `name(..)`, which keeps a short plain-ASCII word), a format
//! string never captures a variable inline, and the call sites are listed (`CALL_SITES`).
//!
//! The file lives in `log_dirs()`: `%TEMP%` when the path judge accepts it, else Local AppData.
//! It is made owner-only, never follows a link, is appended to only when it is this user's own
//! single-link file, and is capped with one `.old` generation. The write runs off the UI thread
//! (`dlog_ui`) or, at exit, inside `teardown_process`.

use std::collections::VecDeque;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread::ThreadId;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use crate::localfs;

/// In-memory text past this loses its oldest half (`DL:452-457`).
pub const MAX_LOG: usize = 200 * 1024 * 1024;
/// The retrospective ring's bound (`DL:36-38`).
pub const RING_MAX: usize = 10 * 1024 * 1024;
/// The file never grows past this; the next write starts a new file and keeps the old one once.
pub const MAX_FILE: u64 = 256 * 1024 * 1024;
pub const TRUNCATED: &str = "*GIANT LOG TRUNCATED*\n";
pub const FILE_NAME: &str = "debuglog.txt";
pub const OLD_NAME: &str = "debuglog.old.txt";

pub const ENABLED_HEADING: &str = "Debug Logging Enabled";
pub const ENABLED_TEXT: &str = "Please reproduce the bug. Then toggle debug logging again to save the log.";
pub const STOPPED_HEADING: &str = "Debug Logging Stopped";
pub const SAVE_PROBLEM_HEADING: &str = "Problem Saving Debug Log";
pub const AUTO_TITLE: &str = "Debug logging turned on";
pub const AUTO_DETAIL: &str = "You may notice worse performance while debug logging is on.";
/// The hover text of the top-right indicator (`IH:389`).
pub const INDICATOR_HOVER: &str = "Debug logging is enabled.";

pub fn stopped_text(path: &Path) -> String {
    format!("Please send {} to the developers.", path.display())
}

pub fn save_failed_text(reason: &str) -> String {
    format!("Failed to save debug log: {reason}")
}

pub fn could_not_open(path: &Path) -> String {
    format!("could not open {} for writing", path.display())
}

/// The log file in a folder (`temp\debuglog.txt`).
pub fn file_path(temp: &Path) -> PathBuf {
    temp.join(FILE_NAME)
}

/// One entry (`DL:447-448`).
pub fn entry(secs: i64, micros: u32, file: &str, line: u32, module: &str, thread: &str, text: &str) -> String {
    format!("{secs}.{micros:06} {file}:{line} ({module}) {thread}: {text}\n")
}

/// What goes to the debug output with `LogToSyslog`: the prefix only, never the text (`DL:411-419`).
pub fn syslog_prefix(secs: i64, micros: u32, file: &str, line: u32, module: &str) -> String {
    format!("TandemDebugLog: {secs}.{micros:06} {file}:{line} ({module}): ")
}

/// A value that cannot carry a secret: only these are accepted as arguments of `dlog!`.
/// A `String` or a borrowed `&str` is not one, so typed text, a title, a path or a token cannot
/// be passed by accident; `name(..)` turns a short identifier into one.
pub trait LogSafe {}

macro_rules! log_safe {
    ($($t:ty),* $(,)?) => { $(impl LogSafe for $t {})* };
}
log_safe!(u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize, f32, f64, bool, char, std::time::Duration, Name);
impl LogSafe for &'static str {}

/// Identity, so the macro can demand `LogSafe` of each argument.
#[doc(hidden)]
pub fn safe<T: LogSafe + ?Sized>(t: &T) -> &T {
    t
}

/// A short word for a log line: ASCII letters, digits and `. _ - space` as they are, anything
/// else `?`, cut at 64 characters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Name(String);

pub fn name(s: &str) -> Name {
    Name(s.chars().take(64).map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ' ') { c } else { '?' }).collect())
}

/// A list of short words (program names in a job ancestry): each one passed through `name`, shown
/// as `["a", "b"]`. The only way to make one is `names`, so it holds nothing unsanitized.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Names(Vec<Name>);

pub fn names(items: &[String]) -> Names {
    Names(items.iter().map(|s| name(s)).collect())
}

impl LogSafe for Names {}

impl fmt::Display for Names {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[")?;
        for (i, n) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "\"{n}\"")?;
        }
        f.write_str("]")
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Whether a format string captures a variable inline (`{secret}`), which skips the `LogSafe`
/// check on that value. Only `{}`, `{:..}` and the escapes `{{` and `}}` are allowed.
pub const fn has_capture(fmt: &str) -> bool {
    let b = fmt.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'{' {
            if i + 1 >= b.len() {
                return true;
            }
            match b[i + 1] {
                b'{' => i += 1,
                b'}' | b':' => {}
                _ => return true,
            }
        }
        i += 1;
    }
    false
}

/// Whether the notification the person clicked is still the automatic-start notice: it was the
/// last one shown (`recorded` is `notify::shown_count()` right after it).
pub fn same_notice(recorded: Option<u64>, shown_now: u64) -> bool {
    recorded == Some(shown_now)
}

/// Fails the build when a log format captures a variable inline: the guarantee that every
/// logged value passed `LogSafe` is the type system's, for every spelling of the macro.
#[doc(hidden)]
pub const fn no_capture(fmt: &str) {
    assert!(!has_capture(fmt), "a log format may not capture a variable inline; pass it as an argument");
}

/// A URL as a log name: its scheme and host only (13x B8, the 13w merge): never the user
/// information, port, path, query or fragment, which can carry a token or what a page was given.
/// A URL with no host (`data:`, `about:blank`) is its scheme alone.
pub fn origin(url: &str) -> Name {
    let url = url.trim();
    let Some((scheme, rest)) = url.split_once(':') else { return name("") };
    if scheme.is_empty() || !scheme.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')) {
        return name("");
    }
    let scheme = scheme.to_ascii_lowercase();
    let Some(rest) = rest.strip_prefix("//") else { return name(&scheme) };
    let authority = &rest[..rest.find(['/', '?', '#', '\\']).unwrap_or(rest.len())];
    let hostport = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if hostport.starts_with('[') {
        hostport.split_once(']').map_or(hostport, |(h, _)| &hostport[..h.len() + 1])
    } else {
        hostport.split(':').next().unwrap_or("")
    };
    name(&format!("{scheme} {}", host.to_ascii_lowercase()))
}

/// The program a command line starts, as a log name: the first word (or quoted path), its file
/// name without folder or ".exe". Arguments are never part of it.
pub fn program(cmd: &str) -> Name {
    let cmd = cmd.trim_start();
    let first = match cmd.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(""),
        None => cmd.split_whitespace().next().unwrap_or(""),
    };
    let file = first.rsplit(['/', '\\']).next().unwrap_or("");
    name(file.strip_suffix(".exe").or_else(|| file.strip_suffix(".EXE")).unwrap_or(file))
}

/// Words a log format must never name (the call-site test reads every `dlog!` format).
pub const FORBIDDEN_WORDS: &[&str] = &["text", "input", "key", "cookie", "token", "password", "secret", "env", "clip", "title", "url", "argv", "typed", "chars"];

/// Words an argument expression of a log call may not name: raw terminal input, a paste, a title,
/// a URL, an environment value or a secret never reaches a log, whatever its type.
pub const FORBIDDEN_ARG_WORDS: &[&str] = &["text", "input", "typed", "key", "paste", "clip", "password", "passwd", "token", "cookie", "secret", "env", "url", "argv", "bytes", "chars", "title", "data", "cmdline", "args"];

/// The retrospective log's texts (`AD:3345-3373`).
pub const NO_RING_HEADING: &str = "No Retrospective Logs";
pub const NO_RING_TEXT: &str = "No retrospective debug logs have been recorded yet.";
pub const RING_FILE: &str = "retrospective-debuglog.txt";
pub const RING_SAVE_TITLE: &str = "Save Retrospective Debug Logs";
pub const RING_SAVE_FAILED_HEADING: &str = "Could Not Save";

pub fn ring_save_failed(e: &str) -> String {
    format!("Failed to save retrospective debug logs: {e}")
}

/// One retrospective entry: the debug log's without the thread (`DL:350-351`).
pub fn ring_entry(secs: i64, micros: u32, file: &str, line: u32, module: &str, text: &str) -> String {
    format!("{secs}.{micros:06} {file}:{line} ({module}): {text}\n")
}

/// The ring's scrub (`DL:288-311`): the text inside `chars="..."` and `unmodchars="..."` (a key
/// event's characters) becomes `[redacted]`. Tandem also redacts the shapes of a secret (P2):
/// `password=`/`token=`/`secret=` values, a bearer token, a cookie header and URL userinfo.
pub fn scrub(text: &str) -> String {
    use regex::Regex;
    static RES: OnceLock<[(Regex, &'static str); 5]> = OnceLock::new();
    let res = RES.get_or_init(|| {
        let re = |p: &str| Regex::new(p).expect("scrub pattern");
        [
            (re(r#"chars="[^"]*""#), "chars=\"[redacted]\""),
            (re(r"(?i)\b(password|passwd|token|secret|apikey|api_key)(\s*[=:]\s*)\S+"), "$1$2[redacted]"),
            (re(r"(?i)\b(bearer)(\s+)\S+"), "$1$2[redacted]"),
            (re(r"(?i)\b(cookie)(\s*:\s*)[^\r\n]+"), "$1$2[redacted]"),
            (re(r"://[^/\s@]+@"), "://[redacted]@"),
        ]
    });
    let mut out = std::borrow::Cow::Borrowed(text);
    for (r, with) in res {
        if let std::borrow::Cow::Owned(s) = r.replace_all(&out, *with) {
            out = std::borrow::Cow::Owned(s);
        }
    }
    out.into_owned()
}

/// Every file holding `dlog!` calls and how many: a new call site is a review of what it passes.
pub const CALL_SITES: &[(&str, usize)] = &[
    ("app/api_ui.rs", 2),
    ("app/bell_ui.rs", 2),
    ("app/browser_app.rs", 2),
    ("app/browser_db_app.rs", 1),
    ("app/chat_window_app.rs", 2),
    ("app/clipboard_ui.rs", 1),
    ("app/hotkey_ui.rs", 2),
    ("app/menu.rs", 2),
    ("app/mod.rs", 6),
    ("app/restore_ui.rs", 2),
    ("app/session_end_ui.rs", 1),
    ("app/tmux_ui.rs", 2),
    ("app/tool_web_ui.rs", 1),
    ("app/triggers_ui.rs", 1),
    ("app/web_popover_app.rs", 4),
    ("app/webhost_app.rs", 6),
    ("app/windows_ui.rs", 3),
    ("globals.rs", 1),
    ("host_client.rs", 2),
    ("jobwatch.rs", 2),
    ("trigger_eval.rs", 1),
    ("webhost_win.rs", 2),
];


/// The text of the log in memory, and the retrospective ring beside it.
#[derive(Debug)]
pub struct Log {
    on: bool,
    body: String,
    header: String,
    ring: VecDeque<String>,
    ring_bytes: usize,
    cap: usize,
    ring_cap: usize,
}

impl Default for Log {
    fn default() -> Log {
        Log::new()
    }
}

impl Log {
    pub const fn new() -> Log {
        Log::with_cap(MAX_LOG)
    }

    /// A log with its own cap in bytes (tests).
    pub const fn with_cap(cap: usize) -> Log {
        Log::with_caps(cap, RING_MAX)
    }

    /// A log with its own body and ring caps in bytes (tests).
    pub const fn with_caps(cap: usize, ring_cap: usize) -> Log {
        Log { on: false, body: String::new(), header: String::new(), ring: VecDeque::new(), ring_bytes: 0, cap, ring_cap }
    }

    /// Starts logging from empty with this header (`DL:554-583`).
    pub fn start(&mut self, header: String) {
        self.on = true;
        self.header = header;
        self.body.clear();
    }

    /// Stops: the header and the body, or None when it was off.
    pub fn stop(&mut self) -> Option<(String, String)> {
        if !self.on {
            return None;
        }
        self.on = false;
        Some((std::mem::take(&mut self.header), std::mem::take(&mut self.body)))
    }

    pub fn is_on(&self) -> bool {
        self.on
    }

    /// Keeps one entry while on; past the cap the oldest half becomes `TRUNCATED` (`DL:452-457`).
    pub fn append(&mut self, e: &str) {
        if !self.on {
            return;
        }
        self.body.push_str(e);
        if self.body.len() > self.cap {
            let mut cut = self.cap / 2;
            while !self.body.is_char_boundary(cut) {
                cut += 1;
            }
            self.body.replace_range(..cut, TRUNCATED);
        }
    }

    /// A retrospective line: the debug log's own while logging is on, else the newest 10 MB of
    /// the ring (`DL:333-366`).
    pub fn remember(&mut self, e: &str) {
        if self.on {
            return self.append(e);
        }
        let e = scrub(e);
        self.ring_bytes += e.len();
        self.ring.push_back(e);
        while self.ring_bytes > self.ring_cap && self.ring.len() > 1 {
            if let Some(old) = self.ring.pop_front() {
                self.ring_bytes -= old.len();
            }
        }
    }

    /// The ring's lines, oldest first.
    pub fn ring_text(&self) -> String {
        self.ring.iter().map(String::as_str).collect()
    }

    /// Forgets the ring.
    pub fn clear_ring(&mut self) {
        self.ring.clear();
        self.ring_bytes = 0;
    }

    /// How many lines the ring holds.
    pub fn ring_len(&self) -> usize {
        self.ring.len()
    }
}

/// The file operations a save needs (a fake in tests, the owner-only disk for real).
pub trait Write3 {
    /// A new, empty file in place of any old one.
    fn create(&mut self, p: &Path) -> std::io::Result<()>;
    /// More bytes at the end of an existing file.
    fn append(&mut self, p: &Path, chunk: &[u8]) -> std::io::Result<()>;
}

/// Header, body and footer as three writes (`DL:198-239`): onto the end of an existing file only
/// when `append`, else after a fresh `create` (also when appending to a file that is not there).
pub fn write(path: &Path, header: &str, body: &str, footer: &str, append: bool, exists: bool, fs: &mut dyn Write3) -> Result<(), String> {
    if !(append && exists) {
        fs.create(path).map_err(|_| could_not_open(path))?;
    }
    for chunk in [header, body, footer] {
        fs.append(path, chunk.as_bytes()).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// What the header and the footer say, in iTerm2's layout (`DL:98-150`). Counts and numbers
/// only: no window title, tab title, folder or command line, which can hold a secret.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    pub version: String,
    pub date: String,
    pub unix: i64,
    pub machine: String,
    pub screens: Vec<String>,
    pub os: String,
    pub key_window: Option<usize>,
    pub windows: Vec<WindowFact>,
    pub order: Vec<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowFact {
    pub number: usize,
    pub tabs: usize,
    pub panes: usize,
}

fn plural(n: usize, one: &str) -> String {
    format!("{n} {one}{}", if n == 1 { "" } else { "s" })
}

fn joined<T: ToString>(items: &[T]) -> String {
    if items.is_empty() { "none".into() } else { items.iter().map(T::to_string).collect::<Vec<_>>().join(", ") }
}

fn screens_line(f: &Facts) -> String {
    if f.screens.is_empty() { "none".into() } else { f.screens.join("; ") }
}

fn windows_line(f: &Facts) -> String {
    if f.windows.is_empty() {
        return "none".into();
    }
    f.windows.iter().map(|w| format!("{} ({}, {})", w.number, plural(w.tabs, "tab"), plural(w.panes, "pane"))).collect::<Vec<_>>().join("; ")
}

pub fn header(f: &Facts) -> String {
    let key = f.key_window.map_or("none".to_string(), |n| n.to_string());
    format!(
        "Tandem version: {}\nDate: {} ({})\nMachine: {}\nScreens: {}\nOS version: {}\nKey window: {}\nWindows: {}\nOrdered windows: {}\nPinned messages: \n------ END HEADER ------\n\n",
        f.version,
        f.date,
        f.unix,
        f.machine,
        screens_line(f),
        f.os,
        key,
        windows_line(f),
        joined(&f.order)
    )
}

pub fn footer(f: &Facts) -> String {
    format!("------ BEGIN FOOTER -----\nScreens: {}\nWindows: {}\nOrdered windows: {}\n", screens_line(f), windows_line(f), joined(&f.order))
}

/// The footer of a log saved while Tandem ends: no window is left to describe.
pub const EXIT_FOOTER: &str = "------ BEGIN FOOTER -----\nTandem was ending; no window facts were taken.\n";

// ---- the process-wide log ----

static LOG: Mutex<Log> = Mutex::new(Log::new());
static ON: AtomicBool = AtomicBool::new(false);
static APPEND: AtomicBool = AtomicBool::new(false);
static SYSLOG: AtomicBool = AtomicBool::new(false);
static MAIN: OnceLock<ThreadId> = OnceLock::new();

/// One atomic load: `dlog!` does nothing and formats nothing while this is false.
pub fn is_on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// Records the event loop's thread, so entries name it "main" (`run` calls it first).
pub fn mark_main_thread() {
    let _ = MAIN.set(std::thread::current().id());
}

/// `LogToSyslog`: entries also send their prefix to the debug output.
pub fn set_syslog(on: bool) {
    SYSLOG.store(on, Ordering::Relaxed);
}

/// Starts the log from empty. Without `append` the old file goes first (`DL:554-579`).
pub fn start_logging(header: String, append: bool) {
    #[cfg(windows)]
    if !append {
        remove_old_files();
    }
    let mut l = LOG.lock().unwrap_or_else(|e| e.into_inner());
    APPEND.store(append, Ordering::Relaxed);
    l.start(header);
    ON.store(true, Ordering::Relaxed);
}

/// Stops the log and hands back its header, body and whether the file is appended to. None when
/// it was off. Nothing is written here: the caller saves where it may take time.
pub fn stop() -> Option<(String, String, bool)> {
    let mut l = LOG.lock().unwrap_or_else(|e| e.into_inner());
    ON.store(false, Ordering::Relaxed);
    let append = APPEND.load(Ordering::Relaxed);
    l.stop().map(|(h, b)| (h, b, append))
}

/// Stops and writes on this thread; the file's path when logging was on.
#[cfg(windows)]
pub fn stop_logging(footer: &dyn Fn() -> String) -> Result<Option<PathBuf>, String> {
    match stop() {
        None => Ok(None),
        Some((h, b, append)) => save(&h, &b, &footer(), append).map(Some),
    }
}

/// The one teardown's step (`AD:1130`, P21): stops and writes silently, on every way out, a panic
/// on the main thread included. The panic hook runs before the stack unwinds, so a thread that
/// panicked inside the log's own lock must not wait for it: `try_lock`, or give up.
pub fn stop_at_exit() {
    if !is_on() {
        return;
    }
    let taken = {
        let mut l = match LOG.try_lock() {
            Ok(l) => l,
            Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return,
        };
        ON.store(false, Ordering::Relaxed);
        l.stop()
    };
    #[cfg(windows)]
    if let Some((h, b)) = taken {
        // A save already running finishes first; give up after EXIT_WAIT (the process is ending).
        let start = std::time::Instant::now();
        let guard = loop {
            match SAVE.try_lock() {
                Ok(g) => break Some(g),
                Err(std::sync::TryLockError::Poisoned(p)) => break Some(p.into_inner()),
                Err(std::sync::TryLockError::WouldBlock) if start.elapsed() < EXIT_WAIT => std::thread::sleep(std::time::Duration::from_millis(10)),
                Err(std::sync::TryLockError::WouldBlock) => break None,
            }
        };
        if guard.is_some() {
            let _ = save_locked(&h, &b, EXIT_FOOTER, APPEND.load(Ordering::Relaxed));
        }
    }
    #[cfg(not(windows))]
    drop(taken);
}

fn thread_name() -> &'static str {
    if MAIN.get() == Some(&std::thread::current().id()) { "main" } else { "other" }
}

/// What `dlog!` calls once it knows logging is on.
#[doc(hidden)]
pub fn emit(file: &str, line: u32, module: &str, args: fmt::Arguments<'_>) {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let (secs, micros) = (now.as_secs() as i64, now.subsec_micros());
    let file = file.rsplit(['/', '\\']).next().unwrap_or(file);
    if SYSLOG.load(Ordering::Relaxed) {
        debug_output(&syslog_prefix(secs, micros, file, line, module));
    }
    let e = entry(secs, micros, file, line, module, thread_name(), &args.to_string());
    // Never held across a call out: the entry is built first.
    LOG.lock().unwrap_or_else(|e| e.into_inner()).append(&e);
}

/// What `rlog!` calls: while logging, the line goes to the debug log as `dlog!` would; else it
/// is scrubbed into the ring (`DLH:109-128`).
#[doc(hidden)]
pub fn emit_retro(file: &str, line: u32, module: &str, args: fmt::Arguments<'_>) {
    if is_on() {
        return emit(file, line, module, args);
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let file = file.rsplit(['/', '\\']).next().unwrap_or(file);
    let e = ring_entry(now.as_secs() as i64, now.subsec_micros(), file, line, module, &args.to_string());
    LOG.lock().unwrap_or_else(|e| e.into_inner()).remember(&e);
}

/// The ring's text, oldest line first.
pub fn ring_text() -> String {
    LOG.lock().unwrap_or_else(|e| e.into_inner()).ring_text()
}

pub fn clear_ring() {
    LOG.lock().unwrap_or_else(|e| e.into_inner()).clear_ring();
}

/// `RLogRedact` (`DL:313-331`): the full text only while debug logging is on, else the redacted
/// one. For a value a developer needs but a retrospective log may not keep.
pub fn redact<'a>(full: &'a str, redacted: &'a str) -> &'a str {
    if is_on() { full } else { redacted }
}

/// The debug output (`OutputDebugStringW`): slow while a debugger listens, so only the two keys
/// that ask for it call this, and only the prefix of a log entry or a number table goes (any
/// process of the session can read it).
pub fn debug_output(text: &str) {
    #[cfg(windows)]
    {
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: NUL-terminated and alive for the call.
        unsafe { windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringW(wide.as_ptr()) };
    }
    #[cfg(not(windows))]
    let _ = text;
}

/// A debug log line. Costs one atomic load while logging is off and formats nothing; every
/// argument must be `LogSafe` and the format may not capture a variable (`{name}`).
#[macro_export]
macro_rules! dlog {
    ($fmt:literal $(, $arg:expr)* $(,)?) => {
        if $crate::dlog::is_on() {
            const _: () = $crate::dlog::no_capture($fmt);
            $crate::dlog::emit(file!(), line!(), module_path!(), format_args!($fmt $(, $crate::dlog::safe(&$arg))*));
        }
    };
}

/// A retrospective log line (`RLog`): kept in a 10 MB ring while debug logging is off, in the
/// debug log while it is on. Same rules as `dlog!`: every argument `LogSafe`, no inline capture.
#[macro_export]
macro_rules! rlog {
    ($fmt:literal $(, $arg:expr)* $(,)?) => {
        {
            const _: () = $crate::dlog::no_capture($fmt);
            $crate::dlog::emit_retro(file!(), line!(), module_path!(), format_args!($fmt $(, $crate::dlog::safe(&$arg))*));
        }
    };
}

// ---- the file ----

/// The folders a log may go in, best first: `%TEMP%` when the judge accepts it (a share is never
/// one: the log holds window facts and the connection would send the person's hash), then
/// `%LOCALAPPDATA%\Tandem\Logs`.
#[cfg(windows)]
pub fn log_dirs_with(env: &dyn Fn(&str) -> Option<String>) -> Vec<localfs::LocalPath> {
    let mut out = Vec::new();
    if let Some(t) = env("TEMP").filter(|t| !t.is_empty()).and_then(localfs::LocalPath::judge) {
        out.push(t);
    }
    let logs = env("LOCALAPPDATA").filter(|t| !t.is_empty()).and_then(localfs::LocalPath::judge).and_then(|b| b.join("Tandem")).and_then(|b| b.join("Logs"));
    if let Some(logs) = logs {
        let _ = localfs::create_dir_all(&logs);
        out.push(logs);
    }
    out
}

#[cfg(windows)]
pub fn log_dirs() -> Vec<localfs::LocalPath> {
    log_dirs_with(&|k| std::env::var(k).ok())
}

/// One lock for every disk write of the log: a save, the private state files and the removal of
/// the old file at start. Without it an off/on/off during a large save lets the UI thread delete the
/// file a worker is writing, and a second save interleaves with the first.
#[cfg(windows)]
static SAVE: Mutex<()> = Mutex::new(());

/// How long the exit save waits for a save already running (the process is ending).
#[cfg(windows)]
const EXIT_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

#[cfg(windows)]
fn remove_old_files() {
    let _one = SAVE.lock().unwrap_or_else(|e| e.into_inner());
    for dir in log_dirs() {
        if let Some(f) = dir.unfollowed(FILE_NAME) {
            let _ = localfs::remove_unfollowed(&f);
        }
    }
}

/// The disk side of `write`: a new file is made owner-only and never through a link.
#[cfg(windows)]
struct Disk {
    file: localfs::Unfollowed,
    /// The one handle every chunk goes through: the file `prepare` judged, or the one `create`
    /// made. The name is never opened again, so a swap after the judgment is not followed.
    handle: Option<std::fs::File>,
}

#[cfg(windows)]
impl Write3 for Disk {
    fn create(&mut self, _: &Path) -> std::io::Result<()> {
        // An open handle on the old file would leave its delete pending and refuse CREATE_NEW.
        self.handle = None;
        match localfs::remove_unfollowed(&self.file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        self.handle = Some(localfs::create_owner_only(&self.file)?);
        Ok(())
    }

    fn append(&mut self, _: &Path, chunk: &[u8]) -> std::io::Result<()> {
        use std::io::Write;
        match self.handle.as_mut() {
            Some(f) => f.write_all(chunk),
            None => Err(std::io::Error::other("no file is open for the log")),
        }
    }
}

/// Whether an existing log file is this user's own plain single-link file with the owner-only
/// DACL `create_owner_only` makes (protected, one allow entry, for the user): a file with a wider
/// inherited DACL may be read by others, so it is not appended to.
#[cfg(windows)]
fn is_ours(f: &std::fs::File) -> bool {
    let Ok(st) = localfs::file_stat(f) else { return false };
    let owner_only = |sid: &str| localfs::dacl_sddl_of(f).is_ok_and(|s| s.starts_with("D:P") && s.matches('(').count() == 1 && s.contains(sid));
    st.is_file
        && !st.is_reparse
        && localfs::link_count(f).is_ok_and(|n| n == 1)
        && matches!((crate::winpipe::file_owner_sid(f), crate::winpipe::user_sid()), (Ok(a), Ok(b)) if a == b && owner_only(&b))
}

/// What `prepare` found at the log's name.
#[cfg(windows)]
enum Prepared {
    /// No file (or one moved aside): a fresh one is made.
    Fresh,
    /// The user's own file, open for appending: the writes go through this handle.
    Append(std::fs::File),
}

/// Looks at the file before the write: whether to append to it. A file that is not ours, or a
/// link, is renamed away; one the new text would push past `cap` becomes the `.old` generation.
#[cfg(windows)]
fn prepare(dir: &localfs::LocalPath, file: &localfs::Unfollowed, append: bool, incoming: u64, cap: u64) -> Result<Prepared, String> {
    let open = localfs::open_unfollowed(file, |o| {
        o.read(true).append(true);
    });
    let f = match open {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Prepared::Fresh),
        Err(_) => return aside(dir, file).map(|_| Prepared::Fresh),
    };
    if !is_ours(&f) {
        drop(f);
        return aside(dir, file).map(|_| Prepared::Fresh);
    }
    let len = localfs::file_stat(&f).map_or(0, |s| s.len);
    if append && len + incoming > cap {
        drop(f);
        let old = dir.unfollowed(OLD_NAME).ok_or_else(|| could_not_open(dir.path()))?;
        match localfs::remove_unfollowed(&old) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
        localfs::rename_unfollowed(file, &old).map_err(|e| e.to_string())?;
        return Ok(Prepared::Fresh);
    }
    Ok(Prepared::Append(f))
}

/// Moves an entry that is not ours to `debuglog.stale-<unix>.txt`.
#[cfg(windows)]
fn aside(dir: &localfs::LocalPath, file: &localfs::Unfollowed) -> Result<(), String> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let to = dir.unfollowed(format!("debuglog.stale-{}-{}.txt", now.as_secs(), now.subsec_nanos())).ok_or_else(|| could_not_open(dir.path()))?;
    localfs::rename_unfollowed(file, &to).map_err(|e| e.to_string())
}

/// Saves into one folder; the file's path.
#[cfg(windows)]
pub fn save_in(dir: &localfs::LocalPath, header: &str, body: &str, footer: &str, append: bool) -> Result<PathBuf, String> {
    save_in_capped(dir, header, body, footer, append, MAX_FILE)
}

#[cfg(windows)]
pub fn save_in_capped(dir: &localfs::LocalPath, header: &str, body: &str, footer: &str, append: bool, cap: u64) -> Result<PathBuf, String> {
    let file = dir.unfollowed(FILE_NAME).ok_or_else(|| could_not_open(dir.path()))?;
    let (exists, handle) = match prepare(dir, &file, append, (header.len() + body.len() + footer.len()) as u64, cap)? {
        Prepared::Fresh => (false, None),
        Prepared::Append(f) => (true, Some(f)),
    };
    let mut disk = Disk { file: file.clone(), handle };
    write(file.path(), header, body, footer, append, exists, &mut disk)?;
    Ok(file.path().to_path_buf())
}

/// A small owner-only file in the first log folder that takes it (the state size files): a
/// plain `name`, replaced when it exists, never written through a link. The file's path.
#[cfg(windows)]
pub fn save_private(name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    use std::io::Write;
    let _one = SAVE.lock().unwrap_or_else(|e| e.into_inner());
    let mut first_error = None;
    for dir in log_dirs() {
        let Some(file) = dir.unfollowed(name) else { continue };
        let made = match localfs::remove_unfollowed(&file) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
        .and_then(|()| localfs::create_owner_only(&file))
        .and_then(|mut f| f.write_all(bytes));
        match made {
            Ok(()) => return Ok(file.path().to_path_buf()),
            Err(e) => {
                first_error.get_or_insert(e.to_string());
            }
        }
    }
    Err(first_error.unwrap_or_else(|| could_not_open(Path::new(name))))
}

/// Saves into the first folder that takes it (iTerm2 reports the failure of its one folder).
#[cfg(windows)]
pub fn save(header: &str, body: &str, footer: &str, append: bool) -> Result<PathBuf, String> {
    let _one = SAVE.lock().unwrap_or_else(|e| e.into_inner());
    save_locked(header, body, footer, append)
}

/// `save` for a caller that holds `SAVE`.
#[cfg(windows)]
fn save_locked(header: &str, body: &str, footer: &str, append: bool) -> Result<PathBuf, String> {
    let mut first_error = None;
    for dir in log_dirs() {
        match save_in(&dir, header, body, footer, append) {
            Ok(p) => return Ok(p),
            Err(e) => {
                first_error.get_or_insert(e);
            }
        }
    }
    Err(first_error.unwrap_or_else(|| could_not_open(Path::new(FILE_NAME))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// The process-wide log is shared by tests that start it.
    fn global_guard() -> std::sync::MutexGuard<'static, ()> {
        static G: Mutex<()> = Mutex::new(());
        G.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A fake disk: records each call, and fails on request.
    #[derive(Default)]
    struct Fake {
        calls: Vec<String>,
        fail_create: bool,
        fail_append: bool,
    }

    impl Write3 for Fake {
        fn create(&mut self, p: &Path) -> std::io::Result<()> {
            self.calls.push(format!("create {}", p.display()));
            if self.fail_create { Err(std::io::Error::other("no")) } else { Ok(()) }
        }
        fn append(&mut self, p: &Path, chunk: &[u8]) -> std::io::Result<()> {
            self.calls.push(format!("append {} {}", p.display(), String::from_utf8_lossy(chunk)));
            if self.fail_append { Err(std::io::Error::other("disk full")) } else { Ok(()) }
        }
    }

    /// `DL:395-461`: the entry, the system log prefix, and the on/off life of a log.
    #[test]
    fn log_entries_follow_iterm2() {
        assert_eq!(entry(1700000000, 42, "bell_ui.rs", 33, "tandem::app::bell_ui", "main", "Beep"), "1700000000.000042 bell_ui.rs:33 (tandem::app::bell_ui) main: Beep\n");
        assert_eq!(syslog_prefix(1700000000, 42, "bell_ui.rs", 33, "tandem::app::bell_ui"), "TandemDebugLog: 1700000000.000042 bell_ui.rs:33 (tandem::app::bell_ui): ");
        let mut log = Log::default();
        assert!(!log.is_on());
        log.append("dropped\n");
        assert_eq!(log.stop(), None, "off: nothing kept, nothing to stop");
        log.start("HEAD\n".into());
        assert!(log.is_on());
        log.append("one\n");
        log.append("two\n");
        assert_eq!(log.stop(), Some(("HEAD\n".to_string(), "one\ntwo\n".to_string())));
        assert!(!log.is_on());
        assert_eq!(log.stop(), None, "a second stop has nothing");
        log.start("H2\n".into());
        assert_eq!(log.stop(), Some(("H2\n".to_string(), String::new())), "a new start begins empty");
    }

    /// `DL:452-457`: past the cap the oldest half becomes the truncation line.
    #[test]
    fn giant_log_truncates() {
        let mut log = Log::with_cap(100);
        log.start(String::new());
        let a = "a".repeat(60);
        let b = "b".repeat(60);
        log.append(&a);
        log.append(&b);
        let (_, body) = log.stop().unwrap();
        assert_eq!(body, format!("{TRUNCATED}{}{}", "a".repeat(10), b), "the first 50 bytes went");
        assert_eq!(TRUNCATED, "*GIANT LOG TRUNCATED*\n");
        assert_eq!(MAX_LOG, 200 * 1024 * 1024);
        // A cut never splits a character.
        let mut log = Log::with_cap(10);
        log.start(String::new());
        log.append("\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}");
        assert!(log.stop().unwrap().1.starts_with(TRUNCATED));
    }

    /// `DL:198-239`: header, body, footer in that order; append only to an existing file.
    #[test]
    fn flush_follows_iterm2() {
        let path = Path::new("C:\\t\\debuglog.txt");
        let mut f = Fake::default();
        write(path, "H", "B", "F", false, true, &mut f).unwrap();
        assert_eq!(f.calls, vec!["create C:\\t\\debuglog.txt", "append C:\\t\\debuglog.txt H", "append C:\\t\\debuglog.txt B", "append C:\\t\\debuglog.txt F"]);
        let mut f = Fake::default();
        write(path, "H", "B", "F", true, true, &mut f).unwrap();
        assert_eq!(f.calls.len(), 3, "append onto an existing file: no create");
        assert!(f.calls.iter().all(|c| c.starts_with("append")));
        let mut f = Fake::default();
        write(path, "H", "B", "F", true, false, &mut f).unwrap();
        assert!(f.calls[0].starts_with("create"), "append but no file yet: create");
        let mut f = Fake { fail_create: true, ..Default::default() };
        assert_eq!(write(path, "H", "B", "F", false, true, &mut f), Err(could_not_open(path)));
        assert_eq!(could_not_open(path), "could not open C:\\t\\debuglog.txt for writing");
        let mut f = Fake { fail_append: true, ..Default::default() };
        assert_eq!(write(path, "H", "B", "F", false, true, &mut f), Err("disk full".to_string()), "a write failure gives the system's reason");
        assert_eq!(ENABLED_HEADING, "Debug Logging Enabled");
        assert_eq!(ENABLED_TEXT, "Please reproduce the bug. Then toggle debug logging again to save the log.");
        assert_eq!(STOPPED_HEADING, "Debug Logging Stopped");
        assert_eq!(stopped_text(path), "Please send C:\\t\\debuglog.txt to the developers.");
        assert_eq!(SAVE_PROBLEM_HEADING, "Problem Saving Debug Log");
        assert_eq!(save_failed_text("disk full"), "Failed to save debug log: disk full");
        assert_eq!(AUTO_TITLE, "Debug logging turned on");
        assert_eq!(AUTO_DETAIL, "You may notice worse performance while debug logging is on.");
        assert_eq!(file_path(Path::new("C:\\Temp")), PathBuf::from("C:\\Temp").join("debuglog.txt"));
    }

    /// P2: only values that cannot carry a secret reach the log; a name is cleaned.
    #[test]
    fn log_arguments_are_log_safe() {
        assert_eq!(name("menu.copy-1 ok").to_string(), "menu.copy-1 ok");
        // The 13w x 13x merge: a URL is logged as its scheme and host only.
        assert_eq!(origin("https://user:pw@Example.COM:8443/p/a?token=x#f").to_string(), "https example.com");
        assert_eq!(origin("http://[::1]:80/x").to_string(), "http ???1?", "brackets are not name characters");
        assert_eq!(origin("data:text/html,<b>secret</b>").to_string(), "data");
        assert_eq!(origin("about:blank").to_string(), "about");
        assert_eq!(origin("iterm2-about:history").to_string(), "iterm2-about");
        assert_eq!(origin("https://a.test\\x/y").to_string(), "https a.test");
        assert_eq!(origin("not a url").to_string(), "");
        assert_eq!(origin("https://h\u{202e}x.test/").to_string(), "https h?x.test");
        assert_eq!(name("a/b:c\u{202e}\r\nx").to_string(), "a?b?c???x");
        assert_eq!(name(&"x".repeat(200)).to_string().len(), 64);
        assert_eq!(format!("{}", name("")), "");
        // The compile-time half: a `String` or a non-static `&str` is no `LogSafe`.
        fn takes<T: LogSafe + ?Sized>(_: &T) {}
        takes(&7u64);
        takes(&true);
        takes(&'c');
        takes(&"literal");
        takes(&std::time::Duration::from_secs(1));
        takes(&name("x"));
    }

    /// The header and footer: Tandem's facts in iTerm2's layout, counts and numbers only (P3).
    #[test]
    fn header_and_footer_follow_iterm2() {
        let facts = Facts {
            version: "1.2.3".into(),
            date: "2026-10-06 12:00:00".into(),
            unix: 1790000000,
            machine: "x86_64 Some CPU".into(),
            screens: vec!["0,0 1920x1080 @ 1.00".into(), "1920,0 2560x1440 @ 1.50".into()],
            os: "Windows 11 Pro build 26100".into(),
            key_window: Some(2),
            windows: vec![WindowFact { number: 1, tabs: 3, panes: 5 }, WindowFact { number: 2, tabs: 1, panes: 1 }],
            order: vec![2, 1],
        };
        let h = header(&facts);
        assert_eq!(
            h,
            "Tandem version: 1.2.3\nDate: 2026-10-06 12:00:00 (1790000000)\nMachine: x86_64 Some CPU\nScreens: 0,0 1920x1080 @ 1.00; 1920,0 2560x1440 @ 1.50\nOS version: Windows 11 Pro build 26100\nKey window: 2\nWindows: 1 (3 tabs, 5 panes); 2 (1 tab, 1 pane)\nOrdered windows: 2, 1\nPinned messages: \n------ END HEADER ------\n\n"
        );
        let f = footer(&facts);
        assert_eq!(f, "------ BEGIN FOOTER -----\nScreens: 0,0 1920x1080 @ 1.00; 1920,0 2560x1440 @ 1.50\nWindows: 1 (3 tabs, 5 panes); 2 (1 tab, 1 pane)\nOrdered windows: 2, 1\n");
        let none = Facts { key_window: None, windows: vec![], order: vec![], ..facts };
        assert!(header(&none).contains("Key window: none\nWindows: none\nOrdered windows: none\n"));
    }

    /// The retrospective ring keeps the newest lines within its byte bound.
    #[test]
    fn remember_keeps_the_newest() {
        let mut log = Log::default();
        let line = "x".repeat(1024 * 1024);
        for _ in 0..12 {
            log.remember(&line);
        }
        assert_eq!(log.ring_len(), 10, "10 MB of 1 MB lines");
        let mut on = Log::default();
        on.start(String::new());
        on.remember("goes to the debug log\n");
        assert_eq!(on.ring_len(), 0, "never while debug logging is on");
        assert_eq!(on.stop().unwrap().1, "goes to the debug log\n");
    }

    /// The macro costs nothing and formats nothing while off, and writes while on.
    #[test]
    fn the_macro_is_free_while_off() {
        fn boom() -> u64 {
            panic!("formatted while off")
        }
        let _g = global_guard();
        assert!(!is_on());
        crate::dlog!("never built {}", boom());
        start_logging("HEAD\n".into(), true);
        assert!(is_on());
        crate::dlog!("hello {}", 5u64);
        let (_, body, _) = stop().unwrap();
        assert!(body.contains(" dlog.rs:"), "the file name, not the path: {body}");
        assert!(body.contains("(tandem::dlog::tests) "), "{body}");
        assert!(body.ends_with(": hello 5\n"), "{body}");
        assert!(!is_on());
    }

    /// A program's name only: no folder, no `.exe`, no arguments.
    #[test]
    fn program_names_carry_no_arguments() {
        assert_eq!(program(r#""C:\Program Files\PowerShell\pwsh.exe" -NoLogo -Command secret"#).to_string(), "pwsh");
        assert_eq!(program("ssh user@host -i key").to_string(), "ssh");
        assert_eq!(program("").to_string(), "");
    }

    /// `{}` and `{:..}` are plain; `{secret}` and `{0}` capture or index a named value.
    #[test]
    fn inline_captures_are_found() {
        assert!(!has_capture("a {} b {:?} c {:x} {{literal}}"));
        assert!(has_capture("typed {secret}"));
        assert!(has_capture("{0} twice"));
        assert!(has_capture("{secret:?}"));
        assert!(has_capture("open {"));
        // The macro itself runs the check in a `const`, so every spelling gets it.
        let src = include_str!("dlog.rs").split("#[cfg(test)]").next().unwrap();
        assert_eq!(src.matches("const _: () = $crate::dlog::no_capture($fmt);").count(), 2, "dlog! and rlog! check their format at compile time");
    }

    /// Resume notes "Log sites": a Settings edit (any writer: the window, a script, remote
    /// settings) logs its setting's name and the kind of value, never the value.
    #[test]
    fn settings_edits_are_logged_by_name_only() {
        let _g = global_guard();
        let mut globals = crate::globals::Globals::empty(Path::new("never-written.toml"));
        start_logging("H\n".into(), true);
        globals.set("SomeTextSetting", toml::Value::String("hunter2-value".into()));
        globals.set("SomeFlagSetting", toml::Value::Boolean(true));
        let (_, body, _) = stop().unwrap();
        assert!(body.contains("setting changed: SomeTextSetting (text)\n"), "{body}");
        assert!(body.contains("setting changed: SomeFlagSetting (bool)\n"), "{body}");
        assert!(!body.contains("hunter2"), "{body}");
    }

    /// The sites the Resume notes list that were missing: restored windows (count), focus changes
    /// between panes, the trigger index.
    #[test]
    fn listed_log_sites_exist() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let read = |f: &str| std::fs::read_to_string(src.join(f)).unwrap();
        assert!(read("app/restore_ui.rs").contains("rlog!(\"restoring saved windows: {}\""));
        assert!(read("app/mod.rs").contains("dlog!(\"pane focus changed to {}\""));
        assert!(read("trigger_eval.rs").contains("dlog!(\"trigger {} fired: {}\", i, info.class)"));
        assert!(!read("app/api_requests.rs").contains("setting changed by a script"), "one site for every writer");
    }

    /// P2, review of Task 2: no public function turns an arbitrary `String` into a log-safe value.
    /// The `LogSafe` implementors are the numbers, `&'static str` and three types whose only
    /// constructors sanitize or build from numbers: `Name`/`Names` (`name`, `names`) and the
    /// timer `Table` (`perf::log_table`); none has a public `fn (String) -> Self`.
    #[test]
    fn no_string_becomes_log_safe_by_a_constructor() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut implementors = Vec::new();
        let mut dirs = vec![src];
        let impl_for = regex::Regex::new(r"LogSafe for ([A-Za-z_&' ]+?)\s*\{").unwrap();
        while let Some(d) = dirs.pop() {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    dirs.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    let t = std::fs::read_to_string(&p).unwrap();
                    let live = t.split("#[cfg(test)]\nmod tests").next().unwrap_or("");
                    for c in impl_for.captures_iter(live) {
                        implementors.push(c[1].to_string());
                    }
                    assert!(!live.contains("pub fn built("), "{p:?}: a public String-to-LogSafe constructor");
                }
            }
        }
        implementors.sort();
        assert_eq!(implementors, ["&'static str", "Names", "Table"], "the LogSafe implementors");
        let own = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/dlog.rs")).unwrap();
        let macro_list = own.lines().find(|l| l.starts_with("log_safe!(")).unwrap();
        assert!(!macro_list.contains("Built"), "{macro_list}");
        assert_eq!(names(&["claude".to_string(), "a b\"c".to_string()]).to_string(), "[\"claude\", \"a b?c\"]", "sanitized, in iTerm2's list spelling");
    }

    /// The class (Task 2 round 2): no public function that returns a log-safe type takes a runtime
    /// string, whatever it is called. Only `name`, `names` and `program` may (they sanitize and bound; `program` keeps one file name); every other
    /// constructor takes `&'static str`, numbers or other log-safe values.
    #[test]
    fn every_log_safe_constructor_refuses_runtime_strings() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let sig = regex::Regex::new(r"(?s)pub(?:\([a-z]+\))? fn (\w+)\s*(?:<[^>]*>)?\(([^)]*)\)\s*->\s*([^{;]+)").unwrap();
        let ret = regex::Regex::new(r"\b(Name|Names|Table|Box<dyn LogSafe>|impl LogSafe)\b").unwrap();
        let runtime = regex::Regex::new(r"&\s*(?:'[a-z]+\s+)?(?:mut\s+)?str\b|\bString\b|AsRef<|Into<|Display|ToString|\bimpl\b|\bT\b|&\[\s*String").unwrap();
        let (mut checked, mut dirs) = (Vec::new(), vec![src]);
        while let Some(d) = dirs.pop() {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    dirs.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    let t = std::fs::read_to_string(&p).unwrap();
                    let live = t.split("#[cfg(test)]\nmod tests").next().unwrap_or("");
                    for c in sig.captures_iter(live) {
                        // `Table` is perf's log-safe table; other files' `Table` types are not ours.
                        let ours = ret.captures(&c[3]).is_some_and(|m| &m[1] != "Table" || p.ends_with("perf.rs"));
                        if !ours {
                            continue;
                        }
                        checked.push(c[1].to_string());
                        let args = c[2].replace("&'static str", "");
                        let allowed = matches!(&c[1], "name" | "names" | "program" | "origin");
                        assert!(allowed || !runtime.is_match(&args), "{p:?}: `{}` returns a log-safe value from a runtime string ({})", &c[1], &c[2]);
                    }
                }
            }
        }
        checked.sort();
        assert_eq!(checked, ["log_table", "name", "names", "origin", "program"], "the log-safe constructors");
    }

    /// T2 m5: `emit` and `emit_retro` are public for the macros only; no other code calls them, so
    /// nothing reaches the log without `LogSafe` arguments and a checked format.
    #[test]
    fn emit_is_called_only_by_the_macros() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut dirs = vec![src.clone()];
        while let Some(d) = dirs.pop() {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    dirs.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") && !p.ends_with("dlog.rs") {
                    let t = std::fs::read_to_string(&p).unwrap();
                    let live = t.split("#[cfg(test)]\nmod tests").next().unwrap_or("");
                    assert!(!live.contains("dlog::emit(") && !live.contains("dlog::emit_retro("), "{p:?} calls the log's emit directly");
                }
            }
        }
    }

    /// Every live `dlog!` and `rlog!` call: (file, macro, format string, argument text).
    fn live_sites() -> Vec<(String, &'static str, String, String)> {
        let call = regex::Regex::new(r"\b(dlog|rlog)!\s*[(\[{]").unwrap();
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found = Vec::new();
        let mut dirs = vec![src.clone()];
        while let Some(d) = dirs.pop() {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    dirs.push(p);
                    continue;
                }
                if p.extension().is_none_or(|x| x != "rs") {
                    continue;
                }
                let rel = p.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
                let text = std::fs::read_to_string(&p).unwrap();
                // Tests and docs mention the macros; only the code above them counts. The split is
                // at the test module, not the first `#[cfg(test)]` (a test-only static above live
                // code hid webhost_win.rs's sites: the 13w x 13x merge).
                let live = text.split("#[cfg(test)]\nmod tests").next().unwrap_or("");
                for m in call.captures_iter(live) {
                    let mac: &'static str = if &m[1] == "dlog" { "dlog" } else { "rlog" };
                    let rest = &live[m.get(0).unwrap().end()..];
                    // The call ends at its matching bracket.
                    let mut depth = 1;
                    let mut end = rest.len();
                    for (i, c) in rest.char_indices() {
                        match c {
                            '(' | '[' | '{' => depth += 1,
                            ')' | ']' | '}' => {
                                depth -= 1;
                                if depth == 0 {
                                    end = i;
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    let body = &rest[..end];
                    let fmt = body.split('"').nth(1).unwrap_or("").to_string();
                    let args = body.rsplit_once('"').map_or(String::new(), |(_, a)| a.to_string());
                    found.push((rel.clone(), mac, fmt, args));
                }
            }
        }
        found
    }

    /// Every `dlog!` and `rlog!` call site is listed; a format string never names a secret-shaped
    /// word and never captures a variable inline (an inline capture skips the `LogSafe` check);
    /// and no argument is raw terminal input, a paste, a title, a URL, an environment value or a
    /// token: only a number, a flag, a literal or `name(..)` of a selector or a program.
    #[test]
    fn log_call_sites_are_listed_and_safe() {
        let sites = live_sites();
        let mut counts: Vec<(String, usize)> = Vec::new();
        for (rel, _, fmt, args) in &sites {
            for bad in FORBIDDEN_WORDS {
                assert!(!fmt.to_ascii_lowercase().contains(bad), "{rel}: a log format names `{bad}`: {fmt}");
            }
            assert!(!has_capture(fmt), "{rel}: an inline capture in a log format: {fmt}");
            for bad in FORBIDDEN_ARG_WORDS {
                assert!(!args.to_ascii_lowercase().contains(bad), "{rel}: a log argument names `{bad}` (raw input, a paste, a title, a url or a secret): {args}");
            }
            match counts.iter_mut().find(|(f, _)| f == rel) {
                Some((_, n)) => *n += 1,
                None => counts.push((rel.clone(), 1)),
            }
        }
        counts.sort();
        let want: Vec<(String, usize)> = CALL_SITES.iter().map(|(f, n)| (f.to_string(), *n)).collect();
        assert_eq!(counts, want, "a new dlog!/rlog! call site needs a line in CALL_SITES after a secrets review");
    }

    /// B3: the state size files are made by the same owner-only helper: replaced when present,
    /// one allow entry for the user.
    #[cfg(windows)]
    #[test]
    fn private_files_are_owner_only_and_replaced() {
        let name = format!("statesize.test-{}.txt", std::process::id());
        let first = save_private(&name, b"first\n").unwrap();
        let second = save_private(&name, b"2\n").unwrap();
        assert_eq!(first, second);
        assert_eq!(std::fs::read_to_string(&second).unwrap(), "2\n", "replaced, not appended");
        let dir = log_dirs().into_iter().find(|d| second.starts_with(d.path())).unwrap();
        let sddl = localfs::dacl_sddl(&dir.unfollowed(&name).unwrap()).unwrap();
        assert_eq!(sddl.matches("(A;").count(), 1, "{sddl}");
        let _ = localfs::remove_unfollowed(&dir.unfollowed(&name).unwrap());
        assert!(save_private("..\\x.txt", b"x").is_err() || !Path::new("..\\x.txt").exists());
    }

    /// The scrub and the ring (`DL:288-366`): keystroke fields are redacted, and secret shapes
    /// go too (P2).
    #[test]
    fn scrub_follows_iterm2() {
        assert_eq!(scrub("key chars=\"ab\" unmodchars=\"c\" x"), "key chars=\"[redacted]\" unmodchars=\"[redacted]\" x");
        assert_eq!(scrub("no keystrokes here"), "no keystrokes here");
        assert_eq!(scrub("chars=abc"), "chars=abc", "no quote, no scrub");
        assert_eq!(scrub("login password=hunter2 ok"), "login password=[redacted] ok");
        assert_eq!(scrub("Authorization: Bearer abc.def"), "Authorization: Bearer [redacted]");
        assert_eq!(scrub("cookie: sid=1"), "cookie: [redacted]");
        assert_eq!(scrub("open https://me:pw@host/x"), "open https://[redacted]@host/x");
    }

    /// `DL:333-366`: the ring keeps the newest lines that fit, never fewer than one; while
    /// logging the line goes to the body.
    #[test]
    fn ring_follows_iterm2() {
        assert_eq!(RING_MAX, 10 * 1024 * 1024);
        assert_eq!(ring_entry(1700000000, 42, "bell_ui.rs", 33, "tandem::app::bell_ui", "Beep"), "1700000000.000042 bell_ui.rs:33 (tandem::app::bell_ui): Beep\n");
        let mut log = Log::with_caps(MAX_LOG, 30);
        for i in 0..5 {
            log.remember(&format!("line number{i}\n"));
        }
        assert_eq!(log.ring_len(), 2, "two 13-byte lines fit in 30");
        assert_eq!(log.ring_text(), "line number3\nline number4\n");
        let mut tiny = Log::with_caps(MAX_LOG, 5);
        tiny.remember("a line longer than the cap\n");
        assert_eq!(tiny.ring_len(), 1, "never fewer than one");
        tiny.remember("next\n");
        assert_eq!(tiny.ring_text(), "next\n");
        let mut log = Log::default();
        log.remember("chars=\"q\" typed\n");
        assert_eq!(log.ring_text(), "chars=\"[redacted]\" typed\n", "the ring scrubs");
        log.clear_ring();
        assert_eq!(log.ring_text(), "");
        let mut on = Log::default();
        on.start(String::new());
        on.remember("to the body\n");
        assert_eq!(on.ring_len(), 0);
        assert_eq!(on.stop().unwrap().1, "to the body\n");
    }

    /// `RLogRedact` (`DL:313-331`): the full text only while logging; and `rlog!` keeps a line
    /// in the ring while off and in the log while on.
    #[test]
    fn rlog_and_redact_follow_iterm2() {
        let _g = global_guard();
        clear_ring();
        assert_eq!(redact("full", "redacted"), "redacted");
        crate::rlog!("kept {}", 3u64);
        assert!(ring_text().ends_with(": kept 3\n"), "{}", ring_text());
        assert!(ring_text().contains(" dlog.rs:"));
        start_logging("H\n".into(), true);
        assert_eq!(redact("full", "redacted"), "full");
        crate::rlog!("while on {}", 4u64);
        let (_, body, _) = stop().unwrap();
        assert!(body.ends_with(": while on 4\n"), "{body}");
        assert!(!ring_text().contains("while on"), "never in the ring while logging");
        clear_ring();
        assert_eq!(ring_text(), "");
        assert_eq!(NO_RING_HEADING, "No Retrospective Logs");
        assert_eq!(NO_RING_TEXT, "No retrospective debug logs have been recorded yet.");
        assert_eq!(RING_FILE, "retrospective-debuglog.txt");
        assert_eq!(RING_SAVE_TITLE, "Save Retrospective Debug Logs");
        assert_eq!(RING_SAVE_FAILED_HEADING, "Could Not Save");
        assert_eq!(ring_save_failed("disk full"), "Failed to save retrospective debug logs: disk full");
    }

    /// A click on a notification stops the log only when the automatic-start notice is still the
    /// last notification shown; every notification goes through `notify::show_with`, which counts.
    #[test]
    fn only_the_debug_notice_click_stops_logging() {
        assert!(same_notice(Some(4), 4));
        assert!(!same_notice(Some(4), 5), "another notification was shown since");
        assert!(!same_notice(None, 4));
        let n = include_str!("notify.rs");
        assert!(n.split("pub fn show_with(").nth(1).unwrap().split("\n}").next().unwrap().contains("SHOWN.fetch_add"), "show_with counts every notification");
        let a = include_str!("app/alerts_ui.rs");
        assert!(a.split("fn reveal_notified(").nth(1).unwrap().split("\n    }").next().unwrap().contains("same_notice("), "the click compares");
    }

    /// `AD:1130`, P21: the one teardown stops the log and writes the file, and a panic reaches
    /// it through the panic hook.
    #[test]
    fn the_teardown_saves_the_log() {
        let m = include_str!("app/mod.rs");
        let teardown = m.split("pub(crate) fn teardown_process()").nth(1).and_then(|r| r.split("\n}").next()).expect("teardown_process");
        assert!(teardown.contains("crate::dlog::stop_at_exit()"), "teardown writes the debug log");
        let run = m.split("pub fn run()").nth(1).unwrap();
        assert!(run.contains("crate::dlog::mark_main_thread()"), "the event loop's thread is recorded");
    }

    /// The pure choice of the log folder: `%TEMP%` only when the judge accepts it (B3).
    #[cfg(windows)]
    #[test]
    fn temp_on_a_share_falls_back() {
        let local = std::env::temp_dir();
        std::fs::create_dir_all(local.join("TandemDlogTest")).unwrap();
        let env = |share: bool| {
            let local = local.clone();
            move |k: &str| match k {
                "TEMP" if share => Some(r"\\srv\share\tmp".to_string()),
                "TEMP" => Some(local.to_string_lossy().to_string()),
                "LOCALAPPDATA" => Some(local.join("TandemDlogTest").to_string_lossy().to_string()),
                _ => None,
            }
        };
        let dirs: Vec<PathBuf> = log_dirs_with(&env(false)).into_iter().map(|d| d.into_path_buf()).collect();
        assert_eq!(dirs[0], local, "a local TEMP is the first choice");
        let dirs: Vec<PathBuf> = log_dirs_with(&env(true)).into_iter().map(|d| d.into_path_buf()).collect();
        assert!(dirs.iter().all(|d| !d.to_string_lossy().starts_with(r"\\")), "a share is never a candidate: {dirs:?}");
        assert!(dirs.first().is_some_and(|d| d.ends_with(r"TandemDlogTest\Tandem\Logs")), "Local AppData is the fallback: {dirs:?}");
    }

    #[cfg(windows)]
    fn scratch(tag: &str) -> localfs::LocalPath {
        let dir = std::env::temp_dir().join(format!("tandem-dlog-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        localfs::LocalPath::judge(&dir).expect("a temp folder is local")
    }

    /// B3: a new file has exactly one allow ACE, for this user; a planted link is refused and
    /// the entry renamed away; the file is the user's own.
    #[cfg(windows)]
    #[test]
    fn log_file_is_owner_only_and_unfollowed() {
        let dir = scratch("acl");
        let path = save_in(&dir, "H\n", "body\n", "F\n", false).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "H\nbody\nF\n");
        let sddl = localfs::dacl_sddl(&dir.unfollowed(FILE_NAME).unwrap()).unwrap();
        assert_eq!(sddl.matches("(A;").count(), 1, "one allow ACE: {sddl}");
        assert!(sddl.contains(&crate::winpipe::user_sid().unwrap()) && sddl.starts_with("D:P"), "for the user, protected: {sddl}");
        // A planted junction at the file's name is not followed and not written through.
        let target = scratch("acl-target");
        let _ = std::fs::remove_file(&path);
        let status = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&path).arg(target.path()).output().unwrap();
        assert!(status.status.success(), "mklink: {}", String::from_utf8_lossy(&status.stdout));
        let saved = save_in(&dir, "H2\n", "b2\n", "F2\n", true).unwrap();
        assert_eq!(std::fs::read_to_string(&saved).unwrap(), "H2\nb2\nF2\n", "a fresh file stands at the name");
        assert!(std::fs::read_dir(target.path()).unwrap().next().is_none(), "nothing was written through the junction");
        let stale = std::fs::read_dir(dir.path()).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("debuglog.stale-")).count();
        assert_eq!(stale, 1, "the planted entry was renamed away");
        let _ = std::fs::remove_dir_all(dir.path());
        let _ = std::fs::remove_dir_all(target.path());
    }

    /// T1 m1: one lock serializes every disk write of the log: `remove_old_files` (on the UI thread, at
    /// start) waits for a save that is running instead of deleting the file under it.
    #[cfg(windows)]
    #[test]
    fn saves_are_serialized() {
        let held = SAVE.lock().unwrap_or_else(|e| e.into_inner());
        let done = std::sync::Arc::new(AtomicBool::new(false));
        let d = done.clone();
        let t = std::thread::spawn(move || {
            remove_old_files();
            d.store(true, Ordering::SeqCst);
        });
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!done.load(Ordering::SeqCst), "remove_old_files must wait for a save in progress");
        drop(held);
        t.join().unwrap();
        assert!(done.load(Ordering::SeqCst));
    }

    /// T1 m3: an existing log whose DACL is wider than the one owner-only entry (inherited from the
    /// folder) is not appended to: it is renamed away and a fresh owner-only file starts.
    #[cfg(windows)]
    #[test]
    fn a_log_with_a_wider_dacl_is_not_appended_to() {
        let dir = scratch("wide");
        // Our own file (owner is the user, one ACE), then a second allow entry for Everyone.
        let first = save_in(&dir, "OLD\n", "", "", false).unwrap();
        let grant = std::process::Command::new("icacls").arg(&first).args(["/grant", "*S-1-1-0:R"]).output().unwrap();
        assert!(grant.status.success(), "icacls: {}", String::from_utf8_lossy(&grant.stdout));
        let wide = localfs::dacl_sddl(&dir.unfollowed(FILE_NAME).unwrap()).unwrap();
        assert!(wide.matches("(A;").count() > 1, "the file now has a wider DACL: {wide}");
        let saved = save_in(&dir, "H\n", "b\n", "F\n", true).unwrap();
        assert_eq!(std::fs::read_to_string(&saved).unwrap(), "H\nb\nF\n", "a fresh file, not appended");
        let sddl = localfs::dacl_sddl(&dir.unfollowed(FILE_NAME).unwrap()).unwrap();
        assert_eq!(sddl.matches("(A;").count(), 1, "{sddl}");
        let _ = std::fs::remove_dir_all(dir.path());
    }

    /// T1 m2: the file `prepare` judged is the handle the chunks go through (the name is not opened again).
    #[cfg(windows)]
    #[test]
    fn prepare_hands_back_the_judged_handle() {
        use std::io::Write;
        let dir = scratch("handle");
        let file = dir.unfollowed(FILE_NAME).unwrap();
        assert!(matches!(prepare(&dir, &file, true, 1, 1 << 20), Ok(Prepared::Fresh)), "no file yet");
        save_in(&dir, "one\n", "", "", false).unwrap();
        match prepare(&dir, &file, true, 1, 1 << 20) {
            Ok(Prepared::Append(mut f)) => f.write_all(b"two\n").unwrap(),
            _ => panic!("our own file is appended to through its handle"),
        }
        assert_eq!(std::fs::read_to_string(dir.path().join(FILE_NAME)).unwrap(), "one\ntwo\n");
        let _ = std::fs::remove_dir_all(dir.path());
    }

    /// B3: appending never grows the file past the cap; the old one is kept as `.old` once.
    #[cfg(windows)]
    #[test]
    fn append_is_capped() {
        let dir = scratch("cap");
        save_in_capped(&dir, "H1\n", "one\n", "F1\n", false, 64).unwrap();
        save_in_capped(&dir, "H2\n", "two\n", "F2\n", true, 64).unwrap();
        let p = dir.path().join(FILE_NAME);
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("H1") && text.contains("H2"), "under the cap it appends: {text}");
        let big = "x".repeat(100);
        save_in_capped(&dir, "H3\n", &big, "F3\n", true, 64).unwrap();
        let now = std::fs::read_to_string(&p).unwrap();
        assert!(now.starts_with("H3") && !now.contains("H1"), "over the cap a fresh file starts: {}", now.len());
        let old = std::fs::read_to_string(dir.path().join(OLD_NAME)).unwrap();
        assert!(old.contains("H1") && old.contains("H2"), "the earlier file is the one generation kept");
        save_in_capped(&dir, "H4\n", &big, "F4\n", true, 64).unwrap();
        let old = std::fs::read_to_string(dir.path().join(OLD_NAME)).unwrap();
        assert!(old.contains("H3") && !old.contains("H1"), "a second rotation replaces the first .old");
        let _ = std::fs::remove_dir_all(dir.path());
    }
}

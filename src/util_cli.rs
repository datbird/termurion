//! The utilities' dispatch (milestone 11e): the names `termurion-utils.exe` answers to, the
//! choice of utility from its own file name or its first argument, the `Io` context every
//! utility takes (streams, the console as a `Tty`, environment, working folder, a random
//! source and a command runner), the console input modes the scripts' `stty` calls stand
//! for, and the UTF-16 key decoding the console reader uses. Pure; `util_console` is the
//! real console.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

/// iTerm2's bundled utilities that Tandem ports, sorted. `it2` is the Swift program's two clipping verbs only (13y1 Task 7).
pub const NAMES: [&str; 18] = [
    "imgcat",
    "imgls",
    "it2",
    "it2api",
    "it2attention",
    "it2cat",
    "it2check",
    "it2copy",
    "it2dl",
    "it2getvar",
    "it2git",
    "it2profile",
    "it2setcolor",
    "it2setkeylabel",
    "it2ssh",
    "it2tip",
    "it2ul",
    "it2universion",
];

/// The multi-call program's own file stem.
pub const EXE_STEM: &str = "termurion-utils";

/// The utility a program path names: its file stem (after the last `\` or `/`, ".exe"
/// removed), compared without case.
pub fn name_of(argv0: &str) -> Option<&'static str> {
    let file = argv0.rsplit(['\\', '/']).next().unwrap_or(argv0);
    let lower = file.to_ascii_lowercase();
    let stem = lower.strip_suffix(".exe").unwrap_or(&lower);
    NAMES.iter().copied().find(|n| *n == stem)
}

/// The utility to run and its arguments: the one the program's own name names, else (run
/// as `termurion-utils`) the one its first argument names. Err is the usage text.
pub fn pick(args: &[String]) -> Result<(&'static str, Vec<String>), String> {
    if let Some(name) = args.first().and_then(|a| name_of(a)) {
        return Ok((name, args[1..].to_vec()));
    }
    match args.get(1).and_then(|a| name_of(a)) {
        Some(name) => Ok((name, args[2..].to_vec())),
        None => Err(multi_usage()),
    }
}

/// What `termurion-utils` run by itself prints.
pub fn multi_usage() -> String {
    format!("Usage: {EXE_STEM} <utility> [arguments...]\nUtilities: {}\n", NAMES.join(", "))
}

/// The console input mode a script's `stty` stands for: `Raw` is `stty -echo -icanon raw`
/// (it2check), `Cbreak` is `stty -echo` plus bash's character-mode `read -d` (it2getvar,
/// it2profile, it2ul), which keeps signals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Raw,
    Cbreak,
}

const ENABLE_PROCESSED_INPUT: u32 = 0x1;
const ENABLE_LINE_INPUT: u32 = 0x2;
const ENABLE_ECHO_INPUT: u32 = 0x4;
const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x200;

/// The console input mode for `m`, from the mode the console had: VT input on (replies
/// arrive as typed), line input and echo off, processed input (^C as a signal) kept only in
/// `Cbreak`; every other bit kept.
pub fn input_mode(m: Mode, current: u32) -> u32 {
    let off = match m {
        Mode::Raw => ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT,
        Mode::Cbreak => ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT,
    };
    (current | ENABLE_VIRTUAL_TERMINAL_INPUT) & !off
}

/// `/dev/tty`: the console the utility talks to directly.
pub trait Tty {
    fn write(&mut self, bytes: &[u8]);
    /// The next byte typed (or replied), waiting at most `wait` (None: without end). None
    /// when the wait ran out or there is no console.
    fn read(&mut self, wait: Option<Duration>) -> Option<u8>;
    fn set_mode(&mut self, m: Mode);
    /// Drops input not read yet.
    fn discard_input(&mut self);
    /// Puts back every mode `set_mode` changed.
    fn restore(&mut self);
    fn is_console(&self) -> bool;
}

/// A program a utility runs where its script runs one (curl, tar, git, python).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cmd {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub stdin: Option<Vec<u8>>,
    /// Standard streams shared with the utility; the output is not captured, and Ctrl+C is
    /// left to the program (the utility waits for it and returns its code).
    pub inherit: bool,
    /// Standard error discarded (the scripts' `2>/dev/null`).
    pub quiet: bool,
    /// Environment changes for the program: a value sets the variable, None removes it.
    pub env: Vec<(String, Option<String>)>,
    /// Standard error joins the captured standard output (the scripts' `2>&1`); ignored for
    /// an `inherit` run.
    pub merge_stderr: bool,
}

/// A finished program: its exit code and captured standard output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    pub code: i32,
    pub stdout: Vec<u8>,
}

/// Everything a utility touches outside itself.
pub struct Io<'a> {
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
    pub input: &'a mut dyn Read,
    /// `[ -t 0 ]` and `[ -t 1 ]`.
    pub stdin_tty: bool,
    pub stdout_tty: bool,
    pub tty: &'a mut dyn Tty,
    pub env: &'a dyn Fn(&str) -> Option<String>,
    pub cwd: PathBuf,
    /// `$RANDOM`: 0 to 32767.
    pub random: &'a mut dyn FnMut() -> u32,
    pub run: &'a mut dyn FnMut(&Cmd) -> Result<Output, String>,
    /// `builtin command -V <name>` succeeds (the scripts' `check_dependency`).
    pub have: &'a dyn Fn(&str) -> bool,
    /// An SSH origin secret from Tandem's secret pipe (`ssh_secret_win::fetch`); None when
    /// no Tandem of this user answers.
    pub secret: &'a mut dyn FnMut() -> Option<String>,
    /// `command env`: every environment variable, in the process's order.
    pub env_all: &'a dyn Fn() -> Vec<(String, String)>,
}

/// A path a utility may open (following links): not a network or device path in any
/// spelling (`prompted::network_path`), and no link on the way to it, itself included,
/// leading to one (`git_state::program_links_local`, which reads link targets without
/// opening them). Nothing else is ever probed.
pub fn may_open(path: &std::path::Path) -> bool {
    let text = path.to_string_lossy();
    let local = local_form(&text);
    !crate::prompted::network_path(local) && crate::git_state::program_links_local(std::path::Path::new(local))
}

/// A verbatim local path (`\\?\` and a drive path) in its plain form, `git_state`'s link
/// rule; anything else as it is, so `\\?\UNC\`, `\\?\GLOBALROOT\`, `\\?\Volume{...}\`,
/// `\\.\` and `\??\` still read as network or device paths.
pub fn local_form(path: &str) -> &str {
    path.strip_prefix(r"\\?\").filter(|rest| crate::prompted::local_path_ok(rest)).unwrap_or(path)
}

/// `cwd` joined with `arg`, both in `local_form`.
pub fn join_local(cwd: &std::path::Path, arg: &str) -> PathBuf {
    let cwd = cwd.to_string_lossy();
    std::path::Path::new(local_form(&cwd)).join(local_form(arg))
}

/// The folders of a PATH value that may be looked in, in order: each `;`-separated entry
/// trimmed and unquoted (`"C:\Program Files\x"` is that folder), in `local_form`, kept only
/// when it is a local drive path (`prompted::local_path_ok`: never the working folder, a
/// relative, drive-relative, rooted or network folder) with no link on the way to a network
/// path (`may_open`). Nothing but link targets is read. The one rule for every PATH walker:
/// `found_on_path`, `util_api::python_candidates` and `git_state::find_git`.
pub fn path_dirs(path_var: &str) -> impl Iterator<Item = PathBuf> + '_ {
    path_var
        .split(';')
        .map(crate::git_state::unquote)
        .map(local_form)
        .filter(|d| crate::prompted::local_path_ok(d))
        .map(PathBuf::from)
        .filter(|d| may_open(d))
}

/// `builtin command -V <program>` over `path_var`, `pathext` and `cwd`: a name with a
/// separator is that file (in `cwd` when relative); else a file of that name, with a
/// `pathext` extension or none, in a `path_dirs` folder. `is_file` is asked only about paths
/// that pass `may_open`.
pub fn found_on_path(program: &str, path_var: &str, pathext: &str, cwd: &std::path::Path, is_file: &dyn Fn(&std::path::Path) -> bool) -> bool {
    let local = |p: &std::path::Path| may_open(p) && is_file(p);
    if program.contains(['\\', '/']) {
        return !crate::prompted::network_path(local_form(program)) && local(&join_local(cwd, program));
    }
    path_dirs(path_var).any(|dir| {
        let base = dir.join(program);
        local(&base) || pathext.split(';').filter(|e| !e.is_empty()).any(|e| local(&PathBuf::from(format!("{}{e}", base.display()))))
    })
}

/// `$TERM`.
pub fn term(io: &Io) -> Option<String> {
    (io.env)("TERM")
}

/// Runs utility `name` with `args`; returns its exit code. Each utility's task adds its arm
/// here; a name not built yet prints the usage. A utility returns its exit code and never
/// calls `std::process::exit`: the console's restore (`Drop`, then `real_main`'s restore of
/// every saved mode) runs only when it returns.
pub fn run(name: &str, args: &[String], io: &mut Io) -> i32 {
    use crate::{util_files, util_git, util_query, util_simple, util_tip};
    match name {
        "imgcat" => util_files::imgcat(args, io),
        "it2cat" => util_files::it2cat(args, io),
        "it2dl" => util_files::it2dl(args, io),
        "it2copy" => util_simple::it2copy(args, io),
        "it2setcolor" => util_simple::it2setcolor(args, io),
        "it2attention" => util_simple::it2attention(args, io),
        "it2universion" => util_simple::it2universion(args, io),
        "it2setkeylabel" => util_simple::it2setkeylabel(args, io),
        "it2git" => util_git::it2git(args, io),
        "it2tip" => util_tip::it2tip(args, io),
        "it2check" => util_query::it2check(args, io),
        "it2getvar" => util_query::it2getvar(args, io),
        "it2profile" => util_query::it2profile(args, io),
        "it2ul" => util_query::it2ul(args, io),
        "it2api" => crate::util_api::it2api(args, io),
        "it2" => crate::it2_session::it2(args, io),
        "it2ssh" => crate::util_it2ssh::run(args, io),
        #[cfg(windows)]
        "imgls" => {
            let fs = crate::util_imgls::WinFs::new(io.cwd.clone());
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
            // `stamps::local` takes unsigned seconds: a time before 1970 reads as 1970.
            let local = |t: i64| crate::stamps::local(t.max(0) as u64);
            crate::util_imgls::imgls(args, io, &fs, now, &local)
        }
        _ => {
            let _ = io.err.write_all(multi_usage().as_bytes());
            2
        }
    }
}

/// What a console focus-in record reads as while a reply is awaited: ConPTY's input parser
/// takes a reply's `ESC [ I` (as in `ESC [ ITERM2 ...`) for a focus report.
pub const FOCUS_IN: &[u8] = b"\x1b[I";

/// How Tandem's `CSI 1337 n` answer starts once ConPTY has swallowed its `ESC [ I`.
pub const SWALLOWED_REPLY: &[u8] = b"TERM2 ";

/// The shipped ConPTY asks the terminal for focus reports in its own start-up handshake and
/// takes every `ESC [ I` in its input for one, so `ESC [ ITERM2 3.7.0 n` reaches the program
/// as `TERM2 3.7.0 n` (measured, `conpty_carries_replies`). After `CSI 1337 n`, a reply
/// starting "TERM2 " gets its `ESC [ I` back; anything else is unchanged. it2check's reader
/// applies this on Windows; the version bytes are still compared by the script's rule.
pub fn restore_reply_prefix(reply: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(reply.len() + FOCUS_IN.len());
    if reply.starts_with(SWALLOWED_REPLY) {
        out.extend_from_slice(FOCUS_IN);
    }
    out.extend_from_slice(reply);
    out
}

/// Console key characters (UTF-16 units) to UTF-8: a surrogate pair is joined, a lone
/// surrogate is U+FFFD.
#[derive(Debug, Default)]
pub struct KeyText {
    high: Option<u16>,
}

impl KeyText {
    pub fn push(&mut self, unit: u16, out: &mut Vec<u8>) {
        let mut put = |c: char| {
            let mut b = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
        };
        match unit {
            0xD800..=0xDBFF => {
                if self.high.replace(unit).is_some() {
                    put('\u{FFFD}');
                }
            }
            0xDC00..=0xDFFF => match self.high.take() {
                Some(h) => {
                    let c = 0x10000 + ((u32::from(h) - 0xD800) << 10) + (u32::from(unit) - 0xDC00);
                    put(char::from_u32(c).unwrap_or('\u{FFFD}'));
                }
                None => put('\u{FFFD}'),
            },
            _ => {
                if self.high.take().is_some() {
                    put('\u{FFFD}');
                }
                put(char::from_u32(u32::from(unit)).unwrap_or('\u{FFFD}'));
            }
        }
    }

    /// A high surrogate still waiting for its pair (input ended) becomes U+FFFD.
    pub fn flush(&mut self, out: &mut Vec<u8>) {
        if self.high.take().is_some() {
            out.extend_from_slice("\u{FFFD}".as_bytes());
        }
    }
}

/// A fresh folder under the temp folder for one test, removed when dropped.
#[cfg(test)]
pub(crate) struct TempDir(pub PathBuf);

#[cfg(test)]
impl TempDir {
    pub fn new(name: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
}

#[cfg(test)]
impl std::ops::Deref for TempDir {
    type Target = std::path::Path;
    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}

#[cfg(test)]
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A console for tests: records what is written and which modes were set, and replays
/// `replies` (None: a wait that ran out).
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct FakeTty {
    pub written: Vec<u8>,
    pub replies: std::collections::VecDeque<Option<u8>>,
    pub modes: Vec<Mode>,
    pub discarded: bool,
    pub restored: bool,
    pub console: bool,
}

#[cfg(test)]
impl Tty for FakeTty {
    fn write(&mut self, bytes: &[u8]) {
        self.written.extend_from_slice(bytes);
    }
    fn read(&mut self, _wait: Option<Duration>) -> Option<u8> {
        self.replies.pop_front().flatten()
    }
    fn set_mode(&mut self, m: Mode) {
        self.modes.push(m);
    }
    fn discard_input(&mut self) {
        self.discarded = true;
    }
    fn restore(&mut self) {
        self.restored = true;
    }
    fn is_console(&self) -> bool {
        self.console
    }
}

/// A utility run in memory: its streams, environment, console, the programs it ran (each
/// answered from `answers`, else an error), the commands `have` reports missing, the
/// `$RANDOM` values it gets, and the SSH origin secrets it fetches (none left: None).
#[cfg(test)]
#[derive(Default)]
pub(crate) struct Harness {
    pub out: Vec<u8>,
    pub err: Vec<u8>,
    pub input: Vec<u8>,
    pub stdin_tty: bool,
    pub stdout_tty: bool,
    pub env: std::collections::BTreeMap<String, String>,
    pub cwd: PathBuf,
    pub tty: FakeTty,
    pub runs: Vec<Cmd>,
    pub answers: std::collections::VecDeque<Result<Output, String>>,
    pub missing: Vec<String>,
    pub randoms: std::collections::VecDeque<u32>,
    pub secrets: std::collections::VecDeque<Option<String>>,
}

#[cfg(test)]
impl Harness {
    pub fn run(&mut self, name: &str, args: &[&str]) -> i32 {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        self.with_io(&mut |io| run(name, &args, io))
    }

    /// Calls `f` with an `Io` over the harness's fields (a utility's inner entry point).
    pub fn with_io(&mut self, f: &mut dyn FnMut(&mut Io) -> i32) -> i32 {
        let Harness { out, err, input, stdin_tty, stdout_tty, env, cwd, tty, runs, answers, missing, randoms, secrets } = self;
        let mut input: &[u8] = input;
        let env_fn = |k: &str| env.get(k).cloned();
        let have = |p: &str| !missing.iter().any(|m| m == p);
        let mut random = || randoms.pop_front().expect("Harness: no $RANDOM value left");
        let mut secret = || secrets.pop_front().flatten();
        let env_all = || env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        let mut run_fn = |c: &Cmd| {
            runs.push(c.clone());
            answers.pop_front().unwrap_or_else(|| Err(format!("Harness: no answer for {}", c.program)))
        };
        let mut io = Io {
            out,
            err,
            input: &mut input,
            stdin_tty: *stdin_tty,
            stdout_tty: *stdout_tty,
            tty,
            env: &env_fn,
            cwd: cwd.clone(),
            random: &mut random,
            run: &mut run_fn,
            have: &have,
            secret: &mut secret,
            env_all: &env_all,
        };
        f(&mut io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn names_follow_the_bundle() {
        let mut sorted = NAMES;
        sorted.sort();
        assert_eq!(sorted, NAMES);
        assert_eq!(NAMES.len(), 18);
        assert!(NAMES.contains(&"it2ssh") && NAMES.contains(&"it2"));
        assert_eq!(name_of("C:\\t\\utilities\\IMGCAT.EXE"), Some("imgcat"));
        assert_eq!(name_of("it2ul"), Some("it2ul"));
        assert_eq!(name_of("termurion-utils.exe"), None);
        assert_eq!(pick(&strings(&["C:\\t\\termurion-utils.exe", "it2check", "2.0"])), Ok(("it2check", strings(&["2.0"]))));
        assert_eq!(pick(&strings(&["imgcat.exe", "a.png"])), Ok(("imgcat", strings(&["a.png"]))));
        assert_eq!(pick(&strings(&["termurion-utils.exe"])), Err(multi_usage()));
        assert_eq!(pick(&strings(&["termurion-utils.exe", "it2ssh", "h"])), Ok(("it2ssh", strings(&["h"]))));
        assert_eq!(pick(&strings(&["termurion-utils.exe", "it2", "session"])), Ok(("it2", strings(&["session"]))));
        assert!(multi_usage().starts_with("Usage: termurion-utils <utility> [arguments...]\nUtilities: imgcat, imgls, "));
        assert!(multi_usage().ends_with(", it2universion\n"));
    }

    #[test]
    fn unbuilt_utilities_print_the_usage() {
        let mut h = Harness::default();
        assert_eq!(h.run("no-such", &[]), 2);
        assert_eq!(String::from_utf8(h.err).unwrap(), multi_usage());
        assert!(h.out.is_empty());
    }

    #[test]
    fn input_modes_follow_stty() {
        assert_eq!(input_mode(Mode::Raw, 0x1F7), 0x3F0);
        assert_eq!(input_mode(Mode::Cbreak, 0x1F7), 0x3F1);
        for current in [0, 0x1F7, 0x200, 0x3FF, 0x7] {
            for m in [Mode::Raw, Mode::Cbreak] {
                assert_ne!(input_mode(m, current) & 0x200, 0, "{m:?} {current:#x}");
            }
        }
    }

    #[test]
    fn console_keys_become_bytes() {
        let mut k = KeyText::default();
        let mut out = Vec::new();
        k.push(0x41, &mut out);
        assert_eq!(out, b"A");
        out.clear();
        k.push(0xD83D, &mut out);
        assert!(out.is_empty(), "nothing after the first unit");
        k.push(0xDE00, &mut out);
        assert_eq!(out, "\u{1F600}".as_bytes());
        assert_eq!(out.len(), 4);
        out.clear();
        k.push(0xDE00, &mut out);
        assert_eq!(out, [0xEF, 0xBF, 0xBD]);
        out.clear();
        k.push(0xE9, &mut out);
        assert_eq!(out, [0xC3, 0xA9]);
        out.clear();
        k.push(0xD83D, &mut out);
        k.flush(&mut out);
        assert_eq!(out, [0xEF, 0xBF, 0xBD], "a trailing lone high surrogate");
        out.clear();
        k.flush(&mut out);
        assert!(out.is_empty(), "nothing left to flush");
    }

    #[test]
    fn network_paths_are_never_opened() {
        use std::path::Path;
        for p in ["\\\\host\\s\\x", "//host/s/x", "/\\host\\s", "\\/host/s", "\\\\?\\UNC\\host\\s", "\\\\.\\pipe\\x", "\\??\\UNC\\host\\s"] {
            assert!(!may_open(Path::new(p)), "{p}");
        }
        let dir = std::env::temp_dir().join(format!("tandem-util-cli-open-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(may_open(&dir.join("a.txt")), "a local path that is not there yet");
        // A verbatim local path is read in its plain form (git_state's link rule); every
        // other verbatim or device form stays refused.
        let verbatim = format!("\\\\?\\{}", dir.join("a.txt").display());
        assert!(may_open(std::path::Path::new(&verbatim)), "{verbatim}");
        assert_eq!(local_form(&verbatim), dir.join("a.txt").to_str().unwrap());
        for p in ["\\\\?\\UNC\\h\\s", "\\\\?\\GLOBALROOT\\Device\\Mup\\h\\s", "\\\\?\\Volume{0}\\x", "\\\\.\\C:\\x", "\\??\\C:\\x", "//?/C:/x"] {
            assert_eq!(local_form(p), p);
            assert!(!may_open(Path::new(p)), "{p}");
        }
        assert_eq!(join_local(Path::new(&verbatim).parent().unwrap(), "b.txt"), dir.join("b.txt"));
        assert_eq!(join_local(Path::new("C:\\w"), &verbatim), dir.join("a.txt"));
        #[cfg(windows)]
        match std::os::windows::fs::symlink_dir("\\\\nohost.invalid\\share", dir.join("net")) {
            Ok(()) => {
                assert!(!may_open(&dir.join("net")));
                assert!(!may_open(&dir.join("net").join("x")));
            }
            Err(e) => eprintln!("MEASURED: no symbolic link to a network path made ({e}); git_state's link tests cover the rule"),
        }
        let _ = std::fs::remove_dir(dir.join("net"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn path_folders_are_local() {
        use std::cell::RefCell;
        use std::path::Path;
        let dir = std::env::temp_dir().join(format!("tandem-util-cli-path-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let net = dir.join("net");
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_dir("\\\\nohost.invalid\\share", &net).is_ok();
        #[cfg(not(windows))]
        let linked = false;
        if !linked {
            eprintln!("MEASURED: no symbolic link to a network path made; network_paths_are_never_opened covers the rule");
        }
        let path = format!("/\\h\\s;\\/h/s;\\\\?\\UNC\\h\\s;\\\\h\\s;//h/s; \"C:\\Q\" ;{};relative;.;C:rel;\\rooted;;\\\\?\\C:\\V", net.display());
        let mut want = vec![PathBuf::from("C:\\Q")];
        if !linked {
            want.push(net.clone());
        }
        want.push(PathBuf::from("C:\\V"));
        assert_eq!(path_dirs(&path).collect::<Vec<_>>(), want);

        // `command -V curl` asks about nothing outside those folders.
        let probed = RefCell::new(Vec::new());
        let none = |p: &Path| {
            probed.borrow_mut().push(p.to_path_buf());
            false
        };
        assert!(!found_on_path("curl", &path, ".COM;.EXE;", Path::new("C:\\w"), &none));
        let probed = probed.into_inner();
        assert!(probed.iter().all(|p| p.starts_with("C:\\Q") || p.starts_with("C:\\V") || (!linked && p.starts_with(&net))), "{probed:?}");
        for f in ["C:\\Q\\curl", "C:\\Q\\curl.COM", "C:\\Q\\curl.EXE", "C:\\V\\curl.EXE"] {
            assert!(probed.contains(&PathBuf::from(f)), "{f} in {probed:?}");
        }
        let exe = |p: &Path| p == Path::new("C:\\Q\\curl.EXE");
        assert!(found_on_path("curl", &path, ".COM;.EXE", Path::new("C:\\w"), &exe));

        // A name with a separator is the file itself, in the working folder when relative;
        // a network one is never asked about.
        let probed = RefCell::new(Vec::new());
        let at = |p: &Path| {
            probed.borrow_mut().push(p.to_path_buf());
            p == Path::new("C:\\w\\bin\\curl.exe")
        };
        assert!(found_on_path("bin\\curl.exe", "", ".EXE", Path::new("C:\\w"), &at));
        for p in ["\\\\h\\s\\curl.exe", "//h/s/curl.exe", "/\\h\\s\\curl.exe", "\\/h/s/curl.exe", "\\\\?\\UNC\\h\\s\\curl.exe"] {
            assert!(!found_on_path(p, "", ".EXE", Path::new("C:\\w"), &at), "{p}");
        }
        assert_eq!(probed.into_inner(), [PathBuf::from("C:\\w\\bin\\curl.exe")]);
        let _ = std::fs::remove_dir(&net);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn swallowed_reply_prefix_is_restored() {
        assert_eq!(restore_reply_prefix(b"TERM2 3.7.0n"), b"\x1b[ITERM2 3.7.0n");
        assert_eq!(restore_reply_prefix(b"TERM2 3.7.0n\x1b[0n"), b"\x1b[ITERM2 3.7.0n\x1b[0n");
        for kept in [&b"\x1b[ITERM2 3.7.0n"[..], b"\x1b[0n", b"TERM2", b"", b"ITERM2 3.7.0n"] {
            assert_eq!(restore_reply_prefix(kept), kept, "{:?}", String::from_utf8_lossy(kept));
        }
    }
}

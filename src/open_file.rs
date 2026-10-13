//! Opening files and folders with Tandem (m13v1 Task 2), after iTerm2's `openFile:`
//! (`iTermApplicationDelegate.m:785-913`, called AD below). Pure: what a path is, where it
//! opens, the text a script runs with, the batching of many opens, and Explorer's "Open with"
//! entries. Every path here came from another program (Explorer, the pipe) and is untrusted:
//! the one network rule (`scripts::is_network_path`, `stays_local`) comes before any file
//! system call, a name reaches a shell only through `pathnav::quote_for`, and nothing here
//! starts a process (P3).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// What a path is, by its extension (`AD:778-822`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Folder,
    ScriptArchive,
    ProfileTab,
    ColorPresets,
    Recording,
    Arrangement,
    Triggers,
    Script,
}

/// iTerm2's document types by extension, case-insensitive; a folder is a folder; everything else
/// is a script to run. `.itermarrangement` is not an iTerm2 type, so it is a script like any file.
pub fn classify(path: &Path, is_dir: bool) -> Kind {
    if is_dir {
        return Kind::Folder;
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "its" => Kind::ScriptArchive,
        "itermtab" => Kind::ProfileTab,
        "itermcolors" => Kind::ColorPresets,
        "itr" => Kind::Recording,
        "iterm2arrangement" => Kind::Arrangement,
        "it2triggers" => Kind::Triggers,
        _ => Kind::Script,
    }
}

/// Where the session opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Window,
    Tab,
    VerticalSplit,
    HorizontalSplit,
}

/// `NewInstanceOpenStyle` when 0 to 3 (0 window, 1 tab, 2 vertical split, 3 horizontal split),
/// else the legacy keys: a window, else a split (vertical when asked), else a tab (`AD:871-885`).
pub fn style(new_instance: i64, in_new_windows: bool, in_splits: bool, vertical: bool) -> Style {
    match new_instance {
        0 => Style::Window,
        1 => Style::Tab,
        2 => Style::VerticalSplit,
        3 => Style::HorizontalSplit,
        _ if in_new_windows => Style::Window,
        _ if in_splits => if vertical { Style::VerticalSplit } else { Style::HorizontalSplit },
        _ => Style::Tab,
    }
}

/// `terminalToOpenFileIn` is nil (a new window) when `OpenFileInNewWindows` (`AD:3768-3774`).
pub fn target_is_new_window(in_new_windows: bool) -> bool {
    in_new_windows
}

/// The shell family a profile's command line starts, for quoting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shell {
    PowerShell,
    Cmd,
    Posix,
}

/// The program of a command line: a quoted path whole; else the text up to the first `.exe`
/// (a path may hold spaces); else the first word.
fn program_of(command_line: &str) -> String {
    let c = command_line.trim_start();
    if let Some(rest) = c.strip_prefix('"') {
        return rest.split('"').next().unwrap_or_default().to_string();
    }
    let lower = c.to_ascii_lowercase();
    match lower.find(".exe") {
        Some(at) => c[..at + 4].to_string(),
        None => c.split_whitespace().next().unwrap_or_default().to_string(),
    }
}

/// The program's file stem, case-insensitive: `pwsh` and `powershell` are PowerShell, `cmd` is
/// cmd, anything else a POSIX shell. (13p's `shell_inject::shell_of` answers another question.)
pub fn shell_of(command_line: &str) -> Shell {
    let program = program_of(command_line).to_ascii_lowercase();
    let base = program.rsplit(['\\', '/']).next().unwrap_or_default();
    let stem = base.strip_suffix(".exe").unwrap_or(base);
    match stem {
        "pwsh" | "powershell" => Shell::PowerShell,
        "cmd" => Shell::Cmd,
        _ => Shell::Posix,
    }
}

fn nav_shell(shell: Shell) -> crate::pathnav::Shell {
    match shell {
        Shell::PowerShell => crate::pathnav::Shell::PowerShell,
        Shell::Cmd => crate::pathnav::Shell::Cmd,
        Shell::Posix => crate::pathnav::Shell::Posix,
    }
}

/// What a refused name says (B5), naming it through `shown_name`.
pub fn refused_banner(shown: &str) -> String {
    format!("Tandem will not run \u{201c}{shown}\u{201d}: its name holds characters a shell reads as commands.")
}

/// The path as the run text writes it: `pathnav::quote_for`, the one refusal for every shell (a
/// quote look-alike, `%`, `$`, `;`, a control or bidi character, a trailing backslash, a network
/// path). Err is the banner text of `quote_for`.
pub fn quoted_path(path: &Path, shell: Shell) -> Result<String, String> {
    let text = path.to_str().ok_or_else(|| crate::pathnav::REFUSED.to_string())?;
    crate::pathnav::quote_for(text, nav_shell(shell))
}

/// What the session types to run the file and end (`AD:843-848`). Built only from a `quote_for` Ok.
pub fn run_text(path: &Path, shell: Shell) -> Result<String, String> {
    let q = quoted_path(path, shell)?;
    Ok(match shell {
        Shell::PowerShell => format!("& {q}; exit"),
        Shell::Cmd => format!("{q} & exit"),
        Shell::Posix => format!("{q}; exit"),
    })
}

/// The profile's text then the run text on the next line, unless the profile's text is replaced
/// (`OpenFileOverridesSendText`). The separator is `\r`, 13p's Enter (Decision), not iTerm2's `\n`.
pub fn initial_text(profile_text: &str, run: &str, overrides: bool) -> String {
    if !profile_text.is_empty() && !overrides {
        format!("{profile_text}\r{run}")
    } else {
        run.to_string()
    }
}

/// Every `\` doubled: Initial Text is an interpolated string (`AD:860-861`), applied to the whole
/// combined text, the profile's own part included, as iTerm2 does (a faithful quirk).
pub fn escape_for_template(text: &str) -> String {
    text.replace('\\', "\\\\")
}

/// The value stored as the session copy's "Initial Text".
pub fn session_initial_text(profile_text: &str, run: &str, overrides: bool) -> String {
    escape_for_template(&initial_text(profile_text, run, overrides))
}

/// "OK to run \u{201c}{quoted}\u{201d}?" over `quoted_path`'s form (`AD:841`, `:851`).
pub fn confirm_run_title(quoted: &str) -> String {
    format!("OK to run \u{201c}{quoted}\u{201d}?")
}

pub const CONFIRM_RUN_ID: &str = "NoSyncConfirmRunOpenFile";
pub const COLORS_IMPORTED_HEADING: &str = "Colors Scheme Imported";
pub const COLORS_IMPORTED_TEXT: &str = "The color scheme was imported and added to presets. You can find it under Settings > Profiles > Colors > Load Presets\u{2026}.";
/// The first line of the question for a file that came from outside (B6).
pub const OUTSIDE_NOTE: &str = "This file came from outside this computer or the network.";
/// What a network path (or one reached through a link to one) says; nothing was probed.
pub const NETWORK_BANNER: &str = "Tandem does not open files on network locations.";
/// A file larger than this never gets the silenceable question (B6).
pub const MAX_SILENCED_LEN: u64 = 1 << 20;
/// A data file (arrangement, recording, archive, colors) larger than this is refused unread.
pub const MAX_DATA_LEN: u64 = 64 << 20;

/// What opening a path does after the question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub guid: String,
    pub dir: Option<PathBuf>,
    pub text: Option<String>,
    pub style: Style,
    pub new_window: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct OpenKeys {
    pub overrides_send_text: bool,
    pub new_instance_style: i64,
    pub in_new_windows: bool,
    pub in_splits: bool,
    pub vertical: bool,
}

/// A folder opens a session there; a script's session types its run text. Err is the refusal
/// banner (the run text could not be built from a safe name): nothing opens.
pub fn plan(path: &Path, is_dir: bool, profile_guid: &str, profile_text: &str, command_line: &str, keys: OpenKeys) -> Result<Launch, String> {
    let (dir, text) = if is_dir {
        (Some(path.to_path_buf()), None)
    } else {
        let run = run_text(path, shell_of(command_line))?;
        (None, Some(session_initial_text(profile_text, &run, keys.overrides_send_text)))
    };
    Ok(Launch {
        guid: profile_guid.to_string(),
        dir,
        text,
        style: style(keys.new_instance_style, keys.in_new_windows, keys.in_splits, keys.vertical),
        new_window: target_is_new_window(keys.in_new_windows),
    })
}

/// What a probe of a path found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stat {
    pub is_dir: bool,
    pub is_file: bool,
    pub len: u64,
}

/// The file system questions about an outside path, behind a seam so a test counts them.
pub trait FileProbe {
    /// `scripts::stays_local`: not a link chain reaching a network path.
    fn stays_local(&self, path: &Path) -> bool;
    fn stat(&self, path: &Path) -> Option<Stat>;
    /// Mark of the Web (`arrangement::has_mark_of_the_web`).
    fn marked(&self, path: &Path) -> bool;
}

pub struct RealProbe;

impl FileProbe for RealProbe {
    fn stays_local(&self, path: &Path) -> bool {
        crate::scripts::stays_local(path)
    }
    fn stat(&self, path: &Path) -> Option<Stat> {
        let m = crate::localfs::stat(&crate::localfs::LocalPath::judge(path)?).ok()?;
        Some(Stat { is_dir: m.is_dir, is_file: m.is_file, len: m.len })
    }
    fn marked(&self, path: &Path) -> bool {
        crate::arrangement::has_mark_of_the_web(path)
    }
}

/// What the worker learned about an opened path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Facts {
    pub path: PathBuf,
    pub kind: Kind,
    pub is_dir: bool,
    /// A regular file (not a device or a pipe).
    pub regular: bool,
    pub len: u64,
    pub marked: bool,
}

/// The checks on an outside path, in order: the network rule on the text alone (no call), the link
/// check, the stat, the Mark of the Web. Runs on a worker (a mapped drive may be slow).
pub fn inspect(path: &Path, fs: &dyn FileProbe) -> Result<Facts, String> {
    if crate::scripts::is_network_path(&path.to_string_lossy()) || !fs.stays_local(path) {
        return Err(NETWORK_BANNER.to_string());
    }
    let Some(stat) = fs.stat(path) else {
        return Err(format!("Tandem could not open \u{201c}{}\u{201d}: it does not exist.", crate::inert_files::shown_name(&path.to_string_lossy())));
    };
    let kind = classify(path, stat.is_dir);
    let marked = !stat.is_dir && fs.marked(path);
    Ok(Facts { path: path.to_path_buf(), kind, is_dir: stat.is_dir, regular: stat.is_file, len: stat.len, marked })
}

/// Whether a silenced "OK to run" may cover the file (B6): a local regular file under 1 MiB with
/// no Mark of the Web. (The name passes `quote_for` or no question is asked at all.)
pub fn silenceable(f: &Facts) -> bool {
    !f.marked && f.regular && f.len <= MAX_SILENCED_LEN
}

/// The id, kind and text of the "OK to run" question: permanently silenceable only for an
/// eligible file; any other asks every time, saying why.
pub fn run_question(f: &Facts, title: &str) -> (Option<&'static str>, crate::warnings::Kind, String) {
    if silenceable(f) {
        (Some(CONFIRM_RUN_ID), crate::warnings::Kind::Permanent, title.to_string())
    } else {
        (None, crate::warnings::Kind::Persistent, format!("{OUTSIDE_NOTE}\n{title}"))
    }
}

/// Explorer opens many files at once as many processes. They gather for this long and open
/// together; none is dropped silently.
pub const BATCH_WINDOW: Duration = Duration::from_millis(400);
/// At most this many files open per `OPEN_SPAN` (the same bound as one message, `launch::MAX_PATHS`).
pub const MAX_OPEN: usize = crate::launch::MAX_PATHS;
pub const OPEN_SPAN: Duration = Duration::from_secs(10);

/// The one banner for files past the bound.
pub fn ignored_banner(n: usize) -> String {
    format!("{n} file{} ignored", if n == 1 { "" } else { "s" })
}

/// The files ignored lately, from every place that drops one (the batch, the workers, the alert
/// queue), so the one banner adds up. A count older than `OPEN_SPAN` starts again.
#[derive(Debug, Default)]
pub struct IgnoredCount {
    n: usize,
    at: Option<Instant>,
}

impl IgnoredCount {
    /// Adds `k` ignored files seen at `now`; the total for the banner.
    pub fn add(&mut self, now: Instant, k: usize) -> usize {
        if self.at.is_none_or(|t| now.saturating_duration_since(t) > OPEN_SPAN) {
            self.n = 0;
        }
        self.n = self.n.saturating_add(k);
        self.at = Some(now);
        self.n
    }
}

/// Paths waiting for their batch window, and the budget of files opened lately.
#[derive(Debug, Default)]
pub struct Batch {
    pending: Vec<PathBuf>,
    due: Option<Instant>,
    ignored: usize,
    started: VecDeque<(Instant, usize)>,
}

impl Batch {
    /// Adds `paths`; the window starts at the first. More than `MAX_OPEN` waiting are counted.
    pub fn push(&mut self, paths: Vec<PathBuf>, now: Instant) {
        for p in paths {
            if self.pending.len() >= MAX_OPEN {
                self.ignored += 1;
            } else {
                self.pending.push(p);
            }
        }
        if self.due.is_none() && (!self.pending.is_empty() || self.ignored > 0) {
            self.due = Some(now + BATCH_WINDOW);
        }
    }

    pub fn due(&self) -> Option<Instant> {
        self.due
    }

    /// Paths gathered and not yet opened (an ignored-only batch still counts once, for its banner).
    pub fn waiting(&self) -> usize {
        self.pending.len().max(usize::from(self.due.is_some()))
    }

    /// Every gathered path, taken out unopened (to hand to the Tandem that owns the pipe).
    pub fn take_all(&mut self) -> Vec<PathBuf> {
        self.due = None;
        std::mem::take(&mut self.pending)
    }

    /// The batch once its window ended: the paths within the budget of this ten seconds, and how
    /// many were ignored (the bound, or the budget).
    pub fn take(&mut self, now: Instant) -> Option<(Vec<PathBuf>, usize)> {
        if self.due.is_none_or(|d| now < d) {
            return None;
        }
        self.due = None;
        while self.started.front().is_some_and(|(t, _)| now.saturating_duration_since(*t) >= OPEN_SPAN) {
            self.started.pop_front();
        }
        let used: usize = self.started.iter().map(|(_, n)| n).sum();
        let room = MAX_OPEN.saturating_sub(used);
        let mut paths = std::mem::take(&mut self.pending);
        let over = paths.len().saturating_sub(room);
        paths.truncate(room);
        if !paths.is_empty() {
            self.started.push_back((now, paths.len()));
        }
        let ignored = std::mem::take(&mut self.ignored) + over;
        Some((paths, ignored))
    }
}

/// The file types Explorer's "Open with" lists Tandem for (P10): iTerm2's document types and
/// scripts, never `.bat`, `.cmd`, `.exe` and the like.
pub const SUPPORTED_TYPES: [&str; 14] = [".command", ".tool", ".sh", ".zsh", ".csh", ".pl", ".bash", ".ps1", ".its", ".itermtab", ".itermcolors", ".itr", ".iterm2arrangement", ".it2triggers"];

pub const APPLICATION_KEY: &str = "Software\\Classes\\Applications\\tandem.exe";

/// The registry entries (key, value name, value) that make Tandem an "Open with" choice for one
/// user. Never a default association.
pub fn open_with_entries(exe: &Path) -> Vec<(String, String, String)> {
    let mut v = vec![
        (APPLICATION_KEY.to_string(), "FriendlyAppName".to_string(), "Tandem".to_string()),
        (format!("{APPLICATION_KEY}\\shell\\open\\command"), String::new(), format!("\"{}\" \"%1\"", exe.display())),
    ];
    for ext in SUPPORTED_TYPES {
        v.push((format!("{APPLICATION_KEY}\\SupportedTypes"), ext.to_string(), String::new()));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    #[test]
    fn open_kinds_follow_iterm2() {
        let k = |p: &str| classify(Path::new(p), false);
        assert_eq!(k("x.its"), Kind::ScriptArchive);
        assert_eq!(k("X.ITERMTAB"), Kind::ProfileTab);
        assert_eq!(k("a.itermcolors"), Kind::ColorPresets);
        assert_eq!(k("r.itr"), Kind::Recording);
        assert_eq!(k("w.iterm2arrangement"), Kind::Arrangement);
        assert_eq!(k("w.itermarrangement"), Kind::Script, "not an iTerm2 type (R13vw-5)");
        assert_eq!(k("t.it2triggers"), Kind::Triggers);
        for p in ["s.ps1", "s.sh", "noext"] {
            assert_eq!(k(p), Kind::Script, "{p}");
        }
        assert_eq!(classify(Path::new("x.its"), true), Kind::Folder);
        assert_eq!(classify(Path::new("anything"), true), Kind::Folder);
        let ps = quoted_path(Path::new("C:\\a b\\s.ps1"), Shell::PowerShell).unwrap();
        assert_eq!(confirm_run_title(&ps), "OK to run \u{201c}'C:\\a b\\s.ps1'\u{201d}?");
        let posix = quoted_path(Path::new("/mnt/c/a b/s.sh"), Shell::Posix).unwrap();
        assert_eq!(confirm_run_title(&posix), "OK to run \u{201c}/mnt/c/a\\ b/s.sh\u{201d}?");
        assert_eq!(CONFIRM_RUN_ID, "NoSyncConfirmRunOpenFile");
        assert_eq!(COLORS_IMPORTED_HEADING, "Colors Scheme Imported");
        assert_eq!(COLORS_IMPORTED_TEXT, "The color scheme was imported and added to presets. You can find it under Settings > Profiles > Colors > Load Presets\u{2026}.");
    }

    #[test]
    fn open_style_follows_iterm2() {
        assert_eq!(style(-1, false, false, true), Style::Tab);
        assert_eq!(style(-1, true, true, true), Style::Window);
        assert_eq!(style(-1, false, true, true), Style::VerticalSplit);
        assert_eq!(style(-1, false, true, false), Style::HorizontalSplit);
        assert_eq!(style(0, false, true, true), Style::Window);
        assert_eq!(style(1, true, true, true), Style::Tab);
        assert_eq!(style(2, false, false, false), Style::VerticalSplit);
        assert_eq!(style(3, true, false, true), Style::HorizontalSplit);
        assert_eq!(style(4, false, false, true), Style::Tab, "out of 0..=3 follows the legacy keys");
        assert_eq!(style(-7, true, false, true), Style::Window);
        assert!(target_is_new_window(true));
        assert!(!target_is_new_window(false));
    }

    #[test]
    fn run_text_follows_the_shell() {
        assert_eq!(shell_of("pwsh.exe -NoLogo"), Shell::PowerShell);
        assert_eq!(shell_of("C:\\Program Files\\PowerShell\\7\\PWSH.EXE"), Shell::PowerShell);
        assert_eq!(shell_of("\"C:\\Program Files\\PowerShell\\7\\pwsh.exe\" -NoLogo"), Shell::PowerShell);
        assert_eq!(shell_of("powershell"), Shell::PowerShell);
        assert_eq!(shell_of("cmd.exe /k"), Shell::Cmd);
        assert_eq!(shell_of("wsl.exe"), Shell::Posix);
        assert_eq!(shell_of("C:\\msys64\\usr\\bin\\bash.exe --login"), Shell::Posix);
        assert_eq!(run_text(Path::new("C:\\a b\\s.ps1"), Shell::PowerShell).unwrap(), "& 'C:\\a b\\s.ps1'; exit");
        assert_eq!(run_text(Path::new("C:\\a b\\s.bat"), Shell::Cmd).unwrap(), "\"C:\\a b\\s.bat\" & exit");
        assert_eq!(run_text(Path::new("/mnt/c/a b/s.sh"), Shell::Posix).unwrap(), "/mnt/c/a\\ b/s.sh; exit");
    }

    /// B5: a name any supported shell reads as more than a name is refused for every shell, so a
    /// quote look-alike, a `%`, a `$` or a `;` never reaches the command line.
    #[test]
    fn run_text_refuses_curly_quotes() {
        for bad in [
            "C:\\a\\\u{2019}; calc; \u{2018}b.ps1",
            "C:\\it's\\s.ps1",
            "C:\\a\\%PATH%\\s.bat",
            "C:\\a\\s;calc.ps1",
            "C:\\a\\$x.ps1",
            "C:\\a\\`x.ps1",
            "C:\\a\\x&y.ps1",
            "C:\\a (x86)\\s.ps1",
            "C:\\a\\s\n.ps1",
            "C:\\a\\\u{202e}s.ps1",
            "C:\\a\\",
        ] {
            for shell in [Shell::PowerShell, Shell::Cmd, Shell::Posix] {
                assert!(quoted_path(Path::new(bad), shell).is_err(), "{bad:?} {shell:?}");
                assert!(run_text(Path::new(bad), shell).is_err(), "{bad:?} {shell:?}");
            }
        }
    }

    #[test]
    fn confirm_title_is_safe_text() {
        assert!(quoted_path(Path::new("C:\\a\\\u{202e}gnp.ps1"), Shell::PowerShell).is_err());
        let launch = plan(Path::new("C:\\a\\\u{202e}s.ps1"), false, "G", "", "pwsh", OpenKeys { overrides_send_text: true, new_instance_style: -1, in_new_windows: false, in_splits: false, vertical: true });
        assert!(launch.is_err(), "a refusal opens nothing");
        assert!(refused_banner("x.ps1").starts_with("Tandem will not run \u{201c}x.ps1\u{201d}"));
    }

    #[test]
    fn initial_text_follows_iterm2() {
        assert_eq!(initial_text("echo hi", "RUN", false), "echo hi\rRUN");
        assert_eq!(initial_text("echo hi", "RUN", true), "RUN");
        assert_eq!(initial_text("", "RUN", false), "RUN");
        assert_eq!(escape_for_template("a\\b"), "a\\\\b");
        // A run text with a backslash and an open parenthesis would open an interpolation if it
        // were not escaped: the evaluated text is exactly the run text.
        let r = "& 'C:\\x\\(y)\\s.ps1'; exit".to_string();
        assert_eq!(crate::expr::template_pieces(&session_initial_text("", &r, true)), vec![(false, r.clone())]);
        let real = run_text(Path::new("C:\\x\\y\\s.ps1"), Shell::PowerShell).unwrap();
        assert_eq!(crate::expr::template_pieces(&session_initial_text("", &real, true)), vec![(false, real.clone())]);
        assert_eq!(session_initial_text("cd \\tmp", "RUN", false), "cd \\\\tmp\rRUN");
        let keys = OpenKeys { overrides_send_text: true, new_instance_style: -1, in_new_windows: true, in_splits: false, vertical: true };
        let folder = plan(Path::new("C:\\work"), true, "G", "echo hi", "pwsh", keys).unwrap();
        assert_eq!((folder.dir.as_deref(), folder.text.as_deref(), folder.guid.as_str()), (Some(Path::new("C:\\work")), None, "G"));
        assert!(folder.new_window, "follows target_is_new_window");
        let script = plan(Path::new("C:\\w\\s.bat"), false, "G", "echo hi", "cmd.exe", keys).unwrap();
        assert_eq!((script.dir.clone(), script.text.as_deref()), (None, Some("\"C:\\\\w\\\\s.bat\" & exit")));
        let both = plan(Path::new("C:\\w\\s.ps1"), false, "G", "echo hi", "pwsh", OpenKeys { overrides_send_text: false, ..keys }).unwrap();
        assert_eq!(both.text.as_deref(), Some("echo hi\r& 'C:\\\\w\\\\s.ps1'; exit"));
        assert!(!plan(Path::new("C:\\w\\s.ps1"), false, "G", "", "pwsh", OpenKeys { in_new_windows: false, ..keys }).unwrap().new_window);
    }

    /// B5: one network rule, asked before any probe.
    struct Counting {
        calls: Cell<usize>,
        local: bool,
    }
    impl FileProbe for Counting {
        fn stays_local(&self, _: &Path) -> bool {
            self.calls.set(self.calls.get() + 1);
            self.local
        }
        fn stat(&self, _: &Path) -> Option<Stat> {
            self.calls.set(self.calls.get() + 1);
            Some(Stat { is_dir: false, is_file: true, len: 10 })
        }
        fn marked(&self, _: &Path) -> bool {
            self.calls.set(self.calls.get() + 1);
            false
        }
    }

    #[test]
    fn open_refuses_a_network_path_before_any_probe() {
        let probe = Counting { calls: Cell::new(0), local: true };
        for p in ["\\\\h\\s\\a.ps1", "//h/s/a.ps1", "\\\\?\\UNC\\h\\s\\a.ps1", "\\\\.\\pipe\\x", "\\??\\UNC\\h\\s", "\\/h/s/a.ps1"] {
            assert_eq!(inspect(Path::new(p), &probe).unwrap_err(), NETWORK_BANNER, "{p}");
        }
        assert_eq!(probe.calls.get(), 0, "no file system call for a network path");
        let mapped = Counting { calls: Cell::new(0), local: false };
        assert_eq!(inspect(Path::new("Z:\\a.ps1"), &mapped).unwrap_err(), NETWORK_BANNER);
        assert_eq!(mapped.calls.get(), 1, "only the link check ran, never the stat");
        let ok = Counting { calls: Cell::new(0), local: true };
        let facts = inspect(Path::new("C:\\a\\s.ps1"), &ok).unwrap();
        assert_eq!((facts.kind.clone(), facts.regular, facts.marked), (Kind::Script, true, false));
    }

    #[test]
    fn silenced_ok_does_not_cover_a_marked_file() {
        let base = Facts { path: PathBuf::from("C:\\a\\s.ps1"), kind: Kind::Script, is_dir: false, regular: true, len: 100, marked: false };
        assert!(silenceable(&base));
        assert!(!silenceable(&Facts { marked: true, ..base.clone() }), "a downloaded file always asks");
        assert!(!silenceable(&Facts { len: MAX_SILENCED_LEN + 1, ..base.clone() }));
        assert!(!silenceable(&Facts { regular: false, ..base.clone() }));
        let (id, kind, text) = run_question(&base, "OK to run x?");
        assert_eq!((id, kind), (Some(CONFIRM_RUN_ID), crate::warnings::Kind::Permanent));
        assert_eq!(text, "OK to run x?");
        let (id, kind, text) = run_question(&Facts { marked: true, ..base }, "OK to run x?");
        assert_eq!((id, kind), (None, crate::warnings::Kind::Persistent));
        assert!(text.starts_with(OUTSIDE_NOTE) && text.ends_with("OK to run x?"));
    }

    /// Explorer opens many files as many processes: they are batched in a short window and none
    /// is silently dropped; past the bound one count says how many were ignored.
    #[test]
    fn explorer_opens_are_batched_not_dropped() {
        let t0 = Instant::now();
        let mut b = Batch::default();
        assert_eq!(b.due(), None);
        for i in 0..10u64 {
            b.push(vec![PathBuf::from(format!("C:\\f{i}.ps1"))], t0 + Duration::from_millis(i * 30));
        }
        assert_eq!(b.due(), Some(t0 + BATCH_WINDOW), "the window starts at the first");
        assert!(b.take(t0 + Duration::from_millis(300)).is_none(), "not before the window ends");
        let (paths, ignored) = b.take(t0 + BATCH_WINDOW).unwrap();
        assert_eq!((paths.len(), ignored), (10, 0), "ten in one batch, in order");
        assert_eq!(paths[0], PathBuf::from("C:\\f0.ps1"));
        assert_eq!(paths[9], PathBuf::from("C:\\f9.ps1"));
        assert_eq!(b.due(), None);
        // The budget is per ten seconds: 6 more fit, the rest is counted.
        let t1 = t0 + Duration::from_secs(1);
        for i in 0..9u64 {
            b.push(vec![PathBuf::from(format!("C:\\g{i}.ps1"))], t1);
        }
        let (paths, ignored) = b.take(t1 + BATCH_WINDOW).unwrap();
        assert_eq!((paths.len(), ignored), (MAX_OPEN - 10, 3), "past the bound: counted, never silent");
        assert_eq!(ignored_banner(3), "3 files ignored");
        assert_eq!(ignored_banner(1), "1 file ignored");
        // A later window starts afresh.
        let t2 = t0 + Duration::from_secs(30);
        b.push(vec![PathBuf::from("C:\\h.ps1")], t2);
        assert_eq!(b.take(t2 + BATCH_WINDOW).unwrap(), (vec![PathBuf::from("C:\\h.ps1")], 0));
    }

    #[test]
    fn a_flood_of_open_requests_is_capped() {
        let t0 = Instant::now();
        let mut b = Batch::default();
        for _ in 0..1000 {
            b.push(vec![PathBuf::from("C:\\x.ps1")], t0);
        }
        let (paths, ignored) = b.take(t0 + BATCH_WINDOW).unwrap();
        assert_eq!(paths.len(), MAX_OPEN);
        assert_eq!(ignored, 1000 - MAX_OPEN);
        let mut b = Batch::default();
        b.push((0..40).map(|i| PathBuf::from(format!("C:\\{i}"))).collect(), t0);
        let (paths, ignored) = b.take(t0 + BATCH_WINDOW).unwrap();
        assert_eq!((paths.len(), ignored), (MAX_OPEN, 40 - MAX_OPEN));
    }

    #[test]
    fn open_with_entries_name_only_tandem() {
        let entries = open_with_entries(Path::new("C:\\Tandem\\termurion.exe"));
        assert!(entries.iter().all(|(k, _, _)| k.starts_with("Software\\Classes\\Applications\\tandem.exe")), "{entries:?}");
        let get = |k: &str, n: &str| entries.iter().find(|(kk, nn, _)| kk.ends_with(k) && nn == n).map(|(_, _, v)| v.as_str());
        assert_eq!(get("shell\\open\\command", ""), Some("\"C:\\Tandem\\termurion.exe\" \"%1\""));
        for ext in [".its", ".itermtab", ".itermcolors", ".itr", ".iterm2arrangement", ".it2triggers", ".command", ".tool", ".sh", ".zsh", ".csh", ".pl", ".ps1", ".bash"] {
            assert_eq!(get("SupportedTypes", ext), Some(""), "{ext}");
        }
        for ext in [".bat", ".cmd", ".exe", ".lnk", ".msi", ".vbs", ".js"] {
            assert_eq!(get("SupportedTypes", ext), None, "{ext} is never offered (P10)");
        }
        assert!(!entries.iter().any(|(k, _, _)| k.to_ascii_lowercase().contains("fileexts") || k.to_ascii_lowercase().contains("userchoice")), "never a default association");
    }

    /// P3: no request kind runs a command line, a shell string or an environment from here.
    #[test]
    fn no_open_request_runs_a_command() {
        let src = include_str!("open_file.rs").split("#[cfg(test)]").next().unwrap();
        for banned in ["Command::new", "ShellExecute", "CreateProcess", "cmd /c", "std::process"] {
            assert!(!src.contains(banned), "{banned}");
        }
    }

    /// Review fix 2: one count for every way a file is ignored (the batch, the workers, the alert
    /// queue), kept for `OPEN_SPAN` after the last one so the one banner adds up.
    #[test]
    fn ignored_files_add_up_in_one_banner() {
        let t0 = std::time::Instant::now();
        let mut c = IgnoredCount::default();
        assert_eq!(c.add(t0, 2), 2);
        assert_eq!(c.add(t0 + Duration::from_secs(5), 1), 3);
        assert_eq!(c.add(t0 + Duration::from_secs(5) + OPEN_SPAN + Duration::from_millis(1), 1), 1, "a new burst starts again");
    }
}

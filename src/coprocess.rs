//! Coprocesses: the per-pane state and rules (one at a time, the MRU, the stderr cap, the
//! output queue). `coproc_win` runs the process.


use std::collections::VecDeque;

/// Commands kept in `Coprocess MRU`.
pub const MRU_MAX: usize = 20;
/// Characters of stderr kept for the "terminated with output on stderr" banner.
pub const STDERR_MAX: usize = 100_000;
pub const TRUNCATED: &str = "-- output truncated --";
/// Bytes of coprocess output that may wait for the shell to read (iTerm2 reads more from
/// the coprocess only while the pty's write buffer is under 10 KB).
pub const WRITE_LIMIT: usize = 10 * 1024;
/// Bytes of terminal output that may wait for a coprocess that does not read.
pub const INPUT_MAX: usize = 4 * 1024 * 1024;
/// The globals keys.
pub const MRU_KEY: &str = "Coprocess MRU";
pub const IGNORE_ERRORS_KEY: &str = "NoSyncCoprocessCommandsToIgnoreErrorOutput";
pub const SUPPRESS_TRIGGER_WARNING_KEY: &str = "NoSyncSuppressCoprocessTriggerWarning";

/// Adds a command to the MRU: newest first, once, at most 20.
pub fn mru_add(mru: &mut Vec<String>, command: &str) {
    let c = command.trim();
    if c.is_empty() {
        return;
    }
    mru.retain(|m| m != c);
    mru.insert(0, c.to_string());
    mru.truncate(MRU_MAX);
}

/// A coprocess's stderr, kept up to the cap.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stderr {
    text: String,
    chars: usize,
    full: bool,
}

impl Stderr {
    pub fn push(&mut self, s: &str) {
        if self.full {
            return;
        }
        let room = STDERR_MAX - self.chars;
        let n = s.chars().count();
        if n <= room {
            self.text.push_str(s);
            self.chars += n;
        } else {
            self.text.extend(s.chars().take(room));
            self.text.push('\n');
            self.text.push_str(TRUNCATED);
            self.full = true;
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }
}

/// How many bytes of coprocess output may go to the shell now, with `pending` bytes still
/// waiting in the pty's writer.
pub fn budget(pending: usize) -> usize {
    WRITE_LIMIT.saturating_sub(pending)
}

/// Coprocess output waiting to go to the shell, in order.
#[derive(Debug, Default)]
pub struct OutQueue {
    chunks: VecDeque<Vec<u8>>,
}

impl OutQueue {
    pub fn push(&mut self, chunk: Vec<u8>) {
        if !chunk.is_empty() {
            self.chunks.push_back(chunk);
        }
    }

    /// Up to `budget` bytes; a chunk that does not fit is split.
    pub fn take(&mut self, budget: usize) -> Vec<u8> {
        let mut out = Vec::new();
        while out.len() < budget {
            let Some(mut c) = self.chunks.pop_front() else { break };
            let room = budget - out.len();
            if c.len() > room {
                let rest = c.split_off(room);
                self.chunks.push_front(rest);
            }
            out.extend_from_slice(&c);
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }
}

/// Terminal output on its way to a coprocess: past the cap, output is dropped and counted.
#[derive(Debug, Default)]
pub struct Backlog {
    pending: usize,
    pub dropped: usize,
}

impl Backlog {
    /// Whether `n` more bytes may be queued.
    pub fn admit(&mut self, n: usize) -> bool {
        if self.pending + n > INPUT_MAX {
            self.dropped += n;
            return false;
        }
        self.pending += n;
        true
    }

    /// The coprocess took `n` bytes.
    pub fn sent(&mut self, n: usize) {
        self.pending = self.pending.saturating_sub(n);
    }
}

/// Where a start comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    Menu,
    Trigger,
    KeyBinding,
    ContextMenu,
    SmartSelection,
    SemanticHistory,
    FileDrop,
    /// A script's `run_coprocess` method.
    Api,
}

/// Why a start is refused, and how the refusal shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Nothing shows (the menu item is disabled; a file drop is ignored).
    Quiet,
    /// The trigger's announcement with Silence Warning.
    TriggerWarning,
    /// A banner (iTerm2 replaces the running one here and orphans it; Tandem refuses).
    Banner,
}

/// One coprocess per pane on every path.
pub fn start_policy(running: bool, entry: Entry) -> Result<(), Refusal> {
    if !running {
        return Ok(());
    }
    Err(match entry {
        Entry::Menu | Entry::FileDrop => Refusal::Quiet,
        Entry::Trigger => Refusal::TriggerWarning,
        Entry::KeyBinding | Entry::ContextMenu | Entry::SmartSelection | Entry::SemanticHistory | Entry::Api => Refusal::Banner,
    })
}

/// The trigger's refusal text, from the action's title.
pub fn trigger_warning(action_title: &str) -> String {
    format!("{} trigger: Can't run two coprocesses at once.", action_title.trim_end_matches('\u{2026}').trim_end_matches("..."))
}

/// `fileDropCoprocess`: the command for dropped files, `\\(filenames)` filled with the paths,
/// each a value for the one cmd.exe builder (`semantic::build_cmd_line`: quoted, or bare inside
/// the template's own quotes). None when the setting is empty; Err (`CMD_REFUSED`) when a
/// dropped name holds what cmd.exe would read (a downloaded `a&calc&.txt`, `%`, parentheses,
/// a drive root's trailing backslash): nothing runs.
pub fn drop_command(template: &str, files: &[String]) -> Option<Result<String, String>> {
    use crate::semantic::CmdPiece;
    let t = template.trim();
    if t.is_empty() {
        return None;
    }
    let mut pieces = Vec::new();
    for (i, text) in t.split("\\(filenames)").enumerate() {
        if i > 0 {
            for (n, f) in files.iter().enumerate() {
                if n > 0 {
                    pieces.push(CmdPiece::Text(" ".into()));
                }
                pieces.push(CmdPiece::Value(f.clone()));
            }
        }
        pieces.push(CmdPiece::Text(text.to_string()));
    }
    Some(crate::semantic::build_cmd_line(&pieces).ok_or_else(|| crate::semantic::CMD_REFUSED.to_string()))
}

/// The banner when a coprocess ends with stderr output.
pub fn stderr_banner(command: &str) -> String {
    format!("Coprocess \u{201c}{command}\u{201d} terminated with output on stderr.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mru_keeps_twenty() {
        let mut m = Vec::new();
        for i in 0..25 {
            mru_add(&mut m, &format!("c{i}"));
        }
        assert_eq!(m.len(), MRU_MAX);
        assert_eq!(m[0], "c24");
        mru_add(&mut m, "c10");
        assert_eq!((m[0].as_str(), m.iter().filter(|c| *c == "c10").count()), ("c10", 1), "newest first, once");
        mru_add(&mut m, "   ");
        assert_eq!(m[0], "c10", "blank commands are not kept");
    }

    #[test]
    fn stderr_is_capped() {
        let mut e = Stderr::default();
        e.push(&"x".repeat(STDERR_MAX - 2));
        e.push("abcd");
        assert_eq!(e.text().chars().count(), STDERR_MAX + TRUNCATED.len() + 1);
        assert!(e.text().ends_with(TRUNCATED));
        e.push("more");
        assert!(e.text().ends_with(TRUNCATED), "nothing after the mark");
        assert!(Stderr::default().text().is_empty());
    }

    #[test]
    fn coprocess_output_is_bounded() {
        let mut q = OutQueue::default();
        q.push(vec![b'a'; 30_000]);
        q.push(vec![b'b'; 10]);
        assert_eq!(q.take(budget(0)).len(), WRITE_LIMIT, "at most 10 KB waits for the shell");
        assert_eq!(q.take(budget(WRITE_LIMIT)).len(), 0, "the shell has not read yet");
        let rest = q.take(usize::MAX);
        assert_eq!(rest.len(), 30_000 - WRITE_LIMIT + 10);
        assert_eq!(rest.last(), Some(&b'b'), "in order");
        assert!(q.is_empty());
    }

    #[test]
    fn input_backlog_is_capped() {
        let mut b = Backlog::default();
        assert!(b.admit(INPUT_MAX - 1));
        assert!(!b.admit(10), "a coprocess that does not read loses input past the cap");
        assert_eq!(b.dropped, 10);
        b.sent(INPUT_MAX - 1);
        assert!(b.admit(10));
    }

    #[test]
    fn one_coprocess_per_pane() {
        assert_eq!(start_policy(false, Entry::Menu), Ok(()));
        assert_eq!(start_policy(true, Entry::Menu), Err(Refusal::Quiet));
        assert_eq!(start_policy(true, Entry::Trigger), Err(Refusal::TriggerWarning));
        assert_eq!(start_policy(true, Entry::KeyBinding), Err(Refusal::Banner));
        assert_eq!(start_policy(true, Entry::Api), Err(Refusal::Banner), "refused like a key binding");
        assert_eq!(start_policy(true, Entry::FileDrop), Err(Refusal::Quiet));
        assert_eq!(trigger_warning("Run Coprocess\u{2026}"), "Run Coprocess trigger: Can't run two coprocesses at once.");
    }

    #[test]
    fn stderr_banner_text() {
        assert_eq!(stderr_banner("grep x"), "Coprocess \u{201c}grep x\u{201d} terminated with output on stderr.");
    }

    #[test]
    fn file_drop_command() {
        let files = vec![r"C:\a b\x.txt".to_string(), r"C:\y.txt".to_string()];
        assert_eq!(drop_command("", &files), None, "no fileDropCoprocess: a drop pastes as usual");
        assert_eq!(drop_command("upload \\(filenames)", &files), Some(Ok(r#"upload "C:\a b\x.txt" "C:\y.txt""#.into())));
        assert_eq!(drop_command("upload", &files), Some(Ok("upload".into())));
    }

    /// The one cmd.exe builder and its refusal: a dropped name cmd.exe would read is refused
    /// (with the message), whether or not the template quotes the placeholder; a pre-quoted
    /// placeholder gets the path bare, one argument.
    #[test]
    fn dropped_names_cannot_run_commands() {
        let refused = Some(Err(crate::semantic::CMD_REFUSED.to_string()));
        for name in [r"C:\d\a&calc&.txt", r"C:\d\%PATH%.txt", r"C:\d\a(1).txt", "C:\\"] {
            let files = vec![r"C:\ok.txt".to_string(), name.to_string()];
            assert_eq!(drop_command("x \\(filenames)", &files), refused, "{name}");
            assert_eq!(drop_command("x \"\\(filenames)\"", &files), refused, "{name}");
        }
        let line = drop_command("upload \"\\(filenames)\"", &[r"C:\a b\x.txt".to_string()]).unwrap().unwrap();
        assert_eq!(crate::semantic::cmd_command_line(&line), r#"cmd.exe /d /v:off /s /c "upload "C:\a b\x.txt"""#);
        assert_eq!(crate::semantic::split_command_line(&line), ["upload", r"C:\a b\x.txt"]);
    }
}

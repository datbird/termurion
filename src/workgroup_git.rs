//! Git items and Diff mode (milestone 13y1, Task 6): the pure side. The changed-file groups and the
//! "All Files (n)" row, the navigation label, the base selector's recents, the per-file command as an
//! argument vector, and the Diff waiting overlay's rules and texts. The git runs are `git_state`'s;
//! the app side is `app/workgroup_git_app.rs`.
//!
//! Citations: `STV` `iTermSessionToolbarView.swift`, `GBS` `CCGitBaseSelectorItem.swift`, `WI`
//! `iTermWorkgroupInstance.swift`, `WPP` `iTermWorkgroupPeerPort.swift`, `WNT`
//! `WorkgroupNavigationToolbarItem.swift`, `DWV` `DiffWaitingPromptView.swift`, `DWO`
//! `SessionView+DiffWaitingPromptOverlay.swift` under `~/gitrepos/iterm2-ref/sources`.

use crate::git_state::{Change, FileStatus};
use crate::inert_files::display_text;
use std::time::Duration;

/// The workgroup poller's cadence (`WI:942`).
pub const CADENCE: Duration = Duration::from_secs(2);
/// The default base (`GBS:36`): HEAD, which asks the poller for no base at all.
pub const DEFAULT_BASE: &str = "HEAD";
/// The stored recents (`GBS:37`), and how many and how long they are.
pub const RECENTS_KEY: &str = "NoSyncWorkgroupGitBaseRecents";
pub const MAX_RECENTS: usize = 16;
pub const MAX_RECENT_LEN: usize = 200;
/// The longest base ref a command takes, and the longest file name (B4).
pub const MAX_BASE: usize = 256;
pub const MAX_FILE: usize = 1024;

pub const EMPTY_DIFF: &str = "Empty Diff";
pub const STAGED: &str = "Staged";
pub const UNSTAGED: &str = "Unstaged";

/// The staged and unstaged groups (`STV:421-470`): staged is an index change, unstaged a working tree
/// change that is not untracked; a file in both is listed in both and counted once.
#[derive(Clone, Debug, PartialEq)]
pub struct Groups {
    pub staged: Vec<FileStatus>,
    pub unstaged: Vec<FileStatus>,
    pub unique: usize,
}

pub fn groups(statuses: &[FileStatus]) -> Groups {
    let staged: Vec<FileStatus> = statuses.iter().filter(|s| s.index != Change::None).cloned().collect();
    let unstaged: Vec<FileStatus> = statuses.iter().filter(|s| s.workdir != Change::None && s.workdir != Change::Untracked).cloned().collect();
    let unique = staged.iter().chain(unstaged.iter()).map(|s| s.path.as_str()).collect::<std::collections::HashSet<_>>().len();
    Groups { staged, unstaged, unique }
}

/// `containsDiffableChange`: any change that is not untracked (difftool ignores untracked files).
pub fn diffable(statuses: &[FileStatus]) -> bool {
    statuses.iter().any(|s| (s.index != Change::None && s.index != Change::Untracked) || (s.workdir != Change::None && s.workdir != Change::Untracked))
}

pub fn all_files_title(unique: usize) -> String {
    format!("All Files ({unique})")
}

/// "X/Y" for a picked file, None (the label hides) otherwise (`WI:1540-1552`).
pub fn nav_label(position: usize, count: usize) -> Option<String> {
    (position > 0).then(|| format!("{position}/{count}"))
}

/// The saved recents with `committed` moved to the front (`GBS:recordRecent`): HEAD, an empty and an
/// over-long value are not recorded, and a saved value that is over-long is dropped.
pub fn recents(saved: &[String], committed: &str, max: usize) -> Vec<String> {
    let mut out: Vec<String> = saved.iter().filter(|s| !s.is_empty() && s.chars().count() <= MAX_RECENT_LEN).cloned().collect();
    if !committed.is_empty() && committed != DEFAULT_BASE && committed.chars().count() <= MAX_RECENT_LEN {
        out.retain(|s| s != committed);
        out.insert(0, committed.to_string());
    }
    out.truncate(max);
    out
}

/// A base ref a command may take (B4): not blank, not an option, no control character, bounded.
pub fn base_ok(base: &str) -> bool {
    !base.trim().is_empty() && !base.starts_with('-') && base.chars().count() <= MAX_BASE && !base.chars().any(char::is_control)
}

/// What the base field's text commits as (`currentBase`): trimmed, empty is HEAD; an option or a
/// control character is refused, with the banner text.
pub fn typed_base(text: &str) -> Result<String, &'static str> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(DEFAULT_BASE.to_string());
    }
    if base_ok(t) { Ok(t.to_string()) } else { Err("That is not a usable base: it may not start with a dash, hold a control character or be over 256 characters.") }
}

/// A row of the changed-file popup.
#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    AllFiles(String),
    Header(String),
    /// A file: its change letter, the shown (shortened, cleaned) name and the path a pick uses.
    File { letter: char, text: String, path: String },
}

fn letter(c: Change) -> char {
    match c {
        Change::Modified => 'M',
        Change::Added => 'A',
        Change::Deleted => 'D',
        Change::Renamed => 'R',
        Change::TypeChange => 'T',
        Change::Untracked => '?',
        Change::Conflicted => 'U',
        Change::None => ' ',
    }
}

/// The leading directory segments every path shares (`commonDirectoryPrefixLength`).
fn common_prefix(paths: &[&str]) -> usize {
    let dirs: Vec<Vec<&str>> = paths.iter().map(|p| { let mut c: Vec<&str> = p.split('/').filter(|c| !c.is_empty()).collect(); c.pop(); c }).collect();
    let Some(first) = dirs.first() else { return 0 };
    (0..first.len()).take_while(|&i| dirs.iter().all(|d| d.get(i) == first.get(i))).count()
}

/// The changed-file selector (`CCDiffSelectorItem`): the list, the pick, and the walk.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selector {
    statuses: Vec<FileStatus>,
    ordered: Vec<String>,
    picked: Option<String>,
    anchor: Option<String>,
}

impl Selector {
    /// A new list (`set(fileStatuses:)`): a pick that is no longer a row resets to All Files; the walk's
    /// anchor stays.
    pub fn set(&mut self, statuses: &[FileStatus]) {
        let g = groups(statuses);
        let mut seen = std::collections::HashSet::new();
        self.ordered = g.staged.iter().chain(g.unstaged.iter()).map(|s| s.path.clone()).filter(|p| seen.insert(p.clone())).collect();
        self.statuses = statuses.to_vec();
        if self.picked.as_ref().is_some_and(|p| !self.ordered.contains(p)) {
            self.picked = None;
        }
    }

    pub fn rows(&self) -> Vec<Row> {
        let g = groups(&self.statuses);
        let has = !g.staged.is_empty() || !g.unstaged.is_empty();
        let mut rows = vec![Row::AllFiles(if has { all_files_title(g.unique) } else { EMPTY_DIFF.to_string() })];
        let all: Vec<&str> = self.statuses.iter().map(|s| s.path.as_str()).collect();
        let skip = common_prefix(&all);
        let mut group = |title: &str, list: &[FileStatus], col: fn(&FileStatus) -> Change| {
            if list.is_empty() {
                return;
            }
            rows.push(Row::Header(title.to_string()));
            for f in list {
                let short: Vec<&str> = f.path.split('/').filter(|c| !c.is_empty()).skip(skip).collect();
                rows.push(Row::File { letter: letter(col(f)), text: display_text(&short.join("/"), 128, false), path: f.path.clone() });
            }
        };
        group(STAGED, &g.staged, |f| f.index);
        group(UNSTAGED, &g.unstaged, |f| f.workdir);
        rows
    }

    pub fn picked(&self) -> Option<&str> {
        self.picked.as_deref()
    }

    /// The popup's choice: a file, or None for All Files (which forgets the walk's anchor).
    pub fn pick(&mut self, file: Option<&str>) {
        self.picked = file.map(str::to_string);
        self.anchor = self.picked.clone();
    }

    pub fn count(&self) -> usize {
        self.ordered.len()
    }

    fn index(&self) -> Option<usize> {
        self.picked.as_ref().and_then(|p| self.ordered.iter().position(|o| o == p))
    }

    /// 1-based place of the picked file, 0 for All Files.
    pub fn position(&self) -> usize {
        self.index().map_or(0, |i| i + 1)
    }

    pub fn can_back(&self) -> bool {
        self.index().is_some_and(|i| i > 0)
    }

    pub fn can_forward(&self) -> bool {
        self.index().is_some_and(|i| i + 1 < self.ordered.len())
    }

    /// The next (or previous) file, wrapping (`advanceFile`); it becomes the pick.
    pub fn advance(&mut self, forward: bool) -> Option<String> {
        let n = self.ordered.len();
        if n == 0 {
            return None;
        }
        let anchor = self.anchor.clone().or_else(|| self.picked.clone());
        let chosen = match anchor {
            Some(a) => match self.ordered.iter().position(|o| *o == a) {
                Some(i) => self.ordered[if forward { (i + 1) % n } else { (i + n - 1) % n }].clone(),
                None if forward => self.ordered.iter().find(|o| o.as_str() > a.as_str()).unwrap_or(&self.ordered[0]).clone(),
                None => self.ordered.iter().rev().find(|o| o.as_str() < a.as_str()).unwrap_or(&self.ordered[n - 1]).clone(),
            },
            None => self.ordered[if forward { 0 } else { n - 1 }].clone(),
        };
        self.pick(Some(&chosen));
        Some(chosen)
    }
}

/// Whether a Diff member's deferred launch fires: its member is shown and git reports a change
/// (`WI:1364-1385`).
pub fn diff_should_launch(visible: bool, has_changes: bool) -> bool {
    visible && has_changes
}

pub const WAITING_TITLE: &str = "Diff session is waiting for changes.";
pub const WAITING_BODY: &str = "This session is set to Diff mode and only starts its command when git reports staged or unstaged changes. The working tree is currently clean, or Tandem hasn\u{2019}t finished its first check yet. Edit a file (or stage one) and the command will start on its own. To run it now anyway, click below.";
pub const RELOAD_TITLE: &str = "Reload queued. Waiting for changes.";
pub const RELOAD_BODY: &str = "This session is set to Diff mode, so Reload will re-run the command only after git reports staged or unstaged changes. The previous output is still on screen behind this panel. Cancel to keep that output and abandon the reload, or run the command now anyway.";
pub const RUN_ANYWAY: &str = "Run Anyway";

/// Why a command was not made; nothing starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    Empty,
    Expression,
    Base,
    File,
}

impl Refusal {
    pub fn text(self) -> &'static str {
        match self {
            Refusal::Empty => "This member has no command to run.",
            Refusal::Expression => "A workgroup command uses an expression Tandem does not run here, so nothing started.",
            Refusal::Base => "The base for this command is not a usable name (not blank, no leading dash, no control character, at most 256 characters), so nothing started.",
            Refusal::File => "That file name cannot be passed to a command (a control character, empty or over 1024 characters), so nothing started.",
        }
    }
}

fn argv(template: &str, file: Option<&str>, base: &str) -> Result<Vec<String>, Refusal> {
    let tokens = crate::semantic::split_command_line(template);
    let Some(program) = tokens.first() else { return Err(Refusal::Empty) };
    // A value is never the program, and nothing but the two known expressions is run.
    let known = |t: &str| t.replace("\\(gitBase)", "").replace("\\(file)", "");
    if program.contains("\\(") || tokens.iter().any(|t| known(t).contains("\\(")) {
        return Err(Refusal::Expression);
    }
    let uses = |what: &str| tokens.iter().any(|t| t.contains(what));
    if uses("\\(gitBase)") && !base_ok(base) {
        return Err(Refusal::Base);
    }
    let file_arg = if uses("\\(file)") {
        let f = file.ok_or(Refusal::File)?;
        if f.is_empty() || f.len() > MAX_FILE || f.chars().any(char::is_control) {
            return Err(Refusal::File);
        }
        Some(if f.starts_with('-') { format!("./{f}") } else { f.to_string() })
    } else {
        None
    };
    Ok(tokens
        .into_iter()
        .map(|t| {
            let t = t.replace("\\(gitBase)", base);
            match &file_arg {
                Some(f) => t.replace("\\(file)", f),
                None => t,
            }
        })
        .collect())
}

/// The per-file command as arguments (B4): the person's template split first, then `\(file)` and
/// `\(gitBase)` put inside their own tokens, so each value is exactly one argument.
pub fn per_file_argv(template: &str, file: &str, base: &str) -> Result<Vec<String>, Refusal> {
    argv(template, Some(file), base)
}

/// The All Files command (the member's own command) as arguments.
pub fn all_files_argv(template: &str, base: &str) -> Result<Vec<String>, Refusal> {
    argv(template, None, base)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_state::{Change, FileStatus};

    fn st(path: &str, index: Change, workdir: Change) -> FileStatus {
        FileStatus { path: path.into(), index, workdir }
    }

    /// The groups follow `CCDiffSelectorItem.set(fileStatuses:)` (`STV:421-470`): a file staged and
    /// unstaged is listed twice and counted once; untracked files make no change; "Empty Diff" with
    /// nothing; the navigation label and the recents follow the source.
    #[test]
    fn changed_files_follow_iterm2() {
        let list = [st("a.rs", Change::Modified, Change::None), st("b.rs", Change::None, Change::Modified), st("c.rs", Change::Added, Change::Modified)];
        let g = groups(&list);
        assert_eq!(g.staged.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["a.rs", "c.rs"]);
        assert_eq!(g.unstaged.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["b.rs", "c.rs"]);
        assert_eq!(g.unique, 3);
        assert_eq!(all_files_title(g.unique), "All Files (3)");
        assert!(diffable(&list));

        let untracked = [st("u.rs", Change::None, Change::Untracked)];
        let g = groups(&untracked);
        assert!(g.staged.is_empty() && g.unstaged.is_empty() && g.unique == 0);
        assert!(!diffable(&untracked));
        assert!(!diffable(&[]));
        assert_eq!((EMPTY_DIFF, STAGED, UNSTAGED), ("Empty Diff", "Staged", "Unstaged"));

        assert_eq!(nav_label(2, 3).as_deref(), Some("2/3"));
        assert_eq!(nav_label(0, 3), None, "All Files picked: the label hides");

        assert_eq!(recents(&["main".to_string()], "dev", 16), ["dev", "main"]);
        assert_eq!(recents(&["dev".to_string(), "main".to_string()], "main", 16), ["main", "dev"], "an existing value moves to the front");
        assert_eq!(recents(&["main".to_string()], "HEAD", 16), ["main"], "HEAD is never recorded");
        assert_eq!(recents(&["main".to_string()], "", 16), ["main"]);
        assert_eq!(recents(&["x".repeat(MAX_RECENT_LEN + 1)], "y", 16), ["y"], "an over-long saved value is dropped");
        assert_eq!(recents(&["x".repeat(MAX_RECENT_LEN + 1)], "z".repeat(MAX_RECENT_LEN + 1).as_str(), 16), Vec::<String>::new());
        let many: Vec<String> = (0..20).map(|i| format!("b{i}")).collect();
        assert_eq!(recents(&many, "new", 16).len(), 16);
        assert_eq!(recents(&many, "new", 16)[0], "new");
        assert_eq!((CADENCE.as_secs(), DEFAULT_BASE, RECENTS_KEY, MAX_RECENTS), (2, "HEAD", "NoSyncWorkgroupGitBaseRecents", 16));

        // The typed base: trimmed, empty is HEAD, an option or a control character is refused (B4).
        assert_eq!(typed_base("  main "), Ok("main".to_string()));
        assert_eq!(typed_base("   "), Ok("HEAD".to_string()));
        for bad in ["-x", "--output=a", "a\x03b", "a\nb", "x".repeat(MAX_BASE + 1).as_str()] {
            assert!(typed_base(bad).is_err(), "{bad:?}");
        }
    }

    /// The popup's rows: a group header and the letter plus the path shortened by the common directory
    /// prefix (`STV:580-660`); names are cleaned for display.
    #[test]
    fn selector_rows_follow_iterm2() {
        let list = [st("src/a/x.rs", Change::Modified, Change::None), st("src/a/y.rs", Change::None, Change::Deleted), st("src/b/z.rs", Change::Added, Change::None)];
        let mut s = Selector::default();
        s.set(&list);
        let rows = s.rows();
        assert_eq!(rows[0], Row::AllFiles("All Files (3)".into()));
        assert_eq!(rows[1], Row::Header("Staged".into()));
        assert_eq!(rows[2], Row::File { letter: 'M', text: "a/x.rs".into(), path: "src/a/x.rs".into() });
        assert_eq!(rows[3], Row::File { letter: 'A', text: "b/z.rs".into(), path: "src/b/z.rs".into() });
        assert_eq!(rows[4], Row::Header("Unstaged".into()));
        assert_eq!(rows[5], Row::File { letter: 'D', text: "a/y.rs".into(), path: "src/a/y.rs".into() });
        s.set(&[]);
        assert_eq!(s.rows(), vec![Row::AllFiles("Empty Diff".into())]);
        let mut s = Selector::default();
        s.set(&[st("a\u{202e}b.rs", Change::Modified, Change::None)]);
        let Row::File { text, path, .. } = &s.rows()[2] else { panic!("a file row") };
        assert!(!text.contains('\u{202e}'), "the shown name is inert");
        assert_eq!(path, "a\u{202e}b.rs", "the path itself is what a pick uses");
    }

    /// Back and Forward walk the deduplicated list, wrapping; an anchor that left the list lands on the
    /// next name by string order; a pick that left the list resets to All Files (`STV:682-760`).
    #[test]
    fn navigation_follows_iterm2() {
        let list = [st("a.rs", Change::Modified, Change::Modified), st("b.rs", Change::Modified, Change::None), st("c.rs", Change::None, Change::Modified)];
        let mut s = Selector::default();
        s.set(&list);
        assert_eq!((s.count(), s.position(), s.picked()), (3, 0, None));
        assert!(!s.can_back() && !s.can_forward(), "All Files has no neighbors");
        assert_eq!(s.advance(true).as_deref(), Some("a.rs"), "from All Files, forward is the first");
        assert_eq!(nav_label(s.position(), s.count()).as_deref(), Some("1/3"));
        assert!(!s.can_back() && s.can_forward());
        assert_eq!(s.advance(true).as_deref(), Some("b.rs"));
        assert_eq!(s.advance(true).as_deref(), Some("c.rs"));
        assert_eq!(s.advance(true).as_deref(), Some("a.rs"), "wraps");
        assert_eq!(s.advance(false).as_deref(), Some("c.rs"), "wraps back");
        s.pick(None);
        assert_eq!(s.advance(false).as_deref(), Some("c.rs"), "from All Files, back is the last");
        // The picked file stays picked while it is listed, and resets when it is not.
        s.set(&list);
        assert_eq!(s.picked(), Some("c.rs"));
        s.set(&[st("a.rs", Change::Modified, Change::None)]);
        assert_eq!(s.picked(), None);
        // An anchor that left the list: forward finds the next name by string order.
        let mut s = Selector::default();
        s.set(&list);
        s.pick(Some("b.rs"));
        s.set(&[st("a.rs", Change::Modified, Change::None), st("c.rs", Change::Modified, Change::None)]);
        assert_eq!(s.advance(true).as_deref(), Some("c.rs"));
        let mut empty = Selector::default();
        assert_eq!(empty.advance(true), None);
    }

    /// Diff mode: a buried or empty Diff peer waits; the texts are iTerm2's with Tandem for iTerm2.
    #[test]
    fn diff_mode_follows_iterm2() {
        assert!(!diff_should_launch(false, true), "a buried peer waits");
        assert!(!diff_should_launch(true, false));
        assert!(diff_should_launch(true, true));
        assert_eq!(WAITING_TITLE, "Diff session is waiting for changes.");
        assert_eq!(RELOAD_TITLE, "Reload queued. Waiting for changes.");
        assert_eq!(RUN_ANYWAY, "Run Anyway");
        assert!(WAITING_BODY.starts_with("This session is set to Diff mode and only starts its command when git reports staged or unstaged changes.") && WAITING_BODY.contains("or Tandem hasn\u{2019}t finished its first check yet"));
        assert!(RELOAD_BODY.starts_with("This session is set to Diff mode, so Reload will re-run the command only after git reports staged or unstaged changes.") && RELOAD_BODY.ends_with("or run the command now anyway."));
        assert!(!WAITING_BODY.contains("iTerm2") && !RELOAD_BODY.contains("iTerm2"));
    }

    /// A value is exactly one argument whatever it holds (B4): the person's template is split first and
    /// the file and the base go inside their own tokens; nothing is quoted.
    #[test]
    fn per_file_argv_is_one_argument_per_value() {
        let t = "git diff \\(gitBase) -- \\(file)";
        let ok = |file: &str, base: &str| per_file_argv(t, file, base).unwrap();
        assert_eq!(ok("a b's.rs", "HEAD"), ["git", "diff", "HEAD", "--", "a b's.rs"]);
        assert_eq!(ok("50%.txt", "HEAD"), ["git", "diff", "HEAD", "--", "50%.txt"]);
        assert_eq!(ok("x&y (1).rs", "HEAD"), ["git", "diff", "HEAD", "--", "x&y (1).rs"]);
        assert_eq!(ok("--output=x", "HEAD"), ["git", "diff", "HEAD", "--", "./--output=x"], "a leading dash is a relative name");
        assert_eq!(ok("a.rs", "HEAD~1"), ["git", "diff", "HEAD~1", "--", "a.rs"]);
        assert_eq!(ok("a.rs", "main..dev"), ["git", "diff", "main..dev", "--", "a.rs"]);
        assert_eq!(ok("a.rs", ";rm -rf ~"), ["git", "diff", ";rm -rf ~", "--", "a.rs"], "one argument, never syntax");
        for bad in ["a\x03b", "a\0b", "a\rb", "", "n".repeat(MAX_FILE + 1).as_str()] {
            assert_eq!(per_file_argv(t, bad, "HEAD"), Err(Refusal::File), "{bad:?}");
        }
        for bad in ["-x", "--output=a", "", "  ", "a\x03", "b".repeat(MAX_BASE + 1).as_str()] {
            assert_eq!(per_file_argv(t, "a.rs", bad), Err(Refusal::Base), "{bad:?}");
        }
        assert_eq!(per_file_argv("git difftool -y -x vimdiff \\(gitBase) -- \\(file)", "f.rs", "HEAD").unwrap(), ["git", "difftool", "-y", "-x", "vimdiff", "HEAD", "--", "f.rs"], "the person's own -x stays");
        assert_eq!(per_file_argv("git diff \\(user.x)", "f.rs", "HEAD"), Err(Refusal::Expression));
        assert_eq!(per_file_argv("\\(file) x", "f.rs", "HEAD"), Err(Refusal::Expression), "a value is never the program");
        assert_eq!(per_file_argv("", "f.rs", "HEAD"), Err(Refusal::Empty));
        // All Files: the same rules with no file.
        assert_eq!(all_files_argv("git diff \\(gitBase)", "HEAD").unwrap(), ["git", "diff", "HEAD"]);
        assert_eq!(all_files_argv("git diff \\(gitBase)", "-x"), Err(Refusal::Base));
        assert_eq!(all_files_argv("lazygit", "-x").unwrap(), ["lazygit"], "a template with no base takes none");
        assert!(Refusal::Base.text().contains("base") && Refusal::File.text().contains("file"));
        let src = include_str!("workgroup_git.rs").split("#[cfg(test)]").next().unwrap();
        assert!(!src.contains("fn quote") && !src.contains("'\\\\''"), "no second quoter (B3)");
    }
}

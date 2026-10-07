//! git as data (milestone 9i, Task 4): the porcelain parser, the repository operation read
//! from the git directory, the local-path and repository-config guards, `fetch_state` over
//! a `GitRun` trait with one deadline, iTerm2's worker cache and poller state machine, the
//! rendered string, the click menu model, and the remote state from `user.git*` variables.
//! Pure code: `git_run_win.rs` (Task 5) implements `GitRun` with a hidden process in a Job
//! Object; `src/app/git_ui.rs` owns a `GitPoller` per pane.
//!
//! The git CLI stands in for iTerm2's libgit2 helper. Citations use the milestone 9i
//! brief's abbreviations, paths under `~/gitrepos/iterm2-ref/sources/`: `GIT` =
//! `StatusBar/Components/iTermStatusBarGitComponent.m`, `GSM` = `iTermGitStringMaker.m`,
//! `GP` = `StatusBar/iTermGitPoller.m`, `GPW` = `StatusBar/Components/iTermGitPollWorker.m`,
//! `GS`/`GSMA` = `StatusBar/Components/iTermGitState.m`/`iTermGitState+MainApp.m`, `GCL` =
//! `../pidinfo/iTermGitClient.m`.
//!
//! Security. Opening a network path (`\\host\share`) sends the user's Windows credentials to
//! that host, so nothing here opens, or lets git open, a path the one judge refuses
//! (`localfs::LocalPath`, `netpath::judge`: mapped and SUBST drives, links to shares): the session directory, symbolic links on the way to it, every
//! `.git` git could find above it, a `.git` file's `gitdir:`, a worktree's `commondir`,
//! `objects/info/alternates`, every path-like value in the repository's own config and the
//! files it includes, and the git directory `rev-parse` reports. A repository's own config
//! also must not run programs through the poll. `git status` runs three kinds: the
//! fsmonitor hook (`core.fsmonitor`, switched off with `-c`), the `post-index-change` hook
//! (only when status rewrites the index, which `--no-optional-locks` stops), and the clean
//! filters of `.gitattributes` drivers (`filter.<name>.clean`/`.process`, run when a file's
//! stat data changed and git hashes it). `-c` has no wildcard, so every filter driver the
//! repository's config defines is switched off by name. A repository whose config defines a
//! git-lfs extension is not polled (the trusted global lfs filter would run its command). A
//! lazy fetch of a partial clone is stopped with `GIT_NO_LAZY_FETCH`; on git older than
//! 2.44 a partial clone is not polled. `-c protocol.ext.allow=never` and
//! `-c core.virtualFilesystem=` are defense in depth. Links inside the git directory must
//! stay local too. Hooks, pagers, diff drivers and credential helpers are not reached by
//! `status --porcelain`, `rev-parse` or `for-each-ref`. Menu commands never pass a branch
//! that git could read as an option.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::localfs::{self, LocalPath};
use crate::prompted::{local_path_ok, network_path};

/// The repository operation in progress (libgit2's `git_repository_state`, folded as
/// `GCL:660-689` folds it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RepoState {
    #[default]
    None,
    Merge,
    Revert,
    Cherrypick,
    Bisect,
    Rebase,
    Apply,
}

impl RepoState {
    /// The word the component shows instead of the branch (`GSM:52-66`).
    pub fn text(self) -> Option<&'static str> {
        match self {
            RepoState::None => None,
            RepoState::Merge => Some("Merging"),
            RepoState::Revert => Some("Reverting"),
            RepoState::Cherrypick => Some("Cherrypicking"),
            RepoState::Bisect => Some("Bisecting"),
            RepoState::Rebase => Some("Rebasing"),
            RepoState::Apply => Some("Applying"),
        }
    }
}

/// iTerm2's `iTermGitState`. `ahead` and `behind` are strings, `""` without a live
/// upstream; the rich stats come only from a remote host's variables.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct GitState {
    pub branch: String,
    pub ahead: String,
    pub behind: String,
    pub dirty: bool,
    pub adds: i64,
    pub deletes: i64,
    pub lines_inserted: i64,
    pub lines_deleted: i64,
    pub files_added: i64,
    pub files_modified: i64,
    pub files_deleted: i64,
    pub repo_state: RepoState,
    /// The changed files against HEAD or a base (`populateHeadFileStatusesOnState:`); None unless the
    /// poll asked (a workgroup's). Untracked files are not in it.
    pub file_statuses: Option<Vec<FileStatus>>,
}

/// `iTermGitFileChangeKind` (`GCL:198-269`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    None,
    Added,
    Modified,
    Deleted,
    Renamed,
    TypeChange,
    Untracked,
    Conflicted,
}

/// `iTermGitFileStatus`: a path with its index and its working tree change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileStatus {
    pub path: String,
    pub index: Change,
    pub workdir: Change,
}

impl GitState {
    /// `prettyDescription` (`GS:153-159`). The directory and xcode fields are never set on
    /// Windows, so they print as `(null)`.
    pub fn pretty(&self) -> String {
        format!(
            "dir=(null) xcode=(null) ahead={} behind={} branch={} dirty={} adds={} deletes={} lines=+{}/-{} files=+{}/~{}/-{}",
            self.ahead,
            self.behind,
            self.branch,
            self.dirty as u8,
            self.adds,
            self.deletes,
            self.lines_inserted,
            self.lines_deleted,
            self.files_added,
            self.files_modified,
            self.files_deleted
        )
    }
}

/// `git status --porcelain=v2 --branch -z` as libgit2's state (`GCL:281-314,590-630`): an
/// unborn branch (`(initial)`) has no state; a detached HEAD's branch is the full commit id;
/// no `branch.ab` means no upstream (ahead and behind `""`); dirty is any entry; adds count
/// untracked files; deletes count worktree deletions.
pub fn parse_porcelain(z: &[u8]) -> Option<GitState> {
    let mut st = GitState::default();
    let (mut oid, mut head, mut ab) = (None, None, None);
    let mut records = z.split(|&b| b == 0);
    while let Some(rec) = records.next() {
        if let Some(h) = rec.strip_prefix(b"# ") {
            let h = String::from_utf8_lossy(h);
            if let Some(v) = h.strip_prefix("branch.oid ") {
                oid = Some(v.to_string());
            } else if let Some(v) = h.strip_prefix("branch.head ") {
                head = Some(v.to_string());
            } else if let Some(v) = h.strip_prefix("branch.ab ") {
                ab = Some(v.to_string());
            }
            continue;
        }
        match rec.first() {
            Some(b'1') | Some(b'2') => {
                st.dirty = true;
                if rec.get(3) == Some(&b'D') {
                    st.deletes += 1;
                }
                if rec[0] == b'2' {
                    // A rename or copy's original path is the next record.
                    records.next();
                }
            }
            Some(b'u') => st.dirty = true,
            Some(b'?') => {
                st.dirty = true;
                st.adds += 1;
            }
            _ => {}
        }
    }
    let oid = oid?;
    if oid == "(initial)" {
        return None;
    }
    let head = head?;
    st.branch = if head == "(detached)" { oid } else { head };
    if let Some(ab) = ab {
        let mut parts = ab.split_whitespace();
        st.ahead = parts.next().map(|a| a.trim_start_matches('+').to_string()).unwrap_or_default();
        st.behind = parts.next().map(|b| b.trim_start_matches('-').to_string()).unwrap_or_default();
    }
    Some(st)
}

/// The most entries a per-file list holds, and the longest name in it. Git output is the repository's:
/// a hostile one cannot grow the list, and a name with a control character, an empty name or one that
/// is not UTF-8 is not listed (it could never be a safe argument).
pub const MAX_FILES: usize = 10_000;
pub const MAX_PATH: usize = 1024;

fn usable_path(p: &[u8]) -> Option<String> {
    let s = std::str::from_utf8(p).ok()?;
    (!s.is_empty() && s.len() <= MAX_PATH && !s.chars().any(char::is_control)).then(|| s.to_string())
}

fn index_change(c: u8) -> Change {
    match c {
        b'A' | b'C' => Change::Added,
        b'M' => Change::Modified,
        b'D' => Change::Deleted,
        b'R' => Change::Renamed,
        b'T' => Change::TypeChange,
        _ => Change::None,
    }
}

fn workdir_change(c: u8) -> Change {
    match c {
        b'A' => Change::Added,
        b'M' => Change::Modified,
        b'D' => Change::Deleted,
        b'R' => Change::Renamed,
        b'T' => Change::TypeChange,
        _ => Change::None,
    }
}

/// `git status --porcelain=v2 -z --find-renames` as `iTermGitFileStatusForEntry` maps it
/// (`GCL:198-269`): the index column from X, the workdir column from Y, an unmerged entry Conflicted on
/// the workdir side, a rename by its new path. Untracked, ignored and header records are not listed.
pub fn parse_file_statuses(z: &[u8]) -> Vec<FileStatus> {
    let mut out = Vec::new();
    let mut records = z.split(|&b| b == 0);
    while let Some(rec) = records.next() {
        if out.len() >= MAX_FILES {
            break;
        }
        let fields = match rec.first() {
            Some(b'1') => 9,
            Some(b'2') => 10,
            Some(b'u') => 11,
            _ => continue,
        };
        if rec[0] == b'2' {
            // A rename's original path is the next record.
            records.next();
        }
        let parts: Vec<&[u8]> = rec.splitn(fields, |&b| b == b' ').collect();
        if parts.len() != fields || parts[1].len() != 2 {
            continue;
        }
        let Some(path) = usable_path(parts[fields - 1]) else { continue };
        let (index, workdir) = if rec[0] == b'u' { (Change::None, Change::Conflicted) } else { (index_change(parts[1][0]), workdir_change(parts[1][1])) };
        if index == Change::None && workdir == Change::None {
            continue;
        }
        out.push(FileStatus { path, index, workdir });
    }
    out
}

/// `git diff --name-status -z --find-renames <tree>` as `populateFileStatusesAgainstBase:` lists it
/// (`GCL:404-465`): every entry in the workdir column; A, D, M, R (new path), T and U are listed, any
/// other letter (a copy) is not.
pub fn parse_base_diff(z: &[u8]) -> Vec<FileStatus> {
    let mut out = Vec::new();
    let mut it = z.split(|&b| b == 0);
    while let Some(code) = it.next() {
        if out.len() >= MAX_FILES {
            break;
        }
        let Some(&first) = code.first() else { continue };
        let two = matches!(first, b'R' | b'C');
        let Some(a) = it.next() else { break };
        let path = if two {
            let Some(b) = it.next() else { break };
            b
        } else {
            a
        };
        let change = match first {
            b'A' => Change::Added,
            b'D' => Change::Deleted,
            b'M' => Change::Modified,
            b'R' => Change::Renamed,
            b'T' => Change::TypeChange,
            b'U' => Change::Conflicted,
            _ => continue,
        };
        if let Some(path) = usable_path(path) {
            out.push(FileStatus { path, index: Change::None, workdir: change });
        }
    }
    out
}

/// The diff stats (`populateDiffStatsOnState:`, `GCL:477-557`) from `--name-status -z --no-renames` and
/// `--numstat -z --no-renames` against HEAD: (files added, modified, deleted, lines inserted, lines
/// deleted). Added counts A entries plus `untracked`; modified counts M and T entries; only M and T
/// entries add their lines (an added file contributes none).
pub fn diff_stats(name_status_z: &[u8], numstat_z: &[u8], untracked: i64) -> (i64, i64, i64, i64, i64) {
    let mut lines: HashMap<&[u8], (i64, i64)> = HashMap::new();
    for rec in numstat_z.split(|&b| b == 0).take(MAX_FILES) {
        let rec = rec.strip_prefix(b"\n").unwrap_or(rec);
        let mut f = rec.splitn(3, |&b| b == b'\t');
        let (Some(a), Some(d), Some(path)) = (f.next(), f.next(), f.next()) else { continue };
        let n = |x: &[u8]| std::str::from_utf8(x).ok().and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
        lines.insert(path, (n(a), n(d)));
    }
    let (mut added, mut modified, mut deleted, mut ins, mut del) = (untracked, 0, 0, 0, 0);
    let mut it = z_records(name_status_z);
    while let Some(code) = it.next() {
        let Some(&first) = code.first() else { continue };
        let Some(path) = it.next() else { break };
        if matches!(first, b'R' | b'C') {
            it.next();
        }
        match first {
            b'A' => added += 1,
            b'D' => deleted += 1,
            b'M' | b'T' => {
                modified += 1;
                let (i, d) = lines.get(path).copied().unwrap_or((0, 0));
                ins += i;
                del += d;
            }
            _ => {}
        }
    }
    (added, modified, deleted, ins, del)
}

fn z_records(z: &[u8]) -> impl Iterator<Item = &[u8]> {
    z.split(|&b| b == 0).take(MAX_FILES * 3)
}

/// The worker cache's key (`GPW:87-105`): the path, `\x01stats` with stats, `\x02<base>` for a base
/// that is not empty or HEAD.
pub fn cache_key(path: &str, base: Option<&str>, stats: bool) -> String {
    let mut key = path.to_string();
    if stats {
        key.push_str("\x01stats");
    }
    if let Some(b) = base.filter(|b| !b.is_empty() && *b != "HEAD") {
        key.push('\x02');
        key.push_str(b);
    }
    key
}

macro_rules! guarded {
    ($($rest:expr),* $(,)?) => {
        &[
            "--no-optional-locks",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.useBuiltinFSMonitor=false",
            "-c",
            "safe.bareRepository=explicit",
            "-c",
            "core.quotepath=false",
            "-c",
            "protocol.ext.allow=never",
            "-c",
            "core.virtualFilesystem=",
            $($rest),*
        ]
    };
}

/// Per-file statuses against HEAD (`populateHeadFileStatusesOnState:`, `GCL:326-347`).
pub const FILES_ARGS: &[&str] = guarded!["status", "--porcelain=v2", "-z", "--untracked-files=no", "--ignore-submodules=all", "--find-renames"];
/// A base's tree: the caller appends `<base>^{tree}`; the typed base never reaches git as an option.
pub const BASE_TREE_ARGS: &[&str] = guarded!["rev-parse", "--verify", "--quiet", "--end-of-options"];
/// Per-file statuses against a base's tree (`GCL:350-475`): the caller appends the tree id and `--`.
pub const BASE_DIFF_ARGS: &[&str] = guarded!["diff", "--name-status", "-z", "--find-renames", "--no-ext-diff", "--no-textconv", "--ignore-submodules=all"];
/// The diff stats' two runs, against HEAD (`GCL:477-557`).
pub const NUMSTAT_ARGS: &[&str] = guarded!["diff", "--numstat", "-z", "--no-renames", "--no-ext-diff", "--no-textconv", "--ignore-submodules=all", "HEAD", "--"];
pub const NAME_STATUS_ARGS: &[&str] = guarded!["diff", "--name-status", "-z", "--no-renames", "--no-ext-diff", "--no-textconv", "--ignore-submodules=all", "HEAD", "--"];

/// The operation in progress, from files in the git directory in libgit2's
/// `git_repository_state` order, in a judged git directory.
/// Links are not followed: a link is neither a file nor a folder here.
pub fn repo_state(git_dir: &LocalPath) -> RepoState {
    let meta = |p: &str| git_dir.join(p).and_then(|p| localfs::lstat(&p).ok());
    let file = |p: &str| meta(p).is_some_and(|m| m.is_file);
    let dir = |p: &str| meta(p).is_some_and(|m| m.is_dir);
    if file("rebase-merge/interactive") || dir("rebase-merge") || file("rebase-apply/rebasing") {
        RepoState::Rebase
    } else if file("rebase-apply/applying") || dir("rebase-apply") {
        RepoState::Apply
    } else if file("MERGE_HEAD") {
        RepoState::Merge
    } else if file("REVERT_HEAD") {
        RepoState::Revert
    } else if file("CHERRY_PICK_HEAD") {
        RepoState::Cherrypick
    } else if file("BISECT_LOG") {
        RepoState::Bisect
    } else {
        RepoState::None
    }
}

/// `for-each-ref`'s output: one branch per line, newest first.
pub fn parse_branches(stdout: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(stdout).lines().map(|l| l.trim_end_matches('\r')).filter(|l| !l.is_empty()).map(str::to_string).collect()
}

/// How one git run ended.
#[derive(Clone, Debug, PartialEq)]
pub enum RunResult {
    /// It exited; `capped` when stdout was cut at the cap.
    Exit { code: i32, stdout: Vec<u8>, capped: bool },
    /// Killed, with its children, at the deadline.
    TimedOut,
    /// It could not start.
    Failed(String),
}

/// Runs git hidden in `dir` with `GIT_ENV` set and `GIT_ENV_REMOVED` removed, stdin null,
/// and kills it with its process tree at `timeout`. `cap` limits the stdout kept.
pub trait GitRun: Send + Sync {
    fn run(&self, git: &Path, dir: &Path, args: &[String], timeout: Duration, cap: Option<usize>) -> RunResult;
}

/// The hardening every git line here starts with (`STATUS_ARGS` and the others begin with
/// it), for a git run outside the poller that must be as safe: it2git, run by the prompt.
pub const HARDENED: &[&str] = &[
    "--no-optional-locks",
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.useBuiltinFSMonitor=false",
    "-c",
    "safe.bareRepository=explicit",
    "-c",
    "core.quotepath=false",
    "-c",
    "protocol.ext.allow=never",
    "-c",
    "core.virtualFilesystem=",
];

/// `args` as a guarded git line: `HARDENED`, the filter overrides `vet_repo` found, then
/// `args` (the subcommand and its arguments).
pub fn guarded_args(vet: &Vetted, args: &[&str]) -> Vec<String> {
    HARDENED.iter().map(|a| a.to_string()).chain(vet.args.iter().cloned()).chain(args.iter().map(|a| a.to_string())).collect()
}

/// What a git started in a pane adds to `HARDENED`: no pager (a repository's `core.pager` and `pager.*`
/// run programs), no external diff, no hooks, no credential helper, no ssh program, no transport but
/// the repository's own files.
const LAUNCH_ARGS: &[&str] = &[
    "--no-pager",
    "-c",
    "core.hooksPath=NUL",
    "-c",
    "core.pager=cat",
    "-c",
    "diff.external=",
    "-c",
    "credential.helper=",
    "-c",
    "protocol.allow=never",
    "-c",
    "core.sshCommand=false",
    // Never into a submodule: a nested repository's own config is not vetted, and porcelain diff and
    // status run git inside each one (its filters, its fsmonitor). The command-line flag after the
    // subcommand is what wins over `.gitmodules`' `ignore`; these keep every other path short.
    "-c",
    "submodule.recurse=false",
    "-c",
    "diff.submodule=short",
    "-c",
    "diff.ignoreSubmodules=all",
    "-c",
    "status.submoduleSummary=false",
];

/// The git subcommands a pane launch runs: each takes `--ignore-submodules=all`. Any other is refused
/// (a template's own global option before the subcommand, such as `-c`, is refused with it).
const LAUNCH_SUBCOMMANDS: &[&str] = &["diff", "difftool", "status", "show", "log"];

/// The long options a template must not pass after the subcommand: each would undo an override (a later
/// option wins). Matched as git matches them, by any unambiguous prefix and through `--no-`.
const LAUNCH_UNDOES: &[&str] = &["ignore-submodules", "submodule", "recurse-submodules", "ext-diff", "textconv"];

/// Why a template's argument must not reach a launched git: it re-enables a submodule, an external diff or
/// a textconv. Arguments after `--` are paths.
fn undoes_an_override(arg: &str) -> bool {
    let Some(long) = arg.strip_prefix("--") else { return false };
    let name = long.split('=').next().unwrap_or("");
    // Our own options, and `--text` (an exact option name wins over an abbreviation of `--textconv`).
    if matches!(arg, "--no-ext-diff" | "--no-textconv" | "--text") {
        return false;
    }
    let mut core = name;
    while let Some(rest) = core.strip_prefix("no-") {
        core = rest;
    }
    !core.is_empty() && LAUNCH_UNDOES.iter().any(|u| u.starts_with(core))
}

/// THE git argument vector for a git started in a pane (every workgroup launch of git takes it):
/// `HARDENED`, `LAUNCH_ARGS` and the vet's filter overrides, then the subcommand (one of
/// `LAUNCH_SUBCOMMANDS`) with `--ignore-submodules=all` right after it, and `--no-ext-diff --no-textconv`
/// after `diff`, `show` and `log`. The rest is the person's own template, less any option that would undo
/// an override (refused).
pub fn launch_args(vet: &Vetted, args: &[&str]) -> Result<Vec<String>, String> {
    let Some((sub, rest)) = args.split_first() else { return Err("The git command has no subcommand, so it did not start.".into()) };
    if !LAUNCH_SUBCOMMANDS.contains(sub) {
        return Err("Tandem starts git in a workgroup only for diff, difftool, status, show or log, so this command did not start.".into());
    }
    let paths_from = rest.iter().position(|a| *a == "--").unwrap_or(rest.len());
    if rest[..paths_from].iter().any(|a| undoes_an_override(a)) {
        return Err("The git command turns submodules, an external diff or a text conversion back on, so it did not start.".into());
    }
    let mut out: Vec<String> = HARDENED.iter().chain(LAUNCH_ARGS).map(|a| a.to_string()).chain(vet.args.iter().cloned()).collect();
    out.push(sub.to_string());
    out.push("--ignore-submodules=all".into());
    if matches!(*sub, "diff" | "show" | "log") {
        out.push("--no-ext-diff".into());
        out.push("--no-textconv".into());
    }
    out.extend(rest.iter().map(|a| a.to_string()));
    Ok(out)
}

/// What a launch removes from the environment it inherits: `GIT_ENV_REMOVED` (another repository than the
/// vetted one) and every variable that names a program git runs or config it reads.
pub const LAUNCH_ENV_REMOVED: &[&str] =
    &["GIT_EXTERNAL_DIFF", "GIT_ASKPASS", "SSH_ASKPASS", "GIT_CONFIG_PARAMETERS", "GIT_CONFIG_COUNT", "GIT_CONFIG", "GIT_EXEC_PATH", "GIT_SSH", "GIT_SSH_COMMAND", "GIT_PROXY_COMMAND"];

/// The environment of a git started in a pane, as the session's `NAME=value` entries; a `-NAME` entry
/// removes the variable. The pane applies them last (`session_env::launch_last`), so they reach the child.
pub fn launch_env() -> Vec<String> {
    let mut out: Vec<String> = GIT_ENV.iter().map(|(k, v)| format!("{k}={v}")).collect();
    out.extend(["GIT_CONFIG_NOSYSTEM=1", "GIT_PAGER=cat", "PAGER=cat"].map(String::from));
    out.extend(GIT_ENV_REMOVED.iter().chain(LAUNCH_ENV_REMOVED).map(|n| format!("-{n}")));
    out
}

/// A launch of git in a pane: its command line and environment, or why it must not run. The ONE place a
/// workgroup git line is made: the verdict of the repository's own config comes first.
pub fn launch(git: &Path, vet: &Vetted, args: &[&str]) -> Result<(String, Vec<String>), String> {
    if let Some(why) = vet.launch_refusal() {
        return Err(why);
    }
    let line = crate::workgroup_run::argv_line(git, &launch_args(vet, args)?)?;
    Ok((line, launch_env()))
}

/// Whether `program` is git: its file name, without `.exe`, is `git` in any case.
pub fn is_git(program: &Path) -> bool {
    program.file_stem().is_some_and(|s| s.eq_ignore_ascii_case("git"))
}

/// THE rule for a program a workgroup starts in a pane from a template's tokens: git (named git, or a
/// name that resolves to a file that is git) runs only through `launch`, with the vetted `git`; any other
/// program as the file `resolve` finds.
pub fn pane_line(git: &Path, vet: &Vetted, tokens: &[String], resolve: &dyn Fn(&Path) -> Option<PathBuf>) -> Result<(String, Vec<String>), String> {
    let Some(program) = tokens.first() else { return Err("The diff command is empty, so it did not start.".into()) };
    let rest: Vec<&str> = tokens[1..].iter().map(String::as_str).collect();
    let found = resolve(Path::new(program));
    if is_git(Path::new(program)) || found.as_deref().is_some_and(is_git) {
        return launch(git, vet, &rest);
    }
    let found = found.ok_or("The diff program was not found, so it did not start.")?;
    Ok((crate::workgroup_run::argv_line(&found, &tokens[1..])?, Vec::new()))
}

/// The branch, counts and entries (`GCL:590-630` with the CLI). Every git line starts
/// with `--no-optional-locks` and the same `-c` hardening: no fsmonitor hook, no implicit
/// bare repository (a folder of repository files git would find on its own), paths printed
/// raw, no `ext::` transport, and no microsoft/git virtual filesystem hook.
pub const STATUS_ARGS: &[&str] = &[
    "--no-optional-locks",
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.useBuiltinFSMonitor=false",
    "-c",
    "safe.bareRepository=explicit",
    "-c",
    "core.quotepath=false",
    "-c",
    "protocol.ext.allow=never",
    "-c",
    "core.virtualFilesystem=",
    "status",
    "--porcelain=v2",
    "--branch",
    "--untracked-files=all",
    "--ignore-submodules=all",
    "--no-renames",
    "-z",
];
/// The git directory, for the operation in progress.
pub const GIT_DIR_ARGS: &[&str] = &[
    "--no-optional-locks",
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.useBuiltinFSMonitor=false",
    "-c",
    "safe.bareRepository=explicit",
    "-c",
    "core.quotepath=false",
    "-c",
    "protocol.ext.allow=never",
    "-c",
    "core.virtualFilesystem=",
    "rev-parse",
    "--absolute-git-dir",
];
/// The 8 most recently committed branches (`pidinfo.m:936-975`).
pub const BRANCHES_ARGS: &[&str] = &[
    "--no-optional-locks",
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.useBuiltinFSMonitor=false",
    "-c",
    "safe.bareRepository=explicit",
    "-c",
    "core.quotepath=false",
    "-c",
    "protocol.ext.allow=never",
    "-c",
    "core.virtualFilesystem=",
    "for-each-ref",
    "--count=8",
    "--sort=-committerdate",
    "--format=%(refname:short)",
    "refs/heads/",
];
/// The Log command (`GIT:472-505`). A repository's config cannot make it verify
/// signatures (`log.showSignature` runs its `gpg.program`).
pub const LOG_ARGS: &[&str] = &[
    "--no-optional-locks",
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.useBuiltinFSMonitor=false",
    "-c",
    "safe.bareRepository=explicit",
    "-c",
    "core.quotepath=false",
    "-c",
    "protocol.ext.allow=never",
    "-c",
    "core.virtualFilesystem=",
    "-c",
    "log.showSignature=false",
    "log",
];
/// Log's timeout (`GIT:504`) and the characters it shows before `TRUNCATED` (`GIT:494`).
pub const LOG_TIMEOUT: Duration = Duration::from_secs(5);
pub const LOG_CAP: usize = 100_000;
pub const TRUNCATED: &str = "\n[Truncated]\n";
/// Set for every status bar git run: no prompts, no optional locks, no lazy fetch.
pub const GIT_ENV: &[(&str, &str)] = &[("GIT_TERMINAL_PROMPT", "0"), ("GIT_OPTIONAL_LOCKS", "0"), ("GIT_NO_LAZY_FETCH", "1")];
/// Removed from Tandem's environment for every status bar git run: they pick another
/// repository than the one the checks looked at.
pub const GIT_ENV_REMOVED: &[&str] =
    &["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR", "GIT_OBJECT_DIRECTORY", "GIT_ALTERNATE_OBJECT_DIRECTORIES"];
/// Stdout kept from `rev-parse`, `for-each-ref` and `version`.
const SMALL_CAP: usize = 64 * 1024;
/// Stdout kept from `status`; a longer answer (about 80000 untracked files) is no state.
pub const STATUS_CAP: usize = 4 * 1024 * 1024;
/// The first git with `GIT_NO_LAZY_FETCH`.
pub const NO_LAZY_FETCH_VERSION: (u32, u32) = (2, 44);
/// The recent branches the menu offers (`GIT:361`).
pub const MENU_BRANCHES: usize = 7;

/// The git the status bar runs, from `gitSearchPath`: empty is `git` from PATH (see
/// `find_git`), else the first `;`-separated folder's `git.exe`; a folder that fails
/// `local_path_ok`, or that the judge refuses (a link to a share, a mapped drive), is no git.
pub fn git_program(search_path: &str) -> Option<PathBuf> {
    git_program_named(search_path).filter(|p| p == Path::new("git") || program_links_local(p))
}

/// `git_program` from the text alone, without looking at the disk: `git`, or the first
/// folder's `git.exe` when that folder passes `local_path_ok`. Whoever runs it checks the
/// links (`program_links_local`).
pub fn git_program_named(search_path: &str) -> Option<PathBuf> {
    let first = unquote(search_path.split(';').next().unwrap_or(""));
    if first.is_empty() {
        return Some(PathBuf::from("git"));
    }
    local_path_ok(first).then(|| exe_in(first))
}

/// The judge passes `program` (no link on the way to it leads to a network path, and no mapped or
/// SUBST drive onto a share).
pub fn program_links_local(program: &Path) -> bool {
    localfs::judged(program)
}

/// `git.exe` in the first `util_cli::path_dirs` folder that has it, with no link on the way
/// to a network path. Network and relative folders, and folders reached through a link to a
/// network path, are skipped without being looked in (a UNC entry probed on every poll would
/// leak).
pub fn find_git(path_var: &str, is_file: &dyn Fn(&LocalPath) -> bool) -> Option<PathBuf> {
    crate::util_cli::path_dirs(path_var).filter_map(|d| LocalPath::judge(d.join("git.exe"))).find(|p| is_file(p)).map(LocalPath::into_path_buf)
}

/// A PATH entry trimmed, its surrounding double quotes removed.
pub(crate) fn unquote(s: &str) -> &str {
    let s = s.trim();
    s.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(s)
}

fn exe_in(dir: &str) -> PathBuf {
    let sep = if dir.ends_with(['\\', '/']) { "" } else { "\\" };
    PathBuf::from(format!("{dir}{sep}git.exe"))
}

/// Whether git may run in `dir` at all (see `vet_repo`).
pub fn repo_path_ok(dir: &Path) -> bool {
    vet_repo(dir).is_some()
}

/// `gitTimeout` as iTerm2's helper takes it: whole seconds (`pidinfo.m:842-900`), never
/// under 1.
pub fn timeout_of(setting: f64) -> Duration {
    // `max` first: it turns NaN into 1, where `clamp` would keep it.
    Duration::from_secs(setting.trunc().max(1.0).min(i32::MAX as f64) as u64)
}

/// git's `MAX_INCLUDE_DEPTH`, also the limit on links followed.
const MAX_DEPTH: usize = 10;

/// What the checks found in a repository that may be polled.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Vetted {
    /// `-c` arguments that switch off the filter drivers the repositories' own config
    /// defines.
    pub args: Vec<String>,
    /// A partial clone (`extensions.partialClone` or a `remote.*.promisor`): git may start
    /// a lazy fetch, which only `GIT_NO_LAZY_FETCH` (git 2.44) stops.
    pub partial_clone: bool,
    /// The nearest folder with a `.git` in it: the working tree's top, which porcelain paths are
    /// relative to.
    pub root: Option<PathBuf>,
    /// The config keys (names only, never values) that run a program: a pager, an external diff or
    /// textconv, a tool, an alias, a credential helper, ssh. They change nothing for the poller, whose
    /// lines never reach them, but a git started in a pane does (`launch_refusal`).
    pub programs: Vec<String>,
}

impl Vetted {
    /// Why a git started in a pane must not run here: the repository's own config names programs
    /// (it is untrusted: the coding agent being reviewed writes it), or it is a partial clone whose
    /// lazy fetch would reach ssh or a credential helper. None when it may.
    pub fn launch_refusal(&self) -> Option<String> {
        if let Some(first) = self.programs.first() {
            let shown: Vec<String> = self.programs.iter().take(3).map(|k| crate::inert_files::display_text(k, 64, false)).collect();
            let more = if self.programs.len() > 3 { format!(" and {} more", self.programs.len() - 3) } else { String::new() };
            let _ = first;
            return Some(format!("This repository's own git configuration runs programs ({}{more}), so Tandem will not start git in it. Remove those settings from its .git/config to use it here.", shown.join(", ")));
        }
        self.partial_clone.then(|| "This repository is a partial clone, so git may fetch from a remote while it runs; Tandem will not start git in it.".to_string())
    }
}

/// What the worker found for a folder: the vet verdict and the git it would run. A launch in a pane takes
/// only this (the UI thread never vets or searches).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Launch {
    pub vet: Option<Vetted>,
    pub git: Option<PathBuf>,
}

/// Why a git started in a pane must not run for this verdict (Run Anyway and every launch): None when it may.
pub fn launch_verdict(l: Option<&Launch>) -> Option<String> {
    let Some(l) = l else { return Some("Tandem has not finished checking this folder yet.".into()) };
    let Some(vet) = &l.vet else { return Some("git will not run in this folder (its configuration is not safe to read).".into()) };
    if vet.root.is_none() {
        return Some("This folder is not in a git repository, so git will not run.".into());
    }
    if l.git.is_none() {
        return Some("git was not found, so the diff did not start.".into());
    }
    vet.launch_refusal()
}

/// The checks before any git run, in `dir` and every repository git could find above it
/// (see the module comment). `None` refuses: git does not run and the component is empty,
/// as for a folder outside any repository.
pub fn vet_repo(dir: &Path) -> Option<Vetted> {
    if !local_path_ok(dir.to_str()?) {
        return None;
    }
    let dir = LocalPath::judge(dir)?;
    let mut filters = BTreeSet::new();
    let mut partial_clone = false;
    let mut programs: Vec<String> = Vec::new();
    let mut v = Vet { filters: &mut filters, partial_clone: &mut partial_clone, programs: &mut programs };
    let mut root = None;
    let mut at = Some(dir);
    while let Some(a) = at {
        // Each name is judged (its link read, the target walked) before it is looked at.
        let dotgit = a.join(".git")?;
        if localfs::lstat(&dotgit).is_ok() {
            root.get_or_insert_with(|| a.path().to_path_buf());
            let kind = localfs::stat(&dotgit).ok();
            if kind.is_some_and(|k| k.is_dir) {
                vet_git_dir(&dotgit, a.path(), &mut v)?;
            } else if kind.is_some_and(|k| k.is_file) {
                vet_git_dir(&gitdir_target(&dotgit)?, a.path(), &mut v)?;
            }
        }
        // A bare repository here: `safe.bareRepository=explicit` refuses it, but git reads
        // its config first.
        let (head, config) = (a.join("HEAD")?, a.join("config")?);
        if localfs::is_file(&head) && localfs::is_file(&config) {
            vet_git_dir(&a, a.path(), &mut v)?;
        }
        at = a.parent();
    }
    programs.sort();
    programs.dedup();
    Some(Vetted { args: filter_args(&filters), partial_clone, root, programs })
}

/// What `vet_config` collects.
struct Vet<'a> {
    filters: &'a mut BTreeSet<String>,
    partial_clone: &'a mut bool,
    programs: &'a mut Vec<String>,
}

/// A `.git` file's `gitdir:` (a worktree or submodule), when it is relative or local.
fn gitdir_target(dotgit: &LocalPath) -> Option<LocalPath> {
    let text = read_small(dotgit)?;
    let p = text.lines().next()?.strip_prefix("gitdir:")?.trim();
    local_target(dotgit.parent()?.path(), p)
}

/// `p` resolved against `base` when relative; an absolute `p` must be a drive path. Either way
/// the judge passes it (no link on the way leaves this computer).
fn local_target(base: &Path, p: &str) -> Option<LocalPath> {
    if p.is_empty() || network_path(p) {
        return None;
    }
    let path = if is_relative(p) {
        base.join(p)
    } else if local_path_ok(p) {
        PathBuf::from(p)
    } else {
        return None;
    };
    LocalPath::judge(path)
}

/// Relative on Windows: not rooted (`\x`, `/x`, `\\host`) and no drive (`C:x`).
fn is_relative(p: &str) -> bool {
    !p.starts_with(['\\', '/']) && p.as_bytes().get(1) != Some(&b':')
}

/// The folders whose entries git opens without the checks seeing each path: every entry
/// of these must be a plain file or folder, or a link to a local path. Loose objects and
/// nested refs are not listed (thousands of entries a poll); a link there still needs a
/// hostile writer inside an already checked folder.
const LISTED: &[&str] = &["", "objects", "objects/info", "objects/pack", "info", "refs", "refs/heads"];

/// Every entry of `dir` is not a link, or a link to a local path. A folder that is not
/// there has nothing to check.
fn entries_local(dir: Option<LocalPath>) -> bool {
    let Some(dir) = dir else { return false };
    let Ok(entries) = localfs::entries(&dir) else { return true };
    entries.iter().all(|e| e.local().is_some())
}

/// A git directory's own files: no link inside it to a network path, its config (and a
/// worktree's), its common directory's, and the object alternates. `root` is the working
/// tree folder git would run in, for relative config paths.
fn vet_git_dir(git_dir: &LocalPath, root: &Path, v: &mut Vet) -> Option<()> {
    let mut dirs = vec![git_dir.clone()];
    if !entries_local(Some(git_dir.clone())) {
        return None;
    }
    if let Some(text) = read_optional(&git_dir.join("commondir")?)? {
        dirs.push(local_target(git_dir.path(), text.trim())?);
    }
    for d in &dirs {
        if !LISTED.iter().all(|sub| entries_local(d.join(sub))) {
            return None;
        }
        vet_config(&d.join("config")?, root, v, 0)?;
        vet_config(&d.join("config.worktree")?, root, v, 0)?;
        let objects = d.join("objects")?;
        if let Some(text) = read_optional(&objects.join("info")?.join("alternates")?)? {
            for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
                local_target(objects.path(), line.trim_matches('"'))?;
            }
        }
    }
    Some(())
}

/// A path value from a config file, as git resolves it: `~/` is the user's folder, a
/// relative path is against `base`. `%(prefix)` and `~user` are refused.
fn config_path(v: &str, base: &Path) -> Option<LocalPath> {
    if let Some(rest) = v.strip_prefix("~/").or_else(|| v.strip_prefix("~\\")) {
        let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).ok()?;
        local_target(Path::new(&home), rest)
    } else if v.starts_with('~') || v.starts_with("%(") {
        None
    } else {
        local_target(base, v)
    }
}

/// One config file: no value that names a network path, path-valued keys with no link to
/// one, filter driver names `-c` can carry, no git-lfs extension (the trusted global lfs
/// filter would run its command), whether it is a partial clone, and each included file
/// checked the same way.
fn vet_config(file: &LocalPath, root: &Path, v: &mut Vet, depth: usize) -> Option<()> {
    if depth > MAX_DEPTH {
        return None;
    }
    let Some(text) = read_optional(file)? else { return Some(()) };
    let parent = file.parent()?;
    let here = parent.path();
    for e in config_entries(&text)? {
        let val = e.value.trim();
        if network_path(val) {
            return None;
        }
        let sub = e.sub.as_deref().unwrap_or("");
        note_program(&e.section, e.sub.as_deref(), &e.key, v.programs);
        match (e.section.as_str(), e.key.as_str()) {
            ("filter", _) => {
                if sub.is_empty() || sub.contains(['=', '\n', '\r', '\0']) {
                    return None;
                }
                v.filters.insert(sub.to_string());
            }
            ("lfs", _) if sub.to_ascii_lowercase().starts_with("extension") => return None,
            ("extensions", "partialclone") | ("remote", "promisor") => *v.partial_clone = true,
            ("core", "attributesfile" | "excludesfile" | "hookspath") if !val.is_empty() => {
                config_path(val, root)?;
            }
            ("core", "worktree") if !val.is_empty() => {
                config_path(val, here)?;
            }
            ("include" | "includeif", "path") => {
                let target = config_path(val, here)?;
                vet_config(&target, root, v, depth + 1)?;
            }
            _ => {}
        }
    }
    Some(())
}

/// Records the key when it names a program git would run in a pane (not by the poller's lines).
/// Section and key are lowercased by the parser. Filter drivers are not listed: `-c` switches
/// them off for every line.
fn note_program(section: &str, sub: Option<&str>, key: &str, out: &mut Vec<String>) {
    let runs = match section {
        "core" => matches!(key, "pager" | "sshcommand" | "fsmonitor" | "editor" | "askpass"),
        "pager" | "alias" | "difftool" | "mergetool" | "credential" | "uploadpack" | "receive" | "web" | "browser" | "man" | "gpg" | "gui" => true,
        "sequence" => key == "editor",
        "diff" => matches!(key, "external" | "tool" | "guitool") || (sub.is_some() && matches!(key, "command" | "textconv")),
        "merge" => sub.is_some() && key == "driver" || key == "tool",
        "http" => matches!(key, "proxy" | "sslcommand"),
        _ => false,
    };
    if runs {
        let name = match sub {
            Some(s) => format!("{section}.{s}.{key}"),
            None => format!("{section}.{key}"),
        };
        if !out.contains(&name) {
            out.push(name);
        }
    }
}

/// `-c` overrides that switch off the named filter drivers (an empty command runs nothing;
/// verified with git 2.43).
fn filter_args(names: &BTreeSet<String>) -> Vec<String> {
    let mut out = Vec::new();
    for n in names {
        for key in ["clean=", "smudge=", "process=", "required=false"] {
            out.push("-c".to_string());
            out.push(format!("filter.{n}.{key}"));
        }
    }
    out
}

/// A small text file's contents; `None` when it cannot be read.
fn read_small(p: &LocalPath) -> Option<String> {
    let bytes = localfs::read(p).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// `Some(None)` when the file is not there, `None` when it is there but unreadable.
fn read_optional(p: &LocalPath) -> Option<Option<String>> {
    match localfs::read(p) {
        Ok(bytes) => Some(Some(String::from_utf8_lossy(&bytes).into_owned())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(None),
        Err(_) => None,
    }
}

/// One `key = value` from a git config file, section and key lowercased.
struct ConfigEntry {
    section: String,
    sub: Option<String>,
    key: String,
    value: String,
}

/// git's config file syntax, enough to see every value: `[section]`, `[section "sub"]`,
/// `[section.sub]`, comments, quotes, escapes, and continued lines. `None` for a file git
/// would refuse to read.
fn config_entries(text: &str) -> Option<Vec<ConfigEntry>> {
    let c: Vec<char> = text.trim_start_matches('\u{FEFF}').chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    let (mut section, mut sub) = (String::new(), None);
    let to_eol = |i: &mut usize| {
        while *i < c.len() && c[*i] != '\n' {
            *i += 1;
        }
    };
    while i < c.len() {
        match c[i] {
            ch if ch.is_whitespace() => i += 1,
            '#' | ';' => to_eol(&mut i),
            '[' => {
                i += 1;
                let start = i;
                while i < c.len() && !matches!(c[i], ']' | '"' | ' ' | '\t' | '\n') {
                    i += 1;
                }
                let name: String = c[start..i].iter().collect();
                while i < c.len() && matches!(c[i], ' ' | '\t') {
                    i += 1;
                }
                if c.get(i) == Some(&'"') {
                    i += 1;
                    let mut s = String::new();
                    loop {
                        match c.get(i)? {
                            '"' => break,
                            '\n' => return None,
                            '\\' => {
                                i += 1;
                                s.push(*c.get(i)?);
                            }
                            ch => s.push(*ch),
                        }
                        i += 1;
                    }
                    i += 1;
                    section = name.to_ascii_lowercase();
                    sub = Some(s);
                } else if let Some((s, rest)) = name.split_once('.') {
                    section = s.to_ascii_lowercase();
                    sub = Some(rest.to_ascii_lowercase());
                } else {
                    section = name.to_ascii_lowercase();
                    sub = None;
                }
                if c.get(i) != Some(&']') {
                    return None;
                }
                i += 1;
            }
            ch if ch.is_ascii_alphabetic() => {
                let start = i;
                while i < c.len() && (c[i].is_ascii_alphanumeric() || c[i] == '-') {
                    i += 1;
                }
                let key = c[start..i].iter().collect::<String>().to_ascii_lowercase();
                while i < c.len() && matches!(c[i], ' ' | '\t') {
                    i += 1;
                }
                let mut value = String::new();
                if c.get(i) == Some(&'=') {
                    i += 1;
                    let mut quoted = false;
                    while i < c.len() {
                        match c[i] {
                            '\n' if !quoted => break,
                            '\n' => return None,
                            '"' => quoted = !quoted,
                            '#' | ';' if !quoted => {
                                to_eol(&mut i);
                                break;
                            }
                            '\\' => {
                                i += 1;
                                match c.get(i)? {
                                    '\n' => {}
                                    '\r' if c.get(i + 1) == Some(&'\n') => i += 1,
                                    'n' => value.push('\n'),
                                    't' => value.push('\t'),
                                    'b' => value.push('\u{8}'),
                                    ch @ ('\\' | '"') => value.push(*ch),
                                    _ => return None,
                                }
                            }
                            ch => value.push(ch),
                        }
                        i += 1;
                    }
                    if quoted {
                        return None;
                    }
                } else if !matches!(c.get(i), None | Some('\n' | '\r' | '#' | ';')) {
                    return None;
                }
                out.push(ConfigEntry { section: section.clone(), sub: sub.clone(), key, value });
            }
            _ => return None,
        }
    }
    Some(out)
}

/// `base` with `extra` inserted after its first argument (before the subcommand).
fn with_extra(base: &[&str], extra: &[String]) -> Vec<String> {
    let mut v: Vec<String> = base.iter().map(|s| s.to_string()).collect();
    v.splice(1..1, extra.iter().cloned());
    v
}

/// One poll's answer: `state` is `None` outside a repository, on an unborn branch, or on a
/// timeout, which `timed_out` tells apart.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Fetched {
    pub state: Option<GitState>,
    pub timed_out: bool,
}

/// The state of the repository around `dir`: the checks, `status`, then `rev-parse` for the
/// git directory, all inside `timeout` (iTerm2's helper kills itself at `gitTimeout`).
/// Exit 128 (not a repository, or `safe.directory` refused it) is no state, not a timeout.
pub fn fetch_state(run: &dyn GitRun, git: &Path, dir: &Path, timeout: Duration) -> Fetched {
    fetch_state_with(run, git, dir, timeout, None, false)
}

/// `fetch_state`, and with `stats` also what a workgroup's poller asks for (`includeDiffStats`): the
/// per-file statuses against HEAD, or against `base` when it resolves to a tree (an unresolved or
/// refused base keeps the HEAD list), then the two diff stats runs. All inside the one timeout and the
/// same checks and hardening; an extra run that fails, is capped or runs out of time leaves its field
/// unset and the rest of the state stands.
pub fn fetch_state_with(run: &dyn GitRun, git: &Path, dir: &Path, timeout: Duration, base: Option<&str>, stats: bool) -> Fetched {
    let start = Instant::now();
    let left = || timeout.saturating_sub(start.elapsed());
    let none = Fetched::default();
    let timed_out = Fetched { state: None, timed_out: true };
    let Some(vet) = vet_repo(dir) else { return none };
    match lazy_fetch_safe(run, git, dir, &vet, left()) {
        Some(true) => {}
        Some(false) => return none,
        None => return timed_out,
    }
    if left().is_zero() {
        return timed_out;
    }
    let mut state = match run.run(git, dir, &with_extra(STATUS_ARGS, &vet.args), left(), Some(STATUS_CAP)) {
        RunResult::Exit { code: 0, stdout, capped: false } => match parse_porcelain(&stdout) {
            Some(s) => s,
            None => return none,
        },
        RunResult::TimedOut => return timed_out,
        _ => return none,
    };
    if left().is_zero() {
        return timed_out;
    }
    match run.run(git, dir, &with_extra(GIT_DIR_ARGS, &vet.args), left(), Some(SMALL_CAP)) {
        RunResult::Exit { code: 0, stdout, capped: false } => {
            let d = String::from_utf8_lossy(&stdout).trim().to_string();
            if let Some(d) = Some(d).filter(|d| local_path_ok(d)).and_then(LocalPath::judge) {
                state.repo_state = repo_state(&d);
            }
        }
        RunResult::TimedOut => return timed_out,
        _ => {}
    }
    if stats {
        extra_runs(run, git, dir, &vet, &mut state, base, &left);
    }
    Fetched { state: Some(state), timed_out: false }
}

/// The files pass and the stats runs of `fetch_state_with`.
fn extra_runs(run: &dyn GitRun, git: &Path, dir: &Path, vet: &Vetted, state: &mut GitState, base: Option<&str>, left: &dyn Fn() -> Duration) {
    let exit0 = |r: RunResult| match r {
        RunResult::Exit { code: 0, stdout, capped: false } => Some(stdout),
        _ => None,
    };
    let base = base.map(str::trim).filter(|b| !b.is_empty() && *b != "HEAD" && !b.starts_with('-') && b.chars().count() <= 256 && !b.chars().any(char::is_control));
    let mut statuses = None;
    if let Some(b) = base {
        if !left().is_zero() {
            let mut args = with_extra(BASE_TREE_ARGS, &vet.args);
            args.push(format!("{b}^{{tree}}"));
            let tree = exit0(run.run(git, dir, &args, left(), Some(SMALL_CAP))).map(|o| String::from_utf8_lossy(&o).trim().to_string());
            if let Some(tree) = tree.filter(|t| (40..=64).contains(&t.len()) && t.bytes().all(|c| c.is_ascii_hexdigit())) {
                let mut args = with_extra(BASE_DIFF_ARGS, &vet.args);
                args.extend([tree, "--".to_string()]);
                statuses = exit0(run.run(git, dir, &args, left(), Some(STATUS_CAP))).map(|o| parse_base_diff(&o));
            }
        }
    }
    if statuses.is_none() && !left().is_zero() {
        statuses = exit0(run.run(git, dir, &with_extra(FILES_ARGS, &vet.args), left(), Some(STATUS_CAP))).map(|o| parse_file_statuses(&o));
    }
    state.file_statuses = statuses;
    if left().is_zero() {
        return;
    }
    let name_status = exit0(run.run(git, dir, &with_extra(NAME_STATUS_ARGS, &vet.args), left(), Some(STATUS_CAP)));
    if left().is_zero() {
        return;
    }
    let numstat = exit0(run.run(git, dir, &with_extra(NUMSTAT_ARGS, &vet.args), left(), Some(STATUS_CAP)));
    if let (Some(n), Some(m)) = (name_status, numstat) {
        (state.files_added, state.files_modified, state.files_deleted, state.lines_inserted, state.lines_deleted) = diff_stats(&n, &m, state.adds);
    }
}

/// The 8 most recently committed branches, newest first; none when git may not run or
/// fails.
pub fn recent_branches(run: &dyn GitRun, git: &Path, dir: &Path, timeout: Duration) -> Vec<String> {
    let start = Instant::now();
    let Some(vet) = vet_repo(dir) else { return Vec::new() };
    if lazy_fetch_safe(run, git, dir, &vet, timeout) != Some(true) {
        return Vec::new();
    }
    let left = timeout.saturating_sub(start.elapsed());
    match run.run(git, dir, &with_extra(BRANCHES_ARGS, &vet.args), left, Some(SMALL_CAP)) {
        RunResult::Exit { code: 0, stdout, .. } => parse_branches(&stdout),
        _ => Vec::new(),
    }
}

/// The Log command's output (`GIT:472-505`): `git log` in `dir` with the same checks and
/// hardening as a poll; past `LOG_CAP` git is stopped and `TRUNCATED` appended. Empty when
/// git may not run, cannot start, or times out.
pub fn git_log(run: &dyn GitRun, git: &Path, dir: &Path, timeout: Duration) -> String {
    let start = Instant::now();
    let Some(vet) = vet_repo(dir) else { return String::new() };
    if lazy_fetch_safe(run, git, dir, &vet, timeout) != Some(true) {
        return String::new();
    }
    let left = timeout.saturating_sub(start.elapsed());
    match run.run(git, dir, &with_extra(LOG_ARGS, &vet.args), left, Some(LOG_CAP)) {
        RunResult::Exit { stdout, capped, .. } => {
            let mut text = String::from_utf8_lossy(&stdout).into_owned();
            if capped {
                text.push_str(TRUNCATED);
            }
            text
        }
        _ => String::new(),
    }
}

/// Whether git cannot start a lazy fetch here: not a partial clone, or a git that honors
/// `GIT_NO_LAZY_FETCH`. An older git, or one whose version cannot be read, refuses the
/// repository (the fetch would run its remote's transport, `uploadpack` or
/// `core.sshCommand`). `None` when asking for the version timed out.
fn lazy_fetch_safe(run: &dyn GitRun, git: &Path, dir: &Path, vet: &Vetted, timeout: Duration) -> Option<bool> {
    if !vet.partial_clone {
        return Some(true);
    }
    match git_version(run, git, dir, timeout) {
        Ok(v) => Some(v >= NO_LAZY_FETCH_VERSION),
        Err(true) => None,
        Err(false) => Some(false),
    }
}

/// `git version`'s major and minor, asked once per git program and remembered. `Err(true)`
/// when it timed out (not remembered, so a later poll asks again).
fn git_version(run: &dyn GitRun, git: &Path, dir: &Path, timeout: Duration) -> Result<(u32, u32), bool> {
    static KNOWN: std::sync::OnceLock<std::sync::Mutex<HashMap<PathBuf, Option<(u32, u32)>>>> = std::sync::OnceLock::new();
    let known = KNOWN.get_or_init(Default::default);
    if let Some(v) = known.lock().unwrap_or_else(|e| e.into_inner()).get(git) {
        return v.ok_or(false);
    }
    let v = match run.run(git, dir, &["version".to_string()], timeout, Some(SMALL_CAP)) {
        RunResult::Exit { code: 0, stdout, .. } => parse_git_version(&stdout),
        RunResult::TimedOut => return Err(true),
        _ => None,
    };
    known.lock().unwrap_or_else(|e| e.into_inner()).insert(git.to_path_buf(), v);
    v.ok_or(false)
}

/// `git version 2.43.0.windows.1` as `(2, 43)`.
pub fn parse_git_version(stdout: &[u8]) -> Option<(u32, u32)> {
    let text = String::from_utf8_lossy(stdout);
    let v = text.strip_prefix("git version ")?.split_whitespace().next()?;
    let mut parts = v.split('.');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

/// `GPW`'s cache TTL.
pub const CACHE_TTL: Duration = Duration::from_secs(1);

/// iTerm2's poll worker (`GPW:94-201`), basic key only: a state per path for 1 s, and the
/// requests waiting on the one git run in flight per path.
#[derive(Debug, Default)]
pub struct WorkerCache {
    /// The state and when it was made.
    cache: HashMap<String, (GitState, Instant)>,
    /// Requests waiting on the run in flight.
    pending: HashMap<String, usize>,
}

/// What a request for a path does.
#[derive(Clone, Debug, PartialEq)]
pub enum Lookup {
    /// Answered from the cache at once.
    Cached(Option<GitState>),
    /// A run for this path is in flight; its answer serves this request too.
    Joined,
    /// Start a run, then `finish`.
    Start,
}

impl WorkerCache {
    pub fn lookup(&mut self, path: &str, now: Instant) -> Lookup {
        if let Some((s, made)) = self.cache.get(path) {
            if now.saturating_duration_since(*made) < CACHE_TTL {
                return Lookup::Cached(Some(s.clone()));
            }
        }
        match self.pending.get_mut(path) {
            Some(n) if *n > 0 => {
                *n += 1;
                Lookup::Joined
            }
            _ => {
                self.pending.insert(path.to_string(), 1);
                Lookup::Start
            }
        }
    }

    /// A run's answer, made at `now` (the worker records when its git finished). A cached
    /// state newer than the answer wins but keeps the answer's timeout; an answer with no
    /// state counts as oldest, as a nil state's creation time is 0 in iTerm2 (so a folder
    /// that stops being a repository shows its last state until `invalidate`, kept).
    pub fn finish(&mut self, path: &str, fetched: Fetched, now: Instant) -> Fetched {
        self.pending.remove(path);
        if let Some((cached, made)) = self.cache.get(path) {
            if fetched.state.is_none() || *made > now {
                return Fetched { state: Some(cached.clone()), timed_out: fetched.timed_out };
            }
        }
        if let Some(s) = &fetched.state {
            self.cache.insert(path.to_string(), (s.clone(), now));
        }
        fetched
    }

    /// Forgets the path's cached state and waiting requests (`GPW:161-199`).
    pub fn invalidate(&mut self, path: &str) {
        // Every key made from the path: bare, with stats, with a base (`cache_key`).
        let of_path = |k: &String| k == path || k.strip_prefix(path).is_some_and(|rest| rest.starts_with(['\x01', '\x02']));
        self.cache.retain(|k, _| !of_path(k));
        self.pending.retain(|k, _| !of_path(k));
    }

    /// `debugInfoForDirectory:` (`GPW:74-85`); Tandem has no rich cache.
    pub fn debug_info(&self, path: &str, now: Instant) -> String {
        let basic = self.cache.get(path).map_or_else(|| "none".to_string(), |(_, made)| format!("age {}", now.saturating_duration_since(*made).as_secs_f64()));
        format!("Basic cache: {basic}\nRich cache: none\nPending calls: {}\n", self.pending.get(path).copied().unwrap_or(0))
    }
}

/// What `gitPollerShouldPoll:after:` reads from the session (`GIT:636-667`).
#[derive(Clone, Copy, Debug, Default)]
pub struct PollGate {
    /// The component is on screen.
    pub visible: bool,
    /// When a newline was last sent to the session.
    pub last_newline: Option<Instant>,
    /// When the session last printed.
    pub last_activity: Option<Instant>,
}

/// The newline grace: git takes a moment to update after Enter (`GIT:651-653`).
const NEWLINE_GRACE: Duration = Duration::from_secs(3);
/// `pollIntervalWhenInactive` (`GIT:659`).
const INACTIVE_POLL: Duration = Duration::from_secs(60);
/// The directory-change cache invalidation's rate limit (`GP:26-27`).
const INVALIDATE_EVERY: Duration = Duration::from_millis(500);

/// `gitPollerShouldPoll:after:` (`GIT:636-667`): never off screen; always the first time;
/// then when a newline went out after `last_poll - 3 s`, or when over 60 s passed and the
/// session printed since.
pub fn should_poll(last_poll: Option<Instant>, gate: &PollGate, now: Instant) -> bool {
    if !gate.visible {
        return false;
    }
    let Some(last) = last_poll else { return true };
    if gate.last_newline.is_some_and(|nl| nl + NEWLINE_GRACE > last) {
        return true;
    }
    now.saturating_duration_since(last) > INACTIVE_POLL && gate.last_activity.is_some_and(|a| a > last)
}

/// iTerm2's `iTermGitPoller` (`GP`) as a state machine: the caller sends `Polls.request`
/// to the worker, invalidates `Polls.invalidate` in its `WorkerCache`, hands each answer to
/// `did_poll`, and calls `tick` at `next_due`.
#[derive(Clone, Debug)]
pub struct GitPoller {
    pub enabled: bool,
    pub directory: Option<String>,
    pub state: Option<GitState>,
    pub last_poll: Option<Instant>,
    /// This directory has answered without a timeout (`hasSuccessfullyFetched`).
    pub has_fetched: bool,
    /// The last poll timed out (`lastPollTimedOut`).
    pub timed_out: bool,
    /// The Polling Interval knob, in seconds.
    pub cadence: f64,
    /// The running timer's interval: `cadence` when it was last started.
    interval: Duration,
    next_fire: Instant,
    /// The rate limiter's window end, and the invalidation waiting for it.
    limiter_until: Option<Instant>,
    limiter_block: Option<String>,
}

/// What the poller asks for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Polls {
    /// Poll this path.
    pub request: Option<String>,
    /// Invalidate the worker cache for this path.
    pub invalidate: Option<String>,
}

/// The knob as a cadence: 0 reads as 1 (`GIT:245-247`). iTerm2 has no other floor (only
/// NSTimer's 0.1 ms); Tandem floors at 1 s (controller ruling), so a tiny or non-number
/// value cannot make the timer spin on the UI thread.
fn cadence_of(knob: f64) -> f64 {
    if knob.is_finite() && knob > 0.0 { knob.clamp(MIN_CADENCE, 1e9) } else { 1.0 }
}

/// The shortest polling interval, in seconds.
const MIN_CADENCE: f64 = 1.0;

impl GitPoller {
    /// Disabled until `set_enabled`, the timer started at `cadence` (`GP:24-34`).
    pub fn new(cadence: f64, now: Instant) -> Self {
        let cadence = cadence_of(cadence);
        let interval = Duration::from_secs_f64(cadence);
        GitPoller {
            enabled: false,
            directory: None,
            state: None,
            last_poll: None,
            has_fetched: false,
            timed_out: false,
            cadence,
            interval,
            next_fire: now + interval,
            limiter_until: None,
            limiter_block: None,
        }
    }

    /// The knob changed: only the cadence; the running timer keeps its interval until the
    /// next `bump` (iTerm2's, kept).
    pub fn set_cadence(&mut self, knob: f64) {
        self.cadence = cadence_of(knob);
    }

    /// True when the component must redraw: disabling clears the display (`GP:47-56`).
    pub fn set_enabled(&mut self, on: bool) -> bool {
        if on == self.enabled {
            return false;
        }
        self.enabled = on;
        !on
    }

    /// The session's directory changed (`GP:170-194`): the per-directory flags reset, the
    /// cache for the new path is invalidated through a 0.5 s rate limiter, and it polls
    /// without restarting the timer.
    pub fn set_directory(&mut self, dir: Option<String>, gate: &PollGate, now: Instant) -> Polls {
        if dir == self.directory {
            return Polls::default();
        }
        let mut invalidate = self.fire_limiter(now);
        if let Some(d) = &dir {
            if self.limiter_until.is_none() {
                invalidate = Some(d.clone());
                self.limiter_until = Some(now + INVALIDATE_EVERY);
            } else {
                self.limiter_block = Some(d.clone());
            }
        }
        self.directory = dir;
        self.has_fetched = false;
        self.timed_out = false;
        Polls { request: self.poll(gate, now), invalidate }
    }

    /// Poll now, then restart the timer at the current cadence (`GP:67-74`).
    pub fn bump(&mut self, gate: &PollGate, now: Instant) -> Polls {
        let request = self.poll(gate, now);
        self.interval = Duration::from_secs_f64(self.cadence);
        self.next_fire = now + self.interval;
        Polls { request, invalidate: None }
    }

    /// The timer and the rate limiter, when due.
    pub fn tick(&mut self, gate: &PollGate, now: Instant) -> Polls {
        let invalidate = self.fire_limiter(now);
        let mut request = None;
        if now >= self.next_fire {
            // The first fire time after `now` on the timer's grid, in one step however late
            // the tick is.
            let every = self.interval.as_nanos().max(1);
            let late = now.duration_since(self.next_fire).as_nanos() % every;
            self.next_fire = now + Duration::from_nanos((every - late) as u64);
            request = self.poll(gate, now);
        }
        Polls { request, invalidate }
    }

    /// An answer for `path` (`GP:112-139`). One for another directory than the current one
    /// is dropped (false); otherwise true, and the component redraws.
    pub fn did_poll(&mut self, path: &str, fetched: Fetched) -> bool {
        if self.directory.as_deref() != Some(path) {
            return false;
        }
        if !fetched.timed_out {
            self.has_fetched = true;
        }
        self.timed_out = fetched.timed_out;
        self.state = fetched.state;
        true
    }

    /// After the timeout was raised (`GP:77-84`): the warning goes, and it bumps.
    pub fn clear_timeout_and_retry(&mut self, gate: &PollGate, now: Instant) -> Polls {
        self.timed_out = false;
        self.bump(gate, now)
    }

    /// When `tick` next has work: the timer while polling can happen, and a waiting
    /// invalidation.
    pub fn next_due(&self) -> Option<Instant> {
        let timer = (self.enabled && self.directory.as_deref().is_some_and(|d| !d.is_empty())).then_some(self.next_fire);
        let limiter = self.limiter_block.as_ref().and(self.limiter_until);
        match (timer, limiter) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// `poll` (`GP:86-110`).
    fn poll(&mut self, gate: &PollGate, now: Instant) -> Option<String> {
        if !self.enabled {
            return None;
        }
        let dir = self.directory.clone().filter(|d| !d.is_empty())?;
        if !should_poll(self.last_poll, gate, now) {
            return None;
        }
        self.last_poll = Some(now);
        Some(dir)
    }

    /// iTerm2's rate limiter timer: at the window's end, the waiting block runs and opens a
    /// new window; with none waiting the window just closes.
    fn fire_limiter(&mut self, now: Instant) -> Option<String> {
        let until = self.limiter_until?;
        if now < until {
            return None;
        }
        self.limiter_until = None;
        let block = self.limiter_block.take()?;
        self.limiter_until = Some(now + INVALIDATE_EVERY);
        Some(block)
    }
}

/// NSString's `integerValue`: leading whitespace, a sign, then digits; 0 without digits.
fn ns_integer(s: &str) -> i64 {
    let s = s.trim_start();
    let (neg, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let mut n: i64 = 0;
    for b in digits.bytes().take_while(u8::is_ascii_digit) {
        n = n.saturating_mul(10).saturating_add((b - b'0') as i64);
    }
    if neg { -n } else { n }
}

/// NSString's `boolValue`: after whitespace, signs and zeros, `Y`, `y`, `T`, `t` or 1-9.
fn ns_bool(s: &str) -> bool {
    let rest = s.trim_start().trim_start_matches(['+', '-']).trim_start_matches('0');
    rest.chars().next().is_some_and(|c| matches!(c, 'Y' | 'y' | 'T' | 't' | '1'..='9'))
}

/// A remote host's state from its `user.git*` variables (`GSMA:34-59`), keys without the
/// `user.` prefix: all six required ones must be set.
pub fn remote_state(user: &BTreeMap<String, String>) -> Option<GitState> {
    let get = |k: &str| user.get(k).map(String::as_str);
    let opt = |k: &str| get(k).map_or(0, ns_integer);
    let dirty = get("gitDirty")?;
    Some(GitState {
        branch: get("gitBranch")?.to_string(),
        ahead: get("gitPushCount")?.to_string(),
        behind: get("gitPullCount")?.to_string(),
        dirty: match dirty {
            "dirty" => true,
            "clean" => false,
            d => ns_bool(d),
        },
        adds: ns_integer(get("gitAdds")?),
        deletes: ns_integer(get("gitDeletes")?),
        lines_inserted: opt("gitLinesInserted"),
        lines_deleted: opt("gitLinesDeleted"),
        files_added: opt("gitFilesAdded"),
        files_modified: opt("gitFilesModified"),
        files_deleted: opt("gitFilesDeleted"),
        repo_state: RepoState::None,
        file_statuses: None,
    })
}

/// `bumpIfLastCommandWasGit` (`GIT:267-282`): the first word (a space must follow it) is
/// `git` after the last `/`. Tandem also splits on `\` and drops `.exe`, ignoring case.
pub fn looks_like_git(command: &str) -> bool {
    let Some((first, _)) = command.split_once(' ') else { return false };
    let base = first.rsplit(['/', '\\']).next().unwrap_or(first).to_ascii_lowercase();
    base.strip_suffix(".exe").unwrap_or(&base) == "git"
}

/// The timeout warning's variants (`GIT:153-160`), the long one first; U+26A0 without the
/// emoji selector.
pub const TIMED_OUT_SHORT: &str = "\u{26A0} timed out";
pub const TIMED_OUT_ICON: &str = "\u{26A0}";

/// `attributedStringValueForBranch:` (`GSM:48-178`). `ready` is iTerm2's `pollerReady`
/// (a state and the poller enabled); `branch` is the variant being rendered.
pub fn git_text(state: Option<&GitState>, status: Option<&str>, ready: bool, branch: &str) -> Option<String> {
    if let Some(s) = status {
        return Some(s.to_string());
    }
    let st = state.filter(|_| ready)?;
    if let Some(word) = st.repo_state.text() {
        return Some(word.to_string());
    }
    if branch.is_empty() {
        return None;
    }
    let rich = st.files_added != 0 || st.files_modified != 0 || st.files_deleted != 0 || st.lines_inserted != 0 || st.lines_deleted != 0;
    if rich {
        let pair = |add: i64, del: i64, what: &str| match (add > 0, del > 0) {
            (false, false) => None,
            (false, true) => Some(format!("-{del} {what}")),
            (true, false) => Some(format!("+{add} {what}")),
            (true, true) => Some(format!("+{add}/-{del} {what}")),
        };
        let parts: Vec<String> = [pair(st.files_added, st.files_deleted, "files"), pair(st.lines_inserted, st.lines_deleted, "lines")].into_iter().flatten().collect();
        return Some(parts.join(" "));
    }
    let mut out = branch.to_string();
    if st.adds != 0 && st.deletes != 0 {
        out.push_str("\u{2009}\u{00B1}");
    } else {
        if st.adds != 0 {
            out.push_str("\u{2009}+");
        }
        if st.deletes != 0 {
            out.push_str("\u{2009}-");
        }
    }
    if st.dirty {
        out.push_str("\u{2009}\u{25CF}");
    }
    if ns_integer(&st.ahead) > 0 {
        out.push_str("\u{2002}\u{2191}");
        out.push_str(&st.ahead);
    }
    if ns_integer(&st.behind) > 0 {
        out.push_str("\u{2002}\u{2193}");
        out.push_str(&st.behind);
    }
    Some(out)
}

/// `attributedStringVariants` (`GSM:292-302`): the one branch's string, or `[""]`.
pub fn git_variants(state: Option<&GitState>, status: Option<&str>, ready: bool) -> Vec<String> {
    let text = state.filter(|s| !s.branch.is_empty()).and_then(|s| git_text(Some(s), status, ready, &s.branch));
    vec![text.unwrap_or_default()]
}

/// `shouldShowTimeoutError` (`GIT:178-190`).
pub fn show_timeout(local: bool, enabled: bool, has_fetched: bool, timed_out: bool) -> bool {
    local && enabled && !has_fetched && timed_out
}

/// The timeout the warning offers (`GIT:194`).
pub fn proposed_timeout(t: f64) -> f64 {
    (t * 2.0).max(t + 2.0)
}

/// `formatTimeoutSeconds:` (`GIT:214-219`).
pub fn format_timeout(s: f64) -> String {
    if (s - s.round()).abs() < 0.01 { format!("{}", s.round() as i64) } else { format!("{s:.1}") }
}

/// The warning's accept button (`GIT:202`).
pub fn increase_button(proposed: f64) -> String {
    format!("Increase to {}s", format_timeout(proposed))
}

/// The timeout warning's text (`GIT:195-201`).
pub fn timeout_message(dir: Option<&str>, current: f64, proposed: f64) -> String {
    format!(
        "Running git in {} didn\u{2019}t finish within {} seconds, so the status bar component can\u{2019}t show the branch. \
This often happens in very large repositories or when the working tree is on a slow filesystem.\n\n\
Would you like to increase the timeout to {} seconds?",
        dir.unwrap_or("the current directory"),
        format_timeout(current),
        format_timeout(proposed)
    )
}

/// The Debug Info text (`GIT:513-536`). Never polled prints 0 seconds (iTerm2 prints `-0`
/// from a nil date).
pub fn debug_text(dir: Option<&str>, cadence: f64, enabled: bool, since_poll: Option<f64>, state: Option<&GitState>, cache_info: &str) -> String {
    format!(
        "Directory: {}\nPolling cadence: {} sec\nPolling enabled: {}\nLast polled {} seconds ago\nRepo state: {}\n{}",
        dir.unwrap_or("(null)"),
        cadence,
        if enabled { "Yes" } else { "No" },
        since_poll.unwrap_or(0.0),
        state.map_or_else(|| "(null)".to_string(), GitState::pretty),
        cache_info
    )
}

/// A click menu command (`GIT:507-626`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitCommand {
    Commit,
    AddCommit,
    Stash,
    Log,
    Push(String),
    Pull(String),
    Checkout(String),
    Debug,
}

impl GitCommand {
    /// git's arguments (none for Debug Info). A branch is never read as an option.
    pub fn args(&self) -> Vec<String> {
        let v: &[&str] = match self {
            GitCommand::Commit => &["commit"],
            GitCommand::AddCommit => &["commit", "-a"],
            GitCommand::Stash => &["stash"],
            GitCommand::Log => &["log"],
            GitCommand::Push(b) => return vec!["push".into(), "origin".into(), ref_arg(b)],
            GitCommand::Pull(b) => return vec!["pull".into(), "origin".into(), ref_arg(b)],
            GitCommand::Checkout(b) => return vec!["checkout".into(), ref_arg(b)],
            GitCommand::Debug => &[],
        };
        v.iter().map(|s| s.to_string()).collect()
    }

    /// The component's text while the command's window runs.
    pub fn status(&self) -> Option<&'static str> {
        match self {
            GitCommand::Commit | GitCommand::AddCommit => Some("Committing\u{2026}"),
            GitCommand::Stash => Some("Stashing\u{2026}"),
            GitCommand::Push(_) => Some("Pushing\u{2026}"),
            GitCommand::Pull(_) => Some("Pulling\u{2026}"),
            GitCommand::Checkout(_) => Some("Checking out\u{2026}"),
            GitCommand::Log | GitCommand::Debug => None,
        }
    }

    /// The single-use window starts buried (`InitiallyBuried`).
    pub fn buried(&self) -> bool {
        matches!(self, GitCommand::Stash | GitCommand::Push(_) | GitCommand::Pull(_) | GitCommand::Checkout(_))
    }
}

/// A branch as an argument git cannot read as an option. A hostile repository's HEAD can
/// name `refs/heads/--upload-pack=x`, and `--` or `--end-of-options` do not help (pull
/// rebuilds fetch's argument list without them; checkout reads `--` as pathspecs), so a
/// name starting with `-` goes as its full ref name. The menu never offers one.
fn ref_arg(branch: &str) -> String {
    if branch.starts_with('-') { format!("refs/heads/{branch}") } else { branch.to_string() }
}

/// A menu row; a disabled one shows but does nothing.
#[derive(Clone, Debug, PartialEq)]
pub struct GitItem {
    pub title: String,
    pub enabled: bool,
    pub command: GitCommand,
}

/// The menu for a state with a branch (`GIT:343-404`); `None` is a separator. The branch
/// list is cut to 7 before the current branch is skipped (iTerm2's, kept). A branch whose
/// name starts with `-` is never offered: Push and Pull are disabled, and Check out skips it.
pub fn git_menu(state: &GitState, branches: &[String]) -> Vec<Option<GitItem>> {
    let item = |title: &str, enabled: bool, command: GitCommand| Some(GitItem { title: title.to_string(), enabled, command });
    let b = &state.branch;
    let safe = !b.starts_with('-');
    let mut m = vec![
        item("Commit", state.dirty, GitCommand::Commit),
        item("Add & Commit", state.dirty, GitCommand::AddCommit),
        item("Stash", state.dirty, GitCommand::Stash),
        item("Log", true, GitCommand::Log),
        item(&format!("Push origin {b}"), (ns_integer(&state.ahead) > 0 || state.ahead == "error") && safe, GitCommand::Push(b.clone())),
        item(&format!("Pull origin {b}"), !state.dirty && safe, GitCommand::Pull(b.clone())),
        None,
    ];
    for other in branches.iter().take(MENU_BRANCHES).filter(|o| !o.is_empty() && *o != b && !o.starts_with('-')) {
        m.push(item(&format!("Check out {other}"), true, GitCommand::Checkout(other.clone())));
    }
    m.push(None);
    m.push(item("Show Debug Info", true, GitCommand::Debug));
    m
}

/// The running command's cancel row (`GIT:318`).
pub fn cancel_title(status: &str) -> String {
    format!("Cancel {}", status.replace('\u{2026}', ""))
}

/// The command sent to a remote host's shell (`GIT:577-594`): each argument with iTerm2's
/// backslash escaping, newlines included.
pub fn remote_command(args: &[String]) -> String {
    let esc = |a: &String| {
        let job = crate::paste::Job::new(a, crate::paste::ESCAPE);
        crate::paste::sanitize(&job, &crate::paste::Prefs::default()).replace('\r', "\\\r").replace('\n', "\\\n")
    };
    let mut out = "git".to_string();
    for a in args {
        out.push(' ');
        out.push_str(&esc(a));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Mutex;

    type Reply = Box<dyn Fn(&str) -> RunResult + Send + Sync>;

    /// A `GitRun` that records every call and answers by subcommand.
    struct Fake {
        calls: Mutex<Vec<(Vec<String>, Duration)>>,
        delay: Duration,
        reply: Reply,
    }

    impl Fake {
        fn new(reply: impl Fn(&str) -> RunResult + Send + Sync + 'static) -> Fake {
            Fake { calls: Mutex::new(Vec::new()), delay: Duration::ZERO, reply: Box::new(reply) }
        }
        fn calls(&self) -> Vec<(Vec<String>, Duration)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl GitRun for Fake {
        fn run(&self, _git: &Path, _dir: &Path, args: &[String], timeout: Duration, _cap: Option<usize>) -> RunResult {
            self.calls.lock().unwrap().push((args.to_vec(), timeout));
            std::thread::sleep(self.delay);
            (self.reply)(&sub(args))
        }
    }

    fn sub(args: &[String]) -> String {
        args.iter().find(|a| matches!(a.as_str(), "status" | "rev-parse" | "for-each-ref" | "version")).cloned().unwrap_or_default()
    }

    fn exit(out: &[u8]) -> RunResult {
        RunResult::Exit { code: 0, stdout: out.to_vec(), capped: false }
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tandem-git-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// A temp folder with an empty `.git` folder.
    fn temp_repo(name: &str) -> PathBuf {
        let d = temp(name);
        fs::create_dir(d.join(".git")).unwrap();
        d
    }

    const PORC: &[u8] = b"# branch.oid 1234567890abcdef1234567890abcdef12345678\0# branch.head main\0";
    const SEC: Duration = Duration::from_secs(1);
    const GIT: &str = "git";

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn fetched(state: Option<GitState>, timed_out: bool) -> Fetched {
        Fetched { state, timed_out }
    }

    fn main_state() -> GitState {
        GitState { branch: "main".into(), ..Default::default() }
    }

    #[test]
    fn porcelain_parses_like_libgit2() {
        let z = b"# branch.oid 1234567890abcdef1234567890abcdef12345678\0# branch.head main\0\
# branch.upstream origin/main\0# branch.ab +2 -1\0\
1 .M N... 100644 100644 100644 aaa bbb src/a.rs\0\
1 .D N... 100644 100644 000000 aaa aaa gone.rs\0? new1.txt\0? dir/new2.txt\0";
        let s = parse_porcelain(z).unwrap();
        assert_eq!((s.branch.as_str(), s.ahead.as_str(), s.behind.as_str()), ("main", "2", "1"));
        assert!(s.dirty);
        assert_eq!((s.adds, s.deletes), (2, 1));
        assert_eq!(s.repo_state, RepoState::None);

        let oid = "0123456789abcdef0123456789abcdef01234567";
        let z = format!("# branch.oid {oid}\0# branch.head (detached)\0");
        let s = parse_porcelain(z.as_bytes()).unwrap();
        assert_eq!(s.branch, oid, "detached: the full commit id");
        assert_eq!((s.ahead.as_str(), s.behind.as_str()), ("", ""), "no upstream: no counts");
        assert!(!s.dirty, "no entries is clean");
        assert_eq!((s.adds, s.deletes), (0, 0));

        assert_eq!(parse_porcelain(b"# branch.oid (initial)\0# branch.head main\0? a\0"), None, "unborn branch");
        assert_eq!(parse_porcelain(b""), None, "no headers");

        // A rename entry's second field is the original path, never an entry of its own.
        let z = [PORC, b"2 R. N... 100644 100644 100644 aaa bbb R100 new.rs\0?weird\0"].concat();
        let s = parse_porcelain(&z).unwrap();
        assert!(s.dirty);
        assert_eq!((s.adds, s.deletes), (0, 0));

        let z = [PORC, b"u UD N... 100644 100644 100644 100644 a b c conflict.rs\0"].concat();
        let s = parse_porcelain(&z).unwrap();
        assert!(s.dirty, "an unmerged entry is dirty");
        assert_eq!((s.adds, s.deletes), (0, 0), "and counts nothing else");

        let z = [PORC, b"1 D. N... 100644 000000 000000 aaa 000 staged-rm.rs\0"].concat();
        assert_eq!(parse_porcelain(&z).unwrap().deletes, 0, "an index deletion is not a worktree deletion");
    }

    #[test]
    fn repo_state_follows_libgit2() {
        let case = |name: &str, make: &dyn Fn(&Path)| {
            let g = temp(&format!("state-{name}"));
            make(&g);
            let got = repo_state(&LocalPath::judge(&g).unwrap());
            fs::remove_dir_all(&g).unwrap();
            got
        };
        let file = |g: &Path, p: &str| {
            let p = g.join(p);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, "x").unwrap();
        };
        let dir = |g: &Path, p: &str| fs::create_dir_all(g.join(p)).unwrap();
        assert_eq!(case("none", &|_| {}), RepoState::None);
        assert_eq!(case("rmi", &|g| file(g, "rebase-merge/interactive")), RepoState::Rebase);
        assert_eq!(case("rm", &|g| dir(g, "rebase-merge")), RepoState::Rebase);
        assert_eq!(case("rar", &|g| file(g, "rebase-apply/rebasing")), RepoState::Rebase);
        assert_eq!(case("raa", &|g| file(g, "rebase-apply/applying")), RepoState::Apply);
        assert_eq!(case("ra", &|g| dir(g, "rebase-apply")), RepoState::Apply);
        assert_eq!(case("merge", &|g| file(g, "MERGE_HEAD")), RepoState::Merge);
        assert_eq!(case("revert", &|g| file(g, "REVERT_HEAD")), RepoState::Revert);
        assert_eq!(case("pick", &|g| file(g, "CHERRY_PICK_HEAD")), RepoState::Cherrypick);
        assert_eq!(case("bisect", &|g| file(g, "BISECT_LOG")), RepoState::Bisect);
        assert_eq!(case("both", &|g| { dir(g, "rebase-merge"); file(g, "MERGE_HEAD") }), RepoState::Rebase, "rebase wins");
        assert_eq!(case("mdir", &|g| dir(g, "MERGE_HEAD")), RepoState::None, "MERGE_HEAD must be a file");
    }

    #[test]
    fn guarded_lines_share_the_hardening() {
        for base in [STATUS_ARGS, GIT_DIR_ARGS, BRANCHES_ARGS, LOG_ARGS] {
            assert_eq!(&base[..HARDENED.len()], HARDENED);
        }
        let vet = Vetted { args: vec!["-c".into(), "filter.x.clean=".into()], partial_clone: false, root: None, programs: vec![] };
        let line = guarded_args(&vet, &["status", "--porcelain"]);
        assert_eq!(&line[..HARDENED.len()], HARDENED);
        assert_eq!(&line[HARDENED.len()..], ["-c", "filter.x.clean=", "status", "--porcelain"]);
    }

    #[test]
    // The file-backed checks need real local paths: `local_path_ok` accepts only drive
    // paths, so on Linux every temp folder is refused (the suite runs on Windows).
    #[cfg_attr(not(windows), ignore = "needs a drive-letter temp folder")]
    fn git_never_runs_off_local_paths() {
        let fake = Fake::new(|_| exit(PORC));
        for d in [r"\\server\share\repo", "//server/share/repo", r"\\?\C:\x", r"\\?\UNC\server\share"] {
            assert_eq!(fetch_state(&fake, Path::new(GIT), Path::new(d), 4 * SEC), fetched(None, false), "{d}");
            assert!(!repo_path_ok(Path::new(d)), "{d}");
            assert!(recent_branches(&fake, Path::new(GIT), Path::new(d), 4 * SEC).is_empty(), "{d}");
        }
        let d = temp("unc-gitdir");
        fs::write(d.join(".git"), "gitdir: \\\\server\\x\n").unwrap();
        assert!(!repo_path_ok(&d));
        assert_eq!(fetch_state(&fake, Path::new(GIT), &d, 4 * SEC), fetched(None, false));
        fs::write(d.join(".git"), "gitdir: //server/x\n").unwrap();
        assert!(!repo_path_ok(&d));
        assert!(fake.calls().is_empty(), "git never started: {:?}", fake.calls());

        // A worktree's relative `gitdir:` is fine.
        let d2 = temp("rel-gitdir");
        fs::create_dir(d2.join("real.git")).unwrap();
        fs::write(d2.join(".git"), "gitdir: real.git\n").unwrap();
        assert!(repo_path_ok(&d2));
        assert_eq!(fetch_state(&fake, Path::new(GIT), &d2, 4 * SEC), fetched(Some(main_state()), false));
        assert_eq!(fake.calls().len(), 2);

        // The git directory rev-parse names is read only when it is local.
        let merging = temp("merging");
        fs::write(merging.join("MERGE_HEAD"), "x").unwrap();
        let local = format!("{}\n", merging.display());
        let unc = Fake::new(|s| if s == "status" { exit(PORC) } else { exit(b"//server/x/.git\n") });
        let got = fetch_state(&unc, Path::new(GIT), &d2, 4 * SEC).state.unwrap();
        assert_eq!(got.repo_state, RepoState::None);
        let near = Fake::new(move |s| if s == "status" { exit(PORC) } else { exit(local.as_bytes()) });
        assert_eq!(fetch_state(&near, Path::new(GIT), &d2, 4 * SEC).state.unwrap().repo_state, RepoState::Merge);

        assert_eq!(git_program("\\\\server\\tools"), None);
        assert_eq!(git_program("//server/tools;C:\\Git\\cmd"), None, "only the first entry is used");
        assert_eq!(git_program(""), Some(PathBuf::from("git")));
        assert_eq!(git_program("C:\\Git\\cmd;D:\\x"), Some(PathBuf::from("C:\\Git\\cmd\\git.exe")));
        assert_eq!(git_program("\"C:\\Program Files\\Git\\cmd\\\""), Some(PathBuf::from("C:\\Program Files\\Git\\cmd\\git.exe")));
        assert_eq!(git_program_named("\\\\server\\tools"), None, "the text check alone refuses a share");
        assert_eq!(git_program_named("//server/tools"), None);
        assert_eq!(git_program_named(""), Some(PathBuf::from("git")));
        assert_eq!(git_program_named("C:\\Git\\cmd;D:\\x"), Some(PathBuf::from("C:\\Git\\cmd\\git.exe")));

        // Finding git on PATH never looks in a network folder.
        let looked = Mutex::new(Vec::new());
        let is_file = |p: &LocalPath| {
            looked.lock().unwrap().push(p.path().to_path_buf());
            p.path() == Path::new("D:\\Git\\cmd\\git.exe")
        };
        assert_eq!(find_git("\\\\server\\bin;relative;C:\\none;\"D:\\Git\\cmd\"", &is_file), Some(PathBuf::from("D:\\Git\\cmd\\git.exe")));
        assert_eq!(*looked.lock().unwrap(), vec![PathBuf::from("C:\\none\\git.exe"), PathBuf::from("D:\\Git\\cmd\\git.exe")]);
        assert_eq!(find_git("\\\\server\\bin", &is_file), None);
        for p in [d, d2, merging] {
            fs::remove_dir_all(p).unwrap();
        }
    }

    /// Task 6 round 1, item 1 (class: a repository's own config is untrusted): a repo that sets any key
    /// that runs a program is refused for a launch, whatever the key.
    #[test]
    #[cfg_attr(not(windows), ignore = "needs a drive-letter temp folder")]
    fn launch_refuses_a_repo_that_configures_programs() {
        let cases = [
            ("pager", "[core]\n\tpager = calc.exe\n"),
            ("pagerdiff", "[pager]\n\tdiff = calc\n"),
            ("ext", "[diff]\n\texternal = calc\n"),
            ("drivercmd", "[diff \"x\"]\n\tcommand = calc\n"),
            ("textconv", "[diff \"x\"]\n\ttextconv = calc\n"),
            ("difftool", "[difftool \"t\"]\n\tcmd = calc\n"),
            ("difftool-name", "[diff]\n\ttool = t\n"),
            ("mergetool", "[mergetool \"t\"]\n\tcmd = calc\n"),
            ("alias", "[alias]\n\tdiff = !calc\n"),
            ("ssh", "[core]\n\tsshCommand = calc\n"),
            ("cred", "[credential]\n\thelper = calc\n"),
            ("credurl", "[credential \"https://x\"]\n\thelper = calc\n"),
            ("fsm", "[core]\n\tfsmonitor = calc\n"),
            ("editor", "[core]\n\teditor = calc\n"),
            ("seqeditor", "[sequence]\n\teditor = calc\n"),
            ("upload", "[uploadpack]\n\tpackObjectsHook = calc\n"),
            ("promisor", "[remote \"o\"]\n\tpromisor = true\n"),
            ("partial", "[extensions]\n\tpartialClone = o\n"),
            ("include", "[include]\n\tpath = more.cfg\n"),
        ];
        for (name, text) in cases {
            let d = temp_repo(&format!("launch-{name}"));
            fs::write(d.join(".git").join("config"), text).unwrap();
            fs::write(d.join(".git").join("more.cfg"), "[core]\n\tpager = calc\n").unwrap();
            let v = vet_repo(&d).unwrap_or_else(|| panic!("{name}: still vetted for the poller"));
            assert!(v.launch_refusal().is_some(), "{name}: a launch must be refused");
            fs::remove_dir_all(&d).unwrap();
        }
        let d = temp_repo("launch-plain");
        fs::write(d.join(".git").join("config"), "[core]\n\tbare = false\n[remote \"origin\"]\n\turl = https://example.com/r.git\n[filter \"lfs\"]\n\tclean = git-lfs clean\n[user]\n\tname = a\n").unwrap();
        let v = vet_repo(&d).unwrap();
        assert_eq!(v.launch_refusal(), None, "a plain config launches (filters are switched off by -c)");
        assert!(v.args.iter().any(|a| a == "filter.lfs.clean="));
        fs::remove_dir_all(&d).unwrap();
    }

    /// Task 6 round 1, items 2 and 3: a launch (and Run Anyway) needs the worker's verdict for the folder:
    /// a vetted repository with a working tree top, a git, and a config that runs nothing.
    #[test]
    fn run_anyway_needs_a_vetted_repository() {
        let ok = Launch { vet: Some(Vetted { root: Some(PathBuf::from(r"C:\r")), ..Default::default() }), git: Some(PathBuf::from(r"C:\g\git.exe")) };
        assert_eq!(launch_verdict(Some(&ok)), None);
        assert!(launch_verdict(None).is_some_and(|t| t.contains("not finished")), "no answer yet: git does not run");
        assert!(launch_verdict(Some(&Launch { vet: None, ..ok.clone() })).is_some_and(|t| t.contains("not safe")));
        assert!(launch_verdict(Some(&Launch { vet: Some(Vetted::default()), ..ok.clone() })).is_some_and(|t| t.contains("not in a git repository")), "no working tree top");
        assert!(launch_verdict(Some(&Launch { git: None, ..ok.clone() })).is_some_and(|t| t.contains("git was not found")));
        let bad = Launch { vet: Some(Vetted { root: Some(PathBuf::from(r"C:\r")), programs: vec!["core.pager".into()], ..Default::default() }), ..ok };
        assert!(launch_verdict(Some(&bad)).is_some_and(|t| t.contains("core.pager")));
    }

    #[test]
    #[cfg_attr(not(windows), ignore = "needs a drive-letter temp folder")]
    fn a_folder_outside_any_repository_has_no_root() {
        let d = temp("no-repo");
        let v = vet_repo(&d).expect("outside a repository is not unsafe");
        assert_eq!(v.root, None);
        assert!(launch_verdict(Some(&Launch { vet: Some(v), git: Some(PathBuf::from("git")) })).is_some());
        fs::remove_dir_all(&d).unwrap();
    }

    /// The pty launch's argv and environment carry the overrides, and are built in one place.
    #[test]
    fn launch_line_carries_the_overrides() {
        let vet = Vetted { args: vec!["-c".into(), "filter.x.clean=".into()], partial_clone: false, root: None, programs: vec![] };
        let diff = launch_args(&vet, &["diff", "HEAD", "--", "f"]).unwrap();
        assert_eq!(&diff[..HARDENED.len()], HARDENED);
        for want in [
            "--no-pager", "core.hooksPath=NUL", "core.pager=cat", "diff.external=", "credential.helper=", "protocol.allow=never", "core.sshCommand=false", "filter.x.clean=", "submodule.recurse=false", "diff.submodule=short",
            "diff.ignoreSubmodules=all",
        ] {
            assert!(diff.iter().any(|a| a == want), "{want} in {diff:?}");
        }
        let at = diff.iter().position(|a| a == "diff").unwrap();
        assert_eq!(&diff[at..], ["diff", "--ignore-submodules=all", "--no-ext-diff", "--no-textconv", "HEAD", "--", "f"], "the overrides come right after the subcommand");
        let tool = launch_args(&vet, &["difftool", "-y", "-x", "vimdiff"]).unwrap();
        let at = tool.iter().position(|a| a == "difftool").unwrap();
        assert_eq!(&tool[at..], ["difftool", "--ignore-submodules=all", "-y", "-x", "vimdiff"], "difftool is the person's own tool, never into a submodule");
        for sub in ["status", "show", "log"] {
            let a = launch_args(&vet, &[sub]).unwrap();
            let at = a.iter().position(|x| x == sub).unwrap();
            assert_eq!(a[at + 1], "--ignore-submodules=all", "{a:?}");
            assert_eq!(a.iter().any(|x| x == "--no-ext-diff"), sub != "status", "{a:?}");
        }
        // A subcommand outside the list, or a global option before it, is refused.
        for bad in [&["-c", "core.pager=calc", "diff"][..], &["fetch"], &["submodule", "foreach", "calc"], &[]] {
            assert!(launch_args(&vet, bad).is_err(), "{bad:?}");
        }
        // An option that undoes an override, also abbreviated or negated twice, is refused before `--`.
        for bad in ["--ignore-submodules=none", "--ignore-sub=dirty", "--submodule=diff", "--sub=log", "--recurse-submodules", "--ext-diff", "--ext", "--textconv", "--no-no-ext-diff"] {
            assert!(launch_args(&vet, &["diff", bad]).is_err(), "{bad}");
        }
        for ok in ["--no-ext-diff", "--no-textconv", "--stat", "--ignore-space-change", "-w", "--text"] {
            assert!(launch_args(&vet, &["diff", ok]).is_ok(), "{ok}");
        }
        assert!(launch_args(&vet, &["diff", "HEAD", "--", "--ext-diff"]).is_ok(), "a path after -- is a path");
        let env = launch_env();
        for want in ["GIT_CONFIG_NOSYSTEM=1", "GIT_TERMINAL_PROMPT=0", "GIT_NO_LAZY_FETCH=1", "GIT_PAGER=cat", "PAGER=cat"] {
            assert!(env.iter().any(|e| e == want), "{want}");
        }
        for gone in [
            "-GIT_EXTERNAL_DIFF", "-GIT_ASKPASS", "-SSH_ASKPASS", "-GIT_DIR", "-GIT_WORK_TREE", "-GIT_CONFIG_PARAMETERS", "-GIT_CONFIG_COUNT", "-GIT_EXEC_PATH", "-GIT_SSH", "-GIT_SSH_COMMAND", "-GIT_PROXY_COMMAND",
        ] {
            assert!(env.iter().any(|e| e == gone), "{gone}");
        }
    }

    #[test]
    fn programs_in_a_config_are_listed_without_values() {
        let mut programs = Vec::new();
        let text = "[core]\n\tpager = secret-value\n[alias]\n\tx = !y\n";
        for e in config_entries(text).unwrap() {
            note_program(&e.section, e.sub.as_deref(), &e.key, &mut programs);
        }
        assert_eq!(programs, ["core.pager", "alias.x"]);
    }

    #[test]
    // The file-backed checks need real local paths: `local_path_ok` accepts only drive
    // paths, so on Linux every temp folder is refused (the suite runs on Windows).
    #[cfg_attr(not(windows), ignore = "needs a drive-letter temp folder")]
    fn repo_config_cannot_run_programs_or_reach_shares() {
        let fake = Fake::new(|_| exit(PORC));
        let d = temp_repo("filters");
        fs::write(d.join(".git").join("config"), "[core]\n\tbare = false\n[filter \"Ev.il\"]\n\tprocess = calc.exe\n[filter.LEGACY]\n\tclean = x\n[include]\n\tpath = \"more.cfg\" ; a comment\n").unwrap();
        fs::write(d.join(".git").join("more.cfg"), "[filter \"third\"]\nsmudge = y\n").unwrap();
        fetch_state(&fake, Path::new(GIT), &d, 4 * SEC);
        let calls = fake.calls();
        assert_eq!(calls.len(), 2);
        for (args, _) in &calls {
            assert_eq!(args[0], "--no-optional-locks");
            let has = |a: &str| args.iter().any(|x| x == a);
            assert!(has("core.fsmonitor=false"), "{args:?}");
            for name in ["Ev.il", "legacy", "third"] {
                for key in ["clean=", "smudge=", "process=", "required=false"] {
                    assert!(has(&format!("filter.{name}.{key}")), "{name}.{key} in {args:?}");
                }
            }
            let at = args.iter().position(|a| a == "status" || a == "rev-parse").unwrap();
            assert!(args[..at].iter().any(|a| a.starts_with("filter.")), "the overrides go before the subcommand");
        }
        let refused = |name: &str, file: &str, text: &str| {
            let d = temp_repo(name);
            let p = d.join(".git").join(file);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, text).unwrap();
            let ok = repo_path_ok(&d);
            fs::remove_dir_all(&d).unwrap();
            !ok
        };
        assert!(refused("inc-unc", "config", "[include]\n\tpath = \\\\\\\\server\\\\share\\\\x\n"), "an include on a share");
        assert!(refused("inc-quoted", "config", "[includeIf \"gitdir:C:/\"]\n\tpath = \"//server/share/x\"\n"));
        assert!(refused("excludes", "config", "[core]\n\texcludesFile = //server/share/ignore\n"));
        assert!(refused("worktree", "config.worktree", "[core]\n\tworktree = \\\\\\\\server\\\\w\n"));
        assert!(refused("commondir", "commondir", "\\\\server\\common\n"));
        assert!(refused("alternates", "objects/info/alternates", "//server/objects\n"));
        assert!(refused("bad-name", "config", "[filter \"a=b\"]\n\tclean = x\n"), "a name -c cannot carry");
        assert!(refused("deep", "config", "[include]\n\tpath = config\n"), "an include loop");
        assert!(!refused("plain", "config", "[remote \"origin\"]\n\turl = https://example.com/r.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n"));
        assert!(!refused("local-alt", "objects/info/alternates", "# comment\nC:/objects\n../../other/objects\n"));

        // A bare repository git would find above the directory is checked too.
        let bare = temp("bare");
        fs::write(bare.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(bare.join("config"), "[core]\n\texcludesFile = //server/x\n").unwrap();
        fs::create_dir(bare.join("sub")).unwrap();
        assert!(!repo_path_ok(&bare.join("sub")));
        for p in [d, bare] {
            fs::remove_dir_all(p).unwrap();
        }
    }

    #[test]
    // The file-backed checks need real local paths: `local_path_ok` accepts only drive
    // paths, so on Linux every temp folder is refused (the suite runs on Windows).
    #[cfg_attr(not(windows), ignore = "needs a drive-letter temp folder")]
    fn fetch_state_shares_one_deadline() {
        let d = temp_repo("deadline");
        let mut fake = Fake::new(|s| if s == "status" { exit(PORC) } else { exit(b"") });
        fake.delay = ms(50);
        assert_eq!(fetch_state(&fake, Path::new(GIT), &d, 4 * SEC), fetched(Some(main_state()), false));
        let calls = fake.calls();
        assert_eq!(sub(&calls[0].0), "status");
        assert!(calls[0].1 <= 4 * SEC && calls[0].1 > 3 * SEC, "status gets what the checks left: {:?}", calls[0].1);
        assert_eq!(sub(&calls[1].0), "rev-parse");
        assert!(calls[1].1 <= 4 * SEC - ms(50) && calls[1].1 > 3 * SEC, "{:?}", calls[1].1);

        let not_repo = Fake::new(|_| RunResult::Exit { code: 128, stdout: Vec::new(), capped: false });
        assert_eq!(fetch_state(&not_repo, Path::new(GIT), &d, 4 * SEC), fetched(None, false));
        assert_eq!(not_repo.calls().len(), 1);
        let huge = Fake::new(|_| RunResult::Exit { code: 0, stdout: PORC.to_vec(), capped: true });
        assert_eq!(fetch_state(&huge, Path::new(GIT), &d, 4 * SEC), fetched(None, false), "a capped status is no state");
        let failed = Fake::new(|_| RunResult::Failed("no git".into()));
        assert_eq!(fetch_state(&failed, Path::new(GIT), &d, 4 * SEC), fetched(None, false));
        let slow = Fake::new(|_| RunResult::TimedOut);
        assert_eq!(fetch_state(&slow, Path::new(GIT), &d, 4 * SEC), fetched(None, true));
        let slow_dir = Fake::new(|s| if s == "status" { exit(PORC) } else { RunResult::TimedOut });
        assert_eq!(fetch_state(&slow_dir, Path::new(GIT), &d, 4 * SEC), fetched(None, true));

        let branches = Fake::new(|_| exit(b"main\r\nfeature/x\n\nold\n"));
        assert_eq!(recent_branches(&branches, Path::new(GIT), &d, 4 * SEC), ["main", "feature/x", "old"]);
        assert_eq!(sub(&branches.calls()[0].0), "for-each-ref");
        assert!(branches.calls()[0].0.iter().any(|a| a == "--count=8"));
        assert_eq!(parse_branches(b"a\nb"), ["a", "b"]);

        assert_eq!(timeout_of(4.0), 4 * SEC);
        assert_eq!(timeout_of(4.9), 4 * SEC);
        assert_eq!(timeout_of(0.2), SEC);
        assert_eq!(timeout_of(-3.0), SEC);
        assert_eq!(timeout_of(f64::NAN), SEC);
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn worker_cache_follows_iterm2() {
        let t0 = Instant::now() + 10 * SEC;
        let p = "C:\\r";
        let mut c = WorkerCache::default();
        assert!(matches!(c.lookup(p, t0), Lookup::Start));
        assert!(matches!(c.lookup(p, t0), Lookup::Joined));
        assert_eq!(c.debug_info(p, t0), "Basic cache: none\nRich cache: none\nPending calls: 2\n");
        let s = main_state();
        assert_eq!(c.finish(p, fetched(Some(s.clone()), false), t0), fetched(Some(s.clone()), false));
        assert_eq!(c.debug_info(p, t0 + ms(500)), "Basic cache: age 0.5\nRich cache: none\nPending calls: 0\n");
        match c.lookup(p, t0 + ms(500)) {
            Lookup::Cached(Some(x)) => assert_eq!(x, s),
            _ => panic!("under 1 s is cached"),
        }
        assert!(matches!(c.lookup("C:\\other", t0), Lookup::Start), "keyed by path");
        assert!(matches!(c.lookup(p, t0 + SEC), Lookup::Start), "1 s old is stale");

        let old = GitState { branch: "old".into(), ..Default::default() };
        assert_eq!(c.finish(p, fetched(Some(old), false), t0 - SEC), fetched(Some(s.clone()), false), "an older reply loses");
        assert_eq!(c.finish(p, fetched(None, true), t0 + 2 * SEC), fetched(Some(s.clone()), true), "the timeout passes through");
        assert_eq!(c.finish(p, fetched(None, false), t0 + 2 * SEC), fetched(Some(s.clone()), false), "a stateless reply is oldest");
        let new = GitState { branch: "new".into(), ..Default::default() };
        assert_eq!(c.finish(p, fetched(Some(new.clone()), false), t0 + 3 * SEC), fetched(Some(new), false));
        assert_eq!(c.debug_info(p, t0 + 3 * SEC), "Basic cache: age 0\nRich cache: none\nPending calls: 0\n");

        assert!(matches!(c.lookup(p, t0 + 5 * SEC), Lookup::Start));
        c.invalidate(p);
        assert_eq!(c.debug_info(p, t0 + 5 * SEC), "Basic cache: none\nRich cache: none\nPending calls: 0\n");
        assert!(matches!(c.lookup(p, t0 + 5 * SEC), Lookup::Start), "no pending call to join");
        assert_eq!(c.finish(p, fetched(None, false), t0 + 5 * SEC), fetched(None, false), "nothing cached, nothing kept");
        assert_eq!(c.debug_info(p, t0 + 5 * SEC), "Basic cache: none\nRich cache: none\nPending calls: 0\n");
    }

    fn visible() -> PollGate {
        PollGate { visible: true, last_newline: None, last_activity: None }
    }

    #[test]
    fn stale_poll_results_are_dropped() {
        let t0 = Instant::now();
        let mut p = GitPoller::new(2.0, t0);
        p.set_enabled(true);
        assert_eq!(p.set_directory(Some("C:\\a".into()), &visible(), t0).request.as_deref(), Some("C:\\a"));
        p.set_directory(Some("C:\\b".into()), &visible(), t0 + SEC);
        assert!(!p.did_poll("C:\\a", fetched(Some(main_state()), true)));
        assert_eq!(p.state, None);
        assert!(!p.timed_out && !p.has_fetched);
        assert!(p.did_poll("C:\\b", fetched(Some(main_state()), false)));
        assert_eq!(p.state, Some(main_state()));
        assert!(p.has_fetched);
    }

    #[test]
    fn timeouts_are_remembered_per_directory() {
        let t0 = Instant::now();
        let mut p = GitPoller::new(2.0, t0);
        p.set_enabled(true);
        p.set_directory(Some("C:\\big".into()), &visible(), t0);
        assert!(p.did_poll("C:\\big", fetched(None, true)));
        assert!(p.timed_out && !p.has_fetched);
        assert!(show_timeout(true, p.enabled, p.has_fetched, p.timed_out));
        assert!(!show_timeout(false, true, false, true), "not on a remote host");
        assert!(!show_timeout(true, false, false, true), "not when disabled");
        assert!(!show_timeout(true, true, true, true), "not once this directory answered");
        assert!(!show_timeout(true, true, false, false));

        p.did_poll("C:\\big", fetched(None, false));
        assert!(!p.timed_out && p.has_fetched, "a clean reply clears it, even with no state");
        p.did_poll("C:\\big", fetched(None, true));
        assert!(p.timed_out && p.has_fetched, "a later timeout after an answer does not warn");
        assert!(!show_timeout(true, true, p.has_fetched, p.timed_out));

        p.set_directory(Some("C:\\other".into()), &visible(), t0 + SEC);
        assert!(!p.timed_out && !p.has_fetched, "per directory");
        p.did_poll("C:\\other", fetched(None, true));
        let typed = PollGate { last_newline: Some(t0 + 2 * SEC), ..visible() };
        let polls = p.clear_timeout_and_retry(&typed, t0 + 2 * SEC);
        assert!(!p.timed_out);
        assert_eq!(polls.request.as_deref(), Some("C:\\other"));
    }

    #[test]
    fn git_polling_follows_iterm2() {
        let t0 = Instant::now();
        let last = t0 + 10 * SEC;
        let hidden = PollGate { visible: false, ..visible() };
        assert!(should_poll(None, &visible(), t0), "the first poll always runs");
        assert!(!should_poll(None, &hidden, t0));
        let nl = |t| PollGate { last_newline: Some(t), ..visible() };
        assert!(should_poll(Some(last), &nl(last - 2 * SEC), last + SEC), "a newline after last_poll - 3 s");
        assert!(!should_poll(Some(last), &nl(last - 3 * SEC), last + SEC), "strictly after");
        assert!(!should_poll(Some(last), &nl(last - 4 * SEC), last + SEC));
        assert!(!should_poll(Some(last), &PollGate { visible: false, ..nl(last) }, last + SEC));
        let out = |t| PollGate { last_activity: Some(t), ..visible() };
        assert!(should_poll(Some(last), &out(last + SEC), last + 61 * SEC), "output and over 60 s");
        assert!(!should_poll(Some(last), &out(last + SEC), last + 59 * SEC));
        assert!(!should_poll(Some(last), &out(last - SEC), last + 61 * SEC), "output before the last poll");
        assert!(!should_poll(Some(last), &visible(), last + 61 * SEC));

        // Disabled, or no directory: nothing polls.
        let mut p = GitPoller::new(2.0, t0);
        assert_eq!(p.set_directory(Some("C:\\r".into()), &visible(), t0).request, None, "disabled");
        assert!(!p.set_enabled(true), "enabling does not redraw");
        assert!(!p.set_enabled(true), "no change");
        assert!(p.set_enabled(false), "disabling redraws to clear");
        assert_eq!(p.bump(&visible(), t0).request, None);
        p.set_enabled(true);
        let mut q = GitPoller::new(2.0, t0);
        q.set_enabled(true);
        assert_eq!(q.bump(&visible(), t0).request, None, "no directory");
        assert_eq!(q.set_directory(Some(String::new()), &visible(), t0).request, None, "an empty directory");
        let mut h = GitPoller::new(2.0, t0);
        h.set_enabled(true);
        assert_eq!(h.set_directory(Some("C:\\r".into()), &hidden, t0).request, None, "not visible");

        // The timer: first poll, then only with activity.
        let mut p = GitPoller::new(2.0, t0);
        p.set_enabled(true);
        assert_eq!(p.next_due(), None, "no directory: nothing to poll");
        let first = p.set_directory(Some("C:\\r".into()), &visible(), t0 + SEC);
        assert_eq!(first.request.as_deref(), Some("C:\\r"));
        assert_eq!(first.invalidate.as_deref(), Some("C:\\r"));
        assert_eq!(p.last_poll, Some(t0 + SEC));
        assert_eq!(p.next_due(), Some(t0 + 2 * SEC), "a directory change does not restart the timer");
        assert_eq!(p.tick(&visible(), t0 + ms(1500)).request, None, "not due");
        assert_eq!(p.tick(&visible(), t0 + 2 * SEC).request, None, "due, but nothing happened");
        assert_eq!(p.next_due(), Some(t0 + 4 * SEC));
        assert_eq!(p.tick(&nl(t0 + 3 * SEC), t0 + 4 * SEC).request.as_deref(), Some("C:\\r"));
        assert_eq!(p.last_poll, Some(t0 + 4 * SEC));

        // bump polls and restarts the timer; the knob alone waits for the next bump.
        assert_eq!(p.bump(&nl(t0 + 5 * SEC), t0 + 5 * SEC).request.as_deref(), Some("C:\\r"));
        assert_eq!(p.next_due(), Some(t0 + 7 * SEC));
        p.set_cadence(5.0);
        assert_eq!(p.cadence, 5.0);
        assert_eq!(p.next_due(), Some(t0 + 7 * SEC), "iTerm2 keeps the running timer");
        p.tick(&visible(), t0 + 7 * SEC);
        assert_eq!(p.next_due(), Some(t0 + 9 * SEC), "still the old interval");
        p.bump(&visible(), t0 + 8 * SEC);
        assert_eq!(p.next_due(), Some(t0 + 13 * SEC), "the bump takes the new cadence");
        p.set_cadence(0.0);
        assert_eq!(p.cadence, 1.0, "an interval of 0 is 1");
        assert_eq!(GitPoller::new(0.0, t0).cadence, 1.0);
        for knob in [1e-12, 0.5, f64::NAN, -2.0, f64::INFINITY] {
            p.set_cadence(knob);
            assert_eq!(p.cadence, 1.0, "{knob} floors at 1 s");
        }
        // However late the tick, the catch-up is one step onto the timer's grid.
        let mut tiny = GitPoller::new(1e-12, t0);
        tiny.set_enabled(true);
        tiny.set_directory(Some("C:\\r".into()), &visible(), t0);
        tiny.bump(&visible(), t0);
        let late = t0 + Duration::from_secs(10_000_000) + ms(250);
        tiny.tick(&visible(), late);
        assert_eq!(tiny.next_due(), Some(t0 + Duration::from_secs(10_000_001)));

        // Cache invalidation on a directory change, at most every 0.5 s.
        let t = t0 + 20 * SEC;
        let mut p = GitPoller::new(2.0, t);
        p.set_enabled(true);
        assert_eq!(p.set_directory(Some("C:\\a".into()), &visible(), t).invalidate.as_deref(), Some("C:\\a"));
        let second = p.set_directory(Some("C:\\b".into()), &nl(t + ms(200)), t + ms(200));
        assert_eq!(second.invalidate, None);
        assert_eq!(second.request.as_deref(), Some("C:\\b"));
        assert_eq!(p.next_due(), Some(t + ms(500)));
        assert_eq!(p.tick(&visible(), t + ms(499)).invalidate, None);
        assert_eq!(p.tick(&visible(), t + ms(500)).invalidate.as_deref(), Some("C:\\b"));
        assert_eq!(p.set_directory(Some("C:\\c".into()), &visible(), t + ms(600)).invalidate, None);
        assert_eq!(p.tick(&visible(), t + SEC).invalidate.as_deref(), Some("C:\\c"));
        assert_eq!(p.tick(&visible(), t + ms(1600)).invalidate, None, "nothing waiting");
        assert_eq!(p.set_directory(Some("C:\\d".into()), &visible(), t + 3 * SEC).invalidate.as_deref(), Some("C:\\d"), "quiet again");
        assert_eq!(p.set_directory(Some("C:\\d".into()), &visible(), t + 4 * SEC).request, None, "the same directory changes nothing");
        assert_eq!(p.set_directory(None, &visible(), t + 5 * SEC).invalidate, None, "no directory, nothing to invalidate");
    }

    #[test]
    fn git_text_follows_iterm2() {
        let s = main_state();
        let text = |st: &GitState| git_text(Some(st), None, true, "main");
        assert_eq!(git_text(Some(&s), Some("Committing\u{2026}"), true, "main").as_deref(), Some("Committing\u{2026}"));
        assert_eq!(git_text(None, Some("Stashing\u{2026}"), false, "main").as_deref(), Some("Stashing\u{2026}"), "a status wins");
        assert_eq!(git_text(Some(&s), None, false, "main"), None, "not ready");
        assert_eq!(git_text(None, None, true, "main"), None);
        for (rs, word) in [
            (RepoState::Merge, "Merging"),
            (RepoState::Revert, "Reverting"),
            (RepoState::Cherrypick, "Cherrypicking"),
            (RepoState::Bisect, "Bisecting"),
            (RepoState::Rebase, "Rebasing"),
            (RepoState::Apply, "Applying"),
        ] {
            assert_eq!(rs.text(), Some(word));
            assert_eq!(text(&GitState { repo_state: rs, ..s.clone() }).as_deref(), Some(word));
        }
        assert_eq!(RepoState::None.text(), None);
        assert_eq!(git_text(Some(&s), None, true, ""), None, "no branch");
        assert_eq!(text(&s).as_deref(), Some("main"));
        assert_eq!(text(&GitState { adds: 1, deletes: 2, ..s.clone() }).as_deref(), Some("main\u{2009}\u{00B1}"));
        assert_eq!(text(&GitState { adds: 1, ..s.clone() }).as_deref(), Some("main\u{2009}+"));
        assert_eq!(text(&GitState { deletes: 1, ..s.clone() }).as_deref(), Some("main\u{2009}-"));
        assert_eq!(text(&GitState { dirty: true, ..s.clone() }).as_deref(), Some("main\u{2009}\u{25CF}"));
        assert_eq!(text(&GitState { ahead: "2".into(), ..s.clone() }).as_deref(), Some("main\u{2002}\u{2191}2"));
        assert_eq!(text(&GitState { behind: "3".into(), ..s.clone() }).as_deref(), Some("main\u{2002}\u{2193}3"));
        assert_eq!(text(&GitState { ahead: "0".into(), behind: "error".into(), ..s.clone() }).as_deref(), Some("main"));
        let all = GitState { adds: 1, dirty: true, ahead: "1".into(), behind: "2".into(), ..s.clone() };
        assert_eq!(text(&all).as_deref(), Some("main\u{2009}+\u{2009}\u{25CF}\u{2002}\u{2191}1\u{2002}\u{2193}2"));
        let rich = GitState { files_added: 2, lines_inserted: 10, lines_deleted: 3, ..s.clone() };
        assert_eq!(text(&rich).as_deref(), Some("+2 files +10/-3 lines"));
        assert_eq!(text(&GitState { files_deleted: 1, lines_deleted: 4, ..s.clone() }).as_deref(), Some("-1 files -4 lines"));
        assert_eq!(text(&GitState { files_added: 1, files_deleted: 2, lines_inserted: 5, ..s.clone() }).as_deref(), Some("+1/-2 files +5 lines"));
        assert_eq!(text(&GitState { files_modified: 3, ..s.clone() }).as_deref(), Some(""), "modified files alone add no part");
        assert_eq!(git_text(Some(&s), None, true, "M").as_deref(), Some("M"), "the minimum width's branch");

        assert_eq!(git_variants(Some(&s), None, true), ["main"]);
        assert_eq!(git_variants(Some(&GitState { branch: String::new(), ..s.clone() }), None, true), [""]);
        assert_eq!(git_variants(None, Some("Committing\u{2026}"), true), [""], "no branch, no status either");
        assert_eq!(git_variants(Some(&s), None, false), [""]);
        assert_eq!((TIMED_OUT_SHORT, TIMED_OUT_ICON), ("\u{26A0} timed out", "\u{26A0}"));
    }

    /// Log runs the hardened `git log` in a vetted repository, shows a capped answer with
    /// the marker, and shows nothing on a timeout or off a local path (git never starts).
    #[test]
    fn log_is_hardened_and_truncated_like_iterm2() {
        let d = temp_repo("log");
        let capped = Fake::new(|_| RunResult::Exit { code: 0, stdout: b"commit abc\n".to_vec(), capped: true });
        assert_eq!(git_log(&capped, Path::new(GIT), &d, LOG_TIMEOUT), "commit abc\n\n[Truncated]\n");
        let calls = capped.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0.first().map(String::as_str), Some("--no-optional-locks"));
        assert_eq!(calls[0].0.last().map(String::as_str), Some("log"));
        assert!(calls[0].0.windows(2).any(|w| w[0] == "-c" && w[1] == "log.showSignature=false"));
        assert!(calls[0].1 <= LOG_TIMEOUT);
        assert_eq!(git_log(&Fake::new(|_| exit(b"x")), Path::new(GIT), &d, LOG_TIMEOUT), "x");
        assert_eq!(git_log(&Fake::new(|_| RunResult::TimedOut), Path::new(GIT), &d, LOG_TIMEOUT), "");
        let unc = Fake::new(|_| exit(b"x"));
        assert_eq!(git_log(&unc, Path::new(GIT), Path::new("\\\\server\\share\\r"), LOG_TIMEOUT), "");
        assert!(unc.calls().is_empty());
    }

    #[test]
    fn timeout_and_debug_texts_follow_iterm2() {
        assert_eq!(proposed_timeout(4.0), 8.0);
        assert_eq!(proposed_timeout(1.0), 3.0);
        assert_eq!(format_timeout(8.0), "8");
        assert_eq!(format_timeout(3.004), "3");
        assert_eq!(format_timeout(2.5), "2.5");
        assert_eq!(increase_button(8.0), "Increase to 8s");
        assert_eq!(
            timeout_message(None, 4.0, 8.0),
            "Running git in the current directory didn\u{2019}t finish within 4 seconds, so the status bar component can\u{2019}t show the branch. \
This often happens in very large repositories or when the working tree is on a slow filesystem.\n\nWould you like to increase the timeout to 8 seconds?"
        );
        assert!(timeout_message(Some("C:\\big"), 4.5, 8.5).starts_with("Running git in C:\\big didn\u{2019}t finish within 4.5 seconds"));
        let s = GitState { branch: "main".into(), ahead: "1".into(), dirty: true, adds: 2, lines_inserted: 3, files_modified: 4, ..Default::default() };
        assert_eq!(s.pretty(), "dir=(null) xcode=(null) ahead=1 behind= branch=main dirty=1 adds=2 deletes=0 lines=+3/-0 files=+0/~4/-0");
        assert_eq!(
            debug_text(Some("C:\\r"), 2.0, true, Some(1.5), Some(&s), "Basic cache: none\nRich cache: none\nPending calls: 0\n"),
            format!("Directory: C:\\r\nPolling cadence: 2 sec\nPolling enabled: Yes\nLast polled 1.5 seconds ago\nRepo state: {}\nBasic cache: none\nRich cache: none\nPending calls: 0\n", s.pretty())
        );
        assert_eq!(debug_text(None, 0.5, false, None, None, ""), "Directory: (null)\nPolling cadence: 0.5 sec\nPolling enabled: No\nLast polled 0 seconds ago\nRepo state: (null)\n");
    }

    #[test]
    fn git_menu_follows_iterm2() {
        let titles = |m: &[Option<GitItem>]| m.iter().map(|i| i.as_ref().map(|i| (i.title.clone(), i.enabled))).collect::<Vec<_>>();
        let t = |s: &str, on: bool| Some((s.to_string(), on));
        let dirty = GitState { branch: "main".into(), dirty: true, ahead: "1".into(), ..Default::default() };
        assert_eq!(
            titles(&git_menu(&dirty, &[])),
            [t("Commit", true), t("Add & Commit", true), t("Stash", true), t("Log", true), t("Push origin main", true), t("Pull origin main", false), None, None, t("Show Debug Info", true)]
        );
        let clean = GitState { branch: "main".into(), ahead: "error".into(), ..Default::default() };
        let m = titles(&git_menu(&clean, &[]));
        assert_eq!(&m[..6], [t("Commit", false), t("Add & Commit", false), t("Stash", false), t("Log", true), t("Push origin main", true), t("Pull origin main", true)]);
        let even = GitState { ahead: "0".into(), ..clean.clone() };
        assert_eq!(titles(&git_menu(&even, &[]))[4], t("Push origin main", false));

        let branches: Vec<String> = ["main", "a", "b", "c", "d", "e", "f", "g"].iter().map(|s| s.to_string()).collect();
        let m = git_menu(&clean, &branches);
        let rest = titles(&m[6..]);
        assert_eq!(
            rest,
            [None, t("Check out a", true), t("Check out b", true), t("Check out c", true), t("Check out d", true), t("Check out e", true), t("Check out f", true), None, t("Show Debug Info", true)],
            "cut to 7, then the current branch skipped"
        );
        let with_empty: Vec<String> = ["", "x"].iter().map(|s| s.to_string()).collect();
        assert_eq!(titles(&git_menu(&clean, &with_empty)[6..9]), [None, t("Check out x", true), None]);

        let cmd = |m: &[Option<GitItem>], i: usize| m[i].as_ref().unwrap().command.clone();
        let m = git_menu(&clean, &branches);
        assert_eq!(cmd(&m, 0), GitCommand::Commit);
        assert_eq!(cmd(&m, 1), GitCommand::AddCommit);
        assert_eq!(cmd(&m, 4), GitCommand::Push("main".into()));
        assert_eq!(cmd(&m, 5), GitCommand::Pull("main".into()));
        assert_eq!(cmd(&m, 7), GitCommand::Checkout("a".into()));
        assert_eq!(cmd(&m, 14), GitCommand::Debug);

        let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(GitCommand::Commit.args(), s(&["commit"]));
        assert_eq!(GitCommand::AddCommit.args(), s(&["commit", "-a"]));
        assert_eq!(GitCommand::Stash.args(), s(&["stash"]));
        assert_eq!(GitCommand::Log.args(), s(&["log"]));
        assert_eq!(GitCommand::Push("m".into()).args(), s(&["push", "origin", "m"]));
        assert_eq!(GitCommand::Pull("m".into()).args(), s(&["pull", "origin", "m"]));
        assert_eq!(GitCommand::Checkout("b".into()).args(), s(&["checkout", "b"]));
        assert_eq!(GitCommand::Commit.status(), Some("Committing\u{2026}"));
        assert_eq!(GitCommand::AddCommit.status(), Some("Committing\u{2026}"));
        assert_eq!(GitCommand::Stash.status(), Some("Stashing\u{2026}"));
        assert_eq!(GitCommand::Push(String::new()).status(), Some("Pushing\u{2026}"));
        assert_eq!(GitCommand::Pull(String::new()).status(), Some("Pulling\u{2026}"));
        assert_eq!(GitCommand::Checkout(String::new()).status(), Some("Checking out\u{2026}"));
        assert_eq!((GitCommand::Log.status(), GitCommand::Debug.status()), (None, None));
        let buried: Vec<bool> = [GitCommand::Commit, GitCommand::AddCommit, GitCommand::Stash, GitCommand::Log, GitCommand::Push(String::new()), GitCommand::Pull(String::new()), GitCommand::Checkout(String::new()), GitCommand::Debug]
            .iter()
            .map(GitCommand::buried)
            .collect();
        assert_eq!(buried, [false, false, true, false, true, true, true, false]);

        // A branch named like an option is never offered, and never passed as one.
        let evil = GitState { branch: "--upload-pack=tools/evil".into(), ahead: "1".into(), ..Default::default() };
        let m = titles(&git_menu(&evil, &s(&["-x", "ok", "--exec=y"])));
        assert_eq!(m[4], t("Push origin --upload-pack=tools/evil", false));
        assert_eq!(m[5], t("Pull origin --upload-pack=tools/evil", false));
        assert_eq!(&m[6..9], [None, t("Check out ok", true), None]);
        assert_eq!(GitCommand::Pull("--upload-pack=x".into()).args(), s(&["pull", "origin", "refs/heads/--upload-pack=x"]));
        assert_eq!(GitCommand::Push("-x".into()).args(), s(&["push", "origin", "refs/heads/-x"]));
        assert_eq!(GitCommand::Checkout("-x".into()).args(), s(&["checkout", "refs/heads/-x"]));

        assert_eq!(cancel_title("Committing\u{2026}"), "Cancel Committing");
        assert_eq!(remote_command(&s(&["checkout", "my branch"])), "git checkout my\\ branch");
        assert_eq!(remote_command(&s(&["checkout", "a$b\nc"])), "git checkout a\\$b\\\nc");
    }

    #[test]
    fn remote_git_state_follows_iterm2() {
        let mut v: BTreeMap<String, String> = [("gitBranch", "main"), ("gitPushCount", "2"), ("gitPullCount", "0"), ("gitDirty", "dirty"), ("gitAdds", "3"), ("gitDeletes", "1")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let s = remote_state(&v).unwrap();
        assert_eq!(
            s,
            GitState { branch: "main".into(), ahead: "2".into(), behind: "0".into(), dirty: true, adds: 3, deletes: 1, ..Default::default() }
        );
        for (d, want) in [("clean", false), ("dirty", true), ("true", true), ("YES", true), ("1", true), ("no", false), ("0", false), ("", false), ("  t", true)] {
            v.insert("gitDirty".into(), d.into());
            assert_eq!(remote_state(&v).unwrap().dirty, want, "{d:?}");
        }
        for k in ["gitBranch", "gitPushCount", "gitPullCount", "gitDirty", "gitAdds", "gitDeletes"] {
            let mut w = v.clone();
            w.remove(k);
            assert_eq!(remote_state(&w), None, "{k} is required");
        }
        for (k, n) in [("gitLinesInserted", "10"), ("gitLinesDeleted", "4"), ("gitFilesAdded", "1"), ("gitFilesModified", " 2x"), ("gitFilesDeleted", "-3")] {
            v.insert(k.into(), n.into());
        }
        let s = remote_state(&v).unwrap();
        assert_eq!((s.lines_inserted, s.lines_deleted, s.files_added, s.files_modified, s.files_deleted), (10, 4, 1, 2, -3));
        v.insert("gitAdds".into(), "many".into());
        assert_eq!(remote_state(&v).unwrap().adds, 0, "integerValue of text is 0");
    }

    #[test]
    // The file-backed checks need real local paths: `local_path_ok` accepts only drive
    // paths, so on Linux every temp folder is refused (the suite runs on Windows).
    #[cfg_attr(not(windows), ignore = "needs a drive-letter temp folder")]
    fn hostile_repos_are_not_polled() {
        let refused = |name: &str, file: &str, text: &str| {
            let d = temp_repo(name);
            let p = d.join(".git").join(file);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, text).unwrap();
            let ok = repo_path_ok(&d);
            fs::remove_dir_all(&d).unwrap();
            !ok
        };
        // git-lfs, trusted globally, runs extension commands from the repository's config.
        assert!(refused("lfs-ext", "config", "[lfs \"extension.foo\"]\n\tclean = evil %f\n"));
        assert!(refused("lfs-legacy", "config", "[lfs.extension.bar]\n\tsmudge = evil\n"));
        let d = temp_repo("lfs-included");
        fs::write(d.join(".git").join("config"), "[include]\n\tpath = x.cfg\n").unwrap();
        fs::write(d.join(".git").join("x.cfg"), "[lfs \"Extension.z\"]\n\tclean = evil\n").unwrap();
        assert!(!repo_path_ok(&d), "an lfs extension in an included file");
        fs::remove_dir_all(&d).unwrap();
        assert!(!refused("lfs-url", "config", "[lfs]\n\turl = https://example.com/lfs\n"), "other lfs keys are fine");

        // Every spelling Windows opens as a share.
        for v in ["/\\\\server\\\\share\\\\a", "\\\\/server/share/a", "\\\\??\\\\UNC\\\\server\\\\s", "\\\\\\\\?\\\\UNC\\\\server\\\\s"] {
            assert!(refused("mixed", "config", &format!("[core]\n\tattributesFile = {v}\n")), "{v}");
        }
        assert!(refused("mixed-inc", "config", "[include]\n\tpath = /\\\\server/x\n"));
        assert!(refused("mixed-any", "config", "[remote \"x\"]\n\turl = \\\\/server/x\n"), "any value, not only path keys");
        assert!(!refused("local-paths", "config", "[core]\n\texcludesFile = ~/.gitignore_global\n\tattributesFile = .gitattributes-local\n\thooksPath = C:/hooks\n"));

        // A partial clone on git without GIT_NO_LAZY_FETCH is not polled; on 2.44 it is.
        let d = temp_repo("partial");
        fs::write(d.join(".git").join("config"), "[extensions]\n\tpartialClone = origin\n").unwrap();
        assert!(vet_repo(&d).unwrap().partial_clone);
        let old = Fake::new(|s| if s == "version" { exit(b"git version 2.43.0.windows.1\n") } else { exit(PORC) });
        let old_git = Path::new("C:\\old\\git.exe");
        assert_eq!(fetch_state(&old, old_git, &d, 4 * SEC), fetched(None, false));
        assert!(recent_branches(&old, old_git, &d, 4 * SEC).is_empty());
        assert_eq!(old.calls().iter().map(|(a, _)| sub(a)).collect::<Vec<_>>(), ["version"], "asked once, then remembered");
        let new = Fake::new(|s| if s == "version" { exit(b"git version 2.44.1\n") } else { exit(PORC) });
        assert_eq!(fetch_state(&new, Path::new("C:\\new\\git.exe"), &d, 4 * SEC), fetched(Some(main_state()), false));
        fs::write(d.join(".git").join("config"), "[remote \"origin\"]\n\tpromisor = true\n").unwrap();
        assert!(vet_repo(&d).unwrap().partial_clone);
        assert_eq!(fetch_state(&old, old_git, &d, 4 * SEC), fetched(None, false));
        let slow = Fake::new(|_| RunResult::TimedOut);
        assert_eq!(fetch_state(&slow, Path::new("C:\\slow\\git.exe"), &d, 4 * SEC), fetched(None, true));
        fs::write(d.join(".git").join("config"), "[core]\n\tbare = false\n").unwrap();
        assert!(!vet_repo(&d).unwrap().partial_clone);
        assert_eq!(parse_git_version(b"git version 2.45.2\n"), Some((2, 45)));
        assert_eq!(parse_git_version(b"nonsense"), None);
        fs::remove_dir_all(&d).unwrap();

        // Every git line carries the transport and virtual filesystem overrides.
        for base in [STATUS_ARGS, GIT_DIR_ARGS, BRANCHES_ARGS] {
            assert!(base.contains(&"protocol.ext.allow=never") && base.contains(&"core.virtualFilesystem="));
        }
    }

    /// A link inside `.git` to a share is refused before anything reads through it. Making
    /// a symbolic link needs Developer Mode or the privilege; without it that half is
    /// skipped. A junction (no privilege needed) to a local folder is fine.
    #[cfg(windows)]
    #[test]
    fn links_inside_git_dir_stay_local() {
        let d = temp_repo("links");
        let local = temp("links-target");
        let junction = d.join(".git").join("objects");
        let made = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&junction).arg(&local).output().unwrap();
        assert!(made.status.success(), "{made:?}");
        assert!(repo_path_ok(&d), "a local junction");
        fs::remove_dir(&junction).unwrap();
        if std::os::windows::fs::symlink_file("\\\\server\\share\\config", d.join(".git").join("config")).is_ok() {
            assert!(!repo_path_ok(&d), "config linked to a share");
            fs::remove_file(d.join(".git").join("config")).unwrap();
        }
        if std::os::windows::fs::symlink_dir("\\\\server\\share\\pack", d.join(".git").join("pack-link")).is_ok() {
            assert!(!repo_path_ok(&d), "any entry linked to a share");
        }
        let _ = fs::remove_dir_all(&d);
        fs::remove_dir_all(&local).unwrap();
    }

    /// A `HEAD` or `config` in the session folder (or a parent) that links to a share is
    /// refused before the bare-repository check opens it.
    #[cfg(windows)]
    #[test]
    fn bare_repo_names_linked_to_a_share_are_refused() {
        let d = temp("bare-links");
        fs::write(d.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        assert!(repo_path_ok(&d), "a plain HEAD without config is no repository");
        let sub = d.join("sub");
        fs::create_dir(&sub).unwrap();
        for name in ["HEAD", "config"] {
            let _ = fs::remove_file(d.join(name));
            if std::os::windows::fs::symlink_file("\\\\server\\share\\x", d.join(name)).is_ok() {
                assert!(!repo_path_ok(&d), "{name} linked to a share");
                assert!(!repo_path_ok(&sub), "{name} linked to a share in a parent");
            }
        }
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn looks_like_git_follows_iterm2() {
        assert!(looks_like_git("git status"));
        assert!(looks_like_git("/usr/bin/git log"));
        assert!(looks_like_git("C:\\Git\\cmd\\git.exe pull"));
        assert!(looks_like_git("GIT.EXE pull"));
        assert!(!looks_like_git("git"), "needs a space");
        assert!(!looks_like_git("gitk x"));
        assert!(!looks_like_git("echo git"));
        assert!(!looks_like_git(""));
    }

    // --- 13y1 Task 6: per-file statuses, the base and the diff stats (`iTermGitClient.m`) ---

    fn st(path: &str, index: Change, workdir: Change) -> FileStatus {
        FileStatus { path: path.into(), index, workdir }
    }

    /// The porcelain v2 `-z` list `git status --find-renames` prints, one record per entry.
    const V2: &[u8] = b"# branch.oid 1234567890abcdef1234567890abcdef12345678\0# branch.head main\0\
1 M. N... 100644 100644 100644 aaaa bbbb a.rs\0\
1 .M N... 100644 100644 100644 aaaa aaaa b.rs\0\
1 MM N... 100644 100644 100644 aaaa cccc c.rs\0\
2 R. N... 100644 100644 100644 aaaa aaaa R100 new.rs\0old.rs\0\
u UU N... 100644 100644 100644 100644 aaaa bbbb cccc d.rs\0";

    /// The per-file list follows `iTermGitFileStatusForEntry` (`GCL:198-269`): the index column from X,
    /// the workdir column from Y, an unmerged entry is Conflicted on the workdir side, a rename is its
    /// new path; the base's diff puts everything in the workdir column; the stats count as
    /// `populateDiffStatsOnState:` does (`GCL:477-557`); the cache key is `GPW:87-105`'s.
    #[test]
    fn file_statuses_follow_iterm2() {
        assert_eq!(
            parse_file_statuses(V2),
            vec![
                st("a.rs", Change::Modified, Change::None),
                st("b.rs", Change::None, Change::Modified),
                st("c.rs", Change::Modified, Change::Modified),
                st("new.rs", Change::Renamed, Change::None),
                st("d.rs", Change::None, Change::Conflicted),
            ]
        );
        assert_eq!(
            parse_file_statuses(b"1 A. N... 000000 100644 100644 0 b new file.rs\0 1 T. N... 100644 120000 120000 a b t.rs\0"),
            vec![st("new file.rs", Change::Added, Change::None)],
            "a path with a blank is whole; a malformed record is skipped"
        );
        assert_eq!(parse_file_statuses(b"1 D. N... 100644 000000 000000 a b gone.rs\0"), vec![st("gone.rs", Change::Deleted, Change::None)]);
        assert_eq!(parse_file_statuses(b"1 T. N... 100644 120000 120000 a b t.rs\0"), vec![st("t.rs", Change::TypeChange, Change::None)]);
        assert_eq!(parse_file_statuses(b"? untracked.rs\0"), Vec::new(), "untracked files are not listed");

        assert_eq!(
            parse_base_diff(b"M\0x.rs\0A\0y.rs\0R087\0o.rs\0n.rs\0"),
            vec![st("x.rs", Change::None, Change::Modified), st("y.rs", Change::None, Change::Added), st("n.rs", Change::None, Change::Renamed)]
        );
        assert_eq!(
            parse_base_diff(b"D\0d.rs\0T\0t.rs\0U\0u.rs\0C100\0from.rs\0copy.rs\0X\0x\0M\0m.rs\0"),
            vec![st("d.rs", Change::None, Change::Deleted), st("t.rs", Change::None, Change::TypeChange), st("u.rs", Change::None, Change::Conflicted), st("m.rs", Change::None, Change::Modified)],
            "a copy and an unknown letter are skipped"
        );

        assert_eq!(diff_stats(b"M\0a\0A\0b\0D\0c\0T\0d\0", b"3\t1\ta\010\t0\tb\01\t1\td\0", 2), (3, 2, 1, 4, 2));
        assert_eq!(diff_stats(b"M\0bin\0", b"-\t-\tbin\0", 0), (0, 1, 0, 0, 0), "a binary file adds no lines");
        assert_eq!(diff_stats(b"", b"", 5), (5, 0, 0, 0, 0), "untracked files count as added");

        assert_eq!(cache_key("C:\\r", None, false), "C:\\r");
        assert_eq!(cache_key("C:\\r", Some("HEAD"), true), "C:\\r\x01stats");
        assert_eq!(cache_key("C:\\r", Some(""), true), "C:\\r\x01stats");
        assert_eq!(cache_key("C:\\r", Some("main"), true), "C:\\r\x01stats\x02main");
        assert_eq!(cache_key("C:\\r", Some("main"), false), "C:\\r\x02main");

        for (name, args) in [("files", FILES_ARGS), ("base diff", BASE_DIFF_ARGS), ("numstat", NUMSTAT_ARGS), ("name-status", NAME_STATUS_ARGS), ("base tree", BASE_TREE_ARGS)] {
            assert!(args.starts_with(HARDENED), "{name} starts with the hardening");
            assert!(args.contains(&"--no-optional-locks") && args.contains(&"core.fsmonitor=false"), "{name}");
        }
        for (name, args) in [("base diff", BASE_DIFF_ARGS), ("numstat", NUMSTAT_ARGS), ("name-status", NAME_STATUS_ARGS)] {
            assert!(args.contains(&"--no-ext-diff") && args.contains(&"--no-textconv"), "{name} never runs a configured diff program");
        }
        assert!(FILES_ARGS.contains(&"--untracked-files=no") && FILES_ARGS.contains(&"--find-renames") && FILES_ARGS.contains(&"--porcelain=v2") && FILES_ARGS.contains(&"-z"));
        assert!(BASE_TREE_ARGS.ends_with(&["--end-of-options"]), "the typed base comes after --end-of-options");
        assert!(NUMSTAT_ARGS.contains(&"--no-renames") && NAME_STATUS_ARGS.contains(&"--no-renames"));
    }

    /// Output is bounded and a hostile list cannot panic the parsers.
    #[test]
    fn file_status_parsers_are_bounded() {
        let mut big = Vec::new();
        for i in 0..(MAX_FILES + 50) {
            big.extend_from_slice(format!("1 .M N... 100644 100644 100644 a b f{i}.rs\0").as_bytes());
        }
        assert_eq!(parse_file_statuses(&big).len(), MAX_FILES);
        let mut diff = Vec::new();
        for i in 0..(MAX_FILES + 50) {
            diff.extend_from_slice(format!("M\0f{i}.rs\0").as_bytes());
        }
        assert_eq!(parse_base_diff(&diff).len(), MAX_FILES);
        // A name with a control character, an empty or over-long name, or invalid UTF-8 is not listed.
        let long = "x".repeat(MAX_PATH + 1);
        let hostile = format!("1 .M N... 100644 100644 100644 a b a\x03b.rs\0\
1 .M N... 100644 100644 100644 a b {long}\0\
1 .M N... 100644 100644 100644 a b ok.rs\0");
        assert_eq!(parse_file_statuses(hostile.as_bytes()), vec![st("ok.rs", Change::None, Change::Modified)]);
        assert!(parse_file_statuses(b"1 .M N... 100644 100644 100644 a b \xff\xfe\0").is_empty());
        assert!(parse_base_diff(b"M\0a\rb\0R100\0only-one\0").is_empty());
        for junk in [&b""[..], b"\0\0\0", b"1", b"2 R. N... 1 1 1 a b R100 x\0", b"u UU", b"R\0", b"M"] {
            let _ = (parse_file_statuses(junk), parse_base_diff(junk), diff_stats(junk, junk, 0));
        }
    }

    /// The prefix of a key is its path: invalidating a path forgets every base and stats variant of it.
    #[test]
    fn invalidating_a_path_forgets_its_keys() {
        let now = Instant::now();
        let mut c = WorkerCache::default();
        let (p, other) = ("C:\\r", "C:\\r2");
        for key in [p.to_string(), cache_key(p, Some("main"), true), cache_key(p, None, true), cache_key(other, None, true)] {
            assert_eq!(c.lookup(&key, now), Lookup::Start);
            c.finish(&key, fetched(Some(main_state()), false), now);
        }
        c.invalidate(p);
        assert_eq!(c.lookup(p, now), Lookup::Start);
        assert_eq!(c.lookup(&cache_key(p, Some("main"), true), now), Lookup::Start);
        assert_eq!(c.lookup(&cache_key(p, None, true), now), Lookup::Start);
        assert!(matches!(c.lookup(&cache_key(other, None, true), now), Lookup::Cached(Some(_))), "another path (a longer name) keeps its state");
    }

    /// A `GitRun` that answers by the arguments themselves.
    struct Script {
        calls: Mutex<Vec<Vec<String>>>,
        reply: Box<dyn Fn(&[String]) -> RunResult + Send + Sync>,
    }

    impl GitRun for Script {
        fn run(&self, _git: &Path, _dir: &Path, args: &[String], _timeout: Duration, _cap: Option<usize>) -> RunResult {
            self.calls.lock().unwrap().push(args.to_vec());
            (self.reply)(args)
        }
    }

    fn has(args: &[String], word: &str) -> bool {
        args.iter().any(|a| a == word)
    }

    /// `fetch_state` stays today's two runs; with stats it adds the files pass and the two stats runs;
    /// a base is resolved to a tree first and never reaches git as an option; a failed or capped extra
    /// run leaves its field unset and keeps the rest of the state.
    #[test]
    fn fetch_state_with_asks_only_for_what_is_wanted() {
        let dir = temp_repo("wg-fetch");
        let tree = "0123456789abcdef0123456789abcdef01234567";
        let porcelain = [PORC, b"1 .M N... 100644 100644 100644 a b f.rs\0".as_slice()].concat();
        let script = |fail_numstat: bool| {
            let porcelain = porcelain.clone();
            Script {
                calls: Mutex::new(Vec::new()),
                reply: Box::new(move |a| {
                    if has(a, "status") && has(a, "--branch") {
                        exit(&porcelain)
                    } else if has(a, "status") {
                        exit(b"1 .M N... 100644 100644 100644 a b f.rs\0")
                    } else if has(a, "--absolute-git-dir") {
                        exit(b"C:\\nowhere\\.git\n")
                    } else if has(a, "--verify") {
                        exit(format!("{tree}\n").as_bytes())
                    } else if has(a, "--numstat") {
                        if fail_numstat { RunResult::Exit { code: 128, stdout: Vec::new(), capped: false } } else { exit(b"2\t1\tf.rs\0") }
                    } else if has(a, "--name-status") && has(a, "--find-renames") {
                        exit(b"M\0g.rs\0")
                    } else if has(a, "--name-status") {
                        exit(b"M\0f.rs\0")
                    } else {
                        exit(b"")
                    }
                }),
            }
        };
        let s = script(false);
        let plain = fetch_state(&s, Path::new(GIT), &dir, SEC);
        assert_eq!(plain.state.as_ref().and_then(|x| x.file_statuses.clone()), None, "no list unless asked");
        assert_eq!(s.calls.lock().unwrap().len(), 2, "today's callers run what they ran");

        let s = script(false);
        let got = fetch_state_with(&s, Path::new(GIT), &dir, SEC, None, true).state.unwrap();
        assert_eq!(got.file_statuses, Some(vec![st("f.rs", Change::None, Change::Modified)]));
        assert_eq!((got.files_added, got.files_modified, got.files_deleted, got.lines_inserted, got.lines_deleted), (0, 1, 0, 2, 1));
        assert!(!s.calls.lock().unwrap().iter().any(|a| has(a, "--verify")), "HEAD asks for no tree");

        // A base: its tree first, then the diff against that tree, every entry in the workdir column.
        let s = script(false);
        let got = fetch_state_with(&s, Path::new(GIT), &dir, SEC, Some("HEAD~1"), true).state.unwrap();
        assert_eq!(got.file_statuses, Some(vec![st("g.rs", Change::None, Change::Modified)]));
        let calls = s.calls.lock().unwrap().clone();
        let verify = calls.iter().find(|a| has(a, "--verify")).expect("rev-parse");
        assert_eq!(&verify[verify.len() - 2..], ["--end-of-options", "HEAD~1^{tree}"]);
        let diff = calls.iter().find(|a| has(a, "--find-renames") && has(a, "--name-status")).expect("diff");
        assert!(diff.contains(&tree.to_string()), "the diff names the tree, not the typed base: {diff:?}");

        // An option as a base never reaches git: the HEAD list stays.
        let s = script(false);
        let got = fetch_state_with(&s, Path::new(GIT), &dir, SEC, Some("-x"), true).state.unwrap();
        assert_eq!(got.file_statuses, Some(vec![st("f.rs", Change::None, Change::Modified)]));
        assert!(!s.calls.lock().unwrap().iter().any(|a| a.iter().any(|x| x.contains("-x"))), "{:?}", s.calls.lock().unwrap());

        // A failed stats run leaves the stats unset and the rest of the state.
        let s = script(true);
        let got = fetch_state_with(&s, Path::new(GIT), &dir, SEC, None, true).state.unwrap();
        assert_eq!(got.file_statuses, Some(vec![st("f.rs", Change::None, Change::Modified)]));
        assert_eq!((got.files_modified, got.lines_inserted), (0, 0));
        assert_eq!(got.branch, "main");
        let _ = fs::remove_dir_all(dir);
    }

    /// 13y1 Task 6 item 1 round 3: git, named as git or as a file that is git, starts only through
    /// `launch` (the vetted git, the overrides, the environment); any other program as it resolves.
    #[test]
    fn a_git_program_runs_only_through_the_launch() {
        let vet = Vetted::default();
        let git = Path::new(r"C:\Git\cmd\git.exe");
        let to = |p: &'static str| move |_: &Path| Some(PathBuf::from(p));
        let words = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        for (tokens, resolved) in [
            (words(&["git", "diff", "HEAD"]), r"C:\Git\cmd\git.exe"),
            (words(&[r"C:\Program Files\Git\cmd\GIT.EXE", "diff"]), r"C:\Program Files\Git\cmd\GIT.EXE"),
            (words(&["g", "diff"]), r"C:\tools\Git.exe"),
            (words(&["gitwrap", "status"]), r"C:\Git\bin\git"),
        ] {
            let (line, env) = pane_line(git, &vet, &tokens, &to(resolved)).expect("a launch");
            assert!(line.starts_with(r"C:\Git\cmd\git.exe "), "{tokens:?}: the vetted git, {line}");
            assert!(line.contains("--ignore-submodules=all") && line.contains("core.pager=cat"), "{tokens:?}: {line}");
            assert!(env.iter().any(|e| e == "-GIT_DIR") && env.iter().any(|e| e == "GIT_PAGER=cat"), "{tokens:?}: {env:?}");
        }
        let (line, env) = pane_line(git, &vet, &words(&["delta", "x"]), &to(r"C:\d\delta.exe")).unwrap();
        assert_eq!((line.as_str(), env.len()), (r"C:\d\delta.exe x", 0), "any other program as it resolves");
        assert!(pane_line(git, &vet, &words(&["nope"]), &|_: &Path| None).is_err());
        assert!(pane_line(git, &vet, &[], &to("x")).is_err());
    }

    /// 13y1 Task 6 item 1 round 3, against real git: a nested repository's own config (the agent can
    /// write it) never runs through a launched diff, difftool or status. Porcelain diff checks each
    /// submodule by running status inside it, which reads its config and runs its filter; a launch never
    /// recurses. The line and environment are exactly what `launch` makes.
    #[cfg(windows)]
    #[test]
    fn a_launch_never_recurses_into_a_submodule() {
        use std::process::Command;
        let path = std::env::var("PATH").unwrap_or_default();
        let Some(git) = find_git(&path, &crate::localfs::is_file) else {
            eprintln!("a_launch_never_recurses_into_a_submodule: git was not found; skipped");
            return;
        };
        let dir = temp("wg-submodule");
        let sub = dir.join("sub");
        fs::create_dir_all(&sub).unwrap();
        let g = |at: &Path, args: &[&str]| {
            let out = Command::new(&git).current_dir(at).args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"]).args(args).output().expect("git ran");
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        // A nested repository the agent wrote: its own config names a filter, its attributes use it.
        g(&sub, &["init", "-q"]);
        fs::write(sub.join("f.txt"), "hello\n").unwrap();
        fs::write(sub.join(".gitattributes"), "* filter=z\n").unwrap();
        g(&sub, &["add", "."]);
        g(&sub, &["commit", "-q", "-m", "s"]);
        g(&sub, &["config", "filter.z.clean", "echo ran > ../RAN; cat"]);
        g(&dir, &["init", "-q"]);
        fs::write(dir.join("a.txt"), "a\n").unwrap();
        g(&dir, &["add", "a.txt", "sub"]);
        g(&dir, &["commit", "-q", "-m", "p"]);
        // A same-size edit: git must read the content, which runs the clean filter.
        fs::write(sub.join("f.txt"), "jello\n").unwrap();
        let vet = vet_repo(&dir).expect("vetted");
        assert_eq!(vet.launch_refusal(), None, "the parent's own config is clean");
        let marker = dir.join("RAN");
        for args in [&["diff", "HEAD"][..], &["diff"], &["status"], &["difftool", "-y", "-x", "true", "HEAD"]] {
            let _ = fs::remove_file(&marker);
            let (line, env) = launch(&git, &vet, args).expect("a launch");
            let argv = crate::semantic::split_command_line(&line);
            let vars = crate::session_env::merge(std::env::vars().collect(), &crate::session_env::launch_last(Vec::new(), &env));
            let out = Command::new(&git).current_dir(&dir).args(&argv[1..]).env_clear().envs(vars).stdin(std::process::Stdio::null()).output().expect("git ran");
            assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
            assert!(!marker.exists(), "{args:?} ran the submodule's filter: {argv:?}");
        }
        let _ = fs::remove_dir_all(dir);
    }

    /// A real repository: staged, unstaged and untracked changes, the base and the stats against real
    /// git (R13xy-8). Skipped with a note when git is not found.
    #[cfg(windows)]
    #[test]
    fn base_and_stats_follow_iterm2() {
        use std::process::Command;
        let path = std::env::var("PATH").unwrap_or_default();
        let Some(git) = find_git(&path, &crate::localfs::is_file) else {
            eprintln!("base_and_stats_follow_iterm2: git was not found; skipped");
            return;
        };
        let dir = temp("wg-real");
        let g = |args: &[&str]| {
            let out = Command::new(&git).current_dir(&dir).args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"]).args(args).output().expect("git ran");
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        g(&["init", "-q"]);
        fs::write(dir.join("a.txt"), "one\n").unwrap();
        fs::write(dir.join("b.txt"), "one\ntwo\n").unwrap();
        g(&["add", "."]);
        g(&["commit", "-q", "-m", "first"]);
        fs::write(dir.join("a.txt"), "one\nstaged\n").unwrap();
        g(&["add", "a.txt"]);
        fs::write(dir.join("b.txt"), "changed\n").unwrap();
        fs::write(dir.join("u.txt"), "untracked\n").unwrap();
        let run = crate::git_run_win::WinGitRun;
        let timeout = Duration::from_secs(20);
        let got = fetch_state_with(&run, &git, &dir, timeout, None, true).state.expect("a state");
        assert_eq!(got.file_statuses, Some(vec![st("a.txt", Change::Modified, Change::None), st("b.txt", Change::None, Change::Modified)]), "no u.txt");
        assert_eq!((got.files_added, got.files_modified, got.files_deleted), (1, 2, 0), "the untracked file counts as added");
        assert_eq!((got.lines_inserted, got.lines_deleted), (2, 2), "a.txt +1, b.txt +1 -2... as git diff HEAD counts");
        let plain = fetch_state(&run, &git, &dir, timeout).state.expect("a state");
        assert_eq!(plain.file_statuses, None);

        g(&["add", "."]);
        g(&["commit", "-q", "-m", "second"]);
        fs::write(dir.join("b.txt"), "third\n").unwrap();
        let since = fetch_state_with(&run, &git, &dir, timeout, Some("HEAD~1"), true).state.expect("a state");
        assert_eq!(
            since.file_statuses,
            Some(vec![st("a.txt", Change::None, Change::Modified), st("b.txt", Change::None, Change::Modified), st("u.txt", Change::None, Change::Added)]),
            "the files changed since HEAD~1, all in the workdir column"
        );
        let nope = fetch_state_with(&run, &git, &dir, timeout, Some("nope"), true).state.expect("a state");
        assert_eq!(nope.file_statuses, Some(vec![st("b.txt", Change::None, Change::Modified)]), "an unresolved base keeps the HEAD list");
        let dash = fetch_state_with(&run, &git, &dir, timeout, Some("-x"), true).state.expect("a state");
        assert_eq!(dash.file_statuses, nope.file_statuses, "an option is no base");
        let _ = fs::remove_dir_all(dir);
    }
}

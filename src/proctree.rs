//! The foreground job. Windows has no process groups, so the job is the deepest
//! console program under the pane's shell (iTerm2 takes the deepest foreground process).


use std::collections::HashSet;

/// How deep the walk goes below the shell (iTerm2's cap).
const MAX_DEPTH: usize = 50;

/// Console hosts ConPTY starts; never a job.
const HOSTS: [&str; 2] = ["conhost", "openconsole"];

/// One process in a snapshot.
#[derive(Clone, Debug, PartialEq)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    /// The image name, as Toolhelp gives it ("vim.exe").
    pub name: String,
    /// Creation time (any monotonic unit); a child is never older than its parent.
    pub created: u64,
    /// A Windows-subsystem (GUI) program: not in the terminal, never the job.
    pub gui: bool,
    pub cmdline: Option<String>,
}

/// The job a session shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    /// `jobName`: the image name without ".exe".
    pub name: String,
    /// `processTitle`: argv0's file name without ".exe", else the name.
    pub title: String,
    /// `commandLine`: the full line, else the name.
    pub cmdline: String,
    pub pid: u32,
}

/// A name without a trailing ".exe" (any case).
pub fn strip_exe(name: &str) -> &str {
    let n = name.len();
    if n > 4 && name.is_char_boundary(n - 4) && name[n - 4..].eq_ignore_ascii_case(".exe") { &name[..n - 4] } else { name }
}

/// Whether two job names are the same program: case-insensitive, ".exe" optional.
pub fn same_job(a: &str, b: &str) -> bool {
    strip_exe(a.trim()).eq_ignore_ascii_case(strip_exe(b.trim()))
}

/// argv0's file name without ".exe" (quoted or not).
pub fn argv0_title(cmdline: &str) -> &str {
    let s = cmdline.trim_start();
    let argv0 = match s.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(rest),
        None => s.split(char::is_whitespace).next().unwrap_or(s),
    };
    strip_exe(argv0.rsplit(['\\', '/']).next().unwrap_or(argv0))
}

fn is_host(p: &Proc) -> bool {
    HOSTS.iter().any(|h| same_job(h, &p.name))
}

/// The processes under `root`, with their depth, parents before children. A child older
/// than its parent is a reused pid and not a child; a cycle is walked once.
fn walk(procs: &[Proc], root: u32) -> Vec<(&Proc, usize)> {
    let Some(r) = procs.iter().find(|p| p.pid == root) else { return Vec::new() };
    let mut seen = HashSet::from([root]);
    let mut out = vec![(r, 0)];
    let mut i = 0;
    while i < out.len() {
        let (parent, depth) = out[i];
        i += 1;
        // A GUI program's children (an editor's helpers) are not in this pane.
        if depth >= MAX_DEPTH || parent.gui {
            continue;
        }
        let mut kids: Vec<&Proc> = procs.iter().filter(|q| q.ppid == parent.pid && q.created >= parent.created && !seen.contains(&q.pid)).collect();
        kids.sort_by_key(|q| q.created);
        for k in kids {
            seen.insert(k.pid);
            out.push((k, depth + 1));
        }
    }
    out
}

fn job_of(p: &Proc) -> Job {
    let name = strip_exe(&p.name).to_string();
    let title = p.cmdline.as_deref().map(argv0_title).filter(|t| !t.is_empty()).map_or_else(|| name.clone(), str::to_string);
    Job { title, cmdline: p.cmdline.clone().unwrap_or_else(|| name.clone()), name, pid: p.pid }
}

fn deepest(procs: &[Proc], root: u32) -> Option<&Proc> {
    walk(procs, root).into_iter().filter(|(p, _)| !p.gui && !is_host(p)).max_by_key(|(p, d)| (*d, p.created)).map(|(p, _)| p)
}

/// The deepest console program under the shell (the shell itself when it has none);
/// the newest wins a tie. None when the shell is gone.
pub fn display_job(procs: &[Proc], root: u32) -> Option<Job> {
    deepest(procs, root).map(job_of)
}

/// `foregroundJobAncestors`: the job and its parents up to the shell (not included),
/// lowercased names without ".exe", deepest first.
pub fn ancestors(procs: &[Proc], root: u32) -> Vec<String> {
    let Some(mut p) = deepest(procs, root) else { return Vec::new() };
    let mut out = Vec::new();
    let mut guard = 0;
    while p.pid != root && guard <= MAX_DEPTH {
        out.push(strip_exe(&p.name).to_lowercase());
        match procs.iter().find(|q| q.pid == p.ppid) {
            Some(q) => p = q,
            None => break,
        }
        guard += 1;
    }
    out
}

/// Every process under the shell (not the shell), for the close prompt.
pub fn descendants(procs: &[Proc], root: u32) -> Vec<&Proc> {
    walk(procs, root).into_iter().skip(1).map(|(p, _)| p).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, ppid: u32, name: &str, created: u64) -> Proc {
        Proc { pid, ppid, name: name.into(), created, gui: false, cmdline: None }
    }

    #[test]
    fn deepest_console_descendant_wins() {
        let procs = vec![p(10, 1, "pwsh.exe", 100), p(11, 10, "cmd.exe", 110), p(12, 11, "vim.exe", 120), p(13, 10, "git.exe", 130)];
        let j = display_job(&procs, 10).unwrap();
        assert_eq!((j.name.as_str(), j.pid), ("vim", 12));
        let only = display_job(&[p(10, 1, "pwsh.exe", 100)], 10).unwrap();
        assert_eq!(only.name, "pwsh", "no children: the shell is the job");
        let tie = vec![p(10, 1, "pwsh.exe", 100), p(11, 10, "a.exe", 110), p(12, 10, "b.exe", 120)];
        assert_eq!(display_job(&tie, 10).unwrap().name, "b", "the newest wins a tie");
        assert_eq!(display_job(&tie, 99), None, "the shell is gone");
    }

    #[test]
    fn gui_children_are_not_the_job() {
        let mut np = p(11, 10, "notepad.exe", 110);
        np.gui = true;
        let procs = vec![p(10, 1, "cmd.exe", 100), np, p(12, 10, "conhost.exe", 105), p(13, 10, "OpenConsole.exe", 106)];
        assert_eq!(display_job(&procs, 10).unwrap().name, "cmd");
    }

    #[test]
    fn cycles_and_stale_parents() {
        // 11 and 12 name each other; 13 claims 10 as parent but is older (a reused pid).
        let procs = vec![p(10, 12, "cmd.exe", 100), p(11, 10, "a.exe", 110), p(12, 11, "b.exe", 120), p(13, 10, "old.exe", 50)];
        let j = display_job(&procs, 10).unwrap();
        assert_eq!(j.name, "b");
        let mut deep = vec![p(1000, 1, "sh.exe", 0)];
        for i in 1..200u32 {
            deep.push(p(1000 + i, 1000 + i - 1, "sh.exe", i as u64));
        }
        assert_eq!(display_job(&deep, 1000).unwrap().pid, 1050, "depth stops at 50");
    }

    #[test]
    fn ancestors_exclude_the_root() {
        let procs = vec![p(10, 1, "pwsh.exe", 100), p(11, 10, "CMD.exe", 110), p(12, 11, "Vim.exe", 120)];
        assert_eq!(ancestors(&procs, 10), vec!["vim", "cmd"]);
        assert!(ancestors(&procs[..1], 10).is_empty());
        let names: Vec<&str> = descendants(&procs, 10).iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["CMD.exe", "Vim.exe"]);
    }

    #[test]
    fn exe_is_stripped() {
        assert_eq!(strip_exe("Vim.EXE"), "Vim");
        assert_eq!(strip_exe("python3"), "python3");
        assert_eq!(argv0_title("\"C:\\Program Files\\Git\\usr\\bin\\vim.exe\" a.txt"), "vim");
        assert_eq!(argv0_title("python  -m x"), "python");
        let mut v = p(12, 10, "vim.exe", 120);
        v.cmdline = Some("vim.exe notes.md".into());
        let j = display_job(&[p(10, 1, "cmd.exe", 100), v], 10).unwrap();
        assert_eq!((j.title.as_str(), j.cmdline.as_str()), ("vim", "vim.exe notes.md"));
        assert!(same_job("VIM", "vim.exe") && same_job("vim.exe", "Vim") && !same_job("vi", "vim"));
    }

    #[test]
    fn programs_under_a_gui_program_are_not_the_job() {
        let mut ide = p(11, 10, "code.exe", 110);
        ide.gui = true;
        let procs = vec![p(10, 1, "pwsh.exe", 100), ide, p(12, 11, "git.exe", 120), p(13, 12, "ssh.exe", 130)];
        assert_eq!(display_job(&procs, 10).unwrap().name, "pwsh", "a GUI program's own console helpers are not in this pane");
        assert!(ancestors(&procs, 10).is_empty());
    }
}

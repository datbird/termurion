//! Reads the process tree for `proctree` from the platform's process table: creation times, the
//! GUI flag from each program's PE header, and command lines. A watcher thread checks the panes'
//! shells and wakes the window when a job changes, and reads each shell's console input mode for
//! password prompt detection (`pwm_prompt::at_prompt`).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::proctree::{self, Proc};
use crate::platform::proc_table::{self, Opened};

/// How often the watcher looks while Termurion is in front, and while it is not (iTerm2's
/// process cache rates).
pub const FAST: Duration = Duration::from_millis(500);
pub const SLOW: Duration = Duration::from_secs(5);
/// The most handles one wait takes (`WaitForMultipleObjects` allows 64; one is kept spare).
pub const MAX_WAIT: usize = 63;
/// The shortest time between two looks, so a handle that stays signaled (an exited process
/// something else still holds open) can never make the watcher spin.
const MIN_LOOK: Duration = Duration::from_millis(50);
/// Whether a program is a Windows-subsystem (GUI) program, from its PE header.
pub fn is_gui_exe(path: &str) -> bool {
    use std::io::Read;
    let mut head = [0u8; 1024];
    // The image path of a running process may be on a share: judged before it is opened.
    let n = crate::localfs::judge_io(path).and_then(|p| crate::localfs::open(&p)).and_then(|mut f| f.read(&mut head)).unwrap_or(0);
    subsystem(&head[..n]) == Some(2)
}

/// The PE optional header's Subsystem field (2 GUI, 3 console).
pub fn subsystem(head: &[u8]) -> Option<u16> {
    let at = |o: usize, n: usize| head.get(o..o + n);
    if at(0, 2)? != b"MZ" {
        return None;
    }
    let pe = u32::from_le_bytes(at(0x3C, 4)?.try_into().ok()?) as usize;
    if at(pe, 4)? != b"PE\0\0" {
        return None;
    }
    // The optional header starts 24 bytes in; Subsystem is at 68 for PE32 and PE32+.
    Some(u16::from_le_bytes(at(pe + 24 + 68, 2)?.try_into().ok()?))
}

/// Program facts kept between looks: the GUI flag by image path, and command lines by
/// (pid, creation time).
#[derive(Default)]
pub struct Cache {
    gui: HashMap<String, bool>,
    lines: HashMap<(u32, u64), Option<String>>,
}

impl Cache {
    /// Whether the program at `path` is a GUI program (read once per path; Windows only).
    pub fn is_gui(&mut self, path: &str) -> bool {
        *self.gui.entry(path.to_string()).or_insert_with(|| is_gui_exe(path))
    }
}

/// The processes under the given shells, with creation times, GUI flags, and the command
/// lines of each shell's job. Only processes under a shell are opened.
pub fn snapshot(roots: &[u32], cache: &mut Cache) -> Result<Vec<Proc>, String> {
    snapshot_trees(roots, &[], cache).map(|(procs, _)| procs)
}

/// Each shell's whole tree, the shell first.
pub type Trees = HashMap<u32, Vec<Proc>>;

/// `snapshot`, plus each shell in `trees` (one of `roots`) with every process under it, the
/// shell first, each with its command line (the Jobs tool's tree). Command lines are read once
/// per process, kept by pid and creation time while the process lives.
pub fn snapshot_trees(roots: &[u32], trees: &[u32], cache: &mut Cache) -> Result<(Vec<Proc>, Trees), String> {
    let all = proc_table::rows()?;
    let mut kids: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, r) in all.iter().enumerate() {
        if r.pid != r.ppid {
            kids.entry(r.ppid).or_default().push(i);
        }
    }
    let mut want: Vec<usize> = all.iter().enumerate().filter(|(_, r)| roots.contains(&r.pid)).map(|(i, _)| i).collect();
    let mut seen: HashSet<usize> = want.iter().copied().collect();
    let mut i = 0;
    while i < want.len() && want.len() < 4096 {
        for &k in kids.get(&all[want[i]].pid).map(Vec::as_slice).unwrap_or(&[]) {
            if seen.insert(k) {
                want.push(k);
            }
        }
        i += 1;
    }
    let mut procs = Vec::with_capacity(want.len());
    for i in want {
        let row = &all[i];
        let h = proc_table::open(row.pid);
        let made = h.as_ref().map_or(0, Opened::created);
        // The PE header check means nothing off Windows: skip even the image lookup.
        let gui = crate::platform::IS_WINDOWS && h.as_ref().and_then(Opened::image).is_some_and(|p| cache.is_gui(&p));
        procs.push(Proc { pid: row.pid, ppid: row.ppid, name: row.name.clone(), created: made, gui, cmdline: None, background: row.background });
    }
    let mut live = HashSet::new();
    let mut line_of = |p: &mut Proc, cache: &mut Cache| {
        let key = (p.pid, p.created);
        live.insert(key);
        cache.lines.entry(key).or_insert_with(|| proc_table::open(p.pid).and_then(|h| h.cmdline()));
        p.cmdline = cache.lines[&key].clone();
    };
    for &root in roots {
        let Some(job) = proctree::display_job(&procs, root) else { continue };
        let Some(p) = procs.iter_mut().find(|p| p.pid == job.pid) else { continue };
        line_of(p, cache);
    }
    let mut out = HashMap::new();
    for &root in trees {
        let Some(shell) = procs.iter().find(|p| p.pid == root).cloned() else { continue };
        let mut tree: Vec<Proc> = std::iter::once(shell).chain(proctree::descendants(&procs, root).into_iter().cloned()).collect();
        for p in &mut tree {
            line_of(p, cache);
        }
        out.insert(root, tree);
    }
    cache.lines.retain(|k, _| live.contains(k));
    Ok((procs, out))
}

/// The processes the watcher waits on: with the key on, each watched shell's foreground job
/// (the shell itself when it runs the job), sorted, once each, at most `MAX_WAIT`; off, none.
pub fn wait_pids(jobs: &HashMap<u32, ShellJob>, on: bool) -> Vec<u32> {
    if !on {
        return Vec::new();
    }
    let mut pids: Vec<u32> = jobs.values().filter_map(|j| j.job.as_ref().map(|j| j.pid)).collect();
    pids.sort_unstable();
    pids.dedup();
    pids.truncate(MAX_WAIT);
    pids
}

/// The names an ancestry lost, in the old order (`PC:479-500`); growth or no change is empty.
pub fn ancestry_shrank(old: &[String], new: &[String]) -> Vec<String> {
    old.iter().filter(|n| !new.contains(n)).cloned().collect()
}

/// Logs a shrunken ancestry through `rlog!`: the line, then per vanished name whether a process
/// of that name is still in the process list. Names are program names, never command lines.
fn log_ancestry_shrink(procs: &[Proc], root: u32, old: &[String], now: &ShellJob) {
    let removed = ancestry_shrank(old, &now.ancestors);
    if removed.is_empty() {
        return;
    }
    let deepest = now.job.as_ref().map_or("nil".to_string(), |j| j.pid.to_string());
    crate::rlog!(
        "[ANCESTRYDIAG] tracked pid {}: ancestry shrank {} -> {} (removed {}); deepest fg job pid={}",
        root,
        crate::dlog::names(old),
        crate::dlog::names(&now.ancestors),
        crate::dlog::names(&removed),
        crate::dlog::name(&deepest)
    );
    for gone in &removed {
        let alive = procs.iter().any(|p| proctree::strip_exe(&p.name).eq_ignore_ascii_case(gone));
        crate::rlog!("[ANCESTRYDIAG] vanished {}: still in the process list: {}", crate::dlog::name(gone), alive);
    }
}

pub use crate::proctree::ShellJob;

#[derive(Default)]
struct Shared {
    roots: HashSet<u32>,
    jobs: HashMap<u32, ShellJob>,
    fast: bool,
    /// `FastForegroundJobUpdates`: the wait between looks ends when a watched foreground job
    /// exits. Off, the watcher only polls.
    fast_updates: bool,
    /// `LogForegroundJobAncestryDiagnostics`: a shrinking ancestry is logged (`PC:470-500`).
    ancestry_diagnostics: bool,
    error: Option<String>,
    /// Shells whose whole tree the Jobs tool shows, and each one's last tree.
    trees_wanted: HashSet<u32>,
    trees: HashMap<u32, Vec<Proc>>,
    /// Bumped whenever a kept tree changes (the Jobs tool refreshes on it).
    tree_gen: u64,
    /// Password prompt detection's debounce while it is on (`detectPasswordInput`,
    /// `detectPasswordInputDebounce`): each local shell's console input mode is read at every
    /// look. None: off, nothing is attached to.
    read_debounce: Option<f64>,
    /// The shells whose consoles Termurion owns (local panes; termurion-host reads its own), each
    /// with its pane's pause flag: a paused console is never read.
    consoles: HashMap<u32, Arc<std::sync::atomic::AtomicBool>>,
    /// Each local shell's samples, debounced. A console whose mode could not be read is not
    /// at a prompt.
    prompts: HashMap<u32, crate::pwm_prompt::Debounce>,
}

/// The consoles a look reads: those not paused (Undo Close, an inline file question), each
/// with its flag for the check under `CONSOLE`.
pub(crate) fn console_targets(consoles: &HashMap<u32, Arc<std::sync::atomic::AtomicBool>>) -> Vec<(u32, Arc<std::sync::atomic::AtomicBool>)> {
    consoles.iter().filter(|(_, p)| !p.load(std::sync::atomic::Ordering::Acquire)).map(|(pid, p)| (*pid, p.clone())).collect()
}

/// The watcher: a thread that looks at the registered shells' trees and calls `notify`
/// when a job changes.
#[derive(Clone)]
pub struct JobWatch {
    shared: Arc<Mutex<Shared>>,
}

impl JobWatch {
    pub fn start(notify: Arc<dyn Fn() + Send + Sync>) -> JobWatch {
        let shared = Arc::new(Mutex::new(Shared { fast: true, fast_updates: true, ..Default::default() }));
        let weak = Arc::downgrade(&shared);
        std::thread::spawn(move || {
            let mut cache = Cache::default();
            loop {
                let Some(shared) = weak.upgrade() else { return };
                let (roots, trees, fast, shells, waits) = {
                    let s = shared.lock().unwrap_or_else(|e| e.into_inner());
                    let trees: Vec<u32> = s.trees_wanted.iter().copied().collect();
                    let roots: Vec<u32> = s.roots.union(&s.trees_wanted).copied().collect();
                    let shells = if s.read_debounce.is_some() { console_targets(&s.consoles) } else { Vec::new() };
                    (roots, trees, s.fast, shells, wait_pids(&s.jobs, s.fast_updates))
                };
                drop(shared);
                if !shells.is_empty() {
                    // Each sample with its own time. Unknown (the mode could not be read) is
                    // never a password prompt; nor is one paused since it was picked (refused
                    // under `CONSOLE`, so no read runs on a paused console).
                    let samples: Vec<(u32, bool, std::time::Instant)> = shells.iter().map(|(pid, paused)| (*pid, crate::platform::pty::console_mode_unless(*pid, paused).is_ok_and(crate::pwm_prompt::at_prompt), std::time::Instant::now())).collect();
                    let Some(shared) = weak.upgrade() else { return };
                    let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
                    let mut changed = false;
                    if let Some(debounce) = s.read_debounce {
                        for (pid, prompt, at) in samples {
                            // A shell forgotten while it was read stays forgotten.
                            if s.consoles.contains_key(&pid) {
                                changed |= s.prompts.entry(pid).or_default().sample(prompt, at, debounce).is_some();
                            }
                        }
                    }
                    drop(s);
                    if changed {
                        notify();
                    }
                }
                if !roots.is_empty() {
                    let result = snapshot_trees(&roots, &trees, &mut cache);
                    let Some(shared) = weak.upgrade() else { return };
                    let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
                    let mut changed = false;
                    match result {
                        Ok((procs, got)) => {
                            s.error = None;
                            for r in &trees {
                                // A tree dropped while this snapshot ran stays dropped.
                                if !s.trees_wanted.contains(r) {
                                    continue;
                                }
                                let now = got.get(r).cloned().unwrap_or_default();
                                if s.trees.get(r) != Some(&now) {
                                    s.trees.insert(*r, now);
                                    s.tree_gen = s.tree_gen.wrapping_add(1);
                                }
                            }
                            for r in roots {
                                let now = ShellJob { job: proctree::display_job(&procs, r), ancestors: proctree::ancestors(&procs, r), root: proctree::root_name(&procs, r) };
                                if s.roots.contains(&r) && s.jobs.get(&r) != Some(&now) {
                                    if s.ancestry_diagnostics
                                        && let Some(old) = s.jobs.get(&r) {
                                            log_ancestry_shrink(&procs, r, &old.ancestors, &now);
                                        }
                                    s.jobs.insert(r, now);
                                    changed = true;
                                }
                            }
                        }
                        Err(e) => {
                            changed = s.error.as_ref() != Some(&e);
                            s.error = Some(e);
                        }
                    }
                    drop(s);
                    if changed {
                        notify();
                    }
                }
                // A job that ends wakes the next look at once (iTerm2's process exit source);
                // a job that starts is found by the look after the plain gap.
                let began = std::time::Instant::now();
                proc_table::wait_any(&waits, if fast { FAST } else { SLOW });
                if let Some(rest) = MIN_LOOK.checked_sub(began.elapsed()) {
                    std::thread::sleep(rest);
                }
            }
        });
        JobWatch { shared }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn watch(&self, shell: u32) {
        self.lock().roots.insert(shell);
    }

    pub fn forget(&self, shell: u32) {
        let mut s = self.lock();
        s.roots.remove(&shell);
        s.jobs.remove(&shell);
        s.prompts.remove(&shell);
        s.consoles.remove(&shell);
        s.trees_wanted.remove(&shell);
        if s.trees.remove(&shell).is_some() {
            s.tree_gen = s.tree_gen.wrapping_add(1);
        }
    }

    /// The Jobs tool shows (or stops showing) this shell's tree: the watcher keeps the whole
    /// tree, command lines included, from its snapshots.
    pub fn set_tree_wanted(&self, shell: u32, on: bool) {
        let mut s = self.lock();
        if on {
            s.trees_wanted.insert(shell);
        } else {
            s.trees_wanted.remove(&shell);
            if s.trees.remove(&shell).is_some() {
                s.tree_gen = s.tree_gen.wrapping_add(1);
            }
        }
    }

    /// The shell and every process under it, from the last snapshot (empty until the watcher
    /// has looked, or when the tree is not wanted). The UI thread never snapshots.
    pub fn tree(&self, shell: u32) -> Vec<Proc> {
        self.lock().trees.get(&shell).cloned().unwrap_or_default()
    }

    /// Changes whenever a kept tree changed.
    pub fn tree_gen(&self) -> u64 {
        self.lock().tree_gen
    }

    /// A local pane's shell: Termurion owns its console (and every close of it), so the watcher
    /// may read its input mode while `paused` (its pane's pause flag) is clear. A hosted
    /// shell's console is never attached to here.
    pub fn watch_console(&self, shell: u32, paused: Arc<std::sync::atomic::AtomicBool>) {
        self.lock().consoles.insert(shell, paused);
    }

    /// Password prompt detection on with its debounce (seconds), or off (None): off, no
    /// console is attached to and every shell reads as not at a prompt.
    pub fn set_read_modes(&self, debounce: Option<f64>) {
        let mut s = self.lock();
        s.read_debounce = debounce;
        if debounce.is_none() {
            s.prompts.clear();
        }
    }

    /// The local shell's console is at a password prompt by its debounced samples (echo off,
    /// line input on); false when unknown.
    pub fn password_input(&self, shell: u32) -> bool {
        self.lock().prompts.get(&shell).is_some_and(|d| d.on)
    }

    /// `LogForegroundJobAncestryDiagnostics`, read by the watcher at each look.
    pub fn set_ancestry_diagnostics(&self, on: bool) {
        self.lock().ancestry_diagnostics = on;
    }

    /// `FastForegroundJobUpdates`: on, a watched foreground job's exit ends the wait at once;
    /// off, plain polling from the next look.
    pub fn set_fast_updates(&self, on: bool) {
        self.lock().fast_updates = on;
    }

    /// Termurion in front: look every 0.5 s; else every 5 s.
    pub fn set_fast(&self, fast: bool) {
        self.lock().fast = fast;
    }

    pub fn job(&self, shell: u32) -> ShellJob {
        self.lock().jobs.get(&shell).cloned().unwrap_or_default()
    }

    /// The last snapshot error, shown once by the window.
    pub fn error(&self) -> Option<String> {
        self.lock().error.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proctree::Job;

    /// `PC:479-500`: the names that vanished from a shrinking ancestry, and the one log line.
    #[test]
    fn ancestry_diagnostics_follow_iterm2() {
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(ancestry_shrank(&names(&["claude", "node", "pwsh"]), &names(&["node", "pwsh"])), names(&["claude"]));
        assert!(ancestry_shrank(&names(&["node"]), &names(&["claude", "node"])).is_empty(), "growth is not a shrink");
        assert!(ancestry_shrank(&names(&["node"]), &names(&["node"])).is_empty());
        assert_eq!(ancestry_shrank(&names(&["claude", "pwsh"]), &names(&["node", "pwsh"])), names(&["claude"]), "a swap of equal length loses a name too: iTerm2 has no length guard");
        // The line itself is made by `rlog!` from log-safe parts (`dlog::names`).
        assert_eq!(crate::dlog::names(&names(&["claude", "node"])).to_string(), "[\"claude\", \"node\"]");
        assert_eq!(crate::dlog::names(&[]).to_string(), "[]");
    }

    #[test]
    fn pe_subsystem_reads_the_header() {
        let mut h = vec![0u8; 400];
        h[0..2].copy_from_slice(b"MZ");
        h[0x3C..0x40].copy_from_slice(&128u32.to_le_bytes());
        h[128..132].copy_from_slice(b"PE\0\0");
        h[128 + 24 + 68..128 + 24 + 70].copy_from_slice(&2u16.to_le_bytes());
        assert_eq!(subsystem(&h), Some(2));
        assert_eq!(subsystem(b"not a program"), None);
    }

    fn job_of(pid: u32) -> ShellJob {
        ShellJob { job: Some(Job { name: String::new(), title: String::new(), cmdline: String::new(), pid }), ..Default::default() }
    }

    /// Review Focus 9: the wait set follows the rule.
    #[test]
    fn wait_pids_follow_the_rule() {
        let jobs: HashMap<u32, ShellJob> = [(10, job_of(11)), (20, job_of(20)), (30, ShellJob::default())].into_iter().collect();
        assert_eq!(wait_pids(&jobs, true), vec![11, 20]);
        assert_eq!(wait_pids(&jobs, false), Vec::<u32>::new());
        let many: HashMap<u32, ShellJob> = (0..100u32).map(|i| (1000 + i, job_of(500 - i))).collect();
        let got = wait_pids(&many, true);
        assert_eq!(got.len(), MAX_WAIT);
        assert_eq!(got, (0..MAX_WAIT as u32).map(|i| 401 + i).collect::<Vec<_>>(), "the smallest 63");
        let same: HashMap<u32, ShellJob> = [(1, job_of(7)), (2, job_of(7))].into_iter().collect();
        assert_eq!(wait_pids(&same, true), vec![7], "once each");
    }

    /// 13p S9 x 13q B7: a forgotten shell (an ended pane's) leaves the wait set at once, so no
    /// signaled handle of a dead shell wakes every look and no reused pid is waited on.
    #[test]
    fn a_forgotten_shell_leaves_the_wait_set() {
        let w = JobWatch { shared: Arc::new(Mutex::new(Shared { fast_updates: true, ..Default::default() })) };
        w.watch(10);
        w.watch(20);
        {
            let mut s = w.lock();
            s.jobs.insert(10, job_of(10));
            s.jobs.insert(20, job_of(21));
        }
        assert_eq!(wait_pids(&w.lock().jobs, true), vec![10, 21]);
        w.forget(10);
        assert_eq!(wait_pids(&w.lock().jobs, true), vec![21], "the ended shell is not waited on");
        assert!(!w.lock().roots.contains(&10), "nor looked at again");
        assert_eq!(w.job(10), ShellJob::default(), "nor reported");
    }
}

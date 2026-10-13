//! Reads the process tree from Windows for `proctree`: a Toolhelp snapshot, creation
//! times, the GUI flag from each program's PE header, and command lines. A watcher thread
//! checks the panes' shells and wakes the window when a job changes, and reads each shell's
//! console input mode for password prompt detection (`pwm_prompt::at_prompt`).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE, STILL_ACTIVE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, QueryFullProcessImageNameW, TerminateProcess, WaitForMultipleObjects};

use crate::proctree::{self, Job, Proc};
use crate::toolbelt_tools::SignalWay;

/// How often the watcher looks while Termurion is in front, and while it is not (iTerm2's
/// process cache rates).
pub const FAST: Duration = Duration::from_millis(500);
pub const SLOW: Duration = Duration::from_secs(5);
/// The most handles one wait takes (`WaitForMultipleObjects` allows 64; one is kept spare).
pub const MAX_WAIT: usize = 63;
/// The shortest time between two looks, so a handle that stays signaled (an exited process
/// something else still holds open) can never make the watcher spin.
const MIN_LOOK: Duration = Duration::from_millis(50);
const SYNCHRONIZE: u32 = 0x0010_0000;

fn os_err(what: &str) -> String {
    format!("{what} failed: {}", std::io::Error::last_os_error())
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

fn open(pid: u32) -> Option<Handle> {
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    (!h.is_null()).then_some(Handle(h))
}

/// Every process's pid, parent, and image name.
fn toolhelp() -> Result<Vec<(u32, u32, String)>, String> {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return Err(os_err("CreateToolhelp32Snapshot"));
        }
        let snap = Handle(snap);
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut out = Vec::new();
        let mut ok = Process32FirstW(snap.0, &mut e);
        while ok != 0 {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            out.push((e.th32ProcessID, e.th32ParentProcessID, String::from_utf16_lossy(&e.szExeFile[..len])));
            ok = Process32NextW(snap.0, &mut e);
        }
        Ok(out)
    }
}

fn created(h: &Handle) -> u64 {
    let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut c, mut x, mut k, mut u) = (z, z, z, z);
    if unsafe { GetProcessTimes(h.0, &mut c, &mut x, &mut k, &mut u) } == 0 {
        return 0;
    }
    ((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64
}

fn image_path(h: &Handle) -> Option<String> {
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    (unsafe { QueryFullProcessImageNameW(h.0, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) } != 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
}

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

/// A process's command line (NtQueryInformationProcess, ProcessCommandLineInformation).
fn command_line(h: &Handle) -> Option<String> {
    type Query = unsafe extern "system" fn(HANDLE, u32, *mut core::ffi::c_void, u32, *mut u32) -> i32;
    let ntdll: Vec<u16> = "ntdll.dll".encode_utf16().chain(Some(0)).collect();
    let f = unsafe { GetProcAddress(GetModuleHandleW(ntdll.as_ptr()), c"NtQueryInformationProcess".as_ptr().cast())? };
    let query: Query = unsafe { std::mem::transmute(f) };
    let mut size = 1024u32;
    for _ in 0..4 {
        // A UNICODE_STRING (length, max length, pointer) followed by the text it points to.
        let mut buf = vec![0u64; size as usize / 8 + 1];
        let mut need = 0u32;
        let status = unsafe { query(h.0, 60, buf.as_mut_ptr().cast(), (buf.len() * 8) as u32, &mut need) };
        if status == 0 {
            let len = (buf[0] & 0xFFFF) as usize / 2;
            let ptr = buf[1] as *const u16;
            if ptr.is_null() {
                return None;
            }
            // SAFETY: the pointer is into `buf`, which the call filled.
            let text = unsafe { std::slice::from_raw_parts(ptr, len) };
            return Some(String::from_utf16_lossy(text));
        }
        // STATUS_INFO_LENGTH_MISMATCH and friends: grow and retry.
        size = need.max(size * 2);
    }
    None
}

/// Program facts kept between looks: the GUI flag by image path, and command lines by
/// (pid, creation time).
#[derive(Default)]
pub struct Cache {
    gui: HashMap<String, bool>,
    lines: HashMap<(u32, u64), Option<String>>,
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
    let all = toolhelp()?;
    let mut kids: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, (pid, ppid, _)) in all.iter().enumerate() {
        if pid != ppid {
            kids.entry(*ppid).or_default().push(i);
        }
    }
    let mut want: Vec<usize> = all.iter().enumerate().filter(|(_, (pid, _, _))| roots.contains(pid)).map(|(i, _)| i).collect();
    let mut seen: HashSet<usize> = want.iter().copied().collect();
    let mut i = 0;
    while i < want.len() && want.len() < 4096 {
        for &k in kids.get(&all[want[i]].0).map(Vec::as_slice).unwrap_or(&[]) {
            if seen.insert(k) {
                want.push(k);
            }
        }
        i += 1;
    }
    let mut procs = Vec::with_capacity(want.len());
    for i in want {
        let (pid, ppid, ref name) = all[i];
        let h = open(pid);
        let made = h.as_ref().map_or(0, created);
        let gui = h.as_ref().and_then(image_path).is_some_and(|p| *cache.gui.entry(p.clone()).or_insert_with(|| is_gui_exe(&p)));
        procs.push(Proc { pid, ppid, name: name.clone(), created: made, gui, cmdline: None });
    }
    let mut live = HashSet::new();
    let mut line_of = |p: &mut Proc, cache: &mut Cache| {
        let key = (p.pid, p.created);
        live.insert(key);
        if !cache.lines.contains_key(&key) {
            let line = open(p.pid).and_then(|h| command_line(&h));
            cache.lines.insert(key, line);
        }
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

/// Waits until any of `pids` exits or `gap` passes. Handles are opened fresh and closed on
/// return, so no pid is held past its process's end; a pid that cannot be opened (gone, or
/// not ours) is skipped, and with no handle this is a plain sleep.
pub fn wait_any(pids: &[u32], gap: Duration) {
    let handles: Vec<HANDLE> = pids.iter().take(MAX_WAIT).filter_map(|&pid| {
        let h = unsafe { OpenProcess(SYNCHRONIZE, 0, pid) };
        (!h.is_null()).then_some(h)
    }).collect();
    if handles.is_empty() {
        std::thread::sleep(gap);
        return;
    }
    let ms = gap.as_millis().min(u128::from(u32::MAX - 1)) as u32;
    unsafe {
        WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, ms);
        for h in handles {
            CloseHandle(h);
        }
    }
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

/// What the window knows about one shell's job.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShellJob {
    pub job: Option<Job>,
    pub ancestors: Vec<String>,
    /// The shell's own name (`proctree::root_name`); None once it is gone.
    pub root: Option<String>,
}

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
fn console_targets(consoles: &HashMap<u32, Arc<std::sync::atomic::AtomicBool>>) -> Vec<(u32, Arc<std::sync::atomic::AtomicBool>)> {
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
                    let samples: Vec<(u32, bool, std::time::Instant)> = shells.iter().map(|(pid, paused)| (*pid, crate::pty::console_mode_unless(*pid, paused).is_ok_and(crate::pwm_prompt::at_prompt), std::time::Instant::now())).collect();
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
                                    if s.ancestry_diagnostics {
                                        if let Some(old) = s.jobs.get(&r) {
                                            log_ancestry_shrink(&procs, r, &old.ancestors, &now);
                                        }
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
                wait_any(&waits, if fast { FAST } else { SLOW });
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

/// Whether the process `pid` still runs and is the one created at `created` (a reused pid is
/// another process). A cheap open, not a snapshot.
pub fn process_alive(pid: u32, created: u64) -> bool {
    let Some(h) = open(pid) else { return false };
    let mut code = 0u32;
    let running = unsafe { GetExitCodeProcess(h.0, &mut code) } != 0 && code == STILL_ACTIVE as u32;
    running && self::created(&h) == created
}

/// A signal sent, or on its way.
pub enum Sent {
    Done,
    /// A console event, sent from its own thread (the attach waits out any pseudoconsole
    /// close, up to 2 s): its result, for the caller to collect when the thread ends.
    Later(std::thread::JoinHandle<Result<(), String>>),
}

/// Sends one of the Jobs tool's signals (`toolbelt_tools::signal_way`) to `pid`, only to the
/// process created at `created` when given (a pid the tree showed may have been reused
/// since): `Terminate(code)` opens the process for termination and ends it with that exit
/// code; `CtrlC` and `CtrlBreak` are console events sent to the whole console `pid` runs on,
/// from a thread that attaches to it for the moment. Only for a console this process owns
/// the closes of (a local pane): a hosted pane's goes through termurion-host. `stop` is the
/// pane's pause flag: a console paused (for an inline file's question or Undo Close) is
/// never attached to.
pub fn signal_process(pid: u32, created: Option<u64>, way: SignalWay, stop: Arc<std::sync::atomic::AtomicBool>) -> Result<Sent, String> {
    let access = match way {
        SignalWay::Terminate(_) => PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
        SignalWay::CtrlC | SignalWay::CtrlBreak => PROCESS_QUERY_LIMITED_INFORMATION,
    };
    let h = unsafe { OpenProcess(access, 0, pid) };
    if h.is_null() {
        return Err(os_err("OpenProcess"));
    }
    let h = Handle(h);
    if created.is_some_and(|c| self::created(&h) != c) {
        return Err(format!("process {pid} has already ended"));
    }
    match way {
        SignalWay::Terminate(code) => {
            if unsafe { TerminateProcess(h.0, code) } == 0 {
                return Err(os_err("TerminateProcess"));
            }
            Ok(Sent::Done)
        }
        SignalWay::CtrlC | SignalWay::CtrlBreak => Ok(Sent::Later(console_event(pid, way == SignalWay::CtrlBreak, stop))),
    }
}

/// How long a console event's sender stays attached waiting for its own copy of the event.
const OWN_EVENT_WAIT: Duration = Duration::from_millis(500);

/// Ctrl+C (or Ctrl+Break) to the whole console `pid` runs on, from its own thread, never
/// joined by the caller: attached under `pty::CONSOLE`, and not at all once `stop` is set
/// (a session paused for Undo Close). It stays attached, holding the lock, until this
/// process's own handler has seen the event (at most `OWN_EVENT_WAIT`), so the copy Windows
/// delivers to the sender never lands after a later attach reset the handler list.
pub fn console_event(pid: u32, ctrl_break: bool, stop: Arc<std::sync::atomic::AtomicBool>) -> std::thread::JoinHandle<Result<(), String>> {
    std::thread::spawn(move || crate::pty::attached_unless(pid, &stop, || crate::pty::send_ctrl_event_attached(ctrl_break, OWN_EVENT_WAIT).map(|_| ())))
}

/// Every process under a shell now (for the close prompt), with its name.
pub fn descendants_now(shell: u32) -> Result<Vec<Proc>, String> {
    let procs = snapshot(&[shell], &mut Cache::default())?;
    Ok(proctree::descendants(&procs, shell).into_iter().cloned().collect())
}

/// The processes still alive under a shell that already exited (its children and theirs, found
/// by the parent id they kept), with creation times: what a `start /b` left attached to the
/// console. Console hosts are not programs and are left out.
pub fn orphans_of(dead: u32) -> Result<Vec<Proc>, String> {
    let all = toolhelp()?;
    let mut kids: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, (pid, ppid, _)) in all.iter().enumerate() {
        if pid != ppid {
            kids.entry(*ppid).or_default().push(i);
        }
    }
    let mut want: Vec<usize> = kids.get(&dead).cloned().unwrap_or_default();
    let mut seen: HashSet<usize> = want.iter().copied().collect();
    let mut i = 0;
    while i < want.len() && want.len() < 4096 {
        for &k in kids.get(&all[want[i]].0).map(Vec::as_slice).unwrap_or(&[]) {
            if seen.insert(k) {
                want.push(k);
            }
        }
        i += 1;
    }
    let mut cache = Cache::default();
    let mut out = Vec::new();
    for i in want {
        let (pid, ppid, ref name) = all[i];
        let bare = proctree::strip_exe(name);
        if bare.eq_ignore_ascii_case("conhost") || bare.eq_ignore_ascii_case("OpenConsole") {
            continue;
        }
        let h = open(pid);
        let made = h.as_ref().map_or(0, created);
        let gui = h.as_ref().and_then(image_path).is_some_and(|p| *cache.gui.entry(p.clone()).or_insert_with(|| is_gui_exe(&p)));
        out.push(Proc { pid, ppid, name: name.clone(), created: made, gui, cmdline: None });
    }
    Ok(out)
}

/// The names (no ".exe") of those of `set` ((pid, creation time)) that still run: a reused pid
/// has another creation time and is not counted.
pub fn still_running(set: &[(u32, u64)]) -> Vec<String> {
    let Ok(all) = toolhelp() else { return Vec::new() };
    set.iter()
        .filter_map(|(pid, made)| {
            let (_, _, name) = all.iter().find(|(p, _, _)| p == pid)?;
            let h = open(*pid)?;
            let mut code = 0u32;
            let alive = unsafe { GetExitCodeProcess(h.0, &mut code) } != 0 && code == STILL_ACTIVE as u32;
            (alive && created(&h) == *made).then(|| proctree::strip_exe(name).to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(is_gui_exe(r"C:\Windows\notepad.exe"));
        assert!(!is_gui_exe(r"C:\Windows\System32\cmd.exe"));
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

    #[test]
    fn watcher_wakes_on_job_exit() {
        use std::time::Instant;
        let mut child = std::process::Command::new("powershell.exe").args(["-NoProfile", "-Command", "Start-Sleep -Milliseconds 300"]).spawn().expect("spawn");
        let t = Instant::now();
        wait_any(&[child.id()], Duration::from_secs(5));
        assert!(t.elapsed() < Duration::from_secs(2), "woke on exit, not on the gap: {:?}", t.elapsed());
        let _ = child.wait();
        let t = Instant::now();
        wait_any(&[4_000_000_000], Duration::from_millis(100));
        let e = t.elapsed();
        assert!(e >= Duration::from_millis(90) && e < Duration::from_secs(1), "a gone pid waits the gap: {e:?}");
        let t = Instant::now();
        wait_any(&[], Duration::from_millis(50));
        let e = t.elapsed();
        assert!(e >= Duration::from_millis(40) && e < Duration::from_secs(1), "{e:?}");
    }

    #[test]
    fn an_unreadable_console_is_not_a_prompt() {
        // No such process: the mode is unknown, which the watcher reads as no prompt.
        let got = crate::pty::console_mode(0xFFFF_FFF0);
        assert!(got.is_err(), "{got:?}");
        assert!(!got.is_ok_and(crate::pwm_prompt::at_prompt));
        let w = JobWatch::start(std::sync::Arc::new(|| {}));
        assert!(!w.password_input(0xFFFF_FFF0), "an unwatched shell is not at a prompt");
    }

    /// A paused local console (Undo Close, or output held for an inline file question) is
    /// never read: it is left out of each look, and one paused after the look is refused under
    /// `CONSOLE` by `console_mode_unless`.
    #[test]
    fn a_paused_console_is_not_read() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let (running, paused) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(true)));
        let consoles: HashMap<u32, Arc<AtomicBool>> = [(1, running.clone()), (2, paused)].into_iter().collect();
        let ids = |c: &HashMap<u32, Arc<AtomicBool>>| console_targets(c).into_iter().map(|t| t.0).collect::<Vec<_>>();
        assert_eq!(ids(&consoles), vec![1]);
        let targets = console_targets(&consoles);
        running.store(true, Ordering::Release);
        assert_eq!(crate::pty::console_mode_unless(targets[0].0, &targets[0].1), Err("the console is paused".to_string()));
    }

    #[test]
    fn a_real_tree_has_a_job() {
        let mut child = std::process::Command::new("cmd.exe").args(["/d", "/c", "ping -n 4 127.0.0.1 >NUL"]).spawn().unwrap();
        std::thread::sleep(Duration::from_millis(600));
        let mut cache = Cache::default();
        let procs = snapshot(&[child.id()], &mut cache).unwrap();
        let job = proctree::display_job(&procs, child.id());
        let _ = child.kill();
        let _ = child.wait();
        let job = job.expect("the shell is there");
        assert_eq!(job.name.to_lowercase(), "ping", "{procs:?}");
        assert!(job.cmdline.to_lowercase().contains("127.0.0.1"), "{}", job.cmdline);
        assert_eq!(proctree::ancestors(&procs, child.id()), vec!["ping"]);
    }

    #[test]
    fn a_tree_has_every_command_line_and_signals_reach_it() {
        let mut child = std::process::Command::new("cmd.exe").args(["/d", "/c", "ping -n 30 127.0.0.1 >NUL"]).spawn().unwrap();
        std::thread::sleep(Duration::from_millis(600));
        let root = child.id();
        let mut cache = Cache::default();
        let (_, trees) = snapshot_trees(&[root], &[root], &mut cache).unwrap();
        let tree = trees.get(&root).cloned().unwrap_or_default();
        assert_eq!(tree.first().map(|p| p.pid), Some(root), "the shell leads its tree");
        let ping = tree.iter().find(|p| p.name.eq_ignore_ascii_case("ping.exe")).cloned();
        let ping = ping.unwrap_or_else(|| panic!("{tree:?}"));
        assert!(ping.cmdline.as_deref().is_some_and(|c| c.contains("127.0.0.1")), "{ping:?}");
        assert!(tree[0].cmdline.as_deref().is_some_and(|c| c.to_lowercase().contains("cmd")), "{:?}", tree[0]);
        assert!(process_alive(ping.pid, ping.created));
        // A pid with another creation time is another process: never signalled.
        let running = || Arc::new(std::sync::atomic::AtomicBool::new(false));
        assert!(signal_process(ping.pid, Some(ping.created.wrapping_add(1)), SignalWay::Terminate(137), running()).is_err());
        assert!(signal_process(ping.pid, Some(ping.created.wrapping_add(1)), SignalWay::CtrlC, running()).is_err(), "nor sent a console event");
        // A paused console is never attached to: the event is refused.
        let paused = Arc::new(std::sync::atomic::AtomicBool::new(true));
        match signal_process(ping.pid, Some(ping.created), SignalWay::CtrlC, paused) {
            Ok(Sent::Later(h)) => assert_eq!(h.join().unwrap(), Err("the console is paused".to_string())),
            other => panic!("{:?}", other.map(|_| ())),
        }
        assert!(process_alive(ping.pid, ping.created));
        assert!(matches!(signal_process(root, Some(tree[0].created), SignalWay::Terminate(137), running()), Ok(Sent::Done)));
        let status = child.wait().unwrap();
        assert_eq!(status.code(), Some(137), "KILL ends with 128 + 9");
        assert!(!process_alive(root, tree[0].created));
        let _ = signal_process(ping.pid, Some(ping.created), SignalWay::Terminate(143), running());
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

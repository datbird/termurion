//! Process facts on Windows, from the Toolhelp reader in `jobwatch` (a Windows-only module).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use windows_sys::Win32::Foundation::STILL_ACTIVE;
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, TerminateProcess};

use super::proc_table::{created, open, open_with, os_err, rows, Handle};
use crate::jobwatch::{snapshot, Cache};
use crate::platform::proc_table::Row;
use crate::proctree::{self, Job, Proc};
use crate::toolbelt_tools::SignalWay;

pub use crate::jobwatch::JobWatch;
pub use crate::pwm_run::ProcessRunner;
pub use crate::script_proc::{ScriptProcess, KILLED_CODE};
pub(crate) use crate::script_provision::KillJob;


/// The job a shell shows: the deepest console program under it. None when the shell is gone.
pub fn foreground_job(pid: u32) -> Option<Job> {
    let procs = snapshot(&[pid], &mut Cache::default()).ok()?;
    proctree::display_job(&procs, pid)
}

/// The full path of the program `pid` runs.
pub fn process_image(pid: u32) -> Option<String> {
    open(pid).and_then(|o| o.image())
}

/// The ids of the processes started by `pid`.
pub fn children(pid: u32) -> Vec<u32> {
    rows().map(|all| all.into_iter().filter(|r| r.pid != r.ppid && r.ppid == pid).map(|r| r.pid).collect()).unwrap_or_default()
}

/// Whether the process `pid` still runs and is the one created at `created` (a reused pid is
/// another process). A cheap open, not a snapshot.
pub fn process_alive(pid: u32, created: u64) -> bool {
    let Some(h) = open_with(pid, PROCESS_QUERY_LIMITED_INFORMATION) else { return false };
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
    std::thread::spawn(move || crate::platform::pty::attached_unless(pid, &stop, || crate::platform::pty::send_ctrl_event_attached(ctrl_break, OWN_EVENT_WAIT).map(|_| ())))
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
    let all = rows()?;
    let mut kids: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, r) in all.iter().enumerate() {
        if r.pid != r.ppid {
            kids.entry(r.ppid).or_default().push(i);
        }
    }
    let mut want: Vec<usize> = kids.get(&dead).cloned().unwrap_or_default();
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
    let mut cache = Cache::default();
    let mut out = Vec::new();
    for i in want {
        let Row { pid, ppid, ref name, .. } = all[i];
        let bare = proctree::strip_exe(name);
        if bare.eq_ignore_ascii_case("conhost") || bare.eq_ignore_ascii_case("OpenConsole") {
            continue;
        }
        let h = open(pid);
        let made = h.as_ref().map_or(0, |o| o.created());
        let gui = h.as_ref().and_then(|o| o.image()).is_some_and(|p| cache.is_gui(&p));
        out.push(Proc { pid, ppid, name: name.clone(), created: made, gui, cmdline: None, background: false });
    }
    Ok(out)
}

/// The names (no ".exe") of those of `set` ((pid, creation time)) that still run: a reused pid
/// has another creation time and is not counted.
pub fn still_running(set: &[(u32, u64)]) -> Vec<String> {
    let Ok(all) = rows() else { return Vec::new() };
    set.iter()
        .filter_map(|(pid, made)| {
            let name = &all.iter().find(|r| r.pid == *pid)?.name;
            let h = open_with(*pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
            let mut code = 0u32;
            let alive = unsafe { GetExitCodeProcess(h.0, &mut code) } != 0 && code == STILL_ACTIVE as u32;
            (alive && self::created(&h) == *made).then(|| proctree::strip_exe(name).to_string())
        })
        .collect()
}


/// Starts `exe` detached from this process (its own process group, out of this process's job
/// when Windows allows), so it outlives it.
pub fn start_detached(exe: &std::path::Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let spawn = |flags: u32| {
        std::process::Command::new(exe)
            .creation_flags(flags)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
    };
    let base = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    // A job that forbids breakaway refuses the flag; the child then lives in this process's job.
    spawn(base | CREATE_BREAKAWAY_FROM_JOB).or_else(|_| spawn(base)).map(drop).map_err(|e| format!("could not start {}: {e}", exe.display()))
}

/// Waits up to 30 seconds for the process `pid` to end (the relaunch waiting for the old
/// Termurion); at once when it cannot be opened (already gone).
pub fn wait_for_pid(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
    // SAFETY: a handle opened here, waited on and closed here.
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if h.is_null() {
            return;
        }
        WaitForSingleObject(h, 30_000);
        CloseHandle(h);
    }
}

/// One line in the debug output (`OutputDebugStringW`, iTerm2's `NSLog`).
pub fn debug_line(text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    // SAFETY: a NUL-terminated wide string that outlives the call.
    unsafe { windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringW(wide.as_ptr()) };
}

/// A command that runs `line` through cmd.exe as written: the one argument builder
/// (`semantic::cmd_args`) refuses what cmd.exe would read as syntax. The caller sets the rest and starts it.
pub fn command_for_line(line: &str) -> Result<std::process::Command, crate::platform::Unsupported> {
    use std::os::windows::process::CommandExt;
    let mut c = std::process::Command::new("cmd.exe");
    c.raw_arg(crate::semantic::cmd_args(line));
    Ok(c)
}

/// The child gets no console window of its own (`CREATE_NO_WINDOW`); its own windows still show.
pub fn hide_console(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobwatch::snapshot_trees;

    #[test]
    fn own_process_has_an_image_and_a_child_list() {
        let me = std::process::id();
        let image = process_image(me).expect("own image path");
        assert!(image.to_ascii_lowercase().ends_with(".exe"), "{image}");
        let mut child = std::process::Command::new("cmd.exe").args(["/c", "ping -n 3 127.0.0.1 >nul"]).stdout(std::process::Stdio::null()).spawn().unwrap();
        assert!(children(me).contains(&child.id()));
        let _ = child.kill();
        let _ = child.wait();
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
}

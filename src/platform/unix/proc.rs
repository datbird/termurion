//! Process facts on Unix, from the kernel's process table (`proc_table`): the foreground job, the
//! tree under a shell, and signals.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use super::proc_table::{open, rows};
use crate::jobwatch::{snapshot, Cache};
use crate::platform::Unsupported;
use crate::proctree::{self, Job, Proc};
use crate::toolbelt_tools::SignalWay;

pub use crate::jobwatch::JobWatch;

/// The job a shell shows: the deepest foreground program under it. None when the shell is gone.
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

/// What a shell left running after it exited: every process still in its session, found by the
/// session id (a shell is its session's leader, so nothing it left reparents out of it).
pub fn orphans_of(dead: u32) -> Result<Vec<Proc>, String> {
    let sid = libc::pid_t::try_from(dead).map_err(|_| format!("process {dead} is not a valid id"))?;
    let mut out = Vec::new();
    for r in rows()? {
        if r.pid == dead {
            continue;
        }
        // SAFETY: getsid only reads the process table.
        if unsafe { libc::getsid(r.pid as libc::pid_t) } != sid {
            continue;
        }
        let Some(o) = open(r.pid) else { continue };
        out.push(Proc { pid: r.pid, ppid: r.ppid, name: r.name, created: o.created(), gui: false, cmdline: None, background: r.background });
    }
    Ok(out)
}

/// The names of those of `set` ((pid, start time)) that still run: a reused pid has another
/// start time and is not counted.
pub fn still_running(set: &[(u32, u64)]) -> Vec<String> {
    let Ok(all) = rows() else { return Vec::new() };
    set.iter()
        .filter_map(|(pid, made)| {
            let name = &all.iter().find(|r| r.pid == *pid)?.name;
            (open(*pid)?.created() == *made).then(|| name.clone())
        })
        .collect()
}

/// A pid as the kernel's signed one; 0 and the out-of-range are refused (kill(0) would signal our
/// own group).
fn kernel_pid(pid: u32) -> Result<libc::pid_t, String> {
    libc::pid_t::try_from(pid).ok().filter(|p| *p > 0).ok_or_else(|| format!("process {pid} is not a valid id"))
}

fn os_err(what: &str) -> String {
    format!("{what} failed: {}", std::io::Error::last_os_error())
}

/// Ctrl+C (SIGINT) or Ctrl+Break (SIGQUIT) to the foreground group of the pty `pid` runs on,
/// from its own thread. Only for a pty this process holds (a local pane): a hosted pane's goes
/// through termurion-host. Not at all once `stop` is set (a pane paused for Undo Close).
pub fn console_event(pid: u32, ctrl_break: bool, stop: Arc<AtomicBool>) -> JoinHandle<Result<(), String>> {
    std::thread::spawn(move || {
        if stop.load(Ordering::Acquire) {
            return Err("the console is paused".to_string());
        }
        let group = super::pty::foreground_group(pid).ok_or_else(|| format!("no pty for process {pid}"))?;
        let sig = if ctrl_break { libc::SIGQUIT } else { libc::SIGINT };
        // SAFETY: killpg with a group the pty reported.
        if unsafe { libc::killpg(group, sig) } < 0 { Err(os_err("killpg")) } else { Ok(()) }
    })
}

/// The Jobs tool's signal, sent or on its way (Windows' shape).
pub enum Sent {
    Done,
    /// A console event sent from its own thread; its result.
    Later(JoinHandle<Result<(), String>>),
}

/// Every process under a shell now (the close prompt's list).
pub fn descendants_now(shell: u32) -> Result<Vec<Proc>, String> {
    let procs = snapshot(&[shell], &mut Cache::default())?;
    Ok(proctree::descendants(&procs, shell).into_iter().cloned().collect())
}

/// Whether the process `pid` still runs and is the one started at `created` (a reused pid is
/// another process).
pub fn process_alive(pid: u32, created: u64) -> bool {
    open(pid).is_some_and(|o| o.created() == created)
}

fn ended(pid: u32) -> String {
    format!("process {pid} has already ended")
}

/// The start-time check, then a plain kill (macOS has no handle to a process; the gap is one syscall).
fn check_then_kill(pid: u32, target: libc::pid_t, created: Option<u64>, sig: libc::c_int) -> Result<(), String> {
    let o = open(pid).ok_or_else(|| ended(pid))?;
    if created.is_some_and(|c| o.created() != c) {
        return Err(ended(pid));
    }
    // SAFETY: a plain kill of a checked pid.
    if unsafe { libc::kill(target, sig) } < 0 { Err(os_err("kill")) } else { Ok(()) }
}

/// Linux: a pidfd pins the process, so a pid reused after the check can never get the signal.
/// Kernels before 5.3 have no pidfd, and some container seccomp profiles answer EPERM (pidfd_open
/// itself never checks permission); both take the plain path.
#[cfg(target_os = "linux")]
fn send_checked(pid: u32, target: libc::pid_t, created: Option<u64>, sig: libc::c_int) -> Result<(), String> {
    // SAFETY: pidfd_open takes a pid and flags and returns a descriptor we own.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, target, 0) };
    if fd < 0 {
        return match std::io::Error::last_os_error().raw_os_error() {
            Some(libc::ENOSYS | libc::EPERM) => check_then_kill(pid, target, created, sig),
            Some(libc::ESRCH) => Err(ended(pid)),
            _ => Err(os_err("pidfd_open")),
        };
    }
    let fd = fd as libc::c_int;
    let checked = open(pid).ok_or_else(|| ended(pid)).and_then(|o| if created.is_some_and(|c| o.created() != c) { Err(ended(pid)) } else { Ok(()) });
    let sent = checked.and_then(|()| {
        // SAFETY: the fd is ours; no siginfo, no flags.
        if unsafe { libc::syscall(libc::SYS_pidfd_send_signal, fd, sig, std::ptr::null::<libc::c_void>(), 0) } < 0 { Err(os_err("pidfd_send_signal")) } else { Ok(()) }
    });
    // SAFETY: closing the descriptor opened above.
    unsafe { libc::close(fd) };
    sent
}

#[cfg(not(target_os = "linux"))]
fn send_checked(pid: u32, target: libc::pid_t, created: Option<u64>, sig: libc::c_int) -> Result<(), String> {
    check_then_kill(pid, target, created, sig)
}

/// Sends one of the Jobs tool's signals to `pid`, only to the process started at `created` when
/// given (a pid the tree showed may have been reused since). `CtrlC` is SIGINT and `CtrlBreak`
/// SIGQUIT to the process itself; `Terminate(code)` is signal `code - 128`. `stop` matters only to
/// `console_event`.
pub fn signal_process(pid: u32, created: Option<u64>, way: SignalWay, _stop: Arc<AtomicBool>) -> Result<Sent, String> {
    let target = kernel_pid(pid)?;
    let sig = match way {
        SignalWay::CtrlC => libc::SIGINT,
        SignalWay::CtrlBreak => libc::SIGQUIT,
        SignalWay::Terminate(code) => code.checked_sub(128).filter(|n| (1..=64).contains(n)).map(|n| n as libc::c_int).ok_or_else(|| format!("{code} is not a signal exit code"))?,
    };
    send_checked(pid, target, created, sig)?;
    Ok(Sent::Done)
}

/// Starts a program detached from this process: no stdio, a session of its own (so a closed
/// terminal or a killed Termurion does not take it along). Only the session host needs it. A thread
/// waits on the child so it never lingers as a zombie while Termurion runs.
pub fn start_detached(exe: &std::path::Path) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(exe);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // It lives on after Termurion: never keep the folder Termurion started in (a removable mount) busy.
    cmd.current_dir("/");
    // SAFETY: setsid is async-signal-safe and touches nothing of ours.
    unsafe {
        cmd.pre_exec(|| if libc::setsid() < 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) });
    }
    // Under the spawn gate, so a descriptor another thread has just made is never inherited.
    let child = {
        let _gate = super::pty::spawn_gate();
        cmd.spawn()
    };
    let mut child = child.map_err(|e| format!("{}: {e}", exe.display()))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Waits up to 30 seconds for the process `pid` to end; at once when it is not there.
pub fn wait_for_pid(pid: u32) {
    let Ok(pid) = libc::pid_t::try_from(pid) else { return };
    let end = std::time::Instant::now() + std::time::Duration::from_secs(30);
    // Signal 0 only asks whether the process exists (ESRCH: gone; EPERM: there, not ours).
    while unsafe { libc::kill(pid, 0) } == 0 && std::time::Instant::now() < end {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// The debug output of a Unix program is its stderr.
pub fn debug_line(text: &str) {
    // Some callers end the line themselves.
    eprintln!("{}", text.trim_end_matches('\n'));
}

/// The exit code a stopped script reports on Windows (`STATUS_CONTROL_C_EXIT`).
pub const KILLED_CODE: u32 = 0xC000_013A;

/// A launched script's process. None starts on Unix yet, so none exists to call these on.
pub struct ScriptProcess {
    pub decoder: crate::script_launch::Decoder,
    pub cookie: String,
    never: std::convert::Infallible,
}

impl ScriptProcess {
    pub fn start(_program: &std::path::Path, _args: &[String], _env: &[(String, String)], _cwd: Option<&std::path::Path>, _notify: crate::platform::pty::Notify) -> Result<ScriptProcess, String> {
        Err(Unsupported { feature: "Python API scripts" }.into())
    }

    pub fn pid(&self) -> u32 {
        match self.never {}
    }

    pub fn take_output(&mut self, _cap: usize) -> Vec<u8> {
        match self.never {}
    }

    pub fn exit_code(&mut self) -> Option<u32> {
        match self.never {}
    }

    pub fn terminate(&mut self) {
        match self.never {}
    }

    pub fn active_processes(&self) -> u32 {
        match self.never {}
    }

    pub(crate) fn into_job(self) -> KillJob {
        match self.never {}
    }
}

/// The job a script's processes run in (Windows' Job Object). None exists on Unix.
pub(crate) struct KillJob {
    never: std::convert::Infallible,
}

impl KillJob {
    pub(crate) fn active_processes(&self) -> u32 {
        match self.never {}
    }
}

/// Runs a password manager CLI (`op`, `lpass`): no child process is started on Unix yet, so every
/// request reports why.
#[derive(Default)]
pub struct ProcessRunner;

impl ProcessRunner {
    pub fn new() -> ProcessRunner {
        ProcessRunner
    }
}

impl crate::pwm_run::Runner for ProcessRunner {
    fn run(&mut self, _r: crate::pwm_run::Request) -> Result<crate::pwm_run::Output, String> {
        Err(Unsupported { feature: "Password manager command line tools" }.into())
    }
}

/// A command line is cmd.exe's; no shell is chosen for one on Unix yet.
pub fn command_for_line(_line: &str) -> Result<std::process::Command, Unsupported> {
    Err(Unsupported { feature: "Running a command line" })
}

/// A child never opens a console window on Unix.
pub fn hide_console(_cmd: &mut std::process::Command) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::pty::{spawn, PtyApi};
    use std::time::{Duration, Instant};

    const SH_I: &str = "env PS1=> ENV= /bin/sh -i";

    fn running() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    /// Polls `f` for up to 3 seconds.
    fn within_3s(mut f: impl FnMut() -> bool) -> bool {
        let end = Instant::now() + Duration::from_secs(3);
        while Instant::now() < end {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        f()
    }

    /// The job's title is argv0, so a shell reads "sh" even where the kernel's name is "bash".
    fn job_is(pid: u32, name: &str) -> bool {
        foreground_job(pid).is_some_and(|j| j.title == name)
    }

    #[test]
    fn the_foreground_job_follows_the_pty() {
        let p = spawn(&PtyApi, SH_I, 80, 24).unwrap();
        p.read_until(b">", Duration::from_secs(5)).unwrap_or_else(|got| panic!("{:?}", String::from_utf8_lossy(&got)));
        let pid = p.pid();
        assert!(job_is(pid, "sh"), "an idle shell is its own job");
        p.write(b"sleep 5\n").unwrap();
        assert!(within_3s(|| job_is(pid, "sleep")), "{:?}", foreground_job(pid));
        let job = foreground_job(pid).unwrap();
        assert_eq!(job.cmdline, "sleep 5");
        console_event(pid, false, running()).join().unwrap().unwrap();
        assert!(within_3s(|| job_is(pid, "sh")), "Ctrl+C ended the sleep: {:?}", foreground_job(pid));
        // A background job is in the process list but never the job.
        p.write(b"sleep 30 &\n").unwrap();
        let seen = || rows().unwrap().into_iter().any(|r| r.name == "sleep" && r.ppid == pid && r.background);
        assert!(within_3s(seen), "the background sleep is listed as background");
        assert!(job_is(pid, "sh"), "{:?}", foreground_job(pid));
        assert_eq!(descendants_now(pid).unwrap().len(), 1, "the close prompt still sees it");
        for c in descendants_now(pid).unwrap() {
            let _ = signal_process(c.pid, Some(c.created), SignalWay::Terminate(137), running());
        }
    }

    #[test]
    fn a_console_event_needs_a_pty_we_hold() {
        let e = console_event(1, false, running()).join().unwrap().unwrap_err();
        assert!(e.contains("no pty"), "{e}");
        let p = spawn(&PtyApi, SH_I, 80, 24).unwrap();
        p.read_until(b">", Duration::from_secs(5)).unwrap();
        let paused = Arc::new(AtomicBool::new(true));
        assert_eq!(console_event(p.pid(), false, paused).join().unwrap(), Err("the console is paused".to_string()));
    }

    #[test]
    fn signal_process_checks_the_creation_time() {
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        let made = open(pid).unwrap().created();
        assert!(process_alive(pid, made));
        assert!(signal_process(pid, Some(made.wrapping_add(1)), SignalWay::Terminate(137), running()).is_err());
        assert!(child.try_wait().unwrap().is_none(), "the child lives");
        assert!(signal_process(0, None, SignalWay::CtrlC, running()).is_err(), "pid 0 would signal our own group");
        assert!(signal_process(pid, None, SignalWay::Terminate(5), running()).is_err(), "not a signal exit code");
        assert!(matches!(signal_process(pid, Some(made), SignalWay::Terminate(137), running()), Ok(Sent::Done)));
        let status = child.wait().unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(9), "KILL is 128 + 9");
        assert!(!process_alive(pid, made));
        assert!(signal_process(pid, Some(made), SignalWay::CtrlC, running()).is_err(), "an ended process");
    }

    #[test]
    fn ctrl_c_and_break_are_int_and_quit() {
        use std::os::unix::process::ExitStatusExt;
        for (way, sig) in [(SignalWay::CtrlC, libc::SIGINT), (SignalWay::CtrlBreak, libc::SIGQUIT)] {
            let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
            assert!(matches!(signal_process(child.id(), None, way, running()), Ok(Sent::Done)));
            assert_eq!(child.wait().unwrap().signal(), Some(sig));
        }
    }

    #[test]
    fn our_own_tree_is_listed() {
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let me = std::process::id();
        assert!(children(me).contains(&child.id()));
        assert!(process_image(child.id()).is_some_and(|i| i.ends_with("sleep")));
        assert_eq!(crate::platform::proc::process_name(child.id()).as_deref(), Some("sleep"));
        let made = open(child.id()).unwrap().created();
        assert_eq!(still_running(&[(child.id(), made), (child.id(), made + 1)]), vec!["sleep".to_string()]);
        let _ = child.kill();
        let _ = child.wait();
        assert!(still_running(&[(child.id(), made)]).is_empty());
        assert!(orphans_of(4_000_000).unwrap().is_empty());
    }

    #[test]
    fn a_job_watch_reports_a_change() {
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = hits.clone();
        let w = JobWatch::start(Arc::new(move || {
            counter.fetch_add(1, Ordering::Relaxed);
        }));
        let p = spawn(&PtyApi, SH_I, 80, 24).unwrap();
        p.read_until(b">", Duration::from_secs(5)).unwrap();
        let pid = p.pid();
        w.watch(pid);
        let named = |n: &str| w.job(pid).job.is_some_and(|j| j.title == n);
        assert!(within_3s(|| named("sh")), "{:?}", w.job(pid));
        let before = hits.load(Ordering::Relaxed);
        p.write(b"sleep 5\n").unwrap();
        assert!(within_3s(|| named("sleep")), "{:?}", w.job(pid));
        assert!(hits.load(Ordering::Relaxed) > before, "the window was told");
        assert_eq!(w.job(pid).ancestors, vec!["sleep"]);
        assert_eq!(w.error(), None);
        w.forget(pid);
        console_event(pid, false, running()).join().unwrap().unwrap();
    }
}

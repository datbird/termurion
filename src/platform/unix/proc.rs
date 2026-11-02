//! No process tree reader on Unix yet. Job info is cosmetic, so these answer None or empty and
//! say so once in the debug log.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread::JoinHandle;

use crate::platform::Unsupported;
use crate::proctree::{Job, Proc};

fn note() {
    static LOGGED: AtomicBool = AtomicBool::new(false);
    if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        crate::dlog!("Process information is not available on this platform yet");
    }
}

pub fn foreground_job(_pid: u32) -> Option<Job> {
    note();
    None
}

pub fn process_image(_pid: u32) -> Option<String> {
    note();
    None
}

pub fn children(_pid: u32) -> Vec<u32> {
    note();
    Vec::new()
}

/// What a shell left running after it exited. Reports Unsupported; the one caller ignores the
/// error, because job info is cosmetic.
pub fn orphans_of(_dead: u32) -> Result<Vec<Proc>, String> {
    note();
    Err(Unsupported { feature: "Process information" }.into())
}

/// The names of the listed processes that still run: none can be told, so none.
pub fn still_running(_set: &[(u32, u64)]) -> Vec<String> {
    note();
    Vec::new()
}

/// Sends Ctrl+C or Ctrl+Break to a console. Unix has no consoles to send it to.
pub fn console_event(_pid: u32, _ctrl_break: bool, _stop: Arc<AtomicBool>) -> JoinHandle<Result<(), String>> {
    std::thread::spawn(|| Err(Unsupported { feature: "Console signals" }.into()))
}

/// Starts a program detached from this process: no stdio, a session of its own (so a closed
/// terminal or a killed Termurion does not take it along). Only the session host needs it. A thread
/// waits on the child so it never lingers as a zombie while Termurion runs.
pub fn start_detached(exe: &std::path::Path) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(exe);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
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

/// The Jobs tool's signal, sent or on its way (Windows' shape).
pub enum Sent {
    Done,
    /// A console event sent from its own thread; its result.
    Later(JoinHandle<Result<(), String>>),
}

/// Every process under a shell now (the close prompt's list): none can be listed yet.
pub fn descendants_now(_shell: u32) -> Result<Vec<Proc>, String> {
    note();
    Err(Unsupported { feature: "Process information" }.into())
}

/// Whether the process created at `created` still runs: none can be told, so no.
pub fn process_alive(_pid: u32, _created: u64) -> bool {
    note();
    false
}

/// Sends one of the Jobs tool's signals: Unix has no sender yet.
pub fn signal_process(_pid: u32, _created: Option<u64>, _way: crate::toolbelt_tools::SignalWay, _stop: Arc<AtomicBool>) -> Result<Sent, String> {
    Err(Unsupported { feature: "Sending signals to jobs" }.into())
}

/// The job watcher: it needs the process list, which Unix does not read yet, so it watches
/// nothing. Every call answers none or empty, and `error` says why for the Jobs tool.
#[derive(Clone)]
pub struct JobWatch;

impl JobWatch {
    pub fn start(_notify: Arc<dyn Fn() + Send + Sync>) -> JobWatch {
        note();
        JobWatch
    }

    pub fn watch(&self, _shell: u32) {}

    pub fn forget(&self, _shell: u32) {}

    pub fn set_tree_wanted(&self, _shell: u32, _on: bool) {}

    pub fn tree(&self, _shell: u32) -> Vec<Proc> {
        Vec::new()
    }

    pub fn tree_gen(&self) -> u64 {
        0
    }

    pub fn watch_console(&self, _shell: u32, _paused: Arc<AtomicBool>) {}

    pub fn set_read_modes(&self, _debounce: Option<f64>) {}

    pub fn password_input(&self, _shell: u32) -> bool {
        false
    }

    pub fn set_ancestry_diagnostics(&self, _on: bool) {}

    pub fn set_fast_updates(&self, _on: bool) {}

    pub fn set_fast(&self, _fast: bool) {}

    pub fn job(&self, _shell: u32) -> crate::proctree::ShellJob {
        crate::proctree::ShellJob::default()
    }

    /// Why the watcher shows nothing.
    pub fn error(&self) -> Option<String> {
        Some(Unsupported { feature: "Process information" }.to_string())
    }
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

    #[test]
    fn unix_proc_answers_none_and_empty() {
        assert!(foreground_job(1).is_none());
        assert!(process_image(1).is_none());
        assert!(children(1).is_empty());
        assert!(still_running(&[(1, 1)]).is_empty());
        assert!(start_detached(std::path::Path::new("x")).unwrap_err().contains("Session host is not available"));
        assert!(orphans_of(1).unwrap_err().contains("Process information is not available"));
    }
}

//! Windows provisioning of Python environments: the program runner (every program hidden, in its
//! own Job Object that ends everything it started), the Job Object itself, the System32 folder, and
//! the worker that runs jobs one at a time.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::script_provision::{run_job, system_tool_in, wait_limited, Job, Outcome, Runner, Step, BUILD_TIMEOUT, CANCELED, STOPPED, STOP_WAIT};

/// Runs provisioning for real: every program hidden (`CREATE_NO_WINDOW`), stdin empty, stdout
/// and stderr on one pipe, Termurion's environment cleaned by `child_env` plus the step's
/// variables. Each run is in its own Job Object that ends every process it started: when the
/// program ends (so nothing it left behind holds the output pipe), when it runs past
/// `timeout`, and when Termurion quits (the job closes with it). Each output line goes to `lines`.
pub struct RealRunner {
    pub lines: std::sync::mpsc::Sender<String>,
    pub timeout: Duration,
    /// Set from outside to stop the running program (the installer window's Cancel).
    pub cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

/// A Job Object whose processes all end when it is closed (provisioning runs and launched
/// scripts).
pub(crate) struct KillJob(windows_sys::Win32::Foundation::HANDLE);

impl KillJob {
    pub(crate) fn new() -> Result<KillJob, String> {
        use windows_sys::Win32::System::JobObjects::{CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE};
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(format!("CreateJobObjectW failed: {}", std::io::Error::last_os_error()));
            }
            let job = KillJob(job);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let size = std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
            if SetInformationJobObject(job.0, JobObjectExtendedLimitInformation, (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(), size) == 0 {
                return Err(format!("SetInformationJobObject failed: {}", std::io::Error::last_os_error()));
            }
            Ok(job)
        }
    }

    /// Puts a process started suspended in the job, then lets it run, so nothing it starts can
    /// get out of the job first. A process none of whose threads could be resumed would sit
    /// suspended for ever: that is a failed start.
    pub(crate) fn adopt(&self, child: &std::process::Child) -> Result<(), String> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
        if unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle()) } == 0 {
            return Err(format!("AssignProcessToJobObject failed: {}", std::io::Error::last_os_error()));
        }
        if resume_threads(child.id()) == 0 {
            return Err(format!("Could not resume the new process: {}", std::io::Error::last_os_error()));
        }
        Ok(())
    }

    /// Ends every process in the job with exit code 1.
    pub(crate) fn terminate(&self) {
        self.terminate_with(1);
    }

    /// Ends every process in the job with exit code `code`.
    pub(crate) fn terminate_with(&self, code: u32) {
        unsafe { windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0, code) };
    }

    /// How many processes in the job are still running; `u32::MAX` when that cannot be read,
    /// so a job is never let go (ending what it holds) on a failed query.
    pub(crate) fn active_processes(&self) -> u32 {
        use windows_sys::Win32::System::JobObjects::{JobObjectBasicAccountingInformation, QueryInformationJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION};
        unsafe {
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = std::mem::zeroed();
            let size = std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32;
            if QueryInformationJobObject(self.0, JobObjectBasicAccountingInformation, (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(), size, std::ptr::null_mut()) == 0 {
                return u32::MAX;
            }
            info.ActiveProcesses
        }
    }
}

impl Drop for KillJob {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

/// Resumes the threads of a process started suspended (it has only its first one); how many
/// were resumed.
fn resume_threads(pid: u32) -> u32 {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32};
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snap == INVALID_HANDLE_VALUE {
            return 0;
        }
        let mut resumed = 0;
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut more = Thread32First(snap, &mut entry) != 0;
        while more {
            if entry.th32OwnerProcessID == pid {
                let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                if !thread.is_null() {
                    if ResumeThread(thread) != u32::MAX {
                        resumed += 1;
                    }
                    CloseHandle(thread);
                }
            }
            more = Thread32Next(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
        resumed
    }
}

/// "an hour" for `BUILD_TIMEOUT`, else the seconds.
fn duration_text(d: Duration) -> String {
    if d == BUILD_TIMEOUT { "an hour".to_string() } else { format!("{} seconds", d.as_secs()) }
}

/// The System32 folder from `GetSystemDirectoryW` (not Termurion's environment), None when it
/// cannot be read.
pub fn system_directory() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    let mut buf = [0u16; 520];
    let n = unsafe { windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    (n > 0 && n < buf.len()).then(|| PathBuf::from(std::ffi::OsString::from_wide(&buf[..n])))
}

impl RealRunner {
    /// A runner with `BUILD_TIMEOUT`.
    pub fn new(lines: std::sync::mpsc::Sender<String>) -> RealRunner {
        RealRunner { lines, timeout: BUILD_TIMEOUT, cancel: None }
    }

    /// A Windows tool from System32 (curl.exe, tar.exe), found with `GetSystemDirectoryW`
    /// (not Termurion's environment), so a program of the same name on PATH is never run
    /// instead; an error naming it when System32 lacks it (`system_tool_in`).
    pub(crate) fn system_tool(name: &str) -> Result<PathBuf, String> {
        system_tool_in(system_directory().as_deref(), name, &|p: &Path| p.is_file())
    }

    /// How a failure names the run: the program's name and the arguments up to the first that
    /// is not an option ("uv venv", "python -m venv").
    fn label(step: &Step) -> String {
        let mut words = vec![step.program.file_stem().map_or_else(|| step.program.display().to_string(), |s| s.to_string_lossy().into_owned())];
        for a in &step.args {
            words.push(a.clone());
            if !a.starts_with('-') {
                break;
            }
        }
        words.join(" ")
    }
}

impl RealRunner {
    /// `run`, polling `watch` while the program runs: when it says true the job is killed and
    /// the run fails with `STOPPED`.
    fn run_watched(&mut self, step: &Step, watch: Option<&dyn Fn() -> bool>) -> Result<String, String> {
        use std::io::Read;
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_SUSPENDED: u32 = 0x0000_0004;
        let label = Self::label(step);
        let job = KillJob::new().map_err(|e| format!("Could not run {label}: {e}"))?;
        let (mut reader, writer) = std::io::pipe().map_err(|e| format!("Could not run {label}: {e}"))?;
        let err_writer = writer.try_clone().map_err(|e| format!("Could not run {label}: {e}"))?;
        let inherited = std::env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)));
        let mut cmd = Command::new(&step.program);
        cmd.args(&step.args)
            .env_clear()
            .envs(crate::script_runtime::child_env(inherited, &step.env))
            .stdin(Stdio::null())
            .stdout(writer)
            .stderr(err_writer)
            .creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
        let spawned = cmd.spawn();
        // The pipe's write ends go with the command, so the read ends when the program does.
        drop(cmd);
        let mut child = spawned.map_err(|e| format!("Could not run {label}: {e}"))?;
        if let Err(e) = job.adopt(&child) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("Could not run {label}: {e}"));
        }
        let lines = self.lines.clone();
        let (read_tx, read_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut all, mut line, mut buf) = (Vec::new(), Vec::new(), [0u8; 4096]);
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                all.extend_from_slice(&buf[..n]);
                for &b in &buf[..n] {
                    if b == b'\n' {
                        let text = String::from_utf8_lossy(&line);
                        let _ = lines.send(text.trim_end_matches('\r').to_string());
                        line.clear();
                    } else {
                        line.push(b);
                    }
                }
            }
            if !line.is_empty() {
                let _ = lines.send(String::from_utf8_lossy(&line).trim_end_matches('\r').to_string());
            }
            let _ = read_tx.send(String::from_utf8_lossy(&all).into_owned());
        });
        let deadline = std::time::Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) if std::time::Instant::now() >= deadline => {
                    job.terminate();
                    let _ = child.kill();
                    let within = duration_text(self.timeout);
                    // Waited for a while only: a process that will not end must not hold the
                    // worker (and the Scripts folder's scans) for ever.
                    break match wait_limited(|| child.try_wait().ok().flatten(), STOP_WAIT) {
                        Some(_) => Err(format!("{label} did not finish within {within} and was stopped.")),
                        None => Err(format!("{label} did not finish within {within} and could not be stopped.")),
                    };
                }
                Ok(None) if watch.is_some_and(|w| w()) => {
                    job.terminate();
                    let _ = child.kill();
                    let _ = wait_limited(|| child.try_wait().ok().flatten(), STOP_WAIT);
                    break Err(STOPPED.to_string());
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => {
                    job.terminate();
                    break Err(format!("Could not wait for {label}: {e}"));
                }
            }
        };
        // The program ended: whatever it left running would hold the pipe, so it ends too;
        // the reader then sees the end of the output. It is not waited on for ever.
        job.terminate();
        let output = read_rx.recv_timeout(Duration::from_secs(5)).unwrap_or_default();
        let status = status?;
        if status.success() {
            return Ok(output);
        }
        // uv's own output holds the reason (a missing wheel, an unknown package), so the
        // error carries its end (`UV:559-571`).
        let code = status.code().map_or_else(|| "unknown".to_string(), |c| (c as u32).to_string());
        let tail: String = {
            let chars: Vec<char> = output.chars().collect();
            chars[chars.len().saturating_sub(2000)..].iter().collect()
        };
        let detail = if tail.is_empty() { String::new() } else { format!("\n{tail}") };
        Err(format!("{label} failed with status {code}.{detail}"))
    }
}

impl Runner for RealRunner {
    fn run(&mut self, step: &Step) -> Result<String, String> {
        match self.cancel.clone() {
            Some(flag) => self.run_watched(step, Some(&move || flag.load(std::sync::atomic::Ordering::SeqCst))),
            None => self.run_watched(step, None),
        }
    }

    fn download(&mut self, url: &str, to: &Path) -> Result<(), String> {
        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
        }
        let args = ["--fail", "--location", "--silent", "--show-error", "--proto", "=https", "--output"].iter().map(|a| a.to_string()).chain([to.to_string_lossy().into_owned(), url.to_string()]).collect();
        self.run(&Step { program: Self::system_tool("curl.exe")?, args, env: Vec::new() }).map(|_| ())
    }

    fn sha256(&mut self, file: &Path) -> Result<String, String> {
        use sha2::{Digest, Sha256};
        use std::io::Read;
        let mut f = std::fs::File::open(file).map_err(|e| format!("Could not read {}: {e}", file.display()))?;
        let (mut hasher, mut buf) = (Sha256::new(), vec![0u8; 64 * 1024]);
        loop {
            let n = f.read(&mut buf).map_err(|e| format!("Could not read {}: {e}", file.display()))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
    }

    fn extract(&mut self, zip: &Path, to: &Path) -> Result<(), String> {
        std::fs::create_dir_all(to).map_err(|e| format!("Could not create {}: {e}", to.display()))?;
        // bsdtar refuses absolute paths and ".." in an archive unless -P is passed; it is not.
        let args = vec!["-xf".to_string(), zip.to_string_lossy().into_owned(), "-C".to_string(), to.to_string_lossy().into_owned()];
        self.run(&Step { program: Self::system_tool("tar.exe")?, args, env: Vec::new() }).map(|_| ())
    }

    fn exists(&self, p: &Path) -> bool {
        p.exists()
    }

    fn find_file(&self, dir: &Path, name: &str) -> Option<PathBuf> {
        let p = dir.join(name);
        p.is_file().then_some(p)
    }

    fn list_dirs(&self, dir: &Path) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default();
        out.sort();
        out
    }

    fn rename(&mut self, from: &Path, to: &Path) -> Result<(), String> {
        if let Some(dir) = to.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::rename(from, to).map_err(|e| format!("Could not move {} to {}: {e}", from.display(), to.display()))
    }

    fn remove(&mut self, p: &Path) {
        let _ = if p.is_dir() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) };
    }

    fn write(&mut self, p: &Path, text: &str) -> Result<(), String> {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(p, text).map_err(|e| format!("Could not write {}: {e}", p.display()))
    }

    fn log(&mut self, line: &str) {
        let _ = self.lines.send(line.to_string());
    }
}

/// Runs jobs on a worker thread, one at a time in the order submitted (iTerm2's serial
/// `provisionQueue`). The app is woken when a job ends or a line of output arrives.
pub struct Provisioner {
    jobs: std::sync::mpsc::Sender<(u64, Job)>,
    done: std::sync::mpsc::Receiver<(u64, Result<Outcome, String>)>,
    lines: std::sync::mpsc::Receiver<String>,
    next: std::cell::Cell<u64>,
    /// Jobs submitted and not yet ended.
    pending: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    /// Set by `cancel`: the running program stops and the jobs behind it end as canceled, until
    /// the queue is empty.
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Provisioner {
    pub fn start(uv_dir: PathBuf, notify: crate::platform::pty::Notify) -> Provisioner {
        use std::sync::atomic::Ordering::SeqCst;
        use std::sync::mpsc::channel;
        let (jobs, job_rx) = channel::<(u64, Job)>();
        let (done_tx, done) = channel();
        let (line_tx, line_rx) = channel::<String>();
        let (app_lines, lines) = channel();
        let pending = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        // Lines pass through here so each one wakes the app.
        let wake = notify.clone();
        std::thread::spawn(move || {
            for line in line_rx {
                if app_lines.send(line).is_err() {
                    break;
                }
                wake();
            }
        });
        let left = pending.clone();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop = cancel.clone();
        std::thread::spawn(move || {
            let mut runner = RealRunner::new(line_tx);
            runner.cancel = Some(stop.clone());
            for (id, job) in job_rx {
                let result = if stop.load(SeqCst) { Err(CANCELED.to_string()) } else { run_job(&job, &uv_dir, &mut runner) };
                // A run the cancel stopped reads as canceled, whatever text the stop left.
                let result = match result {
                    Err(_) if stop.load(SeqCst) => Err(CANCELED.to_string()),
                    other => other,
                };
                if left.fetch_sub(1, SeqCst) == 1 {
                    stop.store(false, SeqCst);
                }
                if done_tx.send((id, result)).is_err() {
                    break;
                }
                notify();
            }
        });
        Provisioner { jobs, done, lines, next: std::cell::Cell::new(1), pending, cancel }
    }

    /// Queues a job; its result comes back from `try_done` with this id.
    pub fn submit(&self, job: Job) -> u64 {
        let id = self.next.get();
        self.next.set(id + 1);
        self.pending.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.jobs.send((id, job)).is_err() {
            self.pending.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        }
        id
    }

    pub fn try_done(&self) -> Option<(u64, Result<Outcome, String>)> {
        self.done.try_recv().ok()
    }

    pub fn try_line(&self) -> Option<String> {
        self.lines.try_recv().ok()
    }

    /// Stops the running program and ends every waiting job as `CANCELED`.
    pub fn cancel(&self) {
        self.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Whether a job is running or waiting.
    pub fn busy(&self) -> bool {
        self.pending.load(std::sync::atomic::Ordering::SeqCst) > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreadable_job_counts_as_active() {
        // A job whose count cannot be read is kept (dropping it would end what still runs):
        // here the handle is an event's, not a job's.
        let event = unsafe { windows_sys::Win32::System::Threading::CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
        assert!(!event.is_null());
        let job = KillJob(event);
        assert_eq!(job.active_processes(), u32::MAX);
    }
}

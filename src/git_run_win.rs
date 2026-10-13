//! Runs git for the status bar (milestone 9i, Task 5): hidden (`CREATE_NO_WINDOW`), stdin
//! and stderr on `NUL`, `GIT_ENV` set and `GIT_ENV_REMOVED` gone from the environment, in
//! its own Job Object with kill-on-close, and killed with every program it started at the
//! deadline. `WinGitRun` is `git_state::GitRun` for the worker threads in
//! `src/app/git_ui.rs`.

use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation, WAIT_OBJECT_0};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadFile};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::IO::CancelSynchronousIo;
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess,
    InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES,
    STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
};

use crate::git_state::{GIT_ENV, GIT_ENV_REMOVED, GitRun, RunResult, find_git};
use crate::prompted::local_path_ok;

/// How often the runner looks at git between waits.
const POLL: Duration = Duration::from_millis(10);
const READ_SIZE: usize = 64 * 1024;
/// The least time the output gets to arrive after git exited.
const DRAIN: Duration = Duration::from_millis(100);

/// The Job Object runner.
pub struct WinGitRun;

impl GitRun for WinGitRun {
    fn run(&self, git: &Path, dir: &Path, args: &[String], timeout: Duration, cap: Option<usize>) -> RunResult {
        run_with(git, dir, args, timeout, cap, &mut |_| {})
    }
}

/// The program to start: a bare `git` only through `find_git` (Windows' own search would
/// look in every PATH folder, network ones too); anything else must be a local path.
fn program(git: &Path) -> Option<PathBuf> {
    if git == Path::new("git") {
        let path = std::env::var("PATH").unwrap_or_default();
        return find_git(&path, &crate::localfs::is_file);
    }
    local_path_ok(git.to_str()?).then(|| git.to_path_buf())
}

/// `run`, telling `started` the process id once it runs (for the tests).
fn run_with(git: &Path, dir: &Path, args: &[String], timeout: Duration, cap: Option<usize>, started: &mut dyn FnMut(u32)) -> RunResult {
    let deadline = Instant::now() + timeout;
    let Some(exe) = program(git) else { return RunResult::Failed(format!("{} was not found", git.display())) };
    if !dir.to_str().is_some_and(local_path_ok) {
        return RunResult::Failed(format!("{} is not a local folder", dir.display()));
    }
    let mut line = quote_arg(&exe.to_string_lossy());
    for a in args {
        line.push(' ');
        line.push_str(&quote_arg(a));
    }
    let env = env_block(std::env::vars_os());
    unsafe { run_in_job(&exe, dir, &line, &env, deadline, cap, started) }
}

/// One argument as the C runtime (and git's) reads the command line back.
fn quote_arg(a: &str) -> String {
    if !a.is_empty() && !a.contains([' ', '\t', '\n', '\u{b}', '"']) {
        return a.to_string();
    }
    let mut out = String::from('"');
    let mut slashes = 0;
    for c in a.chars() {
        if c == '\\' {
            slashes += 1;
            continue;
        }
        // Backslashes before a quote are doubled, and the quote escaped.
        let n = if c == '"' { slashes * 2 + 1 } else { slashes };
        out.extend(std::iter::repeat_n('\\', n));
        out.push(c);
        slashes = 0;
    }
    // Before the closing quote, doubled.
    out.extend(std::iter::repeat_n('\\', slashes * 2));
    out.push('"');
    out
}

/// The child's environment block: Termurion's, less `GIT_ENV_REMOVED` (any case, as Windows
/// names are), with `GIT_ENV` set; sorted as Windows keeps it.
fn env_block(vars: impl IntoIterator<Item = (OsString, OsString)>) -> Vec<u16> {
    let named = |k: &OsStr, n: &str| k.to_string_lossy().eq_ignore_ascii_case(n);
    let mut kept: Vec<(OsString, OsString)> =
        vars.into_iter().filter(|(k, _)| !GIT_ENV_REMOVED.iter().any(|r| named(k, r)) && !GIT_ENV.iter().any(|(s, _)| named(k, s))).collect();
    kept.extend(GIT_ENV.iter().map(|(k, v)| (OsString::from(k), OsString::from(v))));
    kept.sort_by_key(|(k, _)| k.to_string_lossy().to_uppercase());
    let mut out = Vec::new();
    for (k, v) in kept {
        out.extend(k.encode_wide());
        out.push(u16::from(b'='));
        out.extend(v.encode_wide());
        out.push(0);
    }
    if out.is_empty() {
        out.push(0);
    }
    out.push(0);
    out
}

fn os_err(what: &str) -> String {
    format!("{what} failed: {}", std::io::Error::last_os_error())
}

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}

/// What the stdout reader got, and whether it stopped at the cap.
type Output = (Vec<u8>, bool);

/// Reads the pipe on its own thread until it closes or passes `cap`, then sends it all.
/// Returns the thread too, so a read that never ends can be cancelled.
fn read_stdout(h: HANDLE, cap: Option<usize>) -> (mpsc::Receiver<Output>, std::thread::JoinHandle<()>) {
    let (tx, rx) = mpsc::channel();
    let raw = h as usize;
    let reader = std::thread::spawn(move || {
        let h = raw as HANDLE;
        let mut buf = vec![0u8; READ_SIZE];
        let mut got = Vec::new();
        let mut capped = false;
        loop {
            let mut n = 0u32;
            let ok = unsafe { ReadFile(h, buf.as_mut_ptr(), buf.len() as u32, &mut n, null_mut()) };
            if ok == 0 || n == 0 {
                break;
            }
            got.extend_from_slice(&buf[..n as usize]);
            if let Some(c) = cap.filter(|&c| got.len() > c) {
                got.truncate(c);
                capped = true;
                break;
            }
        }
        unsafe { CloseHandle(h) };
        let _ = tx.send((got, capped));
    });
    (rx, reader)
}

/// How the wait for git ended.
#[derive(Debug, PartialEq)]
enum Waited {
    /// It exited; the output when the reader had already finished.
    Exited(Option<Output>),
    /// The reader passed the cap first.
    Capped(Vec<u8>),
    TimedOut,
}

/// Waits for git (`exited` waits up to the given time and says whether it ended) until
/// `deadline`, keeping whatever the reader sends meanwhile: it can see the end of file
/// before the process is signaled.
fn wait_for(exited: &mut dyn FnMut(Duration) -> bool, output: &mpsc::Receiver<Output>, deadline: Instant) -> Waited {
    let mut early = None;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Waited::TimedOut;
        }
        if exited(left.min(POLL)) {
            return Waited::Exited(early);
        }
        if early.is_none() {
            match output.try_recv() {
                // Past the cap: stop git now, with what was kept.
                Ok((stdout, true)) => return Waited::Capped(stdout),
                Ok(out) => early = Some(out),
                Err(_) => {}
            }
        }
    }
}

/// The output once git exited: at least `DRAIN` to arrive however near the deadline; then
/// a read held open by a handle outside the job is cancelled (`cancel`) and what it got is
/// kept (git has exited, so its writes are all in the pipe).
fn drain(early: Option<Output>, output: &mpsc::Receiver<Output>, deadline: Instant, cancel: &mut dyn FnMut()) -> Option<Output> {
    if early.is_some() {
        return early;
    }
    if let Ok(out) = output.recv_timeout(deadline.saturating_duration_since(Instant::now()).max(DRAIN)) {
        return Some(out);
    }
    cancel();
    output.recv_timeout(DRAIN).ok()
}

/// Starts the program suspended, puts it in a kill-on-close job before it runs a single
/// instruction (so nothing it starts escapes), and waits for it until `deadline`.
unsafe fn run_in_job(exe: &Path, dir: &Path, line: &str, env: &[u16], deadline: Instant, cap: Option<usize>, started: &mut dyn FnMut(u32)) -> RunResult {
    unsafe {
        // Not inheritable: `start` lets only this child inherit them, only while it starts.
        let (mut out_r, mut out_w): (HANDLE, HANDLE) = (null_mut(), null_mut());
        if CreatePipe(&mut out_r, &mut out_w, null(), 0) == 0 {
            return RunResult::Failed(os_err("CreatePipe"));
        }
        let nul_name = wide(OsStr::new("NUL"));
        let nul = CreateFileW(nul_name.as_ptr(), GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, null(), OPEN_EXISTING, 0, null_mut());
        if nul == INVALID_HANDLE_VALUE {
            let e = os_err("CreateFileW (NUL)");
            CloseHandle(out_r);
            CloseHandle(out_w);
            return RunResult::Failed(e);
        }
        let started_job = start(exe, dir, line, env, [nul, out_w]);
        // The child has its own copies; the read end sees the end of file when it exits.
        CloseHandle(nul);
        CloseHandle(out_w);
        let (job, process, pid) = match started_job {
            Ok(v) => v,
            Err(e) => {
                CloseHandle(out_r);
                return RunResult::Failed(e);
            }
        };
        started(pid);
        let (output, reader) = read_stdout(out_r, cap);
        let end = |result: RunResult| {
            TerminateJobObject(job, 1);
            CloseHandle(job);
            CloseHandle(process);
            result
        };
        let early = match wait_for(&mut |d| WaitForSingleObject(process, d.as_millis() as u32) == WAIT_OBJECT_0, &output, deadline) {
            Waited::TimedOut => return end(RunResult::TimedOut),
            Waited::Capped(stdout) => return end(RunResult::Exit { code: -1, stdout, capped: true }),
            Waited::Exited(early) => early,
        };
        let mut code = 0u32;
        GetExitCodeProcess(process, &mut code);
        // Anything git left running goes with the job, closing its copies of the pipe.
        TerminateJobObject(job, 1);
        let mut cancel = || {
            CancelSynchronousIo(reader.as_raw_handle() as HANDLE);
        };
        match drain(early, &output, deadline, &mut cancel) {
            Some((stdout, capped)) => end(RunResult::Exit { code: code as i32, stdout, capped }),
            None => end(RunResult::TimedOut),
        }
    }
}

/// Creates the job and the suspended process, puts it in the job and lets it run. Only
/// `handles` are inherited (stdin and stderr, then stdout), never another pane's; they are
/// inheritable only during `CreateProcessW`. Returns
/// (job, process, pid).
unsafe fn start(exe: &Path, dir: &Path, line: &str, env: &[u16], handles: [HANDLE; 2]) -> Result<(HANDLE, HANDLE, u32), String> {
    unsafe {
        let job = CreateJobObjectW(null(), null());
        if job.is_null() {
            return Err(os_err("CreateJobObjectW"));
        }
        let fail = |e: String| {
            CloseHandle(job);
            Err(e)
        };
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if SetInformationJobObject(job, JobObjectExtendedLimitInformation, (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(), std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32) == 0 {
            return fail(os_err("SetInformationJobObject"));
        }
        let mut size = 0usize;
        InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut size);
        let mut buf = vec![0u8; size];
        let attrs = buf.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
        if InitializeProcThreadAttributeList(attrs, 1, 0, &mut size) == 0 {
            return fail(os_err("InitializeProcThreadAttributeList"));
        }
        if UpdateProcThreadAttribute(attrs, 0, PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize, handles.as_ptr().cast(), std::mem::size_of_val(&handles), null_mut(), null()) == 0 {
            DeleteProcThreadAttributeList(attrs);
            return fail(os_err("UpdateProcThreadAttribute"));
        }
        let mut si: STARTUPINFOEXW = std::mem::zeroed();
        si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        si.StartupInfo.hStdInput = handles[0];
        si.StartupInfo.hStdOutput = handles[1];
        si.StartupInfo.hStdError = handles[0];
        si.lpAttributeList = attrs;
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
        let app = wide(exe.as_os_str());
        let mut cmd = wide(OsStr::new(line));
        let cwd = wide(dir.as_os_str());
        let flags = EXTENDED_STARTUPINFO_PRESENT | CREATE_NO_WINDOW | CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT;
        // Inheritable only for this call: the handle list needs it, and no other spawn can
        // take them outside it (std's `Command` inherits every inheritable handle).
        for h in handles {
            SetHandleInformation(h, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
        }
        let ok = CreateProcessW(app.as_ptr(), cmd.as_mut_ptr(), null(), null(), 1, flags, env.as_ptr().cast(), cwd.as_ptr(), &si.StartupInfo, &mut pi);
        for h in handles {
            SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0);
        }
        DeleteProcThreadAttributeList(attrs);
        if ok == 0 {
            return fail(os_err("CreateProcessW (git)"));
        }
        if AssignProcessToJobObject(job, pi.hProcess) == 0 {
            let e = os_err("AssignProcessToJobObject");
            TerminateProcess(pi.hProcess, 1);
            CloseHandle(pi.hThread);
            CloseHandle(pi.hProcess);
            return fail(e);
        }
        if ResumeThread(pi.hThread) == u32::MAX {
            let e = os_err("ResumeThread");
            TerminateJobObject(job, 1);
            CloseHandle(pi.hThread);
            CloseHandle(pi.hProcess);
            return fail(e);
        }
        CloseHandle(pi.hThread);
        Ok((job, pi.hProcess, pi.dwProcessId))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    fn alive(pid: u32) -> bool {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return false;
            }
            let mut code = 0u32;
            GetExitCodeProcess(h, &mut code);
            CloseHandle(h);
            code == 259
        }
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// Review Focus 2: a git that hangs ends at the deadline with its whole tree: the ping
    /// under `cmd.exe` is gone too.
    #[test]
    fn hung_git_is_killed_at_the_deadline() {
        let cmd = Path::new(&std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())).join(r"System32\cmd.exe");
        let pid = Arc::new(Mutex::new(None::<u32>));
        let pings = Arc::new(Mutex::new(HashSet::new()));
        let done = Arc::new(AtomicBool::new(false));
        let watcher = {
            let (pid, pings, done) = (pid.clone(), pings.clone(), done.clone());
            std::thread::spawn(move || {
                while !done.load(Ordering::Acquire) {
                    if let Some(p) = *pid.lock().unwrap() {
                        for d in crate::jobwatch::descendants_now(p).unwrap_or_default() {
                            if d.name.eq_ignore_ascii_case("PING.EXE") {
                                pings.lock().unwrap().insert(d.pid);
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            })
        };
        let start = Instant::now();
        let got = run_with(&cmd, &std::env::temp_dir(), &strings(&["/d", "/c", "ping", "-n", "30", "127.0.0.1"]), Duration::from_secs(1), None, &mut |p| {
            *pid.lock().unwrap() = Some(p);
        });
        let took = start.elapsed();
        done.store(true, Ordering::Release);
        watcher.join().unwrap();
        assert_eq!(got, RunResult::TimedOut);
        assert!(took >= Duration::from_millis(900) && took < Duration::from_secs(2), "{took:?}");
        let pings = pings.lock().unwrap().clone();
        assert!(!pings.is_empty(), "ping ran under cmd.exe");
        std::thread::sleep(Duration::from_millis(300));
        let cmd_pid = pid.lock().unwrap().unwrap();
        assert!(!alive(cmd_pid), "cmd.exe is gone");
        for p in pings {
            assert!(!alive(p), "ping {p} is gone");
        }
    }

    /// A bare `git` is found through `find_git` and runs hidden in a job: its output comes
    /// back, and a capped read says so.
    #[test]
    fn git_runs_hidden_in_a_job() {
        let dir = std::env::temp_dir();
        let got = WinGitRun.run(Path::new("git"), &dir, &strings(&["--version"]), Duration::from_secs(10), Some(64 * 1024));
        match got {
            RunResult::Exit { code: 0, stdout, capped: false } => assert!(String::from_utf8_lossy(&stdout).starts_with("git version "), "{stdout:?}"),
            other => panic!("{other:?}"),
        }
        match WinGitRun.run(Path::new("git"), &dir, &strings(&["--version"]), Duration::from_secs(10), Some(4)) {
            RunResult::Exit { stdout, capped: true, .. } => assert_eq!(stdout, b"git "),
            other => panic!("{other:?}"),
        }
    }

    /// The reader can finish before the process is signaled (git closes stdout, then
    /// exits): its output is kept, never read as a timeout; the cap still stops git early.
    #[test]
    fn output_before_the_exit_signal_is_kept() {
        let deadline = Instant::now() + Duration::from_secs(5);
        let (tx, rx) = mpsc::channel();
        tx.send((b"ok".to_vec(), false)).unwrap();
        drop(tx);
        let mut calls = 0;
        let got = wait_for(&mut |_| { calls += 1; calls > 2 }, &rx, deadline);
        assert_eq!(got, Waited::Exited(Some((b"ok".to_vec(), false))));
        assert_eq!(drain(Some((b"ok".to_vec(), false)), &rx, deadline, &mut || {}), Some((b"ok".to_vec(), false)));

        let (tx, rx) = mpsc::channel();
        tx.send((b"ab".to_vec(), true)).unwrap();
        assert_eq!(wait_for(&mut |_| false, &rx, deadline), Waited::Capped(b"ab".to_vec()));
        let (_tx, rx) = mpsc::channel::<Output>();
        assert_eq!(wait_for(&mut |_| false, &rx, Instant::now() + Duration::from_millis(50)), Waited::TimedOut);
    }

    /// After the exit the output gets at least `DRAIN` even at the deadline; a read held
    /// open elsewhere is cancelled and its output kept.
    #[test]
    fn output_drains_after_the_exit() {
        let (tx, rx) = mpsc::channel();
        let late = tx.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            let _ = late.send((b"late".to_vec(), false));
        });
        assert_eq!(drain(None, &rx, Instant::now(), &mut || {}), Some((b"late".to_vec(), false)), "a deadline already passed still waits DRAIN");
        let mut cancelled = false;
        let got = drain(None, &rx, Instant::now(), &mut || {
            cancelled = true;
            tx.send((b"held".to_vec(), false)).unwrap();
        });
        assert!(cancelled);
        assert_eq!(got, Some((b"held".to_vec(), false)));
    }

    /// Windows' argument quoting, as git's C runtime reads it back.
    #[test]
    fn arguments_are_quoted_for_the_c_runtime() {
        assert_eq!(quote_arg("status"), "status");
        assert_eq!(quote_arg(""), "\"\"");
        assert_eq!(quote_arg("a b"), "\"a b\"");
        assert_eq!(quote_arg(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(quote_arg(r"C:\dir\"), r"C:\dir\");
        assert_eq!(quote_arg(r"C:\my dir\"), r#""C:\my dir\\""#);
        assert_eq!(quote_arg(r#"a\"b"#), r#""a\\\"b""#);
    }

    /// The child's environment: `GIT_ENV` set, `GIT_ENV_REMOVED` gone whatever its case,
    /// the rest kept; a Windows block ends in two NULs.
    #[test]
    fn environment_block_is_hardened() {
        let vars = vec![("Path".to_string(), r"C:\x".to_string()), ("git_dir".to_string(), "evil".to_string()), ("GIT_TERMINAL_PROMPT".to_string(), "1".to_string())];
        let block = String::from_utf16(&env_block(vars.into_iter().map(|(k, v)| (k.into(), v.into())))).unwrap();
        let entries: Vec<&str> = block.trim_end_matches('\0').split('\0').collect();
        assert!(entries.contains(&"Path=C:\\x"), "{entries:?}");
        assert!(entries.contains(&"GIT_TERMINAL_PROMPT=0"), "{entries:?}");
        assert!(entries.contains(&"GIT_OPTIONAL_LOCKS=0") && entries.contains(&"GIT_NO_LAZY_FETCH=1"), "{entries:?}");
        assert!(!entries.iter().any(|e| e.to_ascii_uppercase().starts_with("GIT_DIR=")), "{entries:?}");
        assert!(block.ends_with("\0\0"));
    }
}

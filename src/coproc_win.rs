//! Runs a coprocess: `cmd.exe /d /s /c <command>`, hidden, with pipes for stdin, stdout,
//! and stderr, in its own Job Object so stopping it ends every program it started.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::ReadFile;
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
    InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION,
    ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute,
};

use crate::coprocess::{Backlog, Stderr};

/// Chunks of coprocess output that may wait for the window; past this the reader waits,
/// so a coprocess that floods is held back by its own pipe.
const QUEUED_CHUNKS: usize = 16;
const READ_SIZE: usize = 1024;

fn os_err(what: &str) -> String {
    format!("{what} failed: {}", std::io::Error::last_os_error())
}

/// A running coprocess.
pub struct Coprocess {
    pub command: String,
    /// Run Silent Coprocess: the session's output is not drawn while it runs.
    pub mute: bool,
    job: HANDLE,
    input: Option<mpsc::Sender<Vec<u8>>>,
    consumed: Arc<AtomicUsize>,
    backlog: Backlog,
    output: Receiver<Vec<u8>>,
    /// A piece `finished` took while looking; `try_output` gives it first.
    held: Option<Vec<u8>>,
    stdout_done: Arc<AtomicBool>,
    stderr: Arc<Mutex<Stderr>>,
    pid: u32,
}

// SAFETY: the job handle is a process-wide kernel handle, usable from any thread.
unsafe impl Send for Coprocess {}

fn pipe(sa: &SECURITY_ATTRIBUTES) -> Result<(HANDLE, HANDLE), String> {
    let (mut r, mut w): (HANDLE, HANDLE) = (null_mut(), null_mut());
    if unsafe { CreatePipe(&mut r, &mut w, sa, 0) } == 0 {
        return Err(os_err("CreatePipe"));
    }
    Ok((r, w))
}

fn no_inherit(h: HANDLE) {
    unsafe { SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0) };
}

/// Reads a pipe until it closes, handing each piece to `each`.
fn read_all(h: HANDLE, mut each: impl FnMut(&[u8]) -> bool + Send + 'static, done: impl FnOnce() + Send + 'static) {
    let raw = h as usize;
    std::thread::spawn(move || {
        let h = raw as HANDLE;
        let mut buf = vec![0u8; READ_SIZE];
        loop {
            let mut n = 0u32;
            let ok = unsafe { ReadFile(h, buf.as_mut_ptr(), buf.len() as u32, &mut n, null_mut()) };
            if ok == 0 || n == 0 || !each(&buf[..n as usize]) {
                break;
            }
        }
        unsafe { CloseHandle(h) };
        done();
    });
}

impl Coprocess {
    /// Starts `command` in `cwd`. `notify` wakes the window when output comes or it ends.
    pub fn start(command: &str, mute: bool, cwd: &std::path::Path, notify: Arc<dyn Fn() + Send + Sync>) -> Result<Coprocess, String> {
        let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: null_mut(), bInheritHandle: 1 };
        let (in_r, in_w) = pipe(&sa)?;
        let (out_r, out_w) = pipe(&sa).inspect_err(|_| unsafe {
            CloseHandle(in_r);
            CloseHandle(in_w);
        })?;
        let (err_r, err_w) = pipe(&sa).inspect_err(|_| unsafe {
            for h in [in_r, in_w, out_r, out_w] {
                CloseHandle(h);
            }
        })?;
        for h in [in_w, out_r, err_r] {
            no_inherit(h);
        }
        let child_ends = [in_r, out_w, err_w];
        let started = unsafe { spawn(command, cwd, child_ends) };
        for h in child_ends {
            unsafe { CloseHandle(h) };
        }
        let (job, pid) = match started {
            Ok(v) => v,
            Err(e) => {
                for h in [in_w, out_r, err_r] {
                    unsafe { CloseHandle(h) };
                }
                return Err(e);
            }
        };
        // stdin: a writer thread, so a coprocess that does not read never blocks the window.
        let consumed = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let (raw, taken) = (in_w as usize, consumed.clone());
        std::thread::spawn(move || {
            let h = raw as HANDLE;
            'chunks: for chunk in rx {
                let mut done = 0;
                while done < chunk.len() {
                    let mut n = 0u32;
                    let rest = &chunk[done..];
                    if unsafe { windows_sys::Win32::Storage::FileSystem::WriteFile(h, rest.as_ptr(), rest.len() as u32, &mut n, null_mut()) } == 0 {
                        // The coprocess closed its stdin: later writes are dropped (iTerm2).
                        break 'chunks;
                    }
                    done += n as usize;
                }
                taken.fetch_add(chunk.len(), Ordering::Relaxed);
            }
            unsafe { CloseHandle(h) };
        });
        let (otx, orx): (SyncSender<Vec<u8>>, Receiver<Vec<u8>>) = mpsc::sync_channel(QUEUED_CHUNKS);
        let stdout_done = Arc::new(AtomicBool::new(false));
        let (n1, n2, flag) = (notify.clone(), notify.clone(), stdout_done.clone());
        read_all(
            out_r,
            move |b| {
                let ok = otx.send(b.to_vec()).is_ok();
                n1();
                ok
            },
            move || {
                flag.store(true, Ordering::Release);
                n2();
            },
        );
        let stderr = Arc::new(Mutex::new(Stderr::default()));
        let keep = stderr.clone();
        read_all(err_r, move |b| {
            keep.lock().unwrap_or_else(|e| e.into_inner()).push(&String::from_utf8_lossy(b));
            true
        }, || {});
        Ok(Coprocess { command: command.to_string(), mute, job, input: Some(tx), consumed, backlog: Backlog::default(), output: orx, held: None, stdout_done, stderr, pid })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Terminal output for the coprocess (raw, before parsing). Past the backlog cap it is
    /// dropped and counted in `dropped()`.
    pub fn write(&mut self, bytes: &[u8]) {
        self.backlog.sent(self.consumed.swap(0, Ordering::Relaxed));
        let Some(tx) = &self.input else { return };
        if self.backlog.admit(bytes.len()) && tx.send(bytes.to_vec()).is_err() {
            self.input = None;
        }
    }

    /// Bytes dropped because the coprocess did not read them.
    pub fn dropped(&self) -> usize {
        self.backlog.dropped
    }

    /// The next piece of coprocess output, if any.
    pub fn try_output(&mut self) -> Option<Vec<u8>> {
        if let Some(h) = self.held.take() {
            return Some(h);
        }
        match self.output.try_recv() {
            Ok(c) => Some(c),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }

    /// Its stdout closed (it exited) and every piece was taken.
    pub fn finished(&mut self) -> bool {
        if self.held.is_some() || !self.stdout_done.load(Ordering::Acquire) {
            return false;
        }
        match self.output.try_recv() {
            Ok(c) => {
                self.held = Some(c);
                false
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => true,
        }
    }

    pub fn stderr(&self) -> String {
        self.stderr.lock().unwrap_or_else(|e| e.into_inner()).text().to_string()
    }

    /// Closes its input (end of file for the coprocess).
    pub fn close_input(&mut self) {
        self.input = None;
    }

    /// Ends the coprocess and every program it started.
    pub fn stop(&mut self) {
        self.input = None;
        if !self.job.is_null() {
            unsafe {
                TerminateJobObject(self.job, 1);
                CloseHandle(self.job);
            }
            self.job = null_mut();
        }
    }
}

impl Drop for Coprocess {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Creates the job and the process in it. Returns (job, pid).
unsafe fn spawn(command: &str, cwd: &std::path::Path, ends: [HANDLE; 3]) -> Result<(HANDLE, u32), String> {
    unsafe {
        let job = CreateJobObjectW(null(), null());
        if job.is_null() {
            return Err(os_err("CreateJobObjectW"));
        }
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if SetInformationJobObject(job, JobObjectExtendedLimitInformation, (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(), std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32) == 0 {
            let e = os_err("SetInformationJobObject");
            CloseHandle(job);
            return Err(e);
        }
        let fail = |e: String| {
            CloseHandle(job);
            Err(e)
        };
        let mut size = 0usize;
        InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut size);
        let mut buf = vec![0u8; size];
        let attrs = buf.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
        if InitializeProcThreadAttributeList(attrs, 1, 0, &mut size) == 0 {
            return fail(os_err("InitializeProcThreadAttributeList"));
        }
        // Only the three pipe ends are inherited, never another pane's handles.
        if UpdateProcThreadAttribute(attrs, 0, PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize, ends.as_ptr().cast(), std::mem::size_of_val(&ends), null_mut(), null()) == 0 {
            DeleteProcThreadAttributeList(attrs);
            return fail(os_err("UpdateProcThreadAttribute"));
        }
        let mut si: STARTUPINFOEXW = std::mem::zeroed();
        si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        si.StartupInfo.hStdInput = ends[0];
        si.StartupInfo.hStdOutput = ends[1];
        si.StartupInfo.hStdError = ends[2];
        si.lpAttributeList = attrs;
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
        // /s: cmd strips exactly the outer quotes, so the command is passed as typed.
        let line = format!("cmd.exe /d /s /c \"{command}\"");
        let mut cmd: Vec<u16> = OsStr::new(&line).encode_wide().chain(Some(0)).collect();
        let dir: Vec<u16> = cwd.as_os_str().encode_wide().chain(Some(0)).collect();
        let ok = CreateProcessW(null(), cmd.as_mut_ptr(), null(), null(), 1, EXTENDED_STARTUPINFO_PRESENT | CREATE_NO_WINDOW | CREATE_SUSPENDED, null(), dir.as_ptr(), &si.StartupInfo, &mut pi);
        DeleteProcThreadAttributeList(attrs);
        if ok == 0 {
            return fail(os_err("CreateProcessW (coprocess)"));
        }
        if AssignProcessToJobObject(job, pi.hProcess) == 0 {
            let e = os_err("AssignProcessToJobObject");
            TerminateProcess(pi.hProcess, 1);
            CloseHandle(pi.hThread);
            CloseHandle(pi.hProcess);
            return fail(e);
        }
        ResumeThread(pi.hThread);
        CloseHandle(pi.hThread);
        CloseHandle(pi.hProcess);
        Ok((job, pi.dwProcessId))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait_for(co: &mut Coprocess, secs: u64) -> Vec<u8> {
        let mut got = Vec::new();
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end && !co.finished() {
            while let Some(c) = co.try_output() {
                got.extend(c);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        while let Some(c) = co.try_output() {
            got.extend(c);
        }
        got
    }

    fn alive(pid: u32) -> bool {
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

    #[test]
    fn coprocess_round_trip() {
        let home = std::env::temp_dir();
        let mut co = Coprocess::start("findstr .", false, &home, Arc::new(|| {})).unwrap();
        co.write(b"hello\r\n");
        std::thread::sleep(Duration::from_millis(200));
        co.close_input();
        let out = wait_for(&mut co, 10);
        assert!(String::from_utf8_lossy(&out).contains("hello"), "out {:?} err {:?}", String::from_utf8_lossy(&out), co.stderr());
        assert!(co.finished());
        let mut e = Coprocess::start("echo oops 1>&2", false, &home, Arc::new(|| {})).unwrap();
        wait_for(&mut e, 10);
        std::thread::sleep(Duration::from_millis(100));
        assert!(e.stderr().contains("oops"), "{:?}", e.stderr());
    }

    #[test]
    fn pane_close_ends_the_coprocess() {
        let home = std::env::temp_dir();
        let co = Coprocess::start("ping -n 60 127.0.0.1", false, &home, Arc::new(|| {})).unwrap();
        std::thread::sleep(Duration::from_millis(500));
        let procs = crate::jobwatch::descendants_now(co.pid()).unwrap();
        let ping = procs.iter().find(|p| p.name.eq_ignore_ascii_case("PING.EXE")).expect("ping runs under cmd").pid;
        let cmd = co.pid();
        drop(co);
        std::thread::sleep(Duration::from_millis(500));
        assert!(!alive(cmd) && !alive(ping), "the whole tree ends");
    }
}

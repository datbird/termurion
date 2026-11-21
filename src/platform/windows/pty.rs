//! A child process running inside a ConPTY pseudoconsole.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;
use std::ptr::{null, null_mut};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    BOOL, CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation, WAIT_OBJECT_0,
};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadFile, WriteFile};
use windows_sys::Win32::System::Console::{
    AttachConsole, COORD, CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, FreeConsole, GetConsoleMode, GetStdHandle, HPCON, STD_ERROR_HANDLE, STD_INPUT_HANDLE,
    STD_OUTPUT_HANDLE, SetConsoleCtrlHandler, SetStdHandle,
};
use windows_sys::Win32::System::IO::CancelSynchronousIo;
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DETACHED_PROCESS, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
    GetCurrentProcess, GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
    STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
};

use super::conpty::Api;
use crate::platform::pty::{Notify, Read};
use crate::pty_budget::Budget;
use crate::ssh_transport::Transport;

/// One console attach at a time (Send Signal's events, password prompt mode reads, the write
/// side), and never across a pseudoconsole close: `Pty::drop` holds the read side, because
/// closing a pseudoconsole ends every process attached to it, and Termurion (or termurion-host) must
/// never be one then. Closes share the read side, so they still run side by side.
pub(crate) static CONSOLE: std::sync::RwLock<()> = std::sync::RwLock::new(());

/// Which consoles attaches want: OpenConsole blocks console calls on a full output pipe, so the
/// reader of a console being attached to (or waited for) reads past its budget. Only that one:
/// a flood held at its budget must not grow while other consoles are read.
#[derive(Default)]
struct Wanted {
    /// Live readers by their shell's pid (a count: a reused pid's old reader may still drain).
    readers: std::collections::HashMap<u32, usize>,
    /// Attaches waiting for `CONSOLE` or running, by target pid.
    by_pid: std::collections::HashMap<u32, usize>,
    /// Attaches to a pid that is no reader's shell (Send Signal to a job): its console is not
    /// known, so every reader reads past its budget meanwhile.
    unknown: usize,
}

static CONSOLE_WANTED: std::sync::Mutex<Option<Wanted>> = std::sync::Mutex::new(None);

fn wanted<T>(f: impl FnOnce(&mut Wanted) -> T) -> T {
    f(CONSOLE_WANTED.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default))
}

fn count(map: &mut std::collections::HashMap<u32, usize>, key: u32, up: bool) {
    if up {
        *map.entry(key).or_default() += 1;
    } else if let Some(n) = map.get_mut(&key) {
        *n -= 1;
        if *n == 0 {
            map.remove(&key);
        }
    }
}

/// Marks an attach to `pid` until it returns, however it returns.
struct WantMark(Option<u32>);

impl WantMark {
    fn new(pid: u32) -> WantMark {
        wanted(|w| {
            if w.readers.contains_key(&pid) {
                count(&mut w.by_pid, pid, true);
                WantMark(Some(pid))
            } else {
                w.unknown += 1;
                WantMark(None)
            }
        })
    }
}

impl Drop for WantMark {
    fn drop(&mut self) {
        wanted(|w| match self.0 {
            Some(pid) => count(&mut w.by_pid, pid, false),
            None => w.unknown -= 1,
        });
    }
}

/// An attach wants the console of the shell `pid`: its reader reads past its budget.
fn console_wanted(pid: u32) -> bool {
    wanted(|w| w.unknown > 0 || w.by_pid.contains_key(&pid))
}

/// Attached right now (inside `attached`).
static ATTACHED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The process's answer to console events while attached: Ctrl+C and Ctrl+Break (sent by Send
/// Signal, or typed in that console) are handled, so they never end it. A close event while
/// attached is answered handled too, so no other handler runs; Windows still ends the process
/// after a close event whatever the answer, which is why a close never happens while attached
/// (`CONSOLE`). Registered after each attach, never removed on the way out (the null handler's
/// ignore flag would be inherited by every shell started after it, and removing a handler
/// before the event's thread runs here would let the default handler exit).
unsafe extern "system" fn ignore_ctrl(kind: u32) -> BOOL {
    let (handled, seen) = ctrl_answer(kind, ATTACHED.load(std::sync::atomic::Ordering::SeqCst));
    if seen {
        SEEN.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    handled as BOOL
}

/// `ignore_ctrl`'s answer to an event while attached or not: whether it is handled, and
/// whether it counts as this process's own Ctrl+C or Ctrl+Break seen while attached.
fn ctrl_answer(kind: u32, attached: bool) -> (bool, bool) {
    let ctrl = kind == CTRL_C_EVENT || kind == CTRL_BREAK_EVENT;
    (ctrl || (kind == CTRL_CLOSE_EVENT && attached), ctrl && attached)
}

/// Set by `ignore_ctrl` when a Ctrl+C or Ctrl+Break reaches this process while attached.
static SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Sends Ctrl+C (or Ctrl+Break) to the whole console this process is attached to (inside
/// `attached_unless`), then stays attached until `ignore_ctrl` has seen its own copy, at most
/// `wait`. Windows delivers that copy on a thread it starts a little later; were it to land
/// after the detach and a next attach (which starts the handler list over), only the default
/// handler would answer, and Ctrl+Break would end this process. True when it was seen (a
/// process that ignores Ctrl+C never sees it and waits out `wait`).
pub(crate) fn send_ctrl_event_attached(ctrl_break: bool, wait: Duration) -> Result<bool, String> {
    SEEN.store(false, std::sync::atomic::Ordering::SeqCst);
    let event = if ctrl_break { CTRL_BREAK_EVENT } else { CTRL_C_EVENT };
    if unsafe { windows_sys::Win32::System::Console::GenerateConsoleCtrlEvent(event, 0) } == 0 {
        return Err(format!("GenerateConsoleCtrlEvent failed: {}", std::io::Error::last_os_error()));
    }
    let end = Instant::now() + wait;
    while !SEEN.load(std::sync::atomic::Ordering::SeqCst) {
        if Instant::now() >= end {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(true)
}

/// Runs `f` attached to the console `pid` runs on, under `CONSOLE`, with the ignore-Ctrl
/// handler installed; refused (no attach) when `stop` is set once `CONSOLE` is held. The
/// standard handles the attach may set are put back after. Nothing may log to stderr inside
/// `f` (it would show in that console): errors come back and are reported by the caller,
/// after the detach.
pub(crate) fn attached_unless<T>(pid: u32, stop: &std::sync::atomic::AtomicBool, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let _wanted = WantMark::new(pid);
    let _one = CONSOLE.write().unwrap_or_else(|e| e.into_inner());
    if stop.load(std::sync::atomic::Ordering::Acquire) {
        return Err("the console is paused".into());
    }
    let kinds = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE];
    let before = kinds.map(|k| unsafe { GetStdHandle(k) });
    if unsafe { AttachConsole(pid) } == 0 {
        return Err(format!("AttachConsole failed: {}", std::io::Error::last_os_error()));
    }
    // After the attach, every time: an attach starts the process's handler list over
    // (measured in a windows-subsystem build: a handler added before the attach, or at an
    // earlier one, was not asked, and a Ctrl+Break ended the process). The removal finds
    // nothing then; it only keeps the list from growing should an attach ever keep it.
    unsafe {
        SetConsoleCtrlHandler(Some(ignore_ctrl), 0);
        SetConsoleCtrlHandler(Some(ignore_ctrl), 1);
    }
    ATTACHED.store(true, std::sync::atomic::Ordering::SeqCst);
    let result = f();
    unsafe {
        FreeConsole();
        ATTACHED.store(false, std::sync::atomic::Ordering::SeqCst);
        for (k, h) in kinds.into_iter().zip(before) {
            if GetStdHandle(k) != h {
                SetStdHandle(k, h);
            }
        }
    }
    result
}

/// The input mode of the console `pid` runs on (`ENABLE_ECHO_INPUT`, `ENABLE_LINE_INPUT`, ...):
/// attached for the moment, `CONIN$` opened, read and closed. Never while a console event is
/// being sent or a pseudoconsole is closing (all hold `CONSOLE`). Only for a console this
/// process owns the closes of: Termurion reads its local panes, termurion-host its own sessions.
///
/// There is no timeout: a console whose OpenConsole hangs (its output pipe full and not
/// read) would hold this, and `CONSOLE` with it, for good. Termurion never reads a pane it
/// paused, and termurion-host skips paused sessions (`console_mode_unless`).
pub fn console_mode(pid: u32) -> Result<u32, String> {
    console_mode_unless(pid, &std::sync::atomic::AtomicBool::new(false))
}

/// `console_mode`, refused without attaching when `paused` is set, checked under `CONSOLE`
/// (so a pause made with `Pty::pause_after_reads` never lands inside the read).
pub fn console_mode_unless(pid: u32, paused: &std::sync::atomic::AtomicBool) -> Result<u32, String> {
    attached_unless(pid, paused, || {
        let name: Vec<u16> = "CONIN$".encode_utf16().chain(Some(0)).collect();
        let h = unsafe { CreateFileW(name.as_ptr(), GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, null(), OPEN_EXISTING, 0, null_mut()) };
        if h == INVALID_HANDLE_VALUE {
            return Err(format!("CreateFileW(CONIN$) failed: {}", std::io::Error::last_os_error()));
        }
        let mut mode = 0u32;
        let ok = unsafe { GetConsoleMode(h, &mut mode) } != 0;
        let err = (!ok).then(|| format!("GetConsoleMode failed: {}", std::io::Error::last_os_error()));
        unsafe { CloseHandle(h) };
        err.map_or(Ok(mode), Err)
    })
}

pub struct Pty {
    api: Api,
    /// 0 for a pipes-only child (`Transport::Pipes`): no pseudoconsole to resize, clear or close.
    hpc: HPCON,
    /// Input goes through a writer thread, so a child that is slow to read never
    /// blocks the caller. Dropping the sender ends the thread, which closes the pipe.
    writer: Option<mpsc::Sender<Vec<u8>>>,
    write_error: Arc<std::sync::Mutex<Option<String>>>,
    /// Bytes sent to the writer and not yet written.
    pending: Arc<std::sync::atomic::AtomicUsize>,
    /// Undo Close: the reader holds off, so the program waits on ConPTY (iTerm2 pauses).
    paused: Arc<std::sync::atomic::AtomicBool>,
    process: HANDLE,
    output: Receiver<Vec<u8>>,
    /// Output read and not yet taken: the reader stops at the budget.
    budget: Arc<Budget>,
    reader: Option<JoinHandle<()>>,
    reader_done: Receiver<()>,
    /// Something kept alive as long as the pty (a handed-off console's slot in the live count).
    held: Option<Box<dyn std::any::Any + Send>>,
}

/// How long Drop waits for ConPTY to close the output pipe before it ends the child.
const CLOSE_WAIT: Duration = Duration::from_secs(2);

fn check_size(cols: u16, rows: u16) -> Result<(), String> {
    if cols == 0 || rows == 0 || cols > i16::MAX as u16 || rows > i16::MAX as u16 {
        return Err(format!("bad pty size {cols}x{rows}"));
    }
    Ok(())
}

// SAFETY: the raw handles are process-wide kernel handles, valid on any thread. Pty is
// not Sync, so only one thread uses them at a time.
unsafe impl Send for Pty {}

fn os_err(what: &str) -> String {
    format!("{what} failed: {}", std::io::Error::last_os_error())
}

fn coord(cols: u16, rows: u16) -> COORD {
    COORD { X: cols as i16, Y: rows as i16 }
}

/// pwsh.exe when it is on the PATH, else Windows PowerShell.
pub fn default_shell() -> String {
    let found = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join("pwsh.exe").is_file()))
        .unwrap_or(false);
    if found { "pwsh.exe -NoLogo".into() } else { "powershell.exe -NoLogo".into() }
}

/// Whether the environment Termurion inherited names a `TERM` the child gets too.
pub fn inherits_term() -> bool {
    std::env::var_os("TERM").is_some()
}

pub fn spawn(api: &Api, cmdline: &str, cols: u16, rows: u16) -> Result<Pty, String> {
    spawn_notify(api, cmdline, None, &[], cols, rows, Arc::new(|| {}))
}

/// Starts `cmdline` in a new pseudoconsole, in `cwd` when given. `env`'s `NAME=value` entries
/// go over the inherited environment; with none the child inherits it unchanged.
pub fn spawn_notify(api: &Api, cmdline: &str, cwd: Option<&std::path::Path>, env: &[String], cols: u16, rows: u16, notify: Notify) -> Result<Pty, String> {
    spawn_with(api, cmdline, cwd, env, 0, cols, rows, notify)
}

/// `spawn` with these `CreatePseudoConsole` flags (`ssh_transport::conpty_flags`).
pub fn spawn_flags(api: &Api, cmdline: &str, flags: u32, cols: u16, rows: u16) -> Result<Pty, String> {
    spawn_with(api, cmdline, None, &[], flags, cols, rows, Arc::new(|| {}))
}

/// Starts `cmdline` on transport `t`: Passthrough and Conpty in a pseudoconsole created with
/// `ssh_transport::conpty_flags`, Pipes on two anonymous pipes with no console (a `Pty` with no
/// pseudoconsole: resize and clear do nothing). `cwd` and `env` as `spawn_notify`.
#[allow(clippy::too_many_arguments)]
pub fn spawn_transport(api: &Api, t: Transport, cmdline: &str, cwd: Option<&std::path::Path>, env: &[String], cols: u16, rows: u16, notify: Notify) -> Result<Pty, String> {
    match crate::ssh_transport::conpty_flags(t) {
        Some(flags) => spawn_with(api, cmdline, cwd, env, flags, cols, rows, notify),
        None => {
            check_size(cols, rows)?;
            let (input, output, process) = unsafe { start_pipes(cmdline, cwd, env)? };
            assemble(api, 0, input, output, process, notify)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_with(api: &Api, cmdline: &str, cwd: Option<&std::path::Path>, env: &[String], flags: u32, cols: u16, rows: u16, notify: Notify) -> Result<Pty, String> {
    check_size(cols, rows)?;
    let (hpc, input, output, process) = unsafe { start(api, cmdline, cwd, env, flags, cols, rows)? };
    assemble(api, hpc, input, output, process, notify)
}

/// The `Pty` around a started child: its reader and writer threads and its exit watch.
fn assemble(api: &Api, hpc: HPCON, input: HANDLE, output: HANDLE, process: HANDLE, notify: Notify) -> Result<Pty, String> {
    let paused = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let budget = Budget::new();
    let pid = unsafe { windows_sys::Win32::System::Threading::GetProcessId(process) };
    let (rx, reader_done, reader) = start_reader(output, pid, notify.clone(), paused.clone(), budget.clone());
    let pending = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (writer, write_error) = start_writer(input, pending.clone(), notify.clone());
    let pty = Pty { api: api.clone(), hpc, writer: Some(writer), write_error, pending, paused, process, output: rx, budget, reader: Some(reader), reader_done, held: None };
    watch_exit(pty.process, notify)?;
    Ok(pty)
}

/// The `Pty` of a console a console host handed over (the default-terminal handoff): `hpc` is the
/// packed pseudoconsole (`ConptyPackPseudoConsole`), `input` our write end and `output` our read
/// end of the two pipes, `client` the client process. Termurion owns all four from here: a failure
/// closes them (the pseudoconsole through the api).
pub fn from_handoff(api: &Api, hpc: HPCON, input: HANDLE, output: HANDLE, client: HANDLE, notify: Notify) -> Result<Pty, String> {
    assemble(api, hpc, input, output, client, notify)
}

/// Calls `notify` once the child exits. It waits on its own copy of the process
/// handle, so the Pty can close its copy at any time.
fn watch_exit(process: HANDLE, notify: Notify) -> Result<(), String> {
    let mut copy: HANDLE = null_mut();
    let ok = unsafe {
        DuplicateHandle(GetCurrentProcess(), process, GetCurrentProcess(), &mut copy, 0, 0, DUPLICATE_SAME_ACCESS)
    };
    if ok == 0 {
        return Err(os_err("DuplicateHandle"));
    }
    let raw = copy as usize;
    std::thread::spawn(move || {
        let copy = raw as HANDLE;
        unsafe {
            WaitForSingleObject(copy, INFINITE);
            CloseHandle(copy);
        }
        notify();
    });
    Ok(())
}

/// Creates the pseudoconsole (with `flags`, 0 for every pane but an SSH transport) and the
/// child. Returns (pseudoconsole, our input write end, our output read end, child process).
unsafe fn start(api: &Api, cmdline: &str, cwd: Option<&std::path::Path>, env: &[String], flags: u32, cols: u16, rows: u16) -> Result<(HPCON, HANDLE, HANDLE, HANDLE), String> {
    unsafe {
        let (mut in_read, mut in_write): (HANDLE, HANDLE) = (null_mut(), null_mut());
        let (mut out_read, mut out_write): (HANDLE, HANDLE) = (null_mut(), null_mut());
        if CreatePipe(&mut in_read, &mut in_write, null(), 0) == 0 {
            return Err(os_err("CreatePipe (input)"));
        }
        if CreatePipe(&mut out_read, &mut out_write, null(), 0) == 0 {
            CloseHandle(in_read);
            CloseHandle(in_write);
            return Err(os_err("CreatePipe (output)"));
        }

        let mut hpc: HPCON = 0;
        let hr = (api.create)(coord(cols, rows), in_read, out_write, flags, &mut hpc);
        // ConPTY keeps its own copies of these two ends.
        CloseHandle(in_read);
        CloseHandle(out_write);
        if hr != 0 {
            CloseHandle(in_write);
            CloseHandle(out_read);
            return Err(format!("CreatePseudoConsole failed: HRESULT {hr:#010x}"));
        }
        let fail = |msg: String| {
            (api.close)(hpc);
            CloseHandle(in_write);
            CloseHandle(out_read);
            Err(msg)
        };

        let mut attr_size = 0usize;
        // This first call fails on purpose. It only reports the size to allocate.
        InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut attr_size);
        let mut attr_buf = vec![0u8; attr_size];
        let attrs = attr_buf.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
        if InitializeProcThreadAttributeList(attrs, 1, 0, &mut attr_size) == 0 {
            return fail(os_err("InitializeProcThreadAttributeList"));
        }
        // The attribute value is the HPCON itself, not a pointer to it.
        if UpdateProcThreadAttribute(
            attrs,
            0,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
            hpc as *const core::ffi::c_void,
            std::mem::size_of::<HPCON>(),
            null_mut(),
            null(),
        ) == 0
        {
            DeleteProcThreadAttributeList(attrs);
            return fail(os_err("UpdateProcThreadAttribute"));
        }

        let mut si: STARTUPINFOEXW = std::mem::zeroed();
        si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        // Null std handles with this flag, so the child cannot inherit ours.
        si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        si.lpAttributeList = attrs;
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
        let mut cmd: Vec<u16> = OsStr::new(cmdline).encode_wide().chain(Some(0)).collect();
        let dir = wide_dir(cwd);
        let block = env_block(env);
        let creation = if block.is_some() { EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT } else { EXTENDED_STARTUPINFO_PRESENT };
        let ok = CreateProcessW(
            null(),
            cmd.as_mut_ptr(),
            null(),
            null(),
            0,
            creation,
            block.as_ref().map_or(null(), |b| b.as_ptr() as *const core::ffi::c_void),
            dir.as_ref().map_or(null(), |d| d.as_ptr()),
            &si.StartupInfo,
            &mut pi,
        );
        DeleteProcThreadAttributeList(attrs);
        if ok == 0 {
            return fail(os_err(&format!("CreateProcessW ({cmdline})")));
        }
        CloseHandle(pi.hThread);
        Ok((hpc, in_write, out_read, pi.hProcess))
    }
}

/// The working folder CreateProcessW opens for the child: judged like any probe. One the judge
/// refuses (a share, a mapped or SUBST drive onto one, a link to one) is replaced by the home
/// folder when that passes, else the child inherits Termurion's own.
fn working_folder(cwd: Option<&std::path::Path>) -> Option<crate::localfs::LocalPath> {
    let cwd = cwd?;
    crate::localfs::LocalPath::judge(cwd).or_else(|| std::env::var_os("USERPROFILE").and_then(crate::localfs::LocalPath::judge))
}

/// A judged working folder as a null-terminated wide string for CreateProcessW.
fn wide_dir(cwd: Option<&std::path::Path>) -> Option<Vec<u16>> {
    working_folder(cwd).map(|d| d.path().as_os_str().encode_wide().chain(Some(0)).collect())
}

/// The null environment (inherit ours) unless there is something to add. Never logged: it can
/// hold the session's API cookie.
fn env_block(env: &[String]) -> Option<Vec<u16>> {
    (!env.is_empty()).then(|| block_over(std::env::vars_os().map(|(k, v)| (k.to_string_lossy().into_owned(), v.to_string_lossy().into_owned())).collect(), env))
}

/// The block a child gets: `env` merged over the inherited `ours`.
fn block_over(ours: Vec<(String, String)>, env: &[String]) -> Vec<u16> {
    crate::session_env::block(&crate::session_env::merge(ours, env))
}

/// Starts the child on two anonymous pipes with no console (`Transport::Pipes`): its stdin is
/// the input pipe, its stdout and stderr the output pipe. Only those two ends are inherited
/// (a handle list), and they are inheritable only until the child has them. Returns (our input
/// write end, our output read end, child process).
unsafe fn start_pipes(cmdline: &str, cwd: Option<&std::path::Path>, env: &[String]) -> Result<(HANDLE, HANDLE, HANDLE), String> {
    unsafe {
        let (mut in_read, mut in_write): (HANDLE, HANDLE) = (null_mut(), null_mut());
        let (mut out_read, mut out_write): (HANDLE, HANDLE) = (null_mut(), null_mut());
        if CreatePipe(&mut in_read, &mut in_write, null(), 0) == 0 {
            return Err(os_err("CreatePipe (input)"));
        }
        if CreatePipe(&mut out_read, &mut out_write, null(), 0) == 0 {
            CloseHandle(in_read);
            CloseHandle(in_write);
            return Err(os_err("CreatePipe (output)"));
        }
        // The child's ends are closed here whatever happens; ours only on a failure.
        let fail = |msg: String| {
            for h in [in_read, in_write, out_read, out_write] {
                CloseHandle(h);
            }
            Err(msg)
        };
        let theirs = [in_read, out_write];
        for h in theirs {
            if SetHandleInformation(h, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) == 0 {
                return fail(os_err("SetHandleInformation"));
            }
        }

        let mut attr_size = 0usize;
        // This first call fails on purpose. It only reports the size to allocate.
        InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut attr_size);
        let mut attr_buf = vec![0u8; attr_size];
        let attrs = attr_buf.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
        if InitializeProcThreadAttributeList(attrs, 1, 0, &mut attr_size) == 0 {
            return fail(os_err("InitializeProcThreadAttributeList"));
        }
        // The value is the array of handles; it must outlive the attribute list.
        if UpdateProcThreadAttribute(
            attrs,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            theirs.as_ptr() as *const core::ffi::c_void,
            std::mem::size_of_val(&theirs),
            null_mut(),
            null(),
        ) == 0
        {
            DeleteProcThreadAttributeList(attrs);
            return fail(os_err("UpdateProcThreadAttribute"));
        }

        let mut si: STARTUPINFOEXW = std::mem::zeroed();
        si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        si.StartupInfo.hStdInput = in_read;
        si.StartupInfo.hStdOutput = out_write;
        si.StartupInfo.hStdError = out_write;
        si.lpAttributeList = attrs;
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
        let mut cmd: Vec<u16> = OsStr::new(cmdline).encode_wide().chain(Some(0)).collect();
        let dir = wide_dir(cwd);
        let block = env_block(env);
        let mut creation = EXTENDED_STARTUPINFO_PRESENT | DETACHED_PROCESS;
        if block.is_some() {
            creation |= CREATE_UNICODE_ENVIRONMENT;
        }
        let ok = CreateProcessW(
            null(),
            cmd.as_mut_ptr(),
            null(),
            null(),
            1,
            creation,
            block.as_ref().map_or(null(), |b| b.as_ptr() as *const core::ffi::c_void),
            dir.as_ref().map_or(null(), |d| d.as_ptr()),
            &si.StartupInfo,
            &mut pi,
        );
        DeleteProcThreadAttributeList(attrs);
        if ok == 0 {
            return fail(os_err(&format!("CreateProcessW ({cmdline})")));
        }
        CloseHandle(pi.hThread);
        // The child has its own copies now.
        CloseHandle(in_read);
        CloseHandle(out_write);
        Ok((in_write, out_read, pi.hProcess))
    }
}

/// Writes input on its own thread. The thread owns the input handle and closes it
/// when the sender is dropped or a write fails.
pub(crate) fn start_writer(input: HANDLE, pending: Arc<std::sync::atomic::AtomicUsize>, notify: Notify) -> (mpsc::Sender<Vec<u8>>, Arc<std::sync::Mutex<Option<String>>>) {
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let error = Arc::new(std::sync::Mutex::new(None));
    let seen = error.clone();
    let raw = input as usize;
    std::thread::spawn(move || {
        let input = raw as HANDLE;
        'chunks: for chunk in rx {
            let mut done = 0;
            while done < chunk.len() {
                let mut n = 0u32;
                let rest = &chunk[done..];
                let ok = unsafe { WriteFile(input, rest.as_ptr(), rest.len() as u32, &mut n, null_mut()) };
                if ok == 0 {
                    *seen.lock().unwrap() = Some(os_err("WriteFile"));
                    break 'chunks;
                }
                done += n as usize;
            }
            let before = pending.fetch_sub(chunk.len(), std::sync::atomic::Ordering::Relaxed);
            // Room again for a coprocess's output: wake the window to send more.
            let limit = crate::coprocess::WRITE_LIMIT;
            if before >= limit && before - chunk.len() < limit {
                notify();
            }
        }
        unsafe { CloseHandle(input) };
    });
    (tx, error)
}

/// Drains ConPTY output on its own thread. It stops at a full `budget` (so the program waits on
/// the pipe), except while an attach wants this shell's console (or a console no reader owns):
/// ConPTY must never block one then.
fn start_reader(output: HANDLE, pid: u32, notify: Notify, paused: Arc<std::sync::atomic::AtomicBool>, budget: Arc<Budget>) -> (Receiver<Vec<u8>>, Receiver<()>, JoinHandle<()>) {
    // Known before the first read, so an attach to this shell from now on lifts this reader.
    wanted(|w| count(&mut w.readers, pid, true));
    /// Forgets the reader when its thread ends, a panic included.
    struct Registered(u32);
    impl Drop for Registered {
        fn drop(&mut self) {
            wanted(|w| count(&mut w.readers, self.0, false));
        }
    }
    let (tx, rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    // HANDLE is a raw pointer and not Send. The thread owns this handle from here on.
    let raw = output as usize;
    let reader = std::thread::spawn(move || {
        let _registered = Registered(pid);
        let output = raw as HANDLE;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            budget.wait_for_room(Duration::from_millis(10), || console_wanted(pid));
            let mut n = 0u32;
            let ok = unsafe { ReadFile(output, buf.as_mut_ptr(), buf.len() as u32, &mut n, null_mut()) };
            // While paused, a chunk already read waits here and nothing more is read.
            while paused.load(std::sync::atomic::Ordering::Acquire) && ok != 0 && n != 0 {
                std::thread::sleep(Duration::from_millis(20));
            }
            if ok == 0 || n == 0 {
                break;
            }
            budget.add(n as usize);
            if tx.send(buf[..n as usize].to_vec()).is_err() {
                break;
            }
            notify();
        }
        unsafe { CloseHandle(output) };
        let _ = done_tx.send(());
        notify();
    });
    (rx, done_rx, reader)
}

impl Pty {
    /// Keeps `what` alive until the pty is dropped.
    pub fn hold(&mut self, what: Box<dyn std::any::Any + Send>) {
        self.held = Some(what);
    }

    /// Ends the child at once. psmux's client stays after its session is gone; the server
    /// (a separate process) is not touched.
    pub fn terminate(&self) {
        unsafe { TerminateProcess(self.process, 1) };
    }

    /// Queues `bytes` for the child. Returns at once. A failed earlier write is reported here.
    pub fn write(&self, bytes: &[u8]) -> Result<(), String> {
        if let Some(e) = self.write_error.lock().unwrap().clone() {
            return Err(e);
        }
        match &self.writer {
            Some(w) => {
                self.pending.fetch_add(bytes.len(), std::sync::atomic::Ordering::Relaxed);
                w.send(bytes.to_vec()).map_err(|_| "the pty writer has stopped".to_string())
            }
            None => Err("the pty is closed".to_string()),
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        check_size(cols, rows)?;
        if self.hpc == 0 {
            return Ok(());
        }
        let hr = unsafe { (self.api.resize)(self.hpc, coord(cols, rows)) };
        if hr != 0 {
            return Err(format!("ResizePseudoConsole failed: HRESULT {hr:#010x}"));
        }
        Ok(())
    }

    /// Clears ConPTY's own buffer, keeping the cursor row, so it matches a cleared screen.
    pub fn clear(&self) -> Result<(), String> {
        if self.hpc == 0 {
            return Ok(());
        }
        let Some(clear) = self.api.clear else {
            return Err("the Windows copy of ConPTY cannot clear its buffer".to_string());
        };
        let hr = unsafe { clear(self.hpc, 1) };
        if hr != 0 {
            return Err(format!("ConptyClearPseudoConsole failed: HRESULT {hr:#010x}"));
        }
        Ok(())
    }

    pub fn try_read(&self) -> Read {
        match self.output.try_recv() {
            Ok(chunk) => {
                self.budget.take(chunk.len());
                Read::Data(chunk)
            }
            Err(TryRecvError::Empty) => Read::Empty,
            Err(TryRecvError::Disconnected) => Read::Closed,
        }
    }

    /// The shell's process id (the root of its job tree).
    pub fn pid(&self) -> u32 {
        unsafe { windows_sys::Win32::System::Threading::GetProcessId(self.process) }
    }

    /// Undo Close: stop reading the program's output (it then waits), or go on.
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, std::sync::atomic::Ordering::Release);
    }

    /// `set_paused(true)` once no console mode read is under way (it holds `CONSOLE` for
    /// writing): a paused console can fill its output pipe, after which OpenConsole blocks
    /// every console call on it, so a read must never start on it or run across the pause.
    /// Readers check `is_paused` under the same lock (`console_mode_unless`).
    pub fn pause_after_reads(&self) {
        let _reads = CONSOLE.read().unwrap_or_else(|e| e.into_inner());
        self.set_paused(true);
    }

    /// Paused for Undo Close.
    pub fn is_paused(&self) -> bool {
        self.paused.load(std::sync::atomic::Ordering::Acquire)
    }

    /// The pause flag, for a reader that checks it under `CONSOLE` (`console_mode_unless`).
    pub fn pause_flag(&self) -> Arc<std::sync::atomic::AtomicBool> {
        self.paused.clone()
    }

    /// Bytes queued for the child that it has not taken yet.
    pub fn pending(&self) -> usize {
        self.pending.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Output read from the child and not yet taken.
    pub fn queued(&self) -> usize {
        self.budget.queued()
    }

    /// The budget itself, for a console reader that skips a full one.
    pub fn budget(&self) -> Arc<Budget> {
        self.budget.clone()
    }

    /// The child's exit code, or None while it runs.
    pub fn exit_code(&self) -> Option<u32> {
        self.wait_exit(Duration::ZERO)
    }

    /// Returns the next chunk of output, or None if nothing came within `wait`.
    pub fn read_timeout(&self, wait: Duration) -> Option<Vec<u8>> {
        match self.output.recv_timeout(wait) {
            Ok(chunk) => {
                self.budget.take(chunk.len());
                Some(chunk)
            }
            Err(RecvTimeoutError::Timeout) => None,
            // The output pipe closed. Sleep, so callers that poll do not spin.
            Err(RecvTimeoutError::Disconnected) => {
                std::thread::sleep(wait);
                None
            }
        }
    }

    /// Reads until `needle` appears. Ok with everything read on a hit, Err with
    /// everything read on timeout.
    pub fn read_until(&self, needle: &[u8], wait: Duration) -> Result<Vec<u8>, Vec<u8>> {
        crate::platform::pty::read_until_with(|d| self.read_timeout(d), needle, wait)
    }

    /// Waits for the child to exit and returns its exit code.
    pub fn wait_exit(&self, wait: Duration) -> Option<u32> {
        if unsafe { WaitForSingleObject(self.process, wait.as_millis() as u32) } != WAIT_OBJECT_0 {
            return None;
        }
        let mut code = 0u32;
        if unsafe { GetExitCodeProcess(self.process, &mut code) } == 0 {
            return None;
        }
        Some(code)
    }
}

impl Pty {
    /// The Windows copy of ConPTY can ignore the close (milestone 0 results), so the
    /// output pipe never ends. End the child, then cancel the reader's blocked read until
    /// the reader exits. Returns false if it never does.
    fn force_reader_stop(&self, reader: &JoinHandle<()>) -> bool {
        eprintln!("termurion: ConPTY ignored the close; ending the child");
        unsafe { TerminateProcess(self.process, 1) };
        self.cancel_reader(reader)
    }

    /// Cancels the reader's blocked read until the reader exits. False if it never does.
    fn cancel_reader(&self, reader: &JoinHandle<()>) -> bool {
        let thread = reader.as_raw_handle() as HANDLE;
        for _ in 0..50 {
            // A cancel only hits a read in progress, so repeat it until the reader is out.
            unsafe { CancelSynchronousIo(thread) };
            if self.reader_done.recv_timeout(Duration::from_millis(20)).is_ok() {
                return true;
            }
        }
        false
    }
}

impl Pty {
    /// Drop for a pipes-only child: closing its stdin lets it end on its own; after
    /// `CLOSE_WAIT` it is ended. A grandchild may still hold the output pipe, so the reader's
    /// read is cancelled when it does not end.
    fn drop_pipes(&mut self) {
        self.set_paused(false);
        self.writer.take();
        if unsafe { WaitForSingleObject(self.process, CLOSE_WAIT.as_millis() as u32) } != WAIT_OBJECT_0 {
            unsafe { TerminateProcess(self.process, 1) };
        }
        if let Some(reader) = self.reader.take() {
            if self.reader_done.recv_timeout(CLOSE_WAIT).is_err() && !self.cancel_reader(&reader) {
                eprintln!("termurion: the pipes reader did not stop; leaving it running");
                // The process handle stays open for good: its pid must not be reused while the
                // reader still owns it in `CONSOLE_WANTED`.
                return;
            }
            let _ = reader.join();
        }
        unsafe { CloseHandle(self.process) };
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // First: a reader waiting at the budget drains again, so no close blocks on a full pipe.
        self.budget.close();
        if self.hpc == 0 {
            return self.drop_pipes();
        }
        // No attach while the pseudoconsole closes (it would end this process), and the
        // messages below never land in an attached console.
        let _console = CONSOLE.read().unwrap_or_else(|e| e.into_inner());
        // Closing the pseudoconsole ends the child and closes the output pipe. The
        // reader thread keeps draining meanwhile (a pause for Undo Close ends here), so the
        // close cannot block on a full pipe.
        self.set_paused(false);
        unsafe { (self.api.close)(self.hpc) };
        // The writer thread closes the input pipe once its queue ends or a write fails.
        self.writer.take();
        if let Some(reader) = self.reader.take() {
            if self.reader_done.recv_timeout(CLOSE_WAIT).is_err() && !self.force_reader_stop(&reader) {
                eprintln!("termurion: the pty reader did not stop; leaving it running");
                // The process handle stays open for good: its pid must not be reused while the
                // reader still owns it in `CONSOLE_WANTED`.
                return;
            }
            let _ = reader.join();
        }
        unsafe { CloseHandle(self.process) };
    }
}

/// The shipped ConPTY from the repo's vendor folder, for the live tests in other modules.
#[cfg(test)]
pub fn test_api() -> Api {
    super::conpty::load(&super::conpty::Source::Shipped(super::conpty::dev_vendor_dir())).unwrap()
}

#[cfg(test)]
pub(crate) mod tests {
    use crate::test_tmp::TempDir;

    /// 13y1 Task 6 item 1 round 3: a git launch's removals reach the child. The block is made the way a
    /// pane's is: the session's variables, the POSIX ones and TERM, the launch's entries applied last
    /// (`session_env::launch_last`), merged over an inherited environment that holds every variable a
    /// launch removes. None of them is in the final block; the launch's own values are.
    #[test]
    fn a_launch_env_survives_into_the_block() {
        let removed = [
            "GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR", "GIT_OBJECT_DIRECTORY", "GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_EXTERNAL_DIFF", "GIT_ASKPASS", "SSH_ASKPASS",
            "GIT_CONFIG_PARAMETERS", "GIT_CONFIG_COUNT", "GIT_EXEC_PATH", "GIT_SSH", "GIT_SSH_COMMAND", "GIT_PROXY_COMMAND",
        ];
        let mut inherited: Vec<(String, String)> = removed.iter().enumerate().map(|(i, n)| (if i % 2 == 0 { n.to_string() } else { n.to_lowercase() }, "C:\\evil\\calc.exe".to_string())).collect();
        inherited.push(("Path".into(), "C:\\W".into()));
        inherited.push(("GIT_PAGER".into(), "calc".into()));
        let features = "F";
        let facts = crate::session_env::Facts { window: 0, tab: 0, pane: 0, id: "1", profile_name: "P", cookie: None, term_features: features, lc_terminal: true, path: None, ssh_path: None, ssh_transport: None };
        let mut session = crate::session_env::vars(&facts);
        session.extend(crate::session_env::posix_vars(&crate::session_env::Posix { colorfgbg: Some("15;0".into()), ..Default::default() }));
        session.push("TERM=xterm-256color".into());
        let mut launch = crate::git_state::launch_env();
        launch.insert(0, "ITERM_WORKGROUP_ID=wg-1".into());
        let env = crate::session_env::launch_last(session, &launch);
        let block = String::from_utf16(&super::block_over(inherited, &env)).unwrap();
        let names: Vec<(String, String)> = block.split('\0').filter(|e| !e.is_empty()).filter_map(|e| e.split_once('=')).map(|(n, v)| (n.to_uppercase(), v.to_string())).collect();
        for gone in removed {
            assert!(!names.iter().any(|(n, _)| n == gone), "{gone} reached the child: {names:?}");
        }
        for (n, v) in [("GIT_PAGER", "cat"), ("PAGER", "cat"), ("GIT_CONFIG_NOSYSTEM", "1"), ("GIT_TERMINAL_PROMPT", "0"), ("ITERM_WORKGROUP_ID", "wg-1"), ("PATH", "C:\\W"), ("TERM", "xterm-256color")] {
            assert!(names.iter().any(|(k, x)| k == n && x == v), "{n}={v} in {names:?}");
        }
    }

    /// Round 3: CreateProcessW opens the working folder, so a share or a path the judge refuses
    /// never becomes one (the home folder takes its place when it passes).
    #[test]
    fn a_refused_working_folder_is_never_handed_to_create_process() {
        for d in [r"\\attacker\share", r"\\?\UNC\h\s\x", "relative"] {
            let got = super::working_folder(Some(std::path::Path::new(d)));
            assert!(got.as_ref().is_none_or(|g| !g.path().to_string_lossy().eq_ignore_ascii_case(d)), "{d}: {got:?}");
        }
        let _td = TempDir::new("t");
        let temp = _td.to_path_buf();
        assert_eq!(super::working_folder(Some(&temp)).map(|g| g.into_path_buf()), Some(temp));
        assert_eq!(super::working_folder(None), None);
    }

    use super::*;
    use crate::platform::windows::conpty::{self, Source, dev_vendor_dir};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn apis() -> Vec<Api> {
        vec![
            conpty::load(&Source::Shipped(dev_vendor_dir())).unwrap(),
            conpty::load(&Source::Inbox).unwrap(),
        ]
    }

    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    /// The child half of `a_close_never_happens_while_attached`: started detached (no console
    /// of its own, as the GUI and termurion-host run), it reads a shell's console mode in a loop
    /// on one thread while the pty closes on another. A close that landed inside an attach
    /// would end this process.
    #[test]
    #[ignore]
    fn close_while_reading_child() {
        if std::env::var_os("TERMURION_CLOSE_CHILD").is_none() {
            return;
        }
        let api = conpty::load(&Source::Shipped(dev_vendor_dir())).unwrap();
        for _ in 0..4 {
            let pty = spawn(&api, "cmd.exe /d /k", 80, 24).unwrap();
            let pid = pty.pid();
            let start = Instant::now();
            // The shell sets its own input mode as it starts: before that the console is readable
            // but not yet in the mode of a shell at its prompt.
            while !console_mode(pid).is_ok_and(|m| m & 0x4 != 0) && start.elapsed() < secs(5) {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(console_mode(pid).is_ok_and(|m| m & 0x4 != 0), "a shell at its prompt echoes");
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let s2 = stop.clone();
            let reader = std::thread::spawn(move || {
                while !s2.load(Ordering::SeqCst) {
                    let _ = console_mode(pid);
                    // Room for the close to take the lock (the watcher reads twice a second).
                    std::thread::sleep(Duration::from_millis(1));
                }
            });
            std::thread::sleep(Duration::from_millis(30));
            drop(pty);
            stop.store(true, Ordering::SeqCst);
            reader.join().unwrap();
        }
    }

    /// The PowerShell reader the m10b checklist names: line input on, echo off, then one line.
    const LINE_NO_ECHO_READER: &str = "$k = Add-Type -Name K -Namespace T -PassThru -MemberDefinition '[DllImport(\"kernel32.dll\")] public static extern IntPtr GetStdHandle(int n); [DllImport(\"kernel32.dll\")] public static extern bool SetConsoleMode(IntPtr h, uint m);'; [void]$k::SetConsoleMode($k::GetStdHandle(-10), 3); Write-Host -NoNewline 'Password: '; $null = [Console]::In.ReadLine()";

    /// Base64 of `text` as UTF-16LE, for `powershell -EncodedCommand`.
    pub(crate) fn encoded_command(text: &str) -> String {
        const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let bytes: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut out = String::new();
        for c in bytes.chunks(3) {
            let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
            for i in 0..4 {
                out.push(if i <= c.len() { ABC[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
            }
        }
        out
    }

    /// The child half of `measure_password_prompt_modes`: each reader waits at its password
    /// prompt in a ConPTY, its console input mode is read three times, and the reader is
    /// ended. Nothing is typed. Started detached (no console of its own) so it can attach.
    #[test]
    #[ignore]
    fn measure_password_prompt_modes_child() {
        if std::env::var_os("TERMURION_MEASURE_CHILD").is_none() {
            return;
        }
        let api = conpty::load(&Source::Shipped(dev_vendor_dir())).unwrap();
        let reader = format!("powershell.exe -NoProfile -NoLogo -EncodedCommand {}", encoded_command(LINE_NO_ECHO_READER));
        let cases = [
            ("ssh", "ssh.exe -o PreferredAuthentications=password -o PubkeyAuthentication=no -o StrictHostKeyChecking=no -o UserKnownHostsFile=NUL nobody@localhost".to_string()),
            ("runas", "runas.exe /user:nobody cmd".to_string()),
            ("powershell line-no-echo reader", reader),
        ];
        for (label, cmdline) in cases {
            let pty = spawn(&api, &cmdline, 120, 30).unwrap();
            let seen = pty.read_until(b"assword", secs(30));
            let prompt = seen.is_ok();
            // Room for the reader to reach its read after printing the prompt.
            std::thread::sleep(Duration::from_millis(1500));
            let modes: Vec<String> = (0..3)
                .map(|_| {
                    let m = console_mode(pty.pid());
                    std::thread::sleep(Duration::from_millis(300));
                    match m {
                        Ok(m) => format!("{m:#06x} (line {}, echo {}, processed {}, at_prompt {})", m & 0x2 != 0, m & 0x4 != 0, m & 0x1 != 0, crate::pwm_prompt::at_prompt(m)),
                        Err(e) => format!("unreadable: {e}"),
                    }
                })
                .collect();
            unsafe { TerminateProcess(pty.process, 1) };
            let shown = if prompt { String::new() } else { format!(" output {:?}", text(&seen.unwrap_err()).chars().rev().take(200).collect::<String>().chars().rev().collect::<String>()) };
            println!("MEASURED {label}: prompt seen {prompt}; modes {modes:?}{shown}");
            drop(pty);
        }
    }

    /// Final review I3 of m10b: the console input modes of ssh's and runas's password prompts
    /// (and the checklist's PowerShell reader). Run by hand:
    /// `cargo test --lib pty::tests::measure_password_prompt_modes -- --ignored --exact --nocapture`.
    #[test]
    #[ignore]
    fn measure_password_prompt_modes() {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "pty::tests::measure_password_prompt_modes_child", "--ignored", "--nocapture", "--test-threads=1"])
            .env("TERMURION_MEASURE_CHILD", "1")
            .creation_flags(DETACHED_PROCESS)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut out = child.stdout.take().unwrap();
        let reading = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = std::io::Read::read_to_string(&mut out, &mut s);
            s
        });
        let end = Instant::now() + secs(150);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break Some(status);
            }
            if Instant::now() >= end {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let got = reading.join().unwrap();
        for line in got.lines().filter(|l| l.contains("MEASURED")) {
            println!("{line}");
        }
        assert!(status.is_some_and(|s| s.success()), "the measuring process failed or hung: {status:?}\n{got}");
    }

    #[test]
    fn our_own_ctrl_event_is_seen_only_while_attached() {
        assert_eq!(ctrl_answer(CTRL_BREAK_EVENT, true), (true, true));
        assert_eq!(ctrl_answer(CTRL_C_EVENT, true), (true, true));
        assert_eq!(ctrl_answer(CTRL_BREAK_EVENT, false), (true, false), "handled, but not counted after the detach");
        assert_eq!(ctrl_answer(CTRL_CLOSE_EVENT, true), (true, false));
        assert_eq!(ctrl_answer(CTRL_CLOSE_EVENT, false), (false, false));
    }

    #[test]
    fn encoded_command_is_utf16_base64() {
        assert_eq!(encoded_command("dir"), "ZABpAHIA");
        assert_eq!(encoded_command("ab"), "YQBiAA==");
    }

    #[test]
    fn a_close_never_happens_while_attached() {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        // What the child prints when it panics, for the failure message.
        let _td = TempDir::new("close-child-log");
        let log = _td.to_path_buf();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "pty::tests::close_while_reading_child", "--ignored", "--test-threads=1", "--nocapture"])
            .env("TERMURION_CLOSE_CHILD", "1")
            .creation_flags(DETACHED_PROCESS)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::fs::File::create(&log).unwrap())
            .spawn()
            .unwrap();
        // A hang fails the suite instead of holding it.
        let end = Instant::now() + secs(60);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= end {
                let _ = child.kill();
                let _ = child.wait();
                panic!("the reading process did not finish within 60 s");
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let said = std::fs::read_to_string(&log).unwrap_or_default();
        assert!(status.success(), "the reading process survived every close: {status:?}: {said}");
    }

    #[test]
    fn runs_a_command_and_reads_output() {
        for api in apis() {
            let p = spawn(&api, "cmd.exe /c echo termurion-ok", 80, 24).unwrap();
            p.read_until(b"termurion-ok", secs(10))
                .unwrap_or_else(|got| panic!("{}: got {:?}", api.source.label(), text(&got)));
            assert_eq!(p.wait_exit(secs(10)), Some(0), "{}", api.source.label());
        }
    }

    #[test]
    fn reports_exit_code() {
        for api in apis() {
            let p = spawn(&api, "cmd.exe /c exit 7", 80, 24).unwrap();
            assert_eq!(p.wait_exit(secs(10)), Some(7), "{}", api.source.label());
        }
    }

    #[test]
    fn input_reaches_the_child() {
        for api in apis() {
            let p = spawn(&api, "cmd.exe /q /k", 80, 24).unwrap();
            // cmd prints abc123 only when it runs the echo. The typed line shows abc^123.
            p.write(b"echo abc^123\r").unwrap();
            p.read_until(b"abc123", secs(10))
                .unwrap_or_else(|got| panic!("{}: got {:?}", api.source.label(), text(&got)));
            p.write(b"exit 3\r").unwrap();
            assert_eq!(p.wait_exit(secs(10)), Some(3), "{}", api.source.label());
        }
    }

    #[test]
    fn resize_succeeds() {
        // Shipped ConPTY only. The Windows copy loses the close if a resize arrives while it
        // is still starting (about 1 run in 8), and the drop then waits forever. Milestone 0
        // measured this; see docs/probe.
        let api = conpty::load(&Source::Shipped(dev_vendor_dir())).unwrap();
        let p = spawn(&api, "cmd.exe /q /k", 80, 24).unwrap();
        p.resize(120, 40).unwrap();
    }

    #[test]
    fn reader_survives_a_zero_byte_write() {
        let (mut r, mut w): (HANDLE, HANDLE) = (null_mut(), null_mut());
        assert_ne!(unsafe { CreatePipe(&mut r, &mut w, null(), 0) }, 0);
        let (rx, _done, reader) = start_reader(r, 0, Arc::new(|| {}), Arc::new(std::sync::atomic::AtomicBool::new(false)), Budget::new());
        let mut n = 0u32;
        unsafe {
            WriteFile(w, b"".as_ptr(), 0, &mut n, null_mut());
            WriteFile(w, b"x".as_ptr(), 1, &mut n, null_mut());
            CloseHandle(w);
        }
        reader.join().unwrap();
        let got: Vec<u8> = rx.try_iter().flatten().collect();
        assert_eq!(got, b"x");
    }

    /// A reader is known by its shell's pid while it runs and forgotten when its thread ends
    /// (by a guard, so a panic forgets it too).
    #[test]
    fn a_reader_is_forgotten_when_it_ends() {
        let pid = 0xFFFF_FF01;
        let (mut r, mut w): (HANDLE, HANDLE) = (null_mut(), null_mut());
        assert_ne!(unsafe { CreatePipe(&mut r, &mut w, null(), 0) }, 0);
        let (_rx, _done, reader) = start_reader(r, pid, Arc::new(|| {}), Arc::new(std::sync::atomic::AtomicBool::new(false)), Budget::new());
        assert!(wanted(|w| w.readers.contains_key(&pid)));
        unsafe { CloseHandle(w) };
        reader.join().unwrap();
        assert!(!wanted(|w| w.readers.contains_key(&pid)), "an ended reader still owns its pid");
    }

    #[test]
    fn pty_can_move_to_another_thread() {
        // Milestone 1 closes panes off the UI thread.
        fn assert_send<T: Send>() {}
        assert_send::<Pty>();
    }

    #[test]
    fn inbox_drop_after_early_resize_is_bounded() {
        // The Windows copy loses the close if a resize arrives while it starts (about 1 run
        // in 8, milestone 0 results). Drop must still return.
        let api = conpty::load(&Source::Inbox).unwrap();
        for run in 0..20 {
            let p = spawn(&api, "cmd.exe /q /k", 80, 24).unwrap();
            p.resize(120, 40).unwrap();
            let start = Instant::now();
            drop(p);
            assert!(start.elapsed() < secs(5), "run {run}: drop took {:?}", start.elapsed());
        }
    }

    fn shipped() -> Api {
        conpty::load(&Source::Shipped(dev_vendor_dir())).unwrap()
    }

    #[test]
    fn notify_fires_on_output_and_exit() {
        let count = Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        let p = spawn_notify(&shipped(), "cmd.exe /c echo notify-me", None, &[], 80, 24, Arc::new(move || {
            c.fetch_add(1, Ordering::SeqCst);
        }))
        .unwrap();
        assert_eq!(p.wait_exit(secs(10)), Some(0));
        let end = Instant::now() + secs(5);
        while count.load(Ordering::SeqCst) < 2 && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(count.load(Ordering::SeqCst) >= 2, "notify ran {} times", count.load(Ordering::SeqCst));
    }

    #[test]
    fn try_read_returns_output_without_blocking() {
        let p = spawn(&shipped(), "cmd.exe /q /k echo try-read", 80, 24).unwrap();
        let mut all = Vec::new();
        let end = Instant::now() + secs(10);
        while !text(&all).contains("try-read") && Instant::now() < end {
            match p.try_read() {
                Read::Data(d) => all.extend(d),
                Read::Empty => std::thread::sleep(Duration::from_millis(20)),
                Read::Closed => panic!("output closed early"),
            }
        }
        assert!(text(&all).contains("try-read"), "{:?}", text(&all));
    }

    const FLOOD: &str = r#"cmd.exe /d /c "for /l %i in (0,0,1) do @echo xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx""#;

    /// A full budget, `p` never read: waits until the reader stops at it.
    fn wait_full(p: &Pty) {
        let end = Instant::now() + secs(10);
        while p.queued() < crate::pty_budget::PTY_BUDGET {
            assert!(Instant::now() < end, "the budget never filled: {}", p.queued());
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The CPU time `process` has used, for telling a blocked writer from a running one.
    fn cpu(process: HANDLE) -> Duration {
        use windows_sys::Win32::Foundation::FILETIME;
        let zero = || FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut a, mut b, mut k, mut u) = (zero(), zero(), zero(), zero());
        assert_ne!(unsafe { windows_sys::Win32::System::Threading::GetProcessTimes(process, &mut a, &mut b, &mut k, &mut u) }, 0);
        let t = |f: FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) * 100;
        Duration::from_nanos(t(k) + t(u))
    }

    #[test]
    fn a_flood_waits_at_the_budget() {
        let p = spawn(&shipped(), FLOOD, 80, 24).unwrap();
        wait_full(&p);
        std::thread::sleep(Duration::from_millis(500));
        // The flood itself stops: were OpenConsole to buffer its output instead, cmd would
        // keep running and the memory would only move there.
        let before = cpu(p.process);
        std::thread::sleep(secs(1));
        let used = cpu(p.process) - before;
        assert!(used < Duration::from_millis(100), "the flood ran on while held: {used:?} of CPU in 1 s");
        assert!(p.queued() <= crate::pty_budget::PTY_BUDGET + 64 * 1024, "the reader read past the budget: {}", p.queued());
        while let Read::Data(_) = p.try_read() {}
        let end = Instant::now() + secs(5);
        while p.queued() < 256 * 1024 {
            assert!(Instant::now() < end, "the reader did not read again after the drain: {}", p.queued());
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_dropped_pty_with_a_full_budget_hangs_up() {
        let p = spawn(&shipped(), FLOOD, 80, 24).unwrap();
        wait_full(&p);
        let start = Instant::now();
        drop(p);
        // Past CLOSE_WAIT Drop gives up on the reader (force_reader_stop): inside it, the
        // reader ended and said so on `reader_done`.
        assert!(start.elapsed() < CLOSE_WAIT, "the reader did not end on the close: {:?}", start.elapsed());
    }

    /// Runs `attach_child` with `which` in a copy of this test binary that has no console. An
    /// attach from a process with one fails at once (AttachConsole: access denied), which would
    /// let these tests pass without ever attaching.
    fn in_detached_child(which: &str) {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "platform::windows::pty::tests::attach_child", "--ignored", "--test-threads=1", "--nocapture"])
            .env("TERMURION_ATTACH_CHILD", which)
            .creation_flags(DETACHED_PROCESS)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let (mut out, mut err) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
        let reading = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = std::io::Read::read_to_string(&mut out, &mut s);
            let _ = std::io::Read::read_to_string(&mut err, &mut s);
            s
        });
        let end = Instant::now() + secs(60);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break Some(status);
            }
            if Instant::now() >= end {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let got = reading.join().unwrap();
        assert!(status.is_some_and(|s| s.success()) && got.contains("1 passed"), "{which}: {status:?}\n{got}");
    }

    /// The child half of the attach tests below: a flood on `which`'s ConPTY fills its budget,
    /// then an attach (a mode read, or a write that makes output) must be done within 2 s.
    #[test]
    #[ignore]
    fn attach_child() {
        let Ok(which) = std::env::var("TERMURION_ATTACH_CHILD") else { return };
        if which == "neighbor" {
            return neighbor_reads_leave_a_full_budget();
        }
        let api = if which.contains("inbox") { conpty::load(&Source::Inbox).unwrap() } else { shipped() };
        let p = spawn(&api, FLOOD, 80, 24).unwrap();
        // ConPTY floods slowly: wait for the full budget, then 1 s more for the pipe to fill.
        wait_full(&p);
        std::thread::sleep(secs(1));
        let pid = p.pid();
        let write = which.contains("write");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let no = std::sync::atomic::AtomicBool::new(false);
            let r = if write { attached_unless(pid, &no, write_conout).map(|_| 0) } else { console_mode_unless(pid, &no) };
            let _ = tx.send(r);
        });
        let got = rx.recv_timeout(secs(2)).expect("the attach waited on the full pipe");
        assert!(got.is_ok(), "the attach failed: {got:?}");
    }

    /// The child half of `reading_one_console_never_lifts_another`: a full flood stays at its
    /// budget while an idle neighbor is attached every 50 ms for 3 s, each attach reading its
    /// mode and staying 100 ms (as Send Signal stays for its own copy of the event).
    fn neighbor_reads_leave_a_full_budget() {
        let (full, idle) = (spawn(&shipped(), FLOOD, 80, 24).unwrap(), spawn(&shipped(), "cmd.exe /d /k", 80, 24).unwrap());
        wait_full(&full);
        std::thread::sleep(secs(1));
        let no = std::sync::atomic::AtomicBool::new(false);
        let end = Instant::now() + secs(3);
        while Instant::now() < end {
            assert!(console_mode_unless(idle.pid(), &no).is_ok(), "the neighbor's mode was not read");
            assert_eq!(attached_unless(idle.pid(), &no, || Ok(std::thread::sleep(Duration::from_millis(100)))), Ok(()));
            std::thread::sleep(Duration::from_millis(50));
        }
        let limit = crate::pty_budget::PTY_BUDGET + 64 * 1024;
        assert!(full.queued() <= limit, "reading the neighbor let the full one grow: {} over {limit}", full.queued());
    }

    /// Writes 64 KiB of text to the attached console.
    fn write_conout() -> Result<(), String> {
        let name: Vec<u16> = "CONOUT$".encode_utf16().chain(Some(0)).collect();
        let h = unsafe { CreateFileW(name.as_ptr(), GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, null(), OPEN_EXISTING, 0, null_mut()) };
        if h == INVALID_HANDLE_VALUE {
            return Err(os_err("CreateFileW(CONOUT$)"));
        }
        let text: Vec<u16> = std::iter::repeat_n(b'x' as u16, 32 * 1024).collect();
        let mut n = 0u32;
        let ok = unsafe { windows_sys::Win32::System::Console::WriteConsoleW(h, text.as_ptr().cast(), text.len() as u32, &mut n, null_mut()) } != 0;
        let err = (!ok).then(|| os_err("WriteConsoleW"));
        unsafe { CloseHandle(h) };
        err.map_or(Ok(()), Err)
    }

    /// Only the console being attached reads past its budget: sampling an idle pane must not
    /// grow a held flood.
    #[test]
    fn reading_one_console_never_lifts_another() {
        in_detached_child("neighbor");
    }

    #[test]
    fn a_console_read_never_waits_on_a_full_budget() {
        in_detached_child("read");
    }

    /// The older inbox conhost holds the console lock while it paints into a full pipe.
    #[test]
    fn an_inbox_console_read_never_waits_on_a_full_budget() {
        in_detached_child("read-inbox");
    }

    /// An attach that writes (text, or a resize as `ssh_probe_win` does) makes output, which
    /// OpenConsole cannot put in a full pipe.
    #[test]
    fn a_writing_attach_never_waits_on_a_full_budget() {
        in_detached_child("write");
    }

    #[test]
    fn ctrl_c_ends_a_flood_while_it_is_read() {
        // A test run can ignore Ctrl+C, and every child inherits that (cmd then never stops), so
        // the flood takes Ctrl+C back first, as signal_child does.
        let script = "$k = Add-Type -Name C -Namespace T -PassThru -MemberDefinition '[DllImport(\"kernel32.dll\")] public static extern bool SetConsoleCtrlHandler(IntPtr h, bool add);'; [void]$k::SetConsoleCtrlHandler([IntPtr]::Zero, $false); $s = 'x' * 4000; while ($true) { [Console]::Out.Write($s) }";
        let p = spawn(&shipped(), &format!("powershell.exe -NoProfile -EncodedCommand {}", encoded_command(script)), 80, 24).unwrap();
        let mut seen = 0;
        let end = Instant::now() + secs(20);
        while seen < 64 * 1024 {
            assert!(Instant::now() < end, "the flood never started");
            if let Some(d) = p.read_timeout(Duration::from_millis(100)) {
                seen += d.len();
            }
        }
        p.write(b"\x03").unwrap();
        let end = Instant::now() + secs(5);
        while p.exit_code().is_none() {
            assert!(Instant::now() < end, "Ctrl+C did not end the flood within 5 s");
            while let Read::Data(_) = p.try_read() {}
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn exit_code_is_none_while_running() {
        let p = spawn(&shipped(), "cmd.exe /q /k", 80, 24).unwrap();
        assert_eq!(p.exit_code(), None);
    }

    #[test]
    fn zero_size_is_an_error() {
        let err = spawn(&shipped(), "cmd.exe /q /k", 0, 24).err().unwrap();
        assert!(err.contains("size"), "{err}");
        let p = spawn(&shipped(), "cmd.exe /q /k", 80, 24).unwrap();
        assert!(p.resize(80, 0).unwrap_err().contains("size"));
    }

    #[test]
    fn write_does_not_block_when_the_child_is_not_reading() {
        // ping never reads its input, so a large write fills the pipe.
        let p = spawn(&shipped(), "ping -n 8 127.0.0.1", 80, 24).unwrap();
        let t = Instant::now();
        p.write(&vec![b'a'; 32 << 20]).unwrap();
        assert!(t.elapsed() < secs(1), "write blocked for {:?}", t.elapsed());
    }

    #[test]
    fn the_child_starts_in_the_given_directory() {
        let _td = TempDir::new("t");
        let dir = _td.to_path_buf();
        let p = spawn_notify(&shipped(), "cmd.exe /c cd", Some(&dir), &[], 200, 24, Arc::new(|| {})).unwrap();
        let want = dir.to_string_lossy().trim_end_matches('\\').to_string();
        p.read_until(want.as_bytes(), secs(10)).unwrap_or_else(|got| panic!("{:?}", text(&got)));
    }

    /// The session environment reaches the child over the inherited one (PATH survives the
    /// merge: `where` finds cmd.exe); with nothing extra the child gets the inherited
    /// environment, unchanged. The carets keep the console title (the command line) from
    /// matching what the child prints.
    #[test]
    fn env_reaches_the_child() {
        let cmd = "cmd.exe /c echo [%ITERM_SESSION_ID%^]&& where cmd >nul && echo env^-done";
        let p = spawn_notify(&shipped(), cmd, None, &["ITERM_SESSION_ID=w0t0p0:5".into()], 200, 24, Arc::new(|| {})).unwrap();
        let got = p.read_until(b"env-done", secs(10)).unwrap_or_else(|got| panic!("{:?}", text(&got)));
        assert!(text(&got).contains("[w0t0p0:5]"), "{:?}", text(&got));
        let p = spawn_notify(&shipped(), cmd, None, &[], 200, 24, Arc::new(|| {})).unwrap();
        let got = p.read_until(b"env-done", secs(10)).unwrap_or_else(|got| panic!("{:?}", text(&got)));
        assert!(text(&got).contains("[%ITERM_SESSION_ID%]"), "{:?}", text(&got));

        // The utilities folder on PATH (`utilities::path_with`) and iTerm2's TERM_PROGRAM.
        let _td = TempDir::new("env-path");
        let dir = _td.to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("probe.cmd"), "").unwrap();
        let path = crate::utilities::path_with(&std::env::var("PATH").unwrap_or_default(), &dir.display().to_string());
        let env = [format!("PATH={path}"), format!("TERM_PROGRAM={}", crate::session_env::TERM_PROGRAM)];
        let cmd = "cmd.exe /c where probe && echo [%TERM_PROGRAM%^]&& echo env^-done";
        let p = spawn_notify(&shipped(), cmd, None, &env, 200, 24, Arc::new(|| {})).unwrap();
        let got = p.read_until(b"env-done", secs(10)).unwrap_or_else(|got| panic!("{:?}", text(&got)));
        let got = text(&got);
        assert!(got.to_lowercase().contains(&dir.join("probe.cmd").display().to_string().to_lowercase()), "{got:?}");
        assert!(got.contains("[iTerm.app]"), "{got:?}");
        drop(p);
    }

    #[test]
    fn bad_command_is_an_error() {
        let err = spawn(&apis()[0], "no-such-program-xyz.exe", 80, 24).err().unwrap();
        assert!(err.contains("CreateProcessW"), "{err}");
    }

    #[test]
    fn drop_returns_while_child_runs() {
        for api in apis() {
            let p = spawn(&api, "ping -n 30 127.0.0.1", 80, 24).unwrap();
            // ConPTY can put the title sequence inside "Pinging": wait for a reply line.
            p.read_until(b"Reply from", secs(10)).unwrap_or_else(|got| panic!("{:?}", text(&got)));
            let start = Instant::now();
            drop(p);
            assert!(start.elapsed() < secs(5), "{}: drop took {:?}", api.source.label(), start.elapsed());
        }
    }

    #[test]
    fn drop_returns_after_child_exits() {
        for api in apis() {
            let p = spawn(&api, "cmd.exe /c exit 0", 80, 24).unwrap();
            assert_eq!(p.wait_exit(secs(10)), Some(0));
            let start = Instant::now();
            drop(p);
            assert!(start.elapsed() < secs(5), "{}: drop took {:?}", api.source.label(), start.elapsed());
        }
    }

    /// The flags the last `fake_create` was given.
    static CREATE_FLAGS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    unsafe extern "system" fn fake_create(_: COORD, _: HANDLE, _: HANDLE, flags: u32, _: *mut HPCON) -> windows_sys::core::HRESULT {
        CREATE_FLAGS.store(flags, Ordering::SeqCst);
        0x80004005u32 as i32
    }

    unsafe extern "system" fn fake_resize(_: HPCON, _: COORD) -> windows_sys::core::HRESULT {
        0
    }

    unsafe extern "system" fn fake_close(_: HPCON) {}

    #[test]
    fn flags_reach_create_pseudo_console() {
        use crate::ssh_transport::Transport;
        let fake = Api { source: Source::Inbox, create: fake_create, resize: fake_resize, close: fake_close, clear: None, pack: None };
        let quiet: Notify = Arc::new(|| {});
        let err = spawn_transport(&fake, Transport::Passthrough, "cmd.exe", None, &[], 80, 24, quiet.clone()).err().unwrap();
        assert!(err.contains("0x80004005"), "{err}");
        assert_eq!(CREATE_FLAGS.load(Ordering::SeqCst), 8);
        CREATE_FLAGS.store(u32::MAX, Ordering::SeqCst);
        let err = spawn_transport(&fake, Transport::Conpty, "cmd.exe", None, &[], 80, 24, quiet).err().unwrap();
        assert!(err.contains("0x80004005"), "{err}");
        assert_eq!(CREATE_FLAGS.load(Ordering::SeqCst), 0);
    }

    fn pipes(cmdline: &str) -> Pty {
        spawn_transport(&shipped(), crate::ssh_transport::Transport::Pipes, cmdline, None, &[], 80, 24, Arc::new(|| {})).unwrap()
    }

    #[test]
    fn pipes_pty_runs_a_child() {
        let p = pipes("cmd.exe /d /c echo termurion-pipes");
        p.read_until(b"termurion-pipes", secs(10)).unwrap_or_else(|got| panic!("{:?}", text(&got)));
        assert_eq!(p.wait_exit(secs(10)), Some(0));
        assert_eq!(p.resize(100, 30), Ok(()));
        assert_eq!(p.clear(), Ok(()));
        assert!(p.pid() > 0);
    }

    #[test]
    fn pipes_pty_takes_input() {
        let p = pipes("cmd.exe /d /v:on /c \"set /p x=&echo got!x!\"");
        p.write(b"abc\r\n").unwrap();
        p.read_until(b"gotabc", secs(10)).unwrap_or_else(|got| panic!("{:?}", text(&got)));
    }

    #[test]
    fn pipes_pty_drop_ends_a_child() {
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE};
        let p = pipes("cmd.exe /d /c pause");
        p.read_until(b"Press any key", secs(10)).unwrap_or_else(|got| panic!("{:?}", text(&got)));
        assert_eq!(p.exit_code(), None, "pause waits for its input");
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, p.pid()) };
        assert!(!handle.is_null(), "{}", os_err("OpenProcess"));
        let start = Instant::now();
        drop(p);
        let ended = unsafe { WaitForSingleObject(handle, 3000) } == WAIT_OBJECT_0;
        unsafe { CloseHandle(handle) };
        assert!(ended && start.elapsed() < secs(3), "the child still ran {:?} after the drop", start.elapsed());
    }
}

/// Windows needs no gate: handles are made inheritable per call, never by default.
pub fn spawn_gated<T>(f: impl FnOnce() -> T) -> T {
    f()
}

/// The ConPTY shipped beside the exe, else the copy built into Windows.
pub fn load_for_app(exe_dir: &std::path::Path) -> Result<(super::conpty::Api, Option<String>), String> {
    use super::conpty::{self, Source};
    match conpty::load(&Source::Shipped(exe_dir.to_path_buf())) {
        Ok(api) => Ok((api, None)),
        Err(e) => match conpty::load(&Source::Inbox) {
            Ok(api) => Ok((api, Some(format!("Shipped ConPTY did not load ({e}). Using the Windows copy.")))),
            Err(e2) => Err(format!("No ConPTY: {e}; {e2}")),
        },
    }
}

pub fn shipped_has_pack(exe_dir: &std::path::Path) -> bool {
    use super::conpty::{self, Source};
    conpty::load(&Source::Shipped(exe_dir.to_path_buf())).is_ok_and(|a| a.pack.is_some())
}

pub fn api_has_pack(api: &super::conpty::Api) -> bool {
    api.pack.is_some()
}

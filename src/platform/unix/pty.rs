//! A child process on a Unix pseudo terminal (Linux and macOS), made with `openpty`. Not
//! alacritty's tty: that one sets `ALACRITTY_WINDOW_ID` and `WINDOWID`, wraps macOS shells in
//! `login`, and ties exit detection to its own event loop, none of which fits the `Pty` contract
//! (pause, pending, exit code, `read_until`) the pane and the session host use.
//!
//! One reader thread polls the master and a wake pipe, one writer thread writes a dup of the
//! master, and one waiter thread per child collects its exit. A process wide `SIGCHLD` handler
//! would reap children `std::process::Command` waits on elsewhere (git, curl), so none is used.

use std::collections::HashMap;
use std::ffi::OsString;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::platform::pty::{Notify, Read};
use crate::ssh_transport::Transport;

/// Nothing to load on Unix.
#[derive(Clone, Debug, Default)]
pub struct PtyApi;

/// How long Drop lets a hung-up child end before the group is killed.
const CLOSE_WAIT: Duration = Duration::from_secs(2);

/// Variables of the terminal Termurion was started from. A pane's shell is on a terminal of its
/// own, so none of them may reach it (tmux refuses to nest on `TMUX`, programs draw for the
/// wrong terminal on `TERM`).
const OUTER_TERMINAL_VARS: &[&str] = &[
    "TERM", "TERM_PROGRAM", "TERM_PROGRAM_VERSION", "TERM_SESSION_ID", "TERM_FEATURES", "COLORTERM", "ITERM_SESSION_ID", "ITERM_PROFILE", "LC_TERMINAL", "LC_TERMINAL_VERSION",
    "TMUX", "TMUX_PANE", "STY", "WINDOWID", "KITTY_WINDOW_ID", "KITTY_PID", "KITTY_LISTEN_ON", "ALACRITTY_WINDOW_ID", "ALACRITTY_SOCKET", "ALACRITTY_LOG", "VTE_VERSION", "WT_SESSION",
    "WEZTERM_PANE", "COLUMNS", "LINES", "XDG_ACTIVATION_TOKEN", "DESKTOP_STARTUP_ID",
];

/// Every pty child's master by child pid, from spawn to drop: the foreground group and the input
/// mode are read off it.
static REGISTRY: Mutex<Option<HashMap<u32, Arc<OwnedFd>>>> = Mutex::new(None);

/// Held from creating the descriptors until the child has started, so another spawn of ours never
/// forks inside the gap between `openpty` and the close-on-exec flag.
static SPAWN_LOCK: Mutex<()> = Mutex::new(());

fn registry<T>(f: impl FnOnce(&mut HashMap<u32, Arc<OwnedFd>>) -> T) -> T {
    let mut g = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(HashMap::new))
}

/// The child's exit, set once by its waiter thread.
struct Exit {
    code: Mutex<Option<u32>>,
    done: Condvar,
}

pub struct Pty {
    pid: u32,
    /// A pty child (not a pipes one): registered, and its group is hung up on drop.
    tty: Option<Arc<OwnedFd>>,
    /// Input goes through a writer thread, so a child that is slow to read never blocks the
    /// caller. Dropping the sender ends the thread.
    writer: Option<mpsc::Sender<Vec<u8>>>,
    write_error: Arc<Mutex<Option<String>>>,
    /// Bytes sent to the writer and not yet written.
    pending: Arc<AtomicUsize>,
    /// Undo Close: the reader holds off, so the program waits on the pty.
    paused: Arc<AtomicBool>,
    output: Receiver<Vec<u8>>,
    /// Writing to it wakes the reader out of its poll.
    wake: OwnedFd,
    exit: Arc<Exit>,
    /// Something kept alive as long as the pty.
    held: Option<Box<dyn std::any::Any + Send>>,
}

fn os_err(what: &str) -> String {
    format!("{what} failed: {}", std::io::Error::last_os_error())
}

fn check_size(cols: u16, rows: u16) -> Result<(), String> {
    if cols == 0 || rows == 0 || cols > i16::MAX as u16 || rows > i16::MAX as u16 {
        return Err(format!("bad pty size {cols}x{rows}"));
    }
    Ok(())
}

pub fn spawn(api: &PtyApi, cmdline: &str, cols: u16, rows: u16) -> Result<Pty, String> {
    spawn_notify(api, cmdline, None, &[], cols, rows, Arc::new(|| {}))
}

/// Starts `cmdline` on a new pty, in `cwd` when given. `env`'s `NAME=value` entries go over the
/// inherited environment (less the outer terminal's variables).
pub fn spawn_notify(api: &PtyApi, cmdline: &str, cwd: Option<&std::path::Path>, env: &[String], cols: u16, rows: u16, notify: Notify) -> Result<Pty, String> {
    spawn_transport(api, Transport::Conpty, cmdline, cwd, env, cols, rows, notify)
}

/// Starts `cmdline` on transport `t`: Passthrough and Conpty are the pty, Pipes is two pipes
/// with no tty (resize and clear then do nothing).
#[allow(clippy::too_many_arguments)]
pub fn spawn_transport(_api: &PtyApi, t: Transport, cmdline: &str, cwd: Option<&std::path::Path>, env: &[String], cols: u16, rows: u16, notify: Notify) -> Result<Pty, String> {
    spawn_with_env(inherited_env(), t, cmdline, cwd, env, cols, rows, notify)
}

/// `spawn_transport` over a given inherited environment (a test hands it one).
#[allow(clippy::too_many_arguments)]
fn spawn_with_env(inherited: Vec<(OsString, OsString)>, t: Transport, cmdline: &str, cwd: Option<&std::path::Path>, env: &[String], cols: u16, rows: u16, notify: Notify) -> Result<Pty, String> {
    check_size(cols, rows)?;
    // The error already names the line.
    let words = crate::posix_words::split(cmdline)?;
    let mut cmd = Command::new(&words[0]);
    cmd.args(&words[1..]);
    cmd.env_clear();
    cmd.envs(child_env(inherited, env));
    if let Some(dir) = working_folder(cwd) {
        cmd.current_dir(dir.path());
    }
    let _gate = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (wake_r, wake) = make_pipe()?;
    let (master, input, child) = match t {
        Transport::Passthrough | Transport::Conpty => {
            let (master, slave) = open_pty(cols, rows)?;
            let (a, b) = (clone_fd(&slave)?, clone_fd(&slave)?);
            cmd.stdin(Stdio::from(a)).stdout(Stdio::from(b)).stderr(Stdio::from(slave));
            // A session of its own with the pty as its controlling terminal.
            unsafe {
                cmd.pre_exec(|| {
                    if libc::setsid() < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let master = Arc::new(master);
            let input = clone_fd(&master)?;
            let child = cmd.spawn().map_err(|e| spawn_error(cmdline, cwd, &e))?;
            (Source::Tty(master), input, child)
        }
        Transport::Pipes => {
            let (stdin_r, stdin_w) = make_pipe()?;
            let (out_r, out_w) = make_pipe()?;
            let err_w = clone_fd(&out_w)?;
            cmd.stdin(Stdio::from(stdin_r)).stdout(Stdio::from(out_w)).stderr(Stdio::from(err_w));
            let child = cmd.spawn().map_err(|e| spawn_error(cmdline, cwd, &e))?;
            (Source::Pipe(out_r), stdin_w, child)
        }
    };
    drop(cmd);
    drop(_gate);
    let pid = child.id();
    let paused = Arc::new(AtomicBool::new(false));
    let exit = Arc::new(Exit { code: Mutex::new(None), done: Condvar::new() });
    let (read_fd, tty) = match master {
        Source::Tty(m) => {
            registry(|r| r.insert(pid, m.clone()));
            (m.clone(), Some(m))
        }
        Source::Pipe(fd) => (Arc::new(fd), None),
    };
    let (output, _reader) = start_reader(read_fd, wake_r, notify.clone(), paused.clone());
    let pending = Arc::new(AtomicUsize::new(0));
    let (writer, write_error) = start_writer(input, pending.clone(), notify.clone());
    start_waiter(child, exit.clone(), notify);
    Ok(Pty { pid, tty, writer: Some(writer), write_error, pending, paused, output, wake, exit, held: None })
}

enum Source {
    Tty(Arc<OwnedFd>),
    Pipe(OwnedFd),
}

fn spawn_error(cmdline: &str, cwd: Option<&std::path::Path>, e: &std::io::Error) -> String {
    match cwd {
        Some(dir) => format!("spawn ({cmdline}) in {} failed: {e}", dir.display()),
        None => format!("spawn ({cmdline}) failed: {e}"),
    }
}

/// The working folder the child starts in: judged like any probe. One the judge refuses is
/// replaced by the home folder when that passes, else the child inherits Termurion's own.
fn working_folder(cwd: Option<&std::path::Path>) -> Option<crate::localfs::LocalPath> {
    let cwd = cwd?;
    crate::localfs::LocalPath::judge(cwd).or_else(|| crate::platform::paths::home_dir().and_then(crate::localfs::LocalPath::judge))
}

fn inherited_env() -> Vec<(OsString, OsString)> {
    std::env::vars_os().collect()
}

/// The child's environment: the inherited set less the outer terminal's variables, with the
/// session's `NAME=value` entries merged over it. Never logged: it can hold the session's cookie.
pub(crate) fn child_env(inherited: Vec<(OsString, OsString)>, env: &[String]) -> Vec<(OsString, OsString)> {
    let kept = inherited.into_iter().filter(|(n, _)| !OUTER_TERMINAL_VARS.iter().any(|o| n.as_bytes() == o.as_bytes())).collect();
    merge_env(kept, env)
}

/// `session_env::merge` for Unix: names are case-sensitive (`http_proxy` and `HTTP_PROXY` are two
/// variables) and values stay raw bytes. Same rules otherwise: `NEVER_INHERITED` names are
/// dropped from `base`, an entry holding a NUL is ignored, `-NAME` removes, `NAME=value` sets.
fn merge_env(base: Vec<(OsString, OsString)>, extra: &[String]) -> Vec<(OsString, OsString)> {
    let mut out = base;
    out.retain(|(n, _)| !crate::session_env::NEVER_INHERITED.iter().any(|x| n.as_bytes().eq_ignore_ascii_case(x.as_bytes())));
    for e in extra.iter().filter(|e| !e.contains('\0')) {
        if let Some(gone) = e.strip_prefix('-').filter(|n| !n.is_empty() && !n.contains('=')) {
            out.retain(|(n, _)| n.as_bytes() != gone.as_bytes());
            continue;
        }
        // The name ends at the first `=` after its first character, as `session_env::merge` reads it.
        let Some(at) = e.char_indices().skip(1).find(|(_, c)| *c == '=').map(|(i, _)| i) else { continue };
        let (name, value) = (&e[..at], &e[at + 1..]);
        out.retain(|(n, _)| n.as_bytes() != name.as_bytes());
        out.push((OsString::from(name), OsString::from(value)));
    }
    out
}

/// A pipe, close-on-exec from the start where the OS can (`pipe2`), else flagged at once.
#[cfg(target_os = "linux")]
fn make_pipe() -> Result<(OwnedFd, OwnedFd), String> {
    let mut fds = [0 as RawFd; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
        return Err(os_err("pipe2"));
    }
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

/// macOS has no `pipe2`: flagged right after, under `SPAWN_LOCK` against our own spawns.
#[cfg(not(target_os = "linux"))]
fn make_pipe() -> Result<(OwnedFd, OwnedFd), String> {
    let mut fds = [0 as RawFd; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } < 0 {
        return Err(os_err("pipe"));
    }
    let (r, w) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    cloexec(&r)?;
    cloexec(&w)?;
    Ok((r, w))
}

#[cfg(not(target_os = "linux"))]
fn cloexec(fd: &OwnedFd) -> Result<(), String> {
    if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(os_err("fcntl(FD_CLOEXEC)"));
    }
    Ok(())
}

/// A duplicate that is close-on-exec too.
fn clone_fd(fd: &OwnedFd) -> Result<OwnedFd, String> {
    fd.try_clone().map_err(|e| format!("dup failed: {e}"))
}

/// A pty of this size, the master and our copy of the slave both close-on-exec; the slave
/// reads UTF-8 (`IUTF8`, so erasing a multi-byte character takes the whole character).
fn open_pty(cols: u16, rows: u16) -> Result<(OwnedFd, OwnedFd), String> {
    let (master, slave) = open_pair(cols, rows)?;
    let mut tio: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(slave.as_raw_fd(), &mut tio) } < 0 {
        return Err(os_err("tcgetattr"));
    }
    tio.c_iflag |= libc::IUTF8;
    if unsafe { libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &tio) } < 0 {
        return Err(os_err("tcsetattr"));
    }
    Ok((master, slave))
}

/// Linux: both ends are opened close-on-exec (`O_CLOEXEC`, and `TIOCGPTPEER` for the slave), so
/// no fork on another thread can ever inherit one.
#[cfg(target_os = "linux")]
fn open_pair(cols: u16, rows: u16) -> Result<(OwnedFd, OwnedFd), String> {
    let flags = libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC;
    let m = unsafe { libc::posix_openpt(flags) };
    if m < 0 {
        return Err(os_err("posix_openpt"));
    }
    let master = unsafe { OwnedFd::from_raw_fd(m) };
    if unsafe { libc::grantpt(m) } < 0 || unsafe { libc::unlockpt(m) } < 0 {
        return Err(os_err("grantpt/unlockpt"));
    }
    let s = unsafe { libc::ioctl(m, libc::TIOCGPTPEER as _, flags) };
    if s < 0 {
        return Err(os_err("ioctl(TIOCGPTPEER)"));
    }
    let slave = unsafe { OwnedFd::from_raw_fd(s) };
    let ws = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
    if unsafe { libc::ioctl(m, libc::TIOCSWINSZ as _, &ws) } < 0 {
        return Err(os_err("ioctl(TIOCSWINSZ)"));
    }
    Ok((master, slave))
}

/// macOS: `openpty`, then close-on-exec at once, under `SPAWN_LOCK` against our own spawns.
#[cfg(not(target_os = "linux"))]
fn open_pair(cols: u16, rows: u16) -> Result<(OwnedFd, OwnedFd), String> {
    let (mut m, mut s) = (0 as RawFd, 0 as RawFd);
    let mut ws = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
    // (null_mut and &mut: macOS declares the last two arguments mutable, Linux const.)
    if unsafe { libc::openpty(&mut m, &mut s, std::ptr::null_mut(), std::ptr::null_mut(), &mut ws) } < 0 {
        return Err(os_err("openpty"));
    }
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(m), OwnedFd::from_raw_fd(s)) };
    cloexec(&master)?;
    cloexec(&slave)?;
    Ok((master, slave))
}

/// Writes input on its own thread. The thread owns the descriptor and closes it when the sender
/// is dropped or a write fails.
fn start_writer(fd: OwnedFd, pending: Arc<AtomicUsize>, notify: Notify) -> (mpsc::Sender<Vec<u8>>, Arc<Mutex<Option<String>>>) {
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let error = Arc::new(Mutex::new(None));
    let seen = error.clone();
    std::thread::spawn(move || {
        'chunks: for chunk in rx {
            let mut done = 0;
            while done < chunk.len() {
                let rest = &chunk[done..];
                let n = unsafe { libc::write(fd.as_raw_fd(), rest.as_ptr().cast(), rest.len()) };
                if n < 0 {
                    if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    *seen.lock().unwrap() = Some(os_err("write"));
                    break 'chunks;
                }
                done += n as usize;
            }
            let before = pending.fetch_sub(chunk.len(), Ordering::Relaxed);
            // Room again for a coprocess's output: wake the window to send more.
            let limit = crate::coprocess::WRITE_LIMIT;
            if before >= limit && before - chunk.len() < limit {
                notify();
            }
        }
    });
    (tx, error)
}

/// Polls `fds` for at most `ms` (-1 forever), retrying an interrupted call.
fn poll(fds: &mut [libc::pollfd], ms: i32) -> bool {
    loop {
        let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, ms) };
        if n >= 0 {
            return true;
        }
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return false;
        }
    }
}

/// Drains the child's output on its own thread. While paused it reads nothing, so the child
/// blocks once the pty's buffer is full (as on Windows). EOF, EIO or a write to `wake` ends it.
fn start_reader(fd: Arc<OwnedFd>, wake: OwnedFd, notify: Notify, paused: Arc<AtomicBool>) -> (Receiver<Vec<u8>>, JoinHandle<()>) {
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut buf = vec![0u8; 64 * 1024];
        let pfd = |fd: RawFd| libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        let woken = |ms: i32| {
            let mut w = [pfd(wake.as_raw_fd())];
            poll(&mut w, ms) && w[0].revents != 0
        };
        'read: loop {
            if paused.load(Ordering::Acquire) {
                if woken(20) {
                    break;
                }
                continue;
            }
            let mut fds = [pfd(fd.as_raw_fd()), pfd(wake.as_raw_fd())];
            if !poll(&mut fds, -1) || fds[1].revents != 0 {
                break;
            }
            if fds[0].revents == 0 {
                continue;
            }
            let n = unsafe { libc::read(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            if n <= 0 {
                break;
            }
            // While paused, a chunk already read waits here and nothing more is read.
            while paused.load(Ordering::Acquire) {
                if woken(20) {
                    break 'read;
                }
            }
            if tx.send(buf[..n as usize].to_vec()).is_err() {
                break;
            }
            notify();
        }
        drop(tx);
        notify();
    });
    (rx, reader)
}

/// Waits for the child to end without reaping it, then reaps it under the lock `terminate`
/// and the group kills take, so a signal never lands on a reused pid.
fn start_waiter(mut child: std::process::Child, exit: Arc<Exit>, notify: Notify) {
    let pid = child.id() as libc::pid_t;
    std::thread::spawn(move || {
        loop {
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            let rc = unsafe { libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, libc::WEXITED | libc::WNOWAIT) };
            if rc == 0 || std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                break;
            }
        }
        let mut code = exit.code.lock().unwrap();
        let status = child.wait();
        *code = Some(status.map(|s| s.code().map_or_else(|| 128 + s.signal().unwrap_or(0) as u32, |c| c as u32)).unwrap_or(1));
        drop(code);
        exit.done.notify_all();
        notify();
    });
}

impl Exit {
    /// Sends `sig` to `pid` (or its group) only while the child has not been reaped.
    fn signal(&self, pid: u32, group: bool, sig: libc::c_int) {
        let code = self.code.lock().unwrap();
        if code.is_none() {
            unsafe {
                if group {
                    libc::killpg(pid as libc::pid_t, sig);
                } else {
                    libc::kill(pid as libc::pid_t, sig);
                }
            }
        }
    }
}

/// The foreground process group of the pty whose shell is `pid` (what a job watcher asks).
#[allow(dead_code)] // the job watcher (Task 5) is its caller
pub(crate) fn foreground_group(pid: u32) -> Option<i32> {
    let master = registry(|r| r.get(&pid).cloned())?;
    let g = unsafe { libc::tcgetpgrp(master.as_raw_fd()) };
    (g > 0).then_some(g)
}

/// The input mode of the pty `pid` runs on, as the console-mode bits `pwm_prompt` tests: `ICANON`
/// is `ENABLE_LINE_INPUT`, `ECHO` is `ENABLE_ECHO_INPUT`. Refused while `paused`.
pub fn console_mode_unless(pid: u32, paused: &AtomicBool) -> Result<u32, String> {
    if paused.load(Ordering::Acquire) {
        return Err("the console is paused".to_string());
    }
    let master = registry(|r| r.get(&pid).cloned()).ok_or_else(|| format!("no pty for process {pid}"))?;
    let mut tio: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(master.as_raw_fd(), &mut tio) } < 0 {
        return Err(os_err("tcgetattr"));
    }
    let mut mode = 0;
    if tio.c_lflag & libc::ICANON != 0 {
        mode |= crate::pwm_prompt::ENABLE_LINE_INPUT;
    }
    if tio.c_lflag & libc::ECHO != 0 {
        mode |= crate::pwm_prompt::ENABLE_ECHO_INPUT;
    }
    Ok(mode)
}

/// The shell a new pane starts: `$SHELL`, else the password database's, as a login shell.
pub fn default_shell() -> String {
    let pw = super::paths::passwd_field(|pw| pw.pw_shell).and_then(|s| s.into_string().ok());
    login_shell(std::env::var("SHELL").ok().as_deref(), pw.as_deref())
}

/// The first absolute, executable candidate, else `/bin/sh`, followed by ` -l`. A path with a
/// byte outside `[A-Za-z0-9/._+-]` is single-quoted so the line splits back to one word.
pub(crate) fn login_shell(env_shell: Option<&str>, pw_shell: Option<&str>) -> String {
    let path = [env_shell, pw_shell].into_iter().flatten().find(|c| executable_file(c)).unwrap_or("/bin/sh");
    let plain = path.bytes().all(|b| b.is_ascii_alphanumeric() || b"/._+-".contains(&b));
    if plain { format!("{path} -l") } else { format!("{} -l", crate::codecierge::sh_quote(path)) }
}

fn executable_file(path: &str) -> bool {
    let Ok(c) = std::ffi::CString::new(path) else { return false };
    if !path.starts_with('/') {
        return false;
    }
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    unsafe { libc::stat(c.as_ptr(), &mut st) == 0 && st.st_mode & libc::S_IFMT == libc::S_IFREG && libc::access(c.as_ptr(), libc::X_OK) == 0 }
}

/// Unix hands the child a `TERM` of the profile's choosing, never the one Termurion was started
/// under (`OUTER_TERMINAL_VARS`).
pub fn inherits_term() -> bool {
    false
}

/// Nothing to load on Unix.
pub fn load_for_app(_exe_dir: &std::path::Path) -> Result<(PtyApi, Option<String>), String> {
    Ok((PtyApi, None))
}

pub fn api_has_pack(_api: &PtyApi) -> bool {
    false
}

/// No pack on Unix.
pub fn shipped_has_pack(_exe_dir: &std::path::Path) -> bool {
    false
}

#[cfg(test)]
pub fn test_api() -> PtyApi {
    PtyApi
}

impl Pty {
    /// Keeps `what` alive until the pty is dropped.
    pub fn hold(&mut self, what: Box<dyn std::any::Any + Send>) {
        self.held = Some(what);
    }

    /// Ends the shell at once (SIGKILL to the shell only, as `TerminateProcess` ends only the shell).
    pub fn terminate(&self) {
        self.exit.signal(self.pid, false, libc::SIGKILL);
    }

    /// Queues `bytes` for the child. Returns at once. A failed earlier write is reported here.
    pub fn write(&self, bytes: &[u8]) -> Result<(), String> {
        if let Some(e) = self.write_error.lock().unwrap().clone() {
            return Err(e);
        }
        match &self.writer {
            Some(w) => {
                self.pending.fetch_add(bytes.len(), Ordering::Relaxed);
                w.send(bytes.to_vec()).map_err(|_| "the pty writer has stopped".to_string())
            }
            None => Err("the pty is closed".to_string()),
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        check_size(cols, rows)?;
        let Some(master) = &self.tty else { return Ok(()) };
        let ws = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
        if unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ as _, &ws) } < 0 {
            return Err(os_err("ioctl(TIOCSWINSZ)"));
        }
        Ok(())
    }

    /// A Unix pty keeps no screen of its own, so there is nothing to clear: the one allowed
    /// no-op (Windows clears ConPTY's buffer to match a cleared screen).
    pub fn clear(&self) -> Result<(), String> {
        Ok(())
    }

    pub fn try_read(&self) -> Read {
        match self.output.try_recv() {
            Ok(chunk) => Read::Data(chunk),
            Err(TryRecvError::Empty) => Read::Empty,
            Err(TryRecvError::Disconnected) => Read::Closed,
        }
    }

    /// The shell's process id.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Undo Close: stop reading the program's output (it then waits), or go on.
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Release);
    }

    /// `set_paused(true)`. Reading the input mode never blocks on Unix, so there is no read to
    /// wait out; readers still check `is_paused` (`console_mode_unless`).
    pub fn pause_after_reads(&self) {
        self.set_paused(true);
    }

    /// Paused for Undo Close.
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    /// The pause flag, for a reader that checks it (`console_mode_unless`).
    pub fn pause_flag(&self) -> Arc<AtomicBool> {
        self.paused.clone()
    }

    /// Bytes queued for the child that it has not taken yet.
    pub fn pending(&self) -> usize {
        self.pending.load(Ordering::Relaxed)
    }

    /// The child's exit code (`128 + signal` for a killed one), or None while it runs.
    pub fn exit_code(&self) -> Option<u32> {
        self.wait_exit(Duration::ZERO)
    }

    /// Returns the next chunk of output, or None if nothing came within `wait`.
    pub fn read_timeout(&self, wait: Duration) -> Option<Vec<u8>> {
        match self.output.recv_timeout(wait) {
            Ok(chunk) => Some(chunk),
            Err(RecvTimeoutError::Timeout) => None,
            // The output closed. Sleep, so callers that poll do not spin.
            Err(RecvTimeoutError::Disconnected) => {
                std::thread::sleep(wait);
                None
            }
        }
    }

    /// Reads until `needle` appears. Ok with everything read on a hit, Err with everything read
    /// on timeout.
    pub fn read_until(&self, needle: &[u8], wait: Duration) -> Result<Vec<u8>, Vec<u8>> {
        crate::platform::pty::read_until_with(|d| self.read_timeout(d), needle, wait)
    }

    /// Waits for the child to exit and returns its exit code.
    pub fn wait_exit(&self, wait: Duration) -> Option<u32> {
        let g = self.exit.code.lock().unwrap();
        let (g, _) = self.exit.done.wait_timeout_while(g, wait, |c| c.is_none()).unwrap();
        *g
    }
}

impl Drop for Pty {
    /// Returns at once. A pty child's group is hung up and the master closes (the reader does
    /// that), which hangs up the slave for every holder; a pipes child sees its stdin close. A
    /// shell that ignores the hangup is killed (its group, for a pty) after `CLOSE_WAIT`.
    fn drop(&mut self) {
        self.set_paused(false);
        self.writer.take();
        let group = self.tty.is_some();
        if let Some(own) = self.tty.take() {
            // Only this pty's own entry: a reused pid may belong to a newer one by now.
            registry(|r| {
                if r.get(&self.pid).is_some_and(|e| Arc::ptr_eq(e, &own)) {
                    r.remove(&self.pid);
                }
            });
            self.exit.signal(self.pid, true, libc::SIGHUP);
        }
        // Wakes the reader, which drops its master.
        unsafe { libc::write(self.wake.as_raw_fd(), [0u8].as_ptr().cast(), 1) };
        let (exit, pid) = (self.exit.clone(), self.pid);
        std::thread::spawn(move || {
            let g = exit.code.lock().unwrap();
            let (g, _) = exit.done.wait_timeout_while(g, CLOSE_WAIT, |c| c.is_none()).unwrap();
            let ended = g.is_some();
            drop(g);
            if !ended {
                exit.signal(pid, group, libc::SIGKILL);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::by_os;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn os(n: &str, v: &str) -> (OsString, OsString) {
        (OsString::from(n), OsString::from(v))
    }

    fn text(b: &[u8]) -> String {
        String::from_utf8_lossy(b).into_owned()
    }

    fn run(cmd: &str) -> (Pty, String) {
        let p = spawn(&PtyApi, cmd, 80, 24).unwrap();
        assert_eq!(p.wait_exit(secs(10)).is_some(), true, "{cmd} did not end");
        let mut out = Vec::new();
        while let Some(c) = p.read_timeout(Duration::from_millis(200)) {
            out.extend(c);
        }
        (p, text(&out))
    }

    #[test]
    fn a_shell_echoes_through_the_pty() {
        let p = spawn(&PtyApi, "/bin/sh -c 'echo pty-ok'", 80, 24).unwrap();
        p.read_until(b"pty-ok", secs(5)).unwrap_or_else(|got| panic!("{:?}", text(&got)));
        assert_eq!(p.wait_exit(secs(5)), Some(0));
    }

    #[test]
    fn exit_codes_and_signals_are_reported() {
        assert_eq!(run("/bin/sh -c 'exit 4'").0.exit_code(), Some(4));
        assert_eq!(run("/bin/sh -c 'kill -TERM $$'").0.exit_code(), Some(143));
    }

    #[test]
    fn the_size_reaches_the_child() {
        let (_p, out) = {
            let p = spawn(&PtyApi, "/bin/sh -c 'stty size'", 100, 30).unwrap();
            p.read_until(b"30 100", secs(5)).map(|o| (p, text(&o))).unwrap_or_else(|got| panic!("{:?}", text(&got)))
        };
        assert!(out.contains("30 100"), "{out}");
        let p = spawn(&PtyApi, "env PS1=> ENV= /bin/sh -i", 80, 24).unwrap();
        p.read_until(b">", secs(5)).unwrap_or_else(|got| panic!("{:?}", text(&got)));
        p.resize(120, 40).unwrap();
        p.write(b"stty size\n").unwrap();
        p.read_until(b"40 120", secs(5)).unwrap_or_else(|got| panic!("{:?}", text(&got)));
    }

    #[test]
    fn the_pty_is_utf8() {
        let (_p, out) = run("/bin/sh -c 'stty -a'");
        assert!(out.contains("iutf8") && !out.contains("-iutf8"), "{out}");
    }

    #[test]
    fn outer_terminal_variables_never_reach_the_shell() {
        let env = child_env(vec![os("TERM", "xterm-kitty"), os("TMUX", "/tmp/t,1,0"), os("PATH", "/bin")], &["TERM=xterm-256color".to_string()]);
        assert!(env.contains(&os("TERM", "xterm-256color")), "{env:?}");
        assert!(env.contains(&os("PATH", "/bin")), "{env:?}");
        assert!(!env.iter().any(|(n, _)| n == "TMUX"), "{env:?}");
        assert!(!inherits_term());
        // And live: the shell's own TERM is the profile's, not what this process was started under.
        let outer = ["TMUX", "KITTY_WINDOW_ID", "ITERM_SESSION_ID"];
        let line = format!("/bin/sh -c 'echo T=$TERM:$TMUX:$KITTY_WINDOW_ID:$ITERM_SESSION_ID'");
        let p = spawn_notify(&PtyApi, &line, None, &["TERM=xterm-256color".to_string()], 80, 24, Arc::new(|| {})).unwrap();
        let got = p.read_until(b"T=xterm-256color:::", secs(5));
        assert!(got.is_ok(), "{:?} {outer:?}", got.err().map(|g| text(&g)));
    }

    #[test]
    fn a_shell_inherits_no_other_pty() {
        let _alive = spawn(&PtyApi, "env PS1=> ENV= /bin/sh -i", 80, 24).unwrap();
        let (_p, out) = run("/bin/sh -c 'for f in 3 4 5 6 7 8 9 10 11 12; do [ -e /dev/fd/$f ] && echo OPEN$f; done; echo END'");
        assert!(out.contains("END") && !out.contains("OPEN"), "{out}");
    }

    #[test]
    fn drop_ends_a_shell_that_ignores_hangup() {
        let p = spawn(&PtyApi, "/bin/sh -c 'trap \"\" HUP; while :; do sleep 1; done'", 80, 24).unwrap();
        let pid = p.pid() as libc::pid_t;
        // Past the trap being set.
        std::thread::sleep(Duration::from_millis(300));
        let t = std::time::Instant::now();
        drop(p);
        assert!(t.elapsed() < Duration::from_millis(500), "{:?}", t.elapsed());
        let end = std::time::Instant::now() + secs(3);
        while std::time::Instant::now() < end && unsafe { libc::kill(pid, 0) } == 0 {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "the shell is still there");
        assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    }

    #[test]
    fn pipes_transport_has_no_tty() {
        let p = spawn_transport(&PtyApi, Transport::Pipes, "/bin/sh -c 'test -t 0 || echo NO-TTY'", None, &[], 80, 24, Arc::new(|| {})).unwrap();
        p.read_until(b"NO-TTY", secs(5)).unwrap_or_else(|got| panic!("{:?}", text(&got)));
        assert_eq!(p.resize(90, 20), Ok(()));
        assert_eq!(p.clear(), Ok(()));
    }

    #[test]
    fn a_missing_program_reports_the_command() {
        let e = spawn(&PtyApi, "no-such-program-xyz --flag", 80, 24).err().unwrap();
        assert!(e.contains("no-such-program-xyz"), "{e}");
        let e = spawn(&PtyApi, "/bin/sh -c 'unclosed", 80, 24).err().unwrap();
        assert!(e.contains("unterminated"), "{e}");
        assert_eq!(e.matches("unclosed").count(), 1, "the line is named once: {e}");
        // A Windows command line is a plain command here, never a shell string.
        let e = spawn(&PtyApi, r#"pwsh.exe-not-here -NoLogo "C:\Program Files\x.exe""#, 80, 24).err().unwrap();
        assert!(e.contains("pwsh.exe-not-here"), "{e}");
    }

    #[test]
    fn console_mode_reads_the_echo_state() {
        let p = spawn(&PtyApi, "/bin/sh -c 'stty -echo; sleep 2'", 80, 24).unwrap();
        let no = AtomicBool::new(false);
        let end = std::time::Instant::now() + secs(2);
        let mut mode = console_mode_unless(p.pid(), &no);
        while !mode.as_ref().is_ok_and(|m| crate::pwm_prompt::at_prompt(*m)) && std::time::Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
            mode = console_mode_unless(p.pid(), &no);
        }
        assert!(mode.as_ref().is_ok_and(|m| crate::pwm_prompt::at_prompt(*m)), "{mode:?}");
        assert_eq!(console_mode_unless(p.pid(), &AtomicBool::new(true)), Err("the console is paused".to_string()));
        assert!(console_mode_unless(1, &no).is_err());
    }

    #[test]
    fn login_shell_takes_the_first_usable_candidate() {
        assert_eq!(login_shell(Some("/bin/sh"), None), "/bin/sh -l");
        assert_eq!(login_shell(Some("zsh"), Some("/bin/sh")), "/bin/sh -l", "not absolute");
        assert_eq!(login_shell(Some("/no/such/shell"), Some("/bin/sh")), "/bin/sh -l", "not there");
        assert_eq!(login_shell(None, None), "/bin/sh -l");
        assert_eq!(by_os("", "x"), if cfg!(windows) { "" } else { "x" });
    }

    #[test]
    fn the_foreground_group_is_the_shells_own() {
        let p = spawn(&PtyApi, "env PS1=> ENV= /bin/sh -i", 80, 24).unwrap();
        p.read_until(b">", secs(5)).unwrap_or_else(|got| panic!("{:?}", text(&got)));
        assert_eq!(foreground_group(p.pid()), Some(p.pid() as i32));
        assert_eq!(foreground_group(1), None);
        let pid = p.pid();
        drop(p);
        assert_eq!(foreground_group(pid), None, "gone with the pty");
    }

    #[test]
    fn terminate_ends_the_shell_only() {
        let p = spawn(&PtyApi, "/bin/sh -c 'sleep 30'", 80, 24).unwrap();
        p.terminate();
        assert_eq!(p.wait_exit(secs(5)), Some(137));
    }

    #[test]
    fn names_are_case_sensitive_and_raw_values_survive() {
        let env = child_env(vec![os("HTTP_PROXY", "inherited"), os("PATH", "/bin")], &["http_proxy=x".to_string()]);
        assert!(env.contains(&os("HTTP_PROXY", "inherited")), "a lower case name is another variable: {env:?}");
        assert!(env.contains(&os("http_proxy", "x")), "{env:?}");
        let env = child_env(vec![os("PATH", "/bin"), os("Path", "/x")], &["-path".to_string(), "-Path".to_string()]);
        assert_eq!(env, vec![os("PATH", "/bin")], "-Path removes only Path");
        // A value that is not UTF-8 reaches the child as it is.
        let raw = (OsString::from("RAWV"), OsString::from_vec(vec![b'f', 0xff]));
        assert!(child_env(vec![raw.clone()], &[]).contains(&raw));
        let p = spawn_with_env(vec![os("PATH", "/bin:/usr/bin"), raw], Transport::Conpty, "/bin/sh -c 'printf %s \"$RAWV\" | od -An -tx1 | tr -d \" \"'", None, &[], 80, 24, Arc::new(|| {})).unwrap();
        let got = p.read_until(b"66ff", secs(5));
        assert!(got.is_ok(), "{:?}", got.err().map(|g| text(&g)));
    }

    #[test]
    fn a_drop_leaves_a_newer_ptys_registry_entry() {
        let p = spawn(&PtyApi, "/bin/sh -c 'sleep 5'", 80, 24).unwrap();
        let pid = p.pid();
        // As if the pid was reused by a newer pty whose entry replaced this one.
        let (_r, w) = make_pipe().unwrap();
        let newer = Arc::new(w);
        registry(|r| r.insert(pid, newer.clone()));
        drop(p);
        assert!(registry(|r| r.get(&pid).is_some_and(|e| Arc::ptr_eq(e, &newer))), "the newer entry was removed");
        registry(|r| r.remove(&pid));
    }
}

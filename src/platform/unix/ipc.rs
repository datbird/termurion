//! The session host's local IPC on Unix: a stream socket in a private per-user folder.
//!
//! The folder is the security boundary. It is made 0700 by `mkdir` itself (never a chmod after
//! the fact on a path someone could have planted), then checked with `lstat` (a real folder, ours,
//! no group or other bits). A socket path is never followed through a link, every fd is
//! close-on-exec from birth, and the peer's uid is read before a byte is.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::api_auth::NoauthFile;
use crate::api_peer::Peer;
use crate::platform::Unsupported;

const FEATURE: &str = "Session host";
const API: &str = "The automation API";
const ALLOW_ALL: &str = "The automation allow-all file";

fn no<T>() -> Result<T, String> {
    Err(Unsupported { feature: FEATURE }.into())
}

fn euid() -> u32 {
    unsafe { libc::geteuid() }
}

/// The one decision on a peer: the same user as `own`, or refused.
fn same_user(peer_uid: u32, own: u32) -> Result<(), String> {
    if peer_uid == own { Ok(()) } else { Err(format!("the peer runs as user {peer_uid}, not this user")) }
}

fn errno_is(e: &std::io::Error, codes: &[i32]) -> bool {
    e.raw_os_error().is_some_and(|c| codes.contains(&c))
}

/// Where the socket files live for this user: `$XDG_RUNTIME_DIR/termurion` when that folder is
/// absolute and ours, else `<tmp>/termurion-<euid>`. Pure but for the one ownership look.
pub fn runtime_dir_from(xdg: Option<&Path>, tmp: &Path, euid: u32) -> PathBuf {
    if let Some(x) = xdg
        && x.is_absolute()
        && std::fs::symlink_metadata(x).is_ok_and(|m| m.file_type().is_dir() && m.uid() == euid && m.mode() & 0o022 == 0)
    {
        return x.join("termurion");
    }
    tmp.join(format!("termurion-{euid}"))
}

/// A folder only this user can enter, as it is now: a real folder (never a link), ours, with
/// no group or other permission.
pub fn check_private(dir: &Path) -> Result<(), String> {
    let refused = || format!("{} is not a private folder of this user", dir.display());
    let m = std::fs::symlink_metadata(dir).map_err(|_| refused())?;
    if !m.file_type().is_dir() || m.uid() != euid() || m.mode() & 0o077 != 0 {
        return Err(refused());
    }
    Ok(())
}

/// The folder that will hold the runtime folder must not let another user rename ours away: not
/// group or other writable, unless it is sticky (/tmp). A link to such a folder (macOS /tmp) is followed.
pub fn check_parent(parent: &Path) -> Result<(), String> {
    let m = std::fs::metadata(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    if !m.is_dir() || (m.mode() & 0o022 != 0 && m.mode() & 0o1000 == 0) {
        return Err(format!("{} lets other users rename folders in it", parent.display()));
    }
    Ok(())
}

/// Where the runtime folder is now, made or not.
fn runtime_dir_path() -> PathBuf {
    let xdg = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    let tmp = std::env::var_os("TMPDIR").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| PathBuf::from("/tmp"));
    runtime_dir_from(xdg.as_deref(), &tmp, euid())
}

/// The runtime folder, made 0700 when missing and checked either way.
pub fn runtime_dir() -> Result<PathBuf, String> {
    let dir = runtime_dir_path();
    if let Some(parent) = dir.parent() {
        check_parent(parent)?;
    }
    // The mode goes to mkdir itself, so there is no moment the folder is more open than 0700.
    match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    }
    check_private(&dir)?;
    Ok(dir)
}

/// A bare name lives in the runtime folder; a name with a `/` is the socket path itself (its
/// folder must be private too).
fn resolve(name: &str) -> Result<PathBuf, String> {
    if name.contains('/') {
        let path = PathBuf::from(name);
        if let Some(parent) = path.parent() {
            // A missing folder is made 0700 by mkdir itself (one level), then checked as any.
            let _ = std::fs::DirBuilder::new().mode(0o700).create(parent);
            check_private(parent)?;
        }
        Ok(path)
    } else {
        Ok(runtime_dir()?.join(name))
    }
}

fn addr_for(path: &Path) -> Result<(libc::sockaddr_un, libc::socklen_t), String> {
    let bytes = path.as_os_str().as_bytes();
    let mut sa: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    // The limit is the struct's own: 104 on macOS, 108 on Linux, and room for the NUL.
    if bytes.len() >= sa.sun_path.len() {
        return Err(format!("the socket path {} is too long", path.display()));
    }
    if bytes.contains(&0) {
        return Err(format!("the socket path {} holds a NUL", path.display()));
    }
    sa.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (d, s) in sa.sun_path.iter_mut().zip(bytes) {
        *d = *s as libc::c_char;
    }
    #[cfg(target_os = "macos")]
    {
        sa.sun_len = std::mem::size_of::<libc::sockaddr_un>() as u8;
    }
    Ok((sa, std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t))
}

fn set_nonblocking(fd: RawFd, on: bool) -> std::io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let want = if on { flags | libc::O_NONBLOCK } else { flags & !libc::O_NONBLOCK };
    if unsafe { libc::fcntl(fd, libc::F_SETFL, want) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Linux makes the fd close-on-exec in the same call that creates it.
#[cfg(target_os = "linux")]
fn new_socket() -> std::io::Result<OwnedFd> {
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// macOS has no `SOCK_CLOEXEC`: the flag is set at once, under `SPAWN_LOCK` so none of our own
/// spawns forks in between.
#[cfg(not(target_os = "linux"))]
fn new_socket() -> std::io::Result<OwnedFd> {
    let _gate = super::pty::spawn_gate();
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    finish_fd(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// macOS: close-on-exec, and no SIGPIPE from a write to a gone peer.
#[cfg(not(target_os = "linux"))]
fn finish_fd(fd: OwnedFd) -> std::io::Result<OwnedFd> {
    if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let on: libc::c_int = 1;
    let r = unsafe { libc::setsockopt(fd.as_raw_fd(), libc::SOL_SOCKET, libc::SO_NOSIGPIPE, (&on as *const libc::c_int).cast(), std::mem::size_of::<libc::c_int>() as libc::socklen_t) };
    if r < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(fd)
}

/// One connection attempt that never blocks: a full backlog is EAGAIN, not a hang.
fn try_connect(sa: &libc::sockaddr_un, len: libc::socklen_t) -> std::io::Result<OwnedFd> {
    let fd = new_socket()?;
    set_nonblocking(fd.as_raw_fd(), true)?;
    if unsafe { libc::connect(fd.as_raw_fd(), (sa as *const libc::sockaddr_un).cast(), len) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    set_nonblocking(fd.as_raw_fd(), false)?;
    Ok(fd)
}

/// Waits up to `wait` for `fd` to be readable (or hung up). False when the time ran out.
fn poll_in(fd: RawFd, wait: Duration) -> Result<bool, String> {
    let end = Instant::now() + wait;
    loop {
        let left = end.saturating_duration_since(Instant::now());
        // Round up, so a short wait is not a busy loop.
        let ms = left.as_millis().saturating_add(u128::from(left.subsec_nanos() % 1_000_000 != 0)).min(i32::MAX as u128) as libc::c_int;
        let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        let n = unsafe { libc::poll(&mut p, 1, ms) };
        if n > 0 {
            return Ok(true);
        }
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() != Some(libc::EINTR) {
                return Err(format!("poll failed: {e}"));
            }
        }
        if Instant::now() >= end {
            return Ok(false);
        }
    }
}

/// A connected socket end.
pub struct Pipe {
    fd: OwnedFd,
}

/// The listening side: a bound socket and the file it made.
pub struct Listener {
    fd: OwnedFd,
    path: PathBuf,
    /// The file it bound, so drop only removes its own.
    id: (u64, u64),
    /// The held `<name>.lock`: one host per name, whatever the probe below sees.
    lock: HostLock,
    /// When the socket and lock file times were last refreshed.
    touched: Instant,
    /// The name it was bound under, to bind again when its file is gone.
    name: String,
    /// When `keep_reachable` last looked.
    checked: Instant,
}

/// How often a host looks that its socket is still where a client would look for it.
const CHECK_EVERY: Duration = Duration::from_secs(2);

/// Tmp cleaners (macOS, systemd-tmpfiles) remove files nobody touched for days; a live host
/// touches its two files this often.
const TOUCH_EVERY: Duration = Duration::from_secs(3600);

/// An exclusive flock on `<socket>.lock`, held from before the stale probe until drop. With it,
/// probe, unlink and bind cannot interleave with another host of this user.
struct HostLock {
    _fd: OwnedFd,
    path: PathBuf,
    /// The locked file, so drop only removes its own.
    id: (u64, u64),
}

impl HostLock {
    fn take(socket: &Path) -> Result<HostLock, String> {
        let mut lock_path = socket.as_os_str().to_owned();
        lock_path.push(".lock");
        let lock_path = PathBuf::from(lock_path);
        let c = std::ffi::CString::new(lock_path.as_os_str().as_bytes()).map_err(|_| format!("{} holds a NUL", lock_path.display()))?;
        // A holder that unlinks its file at drop may leave us a file nobody else sees: retry on that.
        for _ in 0..8 {
            let raw = unsafe { libc::open(c.as_ptr(), libc::O_RDWR | libc::O_CREAT | libc::O_CLOEXEC | libc::O_NOFOLLOW, 0o600 as libc::c_uint) };
            if raw < 0 {
                return Err(format!("could not open {}: {}", lock_path.display(), std::io::Error::last_os_error()));
            }
            let fd = unsafe { OwnedFd::from_raw_fd(raw) };
            if unsafe { libc::flock(fd.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } < 0 {
                let e = std::io::Error::last_os_error();
                if errno_is(&e, &[libc::EWOULDBLOCK, libc::EAGAIN]) {
                    return Err(format!("another session host is already listening at {}", socket.display()));
                }
                return Err(format!("could not lock {}: {e}", lock_path.display()));
            }
            // Still the file at the path (not unlinked by the last holder while we waited)?
            let mut st: libc::stat = unsafe { std::mem::zeroed() };
            if unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } < 0 {
                return Err(format!("{}: {}", lock_path.display(), std::io::Error::last_os_error()));
            }
            let mine = (st.st_dev as u64, st.st_ino as u64);
            match std::fs::symlink_metadata(&lock_path) {
                Ok(m) if (m.dev(), m.ino()) == mine => return Ok(HostLock { _fd: fd, path: lock_path, id: mine }),
                _ => continue,
            }
        }
        Err(format!("could not lock {}", lock_path.display()))
    }
}

impl HostLock {
    /// Whether the file at the path is still the one this lock holds.
    fn still_ours(&self) -> bool {
        std::fs::symlink_metadata(&self.path).is_ok_and(|m| (m.dev(), m.ino()) == self.id)
    }
}

impl Drop for HostLock {
    fn drop(&mut self) {
        // Removed while still locked, so the next host makes a fresh file; a newer host's file stays.
        if self.still_ours() {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Why `connect_owned` failed.
#[derive(Debug)]
pub enum ConnectError {
    /// The server belongs to another user.
    Denied(String),
    Other(String),
}

fn file_id(path: &Path) -> Option<(u64, u64)> {
    std::fs::symlink_metadata(path).ok().filter(|m| m.file_type().is_socket()).map(|m| (m.dev(), m.ino()))
}

impl Listener {
    /// Binds `name`. Fails when a live host already listens there; a file left by a dead one
    /// is replaced.
    pub fn bind(name: &str) -> Result<Listener, String> {
        let path = resolve(name)?;
        let lock = HostLock::take(&path)?;
        let (fd, id) = bind_socket(&path)?;
        Ok(Listener { fd, path, id, lock, touched: Instant::now(), name: name.to_string(), checked: Instant::now() })
    }

    /// Logind removes the runtime folder at the last logout, and a tmp cleaner may take the
    /// socket; the host lives on with its shells, so it binds again where a client looks now
    /// (making the 0700 folder and the lock again). A newer host already there is left alone:
    /// this one keeps its shells and tries again later.
    fn keep_reachable(&mut self) {
        self.checked = Instant::now();
        let want = where_now(&self.name);
        let socket_ok = file_id(&self.path) == Some(self.id);
        if want == self.path && socket_ok && self.lock.still_ours() {
            return;
        }
        // Same place, the socket still ours: only the lock file went, so only the lock is taken again
        // (a full bind would probe this host's own socket and fail).
        let rebound = if want == self.path && socket_ok {
            HostLock::take(&self.path).map(|lock| self.lock = lock)
        // Same place and the lock is still ours: only the socket file went.
        } else if want == self.path && self.lock.still_ours() {
            bind_socket(&self.path).map(|(fd, id)| {
                self.fd = fd;
                self.id = id;
            })
        } else {
            Listener::bind(&self.name).map(|l| *self = l)
        };
        match rebound {
            Ok(()) => crate::dlog!("The session host listens again after its socket was gone"),
            Err(_) => crate::dlog!("The session host socket is gone and could not be made again"),
        }
    }
}

/// Where `name`'s socket would be now, without making anything.
fn where_now(name: &str) -> PathBuf {
    if name.contains('/') { PathBuf::from(name) } else { runtime_dir_path().join(name) }
}

/// Binds and listens at `path` (the lock already held); a file left by a dead host is replaced.
fn bind_socket(path: &Path) -> Result<(OwnedFd, (u64, u64)), String> {
    let (sa, len) = addr_for(path)?;
    let fd = new_socket().map_err(|e| format!("socket failed: {e}"))?;
    let mut replaced = false;
    loop {
        if unsafe { libc::bind(fd.as_raw_fd(), (&sa as *const libc::sockaddr_un).cast(), len) } == 0 {
            break;
        }
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() != Some(libc::EADDRINUSE) || replaced {
            return Err(format!("could not bind {}: {e}", path.display()));
        }
        replaced = true;
        match try_connect(&sa, len) {
            Ok(_) => return Err(format!("another session host is already listening at {}", path.display())),
            // A full backlog is EAGAIN on Linux; macOS answers ECONNREFUSED instead, which is why
            // the held lock above, not this probe, is what keeps a live host's socket safe.
            Err(c) if errno_is(&c, &[libc::EAGAIN]) => return Err(format!("another session host is already listening at {}", path.display())),
            Err(c) if errno_is(&c, &[libc::ECONNREFUSED, libc::ENOENT]) => {}
            Err(c) => return Err(format!("could not look at {}: {c}", path.display())),
        }
        // Stale: only a socket of ours is removed, never a file someone put there.
        match std::fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_socket() && m.uid() == euid() => {
                if let Err(e) = std::fs::remove_file(&path)
                    && e.kind() != std::io::ErrorKind::NotFound {
                        return Err(format!("could not remove {}: {e}", path.display()));
                    }
            }
            Ok(_) => return Err(format!("{} exists and is not a socket of this user", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
    }
    let id = file_id(path).ok_or_else(|| format!("{} is not the socket just bound", path.display()))?;
    // 0600 whatever the umask, before anyone can connect; the private folder means nobody swapped it.
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| format!("{} holds a NUL", path.display()))?;
    if unsafe { libc::chmod(c.as_ptr(), 0o600) } < 0 {
        return Err(format!("could not set the mode of {}: {}", path.display(), std::io::Error::last_os_error()));
    }
    if unsafe { libc::listen(fd.as_raw_fd(), 16) } < 0 {
        return Err(format!("listen failed: {}", std::io::Error::last_os_error()));
    }
    // accept must never block while it holds the macOS spawn gate.
    set_nonblocking(fd.as_raw_fd(), true).map_err(|e| format!("fcntl failed: {e}"))?;
    Ok((fd, id))
}

impl Listener {
    /// A socket that listens serves any number of clients one after another, so there is no
    /// instance to replace (the Windows pipe needs a fresh one per client).
    pub fn renew(&mut self) -> Result<(), String> {
        Ok(())
    }

    /// Sets the access and modify times of the socket and the lock file to now, so a tmp cleaner
    /// never takes them from under a live host. A file already gone is logged here;
    /// `keep_reachable` makes it again.
    pub fn refresh(&mut self) {
        self.touched = Instant::now();
        let now = [libc::timespec { tv_sec: 0, tv_nsec: libc::UTIME_NOW }; 2];
        if file_id(&self.path) == Some(self.id) {
            if let Ok(c) = std::ffi::CString::new(self.path.as_os_str().as_bytes()) {
                unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), now.as_ptr(), libc::AT_SYMLINK_NOFOLLOW) };
            }
        } else {
            crate::dlog!("The session host socket file is gone");
        }
        if std::fs::symlink_metadata(&self.lock.path).is_ok() {
            unsafe { libc::futimens(self.lock._fd.as_raw_fd(), now.as_ptr()) };
        } else {
            crate::dlog!("The session host lock file is gone");
        }
    }

    /// Waits up to `wait` for a client of this user. None when none came; a peer of another user
    /// is dropped unread and counts as none.
    pub fn accept(&mut self, wait: Duration) -> Result<Option<Pipe>, String> {
        if self.checked.elapsed() >= CHECK_EVERY {
            self.keep_reachable();
        }
        if self.touched.elapsed() >= TOUCH_EVERY {
            self.refresh();
        }
        if !poll_in(self.fd.as_raw_fd(), wait)? {
            return Ok(None);
        }
        let fd = match accept_cloexec(self.fd.as_raw_fd()) {
            Ok(fd) => fd,
            Err(e) if errno_is(&e, &[libc::EAGAIN, libc::EWOULDBLOCK, libc::EINTR, libc::ECONNABORTED]) => return Ok(None),
            Err(e) => return Err(format!("accept failed: {e}")),
        };
        // Sockets accepted from a non-blocking listener may inherit that (BSD).
        set_nonblocking(fd.as_raw_fd(), false).map_err(|e| format!("fcntl failed: {e}"))?;
        let pipe = Pipe { fd };
        match pipe.peer_ids().and_then(|(uid, _)| same_user(uid, euid())) {
            Ok(()) => Ok(Some(pipe)),
            Err(e) => {
                crate::host_base::log(&format!("host: dropped a connection, {e}"));
                Ok(None)
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn accept_cloexec(listener: RawFd) -> std::io::Result<OwnedFd> {
    let fd = unsafe { libc::accept4(listener, std::ptr::null_mut(), std::ptr::null_mut(), libc::SOCK_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

#[cfg(not(target_os = "linux"))]
fn accept_cloexec(listener: RawFd) -> std::io::Result<OwnedFd> {
    let _gate = super::pty::spawn_gate();
    let fd = unsafe { libc::accept(listener, std::ptr::null_mut(), std::ptr::null_mut()) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    finish_fd(unsafe { OwnedFd::from_raw_fd(fd) })
}

impl Drop for Listener {
    fn drop(&mut self) {
        // Only the file this listener bound: a newer host's socket at the same path stays.
        if file_id(&self.path) == Some(self.id) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

impl Pipe {
    /// The peer's (uid, pid), as the kernel recorded them.
    #[cfg(target_os = "linux")]
    fn peer_ids(&self) -> Result<(u32, u32), String> {
        let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let r = unsafe { libc::getsockopt(self.fd.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut cred as *mut libc::ucred).cast(), &mut len) };
        if r < 0 {
            return Err(format!("SO_PEERCRED failed: {}", std::io::Error::last_os_error()));
        }
        Ok((cred.uid, cred.pid as u32))
    }

    #[cfg(not(target_os = "linux"))]
    fn peer_ids(&self) -> Result<(u32, u32), String> {
        let (mut uid, mut gid) = (0 as libc::uid_t, 0 as libc::gid_t);
        if unsafe { libc::getpeereid(self.fd.as_raw_fd(), &mut uid, &mut gid) } < 0 {
            return Err(format!("getpeereid failed: {}", std::io::Error::last_os_error()));
        }
        let mut pid: libc::pid_t = 0;
        let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
        let r = unsafe { libc::getsockopt(self.fd.as_raw_fd(), libc::SOL_LOCAL, libc::LOCAL_PEERPID, (&mut pid as *mut libc::pid_t).cast(), &mut len) };
        if r < 0 {
            return Err(format!("LOCAL_PEERPID failed: {}", std::io::Error::last_os_error()));
        }
        Ok((uid, pid as u32))
    }

    /// Reads what is there (blocking). 0 means the other end closed or reset.
    pub fn read(&self, buf: &mut [u8]) -> Result<usize, String> {
        loop {
            let n = unsafe { libc::recv(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len(), 0) };
            if n >= 0 {
                return Ok(n as usize);
            }
            let e = std::io::Error::last_os_error();
            match e.raw_os_error() {
                Some(libc::EINTR) => continue,
                // A gone peer reads as closed, as on Windows.
                Some(libc::ECONNRESET | libc::EPIPE | libc::ENOTCONN) => return Ok(0),
                _ => return Err(format!("read failed: {e}")),
            }
        }
    }

    /// Reads what is there, waiting at most `wait`. None when nothing came; Some(0) when the
    /// other end closed.
    pub fn read_timeout(&self, buf: &mut [u8], wait: Duration) -> Result<Option<usize>, String> {
        if !poll_in(self.fd.as_raw_fd(), wait)? {
            return Ok(None);
        }
        self.read(buf).map(Some)
    }

    pub fn write_all(&self, bytes: &[u8]) -> Result<(), String> {
        let mut left = bytes;
        while !left.is_empty() {
            #[cfg(target_os = "linux")]
            let flags = libc::MSG_NOSIGNAL;
            // macOS sets SO_NOSIGPIPE on the socket instead.
            #[cfg(not(target_os = "linux"))]
            let flags = 0;
            let n = unsafe { libc::send(self.fd.as_raw_fd(), left.as_ptr().cast(), left.len(), flags) };
            if n < 0 {
                let e = std::io::Error::last_os_error();
                if e.raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                return Err(format!("write failed: {e}"));
            }
            left = &left[n as usize..];
        }
        Ok(())
    }

    /// Ends every read and write in progress, so a blocked reader returns 0 and the other end
    /// sees the socket close.
    pub fn shutdown(&self) {
        unsafe { libc::shutdown(self.fd.as_raw_fd(), libc::SHUT_RDWR) };
    }

    /// The pid of the process at the other end.
    pub fn server_pid(&self) -> Result<u32, String> {
        self.peer_ids().map(|(_, pid)| pid)
    }

    /// Ok when the other end runs as this user.
    pub fn server_is_this_user(&self) -> Result<(), String> {
        let (uid, _) = self.peer_ids()?;
        same_user(uid, euid()).map_err(|_| "the session host belongs to another user".to_string())
    }
}

/// Connects to `name`. None when nothing listens there.
pub fn connect(name: &str, wait: Duration) -> Result<Option<Pipe>, String> {
    let path = resolve(name)?;
    let (sa, len) = addr_for(&path)?;
    let end = Instant::now() + wait;
    loop {
        // Never through a link, never onto something that is not a socket.
        match std::fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("{}: {e}", path.display())),
            Ok(m) if !m.file_type().is_socket() => return Err(format!("{} is not a socket", path.display())),
            Ok(_) => {}
        }
        let busy = match try_connect(&sa, len) {
            Ok(fd) => return Ok(Some(Pipe { fd })),
            Err(e) if errno_is(&e, &[libc::ENOENT]) => return Ok(None),
            Err(e) if errno_is(&e, &[libc::ECONNREFUSED]) => false,
            Err(e) if errno_is(&e, &[libc::EAGAIN]) => true,
            Err(e) => return Err(format!("could not connect to {}: {e}", path.display())),
        };
        if Instant::now() >= end {
            return if busy { Err(format!("the session host at {} is busy", path.display())) } else { Ok(None) };
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// `connect`, refusing a server that runs as another user.
pub fn connect_owned(name: &str, wait: Duration) -> Result<Option<Pipe>, ConnectError> {
    let Some(pipe) = connect(name, wait).map_err(ConnectError::Other)? else { return Ok(None) };
    match pipe.peer_ids() {
        Ok((uid, _)) => match same_user(uid, euid()) {
            Ok(()) => Ok(Some(pipe)),
            Err(_) => Err(ConnectError::Denied("the session host belongs to another user".to_string())),
        },
        Err(e) => Err(ConnectError::Other(e)),
    }
}

/// The host's socket: `host.sock` in the runtime folder.
pub fn host_pipe_name() -> Result<String, String> {
    let path = runtime_dir()?.join("host.sock");
    path.to_str().map(str::to_string).ok_or_else(|| format!("{} is not text", path.display()))
}

pub fn secret_pipe_name() -> Result<String, String> {
    no()
}

pub fn url_pipe_name() -> Result<String, String> {
    no()
}

/// Denies: the peer facts cannot be read, so a caller cannot judge a peer.
pub fn own_peer() -> Result<Peer, String> {
    no()
}

/// Denies: as `own_peer`.
pub fn thread_peer() -> Result<Peer, String> {
    no()
}

/// Denies: whether this process is elevated cannot be read, and a caller treats Err as "not elevated, not trusted".
pub fn elevated() -> Result<bool, String> {
    no()
}

/// Denies: no user identity text exists here.
pub fn user_sid() -> Result<String, String> {
    no()
}

pub fn available() -> Result<(), Unsupported> {
    Err(Unsupported { feature: ALLOW_ALL })
}

pub fn set_file_owner_to_user(_file: &std::fs::File) -> Result<(), String> {
    Err(Unsupported { feature: ALLOW_ALL }.into())
}

pub fn create_new_for_write(_path: &str, _set_owner: bool) -> Result<std::fs::File, String> {
    Err(Unsupported { feature: ALLOW_ALL }.into())
}

/// No peer check on Unix yet, so every connection is refused.
pub fn check_peer(_peer: std::net::SocketAddr, _local: std::net::SocketAddr) -> Result<u32, String> {
    Err(Unsupported { feature: API }.into())
}

/// No allow-all file is read on Unix: the check is Windows' (owner SID and DACL), so every start
/// has none and the API keeps asking for the cookie (the safe answer).
pub fn read_noauth_file(_path: &str) -> Option<NoauthFile> {
    None
}

#[cfg(test)]
pub(crate) fn test_name(what: &str) -> String {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    format!("termurion-test-{what}-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A scratch path removed when it goes out of scope, even after a failed assert.
    struct Scratch(PathBuf);

    impl std::ops::Deref for Scratch {
        type Target = PathBuf;
        fn deref(&self) -> &PathBuf {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            if std::fs::symlink_metadata(&self.0).is_ok_and(|m| m.is_dir()) {
                let _ = std::fs::remove_dir_all(&self.0);
            } else {
                let _ = std::fs::remove_file(&self.0);
            }
        }
    }

    /// A fresh scratch folder with the given mode.
    fn scratch(mode: u32) -> Scratch {
        let d = std::env::temp_dir().join(test_name("dir"));
        std::fs::DirBuilder::new().mode(mode).create(&d).unwrap();
        std::fs::set_permissions(&d, std::os::unix::fs::PermissionsExt::from_mode(mode)).unwrap();
        Scratch(d)
    }

    #[test]
    fn refresh_brings_old_file_times_to_now() {
        let name = test_name("touch");
        let mut l = Listener::bind(&name).unwrap();
        let path = runtime_dir().unwrap().join(&name);
        let lock = runtime_dir().unwrap().join(format!("{name}.lock"));
        let old = [libc::timespec { tv_sec: 1000, tv_nsec: 0 }; 2];
        for f in [&path, &lock] {
            let c = std::ffi::CString::new(f.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), old.as_ptr(), libc::AT_SYMLINK_NOFOLLOW) }, 0);
            assert_eq!(std::fs::symlink_metadata(f).unwrap().mtime(), 1000, "the test set an old time");
        }
        l.refresh();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        for f in [&path, &lock] {
            let m = std::fs::symlink_metadata(f).unwrap();
            assert!((now - m.mtime()).abs() < 60 && (now - m.atime()).abs() < 60, "{} was touched", f.display());
        }
        // A file that is gone is logged, not an error or a panic.
        std::fs::remove_file(&path).unwrap();
        l.refresh();
    }

    #[test]
    fn only_the_same_user_is_admitted() {
        assert!(same_user(1000, 1000).is_ok());
        assert!(same_user(0, 0).is_ok());
        assert!(same_user(1001, 1000).is_err(), "a different uid is refused");
        assert!(same_user(0, 1000).is_err(), "even root is not this user");
        assert!(same_user(1000, 0).is_err());
    }

    #[test]
    fn two_binders_leave_exactly_one_winner() {
        for round in 0..20 {
            let name = test_name(&format!("race{round}"));
            let path = runtime_dir().unwrap().join(&name);
            let gate = Arc::new(std::sync::Barrier::new(2));
            let run = |gate: Arc<std::sync::Barrier>, name: String| {
                std::thread::spawn(move || {
                    gate.wait();
                    Listener::bind(&name)
                })
            };
            let (a, b) = (run(gate.clone(), name.clone()), run(gate, name.clone()));
            let (a, b) = (a.join().unwrap(), b.join().unwrap());
            let winners: Vec<&Listener> = [&a, &b].into_iter().filter_map(|r| r.as_ref().ok()).collect();
            assert_eq!(winners.len(), 1, "exactly one host wins ({:?} / {:?})", a.as_ref().err(), b.as_ref().err());
            assert_eq!(file_id(&path), Some(winners[0].id), "the path keeps the winner's socket");
            let loser = if a.is_err() { a.err() } else { b.err() }.unwrap();
            assert!(loser.contains("already listening"), "{loser}");
        }
    }

    #[test]
    fn a_world_writable_parent_is_refused() {
        let open = scratch(0o777);
        assert!(check_parent(&open).unwrap_err().contains("rename"));
        let sticky = scratch(0o700);
        std::fs::set_permissions(&*sticky, std::os::unix::fs::PermissionsExt::from_mode(0o1777)).unwrap();
        check_parent(&sticky).unwrap();
        let private = scratch(0o700);
        check_parent(&private).unwrap();
        // An XDG folder others can write into is not trusted either.
        assert_ne!(runtime_dir_from(Some(&open), Path::new("/tmp"), euid()), open.join("termurion"));
    }

    #[test]
    fn a_socket_round_trips_between_threads() {
        let name = test_name("trip");
        let mut l = Listener::bind(&name).unwrap();
        let server = std::thread::spawn(move || {
            let p = l.accept(Duration::from_secs(5)).unwrap().expect("a client");
            let mut buf = [0u8; 16];
            let n = p.read(&mut buf).unwrap();
            p.write_all(&buf[..n]).unwrap();
            p
        });
        let c = connect(&name, Duration::from_secs(5)).unwrap().expect("connects");
        assert_eq!(c.server_pid().unwrap(), std::process::id());
        c.server_is_this_user().unwrap();
        c.write_all(b"ping").unwrap();
        let mut buf = [0u8; 16];
        let n = c.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"ping");
        let p = server.join().unwrap();
        p.shutdown();
        assert_eq!(c.read(&mut buf).unwrap(), 0, "a closed peer reads as 0");
        assert_eq!(c.read_timeout(&mut buf, Duration::from_millis(50)).unwrap(), Some(0));
    }

    #[test]
    fn a_quiet_peer_times_out() {
        let name = test_name("quiet");
        let mut l = Listener::bind(&name).unwrap();
        let c = connect_owned(&name, Duration::from_secs(1)).unwrap().expect("connects");
        let _p = l.accept(Duration::from_secs(1)).unwrap().expect("accepted");
        let mut buf = [0u8; 4];
        assert_eq!(c.read_timeout(&mut buf, Duration::from_millis(30)).unwrap(), None);
        assert!(l.accept(Duration::from_millis(30)).unwrap().is_none());
    }

    #[test]
    fn connect_finds_nothing_where_nothing_listens() {
        assert!(connect(&test_name("none"), Duration::ZERO).unwrap().is_none());
    }

    /// A runtime folder removed under a live host (logind at the last logout): the host makes the
    /// 0700 folder, lock and socket again, and a client reaches it there.
    #[test]
    fn a_host_whose_folder_is_gone_listens_again() {
        let dir = scratch(0o700);
        let path = dir.join("h");
        let name = path.to_string_lossy().into_owned();
        let mut l = Listener::bind(&name).unwrap();
        std::fs::remove_dir_all(&*dir).unwrap();
        assert!(std::os::unix::net::UnixStream::connect(&path).is_err(), "nothing to reach");
        l.checked = Instant::now() - CHECK_EVERY;
        assert!(l.accept(Duration::from_millis(10)).unwrap().is_none());
        check_private(&dir).expect("the folder is back, private");
        assert!(file_id(&path).is_some() && dir.join("h.lock").exists(), "socket and lock are back");
        let c = connect_owned(&name, Duration::from_secs(1)).unwrap().expect("reachable again");
        assert!(l.accept(Duration::from_secs(1)).unwrap().is_some());
        drop(c);
        // Only the socket file gone (the lock still ours): the socket alone comes back.
        std::fs::remove_file(&path).unwrap();
        l.checked = Instant::now() - CHECK_EVERY;
        assert!(l.accept(Duration::from_millis(10)).unwrap().is_none());
        let c = connect_owned(&name, Duration::from_secs(1)).unwrap().expect("reachable again");
        assert!(l.accept(Duration::from_secs(1)).unwrap().is_some());
        drop(c);
        // A newer host took the path meanwhile: it is left alone, and this one keeps going.
        std::fs::remove_dir_all(&*dir).unwrap();
        let newer = Listener::bind(&name).unwrap();
        l.checked = Instant::now() - CHECK_EVERY;
        assert!(l.accept(Duration::from_millis(10)).unwrap().is_none());
        assert_eq!(file_id(&path), Some(newer.id), "the newer host's socket stays");
        drop(l);
        assert_eq!(file_id(&path), Some(newer.id));
        assert!(dir.join("h.lock").exists(), "and so does its lock file");
    }

    /// Only the lock file removed (a tmp cleaner): the lock alone is taken again, and the host
    /// never probes its own socket.
    #[test]
    fn a_host_whose_lock_file_is_gone_takes_it_again() {
        let dir = scratch(0o700);
        let path = dir.join("h");
        let lock = dir.join("h.lock");
        let name = path.to_string_lossy().into_owned();
        let mut l = Listener::bind(&name).unwrap();
        let id = l.id;
        std::fs::remove_file(&lock).unwrap();
        l.checked = Instant::now() - CHECK_EVERY;
        assert!(l.accept(Duration::from_millis(10)).unwrap().is_none(), "no phantom client");
        assert!(lock.exists(), "the lock file is back");
        assert!(l.lock.still_ours(), "and held");
        assert_eq!((l.id, file_id(&path)), (id, Some(id)), "the socket was never touched");
        l.checked = Instant::now() - CHECK_EVERY;
        assert!(l.accept(Duration::from_millis(50)).unwrap().is_none(), "still no phantom client");
        assert!(Listener::bind(&name).err().unwrap().contains("already listening"), "the lock keeps a second host out");
    }

    /// The spec's 0600 on the socket file, whatever the umask (the 0700 folder is the boundary).
    #[test]
    fn the_socket_file_is_owner_only() {
        let name = test_name("mode");
        let l = Listener::bind(&name).unwrap();
        assert_eq!(std::fs::symlink_metadata(&l.path).unwrap().mode() & 0o777, 0o600);
    }

    #[test]
    fn a_stale_socket_is_replaced() {
        let name = test_name("stale");
        let path = runtime_dir().unwrap().join(&name);
        drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
        assert!(path.exists(), "a dead host leaves its file");
        let first = Listener::bind(&name).expect("the stale file is replaced");
        let again = Listener::bind(&name).err().expect("a live host is not replaced");
        assert!(again.contains("already listening"), "{again}");
        drop(first);
        assert!(!path.exists(), "a listener removes its own file");
    }

    #[test]
    fn a_dropped_listener_leaves_a_newer_socket() {
        let name = test_name("newer");
        let path = runtime_dir().unwrap().join(&name);
        let old = Listener::bind(&name).unwrap();
        std::fs::remove_file(&path).unwrap();
        let _newer = std::os::unix::net::UnixListener::bind(&path).unwrap();
        drop(old);
        assert!(path.exists(), "the newer host's socket stays");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_shared_runtime_folder_is_refused() {
        let open = scratch(0o777);
        let e = check_private(&open).unwrap_err();
        assert!(e.contains("is not a private folder of this user") && e.contains(&open.display().to_string()), "{e}");
        let private = scratch(0o700);
        check_private(&private).unwrap();
        let link = Scratch(std::env::temp_dir().join(test_name("link")));
        std::os::unix::fs::symlink(&*private, &*link).unwrap();
        assert!(check_private(&link).is_err(), "a link to a private folder is refused");
        let file = private.join("f");
        std::fs::write(&file, b"").unwrap();
        assert!(check_private(&file).is_err(), "a file is not a folder");
    }

    #[test]
    fn the_runtime_folder_is_private_and_a_planted_link_is_not_followed() {
        let dir = runtime_dir().unwrap();
        assert_eq!(std::fs::symlink_metadata(&dir).unwrap().mode() & 0o077, 0);
        // A link where a socket should be is refused, whatever it points to.
        let name = test_name("plant");
        let target = Scratch(std::env::temp_dir().join(test_name("target")));
        let _real = std::os::unix::net::UnixListener::bind(&*target).unwrap();
        let link = Scratch(dir.join(&name));
        std::os::unix::fs::symlink(&*target, &*link).unwrap();
        assert!(connect(&name, Duration::ZERO).is_err(), "connect does not follow a link");
        assert!(Listener::bind(&name).is_err(), "bind does not replace a link");
        assert!(std::fs::symlink_metadata(&*link).is_ok());
    }

    #[test]
    fn runtime_dir_follows_xdg_then_tmp() {
        let tmp = Path::new("/tmp");
        let mine = scratch(0o700);
        let uid = euid();
        let fallback = PathBuf::from(format!("/tmp/termurion-{uid}"));
        assert_eq!(runtime_dir_from(Some(&mine), tmp, uid), mine.join("termurion"));
        assert_eq!(runtime_dir_from(None, tmp, uid), fallback);
        assert_eq!(runtime_dir_from(Some(Path::new("run/user/1")), tmp, uid), fallback, "a relative folder is not trusted");
        assert_eq!(runtime_dir_from(Some(&mine.join("missing")), tmp, uid), fallback, "a missing folder is not trusted");
        let other = uid.wrapping_add(1);
        assert_eq!(runtime_dir_from(Some(&mine), Path::new("/var/t"), other), PathBuf::from(format!("/var/t/termurion-{other}")), "a folder of someone else is not trusted");
    }

    #[test]
    fn a_long_socket_path_is_refused() {
        let name = "x".repeat(120);
        let e = Listener::bind(&name).err().unwrap();
        assert!(e.contains("too long") && e.contains(&name), "{e}");
        assert!(connect(&name, Duration::ZERO).err().unwrap().contains("too long"));
    }

    #[test]
    fn shutdown_unblocks_a_reader() {
        let name = test_name("wake");
        let mut l = Listener::bind(&name).unwrap();
        let c = Arc::new(connect(&name, Duration::from_secs(1)).unwrap().unwrap());
        let _server = l.accept(Duration::from_secs(1)).unwrap().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let c2 = c.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4];
            tx.send(c2.read(&mut buf)).unwrap();
        });
        std::thread::sleep(Duration::from_millis(100));
        c.shutdown();
        assert_eq!(rx.recv_timeout(Duration::from_secs(1)).expect("the reader woke").unwrap(), 0);
    }

    #[test]
    fn the_unbuilt_servers_stay_unsupported() {
        assert!(elevated().is_err());
        assert!(user_sid().is_err());
        assert!(available().is_err());
        assert!(create_new_for_write("x", true).is_err());
        assert!(own_peer().is_err());
        assert!(thread_peer().is_err());
        assert!(secret_pipe_name().is_err());
        assert!(url_pipe_name().is_err());
    }

    #[test]
    fn the_host_socket_lives_in_the_runtime_folder() {
        let name = host_pipe_name().unwrap();
        assert_eq!(Path::new(&name), runtime_dir().unwrap().join("host.sock"));
    }
}

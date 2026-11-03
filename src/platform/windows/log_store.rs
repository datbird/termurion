//! Windows folders and files of the debug log: the log folders, the owner-only file that is never
//! written through a link, its cap and `.old` generation, and the lock every disk write takes.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::dlog::{could_not_open, write, Write3, EXIT_FOOTER, FILE_NAME, MAX_FILE, OLD_NAME};
use crate::localfs;

/// The folders a log may go in, best first: `%TEMP%` when the judge accepts it (a share is never
/// one: the log holds window facts and the connection would send the person's hash), then
/// `%LOCALAPPDATA%\Termurion\Logs`.
pub fn log_dirs_with(env: &dyn Fn(&str) -> Option<String>) -> Vec<localfs::LocalPath> {
    let mut out = Vec::new();
    if let Some(t) = env("TEMP").filter(|t| !t.is_empty()).and_then(localfs::LocalPath::judge) {
        out.push(t);
    }
    let logs = env("LOCALAPPDATA").filter(|t| !t.is_empty()).and_then(localfs::LocalPath::judge).and_then(|b| b.join("Termurion")).and_then(|b| b.join("Logs"));
    if let Some(logs) = logs {
        let _ = localfs::create_dir_all(&logs);
        out.push(logs);
    }
    out
}

pub fn log_dirs() -> Vec<localfs::LocalPath> {
    log_dirs_with(&|k| std::env::var(k).ok())
}

/// One lock for every disk write of the log: a save, the private state files and the removal of
/// the old file at start. Without it an off/on/off during a large save lets the UI thread delete the
/// file a worker is writing, and a second save interleaves with the first.
pub(crate) static SAVE: Mutex<()> = Mutex::new(());

/// How long the exit save waits for a save already running (the process is ending).
const EXIT_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

pub(crate) fn remove_old_files() {
    let _one = SAVE.lock().unwrap_or_else(|e| e.into_inner());
    for dir in log_dirs() {
        if let Some(f) = dir.unfollowed(FILE_NAME) {
            let _ = localfs::remove_unfollowed(&f);
        }
    }
}

/// The disk side of `write`: a new file is made owner-only and never through a link.
struct Disk {
    file: localfs::Unfollowed,
    /// The one handle every chunk goes through: the file `prepare` judged, or the one `create`
    /// made. The name is never opened again, so a swap after the judgment is not followed.
    handle: Option<std::fs::File>,
}

impl Write3 for Disk {
    fn create(&mut self, _: &Path) -> std::io::Result<()> {
        // An open handle on the old file would leave its delete pending and refuse CREATE_NEW.
        self.handle = None;
        match localfs::remove_unfollowed(&self.file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        self.handle = Some(localfs::create_owner_only(&self.file)?);
        Ok(())
    }

    fn append(&mut self, _: &Path, chunk: &[u8]) -> std::io::Result<()> {
        use std::io::Write;
        match self.handle.as_mut() {
            Some(f) => f.write_all(chunk),
            None => Err(std::io::Error::other("no file is open for the log")),
        }
    }
}

/// Whether an existing log file is this user's own owner-only file (`localfs::is_owner_only`): a
/// file with a wider inherited DACL may be read by others, so it is not appended to.
fn is_ours(f: &std::fs::File) -> bool {
    localfs::is_owner_only(f)
}

/// What `prepare` found at the log's name.
pub(crate) enum Prepared {
    /// No file (or one moved aside): a fresh one is made.
    Fresh,
    /// The user's own file, open for appending: the writes go through this handle.
    Append(std::fs::File),
}

/// Looks at the file before the write: whether to append to it. A file that is not ours, or a
/// link, is renamed away; one the new text would push past `cap` becomes the `.old` generation.
pub(crate) fn prepare(dir: &localfs::LocalPath, file: &localfs::Unfollowed, append: bool, incoming: u64, cap: u64) -> Result<Prepared, String> {
    let open = localfs::open_unfollowed(file, |o| {
        o.read(true).append(true);
    });
    let f = match open {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Prepared::Fresh),
        Err(_) => return aside(dir, file).map(|_| Prepared::Fresh),
    };
    if !is_ours(&f) {
        drop(f);
        return aside(dir, file).map(|_| Prepared::Fresh);
    }
    let len = localfs::file_stat(&f).map_or(0, |s| s.len);
    if append && len + incoming > cap {
        drop(f);
        let old = dir.unfollowed(OLD_NAME).ok_or_else(|| could_not_open(dir.path()))?;
        match localfs::remove_unfollowed(&old) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
        localfs::rename_unfollowed(file, &old).map_err(|e| e.to_string())?;
        return Ok(Prepared::Fresh);
    }
    Ok(Prepared::Append(f))
}

/// Moves an entry that is not ours to `debuglog.stale-<unix>.txt`.
fn aside(dir: &localfs::LocalPath, file: &localfs::Unfollowed) -> Result<(), String> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let to = dir.unfollowed(format!("debuglog.stale-{}-{}.txt", now.as_secs(), now.subsec_nanos())).ok_or_else(|| could_not_open(dir.path()))?;
    localfs::rename_unfollowed(file, &to).map_err(|e| e.to_string())
}

/// Saves into one folder; the file's path.
pub fn save_in(dir: &localfs::LocalPath, header: &str, body: &str, footer: &str, append: bool) -> Result<PathBuf, String> {
    save_in_capped(dir, header, body, footer, append, MAX_FILE)
}

pub fn save_in_capped(dir: &localfs::LocalPath, header: &str, body: &str, footer: &str, append: bool, cap: u64) -> Result<PathBuf, String> {
    let file = dir.unfollowed(FILE_NAME).ok_or_else(|| could_not_open(dir.path()))?;
    let (exists, handle) = match prepare(dir, &file, append, (header.len() + body.len() + footer.len()) as u64, cap)? {
        Prepared::Fresh => (false, None),
        Prepared::Append(f) => (true, Some(f)),
    };
    let mut disk = Disk { file: file.clone(), handle };
    write(file.path(), header, body, footer, append, exists, &mut disk)?;
    Ok(file.path().to_path_buf())
}

/// A small owner-only file in the first log folder that takes it (the state size files): a
/// plain `name`, replaced when it exists, never written through a link. The file's path.
pub fn save_private(name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    use std::io::Write;
    let _one = SAVE.lock().unwrap_or_else(|e| e.into_inner());
    let mut first_error = None;
    for dir in log_dirs() {
        let Some(file) = dir.unfollowed(name) else { continue };
        let made = match localfs::remove_unfollowed(&file) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
        .and_then(|()| localfs::create_owner_only(&file))
        .and_then(|mut f| f.write_all(bytes));
        match made {
            Ok(()) => return Ok(file.path().to_path_buf()),
            Err(e) => {
                first_error.get_or_insert(e.to_string());
            }
        }
    }
    Err(first_error.unwrap_or_else(|| could_not_open(Path::new(name))))
}

/// Saves into the first folder that takes it (iTerm2 reports the failure of its one folder).
pub fn save(header: &str, body: &str, footer: &str, append: bool) -> Result<PathBuf, String> {
    let _one = SAVE.lock().unwrap_or_else(|e| e.into_inner());
    save_locked(header, body, footer, append)
}

/// `save` for a caller that holds `SAVE`.
fn save_locked(header: &str, body: &str, footer: &str, append: bool) -> Result<PathBuf, String> {
    let mut first_error = None;
    for dir in log_dirs() {
        match save_in(&dir, header, body, footer, append) {
            Ok(p) => return Ok(p),
            Err(e) => {
                first_error.get_or_insert(e);
            }
        }
    }
    Err(first_error.unwrap_or_else(|| could_not_open(Path::new(FILE_NAME))))
}

/// The exit save: a save already running finishes first; give up after EXIT_WAIT (the process is
/// ending). `append` is asked after the wait, as the log's flag stands then.
pub fn save_at_exit(taken: Option<(String, String)>, append: &dyn Fn() -> bool) {
    if let Some((h, b)) = taken {
        // A save already running finishes first; give up after EXIT_WAIT (the process is ending).
        let start = std::time::Instant::now();
        let guard = loop {
            match SAVE.try_lock() {
                Ok(g) => break Some(g),
                Err(std::sync::TryLockError::Poisoned(p)) => break Some(p.into_inner()),
                Err(std::sync::TryLockError::WouldBlock) if start.elapsed() < EXIT_WAIT => std::thread::sleep(std::time::Duration::from_millis(10)),
                Err(std::sync::TryLockError::WouldBlock) => break None,
            }
        };
        if guard.is_some() {
            let _ = save_locked(&h, &b, EXIT_FOOTER, append());
        }
    }
}

pub fn debug_output(text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: NUL-terminated and alive for the call.
    unsafe { windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringW(wide.as_ptr()) };
}

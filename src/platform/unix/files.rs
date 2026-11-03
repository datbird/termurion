//! Unix twins of the file calls behind `localfs`. What has no Unix meaning yet says so.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::PathBuf;

use crate::localfs::{LocalPath, Unfollowed};

fn unsupported<T>(feature: &'static str) -> io::Result<T> {
    Err(io::Error::other(String::from(super::super::Unsupported { feature })))
}

pub fn open_unfollowed(_p: &Unfollowed, _configure: impl FnOnce(&mut OpenOptions)) -> io::Result<File> {
    unsupported("Opening a file without following a link")
}

/// Makes a NEW file at the entry, readable and writable by this user alone (mode 0600), refusing an
/// existing entry (a planted file or link) and never following a link at the name.
pub fn create_owner_only(p: &Unfollowed) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new().write(true).create_new(true).mode(0o600).custom_flags(libc::O_NOFOLLOW).open(p.path())
}

/// The owner-only access list is a Windows fact; nothing on Unix calls this.
pub fn is_owner_only(_f: &File) -> bool {
    false
}

pub fn link_count(_f: &File) -> io::Result<u32> {
    unsupported("Counting a file's names")
}

pub fn dacl_sddl(_p: &Unfollowed) -> io::Result<String> {
    unsupported("Reading an access list")
}

pub fn dacl_sddl_of(_f: &File) -> io::Result<String> {
    unsupported("Reading an access list")
}

pub fn final_path(_f: &File) -> Option<PathBuf> {
    None
}

/// The bytes free for the person on the file system holding a judged folder (`statvfs`).
pub fn free_space(dir: &LocalPath) -> io::Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(dir.path().as_os_str().as_bytes()).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(u64::from(st.f_bavail).saturating_mul(u64::from(st.f_frsize)))
}

pub fn rename_no_replace(from: &LocalPath, to: &LocalPath) -> io::Result<()> {
    if to.path().exists() {
        return Err(io::Error::from(io::ErrorKind::AlreadyExists));
    }
    std::fs::rename(from.path(), to.path())
}

pub fn watch_tree(_dir: &LocalPath, _filter: u32) -> Option<*mut std::ffi::c_void> {
    None
}

pub fn entry_is_reparse(m: &std::fs::Metadata) -> bool {
    m.file_type().is_symlink()
}

pub fn entry_is_hidden(_m: &std::fs::Metadata) -> bool {
    false
}

/// Only Windows has verbs: "open" is the default app, anything else is not offered.
pub fn shell_verb(verb: &str, p: &LocalPath, cwd: Option<&LocalPath>, background: bool) -> Result<(), String> {
    // The folder only matters to ShellExecute.
    let _ = cwd;
    if verb == "open" { crate::platform::shell_open::open_file(p.path(), background) } else { Err(crate::platform::Unsupported { feature: "Editing a file with its app" }.into()) }
}

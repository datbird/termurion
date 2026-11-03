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

/// Moves `from` to `to` in one step that fails when `to` exists, so nothing can appear at `to`
/// between a check and the move. A file system without that move gets a hard link, which is
/// just as exclusive, then the old name goes.
pub fn rename_no_replace(from: &LocalPath, to: &LocalPath) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c = |p: &LocalPath| std::ffi::CString::new(p.path().as_os_str().as_bytes()).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e));
    match exclusive_rename(&c(from)?, &c(to)?) {
        Err(e) if matches!(e.raw_os_error(), Some(libc::EINVAL | libc::ENOSYS | libc::ENOTSUP | libc::EOPNOTSUPP)) => {
            std::fs::hard_link(from.path(), to.path())?;
            std::fs::remove_file(from.path())
        }
        done => done,
    }
}

#[cfg(target_os = "linux")]
fn exclusive_rename(from: &std::ffi::CStr, to: &std::ffi::CStr) -> io::Result<()> {
    if unsafe { libc::renameat2(libc::AT_FDCWD, from.as_ptr(), libc::AT_FDCWD, to.as_ptr(), libc::RENAME_NOREPLACE) } == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

#[cfg(not(target_os = "linux"))]
fn exclusive_rename(from: &std::ffi::CStr, to: &std::ffi::CStr) -> io::Result<()> {
    if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) } == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The move never replaces: an existing name is refused and both files stay as they were.
    #[test]
    fn rename_no_replace_never_replaces() {
        let d = std::env::temp_dir().join(format!("termurion-rnr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let (a, b, c) = (d.join("a"), d.join("b"), d.join("c"));
        std::fs::write(&a, "A").unwrap();
        std::fs::write(&b, "B").unwrap();
        let judged = |p: &std::path::Path| LocalPath::judge(p).unwrap();
        let e = rename_no_replace(&judged(&a), &judged(&b)).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists, "{e}");
        assert_eq!((std::fs::read_to_string(&a).unwrap(), std::fs::read_to_string(&b).unwrap()), ("A".into(), "B".into()));
        rename_no_replace(&judged(&a), &judged(&c)).unwrap();
        assert!(!a.exists() && std::fs::read_to_string(&c).unwrap() == "A");
        let _ = std::fs::remove_dir_all(&d);
    }
}

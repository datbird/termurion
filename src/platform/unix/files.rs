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
    move_no_replace(from.path(), to.path(), &exclusive_rename_paths, &|a, b| std::fs::hard_link(a, b))
}

fn exclusive_rename_paths(from: &std::path::Path, to: &std::path::Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c = |p: &std::path::Path| std::ffi::CString::new(p.as_os_str().as_bytes()).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e));
    exclusive_rename(&c(from)?, &c(to)?)
}

type Mover<'a> = &'a dyn Fn(&std::path::Path, &std::path::Path) -> io::Result<()>;

/// `rename_no_replace` with the exclusive move and the hard link given (a test fakes them).
fn move_no_replace(from: &std::path::Path, to: &std::path::Path, exclusive: Mover, link: Mover) -> io::Result<()> {
    let unsupported = |e: &io::Error| matches!(e.raw_os_error(), Some(libc::EINVAL | libc::ENOSYS | libc::ENOTSUP | libc::EOPNOTSUPP));
    match exclusive(from, to) {
        Err(e) if unsupported(&e) => match link(from, to) {
            Ok(()) => std::fs::remove_file(from),
            // FAT and exFAT have no links either: the old check then rename, the best they allow.
            Err(e) if matches!(e.raw_os_error(), Some(libc::EPERM | libc::ENOTSUP | libc::EOPNOTSUPP | libc::EXDEV)) => {
                if std::fs::symlink_metadata(to).is_ok() {
                    return Err(io::Error::from(io::ErrorKind::AlreadyExists));
                }
                std::fs::rename(from, to)
            }
            Err(e) => Err(e),
        },
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

/// macOS mounts `/net/<host>` (the autofs hosts map) and `/Network` on first touch, so a path
/// there is a network path: probing it would contact that host. Linux has no such folders.
pub fn is_auto_mount(path: &str) -> bool {
    cfg!(target_os = "macos") && auto_mount_text(path)
}

/// `/net` or `/Network` (any case, as APFS reads it) as the first folder, after `.` and `..`
/// are read the way the path walks.
fn auto_mount_text(path: &str) -> bool {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    path.starts_with('/') && parts.first().is_some_and(|f| f.eq_ignore_ascii_case("net") || f.eq_ignore_ascii_case("network"))
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
    if verb != "open" {
        return Err(crate::platform::Unsupported { feature: "Editing a file with its app" }.into());
    }
    // open and xdg-open run programs: the open judge shows those, and hands the opener the
    // canonical path it judged (a link swapped after the judge no longer matters).
    match super::open_judge::judged_open(p.path()) {
        super::open_judge::Judged::Open(real) => crate::platform::shell_open::open_file(&real, background),
        super::open_judge::Judged::Reveal(shown) => crate::platform::shell_open::reveal(&shown.to_string_lossy()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// macOS: `/net` and `/Network` are network mounts, so the probe rule never touches them.
    #[test]
    fn auto_mount_folders_are_network_paths_on_macos() {
        let mac = cfg!(target_os = "macos");
        for p in ["/net/evil.example/x", "/Network/Servers/h/x", "/NET/h", "/net", "/a/../net/h/x", "//net/h"] {
            assert_eq!(is_auto_mount(p), mac, "{p}");
        }
        for p in ["/network-notes/x", "/home/u/net/x", "/netx/y"] {
            assert!(!is_auto_mount(p), "{p}");
        }
        assert_eq!(crate::netpath::may_probe("/net/evil.example/x", &crate::netpath::Probe::default()), !mac);
    }

    /// Every Unix open of a file (Semantic History, the Transfers menu) goes through the open
    /// judge, which shows a program and opens the canonical path it judged.
    #[test]
    fn opening_a_file_goes_through_the_open_judge() {
        let me = include_str!("files.rs");
        let body = &me[me.find("pub fn shell_verb(").unwrap()..];
        let body = &body[..body.find("\n}\n").unwrap()];
        assert!(body.contains("judged_open("), "{body}");
    }

    /// FAT and exFAT have neither the exclusive move nor hard links: the download still lands,
    /// through the old check then rename, and still never over an existing name.
    #[test]
    fn a_file_system_without_links_still_moves() {
        let d = std::env::temp_dir().join(format!("termurion-nolink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let (a, b, c) = (d.join("a"), d.join("b"), d.join("c"));
        std::fs::write(&a, "A").unwrap();
        std::fs::write(&b, "B").unwrap();
        let no_rename = |_: &std::path::Path, _: &std::path::Path| Err(io::Error::from_raw_os_error(libc::ENOTSUP));
        for code in [libc::EPERM, libc::ENOTSUP, libc::EXDEV] {
            let no_link = move |_: &std::path::Path, _: &std::path::Path| Err(io::Error::from_raw_os_error(code));
            assert_eq!(move_no_replace(&a, &b, &no_rename, &no_link).unwrap_err().kind(), io::ErrorKind::AlreadyExists, "{code}");
            assert_eq!(std::fs::read_to_string(&b).unwrap(), "B");
        }
        let no_link = |_: &std::path::Path, _: &std::path::Path| Err(io::Error::from_raw_os_error(libc::EPERM));
        move_no_replace(&a, &c, &no_rename, &no_link).unwrap();
        assert!(!a.exists() && std::fs::read_to_string(&c).unwrap() == "A");
        // Any other link error is the answer.
        std::fs::write(&a, "A").unwrap();
        let denied = |_: &std::path::Path, _: &std::path::Path| Err(io::Error::from_raw_os_error(libc::EACCES));
        assert_eq!(move_no_replace(&a, &d.join("e"), &no_rename, &denied).unwrap_err().raw_os_error(), Some(libc::EACCES));
        let _ = std::fs::remove_dir_all(&d);
    }

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

//! The OS calls behind `localfs`, the path gate: opening an entry without following a link, files only
//! the owner can read, link counts, access lists, free space, change watches, the facts of a folder
//! entry, and the shell's open verbs. The gate keeps the judging; the calls live here.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::PathBuf;

use crate::localfs::{LocalPath, Unfollowed};

#[cfg(unix)]
use super::unix::files as imp;
#[cfg(windows)]
use super::windows::files as imp;

/// Opens the entry itself with the options `configure` sets; the reparse point is never followed
/// (`FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS` are always set, last). Unix does not
/// open this way yet and says so.
pub fn open_unfollowed(p: &Unfollowed, configure: impl FnOnce(&mut OpenOptions)) -> io::Result<File> {
    imp::open_unfollowed(p, configure)
}

/// Makes a NEW file at the entry, readable and writable by this user alone, refusing an existing
/// entry and never following a link at the name.
pub fn create_owner_only(p: &Unfollowed) -> io::Result<File> {
    imp::create_owner_only(p)
}

/// Whether the open file is this user's own plain single-link file with the owner-only access list
/// `create_owner_only` makes. The handle was opened with read access. Unix has no such list: false.
pub fn is_owner_only(f: &File) -> bool {
    imp::is_owner_only(f)
}

/// How many names (hard links) the open file has.
pub fn link_count(f: &File) -> io::Result<u32> {
    imp::link_count(f)
}

/// The entry's access list as SDDL text (a test reads back what `create_owner_only` set).
pub fn dacl_sddl(p: &Unfollowed) -> io::Result<String> {
    imp::dacl_sddl(p)
}

/// An open file's access list as SDDL text (the handle was opened with read access).
pub fn dacl_sddl_of(f: &File) -> io::Result<String> {
    imp::dacl_sddl_of(f)
}

/// The path an OPEN handle really names (every link followed), so a check on it is a check on what a
/// read of that handle returns, whatever was swapped meanwhile. None when the system cannot say (and
/// on other hosts).
pub fn final_path(f: &File) -> Option<PathBuf> {
    imp::final_path(f)
}

/// The bytes free for the person on the drive or file system holding a judged folder.
pub fn free_space(dir: &LocalPath) -> io::Result<u64> {
    imp::free_space(dir)
}

/// A move that never replaces: `AlreadyExists` when `to` is there.
/// Whether `path` is a folder the OS mounts from the network on first touch (macOS `/net`).
pub fn is_auto_mount(path: &str) -> bool {
    imp::is_auto_mount(path)
}

pub fn rename_no_replace(from: &LocalPath, to: &LocalPath) -> io::Result<()> {
    imp::rename_no_replace(from, to)
}

/// A change notification on the whole tree under a judged folder, None when it cannot be watched
/// (and on Unix, which has no such handle).
pub fn watch_tree(dir: &LocalPath, filter: u32) -> Option<*mut std::ffi::c_void> {
    imp::watch_tree(dir, filter)
}

/// Whether the entry's metadata says reparse point (Windows) or symbolic link (Unix).
pub fn entry_is_reparse(m: &std::fs::Metadata) -> bool {
    imp::entry_is_reparse(m)
}

/// The hidden attribute of the entry's metadata (Windows); a leading dot is the caller's rule.
pub fn entry_is_hidden(m: &std::fs::Metadata) -> bool {
    imp::entry_is_hidden(m)
}

/// `verb` ("open", "edit") on a judged file or folder: ShellExecute on Windows, the default app
/// elsewhere (any other verb is refused). `cwd` only matters to ShellExecute.
pub fn shell_verb(verb: &str, p: &LocalPath, cwd: Option<&LocalPath>, background: bool) -> Result<(), String> {
    imp::shell_verb(verb, p, cwd, background)
}

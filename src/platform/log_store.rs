//! Where the debug log goes on disk: the log folders, the owner-only file, the private state files,
//! the removal of the old file at start, the save at exit, and the debug output. The log itself
//! (the ring, the entries, the header and footer) stays in `dlog`.

use std::path::PathBuf;

use crate::localfs::LocalPath;

#[cfg(unix)]
use super::unix::log_store as imp;
#[cfg(windows)]
use super::windows::log_store as imp;

/// The folders a log may go in, best first: `%TEMP%` when the judge accepts it (a share is never
/// one), then `%LOCALAPPDATA%\Termurion\Logs`. Unix has no log folder yet: none.
pub fn log_dirs_with(env: &dyn Fn(&str) -> Option<String>) -> Vec<LocalPath> {
    imp::log_dirs_with(env)
}

pub fn log_dirs() -> Vec<LocalPath> {
    imp::log_dirs()
}

/// Saves into the first folder that takes it; the file's path.
pub fn save(header: &str, body: &str, footer: &str, append: bool) -> Result<PathBuf, String> {
    imp::save(header, body, footer, append)
}

/// A small owner-only file in the first log folder that takes it (the state size files): a plain
/// `name`, replaced when it exists, never written through a link. The file's path.
pub fn save_private(name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    imp::save_private(name, bytes)
}

/// Starting a log without appending removes the old file first, under the same lock as a save.
pub fn remove_old_files() {
    imp::remove_old_files()
}

/// The exit save of a log taken at teardown (`taken`: header and body), waiting a little for a save
/// already running. `append` is asked after the wait.
pub fn save_at_exit(taken: Option<(String, String)>, append: &dyn Fn() -> bool) {
    imp::save_at_exit(taken, append)
}

/// The debug output (`OutputDebugStringW`); Unix has none, so the text goes nowhere.
pub fn debug_output(text: &str) {
    imp::debug_output(text)
}

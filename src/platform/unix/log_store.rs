//! Unix has no log folder yet: a save says so, and the rest has nothing to do.

use std::path::PathBuf;

use crate::localfs::LocalPath;

pub fn log_dirs_with(_env: &dyn Fn(&str) -> Option<String>) -> Vec<LocalPath> {
    Vec::new()
}

pub fn log_dirs() -> Vec<LocalPath> {
    Vec::new()
}

/// The debug log is saved by the Windows folder rules; Unix has no log folder yet.
pub fn save(_header: &str, _body: &str, _footer: &str, _append: bool) -> Result<PathBuf, String> {
    Err(crate::platform::Unsupported { feature: "Saving the debug log" }.to_string())
}

/// The debug log's private files are written by the Windows folder rules; Unix has no log folder yet.
pub fn save_private(_name: &str, _bytes: &[u8]) -> Result<PathBuf, String> {
    Err(crate::platform::Unsupported { feature: "Saving the debug log's files" }.to_string())
}

/// There is no old file to remove: nothing is ever saved.
pub fn remove_old_files() {}

/// Nothing is saved at exit, so the taken log is dropped.
pub fn save_at_exit(taken: Option<(String, String)>, _append: &dyn Fn() -> bool) {
    drop(taken);
}

/// The debug output is a Windows call; nothing on Unix reads it.
pub fn debug_output(_text: &str) {}

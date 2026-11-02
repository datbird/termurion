//! Where things live for this user. Only the home folder so far.

#[cfg(unix)]
use super::unix::paths as imp;
#[cfg(windows)]
use super::windows::paths as imp;

/// The person's home folder: `%USERPROFILE%` on Windows; on Unix `$HOME` when absolute, else the
/// password database's entry for the effective user. None when neither names one.
pub fn home_dir() -> Option<std::path::PathBuf> {
    imp::home_dir()
}

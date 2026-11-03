//! Where things live for this user: the home folder, the settings folder, the machine-local data
//! folder, and what a shell reports as a directory.

use std::path::PathBuf;

#[cfg(unix)]
use super::unix::paths as imp;
#[cfg(windows)]
use super::windows::paths as imp;

/// The app's folder name under the base: `Termurion` on Windows and macOS, `termurion` on Linux.
pub const APP_FOLDER: &str = imp::APP_FOLDER;

/// Whether launch may touch the Downloads folder. False on macOS, where that asks the person for
/// access before they have done anything; the first download sweeps it instead.
pub const SWEEP_DOWNLOADS_AT_LAUNCH: bool = imp::SWEEP_DOWNLOADS_AT_LAUNCH;

/// The person's home folder: `%USERPROFILE%` on Windows; on Unix `$HOME` when absolute, else the
/// password database's entry for the effective user. None when neither names one.
pub fn home_dir() -> Option<PathBuf> {
    imp::home_dir()
}

/// The folder settings live under: `%APPDATA%` on Windows, `~/Library/Application Support` on macOS,
/// `$XDG_CONFIG_HOME` (absolute) else `~/.config` on Linux. The temp folder when none is known.
pub fn config_base() -> PathBuf {
    imp::config_base()
}

/// The folder settings, profiles and saved windows live in.
pub fn config_dir() -> PathBuf {
    config_base().join(APP_FOLDER)
}

/// The machine-local base: `%LOCALAPPDATA%`, `~/Library/Caches`, or `$XDG_STATE_HOME` (absolute) else
/// `~/.local/state`. The temp folder when none is known.
pub fn local_base() -> PathBuf {
    imp::local_base()
}

/// The machine-local data folder (logs, caches, dumps).
pub fn local_dir() -> PathBuf {
    local_base().join(APP_FOLDER)
}

/// A directory a shell reported, as a local path: a Windows drive path (or its OSC 7 form) on
/// Windows, an absolute path on Unix. None for anything else (a remote or foreign path).
pub fn local_path(reported: &str) -> Option<PathBuf> {
    imp::local_path(reported)
}

/// A directory a Windows program reported, as a Windows path ("/C:/x" or "C:\\x"), and nothing else.
/// Where a remote host's folder must never be mistaken for one of this machine's: an ssh pane
/// reports its own paths, so on Unix, where every absolute path is local, none of them is. None
/// on Unix always.
pub fn windows_local_path(reported: &str) -> Option<PathBuf> {
    imp::windows_local_path(reported)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn config_folders_follow_each_os() {
        use super::super::unix::paths::{config_base_from, local_base_from};
        use std::path::Path;
        assert_eq!(config_base_from(Some("/x"), Some(Path::new("/h"))), PathBuf::from("/x"));
        assert_eq!(config_base_from(Some("rel"), Some(Path::new("/h"))), PathBuf::from("/h/.config"));
        assert_eq!(config_base_from(None, Some(Path::new("/h"))), PathBuf::from("/h/.config"));
        assert_eq!(config_base_from(None, None), std::env::temp_dir());
        assert_eq!(local_base_from(Some("/s"), Some(Path::new("/h"))), PathBuf::from("/s"));
        assert_eq!(local_base_from(Some(""), Some(Path::new("/h"))), PathBuf::from("/h/.local/state"));
        assert_eq!(local_base_from(None, None), std::env::temp_dir());
        assert_eq!(APP_FOLDER, "termurion");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn config_folders_follow_each_os() {
        use super::super::unix::paths::{config_base_from, local_base_from};
        use std::path::Path;
        assert_eq!(config_base_from(None, Some(Path::new("/h"))), PathBuf::from("/h/Library/Application Support"));
        assert_eq!(config_base_from(Some("/x"), Some(Path::new("/h"))), PathBuf::from("/h/Library/Application Support"), "XDG is ignored on macOS");
        assert_eq!(config_base_from(None, None), std::env::temp_dir());
        assert_eq!(local_base_from(Some("/s"), Some(Path::new("/h"))), PathBuf::from("/h/Library/Caches"), "XDG is ignored on macOS");
        assert_eq!(local_base_from(None, Some(Path::new("/h"))), PathBuf::from("/h/Library/Caches"));
        assert_eq!(local_base_from(None, None), std::env::temp_dir());
        assert_eq!(APP_FOLDER, "Termurion");
    }

    #[cfg(windows)]
    #[test]
    fn config_folders_follow_each_os() {
        let appdata = PathBuf::from(std::env::var_os("APPDATA").unwrap());
        assert_eq!(config_dir(), appdata.join("Termurion"));
        let local = PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap());
        assert_eq!(local_dir(), local.join("Termurion"));
    }

    #[cfg(unix)]
    #[test]
    fn nothing_is_a_windows_path_on_unix() {
        assert_eq!(windows_local_path("/C:/x"), None);
        assert_eq!(windows_local_path(r"C:\x"), None);
        assert_eq!(windows_local_path("/home/u"), None);
    }

    #[cfg(unix)]
    #[test]
    fn local_path_follows_the_os() {
        assert_eq!(local_path("/home/rob"), Some(PathBuf::from("/home/rob")));
        assert_eq!(local_path(r"C:\x"), None);
        assert_eq!(local_path("rel"), None);
    }

    #[cfg(windows)]
    #[test]
    fn local_path_follows_the_os() {
        assert_eq!(local_path("/C:/Users/rob"), Some(PathBuf::from(r"C:\Users\rob")));
        assert_eq!(local_path(r"C:\Users\rob"), Some(PathBuf::from(r"C:\Users\rob")));
        assert_eq!(local_path("C:/x/y"), Some(PathBuf::from(r"C:\x\y")));
        assert_eq!(local_path("/home/rob"), None);
        assert_eq!(local_path("rel"), None);
    }
}

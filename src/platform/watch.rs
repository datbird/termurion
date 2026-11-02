//! Folder watchers: the Dynamic Profiles folder and the Scripts folder. Windows waits on change
//! notifications from a worker thread; Unix has no watcher yet.

#[cfg(unix)]
use super::unix::watch as imp;
#[cfg(windows)]
use super::windows::watch as imp;

pub use imp::{stop_all, DynamicScanner, ScanEvent, ScriptsScanner};

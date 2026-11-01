//! The release feed fetch. Windows runs `curl.exe` from System32 in a job; Unix has no fetch yet, so a
//! check ends at once with the Unsupported text.

#[cfg(unix)]
use super::unix::update_fetch as imp;
#[cfg(windows)]
use super::windows::update_fetch as imp;

pub use imp::{Checker, CurlFetch, cancel_running};

//! The release feed fetch: curl with Termurion's own fixed arguments. Windows runs `curl.exe` from
//! System32 in a job; Unix runs the system curl in a process group of its own.

#[cfg(unix)]
use super::unix::update_fetch as imp;
#[cfg(windows)]
use super::windows::update_fetch as imp;

pub use imp::{Checker, CurlFetch, cancel_running};
#[cfg(test)]
pub(crate) use imp::{fetch_with, test_curl};

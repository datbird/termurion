//! The Windows release feed fetch.

pub use crate::update_fetch_win::{Checker, CurlFetch, cancel_running};
#[cfg(test)]
pub(crate) use crate::update_fetch_win::{fetch_with, test_curl};

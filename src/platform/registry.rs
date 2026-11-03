//! The per-user registry: Windows writes `HKEY_CURRENT_USER`; Unix has none and says so.

#[cfg(unix)]
use super::unix::registry as imp;
#[cfg(windows)]
use super::windows::registry as imp;

pub use imp::HkcuRegistry;

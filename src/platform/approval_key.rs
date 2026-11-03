//! The install's approval key at rest: Windows protects it with DPAPI for this user; Unix keeps no
//! key file yet, so every import asks.

#[cfg(unix)]
use super::unix::approval_key as imp;
#[cfg(windows)]
use super::windows::approval_key as imp;

pub use imp::{key_in, protect_key, unprotect_key};
pub use imp::dpapi_with;

//! Asking the person to prove it is them before the password manager opens: Windows Hello, or the
//! system credential prompt.

#[cfg(unix)]
use super::unix::user_auth as imp;
#[cfg(windows)]
use super::windows::user_auth as imp;

pub use imp::{hello, prompt};

pub enum Answer {
    Granted,
    Canceled,
    /// The reason, finishing "Authentication failed because ".
    Failed(String),
}

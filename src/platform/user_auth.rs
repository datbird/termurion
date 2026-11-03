//! Asking the person to prove it is them before the password manager opens: Windows Hello, or the
//! system credential prompt.

#[cfg(target_os = "macos")]
use super::macos::user_auth as imp;
#[cfg(all(unix, not(target_os = "macos")))]
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

/// Whether a system or app cancel of the prompt shows no alert (iTerm2 on macOS; Windows keeps its alert).
#[cfg(target_os = "macos")]
pub const SILENT_CANCEL: bool = true;
#[cfg(not(target_os = "macos"))]
pub const SILENT_CANCEL: bool = false;

//! Asking the person to prove it is them before the password manager opens: Windows Hello, the
//! system credential prompt, Touch ID on macOS, or the account password checked by PAM on Linux.

#[cfg(target_os = "macos")]
use super::macos::user_auth as imp;
#[cfg(target_os = "linux")]
use super::linux::user_auth as imp;
#[cfg(windows)]
use super::windows::user_auth as imp;

pub use imp::{hello, prompt};
#[cfg(target_os = "linux")]
pub use imp::{account_name, check, ready};

/// Without the in-app gate there is no name to ask for and nothing to check.
#[cfg(not(target_os = "linux"))]
pub fn account_name() -> Result<String, String> {
    Err(format!("the account password is not asked for here ({}).", super::OS_NAME))
}

/// See `account_name`.
#[cfg(not(target_os = "linux"))]
pub fn ready() -> Result<(), String> {
    Err(format!("the account password is not checked here ({}).", super::OS_NAME))
}

/// See `account_name`.
#[cfg(not(target_os = "linux"))]
pub fn check(_user: &str, _password: &crate::pwm::Secret) -> Result<(), String> {
    Err(format!("the account password is not checked here ({}).", super::OS_NAME))
}

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

/// Whether the app asks for the password in its own window and checks it off the UI thread (Linux),
/// instead of the system showing a prompt that waits (Windows, macOS).
#[cfg(target_os = "linux")]
pub const GATE_IN_APP: bool = true;
#[cfg(not(target_os = "linux"))]
pub const GATE_IN_APP: bool = false;

/// Wrong answers allowed before the in-app gate fails.
pub const TRIES: u8 = 3;
pub const THREE_WRONG: &str = "three wrong passwords";
/// The reason for a wrong password (Windows says the same).
pub const WRONG: &str = "valid credentials weren't supplied.";

/// What the gate does after an answer.
pub enum Next {
    /// Ask again.
    Again,
    Done(Answer),
}

/// Counts the wrong answers: three fail the gate. Any other failure (no PAM, no service) ends it at once.
#[derive(Default)]
pub struct Gate {
    wrong: u8,
}

impl Gate {
    pub fn answer(&mut self, checked: Result<(), String>) -> Next {
        match checked {
            Ok(()) => Next::Done(Answer::Granted),
            Err(e) if e == WRONG => {
                self.wrong += 1;
                if self.wrong >= TRIES { Next::Done(Answer::Failed(THREE_WRONG.to_string())) } else { Next::Again }
            }
            Err(e) => Next::Done(Answer::Failed(e)),
        }
    }
}

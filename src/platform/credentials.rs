//! The system credential store: Windows Credential Manager. Unix has none yet, so every call reports
//! Unsupported (never an empty answer). The generic-target calls serve the AI keys (`ai_keys`) and
//! refuse any target outside Termurion's prefixes.

use crate::pwm::Secret;

#[cfg(unix)]
use super::unix::credentials as imp;
#[cfg(windows)]
use super::windows::credentials as imp;

pub use imp::CredentialManager;

/// Reads a generic credential's blob as UTF-8 text; `Ok(None)` when there is no such
/// credential. `what` names the secret in the not-UTF-8 text ("password", "API key").
pub fn read(target: &str, what: &str) -> Result<Option<Secret>, String> {
    imp::read_generic(target, what)
}

/// Writes (creates or replaces) a generic credential. `what` names the secret in the too-long text.
pub fn write(target: &str, user: &str, secret: &Secret, comment: &str, what: &str) -> Result<(), String> {
    imp::write_generic(target, user, secret, comment, what)
}

/// Deletes a generic credential: `Ok(true)` when one was deleted, `Ok(false)` when there was none.
pub fn delete(target: &str) -> Result<bool, String> {
    imp::delete_generic(target)
}

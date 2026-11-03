//! The system credential store: Windows Credential Manager, or the macOS Keychain. Linux has none yet,
//! so every call reports Unsupported (never an empty answer). The generic-target calls serve the AI keys (`ai_keys`) and
//! refuse any target outside Termurion's prefixes.

use crate::pwm::Secret;

#[cfg(target_os = "macos")]
use super::macos::keychain as imp;
#[cfg(all(unix, not(target_os = "macos")))]
use super::unix::credentials as imp;
#[cfg(windows)]
use super::windows::credentials as imp;

pub use imp::CredentialManager;

/// The gear menu's name for the system store (iTerm2's `iTermPasswordManager.xib` says "macOS Keychain").
#[cfg(target_os = "macos")]
pub const STORE_MENU_TITLE: &str = "macOS Keychain";
#[cfg(not(target_os = "macos"))]
pub const STORE_MENU_TITLE: &str = "Windows Credential Manager";

/// The store's short name in the "query failed" alert.
#[cfg(target_os = "macos")]
pub const STORE_QUERY_NAME: &str = "Keychain";
#[cfg(not(target_os = "macos"))]
pub const STORE_QUERY_NAME: &str = "Credential Manager";

/// The prefixes of every target Termurion owns: the password manager's `Termurion/` and
/// `Termurion-Browser/` and the AI keys' `TermurionAI/` (compared case-sensitively).
const OWNED_PREFIXES: [&str; 3] = ["Termurion/", "Termurion-Browser/", "TermurionAI/"];

/// Whether a target is one of Termurion's. Every generic call refuses any other target before
/// it reaches the system store, so nothing here ever touches another program's credential.
pub(crate) fn owned(target: &str) -> bool {
    OWNED_PREFIXES.iter().any(|p| target.starts_with(p))
}

/// The refusal for a target that is not Termurion's.
#[cfg(any(windows, target_os = "macos"))]
pub(crate) fn guard(target: &str) -> Result<(), String> {
    if owned(target) {
        Ok(())
    } else {
        Err("Not a Termurion credential.".to_string())
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_guard_for_every_store() {
        assert!(owned("TermurionAI/x"));
        assert!(owned("Termurion/site"));
        assert!(owned("Termurion-Browser/https://x.y"));
        assert!(!owned("termurionai/x"));
        assert!(!owned("iTerm2"));
        assert!(!owned("Termurion-Browse/x"));
    }
}

//! iTerm2's own settings on this machine. Only macOS has them: iTerm2 keeps them in the
//! `com.googlecode.iterm2` preferences domain, which is read live (the plist file on disk can lag).

use std::path::PathBuf;

#[cfg(unix)]
use super::unix::iterm2_prefs as imp;
#[cfg(windows)]
use super::windows::iterm2_prefs as imp;

/// iTerm2's preferences domain.
pub const ITERM2_DOMAIN: &str = "com.googlecode.iterm2";

/// iTerm2's settings file, where the Mac keeps it. None where iTerm2 does not run.
pub fn live_file() -> Option<PathBuf> {
    imp::live_file()
}

/// The domain's keys as binary plist bytes. `Ok(None)` for a domain with no keys.
pub fn read_domain(domain: &str) -> Result<Option<Vec<u8>>, String> {
    imp::read_domain(domain)
}

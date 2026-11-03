//! Linux has no iTerm2; macOS reads its live domain (`platform::macos::iterm2_prefs`).

use std::path::PathBuf;

#[cfg(not(target_os = "macos"))]
pub fn live_file() -> Option<PathBuf> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn read_domain(_domain: &str) -> Result<Option<Vec<u8>>, String> {
    Ok(None)
}

#[cfg(target_os = "macos")]
pub fn live_file() -> Option<PathBuf> {
    super::super::macos::iterm2_prefs::live_file()
}

#[cfg(target_os = "macos")]
pub fn read_domain(domain: &str) -> Result<Option<Vec<u8>>, String> {
    super::super::macos::iterm2_prefs::read_domain(domain)
}

//! No download quarantine mark on Linux; macOS has its own (`platform::macos::quarantine`).

use std::path::Path;

#[cfg(not(target_os = "macos"))]
pub fn mark(_path: &Path, _text: &str) -> std::io::Result<()> {
    Err(std::io::Error::other(super::super::Unsupported { feature: "Download quarantine", plural: false }.to_string()))
}

#[cfg(target_os = "macos")]
pub fn mark(path: &Path, text: &str) -> std::io::Result<()> {
    super::super::macos::quarantine::mark(path, text)
}

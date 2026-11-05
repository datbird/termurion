//! Picks the Unix mark: macOS has the quarantine attribute, Linux the freedesktop origin attributes.

use std::path::Path;

pub fn mark(path: &Path, text: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    return super::super::macos::quarantine::mark(path, text);
    #[cfg(not(target_os = "macos"))]
    super::super::linux::quarantine::mark(path, text)
}

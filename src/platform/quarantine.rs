//! The "from the internet" mark on a downloaded file. Windows writes the Zone.Identifier stream; Unix has
//! no mark yet (macOS's quarantine attribute is a later step), so it reports Unsupported.

use std::path::Path;

#[cfg(unix)]
use super::unix::quarantine as imp;
#[cfg(windows)]
use super::windows::quarantine as imp;

/// Marks the existing file `path` with `text` (the zone file's contents). Err when it cannot.
pub fn mark(path: &Path, text: &str) -> std::io::Result<()> {
    imp::mark(path, text)
}

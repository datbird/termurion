//! iTerm2 runs only on macOS, so there is nothing live to read.

use std::path::PathBuf;

pub fn live_file() -> Option<PathBuf> {
    None
}

pub fn read_domain(_domain: &str) -> Result<Option<Vec<u8>>, String> {
    Ok(None)
}

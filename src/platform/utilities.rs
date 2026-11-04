//! Putting the command line utilities beside the program: one `.exe` link per utility name.

use std::path::Path;

use crate::utilities::Ensured;

#[cfg(unix)]
use super::unix::utilities as imp;
#[cfg(windows)]
use super::windows::utilities as imp;

/// Makes `exe_dir\utilities` and brings one `<name>.exe` per `util_cli::NAMES` entry up to date with
/// `exe_dir\termurion-utils.exe` (see the Windows backend). Unix does not put them on PATH yet.
pub fn ensure(exe_dir: &Path) -> Result<Ensured, String> {
    imp::ensure(exe_dir)
}

/// Whether this OS keeps a utilities folder beside the program. Unix has none, so a start there
/// neither makes one nor says anything about it.
pub const HAS_FOLDER: bool = cfg!(windows);

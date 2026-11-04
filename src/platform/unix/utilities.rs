//! The utilities are `.exe` hard links beside the Windows build; Unix does not put them on PATH yet.

use std::path::Path;

use crate::utilities::Ensured;

pub fn ensure(_exe_dir: &Path) -> Result<Ensured, String> {
    Err(crate::platform::Unsupported { feature: "The command line utilities", plural: true }.to_string())
}

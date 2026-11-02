//! No download quarantine mark on Unix yet.

use std::path::Path;

pub fn mark(_path: &Path, _text: &str) -> std::io::Result<()> {
    Err(std::io::Error::other(super::super::Unsupported { feature: "Download quarantine" }.to_string()))
}

//! The Zone.Identifier stream.

use std::path::Path;

/// `path`'s Zone.Identifier stream, judged.
fn zone_stream(path: &Path) -> std::io::Result<crate::localfs::LocalPath> {
    let mut stream = path.as_os_str().to_os_string();
    stream.push(":Zone.Identifier");
    crate::localfs::judge_io(std::path::PathBuf::from(stream))
}

pub fn mark(path: &Path, text: &str) -> std::io::Result<()> {
    crate::localfs::write(&zone_stream(path)?, text)
}

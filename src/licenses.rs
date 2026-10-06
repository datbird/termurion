//! The Open Source Licenses list (`AD:3735-3741`; iTerm2's `ThirdParty/Licenses.txt` is each
//! package's `name/FILE` header followed by the text). `build.rs` gathers the license files of
//! every crate in Cargo.lock, the three path crates and the ConPTY package into
//! `OUT_DIR/licenses.txt` at build time; nothing is fetched at run time.

/// One list entry: "{crate} {version}/{file}", the text, and a blank line.
pub fn entry(crate_name: &str, version: &str, file: &str, text: &str) -> String {
    format!("{crate_name} {version}/{file}\n{text}\n\n")
}

/// The whole list, as `build.rs` wrote it.
pub fn text() -> &'static str {
    include_str!(concat!(env!("OUT_DIR"), "/licenses.txt"))
}

/// The file name Open Source Licenses opens.
pub const FILE_NAME: &str = "Tandem Licenses.txt";

/// Writes the list to `dir\Tandem Licenses.txt` and returns the path (P17): the folder is
/// Tandem's own (`%LOCALAPPDATA%\Tandem`), not the predictable temp folder; the text goes to a
/// fresh temp file in it (`create_new`, never through an existing link) and is renamed over the
/// final name; a link at the final name is refused.
pub fn write_to(dir: &std::path::Path, text: &str) -> std::io::Result<std::path::PathBuf> {
    use std::io::Write;
    std::fs::create_dir_all(dir)?;
    let target = dir.join(FILE_NAME);
    if std::fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "the license file name is a link"));
    }
    let tmp = dir.join(format!("{FILE_NAME}.{}.tmp", std::process::id()));
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
    let done = f.write_all(text.as_bytes()).and_then(|_| f.sync_all());
    drop(f);
    match done.and_then(|_| std::fs::rename(&tmp, &target)) {
        Ok(()) => Ok(target),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_file_is_written_whole_and_replaced() {
        let dir = std::env::temp_dir().join(format!("tandem-licenses-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = write_to(&dir, "first").unwrap();
        assert_eq!(p, dir.join("Tandem Licenses.txt"));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "first");
        write_to(&dir, "second").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "second");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temp file is left");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every direct dependency in Cargo.toml has its license in the list.
    #[test]
    fn every_dependency_is_listed() {
        let manifest: toml::Table = toml::from_str(include_str!("../Cargo.toml")).unwrap();
        let mut names = Vec::new();
        for table in std::iter::once(&manifest).chain(manifest.get("target").and_then(|t| t.as_table()).into_iter().flat_map(|t| t.values().filter_map(|v| v.as_table()))) {
            if let Some(d) = table.get("dependencies").and_then(|d| d.as_table()) {
                names.extend(d.keys().cloned());
            }
        }
        assert!(names.len() > 10, "the scan found {} dependencies", names.len());
        let all = text();
        let lock = include_str!("../Cargo.lock");
        for n in names {
            // Only crates Cargo.lock names are in the build.
            if lock.contains(&format!("name = \"{n}\"")) {
                assert!(all.contains(&format!("\n{n} ")) || all.starts_with(&format!("{n} ")), "{n} has no license entry");
            }
        }
    }

    /// `AD:3735-3741`, `ThirdParty/Licenses.txt`: a header line, then the text.
    #[test]
    fn license_entries_follow_iterm2() {
        assert_eq!(entry("serde", "1.0.1", "LICENSE-MIT", "text"), "serde 1.0.1/LICENSE-MIT\ntext\n\n");
        let all = text();
        assert!(!all.is_empty());
        assert!(all.contains("alacritty_terminal"), "the vendored terminal crate is listed");
        assert!(all.contains("glyphon") && all.contains("vte "), "the other path crates are listed");
        assert!(all.contains("Microsoft.Windows.Console.ConPTY"), "the shipped ConPTY files are covered");
    }
}

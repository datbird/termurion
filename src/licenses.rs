//! The Open Source Licenses list (`AD:3735-3741`; iTerm2's `ThirdParty/Licenses.txt` is each
//! package's `name/FILE` header followed by the text). `build.rs` gathers the license files of
//! every crate in Cargo.lock, the three path crates and every notice file under resources/ (iTerm2, tab icons, terminfo, ConPTY) into
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
pub const FILE_NAME: &str = "Termurion Licenses.txt";

/// Writes the list to `dir\Termurion Licenses.txt` and returns the path (P17): the folder is
/// Termurion's own (`%LOCALAPPDATA%\Termurion`), not the predictable temp folder; the text goes to a
/// fresh temp file in it (`create_new`, never through an existing link) and is renamed over the
/// final name; a link at the final name is refused.
pub fn write_to(dir: &crate::localfs::LocalPath, text: &str) -> std::io::Result<crate::localfs::LocalPath> {
    use crate::localfs;
    use std::io::Write;
    localfs::create_dir_all(dir)?;
    // Each name is an entry of the judged folder (13v1's one path judge): a link there is judged whole.
    let target = dir.child(FILE_NAME).ok_or_else(localfs::refused)?;
    if localfs::lstat(&target).is_ok_and(|s| s.is_link || s.is_reparse) {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "the license file name is a link"));
    }
    let tmp = dir.child(format!("{FILE_NAME}.{}.tmp", std::process::id())).ok_or_else(localfs::refused)?;
    // A leftover of this exact name (a crashed run with the same pid) is removed first.
    if localfs::lstat(&tmp).is_ok() {
        localfs::remove_file(&tmp)?;
    }
    let mut f = localfs::open_custom(&tmp, |o| {
        o.write(true).create_new(true);
    })?;
    let done = f.write_all(text.as_bytes()).and_then(|_| f.sync_all());
    drop(f);
    match done.and_then(|_| localfs::rename(&tmp, &target)) {
        Ok(()) => Ok(target),
        Err(e) => {
            let _ = localfs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    /// Final review 12: a stale temp file with this process's name (a crashed run with the same
    /// pid) does not stop the file from being written.
    #[test]
    fn a_stale_temp_file_does_not_block_the_write() {
        let dir = std::env::temp_dir().join(format!("termurion-licenses-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{FILE_NAME}.{}.tmp", std::process::id())), "left over").unwrap();
        let judged = crate::localfs::LocalPath::judge(&dir).unwrap();
        let p = write_to(&judged, "fresh").unwrap();
        assert_eq!(std::fs::read_to_string(p.path()).unwrap(), "fresh");
        let _ = std::fs::remove_dir_all(&dir);
    }

    use super::*;

    #[test]
    fn the_list_file_is_written_whole_and_replaced() {
        let dir = std::env::temp_dir().join(format!("termurion-licenses-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let judged = crate::localfs::LocalPath::judge(&dir).unwrap();
        let p = write_to(&judged, "first").unwrap();
        assert_eq!(p.path(), dir.join("Termurion Licenses.txt"));
        assert_eq!(std::fs::read_to_string(p.path()).unwrap(), "first");
        write_to(&judged, "second").unwrap();
        assert_eq!(std::fs::read_to_string(p.path()).unwrap(), "second");
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
        assert!(all.contains("resources/conpty/LICENSE") && all.contains("resources/conpty/NOTICE.md"), "the shipped ConPTY files are covered");
    }

    /// How a bundled file under `resources/` is covered.
    enum Cover {
        /// A license file in the file's own folder (or one above it, below `resources/`).
        Folder,
        /// A notice at the top of `resources/`, which says which files it covers.
        Root,
        /// Termurion's own file.
        Own(&'static str),
    }

    const RESOURCES: &[(&str, Cover)] = &[
        ("browser/", Cover::Folder),
        ("render_plugins/", Cover::Folder),
        ("ssh/", Cover::Folder),
        ("utilities/", Cover::Folder),
        ("tab_icons/", Cover::Folder),
        ("terminfo/", Cover::Folder),
        ("conpty/", Cover::Folder),
        ("ai/", Cover::Root),
        ("scripts/", Cover::Root),
        ("ColorPresets.plist", Cover::Root),
        ("tips.toml", Cover::Root),
        ("code-review-settings.txt", Cover::Root),
        ("shell_integration/termurion_shell_integration.ps1", Cover::Own("Termurion's own PowerShell script")),
    ];

    fn is_notice(name: &str) -> bool {
        let n = name.to_ascii_uppercase();
        ["LICENSE", "LICENCE", "COPYING", "NOTICE"].iter().any(|p| n.starts_with(p))
    }

    fn walk(dir: &std::path::Path, base: &std::path::Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, base, out);
            } else {
                out.push(p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/"));
            }
        }
    }

    /// The class rule (m13v2 Task 7 fix): every file bundled from `resources/` is covered by a
    /// notice that the list carries, or is declared Termurion's own. A new file with neither fails.
    #[test]
    fn every_bundled_file_has_a_notice() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources");
        let mut files = Vec::new();
        walk(&base, &base, &mut files);
        assert!(files.len() > 100, "the walk found {} files", files.len());
        let all = text();
        let mut bad = Vec::new();
        for f in &files {
            let name = f.rsplit('/').next().unwrap();
            if is_notice(name) {
                continue;
            }
            let Some((_, cover)) = RESOURCES.iter().find(|(p, _)| f.starts_with(p)) else {
                bad.push(format!("{f}: no rule"));
                continue;
            };
            let dir: Vec<&str> = f.split('/').collect();
            let notices: Vec<String> = match cover {
                Cover::Own(why) => {
                    assert!(!why.is_empty());
                    continue;
                }
                Cover::Root => (0..1).flat_map(|_| ["LICENSE", "COPYING-iTerm2", "LICENSE-iTerm2"]).map(String::from).collect(),
                Cover::Folder => (1..dir.len())
                    .rev()
                    .flat_map(|n| {
                        let folder = dir[..n].join("/");
                        std::fs::read_dir(base.join(&folder)).unwrap().flatten().filter_map(move |e| e.file_name().into_string().ok()).filter(|n| is_notice(n)).map(move |n| format!("{folder}/{n}"))
                    })
                    .collect(),
            };
            if notices.is_empty() {
                bad.push(format!("{f}: no notice file covers it"));
            }
            for n in notices {
                let path = format!("resources/{n}");
                if !base.join(&n).is_file() {
                    bad.push(format!("{f}: {n} is missing"));
                } else if !all.contains(&path) {
                    bad.push(format!("{f}: {path} is not in the list"));
                }
            }
        }
        bad.sort();
        bad.dedup();
        assert!(bad.is_empty(), "bundled files without a notice in the list:\n{}", bad.join("\n"));
    }

    /// The files the installer copies are Termurion's own binaries or the ConPTY pair, whose notice is in the list.
    #[test]
    fn every_installed_file_has_a_notice() {
        let ps = include_str!("../scripts/install-windows.ps1");
        let line = ps.lines().find(|l| l.starts_with("$files = ")).expect("the file list");
        let names: Vec<&str> = line.trim_start_matches("$files = ").split(',').map(|n| n.trim().trim_matches('\'')).collect();
        assert!(names.len() >= 7, "{names:?}");
        for n in names {
            match n {
                "termurion.exe" | "termurion-host.exe" | "termurion-git.exe" | "termurion-utils.exe" | "cc-status.exe" => {}
                "conpty.dll" | "OpenConsole.exe" => assert!(text().contains("resources/conpty/NOTICE.md"), "{n}"),
                other => panic!("{other} is installed with no notice: add it to the list or declare it Termurion's own"),
            }
        }
    }
}

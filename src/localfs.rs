//! The one gate before Tandem touches a path that came from outside: a file or folder another
//! program named, a document, a saved window or arrangement, a setting. Even `is_dir` on a share
//! path connects to it (and sends the person's credentials), so the path is judged first, by
//! `scripts::stays_local` (the text rule, device names, mapped and SUBST drives, every folder from
//! the root down and every link target's folders, root first), and only a path that passes reaches
//! the filesystem. A refused path answers as a missing one. Tandem's own config paths (under
//! `%APPDATA%\Tandem`, its own exe) need no judge; the source test below lists every other site.

use std::fs::{File, Metadata, ReadDir};
use std::io;
use std::path::Path;

fn refused() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "Tandem does not open files on network locations.")
}

/// Whether `p` may be touched at all.
pub fn judged(p: &Path) -> bool {
    crate::scripts::stays_local(p)
}

pub fn is_dir(p: &Path) -> bool {
    judged(p) && p.is_dir()
}

pub fn is_file(p: &Path) -> bool {
    judged(p) && p.is_file()
}

pub fn exists(p: &Path) -> bool {
    judged(p) && p.exists()
}

/// The entry's own metadata (a link is not followed), after the judge.
pub fn symlink_metadata(p: &Path) -> io::Result<Metadata> {
    if judged(p) { std::fs::symlink_metadata(p) } else { Err(refused()) }
}

pub fn metadata(p: &Path) -> io::Result<Metadata> {
    if judged(p) { std::fs::metadata(p) } else { Err(refused()) }
}

pub fn open(p: &Path) -> io::Result<File> {
    if judged(p) { File::open(p) } else { Err(refused()) }
}

pub fn read_dir(p: &Path) -> io::Result<ReadDir> {
    if judged(p) { std::fs::read_dir(p) } else { Err(refused()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_path_is_never_touched() {
        for p in [r"\\attacker\share\f", r"//attacker/share/f", r"\\?\UNC\h\s\f", r"C:\x\NUL", "C:\\x\\COM\u{b9}"] {
            let p = Path::new(p);
            assert!(!is_dir(p) && !is_file(p) && !exists(p), "{p:?}");
            assert_eq!(metadata(p).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
            assert!(symlink_metadata(p).is_err() && open(p).is_err() && read_dir(p).is_err());
        }
        assert!(is_dir(&std::env::temp_dir()) || !std::env::temp_dir().is_dir());
    }

    /// Probe calls that may stay: the function is judged before the call, or the path is Tandem's
    /// own (config, scripts, runtime, staging) or chosen by the person in a dialog for this very
    /// action. Every other site fails the test: a new `is_dir`, `exists`, `metadata` or open in
    /// `src/app` or `start_dir.rs` goes through `localfs` (or the judge it calls), or is listed
    /// here with the reason.
    const OWN: &[(&str, &str, &str)] = &[
        ("api_ui.rs", "api_launch", "Tandem's own consent file under %APPDATA%"),
        ("claude_code_app.rs", "install_hook", "the hook exe beside Tandem's own exe"),
        ("mod.rs", "default_shell", "the person's own PATH entries looking for pwsh.exe"),
        ("mod.rs", "run", "Tandem's own uv folder under %APPDATA%"),
        ("pwm_app.rs", "build", "the person's own PATH entries looking for op.exe"),
        ("semantic_ui.rs", "has_code", "the person's own PATH entries looking for code.exe"),
        ("script_runtime_ui.rs", "find_backend", "Tandem's own uv and runtime folders under %APPDATA%"),
        ("script_runtime_ui.rs", "ensure_environment", "Tandem's own uv and environment folders under %APPDATA%"),
        ("script_runtime_ui.rs", "install_python_runtime", "Tandem's own uv folder under %APPDATA%"),
        ("script_runtime_ui.rs", "provision_done", "Tandem's own uv folder under %APPDATA%"),
        ("scripts_app.rs", "spawn_script", "the script's own virtual environment under Tandem's runtime folder"),
        ("scripts_app.rs", "remove_path", "an entry of Tandem's own staging or scripts folder"),
        ("scripts_app.rs", "extract_archive", "Tandem's own extraction folder"),
        ("scripts_app.rs", "repl_environment_failed", "Tandem's own virtual environments folder"),
        ("scripts_app.rs", "export_one", "the Desktop known folder, a name that is free"),
        ("settings_export_ui.rs", "entries", "Tandem's own settings folders and the export staging folder"),
        ("settings_export_ui.rs", "is_reparse", "an entry of Tandem's own settings folders"),
        ("settings_export_ui.rs", "copy_contents", "Tandem's own settings folders and the export staging folder"),
        ("settings_export_ui.rs", "runtime_requirements", "Tandem's own scripts folder"),
        ("settings_export_ui.rs", "export_entity", "Tandem's own settings folders"),
        ("settings_export_ui.rs", "stage_scripts", "Tandem's own staging folders"),
        ("settings_export_ui.rs", "all_entries", "Tandem's own settings folders"),
        ("pathnav_ui.rs", "pathnav_sniff", "judged inside by netpath::may_probe with the setting's Probe (EnableSemanticHistoryOnNetworkMounts), before the closure runs"),
        ("pathnav_ui.rs", "open_local", "judged inside by netpath::may_probe with the setting's Probe, before the closure runs"),
    ];
    /// What counts as having judged: the one judge or a wrapper that calls it.
    const JUDGES: &[&str] = &["localfs::", "stays_local", "ancestors_stay_local", "may_probe", "readable_path", "local_folder", "netpath::judge"];

    const PROBES: &[&str] = &[".is_dir()", ".exists()", ".is_file()", ".metadata()", "fs::metadata(", "symlink_metadata(", "File::open(", "fs::read_dir(", "try_exists(", "canonicalize("];

    fn fn_name(line: &str) -> Option<String> {
        let t = line.trim_start();
        let t = t.strip_prefix("pub(super) ").or_else(|| t.strip_prefix("pub(crate) ")).or_else(|| t.strip_prefix("pub ")).unwrap_or(t);
        let t = t.strip_prefix("async ").unwrap_or(t);
        let rest = t.strip_prefix("fn ")?;
        Some(rest.split(|c: char| !(c.is_alphanumeric() || c == '_')).next()?.to_string())
    }

    #[test]
    fn every_path_probe_goes_through_the_judge() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(src.join("app")).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "rs")).collect();
        files.push(src.join("start_dir.rs"));
        files.sort();
        let (mut wrong, mut used) = (Vec::new(), Vec::new());
        for path in files {
            let file = path.file_name().unwrap().to_string_lossy().into_owned();
            let text = std::fs::read_to_string(&path).unwrap();
            let live = text.split("\n#[cfg(test)]").next().unwrap();
            let lines: Vec<&str> = live.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                if line.trim_start().starts_with("//") || !PROBES.iter().any(|p| line.contains(p)) {
                    continue;
                }
                let Some(start) = (0..=i).rev().find(|&j| fn_name(lines[j]).is_some()) else {
                    wrong.push(format!("{file}:{} a probe outside any function", i + 1));
                    continue;
                };
                let name = fn_name(lines[start]).unwrap();
                // The judge must come before the probe: in an earlier line of the function, or the same line.
                let before = lines[start..i].join("\n") + "\n" + line.split(|c| PROBES.iter().any(|p| line.find(p) == Some(0) && c == 'x')).next().unwrap_or("");
                let judged = JUDGES.iter().any(|j| before.contains(j));
                let own = OWN.iter().any(|(f, n, _)| *f == file && *n == name);
                if own {
                    used.push((file.clone(), name.clone()));
                }
                if !judged && !own {
                    wrong.push(format!("{file}:{} in {name}: judge the path first (crate::localfs::*) or list it in OWN with the reason", i + 1));
                }
            }
        }
        for (f, n, _) in OWN {
            if !used.iter().any(|(a, b)| a == f && b == n) {
                wrong.push(format!("{f} {n} is in OWN but has no unjudged probe now: remove it"));
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    /// There is one device-name rule (`inert_files::is_device_name`; `scripts::is_device_name` and
    /// `zip_check::device_name` call it): no other live source lists the device names.
    #[test]
    fn there_is_one_device_name_rule() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut copies = Vec::new();
        let mut dirs = vec![src.clone(), src.join("app")];
        while let Some(d) = dirs.pop() {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                if p.extension().is_some_and(|e| e == "rs") && p.file_name().is_some_and(|n| n != "inert_files.rs") {
                    let t = std::fs::read_to_string(&p).unwrap();
                    let live = t.split("\n#[cfg(test)]").next().unwrap();
                    if live.contains("\"CONOUT$\"") && !p.file_name().is_some_and(|n| n == "util_console.rs") {
                        copies.push(p.file_name().unwrap().to_string_lossy().into_owned());
                    }
                }
            }
        }
        assert!(copies.is_empty(), "a second list of device names: {copies:?}");
    }
}

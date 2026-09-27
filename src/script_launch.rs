//! Launching a script (iTerm2's `launchScriptWithAbsolutePath:`, `M:867-977`, and
//! `iTermAPIScriptLauncher`, `L:725-1029`): what a path is, the launch's environment and
//! Script Console line, the exit texts, the API gate and the output decoder.

use std::path::{Path, PathBuf};

use crate::script_runtime::{shebang_version, MARKER, SETUP_CFG, VENV_DIR};
use crate::scripts::{self, Fs};

pub use crate::script_runtime::env_missing_line;

/// A launch someone or something asked for: the path (a `.py` file or a full environment's
/// folder), its arguments, and whether a person chose it (`explicitUserAction`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub path: PathBuf,
    pub args: Vec<String>,
    pub explicit: bool,
}

/// What a path is to the launcher.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A `.py` file, run with the shared environment of its shebang's version.
    Basic { script: PathBuf, version: Option<String> },
    /// A folder with its own environment and main `<container>\<name>\<name>.py`.
    Full { container: PathBuf, main: PathBuf },
    /// A folder whose environment lost its interpreter but kept its marker and `setup.cfg`
    /// (`M:980-992`).
    NeedsRebuild { container: PathBuf },
    /// Any other folder (`M:957-973`).
    Malformed(PathBuf),
    /// Anything else: never opened (implementer-rules).
    NotAScript(PathBuf),
}

/// A full environment's main script: `<container>\<name>\<name>.py`.
fn main_of(container: &Path) -> Option<PathBuf> {
    let name = container.file_name()?.to_string_lossy().into_owned();
    Some(container.join(&name).join(format!("{name}.py")))
}

/// `environmentForScript:checkForMain:YES` (`L:1071-1117`): the folder runs as one script.
fn runnable_full(container: &Path, fs: &dyn Fs) -> Option<PathBuf> {
    let main = main_of(container)?;
    (fs.is_file(&main) && scripts::is_full_environment(container, fs)).then_some(main)
}

/// Whether `path` is a script file: the extension exactly `py` (Task 1's rule).
fn is_py(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "py")
}

/// `fullEnvironmentContainerForMainPyPath:` (`L:1051-1069`): the container of a runnable full
/// environment whose main is `path`.
pub fn full_container_for_main(path: &Path, fs: &dyn Fs) -> Option<PathBuf> {
    if !is_py(path) {
        return None;
    }
    let container = path.parent()?.parent()?;
    (main_of(container).as_deref() == Some(path) && runnable_full(container, fs).is_some()).then(|| container.to_path_buf())
}

/// `uvScriptContainerNeedsReprovision:` (`M:983-992`).
fn needs_rebuild(container: &Path, fs: &dyn Fs) -> bool {
    fs.is_file(&container.join(MARKER)) && fs.is_file(&container.join(SETUP_CFG)) && !fs.is_file(&crate::script_runtime::venv_python(&container.join(VENV_DIR)))
}

/// What `path` is (`M:867-977`): an inner main redirects to its container; a runnable full
/// environment; a folder needing a rebuild; a `.py` file with its shebang's version
/// (`first_line` reads the file's first line); any other folder is malformed; the rest is
/// not a script.
pub fn classify(path: &Path, fs: &dyn Fs, first_line: &dyn Fn(&Path) -> Option<String>) -> Kind {
    let path = full_container_for_main(path, fs).unwrap_or_else(|| path.to_path_buf());
    if fs.is_dir(&path) {
        if let Some(main) = runnable_full(&path, fs) {
            return Kind::Full { container: path, main };
        }
        if needs_rebuild(&path, fs) {
            return Kind::NeedsRebuild { container: path };
        }
        return Kind::Malformed(path);
    }
    if is_py(&path) && fs.is_file(&path) {
        let version = first_line(&path).and_then(|l| shebang_version(&l));
        return Kind::Basic { script: path, version };
    }
    Kind::NotAScript(path)
}

fn name_of(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The Script Console entry's name (`L:747-754`): a script's file stem, a full environment's
/// folder.
pub fn entry_name(kind: &Kind) -> String {
    match kind {
        Kind::Basic { script, .. } => script.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        Kind::Full { container, .. } | Kind::NeedsRebuild { container } => name_of(container),
        Kind::Malformed(p) | Kind::NotAScript(p) => name_of(p),
    }
}

/// The path the Scripts menu knows it by: the file, or the container.
pub fn full_path(kind: &Kind) -> PathBuf {
    match kind {
        Kind::Basic { script, .. } => script.clone(),
        Kind::Full { container, .. } | Kind::NeedsRebuild { container } => container.clone(),
        Kind::Malformed(p) | Kind::NotAScript(p) => p.clone(),
    }
}

/// The script that runs (the entry's `path`): the file, or the main .py.
pub fn main_path(kind: &Kind) -> PathBuf {
    match kind {
        Kind::Full { main, .. } => main.clone(),
        other => full_path(other),
    }
}

/// A script's environment (`L:855-920`, and the wrapper's `PYTHONUNBUFFERED`): `base` (Tandem's
/// own) without `PYTHONPATH` and a stale `IT2_SUITE` (names ignoring case, as Windows
/// does), plus the cookie, the key, unbuffered UTF-8 output and the venv's certifi roots when
/// they exist. `HOME` and `SHELL` are not set (Decision). Never logged: it holds the cookie.
pub fn launch_env(base: Vec<(String, String)>, cookie: &str, key: &str, venv: Option<&Path>, certifi_exists: bool) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = base.into_iter().filter(|(k, _)| !k.eq_ignore_ascii_case("PYTHONPATH") && !k.eq_ignore_ascii_case("IT2_SUITE")).collect();
    let mut extra = vec![("ITERM2_COOKIE", cookie.to_string()), ("ITERM2_KEY", key.to_string()), ("PYTHONUNBUFFERED", "1".to_string()), ("PYTHONIOENCODING", "utf-8".to_string())];
    if let (Some(v), true) = (venv, certifi_exists) {
        extra.push(("SSL_CERT_FILE", crate::script_runtime::certifi_path(v).to_string_lossy().into_owned()));
    }
    for (k, v) in extra {
        out.retain(|(o, _)| !o.eq_ignore_ascii_case(k));
        out.push((k.to_string(), v));
    }
    out
}

/// The first line of a launch's log (`L:838-839`): the interpreter, the script and the
/// arguments. Never the environment.
pub fn console_line(python: &Path, script: &Path, args: &[String]) -> String {
    let mut words = vec![python.display().to_string(), script.display().to_string()];
    words.extend(args.iter().cloned());
    format!("{}\n", words.join(" "))
}

/// A process ended by an exception (NTSTATUS error codes, 0xC0000000 and up) reads as killed
/// by a signal (Decision).
const KILLED_FROM: u32 = 0xC000_0000;

/// What the log gets when a script ends (`L:976-983`): nothing for status 0.
pub fn exit_text(code: u32) -> Option<String> {
    match code {
        0 => None,
        c if c >= KILLED_FROM => Some(KILLED.to_string()),
        c => Some(format!("\n** Script exited with status {c} **")),
    }
}

/// Whether a launch goes ahead (`M:883-889`, `M:924-928`, `L:741-749`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchGate {
    Launch,
    /// A person asked while the API is off: "Enable Python API?" first (forced).
    EnableFirst,
    /// Nobody asked and the API is off: nothing runs.
    Skip,
}

pub fn launch_gate(explicit: bool, api_on: bool) -> LaunchGate {
    match (explicit, api_on) {
        (_, true) => LaunchGate::Launch,
        (true, false) => LaunchGate::EnableFirst,
        (false, false) => LaunchGate::Skip,
    }
}

/// A script's output as UTF-8 text: a character split between reads waits for its other
/// bytes; an invalid byte becomes U+FFFD (Decision; iTerm2 drops the whole chunk).
#[derive(Debug, Default)]
pub struct Decoder {
    carry: Vec<u8>,
}

impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) -> String {
        let mut buf = std::mem::take(&mut self.carry);
        buf.extend_from_slice(bytes);
        let mut out = String::new();
        let mut rest = &buf[..];
        loop {
            match std::str::from_utf8(rest) {
                Ok(s) => {
                    out.push_str(s);
                    break;
                }
                Err(e) => {
                    let (good, bad) = rest.split_at(e.valid_up_to());
                    // Checked valid just above.
                    out.push_str(std::str::from_utf8(good).unwrap_or_default());
                    match e.error_len() {
                        Some(n) => {
                            out.push('\u{FFFD}');
                            rest = &bad[n..];
                        }
                        // The end is the start of a character: it waits for the next read.
                        None => {
                            self.carry = bad.to_vec();
                            break;
                        }
                    }
                }
            }
        }
        out
    }
}

/// The notification when a script ends unexpectedly (`L:985-993`).
pub const SCRIPT_FAILED: &str = "Script Failed";
/// Whether a script's end posts "Script Failed" (`L:983-1004`): only an abnormal end (a
/// status other than 0, or killed) the person did not ask for.
pub fn notifies(code: u32, terminated_by_user: bool) -> bool {
    exit_text(code).is_some() && !terminated_by_user
}
pub fn ended_unexpectedly(name: &str) -> String {
    format!("\u{201c}{name}\u{201d} ended unexpectedly.")
}
/// A launch that failed to start (`L:1022-1029`, `L:787`).
pub const ERROR_RUNNING_TITLE: &str = "Error running script";
pub fn error_running(path: &str, reason: &str) -> String {
    format!("Script at \u{201c}{path}\u{201d} failed.\n\n{reason}")
}
pub fn failed_to_launch(reason: &str) -> String {
    format!("ERROR: Failed to launch: {reason}")
}
/// A folder that is not a script (`M:957-973`).
pub const MALFORMED_HEADING: &str = "Cannot Run Script";
pub fn malformed(name: &str) -> String {
    format!("The script \u{201c}{name}\u{201d} is malformed.")
}
pub const REVEAL: &str = "Reveal";
pub const MALFORMED_ID: &str = "NoSyncScriptMalformed";
/// A full environment that lost its interpreter, launched by a person (`M:994-1043`).
pub const REBUILD_HEADING: &str = "Rebuild Python Environment?";
pub fn rebuild_text(name: &str) -> String {
    format!("The Python environment for \u{201c}{name}\u{201d} is missing (the shared runtime may have been deleted). Rebuild it from its saved requirements now?")
}
pub const REBUILD: &str = "Rebuild";
/// Persistent in iTerm2: never silenced.
pub const REBUILD_ID: &str = "NoSyncRebuildMissingUvEnv";
pub const COULD_NOT_REBUILD: &str = "Could Not Rebuild Environment";
pub const REBUILDING: &str = "Rebuilding the Python environment\u{2026}";
/// The log line of a script killed (`L:977`).
pub const KILLED: &str = "\n** Script was killed by a signal **";

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[derive(Default)]
    struct FakeFs {
        files: BTreeSet<PathBuf>,
        dirs: BTreeSet<PathBuf>,
    }

    impl FakeFs {
        fn file(&mut self, p: &Path) {
            let mut up = p.parent();
            while let Some(d) = up {
                self.dirs.insert(d.to_path_buf());
                up = d.parent();
            }
            self.files.insert(p.to_path_buf());
        }
    }

    impl Fs for FakeFs {
        fn list(&self, _dir: &Path) -> Vec<(String, bool)> {
            Vec::new()
        }
        fn is_file(&self, p: &Path) -> bool {
            self.files.contains(p)
        }
        fn is_dir(&self, p: &Path) -> bool {
            self.dirs.contains(p)
        }
        fn canonical(&self, dir: &Path) -> Option<PathBuf> {
            Some(dir.to_path_buf())
        }
    }

    #[test]
    fn classify_follows_iterm2() {
        let s = PathBuf::from(r"C:\S");
        let mut fs = FakeFs::default();
        fs.file(&s.join("a.py"));
        // A full environment as in Task 1.
        for rel in [r"Full\.venv\Scripts\python.exe", r"Full\python-runtime.json", r"Full\setup.cfg", r"Full\Full\Full.py"] {
            fs.file(&s.join(rel));
        }
        for rel in [r"Gone\setup.cfg", r"Gone\python-runtime.json", r"Gone\Gone\Gone.py"] {
            fs.file(&s.join(rel));
        }
        fs.file(&s.join(r"Folder\x.txt"));
        fs.file(&s.join("notes.txt"));
        let a = s.join("a.py");
        let first = |p: &Path| (p == a.as_path()).then(|| "#!/usr/bin/env python3.10\n".to_string());

        assert_eq!(classify(&a, &fs, &first), Kind::Basic { script: a.clone(), version: Some("3.10".into()) });
        let full = Kind::Full { container: s.join("Full"), main: s.join(r"Full\Full\Full.py") };
        assert_eq!(classify(&s.join("Full"), &fs, &first), full);
        assert_eq!(classify(&s.join(r"Full\Full\Full.py"), &fs, &first), full, "the inner main redirects");
        assert_eq!(full_container_for_main(&s.join(r"Full\Full\Full.py"), &fs), Some(s.join("Full")));
        assert_eq!(classify(&s.join("Gone"), &fs, &first), Kind::NeedsRebuild { container: s.join("Gone") });
        let gone_main = s.join(r"Gone\Gone\Gone.py");
        assert_eq!(classify(&gone_main, &fs, &first), Kind::Basic { script: gone_main.clone(), version: None }, "no redirect without an environment");
        assert_eq!(full_container_for_main(&gone_main, &fs), None);
        assert_eq!(classify(&s.join("Folder"), &fs, &first), Kind::Malformed(s.join("Folder")));
        assert_eq!(classify(&s.join("notes.txt"), &fs, &first), Kind::NotAScript(s.join("notes.txt")));
        assert_eq!(classify(&s.join("missing.py"), &fs, &first), Kind::NotAScript(s.join("missing.py")));
        assert_eq!(classify(&s.join("Missing"), &fs, &first), Kind::NotAScript(s.join("Missing")));

        let basic = classify(&a, &fs, &first);
        assert_eq!(entry_name(&basic), "a");
        assert_eq!(entry_name(&full), "Full");
        assert_eq!((full_path(&full), main_path(&full)), (s.join("Full"), s.join(r"Full\Full\Full.py")));
        assert_eq!((full_path(&basic), main_path(&basic)), (a.clone(), a));
    }

    #[test]
    fn launch_env_follows_iterm2() {
        let base = vec![("Path", "C:\\W"), ("PYTHONPATH", "x"), ("It2_Suite", "s"), ("HOME", "h")].into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>();
        let env = launch_env(base.clone(), "c00kie", "KEY", Some(Path::new(r"C:\v")), true);
        let get = |env: &[(String, String)], k: &str| env.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.clone());
        assert_eq!(get(&env, "Path").as_deref(), Some("C:\\W"));
        assert_eq!(get(&env, "HOME").as_deref(), Some("h"));
        assert_eq!(get(&env, "ITERM2_COOKIE").as_deref(), Some("c00kie"));
        assert_eq!(get(&env, "ITERM2_KEY").as_deref(), Some("KEY"));
        assert_eq!(get(&env, "PYTHONUNBUFFERED").as_deref(), Some("1"));
        assert_eq!(get(&env, "PYTHONIOENCODING").as_deref(), Some("utf-8"));
        assert_eq!(get(&env, "SSL_CERT_FILE").as_deref(), Some(r"C:\v\Lib\site-packages\certifi\cacert.pem"));
        assert_eq!(get(&env, "PYTHONPATH"), None);
        assert_eq!(get(&env, "IT2_SUITE"), None);
        assert_eq!(env.len(), 7);
        let env = launch_env(base, "c00kie", "KEY", Some(Path::new(r"C:\v")), false);
        assert_eq!(get(&env, "SSL_CERT_FILE"), None);

        let (py, s) = (Path::new(r"C:\v\Scripts\python.exe"), Path::new(r"C:\S\a.py"));
        assert_eq!(console_line(py, s, &["a b".into()]), format!("{} {} a b\n", py.display(), s.display()));
        assert_eq!(console_line(py, s, &[]), format!("{} {}\n", py.display(), s.display()));
    }

    #[test]
    fn exit_texts_follow_iterm2() {
        assert_eq!(exit_text(0), None);
        assert_eq!(exit_text(3).as_deref(), Some("\n** Script exited with status 3 **"));
        assert_eq!(exit_text(0xC000013A).as_deref(), Some("\n** Script was killed by a signal **"));
        assert_eq!(ended_unexpectedly("tool"), "\u{201c}tool\u{201d} ended unexpectedly.");
        assert_eq!(SCRIPT_FAILED, "Script Failed");
        assert!(notifies(3, false) && notifies(0xC000013A, false));
        assert!(!notifies(0, false) && !notifies(3, true) && !notifies(0xC000013A, true));
        assert_eq!(ERROR_RUNNING_TITLE, "Error running script");
        assert_eq!(error_running(r"C:\S\a.py", "boom"), "Script at \u{201c}C:\\S\\a.py\u{201d} failed.\n\nboom");
        assert_eq!(failed_to_launch("boom"), "ERROR: Failed to launch: boom");
        assert_eq!(malformed("Folder"), "The script \u{201c}Folder\u{201d} is malformed.");
        assert_eq!((MALFORMED_HEADING, REVEAL, MALFORMED_ID), ("Cannot Run Script", "Reveal", "NoSyncScriptMalformed"));
        assert_eq!(
            env_missing_line("Full"),
            "The Python environment for \u{201c}Full\u{201d} is missing (the shared runtime may have been deleted). Launch it from the Scripts menu to rebuild it.\n"
        );
        assert_eq!(
            rebuild_text("Full"),
            "The Python environment for \u{201c}Full\u{201d} is missing (the shared runtime may have been deleted). Rebuild it from its saved requirements now?"
        );
        assert_eq!((REBUILD_HEADING, REBUILD, REBUILD_ID), ("Rebuild Python Environment?", "Rebuild", "NoSyncRebuildMissingUvEnv"));
        assert_eq!((COULD_NOT_REBUILD, REBUILDING), ("Could Not Rebuild Environment", "Rebuilding the Python environment\u{2026}"));
    }

    #[test]
    fn launch_gate_follows_iterm2() {
        assert_eq!(launch_gate(true, true), LaunchGate::Launch);
        assert_eq!(launch_gate(true, false), LaunchGate::EnableFirst);
        assert_eq!(launch_gate(false, true), LaunchGate::Launch);
        assert_eq!(launch_gate(false, false), LaunchGate::Skip);
    }

    #[test]
    fn decoder_keeps_split_characters() {
        let mut d = Decoder::default();
        assert_eq!(d.push(&[0xE2, 0x86]), "");
        assert_eq!(d.push(&[0x92, b'x']), "\u{2192}x");
        assert_eq!(d.push(&[0xFF]), "\u{FFFD}");
        assert_eq!(d.push(b"a\xFFb"), "a\u{FFFD}b");
    }
}

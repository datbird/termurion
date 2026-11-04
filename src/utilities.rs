//! The `utilities` folder beside termurion.exe (milestone 11e): one hard link (or copy) of
//! `termurion-utils.exe` per utility name, refreshed at every start when stale, and the PATH
//! value that puts the folder at the end of a new session's PATH (`PS:3273-3281`). Pure
//! except `ensure`.

use std::ffi::OsStr;
use std::path::PathBuf;

/// The folder beside termurion.exe that holds the utilities.
pub const FOLDER: &str = "utilities";

/// The multi-call program every utility name links to.
pub const EXE: &str = "termurion-utils.exe";

/// iTerm2's it2api script (`util_api::script`), written into the folder.
pub const API_SCRIPT: &str = "it2api.py";

/// A file's length and modified time (nanoseconds since 1970), enough to tell a link or copy
/// of an older `termurion-utils.exe` from a current one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub len: u64,
    pub modified: u64,
}

/// A utility file needs refreshing: it is missing, or its length or time differs from the
/// program's (a hard link shares both; `copy` keeps the time).
pub fn stale(src: Stamp, dst: Option<Stamp>) -> bool {
    dst != Some(src)
}

/// iTerm2's PATH rule (`PS:3276-3279`): `base` split on the separator, the folder added as
/// one more component, joined again. Windows separates with ";". An empty PATH is one empty
/// component, and the folder is added even when already present.
pub fn path_with(base: &str, dir: &str) -> String {
    let mut parts: Vec<&str> = base.split(';').collect();
    parts.push(dir);
    parts.join(";")
}

/// The folder as a PATH entry: its plain local form, only when that is a drive path (never
/// relative, which each session would resolve against its own working folder, and never a
/// network path) with no ";" (which would split it into two entries).
pub fn path_entry(folder: &str) -> Option<String> {
    let local = crate::util_cli::local_form(folder);
    (crate::prompted::local_path_ok(local) && !local.contains(';')).then(|| local.to_string())
}

/// Windows' standard PATH, the stand-in for iTerm2's `_PATH_STDPATH` (`PS:3276`): the
/// system path a fresh Windows 10 or 11 install gives, under `system_root`.
pub fn std_path(system_root: &str) -> String {
    let r = system_root;
    format!("{r}\\system32;{r};{r}\\System32\\Wbem;{r}\\System32\\WindowsPowerShell\\v1.0\\;{r}\\System32\\OpenSSH\\")
}

/// A new session's PATH with the utilities folder (`entry`, from `path_entry`) at the end:
/// Termurion's PATH by iTerm2's rule, the standard path when PATH is unset, and None (the
/// session keeps its inherited PATH) when Termurion's PATH is not Unicode.
pub fn session_path(path: Option<&OsStr>, system_root: &str, entry: &str) -> Option<String> {
    match path {
        Some(p) => p.to_str().map(|p| path_with(p, entry)),
        None => Some(path_with(&std_path(system_root), entry)),
    }
}

/// What `ensure` did: the folder, and each file it could not bring up to date as
/// "<file>: <error>".
#[derive(Debug, PartialEq, Eq)]
pub struct Ensured {
    pub folder: PathBuf,
    pub failed: Vec<String>,
}

/// The banner for files `ensure` could not bring up to date, when there are any.
pub fn refresh_banner(failed: &[String]) -> Option<String> {
    (!failed.is_empty()).then(|| format!("Some utilities could not be refreshed ({}). Close any running utility and restart Termurion.", failed.join("; ")))
}

pub use crate::platform::utilities::ensure;

/// What a start does about the folder: its path and the banner to show, if any. A system with no
/// utilities folder gets neither (a feature this OS lacks is reported only when someone uses it).
pub fn at_launch(exe_dir: &std::path::Path) -> (Option<PathBuf>, Option<String>) {
    if !crate::platform::utilities::HAS_FOLDER {
        return (None, None);
    }
    match ensure(exe_dir) {
        Ok(done) => {
            let entry = path_entry(&done.folder.to_string_lossy());
            let banner = if let Some(text) = refresh_banner(&done.failed) {
                crate::host_base::log(&format!("termurion: {text}"));
                Some(text)
            } else if entry.is_none() {
                Some(format!("The utilities folder {} has a \";\" in its path, so it is not put on PATH.", done.folder.display()))
            } else {
                None
            };
            (Some(done.folder), banner)
        }
        Err(e) => (None, Some(format!("The utilities folder could not be made: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    use std::path::Path;

    /// Item (c): the macOS launch banner "The utilities folder could not be made".
    #[cfg(unix)]
    #[test]
    fn a_launch_without_a_utilities_folder_raises_no_banner() {
        assert_eq!(at_launch(std::path::Path::new("/nonexistent")), (None, None));
    }

    #[test]
    fn path_follows_iterm2() {
        let d = "C:\\T\\utilities";
        assert_eq!(path_with("C:\\W", d), "C:\\W;C:\\T\\utilities");
        assert_eq!(path_with("", d), format!(";{d}"), "an empty PATH is one empty component");
        assert_eq!(path_with("C:\\W;", d), format!("C:\\W;;{d}"));
        assert_eq!(path_with(&format!("C:\\W;{d}"), d), format!("C:\\W;{d};{d}"), "appended even when already present");
    }

    #[test]
    fn stale_follows_the_stamps() {
        let s = Stamp { len: 10, modified: 5 };
        assert!(stale(s, None));
        assert!(!stale(s, Some(s)));
        assert!(stale(s, Some(Stamp { len: 11, ..s })));
        assert!(stale(s, Some(Stamp { modified: 6, ..s })));
    }

    #[cfg(windows)]
    #[test]
    fn utilities_folder_follows_the_exe() {
        let dir = std::env::temp_dir().join(format!("termurion-utilities-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let folder = dir.join(FOLDER);

        // No termurion-utils.exe: an error and no folder.
        assert!(ensure(&dir).is_err());
        assert!(!folder.exists(), "no folder without the program");

        let small = std::path::PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into())).join("System32").join("where.exe");
        let src = dir.join(EXE);
        std::fs::copy(&small, &src).unwrap();
        let len = std::fs::metadata(&src).unwrap().len();
        assert_eq!(ensure(&dir).unwrap(), Ensured { folder: folder.clone(), failed: Vec::new() });
        let mut names: Vec<String> = std::fs::read_dir(&folder).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        let want: Vec<String> = crate::util_cli::NAMES.iter().map(|n| format!("{n}.exe")).collect();
        let api = folder.join(API_SCRIPT);
        assert_eq!(std::fs::read_to_string(&api).unwrap(), crate::util_api::script(), "it2api.py is the script");
        names.retain(|n| n != API_SCRIPT);
        assert_eq!(names, want, "one file per name");
        assert_eq!(want.first().map(String::as_str), Some("imgcat.exe"));
        assert_eq!(want.last().map(String::as_str), Some("it2universion.exe"));
        let times = |f: &Path| -> Vec<(u64, std::time::SystemTime)> {
            crate::util_cli::NAMES.iter().map(|n| {
                let m = std::fs::metadata(f.join(format!("{n}.exe"))).unwrap();
                (m.len(), m.modified().unwrap())
            }).collect()
        };
        let first = times(&folder);
        assert!(first.iter().all(|(l, _)| *l == len), "each as long as termurion-utils.exe");

        // An unrelated file survives; a second pass changes nothing.
        std::fs::write(folder.join("notes.txt"), "mine").unwrap();
        let api_time = std::fs::metadata(&api).unwrap().modified().unwrap();
        ensure(&dir).unwrap();
        assert_eq!(times(&folder), first, "nothing refreshed");
        assert_eq!(std::fs::metadata(&api).unwrap().modified().unwrap(), api_time, "it2api.py not rewritten");

        // A changed it2api.py is written again.
        std::fs::write(&api, "print('changed')\n").unwrap();
        ensure(&dir).unwrap();
        assert_eq!(std::fs::read_to_string(&api).unwrap(), crate::util_api::script(), "rewritten");

        // A new, longer termurion-utils.exe (a new file, so the old links keep the old bytes).
        std::fs::remove_file(&src).unwrap();
        let mut longer = std::fs::read(&small).unwrap();
        longer.extend_from_slice(&[0u8; 4096]);
        std::fs::write(&src, &longer).unwrap();
        assert_eq!(std::fs::metadata(folder.join("imgcat.exe")).unwrap().len(), len, "the old link keeps its length");
        ensure(&dir).unwrap();
        assert!(times(&folder).iter().all(|(l, _)| *l == longer.len() as u64), "every name refreshed");
        assert_eq!(std::fs::read(folder.join("it2check.exe")).unwrap(), longer);
        assert_eq!(std::fs::read_to_string(folder.join("notes.txt")).unwrap(), "mine", "never touched");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn path_entry_is_local_absolute_and_whole() {
        assert_eq!(path_entry("C:\\T\\utilities").as_deref(), Some("C:\\T\\utilities"));
        assert_eq!(path_entry("\\\\?\\C:\\T\\utilities").as_deref(), Some("C:\\T\\utilities"), "a verbatim local path in its plain form");
        for bad in ["", "utilities", "\\T\\utilities", "C:utilities", "C:\\a;b\\utilities", "\\\\host\\s\\utilities", "//host/s/utilities", "\\\\?\\UNC\\host\\s\\utilities"] {
            assert_eq!(path_entry(bad), None, "{bad}");
        }
    }

    #[test]
    fn session_path_follows_iterm2() {
        let d = "C:\\T\\utilities";
        assert_eq!(session_path(Some(OsStr::new("C:\\W")), "C:\\Windows", d).as_deref(), Some("C:\\W;C:\\T\\utilities"));
        assert_eq!(session_path(Some(OsStr::new("")), "C:\\Windows", d), Some(format!(";{d}")), "an empty PATH as the brief pins");
        // Unset: the standard path (iTerm2's `_PATH_STDPATH`), never a leading empty component.
        assert_eq!(
            std_path("C:\\Windows"),
            "C:\\Windows\\system32;C:\\Windows;C:\\Windows\\System32\\Wbem;C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\;C:\\Windows\\System32\\OpenSSH\\"
        );
        assert_eq!(session_path(None, "C:\\Windows", d), Some(format!("{};{d}", std_path("C:\\Windows"))));
        // Not Unicode: no PATH value, so the session keeps the PATH it inherits.
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            let bad = std::ffi::OsString::from_wide(&[0x43, 0xD800, 0x3B, 0x44]);
            assert_eq!(session_path(Some(&bad), "C:\\Windows", d), None);
        }
    }

    #[test]
    fn refresh_failures_make_a_banner() {
        assert_eq!(refresh_banner(&[]), None);
        assert_eq!(
            refresh_banner(&["imgcat.exe: Access is denied. (os error 5)".to_string(), "it2ul.exe: x".to_string()]).as_deref(),
            Some("Some utilities could not be refreshed (imgcat.exe: Access is denied. (os error 5); it2ul.exe: x). Close any running utility and restart Termurion.")
        );
    }

    /// A folder for one test holding a copy of `exe` (from System32) as termurion-utils.exe.
    #[cfg(windows)]
    fn with_program(name: &str, exe: &str) -> (crate::util_cli::TempDir, std::path::PathBuf) {
        let dir = crate::util_cli::TempDir::new(name);
        let system = std::path::PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into())).join("System32");
        std::fs::copy(system.join(exe), dir.join(EXE)).unwrap();
        let src = system.join(exe);
        (dir, src)
    }

    /// Replaces termurion-utils.exe with a new, longer file.
    #[cfg(windows)]
    fn grow(dir: &Path, from: &Path) -> Vec<u8> {
        let mut longer = std::fs::read(from).unwrap();
        longer.extend_from_slice(&[0u8; 4096]);
        std::fs::remove_file(dir.join(EXE)).unwrap();
        std::fs::write(dir.join(EXE), &longer).unwrap();
        longer
    }

    #[cfg(windows)]
    #[test]
    fn network_and_relative_folders_are_refused() {
        for p in ["\\\\nohost.invalid\\share\\Termurion", "//nohost.invalid/share/Termurion", "\\\\?\\UNC\\nohost.invalid\\share\\Termurion", "\\\\?\\GLOBALROOT\\Device\\Mup\\nohost.invalid\\s"] {
            let t = std::time::Instant::now();
            let e = ensure(Path::new(p)).unwrap_err();
            assert!(e.contains("network folder"), "{p}: {e}");
            assert!(t.elapsed() < std::time::Duration::from_millis(500), "{p}: nothing probed");
        }
        for p in ["", "rel-termurion", "\\rooted-termurion", "C:rel-termurion"] {
            let e = ensure(Path::new(p)).unwrap_err();
            assert!(e.contains("not a local folder"), "{p}: {e}");
        }
        assert!(!Path::new("rel-termurion").exists() && !Path::new("utilities").exists(), "nothing made in the working folder");

        // A verbatim local folder works, in its plain form.
        let (dir, _) = with_program("termurion-utilities-verbatim", "where.exe");
        let verbatim = std::path::PathBuf::from(format!("\\\\?\\{}", dir.display()));
        let done = ensure(&verbatim).unwrap();
        assert_eq!(done.folder, dir.join(FOLDER));
        assert!(done.failed.is_empty());

        // A file named utilities: the folder cannot be made, and the file is left alone.
        let (dir, _) = with_program("termurion-utilities-file", "where.exe");
        std::fs::write(dir.join(FOLDER), "mine").unwrap();
        assert!(ensure(&dir).is_err());
        assert_eq!(std::fs::read_to_string(dir.join(FOLDER)).unwrap(), "mine");
    }

    #[cfg(windows)]
    #[test]
    fn refresh_failures_are_reported() {
        use std::os::windows::fs::OpenOptionsExt;
        let (dir, from) = with_program("termurion-utilities-held", "PING.EXE");
        let folder = ensure(&dir).unwrap().folder;

        // A running utility is replaced (renamed aside when it cannot be removed); a file held
        // without sharing is reported, and every other name is refreshed.
        let mut running = std::process::Command::new(folder.join("imgls.exe"))
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(folder.join("imgcat.exe")).unwrap();
        let longer = grow(&dir, &from);
        let done = ensure(&dir).unwrap();
        assert_eq!(done.failed.len(), 1, "{:?}", done.failed);
        assert!(done.failed[0].starts_with("imgcat.exe: "), "{:?}", done.failed);
        for name in crate::util_cli::NAMES.iter().filter(|n| **n != "imgcat") {
            assert_eq!(std::fs::metadata(folder.join(format!("{name}.exe"))).unwrap().len(), longer.len() as u64, "{name}");
        }
        // Windows 10 and later remove a running image's link (POSIX delete), so the rename is
        // the fallback for older systems and other file systems.
        eprintln!("MEASURED: running imgls.exe set aside as .old: {}", folder.join("imgls.exe.old").exists());
        assert!(running.try_wait().unwrap().is_none(), "the utility was running throughout");
        let _ = running.kill();
        let _ = running.wait();
        drop(held);
        std::fs::write(folder.join("imgls.exe.old"), "set aside").unwrap();
        let done = ensure(&dir).unwrap();
        assert!(done.failed.is_empty(), "{:?}", done.failed);
        assert_eq!(std::fs::read(folder.join("imgcat.exe")).unwrap(), longer);
        assert!(!folder.join("imgls.exe.old").exists(), "set-aside files go at the next start");

        // A folder where a name should be is reported and left alone.
        std::fs::remove_file(folder.join("it2ul.exe")).unwrap();
        std::fs::create_dir(folder.join("it2ul.exe")).unwrap();
        std::fs::write(folder.join("it2ul.exe").join("mine.txt"), "mine").unwrap();
        let done = ensure(&dir).unwrap();
        assert_eq!(done.failed, vec!["it2ul.exe: a folder has this name".to_string()]);
        assert_eq!(std::fs::read_to_string(folder.join("it2ul.exe").join("mine.txt")).unwrap(), "mine");
        std::fs::remove_dir_all(folder.join("it2ul.exe")).unwrap();

        // A folder Termurion may not write in: every name is reported. A token holding
        // SeRestorePrivilege (an elevated ssh session) writes through the deny, so there the
        // case cannot be observed.
        let privs = std::process::Command::new("whoami").arg("/priv").output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
        if privs.lines().any(|l| l.starts_with("SeRestorePrivilege") && l.trim_end().ends_with("Enabled")) {
            eprintln!("MEASURED: SeRestorePrivilege is enabled for this token; the read-only folder case is not observable here");
            return;
        }
        struct Deny<'a>(&'a Path);
        impl Drop for Deny<'_> {
            fn drop(&mut self) {
                let _ = std::process::Command::new("icacls").arg(self.0).args(["/remove:d", "*S-1-1-0"]).output();
            }
        }
        let deny = Deny(&folder);
        let denied = std::process::Command::new("icacls").arg(&folder).args(["/deny", "*S-1-1-0:(WD,AD)"]).output().unwrap();
        assert!(denied.status.success(), "{}", String::from_utf8_lossy(&denied.stdout));
        grow(&dir, &from);
        let done = ensure(&dir).unwrap();
        assert_eq!(done.failed.len(), crate::util_cli::NAMES.len(), "{:?}", done.failed);
        for (f, name) in done.failed.iter().zip(crate::util_cli::NAMES) {
            assert!(f.starts_with(&format!("{name}.exe: ")), "{f}");
        }
        drop(deny);
    }
}

//! The `utilities` folder beside tandem.exe (milestone 11e): one hard link (or copy) of
//! `tandem-utils.exe` per utility name, refreshed at every start when stale, and the PATH
//! value that puts the folder at the end of a new session's PATH (`PS:3273-3281`). Pure
//! except `ensure`.

#[cfg(windows)]
use std::path::{Path, PathBuf};

/// The folder beside tandem.exe that holds the utilities.
pub const FOLDER: &str = "utilities";

/// The multi-call program every utility name links to.
pub const EXE: &str = "tandem-utils.exe";

/// iTerm2's it2api script (`util_api::script`), written into the folder.
pub const API_SCRIPT: &str = "it2api.py";

/// A file's length and modified time (nanoseconds since 1970), enough to tell a link or copy
/// of an older `tandem-utils.exe` from a current one.
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

#[cfg(windows)]
fn stamp(path: &Path) -> Option<Stamp> {
    let m = std::fs::metadata(path).ok()?;
    let modified = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    Some(Stamp { len: m.len(), modified })
}

/// Makes `exe_dir\utilities` and brings one `<name>.exe` per `util_cli::NAMES` entry up to
/// date with `exe_dir\tandem-utils.exe`: a stale one is removed and hard linked again, or
/// copied when a link cannot be made (another volume, a file system without links). A name
/// that cannot be refreshed (a running utility locks its file) is skipped. `it2api.py` is
/// written when its bytes are not `util_api::script()`'s (a failed write is skipped too). Files Tandem does
/// not make are never touched. Err when `tandem-utils.exe` is missing (no folder is made) or
/// the folder cannot be made.
#[cfg(windows)]
pub fn ensure(exe_dir: &Path) -> Result<PathBuf, String> {
    let src = exe_dir.join(EXE);
    let Some(src_stamp) = stamp(&src) else { return Err(format!("{} is missing", src.display())) };
    let folder = exe_dir.join(FOLDER);
    std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    for name in crate::util_cli::NAMES {
        let dst = folder.join(format!("{name}.exe"));
        if !stale(src_stamp, stamp(&dst)) {
            continue;
        }
        if dst.exists() && std::fs::remove_file(&dst).is_err() {
            continue;
        }
        if std::fs::hard_link(&src, &dst).is_err() {
            let _ = std::fs::copy(&src, &dst);
        }
    }
    let api = folder.join(API_SCRIPT);
    let script = crate::util_api::script();
    if std::fs::read(&api).ok().as_deref() != Some(script.as_bytes()) {
        let _ = std::fs::write(&api, script);
    }
    Ok(folder)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let dir = std::env::temp_dir().join(format!("tandem-utilities-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let folder = dir.join(FOLDER);

        // No tandem-utils.exe: an error and no folder.
        assert!(ensure(&dir).is_err());
        assert!(!folder.exists(), "no folder without the program");

        let small = std::path::PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into())).join("System32").join("where.exe");
        let src = dir.join(EXE);
        std::fs::copy(&small, &src).unwrap();
        let len = std::fs::metadata(&src).unwrap().len();
        assert_eq!(ensure(&dir).unwrap(), folder);
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
        assert!(first.iter().all(|(l, _)| *l == len), "each as long as tandem-utils.exe");

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

        // A new, longer tandem-utils.exe (a new file, so the old links keep the old bytes).
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
}

//! Which paths Semantic History may probe (`NFM:17-70`, `SHC:86`). A file on a slow
//! network drive can freeze the app, so a path under an ignored folder or on a mapped drive
//! is never touched unless `EnableSemanticHistoryOnNetworkMounts` says so. A network path
//! (`\\host\share` in any spelling) is never touched at all: the one rule is
//! `scripts::is_network_path` (with `scripts::stays_local` for links), not a copy of it here.

use std::path::Path;
use std::sync::Arc;

/// Answers whether a drive root (`Z:\`) is a mapped network drive.
pub trait Drives {
    fn is_remote(&self, root: &str) -> bool;
}

/// Every drive is local: the non-Windows and default answer.
pub struct LocalDrives;

impl Drives for LocalDrives {
    fn is_remote(&self, _root: &str) -> bool {
        false
    }
}

/// `GetDriveTypeW`, asked once per drive letter for the run (a drive remapped while Tandem
/// runs keeps its first answer; Decision).
#[cfg(windows)]
pub struct RealDrives;

#[cfg(windows)]
impl Drives for RealDrives {
    fn is_remote(&self, root: &str) -> bool {
        use std::sync::Mutex;
        use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
        static CACHE: Mutex<[Option<bool>; 26]> = Mutex::new([None; 26]);
        let letter = match root.as_bytes().first() {
            Some(c) if c.is_ascii_alphabetic() => (c.to_ascii_uppercase() - b'A') as usize,
            _ => return false,
        };
        if let Some(known) = CACHE.lock().ok().and_then(|c| c[letter]) {
            return known;
        }
        let wide: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
        // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call.
        let remote = unsafe { GetDriveTypeW(wide.as_ptr()) } == DRIVE_REMOTE;
        if let Ok(mut c) = CACHE.lock() {
            c[letter] = Some(remote);
        }
        remote
    }
}

/// `DRIVE_REMOTE`.
#[cfg(windows)]
const DRIVE_REMOTE: u32 = 4;

/// The drive answers for this platform.
pub fn real_drives() -> Arc<dyn Drives + Send + Sync> {
    #[cfg(windows)]
    {
        Arc::new(RealDrives)
    }
    #[cfg(not(windows))]
    {
        Arc::new(LocalDrives)
    }
}

/// What Semantic History may probe: the settings `PathsToIgnore` and
/// `EnableSemanticHistoryOnNetworkMounts`, and the drive answers.
#[derive(Clone)]
pub struct Probe {
    pub ignore: Vec<String>,
    pub allow_network: bool,
    pub drives: Arc<dyn Drives + Send + Sync>,
}

impl Default for Probe {
    fn default() -> Probe {
        Probe { ignore: Vec::new(), allow_network: false, drives: Arc::new(LocalDrives) }
    }
}

impl std::fmt::Debug for Probe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Probe").field("ignore", &self.ignore).field("allow_network", &self.allow_network).finish()
    }
}

impl Probe {
    /// `PathsToIgnore` split on "," (empty entries dropped, nothing trimmed, as iTerm2).
    pub fn from_settings(paths_to_ignore: &str, allow: bool, drives: Arc<dyn Drives + Send + Sync>) -> Probe {
        Probe { ignore: paths_to_ignore.split(',').filter(|s| !s.is_empty()).map(String::from).collect(), allow_network: allow, drives }
    }
}

/// `/` read as `\`, case ignored (Windows paths; Decision).
fn normal(s: &str) -> String {
    s.replace('/', "\\").to_lowercase()
}

/// Whether `path` is under one of the folders (`NFM:48-70`): each entry gets a trailing `\`
/// unless it has one, so `D:\slow` hides `D:\slow\a.txt` but not `D:\slower`, and not
/// the folder itself.
pub fn has_ignored_prefix(path: &str, ignore: &[String]) -> bool {
    let path = normal(path);
    ignore.iter().any(|entry| {
        let mut e = normal(entry);
        if !e.ends_with('\\') {
            e.push('\\');
        }
        path.starts_with(&e)
    })
}

/// Whether a path may be probed on disk. Never a network path or one reached through a link
/// to one (the repo's one rule); then, unless network mounts are allowed (`NFM:20-23`), not
/// under an ignored folder and not on a mapped network drive (`NFM:17-37`).
pub fn may_probe(path: &str, p: &Probe) -> bool {
    if !crate::scripts::stays_local(Path::new(path)) {
        return false;
    }
    if p.allow_network {
        return true;
    }
    if has_ignored_prefix(path, &p.ignore) {
        return false;
    }
    let b = path.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return !p.drives.is_remote(&format!("{}:\\", b[0] as char));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Z:\` is a mapped network drive.
    pub struct FakeDrives;
    impl Drives for FakeDrives {
        fn is_remote(&self, root: &str) -> bool {
            root.eq_ignore_ascii_case("Z:\\")
        }
    }

    fn local() -> Arc<dyn Drives + Send + Sync> {
        Arc::new(LocalDrives)
    }

    #[test]
    fn netpath_follows_iterm2() {
        let p = Probe::from_settings(r"D:\slow,,E:\x", false, local());
        assert_eq!(p.ignore, vec![r"D:\slow".to_string(), r"E:\x".to_string()]);
        assert!(has_ignored_prefix(r"D:\slow\a.txt", &p.ignore));
        assert!(has_ignored_prefix(r"d:/SLOW/a.txt", &p.ignore));
        assert!(!has_ignored_prefix(r"D:\slower\a.txt", &p.ignore));
        assert!(!has_ignored_prefix(r"D:\slow", &p.ignore));
        let spaced = Probe::from_settings(r" D:\slow", false, local());
        assert!(!has_ignored_prefix(r"D:\slow\a.txt", &spaced.ignore));
    }

    #[test]
    fn mapped_drives_are_not_probed() {
        let p = Probe { drives: Arc::new(FakeDrives), ..Probe::default() };
        assert!(!may_probe(r"Z:\share\f.txt", &p));
        assert!(may_probe(r"C:\f.txt", &p));
        let allow = Probe { allow_network: true, ..p };
        assert!(may_probe(r"Z:\share\f.txt", &allow));
    }

    #[test]
    fn unc_is_never_probed_even_when_allowed() {
        let allow = Probe { allow_network: true, ..Probe::default() };
        for path in [r"\\srv\s\f", "//srv/s/f", r"/\srv\s\f", r"\/srv/s/f", r"\\?\UNC\srv\s\f", r"\??\UNC\srv\s\f"] {
            assert!(!may_probe(path, &allow), "{path}");
            assert!(!may_probe(path, &Probe::default()), "{path}");
        }
        // The ignore list is skipped when network mounts are allowed (`NFM:20-23`).
        let ignoring = Probe::from_settings(r"C:\ign", true, local());
        assert!(may_probe(r"C:\ign\a.txt", &ignoring));
        assert!(!may_probe(r"C:\ign\a.txt", &Probe::from_settings(r"C:\ign", false, local())));
    }

    #[cfg(windows)]
    #[test]
    fn real_drives_answer() {
        assert!(!RealDrives.is_remote(r"C:\"));
    }
}

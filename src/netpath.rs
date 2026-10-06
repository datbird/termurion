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
    /// Where a SUBST or `DefineDosDevice` drive letter points (`\??\C:\dir`, `\??\UNC\h\s`),
    /// None for a letter that is a real volume or network drive. An object-manager query: no I/O.
    fn subst_target(&self, _root: &str) -> Option<String> {
        None
    }
}

/// Every drive is local: the non-Windows and default answer.
pub struct LocalDrives;

impl Drives for LocalDrives {
    fn is_remote(&self, _root: &str) -> bool {
        false
    }
}

/// `GetDriveTypeW`'s `DRIVE_FIXED`, which is NOT kept: a USB disk reports it, and once unplugged its
/// letter can be `net use`d, so a fixed answer is asked again on every call (the call is cheap).
#[cfg(test)]
const DRIVE_FIXED: u32 = 3;
/// `DRIVE_REMOTE`.
#[cfg(any(windows, test))]
const DRIVE_REMOTE: u32 = 4;

/// Whether drive `letter` (0 for A) is a network drive. Only a REMOTE answer is kept for the run (the
/// safe side: a letter once seen as a share stays refused); every other answer is asked again.
#[cfg(any(windows, test))]
fn remote_answer(cache: &std::sync::Mutex<[Option<bool>; 26]>, letter: usize, ask: impl FnOnce() -> u32) -> bool {
    if cache.lock().ok().and_then(|c| c[letter]) == Some(true) {
        return true;
    }
    let remote = ask() == DRIVE_REMOTE;
    if remote {
        if let Ok(mut c) = cache.lock() {
            c[letter] = Some(true);
        }
    }
    remote
}

/// `GetDriveTypeW` per drive letter, the stable answers kept for the run (`remote_answer`).
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
        remote_answer(&CACHE, letter, || {
            let wide: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
            // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call.
            unsafe { GetDriveTypeW(wide.as_ptr()) }
        })
    }

    fn subst_target(&self, root: &str) -> Option<String> {
        use windows_sys::Win32::Storage::FileSystem::QueryDosDeviceW;
        let b = root.as_bytes();
        if !b.first().is_some_and(u8::is_ascii_alphabetic) {
            return None;
        }
        let name: Vec<u16> = format!("{}:", b[0] as char).encode_utf16().chain(Some(0)).collect();
        let mut buf = vec![0u16; 1024];
        // SAFETY: `name` is NUL-terminated and `buf` is writable for its length.
        let n = unsafe { QueryDosDeviceW(name.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
        if n == 0 {
            return None;
        }
        let end = buf[..n.min(buf.len())].iter().position(|&c| c == 0).unwrap_or(n.min(buf.len()));
        let target = String::from_utf16_lossy(&buf[..end]);
        target.starts_with("\\??\\").then_some(target)
    }
}

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

/// What one click may spend on probing: the shared count of `exists` calls and link reads, and
/// the folders already found to stay local (each folder's link is read once per click).
pub struct Budget {
    left: std::cell::Cell<usize>,
    verified: std::cell::RefCell<std::collections::HashSet<std::path::PathBuf>>,
}

impl Budget {
    pub fn new(n: usize) -> Budget {
        Budget { left: std::cell::Cell::new(n), verified: Default::default() }
    }

    pub fn left(&self) -> usize {
        self.left.get()
    }

    /// Takes one probe; false when none is left.
    pub fn spend(&self) -> bool {
        match self.left.get() {
            0 => false,
            n => {
                self.left.set(n - 1);
                #[cfg(test)]
                crate::semantic::count_probe();
                true
            }
        }
    }
}

/// `p` without a long-path prefix (`\\?\` or `\??\`, either separator) before a drive letter, so
/// the ignore list and the drive check read `Z:\...` however it was spelled.
fn without_long_prefix(p: &str) -> &str {
    let b = p.as_bytes();
    let sep = |c: u8| c == b'\\' || c == b'/';
    if b.len() >= 6 && sep(b[0]) && (sep(b[1]) || b[1] == b'?') && b[2] == b'?' && sep(b[3]) && b[4].is_ascii_alphabetic() && b[5] == b':' {
        &p[4..]
    } else {
        p
    }
}

/// What one judgement reads: the ignore list, whether network mounts are allowed, the drive answers.
struct Rule<'a> {
    ignore: &'a [String],
    allow_network: bool,
    drives: &'a dyn Drives,
}

/// The cheap lexical checks, no I/O: not a network path, not a DOS device name (CON, NUL, COM1, LPT1:
/// a device may be redirected to a share), and (unless network mounts are allowed, `NFM:20-23`) not
/// under an ignored folder and not on a mapped network drive (`NFM:17-37`).
fn lexically_local(path: &str, r: &Rule) -> bool {
    if crate::scripts::is_network_path(path) || crate::scripts::is_device_name(Path::new(path)) {
        return false;
    }
    if r.allow_network {
        return true;
    }
    let path = without_long_prefix(path);
    if has_ignored_prefix(path, r.ignore) {
        return false;
    }
    let b = path.as_bytes();
    !(b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && r.drives.is_remote(&format!("{}:\\", b[0] as char)))
}

/// `path` with a SUBST letter replaced by what it points at, None when the letter is a real drive.
fn subst_resolved(path: &str, r: &Rule) -> Option<std::path::PathBuf> {
    let p = without_long_prefix(path);
    let b = p.as_bytes();
    if !(b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':') {
        return None;
    }
    let target = r.drives.subst_target(&format!("{}:\\", b[0] as char))?;
    let rest = p[2..].trim_start_matches(['\\', '/']);
    let base = without_long_prefix(&target).trim_end_matches('\\');
    Some(std::path::PathBuf::from(if rest.is_empty() { base.to_string() } else { format!("{base}\\{rest}") }))
}

/// Whether a path may be probed on disk. Never a network path or one reached through a link
/// to one (the repo's one rule). The lexical checks run first, with no I/O; then the folders
/// are read root-first, each one's link before anything below it, every hop checked the same
/// way, and every read taken from `budget` (B2: a folder that is a link to a share).
pub fn may_probe_with(path: &str, p: &Probe, read_link: &dyn Fn(&Path) -> Option<std::path::PathBuf>, budget: &Budget) -> bool {
    local_chain(Path::new(path), &Rule { ignore: &p.ignore, allow_network: p.allow_network, drives: &*p.drives }, read_link, budget, 0)
}

/// THE judge of every path that comes from outside (a file or folder another program named, a
/// document, saved state, a setting) before Tandem calls `is_dir`, `exists`, `metadata` or opens it
/// (even `is_dir` on a share path connects to it). Network and device paths, mapped and SUBST drives
/// that lead to a share, and every folder from the root down and every link target's own folders
/// (again root first) are checked; nothing is read below a link before the link itself.
pub fn judge(path: &Path, read_link: &dyn Fn(&Path) -> Option<std::path::PathBuf>, drives: &dyn Drives) -> bool {
    local_chain(path, &Rule { ignore: &[], allow_network: false, drives }, read_link, &Budget::new(PROBE_LIMIT), 0)
}

fn local_chain(path: &Path, r: &Rule, read_link: &dyn Fn(&Path) -> Option<std::path::PathBuf>, budget: &Budget, depth: usize) -> bool {
    const HOPS: usize = 8;
    if depth > HOPS || !lexically_local(&path.to_string_lossy(), r) {
        return false;
    }
    if let Some(real) = subst_resolved(&path.to_string_lossy(), r) {
        return local_chain(&real, r, read_link, budget, depth + 1);
    }
    let mut chain: Vec<&Path> = path.ancestors().filter(|a| !a.as_os_str().is_empty()).collect();
    chain.reverse();
    for at in chain {
        if budget.verified.borrow().contains(at) {
            continue;
        }
        if !budget.spend() {
            return false;
        }
        if let Some(target) = read_link(at) {
            let target = match at.parent() {
                Some(parent) if target.is_relative() => parent.join(target),
                _ => target,
            };
            if !local_chain(&target, r, read_link, budget, depth + 1) {
                return false;
            }
        }
        budget.verified.borrow_mut().insert(at.to_path_buf());
    }
    true
}

/// `may_probe_with` over the real links with a budget of its own.
pub fn may_probe(path: &str, p: &Probe) -> bool {
    may_probe_with(path, p, &crate::scripts::read_link, &Budget::new(PROBE_LIMIT))
}

/// Reads a lone caller (no click budget) may spend on one path.
const PROBE_LIMIT: usize = 64;

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

    /// Final wave minor 10: a drive that was unmapped when first asked is asked again.
    #[test]
    fn a_drive_mapped_later_is_seen() {
        let cache = std::sync::Mutex::new([None; 26]);
        let asked = std::cell::Cell::new(0);
        let asked = &asked;
        let ask = |kind: u32| {
            move || {
                asked.set(asked.get() + 1);
                kind
            }
        };
        assert!(!remote_answer(&cache, 25, ask(1)), "no root directory yet");
        assert!(remote_answer(&cache, 25, ask(DRIVE_REMOTE)), "mapped later in the session");
        assert!(remote_answer(&cache, 25, ask(1)), "a network answer is kept");
        assert_eq!(asked.get(), 2, "the kept answer asked nothing");
        // A fixed answer is NOT kept: a USB disk reports it, and its letter can be mapped later.
        assert!(!remote_answer(&cache, 2, ask(DRIVE_FIXED)), "fixed");
        assert!(remote_answer(&cache, 2, ask(DRIVE_REMOTE)), "the same letter, now a share");
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

    use std::cell::RefCell;
    use std::path::PathBuf;

    /// Links by path (lower case, `\`); every read is recorded.
    fn links<'a>(map: &'static [(&'static str, &'static str)], seen: &'a RefCell<Vec<String>>) -> impl Fn(&Path) -> Option<PathBuf> + 'a {
        move |p: &Path| {
            let key = p.to_string_lossy().to_lowercase();
            seen.borrow_mut().push(key.clone());
            map.iter().find(|(k, _)| *k == key).map(|(_, v)| PathBuf::from(v))
        }
    }

    #[test]
    fn the_cheap_checks_run_before_any_link_read() {
        let seen = RefCell::new(Vec::new());
        let rl = links(&[], &seen);
        let p = Probe::from_settings(r"C:\ign", false, Arc::new(FakeDrives));
        let b = Budget::new(50);
        assert!(!may_probe_with(r"C:\ign\a.txt", &p, &rl, &b));
        assert!(!may_probe_with(r"Z:\x\y.txt", &p, &rl, &b));
        assert!(!may_probe_with(r"\\srv\s\f", &p, &rl, &b));
        assert!(seen.borrow().is_empty(), "no link was read: {:?}", seen.borrow());
        assert_eq!(b.left(), 50);
    }

    #[test]
    fn long_path_spellings_get_the_same_checks() {
        let p = Probe::from_settings(r"C:\ign", false, Arc::new(FakeDrives));
        for path in [r"\\?\Z:\share\f.txt", r"\??\Z:\share\f.txt", r"//?/z:/share/f.txt", r"\\?\C:\ign\a.txt", r"\??\c:\IGN\a.txt"] {
            assert!(!may_probe(path, &p), "{path}");
        }
        assert!(may_probe(r"\\?\C:\f.txt", &p));
        assert!(may_probe(r"\\?\Z:\share\f.txt", &Probe { allow_network: true, ..p }));
    }

    #[test]
    fn ancestor_links_are_followed_root_first_and_checked() {
        let p = Probe { drives: Arc::new(FakeDrives), ..Probe::default() };
        let map: &'static [(&str, &str)] = &[(r"c:\proj\unc", r"\\srv\s"), (r"c:\proj\mapped", r"Z:\"), (r"c:\proj\ok", r"C:\other"), (r"c:\proj\rel", "ok")];
        let seen = RefCell::new(Vec::new());
        let rl = links(map, &seen);
        assert!(!may_probe_with(r"C:\proj\unc\x.txt", &p, &rl, &Budget::new(50)));
        assert!(!seen.borrow().iter().any(|s| s.ends_with("x.txt")), "the leaf was read under a link: {:?}", seen.borrow());
        assert!(!may_probe_with(r"C:\proj\mapped\sub\x.txt", &p, &rl, &Budget::new(50)));
        assert!(may_probe_with(r"C:\proj\ok\x.txt", &p, &rl, &Budget::new(50)));
        assert!(may_probe_with(r"C:\proj\rel\x.txt", &p, &rl, &Budget::new(50)));
        // The leaf itself may be the link.
        assert!(!may_probe_with(r"C:\proj\mapped", &p, &rl, &Budget::new(50)));
    }

    #[test]
    fn link_reads_count_toward_the_budget() {
        let seen = RefCell::new(Vec::new());
        let rl = links(&[], &seen);
        let p = Probe::default();
        let b = Budget::new(3);
        assert!(!may_probe_with(r"C:\a\b\c\d\e.txt", &p, &rl, &b), "six folders and files, three reads");
        assert_eq!(seen.borrow().len(), 3);
        // Folders already found local are not read again.
        let b = Budget::new(10);
        assert!(may_probe_with(r"C:\a\b\x.txt", &p, &rl, &b));
        let before = b.left();
        assert!(may_probe_with(r"C:\a\b\y.txt", &p, &rl, &b));
        assert_eq!(before - b.left(), 1, "only the new file is read");
    }

    #[cfg(windows)]
    #[test]
    fn real_drives_answer() {
        assert!(!RealDrives.is_remote(r"C:\"));
    }

    /// Task 3 fix round 2, item 2: a SUBST letter is resolved (QueryDosDeviceW) and its target judged,
    /// every folder of a link target is walked root first, the leaf never read through a link.
    struct Subst;
    impl Drives for Subst {
        fn is_remote(&self, root: &str) -> bool {
            root.eq_ignore_ascii_case("Z:\\")
        }
        fn subst_target(&self, root: &str) -> Option<String> {
            match root.to_ascii_uppercase().as_str() {
                "Y:\\" => Some(r"\??\UNC\h\s".to_string()),
                "X:\\" => Some(r"\??\C:\ok".to_string()),
                "W:\\" => Some(r"\??\Z:\x".to_string()),
                "V:\\" => Some(r"\??\V:\loop".to_string()),
                "U:\\" => Some(r"\??\C:\j".to_string()),
                _ => None,
            }
        }
    }

    #[test]
    fn subst_letters_are_resolved_and_judged() {
        let none = |_: &Path| None;
        assert!(judge(Path::new(r"C:\a\f"), &none, &Subst));
        assert!(!judge(Path::new(r"Y:\a\f"), &none, &Subst), "a letter onto a UNC target");
        assert!(judge(Path::new(r"X:\a\f"), &none, &Subst), "a letter onto a local folder");
        assert!(!judge(Path::new(r"W:\a"), &none, &Subst), "a letter onto a mapped drive");
        assert!(!judge(Path::new(r"V:\a"), &none, &Subst), "a loop ends");
        // The target is itself judged for links: U: is C:\j, a junction to a share.
        let read = |p: &Path| (p == Path::new(r"C:\j")).then(|| PathBuf::from(r"\\attacker\share"));
        assert!(!judge(Path::new(r"U:\f"), &read, &Subst));
    }

    #[test]
    fn a_link_targets_own_folders_are_walked_root_first() {
        // C:\j -> D:\x\y and D:\x -> \\attacker\share: D:\x must be read before D:\x\y is.
        let seen = RefCell::new(Vec::new());
        let map: &'static [(&str, &str)] = &[(r"c:\j", r"D:\x\y"), (r"d:\x", r"\\attacker\share")];
        let rl = links(map, &seen);
        assert!(!judge(Path::new(r"C:\j\f.txt"), &rl, &Subst));
        let reads = seen.borrow().clone();
        let x = reads.iter().position(|r| r == r"d:\x").expect("the folder D: x was read");
        assert!(reads.iter().position(|r| r == r"d:\x\y").is_none_or(|y| x < y), "root first: {reads:?}");
        assert!(!reads.iter().any(|r| r.ends_with("f.txt")), "{reads:?}");
    }
}

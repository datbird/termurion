//! The Scripts folder and its script tree (iTerm2's `iTermScriptsMenuController` scan and
//! `NSFileManager+iTerm` folder rule): which folder holds the scripts, what in it is a script,
//! and the texts of the scan's questions.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// The Scripts folder's name under `%APPDATA%\Tandem` (`FM:248`).
pub const SCRIPTS_DIR: &str = "Scripts";
/// The folder under the Scripts folder whose scripts start with Tandem (`FM:270-276`).
pub const AUTOLAUNCH_DIR: &str = "AutoLaunch";
/// How long a scan runs before it asks whether to go on (`M:473`).
pub const SCAN_LIMIT: Duration = Duration::from_secs(8);
/// How much longer a scan runs after Continue (`M:507`).
pub const SCAN_EXTEND: Duration = Duration::from_secs(5);

/// Segoe Fluent Icons' CheckMark and Warning: the custom folder's valid and invalid marks
/// (iTerm2's CheckMark and WarningSign images, `GP:4218`).
pub const CHECK_GLYPH: char = '\u{E73E}';
pub const WARNING_GLYPH: char = '\u{E7BA}';

/// The slow-scan question (`M:489-509`).
pub const SLOW_HEADING: &str = "Performance Issue";
pub const STOP: &str = "Stop";
pub const CONTINUE: &str = "Continue";
/// The notice after Stop (`M:497-506`).
pub const DISABLED_HEADING: &str = "Scripts Disabled";
pub const DISABLED_TEXT: &str = "Some scripts will not be available until the app has restarted or you change the scripts folder.";
/// The question for an `.its` archive found in the folder (`M:560-568`).
pub const ARCHIVE_HEADING: &str = "Install Script Archive?";
pub const MOVE_TO_TRASH: &str = "Move to Trash";
pub const ARCHIVE_ID: &str = "NoSyncInstallScriptArchive";

pub fn slow_text(root: &Path) -> String {
    format!("It is taking a long time to locate all scripts under {}. Avoid storing many files or using network mounts for the scripts folder.\n\nContinue?", root.display())
}

pub fn archive_text(file: &str) -> String {
    format!("A script archive named \u{201c}{file}\u{201d} was found in the Scripts directory. Would you like to install it?")
}

/// How deep the scan goes below the Scripts folder; links could otherwise nest folders
/// without end.
pub const MAX_DEPTH: usize = 32;

/// What the scan reads from the disk.
pub trait Fs {
    /// The entries of `dir`: each name and whether it is a folder (a link or junction to a
    /// local folder counts), in any order.
    fn list(&self, dir: &Path) -> Vec<(String, bool)>;
    fn is_file(&self, p: &Path) -> bool;
    fn is_dir(&self, p: &Path) -> bool;
    /// The folder's real path with every link resolved, None when it cannot be read.
    fn canonical(&self, dir: &Path) -> Option<PathBuf>;
    /// Where a symbolic link or junction points, read from the link itself (nothing it
    /// names is touched); None when `p` is no link.
    fn read_link(&self, _p: &Path) -> Option<PathBuf> {
        None
    }
}

pub struct RealFs;

impl Fs for RealFs {
    fn list(&self, dir: &Path) -> Vec<(String, bool)> {
        let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
        entries
            .flatten()
            .map(|e| {
                let folder = match e.file_type() {
                    Ok(t) if t.is_symlink() => link_is_local_folder(&e.path()),
                    Ok(t) => t.is_dir(),
                    Err(_) => false,
                };
                (e.file_name().to_string_lossy().into_owned(), folder)
            })
            .collect()
    }
    fn is_file(&self, p: &Path) -> bool {
        p.is_file()
    }
    fn is_dir(&self, p: &Path) -> bool {
        p.is_dir()
    }
    fn canonical(&self, dir: &Path) -> Option<PathBuf> {
        std::fs::canonicalize(dir).ok()
    }
    fn read_link(&self, p: &Path) -> Option<PathBuf> {
        read_link(p)
    }
}

/// Where a symbolic link or junction points, read from the local link itself; None when `p`
/// is no link.
pub fn read_link(p: &Path) -> Option<PathBuf> {
    std::fs::read_link(p).ok()
}

/// How many hops of a link chain are followed.
const LINK_HOPS: usize = 8;

/// Where a chain of symbolic links or junctions from `start` ends, reading each hop with
/// `read_link` (None: not a link). None when a hop names a network path (it is never followed)
/// or the chain is longer than `LINK_HOPS`; `start` itself when it is no link. Text rule only;
/// `link_chain_end_on` also refuses a hop onto a mapped drive.
pub fn link_chain_end(start: &Path, read_link: &dyn Fn(&Path) -> Option<PathBuf>) -> Option<PathBuf> {
    link_chain_end_on(start, read_link, &crate::netpath::LocalDrives)
}

/// The drive letter of a path as `X:\`, through a `\\?\` or `\??\` prefix.
fn drive_root(text: &str) -> Option<String> {
    let b = text.as_bytes();
    let at = if b.len() >= 4 && matches!(b[0], b'\\' | b'/') && b[2] == b'?' && matches!(b[3], b'\\' | b'/') { 4 } else { 0 };
    (b.len() >= at + 2 && b[at].is_ascii_alphabetic() && b[at + 1] == b':').then(|| format!("{}:\\", b[at] as char))
}

/// Whether `path` is on a mapped network drive (`GetDriveTypeW`, no network I/O).
fn on_remote_drive(path: &Path, drives: &dyn crate::netpath::Drives) -> bool {
    drive_root(&path.to_string_lossy()).is_some_and(|root| drives.is_remote(&root))
}

/// Whether any folder or the file of `path` is a Win32 DOS device name (CON, PRN, AUX, NUL, CONIN$,
/// CONOUT$, COM1-9, LPT1-9, with any extension, trailing spaces or colon): Windows opens the device,
/// not a file, and an LPTn or COMn may be redirected to a share.
pub fn is_device_name(path: &Path) -> bool {
    path.components().any(|c| {
        let std::path::Component::Normal(name) = c else { return false };
        let name = name.to_string_lossy();
        let stem = name.split('.').next().unwrap_or("").trim_end_matches([' ', ':']).to_ascii_uppercase();
        matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
            || ["COM", "LPT"].iter().any(|p| stem.strip_prefix(p).is_some_and(|d| d.len() == 1 && matches!(d.as_bytes()[0], b'1'..=b'9')))
    })
}

/// `link_chain_end` that also refuses a hop landing on a mapped drive.
pub fn link_chain_end_on(start: &Path, read_link: &dyn Fn(&Path) -> Option<PathBuf>, drives: &dyn crate::netpath::Drives) -> Option<PathBuf> {
    let mut at = start.to_path_buf();
    for _ in 0..LINK_HOPS {
        let Some(target) = read_link(&at) else {
            // Not a link any more: the end of the chain.
            return Some(at);
        };
        let target = match at.parent() {
            Some(parent) if target.is_relative() => parent.join(target),
            _ => target,
        };
        if is_network_path(&target.to_string_lossy()) || on_remote_drive(&target, drives) {
            return None;
        }
        at = target;
    }
    None
}

/// Whether a symbolic link or junction leads to a local folder. Each hop of the link is read
/// first, so a link to a network path is never followed (and never probed).
fn link_is_local_folder(link: &Path) -> bool {
    link_chain_end(link, &read_link).is_some_and(|end| end.is_dir())
}

/// Whether `path` is safe to touch as a file without a person asking: not a network path, not
/// on a mapped drive, and no folder from the root down (nor the file) a link whose chain reaches
/// a network path or a mapped drive (or never ends). Reads only the links themselves.
pub fn stays_local(path: &Path) -> bool {
    stays_local_with(path, &read_link, &*crate::netpath::real_drives())
}

/// `stays_local` with the link reader and the drive types given (the real ones, or a test's).
pub fn stays_local_with(path: &Path, read_link: &dyn Fn(&Path) -> Option<PathBuf>, drives: &dyn crate::netpath::Drives) -> bool {
    ancestors_stay_local_with(path, read_link, drives)
}

/// Whether a folder can be listed without a person asking: `stays_local` for it and for
/// every folder above it, since any of them may be a link or junction to a network path
/// (a Scripts folder, or `%APPDATA%` itself, redirected to a share). Reads only the links.
pub fn ancestors_stay_local(path: &Path, read_link: &dyn Fn(&Path) -> Option<PathBuf>) -> bool {
    ancestors_stay_local_with(path, read_link, &*crate::netpath::real_drives())
}

/// The one local-path rule: the text rule and the drive type first (nothing is read for a path
/// that fails them), then every folder ROOT FIRST, so a junction is read before anything goes
/// through it (a read of `C:\j\f` with `C:\j` a junction to a share would reach the share).
pub fn ancestors_stay_local_with(path: &Path, read_link: &dyn Fn(&Path) -> Option<PathBuf>, drives: &dyn crate::netpath::Drives) -> bool {
    if is_network_path(&path.to_string_lossy()) || on_remote_drive(path, drives) || is_device_name(path) {
        return false;
    }
    let mut chain: Vec<&Path> = path.ancestors().filter(|a| !a.as_os_str().is_empty()).collect();
    chain.reverse();
    chain.into_iter().all(|a| link_chain_end_on(a, read_link, drives).is_some())
}

/// Logged in the app-wide entry when AutoLaunch is skipped because its folder is reached
/// through a link to a network path (Tandem's).
pub fn autolaunch_not_local(folder: &Path) -> String {
    format!("AutoLaunch skipped: {} is reached through a link to a network path.\n", folder.display())
}

/// Logged in the app-wide entry when the Scripts folder is reached through a link to a
/// network path: it is not made, watched, scanned or cleaned up (Tandem's).
pub fn scripts_folder_not_local(folder: &Path) -> String {
    format!("Scripts folder skipped: {} is reached through a link to a network path.\n", folder.display())
}

/// A path Tandem never touches as a file: a network path or a device path. `/` and `\` are
/// alike, as Windows reads them: any two leading separators (`\\host`, `//host`, `\/host`,
/// `/\host`) name a share or a device (`\\?\UNC\`, `\\.\`), and the NT prefix `\??\`
/// reaches the same. Only the long-path forms of a drive, `\\?\C:\` and `\??\C:\`, are
/// ordinary paths.
pub fn is_network_path(text: &str) -> bool {
    let sep = |c: u8| c == b'\\' || c == b'/';
    let b = text.as_bytes();
    let drive_follows = |at: usize| b.len() >= at + 2 && b[at].is_ascii_alphabetic() && b[at + 1] == b':';
    // `\\?\` or `\??\` (either separator): local only before a drive letter.
    let long = b.len() >= 4 && sep(b[0]) && sep(b[3]) && b[2] == b'?' && (sep(b[1]) || b[1] == b'?');
    if long {
        return !drive_follows(4);
    }
    b.len() >= 2 && sep(b[0]) && sep(b[1])
}

/// `%APPDATA%\Tandem\Scripts`, iTerm2's `<Application Support>/iTerm2/Scripts`.
pub fn default_folder(appdata: &Path) -> PathBuf {
    appdata.join("Tandem").join(SCRIPTS_DIR)
}

/// `stringByExpandingTildeInPath`: a leading `~` alone or before a separator is the home folder.
pub fn expand_tilde(text: &str, home: &Path) -> PathBuf {
    if text == "~" {
        return home.to_path_buf();
    }
    match text.strip_prefix("~\\").or_else(|| text.strip_prefix("~/")) {
        Some(rest) => home.join(rest),
        None => PathBuf::from(text),
    }
}

/// `customScriptsFolderIsValid:` (`FM:228-234`): tilde-expanded, no space, an existing folder.
/// A network or device path (`is_network_path`), or one reached through a link to one
/// (`ancestors_stay_local`), is never probed and never valid.
pub fn custom_folder_valid(text: &str, home: &Path, fs: &dyn Fs) -> bool {
    if text.is_empty() || is_network_path(text) {
        return false;
    }
    let candidate = expand_tilde(text, home);
    !candidate.to_string_lossy().contains(' ') && ancestors_stay_local(&candidate, &|p: &Path| fs.read_link(p)) && fs.is_dir(&candidate)
}

/// `scriptsPath` (`FM:236-252`): the custom folder when it is on and valid, else the default.
pub fn scripts_folder(use_custom: bool, custom: &str, appdata: &Path, home: &Path, fs: &dyn Fs) -> PathBuf {
    if use_custom && custom_folder_valid(custom, home, fs) {
        expand_tilde(custom, home)
    } else {
        default_folder(appdata)
    }
}

/// `autolaunchScriptPath` (`FM:270-272`).
pub fn autolaunch_folder(scripts: &Path) -> PathBuf {
    scripts.join(AUTOLAUNCH_DIR)
}

/// A folder that runs as one script: a uv environment, `.venv` with its interpreter plus the
/// `python-runtime.json` marker (`SR:98-107`, Windows' `Scripts\python.exe`). Its main
/// `<dir>\<name>\<name>.py` is not checked here (`checkForMain:NO`, `M:522-525`).
pub fn is_full_environment(dir: &Path, fs: &dyn Fs) -> bool {
    fs.is_file(&dir.join(".venv").join("Scripts").join("python.exe")) && fs.is_file(&dir.join("python-runtime.json"))
}

/// One Scripts menu item: a script (a `.py` file or a full environment) or a folder of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    Script { name: String, path: PathBuf, full_environment: bool },
    Folder { name: String, path: PathBuf, children: Vec<Node> },
}

impl Node {
    pub fn name(&self) -> &str {
        match self {
            Node::Script { name, .. } | Node::Folder { name, .. } => name,
        }
    }

    fn is_folder(&self) -> bool {
        matches!(self, Node::Folder { .. })
    }
}

/// A scan's findings: the tree, the `.its` archives with whether each is in the AutoLaunch
/// folder, and whether the person stopped it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanResult {
    pub tree: Vec<Node>,
    pub archives: Vec<(PathBuf, bool)>,
    pub stopped: bool,
}

/// `populateScriptItem:` (`M:476-545`) from `root`, sorted. `elapsed` is read before each entry;
/// once it reaches the limit `slow` is asked: true extends the limit to that time plus
/// `SCAN_EXTEND`, false stops, keeping what the finished folders held.
/// A root reached through a link to a network path is never listed: the result is empty.
pub fn scan(root: &Path, fs: &dyn Fs, elapsed: &mut dyn FnMut() -> Duration, slow: &mut dyn FnMut() -> bool) -> ScanResult {
    if !ancestors_stay_local(root, &|p: &Path| fs.read_link(p)) {
        return ScanResult::default();
    }
    let mut walk = Walk { fs, elapsed, slow, limit: SCAN_LIMIT, stopped: false, archives: Vec::new(), walking: Vec::new() };
    let mut tree = walk.folder(root, false, true);
    sort(&mut tree);
    let mut archives = walk.archives;
    archives.sort();
    ScanResult { tree, archives, stopped: walk.stopped }
}

struct Walk<'a> {
    fs: &'a dyn Fs,
    elapsed: &'a mut dyn FnMut() -> Duration,
    slow: &'a mut dyn FnMut() -> bool,
    limit: Duration,
    stopped: bool,
    archives: Vec<(PathBuf, bool)>,
    /// The real paths of the folders being walked, the root first: a link back to one of
    /// them is not entered again.
    walking: Vec<PathBuf>,
}

impl Walk<'_> {
    fn folder(&mut self, dir: &Path, autolaunch: bool, top: bool) -> Vec<Node> {
        let real = self.fs.canonical(dir);
        if self.walking.len() > MAX_DEPTH || real.as_ref().is_some_and(|r| self.walking.contains(r)) {
            return Vec::new();
        }
        self.walking.push(real.unwrap_or_else(|| dir.to_path_buf()));
        let out = self.entries(dir, autolaunch, top);
        self.walking.pop();
        out
    }

    fn entries(&mut self, dir: &Path, autolaunch: bool, top: bool) -> Vec<Node> {
        let mut entries = self.fs.list(dir);
        // The listing's order is the file system's; a fixed order keeps a stopped scan's
        // result the same from run to run.
        entries.sort();
        let mut out = Vec::new();
        for (name, is_folder) in entries {
            let now = (self.elapsed)();
            if now >= self.limit {
                if (self.slow)() {
                    self.limit = now + SCAN_EXTEND;
                } else {
                    self.stopped = true;
                    return out;
                }
            }
            // Hidden entries are skipped and hidden folders never entered (`M:511-517`).
            if name.starts_with('.') {
                continue;
            }
            let path = dir.join(&name);
            if is_folder {
                if is_full_environment(&path, self.fs) {
                    out.push(Node::Script { name, path, full_environment: true });
                    continue;
                }
                let children = self.folder(&path, top && name == AUTOLAUNCH_DIR, false);
                if self.stopped {
                    return out;
                }
                if !children.is_empty() {
                    out.push(Node::Folder { name, path, children });
                }
            } else {
                match path.extension().and_then(|e| e.to_str()) {
                    Some("py") => out.push(Node::Script { name, path, full_environment: false }),
                    Some("its") => self.archives.push((path, autolaunch)),
                    _ => {}
                }
            }
        }
        out
    }
}

/// iTerm2's item order (`M:68-77`): folders first, then names ignoring case.
pub fn sort(nodes: &mut [Node]) {
    nodes.sort_by(|a, b| b.is_folder().cmp(&a.is_folder()).then_with(|| a.name().to_lowercase().cmp(&b.name().to_lowercase())).then_with(|| a.name().cmp(b.name())));
    for n in nodes.iter_mut() {
        if let Node::Folder { children, .. } = n {
            sort(children);
        }
    }
}

/// `allScripts` (`M:197-215`): every script's menu title with its folders, joined by "/".
pub fn all_scripts(nodes: &[Node]) -> Vec<String> {
    let mut out = Vec::new();
    collect(nodes, "", &mut out);
    out
}

fn collect(nodes: &[Node], prefix: &str, out: &mut Vec<String>) {
    for n in nodes {
        match n {
            Node::Script { name, .. } => out.push(format!("{prefix}{name}")),
            Node::Folder { name, children, .. } => collect(children, &format!("{prefix}{name}/"), out),
        }
    }
}

/// The mark beside the custom folder field and its alpha (`GP:4204-4219`): a check when the
/// text is a valid folder, else a warning; full when the setting is on, half when it is off
/// with text, none when it is off and empty.
pub fn folder_glyph(use_custom: bool, text: &str, valid: bool) -> Option<(char, f32)> {
    let alpha = if use_custom {
        1.0
    } else if !text.is_empty() {
        0.5
    } else {
        return None;
    };
    Some((if valid { CHECK_GLYPH } else { WARNING_GLYPH }, alpha))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[derive(Default)]
    struct FakeFs {
        files: BTreeSet<PathBuf>,
        dirs: BTreeSet<PathBuf>,
        /// Folder links: the link's path and the folder it leads to.
        links: std::collections::BTreeMap<PathBuf, PathBuf>,
    }

    impl FakeFs {
        /// Adds a file and every folder above it up to `root`.
        fn file(&mut self, root: &Path, rel: &str) {
            let p = root.join(rel);
            self.folders_above(root, &p);
            self.files.insert(p);
        }
        fn dir(&mut self, root: &Path, rel: &str) {
            let p = root.join(rel);
            self.folders_above(root, &p);
            self.dirs.insert(p);
        }
        fn folders_above(&mut self, root: &Path, p: &Path) {
            let mut up = p.parent();
            while let Some(d) = up {
                self.dirs.insert(d.to_path_buf());
                if d == root {
                    break;
                }
                up = d.parent();
            }
        }
    }

    impl FakeFs {
        /// A path with its first link resolved (the fake's links do not chain).
        fn resolve(&self, p: &Path) -> PathBuf {
            for (link, target) in &self.links {
                if let Ok(rest) = p.strip_prefix(link) {
                    return target.join(rest);
                }
            }
            p.to_path_buf()
        }
    }

    impl Fs for FakeFs {
        fn list(&self, dir: &Path) -> Vec<(String, bool)> {
            let dir = &self.resolve(dir);
            let name = |p: &PathBuf| p.file_name().unwrap().to_string_lossy().into_owned();
            // Reversed, so the scan's own order is what the tests see.
            let mut out: Vec<(String, bool)> = self.files.iter().filter(|p| p.parent() == Some(dir)).map(|p| (name(p), false)).collect();
            out.extend(self.dirs.iter().filter(|p| p.parent() == Some(dir)).map(|p| (name(p), true)));
            out.extend(self.links.keys().filter(|p| p.parent() == Some(dir)).map(|p| (name(p), true)));
            out.reverse();
            out
        }
        fn is_file(&self, p: &Path) -> bool {
            self.files.contains(&self.resolve(p))
        }
        fn is_dir(&self, p: &Path) -> bool {
            self.dirs.contains(&self.resolve(p))
        }
        fn canonical(&self, dir: &Path) -> Option<PathBuf> {
            let real = self.resolve(dir);
            self.dirs.contains(&real).then_some(real)
        }
        fn read_link(&self, p: &Path) -> Option<PathBuf> {
            self.links.get(p).cloned()
        }
    }

    fn never() -> impl FnMut() -> Duration {
        || Duration::ZERO
    }

    fn script(path: &Path, full: bool) -> Node {
        Node::Script { name: path.file_name().unwrap().to_string_lossy().into_owned(), path: path.to_path_buf(), full_environment: full }
    }

    fn folder(path: &Path, children: Vec<Node>) -> Node {
        Node::Folder { name: path.file_name().unwrap().to_string_lossy().into_owned(), path: path.to_path_buf(), children }
    }

    #[test]
    fn scripts_folder_follows_iterm2() {
        let appdata = Path::new(r"C:\Users\u\AppData\Roaming");
        let home = Path::new(r"C:\Users\u");
        let default = PathBuf::from(r"C:\Users\u\AppData\Roaming\Tandem\Scripts");
        assert_eq!(default_folder(appdata), default);
        let mut fs = FakeFs::default();
        fs.dirs.insert(PathBuf::from(r"D:\s"));
        fs.dirs.insert(PathBuf::from(r"D:\my s"));
        fs.dirs.insert(PathBuf::from(r"C:\Users\u\s"));
        assert!(custom_folder_valid(r"D:\s", home, &fs));
        // A space makes it invalid even when the folder exists.
        assert!(!custom_folder_valid(r"D:\my s", home, &fs));
        assert!(!custom_folder_valid("", home, &fs));
        assert!(!custom_folder_valid(r"D:\missing", home, &fs));
        assert!(custom_folder_valid(r"~\s", home, &fs));
        assert!(custom_folder_valid("~/s", home, &fs));
        assert_eq!(expand_tilde("~", home), home);
        assert_eq!(expand_tilde(r"D:\~s", home), PathBuf::from(r"D:\~s"));
        // A network or device path is never probed, whichever separators spell it; the local
        // long-path forms are paths.
        for p in [r"\\host\s", "//host/s", r"\\?\UNC\host\s", r"\\.\D:\s", r"\/host/s", r"/\host\s", r"\??\UNC\host\s", r"\\.\UNC\host\s", r"//?/UNC/host/s", r"\\?\Volume{0}\s", r"\??\"] {
            fs.dirs.insert(PathBuf::from(p));
            assert!(!custom_folder_valid(p, home, &fs), "{p}");
            assert!(is_network_path(p), "{p}");
        }
        fs.dirs.insert(PathBuf::from(r"\\?\D:\s"));
        assert!(custom_folder_valid(r"\\?\D:\s", home, &fs));
        assert!(!is_network_path(r"\\?\D:\s") && !is_network_path(r"D:\s"));
        assert!(!is_network_path(r"\??\D:\s") && !is_network_path(r"\s") && !is_network_path("s") && !is_network_path(""));
        assert_eq!(scripts_folder(true, r"D:\s", appdata, home, &fs), PathBuf::from(r"D:\s"));
        assert_eq!(scripts_folder(true, r"~\s", appdata, home, &fs), PathBuf::from(r"C:\Users\u\s"));
        assert_eq!(scripts_folder(false, r"D:\s", appdata, home, &fs), default);
        assert_eq!(scripts_folder(true, r"D:\my s", appdata, home, &fs), default);
        assert_eq!(scripts_folder(true, r"D:\missing", appdata, home, &fs), default);
        assert_eq!(autolaunch_folder(Path::new(r"D:\s")), PathBuf::from(r"D:\s\AutoLaunch"));
    }

    #[test]
    fn custom_folder_through_a_network_link_is_never_probed() {
        let home = Path::new(r"C:\Users\u");
        let mut fs = FakeFs::default();
        // The folder itself, or a folder above it, is a link to a share the fake would list.
        fs.links.insert(PathBuf::from(r"D:\s"), PathBuf::from(r"\\h\share"));
        fs.links.insert(PathBuf::from(r"D:\l"), PathBuf::from(r"\\h\x"));
        fs.dirs.insert(PathBuf::from(r"\\h\share"));
        fs.dirs.insert(PathBuf::from(r"\\h\x\sub"));
        assert!(!custom_folder_valid(r"D:\s", home, &fs));
        assert!(!custom_folder_valid(r"D:\l\sub", home, &fs));
        let appdata = Path::new(r"C:\A");
        assert_eq!(scripts_folder(true, r"D:\s", appdata, home, &fs), default_folder(appdata));
        // A link to a local folder stays valid.
        fs.links.insert(PathBuf::from(r"D:\ok"), PathBuf::from(r"E:\real"));
        fs.dirs.insert(PathBuf::from(r"E:\real"));
        assert!(custom_folder_valid(r"D:\ok", home, &fs));
        assert_eq!(scripts_folder_not_local(Path::new(r"C:\S")), "Scripts folder skipped: C:\\S is reached through a link to a network path.\n");
    }

    fn tree_fs(root: &Path, with_marker: bool) -> FakeFs {
        let mut fs = FakeFs::default();
        for f in ["b.py", "A.py", ".hidden.py", "notes.txt", "x.scpt", "pack.its", r"zeta\z.py", r"Empty\readme.md", r".git\c.py", r"__pycache__\m.cpython-312.pyc", r"AutoLaunch\run.py", r"AutoLaunch\auto.its", r"Full\Full\Full.py", r"Full\setup.cfg", r"Full\.venv\Scripts\python.exe"] {
            fs.file(root, f);
        }
        if with_marker {
            fs.file(root, r"Full\python-runtime.json");
        }
        fs.dir(root, "Tool.app");
        fs
    }

    #[test]
    fn script_tree_follows_iterm2() {
        let root = Path::new(r"C:\S");
        let fs = tree_fs(root, true);
        let r = scan(root, &fs, &mut never(), &mut || panic!("not slow"));
        let expected = vec![
            folder(&root.join("AutoLaunch"), vec![script(&root.join(r"AutoLaunch\run.py"), false)]),
            folder(&root.join("zeta"), vec![script(&root.join(r"zeta\z.py"), false)]),
            script(&root.join("A.py"), false),
            script(&root.join("b.py"), false),
            script(&root.join("Full"), true),
        ];
        assert_eq!(r.tree, expected);
        assert_eq!(r.archives, vec![(root.join(r"AutoLaunch\auto.its"), true), (root.join("pack.its"), false)]);
        assert!(!r.stopped);
        assert_eq!(all_scripts(&r.tree), ["AutoLaunch/run.py", "zeta/z.py", "A.py", "b.py", "Full"]);
        assert_eq!(r.tree[4].name(), "Full");

        // Without its marker the environment is gone: its folders are scanned as folders.
        let fs = tree_fs(root, false);
        let r = scan(root, &fs, &mut never(), &mut || false);
        let full = folder(&root.join("Full"), vec![folder(&root.join(r"Full\Full"), vec![script(&root.join(r"Full\Full\Full.py"), false)])]);
        assert_eq!(r.tree[1], full);
        assert_eq!(all_scripts(&r.tree), ["AutoLaunch/run.py", "Full/Full/Full.py", "zeta/z.py", "A.py", "b.py"]);

        // An `.its` in a folder named AutoLaunch below the top is not the AutoLaunch folder's.
        let mut fs = FakeFs::default();
        fs.file(root, r"sub\AutoLaunch\deep.its");
        let r = scan(root, &fs, &mut never(), &mut || false);
        assert_eq!(r.archives, vec![(root.join(r"sub\AutoLaunch\deep.its"), false)]);
        assert!(r.tree.is_empty());
    }

    #[test]
    fn links_are_followed_without_loops() {
        let root = Path::new(r"C:\S");
        let mut fs = FakeFs::default();
        fs.file(root, r"real\r.py");
        fs.dirs.insert(root.to_path_buf());
        // A link to a folder is entered as the folder is (`M:519-529`); a link back to a
        // folder being walked is not entered again.
        fs.links.insert(root.join("alias"), root.join("real"));
        fs.links.insert(root.join(r"real\up"), root.to_path_buf());
        let r = scan(root, &fs, &mut never(), &mut || false);
        assert_eq!(all_scripts(&r.tree), ["alias/r.py", "real/r.py"]);
        // Past MAX_DEPTH folders nothing more is read.
        let mut fs = FakeFs::default();
        let deep = (0..MAX_DEPTH + 2).map(|i| format!("d{i}")).collect::<Vec<_>>().join("\\");
        fs.file(root, &format!(r"{deep}\deep.py"));
        fs.file(root, r"d0\shallow.py");
        let r = scan(root, &fs, &mut never(), &mut || false);
        assert_eq!(all_scripts(&r.tree), ["d0/shallow.py"]);
    }

    #[test]
    fn roots_never_follow_network_links() {
        // The Scripts folder itself, or a folder above it, a link or junction to a share: the
        // scan lists nothing there.
        let root = Path::new(r"C:\Users\u\AppData\Roaming\Tandem\Scripts");
        let mut fs = FakeFs::default();
        fs.file(root, "a.py");
        assert_eq!(all_scripts(&scan(root, &fs, &mut never(), &mut || false).tree), ["a.py"]);
        for link in [r"C:\Users\u\AppData\Roaming\Tandem\Scripts", r"C:\Users\u\AppData"] {
            let mut fs = FakeFs::default();
            fs.file(root, "a.py");
            fs.links.insert(PathBuf::from(link), PathBuf::from(r"\\server\share"));
            assert_eq!(scan(root, &fs, &mut never(), &mut || panic!("not slow")), ScanResult::default(), "{link}");
        }

        // The same rule for the AutoLaunch folder: a link on it or above it that reaches a
        // network path, straight or on a later hop, or that loops.
        let links: std::collections::HashMap<PathBuf, PathBuf> = [
            (r"C:\S1\AutoLaunch", r"\\server\x"),
            (r"C:\S2", r"C:\hop"),
            (r"C:\hop", r"\\?\UNC\srv\y"),
            (r"C:\S3", r"C:\loop"),
            (r"C:\loop", r"C:\S3"),
            (r"C:\S4", r"D:\scripts"),
        ]
        .into_iter()
        .map(|(a, b)| (PathBuf::from(a), PathBuf::from(b)))
        .collect();
        let read = |p: &Path| links.get(p).cloned();
        let local = |p: &str| ancestors_stay_local(&autolaunch_folder(Path::new(p)), &read);
        assert!(!local(r"C:\S1"));
        assert!(!local(r"C:\S2"));
        assert!(!local(r"C:\S3"));
        assert!(local(r"C:\S4"));
        assert!(local(r"C:\S5"));
        assert!(!ancestors_stay_local(Path::new(r"\\server\s\AutoLaunch"), &|_| panic!("a network path is never read")));
        assert_eq!(autolaunch_not_local(Path::new(r"C:\S1\AutoLaunch")), "AutoLaunch skipped: C:\\S1\\AutoLaunch is reached through a link to a network path.\n");
    }

    /// Review fix: the ONE rule also refuses a mapped drive and a junction anywhere above the file
    /// that lands on a share or a mapped drive, reading root first so nothing is read through it.
    /// The drive types and links are injected; the rule itself runs for real.
    struct Remote(&'static [char]);
    impl crate::netpath::Drives for Remote {
        fn is_remote(&self, root: &str) -> bool {
            root.chars().next().is_some_and(|c| self.0.contains(&c.to_ascii_uppercase()))
        }
    }

    #[test]
    fn the_one_rule_refuses_mapped_drives_and_ancestor_links() {
        let drives = Remote(&['Z']);
        let none = |_: &Path| None;
        assert!(stays_local_with(Path::new(r"C:\a\s.ps1"), &none, &drives));
        for p in [r"Z:\a.ps1", r"z:\a\b.ps1", r"\\?\Z:\a.ps1", r"\??\Z:\a.ps1"] {
            assert!(!stays_local_with(Path::new(p), &none, &drives), "{p}");
            assert!(!ancestors_stay_local_with(Path::new(p), &none, &drives), "{p}");
        }
        // A junction above the file to a share, to a mapped drive, or through a chain to one.
        let links: std::collections::HashMap<PathBuf, PathBuf> =
            [(r"C:\j", r"\\attacker\share"), (r"C:\m", r"Z:\x"), (r"C:\c", r"C:\hop"), (r"C:\hop", r"\\?\UNC\h\s"), (r"C:\ok", r"D:\real")].into_iter().map(|(a, b)| (PathBuf::from(a), PathBuf::from(b))).collect();
        let reads = std::cell::RefCell::new(Vec::new());
        let read = |p: &Path| {
            reads.borrow_mut().push(p.to_path_buf());
            links.get(p).cloned()
        };
        for p in [r"C:\j\s.ps1", r"C:\j\deep\er\s.ps1", r"C:\m\s.ps1", r"C:\c\s.ps1"] {
            reads.borrow_mut().clear();
            assert!(!stays_local_with(Path::new(p), &read, &drives), "{p}");
            assert!(!reads.borrow().iter().any(|r| r.to_string_lossy().ends_with("s.ps1")), "{p}: the file was read through the link: {:?}", reads.borrow());
            assert!(!ancestors_stay_local_with(Path::new(p), &read, &drives), "{p}");
        }
        assert!(stays_local_with(Path::new(r"C:\ok\s.ps1"), &read, &drives), "a junction to a local folder is fine");
        // A relative link target is joined to its folder before the check.
        let rel = |p: &Path| (p == Path::new(r"C:\r")).then(|| PathBuf::from(r"..\..\x"));
        assert!(stays_local_with(Path::new(r"C:\r\f"), &rel, &drives));
        // The wrappers used by every caller ask the real drive types, no network I/O.
        assert!(stays_local(Path::new(r"C:\Windows\notepad.exe")));
    }

    /// Review fix 4: a Win32 DOS device name (CON, PRN, AUX, NUL, COM1-9, LPT1-9, with any
    /// extension, in any folder of the path) is never a local file: opening it would touch a
    /// device, possibly one redirected to a share.
    #[test]
    fn dos_device_names_are_never_local_files() {
        let drives = Remote(&[]);
        let none = |_: &Path| None;
        for p in [r"C:\x\NUL", r"C:\x\nul.ps1", r"C:\x\Con.txt", r"C:\x\aux.tar.gz", r"C:\x\LPT1", r"C:\x\com9.ps1", r"C:\x\prn .ps1", r"C:\x\NUL:", r"C:\x\lpt3\f.ps1", r"CON", r"C:\x\CONIN$", r"C:\x\conout$.ps1"] {
            assert!(is_device_name(Path::new(p)), "{p}");
            assert!(!stays_local_with(Path::new(p), &none, &drives), "{p}");
        }
        for p in [r"C:\x\console.ps1", r"C:\x\COM0", r"C:\x\COM10", r"C:\x\LPT", r"C:\x\nulx", r"C:\x\a.nul", r"C:\x\s.ps1"] {
            assert!(!is_device_name(Path::new(p)), "{p}");
            assert!(stays_local_with(Path::new(p), &none, &drives), "{p}");
        }
    }

    #[test]
    fn slow_scan_follows_iterm2() {
        let root = Path::new(r"C:\S");
        let mut fs = FakeFs::default();
        for i in 0..10 {
            fs.file(root, &format!(r"a\a{i}.py"));
            fs.file(root, &format!(r"b\b{i}.py"));
        }
        fs.file(root, "top.py");
        // Each read of the clock is 3 s later than the last.
        let clock = |t: &std::cell::Cell<u64>| {
            t.set(t.get() + 3);
            Duration::from_secs(t.get())
        };

        // Asked first when the clock reaches 8 s (at 9), then only 5 s after each Continue.
        let t = std::cell::Cell::new(0);
        let asked = std::cell::RefCell::new(Vec::new());
        let r = scan(root, &fs, &mut || clock(&t), &mut || {
            asked.borrow_mut().push(t.get());
            true
        });
        let asked = asked.into_inner();
        assert_eq!(&asked[..3], [9, 15, 21]);
        assert!(!r.stopped);
        assert_eq!(all_scripts(&r.tree).len(), 21);

        // Stop keeps the folders that were finished: the sixth question (at 39 s) comes at
        // `b`'s first entry, so `a` stays and `b`, which was being read, goes.
        let t = std::cell::Cell::new(0);
        let mut n = 0;
        let r = scan(root, &fs, &mut || clock(&t), &mut || {
            n += 1;
            n < 6
        });
        assert!(r.stopped);
        assert_eq!(r.tree.len(), 1);
        assert_eq!(r.tree[0].name(), "a");
        assert_eq!(all_scripts(&r.tree).len(), 10);

        assert_eq!(
            slow_text(Path::new(r"C:\S")),
            "It is taking a long time to locate all scripts under C:\\S. Avoid storing many files or using network mounts for the scripts folder.\n\nContinue?"
        );
        assert_eq!((SLOW_HEADING, STOP, CONTINUE), ("Performance Issue", "Stop", "Continue"));
        assert_eq!(DISABLED_HEADING, "Scripts Disabled");
        assert_eq!(DISABLED_TEXT, "Some scripts will not be available until the app has restarted or you change the scripts folder.");
        assert_eq!(archive_text("pack.its"), "A script archive named \u{201c}pack.its\u{201d} was found in the Scripts directory. Would you like to install it?");
        assert_eq!((ARCHIVE_HEADING, MOVE_TO_TRASH, ARCHIVE_ID), ("Install Script Archive?", "Move to Trash", "NoSyncInstallScriptArchive"));
        assert_eq!((SCAN_LIMIT, SCAN_EXTEND), (Duration::from_secs(8), Duration::from_secs(5)));
        assert_eq!((SCRIPTS_DIR, AUTOLAUNCH_DIR), ("Scripts", "AutoLaunch"));
    }

    #[test]
    fn folder_glyph_follows_iterm2() {
        assert_eq!(folder_glyph(true, "x", true), Some((CHECK_GLYPH, 1.0)));
        assert_eq!(folder_glyph(true, "x", false), Some((WARNING_GLYPH, 1.0)));
        assert_eq!(folder_glyph(false, "x", false), Some((WARNING_GLYPH, 0.5)));
        assert_eq!(folder_glyph(false, "x", true), Some((CHECK_GLYPH, 0.5)));
        assert_eq!(folder_glyph(false, "", false), None);
        assert_eq!(folder_glyph(true, "", false), Some((WARNING_GLYPH, 1.0)));
    }
}

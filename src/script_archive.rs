//! Open Python REPL's banner and arguments (`AD:3528-3551`, `API/iTermReplLauncher.swift`),
//! and import and export (`IM`, `AR`, `EX`): which layouts an archive may have, the export's
//! zip name and the tar.exe arguments that write and read it, the replace backup's names and
//! the texts. Pure.

use std::path::{Path, PathBuf};

use crate::scripts::Fs;

/// iTerm2's banner as vendored, byte for byte (`sources/repl_banner.txt` at `64791fc`).
const BANNER: &str = include_str!("../resources/scripts/repl_banner.txt");

/// The banner's SHA-256: iTerm2's file at `64791fc`.
pub const BANNER_SHA256: &str = "6ea333be584b6ecb6343843868404c8f8bd9bc214aad0807973af69140734637";

/// The banner fed to the REPL's pane before the interpreter writes: iTerm2's, naming Tandem
/// and Win-click (the link's URL stays iTerm2's).
pub fn repl_banner() -> String {
    BANNER.replace("\u{2318}-click", "Win-click").replace("iTerm2", "Tandem")
}

/// The interpreter's arguments under uv (`iTermReplLauncher.arguments(usesUV:)`): the
/// standard library's asyncio REPL, which allows a top-level `await`.
pub fn repl_args() -> [&'static str; 2] {
    ["-m", "asyncio"]
}

/// The REPL's Script Console entry name.
pub const REPL_NAME: &str = "REPL";

/// What an extracted archive holds (`AR:68-136`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Layout {
    /// One `.py` file.
    Basic { file: PathBuf },
    /// A full environment: the folder moved into the Scripts folder (iTerm2's `from`, the
    /// temporary folder joined with the inner folder's name), its name, and whether its
    /// `metadata.json` asks for AutoLaunch.
    Full { folder: PathBuf, name: String, autolaunch: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayoutError {
    NoValidScript,
    /// A `setup.py`: an archive from before `setup.cfg`.
    Legacy,
}

pub const SETUP_CFG: &str = "setup.cfg";
const SETUP_PY: &str = "setup.py";
pub const METADATA: &str = "metadata.json";

/// A folder's entries without the hidden ones, sorted by name (iTerm2 takes the file
/// system's order; Decision).
fn visible(fs: &dyn Fs, dir: &Path) -> Vec<(String, bool)> {
    let mut v: Vec<(String, bool)> = fs.list(dir).into_iter().filter(|(n, _)| !n.starts_with('.')).collect();
    v.sort();
    v
}

/// `archiveFromContainer:deprecated:` (`AR:68-136`) over the folder the archive was extracted
/// into: exactly one visible item, a `.py` file (basic) or a folder with at least two
/// visible items: `setup.cfg` (required), `metadata.json` (optional), exactly one folder, and
/// nothing else (a `setup.py` is a legacy archive), whose `<name>\<name>.py` exists. `read`
/// reads `metadata.json`.
pub fn layout(temp: &Path, fs: &dyn Fs, read: &dyn Fn(&Path) -> Option<String>) -> Result<Layout, LayoutError> {
    let top = visible(fs, temp);
    let [(top_name, top_dir)] = top.as_slice() else { return Err(LayoutError::NoValidScript) };
    let top_item = temp.join(top_name);
    if !top_dir && Path::new(top_name).extension().is_some_and(|e| e == "py") {
        return Ok(Layout::Basic { file: top_item });
    }
    if !top_dir {
        return Err(LayoutError::NoValidScript);
    }
    let inner = visible(fs, &top_item);
    if inner.len() < 2 {
        return Err(LayoutError::NoValidScript);
    }
    let (mut setup_cfg, mut folder) = (false, None);
    for (name, dir) in &inner {
        if name == SETUP_CFG {
            setup_cfg = true;
        } else if name == METADATA {
        } else if folder.is_none() && *dir {
            folder = Some(name.clone());
        } else if name == SETUP_PY {
            return Err(LayoutError::Legacy);
        } else {
            return Err(LayoutError::NoValidScript);
        }
    }
    let (Some(name), true) = (folder, setup_cfg) else { return Err(LayoutError::NoValidScript) };
    if !fs.is_file(&top_item.join(&name).join(format!("{name}.py"))) {
        return Err(LayoutError::NoValidScript);
    }
    // iTerm2 reads the metadata and installs from the temporary folder joined with the inner
    // folder's name (`AR:37-54`, `AR:278`); an archive whose names differ fails later.
    let folder = temp.join(&name);
    Ok(Layout::Full { autolaunch: autolaunch_of(read(&folder.join(METADATA)).as_deref()), folder, name })
}

/// `wantsAutoLaunch` (`AR:138-140`): the metadata's `AutoLaunch` as a boolean.
fn autolaunch_of(metadata: Option<&str>) -> bool {
    metadata.and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok()).and_then(|v| v.get("AutoLaunch").and_then(|a| a.as_bool().or_else(|| a.as_i64().map(|n| n != 0)))).unwrap_or(false)
}

/// `urlForNewZipFileInFolder:name:extension:` (`EX:21-35`): `<name>.zip`, else
/// `<name> (2).zip`, `<name> (3).zip` and on.
pub fn export_name(folder: &Path, name: &str, exists: &dyn Fn(&Path) -> bool) -> PathBuf {
    let mut count = 1;
    loop {
        let file = if count == 1 { format!("{name}.zip") } else { format!("{name} ({count}).zip") };
        let p = folder.join(file);
        if !exists(&p) {
            return p;
        }
        count += 1;
    }
}

/// tar.exe's arguments writing the export's zip (`EX:125-133`, `zip -r` relative to the
/// parent): `setup.cfg` and the inner folder, never the metadata (only a signed archive
/// holds it).
pub fn export_tar_args(out: &Path, parent: &Path, name: &str) -> Vec<String> {
    let s = |p: &Path| p.to_string_lossy().into_owned();
    vec!["--format".into(), "zip".into(), "-c".into(), "-f".into(), s(out), "-C".into(), s(parent), format!("{name}/{SETUP_CFG}"), format!("{name}/{name}")]
}

/// tar.exe's arguments extracting an archive (bsdtar refuses absolute paths and ".." unless
/// `-P` is passed; it is not).
pub fn extract_tar_args(zip: &Path, to: &Path) -> Vec<String> {
    let s = |p: &Path| p.to_string_lossy().into_owned();
    vec!["-xf".into(), s(zip), "-C".into(), s(to)]
}

/// `writeMetadataTo:` (`EX:92-100`).
pub fn metadata_json(autolaunch: bool) -> String {
    serde_json::json!({ "AutoLaunch": autolaunch }).to_string()
}

/// The hidden name a replaced script is moved aside to, and the hidden staging folder an
/// install is extracted into (`IM:25-35`).
pub const BACKUP_PREFIX: &str = ".replacing-";
pub const STAGING_PREFIX: &str = ".installing-";

/// `.replacing-<name>-<UUID>` (`IM:450`).
pub fn backup_name(name: &str, uuid: &str) -> String {
    format!("{BACKUP_PREFIX}{name}-{uuid}")
}

/// The script name when `entry` is one of our backups (`IM:490-505`): the prefix, a name,
/// then "-" and a UUID (36 characters).
pub fn stale_backup(entry: &str) -> Option<String> {
    let rest = entry.strip_prefix(BACKUP_PREFIX)?;
    const TAIL: usize = 37;
    if rest.len() <= TAIL || !rest.is_char_boundary(rest.len() - TAIL) {
        return None;
    }
    let (name, tail) = rest.split_at(rest.len() - TAIL);
    let uuid = tail.strip_prefix('-')?;
    uuid::Uuid::try_parse(uuid).ok()?;
    Some(name.to_string())
}

/// Import and export texts (iTerm2's with Tandem's names; `IM`, `AR`, `M:683-789`, `EX`).
pub const IMPORT_IN_PROGRESS: &str = "Another import is in progress. Please try again after it completes.";
pub fn unzip_failed(e: &str) -> String {
    format!("Could not unzip archive: {e}")
}
pub const NO_VALID_SCRIPT: &str = "Archive does not contain a valid Tandem script";
pub const LEGACY_ARCHIVE: &str = "This archive was created by an older version of Tandem. This kind of archive is no longer supported and cannot be installed.";
pub fn already_installed(name: &str) -> String {
    format!("A script named \u{201c}{name}\u{201d} is already installed")
}
pub const ALREADY_HEADING: &str = "Script Already Exists";
pub const REPLACE: &str = "Replace Script";
pub fn replace_failed(name: &str) -> String {
    format!("Could not replace \u{201c}{name}\u{201d}: the existing script could not be moved aside, so it was left unchanged.")
}
pub fn replace_canceled(name: &str) -> String {
    format!("Replacing \u{201c}{name}\u{201d} was canceled. The existing script was kept.")
}
pub const COULD_NOT_INSTALL: &str = "The script could not be installed.";
/// `AR:241-249`: a full environment whose `setup.cfg` cannot be read.
pub const MISSING_SETUP_CFG: &str = "Cannot find setup.cfg";
/// `AR:412-418`: the environment could not be built (under uv every failure reads so).
pub fn runtime_install_failed(e: &str) -> String {
    format!("Failed to install Python Runtime: {e}")
}
pub const COULD_NOT_INSTALL_TITLE: &str = "Could Not Install Script";
pub const TRY_AGAIN: &str = "Try Again";
pub const IMPORTED_TITLE: &str = "Script Imported Successfully";
pub const LAUNCH: &str = "Launch";
pub const EXPORT_FAILED: &str = "Export Failed";
pub const FAILED_TO_CREATE: &str = "Failed to create archive";
pub const ZIP_FAILED: &str = "Failed to create zip file.";
pub const INVALID_LOCATION: &str = "Invalid location (not under Scripts folder).";
pub const NO_SCRIPT_FOUND: &str = "No found script at selected location.";
pub const SETUP_CFG_PARSE: &str = "Could not parse install_requires in setup.cfg";
pub const OFFER_AUTOLAUNCH: &str = "Offer to launch automatically during installation";
/// The chooser window's title (Tandem's; iTerm2's open panel has none).
pub const EXPORT_TITLE: &str = "Export Script";

/// A signed archive cannot be checked, so it is refused (Tandem's; Decision).
pub fn its_refused() -> String {
    "Tandem cannot check the signature of a signed script archive (.its), so it cannot be installed. Ask its author for an unsigned .zip export.".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    #[derive(Default)]
    struct FakeFs {
        files: BTreeSet<PathBuf>,
        dirs: BTreeSet<PathBuf>,
        text: BTreeMap<PathBuf, String>,
    }

    impl FakeFs {
        fn file(&mut self, p: &str) -> &mut Self {
            let p = PathBuf::from(p);
            self.above(&p);
            self.files.insert(p);
            self
        }
        fn dir(&mut self, p: &str) -> &mut Self {
            let p = PathBuf::from(p);
            self.above(&p);
            self.dirs.insert(p);
            self
        }
        fn above(&mut self, p: &Path) {
            let mut up = p.parent();
            while let Some(d) = up.filter(|d| d.starts_with(r"C:\T")) {
                self.dirs.insert(d.to_path_buf());
                up = d.parent();
            }
        }
    }

    impl Fs for FakeFs {
        fn list(&self, dir: &Path) -> Vec<(String, bool)> {
            let name = |p: &PathBuf| p.file_name().unwrap().to_string_lossy().into_owned();
            let mut v: Vec<(String, bool)> = self.files.iter().filter(|p| p.parent() == Some(dir)).map(|p| (name(p), false)).collect();
            v.extend(self.dirs.iter().filter(|p| p.parent() == Some(dir)).map(|p| (name(p), true)));
            // Reversed, so the layout cannot lean on the listing's order.
            v.reverse();
            v
        }
        fn is_file(&self, p: &Path) -> bool {
            self.files.contains(p)
        }
        fn is_dir(&self, p: &Path) -> bool {
            self.dirs.contains(p)
        }
        fn canonical(&self, dir: &Path) -> Option<PathBuf> {
            self.dirs.contains(dir).then(|| dir.to_path_buf())
        }
    }

    fn run(fs: &FakeFs) -> Result<Layout, LayoutError> {
        layout(Path::new(r"C:\T"), fs, &|p: &Path| fs.text.get(p).cloned())
    }

    fn tool() -> FakeFs {
        let mut fs = FakeFs::default();
        fs.dir(r"C:\T").file(r"C:\T\tool\setup.cfg").file(r"C:\T\tool\tool\tool.py");
        fs
    }

    #[test]
    fn repl_follows_iterm2() {
        use sha2::{Digest, Sha256};
        let sum: String = Sha256::digest(BANNER.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(sum, BANNER_SHA256, "the vendored banner is iTerm2's, byte for byte");
        let b = repl_banner();
        assert!(b.starts_with("\x1b[1mWelcome to the Tandem Python REPL!\x1b[m\r\n\r\n"), "{b:?}");
        // Every line keeps its CRLF.
        assert!(b.ends_with("\r\n"));
        assert_eq!(b.matches('\n').count(), b.matches("\r\n").count());
        assert!(b.contains("import iterm2\r\nconnection=await iterm2.Connection.async_create()\r\n"));
        assert!(b.contains("\x1b]8;;https://iterm2.com/python-api\x07Win-click here for Tandem Python API Docs\x1b]8;;\x07"));
        assert!(!b.replace("https://iterm2.com/python-api", "").contains("iTerm2"));
        assert!(!b.contains('\u{2318}'));
        assert_eq!(repl_args(), ["-m", "asyncio"]);
        assert_eq!(REPL_NAME, "REPL");
    }

    #[test]
    fn archive_layout_follows_iterm2() {
        let mut fs = FakeFs::default();
        fs.dir(r"C:\T").file(r"C:\T\a.py");
        assert_eq!(run(&fs), Ok(Layout::Basic { file: PathBuf::from(r"C:\T\a.py") }));

        let mut fs = tool();
        fs.file(r"C:\T\tool\metadata.json");
        fs.text.insert(PathBuf::from(r"C:\T\tool\metadata.json"), r#"{"AutoLaunch": true}"#.into());
        let full = |autolaunch| Ok(Layout::Full { folder: PathBuf::from(r"C:\T\tool"), name: "tool".into(), autolaunch });
        assert_eq!(run(&fs), full(true));
        // A hidden item beside them is not counted.
        fs.file(r"C:\T\.DS_Store");
        assert_eq!(run(&fs), full(true));
        assert_eq!(run(&tool()), full(false), "without metadata");

        let mut legacy = tool();
        legacy.file(r"C:\T\tool\setup.py");
        assert_eq!(run(&legacy), Err(LayoutError::Legacy));
        let mut extra = tool();
        extra.file(r"C:\T\tool\x.txt");
        assert_eq!(run(&extra), Err(LayoutError::NoValidScript));
        let mut two = tool();
        two.file(r"C:\T\b.py");
        assert_eq!(run(&two), Err(LayoutError::NoValidScript));
        let mut only_cfg = FakeFs::default();
        only_cfg.dir(r"C:\T").file(r"C:\T\tool\setup.cfg");
        assert_eq!(run(&only_cfg), Err(LayoutError::NoValidScript));
        let mut no_main = FakeFs::default();
        no_main.dir(r"C:\T").file(r"C:\T\tool\setup.cfg").file(r"C:\T\tool\tool\other.py");
        assert_eq!(run(&no_main), Err(LayoutError::NoValidScript));
        let mut txt = FakeFs::default();
        txt.dir(r"C:\T").file(r"C:\T\a.txt");
        assert_eq!(run(&txt), Err(LayoutError::NoValidScript));
        // Two folders beside setup.cfg: the second is neither required nor optional.
        let mut folders = tool();
        folders.dir(r"C:\T\tool\zz");
        assert_eq!(run(&folders), Err(LayoutError::NoValidScript));
        let mut empty = FakeFs::default();
        empty.dir(r"C:\T");
        assert_eq!(run(&empty), Err(LayoutError::NoValidScript));
    }

    #[test]
    fn export_follows_iterm2() {
        let desktop = Path::new(r"C:\U\Desktop");
        let taken = |p: &Path| p == Path::new(r"C:\U\Desktop\tool.zip");
        assert_eq!(export_name(desktop, "tool", &taken), PathBuf::from(r"C:\U\Desktop\tool (2).zip"));
        let taken2 = |p: &Path| p == Path::new(r"C:\U\Desktop\tool.zip") || p == Path::new(r"C:\U\Desktop\tool (2).zip");
        assert_eq!(export_name(desktop, "tool", &taken2), PathBuf::from(r"C:\U\Desktop\tool (3).zip"));
        assert_eq!(export_name(desktop, "tool", &|_| false), PathBuf::from(r"C:\U\Desktop\tool.zip"));
        let (out, parent) = (Path::new(r"C:\U\Desktop\tool.zip"), Path::new(r"C:\S"));
        assert_eq!(export_tar_args(out, parent, "tool"), ["--format", "zip", "-c", "-f", r"C:\U\Desktop\tool.zip", "-C", r"C:\S", "tool/setup.cfg", "tool/tool"]);
        assert_eq!(extract_tar_args(out, Path::new(r"C:\S\.installing-x")), ["-xf", r"C:\U\Desktop\tool.zip", "-C", r"C:\S\.installing-x"]);
        assert_eq!(metadata_json(false), "{\"AutoLaunch\":false}");
        assert_eq!(metadata_json(true), "{\"AutoLaunch\":true}");
    }

    #[test]
    fn replace_follows_iterm2() {
        let uuid = "0F0E4C5A-1B2C-4D3E-8F90-A1B2C3D4E5F6";
        assert_eq!(uuid.len(), 36);
        assert_eq!(backup_name("tool", uuid), format!(".replacing-tool-{uuid}"));
        assert_eq!(stale_backup(&backup_name("tool", uuid)), Some("tool".to_string()));
        // Names may hold hyphens.
        assert_eq!(stale_backup(&backup_name("my-tool.py", uuid)), Some("my-tool.py".to_string()));
        assert_eq!(stale_backup(&format!(".replacing-tool_{uuid}")), None, "no hyphen before the UUID");
        assert_eq!(stale_backup(".replacing-tool-not-a-uuid-at-all-but-36-characters!"), None);
        assert_eq!(stale_backup(&format!(".replacing-{uuid}")), None, "too short for a name");
        assert_eq!(stale_backup(".installing-x"), None);
        assert_eq!(stale_backup("tool"), None);
        assert_eq!((BACKUP_PREFIX, STAGING_PREFIX), (".replacing-", ".installing-"));
    }

    #[test]
    fn import_texts_follow_iterm2() {
        let q = |s: &str| format!("\u{201c}{s}\u{201d}");
        assert_eq!(IMPORT_IN_PROGRESS, "Another import is in progress. Please try again after it completes.");
        assert_eq!(unzip_failed("boom"), "Could not unzip archive: boom");
        assert_eq!(NO_VALID_SCRIPT, "Archive does not contain a valid Tandem script");
        assert_eq!(LEGACY_ARCHIVE, "This archive was created by an older version of Tandem. This kind of archive is no longer supported and cannot be installed.");
        assert_eq!(already_installed("tool"), format!("A script named {} is already installed", q("tool")));
        assert_eq!((ALREADY_HEADING, REPLACE), ("Script Already Exists", "Replace Script"));
        assert_eq!(replace_failed("tool"), format!("Could not replace {}: the existing script could not be moved aside, so it was left unchanged.", q("tool")));
        assert_eq!(replace_canceled("tool"), format!("Replacing {} was canceled. The existing script was kept.", q("tool")));
        assert_eq!(COULD_NOT_INSTALL, "The script could not be installed.");
        assert_eq!(MISSING_SETUP_CFG, "Cannot find setup.cfg");
        assert_eq!(runtime_install_failed("boom"), "Failed to install Python Runtime: boom");
        assert_eq!((COULD_NOT_INSTALL_TITLE, TRY_AGAIN, IMPORTED_TITLE, LAUNCH), ("Could Not Install Script", "Try Again", "Script Imported Successfully", "Launch"));
        assert_eq!((EXPORT_FAILED, FAILED_TO_CREATE, ZIP_FAILED), ("Export Failed", "Failed to create archive", "Failed to create zip file."));
        assert_eq!((INVALID_LOCATION, NO_SCRIPT_FOUND), ("Invalid location (not under Scripts folder).", "No found script at selected location."));
        assert_eq!(SETUP_CFG_PARSE, "Could not parse install_requires in setup.cfg");
        assert_eq!(OFFER_AUTOLAUNCH, "Offer to launch automatically during installation");
        assert_eq!(EXPORT_TITLE, "Export Script");
        let r = its_refused();
        assert!(r.contains(".its") && r.contains(".zip"), "{r}");
        let all = [IMPORT_IN_PROGRESS.to_string(), unzip_failed("e"), NO_VALID_SCRIPT.into(), LEGACY_ARCHIVE.into(), already_installed("n"), ALREADY_HEADING.into(), REPLACE.into(), replace_failed("n"), replace_canceled("n"), COULD_NOT_INSTALL.into(), MISSING_SETUP_CFG.into(), runtime_install_failed("e"), COULD_NOT_INSTALL_TITLE.into(), TRY_AGAIN.into(), IMPORTED_TITLE.into(), LAUNCH.into(), EXPORT_FAILED.into(), FAILED_TO_CREATE.into(), ZIP_FAILED.into(), INVALID_LOCATION.into(), NO_SCRIPT_FOUND.into(), SETUP_CFG_PARSE.into(), OFFER_AUTOLAUNCH.into(), EXPORT_TITLE.into(), r];
        for t in all {
            assert!(!t.contains("iTerm2"), "{t}");
        }
    }
}

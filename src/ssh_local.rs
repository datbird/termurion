//! The local side of an SSH file transfer (milestone 14d, `CFT:151-195`, `TF:107-224`): temp
//! names, the final name, the `ssh://` source URL, finishing a download (the Mark of the Web,
//! then the move), unzipping a folder, and the texts. Everything a remote sent is hostile here:
//! names go through `inline_file::clean_name` and the one `inert_files` rule (P1), the mark
//! through `inert_files::mark_strict` (P2), a zip through `zip_check` and a walk that never
//! follows a reparse point (P4, R14de-1), and each download writes only its own temp file
//! (R14de-4).

use crate::inert_files::{refused_on_listing, refused_path};
use crate::layout::PaneId;
use std::path::{Path, PathBuf};

/// A download's own hidden file or folder; the person never sees it.
pub const TEMP_PREFIX: &str = ".Tandem.";

pub fn temp_name(uuid: &str) -> String {
    format!("{TEMP_PREFIX}{uuid}")
}

/// Texts: iTerm2's (`CFT`, `TF`), Explorer for Finder.
pub const NO_DOWNLOADS: &str = "Unable to find Downloads folder";
pub const FAILED_TO_QUARANTINE: &str = "Failed to quarantine";
/// Failed to quarantine and the temp file could not be removed either: the app warns with `danger`.
pub const NOT_REMOVED: &str = "Failed to quarantine and could not remove the file";
pub const NOT_ALLOWED: &str = "files of this type are not allowed";
pub const KEEP_OR_REPLACE_ID: &str = "NoSyncOverwriteOrReplaceFile";
pub const KEEP_BOTH: &str = "Keep Both";
pub const REPLACE: &str = "Replace";
pub const OVERWRITE_HEADING: &str = "Overwrite existing file?";
pub const DANGER_HEADING: &str = "Danger!";
pub const TRANSFER_CANCELLED: &str = "Transfer cancelled";
/// The first stat refuses a file the drive cannot hold (P13).
pub const NOT_ENOUGH_SPACE: &str = "Not enough space on the drive";

pub fn keep_or_replace(name: &str) -> String {
    format!("A file named {name} already exists. Keep both files or replace the existing file?")
}

pub fn danger(path: &str) -> String {
    format!("The file at \u{201c}{path}\u{201d} could not be quarantined or deleted! It is dangerous and should be removed.")
}

pub fn download_finished(name: &str) -> String {
    format!("Download finished for \u{201c}{name}\u{201d}.")
}

pub fn download_failed(name: &str) -> String {
    format!("Download failed for \u{201c}{name}\u{201d}.")
}

pub fn upload_finished(name: &str) -> String {
    format!("Upload finished for \u{201c}{name}\u{201d}.")
}

pub fn upload_failed(name: &str) -> String {
    format!("Upload failed for \u{201c}{name}\u{201d}.")
}

/// Why a remote name may not be written at all (P1): its cleaned form is a type the shell reads
/// on listing (`x.url`, `desktop.ini`, ...). Checked before the first fetch; never renamed.
pub fn refused_name(remote_name: &str) -> Option<&'static str> {
    refused_on_listing(&crate::inline_file::clean_name(remote_name)).then_some(NOT_ALLOWED)
}

/// The free name for `remote_name` in `dir` and, when the plain name was taken, the plain name
/// (the question's Replace answer). The name is cleaned (`clean_name`), a folder's zip gets
/// `.zip`, and numbering splits at the FIRST dot of the name (`TF:107-149`: `a (1).tar.gz`,
/// ` (1).bashrc`).
pub fn final_destination(dir: &Path, remote_name: &str, zip_of_folder: bool, exists: &dyn Fn(&Path) -> bool) -> (PathBuf, Option<PathBuf>) {
    let mut name = crate::inline_file::clean_name(remote_name);
    if zip_of_folder {
        name.push_str(".zip");
    }
    let plain = dir.join(&name);
    if !exists(&plain) {
        return (plain, None);
    }
    let (prefix, suffix) = match name.find('.') {
        Some(i) => name.split_at(i),
        None => (name.as_str(), ""),
    };
    let numbered = (1..=10_000).map(|n| dir.join(format!("{prefix} ({n}){suffix}"))).find(|c| !exists(c));
    // Ten thousand copies: a name no one has taken.
    let free = numbered.unwrap_or_else(|| dir.join(format!("{prefix} ({}){suffix}", uuid::Uuid::new_v4())));
    (free, Some(plain))
}

/// `urlPathAllowed`, `urlUserAllowed` and `urlHostAllowed` of `NSCharacterSet`: unreserved and
/// sub-delimiters, plus `:`, `@` and `/` in a path, `:`, `[` and `]` in a host, nothing more in a user.
#[derive(Clone, Copy)]
enum Part {
    Path,
    User,
    Host,
}

fn percent(s: &str, part: Part) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        let plain = b.is_ascii_alphanumeric()
            || b"-._~!$&'()*+,;=".contains(&b)
            || match part {
                Part::Path => b":@/".contains(&b),
                Part::User => false,
                Part::Host => b":[]".contains(&b),
            };
        if plain {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The `ssh://user@host/path` a download came from (`CFT:128-135`), percent-encoded like
/// `URLComponents`, controls included: it holds no CR, LF or other control character, so it can
/// never add a line to the Zone.Identifier text (R14de-2). A path without a leading `/` gets one.
pub fn ssh_url(user: Option<&str>, host: &str, path: &str) -> String {
    let user = user.map(|u| format!("{}@", percent(u, Part::User))).unwrap_or_default();
    let slash = if path.starts_with('/') { "" } else { "/" };
    format!("ssh://{user}{}{slash}{}", percent(host, Part::Host), percent(path, Part::Path))
}

/// Where a download's bytes go: its own hidden temp file, never the final name (R14de-4).
#[derive(Debug)]
pub enum Place {
    /// Win-click: `dir\.Tandem.<UUID>`, finished into `dir`.
    Downloads { temp: PathBuf, dir: PathBuf },
    /// Download Files: `%TEMP%\.Tandem.<UUID>` (the zip while the job is a folder), finished
    /// into `dest` by batch `batch` (Task 7).
    Temp { temp: PathBuf, dest: PathBuf, batch: u64 },
}

impl Place {
    pub fn temp(&self) -> &Path {
        match self {
            Place::Downloads { temp, .. } | Place::Temp { temp, .. } => temp,
        }
    }
}

#[derive(Debug)]
pub struct Local {
    pub place: Place,
    pub pane: PaneId,
    pub remote: String,
    pub short_name: String,
    pub host: String,
    pub user: Option<String>,
    pub written: u64,
    pub total: Option<u64>,
    pub upload: bool,
    /// The percent last shown in the banner (a banner changes at most every 10 percent).
    pub shown: u64,
    /// The finished download is a folder's zip (its final name gets `.zip`).
    pub zip: bool,
}

impl Local {
    pub fn new(place: Place, pane: PaneId, remote: &str, host: &str, user: Option<&str>) -> Local {
        let short_name = remote.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string();
        Local { place, pane, remote: remote.to_string(), short_name, host: host.to_string(), user: user.map(str::to_string), written: 0, total: None, upload: false, shown: 0, zip: false }
    }

    pub fn url(&self) -> String {
        ssh_url(self.user.as_deref(), &self.host, &self.remote)
    }

    /// The percent done, when the size is known.
    pub fn percent(&self) -> Option<u64> {
        self.total.filter(|t| *t > 0).map(|t| (self.written.min(t) as u128 * 100 / t as u128) as u64)
    }
}

/// Whether the files under `dir` (symlinks not followed) add up to more than `cap` bytes; the
/// walk stops as soon as they do, so a huge folder is never read through (P19).
pub fn tree_over(dir: &Path, cap: u64) -> std::io::Result<bool> {
    let (mut total, mut stack) = (0u64, vec![dir.to_path_buf()]);
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)? {
            let e = e?;
            let m = std::fs::symlink_metadata(e.path())?;
            if m.is_dir() {
                stack.push(e.path());
            } else if m.is_file() {
                total = total.saturating_add(m.len());
                if total > cap {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

/// The banner text (Tandem's): "Downloading a.txt from h: 25%", or the byte count with no total.
pub fn progress_text(l: &Local) -> String {
    let (verb, prep) = if l.upload { ("Uploading", "to") } else { ("Downloading", "from") };
    match l.percent() {
        Some(p) => format!("{verb} {} {prep} {}: {p}%", l.short_name, l.host),
        None => format!("{verb} {} {prep} {}: {} bytes", l.short_name, l.host, l.written),
    }
}

/// Moves a finished temp file into place. The final path is refused when it is a type the shell
/// reads on listing (P1); the temp is marked with `mark` (a failure removes it: "Failed to
/// quarantine", or `NOT_REMOVED` when it will not go); then it replaces `final_path`, and a
/// failed move removes it and reports the error (iTerm2 reports success).
pub fn finish_with(temp: &Path, final_path: &Path, host_url: &str, mark: &dyn Fn(&Path, &str) -> std::io::Result<()>) -> Result<PathBuf, String> {
    if refused_path(&final_path.to_string_lossy()) {
        let _ = std::fs::remove_file(temp);
        return Err(NOT_ALLOWED.to_string());
    }
    if mark(temp, host_url).is_err() {
        let gone = std::fs::remove_file(temp).is_ok() || !temp.exists();
        return Err(if gone { FAILED_TO_QUARANTINE } else { NOT_REMOVED }.to_string());
    }
    match std::fs::rename(temp, final_path) {
        Ok(()) => Ok(final_path.to_path_buf()),
        Err(e) => {
            let _ = std::fs::remove_file(temp);
            Err(e.to_string())
        }
    }
}

#[cfg(windows)]
pub fn finish(temp: &Path, final_path: &Path, host_url: &str) -> Result<PathBuf, String> {
    finish_with(temp, final_path, host_url, &|p, u| crate::inert_files::mark_strict(p, Some(u)))
}

/// What a zip may weigh as a file we read whole before checking it.
const ZIP_FILE_MAX: u64 = 1 << 30;

fn is_reparse(m: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        m.file_attributes() & 0x400 != 0 || m.file_type().is_symlink()
    }
    #[cfg(not(windows))]
    {
        m.file_type().is_symlink()
    }
}

/// Every file and folder under `root` (relative), walked with `symlink_metadata` only.
fn tree(root: &Path) -> Result<Vec<(PathBuf, bool)>, String> {
    let mut out = Vec::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        for e in std::fs::read_dir(root.join(&rel)).map_err(|e| e.to_string())? {
            let e = e.map_err(|e| e.to_string())?;
            let path = rel.join(e.file_name());
            let m = std::fs::symlink_metadata(root.join(&path)).map_err(|e| e.to_string())?;
            if is_reparse(&m) {
                return Err(crate::zip_check::refused(&format!("\u{201c}{}\u{201d} is a link", path.display())));
            }
            if m.is_dir() {
                stack.push(path.clone());
            }
            out.push((path, m.is_dir()));
        }
    }
    out.sort();
    Ok(out)
}

/// Marks every file under `from` and moves it to the same place under `dest`, replacing what is
/// there. Before anything moves: no entry is a reparse point or a file with another hard link,
/// no name is one Explorer reads or Windows reads as another item, and every file took its mark
/// (a failure is "Failed to quarantine" and nothing moved). `from` should be on the volume of `dest`.
#[cfg(windows)]
pub fn mark_and_move(from: &Path, dest: &Path, host_url: &str) -> Result<(), String> {
    mark_and_move_with(from, dest, host_url, &|p, u| crate::inert_files::mark_strict(p, Some(u)))
}

#[cfg(windows)]
pub fn mark_and_move_with(from: &Path, dest: &Path, host_url: &str, mark: &dyn Fn(&Path, &str) -> std::io::Result<()>) -> Result<(), String> {
    use crate::script_archive as sa;
    if let Some(entry) = sa::unsafe_entry(from, &sa::stage_entries) {
        return Err(sa::unsafe_entry_text(entry.strip_prefix(from).unwrap_or(&entry)));
    }
    let entries = tree(from)?;
    for (rel, _) in &entries {
        let bad = rel.components().any(|c| {
            let part = c.as_os_str().to_string_lossy();
            crate::zip_check::bad_part(&part).is_some() || refused_on_listing(&part)
        });
        if bad {
            return Err(crate::zip_check::refused(&format!("\u{201c}{}\u{201d} is not a name this PC can keep", rel.display())));
        }
    }
    for (rel, is_dir) in &entries {
        if !is_dir && mark(&from.join(rel), host_url).is_err() {
            return Err(FAILED_TO_QUARANTINE.to_string());
        }
    }
    let ok_place = |p: &Path| std::fs::symlink_metadata(p).map(|m| !is_reparse(&m)).unwrap_or(true);
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    for (rel, is_dir) in &entries {
        let to = dest.join(rel);
        if !ok_place(&to) {
            return Err(format!("{} is a link", to.display()));
        }
        if *is_dir {
            match std::fs::create_dir(&to) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && to.is_dir() => {}
                Err(e) => return Err(e.to_string()),
            }
        }
    }
    for (rel, is_dir) in &entries {
        if !is_dir {
            std::fs::rename(from.join(rel), dest.join(rel)).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Unzips `zip` into `dest` (a folder download, `FT:150-172`): the file is read and checked by
/// `zip_check` first (the whole archive is refused on any unsafe entry, and `extract` is never
/// called), `extract` unpacks it into a fresh empty folder beside `dest`, and `mark_and_move`
/// moves the marked files into place. The fresh folder is removed on every path. The caller
/// removes the zip.
#[cfg(windows)]
pub fn unzip_with(zip: &Path, dest: &Path, host_url: &str, extract: &mut dyn FnMut(&Path, &Path) -> Result<(), String>) -> Result<(), String> {
    let size = std::fs::metadata(zip).map_err(|e| e.to_string())?.len();
    if size > ZIP_FILE_MAX {
        return Err(crate::zip_check::refused("the archive is too large"));
    }
    let bytes = std::fs::read(zip).map_err(|e| e.to_string())?;
    crate::zip_check::check(&bytes)?;
    drop(bytes);
    let parent = dest.parent().ok_or_else(|| "no folder to unzip into".to_string())?;
    let fresh = parent.join(temp_name(&uuid::Uuid::new_v4().to_string().to_uppercase()));
    std::fs::create_dir(&fresh).map_err(|e| e.to_string())?;
    let result = extract(zip, &fresh).and_then(|()| mark_and_move(&fresh, dest, host_url));
    let _ = std::fs::remove_dir_all(&fresh);
    result
}

/// `unzip_with` through System32 `tar.exe` (`script_provision::RealRunner`, never `-P`).
#[cfg(windows)]
pub fn unzip(zip: &Path, dest: &Path, host_url: &str) -> Result<(), String> {
    use crate::script_provision::{RealRunner, Runner};
    let (tx, rx) = std::sync::mpsc::channel();
    let mut runner = RealRunner::new(tx);
    let r = unzip_with(zip, dest, host_url, &mut |z, to| runner.extract(z, to));
    // The runner's log lines are not shown.
    while rx.try_recv().is_ok() {}
    r
}

/// The bytes free for the person on the drive holding `dir` (`GetDiskFreeSpaceExW`).
#[cfg(windows)]
pub fn free_space(dir: &Path) -> Result<u64, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free = 0u64;
    if unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) } == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(free)
}

/// A stale temp: a `.Tandem.` file or folder, or a `Tandem-<UUID>` batch folder, untouched for
/// more than a day (a crash mid-download leaves one; P24).
pub fn is_stale_temp(name: &str, is_dir: bool, age_secs: u64) -> bool {
    let ours = name.starts_with(TEMP_PREFIX) || (is_dir && name.strip_prefix("Tandem-").is_some_and(|u| uuid::Uuid::try_parse(u).is_ok()));
    ours && age_secs > 24 * 3600
}

/// Removes the stale temps directly in `dir` (never follows a link).
pub fn sweep_stale(dir: &Path, now: std::time::SystemTime) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(m) = std::fs::symlink_metadata(e.path()) else { continue };
        let age = m.modified().ok().and_then(|t| now.duration_since(t).ok()).map(|d| d.as_secs()).unwrap_or(0);
        if !is_stale_temp(&e.file_name().to_string_lossy(), m.is_dir(), age) {
            continue;
        }
        if m.is_dir() && !is_reparse(&m) {
            let _ = std::fs::remove_dir_all(e.path());
        } else {
            let _ = std::fs::remove_file(e.path()).or_else(|_| std::fs::remove_dir(e.path()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zip_check::tests::{dir, file, zip, Entry};

    fn taken(paths: &'static [&'static str]) -> impl Fn(&Path) -> bool {
        move |p| paths.iter().any(|t| Path::new(t) == p)
    }
    const D: &str = r"D:\dl";

    #[test]
    fn final_names_follow_iterm2() {
        let d = Path::new(D);
        let join = |n: &str| d.join(n);
        let t = taken(&[r"D:\dl\a.tar.gz"]);
        assert_eq!(final_destination(d, "a.tar.gz", false, &t), (join("a (1).tar.gz"), Some(join("a.tar.gz"))));
        assert_eq!(final_destination(d, "b.txt", false, &|_| false), (join("b.txt"), None));
        let t = taken(&[r"D:\dl\proj.zip", r"D:\dl\proj (1).zip"]);
        assert_eq!(final_destination(d, "proj", true, &t), (join("proj (2).zip"), Some(join("proj.zip"))));
        let t = taken(&[r"D:\dl\.bashrc"]);
        assert_eq!(final_destination(d, ".bashrc", false, &t), (join(" (1).bashrc"), Some(join(".bashrc"))));
        for (name, want) in [("a:b?.txt", "a_b_.txt"), ("CON", "_CON"), ("CONIN$", "_CONIN$"), ("CON .txt", "_CON .txt"), ("..", "Unnamed file"), ("", "Unnamed file")] {
            let (p, q) = final_destination(d, name, false, &|_| false);
            assert_eq!((p.clone(), q), (join(want), None), "{name:?}");
            assert_eq!(p.parent(), Some(d), "{name:?}");
        }
        // The cap on numbering: a host that says everything is taken still gets a name.
        let (p, q) = final_destination(d, "z.txt", false, &|c| c == join("z.txt"));
        assert_eq!(q, Some(join("z.txt")));
        assert!(p.to_string_lossy().starts_with(r"D:\dl\z (") && p != join("z.txt"));
    }

    #[test]
    fn refused_names_are_never_renamed() {
        // P1: types the shell reads on listing are refused whole, whatever the case or the tail.
        for bad in ["x.url", "X.URL", "a.lnk", "desktop.ini", "Desktop.INI.", "x.library-ms", "x.theme", "x.url.", "x.url ", "/h/d/x.url", "dir\\x.lnk"] {
            assert_eq!(refused_name(bad), Some(NOT_ALLOWED), "{bad:?}");
        }
        for fine in ["a.txt", "url.txt", "a.ini", "CON", "x.url.txt", "notes"] {
            assert_eq!(refused_name(fine), None, "{fine:?}");
        }
    }

    #[test]
    fn ssh_urls_follow_iterm2() {
        assert_eq!(ssh_url(Some("bob"), "h.example", "/home/bob/a b.txt"), "ssh://bob@h.example/home/bob/a%20b.txt");
        assert_eq!(ssh_url(None, "h", "/x"), "ssh://h/x");
        for (path, want) in [
            ("/r/\u{e9}.txt", "/r/%C3%A9.txt"),
            ("/a#b", "/a%23b"),
            ("/a?b", "/a%3Fb"),
            ("/a%b", "/a%25b"),
            ("/x\r\nZoneId=0", "/x%0D%0AZoneId=0"),
            ("/a\u{0}b", "/a%00b"),
            ("/a\tb", "/a%09b"),
            ("/a\u{7f}b", "/a%7Fb"),
            ("/a\"b", "/a%22b"),
            ("/a<b>", "/a%3Cb%3E"),
            ("/a\\b", "/a%5Cb"),
            ("/a[b]", "/a%5Bb%5D"),
            ("/a^\u{60}{|}b", "/a%5E%60%7B%7C%7Db"),
            ("/a-._~!$&'()*+,;=:@b", "/a-._~!$&'()*+,;=:@b"),
            ("~/x", "/~/x"),
            ("rel", "/rel"),
        ] {
            assert_eq!(ssh_url(None, "h", path), format!("ssh://h{want}"), "{path:?}");
        }
        for (user, want) in [("a b", "a%20b@"), ("a:b", "a%3Ab@"), ("a@b", "a%40b@"), ("a/b", "a%2Fb@"), ("a\r\nb", "a%0D%0Ab@")] {
            assert_eq!(ssh_url(Some(user), "h", "/x"), format!("ssh://{want}h/x"), "{user:?}");
        }
        assert_eq!(ssh_url(None, "h\r\nx", "/x"), "ssh://h%0D%0Ax/x");
        assert_eq!(ssh_url(None, "[::1]", "/x"), "ssh://[::1]/x");
        // The mark text built from it has exactly its three lines.
        let t = crate::inert_files::zone_text(Some(&ssh_url(None, "h", "/x\r\nZoneId=0")));
        assert_eq!(t.lines().count(), 3);
        assert_eq!(t.lines().filter(|l| l.starts_with("ZoneId=")).count(), 1);
        assert!(t.ends_with("\r\n") && !t.replace("\r\n", "").contains(['\r', '\n']));
    }

    #[test]
    fn transfer_texts_follow_iterm2() {
        assert_eq!(download_finished("a.txt"), "Download finished for \u{201c}a.txt\u{201d}.");
        assert_eq!(download_failed("a.txt"), "Download failed for \u{201c}a.txt\u{201d}.");
        assert_eq!(upload_finished("a.txt"), "Upload finished for \u{201c}a.txt\u{201d}.");
        assert_eq!(upload_failed("a.txt"), "Upload failed for \u{201c}a.txt\u{201d}.");
        assert_eq!(keep_or_replace("a.txt"), "A file named a.txt already exists. Keep both files or replace the existing file?");
        assert_eq!((OVERWRITE_HEADING, KEEP_BOTH, REPLACE, KEEP_OR_REPLACE_ID), ("Overwrite existing file?", "Keep Both", "Replace", "NoSyncOverwriteOrReplaceFile"));
        assert_eq!((FAILED_TO_QUARANTINE, NO_DOWNLOADS, DANGER_HEADING, TRANSFER_CANCELLED), ("Failed to quarantine", "Unable to find Downloads folder", "Danger!", "Transfer cancelled"));
        assert_eq!(danger("C:\\x"), "The file at \u{201c}C:\\x\u{201d} could not be quarantined or deleted! It is dangerous and should be removed.");
        let mut l = Local::new(Place::Downloads { temp: PathBuf::from("t"), dir: PathBuf::from("d") }, 1, "/r/a.txt", "h", None);
        l.total = Some(2048);
        l.written = 512;
        assert_eq!(progress_text(&l), "Downloading a.txt from h: 25%");
        l.upload = true;
        assert_eq!(progress_text(&l), "Uploading a.txt to h: 25%");
        l.upload = false;
        l.total = None;
        assert_eq!(progress_text(&l), "Downloading a.txt from h: 512 bytes");
        l.total = Some(0);
        assert_eq!(progress_text(&l), "Downloading a.txt from h: 512 bytes", "no division by a zero size");
        assert_eq!(Local::new(Place::Downloads { temp: PathBuf::new(), dir: PathBuf::new() }, 1, "/r/d/", "h", None).short_name, "d");
    }

    #[test]
    fn stale_temps_are_swept() {
        let day = 24 * 3600;
        assert!(is_stale_temp(".Tandem.ABC", false, day + 1) && is_stale_temp(".Tandem.ABC", true, day + 1));
        assert!(!is_stale_temp(".Tandem.ABC", false, day), "a day old exactly is kept");
        assert!(!is_stale_temp("a.txt", false, 10 * day) && !is_stale_temp(".Tandemx", false, 0));
        let u = "0f8fad5b-d9cb-469f-a165-70867728950e";
        assert!(is_stale_temp(&format!("Tandem-{u}"), true, 2 * day));
        assert!(!is_stale_temp(&format!("Tandem-{u}"), false, 2 * day), "a batch is a folder");
        assert!(!is_stale_temp("Tandem-notes", true, 2 * day));
    }

    // The hostile archives of the pure check also stop `unzip_with` before anything runs.
    #[cfg(windows)]
    fn hostile() -> Vec<(&'static str, Vec<u8>)> {
        vec![
            ("dotdot", zip(&[dir("d/"), file("d/../x", "x")])),
            ("url", zip(&[dir("d/"), file("d/x.url", "x")])),
            ("stream", zip(&[dir("d/"), file("d/x.txt:Zone.Identifier", "x")])),
            ("symlink", zip(&[dir("d/"), Entry { external: 0o120_777 << 16, ..file("d/l", "/etc/passwd") }])),
            ("notype", zip(&[dir("d/"), Entry { external: 0o644 << 16, ..file("d/a", "x") }])),
            ("latin1", zip(&[dir("d/"), Entry { name: b"d/\xe9".to_vec(), flags: 0, ..file("d/x", "x") }])),
            ("notzip", b"not a zip at all".to_vec()),
        ]
    }

    #[cfg(windows)]
    mod windows {
        use super::*;
        use crate::arrangement::has_mark_of_the_web;

        fn fresh(tag: &str) -> PathBuf {
            static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let p = std::env::temp_dir().join(format!("tandem-sshlocal-{tag}-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            p
        }
        fn listing(p: &Path) -> Vec<String> {
            let mut v: Vec<String> = std::fs::read_dir(p).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
            v.sort();
            v
        }

        #[test]
        fn finish_follows_iterm2() {
            let d = fresh("finish");
            let temp = d.join(".Tandem.U");
            std::fs::write(&temp, "abc").unwrap();
            let a = d.join("a.txt");
            assert_eq!(finish(&temp, &a, "ssh://h/a.txt"), Ok(a.clone()));
            assert!(!temp.exists() && has_mark_of_the_web(&a));
            assert_eq!(std::fs::read_to_string(&a).unwrap(), "abc");
            assert_eq!(std::fs::read_to_string(format!("{}:Zone.Identifier", a.display())).unwrap(), crate::inert_files::zone_text(Some("ssh://h/a.txt")));
            // Over an existing file: replaced.
            std::fs::write(&a, "old").unwrap();
            std::fs::write(&temp, "new").unwrap();
            assert_eq!(finish(&temp, &a, "ssh://h/a.txt"), Ok(a.clone()));
            assert_eq!(std::fs::read_to_string(&a).unwrap(), "new");
            // A final path in a folder that is not there: the OS text, the temp removed.
            std::fs::write(&temp, "x").unwrap();
            let e = finish(&temp, &d.join("no").join("b.txt"), "ssh://h/b").unwrap_err();
            assert!(!e.is_empty() && !temp.exists());
            let _ = std::fs::remove_dir_all(&d);
        }

        #[test]
        fn a_refused_final_name_is_not_written() {
            let d = fresh("refuse");
            for name in ["x.url", "x.lnk", "desktop.ini", "x.theme"] {
                let temp = d.join(".Tandem.R");
                std::fs::write(&temp, "[InternetShortcut]").unwrap();
                assert_eq!(finish(&temp, &d.join(name), "ssh://h/x"), Err(NOT_ALLOWED.to_string()), "{name}");
                assert!(!temp.exists() && !d.join(name).exists(), "{name}");
            }
            assert_eq!(listing(&d), Vec::<String>::new());
            let _ = std::fs::remove_dir_all(&d);
        }

        #[test]
        fn temp_place_writes_its_own_file() {
            let t = fresh("temp");
            let x = t.join("x.txt");
            std::fs::write(&x, vec![b'o'; 100]).unwrap();
            let url = "ssh://h/x.txt";
            // A download writes only its own temp, and finishing leaves exactly its bytes.
            let temp = t.join(temp_name("U1"));
            std::fs::write(&temp, "abc").unwrap();
            assert_eq!(finish(&temp, &x, url), Ok(x.clone()));
            assert_eq!(std::fs::read(&x).unwrap(), b"abc");
            // Two downloads of one name with interleaved writes: last finished wins, nothing left over.
            let (t1, t2) = (t.join(temp_name("U2")), t.join(temp_name("U3")));
            let append = |p: &Path, s: &str| {
                use std::io::Write;
                std::fs::OpenOptions::new().create(true).append(true).open(p).unwrap().write_all(s.as_bytes()).unwrap();
            };
            append(&t1, "on");
            append(&t2, "sec");
            append(&t1, "e");
            append(&t2, "ond");
            assert_eq!(std::fs::read(&x).unwrap(), b"abc", "no write reached the final name");
            finish(&t2, &x, url).unwrap();
            finish(&t1, &x, url).unwrap();
            assert_eq!(std::fs::read(&x).unwrap(), b"one");
            assert_eq!(listing(&t), ["x.txt"]);
            // A mark that fails: "Failed to quarantine", the temp removed, the old file untouched.
            let temp = t.join(temp_name("U4"));
            std::fs::write(&temp, "bad").unwrap();
            let refuse = |_: &Path, _: &str| -> std::io::Result<()> { Err(std::io::Error::other("no streams")) };
            assert_eq!(finish_with(&temp, &x, url, &refuse), Err(FAILED_TO_QUARANTINE.to_string()));
            assert!(!temp.exists());
            assert_eq!(std::fs::read(&x).unwrap(), b"one");
            let _ = std::fs::remove_dir_all(&t);
        }

        fn tar_zip(parent: &Path, folder: &str, out: &str) {
            let tar = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap().join("System32").join("tar.exe");
            let r = std::process::Command::new(tar).current_dir(parent).args(["-a", "-cf", out, folder]).status().unwrap();
            assert!(r.success());
        }

        #[test]
        fn unzip_marks_every_file() {
            let t = fresh("unzip");
            let src = t.join("src");
            std::fs::create_dir_all(src.join("d").join("s")).unwrap();
            std::fs::write(src.join("d").join("a.txt"), "a").unwrap();
            std::fs::write(src.join("d").join("s").join("b.txt"), "b").unwrap();
            tar_zip(&src, "d", "x.zip");
            let zip_path = t.join("x.zip");
            std::fs::rename(src.join("x.zip"), &zip_path).unwrap();
            let out = t.join("out");
            std::fs::create_dir(&out).unwrap();
            // An existing file is replaced (unzip -o).
            std::fs::create_dir_all(out.join("d")).unwrap();
            std::fs::write(out.join("d").join("a.txt"), "old").unwrap();
            let before = listing(&t);
            unzip(&zip_path, &out, "ssh://h/d").unwrap();
            for f in [out.join("d").join("a.txt"), out.join("d").join("s").join("b.txt")] {
                assert!(f.exists() && has_mark_of_the_web(&f), "{}", f.display());
            }
            assert_eq!(std::fs::read_to_string(out.join("d").join("a.txt")).unwrap(), "a");
            assert_eq!(listing(&t), before, "no .Tandem. folder is left beside the destination");
            // Not a zip: refused before tar runs.
            let bad = t.join("bad.zip");
            std::fs::write(&bad, "hello").unwrap();
            assert!(unzip(&bad, &t.join("o2"), "ssh://h/d").unwrap_err().contains("not a zip"));
            let _ = std::fs::remove_dir_all(&t);
        }

        #[test]
        fn unzip_refuses_hostile_archives() {
            for (name, bytes) in hostile() {
                let t = fresh("hostile");
                let z = t.join("x.zip");
                std::fs::write(&z, &bytes).unwrap();
                let before = listing(&t);
                let mut calls = 0;
                let r = unzip_with(&z, &t.join("out"), "ssh://h/d", &mut |_, _| {
                    calls += 1;
                    Ok(())
                });
                assert!(r.is_err(), "{name}");
                assert_eq!(calls, 0, "{name}: tar.exe must not start");
                assert_eq!(listing(&t), before, "{name}: nothing new on disk");
                let _ = std::fs::remove_dir_all(&t);
            }
        }

        #[test]
        fn mark_and_move_refuses_reparse_points() {
            let t = fresh("reparse");
            let f = t.join("F");
            let o = t.join("O");
            std::fs::create_dir_all(f.join("d")).unwrap();
            std::fs::create_dir_all(&o).unwrap();
            std::fs::write(f.join("d").join("a.txt"), "a").unwrap();
            std::fs::write(o.join("victim.txt"), "v").unwrap();
            let junction = f.join("d").join("j");
            let made = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&junction).arg(&o).output().unwrap();
            assert!(made.status.success(), "{made:?}");
            let out = t.join("out");
            let e = mark_and_move(&f, &out, "ssh://h/d").unwrap_err();
            assert!(e.contains(r"d\j"), "{e}");
            let victim = o.join("victim.txt");
            assert!(!has_mark_of_the_web(&victim));
            assert_eq!(std::fs::read_to_string(&victim).unwrap(), "v");
            assert!(!out.exists(), "nothing moved");
            // Without the junction it moves, marked, and the source is left empty of files.
            std::fs::remove_dir(&junction).unwrap();
            mark_and_move(&f, &out, "ssh://h/d").unwrap();
            let a = out.join("d").join("a.txt");
            assert!(a.exists() && has_mark_of_the_web(&a));
            assert!(!f.join("d").join("a.txt").exists());
            // A mark that fails moves nothing.
            std::fs::write(f.join("d").join("b.txt"), "b").unwrap();
            let out2 = t.join("out2");
            let refuse = |_: &Path, _: &str| -> std::io::Result<()> { Err(std::io::Error::other("no streams")) };
            assert_eq!(mark_and_move_with(&f, &out2, "u", &refuse), Err(FAILED_TO_QUARANTINE.to_string()));
            assert!(!out2.exists());
            let _ = std::fs::remove_dir_all(&t);
        }

        #[test]
        fn free_space_is_read() {
            assert!(free_space(&std::env::temp_dir()).unwrap() > 0);
            assert!(free_space(Path::new(r"Z:\\no\\such\\folder\\anywhere")).is_err());
        }

        #[test]
        fn old_temps_are_removed_from_a_folder() {
            let t = fresh("sweep");
            let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 24 * 3600);
            let stale = t.join(".Tandem.OLD");
            std::fs::write(&stale, "x").unwrap();
            std::fs::File::options().write(true).open(&stale).unwrap().set_modified(old).unwrap();
            let young = t.join(".Tandem.NEW");
            std::fs::write(&young, "x").unwrap();
            let other = t.join("keep.txt");
            std::fs::write(&other, "x").unwrap();
            std::fs::File::options().write(true).open(&other).unwrap().set_modified(old).unwrap();
            sweep_stale(&t, std::time::SystemTime::now());
            assert_eq!(listing(&t), [".Tandem.NEW", "keep.txt"]);
            let _ = std::fs::remove_dir_all(&t);
        }
    }
}

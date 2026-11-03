//! Unix has no Recycle Bin call and no per-extension edit verb yet; the Desktop is the home folder's.

use std::path::{Path, PathBuf};

use super::super::Unsupported;
use crate::script_archive::Stat;

/// The home folder's Desktop, when there is one.
pub fn desktop() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("HOME")?).join("Desktop");
    dir.is_dir().then_some(dir)
}

pub fn recycle(_path: &Path) -> Result<(), Unsupported> {
    Err(Unsupported { feature: "Moving to the Trash" })
}

/// Nothing says which app edits `.py`; the caller reveals the file instead.
pub fn edit_app_name() -> Option<String> {
    None
}

/// A symbolic link.
pub fn is_reparse(p: &Path) -> bool {
    p.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink())
}

/// A name is its own on Unix: the entry's last part, when the entry exists (a link not followed).
pub fn final_name(p: &Path) -> Option<String> {
    p.symlink_metadata().ok()?;
    Some(p.file_name()?.to_string_lossy().into_owned())
}

/// Renames the new file over `target` in the judged folder.
pub fn replace_in_folder(folder: &Path, _judged: Option<(u32, u64)>, tmp: &str, target: &str) -> std::io::Result<()> {
    let folder = crate::localfs::judge_io(folder)?;
    let (Some(from), Some(to)) = (folder.child(tmp), folder.child(target)) else { return Err(crate::localfs::refused()) };
    crate::localfs::rename(&from, &to)
}

/// The home folder's Downloads, when there is one.
pub fn downloads() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("HOME")?).join("Downloads");
    dir.is_dir().then_some(dir)
}

#[cfg(test)]
pub fn short_name(_p: &Path) -> Option<String> {
    None
}

/// A staging folder's entries for `sa::unsafe_entry`, never following a link: a symbolic link, a
/// folder, or a file with its hard link count. A folder that cannot be read reads as a link, so the
/// archive is refused.
pub fn stage_entries(dir: &Path) -> Vec<(String, Stat)> {
    use std::os::unix::fs::MetadataExt;
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![(String::new(), Stat::Reparse)] };
    rd.flatten()
        .map(|e| {
            let stat = match e.path().symlink_metadata() {
                Err(_) => Stat::Reparse,
                Ok(m) if m.file_type().is_symlink() => Stat::Reparse,
                Ok(m) if m.is_dir() => Stat::Dir,
                Ok(m) => Stat::File { links: u32::try_from(m.nlink()).unwrap_or(u32::MAX) },
            };
            (e.file_name().to_string_lossy().into_owned(), stat)
        })
        .collect()
}

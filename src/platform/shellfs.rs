//! Files and folders the OS shell knows about: the Desktop folder, the Recycle Bin, the app that
//! edits a script, and the facts of a directory entry the import checks read (a link, its real name).

use std::path::{Path, PathBuf};

use super::Unsupported;

#[cfg(unix)]
use super::unix::shellfs as imp;
#[cfg(windows)]
use super::windows::shellfs as imp;

/// The user's Desktop folder (where Windows keeps it, OneDrive included).
pub fn desktop() -> Option<PathBuf> {
    imp::desktop()
}

/// Moves a file or folder to the Recycle Bin (`trashItemAtURL:`), with undo allowed and no
/// question or error window. Unix has no Trash call yet and says so.
pub fn recycle(path: &Path) -> Result<(), Unsupported> {
    imp::recycle(path)
}

/// The display name of the app that edits `.py` files (the "edit" verb's); None when there is none.
pub fn edit_app_name() -> Option<String> {
    imp::edit_app_name()
}

/// A symbolic link, a junction or any other reparse point.
pub fn is_reparse(p: &Path) -> bool {
    imp::is_reparse(p)
}

/// The name the existing entry at `p` really has (its final path's last part, a link itself never
/// followed); None when there is none.
pub fn final_name(p: &Path) -> Option<String> {
    imp::final_name(p)
}

/// Renames the new file `tmp` over `target` in `folder`, replacing the old file. The folder is opened
/// without following a link and must still be the judged one (`judged`: Windows' volume serial and file
/// index; Unix has no such identity to check). The destination is never resolved again from the textual path.
pub fn replace_in_folder(folder: &Path, judged: Option<(u32, u64)>, tmp: &str, target: &str) -> std::io::Result<()> {
    imp::replace_in_folder(folder, judged, tmp, target)
}

/// The folder the OS keeps Downloads in for this user, when it names one.
pub fn downloads() -> Option<PathBuf> {
    imp::downloads()
}

/// The 8.3 short name of an existing entry when it differs from its own (Windows); None elsewhere.
#[cfg(test)]
pub fn short_name(p: &Path) -> Option<String> {
    imp::short_name(p)
}

//! The clipboard beyond plain text: Copy with Styles (HTML and RTF) and files copied in the file manager.

#[cfg(unix)]
use super::unix::clipboard as imp;
#[cfg(windows)]
use super::windows::clipboard as imp;

/// Replaces the clipboard with `text` and, for programs that take styles, `html` (a full
/// "HTML Format" document) and `rtf`. Unix puts plain text only and refuses a styled copy.
pub fn set(text: &str, html: &str, rtf: &str) -> Result<(), String> {
    imp::set(text, html, rtf)
}

/// The first file on the clipboard (a copied file in Explorer), if any.
pub fn first_file() -> Option<std::path::PathBuf> {
    files().into_iter().next()
}

/// Every file on the clipboard, in order. Empty on Unix, which has no file list yet.
pub fn files() -> Vec<std::path::PathBuf> {
    imp::files()
}

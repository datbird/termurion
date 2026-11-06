//! The clipboard beyond plain text: Copy with Styles (HTML and RTF) and files copied in the file manager.

#[cfg(unix)]
use super::unix::clipboard as imp;
#[cfg(windows)]
use super::windows::clipboard as imp;

/// Replaces the clipboard with `text` and, for programs that take styles, `html` (a full
/// "HTML Format" document) and `rtf`. Unix sets HTML beside the plain text and refuses RTF alone.
pub fn set(text: &str, html: &str, rtf: &str) -> Result<(), String> {
    imp::set(text, html, rtf)
}

/// The app's clipboard: arboard on Windows and macOS; on Linux text may go through the window's
/// Wayland connection instead (see `attach`).
pub use imp::Clipboard;

/// Called with the event loop before the first clipboard is opened: on a Wayland session without
/// the data-control protocol (GNOME, weston) text then goes through the window's own connection.
pub fn attach(display: &dyn winit::raw_window_handle::HasDisplayHandle) {
    imp::attach(display)
}

/// Lets go of what `attach` made, while the display still lives. Twice is safe. True when a
/// clipboard read that never ended keeps part of it alive (Linux): the process must end before the
/// display closes.
pub fn detach() -> bool {
    imp::detach()
}

/// The first file on the clipboard (a copied file in Explorer), if any.
pub fn first_file() -> Option<std::path::PathBuf> {
    files().into_iter().next()
}

/// Every file on the clipboard, in order. Empty on Unix, which has no file list yet.
pub fn files() -> Vec<std::path::PathBuf> {
    imp::files()
}

/// Whether this OS has a primary selection at all (Linux); callers skip the work where it has none.
pub const HAS_PRIMARY_SELECTION: bool = cfg!(not(any(windows, target_os = "macos")));

/// Puts `text` in the primary selection, the buffer a middle click pastes. Linux only; Windows and macOS
/// have none and return Ok.
pub fn set_selection(text: &str) -> Result<(), String> {
    imp::set_selection(text)
}

/// Reads the primary selection off the UI thread and calls `done` with it (None when empty). Windows and
/// macOS call `done(None)` at once.
pub fn request_selection(done: Box<dyn FnOnce(Option<String>) + Send>) {
    imp::request_selection(done)
}

/// Replaces the clipboard with a secret `text`, kept out of the clipboard history and the cloud clipboard
/// where the OS has them (Windows history and cloud clipboard, the Linux password hint, macOS concealed type).
pub fn set_private_text(cb: &mut Clipboard, text: &str) -> Result<(), arboard::Error> {
    imp::set_private_text(cb, text)
}

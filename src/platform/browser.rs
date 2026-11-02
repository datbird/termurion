//! The embedded browser. Windows hosts WebView2 (`webhost_win`, which stays a Windows-only module) and
//! shows web popovers in a popup window; Unix has no web view yet, so `available` reports Unsupported
//! and the callers say so where a page would open.

use super::Unsupported;

#[cfg(unix)]
use super::unix::browser as imp;
#[cfg(windows)]
use super::windows::browser as imp;

#[cfg(windows)]
pub use imp::{WebPopover, html_setup, quick_look_setup};
pub use imp::default_browser_name;

/// The friendly name of the registered handler of https, for the Composer's "This will open ... in
/// <browser>." line; None when there is none or the name is blank.
pub fn https_handler_name() -> Option<String> {
    imp::https_handler_name()
}

/// Ok when pages can be shown in a web view here.
pub fn available() -> Result<(), Unsupported> {
    imp::available()
}

/// Whether a new web view starts InPrivate (a fresh view's default setup says it does not).
pub fn default_view_in_private() -> bool {
    imp::default_view_in_private()
}

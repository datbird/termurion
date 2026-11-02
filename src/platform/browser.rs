//! The embedded browser. Windows hosts WebView2 (`webhost_win`, which stays a Windows-only module) and
//! shows web popovers in a popup window; Unix has no web view yet, so `available` reports Unsupported
//! and the callers say so where a page would open.

use super::Unsupported;

#[cfg(unix)]
use super::unix::browser as imp;
#[cfg(windows)]
use super::windows::browser as imp;

pub use imp::{
    Env, Rasterizer, ViewSetup, WebPopover, WebView, cleanup_files, close_all, default_browser_name, fixed_runtime_check, focus_window, foreground_root,
    local_appdata, prepare_folder, runtime_version,
};

/// What the app tells the view about a notification it holds (`notification_report`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteReport {
    /// Termurion showed it: `ReportShown`, the object stays held.
    Shown,
    /// It was closed (by the person, or the app took it away): `ReportClosed`, then released.
    Closed,
    /// Termurion will not show it: released without a report.
    Release,
}

/// The HTML popover's view: the bridge script, a message channel for it, no automatic windows, no
/// developer tools, every request answered by the popover's `LocalDoc` (the document, or 403), and
/// nothing may load after Termurion's own navigation to it (P6, B4). InPrivate: whatever the script
/// stores stays in memory and goes with the popover (each document has its own origin, so nothing
/// on disk would ever be read again).
pub fn html_setup() -> ViewSetup {
    ViewSetup {
        scripts: vec![crate::web_popover::WEBRTC_OFF_JS.to_string(), crate::webhost::INVOKE_JS.to_string()],
        guard_local_schemes: true,
        in_private: true,
        allowed_schemes: Some(Vec::new()),
        web_message: true,
        resource_filters: vec!["*".to_string()],
        ..Default::default()
    }
}

/// Quick Look's view (`WebUserAgent` as the factory applies it): no bridge, InPrivate (P7), http
/// and https only, through the one navigation verdict (B4).
pub fn quick_look_setup(user_agent: &str) -> ViewSetup {
    ViewSetup {
        user_agent: Some(user_agent.to_string()),
        in_private: true,
        guard_local_schemes: true,
        allowed_schemes: Some(vec!["http".to_string(), "https".to_string()]),
        ..Default::default()
    }
}

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

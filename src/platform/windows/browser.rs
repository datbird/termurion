//! The embedded browser on Windows: WebView2 through `webhost_win`, and the popup that holds a popover's view.

use super::super::Unsupported;

pub use super::web_popover::{WebPopover, default_browser_name, html_setup, quick_look_setup};

pub fn available() -> Result<(), Unsupported> {
    Ok(())
}

pub fn default_view_in_private() -> bool {
    crate::webhost_win::ViewSetup::default().in_private
}

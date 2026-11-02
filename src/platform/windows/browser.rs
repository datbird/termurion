//! The embedded browser on Windows: WebView2 through `webhost_win`, and the popup that holds a popover's view.

use super::super::Unsupported;

pub use super::web_popover::{WebPopover, default_browser_name, html_setup, quick_look_setup};

pub fn available() -> Result<(), Unsupported> {
    Ok(())
}

pub fn default_view_in_private() -> bool {
    crate::webhost_win::ViewSetup::default().in_private
}

/// The https handler's friendly name (`AssocQueryStringW`, `ASSOCSTR_FRIENDLYAPPNAME`); None when there is no handler.
pub fn https_handler_name() -> Option<String> {
    use windows_sys::Win32::UI::Shell::{ASSOCF_NONE, ASSOCSTR_FRIENDLYAPPNAME, AssocQueryStringW};
    let assoc: Vec<u16> = "https\0".encode_utf16().collect();
    let verb: Vec<u16> = "open\0".encode_utf16().collect();
    let mut buf = vec![0u16; 512];
    let mut len = buf.len() as u32;
    let hr = unsafe { AssocQueryStringW(ASSOCF_NONE as _, ASSOCSTR_FRIENDLYAPPNAME, assoc.as_ptr(), verb.as_ptr(), buf.as_mut_ptr(), &mut len) };
    if hr != 0 {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let name = String::from_utf16_lossy(&buf[..end]);
    (!name.trim().is_empty()).then_some(name)
}

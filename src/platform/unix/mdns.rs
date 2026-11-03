//! Bonjour browsing is the Windows DNS-SD API; Unix says so when the setting asks for it.

use crate::bonjour::Event;

pub struct Browser;

impl Browser {
    pub fn start(_notify: crate::platform::pty::Notify) -> Result<Browser, String> {
        Err(crate::platform::Unsupported { feature: "Bonjour discovery" }.to_string())
    }

    pub fn try_event(&self) -> Option<Event> {
        None
    }
}

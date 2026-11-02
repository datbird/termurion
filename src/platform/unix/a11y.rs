//! No screen reader support on Unix yet.

use super::super::a11y::{Request, WindowSnapshot};
use super::super::{Unsupported, unsupported};

const FEATURE: &str = "Accessibility";

pub fn available() -> Result<(), Unsupported> {
    unsupported(FEATURE)
}

/// The provider tree of one window. Nothing makes one on Unix, so it can never exist.
pub struct Host {
    never: std::convert::Infallible,
}

impl Host {
    /// Always Err: there is no accessibility backend, and the caller shows the text.
    pub fn install(_hwnd: isize, _send: Box<dyn Fn(Request) + Send + Sync>) -> Result<Host, String> {
        Err(Unsupported { feature: FEATURE }.into())
    }

    pub fn publish(&self, _snap: WindowSnapshot) {
        match self.never {}
    }

    pub fn raise(&self, _pane: u64, _e: &crate::access::Event) {
        match self.never {}
    }

    pub fn raise_focus(&self, _pane: u64) {
        match self.never {}
    }

    /// No client can listen without a backend.
    pub fn listening() -> bool {
        false
    }
}

/// Nothing is connected on Unix, so there is nothing to drop (process teardown calls it on every OS).
pub fn disconnect_all() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_a11y_unsupported() {
        assert!(available().unwrap_err().to_string().contains("Accessibility is not available"));
        let e = Host::install(0, Box::new(|_| {})).err().unwrap();
        assert!(e.contains("Accessibility is not available"), "{e}");
        assert!(!Host::listening());
    }
}

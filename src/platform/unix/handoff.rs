//! No default-terminal handoff on Unix. A `Handoff` and a `Server` can never exist (Infallible), and
//! registering reports Unsupported.

use std::convert::Infallible;

use super::super::Unsupported;
use super::super::pty::{Notify, Pty, PtyApi};

const FEATURE: &str = "Default terminal";

/// A console handed to Termurion. Nothing makes one on Unix.
#[derive(Debug)]
pub struct Handoff {
    never: Infallible,
}

/// The registered class. Nothing makes one on Unix.
pub struct Server {
    never: Infallible,
}

impl Server {
    pub fn take_ignored(&self) -> usize {
        match self.never {}
    }
}

pub fn can_serve(_api: &PtyApi) -> bool {
    false
}

pub fn register(_send: Box<dyn Fn(Handoff) + Send + Sync>, _api: &PtyApi) -> Result<Server, String> {
    Err(Unsupported { feature: FEATURE }.into())
}

/// Nothing arrives on Unix, so there is nothing to refuse (process teardown calls it on every OS).
pub fn begin_closing() {}

pub fn into_pty(_api: &PtyApi, h: Handoff, _notify: Notify) -> Result<(Pty, String), String> {
    match h.never {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_handoff_unsupported() {
        let e = register(Box::new(|_| {}), &PtyApi).err().unwrap();
        assert!(e.contains("Default terminal is not available"), "{e}");
        assert!(!can_serve(&PtyApi));
    }
}

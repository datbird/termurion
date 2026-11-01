//! No pseudo terminal on Unix yet (P1). Starting a shell reports Unsupported; a `Pty` can never exist.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use crate::platform::Unsupported;
use crate::platform::pty::{Notify, Read};
use crate::ssh_transport::Transport;

const FEATURE: &str = "Local shells";

/// Nothing to load on Unix.
#[derive(Clone, Debug, Default)]
pub struct PtyApi;

/// A running child. Nothing makes one on Unix, so every method is unreachable.
pub struct Pty {
    never: std::convert::Infallible,
}

fn no<T>() -> Result<T, String> {
    Err(Unsupported { feature: FEATURE }.into())
}

pub fn spawn(_api: &PtyApi, _cmdline: &str, _cols: u16, _rows: u16) -> Result<Pty, String> {
    no()
}

pub fn spawn_notify(_api: &PtyApi, _cmdline: &str, _cwd: Option<&std::path::Path>, _env: &[String], _cols: u16, _rows: u16, _notify: Notify) -> Result<Pty, String> {
    no()
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_transport(_api: &PtyApi, _t: Transport, _cmdline: &str, _cwd: Option<&std::path::Path>, _env: &[String], _cols: u16, _rows: u16, _notify: Notify) -> Result<Pty, String> {
    no()
}

/// The input mode of a console. Unix has no console mode, so this always reports Unsupported.
pub fn console_mode_unless(_pid: u32, _paused: &AtomicBool) -> Result<u32, String> {
    Err(Unsupported { feature: "Console input mode" }.into())
}

#[cfg(test)]
pub fn test_api() -> PtyApi {
    PtyApi
}

impl Pty {
    pub fn hold(&mut self, _what: Box<dyn std::any::Any + Send>) {
        match self.never {}
    }

    pub fn terminate(&self) {
        match self.never {}
    }

    pub fn write(&self, _bytes: &[u8]) -> Result<(), String> {
        match self.never {}
    }

    pub fn resize(&self, _cols: u16, _rows: u16) -> Result<(), String> {
        match self.never {}
    }

    pub fn clear(&self) -> Result<(), String> {
        match self.never {}
    }

    pub fn try_read(&self) -> Read {
        match self.never {}
    }

    pub fn pid(&self) -> u32 {
        match self.never {}
    }

    pub fn set_paused(&self, _paused: bool) {
        match self.never {}
    }

    pub fn pause_after_reads(&self) {
        match self.never {}
    }

    pub fn is_paused(&self) -> bool {
        match self.never {}
    }

    pub fn pause_flag(&self) -> Arc<AtomicBool> {
        match self.never {}
    }

    pub fn pending(&self) -> usize {
        match self.never {}
    }

    pub fn exit_code(&self) -> Option<u32> {
        match self.never {}
    }

    pub fn read_timeout(&self, _wait: Duration) -> Option<Vec<u8>> {
        match self.never {}
    }

    pub fn read_until(&self, _needle: &[u8], _wait: Duration) -> Result<Vec<u8>, Vec<u8>> {
        match self.never {}
    }

    pub fn wait_exit(&self, _wait: Duration) -> Option<u32> {
        match self.never {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_spawn_reports_unsupported() {
        let e = spawn(&PtyApi, "sh", 80, 24).err().unwrap();
        assert!(e.contains("Local shells is not available"), "{e}");
        let e = spawn_transport(&PtyApi, Transport::Conpty, "sh", None, &[], 80, 24, Arc::new(|| {})).err().unwrap();
        assert!(e.contains("Local shells is not available"), "{e}");
    }

    #[test]
    fn unix_console_mode_reports_unsupported() {
        let e = console_mode_unless(1, &AtomicBool::new(false)).unwrap_err();
        assert!(e.contains("not available"), "{e}");
    }
}

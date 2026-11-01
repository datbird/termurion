//! No session-host IPC on Unix yet (the Unix socket is P1). Every call reports Unsupported.

use std::time::Duration;

use crate::api_peer::Peer;
use crate::platform::Unsupported;

const FEATURE: &str = "Session host";

fn no<T>() -> Result<T, String> {
    Err(Unsupported { feature: FEATURE }.into())
}

/// A connected pipe end. Nothing makes one on Unix, so it can never exist.
pub struct Pipe {
    never: std::convert::Infallible,
}

/// The listening side. Nothing makes one on Unix.
pub struct Listener {
    never: std::convert::Infallible,
}

/// Why `connect_owned` failed.
#[derive(Debug)]
pub enum ConnectError {
    /// The pipe's owner refused this user.
    Denied(String),
    Other(String),
}

impl Listener {
    pub fn bind(_name: &str) -> Result<Listener, String> {
        no()
    }

    pub fn renew(&mut self) -> Result<(), String> {
        match self.never {}
    }

    pub fn accept(&mut self, _wait: Duration) -> Result<Option<Pipe>, String> {
        match self.never {}
    }
}

impl Pipe {
    pub fn read(&self, _buf: &mut [u8]) -> Result<usize, String> {
        match self.never {}
    }

    pub fn read_timeout(&self, _buf: &mut [u8], _wait: Duration) -> Result<Option<usize>, String> {
        match self.never {}
    }

    pub fn write_all(&self, _bytes: &[u8]) -> Result<(), String> {
        match self.never {}
    }

    pub fn shutdown(&self) {
        match self.never {}
    }

    pub fn server_pid(&self) -> Result<u32, String> {
        match self.never {}
    }

    /// Denies: no Unix peer check exists yet, so nothing is trusted.
    pub fn server_is_this_user(&self) -> Result<(), String> {
        match self.never {}
    }
}

pub fn connect(_name: &str, _wait: Duration) -> Result<Option<Pipe>, String> {
    no()
}

pub fn connect_owned(_name: &str, _wait: Duration) -> Result<Option<Pipe>, ConnectError> {
    Err(ConnectError::Other(Unsupported { feature: FEATURE }.to_string()))
}

pub fn host_pipe_name() -> Result<String, String> {
    no()
}

pub fn secret_pipe_name() -> Result<String, String> {
    no()
}

pub fn url_pipe_name() -> Result<String, String> {
    no()
}

/// Denies: the peer facts cannot be read, so a caller cannot judge a peer.
pub fn own_peer() -> Result<Peer, String> {
    no()
}

/// Denies: as `own_peer`.
pub fn thread_peer() -> Result<Peer, String> {
    no()
}

/// A name no other test run shares.
#[cfg(test)]
pub(crate) fn test_name(what: &str) -> String {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    format!("termurion-test-{what}-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_ipc_connect_is_unsupported() {
        let e = connect("x", Duration::ZERO).err().unwrap();
        assert!(e.contains("Session host is not available"), "{e}");
    }

    #[test]
    fn unix_ipc_checks_deny() {
        assert!(own_peer().is_err());
        assert!(thread_peer().is_err());
        assert!(host_pipe_name().is_err());
        assert!(matches!(connect_owned("x", Duration::ZERO), Err(ConnectError::Other(e)) if e.contains("Session host is not available")));
    }
}

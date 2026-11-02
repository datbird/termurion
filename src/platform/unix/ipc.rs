//! No session-host IPC on Unix yet (the Unix socket is P1). Every call reports Unsupported.

use std::time::Duration;

use crate::api_peer::Peer;
use crate::platform::Unsupported;

const FEATURE: &str = "Session host";
const ALLOW_ALL: &str = "The automation allow-all file";

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

/// Denies: whether this process is elevated cannot be read, and a caller treats Err as "not elevated, not trusted".
pub fn elevated() -> Result<bool, String> {
    no()
}

/// Denies: no user identity text exists here.
pub fn user_sid() -> Result<String, String> {
    no()
}

pub fn available() -> Result<(), Unsupported> {
    Err(Unsupported { feature: ALLOW_ALL })
}

pub fn set_file_owner_to_user(_file: &std::fs::File) -> Result<(), String> {
    Err(Unsupported { feature: ALLOW_ALL }.into())
}

pub fn create_new_for_write(_path: &str, _set_owner: bool) -> Result<std::fs::File, String> {
    Err(Unsupported { feature: ALLOW_ALL }.into())
}

/// No peer check on Unix yet, so every connection is refused.
pub fn check_peer(_peer: std::net::SocketAddr, _local: std::net::SocketAddr) -> Result<u32, String> {
    no()
}

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
    fn unix_ipc_elevated_denies() {
        assert!(elevated().is_err());
        assert!(user_sid().is_err());
        assert!(available().is_err());
        assert!(create_new_for_write("x", true).is_err());
    }

    #[test]
    fn unix_ipc_checks_deny() {
        assert!(own_peer().is_err());
        assert!(thread_peer().is_err());
        assert!(host_pipe_name().is_err());
        assert!(matches!(connect_owned("x", Duration::ZERO), Err(ConnectError::Other(e)) if e.contains("Session host is not available")));
    }
}

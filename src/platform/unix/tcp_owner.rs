//! Who owns a TCP connection or a listening port. Unix has no lookup yet and says so.

use std::net::SocketAddr;

use crate::api_peer::Row;
use crate::platform::Unsupported;

/// No peer check on Unix yet, so every connection is refused.
pub fn check(_peer: SocketAddr, _local: SocketAddr) -> Result<u32, String> {
    Err(Unsupported { feature: "The automation API" }.into())
}

/// No connection table on Unix yet.
pub fn tcp_rows() -> Result<Vec<Row>, String> {
    Err(Unsupported { feature: "Finding the process on a connection" }.to_string())
}

/// The listening-port table is a Windows call; Unix reports that it is not available.
pub fn listen_rows() -> Result<Vec<Row>, String> {
    Err(Unsupported { feature: "Finding the process on a port" }.to_string())
}

/// The name of a process by id is a Windows call; Unix has none yet.
pub fn process_name(_pid: u32) -> Option<String> {
    None
}

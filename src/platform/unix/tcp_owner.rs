//! Who owns a TCP connection or a listening port. Unix has no lookup yet and says so.

use std::net::SocketAddr;

use crate::api_peer::Row;
use crate::platform::Unsupported;

/// No peer check on Unix yet, so every connection is refused: the one refusal `ipc::check_peer` gives too.
pub fn check(peer: SocketAddr, local: SocketAddr) -> Result<u32, String> {
    super::ipc::check_peer(peer, local)
}

/// No connection table on Unix yet.
pub fn tcp_rows() -> Result<Vec<Row>, String> {
    Err(Unsupported { feature: "Finding the process on a connection" }.to_string())
}

/// The listening-port table is a Windows call; Unix reports that it is not available.
pub fn listen_rows() -> Result<Vec<Row>, String> {
    Err(Unsupported { feature: "Finding the process on a port" }.to_string())
}

/// The name of a process by id: the one Unix answer, `platform::proc::process_name`.
pub fn process_name(pid: u32) -> Option<String> {
    crate::platform::proc::process_name(pid)
}

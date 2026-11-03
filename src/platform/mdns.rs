//! Local-network discovery of hosts (Bonjour): Windows browses with the DNS-SD API; Unix says it is
//! not available. What is found stays untrusted, and `bonjour` cleans it.

#[cfg(unix)]
use super::unix::mdns as imp;
#[cfg(windows)]
use super::windows::mdns as imp;

pub use imp::Browser;

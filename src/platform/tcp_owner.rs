//! Who owns a TCP connection or a listening port: Windows reads the machine's TCP table; Unix has no
//! lookup yet and says so.

#[cfg(unix)]
use super::unix::tcp_owner as imp;
#[cfg(windows)]
use super::windows::tcp_owner as imp;

pub use imp::{check, listen_rows, process_name, tcp_rows};

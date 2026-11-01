//! The origin secret pipe it2ssh asks for a secret. Windows serves it on a named pipe; Unix does not
//! serve one yet and says so.

use std::sync::{Arc, Mutex};

use crate::ssh_secret::Secrets;

#[cfg(unix)]
use super::unix::ssh_secret as imp;
#[cfg(windows)]
use super::windows::ssh_secret as imp;

/// The serving thread; dropping it stops the pipe.
pub use imp::Server;

/// Serves secrets on this user's pipe.
pub fn start(secrets: Arc<Mutex<Secrets>>) -> Result<Server, String> {
    imp::start(secrets)
}

/// The text of a start error with this user's account id left out (the pipe name carries it).
pub fn scrub_error(e: String) -> String {
    imp::scrub_error(e)
}

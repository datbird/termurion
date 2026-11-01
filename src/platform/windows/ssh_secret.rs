//! The origin secret pipe on Windows.

use std::sync::{Arc, Mutex};

use crate::ssh_secret::Secrets;

pub use crate::ssh_secret_win::Server;

pub fn start(secrets: Arc<Mutex<Secrets>>) -> Result<Server, String> {
    Server::start(secrets)
}

pub fn scrub_error(e: String) -> String {
    match super::ipc::user_sid() {
        Ok(sid) => e.replace(&sid, "<SID>"),
        Err(_) => e,
    }
}

//! No origin secret pipe on Unix yet.

use std::sync::{Arc, Mutex};

use crate::ssh_secret::Secrets;

/// Never made on Unix; `start` fails.
pub struct Server;

pub fn start(_secrets: Arc<Mutex<Secrets>>) -> Result<Server, String> {
    super::super::unsupported("The SSH origin secret pipe").map_err(|e| e.to_string())
}

pub fn scrub_error(e: String) -> String {
    e
}

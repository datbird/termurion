//! No release feed fetch on Unix yet: a check ends with the Unsupported text.

use std::sync::Mutex;

use crate::platform::pty::Notify;
use crate::update::{Fetch, Fetched};

fn unsupported() -> Fetched {
    Fetched::Transport(crate::platform::Unsupported { feature: "Checking for updates" }.to_string())
}

pub struct CurlFetch;

impl Fetch for CurlFetch {
    fn get(&self, _url: &str) -> Fetched {
        unsupported()
    }
}

/// Answers every check at once with the Unsupported text.
pub struct Checker {
    notify: Notify,
    result: Mutex<Option<Fetched>>,
}

impl Checker {
    pub fn start(_fetch: Box<dyn Fetch + Send>, notify: Notify) -> Checker {
        Checker { notify, result: Mutex::new(None) }
    }

    pub fn check(&self) {
        *self.result.lock().unwrap_or_else(|e| e.into_inner()) = Some(unsupported());
        (self.notify)();
    }

    pub fn busy(&self) -> bool {
        false
    }

    pub fn try_result(&self) -> Option<Fetched> {
        self.result.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    pub fn cancel(&self) {
        *self.result.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

pub fn cancel_running() {}

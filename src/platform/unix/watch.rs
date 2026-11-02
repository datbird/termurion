//! No folder watch on Unix yet. The Dynamic Profiles scanner never asks for a reload (the first
//! read at start still runs); the Scripts scanner gives one empty result and nothing more, as it
//! does for a folder behind a network link. Each says so once in the debug log.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::platform::pty::Notify;
use crate::scripts::ScanResult;

fn note() {
    static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        crate::dlog!("Folder watching is not available on this platform yet");
    }
}

/// Dynamic Profiles are read on Windows only until the trusted open exists here.
pub fn dynamic_profiles() -> Result<(), crate::platform::Unsupported> {
    Err(crate::platform::Unsupported { feature: "Dynamic Profiles" })
}

/// Stops every watch: there is none.
pub fn stop_all() {}

pub struct DynamicScanner {
    folder: PathBuf,
}

impl DynamicScanner {
    pub fn start(folder: PathBuf, _latency: Duration, _notify: Notify) -> DynamicScanner {
        note();
        DynamicScanner { folder }
    }

    /// Never: nothing watches the folder.
    pub fn take_reload(&self) -> bool {
        false
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }
}

#[derive(Debug)]
pub enum ScanEvent {
    /// The scan passed its time limit: `answer_slow` says whether it goes on.
    Slow,
    Done(ScanResult),
}

pub struct ScriptsScanner {
    folder: PathBuf,
    first: std::cell::Cell<bool>,
}

impl ScriptsScanner {
    pub fn start(folder: PathBuf, notify: Notify) -> ScriptsScanner {
        note();
        notify();
        ScriptsScanner { folder, first: std::cell::Cell::new(true) }
    }

    /// The one empty result, then nothing.
    pub fn try_event(&self) -> Option<ScanEvent> {
        self.first.replace(false).then(|| ScanEvent::Done(ScanResult::default()))
    }

    pub fn answer_slow(&self, _go_on: bool) {}

    pub fn hold(&mut self, _on: bool) {}

    pub fn rescan(&self) {}

    pub fn folder(&self) -> &Path {
        &self.folder
    }
}

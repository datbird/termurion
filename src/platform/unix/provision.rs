//! Unix runs no provisioning yet. Each job ends with the Unsupported text.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::script_provision::{Job, Outcome, Runner, Step, BUILD_TIMEOUT};

/// What a program runner would do, which Unix cannot yet.
fn provisioning_unsupported() -> String {
    crate::platform::Unsupported { feature: "Installing Python for scripts" }.to_string()
}

pub struct RealRunner {
    pub lines: std::sync::mpsc::Sender<String>,
    pub timeout: Duration,
    pub cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

impl RealRunner {
    pub fn new(lines: std::sync::mpsc::Sender<String>) -> RealRunner {
        RealRunner { lines, timeout: BUILD_TIMEOUT, cancel: None }
    }

    /// The System32 tools are Windows'; Unix names the missing feature.
    pub fn system_tool(name: &str) -> Result<PathBuf, String> {
        Err(format!("{name} is not used: {}", provisioning_unsupported()))
    }
}

impl Runner for RealRunner {
    fn run(&mut self, _step: &Step) -> Result<String, String> {
        Err(provisioning_unsupported())
    }

    fn download(&mut self, _url: &str, _to: &Path) -> Result<(), String> {
        Err(provisioning_unsupported())
    }

    fn sha256(&mut self, _file: &Path) -> Result<String, String> {
        Err(provisioning_unsupported())
    }

    fn extract(&mut self, _zip: &Path, _to: &Path) -> Result<(), String> {
        Err(provisioning_unsupported())
    }

    fn exists(&self, p: &Path) -> bool {
        crate::localfs::local_exists(p)
    }

    fn find_file(&self, dir: &Path, name: &str) -> Option<PathBuf> {
        Some(dir.join(name)).filter(|p| crate::localfs::is_local_file(p))
    }

    fn list_dirs(&self, _dir: &Path) -> Vec<PathBuf> {
        Vec::new()
    }

    fn rename(&mut self, _from: &Path, _to: &Path) -> Result<(), String> {
        Err(provisioning_unsupported())
    }

    fn remove(&mut self, _p: &Path) {}

    fn write(&mut self, _p: &Path, _text: &str) -> Result<(), String> {
        Err(provisioning_unsupported())
    }

    fn log(&mut self, line: &str) {
        let _ = self.lines.send(line.to_string());
    }
}

/// Every job submitted ends at once with the Unsupported text.
pub struct Provisioner {
    done: std::cell::RefCell<std::collections::VecDeque<(u64, Result<Outcome, String>)>>,
    next: std::cell::Cell<u64>,
    notify: crate::platform::pty::Notify,
}

impl Provisioner {
    pub fn start(_uv_dir: PathBuf, notify: crate::platform::pty::Notify) -> Provisioner {
        Provisioner { done: Default::default(), next: std::cell::Cell::new(1), notify }
    }

    pub fn submit(&self, _job: Job) -> u64 {
        let id = self.next.get();
        self.next.set(id + 1);
        self.done.borrow_mut().push_back((id, Err(provisioning_unsupported())));
        (self.notify)();
        id
    }

    pub fn try_done(&self) -> Option<(u64, Result<Outcome, String>)> {
        self.done.borrow_mut().pop_front()
    }

    pub fn try_line(&self) -> Option<String> {
        None
    }

    pub fn cancel(&self) {}

    pub fn busy(&self) -> bool {
        !self.done.borrow().is_empty()
    }
}

/// There is no System32 folder: none.
pub fn system_directory() -> Option<PathBuf> {
    None
}

/// The job a script's processes run in (Windows' Job Object). None exists on Unix.
pub(crate) use super::proc::KillJob;

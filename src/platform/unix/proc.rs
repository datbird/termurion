//! No process tree reader on Unix yet. Job info is cosmetic, so these answer None or empty and
//! say so once in the debug log.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread::JoinHandle;

use crate::platform::Unsupported;
use crate::proctree::{Job, Proc};

fn note() {
    static LOGGED: AtomicBool = AtomicBool::new(false);
    if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        crate::dlog!("Process information is not available on this platform yet");
    }
}

pub fn foreground_job(_pid: u32) -> Option<Job> {
    note();
    None
}

pub fn process_image(_pid: u32) -> Option<String> {
    note();
    None
}

pub fn children(_pid: u32) -> Vec<u32> {
    note();
    Vec::new()
}

/// What a shell left running after it exited. Reports Unsupported, so a caller shows it.
pub fn orphans_of(_dead: u32) -> Result<Vec<Proc>, String> {
    note();
    Err(Unsupported { feature: "Process information" }.into())
}

/// The names of the listed processes that still run: none can be told, so none.
pub fn still_running(_set: &[(u32, u64)]) -> Vec<String> {
    note();
    Vec::new()
}

/// Sends Ctrl+C or Ctrl+Break to a console. Unix has no consoles to send it to.
pub fn console_event(_pid: u32, _ctrl_break: bool, _stop: Arc<AtomicBool>) -> JoinHandle<Result<(), String>> {
    std::thread::spawn(|| Err(Unsupported { feature: "Console signals" }.into()))
}

/// Starts a program detached from this process. Only the session host needs it, and it has no Unix backend yet.
pub fn start_detached(_exe: &std::path::Path) -> Result<(), String> {
    Err(Unsupported { feature: "Session host" }.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_proc_answers_none_and_empty() {
        assert!(foreground_job(1).is_none());
        assert!(process_image(1).is_none());
        assert!(children(1).is_empty());
        assert!(still_running(&[(1, 1)]).is_empty());
        assert!(start_detached(std::path::Path::new("x")).unwrap_err().contains("Session host is not available"));
        assert!(orphans_of(1).unwrap_err().contains("Process information is not available"));
    }
}

//! Process facts on Windows, from the Toolhelp reader in `jobwatch` (a Windows-only module).

use crate::proctree::{self, Job};

pub use crate::jobwatch::{console_event, orphans_of, still_running};

/// The job a shell shows: the deepest console program under it. None when the shell is gone.
pub fn foreground_job(pid: u32) -> Option<Job> {
    let procs = crate::jobwatch::snapshot(&[pid], &mut crate::jobwatch::Cache::default()).ok()?;
    proctree::display_job(&procs, pid)
}

/// The full path of the program `pid` runs.
pub fn process_image(pid: u32) -> Option<String> {
    crate::jobwatch::process_image(pid)
}

/// The ids of the processes started by `pid`.
pub fn children(pid: u32) -> Vec<u32> {
    crate::jobwatch::children_of(pid)
}

/// Starts `exe` detached from this process (its own process group, out of this process's job
/// when Windows allows), so it outlives it.
pub fn start_detached(exe: &std::path::Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let spawn = |flags: u32| {
        std::process::Command::new(exe)
            .creation_flags(flags)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
    };
    let base = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    // A job that forbids breakaway refuses the flag; the child then lives in this process's job.
    spawn(base | CREATE_BREAKAWAY_FROM_JOB).or_else(|_| spawn(base)).map(drop).map_err(|e| format!("could not start {}: {e}", exe.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_process_has_an_image_and_a_child_list() {
        let me = std::process::id();
        let image = process_image(me).expect("own image path");
        assert!(image.to_ascii_lowercase().ends_with(".exe"), "{image}");
        let mut child = std::process::Command::new("cmd.exe").args(["/c", "ping -n 3 127.0.0.1 >nul"]).stdout(std::process::Stdio::null()).spawn().unwrap();
        assert!(children(me).contains(&child.id()));
        let _ = child.kill();
        let _ = child.wait();
    }
}

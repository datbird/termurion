//! Process facts on Windows, from the Toolhelp reader in `jobwatch` (a Windows-only module).

use crate::proctree::{self, Job};

pub use crate::jobwatch::{console_event, descendants_now, orphans_of, process_alive, signal_process, still_running, JobWatch, Sent};
pub use crate::pwm_run::ProcessRunner;
pub use crate::script_proc::{ScriptProcess, KILLED_CODE};
pub(crate) use crate::script_provision::KillJob;


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

/// Waits up to 30 seconds for the process `pid` to end (the relaunch waiting for the old
/// Termurion); at once when it cannot be opened (already gone).
pub fn wait_for_pid(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
    // SAFETY: a handle opened here, waited on and closed here.
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if h.is_null() {
            return;
        }
        WaitForSingleObject(h, 30_000);
        CloseHandle(h);
    }
}

/// One line in the debug output (`OutputDebugStringW`, iTerm2's `NSLog`).
pub fn debug_line(text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    // SAFETY: a NUL-terminated wide string that outlives the call.
    unsafe { windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringW(wide.as_ptr()) };
}

/// A command that runs `line` through cmd.exe as written: the one argument builder
/// (`semantic::cmd_args`) refuses what cmd.exe would read as syntax. The caller sets the rest and starts it.
pub fn command_for_line(line: &str) -> Result<std::process::Command, crate::platform::Unsupported> {
    use std::os::windows::process::CommandExt;
    let mut c = std::process::Command::new("cmd.exe");
    c.raw_arg(crate::semantic::cmd_args(line));
    Ok(c)
}

/// The child gets no console window of its own (`CREATE_NO_WINDOW`); its own windows still show.
pub fn hide_console(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
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

//! Process facts for the shell job, the close prompt and the console signals. Windows reads the
//! Toolhelp snapshot; Unix reads its kernel's process table (`proc_table`).

#[cfg(unix)]
use super::unix::proc as imp;
#[cfg(windows)]
use super::windows::proc as imp;

pub use imp::{children, command_for_line, hide_console, console_event, debug_line, descendants_now, foreground_job, orphans_of, process_alive, process_image, signal_process, start_detached, still_running, wait_for_pid, JobWatch, ProcessRunner, ScriptProcess, Sent, KILLED_CODE};
pub(crate) use imp::KillJob;

/// The file name of the program `pid` runs, or None when it is gone or off limits.
pub fn process_name(pid: u32) -> Option<String> {
    let image = process_image(pid)?;
    let name = std::path::Path::new(&image).file_name()?;
    Some(name.to_string_lossy().into_owned())
}

//! Process facts for the shell job, the close prompt and the console signals. Windows reads the
//! Toolhelp snapshot; Unix has no reader yet and answers none or empty.

#[cfg(unix)]
use super::unix::proc as imp;
#[cfg(windows)]
use super::windows::proc as imp;

pub use imp::{children, command_for_line, hide_console, console_event, debug_line, descendants_now, foreground_job, orphans_of, process_alive, process_image, signal_process, start_detached, still_running, wait_for_pid, JobWatch, ProcessRunner, ScriptProcess, Sent, KILLED_CODE};
pub(crate) use imp::KillJob;

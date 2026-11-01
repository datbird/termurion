//! Process facts for the shell job, the close prompt and the console signals. Windows reads the
//! Toolhelp snapshot; Unix has no reader yet and answers none or empty.

#[cfg(unix)]
use super::unix::proc as imp;
#[cfg(windows)]
use super::windows::proc as imp;

pub use imp::{children, console_event, foreground_job, orphans_of, process_image, start_detached, still_running};

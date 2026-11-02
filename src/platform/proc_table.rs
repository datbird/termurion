//! The process table, read per OS: every process's pid, parent and name, and the facts of one
//! opened process (start time, image, command line). `jobwatch` and `proctree` stay OS-free on top.

#[cfg(unix)]
use super::unix::proc_table as imp;
#[cfg(windows)]
use super::windows::proc_table as imp;

pub use imp::{open, rows, wait_any, Opened};

/// One process in the table.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub pid: u32,
    pub ppid: u32,
    /// The image name (Windows) or the kernel's command name (Unix).
    pub name: String,
    /// Not in its terminal's foreground group (Unix); always false on Windows.
    pub background: bool,
}

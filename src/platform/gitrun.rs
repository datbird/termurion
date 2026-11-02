//! Runs git for the status bar and the git menu: hidden, in a job that ends its process tree at the
//! deadline. The tree kill is a Windows Job Object; Unix has no runner yet.

#[cfg(unix)]
use super::unix::gitrun as imp;
#[cfg(windows)]
use super::windows::gitrun as imp;

pub use imp::SystemGitRun;

//! Reading a command's help for the AI: Windows runs PowerShell's `Get-Help`; Unix says it is not there.

#[cfg(unix)]
use super::unix::cmd_help as imp;
#[cfg(windows)]
use super::windows::cmd_help as imp;

pub use imp::get_help;

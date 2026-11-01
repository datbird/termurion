//! A coprocess: a command run beside a session, fed its output. Windows runs it hidden in a
//! Job Object (`coproc_win`, a Windows-only module); Unix has none yet.

#[cfg(unix)]
pub use super::unix::coproc::Coprocess;
#[cfg(windows)]
pub use crate::coproc_win::Coprocess;

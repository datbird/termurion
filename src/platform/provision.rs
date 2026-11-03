//! Running the programs that provision Python environments for scripts: the runner, the job that ends
//! everything a program started, the System32 folder, and the worker. The order of the steps stays
//! in `script_provision`; Unix runs none of it yet and says so.

#[cfg(unix)]
use super::unix::provision as imp;
#[cfg(windows)]
use super::windows::provision as imp;

pub use imp::{system_directory, Provisioner, RealRunner};
pub(crate) use imp::KillJob;

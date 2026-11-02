//! Facts about this machine for the debug log header and footer.

#[cfg(unix)]
use super::unix::sysinfo as imp;
#[cfg(windows)]
use super::windows::sysinfo as imp;

/// The processor, the OS product name and its build, each empty when unknown.
pub struct Machine {
    pub cpu: String,
    pub product: String,
    pub build: String,
}

pub fn machine() -> Machine {
    imp::machine()
}

//! The OS switches that gate what a web page may ask for (read only; a missing value is None).
//! Windows reads the notification and location consent values; Unix has none to read yet, so every
//! value is None and the page-permission code treats the system as not saying no.

#[cfg(unix)]
use super::unix::privacy as imp;
#[cfg(windows)]
use super::windows::privacy as imp;

/// The Windows privacy switches.
#[derive(Default)]
pub struct Privacy {
    pub toast: Option<u32>,
    pub machine: Option<String>,
    pub user: Option<String>,
    pub non_packaged: Option<String>,
}

/// Reads the switches now.
pub fn read() -> Privacy {
    imp::read()
}

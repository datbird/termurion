//! The Terminal tab's locale line.

use super::Unsupported;

#[cfg(unix)]
use super::unix::locale as imp;
#[cfg(windows)]
use super::windows::locale as imp;

/// The Terminal tab's locale line: the locale shown after `LANG=` (Custom and Automatic) and
/// the text (its title, or the status).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub lang: Option<String>,
    pub text: String,
}

/// The status for a profile's mode value, custom locale, encoding and `FallbackLCCType`.
pub fn status(mode: i64, custom: &str, ns_encoding: i64, fallback: &str) -> Result<Status, Unsupported> {
    imp::status(mode, custom, ns_encoding, fallback)
}

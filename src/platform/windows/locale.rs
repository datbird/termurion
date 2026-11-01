//! The locale line, from `locale_win`.

use super::super::Unsupported;
use super::super::locale::Status;

pub fn status(mode: i64, custom: &str, ns_encoding: i64, fallback: &str) -> Result<Status, Unsupported> {
    Ok(crate::locale_win::status(mode, custom, ns_encoding, fallback))
}

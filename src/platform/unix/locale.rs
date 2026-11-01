//! The locale line is not read on Unix yet.

use super::super::Unsupported;
use super::super::locale::Status;

pub fn status(_mode: i64, _custom: &str, _ns_encoding: i64, _fallback: &str) -> Result<Status, Unsupported> {
    super::super::unsupported("The locale status line")
}

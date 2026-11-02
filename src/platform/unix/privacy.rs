//! No privacy switches to read on Unix yet.

use super::super::privacy::Privacy;

/// Nothing is read, so every value is None.
pub fn read() -> Privacy {
    Privacy::default()
}

//! The approval key at rest: Unix has no DPAPI and keeps no key file, so every import asks.

use crate::workgroup_restore::KEY_LEN;

/// DPAPI is Windows'; on Unix every call fails, which callers read as "could not protect".
/// An unreachable stand-in: its only callers (the card file in `browser_autofill`) are Windows-only.
pub fn dpapi_with(_data: &[u8], _protect: bool, _entropy: &[u8]) -> Option<Vec<u8>> {
    None
}

/// No DPAPI on Unix: there is no protected key.
pub fn protect_key(_key: &[u8; KEY_LEN]) -> Option<Vec<u8>> {
    None
}

/// No DPAPI on Unix: no blob is a key.
pub fn unprotect_key(_blob: &[u8]) -> Option<[u8; KEY_LEN]> {
    None
}

/// No key file is kept on Unix yet (it is owner-only by Windows ACL): every import asks.
pub fn key_in(_dir: &std::path::Path) -> Option<[u8; KEY_LEN]> {
    None
}

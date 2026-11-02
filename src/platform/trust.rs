//! Checking the signature of a signed script archive (`.its`): Windows' own chain and signature check
//! (WinVerifyTrust); Unix has none yet.

#[cfg(unix)]
use super::unix::trust as imp;
#[cfg(windows)]
use super::windows::trust as imp;

/// Verifies the bytes of an `.its` file: the layout (`sigarchive`) and the chain and signature (the OS).
pub fn verify_archive(bytes: &[u8]) -> Result<crate::sigarchive::Verified, String> {
    imp::verify_archive(bytes)
}

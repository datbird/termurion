//! Windows' chain and signature check, through `sigarchive_win`.

pub fn verify_archive(bytes: &[u8]) -> Result<crate::sigarchive::Verified, String> {
    crate::sigarchive::verify_file(bytes, &crate::sigarchive_win::CryptoApi::online())
}

//! No signature check on Unix yet: a signed archive is refused with the reason.

pub fn verify_archive(_bytes: &[u8]) -> Result<crate::sigarchive::Verified, String> {
    Err(super::super::Unsupported { feature: "Verifying a signed script archive", plural: false }.into())
}

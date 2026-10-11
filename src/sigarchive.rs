//! iTerm2's signed script archive (`.its`): its chunk format, the header, metadata and signature
//! rules (`SignedArchive/SIGArchive*.m`), and what the importer says about it. Pure: the
//! certificate chain and the RSA signature are checked by a `Verifier` (`sigarchive_win`).
//! The file is untrusted input: every length is checked arithmetic, nothing allocates from a
//! length the file claims, and nothing is unpacked here.

use sha2::{Digest, Sha256};

/// The header chunk's text (`SIGArchiveHeaderMagicString`).
pub const MAGIC: &str = "signed-archive";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tag {
    Header = 0,
    Payload = 1,
    Metadata = 2,
    Signature = 3,
    Certificate = 4,
    Signature2 = 5,
}

/// One chunk: its tag, where its payload starts and how long that is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub tag: i64,
    pub offset: u64,
    pub len: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parsed {
    /// The payload zip: offset and length in the file.
    pub payload: (u64, u64),
    /// Every certificate chunk, leaf first.
    pub certificates: Vec<Vec<u8>>,
    pub signature2: Vec<u8>,
    /// The bytes signature2 signs: from the start through the chunk before it.
    pub signed_len: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SigError {
    NoHeader,
    MalformedHeader,
    NoMetadata,
    MalformedMetadata,
    VersionTooNew,
    DeprecatedOldVersion,
    UnsupportedAlgorithm,
    NoCertificate,
    MalformedCertificate,
    NoSignature,
    NoPayload,
    SignatureDoesNotMatchPayload,
    TrustFailed,
    InputFileMalformed,
    IoRead,
}

pub const CORRUPT: &str = "This script archive is corrupt and cannot be installed.";
pub const NOT_VALID: &str = "This is not a valid Tandem script archive.";
pub const CONFIRM_HEADING: &str = "Confirm Installation";
pub const REVEAL_CONTENTS: &str = "Reveal Contents";
pub const CANCELED: &str = "Installation canceled by user request.";
pub const CERT_NIL: &str = "Could not find certificate after verification (nil data)";
pub const CERT_BAD: &str = "Could not find certificate after verification (bad data)";

/// The most an `.its` file may weigh (Decision).
pub const MAX_FILE: u64 = 256 << 20;

pub trait Verifier {
    /// Checks the chain of `certs` (leaf first) and `signature` over `digest` with the leaf's key;
    /// the leaf's common name on success.
    fn verify(&self, certs: &[Vec<u8>], digest: &[u8; 32], signature: &[u8]) -> Result<Option<String>, SigError>;
}

impl SigError {
    /// iTerm2's text for the error (`SE:48-97`) and a final period.
    pub fn text(self) -> String {
        let t = match self {
            SigError::NoCertificate => "No certificate found",
            SigError::NoMetadata => "No metadata found",
            SigError::NoSignature => "No signature found",
            SigError::NoPayload => "No payload found",
            SigError::NoHeader => "No header found",
            SigError::VersionTooNew => "This file is from a newer version of this app and cannot be loaded. Please upgrade and try again",
            SigError::MalformedMetadata => "Metadata chunk malformed",
            SigError::DeprecatedOldVersion => "This file is in an older format that is no longer supported because its authenticity cannot be guaranteed. It cannot be opened by this version of the app",
            SigError::MalformedHeader => "Header chunk malformed",
            SigError::UnsupportedAlgorithm => "Unsupported algorithm",
            SigError::InputFileMalformed => "Input file malformed",
            SigError::MalformedCertificate => "Malformed certificate",
            SigError::TrustFailed => "Trust chain verification failed",
            SigError::SignatureDoesNotMatchPayload => "Signature does not match payload",
            SigError::IoRead => "Error reading source file",
        };
        format!("{t}.")
    }
}

/// Chunks and certificates a file may hold (a crafted file cannot make a long list).
const MAX_CHUNKS: usize = 1024;
const MAX_CERTIFICATES: usize = 16;
const MAX_CERTIFICATE: u64 = 64 << 10;

/// Every chunk (`SAC:14-45`): an 8-byte big-endian tag, an 8-byte big-endian length (both
/// non-negative), then the payload; the whole file must be chunks.
pub fn chunks(bytes: &[u8]) -> Result<Vec<Chunk>, SigError> {
    let total = bytes.len() as u64;
    let mut out = Vec::new();
    let mut pos: u64 = 0;
    while pos < total {
        let header_end = pos.checked_add(16).filter(|e| *e <= total).ok_or(SigError::IoRead)?;
        let word = |at: u64| -> i64 {
            let mut b = [0u8; 8];
            b.copy_from_slice(&bytes[at as usize..at as usize + 8]);
            i64::from_be_bytes(b)
        };
        let (tag, len) = (word(pos), word(pos + 8));
        if tag < 0 || len < 0 {
            return Err(SigError::InputFileMalformed);
        }
        let end = header_end.checked_add(len as u64).filter(|e| *e <= total).ok_or(SigError::IoRead)?;
        if out.len() >= MAX_CHUNKS {
            return Err(SigError::InputFileMalformed);
        }
        out.push(Chunk { tag, offset: header_end, len: len as u64 });
        pos = end;
    }
    Ok(out)
}

fn data<'a>(bytes: &'a [u8], c: &Chunk) -> &'a [u8] {
    &bytes[c.offset as usize..(c.offset + c.len) as usize]
}

/// What `parse` refuses with, and the detail iTerm2 adds to an unsupported algorithm.
fn parse_detail(bytes: &[u8]) -> Result<Parsed, (SigError, Option<String>)> {
    let plain = |e: SigError| (e, None);
    let cs = chunks(bytes).map_err(plain)?;
    let first_with = |tag: Tag| cs.iter().position(|c| c.tag == tag as i64);
    // SAR:33-55: the header chunk is the first chunk and UTF-8 text equal to the magic.
    let h = first_with(Tag::Header).ok_or(plain(SigError::NoHeader))?;
    if h != 0 {
        return Err(plain(SigError::MalformedHeader));
    }
    if std::str::from_utf8(data(bytes, &cs[0])) != Ok(MAGIC) {
        return Err(plain(SigError::MalformedHeader));
    }
    let m = first_with(Tag::Metadata).ok_or(plain(SigError::NoMetadata))?;
    let metadata = std::str::from_utf8(data(bytes, &cs[m])).map_err(|_| plain(SigError::MalformedMetadata))?;
    verify_metadata(metadata)?;
    // SAR:146-167: signature2 is the last chunk, and signs every chunk before it.
    let last = cs.last().filter(|c| c.tag == Tag::Signature2 as i64).ok_or(plain(SigError::NoSignature))?;
    if cs.iter().filter(|c| c.tag == Tag::Signature2 as i64).count() != 1 {
        return Err(plain(SigError::InputFileMalformed));
    }
    let signed_len = match cs.len().checked_sub(2).map(|i| &cs[i]) {
        Some(pen) => pen.offset + pen.len,
        None => return Err(plain(SigError::NoPayload)),
    };
    let mut certificates = Vec::new();
    for c in cs.iter().filter(|c| c.tag == Tag::Certificate as i64) {
        if c.len > MAX_CERTIFICATE || certificates.len() >= MAX_CERTIFICATES {
            return Err(plain(SigError::MalformedCertificate));
        }
        certificates.push(data(bytes, c).to_vec());
    }
    if certificates.is_empty() {
        return Err(plain(SigError::NoCertificate));
    }
    let payloads: Vec<&Chunk> = cs.iter().filter(|c| c.tag == Tag::Payload as i64).collect();
    let payload = match payloads.as_slice() {
        [] => return Err(plain(SigError::NoPayload)),
        [one] => (one.offset, one.len),
        // Two payloads: which one is installed would be the file's choice.
        _ => return Err(plain(SigError::InputFileMalformed)),
    };
    Ok(Parsed { payload, certificates, signature2: data(bytes, last).to_vec(), signed_len })
}

/// `integerValue`: the leading integer of the text, else 0.
fn leading_integer(s: &str) -> i64 {
    let t = s.trim_start();
    let (neg, t) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).take(18).collect();
    let n: i64 = digits.parse().unwrap_or(0);
    if neg { -n } else { n }
}

/// `SAV:170-248`: `key=value` rows; the known keys once each; version 2 and digest SHA2.
fn verify_metadata(metadata: &str) -> Result<(), (SigError, Option<String>)> {
    let (mut version, mut digest_type): (Option<&str>, Option<&str>) = (None, None);
    for row in metadata.split('\n') {
        let Some((key, value)) = row.split_once('=') else { continue };
        let slot = match key {
            "version" => &mut version,
            "digest-type" => &mut digest_type,
            _ => continue,
        };
        if slot.is_some() {
            return Err((SigError::MalformedMetadata, None));
        }
        *slot = Some(value);
    }
    let version = leading_integer(version.ok_or((SigError::MalformedMetadata, None))?);
    if version > 2 {
        return Err((SigError::VersionTooNew, None));
    }
    if version < 2 {
        return Err((SigError::DeprecatedOldVersion, None));
    }
    match digest_type.ok_or((SigError::MalformedMetadata, None))? {
        "SHA2" => Ok(()),
        other => Err((SigError::UnsupportedAlgorithm, Some(format!("Algorithm \"{}\" not supported.", crate::inert_files::display_text(other, 32, false))))),
    }
}

/// `smellsLikeSignedArchive:` (`SAV:60-77`): a first chunk that is the magic header.
pub fn smells_like_signed_archive(bytes: &[u8]) -> bool {
    let Ok(cs) = chunks(bytes) else { return false };
    cs.first().is_some_and(|c| c.tag == Tag::Header as i64 && data(bytes, c) == MAGIC.as_bytes())
}

/// The parts the verifier needs (`SAV:40-330`, `SAR:33-176`), refused with iTerm2's error.
pub fn parse(bytes: &[u8]) -> Result<Parsed, SigError> {
    parse_detail(bytes).map_err(|(e, _)| e)
}

/// The text the importer shows for a file `parse` refuses: iTerm2's error text, with the detail
/// of an unsupported algorithm.
pub fn refusal_text(bytes: &[u8]) -> String {
    match parse_detail(bytes) {
        Err((_, Some(detail))) => detail,
        Err((e, None)) => e.text(),
        Ok(_) => "Unknown error".to_string(),
    }
}

/// SHA-256 of the signed bytes (`SIGSHA2VerificationAlgorithm`).
pub fn digest(bytes: &[u8], signed_len: u64) -> [u8; 32] {
    let end = usize::try_from(signed_len).unwrap_or(usize::MAX).min(bytes.len());
    Sha256::digest(&bytes[..end]).into()
}

/// Parses, then asks `v` about the chain and the signature (`SAV:300-330`); the leaf's common name.
pub fn verify_its(bytes: &[u8], v: &dyn Verifier) -> Result<(Parsed, Option<String>), SigError> {
    let parsed = parse(bytes)?;
    let d = digest(bytes, parsed.signed_len);
    let name = v.verify(&parsed.certificates, &d, &parsed.signature2)?;
    Ok((parsed, name))
}

/// The confirmation (`IM:266-270`): the file's name and the certificate's common name, both
/// text the file chose, so shown cleaned.
pub fn confirm_text(file: &str, author: Option<&str>) -> String {
    let file = crate::inert_files::display_text(file, 128, false);
    let author = author.map(|a| crate::inert_files::display_text(a, 128, false)).filter(|a| !a.trim().is_empty());
    format!("The signature of \u{201c}{file}\u{201d} has been verified. The author is:\n\n{}\n\nWould you like to install it?", author.as_deref().unwrap_or("Unknown"))
}

/// Said under the confirmation (beyond iTerm2): the name is the certificate's own text, not an
/// identity Tandem knows, so the certificate's fingerprint is shown to compare with the author's.
pub fn confirm_note(leaf: &[u8]) -> String {
    format!("Tandem has not checked who this author is: the name comes from the certificate. Certificate fingerprint (SHA-256):\n{}", thumbprint(leaf))
}

/// The certificate's SHA-256, as `AB:CD:..`.
pub fn thumbprint(leaf: &[u8]) -> String {
    Sha256::digest(leaf).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

/// What a verified file gives the importer: the payload zip, the leaf certificate's bytes and
/// the certificate's common name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verified {
    pub payload: Vec<u8>,
    pub leaf: Vec<u8>,
    pub author: Option<String>,
}

/// The importer's verification (`IM:157-270`): a file that does not smell like an archive is
/// corrupt; a refusal answers its text; a verified file gives its payload. Nothing is unpacked.
pub fn verify_file(bytes: &[u8], v: &dyn Verifier) -> Result<Verified, String> {
    if !smells_like_signed_archive(bytes) {
        return Err(CORRUPT.to_string());
    }
    let parsed = parse(bytes).map_err(|_| refusal_text(bytes))?;
    if parsed.payload.1 > crate::extract::MAX_ARCHIVE {
        return Err(CORRUPT.to_string());
    }
    let (parsed, author) = verify_its(bytes, v).map_err(SigError::text)?;
    let leaf = parsed.certificates.into_iter().next().unwrap_or_default();
    if leaf.is_empty() {
        return Err(CERT_NIL.to_string());
    }
    if leaf[0] != 0x30 {
        return Err(CERT_BAD.to_string());
    }
    let (start, len) = (usize::try_from(parsed.payload.0).map_err(|_| CORRUPT)?, usize::try_from(parsed.payload.1).map_err(|_| CORRUPT)?);
    let payload = start.checked_add(len).and_then(|end| bytes.get(start..end)).ok_or(CORRUPT)?.to_vec();
    Ok(Verified { payload, leaf, author })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(tag: i64, payload: &[u8]) -> Vec<u8> {
        let mut v = tag.to_be_bytes().to_vec();
        v.extend((payload.len() as i64).to_be_bytes());
        v.extend(payload);
        v
    }

    const META: &[u8] = b"version=2\ndigest-type=SHA2";

    /// header, metadata, payload "ZIP", certificate "C1", signature2 "S".
    fn good() -> Vec<u8> {
        [chunk(0, MAGIC.as_bytes()), chunk(2, META), chunk(1, b"ZIP"), chunk(4, b"C1"), chunk(5, b"S")].concat()
    }

    fn with_meta(meta: &[u8]) -> Vec<u8> {
        [chunk(0, MAGIC.as_bytes()), chunk(2, meta), chunk(1, b"ZIP"), chunk(4, b"C1"), chunk(5, b"S")].concat()
    }

    struct Fake(Result<Option<String>, SigError>);
    impl Verifier for Fake {
        fn verify(&self, _: &[Vec<u8>], _: &[u8; 32], _: &[u8]) -> Result<Option<String>, SigError> {
            self.0.clone()
        }
    }

    #[test]
    fn archive_parse_follows_iterm2() {
        let bytes = good();
        let p = parse(&bytes).unwrap();
        assert_eq!(&bytes[p.payload.0 as usize..(p.payload.0 + p.payload.1) as usize], b"ZIP");
        assert_eq!(p.certificates, vec![b"C1".to_vec()]);
        assert_eq!(p.signature2, b"S");
        // The signature chunk is 16 bytes of header and "S": the signed bytes end where it starts.
        assert_eq!(p.signed_len as usize, bytes.len() - 17);
        let d = digest(&bytes, p.signed_len);
        assert_eq!(d.to_vec(), Sha256::digest(&bytes[..p.signed_len as usize]).to_vec());
        // Unknown metadata keys are ignored; the rows may be in any order.
        let extra = [chunk(0, MAGIC.as_bytes()), chunk(2, b"x=y\ndigest-type=SHA2\nversion=2\nnote"), chunk(1, b"Z"), chunk(4, b"C1"), chunk(4, b"C2"), chunk(5, b"S")].concat();
        let p = parse(&extra).unwrap();
        assert_eq!(p.certificates.len(), 2);
        // The chunk list is complete and in order.
        let cs = chunks(&bytes).unwrap();
        assert_eq!(cs.iter().map(|c| c.tag).collect::<Vec<_>>(), vec![0, 2, 1, 4, 5]);
        assert!(smells_like_signed_archive(&bytes));
    }

    #[test]
    fn archive_refusals_follow_iterm2() {
        let no_header = [chunk(2, META), chunk(1, b"ZIP"), chunk(4, b"C1"), chunk(5, b"S")].concat();
        assert_eq!(parse(&no_header), Err(SigError::NoHeader));
        assert_eq!(parse(&[]), Err(SigError::NoHeader));
        let zip_header = [chunk(0, b"zip"), chunk(2, META), chunk(1, b"ZIP"), chunk(4, b"C1"), chunk(5, b"S")].concat();
        assert_eq!(parse(&zip_header), Err(SigError::MalformedHeader));
        assert!(!smells_like_signed_archive(&zip_header) && !smells_like_signed_archive(&no_header));
        assert_eq!(refusal_text(&zip_header), SigError::MalformedHeader.text());
        let late_header = [chunk(2, META), chunk(0, MAGIC.as_bytes()), chunk(1, b"ZIP"), chunk(4, b"C1"), chunk(5, b"S")].concat();
        assert_eq!(parse(&late_header), Err(SigError::MalformedHeader));
        assert_eq!(parse(&with_meta(b"version=2\nversion=2\ndigest-type=SHA2")), Err(SigError::MalformedMetadata));
        assert_eq!(parse(&with_meta(b"digest-type=SHA2")), Err(SigError::MalformedMetadata));
        assert_eq!(parse(&with_meta(b"version=2")), Err(SigError::MalformedMetadata));
        assert_eq!(parse(&with_meta(b"version=3\ndigest-type=SHA2")), Err(SigError::VersionTooNew));
        assert_eq!(parse(&with_meta(b"version=1\ndigest-type=SHA2")), Err(SigError::DeprecatedOldVersion));
        assert_eq!(parse(&with_meta(b"version=two\ndigest-type=SHA2")), Err(SigError::DeprecatedOldVersion));
        let md5 = with_meta(b"version=2\ndigest-type=MD5");
        assert_eq!(parse(&md5), Err(SigError::UnsupportedAlgorithm));
        assert_eq!(refusal_text(&md5), "Algorithm \"MD5\" not supported.");
        let no_meta = [chunk(0, MAGIC.as_bytes()), chunk(1, b"ZIP"), chunk(4, b"C1"), chunk(5, b"S")].concat();
        assert_eq!(parse(&no_meta), Err(SigError::NoMetadata));
        let no_cert = [chunk(0, MAGIC.as_bytes()), chunk(2, META), chunk(1, b"ZIP"), chunk(5, b"S")].concat();
        assert_eq!(parse(&no_cert), Err(SigError::NoCertificate));
        let no_sig = [chunk(0, MAGIC.as_bytes()), chunk(2, META), chunk(1, b"ZIP"), chunk(4, b"C1")].concat();
        assert_eq!(parse(&no_sig), Err(SigError::NoSignature));
        // Signature2 must be last, and only one.
        let sig_first = [chunk(0, MAGIC.as_bytes()), chunk(2, META), chunk(5, b"S"), chunk(1, b"ZIP"), chunk(4, b"C1")].concat();
        assert_eq!(parse(&sig_first), Err(SigError::NoSignature));
        let two_sigs = [chunk(0, MAGIC.as_bytes()), chunk(2, META), chunk(1, b"ZIP"), chunk(4, b"C1"), chunk(5, b"S"), chunk(5, b"T")].concat();
        assert_eq!(parse(&two_sigs), Err(SigError::InputFileMalformed));
        let no_payload = [chunk(0, MAGIC.as_bytes()), chunk(2, META), chunk(4, b"C1"), chunk(5, b"S")].concat();
        assert_eq!(parse(&no_payload), Err(SigError::NoPayload));
        let two_payloads = [chunk(0, MAGIC.as_bytes()), chunk(2, META), chunk(1, b"A"), chunk(1, b"B"), chunk(4, b"C1"), chunk(5, b"S")].concat();
        assert_eq!(parse(&two_payloads), Err(SigError::InputFileMalformed));
        // A length past the end, a short header, a negative length or tag, a length near u64::MAX.
        let mut long = good();
        long.truncate(long.len() - 1);
        assert_eq!(chunks(&long), Err(SigError::IoRead));
        assert_eq!(chunks(&[0u8; 9]), Err(SigError::IoRead));
        let mut neg = chunk(0, b"x");
        neg[8] = 0x80;
        assert_eq!(chunks(&neg), Err(SigError::InputFileMalformed));
        let mut neg_tag = chunk(0, b"x");
        neg_tag[0] = 0x80;
        assert_eq!(chunks(&neg_tag), Err(SigError::InputFileMalformed));
        let mut huge = chunk(0, b"x");
        huge[8..16].copy_from_slice(&i64::MAX.to_be_bytes());
        assert_eq!(chunks(&huge), Err(SigError::IoRead));
        assert_eq!(SigError::TrustFailed.text(), "Trust chain verification failed.");
        assert_eq!(SigError::SignatureDoesNotMatchPayload.text(), "Signature does not match payload.");
        // The verifier's answer is the importer's.
        assert_eq!(verify_its(&good(), &Fake(Err(SigError::SignatureDoesNotMatchPayload))).unwrap_err(), SigError::SignatureDoesNotMatchPayload);
        assert_eq!(verify_its(&good(), &Fake(Err(SigError::TrustFailed))).unwrap_err(), SigError::TrustFailed);
        let (parsed, name) = verify_its(&good(), &Fake(Ok(Some("Jane".into())))).unwrap();
        assert_eq!((parsed.payload.1, name.as_deref()), (3, Some("Jane")));
        // A refused file is refused before the verifier is asked.
        assert_eq!(verify_its(&zip_header, &Fake(Ok(None))).unwrap_err(), SigError::MalformedHeader);
    }

    #[test]
    fn archive_confirm_follows_iterm2() {
        assert_eq!(confirm_text("a.its", Some("Jane Dev")), "The signature of \u{201c}a.its\u{201d} has been verified. The author is:\n\nJane Dev\n\nWould you like to install it?");
        assert!(confirm_text("a.its", None).contains("\n\nUnknown\n\n"));
        // The author's name is the file's text: hidden and bidirectional characters never show.
        let sneaky = confirm_text("a\u{202e}.its", Some("Jane\u{202e}\u{200b} Dev"));
        assert!(!sneaky.contains('\u{202e}') && !sneaky.contains('\u{200b}'), "{sneaky:?}");
        assert_eq!(thumbprint(b"abc"), "BA:78:16:BF:8F:01:CF:EA:41:41:40:DE:5D:AE:22:23:B0:03:61:A3:96:17:7A:9C:B4:10:FF:61:F2:00:15:AD");
        assert_eq!(CONFIRM_HEADING, "Confirm Installation");
        assert_eq!((REVEAL_CONTENTS, CANCELED), ("Reveal Contents", "Installation canceled by user request."));
        assert_eq!(CORRUPT, "This script archive is corrupt and cannot be installed.");
        let x = crate::script_archive::trusted_autolaunch_text("x");
        assert_eq!(x, "\u{201c}x\u{201d} would like to launch automatically when Tandem starts. Would you like to allow that?");
        assert_eq!(crate::script_archive::explicit_autolaunch_text("x"), "\u{201c}x\u{201d} can launch automatically when Tandem starts. Would you like to allow that?");
        assert_eq!((crate::script_archive::AUTOLAUNCH_HEADING, crate::script_archive::LAUNCH_AUTOMATICALLY, crate::script_archive::LAUNCH_MANUALLY), ("Allow Auto-Launch?", "Launch Automatically", "Lauch Manually"));
    }

    #[test]
    fn verified_file_gives_its_payload_and_refusals_give_texts() {
        let leaf_archive = [chunk(0, MAGIC.as_bytes()), chunk(2, META), chunk(1, b"ZIP"), chunk(4, &[0x30, 1]), chunk(5, b"S")].concat();
        let v = verify_file(&leaf_archive, &Fake(Ok(Some("Jane".into())))).unwrap();
        assert_eq!((v.payload.as_slice(), v.leaf.as_slice(), v.author.as_deref()), (&b"ZIP"[..], &[0x30u8, 1][..], Some("Jane")));
        assert_eq!(verify_file(b"PK\x03\x04 not an archive at all", &Fake(Ok(None))), Err(CORRUPT.to_string()));
        assert_eq!(verify_file(&good(), &Fake(Ok(None))), Err(CERT_BAD.to_string()));
        assert_eq!(verify_file(&leaf_archive, &Fake(Err(SigError::TrustFailed))), Err("Trust chain verification failed.".to_string()));
        assert_eq!(verify_file(&with_meta(b"version=2\ndigest-type=MD5"), &Fake(Ok(None))), Err("Algorithm \"MD5\" not supported.".to_string()));
    }
}

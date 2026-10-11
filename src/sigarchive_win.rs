//! Checks a signed script archive's certificate chain and RSA signature with CryptoAPI and
//! CNG (`SIGTrust`, `SIGSHA2VerificationAlgorithm`): the chain against Windows' trust store
//! with revocation, then PKCS#1 v1.5 SHA-256 over the digest with the leaf's key. The one
//! verifier of an archive's signature; `sigarchive` states what to verify.

use crate::sigarchive::{SigError, Verifier};
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Security::Cryptography::{
    BCRYPT_PAD_PKCS1, BCRYPT_PKCS1_PADDING_INFO, BCRYPT_SHA256_ALGORITHM, BCryptDestroyKey, BCryptVerifySignature, CERT_CHAIN_CACHE_ONLY_URL_RETRIEVAL, CERT_CHAIN_CONTEXT, CERT_CHAIN_DISABLE_AUTH_ROOT_AUTO_UPDATE, CERT_CHAIN_ENGINE_CONFIG,
    CERT_CHAIN_PARA, CERT_CHAIN_POLICY_BASE, CERT_CHAIN_POLICY_PARA, CERT_CHAIN_POLICY_STATUS, CERT_CHAIN_REVOCATION_CHECK_CACHE_ONLY, CERT_CHAIN_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT, CERT_CONTEXT, CERT_NAME_ATTR_TYPE, CERT_NAME_ISSUER_FLAG,
    CERT_OPEN_STORE_FLAGS, CERT_STORE_ADD_ALWAYS, CERT_STORE_CREATE_NEW_FLAG, CERT_STORE_PROV_MEMORY, CertAddCertificateContextToStore, CertCloseStore, CertCreateCertificateChainEngine, CertCreateCertificateContext,
    CertFreeCertificateChain, CertFreeCertificateChainEngine, CertFreeCertificateContext, CertGetCertificateChain, CertGetNameStringW, CertOpenStore, CertVerifyCertificateChainPolicy, CryptImportPublicKeyInfoEx2, HCERTSTORE,
    X509_ASN_ENCODING, szOID_COMMON_NAME,
};

/// How long a certificate's revocation or issuer lookup may take, so a hostile server cannot hold
/// the import (milliseconds).
const URL_TIMEOUT_MS: u32 = 10_000;

/// Windows' CryptoAPI as the archive's verifier. `offline` builds the chain from the cache only:
/// no issuer or revocation fetch and no automatic root update can reach the network.
pub struct CryptoApi {
    pub offline: bool,
    #[cfg(test)]
    trust_anchor: Option<Vec<u8>>,
}

impl CryptoApi {
    #[cfg(not(test))]
    pub fn new(offline: bool) -> CryptoApi { CryptoApi { offline } }
    #[cfg(test)]
    pub fn new(offline: bool) -> CryptoApi { CryptoApi { offline, trust_anchor: None } }
    /// The app's verifier: Windows' trust store, revocation checked online.
    pub fn online() -> CryptoApi {
        CryptoApi::new(false)
    }
    /// Tests only: the chain is built against this one root alone (empty: no root at all).
    #[cfg(test)]
    pub fn with_anchor(mut self, der: Vec<u8>) -> CryptoApi {
        self.trust_anchor = Some(der);
        self
    }
    #[cfg(test)]
    fn anchor(&self) -> Option<&[u8]> { self.trust_anchor.as_deref() }
    #[cfg(not(test))]
    fn anchor(&self) -> Option<&[u8]> { None }
}

struct Cert(*const CERT_CONTEXT);
impl Cert {
    fn new(der: &[u8]) -> Option<Cert> {
        let len = u32::try_from(der.len()).ok()?;
        // SAFETY: `der` is a valid slice for `len` bytes; the context copies it.
        let c = unsafe { CertCreateCertificateContext(X509_ASN_ENCODING, der.as_ptr(), len) };
        (!c.is_null()).then_some(Cert(c))
    }
}
impl Drop for Cert {
    fn drop(&mut self) {
        // SAFETY: a context this value created and owns.
        unsafe { CertFreeCertificateContext(self.0) };
    }
}

struct Store(HCERTSTORE);
impl Store {
    fn memory() -> Option<Store> {
        // SAFETY: a memory store takes no parameter.
        let s = unsafe { CertOpenStore(CERT_STORE_PROV_MEMORY, 0, 0, CERT_STORE_CREATE_NEW_FLAG as CERT_OPEN_STORE_FLAGS, null()) };
        (!s.is_null()).then_some(Store(s))
    }
    fn add(&self, c: &Cert) -> bool {
        // SAFETY: both handles are live; the store keeps its own copy.
        unsafe { CertAddCertificateContextToStore(self.0, c.0, CERT_STORE_ADD_ALWAYS, null_mut()) != 0 }
    }
}
impl Drop for Store {
    fn drop(&mut self) {
        // SAFETY: a store this value opened and owns.
        unsafe { CertCloseStore(self.0, 0) };
    }
}

/// A chain engine (0 is the current user's default engine, never freed).
struct Engine(isize);
impl Drop for Engine {
    fn drop(&mut self) {
        if self.0 != 0 {
            // SAFETY: an engine this value created.
            unsafe { CertFreeCertificateChainEngine(self.0) };
        }
    }
}

struct Chain(*const CERT_CHAIN_CONTEXT);
impl Drop for Chain {
    fn drop(&mut self) {
        // SAFETY: a chain this value was given by `CertGetCertificateChain`.
        unsafe { CertFreeCertificateChain(self.0) };
    }
}

struct Key(*mut c_void);
impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: a key `CryptImportPublicKeyInfoEx2` made.
        unsafe { BCryptDestroyKey(self.0) };
    }
}

impl CryptoApi {
    /// The chain engine and the flags: the default engine online (its lookups bounded by
    /// `URL_TIMEOUT_MS`), cache-only offline; a test's engine trusts its own root alone.
    fn engine(&self, anchor_store: Option<&Store>, offline: bool, root_update: bool) -> Result<(Engine, u32), SigError> {
        let mut flags = CERT_CHAIN_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT;
        if offline {
            flags |= CERT_CHAIN_REVOCATION_CHECK_CACHE_ONLY | CERT_CHAIN_CACHE_ONLY_URL_RETRIEVAL;
            // The first pass lets Windows consult its own Microsoft root list (never a URL taken
            // from the untrusted certificate); every other offline build stays fully offline.
            if !root_update {
                flags |= CERT_CHAIN_DISABLE_AUTH_ROOT_AUTO_UPDATE;
            }
        }
        // SAFETY: an all-zero configuration is the documented default.
        let mut config: CERT_CHAIN_ENGINE_CONFIG = unsafe { std::mem::zeroed() };
        config.cbSize = std::mem::size_of::<CERT_CHAIN_ENGINE_CONFIG>() as u32;
        config.dwUrlRetrievalTimeout = URL_TIMEOUT_MS;
        if let Some(s) = anchor_store {
            config.hExclusiveRoot = s.0;
        }
        let mut handle: isize = 0;
        // SAFETY: `config` is filled in and outlives the call; `handle` is an out parameter.
        if unsafe { CertCreateCertificateChainEngine(&config, &mut handle) } == 0 {
            return Err(SigError::TrustFailed);
        }
        Ok((Engine(handle), flags))
    }

    /// Whether the chain from `leaf` ends at a trusted root with no error (basic X.509 policy and
    /// revocation, `SP:15-64`).
    fn chain_trusted(&self, leaf: &Cert, extra: &Store) -> Result<(), SigError> {
        // The archive's certificates are not trusted yet and name the hosts a lookup would
        // contact: build the chain from the cache alone first, and go online only for a chain
        // that already ends at a trusted root (only revocation not yet known may be left).
        if !self.offline {
            let status = self.chain_error_status(leaf, extra, true)?;
            if !prepass_allows(status) {
                return Err(SigError::TrustFailed);
            }
        }
        self.chain_checked(leaf, extra, self.offline)
    }

    /// `dwErrorStatus` of the chain built from the cache (`offline`) or with lookups.
    fn chain_error_status(&self, leaf: &Cert, extra: &Store, offline: bool) -> Result<u32, SigError> {
        let anchor = self.anchor_store()?;
        let (engine, flags) = self.engine(anchor.as_ref(), offline, true)?;
        // SAFETY: zeroed parameters ask for no particular usage.
        let mut para: CERT_CHAIN_PARA = unsafe { std::mem::zeroed() };
        para.cbSize = std::mem::size_of::<CERT_CHAIN_PARA>() as u32;
        let mut chain: *mut CERT_CHAIN_CONTEXT = null_mut();
        // SAFETY: every handle is live for the call; `chain` is an out parameter.
        if unsafe { CertGetCertificateChain(engine.0, leaf.0, null(), extra.0, &para, flags, null(), &mut chain) } == 0 {
            return Err(SigError::TrustFailed);
        }
        let chain = Chain(chain);
        // SAFETY: the context is live while `chain` is.
        Ok(unsafe { (*chain.0).TrustStatus.dwErrorStatus })
    }

    fn anchor_store(&self) -> Result<Option<Store>, SigError> {
        match self.anchor() {
            Some(der) => {
                let store = Store::memory().ok_or(SigError::TrustFailed)?;
                if !der.is_empty() {
                    let c = Cert::new(der).ok_or(SigError::TrustFailed)?;
                    if !store.add(&c) {
                        return Err(SigError::TrustFailed);
                    }
                }
                Ok(Some(store))
            }
            None => Ok(None),
        }
    }

    fn chain_checked(&self, leaf: &Cert, extra: &Store, offline: bool) -> Result<(), SigError> {
        let anchor = self.anchor_store()?;
        let (engine, flags) = self.engine(anchor.as_ref(), offline, false)?;
        // SAFETY: zeroed parameters ask for no particular usage.
        let mut para: CERT_CHAIN_PARA = unsafe { std::mem::zeroed() };
        para.cbSize = std::mem::size_of::<CERT_CHAIN_PARA>() as u32;
        let mut chain: *mut CERT_CHAIN_CONTEXT = null_mut();
        // SAFETY: every handle is live for the call; `chain` is an out parameter.
        if unsafe { CertGetCertificateChain(engine.0, leaf.0, null(), extra.0, &para, flags, null(), &mut chain) } == 0 {
            return Err(SigError::TrustFailed);
        }
        let chain = Chain(chain);
        // SAFETY: the context is live while `chain` is.
        if unsafe { (*chain.0).TrustStatus.dwErrorStatus } != 0 {
            return Err(SigError::TrustFailed);
        }
        // SAFETY: zeroed policy parameters and status, sized as the API wants.
        let (mut policy, mut status): (CERT_CHAIN_POLICY_PARA, CERT_CHAIN_POLICY_STATUS) = unsafe { std::mem::zeroed() };
        policy.cbSize = std::mem::size_of::<CERT_CHAIN_POLICY_PARA>() as u32;
        status.cbSize = std::mem::size_of::<CERT_CHAIN_POLICY_STATUS>() as u32;
        // SAFETY: the chain and both structures are live for the call.
        let ok = unsafe { CertVerifyCertificateChainPolicy(CERT_CHAIN_POLICY_BASE, chain.0, &policy, &mut status) };
        if ok == 0 || status.dwError != 0 {
            return Err(SigError::TrustFailed);
        }
        Ok(())
    }
}

/// `SV:22-98`: PKCS#1 v1.5 SHA-256 over `digest` with the leaf's public key.
fn signature_matches(leaf: &Cert, digest: &[u8; 32], signature: &[u8]) -> Result<(), SigError> {
    let mut key: *mut c_void = null_mut();
    // SAFETY: the context and its info are live; `key` is an out parameter.
    let imported = unsafe { CryptImportPublicKeyInfoEx2(X509_ASN_ENCODING, &(*(*leaf.0).pCertInfo).SubjectPublicKeyInfo, 0, null(), &mut key) };
    if imported == 0 {
        return Err(SigError::MalformedCertificate);
    }
    let key = Key(key);
    let pad = BCRYPT_PKCS1_PADDING_INFO { pszAlgId: BCRYPT_SHA256_ALGORITHM };
    let len = u32::try_from(signature.len()).map_err(|_| SigError::SignatureDoesNotMatchPayload)?;
    // SAFETY: the padding info, the digest and the signature are live for the call.
    let status = unsafe { BCryptVerifySignature(key.0, &pad as *const _ as *const c_void, digest.as_ptr(), 32, signature.as_ptr(), len, BCRYPT_PAD_PKCS1) };
    // A mismatch is STATUS_INVALID_SIGNATURE; a key that cannot verify this scheme (not RSA) is no match either.
    if status != 0 {
        return Err(SigError::SignatureDoesNotMatchPayload);
    }
    Ok(())
}

/// `CERT_TRUST_REVOCATION_STATUS_UNKNOWN` and `CERT_TRUST_IS_OFFLINE_REVOCATION`: all that may be
/// left of a chain built from the cache before the online build (anything else, an untrusted
/// root or a partial chain among it, ends the import with no lookup at all).
const PREPASS_OK: u32 = 0x0000_0040 | 0x0100_0000;

fn prepass_allows(error_status: u32) -> bool {
    error_status & !PREPASS_OK == 0
}

/// The name of the certificate's issuer (a common name), at most 255 characters.
fn issuer_name(c: &Cert) -> Option<String> {
    let mut buf = [0u16; 256];
    // SAFETY: the context is live; `buf` holds `buf.len()` characters.
    let n = unsafe { CertGetNameStringW(c.0, CERT_NAME_ATTR_TYPE, CERT_NAME_ISSUER_FLAG, szOID_COMMON_NAME as *const c_void, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    (n > 1).then(|| String::from_utf16_lossy(&buf[..n - 1])).filter(|s| !s.trim().is_empty())
}

/// The certificate's common name (`SC:62-70`), at most 255 characters; None when it has none.
fn common_name(c: &Cert) -> Option<String> {
    let mut buf = [0u16; 256];
    // SAFETY: the context is live; `buf` holds `buf.len()` characters.
    let n = unsafe { CertGetNameStringW(c.0, CERT_NAME_ATTR_TYPE, 0, szOID_COMMON_NAME as *const c_void, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    (n > 1).then(|| String::from_utf16_lossy(&buf[..n - 1])).filter(|s| !s.trim().is_empty())
}

impl Verifier for CryptoApi {
    fn issuer(&self, cert: &[u8]) -> Option<String> {
        issuer_name(&Cert::new(cert)?)
    }

    fn verify(&self, certs: &[Vec<u8>], digest: &[u8; 32], signature: &[u8]) -> Result<Option<String>, SigError> {
        let (first, rest) = certs.split_first().ok_or(SigError::NoCertificate)?;
        let leaf = Cert::new(first).ok_or(SigError::MalformedCertificate)?;
        let extra = Store::memory().ok_or(SigError::TrustFailed)?;
        for der in rest {
            let c = Cert::new(der).ok_or(SigError::MalformedCertificate)?;
            if !extra.add(&c) {
                return Err(SigError::TrustFailed);
            }
        }
        // iTerm2 evaluates trust first, then checks the signature (`SAV:300-330`).
        self.chain_trusted(&leaf, &extra)?;
        signature_matches(&leaf, digest, signature)?;
        Ok(common_name(&leaf))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sigarchive::{self, MAGIC};
    use windows_sys::Win32::Security::Cryptography::{
        BCRYPT_RSA_ALGORITHM, CERT_CREATE_SELFSIGN_NO_KEY_INFO, CERT_X500_NAME_STR, CRYPT_ALGORITHM_IDENTIFIER, CRYPT_INTEGER_BLOB, CertCreateSelfSignCertificate, CertStrToNameW, MS_KEY_STORAGE_PROVIDER, NCRYPT_LENGTH_PROPERTY,
        NCryptCreatePersistedKey, NCryptDeleteKey, NCryptFinalizeKey, NCryptFreeObject, NCryptOpenStorageProvider, NCryptSetProperty, NCryptSignHash, szOID_RSA_SHA256RSA,
    };

    /// A self-signed RSA-2048 certificate over a key made for this test alone (no name: the key
    /// is never stored, and the guard frees it even when the test fails).
    struct TestRoot {
        provider: usize,
        key: usize,
        cert: Cert,
        der: Vec<u8>,
    }

    impl TestRoot {
        fn new(common_name: &str) -> TestRoot {
            let wide: Vec<u16> = format!("CN={common_name}").encode_utf16().chain([0]).collect();
            // SAFETY: plain out parameters and live buffers throughout; each handle is owned by the
            // value built at the end (or freed on an early failure).
            unsafe {
                let (mut provider, mut key) = (0usize, 0usize);
                assert_eq!(NCryptOpenStorageProvider(&mut provider, MS_KEY_STORAGE_PROVIDER, 0), 0);
                assert_eq!(NCryptCreatePersistedKey(provider, &mut key, BCRYPT_RSA_ALGORITHM, null(), 0, 0), 0);
                let bits = 2048u32.to_le_bytes();
                assert_eq!(NCryptSetProperty(key, NCRYPT_LENGTH_PROPERTY, bits.as_ptr(), 4, 0), 0);
                assert_eq!(NCryptFinalizeKey(key, 0), 0);
                let mut size = 0u32;
                assert_ne!(CertStrToNameW(X509_ASN_ENCODING, wide.as_ptr(), CERT_X500_NAME_STR, null(), null_mut(), &mut size, null_mut()), 0);
                let mut name = vec![0u8; size as usize];
                assert_ne!(CertStrToNameW(X509_ASN_ENCODING, wide.as_ptr(), CERT_X500_NAME_STR, null(), name.as_mut_ptr(), &mut size, null_mut()), 0);
                let subject = CRYPT_INTEGER_BLOB { cbData: size, pbData: name.as_mut_ptr() };
                let algorithm = CRYPT_ALGORITHM_IDENTIFIER { pszObjId: szOID_RSA_SHA256RSA as *mut u8, Parameters: CRYPT_INTEGER_BLOB { cbData: 0, pbData: null_mut() } };
                let ctx = CertCreateSelfSignCertificate(key, &subject, CERT_CREATE_SELFSIGN_NO_KEY_INFO, null(), &algorithm, null(), null(), null());
                if ctx.is_null() {
                    NCryptDeleteKey(key, 0);
                    NCryptFreeObject(provider);
                    panic!("CertCreateSelfSignCertificate failed");
                }
                let der = std::slice::from_raw_parts((*ctx).pbCertEncoded, (*ctx).cbCertEncoded as usize).to_vec();
                TestRoot { provider, key, cert: Cert(ctx), der }
            }
        }

        fn sign(&self, digest: &[u8; 32]) -> Vec<u8> {
            let pad = BCRYPT_PKCS1_PADDING_INFO { pszAlgId: BCRYPT_SHA256_ALGORITHM };
            let mut len = 0u32;
            // SAFETY: the key is live; the first call asks the size, the second fills a buffer of it.
            unsafe {
                let p = &pad as *const _ as *const c_void;
                assert_eq!(NCryptSignHash(self.key, p, digest.as_ptr(), 32, null_mut(), 0, &mut len, BCRYPT_PAD_PKCS1), 0);
                let mut sig = vec![0u8; len as usize];
                assert_eq!(NCryptSignHash(self.key, p, digest.as_ptr(), 32, sig.as_mut_ptr(), len, &mut len, BCRYPT_PAD_PKCS1), 0);
                sig.truncate(len as usize);
                sig
            }
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            // SAFETY: handles this value owns; `cert` frees itself.
            unsafe {
                if NCryptDeleteKey(self.key, 0) != 0 {
                    NCryptFreeObject(self.key);
                }
                NCryptFreeObject(self.provider);
            }
            let _ = &self.cert;
        }
    }

    fn chunk(tag: i64, payload: &[u8]) -> Vec<u8> {
        let mut v = tag.to_be_bytes().to_vec();
        v.extend((payload.len() as i64).to_be_bytes());
        v.extend(payload);
        v
    }

    fn signed_archive(root: &TestRoot, payload: &[u8]) -> Vec<u8> {
        let mut body = [chunk(0, MAGIC.as_bytes()), chunk(2, b"version=2\ndigest-type=SHA2"), chunk(1, payload), chunk(4, &root.der)].concat();
        let sig = root.sign(&sigarchive::digest(&body, body.len() as u64));
        body.extend(chunk(5, &sig));
        body
    }

    /// R13xy-14: no case touches the machine's root store or the network.
    #[test]
    fn chain_and_signature_verify() {
        let root = TestRoot::new("Test Author");
        let archive = signed_archive(&root, b"PK-payload");
        // No root trusted: the chain fails.
        let none = CryptoApi::new(true).with_anchor(Vec::new());
        assert_eq!(sigarchive::verify_its(&archive, &none).unwrap_err(), SigError::TrustFailed);
        // The test root trusted: verified, with the leaf's common name.
        let trusting = CryptoApi::new(true).with_anchor(root.der.clone());
        let (parsed, name) = sigarchive::verify_its(&archive, &trusting).unwrap();
        assert_eq!(name.as_deref(), Some("Test Author"));
        assert_eq!(&archive[parsed.payload.0 as usize..(parsed.payload.0 + parsed.payload.1) as usize], b"PK-payload");
        // One payload byte flipped: the chain still holds, the signature does not.
        let mut bad = archive.clone();
        bad[parsed.payload.0 as usize] ^= 1;
        assert_eq!(sigarchive::verify_its(&bad, &trusting).unwrap_err(), SigError::SignatureDoesNotMatchPayload);
        // A garbled certificate is refused as one.
        assert_eq!(trusting.verify(&[vec![1, 2, 3]], &[0; 32], &[]), Err(SigError::MalformedCertificate));
        assert_eq!(trusting.verify(&[], &[0; 32], &[]), Err(SigError::NoCertificate));
        // The issuer shown for a self-signed certificate is its own name.
        assert_eq!(trusting.issuer(&root.der).as_deref(), Some("Test Author"));
    }

    /// The online verifier builds from the cache first: an untrusted chain ends there with no
    /// lookup, a trusted one goes on to the online build (both orders), and only revocation not
    /// yet known may be left after the first build.
    #[test]
    fn online_build_follows_a_cache_only_build() {
        let root = TestRoot::new("Test Author");
        let archive = signed_archive(&root, b"PK-payload");
        let untrusted = CryptoApi::new(false).with_anchor(Vec::new());
        assert_eq!(sigarchive::verify_its(&archive, &untrusted).unwrap_err(), SigError::TrustFailed);
        let trusted = CryptoApi::new(false).with_anchor(root.der.clone());
        assert!(sigarchive::verify_its(&archive, &trusted).is_ok());
        assert!(prepass_allows(0) && prepass_allows(0x40) && prepass_allows(0x0100_0000 | 0x40));
        assert!(!prepass_allows(0x20), "untrusted root");
        assert!(!prepass_allows(0x1_0000), "partial chain");
        assert!(!prepass_allows(0x4), "expired");
    }

    /// The test-only trust anchor never reaches a release build: every line that names it is the
    /// one cfg(test) item.
    #[test]
    fn trust_anchor_is_test_only() {
        let src = include_str!("sigarchive_win.rs");
        let production = src.split("#[cfg(test)]\nmod tests").next().unwrap();
        let lines: Vec<&str> = production.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            if !code.contains("trust_anchor") {
                continue;
            }
            // Each such line is the field, or sits in a function carried by cfg(test) on the line above.
            let attr = lines[..i].iter().rev().find(|l| !l.trim().is_empty() && !l.trim_start().starts_with("///")).map(|l| l.trim());
            let in_new = code.contains("CryptoApi { offline, trust_anchor: None }") || code.contains("self.trust_anchor = Some");
            assert!(attr == Some("#[cfg(test)]") || in_new, "line {}: {line}", i + 1);
        }
    }
}

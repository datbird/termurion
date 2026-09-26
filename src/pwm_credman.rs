//! Windows Credential Manager as a password manager source (iTerm2's `KeychainPasswordDataSource`
//! in Windows words). Accounts are the current user's generic credentials whose target starts
//! with `Tandem/` (`pwm_source::target_name` and `parse_target` hold the naming); the password is
//! the credential's blob as UTF-8. A blob is copied straight into a `Secret` and the copy Windows
//! handed back is zeroed before `CredFree`. No test calls this module; the milestone's checklist
//! covers it by hand.

use crate::pwm::{Account, Secret};
use crate::pwm_source::{self, Caps, Kind, Source};
use windows_sys::w;
use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND};
use windows_sys::Win32::Security::Credentials::{
    CredDeleteW, CredEnumerateW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_MAX_CREDENTIAL_BLOB_SIZE,
    CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
};

/// The comment written on every credential Tandem stores (shown in Control Panel).
const COMMENT: &str = "Tandem password manager";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn last_error() -> String {
    std::io::Error::last_os_error().to_string()
}

/// A NUL-terminated wide string Windows owns, copied out (target and user names, never secrets).
///
/// # Safety
/// `p` is null or points at a NUL-terminated UTF-16 string.
unsafe fn from_wide(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: the caller's contract; the scan stops at the terminating NUL.
    unsafe {
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
    }
}

/// Overwrites a credential's blob with zeros before it is freed.
///
/// # Safety
/// `c` points at a `CREDENTIALW` Windows returned, whose blob is writable for its size.
unsafe fn zero_blob(c: *mut CREDENTIALW) {
    // SAFETY: the caller's contract; writes stay within the blob's size.
    unsafe {
        let blob = (*c).CredentialBlob;
        if !blob.is_null() {
            for i in 0..(*c).CredentialBlobSize as usize {
                std::ptr::write_volatile(blob.add(i), 0);
            }
        }
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

/// The account's target, refused unless it is one of Tandem's (so nothing here ever reads,
/// writes or deletes another program's credential).
fn target(a: &Account) -> Result<&str, String> {
    match pwm_source::parse_target(&a.id) {
        Some(_) => Ok(&a.id),
        None => Err("Not a Tandem credential.".to_string()),
    }
}

/// Writes (creates or replaces) a generic credential for the current user on this machine.
fn write(target: &str, user: &str, p: &Secret) -> Result<(), String> {
    let bytes = p.expose().as_bytes();
    if bytes.len() > CRED_MAX_CREDENTIAL_BLOB_SIZE as usize {
        return Err(format!(
            "The password is too long for Windows Credential Manager (at most {CRED_MAX_CREDENTIAL_BLOB_SIZE} bytes)."
        ));
    }
    let mut target_w = wide(target);
    let mut comment_w = wide(COMMENT);
    let mut user_w = wide(user);
    // SAFETY: CREDENTIALW is plain data; all-zero is a valid empty credential (null pointers,
    // no attributes), and every field Windows reads is set below.
    let mut c: CREDENTIALW = unsafe { std::mem::zeroed() };
    c.Type = CRED_TYPE_GENERIC;
    c.TargetName = target_w.as_mut_ptr();
    c.Comment = comment_w.as_mut_ptr();
    c.CredentialBlobSize = bytes.len() as u32;
    // CredWriteW only reads the blob; the pointer is mutable in the struct's type alone.
    c.CredentialBlob = bytes.as_ptr() as *mut u8;
    c.Persist = CRED_PERSIST_LOCAL_MACHINE;
    if !user.is_empty() {
        c.UserName = user_w.as_mut_ptr();
    }
    // SAFETY: every pointer in `c` outlives the call (the wide buffers and the secret's bytes).
    if unsafe { CredWriteW(&c, 0) } == 0 {
        return Err(last_error());
    }
    Ok(())
}

/// Windows Credential Manager, always available, one account, no one-time codes.
#[derive(Default)]
pub struct CredentialManager;

impl CredentialManager {
    pub fn new() -> CredentialManager {
        CredentialManager
    }
}

impl Source for CredentialManager {
    fn kind(&self) -> Kind {
        Kind::CredentialManager
    }

    fn name(&self) -> String {
        "Credential Manager".to_string()
    }

    fn caps(&self) -> Caps {
        pwm_source::credman_caps()
    }

    fn available(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn accounts(&mut self) -> Result<Vec<Account>, String> {
        let mut count = 0u32;
        let mut list: *mut *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: out-pointers are valid; on success Windows owns `list` until CredFree.
        if unsafe { CredEnumerateW(w!("Tandem/*"), 0, &mut count, &mut list) } == 0 {
            // SAFETY: plain thread-local read right after the failing call.
            if unsafe { GetLastError() } == ERROR_NOT_FOUND {
                return Ok(Vec::new());
            }
            return Err(last_error());
        }
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count as usize {
            // SAFETY: `list` holds `count` valid credential pointers until CredFree below.
            unsafe {
                let c = *list.add(i);
                // The filter is case-insensitive; parse_target keeps only Tandem's exact prefix.
                if let Some(a) = pwm_source::parse_target(&from_wide((*c).TargetName)) {
                    out.push(a);
                }
                // Enumeration returns every blob too; none is read, all are zeroed.
                zero_blob(c);
            }
        }
        // SAFETY: `list` came from CredEnumerateW and is freed once.
        unsafe { CredFree(list as *const _) };
        Ok(out)
    }

    fn password(&mut self, a: &Account) -> Result<(Secret, Option<Secret>), String> {
        let target_w = wide(target(a)?);
        let mut c: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: valid target and out-pointer; on success Windows owns `c` until CredFree.
        if unsafe { CredReadW(target_w.as_ptr(), CRED_TYPE_GENERIC, 0, &mut c) } == 0 {
            return Err(last_error());
        }
        // SAFETY: `c` is a valid credential until CredFree; the blob is `CredentialBlobSize`
        // bytes. The copy goes into a buffer the Secret then owns (from_utf8 does not copy), and
        // a buffer that is not UTF-8 is wrapped in a Secret too so it is zeroed on drop.
        let decoded = unsafe {
            let size = (*c).CredentialBlobSize as usize;
            let mut bytes = Vec::with_capacity(size);
            if size > 0 {
                bytes.extend_from_slice(std::slice::from_raw_parts((*c).CredentialBlob, size));
            }
            zero_blob(c);
            CredFree(c as *const _);
            String::from_utf8(bytes)
        };
        match decoded {
            Ok(s) => Ok((Secret::new(s), None)),
            Err(e) => {
                let mut raw = e.into_bytes();
                for b in raw.iter_mut() {
                    // SAFETY: a plain byte write into our own buffer.
                    unsafe { std::ptr::write_volatile(b, 0) };
                }
                Err("The stored password is not UTF-8 text.".to_string())
            }
        }
    }

    fn set_password(&mut self, a: &Account, p: Secret) -> Result<(), String> {
        write(target(a)?, &a.user_name, &p)
    }

    fn delete(&mut self, a: &Account) -> Result<(), String> {
        let target_w = wide(target(a)?);
        // SAFETY: a valid NUL-terminated target.
        if unsafe { CredDeleteW(target_w.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 {
            return Err(last_error());
        }
        Ok(())
    }

    fn add(&mut self, name: &str, user: &str, p: Secret, _flags: &[(String, bool)]) -> Result<Account, String> {
        // Credential Manager has no Add Account toggles, so flags is ignored (as iTerm2's keychain).
        let t = pwm_source::target_name(name, user);
        write(&t, user, &p)?;
        pwm_source::parse_target(&t).ok_or_else(|| "Not a Tandem credential.".to_string())
    }

    fn edit_in_place(&mut self, _a: &Account, _name: Option<&str>, _user: Option<&str>, _p: Option<Secret>) -> Result<(), String> {
        // Never called: caps().in_place is false, so the window deletes and re-adds.
        Err("In-place edit is not supported.".to_string())
    }

    fn toggle_otp(&mut self, _a: &Account) -> Result<Account, String> {
        // Never called: no account has a one-time code (iTerm2's keychain traps here).
        Err("One-time codes are not supported.".to_string())
    }

    fn reload(&mut self) {}

    fn reset_configuration(&mut self) {}

    fn switch_account(&mut self) -> Result<(), String> {
        Ok(())
    }
}

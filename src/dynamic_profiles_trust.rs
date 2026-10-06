//! Whether a Dynamic Profiles folder or file may be trusted with what iTerm2 lets a dynamic
//! profile do (Command, Initial Text, Triggers, Working Directory): the owner is the person and
//! no one else can write it (13v2's ruling). Another user who can write there could plant a
//! profile that runs a command when the person opens it.

/// An access control entry, reduced to what the judgment needs.
#[derive(Clone, Debug)]
pub struct Ace {
    pub sid: String,
    pub mask: u32,
    /// An allow entry (a deny entry grants nothing).
    pub allow: bool,
    /// Applies to what is inside a folder, not to the folder itself.
    pub inherit_only: bool,
}

/// The rights that change a file or a folder: write data or add file, append data or add
/// subfolder, write extended attributes, delete child, delete, write the access list, take
/// ownership, generic write and generic all.
const WRITE_RIGHTS: u32 = 0x2 | 0x4 | 0x10 | 0x40 | 0x1_0000 | 0x4_0000 | 0x8_0000 | 0x4000_0000 | 0x1000_0000;

/// Accounts that may always write: the creator-owner placeholder, the system, the
/// Administrators group and the installer service.
fn always_trusted(sid: &str) -> bool {
    matches!(sid, "S-1-3-0" | "S-1-5-18" | "S-1-5-32-544") || sid.starts_with("S-1-5-80-")
}

/// Ok when `owner` is `me` (or the Administrators group) and the access list (None: no list, which lets everyone write) grants
/// no write right to anyone but `me` and the always trusted accounts.
pub fn judge(owner: &str, me: &str, dacl: Option<&[Ace]>) -> Result<(), String> {
    // An elevated process makes files owned by the Administrators group; its members can change
    // anything of the person's already.
    if owner != me && owner != "S-1-5-32-544" {
        return Err("owned by another user".into());
    }
    let Some(aces) = dacl else { return Err("writable by other users".into()) };
    let others = aces.iter().find(|a| a.allow && !a.inherit_only && a.mask & WRITE_RIGHTS != 0 && a.sid != me && !always_trusted(&a.sid));
    if let Some(a) = others {
        return Err(format!("writable by other users ({} may write)", a.sid));
    }
    Ok(())
}

/// `judge` for a file or folder on disk: its owner and access list as Windows reports them.
/// Unreadable means not trusted. The object is opened without following a link.
#[cfg(windows)]
pub fn check(path: &std::path::Path) -> Result<(), String> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{HANDLE, HLOCAL, LocalFree};
    use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{ACL, DACL_SECURITY_INFORMATION, GetAce, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID};
    use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, READ_CONTROL};

    let file = std::fs::OpenOptions::new()
        .access_mode(READ_CONTROL)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|e| format!("its owner could not be read ({e})"))?;
    let me = crate::winpipe::user_sid().map_err(|e| format!("the current user is unknown ({e})"))?;
    let handle = file.as_raw_handle() as HANDLE;
    unsafe {
        let (mut owner, mut dacl): (PSID, *mut ACL) = (null_mut(), null_mut());
        let mut sd: PSECURITY_DESCRIPTOR = null_mut();
        let err = GetSecurityInfo(handle, SE_FILE_OBJECT, OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION, &mut owner, null_mut(), &mut dacl, null_mut(), &mut sd);
        if err != 0 {
            return Err(format!("its owner could not be read ({})", std::io::Error::from_raw_os_error(err as i32)));
        }
        let result = (|| {
            let owner_text = if owner.is_null() { return Err("it has no owner".to_string()) } else { crate::winpipe::sid_text(owner)? };
            let aces = if dacl.is_null() {
                None
            } else {
                let mut aces = Vec::new();
                for i in 0..(*dacl).AceCount as u32 {
                    let mut ace: *mut core::ffi::c_void = null_mut();
                    if GetAce(dacl, i, &mut ace) == 0 || ace.is_null() {
                        return Err("its access list could not be read".to_string());
                    }
                    // ACE_HEADER { type, flags, size } then, for the allow and deny forms, the
                    // mask and the SID.
                    let bytes = ace as *const u8;
                    let (kind, flags) = (*bytes, *bytes.add(1));
                    let allow = match kind {
                        0 | 9 => true,
                        1 | 10 => false,
                        _ => continue,
                    };
                    let mask = (bytes.add(4) as *const u32).read_unaligned();
                    let sid = crate::winpipe::sid_text(bytes.add(8) as PSID)?;
                    aces.push(Ace { sid, mask, allow, inherit_only: flags & 0x08 != 0 });
                }
                Some(aces)
            };
            judge(&owner_text, &me, aces.as_deref())
        })();
        LocalFree(sd as HLOCAL);
        result
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn ace(sid: &str, mask: u32) -> Ace {
        Ace { sid: sid.into(), mask, allow: true, inherit_only: false }
    }

    const ME: &str = "S-1-5-21-1-2-3-1001";
    const FULL: u32 = 0x001F_01FF;
    const READ: u32 = 0x0012_0089;

    #[test]
    fn dynamic_trust_judges_owner_and_writers() {
        let ordinary = [ace(ME, FULL), ace("S-1-5-18", FULL), ace("S-1-5-32-544", FULL), ace("S-1-5-32-545", READ)];
        assert_eq!(judge(ME, ME, Some(&ordinary)), Ok(()));
        assert_eq!(judge("S-1-5-32-544", ME, Some(&ordinary)), Ok(()), "made by an elevated process");
        let err = judge("S-1-5-21-9-9-9-500", ME, Some(&ordinary)).unwrap_err();
        assert!(err.contains("owned by another user"), "{err}");
        let everyone = [ace(ME, FULL), ace("S-1-1-0", 0x4)];
        assert!(judge(ME, ME, Some(&everyone)).unwrap_err().contains("writable by other users"));
        for mask in [0x2, 0x4, 0x10, 0x1_0000, 0x4_0000, 0x8_0000, 0x4000_0000, 0x1000_0000] {
            let users = [ace(ME, FULL), ace("S-1-5-32-545", mask)];
            assert!(judge(ME, ME, Some(&users)).is_err(), "mask {mask:#x}");
        }
        // Reading and executing is no write; neither is a deny or an inherit-only entry.
        let read_only = [ace(ME, FULL), ace("S-1-1-0", READ)];
        assert_eq!(judge(ME, ME, Some(&read_only)), Ok(()));
        let deny = [ace(ME, FULL), Ace { sid: "S-1-1-0".into(), mask: FULL, allow: false, inherit_only: false }];
        assert_eq!(judge(ME, ME, Some(&deny)), Ok(()));
        let inherit = [ace(ME, FULL), Ace { sid: "S-1-1-0".into(), mask: FULL, allow: true, inherit_only: true }];
        assert_eq!(judge(ME, ME, Some(&inherit)), Ok(()));
        // No DACL means everyone may write.
        assert!(judge(ME, ME, None).unwrap_err().contains("writable by other users"));
        // The creator-owner placeholder is the owner.
        assert_eq!(judge(ME, ME, Some(&[ace(ME, FULL), ace("S-1-3-0", FULL)])), Ok(()));
    }

    #[cfg(windows)]
    #[test]
    fn a_folder_the_person_just_made_is_trusted() {
        // Under %APPDATA%, whose access list is the person's own (a temp folder may carry extra
        // entries a sandbox adds).
        let base = std::path::PathBuf::from(std::env::var_os("APPDATA").expect("APPDATA"));
        let dir = base.join(format!("tandem-dp-trust-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.json");
        std::fs::write(&file, b"{}").unwrap();
        assert_eq!(check(&dir), Ok(()), "the person's own folder");
        assert_eq!(check(&file), Ok(()));
        assert!(check(&dir.join("missing.json")).is_err(), "unreadable means not trusted");
        // C:\Users\Public is writable by everyone.
        assert!(check(std::path::Path::new("C:\\Users\\Public")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

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

/// The Windows Modules Installer service (TrustedInstaller), the one service account that may
/// write: any other service SID is an ordinary account.
const TRUSTED_INSTALLER: &str = "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";

/// Accounts that may always write: the creator-owner placeholder, the system, the
/// Administrators group and TrustedInstaller.
fn always_trusted(sid: &str) -> bool {
    matches!(sid, "S-1-3-0" | "S-1-5-18" | "S-1-5-32-544") || sid == TRUSTED_INSTALLER
}

/// What one open handle says about the object it opened: the one source of every fact judged
/// (never a second look at the path, which could name something else by then).
#[derive(Clone, Debug)]
pub struct Facts {
    /// A symbolic link or junction (the handle was opened without following it).
    pub reparse: bool,
    pub dir: bool,
    /// How many names the file has.
    pub links: u32,
    pub owner: String,
    pub dacl: Option<Vec<Ace>>,
    /// The ZoneId in the file's Zone.Identifier stream (the Mark of the Web), when it has one.
    pub zone: Option<u32>,
}

/// What opening one object once found: whether it is a folder, the verdict, and the bytes read
/// from the same handle (only for a trusted file; an error says why not).
#[derive(Clone, Debug)]
pub struct Opened {
    pub dir: bool,
    pub trust: Result<(), String>,
    pub bytes: Result<Vec<u8>, String>,
    /// What the same handle says of the file: its last write time, its identity (volume serial and
    /// file index) and how many names it has. A write-back compares them with what the load saw.
    pub stat: Stat,
}

/// A file's last write time, identity and name count, all from one handle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stat {
    pub mtime: Option<std::time::SystemTime>,
    pub id: Option<(u32, u64)>,
    pub links: u32,
}

/// The verdict on `facts` for the user `me`: no link of any kind (a folder or file that is one is
/// judged by the target it names, which is no part of what was asked), no second name for a file
/// another user owns, the owner and writers rule of `judge`, and no Mark of the Web (zone 3 or 4:
/// a file saved from a browser is read without a question, so it must be unblocked first).
pub fn judge_facts(f: &Facts, me: &str) -> Result<(), String> {
    if f.reparse {
        return Err("it is a link (a symbolic link or junction), which Termurion does not follow".into());
    }
    if !f.dir && f.links > 1 && f.owner != me && f.owner != "S-1-5-32-544" {
        return Err("it has more than one name and is not yours".into());
    }
    judge(&f.owner, me, f.dacl.as_deref())?;
    if matches!(f.zone, Some(3 | 4)) {
        return Err("it came from the internet or another computer: unblock the file first (Properties, Unblock)".into());
    }
    Ok(())
}

/// Ok when `owner` is `me` (or the Administrators group or TrustedInstaller) and the access list (None: no list, which lets everyone write) grants
/// no write right to anyone but `me` and the always trusted accounts.
pub fn judge(owner: &str, me: &str, dacl: Option<&[Ace]>) -> Result<(), String> {
    // An elevated process makes files owned by the Administrators group; its members can change
    // anything of the person's already. TrustedInstaller owns the system's protected folders (a
    // WebView2 runtime among them) and may always write, so owning one adds nothing.
    if owner != me && owner != "S-1-5-32-544" && owner != TRUSTED_INSTALLER {
        return Err("owned by another user".into());
    }
    let Some(aces) = dacl else { return Err("writable by other users".into()) };
    let others = aces.iter().find(|a| a.allow && !a.inherit_only && a.mask & WRITE_RIGHTS != 0 && a.sid != me && !always_trusted(&a.sid));
    if let Some(a) = others {
        return Err(format!("writable by other users ({} may write)", a.sid));
    }
    Ok(())
}

/// Opens `path` ONCE, without following a link, and answers from that handle alone: the
/// type, the link count, the owner and access list (`GetSecurityInfo`), and, for a file that
/// passes `judge_facts`, its bytes (at most `limit`). A file is opened with a read-only share
/// mode, so it cannot be swapped between the judgment and the read. Unreadable is an error.
/// The path is a judged one (13v1's one path judge); the handle is `localfs::open_unfollowed`.
#[cfg(windows)]
pub fn open(path: &crate::localfs::LocalPath, limit: usize) -> Result<Opened, String> {
    use std::io::Read;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{HANDLE, HLOCAL, LocalFree};
    use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{ACL, DACL_SECURITY_INFORMATION, GetAce, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID};
    use windows_sys::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle, READ_CONTROL};

    // A folder is opened with every share right (an editor keeps working in it); a file with
    // read only, so nobody writes it under the read.
    let is_dir = crate::localfs::lstat(path).map(|s| s.is_dir).unwrap_or(false);
    let share = if is_dir { FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE } else { FILE_SHARE_READ };
    // The one no-follow handle (FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS).
    let mut file = crate::localfs::open_unfollowed(&crate::localfs::Unfollowed::from(path.clone()), |o| {
        o.access_mode(READ_CONTROL | FILE_READ_ATTRIBUTES | FILE_READ_DATA).share_mode(share);
    })
    .map_err(|e| e.to_string())?;
    let me = crate::winpipe::user_sid().map_err(|e| format!("the current user is unknown ({e})"))?;
    let handle = file.as_raw_handle() as HANDLE;
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: the handle is open for the call and `info` is a plain out structure.
    if unsafe { GetFileInformationByHandle(handle, &mut info) } == 0 {
        return Err(format!("it could not be examined ({})", std::io::Error::last_os_error()));
    }
    let (reparse, dir) = (info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0, info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0);
    let stat = Stat { mtime: filetime_to_system(info.ftLastWriteTime.dwHighDateTime, info.ftLastWriteTime.dwLowDateTime), id: Some((info.dwVolumeSerialNumber, (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow))), links: info.nNumberOfLinks };
    let facts = unsafe {
        let (mut owner, mut dacl): (PSID, *mut ACL) = (null_mut(), null_mut());
        let mut sd: PSECURITY_DESCRIPTOR = null_mut();
        let err = GetSecurityInfo(handle, SE_FILE_OBJECT, OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION, &mut owner, null_mut(), &mut dacl, null_mut(), &mut sd);
        if err != 0 {
            return Ok(Opened { dir, trust: Err(format!("its owner could not be read ({})", std::io::Error::from_raw_os_error(err as i32))), bytes: Err("not read".into()), stat });
        }
        let read_acl = || -> Result<(String, Option<Vec<Ace>>), String> {
            let owner_text = if owner.is_null() { return Err("it has no owner".to_string()) } else { crate::winpipe::sid_text(owner)? };
            if dacl.is_null() {
                return Ok((owner_text, None));
            }
            let mut aces = Vec::new();
            for i in 0..(*dacl).AceCount as u32 {
                let mut ace: *mut core::ffi::c_void = null_mut();
                if GetAce(dacl, i, &mut ace) == 0 || ace.is_null() {
                    return Err("its access list could not be read".to_string());
                }
                // ACE_HEADER { type, flags, size } then, for the allow and deny forms, the mask
                // and the SID.
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
            Ok((owner_text, Some(aces)))
        };
        let got = read_acl();
        LocalFree(sd as HLOCAL);
        got
    };
    let (owner, dacl) = match facts {
        Ok(f) => f,
        Err(why) => return Ok(Opened { dir, trust: Err(why), bytes: Err("not read".into()), stat }),
    };
    let zone = if dir || reparse { None } else { zone_id(path.path()) };
    let trust = judge_facts(&Facts { reparse, dir, links: info.nNumberOfLinks, owner, dacl, zone }, &me);
    if dir || reparse || trust.is_err() {
        return Ok(Opened { dir, trust, bytes: Err("not read".into()), stat });
    }
    let size = (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow);
    let bytes = if size > limit as u64 {
        Err(format!("file too large (the limit is {limit} bytes)"))
    } else {
        let mut out = Vec::new();
        match (&mut file).take(limit as u64 + 1).read_to_end(&mut out) {
            Ok(_) if out.len() > limit => Err(format!("file too large (the limit is {limit} bytes)")),
            Ok(_) => Ok(out),
            Err(e) => Err(e.to_string()),
        }
    };
    Ok(Opened { dir, trust, bytes, stat })
}

#[cfg(not(windows))]
pub fn open(_path: &crate::localfs::LocalPath, _limit: usize) -> Result<Opened, String> {
    Err("Dynamic Profiles are read on Windows only".into())
}

/// A FILETIME (100 ns ticks since 1601) as a `SystemTime`, as `Metadata::modified` makes it.
#[cfg(windows)]
fn filetime_to_system(high: u32, low: u32) -> Option<std::time::SystemTime> {
    const UNIX_EPOCH_TICKS: u64 = 116_444_736_000_000_000;
    let ticks = (u64::from(high) << 32) | u64::from(low);
    ticks.checked_sub(UNIX_EPOCH_TICKS).map(|t| std::time::UNIX_EPOCH + std::time::Duration::from_nanos(t.saturating_mul(100)))
}

/// The ZoneId in a file's Zone.Identifier stream (`inert_files::zone_of` reads the text).
#[cfg(windows)]
fn zone_id(path: &std::path::Path) -> Option<u32> {
    use std::io::Read;
    let mut stream = path.as_os_str().to_os_string();
    stream.push(":Zone.Identifier");
    let mut text = String::new();
    // The stream's name goes through the one path judge as `inert_files` judges it.
    crate::localfs::open(&crate::localfs::judge(std::path::PathBuf::from(stream))?).ok()?.take(4096).read_to_string(&mut text).ok()?;
    crate::inert_files::zone_of(&text)
}
#[cfg(test)]
mod tests {
    use super::*;

    /// The tests open by path: the path is judged first, as `dynamic_profiles::system_open` does.
    #[cfg(windows)]
    fn open(p: &std::path::Path, limit: usize) -> Result<Opened, String> {
        super::open(&crate::localfs::LocalPath::judge(p).expect("a local path"), limit)
    }

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

    fn facts(owner: &str) -> Facts {
        Facts { reparse: false, dir: false, links: 1, owner: owner.into(), dacl: Some(vec![ace(owner, FULL)]), zone: None }
    }

    /// What one open handle says is what is judged: a link of any kind, a second name for a file
    /// that is not the person's, and a Mark of the Web (zone 3 or 4) each refuse.
    #[test]
    fn dynamic_trust_judges_the_handle() {
        assert_eq!(judge_facts(&facts(ME), ME), Ok(()));
        let link = Facts { reparse: true, ..facts(ME) };
        assert!(judge_facts(&link, ME).unwrap_err().contains("link"));
        let junction = Facts { reparse: true, dir: true, ..facts(ME) };
        assert!(judge_facts(&junction, ME).unwrap_err().contains("link"));
        // Two names: fine for the person's own file, refused when another user owns it.
        let two = Facts { links: 2, ..facts(ME) };
        assert_eq!(judge_facts(&two, ME), Ok(()));
        let theirs = Facts { links: 2, ..facts("S-1-5-32-544") };
        assert_eq!(judge_facts(&theirs, ME), Ok(()), "Administrators-owned is as trusted as before");
        let other = Facts { links: 2, ..facts("S-1-5-21-9-9-9-500") };
        assert!(judge_facts(&other, ME).is_err());
        // A folder's link count means nothing.
        assert_eq!(judge_facts(&Facts { dir: true, links: 5, ..facts(ME) }, ME), Ok(()));
        for zone in [3, 4] {
            let marked = Facts { zone: Some(zone), ..facts(ME) };
            assert!(judge_facts(&marked, ME).unwrap_err().contains("unblock the file first"), "zone {zone}");
        }
        for zone in [0, 1, 2] {
            assert_eq!(judge_facts(&Facts { zone: Some(zone), ..facts(ME) }, ME), Ok(()), "zone {zone}");
        }
        // The owner and writer rules still apply.
        assert!(judge_facts(&Facts { dacl: None, ..facts(ME) }, ME).is_err());
    }

    /// Only the TrustedInstaller service may write, not any service account.
    /// 13x review round 1 item 4 (B2): a folder TrustedInstaller owns (a system-protected runtime
    /// folder) is trusted as an owner; any other account is not.
    #[test]
    fn a_trusted_installer_owned_folder_is_trusted() {
        let me = "S-1-5-21-1-2-3-1001";
        assert!(judge(TRUSTED_INSTALLER, me, Some(&[ace(TRUSTED_INSTALLER, 0x1f01ff), ace("S-1-5-32-545", 0x1200a9)])).is_ok());
        assert!(judge("S-1-5-21-1-2-3-1002", me, Some(&[])).is_err());
        assert!(judge("S-1-5-80-1-2-3-4-5", me, Some(&[])).is_err(), "another service account is no one special");
    }

    #[test]
    fn only_trusted_installer_is_a_trusted_service() {
        let ti = "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";
        assert_eq!(judge(ME, ME, Some(&[ace(ME, FULL), ace(ti, FULL)])), Ok(()));
        assert!(judge(ME, ME, Some(&[ace(ME, FULL), ace("S-1-5-80-1-2-3-4-5", FULL)])).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn dynamic_open_reads_through_one_handle() {
        let base = std::path::PathBuf::from(std::env::var_os("APPDATA").expect("APPDATA"));
        let dir = base.join(format!("tandem-dp-open-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.json");
        std::fs::write(&file, b"{\"Profiles\": []}").unwrap();
        let got = open(&file, 1 << 20).unwrap();
        assert!(!got.dir && got.trust.is_ok(), "{:?}", got.trust);
        assert_eq!(got.bytes.unwrap(), b"{\"Profiles\": []}");
        let d = open(&dir, 1 << 20).unwrap();
        assert!(d.dir && d.trust.is_ok());
        // The bytes are not read from a file that is refused.
        crate::inert_files::mark(&file);
        let marked = open(&file, 1 << 20).unwrap();
        assert!(marked.trust.unwrap_err().contains("unblock the file first"));
        assert!(marked.bytes.is_err(), "a refused file is not read");
        // A junction is refused as the link it is, and is not followed.
        let target = dir.join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("b.json"), b"{}").unwrap();
        let junction = dir.join("junc");
        let made = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&junction).arg(&target).output().unwrap();
        assert!(made.status.success(), "mklink: {}", String::from_utf8_lossy(&made.stdout));
        let j = open(&junction, 1 << 20).unwrap();
        assert!(j.trust.unwrap_err().contains("link"));
        // A file larger than the limit is an error, not a partial read.
        std::fs::write(dir.join("big.json"), vec![b' '; 2000]).unwrap();
        assert!(open(&dir.join("big.json"), 1000).unwrap().bytes.is_err());
        let _ = std::process::Command::new("cmd").args(["/C", "rmdir"]).arg(&junction).output();
        let _ = std::fs::remove_dir_all(&dir);
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
        assert_eq!(open(&dir, 1 << 20).unwrap().trust, Ok(()), "the person's own folder");
        assert_eq!(open(&file, 1 << 20).unwrap().trust, Ok(()));
        assert!(open(&dir.join("missing.json"), 1 << 20).is_err(), "unreadable means not trusted");
        // C:\Users\Public is writable by everyone.
        assert!(open(std::path::Path::new("C:\\Users\\Public"), 1 << 20).unwrap().trust.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

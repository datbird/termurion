//! The install's approval key at rest: DPAPI protects it for this user, and the key file is made once,
//! owner-only. The autofill contact card uses the same DPAPI wrapper with its own entropy.

use crate::workgroup_restore::{KEY_FILE, KEY_LEN};

/// The entropy the key is protected with: DPAPI unprotects it only with these bytes too, so a blob
/// another program protected for this user is no key either.
const KEY_ENTROPY: &[u8] = b"termurion workgroup approval key\0v1";

/// DPAPI (`CryptProtectData`, this user's scope, no UI) over `data` with `KEY_ENTROPY`, or the
/// reverse. None on any failure.
fn dpapi(data: &[u8], protect: bool) -> Option<Vec<u8>> {
    dpapi_with(data, protect, KEY_ENTROPY)
}

/// `dpapi` with the caller's own entropy: THE DPAPI wrapper (the autofill contact card at rest uses
/// it with its own entropy, so a blob made for one use never opens as the other).
pub fn dpapi_with(data: &[u8], protect: bool, entropy: &[u8]) -> Option<Vec<u8>> {
    use windows_sys::Win32::Foundation::{HLOCAL, LocalFree};
    use windows_sys::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData};
    let input = CRYPT_INTEGER_BLOB { cbData: u32::try_from(data.len()).ok()?, pbData: data.as_ptr() as *mut u8 };
    let entropy = CRYPT_INTEGER_BLOB { cbData: u32::try_from(entropy.len()).ok()?, pbData: entropy.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    // SAFETY: the blobs point at live buffers of the lengths given (DPAPI only reads them); the
    // output is copied out and freed once.
    unsafe {
        let ok = if protect {
            CryptProtectData(&input, std::ptr::null(), &entropy, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        } else {
            CryptUnprotectData(&input, std::ptr::null_mut(), &entropy, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        };
        if ok == 0 || out.pbData.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        if !protect {
            std::ptr::write_bytes(out.pbData, 0, out.cbData as usize);
        }
        LocalFree(out.pbData as HLOCAL);
        Some(bytes)
    }
}

/// The key as the key file holds it: protected for this user (`dpapi`).
pub fn protect_key(key: &[u8; KEY_LEN]) -> Option<Vec<u8>> {
    dpapi(key, true)
}

/// The key a key file's bytes hold, when this user's DPAPI unprotects them with Termurion's entropy
/// into exactly a key. Plain bytes, another user's or another install's blob: None.
pub fn unprotect_key(blob: &[u8]) -> Option<[u8; KEY_LEN]> {
    let plain = dpapi(blob, false)?;
    plain.as_slice().try_into().ok()
}

/// The install's approval key in `dir` (Termurion's settings folder), made when absent. The file holds
/// the key protected with DPAPI for this user (`protect_key`) and is read only when it is this
/// user's own owner-only file (`localfs::is_owner_only`), never through a link. So no file copied
/// in (an archive's, through any name) can be a key: one that is not owner-only or does not
/// unprotect is removed (through the handle that judged it, never a file another Termurion put there
/// since) and a fresh key made, so every earlier approval asks again (Task 9 findings r2). A new
/// key is written whole to an owner-only temporary file and then given the name only if no other
/// Termurion gave it one first (then that key is read), so no reader ever sees half a key. None when
/// the folder or file cannot be used: then nothing restores unasked.
pub fn key_in(dir: &std::path::Path) -> Option<[u8; KEY_LEN]> {
    use crate::localfs;
    use std::io::{Read, Write};
    use std::os::windows::fs::OpenOptionsExt;
    let dir = localfs::LocalPath::judge(dir)?;
    localfs::create_dir_all(&dir).ok()?;
    let file = dir.unfollowed(KEY_FILE)?;
    // A file at the name: its key, or None after removing it.
    let read = |file: &localfs::Unfollowed| -> std::io::Result<Option<[u8; KEY_LEN]>> {
        const DELETE: u32 = 0x0001_0000;
        const READ_CONTROL: u32 = 0x0002_0000;
        let mut f = localfs::open_unfollowed(file, |o| {
            o.access_mode(windows_sys::Win32::Foundation::GENERIC_READ | READ_CONTROL | DELETE).share_mode(windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ);
        })?;
        let mut blob = Vec::new();
        let key = if localfs::is_owner_only(&f) && (&mut f).take(4096).read_to_end(&mut blob).is_ok() { unprotect_key(&blob) } else { None };
        if key.is_none() {
            delete_open(&f)?;
        }
        Ok(key)
    };
    // Another Termurion holding the file open for writing (32, a sharing violation) or removing it (5,
    // delete-pending until its handle closes) is waited for, up to about a second.
    let busy = |e: &std::io::Error| matches!(e.raw_os_error(), Some(32) | Some(5));
    for _ in 0..50 {
        match read(&file) {
            Ok(Some(k)) => return Some(k),
            Ok(None) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) if busy(&e) => {
                std::thread::sleep(std::time::Duration::from_millis(20));
                continue;
            }
            Err(_) => return None,
        }
        let mut k = [0u8; KEY_LEN];
        getrandom::fill(&mut k).ok()?;
        let blob = protect_key(&k)?;
        let temp = dir.unfollowed(format!("{KEY_FILE}.{}.tmp", uuid::Uuid::new_v4()))?;
        let written = localfs::create_owner_only(&temp).and_then(|mut f| f.write_all(&blob).and_then(|()| f.sync_all()));
        if written.is_err() {
            let _ = localfs::remove_unfollowed(&temp);
            return None;
        }
        let named = rename_new(&temp, &file);
        let _ = localfs::remove_unfollowed(&temp);
        match named {
            Ok(()) => return Some(k),
            // Another copy of Termurion named its key first: read that one.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            // The name is still delete-pending under another Termurion's handle: try again.
            Err(e) if busy(&e) => {
                std::thread::sleep(std::time::Duration::from_millis(20));
                continue;
            }
            Err(_) => return None,
        }
    }
    None
}

/// Removes the open file (opened with DELETE access): the very file that was judged, whatever has
/// the name by then. POSIX semantics first, so the name is free at once (another Termurion starting now
/// makes its key instead of meeting a delete-pending name); where the volume has none (FAT, older
/// Windows), the classic mark that removes it when its handles close (`key_in` then waits).
fn delete_open(f: &std::fs::File) -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle};
    // FILE_DISPOSITION_INFO_EX: FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
    // class FileDispositionInfoEx (21).
    let ex: u32 = 0x1 | 0x2;
    // SAFETY: an open handle and a structure of the size given.
    if unsafe { SetFileInformationByHandle(f.as_raw_handle() as _, 21, &ex as *const _ as *const _, std::mem::size_of::<u32>() as u32) } != 0 {
        return Ok(());
    }
    let info = FILE_DISPOSITION_INFO { DeleteFile: 1 };
    // SAFETY: an open handle and a structure of the size given.
    let ok = unsafe { SetFileInformationByHandle(f.as_raw_handle() as _, FileDispositionInfo, &info as *const _ as *const _, std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32) };
    if ok == 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
}

/// `from` renamed to `to` only when nothing has that name (`MoveFileExW` without
/// `MOVEFILE_REPLACE_EXISTING`): AlreadyExists otherwise. The file keeps its own DACL.
fn rename_new(from: &crate::localfs::Unfollowed, to: &crate::localfs::Unfollowed) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let wide = |p: &std::path::Path| p.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<u16>>();
    let (a, b) = (wide(from.path()), wide(to.path()));
    // SAFETY: both NUL-terminated and live across the call.
    let ok = unsafe { windows_sys::Win32::Storage::FileSystem::MoveFileExW(a.as_ptr(), b.as_ptr(), 0) };
    if ok != 0 {
        return Ok(());
    }
    let e = std::io::Error::last_os_error();
    // ERROR_ALREADY_EXISTS (183) / ERROR_FILE_EXISTS (80).
    if matches!(e.raw_os_error(), Some(183) | Some(80)) { Err(std::io::ErrorKind::AlreadyExists.into()) } else { Err(e) }
}

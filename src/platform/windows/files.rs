//! Windows file calls behind `localfs`: opening an entry without following a reparse point, the
//! owner-only file and its access list, link counts, free space, change watches, the facts of a
//! folder entry, and ShellExecute.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::PathBuf;

use crate::localfs::{file_stat, LocalPath, Unfollowed};

/// Opens the entry itself with the options `configure` sets; the reparse point is never followed
/// (`FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS` are always set, last).
pub fn open_unfollowed(p: &Unfollowed, configure: impl FnOnce(&mut OpenOptions)) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT};
    let mut options = OpenOptions::new();
    configure(&mut options);
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS).open(p.path())
}

/// Makes a NEW file at the entry, readable and writable by this user alone: owned by the user's SID
/// (an elevated token would own it as Administrators) with a protected DACL of one allow entry for
/// that SID (nothing inherited), `CREATE_NEW` so an existing entry (a
/// planted file or link) is refused rather than reused, and the reparse point never followed.
/// Another user's share of the folder gets no access, whatever the folder's own ACL says.
pub fn create_owner_only(p: &Unfollowed) -> io::Result<File> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{GENERIC_WRITE, HLOCAL, INVALID_HANDLE_VALUE, LocalFree};
    use windows_sys::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
    use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows_sys::Win32::Storage::FileSystem::{CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ};
    let sid = crate::platform::windows::ipc::user_sid().map_err(io::Error::other)?;
    let sddl: Vec<u16> = format!("O:{sid}D:P(A;;FA;;;{sid})").encode_utf16().chain(std::iter::once(0)).collect();
    let name: Vec<u16> = p.path().as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    // SAFETY: both strings are NUL-terminated and outlive the calls; the descriptor is freed once;
    // the handle is owned by the `File` made from it.
    unsafe {
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, std::ptr::null_mut()) == 0 {
            return Err(io::Error::last_os_error());
        }
        let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd, bInheritHandle: 0 };
        let h = CreateFileW(name.as_ptr(), GENERIC_WRITE, FILE_SHARE_READ, &sa, CREATE_NEW, FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT, std::ptr::null_mut());
        let err = io::Error::last_os_error();
        LocalFree(sd as HLOCAL);
        if h == INVALID_HANDLE_VALUE {
            return Err(err);
        }
        Ok(File::from_raw_handle(h as _))
    }
}

/// Whether the open file is this user's own plain single-link file with the owner-only DACL
/// `create_owner_only` makes: owned by the user's SID, protected, one allow entry (full access)
/// for that SID. The handle was opened with read access.
pub fn is_owner_only(f: &File) -> bool {
    let Ok(st) = file_stat(f) else { return false };
    let owner_only = |sid: &str| dacl_sddl_of(f).is_ok_and(|s| s == format!("D:P(A;;FA;;;{sid})"));
    st.is_file
        && !st.is_reparse
        && link_count(f).is_ok_and(|n| n == 1)
        && matches!((crate::platform::windows::ipc::file_owner_sid(f), crate::platform::windows::ipc::user_sid()), (Ok(a), Ok(b)) if a == b && owner_only(&b))
}

/// How many names (hard links) the open file has.
pub fn link_count(f: &File) -> io::Result<u32> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle};
    // SAFETY: a valid handle and a zeroed out-structure.
    unsafe {
        let mut info: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
        if GetFileInformationByHandle(f.as_raw_handle() as _, &mut info) == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(info.nNumberOfLinks)
    }
}

/// The entry's DACL as SDDL text (a test reads back what `create_owner_only` set).
pub fn dacl_sddl(p: &Unfollowed) -> io::Result<String> {
    let f = open_unfollowed(p, |o| {
        o.read(true);
    })?;
    dacl_sddl_of(&f)
}

/// An open file's DACL as SDDL text (the handle was opened with read access).
pub fn dacl_sddl_of(f: &File) -> io::Result<String> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{HLOCAL, LocalFree};
    use windows_sys::Win32::Security::Authorization::{ConvertSecurityDescriptorToStringSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR};
    // SAFETY: the handle is open; the descriptor and the text are freed once.
    unsafe {
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        let rc = GetSecurityInfo(f.as_raw_handle() as _, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), &mut sd);
        if rc != 0 {
            return Err(io::Error::from_raw_os_error(rc as i32));
        }
        let mut text: *mut u16 = std::ptr::null_mut();
        let ok = ConvertSecurityDescriptorToStringSecurityDescriptorW(sd, SDDL_REVISION_1, DACL_SECURITY_INFORMATION, &mut text, std::ptr::null_mut());
        let err = io::Error::last_os_error();
        LocalFree(sd as HLOCAL);
        if ok == 0 {
            return Err(err);
        }
        let mut n = 0;
        while *text.add(n) != 0 {
            n += 1;
        }
        let out = String::from_utf16_lossy(std::slice::from_raw_parts(text, n));
        LocalFree(text as HLOCAL);
        Ok(out)
    }
}

/// The path an OPEN handle really names (`GetFinalPathNameByHandleW`, every link followed), so a
/// check on it is a check on what a read of that handle returns, whatever was swapped meanwhile.
/// None when the system cannot say (and on other hosts).
pub fn final_path(f: &File) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW;
    let mut buf = vec![0u16; 4096];
    // SAFETY: an open handle and a buffer of the length given.
    let n = unsafe { GetFinalPathNameByHandleW(f.as_raw_handle() as _, buf.as_mut_ptr(), buf.len() as u32, 0) } as usize;
    (n != 0 && n < buf.len()).then(|| PathBuf::from(std::ffi::OsString::from_wide(&buf[..n])))
}

/// The bytes free for the person on the drive holding a judged folder (`GetDiskFreeSpaceExW`).
pub fn free_space(dir: &LocalPath) -> io::Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = dir.path().as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free = 0u64;
    // SAFETY: `wide` is NUL-terminated; the other outputs are optional.
    if unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(free)
}

/// A move that never replaces: `AlreadyExists` when `to` is there. It is `MoveFileExW` with
/// no flags (the standard library's rename passes MOVEFILE_REPLACE_EXISTING), atomic against a file
/// that appears at the name at any time.
/// Windows has no folder that mounts on touch; UNC paths are `is_network_path`'s.
pub fn is_auto_mount(_path: &str) -> bool {
    false
}

pub fn rename_no_replace(from: &LocalPath, to: &LocalPath) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;
    let wide = |p: &std::path::Path| p.as_os_str().encode_wide().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (f, t) = (wide(from.path()), wide(to.path()));
    // SAFETY: both are NUL-terminated wide strings that live through the call.
    if unsafe { MoveFileExW(f.as_ptr(), t.as_ptr(), 0) } != 0 {
        return Ok(());
    }
    let e = io::Error::last_os_error();
    // ERROR_ALREADY_EXISTS (183) and ERROR_FILE_EXISTS (80).
    Err(if matches!(e.raw_os_error(), Some(183) | Some(80)) { io::Error::from(io::ErrorKind::AlreadyExists) } else { e })
}

/// A change notification on the whole tree under a judged folder, None when it cannot be watched.
pub fn watch_tree(dir: &LocalPath, filter: u32) -> Option<windows_sys::Win32::Foundation::HANDLE> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::FindFirstChangeNotificationW;
    let wide: Vec<u16> = dir.path().as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: `wide` is NUL-terminated and outlives the call.
    let h = unsafe { FindFirstChangeNotificationW(wide.as_ptr(), 1, filter) };
    (h != INVALID_HANDLE_VALUE).then_some(h)
}

pub fn entry_is_reparse(m: &std::fs::Metadata) -> bool {
    std::os::windows::fs::MetadataExt::file_attributes(m) & 0x400 != 0
}

pub fn entry_is_hidden(m: &std::fs::Metadata) -> bool {
    std::os::windows::fs::MetadataExt::file_attributes(m) & 0x2 != 0
}

/// ShellExecute `verb` ("open", "edit") on a judged file or folder: ShellExecute takes a file only
/// as a `LocalPath` (round 5 item 4).
pub fn shell_verb(verb: &str, p: &LocalPath, cwd: Option<&LocalPath>, background: bool) -> Result<(), String> {
    crate::platform::shell_open::shell_verb(verb, p.path(), cwd.map(|d| d.path()), background)
}

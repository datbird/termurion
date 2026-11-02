//! Windows shell folders and the Recycle Bin.

use std::path::{Path, PathBuf};

use super::super::Unsupported;

/// The user's Desktop folder (where Windows keeps it, OneDrive included).
pub fn desktop() -> Option<PathBuf> {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_Desktop, SHGetKnownFolderPath};
    let mut out: windows_sys::core::PWSTR = std::ptr::null_mut();
    let hr = unsafe { SHGetKnownFolderPath(&FOLDERID_Desktop, 0, std::ptr::null_mut(), &mut out) };
    if hr < 0 || out.is_null() {
        return None;
    }
    let len = (0..).take_while(|&i| unsafe { *out.add(i) } != 0).count();
    let path = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(out, len) });
    unsafe { CoTaskMemFree(out as _) };
    Some(path.into())
}

/// `SHFileOperationW` with undo allowed, no questions and no error window; a failure is ignored, as
/// iTerm2 ignores it.
pub fn recycle(path: &Path) -> Result<(), Unsupported> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE, SHFILEOPSTRUCTW};
    // The list of paths ends with two NULs.
    let mut from: Vec<u16> = path.as_os_str().encode_wide().collect();
    from.extend([0, 0]);
    let mut op: SHFILEOPSTRUCTW = unsafe { std::mem::zeroed() };
    op.wFunc = FO_DELETE;
    op.pFrom = from.as_ptr();
    op.fFlags = (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT) as u16;
    unsafe { SHFileOperationW(&mut op) };
    Ok(())
}

/// `AssocQueryStringW(ASSOCSTR_FRIENDLYAPPNAME)` for the "edit" verb of `.py`.
pub fn edit_app_name() -> Option<String> {
    use windows_sys::Win32::UI::Shell::{AssocQueryStringW, ASSOCF_INIT_IGNOREUNKNOWN, ASSOCF_NOTRUNCATE, ASSOCSTR_FRIENDLYAPPNAME};
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (ext, verb) = (wide(".py"), wide("edit"));
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    let hr = unsafe { AssocQueryStringW(ASSOCF_INIT_IGNOREUNKNOWN | ASSOCF_NOTRUNCATE, ASSOCSTR_FRIENDLYAPPNAME, ext.as_ptr(), verb.as_ptr(), buf.as_mut_ptr(), &mut len) };
    if hr != 0 {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let name = String::from_utf16_lossy(&buf[..end]);
    (!name.trim().is_empty()).then_some(name)
}

/// A symbolic link, a junction or any other reparse point (`FILE_ATTRIBUTE_REPARSE_POINT`).
pub fn is_reparse(p: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    p.symlink_metadata().is_ok_and(|m| m.file_attributes() & 0x400 != 0)
}

/// The name the existing entry at `p` really has (its final path's last part, the reparse point
/// itself never followed); None when there is none.
pub fn final_name(p: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, GetFinalPathNameByHandleW};
    let f = std::fs::OpenOptions::new().access_mode(0).custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).open(p).ok()?;
    let mut buf = vec![0u16; 1024];
    // SAFETY: an open handle and a buffer of the length given.
    let n = unsafe { GetFinalPathNameByHandleW(f.as_raw_handle() as _, buf.as_mut_ptr(), buf.len() as u32, 0) } as usize;
    if n == 0 || n >= buf.len() {
        return None;
    }
    let path = PathBuf::from(std::ffi::OsString::from_wide(&buf[..n]));
    Some(path.file_name()?.to_string_lossy().into_owned())
}

/// The handle-based rename: the folder is opened without following a link and without the
/// delete share right, so it cannot be renamed or replaced while the call runs; the new file is
/// opened for DELETE (no link followed) and renamed with `FileRenameInfo` to `target` in that
/// folder, replacing the old file. The folder must still be the judged one (`judged`: volume serial
/// and file index). The target is the bare file name joined to the path the folder's own handle
/// reports (links resolved), so the destination is never resolved again from the textual path.
pub fn replace_in_folder(folder: &Path, judged: Option<(u32, u64)>, tmp: &str, target: &str) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, DELETE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileRenameInfo, GetFileInformationByHandle, GetFinalPathNameByHandleW, SetFileInformationByHandle};
    // The folder through 13v1's one path judge, then opened as itself (no link followed).
    let folder = crate::localfs::judge_io(folder)?;
    let dir = crate::localfs::open_unfollowed(&crate::localfs::Unfollowed::from(folder.clone()), |o| {
        o.access_mode(FILE_READ_ATTRIBUTES).share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
    })?;
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: the handle is open for the call and `info` is a plain out structure.
    if unsafe { GetFileInformationByHandle(dir.as_raw_handle() as HANDLE, &mut info) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 || info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(std::io::Error::other("the folder is a link"));
    }
    // The folder reached by this path now is the folder that was judged, not one swapped in since.
    let now = (info.dwVolumeSerialNumber, (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow));
    // An unknown identity is a refusal too: the folder cannot be shown to be the judged one.
    if judged != Some(now) {
        return Err(std::io::Error::other("the folder is not the one that was judged"));
    }
    let tmp = folder.unfollowed(tmp).ok_or_else(crate::localfs::refused)?;
    let new = crate::localfs::open_unfollowed(&tmp, |o| {
        o.access_mode(DELETE).share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
    })?;
    // FILE_RENAME_INFO: ReplaceIfExists (4 bytes, padded to 8), RootDirectory (8, none), FileNameLength
    // (4, in bytes), then the name.
    // The folder's own path as the HANDLE reports it (links resolved), then the bare file name: a
    // bare name alone is resolved against the process's current folder by the call.
    let mut base = vec![0u16; 4096];
    // SAFETY: the handle is open; the buffer length is passed.
    let n = unsafe { GetFinalPathNameByHandleW(dir.as_raw_handle() as HANDLE, base.as_mut_ptr(), base.len() as u32, 0) } as usize;
    if n == 0 || n >= base.len() {
        return Err(std::io::Error::other("the folder's path could not be read from its handle"));
    }
    base.truncate(n);
    if base.last() != Some(&(b'\\' as u16)) {
        base.push(b'\\' as u16);
    }
    let name: Vec<u16> = base.into_iter().chain(std::ffi::OsStr::new(target).encode_wide()).collect();
    let mut buf = vec![0u8; 24 + name.len() * 2];
    buf[0] = 1;
    buf[16..20].copy_from_slice(&((name.len() * 2) as u32).to_le_bytes());
    for (i, u) in name.iter().enumerate() {
        buf[20 + i * 2..22 + i * 2].copy_from_slice(&u.to_le_bytes());
    }
    // SAFETY: the handle is open; the buffer is a FILE_RENAME_INFO of the size passed.
    let ok = unsafe { SetFileInformationByHandle(new.as_raw_handle() as HANDLE, FileRenameInfo, buf.as_ptr().cast(), buf.len() as u32) };
    drop(dir);
    if ok == 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
}

/// The folder Windows keeps Downloads in for this user.
pub fn downloads() -> Option<PathBuf> {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_Downloads, SHGetKnownFolderPath};
    let mut out: windows_sys::core::PWSTR = std::ptr::null_mut();
    // SAFETY: `out` receives a CoTaskMem string that is freed below.
    let hr = unsafe { SHGetKnownFolderPath(&FOLDERID_Downloads, 0, std::ptr::null_mut(), &mut out) };
    if hr < 0 || out.is_null() {
        return None;
    }
    let len = (0..).take_while(|&i| unsafe { *out.add(i) } != 0).count();
    let path = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(out, len) });
    unsafe { CoTaskMemFree(out as _) };
    Some(path.into())
}

#[cfg(test)]
pub fn short_name(p: &Path) -> Option<String> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    let wide: Vec<u16> = p.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut buf = vec![0u16; 1024];
    // SAFETY: a NUL-terminated path and a buffer of the length given.
    let n = unsafe { windows_sys::Win32::Storage::FileSystem::GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let short = PathBuf::from(std::ffi::OsString::from_wide(&buf[..n.min(buf.len())]));
    let name = short.file_name()?.to_string_lossy().into_owned();
    (n > 0 && name != p.file_name()?.to_string_lossy()).then_some(name)
}

//! The one gate before Termurion touches a path that came from outside: a file or folder another
//! program named, a document, a saved window or arrangement, a setting, a pane's working folder.
//! Even `is_dir` on a share path connects to it (and sends the person's NTLM hash), so a path is
//! judged first by `netpath::judge` (text, the drive letter's device map, the drive type of a local
//! volume only, then every folder's reparse data root first) and only a `LocalPath` reaches the
//! filesystem: `LocalPath` is made only by the judge, and every probe here takes one, so a probe on
//! an unjudged path does not compile. Termurion's own config and data paths and PATH lookups may use
//! `std::fs` directly; `every_path_probe_goes_through_the_judge` lists each such site with its
//! reason and fails on any other raw probe anywhere in `src`.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A path the judge found local: every folder from the root down was read for links (each link's
/// target judged the same way) and no step touched the network. Only `LocalPath::judge` (and its
/// test and Semantic History forms) makes one.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LocalPath(PathBuf);

impl LocalPath {
    /// The judge with the real device map and reparse reader.
    pub fn judge(p: impl AsRef<Path>) -> Option<LocalPath> {
        LocalPath::judge_with(p, &crate::netpath::read_reparse, &*crate::netpath::real_drives())
    }

    /// The same judge with the reparse reader and drive answers given (tests).
    pub fn judge_with(p: impl AsRef<Path>, read_link: &dyn Fn(&Path) -> Option<PathBuf>, drives: &dyn crate::netpath::Drives) -> Option<LocalPath> {
        let p = p.as_ref();
        crate::netpath::judge(p, read_link, drives).then(|| LocalPath(p.to_path_buf()))
    }

    /// Semantic History's judge: the same rule with the person's ignore list and
    /// `EnableSemanticHistoryOnNetworkMounts` (a mapped drive only when that says so; never UNC).
    pub fn semantic(p: &str, probe: &crate::netpath::Probe) -> Option<LocalPath> {
        crate::netpath::may_probe(p, probe).then(|| LocalPath(PathBuf::from(p)))
    }

    /// `semantic` with one click's shared read budget.
    pub fn semantic_with(p: &str, probe: &crate::netpath::Probe, budget: &crate::netpath::Budget) -> Option<LocalPath> {
        crate::netpath::may_probe_with(p, probe, &crate::netpath::read_reparse, budget).then(|| LocalPath(PathBuf::from(p)))
    }

    /// A path the person picked in a file dialog at this moment (judge rulings r3, item 1): their
    /// own choice, so it may be on a share (Windows authenticates as it does for every app). Made
    /// only by `pick_file` and `save_file`, from the dialog they are given; every other path is judged.
    fn person_picked(p: &Path) -> LocalPath {
        LocalPath(p.to_path_buf())
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn into_path_buf(self) -> PathBuf {
        self.0
    }

    /// `rel` under this path, judged again (`rel` may climb out or be a link itself).
    pub fn join(&self, rel: impl AsRef<Path>) -> Option<LocalPath> {
        LocalPath::judge(self.0.join(rel))
    }

    /// One entry `name` of this folder: only the entry's own reparse data is read (every folder
    /// above it was judged with this path); a link there is judged whole. Anything but a single
    /// plain name is judged whole too.
    pub fn child(&self, name: impl AsRef<Path>) -> Option<LocalPath> {
        let name = name.as_ref();
        let mut parts = name.components();
        let plain = matches!((parts.next(), parts.next()), (Some(std::path::Component::Normal(_)), None));
        let path = self.0.join(name);
        if !plain || crate::scripts::is_device_name(name) {
            return LocalPath::judge(path);
        }
        match crate::netpath::read_reparse(&path) {
            None => Some(LocalPath(path)),
            Some(_) => LocalPath::judge(path),
        }
    }

    /// The folder above: every folder above a judged path was itself judged.
    pub fn parent(&self) -> Option<LocalPath> {
        self.0.parent().filter(|p| !p.as_os_str().is_empty()).map(|p| LocalPath(p.to_path_buf()))
    }
}

/// An entry of a judged folder that is opened only as itself: a link there is never followed, so
/// its target is not judged (or touched). Only `LocalPath::unfollowed` makes one and only
/// `open_unfollowed` opens it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unfollowed(PathBuf);

impl Unfollowed {
    pub fn path(&self) -> &Path {
        &self.0
    }
}

/// A judged path may be opened as itself too.
impl From<LocalPath> for Unfollowed {
    fn from(p: LocalPath) -> Unfollowed {
        Unfollowed(p.0)
    }
}

impl LocalPath {
    /// The entry `name` of this folder, to open as itself (`open_unfollowed`); None for anything but
    /// one plain name that is no device name.
    pub fn unfollowed(&self, name: impl AsRef<Path>) -> Option<Unfollowed> {
        let name = name.as_ref();
        let mut parts = name.components();
        let plain = matches!((parts.next(), parts.next()), (Some(std::path::Component::Normal(_)), None));
        (plain && !crate::scripts::is_device_name(name)).then(|| Unfollowed(self.0.join(name)))
    }
}

/// Opens the entry itself with the options `configure` sets; the reparse point is never followed
/// (`FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS` are always set, last).
#[cfg(windows)]
pub fn open_unfollowed(p: &Unfollowed, configure: impl FnOnce(&mut OpenOptions)) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT};
    let mut options = OpenOptions::new();
    configure(&mut options);
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS).open(&p.0)
}

/// Makes a NEW file at the entry, readable and writable by this user alone: owned by the user's SID
/// (an elevated token would own it as Administrators) with a protected DACL of one allow entry for
/// that SID (nothing inherited), `CREATE_NEW` so an existing entry (a
/// planted file or link) is refused rather than reused, and the reparse point never followed.
/// Another user's share of the folder gets no access, whatever the folder's own ACL says.
#[cfg(windows)]
pub fn create_owner_only(p: &Unfollowed) -> io::Result<File> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{GENERIC_WRITE, HLOCAL, INVALID_HANDLE_VALUE, LocalFree};
    use windows_sys::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
    use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows_sys::Win32::Storage::FileSystem::{CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ};
    let sid = crate::platform::windows::ipc::user_sid().map_err(io::Error::other)?;
    let sddl: Vec<u16> = format!("O:{sid}D:P(A;;FA;;;{sid})").encode_utf16().chain(std::iter::once(0)).collect();
    let name: Vec<u16> = p.0.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
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

/// Makes a NEW file at the entry, readable and writable by this user alone (mode 0600), refusing an
/// existing entry (a planted file or link) and never following a link at the name.
#[cfg(unix)]
pub fn create_owner_only(p: &Unfollowed) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new().write(true).create_new(true).mode(0o600).custom_flags(libc::O_NOFOLLOW).open(&p.0)
}

/// Whether the open file is this user's own plain single-link file with the owner-only DACL
/// `create_owner_only` makes: owned by the user's SID, protected, one allow entry (full access)
/// for that SID. The handle was opened with read access.
#[cfg(windows)]
pub fn is_owner_only(f: &File) -> bool {
    let Ok(st) = file_stat(f) else { return false };
    let owner_only = |sid: &str| dacl_sddl_of(f).is_ok_and(|s| s == format!("D:P(A;;FA;;;{sid})"));
    st.is_file
        && !st.is_reparse
        && link_count(f).is_ok_and(|n| n == 1)
        && matches!((crate::platform::windows::ipc::file_owner_sid(f), crate::platform::windows::ipc::user_sid()), (Ok(a), Ok(b)) if a == b && owner_only(&b))
}

/// Opens the SQLite database at a judged path (created when absent, no URI parsing, no shared
/// mutex). The gate's one database opener: a path from outside reaches SQLite only as a `LocalPath`.
pub fn open_database(p: &LocalPath) -> rusqlite::Result<rusqlite::Connection> {
    use rusqlite::OpenFlags;
    rusqlite::Connection::open_with_flags(&p.0, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_NO_MUTEX)
}

/// How many names (hard links) the open file has.
#[cfg(windows)]
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

/// The entry itself renamed (a link is moved, never followed).
pub fn rename_unfollowed(from: &Unfollowed, to: &Unfollowed) -> io::Result<()> {
    std::fs::rename(&from.0, &to.0)
}

/// The entry itself removed (a link is deleted, its target stays).
pub fn remove_unfollowed(p: &Unfollowed) -> io::Result<()> {
    match std::fs::remove_file(&p.0) {
        // A junction or directory link goes with `remove_dir`.
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => std::fs::remove_dir(&p.0).or(Err(e)),
        other => other,
    }
}

/// The entry's DACL as SDDL text (a test reads back what `create_owner_only` set).
#[cfg(windows)]
pub fn dacl_sddl(p: &Unfollowed) -> io::Result<String> {
    let f = open_unfollowed(p, |o| {
        o.read(true);
    })?;
    dacl_sddl_of(&f)
}

/// An open file's DACL as SDDL text (the handle was opened with read access).
#[cfg(windows)]
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

impl AsRef<Path> for LocalPath {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

/// The judge as an `Option`, for a caller that reads better as `localfs::judge(p)`.
pub fn judge(p: impl AsRef<Path>) -> Option<LocalPath> {
    LocalPath::judge(p)
}

/// Whether `p` may be touched at all.
pub fn judged(p: &Path) -> bool {
    LocalPath::judge(p).is_some()
}

/// What a probe of a refused path answers: the path is treated as one Termurion may not open.
pub fn refused() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "Termurion does not open files on network locations.")
}

/// The judge as an `io::Result` (a refused path is `refused()`), for callers that go on to open.
pub fn judge_io(p: impl AsRef<Path>) -> io::Result<LocalPath> {
    LocalPath::judge(p).ok_or_else(refused)
}

/// The file the person picks in this open dialog at this moment, as a `LocalPath`: their own pick is
/// not judged (`LocalPath::person_picked`, judge rulings r3, item 1). The dialog itself is the
/// argument, so a path from Explorer, the launch pipe or the command line cannot become one (round
/// 5 item 5); those are judged. None when nothing was picked.
pub fn pick_file(dialog: rfd::FileDialog) -> Option<LocalPath> {
    dialog.pick_file().filter(|p| !p.as_os_str().is_empty()).map(|p| LocalPath::person_picked(&p))
}

/// The folder the person picks in this dialog at this moment (as `pick_file`). The caller still judges
/// where it is (the extensions root refuses a network location).
pub fn pick_folder(dialog: rfd::FileDialog) -> Option<LocalPath> {
    dialog.pick_folder().filter(|p| !p.as_os_str().is_empty()).map(|p| LocalPath::person_picked(&p))
}

/// The files the person picks in this open dialog at this moment (as `pick_file`, several allowed).
pub fn pick_files(dialog: rfd::FileDialog) -> Vec<LocalPath> {
    dialog.pick_files().unwrap_or_default().into_iter().filter(|p| !p.as_os_str().is_empty()).map(|p| LocalPath::person_picked(&p)).collect()
}

/// The file the person named in the Save Contents dialog (`save_panel`), which is not an rfd dialog
/// because it carries an accessory: the same trust as `save_file`.
pub fn save_panel_file(p: std::path::PathBuf) -> Option<LocalPath> {
    Some(p).filter(|p| !p.as_os_str().is_empty()).map(|p| LocalPath::person_picked(&p))
}

/// The file the person names in this save dialog at this moment (as `pick_file`).
pub fn save_file(dialog: rfd::FileDialog) -> Option<LocalPath> {
    dialog.save_file().filter(|p| !p.as_os_str().is_empty()).map(|p| LocalPath::person_picked(&p))
}

/// Opens a judged file or folder with its default app (ShellExecute "open"), with a judged working
/// folder; `background` asks the app not to take the focus. The one way a file from outside reaches
/// the shell: `semantic_ui::shell_open` takes URLs only.
pub fn open_with_shell(p: &LocalPath, cwd: Option<&LocalPath>, background: bool) -> io::Result<()> {
    shell_verb("open", p, cwd, background)
}

/// ShellExecute `verb` ("open", "edit") on a judged file or folder: ShellExecute takes a file only
/// as a `LocalPath` (round 5 item 4).
#[cfg(windows)]
pub fn shell_verb(verb: &str, p: &LocalPath, cwd: Option<&LocalPath>, background: bool) -> io::Result<()> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_SHOWNOACTIVATE, SW_SHOWNORMAL};
    let wide = |s: &std::ffi::OsStr| {
        use std::os::windows::ffi::OsStrExt;
        s.encode_wide().chain(Some(0)).collect::<Vec<u16>>()
    };
    let (verb, file) = (wide(verb.as_ref()), wide(p.0.as_os_str()));
    let dir = cwd.map(|d| wide(d.0.as_os_str()));
    let show = if background { SW_SHOWNOACTIVATE } else { SW_SHOWNORMAL };
    let r = unsafe { ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), std::ptr::null(), dir.as_ref().map_or(std::ptr::null(), |d| d.as_ptr()), show) };
    // Values of 32 or less are errors (SE_ERR_*).
    if r as isize > 32 { Ok(()) } else { Err(io::Error::other(format!("ShellExecute error {}", r as isize))) }
}

#[cfg(not(windows))]
pub fn shell_verb(verb: &str, p: &LocalPath, _cwd: Option<&LocalPath>, _background: bool) -> io::Result<()> {
    Err(io::Error::other(format!("{verb} {} needs Windows", p.0.display())))
}

/// Judge, then `is_dir` (a refused path is no folder).
pub fn is_local_dir(p: &Path) -> bool {
    LocalPath::judge(p).is_some_and(|p| is_dir(&p))
}

/// Judge, then `is_file`.
pub fn is_local_file(p: &Path) -> bool {
    LocalPath::judge(p).is_some_and(|p| is_file(&p))
}

/// Judge, then `exists` (a refused path is missing).
pub fn local_exists(p: &Path) -> bool {
    LocalPath::judge(p).is_some_and(|p| exists(&p))
}

/// What a probe learns about an entry, so callers never call `is_dir` on a std type (the guard reads
/// every `.is_dir()` as a probe).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stat {
    pub is_dir: bool,
    pub is_file: bool,
    /// A symbolic link or junction.
    pub is_link: bool,
    /// Any reparse point (a link, a mount point, a placeholder).
    pub is_reparse: bool,
    pub len: u64,
    pub modified: Option<SystemTime>,
    pub readonly: bool,
    /// The hidden attribute (Windows); a leading dot is the caller's rule.
    pub hidden: bool,
}

impl Stat {
    fn of(m: &std::fs::Metadata) -> Stat {
        let t = m.file_type();
        #[cfg(windows)]
        let reparse = std::os::windows::fs::MetadataExt::file_attributes(m) & 0x400 != 0;
        #[cfg(not(windows))]
        let reparse = t.is_symlink();
        #[cfg(windows)]
        let hidden = std::os::windows::fs::MetadataExt::file_attributes(m) & 0x2 != 0;
        #[cfg(not(windows))]
        let hidden = false;
        Stat { is_dir: t.is_dir(), is_file: t.is_file(), is_link: t.is_symlink(), is_reparse: reparse || t.is_symlink(), len: m.len(), modified: m.modified().ok(), readonly: m.permissions().readonly(), hidden }
    }
}

pub fn is_dir(p: &LocalPath) -> bool {
    p.0.is_dir()
}

pub fn is_file(p: &LocalPath) -> bool {
    p.0.is_file()
}

pub fn exists(p: &LocalPath) -> bool {
    p.0.exists()
}

/// What the path leads to (a link is followed; its target was judged with it).
pub fn stat(p: &LocalPath) -> io::Result<Stat> {
    std::fs::metadata(&p.0).map(|m| Stat::of(&m))
}

/// The names of a judged folder's entries and whether each is a real folder (a link or reparse point
/// is not), looking at no more than `limit` entries.
pub fn list_dir(p: &LocalPath, limit: usize) -> io::Result<Vec<(String, bool)>> {
    let rd = std::fs::read_dir(&p.0)?;
    Ok(rd
        .take(limit)
        .filter_map(Result::ok)
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            let st = Stat::of(&m);
            Some((e.file_name().to_string_lossy().to_string(), st.is_dir && !st.is_reparse))
        })
        .collect())
}

/// The entry itself (a link is not followed).
pub fn lstat(p: &LocalPath) -> io::Result<Stat> {
    std::fs::symlink_metadata(&p.0).map(|m| Stat::of(&m))
}

/// An open file's own facts (no path is touched).
pub fn file_stat(f: &File) -> io::Result<Stat> {
    f.metadata().map(|m| Stat::of(&m))
}

/// The path an OPEN handle really names (`GetFinalPathNameByHandleW`, every link followed), so a
/// check on it is a check on what a read of that handle returns, whatever was swapped meanwhile.
/// None when the system cannot say (and on other hosts).
#[cfg(windows)]
pub fn final_path(f: &File) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW;
    let mut buf = vec![0u16; 4096];
    // SAFETY: an open handle and a buffer of the length given.
    let n = unsafe { GetFinalPathNameByHandleW(f.as_raw_handle() as _, buf.as_mut_ptr(), buf.len() as u32, 0) } as usize;
    (n != 0 && n < buf.len()).then(|| PathBuf::from(std::ffi::OsString::from_wide(&buf[..n])))
}

#[cfg(not(windows))]
pub fn final_path(_f: &File) -> Option<PathBuf> {
    None
}

pub fn open(p: &LocalPath) -> io::Result<File> {
    File::open(&p.0)
}

/// Opens with the options `configure` sets on a fresh `OpenOptions` (share mode, access, flags).
pub fn open_custom(p: &LocalPath, configure: impl FnOnce(&mut OpenOptions)) -> io::Result<File> {
    let mut options = OpenOptions::new();
    configure(&mut options);
    options.open(&p.0)
}

pub fn create(p: &LocalPath) -> io::Result<File> {
    File::create(&p.0)
}

pub fn read(p: &LocalPath) -> io::Result<Vec<u8>> {
    std::fs::read(&p.0)
}

pub fn read_to_string(p: &LocalPath) -> io::Result<String> {
    std::fs::read_to_string(&p.0)
}

pub fn write(p: &LocalPath, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    std::fs::write(&p.0, bytes)
}

/// A judged path's own link target (read from the link itself, nothing followed); None when it is
/// no link.
pub fn read_link(p: &LocalPath) -> Option<PathBuf> {
    crate::netpath::read_reparse(&p.0)
}

/// The fully resolved path, judged again (it names what the links led to).
pub fn canonicalize(p: &LocalPath) -> Option<LocalPath> {
    std::fs::canonicalize(&p.0).ok().and_then(LocalPath::judge)
}

pub fn create_dir(p: &LocalPath) -> io::Result<()> {
    std::fs::create_dir(&p.0)
}

/// The bytes free for the person on the drive holding a judged folder (`GetDiskFreeSpaceExW`).
#[cfg(windows)]
pub fn free_space(dir: &LocalPath) -> io::Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = dir.0.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free = 0u64;
    // SAFETY: `wide` is NUL-terminated; the other outputs are optional.
    if unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(free)
}

/// The bytes free for the person on the file system holding a judged folder (`statvfs`).
#[cfg(unix)]
pub fn free_space(dir: &LocalPath) -> io::Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(dir.0.as_os_str().as_bytes()).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(u64::from(st.f_bavail).saturating_mul(u64::from(st.f_frsize)))
}

pub fn create_dir_all(p: &LocalPath) -> io::Result<()> {
    std::fs::create_dir_all(&p.0)
}

pub fn remove_file(p: &LocalPath) -> io::Result<()> {
    std::fs::remove_file(&p.0)
}

pub fn remove_dir(p: &LocalPath) -> io::Result<()> {
    std::fs::remove_dir(&p.0)
}

pub fn remove_dir_all(p: &LocalPath) -> io::Result<()> {
    std::fs::remove_dir_all(&p.0)
}

pub fn rename(from: &LocalPath, to: &LocalPath) -> io::Result<()> {
    std::fs::rename(&from.0, &to.0)
}

/// A move that never replaces: `AlreadyExists` when `to` is there. On Windows it is `MoveFileExW` with
/// no flags (the standard library's rename passes MOVEFILE_REPLACE_EXISTING), atomic against a file
/// that appears at the name at any time.
pub fn rename_no_replace(from: &LocalPath, to: &LocalPath) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::MoveFileExW;
        let wide = |p: &std::path::Path| p.as_os_str().encode_wide().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let (f, t) = (wide(&from.0), wide(&to.0));
        // SAFETY: both are NUL-terminated wide strings that live through the call.
        if unsafe { MoveFileExW(f.as_ptr(), t.as_ptr(), 0) } != 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        // ERROR_ALREADY_EXISTS (183) and ERROR_FILE_EXISTS (80).
        Err(if matches!(e.raw_os_error(), Some(183) | Some(80)) { io::Error::from(io::ErrorKind::AlreadyExists) } else { e })
    }
    #[cfg(not(windows))]
    {
        if to.0.exists() {
            return Err(io::Error::from(io::ErrorKind::AlreadyExists));
        }
        std::fs::rename(&from.0, &to.0)
    }
}

pub fn copy(from: &LocalPath, to: &LocalPath) -> io::Result<u64> {
    std::fs::copy(&from.0, &to.0)
}

/// A judged file or folder into an archive. Links are stored as links, never followed
/// (`follow_symlinks(false)`), so the walk below a judged folder reads nothing a link leads to.
pub fn append_to_tar<W: io::Write>(tar: &mut tar::Builder<W>, name: &Path, p: &LocalPath) -> io::Result<()> {
    tar.follow_symlinks(false);
    if is_dir(p) { tar.append_dir_all(name, &p.0) } else { tar.append_path_with_name(&p.0, name) }
}

/// A change notification on the whole tree under a judged folder, None when it cannot be watched.
#[cfg(windows)]
pub fn watch_tree(dir: &LocalPath, filter: u32) -> Option<windows_sys::Win32::Foundation::HANDLE> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::FindFirstChangeNotificationW;
    let wide: Vec<u16> = dir.0.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: `wide` is NUL-terminated and outlives the call.
    let h = unsafe { FindFirstChangeNotificationW(wide.as_ptr(), 1, filter) };
    (h != INVALID_HANDLE_VALUE).then_some(h)
}

/// The folder a child process starts in (CreateProcess opens it).
pub fn set_cwd(cmd: &mut std::process::Command, dir: &LocalPath) {
    cmd.current_dir(&dir.0);
}

/// One folder entry, its kind read from the listing itself (no further I/O on Windows).
#[derive(Clone, Debug)]
pub struct Entry {
    pub name: OsString,
    pub stat: Stat,
    path: PathBuf,
}

impl Entry {
    /// The entry's path for showing or comparing; probing it goes through `local`.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Removes the entry itself, a link included (the link, never what it leads to: deleting a
    /// reparse point does not open its target). The folder holding it was judged.
    pub fn unlink(&self) -> io::Result<()> {
        std::fs::remove_file(&self.path).or_else(|_| std::fs::remove_dir(&self.path))
    }

    /// The entry as a judged path: a plain entry of a judged folder is local as it stands; a reparse
    /// point is judged (its target read and walked).
    pub fn local(&self) -> Option<LocalPath> {
        if self.stat.is_reparse { LocalPath::judge(&self.path) } else { Some(LocalPath(self.path.clone())) }
    }
}

/// The entries of a judged folder (an unreadable entry is skipped).
pub fn entries(dir: &LocalPath) -> io::Result<Vec<Entry>> {
    Ok(std::fs::read_dir(&dir.0)?.flatten().filter_map(|e| Some(Entry { name: e.file_name(), stat: e.metadata().ok().map(|m| Stat::of(&m))?, path: e.path() })).collect())
}

/// `entries` capped at `max` (a folder with a huge number of files is not read whole).
pub fn entries_upto(dir: &LocalPath, max: usize) -> io::Result<Vec<Entry>> {
    Ok(std::fs::read_dir(&dir.0)?.flatten().take(max).filter_map(|e| Some(Entry { name: e.file_name(), stat: e.metadata().ok().map(|m| Stat::of(&m))?, path: e.path() })).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_path_is_never_judged() {
        for p in [r"\\attacker\share\f", r"//attacker/share/f", r"\\?\UNC\h\s\f", r"C:\x\NUL", "C:\\x\\COM\u{b9}", "relative\\f", r"\\.\pipe\x"] {
            assert_eq!(LocalPath::judge(p), None, "{p}");
            assert!(!judged(Path::new(p)));
        }
        let t = LocalPath::judge(std::env::temp_dir()).expect("the temp folder is local");
        assert!(is_dir(&t));
        assert_eq!(t.parent().map(|p| p.path().to_path_buf()), std::env::temp_dir().parent().map(Path::to_path_buf));
        assert_eq!(t.join(r"\\attacker\share"), None, "a join that leaves for a share is judged again");
    }

    /// A listing's plain entries are local as they stand; an entry that is a reparse point is judged.
    #[cfg(windows)]
    #[test]
    fn listing_entries_are_judged_when_they_are_links() {
        let base = std::env::temp_dir().join(format!("termurion-entries-{}", std::process::id()));
        std::fs::create_dir_all(base.join("plain")).unwrap();
        std::fs::write(base.join("f.txt"), b"x").unwrap();
        let made = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(base.join("j")).arg(base.join("plain")).output().unwrap();
        assert!(made.status.success());
        let dir = LocalPath::judge(&base).unwrap();
        let mut seen: Vec<(String, bool, bool, bool)> = entries(&dir).unwrap().iter().map(|e| (e.name.to_string_lossy().into_owned(), e.stat.is_dir, e.stat.is_reparse, e.local().is_some())).collect();
        seen.sort();
        assert_eq!(seen, [("f.txt".into(), false, false, true), ("j".into(), false, true, true), ("plain".into(), true, false, true)]);
        std::fs::remove_dir(base.join("j")).unwrap();
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// The reading of one Rust source file: test-only items blanked (every `#[cfg(test)]` or
    /// `#[test]` item, wherever it sits; line numbers kept), strings and comments left in place.
    pub(crate) fn live_code(text: &str) -> String {
        let lines: Vec<&str> = text.lines().collect();
        let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        let is_test_attr = |l: &str| {
            let t = l.trim();
            // Only test-only items: `#[cfg(any(windows, test))]` is live code on Windows.
            t == "#[test]" || t == "#[cfg(test)]" || t.starts_with("#[cfg(all(test,") || t.starts_with("#[cfg(all(test)")
        };
        let mut i = 0;
        while i < lines.len() {
            if !is_test_attr(lines[i]) {
                i += 1;
                continue;
            }
            // The item: further attributes, then its first line; it ends at `;` or when its braces close.
            let mut j = i + 1;
            while j < lines.len() && (lines[j].trim().starts_with("#[") || lines[j].trim().is_empty()) {
                j += 1;
            }
            let end = item_end(&lines, j);
            for line in out.iter_mut().take(end + 1).skip(i) {
                line.clear();
            }
            i = end + 1;
        }
        out.join("\n")
    }

    /// The last line of the item starting at line `start`: the line whose `;` ends it before any
    /// brace opens, or where the braces it opens close (strings, chars and comments skipped).
    fn item_end(lines: &[&str], start: usize) -> usize {
        let mut depth = 0i32;
        let mut opened = false;
        for (n, line) in lines.iter().enumerate().skip(start) {
            let b: Vec<char> = line.chars().collect();
            let mut k = 0;
            while k < b.len() {
                match b[k] {
                    '/' if b.get(k + 1) == Some(&'/') => break,
                    '"' => {
                        k += 1;
                        while k < b.len() && b[k] != '"' {
                            if b[k] == '\\' {
                                k += 1;
                            }
                            k += 1;
                        }
                    }
                    'r' if b.get(k + 1) == Some(&'"') || (b.get(k + 1) == Some(&'#') && b.get(k + 2) == Some(&'"')) => {
                        let hashes = b[k + 1..].iter().take_while(|&&c| c == '#').count();
                        k += 2 + hashes;
                        let close: String = std::iter::once('"').chain(std::iter::repeat_n('#', hashes)).collect();
                        let rest: String = b[k..].iter().collect();
                        match rest.find(&close) {
                            Some(at) => k += rest[..at].chars().count() + close.chars().count() - 1,
                            None => k = b.len(),
                        }
                    }
                    '\'' if b.get(k + 2) == Some(&'\'') => k += 2,
                    '\'' if b.get(k + 1) == Some(&'\\') => {
                        while k + 1 < b.len() && b[k + 1] != '\'' {
                            k += 1;
                        }
                        k += 1;
                    }
                    '{' => {
                        depth += 1;
                        opened = true;
                    }
                    '}' => {
                        depth -= 1;
                        if opened && depth == 0 {
                            return n;
                        }
                    }
                    ';' if !opened && depth == 0 => return n,
                    _ => {}
                }
                k += 1;
            }
        }
        lines.len().saturating_sub(1)
    }

    /// Raw probes: any of these on a path that did not come through the judge connects to a share
    /// path. `localfs::` calls are the judged forms and are not counted.
    const PROBES: &[&str] = &[
        ".is_dir()",
        ".is_file()",
        ".exists()",
        ".is_symlink()",
        "try_exists(",
        ".metadata()",
        ".symlink_metadata()",
        "fs::metadata(",
        "fs::symlink_metadata(",
        "fs::canonicalize(",
        ".canonicalize()",
        "File::open(",
        "File::create(",
        "File::options(",
        "OpenOptions::new(",
        "fs::read(",
        "fs::read_to_string(",
        "fs::read_dir(",
        ".read_dir()",
        "fs::read_link(",
        ".read_link()",
        "read_reparse(",
        "fs::write(",
        "fs::copy(",
        "fs::rename(",
        "fs::create_dir(",
        "fs::create_dir_all(",
        "fs::remove_file(",
        "fs::remove_dir(",
        "fs::remove_dir_all(",
        "Value::from_file(",
        "append_path(",
        "append_path_with_name(",
        "append_dir_all(",
        "image::open(",
        "ImageReader::open(",
        "set_current_dir(",
        ".current_dir(",
        "Connection::open(",
        "open_with_flags(",
        "CreateFileW(",
        "GetDiskFreeSpaceExW(",
        "FindFirstChangeNotificationW(",
        "image_dimensions(",
        "Path::is_dir(",
        "Path::exists(",
        "Path::is_file(",
        "judge_with(",
        "fs::exists(",
        "ShellExecuteW(",
        "ShellExecuteExW(",
        "FindFirstFileW(",
        "FindFirstFileExW(",
        "GetFileAttributesW(",
        "GetFileAttributesExW(",
        "fs::set_permissions(",
        ".set_permissions(",
        "fs::hard_link(",
        "symlink_file(",
        "symlink_dir(",
    ];
    /// Probe functions that must not be imported bare (`use std::fs::read_to_string;`).
    const BARE: &[&str] = &["metadata", "symlink_metadata", "canonicalize", "read", "read_to_string", "read_dir", "read_link", "write", "copy", "rename", "create_dir", "create_dir_all", "remove_file", "remove_dir", "remove_dir_all"];

    /// The files that ARE the gate: the judged helpers here, and the judge's own reparse reader.
    const GATE: &[&str] = &["localfs.rs", "netpath.rs"];

    /// Raw probes that may stay, by file and function, with how many there are: Termurion's own config
    /// and data paths (under `%APPDATA%\Termurion`, its runtime and staging folders, its own exe and the
    /// files shipped beside it, the temp folder it made), Windows' own folders, and PATH lookups. A
    /// new raw probe in a listed function changes its count and fails too.
    const OWN: &[(&str, &str, usize, &str)] = &[
        ("render_fx.rs", "discover_in", 2, "plugin.toml files in Termurion's own RenderPlugins folder beside its settings (render_fx::user_dir)"),
        ("render_fx.rs", "shader_text", 1, "a shader file inside a plugin folder of Termurion's own RenderPlugins folder; names with a separator or .. are refused first"),
        ("settings_effects.rs", "editor", 1, "creates Termurion's own RenderPlugins folder beside its settings on the person's click"),
        ("ai_discovery.rs", "load", 1, "Termurion's own model cache beside its settings (ai_discovery::default_path)"),
        ("ai_discovery.rs", "save", 2, "Termurion's own model cache beside its settings (ai_discovery::default_path)"),
        ("ai_wire_log.rs", "open", 3, "Termurion's own AIChatWire log folder beside its settings"),
        ("api_auth.rs", "read_noauth_file", 2, "the Python API's disable-auth file in the person's own profile folder (api_auth::noauth_path), opened without following a link"),
        ("app/api_ui.rs", "api_launch", 1, "Termurion's own API consent file beside its settings and the API's disable-auth file it writes"),
        ("app/api_ui.rs", "record_consent", 2, "Termurion's own API consent file beside its settings and the API's disable-auth file it writes"),
        ("app/api_ui.rs", "set_allow_all", 1, "Termurion's own API consent file beside its settings and the API's disable-auth file it writes"),
        ("app/api_ui.rs", "sync_api_server", 1, "Termurion's own API consent file beside its settings and the API's disable-auth file it writes"),
        ("app/api_ui.rs", "write_noauth_file", 4, "Termurion's own API consent file beside its settings and the API's disable-auth file it writes"),
        ("app/scripts_app.rs", "mark_tree", 2, "the Reveal Contents folder Termurion itself just created under the temp folder (marks the files it extracted there)"),
        ("pwm_run.rs", "resolve_in", 1, "a PATH folder of the person's own environment, searched for the password manager CLI (bw.cmd) to start; nothing is read from it"),
        ("app/claude_code_app.rs", "install_hook", 1, "the hook exe beside Termurion's own exe"),
        ("app/coprocess_ui.rs", "coprocess_bar", 1, "a file Termurion names in the temp folder for the coprocess's errors"),
        ("app/jumplist_ui.rs", "delete_orphans", 2, "Termurion's own Recents folder under %APPDATA%\\Termurion (the list and its .itermtab files, exact uuid names only)"),
        ("app/jumplist_ui.rs", "read_list", 1, "Termurion's own Recents folder under %APPDATA%\\Termurion (recents.json and removed.json, bounded reads)"),
        ("app/jumplist_ui.rs", "store_removed", 2, "Termurion's own Recents folder under %APPDATA%\\Termurion (removed.json)"),
        ("app/jumplist_ui.rs", "store_recents", 3, "Termurion's own Recents folder under %APPDATA%\\Termurion (the list and its .itermtab files, exact uuid names only)"),
        ("app/history_ui.rs", "history", 1, "Termurion's own paste history file"),
        ("app/history_ui.rs", "remember_paste", 1, "Termurion's own paste history file"),
        ("app/menu.rs", "ensure_pwsh_script", 2, "Termurion's own PowerShell integration script under %APPDATA% / %LOCALAPPDATA%"),
        ("app/menu.rs", "write_pwsh_script", 2, "Termurion's own PowerShell integration script under %APPDATA% / %LOCALAPPDATA%"),
        ("app/mod.rs", "default_shell", 1, "the person's own PATH entries looking for pwsh.exe"),
        ("app/mod.rs", "run", 1, "Termurion's own uv folder under %APPDATA%"),
        ("app/restore_ui.rs", "saved_windows", 3, "Termurion's own saved-windows state file and the mark beside it that says an Import All brought it"),
        ("app/restore_ui.rs", "write_state", 2, "Termurion's own saved-windows state file and the Import All mark beside it"),
        ("app/script_deps_app.rs", "deps_select", 2, "a script's container in Termurion's Scripts folder (scripts_folder judges a custom one on every call)"),
        ("app/script_deps_app.rs", "deps_upgrade_ready", 4, "a script's container in Termurion's Scripts folder (scripts_folder judges a custom one on every call)"),
        ("app/script_deps_app.rs", "deps_version_ready", 3, "a script's container in Termurion's Scripts folder (scripts_folder judges a custom one on every call)"),
        ("app/script_deps_app.rs", "edit_setup_cfg", 3, "a script's container in Termurion's Scripts folder (scripts_folder judges a custom one on every call)"),
        ("app/script_deps_app.rs", "pip_backend", 1, "a script's container in Termurion's Scripts folder (scripts_folder judges a custom one on every call)"),
        ("app/script_runtime_ui.rs", "ensure_environment", 3, "Termurion's own uv, runtime and environment folders under %APPDATA%"),
        ("app/script_runtime_ui.rs", "find_backend", 2, "Termurion's own uv folder, and python candidates from the person's own PATH"),
        ("app/script_runtime_ui.rs", "install_python_runtime", 1, "Termurion's own uv, runtime and environment folders under %APPDATA%"),
        ("app/script_runtime_ui.rs", "provision_done", 1, "Termurion's own uv, runtime and environment folders under %APPDATA%"),
        ("app/scripts_app.rs", "container_inputs", 2, "Termurion's Scripts folder (scripts_folder judges a custom one on every call), its staging and environment folders"),
        ("app/scripts_app.rs", "export_one", 1, "the Desktop known folder, a free name for the export"),
        ("app/scripts_app.rs", "export_script_to", 3, "Termurion's Scripts folder (scripts_folder judges a custom one on every call), its staging and environment folders"),
        ("app/scripts_app.rs", "extract_bytes", 2, "Termurion's Scripts folder (scripts_folder judges a custom one on every call), its staging and environment folders"),
        ("app/scripts_app.rs", "sweep_reveals", 1, "the temporary folder, listed only for Termurion's own termurion-its-* folders, which remove_path deletes without following a link"),
        ("app/scripts_app.rs", "finish_new_script", 1, "Termurion's Scripts folder (scripts_folder judges a custom one on every call), its staging and environment folders"),
        ("app/scripts_app.rs", "import_environment_ready", 1, "Termurion's Scripts folder (scripts_folder judges a custom one on every call), its staging and environment folders"),
        ("app/scripts_app.rs", "import_install", 2, "Termurion's Scripts folder (scripts_folder judges a custom one on every call), its staging and environment folders"),
        ("app/scripts_app.rs", "import_install_done", 1, "Termurion's Scripts folder (scripts_folder judges a custom one on every call), its staging and environment folders"),
        ("app/scripts_app.rs", "import_layout", 3, "Termurion's Scripts folder (scripts_folder judges a custom one on every call), its staging and environment folders"),
        ("app/scripts_app.rs", "import_replace", 1, "Termurion's Scripts folder (scripts_folder judges a custom one on every call), its staging and environment folders"),
        ("app/scripts_app.rs", "import_start", 2, "Termurion's Scripts folder (scripts_folder judges a custom one on every call), its staging and environment folders"),
        ("app/scripts_app.rs", "move_aside_again", 1, "Termurion's Scripts folder (scripts_folder judges a custom one on every call), its staging and environment folders"),
        ("app/scripts_app.rs", "new_script_save", 1, "Termurion's Scripts folder (scripts_folder judges a custom one on every call), its staging and environment folders"),
        ("app/scripts_app.rs", "recover_in", 4, "Termurion's Scripts folder (scripts_folder judges a custom one on every call), its staging and environment folders"),
        ("app/scripts_app.rs", "remove_path", 7, "Termurion's Scripts folder (scripts_folder judges a custom one on every call), its staging and environment folders"),
        ("app/scripts_app.rs", "repl_environment_failed", 2, "Termurion's own shared environments folder"),
        ("app/scripts_app.rs", "reveal_scripts_folder", 1, "Termurion's Scripts folder (scripts_folder judges a custom one on every call), its staging and environment folders"),
        ("app/scripts_app.rs", "run_autolaunch", 1, "Termurion's Scripts folder (scripts_folder judges a custom one on every call), its staging and environment folders"),
        ("app/scripts_app.rs", "spawn_script", 1, "the script's own environment under Termurion's runtime folder"),
        ("app/settings_export_ui.rs", "all_entries", 1, "Termurion's own settings folders and its export and import staging folders"),
        ("app/settings_export_ui.rs", "apply_user_defaults", 1, "Termurion's own settings folders and its export and import staging folders"),
        ("app/settings_export_ui.rs", "copy_contents", 3, "Termurion's own settings folders and its export and import staging folders"),
        ("app/settings_export_ui.rs", "entries", 1, "Termurion's own settings folders and its export and import staging folders"),
        ("app/settings_export_ui.rs", "export_entity", 3, "Termurion's own settings folders and its export and import staging folders"),
        ("app/settings_export_ui.rs", "export_into", 3, "Termurion's own settings folders and its export and import staging folders"),
        ("app/settings_export_ui.rs", "final_name", 1, "Termurion's own settings folders and its export and import staging folders"),
        ("app/settings_export_ui.rs", "importable", 2, "Termurion's own settings folders and its export and import staging folders"),
        ("app/settings_export_ui.rs", "finish_import", 8, "Termurion's own settings folders and its export and import staging folders"),
        ("app/settings_export_ui.rs", "is_reparse", 1, "Termurion's own settings folders and its export and import staging folders"),
        ("app/settings_export_ui.rs", "restored_scripts_done", 1, "Termurion's own settings folders and its export and import staging folders"),
        ("app/settings_export_ui.rs", "runtime_requirements", 2, "Termurion's own settings folders and its export and import staging folders"),
        ("app/settings_export_ui.rs", "stage_import_with", 3, "Termurion's own settings folders and its export and import staging folders"),
        ("app/restore_ui.rs", "restore_after_trouble", 1, "Termurion's own saved-windows state file, removed when the person discards the saved windows"),
        ("app/panels_app.rs", "panel_act", 1, "Termurion's own crash dump, found in its own crash folder and deleted by Discard"),
        ("app/panels_app.rs", "read_tail", 2, "Termurion's own crash log file in its own folder"),
        ("crash_report.rs", "pending", 2, "Termurion's own crash dump folder, listed for a dump this run has not reported"),
        ("app/settings_export_ui.rs", "stage_scripts", 4, "Termurion's own settings folders and its export and import staging folders"),
        ("app/shell_history_ui.rs", "save_shell_history", 1, "Termurion's own shell history file"),
        ("app/shell_history_ui.rs", "shell_history", 1, "Termurion's own shell history file"),
        ("app/snippets_ui.rs", "load_snippets", 1, "Termurion's own snippets file"),
        ("app/snippets_ui.rs", "save_snippets", 1, "Termurion's own snippets file"),
        ("app/ssh_panel_app.rs", "make_batch_folder", 1, "a new batch folder Termurion makes in the temp folder (create_dir, never adopted) and its files"),
        ("app/ssh_panel_app.rs", "ssh_download_files", 1, "a new batch folder Termurion makes in the temp folder (create_dir, never adopted) and its files"),
        ("app/toolbelt_tools_ui.rs", "tool_button", 1, "Termurion's own paste history file"),
        ("chat_store.rs", "open", 3, "Termurion's own chat database and its lock beside its settings"),
        ("chat_view.rs", "materialize", 4, "Termurion's own attachment folder in the temp folder, each step canonicalized to stay inside it"),
        ("platform/windows/conpty.rs", "load_shipped", 1, "the ConPTY files shipped beside Termurion's own exe"),
        ("git_run_win.rs", "run_in_job", 1, "the NUL device, not a path"),
        ("globals.rs", "load", 1, "Termurion's own settings file"),
        ("globals.rs", "save", 3, "Termurion's own settings file"),
        ("host_base.rs", "log", 1, "Termurion's own log file"),
        ("host.rs", "reopen_app", 1, "the person's own profile folder as the working folder of the Termurion the host restarts"),
        ("bin/termurion-host.rs", "run", 1, "termurion.exe beside the host's own exe"),
        ("crashdump.rs", "install", 1, "Termurion's own crashes folder under %LOCALAPPDATA%"),
        ("crashdump.rs", "prune", 3, "Termurion's own crashes folder under %LOCALAPPDATA% (only .dmp files)"),
        ("crashdump.rs", "write_dump", 3, "Termurion's own crashes folder under %LOCALAPPDATA%"),
        ("notes.rs", "ensure_loaded", 1, "Termurion's own notes file under %APPDATA%\\Termurion"),
        ("notes.rs", "file_exists", 1, "Termurion's own notes file under %APPDATA%\\Termurion"),
        ("notes.rs", "flush", 1, "Termurion's own notes file under %APPDATA%\\Termurion"),
        ("notes.rs", "stat_token", 1, "Termurion's own notes file under %APPDATA%\\Termurion"),
        ("notes.rs", "write", 7, "Termurion's own notes file under %APPDATA%\\Termurion"),
        ("open_quickly_ui.rs", "system_fonts", 1, "Windows' own Fonts folder"),
        ("probe.rs", "win_console_mouse", 1, "the test helper exe beside Termurion's own exe"),
        ("platform/windows/pty.rs", "console_mode_unless", 1, "the console device CONIN$, not a path"),
        ("restore_state.rs", "write_atomic", 3, "Termurion's own window-state file"),
        ("script_archive.rs", "hard_links", 1, "a folder Termurion just extracted an archive into: its own script staging folder, or the fresh folder it made beside a judged download"),
        ("script_archive.rs", "stage_entries", 7, "a folder Termurion just extracted an archive into: its own script staging folder, or the fresh folder it made beside a judged download"),
        ("script_new.rs", "write_files", 2, "a new script's files in Termurion's Scripts folder"),
        ("script_provision.rs", "download", 1, "Termurion's own runtime, download and environment folders under %APPDATA%"),
        ("script_provision.rs", "exists", 1, "Termurion's own runtime, download and environment folders under %APPDATA%"),
        ("script_provision.rs", "extract", 1, "Termurion's own runtime, download and environment folders under %APPDATA%"),
        ("script_provision.rs", "find_file", 1, "Termurion's own runtime, download and environment folders under %APPDATA%"),
        ("script_provision.rs", "list_dirs", 2, "Termurion's own runtime, download and environment folders under %APPDATA%"),
        ("script_provision.rs", "remove", 3, "Termurion's own runtime, download and environment folders under %APPDATA%"),
        ("script_provision.rs", "rename", 2, "Termurion's own runtime, download and environment folders under %APPDATA%"),
        ("script_provision.rs", "sha256", 1, "Termurion's own runtime, download and environment folders under %APPDATA%"),
        ("script_provision.rs", "system_tool", 1, "Windows' own System32 folder (GetSystemDirectory)"),
        ("script_provision.rs", "write", 2, "Termurion's own runtime, download and environment folders under %APPDATA%"),
        ("scripts.rs", "read_link", 1, "the judge's own reparse reader behind the Fs seam (netpath::judge calls it root first)"),
        ("scripts_scan.rs", "start_with", 1, "Termurion's Scripts folder (scripts_folder judges a custom one on every call)"),
        ("settings_keys.rs", "add_icon_font", 1, "Windows' own Fonts folder"),
        ("shell_history.rs", "write_whole", 2, "Termurion's own shell history file"),
        ("statusbar_setup_ui.rs", "add_symbol_font", 1, "Windows' own Fonts folder (constant paths)"),
        ("store.rs", "load", 1, "Termurion's own profiles file"),
        ("store.rs", "save", 3, "Termurion's own profiles file"),
        ("terminfo.rs", "ensure", 3, "Termurion's own terminfo folder under %APPDATA%\\Termurion"),
        ("util_console.rs", "open_console", 1, "the console devices CONIN$ and CONOUT$, not paths"),
        ("utilities.rs", "ensure", 5, "Termurion's own utilities folder under %APPDATA%\\Termurion"),
        ("utilities.rs", "refresh", 6, "Termurion's own utilities folder under %APPDATA%\\Termurion"),
        ("platform/windows/shell_open.rs", "open_url", 1, "URL targets only: platform::shell_open::open_with has semantic::shell_url refuse a path, a drive path, a network path and a file: link before ShellExecuteW; a file opens only through localfs::open_with_shell with a LocalPath"),
        ("app/semantic_ui.rs", "shell_open_scheme", 1, "URL targets only: link_scheme::scheme_of and screen judge it again (a path has no scheme, file: is on the blocked list) before ShellExecuteW"),
        ("utilities.rs", "stamp", 1, "Termurion's own utilities folder under %APPDATA%\\Termurion"),
        ("platform/windows/ipc.rs", "connect_owned", 1, "a named pipe, not a file path"),
    ];

    fn fn_name(line: &str) -> Option<String> {
        let t = line.trim_start();
        let t = ["pub(super) ", "pub(crate) ", "pub "].iter().find_map(|p| t.strip_prefix(p)).unwrap_or(t);
        let t = ["const ", "async ", "unsafe "].iter().fold(t, |t, p| t.strip_prefix(p).unwrap_or(t));
        let t = t.strip_prefix("extern \"system\" ").unwrap_or(t);
        let rest = t.strip_prefix("fn ")?;
        Some(rest.split(|c: char| !(c.is_alphanumeric() || c == '_')).next()?.to_string())
    }

    /// A line with the text inside its string literals blanked, so a message naming a call is not
    /// read as the call.
    fn blank_strings(line: &str) -> String {
        let chars: Vec<char> = line.chars().collect();
        let mut out = String::with_capacity(line.len());
        let mut i = 0;
        while i < chars.len() {
            let char_literal = i > 0 && chars[i - 1] == '\'' && chars.get(i + 1) == Some(&'\'');
            if chars[i] != '"' || char_literal {
                out.push(chars[i]);
                i += 1;
                continue;
            }
            // A raw string (`r"..."`, `r#"..."#`) has no escapes and ends at its quote and hashes.
            let hashes = chars[..i].iter().rev().take_while(|&&c| c == '#').count();
            let raw = i > hashes && chars[i - hashes - 1] == 'r';
            let hashes = if raw { hashes } else { 0 };
            out.push('"');
            i += 1;
            while i < chars.len() {
                if !raw && chars[i] == '\\' {
                    out.push_str("  ");
                    i += 2;
                    continue;
                }
                if chars[i] == '"' && chars[i + 1..].iter().take(hashes).filter(|&&c| c == '#').count() == hashes {
                    break;
                }
                out.push(' ');
                i += 1;
            }
            if i < chars.len() {
                out.push('"');
                i += 1;
            }
        }
        out
    }

    /// Every raw probe in one file's live code, as (function, line, text); the gate files have none.
    fn raw_probes(file: &str, text: &str) -> Vec<(String, usize, String)> {
        let live = live_code(text);
        let mut found = Vec::new();
        let mut current = String::from("(outside any function)");
        for (i, line) in live.lines().enumerate() {
            if let Some(name) = fn_name(line) {
                current = name;
            }
            let t = line.trim_start();
            if t.starts_with("//") {
                continue;
            }
            let bare = t.starts_with("use std::fs::") && BARE.iter().any(|f| t.split(|c: char| !(c.is_alphanumeric() || c == '_')).any(|w| w == *f));
            let code = blank_strings(line).replace("localfs::", "");
            let hits = PROBES.iter().map(|p| code.matches(p).count()).sum::<usize>() + usize::from(bare);
            for _ in 0..hits {
                found.push((current.clone(), i + 1, format!("{file}:{} {}", i + 1, t)));
            }
        }
        found
    }

    /// The raw probes of one file that no OWN entry covers, and the OWN entries whose count is off.
    fn scan_with(file: &str, text: &str, own: &[(&str, &str, usize, &str)]) -> Vec<String> {
        if GATE.contains(&file) {
            return Vec::new();
        }
        let probes = raw_probes(file, text);
        let mut wrong = Vec::new();
        let mut by_fn: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        for (f, _, at) in probes {
            by_fn.entry(f).or_default().push(at);
        }
        for (f, sites) in &by_fn {
            match own.iter().find(|(of, on, _, _)| *of == file && on == f) {
                Some((_, _, n, _)) if *n == sites.len() => {}
                Some((_, _, n, _)) => wrong.push(format!("{file} {f}: OWN lists {n} raw probes, the code has {}:\n  {}", sites.len(), sites.join("\n  "))),
                None => wrong.extend(sites.iter().map(|s| format!("{s}\n    in {f}: judge the path first (localfs::LocalPath::judge) or list it in OWN with the reason"))),
            }
        }
        for (of, on, _, _) in own.iter().filter(|(of, _, _, _)| *of == file) {
            if !by_fn.contains_key(*on) {
                wrong.push(format!("{of} {on} is in OWN but has no raw probe now: remove it"));
            }
        }
        wrong
    }

    fn scan(file: &str, text: &str) -> Vec<String> {
        scan_with(file, text, OWN)
    }

    /// Every `.rs` under `src`, as (path relative to `src` with `/`, text).
    fn sources() -> Vec<(String, String)> {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut out = Vec::new();
        let mut dirs = vec![src.clone()];
        while let Some(d) = dirs.pop() {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    dirs.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    let rel = p.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
                    out.push((rel, std::fs::read_to_string(&p).unwrap()));
                }
            }
        }
        out.sort();
        out
    }

    /// Round 3, item 2c: no raw probe anywhere in `src` outside the gate and the OWN list. A path
    /// from outside reaches the filesystem only as a `LocalPath`.
    #[test]
    fn every_path_probe_goes_through_the_judge() {
        let all = sources();
        assert!(all.len() > 400, "every source file is read ({} found)", all.len());
        let mut wrong: Vec<String> = all.iter().flat_map(|(f, t)| scan(f, t)).collect();
        for (of, _, _, why) in OWN {
            if !all.iter().any(|(f, _)| f == of) {
                wrong.push(format!("OWN names {of}, which does not exist"));
            }
            assert!(!why.trim().is_empty(), "every OWN entry has its reason");
        }
        assert!(wrong.is_empty(), "{} raw probes:\n{}", wrong.len(), wrong.join("\n"));
    }

    /// Round 3, item 2c: the guard itself is proven. Each planted file holds one unjudged probe the
    /// scanner must report; the clean ones hold none.
    #[test]
    fn the_guard_catches_planted_probes() {
        let planted: &[(&str, &str)] = &[
            ("a judge of another path earlier in the function", "fn f(a: &Path, b: &Path) {\n    if crate::localfs::exists(a) {}\n    let _ = b.is_dir();\n}\n"),
            ("after an early cfg(test) item", "use std::path::Path;\n#[cfg(test)]\nuse std::fmt;\nfn f(b: &Path) {\n    let _ = b.is_dir();\n}\n"),
            ("a probe before a judge on the same line", "fn f(p: &Path, q: &Path) {\n    let _ = p.is_dir() && crate::localfs::exists(q);\n}\n"),
            ("fs::read", "fn f(b: &Path) {\n    let _ = std::fs::read(b);\n}\n"),
            ("fs::read_to_string", "fn f(b: &Path) {\n    let _ = std::fs::read_to_string(b);\n}\n"),
            ("image::open", "fn f(b: &Path) {\n    let _ = image::open(b);\n}\n"),
            ("a working folder", "fn f(c: &mut std::process::Command, b: &Path) {\n    c.current_dir(b);\n}\n"),
            ("OpenOptions", "fn f(b: &Path) {\n    let _ = std::fs::OpenOptions::new().read(true).open(b);\n}\n"),
            ("an imported probe", "use std::fs::read_to_string;\nfn f(b: &Path) {\n    let _ = read_to_string(b);\n}\n"),
            ("after a test module", "fn g() {}\n#[cfg(test)]\nmod tests {\n    fn t() {}\n}\nfn f(b: &Path) {\n    let _ = b.exists();\n}\n"),
            ("a judge with a fake disk in live code", "fn f(b: &Path) {\n    let _ = LocalPath::judge_with(b, &|_| None, &LocalDrives);\n}\n"),
            ("a probe after a raw string ending in a backslash", "fn f(p: &Path) {\n    let _ = (r\"C:\\\", p.is_dir());\n}\n"),
            ("a probe after a char literal quote", "fn f(p: &Path) {\n    let _ = ('\"', p.exists());\n}\n"),
            ("a second probe in an OWN function", "fn own_one(b: &Path) {\n    let _ = b.is_file();\n    let _ = b.is_dir();\n}\n"),
            ("a cfg(any(windows, test)) item is live code", "#[cfg(any(windows, test))]\nfn f(b: &Path) {\n    let _ = b.is_dir();\n}\n"),
            ("fs::exists", "fn f(b: &Path) {\n    let _ = std::fs::exists(b);\n}\n"),
            ("ShellExecuteW", "fn f(b: &[u16]) {\n    unsafe { ShellExecuteW(0, w!(\"open\"), b.as_ptr(), null(), null(), 1) };\n}\n"),
            ("FindFirstFileW", "fn f(b: &[u16]) {\n    unsafe { FindFirstFileW(b.as_ptr(), &mut d) };\n}\n"),
            ("GetFileAttributesW", "fn f(b: &[u16]) {\n    unsafe { GetFileAttributesW(b.as_ptr()) };\n}\n"),
            ("set_permissions", "fn f(b: &Path, p: std::fs::Permissions) {\n    let _ = std::fs::set_permissions(b, p);\n}\n"),
        ];
        let own: &[(&str, &str, usize, &str)] = &[("planted.rs", "own_one", 1, "a test entry")];
        let missed: Vec<&str> = planted.iter().filter(|(_, text)| scan_with("planted.rs", text, if text.contains("fn own_one") { own } else { &[] }).is_empty()).map(|(what, _)| *what).collect();
        assert!(missed.is_empty(), "the guard missed: {missed:?}");
        let clean: &[&str] = &[
            "fn f(p: &LocalPath) {\n    let _ = crate::localfs::is_dir(p) && localfs::read_dir_ok(p);\n    let _ = crate::localfs::read(p);\n}\n",
            "fn f() {}\n#[cfg(test)]\nmod tests {\n    fn t(p: &Path) {\n        let _ = std::fs::read(p);\n        let s = \"}\";\n    }\n}\n",
            "#[cfg(all(test, windows))]\nfn t(p: &Path) -> bool {\n    p.is_dir()\n}\n",
            "#[test]\nfn t() {\n    assert!(std::path::Path::new(\"x\").exists());\n}\n",
            "fn f() {\n    let _ = format!(\"std::fs::read(x) and p.is_dir() in a message\");\n}\n",
        ];
        for text in clean {
            assert_eq!(scan_with("planted.rs", text, &[]), Vec::<String>::new(), "{text}");
        }
        assert_eq!(scan_with("planted.rs", "fn own_one(b: &Path) {\n    let _ = b.is_file();\n}\n", own), Vec::<String>::new(), "a listed probe");
    }

    /// Judge rulings r3, item 1: a path the person picks in a file dialog at that moment may be on a
    /// share (every Windows app connects there); every other path stays refused. Only dialog code
    /// (a file that opens an `rfd::FileDialog`) makes one.
    #[test]
    fn a_person_picked_path_may_be_on_a_share() {
        let share = Path::new(r"\\srv\share\notes.txt");
        assert!(LocalPath::judge(share).is_none(), "the judge still refuses it");
        let got = LocalPath::person_picked(share);
        assert_eq!(got.path(), share, "a picked path is the person's choice");
        assert!(LocalPath::person_picked(Path::new(r"Z:\x")).path().ends_with("x"));
    }

    /// Each function of one file's live code as (name, signature, body): a function runs from its
    /// `fn` line to the next one; its signature is the text up to its first `{`.
    fn functions_of(text: &str) -> Vec<(String, String, String)> {
        let live = live_code(text);
        let mut out: Vec<(String, String, String)> = Vec::new();
        for line in live.lines() {
            let code = blank_strings(line);
            if let Some(name) = fn_name(&code) {
                out.push((name, String::new(), String::new()));
            }
            if let Some((_, sig, body)) = out.last_mut() {
                if !body.contains('{') {
                    sig.push_str(code.split('{').next().unwrap_or(""));
                    sig.push(' ');
                }
                body.push_str(&code);
                body.push('\n');
            }
        }
        out
    }

    /// Judge round 5, item 1: the outside entry points (Explorer, the launch pipe, the command line)
    /// judge what they were given; only a dialog's own result is the person's pick (by type, the next
    /// tests).
    #[test]
    fn outside_entry_points_judge_their_paths() {
        let fns = |f: &str| functions_of(&sources().into_iter().find(|(n, _)| n == f).unwrap().1);
        let body = |f: &str, name: &str| fns(f).into_iter().find(|(n, _, _)| n == name).unwrap_or_else(|| panic!("{f} {name}")).2;
        assert!(body("app/arrange_ui.rs", "load_arrangement_path").contains("judge_io("), "an arrangement from outside is judged");
        assert!(body("app/replay_ui.rs", "import_recording_at").contains("judge_io("), "a recording from outside is judged");
        let opened = body("app/open_file_ui.rs", "opened");
        assert!(opened.contains("self.import_recording_at(") && opened.contains("self.load_arrangement_path("), "Explorer, the pipe and the command line use the judged entry points");
    }

    /// There is one device-name rule (`inert_files::is_device_name`; `scripts::is_device_name` and
    /// `zip_check::device_name` call it): no other live source lists the device names.
    #[test]
    fn there_is_one_device_name_rule() {
        let copies: Vec<String> = sources()
            .into_iter()
            .filter(|(f, t)| f != "inert_files.rs" && f != "util_console.rs" && live_code(t).contains("\"CONOUT$\""))
            .map(|(f, _)| f)
            .collect();
        assert!(copies.is_empty(), "a second list of device names: {copies:?}");
    }
}

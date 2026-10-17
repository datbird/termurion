//! THE judge of every path from outside before Termurion probes it, and which paths Semantic History
//! may probe (`NFM:17-70`, `SHC:86`). Any probe of a path on a share (`is_dir`, `metadata`, an open,
//! a folder listing) connects to it and sends the person's NTLM hash, so a path is judged with no
//! I/O on the network, in this order:
//! 1. text: a UNC or device-namespace path (`\\host`, `//host`, `\\?\UNC`, `\\.\`), a DOS device
//!    name (CON, NUL, COM1: a device may be redirected to a share), a relative path;
//! 2. the drive letter's device-map target (`QueryDosDeviceW`, an object-manager query): a SUBST
//!    letter (`\??\C:\dir`) is replaced by its target and judged again; ANY other target that is not
//!    a local volume (`\??\UNC\..`, `\Device\Mup\..`, `\Device\LanmanRedirector\..`, anything not
//!    `\Device\HarddiskVolume*`, `\Device\CdRom*` or `\Device\Floppy*`) is network;
//! 3. `GetDriveTypeW`, asked only for a letter step 2 found to be a local volume (it opens the root,
//!    so it is never asked for a SUBST or redirected letter);
//! 4. every folder from the root down, then the leaf, reads its reparse DATA with the reparse
//!    point itself opened (`FILE_FLAG_OPEN_REPARSE_POINT`, nothing followed) on components already
//!    found local; a link's target is judged again from step 1 (root first) before anything below
//!    the link is read, and a reparse point Termurion cannot read is refused.
//! Semantic History's ignore list and `EnableSemanticHistoryOnNetworkMounts` add to this (mapped
//! drives allowed only when the setting says so); a UNC path is never touched at all.

use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The drive letters' device map and drive types.
pub trait Drives {
    /// `QueryDosDeviceW("X:")`: the letter's current target (`\Device\HarddiskVolume3`, `\??\C:\dir`
    /// for a SUBST, `\Device\LanmanRedirector\;Z:..\h\s` for a mapped drive); None when the letter is
    /// not defined. An object-manager query: no I/O.
    fn dos_device(&self, letter: char) -> Option<String>;
    /// `GetDriveTypeW("X:\")`. Asked only for a letter whose device is a local volume.
    fn drive_type(&self, letter: char) -> u32;
}

/// Every drive is a local volume: the non-Windows and default answer.
pub struct LocalDrives;

impl Drives for LocalDrives {
    fn dos_device(&self, _letter: char) -> Option<String> {
        Some(r"\Device\HarddiskVolume1".to_string())
    }
    fn drive_type(&self, _letter: char) -> u32 {
        DRIVE_FIXED
    }
}

/// Tests: the letters given are mapped network drives, every other letter a local volume.
#[cfg(test)]
pub struct MappedLetters(pub &'static [char]);

#[cfg(test)]
impl Drives for MappedLetters {
    fn dos_device(&self, letter: char) -> Option<String> {
        Some(if self.0.contains(&letter.to_ascii_uppercase()) { format!(r"\Device\LanmanRedirector\;{letter}:0000000000012345\srv\share") } else { r"\Device\HarddiskVolume1".to_string() })
    }
    fn drive_type(&self, letter: char) -> u32 {
        if self.0.contains(&letter.to_ascii_uppercase()) { DRIVE_REMOTE } else { DRIVE_FIXED }
    }
}

/// `GetDriveTypeW`'s `DRIVE_FIXED`, which is NOT kept: a USB disk reports it, and once unplugged its
/// letter can be `net use`d, so a fixed answer is asked again on every call (the call is cheap).
const DRIVE_FIXED: u32 = 3;
/// `DRIVE_REMOTE`.
const DRIVE_REMOTE: u32 = 4;

/// Whether a `GetDriveTypeW` answer is a local drive: FIXED, REMOVABLE, CDROM or RAMDISK. REMOTE,
/// UNKNOWN (0) and NO_ROOT_DIR (1) are not (judge rulings r3, item 2).
fn drive_local(kind: u32) -> bool {
    matches!(kind, 2 | 3 | 5 | 6)
}

/// `remote_answer` with the real answer kept for the caller: REMOTE once seen stays REMOTE.
#[cfg(any(windows, test))]
fn drive_answer(cache: &std::sync::Mutex<[Option<bool>; 26]>, letter: usize, ask: impl FnOnce() -> u32) -> u32 {
    let mut kind = 0;
    let remote = remote_answer(cache, letter, || {
        kind = ask();
        kind
    });
    if remote { DRIVE_REMOTE } else { kind }
}

/// Whether drive `letter` (0 for A) is a network drive. Only a REMOTE answer is kept for the run (the
/// safe side: a letter once seen as a share stays refused); every other answer is asked again.
#[cfg(any(windows, test))]
fn remote_answer(cache: &std::sync::Mutex<[Option<bool>; 26]>, letter: usize, ask: impl FnOnce() -> u32) -> bool {
    if cache.lock().ok().and_then(|c| c[letter]) == Some(true) {
        return true;
    }
    let remote = ask() == DRIVE_REMOTE;
    if remote
        && let Ok(mut c) = cache.lock() {
            c[letter] = Some(true);
        }
    remote
}

/// `QueryDosDeviceW` and `GetDriveTypeW`.
#[cfg(windows)]
pub struct RealDrives;

#[cfg(windows)]
impl Drives for RealDrives {
    fn dos_device(&self, letter: char) -> Option<String> {
        use windows_sys::Win32::Storage::FileSystem::QueryDosDeviceW;
        if !letter.is_ascii_alphabetic() {
            return None;
        }
        let name: Vec<u16> = format!("{letter}:").encode_utf16().chain(Some(0)).collect();
        // The answer is a list of strings, the first the current mapping; a SUBST target can be a
        // long path, so a short buffer is retried once with the largest one.
        for len in [1024usize, 32 * 1024] {
            let mut buf = vec![0u16; len];
            // SAFETY: `name` is NUL-terminated and `buf` is writable for its length.
            let n = unsafe { QueryDosDeviceW(name.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
            if n == 0 {
                if std::io::Error::last_os_error().raw_os_error() == Some(windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER as i32) {
                    continue;
                }
                return None;
            }
            let n = n.min(buf.len());
            let end = buf[..n].iter().position(|&c| c == 0).unwrap_or(n);
            return Some(String::from_utf16_lossy(&buf[..end]));
        }
        None
    }

    fn drive_type(&self, letter: char) -> u32 {
        use std::sync::Mutex;
        use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
        static CACHE: Mutex<[Option<bool>; 26]> = Mutex::new([None; 26]);
        if !letter.is_ascii_alphabetic() {
            return 0;
        }
        let index = (letter.to_ascii_uppercase() as u8 - b'A') as usize;
        drive_answer(&CACHE, index, || {
            let wide: Vec<u16> = format!("{letter}:\\").encode_utf16().chain(Some(0)).collect();
            // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call.
            unsafe { GetDriveTypeW(wide.as_ptr()) }
        })
    }
}

/// The drive answers for this platform.
pub fn real_drives() -> Arc<dyn Drives + Send + Sync> {
    #[cfg(windows)]
    {
        Arc::new(RealDrives)
    }
    #[cfg(not(windows))]
    {
        Arc::new(LocalDrives)
    }
}

/// What a reparse point Termurion cannot read, or one whose tag may send an open elsewhere (HSM, DFS,
/// a third-party filter), is answered as: a link to a network path, so the judge refuses it.
pub const UNREADABLE_LINK: &str = r"\\?\UNC\unreadable-reparse-point";

/// `p`'s own reparse data, read with the reparse point itself opened (`FILE_FLAG_OPEN_REPARSE_POINT`
/// with no data access, so nothing is followed): None when `p` is not a reparse point (or does not
/// exist), the target of a symbolic link or junction (relative as stored), `UNREADABLE_LINK` for a
/// tag that may redirect elsewhere or any failure to read. Called only on a path whose every folder
/// the judge has found local.
#[cfg(windows)]
pub fn read_reparse(p: &Path) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, GetLastError, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FileAttributeTagInfo, GetFileInformationByHandleEx, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;
    /// `FSCTL_GET_REPARSE_POINT` (winioctl.h).
    const FSCTL_GET_REPARSE_POINT: u32 = 0x0009_00A8;
    let wide: Vec<u16> = p.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: `wide` is NUL-terminated; no security attributes or template.
    let h = unsafe {
        CreateFileW(wide.as_ptr(), 0, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, std::ptr::null(), OPEN_EXISTING, FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS, std::ptr::null_mut())
    };
    if h == INVALID_HANDLE_VALUE {
        // SAFETY: reads the calling thread's last error.
        let e = unsafe { GetLastError() };
        // Nothing there: nothing to follow (the probe after the judge finds nothing either).
        return if e == ERROR_FILE_NOT_FOUND || e == ERROR_PATH_NOT_FOUND { None } else { Some(PathBuf::from(UNREADABLE_LINK)) };
    }
    let answer = (|| {
        let mut info = FILE_ATTRIBUTE_TAG_INFO { FileAttributes: 0, ReparseTag: 0 };
        // SAFETY: `h` is open and `info` is writable for its size.
        if unsafe { GetFileInformationByHandleEx(h, FileAttributeTagInfo, &mut info as *mut _ as *mut _, std::mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32) } == 0 {
            return Some(PathBuf::from(UNREADABLE_LINK));
        }
        if info.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT == 0 {
            return None;
        }
        match reparse_kind(info.ReparseTag) {
            Reparse::Inert => return None,
            Reparse::Opaque => return Some(PathBuf::from(UNREADABLE_LINK)),
            Reparse::Link => {}
        }
        let mut buf = vec![0u8; 16 * 1024];
        let mut got = 0u32;
        // SAFETY: `h` is open; `buf` is writable for its length; no overlapped I/O.
        let ok = unsafe { DeviceIoControl(h, FSCTL_GET_REPARSE_POINT, std::ptr::null(), 0, buf.as_mut_ptr().cast(), buf.len() as u32, &mut got, std::ptr::null_mut()) };
        if ok == 0 {
            return Some(PathBuf::from(UNREADABLE_LINK));
        }
        Some(link_target(&buf[..(got as usize).min(buf.len())]).unwrap_or_else(|| PathBuf::from(UNREADABLE_LINK)))
    })();
    // SAFETY: `h` was opened above and is closed once.
    unsafe { CloseHandle(h) };
    answer
}

/// Off Windows: the standard link read.
#[cfg(not(windows))]
pub fn read_reparse(p: &Path) -> Option<PathBuf> {
    std::fs::read_link(p).ok()
}

/// `IO_REPARSE_TAG_MOUNT_POINT` (a junction or a volume mount point).
const TAG_MOUNT_POINT: u32 = 0xA000_0003;
/// `IO_REPARSE_TAG_SYMLINK`.
const TAG_SYMLINK: u32 = 0xA000_000C;

#[derive(Debug, PartialEq, Eq)]
enum Reparse {
    /// A symbolic link or junction: its target is read and judged.
    Link,
    /// A tag whose filter keeps the data on this machine (or that Win32 never follows).
    Inert,
    /// Anything else: its filter may fetch the file from elsewhere (HSM, DFS, a third-party tag).
    Opaque,
}

/// The reparse tags Termurion knows: the two link tags, and the ones whose data stays local (dedup,
/// compression, cloud placeholders and their sync root, app execution aliases, projected folders,
/// container layers, WSL special files). Every other tag is opaque.
fn reparse_kind(tag: u32) -> Reparse {
    const INERT: &[u32] = &[
        0x8000_0013, // DEDUP
        0x8000_0017, // WOF
        0x8000_0018, // WCI
        0x9000_1018, // WCI_1
        0xA000_001F, // WCI_TOMBSTONE
        0xA000_0027, // WCI_LINK
        0x8000_001B, // APPEXECLINK
        0x9000_001C, // PROJFS
        0xA000_001D, // LX_SYMLINK (Win32 never follows it)
        0x8000_0021, // ONEDRIVE
        0x8000_0023, // AF_UNIX
        0x8000_0024, // LX_FIFO
        0x8000_0025, // LX_CHR
        0x8000_0026, // LX_BLK
    ];
    match tag {
        TAG_SYMLINK | TAG_MOUNT_POINT => Reparse::Link,
        // IO_REPARSE_TAG_CLOUD and CLOUD_1..F (the high nibble of the low word varies).
        t if t & 0xFFFF_0FFF == 0x9000_001A => Reparse::Inert,
        t if INERT.contains(&t) => Reparse::Inert,
        _ => Reparse::Opaque,
    }
}

/// The substitute name of a symbolic link's or junction's `REPARSE_DATA_BUFFER`: `\??\` written as
/// `\\?\` (so a path reads as Win32 does), a relative symbolic link's target as stored. None for a
/// buffer that does not parse.
fn link_target(buf: &[u8]) -> Option<PathBuf> {
    let u16_at = |i: usize| buf.get(i..i + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize);
    let u32_at = |i: usize| buf.get(i..i + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let tag = u32_at(0)?;
    // Header (8 bytes), then SubstituteNameOffset, SubstituteNameLength, PrintNameOffset,
    // PrintNameLength (2 bytes each), then a symbolic link's Flags (4), then the path buffer.
    let (offset, len) = (u16_at(8)?, u16_at(10)?);
    let (path_buffer, relative) = match tag {
        TAG_SYMLINK => (20, u32_at(16)? & 1 == 1),
        TAG_MOUNT_POINT => (16, false),
        _ => return None,
    };
    let bytes = buf.get(path_buffer + offset..path_buffer + offset + len)?;
    let wide: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let name = String::from_utf16(&wide).ok()?;
    if name.is_empty() {
        return None;
    }
    if relative {
        return Some(PathBuf::from(name));
    }
    Some(PathBuf::from(match name.strip_prefix(r"\??\") {
        Some(rest) => format!(r"\\?\{rest}"),
        None => name,
    }))
}

/// What Semantic History may probe: the settings `PathsToIgnore` and
/// `EnableSemanticHistoryOnNetworkMounts`, and the drive answers.
#[derive(Clone)]
pub struct Probe {
    pub ignore: Vec<String>,
    pub allow_network: bool,
    pub drives: Arc<dyn Drives + Send + Sync>,
}

impl Default for Probe {
    fn default() -> Probe {
        Probe { ignore: Vec::new(), allow_network: false, drives: Arc::new(LocalDrives) }
    }
}

impl std::fmt::Debug for Probe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Probe").field("ignore", &self.ignore).field("allow_network", &self.allow_network).finish()
    }
}

impl Probe {
    /// `PathsToIgnore` split on "," (empty entries dropped, nothing trimmed, as iTerm2).
    pub fn from_settings(paths_to_ignore: &str, allow: bool, drives: Arc<dyn Drives + Send + Sync>) -> Probe {
        Probe { ignore: paths_to_ignore.split(',').filter(|s| !s.is_empty()).map(String::from).collect(), allow_network: allow, drives }
    }
}

/// `/` read as `\`, case ignored (Windows paths; Decision).
fn normal(s: &str) -> String {
    s.replace('/', "\\").to_lowercase()
}

/// Whether `path` is under one of the folders (`NFM:48-70`): each entry gets a trailing `\`
/// unless it has one, so `D:\slow` hides `D:\slow\a.txt` but not `D:\slower`, and not
/// the folder itself.
pub fn has_ignored_prefix(path: &str, ignore: &[String]) -> bool {
    let path = normal(path);
    ignore.iter().any(|entry| {
        let mut e = normal(entry);
        if !e.ends_with('\\') {
            e.push('\\');
        }
        path.starts_with(&e)
    })
}

/// What one click may spend on probing: the shared count of `exists` calls and link reads, and
/// the folders already found to stay local (each folder's link is read once per click).
pub struct Budget {
    left: std::cell::Cell<usize>,
    verified: std::cell::RefCell<std::collections::HashSet<std::path::PathBuf>>,
}

impl Budget {
    pub fn new(n: usize) -> Budget {
        Budget { left: std::cell::Cell::new(n), verified: Default::default() }
    }

    pub fn left(&self) -> usize {
        self.left.get()
    }

    /// Takes one probe; false when none is left.
    pub fn spend(&self) -> bool {
        match self.left.get() {
            0 => false,
            n => {
                self.left.set(n - 1);
                #[cfg(test)]
                crate::semantic::count_probe();
                true
            }
        }
    }
}

/// `p` without a long-path prefix (`\\?\` or `\??\`, either separator) before a drive letter, so
/// the ignore list and the drive check read `Z:\...` however it was spelled.
fn without_long_prefix(p: &str) -> &str {
    let b = p.as_bytes();
    let sep = |c: u8| c == b'\\' || c == b'/';
    if b.len() >= 6 && sep(b[0]) && (sep(b[1]) || b[1] == b'?') && b[2] == b'?' && sep(b[3]) && b[4].is_ascii_alphabetic() && b[5] == b':' {
        &p[4..]
    } else {
        p
    }
}

/// What one judgement reads: the ignore list, whether network mounts are allowed, the drive answers.
struct Rule<'a> {
    ignore: &'a [String],
    allow_network: bool,
    drives: &'a dyn Drives,
}

/// Step 1, text only: not a network or device-namespace path, not a DOS device name, absolute
/// (a relative path is read against a working folder that may itself be a share), and (unless network
/// mounts are allowed, `NFM:20-23`) not under an ignored folder.
fn text_local(path: &str, r: &Rule) -> bool {
    if crate::scripts::is_network_path(path) || crate::scripts::is_device_name(Path::new(path)) {
        return false;
    }
    let p = without_long_prefix(path);
    let b = p.as_bytes();
    let absolute = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/');
    #[cfg(not(windows))]
    let absolute = absolute || b.first() == Some(&b'/');
    absolute && (r.allow_network || !has_ignored_prefix(p, r.ignore))
}

/// What a drive letter is, from its device-map target alone.
#[derive(Debug, PartialEq, Eq)]
enum Letter {
    /// A local volume (`\Device\HarddiskVolume*`, `\Device\CdRom*`, `\Device\Floppy*`).
    Volume,
    /// A SUBST letter onto this path on another letter.
    Subst(String),
    /// A mapped network drive, or any other redirector or device.
    Network,
    /// A SUBST or raw `DefineDosDevice` target that is not a drive-letter path (`\??\UNC\..`).
    Elsewhere,
    /// No such letter.
    Missing,
}

fn letter_kind(target: Option<String>) -> Letter {
    let Some(t) = target else { return Letter::Missing };
    if let Some(rest) = t.strip_prefix(r"\??\") {
        let b = rest.as_bytes();
        let plain = b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b.len() == 2 || b[2] == b'\\');
        return if plain { Letter::Subst(rest.to_string()) } else { Letter::Elsewhere };
    }
    // Any device that is not a redirector may be a local volume (HarddiskVolume, CdRom, a RAM disk,
    // a VeraCrypt volume); `GetDriveTypeW` then has the last word (REMOTE, UNKNOWN and NO_ROOT_DIR refuse).
    let name = t.strip_prefix(r"\Device\").or_else(|| t.get(..8).filter(|p| p.eq_ignore_ascii_case(r"\Device\")).map(|_| &t[8..]));
    // A local volume's target is the device alone; a mapped, RDP or VirtualBox drive names a share after it.
    let redirector = match name {
        Some(n) => n.is_empty() || n.contains('\\') || n.eq_ignore_ascii_case("Mup") || n.to_ascii_lowercase().ends_with("redirector"),
        None => true,
    };
    let volume = !redirector;
    if volume { Letter::Volume } else { Letter::Network }
}

/// THE judge of every path that comes from outside (a file or folder another program named, a
/// document, saved state, a setting) before Termurion calls `is_dir`, `exists`, `metadata` or opens it
/// (even `is_dir` on a share path connects to it). Steps 1-4 of the module comment; nothing is read
/// on the network and nothing below a link is read before the link itself.
pub fn judge(path: &Path, read_link: &dyn Fn(&Path) -> Option<PathBuf>, drives: &dyn Drives) -> bool {
    local_chain(path, &Rule { ignore: &[], allow_network: false, drives }, read_link, &Budget::new(PROBE_LIMIT), 0)
}

/// Whether a path may be probed on disk by Semantic History: the judge with the ignore list and the
/// network-mounts setting, every read taken from `budget` (B2: a folder that is a link to a share).
pub fn may_probe_with(path: &str, p: &Probe, read_link: &dyn Fn(&Path) -> Option<PathBuf>, budget: &Budget) -> bool {
    local_chain(Path::new(path), &Rule { ignore: &p.ignore, allow_network: p.allow_network, drives: &*p.drives }, read_link, budget, 0)
}

fn local_chain(path: &Path, r: &Rule, read_link: &dyn Fn(&Path) -> Option<PathBuf>, budget: &Budget, depth: usize) -> bool {
    const HOPS: usize = 8;
    let text = path.to_string_lossy();
    // 1. Text.
    if depth > HOPS || !text_local(&text, r) {
        return false;
    }
    // 2. The letter's device map, before anything is asked of the drive itself.
    let p = without_long_prefix(&text);
    let letter = p.as_bytes()[0] as char;
    match letter_kind(r.drives.dos_device(letter)) {
        Letter::Subst(target) => {
            let rest = p[2..].trim_start_matches(['\\', '/']);
            let base = target.trim_end_matches('\\');
            let real = if rest.is_empty() { format!("{base}\\") } else { format!("{base}\\{rest}") };
            return local_chain(Path::new(&real), r, read_link, budget, depth + 1);
        }
        Letter::Elsewhere | Letter::Missing => return false,
        Letter::Network if !r.allow_network => return false,
        Letter::Network => {}
        // 3. The drive type, only for a local volume.
        Letter::Volume => {
            if !r.allow_network && !drive_local(r.drives.drive_type(letter)) {
                return false;
            }
        }
    }
    // 4. Every folder root first, each one's reparse data before anything below it. Win32 collapses
    // `.` and `..` lexically before any link is resolved (RtlGetFullPathName_U), so the walk runs on
    // the collapsed path; a verbatim path is not collapsed by Win32, so a step there is refused.
    // After a link, a relative target is joined onto the parent as already redirected, and the rest
    // of the path (no `..` left in it) continues below the resolved target: no component of a
    // redirected path is ever popped.
    let steps = path.components().any(|c| matches!(c, std::path::Component::CurDir | std::path::Component::ParentDir));
    if steps && p.len() != text.len() {
        return false;
    }
    let collapsed = normalize(path);
    let mut resolved = PathBuf::new();
    for comp in collapsed.components() {
        match comp {
            std::path::Component::Prefix(_) => {
                resolved.push(comp.as_os_str());
                continue;
            }
            // `normalize` left none of these.
            std::path::Component::CurDir | std::path::Component::ParentDir => return false,
            _ => resolved.push(comp.as_os_str()),
        }
        let at = resolved.clone();
        if budget.verified.borrow().contains(&at) {
            continue;
        }
        if !budget.spend() {
            return false;
        }
        if let Some(target) = read_link(&at) {
            let parent = at.parent().map(Path::to_path_buf);
            let target = match parent {
                Some(parent) if target.is_relative() => normalize(&parent.join(target)),
                _ => target,
            };
            if !local_chain(&target, r, read_link, budget, depth + 1) {
                return false;
            }
            // The rest of the path is below the link's target.
            resolved = target;
        }
        budget.verified.borrow_mut().insert(at);
    }
    true
}

/// `p` with `.` and `..` collapsed lexically (never above the root).
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if out.parent().is_some() && !matches!(out.components().next_back(), Some(std::path::Component::RootDir | std::path::Component::Prefix(_))) {
                    out.pop();
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `may_probe_with` over the real links with a budget of its own.
pub fn may_probe(path: &str, p: &Probe) -> bool {
    may_probe_with(path, p, &read_reparse, &Budget::new(PROBE_LIMIT))
}

/// Reads a lone caller (no click budget) may spend on one path.
const PROBE_LIMIT: usize = 64;

#[cfg(test)]
mod tests {
    use super::*;

    /// `Z:\` is a mapped network drive.
    #[allow(non_upper_case_globals)]
    const FakeDrives: MappedLetters = MappedLetters(&['Z']);

    fn local() -> Arc<dyn Drives + Send + Sync> {
        Arc::new(LocalDrives)
    }

    #[test]
    fn netpath_follows_iterm2() {
        let p = Probe::from_settings(r"D:\slow,,E:\x", false, local());
        assert_eq!(p.ignore, vec![r"D:\slow".to_string(), r"E:\x".to_string()]);
        assert!(has_ignored_prefix(r"D:\slow\a.txt", &p.ignore));
        assert!(has_ignored_prefix(r"d:/SLOW/a.txt", &p.ignore));
        assert!(!has_ignored_prefix(r"D:\slower\a.txt", &p.ignore));
        assert!(!has_ignored_prefix(r"D:\slow", &p.ignore));
        let spaced = Probe::from_settings(r" D:\slow", false, local());
        assert!(!has_ignored_prefix(r"D:\slow\a.txt", &spaced.ignore));
    }

    /// Final wave minor 10: a drive that was unmapped when first asked is asked again.
    #[test]
    fn a_drive_mapped_later_is_seen() {
        let cache = std::sync::Mutex::new([None; 26]);
        let asked = std::cell::Cell::new(0);
        let asked = &asked;
        let ask = |kind: u32| {
            move || {
                asked.set(asked.get() + 1);
                kind
            }
        };
        assert!(!remote_answer(&cache, 25, ask(1)), "no root directory yet");
        assert!(remote_answer(&cache, 25, ask(DRIVE_REMOTE)), "mapped later in the session");
        assert!(remote_answer(&cache, 25, ask(1)), "a network answer is kept");
        assert_eq!(asked.get(), 2, "the kept answer asked nothing");
        // A fixed answer is NOT kept: a USB disk reports it, and its letter can be mapped later.
        assert!(!remote_answer(&cache, 2, ask(DRIVE_FIXED)), "fixed");
        assert!(remote_answer(&cache, 2, ask(DRIVE_REMOTE)), "the same letter, now a share");
    }

    #[test]
    fn mapped_drives_are_not_probed() {
        let p = Probe { drives: Arc::new(FakeDrives), ..Probe::default() };
        assert!(!may_probe(r"Z:\share\f.txt", &p));
        assert!(may_probe(r"C:\f.txt", &p));
        let allow = Probe { allow_network: true, ..p };
        assert!(may_probe(r"Z:\share\f.txt", &allow));
    }

    #[test]
    fn unc_is_never_probed_even_when_allowed() {
        let allow = Probe { allow_network: true, ..Probe::default() };
        for path in [r"\\srv\s\f", "//srv/s/f", r"/\srv\s\f", r"\/srv/s/f", r"\\?\UNC\srv\s\f", r"\??\UNC\srv\s\f"] {
            assert!(!may_probe(path, &allow), "{path}");
            assert!(!may_probe(path, &Probe::default()), "{path}");
        }
        // The ignore list is skipped when network mounts are allowed (`NFM:20-23`).
        let ignoring = Probe::from_settings(r"C:\ign", true, local());
        assert!(may_probe(r"C:\ign\a.txt", &ignoring));
        assert!(!may_probe(r"C:\ign\a.txt", &Probe::from_settings(r"C:\ign", false, local())));
    }

    use std::cell::RefCell;
    use std::path::PathBuf;

    /// Links by path (lower case, `\`); every read is recorded.
    fn links<'a>(map: &'static [(&'static str, &'static str)], seen: &'a RefCell<Vec<String>>) -> impl Fn(&Path) -> Option<PathBuf> + 'a {
        move |p: &Path| {
            let key = p.to_string_lossy().to_lowercase();
            seen.borrow_mut().push(key.clone());
            map.iter().find(|(k, _)| *k == key).map(|(_, v)| PathBuf::from(v))
        }
    }

    #[test]
    fn the_cheap_checks_run_before_any_link_read() {
        let seen = RefCell::new(Vec::new());
        let rl = links(&[], &seen);
        let p = Probe::from_settings(r"C:\ign", false, Arc::new(FakeDrives));
        let b = Budget::new(50);
        assert!(!may_probe_with(r"C:\ign\a.txt", &p, &rl, &b));
        assert!(!may_probe_with(r"Z:\x\y.txt", &p, &rl, &b));
        assert!(!may_probe_with(r"\\srv\s\f", &p, &rl, &b));
        assert!(seen.borrow().is_empty(), "no link was read: {:?}", seen.borrow());
        assert_eq!(b.left(), 50);
    }

    #[test]
    fn long_path_spellings_get_the_same_checks() {
        let p = Probe::from_settings(r"C:\ign", false, Arc::new(FakeDrives));
        for path in [r"\\?\Z:\share\f.txt", r"\??\Z:\share\f.txt", r"//?/z:/share/f.txt", r"\\?\C:\ign\a.txt", r"\??\c:\IGN\a.txt"] {
            assert!(!may_probe(path, &p), "{path}");
        }
        assert!(may_probe(r"\\?\C:\f.txt", &p));
        assert!(may_probe(r"\\?\Z:\share\f.txt", &Probe { allow_network: true, ..p }));
    }

    #[test]
    fn ancestor_links_are_followed_root_first_and_checked() {
        let p = Probe { drives: Arc::new(FakeDrives), ..Probe::default() };
        let map: &'static [(&str, &str)] = &[(r"c:\proj\unc", r"\\srv\s"), (r"c:\proj\mapped", r"Z:\"), (r"c:\proj\ok", r"C:\other"), (r"c:\proj\rel", "ok")];
        let seen = RefCell::new(Vec::new());
        let rl = links(map, &seen);
        assert!(!may_probe_with(r"C:\proj\unc\x.txt", &p, &rl, &Budget::new(50)));
        assert!(!seen.borrow().iter().any(|s| s.ends_with("x.txt")), "the leaf was read under a link: {:?}", seen.borrow());
        assert!(!may_probe_with(r"C:\proj\mapped\sub\x.txt", &p, &rl, &Budget::new(50)));
        assert!(may_probe_with(r"C:\proj\ok\x.txt", &p, &rl, &Budget::new(50)));
        assert!(may_probe_with(r"C:\proj\rel\x.txt", &p, &rl, &Budget::new(50)));
        // The leaf itself may be the link.
        assert!(!may_probe_with(r"C:\proj\mapped", &p, &rl, &Budget::new(50)));
    }

    #[test]
    fn link_reads_count_toward_the_budget() {
        let seen = RefCell::new(Vec::new());
        let rl = links(&[], &seen);
        let p = Probe::default();
        let b = Budget::new(3);
        assert!(!may_probe_with(r"C:\a\b\c\d\e.txt", &p, &rl, &b), "six folders and files, three reads");
        assert_eq!(seen.borrow().len(), 3);
        // Folders already found local are not read again.
        let b = Budget::new(10);
        assert!(may_probe_with(r"C:\a\b\x.txt", &p, &rl, &b));
        let before = b.left();
        assert!(may_probe_with(r"C:\a\b\y.txt", &p, &rl, &b));
        assert_eq!(before - b.left(), 1, "only the new file is read");
    }

    #[cfg(windows)]
    #[test]
    fn real_drives_answer() {
        let c = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()).chars().next().unwrap();
        assert_eq!(letter_kind(RealDrives.dos_device(c)), Letter::Volume, "{:?}", RealDrives.dos_device(c));
        assert_ne!(RealDrives.drive_type(c), DRIVE_REMOTE);
    }

    /// Task 3 fix round 2, item 2: a SUBST letter is resolved (QueryDosDeviceW) and its target judged,
    /// every folder of a link target is walked root first, the leaf never read through a link.
    /// Every drive-type question is recorded (round 3: never asked before the letter is resolved).
    struct Subst(std::cell::RefCell<Vec<char>>);
    impl Subst {
        fn new() -> Subst {
            Subst(Default::default())
        }
    }
    impl Drives for Subst {
        fn dos_device(&self, letter: char) -> Option<String> {
            Some(match letter.to_ascii_uppercase() {
                'Z' => r"\Device\LanmanRedirector\;Z:0000000000012345\srv\share",
                'Y' => r"\??\UNC\h\s",
                'X' => r"\??\C:\ok",
                'W' => r"\??\Z:\x",
                'V' => r"\??\V:\loop",
                'U' => r"\??\C:\j",
                'M' => r"\Device\Mup\h\s",
                'L' => r"\Device\LanmanRedirector\h\s",
                'K' => r"\Device\VBoxMiniRdr\;K:\vboxsvr\s",
                'Q' => r"\??\Volume{0b1c2d3e-0000-0000-0000-100000000000}\",
                'R' => r"\??\GLOBALROOT\Device\Mup\h\s",
                'N' => return None,
                'E' => r"\Device\CdRom0",
                _ => r"\Device\HarddiskVolume3",
            }.to_string())
        }
        fn drive_type(&self, letter: char) -> u32 {
            self.0.borrow_mut().push(letter.to_ascii_uppercase());
            DRIVE_FIXED
        }
    }

    #[test]
    fn subst_letters_are_resolved_and_judged() {
        let none = |_: &Path| None;
        let d = Subst::new();
        assert!(judge(Path::new(r"C:\a\f"), &none, &d));
        assert!(!judge(Path::new(r"Y:\a\f"), &none, &d), "a letter onto a UNC target");
        assert!(judge(Path::new(r"X:\a\f"), &none, &d), "a letter onto a local folder");
        assert!(!judge(Path::new(r"W:\a"), &none, &d), "a letter onto a mapped drive");
        assert!(!judge(Path::new(r"V:\a"), &none, &d), "a loop ends");
        // The target is itself judged for links: U: is C:\j, a junction to a share.
        let read = |p: &Path| (p == Path::new(r"C:\j")).then(|| PathBuf::from(r"\\attacker\share"));
        assert!(!judge(Path::new(r"U:\f"), &read, &d));
    }

    #[test]
    fn a_link_targets_own_folders_are_walked_root_first() {
        // C:\j -> D:\x\y and D:\x -> \\attacker\share: D:\x must be read before D:\x\y is.
        let seen = RefCell::new(Vec::new());
        let map: &'static [(&str, &str)] = &[(r"c:\j", r"D:\x\y"), (r"d:\x", r"\\attacker\share")];
        let rl = links(map, &seen);
        assert!(!judge(Path::new(r"C:\j\f.txt"), &rl, &Subst::new()));
        let reads = seen.borrow().clone();
        let x = reads.iter().position(|r| r == r"d:\x").expect("the folder D: x was read");
        assert!(reads.iter().position(|r| r == r"d:\x\y").is_none_or(|y| x < y), "root first: {reads:?}");
        assert!(!reads.iter().any(|r| r.ends_with("f.txt")), "{reads:?}");
    }

    /// Judge round 3 re-review, Important 1: a relative link target is joined onto the path already
    /// redirected by earlier links, as Windows resolves it, and the walk goes on from there. With
    /// C:\r\d -> x\y and C:\r\d\l -> ..\z the real place is C:\r\x\z (not the lexical C:\r\z), and a
    /// link there to a share must be found before anything below it is read.
    #[test]
    fn a_chain_of_relative_links_is_walked_where_windows_resolves_it() {
        let seen = RefCell::new(Vec::new());
        let map: &'static [(&str, &str)] = &[(r"c:\r\d", r"x\y"), (r"c:\r\x\y\l", r"..\z"), (r"c:\r\x\z", r"\\attacker\share")];
        let rl = links(map, &seen);
        assert!(!judge(Path::new(r"C:\r\d\l\f"), &rl, &Subst::new()), "the chain ends on a share");
        let reads = seen.borrow().clone();
        assert!(reads.iter().any(|r| r == r"c:\r\x\z"), "the redirected place was read: {reads:?}");
        assert!(!reads.iter().any(|r| r == r"c:\r\z" || r.ends_with(r"\f")), "nothing lexical or below the share link was read: {reads:?}");
        // A chain that stays local is accepted, and every component read is on the resolved path.
        let seen = RefCell::new(Vec::new());
        let map: &'static [(&str, &str)] = &[(r"c:\r\d", r"x\y"), (r"c:\r\x\y\l", r"..\z")];
        let rl = links(map, &seen);
        assert!(judge(Path::new(r"C:\r\d\l\f"), &rl, &Subst::new()));
        assert!(seen.borrow().iter().any(|r| r == r"c:\r\x\z\f"), "{:?}", seen.borrow());
    }

    /// Judge round 5, item 2: Win32 collapses `.` and `..` lexically before any link is resolved
    /// (RtlGetFullPathName_U), so the judge does too, then walks the collapsed path; it never pops a
    /// component of a path a link redirected. With C:\r\d -> x\y, `C:\r\d\..\f` is C:\r\f (not the
    /// redirected C:\r\x\f), and `C:\r\d\..\..\f` is C:\f. A link there to a share is found.
    #[test]
    fn a_parent_step_is_collapsed_before_any_link_is_read() {
        let map: &'static [(&str, &str)] = &[(r"c:\r\d", r"x\y"), (r"c:\r\f", r"\\attacker\share"), (r"c:\f", r"\\attacker\share")];
        for (path, real, never) in [(r"C:\r\d\..\f", r"c:\r\f", r"c:\r\x\f"), (r"C:\r\d\..\..\f", r"c:\f", r"c:\r\f"), (r"C:\r\.\d\..\f", r"c:\r\f", r"c:\r\x\f")] {
            let seen = RefCell::new(Vec::new());
            let rl = links(map, &seen);
            assert!(!judge(Path::new(path), &rl, &Subst::new()), "{path} is {real}, a link to a share");
            let reads = seen.borrow().clone();
            assert!(reads.iter().any(|r| r == real), "{path}: the real place was read: {reads:?}");
            assert!(!reads.iter().any(|r| r == never), "{path}: a popped redirected path was read: {reads:?}");
        }
        // A link and a parent step after it: `C:\r\d\l\..\g` is C:\r\d\g, below the link's target.
        let seen = RefCell::new(Vec::new());
        let rl = links(map, &seen);
        assert!(judge(Path::new(r"C:\r\d\l\..\g"), &rl, &Subst::new()));
        assert!(seen.borrow().iter().any(|r| r == r"c:\r\x\y\g"), "{:?}", seen.borrow());
        assert!(!seen.borrow().iter().any(|r| r.ends_with(r"\l")), "the collapsed step is never read: {:?}", seen.borrow());
        // A verbatim path is not collapsed by Win32: a parent or current step in it is refused unread.
        for path in [r"\\?\C:\r\d\..\f", r"\\?\C:\r\.\f"] {
            let seen = RefCell::new(Vec::new());
            assert!(!judge(Path::new(path), &links(map, &seen), &Subst::new()), "{path}");
            assert!(seen.borrow().is_empty(), "{path}: {:?}", seen.borrow());
        }
    }

    /// Judge rulings r3, item 2: FIXED, REMOVABLE, CDROM and RAMDISK are local while the device is
    /// not a redirector (RAM disks and VeraCrypt volumes are allowed); REMOTE, UNKNOWN and
    /// NO_ROOT_DIR are refused, and the real drive answer is passed through, not collapsed to FIXED.
    #[test]
    fn drive_kinds_follow_the_ruling() {
        let k = |t: &str| letter_kind(Some(t.to_string()));
        for local in [r"\Device\ImDisk0", r"\Device\VeraCryptVolumeE", r"\Device\HarddiskVolume9"] {
            assert_eq!(k(local), Letter::Volume, "{local}");
        }
        for net in [r"\Device\Mup\h\s", r"\Device\LanmanRedirector\h\s", r"\Device\WebDavRedirector\h", r"\Device\SomeVendorRedirector\x", r"\device\mup", r"\Device\Harddisk3\DR7", r"\Device\RdpDr\;T:1\tsclient\c"] {
            assert_eq!(k(net), Letter::Network, "{net}");
        }
        assert_eq!(k(r"\??\UNC\h\s"), Letter::Elsewhere);
        for (kind, ok) in [(0u32, false), (1, false), (2, true), (3, true), (4, false), (5, true), (6, true), (7, false)] {
            assert_eq!(drive_local(kind), ok, "type {kind}");
        }
        let cache = std::sync::Mutex::new([None; 26]);
        assert_eq!(drive_answer(&cache, 3, || 6), 6, "the answer is passed through");
        assert_eq!(drive_answer(&cache, 3, || 0), 0);
        assert_eq!(drive_answer(&cache, 3, || DRIVE_REMOTE), DRIVE_REMOTE);
        assert_eq!(drive_answer(&cache, 3, || 3), DRIVE_REMOTE, "a share once seen stays refused");
        // The judge: a device that is not a redirector is local only with a local drive type.
        struct Typed(&'static str, u32);
        impl Drives for Typed {
            fn dos_device(&self, _: char) -> Option<String> {
                Some(self.0.to_string())
            }
            fn drive_type(&self, _: char) -> u32 {
                self.1
            }
        }
        let seen = RefCell::new(Vec::new());
        let rl = links(&[], &seen);
        for (kind, ok) in [(2u32, true), (3, true), (5, true), (6, true), (0, false), (1, false), (4, false)] {
            assert_eq!(judge(Path::new(r"R:\a"), &rl, &Typed(r"\Device\ImDisk0", kind)), ok, "ImDisk type {kind}");
        }
        assert!(!judge(Path::new(r"R:\a"), &rl, &Typed(r"\Device\LanmanRedirector\x", 3)), "a redirector is refused whatever the type");
    }

    /// Round 3, item 2a: the judge itself never touches the network. A letter's device map is read
    /// first; GetDriveTypeW (which opens the root) is asked only for a letter whose target is a local
    /// volume, never for a SUBST letter, a mapped drive or any other redirected device, and no
    /// reparse data is read on such a letter.
    #[test]
    fn the_drive_type_is_asked_only_for_a_local_volume() {
        let d = Subst::new();
        let seen = RefCell::new(Vec::new());
        let rl = links(&[], &seen);
        for p in [r"Y:\a", r"Z:\a", r"W:\a", r"M:\a", r"L:\a", r"K:\a", r"Q:\a", r"R:\a", r"N:\a", r"\\?\M:\a", r"\??\Y:\a"] {
            assert!(!judge(Path::new(p), &rl, &d), "{p}");
        }
        assert!(d.0.borrow().is_empty(), "GetDriveTypeW asked for {:?}", d.0.borrow());
        assert!(seen.borrow().is_empty(), "reparse data read on {:?}", seen.borrow());
        // A SUBST letter onto a local folder: only the resolved letter's type is asked.
        assert!(judge(Path::new(r"X:\a"), &rl, &d));
        assert_eq!(d.0.borrow().as_slice(), ['C']);
        assert!(!seen.borrow().iter().any(|r| r.starts_with("x:")), "the SUBST letter itself is never read: {:?}", seen.borrow());
        assert!(judge(Path::new(r"E:\a"), &rl, &d), "a CD drive is a local volume");
    }

    #[test]
    fn device_map_targets_are_classified() {
        let k = |t: &str| letter_kind(Some(t.to_string()));
        assert_eq!(k(r"\Device\HarddiskVolume12"), Letter::Volume);
        assert_eq!(k(r"\device\harddiskvolume1"), Letter::Volume);
        assert_eq!(k(r"\Device\Floppy0"), Letter::Volume);
        assert_eq!(k(r"\??\C:\dir"), Letter::Subst(r"C:\dir".into()));
        assert_eq!(k(r"\??\C:"), Letter::Subst(r"C:".into()));
        for t in [r"\??\UNC\h\s", r"\??\Volume{x}\", r"\??\GLOBALROOT\Device\Mup\h", r"\??\CC:\x"] {
            assert_eq!(k(t), Letter::Elsewhere, "{t}");
        }
        for t in [r"\Device\Mup\h\s", r"\Device\LanmanRedirector\;Z:0\h\s", r"\Device\WebDavRedirector\h", r"\Device\HarddiskVolume1\..\Mup", r"\Device\RdpDr\;T:1\tsclient\c", ""] {
            assert_eq!(k(t), Letter::Network, "{t}");
        }
        assert_eq!(letter_kind(None), Letter::Missing);
    }

    #[test]
    fn a_relative_path_is_never_judged_local() {
        let none = |_: &Path| None;
        for p in ["a.txt", r"a\b", r"\a\b", "C:a", "C:", ".", ""] {
            assert!(!judge(Path::new(p), &none, &LocalDrives), "{p:?}");
        }
    }

    /// The reparse reader's parts: which tags are followed, ignored or refused, and how a symbolic
    /// link's and a junction's data reads.
    #[test]
    fn reparse_data_is_read_and_unknown_tags_are_refused() {
        assert_eq!(reparse_kind(TAG_SYMLINK), Reparse::Link);
        assert_eq!(reparse_kind(TAG_MOUNT_POINT), Reparse::Link);
        for t in [0x9000_001A, 0x9000_F01A, 0x8000_0013, 0x8000_001B] {
            assert_eq!(reparse_kind(t), Reparse::Inert, "{t:#x}");
        }
        // DFS, DFSR, HSM, NFS, Azure File Sync, an unknown third-party tag.
        for t in [0x8000_000A, 0x8000_0012, 0xC000_0004, 0x8000_0014, 0x8000_001E, 0x0000_1234] {
            assert_eq!(reparse_kind(t), Reparse::Opaque, "{t:#x}");
        }
        fn buffer(tag: u32, name: &str, flags: Option<u32>) -> Vec<u8> {
            let w: Vec<u8> = name.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
            let mut b = tag.to_le_bytes().to_vec();
            b.extend([0u8; 4]);
            b.extend(0u16.to_le_bytes());
            b.extend((w.len() as u16).to_le_bytes());
            b.extend((w.len() as u16).to_le_bytes());
            b.extend(0u16.to_le_bytes());
            if let Some(f) = flags {
                b.extend(f.to_le_bytes());
            }
            b.extend(&w);
            b
        }
        assert_eq!(link_target(&buffer(TAG_MOUNT_POINT, r"\??\UNC\h\s", None)), Some(PathBuf::from(r"\\?\UNC\h\s")));
        assert_eq!(link_target(&buffer(TAG_MOUNT_POINT, r"\??\C:\t", None)), Some(PathBuf::from(r"\\?\C:\t")));
        assert_eq!(link_target(&buffer(TAG_SYMLINK, r"\??\D:\x", Some(0))), Some(PathBuf::from(r"\\?\D:\x")));
        assert_eq!(link_target(&buffer(TAG_SYMLINK, r"..\x", Some(1))), Some(PathBuf::from(r"..\x")));
        assert_eq!(link_target(&buffer(TAG_SYMLINK, r"\??\x", Some(0))[..12]), None, "a short buffer");
        // Every spelling of a link target onto a share is refused by the judge.
        for t in [r"\\?\UNC\h\s", r"\\?\Volume{x}\", UNREADABLE_LINK] {
            let read = |p: &Path| (p == Path::new(r"C:\j")).then(|| PathBuf::from(t));
            assert!(!judge(Path::new(r"C:\j\f"), &read, &LocalDrives), "{t}");
        }
    }

    /// The real reader on this machine: a plain folder and a missing path are not links; a junction
    /// made in the temp folder (onto another local folder, never a share) reads its target.
    #[cfg(windows)]
    #[test]
    fn the_real_reader_reads_a_junction_without_following_it() {
        let base = std::env::temp_dir().join(format!("termurion-reparse-{}", std::process::id()));
        let target = base.join("target");
        let link = base.join("link");
        std::fs::create_dir_all(&target).unwrap();
        assert_eq!(read_reparse(&target), None);
        assert_eq!(read_reparse(&base.join("missing").join("x")), None);
        let made = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&link).arg(&target).output().unwrap();
        assert!(made.status.success(), "{made:?}");
        let read = read_reparse(&link).expect("a junction");
        assert!(read.to_string_lossy().to_lowercase().ends_with(&target.to_string_lossy().to_lowercase()[2..]), "{read:?}");
        assert!(judge(&link.join("f.txt"), &read_reparse, &RealDrives));
        std::fs::remove_dir(&link).unwrap();
        std::fs::remove_dir_all(&base).unwrap();
    }
}

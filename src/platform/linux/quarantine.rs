//! What Linux marks a download with: the freedesktop origin attributes, the same ones Chromium and
//! Firefox write. No quarantine exists here, so this records where the file came from.

use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

const ORIGIN: &str = "user.xdg.origin.url";
const REFERRER: &str = "user.xdg.referrer.url";
/// The longest value written; a longer one is skipped (logged once), as an over-long value would
/// fail the download on a file system with small attribute blocks.
const MAX_VALUE: usize = 2048;

/// Records the page the file came from (and its referrer, when `text` has one). A file system
/// without user attributes keeps the file unmarked; any other failure is an error.
pub fn mark(path: &Path, text: &str) -> io::Result<()> {
    mark_with(path, text, &set_xattr)
}

fn mark_with(path: &Path, text: &str, set: &dyn Fn(&Path, &str, &[u8]) -> io::Result<()>) -> io::Result<()> {
    let pairs = [(ORIGIN, super::super::quarantine::host_url(text)), (REFERRER, super::super::quarantine::referrer_url(text))];
    for (name, value) in pairs {
        let Some(value) = value else { continue };
        if value.len() > MAX_VALUE {
            static LONG: AtomicBool = AtomicBool::new(false);
            if !LONG.swap(true, Ordering::Relaxed) {
                crate::dlog!("A download's origin address is too long to keep, so it is not marked");
            }
            continue;
        }
        match set(path, name, value.as_bytes()) {
            Ok(()) => {}
            Err(e) if unsupported(&e) => {
                static LOGGED: AtomicBool = AtomicBool::new(false);
                if !LOGGED.swap(true, Ordering::Relaxed) {
                    crate::dlog!("This file system keeps no user attributes, so downloads are not marked");
                }
                return Ok(());
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn unsupported(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(c) if c == libc::ENOTSUP || c == libc::EOPNOTSUPP)
}

fn set_xattr(path: &Path, name: &str, value: &[u8]) -> io::Result<()> {
    let p = CString::new(path.as_os_str().as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path has a NUL"))?;
    let n = CString::new(name).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "name has a NUL"))?;
    // lsetxattr: a link is never followed to mark what it points at.
    // SAFETY: both strings are NUL-terminated and the value is a live slice.
    let r = unsafe { libc::lsetxattr(p.as_ptr(), n.as_ptr(), value.as_ptr().cast(), value.len(), 0) };
    if r == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xattr(p: &Path, name: &str) -> Option<String> {
        let c = CString::new(p.as_os_str().as_bytes()).unwrap();
        let n = CString::new(name).unwrap();
        let mut buf = [0u8; 512];
        let got = unsafe { libc::getxattr(c.as_ptr(), n.as_ptr(), buf.as_mut_ptr().cast(), buf.len()) };
        (got >= 0).then(|| String::from_utf8_lossy(&buf[..got as usize]).into_owned())
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("quarantine-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(format!("{name}-{}", std::process::id()));
        std::fs::write(&p, "hi").unwrap();
        p
    }

    /// A value over the cap is skipped, not an error, and nothing is written for it.
    #[test]
    fn an_over_long_origin_is_skipped_not_a_failure() {
        let long = format!("[ZoneTransfer]\r\nZoneId=3\r\nHostUrl=ssh://h/{}\r\n", "a".repeat(MAX_VALUE));
        let wrote = std::cell::Cell::new(0);
        let r = mark_with(Path::new("/nowhere"), &long, &|_, _, _| {
            wrote.set(wrote.get() + 1);
            Err(io::Error::from_raw_os_error(libc::ENOSPC))
        });
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(wrote.get(), 0);
    }

    /// A link is not followed: the target keeps no mark.
    #[test]
    fn a_link_is_never_followed() {
        let target = scratch("link-target");
        let link = target.with_extension("lnk");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let _ = mark(&link, "[ZoneTransfer]\r\nZoneId=3\r\nHostUrl=https://h/x\r\n");
        assert_eq!(xattr(&target, ORIGIN), None, "the link's target was marked");
        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_file(&target);
    }

    #[test]
    fn a_download_carries_its_origin() {
        let p = scratch("origin");
        let zone = "[ZoneTransfer]\r\nZoneId=3\r\nReferrerUrl=https://r.test/page\r\nHostUrl=https://h.test/x.bin\r\n";
        let r = mark(&p, zone);
        let (origin, referrer) = (xattr(&p, "user.xdg.origin.url"), xattr(&p, "user.xdg.referrer.url"));
        let _ = std::fs::remove_file(&p);
        r.unwrap();
        assert_eq!(origin.as_deref(), Some("https://h.test/x.bin"));
        assert_eq!(referrer.as_deref(), Some("https://r.test/page"));
    }

    /// A file system that refuses user attributes keeps the download; any other error is still an error.
    #[test]
    fn no_xattr_support_keeps_the_download() {
        let p = Path::new("/tmp/x");
        let zone = "[ZoneTransfer]\r\nZoneId=3\r\nHostUrl=https://h.test/x\r\n";
        let refuse = |code: i32| move |_: &Path, _: &str, _: &[u8]| Err(io::Error::from_raw_os_error(code));
        assert!(mark_with(p, zone, &refuse(libc::ENOTSUP)).is_ok());
        assert!(mark_with(p, zone, &refuse(libc::EOPNOTSUPP)).is_ok());
        for code in [libc::EACCES, libc::ENOSPC, libc::ENOENT, libc::EPERM] {
            assert_eq!(mark_with(p, zone, &refuse(code)).unwrap_err().raw_os_error(), Some(code));
        }
    }

    /// A text with no page leaves nothing to write, and a missing file is not marked.
    #[test]
    fn nothing_to_say_and_no_file() {
        let p = scratch("plain");
        let r = mark(&p, "[ZoneTransfer]\r\nZoneId=3\r\n");
        let origin = xattr(&p, ORIGIN);
        let _ = std::fs::remove_file(&p);
        r.unwrap();
        assert_eq!(origin, None);
        assert!(mark(Path::new("/no/such/termurion-file"), "HostUrl=https://h.test/x").is_err());
    }
}

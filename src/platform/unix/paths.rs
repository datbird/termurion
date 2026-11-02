//! Unix folders.

use std::ffi::{CStr, OsString};
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

/// The app's folder name under the base.
#[cfg(target_os = "macos")]
pub const APP_FOLDER: &str = "Termurion";
#[cfg(not(target_os = "macos"))]
pub const APP_FOLDER: &str = "termurion";

/// The settings base for this OS.
pub fn config_base() -> PathBuf {
    config_base_from(std::env::var("XDG_CONFIG_HOME").ok().as_deref(), home_dir().as_deref())
}

/// The machine-local base for this OS.
pub fn local_base() -> PathBuf {
    local_base_from(std::env::var("XDG_STATE_HOME").ok().as_deref(), home_dir().as_deref())
}

/// The pure choice over the XDG variable and home, so a test can feed both. macOS ignores XDG.
pub fn config_base_from(xdg: Option<&str>, home: Option<&Path>) -> PathBuf {
    if cfg!(target_os = "macos") {
        return home.map_or_else(std::env::temp_dir, |h| h.join("Library").join("Application Support"));
    }
    xdg_or(xdg, home, ".config")
}

/// `config_base_from`'s machine-local twin.
pub fn local_base_from(xdg: Option<&str>, home: Option<&Path>) -> PathBuf {
    if cfg!(target_os = "macos") {
        return home.map_or_else(std::env::temp_dir, |h| h.join("Library").join("Caches"));
    }
    xdg_or(xdg, home, ".local/state")
}

/// An absolute XDG value, else `~/<fallback>`, else the temp folder.
fn xdg_or(xdg: Option<&str>, home: Option<&Path>, fallback: &str) -> PathBuf {
    if let Some(x) = xdg.map(Path::new).filter(|p| p.is_absolute()) {
        return x.to_path_buf();
    }
    home.map_or_else(std::env::temp_dir, |h| h.join(fallback))
}

/// Nothing on Unix is a Windows path.
pub fn windows_local_path(_reported: &str) -> Option<PathBuf> {
    None
}

/// A reported directory: an absolute Unix path. None for anything else.
pub fn local_path(reported: &str) -> Option<PathBuf> {
    reported.starts_with('/').then(|| PathBuf::from(reported))
}

/// `$HOME` when it is an absolute path, else the effective user's `pw_dir`.
pub fn home_dir() -> Option<PathBuf> {
    home_from(std::env::var_os("HOME"), passwd_field(|pw| pw.pw_dir))
}

/// The pure choice, so a test can feed both sources.
fn home_from(env_home: Option<OsString>, pw_dir: Option<OsString>) -> Option<PathBuf> {
    let absolute = |v: OsString| Some(PathBuf::from(v)).filter(|p| p.is_absolute());
    env_home.and_then(absolute).or_else(|| pw_dir.and_then(absolute))
}

/// One string field of the effective user's passwd entry (`getpwuid_r`, no shared static buffer).
pub(crate) fn passwd_field(field: impl Fn(&libc::passwd) -> *mut libc::c_char) -> Option<OsString> {
    let mut buf = vec![0u8; 4096];
    loop {
        let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
        let mut out: *mut libc::passwd = std::ptr::null_mut();
        let rc = unsafe { libc::getpwuid_r(libc::geteuid(), &mut pw, buf.as_mut_ptr().cast(), buf.len(), &mut out) };
        if rc == libc::ERANGE && buf.len() < 1 << 20 {
            buf.resize(buf.len() * 4, 0);
            continue;
        }
        if rc != 0 || out.is_null() {
            return None;
        }
        let p = field(&pw);
        if p.is_null() {
            return None;
        }
        let bytes = unsafe { CStr::from_ptr(p) }.to_bytes();
        return (!bytes.is_empty()).then(|| OsString::from_vec(bytes.to_vec()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_prefers_an_absolute_home_variable() {
        let some = |s: &str| Some(OsString::from(s));
        assert_eq!(home_from(some("/home/a"), some("/home/b")), Some(PathBuf::from("/home/a")));
        assert_eq!(home_from(some("rel"), some("/home/b")), Some(PathBuf::from("/home/b")), "a relative $HOME is ignored");
        assert_eq!(home_from(None, some("/home/b")), Some(PathBuf::from("/home/b")));
        assert_eq!(home_from(None, None), None);
    }

    #[test]
    fn the_password_database_answers() {
        assert!(passwd_field(|pw| pw.pw_dir).is_some_and(|d| d.to_string_lossy().starts_with('/')));
    }
}

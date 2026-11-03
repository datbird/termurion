//! Finder alias files. An alias is a small file (not a symbolic link) that names another file;
//! `open` follows it, so the open judge must judge what it names, not the alias's own name.

use std::path::{Path, PathBuf};

use core_foundation::array::CFArray;
use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::data::CFData;
use core_foundation::dictionary::CFDictionary;
use core_foundation::error::CFError;
use core_foundation::string::CFString;
use core_foundation::url::CFURL;
use core_foundation_sys::error::CFErrorRef;
use core_foundation_sys::url as sys;

/// The text of a CFError the system handed back (and frees it).
fn error_text(e: CFErrorRef) -> String {
    if e.is_null() {
        return "The alias could not be read.".into();
    }
    unsafe { CFError::wrap_under_create_rule(e) }.description().to_string()
}

/// Whether `path` is a Finder alias file (a regular file the system calls an alias, and not a
/// symbolic link). None when it is not one; Some(Err) when it is, or may be, and its target
/// cannot be found.
pub fn finder_alias(path: &Path) -> Option<Result<PathBuf, String>> {
    // An alias is a regular file; everything else is judged without asking the system.
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.file_type().is_file() {
        return None;
    }
    // A path that cannot be made a URL cannot be asked about: shown, not opened.
    let Some(url) = CFURL::from_path(path, false) else { return Some(Err("The alias has no usable name.".into())) };
    match is_alias(&url) {
        Ok(false) => None,
        Ok(true) => Some(resolve(&url)),
        Err(e) => Some(Err(e)),
    }
}

/// `kCFURLIsAliasFileKey` true and `kCFURLIsSymbolicLinkKey` false.
fn is_alias(url: &CFURL) -> Result<bool, String> {
    let (alias_key, link_key) = unsafe { (CFString::wrap_under_get_rule(sys::kCFURLIsAliasFileKey), CFString::wrap_under_get_rule(sys::kCFURLIsSymbolicLinkKey)) };
    let keys = CFArray::from_CFTypes(&[alias_key.clone(), link_key.clone()]);
    let mut err: CFErrorRef = std::ptr::null_mut();
    let d = unsafe { sys::CFURLCopyResourcePropertiesForKeys(url.as_concrete_TypeRef(), keys.as_concrete_TypeRef(), &mut err) };
    if d.is_null() {
        return Err(error_text(err));
    }
    let props: CFDictionary<CFString, CFType> = unsafe { CFDictionary::wrap_under_create_rule(d) };
    let flag = |k: &CFString| props.find(k).and_then(|v| v.downcast::<CFBoolean>()).map(bool::from);
    // A flag the system did not answer counts as an alias: fail safe.
    Ok(flag(&alias_key).unwrap_or(true) && !flag(&link_key).unwrap_or(false))
}

/// The target the same probe rule as a click lets be touched: no network mount, no auto-mount
/// folder, no device, and a name that is text. Nothing is asked of the disk before this.
fn screened(path: &Path) -> Result<(), String> {
    let text = path.to_str().ok_or_else(|| "The alias names a file whose name is not text.".to_string())?;
    if crate::netpath::may_probe(text, &crate::netpath::Probe::default()) {
        Ok(())
    } else {
        Err(format!("{text} is on a network or special location."))
    }
}

unsafe extern "C" {
    // core-foundation-sys hides this key behind a feature; it is in CoreFoundation since macOS 10.8.
    static kCFURLPathKey: core_foundation_sys::string::CFStringRef;
}

/// The path stored in bookmark data, read without touching the target.
fn stored_path(data: &CFData) -> Result<PathBuf, String> {
    let v = unsafe { sys::CFURLCreateResourcePropertyForKeyFromBookmarkData(std::ptr::null(), kCFURLPathKey, data.as_concrete_TypeRef()) };
    if v.is_null() {
        return Err("The alias stores no path.".into());
    }
    let v = unsafe { CFType::wrap_under_create_rule(v) };
    v.downcast::<CFString>().map(|s| PathBuf::from(s.to_string())).ok_or_else(|| "The alias stores no path.".to_string())
}

/// The volume the bookmark data names, read without touching it: the resolver can find a file
/// by volume and file number, so the volume is screened like the stored path.
fn stored_volume(data: &CFData) -> Result<PathBuf, String> {
    let v = unsafe { sys::CFURLCreateResourcePropertyForKeyFromBookmarkData(std::ptr::null(), sys::kCFURLVolumeURLKey, data.as_concrete_TypeRef()) };
    if v.is_null() {
        return Err("The alias stores no volume.".into());
    }
    let v = unsafe { CFType::wrap_under_create_rule(v) };
    v.downcast::<CFURL>().and_then(|u| u.to_path()).ok_or_else(|| "The alias stores no volume.".to_string())
}

/// The canonical target of the alias file at `url`, without any dialog and without mounting.
fn resolve(url: &CFURL) -> Result<PathBuf, String> {
    let mut err: CFErrorRef = std::ptr::null_mut();
    let data = unsafe { sys::CFURLCreateBookmarkDataFromFile(std::ptr::null(), url.as_concrete_TypeRef(), &mut err) };
    if data.is_null() {
        return Err(error_text(err));
    }
    let data = unsafe { CFData::wrap_under_create_rule(data) };
    // The path the alias stored is read from the bookmark bytes alone and screened first: the
    // system resolver looks the target up, and that must never reach a network mount.
    screened(&stored_path(&data)?)?;
    screened(&stored_volume(&data)?)?;
    let mut stale = 0;
    let target = unsafe {
        sys::CFURLCreateByResolvingBookmarkData(
            std::ptr::null(),
            data.as_concrete_TypeRef(),
            sys::kCFURLBookmarkResolutionWithoutUIMask | sys::kCFURLBookmarkResolutionWithoutMountingMask,
            std::ptr::null(),
            std::ptr::null(),
            &mut stale,
            &mut err,
        )
    };
    if target.is_null() {
        return Err(error_text(err));
    }
    let target = unsafe { CFURL::wrap_under_create_rule(target) };
    let path = target.to_path().ok_or_else(|| "The alias names no file.".to_string())?;
    screened(&path)?;
    std::fs::canonicalize(&path).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::unix::open_judge::{judged_open, runnable, Judged};
    use std::os::unix::fs::PermissionsExt;

    /// A target that is a network mount or auto-mount folder is refused before anything is read
    /// from it, and a name that is not text is refused too.
    #[test]
    fn a_network_target_is_refused_unread() {
        assert!(screened(Path::new("/net/evil.example/x.txt")).is_err());
        assert!(screened(Path::new("/Network/Servers/h/x.txt")).is_err());
        use std::os::unix::ffi::OsStrExt;
        assert!(screened(Path::new(std::ffi::OsStr::from_bytes(b"/tmp/\xff.txt"))).is_err());
        assert!(screened(Path::new("/tmp/x.txt")).is_ok());
    }

    /// The stored path of an alias is read from its bookmark bytes without resolving it.
    #[test]
    fn the_stored_path_is_read_before_resolving() {
        let d = std::env::temp_dir().join(format!("termurion-alias-stored-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        file(&d.join("t.txt"), 0o644);
        make_alias(&d.join("t.txt"), &d.join("a.txt"));
        let mut err: CFErrorRef = std::ptr::null_mut();
        let data = unsafe { sys::CFURLCreateBookmarkDataFromFile(std::ptr::null(), CFURL::from_path(d.join("a.txt"), false).unwrap().as_concrete_TypeRef(), &mut err) };
        assert!(!data.is_null());
        let data = unsafe { CFData::wrap_under_create_rule(data) };
        assert_eq!(stored_path(&data).unwrap().file_name().unwrap(), "t.txt");
        assert!(stored_volume(&data).is_ok_and(|v| v.is_absolute()));
        // Both are screened before the resolver, in that order.
        let src = include_str!("alias.rs");
        let code = &src[..src.find("#[cfg(test)]").unwrap()];
        let at = |s: &str| code.find(s).unwrap();
        assert!(at("screened(&stored_path(") < at("screened(&stored_volume("));
        assert!(at("screened(&stored_volume(") < at("sys::CFURLCreateByResolvingBookmarkData("));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Writes a Finder alias at `alias` naming `target` (what the Finder's Make Alias does).
    fn make_alias(target: &Path, alias: &Path) {
        let t = CFURL::from_path(target, target.is_dir()).unwrap();
        let a = CFURL::from_path(alias, false).unwrap();
        let mut err: CFErrorRef = std::ptr::null_mut();
        let data = unsafe {
            sys::CFURLCreateBookmarkData(std::ptr::null(), t.as_concrete_TypeRef(), sys::kCFURLBookmarkCreationSuitableForBookmarkFile, std::ptr::null(), std::ptr::null(), &mut err)
        };
        assert!(!data.is_null(), "{}", error_text(err));
        let data = unsafe { CFData::wrap_under_create_rule(data) };
        let ok = unsafe { sys::CFURLWriteBookmarkDataToFile(data.as_concrete_TypeRef(), a.as_concrete_TypeRef(), 0, &mut err) };
        assert!(ok != 0, "{}", error_text(err));
    }

    fn file(p: &Path, mode: u32) {
        std::fs::write(p, "x").unwrap();
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    /// A harmless name on an alias to a program is shown, an alias to a document opens that
    /// document, and an alias that no longer resolves is shown too.
    #[test]
    fn an_alias_is_judged_by_its_target() {
        let d = std::env::temp_dir().join(format!("termurion-alias-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).unwrap();
        let real = std::fs::canonicalize(&d).unwrap();

        file(&d.join("payload.terminal"), 0o644);
        make_alias(&d.join("payload.terminal"), &d.join("README.txt"));
        let readme = d.join("README.txt");
        assert_eq!(finder_alias(&readme), Some(Ok(real.join("payload.terminal"))));
        assert!(runnable(&readme.to_string_lossy()));
        assert_eq!(judged_open(&readme), Judged::Reveal(readme.clone()));

        file(&d.join("notes.txt"), 0o644);
        make_alias(&d.join("notes.txt"), &d.join("link.md"));
        let link = d.join("link.md");
        assert!(!runnable(&link.to_string_lossy()));
        assert_eq!(judged_open(&link), Judged::Open(real.join("notes.txt")));

        // An alias of an alias is followed all the way.
        make_alias(&readme, &d.join("again.txt"));
        assert!(runnable(&d.join("again.txt").to_string_lossy()));

        // A plain file and a symbolic link are not aliases.
        assert_eq!(finder_alias(&d.join("notes.txt")), None);
        let sym = d.join("sym.txt");
        std::os::unix::fs::symlink(d.join("notes.txt"), &sym).unwrap();
        assert_eq!(finder_alias(&sym), None);

        // Its target is gone: nothing says what it opens, so it is shown.
        file(&d.join("gone.txt"), 0o644);
        make_alias(&d.join("gone.txt"), &d.join("dangling.txt"));
        std::fs::remove_file(d.join("gone.txt")).unwrap();
        assert!(matches!(finder_alias(&d.join("dangling.txt")), Some(Err(_))));
        assert!(runnable(&d.join("dangling.txt").to_string_lossy()));
        assert_eq!(judged_open(&d.join("dangling.txt")), Judged::Reveal(d.join("dangling.txt")));
        let _ = std::fs::remove_dir_all(&d);
    }
}

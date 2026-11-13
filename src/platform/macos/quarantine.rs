//! The quarantine mark on a download (what Windows does with the Mark of the Web): Gatekeeper
//! asks about the file the first time it is opened. The mark is the `com.apple.quarantine`
//! attribute, set through the file URL's quarantine properties so the system writes it.

use std::path::Path;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_foundation::{ns_string, NSArray, NSBundle, NSCopying, NSDate, NSDictionary, NSMutableDictionary, NSString, NSURL, NSURLQuarantinePropertiesKey};

/// The app's display name for the mark: the bundle's `CFBundleName`, else "Termurion".
fn agent_name() -> Retained<NSString> {
    let name = NSBundle::mainBundle().objectForInfoDictionaryKey(ns_string!("CFBundleName"));
    name.and_then(|n| n.downcast::<NSString>().ok()).unwrap_or_else(|| NSString::from_str("Termurion"))
}

/// The app's bundle id for the mark, else the shipped one.
fn agent_bundle() -> Retained<NSString> {
    NSBundle::mainBundle().bundleIdentifier().unwrap_or_else(|| NSString::from_str(crate::presence::MAC_BUNDLE_ID))
}

pub fn mark(path: &Path, text: &str) -> std::io::Result<()> {
    let p = path.to_str().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "path is not UTF-8"))?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(p));
    let key = unsafe { NSURLQuarantinePropertiesKey };
    // Keep what the file already carries (a mark from an earlier step), then set ours over it.
    let props: Retained<NSMutableDictionary<NSString, AnyObject>> = NSMutableDictionary::new();
    let existing = url.resourceValuesForKeys_error(&NSArray::from_slice(&[key])).map_err(|e| failed(&e))?;
    if let Some(old) = existing.objectForKey(key).and_then(|o| o.downcast::<NSDictionary>().ok()) {
        for k in old.allKeys().iter() {
            if let (Some(name), Some(v)) = (k.downcast_ref::<NSString>(), old.objectForKey(&k)) {
                put(&props, name, &v);
            }
        }
    }
    put(&props, ns_string!("LSQuarantineAgentName"), agent_name().as_ref());
    put(&props, ns_string!("LSQuarantineAgentBundleIdentifier"), agent_bundle().as_ref());
    // The data URL is a URL, not text; one that does not parse is left out.
    if let Some(u) = super::super::quarantine::host_url(text).and_then(|u| NSURL::URLWithString(&NSString::from_str(u))) {
        put(&props, ns_string!("LSQuarantineDataURL"), u.as_ref());
    }
    put(&props, ns_string!("LSQuarantineTimeStamp"), NSDate::now().as_ref());
    put(&props, ns_string!("LSQuarantineType"), ns_string!("LSQuarantineTypeOtherDownload").as_ref());
    let any: &AnyObject = props.as_ref();
    unsafe { url.setResourceValue_forKey_error(Some(any), key) }.map_err(|e| failed(&e))
}

fn put(d: &NSMutableDictionary<NSString, AnyObject>, k: &NSString, v: &AnyObject) {
    unsafe { d.setObject_forKey(v, ProtocolObject::<dyn NSCopying>::from_ref(k)) };
}

fn failed(e: &objc2_foundation::NSError) -> std::io::Error {
    std::io::Error::other(e.localizedDescription().to_string())
}

#[cfg(test)]
mod tests {
    use crate::test_tmp::TempDir;
    use super::*;

    fn quarantine_of(p: &Path) -> Option<String> {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(p.as_os_str().as_bytes()).unwrap();
        let name = std::ffi::CString::new("com.apple.quarantine").unwrap();
        let mut buf = [0u8; 512];
        let n = unsafe { libc::getxattr(c.as_ptr(), name.as_ptr(), buf.as_mut_ptr().cast(), buf.len(), 0, 0) };
        (n >= 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).into_owned())
    }

    /// A marked download carries the quarantine attribute, with this app named in it.
    #[test]
    fn a_download_is_quarantined() {
        let _td = TempDir::new("quarantine");
        let p = _td.file("x.txt");
        std::fs::write(&p, "hi").unwrap();
        let zone = "[ZoneTransfer]\r\nZoneId=3\r\nHostUrl=ssh://u@h/x\r\n";
        let r = mark(&p, zone);
        let got = quarantine_of(&p);
        r.unwrap();
        let got = got.expect("the attribute is there");
        assert!(got.contains(";Termurion;"), "{got}");
    }

    /// A file that is not there cannot be marked.
    #[test]
    fn a_missing_file_fails() {
        assert!(mark(Path::new("/no/such/termurion-file.txt"), "").is_err());
    }
}

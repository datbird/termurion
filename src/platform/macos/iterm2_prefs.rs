//! iTerm2's live settings on the Mac: the preferences domain read through CFPreferences, so what
//! iTerm2 has not yet flushed to its plist is seen too.

use std::path::PathBuf;

use core_foundation::array::CFArray;
use core_foundation::base::{CFType, TCFType};
use core_foundation::data::CFData;
use core_foundation::string::CFString;
use core_foundation_sys::array::CFArrayRef;
use core_foundation_sys::base::kCFAllocatorDefault;
use core_foundation_sys::dictionary::CFDictionaryRef;
use core_foundation_sys::preferences::{kCFPreferencesAnyHost, kCFPreferencesCurrentUser, CFPreferencesCopyKeyList, CFPreferencesCopyMultiple};
use core_foundation_sys::propertylist::{kCFPropertyListBinaryFormat_v1_0, CFPropertyListCreateData};

pub fn live_file() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Preferences").join(format!("{}.plist", super::super::iterm2_prefs::ITERM2_DOMAIN)))
}

pub fn read_domain(domain: &str) -> Result<Option<Vec<u8>>, String> {
    let app = CFString::new(domain);
    // SAFETY: the constants are CF globals; each Copy call returns an owned (+1) object or null.
    let keys = unsafe {
        let r: CFArrayRef = CFPreferencesCopyKeyList(app.as_concrete_TypeRef(), kCFPreferencesCurrentUser, kCFPreferencesAnyHost);
        if r.is_null() {
            return Ok(None);
        }
        CFArray::<CFType>::wrap_under_create_rule(r)
    };
    if keys.is_empty() {
        return Ok(None);
    }
    let dict: CFDictionaryRef = unsafe {
        CFPreferencesCopyMultiple(keys.as_concrete_TypeRef(), app.as_concrete_TypeRef(), kCFPreferencesCurrentUser, kCFPreferencesAnyHost)
    };
    if dict.is_null() {
        return Ok(None);
    }
    // Owned dictionary, released when this guard drops.
    let dict = unsafe { CFType::wrap_under_create_rule(dict as _) };
    let mut err = std::ptr::null_mut();
    let data = unsafe { CFPropertyListCreateData(kCFAllocatorDefault, dict.as_concrete_TypeRef(), kCFPropertyListBinaryFormat_v1_0, 0, &mut err) };
    if data.is_null() {
        if !err.is_null() {
            unsafe { core_foundation_sys::base::CFRelease(err as _) };
        }
        return Err(format!("could not read the {domain} settings"));
    }
    let data = unsafe { CFData::wrap_under_create_rule(data) };
    Ok(Some(data.bytes().to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_foundation::boolean::CFBoolean;
    use core_foundation_sys::preferences::{CFPreferencesAppSynchronize, CFPreferencesSetAppValue};

    /// Removes the test key and flushes, even when the test fails.
    struct Cleanup(CFString);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let key = CFString::new("P2Check");
            unsafe {
                CFPreferencesSetAppValue(key.as_concrete_TypeRef(), std::ptr::null(), self.0.as_concrete_TypeRef());
                CFPreferencesAppSynchronize(self.0.as_concrete_TypeRef());
            }
        }
    }

    #[test]
    fn a_domain_reads_live() {
        let domain = format!("dev.termurion.p2test-{}", uuid::Uuid::new_v4());
        let app = CFString::new(&domain);
        let _guard = Cleanup(app.clone());
        let key = CFString::new("P2Check");
        unsafe {
            CFPreferencesSetAppValue(key.as_concrete_TypeRef(), CFBoolean::true_value().as_concrete_TypeRef() as _, app.as_concrete_TypeRef());
            CFPreferencesAppSynchronize(app.as_concrete_TypeRef());
        }
        let bytes = read_domain(&domain).unwrap().expect("the domain has a key");
        let root = plist::Value::from_reader(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(root.as_dictionary().and_then(|d| d.get("P2Check")).and_then(|v| v.as_boolean()), Some(true));
    }

    #[test]
    fn an_unknown_domain_is_empty() {
        assert_eq!(read_domain(&format!("dev.termurion.p2none-{}", uuid::Uuid::new_v4())).unwrap(), None);
    }
}

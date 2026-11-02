//! The user's locale and the system's list on Unix; the Terminal tab's status line is not read yet.

use super::super::Unsupported;
use super::super::locale::Status;

pub fn status(_mode: i64, _custom: &str, _ns_encoding: i64, _fallback: &str) -> Result<Status, Unsupported> {
    super::super::unsupported("The locale status line")
}

/// The locale the environment names, in POSIX order: `LC_ALL`, then `LC_CTYPE`, then `LANG`; the
/// first one that is set and not empty.
fn env_lang_from(get: &dyn Fn(&str) -> Option<String>) -> String {
    ["LC_ALL", "LC_CTYPE", "LANG"].iter().filter_map(|k| get(k)).find(|v| !v.is_empty()).unwrap_or_default()
}

fn env_lang() -> String {
    env_lang_from(&|k| std::env::var(k).ok())
}

/// The user's locale name: the environment's when it names one, else the system's. An app started
/// from the Dock or Finder gets no `LANG` from launchd, so the environment alone would say none.
fn lang_name() -> String {
    let env = env_lang();
    if super::super::locale::locale_from_lang_env(&env).is_some() {
        return env;
    }
    os::user_locale_name().unwrap_or(env)
}

/// `ll_CC` names out of a locale folder's entries (`en_US.UTF-8`, `en_US`, `UTF-8`, `C.UTF-8`):
/// the encoding is cut off, anything of another shape is dropped, each name once, sorted.
fn names_to_locales(names: &[String]) -> Vec<String> {
    let mut out: Vec<String> = names.iter().map(|n| n.split(['.', '@']).next().unwrap_or("").to_string()).filter(|b| is_ll_cc(b)).collect();
    out.sort();
    out.dedup();
    out
}

/// `ll_CC` (two or three lowercase letters, an underscore, two uppercase letters).
fn is_ll_cc(s: &str) -> bool {
    let Some((l, c)) = s.split_once('_') else { return false };
    (2..=3).contains(&l.len()) && l.bytes().all(|b| b.is_ascii_lowercase()) && c.len() == 2 && c.bytes().all(|b| b.is_ascii_uppercase())
}

/// The `ll`, `ll_CC` or `ll_CC.ENC` shape (the system's own list is not read on Linux).
fn looks_like_locale(candidate: &str) -> bool {
    let base = candidate.split('.').next().unwrap_or("").split('@').next().unwrap_or("");
    let language = base.split('_').next().unwrap_or("");
    (2..=3).contains(&language.len()) && language.bytes().all(|b| b.is_ascii_lowercase())
}

/// The names in `/usr/share/locale` (macOS) or none (Linux).
fn installed_names() -> &'static [String] {
    os::installed_names()
}

#[cfg(target_os = "macos")]
mod os {
    use std::ffi::c_char;
    use std::sync::OnceLock;

    use core_foundation_sys::base::{CFRelease, CFTypeRef};
    use core_foundation_sys::locale::{CFLocaleCopyCurrent, CFLocaleGetIdentifier};
    use core_foundation_sys::string::{kCFStringEncodingUTF8, CFStringGetCString};

    /// The identifier of the user's current locale (`en_US`), read through CoreFoundation.
    pub fn user_locale_name() -> Option<String> {
        // SAFETY: CFLocaleCopyCurrent returns an owned locale or null; the identifier is borrowed
        // from it and copied out before the locale is released; the buffer is as long as stated.
        unsafe {
            let locale = CFLocaleCopyCurrent();
            if locale.is_null() {
                return None;
            }
            let id = CFLocaleGetIdentifier(locale);
            let mut buf = [0 as c_char; 128];
            let ok = !id.is_null() && CFStringGetCString(id, buf.as_mut_ptr(), buf.len() as _, kCFStringEncodingUTF8) != 0;
            let name = ok.then(|| std::ffi::CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned());
            CFRelease(locale as CFTypeRef);
            name.filter(|n| !n.is_empty())
        }
    }

    /// The folder names of `/usr/share/locale`, read once.
    pub fn installed_names() -> &'static [String] {
        static NAMES: OnceLock<Vec<String>> = OnceLock::new();
        NAMES.get_or_init(|| {
            std::fs::read_dir("/usr/share/locale").map(|d| d.filter_map(|e| e.ok()?.file_name().into_string().ok()).collect()).unwrap_or_default()
        })
    }
}

#[cfg(not(target_os = "macos"))]
mod os {
    /// Linux has no per-user setting outside the environment.
    pub fn user_locale_name() -> Option<String> {
        None
    }

    pub fn installed_names() -> &'static [String] {
        &[]
    }
}

/// The user's locale as a BCP 47 tag.
pub fn user_locale() -> Option<String> {
    super::super::locale::locale_from_lang_env(&lang_name())
}

/// The user's locale (the environment's, else macOS's) stands for the user's side of the picture;
/// the system's list is read on macOS only.
pub fn facts(ns_encoding: i64, fallback: &str) -> crate::locale_guess::Facts {
    let lang = lang_name();
    let tag = super::super::locale::locale_from_lang_env(&lang);
    let current = lang.split(['.', '@']).next().unwrap_or("").to_string();
    let country = tag.as_deref().and_then(|t| t.rsplit('-').next()).filter(|c| c.len() == 2 && c.bytes().all(|b| b.is_ascii_uppercase())).map(str::to_string);
    crate::locale_guess::Facts {
        preferred: tag.into_iter().collect(),
        current,
        country,
        encoding: crate::locale_guess::encoding_name(ns_encoding).map(str::to_string),
        fallback_lc_ctype: fallback.to_string(),
    }
}

/// macOS: every `ll_CC` folder of `/usr/share/locale`. Linux: the environment's locale alone, as
/// the list of installed locales is not read there yet.
pub fn system_locales() -> Vec<String> {
    let names = installed_names();
    if !names.is_empty() {
        return names_to_locales(names);
    }
    let lang = lang_name();
    let base = lang.split(['.', '@']).next().unwrap_or("");
    (!base.is_empty() && base != "C" && base != "POSIX").then(|| base.to_string()).into_iter().collect()
}

/// No display names are read on Unix; the picker shows the codes.
pub fn names_for(_locales: &[String]) -> (std::collections::HashMap<String, String>, std::collections::HashMap<String, String>) {
    Default::default()
}

/// macOS: the name is a folder of `/usr/share/locale`, or the profile's encoding name. Linux: a
/// name of the `ll`, `ll_CC` or `ll_CC.ENC` shape, as the system's own list is not read there.
pub fn supported(candidate: &str, encoding: Option<&str>) -> bool {
    if encoding == Some(candidate) && !candidate.is_empty() {
        return true;
    }
    let names = installed_names();
    if names.is_empty() { looks_like_locale(candidate) } else { names.iter().any(|n| n == candidate) }
}

pub fn segmenter() -> Box<dyn crate::wordsel::Segmenter> {
    Box::new(crate::wordsel::NoSegmenter)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn the_environment_goes_lc_all_then_lc_ctype_then_lang() {
        assert_eq!(env_lang_from(&env(&[("LC_ALL", "de_DE.UTF-8"), ("LC_CTYPE", "fr_FR.UTF-8"), ("LANG", "en_US.UTF-8")])), "de_DE.UTF-8");
        assert_eq!(env_lang_from(&env(&[("LC_CTYPE", "fr_FR.UTF-8"), ("LANG", "en_US.UTF-8")])), "fr_FR.UTF-8");
        assert_eq!(env_lang_from(&env(&[("LANG", "en_US.UTF-8")])), "en_US.UTF-8");
    }

    #[test]
    fn an_empty_variable_is_skipped_and_none_set_is_empty() {
        assert_eq!(env_lang_from(&env(&[("LC_ALL", ""), ("LANG", "pt_BR")])), "pt_BR");
        assert_eq!(env_lang_from(&env(&[])), "");
    }

    #[test]
    fn locale_folder_names_become_one_sorted_list() {
        let names: Vec<String> = ["en_US.UTF-8", "en_US", "en_US.ISO8859-1", "de_DE.UTF-8", "UTF-8", "C.UTF-8", "ISO8859-1", "zh_Hans", "pt_BR"].iter().map(|s| s.to_string()).collect();
        assert_eq!(names_to_locales(&names), ["de_DE", "en_US", "pt_BR"]);
    }

    #[test]
    fn the_locale_shape_is_checked() {
        assert!(is_ll_cc("en_US") && is_ll_cc("fil_PH"));
        assert!(!is_ll_cc("en") && !is_ll_cc("EN_us") && !is_ll_cc("C") && !is_ll_cc("en_USA"));
        assert!(looks_like_locale("en_US.UTF-8") && looks_like_locale("fr") && !looks_like_locale("EN") && !looks_like_locale(""));
    }

    /// The Dock gives an app no `LANG`: the guess still finds a supported locale from the system.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_names_a_locale_without_lang() {
        let f = facts(4, "");
        assert!(!f.preferred.is_empty(), "the system names the user's locale");
        let found = crate::locale_guess::lang(&f, &|c| supported(c, Some("UTF-8")));
        assert!(found.is_some(), "a candidate is a folder of /usr/share/locale: {:?}", crate::locale_guess::candidates(&f));
        assert!(system_locales().len() > 10, "the list is read from /usr/share/locale");
    }
}

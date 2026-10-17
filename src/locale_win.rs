//! The Windows facts for the locale guesser (`locale_guess`): the user's preferred languages,
//! region and locale names, and whether Windows knows a locale (R6, Decisions "Locale on
//! Windows"). Windows cannot see which locales a WSL distribution has generated.

use crate::locale_guess::{self, Facts};
use std::collections::HashSet;
use std::sync::OnceLock;
use windows_sys::Win32::Globalization::{
    EnumSystemLocalesEx, GetLocaleInfoEx, GetUserDefaultLocaleName, GetUserPreferredUILanguages, LOCALE_SLOCALIZEDCOUNTRYNAME,
    LOCALE_SLOCALIZEDLANGUAGENAME, LOCALE_SPECIFICDATA, MUI_LANGUAGE_NAME,
};

/// `LOCALE_NAME_MAX_LENGTH`.
const NAME_MAX: usize = 85;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// The user's preferred UI languages, most preferred first (`en-US`).
fn preferred_languages() -> Vec<String> {
    let (mut count, mut len) = (0u32, 0u32);
    // SAFETY: the first call only asks for the buffer's length; the second fills a buffer of
    // that length.
    unsafe {
        if GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, std::ptr::null_mut(), &mut len) == 0 || len == 0 {
            return Vec::new();
        }
        let mut buf = vec![0u16; len as usize];
        if GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, buf.as_mut_ptr(), &mut len) == 0 {
            return Vec::new();
        }
        // A list of NUL-terminated names ended by an extra NUL.
        buf.split(|&c| c == 0).filter(|n| !n.is_empty()).map(String::from_utf16_lossy).collect()
    }
}

/// The user's default locale name (`en-US`).
fn default_locale_name() -> String {
    let mut buf = [0u16; NAME_MAX];
    // SAFETY: the buffer's length is passed.
    let n = unsafe { GetUserDefaultLocaleName(buf.as_mut_ptr(), NAME_MAX as i32) };
    if n <= 0 {
        return String::new();
    }
    from_wide(&buf)
}

/// The region of a locale name: its last subtag of two capital letters (`en-US` gives `US`).
fn region_of(name: &str) -> Option<String> {
    name.split('-').skip(1).collect::<Vec<_>>().into_iter().rev().find(|t| t.len() == 2 && t.bytes().all(|b| b.is_ascii_uppercase())).map(str::to_string)
}

/// The guesser's facts for a profile with encoding `ns_encoding` and Advanced's
/// `FallbackLCCType` `fallback`.
pub fn facts(ns_encoding: i64, fallback: &str) -> Facts {
    let name = default_locale_name();
    Facts {
        preferred: preferred_languages(),
        current: name.replace('-', "_"),
        country: region_of(&name),
        encoding: locale_guess::encoding_name(ns_encoding).map(str::to_string),
        fallback_lc_ctype: fallback.to_string(),
    }
}

/// Whether `candidate` (`ll_CC.ENC`, `ll_CC`, `ll` or just the encoding's name) is usable:
/// its language (script, country) part is a valid Windows locale name, its encoding anything,
/// or it is `encoding` alone.
pub fn supported(candidate: &str, encoding: Option<&str>) -> bool {
    if encoding == Some(candidate) && !candidate.is_empty() {
        return true;
    }
    let base = candidate.split('.').next().unwrap_or("");
    let base = base.split('@').next().unwrap_or("");
    // A language is two or three lowercase letters: this keeps `UTF-8` and `C` out.
    let language = base.split('_').next().unwrap_or("");
    if !(2..=3).contains(&language.len()) || !language.bytes().all(|b| b.is_ascii_lowercase()) {
        return false;
    }
    let (languages, specific) = known_locales();
    known_in(base, languages, specific)
}

/// Windows must have the language, or the language with that country, once a script subtag is
/// set aside (`IsValidLocaleName` accepts any well-formed name, `xx-QQ`, so it is not asked).
fn known_in(base: &str, languages: &HashSet<String>, specific: &HashSet<String>) -> bool {
    let parts: Vec<&str> = base.split('_').filter(|t| !is_script(t)).collect();
    match parts.as_slice() {
        [l] => languages.contains(*l),
        _ => specific.contains(&parts.join("_")),
    }
}

/// A script subtag: four letters (`Latn`).
fn is_script(t: &str) -> bool {
    t.len() == 4 && t.bytes().all(|b| b.is_ascii_alphabetic())
}

/// Windows' specific locales (`en_US`) and their languages (`en`), read once.
fn known_locales() -> &'static (HashSet<String>, HashSet<String>) {
    static KNOWN: OnceLock<(HashSet<String>, HashSet<String>)> = OnceLock::new();
    KNOWN.get_or_init(|| known_from(&system_locales()))
}

/// The specific locales (script subtags removed: `sr-Latn-RS` is `sr_RS`) and their languages.
fn known_from(list: &[String]) -> (HashSet<String>, HashSet<String>) {
    let specific: HashSet<String> = list.iter().map(|l| l.split('_').filter(|t| !is_script(t)).collect::<Vec<_>>().join("_")).collect();
    let languages = specific.iter().filter_map(|l| l.split('_').next()).map(str::to_string).collect();
    (languages, specific)
}

unsafe extern "system" fn collect(name: *const u16, _flags: u32, lparam: isize) -> i32 {
    // SAFETY: `lparam` is the `Vec` `system_locales` passed, alive for the whole enumeration;
    // `name` is NUL-terminated.
    let out = unsafe { &mut *(lparam as *mut Vec<String>) };
    let mut len = 0;
    while unsafe { *name.add(len) } != 0 {
        len += 1;
    }
    let text = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(name, len) });
    if !text.is_empty() {
        out.push(text.replace('-', "_"));
    }
    1
}

/// Every specific locale Windows has, as `ll_CC`.
pub fn system_locales() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    // SAFETY: the callback only runs during the call and `out` outlives it.
    unsafe { EnumSystemLocalesEx(Some(collect), LOCALE_SPECIFICDATA, &mut out as *mut Vec<String> as isize, std::ptr::null()) };
    out
}

/// `GetLocaleInfoEx` as text: `kind` for the locale named `name` (`en-US`).
fn info(name: &str, kind: u32) -> Option<String> {
    let name = wide(name);
    let mut buf = [0u16; 128];
    // SAFETY: `name` is NUL-terminated and the buffer's length is passed.
    let n = unsafe { GetLocaleInfoEx(name.as_ptr(), kind, buf.as_mut_ptr(), buf.len() as i32) };
    (n > 0).then(|| from_wide(&buf)).filter(|t| !t.is_empty())
}

/// The localized name of language `code` (`en`).
pub fn language_name(code: &str) -> Option<String> {
    plain_code(code).then(|| info(code, LOCALE_SLOCALIZEDLANGUAGENAME)).flatten()
}

/// The localized name of country `code` (`US`): asked of a locale of that region.
pub fn country_name(code: &str) -> Option<String> {
    if !plain_code(code) {
        return None;
    }
    let suffix = format!("_{code}");
    let locale = system_locales().into_iter().find(|l| l.ends_with(&suffix))?;
    info(&locale.replace('_', "-"), LOCALE_SLOCALIZEDCOUNTRYNAME)
}

/// The Terminal tab's locale line: the locale shown after `LANG=` (Custom and Automatic) and
/// the text (its title, or the status).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub lang: Option<String>,
    pub text: String,
}

/// The status for a profile's mode value, custom locale, encoding and `FallbackLCCType`.
pub fn status(mode: i64, custom: &str, ns_encoding: i64, fallback: &str) -> Status {
    let f = facts(ns_encoding, fallback);
    let name = locale_guess::encoding_name(ns_encoding);
    let supported = |c: &str| supported(c, name);
    let mode = locale_guess::Mode::from_value(mode);
    let custom = if custom.is_empty() || locale_guess::is_plain(custom) { custom } else { "" };
    let title = |l: &str| locale_guess::title(l, &language_name, &country_name);
    Status { lang: locale_guess::status_locale(mode, custom, &f, &supported), text: locale_guess::status(mode, custom, &f, &supported, &title) }
}

/// Localized language and country names for the locales (`ll_CC`) of a list, each read once:
/// (language code, name) and (country code, name).
pub fn names_for(locales: &[String]) -> (std::collections::HashMap<String, String>, std::collections::HashMap<String, String>) {
    let mut languages = std::collections::HashMap::new();
    let mut countries = std::collections::HashMap::new();
    for l in locales {
        let Some((lang, cc)) = l.split_once('_') else { continue };
        if !languages.contains_key(lang)
            && let Some(n) = language_name(lang) {
                languages.insert(lang.to_string(), n);
            }
        if !countries.contains_key(cc) && plain_code(cc)
            && let Some(n) = info(&l.replace('_', "-"), LOCALE_SLOCALIZEDCOUNTRYNAME) {
                countries.insert(cc.to_string(), n);
            }
    }
    (languages, countries)
}

/// A language or country code as locale names have it: letters, digits and `-` only.
fn plain_code(code: &str) -> bool {
    !code.is_empty() && code.len() <= 16 && code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_locale_facts_are_sane() {
        let f = facts(4, "");
        assert!(!f.preferred.is_empty());
        let first = &f.preferred[0];
        assert!(first.contains('-') || (2..=3).contains(&first.len()), "{first}");
        assert_eq!(f.encoding.as_deref(), Some("UTF-8"));
        assert!(!f.current.is_empty() && !f.current.contains('-'), "{}", f.current);
        assert!(supported("en_US.UTF-8", Some("UTF-8")));
        assert!(supported("en_US", Some("UTF-8")));
        assert!(supported("en", Some("UTF-8")));
        assert!(supported("UTF-8", Some("UTF-8")));
        assert!(!supported("xx_QQ.UTF-8", Some("UTF-8")));
        assert!(!supported("en_QQ.UTF-8", Some("UTF-8")));
        assert!(!supported("xx", Some("UTF-8")));
        assert!(supported("zh_Hans_CN.UTF-8", Some("UTF-8")));
        assert!(!supported("UTF-8", Some("ISO8859-1")));
        assert!(!supported("", Some("UTF-8")));
        assert!(system_locales().iter().any(|l| l == "en_US"));
        assert_eq!(language_name("en").as_deref().map(|n| n.is_empty()), Some(false));
        assert_eq!(country_name("US").as_deref().map(|n| n.is_empty()), Some(false));
        assert_eq!(country_name("QQ-not"), None);
        let with = facts(4, "C.UTF-8");
        assert_eq!(with.fallback_lc_ctype, "C.UTF-8");
        assert_eq!(facts(999, "").encoding, None);
    }

    #[test]
    fn script_locales_are_known_without_their_script() {
        let list = vec!["sr_Latn_RS".to_string()];
        let (languages, specific) = known_from(&list);
        assert!(known_in("sr_RS", &languages, &specific));
        assert!(known_in("sr_Latn_RS", &languages, &specific));
        assert!(known_in("sr", &languages, &specific));
        assert!(!known_in("sr_BA", &languages, &specific));
    }

    #[test]
    fn windows_names_and_status_resolve() {
        let (languages, countries) = names_for(&["en_US".to_string(), "de_DE".to_string()]);
        assert!(languages.get("en").is_some_and(|n| !n.is_empty()));
        assert!(countries.get("DE").is_some_and(|n| !n.is_empty()));
        let custom = status(2, "de_DE.UTF-8", 4, "");
        assert_eq!(custom.lang.as_deref(), Some("de_DE.UTF-8"));
        assert!(custom.text.ends_with(", UTF-8"), "{}", custom.text);
        assert_eq!(status(0, "", 4, "").text, locale_guess::STATUS_NOT_SET);
        assert_eq!(status(2, "", 4, "").text, locale_guess::STATUS_NO_LOCALE_SELECTED);
        assert_eq!(status(3, "", 4, "").text, "LC_CTYPE=UTF-8");
    }
}

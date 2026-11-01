//! The Terminal tab's locale line.

use super::Unsupported;

#[cfg(unix)]
use super::unix::locale as imp;
#[cfg(windows)]
use super::windows::locale as imp;

/// The Terminal tab's locale line: the locale shown after `LANG=` (Custom and Automatic) and
/// the text (its title, or the status).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub lang: Option<String>,
    pub text: String,
}

/// The status for a profile's mode value, custom locale, encoding and `FallbackLCCType`.
pub fn status(mode: i64, custom: &str, ns_encoding: i64, fallback: &str) -> Result<Status, Unsupported> {
    imp::status(mode, custom, ns_encoding, fallback)
}

/// The user's locale as a BCP 47 tag (`en-US`), or None when the system names none. Windows asks
/// for the user's default locale name; Unix reads `$LANG`.
pub fn user_locale() -> Option<String> {
    imp::user_locale()
}

/// A POSIX locale name (`en_US.UTF-8`, `de_DE@euro`, `pt_BR`) as a BCP 47 tag (`en-US`). None for
/// the unset, `C` and `POSIX` locales, which name no language.
pub fn locale_from_lang_env(lang: &str) -> Option<String> {
    let base = lang.split(['.', '@']).next().unwrap_or("");
    if base.is_empty() || base == "C" || base == "POSIX" {
        return None;
    }
    Some(base.replace('_', "-"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_from_lang_env() {
        assert_eq!(super::locale_from_lang_env("en_US.UTF-8").as_deref(), Some("en-US"));
        assert_eq!(super::locale_from_lang_env("de_DE@euro").as_deref(), Some("de-DE"));
        assert_eq!(super::locale_from_lang_env("pt_BR").as_deref(), Some("pt-BR"));
        assert_eq!(super::locale_from_lang_env("fr").as_deref(), Some("fr"));
        assert_eq!(super::locale_from_lang_env("C"), None);
        assert_eq!(super::locale_from_lang_env("POSIX"), None);
        assert_eq!(super::locale_from_lang_env(""), None);
        assert_eq!(super::locale_from_lang_env("C.UTF-8"), None);
    }
}

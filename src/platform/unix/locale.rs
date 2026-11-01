//! The locale line is not read on Unix yet.

use super::super::Unsupported;
use super::super::locale::Status;

pub fn status(_mode: i64, _custom: &str, _ns_encoding: i64, _fallback: &str) -> Result<Status, Unsupported> {
    super::super::unsupported("The locale status line")
}

/// `$LANG` as a BCP 47 tag.
pub fn user_locale() -> Option<String> {
    super::super::locale::locale_from_lang_env(&std::env::var("LANG").ok()?)
}

/// The user's locale from `$LANG` stands for the whole system list on Unix until `locale -a` is read.
pub fn facts(ns_encoding: i64, fallback: &str) -> crate::locale_guess::Facts {
    let lang = std::env::var("LANG").unwrap_or_default();
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

pub fn system_locales() -> Vec<String> {
    std::env::var("LANG").ok().and_then(|l| l.split(['.', '@']).next().map(str::to_string)).filter(|l| !l.is_empty() && l != "C" && l != "POSIX").into_iter().collect()
}

/// No display names are read on Unix; the picker shows the codes.
pub fn names_for(_locales: &[String]) -> (std::collections::HashMap<String, String>, std::collections::HashMap<String, String>) {
    Default::default()
}

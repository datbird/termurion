//! The locale line, from `locale_win`.

use super::super::Unsupported;
use super::super::locale::Status;

pub fn status(mode: i64, custom: &str, ns_encoding: i64, fallback: &str) -> Result<Status, Unsupported> {
    Ok(crate::locale_win::status(mode, custom, ns_encoding, fallback))
}

/// The user's default locale name (`en-US`).
pub fn user_locale() -> Option<String> {
    Some(crate::locale_win::default_locale_name()).filter(|n| !n.is_empty())
}

pub fn facts(ns_encoding: i64, fallback: &str) -> crate::locale_guess::Facts {
    crate::locale_win::facts(ns_encoding, fallback)
}

pub fn system_locales() -> Vec<String> {
    crate::locale_win::system_locales()
}

pub fn names_for(locales: &[String]) -> (std::collections::HashMap<String, String>, std::collections::HashMap<String, String>) {
    crate::locale_win::names_for(locales)
}

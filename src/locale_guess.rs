//! iTerm2's locale guesser (`iTermLocaleGuesser.swift`) over injected facts and a `supported`
//! predicate, and the four "Set locale variables" modes (`PS:3284-3331`). Pure: the Windows
//! facts come from `locale_win`.

/// What the guesser knows about the machine and the profile.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// The user's preferred languages in order, as BCP 47 (`en-US`, `zh-Hans-CN`).
    pub preferred: Vec<String>,
    /// The current locale's identifier (`en_US`).
    pub current: String,
    /// The region's country code (`US`), when there is one.
    pub country: Option<String>,
    /// The profile's encoding as an IANA-derived name (`UTF-8`), when it has one.
    pub encoding: Option<String>,
    /// Advanced > Terminal "FallbackLCCType".
    pub fallback_lc_ctype: String,
}

/// The name `LC_CTYPE` and the `.ENC` suffix use for an `NSStringEncoding` value
/// (`LG:174-217`): CoreFoundation's IANA name upper-cased unless iTerm2 lists it in lower case,
/// then `ISO-` as `ISO` and `EUC-` as `euc`. `None` for a value with no name.
pub fn encoding_name(ns_encoding: i64) -> Option<&'static str> {
    Some(match ns_encoding {
        1 => "US-ASCII",
        3 => "eucJP",
        4 => "UTF-8",
        5 => "ISO8859-1",
        8 => "SHIFT_JIS",
        9 => "ISO8859-2",
        10 => "UTF-16",
        11 => "windows-1251",
        12 => "windows-1252",
        13 => "windows-1253",
        14 => "windows-1254",
        15 => "windows-1250",
        21 => "ISO2022-JP",
        30 => "macintosh",
        0x8c00_0100 => "UTF-32",
        0x9000_0100 => "UTF-16BE",
        0x9400_0100 => "UTF-16LE",
        0x9800_0100 => "UTF-32BE",
        0x9c00_0100 => "UTF-32LE",
        _ => return None,
    })
}

/// Whether `s` is plain locale text: 1 to 64 of letters, digits and `_ . @ + -`. A locale name
/// goes into a session's environment, so nothing else (a NUL, `=`, a space) may reach it.
fn plain(s: &str) -> bool {
    (1..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.@+-".contains(&b))
}

/// iTerm2's candidate locales for `LANG`, in its order (`LG:260-287`).
pub fn candidates(f: &Facts) -> Vec<String> {
    // The preferred languages with their country removed (split at the first hyphen), kept up
    // to and including the first code of at most two bytes.
    let bare: Vec<&str> = f.preferred.iter().map(|l| l.split_once('-').map_or(l.as_str(), |(a, _)| a)).collect();
    let codes = match bare.iter().position(|c| c.len() <= 2) {
        Some(at) => &bare[..=at],
        None => &bare[..],
    };
    let with_country: Vec<String> = match &f.country {
        Some(cc) => codes.iter().map(|c| format!("{c}_{cc}")).collect(),
        None => Vec::new(),
    };
    let with_encoding = |v: &[String]| -> Vec<String> {
        match &f.encoding {
            Some(e) => v.iter().map(|l| format!("{l}.{e}")).collect(),
            None => Vec::new(),
        }
    };
    // All preferred languages, hyphenated ones first (stable), so en_US precedes en.
    let mut sorted: Vec<&String> = f.preferred.iter().collect();
    sorted.sort_by_key(|l| !l.contains('-'));
    let as_locales: Vec<String> = sorted.iter().map(|l| l.replace('-', "_")).collect();
    let mut out = with_encoding(&with_country);
    out.extend(with_country);
    out.extend(codes.iter().map(|c| c.to_string()));
    out.extend(with_encoding(&as_locales));
    out
}

/// `valueForLanguageEnvironmentVariable`: the first candidate `supported` accepts.
pub fn lang(f: &Facts, supported: &dyn Fn(&str) -> bool) -> Option<String> {
    candidates(f).into_iter().find(|c| plain(c) && supported(c))
}

/// `dictionaryWithLC_CTYPE`: `FallbackLCCType` when set (no support check), else the
/// encoding's name when `supported`.
pub fn lc_ctype(f: &Facts, supported: &dyn Fn(&str) -> bool) -> Option<String> {
    if plain(&f.fallback_lc_ctype) {
        return Some(f.fallback_lc_ctype.clone());
    }
    f.encoding.clone().filter(|e| plain(e) && supported(e))
}

/// The profile's "Set Local Environment Vars" (`AB:574-577`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    DoNotSet,
    Automatic,
    Custom,
    Minimal,
}

impl Mode {
    /// 0, 1, 2 and 3; anything else is Automatic (iTerm2's default).
    pub fn from_value(v: i64) -> Mode {
        match v {
            0 => Mode::DoNotSet,
            2 => Mode::Custom,
            3 => Mode::Minimal,
            _ => Mode::Automatic,
        }
    }
}

/// What a session start does about the locale.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Variables to set, in order.
    Vars(Vec<(String, String)>),
    /// Nothing to set.
    None,
    /// Automatic found no supported locale: iTerm2 asks the person (`PS:3322`).
    NeedsPrompt,
    /// A custom locale without "UTF-8" under a UTF-8 profile (`PS:3370-3390`).
    SuspectCustom(String),
}

/// The variables a session gets (`PS:3284-3331`). `utf8` is whether the profile's encoding is
/// UTF-8.
pub fn locale_vars(mode: Mode, custom: &str, f: &Facts, utf8: bool, supported: &dyn Fn(&str) -> bool) -> Outcome {
    let var = |name: &str, v: String| Outcome::Vars(vec![(name.to_string(), v)]);
    match mode {
        Mode::DoNotSet => Outcome::None,
        Mode::Minimal => lc_ctype(f, supported).map_or(Outcome::None, |v| var("LC_CTYPE", v)),
        // A custom locale that is not plain locale text reads as empty, never as a suspect
        // locale (B3); an empty one is Automatic.
        Mode::Custom if plain(custom) => {
            if utf8 && !custom.contains("UTF-8") {
                Outcome::SuspectCustom(custom.to_string())
            } else {
                var("LANG", custom.to_string())
            }
        }
        Mode::Custom | Mode::Automatic => lang(f, supported).map_or(Outcome::NeedsPrompt, |v| var("LANG", v)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(preferred: &[&str], country: Option<&str>, encoding: Option<&str>) -> Facts {
        Facts {
            preferred: preferred.iter().map(|s| s.to_string()).collect(),
            current: "en_US".into(),
            country: country.map(str::to_string),
            encoding: encoding.map(str::to_string),
            fallback_lc_ctype: String::new(),
        }
    }

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn candidates_follow_iterm2() {
        let a = facts(&["en-US", "fr"], Some("US"), Some("UTF-8"));
        assert_eq!(candidates(&a), strs(&["en_US.UTF-8", "en_US", "en", "en_US.UTF-8", "fr.UTF-8"]));
        let b = facts(&["zh-Hans-CN", "en-GB"], Some("CN"), Some("UTF-8"));
        assert_eq!(candidates(&b), strs(&["zh_CN.UTF-8", "zh_CN", "zh", "zh_Hans_CN.UTF-8", "en_GB.UTF-8"]));
        let c = facts(&["fil-PH", "tl"], Some("PH"), Some("UTF-8"));
        assert_eq!(candidates(&c), strs(&["fil_PH.UTF-8", "tl_PH.UTF-8", "fil_PH", "tl_PH", "fil", "tl", "fil_PH.UTF-8", "tl.UTF-8"]));
        let no_country = facts(&["en-US", "fr"], None, Some("UTF-8"));
        assert_eq!(candidates(&no_country), strs(&["en", "en_US.UTF-8", "fr.UTF-8"]));
        let no_encoding = facts(&["en-US", "fr"], Some("US"), None);
        assert_eq!(candidates(&no_encoding), strs(&["en_US", "en"]));
        // No code of two letters or fewer: every language is kept.
        let long = facts(&["fil-PH", "yue-HK"], Some("PH"), None);
        assert_eq!(candidates(&long), strs(&["fil_PH", "yue_PH", "fil", "yue"]));
        assert_eq!(candidates(&Facts::default()), Vec::<String>::new());
        assert_eq!(lang(&b, &|c| c == "en_GB.UTF-8").as_deref(), Some("en_GB.UTF-8"));
        assert_eq!(lang(&b, &|_| false), None);
        assert_eq!(lang(&a, &|_| true).as_deref(), Some("en_US.UTF-8"));
    }

    #[test]
    fn modes_follow_iterm2() {
        let all = |_: &str| true;
        let none = |_: &str| false;
        let f = facts(&["en-US", "fr"], Some("US"), Some("UTF-8"));
        let vars = |l: &[(&str, &str)]| Outcome::Vars(l.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect());
        assert_eq!(locale_vars(Mode::Automatic, "", &f, true, &all), vars(&[("LANG", "en_US.UTF-8")]));
        assert_eq!(locale_vars(Mode::DoNotSet, "", &f, true, &all), Outcome::None);
        assert_eq!(locale_vars(Mode::Minimal, "", &f, true, &all), vars(&[("LC_CTYPE", "UTF-8")]));
        let fallback = Facts { fallback_lc_ctype: "C.UTF-8".into(), ..f.clone() };
        assert_eq!(locale_vars(Mode::Minimal, "", &fallback, true, &none), vars(&[("LC_CTYPE", "C.UTF-8")]), "the fallback wins without a check");
        assert_eq!(locale_vars(Mode::Custom, "de_DE.UTF-8", &f, true, &all), vars(&[("LANG", "de_DE.UTF-8")]));
        assert_eq!(locale_vars(Mode::Custom, "de_DE", &f, true, &all), Outcome::SuspectCustom("de_DE".into()));
        assert_eq!(locale_vars(Mode::Custom, "de_DE", &f, false, &all), vars(&[("LANG", "de_DE")]));
        assert_eq!(locale_vars(Mode::Custom, "", &f, true, &all), vars(&[("LANG", "en_US.UTF-8")]), "an empty custom locale is Automatic");
        assert_eq!(locale_vars(Mode::Automatic, "", &f, true, &none), Outcome::NeedsPrompt);
        assert_eq!(locale_vars(Mode::Custom, "", &f, true, &none), Outcome::NeedsPrompt);
        assert_eq!(locale_vars(Mode::Minimal, "", &f, true, &none), Outcome::None);
        assert_eq!(Mode::from_value(0), Mode::DoNotSet);
        assert_eq!(Mode::from_value(1), Mode::Automatic);
        assert_eq!(Mode::from_value(2), Mode::Custom);
        assert_eq!(Mode::from_value(3), Mode::Minimal);
        assert_eq!(Mode::from_value(7), Mode::Automatic);
        assert_eq!(Mode::from_value(-1), Mode::Automatic);
    }

    #[test]
    fn custom_locale_is_validated() {
        // B3: a custom locale that is not plain locale text never reaches the environment, and
        // is never a suspect locale: it reads as empty, so Automatic runs.
        let f = facts(&["en-US"], Some("US"), Some("UTF-8"));
        let all = |_: &str| true;
        let auto = Outcome::Vars(vec![("LANG".into(), "en_US.UTF-8".into())]);
        for bad in ["x\u{0}PATH=C:\\evil", "a=b", "de DE", "\u{e9}", "a\nb", &"a".repeat(65)] {
            assert_eq!(locale_vars(Mode::Custom, bad, &f, true, &all), auto, "{bad:?}");
            assert_eq!(locale_vars(Mode::Custom, bad, &f, false, &all), auto, "{bad:?}");
        }
        let ok = "a".repeat(64);
        assert_eq!(locale_vars(Mode::Custom, &ok, &f, false, &all), Outcome::Vars(vec![("LANG".into(), ok.clone())]));
        assert_eq!(locale_vars(Mode::Custom, "sr_RS@latin+x.UTF-8", &f, true, &all), Outcome::Vars(vec![("LANG".into(), "sr_RS@latin+x.UTF-8".into())]));
    }

    #[test]
    fn fallback_lc_ctype_is_validated() {
        let all = |_: &str| true;
        for bad in ["C.UTF-8\u{0}PATH=x", "a=b", "x y"] {
            let f = Facts { fallback_lc_ctype: bad.into(), ..facts(&["en-US"], Some("US"), Some("UTF-8")) };
            assert_eq!(lc_ctype(&f, &all).as_deref(), Some("UTF-8"), "{bad:?}: ignored, the encoding name is used");
            assert_eq!(locale_vars(Mode::Minimal, "", &f, true, &all), Outcome::Vars(vec![("LC_CTYPE".into(), "UTF-8".into())]));
        }
    }

    #[test]
    fn encoding_names_follow_iterm2() {
        for (v, n) in [
            (4, "UTF-8"),
            (1, "US-ASCII"),
            (5, "ISO8859-1"),
            (9, "ISO8859-2"),
            (10, "UTF-16"),
            (12, "windows-1252"),
            (30, "macintosh"),
            (3, "eucJP"),
            (8, "SHIFT_JIS"),
            (21, "ISO2022-JP"),
        ] {
            assert_eq!(encoding_name(v), Some(n), "{v}");
        }
        assert_eq!(encoding_name(999), None);
        assert_eq!(encoding_name(-4), None);
        // Every name is plain locale text, as the environment needs.
        for v in 0..64 {
            if let Some(n) = encoding_name(v) {
                assert!(n.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.@+-".contains(&b)), "{n}");
            }
        }
    }
}

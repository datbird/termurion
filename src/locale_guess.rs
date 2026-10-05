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
        // 13o's CoreFoundation-derived choices (P18): CF's IANA names, `Big5` kept in mixed
        // case (iTerm2's EncodingsWithLowerCase.plist lists it), `EUC-KR` as `eucKR`.
        0x8000_0632 => "GB18030",
        0x8000_0A03 => "Big5",
        0x8000_0940 => "eucKR",
        0x8000_0A02 => "KOI8-R",
        _ => return None,
    })
}

/// Whether `s` is plain locale text: 1 to 64 of letters, digits and `_ . @ + -`. A locale name
/// goes into a session's environment, so nothing else (a NUL, `=`, a space) may reach it.
pub fn is_plain(s: &str) -> bool {
    plain(s)
}

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
            if is_suspect(utf8, custom) {
                Outcome::SuspectCustom(custom.to_string())
            } else {
                var("LANG", custom.to_string())
            }
        }
        Mode::Custom | Mode::Automatic => lang(f, supported).map_or(Outcome::NeedsPrompt, |v| var("LANG", v)),
    }
}

/// One locale of the picker (`LocaleComponents`, `LP:197-245`): its identifier, its title, its
/// language code and whether its encoding is UTF-8.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub locale: String,
    pub title: String,
    pub language: String,
    pub utf8: bool,
}

/// iTerm2's `LocaleComponents` split at `separator`: the language, the country and the encoding.
/// Anything that is not `ll<sep>CC[.enc]` is all language (`C`, `POSIX`).
fn split(locale: &str, separator: char) -> (String, Option<String>, Option<String>) {
    let mut parts_enc = locale.split('.');
    let lc = parts_enc.next().unwrap_or("C");
    let encoding = parts_enc.next().map(str::to_string);
    let parts: Vec<&str> = lc.split(separator).collect();
    if parts.len() != 2 {
        return (locale.to_string(), None, None);
    }
    (parts[0].to_string(), Some(parts[1].to_string()), encoding)
}

/// `LocaleComponents(locale)`: language, country, encoding.
pub fn components(locale: &str) -> (String, Option<String>, Option<String>) {
    split(locale, '_')
}

/// A locale's title (`LP:197-245`): "Language (Country), ENC", "Language (Country)",
/// "Language, ENC" or "Language"; the country only with a localized name for it, the language
/// code itself when it has none.
pub fn title(locale: &str, language_name: &dyn Fn(&str) -> Option<String>, country_name: &dyn Fn(&str) -> Option<String>) -> String {
    let (language, country, encoding) = components(locale);
    let shown = language_name(&language).unwrap_or(language);
    let country = country.and_then(|c| country_name(&c));
    match (country, encoding) {
        (Some(c), Some(e)) => format!("{shown} ({c}), {e}"),
        (Some(c), None) => format!("{shown} ({c})"),
        (None, Some(e)) => format!("{shown}, {e}"),
        (None, None) => shown,
    }
}

/// The picker's two sections (`LP:66-121`): every entry sorted by title, those whose encoding
/// is not UTF-8 apart (below a disabled header) when the profile is UTF-8, and which entry is
/// selected first: `(in bad, index)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picker {
    pub good: Vec<Entry>,
    pub bad: Vec<Entry>,
    pub default: Option<(bool, usize)>,
}

impl Picker {
    /// `locales` as the machine lists them, `utf8` whether the profile is UTF-8, `preferred` the
    /// user's languages (`de-AT`), most preferred first, `names` the localized language and
    /// country names.
    pub fn new(locales: &[String], utf8: bool, preferred: &[String], names: (&dyn Fn(&str) -> Option<String>, &dyn Fn(&str) -> Option<String>)) -> Picker {
        let mut entries: Vec<Entry> = locales
            .iter()
            .map(|l| {
                let (language, _, encoding) = components(l);
                Entry { locale: l.clone(), title: title(l, names.0, names.1), language, utf8: encoding.as_deref() == Some("UTF-8") }
            })
            .collect();
        entries.sort_by(|a, b| a.title.cmp(&b.title));
        let (good, bad): (Vec<Entry>, Vec<Entry>) = entries.into_iter().partition(|e| !utf8 || e.utf8);
        // The default is the first entry whose language is the most preferred one.
        let rank = |language: &str| preferred.iter().position(|p| split(p, '-').0 == language);
        let mut default = None;
        let mut best = usize::MAX;
        for (is_bad, list) in [(false, &good), (true, &bad)] {
            for (i, e) in list.iter().enumerate() {
                if let Some(q) = rank(&e.language).filter(|q| *q < best) {
                    best = q;
                    default = Some((is_bad, i));
                }
            }
        }
        Picker { good, bad, default }
    }

    /// The entry at `(in bad, index)`.
    pub fn entry(&self, at: (bool, usize)) -> Option<&Entry> {
        if at.0 { self.bad.get(at.1) } else { self.good.get(at.1) }
    }

    /// Where `locale` is: `(in bad, index)`.
    pub fn find(&self, locale: &str) -> Option<(bool, usize)> {
        self.good.iter().position(|e| e.locale == locale).map(|i| (false, i)).or_else(|| self.bad.iter().position(|e| e.locale == locale).map(|i| (true, i)))
    }
}

/// The picker's list on a machine with no `locale -a` (R6): each specific locale `ll_CC` as
/// `ll_CC.UTF-8` and as `ll_CC`.
pub fn picker_locales(system: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in system {
        for v in [format!("{l}.UTF-8"), l.clone()] {
            if !out.contains(&v) {
                out.push(v);
            }
        }
    }
    out
}

/// The locale a Custom or Automatic profile shows in Settings (`PTM:397-445`): the custom
/// locale, or the guess.
pub fn status_locale(mode: Mode, custom: &str, f: &Facts, supported: &dyn Fn(&str) -> bool) -> Option<String> {
    match mode {
        Mode::Custom => Some(custom.to_string()).filter(|c| !c.is_empty()),
        Mode::Automatic => lang(f, supported),
        _ => None,
    }
}

/// The Terminal tab's line under "Set Locale Variables" (`PTM:397-445`), without the `LANG=`
/// prefix Settings draws before a locale's title.
pub fn status(mode: Mode, custom: &str, f: &Facts, supported: &dyn Fn(&str) -> bool, title: &dyn Fn(&str) -> String) -> String {
    match mode {
        Mode::DoNotSet => STATUS_NOT_SET.to_string(),
        Mode::Minimal => match lc_ctype(f, supported) {
            Some(v) => format!("LC_CTYPE={v}"),
            None => STATUS_INVALID_ENCODING.to_string(),
        },
        Mode::Custom => status_locale(mode, custom, f, supported).map_or_else(|| STATUS_NO_LOCALE_SELECTED.to_string(), |l| title(&l)),
        Mode::Automatic => status_locale(mode, custom, f, supported).map_or_else(|| STATUS_NO_VALID_LOCALE.to_string(), |l| title(&l)),
    }
}

pub const STATUS_NOT_SET: &str = "$LANG will not be set.";
pub const STATUS_INVALID_ENCODING: &str = "Invalid encoding: neither $LC_CTYPE nor $LANG set.";
// iTerm2's text for an LC_CTYPE that vanishes between two reads; Tandem reads it once, so
// nothing shows it.
#[allow(dead_code)]
pub const STATUS_MISSING_CTYPE: &str = "Unexpectedly missing LC_CTYPE. Report a bug.";
pub const STATUS_NO_LOCALE_SELECTED: &str = "No locale selected.";
pub const STATUS_NO_VALID_LOCALE: &str = "No valid locale exists for this machine\u{2019}s language and country.";

/// The start-time prompt's message (`iTermLocalePrompt.swift`).
pub const PROMPT_MESSAGE: &str = "No valid UNIX locale exists for your computer\u{2019}s current language and country. This may cause command-line apps to misbehave. Please select one from the list below.";
/// Change Locale's message in Settings (`PTM:508-541`).
pub const SELECT_MESSAGE: &str = "Select your preferred locale:";
pub const USE_MINIMAL: &str = "Use Minimal POSIX Locale";
pub const WRONG_ENCODING_SECTION: &str = "Locales with wrong encoding";

/// The prompt's remember box for a profile.
pub fn save_to_profile(name: &str) -> String {
    format!("Save selection to profile {name}")
}

pub const MISMATCH_HEADING: &str = "Wrong Encoding Detected";
pub const MISMATCH_TEXT: &str = "Warning! This profile uses a custom locale that doesn't use UTF-8 as its character encoding, but your profile *is* using UTF-8. This can cause error messages and non-ASCII text to appear wrong.";
pub const CHANGE_LOCALE: &str = "Change Locale";
pub const KEEP_LOCALE: &str = "Keep This Locale";

/// Whether a custom `locale` under a UTF-8 profile draws the mismatch warning (`PS:3370`).
pub fn is_suspect(utf8: bool, locale: &str) -> bool {
    utf8 && !locale.contains("UTF-8")
}

/// The mismatch warning's silence identifier for a profile (`PS:3376`).
pub fn mismatch_id(guid: &str) -> String {
    format!("NoSyncUTF8Mismatch_{guid}")
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

    /// P18 (13o x 13p): every encoding the profile can name, 13o's `encoding::CHOICES` and the
    /// schema's Character Encoding choices, has a locale name, so no choice guesses a UTF-8
    /// locale; the two tables list the same values; only 4 is UTF-8.
    #[test]
    fn every_encoding_choice_has_a_locale_name() {
        for (v, label, _) in crate::encoding::CHOICES {
            let n = encoding_name(*v).unwrap_or_else(|| panic!("{label} ({v:#x}) has no locale name"));
            assert!(is_plain(n), "{n}");
            assert_eq!(n == "UTF-8", *v == crate::encoding::UTF8, "{label}");
        }
        let schema: toml::Value = toml::from_str(include_str!("../schema/profile.toml")).unwrap();
        let row = schema["setting"].as_array().unwrap().iter().find(|r| r["key"].as_str() == Some("Character Encoding")).unwrap();
        let mut listed: Vec<i64> = row["choices"].as_array().unwrap().iter().map(|c| c["value"].as_integer().unwrap()).collect();
        let mut table: Vec<i64> = crate::encoding::CHOICES.iter().map(|c| c.0).collect();
        listed.sort_unstable();
        table.sort_unstable();
        assert_eq!(listed, table, "the popup and 13o's table agree");
        for (v, n) in [(0x8000_0632, "GB18030"), (0x8000_0A03, "Big5"), (0x8000_0940, "eucKR"), (0x8000_0A02, "KOI8-R")] {
            assert_eq!(encoding_name(v), Some(n));
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

    fn german(c: &str) -> Option<String> {
        match c {
            "de" => Some("German".into()),
            "en" => Some("English".into()),
            _ => None,
        }
    }

    fn countries(c: &str) -> Option<String> {
        match c {
            "DE" => Some("Germany".into()),
            "US" => Some("United States".into()),
            _ => None,
        }
    }

    fn nothing(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn locale_status_follows_iterm2() {
        let f = facts(&["en-US"], Some("US"), Some("UTF-8"));
        let all = |_: &str| true;
        let none = |_: &str| false;
        let t = |l: &str| title(l, &german, &countries);
        assert_eq!(status(Mode::DoNotSet, "", &f, &all, &t), "$LANG will not be set.");
        assert_eq!(status(Mode::Custom, "", &f, &all, &t), "No locale selected.");
        assert_eq!(status(Mode::Custom, "de_DE.UTF-8", &f, &all, &t), "German (Germany), UTF-8");
        assert_eq!(status(Mode::Automatic, "", &f, &all, &t), "English (United States), UTF-8");
        assert_eq!(status(Mode::Automatic, "", &f, &none, &t), "No valid locale exists for this machine\u{2019}s language and country.");
        assert_eq!(status(Mode::Minimal, "", &f, &all, &t), "LC_CTYPE=UTF-8");
        assert_eq!(status(Mode::Minimal, "", &f, &none, &t), "Invalid encoding: neither $LC_CTYPE nor $LANG set.");
        assert_eq!(status_locale(Mode::Custom, "de_DE", &f, &all).as_deref(), Some("de_DE"));
        assert_eq!(status_locale(Mode::Minimal, "de_DE", &f, &all), None);
    }

    #[test]
    fn picker_follows_iterm2() {
        let list = picker_locales(&strs(&["en_US", "de_DE"]));
        assert_eq!(list, strs(&["en_US.UTF-8", "en_US", "de_DE.UTF-8", "de_DE"]));
        assert_eq!(components("C"), ("C".to_string(), None, None));
        assert_eq!(components("en_US.UTF-8"), ("en".to_string(), Some("US".to_string()), Some("UTF-8".to_string())));
        assert_eq!(title("xx_YY", &nothing, &nothing), "xx");
        assert_eq!(title("en_US", &german, &countries), "English (United States)");
        assert_eq!(title("de_DE.UTF-8", &german, &nothing), "German, UTF-8");
        let preferred = strs(&["de-AT", "en-US"]);
        // Without localized names the titles are the language codes, so German sorts first and
        // is the default (the most preferred language).
        let p = Picker::new(&list, true, &preferred, (&nothing, &nothing));
        let good: Vec<&str> = p.good.iter().map(|e| e.locale.as_str()).collect();
        let bad: Vec<&str> = p.bad.iter().map(|e| e.locale.as_str()).collect();
        assert_eq!(good, ["de_DE.UTF-8", "en_US.UTF-8"]);
        assert_eq!(bad, ["de_DE", "en_US"]);
        assert_eq!(p.default, Some((false, 0)));
        // With names iTerm2 sorts by the shown title: English before German.
        let q = Picker::new(&list, true, &preferred, (&german, &countries));
        assert_eq!(q.good.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), ["English (United States), UTF-8", "German (Germany), UTF-8"]);
        assert_eq!(q.default, Some((false, 1)));
        assert_eq!(q.find("en_US"), Some((true, 0)));
        // A non-UTF-8 profile has no bad section.
        let r = Picker::new(&list, false, &preferred, (&nothing, &nothing));
        assert_eq!(r.good.len(), 4);
        assert!(r.bad.is_empty());
        // A preferred language only the wrong-encoding section has still defaults there.
        let only_bad = Picker::new(&strs(&["fr_FR"]), true, &strs(&["fr-FR"]), (&nothing, &nothing));
        assert_eq!(only_bad.default, Some((true, 0)));
    }

    #[test]
    fn mismatch_warning_follows_iterm2() {
        assert_eq!(MISMATCH_HEADING, "Wrong Encoding Detected");
        assert_eq!(MISMATCH_TEXT, "Warning! This profile uses a custom locale that doesn't use UTF-8 as its character encoding, but your profile *is* using UTF-8. This can cause error messages and non-ASCII text to appear wrong.");
        assert_eq!(CHANGE_LOCALE, "Change Locale");
        assert_eq!(KEEP_LOCALE, "Keep This Locale");
        assert_eq!(mismatch_id("G"), "NoSyncUTF8Mismatch_G");
    }

    #[test]
    fn start_prompt_follows_iterm2() {
        assert_eq!(PROMPT_MESSAGE, "No valid UNIX locale exists for your computer\u{2019}s current language and country. This may cause command-line apps to misbehave. Please select one from the list below.");
        assert_eq!(SELECT_MESSAGE, "Select your preferred locale:");
        assert_eq!(USE_MINIMAL, "Use Minimal POSIX Locale");
        assert_eq!(WRONG_ENCODING_SECTION, "Locales with wrong encoding");
        assert_eq!(save_to_profile("Work"), "Save selection to profile Work");
        assert_eq!(STATUS_MISSING_CTYPE, "Unexpectedly missing LC_CTYPE. Report a bug.");
    }
}

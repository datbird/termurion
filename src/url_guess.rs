//! iTerm2's URL extraction and guessing for Semantic History: the URL and filename
//! character sets (`NCS:453-478`), the run around a click (`substringIncludingOffset`,
//! `NSS:1043`), the URL inside a run (`rangeOfURLInString`, `NSS:1100`) and the
//! conservative and liberal guesses (`UAF:698-770`). Pure: no disk, no network.

use std::ops::Range;

/// `URLCharacterSet`'s default (`AS:615`): one backslash, fullwidth parentheses, `|`, `[]`.
pub const DEFAULT_URL_CHARS: &str = ".?\\/:;$%=&_-,+~#@!*'(\u{ff08}\u{ff09}|[]";
/// `URLCharacterSetExclusions`' default (`AS:616`).
pub const DEFAULT_URL_EXCLUSIONS: &str = "\u{ac}";
/// `FilenameCharacterSet`'s default (`AS:617`).
pub const DEFAULT_FILENAME_CHARS: &str = "$@";
/// `TrailingPunctuationMarks`' default (`AS:623`).
pub const DEFAULT_TRAILING: &str = "!?\u{2026})].\"';:,";
/// The schemes a guessed URL may carry: the ones `semantic::guard_open` can hand on, plus
/// ssh (B1: a colon string with any other scheme is no URL, `ms-settings:` included).
const GUESS_SCHEMES: &[&str] = &["http", "https", "mailto", "ftp", "ssh"];

/// A set of characters: letters, digits and iTerm2's IDN additions, plus `extra`, less `excluded`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CharSet {
    extra: Vec<char>,
    excluded: Vec<char>,
    /// The filename set adds blanks (`whitespaceCharacterSet`).
    blanks: bool,
}

/// `NCS:466-478`: the configured characters, letters and digits, `'` `-` `.` `:` (the IDN
/// additions), less the exclusions. Tandem takes every Unicode letter and digit for the IDN ranges.
pub fn url(chars: &str, exclusions: &str) -> CharSet {
    CharSet { extra: chars.chars().collect(), excluded: exclusions.chars().collect(), blanks: false }
}

/// `NCS:453-462`: blanks, the URL set, and `extra`. A character `extra` names is in the set
/// even if the URL set excludes it (iTerm2 unions after removing).
pub fn filename(url: &CharSet, extra: &str) -> CharSet {
    let more: Vec<char> = extra.chars().collect();
    let mut set = url.clone();
    set.excluded.retain(|c| !more.contains(c));
    set.extra.extend(more);
    set.blanks = true;
    set
}

impl CharSet {
    pub fn contains(&self, c: char) -> bool {
        if self.excluded.contains(&c) {
            return false;
        }
        c.is_alphanumeric() || "'-.:".contains(c) || self.extra.contains(&c) || (self.blanks && is_blank(c))
    }
}

impl Default for CharSet {
    fn default() -> CharSet {
        url(DEFAULT_URL_CHARS, DEFAULT_URL_EXCLUSIONS)
    }
}

/// `NSCharacterSet.whitespaceCharacterSet`: spaces and tab, no line breaks.
fn is_blank(c: char) -> bool {
    c == '\t' || (c.is_whitespace() && !matches!(c, '\n' | '\r' | '\u{b}' | '\u{c}' | '\u{85}' | '\u{2028}' | '\u{2029}'))
}

/// `substringIncludingOffset` (`NSS:1043`): the byte range of the run of set characters
/// that contains `offset`. A character at `offset` outside the set ends the run after it, so the
/// run before the offset is returned (as iTerm2 does).
pub fn around(s: &str, offset: usize, set: &CharSet) -> (usize, usize) {
    let start = s[..offset].char_indices().rev().find(|(_, c)| !set.contains(*c)).map_or(0, |(i, c)| i + c.len_utf8());
    let end = s[offset..].char_indices().find(|(_, c)| !set.contains(*c)).map_or(s.len(), |(i, _)| offset + i);
    (start, end)
}

/// `stringByRemovingEnclosingPunctuationMarks` (`NSS:1078`).
fn without_enclosing(s: &str) -> &str {
    let Some(first) = s.chars().next() else { return s };
    let closer = match first {
        '(' => ')',
        '[' => ']',
        '"' => '"',
        '\'' => '\'',
        _ => return s,
    };
    let rest = &s[first.len_utf8()..];
    rest.find(closer).map_or(rest, |i| &rest[..i])
}

/// `rangeOfURLInString` (`NSS:1100-1142`): the URL inside `s`, with enclosing punctuation,
/// one trailing mark and leading hyphens gone.
pub fn range_of_url(s: &str, trailing: &str) -> Option<Range<usize>> {
    let mut t = s.trim_matches(|c: char| c.is_whitespace());
    if t.is_empty() {
        return None;
    }
    if let Some(colon) = t.find(':') {
        // The scheme starts after the last character before the colon that is not a
        // letter, digit or hyphen.
        if let Some((i, _)) = t[..colon].char_indices().rev().find(|(_, c)| !(c.is_alphanumeric() || *c == '-')) {
            t = &t[i..];
            let stripped = without_enclosing(t);
            t = if stripped.len() == t.len() {
                // `*http://example.com`: one character goes.
                &t[t.chars().next().map_or(0, char::len_utf8)..]
            } else {
                stripped
            };
        }
    } else {
        t = without_enclosing(t);
    }
    if t.is_empty() {
        return None;
    }
    if let Some(last) = t.chars().next_back().filter(|c| trailing.contains(*c)) {
        t = &t[..t.len() - last.len_utf8()];
    }
    t = t.trim_start_matches('-');
    if t.is_empty() {
        return None;
    }
    let at = s.find(t)?;
    Some(at..at + t.len())
}

/// Everything the guess reads from the Advanced settings and the pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UrlPrefs {
    pub chars: CharSet,
    pub trailing: String,
    pub conservative: bool,
    pub require_slash: bool,
    pub default_scheme: String,
    pub cwd_is_local: bool,
}

impl Default for UrlPrefs {
    fn default() -> UrlPrefs {
        UrlPrefs {
            chars: CharSet::default(),
            trailing: DEFAULT_TRAILING.to_string(),
            conservative: false,
            require_slash: true,
            default_scheme: "https".to_string(),
            cwd_is_local: true,
        }
    }
}

/// One hostname label: letters, digits and inner hyphens.
fn label(l: &str) -> bool {
    let b = l.as_bytes();
    !b.is_empty() && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-') && b[0] != b'-' && b[b.len() - 1] != b'-'
}

/// A hostname of two or more labels (`UAF:931`).
fn dotted_host(s: &str) -> bool {
    let labels: Vec<&str> = s.split('.').collect();
    labels.len() >= 2 && labels.iter().all(|l| label(l))
}

/// A dotted quad (`UAF:924`).
fn dotted_quad(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 4 && parts.iter().all(|p| !p.is_empty() && p.len() <= 3 && p.bytes().all(|b| b.is_ascii_digit()) && (p.len() == 1 || !p.starts_with('0')) && p.parse::<u16>().is_ok_and(|n| n <= 255))
}

/// `stringLooksLikeURL` (`UAF:896-941`).
pub fn looks_like_url(s: &str, require_slash: bool, chars: &CharSet) -> bool {
    if s.is_empty() || s.chars().any(|c| !chars.contains(c)) {
        return false;
    }
    match s.find('/') {
        Some(0) => false,
        Some(_) => true,
        None => !require_slash && (dotted_quad(s) || dotted_host(s)),
    }
}

/// The conservative pattern (`UAF:731-737`): a lower-case scheme, a hostname of two or more
/// labels, an optional port, then a slash or the end.
fn conservative_shape(s: &str) -> bool {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r"^[a-z]+://(([a-zA-Z0-9]|[a-zA-Z0-9][a-zA-Z0-9\-]*[a-zA-Z0-9])\.)+([A-Za-z0-9]|[A-Za-z0-9][A-Za-z0-9\-]*[A-Za-z0-9])(:[1-9][0-9]{0,4})?(/|$)").expect("pattern")
    });
    re.is_match(s)
}

/// `stringIsSingleDomainWord` (`UAF:800-817`): ASCII letters and digits, not starting with a digit.
pub fn single_domain_word(s: &str) -> bool {
    s.bytes().next().is_some_and(|b| !b.is_ascii_digit()) && s.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// The lower-cased scheme of a string with a colon, when a letter starts it and only
/// scheme characters follow.
fn colon_scheme(s: &str) -> Option<String> {
    let head = s.split_once(':')?.0;
    (head.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) && head.chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))).then(|| head.to_ascii_lowercase())
}

/// The URL to open at a click and its byte range in `before + after` (`UAF:605-627`, `:698-770`).
pub fn guess(before: &str, after: &str, p: &UrlPrefs) -> Option<(String, Range<usize>)> {
    let joined = format!("{before}{after}");
    let (s, e) = around(&joined, before.len(), &p.chars);
    let run = &joined[s..e];
    let r = range_of_url(run, &p.trailing)?;
    let text = &run[r.clone()];
    let range = s + r.start..s + r.end;
    // Path-shaped text (a share, a drive path, any mix of separators in the host) is a path for
    // the file route and the network filter, never a web address (a browser reads `\` as `/`).
    let host_part = text.split('/').next().unwrap_or(text);
    if crate::scripts::is_network_path(text) || text.starts_with(['\\', '/']) || host_part.contains('\\') {
        return None;
    }
    if p.conservative {
        return (looks_like_url(text, p.require_slash, &p.chars) && conservative_shape(text) && colon_scheme(text).is_some_and(|c| GUESS_SCHEMES.contains(&c.as_str())))
            .then(|| (text.to_string(), range));
    }
    if text.contains(':') {
        // Registered-handler test (UAF:819): only the schemes guard_open can pass on.
        return colon_scheme(text).is_some_and(|c| GUESS_SCHEMES.contains(&c.as_str())).then(|| (text.to_string(), range));
    }
    if !looks_like_url(text, p.require_slash, &p.chars) {
        return None;
    }
    if !p.cwd_is_local && !text.contains('/') {
        return None;
    }
    let host = text.split('/').next().unwrap_or(text);
    let scheme = if single_domain_word(host) { "http".to_string() } else { p.default_scheme.clone() };
    if !GUESS_SCHEMES.contains(&scheme.to_ascii_lowercase().as_str()) {
        return None;
    }
    Some((format!("{scheme}://{text}"), range))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn click(text: &str) -> (String, String) {
        let (a, b) = text.split_once('|').unwrap();
        (a.to_string(), b.to_string())
    }

    fn g(text: &str, p: &UrlPrefs) -> Option<String> {
        let (b, a) = click(text);
        guess(&b, &a, p).map(|(u, _)| u)
    }

    #[test]
    fn url_charset_follows_iterm2() {
        let s = CharSet::default();
        for c in ['a', 'Z', '7', '\u{e9}', '\u{4f8b}', '.', '?', '\\', '/', '|', '[', ']', '\u{ff08}', '\u{ff09}', '\'', '-', ':'] {
            assert!(s.contains(c), "{c:?}");
        }
        for c in [' ', '"', '<', '>', '{', '^', '`', '\u{ac}'] {
            assert!(!s.contains(c), "{c:?}");
        }
        assert!(url("", "").contains('a') && url("", "").contains('\''));
        assert!(!url(".", ".").contains('.'));
        let f = filename(&s, DEFAULT_FILENAME_CHARS);
        assert!(f.contains(' ') && f.contains('\t') && f.contains('$') && f.contains('@') && !f.contains('\n'));
        assert!(!s.contains('$') || DEFAULT_URL_CHARS.contains('$'));
    }

    #[test]
    fn url_range_follows_iterm2() {
        let t = |s: &str| range_of_url(s, DEFAULT_TRAILING).map(|r| s[r].to_string());
        assert_eq!(t("*(http://example.com)").as_deref(), Some("http://example.com"));
        assert_eq!(t("*http://example.com").as_deref(), Some("http://example.com"));
        assert_eq!(t("(see.com)x").as_deref(), Some("see.com"));
        assert_eq!(t("'quoted'").as_deref(), Some("quoted"));
        assert_eq!(t("example.com.").as_deref(), Some("example.com"));
        assert_eq!(t("a.com..").as_deref(), Some("a.com."));
        assert_eq!(t("example.com\u{2026}").as_deref(), Some("example.com"));
        assert_eq!(t("example.com\"").as_deref(), Some("example.com"));
        assert_eq!(t("--flag.com").as_deref(), Some("flag.com"));
        assert_eq!(t("x-man-page://ls").as_deref(), Some("x-man-page://ls"));
        assert_eq!(t("   "), None);
    }

    #[test]
    fn url_guess_follows_iterm2() {
        let d = UrlPrefs::default();
        let (b, a) = click("go https://exa|mple.com/a?b=1.");
        let (u, r) = guess(&b, &a, &d).unwrap();
        assert_eq!(u, "https://example.com/a?b=1");
        assert_eq!(&format!("{b}{a}")[r], "https://example.com/a?b=1");
        assert_eq!(g("see server/sta|tus now", &d).as_deref(), Some("http://server/status"));
        assert_eq!(g("www.exam|ple.com/x", &d).as_deref(), Some("https://www.example.com/x"));
        let ftp = UrlPrefs { default_scheme: "ftp".into(), ..UrlPrefs::default() };
        assert_eq!(g("www.exam|ple.com/x", &ftp).as_deref(), Some("ftp://www.example.com/x"));
        assert_eq!(g("exam|ple.com", &d), None);
        let loose = UrlPrefs { require_slash: false, ..UrlPrefs::default() };
        assert_eq!(g("exam|ple.com", &loose).as_deref(), Some("https://example.com"));
        assert_eq!(g("/usr/b|in", &d), None);
        assert_eq!(g("10.0.0.|1/x", &d).as_deref(), Some("https://10.0.0.1/x"));
        let remote = UrlPrefs { cwd_is_local: false, ..UrlPrefs::default() };
        assert_eq!(g("foo/b|ar", &remote).as_deref(), Some("http://foo/bar"));
        let both = UrlPrefs { require_slash: false, cwd_is_local: false, ..UrlPrefs::default() };
        assert_eq!(g("exam|ple.com", &both), None);
        assert_eq!(g("mailto:a@|b.com", &d).as_deref(), Some("mailto:a@b.com"));
        // Fullwidth parentheses are URL characters, not wrappers. With a scheme the walk back
        // from the colon drops one character before it (NSS:1118-1128), the opening one; the
        // closing one stays (the trailing marks hold only the ASCII one). The brief said both
        // stay; the source says otherwise and wins.
        assert_eq!(g("\u{ff08}https://a.b/|c\u{ff09}", &d).as_deref(), Some("https://a.b/c\u{ff09}"));
        assert_eq!(g("\u{ff08}a.b/|c\u{ff09}", &d).as_deref(), Some("https://\u{ff08}a.b/c\u{ff09}"));
    }

    #[test]
    fn guessed_urls_carry_only_handled_schemes() {
        let d = UrlPrefs::default();
        for t in ["C:\\missing\\|x.txt", "ms-settings:|display", "search-ms:que|ry=x", "\\\\evil\\s\\a.ex|e:x", "javascript:ale|rt(1)", "file:///C:/|x"] {
            assert_eq!(g(t, &d), None, "{t}");
        }
        let odd = UrlPrefs { default_scheme: "ms-msdt".into(), ..UrlPrefs::default() };
        assert_eq!(g("www.exam|ple.com/x", &odd), None);
    }

    #[test]
    fn path_shaped_text_never_becomes_a_url() {
        let d = UrlPrefs::default();
        for t in ["\\\\evil\\share/|x", "//evil/sh|are/x", "\\/evil/s|/x", "/\\evil/s|/x", "\\\\ev|il\\s", "C:\\a/|b", "\\evil|/x", "ho\\st/|x"] {
            assert_eq!(g(t, &d), None, "{t}");
        }
        let loose = UrlPrefs { require_slash: false, ..UrlPrefs::default() };
        assert_eq!(g("\\\\evil\\sh|are", &loose), None);
        assert_eq!(g("www.exa|mple.com/x", &d).as_deref(), Some("https://www.example.com/x"), "real hosts still guess");
    }

    #[test]
    fn conservative_guess_follows_iterm2() {
        let c = UrlPrefs { conservative: true, ..UrlPrefs::default() };
        assert_eq!(g("https://exa|mple.com/x", &c).as_deref(), Some("https://example.com/x"));
        assert_eq!(g("https://exa|mple.com", &c).as_deref(), Some("https://example.com"));
        assert_eq!(g("https://exa|mple.com:8080/", &c).as_deref(), Some("https://example.com:8080/"));
        assert_eq!(g("https://local|host/", &c), None);
        assert_eq!(g("www.exa|mple.com/x", &c), None);
        assert_eq!(g("example.com/|x", &c), None);
        assert_eq!(g("HTTPS://A.|B/", &c), None);
    }

    #[test]
    fn single_domain_word_follows_iterm2() {
        assert!(single_domain_word("server") && single_domain_word("Server9"));
        assert!(!single_domain_word("9server") && !single_domain_word("a.b") && !single_domain_word("") && !single_domain_word("\u{e9}t\u{e9}"));
    }
}

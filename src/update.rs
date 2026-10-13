//! The release feed: GitHub's releases JSON, Sparkle's version comparator, the schedule, the
//! permission and skip rules and the texts (M13v2 Task 1). Pure: the feed is untrusted input,
//! and nothing here reaches the network or the disk.

use std::cmp::Ordering;

use crate::globals::Globals;
use crate::inert_files::{is_hidden, shown_name};

/// The releases feed: GitHub's API for `datbird/termurion`, no token. While the repository is
/// private it answers 404, which reads as no update information (the 13v/13w ruling).
pub const RELEASES_URL: &str = "https://api.github.com/repos/datbird/termurion/releases?per_page=30";
/// The page Learn More opens when the feed names no better one (and the only host it may name).
pub const RELEASE_PAGE: &str = "https://github.com/datbird/termurion/releases";
/// The project home: Help > Termurion Help opens it and About names it.
pub const REPO_URL: &str = "https://github.com/datbird/termurion";

/// The stored keys (`SUUpdater.m`: `SUEnableAutomaticChecks`, `SUScheduledCheckInterval`,
/// `SULastCheckTime`, `SUHasLaunchedBefore`, `SUSkippedVersion`).
pub const AUTOMATIC_KEY: &str = "SUEnableAutomaticChecks";
pub const INTERVAL_KEY: &str = "SUScheduledCheckInterval";
pub const LAST_CHECK_KEY: &str = "SULastCheckTime";
pub const LAUNCHED_KEY: &str = "SUHasLaunchedBefore";
pub const SKIPPED_KEY: &str = "SUSkippedVersion";

/// The largest feed body read (2 MiB), the most releases kept, and the other bounds on what the
/// network says (P3).
pub const MAX_FEED: usize = 2 * 1024 * 1024;
const MAX_RELEASES: usize = 30;
const MAX_TAG: usize = 64;
const MAX_NAME: usize = 128;
const MAX_NOTES: usize = 16 * 1024;
const MAX_PARTS: usize = 32;
const MAX_URL: usize = 256;

pub const DEFAULT_INTERVAL: f64 = 86400.0;
pub const MIN_INTERVAL: f64 = 3600.0;
/// The longest interval honored (P4): a month.
pub const MAX_INTERVAL: f64 = 2_592_000.0;

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Number,
    Separator,
    Text,
}

/// Unicode category P (`punctuationCharacterSet`): the ASCII ones are `!"#%&'()*,-./:;?@[\\]_{}`;
/// `+ $ < = > ^ ` | ~` are symbols, so they are text parts, as in Sparkle. (Non-ASCII punctuation is
/// text here: no tag the filter admits holds any.)
fn is_punctuation(c: char) -> bool {
    matches!(c, '!' | '"' | '#' | '%' | '&' | '\'' | '(' | ')' | '*' | ',' | '-' | '.' | '/' | ':' | ';' | '?' | '@' | '[' | '\\' | ']' | '_' | '{' | '}')
}

fn kind_of(c: char) -> Kind {
    if c.is_ascii_digit() {
        Kind::Number
    } else if c.is_whitespace() || is_punctuation(c) {
        Kind::Separator
    } else {
        Kind::Text
    }
}

/// `SUStandardVersionComparator`'s parts: runs of digits, runs of other characters, and each
/// separator alone. At most `MAX_PARTS` are kept.
fn parts(v: &str) -> Vec<(Kind, &str)> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut kind = None;
    for (i, c) in v.char_indices() {
        let k = kind_of(c);
        if let Some(prev) = kind {
            if prev != k || k == Kind::Separator {
                out.push((prev, &v[start..i]));
                start = i;
                if out.len() >= MAX_PARTS {
                    return out;
                }
            }
        }
        kind = Some(k);
    }
    if let Some(k) = kind {
        if out.len() < MAX_PARTS {
            out.push((k, &v[start..]));
        }
    }
    out
}

fn compare_numbers(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.trim_start_matches('0'), b.trim_start_matches('0'));
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

/// Sparkle's comparison: part by part, numbers by value, text by text; a number beats a
/// separator, a non-text part beats a text part; when one version runs out, the other's next
/// part decides: text makes the shorter one newer, anything else the longer.
pub fn compare(a: &str, b: &str) -> Ordering {
    let (pa, pb) = (parts(a), parts(b));
    for (x, y) in pa.iter().zip(pb.iter()) {
        let ord = match (x.0, y.0) {
            (Kind::Number, Kind::Number) => compare_numbers(x.1, y.1),
            (Kind::Text, Kind::Text) => x.1.cmp(y.1),
            // Sparkle compares only numbers and strings: two separators are equal.
            (Kind::Separator, Kind::Separator) => Ordering::Equal,
            (Kind::Text, _) => Ordering::Less,
            (_, Kind::Text) => Ordering::Greater,
            (Kind::Number, Kind::Separator) => Ordering::Greater,
            (Kind::Separator, Kind::Number) => Ordering::Less,
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    match pa.len().cmp(&pb.len()) {
        Ordering::Equal => Ordering::Equal,
        Ordering::Less => {
            if pb[pa.len()].0 == Kind::Text {
                Ordering::Greater
            } else {
                Ordering::Less
            }
        }
        Ordering::Greater => {
            if pa[pb.len()].0 == Kind::Text {
                Ordering::Less
            } else {
                Ordering::Greater
            }
        }
    }
}

/// A tag's version: one leading "v" or "V" dropped.
pub fn tag_version(tag: &str) -> &str {
    tag.strip_prefix(['v', 'V']).unwrap_or(tag)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub name: String,
    pub notes: String,
    /// Always the pinned release page or a path under it (`release_url`).
    pub url: String,
    pub prerelease: bool,
}

/// `url` when it is on the pinned releases page (https, github.com, `datbird/termurion/releases`,
/// then a path of plain characters with no dot segment), else None: the feed cannot name
/// anything else to open (B4).
pub fn release_url(url: &str) -> Option<&str> {
    if url.len() > MAX_URL {
        return None;
    }
    let rest = url.strip_prefix(RELEASE_PAGE)?;
    if let Some(first) = rest.chars().next() {
        if !matches!(first, '/' | '?' | '#') {
            return None;
        }
        if !rest[1..].chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '~' | '/' | '-')) {
            return None;
        }
        if rest.split(['/', '?', '#']).any(|seg| seg == "." || seg == "..") {
            return None;
        }
    }
    Some(url)
}

/// Text from the feed as the person reads it: escape sequences and hidden characters dropped,
/// a carriage return dropped, newline and tab kept.
fn clean_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len().min(MAX_NOTES + 4));
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    for n in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&n) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(n) = chars.next() {
                        if n == '\u{7}' {
                            break;
                        }
                        if n == '\u{1b}' {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {
                    chars.next();
                }
            }
        } else if c == '\n' || c == '\t' || !is_hidden(c) {
            out.push(c);
        }
    }
    out
}

fn cut(s: String, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

fn valid_tag(tag: &str) -> bool {
    let n = tag.chars().count();
    if n == 0 || n > MAX_TAG || !tag.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-')) {
        return false;
    }
    // Only a version that starts with a digit is ever compared.
    tag_version(tag).starts_with(|c: char| c.is_ascii_digit())
}

/// GitHub's releases array. Drafts are dropped; a tag that is not a plain version is skipped;
/// at most 30 are kept; a body over 2 MiB, JSON that is not an array or is too deep is an error.
pub fn parse_releases(json: &str) -> Result<Vec<Release>, String> {
    if json.len() > MAX_FEED {
        return Err("the feed is too large".into());
    }
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| shown_name(&e.to_string()))?;
    let items = value.as_array().ok_or_else(|| "the feed is not a list of releases".to_string())?;
    let mut out = Vec::new();
    for item in items.iter().take(MAX_RELEASES) {
        let Some(obj) = item.as_object() else { continue };
        if obj.get("draft").and_then(|v| v.as_bool()) == Some(true) {
            continue;
        }
        let Some(tag) = obj.get("tag_name").and_then(|v| v.as_str()) else { continue };
        if !valid_tag(tag) {
            continue;
        }
        let text = |key: &str| obj.get(key).and_then(|v| v.as_str()).unwrap_or("");
        let name: String = clean_text(text("name")).chars().filter(|c| *c != '\n' && *c != '\t').take(MAX_NAME).collect();
        let notes = clean_text(&cut(text("body").to_string(), MAX_NOTES * 4));
        out.push(Release {
            version: tag_version(tag).to_string(),
            name,
            notes: cut(notes, MAX_NOTES),
            url: release_url(text("html_url")).unwrap_or(RELEASE_PAGE).to_string(),
            prerelease: obj.get("prerelease").and_then(|v| v.as_bool()) == Some(true),
        });
    }
    Ok(out)
}

/// The newest release by `compare`; prereleases only with `test_releases`.
pub fn newest(releases: &[Release], test_releases: bool) -> Option<&Release> {
    let mut best: Option<&Release> = None;
    for r in releases.iter().filter(|r| test_releases || !r.prerelease) {
        if best.is_none_or(|b| compare(&r.version, &b.version) == Ordering::Greater) {
            best = Some(r);
        }
    }
    best
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    Update(Release),
    UpToDate { newest: String },
    Newer { newest: String },
}

/// What a check found. A release older than or equal to the running version is never offered;
/// a background check treats the skipped version as up to date, a manual one offers it.
pub fn outcome(releases: &[Release], test_releases: bool, current: &str, skipped: Option<&str>, manual: bool) -> Found {
    let Some(best) = newest(releases, test_releases) else {
        return Found::UpToDate { newest: current.to_string() };
    };
    match compare(&best.version, current) {
        Ordering::Greater => {
            if !manual && skipped.is_some_and(|s| compare(s, &best.version) == Ordering::Equal) {
                Found::UpToDate { newest: best.version.clone() }
            } else {
                Found::Update(best.clone())
            }
        }
        Ordering::Equal => Found::UpToDate { newest: best.version.clone() },
        Ordering::Less => Found::Newer { newest: best.version.clone() },
    }
}

/// Seconds until the next automatic check (`SUUpdater.m:277-300`): the interval (default a day,
/// at least an hour, at most a month) less the time since the last check, 0 when overdue or
/// never checked. A last check in the future counts as now (P4).
pub fn delay_until_check(now: f64, last: Option<f64>, interval: Option<f64>) -> f64 {
    let interval = interval.filter(|i| i.is_finite()).unwrap_or(DEFAULT_INTERVAL).clamp(MIN_INTERVAL, MAX_INTERVAL);
    if !now.is_finite() {
        return DEFAULT_INTERVAL;
    }
    match last.filter(|l| l.is_finite()) {
        None => 0.0,
        Some(last) => (interval - (now - last.min(now))).max(0.0),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start {
    Ask,
    Schedule,
    Nothing,
}

/// `startUpdateCycle` (`SUUpdater.m:193-240`): `automatic` is the stored answer, None when never
/// answered; the question shows on the second launch, never the first.
pub fn start(automatic: Option<bool>, launched_before: bool) -> Start {
    match automatic {
        Some(true) => Start::Schedule,
        Some(false) => Start::Nothing,
        None if launched_before => Start::Ask,
        None => Start::Nothing,
    }
}

/// The stored answer, never the schema default (Sparkle reads the raw user default).
pub fn automatic_of(g: &Globals) -> Option<bool> {
    g.raw(AUTOMATIC_KEY).and_then(|v| v.as_bool())
}

pub fn automatic_checks_on(g: &Globals) -> bool {
    automatic_of(g) == Some(true)
}

/// The skipped version, when stored as a short string (P4).
pub fn skipped_of(g: &Globals) -> Option<String> {
    g.raw(SKIPPED_KEY).and_then(|v| v.as_str()).filter(|s| s.chars().count() <= 64).map(str::to_string)
}

pub fn last_check_of(g: &Globals) -> Option<f64> {
    g.raw(LAST_CHECK_KEY).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
}

pub fn interval_of(g: &Globals) -> Option<f64> {
    g.raw(INTERVAL_KEY).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
}

/// curl.exe's arguments for the feed (B4, P2): `-q` first so no config file is read, only `proto`
/// (`=https` in production) for the request and https for any redirect, a size and a time cap, no
/// `--fail` so an HTTP error still reports its status, written last by `--write-out`.
pub fn fetch_args(url: &str, version: &str, proto: &str) -> Vec<String> {
    let version: String = version.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-')).collect();
    let mut args: Vec<String> = ["-q", "--silent", "--show-error", "--proto"].iter().map(|s| s.to_string()).collect();
    args.push(proto.to_string());
    for a in ["--proto-redir", "=https", "--max-redirs", "3", "--max-filesize", "2097152", "--max-time", "30", "--tlsv1.2", "--write-out", "\n%{http_code}", "-H", "Accept: application/vnd.github+json", "-H"] {
        args.push(a.to_string());
    }
    args.push(format!("User-Agent: Termurion/{version}"));
    args.push(url.to_string());
    args
}

/// curl's output: the body, then the status on the last line. None when that line is no HTTP
/// status (curl writes 000 when there was no response).
pub fn split_status(stdout: &str) -> Option<(u16, String)> {
    let trimmed = stdout.trim_end_matches(['\r', '\n']);
    let (body, last) = match trimmed.rfind('\n') {
        Some(i) => (&trimmed[..i], &trimmed[i + 1..]),
        None => ("", trimmed),
    };
    let code: u16 = last.trim().parse().ok()?;
    if !(100..=599).contains(&code) {
        return None;
    }
    Some((code, body.trim_end_matches('\r').to_string()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fetched {
    Status { code: u16, body: String },
    Transport(String),
}

pub trait Fetch {
    fn get(&self, url: &str) -> Fetched;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FeedResult {
    Body(String),
    NoInfo,
    Error(String),
}

/// 200 to 299 is the feed; 404 and 401 are no update information, silently (the 13v/13w ruling);
/// every other status and every transport failure is an error (R13vw-1). An error never carries
/// the response body, and curl's text is cleaned.
pub fn classify(f: Fetched) -> FeedResult {
    match f {
        Fetched::Status { code: 200..=299, ref body } if body.len() > MAX_FEED => {
            FeedResult::Error("the feed is too large".into())
        }
        Fetched::Status { code: 200..=299, body } => FeedResult::Body(body),
        Fetched::Status { code: 404 | 401, .. } => FeedResult::NoInfo,
        Fetched::Status { code, .. } => FeedResult::Error(format!("HTTP {code}")),
        Fetched::Transport(text) => FeedResult::Error(shown_name(&clean_text(&text))),
    }
}

pub const TITLE: &str = "Software Update";
pub const CHECKING: &str = "Checking for updates\u{2026}";
pub const UPDATE_HEADING: &str = "A new version of Termurion is available!";
pub const RELEASE_NOTES: &str = "Release Notes:";
pub const SKIP: &str = "Skip This Version";
pub const REMIND: &str = "Remind Me Later";
pub const LEARN_MORE: &str = "Learn More\u{2026}";
pub const UP_TO_DATE_TITLE: &str = "You\u{2019}re up-to-date!";
pub const ERROR_TITLE: &str = "Update Error!";
pub const RETRIEVE_ERROR: &str = "An error occurred in retrieving update information. Please try again later.";
pub const PARSE_ERROR: &str = "An error occurred while parsing the update feed.";
pub const PERMISSION_TITLE: &str = "Check for updates automatically?";
pub const CHECK_AUTOMATICALLY: &str = "Check Automatically";
pub const DONT_CHECK: &str = "Don\u{2019}t Check";

pub fn permission_text() -> String {
    "Should Termurion automatically check for updates? You can always check for updates manually from the Termurion menu.".to_string()
}

/// Versions come from the feed: shown through `shown_name`.
pub fn update_text(new: &str, current: &str) -> String {
    format!(
        "Termurion {} is now available (you have {}). Would you like to learn more about this update on the web?",
        shown_name(new),
        shown_name(current)
    )
}

pub fn up_to_date_text(newest: &str, running: Option<&str>) -> String {
    let mut text = format!("Termurion {} is currently the newest version available.", shown_name(newest));
    if let Some(running) = running {
        text.push_str(&format!("\n(You are currently running version {}.)", shown_name(running)));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::globals::Globals;
    use std::cmp::Ordering::{Equal, Greater, Less};
    use toml::Value;

    #[test]
    fn versions_compare_as_sparkle() {
        for (a, b, want) in [
            ("1.0", "1.0", Equal),
            ("1.1", "1.0.9", Greater),
            ("1.10", "1.9", Greater),
            ("1.0", "1.0.1", Less),
            ("1.0b1", "1.0", Less),
            ("1.0", "1.0.", Less),
            ("1.0a", "1.0b", Less),
            ("2", "1.9.9", Greater),
            ("1.0.1", "1.0a", Greater),
            // '+' is Unicode symbol (Sm), not punctuation: a string part, so it makes the
            // shorter version newer; '-' and '.' are both separators and compare equal.
            ("1.0+1", "1.0", Less),
            ("1.0-1", "1.0.1", Equal),
            ("1.0_1", "1.0 1", Equal),
            ("1.0~1", "1.0", Less),
            ("1.0.1", "1.0-1", Equal),
            // Sparkle's own table (Tests/SUVersionComparisonTest.m): build numbers in brackets,
            // pre-release words, a prerelease of the next point release.
            ("1.0b1 (1234)", "1.0 (1234)", Less),
            ("3.3 (5847)", "3.3.1b1 (5902)", Less),
            ("1.0pre1", "1.0", Less),
            ("1.0b1", "1.0b2", Less),
            ("1.0b2", "1.0", Less),
            ("1.0 (1234)", "1.0 (1235)", Less),
            ("1.0 (1234)", "1.0 (1234)", Equal),
            ("1.5", "1.10", Less),
            ("1.0.0", "1.0", Greater),
            ("2.0a1", "1.9", Greater),
            ("1.0rc1", "1.0", Less),
            ("1.0rc1", "1.0b9", Greater),
        ] {
            assert_eq!(compare(a, b), want, "{a} vs {b}");
            assert_eq!(compare(b, a), want.reverse(), "{b} vs {a}");
        }
        assert_eq!(tag_version("v0.0.2"), "0.0.2");
        assert_eq!(tag_version("V1"), "1");
        assert_eq!(tag_version("1.2"), "1.2");
    }

    #[test]
    fn version_parse_is_bounded_and_panic_free() {
        // Numbers of any length compare by value without overflow; leading zeros do not count.
        let huge = "9".repeat(5000);
        assert_eq!(compare(&huge, &huge), Equal);
        assert_eq!(compare(&format!("{huge}1"), &huge), Greater);
        assert_eq!(compare("007", "7"), Equal);
        // Only the first 32 parts count (P3).
        let long_a = vec!["1"; 40].join(".");
        let long_b = format!("{long_a}.9");
        assert_eq!(compare(&long_a, &long_b), Equal);
        for odd in ["", ".", "..", "\u{0}", "\u{1f600}.1", "v", "-", "1..2"] {
            let _ = compare(odd, "1.0");
            let _ = compare("1.0", odd);
        }
    }

    const FEED: &str = r#"[
      {"tag_name":"v0.0.3","name":"Three","body":"notes three","html_url":"https://github.com/datbird/termurion/releases/tag/v0.0.3","prerelease":true,"draft":false},
      {"tag_name":"v0.0.2","name":"Two","body":"notes two","html_url":"https://github.com/datbird/termurion/releases/tag/v0.0.2","prerelease":false,"draft":false},
      {"tag_name":"v0.0.4","name":"Four","body":"draft","html_url":"https://github.com/datbird/termurion/releases/tag/v0.0.4","prerelease":false,"draft":true},
      {"tag_name":"v0.0.1","name":"One","body":"notes one","html_url":"https://github.com/datbird/termurion/releases/tag/v0.0.1","prerelease":false,"draft":false}
    ]"#;

    #[test]
    fn releases_follow_the_channel() {
        let list = parse_releases(FEED).unwrap();
        assert_eq!(list.len(), 3, "the draft is dropped");
        assert_eq!(list[0].version, "0.0.3");
        assert_eq!(list[1].notes, "notes two");
        assert_eq!(list[1].url, "https://github.com/datbird/termurion/releases/tag/v0.0.2");
        assert_eq!(newest(&list, false).unwrap().version, "0.0.2");
        assert_eq!(newest(&list, true).unwrap().version, "0.0.3");
        assert!(parse_releases("{}").is_err());
        assert!(parse_releases("not json").is_err());
        assert_eq!(parse_releases("[]").unwrap(), Vec::new());
    }

    #[test]
    fn feed_text_is_bounded_and_clean() {
        // A tag that does not match is skipped, not an error (P3).
        let list = parse_releases(
            r#"[{"tag_name":"v1.0 evil","name":"x","body":"","html_url":"","prerelease":false},
                {"tag_name":"latest","name":"x","body":"","html_url":"","prerelease":false},
                {"tag_name":"","name":"x","body":"","html_url":"","prerelease":false},
                {"tag_name":5},7,null,
                {"tag_name":"v2.0","prerelease":false}]"#,
        )
        .unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].version, "2.0");
        assert_eq!(list[0].url, RELEASE_PAGE, "no url means the pinned page");
        let long_tag = format!("v{}", "1".repeat(80));
        assert!(parse_releases(&format!(r#"[{{"tag_name":"{long_tag}"}}]"#)).unwrap().is_empty());
        // Text for the person: controls, ANSI escapes, bidi and zero-width characters go; the
        // body is cut to 16 KiB, the name to 128 characters.
        let body = format!("ok\\u001b[31mred\\u001b[0m\\u202eflip\\u200bzw\\u0007bell\\nline2\\r\\n{}", "x".repeat(40000));
        let name = "n".repeat(300);
        let json = format!(r#"[{{"tag_name":"v1.0","name":"{name}","body":"{body}","html_url":"x","prerelease":false}}]"#);
        let r = &parse_releases(&json).unwrap()[0];
        assert!(r.notes.starts_with("okredflipzwbell\nline2\n"), "{:?}", &r.notes[..40]);
        assert!(r.notes.len() <= 16 * 1024);
        assert!(!r.notes.chars().any(|c| c == '\u{1b}' || c == '\u{202e}' || c == '\u{200b}' || c == '\u{7}' || c == '\r'));
        assert!(r.name.chars().count() <= 128);
        // At most 30 releases are kept; a body over 2 MiB is refused whole.
        let many = format!("[{}]", vec![r#"{"tag_name":"v1.0"}"#; 100].join(","));
        assert_eq!(parse_releases(&many).unwrap().len(), 30);
        let huge = format!(r#"[{{"tag_name":"v1.0","body":"{}"}}]"#, "x".repeat(MAX_FEED + 1));
        assert!(parse_releases(&huge).is_err());
        // Deeply nested input is an error, not a stack overflow.
        assert!(parse_releases(&"[".repeat(100000)).is_err());
    }

    #[test]
    fn learn_more_url_is_pinned() {
        for ok in [
            "https://github.com/datbird/termurion/releases",
            "https://github.com/datbird/termurion/releases/",
            "https://github.com/datbird/termurion/releases/tag/v0.0.2",
            "https://github.com/datbird/termurion/releases?x",
            "https://github.com/datbird/termurion/releases#top",
        ] {
            assert_eq!(release_url(ok), Some(ok), "{ok}");
        }
        for bad in [
            "https://github.com.evil.example/datbird/termurion/releases",
            "https://github.com@evil/datbird/termurion/releases",
            "https://github.com/datbird/termurion/releases@evil",
            "https://github.com/datbird/termurion/releasesx",
            "https://github.com/datbird/termurion/releases/../../x",
            "https://github.com/datbird/termurion/releases/./x",
            "https://github.com/datbird/termurion/releases/%2e%2e/x",
            "https://github.com/datbird/termurion/releases/a b",
            "https://github.com/datbird/termurion/releases/a\\b",
            "https://github.com:444/datbird/termurion/releases",
            "http://github.com/datbird/termurion/releases",
            "HTTPS://github.com/datbird/termurion/releases",
            "file:///C:/Windows/System32/calc.exe",
            "\\\\evil\\share\\x",
            "ms-msdt:/id",
            "",
        ] {
            assert_eq!(release_url(bad), None, "{bad}");
        }
        let list = parse_releases(r#"[{"tag_name":"v1.0","html_url":"https://github.com.evil.example/x"}]"#).unwrap();
        assert_eq!(list[0].url, RELEASE_PAGE);
    }

    fn rel(version: &str, pre: bool) -> Release {
        Release { version: version.into(), name: String::new(), notes: String::new(), url: RELEASE_PAGE.into(), prerelease: pre }
    }

    #[test]
    fn skip_follows_sparkle() {
        let list = vec![rel("0.0.2", false), rel("0.0.1", false)];
        assert_eq!(outcome(&list, false, "0.0.1", Some("0.0.2"), false), Found::UpToDate { newest: "0.0.2".into() });
        assert_eq!(outcome(&list, false, "0.0.1", Some("0.0.2"), true), Found::Update(rel("0.0.2", false)));
        assert_eq!(outcome(&list, false, "0.0.1", None, false), Found::Update(rel("0.0.2", false)));
        assert_eq!(outcome(&list, false, "0.0.2", None, true), Found::UpToDate { newest: "0.0.2".into() });
        assert_eq!(outcome(&list, false, "0.0.3", None, true), Found::Newer { newest: "0.0.2".into() });
        assert_eq!(outcome(&[], false, "0.0.1", None, true), Found::UpToDate { newest: "0.0.1".into() });
        // A feed version older than or equal to the running one is never offered, whatever the
        // way it is written.
        for current in ["0.0.2", "0.0.2.0", "0.0.02", "1.0"] {
            assert!(!matches!(outcome(&list, false, current, None, true), Found::Update(_)), "{current}");
        }
    }

    #[test]
    fn schedule_follows_sparkle() {
        assert_eq!(delay_until_check(1000.0, None, None), 0.0);
        assert_eq!(delay_until_check(1000.0, Some(900.0), None), 86300.0);
        assert_eq!(delay_until_check(1000.0, Some(900.0), Some(60.0)), 3500.0);
        assert_eq!(delay_until_check(100000.0, Some(900.0), None), 0.0);
        // P4: a last check in the future is now; an interval is clamped to an hour..a month; a
        // number that is not finite is the default.
        assert_eq!(delay_until_check(1000.0, Some(9e12), None), 86400.0);
        assert_eq!(delay_until_check(1000.0, Some(1000.0), Some(9e12)), 2_592_000.0);
        assert_eq!(delay_until_check(1000.0, Some(1000.0), Some(f64::NAN)), 86400.0);
        assert_eq!(delay_until_check(1000.0, Some(f64::NAN), None), 0.0);
        assert_eq!(delay_until_check(f64::NAN, Some(900.0), None), DEFAULT_INTERVAL);
    }

    #[test]
    fn permission_follows_sparkle() {
        assert_eq!(start(None, false), Start::Nothing);
        assert_eq!(start(None, true), Start::Ask);
        for lb in [false, true] {
            assert_eq!(start(Some(true), lb), Start::Schedule);
            assert_eq!(start(Some(false), lb), Start::Nothing);
        }
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("termurion-update-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("settings.toml")
    }

    #[test]
    fn consent_follows_sparkle() {
        let path = scratch("consent");
        let mut g = Globals::empty(&path);
        assert_eq!(automatic_of(&g), None);
        assert!(!automatic_checks_on(&g));
        assert!(g.bool("SUEnableAutomaticChecks"), "the checkbox still shows iTerm2's default");
        assert_eq!(start(automatic_of(&g), g.bool("SUHasLaunchedBefore")), Start::Nothing);
        g.set("SUHasLaunchedBefore", Value::Boolean(true));
        g.save().unwrap();
        let mut g = Globals::load(&path).unwrap();
        assert_eq!(start(automatic_of(&g), g.bool("SUHasLaunchedBefore")), Start::Ask);
        g.set("SUEnableAutomaticChecks", Value::Boolean(true));
        g.save().unwrap();
        let mut g = Globals::load(&path).unwrap();
        assert_eq!(g.raw("SUEnableAutomaticChecks"), Some(&Value::Boolean(true)));
        assert_eq!(start(automatic_of(&g), g.bool("SUHasLaunchedBefore")), Start::Schedule);
        assert!(automatic_checks_on(&g));
        assert!(std::fs::read_to_string(&path).unwrap().contains("SUEnableAutomaticChecks = true"));
        g.set("SUEnableAutomaticChecks", Value::Boolean(false));
        g.save().unwrap();
        let g = Globals::load(&path).unwrap();
        assert_eq!(start(automatic_of(&g), g.bool("SUHasLaunchedBefore")), Start::Nothing);
        assert!(!automatic_checks_on(&g));
        for lb in [false, true] {
            assert_ne!(start(automatic_of(&g), lb), Start::Ask);
        }
        // A key not kept at its default is still removed by `set`.
        let mut g = Globals::empty(&path);
        g.set("CheckTestRelease", Value::Boolean(false));
        assert!(g.raw("CheckTestRelease").is_none());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn stored_state_is_read_defensively() {
        let mut g = Globals::empty(&scratch("state"));
        g.set("SUSkippedVersion", Value::String("0.0.9".into()));
        assert_eq!(skipped_of(&g).as_deref(), Some("0.0.9"));
        g.set("SUSkippedVersion", Value::String("9".repeat(65)));
        assert_eq!(skipped_of(&g), None);
        g.set("SUSkippedVersion", Value::Integer(4));
        assert_eq!(skipped_of(&g), None);
        g.set("SULastCheckTime", Value::Float(500.0));
        assert_eq!(last_check_of(&g), Some(500.0));
        g.set("SULastCheckTime", Value::Integer(700));
        assert_eq!(last_check_of(&g), Some(700.0));
        g.set("SULastCheckTime", Value::String("x".into()));
        assert_eq!(last_check_of(&g), None);
        g.set("SUScheduledCheckInterval", Value::Integer(7200));
        assert_eq!(interval_of(&g), Some(7200.0));
    }

    #[test]
    fn status_follows_the_feed() {
        assert_eq!(split_status("[]\n200"), Some((200, "[]".into())));
        assert_eq!(split_status("{\"message\":\"Not Found\"}\n404"), Some((404, "{\"message\":\"Not Found\"}".into())));
        assert_eq!(split_status("[]\r\n200\r\n"), Some((200, "[]".into())));
        assert_eq!(split_status("\n200"), Some((200, String::new())));
        assert_eq!(split_status("200"), Some((200, String::new())));
        assert_eq!(split_status(""), None);
        assert_eq!(split_status("body\nabc"), None);
        assert_eq!(split_status("body\n000"), None);
        assert_eq!(split_status("body\n99999"), None);
        assert_eq!(classify(Fetched::Status { code: 200, body: "[]".into() }), FeedResult::Body("[]".into()));
        assert_eq!(classify(Fetched::Status { code: 299, body: "[]".into() }), FeedResult::Body("[]".into()));
        assert_eq!(classify(Fetched::Status { code: 404, body: "x".into() }), FeedResult::NoInfo);
        assert_eq!(classify(Fetched::Status { code: 401, body: "x".into() }), FeedResult::NoInfo);
        for code in [500, 403, 301, 304, 199] {
            assert!(matches!(classify(Fetched::Status { code, body: "oops".into() }), FeedResult::Error(_)), "{code}");
        }
        let FeedResult::Error(e) = classify(Fetched::Status { code: 500, body: "SECRET BODY".into() }) else { panic!() };
        assert!(!e.contains("SECRET"), "the body of an error status is not shown");
        let FeedResult::Error(e) = classify(Fetched::Transport("curl: (6) Could not resolve host: api.github.com\u{1b}[31m\u{202e}".into())) else { panic!() };
        assert!(e.contains("Could not resolve host") && !e.contains('\u{1b}') && !e.contains('\u{202e}'));
        let big = Fetched::Status { code: 200, body: "x".repeat(MAX_FEED + 1) };
        assert!(matches!(classify(big), FeedResult::Error(_)));
    }

    #[test]
    fn fetch_args_are_https_only_and_bounded() {
        let args = fetch_args("U", "0.0.1", "=https");
        let want = [
            "-q", "--silent", "--show-error", "--proto", "=https", "--proto-redir", "=https", "--max-redirs", "3", "--max-filesize", "2097152", "--max-time", "30", "--tlsv1.2", "--write-out", "\n%{http_code}", "-H",
            "Accept: application/vnd.github+json", "-H", "User-Agent: Termurion/0.0.1", "U",
        ];
        assert_eq!(args, want);
        assert_eq!(args[0], "-q", "-q is first, or curl reads a .curlrc before it");
        for banned in ["--insecure", "-k", "--ssl-no-revoke", "-K", "-o", "-O", "--output", "--remote-name", "--config", "--fail", "-f"] {
            assert!(!args.iter().any(|a| a == banned), "{banned}");
        }
        assert!(fetch_args("U", "0.0.1\nX: y", "=https").contains(&"User-Agent: Termurion/0.0.1Xy".to_string()), "no header can be added through the version");
        assert_eq!(MAX_FEED, 2 * 1024 * 1024);
        assert!(RELEASES_URL.starts_with("https://api.github.com/repos/datbird/termurion/releases?per_page=30"));
    }

    #[test]
    fn texts_follow_sparkle() {
        assert_eq!(CHECKING, "Checking for updates\u{2026}");
        assert_eq!(UPDATE_HEADING, "A new version of Termurion is available!");
        assert_eq!(
            update_text("0.0.2", "0.0.1"),
            "Termurion 0.0.2 is now available (you have 0.0.1). Would you like to learn more about this update on the web?"
        );
        assert_eq!(up_to_date_text("0.0.2", None), "Termurion 0.0.2 is currently the newest version available.");
        assert_eq!(up_to_date_text("0.0.2", Some("0.0.3")), "Termurion 0.0.2 is currently the newest version available.\n(You are currently running version 0.0.3.)");
        assert_eq!(UP_TO_DATE_TITLE, "You\u{2019}re up-to-date!");
        assert_eq!(RETRIEVE_ERROR, "An error occurred in retrieving update information. Please try again later.");
        assert_eq!(PARSE_ERROR, "An error occurred while parsing the update feed.");
        assert_eq!(PERMISSION_TITLE, "Check for updates automatically?");
        assert_eq!(permission_text(), "Should Termurion automatically check for updates? You can always check for updates manually from the Termurion menu.");
        assert_eq!((CHECK_AUTOMATICALLY, DONT_CHECK), ("Check Automatically", "Don\u{2019}t Check"));
        assert_eq!((SKIP, REMIND, LEARN_MORE, RELEASE_NOTES, TITLE, ERROR_TITLE), ("Skip This Version", "Remind Me Later", "Learn More\u{2026}", "Release Notes:", "Software Update", "Update Error!"));
        // Feed text shown to the person is sanitized.
        assert_eq!(update_text("1\u{202e}.0", "0.\u{1b}9"), update_text("1.0", "0.9"));
        assert!(!current_version().is_empty());
    }

    #[test]
    fn update_never_downloads() {
        let src = include_str!("update.rs");
        let code = src.split("#[cfg(test)]").next().unwrap();
        for banned in ["std::fs", "Command::new", "CreateProcess", "ShellExecute", "std::net", "\"-o\"", "\"-O\"", "\"--output\""] {
            assert!(!code.contains(banned), "update.rs must stay pure: {banned}");
        }
    }
}

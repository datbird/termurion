//! Remote path suggestions and search notifications for the Composer (milestone 14d,
//! `SuggestionRequest.swift`, `LC:644-705`, `CD:2681-2698`): the request a typed line makes, the
//! 5-second cache that narrows an earlier answer, the Composer entries a remote answer becomes,
//! and the `%notif search` text. Pure. Everything a remote sends is hostile (P18): an answer is
//! capped and cleaned before it can become text the Composer types into a shell, and a search id
//! is checked before it is sent back.

use std::time::{Duration, Instant};

use crate::ssh_files::SuggestInputs;

/// How long a cached answer lives (`SR:121`, `maxAge`).
pub const MAX_AGE: Duration = Duration::from_secs(5);
/// While framing the limit is divided by this (`PS:24489-24509`, `SR:99-107`).
pub const LIMIT_DIVISOR: usize = 8;
/// The most entries one answer is kept to, as a multiple of the request's limit (P18).
pub const KEEP_FACTOR: usize = 4;
/// The longest entry kept, in characters (P18).
pub const ENTRY_MAX: usize = 255;
/// The longest search id the remote may name (P18).
pub const SEARCH_ID_MAX: usize = 64;
/// The longest `%notif search` payload read (one file's JSON).
pub const NOTIF_MAX: usize = 256 * 1024;
/// The most answers the cache holds.
const CACHE_MAX: usize = 256;

/// The cache of `SuggestionCache` (`SR:121-186`): an answer is kept 5 seconds, and a longer
/// prefix is answered from a shorter one's when every other input is equal.
#[derive(Default)]
pub struct Cache {
    entries: Vec<(SuggestInputs, Vec<String>, Instant)>,
}

fn same_but_prefix(a: &SuggestInputs, b: &SuggestInputs) -> bool {
    a.full_prefix == b.full_prefix
        && a.full_suffix == b.full_suffix
        && a.directories == b.directories
        && a.working_directory == b.working_directory
        && a.executable == b.executable
        && a.limit == b.limit
}

impl Cache {
    /// The values for `inputs`: the exact entry's, or a shorter prefix's narrowed to the longer
    /// one (the prefix characters dropped from each value). The variants are the prefix and
    /// each shorter non-empty prefix, longest first. An expired variant is removed; the first
    /// live one found decides: narrowed to nothing under a different prefix is None at once
    /// (a more general answer may have been cut at its limit, `SR:152-156`).
    pub fn get(&mut self, inputs: &SuggestInputs, now: Instant) -> Option<Vec<String>> {
        let chars: Vec<char> = inputs.prefix.chars().collect();
        for keep in (1..=chars.len()).rev() {
            let variant: String = chars[..keep].iter().collect();
            let Some(i) = self.entries.iter().position(|(k, _, _)| k.prefix == variant && same_but_prefix(k, inputs)) else { continue };
            if now.saturating_duration_since(self.entries[i].2) >= MAX_AGE {
                self.entries.remove(i);
                continue;
            }
            let extra: String = chars[keep..].iter().collect();
            let values: Vec<String> = self.entries[i].1.iter().filter_map(|v| v.strip_prefix(extra.as_str()).map(str::to_string)).collect();
            if values.is_empty() && keep != chars.len() {
                return None;
            }
            return Some(values);
        }
        None
    }

    pub fn insert(&mut self, inputs: SuggestInputs, values: Vec<String>, now: Instant) {
        self.entries.retain(|(k, _, _)| *k != inputs);
        if self.entries.len() >= CACHE_MAX {
            self.entries.remove(0);
        }
        self.entries.push((inputs, values, now));
    }

    /// Drops what expired (`SR:171-175`).
    pub fn purge(&mut self, now: Instant) {
        self.entries.retain(|(_, _, t)| now.saturating_duration_since(*t) < MAX_AGE);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The request a typed `line` makes (`LC:644-705`, `PS:24489-24509`): on the first word the
/// remote `PATH` (the working directory first when the line holds "/"), else the working
/// directory, else the remote home (iTerm2 uses the local home, `LC:671`: Decision, meaningless
/// on a remote from Windows). The limit is 256 for Tab and 64 while typing, divided by 8 while
/// framing and at least 1.
pub fn inputs(line: &str, full_suffix: &str, remote_path: Option<&str>, cwd: Option<&str>, home: Option<&str>, explicit: bool) -> SuggestInputs {
    let (first, prefix) = crate::composer_suggest::word_to_complete(line);
    let directories: Vec<String> = if first {
        let mut dirs: Vec<String> = remote_path.map(|p| p.split(':').map(str::to_string).collect()).unwrap_or_default();
        if line.contains('/') && !dirs.is_empty() {
            if let Some(cwd) = cwd {
                dirs.insert(0, cwd.to_string());
            }
        }
        dirs
    } else {
        cwd.or(home).map(|d| vec![d.to_string()]).unwrap_or_default()
    };
    let base = if explicit { crate::composer_suggest::FILES_TAB } else { crate::composer_suggest::FILES_TYPING };
    SuggestInputs {
        prefix: prefix.to_string(),
        full_prefix: line.to_string(),
        full_suffix: full_suffix.to_string(),
        directories,
        working_directory: cwd.map(str::to_string),
        executable: first,
        limit: (base / LIMIT_DIVISOR).max(1),
    }
}

/// The Composer's entries for the suffixes a remote sent after the typed `name`
/// (`F:959-1016`): `name` plus the suffix, a trailing "/" marking a folder and dropped.
pub fn entries(name: &str, suffixes: &[String]) -> Vec<(String, bool)> {
    suffixes
        .iter()
        .map(|s| {
            let dir = s.ends_with('/');
            (format!("{name}{}", s.strip_suffix('/').unwrap_or(s)), dir)
        })
        .collect()
}

/// One entry the Composer may show and type: no control character, no bidirectional or
/// zero-width one (the person reads what is typed), at most 255 characters (P18).
fn clean_entry(e: &str) -> bool {
    e.chars().count() <= ENTRY_MAX
        && e.chars().all(|c| {
            let mut b = [0u8; 4];
            let s: &str = c.encode_utf8(&mut b);
            crate::inert_files::shown_name(s) == s
        })
}

/// A remote's answer (a JSON array of strings), at most `KEEP_FACTOR` x `limit` entries, the
/// unclean ones dropped first (P18); anything else is empty.
pub fn parse_limited(content: &str, limit: usize) -> Vec<String> {
    let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(content) else { return Vec::new() };
    items.iter().filter_map(serde_json::Value::as_str).filter(|e| clean_entry(e)).take(KEEP_FACTOR * limit.max(1)).map(str::to_string).collect()
}

/// `parse_limited` at the largest limit a framing request has.
pub fn parse(content: &str) -> Vec<String> {
    parse_limited(content, crate::composer_suggest::FILES_TAB / LIMIT_DIVISOR)
}

/// A search id the remote may name, and Termurion may send back: `[A-Za-z0-9_-]{1,64}` (P18).
pub fn valid_search_id(id: &str) -> bool {
    (1..=SEARCH_ID_MAX).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// `%notif search <id> <json>` (`CD:2681-2698`): the id and the JSON, None for a text with no
/// space, a bad id or a payload past `NOTIF_MAX`.
pub fn search_notif(text: &str) -> Option<(String, String)> {
    let (id, json) = text.split_once(' ')?;
    (valid_search_id(id) && json.len() <= NOTIF_MAX).then(|| (id.to_string(), json.to_string()))
}

/// The most search results left unacknowledged per search: a conforming framer stops at 16
/// (`framer.py` `window_size`), so a remote past it is flooding (P18).
pub const ACK_WINDOW: usize = 16;

/// The search acknowledgements still out, per search id, and the results dropped for being past
/// `ACK_WINDOW`: a remote that sends faster than it answers can never grow the queue.
#[derive(Debug, Default)]
pub struct AckWindow {
    out: std::collections::HashMap<String, usize>,
    pub dropped: u64,
}

impl AckWindow {
    /// A result of search `id` arrived: true when it is handled and acknowledged (one more ack
    /// out), false when `ACK_WINDOW` acks are out already and it is dropped (counted).
    pub fn admit(&mut self, id: &str) -> bool {
        let out = self.out.entry(id.to_string()).or_insert(0);
        if *out >= ACK_WINDOW {
            self.dropped += 1;
            return false;
        }
        *out += 1;
        true
    }

    /// An acknowledgement of `id` was answered (or could not be sent).
    pub fn answered(&mut self, id: &str) {
        if let Some(out) = self.out.get_mut(id) {
            *out = out.saturating_sub(1);
            if *out == 0 {
                self.out.remove(id);
            }
        }
    }

    /// Acks still out for `id`.
    pub fn out(&self, id: &str) -> usize {
        self.out.get(id).copied().unwrap_or(0)
    }
}

/// Whether Termurion acknowledges a result of search `id`: only the current search or one Termurion
/// started earlier, never an id the remote made up (P18).
pub fn ack_allowed(id: &str, current: Option<&str>, issued: &[String]) -> bool {
    valid_search_id(id) && (current == Some(id) || issued.iter().any(|i| i == id))
}

/// What a framed pane's reported working directory means for the Composer's file list.
#[derive(Debug, PartialEq, Eq)]
pub enum FramedCwd {
    /// The remote reported none: list from its home.
    Unknown,
    /// A clean POSIX folder.
    Posix(String),
    /// Anything else (a drive path): the remote has no such folder, and the list stays empty
    /// rather than showing THIS PC's folder (G1: remote-written state never drives a local action).
    Refused,
}

/// Reads the working directory a framed pane's remote reported.
pub fn framed_cwd(reported: Option<&str>) -> FramedCwd {
    match reported {
        None => FramedCwd::Unknown,
        Some(d) => match crate::ssh_jumps::clean_directory(d).filter(|d| d.starts_with('/')) {
            Some(d) => FramedCwd::Posix(d),
            None => FramedCwd::Refused,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_framed_pane_never_lists_this_pcs_folder() {
        assert_eq!(framed_cwd(None), FramedCwd::Unknown);
        assert_eq!(framed_cwd(Some("/home/u")), FramedCwd::Posix("/home/u".into()));
        assert_eq!(framed_cwd(Some(r"C:\Users\me")), FramedCwd::Refused);
        // A leading slash is a POSIX path for the remote, whatever follows.
        assert_eq!(framed_cwd(Some("/C:/x")), FramedCwd::Posix("/C:/x".into()));
        assert_eq!(framed_cwd(Some("relative")), FramedCwd::Refused);
    }

    fn inp(prefix: &str) -> SuggestInputs {
        SuggestInputs { prefix: prefix.into(), full_prefix: "ls x/".into(), full_suffix: String::new(), directories: vec!["/h".into()], working_directory: Some("/h".into()), executable: false, limit: 8 }
    }
    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn suggestion_cache_follows_iterm2() {
        let t0 = Instant::now();
        let mut c = Cache::default();
        c.insert(inp("fo"), s(&["o", "obar/", "x"]), t0);
        let at = |n: u64| t0 + Duration::from_secs(n);
        assert_eq!(c.get(&inp("foo"), at(1)), Some(s(&["", "bar/"])));
        assert_eq!(c.get(&inp("fo"), at(1)), Some(s(&["o", "obar/", "x"])));
        // Narrowed to nothing under a different prefix: None at once.
        assert_eq!(c.get(&inp("foz"), at(1)), None);
        // The empty prefix is never a variant.
        let mut e = Cache::default();
        e.insert(inp(""), s(&["a"]), t0);
        assert_eq!(e.get(&inp("a"), t0), None);
        // Any other input differing never matches.
        for other in [
            SuggestInputs { directories: vec!["/z".into()], ..inp("foo") },
            SuggestInputs { working_directory: None, ..inp("foo") },
            SuggestInputs { full_prefix: "ls y/".into(), ..inp("foo") },
            SuggestInputs { full_suffix: "z".into(), ..inp("foo") },
            SuggestInputs { executable: true, ..inp("foo") },
            SuggestInputs { limit: 9, ..inp("foo") },
        ] {
            assert_eq!(c.get(&other, at(1)), None, "{other:?}");
        }
        // The first variant found that narrows to nothing ends the search: a shorter, older
        // variant is not tried.
        let mut d = Cache::default();
        d.insert(inp("f"), s(&["oo", "zed"]), t0);
        d.insert(inp("fo"), s(&["x"]), t0);
        assert_eq!(d.get(&inp("foo"), t0), None);
        // Expired at 5 s, and removed.
        assert_eq!(c.len(), 1);
        assert_eq!(c.get(&inp("fo"), at(5)), None);
        assert!(c.is_empty());
        // Purge drops what expired.
        let mut p = Cache::default();
        p.insert(inp("a"), s(&["b"]), t0);
        p.insert(inp("b"), s(&["b"]), at(4));
        p.purge(at(6));
        assert_eq!(p.len(), 1);
        // Never grows without bound.
        let mut big = Cache::default();
        for i in 0..1000 {
            big.insert(inp(&format!("p{i}")), s(&["a"]), t0);
        }
        assert!(big.len() <= 256);
    }

    #[test]
    fn suggestion_requests_follow_iterm2() {
        let r = inputs("gi", "", Some("/usr/bin:/bin"), Some("/home/u"), Some("/home/u"), false);
        assert_eq!(r, SuggestInputs { prefix: "gi".into(), full_prefix: "gi".into(), full_suffix: String::new(), directories: s(&["/usr/bin", "/bin"]), working_directory: Some("/home/u".into()), executable: true, limit: 8 });
        assert_eq!(inputs("gi", "", Some("/usr/bin:/bin"), Some("/home/u"), Some("/home/u"), true).limit, 32);
        assert_eq!(inputs("./sc", "", Some("/usr/bin:/bin"), Some("/home/u"), None, false).directories, s(&["/home/u", "/usr/bin", "/bin"]));
        let later = inputs("ls sr", "", Some("/usr/bin"), Some("/home/u"), None, false);
        assert_eq!((later.directories, later.executable, later.prefix.as_str()), (s(&["/home/u"]), false, "sr"));
        assert_eq!(inputs("ls ", "", None, Some("/home/u"), None, false).prefix, "");
        // No working directory: the remote home.
        let h = inputs("ls sr", "", None, None, Some("/root"), false);
        assert_eq!((h.directories, h.working_directory), (s(&["/root"]), None));
        // No PATH: no directories on the first word (and "/" adds nothing to nothing).
        assert!(inputs("gi", "", None, Some("/home/u"), None, false).directories.is_empty());
        assert!(inputs("./gi", "", None, Some("/home/u"), None, false).directories.is_empty());
        // Empty PATH items are kept, as `componentsSeparatedByString` does (the framer reads
        // one as the working directory).
        assert_eq!(inputs("gi", "", Some("/a::/b:"), None, None, false).directories, s(&["/a", "", "/b", ""]));
        assert_eq!(inputs("gi", "tail", None, None, None, false).full_suffix, "tail");
    }

    #[test]
    fn suggestion_entries_follow_iterm2() {
        assert_eq!(entries("fo", &s(&["o", "b/"])), vec![("foo".to_string(), false), ("fob".to_string(), true)]);
        assert_eq!(parse(r#"["a","b/"]"#), s(&["a", "b/"]));
        assert!(parse("junk").is_empty());
        assert_eq!(parse(r#"["a", 3, null, "b"]"#), s(&["a", "b"]));
    }

    #[test]
    fn suggestion_answers_are_capped_and_cleaned() {
        // At most 4 x limit entries.
        let many: Vec<String> = (0..1000).map(|i| format!("n{i}")).collect();
        let json = serde_json::to_string(&many).unwrap();
        assert_eq!(parse_limited(&json, 8).len(), 32);
        assert_eq!(parse_limited(&json, 8)[0], "n0");
        assert!(parse(&json).len() <= KEEP_FACTOR * (256 / LIMIT_DIVISOR));
        // A control character, a bidi or zero-width character, or more than 255 characters drops the entry.
        let long = "a".repeat(256);
        let ok = "a".repeat(255);
        let json = serde_json::to_string(&["good", "a\nb", "a\rb", "a\u{1b}[31m", "a\u{7f}", "a\u{85}b", "a\u{202e}b", "a\u{200b}b", &long, &ok]).unwrap();
        assert_eq!(parse_limited(&json, 64), vec!["good".to_string(), ok]);
        // Dropped entries do not count against the cap.
        let mixed: Vec<String> = (0..200).map(|i| if i % 2 == 0 { "x\ny".to_string() } else { format!("k{i}") }).collect();
        assert_eq!(parse_limited(&serde_json::to_string(&mixed).unwrap(), 8).len(), 32);
    }

    #[test]
    fn search_notifications_follow_iterm2() {
        let json = r#"{"absolutePath":"/h/a","kind":{"folder":{}}}"#;
        assert_eq!(search_notif(&format!("S1 {json}")), Some(("S1".to_string(), json.to_string())));
        assert_eq!(search_notif("nospace"), None);
        // The id the remote chose is echoed back, so it is checked: [A-Za-z0-9_-]{1,64}.
        assert!(valid_search_id("a-Z_9"));
        assert!(valid_search_id(&"a".repeat(64)));
        for bad in ["", &"a".repeat(65), "a b", "a\nb", "a\r", "x;rm", "\u{e9}", "a/b", "a=="] {
            assert!(!valid_search_id(bad), "{bad:?}");
        }
        for bad in ["", &"a".repeat(65), "a\nb", "a\r", "x;rm", "\u{e9}", "a/b", "a=="] {
            assert_eq!(search_notif(&format!("{bad} {json}")), None, "{bad:?}");
        }
        assert_eq!(search_notif(&format!("S1 {}", "x".repeat(NOTIF_MAX + 1))), None);
    }

    #[test]
    fn search_acks_go_only_to_ids_we_issued() {
        let issued = s(&["old1", "old2"]);
        assert!(ack_allowed("cur", Some("cur"), &issued));
        assert!(ack_allowed("old2", Some("cur"), &issued));
        assert!(ack_allowed("old1", None, &issued));
        assert!(!ack_allowed("evil", Some("cur"), &issued));
        assert!(!ack_allowed("evil", None, &[]));
        // Never for an id that is not a valid one, even if somehow issued.
        assert!(!ack_allowed("a b", Some("a b"), &s(&["a b"])));
        assert!(!ack_allowed("", Some(""), &[]));
    }

    #[test]
    fn search_acks_out_are_bounded() {
        let mut w = AckWindow::default();
        // A flood: 16 results are handled, the rest dropped and counted until acks come back.
        let admitted = (0..100).filter(|_| w.admit("S1")).count();
        assert_eq!((admitted, w.out("S1"), w.dropped), (ACK_WINDOW, ACK_WINDOW, 84));
        // Another search has its own window.
        assert!(w.admit("S2"));
        // An answered ack frees one place.
        w.answered("S1");
        assert!(w.admit("S1"));
        assert!(!w.admit("S1"));
        // Answers past the acks sent never go below zero, and an id with none out is forgotten.
        for _ in 0..40 {
            w.answered("S1");
        }
        assert_eq!(w.out("S1"), 0);
        assert!(w.out.get("S1").is_none());
        assert_eq!(w.dropped, 85);
    }
}


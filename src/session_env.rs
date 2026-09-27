//! The session environment (`PS:3246-3350`): the variables a new session's shell gets from
//! Tandem, and the Windows environment block that carries them with the inherited ones. Pure.

/// The session restoration key that keeps a session's cookie (iTerm2's
/// `SESSION_ARRANGEMENT_REUSABLE_COOKIE`, `PS:7395-7396`), so a shell that comes back keeps a
/// cookie that works.
pub const REUSABLE_COOKIE_KEY: &str = "Reusable Cookie";

/// Whether `s` has a cookie's form: 32 lowercase hex characters (`api_auth::cookie_text`). A
/// restored "Reusable Cookie" of another form is not put back in the jar.
pub fn is_cookie(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// iTerm2's `sessionId` (`PS:3178-3184`): `w<window>t<tab>p<pane>:<id>`.
pub fn session_id(window: usize, tab: usize, pane: usize, id: &str) -> String {
    format!("w{window}t{tab}p{pane}:{id}")
}

/// The tab and pane numbers of a session about to start (`TAB:7435-7443`): `panes` holds each
/// tab's pane count; `holding` is the tab the pane is already in (Restart Session), `split` the
/// tab a split puts it in. Already in a tab: that tab, its count less one; a split: the target
/// tab's count before the pane joins; otherwise a new tab after the last, pane 0.
pub fn numbers(panes: &[usize], holding: Option<usize>, split: Option<usize>) -> (usize, usize) {
    match (holding, split) {
        (Some(t), _) => (t, panes.get(t).map_or(0, |n| n.saturating_sub(1))),
        (None, Some(t)) => (t, panes.get(t).copied().unwrap_or(0)),
        (None, None) => (panes.len(), 0),
    }
}

/// What a new session's variables are made of.
pub struct Facts<'a> {
    pub window: usize,
    pub tab: usize,
    pub pane: usize,
    pub id: &'a str,
    pub profile_name: &'a str,
    /// The session's API cookie, when it gets one.
    pub cookie: Option<&'a str>,
}

/// The session's variables as `NAME=value`, in iTerm2's order: `ITERM2_COOKIE` (only with a
/// cookie), `ITERM_SESSION_ID`, `TERM_SESSION_ID`, `ITERM_PROFILE`.
pub fn vars(f: &Facts) -> Vec<String> {
    let id = session_id(f.window, f.tab, f.pane, f.id);
    let mut v = Vec::with_capacity(4);
    if let Some(c) = f.cookie {
        v.push(format!("ITERM2_COOKIE={c}"));
    }
    v.push(format!("ITERM_SESSION_ID={id}"));
    v.push(format!("TERM_SESSION_ID={id}"));
    v.push(format!("ITERM_PROFILE={}", f.profile_name));
    v
}

/// Variables never inherited: only a cookie Tandem minted for the session reaches it, never
/// one from Tandem's own environment (Tandem started from a Tandem shell) or a host's.
pub const NEVER_INHERITED: [&str; 2] = ["ITERM2_COOKIE", "ITERM2_KEY"];

/// `extra`'s `NAME=value` entries over `base`: a name already there (compared ignoring case,
/// as Windows compares names) is replaced, with the extra's spelling; the result is sorted by
/// the uppercase name, as Windows keeps an environment block. `NEVER_INHERITED` names in
/// `base` are dropped, whether or not `extra` sets them.
pub fn merge(base: Vec<(String, String)>, extra: &[String]) -> Vec<(String, String)> {
    let mut out = base;
    out.retain(|(n, _)| !NEVER_INHERITED.iter().any(|x| n.eq_ignore_ascii_case(x)));
    for e in extra {
        // The name ends at the first `=` after its first character: a drive's current folder
        // is kept as `=C:=C:\x`.
        let Some(at) = e.char_indices().skip(1).find(|(_, c)| *c == '=').map(|(i, _)| i) else { continue };
        let (name, value) = (&e[..at], &e[at + 1..]);
        let upper = name.to_uppercase();
        out.retain(|(n, _)| n.to_uppercase() != upper);
        out.push((name.to_string(), value.to_string()));
    }
    out.sort_by_cached_key(|(n, _)| n.to_uppercase());
    out
}

/// The pairs as a `CREATE_UNICODE_ENVIRONMENT` block: `NAME=value\0` each, then a final `\0`.
pub fn block(pairs: &[(String, String)]) -> Vec<u16> {
    let mut out = Vec::new();
    for (name, value) in pairs {
        out.extend(name.encode_utf16());
        out.push(u16::from(b'='));
        out.extend(value.encode_utf16());
        out.push(0);
    }
    // An empty block is two NULs too.
    if out.is_empty() {
        out.push(0);
    }
    out.push(0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn session_env_follows_iterm2() {
        assert_eq!(session_id(0, 1, 2, "17"), "w0t1p2:17");
        let with = Facts { window: 0, tab: 1, pane: 2, id: "17", profile_name: "Default", cookie: Some("c") };
        assert_eq!(vars(&with), vec!["ITERM2_COOKIE=c", "ITERM_SESSION_ID=w0t1p2:17", "TERM_SESSION_ID=w0t1p2:17", "ITERM_PROFILE=Default"]);
        let without = Facts { cookie: None, ..with };
        assert_eq!(vars(&without), vec!["ITERM_SESSION_ID=w0t1p2:17", "TERM_SESSION_ID=w0t1p2:17", "ITERM_PROFILE=Default"]);
        assert!(is_cookie(&crate::api_auth::cookie_text([0xab; 16])), "a minted cookie has the form");
        for bad in ["", "ABABABABABABABABABABABABABABABAB", "abab", "gbababababababababababababababab", &"a".repeat(33)] {
            assert!(!is_cookie(bad), "{bad:?}");
        }
    }

    #[test]
    fn env_block_follows_windows() {
        let merged = merge(pairs(&[("Path", "a"), ("HOME", "h")]), &["PATH=b".into(), "X=1".into()]);
        assert_eq!(merged, pairs(&[("HOME", "h"), ("PATH", "b"), ("X", "1")]), "one Path, the extra's value and spelling, sorted by uppercase name");
        let drives = merge(pairs(&[("Path", "a"), ("=C:", r"C:\x")]), &["A=1".into()]);
        assert_eq!(drives, pairs(&[("=C:", r"C:\x"), ("A", "1"), ("Path", "a")]), "a drive entry sorts first and survives");
        let later = merge(Vec::new(), &["V=1".into(), "v=2".into(), "E=".into(), "=D:=D:\\y".into()]);
        assert_eq!(later, pairs(&[("=D:", "D:\\y"), ("E", ""), ("v", "2")]), "a later extra wins; an empty value is kept; a drive name keeps its =");
        assert_eq!(block(&pairs(&[("A", "1")])), "A=1\0\0".encode_utf16().collect::<Vec<u16>>());
        assert_eq!(block(&[]), "\0\0".encode_utf16().collect::<Vec<u16>>());
    }

    #[test]
    fn inherited_cookies_never_pass() {
        let base = pairs(&[("iterm2_cookie", "stale"), ("ITERM2_KEY", "k"), ("Path", "a")]);
        assert_eq!(merge(base.clone(), &["X=1".into()]), pairs(&[("Path", "a"), ("X", "1")]), "no cookie of Tandem's: none at all");
        assert_eq!(merge(base, &["ITERM2_COOKIE=mine".into()]), pairs(&[("ITERM2_COOKIE", "mine"), ("Path", "a")]), "only the cookie minted for the session");
        assert_eq!(merge(Vec::new(), &[]), Vec::<(String, String)>::new());
    }

    #[test]
    fn numbers_follow_ptytab() {
        // Restart Session: the pane is already in tab 1 of three panes.
        assert_eq!(numbers(&[1, 3], Some(1), None), (1, 2));
        // A split into tab 0: the pane count before it joins.
        assert_eq!(numbers(&[2, 1], None, Some(0)), (0, 2));
        // A split target that no longer exists counts no panes.
        assert_eq!(numbers(&[2], None, Some(4)), (4, 0));
        // A new tab: after the last one, pane 0.
        assert_eq!(numbers(&[2, 1], None, None), (2, 0));
        assert_eq!(numbers(&[], None, None), (0, 0));
        // The holding tab wins over a split target.
        assert_eq!(numbers(&[1, 2], Some(0), Some(1)), (0, 0));
    }
}

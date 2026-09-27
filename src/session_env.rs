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

/// `extra`'s `NAME=value` entries over `base`: a name already there (compared ignoring case,
/// as Windows compares names) is replaced, with the extra's spelling; the result is sorted by
/// the uppercase name, as Windows keeps an environment block.
pub fn merge(base: Vec<(String, String)>, extra: &[String]) -> Vec<(String, String)> {
    let mut out = base;
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
}

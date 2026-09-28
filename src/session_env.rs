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
    /// The `Capabilities` encoding for `TERM_FEATURES` (`VT100Output.m:1555-1560`).
    pub term_features: &'a str,
    /// `shouldSetLCTerminal`: `LC_TERMINAL` and `LC_TERMINAL_VERSION` are set.
    pub lc_terminal: bool,
    /// The PATH with the utilities folder appended (`utilities::path_with`), when the folder
    /// was made and `addUtilitiesToPATH` is on.
    pub path: Option<&'a str>,
    /// The profile's Path to SSH (`ssh_config::SshConfig::path_to_ssh`), set as `SSH` for
    /// it2ssh (`PS:3247-3252`, `PS:3548`).
    pub ssh_path: Option<&'a str>,
    /// The label of the SSH transport a pane started on other than ConPTY
    /// (`ssh_transport::label`), set as `TANDEM_SSH_TRANSPORT` for it2ssh.
    pub ssh_transport: Option<&'a str>,
}

/// iTerm2's `TERM_PROGRAM` (`PS:3357`).
pub const TERM_PROGRAM: &str = "iTerm.app";

/// iTerm2's `LC_TERMINAL` (`PS:3333`).
pub const LC_TERMINAL: &str = "iTerm2";

/// The session's variables as `NAME=value`, in iTerm2's order: `ITERM2_COOKIE` (only with a
/// cookie), `ITERM_SESSION_ID`, `TERM_SESSION_ID`, `ITERM_PROFILE`, then iTerm2's own
/// (`PS:3332-3359`): `TERM_PROGRAM`, `TERM_PROGRAM_VERSION`, `COLORTERM`, `TERM_FEATURES`,
/// `LC_TERMINAL` and `LC_TERMINAL_VERSION` with `lc_terminal`, and `PATH` when given; then
/// Tandem's `SSH` and `TANDEM_SSH_TRANSPORT` when given.
pub fn vars(f: &Facts) -> Vec<String> {
    use crate::reports::ITERM2_VERSION;
    let id = session_id(f.window, f.tab, f.pane, f.id);
    let mut v = Vec::with_capacity(13);
    if let Some(c) = f.cookie {
        v.push(format!("ITERM2_COOKIE={c}"));
    }
    v.push(format!("ITERM_SESSION_ID={id}"));
    v.push(format!("TERM_SESSION_ID={id}"));
    v.push(format!("ITERM_PROFILE={}", f.profile_name));
    v.push(format!("TERM_PROGRAM={TERM_PROGRAM}"));
    v.push(format!("TERM_PROGRAM_VERSION={ITERM2_VERSION}"));
    v.push("COLORTERM=truecolor".to_string());
    v.push(format!("TERM_FEATURES={}", f.term_features));
    if f.lc_terminal {
        v.push(format!("LC_TERMINAL={LC_TERMINAL}"));
        v.push(format!("LC_TERMINAL_VERSION={ITERM2_VERSION}"));
    }
    if let Some(p) = f.path {
        v.push(format!("PATH={p}"));
    }
    if let Some(p) = f.ssh_path {
        v.push(format!("SSH={p}"));
    }
    if let Some(t) = f.ssh_transport {
        v.push(format!("TANDEM_SSH_TRANSPORT={t}"));
    }
    v
}

/// Variables never inherited: only a cookie Tandem minted for the session reaches it, never
/// one from Tandem's own environment (Tandem started from a Tandem shell) or a host's.
pub const NEVER_INHERITED: [&str; 2] = ["ITERM2_COOKIE", "ITERM2_KEY"];

/// The session's variables with `extra`'s `NAME=value` entries over them (the REPL's cookie
/// and key): a name already there (compared ignoring case) is replaced where it is, so no
/// name is given twice; others follow at the end.
pub fn overlay(vars: Vec<String>, extra: &[String]) -> Vec<String> {
    let name = |e: &str| e.split_once('=').map_or(e.to_uppercase(), |(n, _)| n.to_uppercase());
    let mut out = vars;
    for e in extra {
        match out.iter_mut().find(|v| name(v) == name(e)) {
            Some(v) => *v = e.clone(),
            None => out.push(e.clone()),
        }
    }
    out
}

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
    fn overlay_replaces_same_names() {
        let vars = vec!["ITERM2_COOKIE=session".to_string(), "ITERM_SESSION_ID=w0t0p0:x".to_string()];
        let extra = ["iterm2_cookie=repl".to_string(), "ITERM2_KEY=K".to_string()];
        assert_eq!(overlay(vars, &extra), ["iterm2_cookie=repl", "ITERM_SESSION_ID=w0t0p0:x", "ITERM2_KEY=K"]);
        assert_eq!(overlay(vec!["A=1".to_string()], &[]), ["A=1"]);
    }

    #[test]
    fn ssh_vars_reach_the_environment() {
        let features = "F";
        let none = Facts { window: 0, tab: 0, pane: 0, id: "1", profile_name: "P", cookie: None, term_features: features, lc_terminal: false, path: None, ssh_path: None, ssh_transport: None };
        let base = vars(&none);
        assert!(!base.iter().any(|v| v.starts_with("SSH=") || v.starts_with("TANDEM_SSH_TRANSPORT=")), "neither without them");
        let with_path = vars(&Facts { ssh_path: Some(r"C:\ssh\ssh.exe"), ..none });
        assert_eq!(with_path[..base.len()], base[..]);
        assert_eq!(with_path[base.len()..], ["SSH=C:\\ssh\\ssh.exe"]);
        let with_transport = vars(&Facts { ssh_transport: Some("pipes"), ..none });
        assert_eq!(with_transport[base.len()..], ["TANDEM_SSH_TRANSPORT=pipes"]);
    }

    #[test]
    fn session_env_follows_iterm2() {
        assert_eq!(session_id(0, 1, 2, "17"), "w0t1p2:17");
        let features = crate::osc1337::capabilities(crate::osc1337::Features { clipboard_writable: false, ambiguous_wide: false, unicode_version: 9 });
        assert_eq!(features, "T3MSc7UUw9Ts3BFGsSyHSxF");
        let with = Facts { window: 0, tab: 1, pane: 2, id: "17", profile_name: "Default", cookie: Some("c"), term_features: &features, lc_terminal: true, path: Some("C:\\W;C:\\T\\utilities"), ssh_path: None, ssh_transport: None };
        let tail = [
            "TERM_PROGRAM=iTerm.app",
            "TERM_PROGRAM_VERSION=3.7.0",
            "COLORTERM=truecolor",
            "TERM_FEATURES=T3MSc7UUw9Ts3BFGsSyHSxF",
            "LC_TERMINAL=iTerm2",
            "LC_TERMINAL_VERSION=3.7.0",
            "PATH=C:\\W;C:\\T\\utilities",
        ];
        let mut want = vec!["ITERM2_COOKIE=c", "ITERM_SESSION_ID=w0t1p2:17", "TERM_SESSION_ID=w0t1p2:17", "ITERM_PROFILE=Default"];
        want.extend(tail);
        assert_eq!(vars(&with), want);
        let without = Facts { cookie: None, lc_terminal: false, path: None, ..with };
        let mut want = vec!["ITERM_SESSION_ID=w0t1p2:17", "TERM_SESSION_ID=w0t1p2:17", "ITERM_PROFILE=Default"];
        want.extend(&tail[..4]);
        assert_eq!(vars(&without), want, "no LC_TERMINAL pair and no PATH");
        let merged = merge(pairs(&[("Path", "C:\\W"), ("HOME", "h")]), &vars(&with));
        let paths: Vec<&(String, String)> = merged.iter().filter(|(n, _)| n.eq_ignore_ascii_case("path")).collect();
        assert_eq!(paths, vec![&("PATH".to_string(), "C:\\W;C:\\T\\utilities".to_string())], "one PATH entry, the session's");
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

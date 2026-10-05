//! The session environment (`PS:3246-3350`): the variables a new session's shell gets from
//! Tandem, and the Windows environment block that carries them with the inherited ones. Pure.

use crate::palette::Rgb;

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

/// The `TERM=<type>` entry a new session gets: only when no entry of its own names TERM (any
/// case) and Tandem's environment has none, as iTerm2 sets it only when the environment lacks
/// it (`PS:3253-3255`).
pub fn term_var(env: &[String], inherited_has_term: bool, term: &str) -> Option<String> {
    let named = env.iter().any(|e| e.split('=').next().is_some_and(|n| n.eq_ignore_ascii_case("TERM")));
    (!named && !inherited_has_term).then(|| format!("TERM={term}"))
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
/// the uppercase name, as Windows keeps an environment block. An extra entry holding a NUL is
/// dropped whole (B3): it would split the block into a second variable. `NEVER_INHERITED` names in
/// `base` are dropped, whether or not `extra` sets them.
pub fn merge(base: Vec<(String, String)>, extra: &[String]) -> Vec<(String, String)> {
    let mut out = base;
    out.retain(|(n, _)| !NEVER_INHERITED.iter().any(|x| n.eq_ignore_ascii_case(x)));
    for e in extra.iter().filter(|e| !e.contains('\0')) {
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

/// The `COLORFGBG` value (`PS:5505-5535`): the last ANSI color equal to the foreground and the
/// last equal to the background as `fg;bg`; with no match for either, `15;0` when the
/// foreground is brighter (HSB brightness) than the background and `0;15` otherwise, if
/// `fallback` (`UseColorfgbgFallback`) is on, else nothing.
pub fn colorfgbg(ansi: &[Rgb; 16], fg: Rgb, bg: Rgb, fallback: bool) -> Option<String> {
    let (mut fg_at, mut bg_at) = (None, None);
    for (i, c) in ansi.iter().enumerate() {
        if *c == fg {
            fg_at = Some(i);
        }
        if *c == bg {
            bg_at = Some(i);
        }
    }
    if let (Some(f), Some(b)) = (fg_at, bg_at) {
        return Some(format!("{f};{b}"));
    }
    // HSB brightness is the largest component.
    let brightness = |c: Rgb| c.r.max(c.g).max(c.b);
    fallback.then(|| if brightness(fg) > brightness(bg) { "15;0" } else { "0;15" }.to_string())
}

/// The POSIX variables Tandem adds to a session beside `vars`.
#[derive(Clone, Debug, Default)]
pub struct Posix {
    pub colorfgbg: Option<String>,
    /// `LANG` or `LC_CTYPE`, in the order to set.
    pub locale: Vec<(String, String)>,
    /// The Windows path of Tandem's terminfo folder, for a WSL session only.
    pub terminfo: Option<String>,
    /// Tandem's own `WSLENV`, kept so a user's setup is not lost.
    pub inherited_wslenv: Option<String>,
    /// A WSL session that gets 13o's `TERM` (the profile's Terminal Type): `WSLENV` names it, as
    /// WSL imports no Windows variable it does not list (P17). The `TERM=` entry itself is
    /// `term_var`'s.
    pub wsl_term: bool,
}

/// `WSLENV` with `add`'s `(name, flags)` entries appended after `inherited`'s; a name already
/// listed (ignoring ASCII case, flags aside) is not added again.
pub fn wslenv(inherited: Option<&str>, add: &[(&str, &str)]) -> String {
    let name = |e: &str| e.split('/').next().unwrap_or("").to_string();
    let mut out: Vec<String> = inherited.unwrap_or("").split(':').filter(|e| !e.is_empty()).map(str::to_string).collect();
    for (n, flags) in add {
        if !out.iter().any(|e| name(e).eq_ignore_ascii_case(n)) {
            out.push(if flags.is_empty() { n.to_string() } else { format!("{n}/{flags}") });
        }
    }
    out.join(":")
}

/// The POSIX variables as `NAME=value`: `COLORFGBG`, the locale pairs, `TERMINFO_DIRS`, then
/// `WSLENV` naming each added variable so WSL passes it on; no `WSLENV` when nothing was added.
pub fn posix_vars(p: &Posix) -> Vec<String> {
    let mut out = Vec::new();
    let mut added: Vec<(&str, &str)> = Vec::new();
    if let Some(c) = &p.colorfgbg {
        out.push(format!("COLORFGBG={c}"));
        added.push(("COLORFGBG", ""));
    }
    for (n, v) in &p.locale {
        out.push(format!("{n}={v}"));
        added.push((n, ""));
    }
    if let Some(t) = &p.terminfo {
        out.push(format!("TERMINFO_DIRS={t}"));
        // `/p`: WSL translates the Windows path to its own.
        added.push(("TERMINFO_DIRS", "p"));
    }
    if p.wsl_term {
        added.push(("TERM", ""));
    }
    if !added.is_empty() {
        out.push(format!("WSLENV={}", wslenv(p.inherited_wslenv.as_deref(), &added)));
    }
    out
}

/// Whether the command line starts `wsl.exe` or `wslhost.exe`: its first token, quotes
/// stripped, file name lowercased without `.exe`. A distribution launcher (`ubuntu.exe`) is not
/// recognized.
pub fn is_wsl(cmdline: &str) -> bool {
    let line = cmdline.trim_start();
    let first = match line.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(""),
        None => line.split_whitespace().next().unwrap_or(""),
    };
    let file = first.rsplit(['\\', '/']).next().unwrap_or("").to_ascii_lowercase();
    let stem = file.strip_suffix(".exe").unwrap_or(&file);
    matches!(stem, "wsl" | "wslhost")
}

/// Whether `path` is a UNC or device path (`\\host\share`, `//host`): never probed or written
/// from the UI thread.
pub fn is_unc(path: &str) -> bool {
    let p = path.trim_start();
    p.starts_with("\\\\") || p.starts_with("//")
}

/// The roaming application data folder (`%APPDATA%`), or `None` when it is not set or is a
/// network path (folder redirection): Tandem writes its terminfo and the PowerShell
/// integration script only to a local folder (P12).
pub fn appdata_local() -> Option<std::path::PathBuf> {
    appdata_local_from(std::env::var("APPDATA").ok().as_deref())
}

/// `appdata_local` over a given `%APPDATA%` value.
pub fn appdata_local_from(appdata: Option<&str>) -> Option<std::path::PathBuf> {
    appdata.filter(|a| !a.trim().is_empty() && !is_unc(a)).map(std::path::PathBuf::from)
}

/// The pairs as a `CREATE_UNICODE_ENVIRONMENT` block: `NAME=value\0` each, then a final `\0`.
pub fn block(pairs: &[(String, String)]) -> Vec<u16> {
    let mut out = Vec::new();
    for (name, value) in pairs {
        debug_assert!(!name.contains('\0') && !value.contains('\0'), "a NUL would split the block");
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


    fn rgbs() -> [Rgb; 16] {
        std::array::from_fn(|i| crate::palette::rgb(i as u8 * 10, i as u8 * 10, i as u8 * 10))
    }

    #[test]
    fn colorfgbg_follows_iterm2() {
        let a = rgbs();
        assert_eq!(colorfgbg(&a, a[15], a[0], false).as_deref(), Some("15;0"));
        assert_eq!(colorfgbg(&a, a[15], a[0], true).as_deref(), Some("15;0"));
        // The last match wins.
        let mut dup = a;
        dup[7] = dup[15];
        assert_eq!(colorfgbg(&dup, a[15], a[0], false).as_deref(), Some("15;0"));
        assert_eq!(colorfgbg(&dup, a[15], a[1], false).as_deref(), Some("15;1"));
        let mut both = a;
        both[3] = both[0];
        assert_eq!(colorfgbg(&both, a[15], a[0], false).as_deref(), Some("15;3"), "the last background match");
        // No match: the fallback by brightness, or nothing.
        let (bright, dark, mid) = (crate::palette::rgb(250, 251, 252), crate::palette::rgb(1, 2, 3), crate::palette::rgb(0, 0, 77));
        assert_eq!(colorfgbg(&a, bright, dark, false), None);
        assert_eq!(colorfgbg(&a, bright, dark, true).as_deref(), Some("15;0"));
        assert_eq!(colorfgbg(&a, dark, bright, true).as_deref(), Some("0;15"));
        assert_eq!(colorfgbg(&a, mid, mid, true).as_deref(), Some("0;15"), "equal brightness");
        // The brightness is the largest component (HSB), not the average.
        assert_eq!(colorfgbg(&a, crate::palette::rgb(200, 0, 0), crate::palette::rgb(100, 100, 100), true).as_deref(), Some("15;0"));
        // A match for one side only is no match.
        assert_eq!(colorfgbg(&a, a[15], dark, true).as_deref(), Some("15;0"));
        assert_eq!(colorfgbg(&a, a[15], dark, false), None);
    }

    #[test]
    fn posix_vars_follow_iterm2() {
        let full = Posix {
            colorfgbg: Some("15;0".into()),
            locale: vec![("LANG".into(), "en_US.UTF-8".into())],
            terminfo: Some(r"C:\A\Tandem\terminfo".into()),
            inherited_wslenv: None,
            wsl_term: false,
        };
        assert_eq!(posix_vars(&full), ["COLORFGBG=15;0", "LANG=en_US.UTF-8", r"TERMINFO_DIRS=C:\A\Tandem\terminfo", "WSLENV=COLORFGBG:LANG:TERMINFO_DIRS/p"]);
        assert_eq!(posix_vars(&Posix::default()), Vec::<String>::new());
        let lc = Posix { locale: vec![("LC_CTYPE".into(), "UTF-8".into())], ..Posix::default() };
        assert_eq!(posix_vars(&lc), ["LC_CTYPE=UTF-8", "WSLENV=LC_CTYPE"]);
        let kept = Posix { colorfgbg: Some("0;15".into()), inherited_wslenv: Some("USERPROFILE/p".into()), ..Posix::default() };
        assert_eq!(posix_vars(&kept), ["COLORFGBG=0;15", "WSLENV=USERPROFILE/p:COLORFGBG"]);
        for yes in ["wsl.exe", r#""C:\Windows\System32\wsl.exe" -d Ubuntu"#, "WSL", "wslhost.exe --x", r"C:\Windows\System32\WSL.EXE -d x", "  wsl  "] {
            assert!(is_wsl(yes), "{yes}");
        }
        for no in ["pwsh.exe", r"C:\msys64\usr\bin\bash.exe --login", "cmd.exe", "ubuntu.exe", "", "wsl2.exe", r"C:\x\notwsl.exe", r"C:\wsl.exe\cmd.exe"] {
            assert!(!is_wsl(no), "{no}");
        }
    }

    /// P17: a WSL session given 13o's `TERM` lists it in `WSLENV` (no flag: a name, not a
    /// path), after Tandem's own entries and once even when the inherited value names it.
    #[test]
    fn wslenv_lists_term_for_wsl() {
        let full = Posix {
            colorfgbg: Some("15;0".into()),
            locale: vec![("LANG".into(), "en_US.UTF-8".into())],
            terminfo: Some(r"C:\A\Tandem\terminfo".into()),
            inherited_wslenv: None,
            wsl_term: true,
        };
        assert_eq!(posix_vars(&full).last().map(String::as_str), Some("WSLENV=COLORFGBG:LANG:TERMINFO_DIRS/p:TERM"));
        let only = Posix { wsl_term: true, ..Posix::default() };
        assert_eq!(posix_vars(&only), ["WSLENV=TERM"], "TERM alone still needs WSLENV");
        let kept = Posix { wsl_term: true, inherited_wslenv: Some("term/u:X".into()), ..Posix::default() };
        assert_eq!(posix_vars(&kept), ["WSLENV=term/u:X"], "a user's own TERM entry is kept as written");
        assert!(!posix_vars(&Posix::default()).iter().any(|v| v.contains("TERM")));
    }

    #[test]
    fn wslenv_appends_once() {
        assert_eq!(wslenv(Some("USERPROFILE/p:lang"), &[("LANG", ""), ("COLORFGBG", "")]), "USERPROFILE/p:lang:COLORFGBG");
        assert_eq!(wslenv(Some(""), &[("TERMINFO_DIRS", "p")]), "TERMINFO_DIRS/p");
        assert_eq!(wslenv(None, &[]), "");
        assert_eq!(wslenv(Some("A/l::B"), &[]), "A/l:B", "empty entries go");
        assert_eq!(wslenv(None, &[("X", ""), ("x", "p")]), "X", "added once");
    }

    #[test]
    fn merge_drops_entries_with_a_nul() {
        // B3: a NUL inside an extra entry would split the environment block.
        let evil = "ITERM_PROFILE=a\0PATH=x".to_string();
        assert_eq!(merge(pairs(&[("Path", "a")]), &[evil.clone()]), pairs(&[("Path", "a")]));
        assert_eq!(merge(Vec::new(), &["LANG=x\0PATH=C:\\evil".into(), "OK=1".into(), "A\0B=1".into()]), pairs(&[("OK", "1")]));
        // A name ends at its first `=` after its first character, so a value holding `=` is
        // one variable, never two.
        assert_eq!(merge(Vec::new(), &["LANG=a=b".into()]), pairs(&[("LANG", "a=b")]));
        assert_eq!(merge(Vec::new(), &["=".into(), "".into(), "=x".into()]), Vec::<(String, String)>::new());
    }

    #[test]
    fn appdata_refuses_network_paths() {
        assert_eq!(appdata_local_from(Some(r"C:\Users\a\AppData\Roaming")), Some(std::path::PathBuf::from(r"C:\Users\a\AppData\Roaming")));
        for no in [None, Some(""), Some("  "), Some(r"\\host\home\a"), Some("//host/home"), Some(r"\\?\UNC\h\s")] {
            assert_eq!(appdata_local_from(no), None, "{no:?}");
        }
        assert!(is_unc(r"\\h\s") && !is_unc(r"C:\h"));
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

    #[test]
    fn term_follows_iterm2() {
        assert_eq!(term_var(&[], false, "xterm-256color").as_deref(), Some("TERM=xterm-256color"));
        assert_eq!(term_var(&["Term=x".into()], false, "xterm"), None);
        assert_eq!(term_var(&["ITERM_PROFILE=a".into()], false, "xterm").as_deref(), Some("TERM=xterm"));
        assert_eq!(term_var(&[], true, "xterm"), None);
    }
}

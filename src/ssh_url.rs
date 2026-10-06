//! The ssh: URL rule (m14e Task 1; iTerm2 `iTermSessionLauncher.m` `toSshTo:`, `NSStringITerm.m`
//! `sanitizedUsername`/`sanitizedHostname`). An ssh: URL comes from outside Tandem, so it is hostile
//! input that becomes an ssh command line: every word taken from it must pass iTerm2's check AND the
//! 14c allowlist grammar (`ssh_jumps::{user_is_safe, host_is_safe, port_is_safe}`, one validator,
//! no copy), and the only other words are generated here (`-l`, `-p`, `-t`, the cd word).

use crate::profile::Profile;
use crate::ssh_jumps::{host_is_safe, port_is_safe, user_is_safe};
use toml::Value;

/// A parsed `ssh:` URL, as NSURL's `user`, `host`, `port` and `path` (all percent-decoded).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SshUrl {
    pub user: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub path: String,
}

/// The settings a URL reads (`SshIntegrationForURLs`, `sshSchemePath`, `validCharactersInSSHUserNames`,
/// `sshURLsSupportPath`, and for ftp: and telnet: `PathToFTP`, `PathToTelnet`), read each time a URL opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub integration: bool,
    pub scheme_path: String,
    pub valid_user_chars: String,
    pub support_path: bool,
    /// `PathToFTP` and `PathToTelnet`: the program an ftp: or telnet: URL runs (`url_open`).
    pub ftp_path: String,
    pub telnet_path: String,
}

impl Settings {
    pub fn from_globals(g: &crate::globals::Globals) -> Settings {
        Settings {
            integration: g.bool("SshIntegrationForURLs"),
            scheme_path: g.string("sshSchemePath"),
            valid_user_chars: g.string("validCharactersInSSHUserNames"),
            support_path: g.bool("sshURLsSupportPath"),
            ftp_path: g.string("PathToFTP"),
            telnet_path: g.string("PathToTelnet"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UrlError {
    /// The user name holds a character that is not allowed (the person is warned).
    IllegalUsername(String),
    /// The host is not allowed; nothing opens and nothing is shown (iTerm2's).
    BadHostname,
    /// No host to connect to (or an ssh URL that does not parse): nothing opens.
    NoHost,
}

/// What a URL changes in a profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// `Custom Command`: "SSH" with integration, else "Yes".
    pub custom_command: &'static str,
    /// `Command`.
    pub command: String,
    /// `Run Command In Login Shell`, set only to `Some(true)` (iTerm2 never writes NO, `SL:635-637`).
    pub login_shell: Option<bool>,
}

pub const ILLEGAL_USERNAME_HEADING: &str = "Illegal Username";

/// The warning for a refused user name. `user` is URL text: the caller shows it through
/// `files_ui::shown_name` when it draws it.
pub fn illegal_username(user: &str) -> String {
    format!(
        "The SSH user name \u{201c}{user}\u{201d} contained a disallowed character. The set of allowed characters is limited for security reasons. You can modify it in Settings > Advanced > Valid characters in SSH user names."
    )
}

/// A URL longer than this does not parse (the hand-over routes cap theirs at 8 KiB too).
const MAX_URL: usize = 8 * 1024;

/// A character that never belongs in a URL's text: whitespace, controls, and the invisible
/// formatting characters that can disguise what a link says.
pub(crate) fn is_hostile_char(c: char) -> bool {
    c.is_control() || c.is_whitespace() || matches!(c, '\u{ad}' | '\u{61c}' | '\u{180e}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2069}' | '\u{feff}')
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            if !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Parses an `ssh:` URL. None: not an `ssh:` URL, or it does not parse (a space or control in
/// it, bad percent-encoding, a port that is not 1-65535). No `//`, or an empty host, means no host.
pub fn parse(url: &str) -> Option<SshUrl> {
    parse_as(url, "ssh")
}

/// The same grammar for another scheme (`telnet`, `ftp`: `url_open`), the one parser of all three.
pub fn parse_as(url: &str, wanted: &str) -> Option<SshUrl> {
    if url.len() > MAX_URL || url.chars().any(is_hostile_char) {
        return None;
    }
    let (scheme, rest) = url.split_once(':')?;
    if !scheme.eq_ignore_ascii_case(wanted) {
        return None;
    }
    let Some(after) = rest.strip_prefix("//") else {
        let path = rest.split(['?', '#']).next().unwrap_or("");
        return Some(SshUrl { user: None, host: None, port: None, path: percent_decode(path)? });
    };
    let end = after.find(['/', '?', '#']).unwrap_or(after.len());
    let (authority, tail) = after.split_at(end);
    let path = if tail.starts_with('/') { tail.split(['?', '#']).next().unwrap_or("") } else { "" };
    let (userinfo, hostport) = match authority.rsplit_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, authority),
    };
    let user = userinfo.map(|u| u.split(':').next().unwrap_or("")).filter(|u| !u.is_empty());
    let user = match user {
        Some(u) => Some(percent_decode(u)?),
        None => None,
    };
    let (host, port) = match hostport.strip_prefix('[') {
        Some(inner) => {
            let (h, after_bracket) = inner.split_once(']')?;
            let port = match after_bracket {
                "" => None,
                p => Some(p.strip_prefix(':')?),
            };
            (h, port)
        }
        None => match hostport.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (hostport, None),
        },
    };
    let port = match port {
        None | Some("") => None,
        Some(p) => {
            if !port_is_safe(p) {
                return None;
            }
            Some(p.parse::<u16>().ok()?)
        }
    };
    let host = Some(percent_decode(host)?).filter(|h| !h.is_empty());
    Some(SshUrl { user, host, port, path: percent_decode(path)? })
}

/// iTerm2's backslash escaping (`stringWithEscapedShellCharactersIncludingNewlines:YES`), for the
/// remote cd path the remote shell reads.
pub fn posix_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if "\\ ()\"&'!$<>;|*?[]#`\t{}^+=@~\r\n".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// iTerm2's user name rule alone (`NS:2792-2803`): alphanumerics in any script plus `extra`,
/// then escaped. Kept for its tests; a command is gated by `username_ok`, which also applies 14c's.
pub fn sanitized_username(user: &str, extra: &str) -> Option<String> {
    user.chars().all(|c| c.is_alphanumeric() || extra.contains(c)).then(|| posix_escape(user))
}

/// iTerm2's host rule alone (`NS:2805-2823`): the first character a letter or digit, the rest
/// `:a-zA-Z0-9-.`. An empty string passes both checks. Kept for its tests; see `hostname_ok`.
pub fn sanitized_hostname(host: &str) -> Option<String> {
    let first = host.chars().next();
    let first_ok = first.is_none_or(|c| c.is_ascii_alphanumeric());
    (first_ok && host.chars().all(|c| c.is_ascii_alphanumeric() || ":-.".contains(c))).then(|| posix_escape(host))
}

/// Whether a URL's user may reach a command line: iTerm2's check (with the setting's characters,
/// which can only narrow) AND the 14c allowlist grammar.
pub fn username_ok(user: &str, extra: &str) -> bool {
    sanitized_username(user, extra).is_some() && user_is_safe(user)
}

/// Whether a URL's host may reach a command line: iTerm2's check AND the 14c grammar. No IPv6
/// (iTerm2 refuses a leading colon; the grammar wants brackets that ssh.exe does not take).
pub fn hostname_ok(host: &str) -> bool {
    sanitized_hostname(host).is_some() && !host.starts_with('[') && host_is_safe(host, false)
}

/// Whether `sshSchemePath` is used: it holds a letter or digit and is not exactly "ssh" (`SL:562-640`).
pub fn scheme_path_usable(path: &str) -> bool {
    path != "ssh" && path.chars().any(char::is_alphanumeric)
}

/// The longest remote folder a URL may name (ruling R10).
pub const MAX_REMOTE_FOLDER: usize = 1024;

/// Ruling R10 (security, beyond iTerm2): the only remote folders a URL may cd to, in both modes.
/// With integration on the cd word is parsed by a shell twice (it2ssh's requote, `ssh_args::parse`
/// re-splitting the command words, then conductor.sh's `$SHELL -c`), so no escaping makes a shell
/// character safe; the folder must hold none. `[A-Za-z0-9._/~+@%=,-]` only, not starting with
/// `-`, at most `MAX_REMOTE_FOLDER` bytes, and a leading `~` only as `~` or `~/` (no `~user`,
/// `~+`, `~-`). `dir` is the decoded folder the cd word would name.
pub fn remote_folder_allowed(dir: &str) -> bool {
    let chars_ok = dir.bytes().all(|b| b.is_ascii_alphanumeric() || b"._/~+@%=,-".contains(&b));
    let tilde_ok = dir.strip_prefix('~').is_none_or(|rest| rest.is_empty() || rest.starts_with('/'));
    !dir.is_empty() && dir.len() <= MAX_REMOTE_FOLDER && chars_ok && tilde_ok && !dir.starts_with('-')
}

/// The remote folder a URL names, as the cd part takes it (a `/~` prefix loses its "/"): None when
/// path support is off, the path is `/` or empty, the folder fails `remote_folder_allowed`, or it
/// starts `//`, which a Windows reader takes for a UNC path (ruling r1).
pub fn cd_path(url: &SshUrl, s: &Settings) -> Option<String> {
    let use_path = s.support_path && url.path.chars().count() > 1;
    if !use_path || url.path.starts_with("//") {
        return None;
    }
    let dir = url.path.strip_prefix("/~").map_or(url.path.clone(), |rest| format!("~{rest}"));
    remote_folder_allowed(&dir).then_some(dir)
}

/// The command a URL gives, as a Windows command line tail (14a Task 9 hands it to it2ssh.exe or
/// ssh.exe through CreateProcessW: the user and host are separate arguments, only the cd path is
/// POSIX-escaped for the remote shell).
pub fn ssh_change(url: &SshUrl, s: &Settings) -> Result<Change, UrlError> {
    // P13(d): a URL with no host opens nothing in both modes (never a bare ssh from outside).
    if url.host.is_none() {
        return Err(UrlError::NoHost);
    }
    let mut words: Vec<String> = Vec::new();
    if let Some(user) = &url.user {
        if !username_ok(user, &s.valid_user_chars) {
            return Err(UrlError::IllegalUsername(user.clone()));
        }
        words.push("-l".into());
        words.push(user.clone());
    }
    if let Some(port) = url.port {
        if !port_is_safe(&port.to_string()) {
            return Err(UrlError::BadHostname);
        }
        words.push("-p".into());
        words.push(port.to_string());
    }
    let use_path = s.support_path && url.path.chars().count() > 1;
    if use_path {
        words.push("-t".into());
    }
    if let Some(host) = &url.host {
        if !hostname_ok(host) {
            return Err(UrlError::BadHostname);
        }
        words.push(host.clone());
    }
    if let Some(path) = cd_path(url, s) {
        words.push(format!("cd {}; exec $SHELL -l", posix_escape(&path)));
    }
    let mut line = String::new();
    for w in &words {
        if !line.is_empty() {
            line.push(' ');
        }
        crate::ssh_probe::quote_arg(w, &mut line);
    }
    if s.integration {
        return Ok(Change { custom_command: "SSH", command: line, login_shell: None });
    }
    let (program, login_shell) = if scheme_path_usable(&s.scheme_path) { (s.scheme_path.as_str(), None) } else { ("ssh", Some(true)) };
    let command = if line.is_empty() { program.to_string() } else { format!("{program} {line}") };
    Ok(Change { custom_command: "Yes", command, login_shell })
}

/// The profile a URL opens (`SL:471-509`): changed when its command is "$$" or its type is not
/// Custom. Ok(None): the profile is used unchanged (not an ssh:, telnet: or ftp: URL, or a custom
/// command of its own). A URL that does not parse is `NoHost`: nothing opens (iTerm2 would open
/// a bare command).
pub fn profile_for_url(profile: &Profile, url: &str, s: &Settings) -> Result<Option<Profile>, UrlError> {
    if crate::url_open::scheme_of(url).is_none() {
        return Ok(None);
    }
    if profile.string("Custom Command") == "Yes" && profile.string("Command") != "$$" {
        return Ok(None);
    }
    let Some(change) = crate::url_open::change_for(url, s)? else { return Ok(None) };
    let mut changed = profile.clone();
    changed.set("Custom Command", Value::String(change.custom_command.into()));
    changed.set("Command", Value::String(change.command));
    if change.login_shell == Some(true) {
        changed.set("Run Command In Login Shell", Value::Boolean(true));
    }
    Ok(Some(changed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::Profile;
    use toml::Value;

    fn s(v: &str) -> Value {
        Value::String(v.into())
    }

    fn settings(integration: bool) -> Settings {
        Settings { integration, scheme_path: "ssh".into(), valid_user_chars: "_-+.".into(), support_path: true, ftp_path: "ftp".into(), telnet_path: "telnet".into() }
    }

    fn url(u: &str) -> SshUrl {
        parse(u).unwrap_or_else(|| panic!("{u} parses"))
    }

    fn on(u: &str) -> Result<Change, UrlError> {
        ssh_change(&url(u), &settings(true))
    }

    fn off(u: &str) -> Result<Change, UrlError> {
        ssh_change(&url(u), &settings(false))
    }

    fn on_err(u: &str) -> UrlError {
        on(u).unwrap_err()
    }

    fn command(r: Result<Change, UrlError>) -> String {
        r.unwrap().command
    }

    #[test]
    fn url_parse_follows_iterm2() {
        let full = SshUrl { user: Some("bob".into()), host: Some("h.example".into()), port: Some(2222), path: "/srv/www".into() };
        assert_eq!(parse("ssh://bob@h.example:2222/srv/www"), Some(full));
        let bare = parse("SSH://h").unwrap();
        assert_eq!((bare.user, bare.host.as_deref(), bare.port, bare.path.as_str()), (None, Some("h"), None, ""));
        assert_eq!(url("ssh://bob:secret@h").user.as_deref(), Some("bob"));
        let escaped = url("ssh://b%20o@h/a%20b");
        assert_eq!((escaped.user.as_deref(), escaped.path.as_str()), (Some("b o"), "/a b"));
        assert_eq!(url("ssh://[::1]:22/").host.as_deref(), Some("::1"));
        assert_eq!(url("ssh://[::1]:22/").port, Some(22));
        assert_eq!(parse("ssh://h:notaport"), None);
        assert_eq!(parse("http://h"), None);
        assert_eq!(url("ssh:h").host, None);
        assert_eq!(parse("not a url"), None);
        // The query and fragment are not the path; an empty authority has no host.
        assert_eq!(url("ssh://h/p?x=1#f").path, "/p");
        assert_eq!(url("ssh://").host, None);
        assert_eq!(url("ssh://u@").host, None);
        // Ports: decimal digits naming 1-65535 only.
        for bad in ["ssh://h:99999", "ssh://h:+22", "ssh://h:-1", "ssh://h:0", "ssh://h:2 2", "ssh://u@h:99999/"] {
            assert_eq!(parse(bad), None, "{bad}");
        }
        // Percent-decoding that is not UTF-8, and URLs holding spaces or controls, do not parse.
        assert_eq!(parse("ssh://h/%ff"), None);
        assert_eq!(parse("ssh://h/a b"), None);
        assert_eq!(parse("ssh://h/a\nb"), None);
        // A backslash is not a URL error but a bad host, caught by the host check.
        assert_eq!(url("ssh://h\\").host.as_deref(), Some("h\\"));
    }

    #[test]
    fn usernames_follow_iterm2() {
        assert_eq!(sanitized_username("bob", "_-+.").as_deref(), Some("bob"));
        assert_eq!(sanitized_username("a+b.c_d-e", "_-+.").as_deref(), Some("a\\+b.c_d-e"));
        for bad in ["b o", "a;b", "$(x)", "a'b", "a@b"] {
            assert_eq!(sanitized_username(bad, "_-+."), None, "{bad}");
        }
        assert!(sanitized_username("jos\u{e9}", "_-+.").is_some());
        assert_eq!(sanitized_username("a@b", "@").as_deref(), Some("a\\@b"));
        assert_eq!(sanitized_username("b o", "_-+. ").as_deref(), Some("b\\ o"));
        assert_eq!(posix_escape("a b(c)"), "a\\ b\\(c\\)");
        assert_eq!(posix_escape("x\ny"), "x\\\ny");
        assert_eq!(posix_escape("~"), "\\~");
        assert_eq!(posix_escape("~/w"), "\\~/w");
        for c in "\\ ()\"&'!$<>;|*?[]#`\t{}^+=@~\r\n".chars() {
            assert_eq!(posix_escape(&c.to_string()), format!("\\{c}"), "{c:?}");
        }
        // P2: the check that gates a command is iTerm2's AND the 14c grammar; the setting only narrows.
        assert!(username_ok("bob", "_-+."));
        assert!(username_ok("a.b_c-d", "_-+."));
        assert!(!username_ok("a+b", "_-+."), "+ is outside the 14c grammar");
        assert!(!username_ok("jos\u{e9}", "_-+."));
        assert!(!username_ok("b o", "_-+. "));
        assert!(!username_ok("a@b", "@"));
        assert!(!username_ok("-oProxyCommand=x", "_-+."), "a leading dash is an ssh option");
        assert!(!username_ok("a.b", "_-"), "the extra set narrows: . is not allowed here");
        assert!(!username_ok("", "_-+."));
    }

    #[test]
    fn hostnames_follow_iterm2() {
        assert_eq!(sanitized_hostname("h.example").as_deref(), Some("h.example"));
        assert_eq!(sanitized_hostname("10.0.0.1").as_deref(), Some("10.0.0.1"));
        // P3: a leading colon is illegal in iTerm2, so IPv6 hosts are refused.
        assert_eq!(sanitized_hostname("::1"), None);
        assert_eq!(sanitized_hostname("-oProxyCommand=x"), None);
        assert_eq!(sanitized_hostname(".h"), None);
        for bad in ["h/x", "h x", "h@x", "h_x"] {
            assert_eq!(sanitized_hostname(bad), None, "{bad}");
        }
        // iTerm2's own check passes an empty string; only the combined check refuses it.
        assert_eq!(sanitized_hostname("").as_deref(), Some(""));
        assert!(hostname_ok("h.example"));
        assert!(hostname_ok("10.0.0.1"));
        assert!(!hostname_ok(""));
        assert!(!hostname_ok("::1"));
        assert!(!hostname_ok("[::1]"));
        assert!(!hostname_ok("-oProxyCommand=x"));
        assert!(!hostname_ok("h:22"), "a colon is outside the 14c grammar");
    }

    #[test]
    fn ssh_commands_follow_iterm2() {
        let srv = "-l bob -p 2222 -t h \"cd /srv; exec $SHELL -l\"";
        assert_eq!(on("ssh://bob@h:2222/srv"), Ok(Change { custom_command: "SSH", command: srv.into(), login_shell: None }));
        assert_eq!(command(on("ssh://h")), "h");
        assert_eq!(command(on("ssh://h/")), "h");
        assert_eq!(command(on("ssh://h/~/w")), "-t h \"cd \\~/w; exec $SHELL -l\"");
        // An unsafe path drops the cd part and keeps -t, as iTerm2; ruling R10 makes a space one.
        assert_eq!(command(on("ssh://h/a%20b")), "-t h");
        for bad in ["a%27b", "a%22b", "a%5Cb", "a%0Db", "a%0Ab", "a%00b", "%1b[31m", "a%7Fb", "a%E2%80%AEb", "a%C2%85b", "a%E2%80%A8b"] {
            assert_eq!(command(on(&format!("ssh://h/{bad}"))), "-t h", "{bad}");
        }
        let mut no_path = settings(true);
        no_path.support_path = false;
        assert_eq!(ssh_change(&url("ssh://h/srv"), &no_path).unwrap().command, "h");
        // The user is a separate word: no \+ (and the 14c grammar forbids + anyway).
        assert_eq!(on("ssh://a+b@h"), Err(UrlError::IllegalUsername("a+b".into())));
        assert_eq!(on("ssh://b%20o@h"), Err(UrlError::IllegalUsername("b o".into())));
        let mut space = settings(true);
        space.valid_user_chars = "_-+. ".into();
        assert_eq!(ssh_change(&url("ssh://b%20o@h"), &space), Err(UrlError::IllegalUsername("b o".into())));
        assert_eq!(on("ssh:h"), Err(UrlError::NoHost));
        // Integration off: a Custom command with ssh and the login shell flag.
        assert_eq!(
            off("ssh://bob@h:2222/srv"),
            Ok(Change { custom_command: "Yes", command: format!("ssh {srv}"), login_shell: Some(true) })
        );
        assert_eq!(command(off("ssh://h")), "ssh h");
        let mut path = settings(false);
        path.scheme_path = "C:\\Tools\\ssh.exe".into();
        let c = ssh_change(&url("ssh://bob@h:2222/srv"), &path).unwrap();
        assert_eq!((c.command.as_str(), c.login_shell), (format!("C:\\Tools\\ssh.exe {srv}").as_str(), None));
        path.scheme_path = "\"C:\\Program Files\\ssh.exe\" -F cfg".into();
        assert_eq!(ssh_change(&url("ssh://bob@h:2222/srv"), &path).unwrap().command, format!("\"C:\\Program Files\\ssh.exe\" -F cfg {srv}"));
        for unusable in ["", "--", "ssh"] {
            path.scheme_path = unusable.into();
            let c = ssh_change(&url("ssh://h"), &path).unwrap();
            assert_eq!((c.command.as_str(), c.login_shell), ("ssh h", Some(true)), "{unusable:?}");
        }
        assert!(scheme_path_usable("C:\\Tools\\ssh.exe"));
        assert!(!scheme_path_usable("ssh") && !scheme_path_usable("--") && !scheme_path_usable(""));
        assert_eq!(off("ssh://b%20o@h"), Err(UrlError::IllegalUsername("b o".into())));
        assert_eq!(off("ssh://-x"), Err(UrlError::BadHostname));
        // P13(d): no host opens nothing in both modes.
        assert_eq!(off("ssh:h"), Err(UrlError::NoHost));
    }

    /// P17: hostile URLs never reach a command line, and the words that do are one argument each.
    #[test]
    fn hostile_urls_never_become_options_or_extra_words() {
        // Host-less URLs open nothing in both modes: the cd word must never become ssh's host.
        for u in ["ssh:/etc;x", "ssh:///a", "ssh://u@/a", "ssh:h", "ssh:/a;b"] {
            for integration in [true, false] {
                assert_eq!(ssh_change(&url(u), &settings(integration)), Err(UrlError::NoHost), "{u} {integration}");
            }
        }
        for bad in ["ssh://%2Doproxy/", "ssh://-oProxyCommand=x/", "ssh://h%20x/", "ssh://h%2Fx/", "ssh://h%40x/", "ssh://[::1]/", "ssh://[fe80::1%25eth0]/", "ssh://h%00/"] {
            let parsed = parse(bad);
            let r = parsed.map(|u| ssh_change(&u, &settings(true)));
            assert!(matches!(r, None | Some(Err(UrlError::BadHostname))), "{bad}: {r:?}");
        }
        for bad in ["ssh://-x@h/", "ssh://%2Dx@h/", "ssh://a;b@h/", "ssh://$(x)@h/", "ssh://a%27b@h/", "ssh://a%40b@h/", "ssh://%00@h/", "ssh://a%0Ab@h/", "ssh://jos%C3%A9@h/"] {
            let r = ssh_change(&url(bad), &settings(true));
            assert!(matches!(r, Err(UrlError::IllegalUsername(_))), "{bad}: {r:?}");
        }
        // A path cannot add a word: a space drops the cd part (ruling R10).
        assert_eq!(command(on("ssh://h/a%20-oProxyCommand=x")), "-t h");
        // A trailing backslash or quote in the URL never reaches the line.
        assert_eq!(on_err("ssh://h\\"), UrlError::BadHostname);
        assert_eq!(command(on("ssh://h/a%5C")), "-t h");
        // Every generated word passes the 14c allowlist words.
        let c = command(on("ssh://bob@h.example:2222/x"));
        assert!(c.starts_with("-l bob -p 2222 -t h.example "));
    }

    /// CommandLineToArgvW's split of a command line (the MSVC runtime rules), as it2ssh.exe and
    /// ssh.exe read the line `ssh_change` writes.
    fn msvc_argv(line: &str) -> Vec<String> {
        let c: Vec<char> = line.chars().collect();
        let (mut out, mut i) = (Vec::new(), 0);
        loop {
            while i < c.len() && matches!(c[i], ' ' | '\t') {
                i += 1;
            }
            if i >= c.len() {
                return out;
            }
            let (mut word, mut quoted) = (String::new(), false);
            while i < c.len() {
                match c[i] {
                    '\\' => {
                        let n = c[i..].iter().take_while(|&&x| x == '\\').count();
                        i += n;
                        if c.get(i) == Some(&'"') {
                            word.extend(std::iter::repeat_n('\\', n / 2));
                            if n % 2 == 1 {
                                word.push('"');
                                i += 1;
                            }
                        } else {
                            word.extend(std::iter::repeat_n('\\', n));
                        }
                    }
                    '"' if quoted && c.get(i + 1) == Some(&'"') => {
                        word.push('"');
                        i += 2;
                    }
                    '"' => {
                        quoted = !quoted;
                        i += 1;
                    }
                    ' ' | '\t' if !quoted => break,
                    x => {
                        word.push(x);
                        i += 1;
                    }
                }
            }
            out.push(word);
        }
    }

    /// Integration on, end to end: the Command a URL gives, split as it2ssh.exe's argv, requoted
    /// into the it2ssh OSC (`util_it2ssh::requote`), parsed by the hook (`setup_for_hook` ->
    /// `ssh_args::parse`), and the command the conductor then writes (`exec_login_shell`), which
    /// conductor.sh runs with `$SHELL -c`.
    fn conductor_runs(u: &str) -> crate::conductor::Command {
        let argv = msvc_argv(&command(on(u)));
        let init = crate::conductor_parser::Init {
            token: String::new(),
            unique_id: "A".into(),
            bool_args: "46AaCfGgKkMNnqsTtVvXxYy".into(),
            sshargs: crate::util_it2ssh::requote(&argv),
            dcs_id: String::new(),
        };
        let setup = crate::conductor::setup_for_hook(&init, false, &[], &[], None, false, &crate::ssh_args::NoConfig);
        let mut c = crate::conductor::Conductor::new(setup, None, "U");
        c.exec_login_shell();
        c.in_flight().cloned().expect("the conductor writes a command")
    }

    /// Integration off, end to end: ssh.exe's argv for the Command a URL gives.
    fn ssh_runs(u: &str) -> Vec<String> {
        msvc_argv(&command(off(u)))
    }

    /// Ruling R10 (security, beyond iTerm2): with integration on the cd word is parsed by a shell
    /// twice, so escaping cannot make it safe. A path outside the allowlist sends no cd word in
    /// either mode: the session lands in the remote home and nothing from the URL runs.
    #[test]
    fn a_hostile_path_never_reaches_a_remote_shell() {
        use crate::conductor::Command;
        let hostile = [
            "ssh://h/x;id",
            "ssh://h/$(id)",
            "ssh://h/`id`",
            "ssh://h/a|b",
            "ssh://h/a&b",
            "ssh://h/a>b",
            "ssh://h/a%20b",
            "ssh://h/a'b",
            "ssh://h/a\"b",
            "ssh://h/a\\b",
            "ssh://h/x%3Bid",
            "ssh://h/%24(id)",
            "ssh://h/%60id%60",
            "ssh://h/x%0aid",
        ];
        for u in hostile {
            assert_eq!(conductor_runs(u), Command::ExecLoginShell(Vec::new()), "{u}: integration on runs no command");
            assert_eq!(ssh_runs(u), ["ssh", "-t", "h"], "{u}: integration off sends no remote command");
        }
        assert_eq!(conductor_runs("ssh://h/srv/app"), Command::Run("cd /srv/app; exec $SHELL -l".into()));
        assert_eq!(ssh_runs("ssh://h/srv/app"), ["ssh", "-t", "h", "cd /srv/app; exec $SHELL -l"]);
        // The home-relative path: the conductor's shell expands `~`; ssh's remote shell reads
        // iTerm2's `\~` as a literal folder name (that cd fails and the session stays home).
        assert_eq!(conductor_runs("ssh://h/~/w"), Command::Run("cd ~/w; exec $SHELL -l".into()));
        assert_eq!(ssh_runs("ssh://h/~/w"), ["ssh", "-t", "h", "cd \\~/w; exec $SHELL -l"]);
        assert_eq!(conductor_runs("ssh://h/a+b@c%25d=e,f.g_h~i-j"), Command::Run("cd /a+b@c%d=e,f.g_h~i-j; exec $SHELL -l".into()));
    }

    #[test]
    fn the_remote_folder_allowlist_follows_ruling_r10() {
        for ok in ["/srv/app", "~", "~/w", "/a+b@c%d=e,f.g_h~i-j", "/A/z/0/9"] {
            assert!(remote_folder_allowed(ok), "{ok}");
        }
        assert!(remote_folder_allowed(&format!("/{}", "a".repeat(1023))), "1024 bytes");
        assert!(!remote_folder_allowed(&format!("/{}", "a".repeat(1024))), "1025 bytes");
        for bad in ["-x", "~root", "~+", "~-", "", "/a\u{e9}", "/a\u{200b}"] {
            assert!(!remote_folder_allowed(bad), "{bad:?}");
        }
        for c in " '\"\\$`;|&<>()*?[]{}!#\n\r\t\0\u{7f}^:".chars() {
            assert!(!remote_folder_allowed(&format!("/a{c}b")), "{c:?}");
        }
    }

    fn profile(custom: &str, command: &str) -> Profile {
        let mut p = Profile::new("t");
        p.set("Custom Command", s(custom));
        p.set("Command", s(command));
        p
    }

    #[test]
    fn profile_for_url_follows_iterm2() {
        let on = settings(true);
        let changed = profile_for_url(&profile("No", ""), "ssh://h", &on).unwrap().expect("changed");
        assert_eq!((changed.string("Custom Command").as_str(), changed.string("Command").as_str()), ("SSH", "h"));
        for kind in ["SSH", "Custom Shell"] {
            assert!(profile_for_url(&profile(kind, "x"), "ssh://h", &on).unwrap().is_some(), "{kind}");
        }
        assert_eq!(profile_for_url(&profile("Yes", "plink -ssh box"), "ssh://h", &on), Ok(None));
        let dollars = profile_for_url(&profile("Yes", "$$"), "ssh://h", &on).unwrap().expect("changed");
        assert_eq!(dollars.string("Custom Command"), "SSH");
        assert_eq!(profile_for_url(&profile("No", ""), "http://h", &on), Ok(None));
        // Integration off sets the login shell flag, on never does.
        let off = profile_for_url(&profile("No", ""), "ssh://h", &settings(false)).unwrap().unwrap();
        assert_eq!((off.string("Custom Command").as_str(), off.string("Command").as_str(), off.bool("Run Command In Login Shell")), ("Yes", "ssh h", true));
        assert!(!changed.bool("Run Command In Login Shell"));
        // Errors reach the caller; an ssh URL that does not parse opens nothing (Decision).
        assert_eq!(profile_for_url(&profile("No", ""), "ssh://b%20o@h", &on), Err(UrlError::IllegalUsername("b o".into())));
        assert_eq!(profile_for_url(&profile("No", ""), "ssh://h:notaport", &on), Err(UrlError::NoHost));
        assert_eq!(profile_for_url(&profile("No", ""), "ssh:h", &settings(false)), Err(UrlError::NoHost));
        assert_eq!(profile_for_url(&profile("No", ""), "SSH://-x", &on), Err(UrlError::BadHostname));
        assert_eq!(
            illegal_username("b o"),
            "The SSH user name \u{201c}b o\u{201d} contained a disallowed character. The set of allowed characters is limited for security reasons. You can modify it in Settings > Advanced > Valid characters in SSH user names."
        );
        assert_eq!(ILLEGAL_USERNAME_HEADING, "Illegal Username");
    }

    #[test]
    fn settings_read_the_four_rows() {
        let dir = std::env::temp_dir().join(format!("tandem-ssh-url-{}", std::process::id()));
        let mut g = crate::globals::Globals::empty(&dir.join("g.toml"));
        assert_eq!(Settings::from_globals(&g), Settings { integration: false, scheme_path: "ssh".into(), valid_user_chars: "_-+.".into(), support_path: true, ftp_path: "ftp".into(), telnet_path: "telnet".into() });
        g.set("SshIntegrationForURLs", Value::Boolean(true));
        g.set("sshSchemePath", s("plink"));
        g.set("validCharactersInSSHUserNames", s("_"));
        g.set("sshURLsSupportPath", Value::Boolean(false));
        g.set("PathToFTP", s("ncftp"));
        g.set("PathToTelnet", s("putty"));
        assert_eq!(Settings::from_globals(&g), Settings { integration: true, scheme_path: "plink".into(), valid_user_chars: "_".into(), support_path: false, ftp_path: "ncftp".into(), telnet_path: "putty".into() });
    }
}

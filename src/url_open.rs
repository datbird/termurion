//! Opening telnet: and ftp: URLs (m13v1 Task 3; iTerm2 `iTermSessionLauncher.m:639-667`). 14e built
//! the ssh half and the one URL grammar: this adds only the two commands on top of it, through
//! the same parser (`ssh_url::parse_as`), the same validators (`ssh_url::{username_ok,
//! hostname_ok}`, which add 14c's `ssh_jumps` grammar to iTerm2's rule) and the same `Change`
//! that `ssh_url::profile_for_url` applies. A URL comes from outside, so it is hostile: the
//! only words taken from it are a checked user, host and port (telnet) or the URL itself,
//! percent-encoded as ONE word (ftp), after the program the person set. No `$$URL$$` text is
//! substituted anywhere (14e-D43), nothing here starts a program, and a URL with no host opens
//! nothing (iTerm2 would run a bare command).

use crate::ssh_jumps::port_is_safe;
use crate::ssh_url::{hostname_ok, parse_as, username_ok, Change, Settings, SshUrl, UrlError};

pub const DEFAULT_TELNET: &str = "telnet";
pub const DEFAULT_FTP: &str = "ftp";

/// The scheme of an external URL (`url_launch::EXTERNAL_SCHEMES`), lower case; None for any other.
pub fn scheme_of(url: &str) -> Option<&'static str> {
    let (scheme, _) = url.split_once(':')?;
    crate::url_launch::EXTERNAL_SCHEMES.iter().copied().find(|s| scheme.eq_ignore_ascii_case(s))
}

/// The program setting as typed (the person's own, 14e-D6), or `default` when it is empty.
fn program<'a>(path: &'a str, default: &'a str) -> &'a str {
    if path.trim().is_empty() { default } else { path.trim() }
}

fn word(arg: &str, line: &mut String) {
    if !line.is_empty() {
        line.push(' ');
    }
    crate::ssh_probe::quote_arg(arg, line);
}

/// `SL:646-667`: `<PathToTelnet> [-l user ]host[ port]`.
pub fn telnet_change(url: &SshUrl, s: &Settings) -> Result<Change, UrlError> {
    let Some(host) = &url.host else { return Err(UrlError::NoHost) };
    let mut args = String::new();
    if let Some(user) = &url.user {
        if !username_ok(user, &s.valid_user_chars) {
            return Err(UrlError::IllegalUsername(user.clone()));
        }
        word("-l", &mut args);
        word(user, &mut args);
    }
    if !hostname_ok(host) {
        return Err(UrlError::BadHostname);
    }
    word(host, &mut args);
    if let Some(port) = url.port {
        if !port_is_safe(&port.to_string()) {
            return Err(UrlError::BadHostname);
        }
        word(&port.to_string(), &mut args);
    }
    let command = format!("{} {args}", program(&s.telnet_path, DEFAULT_TELNET));
    Ok(Change { custom_command: "Yes", command, login_shell: None })
}

/// `SL:639-644`: `<PathToFTP> <url>`, the URL one percent-encoded word. The URL has the same user
/// and host checks as the others and no control or invisible character in its decoded path (a decoded space stays encoded).
pub fn ftp_change(text: &str, url: &SshUrl, s: &Settings) -> Result<Change, UrlError> {
    let Some(host) = &url.host else { return Err(UrlError::NoHost) };
    if let Some(user) = &url.user {
        if !username_ok(user, &s.valid_user_chars) {
            return Err(UrlError::IllegalUsername(user.clone()));
        }
    }
    if !hostname_ok(host) {
        return Err(UrlError::BadHostname);
    }
    if url.path.chars().any(|c| c != ' ' && crate::ssh_url::is_hostile_char(c)) || still_encoded(&url.path) {
        return Err(UrlError::NoHost);
    }
    let mut line = String::new();
    word(&crate::semantic::encode_url_chars(&without_password(text)), &mut line);
    Ok(Change { custom_command: "Yes", command: format!("{} {line}", program(&s.ftp_path, DEFAULT_FTP)), login_shell: None })
}

/// A path that still holds a percent escape after its one decode (`%250A` became `%0A`): a URL-aware
/// ftp client decodes it again, so what was checked is not what runs.
fn still_encoded(path: &str) -> bool {
    path.as_bytes().windows(3).any(|w| w[0] == b'%' && w[1].is_ascii_hexdigit() && w[2].is_ascii_hexdigit())
}

/// The URL without the password of its userinfo (`scheme://user:pw@host` becomes
/// `scheme://user@host`): the password is never stored or typed into a command line, the ftp
/// program prompts for it. The userinfo ends at the last `@` of the authority.
fn without_password(text: &str) -> String {
    let Some((scheme, rest)) = text.split_once("://") else { return text.to_string() };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    match authority.rfind('@') {
        Some(at) => {
            let (userinfo, host) = (&authority[..at], &authority[at + 1..]);
            let user = userinfo.split(':').next().unwrap_or("");
            let user = if user.is_empty() { String::new() } else { format!("{user}@") };
            format!("{scheme}://{user}{host}{tail}")
        }
        None => text.to_string(),
    }
}

/// The change an external URL makes: Ok(None) for a scheme that changes nothing (iTerm2's
/// `x-man-page`, which Termurion does not offer, and every other scheme), and for ssh the 14e rule.
pub fn change_for(url: &str, s: &Settings) -> Result<Option<Change>, UrlError> {
    let Some(scheme) = scheme_of(url) else { return Ok(None) };
    if !crate::url_launch::url_ok(url) {
        return Err(UrlError::NoHost);
    }
    let parsed = parse_as(url, scheme).ok_or(UrlError::NoHost)?;
    Ok(Some(match scheme {
        "ssh" => crate::ssh_url::ssh_change(&parsed, s)?,
        "telnet" => telnet_change(&parsed, s)?,
        _ => ftp_change(url, &parsed, s)?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh_url::{Settings, UrlError};

    fn settings() -> Settings {
        Settings {
            integration: false,
            scheme_path: "ssh".into(),
            valid_user_chars: "_-+.".into(),
            support_path: true,
            ftp_path: "ftp".into(),
            telnet_path: "telnet".into(),
        }
    }

    fn command(url: &str) -> Result<String, UrlError> {
        change_for(url, &settings()).map(|c| c.expect("a telnet or ftp url").command)
    }

    /// Final review item 15: the path is checked after one decode, so a path whose decoded form still
    /// holds an escape (`%250A` decodes to `%0A`) is refused: a URL-aware ftp client would decode it to LF.
    #[test]
    fn a_double_encoded_ftp_path_is_refused() {
        for bad in ["ftp://h/%250A", "ftp://h/a%250d%250ab", "ftp://h/%2500"] {
            assert_eq!(command(bad), Err(UrlError::NoHost), "{bad}");
        }
        assert_eq!(command("ftp://h/100%25").as_deref(), Ok("ftp ftp://h/100%25"), "a lone percent stays");
        assert_eq!(command("ftp://h/a%20b").as_deref(), Ok("ftp ftp://h/a%20b"));
    }

    #[test]
    fn telnet_commands_follow_iterm2() {
        assert_eq!(command("telnet://h").as_deref(), Ok("telnet h"));
        assert_eq!(command("telnet://u@h:23").as_deref(), Ok("telnet -l u h 23"));
        assert_eq!(command("TELNET://h:2323").as_deref(), Ok("telnet h 2323"));
        assert_eq!(command("telnet://bob:pw@h").as_deref(), Ok("telnet -l bob h"), "the password is never a word");
        let c = change_for("telnet://h", &settings()).unwrap().unwrap();
        assert_eq!((c.custom_command, c.login_shell), ("Yes", None));
        let mut s = settings();
        s.telnet_path = "C:\\Tools\\telnet.exe".into();
        assert_eq!(change_for("telnet://u@h", &s).unwrap().unwrap().command, "C:\\Tools\\telnet.exe -l u h");
        s.telnet_path = "  ".into();
        assert_eq!(change_for("telnet://h", &s).unwrap().unwrap().command, "telnet h", "an empty path is telnet");
        // iTerm2 would open a bare telnet; a link from outside never does (P13(d)).
        assert_eq!(command("telnet:h"), Err(UrlError::NoHost));
        assert_eq!(command("telnet://u@/x"), Err(UrlError::NoHost));
        assert_eq!(command("telnet://-h"), Err(UrlError::BadHostname));
        assert_eq!(command("telnet://a%20b@h"), Err(UrlError::IllegalUsername("a b".into())));
        assert_eq!(command("telnet://a;b@h"), Err(UrlError::IllegalUsername("a;b".into())));
        assert_eq!(command("telnet://-x@h"), Err(UrlError::IllegalUsername("-x".into())));
        assert_eq!(command("telnet://u@h:0"), Err(UrlError::NoHost), "port 0 does not parse");
        assert_eq!(command("telnet://h:99999"), Err(UrlError::NoHost));
    }

    #[test]
    fn ftp_commands_follow_iterm2() {
        assert_eq!(command("ftp://h/x").as_deref(), Ok("ftp ftp://h/x"));
        assert_eq!(command("ftp://h/pub/f.txt").as_deref(), Ok("ftp ftp://h/pub/f.txt"));
        assert_eq!(command("FTP://u:p@h:2121/a%20b").as_deref(), Ok("ftp FTP://u@h:2121/a%20b"), "the password is dropped");
        let c = change_for("ftp://h", &settings()).unwrap().unwrap();
        assert_eq!((c.custom_command, c.login_shell), ("Yes", None));
        let mut s = settings();
        s.ftp_path = "C:\\Tools\\ncftp.exe".into();
        assert_eq!(change_for("ftp://h/x", &s).unwrap().unwrap().command, "C:\\Tools\\ncftp.exe ftp://h/x");
        assert_eq!(command("ftp:x"), Err(UrlError::NoHost));
        assert_eq!(command("ftp://-h/x"), Err(UrlError::BadHostname));
        assert_eq!(command("ftp://a;b@h/x"), Err(UrlError::IllegalUsername("a;b".into())));
    }

    /// Ruling (task 3 fix 1): a password in an ftp or telnet URL is never stored. The session copy's
    /// Command (which becomes the saved Bookmark, an arrangement summary and the process command
    /// line) carries the URL without it; the ftp program prompts.
    #[test]
    fn an_ftp_password_is_never_stored() {
        for url in ["ftp://bob:secret@h/x", "ftp://bob:secret@h:21/a%20b?q=1", "ftp://:secret@h/x", "ftp://bob:se%3Acret@h/x", "ftp://bob:s@cret@h/x", "ftp://bob:secret@h"] {
            let c = command(url).unwrap();
            assert!(!c.contains("secret") && !c.contains("se%3Acret") && !c.contains("cret"), "{url} -> {c}");
        }
        assert_eq!(command("ftp://bob:secret@h/x").as_deref(), Ok("ftp ftp://bob@h/x"));
        assert_eq!(command("ftp://:secret@h/x").as_deref(), Ok("ftp ftp://h/x"));
        assert_eq!(command("ftp://bob:s@cret@h/x").as_deref(), Ok("ftp ftp://bob@h/x"), "the last @ ends the userinfo");
        assert_eq!(command("ftp://bob@h/x").as_deref(), Ok("ftp ftp://bob@h/x"), "a user alone stays");
        assert_eq!(command("ftp://h/a@b:c").as_deref(), Ok("ftp ftp://h/a@b:c"), "an @ in the path is not userinfo");
        // No saved or serialized form of the session copy holds it either.
        let profile = crate::store::Store::fresh(&std::env::temp_dir().join(format!("termurion-ftp-pw-{}.toml", std::process::id()))).profiles[0].clone();
        for url in ["ftp://bob:secret@h/x", "telnet://bob:secret@h:23"] {
            let changed = crate::ssh_url::profile_for_url(&profile, url, &settings()).unwrap().unwrap();
            assert!(!format!("{changed:?}").contains("secret"), "{url}");
            assert!(!changed.string("Command").contains("secret"), "{url}");
        }
    }

    /// The 14e-P17 list plus the ftp and telnet cases: nothing hostile reaches a command line as
    /// an option or an extra word.
    #[test]
    fn hostile_urls_open_nothing_for_ftp_and_telnet() {
        for bad in [
            "ftp://h/a b",
            "ftp://h/%00",
            "ftp://h/a%0Ab",
            "ftp://h/\u{202e}x",
            "ftp://h/a\"b",
            "ftp://h\" -x",
            "ftp://%2Dx/",
            "ftp://h%20x/",
            "ftp://[::1]/",
            "ftp://h/%ff",
            "telnet://-h",
            "telnet://h x",
            "telnet://h\n",
            "telnet://h%00",
            "telnet://%2Doproxy/",
            "telnet://u@h:0",
            "telnet://[::1]:23",
            "telnet://h:23 -x",
        ] {
            let r = change_for(bad, &settings());
            assert!(matches!(r, Err(_)), "{bad}: {r:?}");
        }
        for u in ["ftp://h/a%20b", "ftp://h//attacker.example/share", "ftp://h/%5C%5Cattacker%5Cshare", "ftp://u:p@h/x"] {
            let c = change_for(u, &settings()).unwrap().unwrap().command;
            assert_eq!(c.matches(' ').count(), 1, "{u}: program and ONE url word, got {c}");
            assert!(!c.contains('\\') && !c.contains('"'), "{u}: {c}");
        }
    }

    #[test]
    fn other_schemes_change_nothing() {
        for u in ["x-man-page://1/ls", "https://e.com", "mailto:a@b", "gopher://h"] {
            assert_eq!(change_for(u, &settings()), Ok(None), "{u}");
        }
    }

    #[test]
    fn external_schemes_are_one_list() {
        assert_eq!(crate::url_launch::EXTERNAL_SCHEMES, ["ssh", "telnet", "ftp"]);
        for good in ["ssh://h", "SSH://h", "telnet://h", "Telnet://u@h:23", "ftp://h/x", "FTP://h"] {
            assert!(crate::url_launch::url_ok(good), "{good}");
        }
        for bad in ["http://h", "x-man-page://1/ls", "file:///c:/x", "gopher://h", "telnets://h", "ftps://h", "ftp", "ftp://h/a b", "telnet://h\"x"] {
            assert!(!crate::url_launch::url_ok(bad), "{bad}");
        }
        let argv = |a: &[&str]| a.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
        assert_eq!(crate::url_launch::url_argument(&argv(&["termurion.exe", "--url", "telnet://h"])), crate::url_launch::UrlArg::Url("telnet://h".into()));
        assert_eq!(crate::url_launch::url_argument(&argv(&["termurion.exe", "--url", "ftp://h/x"])), crate::url_launch::UrlArg::Url("ftp://h/x".into()));
        assert!(matches!(crate::url_launch::url_argument(&argv(&["termurion.exe", "--url", "telnet://h", "-x"])), crate::url_launch::UrlArg::Refused(_)));
        assert!(matches!(crate::url_launch::url_argument(&argv(&["termurion.exe", "ftp://h"])), crate::url_launch::UrlArg::Refused(_)), "a bare url argument is still refused");
        assert_eq!(crate::url_launch::url_argument(&argv(&["termurion.exe", "C:\\x.ps1"])), crate::url_launch::UrlArg::None);
        // The pipe is the same rule.
        assert!(crate::url_launch::pipe_message("telnet://h").is_some() && crate::url_launch::pipe_message("ftp://h/x").is_some());
        assert!(crate::url_launch::pipe_message("http://h").is_none());
        let msg = crate::url_launch::pipe_message("ftp://h/x").unwrap();
        assert_eq!(crate::url_launch::parse_message(&msg).as_deref(), Some("ftp://h/x"));
        let cwd = std::path::Path::new("C:\\");
        assert_eq!(crate::launch::parse(&["--url".to_string(), "telnet://h".to_string()], cwd), Ok(crate::launch::Request::Url { url: "telnet://h".into() }));
    }

    #[test]
    fn banners_name_the_target_not_the_password() {
        let b = crate::url_launch::open_banner("ftp://bob:secret@example.com:2121/x").unwrap();
        assert_eq!(b, "Opened an ftp link to bob@example.com:2121.");
        assert!(!b.contains("secret"));
        assert_eq!(crate::url_launch::open_banner("telnet://h:23").as_deref(), Some("Opened a telnet link to h:23."));
        assert_eq!(crate::url_launch::open_banner("ssh://h").as_deref(), Some("Opened an ssh link to h."));
        assert_eq!(crate::url_launch::ignored_banner(1), "1 link ignored");
        assert_eq!(crate::url_launch::ignored_banner(6), "6 links ignored");
    }

    /// P3: nothing here starts a program; the command is text for a session the person's profile
    /// starts after the same checks as ssh.
    #[test]
    fn no_url_runs_a_command_here() {
        let src = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("url_open.rs")).unwrap();
        let code = &src[..src.find("#[cfg(test)]").unwrap()];
        for needle in ["Command::new", "ShellExecute", "CreateProcess", "cmd.exe", "std::process"] {
            assert!(!code.contains(needle), "{needle}");
        }
    }
}

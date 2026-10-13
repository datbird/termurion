//! iTerm2's shell history (iTermShellHistoryController): the commands and directories shell
//! integration reports, kept per host, ranked for the Command History and Recent
//! Directories popups, and pruned by age.

use std::collections::BTreeMap;

/// One run of a command.
#[derive(Clone, Debug, PartialEq)]
pub struct Use {
    pub time: f64,
    pub dir: Option<String>,
    pub code: Option<i32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    pub command: String,
    pub uses: Vec<Use>,
}

impl Command {
    pub fn last_use(&self) -> f64 {
        self.uses.iter().map(|u| u.time).fold(0.0, f64::max)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Dir {
    pub path: String,
    pub uses: u64,
    pub last: f64,
    pub starred: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Host {
    pub commands: Vec<Command>,
    pub dirs: Vec<Dir>,
    /// The directory recorded last (a repeat is not a new use).
    last_dir: Option<String>,
}

/// CommandHistoryUsePower and CommandHistoryAgePower.
#[derive(Clone, Copy, Debug)]
pub struct Powers {
    pub uses: f64,
    pub age: f64,
}

pub const POWERS: Powers = Powers { uses: 3.0, age: 1.0 };

/// Most entries a lookup returns (iTerm2's).
pub const LIMIT: usize = 200;
const DAYS_90: f64 = 90.0 * 86_400.0;
const MAX_DIRS: usize = 1000;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Store {
    pub hosts: BTreeMap<String, Host>,
}

/// iTerm2's host record key.
pub fn host_key(user: Option<&str>, host: Option<&str>) -> String {
    format!("{}@{}", user.unwrap_or(""), host.unwrap_or(""))
}

/// iTerm2's command frecency.
pub fn frecency(uses: usize, age: f64, p: Powers) -> f64 {
    (10.0 + uses as f64).log10().powf(p.uses) / (10.0 + age.max(0.0)).log10().powf(p.age)
}

/// What Command History inserts: the command after the typed prefix's length.
pub fn remainder(command: &str, typed: &str) -> String {
    command.chars().skip(typed.chars().count()).collect()
}

/// A recent directory as the shell at the prompt takes it: a local Windows path
/// (`/C:/x` from OSC 7) as `C:\x`; PowerShell and cmd quote it when it needs quoting; a
/// POSIX shell gets iTerm2's backslash escaping, newlines too. `shell`: the shell's name
/// from shell integration.
pub fn insertable_dir(path: &str, shell: Option<&str>) -> String {
    let shell = shell.unwrap_or("").to_ascii_lowercase();
    let windows = matches!(shell.as_str(), "pwsh" | "powershell" | "cmd");
    let local = crate::profile::local_path(path).map(|p| p.to_string_lossy().into_owned());
    match (local, windows) {
        (Some(p), _) if shell == "cmd" => {
            if p.contains([' ', '&', '(', ')', '^', '|', '<', '>', '%', '!']) { format!("\"{p}\"") } else { p }
        }
        (Some(p), _) => {
            if p.contains([' ', '\'', '"', '`', '$', ';', '&', '(', ')', '{', '}', '@', '#', ',', '|', '<', '>']) { format!("'{}'", p.replace('\'', "''")) } else { p }
        }
        (None, _) => {
            let job = crate::paste::Job::new(path, crate::paste::ESCAPE);
            crate::paste::sanitize(&job, &crate::paste::Prefs::default()).replace('\r', "\\\r").replace('\n', "\\\n")
        }
    }
}

/// Writes `text` to `path` through a temporary file, so a crash never leaves half a file.
pub fn write_whole(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

impl Store {
    pub fn record_command(&mut self, host: &str, command: &str, dir: Option<&str>, time: f64) {
        let command = command.trim();
        if command.is_empty() {
            return;
        }
        let h = self.hosts.entry(host.to_string()).or_default();
        let u = Use { time, dir: dir.map(str::to_string), code: None };
        match h.commands.iter_mut().find(|c| c.command == command) {
            Some(c) => c.uses.push(u),
            None => h.commands.push(Command { command: command.to_string(), uses: vec![u] }),
        }
    }

    /// The exit code of the command's newest use.
    pub fn record_code(&mut self, host: &str, command: &str, code: i32) {
        let command = command.trim();
        let newest = self.hosts.get_mut(host).and_then(|h| h.commands.iter_mut().find(|c| c.command == command)).and_then(|c| c.uses.iter_mut().max_by(|a, b| a.time.total_cmp(&b.time)));
        if let Some(u) = newest {
            u.code = Some(code);
        }
    }

    pub fn record_dir(&mut self, host: &str, path: &str, time: f64) {
        let h = self.hosts.entry(host.to_string()).or_default();
        if path.is_empty() || h.last_dir.as_deref() == Some(path) {
            return;
        }
        h.last_dir = Some(path.to_string());
        match h.dirs.iter_mut().find(|d| d.path == path) {
            Some(d) => {
                d.uses += 1;
                d.last = time;
            }
            None => h.dirs.push(Dir { path: path.to_string(), uses: 1, last: time, starred: false }),
        }
    }

    /// The host's commands starting with `prefix` (ignoring case), each with its last use:
    /// newest first for an empty prefix, else by frecency; at most 200.
    pub fn commands(&self, host: &str, prefix: &str, now: f64, p: Powers) -> Vec<(String, f64)> {
        let Some(h) = self.hosts.get(host) else { return Vec::new() };
        let prefix = prefix.to_lowercase();
        let mut found: Vec<(&Command, f64)> = h.commands.iter().filter(|c| c.command.to_lowercase().starts_with(&prefix)).map(|c| (c, c.last_use())).collect();
        if prefix.is_empty() {
            found.sort_by(|a, b| b.1.total_cmp(&a.1));
        } else {
            let score = |c: &Command| frecency(c.uses.len(), now - c.last_use(), p);
            found.sort_by(|a, b| score(b.0).total_cmp(&score(a.0)));
        }
        found.into_iter().take(LIMIT).map(|(c, t)| (c.command.clone(), t)).collect()
    }

    /// Every command of the host: (command, uses, last use), for Autocomplete.
    pub fn command_stats(&self, host: &str) -> Vec<(String, usize, f64)> {
        self.hosts.get(host).map(|h| h.commands.iter().map(|c| (c.command.clone(), c.uses.len(), c.last_use())).collect()).unwrap_or_default()
    }

    /// Every use of every command of the host, oldest first (`iTermShellHistoryController.m
    /// :798-802`; Command History shows these for the session's host).
    pub fn uses(&self, host: &str) -> Vec<(String, Use)> {
        let Some(h) = self.hosts.get(host) else { return Vec::new() };
        let mut all: Vec<(String, Use)> = Vec::new();
        for c in &h.commands {
            for u in &c.uses {
                all.push((c.command.clone(), u.clone()));
            }
        }
        all.sort_by(|a, b| a.1.time.total_cmp(&b.1.time));
        all
    }

    /// Every directory path across every host, for Recent Directories' abbreviation tree, which
    /// spans every host's directories (a correction to the research,
    /// `iTermShellHistoryController.m:765-774`).
    pub fn all_dir_paths(&self) -> Vec<String> {
        self.hosts.values().flat_map(|h| h.dirs.iter().map(|d| d.path.clone())).collect()
    }

    /// The host's directories in iTerm2's order.
    pub fn dirs(&self, host: &str) -> Vec<Dir> {
        let Some(h) = self.hosts.get(host) else { return Vec::new() };
        let mut dirs = h.dirs.clone();
        let tier = |d: &Dir| (d.uses.max(1) as f64).log2().floor() as i64;
        dirs.sort_by(|a, b| b.starred.cmp(&a.starred).then(tier(b).cmp(&tier(a))).then(b.last.total_cmp(&a.last)).then(b.uses.cmp(&a.uses)));
        dirs
    }

    pub fn toggle_star(&mut self, host: &str, path: &str) {
        if let Some(d) = self.hosts.get_mut(host).and_then(|h| h.dirs.iter_mut().find(|d| d.path == path)) {
            d.starred = !d.starred;
        }
    }

    /// iTerm2's removeOldData at start.
    pub fn prune(&mut self, now: f64) {
        let cutoff = now - DAYS_90;
        for h in self.hosts.values_mut() {
            for c in &mut h.commands {
                c.uses.retain(|u| u.time >= cutoff);
            }
            h.commands.retain(|c| !c.uses.is_empty());
            h.dirs.retain(|d| d.starred || d.last >= cutoff);
            let mut unstarred: Vec<f64> = h.dirs.iter().filter(|d| !d.starred).map(|d| d.last).collect();
            if unstarred.len() > MAX_DIRS {
                unstarred.sort_by(|a, b| b.total_cmp(a));
                let oldest_kept = unstarred[MAX_DIRS - 1];
                let mut extra = unstarred.len() - MAX_DIRS;
                // The oldest unstarred go, one each, until 1000 are left.
                h.dirs.sort_by(|a, b| a.last.total_cmp(&b.last));
                h.dirs.retain(|d| {
                    if extra > 0 && !d.starred && d.last <= oldest_kept {
                        extra -= 1;
                        return false;
                    }
                    true
                });
            }
        }
    }

    pub fn erase_commands(&mut self) {
        for h in self.hosts.values_mut() {
            h.commands.clear();
        }
    }

    pub fn erase_dirs(&mut self) {
        for h in self.hosts.values_mut() {
            h.dirs.clear();
            h.last_dir = None;
        }
    }

    /// Shell integration has fed the history at least once.
    pub fn ever_used(&self) -> bool {
        self.hosts.values().any(|h| !h.commands.is_empty() || !h.dirs.is_empty())
    }

    pub fn to_json(&self) -> String {
        use serde_json::json;
        let hosts: serde_json::Map<String, serde_json::Value> = self
            .hosts
            .iter()
            .map(|(k, h)| {
                let commands: Vec<_> = h.commands.iter().map(|c| json!({ "command": c.command, "uses": c.uses.iter().map(|u| json!({ "time": u.time, "dir": u.dir, "code": u.code })).collect::<Vec<_>>() })).collect();
                let dirs: Vec<_> = h.dirs.iter().map(|d| json!({ "path": d.path, "uses": d.uses, "last": d.last, "starred": d.starred })).collect();
                (k.clone(), json!({ "commands": commands, "dirs": dirs, "last_dir": h.last_dir }))
            })
            .collect();
        json!({ "hosts": hosts }).to_string()
    }

    pub fn from_json(text: &str) -> Store {
        let v: serde_json::Value = serde_json::from_str(text).unwrap_or_default();
        let mut store = Store::default();
        let Some(hosts) = v.get("hosts").and_then(|h| h.as_object()) else { return store };
        for (k, h) in hosts {
            let arr = |key: &str| h.get(key).and_then(|a| a.as_array()).cloned().unwrap_or_default();
            let commands = arr("commands")
                .iter()
                .filter_map(|c| {
                    let uses = c.get("uses")?.as_array()?.iter().filter_map(|u| Some(Use { time: u.get("time")?.as_f64()?, dir: u.get("dir").and_then(|d| d.as_str()).map(str::to_string), code: u.get("code").and_then(|c| c.as_i64()).map(|c| c as i32) })).collect();
                    Some(Command { command: c.get("command")?.as_str()?.to_string(), uses })
                })
                .collect();
            let dirs = arr("dirs")
                .iter()
                .filter_map(|d| Some(Dir { path: d.get("path")?.as_str()?.to_string(), uses: d.get("uses")?.as_u64()?, last: d.get("last")?.as_f64()?, starred: d.get("starred").and_then(|s| s.as_bool()).unwrap_or(false) }))
                .collect();
            let last_dir = h.get("last_dir").and_then(|d| d.as_str()).map(str::to_string);
            store.hosts.insert(k.clone(), Host { commands, dirs, last_dir });
        }
        store
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: f64 = 86_400.0;

    #[test]
    fn commands_are_kept_like_iterm2() {
        assert_eq!(host_key(Some("me"), Some("box")), "me@box");
        assert_eq!(host_key(None, Some("box")), "@box");
        let mut s = Store::default();
        s.record_command("h", "ls", Some("/tmp"), 1.0);
        s.record_command("h", " ls ", Some("/"), 2.0);
        s.record_command("h", "  ", None, 3.0);
        assert_eq!(s.hosts["h"].commands.len(), 1, "one entry per command");
        assert_eq!(s.hosts["h"].commands[0].uses.len(), 2);
        s.record_code("h", "ls", 1);
        assert_eq!(s.hosts["h"].commands[0].uses[1].code, Some(1), "the newest use");
        let now = 1000.0 * DAY;
        for i in 0..100 {
            s.record_command("h", "git commit", None, now - DAY - i as f64);
        }
        s.record_command("h", "git checkout", None, now - 10.0);
        let by_score = s.commands("h", "GIT C", now, POWERS);
        assert_eq!(by_score.iter().map(|(c, _)| c.as_str()).collect::<Vec<_>>(), vec!["git commit", "git checkout"], "100 uses a day ago beat one use now");
        assert!(frecency(100, DAY, POWERS) > frecency(1, 10.0, POWERS));
        assert!(frecency(20, 1000.0 * DAY, POWERS) < frecency(1, 10.0, POWERS), "age counts");
        let by_time = s.commands("h", "", now, POWERS);
        assert_eq!(by_time[0].0, "git checkout", "no prefix: newest first");
        assert_eq!(by_time[1].0, "git commit");
        assert!(s.commands("other", "", now, POWERS).is_empty());
        for i in 0..300 {
            s.record_command("h", &format!("c{i}"), None, 5.0);
        }
        assert_eq!(s.commands("h", "", now, POWERS).len(), LIMIT);
    }

    #[test]
    fn directories_are_kept_like_iterm2() {
        let mut s = Store::default();
        s.record_dir("h", "/a", 1.0);
        s.record_dir("h", "/a", 2.0);
        assert_eq!(s.hosts["h"].dirs[0].uses, 1, "a repeat is not a new use");
        s.record_dir("h", "/b", 3.0);
        s.record_dir("h", "/a", 4.0);
        s.record_dir("h", "/b", 5.0);
        s.record_dir("h", "/c", 6.0);
        let order: Vec<String> = s.dirs("h").into_iter().map(|d| d.path).collect();
        assert_eq!(order, vec!["/b", "/a", "/c"], "log2 of uses (1 for both /a and /b), then newer");
        s.toggle_star("h", "/c");
        assert_eq!(s.dirs("h")[0].path, "/c", "starred first");
    }

    #[test]
    fn old_history_is_pruned() {
        let mut s = Store::default();
        let now = 200.0 * DAY;
        s.record_command("h", "old", None, now - 91.0 * DAY);
        s.record_command("h", "mixed", None, now - 91.0 * DAY);
        s.record_command("h", "mixed", None, now - DAY);
        s.record_dir("h", "/old", now - 91.0 * DAY);
        s.record_dir("h", "/starred", now - 91.0 * DAY);
        s.toggle_star("h", "/starred");
        for i in 0..1005 {
            s.record_dir("h", &format!("/d{i}"), now - 10.0 * DAY + i as f64);
        }
        s.prune(now);
        let h = &s.hosts["h"];
        assert_eq!(h.commands.iter().map(|c| (c.command.as_str(), c.uses.len())).collect::<Vec<_>>(), vec![("mixed", 1)]);
        assert!(!h.dirs.iter().any(|d| d.path == "/old"));
        assert!(h.dirs.iter().any(|d| d.path == "/starred"), "starred directories stay");
        assert_eq!(h.dirs.iter().filter(|d| !d.starred).count(), 1000);
        assert!(!h.dirs.iter().any(|d| d.path == "/d0"), "the oldest unstarred go");
    }

    #[test]
    fn uses_come_oldest_first_across_commands() {
        let mut s = Store::default();
        s.record_command("h", "ls", None, 5.0);
        s.record_command("h", "git status", None, 1.0);
        s.record_command("h", "ls", None, 3.0);
        s.record_code("h", "git status", 1);
        let uses = s.uses("h");
        assert_eq!(uses.iter().map(|(c, u)| (c.as_str(), u.time)).collect::<Vec<_>>(), vec![("git status", 1.0), ("ls", 3.0), ("ls", 5.0)]);
        assert_eq!(uses[0].1.code, Some(1));
        assert!(s.uses("other").is_empty());

        let mut d = Store::default();
        d.record_dir("a@x", "/one", 1.0);
        d.record_dir("b@y", "/two", 2.0);
        let mut paths = d.all_dir_paths();
        paths.sort();
        assert_eq!(paths, vec!["/one".to_string(), "/two".to_string()]);
    }

    #[test]
    fn remainders_follow_the_typed_prefix() {
        assert_eq!(remainder("git status", "gi"), "t status");
        assert_eq!(remainder("git", "GIT"), "");
        assert_eq!(remainder("\u{e9}cho x", "\u{e9}"), "cho x");
    }

    #[test]
    fn directories_insert_for_their_shell() {
        assert_eq!(insertable_dir("/C:/Users/rob", Some("pwsh")), "C:\\Users\\rob");
        assert_eq!(insertable_dir("/C:/Users/rob/My Docs", Some("pwsh")), "'C:\\Users\\rob\\My Docs'");
        assert_eq!(insertable_dir("/C:/it's here", Some("powershell")), "'C:\\it''s here'");
        assert_eq!(insertable_dir("/C:/a b", Some("cmd")), "\"C:\\a b\"");
        assert_eq!(insertable_dir("/home/me/My Docs", Some("bash")), "/home/me/My\\ Docs");
        assert_eq!(insertable_dir("/tmp/a\nb", None), "/tmp/a\\\nb", "POSIX: newlines escaped too");
        let dir = std::env::temp_dir().join(format!("termurion-write-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("h.json");
        write_whole(&f, "one").unwrap();
        write_whole(&f, "two").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "two");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temporary file left");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn history_round_trips() {
        let mut s = Store::default();
        s.record_command("me@box", "ls", Some("/tmp"), 1.5);
        s.record_code("me@box", "ls", 2);
        s.record_dir("me@box", "/tmp", 3.0);
        s.toggle_star("me@box", "/tmp");
        let back = Store::from_json(&s.to_json());
        assert_eq!(back.hosts["me@box"].commands, s.hosts["me@box"].commands);
        assert_eq!(back.hosts["me@box"].dirs, s.hosts["me@box"].dirs);
        assert!(Store::from_json("junk").hosts.is_empty());
        assert!(back.ever_used() && !Store::default().ever_used());
    }
}

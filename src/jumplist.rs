//! The Jump List as iTerm2's dock menu, and the recent profiles behind its "Recents" (m13v1 Task 6).
//! Pure: the model of what the list holds, the recents store, the `.itermtab` file text and the
//! renamed-profile decision. The COM writer is `jumplist_win`; the app side is `app::jumplist_ui`.

use serde::{Deserialize, Serialize};

pub const NEW_WINDOW_DEFAULT: &str = "New Window (Default Profile)";
pub const NEW_WINDOW: &str = "New Window\u{2026}";
pub const NEW_TAB: &str = "New Tab\u{2026}";
pub const RESTORE_ARRANGEMENT: &str = "Restore Arrangement";
pub const RECENTS: &str = "Recents";
/// `ProfileDocument.swift:84-109`: the profile-renamed question.
pub const RENAMED_HEADING: &str = "Profile Renamed";
pub const RENAMED_ID: &str = "NoSyncOpenRenamedProfile";

/// A link's title is at most this many characters (B10).
pub const MAX_TITLE: usize = 120;
/// Recents kept (P17): the older ones are dropped, their files deleted.
pub const MAX_RECENTS: usize = 20;
/// Links built per category: Windows shows only as many as the person's slot count allows
/// (10 by default), so more are never read.
pub const MAX_LINKS: usize = 200;
/// The recents list file and a `.itermtab` file are read at most this long.
pub const MAX_LIST_BYTES: usize = 64 * 1024;
pub const MAX_TAB_FILE: usize = 4 * 1024;

/// One Jump List item: the title shown and the arguments Tandem's own exe is started with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub title: String,
    pub args: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Category {
    pub name: String,
    pub links: Vec<Link>,
}

/// What the Jump List holds: Tasks, then the custom categories in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Model {
    pub tasks: Vec<Link>,
    pub categories: Vec<Category>,
}

/// One recent profile: the profile's guid and name when it started a session, and the
/// `<uuid>.itermtab` file that backs its Jump List entry (a bare name in the stored list, the full
/// path in the model).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recent {
    pub guid: String,
    pub name: String,
    pub file: String,
}

/// A profile guid a link may name: the 36 characters of a UUID (B10); any other guid is left out.
fn guid_ok(g: &str) -> bool {
    g.len() == 36 && g.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// A title or name a link may show: clean text (never edited, left out otherwise), capped.
fn shown(text: &str) -> Option<String> {
    let clean = crate::pathnav::safe_text(text)?;
    (!clean.is_empty()).then(|| clean.chars().take(MAX_TITLE).collect())
}

fn link(title: &str, args: &[&str]) -> Link {
    Link { title: title.to_string(), args: args.iter().map(|a| a.to_string()).collect() }
}

/// The dock menu as a Jump List (`AD:1154-1180`, `AD:2649-2690`): Tasks hold "New Window (Default
/// Profile)"; the categories are New Window and New Tab (every profile, menu order), Restore
/// Arrangement (every arrangement) and, with the key on and any recents, Recents first. An entry
/// whose guid or name could add an argument is left out, never edited (B10). `profiles` are
/// (guid, name). The default profile needs no argument (`--new-window` alone opens it), so
/// `default_guid` only keeps the signature iTerm2's menu has.
pub fn model(profiles: &[(String, String)], _default_guid: &str, arrangements: &[String], recents: &[Recent], save_recents: bool) -> Model {
    let shown_profiles: Vec<(&str, String)> = profiles.iter().filter(|(g, _)| guid_ok(g)).filter_map(|(g, n)| Some((g.as_str(), shown(n)?))).take(MAX_LINKS).collect();
    let mut categories = Vec::new();
    if save_recents {
        let links: Vec<Link> = recents.iter().take(MAX_RECENTS).filter_map(|r| Some(link(&shown(&link_title(&r.name))?, &[&r.file]))).collect();
        if !links.is_empty() {
            categories.push(Category { name: RECENTS.into(), links });
        }
    }
    for (name, flag) in [(NEW_WINDOW, "--new-window"), (NEW_TAB, "--new-tab")] {
        let links: Vec<Link> = shown_profiles.iter().map(|(g, n)| link(n, &[flag, g])).collect();
        if !links.is_empty() {
            categories.push(Category { name: name.into(), links });
        }
    }
    let links: Vec<Link> = arrangements.iter().filter(|n| crate::launch::name_ok(n)).filter_map(|n| Some(link(&shown(n)?, &["--arrangement", n]))).take(MAX_LINKS).collect();
    if !links.is_empty() {
        categories.push(Category { name: RESTORE_ARRANGEMENT.into(), links });
    }
    Model { tasks: vec![link(NEW_WINDOW_DEFAULT, &["--new-window"])], categories }
}

/// Drops every link whose arguments the person removed from the Jump List (`BeginList` reports
/// them): `AppendCategory` with one of them fails the whole category, and they must never come
/// back. A category left with no link is dropped.
pub fn without_removed(m: Model, removed: &[Vec<String>]) -> Model {
    let keep = |l: &Link| !removed.contains(&l.args);
    Model {
        tasks: m.tasks.into_iter().filter(keep).collect(),
        categories: m.categories.into_iter().map(|c| Category { name: c.name, links: c.links.into_iter().filter(keep).collect() }).filter(|c| !c.links.is_empty()).collect(),
    }
}

/// The command-line text of a link: each argument quoted by the one rule std's parser inverts
/// (`ssh_probe::quote_arg`), so a name can never add or merge an argument (B10).
pub fn args_string(args: &[String]) -> String {
    let mut out = String::new();
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        crate::ssh_probe::quote_arg(a, &mut out);
    }
    out
}

/// The arguments of a command line after argv0, by `CommandLineToArgvW`'s rules: backslashes
/// before a quote are halved (an odd run escapes the quote), a quote toggles quoting, white space
/// ends an argument outside quotes. Reads back the text `args_string` wrote and the arguments
/// `BeginList` reports for a removed item.
pub fn split_args(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        while i < chars.len() && matches!(chars[i], ' ' | '\t') {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }
        let (mut arg, mut quoted) = (String::new(), false);
        while i < chars.len() {
            let mut slashes = 0;
            while i < chars.len() && chars[i] == '\\' {
                slashes += 1;
                i += 1;
            }
            if i < chars.len() && chars[i] == '"' {
                arg.extend(std::iter::repeat_n('\\', slashes / 2));
                if slashes % 2 == 1 {
                    arg.push('"');
                } else {
                    quoted = !quoted;
                }
                i += 1;
                continue;
            }
            arg.extend(std::iter::repeat_n('\\', slashes));
            if i >= chars.len() {
                break;
            }
            if !quoted && matches!(chars[i], ' ' | '\t') {
                break;
            }
            arg.push(chars[i]);
            i += 1;
        }
        out.push(arg);
    }
    out
}

/// A recent's file name: exactly `<uuid>.itermtab`, no folder part (P17).
pub fn file_name_ok(name: &str) -> bool {
    name.strip_suffix(".itermtab").is_some_and(|stem| stem.len() == 36 && stem.chars().all(|c| c.is_ascii_hexdigit() || c == '-'))
}

/// The recent profiles, newest first, at most one per profile (`ProfileDocument.addToRecents`).
#[derive(Clone, Debug, Default)]
pub struct Recents {
    pub list: Vec<Recent>,
}

impl Recents {
    /// Notes a session of this profile: its entry moves to the front, with its file unchanged for
    /// the same name and `new_file()` for a new one (a renamed profile never leaves two links that
    /// disagree with their file, `PD:84-109`). True when the list changed.
    pub fn add(&mut self, guid: &str, name: &str, new_file: &dyn Fn() -> String) -> bool {
        match self.list.iter().position(|r| r.guid == guid) {
            Some(0) if self.list[0].name == name => return false,
            Some(i) => {
                let mut r = self.list.remove(i);
                if r.name != name {
                    r.name = name.to_string();
                    r.file = new_file();
                }
                self.list.insert(0, r);
            }
            None => self.list.insert(0, Recent { guid: guid.to_string(), name: name.to_string(), file: new_file() }),
        }
        self.list.truncate(MAX_RECENTS);
        true
    }

    pub fn clear(&mut self) {
        self.list.clear();
    }

    /// The list from its file text: only entries with a clean name, a short guid and an exact
    /// `<uuid>.itermtab` file name are kept (the file is another program's to edit); at most
    /// `MAX_RECENTS`; an oversized or unreadable text is an empty list.
    pub fn load(text: &str) -> Recents {
        if text.len() > MAX_LIST_BYTES {
            return Recents::default();
        }
        let Ok(list) = serde_json::from_str::<Vec<Recent>>(text) else { return Recents::default() };
        Recents { list: list.into_iter().filter(|r| file_name_ok(&r.file) && (1..=64).contains(&r.guid.len()) && shown(&r.name).is_some() && r.name.chars().count() <= crate::launch::MAX_NAME).take(MAX_RECENTS).collect() }
    }

    pub fn save(&self) -> String {
        serde_json::to_string(&self.list).unwrap_or_else(|_| "[]".to_string())
    }

    /// The files in the folder that no entry names, among `on_disk` (file names only): the ones
    /// to delete. Only exact `<uuid>.itermtab` names are ever candidates.
    pub fn orphans(&self, on_disk: &[String]) -> Vec<String> {
        on_disk.iter().filter(|f| file_name_ok(f) && !self.list.iter().any(|r| &r.file == *f)).cloned().collect()
    }
}

/// The title of a recent's link (`ProfileDocument.swift:80-82`).
pub fn link_title(name: &str) -> String {
    format!("New \u{201c}{name}\u{201d} tab")
}

/// What an `.itermtab` file holds.
pub fn tab_file_text(guid: &str, name: &str) -> String {
    format!("{guid}\n{name}")
}

/// The guid and name in an `.itermtab` file (`ProfileDocument.swift:28-35`): at least two lines.
pub fn parse_tab_file(text: &str) -> Option<(String, String)> {
    let mut parts = text.split('\n');
    let (guid, name) = (parts.next()?, parts.next()?);
    Some((guid.to_string(), name.to_string()))
}

/// What opening an `.itermtab` file does (`ProfileDocument.swift:36-61`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TabOpen {
    /// Open the profile with this guid.
    Guid(String),
    /// The guid's profile was renamed and another profile has the old name: ask which.
    Ask { by_guid: String, by_name: String, old: String, new: String },
    /// No profile has this guid: nothing opens.
    Missing,
}

pub fn tab_open(guid: &str, name: &str, profile_name_of: &dyn Fn(&str) -> Option<String>, guid_of_name: &dyn Fn(&str) -> Option<String>) -> TabOpen {
    let Some(new) = profile_name_of(guid) else { return TabOpen::Missing };
    if new != name {
        if let Some(by_name) = guid_of_name(name) {
            return TabOpen::Ask { by_guid: guid.to_string(), by_name, old: name.to_string(), new };
        }
    }
    TabOpen::Guid(guid.to_string())
}

pub fn renamed_text(old: &str, new: &str) -> String {
    format!("Would you like to open the profile formerly named {old} that is now called {new} (which this shortcut refers to), or the profile that currently has the name {old}?")
}

pub fn open_name(n: &str) -> String {
    format!("Open {n}")
}

/// Rebuilds are at most this often (a profile edit or a burst of new sessions marks the list due
/// many times; one commit answers them).
pub const REBUILD_GAP: std::time::Duration = std::time::Duration::from_secs(1);

/// What the app tells the Jump List's rebuild loop to do next.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Tick {
    /// When to look again: set only while a rebuild is pending and held back by the gap.
    pub wake: Option<std::time::Instant>,
    /// A failure to tell the person, once until the next success.
    pub banner: Option<String>,
}

/// The rebuild loop's state: the removed items the last commit reported, the time of the last
/// commit, whether the last one failed, and the key's last value.
#[derive(Debug, Default)]
pub struct Sync {
    pub removed: Vec<Vec<String>>,
    pub failed: bool,
    pub started: bool,
    pub key_seen: Option<bool>,
    last: Option<std::time::Instant>,
}

impl Sync {
    /// One pass: nothing when `due` is clear; a wake time while the gap since the last commit
    /// holds the rebuild back; otherwise clears `due`, builds the model, drops the links the last
    /// commit saw removed and hands it to `write` (the injected COM writer), keeping the removed
    /// items it reports. A failure is not retried until `due` is set again, and is reported once.
    pub fn tick(&mut self, due: &mut bool, now: std::time::Instant, build: &dyn Fn() -> Model, write: &mut dyn FnMut(&Model) -> Result<Vec<Vec<String>>, String>) -> Tick {
        if !*due {
            return Tick::default();
        }
        if let Some(last) = self.last {
            let at = last + REBUILD_GAP;
            if now < at {
                return Tick { wake: Some(at), banner: None };
            }
        }
        *due = false;
        self.last = Some(now);
        match write(&without_removed(build(), &self.removed)) {
            Ok(removed) => {
                self.removed = removed;
                self.failed = false;
                Tick::default()
            }
            Err(e) => Tick { wake: None, banner: (!std::mem::replace(&mut self.failed, true)).then_some(e) },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GA: &str = "AAAAAAAA-0000-0000-0000-00000000000A";
    const GB: &str = "BBBBBBBB-0000-0000-0000-00000000000B";

    fn link(title: &str, args: &[&str]) -> Link {
        Link { title: title.to_string(), args: args.iter().map(|a| a.to_string()).collect() }
    }

    fn profiles() -> Vec<(String, String)> {
        vec![(GA.to_string(), "A".to_string()), (GB.to_string(), "B".to_string())]
    }

    /// `AD:1154-1180`, `AD:2649-2690`: "New Window (Default Profile)", then New Window and New Tab
    /// (every profile) and Restore Arrangement (every arrangement); Recents only with the key on.
    #[test]
    fn jump_list_follows_the_dock_menu() {
        let m = model(&profiles(), GA, &["Dev".to_string()], &[], false);
        assert_eq!(m.tasks, vec![link(NEW_WINDOW_DEFAULT, &["--new-window"])]);
        assert_eq!(
            m.categories,
            vec![
                Category { name: NEW_WINDOW.into(), links: vec![link("A", &["--new-window", GA]), link("B", &["--new-window", GB])] },
                Category { name: NEW_TAB.into(), links: vec![link("A", &["--new-tab", GA]), link("B", &["--new-tab", GB])] },
                Category { name: RESTORE_ARRANGEMENT.into(), links: vec![link("Dev", &["--arrangement", "Dev"])] },
            ]
        );
        let recents = vec![
            Recent { guid: GA.into(), name: "A".into(), file: r"C:\r\1.itermtab".into() },
            Recent { guid: GB.into(), name: "B".into(), file: r"C:\r\2.itermtab".into() },
        ];
        let on = model(&profiles(), GA, &["Dev".to_string()], &recents, true);
        assert_eq!(on.categories[0], Category { name: RECENTS.into(), links: vec![link(&link_title("A"), &[r"C:\r\1.itermtab"]), link(&link_title("B"), &[r"C:\r\2.itermtab"])] });
        assert_eq!(on.categories.len(), 4);
        let off = model(&profiles(), GA, &["Dev".to_string()], &recents, false);
        assert!(off.categories.iter().all(|c| c.name != RECENTS));
        let none = model(&profiles(), GA, &[], &[], true);
        assert!(none.categories.iter().all(|c| c.name != RESTORE_ARRANGEMENT && c.name != RECENTS));
        assert_eq!(none.categories.len(), 2);
    }

    /// B10: an entry that could carry anything but its own one argument is left out, never edited.
    #[test]
    fn unsafe_entries_are_left_out() {
        let mut p = profiles();
        p.push(("not-a-guid".into(), "Bad guid".into()));
        p.push((GA.replace('A', "C"), "bidi \u{202e}name".into()));
        p.push((GA.replace('A', "D"), "control \u{7}name".into()));
        let m = model(&p, GA, &["ok".into(), "x\u{200b}y".into(), String::new(), "n".repeat(2000)], &[], false);
        assert_eq!(m.categories[0].links.len(), 2, "{:?}", m.categories[0]);
        assert_eq!(m.categories[2].links, vec![link("ok", &["--arrangement", "ok"])]);
        let long = vec![(GB.to_string(), "n".repeat(300))];
        let m = model(&long, GB, &[], &[], false);
        assert_eq!(m.categories[0].links[0].title.chars().count(), MAX_TITLE);
        let hostile = [Recent { guid: GA.into(), name: "a\u{202e}b".into(), file: "x".into() }];
        assert!(model(&[], GA, &[], &hostile, true).categories.is_empty());
    }

    #[test]
    fn recents_follow_iterm2() {
        let n = std::cell::Cell::new(0);
        let new_file = || {
            n.set(n.get() + 1);
            format!("00000000-0000-0000-0000-{:012}.itermtab", n.get())
        };
        let mut r = Recents::default();
        assert!(r.add("G1", "A", &new_file));
        assert!(r.add("G2", "B", &new_file));
        let first = r.list[1].file.clone();
        assert!(r.add("G1", "A", &new_file));
        assert_eq!(r.list.iter().map(|x| x.guid.as_str()).collect::<Vec<_>>(), ["G1", "G2"]);
        assert_eq!(r.list[0].file, first, "the same name keeps its file");
        assert!(!r.add("G1", "A", &new_file), "already first and unchanged");
        assert!(r.add("G1", "A2", &new_file));
        assert_ne!(r.list[0].file, first, "a new name gets a new file");
        assert_eq!(r.list[0].name, "A2");
        assert_eq!(r.list.len(), 2);
        let back = Recents::load(&r.save());
        assert_eq!(back.list, r.list);
        r.clear();
        assert!(r.list.is_empty());
        assert_eq!(link_title("Dev"), "New \u{201c}Dev\u{201d} tab");
        assert_eq!(tab_file_text("G", "N"), "G\nN");
        assert_eq!(parse_tab_file(&tab_file_text("G", "N")), Some(("G".to_string(), "N".to_string())));
        assert_eq!(parse_tab_file("only"), None);
    }

    /// P17: at most 20 recents; a hostile or oversized list file loads only what is valid.
    #[test]
    fn recents_are_bounded_and_load_defensively() {
        let mut r = Recents::default();
        for i in 0..40 {
            r.add(&format!("G{i}"), &format!("P{i}"), &|| format!("00000000-0000-0000-0000-{:012}.itermtab", i));
        }
        assert_eq!(r.list.len(), MAX_RECENTS);
        assert_eq!(r.list[0].guid, "G39");
        let text = r#"[{"guid":"G","name":"ok","file":"00000000-0000-0000-0000-000000000001.itermtab"},
            {"guid":"G","name":"path","file":"..\\..\\evil.itermtab"},
            {"guid":"G","name":"dir","file":"C:\\x\\00000000-0000-0000-0000-000000000001.itermtab"},
            {"guid":"G","name":"bidi\u202e","file":"00000000-0000-0000-0000-000000000002.itermtab"},
            {"guid":"G","name":"ext","file":"00000000-0000-0000-0000-000000000003.exe"}]"#;
        let l = Recents::load(text);
        assert_eq!(l.list.len(), 1);
        assert_eq!(l.list[0].name, "ok");
        assert!(Recents::load("not json").list.is_empty());
        assert!(Recents::load(&" ".repeat(200_000)).list.is_empty());
        assert!(file_name_ok("00000000-0000-0000-0000-000000000001.itermtab"));
        assert!(!file_name_ok("a.itermtab") && !file_name_ok("00000000-0000-0000-0000-000000000001.itermtab.exe"));
    }

    /// The files in the folder that no recent names are the ones to delete (a renamed profile's
    /// old file, a dropped entry's); only `<uuid>.itermtab` names are ever candidates.
    #[test]
    fn orphans_are_found_by_name_pattern_only() {
        let mut r = Recents::default();
        r.add("G", "N", &|| "00000000-0000-0000-0000-000000000001.itermtab".into());
        let on_disk = ["00000000-0000-0000-0000-000000000001.itermtab", "00000000-0000-0000-0000-000000000002.itermtab", "recents.json", "notes.txt", "x.itermtab"].map(String::from);
        assert_eq!(r.orphans(&on_disk), vec!["00000000-0000-0000-0000-000000000002.itermtab".to_string()]);
    }

    #[test]
    fn removed_items_stay_removed() {
        let m = model(&profiles(), GA, &["Dev".to_string()], &[], false);
        let after = without_removed(m.clone(), &[vec!["--new-tab".into(), GA.into()]]);
        assert_eq!(after.categories[1].links, vec![link("B", &["--new-tab", GB])]);
        assert_eq!(after.categories[0], m.categories[0]);
        assert_eq!(after.categories[2], m.categories[2]);
        assert_eq!(after.tasks, m.tasks);
        // Every category left empty is dropped: AppendCategory with no items would fail.
        let all = without_removed(m.clone(), &[vec!["--arrangement".into(), "Dev".into()]]);
        assert!(all.categories.iter().all(|c| c.name != RESTORE_ARRANGEMENT));
    }

    /// B10: a link's argument string is one `quote_arg`ed word per argument, and the standard
    /// splitter gives the arguments back whatever the name holds.
    #[test]
    fn link_arguments_round_trip() {
        for name in ["a\"b", "a\\", "a b\\", "--url", "--new-window", "x\" --url \"ssh://h", &"n".repeat(300)] {
            let args = vec!["--arrangement".to_string(), name.to_string()];
            let text = args_string(&args);
            assert_eq!(split_args(&text), args, "{name:?} -> {text}");
            let parsed = crate::launch::parse(&split_args(&text), std::path::Path::new(r"C:\w"));
            assert_eq!(parsed, Ok(crate::launch::Request::Arrangement { name: name.to_string() }), "{name:?}");
        }
        let guid = vec!["--new-tab".to_string(), GA.to_string()];
        assert_eq!(args_string(&guid), format!("--new-tab {GA}"));
    }

    #[test]
    fn renamed_profile_follows_iterm2() {
        let names = |g: &str| match g {
            "G" => Some("New".to_string()),
            "H" => Some("Old".to_string()),
            _ => None,
        };
        let by_name = |n: &str| (n == "Old").then(|| "H".to_string());
        let none = |_: &str| None;
        assert_eq!(tab_open("G", "Old", &names, &by_name), TabOpen::Ask { by_guid: "G".into(), by_name: "H".into(), old: "Old".into(), new: "New".into() });
        assert_eq!(tab_open("G", "Old", &names, &none), TabOpen::Guid("G".into()));
        assert_eq!(tab_open("G", "New", &names, &by_name), TabOpen::Guid("G".into()));
        assert_eq!(tab_open("Z", "Old", &names, &by_name), TabOpen::Missing);
        assert_eq!(
            renamed_text("Old", "New"),
            "Would you like to open the profile formerly named Old that is now called New (which this shortcut refers to), or the profile that currently has the name Old?"
        );
        assert_eq!(open_name("Old"), "Open Old");
        assert_eq!(RENAMED_HEADING, "Profile Renamed");
        assert_eq!(RENAMED_ID, "NoSyncOpenRenamedProfile");
    }

    /// The splitter inverts `quote_arg` (CommandLineToArgvW's rules for arguments after argv0).
    #[test]
    fn split_follows_the_windows_rules() {
        assert_eq!(split_args(r#"a "b c" d\ e "f\"g" h\\"#), vec!["a", "b c", r"d\", "e", "f\"g", r"h\\"]);
        assert_eq!(split_args(r#""" x"#), vec!["".to_string(), "x".to_string()]);
        assert_eq!(split_args(r#"a\\"b c" d"#), vec![r"a\b c".to_string(), "d".to_string()]);
        assert!(split_args("   ").is_empty());
    }

    /// The real Jump List is written only through the injected writer: no test and no pure
    /// function reaches `jumplist_win`.
    #[test]
    fn the_writer_is_injected() {
        let src = include_str!("jumplist.rs");
        let live = src.split("#[cfg(test)]").next().unwrap();
        assert!(!live.contains("jumplist_win::"));
        assert!(!live.contains("ICustomDestinationList"));
    }

    #[test]
    fn the_rebuild_loop_is_throttled_and_keeps_removed_items() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let m = || model(&profiles(), GA, &[], &[], false);
        let mut sync = Sync::default();
        let mut written: Vec<Model> = Vec::new();
        let removed = vec![vec!["--new-tab".to_string(), GA.to_string()]];
        let mut due = false;
        let r = sync.tick(&mut due, t0, &m, &mut |_| panic!("not due"));
        assert_eq!(r, Tick::default());
        due = true;
        let r = sync.tick(&mut due, t0, &m, &mut |m| {
            written.push(m.clone());
            Ok(removed.clone())
        });
        assert_eq!((r, due, written.len()), (Tick::default(), false, 1));
        // Due again inside the gap: held back, with the time to look again.
        due = true;
        let r = sync.tick(&mut due, t0 + Duration::from_millis(300), &m, &mut |_| panic!("too soon"));
        assert_eq!((r.wake, due), (Some(t0 + REBUILD_GAP), true));
        // After the gap: the link the last commit saw removed is not offered again.
        let r = sync.tick(&mut due, t0 + REBUILD_GAP, &m, &mut |m| {
            written.push(m.clone());
            Ok(Vec::new())
        });
        assert_eq!((r, due), (Tick::default(), false));
        assert_eq!(written[1].categories[1].links, vec![link("B", &["--new-tab", GB])]);
        assert!(sync.removed.is_empty());
        // A failure is told once, not retried until due again, and clears on success.
        due = true;
        let now = t0 + 3 * REBUILD_GAP;
        let r = sync.tick(&mut due, now, &m, &mut |_| Err("Jump List: boom".into()));
        assert_eq!((r.banner.as_deref(), due), (Some("Jump List: boom"), false));
        due = true;
        let r = sync.tick(&mut due, now + REBUILD_GAP, &m, &mut |_| Err("Jump List: boom".into()));
        assert_eq!(r.banner, None);
        due = true;
        sync.tick(&mut due, now + 2 * REBUILD_GAP, &m, &mut |_| Ok(Vec::new()));
        assert!(!sync.failed);
    }
}

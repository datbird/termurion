//! iTerm2's triggers: the profile's list (stored as iTerm2 stores it, so imported profiles
//! work), the catalog of actions, and plain parameter substitution.

use crate::semantic::CmdPiece;

/// One trigger as a profile stores it (iTerm2's keys), with keys Tandem does not use kept.
#[derive(Clone, Debug, PartialEq)]
pub struct Trigger {
    pub regex: String,
    /// iTerm2's class name for the action ("HighlightTrigger").
    pub action: String,
    pub parameter: String,
    /// iTerm2's `partial`: checked on the line being written ("Instant").
    pub instant: bool,
    /// iTerm2 stores `disabled`.
    pub enabled: bool,
    pub name: String,
    /// Fires only while this program is in front (empty: always).
    pub job: String,
    /// 0 a regex; 100 and up an event (6b).
    pub match_type: i64,
    pub extra: toml::Table,
}

const KNOWN: [&str; 8] = ["regex", "action", "parameter", "partial", "disabled", "name", "job", "matchType"];

impl Trigger {
    pub fn from_table(t: &toml::Table) -> Trigger {
        let s = |k: &str| t.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let b = |k: &str| t.get(k).and_then(|v| v.as_bool().or_else(|| v.as_integer().map(|i| i != 0))).unwrap_or(false);
        Trigger {
            regex: s("regex"),
            action: s("action"),
            // iTerm2 saves a menu choice as a number.
            parameter: match t.get("parameter") {
                Some(toml::Value::Integer(n)) => n.to_string(),
                Some(toml::Value::Float(f)) if f.fract() == 0.0 => (*f as i64).to_string(),
                Some(toml::Value::Float(f)) => f.to_string(),
                _ => s("parameter"),
            },
            instant: b("partial"),
            enabled: !b("disabled"),
            name: s("name"),
            job: s("job"),
            match_type: t.get("matchType").and_then(|v| v.as_integer()).unwrap_or(0),
            extra: t.iter().filter(|(k, _)| !KNOWN.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect(),
        }
    }

    pub fn to_table(&self) -> toml::Table {
        let mut t = self.extra.clone();
        t.insert("regex".into(), self.regex.clone().into());
        t.insert("action".into(), self.action.clone().into());
        t.insert("parameter".into(), self.parameter.clone().into());
        t.insert("partial".into(), self.instant.into());
        t.insert("disabled".into(), (!self.enabled).into());
        t.insert("name".into(), self.name.clone().into());
        t.insert("job".into(), self.job.clone().into());
        t.insert("matchType".into(), self.match_type.into());
        t
    }
}

/// A profile's `Triggers` value as triggers (entries that are not tables are skipped).
pub fn parse_list(v: &toml::Value) -> Vec<Trigger> {
    v.as_array().map(|a| a.iter().filter_map(|e| e.as_table()).map(Trigger::from_table).collect()).unwrap_or_default()
}

/// As `parse_list`, with each trigger's place in the stored list (the index the in-place
/// writers take).
pub fn parse_indexed(v: &toml::Value) -> Vec<(usize, Trigger)> {
    v.as_array().map(|a| a.iter().enumerate().filter_map(|(i, e)| e.as_table().map(|t| (i, Trigger::from_table(t)))).collect()).unwrap_or_default()
}

pub fn to_value(list: &[Trigger]) -> toml::Value {
    toml::Value::Array(list.iter().map(|t| toml::Value::Table(t.to_table())).collect())
}

/// What an action's parameter is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    None,
    Text,
    /// A menu of (title, stored number).
    Popup(&'static [(&'static str, i64)]),
    /// `{#fg,#bg}`, either side may be empty.
    TwoColors,
    /// `name<U+1>value`.
    TwoStrings,
    /// `dot<U+1>textColor<U+1>status`.
    TabStatus,
    /// `key<U+1>1|0`, a profile check box.
    Setting,
    /// A password manager account's display string, from a pop-up of the cached names
    /// (`PasswordTrigger`'s `paramIsPopupButton`).
    Accounts,
}

/// An action as iTerm2's Triggers editor lists it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActionInfo {
    pub class: &'static str,
    pub title: &'static str,
    pub kind: Kind,
    pub placeholder: &'static str,
    /// Safe to run again on the same text (iTerm2's idempotent triggers).
    pub idempotent: bool,
    /// An Instant trigger that may fire again on the same line.
    pub multi_fire: bool,
    /// The milestone that brings it (0: not planned yet); None when it works now.
    pub later: Option<u8>,
}

const fn act(class: &'static str, title: &'static str, kind: Kind, placeholder: &'static str, idempotent: bool) -> ActionInfo {
    ActionInfo { class, title, kind, placeholder, idempotent, multi_fire: false, later: None }
}

const BOUNCE: &[(&str, i64)] = &[("Bounce Until Activated", 0), ("Bounce Once", 1)];
const BUFFER: &[(&str, i64)] = &[("Start Buffering Input", 0), ("Stop Buffering Input", 1)];
const MARK: &[(&str, i64)] = &[("Keep Scrolling", 0), ("Stop Scrolling", 1)];

/// Every terminal action, in iTerm2's menu order (by title).
pub const CATALOG: &[ActionInfo] = &[
    act("AnnotateTrigger", "Annotate\u{2026}", Kind::Text, "Enter annotation", false),
    act("BounceTrigger", "Bounce Dock Icon", Kind::Popup(BOUNCE), "", true),
    act("iTermBufferInputTrigger", "Buffer Input\u{2026}", Kind::Popup(BUFFER), "", true),
    act("CaptureTrigger", "Capture Output", Kind::Text, "Coprocess to run on activation", false),
    act("iTermSGRTrigger", "Change Style\u{2026}", Kind::Text, "Enter SGR codes. See help button for details.", false),
    ActionInfo { later: Some(0), ..act("iTermEnterWorkgroupTrigger", "Enter Workgroup\u{2026}", Kind::Text, "", true) },
    ActionInfo { later: Some(0), ..act("iTermExitWorkgroupTrigger", "Exit Workgroup", Kind::None, "", true) },
    act("iTermFoldTrigger", "Fold to Named Mark", Kind::Text, "Name of Mark", false),
    act("iTermHighlightLineTrigger", "Highlight Line\u{2026}", Kind::TwoColors, "", true),
    ActionInfo { multi_fire: true, ..act("HighlightTrigger", "Highlight Text\u{2026}", Kind::TwoColors, "", true) },
    act("iTermInjectTrigger", "Inject Data\u{2026}", Kind::Text, "Use \\e for esc, \\a for ^G.", false),
    act("iTermRPCTrigger", "Invoke Script Function", Kind::Text, "Function call", false),
    act("iTermHyperlinkTrigger", "Make Hyperlink\u{2026}", Kind::Text, "https://\\0", true),
    act("PasswordTrigger", "Open Password Manager\u{2026}", Kind::Accounts, "", false),
    act("iTermUserNotificationTrigger", "Post Notification\u{2026}", Kind::Text, "Enter Message", false),
    act("iTermShellPromptTrigger", "Prompt Detected", Kind::None, "", true),
    act("SetDirectoryTrigger", "Report Directory", Kind::Text, "Directory", true),
    act("SetHostnameTrigger", "Report User & Host", Kind::Text, "username@hostname", true),
    act("BellTrigger", "Ring Bell", Kind::None, "", false),
    act("ScriptTrigger", "Run Command\u{2026}", Kind::Text, "Enter command to run", false),
    act("CoprocessTrigger", "Run Coprocess\u{2026}", Kind::Text, "Enter coprocess command to run", false),
    act("MuteCoprocessTrigger", "Run Silent Coprocess\u{2026}", Kind::Text, "Enter coprocess command to run", false),
    act("SendTextTrigger", "Send Text\u{2026}", Kind::Text, "Enter text to send", false),
    act("MarkTrigger", "Set Mark", Kind::Popup(MARK), "", true),
    act("iTermSetNamedMarkTrigger", "Set Named Mark", Kind::Text, "Name for this mark", false),
    act("iTermSetProfileBooleanTrigger", "Set Profile Setting\u{2026}", Kind::Setting, "", true),
    act("iTermSetTabStatusTrigger", "Set Tab Status\u{2026}", Kind::TabStatus, "", true),
    act("iTermSetTitleTrigger", "Set Title\u{2026}", Kind::Text, "Enter new title", true),
    act("iTermSetUserVariableTrigger", "Set User Variable\u{2026}", Kind::TwoStrings, "Value for variable", false),
    act("AlertTrigger", "Show Alert\u{2026}", Kind::Text, "Enter text to show in alert", false),
    act("StopTrigger", "Stop Processing Triggers", Kind::None, "", true),
];

/// Older class names iTerm2 still reads.
const SYNONYMS: &[(&str, &str)] = &[("GrowlTrigger", "iTermUserNotificationTrigger")];

pub fn action_info(class: &str) -> Option<&'static ActionInfo> {
    let class = SYNONYMS.iter().find(|(old, _)| *old == class).map_or(class, |(_, new)| new);
    CATALOG.iter().find(|a| a.class == class)
}

/// iTerm2's plain parameter: `\0` to `\8` are the capture groups (a missing one is empty;
/// `\9` is never replaced), then `\a \b \e \n \r \t \\` and `\xHH`; any other backslash
/// sequence stays as written. Captured text goes in as it is.
pub fn substitute(param: &str, captures: &[Option<&str>]) -> String {
    substitute_pieces(param, captures)
        .into_iter()
        .map(|p| match p {
            CmdPiece::Text(t) | CmdPiece::Value(t) => t,
        })
        .collect()
}

/// The actions that put text on a cmd.exe command line: Run Command, Run Coprocess, Run
/// Silent Coprocess, and Capture Output (its parameter is the coprocess a double-click in
/// Captured Output runs). Their captures are program text (output, titles, directories).
pub fn runs_command(action: &str) -> bool {
    matches!(action, "ScriptTrigger" | "CoprocessTrigger" | "MuteCoprocessTrigger" | "CaptureTrigger")
}

/// `substitute` for a cmd.exe command line, through the one builder
/// (`semantic::build_cmd_line`): None when a capture the parameter uses is refused. The
/// parameter's own text is the person's and stays as written.
pub fn substitute_cmd(param: &str, captures: &[Option<&str>]) -> Option<String> {
    crate::semantic::build_cmd_line(&substitute_pieces(param, captures))
}

/// The parameter split into its text (escapes applied) and the captures it substitutes.
fn substitute_pieces(param: &str, captures: &[Option<&str>]) -> Vec<CmdPiece> {
    let chars: Vec<char> = param.chars().collect();
    let mut pieces = Vec::new();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c != '\\' || i + 1 == chars.len() {
            out.push(c);
            i += 1;
            continue;
        }
        let n = chars[i + 1];
        match n {
            '0'..='8' => {
                let g = n as usize - '0' as usize;
                pieces.push(CmdPiece::Text(std::mem::take(&mut out)));
                pieces.push(CmdPiece::Value(captures.get(g).copied().flatten().unwrap_or("").to_string()));
            }
            'a' => out.push('\x07'),
            'b' => out.push('\x08'),
            'e' => out.push('\x1b'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            '\\' => out.push('\\'),
            'x' if chars.get(i + 2).is_some_and(char::is_ascii_hexdigit) && chars.get(i + 3).is_some_and(char::is_ascii_hexdigit) => {
                let hex: String = chars[i + 2..i + 4].iter().collect();
                out.push(u8::from_str_radix(&hex, 16).map(char::from).unwrap_or('?'));
                i += 4;
                continue;
            }
            other => {
                out.push('\\');
                out.push(other);
            }
        }
        i += 2;
    }
    pieces.push(CmdPiece::Text(out));
    pieces
}

/// Enable All and Disable All in place: `disabled` set on every entry that is a table;
/// everything else stays as stored. None when the value is not a list.
pub fn set_all_disabled(v: &toml::Value, disabled: bool) -> Option<toml::Value> {
    let mut a = v.as_array()?.clone();
    for t in a.iter_mut().filter_map(toml::Value::as_table_mut) {
        t.insert("disabled".into(), disabled.into());
    }
    Some(toml::Value::Array(a))
}

/// `toggleTriggerEnabledAtIndex:` (`PS:4506-4512`) in place: `disabled` flips on the stored
/// list's entry `index`; every other entry, and every other key of this one, stays as
/// stored. None when the entry is missing or not a table.
pub fn toggle_disabled_at(v: &toml::Value, index: usize) -> Option<toml::Value> {
    let mut a = v.as_array()?.clone();
    let t = a.get_mut(index)?.as_table_mut()?;
    let disabled = Trigger::from_table(t).enabled;
    t.insert("disabled".into(), disabled.into());
    Some(toml::Value::Array(a))
}

/// The Settings editor's write: `list` (the stored value as `parse_list` read it, then
/// edited) back into the stored list. Entries that are not tables keep their places; a
/// trigger the editor left alone keeps its stored table as is; the rest fill the table
/// places in order, and triggers beyond them are appended.
pub fn merge_list(stored: Option<&toml::Value>, list: &[Trigger]) -> toml::Value {
    let mut next = list.iter();
    let mut out = Vec::new();
    for e in stored.and_then(toml::Value::as_array).into_iter().flatten() {
        match e.as_table() {
            Some(t) => {
                if let Some(trigger) = next.next() {
                    out.push(if Trigger::from_table(t) == *trigger { e.clone() } else { toml::Value::Table(trigger.to_table()) });
                }
            }
            None => out.push(e.clone()),
        }
    }
    out.extend(next.map(|t| toml::Value::Table(t.to_table())));
    toml::Value::Array(out)
}

/// The widest Run Command pool and the longest queue behind it (B5).
pub const MAX_POOL: usize = 64;
pub const MAX_WAITING: usize = 256;

/// Run Command's pool: at most `max` commands at once; the rest wait their turn, at most
/// `MAX_WAITING` of them (the newest are dropped, with one notice per burst).
#[derive(Clone, Debug, Default)]
pub struct CommandPool {
    max: usize,
    running: usize,
    waiting: std::collections::VecDeque<String>,
    /// Commands dropped since the last notice was taken.
    dropped: usize,
}

impl CommandPool {
    /// A pool `max` wide, kept within `1..=MAX_POOL` (a pool that runs nothing is no pool).
    pub fn new(max: i64) -> CommandPool {
        CommandPool { max: max.clamp(1, MAX_POOL as i64) as usize, ..Default::default() }
    }

    /// A command to run: it, when it may start now.
    pub fn submit(&mut self, cmd: String) -> Option<String> {
        if self.running < self.max {
            self.running += 1;
            Some(cmd)
        } else if self.waiting.len() < MAX_WAITING {
            self.waiting.push_back(cmd);
            None
        } else {
            self.dropped += 1;
            None
        }
    }

    /// How many commands were dropped since the last call (the caller says so once).
    pub fn take_dropped(&mut self) -> usize {
        std::mem::take(&mut self.dropped)
    }

    pub fn waiting(&self) -> usize {
        self.waiting.len()
    }

    /// A command ended: the next waiting one to start.
    pub fn finished(&mut self) -> Option<String> {
        match self.waiting.pop_front() {
            Some(next) => Some(next),
            None => {
                self.running = self.running.saturating_sub(1);
                None
            }
        }
    }

    pub fn running(&self) -> usize {
        self.running
    }
}

/// Most triggers a profile keeps, the longest pattern and parameter (P8).
pub const MAX_TRIGGERS: usize = 256;
pub const MAX_PATTERN: usize = 4096;
pub const MAX_PARAMETER: usize = 64 * 1024;
const MAX_KEYS: usize = 32;
const MAX_DEPTH: usize = 8;

fn depth_ok(v: &toml::Value, depth: usize) -> bool {
    match v {
        toml::Value::Array(a) => depth < MAX_DEPTH && a.len() <= 1024 && a.iter().all(|e| depth_ok(e, depth + 1)),
        toml::Value::Table(t) => depth < MAX_DEPTH && t.len() <= MAX_KEYS && t.values().all(|e| depth_ok(e, depth + 1)),
        toml::Value::String(s) => s.len() <= MAX_PARAMETER,
        _ => true,
    }
}

/// A stored `Triggers` value as every reader may see it, made at the one write point
/// (`Profile::set`, `Profile::from_values`): at most `MAX_TRIGGERS` tables, each with a text
/// action (an action Tandem lacks is kept, as `parse_list` keeps it), a pattern of at most `MAX_PATTERN` characters, a parameter of at most
/// `MAX_PARAMETER` bytes and bounded depth. Anything that is not a list is dropped.
pub fn clean(v: &toml::Value) -> toml::Value {
    let ok = |t: &toml::Table| {
        t.len() <= MAX_KEYS
            && t.get("action").is_some_and(toml::Value::is_str)
            && t.get("regex").and_then(toml::Value::as_str).is_none_or(|r| r.chars().count() <= MAX_PATTERN)
            && t.values().all(|e| depth_ok(e, 0))
    };
    let list = v.as_array().map(|a| a.iter().filter_map(toml::Value::as_table).filter(|t| ok(t)).take(MAX_TRIGGERS).cloned().map(toml::Value::Table).collect()).unwrap_or_default();
    toml::Value::Array(list)
}

/// At most one event per key in each period; events inside it are dropped.
#[derive(Clone, Debug)]
pub struct RateLimit<K> {
    last: std::collections::HashMap<K, std::time::Instant>,
}

impl<K> Default for RateLimit<K> {
    fn default() -> Self {
        RateLimit { last: std::collections::HashMap::new() }
    }
}

impl<K: std::hash::Hash + Eq> RateLimit<K> {
    pub fn allow(&mut self, key: K, now: std::time::Instant, period: std::time::Duration) -> bool {
        if self.last.get(&key).is_some_and(|t| now.saturating_duration_since(*t) < period) {
            return false;
        }
        self.last.insert(key, now);
        true
    }
}

/// Add Trigger...'s pattern for selected text: ICU's special characters escaped.
pub fn escape_regex(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if "\\*?+[(){}^$|.]-&".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_captures_with_shell_characters_are_refused() {
        for bad in ["foo&calc", "a|b", "a>b", "a^b", "a(b)", "a!b", "a`b", "a\\", "a\u{ff02}b"] {
            assert_eq!(substitute_cmd("run \"\\1\"", &[Some(""), Some(bad)]), None, "{bad}");
        }
        assert_eq!(substitute_cmd("run \\1", &[Some(""), Some("a b")]), Some("run \"a b\"".into()));
    }

    /// A capture the parameter already quotes goes in bare: one argument for the program.
    #[test]
    fn a_pre_quoted_capture_stays_one_argument() {
        let line = substitute_cmd("git log \"\\1\" \\1", &[Some(""), Some("a --flag")]).unwrap();
        assert_eq!(line, "git log \"a --flag\" \"a --flag\"");
        assert_eq!(crate::semantic::split_command_line(&line), ["git", "log", "a --flag", "a --flag"]);
    }

    #[test]
    fn triggers_round_trip_with_unknown_keys() {
        let v: toml::Value = toml::from_str(r#"list = [{ regex = "err(or)?", action = "HighlightTrigger", parameter = "{#ff0000,}", partial = true, disabled = false, name = "errors", job = "", matchType = 0, performance = 3 }, { regex = "x", action = "NoSuchTrigger" }]"#).unwrap();
        let list = parse_list(&v["list"]);
        assert_eq!(list.len(), 2);
        let t = &list[0];
        assert_eq!((t.regex.as_str(), t.action.as_str(), t.parameter.as_str(), t.instant, t.enabled, t.name.as_str()), ("err(or)?", "HighlightTrigger", "{#ff0000,}", true, true, "errors"));
        assert_eq!(t.to_table().get("performance"), Some(&toml::Value::Integer(3)), "unknown keys are kept");
        assert_eq!(list[1].action, "NoSuchTrigger", "unknown actions are kept, never dropped");
        assert!(list[1].enabled && !list[1].instant, "defaults");
        assert_eq!(Trigger::from_table(&t.to_table()), *t);
    }

    /// P8: the one write point bounds a profile's triggers.
    #[test]
    fn clean_bounds_a_trigger_list() {
        let mk = |regex: String, parameter: String| {
            let mut t = toml::Table::new();
            t.insert("regex".into(), regex.into());
            t.insert("action".into(), "AlertTrigger".into());
            t.insert("parameter".into(), parameter.into());
            toml::Value::Table(t)
        };
        let many = toml::Value::Array((0..300).map(|_| mk("a".into(), "b".into())).collect());
        assert_eq!(clean(&many).as_array().unwrap().len(), MAX_TRIGGERS);
        let mixed = toml::Value::Array(vec![mk("a".repeat(MAX_PATTERN + 1), String::new()), mk("a".into(), "p".repeat(MAX_PARAMETER + 1)), toml::Value::Integer(3), mk("a".repeat(MAX_PATTERN), "ok".into())]);
        assert_eq!(parse_list(&clean(&mixed)).len(), 1, "an over-long pattern or parameter and a non-table are dropped");
        assert_eq!(clean(&toml::Value::Integer(1)), toml::Value::Array(vec![]));
        let mut deep = toml::Value::Integer(1);
        for _ in 0..20 {
            deep = toml::Value::Array(vec![deep]);
        }
        let mut t = mk("a".into(), "b".into()).as_table().unwrap().clone();
        t.insert("eventParams".into(), deep);
        assert!(clean(&toml::Value::Array(vec![toml::Value::Table(t)])).as_array().unwrap().is_empty(), "nested too deep");
        let mut p = crate::profile::Profile::from_values(std::collections::BTreeMap::from([("Triggers".to_string(), many.clone())]));
        assert_eq!(p.get("Triggers").unwrap().as_array().unwrap().len(), MAX_TRIGGERS, "from_values (Dynamic Profiles inherit it)");
        p.set("Triggers", many);
        assert_eq!(p.get("Triggers").unwrap().as_array().unwrap().len(), MAX_TRIGGERS, "set");
    }

    /// B5/P17: a pool is 1..=64 wide with a bounded queue.
    #[test]
    fn command_pool_is_bounded() {
        let mut p = CommandPool::new(0);
        assert_eq!(p.submit("a".into()), Some("a".into()), "below 1 still runs one");
        assert_eq!(p.submit("b".into()), None);
        let mut p = CommandPool::new(1000);
        for i in 0..MAX_POOL {
            assert!(p.submit(i.to_string()).is_some());
        }
        assert!(p.submit("w".into()).is_none());
        for _ in 0..MAX_WAITING + 40 {
            p.submit("x".into());
        }
        assert_eq!(p.waiting(), MAX_WAITING);
        assert_eq!(p.take_dropped(), 41, "the newest were dropped, counted once");
        assert_eq!(p.take_dropped(), 0);
        assert_eq!(p.finished(), Some("w".into()), "the oldest waits first");
    }

    #[test]
    fn catalog_follows_iterm2() {
        assert_eq!(CATALOG.len(), 31);
        assert_eq!(CATALOG[0].title, "Annotate\u{2026}");
        assert_eq!(CATALOG[30].title, "Stop Processing Triggers");
        let titles: Vec<&str> = CATALOG.iter().map(|a| a.title).collect();
        let mut sorted = titles.clone();
        sorted.sort_by_key(|t| t.to_lowercase());
        assert_eq!(titles, sorted, "menu order is by title");
        assert_eq!(action_info("GrowlTrigger").map(|a| a.title), Some("Post Notification\u{2026}"), "iTerm2's synonym");
        assert_eq!(action_info("iTermSGRTrigger").map(|a| a.kind), Some(Kind::Text));
        assert_eq!(action_info("CaptureTrigger").and_then(|a| a.later), None);
        assert_eq!(action_info("iTermRPCTrigger").and_then(|a| a.later), None);
        assert_eq!(action_info("PasswordTrigger").and_then(|a| a.later), None);
        assert_eq!(action_info("PasswordTrigger").map(|a| (a.kind, a.idempotent)), Some((Kind::Accounts, false)));
        assert!(action_info("HighlightTrigger").is_some_and(|a| a.idempotent && a.multi_fire));
        assert!(action_info("BounceTrigger").is_some_and(|a| matches!(a.kind, Kind::Popup(p) if p[0] == ("Bounce Until Activated", 0))));
        assert_eq!(action_info("NoSuchTrigger"), None);
    }

    #[test]
    fn substitution_follows_iterm2() {
        let caps = [Some("whole"), Some("one"), None, Some("three")];
        assert_eq!(substitute(r"\0 \1 [\2] \3 \5", &caps), "whole one [] three ", "missing groups are empty");
        assert_eq!(substitute(r"\9", &caps), r"\9", "iTerm2 never replaces \\9");
        assert_eq!(substitute(r"\\1", &caps), r"\1", "an escaped backslash");
        assert_eq!(substitute(r"\\\1", &caps), r"\one");
        assert_eq!(substitute(r"a\tb\nc\e\a\x41\x4", &caps), "a\tb\nc\x1b\x07A\\x4");
        assert_eq!(substitute(r"\q", &caps), r"\q", "other sequences stay");
        assert_eq!(substitute("tail\\", &caps), "tail\\");
    }

    #[test]
    fn add_trigger_escapes_the_selection() {
        assert_eq!(escape_regex("a.b*c (x) [y] {z} ^$|?+-&\\"), r"a\.b\*c \(x\) \[y\] \{z\} \^\$\|\?\+\-\&\\");
        assert_eq!(escape_regex("plain"), "plain");
    }

    #[test]
    fn run_command_pool_waits_past_16() {
        let mut pool = CommandPool::new(16);
        for i in 0..16 {
            assert_eq!(pool.submit(format!("c{i}")), Some(format!("c{i}")), "starts now");
        }
        assert_eq!(pool.submit("c16".into()), None, "waits");
        assert_eq!(pool.submit("c17".into()), None);
        assert_eq!(pool.finished(), Some("c16".into()), "the first waiting one starts");
        assert_eq!(pool.finished(), Some("c17".into()));
        assert_eq!(pool.finished(), None);
        assert_eq!(pool.running(), 15, "16, two swapped in, one ended");
    }

    #[test]
    fn alert_rate_limit_per_trigger() {
        let mut limit: RateLimit<(u64, usize)> = RateLimit::default();
        let t0 = std::time::Instant::now();
        let second = std::time::Duration::from_secs(1);
        assert!(limit.allow((1, 0), t0, second));
        assert!(!limit.allow((1, 0), t0 + std::time::Duration::from_millis(500), second), "dropped, not deferred");
        assert!(limit.allow((1, 1), t0, second), "per trigger");
        assert!(limit.allow((2, 0), t0, second), "per pane");
        assert!(limit.allow((1, 0), t0 + second, second));
    }

    /// A stored list with an entry that is not a table, a numeric popup parameter, an
    /// integer `disabled` and a key Tandem does not know.
    fn odd_list() -> toml::Value {
        let v: toml::Value = toml::from_str(r#"list = [{ regex = "a", action = "BounceTrigger", parameter = 1, disabled = 1, extra = "x" }, "junk", { regex = "b", action = "BellTrigger" }]"#).unwrap();
        v["list"].clone()
    }

    #[test]
    fn enable_all_sets_disabled_in_place() {
        let v = odd_list();
        let on = set_all_disabled(&v, false).unwrap();
        let off = set_all_disabled(&v, true).unwrap();
        assert!(parse_list(&on).iter().all(|t| t.enabled));
        assert!(parse_list(&off).iter().all(|t| !t.enabled));
        for out in [&on, &off] {
            let a = out.as_array().unwrap();
            assert_eq!(a.len(), 3);
            assert_eq!(a[1], toml::Value::String("junk".into()), "a non-table entry stays in place");
            assert_eq!(a[0]["parameter"], toml::Value::Integer(1), "the stored type stays");
            assert_eq!(a[0]["extra"], toml::Value::String("x".into()));
            assert_eq!(a[2].as_table().unwrap().len(), 3, "only disabled is added");
        }
        assert!(set_all_disabled(&toml::Value::String("x".into()), true).is_none());
    }

    #[test]
    fn toggle_flips_one_entry_in_place() {
        let v = odd_list();
        let out = toggle_disabled_at(&v, 0).unwrap();
        let a = out.as_array().unwrap();
        assert_eq!(a[0]["disabled"], toml::Value::Boolean(false), "1 read as true, flipped");
        assert_eq!(a[0]["parameter"], toml::Value::Integer(1));
        assert_eq!(a[1], v[1]);
        assert_eq!(a[2], v[2], "other entries byte for byte");
        let again = toggle_disabled_at(&out, 2).unwrap();
        assert_eq!(again[2]["disabled"], toml::Value::Boolean(true));
        assert_eq!(again[0], out[0]);
        assert!(toggle_disabled_at(&v, 1).is_none(), "not a table");
        assert!(toggle_disabled_at(&v, 3).is_none(), "past the end");
    }

    #[test]
    fn indexes_are_places_in_the_stored_array() {
        let places: Vec<(usize, String)> = parse_indexed(&odd_list()).into_iter().map(|(i, t)| (i, t.regex)).collect();
        assert_eq!(places, vec![(0, "a".to_string()), (2, "b".to_string())]);
    }

    #[test]
    fn editor_write_keeps_what_it_cannot_parse() {
        let v = odd_list();
        let mut list = parse_list(&v);
        // Unchanged: the stored value comes back as it was.
        assert_eq!(merge_list(Some(&v), &list), v);
        // One edited, one added: the junk stays in place, the untouched entry as stored.
        list[1].regex = "c".into();
        list.push(Trigger { regex: "d".into(), ..list[1].clone() });
        let out = merge_list(Some(&v), &list);
        let a = out.as_array().unwrap();
        assert_eq!(a.len(), 4);
        assert_eq!(a[0], v[0]);
        assert_eq!(a[1], v[1]);
        assert_eq!(a[2]["regex"], toml::Value::String("c".into()));
        assert_eq!(a[3]["regex"], toml::Value::String("d".into()));
        // One removed: the tables left fill the table places in order, the junk stays.
        let out = merge_list(Some(&v), &parse_list(&v)[1..]);
        let a = out.as_array().unwrap();
        assert_eq!(a.len(), 2);
        assert_eq!(a[0]["regex"], toml::Value::String("b".into()));
        assert_eq!(a[1], v[1]);
        // Nothing stored yet.
        assert_eq!(merge_list(None, &list[..1]), to_value(&list[..1]));
    }

    #[test]
    fn number_parameters_from_iterm2() {
        let v: toml::Value = toml::from_str(r#"list = [{ regex = "a", action = "BounceTrigger", parameter = 1 }, { regex = "b", action = "HighlightTrigger", parameter = 2.0 }]"#).unwrap();
        let list = parse_list(&v["list"]);
        assert_eq!(list[0].parameter, "1");
        assert_eq!(list[1].parameter, "2");
    }
}

//! iTerm2's triggers: the profile's list (stored as iTerm2 stores it, so imported profiles
//! work), the catalog of actions, and plain parameter substitution.

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
            parameter: s("parameter"),
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
    ActionInfo { later: Some(9), ..act("CaptureTrigger", "Capture Output", Kind::Text, "Coprocess to run on activation", false) },
    act("iTermSGRTrigger", "Change Style\u{2026}", Kind::Text, "Enter SGR codes. See help button for details.", false),
    ActionInfo { later: Some(0), ..act("iTermEnterWorkgroupTrigger", "Enter Workgroup\u{2026}", Kind::Text, "", true) },
    ActionInfo { later: Some(0), ..act("iTermExitWorkgroupTrigger", "Exit Workgroup", Kind::None, "", true) },
    act("iTermFoldTrigger", "Fold to Named Mark", Kind::Text, "Name of Mark", false),
    act("iTermHighlightLineTrigger", "Highlight Line\u{2026}", Kind::TwoColors, "", true),
    ActionInfo { multi_fire: true, ..act("HighlightTrigger", "Highlight Text\u{2026}", Kind::TwoColors, "", true) },
    act("iTermInjectTrigger", "Inject Data\u{2026}", Kind::Text, "Use \\e for esc, \\a for ^G.", false),
    ActionInfo { later: Some(11), ..act("iTermRPCTrigger", "Invoke Script Function", Kind::Text, "Function call", false) },
    act("iTermHyperlinkTrigger", "Make Hyperlink\u{2026}", Kind::Text, "https://\\0", true),
    ActionInfo { later: Some(10), ..act("PasswordTrigger", "Open Password Manager\u{2026}", Kind::Text, "Open Password Manager to Unlock", false) },
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
    let chars: Vec<char> = param.chars().collect();
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
                out.push_str(captures.get(g).copied().flatten().unwrap_or(""));
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
    out
}

/// Every trigger on or off (Enable All, Disable All).
pub fn set_all_enabled(list: &[Trigger], on: bool) -> Vec<Trigger> {
    list.iter().map(|t| Trigger { enabled: on, ..t.clone() }).collect()
}

/// Run Command's pool: at most `max` commands at once; the rest wait their turn.
#[derive(Clone, Debug, Default)]
pub struct CommandPool {
    max: usize,
    running: usize,
    waiting: std::collections::VecDeque<String>,
}

impl CommandPool {
    pub fn new(max: usize) -> CommandPool {
        CommandPool { max, ..Default::default() }
    }

    /// A command to run: it, when it may start now.
    pub fn submit(&mut self, cmd: String) -> Option<String> {
        if self.running < self.max {
            self.running += 1;
            Some(cmd)
        } else {
            self.waiting.push_back(cmd);
            None
        }
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
        assert_eq!(action_info("CaptureTrigger").and_then(|a| a.later), Some(9));
        assert_eq!(action_info("iTermRPCTrigger").and_then(|a| a.later), Some(11));
        assert_eq!(action_info("PasswordTrigger").and_then(|a| a.later), Some(10));
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

    #[test]
    fn enable_all_rewrites_disabled() {
        let v: toml::Value = toml::from_str(r#"list = [{ regex = "a", action = "BellTrigger", disabled = true }, { regex = "b", action = "BellTrigger" }]"#).unwrap();
        let list = parse_list(&v["list"]);
        assert!(set_all_enabled(&list, true).iter().all(|t| t.enabled));
        assert!(set_all_enabled(&list, false).iter().all(|t| !t.enabled));
    }
}

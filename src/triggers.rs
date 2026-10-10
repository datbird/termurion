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
    /// A workgroup's id, from a pop-up of the configured workgroups sorted by name
    /// (`EnterWorkgroupTrigger.paramIsPopupButton`).
    Workgroup,
    /// A browser Highlight Text's `text;background` pair of hex colors (`HighlightBrowserTrigger`).
    BrowserColors,
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
    /// Exit Workgroup's "leader only" check box (`hasLeaderOnlyOption`), stored in the event params.
    pub leader_only_option: bool,
    /// Offered only for a regex and the events that have a live session to act on (every event
    /// but Session Ended): the two workgroup actions (`allowedMatchTypes`).
    pub live_session_only: bool,
}

const fn act(class: &'static str, title: &'static str, kind: Kind, placeholder: &'static str, idempotent: bool) -> ActionInfo {
    ActionInfo { class, title, kind, placeholder, idempotent, multi_fire: false, later: None, leader_only_option: false, live_session_only: false }
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
    ActionInfo { live_session_only: true, ..act("iTermEnterWorkgroupTrigger", "Enter Workgroup\u{2026}", Kind::Workgroup, "", true) },
    ActionInfo { live_session_only: true, leader_only_option: true, ..act("iTermExitWorkgroupTrigger", "Exit Workgroup", Kind::None, "", true) },
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
pub const SYNONYMS: &[(&str, &str)] = &[("GrowlTrigger", "iTermUserNotificationTrigger")];

pub fn action_info(class: &str) -> Option<&'static ActionInfo> {
    let class = SYNONYMS.iter().find(|(old, _)| *old == class).map_or(class, |(_, new)| new);
    CATALOG.iter().find(|a| a.class == class)
}

/// Match types a browser trigger stores (`matchType`): 0 is a regex (terminal), 1 the page URL,
/// 2 the page content.
pub const URL_REGEX: i64 = 1;
pub const PAGE_CONTENT_REGEX: i64 = 2;

const fn bact(class: &'static str, title: &'static str, kind: Kind, placeholder: &'static str, idempotent: bool) -> ActionInfo {
    act(class, title, kind, placeholder, idempotent)
}

/// Every browser action, in iTerm2's menu order (by title). Enter Reader Mode and the two
/// workgroup actions wait for their milestone (13): `later` is cleared when Task 10 lands.
pub const BROWSER_CATALOG: &[ActionInfo] = &[
    ActionInfo { later: Some(13), ..bact("iTerm2SharedARC.ReaderModeBrowserTrigger", "Enter Reader Mode", Kind::None, "", false) },
    ActionInfo { later: Some(13), live_session_only: false, ..bact("iTermEnterWorkgroupBrowserTrigger", "Enter Workgroup\u{2026}", Kind::Workgroup, "", true) },
    ActionInfo { later: Some(13), ..bact("iTermExitWorkgroupBrowserTrigger", "Exit Workgroup", Kind::None, "", true) },
    bact("iTerm2SharedARC.HighlightBrowserTrigger", "Highlight Text", Kind::BrowserColors, "", false),
    bact("InjectJavascriptContentTrigger", "Inject Javascript (Content Regex)", Kind::Text, "Javascript", false),
    bact("InjectJavascriptURLTrigger", "Inject Javascript (URL Regex)", Kind::Text, "Javascript", false),
    bact("iTerm2SharedARC.HyperlinkBrowserTrigger", "Make Hyperlink\u{2026}", Kind::Text, "https://\\0", true),
    bact("ReloadBrowserTrigger", "Reload After Delay", Kind::Text, "Delay in seconds", false),
];

/// The bare Swift names iTerm2 may also have written.
pub const BROWSER_SYNONYMS: &[(&str, &str)] = &[
    ("ReaderModeBrowserTrigger", "iTerm2SharedARC.ReaderModeBrowserTrigger"),
    ("HighlightBrowserTrigger", "iTerm2SharedARC.HighlightBrowserTrigger"),
    ("HyperlinkBrowserTrigger", "iTerm2SharedARC.HyperlinkBrowserTrigger"),
];

pub fn browser_action_info(class: &str) -> Option<&'static ActionInfo> {
    let class = BROWSER_SYNONYMS.iter().find(|(bare, _)| *bare == class).map_or(class, |(_, full)| full);
    BROWSER_CATALOG.iter().find(|a| a.class == class)
}

pub fn is_browser_action(class: &str) -> bool {
    browser_action_info(class).is_some()
}

/// The match type each browser action is fixed to.
pub fn browser_match_type(class: &str) -> Option<i64> {
    let a = browser_action_info(class)?;
    Some(match a.class {
        "iTerm2SharedARC.HighlightBrowserTrigger" | "iTerm2SharedARC.HyperlinkBrowserTrigger" | "InjectJavascriptContentTrigger" => PAGE_CONTENT_REGEX,
        _ => URL_REGEX,
    })
}

/// Highlight's `text;background`: split on ";" when there are at least two parts, an empty part
/// is None (`HGT:36-41`).
pub fn browser_colors(param: &str) -> (Option<String>, Option<String>) {
    let parts: Vec<&str> = param.split(';').collect();
    if parts.len() < 2 {
        return (None, None);
    }
    let some = |s: &str| if s.is_empty() { None } else { Some(s.to_string()) };
    (some(parts[0]), some(parts[1]))
}

/// A stored color as CSS: "#rrggbb" kept, "p3#rrrrggggbbbb" as `color(display-p3 r g b)`;
/// anything else None, so no stored text other than these two shapes reaches a style.
pub fn css_color(stored: &str) -> Option<String> {
    let hex = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_hexdigit());
    if let Some(h) = stored.strip_prefix('#') {
        return hex(h, 6).then(|| stored.to_string());
    }
    let h = stored.strip_prefix("p3#")?;
    if !hex(h, 12) {
        return None;
    }
    let part = |i: usize| u32::from_str_radix(&h[i * 4..i * 4 + 4], 16).map(|v| format!("{:.4}", f64::from(v) / 65535.0));
    Some(format!("color(display-p3 {} {} {})", part(0).ok()?, part(1).ok()?, part(2).ok()?))
}

/// The key Exit Workgroup keeps its leader-only flag under, in the trigger's event params
/// (`ExitWorkgroupTrigger.leaderOnlyParamKey`).
pub const LEADER_ONLY_KEY: &str = "leaderOnly";

/// The longest workgroup id a trigger's parameter may hold: ids are Tandem's own uuids or
/// `builtin.` names, so anything longer is not one.
pub const MAX_WORKGROUP_ID: usize = 128;

/// A workgroup's name as the pop-up and the description show it (`EWT:67-79`): "Untitled" for none.
fn workgroup_name(name: &str) -> &str {
    if name.is_empty() { "Untitled" } else { name }
}

/// The configured workgroups, (id, name), in the pop-up's order: by shown name, case-insensitive
/// (`localizedCaseInsensitiveCompare:`), the id breaking a tie.
pub fn sorted_workgroups(workgroups: &[(String, String)]) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = workgroups.to_vec();
    v.sort_by_key(|(id, name)| (workgroup_name(name).to_lowercase(), id.clone()));
    v
}

/// The workgroup a trigger enters: its parameter, or, when empty, the first in the pop-up's
/// order (what the pop-up visibly shows, `EWT:48-66`). None when there is neither.
pub fn effective_workgroup(param: &str, workgroups: &[(String, String)]) -> Option<String> {
    if !param.is_empty() {
        return Some(param.to_string());
    }
    sorted_workgroups(workgroups).into_iter().next().map(|(id, _)| id)
}

/// The label for the workgroup the parameter names (`EWT:67-79`): "(unset)" for none, "(missing)"
/// for one that is not configured, "Untitled" for a nameless one, else its name. The name is the
/// person's or an import's, so it is made inert before it is shown.
pub fn enter_workgroup_label(param: &str, workgroups: &[(String, String)]) -> String {
    let Some(id) = effective_workgroup(param, workgroups) else { return "(unset)".into() };
    match workgroups.iter().find(|(i, _)| *i == id) {
        Some((_, name)) => crate::inert_files::display_text(workgroup_name(name), 128, false),
        None => "(missing)".into(),
    }
}

/// "Enter Workgroup \u{201c}<label>\u{201d}" (`EWT:25-27`).
pub fn enter_workgroup_description(label: &str) -> String {
    format!("Enter Workgroup \u{201c}{label}\u{201d}")
}

/// The events an action is offered for: every event, except that the two workgroup actions need
/// a live session and so leave out Session Ended (`allowedMatchTypes`). A regex match is always
/// allowed.
pub fn allowed_events(class: &str) -> Vec<crate::trigger_events::EventKind> {
    use crate::trigger_events::EventKind;
    let live = action_info(class).is_some_and(|a| a.live_session_only);
    EventKind::ALL.iter().copied().filter(|e| !(live && *e == EventKind::SessionEnded)).collect()
}

/// Whether a trigger of this action may match this way (a stored one from an import may not).
pub fn allows_match_type(class: &str, match_type: i64) -> bool {
    if !action_info(class).is_some_and(|a| a.live_session_only) {
        return true;
    }
    match crate::trigger_events::EventKind::from_match_type(match_type) {
        Some(e) => allowed_events(class).contains(&e),
        None => match_type == 0,
    }
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

/// Whether a browser trigger's parameter and match type are within the bounds its action allows
/// (import, export and the API carry browser triggers through `sanitized_trigger`). A parameter
/// that is not text, or too long for its action, is refused; the match type is 0, 1 or 2.
pub fn browser_trigger_ok(action: &str, match_type: i64, parameter: Option<&toml::Value>) -> bool {
    let Some(a) = browser_action_info(action) else { return false };
    if !(0..=PAGE_CONTENT_REGEX).contains(&match_type) {
        return false;
    }
    let text = match parameter {
        None => "",
        Some(toml::Value::String(s)) => s.as_str(),
        Some(_) => return false,
    };
    let (chars, bytes) = (text.chars().count(), text.len());
    match a.class {
        "InjectJavascriptContentTrigger" | "InjectJavascriptURLTrigger" => bytes <= MAX_PARAMETER,
        "iTerm2SharedARC.HyperlinkBrowserTrigger" => chars <= 2048,
        "iTermEnterWorkgroupBrowserTrigger" => chars <= MAX_WORKGROUP_ID,
        // "#rrggbb;#rrggbb" or the p3 form: at most two 15-character colors and a separator.
        "iTerm2SharedARC.HighlightBrowserTrigger" => chars <= 32,
        "ReloadBrowserTrigger" => chars <= 40,
        _ => chars <= 256,
    }
}

/// Make Hyperlink creates a link only for an absolute http or https URL (R13x5-10): the template
/// is filled from page text, then the result must parse as such a URL. `javascript:`, `data:`,
/// `file:`, `vbscript:`, `blob:` and relative text are refused. Task 9 applies it at creation.
pub fn hyperlink_url_ok(url: &str) -> Option<String> {
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let p = crate::ai_policy::url_parts(url)?;
    (p.scheme.eq_ignore_ascii_case("http") || p.scheme.eq_ignore_ascii_case("https")).then(|| url.to_string())
}

/// The classes whose effect is only what is drawn or shown: these import turned on (B7, final
/// review). Every other class, and any class the catalog does not know, imports turned off.
pub const IMPORT_ENABLED: &[&str] = &[
    "AnnotateTrigger",
    "BounceTrigger",
    "iTermSGRTrigger",
    "iTermFoldTrigger",
    "iTermHighlightLineTrigger",
    "HighlightTrigger",
    "iTermHyperlinkTrigger",
    // Browser: drawn-only (Highlight validates its colors, Hyperlink's click is the person's).
    "iTerm2SharedARC.HighlightBrowserTrigger",
    "HighlightBrowserTrigger",
    "iTerm2SharedARC.HyperlinkBrowserTrigger",
    "HyperlinkBrowserTrigger",
    "iTermUserNotificationTrigger",
    "GrowlTrigger",
    "iTermShellPromptTrigger",
    "BellTrigger",
    "MarkTrigger",
    "iTermSetNamedMarkTrigger",
    "iTermSetTabStatusTrigger",
    "iTermSetTitleTrigger",
    "AlertTrigger",
    "StopTrigger",
];

/// The classes that change what Tandem does or trusts (commands, typed text, injected escape
/// sequences, held keystrokes, workgroups, the reported folder, host and user, user variables,
/// settings, scripts). An imported trigger of these is stored turned off until the person enables it.
pub const IMPORT_OFF: &[&str] = &[
    // Browser: page text is untrusted and these run script in the page, reload it, change the
    // view or start/end a workgroup, so an import is stored off and never counts as consent.
    "iTerm2SharedARC.ReaderModeBrowserTrigger",
    "ReaderModeBrowserTrigger",
    "iTermEnterWorkgroupBrowserTrigger",
    "iTermExitWorkgroupBrowserTrigger",
    "InjectJavascriptContentTrigger",
    "InjectJavascriptURLTrigger",
    "ReloadBrowserTrigger",
    "iTermBufferInputTrigger",
    "CaptureTrigger",
    "iTermEnterWorkgroupTrigger",
    "iTermExitWorkgroupTrigger",
    "iTermInjectTrigger",
    "iTermRPCTrigger",
    "PasswordTrigger",
    "SetDirectoryTrigger",
    "SetHostnameTrigger",
    "ScriptTrigger",
    "CoprocessTrigger",
    "MuteCoprocessTrigger",
    "SendTextTrigger",
    "iTermSetProfileBooleanTrigger",
    "iTermSetUserVariableTrigger",
];

/// Whether an imported trigger of this class is stored turned on. An allowlist: a class added to
/// the catalog later imports turned off until it is listed (`every_catalog_class_is_classified`).
pub fn imports_enabled(action: &str) -> bool {
    IMPORT_ENABLED.contains(&action)
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
/// Enable All never turns on an entry that needs its own confirm (Inject Javascript, Reload After
/// Delay, Enter Reader Mode and the two workgroup actions: the classes `imports_enabled` refuses); it
/// leaves those as stored. Disable All disables everything.
pub fn set_all_disabled(v: &toml::Value, disabled: bool) -> Option<toml::Value> {
    let mut a = v.as_array()?.clone();
    for t in a.iter_mut().filter_map(toml::Value::as_table_mut) {
        if !disabled && needs_own_confirm(t.get("action").and_then(|x| x.as_str()).unwrap_or("")) {
            continue;
        }
        t.insert("disabled".into(), disabled.into());
    }
    Some(toml::Value::Array(a))
}

/// A browser action that only its own confirm turns on: any browser class an import does not turn on.
pub fn needs_own_confirm(action: &str) -> bool {
    is_browser_action(action) && !imports_enabled(action)
}

/// How many entries of the stored list `set_all_disabled(.., false)` leaves off for that reason.
pub fn left_off_by_enable_all(v: &toml::Value) -> usize {
    v.as_array().map_or(0, |a| a.iter().filter_map(toml::Value::as_table).filter(|t| needs_own_confirm(t.get("action").and_then(|x| x.as_str()).unwrap_or("")) && !Trigger::from_table(t).enabled).count())
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
/// Keys in a trigger or in a nested table: 64, as B1 bounds an imported event's parameters (P8).
const MAX_KEYS: usize = 64;
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
    /// Whether `allow` would say yes, recording nothing.
    pub fn would_allow(&self, key: &K, now: std::time::Instant, period: std::time::Duration) -> bool {
        !self.last.get(key).is_some_and(|t| now.saturating_duration_since(*t) < period)
    }

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

    /// Review Focus 7: the two workgroup triggers as iTerm2 has them (`EWT`, `XWT`).
    #[test]
    fn workgroup_triggers_follow_iterm2() {
        use crate::trigger_events::EventKind;
        let enter = action_info("iTermEnterWorkgroupTrigger").unwrap();
        assert_eq!((enter.later, enter.idempotent, enter.title, enter.kind), (None, true, "Enter Workgroup\u{2026}", Kind::Workgroup));
        assert!(!enter.leader_only_option);
        let exit = action_info("iTermExitWorkgroupTrigger").unwrap();
        assert_eq!((exit.later, exit.idempotent, exit.title, exit.kind), (None, true, "Exit Workgroup", Kind::None));
        assert!(exit.leader_only_option, "the leader-only check box");
        assert_eq!(LEADER_ONLY_KEY, "leaderOnly");
        assert_eq!(enter_workgroup_label("", &[]), "(unset)");
        assert_eq!(enter_workgroup_label("x", &[("y".into(), "Y".into())]), "(missing)");
        assert_eq!(enter_workgroup_label("y", &[("y".into(), String::new())]), "Untitled");
        assert_eq!(enter_workgroup_label("y", &[("y".into(), "Why".into())]), "Why");
        assert_eq!(enter_workgroup_description("Why"), "Enter Workgroup \u{201c}Why\u{201d}");
        let two = [("b".to_string(), "Beta".to_string()), ("a".to_string(), "Alpha".to_string())];
        assert_eq!(effective_workgroup("", &two), Some("a".to_string()), "empty falls back to the first by sorted name");
        assert_eq!(effective_workgroup("b", &two), Some("b".to_string()));
        assert_eq!(effective_workgroup("", &[]), None);
        assert_eq!(enter_workgroup_label("", &two), "Alpha", "the label shows what the pop-up shows");
        let mixed = [("1".to_string(), "bravo".to_string()), ("2".to_string(), "Alpha".to_string()), ("3".to_string(), String::new())];
        assert_eq!(sorted_workgroups(&mixed).iter().map(|(i, _)| i.as_str()).collect::<Vec<_>>(), ["2", "1", "3"], "case-insensitive; Untitled sorts as Untitled");
        // Session Ended has no live session to act on: not offered for either; every other event is.
        for class in ["iTermEnterWorkgroupTrigger", "iTermExitWorkgroupTrigger"] {
            let events = allowed_events(class);
            assert!(!events.contains(&EventKind::SessionEnded), "{class}");
            assert_eq!(events.len(), EventKind::ALL.len() - 1);
            assert!(allows_match_type(class, 0) && allows_match_type(class, EventKind::JobStarted.match_type()));
            assert!(!allows_match_type(class, EventKind::SessionEnded.match_type()));
        }
        assert_eq!(allowed_events("BellTrigger").len(), EventKind::ALL.len());
        assert!(allows_match_type("BellTrigger", EventKind::SessionEnded.match_type()));
        // A name an import chose cannot disguise the label.
        assert_eq!(enter_workgroup_label("y", &[("y".into(), "A\u{202e}B".into())]), "AB");
        // Imported ones still arrive off.
        assert!(!imports_enabled("iTermEnterWorkgroupTrigger") && !imports_enabled("iTermExitWorkgroupTrigger"));
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

    fn parse_value(items: &[(&str, bool)]) -> toml::Value {
        to_value(&items.iter().map(|(a, enabled)| Trigger { regex: "x".into(), action: a.to_string(), parameter: String::new(), instant: false, enabled: *enabled, name: String::new(), job: String::new(), match_type: 0, extra: toml::Table::new() }).collect::<Vec<_>>())
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
        // Enable All never turns on an entry that needs its own confirm; Disable All still turns it off.
        let mixed = parse_value(&[("BellTrigger", false), ("InjectJavascriptURLTrigger", false), ("ReloadBrowserTrigger", false), ("iTerm2SharedARC.ReaderModeBrowserTrigger", false), ("iTermEnterWorkgroupBrowserTrigger", false), ("iTermExitWorkgroupBrowserTrigger", false), ("iTerm2SharedARC.HighlightBrowserTrigger", false)]);
        let on = set_all_disabled(&mixed, false).unwrap();
        let state: Vec<(String, bool)> = parse_list(&on).into_iter().map(|t| (t.action, t.enabled)).collect();
        assert_eq!(state.iter().filter(|(_, e)| *e).map(|(a, _)| a.as_str()).collect::<Vec<_>>(), vec!["BellTrigger", "iTerm2SharedARC.HighlightBrowserTrigger"]);
        assert_eq!(left_off_by_enable_all(&mixed), 5);
        let all_on = parse_value(&[("InjectJavascriptURLTrigger", true)]);
        assert!(parse_list(&set_all_disabled(&all_on, true).unwrap()).iter().all(|t| !t.enabled), "Disable All still disables");
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

    #[test]
    fn browser_catalog_follows_iterm2() {
        let want = [
            ("iTerm2SharedARC.ReaderModeBrowserTrigger", "Enter Reader Mode", 1),
            ("iTermEnterWorkgroupBrowserTrigger", "Enter Workgroup\u{2026}", 1),
            ("iTermExitWorkgroupBrowserTrigger", "Exit Workgroup", 1),
            ("iTerm2SharedARC.HighlightBrowserTrigger", "Highlight Text", 2),
            ("InjectJavascriptContentTrigger", "Inject Javascript (Content Regex)", 2),
            ("InjectJavascriptURLTrigger", "Inject Javascript (URL Regex)", 1),
            ("iTerm2SharedARC.HyperlinkBrowserTrigger", "Make Hyperlink\u{2026}", 2),
            ("ReloadBrowserTrigger", "Reload After Delay", 1),
        ];
        assert_eq!(BROWSER_CATALOG.len(), 8);
        for (a, (class, title, mt)) in BROWSER_CATALOG.iter().zip(want) {
            assert_eq!((a.class, a.title), (class, title));
            assert_eq!(browser_match_type(class), Some(mt), "{class}");
            assert!(is_browser_action(class) && action_info(class).is_none());
        }
        assert_eq!(browser_action_info("HighlightBrowserTrigger").map(|a| a.class), Some("iTerm2SharedARC.HighlightBrowserTrigger"));
        assert_eq!(browser_action_info("HyperlinkBrowserTrigger").map(|a| a.title), Some("Make Hyperlink\u{2026}"));
        assert_eq!(browser_action_info("ReaderModeBrowserTrigger").map(|a| a.title), Some("Enter Reader Mode"));
        assert!(is_browser_action("HighlightBrowserTrigger"));
        let k = |c| browser_action_info(c).unwrap();
        assert_eq!(k("iTerm2SharedARC.ReaderModeBrowserTrigger").kind, Kind::None);
        assert_eq!(k("iTermExitWorkgroupBrowserTrigger").kind, Kind::None);
        assert_eq!(k("iTermEnterWorkgroupBrowserTrigger").kind, Kind::Workgroup);
        assert_eq!(k("HighlightBrowserTrigger").kind, Kind::BrowserColors);
        assert_eq!(k("ReloadBrowserTrigger").kind, Kind::Text);
        assert_eq!(k("ReloadBrowserTrigger").placeholder, "Delay in seconds");
        assert_eq!(k("InjectJavascriptURLTrigger").placeholder, "Javascript");
        assert_eq!(k("InjectJavascriptContentTrigger").placeholder, "Javascript");
        assert_eq!(k("HyperlinkBrowserTrigger").placeholder, "https://\\0");
        for (c, idem) in [("ReloadBrowserTrigger", false), ("InjectJavascriptURLTrigger", false), ("InjectJavascriptContentTrigger", false), ("HyperlinkBrowserTrigger", true), ("iTermEnterWorkgroupBrowserTrigger", true), ("iTermExitWorkgroupBrowserTrigger", true)] {
            assert_eq!(k(c).idempotent, idem, "{c}");
        }
        for t in CATALOG {
            assert!(browser_action_info(t.class).is_none() && !is_browser_action(t.class), "{}", t.class);
            assert_eq!(browser_match_type(t.class), None);
        }
        assert_eq!((URL_REGEX, PAGE_CONTENT_REGEX), (1, 2));
        for c in ["iTerm2SharedARC.ReaderModeBrowserTrigger", "iTermEnterWorkgroupBrowserTrigger", "iTermExitWorkgroupBrowserTrigger"] {
            assert_eq!(k(c).later, Some(13), "{c}");
        }
    }

    #[test]
    fn browser_import_classification_is_recorded() {
        // Page text is untrusted: only the inert, drawn-only actions import turned on.
        for c in ["iTerm2SharedARC.HighlightBrowserTrigger", "HighlightBrowserTrigger", "iTerm2SharedARC.HyperlinkBrowserTrigger", "HyperlinkBrowserTrigger"] {
            assert!(imports_enabled(c), "{c}");
        }
        for c in ["iTerm2SharedARC.ReaderModeBrowserTrigger", "ReaderModeBrowserTrigger", "iTermEnterWorkgroupBrowserTrigger", "iTermExitWorkgroupBrowserTrigger", "InjectJavascriptContentTrigger", "InjectJavascriptURLTrigger", "ReloadBrowserTrigger"] {
            assert!(!imports_enabled(c), "{c}");
        }
        for a in BROWSER_CATALOG {
            let n = IMPORT_ENABLED.iter().chain(IMPORT_OFF).filter(|c| **c == a.class).count();
            assert_eq!(n, 1, "{}", a.class);
        }
    }

    #[test]
    fn highlight_colors_follow_iterm2() {
        assert_eq!(browser_colors("#ff0000;#00ff00"), (Some("#ff0000".into()), Some("#00ff00".into())));
        assert_eq!(browser_colors(";#00ff00"), (None, Some("#00ff00".into())));
        assert_eq!(browser_colors("#ff0000"), (None, None));
        assert_eq!(css_color("#0a0b0c"), Some("#0a0b0c".into()));
        assert_eq!(css_color("p3#ffff00000000"), Some("color(display-p3 1.0000 0.0000 0.0000)".into()));
        assert_eq!(css_color("red"), None);
        // Page-facing CSS never carries anything but these two shapes.
        for bad in ["#ff0000;color:red", "#ff000", "#ff00000", "#gggggg", "p3#ffff", "p3#ffff0000000g", "#ff0000}", "url(x)", "", "#ff0000 ", "#FF0000\n"] {
            let ok = css_color(bad).is_some();
            assert!(!ok || bad == "#FF0000\n", "{bad:?}");
        }
        assert_eq!(css_color("#ABCDEF"), Some("#ABCDEF".into()));
        assert_eq!(css_color("#ff0000\n"), None);
    }
}

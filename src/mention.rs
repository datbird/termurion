//! Mentions (`MentionParser`, `CockpitWindowController`): the `@` tokens that name a session,
//! the Cockpit's chips, and the `@` picker's model. Pure.
//!
//! A mention grants an AI chat access to a pane, so what may be named is narrow: only a
//! canonical stable id (ruling P3), and only in text the person typed (ruling P9).

use crate::cockpit_rows::{self, RowKey, World};
use crate::layout::PaneId;
use std::ops::Range;

/// A bare UUID (a legacy session guid or a workgroup instance).
pub const UUID_PATTERN: &str = "[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}";
/// What a mention of a session that no longer exists shows.
pub const DEFUNCT: &str = "[defunct session]";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mention {
    /// The whole match including the "@", in bytes of the searched text.
    pub range: Range<usize>,
    /// "session:" or "wg-", lowercased.
    pub prefix: Option<String>,
    /// A uuid verbatim; a stable id folded by `canonical`, else as written.
    pub id: String,
}

fn patterns() -> &'static [fancy_regex::Regex; 2] {
    static RE: std::sync::OnceLock<[fancy_regex::Regex; 2]> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        let id = format!("{UUID_PATTERN}|{}", crate::stable_id::TOKEN_PATTERN);
        let build = |at: &str| fancy_regex::Regex::new(&format!("(?i){at}(session:|wg-)?({id})(?![0-9A-Za-z-])")).expect("the mention pattern compiles");
        [build("@"), build("@?")]
    })
}

/// Whether the character before `at` leaves room for a bare mention: not alphanumeric, "_" or "-".
fn left_boundary(text: &str, at: usize) -> bool {
    text[..at].chars().next_back().is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == '-'))
}

/// Every mention in `text` (`MentionParser.mentions`). With `optional_at` (text a model wrote,
/// which sometimes drops the "@") a bare match is admitted only for a checksum-valid stable id
/// with no prefix and a clean left boundary; an "@"-signed match is the same either way. The
/// checksum is checked only for a bare match: with the "@" a malformed id is kept as written
/// (ruling R12f-5, `canonicalStableID ?? rawToken`).
pub fn mentions(text: &str, optional_at: bool) -> Vec<Mention> {
    let re = &patterns()[usize::from(optional_at)];
    let mut out = Vec::new();
    for caps in re.captures_iter(text).flatten() {
        let (Some(whole), Some(id)) = (caps.get(0), caps.get(2)) else { continue };
        let prefix = caps.get(1).map(|p| p.as_str().to_ascii_lowercase());
        let canonical = crate::stable_id::canonical(id.as_str());
        if !text[whole.start()..].starts_with('@') && (prefix.is_some() || canonical.is_none() || !left_boundary(text, whole.start())) {
            continue;
        }
        out.push(Mention { range: whole.range(), prefix, id: canonical.unwrap_or_else(|| id.as_str().to_string()) });
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Chip {
    Window { guid: String },
    Tab { id: u64 },
    Session { stable_id: String },
}

impl Chip {
    /// The chip as text: "win:<guid>", "tab:<id>", or the session's stable id.
    pub fn token(&self) -> String {
        match self {
            Chip::Window { guid } => format!("win:{guid}"),
            Chip::Tab { id } => format!("tab:{id}"),
            Chip::Session { stable_id } => stable_id.clone(),
        }
    }

    /// The chip a token names; a session token must be a checksum-valid stable id.
    pub fn parse(token: &str) -> Option<Chip> {
        if let Some(guid) = token.strip_prefix("win:") {
            return (!guid.is_empty()).then(|| Chip::Window { guid: guid.to_string() });
        }
        if let Some(id) = token.strip_prefix("tab:") {
            return id.parse().ok().map(|id| Chip::Tab { id });
        }
        crate::stable_id::canonical(token).map(|stable_id| Chip::Session { stable_id })
    }
}

/// The "@" the picker completes (`CW:985-1034`): the byte offset of the first "@" at a word
/// start (the text's start, whitespace or U+FFFC, the object-replacement character an inline
/// attachment leaves) and the query after it up to the next whitespace.
pub fn completion_target(text: &str) -> Option<(usize, String)> {
    let mut before: Option<char> = None;
    for (i, c) in text.char_indices() {
        if c == '@' && before.is_none_or(|b| b.is_whitespace() || b == '\u{FFFC}') {
            let query = text[i + 1..].chars().take_while(|c| !c.is_whitespace()).collect();
            return Some((i, query));
        }
        before = Some(c);
    }
    None
}

/// `text` without the "@query" `completion_target` found (the picker's commit takes the typed
/// query out and leaves the rest as it was).
pub fn remove_target(text: &str) -> Option<String> {
    let (at, query) = completion_target(text)?;
    let end = at + 1 + query.len();
    Some(format!("{}{}", &text[..at], &text[end..]))
}

#[derive(Clone, Debug, PartialEq)]
pub struct PickerItem {
    pub chip: Chip,
    pub label: String,
    pub depth: u8,
}

/// The picker's list (`CW:985-1034`): each window, its tabs and their sessions in outline order,
/// then the buried sessions under no parent. A case-insensitive substring match on the label
/// keeps an item and its ancestors.
pub fn picker_items(world: &World, query: &str) -> Vec<PickerItem> {
    let q = query.to_lowercase();
    let hit = |label: &str| q.is_empty() || label.to_lowercase().contains(&q);
    let session = |s: &cockpit_rows::SessionFact, depth: u8| PickerItem { chip: Chip::Session { stable_id: s.stable_id.clone() }, label: cockpit_rows::session_title(&s.name, s.pane), depth };
    let mut out = Vec::new();
    for w in &world.windows {
        let window = PickerItem { chip: Chip::Window { guid: w.guid.clone() }, label: cockpit_rows::window_title(w.number, &w.title), depth: 0 };
        let mut kept = Vec::new();
        for (n, t) in w.tabs.iter().enumerate() {
            let tab = PickerItem { chip: Chip::Tab { id: t.id }, label: cockpit_rows::tab_title(&t.title, n + 1), depth: 1 };
            let sessions: Vec<PickerItem> = t.sessions.iter().map(|s| session(s, 2)).filter(|i| hit(&i.label)).collect();
            if hit(&tab.label) || !sessions.is_empty() {
                kept.push(tab);
                kept.extend(sessions);
            }
        }
        if hit(&window.label) || !kept.is_empty() {
            out.push(window);
            out.extend(kept);
        }
    }
    out.extend(world.buried.iter().map(|s| session(s, 0)).filter(|i| hit(&i.label)));
    out
}

#[derive(Clone, Debug, PartialEq)]
pub struct Picker {
    pub query: String,
    pub items: Vec<PickerItem>,
    pub selected: usize,
}

impl Picker {
    pub fn new(world: &World, query: &str) -> Picker {
        Picker { query: query.to_string(), items: picker_items(world, query), selected: 0 }
    }

    /// Both moves clamp at the ends.
    pub fn up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn down(&mut self) {
        self.selected = (self.selected + 1).min(self.items.len().saturating_sub(1));
    }

    pub fn current(&self) -> Option<&PickerItem> {
        self.items.get(self.selected)
    }
}

/// The form a chat mention is sent as: "@" and a session chip's stable id. A window or tab
/// chip only groups sessions and has none (`ChatSessionMentionAttachment`).
pub fn mention_text(chip: &Chip) -> Option<String> {
    match chip {
        Chip::Session { stable_id } => Some(format!("@{stable_id}")),
        _ => None,
    }
}

/// The chips the rows chosen stand for (`CW:1074-1125`): a window row, a tab row and a session
/// row give one chip; a bucket, the buried root and a workgroup give one session chip per
/// session under them. Each chip once, in the order met.
pub fn chips_for_rows(keys: &[RowKey], world: &World) -> Vec<Chip> {
    let session = |s: &cockpit_rows::SessionFact| Chip::Session { stable_id: s.stable_id.clone() };
    let mut out: Vec<Chip> = Vec::new();
    for key in keys {
        let found: Vec<Chip> = match key {
            RowKey::Window(guid) => vec![Chip::Window { guid: guid.clone() }],
            RowKey::Tab(id) => vec![Chip::Tab { id: *id }],
            RowKey::Session(pane) => world.session(*pane).map(session).into_iter().collect(),
            RowKey::BuriedRoot => world.buried.iter().map(session).collect(),
            RowKey::Workgroup(_) => Vec::new(),
            RowKey::Group { scope, status } => {
                let in_bucket = |s: &&cockpit_rows::SessionFact| &cockpit_rows::status_key(&s.status) == status;
                if scope == cockpit_rows::BURIED_SCOPE {
                    world.buried.iter().filter(in_bucket).map(session).collect()
                } else {
                    world.windows.iter().filter(|w| &w.guid == scope).flat_map(|w| &w.tabs).flat_map(|t| &t.sessions).filter(in_bucket).map(session).collect()
                }
            }
        };
        for c in found {
            if !out.contains(&c) {
                out.push(c);
            }
        }
    }
    out
}

/// Who wrote a message's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The person, through the chat input.
    Typed,
    /// A model, a watcher event, a tool result or text pasted from a remote program.
    Other,
}

/// The stable ids a message grants its chat control of, each once (ruling P9). Only text the
/// person typed grants: a model's text, a watcher event, a tool result or text pasted from a
/// remote program grants nothing, whatever it contains. Only an "@"-signed, checksum-valid
/// stable id grants (a bare id, a bad checksum, a uuid and a workgroup never do).
pub fn granted_ids(text: &str, source: Source) -> Vec<String> {
    if source != Source::Typed {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    for m in mentions(text, false) {
        if m.prefix.as_deref().is_some_and(|p| p != "session:") || !crate::stable_id::is_valid(&m.id) || out.contains(&m.id) {
            continue;
        }
        out.push(m.id);
    }
    out
}

/// The pane a mention names, live or buried: only a stable id does (ruling P3). A uuid, a
/// session guid, a decimal pane id and a workgroup name nothing.
pub fn resolve(world: &World, mention: &Mention) -> Option<PaneId> {
    if mention.prefix.as_deref().is_some_and(|p| p != "session:") {
        return None;
    }
    let all = world.windows.iter().flat_map(|w| &w.tabs).flat_map(|t| &t.sessions).chain(&world.buried);
    crate::stable_id::find(&mention.id, all.map(|s| (s.pane, s.stable_id.as_str())))
}

/// What a chip shows: a window's, tab's or session's current title, else `DEFUNCT`.
pub fn chip_label(world: &World, chip: &Chip) -> String {
    let found = match chip {
        Chip::Window { guid } => world.windows.iter().find(|w| &w.guid == guid).map(|w| cockpit_rows::window_title(w.number, &w.title)),
        Chip::Tab { id } => world.windows.iter().find_map(|w| w.tabs.iter().position(|t| t.id == *id).map(|n| cockpit_rows::tab_title(&w.tabs[n].title, n + 1))),
        Chip::Session { stable_id } => {
            let all = world.windows.iter().flat_map(|w| &w.tabs).flat_map(|t| &t.sessions).chain(&world.buried);
            let pane = crate::stable_id::find(stable_id, all.clone().map(|s| (s.pane, s.stable_id.as_str())));
            pane.and_then(|p| all.into_iter().find(|s| s.pane == p)).map(|s| cockpit_rows::session_title(&s.name, s.pane))
        }
    };
    found.unwrap_or_else(|| DEFUNCT.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cockpit_rows::{SessionFact, TabFact, WindowFact, BURIED_SCOPE};
    use crate::tabstatus::Status;

    const ID: &str = "ptys_0123456789ABW";

    fn m(range: Range<usize>, prefix: Option<&str>, id: &str) -> Mention {
        Mention { range, prefix: prefix.map(str::to_string), id: id.to_string() }
    }

    #[test]
    fn mentions_follow_iterm2() {
        assert_eq!(mentions("ping @ptys_0123456789ABW now", false), vec![m(5..24, None, ID)]);
        assert_eq!(mentions("@PTYS_0123456789abw", false), vec![m(0..19, None, ID)]);
        assert_eq!(mentions("@session:ptys_0123456789ABW", false), vec![m(0..27, Some("session:"), ID)]);
        assert_eq!(mentions("@SESSION:ptys_0123456789ABW", false)[0].prefix.as_deref(), Some("session:"));
        let uuid = "3F2504E0-4F89-11D3-9A0C-0305E82C3301";
        assert_eq!(mentions(&format!("@wg-{uuid}"), false), vec![m(0..40, Some("wg-"), uuid)]);
        assert_eq!(mentions(&format!("@{uuid}"), false), vec![m(0..37, None, uuid)]);
        // The lookahead refuses a longer run.
        assert_eq!(mentions("@ptys_0123456789ABWX", false), vec![]);
        assert_eq!(mentions("@ptys_0123456789ABW-x", false), vec![]);
        // Without the "@" only the optional form matches, and only a checksum-valid stable id.
        assert_eq!(mentions(ID, false), vec![]);
        assert_eq!(mentions(ID, true), vec![m(0..18, None, ID)]);
        assert_eq!(mentions(&format!("see {uuid} now"), true), vec![]);
        assert_eq!(mentions("x_ptys_0123456789ABW", true), vec![]);
        assert_eq!(mentions("a-ptys_0123456789ABW", true), vec![]);
        assert_eq!(mentions("(ptys_0123456789ABW)", true), vec![m(1..19, None, ID)]);
        assert_eq!(mentions("session:ptys_0123456789ABW", true).len(), 0);
        // A bad checksum with the "@" keeps the raw token; bare it matches nothing.
        assert_eq!(mentions("@ptys_0123456789ABX", false), vec![m(0..19, None, "ptys_0123456789ABX")]);
        assert_eq!(mentions("ptys_0123456789ABX", true), vec![]);
        // An "@"-signed mention is the same in both modes; two in one text.
        assert_eq!(mentions("@ptys_0123456789ABW and ptys_0123456789ABW", true).len(), 2);
        assert_eq!(mentions("@ptys_0123456789ABW", true), mentions("@ptys_0123456789ABW", false));
    }

    #[test]
    fn chip_tokens_follow_iterm2() {
        let w = Chip::Window { guid: "pty-A".into() };
        let t = Chip::Tab { id: 12 };
        let s = Chip::Session { stable_id: ID.into() };
        assert_eq!((w.token().as_str(), t.token().as_str(), s.token().as_str()), ("win:pty-A", "tab:12", ID));
        for c in [w, t, s.clone()] {
            assert_eq!(Chip::parse(&c.token()), Some(c));
        }
        assert_eq!(Chip::parse("tab:x"), None);
        assert_eq!(Chip::parse("ptys_bad"), None);
        assert_eq!(Chip::parse(""), None);
        assert_eq!(Chip::parse("ptys_0123456789abw"), Some(s.clone()));
        assert_eq!(mention_text(&s).as_deref(), Some("@ptys_0123456789ABW"));
        assert_eq!(mention_text(&Chip::Window { guid: "pty-A".into() }), None);
        assert_eq!(mention_text(&Chip::Tab { id: 1 }), None);

        let world = world();
        let group = RowKey::Group { scope: "pty-A".into(), status: "Waiting".into() };
        assert_eq!(chips_for_rows(&[group], &world), vec![sess("ptys_1000000000001")]);
        let all_a = RowKey::Window("pty-A".into());
        assert_eq!(chips_for_rows(&[all_a.clone()], &world), vec![Chip::Window { guid: "pty-A".into() }]);
        assert_eq!(chips_for_rows(&[RowKey::Tab(12), RowKey::Session(4)], &world), vec![Chip::Tab { id: 12 }, sess("ptys_4000000000004")]);
        // The buried root and a buried bucket give one chip per buried session; a workgroup none.
        assert_eq!(chips_for_rows(&[RowKey::BuriedRoot], &world), vec![sess("ptys_9000000000009")]);
        let buried = RowKey::Group { scope: BURIED_SCOPE.into(), status: "Waiting".into() };
        assert_eq!(chips_for_rows(&[buried], &world), vec![sess("ptys_9000000000009")]);
        assert_eq!(chips_for_rows(&[RowKey::Workgroup("w".into())], &world), vec![]);
        // A session reached twice is one chip.
        assert_eq!(chips_for_rows(&[RowKey::Session(4), RowKey::Session(4)], &world), vec![sess("ptys_4000000000004")]);
        // A row whose session vanished gives no chip.
        assert_eq!(chips_for_rows(&[RowKey::Session(77)], &world), vec![]);
    }

    #[test]
    fn completion_target_follows_iterm2() {
        assert_eq!(completion_target("@ab"), Some((0, "ab".into())));
        assert_eq!(completion_target("ls @b"), Some((3, "b".into())));
        assert_eq!(completion_target("a@b"), None);
        assert_eq!(completion_target("\u{FFFC}@x"), Some((3, "x".into())));
        assert_eq!(completion_target("@a @b"), Some((0, "a".into())));
        assert_eq!(completion_target("no at"), None);
        assert_eq!(completion_target("@"), Some((0, String::new())));
        assert_eq!(completion_target("a@b @c d"), Some((4, "c".into())));
        assert_eq!(completion_target("x\n@q"), Some((2, "q".into())));
    }

    #[test]
    fn committing_removes_the_typed_query() {
        assert_eq!(remove_target("@ab").as_deref(), Some(""));
        assert_eq!(remove_target("ls @b now").as_deref(), Some("ls  now"));
        assert_eq!(remove_target("echo hi @").as_deref(), Some("echo hi "));
        assert_eq!(remove_target("a@b"), None);
    }

    fn fact(pane: PaneId, status: Option<&str>, name: &str) -> SessionFact {
        let st = Status { status: status.map(str::to_string), ..Default::default() };
        SessionFact { pane, stable_id: format!("ptys_{pane}00000000000{pane}").chars().take(18).collect(), name: name.into(), status: st, exited: false, alt_screen: false, ancestors: Vec::new(), armed: false }
    }

    fn sess(id: &str) -> Chip {
        Chip::Session { stable_id: id.to_string() }
    }

    fn world() -> World {
        World {
            windows: vec![
                WindowFact {
                    guid: "pty-A".into(),
                    number: 1,
                    title: String::new(),
                    active_tab: 0,
                    armed: false,
                    tabs: vec![
                        TabFact { id: 11, title: String::new(), sessions: vec![fact(1, Some("Waiting"), ""), fact(2, None, "")] },
                        TabFact { id: 12, title: "build".into(), sessions: vec![fact(3, Some("working"), "")] },
                    ],
                },
                WindowFact { guid: "pty-B".into(), number: 2, title: "Logs".into(), active_tab: 0, armed: false, tabs: vec![TabFact { id: 21, title: String::new(), sessions: vec![fact(4, Some("idle"), "tail logs")] }] },
            ],
            buried: vec![fact(9, Some("Waiting"), "")],
        }
    }

    fn labels(items: &[PickerItem]) -> Vec<(String, u8)> {
        items.iter().map(|i| (i.label.clone(), i.depth)).collect()
    }

    #[test]
    fn picker_follows_iterm2() {
        let w = world();
        let all = picker_items(&w, "");
        let l = |s: &str, d: u8| (s.to_string(), d);
        assert_eq!(labels(&all), vec![l("Window 1", 0), l("Tab 1", 1), l("1", 2), l("2", 2), l("build", 1), l("3", 2), l("Window 2: Logs", 0), l("Tab 1", 1), l("tail logs", 2), l("9", 0)]);
        assert_eq!(all[0].chip, Chip::Window { guid: "pty-A".into() });
        assert_eq!(all[1].chip, Chip::Tab { id: 11 });
        assert_eq!(all[2].chip, sess("ptys_1000000000001"));
        // A match keeps its ancestors and nothing under a non-matching sibling.
        assert_eq!(labels(&picker_items(&w, "build")), vec![l("Window 1", 0), l("build", 1)]);
        assert_eq!(labels(&picker_items(&w, "BUILD")), vec![l("Window 1", 0), l("build", 1)]);
        assert_eq!(labels(&picker_items(&w, "tail")), vec![l("Window 2: Logs", 0), l("Tab 1", 1), l("tail logs", 2)]);
        assert_eq!(picker_items(&w, "zzz"), vec![]);

        let mut p = Picker::new(&w, "");
        assert_eq!((p.selected, p.items.len()), (0, 10));
        p.up();
        assert_eq!(p.selected, 0);
        for _ in 0..30 {
            p.down();
        }
        assert_eq!(p.selected, 9);
        assert_eq!(p.current().map(|i| i.label.as_str()), Some("9"));
        let empty = Picker::new(&w, "zzz");
        assert!(empty.current().is_none());
    }

    #[test]
    fn model_text_grants_nothing() {
        let text = format!("go to @{ID} and also {ID} and @session:{ID}");
        // Only text the person typed names a session to grant.
        assert_eq!(granted_ids(&text, Source::Typed), vec![ID.to_string()]);
        assert_eq!(granted_ids(&text, Source::Other), Vec::<String>::new());
        assert_eq!(granted_ids(ID, Source::Typed), Vec::<String>::new());
        assert_eq!(granted_ids("@PTYS_0123456789abw", Source::Typed), vec![ID.to_string()]);
        // A raw token with a bad checksum is never granted.
        assert_eq!(granted_ids("@ptys_0123456789ABX", Source::Typed), Vec::<String>::new());
        // A workgroup instance is not a session.
        assert_eq!(granted_ids("@wg-3F2504E0-4F89-11D3-9A0C-0305E82C3301", Source::Typed), Vec::<String>::new());
    }

    #[test]
    fn non_stable_mentions_resolve_to_nothing() {
        let w = world();
        let one = |t: &str| mentions(t, false).into_iter().next();
        assert_eq!(resolve(&w, &one("@ptys_1000000000001").unwrap()), Some(1));
        assert_eq!(resolve(&w, &one("@session:ptys_4000000000004").unwrap()), Some(4));
        assert_eq!(resolve(&w, &one("@PTYS_9000000000009").unwrap()), Some(9));
        // A session guid (a UUID) names nothing, with or without a prefix.
        assert_eq!(resolve(&w, &one("@6F1C2A3B-0000-4000-8000-000000000001").unwrap()), None);
        assert_eq!(resolve(&w, &one("@session:6F1C2A3B-0000-4000-8000-000000000001").unwrap()), None);
        assert_eq!(resolve(&w, &one("@wg-6F1C2A3B-0000-4000-8000-000000000001").unwrap()), None);
        // A decimal pane id is no mention at all.
        assert_eq!(one("@1"), None);
        assert_eq!(one("@12"), None);
        // A stable id nobody holds names nothing, and a "wg-" prefix never names a session.
        assert_eq!(resolve(&w, &one("@ptys_0123456789ABW").unwrap()), None);
        assert_eq!(resolve(&w, &one("@wg-ptys_1000000000001").unwrap()), None);
    }

    #[test]
    fn labels_follow_the_world() {
        let w = world();
        assert_eq!(chip_label(&w, &Chip::Window { guid: "pty-B".into() }), "Window 2: Logs");
        assert_eq!(chip_label(&w, &Chip::Tab { id: 12 }), "build");
        assert_eq!(chip_label(&w, &Chip::Tab { id: 11 }), "Tab 1");
        assert_eq!(chip_label(&w, &sess("ptys_4000000000004")), "tail logs");
        assert_eq!(chip_label(&w, &sess("ptys_9000000000009")), "9");
        assert_eq!(chip_label(&w, &sess(ID)), DEFUNCT);
        assert_eq!(chip_label(&w, &Chip::Window { guid: "gone".into() }), DEFUNCT);
    }
}

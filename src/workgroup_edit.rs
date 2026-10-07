//! The Workgroups editor's model (Settings > Arrangements > Workgroups): tree rows, labels, the
//! rules for what a session may be given, and the undo snapshots. Pure. iTerm2's editor is
//! `iTermWorkgroupDetailViewController.swift` (WDV), `iTermWorkgroupSessionDetailViewController.swift`
//! (WSDV) and `iTermWorkgroupsEditingViewController.swift` (WEV).

use std::collections::BTreeSet;

use crate::workgroup::{Kind, Mode, NavigationShortcuts, SessionConfig, ToolbarItem, UsageProvider, Workgroup, MAX_SESSIONS};

pub const EMPTY_STATE: &str = "Select a workgroup on the left or click + to create one.";
pub const WHAT_IS: &str = "What is a Workgroup?";
pub const WHAT_IS_TEXT: &str = "A **Workgroup** turns a single session into a coordinated set of related sessions, each with its own command and toolbar; see the [Workgroups documentation](https://iterm2.com/documentation-workgroups.html) for the full story.";
pub const NAME_PLACEHOLDER: &str = "Workgroup name";
pub const UNTITLED_WORKGROUP: &str = "Untitled Workgroup";
pub const ADD_PRESET: &str = "Add Preset";
pub const NO_SESSION: &str = "No session selected.";

/// The undo action names (`WDV`, `WSDV`, `WEV`).
pub const ADD_SESSION: &str = "Add Session";
pub const REMOVE_SESSION: &str = "Remove Session";
pub const RENAME_WORKGROUP: &str = "Rename Workgroup";
pub const CHANGE_SPLIT_LOCATION: &str = "Change Split Location";
pub const CHANGE_PROFILE: &str = "Change Profile";
pub const CHANGE_MODE: &str = "Change Mode";
pub const CHANGE_ORIENTATION: &str = "Change Orientation";
pub const CHANGE_SIDE: &str = "Change Side";
pub const CHANGE_USAGE: &str = "Change AI Usage Settings";
pub const ADD_ITEM: &str = "Add Toolbar Item";
pub const REMOVE_ITEM: &str = "Remove Toolbar Item";
pub const RENAME_PEER: &str = "Rename Peer";
pub const CHANGE_COMMAND: &str = "Change Command";
pub const CHANGE_FILE_COMMAND: &str = "Change Per-File Command";
pub const CHANGE_URL: &str = "Change URL";
pub const CHANGE_SPACER: &str = "Change Spacer Width";
pub const REORDER_ITEM: &str = "Reorder Toolbar Item";
pub const CHANGE_PEER_SHORTCUT: &str = "Change Peer Shortcut";
pub const CHANGE_SHORTCUT: &str = "Change Shortcut";
pub const CHANGE_WORKGROUPS: &str = "Change Workgroups";
pub const ACTIONS: [&str; 20] = [
    ADD_SESSION, REMOVE_SESSION, RENAME_WORKGROUP, CHANGE_SPLIT_LOCATION, CHANGE_PROFILE, CHANGE_MODE, CHANGE_ORIENTATION, CHANGE_SIDE, CHANGE_USAGE, ADD_ITEM,
    REMOVE_ITEM, RENAME_PEER, CHANGE_COMMAND, CHANGE_FILE_COMMAND, CHANGE_URL, CHANGE_SPACER, REORDER_ITEM, CHANGE_PEER_SHORTCUT, CHANGE_SHORTCUT, CHANGE_WORKGROUPS,
];

/// The default names for new sessions, in iTerm2's order (`WorkgroupAnimalNames.swift`).
pub const ANIMALS: [&str; 63] = [
    "Fox", "Hen", "Owl", "Cat", "Dog", "Pig", "Cow", "Bat", "Bee",
    "Elk", "Doe", "Jay", "Ram", "Ewe", "Rat", "Ant", "Yak", "Emu",
    "Ape", "Asp", "Frog", "Hare", "Wolf", "Bear", "Deer", "Goat", "Lion",
    "Mole", "Seal", "Swan", "Crab", "Tuna", "Lamb", "Kiwi", "Duck", "Fawn",
    "Lynx", "Puma", "Moth", "Wren", "Mink", "Toad", "Mule", "Boar", "Crow",
    "Dove", "Otter", "Skunk", "Koala", "Llama", "Camel", "Eagle", "Goose", "Hyena",
    "Sloth", "Gecko", "Panda", "Snake", "Tiger", "Whale", "Zebra", "Mouse", "Horse",
];

/// The first animal not in `taken`; when all are, "Fox 2", "Hen 2", ... then "Fox 3", ...
pub fn animal_name(taken: &BTreeSet<String>) -> String {
    if let Some(n) = ANIMALS.iter().find(|n| !taken.contains(**n)) {
        return (*n).to_string();
    }
    let mut suffix = 2u64;
    loop {
        for n in ANIMALS {
            let c = format!("{n} {suffix}");
            if !taken.contains(&c) {
                return c;
            }
        }
        suffix += 1;
    }
}

fn taken_names(w: &Workgroup) -> BTreeSet<String> {
    w.sessions.iter().filter(|s| !s.display_name.is_empty()).map(|s| s.display_name.clone()).collect()
}

fn percent(location: f64) -> i64 {
    (location * 100.0).round() as i64
}

fn label_of(s: &SessionConfig) -> String {
    use crate::workgroup::Orientation;
    match &s.kind {
        Kind::Root => "Main session".to_string(),
        Kind::Peer => format!("Peer: {}", s.display_name),
        Kind::Split(sp) => match sp.orientation {
            Orientation::Vertical => format!("Split: Vertical {}%", percent(sp.location)),
            Orientation::Horizontal => format!("Split: Horizontal {}%", percent(sp.location)),
        },
        Kind::Tab => "Tab".to_string(),
    }
}

/// The tree as rows: depth, label, session id (`WDV:568-584`), parents before children, siblings in
/// the order they were added.
pub fn tree_rows(w: &Workgroup) -> Vec<(usize, String, String)> {
    let mut out = Vec::new();
    let Some(root) = w.root() else { return out };
    let mut stack = vec![(0usize, root)];
    while let Some((depth, s)) = stack.pop() {
        out.push((depth, label_of(s), s.unique_identifier.clone()));
        if out.len() > MAX_SESSIONS * 4 {
            break;
        }
        for c in w.children(&s.unique_identifier).into_iter().rev() {
            stack.push((depth + 1, c));
        }
    }
    out
}

fn is_peer(s: &SessionConfig) -> bool {
    s.kind == Kind::Peer
}

fn hosts_peers(w: &Workgroup, id: &str) -> bool {
    w.children(id).iter().any(|c| is_peer(c))
}

/// In a peer group: a peer itself, or a host with peer children (`WDV:545-561`).
pub fn in_peer_group(w: &Workgroup, s: &SessionConfig) -> bool {
    is_peer(s) || hosts_peers(w, &s.unique_identifier)
}

/// The Add menu from the selected session alone (`WDV:314-335`): Add Split is off for a peer.
/// `add_menu_in` also knows about a host with peers.
pub fn add_menu(selected: &SessionConfig) -> Vec<(&'static str, bool)> {
    vec![("Add Peer", true), ("Add Split", !is_peer(selected)), ("Add Tab", true)]
}

/// The Add menu with the whole workgroup: no split from anywhere inside a peer group.
pub fn add_menu_in(w: &Workgroup, selected: &SessionConfig) -> Vec<(&'static str, bool)> {
    vec![("Add Peer", true), ("Add Split", !in_peer_group(w, selected)), ("Add Tab", true)]
}

/// Room for one more session (the stored limit).
pub fn can_add(w: &Workgroup) -> bool {
    w.sessions.len() < MAX_SESSIONS
}

/// The workgroup with a new session of `kind` ("peer", "split" or "tab") added for `selected`
/// (`WDV:354-420`): an animal name, no command, no items; a peer under a peer goes to that peer's
/// host; a split is vertical, trailing, 0.5 and is refused inside a peer group; a tab attaches to
/// the root. The workgroup is returned unchanged when nothing can be added.
pub fn add_child(w: &Workgroup, selected: &str, kind: &str) -> Workgroup {
    use crate::workgroup::{Orientation, Side, SplitSettings};
    let mut out = w.clone();
    let (Some(sel), Some(root)) = (w.session(selected), w.root()) else { return out };
    if !can_add(w) {
        return out;
    }
    let (kind, parent) = match kind {
        "peer" => (Kind::Peer, if is_peer(sel) { sel.parent_id.clone().unwrap_or_else(|| sel.unique_identifier.clone()) } else { sel.unique_identifier.clone() }),
        "split" => {
            if in_peer_group(w, sel) {
                return out;
            }
            (Kind::Split(SplitSettings { orientation: Orientation::Vertical, side: Side::TrailingOrBottom, location: 0.5 }), sel.unique_identifier.clone())
        }
        "tab" => (Kind::Tab, root.unique_identifier.clone()),
        _ => return out,
    };
    out.sessions.push(SessionConfig {
        unique_identifier: crate::workgroup::new_id(),
        parent_id: Some(parent),
        kind,
        profile_guid: None,
        command: String::new(),
        per_file_command: String::new(),
        url_string: String::new(),
        toolbar_items: Vec::new(),
        display_name: animal_name(&taken_names(w)),
        mode: Mode::Regular,
        peer_switch_shortcut: None,
    });
    enforce_mode_switcher(&mut out);
    out
}

/// Every host of peers and each of its peers carries a mode switcher (first when added); nobody
/// else does (`WDV:451-479`).
pub fn enforce_mode_switcher(w: &mut Workgroup) {
    let mut need: BTreeSet<String> = BTreeSet::new();
    for host in &w.sessions {
        if is_peer(host) {
            continue;
        }
        let peers: Vec<&SessionConfig> = w.children(&host.unique_identifier).into_iter().filter(|c| is_peer(c)).collect();
        if peers.is_empty() {
            continue;
        }
        need.insert(host.unique_identifier.clone());
        need.extend(peers.iter().map(|p| p.unique_identifier.clone()));
    }
    for s in &mut w.sessions {
        let has = s.toolbar_items.iter().any(|i| *i == ToolbarItem::ModeSwitcher);
        if need.contains(&s.unique_identifier) {
            if !has {
                s.toolbar_items.insert(0, ToolbarItem::ModeSwitcher);
            }
        } else if has {
            s.toolbar_items.retain(|i| *i != ToolbarItem::ModeSwitcher);
        }
    }
}

/// The workgroup without session `id` and everything under it; the root is never removed
/// (`WDV:422-446`).
pub fn remove_session(w: &Workgroup, id: &str) -> Workgroup {
    let mut out = w.clone();
    if out.root().is_none_or(|r| r.unique_identifier == id) || out.session(id).is_none() {
        return out;
    }
    let mut gone: BTreeSet<String> = BTreeSet::from([id.to_string()]);
    loop {
        let more: Vec<String> = out.sessions.iter().filter(|s| !gone.contains(&s.unique_identifier) && s.parent_id.as_ref().is_some_and(|p| gone.contains(p))).map(|s| s.unique_identifier.clone()).collect();
        if more.is_empty() {
            break;
        }
        gone.extend(more);
    }
    out.sessions.retain(|s| !gone.contains(&s.unique_identifier));
    enforce_mode_switcher(&mut out);
    out
}

// --- Toolbar items ---

const GIT_STATUS: &str = "Git Status";
const CHANGED_FILES: &str = "Changed File Selector";
const MODE_SWITCHER: &str = "Peer Mode Switcher";
const NAVIGATION: &str = "Navigation Buttons";
const RELOAD: &str = "Reload";
const GIT_BASE: &str = "Git Base Selector";
const AUTO_SEND: &str = "Auto-Send Clippings When Idle";
const AUTO_REQUEST: &str = "Auto-Request Review When Idle";
const USAGE: &str = "AI Usage";
const SPACER: &str = "Spacer";

/// The picker's order (`WTR:28-64`).
const REGISTRY: [&str; 10] = [GIT_STATUS, CHANGED_FILES, MODE_SWITCHER, NAVIGATION, RELOAD, GIT_BASE, AUTO_SEND, AUTO_REQUEST, USAGE, SPACER];

fn base_name(item: &ToolbarItem) -> &'static str {
    match item {
        ToolbarItem::GitStatus => GIT_STATUS,
        ToolbarItem::ChangedFileSelector => CHANGED_FILES,
        ToolbarItem::ModeSwitcher => MODE_SWITCHER,
        ToolbarItem::Navigation(_) => NAVIGATION,
        ToolbarItem::Reload(_) => RELOAD,
        ToolbarItem::GitBaseSelector => GIT_BASE,
        ToolbarItem::AutoSendClippingsWhenIdle => AUTO_SEND,
        ToolbarItem::AutoRequestReviewWhenIdle => AUTO_REQUEST,
        ToolbarItem::Usage { .. } => USAGE,
        ToolbarItem::Spacer { .. } => SPACER,
        ToolbarItem::Name => "Name",
    }
}

/// A width as iTerm2 prints it: a whole number without a decimal, else one decimal (`WSDV:1033`).
pub fn format_width(v: f64) -> String {
    if v == v.round() {
        format!("{}", v as i64)
    } else {
        format!("{v:.1}")
    }
}

/// The toolbar list's label for an item (`WSDV:1497-1508`).
pub fn item_label(item: &ToolbarItem) -> String {
    let base = base_name(item);
    match item {
        ToolbarItem::Spacer { min_width, max_width } => format!("{base} ({}\u{2013}{} pt)", format_width(*min_width), format_width(*max_width)),
        ToolbarItem::Usage { provider, .. } => format!("{base} ({})", provider.display_name()),
        _ => base.to_string(),
    }
}

/// The items the Add menu offers this session (`WSDV:1110-1160`). The peer mode switcher is left
/// out: the peer-group rule manages it (iTerm2 skips it in this menu although the registry has it).
pub fn offered_items(s: &SessionConfig) -> Vec<&'static str> {
    let has_files = s.toolbar_items.iter().any(|i| *i == ToolbarItem::ChangedFileSelector);
    REGISTRY
        .into_iter()
        .filter(|n| match *n {
            MODE_SWITCHER => false,
            NAVIGATION | GIT_BASE => has_files,
            AUTO_SEND => s.mode == Mode::CodeReview,
            AUTO_REQUEST => s.kind == Kind::Root,
            _ => true,
        })
        .collect()
}

/// The item a picked name adds (`WTR` default values).
pub fn default_item(name: &str) -> Option<ToolbarItem> {
    Some(match name {
        GIT_STATUS => ToolbarItem::GitStatus,
        CHANGED_FILES => ToolbarItem::ChangedFileSelector,
        MODE_SWITCHER => ToolbarItem::ModeSwitcher,
        NAVIGATION => ToolbarItem::Navigation(NavigationShortcuts::defaults()),
        RELOAD => ToolbarItem::Reload(Some(NavigationShortcuts::reload_default())),
        GIT_BASE => ToolbarItem::GitBaseSelector,
        AUTO_SEND => ToolbarItem::AutoSendClippingsWhenIdle,
        AUTO_REQUEST => ToolbarItem::AutoRequestReviewWhenIdle,
        USAGE => ToolbarItem::Usage { provider: UsageProvider::AnthropicClaudeCode, command: String::new(), interval_seconds: 60.0 },
        SPACER => ToolbarItem::Spacer { min_width: 4.0, max_width: 4.0 },
        _ => return None,
    })
}

/// The mode switcher that a peer group requires cannot be removed (`WSDV:1237`).
pub fn is_required_switcher(w: &Workgroup, s: &SessionConfig, index: usize) -> bool {
    if s.toolbar_items.get(index) != Some(&ToolbarItem::ModeSwitcher) {
        return false;
    }
    let host = if is_peer(s) { s.parent_id.clone() } else { Some(s.unique_identifier.clone()) };
    host.is_some_and(|h| hosts_peers(w, &h))
}

/// The smallest refresh interval of the usage item (`WSDV:1043`).
pub const MIN_USAGE_INTERVAL: f64 = 5.0;

// --- The session's rows ---

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Profile,
    Mode,
    Command,
    FileCommand,
    Url,
    Name,
    Shortcut,
    Split,
    Side,
    Location,
    ToolbarItems,
}

fn fields(s: &SessionConfig, in_group: bool, browser: bool) -> Vec<Field> {
    let root = s.kind == Kind::Root;
    let mut out = Vec::new();
    if !root {
        out.push(Field::Profile);
        if !browser {
            out.push(Field::Mode);
            out.push(Field::Command);
        }
    }
    if !root && !browser && s.toolbar_items.iter().any(|i| *i == ToolbarItem::ChangedFileSelector) {
        out.push(Field::FileCommand);
    }
    if !root && browser {
        out.push(Field::Url);
    }
    out.push(Field::Name);
    if in_group {
        out.push(Field::Shortcut);
    }
    if matches!(s.kind, Kind::Split(_)) {
        out.extend([Field::Split, Field::Side, Field::Location]);
    }
    out.push(Field::ToolbarItems);
    out
}

/// The rows a session shows (`WSDV:736-780`) judged from the session alone: a session with a URL
/// is a browser one, and only a peer is known to be in a peer group.
pub fn session_fields(s: &SessionConfig) -> Vec<Field> {
    fields(s, is_peer(s), !s.url_string.is_empty())
}

/// The same with the workgroup (a host of peers shows the shortcut row) and whether its profile
/// is a browser profile.
pub fn session_fields_in(w: &Workgroup, s: &SessionConfig, browser: bool) -> Vec<Field> {
    fields(s, in_peer_group(w, s), browser)
}

// --- Undo ---

/// The most snapshots kept (each is the whole list, itself bounded at a MiB).
pub const MAX_UNDO: usize = 50;

/// Undo and redo for the editor: each entry is the list as it was before an action, with the
/// action's name (`WEV:280-300`).
#[derive(Default)]
pub struct Undo {
    pub stack: Vec<(Vec<Workgroup>, &'static str)>,
    pub redo: Vec<(Vec<Workgroup>, &'static str)>,
    /// The action of the last coalescing push, while nothing else happened since.
    run: Option<&'static str>,
}

impl Undo {
    pub fn push(&mut self, before: Vec<Workgroup>, action: &'static str) {
        self.run = None;
        self.redo.clear();
        self.stack.push((before, action));
        if self.stack.len() > MAX_UNDO {
            self.stack.remove(0);
        }
    }

    /// Typing: a run of the same action is one undo step.
    pub fn push_coalesced(&mut self, before: Vec<Workgroup>, action: &'static str) {
        if self.run == Some(action) && self.stack.last().is_some_and(|(_, a)| *a == action) {
            return;
        }
        self.push(before, action);
        self.run = Some(action);
    }

    /// Ends a typing run (focus moved on).
    pub fn end_run(&mut self) {
        self.run = None;
    }

    pub fn undo_name(&self) -> Option<&'static str> {
        self.stack.last().map(|(_, a)| *a)
    }

    pub fn redo_name(&self) -> Option<&'static str> {
        self.redo.last().map(|(_, a)| *a)
    }

    pub fn undo(&mut self, now: Vec<Workgroup>) -> Option<Vec<Workgroup>> {
        let (before, action) = self.stack.pop()?;
        self.run = None;
        self.redo.push((now, action));
        Some(before)
    }

    pub fn redo(&mut self, now: Vec<Workgroup>) -> Option<Vec<Workgroup>> {
        let (after, action) = self.redo.pop()?;
        self.run = None;
        self.stack.push((now, action));
        Some(after)
    }
}

// --- The explanation text ---

/// `text` with iTerm2's markdown reduced to runs: (text, bold). Links keep their text and lose
/// their address (the editor never opens one).
pub fn plain_runs(md: &str) -> Vec<(String, bool)> {
    let mut flat = String::new();
    let mut rest = md;
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        match (after.find("]("), after.find(')')) {
            (Some(mid), Some(end)) if mid < end => {
                flat.push_str(&rest[..open]);
                flat.push_str(&after[..mid]);
                rest = &after[end + 1..];
            }
            _ => {
                flat.push_str(&rest[..=open]);
                rest = after;
            }
        }
    }
    flat.push_str(rest);
    flat.split("**").enumerate().filter(|(_, t)| !t.is_empty()).map(|(i, t)| (t.to_string(), i % 2 == 1)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workgroup::{claude_code, Kind, Mode, Orientation, Side, SplitSettings, ToolbarItem, UsageProvider, Workgroup};
    use std::collections::BTreeSet;

    fn cc() -> Workgroup {
        crate::workgroup::claude_code::config()
    }

    fn holds_switcher_invariant(w: &Workgroup) -> bool {
        w.sessions.iter().all(|s| {
            let host = if s.kind == Kind::Peer { s.parent_id.clone() } else { Some(s.unique_identifier.clone()) };
            let in_group = host.as_deref().is_some_and(|h| w.sessions.iter().any(|c| c.parent_id.as_deref() == Some(h) && c.kind == Kind::Peer));
            let has = s.toolbar_items.iter().any(|i| *i == ToolbarItem::ModeSwitcher);
            let first = s.toolbar_items.first() == Some(&ToolbarItem::ModeSwitcher);
            if in_group { has && (first || !s.toolbar_items.is_empty()) } else { !has }
        })
    }

    #[test]
    fn editor_rows_follow_iterm2() {
        let w = cc();
        let rows = tree_rows(&w);
        assert_eq!(
            rows,
            vec![
                (0, "Main session".to_string(), claude_code::MAIN.to_string()),
                (1, "Peer: Diff".to_string(), claude_code::DIFF.to_string()),
                (1, "Peer: Code Review".to_string(), claude_code::REVIEW.to_string()),
            ]
        );
        // Splits and tabs.
        let mut s = crate::workgroup::Workgroup::new_empty("x");
        let root = s.sessions[0].unique_identifier.clone();
        s = add_child(&s, &root, "split");
        let split = s.sessions[1].clone();
        assert_eq!(split.kind, Kind::Split(SplitSettings { orientation: Orientation::Vertical, side: Side::TrailingOrBottom, location: 0.5 }));
        assert_eq!(split.parent_id.as_deref(), Some(root.as_str()));
        assert_eq!(tree_rows(&s)[1].1, "Split: Vertical 50%");
        let mut t = s.clone();
        if let Kind::Split(ref mut sp) = t.sessions[1].kind {
            sp.orientation = Orientation::Horizontal;
            sp.location = 0.3;
        }
        assert_eq!(tree_rows(&t)[1].1, "Split: Horizontal 30%");
        // A tab always attaches to the root, even with a split selected.
        let t2 = add_child(&s, &split.unique_identifier, "tab");
        assert_eq!(t2.sessions[2].kind, Kind::Tab);
        assert_eq!(t2.sessions[2].parent_id.as_deref(), Some(root.as_str()));
        assert_eq!(tree_rows(&t2)[2].1, "Tab");

        // A peer on the template takes the first free animal and attaches under the root.
        let a = add_child(&w, claude_code::MAIN, "peer");
        let fox = a.sessions.last().unwrap();
        assert_eq!((fox.display_name.as_str(), fox.kind.clone(), fox.parent_id.as_deref()), ("Fox", Kind::Peer, Some(claude_code::MAIN)));
        assert!(fox.command.is_empty() && fox.toolbar_items.len() == 1 && fox.toolbar_items[0] == ToolbarItem::ModeSwitcher);
        // With a peer selected, the new peer joins that peer's host.
        let b = add_child(&a, claude_code::DIFF, "peer");
        let owl = b.sessions.last().unwrap();
        assert_eq!((owl.display_name.as_str(), owl.parent_id.as_deref()), ("Hen", Some(claude_code::MAIN)));
        assert!(holds_switcher_invariant(&b));
        // A split from inside a peer group is refused (the menu disables it too).
        assert_eq!(add_child(&w, claude_code::DIFF, "split"), w);
        assert_eq!(add_child(&w, claude_code::MAIN, "split"), w);
        // A new peer on an empty workgroup hands the root a mode switcher too.
        let e = crate::workgroup::Workgroup::new_empty("e");
        let e_root = e.sessions[0].unique_identifier.clone();
        let e2 = add_child(&e, &e_root, "peer");
        assert_eq!(e2.sessions[0].toolbar_items, vec![ToolbarItem::ModeSwitcher]);
        assert!(holds_switcher_invariant(&e2));

        // The add menu's enable rules.
        let main = w.session(claude_code::MAIN).unwrap();
        let plain = e.sessions[0].clone();
        assert_eq!(add_menu(&plain), vec![("Add Peer", true), ("Add Split", true), ("Add Tab", true)]);
        assert_eq!(add_menu(w.session(claude_code::DIFF).unwrap()), vec![("Add Peer", true), ("Add Split", false), ("Add Tab", true)]);
        assert_eq!(add_menu_in(&w, main)[1], ("Add Split", false), "the host of a peer group cannot split either");
        assert_eq!(add_menu_in(&e, &plain)[1], ("Add Split", true));

        // animal_name: the list in order, then "<name> 2".
        assert_eq!(ANIMALS.len(), 63);
        assert_eq!((ANIMALS[0], ANIMALS[1], ANIMALS[62]), ("Fox", "Hen", "Horse"));
        let mut taken: BTreeSet<String> = BTreeSet::new();
        assert_eq!(animal_name(&taken), "Fox");
        taken.insert("Fox".into());
        assert_eq!(animal_name(&taken), "Hen");
        taken.extend(ANIMALS.iter().map(|s| s.to_string()));
        assert_eq!(animal_name(&taken), "Fox 2");
        taken.insert("Fox 2".into());
        assert_eq!(animal_name(&taken), "Hen 2");
        taken.extend(ANIMALS.iter().map(|s| format!("{s} 2")));
        assert_eq!(animal_name(&taken), "Fox 3");

        // remove_session: never the root; a split takes its descendants.
        assert_eq!(remove_session(&w, claude_code::MAIN), w);
        let mut nested = s.clone();
        let split_id = nested.sessions[1].unique_identifier.clone();
        nested = add_child(&nested, &split_id, "peer");
        assert_eq!(nested.sessions.len(), 3);
        let gone = remove_session(&nested, &split_id);
        assert_eq!(gone.sessions.len(), 1);
        // Removing the last peer strips the switchers again.
        let one = remove_session(&remove_session(&w, claude_code::REVIEW), claude_code::DIFF);
        assert!(one.sessions[0].toolbar_items.iter().all(|i| *i != ToolbarItem::ModeSwitcher));
        // enforce_mode_switcher puts a missing one first and strips a stray one.
        let mut f = w.clone();
        f.sessions[1].toolbar_items.clear();
        f.sessions[0].toolbar_items.retain(|i| *i != ToolbarItem::ModeSwitcher);
        enforce_mode_switcher(&mut f);
        assert_eq!(f.sessions[1].toolbar_items, vec![ToolbarItem::ModeSwitcher]);
        assert_eq!(f.sessions[0].toolbar_items.first(), Some(&ToolbarItem::ModeSwitcher));
        // The limit is the stored one.
        let mut full = crate::workgroup::Workgroup::new_empty("f");
        let rid = full.sessions[0].unique_identifier.clone();
        for _ in 0..crate::workgroup::MAX_SESSIONS + 3 {
            full = add_child(&full, &rid, "tab");
        }
        assert_eq!(full.sessions.len(), crate::workgroup::MAX_SESSIONS);
        assert!(!can_add(&full));
    }

    #[test]
    fn offered_items_follow_iterm2() {
        let w = cc();
        let diff = w.session(claude_code::DIFF).unwrap().clone();
        let mut peer = diff.clone();
        peer.mode = Mode::Regular;
        peer.toolbar_items.clear();
        let base = ["Git Status", "Changed File Selector", "Reload", "AI Usage", "Spacer"];
        assert_eq!(offered_items(&peer), base);
        // With a changed-file selector it also gets Navigation Buttons and Git Base Selector.
        peer.toolbar_items = vec![ToolbarItem::ChangedFileSelector];
        assert_eq!(offered_items(&peer), ["Git Status", "Changed File Selector", "Navigation Buttons", "Reload", "Git Base Selector", "AI Usage", "Spacer"]);
        // Code Review peers get auto-send; only the root gets auto-request.
        let mut review = peer.clone();
        review.mode = Mode::CodeReview;
        assert!(offered_items(&review).contains(&"Auto-Send Clippings When Idle"));
        assert!(!offered_items(&peer).contains(&"Auto-Send Clippings When Idle"));
        assert!(!offered_items(&review).contains(&"Auto-Request Review When Idle"));
        let root = w.session(claude_code::MAIN).unwrap();
        assert!(offered_items(root).contains(&"Auto-Request Review When Idle"));
        // Labels.
        assert_eq!(item_label(&ToolbarItem::Spacer { min_width: 8.0, max_width: 100.0 }), "Spacer (8\u{2013}100 pt)");
        assert_eq!(item_label(&ToolbarItem::Spacer { min_width: 4.0, max_width: 12.5 }), "Spacer (4\u{2013}12.5 pt)");
        assert_eq!(item_label(&ToolbarItem::Usage { provider: UsageProvider::AnthropicClaudeCode, command: String::new(), interval_seconds: 60.0 }), "AI Usage (Anthropic (Claude Code))");
        assert_eq!(item_label(&ToolbarItem::ModeSwitcher), "Peer Mode Switcher");
        assert_eq!(item_label(&ToolbarItem::GitStatus), "Git Status");
        // Defaults a picked name adds.
        assert_eq!(default_item("Spacer"), Some(ToolbarItem::Spacer { min_width: 4.0, max_width: 4.0 }));
        assert_eq!(default_item("AI Usage"), Some(ToolbarItem::Usage { provider: UsageProvider::AnthropicClaudeCode, command: String::new(), interval_seconds: 60.0 }));
        assert_eq!(default_item("Reload"), Some(ToolbarItem::Reload(Some(crate::workgroup::NavigationShortcuts::reload_default()))));
        assert_eq!(default_item("nope"), None);
        // Rows a session shows.
        let f = |s: &crate::workgroup::SessionConfig| session_fields(s);
        assert_eq!(f(root), vec![Field::Name, Field::ToolbarItems]);
        assert_eq!(
            f(&review),
            vec![Field::Profile, Field::Mode, Field::Command, Field::FileCommand, Field::Name, Field::Shortcut, Field::ToolbarItems]
        );
        let mut tab = review.clone();
        tab.kind = Kind::Tab;
        tab.toolbar_items.clear();
        assert_eq!(f(&tab), vec![Field::Profile, Field::Mode, Field::Command, Field::Name, Field::ToolbarItems]);
        let mut split = tab.clone();
        split.kind = Kind::Split(SplitSettings { orientation: Orientation::Vertical, side: Side::TrailingOrBottom, location: 0.5 });
        assert_eq!(
            f(&split),
            vec![Field::Profile, Field::Mode, Field::Command, Field::Name, Field::Split, Field::Side, Field::Location, Field::ToolbarItems]
        );
        let mut browser = tab.clone();
        browser.url_string = "https://example.com".into();
        assert_eq!(f(&browser), vec![Field::Profile, Field::Url, Field::Name, Field::ToolbarItems]);
        assert_eq!(session_fields_in(&w, w.session(claude_code::MAIN).unwrap(), false), vec![Field::Name, Field::Shortcut, Field::ToolbarItems]);
        assert_eq!(session_fields_in(&w, &tab, true), vec![Field::Profile, Field::Url, Field::Name, Field::ToolbarItems]);
    }

    #[test]
    fn editor_undo_follows_iterm2() {
        let a = vec![Workgroup::new_empty("a")];
        let b = vec![Workgroup::new_empty("a"), Workgroup::new_empty("b")];
        let mut u = Undo::default();
        assert_eq!(u.undo(a.clone()), None);
        u.push(a.clone(), "Add Session");
        assert_eq!(u.undo_name(), Some("Add Session"));
        assert_eq!(u.undo(b.clone()), Some(a.clone()));
        assert_eq!(u.redo_name(), Some("Add Session"));
        assert_eq!(u.redo(a.clone()), Some(b.clone()));
        // A new push clears redo.
        assert_eq!(u.undo(b.clone()), Some(a.clone()));
        u.push(a.clone(), "Remove Session");
        assert_eq!(u.redo(b.clone()), None);
        // The stack is bounded.
        for _ in 0..500 {
            u.push(a.clone(), "Change Workgroups");
        }
        assert!(u.stack.len() <= MAX_UNDO);
        // Typing coalesces; another action ends the run.
        let mut u = Undo::default();
        u.push_coalesced(a.clone(), "Change Command");
        u.push_coalesced(b.clone(), "Change Command");
        assert_eq!(u.stack.len(), 1);
        u.push(b.clone(), "Rename Workgroup");
        u.push_coalesced(b.clone(), "Change Command");
        assert_eq!(u.stack.len(), 3);
        // Every action name the brief lists is a constant the UI uses.
        assert_eq!(ACTIONS.len(), 20);
    }

    #[test]
    fn what_is_text_is_iterm2s() {
        assert!(WHAT_IS_TEXT.starts_with("A **Workgroup** turns a single session"));
        assert_eq!(plain_runs("A **Workgroup** is [docs](https://x.y) ok"), vec![("A ".to_string(), false), ("Workgroup".to_string(), true), (" is docs ok".to_string(), false)]);
        assert_eq!((EMPTY_STATE, WHAT_IS, NAME_PLACEHOLDER, UNTITLED_WORKGROUP, ADD_PRESET), ("Select a workgroup on the left or click + to create one.", "What is a Workgroup?", "Workgroup name", "Untitled Workgroup", "Add Preset"));
    }
}

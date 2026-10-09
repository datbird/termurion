//! The Cockpit's outline (`CockpitWindowController`): windows, status buckets or tabs, and
//! sessions, built from a snapshot the app fills. Pure.

use crate::cockpit::{status_tint, GroupMode, Tint};
use crate::inert_files::shown_name;
use crate::layout::PaneId;
use crate::status_tool::{priority, Pattern};
use crate::tabstatus::Status;
use std::collections::BTreeSet;

pub const NO_STATUS: &str = "No status";
pub const BURIED: &str = "Buried Sessions";
pub const BURIED_SCOPE: &str = "<buried>";

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RowKey {
    /// The window's guid.
    Window(String),
    BuriedRoot,
    Workgroup(String),
    Tab(u64),
    /// A status bucket; `scope` is the window's guid or `BURIED_SCOPE`.
    Group { scope: String, status: String },
    Session(PaneId),
}

impl RowKey {
    /// Sessions, windows, tabs and workgroups can be chosen; buckets and the buried root cannot.
    pub fn selectable(&self) -> bool {
        matches!(self, RowKey::Session(_) | RowKey::Window(_) | RowKey::Tab(_) | RowKey::Workgroup(_))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionFact {
    pub pane: PaneId,
    pub stable_id: String,
    pub name: String,
    pub status: Status,
    pub exited: bool,
    pub alt_screen: bool,
    pub ancestors: Vec<String>,
    pub armed: bool,
    /// The workgroup role prefix of a member (`role_prefix`), None outside a workgroup.
    pub role: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TabFact {
    pub id: u64,
    pub title: String,
    pub sessions: Vec<SessionFact>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WindowFact {
    pub guid: String,
    pub number: usize,
    pub title: String,
    pub active_tab: usize,
    pub tabs: Vec<TabFact>,
    pub armed: bool,
}

/// One live member of a workgroup instance.
#[derive(Clone, Debug, PartialEq)]
pub struct MemberFact {
    pub pane: PaneId,
    pub stable_id: String,
    /// The member's config id (the role id the orchestration snapshot names).
    pub config: String,
    /// The role's display name (empty when the config has none).
    pub role: String,
    pub mode: crate::workgroup::Mode,
}

/// One live workgroup instance (`iTermWorkgroupInstance`): its id, its workgroup's name and
/// its live members, the leader first.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkgroupFact {
    pub id: String,
    pub name: String,
    pub members: Vec<MemberFact>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct World {
    pub windows: Vec<WindowFact>,
    pub buried: Vec<SessionFact>,
    /// The live instances in the order they were entered.
    pub workgroups: Vec<WorkgroupFact>,
    /// Members in no layout (a peer that is not shown): in no window, so only the workgroup
    /// outline and the orchestration snapshot list them.
    pub hidden: Vec<SessionFact>,
}

impl World {
    pub fn session(&self, pane: PaneId) -> Option<&SessionFact> {
        self.windows.iter().flat_map(|w| &w.tabs).flat_map(|t| &t.sessions).chain(&self.buried).chain(&self.hidden).find(|s| s.pane == pane)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub key: RowKey,
    pub title: String,
    pub detail: Option<String>,
    pub status: Option<String>,
    pub armed: bool,
    /// The status word's color (`status_tint`); secondary for rows that are not sessions.
    pub tint: Tint,
    pub children: Vec<Row>,
}

/// The bucket a status falls in: its trimmed text as written, else "No status".
pub fn status_key(st: &Status) -> String {
    match st.status.as_deref().map(str::trim) {
        Some(t) if !t.is_empty() => t.to_string(),
        _ => NO_STATUS.to_string(),
    }
}

/// Buckets by priority, then lower-cased text, then the raw text; "No status" last.
pub fn bucket_order(keys: &mut Vec<String>, patterns: &[Pattern]) {
    keys.sort_by(|a, b| {
        let rank = |k: &String| (k == NO_STATUS, priority(Some(k), patterns), k.to_lowercase(), k.clone());
        rank(a).cmp(&rank(b))
    });
}

/// A window's title: "Window N", or "Window N: title" (`CW:2649-2661`). A title that is empty
/// or only the prefix gives the prefix. Titles come from programs, so they are cleaned.
pub fn window_title(number: usize, title: &str) -> String {
    let prefix = format!("Window {number}");
    let title = shown_name(title);
    if title.is_empty() || title == prefix { prefix } else { format!("{prefix}: {title}") }
}

/// A tab's title, else "Tab N" by 1-based position (`CW:2732-2737`).
pub fn tab_title(title: &str, position: usize) -> String {
    let title = shown_name(title);
    if title.is_empty() { format!("Tab {position}") } else { title }
}

/// The role a member's title carries (`cockpitSessionRolePrefix`, `CW:2673-2707`): a root or
/// peer shows its display name when it has one; a split or tab only a mode other than Regular.
pub fn role_prefix(kind: &crate::workgroup::Kind, mode: crate::workgroup::Mode, display_name: &str) -> Option<String> {
    use crate::workgroup::{Kind, Mode};
    match kind {
        Kind::Root | Kind::Peer => Some(display_name.to_string()).filter(|n| !n.is_empty()),
        Kind::Split(_) | Kind::Tab => (mode != Mode::Regular).then(|| mode.title().to_string()),
    }
}

/// A session's name, else its id; a workgroup member's has its role in front, "<role>: <name>",
/// or the name alone when the role is the name (`CW:2673-2707`). Names come from programs, so
/// they are cleaned.
pub fn session_title(name: &str, pane: PaneId, role: Option<&str>) -> String {
    let name = shown_name(name);
    let base = if name.is_empty() { pane.to_string() } else { name };
    match role.map(shown_name).filter(|r| !r.is_empty() && *r != base) {
        Some(r) => format!("{r}: {base}"),
        None => base,
    }
}

fn title_of(s: &SessionFact) -> String {
    session_title(&s.name, s.pane, s.role.as_deref())
}

/// A workgroup row's title: the workgroup's name, else its instance id (`CW:2743-2746`).
pub fn workgroup_title(w: &WorkgroupFact) -> String {
    let name = shown_name(&w.name);
    if name.is_empty() { shown_name(&w.id) } else { name }
}

fn matches(s: &SessionFact, search: &str, filter: Option<&str>) -> bool {
    (search.is_empty() || title_of(s).to_lowercase().contains(&search.to_lowercase())) && filter.is_none_or(|f| status_key(&s.status) == f)
}

fn session_row(s: &SessionFact) -> Row {
    let key = status_key(&s.status);
    Row { key: RowKey::Session(s.pane), title: title_of(s), detail: s.status.detail.clone(), status: (key != NO_STATUS).then_some(key), armed: s.armed, tint: status_tint(&s.status), children: Vec::new() }
}

/// Status buckets over `sessions`: each present bucket in order, empty ones left out.
fn buckets(scope: &str, sessions: &[&SessionFact], patterns: &[Pattern]) -> Vec<Row> {
    let mut keys: Vec<String> = Vec::new();
    for s in sessions {
        let k = status_key(&s.status);
        if !keys.contains(&k) {
            keys.push(k);
        }
    }
    bucket_order(&mut keys, patterns);
    keys.into_iter()
        .map(|k| {
            let kids: Vec<Row> = sessions.iter().filter(|s| status_key(&s.status) == k).map(|s| session_row(s)).collect();
            Row { key: RowKey::Group { scope: scope.to_string(), status: k.clone() }, title: format!("{} \u{00B7} {}", shown_name(&k), kids.len()), detail: None, status: None, armed: false, tint: Tint::Secondary, children: kids }
        })
        .collect()
}

fn container(key: RowKey, title: String, armed: bool, children: Vec<Row>) -> Row {
    Row { key, title, detail: None, status: None, armed, tint: Tint::Secondary, children }
}

/// The outline.
pub fn build(mode: GroupMode, world: &World, patterns: &[Pattern], search: &str, status_filter: Option<&str>) -> Vec<Row> {
    let keep = |s: &&SessionFact| matches(s, search, status_filter);
    let mut rows = Vec::new();
    match mode {
        // Only sessions in a live workgroup are listed, each instance with its live members
        // (`CW:2151-2172`); an instance with none the search keeps is left out.
        GroupMode::Workgroup => {
            for w in &world.workgroups {
                let kids: Vec<Row> = w.members.iter().filter_map(|m| world.session(m.pane)).filter(keep).map(session_row).collect();
                if !kids.is_empty() {
                    rows.push(container(RowKey::Workgroup(w.id.clone()), workgroup_title(w), false, kids));
                }
            }
        }
        GroupMode::Status => {
            for w in &world.windows {
                let all: Vec<&SessionFact> = w.tabs.iter().flat_map(|t| &t.sessions).filter(keep).collect();
                if !all.is_empty() {
                    rows.push(container(RowKey::Window(w.guid.clone()), window_title(w.number, &w.title), w.armed, buckets(&w.guid, &all, patterns)));
                }
            }
            let buried: Vec<&SessionFact> = world.buried.iter().filter(keep).collect();
            if !buried.is_empty() {
                rows.push(container(RowKey::BuriedRoot, BURIED.to_string(), false, buckets(BURIED_SCOPE, &buried, patterns)));
            }
        }
        GroupMode::Window => {
            for w in &world.windows {
                let mut tabs = Vec::new();
                for (i, t) in w.tabs.iter().enumerate() {
                    let kids: Vec<Row> = t.sessions.iter().filter(keep).map(session_row).collect();
                    if !kids.is_empty() {
                        tabs.push(container(RowKey::Tab(t.id), tab_title(&t.title, i + 1), false, kids));
                    }
                }
                if !tabs.is_empty() {
                    rows.push(container(RowKey::Window(w.guid.clone()), window_title(w.number, &w.title), w.armed, tabs));
                }
            }
            let buried: Vec<Row> = world.buried.iter().filter(keep).map(session_row).collect();
            if !buried.is_empty() {
                rows.push(container(RowKey::BuriedRoot, BURIED.to_string(), false, buried));
            }
        }
    }
    rows
}

/// The total of sessions the search keeps and each present status in bucket order.
pub fn counts(mode: GroupMode, world: &World, patterns: &[Pattern], search: &str) -> (usize, Vec<(String, usize)>) {
    let all: Vec<&SessionFact> = if mode == GroupMode::Workgroup {
        world.workgroups.iter().flat_map(|w| &w.members).filter_map(|m| world.session(m.pane)).filter(|s| matches(s, search, None)).collect()
    } else {
        world.windows.iter().flat_map(|w| &w.tabs).flat_map(|t| &t.sessions).chain(&world.buried).filter(|s| matches(s, search, None)).collect()
    };
    let mut keys: Vec<String> = Vec::new();
    for s in &all {
        let k = status_key(&s.status);
        if !keys.contains(&k) {
            keys.push(k);
        }
    }
    bucket_order(&mut keys, patterns);
    let per = keys.into_iter().map(|k| (all.iter().filter(|s| status_key(&s.status) == k).count(), k)).map(|(n, k)| (k, n)).collect();
    (all.len(), per)
}

fn collect(row: &Row, out: &mut Vec<PaneId>) {
    if let RowKey::Session(p) = row.key {
        if !out.contains(&p) {
            out.push(p);
        }
    }
    for c in &row.children {
        collect(c, out);
    }
}

fn find<'a>(rows: &'a [Row], key: &RowKey) -> Option<&'a Row> {
    rows.iter().find_map(|r| if &r.key == key { Some(r) } else { find(&r.children, key) })
}

/// Every session at or below the rows named, each once, in the order the keys name them.
pub fn sessions_under(rows: &[Row], keys: &[RowKey]) -> Vec<PaneId> {
    let mut out = Vec::new();
    for k in keys {
        if let Some(r) = find(rows, k) {
            collect(r, &mut out);
        }
    }
    out
}

pub fn all_sessions(rows: &[Row]) -> Vec<PaneId> {
    let mut out = Vec::new();
    for r in rows {
        collect(r, &mut out);
    }
    out
}

/// A row shows its children unless collapsed; a filter expands everything.
pub fn expanded(key: &RowKey, collapsed: &BTreeSet<RowKey>, filtering: bool) -> bool {
    filtering || !collapsed.contains(key)
}

/// The rows the list shows, top to bottom, each with its depth: a collapsed row hides its
/// children unless `filtering`.
pub fn flatten<'a>(rows: &'a [Row], collapsed: &BTreeSet<RowKey>, filtering: bool) -> Vec<(usize, &'a Row)> {
    fn walk<'a>(rows: &'a [Row], depth: usize, collapsed: &BTreeSet<RowKey>, filtering: bool, out: &mut Vec<(usize, &'a Row)>) {
        for r in rows {
            out.push((depth, r));
            if expanded(&r.key, collapsed, filtering) {
                walk(&r.children, depth + 1, collapsed, filtering, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(rows, 0, collapsed, filtering, &mut out);
    out
}

/// The selected rows that are still in the outline.
pub fn keep_selection(selected: &[RowKey], rows: &[Row]) -> Vec<RowKey> {
    selected.iter().filter(|k| find(rows, k).is_some()).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status_tool::load_patterns;

    fn fact(pane: PaneId, status: Option<&str>, name: &str) -> SessionFact {
        let st = Status { status: status.map(str::to_string), indicator: (pane == 1).then(|| "#5f87ff".into()), ..Default::default() };
        SessionFact { pane, stable_id: String::new(), name: name.into(), status: st, exited: false, alt_screen: false, ancestors: Vec::new(), armed: false, role: None }
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
            ..Default::default()
        }
    }

    fn pats() -> Vec<Pattern> {
        load_patterns(None, None)
    }

    fn shape(rows: &[Row]) -> Vec<(String, Vec<(String, Vec<PaneId>)>)> {
        rows.iter().map(|w| (w.title.clone(), w.children.iter().map(|g| (g.title.clone(), all_sessions(std::slice::from_ref(g)))).collect())).collect()
    }

    #[test]
    fn status_mode_follows_iterm2() {
        let rows = build(GroupMode::Status, &world(), &pats(), "", None);
        let g = |t: &str, p: &[PaneId]| (t.to_string(), p.to_vec());
        assert_eq!(
            shape(&rows),
            vec![
                ("Window 1".to_string(), vec![g("Waiting \u{00B7} 1", &[1]), g("working \u{00B7} 1", &[3]), g("No status \u{00B7} 1", &[2])]),
                ("Window 2: Logs".to_string(), vec![g("idle \u{00B7} 1", &[4])]),
                ("Buried Sessions".to_string(), vec![g("Waiting \u{00B7} 1", &[9])]),
            ]
        );
        assert_eq!(rows[2].children[0].key, RowKey::Group { scope: BURIED_SCOPE.into(), status: "Waiting".into() });
        assert!(!rows[0].children[0].key.selectable() && !rows[2].key.selectable());
        assert!(rows[0].key.selectable() && rows[0].children[0].children[0].key.selectable());
        let mut busy = world();
        busy.windows[0].tabs[0].sessions = vec![fact(1, Some("busy"), ""), fact(2, Some("Busy"), "")];
        let rows = build(GroupMode::Status, &busy, &pats(), "", None);
        let titles: Vec<&str> = rows[0].children.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(titles, ["working \u{00B7} 1", "Busy \u{00B7} 1", "busy \u{00B7} 1"]);
    }

    #[test]
    fn window_mode_follows_iterm2() {
        let rows = build(GroupMode::Window, &world(), &pats(), "", None);
        assert_eq!(rows[0].title, "Window 1");
        assert_eq!(rows[0].children.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["Tab 1", "build"]);
        assert_eq!(all_sessions(&rows[0].children[0..1]), vec![1, 2]);
        assert_eq!(all_sessions(&rows[0].children[1..2]), vec![3]);
        assert_eq!(rows[1].title, "Window 2: Logs");
        assert_eq!(rows[1].children[0].title, "Tab 1");
        assert_eq!(rows[2].title, "Buried Sessions");
        assert_eq!(rows[2].children.iter().map(|r| r.key.clone()).collect::<Vec<_>>(), vec![RowKey::Session(9)]);
    }

    fn member(pane: PaneId, config: &str, role: &str) -> MemberFact {
        MemberFact { pane, stable_id: String::new(), config: config.into(), role: role.into(), mode: crate::workgroup::Mode::Regular }
    }

    /// Two instances (the second named by its id) and a session in none.
    fn grouped() -> World {
        let mut w = world();
        w.windows[0].tabs[0].sessions[0].role = Some("Chat".into());
        w.hidden = vec![SessionFact { role: Some("Diff".into()), ..fact(5, Some("idle"), "git difftool")}];
        w.workgroups = vec![
            WorkgroupFact { id: "wg-A".into(), name: "Claude Code".into(), members: vec![member(1, "MAIN", "Chat"), member(5, "DIFF", "Diff")] },
            WorkgroupFact { id: "wg-B".into(), name: String::new(), members: vec![member(4, "ONLY", "")] },
            WorkgroupFact { id: "wg-C".into(), name: "Empty".into(), members: Vec::new() },
            WorkgroupFact { id: "wg-D".into(), name: "Gone".into(), members: vec![member(77, "X", "")] },
        ];
        w
    }

    /// `CW:2151-2172`: Workgroup mode lists live instances with their live members, in instance
    /// order; an instance with none is left out; sessions in no workgroup are not listed.
    #[test]
    fn cockpit_workgroups_follow_iterm2() {
        let rows = build(GroupMode::Workgroup, &grouped(), &pats(), "", None);
        let shape: Vec<(RowKey, String, Vec<(RowKey, String)>)> = rows.iter().map(|r| (r.key.clone(), r.title.clone(), r.children.iter().map(|c| (c.key.clone(), c.title.clone())).collect())).collect();
        assert_eq!(
            shape,
            vec![
                (RowKey::Workgroup("wg-A".into()), "Claude Code".to_string(), vec![(RowKey::Session(1), "Chat: 1".to_string()), (RowKey::Session(5), "Diff: git difftool".to_string())]),
                (RowKey::Workgroup("wg-B".into()), "wg-B".to_string(), vec![(RowKey::Session(4), "tail logs".to_string())]),
            ],
            "the unnamed one is titled by its id; empty and gone instances are omitted"
        );
        assert!(rows[0].key.selectable(), "a workgroup row can be chosen for the command bar");
        assert_eq!(all_sessions(&rows), vec![1, 5, 4], "sessions 2, 3 and 9 are in no workgroup");
        assert_eq!(sessions_under(&rows, &[RowKey::Workgroup("wg-A".into())]), vec![1, 5]);
        // The filter counts follow the rows.
        assert_eq!(counts(GroupMode::Workgroup, &grouped(), &pats(), ""), (3, vec![("Waiting".to_string(), 1), ("idle".to_string(), 2)]));
        let waiting = build(GroupMode::Workgroup, &grouped(), &pats(), "", Some("Waiting"));
        assert_eq!(all_sessions(&waiting), vec![1]);
        assert_eq!(waiting.len(), 1, "a group with nothing the filter keeps goes");
        // A search keeps ancestors and matches the title as shown, role included.
        let found = build(GroupMode::Workgroup, &grouped(), &pats(), "diff:", None);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].title.as_str(), all_sessions(&found)), ("Claude Code", vec![5]));
        assert!(build(GroupMode::Workgroup, &grouped(), &pats(), "nothing like it", None).is_empty());
        // No workgroups: nothing, and the other modes do not list a hidden member.
        assert!(build(GroupMode::Workgroup, &world(), &pats(), "", None).is_empty());
        assert!(!all_sessions(&build(GroupMode::Window, &grouped(), &pats(), "", None)).contains(&5));
        // A member's role is in its title in every mode.
        let status = build(GroupMode::Status, &grouped(), &pats(), "", None);
        assert!(status.iter().flat_map(|w| &w.children).flat_map(|g| &g.children).any(|s| s.title == "Chat: 1"));
    }

    /// `CW:2673-2707`.
    #[test]
    fn role_prefix_follows_iterm2() {
        use crate::workgroup::{Kind, Mode, Orientation, Side, SplitSettings};
        let split = Kind::Split(SplitSettings { orientation: Orientation::Vertical, side: Side::TrailingOrBottom, location: 0.5 });
        assert_eq!(role_prefix(&Kind::Root, Mode::Regular, "Chat").as_deref(), Some("Chat"));
        assert_eq!(role_prefix(&Kind::Peer, Mode::Diff, "").as_deref(), None, "a peer with an empty display name");
        assert_eq!(role_prefix(&Kind::Peer, Mode::Diff, "Review").as_deref(), Some("Review"), "a peer shows its role, not its mode");
        assert_eq!(role_prefix(&split, Mode::Diff, "whatever").as_deref(), Some("Diff"));
        assert_eq!(role_prefix(&split, Mode::CodeReview, "").as_deref(), Some("Code Review"));
        assert_eq!(role_prefix(&split, Mode::Regular, "Named"), None);
        assert_eq!(role_prefix(&Kind::Tab, Mode::Regular, "Named"), None, "a Regular tab");
        assert_eq!(role_prefix(&Kind::Tab, Mode::Diff, "Named").as_deref(), Some("Diff"));
        assert_eq!(session_title("zsh", 3, Some("Diff")), "Diff: zsh");
        assert_eq!(session_title("Diff", 3, Some("Diff")), "Diff", "the name alone when the role is the name");
        assert_eq!(session_title("", 3, None), "3", "the id fallback");
        assert_eq!(session_title("", 3, Some("Diff")), "Diff: 3");
        assert_eq!(session_title("zsh", 3, Some("")), "zsh");
        assert_eq!(session_title("a\u{202e}b", 3, Some("D\u{7}iff")), "Diff: ab", "program text and the role are cleaned");
    }

    #[test]
    fn search_follows_iterm2() {
        let rows = build(GroupMode::Status, &world(), &pats(), "LOG", None);
        assert_eq!(shape(&rows), vec![("Window 2: Logs".to_string(), vec![("idle \u{00B7} 1".to_string(), vec![4])])]);
        let k = RowKey::Tab(12);
        let collapsed: BTreeSet<RowKey> = [k.clone()].into();
        assert!(expanded(&k, &collapsed, true) && !expanded(&k, &collapsed, false));
        let rows = build(GroupMode::Status, &world(), &pats(), "", Some("Waiting"));
        assert_eq!(all_sessions(&rows), vec![1, 9]);
        assert_eq!(rows.len(), 2);
        let want = vec![("Waiting".to_string(), 2), ("working".to_string(), 1), ("idle".to_string(), 1), ("No status".to_string(), 1)];
        assert_eq!(counts(GroupMode::Status, &world(), &pats(), ""), (5, want));
    }

    #[test]
    fn refresh_keeps_expansion_and_selection() {
        let rows = build(GroupMode::Window, &world(), &pats(), "", None);
        let kept = keep_selection(&[RowKey::Session(3), RowKey::Session(77), RowKey::Tab(12)], &rows);
        assert_eq!(kept, vec![RowKey::Session(3), RowKey::Tab(12)]);
        assert_eq!(sessions_under(&rows, &[RowKey::Window("pty-A".into())]), vec![1, 2, 3]);
        let status = build(GroupMode::Status, &world(), &pats(), "", None);
        assert_eq!(all_sessions(&status), vec![1, 3, 2, 4, 9]);
    }

    #[test]
    fn row_titles_follow_iterm2() {
        assert_eq!(window_title(1, ""), "Window 1");
        assert_eq!(window_title(1, "Window 1"), "Window 1");
        assert_eq!(window_title(2, "Logs"), "Window 2: Logs");
        assert_eq!(tab_title("", 3), "Tab 3");
        assert_eq!(tab_title("build", 3), "build");
        assert_eq!(session_title("", 42, None), "42");
        assert_eq!(session_title("vim", 42, None), "vim");
        // Names come from programs and remotes: shown through the one cleaning helper.
        assert_eq!(session_title("a\u{202e}b\u{7}c", 1, None), "abc");
        assert_eq!(window_title(3, "x\u{200b}y"), "Window 3: xy");
        assert_eq!(tab_title("t\u{1b}[31mq", 1), "t[31mq");
    }

    #[test]
    fn flatten_follows_expansion() {
        let rows = build(GroupMode::Window, &world(), &pats(), "", None);
        let keys = |v: &[(usize, &Row)]| v.iter().map(|(d, r)| (*d, r.key.clone())).collect::<Vec<_>>();
        let all = flatten(&rows, &BTreeSet::new(), false);
        assert_eq!(all.len(), 11);
        assert_eq!(keys(&all)[0], (0, RowKey::Window("pty-A".into())));
        assert_eq!(keys(&all)[1], (1, RowKey::Tab(11)));
        assert_eq!(keys(&all)[2], (2, RowKey::Session(1)));
        let collapsed: BTreeSet<RowKey> = [RowKey::Tab(11)].into();
        let some = flatten(&rows, &collapsed, false);
        assert!(!keys(&some).iter().any(|(_, k)| *k == RowKey::Session(1)));
        assert!(keys(&some).iter().any(|(_, k)| *k == RowKey::Session(3)));
        // A search expands everything.
        assert!(keys(&flatten(&rows, &collapsed, true)).iter().any(|(_, k)| *k == RowKey::Session(1)));
    }
}

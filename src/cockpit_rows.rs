//! The Cockpit's outline (`CockpitWindowController`): windows, status buckets or tabs, and
//! sessions, built from a snapshot the app fills. Pure.

use crate::cockpit::GroupMode;
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

#[derive(Clone, Debug, Default, PartialEq)]
pub struct World {
    pub windows: Vec<WindowFact>,
    pub buried: Vec<SessionFact>,
}

impl World {
    pub fn session(&self, pane: PaneId) -> Option<&SessionFact> {
        self.windows.iter().flat_map(|w| &w.tabs).flat_map(|t| &t.sessions).chain(&self.buried).find(|s| s.pane == pane)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub key: RowKey,
    pub title: String,
    pub detail: Option<String>,
    pub status: Option<String>,
    pub armed: bool,
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

fn title_of(s: &SessionFact) -> String {
    if s.name.is_empty() { s.pane.to_string() } else { s.name.clone() }
}

fn matches(s: &SessionFact, search: &str, filter: Option<&str>) -> bool {
    (search.is_empty() || title_of(s).to_lowercase().contains(&search.to_lowercase())) && filter.is_none_or(|f| status_key(&s.status) == f)
}

fn session_row(s: &SessionFact) -> Row {
    let key = status_key(&s.status);
    Row { key: RowKey::Session(s.pane), title: title_of(s), detail: s.status.detail.clone(), status: (key != NO_STATUS).then_some(key), armed: s.armed, children: Vec::new() }
}

fn window_title(w: &WindowFact) -> String {
    if w.title.is_empty() { format!("Window {}", w.number) } else { format!("Window {}: {}", w.number, w.title) }
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
            Row { key: RowKey::Group { scope: scope.to_string(), status: k.clone() }, title: format!("{k} \u{00B7} {}", kids.len()), detail: None, status: None, armed: false, children: kids }
        })
        .collect()
}

fn container(key: RowKey, title: String, armed: bool, children: Vec<Row>) -> Row {
    Row { key, title, detail: None, status: None, armed, children }
}

/// The outline. Workgroup grouping is empty until milestone 13.
pub fn build(mode: GroupMode, world: &World, patterns: &[Pattern], search: &str, status_filter: Option<&str>) -> Vec<Row> {
    let keep = |s: &&SessionFact| matches(s, search, status_filter);
    let mut rows = Vec::new();
    match mode {
        GroupMode::Workgroup => return rows,
        GroupMode::Status => {
            for w in &world.windows {
                let all: Vec<&SessionFact> = w.tabs.iter().flat_map(|t| &t.sessions).filter(keep).collect();
                if !all.is_empty() {
                    rows.push(container(RowKey::Window(w.guid.clone()), window_title(w), w.armed, buckets(&w.guid, &all, patterns)));
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
                        let title = if t.title.is_empty() { format!("Tab {}", i + 1) } else { t.title.clone() };
                        tabs.push(container(RowKey::Tab(t.id), title, false, kids));
                    }
                }
                if !tabs.is_empty() {
                    rows.push(container(RowKey::Window(w.guid.clone()), window_title(w), w.armed, tabs));
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
    if mode == GroupMode::Workgroup {
        return (0, Vec::new());
    }
    let all: Vec<&SessionFact> = world.windows.iter().flat_map(|w| &w.tabs).flat_map(|t| &t.sessions).chain(&world.buried).filter(|s| matches(s, search, None)).collect();
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
        SessionFact { pane, stable_id: String::new(), name: name.into(), status: st, exited: false, alt_screen: false, ancestors: Vec::new(), armed: false }
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

    #[test]
    fn workgroup_mode_is_empty() {
        assert!(build(GroupMode::Workgroup, &world(), &pats(), "", None).is_empty());
        assert_eq!(counts(GroupMode::Workgroup, &world(), &pats(), ""), (0, Vec::new()));
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
}

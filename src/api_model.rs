//! What `async_get_app` reads: a pure snapshot of Tandem's windows, tabs, sessions, focus and
//! broadcast domains; iTerm2's ListSessions (`H:2372-2433`, `TAB:6083-6122`), Focus
//! (`H:3338-3380`) and GetBroadcastDomains (`H:1455-1478`, `H:3598-3608`) answers built from
//! it; and the difference of two snapshots as NEW_SESSION, TERMINATE_SESSION, LAYOUT_CHANGE,
//! FOCUS_CHANGE and BROADCAST_CHANGE notifications (`H:1000-1133`).

use crate::api_proto::{self, pb};
use crate::broadcast::{Broadcast, Mode, View};
use crate::layout::{Axis, PaneId, Tree};
use pb::client_originated_message::Submessage as Req;
use pb::focus_changed_notification::window::WindowStatus;
use pb::focus_changed_notification::Event as Focus;
use pb::server_originated_message::Submessage as Resp;

/// A session: its id (the pane id), its frame in window pixels and its grid (None for buried
/// and minimized sessions), and its name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiSession {
    pub id: String,
    pub frame: Option<(i32, i32, i32, i32)>,
    pub grid: Option<(i32, i32)>,
    pub title: String,
}

/// iTerm2's split view tree: `vertical` is a side-by-side split (a vertical divider).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApiNode {
    Leaf(ApiSession),
    Split { vertical: bool, children: Vec<ApiNode> },
}

/// A tab: its stable id, its split tree, its active session, its tmux window ("-1" outside
/// tmux) and gateway client name, and the sessions a maximized pane hides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiTab {
    pub id: u64,
    pub root: ApiNode,
    pub active: Option<String>,
    pub tmux_window: String,
    pub tmux_connection: Option<String>,
    pub minimized: Vec<ApiSession>,
}

/// A terminal window: its `pty-` id, outer frame, number (from 0, as ListSessions sends it),
/// selected tab, tabs, and its broadcast domains as session ids.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiWindow {
    pub id: String,
    pub frame: (i32, i32, i32, i32),
    pub number: i32,
    pub selected_tab: Option<u64>,
    pub tabs: Vec<ApiTab>,
    pub domains: Vec<Vec<String>>,
}

/// Everything the three answers and the notifications read. Windows are in number order.
/// `key_window` is the terminal window with the keyboard while Tandem is the active app;
/// `current_window` is the terminal window used last.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Model {
    pub windows: Vec<ApiWindow>,
    pub buried: Vec<ApiSession>,
    pub app_active: bool,
    pub key_window: Option<String>,
    pub current_window: Option<String>,
}

/// A tab's split tree as iTerm2's split views: a chain of same-axis binary splits is one
/// split view, a lone pane is a vertical split view holding it (`TAB:3108-3111`), and a
/// maximized pane is the only one in the tree, the others minimized (in tree order).
pub fn node_of(tree: &Tree<PaneId>, zoomed: Option<PaneId>, session: &dyn Fn(PaneId) -> ApiSession) -> (ApiNode, Vec<ApiSession>) {
    let lone = |p: PaneId| ApiNode::Split { vertical: true, children: vec![ApiNode::Leaf(session(p))] };
    let leaves: Vec<PaneId> = tree.leaves().into_iter().copied().collect();
    if let Some(z) = zoomed.filter(|z| leaves.contains(z)) {
        return (lone(z), leaves.into_iter().filter(|p| *p != z).map(session).collect());
    }
    let root = match tree {
        Tree::Leaf(p) => lone(*p),
        split => view(split, session),
    };
    (root, Vec::new())
}

/// One split view (or a session view for a leaf).
fn view(tree: &Tree<PaneId>, session: &dyn Fn(PaneId) -> ApiSession) -> ApiNode {
    match tree {
        Tree::Leaf(p) => ApiNode::Leaf(session(*p)),
        Tree::Split { axis, .. } => {
            let mut children = Vec::new();
            flatten(tree, *axis, session, &mut children);
            ApiNode::Split { vertical: *axis == Axis::Row, children }
        }
    }
}

/// The children of a split view along `axis`: nested splits on the same axis join it.
fn flatten(tree: &Tree<PaneId>, axis: Axis, session: &dyn Fn(PaneId) -> ApiSession, out: &mut Vec<ApiNode>) {
    match tree {
        Tree::Split { axis: a, a: first, b: second, .. } if *a == axis => {
            flatten(first, axis, session, out);
            flatten(second, axis, session, out);
        }
        other => out.push(view(other, session)),
    }
}

/// A window's broadcast domains, iTerm2's per-window rule (`H:1455-1478`): with broadcast off,
/// each broadcasting tab's panes; all tabs, every pane of the window; all panes, the current
/// tab's; custom, only the current domain (`PseudoTerminal.m:3182-3191`), empty when the
/// current pane is in none.
pub fn domains(b: &Broadcast, v: &View) -> Vec<Vec<PaneId>> {
    match b.mode(v) {
        Mode::Off => v.tabs.iter().filter(|t| t.broadcasting).map(|t| t.panes.clone()).collect(),
        Mode::AllTabs => vec![v.tabs.iter().flat_map(|t| t.panes.iter().copied()).collect()],
        Mode::AllPanes => vec![v.tabs.get(v.current_tab).map(|t| t.panes.clone()).unwrap_or_default()],
        Mode::Custom => vec![b.domain_panes(v)],
    }
}

fn frame(f: (i32, i32, i32, i32)) -> pb::Frame {
    pb::Frame { origin: Some(pb::Point { x: Some(f.0), y: Some(f.1) }), size: Some(pb::Size { width: Some(f.2), height: Some(f.3) }) }
}

/// A session in a split tree: id, frame, grid size and name.
fn summary(s: &ApiSession) -> pb::SessionSummary {
    pb::SessionSummary {
        unique_identifier: Some(s.id.clone()),
        frame: s.frame.map(frame),
        grid_size: s.grid.map(|(w, h)| pb::Size { width: Some(w), height: Some(h) }),
        title: Some(s.title.clone()),
    }
}

/// A buried or minimized session: id and name only (`H:2398-2404`, `H:2425-2430`).
fn brief_summary(s: &ApiSession) -> pb::SessionSummary {
    pb::SessionSummary { unique_identifier: Some(s.id.clone()), frame: None, grid_size: None, title: Some(s.title.clone()) }
}

fn split_node(n: &ApiNode) -> pb::SplitTreeNode {
    use pb::split_tree_node::split_tree_link::Child;
    use pb::split_tree_node::SplitTreeLink;
    match n {
        ApiNode::Split { vertical, children } => pb::SplitTreeNode {
            vertical: Some(*vertical),
            links: children
                .iter()
                .map(|c| SplitTreeLink {
                    child: Some(match c {
                        ApiNode::Leaf(s) => Child::Session(summary(s)),
                        split => Child::Node(split_node(split)),
                    }),
                })
                .collect(),
        },
        // A root is always a split view; a bare leaf gets one.
        ApiNode::Leaf(_) => split_node(&ApiNode::Split { vertical: true, children: vec![n.clone()] }),
    }
}

/// iTerm2's ListSessions (`H:2372-2433`): windows with id, frame, number and selected tab;
/// tabs with id, split tree, active session, `tmux_window_id` always set, the connection only
/// for tmux tabs, minimized sessions; buried sessions. No tab group fields (Tandem has no tab
/// groups).
pub fn list_sessions(m: &Model) -> pb::ListSessionsResponse {
    use pb::list_sessions_response::{Tab, Window};
    let windows = m
        .windows
        .iter()
        .map(|w| Window {
            window_id: Some(w.id.clone()),
            frame: Some(frame(w.frame)),
            number: Some(w.number),
            selected_tab_id: w.selected_tab.map(|t| t.to_string()),
            tabs: w
                .tabs
                .iter()
                .map(|t| Tab {
                    tab_id: Some(t.id.to_string()),
                    root: Some(split_node(&t.root)),
                    active_session_id: t.active.clone(),
                    minimized_sessions: t.minimized.iter().map(brief_summary).collect(),
                    tmux_window_id: Some(t.tmux_window.clone()),
                    tmux_connection_id: t.tmux_connection.clone(),
                    ..Default::default()
                })
                .collect(),
        })
        .collect();
    pb::ListSessionsResponse { windows, buried_sessions: m.buried.iter().map(brief_summary).collect() }
}

fn focus_event(e: Focus) -> pb::FocusChangedNotification {
    pb::FocusChangedNotification { event: Some(e) }
}

fn window_focus(status: WindowStatus, id: Option<&str>) -> pb::FocusChangedNotification {
    focus_event(Focus::Window(pb::focus_changed_notification::Window { window_status: Some(status as i32), window_id: id.map(str::to_string) }))
}

/// iTerm2's Focus (`H:3338-3380`): whether the app is active; the current terminal window,
/// BECAME_KEY when it is the key window, IS_CURRENT when it is not, RESIGNED_KEY with no id
/// when there is none; then for each window with a selected tab, that tab and each of its
/// tabs' active session (a tab with none is skipped, never sent as an empty id).
pub fn focus(m: &Model) -> pb::FocusResponse {
    let mut out = vec![focus_event(Focus::ApplicationActive(m.app_active))];
    let current = m.current_window.as_deref();
    out.push(match current {
        Some(c) if m.key_window.as_deref() == Some(c) => window_focus(WindowStatus::TerminalWindowBecameKey, Some(c)),
        Some(c) => window_focus(WindowStatus::TerminalWindowIsCurrent, Some(c)),
        None => window_focus(WindowStatus::TerminalWindowResignedKey, None),
    });
    for w in &m.windows {
        let Some(selected) = w.selected_tab else { continue };
        out.push(focus_event(Focus::SelectedTab(selected.to_string())));
        // A tab with no active session has no event (iTerm2's nil guid clears the oneof).
        for a in w.tabs.iter().filter_map(|t| t.active.clone()) {
            out.push(focus_event(Focus::Session(a)));
        }
    }
    pb::FocusResponse { notifications: out }
}

fn domain_list(m: &Model) -> Vec<pb::BroadcastDomain> {
    m.windows.iter().flat_map(|w| w.domains.iter()).map(|d| pb::BroadcastDomain { session_ids: d.clone() }).collect()
}

/// iTerm2's GetBroadcastDomains: every window's domains, in window order (`H:3598-3608`).
pub fn broadcast_domains(m: &Model) -> pb::GetBroadcastDomainsResponse {
    pb::GetBroadcastDomainsResponse { broadcast_domains: domain_list(m) }
}

/// The answer to ListSessions, Focus or GetBroadcastDomains, carrying the request's id; None
/// for any other request.
pub fn answer(req: &pb::ClientOriginatedMessage, m: &Model) -> Option<pb::ServerOriginatedMessage> {
    let sub = match &req.submessage {
        Some(Req::ListSessionsRequest(_)) => Resp::ListSessionsResponse(list_sessions(m)),
        Some(Req::FocusRequest(_)) => Resp::FocusResponse(focus(m)),
        Some(Req::GetBroadcastDomainsRequest(_)) => Resp::GetBroadcastDomainsResponse(broadcast_domains(m)),
        _ => return None,
    };
    let mut out = api_proto::reply_to(req);
    out.submessage = Some(sub);
    Some(out)
}

/// What changed between two snapshots.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Changes {
    pub new_sessions: Vec<String>,
    pub terminated: Vec<String>,
    pub layout: bool,
    pub focus: Vec<pb::FocusChangedNotification>,
    pub broadcast: bool,
}

/// Every session id: the split trees', the minimized and the buried, in model order.
fn session_ids(m: &Model) -> Vec<String> {
    fn walk(n: &ApiNode, out: &mut Vec<String>) {
        match n {
            ApiNode::Leaf(s) => out.push(s.id.clone()),
            ApiNode::Split { children, .. } => children.iter().for_each(|c| walk(c, out)),
        }
    }
    let mut out = Vec::new();
    for t in m.windows.iter().flat_map(|w| &w.tabs) {
        walk(&t.root, &mut out);
        out.extend(t.minimized.iter().map(|s| s.id.clone()));
    }
    out.extend(m.buried.iter().map(|s| s.id.clone()));
    out
}

/// What a layout change compares: ids, order, split trees, selected tabs, active sessions,
/// buried and minimized sessions and tmux ids; never frames, grid sizes, titles, window
/// numbers, domains or focus.
fn shape(m: &Model) -> Model {
    fn bare(s: &ApiSession) -> ApiSession {
        ApiSession { id: s.id.clone(), frame: None, grid: None, title: String::new() }
    }
    fn node(n: &ApiNode) -> ApiNode {
        match n {
            ApiNode::Leaf(s) => ApiNode::Leaf(bare(s)),
            ApiNode::Split { vertical, children } => ApiNode::Split { vertical: *vertical, children: children.iter().map(node).collect() },
        }
    }
    Model {
        windows: m
            .windows
            .iter()
            .map(|w| ApiWindow {
                id: w.id.clone(),
                frame: (0, 0, 0, 0),
                number: 0,
                selected_tab: w.selected_tab,
                tabs: w
                    .tabs
                    .iter()
                    .map(|t| ApiTab {
                        id: t.id,
                        root: node(&t.root),
                        active: t.active.clone(),
                        tmux_window: t.tmux_window.clone(),
                        tmux_connection: t.tmux_connection.clone(),
                        minimized: t.minimized.iter().map(bare).collect(),
                    })
                    .collect(),
                domains: Vec::new(),
            })
            .collect(),
        buried: m.buried.iter().map(bare).collect(),
        ..Default::default()
    }
}

/// Who has the keyboard: a terminal window, another of Tandem's windows, or no one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Key<'a> {
    Terminal(&'a str),
    Other,
    Inactive,
}

fn key_of(m: &Model) -> Key<'_> {
    match (m.app_active, m.key_window.as_deref()) {
        (false, _) => Key::Inactive,
        (true, Some(k)) => Key::Terminal(k),
        (true, None) => Key::Other,
    }
}

/// The focus changes, in iTerm2's notification terms (`H:1000-1068`): the app becoming or
/// resigning active; a terminal window resigning key, then one becoming key (or, when another
/// of Tandem's windows took the keyboard, the current terminal window as IS_CURRENT); then per
/// window a new selected tab, and per tab a new active session.
fn focus_changes(old: &Model, new: &Model) -> Vec<pb::FocusChangedNotification> {
    let mut out = Vec::new();
    if old.app_active != new.app_active {
        out.push(focus_event(Focus::ApplicationActive(new.app_active)));
    }
    let (was, is) = (key_of(old), key_of(new));
    if was != is {
        if let Key::Terminal(a) = was {
            out.push(window_focus(WindowStatus::TerminalWindowResignedKey, Some(a)));
        }
        match is {
            Key::Terminal(b) => out.push(window_focus(WindowStatus::TerminalWindowBecameKey, Some(b))),
            Key::Other if was != Key::Other => {
                if let Some(c) = new.current_window.as_deref() {
                    out.push(window_focus(WindowStatus::TerminalWindowIsCurrent, Some(c)));
                }
            }
            _ => {}
        }
    }
    let old_tab = |id: u64| old.windows.iter().flat_map(|w| &w.tabs).find(|t| t.id == id);
    for w in &new.windows {
        let before = old.windows.iter().find(|o| o.id == w.id).and_then(|o| o.selected_tab);
        if let Some(t) = w.selected_tab.filter(|t| before != Some(*t)) {
            out.push(focus_event(Focus::SelectedTab(t.to_string())));
        }
        for t in &w.tabs {
            let before = old_tab(t.id).and_then(|o| o.active.as_deref());
            if let Some(a) = t.active.as_deref().filter(|a| before != Some(*a)) {
                out.push(focus_event(Focus::Session(a.to_string())));
            }
        }
    }
    out
}

/// `id` when it names one of `windows`: a window closed since it was used last is no current
/// window.
pub fn live_window(windows: &[ApiWindow], id: Option<String>) -> Option<String> {
    id.filter(|i| windows.iter().any(|w| w.id == *i))
}

/// The difference of two snapshots.
pub fn changes(old: &Model, new: &Model) -> Changes {
    let (before, after) = (session_ids(old), session_ids(new));
    Changes {
        new_sessions: after.iter().filter(|s| !before.contains(s)).cloned().collect(),
        terminated: before.iter().filter(|s| !after.contains(s)).cloned().collect(),
        layout: shape(old) != shape(new),
        focus: focus_changes(old, new),
        broadcast: domain_list(old) != domain_list(new),
    }
}

/// The notifications for `c`, in order: NEW_SESSION and TERMINATE_SESSION one per session,
/// LAYOUT_CHANGE with the new ListSessions, FOCUS_CHANGE one per change, BROADCAST_CHANGE with
/// the new domains.
pub fn notifications(c: &Changes, new: &Model) -> Vec<(pb::NotificationType, pb::Notification)> {
    use pb::NotificationType as N;
    let mut out = Vec::new();
    for id in &c.new_sessions {
        let n = pb::Notification { new_session_notification: Some(pb::NewSessionNotification { session_id: Some(id.clone()) }), ..Default::default() };
        out.push((N::NotifyOnNewSession, n));
    }
    for id in &c.terminated {
        let n = pb::Notification { terminate_session_notification: Some(pb::TerminateSessionNotification { session_id: Some(id.clone()) }), ..Default::default() };
        out.push((N::NotifyOnTerminateSession, n));
    }
    if c.layout {
        let n = pb::Notification { layout_changed_notification: Some(pb::LayoutChangedNotification { list_sessions_response: Some(list_sessions(new)) }), ..Default::default() };
        out.push((N::NotifyOnLayoutChange, n));
    }
    for f in &c.focus {
        out.push((N::NotifyOnFocusChange, pb::Notification { focus_changed_notification: Some(f.clone()), ..Default::default() }));
    }
    if c.broadcast {
        let n = pb::Notification { broadcast_domains_changed: Some(pb::BroadcastDomainsChangedNotification { broadcast_domains: domain_list(new) }), ..Default::default() };
        out.push((N::NotifyOnBroadcastChange, n));
    }
    out
}

/// A session as the fixture shows it: frame and grid from the id, named "s<id>".
#[cfg(test)]
pub fn fixture_session(id: PaneId) -> ApiSession {
    let i = id as i32;
    ApiSession { id: id.to_string(), frame: Some((i * 10, 0, 100, 50)), grid: Some((80 + i, 24)), title: format!("s{id}") }
}

/// The tests' Tandem: window "pty-A" number 0 with tabs 5 (panes 1 | (2 / 3), pane 2 active,
/// broadcasting) and 6 (pane 4 in tmux window @2 of client "gw"), tab 5 selected; window
/// "pty-B" number 1 with tab 9 (pane 7); pane 8 buried. "pty-A" is key and current.
#[cfg(test)]
pub fn fixture() -> Model {
    let tree = Tree::Split {
        axis: Axis::Row,
        ratio: 0.5,
        a: Box::new(Tree::Leaf(1)),
        b: Box::new(Tree::Split { axis: Axis::Column, ratio: 0.5, a: Box::new(Tree::Leaf(2)), b: Box::new(Tree::Leaf(3)) }),
    };
    let tab = |id: u64, tree: &Tree<PaneId>, active: PaneId| ApiTab {
        id,
        root: node_of(tree, None, &fixture_session).0,
        active: Some(active.to_string()),
        tmux_window: "-1".into(),
        tmux_connection: None,
        minimized: Vec::new(),
    };
    let tmux = ApiTab { tmux_window: "2".into(), tmux_connection: Some("gw".into()), ..tab(6, &Tree::Leaf(4), 4) };
    Model {
        windows: vec![
            ApiWindow {
                id: "pty-A".into(),
                frame: (10, 20, 800, 600),
                number: 0,
                selected_tab: Some(5),
                tabs: vec![tab(5, &tree, 2), tmux],
                domains: vec![vec!["1".into(), "2".into(), "3".into()]],
            },
            ApiWindow { id: "pty-B".into(), frame: (900, 20, 640, 480), number: 1, selected_tab: Some(9), tabs: vec![tab(9, &Tree::Leaf(7), 7)], domains: Vec::new() },
        ],
        buried: vec![ApiSession { id: "8".into(), frame: None, grid: None, title: "buried".into() }],
        app_active: true,
        key_window: Some("pty-A".into()),
        current_window: Some("pty-A".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broadcast::TabView;
    use pb::split_tree_node::split_tree_link::Child;

    fn leaf(p: PaneId) -> Tree<PaneId> {
        Tree::Leaf(p)
    }

    fn split(axis: Axis, a: Tree<PaneId>, b: Tree<PaneId>) -> Tree<PaneId> {
        Tree::Split { axis, ratio: 0.5, a: Box::new(a), b: Box::new(b) }
    }

    fn l(p: PaneId) -> ApiNode {
        ApiNode::Leaf(fixture_session(p))
    }

    #[test]
    fn split_tree_flattens_like_iterm2() {
        let s = &fixture_session;
        // Same-axis chains are one split view.
        let (root, min) = node_of(&split(Axis::Row, leaf(1), split(Axis::Row, leaf(2), leaf(3))), None, s);
        assert_eq!(root, ApiNode::Split { vertical: true, children: vec![l(1), l(2), l(3)] });
        assert!(min.is_empty());
        let (root, _) = node_of(&split(Axis::Row, split(Axis::Row, leaf(1), leaf(2)), leaf(3)), None, s);
        assert_eq!(root, ApiNode::Split { vertical: true, children: vec![l(1), l(2), l(3)] });
        // A side-by-side split holding a stacked one.
        let tree = split(Axis::Row, leaf(1), split(Axis::Column, leaf(2), leaf(3)));
        let (root, _) = node_of(&tree, None, s);
        assert_eq!(root, ApiNode::Split { vertical: true, children: vec![l(1), ApiNode::Split { vertical: false, children: vec![l(2), l(3)] }] });
        let (root, _) = node_of(&split(Axis::Column, leaf(1), leaf(2)), None, s);
        assert_eq!(root, ApiNode::Split { vertical: false, children: vec![l(1), l(2)] });
        // A single pane is a vertical split view with one leaf (`TAB:3108-3111`).
        assert_eq!(node_of(&leaf(4), None, s), (ApiNode::Split { vertical: true, children: vec![l(4)] }, vec![]));
        // A maximized pane: the only one shown, the others minimized in tree order.
        let (root, min) = node_of(&tree, Some(2), s);
        assert_eq!(root, ApiNode::Split { vertical: true, children: vec![l(2)] });
        assert_eq!(min, vec![fixture_session(1), fixture_session(3)]);
        // A zoomed pane that is not in the tree is ignored.
        assert_eq!(node_of(&tree, Some(99), s).1, vec![]);
    }

    fn session_of(link: &pb::split_tree_node::SplitTreeLink) -> &pb::SessionSummary {
        match &link.child {
            Some(Child::Session(s)) => s,
            other => panic!("not a session: {other:?}"),
        }
    }

    #[test]
    fn list_sessions_follows_iterm2() {
        let r = list_sessions(&fixture());
        assert_eq!(r.windows.len(), 2);
        let a = &r.windows[0];
        assert_eq!(a.window_id.as_deref(), Some("pty-A"));
        assert_eq!(a.frame, Some(frame((10, 20, 800, 600))));
        assert_eq!(a.number, Some(0));
        assert_eq!(a.selected_tab_id.as_deref(), Some("5"));
        assert_eq!(r.windows[1].window_id.as_deref(), Some("pty-B"));
        assert_eq!(r.windows[1].number, Some(1));
        assert_eq!(r.windows[1].selected_tab_id.as_deref(), Some("9"));
        // Tab 5: the split tree, the active session, not in tmux.
        let t5 = &a.tabs[0];
        assert_eq!(t5.tab_id.as_deref(), Some("5"));
        assert_eq!(t5.active_session_id.as_deref(), Some("2"));
        assert_eq!(t5.tmux_window_id.as_deref(), Some("-1"));
        assert_eq!(t5.tmux_connection_id, None);
        assert!(t5.minimized_sessions.is_empty());
        let root = t5.root.as_ref().unwrap();
        assert_eq!(root.vertical, Some(true));
        assert_eq!(root.links.len(), 2);
        let one = session_of(&root.links[0]);
        assert_eq!(one.unique_identifier.as_deref(), Some("1"));
        assert_eq!(one.frame, Some(frame((10, 0, 100, 50))));
        assert_eq!(one.grid_size, Some(pb::Size { width: Some(81), height: Some(24) }));
        assert_eq!(one.title.as_deref(), Some("s1"));
        match &root.links[1].child {
            Some(Child::Node(n)) => {
                assert_eq!(n.vertical, Some(false));
                let ids: Vec<_> = n.links.iter().map(|l| session_of(l).unique_identifier.clone().unwrap()).collect();
                assert_eq!(ids, ["2", "3"]);
            }
            other => panic!("{other:?}"),
        }
        // Tab 6: in tmux window @2 of client "gw".
        let t6 = &a.tabs[1];
        assert_eq!(t6.tab_id.as_deref(), Some("6"));
        assert_eq!(t6.tmux_window_id.as_deref(), Some("2"));
        assert_eq!(t6.tmux_connection_id.as_deref(), Some("gw"));
        assert_eq!(t6.root.as_ref().unwrap().vertical, Some(true));
        assert_eq!(session_of(&t6.root.as_ref().unwrap().links[0]).unique_identifier.as_deref(), Some("4"));
        // No tab group fields.
        for t in r.windows.iter().flat_map(|w| &w.tabs) {
            assert_eq!((&t.tab_group_id, &t.tab_group_name, &t.tab_group_color, t.tab_group_collapsed), (&None, &None, &None, None));
        }
        // Buried: id and name, no frame and no grid size.
        assert_eq!(r.buried_sessions, [pb::SessionSummary { unique_identifier: Some("8".into()), frame: None, grid_size: None, title: Some("buried".into()) }]);
        // Minimized sessions: id and name only.
        let mut m = fixture();
        let (root, min) = node_of(&split(Axis::Row, leaf(1), leaf(2)), Some(1), &fixture_session);
        m.windows[1].tabs[0].root = root;
        m.windows[1].tabs[0].minimized = min;
        let t = &list_sessions(&m).windows[1].tabs[0];
        assert_eq!(t.minimized_sessions, [pb::SessionSummary { unique_identifier: Some("2".into()), frame: None, grid_size: None, title: Some("s2".into()) }]);
    }

    fn app_active(on: bool) -> pb::FocusChangedNotification {
        focus_event(Focus::ApplicationActive(on))
    }

    fn win(status: WindowStatus, id: Option<&str>) -> pb::FocusChangedNotification {
        window_focus(status, id)
    }

    fn tab(id: &str) -> pb::FocusChangedNotification {
        focus_event(Focus::SelectedTab(id.into()))
    }

    fn sess(id: &str) -> pb::FocusChangedNotification {
        focus_event(Focus::Session(id.into()))
    }

    #[test]
    fn focus_follows_iterm2() {
        use WindowStatus::*;
        let m = fixture();
        let tail = [tab("5"), sess("2"), sess("4"), tab("9"), sess("7")];
        let expect = |head: Vec<pb::FocusChangedNotification>| head.into_iter().chain(tail.iter().cloned()).collect::<Vec<_>>();
        assert_eq!(focus(&m).notifications, expect(vec![app_active(true), win(TerminalWindowBecameKey, Some("pty-A"))]));
        // Another of Tandem's windows has the keyboard: the current terminal window.
        let other = Model { key_window: None, ..m.clone() };
        assert_eq!(focus(&other).notifications, expect(vec![app_active(true), win(TerminalWindowIsCurrent, Some("pty-A"))]));
        // Tandem inactive: iTerm2 still names the current terminal window as current
        // (`H:3346-3355`: RESIGNED_KEY only when there is no current terminal).
        let inactive = Model { app_active: false, key_window: None, ..m.clone() };
        assert_eq!(focus(&inactive).notifications, expect(vec![app_active(false), win(TerminalWindowIsCurrent, Some("pty-A"))]));
        // No window: RESIGNED_KEY with no id.
        let none = Model { app_active: false, ..Default::default() };
        assert_eq!(focus(&none).notifications, vec![app_active(false), win(TerminalWindowResignedKey, None)]);
        // A window without a selected tab is skipped.
        let mut empty = m.clone();
        empty.windows[1].selected_tab = None;
        empty.windows[1].tabs.clear();
        assert_eq!(focus(&empty).notifications[2..], [tab("5"), sess("2"), sess("4")]);
        // A tab with no active session sends no session event, never an empty id (the
        // package would refresh on an id it cannot find, and again, `H:3360-3368`).
        let mut idle = m.clone();
        idle.windows[0].tabs[1].active = None;
        assert_eq!(focus(&idle).notifications[2..], [tab("5"), sess("2"), tab("9"), sess("7")]);
    }

    #[test]
    fn current_window_is_a_live_one() {
        let m = fixture();
        assert_eq!(live_window(&m.windows, Some("pty-B".into())), Some("pty-B".into()));
        // A just-closed window is no current window.
        assert_eq!(live_window(&m.windows, Some("pty-C".into())), None);
        assert_eq!(live_window(&m.windows, None), None);
    }

    #[test]
    fn changes_never_read_titles() {
        // The notification tick diffs snapshots built without titles; the result is the same.
        let untitled = |m: &Model| {
            let mut m = m.clone();
            fn clear(n: &mut ApiNode) {
                match n {
                    ApiNode::Leaf(s) => s.title.clear(),
                    ApiNode::Split { children, .. } => children.iter_mut().for_each(clear),
                }
            }
            for t in m.windows.iter_mut().flat_map(|w| &mut w.tabs) {
                clear(&mut t.root);
                t.minimized.iter_mut().for_each(|s| s.title.clear());
            }
            m.buried.iter_mut().for_each(|s| s.title.clear());
            m
        };
        let old = fixture();
        let mut new = fixture();
        let extra = ApiTab { id: 11, ..new.windows[1].tabs[0].clone() };
        new.windows[1].tabs.push(extra);
        new.windows[0].selected_tab = Some(6);
        new.buried.clear();
        new.app_active = false;
        for (a, b) in [(&old, &new), (&new, &old), (&old, &old)] {
            assert_eq!(changes(&untitled(a), &untitled(b)), changes(a, b));
        }
    }

    fn view(tabs: &[(bool, &[PaneId])], current_tab: usize, current: PaneId) -> View {
        View { tabs: tabs.iter().map(|(b, p)| TabView { broadcasting: *b, panes: p.to_vec() }).collect(), current_tab, current, exited: Default::default() }
    }

    #[test]
    fn domains_follow_iterm2() {
        let v = view(&[(true, &[1, 2, 3]), (false, &[4]), (true, &[5])], 1, 4);
        let b = |raw: Mode, custom: &[&[PaneId]]| Broadcast { raw, domains: custom.iter().map(|d| d.iter().copied().collect()).collect(), sources: Default::default() };
        // Off: each broadcasting tab's panes.
        assert_eq!(domains(&b(Mode::Off, &[]), &v), vec![vec![1, 2, 3], vec![5]]);
        assert_eq!(domains(&b(Mode::Off, &[]), &view(&[(false, &[1])], 0, 1)), Vec::<Vec<PaneId>>::new());
        // All tabs: every pane of the window.
        assert_eq!(domains(&b(Mode::AllTabs, &[]), &v), vec![vec![1, 2, 3, 4, 5]]);
        // All panes: the current tab's (only while it broadcasts; else the mode reads off).
        let v0 = view(&[(true, &[1, 2, 3]), (false, &[4])], 0, 2);
        assert_eq!(domains(&b(Mode::AllPanes, &[]), &v0), vec![vec![1, 2, 3]]);
        // Custom: only the current domain, kept even when empty.
        let custom = b(Mode::Custom, &[&[1, 2], &[3, 4]]);
        assert_eq!(domains(&custom, &view(&[(false, &[1, 2, 3, 4])], 0, 4)), vec![vec![3, 4]]);
        assert_eq!(domains(&custom, &view(&[(false, &[1, 2, 3, 4, 5])], 0, 5)), vec![Vec::<PaneId>::new()]);
        // The answer: every window's list, in window order.
        let mut m = fixture();
        m.windows[1].domains = vec![vec!["7".into()], vec![]];
        let ids: Vec<Vec<String>> = broadcast_domains(&m).broadcast_domains.into_iter().map(|d| d.session_ids).collect();
        assert_eq!(ids, [vec!["1", "2", "3"], vec!["7"], vec![]]);
    }

    /// Window `w`'s tab `t`.
    fn tab_mut(m: &mut Model, w: usize, t: usize) -> &mut ApiTab {
        &mut m.windows[w].tabs[t]
    }

    #[test]
    fn changes_follow_iterm2() {
        use pb::NotificationType as N;
        use WindowStatus::*;
        let old = fixture();
        assert_eq!(changes(&old, &old), Changes::default());
        // A new pane.
        let mut new = old.clone();
        tab_mut(&mut new, 1, 0).root = node_of(&split(Axis::Row, leaf(7), leaf(10)), None, &fixture_session).0;
        let c = changes(&old, &new);
        assert_eq!((c.new_sessions.as_slice(), c.terminated.is_empty(), c.layout, c.focus.is_empty(), c.broadcast), (&["10".to_string()][..], true, true, true, false));
        // A closed one.
        let c = changes(&new, &old);
        assert_eq!((c.new_sessions.is_empty(), c.terminated.as_slice(), c.layout), (true, &["10".to_string()][..], true));
        // Burying moves a session: no new, no terminated, a layout change.
        let mut buried = old.clone();
        tab_mut(&mut buried, 1, 0).root = node_of(&split(Axis::Row, leaf(7), leaf(10)), None, &fixture_session).0;
        buried.buried.clear();
        let mut from = buried.clone();
        tab_mut(&mut from, 1, 0).root = node_of(&leaf(7), None, &fixture_session).0;
        from.buried = vec![ApiSession { id: "10".into(), frame: None, grid: None, title: "s10".into() }];
        let c = changes(&buried, &from);
        assert_eq!((c.new_sessions.is_empty(), c.terminated.is_empty(), c.layout), (true, true, true));
        // A title, frame or grid size change alone: nothing.
        let mut cosmetic = old.clone();
        cosmetic.windows[0].frame = (0, 0, 1, 1);
        tab_mut(&mut cosmetic, 0, 0).root = node_of(
            &split(Axis::Row, leaf(1), split(Axis::Column, leaf(2), leaf(3))),
            None,
            &|p| ApiSession { title: "renamed".into(), frame: Some((1, 2, 3, 4)), grid: Some((9, 9)), ..fixture_session(p) },
        )
        .0;
        cosmetic.buried[0].title = "renamed".into();
        assert_eq!(changes(&old, &cosmetic), Changes::default());
        // A selected tab change: selected_tab and layout (ListSessions carries it).
        let mut sel = old.clone();
        sel.windows[0].selected_tab = Some(6);
        let c = changes(&old, &sel);
        assert_eq!((c.focus.as_slice(), c.layout), (&[tab("6")][..], true));
        // An active session change.
        let mut act = old.clone();
        tab_mut(&mut act, 0, 0).active = Some("3".into());
        assert_eq!(changes(&old, &act).focus, [sess("3")]);
        // The key window moving from A to B.
        let moved = Model { key_window: Some("pty-B".into()), current_window: Some("pty-B".into()), ..old.clone() };
        assert_eq!(changes(&old, &moved).focus, [win(TerminalWindowResignedKey, Some("pty-A")), win(TerminalWindowBecameKey, Some("pty-B"))]);
        // The app going inactive, and back.
        let inactive = Model { app_active: false, key_window: None, ..old.clone() };
        assert_eq!(changes(&old, &inactive).focus, [app_active(false), win(TerminalWindowResignedKey, Some("pty-A"))]);
        assert_eq!(changes(&inactive, &old).focus, [app_active(true), win(TerminalWindowBecameKey, Some("pty-A"))]);
        // Another of Tandem's windows taking the keyboard: A resigns, A is current.
        let other = Model { key_window: None, ..old.clone() };
        assert_eq!(changes(&old, &other).focus, [win(TerminalWindowResignedKey, Some("pty-A")), win(TerminalWindowIsCurrent, Some("pty-A"))]);
        // A domain change.
        let mut dom = old.clone();
        dom.windows[1].domains = vec![vec!["7".into()]];
        let c = changes(&old, &dom);
        assert_eq!((c.broadcast, c.layout, c.focus.is_empty()), (true, false, true));

        // The notifications, in order.
        let mut all = old.clone();
        tab_mut(&mut all, 1, 0).root = node_of(&split(Axis::Row, leaf(10), leaf(11)), None, &fixture_session).0;
        tab_mut(&mut all, 1, 0).active = Some("10".into());
        all.windows[1].domains = vec![vec!["10".into(), "11".into()]];
        let c = changes(&old, &all);
        assert_eq!((c.new_sessions.clone(), c.terminated.clone()), (vec!["10".to_string(), "11".to_string()], vec!["7".to_string()]));
        let n = notifications(&c, &all);
        let kinds: Vec<N> = n.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            kinds,
            [N::NotifyOnNewSession, N::NotifyOnNewSession, N::NotifyOnTerminateSession, N::NotifyOnLayoutChange, N::NotifyOnFocusChange, N::NotifyOnBroadcastChange]
        );
        assert_eq!(n[0].1.new_session_notification.as_ref().unwrap().session_id.as_deref(), Some("10"));
        assert_eq!(n[1].1.new_session_notification.as_ref().unwrap().session_id.as_deref(), Some("11"));
        assert_eq!(n[2].1.terminate_session_notification.as_ref().unwrap().session_id.as_deref(), Some("7"));
        assert_eq!(n[3].1.layout_changed_notification.as_ref().unwrap().list_sessions_response, Some(list_sessions(&all)));
        assert_eq!(n[4].1.focus_changed_notification, Some(sess("10")));
        assert_eq!(n[5].1.broadcast_domains_changed.as_ref().unwrap().broadcast_domains, broadcast_domains(&all).broadcast_domains);
        assert!(notifications(&Changes::default(), &all).is_empty());
    }

    #[test]
    fn async_get_app_sequence() {
        use crate::api_dispatch::{route, Route, Subscriptions};
        use pb::notification_response::Status;
        use pb::NotificationType as N;
        let m = fixture();
        let mut subs = Subscriptions::default();
        let notif = |kind: N| Req::NotificationRequest(pb::NotificationRequest { session: Some("all".into()), subscribe: Some(true), notification_type: Some(kind as i32), arguments: None });
        let kinds = [N::NotifyOnNewSession, N::NotifyOnTerminateSession, N::NotifyOnLayoutChange, N::NotifyOnFocusChange, N::NotifyOnBroadcastChange];
        let mut requests = vec![Req::ListSessionsRequest(pb::ListSessionsRequest {})];
        requests.extend(kinds.iter().map(|k| notif(*k)));
        requests.push(Req::FocusRequest(pb::FocusRequest {}));
        requests.push(Req::GetBroadcastDomainsRequest(pb::GetBroadcastDomainsRequest {}));
        let serve = |subs: &mut Subscriptions, id: i64, sub: Req| {
            let req = pb::ClientOriginatedMessage { id: Some(id), submessage: Some(sub) };
            match route(subs, 1, req) {
                Route::Reply(m) => m,
                Route::App(req) => answer(&req, &m).expect("an async_get_app request"),
            }
        };
        let answers: Vec<_> = requests.clone().into_iter().zip(1..).map(|(sub, id)| serve(&mut subs, id, sub)).collect();
        for (a, id) in answers.iter().zip(1..=8) {
            assert_eq!(a.id, Some(id));
        }
        assert_eq!(answers[0].submessage, Some(Resp::ListSessionsResponse(list_sessions(&m))));
        for a in &answers[1..6] {
            assert_eq!(a.submessage, Some(Resp::NotificationResponse(pb::NotificationResponse { status: Some(Status::Ok as i32) })));
        }
        assert_eq!(answers[6].submessage, Some(Resp::FocusResponse(focus(&m))));
        assert_eq!(answers[7].submessage, Some(Resp::GetBroadcastDomainsResponse(broadcast_domains(&m))));
        for k in kinds {
            assert_eq!(subs.subscribers(k), [1]);
        }
        // Subscribing again.
        for (sub, id) in requests[1..6].iter().cloned().zip(9..) {
            let a = serve(&mut subs, id, sub);
            assert_eq!((a.id, a.submessage), (Some(id), Some(Resp::NotificationResponse(pb::NotificationResponse { status: Some(Status::AlreadySubscribed as i32) }))));
        }
        // Anything else is not the model's to answer.
        assert_eq!(answer(&pb::ClientOriginatedMessage { id: Some(1), submessage: None }, &m), None);
    }
}

//! The Python API's layout requests: SavedArrangement (`H:3269-3336`), RestartSession
//! (`H:3497-3522`), SetTabLayout (`H:3550-3596`) and ReorderTabs (`H:3731-3800`), their
//! statuses and plans made from the request and a snapshot of the app's windows, so the
//! app only carries them out.

use crate::api_proto::pb;
use crate::api_session::Snapshot;
use crate::layout::{Axis, Tree};
use pb::split_tree_node::split_tree_link::Child;

/// What a SavedArrangement request asks: save (every window, or the one named), restore (as
/// new windows, or as tabs in the one named), or list the names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Arrange {
    Save { name: String, window: Option<String> },
    Restore { name: String, window: Option<String> },
    List,
}

/// SavedArrangement (`H:3269-3336`): the action, then for a save or restore the window
/// (WINDOW_NOT_FOUND when one is named and `window_known` does not know it). An empty
/// window id names none (`windowID.length`). iTerm2's action is a proto2 enum: unset, or a
/// value the enum does not have (kept as an unknown field by the Objective-C runtime),
/// reads as the default RESTORE, so its REQUEST_MALFORMED branch is never reached.
pub fn arrange(r: &pb::SavedArrangementRequest, window_known: &dyn Fn(&str) -> bool) -> Result<Arrange, pb::saved_arrangement_response::Status> {
    use pb::saved_arrangement_request::Action;
    use pb::saved_arrangement_response::Status;
    let action = r.action.and_then(|a| Action::try_from(a).ok()).unwrap_or(Action::Restore);
    if action == Action::List {
        return Ok(Arrange::List);
    }
    let name = r.name.clone().unwrap_or_default();
    let window = r.window_id.clone().filter(|w| !w.is_empty());
    if window.as_deref().is_some_and(|w| !window_known(w)) {
        return Err(Status::WindowNotFound);
    }
    Ok(match action {
        Action::Save => Arrange::Save { name, window },
        _ => Arrange::Restore { name, window },
    })
}

/// RestartSession (`H:3497-3522`): no session SESSION_NOT_FOUND; one with no program to run
/// again (`isRestartable`, `PS:4849-4854`) SESSION_NOT_RESTARTABLE; with `only_if_exited`
/// a session still running is SESSION_NOT_RESTARTABLE too.
pub fn restart_status(found: bool, restartable: bool, only_if_exited: bool, exited: bool) -> pb::restart_session_response::Status {
    use pb::restart_session_response::Status;
    if !found {
        Status::SessionNotFound
    } else if !restartable || (only_if_exited && !exited) {
        Status::SessionNotRestartable
    } else {
        Status::Ok
    }
}

/// `iTermCheckSplitTreesIsomorphic` (`H:3550-3573`): the same `vertical` (unset reads
/// false), the same number of links, each link of `a` set and of the same kind as in `b`,
/// nodes alike all the way down. Session ids are not compared.
pub fn isomorphic(a: &pb::SplitTreeNode, b: &pb::SplitTreeNode) -> bool {
    if a.vertical.unwrap_or(false) != b.vertical.unwrap_or(false) || a.links.len() != b.links.len() {
        return false;
    }
    a.links.iter().zip(&b.links).all(|(x, y)| match (&x.child, &y.child) {
        (None, _) => false,
        (Some(Child::Session(_)), Some(Child::Session(_))) => true,
        (Some(Child::Node(m)), Some(Child::Node(n))) => isomorphic(m, n),
        _ => false,
    })
}

/// A link's size in cells as `TAB:setSizesFromSplitTreeNode:splitView:` adds it: a session's
/// grid size; a node's children summed along its axis (`vertical` sums widths) and the
/// largest across it.
fn extent(link: &pb::split_tree_node::SplitTreeLink) -> (f32, f32) {
    match &link.child {
        Some(Child::Session(s)) => s.grid_size.as_ref().map_or((0.0, 0.0), |g| (g.width.unwrap_or(0).max(0) as f32, g.height.unwrap_or(0).max(0) as f32)),
        Some(Child::Node(n)) => {
            let sizes = n.links.iter().map(extent);
            if n.vertical.unwrap_or(false) {
                sizes.fold((0.0, 0.0), |(w, h), (cw, ch)| (w + cw, f32::max(h, ch)))
            } else {
                sizes.fold((0.0, 0.0), |(w, h), (cw, ch)| (f32::max(w, cw), h + ch))
            }
        }
        None => (0.0, 0.0),
    }
}

/// How many of a split view's children `tree` covers along `axis`: a chain of same-axis
/// splits is one split view (`api_model::node_of`).
fn chain_len<T>(tree: &Tree<T>, axis: Axis) -> usize {
    match tree {
        Tree::Split { axis: a, a: first, b: second, .. } if *a == axis => chain_len(first, axis) + chain_len(second, axis),
        _ => 1,
    }
}

/// SetTabLayout's sizes (`TAB:setSizesFromSplitTreeNode:`) as Termurion's split ratios: for each
/// binary split of `tree`, by its path from the root (true is the second side), the share of
/// the requested cells its first side takes along its axis (widths for a side-by-side split),
/// in tree order. `req` must be `tree` as `api_model::node_of` flattens it (isomorphic); a
/// split whose sides request no cells at all is left alone.
pub fn ratios(req: &pb::SplitTreeNode, tree: &Tree<u64>) -> Vec<(Vec<bool>, f32)> {
    fn walk<T>(tree: &Tree<T>, axis: Axis, links: &[pb::split_tree_node::SplitTreeLink], path: &mut Vec<bool>, out: &mut Vec<(Vec<bool>, f32)>) {
        match tree {
            Tree::Split { axis: a, a: first, b: second, .. } if *a == axis => {
                let n = chain_len(first, axis);
                if n >= links.len() {
                    return;
                }
                let along = |l: &pb::split_tree_node::SplitTreeLink| {
                    let (w, h) = extent(l);
                    if axis == Axis::Row { w } else { h }
                };
                let head: f32 = links[..n].iter().map(along).sum();
                let total: f32 = head + links[n..].iter().map(along).sum::<f32>();
                if total > 0.0 {
                    out.push((path.clone(), head / total));
                }
                path.push(false);
                walk(first, axis, &links[..n], path, out);
                path.pop();
                path.push(true);
                walk(second, axis, &links[n..], path, out);
                path.pop();
            }
            // Another split view, or a session.
            Tree::Split { axis: a, .. } => {
                if let [pb::split_tree_node::SplitTreeLink { child: Some(Child::Node(n)) }] = links {
                    let inner = if n.vertical.unwrap_or(false) { Axis::Row } else { Axis::Column };
                    if inner == *a {
                        walk(tree, inner, &n.links, path, out);
                    }
                }
            }
            Tree::Leaf(_) => {}
        }
    }
    let axis = if req.vertical.unwrap_or(false) { Axis::Row } else { Axis::Column };
    let mut out = Vec::new();
    walk(tree, axis, &req.links, &mut Vec::new(), &mut out);
    out
}

/// `validateReorderTabsRequest:` (`H:3731-3757`), per assignment in order: its window
/// repeated INVALID_ASSIGNMENT, then unknown INVALID_WINDOW_ID; then per tab: unknown
/// INVALID_TAB_ID, then repeated (in any assignment so far) INVALID_ASSIGNMENT.
pub fn reorder_status(r: &pb::ReorderTabsRequest, s: &Snapshot) -> pb::reorder_tabs_response::Status {
    use pb::reorder_tabs_response::Status;
    let (mut windows, mut tabs): (Vec<&str>, Vec<&str>) = (Vec::new(), Vec::new());
    for a in &r.assignments {
        let w = a.window_id.as_deref().unwrap_or_default();
        if windows.contains(&w) {
            return Status::InvalidAssignment;
        }
        if s.window_key(w).is_none() {
            return Status::InvalidWindowId;
        }
        windows.push(w);
        for t in &a.tab_ids {
            if s.tab_by_id(t).is_none() {
                return Status::InvalidTabId;
            }
            if tabs.contains(&t.as_str()) {
                return Status::InvalidAssignment;
            }
            tabs.push(t);
        }
    }
    Status::Ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use pb::split_tree_node::SplitTreeLink;
    use std::collections::BTreeSet;

    /// Task 1's fixture: window key 1 "pty-A" with tabs 5 (panes 1, 2) and 6 (pane 3),
    /// window key 2 "pty-B" with tab 9 (pane 7); pane 8 buried; pane 3 exited.
    fn fixture() -> Snapshot {
        Snapshot {
            windows: vec![(1, "pty-A".into(), vec![(5, vec![1, 2]), (6, vec![3])]), (2, "pty-B".into(), vec![(9, vec![7])])],
            buried: vec![8],
            exited: BTreeSet::from([3]),
            active: Some(2),
        }
    }

    fn session(id: &str, w: i32, h: i32) -> SplitTreeLink {
        let grid_size = Some(pb::Size { width: Some(w), height: Some(h) });
        SplitTreeLink { child: Some(Child::Session(pb::SessionSummary { unique_identifier: Some(id.into()), frame: None, grid_size, title: None })) }
    }

    fn node(vertical: bool, links: Vec<SplitTreeLink>) -> pb::SplitTreeNode {
        pb::SplitTreeNode { vertical: Some(vertical), links }
    }

    fn sub(n: pb::SplitTreeNode) -> SplitTreeLink {
        SplitTreeLink { child: Some(Child::Node(n)) }
    }

    fn leaf(p: u64) -> Tree<u64> {
        Tree::Leaf(p)
    }

    fn split(axis: Axis, a: Tree<u64>, b: Tree<u64>) -> Tree<u64> {
        Tree::Split { axis, ratio: 0.5, a: Box::new(a), b: Box::new(b) }
    }

    #[test]
    fn isomorphism_follows_iterm2() {
        let a = node(true, vec![session("1", 10, 10), sub(node(false, vec![session("2", 5, 5), session("3", 5, 5)]))]);
        // Ids and sizes are not compared.
        let b = node(true, vec![session("7", 1, 1), sub(node(false, vec![session("8", 2, 2), session("9", 3, 3)]))]);
        assert!(isomorphic(&a, &b));
        // `vertical` differing, at the root or below.
        assert!(!isomorphic(&node(false, a.links.clone()), &b));
        assert!(!isomorphic(&a, &node(true, vec![session("7", 1, 1), sub(node(true, vec![session("8", 2, 2), session("9", 3, 3)]))])));
        // Unset `vertical` reads false.
        assert!(isomorphic(&pb::SplitTreeNode { vertical: None, links: vec![session("1", 1, 1)] }, &node(false, vec![session("2", 1, 1)])));
        // Link counts differing.
        assert!(!isomorphic(&a, &node(true, vec![session("7", 1, 1)])));
        assert!(!isomorphic(&a, &pb::SplitTreeNode::default()));
        // A link with no child.
        let empty = node(true, vec![SplitTreeLink { child: None }]);
        assert!(!isomorphic(&empty, &empty));
        // A session where the other has a node, either way round.
        let flat = node(true, vec![session("1", 1, 1), session("2", 1, 1)]);
        assert!(!isomorphic(&flat, &b));
        assert!(!isomorphic(&b, &flat));
    }

    #[test]
    fn tab_layout_ratios_follow_grid_sizes() {
        let req = node(true, vec![session("1", 40, 10), session("2", 20, 10), session("3", 20, 10)]);
        assert_eq!(ratios(&req, &split(Axis::Row, leaf(1), split(Axis::Row, leaf(2), leaf(3)))), vec![(vec![], 0.5), (vec![true], 0.5)]);
        // A node's extent is the sum along its axis and the largest across it.
        let req = node(true, vec![session("1", 30, 40), sub(node(false, vec![session("2", 50, 10), session("3", 50, 30)]))]);
        assert_eq!(ratios(&req, &split(Axis::Row, leaf(1), split(Axis::Column, leaf(2), leaf(3)))), vec![(vec![], 0.375), (vec![true], 0.25)]);
        // The other nesting of the same flattened view.
        let req = node(true, vec![session("1", 10, 10), session("2", 30, 10), session("3", 60, 10)]);
        assert_eq!(ratios(&req, &split(Axis::Row, split(Axis::Row, leaf(1), leaf(2)), leaf(3))), vec![(vec![], 0.4), (vec![false], 0.25)]);
        // A single pane.
        assert_eq!(ratios(&node(true, vec![session("1", 80, 24)]), &leaf(1)), vec![]);
        // No cells requested: the split is left alone.
        assert_eq!(ratios(&node(false, vec![session("1", 0, 0), session("2", 0, 0)]), &split(Axis::Column, leaf(1), leaf(2))), vec![]);
    }

    #[test]
    fn reorder_follows_iterm2() {
        use pb::reorder_tabs_request::Assignment;
        use pb::reorder_tabs_response::Status;
        let s = fixture();
        let req = |a: &[(&str, &[&str])]| pb::ReorderTabsRequest {
            assignments: a.iter().map(|(w, t)| Assignment { window_id: Some(w.to_string()), tab_ids: t.iter().map(|t| t.to_string()).collect() }).collect(),
        };
        assert_eq!(reorder_status(&req(&[("pty-A", &["6", "5"])]), &s), Status::Ok);
        assert_eq!(reorder_status(&req(&[("pty-A", &["6"]), ("pty-A", &["5"])]), &s), Status::InvalidAssignment);
        assert_eq!(reorder_status(&req(&[("pty-Z", &["5"])]), &s), Status::InvalidWindowId);
        assert_eq!(reorder_status(&req(&[("pty-A", &["77"])]), &s), Status::InvalidTabId);
        assert_eq!(reorder_status(&req(&[("pty-A", &["5"]), ("pty-B", &["5"])]), &s), Status::InvalidAssignment);
        // The repeated window before its existence.
        assert_eq!(reorder_status(&req(&[("pty-Z", &[]), ("pty-Z", &[])]), &s), Status::InvalidWindowId);
        assert_eq!(reorder_status(&req(&[("pty-A", &[]), ("pty-A", &["77"])]), &s), Status::InvalidAssignment);
        // A tab's existence before its repetition.
        assert_eq!(reorder_status(&req(&[("pty-A", &["5", "5", "77"])]), &s), Status::InvalidAssignment);
        assert_eq!(reorder_status(&req(&[("pty-A", &["77", "5", "5"])]), &s), Status::InvalidTabId);
        // Checks run per assignment: a bad tab in the first wins over a bad window after it.
        assert_eq!(reorder_status(&req(&[("pty-A", &["77"]), ("pty-Z", &[])]), &s), Status::InvalidTabId);
        // An unset window id names no window.
        let unset = pb::ReorderTabsRequest { assignments: vec![Assignment { window_id: None, tab_ids: vec![] }] };
        assert_eq!(reorder_status(&unset, &s), Status::InvalidWindowId);
        assert_eq!(reorder_status(&pb::ReorderTabsRequest::default(), &s), Status::Ok);
    }

    #[test]
    fn restart_follows_iterm2() {
        use pb::restart_session_response::Status;
        assert_eq!(restart_status(false, true, false, true), Status::SessionNotFound);
        assert_eq!(restart_status(true, false, false, true), Status::SessionNotRestartable);
        assert_eq!(restart_status(true, true, true, false), Status::SessionNotRestartable);
        assert_eq!(restart_status(true, true, true, true), Status::Ok);
        assert_eq!(restart_status(true, true, false, false), Status::Ok);
    }

    #[test]
    fn arrangements_follow_iterm2() {
        use pb::saved_arrangement_request::Action;
        use pb::saved_arrangement_response::Status;
        let known = |w: &str| w == "pty-A";
        let req = |action: Option<i32>, name: &str, window: Option<&str>| pb::SavedArrangementRequest { name: Some(name.into()), action, window_id: window.map(str::to_string) };
        // Unset is the proto2 default, RESTORE.
        assert_eq!(arrange(&req(None, "x", None), &known), Ok(Arrange::Restore { name: "x".into(), window: None }));
        assert_eq!(arrange(&req(Some(Action::Save as i32), "x", Some("pty-Z")), &known), Err(Status::WindowNotFound));
        assert_eq!(arrange(&req(Some(Action::Save as i32), "x", None), &known), Ok(Arrange::Save { name: "x".into(), window: None }));
        assert_eq!(arrange(&req(Some(Action::Restore as i32), "x", Some("pty-A")), &known), Ok(Arrange::Restore { name: "x".into(), window: Some("pty-A".into()) }));
        assert_eq!(arrange(&req(Some(Action::Restore as i32), "x", Some("pty-Z")), &known), Err(Status::WindowNotFound));
        assert_eq!(arrange(&req(Some(Action::List as i32), "x", Some("pty-Z")), &known), Ok(Arrange::List));
        // An empty window id names none (`windowID.length`).
        assert_eq!(arrange(&req(Some(Action::Save as i32), "x", Some("")), &known), Ok(Arrange::Save { name: "x".into(), window: None }));
        // A value the enum lacks: the Objective-C runtime keeps it as an unknown field and the
        // action reads the default RESTORE (GPBMessage.m:2240-2248), source over the brief.
        assert_eq!(arrange(&req(Some(7), "x", None), &known), Ok(Arrange::Restore { name: "x".into(), window: None }));
    }
}

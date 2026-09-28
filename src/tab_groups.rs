//! iTerm2's tab groups and pinned tabs without the app (milestone 13d): the ordering rules
//! that keep pinned tabs in front and a group's members together, a dropped tab's
//! membership, the keyboard moves, group selection, the collapse rules, the group palette
//! and the per-tab group definition. Pure code; the app applies the orders.
//!
//! Citations: `ORD` = `sources/TabGroups/iTermTabGroupOrdering.swift`, `CON` =
//! `sources/TabGroups/iTermTabGroupContiguity.swift`, `PT` =
//! `sources/TerminalView/PseudoTerminal.m`, `TABH` = `sources/TerminalView/PTYTab.h` under
//! `~/gitrepos/iterm2-ref`.
//!
//! An order vector holds each tab's group id (`None` when ungrouped), in tab order.

use crate::palette::{rgb, Rgb};

/// A tab group's definition. It rides every member tab (`TABH:99-115`); there is no
/// registry, so a window derives its groups from its tabs (`groups_in_window`). The id is a
/// UUID, not window-scoped, so a group keeps it when a tab moves to another window.
#[derive(Clone, Debug, PartialEq)]
pub struct TabGroup {
    pub id: String,
    pub name: String,
    pub color: Rgb,
    pub collapsed: bool,
}

/// A new group's id: an upper-case UUID, iTerm2's `[[NSUUID UUID] UUIDString]`.
pub fn fresh_id() -> String {
    uuid::Uuid::new_v4().hyphenated().to_string().to_uppercase()
}

/// `canonicalOrder` (`ORD`). The tab bar's two ordering invariants:
///   1. Pinned tabs form a prefix (pinned-left, unpinned-right).
///   2. A group's members are contiguous.
/// When both cannot hold (a group with pinned and unpinned members), the pinned invariant
/// wins: the repair never moves a tab across the pinned boundary, so such a group spans the
/// boundary as at most two runs.
///
/// Stable-partition into the pinned prefix and the unpinned suffix, then within each class
/// independently compact every group into one block anchored at its first member in that
/// class. The result is idempotent. `result[k]` is the input index of the tab that belongs
/// at position k.
pub fn canonical_order(groups: &[Option<&str>], pinned: &[bool]) -> Vec<usize> {
    debug_assert_eq!(groups.len(), pinned.len(), "parallel arrays required");
    let compacted = |indices: Vec<usize>| {
        let mut result = Vec::new();
        let mut placed: Vec<&str> = Vec::new();
        for &i in &indices {
            let Some(gid) = groups[i] else {
                result.push(i);
                continue;
            };
            if placed.contains(&gid) {
                // Already emitted with its group's block.
                continue;
            }
            placed.push(gid);
            result.extend(indices.iter().copied().filter(|&j| groups[j] == Some(gid)));
        }
        result
    };
    let is_pinned = |i: usize| pinned.get(i).copied().unwrap_or(false);
    let mut out = compacted((0..groups.len()).filter(|&i| is_pinned(i)).collect());
    out.extend(compacted((0..groups.len()).filter(|&i| !is_pinned(i)).collect()));
    out
}

/// `resolvedGroup(forTabAt:order:)` (`CON`): the group a just-dropped tab belongs to. A drop
/// moves exactly one tab without changing any membership, so just that tab is resolved from
/// where it landed:
///   - strictly between two members of the same group: join that group (how an ungrouped
///     tab is added, or a tab moves between groups);
///   - otherwise (at a group's edge, or not near a group): leave any group, so dragging a
///     member out removes it, including a one-tab group's sole member, whose group dissolves
///     when its last tab is dragged out.
/// Only the dragged tab's membership ever changes, so neighbors are never absorbed.
pub fn resolved_group(index: usize, order: &[Option<&str>]) -> Option<String> {
    if index >= order.len() {
        return None;
    }
    let left = if index > 0 { order[index - 1] } else { None };
    let right = order.get(index + 1).copied().flatten();
    // Strictly inside a run of a single group: join it.
    match left {
        Some(l) if Some(l) == right => Some(l.to_string()),
        // At a group's edge or away from any group: not a member.
        _ => None,
    }
}

/// `indexOfNearestTabOutsideGroup` (`ORD`): the nearest VISIBLE tab OUTSIDE `group`, used
/// when collapsing a group that holds the active tab: selection must move out of the group
/// first, so the active tab is never in a collapsed group. A hidden member of some OTHER
/// collapsed group is not a valid landing spot (selecting it would expand that group and
/// yank focus into it), so `collapsed` (parallel to `order`; empty treats every tab as
/// visible) skips such tabs. None when the group is the whole window or every tab outside
/// it is hidden (the collapse is refused).
///
/// A group is usually contiguous, but `canonical_order` leaves a group straddling the pinned
/// boundary as up to two runs, so a valid landing spot can lie between the first and last
/// member. Every index is scanned for the visible non-member nearest any member (distance to
/// the closest member); among equal distances the later tab wins, matching the
/// contiguous-group behavior of landing just after the group's last member.
pub fn nearest_outside(order: &[Option<&str>], collapsed: &[bool], group: &str) -> Option<usize> {
    let members: Vec<usize> = (0..order.len()).filter(|&i| order[i] == Some(group)).collect();
    if members.is_empty() {
        // No such group.
        return None;
    }
    let visible_outside = |i: usize| order[i] != Some(group) && !collapsed.get(i).copied().unwrap_or(false);
    let mut best = None;
    let mut best_distance = usize::MAX;
    for i in (0..order.len()).filter(|&i| visible_outside(i)) {
        let distance = members.iter().map(|&m| m.abs_diff(i)).min().unwrap_or(usize::MAX);
        // Iterating ascending, a later tab with equal distance replaces the earlier one.
        if distance <= best_distance {
            best = Some(i);
            best_distance = distance;
        }
    }
    best
}

/// `tabGroupIsWholeWindow:` (`PT:9720-9723`): every tab of the window is in `group`, so it
/// cannot collapse.
pub fn whole_window(order: &[Option<&str>], group: &str) -> bool {
    !order.is_empty() && order.iter().all(|g| *g == Some(group))
}

/// One single-tab keyboard move (`SingleTabMove`, `ORD`): the new tab order (a permutation
/// of input indices) plus, when the selected tab's membership changes, its new group id
/// (None = ungrouped). When `changes_membership` is false the caller only reorders; `order`
/// is the identity for a pure membership change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SingleMove {
    pub order: Vec<usize>,
    pub new_group: Option<String>,
    pub changes_membership: bool,
}

/// `singleTabMove` (`ORD`, Move Tab Left/Right): one step of moving a single tab one
/// position in `offset`'s direction (+1 = right, -1 = left), with group membership changing
/// at boundaries so the tab enters or exits a group instead of the whole group moving as a
/// unit. Pinning is ignored; `canonical_order` enforces the invariants afterward. Rules for
/// the tab at `selected` (group `g`), looking at the neighbor it would cross:
///   - g is set with other members, the neighbor is the same group: swap within the group.
///   - g is set with other members, the neighbor differs or the tab is at the group's edge
///     in the travel direction: leave the group (membership to None, no move).
///   - g is set and the tab is the group's ONLY member: it moves as a one-tab unit, keeping
///     its group. It jumps the adjacent unit (an ungrouped tab, a swap, or a whole group,
///     leaping the block to keep both groups contiguous) or wraps at the end.
///   - g is None, the neighbor is in a group: join that group (no move).
///   - g is None, the neighbor is ungrouped: swap.
///   - g is None and there is no neighbor (the tab is at the end): wrap to the far end.
pub fn single_tab_move(order: &[Option<&str>], selected: usize, offset: i32) -> SingleMove {
    let n = order.len();
    let identity: Vec<usize> = (0..n).collect();
    let moved = |order: Vec<usize>| SingleMove { order, new_group: None, changes_membership: false };
    if n < 2 || selected >= n || offset == 0 {
        return moved(identity);
    }
    let s = selected;
    let forward = offset > 0;
    let neighbor = if forward { s.checked_add(1).filter(|&x| x < n) } else { s.checked_sub(1) };
    let g = order[s].filter(|g| !g.is_empty());
    let sole_member = g.is_some() && order.iter().filter(|x| **x == g).count() == 1;

    // Wrap an end tab (any kind) to the opposite end, keeping its membership.
    let wrapped = || {
        let mut o = identity.clone();
        o.remove(s);
        o.insert(if forward { 0 } else { n - 1 }, s);
        moved(o)
    };

    if g.is_some() && !sole_member {
        if let Some(nb) = neighbor.filter(|&nb| order[nb] == g) {
            // Move within the group.
            let mut o = identity;
            o.swap(s, nb);
            return moved(o);
        }
        // At the group's edge in the travel direction: leave the group in place.
        return SingleMove { order: identity, new_group: None, changes_membership: true };
    }

    let Some(nb) = neighbor else {
        return wrapped();
    };
    let ng = order[nb].filter(|g| !g.is_empty());

    if sole_member {
        // A one-tab group moves as a unit, keeping its group. Span the adjacent unit (a
        // whole group's run, or a single ungrouped tab), then place the tab past it.
        let mut o = identity;
        o.remove(s);
        match ng {
            Some(ng) if forward => {
                let mut e = nb;
                while e < n && order[e] == Some(ng) {
                    e += 1;
                }
                // Just past the group's last member.
                o.insert(e - 1, s);
            }
            Some(ng) => {
                let mut b = nb as isize;
                while b >= 0 && order[b as usize] == Some(ng) {
                    b -= 1;
                }
                // Just before the group's first member.
                o.insert((b + 1) as usize, s);
            }
            // Swap with the lone neighbor.
            None => o.insert(nb, s),
        }
        return moved(o);
    }

    // An ungrouped tab.
    if let Some(ng) = ng {
        // Crossing into a group: join it, in place.
        return SingleMove { order: identity, new_group: Some(ng.to_string()), changes_membership: true };
    }
    // Both ungrouped: a plain swap.
    let mut o = identity;
    o.swap(s, nb);
    moved(o)
}

/// `selectTabGroupByOffset:` (`PT:2926-2950`): the tab Select Next (offset > 0) or Previous
/// (offset < 0) Tab Group selects. When the selected tab is in a group, the step skips over
/// the other members of that group (they are contiguous, so stepping until leaving the group
/// lands on the first tab outside it); an ungrouped tab behaves like Next/Previous Tab.
/// Wraps. None with fewer than 2 tabs.
pub fn select_by_offset(order: &[Option<&str>], selected: usize, offset: i32) -> Option<usize> {
    let count = order.len() as i64;
    if count < 2 || selected >= order.len() {
        return None;
    }
    let sel = selected as i64;
    let step = |t: i64| (t + offset as i64).rem_euclid(count);
    let mut target = step(sel);
    if let Some(gid) = order[selected] {
        while target != sel && order[target as usize] == Some(gid) {
            target = step(target);
        }
    }
    Some(target as usize)
}

/// `normalizeTabGroupCollapsedState` (`PT:9690-9713`): each group forced to one collapsed
/// state honoring "the active tab is never in a collapsed group" (`PT:9667-9672`). A group
/// is collapsed only when every member is collapsed and it does not hold the active tab;
/// otherwise it is fully expanded. An ungrouped tab keeps its flag.
pub fn normalized_collapsed(order: &[Option<&str>], collapsed: &[bool], active: usize) -> Vec<bool> {
    let is = |i: usize| collapsed.get(i).copied().unwrap_or(false);
    (0..order.len())
        .map(|i| match order[i] {
            Some(gid) if !gid.is_empty() => {
                let members = || (0..order.len()).filter(|&j| order[j] == Some(gid));
                members().all(is) && order.get(active).copied().flatten() != Some(gid)
            }
            _ => is(i),
        })
        .collect()
}

/// The new group prompt's title (`PT:8958`).
pub const NEW_GROUP_TITLE: &str = "New Tab Group";
/// The rename prompt's title (`PT:9747`).
pub const RENAME_GROUP_TITLE: &str = "Rename Tab Group";
/// Both prompts' text (`PT:8996`).
pub const GROUP_PROMPT: &str = "Enter a name for this tab group.";
/// A new group's name when the tab has no label (`PT:8957`).
pub const DEFAULT_GROUP_NAME: &str = "Group";
/// An empty group name in a menu of groups (`PT:8912`).
pub const UNTITLED: &str = "Untitled";

/// The name a new group's prompt starts with (`PT addTab:toNewGroupWithDefaultName:`,
/// `PT:8953-8967`, `PT:8993-9004`): the tab's label, "Group" when it is empty, trimmed of
/// white space and line breaks (a label ends in a line break).
pub fn default_name(label: &str) -> String {
    let name = if label.is_empty() { DEFAULT_GROUP_NAME } else { label };
    name.trim().to_string()
}

/// Which tabs a group menu's close command closes (`PT closeTabGroup:`,
/// `closeTabsOutsideGroup:`, `closeTabsRightOfGroup:`, `PT:9845-9888`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseTabs {
    /// Close Group: every member, pinned ones too.
    Group,
    /// Close Other Tabs: every tab outside the group but the pinned ones.
    Outside,
    /// Close Tabs to the Right (Below on a side bar): every tab after the group's last
    /// member but the pinned ones.
    Right,
}

/// The heading of the one question those commands ask (`PT:9906`).
pub const CLOSE_TABS_HEADING: &str = "Close Tabs";

/// That question's buttons (`PT:9902`) and identifier, a persistent warning, never remembered
/// (`PT:9904-9905`).
pub const CLOSE_TABS_BUTTONS: [&str; 2] = ["Close", "Cancel"];
pub const CLOSE_TABS_ID: &str = "NoSyncCloseTabGroup";

/// The question the group close commands ask once for `count` tabs (`PT closeTabs:`,
/// `PT:9890-9917`): "<question> (N tabs)".
pub fn close_question(what: CloseTabs, vertical: bool, count: usize) -> String {
    let question = match what {
        CloseTabs::Group => "Close this tab group?",
        CloseTabs::Outside => "Close all tabs outside this group?",
        CloseTabs::Right if vertical => "Close all tabs below this group?",
        CloseTabs::Right => "Close all tabs to the right of this group?",
    };
    format!("{question} ({count} tabs)")
}

/// A duplicated group's name (`PT duplicateTabGroup:`, `PT:9808-9809`): "<name> copy".
pub fn copy_name(name: &str) -> String {
    format!("{name} copy")
}

/// `nextTabGroupColor`'s palette (`PT:9149-9166`): system blue, green, orange, purple and
/// red, #50afbc, and system pink.
pub const PALETTE: [Rgb; 7] = [
    rgb(0, 122, 255),
    rgb(52, 199, 89),
    rgb(255, 149, 0),
    rgb(175, 82, 222),
    rgb(255, 59, 48),
    rgb(0x50, 0xaf, 0xbc),
    rgb(255, 45, 85),
];

/// `didChangePinnedState` (`PTS:400-416`): where the tab at `index` moves when its pin
/// changes, keeping pinned tabs in front: one past the last pinned tab other than itself.
/// The index is `moveTabAtIndex:toIndex:`'s (the tab is taken out, then put back there).
pub fn pin_target(pinned: &[bool], index: usize) -> usize {
    pinned.iter().enumerate().filter(|&(i, p)| *p && i != index).map(|(i, _)| i + 1).last().unwrap_or(0)
}

/// `togglePinTab` (`PTS:355-397`) applied to an order: the tabs `members` (input indices, in
/// tab order) take the pin state `pin`. Each one whose state changes moves as
/// `didChangePinnedState` moves it (`PTS:400-416`): taken out and put back at `pin_target`
/// (`PT moveTabAtIndex:toIndex:`), and the order settles after each move (`PT tabsDidReorder`,
/// `canonical_order`). A group so moves member by member, as a lone tab does. `result[k]` is
/// the input index of the tab that belongs at position k.
pub fn apply_pin(groups: &[Option<&str>], pinned: &[bool], members: &[usize], pin: bool) -> Vec<usize> {
    let mut flags = pinned.to_vec();
    let mut order: Vec<usize> = (0..groups.len()).collect();
    for &m in members {
        if flags.get(m).is_none_or(|p| *p == pin) {
            continue;
        }
        flags[m] = pin;
        let Some(at) = order.iter().position(|&i| i == m) else { continue };
        let now: Vec<bool> = order.iter().map(|&i| flags[i]).collect();
        let to = pin_target(&now, at).min(order.len() - 1);
        order.remove(at);
        order.insert(to, m);
        let g: Vec<Option<&str>> = order.iter().map(|&i| groups[i]).collect();
        let p: Vec<bool> = order.iter().map(|&i| flags[i]).collect();
        order = canonical_order(&g, &p).into_iter().map(|k| order[k]).collect();
    }
    order
}

/// `nextTabGroupColor` (`PT:9149-9166`): a new group's color, cycling the palette by the
/// number of groups already in the window so consecutive groups look distinct.
pub fn next_color(groups_in_window: usize) -> Rgb {
    PALETTE[groups_in_window % PALETTE.len()]
}

/// `tabGroupsInWindow` (`PT:9024-9041`): the distinct groups of a window's tabs, in order of
/// first appearance; the first tab carrying each id supplies the name and color. A tab with
/// an empty id belongs to no group.
pub fn groups_in_window<'a>(tabs: impl Iterator<Item = Option<&'a TabGroup>>) -> Vec<TabGroup> {
    let mut out: Vec<TabGroup> = Vec::new();
    for g in tabs.flatten() {
        if !g.id.is_empty() && !out.iter().any(|o| o.id == g.id) {
            out.push(g.clone());
        }
    }
    out
}

/// `reconcileTabGroupDefinitionForTab:` (`PT:9381-9409`): make `tab`'s carried group
/// definition match the group it now belongs to, copying from any other member (`others`:
/// the window's other tabs' groups and pinned flags). A tab in no group keeps none. A tab
/// that is the sole member of its id keeps what it has (it is the group's definer). The
/// joining tab adopts the group's collapsed state, and its pinned state too, since a group
/// must be entirely pinned or entirely unpinned. Call after any change to a tab's group id.
pub fn reconcile(tab: &mut Option<TabGroup>, pinned: &mut bool, others: &[(Option<&TabGroup>, bool)]) {
    let Some(g) = tab.as_mut() else {
        return;
    };
    if g.id.is_empty() {
        *tab = None;
        return;
    }
    if let Some((m, member_pinned)) = others.iter().find_map(|(o, p)| o.filter(|o| o.id == g.id).map(|o| (o, *p))) {
        g.name = m.name.clone();
        g.color = m.color;
        g.collapsed = m.collapsed;
        *pinned = member_pinned;
    }
}

/// A tab carrying `current` joins the group `id` (None: leaves any group), with its pinned
/// state `pinned`, among the window's other tabs `others`: it starts from its own definition
/// when it already carries that id (a new group's name and color, set before it joins), else
/// from a blank one, then `reconcile`s. Answers its group and pinned state.
pub fn join(current: Option<&TabGroup>, id: Option<String>, pinned: bool, others: &[(Option<&TabGroup>, bool)]) -> (Option<TabGroup>, bool) {
    let mut group = id.map(|id| match current.filter(|g| g.id == id) {
        Some(g) => g.clone(),
        None => TabGroup { id, name: String::new(), color: PALETTE[0], collapsed: false },
    });
    let mut pinned = pinned;
    reconcile(&mut group, &mut pinned, others);
    (group, pinned)
}

/// `groupIDToInheritForNewTabAtIndex:` (`PT:14377-14390`): the group a new tab that landed at
/// `landed` joins: the selected tab's (at `selected`, in `group`) when it landed right after
/// it, so it extends that group's run; else none.
pub fn inherited_group(selected: usize, group: Option<&str>, landed: usize) -> Option<String> {
    group.filter(|g| !g.is_empty() && landed == selected + 1).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Option<&str> = Some("a");
    const B: Option<&str> = Some("b");
    const N: Option<&str> = None;

    fn apply<'a>(groups: &[Option<&'a str>], pinned: &[bool], order: &[usize]) -> (Vec<Option<&'a str>>, Vec<bool>) {
        (order.iter().map(|&i| groups[i]).collect(), order.iter().map(|&i| pinned[i]).collect())
    }

    fn identity(n: usize) -> Vec<usize> {
        (0..n).collect()
    }

    fn group(id: &str, name: &str) -> TabGroup {
        TabGroup { id: id.into(), name: name.into(), color: PALETTE[0], collapsed: false }
    }

    #[test]
    fn canonical_order_follows_iterm2() {
        let cases: [(&[Option<&str>], &[bool], &[usize]); 3] = [
            (&[A, N, A, B, N, B], &[false; 6], &[0, 2, 1, 3, 5, 4]),
            (&[A, N, A, B, N, B], &[false, true, false, false, false, false], &[1, 0, 2, 3, 5, 4]),
            // A group straddling the pinned boundary keeps two runs.
            (&[A, A, N], &[true, false, false], &[0, 1, 2]),
        ];
        for (groups, pinned, want) in cases {
            let got = canonical_order(groups, pinned);
            assert_eq!(got, want, "{groups:?} {pinned:?}");
            let (g2, p2) = apply(groups, pinned, &got);
            assert_eq!(canonical_order(&g2, &p2), identity(groups.len()), "idempotent for {groups:?} {pinned:?}");
        }
        assert_eq!(canonical_order(&[], &[]), Vec::<usize>::new());
    }

    #[test]
    fn drop_membership_follows_iterm2() {
        assert_eq!(resolved_group(1, &[A, N, A]), Some("a".into()));
        assert_eq!(resolved_group(0, &[A, A]), None, "at the edge");
        assert_eq!(resolved_group(2, &[A, A, N]), None);
        assert_eq!(resolved_group(1, &[A, N, B]), None, "neighbors differ");
        assert_eq!(resolved_group(1, &[A, B, A]), Some("a".into()), "a tab dropped inside another group's run moves to it (`CON`)");
        assert_eq!(resolved_group(5, &[A, N, A]), None, "out of range");
        assert_eq!(resolved_group(0, &[A]), None, "a one-tab group's member dragged out dissolves it");
    }

    #[test]
    fn nearest_outside_follows_iterm2() {
        assert_eq!(nearest_outside(&[N, A, A, N], &[], "a"), Some(3), "a tie at distance 1: the later wins");
        assert_eq!(nearest_outside(&[N, A, A], &[], "a"), Some(0));
        assert_eq!(nearest_outside(&[A, A], &[], "a"), None);
        assert_eq!(nearest_outside(&[B, A, N], &[true, false, false], "a"), Some(2), "a hidden member of another collapsed group is skipped");
        assert_eq!(nearest_outside(&[B, A], &[true, false], "a"), None);
        assert_eq!(nearest_outside(&[N], &[], "z"), None);
        assert!(whole_window(&[A, A], "a"));
        assert!(!whole_window(&[A, N], "a"));
    }

    #[test]
    fn single_tab_move_follows_iterm2() {
        let o = [N, A, A, N];
        let mv = |order: Vec<usize>, new_group: Option<&str>, changes: bool| SingleMove { order, new_group: new_group.map(str::to_string), changes_membership: changes };
        assert_eq!(single_tab_move(&o, 1, -1), mv(identity(4), None, true), "leaves at the edge");
        assert_eq!(single_tab_move(&o, 1, 1), mv(vec![0, 2, 1, 3], None, false), "swaps within");
        assert_eq!(single_tab_move(&o, 0, 1), mv(identity(4), Some("a"), true), "joins in place");
        assert_eq!(single_tab_move(&o, 3, 1), mv(vec![3, 0, 1, 2], None, false), "wraps to the front");
        assert_eq!(single_tab_move(&[B, A, A], 0, 1), mv(vec![1, 2, 0], None, false), "a sole member jumps the whole group");
        assert_eq!(single_tab_move(&[N, N], 0, 1), mv(vec![1, 0], None, false));
        assert_eq!(single_tab_move(&[N], 0, 1), mv(identity(1), None, false));
        assert_eq!(single_tab_move(&o, 1, 0), mv(identity(4), None, false));
    }

    #[test]
    fn select_group_follows_iterm2() {
        assert_eq!(select_by_offset(&[A, A, N, B], 0, 1), Some(2));
        assert_eq!(select_by_offset(&[A, A, N, B], 1, -1), Some(3), "wraps, skipping the group");
        assert_eq!(select_by_offset(&[N, N], 0, -1), Some(1));
        assert_eq!(select_by_offset(&[A], 0, 1), None);
        assert_eq!(normalized_collapsed(&[A, A, B], &[true, false, true], 2), vec![false, false, false], "a mixed group expands; b holds the active tab");
        assert_eq!(normalized_collapsed(&[A, A, N], &[true, true, false], 2), vec![true, true, false]);
    }

    #[test]
    fn pin_moves_follow_iterm2() {
        assert_eq!(pin_target(&[true, false, false], 2), 1, "pinning moves the tab right after the pinned zone");
        assert_eq!(pin_target(&[true, true, false], 0), 2, "unpinning tab 0: its own pin is excluded");
        assert_eq!(pin_target(&[false, false], 1), 0);
        // Unpinning moves each member as a lone tab moves (`PTS:382-392`).
        assert_eq!(apply_pin(&[A, N, N], &[true, true, false], &[0], false), vec![1, 2, 0], "[G1 P, X P, U1] unpinning G1 gives [X, U1, G1]");
        assert_eq!(apply_pin(&[A, A, N, N], &[true, true, true, false], &[0, 1], false), vec![2, 3, 1, 0], "[G1 P, G2 P, X P, U1] gives [X, U1, G2, G1]");
        assert_eq!(apply_pin(&[N, N, N, N], &[true, true, false, false], &[0], false), vec![1, 2, 0, 3], "a lone tab lands after the first unpinned tab");
        assert_eq!(apply_pin(&[N, A, A], &[false, false, false], &[1, 2], true), vec![1, 2, 0], "pinning a group keeps its order in front");
        assert_eq!(apply_pin(&[N, N], &[true, false], &[0], true), vec![0, 1], "no change, no move");
    }

    #[test]
    fn group_commands_follow_iterm2() {
        assert_eq!(default_name("vim\n"), "vim", "trimmed");
        assert_eq!(default_name(""), "Group");
        assert_eq!(default_name("  "), "", "a label of spaces is not empty, so it seeds the field and trims to nothing (`PT:8958`, `:9004`)");
        assert_eq!(NEW_GROUP_TITLE, "New Tab Group");
        assert_eq!(RENAME_GROUP_TITLE, "Rename Tab Group");
        assert_eq!(GROUP_PROMPT, "Enter a name for this tab group.");
        assert_eq!(DEFAULT_GROUP_NAME, "Group");
        assert_eq!(UNTITLED, "Untitled");
        assert_eq!(close_question(CloseTabs::Group, false, 3), "Close this tab group? (3 tabs)");
        assert_eq!(close_question(CloseTabs::Outside, false, 1), "Close all tabs outside this group? (1 tabs)");
        assert_eq!(close_question(CloseTabs::Right, false, 2), "Close all tabs to the right of this group? (2 tabs)");
        assert_eq!(close_question(CloseTabs::Right, true, 2), "Close all tabs below this group? (2 tabs)");
        assert_eq!(CLOSE_TABS_HEADING, "Close Tabs");
        assert_eq!((CLOSE_TABS_BUTTONS, CLOSE_TABS_ID), (["Close", "Cancel"], "NoSyncCloseTabGroup"));
        assert_eq!(copy_name("Build"), "Build copy");
    }

    #[test]
    fn palette_follows_iterm2() {
        assert_eq!(PALETTE, [rgb(0, 122, 255), rgb(52, 199, 89), rgb(255, 149, 0), rgb(175, 82, 222), rgb(255, 59, 48), rgb(0x50, 0xaf, 0xbc), rgb(255, 45, 85)]);
        assert_eq!(next_color(0), rgb(0, 122, 255));
        assert_eq!(next_color(7), rgb(0, 122, 255));
        assert_eq!(next_color(5), rgb(0x50, 0xaf, 0xbc));
        let (x, y, b) = (group("a", "x"), group("a", "y"), group("b", ""));
        let got = groups_in_window([Some(&x), None, Some(&y), Some(&b)].into_iter());
        assert_eq!(got.len(), 2);
        assert_eq!((got[0].id.as_str(), got[0].name.as_str()), ("a", "x"));
        assert_eq!(got[1].id, "b");
    }

    #[test]
    fn reconcile_follows_iterm2() {
        let mut tab = Some(group("a", "old"));
        let mut pinned = false;
        let other = TabGroup { name: "new".into(), color: PALETTE[3], collapsed: true, ..group("a", "") };
        reconcile(&mut tab, &mut pinned, &[(Some(&other), true)]);
        let t = tab.unwrap();
        assert_eq!((t.name.as_str(), t.color, t.collapsed, pinned), ("new", PALETTE[3], true, true));

        let mut none = None;
        let mut pinned = true;
        reconcile(&mut none, &mut pinned, &[(Some(&other), false)]);
        assert_eq!((none, pinned), (None, true), "no group: nothing carried, the pin untouched");

        let mut sole = Some(TabGroup { collapsed: true, ..group("s", "mine") });
        let mut pinned = false;
        reconcile(&mut sole, &mut pinned, &[(Some(&other), true), (None, true)]);
        assert_eq!((sole, pinned), (Some(TabGroup { collapsed: true, ..group("s", "mine") }), false), "a sole member keeps its own definition");
    }

    #[test]
    fn joining_keeps_a_new_groups_definition() {
        // Add Tab to New Group: the tab already carries the new group's name and color, and
        // no other tab has the id, so the definition stays.
        let made = TabGroup { name: "Build".into(), color: PALETTE[2], ..group("n", "") };
        assert_eq!(join(Some(&made), Some("n".into()), false, &[(None, false)]), (Some(made.clone()), false));
        // Joining an existing group takes the members' definition and pin state.
        let member = TabGroup { name: "Web".into(), color: PALETTE[4], collapsed: true, ..group("w", "") };
        let (g, pinned) = join(Some(&made), Some("w".into()), false, &[(Some(&member), true)]);
        assert_eq!((g, pinned), (Some(member.clone()), true));
        // A tab in no group that joins an id nobody else has starts blank.
        assert_eq!(join(None, Some("z".into()), false, &[]).0.map(|g| (g.id, g.name, g.color)), Some(("z".into(), String::new(), PALETTE[0])));
        assert_eq!(join(Some(&made), None, true, &[]), (None, true), "leaving: no group");
    }

    #[test]
    fn new_tab_inherits_the_group_it_lands_after() {
        // `groupIDToInheritForNewTabAtIndex:` (`PT:14377-14390`): Win+T from a group's last
        // tab (New Tab appends) lands right after it and joins.
        assert_eq!(inherited_group(2, Some("g"), 3), Some("g".to_string()));
        assert_eq!(inherited_group(1, Some("g"), 3), None, "not right after the selected tab");
        assert_eq!(inherited_group(2, None, 3), None, "the selected tab is in no group");
        assert_eq!(inherited_group(2, Some(""), 3), None);
    }
}

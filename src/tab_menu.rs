//! The tab and tab group context menus as data (milestone 13d): a right press on a tab gives
//! `tab_menu` (`PT tabView:menuForTabViewItem:`, `PT:8253-8500`), one on a group's chip gives
//! `group_menu` (`PT tabView:menuForTabGroup:`, `PT:9427-9478`). Pure code; `app::tab_groups_ui`
//! shows them and runs the picks.
//!
//! Ids are `tab-menu:<action>:<tab id>` and `tab-group-menu:<action>:<group id>`; `parse`
//! splits them. A pick of a group from Add Tab to Group is `tab-menu:join:<tab id>:<group id>`.
//!
//! Citations: `PT` = `sources/TerminalView/PseudoTerminal.m` under `~/gitrepos/iterm2-ref`.

/// One entry of a context menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    /// A command: its menu id, its label, whether it can be picked, and its check mark (None
    /// for an item that has none).
    Item { id: String, label: String, enabled: bool, checked: Option<bool> },
    Submenu { label: String, entries: Vec<Entry> },
    Separator,
}

/// What the tab menu is built from.
#[derive(Clone, Debug)]
pub struct TabCtx {
    /// The tab the menu is for (its id).
    pub tab: u64,
    /// Every tab's label, in tab order.
    pub labels: Vec<String>,
    /// Every tab's id, in tab order ("Select" picks by id).
    pub ids: Vec<u64>,
    /// The tab's index.
    pub index: usize,
    /// Every tab's pinned flag, in tab order.
    pub pinned: Vec<bool>,
    /// The tab is a tmux tab.
    pub tmux: bool,
    /// The tab bar is on a side ("Below" for "to the Right", `PT:8245-8249`).
    pub vertical: bool,
    /// The tab's group id.
    pub group: Option<String>,
    /// The window's groups in order, as (id, name).
    pub groups: Vec<(String, String)>,
    /// Lock Layout is on for the window.
    pub layout_locked: bool,
}

pub const TAB_PREFIX: &str = "tab-menu:";
pub const GROUP_PREFIX: &str = "tab-group-menu:";

/// The label of a group with no name (`PT:8906`).
pub const UNTITLED: &str = "Untitled";

/// Appends a separator unless the menu is empty or already ends with one (`addSeparator`,
/// `PT:8302-8308`).
fn separate(menu: &mut Vec<Entry>) {
    if !matches!(menu.last(), None | Some(Entry::Separator)) {
        menu.push(Entry::Separator);
    }
}

fn item(id: String, label: &str, enabled: bool) -> Entry {
    Entry::Item { id, label: label.to_string(), enabled, checked: None }
}

/// The tab's context menu (`PT:8253-8500`) in iTerm2's sections: Select (more than one tab),
/// Edit Session\u{2026}; New Tab to the Right (Below on a side bar) and Duplicate Tab (not
/// tmux); Close Tab, Close Other Tabs and Close Tabs to the Right when unpinned tabs qualify;
/// Move to New Window (more than one tab, not pinned) and Pin or Unpin Tab (not tmux); Add Tab
/// to Group with Rename Group\u{2026} and Remove Tab from Group for a grouped tab
/// (`PT:8894-8946`); Save Tab as Window Arrangement; Tab Color. Under Lock Layout the items
/// that change the tabs are disabled (`PT validateMenuItem:`, `PT:13831-13852`).
pub fn tab_menu(c: &TabCtx) -> Vec<Entry> {
    let id = |action: &str| format!("{TAB_PREFIX}{action}:{}", c.tab);
    let free = !c.layout_locked;
    let mut menu = Vec::new();
    if c.labels.len() > 1 {
        let entries = c
            .labels
            .iter()
            .zip(&c.ids)
            .enumerate()
            .map(|(n, (label, tab))| item(format!("{TAB_PREFIX}select:{tab}"), &format!("{label} #{}", n + 1), true))
            .collect();
        menu.push(Entry::Submenu { label: "Select".into(), entries });
        menu.push(Entry::Separator);
    }
    separate(&mut menu);
    menu.push(item(id("edit-session"), "Edit Session\u{2026}", true));
    separate(&mut menu);
    menu.push(item(id("new-tab"), if c.vertical { "New Tab Below" } else { "New Tab to the Right" }, free));
    if !c.tmux {
        menu.push(item(id("duplicate"), "Duplicate Tab", free));
    }
    separate(&mut menu);
    menu.push(item(id("close"), "Close Tab", free));
    let unpinned_other = (0..c.pinned.len()).any(|i| i != c.index && !c.pinned[i]);
    let unpinned_after = (c.index + 1..c.pinned.len()).any(|i| !c.pinned[i]);
    if unpinned_other {
        menu.push(item(id("close-others"), "Close Other Tabs", free));
    }
    if unpinned_after {
        menu.push(item(id("close-right"), if c.vertical { "Close Tabs Below" } else { "Close Tabs to the Right" }, free));
    }
    separate(&mut menu);
    let pinned = c.pinned.get(c.index).copied().unwrap_or(false);
    if c.labels.len() > 1 && !pinned {
        menu.push(item(id("move-window"), "Move to New Window", free));
    }
    if !c.tmux {
        menu.push(item(id("pin"), if pinned { "Unpin Tab" } else { "Pin Tab" }, true));
    }
    separate(&mut menu);
    let mut entries = vec![item(id("new-group"), "New Group\u{2026}", true)];
    if !c.groups.is_empty() {
        entries.push(Entry::Separator);
        for (gid, name) in &c.groups {
            let label = if name.is_empty() { UNTITLED } else { name.as_str() };
            let checked = Some(c.group.as_deref() == Some(gid.as_str()));
            entries.push(Entry::Item { id: format!("{TAB_PREFIX}join:{}:{gid}", c.tab), label: label.to_string(), enabled: true, checked });
        }
    }
    menu.push(Entry::Submenu { label: "Add Tab to Group".into(), entries });
    if c.group.is_some() {
        menu.push(item(id("rename-group"), "Rename Group\u{2026}", true));
        menu.push(item(id("remove-group"), "Remove Tab from Group", true));
    }
    separate(&mut menu);
    menu.push(item(id("save"), "Save Tab as Window Arrangement", true));
    separate(&mut menu);
    menu.push(item(id("color"), "Tab Color", true));
    menu
}

/// A group chip's context menu (`PT:9427-9478`): every item enabled (`autoenablesItems` off)
/// but Collapse Group for a group that is the whole window; Group Color right after Rename
/// Group\u{2026}; "Below" for "to the Right" on a side bar.
pub fn group_menu(group: &str, collapsed: bool, whole_window: bool, vertical: bool) -> Vec<Entry> {
    let id = |action: &str| format!("{GROUP_PREFIX}{action}:{group}");
    let on = |action: &str, label: &str| item(id(action), label, true);
    vec![
        on("new-tab", "New Tab in Group"),
        Entry::Separator,
        if collapsed { on("expand", "Expand Group") } else { item(id("collapse"), "Collapse Group", !whole_window) },
        Entry::Separator,
        on("rename", "Rename Group\u{2026}"),
        on("color", "Group Color"),
        Entry::Separator,
        on("duplicate", "Duplicate Group"),
        on("move-window", "Move Group to New Window"),
        on("save", "Save Group as Window Arrangement\u{2026}"),
        Entry::Separator,
        on("ungroup", "Remove All Tabs from Group"),
        Entry::Separator,
        on("close", "Close Group"),
        on("close-others", "Close Other Tabs"),
        on("close-right", if vertical { "Close Tabs Below" } else { "Close Tabs to the Right" }),
    ]
}

/// A tab or group menu id as (action, target): the target is everything after the action (a
/// tab id, a group id, or `<tab id>:<group id>` for a join). None for any other id.
pub fn parse(id: &str) -> Option<(String, String)> {
    let rest = id.strip_prefix(GROUP_PREFIX).or_else(|| id.strip_prefix(TAB_PREFIX))?;
    let (action, target) = rest.split_once(':')?;
    Some((action.to_string(), target.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(menu: &[Entry]) -> Vec<String> {
        menu.iter()
            .map(|e| match e {
                Entry::Item { label, .. } | Entry::Submenu { label, .. } => label.clone(),
                Entry::Separator => "-".into(),
            })
            .collect()
    }

    fn find<'a>(menu: &'a [Entry], label: &str) -> &'a Entry {
        menu.iter().find(|e| matches!(e, Entry::Item { label: l, .. } | Entry::Submenu { label: l, .. } if l == label)).unwrap_or_else(|| panic!("no {label}"))
    }

    fn ctx() -> TabCtx {
        TabCtx {
            tab: 1,
            labels: vec!["a".into(), "b".into(), "c".into()],
            ids: vec![1, 2, 3],
            index: 0,
            pinned: vec![false; 3],
            tmux: false,
            vertical: false,
            group: None,
            groups: Vec::new(),
            layout_locked: false,
        }
    }

    fn no_doubled_separators(menu: &[Entry]) {
        assert!(!matches!(menu.first(), Some(Entry::Separator)), "no leading separator");
        assert!(!matches!(menu.last(), Some(Entry::Separator)), "no trailing separator");
        assert!(!menu.windows(2).any(|w| w[0] == Entry::Separator && w[1] == Entry::Separator), "no doubled separator");
    }

    #[test]
    fn tab_menu_follows_iterm2() {
        let menu = tab_menu(&ctx());
        assert_eq!(
            labels(&menu),
            [
                "Select", "-", "Edit Session\u{2026}", "-", "New Tab to the Right", "Duplicate Tab", "-", "Close Tab", "Close Other Tabs",
                "Close Tabs to the Right", "-", "Move to New Window", "Pin Tab", "-", "Add Tab to Group", "-", "Save Tab as Window Arrangement", "-",
                "Tab Color",
            ]
        );
        no_doubled_separators(&menu);
        let Entry::Submenu { entries, .. } = find(&menu, "Select") else { panic!() };
        assert_eq!(labels(entries), ["a #1", "b #2", "c #3"]);
        assert!(matches!(&entries[2], Entry::Item { id, .. } if id == "tab-menu:select:3"), "selects by tab id");
        let Entry::Submenu { entries, .. } = find(&menu, "Add Tab to Group") else { panic!() };
        assert_eq!(labels(entries), ["New Group\u{2026}"]);
        assert_eq!(find(&menu, "Tab Color"), &Entry::Item { id: "tab-menu:color:1".into(), label: "Tab Color".into(), enabled: true, checked: None });

        // One tab: no Select, no Move to New Window, no Close Other Tabs.
        let one = TabCtx { labels: vec!["a".into()], ids: vec![1], pinned: vec![false], ..ctx() };
        let menu = tab_menu(&one);
        let l = labels(&menu);
        for gone in ["Select", "Move to New Window", "Close Other Tabs", "Close Tabs to the Right"] {
            assert!(!l.contains(&gone.to_string()), "{gone}");
        }
        assert_eq!(l[0], "Edit Session\u{2026}");
        no_doubled_separators(&menu);

        // A side bar says Below.
        let l = labels(&tab_menu(&TabCtx { vertical: true, ..ctx() }));
        assert!(l.contains(&"New Tab Below".to_string()) && l.contains(&"Close Tabs Below".to_string()));
        assert!(!l.contains(&"New Tab to the Right".to_string()));

        // Pinned: Unpin Tab, no Move to New Window.
        let l = labels(&tab_menu(&TabCtx { pinned: vec![true, false, false], ..ctx() }));
        assert!(l.contains(&"Unpin Tab".to_string()) && !l.contains(&"Move to New Window".to_string()));
        // Close Other Tabs and Close Tabs to the Right only when unpinned tabs qualify.
        let l = labels(&tab_menu(&TabCtx { pinned: vec![false, true, true], ..ctx() }));
        assert!(!l.contains(&"Close Other Tabs".to_string()) && !l.contains(&"Close Tabs to the Right".to_string()));
        let l = labels(&tab_menu(&TabCtx { index: 2, tab: 3, ..ctx() }));
        assert!(l.contains(&"Close Other Tabs".to_string()) && !l.contains(&"Close Tabs to the Right".to_string()), "the last tab");

        // tmux: no Duplicate Tab and no pin item.
        let menu = tab_menu(&TabCtx { tmux: true, ..ctx() });
        let l = labels(&menu);
        assert!(!l.contains(&"Duplicate Tab".to_string()) && !l.contains(&"Pin Tab".to_string()));
        no_doubled_separators(&menu);

        // Grouped: the groups after New Group, checked for its own, "Untitled" for no name;
        // then Rename Group and Remove Tab from Group.
        let grouped = TabCtx { group: Some("G1".into()), groups: vec![("G1".into(), "Build".into()), ("G2".into(), String::new())], ..ctx() };
        let menu = tab_menu(&grouped);
        let Entry::Submenu { entries, .. } = find(&menu, "Add Tab to Group") else { panic!() };
        assert_eq!(labels(entries), ["New Group\u{2026}", "-", "Build", "Untitled"]);
        assert!(matches!(&entries[2], Entry::Item { checked: Some(true), id, .. } if id == "tab-menu:join:1:G1"));
        assert!(matches!(&entries[3], Entry::Item { checked: Some(false), .. }));
        let l = labels(&menu);
        let at = l.iter().position(|x| x == "Add Tab to Group").unwrap();
        assert_eq!(&l[at + 1..at + 3], ["Rename Group\u{2026}", "Remove Tab from Group"]);

        // Lock Layout disables what changes the tabs (Close Tab too, `PT:13836`).
        let menu = tab_menu(&TabCtx { layout_locked: true, ..ctx() });
        for label in ["New Tab to the Right", "Duplicate Tab", "Close Tab", "Close Other Tabs", "Close Tabs to the Right", "Move to New Window"] {
            assert!(matches!(find(&menu, label), Entry::Item { enabled: false, .. }), "{label}");
        }
        for label in ["Edit Session\u{2026}", "Pin Tab", "Save Tab as Window Arrangement", "Tab Color"] {
            assert!(matches!(find(&menu, label), Entry::Item { enabled: true, .. }), "{label}");
        }
        assert_eq!(parse("tab-menu:join:1:G1"), Some(("join".into(), "1:G1".into())));
        assert_eq!(parse("tab-menu:close:7"), Some(("close".into(), "7".into())));
    }

    #[test]
    fn group_menu_follows_iterm2() {
        let menu = group_menu("g", false, false, false);
        assert_eq!(
            labels(&menu),
            [
                "New Tab in Group", "-", "Collapse Group", "-", "Rename Group\u{2026}", "Group Color", "-", "Duplicate Group", "Move Group to New Window",
                "Save Group as Window Arrangement\u{2026}", "-", "Remove All Tabs from Group", "-", "Close Group", "Close Other Tabs", "Close Tabs to the Right",
            ]
        );
        assert!(matches!(find(&menu, "Collapse Group"), Entry::Item { enabled: true, id, .. } if id == "tab-group-menu:collapse:g"));
        assert!(matches!(find(&menu, "Group Color"), Entry::Item { id, .. } if id == "tab-group-menu:color:g"));
        assert!(labels(&group_menu("g", true, false, false)).contains(&"Expand Group".to_string()));
        assert!(matches!(find(&group_menu("g", false, true, false), "Collapse Group"), Entry::Item { enabled: false, .. }), "a whole-window group");
        assert_eq!(labels(&group_menu("g", false, false, true)).last().unwrap(), "Close Tabs Below");
        assert_eq!(parse("tab-group-menu:collapse:g"), Some(("collapse".into(), "g".into())));
        assert_eq!(parse("tab-overflow:3"), None);
    }
}

//! The Profiles menu's tag tree and the profile shortcuts (`iTermProfilesMenuController.m`,
//! `ProfilesGeneralPreferencesViewController.m:1329-1418`).

use crate::profile::Profile;

/// One entry of the Profiles menu (after its fixed items).
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Profile { guid: String, name: String, shortcut: Option<char> },
    Folder { name: String, children: Vec<Node> },
    /// "Open All" for the folder at `path` ("" is the top); Alt opens them in a new window.
    OpenAll { path: String },
}

/// One menu while the tree is built: its folders in the order they were made, and its profiles.
#[derive(Default)]
struct Menu {
    folders: Vec<(String, Menu)>,
    leaves: Vec<Node>,
}

impl Menu {
    /// The folder named `name`, made when missing; names match ignoring case, as
    /// `findOrCreateTagSubmenuInMenu` compares titles (the first spelling is kept).
    fn folder(&mut self, name: &str) -> &mut Menu {
        let at = match self.folders.iter().position(|(n, _)| n.to_lowercase() == name.to_lowercase()) {
            Some(i) => i,
            None => {
                self.folders.push((name.to_string(), Menu::default()));
                self.folders.len() - 1
            }
        };
        &mut self.folders[at].1
    }

    fn add(&mut self, node: &Node) {
        let Node::Profile { guid, .. } = node else { return };
        if !self.leaves.iter().any(|l| matches!(l, Node::Profile { guid: g, .. } if g == guid)) {
            self.leaves.push(node.clone());
        }
    }

    /// Folders sorted without regard to case, then the profiles in model order, then Open All
    /// when the menu holds two or more entries (`menuHasMultipleItemsExcludingAlternates`).
    fn nodes(mut self, path: &str) -> Vec<Node> {
        self.folders.sort_by_key(|(n, _)| n.to_lowercase());
        let mut out: Vec<Node> = Vec::new();
        for (name, menu) in self.folders {
            let below = if path.is_empty() { name.clone() } else { format!("{path}/{name}") };
            out.push(Node::Folder { name, children: menu.nodes(&below) });
        }
        out.extend(self.leaves);
        if out.len() >= 2 {
            out.push(Node::OpenAll { path: path.to_string() });
        }
        out
    }
}

/// The tree for profiles given as (guid, name, tags, shortcut), in model order. Untagged
/// profiles sit in the top menu; each tag is a chain of folders named by its "/" parts, and a
/// profile is in the last folder of every tag it names, once. Names and tags are cleaned of
/// control, bidi and zero-width characters (P23); empty parts are skipped.
pub fn tree(profiles: &[(String, String, Vec<String>, Option<char>)]) -> Vec<Node> {
    let mut top = Menu::default();
    for (guid, name, tags, shortcut) in profiles {
        let node = Node::Profile { guid: guid.clone(), name: crate::profile_list::display_text(name), shortcut: *shortcut };
        let chains: Vec<Vec<String>> = tags
            .iter()
            .map(|t| t.split('/').map(crate::profile_list::display_text).filter(|p| !p.is_empty()).collect::<Vec<_>>())
            .filter(|c| !c.is_empty())
            .collect();
        if chains.is_empty() {
            top.add(&node);
            continue;
        }
        let mut seen: Vec<String> = Vec::new();
        for chain in chains {
            let key = chain.join("/").to_lowercase();
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);
            let mut menu = &mut top;
            for part in &chain {
                menu = menu.folder(part);
            }
            menu.add(&node);
        }
    }
    top.nodes("")
}

/// The guids an "Open All" at `path` opens: every profile of that menu and its submenus, once
/// (`getBookmarksInMenu:usedGuids:bookmarks:`).
pub fn open_all_guids(nodes: &[Node], path: &str) -> Vec<String> {
    let mut here = nodes;
    if !path.is_empty() {
        for part in path.split('/') {
            let Some(Node::Folder { children, .. }) = here.iter().find(|n| matches!(n, Node::Folder { name, .. } if name == part)) else { return Vec::new() };
            here = children;
        }
    }
    let mut out = Vec::new();
    collect(here, &mut out);
    out
}

fn collect(nodes: &[Node], out: &mut Vec<String>) {
    for n in nodes {
        match n {
            Node::Profile { guid, .. } if !out.contains(guid) => out.push(guid.clone()),
            Node::Folder { children, .. } => collect(children, out),
            _ => {}
        }
    }
}

/// The shortcut popup's tag for a key (`shortcutTagForKey:`): A-Z are 0-25, 0-9 are 100-109,
/// anything else -1.
pub fn shortcut_tag(key: char) -> i32 {
    match key.to_ascii_uppercase() {
        c @ 'A'..='Z' => c as i32 - 'A' as i32,
        c @ '0'..='9' => 100 + c as i32 - '0' as i32,
        _ => -1,
    }
}

/// The key a popup tag stands for (`shortcutKeyForTag:`); None for -1 and any other number.
pub fn shortcut_key(tag: i32) -> Option<char> {
    match tag {
        0..=25 => Some((b'A' + tag as u8) as char),
        100..=109 => Some((b'0' + (tag - 100) as u8) as char),
        _ => None,
    }
}

/// The popup's items as (tag, title), none first (`updateShortcutTitles`): "Ctrl+Win+A", and
/// "Ctrl+Win+A (Name)" for a key a profile holds.
pub fn shortcut_titles(profiles: &[(String, String, Option<char>)]) -> Vec<(i32, String)> {
    let tags = std::iter::once(-1).chain(0..=25).chain(100..=109);
    tags.map(|tag| {
        let Some(key) = shortcut_key(tag) else { return (tag, String::new()) };
        let title = match profiles.iter().find(|(_, _, k)| k.is_some_and(|k| shortcut_tag(k) == tag)) {
            Some((_, name, _)) => format!("Ctrl+Win+{key} ({})", crate::profile_list::display_text(name)),
            None => format!("Ctrl+Win+{key}"),
        };
        (tag, title)
    })
    .collect()
}

/// A profile's stored shortcut as a key: one of A-Z or 0-9 in any case, else None (the value
/// can come from an imported file).
pub fn stored_key(value: &str) -> Option<char> {
    let mut chars = value.chars();
    let c = chars.next()?;
    if chars.next().is_some() || !c.is_ascii_alphanumeric() {
        return None;
    }
    Some(c.to_ascii_uppercase())
}

/// Gives `key` to the profile `guid` and clears it from every other one
/// (`setShortcutValueToSelectedItem`); None clears this profile's.
pub fn take_shortcut(profiles: &mut [Profile], guid: &str, key: Option<char>) {
    for p in profiles.iter_mut() {
        if p.guid() == guid {
            match key {
                Some(k) => p.set("Shortcut", toml::Value::String(k.to_ascii_uppercase().to_string())),
                None => {
                    p.values.remove("Shortcut");
                }
            }
        } else if key.is_some() && stored_key(&p.string("Shortcut")) == key.map(|k| k.to_ascii_uppercase()) {
            p.values.remove("Shortcut");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(guid: &str, tags: &[&str]) -> (String, String, Vec<String>, Option<char>) {
        (guid.to_string(), guid.to_string(), tags.iter().map(|t| t.to_string()).collect(), None)
    }
    fn leaf(guid: &str) -> Node {
        Node::Profile { guid: guid.into(), name: guid.into(), shortcut: None }
    }
    fn folder(name: &str, children: Vec<Node>) -> Node {
        Node::Folder { name: name.into(), children }
    }
    fn all(path: &str) -> Node {
        Node::OpenAll { path: path.into() }
    }

    /// `findOrCreateTagSubmenuInMenu` puts a new submenu before the first plain item, so the
    /// folders (sorted) come first and the profiles after them (the plan's text lists the
    /// profile first; the source wins). A menu with two or more items (folders count) ends
    /// with Open All.
    #[test]
    fn profile_menu_tree_follows_iterm2() {
        let profiles = [p("A", &[]), p("B", &["work"]), p("C", &["work/db"]), p("D", &["work", "home", "work"])];
        let t = tree(&profiles);
        assert_eq!(
            t,
            vec![folder("home", vec![leaf("D")]), folder("work", vec![folder("db", vec![leaf("C")]), leaf("B"), leaf("D"), all("work")]), leaf("A"), all("")]
        );
    }

    #[test]
    fn tag_folders_merge_ignoring_case_and_skip_empty_parts() {
        let t = tree(&[p("A", &["Work"]), p("B", &["work"]), p("C", &["", "a//b"])]);
        assert!(matches!(&t[0], Node::Folder { name, .. } if name == "a"), "{t:?}");
        assert_eq!(t[1], folder("Work", vec![leaf("A"), leaf("B"), all("Work")]));
        // One profile alone shows no Open All.
        assert_eq!(tree(&[p("A", &[])]), vec![leaf("A")]);
        assert!(tree(&[]).is_empty());
    }

    #[test]
    fn open_all_collects_each_profile_once() {
        let t = tree(&[p("A", &[]), p("B", &["work"]), p("C", &["work/db"]), p("D", &["work", "home"])]);
        assert_eq!(open_all_guids(&t, "work"), vec!["C", "B", "D"]);
        assert_eq!(open_all_guids(&t, ""), vec!["D", "C", "B", "A"]);
        assert!(open_all_guids(&t, "nope").is_empty());
    }

    #[test]
    fn profile_shortcut_follows_iterm2() {
        assert_eq!((shortcut_key(0), shortcut_key(25), shortcut_key(100), shortcut_key(109), shortcut_key(-1), shortcut_key(26)), (Some('A'), Some('Z'), Some('0'), Some('9'), None, None));
        assert_eq!((shortcut_tag('Q'), shortcut_tag('0'), shortcut_tag('9'), shortcut_tag('?')), (16, 100, 109, -1));
        let mut profiles = vec![Profile::new("A"), Profile::new("B")];
        profiles[0].set("Shortcut", toml::Value::String("Q".into()));
        let (a, b) = (profiles[0].guid(), profiles[1].guid());
        take_shortcut(&mut profiles, &b, Some('Q'));
        assert_eq!(profiles[0].string("Shortcut"), "", "taken from A");
        assert_eq!(profiles[1].string("Shortcut"), "Q");
        take_shortcut(&mut profiles, &b, None);
        assert_eq!(profiles[1].string("Shortcut"), "");
        let titles = shortcut_titles(&[(a, "A".into(), None), (b, "B".into(), Some('Q'))]);
        assert_eq!(titles.len(), 37, "none, A-Z and 0-9");
        assert!(titles.contains(&(16, "Ctrl+Win+Q (B)".to_string())));
        assert!(titles.contains(&(0, "Ctrl+Win+A".to_string())));
        assert!(titles.contains(&(-1, String::new())));
    }

    #[test]
    fn stored_shortcuts_are_bounded() {
        assert_eq!((stored_key("q"), stored_key("Q"), stored_key("7")), (Some('Q'), Some('Q'), Some('7')));
        for bad in ["", "QQ", "-", " ", "\u{202e}", "\u{e9}", "Q\n"] {
            assert_eq!(stored_key(bad), None, "{bad:?}");
        }
    }
}

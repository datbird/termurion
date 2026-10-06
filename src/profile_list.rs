//! The profile list's pure parts: the tag hierarchy, the tag query and the filter
//! (`ProfileTagsView.m`, `ProfileListView.m`, `ProfileModel.m`).

use crate::profile::Profile;

/// The longest name or tag the lists show (P23).
pub const MAX_SHOWN: usize = 128;

/// Text from a profile or a tag that may be shown (P23): control, bidi and zero-width characters
/// removed, cut to 128 characters.
pub fn display_text(s: &str) -> String {
    s.chars().filter(|&c| !is_hostile(c)).take(MAX_SHOWN).collect()
}

/// A control, bidi or zero-width character.
pub fn is_hostile(c: char) -> bool {
    c.is_control() || matches!(c, '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
}

/// `sortedIndentedTags` (`ProfileTagsView.m:150-181`): every tag and each missing parent, sorted,
/// as (indented label, full tag path), two non-breaking spaces per level (`ProfileTagsView.m:139-147`).
pub fn tag_tree(tags: &[String]) -> Vec<(String, String)> {
    tag_tree_counted(tags, &mut 0)
}

/// The tag tree kept until the tags change (iTerm2 caches it, `ProfileTagsView.m` `_cache`).
#[derive(Default)]
pub struct TagTreeCache {
    tags: Vec<String>,
    tree: Vec<(String, String)>,
    /// The hash of every profile's tags the tree was built from (`for_profiles`).
    signature: u64,
    /// How many times the tree was built (the tests count it).
    pub builds: usize,
}

impl TagTreeCache {
    /// The tree for every profile's tags, rebuilt only when some tag changed: the list asks each
    /// frame, so it compares a hash of the tags and builds `all_tags` and the tree on a change only.
    pub fn for_profiles<'a>(&mut self, tag_lists: impl Iterator<Item = &'a Vec<String>>) -> &[(String, String)] {
        use std::hash::{Hash, Hasher};
        let lists: Vec<&Vec<String>> = tag_lists.collect();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        lists.hash(&mut hasher);
        let signature = hasher.finish();
        if self.builds == 0 || self.signature != signature {
            let all: Vec<(String, Vec<String>)> = lists.into_iter().map(|t| (String::new(), t.clone())).collect();
            self.tree = tag_tree(&all_tags(&all));
            self.signature = signature;
            self.builds += 1;
        }
        &self.tree
    }

    pub fn get(&mut self, tags: &[String]) -> &[(String, String)] {
        if self.builds == 0 || self.tags != tags {
            self.tree = tag_tree(tags);
            self.tags = tags.to_vec();
            self.builds += 1;
        }
        &self.tree
    }
}

/// `tag_tree`, adding the characters it builds to `work` (the tests bound it). A tag is read to
/// `MAX_TAG_DEPTH` levels only, whatever the caller passes, and each path grows from the one before.
pub fn tag_tree_counted(tags: &[String], work: &mut usize) -> Vec<(String, String)> {
    let mut sorted: Vec<&String> = tags.iter().collect();
    sorted.sort();
    sorted.dedup();
    let mut out = Vec::new();
    let mut previous: Vec<&str> = Vec::new();
    for tag in sorted {
        let parts: Vec<&str> = tag.split('/').take(MAX_TAG_DEPTH).collect();
        let mut matched = previous.iter().zip(&parts).take_while(|(a, b)| a == b).count();
        let mut path = parts[..matched].join("/");
        while matched < parts.len() {
            if matched > 0 {
                path.push('/');
            }
            path.push_str(parts[matched]);
            let label = format!("{}{}", "\u{a0}\u{a0}".repeat(matched), display_text(parts[matched]));
            *work += label.chars().count() + path.chars().count();
            out.push((label, path.clone()));
            matched += 1;
        }
        previous = parts;
    }
    out
}

/// The most tags a profile keeps, the longest tag and its deepest "/" nesting: tags reach the
/// store from imports, the API and arrangements, so they are bounded at the one write point.
pub const MAX_TAGS: usize = 64;
pub const MAX_TAG_LEN: usize = 256;
pub const MAX_TAG_DEPTH: usize = 8;

/// A "Tags" value cleaned: only text tags, at most `MAX_TAGS` of them, each cut to `MAX_TAG_LEN`
/// characters and `MAX_TAG_DEPTH` levels (`Profile::set`, `Profile::from_values`). None for a
/// value that is not a list.
pub fn clean_tags(value: toml::Value) -> Option<toml::Value> {
    let toml::Value::Array(list) = value else { return None };
    let kept = list
        .into_iter()
        .filter_map(|t| match t {
            toml::Value::String(s) => Some(clean_tag(&s)),
            _ => None,
        })
        .take(MAX_TAGS)
        .map(toml::Value::String)
        .collect();
    Some(toml::Value::Array(kept))
}

fn clean_tag(tag: &str) -> String {
    let leveled: Vec<&str> = tag.splitn(MAX_TAG_DEPTH + 1, '/').take(MAX_TAG_DEPTH).collect();
    leveled.join("/").chars().take(MAX_TAG_LEN).collect()
}

/// The search field's text for the selected tags (`ProfileListView.m:1207-1212`).
pub fn tag_query(selected: &[String]) -> String {
    selected.iter().map(|t| format!("tag:{t}")).collect::<Vec<_>>().join(" ")
}

/// Every tag of every profile, once, sorted (`ProfileModel allTags`).
pub fn all_tags(profiles: &[(String, Vec<String>)]) -> Vec<String> {
    let mut all: Vec<String> = profiles.iter().flat_map(|(_, tags)| tags.iter().cloned()).collect();
    all.sort();
    all.dedup();
    all
}

/// The list's rows as (guid, name, tags, command): the command only for a custom command
/// (`ProfileModel.m:281-292`).
pub fn rows(profiles: &[Profile]) -> Vec<(String, String, Vec<String>, String)> {
    profiles
        .iter()
        .map(|p| {
            let tags = p.get("Tags").and_then(|v| v.as_array().map(|a| a.iter().filter_map(|t| t.as_str().map(str::to_string)).collect::<Vec<_>>())).unwrap_or_default();
            let command = if p.string("Custom Command") == "No" { String::new() } else { p.string("Command") };
            (p.guid(), p.name(), tags, command)
        })
        .collect()
}

/// The indexes of the rows the query keeps (`doesProfile:matchFilter:`).
pub fn filter(rows: &[(String, String, Vec<String>, String)], query: &str) -> Vec<usize> {
    rows.iter().enumerate().filter(|(_, (_, name, tags, command))| crate::snippets::profile_matches(query, name, tags, command)).map(|(i, _)| i).collect()
}

/// The selection after a click on `clicked` in `shown` (the visible guids in order): a plain click
/// selects only it; with `multi`, Ctrl toggles it and Shift extends from the last selected row.
pub fn click_selection(selection: &[String], shown: &[String], clicked: &str, ctrl: bool, shift: bool, multi: bool) -> Vec<String> {
    if !multi || !(ctrl || shift) {
        return vec![clicked.to_string()];
    }
    if shift {
        let pos = |g: &str| shown.iter().position(|s| s == g);
        let anchor = selection.last().and_then(|g| pos(g));
        if let (Some(a), Some(c)) = (anchor, pos(clicked)) {
            return shown[a.min(c)..=a.max(c)].to_vec();
        }
        return vec![clicked.to_string()];
    }
    toggled(selection, clicked)
}

/// The selected tags after a click on `tag`: a plain click selects only it, Ctrl toggles it.
pub fn click_tags(selected: &[String], tag: &str, ctrl: bool) -> Vec<String> {
    if ctrl { toggled(selected, tag) } else { vec![tag.to_string()] }
}

/// The selection once a filter has hidden rows (`updateResultsForSearch`, `PLV:1004-1012`): hidden
/// rows leave it, and with nothing left the first match is selected.
pub fn follow_filter(selection: &[String], shown: &[String]) -> Vec<String> {
    let kept: Vec<String> = selection.iter().filter(|g| shown.contains(g)).cloned().collect();
    if kept.is_empty() { shown.first().cloned().into_iter().collect() } else { kept }
}

/// The selection after an arrow key in the search field (`setArrowHandler:`, `PLV:188`): one row
/// from the last selected one, kept in the list.
pub fn arrow(selection: &[String], shown: &[String], down: bool) -> Vec<String> {
    let at = selection.last().and_then(|g| shown.iter().position(|s| s == g));
    let next = match (at, down) {
        (None, true) => 0,
        (None, false) => shown.len().saturating_sub(1),
        (Some(i), true) => (i + 1).min(shown.len().saturating_sub(1)),
        (Some(i), false) => i.saturating_sub(1),
    };
    shown.get(next).cloned().into_iter().collect()
}

fn toggled(list: &[String], item: &str) -> Vec<String> {
    if list.iter().any(|s| s == item) {
        list.iter().filter(|s| *s != item).cloned().collect()
    } else {
        list.iter().cloned().chain(std::iter::once(item.to_string())).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snippets::profile_matches;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn tag_tree_follows_iterm2() {
        let t = tag_tree(&s(&["work", "work/db", "work/web", "home"]));
        let want: Vec<(String, String)> = [("home", "home"), ("work", "work"), ("\u{a0}\u{a0}db", "work/db"), ("\u{a0}\u{a0}web", "work/web")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        assert_eq!(t, want);
        let t = tag_tree(&s(&["a/b/c"]));
        let want: Vec<(String, String)> = [("a", "a"), ("\u{a0}\u{a0}b", "a/b"), ("\u{a0}\u{a0}\u{a0}\u{a0}c", "a/b/c")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        assert_eq!(t, want);
    }

    #[test]
    fn tag_tree_cleans_hostile_labels() {
        let t = tag_tree(&s(&["x\u{202e}y"]));
        assert_eq!(t[0].0, "xy");
    }

    #[test]
    fn hostile_tags_are_bounded_where_they_are_written() {
        let deep = "a/".repeat(50_000);
        let many: Vec<toml::Value> = (0..1000).map(|i| toml::Value::String(format!("t{i}"))).collect();
        let mut p = Profile::new("X");
        p.set("Tags", toml::Value::Array(vec![toml::Value::String(deep.clone())]));
        let stored = rows(&[p.clone()])[0].2.clone();
        assert_eq!(stored.len(), 1);
        assert!(stored[0].chars().count() <= MAX_TAG_LEN, "len {}", stored[0].len());
        assert!(stored[0].split('/').count() <= MAX_TAG_DEPTH);
        p.set("Tags", toml::Value::Array(many.clone()));
        assert_eq!(rows(&[p])[0].2.len(), MAX_TAGS);
        // The same gate for a stored or imported table.
        let mut table = std::collections::BTreeMap::new();
        table.insert("Tags".to_string(), toml::Value::Array(vec![toml::Value::String(deep), toml::Value::Integer(3)]));
        let stored = rows(&[Profile::from_values(table)])[0].2.clone();
        assert_eq!(stored.len(), 1);
        assert!(stored[0].chars().count() <= MAX_TAG_LEN);
        let mut table = std::collections::BTreeMap::new();
        table.insert("Tags".to_string(), toml::Value::Array(many));
        assert_eq!(rows(&[Profile::from_values(table)])[0].2.len(), MAX_TAGS);
    }

    #[test]
    fn tag_tree_work_is_bounded_for_a_deep_tag() {
        // Not through the write point: the tree bounds itself too.
        let deep = vec!["a/".repeat(4_000)];
        let mut work = 0;
        let tree = tag_tree_counted(&deep, &mut work);
        assert!(tree.len() <= MAX_TAG_DEPTH, "rows {}", tree.len());
        assert!(work < 4_000, "work {work}");
        // Fully capped tags cost at most their capped size.
        let capped: Vec<String> = (0..MAX_TAGS).map(|i| format!("{i}{}", "/b".repeat(MAX_TAG_DEPTH - 1))).collect();
        let mut work = 0;
        tag_tree_counted(&capped, &mut work);
        assert!(work < MAX_TAGS * MAX_TAG_DEPTH * 2 * (MAX_TAG_DEPTH + 4), "work {work}");
    }

    #[test]
    fn the_tag_tree_is_built_only_when_the_tags_change() {
        let mut cache = TagTreeCache::default();
        let a = s(&["a/b", "c"]);
        for _ in 0..50 {
            assert_eq!(cache.get(&a).len(), 3);
        }
        assert_eq!(cache.builds, 1);
        assert_eq!(cache.get(&s(&["z"])).len(), 1);
        assert_eq!(cache.builds, 2);
    }

    #[test]
    fn the_selection_follows_the_filter() {
        let shown = s(&["b", "c"]);
        assert_eq!(follow_filter(&s(&["a"]), &shown), s(&["b"]));
        assert_eq!(follow_filter(&s(&["c"]), &shown), s(&["c"]));
        assert_eq!(follow_filter(&s(&["a", "c"]), &shown), s(&["c"]));
        assert!(follow_filter(&s(&["a"]), &[]).is_empty());
        assert_eq!(arrow(&s(&["b"]), &shown, true), s(&["c"]));
        assert_eq!(arrow(&s(&["c"]), &shown, true), s(&["c"]));
        assert_eq!(arrow(&s(&["c"]), &shown, false), s(&["b"]));
        assert_eq!(arrow(&[], &shown, true), s(&["b"]));
        assert_eq!(arrow(&[], &shown, false), s(&["c"]));
        assert!(arrow(&[], &[], true).is_empty());
    }

    #[test]
    fn tag_query_follows_iterm2() {
        assert_eq!(tag_query(&s(&["work/db", "x"])), "tag:work/db tag:x");
        assert_eq!(tag_query(&[]), "");
        let all = all_tags(&[("g1".into(), s(&["b", "a"])), ("g2".into(), s(&["a", "c"]))]);
        assert_eq!(all, s(&["a", "b", "c"]));
        let work = s(&["work/db"]);
        let home = s(&["home"]);
        assert!(profile_matches("tag:work", "A", &work, ""));
        assert!(!profile_matches("tag:work", "B", &home, ""));
        assert!(profile_matches("command:ssh", "C", &[], "ssh host"));
        assert!(!profile_matches("-work", "A", &work, ""));
    }

    #[test]
    fn display_text_is_bounded_and_clean() {
        assert_eq!(display_text("a\u{200b}b\u{0007}c"), "abc");
        assert_eq!(display_text(&"x".repeat(300)).chars().count(), MAX_SHOWN);
    }

    #[test]
    fn filter_uses_the_command_only_for_a_custom_command() {
        let mut ssh = Profile::new("Box");
        ssh.set("Custom Command", "Yes".into());
        ssh.set("Command", "ssh devhost".into());
        let mut login = Profile::new("Plain");
        login.set("Command", "ssh devhost".into());
        let r = rows(&[ssh, login]);
        assert_eq!(r[0].3, "ssh devhost");
        assert_eq!(r[1].3, "");
        assert_eq!(filter(&r, "command:ssh"), vec![0]);
        assert_eq!(filter(&r, ""), vec![0, 1]);
    }

    #[test]
    fn clicks_select_as_a_table_does() {
        let shown = s(&["a", "b", "c", "d"]);
        assert_eq!(click_selection(&s(&["a"]), &shown, "c", false, false, true), s(&["c"]));
        assert_eq!(click_selection(&s(&["a"]), &shown, "c", true, false, true), s(&["a", "c"]));
        assert_eq!(click_selection(&s(&["a", "c"]), &shown, "c", true, false, true), s(&["a"]));
        assert_eq!(click_selection(&s(&["b"]), &shown, "d", false, true, true), s(&["b", "c", "d"]));
        assert_eq!(click_selection(&s(&["a"]), &shown, "c", true, true, false), s(&["c"]));
        assert_eq!(click_tags(&s(&["x"]), "y", false), s(&["y"]));
        assert_eq!(click_tags(&s(&["x"]), "y", true), s(&["x", "y"]));
        assert_eq!(click_tags(&s(&["x", "y"]), "y", true), s(&["x"]));
    }

    #[test]
    fn the_sidebar_tree_is_built_once_while_tags_stay() {
        let a = vec!["work".to_string(), "work/db".to_string()];
        let b = vec!["home".to_string()];
        let mut cache = TagTreeCache::default();
        for _ in 0..3 {
            assert_eq!(cache.for_profiles([&a, &b].into_iter()).len(), 3);
        }
        assert_eq!(cache.builds, 1, "three frames with the same tags build once");
        let c = vec!["play".to_string()];
        assert_eq!(cache.for_profiles([&a, &b, &c].into_iter()).len(), 4);
        assert_eq!(cache.builds, 2);
    }
}

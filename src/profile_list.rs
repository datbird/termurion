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
/// as (indented label, full tag path), four spaces per level.
pub fn tag_tree(tags: &[String]) -> Vec<(String, String)> {
    let mut sorted: Vec<&String> = tags.iter().collect();
    sorted.sort();
    sorted.dedup();
    let mut out = Vec::new();
    let mut previous: Vec<&str> = Vec::new();
    for tag in sorted {
        let parts: Vec<&str> = tag.split('/').collect();
        let mut matched = previous.iter().zip(&parts).take_while(|(a, b)| a == b).count();
        while matched < parts.len() {
            out.push((format!("{}{}", "    ".repeat(matched), display_text(parts[matched])), parts[..=matched].join("/")));
            matched += 1;
        }
        previous = parts;
    }
    out
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
        let want: Vec<(String, String)> = [("home", "home"), ("work", "work"), ("    db", "work/db"), ("    web", "work/web")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        assert_eq!(t, want);
        let t = tag_tree(&s(&["a/b/c"]));
        let want: Vec<(String, String)> = [("a", "a"), ("    b", "a/b"), ("        c", "a/b/c")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        assert_eq!(t, want);
    }

    #[test]
    fn tag_tree_cleans_hostile_labels() {
        let t = tag_tree(&s(&["x\u{202e}y"]));
        assert_eq!(t[0].0, "xy");
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
}

//! Each of the seven toolbelt tools' rows, filters and pick rules: pure transforms from stored
//! data (snippets, shell history, actions, marks, profiles) to what a tool's list should show,
//! and the modifier-key rules for its buttons and double-click. The shared list widget is
//! `tool_list`; the app wires these tools together in `src/app/toolbelt_tools_ui.rs`.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use crate::marks::Mark;
use crate::shell_history::{self, Dir, Use};
use crate::snippets::{self, Snippet};
use crate::user_actions::UserAction;

// ---------------------------------------------------------------------------------------------
// Paste History (`ToolPasteHistory.m`, min 60).
// ---------------------------------------------------------------------------------------------

/// A row's text: `\n` becomes a space, cut to 256 characters, no ellipsis
/// (`ToolPasteHistory.m:221-229`).
pub fn paste_rows(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| v.replace('\n', " ").chars().take(256).collect()).collect()
}

/// The "Erase Paste History" alert (`ToolPasteHistory.m:279-282`).
pub const ERASE_PASTE: (&str, &str) = ("Erase Paste History", "Paste history will be erased. Continue?");

// ---------------------------------------------------------------------------------------------
// Command History (`ToolCommandHistoryView.m`, min 88).
// ---------------------------------------------------------------------------------------------

/// One row: the raw command (for the case-sensitive filter), its display text (newlines as
/// spaces, an emoji prefix for a nonzero exit), whether it is bold (this session's mark), the
/// use's directory (for Option's `cd`) and its time.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandRow {
    pub command: String,
    pub text: String,
    pub bold: bool,
    pub dir: Option<String>,
    pub time: f64,
}

/// Command History's rows, in the order `uses` gives them (`shell_history::Store::uses` is
/// oldest first). A nonzero exit code prefixes `"\u{1F6AB} "`, or `"\u{1F4A9} "` on April Fools'
/// (`ToolCommandHistoryView.m:202-229`).
pub fn command_rows(uses: &[(String, Use)], mine: &dyn Fn(&str, f64) -> bool, april_first: bool) -> Vec<CommandRow> {
    uses.iter()
        .map(|(command, u)| {
            let mut text = command.replace('\n', " ");
            if u.code.is_some_and(|c| c != 0) {
                let prefix = if april_first { "\u{1F4A9} " } else { "\u{1F6AB} " };
                text = format!("{prefix}{text}");
            }
            CommandRow { command: command.clone(), text, bold: mine(command, u.time), dir: u.dir.clone(), time: u.time }
        })
        .collect()
}

/// Command History's filter: case-sensitive substring of the raw command
/// (`ToolCommandHistoryView.m:335`).
pub fn command_filter(rows: &[CommandRow], query: &str) -> Vec<usize> {
    if query.is_empty() {
        return (0..rows.len()).collect();
    }
    (0..rows.len()).filter(|&i| rows[i].command.contains(query)).collect()
}

/// A double-click's insertion (`ToolCommandHistoryView.m:294-314`): Option with a directory
/// inserts `"cd " + dir`, unescaped (the kept bug); Option with no directory inserts nothing.
/// Shift appends a newline.
pub fn command_insert(command: &str, dir: Option<&str>, alt: bool, shift: bool) -> Option<String> {
    let mut text = if alt {
        match dir {
            Some(d) => format!("cd {d}"),
            None => return None,
        }
    } else {
        command.to_string()
    };
    if shift {
        text.push('\n');
    }
    Some(text)
}

/// The "Erase Command History" alert (`ToolCommandHistoryView.m:73-82,362-367`).
pub const ERASE_COMMANDS: (&str, &str) = ("Erase Command History", "Command history for all hosts will be erased. Continue?");

// ---------------------------------------------------------------------------------------------
// Recent Directories (`ToolDirectoriesView.m`, min 88).
// ---------------------------------------------------------------------------------------------

/// Recent Directories' rows: best first becomes best last (`ToolDirectoriesView.m:233-251`).
pub fn dir_rows(best_first: &[Dir]) -> Vec<Dir> {
    let mut v = best_first.to_vec();
    v.reverse();
    v
}

/// Recent Directories' filter: case-insensitive substring of the path
/// (`ToolDirectoriesView.m:299-300`).
pub fn dir_filter(dirs: &[Dir], query: &str) -> Vec<usize> {
    if query.is_empty() {
        return (0..dirs.len()).collect();
    }
    let q = query.to_lowercase();
    (0..dirs.len()).filter(|&i| dirs[i].path.to_lowercase().contains(&q)).collect()
}

/// A path's components, splitting on both `/` and `\` (a Windows drive path's own form is
/// still split the same way; only `dir_text`'s prefix differs for it, a ruling).
fn dir_components(path: &str) -> Vec<&str> {
    path.split(['/', '\\']).filter(|s| !s.is_empty()).collect()
}

#[derive(Clone, Debug, Default)]
struct DirNode {
    children: BTreeMap<String, DirNode>,
}

/// The abbreviation tree spanning every host's directories (`iTermDirectoryTree`,
/// `iTermShellHistoryController.m:765-774`, a correction to the research: not per-host).
#[derive(Clone, Debug, Default)]
pub struct DirTree {
    root: DirNode,
}

impl DirTree {
    /// Adds a path's components to the tree (`-addPath:`).
    pub fn add(&mut self, path: &str) {
        let mut node = &mut self.root;
        for p in dir_components(path) {
            node = node.children.entry(p.to_string()).or_default();
        }
    }

    /// Which of `path`'s components may be abbreviated to their first character: a component is
    /// safe when no sibling at that point in the tree starts with the same first character
    /// (`-abbreviationSafeIndexesInPath:`, `iTermDirectoryTreeNode.m`'s
    /// `numberOfChildrenStartingWithString:`). A path never added still gets an answer for its
    /// matching prefix; the rest reads as unsafe.
    pub fn safe(&self, path: &str) -> Vec<bool> {
        let parts = dir_components(path);
        let mut out = Vec::with_capacity(parts.len());
        let mut node = &self.root;
        for p in &parts {
            let Some(first) = p.chars().next() else {
                out.push(true);
                continue;
            };
            let count = node.children.keys().filter(|k| k.starts_with(first)).count();
            out.push(count <= 1);
            match node.children.get(*p) {
                Some(n) => node = n,
                None => {
                    out.resize(parts.len(), false);
                    break;
                }
            }
        }
        out
    }
}

/// A Windows drive letter component ("C:"), so `dir_text` can keep its own form
/// (Decisions: "a Windows drive path keeps its own form without the leading '/'").
fn is_drive_component(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

/// A directory's display text (`iTermRecentDirectoryMO+Additions.m`'s
/// `attributedStringForTableColumn:basedOnAttributedString:...`): a starred path starts
/// `"\u{2605} /"`, else `"/"`; components join with `/`; a safe component abbreviates to its
/// first character, tried left to right while the text is still too wide; the last component
/// never shortens.
pub fn dir_text(path: &str, starred: bool, safe: &[bool], max_w: f32, width: &dyn Fn(&str) -> f32) -> String {
    let raw = dir_components(path);
    let drive = raw.first().is_some_and(|c| is_drive_component(c));
    let sep = if drive { "\\" } else { "/" };
    let star = if starred { "\u{2605} " } else { "" };
    let lead = if drive { "" } else { "/" };
    let mut parts: Vec<String> = raw.iter().map(|s| s.to_string()).collect();
    let build = |parts: &[String]| format!("{star}{lead}{}", parts.join(sep));
    let mut text = build(&parts);
    for i in 0..parts.len().saturating_sub(1) {
        if width(&text) <= max_w {
            break;
        }
        if safe.get(i).copied().unwrap_or(false) {
            parts[i] = parts[i].chars().take(1).collect();
            text = build(&parts);
        }
    }
    text
}

/// A double-click's insertion: the path escaped like `insertable_dir`, Option prefixing `"cd "`,
/// Shift appending a newline (`ToolDirectoriesView.m:261-280`).
pub fn dir_insert(path: &str, shell: Option<&str>, alt: bool, shift: bool) -> String {
    let mut text = shell_history::insertable_dir(path, shell);
    if alt {
        text = format!("cd {text}");
    }
    if shift {
        text.push('\n');
    }
    text
}

/// The "Erase Saved Directories?" alert, with no message (`ToolDirectoriesView.m:282-290`).
pub const ERASE_DIRS: (&str, &str) = ("Erase Saved Directories?", "");

// ---------------------------------------------------------------------------------------------
// A shared move-to-index helper (`iTermActionsModel.m:164-188`, `iTermSnippetsModel.m:666-690`):
// the moving items keep their relative order; the insertion point is `to` less however many of
// them already sat before it.
// ---------------------------------------------------------------------------------------------

fn move_items<T: Clone>(list: &[T], rows: &[usize], to: usize) -> Vec<T> {
    let moving: Vec<T> = rows.iter().map(|&i| list[i].clone()).collect();
    let count_before = rows.iter().filter(|&&i| i < to).count();
    let mut updated: Vec<T> = list.to_vec();
    for &i in rows.iter().rev() {
        updated.remove(i);
    }
    let insertion_index = to.saturating_sub(count_before).min(updated.len());
    for (k, item) in moving.into_iter().enumerate() {
        let idx = (insertion_index + k).min(updated.len());
        updated.insert(idx, item);
    }
    updated
}

// ---------------------------------------------------------------------------------------------
// Snippets (`TS` = `iTermToolSnippets.m`, min 87).
// ---------------------------------------------------------------------------------------------

/// A snippet's display text (`combinedStringForSnippet:`/`stringForSnippet:`,
/// `TS:604-625,671-676`): the value alone, cut to 40, when the title is empty or equals the
/// value up to 40 characters; otherwise the title (256) and the value, `" \u{2014} "` between
/// them. While searching the value stays cut to 40 even in the combined form (`TS:696-703`,
/// the attributed-string path always trims the value to 40 before deciding whether to show the
/// title). Trimming (`snippets::ellipsize`) turns newlines into `\u{21A9}` and ends a cut string
/// with `\u{2026}`.
pub fn snippet_text(s: &Snippet, searching: bool) -> String {
    let value40 = snippets::ellipsize(&s.value, 40);
    let title256 = snippets::ellipsize(&s.title, 256);
    if title256.is_empty() || snippets::ellipsize(&s.title, 40) == value40 {
        return value40;
    }
    if searching {
        format!("{title256} \u{2014} {value40}")
    } else {
        format!("{title256} \u{2014} {}", snippets::ellipsize(&s.value, 256))
    }
}

/// A node in the Snippets tool's tree: a snippet (by index into the shown list's source slice)
/// or a folder with its full tag path and children.
#[derive(Clone, Debug, PartialEq)]
pub enum SnippetNode {
    Snippet(usize),
    Folder { name: String, path: String, children: Vec<SnippetNode> },
}

#[derive(Default)]
struct SnippetFolderBuild {
    name: String,
    path: String,
    items: Vec<usize>,
    folders: Vec<SnippetFolderBuild>,
}

fn add_snippet_tag(build: &mut SnippetFolderBuild, parts: &[&str], parent_path: &str, snippet: usize) {
    if parts.is_empty() {
        build.items.push(snippet);
        return;
    }
    let name = parts[0];
    let path = if parent_path.is_empty() { name.to_string() } else { format!("{parent_path}/{name}") };
    let idx = match build.folders.iter().position(|f| f.name == name) {
        Some(i) => i,
        None => {
            build.folders.push(SnippetFolderBuild { name: name.to_string(), path: path.clone(), items: Vec::new(), folders: Vec::new() });
            build.folders.len() - 1
        }
    };
    add_snippet_tag(&mut build.folders[idx], &parts[1..], &path, snippet);
}

fn finish_snippet_tree(build: SnippetFolderBuild) -> Vec<SnippetNode> {
    let mut folders = build.folders;
    folders.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    let mut out: Vec<SnippetNode> = build.items.into_iter().map(SnippetNode::Snippet).collect();
    out.extend(folders.into_iter().map(|f| {
        let name = f.name.clone();
        let path = f.path.clone();
        SnippetNode::Folder { name, path, children: finish_snippet_tree(f) }
    }));
    out
}

/// The Snippets tool's tree (`TS:186-298`, `-makeTree`): untagged snippets first, in `shown`'s
/// order; then folders (each tag split on `/`), sorted ignoring case; within a folder its own
/// snippets before its subfolders; a snippet appears once per tag it carries.
pub fn snippet_tree(snippets: &[Snippet], shown: &[usize]) -> Vec<SnippetNode> {
    let mut root = SnippetFolderBuild::default();
    for &i in shown {
        if snippets[i].tags.is_empty() {
            root.items.push(i);
        }
    }
    for &i in shown {
        for tag in &snippets[i].tags {
            let parts: Vec<&str> = tag.split('/').collect();
            add_snippet_tag(&mut root, &parts, "", i);
        }
    }
    finish_snippet_tree(root)
}

/// A displayed row's kind.
#[derive(Clone, Debug, PartialEq)]
pub enum SnippetRowKind {
    Snippet(usize),
    Folder(String),
}

/// One row of the Snippets tool's outline.
#[derive(Clone, Debug, PartialEq)]
pub struct SnippetRow {
    pub depth: usize,
    pub kind: SnippetRowKind,
    pub text: String,
    pub tags: Vec<String>,
}

/// The tree flattened to displayed rows: a collapsed folder (its path not in `expanded`) hides
/// its children (folders start collapsed, a 9j ruling; state is per window, not saved).
pub fn snippet_rows(snippets: &[Snippet], tree: &[SnippetNode], expanded: &BTreeSet<String>, searching: bool) -> Vec<SnippetRow> {
    fn walk(nodes: &[SnippetNode], snippets: &[Snippet], expanded: &BTreeSet<String>, searching: bool, depth: usize, out: &mut Vec<SnippetRow>) {
        for n in nodes {
            match n {
                SnippetNode::Snippet(i) => {
                    let s = &snippets[*i];
                    out.push(SnippetRow { depth, kind: SnippetRowKind::Snippet(*i), text: snippet_text(s, searching), tags: s.tags.clone() });
                }
                SnippetNode::Folder { name, path, children } => {
                    out.push(SnippetRow { depth, kind: SnippetRowKind::Folder(name.clone()), text: name.clone(), tags: Vec::new() });
                    if expanded.contains(path) {
                        walk(children, snippets, expanded, searching, depth + 1, out);
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(tree, snippets, expanded, searching, 0, &mut out);
    out
}

/// Moves the snippets named by `guids` to `child_index` (`TS:876-899`,
/// `-moveSnippetsWithGUIDs:toIndex:`). `bound` is the outline's own child-count check
/// (`_filteredSnippets.count`); `child_index` is then used as if it were an index into the
/// unfiltered `list` (the source's own bug, `-acceptDrop:` passes an outline child index
/// straight through, kept per the 9c ruling). Refused past `bound`.
pub fn move_snippets(list: &[Snippet], guids: &[String], child_index: usize, bound: usize) -> Option<Vec<Snippet>> {
    if child_index > bound {
        return None;
    }
    let rows: Vec<usize> = list.iter().enumerate().filter(|(_, s)| guids.contains(&s.guid)).map(|(i, _)| i).collect();
    if rows.is_empty() {
        return Some(list.to_vec());
    }
    Some(move_items(list, &rows, child_index))
}

/// Option wins over Shift (`TS:372-378`, `-preferredAction`): Option opens Advanced Paste, else
/// Shift opens the Composer, else Send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnippetPick {
    Send,
    AdvancedPaste,
    Composer,
}

pub fn snippet_pick(alt: bool, shift: bool) -> SnippetPick {
    if alt {
        SnippetPick::AdvancedPaste
    } else if shift {
        SnippetPick::Composer
    } else {
        SnippetPick::Send
    }
}

// ---------------------------------------------------------------------------------------------
// Actions (`iTermToolActions.m`, min 60).
// ---------------------------------------------------------------------------------------------

/// An action's title, or "Untitled" for an empty one (`iTermToolActions.m:286-296`).
pub fn action_rows(actions: &[UserAction]) -> Vec<String> {
    actions.iter().map(|a| if a.title.is_empty() { "Untitled".to_string() } else { a.title.clone() }).collect()
}

/// Reordering (`-acceptDrop:`, drop-above only): the selected `rows` move to just above `to`
/// (`iTermActionsModel.m:164-188`).
pub fn move_actions(list: &[UserAction], rows: &[usize], to: usize) -> Vec<UserAction> {
    let mut sorted: Vec<usize> = rows.iter().copied().filter(|&i| i < list.len()).collect();
    sorted.sort_unstable();
    sorted.dedup();
    move_items(list, &sorted, to)
}

// ---------------------------------------------------------------------------------------------
// Named Marks (`ToolNamedMarks.swift`, min 60).
// ---------------------------------------------------------------------------------------------

/// A named mark's row: its id and display name, `"(Unnamed)"` for none (`:154-155`). `marks` is
/// trusted to already be in buffer order (`Screen::marks_in_order`, `namedMarkSort`), and that
/// order is kept, not re-sorted.
pub fn mark_rows(marks: &[(i32, Mark)]) -> Vec<(u32, String)> {
    marks.iter().map(|(_, m)| (m.id, m.name.clone().unwrap_or_else(|| "(Unnamed)".to_string()))).collect()
}

// ---------------------------------------------------------------------------------------------
// Profiles (`ToolProfiles.m`, min 110; search `ProfileModel.m:300-343`).
// ---------------------------------------------------------------------------------------------

/// One row: the guid, the display name (starred for the default profile), and its tags joined
/// by `", "` (`ProfileListView.m:611-668`).
#[derive(Clone, Debug, PartialEq)]
pub struct ProfileRow {
    pub guid: String,
    pub name: String,
    pub tags: String,
}

/// The Profiles tool's rows: `profiles` as `(guid, name, tags, command)`, filtered by
/// `snippets::profile_matches`.
pub fn profile_rows(profiles: &[(String, String, Vec<String>, String)], default_guid: &str, query: &str) -> Vec<ProfileRow> {
    profiles
        .iter()
        .filter(|(_, name, tags, command)| snippets::profile_matches(query, name, tags, command))
        .map(|(guid, name, tags, _)| {
            let name = if guid == default_guid { format!("\u{2605} {name}") } else { name.clone() };
            ProfileRow { guid: guid.clone(), name, tags: tags.join(", ") }
        })
        .collect()
}

/// What a double-click (or the bottom popup's Open) does, by modifier
/// (`ToolProfiles.m:190-201`, Control beats Option beats Shift).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileOpen {
    Tab,
    Window,
    HorizontalSplit,
    VerticalSplit,
}

/// The bottom popup's entries in order (`ToolProfiles.m:65-76`).
pub const PROFILE_OPENS: [(ProfileOpen, &str); 4] = [
    (ProfileOpen::Tab, "New Tab"),
    (ProfileOpen::Window, "New Window"),
    (ProfileOpen::HorizontalSplit, "New Horizontal Split"),
    (ProfileOpen::VerticalSplit, "New Vertical Split"),
];

pub fn profile_open(ctrl: bool, alt: bool, shift: bool) -> ProfileOpen {
    if ctrl {
        ProfileOpen::HorizontalSplit
    } else if alt {
        ProfileOpen::VerticalSplit
    } else if shift {
        ProfileOpen::Window
    } else {
        ProfileOpen::Tab
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::keyaction::Escaping;
    use crate::marks::{Kind, Mark};
    use crate::shell_history::{Dir, Use};
    use crate::snippets::Snippet;
    use crate::user_actions::UserAction;

    fn snip(title: &str, value: &str, tags: &[&str]) -> Snippet {
        Snippet { title: title.into(), value: value.into(), guid: format!("g-{title}-{value}"), tags: tags.iter().map(|s| s.to_string()).collect(), escaping: Escaping::Common, version: 2 }
    }

    fn ua(title: &str) -> UserAction {
        UserAction { action: 0, title: title.to_string(), parameter: String::new(), escaping: Escaping::Common, apply_mode: 0, version: 2 }
    }

    #[test]
    fn list_tools_follow_iterm2() {
        // Paste History: no ellipsis, only \n becomes a space, cut to 256.
        let long = "x".repeat(300);
        let rows = paste_rows(&["a\nb", &long]);
        assert_eq!(rows[0], "a b");
        assert_eq!(rows[1], "x".repeat(256));

        // Command History rows.
        let uses = vec![
            ("git status".to_string(), Use { time: 1.0, dir: Some("/repo".to_string()), code: Some(0) }),
            ("rm -rf x".to_string(), Use { time: 2.0, dir: None, code: Some(1) }),
            ("multi\nline".to_string(), Use { time: 3.0, dir: None, code: None }),
        ];
        let mine = |c: &str, t: f64| c == "rm -rf x" && t == 2.0;
        let rows = command_rows(&uses, &mine, false);
        assert_eq!(rows[0].text, "git status");
        assert!(!rows[0].bold);
        assert_eq!(rows[1].text, "\u{1F6AB} rm -rf x");
        assert!(rows[1].bold);
        assert_eq!(rows[2].text, "multi line");
        assert_eq!(rows.iter().map(|r| r.time).collect::<Vec<_>>(), vec![1.0, 2.0, 3.0], "oldest first, input order preserved");

        let april = command_rows(&uses, &mine, true);
        assert_eq!(april[1].text, "\u{1F4A9} rm -rf x");

        // Case-sensitive filter.
        assert!(command_filter(&rows, "Git").is_empty());
        assert_eq!(command_filter(&rows, "git"), vec![0]);

        // Insert.
        assert_eq!(command_insert("ls", Some("C:\\x y"), true, false), Some("cd C:\\x y".to_string()));
        assert_eq!(command_insert("ls", None, true, false), None);
        assert_eq!(command_insert("ls", None, false, true), Some("ls\n".to_string()));

        // Recent Directories: best first, reversed for display.
        let best_first = vec![Dir { path: "/b".into(), uses: 3, last: 3.0, starred: false }, Dir { path: "/a".into(), uses: 2, last: 2.0, starred: false }];
        let reversed = dir_rows(&best_first);
        assert_eq!(reversed.iter().map(|d| d.path.as_str()).collect::<Vec<_>>(), vec!["/a", "/b"]);

        // Case-insensitive filter.
        let dirs = vec![Dir { path: "C:\\Users".into(), uses: 1, last: 1.0, starred: false }, Dir { path: "/tmp".into(), uses: 1, last: 1.0, starred: false }];
        assert_eq!(dir_filter(&dirs, "USERS"), vec![0]);

        // Abbreviation tree.
        let mut tree = DirTree::default();
        tree.add("/a/bin");
        tree.add("/a/boot");
        tree.add("/a/c");
        assert_eq!(tree.safe("/a/bin"), vec![true, false]);

        let w = |s: &str| s.chars().count() as f32;
        let target = "/u/l/bin";
        assert_eq!(dir_text("/usr/local/bin", false, &[true, true, true], w(target), &w), "/u/l/bin");
        // The last component never shortens, no matter how tight the width.
        assert_eq!(dir_text("/usr/local/bin", false, &[true, true, true], 0.0, &w), "/u/l/bin");
        assert!(dir_text("/tmp", true, &[true], 1000.0, &w).starts_with("\u{2605} /"));

        assert_eq!(dir_insert("/home/me/My Docs", Some("bash"), true, false), "cd /home/me/My\\ Docs");
    }

    #[test]
    fn snippet_and_action_rows_follow_iterm2() {
        let s1 = snip("t", "v", &[]);
        assert_eq!(snippet_text(&s1, false), "t \u{2014} v");

        let long_value = "x".repeat(50);
        let s2 = snip("", &long_value, &[]);
        assert_eq!(snippet_text(&s2, false), format!("{}\u{2026}", "x".repeat(39)));

        let s3 = snip("cmd", "echo hi\nls", &[]);
        assert!(snippet_text(&s3, false).contains("echo hi\u{21a9}ls"));

        let long_value2 = "z".repeat(60);
        let s5 = snip("cmd", &long_value2, &[]);
        assert_eq!(snippet_text(&s5, false), format!("cmd \u{2014} {long_value2}"));
        assert_eq!(snippet_text(&s5, true), format!("cmd \u{2014} {}\u{2026}", "z".repeat(39)));

        // Tree: untagged first in model order; folders sorted ignoring case (a case-sensitive
        // sort would put "Banana" before "apple"); a folder's own snippets before its
        // subfolders; a snippet once per tag; nested a/b paths.
        let snips = vec![
            snip("plain", "1", &[]),
            snip("banana-item", "2", &["Banana"]),
            snip("apple-item", "3", &["apple"]),
            snip("nested-a", "4", &["apple/sub"]),
            snip("nested-b", "5", &["apple/sub"]),
            snip("multi-tag", "6", &["apple", "Banana"]),
        ];
        let shown: Vec<usize> = (0..snips.len()).collect();
        let tree = snippet_tree(&snips, &shown);
        assert_eq!(tree[0], SnippetNode::Snippet(0));
        let (apple, banana) = match (&tree[1], &tree[2]) {
            (SnippetNode::Folder { name: n1, path: p1, children: c1 }, SnippetNode::Folder { name: n2, path: p2, children: c2 }) => {
                assert_eq!(n1, "apple");
                assert_eq!(p1, "apple");
                assert_eq!(n2, "Banana");
                assert_eq!(p2, "Banana");
                (c1.clone(), c2.clone())
            }
            other => panic!("expected two folders, got {other:?}"),
        };
        assert_eq!(apple[0], SnippetNode::Snippet(2), "a folder's own snippets before its subfolders");
        assert!(apple.contains(&SnippetNode::Snippet(5)), "multi-tag snippet appears under apple");
        assert!(banana.contains(&SnippetNode::Snippet(5)), "and once under Banana too");
        match apple.last().unwrap() {
            SnippetNode::Folder { name, path, children } => {
                assert_eq!(name, "sub");
                assert_eq!(path, "apple/sub");
                assert_eq!(children, &vec![SnippetNode::Snippet(3), SnippetNode::Snippet(4)]);
            }
            other => panic!("expected nested sub folder, got {other:?}"),
        }

        // Collapsed folders hide their children.
        let expanded = BTreeSet::new();
        let rows = snippet_rows(&snips, &tree, &expanded, false);
        assert_eq!(rows.iter().filter(|r| matches!(r.kind, SnippetRowKind::Snippet(_))).count(), 1, "nested snippets hidden while folders collapsed");
        let mut expanded2 = BTreeSet::new();
        expanded2.insert("apple".to_string());
        let rows2 = snippet_rows(&snips, &tree, &expanded2, false);
        assert!(rows2.iter().any(|r| matches!(&r.kind, SnippetRowKind::Snippet(i) if *i == 2)));
        assert!(!rows2.iter().any(|r| matches!(&r.kind, SnippetRowKind::Snippet(i) if *i == 3)), "apple/sub still collapsed");

        assert_eq!(snippet_pick(true, true), SnippetPick::AdvancedPaste, "Option wins over Shift");
        assert_eq!(snippet_pick(false, true), SnippetPick::Composer);
        assert_eq!(snippet_pick(false, false), SnippetPick::Send);

        // Actions.
        assert_eq!(action_rows(&[ua(""), ua("named")]), vec!["Untitled".to_string(), "named".to_string()]);

        let actions = vec![ua("a"), ua("b"), ua("c"), ua("d")];
        let moved = move_actions(&actions, &[0], 2);
        assert_eq!(moved.iter().map(|a| a.title.as_str()).collect::<Vec<_>>(), vec!["b", "a", "c", "d"], "dropped above the target");

        // Snippets: child index 2 moves to model index 2 of the unfiltered list (the kept bug).
        let all_snips = vec![snip("s0", "", &[]), snip("s1", "", &[]), snip("s2", "", &[]), snip("s3", "", &[])];
        let moved = move_snippets(&all_snips, std::slice::from_ref(&all_snips[0].guid), 2, 3).unwrap();
        assert_eq!(moved.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(), vec!["s1", "s0", "s2", "s3"]);
        assert!(move_snippets(&all_snips, std::slice::from_ref(&all_snips[0].guid), 4, 3).is_none(), "refuses an index past bound");

        // Named marks.
        let mut m1 = Mark::new(1, Kind::Manual);
        m1.name = Some("checkpoint".into());
        let m2 = Mark::new(2, Kind::Manual);
        let rows = mark_rows(&[(10, m1), (5, m2)]);
        assert_eq!(rows, vec![(1, "checkpoint".to_string()), (2, "(Unnamed)".to_string())], "buffer order kept, not sorted by line");
    }

    #[test]
    fn profile_rows_follow_iterm2() {
        let profiles = vec![
            ("g-default".to_string(), "Default".to_string(), vec!["Work Stuff".to_string()], "bash".to_string()),
            ("g-dev".to_string(), "dev box".to_string(), vec![], "ssh devhost".to_string()),
            ("g-prod".to_string(), "prod box".to_string(), vec![], "ssh prodhost".to_string()),
        ];
        let rows = profile_rows(&profiles, "g-default", "");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].name, "\u{2605} Default");
        assert_eq!(rows[0].tags, "Work Stuff");

        let rows = profile_rows(&profiles, "g-default", "tag:work");
        assert_eq!(rows.iter().map(|r| r.guid.as_str()).collect::<Vec<_>>(), vec!["g-default"]);

        let rows = profile_rows(&profiles, "g-default", "name:dev");
        assert_eq!(rows.iter().map(|r| r.guid.as_str()).collect::<Vec<_>>(), vec!["g-dev"]);

        let rows = profile_rows(&profiles, "g-default", "command:ssh");
        let guids: Vec<&str> = rows.iter().map(|r| r.guid.as_str()).collect();
        assert!(guids.contains(&"g-dev") && guids.contains(&"g-prod"));

        let rows = profile_rows(&profiles, "g-default", "ssh");
        assert!(rows.is_empty(), "a plain word does not search the command");

        let rows = profile_rows(&profiles, "g-default", "-dev");
        let guids: Vec<&str> = rows.iter().map(|r| r.guid.as_str()).collect();
        assert!(!guids.contains(&"g-dev"));
        assert!(guids.contains(&"g-default") && guids.contains(&"g-prod"));

        assert_eq!(profile_open(true, true, true), ProfileOpen::HorizontalSplit);
        assert_eq!(profile_open(false, true, true), ProfileOpen::VerticalSplit);
        assert_eq!(profile_open(false, false, true), ProfileOpen::Window);
        assert_eq!(profile_open(false, false, false), ProfileOpen::Tab);

        let titles: Vec<&str> = PROFILE_OPENS.iter().map(|(_, t)| *t).collect();
        assert_eq!(titles, vec!["New Tab", "New Window", "New Horizontal Split", "New Vertical Split"]);

        assert_eq!(ERASE_PASTE, ("Erase Paste History", "Paste history will be erased. Continue?"));
        assert_eq!(ERASE_COMMANDS, ("Erase Command History", "Command history for all hosts will be erased. Continue?"));
        assert_eq!(ERASE_DIRS, ("Erase Saved Directories?", ""));
    }
}

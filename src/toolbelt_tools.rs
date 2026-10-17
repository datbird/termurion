//! Each of the seven toolbelt tools' rows, filters and pick rules: pure transforms from stored
//! data (snippets, shell history, actions, marks, profiles) to what a tool's list should show,
//! and the modifier-key rules for its buttons and double-click. The shared list widget is
//! `tool_list`; the app wires these tools together in `src/app/toolbelt_tools_ui.rs`.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashSet;

use crate::marks::Mark;
use crate::proctree::Proc;
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
/// still split the same way; only `dir_text`'s prefix and separator differ for it, a ruling:
/// it joins with `\` and has no leading `/`).
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
/// never shortens. A Windows drive path (Termurion stores these as `/C:/...`, `shell_history
/// ::insertable_dir`) keeps its own form: joined with `\`, no leading `/`, and the drive
/// component itself ("C:") is never abbreviated even when the tree marks it safe (it is
/// usually the only child at the root, so it would otherwise always qualify).
pub fn dir_text(path: &str, starred: bool, safe: &[bool], max_w: f32, width: &dyn Fn(&str) -> f32) -> String {
    let raw = dir_components(path);
    let drive = raw.first().is_some_and(|c| is_drive_component(c));
    let sep = if drive { "\\" } else { "/" };
    let star = if starred { "\u{2605} " } else { "" };
    let lead = if drive { "" } else { "/" };
    let mut parts: Vec<String> = raw.iter().map(|s| s.to_string()).collect();
    let build = |parts: &[String]| format!("{star}{lead}{}", parts.join(sep));
    let mut text = build(&parts);
    let start = if drive { 1 } else { 0 };
    for i in start..parts.len().saturating_sub(1) {
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
    folders.sort_by_key(|a| a.name.to_lowercase());
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

/// Each row's identity across a rebuild, so a selection follows its items when a folder opens
/// or closes: the folder path it sits in (a folder row's own path) and the snippet's index
/// (None for a folder). A snippet shown under two tags is two items.
pub fn snippet_row_keys(rows: &[SnippetRow]) -> Vec<(String, Option<usize>)> {
    let mut stack: Vec<String> = Vec::new();
    rows.iter()
        .map(|r| {
            stack.truncate(r.depth);
            match &r.kind {
                SnippetRowKind::Folder(name) => {
                    stack.push(name.clone());
                    (stack.join("/"), None)
                }
                SnippetRowKind::Snippet(k) => (stack.join("/"), Some(*k)),
            }
        })
        .collect()
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

// ---------------------------------------------------------------------------------------------
// Jobs (`JT` = `iTermJobTreeViewController.m`/`ToolJobs.m`, min 76).
// ---------------------------------------------------------------------------------------------

/// One row of the Jobs tool's tree: a process's pid, its depth under the shell, its display
/// text, whether it has children (even while collapsed) and whether it is being watched for
/// termination (the app draws the bell glyph before it).
#[derive(Clone, Debug, PartialEq)]
pub struct JobRow {
    pub pid: u32,
    pub depth: usize,
    pub text: String,
    pub has_children: bool,
    pub watched: bool,
}

/// A row's text: the command line, else the name, cut to 256 characters with `\n` as a space
/// (`JT:245,265-275,771,792-795`); "(terminated)" when neither is there.
fn job_row_text(p: &Proc) -> String {
    let raw = p.cmdline.as_deref().filter(|c| !c.is_empty()).unwrap_or(p.name.as_str());
    if raw.is_empty() {
        return "(terminated)".to_string();
    }
    raw.replace('\n', " ").chars().take(256).collect()
}

/// The Jobs tool's tree, rooted at the shell (`root`), depth-first, a node's children in pid
/// order (`ToolJobs.m:101-104`). The same cycle guard as `proctree::walk` (a child never older
/// than its parent, each pid visited once) keeps a stale or looping pid table from hanging the
/// walk. A pid in `collapsed` still shows its own row but hides its subtree; `has_children` is
/// set either way, so a collapsed row still shows its disclosure. A pid in `watched` is flagged
/// for the app to draw the bell glyph before it.
pub fn job_rows(procs: &[Proc], root: u32, collapsed: &BTreeSet<u32>, watched: &BTreeSet<u32>) -> Vec<JobRow> {
    let Some(r) = procs.iter().find(|p| p.pid == root) else { return Vec::new() };
    let mut seen: HashSet<u32> = HashSet::from([root]);
    let mut out = Vec::new();
    // An explicit stack, not recursion: a node's children are pushed in reverse pid order so
    // the smallest pid is the next one popped, giving a pre-order walk in pid order.
    let mut stack: Vec<(&Proc, usize)> = vec![(r, 0)];
    while let Some((node, depth)) = stack.pop() {
        let mut kids: Vec<&Proc> = procs.iter().filter(|q| q.ppid == node.pid && q.created >= node.created && !seen.contains(&q.pid)).collect();
        kids.sort_by_key(|q| q.pid);
        for k in &kids {
            seen.insert(k.pid);
        }
        let has_children = !kids.is_empty();
        out.push(JobRow { pid: node.pid, depth, text: job_row_text(node), has_children, watched: watched.contains(&node.pid) });
        if !collapsed.contains(&node.pid) {
            for k in kids.into_iter().rev() {
                stack.push((k, depth + 1));
            }
        }
    }
    out
}

/// The six signals Jobs can send (`JT:52-60`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    Hup,
    Int,
    Quit,
    Abrt,
    Kill,
    Term,
}

impl Signal {
    pub fn name(self) -> &'static str {
        match self {
            Signal::Hup => "HUP",
            Signal::Int => "INT",
            Signal::Quit => "QUIT",
            Signal::Abrt => "ABRT",
            Signal::Kill => "KILL",
            Signal::Term => "TERM",
        }
    }

    pub fn number(self) -> i32 {
        match self {
            Signal::Hup => 1,
            Signal::Int => 2,
            Signal::Quit => 3,
            Signal::Abrt => 6,
            Signal::Kill => 9,
            Signal::Term => 15,
        }
    }
}

/// The signal box's rows, in order; the default matches iTerm2's (`JT:52-60`).
pub const SIGNALS: [Signal; 6] = [Signal::Hup, Signal::Int, Signal::Quit, Signal::Abrt, Signal::Kill, Signal::Term];
pub const DEFAULT_SIGNAL: Signal = Signal::Kill;

/// How a signal reaches a process on Windows (Decisions, "Jobs signals on Windows"): INT and
/// QUIT are console events sent to the whole console; the rest are `TerminateProcess` with an
/// exit code of 128 plus the signal number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignalWay {
    CtrlC,
    CtrlBreak,
    Terminate(u32),
}

pub fn signal_way(s: Signal) -> SignalWay {
    match s {
        Signal::Int => SignalWay::CtrlC,
        Signal::Quit => SignalWay::CtrlBreak,
        other => SignalWay::Terminate(128 + other.number() as u32),
    }
}

/// The advanced setting that permanently silences the confirmation (`JT:398-455`).
pub const SEND_SIGNAL_KEY: &str = "NoSyncSuppressSendSignal";

/// The "Confirmation Needed" question before sending a signal (`JT:398-455`): "processes" even
/// for a single one (the kept iTerm2 bug), "and their children" only when a selected row has
/// children.
pub fn signal_question(count: usize, any_children: bool) -> (&'static str, String) {
    let suffix = if any_children { " and their children." } else { "." };
    ("Confirmation Needed", format!("Are you sure? This may terminate {count} processes{suffix}"))
}

/// A terminated job's display name, "(unknown)" for an empty one
/// (`iTermJobTerminationMonitor.swift:139`).
fn job_display_name(name: &str) -> &str {
    if name.is_empty() { "(unknown)" } else { name }
}

fn job_terminated_sentence(name: &str, pid: u32) -> String {
    format!("The job {} with process ID {pid} has terminated.", job_display_name(name))
}

/// "Notify on Termination"'s alert, one job or several (`iTermJobTerminationMonitor.swift:
/// 123-141`): a single job's own sentence, or several bulleted with `"\u{2022} "` and joined by
/// newlines.
pub fn job_terminated_text(jobs: &[(String, u32)]) -> (&'static str, String) {
    if let [(name, pid)] = jobs {
        return ("Job Terminated", job_terminated_sentence(name, *pid));
    }
    let text = jobs.iter().map(|(name, pid)| format!("\u{2022} {}", job_terminated_sentence(name, *pid))).collect::<Vec<_>>().join("\n");
    ("Jobs Terminated", text)
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

    /// A row's identity is its folder path and snippet index, so the same snippet under two
    /// tags is two items, and a selection maps across a folder opening.
    #[test]
    fn snippet_row_keys_name_each_item() {
        let row = |depth: usize, kind: SnippetRowKind| SnippetRow { depth, kind, text: String::new(), tags: Vec::new() };
        let closed = vec![row(0, SnippetRowKind::Folder("apple".into())), row(0, SnippetRowKind::Folder("banana".into())), row(0, SnippetRowKind::Snippet(0))];
        let open = vec![
            row(0, SnippetRowKind::Folder("apple".into())),
            row(1, SnippetRowKind::Snippet(5)),
            row(1, SnippetRowKind::Folder("sub".into())),
            row(2, SnippetRowKind::Snippet(3)),
            row(0, SnippetRowKind::Folder("banana".into())),
            row(1, SnippetRowKind::Snippet(5)),
            row(0, SnippetRowKind::Snippet(0)),
        ];
        let k = |p: &str, i: Option<usize>| (p.to_string(), i);
        assert_eq!(snippet_row_keys(&open), vec![k("apple", None), k("apple", Some(5)), k("apple/sub", None), k("apple/sub", Some(3)), k("banana", None), k("banana", Some(5)), k("", Some(0))]);
        // "banana" and the top-level snippet selected; "apple" opens above them.
        let mut sel = crate::tool_list::Selection::default();
        sel.click(1, false, false, true, 0, 500);
        sel.click(2, true, false, true, 100, 500);
        sel.remap(&snippet_row_keys(&closed), &snippet_row_keys(&open));
        assert_eq!(sel.rows, vec![4, 6]);
    }

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

        // A Windows drive path keeps its own form: `\`-joined, no leading `/`, and the drive
        // component itself is never abbreviated even when the tree marks it safe (the usual
        // case, since it is typically the only child at the root).
        let mut drive_tree = DirTree::default();
        drive_tree.add("/C:/Users/rob");
        let drive_safe = drive_tree.safe("/C:/Users/rob");
        assert_eq!(drive_safe, vec![true, true, true]);
        assert_eq!(dir_text("/C:/Users/rob", false, &drive_safe, 1000.0, &w), "C:\\Users\\rob");
        assert_eq!(dir_text("/C:/Users/rob", false, &drive_safe, 0.0, &w), "C:\\U\\rob", "the drive component never shortens; the last component never shortens either");

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

    fn proc(pid: u32, ppid: u32, name: &str, created: u64, cmdline: Option<&str>) -> Proc {
        Proc { pid, ppid, name: name.into(), created, gui: false, cmdline: cmdline.map(str::to_string) }
    }

    #[test]
    fn jobs_rules_follow_iterm2() {
        // The shell (1) first at depth 0; its children (10, 11) in pid order below it, not
        // creation order; 11's own child (12) nested under it.
        let procs = vec![
            proc(1, 0, "bash", 0, None),
            proc(11, 1, "vim", 2, Some("vim notes.md")),
            proc(10, 1, "git", 1, None),
            proc(12, 11, "gpg", 3, None),
        ];
        let empty = BTreeSet::new();
        let rows = job_rows(&procs, 1, &empty, &empty);
        assert_eq!(rows.iter().map(|r| (r.pid, r.depth)).collect::<Vec<_>>(), vec![(1, 0), (10, 1), (11, 1), (12, 2)]);
        assert_eq!(rows[0].text, "bash", "no command line: the name");
        assert_eq!(rows[2].text, "vim notes.md", "a command line wins over the name");
        assert!(rows[2].has_children && !rows[3].has_children);

        // A newline in the command line becomes a space, and it cuts to 256 characters.
        let long = "x".repeat(300);
        let with_line = vec![proc(1, 0, "bash", 0, None), proc(10, 1, "cat", 1, Some(&format!("cat a\nb {long}")))];
        let rows = job_rows(&with_line, 1, &empty, &empty);
        assert_eq!(rows[1].text.len(), 256);
        assert!(!rows[1].text.contains('\n'));
        assert!(rows[1].text.starts_with("cat a b x"));

        // An unknown name (and no command line) reads as terminated.
        let unknown = vec![proc(1, 0, "bash", 0, None), proc(10, 1, "", 1, None)];
        assert_eq!(job_rows(&unknown, 1, &empty, &empty)[1].text, "(terminated)");

        // A collapsed pid still shows its own row (with has_children set) but hides its subtree.
        let mut collapsed = BTreeSet::new();
        collapsed.insert(11);
        let rows = job_rows(&procs, 1, &collapsed, &empty);
        assert_eq!(rows.iter().map(|r| r.pid).collect::<Vec<_>>(), vec![1, 10, 11]);
        assert!(rows[2].has_children, "the disclosure still shows on a collapsed row");

        // A watched pid is flagged.
        let mut watched = BTreeSet::new();
        watched.insert(10);
        let rows = job_rows(&procs, 1, &empty, &watched);
        assert!(rows.iter().find(|r| r.pid == 10).unwrap().watched);
        assert!(!rows.iter().find(|r| r.pid == 11).unwrap().watched);

        // No shell: no rows.
        assert!(job_rows(&procs, 999, &empty, &empty).is_empty());

        // Signals: names, numbers, order.
        let names: Vec<&str> = SIGNALS.iter().map(|s| s.name()).collect();
        assert_eq!(names, vec!["HUP", "INT", "QUIT", "ABRT", "KILL", "TERM"]);
        let numbers: Vec<i32> = SIGNALS.iter().map(|s| s.number()).collect();
        assert_eq!(numbers, vec![1, 2, 3, 6, 9, 15]);
        assert_eq!(DEFAULT_SIGNAL, Signal::Kill);

        assert_eq!(signal_way(Signal::Int), SignalWay::CtrlC);
        assert_eq!(signal_way(Signal::Quit), SignalWay::CtrlBreak);
        assert_eq!(signal_way(Signal::Hup), SignalWay::Terminate(129));
        assert_eq!(signal_way(Signal::Abrt), SignalWay::Terminate(134));
        assert_eq!(signal_way(Signal::Kill), SignalWay::Terminate(137));
        assert_eq!(signal_way(Signal::Term), SignalWay::Terminate(143));

        assert_eq!(SEND_SIGNAL_KEY, "NoSyncSuppressSendSignal");

        assert_eq!(signal_question(1, false), ("Confirmation Needed", "Are you sure? This may terminate 1 processes.".to_string()));
        let (title, text) = signal_question(2, true);
        assert_eq!(title, "Confirmation Needed");
        assert!(text.ends_with("2 processes and their children."), "{text}");

        assert_eq!(job_terminated_text(&[("vim".to_string(), 7)]), ("Job Terminated", "The job vim with process ID 7 has terminated.".to_string()));
        let (title, text) = job_terminated_text(&[("vim".to_string(), 7), ("ls".to_string(), 8)]);
        assert_eq!(title, "Jobs Terminated");
        assert_eq!(text, "\u{2022} The job vim with process ID 7 has terminated.\n\u{2022} The job ls with process ID 8 has terminated.");
        // An empty name reads as unknown.
        let (_, text) = job_terminated_text(&[(String::new(), 9)]);
        assert!(text.contains("(unknown)"));

    }
}

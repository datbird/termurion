//! iTerm2's snippets (iTermSnippetsModel): saved text with a title, tags, and its own
//! escaping, the file they live in, the Edit > Snippets tag tree, and the search.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::keyaction::Escaping;

/// Written by this version (iTerm2's currentVersion).
pub const VERSION: i64 = 2;

#[derive(Clone, Debug, PartialEq)]
pub struct Snippet {
    pub title: String,
    pub value: String,
    pub guid: String,
    pub tags: Vec<String>,
    pub escaping: Escaping,
    pub version: i64,
}

/// iTerm2's ellipsizedDescriptionNoLongerThan: newlines become U+21A9, leading blanks go,
/// and a longer text keeps `limit - 1` characters and an ellipsis.
pub fn ellipsize(s: &str, limit: usize) -> String {
    let flat: String = s.chars().map(|c| if c == '\r' || c == '\n' { '\u{21a9}' } else { c }).collect();
    let blank = |c: char| c == ' ' || c == '\t';
    if flat.chars().count() <= limit {
        return flat.trim_matches(blank).to_string();
    }
    let Some(start) = flat.find(|c: char| !c.is_whitespace()) else { return String::new() };
    let rest = &flat[start..];
    if rest.chars().count() < limit {
        return rest.trim_matches(blank).to_string();
    }
    let kept: String = rest.chars().take(limit.saturating_sub(1)).collect();
    format!("{}\u{2026}", kept.trim_matches(blank))
}

/// iTerm2's hashWithSHA256 of an array of strings, chained, in hex.
fn fallback_guid(parts: &[&str]) -> String {
    let mut hash = Sha256::digest(b"").to_vec();
    for p in parts {
        let mut h = Sha256::new();
        h.update(&hash);
        h.update(Sha256::digest(p.as_bytes()));
        hash = h.finalize().to_vec();
    }
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

fn guid_of_key(key: &str) -> Option<String> {
    let v: Value = serde_json::from_str(key).ok()?;
    let o = v.as_object()?;
    if o.len() != 1 {
        return None;
    }
    o.get("guid")?.as_str().map(str::to_string)
}

impl Snippet {
    /// One of iTerm2's snippet dictionaries; `index` makes the fallback GUID of old files.
    pub fn from_json(v: &Value, index: usize) -> Option<Snippet> {
        let o = v.as_object()?;
        let text = |k: &str| o.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        let title = text("title");
        let value = text("value");
        let version = o.get("version").and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))).unwrap_or(0);
        let escaping = match version {
            0 => Escaping::Compatibility,
            1 => Escaping::Common,
            _ => Escaping::from_code(o.get("escaping").and_then(Value::as_i64).unwrap_or(0)),
        };
        let guid = match o.get("guid").and_then(Value::as_str) {
            Some(g) => g.to_string(),
            None => fallback_guid(&[&index.to_string(), &title, &value]),
        };
        let tags = o.get("tags").and_then(Value::as_array).map(|a| a.iter().filter_map(|t| t.as_str().map(str::to_string)).collect()).unwrap_or_default();
        Some(Snippet { title, value, guid, tags, escaping, version })
    }

    pub fn to_json(&self) -> Value {
        json!({"title": self.title, "value": self.value, "guid": self.guid, "tags": self.tags, "version": self.version, "escaping": self.escaping.code()})
    }

    /// The title, or the value ellipsized to 30 characters when the title is empty.
    pub fn display_title(&self) -> String {
        if self.title.is_empty() { crate::inert_files::display_text(&ellipsize(&self.value, 30), 200, false) } else { crate::inert_files::display_text(&self.title, 200, false) }
    }

    /// What a Send Snippet key action stores: `{"guid":"..."}`.
    pub fn action_key(&self) -> String {
        json!({"guid": self.guid}).to_string()
    }

    /// A stored key: the GUID dictionary, or (older bindings) the title itself.
    pub fn matches_key(&self, key: &str) -> bool {
        guid_of_key(key).is_some_and(|g| g == self.guid) || (!key.is_empty() && key == self.title)
    }
}

/// The snippets file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Store {
    pub snippets: Vec<Snippet>,
    /// The file was there but did not parse: it is never replaced by an empty list.
    pub broken: bool,
}

impl Store {
    /// The file's text, or None when there is no file.
    pub fn from_file_text(text: Option<&str>) -> Store {
        let Some(text) = text else { return Store::default() };
        let list = serde_json::from_str::<Value>(text).ok().and_then(|v| v.get("snippets").and_then(Value::as_array).cloned());
        match list {
            Some(list) => Store { snippets: list.iter().enumerate().filter_map(|(i, v)| Snippet::from_json(v, i)).collect(), broken: false },
            None => Store { snippets: Vec::new(), broken: true },
        }
    }

    /// The text to write, or None when nothing may be written.
    pub fn file_text(&self) -> Option<String> {
        if self.broken && self.snippets.is_empty() {
            return None;
        }
        let list: Vec<Value> = self.snippets.iter().map(Snippet::to_json).collect();
        serde_json::to_string_pretty(&json!({ "snippets": list })).ok()
    }

    /// The file as read: only a missing file is an empty list.
    pub fn from_read(read: Result<String, std::io::ErrorKind>) -> Store {
        match read {
            Ok(text) => Store::from_file_text(Some(&text)),
            Err(std::io::ErrorKind::NotFound) => Store::from_file_text(None),
            Err(_) => Store { snippets: Vec::new(), broken: true },
        }
    }

    /// Imported snippets go at the end; a GUID already in the list gets a new one.
    pub fn add_imported(&mut self, list: Vec<Snippet>) {
        for mut s in list {
            if self.snippets.iter().any(|x| x.guid == s.guid) {
                s.guid = uuid::Uuid::new_v4().to_string().to_uppercase();
            }
            self.snippets.push(s);
        }
    }

    /// Puts an edited snippet where its GUID is. False when it is gone.
    pub fn replace(&mut self, s: Snippet) -> bool {
        match self.snippets.iter_mut().find(|x| x.guid == s.guid) {
            Some(x) => {
                *x = s;
                true
            }
            None => false,
        }
    }

    pub fn find_key(&self, key: &str) -> Option<&Snippet> {
        self.snippets.iter().find(|s| s.matches_key(key))
    }
}

/// The snippets holding every one of `tags` (the profile's Snippets Filter), exact names.
pub fn filtered(snippets: &[Snippet], tags: &[String]) -> Vec<usize> {
    (0..snippets.len()).filter(|&i| tags.iter().all(|t| snippets[i].tags.contains(t))).collect()
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuNode {
    Snippet(usize),
    Folder(String, Vec<MenuNode>),
}

#[derive(Default)]
struct Folder {
    name: String,
    folders: Vec<Folder>,
    snippets: Vec<usize>,
}

fn add_path(tree: &mut Vec<Folder>, parts: &[&str], snippet: usize) {
    let i = match tree.iter().position(|f| f.name == parts[0]) {
        Some(i) => i,
        None => {
            tree.push(Folder { name: parts[0].to_string(), ..Folder::default() });
            tree.len() - 1
        }
    };
    if parts.len() == 1 {
        tree[i].snippets.push(snippet);
    } else {
        add_path(&mut tree[i].folders, &parts[1..], snippet);
    }
}

fn nodes_of(mut tree: Vec<Folder>, snippets: &[Snippet]) -> Vec<MenuNode> {
    tree.sort_by(|a, b| a.name.cmp(&b.name));
    tree.into_iter()
        .map(|mut f| {
            let mut children = nodes_of(f.folders, snippets);
            f.snippets.sort_by(|&a, &b| snippets[a].title.cmp(&snippets[b].title));
            children.extend(f.snippets.into_iter().map(MenuNode::Snippet));
            MenuNode::Folder(f.name, children)
        })
        .collect()
}

/// Edit > Snippets below its hint: the untagged snippets in order, then the tag tree (each
/// tag split on `/`; folders sorted; in a folder its subfolders, then its snippets by title).
pub fn menu_tree(snippets: &[Snippet], shown: &[usize]) -> (Vec<usize>, Vec<MenuNode>) {
    let mut untagged = Vec::new();
    let mut tree = Vec::new();
    for &i in shown {
        if snippets[i].tags.is_empty() {
            untagged.push(i);
        }
        for tag in &snippets[i].tags {
            let parts: Vec<&str> = tag.split('/').collect();
            add_path(&mut tree, &parts, i);
        }
    }
    (untagged, nodes_of(tree, snippets))
}

/// One line of Edit > Snippets after its hint.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuEntry {
    /// A snippet: its GUID and display title.
    Item(String, String),
    Line,
    /// A folder starts (its name) or ends.
    Open(String),
    Close,
}

/// Edit > Snippets below its hint and line, in order.
pub fn menu_entries(snippets: &[Snippet], shown: &[usize]) -> Vec<MenuEntry> {
    fn node(n: &MenuNode, s: &[Snippet], out: &mut Vec<MenuEntry>) {
        match n {
            MenuNode::Snippet(i) => out.push(MenuEntry::Item(s[*i].guid.clone(), s[*i].display_title())),
            MenuNode::Folder(name, children) => {
                out.push(MenuEntry::Open(name.clone()));
                // Subfolders, a line, then the folder's snippets.
                let folders = children.iter().filter(|c| matches!(c, MenuNode::Folder(..))).count();
                for (k, c) in children.iter().enumerate() {
                    if k == folders && folders > 0 {
                        out.push(MenuEntry::Line);
                    }
                    node(c, s, out);
                }
                out.push(MenuEntry::Close);
            }
        }
    }
    let (untagged, tree) = menu_tree(snippets, shown);
    let mut out: Vec<MenuEntry> = untagged.iter().map(|&i| MenuEntry::Item(snippets[i].guid.clone(), snippets[i].display_title())).collect();
    if !tree.is_empty() {
        out.push(MenuEntry::Line);
        for n in &tree {
            node(n, snippets, &mut out);
        }
    }
    out
}

/// iTerm2's componentsBySplittingProfileListQuery: words, with quotes and backslashes.
fn query_words(q: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut quoted, mut escape, mut first) = (false, false, true);
    let chars: Vec<char> = q.chars().map(|c| if c == '\0' { ' ' } else { c }).collect();
    for i in 0..=chars.len() {
        let end = i == chars.len();
        let c = if end { '\0' } else { chars[i] };
        if end {
            escape = false;
        }
        if c == '\\' && !escape {
            escape = true;
            continue;
        }
        if escape {
            first = false;
            escape = false;
            if quoted && c != '"' && c != '\\' {
                cur.push('\\');
            }
            cur.push(c);
            continue;
        }
        if c == '"' {
            quoted = !quoted;
            first = false;
            continue;
        }
        if end {
            quoted = false;
        }
        if !quoted && (end || c.is_whitespace()) {
            if !first {
                out.push(std::mem::take(&mut cur));
                first = true;
            }
            continue;
        }
        first = false;
        cur.push(c);
    }
    out
}

/// Swift's split(separator:escape:).
fn split_escaped(s: &str, sep: char) -> Vec<String> {
    if s.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut escaped = false;
    for c in s.chars() {
        if escaped {
            cur.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == sep {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    out.push(cur);
    out
}

/// Snippet search's own operators (`title:`/`text:`, not profile search's `name:`/`command:`).
const OPERATORS: [&str; 3] = ["tag:", "title:", "text:"];

/// iTermProfileSearchToken.
struct Token {
    negated: bool,
    op: Option<&'static str>,
    strings: Vec<String>,
    anchor_start: bool,
    anchor_end: bool,
}

impl Token {
    /// Parses one phrase against a caller-chosen operator set (`iTermProfileSearchToken.m`'s
    /// `initWithPhrase:operators:`): snippet search's own `tag:`/`title:`/`text:`, or profile
    /// search's `tag:`/`name:`/`command:` (`profile_matches`). `operators` is `'static` so this
    /// keeps borrowing from the caller's own const array rather than allocating.
    fn phrase(original: &str, operators: &'static [&'static str]) -> Token {
        let (negated, phrase) = match original.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, original),
        };
        let mut op = None;
        let mut string = phrase;
        for &o in operators {
            if let Some(rest) = phrase.strip_prefix(o) {
                op = Some(o);
                string = rest;
                break;
            }
        }
        if phrase.chars().count() >= 2 && phrase.starts_with('"') && phrase.ends_with('"') {
            string = &phrase[1..phrase.len() - 1];
        }
        let mut anchor_start = false;
        if let Some(rest) = string.strip_prefix('^') {
            anchor_start = true;
            string = rest;
        } else if let Some(rest) = string.strip_prefix('*') {
            string = rest;
        }
        let mut anchor_end = false;
        if let Some(rest) = string.strip_suffix('$') {
            anchor_end = true;
            string = rest;
        }
        Token { negated, op, strings: string.split(' ').map(str::to_string).collect(), anchor_start, anchor_end }
    }

    fn tag(tag: &str) -> Token {
        Token { negated: false, op: Some("tag:"), strings: tag.split(' ').map(str::to_string).collect(), anchor_start: false, anchor_end: false }
    }

    fn matches_words(&self, words: &[&str]) -> bool {
        let needle = self.strings.join(" ").to_lowercase();
        if needle.is_empty() {
            return true;
        }
        let n = self.strings.len();
        if n > words.len() {
            return false;
        }
        (0..=words.len() - n).any(|i| {
            let hay = words[i..i + n].join(" ").to_lowercase();
            match (self.anchor_start, self.anchor_end) {
                (true, true) => hay == needle,
                (true, false) => hay.starts_with(&needle),
                // The first occurrence must end the words (NSString's rangeOfString).
                (false, true) => hay.find(&needle).is_some_and(|at| at + needle.len() == hay.len()),
                (false, false) => hay.contains(&needle),
            }
        })
    }

    fn matches_phrase(&self, op: &str, words: &[&str]) -> bool {
        self.op.is_none_or(|o| o == op) && self.matches_words(words)
    }

    fn matches_tag(&self, words: &[&str]) -> bool {
        self.op.is_none_or(|o| o == "tag:") && self.matches_words(words)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Status {
    Excluded,
    None,
    Matched,
}

fn token_status(t: &Token, s: &Snippet) -> Status {
    let mut found = false;
    for (op, content) in [("title:", &s.title), ("text:", &s.value)] {
        let words: Vec<&str> = content.split(|c: char| c.is_whitespace()).collect();
        if t.matches_phrase(op, &words) {
            if t.negated {
                return Status::Excluded;
            }
            found = true;
        }
    }
    let mut tag_hit = false;
    for tag in &s.tags {
        let words: Vec<&str> = tag.split([' ', '\t']).collect();
        if t.matches_tag(&words) {
            if t.negated {
                return Status::Excluded;
            }
            tag_hit = true;
        }
    }
    if found || tag_hit {
        Status::Matched
    } else if t.negated {
        Status::None
    } else {
        Status::Excluded
    }
}

/// iTerm2's snippet search: `tag:`, `title:`, `text:`, `-` to exclude, `|` for either,
/// `^` and `$` anchors, quotes for phrases; `extra_tags` must match too.
pub fn search(snippets: &[Snippet], query: &str, extra_tags: &[String]) -> Vec<usize> {
    let mut nodes: Vec<Vec<Token>> = query_words(query).iter().map(|p| split_escaped(p, '|').iter().map(|t| Token::phrase(t, &OPERATORS)).collect::<Vec<_>>()).filter(|n| !n.is_empty()).collect();
    for tag in extra_tags {
        let node: Vec<Token> = split_escaped(tag, '|').iter().map(|t| Token::tag(t)).collect();
        if !node.is_empty() {
            nodes.push(node);
        }
    }
    (0..snippets.len())
        .filter(|&i| nodes.iter().all(|node| node.iter().map(|t| token_status(t, &snippets[i])).max().unwrap_or(Status::None) != Status::Excluded))
        .collect()
}

/// Profile search's own operators (`iTermProfileSearchToken.m:43`): `tag:`, `name:`, `command:`.
/// Unlike snippet search there is no `|` alternation (`ProfileModel.m:341-343`'s `parseFilter:`
/// makes one token per phrase, never splitting on `|`).
const PROFILE_OPERATORS: [&str; 3] = ["tag:", "name:", "command:"];

fn words(s: &str) -> Vec<&str> {
    s.split(|c: char| c.is_whitespace()).collect()
}

/// iTerm2's profile search (`ProfileModel.m:300-343`, `doesProfileWithName:tags:command:
/// matchFilter:...`), reusing `query_words` and `Token`. A plain word matches the name or a tag;
/// `tag:` matches only tags; `name:` only the name; `command:` only when named explicitly (and,
/// per the source, still also checks the name: `matchesAnyWordInNameWords` allows any operator
/// but `tag:`). `-` negates: a negated token excludes the profile wherever it matches, but never
/// requires a match to pass.
pub fn profile_matches(query: &str, name: &str, tags: &[String], command: &str) -> bool {
    let name_words = words(name);
    let command_words = words(command);
    for phrase in query_words(query) {
        let t = Token::phrase(&phrase, &PROFILE_OPERATORS);
        let mut found = false;
        if t.op != Some("tag:") && t.matches_words(&name_words) {
            if t.negated {
                return false;
            }
            found = true;
        }
        if (t.op.is_none() || t.op == Some("tag:")) && tags.iter().any(|tag| t.matches_words(&words(tag))) {
            if t.negated {
                return false;
            }
            found = true;
        }
        if t.op == Some("command:") && t.matches_words(&command_words) {
            if t.negated {
                return false;
            }
            found = true;
        }
        if !t.negated && !found {
            return false;
        }
    }
    true
}

/// An `.it2snippets` file: a JSON array of snippet dictionaries.
pub fn import(text: &str) -> Result<Vec<Snippet>, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let list = v.as_array().ok_or_else(|| "Malformed file".to_string())?;
    // iTerm2 refuses an entry without a GUID.
    list.iter().enumerate().map(|(i, s)| s.get("guid").and_then(Value::as_str).and(Snippet::from_json(s, i)).ok_or_else(|| "Malformed file".to_string())).collect()
}

pub fn export(snippets: &[&Snippet]) -> String {
    let list: Vec<Value> = snippets.iter().map(|s| s.to_json()).collect();
    serde_json::to_string_pretty(&list).unwrap_or_default()
}

/// A new snippet: from the clipboard text when there is some, else "Untitled".
pub fn new_snippet(clipboard: Option<&str>) -> Snippet {
    let (title, value) = match clipboard.filter(|c| !c.is_empty()) {
        Some(c) => (ellipsize(c, 40), c.to_string()),
        None => ("Untitled".to_string(), String::new()),
    };
    Snippet { title, value, guid: uuid::Uuid::new_v4().to_string().to_uppercase(), tags: Vec::new(), escaping: Escaping::Common, version: VERSION }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snip(title: &str, value: &str, tags: &[&str]) -> Snippet {
        Snippet { title: title.into(), value: value.into(), guid: format!("g-{title}"), tags: tags.iter().map(|t| t.to_string()).collect(), escaping: Escaping::Common, version: VERSION }
    }

    #[test]
    fn snippets_decode_like_iterm2() {
        let v0 = Snippet::from_json(&json!({"title": "t", "value": "v"}), 3).unwrap();
        assert_eq!(v0.escaping, Escaping::Compatibility);
        assert!(v0.tags.is_empty());
        // iTerm2's fallback GUID: the chained SHA-256 of ["3", "t", "v"], in hex.
        assert_eq!(v0.guid, "6e65c62d3a5ee61346d301ddc483b606f904da1e86a55c0275af0af60702acb7");
        assert_eq!(Snippet::from_json(&json!({"title": "t", "value": "v"}), 3).unwrap().guid, v0.guid);
        assert_ne!(Snippet::from_json(&json!({"title": "t", "value": "v"}), 4).unwrap().guid, v0.guid);
        let v1 = Snippet::from_json(&json!({"title": "t", "value": "v", "version": 1, "escaping": 0}), 0).unwrap();
        assert_eq!(v1.escaping, Escaping::Common);
        let v2 = Snippet::from_json(&json!({"title": "t", "value": "v", "version": 2, "escaping": 0, "guid": "G", "tags": ["a"]}), 0).unwrap();
        assert_eq!((v2.escaping, v2.guid.as_str(), v2.tags.clone()), (Escaping::None, "G", vec!["a".to_string()]));
        assert_eq!(v2.to_json(), json!({"title": "t", "value": "v", "guid": "G", "tags": ["a"], "version": 2, "escaping": 0}));
        assert_eq!(Snippet::from_json(&json!("x"), 0), None);
        // Display titles.
        assert_eq!(snip("T", "v", &[]).display_title(), "T");
        assert_eq!(snip("", "  echo hi\nls", &[]).display_title(), "echo hi\u{21a9}ls");
        assert_eq!(snip("", &"x".repeat(40), &[]).display_title(), format!("{}\u{2026}", "x".repeat(29)));
        assert_eq!(ellipsize("abc", 3), "abc");
        assert_eq!(ellipsize("abcd", 3), "ab\u{2026}");
    }

    #[test]
    fn action_keys_match_guid_or_title() {
        let s = snip("deploy", "make deploy", &[]);
        assert_eq!(s.action_key(), r#"{"guid":"g-deploy"}"#);
        assert!(s.matches_key(&s.action_key()));
        assert!(s.matches_key(r#"{ "guid" : "g-deploy" }"#));
        assert!(s.matches_key("deploy"));
        assert!(!s.matches_key(r#"{"guid":"other"}"#));
        assert!(!s.matches_key("Deploy"));
        // A binding saved without a snippet matches none, not an untitled one.
        assert!(!snip("", "x", &[]).matches_key(""));
        let store = Store { snippets: vec![snip("a", "1", &[]), s.clone()], broken: false };
        assert_eq!(store.find_key("deploy").map(|s| s.value.as_str()), Some("make deploy"));
        assert_eq!(store.find_key(r#"{"guid":"nope"}"#), None);
    }

    #[test]
    fn store_never_clobbers_a_broken_file() {
        let none = Store::from_file_text(None);
        assert!(none.snippets.is_empty() && !none.broken);
        assert_eq!(none.file_text().as_deref(), Some("{\n  \"snippets\": []\n}"));
        let broken = Store::from_file_text(Some("{not json"));
        assert!(broken.snippets.is_empty() && broken.broken);
        assert_eq!(broken.file_text(), None);
        // Once there is something to save, it is saved.
        let mut fixed = broken.clone();
        fixed.snippets.push(snip("a", "1", &[]));
        assert!(fixed.file_text().is_some());
        let good = Store::from_file_text(Some(r#"{"snippets":[{"title":"a","value":"1","guid":"G","version":2,"escaping":2}]}"#));
        assert_eq!(good.snippets.len(), 1);
        assert_eq!(Store::from_file_text(Some(&good.file_text().unwrap())), good);
        // No "snippets" array is broken too.
        assert!(Store::from_file_text(Some("{}")).broken);
        // A file that exists but cannot be read is broken; only a missing one is empty.
        let missing = Store::from_read(Err(std::io::ErrorKind::NotFound));
        assert!(!missing.broken && missing.file_text().is_some());
        let locked = Store::from_read(Err(std::io::ErrorKind::PermissionDenied));
        assert!(locked.broken && locked.file_text().is_none());
        assert_eq!(Store::from_read(Ok(good.file_text().unwrap())), good);
    }

    #[test]
    fn menu_tree_follows_iterm2() {
        let s = vec![
            snip("zeta", "1", &[]),
            snip("beta", "2", &["work/git"]),
            snip("alpha", "3", &["work/git", "home"]),
            snip("gamma", "4", &["work"]),
            snip("delta", "5", &["work"]),
            snip("eps", "6", &[]),
            // iTerm2 loses this one: its folder node for "x" (from "x/y") has no snippet list.
            snip("lost", "7", &["x/y"]),
            snip("found", "8", &["x"]),
        ];
        let all: Vec<usize> = (0..s.len()).collect();
        let (untagged, tree) = menu_tree(&s, &all);
        assert_eq!(untagged, vec![0, 5]);
        assert_eq!(
            tree,
            vec![
                MenuNode::Folder("home".into(), vec![MenuNode::Snippet(2)]),
                MenuNode::Folder("work".into(), vec![MenuNode::Folder("git".into(), vec![MenuNode::Snippet(2), MenuNode::Snippet(1)]), MenuNode::Snippet(4), MenuNode::Snippet(3)]),
                MenuNode::Folder("x".into(), vec![MenuNode::Folder("y".into(), vec![MenuNode::Snippet(6)]), MenuNode::Snippet(7)]),
            ]
        );
        // The profile filter: every tag, exact.
        assert_eq!(filtered(&s, &["work/git".into()]), vec![1, 2]);
        assert_eq!(filtered(&s, &["work/git".into(), "home".into()]), vec![2]);
        assert_eq!(filtered(&s, &["Work".into()]), Vec::<usize>::new());
        assert_eq!(filtered(&s, &[]).len(), s.len());
        let (u, t) = menu_tree(&s, &[1, 2]);
        assert!(u.is_empty());
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn menu_entries_follow_iterm2() {
        use MenuEntry::*;
        let s = vec![snip("zeta", "1", &[]), snip("", "echo hi", &[]), snip("b", "2", &["w/g"]), snip("a", "3", &["w"])];
        let item = |i: usize| Item(s[i].guid.clone(), s[i].display_title());
        assert_eq!(menu_entries(&s, &[0, 1, 2, 3]), vec![item(0), item(1), Line, Open("w".into()), Open("g".into()), item(2), Close, Line, item(3), Close]);
        assert_eq!(menu_entries(&s, &[0]), vec![item(0)]);
        assert_eq!(menu_entries(&s, &[3]), vec![Line, Open("w".into()), item(3), Close]);
    }

    #[test]
    fn search_follows_iterm2() {
        let s = vec![
            snip("list files", "ls -la", &["shell"]),
            snip("old deploy", "make deploy", &["git ops"]),
            snip("status", "git status", &["git ops"]),
            snip("Push It", "git push origin", &[]),
        ];
        assert_eq!(search(&s, "", &[]), vec![0, 1, 2, 3]);
        assert_eq!(search(&s, "git", &[]), vec![1, 2, 3]);
        assert_eq!(search(&s, "tag:git", &[]), vec![1, 2]);
        assert_eq!(search(&s, "tag:git -title:old", &[]), vec![2]);
        assert_eq!(search(&s, "^ls", &[]), vec![0]);
        assert_eq!(search(&s, "atus$", &[]), vec![2]);
        assert_eq!(search(&s, "push", &[]), vec![3]);
        assert_eq!(search(&s, "text:deploy|title:list", &[]), vec![0, 1]);
        assert_eq!(search(&s, "\"push origin\"", &[]), vec![3]);
        assert_eq!(search(&s, "title:\"push it\"", &[]), vec![3]);
        assert_eq!(search(&s, "-git", &[]), vec![0]);
        // The profile's Snippets Filter adds tag tokens, matched as substrings of tag words.
        assert_eq!(search(&s, "", &["ops".into()]), vec![1, 2]);
        assert_eq!(search(&s, "make", &["ops".into()]), vec![1]);
    }

    #[test]
    fn profile_search_uses_the_snippet_tokens() {
        // Quotes and escapes route through the same query_words as snippet search.
        assert!(profile_matches("\"dev box\"", "dev box", &[], ""));
        assert!(!profile_matches("\"dev box\"", "prod box", &[], ""));
        // tag:, name:, command: operators (ProfileModel.m:300-343), not snippets' tag:/title:/text:.
        assert!(profile_matches("tag:work", "x", &["work stuff".to_string()], ""));
        assert!(profile_matches("name:dev", "dev box", &[], "irrelevant"));
        assert!(!profile_matches("ssh", "x", &[], "ssh host"), "plain word does not search the command");
        assert!(profile_matches("command:ssh", "x", &[], "ssh host"));
        assert!(!profile_matches("-dev", "dev box", &[], ""));
        // No `|` alternation in profile search: a literal pipe must match literally, not split.
        assert!(profile_matches("a|b", "a|b", &[], ""));
        assert!(!profile_matches("a|b", "a", &[], ""));
    }

    #[test]
    fn import_and_export_round_trip() {
        let a = snip("a", "1", &["t"]);
        let b = snip("b", "2\n", &[]);
        let text = export(&[&a, &b]);
        assert_eq!(import(&text).unwrap(), vec![a.clone(), b]);
        assert!(import("{}").unwrap_err().starts_with("Malformed file"));
        // iTerm2 refuses an imported snippet without a GUID.
        assert!(import(r#"[{"title":"a","value":"1"}]"#).is_err());
        // Imported GUIDs already in the list get new ones.
        let mut store = Store { snippets: vec![a.clone()], broken: false };
        store.add_imported(import(&text).unwrap());
        assert_eq!(store.snippets.len(), 3);
        assert_eq!(store.snippets[1].title, "a");
        assert_ne!(store.snippets[1].guid, a.guid);
        assert_eq!(store.snippets[2].guid, "g-b");
        // An edit finds its snippet by GUID wherever it moved.
        let mut edited = a.clone();
        edited.value = "changed".into();
        store.snippets.insert(0, snip("z", "9", &[]));
        assert!(store.replace(edited));
        assert_eq!(store.snippets[1].value, "changed");
        assert!(!store.replace(snip("gone", "", &[])));
        assert!(import("nope").is_err());
        // New snippets.
        let n = new_snippet(None);
        assert_eq!((n.title.as_str(), n.value.as_str(), n.escaping, n.version), ("Untitled", "", Escaping::Common, VERSION));
        assert_eq!(n.guid.len(), 36);
        let long = "x".repeat(50);
        let c = new_snippet(Some(&long));
        assert_eq!(c.value, long);
        assert_eq!(c.title, format!("{}\u{2026}", "x".repeat(39)));
        assert_ne!(new_snippet(None).guid, n.guid);
    }
}

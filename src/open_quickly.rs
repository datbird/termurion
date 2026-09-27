//! Open Quickly, pure model: the minimum-subsequence matcher, per-feature scoring, commands,
//! tips and the ranked item list (iTerm2's `iTermMinimumSubsequenceMatcher.m`,
//! `iTermOpenQuicklyModel.m` (`M`), `iTermOpenQuicklyCommands.m`,
//! `iTermOpenQuicklyWindowController.m` (`WC`)). The window that hosts this model is
//! `src/open_quickly_ui.rs` and `src/app/open_quickly_app.rs`.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

// -------------------------------------------------------------------------------------------
// Minimum subsequence matcher (`iTermMinimumSubsequenceMatcher.m`).
// -------------------------------------------------------------------------------------------

/// The smallest-span match of `query` as a subsequence of `doc`, case-insensitive: the char
/// indexes into `doc` of each matched query character, in order. `None` when a query character
/// does not occur in `doc` at all. An empty query has no defined span, so it is `None`; callers
/// score an empty query separately (`score_feature`) and never call this for one.
pub fn matched(query: &str, doc: &str) -> Option<Vec<usize>> {
    // Folded one char at a time (not `str::to_lowercase`, which can turn one char into several,
    // e.g. '\u{130}' into two): this keeps `q` and `d` the same length as the originals, so their
    // indexes still point into `query` and `doc`.
    let q: Vec<char> = query.chars().map(fold_char).collect();
    if q.is_empty() {
        return None;
    }
    let d: Vec<char> = doc.chars().map(fold_char).collect();

    let mut posting: HashMap<char, Vec<usize>> = HashMap::new();
    for &c in &q {
        posting.entry(c).or_default();
    }
    for (i, &c) in d.iter().enumerate() {
        if let Some(v) = posting.get_mut(&c) {
            v.push(i);
        }
    }
    for &c in &q {
        if posting.get(&c).is_none_or(|v| v.is_empty()) {
            return None;
        }
    }

    // The initial match: for each query char in order, the earliest doc position after the
    // previous query char's chosen position (`initialIndexes`).
    let mut idxs: Vec<usize> = Vec::with_capacity(q.len());
    let mut largest: i64 = -1;
    for &c in &q {
        let list = &posting[&c];
        let mut i = 0usize;
        while (list[i] as i64) <= largest {
            i += 1;
            if i >= list.len() {
                return None;
            }
        }
        largest = list[i] as i64;
        idxs.push(i);
    }

    let mut best_idxs = idxs.clone();
    let mut best_len: Option<usize> = None;
    loop {
        let start = posting[&q[0]][idxs[0]];
        let end = posting[&q[q.len() - 1]][*idxs.last().unwrap()];
        let len = end - start;
        if best_len.is_none_or(|b| len < b) {
            best_len = Some(len);
            best_idxs = idxs.clone();
        }
        if len == q.len() {
            // `bestIndexes` (`M`): stops as soon as this candidate's range length equals the
            // query length. That is one more than a truly compact span (which has length
            // `q.len() - 1`), so this can and does stop before finding the smallest possible
            // span; kept verbatim.
            break;
        }
        if !advance(&q, &posting, &mut idxs) {
            break;
        }
    }

    Some(best_idxs.iter().enumerate().map(|(qi, &li)| posting[&q[qi]][li]).collect())
}

/// Case-folds one char to one char, so a folded string is always the same length as the
/// original (`'\u{130}'.to_lowercase()` yields two chars; only the first is kept).
fn fold_char(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Advances the match to the next candidate: forces the first query char's index forward, then
/// as many subsequent ones as are needed to keep doc offsets strictly increasing
/// (`advanceIndexes`). Returns `false` when no further candidate exists.
fn advance(q: &[char], posting: &HashMap<char, Vec<usize>>, idxs: &mut [usize]) -> bool {
    let mut previous: i64 = -1;
    for i in 0..q.len() {
        let list = &posting[&q[i]];
        let mut current = idxs[i];
        let mut offset = list[current] as i64;
        if previous == -1 {
            previous = offset;
        }
        let original = current;
        while offset <= previous {
            current += 1;
            if current == list.len() {
                return false;
            }
            offset = list[current] as i64;
        }
        if current == original {
            break;
        }
        idxs[i] = current;
        previous = offset;
    }
    true
}

/// The quality of a match, given the matched indexes and the document's char length
/// (`qualityOfMatchWithMatcher:document:indexSet:`): 1.0 for an exact whole-document span, 0.9
/// for a match starting at index 0 that does not reach the end, else 0.5 divided by the number
/// of contiguous runs in `indexes` (each gap doubles as a penalty). Empty `indexes` is 0.0.
pub fn quality(indexes: &[usize], doc_len: usize) -> f64 {
    if indexes.is_empty() {
        return 0.0;
    }
    let first = indexes[0];
    let last = *indexes.last().unwrap();
    if first == 0 && doc_len > 0 && last == doc_len - 1 {
        1.0
    } else if first == 0 {
        0.9
    } else {
        let mut runs = 1usize;
        for w in indexes.windows(2) {
            if w[1] != w[0] + 1 {
                runs += 1;
            }
        }
        0.5 / runs as f64
    }
}

// -------------------------------------------------------------------------------------------
// Feature scoring (`scoreUsingMatcher:documents:multiplier:name:features:limit:`).
// -------------------------------------------------------------------------------------------

/// The score for `query` against `docs`, one call per feature. `docs` runs oldest to newest:
/// each document's quality is divided by its distance from the end (the newest is undiscounted).
/// An empty query trivially matches every non-empty document for a flat 0.01, with the first
/// non-empty document reported as the match (no highlighting). The total is multiplied by
/// `multiplier` and clamped to `limit`. Returns the score, and, when some document matched, its
/// index into `docs` and its matched char indexes (for building the display title or detail).
pub fn score_feature(query: &str, docs: &[String], multiplier: f64, limit: f64) -> (f64, Option<(usize, Vec<usize>)>) {
    let (_raw, clamped, best) = score_feature_raw(query, docs, multiplier, limit);
    (clamped, best)
}

/// `score_feature`'s body, also returning the unclamped running total: iTerm2's own
/// `scoreUsingMatcher:...` clamps only its `return` value (`MIN(limit, score)`); the entry it
/// stores into the shared `features` array for `detailForSession:features:` to compare keeps
/// the unclamped `score` (`M:1462-1467`). Title-building callers only ever want the clamped
/// value, so `score_feature` stays the public, two-value interface; `push_feature` (the one
/// caller that feeds `detailForSession`'s comparison) uses the raw value instead.
fn score_feature_raw(query: &str, docs: &[String], multiplier: f64, limit: f64) -> (f64, f64, Option<(usize, Vec<usize>)>) {
    if multiplier == 0.0 {
        return (0.0, 0.0, None);
    }
    if query.is_empty() {
        for (i, d) in docs.iter().enumerate() {
            if !d.is_empty() {
                return (0.01, 0.01, Some((i, Vec::new())));
            }
        }
        return (0.0, 0.0, None);
    }

    let mut score = 0.0;
    let mut highest = 0.0;
    let mut best: Option<(usize, Vec<usize>)> = None;
    let n_docs = docs.len();
    for (i, doc) in docs.iter().enumerate() {
        let n = (n_docs - i) as f64;
        let idxs = matched(query, doc);
        let q = idxs.as_ref().map_or(0.0, |v| quality(v, doc.chars().count()));
        let value = q / n;
        if value > highest {
            highest = value;
            best = idxs.map(|v| (i, v));
        }
        score += value * multiplier;
        if score > limit {
            break;
        }
    }
    (score, score.min(limit), best)
}

// -------------------------------------------------------------------------------------------
// Commands and tips (`iTermOpenQuicklyCommands.m`, `commandForQuery:`).
// -------------------------------------------------------------------------------------------

/// A `/x` command restricting the results shown, or `None` for an unrestricted query
/// (`iTermOpenQuicklyNoCommand`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    None,
    InTabsArrangement,
    Arrangement,
    Sessions,
    SwitchProfile,
    Windows,
    CreateTab,
    ColorPreset,
    Script,
    Action,
    Snippet,
    Bookmark,
}

impl Command {
    /// The command letter after the `/`, or `None` for the unrestricted command
    /// (`+[iTermOpenQuicklyCommand command]`).
    pub fn letter(self) -> Option<char> {
        match self {
            Command::None => None,
            Command::InTabsArrangement => Some('A'),
            Command::Arrangement => Some('a'),
            Command::Sessions => Some('f'),
            Command::SwitchProfile => Some('p'),
            Command::Windows => Some('w'),
            Command::CreateTab => Some('t'),
            Command::ColorPreset => Some('c'),
            Command::Script => Some('s'),
            Command::Action => Some(':'),
            Command::Snippet => Some('.'),
            Command::Bookmark => Some('b'),
        }
    }

    /// The tip's restriction description (`+[iTermOpenQuicklyCommand restrictionDescription]`).
    pub fn description(self) -> &'static str {
        match self {
            Command::None => "",
            Command::InTabsArrangement => "window arrangements that open in tabs",
            Command::Arrangement => "window arrangements",
            Command::Sessions => "existing sessions",
            Command::SwitchProfile => "switch profiles",
            Command::Windows => "existing windows",
            Command::CreateTab => "create tab",
            Command::ColorPreset => "load color preset",
            Command::Script => "run script",
            Command::Action => "perform action",
            Command::Snippet => "send snippet",
            Command::Bookmark => "open bookmark",
        }
    }
}

/// The tip order (`M:92-102`): `A a f p w t c s : . b`.
pub const TIP_ORDER: [Command; 11] = [
    Command::InTabsArrangement,
    Command::Arrangement,
    Command::Sessions,
    Command::SwitchProfile,
    Command::Windows,
    Command::CreateTab,
    Command::ColorPreset,
    Command::Script,
    Command::Action,
    Command::Snippet,
    Command::Bookmark,
];

/// Splits a query into its command and the remaining search text (`commandForQuery:`): a `/`
/// followed by more than one character reads the next character as a command letter
/// (case-sensitive) and trims the rest; an unrecognized letter, a bare `/`, or no leading `/`
/// falls back to the unrestricted command over the whole query text.
pub fn parse(query: &str) -> (Command, String) {
    let chars: Vec<char> = query.chars().collect();
    if chars.first() == Some(&'/') && chars.len() > 1 {
        let letter = chars[1];
        for cmd in TIP_ORDER {
            if cmd.letter() == Some(letter) {
                let rest: String = chars[2..].iter().collect();
                return (cmd, rest.trim().to_string());
            }
        }
    }
    (Command::None, query.to_string())
}

// -------------------------------------------------------------------------------------------
// Items (`iTermOpenQuicklyItem` subclasses and `updateWithQuery:`).
// -------------------------------------------------------------------------------------------

/// What an item activates (Task 8 owns activation; this just names the source record).
#[derive(Clone, Debug, PartialEq)]
pub enum ItemKind {
    Tip(char),
    Session(u64),
    Window(u64),
    NewTab(String),
    ChangeProfile(String),
    Arrangement { name: String, in_tabs: bool },
    Action(usize),
    Snippet(usize),
    NamedMark { pane: u64, mark: u32 },
    MenuItem(String),
    /// A script function invocation: the whole query (`OQM:699-716`).
    Invocation(String),
}

/// One ranked result: its score, its display title with the query-matched char indexes to bold,
/// and its detail line with the byte... char ranges within it to bold.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub kind: ItemKind,
    pub score: f64,
    pub title: String,
    pub title_bold: Vec<usize>,
    pub detail: String,
    pub detail_bold: Vec<Range<usize>>,
}

/// One session, as the app snapshots it for a query (`PTYSession` via `detailFunctionForSessions:`
/// and `scoreForSession:matcher:features:attributedName:`). `commands`, `dirs`, `hosts` and
/// `users` are scoring documents (oldest first); `last_command`, `cwd` and `host` are the
/// session's current values, used only for the query-independent detail refinement.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionSource {
    pub pane: u64,
    pub current: bool,
    pub ordinal: u64,
    pub name: String,
    pub tab_title: Option<String>,
    pub badge: String,
    pub commands: Vec<String>,
    pub dirs: Vec<String>,
    pub hosts: Vec<String>,
    pub users: Vec<String>,
    pub profile: String,
    pub git_branch: String,
    pub user_vars: Vec<(String, String)>,
    pub pid: String,
    pub cwd: String,
    pub last_command: String,
    pub host: String,
}

/// A snapshot of everything Open Quickly can search, built by the app once per keystroke.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sources {
    pub sessions: Vec<SessionSource>,
    /// (window id, title, pre-built feature strings to join with an em dash).
    pub windows: Vec<(u64, String, Vec<String>)>,
    /// (name, guid, prevents tabs).
    pub profiles: Vec<(String, String, bool)>,
    pub default_profile: String,
    /// Whether the current session's profile can be switched to another profile at all
    /// (iTerm2 filters candidates by matching `profileType`; Tandem's profiles carry no type
    /// yet, so the app collapses that filter to this one gate).
    pub current_profile_type_ok: bool,
    pub has_window: bool,
    pub arrangements: Vec<String>,
    pub default_arrangement: Option<String>,
    /// (title, display string).
    pub actions: Vec<(String, String)>,
    /// (title, trimmed value, display title).
    pub snippets: Vec<(String, String, String)>,
    /// (pane, mark, name, in the active session).
    pub named_marks: Vec<(u64, u32, String, bool)>,
    /// (id, title, menu path, alternate).
    pub menu_items: Vec<(String, String, Vec<String>, bool)>,
    /// The whole query when it parses as a call or a call sequence (`OQM:950-1003`).
    pub invocation: Option<String>,
}

/// The most results kept, highest score first (`kMaxItems`).
pub const MAX_ITEMS: usize = 100;

/// The search field's placeholder (the xib).
pub const PLACEHOLDER: &str = "Search for sessions, profiles, or arrangements";
pub const NEW_TAB_DETAIL: &str = "Create a new tab with this profile";
pub const NEW_WINDOW_DETAIL: &str = "Create a new window with this profile";
pub const CHANGE_PROFILE_DETAIL: &str = "Change current session\u{2019}s profile";
pub const RESTORE: &str = "Restore window arrangement";
pub const RESTORE_IN_TABS: &str = "Restore window arrangement in tabs";
/// `{}` is the snippet's display title.
pub const SNIPPET_DETAIL: &str = "Send snippet \u{201c}{}\u{201d}. Press \u{2325} to edit first.";
/// `{}` is the named mark's name.
pub const NAMED_MARK_DETAIL: &str = "Named mark \u{201c}{}\u{201d}";
/// `{}` is the menu path, joined with " > ".
pub const MENU_DETAIL: &str = "Menu item under {}";
/// `{}` is the menu path, joined with " > ".
pub const ALT_MENU_DETAIL: &str = "Alternate menu item under {}";
/// `{}` is the preset name. The quotes are reversed in iTerm2's own string
/// (`@"Load color preset \u{201d}%@\u{201c}"`); kept verbatim, though nothing produces this item
/// yet (color presets are milestone 13; the source is empty).
pub const COLOR_PRESET_DETAIL: &str = "Load color preset \u{201d}{}\u{201c}";
pub const UNTITLED: &str = "Untitled";
/// An invocation item's detail (`OQM:705`).
pub const INVOKE_DETAIL: &str = "Invoke";
/// An invocation's multiplier (`kInvocationMultiplier`); its limit is twice that.
pub const INVOCATION_MULTIPLIER: f64 = 5.0;

fn fill(template: &str, value: &str) -> String {
    template.replacen("{}", value, 1)
}

/// Like `fill`, also returning the char range `value` landed at, for the one detail that bolds
/// part of itself: the menu path (`M:393-405`: `itemForMenuItem:matcher:path:` bolds just the
/// `combinedPath` run, unlike every other detail string, which is built through the delegate's
/// generic, unhighlighted `openQuicklyModelAttributedStringForDetail:featureName:`).
fn fill_bold(template: &str, value: &str) -> (String, Range<usize>) {
    let at = template.find("{}").expect("template has a {} placeholder");
    let start = template[..at].chars().count();
    let end = start + value.chars().count();
    (fill(template, value), start..end)
}

const SESSION_BADGE_MULT: f64 = 3.0;
const SESSION_NAME_MULT: f64 = 2.0;
const GIT_BRANCH_MULT: f64 = 1.5;
const HOSTNAME_MULT: f64 = 1.2;
const PROFILE_NAME_MULT: f64 = 1.01;
const USER_VAR_MULT: f64 = 1.0;
const DIRECTORY_MULT: f64 = 0.9;
const COMMAND_MULT: f64 = 0.8;
const USERNAME_MULT: f64 = 0.5;
const OTHER_VAR_MULT: f64 = 0.4;
const ACTION_MULT: f64 = 0.4;
const SNIPPET_MULT: f64 = 0.3;
const NAMED_MARK_MULT: f64 = 0.5;
const ARRANGEMENT_MULT: f64 = 0.11;
const PROFILE_ITEM_MULT: f64 = 0.1;
const MENU_ITEM_MULT: f64 = 0.096;
const WINDOW_MULT: f64 = 0.08;

/// What a command restricts the search to (`supportsSessionLocation` and friends). Color
/// presets, scripts and bookmarks have no Tandem source (milestones 13, 11, and never), so
/// their commands support nothing here; their tips still show.
struct Supports {
    session_location: bool,
    window_location: bool,
    create_new_tab: bool,
    change_profile: bool,
    /// `Some(tabs_only)` when arrangements are searched.
    open_arrangement: Option<bool>,
    action: bool,
    snippet: bool,
    named_marks: bool,
    menu_items: bool,
}

const NOTHING: Supports = Supports {
    session_location: false,
    window_location: false,
    create_new_tab: false,
    change_profile: false,
    open_arrangement: None,
    action: false,
    snippet: false,
    named_marks: false,
    menu_items: false,
};

fn supports(cmd: Command) -> Supports {
    match cmd {
        Command::None => Supports {
            session_location: true,
            window_location: true,
            create_new_tab: true,
            change_profile: true,
            open_arrangement: Some(false),
            action: true,
            snippet: true,
            named_marks: true,
            menu_items: true,
        },
        Command::InTabsArrangement => Supports { open_arrangement: Some(true), ..NOTHING },
        Command::Arrangement => Supports { open_arrangement: Some(false), ..NOTHING },
        Command::Sessions => Supports { session_location: true, ..NOTHING },
        Command::SwitchProfile => Supports { change_profile: true, ..NOTHING },
        Command::Windows => Supports { window_location: true, ..NOTHING },
        Command::CreateTab => Supports { create_new_tab: true, ..NOTHING },
        Command::ColorPreset | Command::Script | Command::Bookmark => NOTHING,
        Command::Action => Supports { action: true, ..NOTHING },
        Command::Snippet => Supports { snippet: true, ..NOTHING },
    }
}

/// The title (or plain document) and its bold indexes for a `score_feature` result over `docs`:
/// the matched document when one matched, else the first document plain (iTerm2 falls back to
/// the document text itself, unhighlighted, when the name itself did not match but another
/// feature carried the item's score).
fn title_from(docs: &[String], result: Option<(usize, Vec<usize>)>) -> (String, Vec<usize>) {
    match result {
        Some((i, idx)) => (docs[i].clone(), idx),
        None => (docs.first().cloned().unwrap_or_default(), Vec::new()),
    }
}

fn untitled_if_empty(title: String) -> String {
    if title.is_empty() {
        UNTITLED.to_string()
    } else {
        title
    }
}

fn add_tips(items: &mut Vec<Item>) {
    for cmd in TIP_ORDER {
        let letter = cmd.letter().expect("tip commands always have a letter");
        items.push(Item {
            kind: ItemKind::Tip(letter),
            score: 0.0,
            title: format!("Tip: Start your query with \u{201c}/{letter}\u{201d}"),
            title_bold: Vec::new(),
            detail: format!("Restricts results to {}", cmd.description()),
            detail_bold: Vec::new(),
        });
    }
}

/// A session's title document (`documentForSession:`): the tab title alone when it contains the
/// session name, `"tab \u{2014} name"` when it does not, else the name alone.
fn session_document(s: &SessionSource) -> String {
    let tab_title = s.tab_title.as_deref().unwrap_or("");
    if tab_title.is_empty() {
        return s.name.clone();
    }
    if tab_title.contains(s.name.as_str()) {
        tab_title.to_string()
    } else {
        format!("{tab_title} \u{2014} {}", s.name)
    }
}

/// One scored, non-title feature of a session, for picking the detail line
/// (`detailForSession:features:`).
struct Feature {
    name: String,
    score: f64,
    value: String,
    bold: Vec<usize>,
}

fn push_feature(features: &mut Vec<Feature>, name: &str, docs: &[String], mult: f64, cap: f64, query: &str, total: &mut f64) {
    let (raw, clamped, mut result) = score_feature_raw(query, docs, mult, cap);
    *total += clamped;
    if query.is_empty() {
        // An empty query trivially matches every non-empty document (`M:1416-1434`): iTerm2
        // adds one `features` entry per non-empty document, all tied at the same score, and
        // `detailForSession:features:` keeps the last tied entry when picking the winner. For a
        // multi-document feature (commands, directories, hosts, users) that is the newest
        // non-empty document, not the oldest.
        if let Some(i) = docs.iter().rposition(|d| !d.is_empty()) {
            result = Some((i, Vec::new()));
        }
    }
    if let Some((i, idx)) = result {
        features.push(Feature { name: name.to_string(), score: raw, value: docs[i].clone(), bold: idx });
    }
}

/// The four refinement candidates (`detailFunctionForSessions:`'s `pwd`, `command`, `hostname`,
/// `badge`), ranked by how many distinct non-empty values they take across all sessions
/// (highest first; ties keep the later candidate first, matching the source's ascending sort
/// followed by a full reversal).
fn refinement_order(sessions: &[SessionSource]) -> Vec<(&'static str, fn(&SessionSource) -> &str)> {
    let candidates: [(&'static str, fn(&SessionSource) -> &str); 4] = [
        ("Directory", |s: &SessionSource| s.cwd.as_str()),
        ("Command", |s: &SessionSource| s.last_command.as_str()),
        ("Host", |s: &SessionSource| s.host.as_str()),
        ("Badge", |s: &SessionSource| s.badge.as_str()),
    ];
    let mut ranked: Vec<(&'static str, fn(&SessionSource) -> &str, usize)> = candidates
        .iter()
        .map(|&(name, f)| {
            let variance: HashSet<&str> = sessions.iter().map(f).filter(|v| !v.is_empty()).collect();
            (name, f, variance.len())
        })
        .collect();
    ranked.sort_by_key(|&(_, _, variance)| variance);
    ranked.reverse();
    ranked.into_iter().map(|(name, f, _)| (name, f)).collect()
}

/// The refinement for one session: the first candidate (in `order`) that has a value for it.
fn refinement_for(sess: &SessionSource, order: &[(&'static str, fn(&SessionSource) -> &str)]) -> Option<(&'static str, String)> {
    for &(name, f) in order {
        let v = f(sess);
        if !v.is_empty() {
            return Some((name, v.to_string()));
        }
    }
    None
}

/// Combines the best-scoring feature (why the session matched) with the query-independent
/// refinement (what distinguishes it from other sessions): joined with an em dash when their
/// feature names differ, else the refinement alone (`addSessionLocationToItems:withMatcher:`).
fn combine_detail(best: Option<&Feature>, refinement: Option<&(&'static str, String)>) -> (String, Vec<Range<usize>>) {
    let detail_part = best.map(|f| {
        let prefix_len = f.name.chars().count() + 2; // "Name: "
        let bold: Vec<Range<usize>> = f.bold.iter().map(|&i| (prefix_len + i)..(prefix_len + i + 1)).collect();
        (format!("{}: {}", f.name, f.value), bold)
    });
    let refinement_text = refinement.map(|(name, value)| format!("{name}: {value}"));

    match (detail_part, refinement_text) {
        (Some((detail_str, detail_bold)), Some(ref_str)) => {
            let same_feature = best.map(|f| f.name.as_str()) == refinement.map(|(n, _)| *n);
            if same_feature {
                (ref_str, Vec::new())
            } else {
                (format!("{detail_str} \u{2014} {ref_str}"), detail_bold)
            }
        }
        (Some((detail_str, detail_bold)), None) => (detail_str, detail_bold),
        (None, Some(ref_str)) => (ref_str, Vec::new()),
        (None, None) => (String::new(), Vec::new()),
    }
}

fn add_sessions(items: &mut Vec<Item>, query: &str, s: &Sources) {
    let max_ordinal = s.sessions.iter().filter(|sess| !sess.current).map(|sess| sess.ordinal).max().unwrap_or(0);
    let order = refinement_order(&s.sessions);
    let cap = 2.0 + (query.chars().count() / 4) as f64;

    for sess in &s.sessions {
        if sess.current {
            continue;
        }
        let docs = vec![session_document(sess)];
        let (name_score, name_result) = score_feature(query, &docs, SESSION_NAME_MULT, cap);
        let mut score = name_score;
        let (title, title_bold) = title_from(&docs, name_result);

        let mut features: Vec<Feature> = Vec::new();
        push_feature(&mut features, "Badge", &[sess.badge.clone()], SESSION_BADGE_MULT, cap, query, &mut score);
        push_feature(&mut features, "Command", &sess.commands, COMMAND_MULT, cap, query, &mut score);
        push_feature(&mut features, "Directory", &sess.dirs, DIRECTORY_MULT, cap, query, &mut score);
        push_feature(&mut features, "Host", &sess.hosts, HOSTNAME_MULT, cap, query, &mut score);
        push_feature(&mut features, "User", &sess.users, USERNAME_MULT, cap, query, &mut score);
        push_feature(&mut features, "Profile", &[sess.profile.clone()], PROFILE_NAME_MULT, cap, query, &mut score);
        push_feature(&mut features, "Git Branch", &[sess.git_branch.clone()], GIT_BRANCH_MULT, cap, query, &mut score);
        for (name, value) in &sess.user_vars {
            push_feature(&mut features, name, &[value.clone()], USER_VAR_MULT, cap, query, &mut score);
        }
        push_feature(&mut features, "pid", &[sess.pid.clone()], OTHER_VAR_MULT, cap, query, &mut score);

        if score <= 0.0 {
            continue;
        }

        if max_ordinal > 0 {
            score += 0.0099 * (sess.ordinal as f64 / max_ordinal as f64);
        }

        let best = features.iter().fold(None::<&Feature>, |acc, f| match acc {
            None => Some(f),
            Some(cur) if f.score >= cur.score => Some(f),
            _ => acc,
        });
        let refinement = refinement_for(sess, &order);
        let (detail, detail_bold) = combine_detail(best, refinement.as_ref());

        items.push(Item {
            kind: ItemKind::Session(sess.pane),
            score,
            title: untitled_if_empty(title),
            title_bold,
            detail,
            detail_bold,
        });
    }
}

fn add_windows(items: &mut Vec<Item>, query: &str, s: &Sources) {
    let cap = 2.0 * WINDOW_MULT;
    for (id, title, features) in &s.windows {
        let docs = vec![title.clone()];
        let (score, result) = score_feature(query, &docs, WINDOW_MULT, cap);
        if score <= 0.0 {
            continue;
        }
        let (t, bold) = title_from(&docs, result);
        let detail = features.join(" \u{2014} ");
        items.push(Item { kind: ItemKind::Window(*id), score, title: untitled_if_empty(t), title_bold: bold, detail, detail_bold: Vec::new() });
    }
}

fn add_new_tabs(items: &mut Vec<Item>, query: &str, s: &Sources) {
    let cap = 2.0 * PROFILE_ITEM_MULT;
    for (name, guid, prevents_tab) in &s.profiles {
        let docs = vec![name.clone()];
        let (mut score, result) = score_feature(query, &docs, PROFILE_ITEM_MULT, cap);
        if score <= 0.0 {
            continue;
        }
        if !query.is_empty() && s.default_profile == *guid {
            score += 0.2;
        }
        let (title, bold) = title_from(&docs, result);
        let detail = if !s.has_window || *prevents_tab { NEW_WINDOW_DETAIL } else { NEW_TAB_DETAIL };
        items.push(Item {
            kind: ItemKind::NewTab(guid.clone()),
            score,
            title: untitled_if_empty(title),
            title_bold: bold,
            detail: detail.to_string(),
            detail_bold: Vec::new(),
        });
    }
}

fn add_change_profile(items: &mut Vec<Item>, query: &str, s: &Sources) {
    let cap = 2.0 * PROFILE_ITEM_MULT;
    for (name, guid, _prevents_tab) in &s.profiles {
        let docs = vec![name.clone()];
        let (mut score, result) = score_feature(query, &docs, PROFILE_ITEM_MULT, cap);
        if score <= 0.0 {
            continue;
        }
        if !query.is_empty() && s.default_profile == *guid {
            score += 0.2;
        }
        let (title, bold) = title_from(&docs, result);
        items.push(Item {
            kind: ItemKind::ChangeProfile(guid.clone()),
            score,
            title: untitled_if_empty(title),
            title_bold: bold,
            detail: CHANGE_PROFILE_DETAIL.to_string(),
            detail_bold: Vec::new(),
        });
    }
}

fn add_arrangements(items: &mut Vec<Item>, query: &str, s: &Sources, tabs_only: bool) {
    let cap = 2.0 * ARRANGEMENT_MULT;
    for name in &s.arrangements {
        let docs = vec![name.clone()];
        let (mut score, result) = score_feature(query, &docs, ARRANGEMENT_MULT, cap);
        if score <= 0.0 {
            continue;
        }
        if !query.is_empty() && s.default_arrangement.as_deref() == Some(name.as_str()) {
            score += 0.2;
        }
        let (title, bold) = title_from(&docs, result);
        let title = untitled_if_empty(title);
        if !tabs_only {
            items.push(Item {
                kind: ItemKind::Arrangement { name: name.clone(), in_tabs: false },
                score,
                title: title.clone(),
                title_bold: bold.clone(),
                detail: RESTORE.to_string(),
                detail_bold: Vec::new(),
            });
        }
        items.push(Item {
            kind: ItemKind::Arrangement { name: name.clone(), in_tabs: true },
            score,
            title,
            title_bold: bold,
            detail: RESTORE_IN_TABS.to_string(),
            detail_bold: Vec::new(),
        });
    }
}

fn add_actions(items: &mut Vec<Item>, query: &str, s: &Sources) {
    let cap = 2.0 * ACTION_MULT;
    for (i, (title, display)) in s.actions.iter().enumerate() {
        let docs = vec![title.clone()];
        let (score, result) = score_feature(query, &docs, ACTION_MULT, cap);
        if score <= 0.0 {
            continue;
        }
        let (t, bold) = title_from(&docs, result);
        items.push(Item {
            kind: ItemKind::Action(i),
            score,
            title: untitled_if_empty(t),
            title_bold: bold,
            detail: display.clone(),
            detail_bold: Vec::new(),
        });
    }
}

fn add_snippets(items: &mut Vec<Item>, query: &str, s: &Sources) {
    let cap = 2.0 * SNIPPET_MULT;
    for (i, (title, value, display_title)) in s.snippets.iter().enumerate() {
        let docs = vec![title.clone(), value.clone()];
        let (score, result) = score_feature(query, &docs, SNIPPET_MULT, cap);
        if score <= 0.0 {
            continue;
        }
        let (t, bold) = title_from(&docs, result);
        items.push(Item {
            kind: ItemKind::Snippet(i),
            score,
            title: untitled_if_empty(t),
            title_bold: bold,
            detail: fill(SNIPPET_DETAIL, display_title),
            detail_bold: Vec::new(),
        });
    }
}

fn add_named_marks(items: &mut Vec<Item>, query: &str, s: &Sources) {
    for (pane, mark, name, active) in &s.named_marks {
        if name.is_empty() {
            continue;
        }
        let mult = NAMED_MARK_MULT * if *active { 1.5 } else { 1.0 };
        let docs = vec![name.clone()];
        let (score, result) = score_feature(query, &docs, mult, mult);
        if score <= 0.0 {
            continue;
        }
        let (t, bold) = title_from(&docs, result);
        items.push(Item {
            kind: ItemKind::NamedMark { pane: *pane, mark: *mark },
            score,
            title: untitled_if_empty(t),
            title_bold: bold,
            detail: fill(NAMED_MARK_DETAIL, name),
            detail_bold: Vec::new(),
        });
    }
}

fn add_menu_items(items: &mut Vec<Item>, query: &str, s: &Sources) {
    for (id, title, path, alternate) in &s.menu_items {
        let docs = vec![title.clone()];
        let (score, result) = score_feature(query, &docs, MENU_ITEM_MULT, MENU_ITEM_MULT);
        if score <= 0.0 {
            continue;
        }
        let (t, bold) = title_from(&docs, result);
        let joined = path.join(" > ");
        let (detail, path_range) = fill_bold(if *alternate { ALT_MENU_DETAIL } else { MENU_DETAIL }, &joined);
        items.push(Item {
            kind: ItemKind::MenuItem(id.clone()),
            score,
            title: untitled_if_empty(t),
            title_bold: bold,
            detail,
            detail_bold: vec![path_range],
        });
    }
}

/// The invocation item (`addInvocation:scope:toItems:withMatcher:`, `OQM:699-716`), for every
/// command and whatever its score.
fn add_invocation(items: &mut Vec<Item>, query: &str, s: &Sources) {
    let Some(invocation) = &s.invocation else { return };
    let docs = vec![invocation.clone()];
    let (score, result) = score_feature(query, &docs, INVOCATION_MULTIPLIER, 2.0 * INVOCATION_MULTIPLIER);
    // The title is the matched document only (`OQM:1051-1064`).
    let (title, title_bold) = result.map(|(i, idx)| (docs[i].clone(), idx)).unwrap_or_default();
    items.push(Item { kind: ItemKind::Invocation(invocation.clone()), score, title, title_bold, detail: INVOKE_DETAIL.to_string(), detail_bold: Vec::new() });
}

/// The ranked results for `query` over `s` (`updateWithQuery:`): tips only for a bare `/`;
/// otherwise the sources the parsed command restricts to, each item kept only when its score is
/// positive; sorted highest score first and capped at `MAX_ITEMS`.
pub fn items(query: &str, s: &Sources) -> Vec<Item> {
    let (cmd, text) = parse(query);
    let sup = supports(cmd);
    let mut items = Vec::new();

    if query == "/" {
        add_tips(&mut items);
    }
    if sup.session_location {
        add_sessions(&mut items, &text, s);
    }
    if sup.window_location {
        add_windows(&mut items, &text, s);
    }
    if sup.create_new_tab {
        add_new_tabs(&mut items, &text, s);
    }
    if sup.change_profile && s.has_window && s.current_profile_type_ok {
        add_change_profile(&mut items, &text, s);
    }
    if let Some(tabs_only) = sup.open_arrangement {
        add_arrangements(&mut items, &text, s, tabs_only);
    }
    if sup.action && s.has_window {
        add_actions(&mut items, &text, s);
    }
    if sup.snippet && s.has_window {
        add_snippets(&mut items, &text, s);
    }
    if sup.named_marks {
        add_named_marks(&mut items, &text, s);
    }
    if sup.menu_items {
        add_menu_items(&mut items, &text, s);
    }
    add_invocation(&mut items, &text, s);

    items.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    items.truncate(MAX_ITEMS);
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores_follow_iterm2() {
        assert_eq!(matched("abc", "axbxc"), Some(vec![0, 2, 4]));
        assert!((quality(&[0, 2, 4], 5) - 1.0).abs() < 1e-9);

        let idx = matched("ab", "abx").unwrap();
        assert_eq!(idx, vec![0, 1]);
        assert!((quality(&idx, 3) - 0.9).abs() < 1e-9);

        let idx = matched("bc", "abc").unwrap();
        assert_eq!(idx, vec![1, 2]);
        assert!((quality(&idx, 3) - 0.5).abs() < 1e-9);

        let idx = matched("ac", "xaxc").unwrap();
        assert_eq!(idx, vec![1, 3]);
        assert!((quality(&idx, 4) - 0.25).abs() < 1e-9);

        // Smallest span wins.
        assert_eq!(matched("ab", "a_ab"), Some(vec![2, 3]));

        // A missing letter is None.
        assert_eq!(matched("abz", "abc"), None);

        // Case-insensitive.
        assert_eq!(matched("ABC", "axbxc"), Some(vec![0, 2, 4]));

        // score_feature: older document divided by 2, newer by 1, capped at the limit.
        let docs = vec!["abc".to_string(), "abc".to_string()];
        let (score, best) = score_feature("abc", &docs, 1.0, 10.0);
        assert!((score - 1.5).abs() < 1e-9, "{score}");
        assert_eq!(best, Some((1, vec![0, 1, 2])));
        let (capped, _) = score_feature("abc", &docs, 1.0, 1.0);
        assert!((capped - 1.0).abs() < 1e-9);

        // Empty query: 0.01 when any document is non-empty, the first non-empty reported.
        let (score, best) = score_feature("", &["".to_string(), "x".to_string()], 1.0, 10.0);
        assert!((score - 0.01).abs() < 1e-9);
        assert_eq!(best, Some((1, Vec::new())));
        let (score, best) = score_feature("", &["".to_string()], 1.0, 10.0);
        assert_eq!(score, 0.0);
        assert_eq!(best, None);

        // A session's cap is 2 + query length / 4 (integer), per feature.
        let query = "abcd"; // len 4, cap = 2 + 4/4 = 3.0
        let cap_expected = 3.0;
        let sess = SessionSource { pane: 1, name: "z".to_string(), commands: vec!["abcd".to_string(); 30], ..Default::default() };
        let result = items(query, &Sources { sessions: vec![sess], ..Default::default() });
        let item = result.iter().find(|it| it.kind == ItemKind::Session(1)).unwrap();
        assert!((item.score - cap_expected).abs() < 1e-6, "{}", item.score);

        // A named mark's cap is its multiplier: 0.5, or 0.75 in the active session.
        let sources = Sources {
            named_marks: vec![(1, 1, "abc".to_string(), false), (2, 2, "abc".to_string(), true)],
            ..Default::default()
        };
        let result = items("abc", &sources);
        let inactive = result.iter().find(|it| it.kind == ItemKind::NamedMark { pane: 1, mark: 1 }).unwrap();
        let active = result.iter().find(|it| it.kind == ItemKind::NamedMark { pane: 2, mark: 2 }).unwrap();
        assert!((inactive.score - 0.5).abs() < 1e-9);
        assert!((active.score - 0.75).abs() < 1e-9);

        // +0.2 to the default profile and default arrangement, only for a non-empty query.
        let sources = Sources {
            profiles: vec![("bash".to_string(), "guid1".to_string(), false)],
            default_profile: "guid1".to_string(),
            has_window: true,
            ..Default::default()
        };
        let bash = items("bash", &sources);
        let item = bash.iter().find(|it| it.kind == ItemKind::NewTab("guid1".to_string())).unwrap();
        assert!((item.score - 0.3).abs() < 1e-9, "{}", item.score); // 0.1 matched + 0.2 default
        let empty = items("", &sources);
        let item = empty.iter().find(|it| it.kind == ItemKind::NewTab("guid1".to_string())).unwrap();
        assert!((item.score - 0.01).abs() < 1e-9, "{}", item.score);

        let sources = Sources { arrangements: vec!["work".to_string()], default_arrangement: Some("work".to_string()), ..Default::default() };
        let work = items("work", &sources);
        let item = work.iter().find(|it| it.kind == ItemKind::Arrangement { name: "work".to_string(), in_tabs: false }).unwrap();
        assert!((item.score - 0.31).abs() < 1e-9, "{}", item.score); // 0.11 matched + 0.2 default

        // Recency adds 0.0099 * ordinal / max only to a positive score.
        let sources = Sources {
            sessions: vec![
                SessionSource { pane: 1, name: "abc".to_string(), ordinal: 5, ..Default::default() },
                SessionSource { pane: 2, name: "abc".to_string(), ordinal: 10, ..Default::default() },
                SessionSource { pane: 3, name: "xyz".to_string(), ordinal: 7, ..Default::default() },
            ],
            ..Default::default()
        };
        let result = items("abc", &sources);
        assert!(result.iter().all(|it| it.kind != ItemKind::Session(3)), "no match, so no recency-driven inclusion");
        let s1 = result.iter().find(|it| it.kind == ItemKind::Session(1)).unwrap();
        let s2 = result.iter().find(|it| it.kind == ItemKind::Session(2)).unwrap();
        let base = 2.0; // exact match, capped at 2 + 3/4 = 2.0
        assert!((s1.score - (base + 0.0099 * 5.0 / 10.0)).abs() < 1e-9, "{}", s1.score);
        assert!((s2.score - (base + 0.0099 * 10.0 / 10.0)).abs() < 1e-9, "{}", s2.score);

        // The current session is never an item.
        let sources = Sources {
            sessions: vec![
                SessionSource { pane: 1, name: "abc".to_string(), current: true, ..Default::default() },
                SessionSource { pane: 2, name: "abc".to_string(), ..Default::default() },
            ],
            ..Default::default()
        };
        let result = items("abc", &sources);
        assert!(result.iter().all(|it| it.kind != ItemKind::Session(1)));
        assert!(result.iter().any(|it| it.kind == ItemKind::Session(2)));

        // Sorted by score, at most 100.
        let sessions: Vec<SessionSource> = (0..150u64).map(|i| SessionSource { pane: i, name: "abc".to_string(), ordinal: i, ..Default::default() }).collect();
        let result = items("abc", &Sources { sessions, ..Default::default() });
        assert_eq!(result.len(), 100);
        for w in result.windows(2) {
            assert!(w[0].score >= w[1].score);
        }
        assert_eq!(result[0].kind, ItemKind::Session(149));
    }

    #[test]
    fn commands_and_tips_follow_iterm2() {
        assert_eq!(parse("/p  dev "), (Command::SwitchProfile, "dev".to_string()));
        assert_eq!(parse("/x y"), (Command::None, "/x y".to_string()));
        assert_eq!(parse("/"), (Command::None, "/".to_string()));
        assert_eq!(parse("/P"), (Command::None, "/P".to_string()));

        let tips = items("/", &Sources::default());
        let letters: Vec<char> = tips.iter().filter_map(|it| if let ItemKind::Tip(c) = it.kind { Some(c) } else { None }).collect();
        assert_eq!(letters, vec!['A', 'a', 'f', 'p', 'w', 't', 'c', 's', ':', '.', 'b']);
        let a_tip = tips.iter().find(|it| it.kind == ItemKind::Tip('A')).unwrap();
        assert_eq!(a_tip.title, "Tip: Start your query with \u{201c}/A\u{201d}");
        assert_eq!(a_tip.detail, "Restricts results to window arrangements that open in tabs");
        assert_eq!(a_tip.score, 0.0);

        // Tips sort after any item matching "/".
        let sources = Sources { sessions: vec![SessionSource { pane: 1, name: "a/b".to_string(), ..Default::default() }], ..Default::default() };
        let result = items("/", &sources);
        let session_pos = result.iter().position(|it| it.kind == ItemKind::Session(1)).unwrap();
        let first_tip_pos = result.iter().position(|it| matches!(it.kind, ItemKind::Tip(_))).unwrap();
        assert!(session_pos < first_tip_pos);

        // /A gives only in-tabs arrangements; /a gives both.
        let sources = Sources { arrangements: vec!["work".to_string()], ..Default::default() };
        let in_tabs_only = items("/A", &sources);
        assert_eq!(in_tabs_only.len(), 1);
        assert!(matches!(&in_tabs_only[0].kind, ItemKind::Arrangement { in_tabs: true, .. }));
        let both = items("/a", &sources);
        assert_eq!(both.len(), 2);

        // /t gives only new-tab items.
        let sources = Sources {
            profiles: vec![("bash".to_string(), "g1".to_string(), false)],
            sessions: vec![SessionSource { pane: 1, name: "bash".to_string(), ..Default::default() }],
            ..Default::default()
        };
        let result = items("/t", &sources);
        assert_eq!(result.len(), 1);
        assert!(matches!(&result[0].kind, ItemKind::NewTab(_)));
    }

    #[test]
    fn open_quickly_invocation_follows_iterm2() {
        let with = Sources { invocation: Some("f(x: 1)".to_string()), ..Default::default() };
        let got: Vec<Item> = items("f(x: 1)", &with).into_iter().filter(|i| matches!(i.kind, ItemKind::Invocation(_))).collect();
        assert_eq!(got.len(), 1);
        let (score, _) = score_feature("f(x: 1)", &["f(x: 1)".to_string()], INVOCATION_MULTIPLIER, 10.0);
        assert_eq!(got[0].kind, ItemKind::Invocation("f(x: 1)".to_string()));
        assert_eq!((got[0].detail.as_str(), got[0].detail_bold.is_empty()), (INVOKE_DETAIL, true));
        assert!((got[0].score - score).abs() < 1e-9 && score > 0.0, "{} {score}", got[0].score);
        assert_eq!((got[0].title.as_str(), got[0].title_bold.clone()), ("f(x: 1)", (0..7).collect::<Vec<usize>>()));
        assert_eq!((INVOKE_DETAIL, INVOCATION_MULTIPLIER), ("Invoke", 5.0));
        // None: no item.
        assert!(!items("f(x: 1)", &Sources::default()).iter().any(|i| matches!(i.kind, ItemKind::Invocation(_))));
        // A restricting command keeps it; its search text is scored.
        let got: Vec<Item> = items("/p fx", &with).into_iter().filter(|i| matches!(i.kind, ItemKind::Invocation(_))).collect();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].title_bold, vec![0, 2]);
        // Added even when the text does not match: no title, score 0.
        let got: Vec<Item> = items("zzz", &with).into_iter().filter(|i| matches!(i.kind, ItemKind::Invocation(_))).collect();
        assert_eq!((got.len(), got[0].score, got[0].title.as_str()), (1, 0.0, ""));
    }

    #[test]
    fn items_follow_iterm2() {
        // Title: the tab title alone when it contains the name, "tab \u{2014} name" otherwise.
        let sources = Sources {
            sessions: vec![
                SessionSource { pane: 1, name: "main".to_string(), tab_title: Some("build".to_string()), ..Default::default() },
                SessionSource { pane: 2, name: "main".to_string(), tab_title: Some("main output".to_string()), ..Default::default() },
            ],
            ..Default::default()
        };
        let result = items("", &sources);
        assert_eq!(result.iter().find(|it| it.kind == ItemKind::Session(1)).unwrap().title, "build \u{2014} main");
        assert_eq!(result.iter().find(|it| it.kind == ItemKind::Session(2)).unwrap().title, "main output");

        // Detail: the best-scoring feature, then the query-independent refinement; the
        // refinement alone when its feature equals the best-scoring one.
        let sessions = vec![
            SessionSource {
                pane: 1,
                name: "s1".to_string(),
                commands: vec!["make".to_string()],
                last_command: "make".to_string(),
                dirs: vec!["C:\\src".to_string()],
                cwd: "C:\\src".to_string(),
                ..Default::default()
            },
            SessionSource { pane: 2, name: "s2".to_string(), dirs: vec!["C:\\other".to_string()], cwd: "C:\\other".to_string(), ..Default::default() },
        ];
        let sources = Sources { sessions, ..Default::default() };
        let result = items("make", &sources);
        let item = result.iter().find(|it| it.kind == ItemKind::Session(1)).unwrap();
        assert_eq!(item.detail, "Command: make \u{2014} Directory: C:\\src");

        let result = items("src", &sources);
        let item = result.iter().find(|it| it.kind == ItemKind::Session(1)).unwrap();
        assert_eq!(item.detail, "Directory: C:\\src");

        // New-tab detail: a tab with a window and a tab-capable profile, else a window.
        let sources = Sources { profiles: vec![("bash".to_string(), "g1".to_string(), false)], has_window: true, ..Default::default() };
        let item = items("bash", &sources).into_iter().find(|it| it.kind == ItemKind::NewTab("g1".to_string())).unwrap();
        assert_eq!(item.detail, NEW_TAB_DETAIL);

        let sources = Sources { profiles: vec![("bash".to_string(), "g1".to_string(), false)], has_window: false, ..Default::default() };
        let item = items("bash", &sources).into_iter().find(|it| it.kind == ItemKind::NewTab("g1".to_string())).unwrap();
        assert_eq!(item.detail, NEW_WINDOW_DETAIL);

        let sources = Sources { profiles: vec![("bash".to_string(), "g1".to_string(), true)], has_window: true, ..Default::default() };
        let item = items("bash", &sources).into_iter().find(|it| it.kind == ItemKind::NewTab("g1".to_string())).unwrap();
        assert_eq!(item.detail, NEW_WINDOW_DETAIL);

        // Change profile.
        let sources = Sources {
            profiles: vec![("bash".to_string(), "g1".to_string(), false)],
            has_window: true,
            current_profile_type_ok: true,
            ..Default::default()
        };
        let item = items("bash", &sources).into_iter().find(|it| it.kind == ItemKind::ChangeProfile("g1".to_string())).unwrap();
        assert_eq!(item.detail, "Change current session\u{2019}s profile");

        // Snippet.
        let sources = Sources { has_window: true, snippets: vec![("t".to_string(), "echo hi".to_string(), "t".to_string())], ..Default::default() };
        let item = items("t", &sources).into_iter().find(|it| it.kind == ItemKind::Snippet(0)).unwrap();
        assert_eq!(item.detail, "Send snippet \u{201c}t\u{201d}. Press \u{2325} to edit first.");

        // Named mark.
        let sources = Sources { named_marks: vec![(1, 1, "m".to_string(), false)], ..Default::default() };
        let item = items("m", &sources).into_iter().find(|it| it.kind == ItemKind::NamedMark { pane: 1, mark: 1 }).unwrap();
        assert_eq!(item.detail, "Named mark \u{201c}m\u{201d}");

        // Menu item, and its alternate.
        let sources = Sources {
            menu_items: vec![("id1".to_string(), "Session Status".to_string(), vec!["View".to_string(), "Toolbelt".to_string()], false)],
            ..Default::default()
        };
        let item = items("Session Status", &sources).into_iter().find(|it| it.kind == ItemKind::MenuItem("id1".to_string())).unwrap();
        assert_eq!(item.detail, "Menu item under View > Toolbelt");

        let sources = Sources {
            menu_items: vec![("id2".to_string(), "Session Status".to_string(), vec!["View".to_string(), "Toolbelt".to_string()], true)],
            ..Default::default()
        };
        let item = items("Session Status", &sources).into_iter().find(|it| it.kind == ItemKind::MenuItem("id2".to_string())).unwrap();
        assert_eq!(item.detail, "Alternate menu item under View > Toolbelt");

        // A missing title reads "Untitled".
        let sources = Sources {
            sessions: vec![SessionSource { pane: 5, name: String::new(), badge: "abc".to_string(), ..Default::default() }],
            ..Default::default()
        };
        let item = items("abc", &sources).into_iter().find(|it| it.kind == ItemKind::Session(5)).unwrap();
        assert_eq!(item.title, "Untitled");

        // Bold indexes follow the matched characters.
        let sources = Sources { sessions: vec![SessionSource { pane: 9, name: "axbxc".to_string(), ..Default::default() }], ..Default::default() };
        let item = items("abc", &sources).into_iter().find(|it| it.kind == ItemKind::Session(9)).unwrap();
        assert_eq!(item.title, "axbxc");
        assert_eq!(item.title_bold, vec![0, 2, 4]);
    }

    #[test]
    fn color_preset_detail_string_kept_for_milestone_13() {
        assert_eq!(fill(COLOR_PRESET_DETAIL, "Solarized"), "Load color preset \u{201d}Solarized\u{201c}");
    }

    #[test]
    fn review_fixes_follow_iterm2() {
        // 1. bestIndexes stops as soon as a candidate's range length equals the query length,
        // even when a smaller span exists further on (kept verbatim).
        let idx = matched("ab", "axbab").unwrap();
        assert_eq!(idx, vec![0, 2]);
        assert!((quality(&idx, 5) - 0.9).abs() < 1e-9);

        // 2. Case folding is one char to one char, so indexes stay valid even when a char's
        // lowercase form is not (e.g. '\u{130}' folds to one char here, not two).
        let sources = Sources {
            sessions: vec![SessionSource { pane: 1, name: "\u{130}stanbul".to_string(), ..Default::default() }],
            ..Default::default()
        };
        let item = items("bul", &sources).into_iter().find(|it| it.kind == ItemKind::Session(1)).unwrap();
        let title_len = item.title.chars().count();
        assert!(item.title_bold.iter().all(|&i| i < title_len), "{:?} vs len {title_len}", item.title_bold);
        assert_eq!(item.title_bold, vec![5, 6, 7]);

        // 3. A menu item's detail bolds only the path run.
        let sources = Sources {
            menu_items: vec![("id1".to_string(), "Session Status".to_string(), vec!["View".to_string(), "Toolbelt".to_string()], false)],
            ..Default::default()
        };
        let item = items("Session Status", &sources).into_iter().find(|it| it.kind == ItemKind::MenuItem("id1".to_string())).unwrap();
        let prefix_len = "Menu item under ".chars().count();
        let path_len = "View > Toolbelt".chars().count();
        assert_eq!(item.detail_bold, vec![prefix_len..(prefix_len + path_len)]);

        // 4. The score fed to detail-feature selection is the unclamped running total, not the
        // total returned for the session's own score.
        let mut features: Vec<Feature> = Vec::new();
        let mut total = 0.0;
        push_feature(&mut features, "Command", &[String::from("abcd"), String::from("abcd")], 0.8, 1.0, "abcd", &mut total);
        assert_eq!(features.len(), 1);
        assert!((features[0].score - 1.2).abs() < 1e-9, "{}", features[0].score); // 0.8 * (1/2 + 1)
        assert!((total - 1.0).abs() < 1e-9, "{total}"); // the session's own total keeps the clamp

        // 5. An empty query's multi-document feature shows its newest non-empty document, not
        // its oldest (detailForSession:features: keeps the last tied entry).
        let mut features: Vec<Feature> = Vec::new();
        let mut total = 0.0;
        push_feature(&mut features, "Command", &[String::from("old"), String::from("new")], 0.8, 10.0, "", &mut total);
        assert_eq!(features[0].value, "new");
    }
}

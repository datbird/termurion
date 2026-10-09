//! Find on a web page (milestone 13x3, Task 1): iTerm2's `custom-find.js` and the commands it takes
//! (`iTermBrowserFindManager.swift`). The script runs only in Tandem's isolated world; this module
//! assembles it, names its four search modes and builds the JSON commands the app hands it. Pure:
//! the page, the secret's use and every result are the app's and the next tasks' business.

use serde_json::{Value, json};

use crate::browser_js::{self, Inject, UserScript, World};
use crate::find::Mode;

/// The message handler the find script posts to (`FM:68`).
pub const HANDLER: &str = "iTermCustomFind";
/// The user script's identifier (`BM:403`).
pub const SCRIPT_ID: &str = "iTermCustomFind";

/// The expression `browser_js_call` runs with the argument `command` (`FM:230`), in the main frame's
/// Tandem world: the page's world has no `iTermCustomFind`.
pub const COMMAND_JS: &str = "return window.iTermCustomFind && await window.iTermCustomFind.handleCommand(command);";

/// The thirteen files copied from `FindOnPage/` (`graph-discovery-api.js`, which `custom-find.js`
/// also includes, is 13x2's). Each is registered in `browser_js::resource`.
pub const FILES: &[&str] = &[
    "custom-find.js",
    "find-api.js",
    "find-constants.js",
    "find-engine.js",
    "find-global-click.js",
    "find-highlighter.js",
    "find-iframe-segment.js",
    "find-match.js",
    "find-nav.js",
    "find-security.js",
    "find-segment.js",
    "find-text-segment.js",
    "find-utils.js",
];

/// `iTermBrowserFindMode` (`FM:15-20`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindMode {
    CaseSensitive,
    CaseInsensitive,
    CaseSensitiveRegex,
    CaseInsensitiveRegex,
}

impl FindMode {
    /// The raw value the script's `VALID_SEARCH_MODES` lists (`FC:15`).
    pub fn raw(self) -> &'static str {
        match self {
            FindMode::CaseSensitive => "caseSensitive",
            FindMode::CaseInsensitive => "caseInsensitive",
            FindMode::CaseSensitiveRegex => "caseSensitiveRegex",
            FindMode::CaseInsensitiveRegex => "caseInsensitiveRegex",
        }
    }
}

/// Unicode category Lu or Lt: `CharacterSet.uppercaseLetters`. Rust's `is_uppercase` is Lu plus
/// Other_Uppercase (symbols such as a Roman numeral) and leaves out titlecase (Lt).
fn uppercase_letter(c: char) -> bool {
    let other_uppercase = matches!(c, '\u{2160}'..='\u{216f}' | '\u{24b6}'..='\u{24cf}' | '\u{1f130}'..='\u{1f149}' | '\u{1f150}'..='\u{1f169}' | '\u{1f170}'..='\u{1f189}');
    let titlecase = matches!(c, '\u{1c5}' | '\u{1c8}' | '\u{1cb}' | '\u{1f2}' | '\u{1f88}'..='\u{1f8f}' | '\u{1f98}'..='\u{1f9f}' | '\u{1fa8}'..='\u{1faf}' | '\u{1fbc}' | '\u{1fcc}' | '\u{1ffc}');
    titlecase || (c.is_uppercase() && !other_uppercase)
}

/// `iTermFindMode.browserFindMode(query:)` (`PB:535-551`): smart case is case sensitive when the
/// query holds an uppercase letter.
pub fn browser_mode(mode: Mode, query: &str) -> FindMode {
    match mode {
        Mode::SmartCase if query.chars().any(uppercase_letter) => FindMode::CaseSensitive,
        Mode::SmartCase => FindMode::CaseInsensitive,
        Mode::CaseSensitive => FindMode::CaseSensitive,
        Mode::CaseInsensitive => FindMode::CaseInsensitive,
        Mode::RegexSensitive => FindMode::CaseSensitiveRegex,
        Mode::RegexInsensitive => FindMode::CaseInsensitiveRegex,
    }
}

/// The name of an include still in `text` (the template loader leaves a missing one in place in a
/// release build).
fn missing_include(text: &str) -> Option<String> {
    let at = text.find("{{INCLUDE:")? + "{{INCLUDE:".len();
    let end = text[at..].find("}}").map_or(text.len(), |e| at + e);
    Some(text[at..end].chars().take(64).collect())
}

/// `FM:356-367`: `custom-find.js` with its includes expanded, then the keys. The secret goes into a
/// JS string literal, so it must be plain letters and digits (the view's is 32 hex characters).
pub fn find_script(secret: &str) -> Result<String, String> {
    if secret.is_empty() || !secret.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err("the find secret is not plain letters and digits".into());
    }
    let src = browser_js::resource("custom-find.js").ok_or("custom-find.js is not embedded")?;
    let script = browser_js::fill_template(src, &[("SECRET", secret), ("SCROLL_BEHAVIOR", "smooth"), ("TEST_FUNCTIONS", ""), ("TEST_IMPLS", ""), ("TEST_FREEZE", "")]);
    match missing_include(&script) {
        Some(name) => Err(format!("find script include not found: {name}")),
        None => Ok(script),
    }
}

/// The find script as a user script: the isolated world, every frame, at document start (`BM:397-408`).
/// An `Err` means the pane gets no find script (find then finds nothing).
pub fn user_script(secret: &str) -> Result<UserScript, String> {
    find_script(secret).map(|code| UserScript { id: SCRIPT_ID, code, inject: Inject::DocumentStart, main_frame_only: false, world: World::Client })
}

/// `FM:223-227`: the command with the instance and the secret added last, so `extra` cannot set them.
pub fn command(action: &str, extra: Value, instance: &str, secret: &str) -> Value {
    let mut o = match extra {
        Value::Object(o) => o,
        _ => serde_json::Map::new(),
    };
    o.insert("action".into(), json!(action));
    o.insert("instanceId".into(), json!(instance));
    o.insert("sessionSecret".into(), json!(secret));
    Value::Object(o)
}

/// The default find state's instance id (`FM:339`).
pub fn default_instance(uuid: &str) -> String {
    format!("main_{uuid}")
}

/// The global find state's instance id (`FM:340`).
pub fn global_instance(uuid: &str) -> String {
    format!("global_{uuid}")
}

/// Longest match text or context kept, in characters (`MAX_CONTEXT_LENGTH`, `FC`).
pub const MAX_TEXT: usize = 500;
/// Largest match identifier kept, serialized: it goes back to the page for a reveal.
pub const MAX_IDENTIFIER_JSON: usize = 4096;
/// Counts a page reports are held to this (a hostile page cannot make the label absurd).
const MAX_COUNT: usize = 100_000_000;

/// One match, as iTerm2's `iTermBrowserFindResult` (`FM:22-51`).
#[derive(Clone, Debug, PartialEq)]
pub struct FindResult {
    pub index: usize,
    pub identifier: Value,
    pub text: Option<String>,
    pub before: Option<String>,
    pub after: Option<String>,
}

/// What a page message did to the model.
#[derive(Clone, Debug, PartialEq)]
pub enum Update {
    /// Not for the model: wrong secret, an unknown shape, or a finished global stream.
    Ignored,
    /// Only the totals moved (`currentChanged`, or a full update without identifiers).
    Counts,
    /// A full update for the bar's search.
    Default { found: bool, results: Vec<FindResult> },
    /// The one batch of the global stream.
    Global(Vec<FindResult>),
}

/// One of the two states (`FM:95-123`).
#[derive(Clone, Debug)]
struct State {
    instance: String,
    term: Option<String>,
    active: bool,
    total: usize,
    current: usize,
    progress: f64,
    complete: bool,
    generation: u64,
    /// The global state's stream: `Some(done)` once a global search began.
    stream: Option<bool>,
}

impl State {
    fn new(instance: String) -> State {
        State { instance, term: None, active: false, total: 0, current: 0, progress: 0.0, complete: false, generation: 0, stream: None }
    }

    fn reset(&mut self, active: bool) {
        self.total = 0;
        self.current = 0;
        self.progress = 0.0;
        self.complete = false;
        self.active = active;
    }

    fn command(&self, action: &str, extra: Value, secret: &str) -> Value {
        command(action, extra, &self.instance, secret)
    }

    /// `clearFind` (`FM:315-326`).
    fn clear(&mut self, secret: &str) -> Value {
        self.term = None;
        self.reset(false);
        self.command("clearFind", json!({}), secret)
    }

    /// The state's `startFind` (`FM:250-288`): an empty term clears.
    fn start(&mut self, term: &str, mode: FindMode, context: usize, secret: &str) -> Value {
        if term.is_empty() {
            return self.clear(secret);
        }
        self.term = Some(term.to_string());
        self.reset(true);
        self.generation += 1;
        self.command("startFind", json!({"searchTerm": term, "contextLength": context, "searchMode": mode.raw()}), secret)
    }
}

/// iTerm2's `iTermBrowserFindManager` without the web view: the bar's search state and the global
/// one. Commands are returned in the order the app must send them (one at a time, `FM:270`).
#[derive(Clone, Debug)]
pub struct Model {
    default: State,
    global: State,
    last_global: bool,
    in_progress: bool,
    /// A `hideResults` for the global instance is owed before the next local command (`FM:452-459`).
    owed_hide: bool,
}

/// Page text as shown: control, bidirectional and zero-width characters out (a newline kept for
/// the snippet to turn into a space), cut to `MAX_TEXT`.
fn shown(s: &str) -> String {
    crate::inert_files::display_text(s, MAX_TEXT, true)
}

/// A count a page reported: a non-negative integer, else 0 (`as? Int ?? 0`), held to `MAX_COUNT`.
fn count(v: Option<&Value>) -> usize {
    v.and_then(Value::as_i64).filter(|n| *n > 0).map_or(0, |n| (n as u64).min(MAX_COUNT as u64) as usize)
}

/// The results of one full update (`FM:126-176`), bounded: at most `MAX_HITS`, strings shown
/// safely, an identifier too big to hand back dropped. `None` when the lists are not lists of
/// objects (iTerm2's cast fails and the update is dropped).
fn results_of(data: &Value) -> Option<Vec<FindResult>> {
    let ids = data.get("matchIdentifiers")?.as_array()?;
    if !ids.iter().all(Value::is_object) {
        return None;
    }
    let contexts = data.get("contexts").and_then(Value::as_array);
    let mut out = Vec::new();
    for (i, id) in ids.iter().enumerate() {
        if out.len() >= crate::global_find_ui::MAX_HITS {
            break;
        }
        if id.to_string().len() > MAX_IDENTIFIER_JSON {
            continue;
        }
        // Both halves or neither, by position (iTerm2 compacts first and can misalign).
        let (before, after) = match contexts.and_then(|c| c.get(i)) {
            Some(c) => match (c.get("contextBefore").and_then(Value::as_str), c.get("contextAfter").and_then(Value::as_str)) {
                (Some(b), Some(a)) => (Some(shown(b)), Some(shown(a))),
                _ => (None, None),
            },
            None => (None, None),
        };
        out.push(FindResult { index: i, identifier: id.clone(), text: id.get("text").and_then(Value::as_str).map(shown), before, after });
    }
    Some(out)
}

impl Model {
    /// The two instance ids carry the given UUIDs (`FM:339-340`).
    pub fn new(uuid_default: &str, uuid_global: &str) -> Model {
        Model { default: State::new(default_instance(uuid_default)), global: State::new(global_instance(uuid_global)), last_global: false, in_progress: false, owed_hide: false }
    }

    /// `hideGlobalSearchResultsIfNeeded` (`FM:452-459`): the hide is owed, not sent here.
    fn owe_hide(&mut self) {
        if self.last_global {
            self.owed_hide = true;
            self.last_global = false;
        }
    }

    fn hide_command(&mut self, secret: &str) -> Option<Value> {
        if std::mem::take(&mut self.owed_hide) { Some(self.global.command("hideResults", json!({}), secret)) } else { None }
    }

    /// The `hideResults` for the global instance that the last `start`, `continue_find` or
    /// `navigation_shortcuts` owes: the app sends it BEFORE that call's command. At most once.
    pub fn take_hide(&mut self, secret: &str) -> Option<Value> {
        self.hide_command(secret)
    }

    /// `startFind` (`FM:469-477`). `None`: an unchanged term that is not forced.
    pub fn start(&mut self, term: &str, mode: FindMode, force: bool, secret: &str) -> Option<Value> {
        self.owe_hide();
        if self.default.term.as_deref() == Some(term) && !force {
            return None;
        }
        self.in_progress = true;
        Some(self.default.start(term, mode, 0, secret))
    }

    /// `findNext` (`FM:479-482`): the owed hide, then the step while a search is active.
    pub fn next(&mut self, secret: &str) -> Vec<Value> {
        self.step("findNext", secret)
    }

    /// `findPrevious` (`FM:484-487`).
    pub fn previous(&mut self, secret: &str) -> Vec<Value> {
        self.step("findPrevious", secret)
    }

    fn step(&mut self, action: &str, secret: &str) -> Vec<Value> {
        self.owe_hide();
        let mut out: Vec<Value> = self.hide_command(secret).into_iter().collect();
        if self.default.active {
            out.push(self.default.command(action, json!({}), secret));
        }
        out
    }

    /// `clearFind` (`FM:489-497`) on the bar's search.
    pub fn clear(&mut self, secret: &str) -> Value {
        self.default.clear(secret)
    }

    /// `executeGlobalSearch` (`FM:414-436`): clear the bar's search the first time, clear the global
    /// instance, start it with 40 characters of context. An empty query only clears.
    pub fn global_start(&mut self, query: &str, mode: FindMode, secret: &str) -> Vec<Value> {
        let mut out = Vec::new();
        if !self.last_global {
            out.push(self.default.clear(secret));
        }
        self.last_global = true;
        self.owed_hide = false;
        out.push(self.global.clear(secret));
        self.global.stream = Some(false);
        if !query.is_empty() {
            out.push(self.global.start(query, mode, 40, secret));
        }
        out
    }

    /// `reveal(globalFindResultWithIdentifier:)` (`FM:438-450`): show the global results first when
    /// the last search was local, then reveal. The measurement that follows is skipped (Decision).
    pub fn global_reveal(&mut self, identifier: &Value, secret: &str) -> Vec<Value> {
        let mut out = Vec::new();
        if !self.last_global {
            out.push(self.global.command("showResults", json!({}), secret));
            // The results are showing again: the next local command hides them (iTerm2 forgets).
            self.last_global = true;
            self.owed_hide = false;
        }
        out.push(self.global.command("reveal", json!({"identifier": identifier}), secret));
        out
    }

    /// `convertVisibleSearchResultsToContentNavigationShortcuts` (`FM:377-399`): `action` is
    /// "open" or "copy", anything else sends nothing. The owed hide goes first (`take_hide`).
    pub fn navigation_shortcuts(&mut self, action: &str, secret: &str) -> Vec<Value> {
        if !matches!(action, "open" | "copy") {
            return Vec::new();
        }
        self.owe_hide();
        let mut out: Vec<Value> = self.hide_command(secret).into_iter().collect();
        out.push(self.default.command("startNavigationShortcuts", json!({"selectionAction": action}), secret));
        out
    }

    /// `handleMessage` (`FM:550-571`) and `handleResultsUpdate` (`FM:126-176`). A full update (one
    /// with `matchIdentifiers`) sets the totals and answers the results; a partial one moves the
    /// totals only (iTerm2 drops it: writer question 3). The totals are the page's, so the shim's
    /// cut list can be shorter than `total()`.
    pub fn on_message(&mut self, body: &Value, secret: &str) -> Update {
        if body.get("sessionSecret").and_then(Value::as_str) != Some(secret) {
            return Update::Ignored;
        }
        let action = body.get("action").and_then(Value::as_str);
        if !matches!(action, Some("resultsUpdated" | "currentChanged")) {
            return Update::Ignored;
        }
        let Some(data) = body.get("data").filter(|d| d.is_object()) else { return Update::Ignored };
        let is_global = data.get("instanceId").and_then(Value::as_str) == Some(self.global.instance.as_str());
        let state = if is_global { &mut self.global } else { &mut self.default };
        let full = data.get("matchIdentifiers").is_some_and(|v| !v.is_null());
        let total = count(data.get("totalMatches"));
        let current = count(data.get("currentMatch")).min(total);
        if !full {
            state.total = total;
            state.current = current;
            return if is_global { Update::Ignored } else { Update::Counts };
        }
        let Some(results) = results_of(data) else { return Update::Ignored };
        state.total = total;
        state.current = current;
        if !state.complete {
            state.complete = true;
            state.progress = 1.0;
        }
        if is_global {
            match state.stream {
                Some(false) => {
                    state.stream = Some(true);
                    Update::Global(results)
                }
                _ => Update::Ignored,
            }
        } else {
            Update::Default { found: total > 0, results }
        }
    }

    /// `continueFind` (`FM:518-544`): whether more work is coming, and the progress.
    pub fn continue_find(&mut self) -> (bool, f64) {
        self.owe_hide();
        if !self.default.active || self.default.complete {
            self.in_progress = false;
            return (false, 1.0);
        }
        (true, self.default.progress)
    }

    /// The search generation: a command queued for an older one is stale (`FM:272`).
    pub fn generation(&self) -> u64 {
        self.default.generation
    }

    /// `activeSearchTerm` (`FM:503-505`).
    pub fn active_term(&self) -> Option<&str> {
        if self.default.active { self.default.term.as_deref() } else { None }
    }

    pub fn total(&self) -> usize {
        self.default.total
    }

    pub fn current(&self) -> usize {
        self.default.current
    }

    pub fn in_progress(&self) -> bool {
        self.in_progress
    }
}

/// The counter beside the bar (`SFC:285-301`): nothing without results; the total while no match
/// is current; else `i/total`, where `i` counts backward with the swap off, as iTerm2's does.
pub fn label(total: usize, current: usize, swap: bool) -> String {
    if current == 0 {
        return if total == 0 { String::new() } else { total.to_string() };
    }
    let i = if swap { current } else { (total + 1).wrapping_sub(current) };
    format!("{i}/{total}")
}

/// A global-search row (`GSEC:484-494`): before, the match, after, newlines as spaces; with the
/// match's byte range.
pub fn snippet(r: &FindResult) -> (String, std::ops::Range<usize>) {
    let flat = |s: &Option<String>| s.as_deref().unwrap_or("").replace('\n', " ");
    let before = flat(&r.before);
    let text = r.text.as_deref().unwrap_or("");
    let start = before.len();
    (format!("{before}{text}{}", flat(&r.after)), start..start + text.len())
}
#[cfg(test)]
mod tests {
    use super::*;

    const THIRTEEN: [&str; 13] = [
        "custom-find.js",
        "find-api.js",
        "find-constants.js",
        "find-engine.js",
        "find-global-click.js",
        "find-highlighter.js",
        "find-iframe-segment.js",
        "find-match.js",
        "find-nav.js",
        "find-security.js",
        "find-segment.js",
        "find-text-segment.js",
        "find-utils.js",
    ];

    /// A file's text as it sits in the script: its two slots filled.
    fn filled(name: &str, secret: &str) -> String {
        browser_js::resource(name).unwrap().replace("{{SECRET}}", secret).replace("{{SCROLL_BEHAVIOR}}", "smooth")
    }

    #[test]
    fn browser_modes_follow_iterm2() {
        use FindMode::*;
        assert_eq!(browser_mode(Mode::SmartCase, "rust"), CaseInsensitive);
        assert_eq!(browser_mode(Mode::SmartCase, "Rust"), CaseSensitive);
        // Unicode uppercase counts (`CharacterSet.uppercaseLetters`), titlecase too (Lt).
        assert_eq!(browser_mode(Mode::SmartCase, "\u{c9}t\u{e9}"), CaseSensitive);
        assert_eq!(browser_mode(Mode::SmartCase, "a\u{1c5}"), CaseSensitive);
        assert_eq!(browser_mode(Mode::SmartCase, "123"), CaseInsensitive);
        assert_eq!(browser_mode(Mode::SmartCase, ""), CaseInsensitive);
        // Other_Uppercase symbols (a Roman numeral, a circled letter) are not letters of category Lu.
        assert_eq!(browser_mode(Mode::SmartCase, "\u{2160}\u{24b6}"), CaseInsensitive);
        assert_eq!(browser_mode(Mode::CaseSensitive, "abc"), CaseSensitive);
        assert_eq!(browser_mode(Mode::CaseInsensitive, "ABC"), CaseInsensitive);
        assert_eq!(browser_mode(Mode::RegexSensitive, "abc"), CaseSensitiveRegex);
        assert_eq!(browser_mode(Mode::RegexInsensitive, "ABC"), CaseInsensitiveRegex);
        let constants = browser_js::resource("find-constants.js").expect("find-constants.js is embedded");
        let list = constants.lines().find(|l| l.starts_with("const VALID_SEARCH_MODES")).expect("VALID_SEARCH_MODES");
        for (mode, raw) in [(CaseSensitive, "caseSensitive"), (CaseInsensitive, "caseInsensitive"), (CaseSensitiveRegex, "caseSensitiveRegex"), (CaseInsensitiveRegex, "caseInsensitiveRegex")] {
            assert_eq!(mode.raw(), raw);
            assert!(list.contains(&format!("'{raw}'")), "{raw} is in {list}");
        }
    }

    #[test]
    fn includes_follow_iterm2() {
        let custom = browser_js::resource("custom-find.js").expect("custom-find.js is embedded");
        let names: Vec<&str> = custom.split("{{INCLUDE:").skip(1).map(|t| t.split("}}").next().unwrap()).collect();
        assert_eq!(
            names,
            ["graph-discovery-api.js", "find-security.js", "find-constants.js", "find-utils.js", "find-global-click.js", "find-segment.js", "find-text-segment.js", "find-iframe-segment.js", "find-match.js", "find-engine.js", "find-api.js", "find-highlighter.js", "find-nav.js"],
            "CF:5-17"
        );
        for n in &names {
            assert!(browser_js::resource(n).is_some(), "{n} is registered");
        }
        let script = find_script("s").expect("the script assembles");
        assert!(!script.contains("{{INCLUDE:"));
        let at = |n: &str| script.find(&filled(n, "s")).unwrap_or_else(|| panic!("{n} is in the script"));
        assert!(at("graph-discovery-api.js") < at("find-nav.js"));
        for pair in names.windows(2) {
            assert!(at(pair[0]) < at(pair[1]), "{} before {}", pair[0], pair[1]);
        }
        for f in FILES {
            assert!(browser_js::resource(f).is_some(), "{f} is registered");
        }
    }

    #[test]
    fn find_script_follows_iterm2() {
        let s = find_script("abc").expect("the script assembles");
        assert!(s.contains("const sessionSecret = \"abc\";"), "FC:7");
        assert!(s.contains("behavior: 'smooth'"), "FE:1313");
        assert!(!s.contains("{{"), "no slot is left");
        for f in ["find-engine.js", "find-nav.js"] {
            assert!(s.contains(&filled(f, "abc")), "{f}");
        }
        assert_eq!(s.matches("Object.defineProperty(window, 'iTermCustomFind'").count(), 1, "FA:308");
        assert_eq!(FILES.len(), 13);
        let mut want = THIRTEEN.to_vec();
        want.sort();
        let mut have = FILES.to_vec();
        have.sort();
        assert_eq!(have, want);
        let u = user_script("abc").expect("the user script");
        assert_eq!((u.id, u.world, u.inject, u.main_frame_only), (SCRIPT_ID, World::Client, Inject::DocumentStart, false));
        assert_eq!(u.code, s);
        // A leftover include is an error naming it, never a silent gap.
        assert_eq!(missing_include("a {{INCLUDE:nope.js}} b"), Some("nope.js".to_string()));
        assert_eq!(missing_include("a b"), None);
    }

    #[test]
    fn commands_follow_iterm2() {
        let c = command("startFind", json!({"searchTerm": "a", "contextLength": 0, "searchMode": "caseSensitive"}), "main_U", "s");
        let o = c.as_object().expect("an object");
        let mut keys: Vec<&str> = o.keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(keys, ["action", "contextLength", "instanceId", "searchMode", "searchTerm", "sessionSecret"]);
        assert_eq!(c, json!({"action": "startFind", "searchTerm": "a", "contextLength": 0, "searchMode": "caseSensitive", "instanceId": "main_U", "sessionSecret": "s"}));
        assert_eq!(command("findNext", json!({}), "main_U", "s")["action"], "findNext");
        // The caller's extra cannot override what the manager sets last (`FM:223-227`).
        let c = command("findNext", json!({"instanceId": "x", "sessionSecret": "y", "action": "z"}), "main_U", "s");
        assert_eq!((c["action"].as_str(), c["instanceId"].as_str(), c["sessionSecret"].as_str()), (Some("findNext"), Some("main_U"), Some("s")));
        let constants = browser_js::resource("find-constants.js").unwrap();
        let valid = constants.lines().find(|l| l.starts_with("const VALID_ACTIONS")).expect("VALID_ACTIONS");
        for a in ["startFind", "findNext", "findPrevious", "clearFind", "reveal", "hideResults", "showResults", "startNavigationShortcuts", "clearNavigationShortcuts"] {
            assert!(valid.contains(&format!("'{a}'")), "{a} is valid");
        }
        let uuid = "123e4567-e89b-12d3-a456-426614174000";
        assert_eq!(uuid.len(), 36);
        assert_eq!(default_instance("U"), "main_U");
        assert_eq!(global_instance("U"), "global_U");
        assert!(default_instance(uuid).len() <= 50 && global_instance(uuid).len() <= 50, "FU:36-45 keeps 50");
        assert!(COMMAND_JS.contains("window.iTermCustomFind") && COMMAND_JS.contains("handleCommand(command)"));
    }

    fn cmd(v: &Value) -> (&str, &str) {
        (v["action"].as_str().unwrap_or(""), v["instanceId"].as_str().unwrap_or(""))
    }

    fn ident(i: usize, text: &str) -> Value {
        json!({"index": i, "coordinates": {"x": i}, "text": text})
    }

    fn full(instance: &str, total: usize, current: usize, ids: Vec<Value>, contexts: Option<Vec<Value>>) -> Value {
        let mut data = json!({"instanceId": instance, "totalMatches": total, "currentMatch": current, "matchIdentifiers": ids});
        if let Some(c) = contexts {
            data["contexts"] = json!(c);
        }
        json!({"sessionSecret": "s", "action": "resultsUpdated", "data": data})
    }

    #[test]
    fn start_rules_follow_iterm2() {
        let mut m = Model::new("U", "U");
        assert_eq!(m.active_term(), None);
        assert!(m.next("s").is_empty(), "FM:291");
        assert!(m.previous("s").is_empty());
        let c = m.start("a", FindMode::CaseSensitive, false, "s").expect("a fresh search starts");
        assert_eq!(c, json!({"action": "startFind", "searchTerm": "a", "contextLength": 0, "searchMode": "caseSensitive", "instanceId": "main_U", "sessionSecret": "s"}));
        assert_eq!(m.active_term(), Some("a"));
        assert!(m.in_progress());
        assert_eq!(m.start("a", FindMode::CaseSensitive, false, "s"), None, "FM:471-474");
        assert!(m.start("a", FindMode::CaseSensitive, true, "s").is_some(), "forced");
        assert_eq!(m.next("s"), vec![command("findNext", json!({}), "main_U", "s")]);
        assert_eq!(m.previous("s"), vec![command("findPrevious", json!({}), "main_U", "s")]);
        let c = m.start("", FindMode::CaseSensitive, false, "s").expect("an empty term clears");
        assert_eq!(cmd(&c), ("clearFind", "main_U"));
        assert_eq!(m.active_term(), None, "FM:315-326");
        assert!(m.next("s").is_empty(), "no longer active");
        // Progress ends when a complete result is seen (`FM:536-541`).
        let mut m = Model::new("U", "U");
        m.start("a", FindMode::CaseInsensitive, false, "s");
        assert!(m.in_progress());
        assert_eq!(m.continue_find(), (true, 0.0));
        assert_eq!(m.on_message(&full("main_U", 1, 1, vec![ident(0, "a")], None), "s"), Update::Default { found: true, results: vec![FindResult { index: 0, identifier: ident(0, "a"), text: Some("a".into()), before: None, after: None }] });
        assert_eq!(m.continue_find(), (false, 1.0));
        assert!(!m.in_progress());
    }

    #[test]
    fn results_follow_iterm2() {
        let mut m = Model::new("U", "U");
        assert_eq!(m.continue_find(), (false, 1.0), "FM:520-525");
        m.start("a", FindMode::CaseSensitive, false, "s");
        let ids = vec![ident(0, "a"), ident(1, "a"), ident(2, "a")];
        let mut bad = full("main_U", 3, 1, ids.clone(), None);
        bad["sessionSecret"] = json!("wrong");
        assert_eq!(m.on_message(&bad, "s"), Update::Ignored, "FM:553-554");
        assert_eq!(m.total(), 0);
        match m.on_message(&full("main_U", 3, 1, ids.clone(), None), "s") {
            Update::Default { found, results } => {
                assert!(found);
                assert_eq!(results.len(), 3);
                assert_eq!(results[1].index, 1);
                assert_eq!(results[1].text.as_deref(), Some("a"));
                assert_eq!((results[1].before.clone(), results[1].after.clone()), (None, None));
            }
            u => panic!("{u:?}"),
        }
        assert_eq!((m.total(), m.current()), (3, 1));
        let ctx = vec![json!({"contextBefore": "x ", "contextAfter": " y"}); 3];
        match m.on_message(&full("main_U", 3, 1, ids.clone(), Some(ctx)), "s") {
            Update::Default { results, .. } => assert_eq!((results[2].before.as_deref(), results[2].after.as_deref()), (Some("x "), Some(" y"))),
            u => panic!("{u:?}"),
        }
        let partial = json!({"sessionSecret": "s", "action": "currentChanged", "data": {"instanceId": "main_U", "totalMatches": 3, "currentMatch": 2}});
        assert_eq!(m.on_message(&partial, "s"), Update::Counts, "writer question 3");
        assert_eq!((m.total(), m.current()), (3, 2));
        // No matches.
        let none = full("main_U", 0, 0, vec![], None);
        assert_eq!(m.on_message(&none, "s"), Update::Default { found: false, results: vec![] });
        // The global stream takes one batch (`FM:168-172`).
        m.global_start("q", FindMode::CaseInsensitive, "s");
        match m.on_message(&full("global_U", 3, 1, ids.clone(), None), "s") {
            Update::Global(r) => assert_eq!(r.len(), 3),
            u => panic!("{u:?}"),
        }
        assert_eq!(m.on_message(&full("global_U", 3, 1, ids, None), "s"), Update::Ignored);
        // Malformed bodies change nothing.
        assert_eq!(m.on_message(&json!("x"), "s"), Update::Ignored);
        assert_eq!(m.on_message(&json!({"sessionSecret": "s", "action": "other", "data": {}}), "s"), Update::Ignored);
        assert_eq!(m.on_message(&json!({"sessionSecret": "s", "action": "resultsUpdated", "data": {"instanceId": "main_U", "matchIdentifiers": [1, 2]}}), "s"), Update::Ignored);
    }

    #[test]
    fn results_are_bounded() {
        use crate::global_find_ui::MAX_HITS;
        let mut m = Model::new("U", "U");
        m.start("a", FindMode::CaseSensitive, false, "s");
        // More matches than the list holds: the count stays true, the list is cut.
        let n = MAX_HITS + 500;
        let ids: Vec<Value> = (0..n).map(|i| ident(i, "a")).collect();
        match m.on_message(&full("main_U", n, 1, ids, None), "s") {
            Update::Default { found, results } => {
                assert!(found);
                assert_eq!(results.len(), MAX_HITS);
            }
            u => panic!("{u:?}"),
        }
        assert_eq!(m.total(), n);
        // The shim cut the lists: fewer identifiers than the total; the true total shows.
        match m.on_message(&full("main_U", 5000, 7, vec![ident(0, "a"), ident(1, "a")], None), "s") {
            Update::Default { found, results } => assert_eq!((found, results.len()), (true, 2)),
            u => panic!("{u:?}"),
        }
        assert_eq!((m.total(), m.current()), (5000, 7));
        // A page cannot make the current match exceed the total, or the total absurd.
        let partial = json!({"sessionSecret": "s", "action": "currentChanged", "data": {"instanceId": "main_U", "totalMatches": 4, "currentMatch": 900}});
        m.on_message(&partial, "s");
        assert_eq!((m.total(), m.current()), (4, 4));
        let partial = json!({"sessionSecret": "s", "action": "currentChanged", "data": {"instanceId": "main_U", "totalMatches": -5, "currentMatch": -1}});
        m.on_message(&partial, "s");
        assert_eq!((m.total(), m.current()), (0, 0));
        // Every string is cut and its hidden characters removed.
        let long = "x".repeat(MAX_TEXT * 3);
        let ids = vec![ident(0, &format!("a\u{202e}b{long}"))];
        let ctx = vec![json!({"contextBefore": format!("p\u{200b}q{long}"), "contextAfter": "\u{1b}[31mz\nw"})];
        match m.on_message(&full("main_U", 1, 1, ids, Some(ctx)), "s") {
            Update::Default { results, .. } => {
                let r = &results[0];
                assert_eq!(r.text.as_ref().unwrap().chars().count(), MAX_TEXT);
                assert!(r.text.as_ref().unwrap().starts_with("ab"), "bidi removed");
                assert!(r.before.as_ref().unwrap().starts_with("pq"), "zero width removed");
                assert_eq!(r.after.as_deref(), Some("[31mz\nw"), "control removed, newline kept for the snippet");
            }
            u => panic!("{u:?}"),
        }
        // An identifier too big to hand back is dropped from the list.
        let huge = json!({"index": 0, "coordinates": "y".repeat(MAX_IDENTIFIER_JSON * 2), "text": "t"});
        match m.on_message(&full("main_U", 2, 1, vec![huge, ident(1, "b")], None), "s") {
            Update::Default { results, .. } => assert_eq!(results.iter().map(|r| r.index).collect::<Vec<_>>(), [1]),
            u => panic!("{u:?}"),
        }
        // The global stream is bounded the same way.
        m.global_start("q", FindMode::CaseSensitive, "s");
        let ids: Vec<Value> = (0..n).map(|i| ident(i, "a")).collect();
        match m.on_message(&full("global_U", n, 1, ids, None), "s") {
            Update::Global(r) => assert_eq!(r.len(), MAX_HITS),
            u => panic!("{u:?}"),
        }
    }

    #[test]
    fn label_follows_iterm2() {
        assert_eq!(label(0, 0, true), "", "SFC:294-296");
        assert_eq!(label(5, 0, true), "5");
        assert_eq!(label(5, 2, true), "2/5");
        assert_eq!(label(5, 2, false), "4/5", "SFC:299");
        assert_eq!(label(0, 3, true), "3/0", "a current index with no total is shown as iTerm2 does");
    }

    #[test]
    fn global_and_shortcut_commands_follow_iterm2() {
        let mut m = Model::new("U", "V");
        m.start("a", FindMode::CaseSensitive, false, "s");
        let c = m.global_start("q", FindMode::CaseInsensitive, "s");
        assert_eq!(c.len(), 3);
        assert_eq!(cmd(&c[0]), ("clearFind", "main_U"));
        assert_eq!(cmd(&c[1]), ("clearFind", "global_V"));
        assert_eq!(c[2], json!({"action": "startFind", "searchTerm": "q", "contextLength": 40, "searchMode": "caseInsensitive", "instanceId": "global_V", "sessionSecret": "s"}));
        assert_eq!(m.active_term(), None, "the default search was cleared");
        let c = m.global_start("r", FindMode::CaseInsensitive, "s");
        assert_eq!(c.iter().map(|v| cmd(v).0).collect::<Vec<_>>(), ["clearFind", "startFind"], "the default clear happens once");
        assert_eq!(cmd(&c[0]).1, "global_V");
        // A local search after a global one hides the global results first (`FM:452-459`).
        let s = m.start("a", FindMode::CaseSensitive, false, "s");
        assert!(s.is_some());
        assert_eq!(m.take_hide("s"), Some(command("hideResults", json!({}), "global_V", "s")));
        assert_eq!(m.take_hide("s"), None, "once");
        m.global_start("q", FindMode::CaseInsensitive, "s");
        assert_eq!(m.next("s"), vec![command("hideResults", json!({}), "global_V", "s")], "the default search is inactive after a global start");
        assert_eq!(m.next("s"), Vec::<Value>::new());
        // Reveal after a local search shows the global results first (`FM:439-443`).
        let mut m = Model::new("U", "V");
        m.start("a", FindMode::CaseSensitive, false, "s");
        let id = json!({"index": 3});
        let c = m.global_reveal(&id, "s");
        assert_eq!(c.iter().map(|v| cmd(v).0).collect::<Vec<_>>(), ["showResults", "reveal"]);
        assert_eq!(c[1]["identifier"], id);
        assert_eq!(cmd(&c[1]).1, "global_V");
        let c = m.global_reveal(&id, "s");
        assert_eq!(c.len(), 1, "already shown");
        // Navigation shortcuts run on the default instance (`FM:383-393`).
        let mut m = Model::new("U", "V");
        assert_eq!(m.navigation_shortcuts("open", "s"), vec![command("startNavigationShortcuts", json!({"selectionAction": "open"}), "main_U", "s")]);
        assert_eq!(m.navigation_shortcuts("copy", "s")[0]["selectionAction"], "copy");
        assert!(m.navigation_shortcuts("run", "s").is_empty(), "only open and copy");
        // Clear.
        m.start("a", FindMode::CaseSensitive, false, "s");
        assert_eq!(cmd(&m.clear("s")), ("clearFind", "main_U"));
        assert_eq!(m.active_term(), None);
    }

    #[test]
    fn snippet_follows_iterm2() {
        let r = |t: &str, b: Option<&str>, a: Option<&str>| FindResult { index: 0, identifier: json!({}), text: Some(t.into()), before: b.map(Into::into), after: a.map(Into::into) };
        assert_eq!(snippet(&r("hit", Some("a\nb "), Some(" c\nd"))), ("a b hit c d".to_string(), 4..7), "GSEC:486-487");
        assert_eq!(snippet(&r("hit", None, None)), ("hit".to_string(), 0..3));
        let none = FindResult { index: 0, identifier: json!({}), text: None, before: Some("x".into()), after: None };
        assert_eq!(snippet(&none), ("x".to_string(), 1..1));
    }
}

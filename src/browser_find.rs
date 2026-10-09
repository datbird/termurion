//! Find on a web page (milestone 13x3, Task 1): iTerm2's `custom-find.js` and the commands it takes
//! (`iTermBrowserFindManager.swift`). The script runs only in Tandem's isolated world; this module
//! assembles it, names its four search modes and builds the JSON commands the app hands it. Pure:
//! the page, the secret's use and every result are the app's and the next tasks' business.

use serde_json::{Value, json};

use crate::browser_js::{self, Inject, UserScript, World};
use crate::find::Mode;
use crate::layout::Rect;

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
        None => type_global_click(&script).and_then(|s| type_nav_keys(&s)).and_then(|s| type_nav_engine(&s)),
    }
}

/// R13x3-1 (Task 1 review, critical): `importIframeGlobalClick` (`FE:665-673`) pastes the coordinates
/// of an `iTermGlobalClick` window message, which any frame can forge, into the script it has run in
/// that frame's Tandem world. Every value put into that script is typed where it is put: the two
/// coordinates are numbers or the click is dropped, the instance id a JSON string. The copied file is
/// not edited; this text is replaced when the script is assembled, and only when it is found exactly
/// once (an iTerm2 update that moved it fails closed: no find script).
const CLICK_RAW: &str = "        const x = globalClickLocation.clickData.coordinates.x;\n        const y = globalClickLocation.clickData.coordinates.y;\n\n        const conversionScript = `\n            (function() {\n                if (window.iTermCustomFind && window.iTermCustomFind.handleClickInFrame) {\n                    return window.iTermCustomFind.handleClickInFrame(${x}, ${y}, '${this.instanceId}');";
const CLICK_TYPED: &str = "        const __tandemAt = (globalClickLocation.clickData && globalClickLocation.clickData.coordinates) || {};\n        const x = Number(__tandemAt.x);\n        const y = Number(__tandemAt.y);\n        if (!Number.isFinite(x) || !Number.isFinite(y)) {\n            this.log('importGlobalClickLocation: refused a click without finite coordinates');\n            globalClickLocation = null;\n            return;\n        }\n\n        const conversionScript = `\n            (function() {\n                if (window.iTermCustomFind && window.iTermCustomFind.handleClickInFrame) {\n                    return window.iTermCustomFind.handleClickInFrame(${x}, ${y}, ${JSON.stringify(String(this.instanceId))});";

/// The find script with the global click typed (`CLICK_TYPED`); an error unless `CLICK_RAW` is in it
/// exactly once.
fn type_global_click(script: &str) -> Result<String, String> {
    match script.matches(CLICK_RAW).count() {
        1 => Ok(script.replacen(CLICK_RAW, CLICK_TYPED, 1)),
        n => Err(format!("the find script's global click text is there {n} times, not once")),
    }
}

/// R13x3-2 (Find URLs): a page's own `keydown` events must not pick a navigation bubble. find-nav's
/// document listener has no `isTrusted` check (it is iTerm2's, copied unchanged), so the assembled
/// script gets one, with the same load-time rule as `CLICK_RAW`: the text must be found exactly once
/// or there is no find script. Keys from the person (and DevTools input, which Chromium marks
/// trusted) still pick.
const NAV_RAW: &str = "            keyListener = (event) => {\n                if (handleKeyEvent(event)) {";
const NAV_TYPED: &str = "            keyListener = (event) => {\n                if (!event.isTrusted) {\n                    return;\n                }\n                if (handleKeyEvent(event)) {";

/// The find script with find-nav's key listener guarded (`NAV_TYPED`); an error unless `NAV_RAW` is
/// in it exactly once.
fn type_nav_keys(script: &str) -> Result<String, String> {
    match script.matches(NAV_RAW).count() {
        1 => Ok(script.replacen(NAV_RAW, NAV_TYPED, 1)),
        n => Err(format!("the find script's navigation key text is there {n} times, not once")),
    }
}

/// Probe run 2026-10-09: find-nav's bubble script (`FN:419-567`), which evaluateInAll runs in each
/// frame's Tandem world, reached the engine with `eval('getEngine')`. `getEngine` is local to
/// custom-find.js's IIFE, so from a script evaluated at the world's global scope it threw "getEngine
/// is not defined" and no bubble was made (iTerm2's copy has the same text). The frame's own
/// `window.iTermCustomFind` (find-api.js exposes `getEngine`) is the engine of the same IIFE. Same
/// load-time rule: exactly once, or no find script.
const NAV_ENGINE_RAW: &str = "engine = eval('getEngine')(instanceId);";
const NAV_ENGINE_TYPED: &str = "engine = window.iTermCustomFind.getEngine(instanceId);";

/// The find script with find-nav's engine lookup reachable (`NAV_ENGINE_TYPED`); an error unless
/// `NAV_ENGINE_RAW` is in it exactly once.
fn type_nav_engine(script: &str) -> Result<String, String> {
    match script.matches(NAV_ENGINE_RAW).count() {
        1 => Ok(script.replacen(NAV_ENGINE_RAW, NAV_ENGINE_TYPED, 1)),
        n => Err(format!("the find script's navigation engine text is there {n} times, not once")),
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

/// The longest pattern or term the page takes (`MAX_SEARCH_TERM_LENGTH`, `FC:8`), in UTF-16 units
/// (the page slices strings by them).
pub const MAX_TERM: usize = 1000;
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
    /// Find URLs' temporary search is the default search and its shortcuts are still owed.
    urls_pending: bool,
}

/// Page text as shown: control, bidirectional and zero-width characters out (a newline kept for
/// the snippet to turn into a space), cut to `MAX_TEXT`.
fn shown(s: &str) -> String {
    crate::inert_files::display_text(s, MAX_TEXT, true)
}

/// A count a page reported: a non-negative integer, else 0 (`as? Int ?? 0`), held to `MAX_COUNT`
/// (one past `i64` too).
fn count(v: Option<&Value>) -> usize {
    v.and_then(Value::as_u64).map_or(0, |n| n.min(MAX_COUNT as u64) as usize)
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

/// A model for a new view: its two instance ids carry fresh UUIDs (`FM:339-340`).
pub fn fresh_model() -> Model {
    Model::new(&uuid::Uuid::new_v4().simple().to_string(), &uuid::Uuid::new_v4().simple().to_string())
}

impl Model {
    /// The two instance ids carry the given UUIDs (`FM:339-340`).
    pub fn new(uuid_default: &str, uuid_global: &str) -> Model {
        Model { default: State::new(default_instance(uuid_default)), global: State::new(global_instance(uuid_global)), last_global: false, in_progress: false, owed_hide: false, urls_pending: false }
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

    /// `startFind` (`FM:469-477`): the commands in the order they are sent, the owed global
    /// `hideResults` first (`FM:470`). An unchanged term that is not forced sends only that hide.
    pub fn start(&mut self, term: &str, mode: FindMode, force: bool, secret: &str) -> Vec<Value> {
        self.owe_hide();
        let mut out: Vec<Value> = self.hide_command(secret).into_iter().collect();
        if self.default.term.as_deref() == Some(term) && !force {
            return out;
        }
        self.in_progress = true;
        self.urls_pending = false;
        out.push(self.default.start(term, mode, 0, secret));
        out
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
        self.urls_pending = false;
        self.default.clear(secret)
    }

    /// `executeGlobalSearch` (`FM:414-436`): clear the bar's search the first time, clear the global
    /// instance, start it with 40 characters of context. An empty query only clears.
    pub fn global_start(&mut self, query: &str, mode: FindMode, secret: &str) -> Vec<Value> {
        let mut out = Vec::new();
        self.urls_pending = false;
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
    /// Only an identifier the model would keep goes back to the page (an object within
    /// `MAX_IDENTIFIER_JSON`); any other sends nothing.
    pub fn global_reveal(&mut self, identifier: &Value, secret: &str) -> Vec<Value> {
        if !identifier.is_object() || identifier.to_string().len() > MAX_IDENTIFIER_JSON {
            return Vec::new();
        }
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
    /// "open" or "copy", anything else sends nothing. The owed hide goes first.
    pub fn navigation_shortcuts(&mut self, action: &str, secret: &str) -> Vec<Value> {
        if !matches!(action, "open" | "copy") {
            return Vec::new();
        }
        self.owe_hide();
        let mut out: Vec<Value> = self.hide_command(secret).into_iter().collect();
        out.push(self.default.command("startNavigationShortcuts", json!({"selectionAction": action}), secret));
        out
    }

    /// Find URLs' search (`closeViewAndDoTemporarySearchForString`, `PT:3880-3883`): a forced
    /// case-sensitive regex `startFind` that the bar's own query never sees, the owed global hide
    /// first. `pattern` is the person's setting and goes to the page as text, so it is bounded
    /// (`MAX_SEARCH_TERM_LENGTH`, `FC:8`); `None` for an empty or over-long one. The shortcuts follow
    /// through `shortcuts_after_complete`.
    pub fn temporary(&mut self, pattern: &str, secret: &str) -> Option<Vec<Value>> {
        let out = self.temporary_search(pattern, secret)?;
        self.urls_pending = true;
        Some(out)
    }

    /// The same temporary search with no open shortcuts to follow (key action 36, `PS:12119-12125`).
    pub fn temporary_search(&mut self, pattern: &str, secret: &str) -> Option<Vec<Value>> {
        if pattern.is_empty() || pattern.encode_utf16().count() > MAX_TERM {
            return None;
        }
        self.owe_hide();
        let mut out: Vec<Value> = self.hide_command(secret).into_iter().collect();
        self.in_progress = true;
        self.urls_pending = false;
        out.push(self.default.start(pattern, FindMode::CaseSensitiveRegex, 0, secret));
        Some(out)
    }

    /// The open shortcuts once the temporary search's full result is in (`PT:3884-3890`); nothing
    /// when it found nothing (the engine would answer "No matches to navigate", `FE:1873-1875`), when
    /// the result is not in yet, when they were already asked for, or when a search of the person's
    /// own replaced the temporary one.
    pub fn shortcuts_after_complete(&mut self, secret: &str) -> Option<Vec<Value>> {
        if !self.urls_pending || !self.default.active || !self.default.complete {
            return None;
        }
        self.urls_pending = false;
        if self.default.total == 0 {
            return None;
        }
        Some(self.navigation_shortcuts("open", secret))
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
        let instance = data.get("instanceId").and_then(Value::as_str);
        let is_global = instance == Some(self.global.instance.as_str());
        if !is_global && instance != Some(self.default.instance.as_str()) {
            return Update::Ignored;
        }
        let state = if is_global { &mut self.global } else { &mut self.default };
        // A late report for a search that was cleared brings nothing back.
        if !state.active {
            return Update::Ignored;
        }
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

    /// `continueFind` (`FM:518-544`): whether more work is coming, and the progress. The global
    /// hide it owes is not sent here: the next command the model returns carries it first.
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
    let i = if swap { current } else { (total + 1).saturating_sub(current) };
    format!("{i}/{total}")
}

/// A global-search row (`GSEC:484-494`): before, the match, after, newlines as spaces (the
/// match's too: one row); with the match's byte range.
pub fn snippet(r: &FindResult) -> (String, std::ops::Range<usize>) {
    let flat = |s: &Option<String>| s.as_deref().unwrap_or("").replace('\n', " ");
    let before = flat(&r.before);
    let text = flat(&r.text);
    let start = before.len();
    (format!("{before}{text}{}", flat(&r.after)), start..start + text.len())
}

// --- The find bar and the menu on a browser pane (13x3 Task 4) ---

/// What a find-family menu command does on a browser pane (`PT:15326-15348`, `PT:14162-14175`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Open,
    Next,
    Previous,
    Clear,
    Jump,
    FindUrls,
    /// Use Selection for Find (`BVC:778-791`): the page's selection becomes the shared find string.
    UseSelection,
    /// Pick Result to Open (`BVC:1429-1435`): the open shortcuts over the current results.
    PickResult,
    /// Find Globally is its own window (Task 6).
    Global,
    /// Disabled for a browser session, or a text-view action that never reaches a web view.
    Nothing,
}

/// The selectors of the find family: the commands `route` rules on. Anything else is not the find
/// bar's business.
pub fn is_find_family(selector: &str) -> bool {
    matches!(
        selector,
        "performFindPanelAction:" | "findNext:" | "findPrevious:" | "clearFindString:" | "filter:" | "jumpToSelection:" | "findUrls:" | "findGlobally:" | "zoomOnSelection:" | "zoomOut:" | "openSelection:" | "smartSelectAllVisible:" | "revealContentNavigationShortcuts:"
    )
}

/// The find family on a browser pane. Filter and Find Cursor are disabled for a browser session
/// (`PT:14162-14175`); Select Matches, Zoom, Open Selection and Find All Smart Selection Matches
/// are text-view actions the web view never gets. Use Selection for Find and Pick Result to Open
/// are the browser view controller's own (`BVC:778-791`, `BVC:1429-1435`; the plan's Resume note
/// said otherwise and the source wins).
pub fn route(selector: &str, label: &str) -> Route {
    match (selector, label) {
        ("performFindPanelAction:", "Find...") => Route::Open,
        ("findNext:", _) => Route::Next,
        ("findPrevious:", _) => Route::Previous,
        ("clearFindString:", _) => Route::Clear,
        ("jumpToSelection:", _) => Route::Jump,
        ("findUrls:", _) => Route::FindUrls,
        ("performFindPanelAction:", "Use Selection for Find") => Route::UseSelection,
        ("revealContentNavigationShortcuts:", _) => Route::PickResult,
        ("findGlobally:", _) => Route::Global,
        _ => Route::Nothing,
    }
}

/// The find string Use Selection for Find sets (`BVC:781-789`): the page's selection, escaped for
/// a regex when the find mode is one. None for an empty selection (iTerm2 has no selection then).
pub fn selection_query(raw: &str, regex_mode: bool) -> Option<String> {
    if raw.is_empty() {
        return None;
    }
    Some(if regex_mode { regex::escape(raw) } else { raw.to_string() })
}

/// Return in the bar always searches forward in a browser, with or without Shift (`PS:9158-9160`,
/// `FD:671-675`).
pub fn enter_forward() -> bool {
    true
}

/// The pane's web area cut into the bar's row and the view below it. A bar taller than the area
/// leaves a view of height 0, never negative; the row is cut to what there is.
pub fn strip(web: Rect, bar_h: f32) -> (Rect, Rect) {
    let bar = bar_h.max(0.0).min(web.h.max(0.0));
    (Rect { h: bar, ..web }, Rect { y: web.y + bar, h: (web.h - bar).max(0.0), ..web })
}

/// A browser pane's whole top split in one place (13x3 B1): the front announcement bar first (its
/// bottom edge, when one is up), then the find bar's row of `find_h` below it (when one is open),
/// then the page. The find row is None while no bar is open. The app's `browser_split` calls this
/// and nothing else cuts these strips, so the placed view, the hover anchor and the scene agree.
pub fn split(r: Rect, announcement_bottom: Option<f32>, find_h: Option<f32>) -> (Option<Rect>, Rect) {
    let below = match announcement_bottom {
        Some(bottom) => {
            let (y, h) = crate::browser_tip::below_bar(r.y, r.h, bottom);
            Rect { y, h, ..r }
        }
        None => r,
    };
    match find_h {
        Some(h) => {
            let (bar, view) = strip(below, h);
            (Some(bar), view)
        }
        None => (None, below),
    }
}

/// iTerm2's one-at-a-time command mutex (`FM:270`): commands wait here, the next goes out when the
/// previous was answered.
#[derive(Clone, Debug, Default)]
pub struct Outbox {
    queue: std::collections::VecDeque<Value>,
    busy: bool,
}

impl Outbox {
    /// Queues `commands` in order; the one to send now, if nothing is in flight.
    pub fn push(&mut self, commands: Vec<Value>) -> Option<Value> {
        self.queue.extend(commands);
        self.next_out()
    }

    /// The answer to the command in flight came: the next one to send, if any.
    pub fn answered(&mut self) -> Option<Value> {
        self.busy = false;
        self.next_out()
    }

    fn next_out(&mut self) -> Option<Value> {
        if self.busy {
            return None;
        }
        let c = self.queue.pop_front()?;
        self.busy = true;
        Some(c)
    }

    /// A new world (the secret changed): nothing queued or in flight is the new page's.
    pub fn reset(&mut self) {
        *self = Outbox::default();
    }
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

    /// A file's text as it sits in the script: its two slots filled, and find-engine's global click
    /// typed (R13x3-1), find-nav's keys guarded (R13x3-2) and its engine lookup reachable (probe run
    /// 2026-10-09).
    fn filled(name: &str, secret: &str) -> String {
        let t = browser_js::resource(name).unwrap().replace("{{SECRET}}", secret).replace("{{SCROLL_BEHAVIOR}}", "smooth");
        match name {
            "find-engine.js" => type_global_click(&t).unwrap(),
            "find-nav.js" => type_nav_keys(&t).and_then(|t| type_nav_engine(&t)).unwrap(),
            _ => t,
        }
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

    /// R13x3-1: the forged-click coordinates are typed in the assembled script, and an iTerm2 text
    /// that no longer holds the target exactly once fails closed.
    #[test]
    fn the_global_click_is_typed() {
        let s = find_script("abc").expect("the script assembles");
        assert_eq!(s.matches(CLICK_TYPED).count(), 1, "the typed text is in the script once");
        assert_eq!(s.matches("handleClickInFrame(${x}, ${y}, '${this.instanceId}')").count(), 0, "the raw interpolation is gone");
        assert_eq!(s.matches("const x = Number(__tandemAt.x);").count(), 1);
        assert!(s.contains("if (!Number.isFinite(x) || !Number.isFinite(y))"));
        // The copied file itself is unchanged and holds the target once.
        assert_eq!(browser_js::resource("find-engine.js").unwrap().matches(CLICK_RAW).count(), 1);
        // Fails closed: the target missing, or there twice.
        assert!(type_global_click("no target here").is_err(), "a missing target is an error");
        assert!(type_global_click(&format!("{CLICK_RAW}\n{CLICK_RAW}")).is_err(), "a doubled target is an error");
        assert_eq!(type_global_click(&format!("a{CLICK_RAW}b")), Ok(format!("a{CLICK_TYPED}b")));
    }


    /// Find URLs (`PT:3872-3905`): a forced case-sensitive regex search that the bar does not
    /// know, then the open shortcuts once, only when something matched.
    #[test]
    fn find_urls_follow_iterm2() {
        let mut m = Model::new("U", "V");
        let c = m.temporary("https?://\\S+", "s").expect("a pattern is searched");
        assert_eq!(c, vec![command("startFind", json!({"searchTerm": "https?://\\S+", "contextLength": 0, "searchMode": "caseSensitiveRegex"}), "main_U", "s")]);
        assert!(m.shortcuts_after_complete("s").is_none(), "nothing before the result");
        assert!(m.on_message(&full("main_U", 2, 1, vec![ident(0, "a"), ident(1, "b")], None), "s") != Update::Ignored);
        let c = m.shortcuts_after_complete("s").expect("shortcuts after the full result");
        assert_eq!(c, vec![command("startNavigationShortcuts", json!({"selectionAction": "open"}), "main_U", "s")]);
        assert!(m.shortcuts_after_complete("s").is_none(), "once");
        // No match: nothing shows (the engine would answer "No matches to navigate").
        m.temporary("zzz", "s").unwrap();
        m.on_message(&full("main_U", 0, 0, vec![], None), "s");
        assert!(m.shortcuts_after_complete("s").is_none());
        // A global hide owed goes first.
        let mut m = Model::new("U", "V");
        m.global_start("q", FindMode::CaseInsensitive, "s");
        let c = m.temporary("u", "s").unwrap();
        assert_eq!(c.iter().map(cmd).collect::<Vec<_>>(), [("hideResults", "global_V"), ("startFind", "main_U")]);
        // A search the person starts after it takes the shortcuts away.
        let mut m = Model::new("U", "V");
        m.temporary("u", "s").unwrap();
        m.start("typed", FindMode::CaseSensitive, false, "s");
        m.on_message(&full("main_U", 1, 1, vec![ident(0, "a")], None), "s");
        assert!(m.shortcuts_after_complete("s").is_none(), "the person's own search is not Find URLs");
        // The pattern is page-bound: empty and over-long ones are refused.
        // Key action 36's search owes no open shortcuts when its result is in.
        let mut k = Model::new("U", "V");
        k.temporary_search("u", "s").unwrap();
        k.on_message(&full("main_U", 2, 1, vec![ident(0, "a"), ident(1, "b")], None), "s");
        assert!(k.shortcuts_after_complete("s").is_none());
        assert!(m.temporary("", "s").is_none());
        assert!(m.temporary(&"a".repeat(1001), "s").is_none());
        assert!(m.temporary(&"a".repeat(1000), "s").is_some());
        // The page counts UTF-16 units (`sanitizeString` slices them): 501 astral characters are 1002.
        assert!(m.temporary(&"\u{1F600}".repeat(501), "s").is_none());
        assert!(m.temporary(&"\u{1F600}".repeat(500), "s").is_some());
    }

    /// Find URLs puts bubbles on the page and the person's keys pick one: find-nav ignores a key
    /// that is not trusted (the copied file is not edited; the assembled script is).
    /// Probe run 2026-10-09 (find_urls_show_bubbles: 0 bubbles): find-nav's bubble script, run in
    /// each frame's Tandem world by evaluateInAll, reached the engine with `eval('getEngine')`, a
    /// name local to custom-find.js's IIFE, so it threw "getEngine is not defined" and no bubble was
    /// made. The assembled script reaches it through the frame's own `window.iTermCustomFind`.
    #[test]
    fn find_nav_bubbles_reach_the_engine() {
        let s = find_script("abc").expect("the script assembles");
        assert!(!s.contains("eval('getEngine')"), "the IIFE-local name is not reachable from evaluateInAll");
        assert_eq!(s.matches("engine = window.iTermCustomFind.getEngine(instanceId);").count(), 1);
        assert!(browser_js::resource("find-api.js").unwrap().contains("\n    getEngine\n};"), "the frame's API exposes getEngine");
        assert_eq!(browser_js::resource("find-nav.js").unwrap().matches(NAV_ENGINE_RAW).count(), 1, "the copy is unchanged");
        assert!(type_nav_engine("no target here").is_err());
        assert!(type_nav_engine(&format!("{NAV_ENGINE_RAW}{NAV_ENGINE_RAW}")).is_err());
        assert_eq!(type_nav_engine(&format!("a{NAV_ENGINE_RAW}b")), Ok(format!("a{NAV_ENGINE_TYPED}b")));
    }

    #[test]
    fn find_nav_ignores_untrusted_keys() {
        let s = find_script("abc").expect("the script assembles");
        assert_eq!(s.matches(NAV_TYPED).count(), 1);
        assert!(s.contains("if (!event.isTrusted) {"));
        assert!(!s.contains(NAV_RAW), "the raw listener is gone");
        assert_eq!(browser_js::resource("find-nav.js").unwrap().matches(NAV_RAW).count(), 1, "the copy is unchanged");
        assert!(type_nav_keys("no target here").is_err());
        assert!(type_nav_keys(&format!("{NAV_RAW}{NAV_RAW}")).is_err());
        assert_eq!(type_nav_keys(&format!("a{NAV_RAW}b")), Ok(format!("a{NAV_TYPED}b")));
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
        let c = m.start("a", FindMode::CaseSensitive, false, "s");
        assert_eq!(c.len(), 1, "a fresh search starts");
        assert_eq!(c[0], json!({"action": "startFind", "searchTerm": "a", "contextLength": 0, "searchMode": "caseSensitive", "instanceId": "main_U", "sessionSecret": "s"}));
        assert_eq!(m.active_term(), Some("a"));
        assert!(m.in_progress());
        assert_eq!(m.start("a", FindMode::CaseSensitive, false, "s"), Vec::<Value>::new(), "FM:471-474");
        assert_eq!(m.start("a", FindMode::CaseSensitive, true, "s").len(), 1, "forced");
        assert_eq!(m.next("s"), vec![command("findNext", json!({}), "main_U", "s")]);
        assert_eq!(m.previous("s"), vec![command("findPrevious", json!({}), "main_U", "s")]);
        let c = m.start("", FindMode::CaseSensitive, false, "s");
        assert_eq!(c.iter().map(cmd).collect::<Vec<_>>(), [("clearFind", "main_U")], "an empty term clears");
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
        assert_eq!(s.iter().map(cmd).collect::<Vec<_>>(), [("hideResults", "global_V"), ("startFind", "main_U")], "the hide first");
        assert_eq!(m.next("s"), vec![command("findNext", json!({}), "main_U", "s")], "the hide is sent once");
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

    /// Task 2 review I1: `start` returns its commands in the order they are sent, the owed global
    /// `hideResults` first, and the hide alone when the term is unchanged.
    #[test]
    fn start_sends_the_owed_hide_first() {
        let hide = command("hideResults", json!({}), "global_V", "s");
        let mut m = Model::new("U", "V");
        m.global_start("q", FindMode::CaseInsensitive, "s");
        let c: Vec<Value> = m.start("a", FindMode::CaseSensitive, false, "s").into_iter().collect();
        assert_eq!(c.iter().map(cmd).collect::<Vec<_>>(), [("hideResults", "global_V"), ("startFind", "main_U")]);
        assert!(m.start("a", FindMode::CaseSensitive, false, "s").into_iter().next().is_none(), "unchanged and nothing owed: nothing");
        // A reveal shows the global results again; the unchanged term still sends the owed hide.
        m.global_reveal(&json!({"index": 0}), "s");
        let c: Vec<Value> = m.start("a", FindMode::CaseSensitive, false, "s").into_iter().collect();
        assert_eq!(c, vec![hide.clone()], "the hide alone");
        assert_eq!(m.next("s"), vec![command("findNext", json!({}), "main_U", "s")], "sent once");
        // Forced: hide, then the start.
        m.global_reveal(&json!({"index": 0}), "s");
        let c: Vec<Value> = m.start("a", FindMode::CaseSensitive, true, "s").into_iter().collect();
        assert_eq!(c.iter().map(cmd).collect::<Vec<_>>(), [("hideResults", "global_V"), ("startFind", "main_U")]);
    }

    /// Task 2 review minors: foreign instance ids, a late update after a clear, counts, the label,
    /// the snippet's match text, and the identifier a reveal hands back.
    #[test]
    fn task_2_review_minors() {
        let mut m = Model::new("U", "V");
        m.start("a", FindMode::CaseSensitive, false, "s");
        // 1. Only the two instance ids are read.
        for id in ["main_X", "", "global_U", "global_V"] {
            assert_eq!(m.on_message(&full(id, 2, 1, vec![ident(0, "a")], None), "s"), Update::Ignored, "{id:?}");
        }
        let mut no_id = full("main_U", 2, 1, vec![ident(0, "a")], None);
        no_id["data"].as_object_mut().unwrap().remove("instanceId");
        assert_eq!(m.on_message(&no_id, "s"), Update::Ignored, "no instance id");
        assert_eq!(m.total(), 0);
        // Counts: held to MAX_COUNT, a float or a negative is 0, a u64 past i64 is held too.
        for (raw, want) in [(json!(200_000_000u64), MAX_COUNT), (json!(2.5), 0), (json!(u64::MAX), MAX_COUNT), (json!(-3), 0)] {
            let mut b = full("main_U", 0, 0, vec![ident(0, "a")], None);
            b["data"]["totalMatches"] = raw.clone();
            m.on_message(&b, "s");
            assert_eq!(m.total(), want, "{raw}");
        }
        // 5. A late update after a clear does not bring the results back.
        m.clear("s");
        assert_eq!(m.on_message(&full("main_U", 3, 1, vec![ident(0, "a")], None), "s"), Update::Ignored);
        let partial = json!({"sessionSecret": "s", "action": "currentChanged", "data": {"instanceId": "main_U", "totalMatches": 3, "currentMatch": 2}});
        assert_eq!(m.on_message(&partial, "s"), Update::Ignored);
        assert_eq!((m.total(), m.current()), (0, 0));
        // 2. The label never wraps.
        assert_eq!(label(1, 5, false), "0/1");
        // 3. The match text's newlines are spaces too.
        let r = FindResult { index: 0, identifier: json!({}), text: Some("h\ni".into()), before: Some("a ".into()), after: None };
        assert_eq!(snippet(&r), ("a h i".to_string(), 2..5));
        // 4. A reveal hands back only an identifier the model would keep.
        assert!(m.global_reveal(&json!("x"), "s").is_empty(), "not an object");
        assert!(m.global_reveal(&json!({"c": "y".repeat(MAX_IDENTIFIER_JSON)}), "s").is_empty(), "too big");
        assert_eq!(m.global_reveal(&json!({"index": 1}), "s").len(), 2);
    }

    #[test]
    fn snippet_follows_iterm2() {
        let r = |t: &str, b: Option<&str>, a: Option<&str>| FindResult { index: 0, identifier: json!({}), text: Some(t.into()), before: b.map(Into::into), after: a.map(Into::into) };
        assert_eq!(snippet(&r("hit", Some("a\nb "), Some(" c\nd"))), ("a b hit c d".to_string(), 4..7), "GSEC:486-487");
        assert_eq!(snippet(&r("hit", None, None)), ("hit".to_string(), 0..3));
        let none = FindResult { index: 0, identifier: json!({}), text: None, before: Some("x".into()), after: None };
        assert_eq!(snippet(&none), ("x".to_string(), 1..1));
    }

    #[test]
    fn browser_find_routes_follow_iterm2() {
        assert_eq!(route("performFindPanelAction:", "Find..."), Route::Open);
        assert_eq!(route("findNext:", "Find Next"), Route::Next);
        assert_eq!(route("findPrevious:", "Find Previous"), Route::Previous);
        assert_eq!(route("clearFindString:", "Clear"), Route::Clear);
        assert_eq!(route("jumpToSelection:", "Jump to Selection"), Route::Jump);
        assert_eq!(route("findUrls:", "Find URLs"), Route::FindUrls);
        assert_eq!(route("findGlobally:", "Find Globally..."), Route::Global);
        assert_eq!(route("performFindPanelAction:", "Use Selection for Find"), Route::UseSelection, "BVC:778-791");
        assert_eq!(route("revealContentNavigationShortcuts:", "Pick Result to Open"), Route::PickResult, "BVC:1429-1435");
        assert_eq!(route("filter:", "Filter"), Route::Nothing);
        assert_eq!(selection_query("a.b*", false), Some("a.b*".to_string()));
        assert_eq!(selection_query("a.b*", true), Some("a\\.b\\*".to_string()), "a regex mode escapes");
        assert_eq!(selection_query("", true), None);
        for (s, l) in [
            ("performFindPanelAction:", "Select Matches"),
            ("zoomOnSelection:", ""),
            ("zoomOut:", ""),
            ("openSelection:", ""),
            ("smartSelectAllVisible:", ""),
        ] {
            assert_eq!(route(s, l), Route::Nothing, "{s} {l}");
        }
        assert!(enter_forward());
        // Every selector the table rules on is in the family, so nothing falls through to the terminal.
        for s in ["performFindPanelAction:", "findNext:", "findPrevious:", "clearFindString:", "filter:", "jumpToSelection:", "findUrls:", "zoomOnSelection:", "openSelection:", "smartSelectAllVisible:", "revealContentNavigationShortcuts:", "findGlobally:"] {
            assert!(is_find_family(s), "{s}");
        }
        assert!(!is_find_family("copy:") && !is_find_family("findCursor:"));
    }

    #[test]
    fn find_strip_follows_windows() {
        let web = Rect { x: 0.0, y: 32.0, w: 800.0, h: 600.0 };
        let (bar, view) = strip(web, 24.0);
        assert_eq!(bar, Rect { x: 0.0, y: 32.0, w: 800.0, h: 24.0 });
        assert_eq!(view, Rect { x: 0.0, y: 56.0, w: 800.0, h: 576.0 });
        let (bar, view) = strip(Rect { x: 0.0, y: 32.0, w: 800.0, h: 10.0 }, 24.0);
        assert_eq!((bar.h, view.h), (10.0, 0.0), "a bar taller than the area leaves no view, never a negative one");
        assert!(view.y >= 32.0);
    }

    /// 13x3 B1, made behavioural (Task 4 review): the announcement bar, the find row and the page
    /// stack in that order from one function, so the placed view (this rectangle) is the one the
    /// hover anchor and the scene read; the rows never overlap and never leave the area.
    #[test]
    fn announcement_and_find_strips_stack_once() {
        let r = Rect { x: 10.0, y: 32.0, w: 800.0, h: 600.0 };
        // Nothing open: the page is the whole area.
        assert_eq!(split(r, None, None), (None, r));
        // Find alone.
        let (find, view) = split(r, None, Some(24.0));
        assert_eq!(find, Some(Rect { h: 24.0, ..r }));
        assert_eq!(view, Rect { y: 56.0, h: 576.0, ..r });
        // The announcement bar alone (it reaches y = 62).
        let (find, view) = split(r, Some(62.0), None);
        assert_eq!((find, view), (None, Rect { y: 62.0, h: 570.0, ..r }));
        // Both: the find row starts where the bar ends, the page where the row ends.
        let (find, view) = split(r, Some(62.0), Some(24.0));
        let find = find.expect("a find row");
        assert_eq!((find.y, find.h), (62.0, 24.0));
        assert_eq!((view.y, view.h), (86.0, 546.0));
        assert_eq!(find.y + find.h, view.y);
        assert_eq!(view.y + view.h, r.y + r.h, "nothing is lost at the bottom");
        // A bar reaching past the area leaves no page and no negative height.
        let (find, view) = split(r, Some(900.0), Some(24.0));
        assert_eq!((find.map(|f| f.h), view.h), (Some(0.0), 0.0));
    }

    /// iTerm2's mutex (`FM:270`): the second command waits for the first's answer.
    #[test]
    fn commands_go_out_one_at_a_time() {
        let mut o = Outbox::default();
        assert_eq!(o.push(vec![json!(1), json!(2)]), Some(json!(1)));
        assert_eq!(o.push(vec![json!(3)]), None, "one is in flight");
        assert_eq!(o.answered(), Some(json!(2)));
        assert_eq!(o.answered(), Some(json!(3)));
        assert_eq!(o.answered(), None);
        assert_eq!(o.push(vec![json!(4)]), Some(json!(4)), "idle again");
        o.push(vec![json!(5)]);
        o.reset();
        assert_eq!(o.answered(), None, "a new world has nothing queued");
    }
}

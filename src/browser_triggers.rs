//! iTerm2's browser triggers, the host side (milestone 13x5, Task 9): the list a page is given, the
//! `setTriggers` call, the match events a page posts, parameter resolution and the action-to-effect
//! table (`iTermBrowserTriggerHandler.swift` and the `*BrowserTrigger.swift` files). Pure: the
//! app side (`app/browser_triggers_app.rs`) applies what is decided here.
//!
//! Everything a page says is untrusted. The engine runs in the isolated world; a message is read only
//! after the secret matched; every count and string is bounded here before the app sees it; the
//! only text that reaches a script as source is a generated secret, a constant, or the person's own
//! stored Inject Javascript text, and the captures of a match are data (`inject_body`). Every other value (the trigger list, a match id, a color) goes in as
//! a typed argument (`browser_marks::Call`).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::browser_js;
use crate::browser_marks::{Call, patch_once};
use crate::triggers::{self, Trigger};

/// `iTermBrowserTriggerHandler.messageHandlerName` (`BTH:18`).
pub const MESSAGE_HANDLER: &str = "iTerm2Trigger";

/// The id of the document-start script (`BM:350-358`).
pub const SCRIPT_ID: &str = "TriggerHandler";

/// Most matches one message may carry that the host acts on (B8). The page's own cap (the patched
/// `triggers.js`) and the bridge cap are the same number; a message is cut here too.
pub const MAX_MATCHES: usize = 64;
/// Longest capture string kept, in bytes (B8): a capture goes into a parameter.
pub const MAX_CAPTURE: usize = 4096;
/// Most captures kept per list.
const MAX_CAPTURES: usize = 100;
/// Longest identifier or match id the host reads.
const MAX_ID: usize = 128;
/// Largest Inject Javascript text run in a page (the stored text is 64 KiB at most, plus captures).
pub const MAX_INJECT: usize = 256 * 1024;
/// Effects a pane may run in one window (B8), and the window.
pub const EFFECTS_PER_WINDOW: usize = 256;
pub const EFFECT_WINDOW: Duration = Duration::from_secs(10);
/// The least time between two Reload After Delay effects of a pane: a delay of 0 on a URL trigger would
/// otherwise reload as fast as pages load.
pub const RELOAD_GAP: Duration = Duration::from_secs(10);
/// Longest pattern a page is given (the profile editor allows `triggers::MAX_PATTERN`).
const MAX_PATTERN: usize = triggers::MAX_PATTERN;

// --- The list ---

/// A pane's browser triggers: the secret of the document-start script, the browser-action triggers as
/// the profile stored them (`source`, disabled ones included) and the same under this load's fresh
/// identifiers (`triggers`, `BTH:184-194`).
#[derive(Clone, Debug)]
pub struct Loaded {
    pub secret: String,
    pub source: Vec<Trigger>,
    pub triggers: Vec<(String, Trigger)>,
}

fn browser_only(list: &[Trigger]) -> Vec<Trigger> {
    list.iter().filter(|t| triggers::is_browser_action(&t.action)).take(triggers::MAX_TRIGGERS).cloned().collect()
}

/// `loadTriggersFromSettings` (`BTH:184-194`): the browser actions of `list`, each under a fresh uuid.
pub fn load(list: &[Trigger], secret: String) -> Loaded {
    let source = browser_only(list);
    let triggers = source.iter().map(|t| (uuid::Uuid::new_v4().to_string(), t.clone())).collect();
    Loaded { secret, source, triggers }
}

/// Whether the profile's list changed in the part this engine reads (`BTH:34-36`: iTerm2 reloads only
/// when the profile's `Triggers` value changes, so an unrelated edit sends nothing): the browser
/// actions of `list`, in order, differ from `l.source` by their stored tables.
pub fn needs_reload(l: &Loaded, list: &[Trigger]) -> bool {
    let now = browser_only(list);
    now.len() != l.source.len() || now.iter().zip(&l.source).any(|(a, b)| a.to_table() != b.to_table())
}

/// A pattern that can take exponential time in a backtracking engine: a group that holds an unbounded
/// quantifier and is itself quantified (`(a+)+`, `(\w+\s*)*`, `(a*){2,}`). A conservative scan; the page
/// runs JavaScript's `RegExp`, which has no step budget, so such a pattern is not given to a page.
pub fn risky_pattern(p: &str) -> bool {
    let b: Vec<char> = p.chars().collect();
    // One flag per open group: does it hold an unbounded quantifier.
    let mut stack: Vec<bool> = Vec::new();
    let mut i = 0;
    let mut in_class = false;
    let unbounded_at = |i: usize| -> Option<usize> {
        // A quantifier starting at i that can repeat without bound: (length of the quantifier, true).
        match b.get(i)? {
            '*' | '+' => Some(1),
            '{' => {
                let end = b[i..].iter().position(|c| *c == '}')? + i;
                let body: String = b[i + 1..end].iter().collect();
                let (lo, hi) = body.split_once(',').map_or((body.as_str(), Some(body.as_str())), |(l, h)| (l, Some(h)));
                let numeric = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
                if !numeric(lo) {
                    return None;
                }
                match hi {
                    // {n,} is unbounded, {n,m} with m > 1 repeats.
                    Some("") => Some(end - i + 1),
                    Some(h) if numeric(h) && h.parse::<u64>().unwrap_or(u64::MAX) > 1 => Some(end - i + 1),
                    _ => None,
                }
            }
            _ => None,
        }
    };
    while i < b.len() {
        match b[i] {
            '\\' => {
                i += 2;
                continue;
            }
            '[' if !in_class => in_class = true,
            ']' if in_class => in_class = false,
            _ if in_class => {}
            '(' => stack.push(false),
            ')' => {
                let inner = stack.pop().unwrap_or(false);
                if inner && unbounded_at(i + 1).is_some() {
                    return true;
                }
                // A group that is itself repeated holds an unbounded quantifier for its parent.
                let repeated = unbounded_at(i + 1).is_some();
                if let Some(top) = stack.last_mut() {
                    *top |= inner || repeated;
                }
            }
            '*' | '+' | '{' => {
                if let Some(n) = unbounded_at(i) {
                    if let Some(top) = stack.last_mut() {
                        *top = true;
                    }
                    i += n;
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }
    false
}

/// Whether a trigger is given to the page: enabled, a known match type, patterns of a sane length and
/// not catastrophic (`risky_pattern`).
fn sendable(t: &Trigger) -> bool {
    let content = t.extra.get("contentregex").and_then(|v| v.as_str()).unwrap_or("");
    t.enabled && (0..=triggers::PAGE_CONTENT_REGEX).contains(&t.match_type) && t.regex.len() <= MAX_PATTERN && content.len() <= MAX_PATTERN && !risky_pattern(&t.regex) && !risky_pattern(content)
}

/// The enabled triggers whose patterns were held back as catastrophic (to be told once).
pub fn held_back(l: &Loaded) -> usize {
    l.triggers.iter().filter(|(_, t)| t.enabled && (risky_pattern(&t.regex) || risky_pattern(t.extra.get("contentregex").and_then(|v| v.as_str()).unwrap_or("")))).count()
}

/// The enabled triggers as `currentTriggersJSON` (`BTH:145-151`): an object from identifier to the stored
/// dictionary, `performance` removed.
pub fn triggers_value(l: &Loaded) -> Value {
    let mut o = serde_json::Map::new();
    for (id, t) in &l.triggers {
        if !sendable(t) {
            continue;
        }
        let mut v = serde_json::to_value(t.to_table()).unwrap_or(Value::Null);
        if let Value::Object(m) = &mut v {
            m.remove("performance");
        }
        o.insert(id.clone(), v);
    }
    Value::Object(o)
}

pub fn triggers_json(l: &Loaded) -> String {
    triggers_value(l).to_string()
}

// --- Scripts ---

/// The engine as the document-start script: `triggers.js` with its secret and its two includes. Four
/// exact-once patches bound what a page can make it do (B8; each fails closed: no patch target, no
/// script): the page text it builds is cut at 1 MiB (also inside one text node), one check posts at most `MAX_MATCHES` matches and
/// stops scanning after 250 ms between matches, and at most 4096 matches are kept for the effects.
pub fn user_script(secret: &str) -> Result<String, String> {
    let src = browser_js::resource("triggers.js").ok_or("the trigger script is missing")?;
    let s = patch_once(src, "function walkNodes(node) {", "function walkNodes(node) {\n                if (fullText.length >= 1048576) { return; }")?;
    // The cap counts per text node too: one huge text node (a log, one `<pre>`) is cut at the append.
    let s = patch_once(&s, "fullText += node.textContent;", "fullText += node.textContent.slice(0, Math.max(0, 1048576 - fullText.length));")?;
    let s = patch_once(&s, "const matches = [];", "const matches = [];\n        const termurionDeadline = Date.now() + 250;")?;
    let s = patch_once(&s, "while ((contentMatch = globalContentRegex.exec(fullText)) !== null) {", "while (matches.length < 64 && Date.now() < termurionDeadline && (contentMatch = globalContentRegex.exec(fullText)) !== null) {")?;
    let s = patch_once(&s, "storedMatches[matchId] = matchData;", "if (matchIdCounter <= 4096) { storedMatches[matchId] = matchData; }")?;
    Ok(browser_js::fill_template(&s, &[("SECRET", secret)]))
}

/// `trigger-remove-highlighted-text.js` as iTerm2 reads it.
fn removal_script() -> &'static str {
    browser_js::resource("trigger-remove-highlighted-text.js").unwrap_or_default()
}

/// A secret as it is written between single quotes: a generated one holds only letters and digits;
/// anything else is dropped rather than escaped.
fn quoted_secret(secret: &str) -> String {
    secret.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect()
}

/// `sendTriggersToWebView`'s call (`BTH:158-174`): `iife` of the highlight removal inlined, then
/// `setTriggers`. The trigger list is the argument `triggers` (`set_triggers_args`), never part of this
/// text: the list holds imported regexes and parameters.
pub fn set_triggers_script(l: &Loaded) -> String {
    let call = format!(
        "try {{\n    {}\n    window.iTerm2Triggers.setTriggers(\n        {{sessionSecret: '{}', \n         triggers: triggers\n        }});\n}} catch(e) {{\n    console.error(e.toString());\n    throw e;\n}}",
        removal_script(),
        quoted_secret(&l.secret)
    );
    browser_js::iife(&call)
}

/// The runner call for `set_triggers_script`: the body returns null (`BTH:175-177`), the list is typed.
pub fn set_triggers_call(l: &Loaded) -> Call {
    Call { body: format!("{}\nreturn null;", set_triggers_script(l)), args: json!({"triggers": triggers_value(l)}) }
}

/// `BTH:106-117`, typed: the call with the match id and the two colors as arguments (`null` for none).
pub fn highlight_call(match_id: &str, text: Option<&str>, background: Option<&str>) -> Call {
    Call { body: format!("{}\nreturn null;", browser_js::iife(HIGHLIGHT_BODY)), args: json!({"matchId": match_id, "textColor": text, "backgroundColor": background}) }
}

/// `BTH:127-129`, typed.
pub fn hyperlink_call(match_id: &str, url: &str) -> Call {
    Call { body: format!("{}\nreturn null;", browser_js::iife(HYPERLINK_BODY)), args: json!({"matchId": match_id, "url": url}) }
}

const HIGHLIGHT_BODY: &str = "window.iTerm2Triggers?.highlightText(matchId, textColor, backgroundColor);";
const HYPERLINK_BODY: &str = "window.iTerm2Triggers?.makeHyperlink(matchId, url);";

/// iTerm2's `highlightText(...)` statement with the values written in, for reading and comparing only
/// (`BTH:106-117`); the app runs `highlight_call`.
pub fn highlight_script(match_id: &str, text: Option<&str>, background: Option<&str>) -> String {
    let lit = |s: Option<&str>| s.map_or("null".to_string(), browser_js::js_string);
    format!("window.iTerm2Triggers?.highlightText({}, {}, {});", browser_js::js_string(match_id), lit(text), lit(background))
}

/// Likewise for `makeHyperlink` (`BTH:127-129`).
pub fn hyperlink_script(match_id: &str, url: &str) -> String {
    format!("window.iTerm2Triggers?.makeHyperlink({}, {});", browser_js::js_string(match_id), browser_js::js_string(url))
}

/// `RBT:54-63` verbatim, with `delay` pasted: `effect` passes only a plain non-negative number.
pub fn reload_script(delay: &str) -> String {
    format!(
        "function reloadAfter(seconds) {{\n    if (typeof seconds !== \"number\" || seconds < 0) {{\n        throw new Error(\"seconds must be a non-negative number\");\n    }}\n    setTimeout(() => {{\n        window.location.reload();\n    }}, seconds * 1000);\n}}\nreloadAfter({delay});"
    )
}

// --- Messages ---

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    /// `urlRegex` or `pageContent`.
    pub match_type: String,
    pub url_captures: Vec<String>,
    pub content_captures: Option<Vec<String>>,
    pub identifier: String,
    pub match_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    Request,
    Matches(Vec<Match>),
    Bogus,
}

#[derive(Deserialize)]
struct RawEvent {
    matches: Vec<RawMatch>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawMatch {
    match_type: String,
    url_captures: Vec<String>,
    content_captures: Option<Vec<String>>,
    identifier: String,
    #[serde(rename = "matchID")]
    match_id: Option<String>,
}

/// Cuts `s` to at most `max` bytes at a character boundary.
fn cut(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

fn cut_all(v: Vec<String>) -> Vec<String> {
    v.into_iter().take(MAX_CAPTURES).map(|s| cut(&s, MAX_CAPTURE)).collect()
}

/// `handleMessage` (`BTH:73-96`): the secret must match, then a request or a match event. An unknown
/// `matchType` fails the decode as Swift's enum does. The result is bounded (B8): at most
/// `MAX_MATCHES` matches, each capture cut to `MAX_CAPTURE` bytes, a match with an over-long identifier
/// dropped.
pub fn message_of(body: &Value, secret: &str) -> Message {
    let Some(o) = body.as_object() else { return Message::Bogus };
    if o.get("sessionSecret").and_then(Value::as_str) != Some(secret) {
        return Message::Bogus;
    }
    if o.get("requestTriggers").and_then(Value::as_bool) == Some(true) {
        return Message::Request;
    }
    let Some(text) = o.get("matchEvent").and_then(Value::as_str) else { return Message::Bogus };
    let Ok(event) = serde_json::from_str::<RawEvent>(text) else { return Message::Bogus };
    let mut out = Vec::new();
    for m in event.matches.into_iter().take(MAX_MATCHES) {
        if !matches!(m.match_type.as_str(), "urlRegex" | "pageContent") {
            return Message::Bogus;
        }
        if m.identifier.len() > MAX_ID || m.match_id.as_ref().is_some_and(|i| i.len() > MAX_ID) {
            continue;
        }
        out.push(Match { match_type: m.match_type, url_captures: cut_all(m.url_captures), content_captures: m.content_captures.map(cut_all), identifier: m.identifier, match_id: m.match_id });
    }
    Message::Matches(out)
}

// --- Parameters and effects ---

/// The URL captures followed by the content captures (`RBT:48`).
pub fn captures(m: &Match) -> Vec<String> {
    m.url_captures.iter().chain(m.content_captures.iter().flatten()).cloned().collect()
}

const HIGHLIGHT: &str = "iTerm2SharedARC.HighlightBrowserTrigger";
const HYPERLINK: &str = "iTerm2SharedARC.HyperlinkBrowserTrigger";
const READER: &str = "iTerm2SharedARC.ReaderModeBrowserTrigger";
const INJECT_URL: &str = "InjectJavascriptURLTrigger";
const INJECT_CONTENT: &str = "InjectJavascriptContentTrigger";
const RELOAD: &str = "ReloadBrowserTrigger";
const ENTER: &str = "iTermEnterWorkgroupBrowserTrigger";
const EXIT: &str = "iTermExitWorkgroupBrowserTrigger";

/// Whether the action resolves its parameter against the captures (M10): Highlight's resolved text is
/// discarded (`HGT:120-130`), Enter Workgroup uses its id as stored (`EWB:106`), Reader Mode and Exit
/// Workgroup have no parameter.
///
/// Inject Javascript (both) is not resolved either (R13x5-10/13, a deviation from `IJT:48-54`): page
/// text is never spliced into the person's script. The match's captures reach the script as typed data
/// (`inject_body`: the `captures` array), so the capability stays and the source is a constant.
pub fn resolves_parameter(class: &str) -> bool {
    triggers::browser_action_info(class).is_some_and(|a| !matches!(a.class, HIGHLIGHT | ENTER | READER | EXIT | INJECT_URL | INJECT_CONTENT))
}

/// The page-world body for an Inject Javascript effect: the captures as a JSON array literal (data,
/// every `<` written `\u003c`, so no capture can end a string or a script), then the person's stored
/// script, unchanged, in iTerm2's `iife`.
pub fn inject_body(script: &str, captures: &[String]) -> String {
    let data = serde_json::to_string(captures).unwrap_or_else(|_| "[]".into()).replace('<', "\\u003c");
    format!("const captures = {data};\n{}\nreturn null;", browser_js::iife(script))
}

/// `paramWithBackreferencesReplaced(withValues:absLine: -1, ...)`: plain `\0`..`\8`, or an interpolated
/// string (`line` is -1) when `interpolated` and the parameter holds `\(`. `var` answers the variables
/// (`Screen::trigger_var(name, -1)`); Err is the evaluation error.
pub fn resolve(param: &str, caps: &[String], interpolated: bool, var: &dyn Fn(&str) -> Option<String>) -> Result<String, String> {
    let groups: Vec<Option<&str>> = caps.iter().map(|c| Some(c.as_str())).collect();
    if interpolated && param.contains("\\(") {
        crate::interp::evaluate(param, var, &groups)
    } else {
        Ok(triggers::substitute(param, &groups))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    Highlight { match_id: String, text: Option<String>, background: Option<String> },
    Hyperlink { match_id: String, url: String },
    /// Inject Javascript: the stored script (a constant) and the match's captures (data).
    Inject { script: String, captures: Vec<String> },
    /// Reload After Delay: its fixed script.
    Reload(String),
    EnterReaderMode,
    EnterWorkgroup(String),
    ExitWorkgroup,
    Stop,
}

/// A Reload After Delay delay: plain digits with an optional fraction, at most a day. Anything else is
/// skipped instead of being pasted into the page (a deviation: iTerm2 pastes the text and lets the
/// page throw, `RBT:54-63`; the text comes from page captures).
fn delay_ok(delay: &str) -> Option<&str> {
    let d = delay.trim();
    let (whole, frac) = d.split_once('.').map_or((d, ""), |(w, f)| (w, f));
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if whole.is_empty() || whole.len() > 5 || frac.len() > 3 || !digits(whole) || !digits(frac) || (d.contains('.') && frac.is_empty()) {
        return None;
    }
    let value: f64 = d.parse().ok()?;
    (value <= 86_400.0).then_some(d)
}

/// What a match does for trigger `t` (`HGT:111-134`, `HLT:42-63`, `IJT:41-117`, `RBT:41-73`,
/// `RMT:32-53`, `EWB:102-117`, `XWB:42-56`). `param` is the resolved text when `resolves_parameter`,
/// else the stored parameter. `workgroups` is (id, name) of the configured workgroups, for Enter
/// Workgroup's default (`EWB:53-63`).
pub fn effect(t: &Trigger, m: &Match, param: &str, workgroups: &[(String, String)]) -> Option<Effect> {
    let class = triggers::browser_action_info(&t.action)?.class;
    match class {
        HIGHLIGHT => {
            let match_id = m.match_id.clone()?;
            let (text, background) = triggers::browser_colors(param);
            let css = |c: Option<String>| c.and_then(|c| triggers::css_color(&c));
            Some(Effect::Highlight { match_id, text: css(text), background: css(background) })
        }
        HYPERLINK => {
            let match_id = m.match_id.clone()?;
            Some(Effect::Hyperlink { match_id, url: triggers::hyperlink_url_ok(param)? })
        }
        INJECT_URL | INJECT_CONTENT => (!param.trim().is_empty() && param.len() <= MAX_INJECT).then(|| Effect::Inject { script: param.to_string(), captures: captures(m) }),
        RELOAD => Some(Effect::Reload(reload_script(delay_ok(param)?))),
        READER => Some(Effect::EnterReaderMode),
        ENTER => triggers::effective_workgroup(param, workgroups).filter(|id| !id.is_empty() && id.len() <= triggers::MAX_WORKGROUP_ID).map(Effect::EnterWorkgroup),
        EXIT => Some(Effect::ExitWorkgroup),
        _ => None,
    }
}

// --- The pane's gate ---

/// Whether a trigger message may be acted on (B5): from the isolated world (the transport says so), from
/// the main frame, and neither the message's source nor the pane's page is one of Termurion's own
/// `iterm2-about:` pages (their text is chosen by hostile sites, and the page world there holds the
/// local-page bridge). A Client source is the context's origin, and Chromium gives an `iterm2-about:`
/// document an opaque one ("://", probe run 2026-10-10b), so `page_url` (the pane's URL) is what
/// refuses a local page; the source half is kept for a source that does name one.
pub fn accepted(world: browser_js::World, frame: Option<u64>, source: &str, page_url: &str) -> bool {
    world == browser_js::World::Client && frame.is_none() && !crate::browser::local_source_ok(source) && !crate::browser::local_source_ok(page_url)
}

/// Whether the list may be sent to the page showing now after a profile edit: never to one of Termurion's
/// own `iterm2-about:` pages (B5; the same rule as `accepted`, for the resend path).
pub fn resend_allowed(page_url: &str) -> bool {
    !crate::browser::local_source_ok(page_url)
}

/// A pane's engine state in its web view: the navigation the page's engine last asked in (a match is
/// acted on only for that page), and the rates (B8). Reset with the world.
#[derive(Debug, Default)]
pub struct Gate {
    served: Option<u64>,
    effects: VecDeque<Instant>,
    reload: Option<Instant>,
}

impl Gate {
    /// The page asked for its triggers in navigation epoch `epoch`.
    pub fn serve(&mut self, epoch: u64) {
        self.served = Some(epoch);
    }

    /// Whether the page showing now has asked for its triggers.
    pub fn is_served(&self) -> bool {
        self.served.is_some()
    }

    /// Whether a match belongs to the page now showing: the epoch is the one that was served. A
    /// navigation that began since makes it another page's.
    pub fn current(&self, epoch: u64) -> bool {
        self.served == Some(epoch)
    }

    /// Whether one more effect may run now: at most `EFFECTS_PER_WINDOW` in `EFFECT_WINDOW`.
    pub fn effect_allowed(&mut self, now: Instant) -> bool {
        while self.effects.front().is_some_and(|t| now.saturating_duration_since(*t) >= EFFECT_WINDOW) {
            self.effects.pop_front();
        }
        if self.effects.len() >= EFFECTS_PER_WINDOW {
            return false;
        }
        self.effects.push_back(now);
        true
    }

    /// Whether a Reload After Delay may be scheduled now (`RELOAD_GAP`).
    pub fn reload_allowed(&mut self, now: Instant) -> bool {
        if self.reload.is_some_and(|t| now.saturating_duration_since(t) < RELOAD_GAP) {
            return false;
        }
        self.reload = Some(now);
        true
    }

    pub fn reset(&mut self) {
        *self = Gate::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trig(action: &str, regex: &str, param: &str, match_type: i64, enabled: bool) -> Trigger {
        Trigger { regex: regex.into(), action: action.into(), parameter: param.into(), instant: false, enabled, name: String::new(), job: String::new(), match_type, extra: toml::Table::new() }
    }

    fn list() -> Vec<Trigger> {
        let mut h = trig(HIGHLIGHT, "a", "#ff0000;", 2, true);
        h.extra.insert("contentregex".into(), "b".into());
        let mut perf = toml::Table::new();
        perf.insert("x".into(), 1.into());
        h.extra.insert("performance".into(), toml::Value::Table(perf));
        vec![h, trig(RELOAD, "r", "5", 1, false), trig("BellTrigger", "bell", "", 0, true)]
    }

    #[test]
    fn triggers_json_follows_iterm2() {
        let l = load(&list(), "s".into());
        assert_eq!(l.triggers.len(), 2, "the terminal trigger is not loaded");
        let v: Value = serde_json::from_str(&triggers_json(&l)).unwrap();
        let o = v.as_object().unwrap();
        assert_eq!(o.len(), 1, "enabled only");
        let (key, t) = o.iter().next().unwrap();
        assert_eq!(key.len(), 36, "a uuid");
        assert_eq!((t["regex"].as_str(), t["contentregex"].as_str(), t["matchType"].as_i64()), (Some("a"), Some("b"), Some(2)));
        assert_eq!((t["action"].as_str(), t["parameter"].as_str(), t["disabled"].as_bool()), (Some(HIGHLIGHT), Some("#ff0000;"), Some(false)));
        assert!(t.get("performance").is_none());
        let again = load(&list(), "s".into());
        let key2 = serde_json::from_str::<Value>(&triggers_json(&again)).unwrap().as_object().unwrap().keys().next().unwrap().clone();
        assert_ne!(*key, key2, "fresh identifiers per load");
    }

    #[test]
    fn trigger_resend_follows_iterm2() {
        let l = load(&list(), "s".into());
        assert!(!needs_reload(&l, &list()));
        let mut more = list();
        more.push(trig("AlertTrigger", "x", "", 0, true));
        assert!(!needs_reload(&l, &more), "an unrelated terminal trigger");
        let mut changed = list();
        changed[0].regex = "c".into();
        assert!(needs_reload(&l, &changed));
        let mut enabled = list();
        enabled[1].enabled = true;
        assert!(needs_reload(&l, &enabled));
        let mut swapped = list();
        swapped.swap(0, 1);
        assert!(needs_reload(&l, &swapped));
    }

    #[test]
    fn set_triggers_script_follows_iterm2() {
        let l = load(&list(), "abc123".into());
        let s = set_triggers_script(&l);
        assert!(s.starts_with("(function() {try {") && s.ends_with("})();"), "{s}");
        assert!(s.contains("removeExistingHighlights") && s.contains("window.iTerm2Triggers.setTriggers(") && s.contains("{sessionSecret: 'abc123',") && s.contains("triggers: triggers"));
        assert_eq!(s.matches("(function() {").count(), 2, "iTerm2's one iife around the inlined removal script's own: no second wrap");
        assert!(!s.contains("\"regex\""), "the list is an argument, never source text");
        let c = set_triggers_call(&l);
        assert!(c.body.ends_with("\nreturn null;") && c.args["triggers"].as_object().unwrap().len() == 1);
        let u = user_script("sec-ret").unwrap();
        assert!(u.contains("const sessionSecret = \"sec-ret\";") && u.contains("api.highlightText") && u.contains("api.makeHyperlink") && !u.contains("{{"));
    }

    #[test]
    fn the_list_never_goes_to_a_local_page() {
        assert!(resend_allowed("https://a.example/") && resend_allowed(""));
        assert!(!resend_allowed("iterm2-about:history") && !resend_allowed("ITERM2-ABOUT:source"));
        assert!(!accepted(browser_js::World::Client, None, "iterm2-about:history", "https://a/"));
    }

    #[test]
    fn the_trigger_script_patches_are_exact_once() {
        let u = user_script("x").unwrap();
        for needle in ["fullText.length >= 1048576", "node.textContent.slice(0, Math.max(0, 1048576 - fullText.length))", "const termurionDeadline = Date.now() + 250;", "matches.length < 64 && Date.now() < termurionDeadline", "if (matchIdCounter <= 4096) { storedMatches[matchId] = matchData; }"] {
            assert_eq!(u.matches(needle).count(), 1, "{needle}");
        }
        // The copy on disk is iTerm2's: none of the patches is in it.
        let raw = include_str!("../resources/browser/triggers.js");
        assert!(!raw.contains("termurionDeadline") && !raw.contains("1048576"));
        // A moved target fails closed.
        assert!(patch_once("function walkNodes(node) {function walkNodes(node) {", "function walkNodes(node) {", "x").is_err());
    }

    #[test]
    fn match_events_follow_iterm2() {
        let ev = "{\"matches\":[{\"matchType\":\"pageContent\",\"urlCaptures\":[\"https://a\"],\"contentCaptures\":[\"foo\",\"o\"],\"identifier\":\"K\",\"matchID\":\"match_1\"}]}";
        assert_eq!(message_of(&json!({"sessionSecret": "s", "requestTriggers": true}), "s"), Message::Request);
        let m = Match { match_type: "pageContent".into(), url_captures: vec!["https://a".into()], content_captures: Some(vec!["foo".into(), "o".into()]), identifier: "K".into(), match_id: Some("match_1".into()) };
        assert_eq!(message_of(&json!({"sessionSecret": "s", "matchEvent": ev}), "s"), Message::Matches(vec![m]));
        let url = "{\"matches\":[{\"matchType\":\"urlRegex\",\"urlCaptures\":[\"u\"],\"identifier\":\"K\"}]}";
        let Message::Matches(v) = message_of(&json!({"sessionSecret": "s", "matchEvent": url}), "s") else { panic!() };
        assert_eq!((v[0].content_captures.clone(), v[0].match_id.clone()), (None, None));
        assert_eq!(message_of(&json!({"sessionSecret": "x", "requestTriggers": true}), "s"), Message::Bogus);
        assert_eq!(message_of(&json!({"sessionSecret": "s", "matchEvent": "{nope"}), "s"), Message::Bogus);
        let unknown = ev.replace("pageContent", "other");
        assert_eq!(message_of(&json!({"sessionSecret": "s", "matchEvent": unknown}), "s"), Message::Bogus);
        assert_eq!(message_of(&json!({"sessionSecret": "s", "requestTriggers": 1}), "s"), Message::Bogus);
        assert_eq!(message_of(&json!("text"), "s"), Message::Bogus);
    }

    #[test]
    fn messages_are_bounded() {
        let big = "x".repeat(10_000);
        let one = |i: usize| json!({"matchType": "pageContent", "urlCaptures": [big.clone()], "contentCaptures": [big.clone()], "identifier": format!("K{i}"), "matchID": "m"});
        let ev = json!({"matches": (0..200).map(one).collect::<Vec<_>>()}).to_string();
        let Message::Matches(v) = message_of(&json!({"sessionSecret": "s", "matchEvent": ev}), "s") else { panic!() };
        assert_eq!(v.len(), MAX_MATCHES);
        assert!(v.iter().all(|m| m.url_captures[0].len() == MAX_CAPTURE && m.content_captures.as_ref().unwrap()[0].len() == MAX_CAPTURE));
        // A capture is cut at a character boundary.
        assert_eq!(cut("\u{e9}\u{e9}", 3), "\u{e9}");
        let long_id = json!({"matches": [{"matchType": "urlRegex", "urlCaptures": [], "identifier": "i".repeat(500)}]}).to_string();
        assert_eq!(message_of(&json!({"sessionSecret": "s", "matchEvent": long_id}), "s"), Message::Matches(vec![]));
    }

    fn page_match() -> Match {
        Match { match_type: "pageContent".into(), url_captures: vec!["https://a".into()], content_captures: Some(vec!["foo".into(), "o".into()]), identifier: "K".into(), match_id: Some("match_1".into()) }
    }

    #[test]
    fn browser_parameters_follow_iterm2() {
        let m = page_match();
        assert_eq!(captures(&m), vec!["https://a", "foo", "o"]);
        assert_eq!(resolve("https://x/\\1", &captures(&m), false, &|_| None).unwrap(), "https://x/foo");
        let var = |n: &str| (n == "line").then(|| "-1".to_string());
        assert_eq!(resolve("\\(matches[2])", &captures(&m), true, &var).unwrap(), "o");
        assert_eq!(resolve("\\(line)", &captures(&m), true, &var).unwrap(), "-1");
        assert!(resolve("\\(1 +)", &captures(&m), true, &var).is_err());
        assert_eq!(resolve("\\(x)", &captures(&m), false, &var).unwrap(), "\\(x)", "plain when interpolation is off");
        for class in [HIGHLIGHT, ENTER, READER, EXIT, INJECT_URL, INJECT_CONTENT] {
            assert!(!resolves_parameter(class), "{class}");
        }
        for class in [HYPERLINK, RELOAD] {
            assert!(resolves_parameter(class), "{class}");
        }
        let t = trig(ENTER, "u", "w\\1", 1, true);
        assert_eq!(effect(&t, &m, &t.parameter, &[]), Some(Effect::EnterWorkgroup("w\\1".into())));
    }

    #[test]
    fn effects_follow_iterm2() {
        let m = page_match();
        let h = trig(HIGHLIGHT, "a", "#ff0000;p3#0000ffff0000", 2, true);
        assert_eq!(effect(&h, &m, &h.parameter, &[]), Some(Effect::Highlight { match_id: "match_1".into(), text: Some("#ff0000".into()), background: Some("color(display-p3 0.0000 1.0000 0.0000)".into()) }));
        let none = Match { match_id: None, ..m.clone() };
        assert_eq!(effect(&h, &none, &h.parameter, &[]), None);
        let hostile = trig(HIGHLIGHT, "a", "red;url(x);", 2, true);
        assert_eq!(effect(&hostile, &m, &hostile.parameter, &[]), Some(Effect::Highlight { match_id: "match_1".into(), text: None, background: None }), "no stored text but a color reaches a style");
        let link = trig(HYPERLINK, "a", "https://\\0", 2, true);
        assert_eq!(effect(&link, &m, "https://x/foo", &[]), Some(Effect::Hyperlink { match_id: "match_1".into(), url: "https://x/foo".into() }));
        for bad in ["javascript:alert(1)", "data:text/html,x", "file:///c:/x", "/relative", "https://a b/"] {
            assert_eq!(effect(&link, &m, bad, &[]), None, "{bad}");
        }
        let inj = trig(INJECT_CONTENT, "a", "x", 2, true);
        assert_eq!(effect(&inj, &m, "document.title='x'", &[]), Some(Effect::Inject { script: "document.title='x'".into(), captures: captures(&m) }));
        let injurl = trig(INJECT_URL, "a", "x", 1, true);
        assert_eq!(effect(&injurl, &m, "console.log(1)", &[]), Some(Effect::Inject { script: "console.log(1)".into(), captures: captures(&m) }));
        let rl = trig(RELOAD, "a", "5", 1, true);
        let Some(Effect::Reload(js)) = effect(&rl, &m, "5", &[]) else { panic!() };
        assert_eq!(js, reload_script("5"));
        assert!(js.starts_with("function reloadAfter(seconds) {\n    if (typeof seconds !== \"number\" || seconds < 0) {") && js.ends_with("reloadAfter(5);"));
        for bad in ["", "abc", "5);alert(1", "-1", "1e9", "99999999", "5.", "1.2345"] {
            assert_eq!(effect(&rl, &m, bad, &[]), None, "{bad:?}");
        }
        assert_eq!(effect(&rl, &m, " 2.5 ", &[]), Some(Effect::Reload(reload_script("2.5"))));
        // A capture is data: the script text is the stored one, and a hostile capture cannot end the literal.
        let hostile = Match { content_captures: Some(vec!["\");fetch(1);(\"</script>\\".into()]), ..m.clone() };
        let Some(Effect::Inject { script, captures: caps }) = effect(&inj, &hostile, "send(captures[0])", &[]) else { panic!() };
        assert_eq!(script, "send(captures[0])");
        let body = inject_body(&script, &caps);
        assert!(body.contains("send(captures[0])") && !body.contains("</script>") && !body.contains("fetch(1);(\"</"), "{body}");
        assert!(body.starts_with("const captures = [") && body.contains("\\\");fetch(1);(\\\"\\u003c/script>"), "{body}");
        assert!(!resolves_parameter(INJECT_CONTENT), "no \\0..\\9 or \\(...) substitution into the script");
        let rd = trig(READER, "a", "", 1, true);
        assert_eq!(effect(&rd, &m, "", &[]), Some(Effect::EnterReaderMode));
        let wg = vec![("id-b".to_string(), "Beta".to_string()), ("id-a".to_string(), "Alpha".to_string())];
        let en = trig(ENTER, "a", "w", 1, true);
        assert_eq!(effect(&en, &m, "w", &wg), Some(Effect::EnterWorkgroup("w".into())));
        assert_eq!(effect(&en, &m, "", &wg), Some(Effect::EnterWorkgroup("id-a".into())), "the first by name");
        assert_eq!(effect(&en, &m, "", &[]), None);
        let ex = trig(EXIT, "a", "", 1, true);
        assert_eq!(effect(&ex, &m, "", &[]), Some(Effect::ExitWorkgroup));
        assert_eq!(effect(&trig("BellTrigger", "a", "", 0, true), &m, "", &[]), None);
        // The statements iTerm2 builds, and their typed form.
        assert_eq!(highlight_script("m", None, Some("#fff")), "window.iTerm2Triggers?.highlightText(\"m\", null, \"#fff\");");
        assert_eq!(browser_js::iife(&highlight_script("m", None, Some("#fff"))), "(function() {window.iTerm2Triggers?.highlightText(\"m\", null, \"#fff\");})();");
        assert_eq!(hyperlink_script("m", "https://x/"), "window.iTerm2Triggers?.makeHyperlink(\"m\", \"https://x/\");");
        let c = highlight_call("m", None, Some("#fff"));
        assert!(c.body.contains("highlightText(matchId, textColor, backgroundColor)") && !c.body.contains("#fff") && !c.body.contains("\"m\""));
        assert_eq!(c.args, json!({"matchId": "m", "textColor": null, "backgroundColor": "#fff"}));
        let hc = hyperlink_call("m", "https://x/\");alert(1);(\"");
        assert!(!hc.body.contains("alert") && hc.args["url"] == "https://x/\");alert(1);(\"");
    }

    #[test]
    fn catastrophic_patterns_are_held_back() {
        for bad in ["(a+)+", "(a*)*b", "(\\w+\\s*)+$", "(a|b+)*c", "((a+))+", "(a+){2,}", "(x+x+)+y"] {
            assert!(risky_pattern(bad), "{bad}");
        }
        for ok in ["wor(ld)", "(foo|bar)+", "a+b+", "[(a+)+]", "\\(a+\\)+", "(\\d{3})-(\\d{4})", "(ab){2}", "^(https?://)?[a-z]+\\.test/"] {
            assert!(!risky_pattern(ok), "{ok}");
        }
        let l = load(&[trig(HIGHLIGHT, "(a+)+", "#fff;", 2, true), trig(HIGHLIGHT, "ok", "#fff;", 2, true)], "s".into());
        assert_eq!(triggers_value(&l).as_object().unwrap().len(), 1);
        assert_eq!(held_back(&l), 1);
        let long = trig(HIGHLIGHT, &"a".repeat(MAX_PATTERN + 1), "#fff;", 2, true);
        assert_eq!(triggers_value(&load(&[long], "s".into())).as_object().unwrap().len(), 0);
    }

    #[test]
    fn only_the_main_frame_of_a_web_page_is_heard() {
        use browser_js::World::{Client, Page};
        assert!(accepted(Client, None, "https://a.test", "https://a.test/x"));
        assert!(!accepted(Page, None, "https://a.test", "https://a.test/x"), "a page-world message never");
        assert!(!accepted(Client, Some(4), "https://a.test", "https://a.test/x"), "a frame never");
        assert!(!accepted(Client, None, "iterm2-about://history", "https://a.test/x"));
        assert!(!accepted(Client, None, "https://a.test", "iterm2-about:history"));
    }

    #[test]
    fn the_gate_binds_pages_and_paces_effects() {
        let mut g = Gate::default();
        assert!(!g.current(3), "nothing served yet");
        g.serve(3);
        assert!(g.current(3) && !g.current(4));
        let t0 = Instant::now();
        for i in 0..EFFECTS_PER_WINDOW {
            assert!(g.effect_allowed(t0), "{i}");
        }
        assert!(!g.effect_allowed(t0));
        assert!(g.effect_allowed(t0 + EFFECT_WINDOW));
        assert!(g.reload_allowed(t0) && !g.reload_allowed(t0 + Duration::from_secs(1)) && g.reload_allowed(t0 + RELOAD_GAP));
        g.reset();
        assert!(!g.current(3));
    }
}

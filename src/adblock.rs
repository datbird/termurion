//! Ad blocking and the browser proxy (milestone 13x, Task 8). Pure: iTerm2 feeds a WebKit
//! content-blocker JSON list to `WKContentRuleListStore` (`ABM:26-288`); WebView2 has no such
//! store, so Termurion compiles the same JSON to a matcher the app asks for every request
//! (`WebResourceRequested`) and for the element-hiding CSS of the top page. Everything a list or a
//! setting holds is untrusted: sizes are capped, parsing never panics, a regex is compiled only
//! from the restricted WebKit subset with a size limit, and a proxy host can never carry a
//! Chromium switch.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use regex::{Regex, RegexBuilder};
use serde_json::Value;

/// The refresh interval while a browser view exists (`ABM:33`).
pub const UPDATE_INTERVAL: Duration = Duration::from_secs(24 * 3600);
/// Days without a successful update after which the stale banner shows (`ABM:34`).
pub const STALE_DAYS: i64 = 7;
/// The global holding the last successful update (Unix seconds); iTerm2's name (`ABM:44`).
pub const LAST_UPDATE_KEY: &str = "NoSyncAdblockLastUpdate";
pub const PARSE_FAILED: &str = "Failed to parse adblock list response";
pub const NOT_JSON: &str = "Downloaded content is not valid JSON format";

/// The longest list accepted, downloaded or read back (P14).
pub const MAX_LIST_BYTES: usize = 8 << 20;
/// Rules kept from a list; the rest are dropped (P14).
pub const MAX_RULES: usize = 100_000;
const MAX_FILTER: usize = 1024;
const MAX_SELECTOR: usize = 512;
const MAX_CSS: usize = 512 << 10;
/// A request's matching time before the answer is "continue" (P14): a hostile list must not
/// freeze a page load on the UI thread.
const MATCH_BUDGET: Duration = Duration::from_millis(2);
const CSS_BUDGET: Duration = Duration::from_millis(25);
/// The longest part of a URL that is matched (and indexed): a URL can be megabytes long.
const MAX_HAYSTACK: usize = 4096;
/// The rules with no literal to index by are judged for every request, so only this many are kept.
const MAX_ALWAYS: usize = 256;
/// Regex program size limits: the rules judged for every request are built when the list compiles
/// (larger allowed), the rest when first needed (small: a cold build is cheap and bounded).
const EAGER_SIZE: usize = 128 << 10;
const LAZY_SIZE: usize = 32 << 10;
const DFA_SIZE: usize = 256 << 10;
const KEY_LEN: usize = 4;
const ALL_BUT_POPUP: u16 = 0x1FF;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resource {
    Document,
    Image,
    StyleSheet,
    Script,
    Font,
    Raw,
    SvgDocument,
    Media,
    Popup,
    Other,
}

impl Resource {
    fn bit(self) -> u16 {
        1 << (self as u16)
    }
}

/// WebView2's `COREWEBVIEW2_WEB_RESOURCE_CONTEXT` as a content-blocker resource type.
pub fn resource_of(webview2_kind: i32) -> Resource {
    match webview2_kind {
        1 => Resource::Document,
        2 => Resource::StyleSheet,
        3 => Resource::Image,
        4 => Resource::Media,
        5 => Resource::Font,
        6 => Resource::Script,
        // XMLHttpRequest, fetch, event source, web socket, manifest, ping, CSP report.
        7 | 8 | 10 | 11 | 12 | 14 | 15 => Resource::Raw,
        _ => Resource::Other,
    }
}

fn resource_named(name: &str) -> Option<Resource> {
    Some(match name {
        "document" => Resource::Document,
        "image" => Resource::Image,
        "style-sheet" => Resource::StyleSheet,
        "script" => Resource::Script,
        "font" => Resource::Font,
        "raw" => Resource::Raw,
        "svg-document" => Resource::SvgDocument,
        "media" => Resource::Media,
        "popup" => Resource::Popup,
        _ => return None,
    })
}

#[derive(Debug)]
enum Filter {
    /// `.*` and the like: matches every URL.
    Any,
    Pattern { text: String, sensitive: bool, re: OnceLock<Option<Regex>> },
}

fn build_regex(text: &str, sensitive: bool, size: usize) -> Option<Regex> {
    RegexBuilder::new(text).case_insensitive(!sensitive).size_limit(size).dfa_size_limit(DFA_SIZE).build().ok()
}

/// `s` cut to at most `max` bytes at a character boundary.
fn cut(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

impl Filter {
    /// Matches the URL's first `MAX_HAYSTACK` bytes; a regex not built yet is built small.
    fn matches(&self, url: &str) -> bool {
        match self {
            Filter::Any => true,
            Filter::Pattern { text, sensitive, re } => re
                .get_or_init(|| build_regex(text, *sensitive, LAZY_SIZE))
                .as_ref()
                .is_some_and(|r| r.is_match(cut(url, MAX_HAYSTACK))),
        }
    }

    /// Builds the regex now (on the worker). False when it cannot be built: the rule is dropped.
    fn prebuild(&self) -> bool {
        match self {
            Filter::Any => true,
            Filter::Pattern { text, sensitive, re } => re.get_or_init(|| build_regex(text, *sensitive, EAGER_SIZE)).is_some(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Party {
    Any,
    First,
    Third,
}

#[derive(Debug)]
struct Domain {
    name: String,
    subdomains: bool,
}

impl Domain {
    fn matches(&self, host: &str) -> bool {
        host == self.name || (self.subdomains && host.len() > self.name.len() && host.ends_with(&self.name) && host.as_bytes()[host.len() - self.name.len() - 1] == b'.')
    }
}

#[derive(Debug)]
enum Domains {
    Any,
    If(Vec<Domain>),
    Unless(Vec<Domain>),
}

impl Domains {
    fn allows(&self, top_host: &str) -> bool {
        match self {
            Domains::Any => true,
            Domains::If(l) => l.iter().any(|d| d.matches(top_host)),
            Domains::Unless(l) => !l.iter().any(|d| d.matches(top_host)),
        }
    }
}

#[derive(Debug)]
enum Action {
    Block,
    Ignore,
    Hide(String),
}

#[derive(Debug)]
struct Rule {
    filter: Filter,
    types: u16,
    party: Party,
    domains: Domains,
    action: Action,
}

/// A compiled list. The regexes of the rules judged for every request are built when it compiles
/// (on the worker); the others are built small on first use, and a literal index narrows each
/// request to a few candidates.
#[derive(Debug)]
pub struct Rules {
    rules: Vec<Rule>,
    index: HashMap<[u8; KEY_LEN], Vec<u32>>,
    always: Vec<u32>,
    hides: Vec<u32>,
}

impl Rules {
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

impl Rules {
    /// Whether every rule a request is always judged against has its regex built (they are built
    /// when the list compiles, on the worker, never on the UI thread).
    #[cfg(test)]
    fn always_prebuilt(&self) -> bool {
        self.always.iter().all(|&i| matches!(&self.rules[i as usize].filter, Filter::Pattern { re, .. } if re.get().is_some()) || matches!(&self.rules[i as usize].filter, Filter::Any))
    }
}

static OVER_BUDGET: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// How many requests were allowed unjudged because matching ran out of its budget (this run).
pub fn over_budget_count() -> usize {
    OVER_BUDGET.load(std::sync::atomic::Ordering::Relaxed)
}

/// Compiles WebKit content-blocker JSON (`ABM:96-115` validates the same shape).
pub fn compile(json: &str) -> Result<Rules, String> {
    if json.trim().is_empty() {
        return Err(PARSE_FAILED.to_string());
    }
    if json.len() > MAX_LIST_BYTES {
        return Err(format!("The adblock list is larger than {} MiB", MAX_LIST_BYTES >> 20));
    }
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(json) else { return Err(NOT_JSON.to_string()) };
    let mut rules = Vec::new();
    for item in items.iter().take(MAX_RULES) {
        if let Some(r) = parse_rule(item) {
            rules.push(r);
        }
    }
    // Each rule is indexed by the rarest four byte piece of its longest literal, so a thousand
    // rules sharing a prefix do not all become candidates for one request.
    let runs: Vec<Option<String>> = rules.iter().map(|r| match (&r.action, &r.filter) {
        (Action::Hide(_), _) | (_, Filter::Any) => None,
        (_, Filter::Pattern { text, .. }) => literal_run(text),
    }).collect();
    let mut count: HashMap<[u8; KEY_LEN], u32> = HashMap::new();
    for run in runs.iter().flatten() {
        let mut seen: Vec<[u8; KEY_LEN]> = grams(run).collect();
        seen.sort_unstable();
        seen.dedup();
        for g in seen {
            *count.entry(g).or_default() += 1;
        }
    }
    let mut index: HashMap<[u8; KEY_LEN], Vec<u32>> = HashMap::new();
    let (mut always, mut hides) = (Vec::new(), Vec::new());
    for (i, r) in rules.iter().enumerate() {
        let i = i as u32;
        if matches!(r.action, Action::Hide(_)) {
            hides.push(i);
            continue;
        }
        match runs[i as usize].as_deref().and_then(|run| grams(run).min_by_key(|g| count.get(g).copied().unwrap_or(0))) {
            Some(k) => index.entry(k).or_default().push(i),
            None => always.push(i),
        }
    }
    // What a request is always judged against, and the element hiding rules, are built here on the
    // worker; a rule that will not build goes, and only so many always-judged rules are kept.
    always.retain(|&i| rules[i as usize].filter.prebuild());
    always.truncate(MAX_ALWAYS);
    hides.retain(|&i| rules[i as usize].filter.prebuild());
    Ok(Rules { rules, index, always, hides })
}

fn parse_rule(v: &Value) -> Option<Rule> {
    let trigger = v.get("trigger")?.as_object()?;
    let action = v.get("action")?.as_object()?;
    let action = match action.get("type")?.as_str()? {
        "block" => Action::Block,
        "ignore-previous-rules" => Action::Ignore,
        "css-display-none" => Action::Hide(action.get("selector")?.as_str().filter(|s| selector_ok(s))?.to_string()),
        // block-cookies and make-https are not supported (Decision).
        _ => return None,
    };
    // Triggers Termurion does not implement make a rule inapplicable rather than broader.
    for unsupported in ["if-top-url", "unless-top-url", "if-frame-url", "unless-frame-url"] {
        if trigger.contains_key(unsupported) {
            return None;
        }
    }
    let text = trigger.get("url-filter")?.as_str()?;
    let sensitive = trigger.get("url-filter-is-case-sensitive").and_then(Value::as_bool).unwrap_or(false);
    let filter = if matches!(text, ".*" | "^.*" | "" | "^") { Filter::Any } else if filter_ok(text) { Filter::Pattern { text: text.to_string(), sensitive, re: OnceLock::new() } } else { return None };
    let types = match trigger.get("resource-type").and_then(Value::as_array) {
        None => ALL_BUT_POPUP,
        Some(l) => l.iter().filter_map(Value::as_str).filter_map(resource_named).fold(0, |m, r| m | r.bit()),
    };
    if types == 0 {
        return None;
    }
    let party = match trigger.get("load-type").and_then(Value::as_array) {
        None => Party::Any,
        Some(l) => {
            let names: Vec<&str> = l.iter().filter_map(Value::as_str).collect();
            match (names.contains(&"first-party"), names.contains(&"third-party")) {
                (true, false) => Party::First,
                (false, true) => Party::Third,
                _ => Party::Any,
            }
        }
    };
    let list = |key: &str| -> Option<Vec<Domain>> {
        let l = trigger.get(key)?.as_array()?;
        Some(
            l.iter()
                .filter_map(Value::as_str)
                .take(1000)
                .map(|d| {
                    let d = d.to_ascii_lowercase();
                    let subdomains = d.starts_with('*');
                    Domain { name: d.trim_start_matches('*').trim_start_matches('.').to_string(), subdomains }
                })
                .filter(|d| !d.name.is_empty())
                .collect(),
        )
    };
    let domains = match (list("if-domain"), list("unless-domain")) {
        (Some(l), _) => Domains::If(l),
        (None, Some(l)) => Domains::Unless(l),
        _ => Domains::Any,
    };
    Some(Rule { filter, types, party, domains, action })
}

/// WebKit's restricted regular expression: no alternation and no counted repeats; short.
fn filter_ok(p: &str) -> bool {
    p.len() <= MAX_FILTER && !p.chars().any(|c| c.is_control() || matches!(c, '{' | '}' | '|'))
}

/// A selector that cannot end its own rule or start another (no braces, semicolons, at-rules,
/// comments, tags, backslashes, balanced quotes).
fn selector_ok(s: &str) -> bool {
    !s.trim().is_empty()
        && s.len() <= MAX_SELECTOR
        && !s.chars().any(|c| c.is_control() || matches!(c, '{' | '}' | ';' | '@' | '<' | '\\'))
        && !s.contains("/*")
        && s.matches('"').count() % 2 == 0
        && s.matches('\'').count() % 2 == 0
}

/// The longest run (lower case) of literal characters every match must contain, or None when
/// there is none of `KEY_LEN` characters.
fn literal_run(pattern: &str) -> Option<String> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut best = String::new();
    let mut run = String::new();
    let mut i = 0;
    let end_run = |run: &mut String, best: &mut String| {
        if run.len() > best.len() {
            *best = std::mem::take(run);
        } else {
            run.clear();
        }
    };
    while i < chars.len() {
        let c = chars[i];
        let literal = match c {
            '\\' => {
                i += 1;
                match chars.get(i) {
                    Some(n) if n.is_ascii_graphic() && !n.is_ascii_alphanumeric() => Some(*n),
                    _ => None,
                }
            }
            '[' => {
                // A class: skip to its end.
                while i < chars.len() && chars[i] != ']' {
                    if chars[i] == '\\' {
                        i += 1;
                    }
                    i += 1;
                }
                None
            }
            '(' => {
                // A group: nothing in it is certain to be required.
                let mut depth = 0;
                while i < chars.len() {
                    match chars[i] {
                        '\\' => i += 1,
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                None
            }
            '.' | '^' | '$' | '*' | '+' | '?' | ')' | ']' => None,
            c if c.is_ascii() => Some(c),
            _ => None,
        };
        i += 1;
        match literal {
            Some(l) => {
                // An optional atom (`x*`, `x?`) is not required.
                if matches!(chars.get(i), Some('*' | '?')) {
                    end_run(&mut run, &mut best);
                } else {
                    run.push(l.to_ascii_lowercase());
                }
            }
            None => end_run(&mut run, &mut best),
        }
    }
    end_run(&mut run, &mut best);
    (best.len() >= KEY_LEN).then_some(best)
}

fn grams(run: &str) -> impl Iterator<Item = [u8; KEY_LEN]> + '_ {
    run.as_bytes().windows(KEY_LEN).map(|w| [w[0], w[1], w[2], w[3]])
}

/// The lower case host of an http(s) or ws(s) URL, None for any other scheme.
fn web_host(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https" | "ws" | "wss") {
        return None;
    }
    let auth = rest.split(['/', '?', '#']).next()?;
    let auth = auth.rsplit('@').next()?;
    let host = if let Some(v6) = auth.strip_prefix('[') { v6.split(']').next()? } else { auth.split(':').next()? };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// The host's registrable part: the last two labels, three under a two letter country code with a
/// common second level label. An approximation of the public suffix list.
fn registrable(host: &str) -> String {
    if host.contains(':') || host.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return host.to_string();
    }
    let labels: Vec<&str> = host.split('.').collect();
    let n = labels.len();
    let take = if n >= 3 && labels[n - 1].len() == 2 && matches!(labels[n - 2], "co" | "com" | "org" | "net" | "gov" | "edu" | "ac" | "ne" | "or" | "go") { 3 } else { 2 };
    labels[n.saturating_sub(take)..].join(".")
}

/// Whether the request `url` for `resource` made by the page `top_url` is blocked.
pub fn blocks(rules: &Rules, url: &str, resource: Resource, top_url: &str) -> bool {
    blocks_within(rules, url, resource, top_url, MATCH_BUDGET)
}

/// `blocks` with its own time budget (the tests of what a list means give a loaded machine room).
fn blocks_within(rules: &Rules, url: &str, resource: Resource, top_url: &str, budget: Duration) -> bool {
    let Some(host) = web_host(url) else { return false };
    let start = Instant::now();
    let top = if resource == Resource::Document || top_url.is_empty() { url } else { top_url };
    let top_host = web_host(top).unwrap_or_default();
    let third = !top_host.is_empty() && registrable(&host) != registrable(&top_host);
    let lower: Vec<u8> = url.bytes().take(MAX_HAYSTACK).map(|b| b.to_ascii_lowercase()).collect();
    let mut candidates: Vec<u32> = rules.always.clone();
    for w in lower.windows(KEY_LEN) {
        if let Some(l) = rules.index.get(&[w[0], w[1], w[2], w[3]]) {
            candidates.extend_from_slice(l);
        }
    }
    candidates.sort_unstable();
    candidates.dedup();
    let mut blocked = false;
    for i in candidates {
        // The budget is checked before every candidate; out of it the request goes on (ads only).
        if start.elapsed() > budget {
            OVER_BUDGET.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return false;
        }
        let r = &rules.rules[i as usize];
        if r.types & resource.bit() == 0 || !r.domains.allows(&top_host) {
            continue;
        }
        if (r.party == Party::Third && !third) || (r.party == Party::First && third) {
            continue;
        }
        if !r.filter.matches(url) {
            continue;
        }
        match r.action {
            Action::Block => blocked = true,
            Action::Ignore => blocked = false,
            Action::Hide(_) => {}
        }
    }
    blocked
}

/// The CSS that hides the elements the list names on the page `top_url` (one rule per selector,
/// so one a browser rejects cannot take the rest with it).
pub fn hiding_css(rules: &Rules, top_url: &str) -> String {
    let Some(top_host) = web_host(top_url).filter(|_| top_url.len() < 4096) else { return String::new() };
    let start = Instant::now();
    let mut css = String::new();
    for &i in &rules.hides {
        if start.elapsed() > CSS_BUDGET || css.len() > MAX_CSS {
            break;
        }
        let r = &rules.rules[i as usize];
        let Action::Hide(sel) = &r.action else { continue };
        if r.domains.allows(&top_host) && r.filter.matches(top_url) {
            css.push_str(sel);
            css.push_str(" { display: none !important; }\n");
        }
    }
    css
}

/// The script that puts `css` on the page (iTerm2's `adblock-css-injection.js`): a JSON string is
/// a valid script string, so nothing in the CSS can end it.
pub fn injection_script(css: &str) -> String {
    let literal = serde_json::to_string(css).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        "(function(){{var old=document.querySelector('style[data-adblock]');if(old){{old.remove();}}var s=document.createElement('style');s.setAttribute('data-adblock','true');s.textContent={literal};(document.head||document.documentElement).appendChild(s);}})();"
    )
}

/// An update is due after a day, when there was never one, and when no rules are kept (`ABM:111-130`).
pub fn needs_update(last: Option<i64>, now: i64, have_rules: bool) -> bool {
    match last {
        None => true,
        Some(l) => !have_rules || l > now || now - l >= UPDATE_INTERVAL.as_secs() as i64,
    }
}

/// The number of rules in a saved list: the length of its JSON array, 0 for anything else (and for a
/// text above the list bound).
pub fn saved_rule_count(json: &str) -> usize {
    if json.len() > MAX_LIST_BYTES {
        return 0;
    }
    match serde_json::from_str::<Value>(json) {
        Ok(Value::Array(a)) => a.len(),
        _ => 0,
    }
}

pub fn stale_text(days: i64) -> Option<String> {
    (days >= STALE_DAYS).then(|| format!("Adblock rules haven't been updated for {days} days"))
}

pub fn invalid_url_text(url: &str) -> String {
    format!("Invalid adblock list URL: {url}")
}

/// The list URL is https with a host and nothing a request line could not hold.
pub fn list_url_ok(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once("://") else { return false };
    scheme.eq_ignore_ascii_case("https") && !url.chars().any(|c| c.is_whitespace() || c.is_control()) && rest.split(['/', '?', '#']).next().is_some_and(|a| !a.is_empty() && !a.starts_with(':') && !a.starts_with('@'))
}

/// A hostname or IPv4 address (`[A-Za-z0-9.-]`, 1 to 253 characters, not starting with `-`) or a
/// bracketed IPv6 address: nothing that could be a second argument (iTerm2 checks only for empty,
/// `iTermBrowserSettingsHandler.swift:383`; Termurion builds a command line, so it is stricter).
pub fn proxy_host_ok(host: &str) -> bool {
    if let Some(inner) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        return !inner.is_empty() && inner.contains(':') && inner.len() <= 45 && inner.bytes().all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.');
    }
    !host.is_empty() && host.len() <= 253 && !host.starts_with('-') && host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
}

/// `--proxy-server=http://host:port` (`BM:1850-1870`: an HTTP proxy), or "" when off or when the
/// host or port would not be accepted.
pub fn proxy_args(enabled: bool, host: &str, port: i64) -> String {
    if enabled && proxy_host_ok(host) && (1..=65535).contains(&port) { format!("--proxy-server=http://{host}:{port}") } else { String::new() }
}

/// Whether `args` is exactly what `proxy_args` makes (or empty): the environment takes nothing else.
pub fn proxy_args_ok(args: &str) -> bool {
    let Some(rest) = args.strip_prefix("--proxy-server=http://") else { return args.is_empty() };
    let Some((host, port)) = rest.rsplit_once(':') else { return false };
    !port.is_empty() && port.len() <= 5 && port.bytes().all(|b| b.is_ascii_digit()) && port.parse::<i64>().is_ok_and(|p| proxy_args(true, host, p) == args)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a list means, judged with room for a loaded machine; the budget has its own test.
    fn blocks(rules: &Rules, url: &str, resource: Resource, top_url: &str) -> bool {
        blocks_within(rules, url, resource, top_url, Duration::from_secs(5))
    }

    const TOP: &str = "https://site.test/";

    fn rules(json: &str) -> Rules {
        compile(json).expect("the list compiles")
    }

    fn block_rule(filter: &str) -> String {
        format!(r#"{{"trigger":{{"url-filter":"{filter}"}},"action":{{"type":"block"}}}}"#)
    }

    #[test]
    fn saved_rule_count_is_the_array_length() {
        assert_eq!(saved_rule_count("[{},{},{}]"), 3);
        assert_eq!(saved_rule_count("{}"), 0);
        assert_eq!(saved_rule_count("x"), 0);
        assert_eq!(saved_rule_count(&format!("[{}]", " ".repeat(MAX_LIST_BYTES))), 0, "above the bound");
    }

    #[test]
    fn content_blocker_follows_webkit() {
        let r = rules(&format!("[{}]", block_rule(r"ads\\.example")));
        assert!(blocks(&r, "https://ads.example/x", Resource::Image, TOP));
        assert!(!blocks(&r, "https://a.b", Resource::Image, TOP));
        // url-filter-is-case-sensitive defaults to false.
        assert!(blocks(&r, "https://ADS.EXAMPLE/x", Resource::Image, TOP));
        let sensitive = rules(r#"[{"trigger":{"url-filter":"ads\\.example","url-filter-is-case-sensitive":true},"action":{"type":"block"}}]"#);
        assert!(!blocks(&sensitive, "https://ADS.EXAMPLE/x", Resource::Image, TOP));
        assert!(blocks(&sensitive, "https://ads.example/x", Resource::Image, TOP));
        // Only the listed resource types.
        let script = rules(r#"[{"trigger":{"url-filter":"ads\\.example","resource-type":["script"]},"action":{"type":"block"}}]"#);
        assert!(blocks(&script, "https://ads.example/a.js", Resource::Script, TOP));
        assert!(!blocks(&script, "https://ads.example/a.png", Resource::Image, TOP));
        // load-type third-party: the request's registrable host differs from the top page's.
        let third = rules(r#"[{"trigger":{"url-filter":"ads\\.example","load-type":["third-party"]},"action":{"type":"block"}}]"#);
        assert!(blocks(&third, "https://ads.example/x", Resource::Script, "https://news.test/"));
        assert!(!blocks(&third, "https://ads.example/x", Resource::Script, "https://www.ads.example/"), "a first-party request");
        let first = rules(r#"[{"trigger":{"url-filter":"ads\\.example","load-type":["first-party"]},"action":{"type":"block"}}]"#);
        assert!(blocks(&first, "https://ads.example/x", Resource::Script, "https://www.ads.example/"));
        assert!(!blocks(&first, "https://ads.example/x", Resource::Script, "https://news.test/"));
        // if-domain / unless-domain with a leading star for subdomains.
        let only = rules(r#"[{"trigger":{"url-filter":".*","if-domain":["*site.com"]},"action":{"type":"block"}}]"#);
        assert!(blocks(&only, "https://x.test/a", Resource::Image, "https://www.site.com/"));
        assert!(blocks(&only, "https://x.test/a", Resource::Image, "https://site.com/"));
        assert!(!blocks(&only, "https://x.test/a", Resource::Image, "https://other.org/"));
        assert!(!blocks(&only, "https://x.test/a", Resource::Image, "https://notsite.com.evil.org/"));
        let except = rules(r#"[{"trigger":{"url-filter":".*","unless-domain":["*site.com"]},"action":{"type":"block"}}]"#);
        assert!(!blocks(&except, "https://x.test/a", Resource::Image, "https://www.site.com/"));
        assert!(blocks(&except, "https://x.test/a", Resource::Image, "https://other.org/"));
        // ignore-previous-rules after a block rule unblocks what it matches.
        let ignore = rules(&format!(r#"[{},{{"trigger":{{"url-filter":"ads\\.example/ok"}},"action":{{"type":"ignore-previous-rules"}}}}]"#, block_rule(r"ads\\.example")));
        assert!(blocks(&ignore, "https://ads.example/x", Resource::Image, TOP));
        assert!(!blocks(&ignore, "https://ads.example/ok", Resource::Image, TOP));
        // css-display-none contributes to the hiding CSS and never blocks.
        let hide = rules(r#"[{"trigger":{"url-filter":".*"},"action":{"type":"css-display-none","selector":".ad"}}]"#);
        assert!(!blocks(&hide, "https://site.test/a", Resource::Image, TOP));
        let css = hiding_css(&hide, TOP);
        assert!(css.contains(".ad") && css.contains("display: none !important"), "{css}");
        // An unknown action type is skipped (so are block-cookies and make-https).
        let unknown = rules(r#"[{"trigger":{"url-filter":".*"},"action":{"type":"frobnicate"}},{"trigger":{"url-filter":".*"},"action":{"type":"block-cookies"}}]"#);
        assert!(!blocks(&unknown, "https://site.test/a", Resource::Image, TOP));
        // Not JSON, or JSON that is not a list of rules.
        for bad in ["", "{", "[1,", "{\"a\":1}", "\"x\""] {
            let e = compile(bad).unwrap_err();
            assert!(e == NOT_JSON || e == PARSE_FAILED, "{bad}: {e}");
        }
        // Only http, https, ws and wss are blocked (Termurion's own pages, data and blob never).
        let all = rules(r#"[{"trigger":{"url-filter":".*"},"action":{"type":"block"}}]"#);
        assert!(blocks(&all, "https://x.test/", Resource::Document, ""));
        for url in ["iterm2-about:welcome", "data:text/html,x", "blob:https://x.test/1", "file:///c:/a.html"] {
            assert!(!blocks(&all, url, Resource::Document, TOP), "{url}");
        }
    }

    #[test]
    fn resources_follow_webview2() {
        let kinds = [(1, Resource::Document), (2, Resource::StyleSheet), (3, Resource::Image), (4, Resource::Media), (5, Resource::Font), (6, Resource::Script), (7, Resource::Raw), (8, Resource::Raw), (16, Resource::Other), (13, Resource::Other), (99, Resource::Other), (-1, Resource::Other)];
        for (k, r) in kinds {
            assert_eq!(resource_of(k), r, "{k}");
        }
    }

    #[test]
    fn a_big_list_answers_a_miss_quickly() {
        let mut json = String::from("[");
        for i in 0..20_000 {
            if i > 0 {
                json.push(',');
            }
            json.push_str(&block_rule(&format!(r"ads{i}\\.example\\.com/banner")));
        }
        json.push(']');
        let r = rules(&json);
        let start = std::time::Instant::now();
        assert!(!blocks(&r, "https://news.example.org/story?id=7", Resource::Document, "https://news.example.org/"));
        assert!(start.elapsed() < Duration::from_millis(50), "{:?}", start.elapsed());
        assert!(blocks(&r, "https://ads19999.example.com/banner.png", Resource::Image, TOP));
    }

    #[test]
    fn a_hostile_list_is_bounded() {
        // Every rule shares one literal. This measures raw matching speed (this module's `blocks` has a
        // 5 s budget so the meaning tests are steady); the 2 ms per-request budget is held by
        // `a_dfa_hostile_list_stays_in_budget`.
        let mut json = String::from("[");
        for i in 0..30_000 {
            if i > 0 {
                json.push(',');
            }
            json.push_str(&block_rule(&format!(r"aaaa{i}[0-9]+zz")));
        }
        json.push(']');
        let r = rules(&json);
        let start = std::time::Instant::now();
        let _ = blocks(&r, "https://x.test/aaaa", Resource::Image, TOP);
        assert!(start.elapsed() < Duration::from_millis(250), "{:?}", start.elapsed());
        // More than the cap: compiled, truncated, never a panic.
        let many = format!("[{}]", vec![block_rule("x"); 100_100].join(","));
        let _ = compile(&many).expect("a long list compiles");
        // A list over the size cap is refused whole.
        let huge = format!("[{}]", "0,".repeat(5 << 20));
        assert!(compile(&huge).is_err());
        // Deep nesting is an error, never a stack overflow.
        let deep = format!("{}{}", "[".repeat(5000), "]".repeat(5000));
        assert!(compile(&deep).is_err());
        // Odd filters are skipped rather than crashing: counted repeats, alternation, a lone escape, a huge pattern.
        let odd = format!(r#"[{},{},{},{},{}]"#, block_rule("a{1000}{1000}"), block_rule("a|b"), block_rule("\\\\"), block_rule("("), block_rule(&"a".repeat(5000)));
        let r = rules(&odd);
        assert!(!blocks(&r, "https://a.test/aaa", Resource::Image, TOP));
    }

    #[test]
    fn hiding_css_cannot_carry_other_rules() {
        let sel = |s: &str| format!(r#"{{"trigger":{{"url-filter":".*"}},"action":{{"type":"css-display-none","selector":{}}}}}"#, serde_json::to_string(s).unwrap());
        let json = format!("[{},{},{},{},{}]", sel(".ad"), sel("a} body{background:url(http://evil/x)"), sel("div > .banner"), sel("x;y"), sel(&"a".repeat(600)));
        let css = hiding_css(&rules(&json), TOP);
        assert!(css.contains(".ad") && css.contains("div > .banner"), "{css}");
        assert!(!css.contains("evil") && !css.contains("x;y"), "{css}");
        assert!(!css.contains(&"a".repeat(600)));
        // It applies to the top page only where the trigger does.
        let scoped = rules(r#"[{"trigger":{"url-filter":".*","if-domain":["*site.test"]},"action":{"type":"css-display-none","selector":".ad"}}]"#);
        assert!(hiding_css(&scoped, "https://www.site.test/p").contains(".ad"));
        assert_eq!(hiding_css(&scoped, "https://other.test/"), "");
        assert_eq!(hiding_css(&scoped, "iterm2-about:welcome"), "", "Termurion's own pages are never styled");
    }

    #[test]
    fn adblock_schedule_follows_iterm2() {
        let now = 1_800_000_000;
        let h = 3600;
        assert!(needs_update(None, now, false));
        assert!(!needs_update(Some(now - 23 * h), now, true));
        assert!(needs_update(Some(now - 25 * h), now, true));
        assert!(needs_update(Some(now - h), now, false), "no rules on disk");
        assert!(needs_update(Some(now - 24 * h), now, true));
        assert!(needs_update(None, now, true), "never updated");
        assert!(needs_update(Some(now + 5 * h), now, true), "a clock set back");
        assert_eq!(UPDATE_INTERVAL, Duration::from_secs(86_400));
        assert_eq!(STALE_DAYS, 7);
        assert_eq!(stale_text(6), None);
        assert_eq!(stale_text(7).as_deref(), Some("Adblock rules haven't been updated for 7 days"));
        assert_eq!(stale_text(30).as_deref(), Some("Adblock rules haven't been updated for 30 days"));
        assert_eq!(invalid_url_text("ftp://x"), "Invalid adblock list URL: ftp://x");
        assert_eq!(LAST_UPDATE_KEY, "NoSyncAdblockLastUpdate");
        assert_eq!(PARSE_FAILED, "Failed to parse adblock list response");
        assert_eq!(NOT_JSON, "Downloaded content is not valid JSON format");
    }

    #[test]
    fn proxy_args_follow_iterm2() {
        assert_eq!(proxy_args(true, "127.0.0.1", 8118), "--proxy-server=http://127.0.0.1:8118");
        assert_eq!(proxy_args(false, "127.0.0.1", 8118), "");
        assert_eq!(proxy_args(true, "", 8118), "");
        assert_eq!(proxy_args(true, "h", 70000), "");
        assert_eq!(proxy_args(true, "h", 0), "");
        assert_eq!(proxy_args(true, "h", 65535), "--proxy-server=http://h:65535");
        assert_eq!(proxy_args(true, "[::1]", 3128), "--proxy-server=http://[::1]:3128");
        for ok in ["127.0.0.1", "proxy.lan", "[::1]", "[fe80::1]", "a"] {
            assert!(proxy_host_ok(ok), "{ok}");
        }
        let long = "a".repeat(254);
        for bad in ["127.0.0.1 --renderer-cmd-prefix=calc.exe", "-x", "--proxy-server=a", "a b", "a\"b", "h:8080", "[::1", "", long.as_str(), "[]", "[zz]", "a=b", "a\tb", "h\u{0}", "u:p@h", "a/b", "http://h"] {
            assert!(!proxy_host_ok(bad), "{bad:?}");
        }
        // The class attack: the host cannot add a Chromium switch, whatever writes it.
        assert_eq!(proxy_args(true, "http://h:1 --remote-debugging-port=9222", 1), "");
        assert_eq!(proxy_args(true, "127.0.0.1 --renderer-cmd-prefix=calc.exe", 8118), "");
        assert_eq!(proxy_args(true, "h --remote-debugging-port=9222", 8118), "");
        assert_eq!(proxy_args(true, "h\n--x", 8118), "");
    }

    #[test]
    fn only_the_proxy_argument_reaches_the_environment() {
        for ok in ["", "--proxy-server=http://127.0.0.1:8118", "--proxy-server=http://[::1]:3128"] {
            assert!(proxy_args_ok(ok), "{ok}");
        }
        for bad in ["--remote-debugging-port=9222", "--proxy-server=http://h:1 --remote-debugging-port=9222", "--proxy-server=http://h:1\n--x", " --proxy-server=http://h:1", "--proxy-server=socks5://h:1", "--proxy-server=http://h:0", "--proxy-server=http://h:99999", "--proxy-server=http://:1", "--proxy-server=http://h", "--proxy-server=http://u:p@h:1"] {
            assert!(!proxy_args_ok(bad), "{bad:?}");
        }
    }

    #[test]
    fn list_urls_are_https_only() {
        assert!(list_url_ok("https://easylist-downloads.adblockplus.org/easylist_content_blocker.json"));
        for bad in ["http://x.test/l.json", "ftp://x.test/l", "file:///c:/l.json", "javascript:alert(1)", "https://", "https:///x", "https://a b/c", "", "HTTPS://", "//x.test/l"] {
            assert!(!list_url_ok(bad), "{bad:?}");
        }
        assert!(list_url_ok("HTTPS://X.test/l.json"));
    }

    #[test]
    fn injection_script_carries_the_css_as_one_string() {
        let js = injection_script("a{b:\"c\"}\n</style><script>alert(1)</script>");
        assert!(js.contains("data-adblock") && js.contains("textContent"));
        // The css is a JSON string literal: its quotes and newline are escaped.
        assert!(js.contains(r#""a{b:\"c\"}\n"#), "{js}");
        assert!(!js.contains("\n</style>"));
    }

    #[test]
    fn schema_defaults_follow_iterm2() {
        let get = |k: &str| crate::schema::find_global(k).and_then(|s| s.default.clone()).unwrap_or_else(|| panic!("{k}"));
        assert_eq!(get("WebKitAdblockEnabled"), toml::Value::Boolean(false));
        assert_eq!(get("AdblockListURL"), toml::Value::String("https://easylist-downloads.adblockplus.org/easylist_content_blocker.json".into()));
        assert_eq!(get("BrowserProxyEnabled"), toml::Value::Boolean(false));
        assert_eq!(get("BrowserProxyHost"), toml::Value::String("127.0.0.1".into()));
        assert_eq!(get("BrowserProxyPort"), toml::Value::Integer(8118));
        for k in ["WebKitAdblockEnabled", "AdblockListURL", "BrowserProxyEnabled", "BrowserProxyHost", "BrowserProxyPort"] {
            let s = crate::schema::find_global(k).unwrap();
            assert_eq!((s.tab.as_str(), s.section.as_str()), ("Advanced", "Web Browser"), "{k}");
        }
    }

    #[test]
    fn the_proxy_values_are_bounded_at_the_write_point() {
        let dir = std::env::temp_dir().join(format!("tandem-adblock-g-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut g = crate::globals::Globals::empty(&dir.join("settings.toml"));
        for bad in ["127.0.0.1 --renderer-cmd-prefix=calc.exe", "a\"b", &"a".repeat(300)] {
            g.set("BrowserProxyHost", toml::Value::String(bad.into()));
            assert_eq!(g.string("BrowserProxyHost"), "127.0.0.1", "{bad:?}");
        }
        g.set("BrowserProxyHost", toml::Value::String("proxy.lan".into()));
        assert_eq!(g.string("BrowserProxyHost"), "proxy.lan");
        g.set("BrowserProxyPort", toml::Value::Integer(70000));
        assert_eq!(g.int("BrowserProxyPort"), 65535);
        g.set("BrowserProxyPort", toml::Value::Integer(-4));
        assert_eq!(g.int("BrowserProxyPort"), 1);
        g.set("AdblockListURL", toml::Value::String("https://x.test/a b".into()));
        assert!(!g.string("AdblockListURL").contains(' '));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A list that cannot be cheap per request (rules with no literal to index by, shaped to make
    /// the regex engine slow) and a megabyte URL: the answer comes back within the budget plus one
    /// bounded match, every always-judged regex was built when the list compiled, and the URL is
    /// cut before any match.
    #[test]
    fn a_dfa_hostile_list_stays_in_budget() {
        let json = format!("[{}]", (0..24).map(|i| block_rule(&format!(".*a{}{i}", ".".repeat(100)))).collect::<Vec<_>>().join(","));
        let r = rules(&json);
        assert_eq!(r.len(), 24);
        assert!(r.always_prebuilt(), "built on the worker, not on first use");
        let url = format!("https://x.test/{}", "a".repeat(1 << 20));
        // The real budget bounds the time, however fast the machine.
        let start = std::time::Instant::now();
        assert!(!blocks_within(&r, &url, Resource::Image, TOP, MATCH_BUDGET), "over budget means allowed (fail open for ads only)");
        assert!(start.elapsed() < Duration::from_millis(250), "{:?}", start.elapsed());
        // A spent budget (none at all) always cuts the answer short, allows the request and is
        // counted (final review Minor 16: not dependent on the machine being slow; other tests can
        // only raise the process-wide counter, never lower it).
        let before = over_budget_count();
        assert!(!blocks_within(&r, &url, Resource::Image, TOP, Duration::ZERO), "over budget means allowed");
        assert!(over_budget_count() > before, "an answer cut short is counted");
    }
}

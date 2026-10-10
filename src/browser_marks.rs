//! Browser named marks, the pure half (milestone 13x5, Task 1): iTerm2's `Location`, the
//! `iterm-mark:` URL fragment and its encoding, the page messages, and the filled page scripts
//! (`iTermBrowserNamedMarkManager.swift`, `BrowserNamedMarks.swift`). The page is untrusted: its
//! answers are validated and bounded here, and no page-derived text is ever pasted into script
//! source. A script that takes a page-derived value gets it as a typed argument of the isolated
//! world's runner (`Call`), through a fail-closed load-time patch of the copied file, which stays
//! byte-identical on disk. Only generated tokens and integers are substituted into source.

use serde_json::{Value, json};

use crate::browser_js::{World, fill_template, js_string, resource};
use crate::marks::percent_decode_strict;

/// The page's message handler for a rename or delete (`NMM:12`).
pub const MESSAGE_HANDLER: &str = "iTerm2NamedMarkUpdate";
/// The layout monitor's message handler (`NMM:13`).
pub const LAYOUT_HANDLER: &str = "iTerm2MarkLayoutUpdate";
/// The fragment prefix that makes a URL a mark's location (`NMM:61`).
pub const FRAGMENT_PREFIX: &str = "iterm-mark:";

/// Bounds on page-derived data (B8): a longer value is refused, not cut, so no half-location exists.
pub const MAX_XPATH: usize = 2048;
pub const MAX_TEXT: usize = 2048;
pub const MAX_NAME: usize = 256;
pub const MAX_GUID: usize = 128;
pub const MAX_URL: usize = 8192;
/// Marks shown on one page, and the most marks one store holds.
pub const MAX_MARKS: usize = 1000;

/// `Location` (`NMM:35-57`): where on a page a mark sits.
#[derive(Clone, Debug, PartialEq)]
pub struct Location {
    pub xpath: String,
    pub offset_y: i64,
    pub scroll_y: i64,
    pub text_fragment: Option<String>,
    pub y: i64,
}

/// Foundation's `.urlFragmentAllowed` and `.urlQueryAllowed` agree on these (RFC 3986 unreserved
/// plus `!$&'()*+,;=:@/?`); every other UTF-8 byte, `%` included, becomes `%XX` in upper case.
fn encode_allowed(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/?".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The `URLComponents.fragment` setter's encoding (`NMM:605-610`).
pub fn encode_fragment(raw: &str) -> String {
    encode_allowed(raw)
}

/// The URL without its fragment (`BNM:197-205`).
pub fn without_fragment(url: &str) -> String {
    url.split('#').next().unwrap_or_default().to_string()
}

/// Whether two URLs are the same page (`NMM:187`).
pub fn same_page(a: &str, b: &str) -> bool {
    without_fragment(a) == without_fragment(b)
}

/// The page URL carrying the mark's location (`NMM:605-610`); None when the result would pass
/// `MAX_URL` (the page's URL itself may be a truncated copy, so it is refused too).
pub fn mark_url(page_url: &str, loc: &Location) -> Option<String> {
    if page_url.is_empty() || page_url.len() > MAX_URL {
        return None;
    }
    let url = format!("{}#{}", without_fragment(page_url), encode_fragment(&loc.fragment()));
    (url.len() <= MAX_URL).then_some(url)
}

impl Location {
    /// Page data is bounded, and an XPath holding `&` is refused: the fragment is `&`-split and `&` is
    /// not escaped by the fragment encoding, so the page could write its own `textFragment` pair
    /// (task-1 re-review minor 5; R13x5-5).
    fn bounded(self) -> Option<Location> {
        (self.xpath.len() <= MAX_XPATH && !self.xpath.contains('&') && self.text_fragment.as_ref().is_none_or(|t| t.len() <= MAX_TEXT)).then_some(self)
    }

    /// From the page's answer (`NMM:45-57`): `xpath` a string, `offsetY`, `scrollY`, `y` integers.
    pub fn from_json(v: &Value) -> Option<Location> {
        let xpath = v.get("xpath")?.as_str()?.to_string();
        let offset_y = v.get("offsetY")?.as_i64()?;
        let scroll_y = v.get("scrollY")?.as_i64()?;
        let y = v.get("y")?.as_i64()?;
        let text_fragment = v.get("textFragment").and_then(Value::as_str).map(str::to_string);
        Location { xpath, offset_y, scroll_y, text_fragment, y }.bounded()
    }

    /// From a URL's raw fragment (`NMM:59-99`). A fragment that is not a mark's is not read.
    pub fn from_url(url: &str) -> Option<Location> {
        let fragment = url.split_once('#')?.1;
        let rest = fragment.strip_prefix(FRAGMENT_PREFIX)?;
        let mut xpath = None;
        let mut offset_y = None;
        let mut scroll_y = None;
        let mut y = None;
        let mut text_fragment = None;
        for pair in rest.split('&') {
            let Some((key, raw)) = pair.split_once('=') else { continue };
            let value = percent_decode_strict(raw).unwrap_or_else(|| raw.to_string());
            match key {
                "xpath" => xpath = Some(value),
                "offsetY" => offset_y = Some(value.parse::<i64>().ok()?),
                "scrollY" => scroll_y = Some(value.parse::<i64>().ok()?),
                "y" => y = Some(value.parse::<i64>().ok()?),
                "textFragment" => text_fragment = percent_decode_strict(&value),
                _ => {}
            }
        }
        Location { xpath: xpath?, offset_y: offset_y?, scroll_y: scroll_y?, text_fragment, y: y? }.bounded()
    }

    /// The unencoded fragment (`NMM:101-114`); the text fragment alone is encoded here, the rest
    /// is left for `encode_fragment`.
    pub fn fragment(&self) -> String {
        let mut out = format!("{FRAGMENT_PREFIX}xpath={}&offsetY={}&scrollY={}&y={}", self.xpath, self.offset_y, self.scroll_y, self.y);
        if let Some(t) = self.text_fragment.as_deref().filter(|t| !t.is_empty()) {
            out.push_str("&textFragment=");
            // "&" would end the pair: escaped here (stored "%2526", read back as "&"), so page text cannot
            // write a pair of its own (task-3 review minor 1; the XPath refuses it instead).
            out.push_str(&encode_allowed(t).replace('&', "%26"));
        }
        out
    }

    /// One entry of the annotation list (`NMM:116-125`).
    pub fn js_dict(&self, name: Option<&str>, guid: &str) -> Value {
        json!({
            "name": name.unwrap_or("Unnamed"),
            "xpath": self.xpath,
            "offsetY": self.offset_y,
            "textFragment": self.text_fragment.as_deref().unwrap_or(""),
            "guid": guid,
            "y": self.y,
        })
    }
}

/// What the annotation box asked for. The box only REQUESTS (B2, class fix): the name is typed and
/// a delete is confirmed in a Tandem window outside the page, which the page cannot see, cover or
/// type into. A forged or clickjacked press can at most open that window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Update {
    RequestRename { guid: String },
    RequestDelete { guid: String },
}

fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// `handleMessage` (`NMM:228-252`), changed (B2): the secret must match (an empty one never does) and
/// the message is `requestDelete: true` or `requestRename: true`. A message carrying a `name` or
/// `delete` is not honored: no page-reachable path changes a mark without the native window. Whether
/// the guid names a mark is the caller's check.
pub fn update_of(body: &Value, secret: &str) -> Option<Update> {
    let guid = body.get("guid")?.as_str()?;
    let sent = body.get("sessionSecret")?.as_str()?;
    if secret.is_empty() || !same_secret(sent, secret) || guid.is_empty() || guid.len() > MAX_GUID {
        return None;
    }
    let flag = |k: &str| body.get(k).and_then(Value::as_bool) == Some(true);
    if flag("requestDelete") {
        return Some(Update::RequestDelete { guid: guid.to_string() });
    }
    flag("requestRename").then(|| Update::RequestRename { guid: guid.to_string() })
}

/// `handleLayoutUpdateMessage` (`NMM:256-260`).
pub fn is_layout_change(body: &Value) -> bool {
    body.get("type").and_then(Value::as_str) == Some("layoutChange")
}

/// `browser_js::js_string` without its quotes, `<`, U+2028 and U+2029 written as escapes.
pub fn js_literal_inner(s: &str) -> String {
    let quoted = js_string(s);
    quoted[1..quoted.len() - 1].replace('<', "\\u003c").replace('\u{2028}', "\\u2028").replace('\u{2029}', "\\u2029")
}

/// A script body and its typed arguments, for `App::browser_js_call` (the runner declares each
/// argument as a constant before the body).
#[derive(Clone, PartialEq)]
pub struct Call {
    pub body: String,
    pub args: Value,
}

/// The args can hold a secret (a card value, a password): `Debug` names them and prints no value.
impl std::fmt::Debug for Call {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.args.as_object().map(|o| o.keys().map(String::as_str).collect()).unwrap_or_default();
        f.debug_struct("Call").field("body_len", &self.body.len()).field("args", &names).finish()
    }
}

/// Replaces `from` by `to` when `from` occurs exactly once (a load-time patch fails closed).
pub(crate) fn patch_once(src: &str, from: &str, to: &str) -> Result<String, String> {
    match src.matches(from).count() {
        1 => Ok(src.replacen(from, to, 1)),
        n => Err(format!("patch target {from:?} found {n} times")),
    }
}

/// The annotation box in the page is display plus two buttons that only REQUEST (B2, class fix; the
/// task-1 re-review showed that any in-page control can be clickjacked and that the page sees the keys
/// typed into the page). The name input is made a hidden field (it can take no key), "Save" becomes
/// "Rename" and posts `requestRename`, "Delete" posts `requestDelete` and removes nothing: the name is
/// typed and the delete confirmed in a Tandem window outside the web view. Defense in depth: the
/// box is built in a closed shadow root made in the isolated world under a host styled with
/// `!important` inline rules, a handler ignores an untrusted event, and a press counts only when the
/// trusted pointer event's point holds that very button (`elementFromPoint` on the shadow root) or,
/// for a keyboard activation (`detail` 0), when that button has focus. All patches are exact-once and
/// fail closed.
pub(crate) fn trust_patched(src: &str) -> Result<String, String> {
    let s = patch_once(src, "deleteButton.addEventListener('click', function(e) {", "deleteButton.addEventListener('click', function(e) { if (!tandemReal(e, deleteButton)) { return; }")?;
    let s = patch_once(&s, "saveButton.addEventListener('click', function(e) {", "saveButton.addEventListener('click', function(e) { if (!tandemReal(e, saveButton)) { return; }")?;
    // Requests only: no name leaves the page, nothing is removed or renamed in it.
    let s = patch_once(&s, "delete: true,", "requestDelete: true,")?;
    let s = patch_once(&s, "name: input.value,", "requestRename: true,")?;
    let s = patch_once(&s, "annotation.remove();", "")?;
    let s = patch_once(&s, "mark.name = input.value;", "")?;
    let s = patch_once(&s, "collapsedView.textContent = input.value;", "")?;
    let s = patch_once(&s, "input.type = 'text';", "input.type = 'hidden';")?;
    let s = patch_once(&s, "saveButton.textContent = 'Save';", "saveButton.textContent = 'Rename';")?;
    let s = patch_once(&s, "collapsedView.title = 'Click to expand and edit name';", "collapsedView.title = 'Click to expand';")?;
    // The name is drawn, never page text (R13x5-43): `window.find` and a select-all read text nodes
    // inside a closed shadow root; generated content (the `::before` rule `shadow_patched` adds) is
    // neither found nor selected.
    let s = patch_once(&s, "collapsedView.textContent = mark.name;", "collapsedView.setAttribute('data-tandem-name', mark.name);")?;
    let s = patch_once(&s, "markTitle.textContent = mark.name;", "markTitle.setAttribute('data-tandem-name', mark.name);")?;
    let s = patch_once(&s, "input.value = mark.name || '';", "input.value = '';")?;
    shadow_patched(&s)
}

/// The annotations live in a closed shadow root under a host Tandem keeps (`window.tandemMarkHost`,
/// the isolated world's window, which a page cannot see) and styles inline with `!important` after
/// `all: initial`, so page CSS cannot move, hide-by-containing-block or restyle it. The root is
/// `window.tandemMarkRoot` for `update-mark-positions.js`. The previous box is removed by that
/// reference, never by a page-controlled id. `tandemReal` is the real-button test.
fn shadow_patched(src: &str) -> Result<String, String> {
    let s = patch_once(
        src,
        "var container = document.createElement('div');",
        "var host = document.createElement('div');\n    host.style.cssText = 'all: initial !important; display: block !important; position: absolute !important; top: 0 !important; left: 0 !important; width: 100% !important; height: 0 !important; overflow: visible !important; z-index: 2147483647 !important; pointer-events: none !important;';\n    var shadow = host.attachShadow({mode: 'closed'});\n    var tandemNames = document.createElement('style');\n    tandemNames.textContent = '[data-tandem-name]::before { content: attr(data-tandem-name) !important; }';\n    shadow.appendChild(tandemNames);\n    window.tandemMarkRoot = shadow;\n    window.tandemMarkHost = host;\n    function tandemReal(e, button) {\n        if (!e.isTrusted) { return false; }\n        if (e.detail === 0) { return shadow.activeElement === button; }\n        return shadow.elementFromPoint(e.clientX, e.clientY) === button;\n    }\n    var container = document.createElement('div');",
    )?;
    let s = patch_once(&s, "container.id = 'iterm-mark-annotations';", "")?;
    let s = patch_once(&s, "document.getElementById('iterm-mark-annotations')", "window.tandemMarkHost")?;
    let s = patch_once(&s, "existingContainer.remove();", "existingContainer.remove();\n        window.tandemMarkHost = null;\n        window.tandemMarkRoot = null;")?;
    patch_once(&s, "document.body.appendChild(container);", "shadow.appendChild(container);\n        document.body.appendChild(host);")
}

/// `update-mark-positions.js` finds the annotations in the shadow root, or none.
fn positions_patched(src: &str) -> Result<String, String> {
    patch_once(
        src,
        "document.querySelectorAll('.iterm-mark-annotation[data-mark-guid]')",
        "(window.tandemMarkRoot ? window.tandemMarkRoot.querySelectorAll('.iterm-mark-annotation[data-mark-guid]') : [])",
    )
}

/// A copied script is an expression; the runner wants a body.
fn as_body(script: &str) -> String {
    format!("return {}", script.trim())
}

fn file(name: &str) -> Result<&'static str, String> {
    resource(name).ok_or_else(|| format!("{name} is not in the resource table"))
}

/// `get-xpath-at-point` at a page point (`NMM:408-418`); the coordinates are integers.
pub fn xpath_script(x: i64, y: i64) -> Option<String> {
    let src = resource("get-xpath-at-point.js")?;
    Some(as_body(&fill_template(src, &[("CLICK_X", &x.to_string()), ("CLICK_Y", &y.to_string())])))
}

/// `navigate-to-named-mark` (`NMM:444-462`). The XPath and text fragment come from the page's DOM,
/// so they travel as the typed arguments `xpathArg` and `textFragmentArg`; the integers are
/// substituted (a Decision: R3 security fix over iTerm2's raw paste).
pub fn navigate_call(loc: &Location) -> Result<Call, String> {
    let src = patch_once(file("navigate-to-named-mark.js")?, "var xpath = \"{{XPATH}}\";", "var xpath = xpathArg;")?;
    let src = patch_once(&src, "var textFragment = \"{{TEXT_FRAGMENT}}\";", "var textFragment = textFragmentArg;")?;
    let filled = fill_template(&src, &[("OFFSET_Y", &loc.offset_y.to_string()), ("SCROLL_Y", &loc.scroll_y.to_string()), ("Y", &loc.y.to_string())]);
    if filled.contains("{{XPATH}}") || filled.contains("{{TEXT_FRAGMENT}}") {
        return Err("a placeholder survived".into());
    }
    Ok(Call { body: as_body(&filled), args: json!({"xpathArg": loc.xpath, "textFragmentArg": loc.text_fragment.as_deref().unwrap_or("")}) })
}

/// `show-named-mark-annotations` (`NMM:480-500`): None for an empty list (`NMM:480-483`) and, failing
/// closed, when a load-time patch finds its target missing or repeated (no annotation is shown). The
/// marks (names are the person's, urls and xpaths the page's) are the typed argument `marksArg`; the
/// secret, substituted into source, is a generated token and written escaped anyway.
pub fn show_call(marks: &[Value], secret: &str) -> Option<Call> {
    if marks.is_empty() {
        return None;
    }
    let src = trust_patched(resource("show-named-mark-annotations.js")?).ok()?;
    let src = patch_once(&src, "var marks = {{MARKS_JSON}};", "var marks = marksArg;").ok()?;
    let filled = fill_template(&src, &[("SECRET", &js_literal_inner(secret))]);
    let shown = &marks[..marks.len().min(MAX_MARKS)];
    Some(Call { body: as_body(&filled), args: json!({ "marksArg": shown }) })
}

/// The annotations taken off the page when no mark is left for it (the show script returns early on
/// an empty list, `NMM:480-483`, so its own removal never runs): by Tandem's reference to the host,
/// never a lookup in the page.
pub fn clear_call() -> Call {
    Call { body: "if (window.tandemMarkHost) { window.tandemMarkHost.remove(); }\nwindow.tandemMarkHost = null;\nwindow.tandemMarkRoot = null;\nreturn true;".into(), args: json!({}) }
}

/// `update-mark-positions` (`NMM:509-515`). Rule (task-1 review): the layout monitor must be
/// running in the isolated world before this runs (the script calls
/// `window.iTermLayoutChangeMonitor.reenableLayoutChangeMonitoring()` and throws without it), after
/// every navigation and after every world rebuild. None when a patch target is missing.
pub fn update_positions_script(secret: &str) -> Option<String> {
    let src = positions_patched(resource("update-mark-positions.js")?).ok()?;
    Some(as_body(&fill_template(&src, &[("SECRET", &js_literal_inner(secret))])))
}

/// The layout monitor (`NMM:431-433`).
pub const LAYOUT_MONITOR_JS: &str = include_str!("../resources/browser/layout-change-monitor.js");
/// `NMM:277-278`.
pub const DISABLE_MONITOR_JS: &str = "console.log('Swift disabling monitor.'); window.iTermLayoutChangeMonitor.disableLayoutChangeMonitoring()";
/// `NMM:273-274`.
pub const ENABLE_MONITOR_JS: &str = "console.log('Swift enabling monitor.'); window.iTermLayoutChangeMonitor.reenableLayoutChangeMonitoring()";

/// The annotation list of a page (`NM:74-80`, `NMM:341-358`): (guid, url, name) rows to `js_dict`
/// entries; a row whose URL carries no mark location is dropped (it is listed in menus, never shown).
pub fn annotation_marks(rows: &[(String, String, String)]) -> Vec<Value> {
    rows.iter().filter_map(|(guid, url, name)| Location::from_url(url).map(|l| l.js_dict(Some(name), guid))).collect()
}

/// Whether some update of `updates` (the answer of `update-mark-positions.js`) is valid for a mark in
/// `known` (`NMM:387-396`): a known `guid` and integer `scrollY`, `offsetY` and `y`. A non-array is false.
pub fn positions_changed(updates: &Value, known: &[&str]) -> bool {
    let int = |u: &Value, k: &str| u.get(k).is_some_and(|v| v.as_i64().is_some());
    updates.as_array().is_some_and(|list| {
        list.iter().any(|u| u.get("guid").and_then(Value::as_str).is_some_and(|g| known.contains(&g)) && int(u, "scrollY") && int(u, "offsetY") && int(u, "y"))
    })
}

/// Whether the cached marks hold one on the page `url` (without fragments, `NMM:367-381`).
pub fn page_has_marks(marks: &[Mark], url: &str) -> bool {
    marks.iter().any(|m| same_page(&m.url, url))
}

/// The guids of the cached marks on the page `url`.
pub fn page_guids(marks: &[Mark], url: &str) -> Vec<String> {
    marks.iter().filter(|m| same_page(&m.url, url)).map(|m| m.guid.clone()).collect()
}

/// The monitor's enable/disable counter must never go below 0 (task-5 re-review): the file rebuilds
/// its observer only when the counter is exactly 0, so an enable with no disable outstanding (a
/// navigation that never completed after the monitor was already on) would leave it at -1 and the
/// observer never back. The clamp is an exact-once patch that fails closed.
pub(crate) fn monitor_patched(src: &str) -> Result<String, String> {
    patch_once(src, "disabledCount -= 1;", "if (disabledCount > 0) { disabledCount -= 1; }")
}

/// The layout monitor as a runner body (`NMM:431-433`): the copied file is an expression. None when
/// the clamp patch does not apply (the monitor then does not run).
pub fn monitor_body() -> Option<String> {
    monitor_patched(LAYOUT_MONITOR_JS).ok().map(|s| as_body(&s))
}

/// Whether a page message of a Marks handler may be acted on: the isolated world's (the transport
/// says so; a Page message never), the main frame's, and not from one of Tandem's own pages (B5).
/// A Client source is the context's origin, which Chromium gives an `iterm2-about:` document as an
/// opaque "://" (probe run 2026-10-10b), so the source half never names a local page; the rule is
/// carried by the pane's URL (`marks_page` refuses a local page before any Marks message is read).
pub fn message_accepted(world: World, frame: Option<u64>, source: &str) -> bool {
    world == World::Client && frame.is_none() && !is_local_page(source)
}

/// The least time between two acts on page-driven messages of one kind (B2: the layout monitor posts
/// on every scroll, resize and mutation the page makes; its own debounce is 500 ms).
pub const MIN_GAP_MS: u64 = 1000;

/// A pane's Marks state in its web view: the marks secret (iTerm2's per-manager secret, `NMM:19-24`,
/// rotated with the world: B3), whether the layout monitor runs in the current isolated world (the
/// position update needs it: R13x5-3), the reveal that waits for its page (Task 5), and the rate gates.
#[derive(Debug)]
pub struct PageState {
    pub secret: String,
    pub monitor: bool,
    /// The reveal waiting for its page (B9): the mark and the navigation it is bound to.
    pub pending: Option<Pending>,
    layout_at: Option<std::time::Instant>,
    request_at: Option<std::time::Instant>,
}

impl PageState {
    pub fn new(secret: String) -> PageState {
        PageState { secret, monitor: false, pending: None, layout_at: None, request_at: None }
    }

    /// The world was rebuilt: a new secret, no monitor, no waiting gate. The pending reveal belongs to
    /// the page that is going and is dropped by Task 5's own rule, not here.
    pub fn rotate(&mut self, secret: String) {
        self.secret = secret;
        self.monitor = false;
        self.layout_at = None;
        self.request_at = None;
        self.pending = None;
    }

    /// A reveal that loads a page: the mark waits, armed for the Starting of the load it began (B9).
    pub fn arm_reveal(&mut self, mark: Mark) {
        self.pending = Some(Pending { mark, stage: Stage::Armed });
    }

    /// A reveal that needs no load: the mark is taken at once (`NMM:183-195`).
    pub fn take_pending(&mut self) -> Option<Mark> {
        self.pending.take().map(|p| p.mark)
    }

    /// A main-frame start (`redirect`: a hop of the navigation before it; `user`: the page's own
    /// gesture). The first start Tandem made after the reveal armed is the reveal's own navigation;
    /// a redirect hop of it moves the binding; any other start is a different navigation and ends
    /// the wait (B9).
    pub fn reveal_starting(&mut self, nav: u64, redirect: bool, user: bool) {
        let Some(p) = self.pending.as_mut() else { return };
        match p.stage {
            Stage::Armed if !redirect && !user => p.stage = Stage::Bound(nav),
            Stage::Bound(_) if redirect => p.stage = Stage::Bound(nav),
            _ => self.pending = None,
        }
    }

    /// The main frame finished navigation `nav`: the pending mark when it is the bound navigation
    /// and it loaded. A failed load keeps the mark (`NMM:223-225`); another navigation's end
    /// (stale, a back or forward) does nothing.
    pub fn reveal_navigated(&mut self, nav: u64, ok: bool) -> Option<Mark> {
        match self.pending.as_ref()?.stage {
            Stage::Bound(n) if n == nav && ok => self.take_pending(),
            _ => None,
        }
    }

    /// A new document or Tandem context: its scripts, monitor included, are gone.
    pub fn document_gone(&mut self) {
        self.monitor = false;
    }

    fn gate(at: &mut Option<std::time::Instant>, now: std::time::Instant) -> bool {
        if at.is_some_and(|t| now.saturating_duration_since(t) < std::time::Duration::from_millis(MIN_GAP_MS)) {
            return false;
        }
        *at = Some(now);
        true
    }

    /// A layout message may act (at most once per `MIN_GAP_MS`).
    pub fn layout_allowed(&mut self, now: std::time::Instant) -> bool {
        Self::gate(&mut self.layout_at, now)
    }

    /// A rename or delete request may act (the same bound; each opens a Tandem window).
    pub fn request_allowed(&mut self, now: std::time::Instant) -> bool {
        Self::gate(&mut self.request_at, now)
    }
}

/// What a layout message from the monitor gets: the position update, or (the rate gate refused it)
/// only the monitor brought back, because the monitor disabled itself to post and waits for the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutAction {
    Update,
    Reenable,
}

pub fn layout_action(allowed: bool) -> LayoutAction {
    if allowed { LayoutAction::Update } else { LayoutAction::Reenable }
}

/// A reveal waiting for its page.
#[derive(Clone, Debug, PartialEq)]
pub struct Pending {
    pub mark: Mark,
    stage: Stage,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Stage {
    /// The load was asked for; its start has not been seen.
    Armed,
    /// Bound to the navigation with this id.
    Bound(u64),
}

/// Why an add is refused (`BVC:591-606`, `NMM:142-145`; `Local` is Tandem's: B5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Not an http or https page (the Edit menu's gate).
    Scheme,
    /// A known method other than GET.
    Method,
    /// An `iterm2-about:` page: Tandem's own pages carry no marks and no annotations (B5).
    Local,
}

/// How the person asked for the mark: the Edit menu, the page's context menu, the Named Marks tool.
/// A mark is only ever added by one of these, never because a page asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddPath {
    Menu,
    ContextMenu,
    Tool,
}

fn scheme_lower(url: &str) -> String {
    url.split_once(':').map(|(s, _)| s.to_ascii_lowercase()).unwrap_or_default()
}

/// Whether `url` is one of Tandem's own pages (the privileged local-page bridge lives there, B5).
pub fn is_local_page(url: &str) -> bool {
    scheme_lower(url) == "iterm2-about"
}

/// The Edit menu's gate (`BVC:591-606`): http or https, and a known method that is exactly "GET".
pub fn can_add(url: &str, method: Option<&str>) -> bool {
    matches!(scheme_lower(url).as_str(), "http" | "https") && method == Some("GET")
}

/// What stops the prompt from opening (review M11): the menu applies `can_add`; the context menu and
/// the tool open it on any page (`BVC:1170-1188`, `BVC:1401-1425`) except Tandem's own.
pub fn gate_before_prompt(path: AddPath, url: &str, method: Option<&str>) -> Option<Refusal> {
    match path {
        AddPath::Menu => {
            if !matches!(scheme_lower(url).as_str(), "http" | "https") {
                Some(Refusal::Scheme)
            } else if method != Some("GET") {
                Some(Refusal::Method)
            } else {
                None
            }
        }
        AddPath::ContextMenu | AddPath::Tool => is_local_page(url).then_some(Refusal::Local),
    }
}

/// `add`'s own check (`NMM:142-145`): a known method other than GET throws; none passes.
pub fn gate_on_add(method: Option<&str>) -> Option<Refusal> {
    method.filter(|m| *m != "GET").map(|_| Refusal::Method)
}

/// Whether a Document-context request is this view's main document request (review M7): the request
/// is the view's, a document (not a sub-resource), and its URI is the one the view's last
/// main-frame NavigationStarting had. An iframe may load that same URL, so this alone does not
/// tell the main document: `MethodState::document` takes only the first request of a navigation.
pub fn document_method_applies(view_matches: bool, is_document: bool, request_uri: &str, last_main_nav_uri: Option<&str>) -> bool {
    view_matches && is_document && last_main_nav_uri == Some(request_uri)
}

/// The method of the page's last main-frame navigation, paired with the navigation that made it
/// (P5) and counted as an epoch: everything a pending add knows is the epoch it began in.
#[derive(Clone, Debug, PartialEq)]
pub struct MethodState {
    method: String,
    nav: u64,
    epoch: u64,
    /// The navigation's method was told: the first document request of a navigation is its own, any
    /// later one (an iframe loading the page's own URL) changes nothing (task-3 review important 1).
    told: bool,
}

impl Default for MethodState {
    fn default() -> Self {
        MethodState { method: "GET".into(), nav: 0, epoch: 0, told: false }
    }
}

impl MethodState {
    /// Every main-frame start, redirects included (a redirected load is a new document request):
    /// a navigation with no request seen counts as GET (Decision).
    pub fn starting(&mut self, nav: u64) {
        self.method = "GET".into();
        self.nav = nav;
        self.told = false;
        self.epoch += 1;
    }

    /// The document request of navigation `nav`; one of another navigation, or a second one of this
    /// navigation, is ignored.
    pub fn document(&mut self, nav: u64, method: &str) {
        if nav == self.nav && !self.told {
            self.told = true;
            self.method = method.to_string();
        }
    }

    /// The world or the view was remade: nothing known of the page, and no earlier epoch matches.
    pub fn reset(&mut self) {
        *self = MethodState { epoch: self.epoch + 1, ..MethodState::default() };
    }

    pub fn method(&self) -> &str {
        &self.method
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

/// The viewport centre in page coordinates (`BVC:1409-1412`): the page's answer is two finite
/// numbers in a sane range, else None.
pub fn center_point(answer: &Value) -> Option<(i64, i64)> {
    let a = answer.as_array()?;
    if a.len() != 2 {
        return None;
    }
    click_point(a[0].as_f64()?, a[1].as_f64()?)
}

/// A page point as the integers the script takes; None for a value that is not finite or lies
/// outside 0 to 1,000,000.
pub fn click_point(x: f64, y: f64) -> Option<(i64, i64)> {
    let ok = |v: f64| v.is_finite() && (0.0..=1_000_000.0).contains(&v);
    (ok(x) && ok(y)).then(|| (x.floor() as i64, y.floor() as i64))
}

/// The script finding the viewport centre (`BVC:1409-1412`).
pub const CENTER_JS: &str = "return [Math.floor(window.innerWidth / 2), Math.floor(window.innerHeight / 2)];";
/// The prompt's window title, as `open_mark_prompt`.
pub const PROMPT_TITLE: &str = "Set Named Mark";
/// The prompt's label (`BDV:16`).
pub const PROMPT_LABEL: &str = "Enter Mark Name";

/// What reveal does (`NMM:183-195`): scroll at once on the same page (the fragment is ignored), else
/// load the mark's URL and scroll when that load finishes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reveal {
    SamePage,
    Load(String),
}

pub fn reveal_step(mark_url: &str, current: &str) -> Reveal {
    if same_page(mark_url, current) {
        Reveal::SamePage
    } else {
        Reveal::Load(mark_url.to_string())
    }
}

/// Whether a rename or delete may still act: the mark is one this page shows (the cache, the page's
/// URL) and the page is the one the window opened for (the navigation epoch).
pub fn request_valid(cache: &[Mark], page_url: &str, guid: &str, opened_epoch: u64, epoch: u64) -> bool {
    opened_epoch == epoch && page_guids(cache, page_url).iter().any(|g| g == guid)
}

/// Whether a person's rename or delete may act: any mark the cache knows, on any page (iTerm2 lets the
/// menus and the tool edit every mark). An unknown id is refused.
pub fn person_edit_valid(cache: &[Mark], guid: &str) -> bool {
    cache.iter().any(|m| m.guid == guid)
}

/// The stored name as the rename window starts it: Tandem's sanitizer, never page text.
pub fn rename_text(stored: &str) -> String {
    crate::inert_files::display_text(stored, MAX_NAME, false)
}

/// The delete window's question: the stored name only.
pub fn delete_text(stored: &str) -> String {
    let name = rename_text(stored);
    let shown = if name.trim().is_empty() { "Unnamed Mark".to_string() } else { name };
    format!("Delete the named mark \"{shown}\"?")
}

/// One mark of the app-wide cache (`NM:8-21`).
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub guid: String,
    pub url: String,
    pub name: String,
    pub sort: i64,
}

/// Every mark of one store, newest first, and a counter that changes whenever they may have.
#[derive(Default)]
pub struct Cache {
    pub marks: Vec<Mark>,
    pub generation: u64,
}

impl Cache {
    /// `mutate` (`NMM:172`, `:294-299`): the mark renamed in place, order kept; false for an unknown guid.
    pub fn rename(&mut self, guid: &str, name: &str) -> bool {
        let Some(m) = self.marks.iter_mut().find(|m| m.guid == guid) else { return false };
        m.name = name.to_string();
        self.generation += 1;
        true
    }

    /// `removeFirst`: false for an unknown guid.
    pub fn remove(&mut self, guid: &str) -> bool {
        let Some(i) = self.marks.iter().position(|m| m.guid == guid) else { return false };
        self.marks.remove(i);
        self.generation += 1;
        true
    }

    pub fn get(&self, guid: &str) -> Option<&Mark> {
        self.marks.iter().find(|m| m.guid == guid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn mark(g: &str, url: &str) -> Mark {
        Mark { guid: g.into(), url: url.into(), name: format!("n{g}"), sort: 1 }
    }

    #[test]
    fn reveal_follows_iterm2() {
        assert_eq!(reveal_step("https://a.b/p#iterm-mark:x", "https://a.b/p#other"), Reveal::SamePage);
        assert_eq!(reveal_step("https://a.b/p#iterm-mark:x", "https://a.b/q"), Reveal::Load("https://a.b/p#iterm-mark:x".into()));
        assert_eq!(reveal_step("https://a.b/p?x=1#iterm-mark:x", "https://a.b/p"), Reveal::Load("https://a.b/p?x=1#iterm-mark:x".into()));
    }

    #[test]
    fn cache_follows_iterm2() {
        let mut c = Cache { marks: vec![mark("g2", "u"), mark("g1", "u")], generation: 4 };
        assert!(c.rename("g1", "z"));
        assert_eq!((c.marks[0].guid.as_str(), c.marks[1].name.as_str(), c.generation), ("g2", "z", 5));
        assert!(c.remove("g2"));
        assert_eq!(c.marks.len(), 1);
        assert_eq!(c.generation, 6);
        assert!(!c.remove("x") && !c.rename("x", "q"));
        assert_eq!(c.generation, 6);
        assert!(c.get("g1").is_some() && c.get("g2").is_none());
    }

    /// B9: a reveal that loads runs only on the end of its own navigation.
    #[test]
    fn a_reveal_is_bound_to_its_navigation() {
        let mut s = PageState::new("s".into());
        s.arm_reveal(mark("g", "https://a.b/p#iterm-mark:x"));
        // A stale end before any start does nothing.
        assert_eq!(s.reveal_navigated(7, true), None);
        s.reveal_starting(8, false, false);
        // Another navigation's end (stale, a hop it never saw) does nothing and keeps the wait.
        assert_eq!(s.reveal_navigated(7, true), None);
        // A failed load keeps the mark (`NMM:223-225`).
        assert_eq!(s.reveal_navigated(8, false), None);
        assert!(s.pending.is_some());
        // A redirect hop moves the binding.
        s.reveal_starting(9, true, false);
        assert_eq!(s.reveal_navigated(8, true), None);
        assert_eq!(s.reveal_navigated(9, true).map(|m| m.guid), Some("g".to_string()));
        assert!(s.pending.is_none());
        // A different navigation starting ends the wait.
        s.arm_reveal(mark("g", "u"));
        s.reveal_starting(10, false, false);
        s.reveal_starting(11, false, true);
        assert!(s.pending.is_none());
        // The person's own click before the reveal's start cancels it.
        s.arm_reveal(mark("g", "u"));
        s.reveal_starting(12, false, true);
        assert!(s.pending.is_none());
        // A rebuilt world drops it.
        s.arm_reveal(mark("g", "u"));
        s.rotate("t".into());
        assert!(s.pending.is_none());
        assert_eq!(s.take_pending(), None);
    }

    #[test]
    fn the_rate_gate_never_strands_the_monitor() {
        let mut s = PageState::new("s".into());
        let t = std::time::Instant::now();
        assert_eq!(layout_action(s.layout_allowed(t)), LayoutAction::Update);
        assert_eq!(layout_action(s.layout_allowed(t + std::time::Duration::from_millis(550))), LayoutAction::Reenable);
        assert_eq!(layout_action(s.layout_allowed(t + std::time::Duration::from_millis(1600))), LayoutAction::Update);
    }

    #[test]
    fn mark_windows_follow_the_native_rules() {
        let cache = vec![mark("g1", "https://a.b/p#iterm-mark:x"), mark("g2", "https://a.b/q#iterm-mark:y")];
        assert!(request_valid(&cache, "https://a.b/p", "g1", 3, 3));
        assert!(!request_valid(&cache, "https://a.b/p", "g1", 3, 4), "a new navigation");
        assert!(!request_valid(&cache, "https://a.b/p", "g2", 3, 3), "a mark of another page");
        assert!(!request_valid(&cache, "https://a.b/p", "nope", 3, 3), "an unknown mark");
        assert!(person_edit_valid(&cache, "g2"), "a person may edit a mark of another page");
        assert!(!person_edit_valid(&cache, "nope"), "an unknown mark");
        assert_eq!(delete_text("a\u{202e}b"), "Delete the named mark \"ab\"?");
        assert_eq!(delete_text(""), "Delete the named mark \"Unnamed Mark\"?");
        assert_eq!(rename_text(&"x".repeat(MAX_NAME + 9)).chars().count(), MAX_NAME);
    }

    fn first() -> Location {
        Location { xpath: "//p[1]".into(), offset_y: 4, scroll_y: 100, text_fragment: Some("a".into()), y: 120 }
    }

    #[test]
    fn location_follows_iterm2() {
        let v = json!({"xpath":"//p[1]","offsetY":4,"scrollY":100,"y":120,"textFragment":"a"});
        assert_eq!(Location::from_json(&v), Some(first()));
        assert_eq!(Location::from_json(&json!({"xpath":"//p[1]","offsetY":4,"scrollY":100,"textFragment":"a"})), None);
        assert_eq!(Location::from_json(&json!({"xpath":"//p[1]","offsetY":"4","scrollY":100,"y":120})), None, "a string is not an Int");
        assert_eq!(Location::from_json(&json!({"xpath":"//p[1]","offsetY":4.5,"scrollY":100,"y":120})), None);
        let l = Location::from_url("https://a.b/p#iterm-mark:xpath=%2F%2Fp%5B1%5D&offsetY=4&scrollY=100&y=120").unwrap();
        assert_eq!((l.xpath.as_str(), l.text_fragment.as_deref()), ("//p[1]", None));
        assert_eq!(Location::from_url("https://a.b/p#access_token=x"), None);
        assert_eq!(Location::from_url("https://a.b/p"), None);
        let skipped = Location::from_url("https://a.b/p#iterm-mark:junk&xpath=%2Fp&offsetY=1&scrollY=2&y=3").unwrap();
        assert_eq!(skipped.xpath, "/p");
        assert_eq!(Location::from_url("https://a.b/p#iterm-mark:xpath=%2Fp&offsetY=x&scrollY=2&y=3"), None);
        let q = Location::from_url("https://a.b/p#iterm-mark:xpath=%2Fp&offsetY=1&scrollY=2&y=3&textFragment=%25E2%2580%259Cq%25E2%2580%259D").unwrap();
        assert_eq!(q.text_fragment.as_deref(), Some("\u{201c}q\u{201d}"));
    }

    #[test]
    fn fragment_encoding_follows_urlcomponents() {
        let loc = Location { xpath: "//p[1]".into(), offset_y: 4, scroll_y: 100, text_fragment: Some("100%".into()), y: 120 };
        let url = mark_url("https://a.b/p", &loc).unwrap();
        assert_eq!(url, "https://a.b/p#iterm-mark:xpath=//p%5B1%5D&offsetY=4&scrollY=100&y=120&textFragment=100%2525");
        assert_eq!(Location::from_url(&url), Some(loc));
        let pct = Location { text_fragment: Some("%41".into()), ..first() };
        let url = mark_url("https://a.b/p", &pct).unwrap();
        assert!(url.ends_with("textFragment=%252541"), "{url}");
        assert_eq!(Location::from_url(&url).unwrap().text_fragment.as_deref(), Some("%41"), "never A");
        let quotes = Location { text_fragment: Some("\u{201c}q\u{201d}".into()), ..first() };
        assert_eq!(Location::from_url(&mark_url("https://a.b/p", &quotes).unwrap()), Some(quotes));
        let attr = Location { xpath: "//p[@class=\"a b\"]".into(), ..first() };
        let url = mark_url("https://a.b/p", &attr).unwrap();
        assert!(url.contains("%22a%20b%22"), "{url}");
        assert_eq!(Location::from_url(&url), Some(attr));
        assert_eq!(encode_fragment("a%b[c]^\u{60}{|}\\<> \u{e9}"), "a%25b%5Bc%5D%5E%60%7B%7C%7D%5C%3C%3E%20%C3%A9");
        assert_eq!(encode_fragment("-._~!$&'()*+,;=:@/?"), "-._~!$&'()*+,;=:@/?");
        let second = Location::from_url("https://a.b/p#iterm-mark:xpath=%2F%2Fp&offsetY=1&scrollY=2&y=3&textFragment=100%25").unwrap();
        assert_eq!(second.text_fragment, None, "the second decode of 100% fails");
        assert_eq!(crate::marks::percent_decode_strict("a%2"), None);
        assert_eq!(crate::marks::percent_decode_strict("%E9"), None);
        assert_eq!(crate::marks::percent_decode_strict("%C3%A9").as_deref(), Some("\u{e9}"));
        assert_eq!(crate::marks::percent_decode_strict("%"), None);
    }

    #[test]
    fn fragment_round_trip_follows_iterm2() {
        let loc = first();
        assert_eq!(loc.fragment(), "iterm-mark:xpath=//p[1]&offsetY=4&scrollY=100&y=120&textFragment=a");
        let url = mark_url("https://a.b/p?q=1#old", &loc).unwrap();
        assert!(url.starts_with("https://a.b/p?q=1#iterm-mark:"), "{url}");
        assert_eq!(Location::from_url(&url), Some(loc.clone()));
        let none = Location { text_fragment: None, ..first() };
        assert!(!none.fragment().contains("textFragment"));
        let empty = Location { text_fragment: Some(String::new()), ..first() };
        assert!(!empty.fragment().contains("textFragment"));
        let d = loc.js_dict(None, "g");
        assert_eq!(d["name"], "Unnamed");
        assert_eq!(d["guid"], "g");
        assert_eq!(d["y"], 120);
        assert_eq!(d["offsetY"], 4);
        assert_eq!(none.js_dict(Some("n"), "g")["textFragment"], "");
        assert_eq!(none.js_dict(Some("n"), "g")["name"], "n");
        assert_eq!(without_fragment("https://a.b/p?q#f"), "https://a.b/p?q");
        assert_eq!(without_fragment("https://a.b/p?q"), "https://a.b/p?q");
        assert!(same_page("https://a.b/p#x", "https://a.b/p"));
        assert!(!same_page("https://a.b/p?x", "https://a.b/p"));
    }

    #[test]
    fn updates_follow_iterm2() {
        let del = json!({"guid":"g","requestDelete":true,"sessionSecret":"s"});
        assert_eq!(update_of(&del, "s"), Some(Update::RequestDelete { guid: "g".into() }));
        let ren = json!({"guid":"g","requestRename":true,"sessionSecret":"s"});
        assert_eq!(update_of(&ren, "s"), Some(Update::RequestRename { guid: "g".into() }));
        assert_eq!(update_of(&ren, "t"), None);
        assert_eq!(update_of(&json!({"guid":"g","requestRename":true}), "s"), None);
        assert_eq!(update_of(&json!({"guid":"g","requestRename":true,"sessionSecret":""}), ""), None, "no secret is never a match");
        assert_eq!(update_of(&json!({"requestRename":true,"sessionSecret":"s"}), "s"), None);
        assert_eq!(update_of(&json!({"guid":"g","requestDelete":false,"sessionSecret":"s"}), "s"), None);
        // Class fix (B2): a message that carries a name or a delete changes nothing; only the native window does.
        assert_eq!(update_of(&json!({"guid":"g","name":"n","sessionSecret":"s"}), "s"), None);
        assert_eq!(update_of(&json!({"guid":"g","delete":true,"sessionSecret":"s"}), "s"), None);
        assert_eq!(update_of(&json!({"guid":"g","requestDelete":true,"name":"n","sessionSecret":"s"}), "s"), Some(Update::RequestDelete { guid: "g".into() }), "a name beside a request is ignored");
        assert!(is_layout_change(&json!({"type":"layoutChange"})));
        assert!(!is_layout_change(&json!({"type":"x"})));
        assert!(!is_layout_change(&json!("layoutChange")));
    }

    #[test]
    fn page_data_is_bounded() {
        let long = "x".repeat(MAX_XPATH + 1);
        assert_eq!(Location::from_json(&json!({"xpath":long,"offsetY":1,"scrollY":2,"y":3})), None);
        let long = "x".repeat(MAX_TEXT + 1);
        assert_eq!(Location::from_json(&json!({"xpath":"/p","offsetY":1,"scrollY":2,"y":3,"textFragment":long})), None);
        let big = Location { text_fragment: Some("\u{201c}".repeat(MAX_TEXT)), ..first() };
        assert_eq!(mark_url("https://a.b/p", &big), None, "the URL stays within MAX_URL");
        assert_eq!(mark_url(&format!("https://a.b/{}", "p".repeat(MAX_URL)), &first()), None);
        let name = "n".repeat(MAX_NAME + 1);
        assert_eq!(update_of(&json!({"guid":"g","name":name,"sessionSecret":"s"}), "s"), None);
        let guid = "g".repeat(MAX_GUID + 1);
        assert_eq!(update_of(&json!({"guid":guid,"delete":true,"sessionSecret":"s"}), "s"), None);
        let marks: Vec<Value> = (0..MAX_MARKS + 5).map(|i| json!({"name":"m","guid":i.to_string()})).collect();
        let call = show_call(&marks, "s").unwrap();
        assert_eq!(call.args["marksArg"].as_array().unwrap().len(), MAX_MARKS);
    }

    #[test]
    fn scripts_follow_iterm2() {
        let x = xpath_script(10, 20).unwrap();
        assert!(x.contains("parseInt(\"10\")") && x.contains("parseInt(\"20\")"), "{x}");
        assert!(x.contains("getTextFragmentData") && !x.contains("{{"));
        assert!(x.starts_with("return (function"));
        assert_eq!(js_literal_inner("a\"b\\c</script>\u{2028}\u{2029}"), "a\\\"b\\\\c\\u003c/script>\\u2028\\u2029");
        // The page's text never reaches the script source (B4): it travels as a typed argument.
        let hostile = "//a[@id=\"x\\\"y\"]</script>\");alert(1);(\"{{SECRET}}\u{2028}";
        let loc = Location { xpath: hostile.into(), offset_y: 7, scroll_y: 8, text_fragment: Some(hostile.into()), y: 9 };
        let call = navigate_call(&loc).unwrap();
        assert_eq!(call.args["xpathArg"], hostile);
        assert_eq!(call.args["textFragmentArg"], hostile);
        for bad in ["alert(1)", "</script>", "x\\\"y", "{{"] {
            assert!(!call.body.contains(bad), "{bad}");
        }
        assert!(call.body.starts_with("return (function") && call.body.contains("var xpath = xpathArg;") && call.body.contains("var textFragment = textFragmentArg;"));
        assert!(call.body.contains("parseInt(\"7\")") && call.body.contains("parseInt(\"8\")") && call.body.contains("parseInt(\"9\")"));
        let none = navigate_call(&Location { text_fragment: None, ..loc.clone() }).unwrap();
        assert_eq!(none.args["textFragmentArg"], "");
        assert!(show_call(&[], "s").is_none());
        let shown = show_call(&[json!({"name":"{{SECRET}}","guid":"g"})], "s3cr3t").unwrap();
        assert_eq!(shown.body.matches("var sessionSecret = \"s3cr3t\"").count(), 1);
        assert!(!shown.body.contains("{{") && shown.body.contains("var marks = marksArg;"));
        assert_eq!(shown.args["marksArg"][0]["name"], "{{SECRET}}");
        assert!(!shown.args.to_string().contains("s3cr3t"));
        let u = update_positions_script("s").unwrap();
        assert!(u.contains("var secret = \"s\"") && u.contains("locateMark") && !u.contains("{{"));
        assert!(u.starts_with("return (function"));
        assert!(u.contains("window.tandemMarkRoot ? window.tandemMarkRoot.querySelectorAll") && !u.contains("document.querySelectorAll('.iterm-mark-annotation"), "the annotations are read from the shadow root");
        assert_eq!(LAYOUT_MONITOR_JS, resource("layout-change-monitor.js").unwrap(), "one copy of the file");
        assert!(positions_patched("nothing").is_err());
        let tricky = update_positions_script("a\"b").unwrap();
        assert!(tricky.contains("var secret = \"a\\\"b\""), "the secret is escaped too");
        assert!(LAYOUT_MONITOR_JS.contains("iTerm2MarkLayoutUpdate") && LAYOUT_MONITOR_JS.contains("iTermLayoutChangeMonitor"));
        assert!(DISABLE_MONITOR_JS.contains("disableLayoutChangeMonitoring()") && ENABLE_MONITOR_JS.contains("reenableLayoutChangeMonitoring()"));
        assert_eq!(MESSAGE_HANDLER, "iTerm2NamedMarkUpdate");
        assert_eq!(LAYOUT_HANDLER, "iTerm2MarkLayoutUpdate");
        assert_eq!(FRAGMENT_PREFIX, "iterm-mark:");
    }

    #[test]
    fn annotation_buttons_only_request() {
        let shown = show_call(&[json!({"name":"n","guid":"g"})], "s").unwrap().body;
        assert_eq!(shown.matches("e.isTrusted").count(), 1, "the real-button test");
        assert_eq!(shown.matches("tandemReal(e, ").count(), 3, "the definition, Delete and Rename");
        assert_eq!(shown.matches("attachShadow({mode: 'closed'})").count(), 1, "the box is in a closed shadow root");
        assert!(shown.contains("shadow.elementFromPoint(e.clientX, e.clientY) === button") && shown.contains("shadow.appendChild(container)"));
        // Requests only: no name leaves the page, nothing is removed in it, the input takes no key.
        assert!(shown.contains("requestDelete: true,") && shown.contains("requestRename: true,"));
        assert!(!shown.contains("input.value,") && !shown.contains("delete: true") && !shown.contains("annotation.remove()") && !shown.contains("mark.name = input.value"));
        assert!(shown.contains("input.type = 'hidden'") && shown.contains("saveButton.textContent = 'Rename'"));
        // The host is styled inline, important, after `all: initial`; the old box goes by Tandem's reference.
        assert!(shown.contains("host.style.cssText = 'all: initial !important;") && shown.contains("position: absolute !important") && shown.contains("height: 0 !important"));
        assert!(shown.contains("document.body.appendChild(host)") && !shown.contains("getElementById") && !shown.contains("iterm-mark-annotations"));
        assert!(shown.contains("var existingContainer = window.tandemMarkHost;") && shown.contains("window.tandemMarkRoot = null;"));
        // The patch fails closed: a target that moved or repeats is an error, not a silent skip.
        let src = crate::browser_js::resource("show-named-mark-annotations.js").unwrap();
        assert!(trust_patched(src).is_ok());
        assert!(trust_patched(&src.replace("deleteButton.addEventListener('click', function(e) {", "deleteButton.addEventListener('click', function(ev) {")).is_err());
        for target in ["delete: true,", "name: input.value,", "annotation.remove();", "input.type = 'text';", "document.getElementById('iterm-mark-annotations')", "existingContainer.remove();"] {
            assert!(trust_patched(&src.replace(target, "")).is_err(), "{target}: a moved target fails closed");
        }
        assert!(trust_patched(&format!("{src}\n{src}")).is_err());
        assert!(patch_once("abc", "b", "x").is_ok() && patch_once("abcb", "b", "x").is_err() && patch_once("ac", "b", "x").is_err());
    }

    /// Probe run 2026-10-10b (R13x5-43): a closed shadow root keeps the name out of `innerText`, but
    /// Chromium's `window.find` and a select-all selection still read text nodes inside it (measured in
    /// Chromium: both found "first"). The name is drawn as generated content (`::before` with
    /// `attr()`), which neither finds nor selects; no text node holds it.
    #[test]
    fn annotation_names_are_not_page_text() {
        let shown = show_call(&[json!({"name":"n","guid":"g"})], "s").unwrap().body;
        assert!(!shown.contains("textContent = mark.name"), "no text node holds the name");
        assert_eq!(shown.matches("setAttribute('data-tandem-name', mark.name)").count(), 2, "the collapsed view and the title");
        assert_eq!(shown.matches("[data-tandem-name]::before { content: attr(data-tandem-name) !important; }").count(), 1);
        assert!(shown.find("shadow.appendChild(tandemNames)").unwrap() < shown.find("shadow.appendChild(container)").unwrap(), "the rule lives in the closed root");
        assert!(shown.contains("input.value = '';") && !shown.contains("input.value = mark.name"), "the hidden input holds no name either");
        let src = crate::browser_js::resource("show-named-mark-annotations.js").unwrap();
        for target in ["collapsedView.textContent = mark.name;", "markTitle.textContent = mark.name;", "input.value = mark.name || '';"] {
            assert!(trust_patched(&src.replace(target, "")).is_err(), "{target}: a moved target fails closed");
        }
    }

    /// Minor 5: the fragment is `&`-split and `&` survives its encoding, so an XPath holding one is
    /// refused (a page could otherwise write its own `textFragment` pair); `=` is safe (a pair splits
    /// at its first one).
    #[test]
    fn an_ampersand_in_an_xpath_is_refused() {
        let evil = "//a[@id=\"x&textFragment=evil\"]";
        assert_eq!(Location::from_json(&json!({"xpath":evil,"offsetY":1,"scrollY":2,"y":3})), None);
        assert_eq!(Location::from_url("https://a/#iterm-mark:xpath=%2Fa%26b&offsetY=1&scrollY=2&y=3"), None);
        let eq = Location::from_json(&json!({"xpath":"//a[@id=\"x\"]","offsetY":1,"scrollY":2,"y":3})).unwrap();
        let url = mark_url("https://a/p", &eq).unwrap();
        assert_eq!(Location::from_url(&url), Some(eq));
    }

    #[test]
    fn add_refusals_follow_iterm2() {
        assert!(can_add("https://a", Some("GET")));
        assert!(can_add("HTTP://a", Some("GET")));
        assert!(!can_add("https://a", Some("POST")));
        assert!(!can_add("https://a", None));
        assert!(!can_add("file:///c:/x", Some("GET")));
        assert!(!can_add("iterm2-about:history", Some("GET")));
        assert_eq!(gate_before_prompt(AddPath::Menu, "about:blank", Some("GET")), Some(Refusal::Scheme));
        assert_eq!(gate_before_prompt(AddPath::Menu, "https://a", Some("POST")), Some(Refusal::Method));
        assert_eq!(gate_before_prompt(AddPath::Menu, "https://a", Some("GET")), None);
        assert_eq!(gate_before_prompt(AddPath::ContextMenu, "about:blank", Some("POST")), None);
        assert_eq!(gate_before_prompt(AddPath::Tool, "file:///c:/x", None), None);
        // Tandem's own pages carry no marks, whichever way they are asked (B5).
        assert_eq!(gate_before_prompt(AddPath::ContextMenu, "iterm2-about:history", Some("GET")), Some(Refusal::Local));
        assert_eq!(gate_before_prompt(AddPath::Tool, "ITERM2-ABOUT:welcome", None), Some(Refusal::Local));
        assert_eq!(gate_on_add(None), None);
        assert_eq!(gate_on_add(Some("GET")), None);
        assert_eq!(gate_on_add(Some("POST")), Some(Refusal::Method));
        assert_eq!(PROMPT_LABEL, "Enter Mark Name");
        assert_eq!(PROMPT_TITLE, "Set Named Mark");
    }

    #[test]
    fn document_method_follows_windows() {
        let u = Some("https://a/r");
        assert!(document_method_applies(true, true, "https://a/r", u));
        assert!(!document_method_applies(false, true, "https://a/r", u));
        assert!(!document_method_applies(true, false, "https://a/r", u));
        assert!(!document_method_applies(true, true, "https://a/frame", u));
        assert!(!document_method_applies(true, true, "https://a/r", None));
    }

    /// A method belongs to the navigation that made it (P5); every start resets to GET; a reset
    /// moves the epoch, so a prompt opened before it can no longer add.
    #[test]
    fn method_tracking_follows_iterm2() {
        let mut m = MethodState::default();
        assert_eq!(m.method(), "GET");
        m.starting(7);
        assert_eq!(m.method(), "GET", "a navigation with no request seen is GET");
        m.document(7, "POST");
        assert_eq!(m.method(), "POST");
        m.document(6, "GET");
        assert_eq!(m.method(), "POST", "another navigation's request is ignored");
        let first = m.epoch();
        m.starting(8);
        assert_eq!(m.method(), "GET");
        assert_ne!(m.epoch(), first);
        m.document(7, "POST");
        assert_eq!(m.method(), "GET", "the old navigation's late request is ignored");
        let e = m.epoch();
        m.reset();
        assert_ne!(m.epoch(), e);
        assert_eq!(m.method(), "GET");
    }

    #[test]
    fn click_points_are_validated() {
        assert_eq!(center_point(&json!([400, 300])), Some((400, 300)));
        assert_eq!(center_point(&json!([400.9, 300.2])), Some((400, 300)));
        assert_eq!(center_point(&json!([400])), None);
        assert_eq!(center_point(&json!(["a", 1])), None);
        assert_eq!(center_point(&json!([-1, 1])), None);
        assert_eq!(center_point(&json!([1e12, 1])), None);
        assert_eq!(center_point(&json!(null)), None);
        assert_eq!(click_point(3.0, 4.0), Some((3, 4)));
        assert_eq!(click_point(f64::NAN, 4.0), None);
        assert_eq!(click_point(f64::INFINITY, 4.0), None);
    }


    #[test]
    fn annotations_follow_iterm2() {
        let rows = vec![
            ("g".to_string(), "https://a.b/p#iterm-mark:xpath=%2F%2Fp&offsetY=1&scrollY=2&y=3".to_string(), "n".to_string()),
            ("h".to_string(), "https://a.b/p".to_string(), "m".to_string()),
        ];
        let got = annotation_marks(&rows);
        assert_eq!(got.len(), 1, "a URL with no location is dropped");
        assert_eq!(got[0]["guid"], "g");
        assert_eq!(got[0]["name"], "n");
        assert_eq!(got[0]["xpath"], "//p");
        assert_eq!(got[0]["offsetY"], 1);
        assert_eq!(got[0]["y"], 3);
        assert_eq!(got[0]["textFragment"], "");
        assert!(annotation_marks(&[]).is_empty());
        // Nothing is run for no marks (`NMM:345-347`, `:480-483`).
        assert!(show_call(&annotation_marks(&[]), "S").is_none());
    }

    #[test]
    fn layout_updates_follow_iterm2() {
        let good = json!([{"guid":"g","scrollY":1,"offsetY":2,"y":3}]);
        assert!(positions_changed(&good, &["g"]));
        assert!(!positions_changed(&good, &["other"]), "an unknown guid");
        assert!(!positions_changed(&json!([{"guid":"g","scrollY":1,"offsetY":2}]), &["g"]), "a missing y");
        assert!(!positions_changed(&json!([{"guid":"g","scrollY":1.5,"offsetY":2,"y":3}]), &["g"]), "not an integer");
        assert!(!positions_changed(&json!({"guid":"g"}), &["g"]), "not an array");
        assert!(positions_changed(&json!([{"guid":"x"}, {"guid":"g","scrollY":1,"offsetY":2,"y":3}]), &["g"]), "some entry");
        let marks = vec![Mark { guid: "g".into(), url: "https://a.b/p#iterm-mark:x".into(), name: "n".into(), sort: 1 }];
        assert!(page_has_marks(&marks, "https://a.b/p"));
        assert!(page_has_marks(&marks, "https://a.b/p#other"));
        assert!(!page_has_marks(&marks, "https://a.b/q"));
        assert!(!page_has_marks(&[], "https://a.b/p"));
        assert_eq!(page_guids(&marks, "https://a.b/p"), vec!["g".to_string()]);
        assert!(page_guids(&marks, "https://a.b/q").is_empty());
        assert!(monitor_body().unwrap().starts_with("return "));
    }

    /// Cancel-then-layout: a navigation that never completes sends an enable with no disable
    /// outstanding. The counter is modelled as the patched file keeps it; unpatched it reaches -1 and
    /// the next layout message's disable/enable pair (count 0 then 1 then 0 is fine only from 0).
    #[test]
    fn the_monitor_counter_never_goes_below_zero() {
        let p = monitor_patched(LAYOUT_MONITOR_JS).unwrap();
        assert!(p.contains("if (disabledCount > 0) { disabledCount -= 1; }") && !p.contains("\n        disabledCount -= 1;"));
        assert_eq!(p.matches("disabledCount -= 1").count(), 1);
        assert!(monitor_patched("nothing").is_err() && monitor_patched("disabledCount -= 1; disabledCount -= 1;").is_err(), "fails closed");
        // The model of the two counters: cancel (enable at 0), then a layout message (disable, host enable).
        let step = |c: i32, enable: bool, clamp: bool| if enable { if clamp && c <= 0 { c } else { c - 1 } } else { c + 1 };
        let run = |clamp: bool| {
            let c = step(0, true, clamp); // cancelled navigation: enable with nothing outstanding
            let c = step(c, false, clamp); // the monitor posts layoutChange and disables itself
            step(c, true, clamp) // the host brings it back
        };
        assert_ne!(run(false), 0, "unpatched: the observer never comes back");
        assert_eq!(run(true), 0, "patched: the observer is rebuilt");
    }

    #[test]
    fn marks_messages_are_bounded_and_isolated() {
        assert!(message_accepted(World::Client, None, "tandem-client"));
        assert!(!message_accepted(World::Page, None, "https://a.b"), "the page world never");
        assert!(!message_accepted(World::Client, Some(4), "tandem-client"), "a subframe never");
        assert!(!message_accepted(World::Client, None, "iterm2-about:history"), "Tandem's pages never");
        let t = std::time::Instant::now();
        let mut s = PageState::new("S".into());
        assert!(s.layout_allowed(t));
        assert!(!s.layout_allowed(t + std::time::Duration::from_millis(999)));
        assert!(s.layout_allowed(t + std::time::Duration::from_millis(1000)));
        assert!(s.request_allowed(t) && !s.request_allowed(t));
        s.monitor = true;
        s.document_gone();
        assert!(!s.monitor);
        s.monitor = true;
        s.rotate("T".into());
        assert!(!s.monitor && s.secret == "T" && s.layout_allowed(t) && s.request_allowed(t));
    }

    #[test]
    fn one_method_per_navigation() {
        let mut m = MethodState::default();
        m.starting(7);
        m.document(7, "POST");
        m.document(7, "GET");
        assert_eq!(m.method(), "POST", "an iframe loading the page's URL cannot change it");
        m.starting(8);
        assert_eq!(m.method(), "GET");
        m.document(7, "POST");
        assert_eq!(m.method(), "GET", "another navigation's");
        m.document(8, "POST");
        assert_eq!(m.method(), "POST");
    }

    #[test]
    fn text_fragment_ampersand_cannot_write_a_pair() {
        let loc = Location { text_fragment: Some("a&xpath=//x&y=0".into()), ..first() };
        let url = mark_url("https://a.b/p", &loc).unwrap();
        let back = Location::from_url(&url).unwrap();
        assert_eq!(back, loc, "round trips, nothing rewritten");
        assert_eq!(back.xpath, "//p[1]");
    }
}

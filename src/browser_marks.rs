//! Browser named marks, the pure half (milestone 13x5, Task 1): iTerm2's `Location`, the
//! `iterm-mark:` URL fragment and its encoding, the page messages, and the filled page scripts
//! (`iTermBrowserNamedMarkManager.swift`, `BrowserNamedMarks.swift`). The page is untrusted: its
//! answers are validated and bounded here, and no page-derived text is ever pasted into script
//! source. A script that takes a page-derived value gets it as a typed argument of the isolated
//! world's runner (`Call`), through a fail-closed load-time patch of the copied file, which stays
//! byte-identical on disk. Only generated tokens and integers are substituted into source.

use serde_json::{Value, json};

use crate::browser_js::{fill_template, js_string, resource};
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
    fn bounded(self) -> Option<Location> {
        (self.xpath.len() <= MAX_XPATH && self.text_fragment.as_ref().is_none_or(|t| t.len() <= MAX_TEXT)).then_some(self)
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
            out.push_str(&encode_allowed(t));
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

/// What the annotation box asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Update {
    Rename { guid: String, name: String },
    Delete { guid: String },
}

fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// `handleMessage` (`NMM:228-252`): the secret must match (an empty one never does), `delete: true`
/// wins, else a `name` renames. Whether the guid names a mark is the caller's check.
pub fn update_of(body: &Value, secret: &str) -> Option<Update> {
    let guid = body.get("guid")?.as_str()?;
    let sent = body.get("sessionSecret")?.as_str()?;
    if secret.is_empty() || !same_secret(sent, secret) || guid.is_empty() || guid.len() > MAX_GUID {
        return None;
    }
    if body.get("delete").and_then(Value::as_bool) == Some(true) {
        return Some(Update::Delete { guid: guid.to_string() });
    }
    let name = body.get("name")?.as_str()?;
    (name.chars().count() <= MAX_NAME).then(|| Update::Rename { guid: guid.to_string(), name: name.to_string() })
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
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub body: String,
    pub args: Value,
}

/// Replaces `from` by `to` when `from` occurs exactly once (a load-time patch fails closed).
pub(crate) fn patch_once(src: &str, from: &str, to: &str) -> Result<String, String> {
    match src.matches(from).count() {
        1 => Ok(src.replacen(from, to, 1)),
        n => Err(format!("patch target {from:?} found {n} times")),
    }
}

/// The annotation box acts only on events the person caused, aimed at the real button (B2, the
/// carried rule plus the task-1 review's clickjacking ruling). Three layers: the box is built in a
/// closed shadow root made in the isolated world (a page cannot reach its input or styles, nor
/// listen inside it); a handler ignores an untrusted event; and Save and Delete act only when the
/// trusted pointer event's point holds that very button (`elementFromPoint` on the shadow root,
/// which answers the host, not the button, when a page overlay covers it) or, for a keyboard
/// activation (`detail` 0), when that button has focus. The Enter key's own `saveButton.click()` is
/// untrusted, so it carries a flag that only this world's wrapper of the button holds.
pub(crate) fn trust_patched(src: &str) -> Result<String, String> {
    let s = patch_once(src, "if (e.key === 'Enter') {", "if (e.isTrusted && e.key === 'Enter') {")?;
    let s = patch_once(&s, "saveButton.click(); // Trigger the save button click", "saveButton.tandemKey = true; saveButton.click(); saveButton.tandemKey = false; // Trigger the save button click")?;
    let s = patch_once(&s, "deleteButton.addEventListener('click', function(e) {", "deleteButton.addEventListener('click', function(e) { if (!tandemReal(e, deleteButton)) { return; }")?;
    let s = patch_once(&s, "saveButton.addEventListener('click', function(e) {", "saveButton.addEventListener('click', function(e) { if (!saveButton.tandemKey && !tandemReal(e, saveButton)) { return; }")?;
    shadow_patched(&s)
}

/// The annotations live in a closed shadow root under a host with the old container's id; the root
/// is kept as `window.tandemMarkRoot` (the isolated world's window, which a page cannot see) for
/// `update-mark-positions.js`. `tandemReal` is the real-button test.
fn shadow_patched(src: &str) -> Result<String, String> {
    let s = patch_once(
        src,
        "var container = document.createElement('div');",
        "var host = document.createElement('div');\n    host.id = 'iterm-mark-annotations';\n    var shadow = host.attachShadow({mode: 'closed'});\n    window.tandemMarkRoot = shadow;\n    function tandemReal(e, button) {\n        if (!e.isTrusted) { return false; }\n        if (e.detail === 0) { return shadow.activeElement === button; }\n        return shadow.elementFromPoint(e.clientX, e.clientY) === button;\n    }\n    var container = document.createElement('div');",
    )?;
    let s = patch_once(&s, "container.id = 'iterm-mark-annotations';", "")?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
        let del = json!({"guid":"g","delete":true,"sessionSecret":"s"});
        assert_eq!(update_of(&del, "s"), Some(Update::Delete { guid: "g".into() }));
        let ren = json!({"guid":"g","name":"n","sessionSecret":"s"});
        assert_eq!(update_of(&ren, "s"), Some(Update::Rename { guid: "g".into(), name: "n".into() }));
        assert_eq!(update_of(&json!({"guid":"g","name":"","sessionSecret":"s"}), "s"), Some(Update::Rename { guid: "g".into(), name: String::new() }));
        assert_eq!(update_of(&ren, "t"), None);
        assert_eq!(update_of(&json!({"guid":"g","name":"n"}), "s"), None);
        assert_eq!(update_of(&json!({"guid":"g","name":"n","sessionSecret":""}), ""), None, "no secret is never a match");
        assert_eq!(update_of(&json!({"name":"n","sessionSecret":"s"}), "s"), None);
        assert_eq!(update_of(&json!({"guid":"g","delete":false,"name":"n","sessionSecret":"s"}), "s"), Some(Update::Rename { guid: "g".into(), name: "n".into() }));
        assert_eq!(update_of(&json!({"guid":"g","delete":false,"sessionSecret":"s"}), "s"), None);
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
    fn annotation_buttons_ignore_untrusted_events() {
        let shown = show_call(&[json!({"name":"n","guid":"g"})], "s").unwrap().body;
        assert_eq!(shown.matches("e.isTrusted").count(), 2, "keydown and the real-button test");
        assert_eq!(shown.matches("tandemReal(e, ").count(), 3, "the definition, Delete and Save");
        assert_eq!(shown.matches("attachShadow({mode: 'closed'})").count(), 1, "the box is in a closed shadow root");
        assert!(shown.contains("shadow.elementFromPoint(e.clientX, e.clientY) === button") && shown.contains("shadow.appendChild(container)"));
        assert!(!shown.contains("container.id") && shown.contains("host.id = 'iterm-mark-annotations'"));
        // The patch fails closed: a target that moved or repeats is an error, not a silent skip.
        let src = crate::browser_js::resource("show-named-mark-annotations.js").unwrap();
        assert!(trust_patched(src).is_ok());
        assert!(trust_patched(&src.replace("deleteButton.addEventListener('click', function(e) {", "deleteButton.addEventListener('click', function(ev) {")).is_err());
        assert!(trust_patched(&format!("{src}\n{src}")).is_err());
        assert!(patch_once("abc", "b", "x").is_ok() && patch_once("abcb", "b", "x").is_err() && patch_once("ac", "b", "x").is_err());
    }
}

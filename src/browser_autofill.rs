//! Browser autofill's pure model (milestone 13x7, Task 1): iTerm2's four autofill scripts and the
//! load-time patches Tandem lays on two of them, the request parser, the contact card and its field
//! values (`prepareAutofillData`), the write script's typed call, and the per-pane operation queue
//! (iTerm2's `AsyncMutex`). Web content is untrusted: a request's fields are page text, so they are
//! bounded here and reach a script only as typed arguments, never as template text. Card values and
//! fills are secrets: they never derive `Debug` over their values and a fill wipes itself when dropped.
//!
//! iTerm2 cites: `AH` Browser/Autofill/iTermBrowserAutofillHandler.swift, `AW` ...Writer.swift,
//! `ACS` ...ContactSource.swift, `ACJ` autofill-core.js, `ADJ` autofill-detector.js, `AFJ`
//! autofill-fill-all.js, `AWJ` autofill-write.js.

use std::collections::VecDeque;
use std::fmt;

use serde_json::{Value, json};

use crate::browser_js::{fill_template, resource};
use crate::browser_marks::{Call, patch_once};

/// The message handler the scripts post to (`AH:13`).
pub const HANDLER: &str = "iTermAutofillHandler";
/// The only message type the handler takes (`AH:36`).
pub const REQUEST_TYPE: &str = "autofillRequest";
/// Every kind of field the detector names (`ACJ:4-90`), in the order a card fills them.
pub const FIELD_TYPES: [&str; 12] = ["firstName", "lastName", "fullName", "email", "phone", "address1", "address2", "city", "state", "zip", "country", "company"];
/// The line the detector patch puts first in the button's click handler (R13x67-2, iTerm2's bug: a
/// page script's `.click()` posted a request).
pub const TRUSTED_CHECK: &str = "if (!e.isTrusted) return;";
/// A request names at most this many fields (page-driven bound; `MAX_MESSAGE` alone allows thousands).
pub const MAX_FIELDS: usize = 64;
/// A field's type, id or name longer than this (in bytes) drops that field.
pub const MAX_TEXT: usize = 256;
/// Fills waiting behind a running one, per pane.
pub const MAX_QUEUE: usize = 128;

fn file(name: &str) -> &'static str {
    resource(name).expect("a copied autofill script is in the resource table")
}

/// `autofill-core.js` as iTerm2 wrote it (the detector and Fill All include it).
pub fn core_js() -> &'static str {
    file("autofill-core.js")
}

/// `autofill-fill-all.js` as iTerm2 wrote it.
pub fn fill_all_js() -> &'static str {
    file("autofill-fill-all.js")
}

/// Replaces `from` by `to` when `from` occurs exactly `n` times (a load-time patch fails closed).
fn patch_n(src: &str, from: &str, to: &str, n: usize) -> Result<String, String> {
    match src.matches(from).count() {
        c if c == n => Ok(src.replace(from, to)),
        c => Err(format!("patch target {from:?} found {c} times, wanted {n}")),
    }
}

/// `autofill-detector.js` with Tandem's patches, every one exact and fail closed (the copy in
/// `resources/` stays iTerm2's, byte for byte). The buttons are built in a closed shadow root held by
/// the isolated world under a host styled with `!important` inline rules, so a page can neither see,
/// restyle nor cover them with a decoy; a click counts only when it is a person's (`isTrusted`) and
/// the trusted pointer's point holds that very button (`elementFromPoint` on the shadow root), or,
/// for a keyboard activation (`detail` 0), when that button has focus (13x5's `tandemReal`).
pub(crate) fn detector_patched(src: &str) -> Result<String, String> {
    let s = patch_once(
        src,
        "const fieldButtons = new WeakMap();",
        "const fieldButtons = new WeakMap();\n    // Tandem: the buttons live in a closed shadow root the page cannot reach.\n    let tandemHost = null;\n    let tandemShadow = null;\n    function tandemRoot() {\n        if (!tandemShadow) {\n            tandemHost = document.createElement('div');\n            tandemHost.style.cssText = 'all: initial !important; display: block !important; position: absolute !important; top: 0 !important; left: 0 !important; width: 100% !important; height: 0 !important; overflow: visible !important; z-index: 2147483647 !important; pointer-events: none !important;';\n            tandemShadow = tandemHost.attachShadow({mode: 'closed'});\n            const rule = document.createElement('style');\n            rule.textContent = 'button { pointer-events: auto !important; }';\n            tandemShadow.appendChild(rule);\n        }\n        if (!tandemHost.isConnected && document.body) { document.body.appendChild(tandemHost); }\n        return tandemShadow;\n    }\n    function tandemReal(e, btn) {\n        if (!e.isTrusted) return false;\n        const root = tandemRoot();\n        if (e.detail === 0) return root.activeElement === btn;\n        return root.elementFromPoint(e.clientX, e.clientY) === btn;\n    }",
    )?;
    let s = patch_once(
        &s,
        "btn.addEventListener('click', e => {",
        "btn.addEventListener('click', e => {\n            // Tandem: act only on a person's click, never a page script's (R13x67-2; iTerm2 bug).\n            if (!e.isTrusted) return;\n            if (!tandemReal(e, btn)) return;",
    )?;
    let s = patch_once(&s, "document.body.appendChild(btn);", "tandemRoot().appendChild(btn);")?;
    let s = patch_once(&s, "document.body.appendChild(meas);", "tandemRoot().appendChild(meas);")?;
    let s = patch_once(&s, "document.body.removeChild(meas);", "meas.remove();")?;
    patch_n(&s, "document.querySelectorAll('[data-iterm-autofill]')", "tandemRoot().querySelectorAll('[data-iterm-autofill]')", 4)
}

/// The patched detector, or the reason it could not be made (no button script is injected then, never
/// a half-patched one).
pub fn detector_js() -> Result<String, String> {
    detector_patched(file("autofill-detector.js"))
}

/// `autofill-write.js` with its three template keys turned into the names of typed arguments: the page's
/// field id and name and the value reach the script as `const` declarations the runner makes from JSON,
/// never as text pasted into the file (iTerm2 pastes the id raw, `AW:29-31`).
pub(crate) fn write_patched(src: &str) -> Result<String, String> {
    let s = patch_once(src, "const fieldId = \"{{FIELD_ID}}\";", "const fieldId = fieldIdArg;")?;
    let s = patch_once(&s, "const fieldName = \"{{FIELD_NAME}}\";", "const fieldName = fieldNameArg;")?;
    patch_once(&s, "const value = {{VALUE}};", "const value = valueArg;")
}

/// The detector as injected (`AH:28-33`): the secret in, the include resolved by 13x2. The secret is
/// Tandem's own hex, never page text. None when the patch failed closed.
pub fn detector_script(secret: &str) -> Option<String> {
    detector_js().ok().map(|js| fill_template(&js, &[("SECRET", secret)]))
}

/// Fill All's script (`AH:81-84`).
pub fn fill_all_script(secret: &str) -> String {
    fill_template(fill_all_js(), &[("SECRET", secret)])
}

/// One field's write (`AW:23-32`): a constant body and the field's id, name and value as typed
/// arguments of `App::browser_js_call`. The caller treats the arguments as secret.
pub fn write_call(id: &str, name: &str, value: &str) -> Call {
    let body = write_patched(file("autofill-write.js")).expect("the copied write script has its template keys");
    Call { body: format!("return {}", body.trim()), args: json!({"fieldIdArg": id, "fieldNameArg": name, "valueArg": value}) }
}

// --- The request (AH:39-60) ---

/// One field the page asked to fill (`id` and `name` as the page sent them, empty text included).
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub kind: String,
    pub id: Option<String>,
    pub name: Option<String>,
}

/// An id or a name: missing or null is none, text is text, anything else fails the entry.
fn text_or_none(v: Option<&Value>) -> Result<Option<String>, ()> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.len() <= MAX_TEXT => Ok(Some(s.clone())),
        _ => Err(()),
    }
}

/// `handleMessage` + `handleAutofillRequest`: the secret matches (13x2's `secret_ok`), the type is
/// `autofillRequest`, `activeField` is an object and `fields` an array. An entry with no text `type`
/// (or a bad id or name, or any over `MAX_TEXT`) is skipped; only the first `MAX_FIELDS` are kept.
pub fn parse_request(body: &Value, secret: &str) -> Option<Vec<Field>> {
    if !crate::browser_js::secret_ok(body, secret) || body.get("type")?.as_str()? != REQUEST_TYPE {
        return None;
    }
    body.get("activeField")?.as_object()?;
    let fields = body.get("fields")?.as_array()?;
    let mut out = Vec::new();
    for f in fields {
        let Some(kind) = f.get("type").and_then(Value::as_str).filter(|k| k.len() <= MAX_TEXT) else { continue };
        let (Ok(id), Ok(name)) = (text_or_none(f.get("id")), text_or_none(f.get("name"))) else { continue };
        out.push(Field { kind: kind.to_string(), id, name });
        if out.len() == MAX_FIELDS {
            break;
        }
    }
    Some(out)
}

// --- The contact card (ACS:134-301) ---

/// Overwrites a string's bytes with zeros and empties it (the capacity stays).
pub fn wipe(s: &mut String) {
    // SAFETY: 0 is valid UTF-8, and the string is emptied right after.
    unsafe {
        for b in s.as_bytes_mut() {
            std::ptr::write_volatile(b, 0);
        }
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
    s.clear();
}

/// What a person's card holds. Never printed: `Debug` names the fields and nothing else.
#[derive(Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Card {
    pub first_name: String,
    pub last_name: String,
    pub full_name: String,
    pub email: String,
    pub phone: String,
    pub address1: String,
    pub address2: String,
    pub city: String,
    pub state: String,
    pub zip: String,
    pub country: String,
    pub company: String,
}

impl fmt::Debug for Card {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Card")
            .field("first_name", &"<redacted>")
            .field("last_name", &"<redacted>")
            .field("full_name", &"<redacted>")
            .field("email", &"<redacted>")
            .field("phone", &"<redacted>")
            .field("address1", &"<redacted>")
            .field("address2", &"<redacted>")
            .field("city", &"<redacted>")
            .field("state", &"<redacted>")
            .field("zip", &"<redacted>")
            .field("country", &"<redacted>")
            .field("company", &"<redacted>")
            .finish()
    }
}

impl Card {
    pub fn is_empty(&self) -> bool {
        *self == Card::default()
    }

    /// Wipes every field (a card is not `Drop` so a caller can still build one with `..default`).
    pub fn wipe(&mut self) {
        for s in [
            &mut self.first_name,
            &mut self.last_name,
            &mut self.full_name,
            &mut self.email,
            &mut self.phone,
            &mut self.address1,
            &mut self.address2,
            &mut self.city,
            &mut self.state,
            &mut self.zip,
            &mut self.country,
            &mut self.company,
        ] {
            wipe(s);
        }
    }
}

/// The card's non-empty values by field type (`extractContactData`). The full name is the card's own
/// when set, else the first and last names joined (Decision: iTerm2 formats a contact's name the same way).
pub fn card_values(card: &Card) -> Vec<(&'static str, String)> {
    let full = if card.full_name.is_empty() {
        [card.first_name.as_str(), card.last_name.as_str()].iter().filter(|p| !p.is_empty()).copied().collect::<Vec<_>>().join(" ")
    } else {
        card.full_name.clone()
    };
    let all = [
        (FIELD_TYPES[0], card.first_name.clone()),
        (FIELD_TYPES[1], card.last_name.clone()),
        (FIELD_TYPES[2], full),
        (FIELD_TYPES[3], card.email.clone()),
        (FIELD_TYPES[4], card.phone.clone()),
        (FIELD_TYPES[5], card.address1.clone()),
        (FIELD_TYPES[6], card.address2.clone()),
        (FIELD_TYPES[7], card.city.clone()),
        (FIELD_TYPES[8], card.state.clone()),
        (FIELD_TYPES[9], card.zip.clone()),
        (FIELD_TYPES[10], card.country.clone()),
        (FIELD_TYPES[11], card.company.clone()),
    ];
    all.into_iter().filter(|(_, v)| !v.is_empty()).collect()
}

/// One value to write into one field. Never printed; wiped when dropped.
#[derive(Clone, PartialEq, Eq)]
pub struct Fill {
    pub id: Option<String>,
    pub name: Option<String>,
    pub value: String,
}

impl fmt::Debug for Fill {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Fill").field("id", &self.id.is_some()).field("name", &self.name.is_some()).field("value", &"<redacted>").finish()
    }
}

impl Drop for Fill {
    fn drop(&mut self) {
        wipe(&mut self.value);
    }
}

/// `prepareAutofillData` (`ACS:89-118`): for each requested field the card has a value for, in the
/// page's order; an empty id or name is dropped.
pub fn prepare(fields: &[Field], card: &Card) -> Vec<Fill> {
    let values = card_values(card);
    let mut out = Vec::new();
    for f in fields {
        let Some((_, value)) = values.iter().find(|(k, _)| *k == f.kind) else { continue };
        out.push(Fill {
            id: f.id.clone().filter(|s| !s.is_empty()),
            name: f.name.clone().filter(|s| !s.is_empty()),
            value: value.clone(),
        });
    }
    out
}

/// Fill All's answer (`AH:90-97`): success and how many fields it found; anything else is none.
/// Both are page-influenced numbers: callers log them clamped.
pub fn fill_all_outcome(v: &Value) -> Option<(bool, i64)> {
    Some((v.get("success")?.as_bool()?, v.get("fieldsFound")?.as_i64()?))
}

// --- The operation queue (iTerm2's AsyncMutex) ---

/// One operation at a time per pane; the rest wait in order. Past `MAX_QUEUE` waiting, the oldest
/// is dropped and counted (`take_dropped`), so the caller can answer it.
pub struct Queue<T> {
    busy: bool,
    waiting: VecDeque<T>,
    dropped: usize,
}

impl<T> Default for Queue<T> {
    fn default() -> Self {
        Queue::new()
    }
}

impl<T> Queue<T> {
    pub fn new() -> Queue<T> {
        Queue { busy: false, waiting: VecDeque::new(), dropped: 0 }
    }

    /// Answers `t` back when nothing runs (start it now), else queues it.
    pub fn push(&mut self, t: T) -> Option<T> {
        if !self.busy {
            self.busy = true;
            return Some(t);
        }
        if self.waiting.len() == MAX_QUEUE {
            self.waiting.pop_front();
            self.dropped += 1;
        }
        self.waiting.push_back(t);
        None
    }

    /// The running operation ended: the next one to start, or none and idle.
    pub fn finish(&mut self) -> Option<T> {
        let next = self.waiting.pop_front();
        self.busy = next.is_some();
        next
    }

    /// Forgets everything waiting and goes idle (the pane closed or its world reset).
    pub fn clear(&mut self) {
        self.waiting.clear();
        self.busy = false;
    }

    pub fn busy(&self) -> bool {
        self.busy
    }

    /// How many waiting operations `push` dropped since the last call.
    pub fn take_dropped(&mut self) -> usize {
        std::mem::take(&mut self.dropped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn raw(name: &str) -> &'static str {
        crate::browser_js::resource(name).unwrap()
    }

    #[test]
    fn autofill_request_follows_iterm2() {
        let body = json!({"type":"autofillRequest","sessionSecret":"s1","activeField":{"type":"email"},
            "fields":[{"type":"email","id":"e","name":null,"value":""},{"type":"zip","id":null,"name":"postal"}]});
        assert_eq!(
            parse_request(&body, "s1"),
            Some(vec![
                Field { kind: "email".into(), id: Some("e".into()), name: None },
                Field { kind: "zip".into(), id: None, name: Some("postal".into()) },
            ])
        );
        assert_eq!(parse_request(&body, "s2"), None);
        assert_eq!(parse_request(&body, ""), None);
        let mut b = body.clone();
        b.as_object_mut().unwrap().remove("sessionSecret");
        assert_eq!(parse_request(&b, "s1"), None);
        let mut b = body.clone();
        b.as_object_mut().unwrap().remove("activeField");
        assert_eq!(parse_request(&b, "s1"), None);
        let mut b = body.clone();
        b["type"] = json!("other");
        assert_eq!(parse_request(&b, "s1"), None);
        let mut b = body.clone();
        b["activeField"] = json!("email");
        assert_eq!(parse_request(&b, "s1"), None, "AH:57: the active field is an object");
        let mut b = body.clone();
        b.as_object_mut().unwrap().remove("fields");
        assert_eq!(parse_request(&b, "s1"), None);
        let mut b = body.clone();
        b["fields"] = json!([{"id":"x"}, {"type":"email","id":"e"}, {"type":"zip","id":7}]);
        assert_eq!(
            parse_request(&b, "s1"),
            Some(vec![Field { kind: "email".into(), id: Some("e".into()), name: None }]),
            "an entry without a type, or with a non-text id, is skipped"
        );
    }

    #[test]
    fn requests_are_bounded() {
        let many: Vec<_> = (0..200).map(|i| json!({"type":"email","id":format!("f{i}")})).collect();
        let body = json!({"type":"autofillRequest","sessionSecret":"s","activeField":{},"fields":many});
        assert_eq!(parse_request(&body, "s").unwrap().len(), MAX_FIELDS);
        let long = "x".repeat(MAX_TEXT + 1);
        let body = json!({"type":"autofillRequest","sessionSecret":"s","activeField":{},
            "fields":[{"type":"email","id":long},{"type":"email","name":long},{"type":long},{"type":"email","id":"x".repeat(MAX_TEXT)}]});
        let got = parse_request(&body, "s").unwrap();
        assert_eq!(got.len(), 1, "an over-long id, name or type drops that field, not the message");
        assert_eq!(got[0].id.as_deref().map(str::len), Some(MAX_TEXT));
    }

    #[test]
    fn copied_scripts_follow_iterm2() {
        // The four copies are iTerm2's files: nothing of Tandem's is in the file itself.
        for f in ["autofill-core.js", "autofill-detector.js", "autofill-fill-all.js", "autofill-write.js"] {
            assert!(!raw(f).contains("Tandem"), "{f} is byte-for-byte iTerm2's");
            assert!(raw(f).starts_with("// ") || raw(f).starts_with(";(function") || raw(f).starts_with("(function"), "{f}");
        }
        assert!(!raw("autofill-detector.js").contains(TRUSTED_CHECK));
        let d = detector_js().unwrap();
        assert_eq!(d.matches(TRUSTED_CHECK).count(), 1);
        let open = "btn.addEventListener('click', e => {";
        assert_eq!(d.matches(open).count(), 1);
        let from = d.find(open).unwrap() + open.len();
        let to = d.find(TRUSTED_CHECK).unwrap();
        let between = &d[from..to];
        assert!(between.contains("Tandem: act only on a person's click"), "the patch says whose fix it is");
        assert!(between.lines().all(|l| l.trim().is_empty() || l.trim_start().starts_with("//")), "the check is the first statement: {between:?}");
        let after = &d[to..];
        assert!(after.contains("if (!tandemReal(e, btn)) return;"));
        assert!(after.find("e.preventDefault();").is_some() && after.find("e.stopPropagation();").is_some());
        assert!(after.find("tandemReal(e, btn)").unwrap() < after.find("e.preventDefault();").unwrap());
        // The buttons live in a closed shadow root the page cannot reach.
        assert_eq!(d.matches("attachShadow({mode: 'closed'})").count(), 1);
        assert!(d.contains("root.elementFromPoint(e.clientX, e.clientY) === btn"));
        assert!(d.contains("tandemRoot().appendChild(btn);") && d.contains("tandemRoot().appendChild(meas);"));
        assert!(!d.contains("document.body.appendChild(btn)") && !d.contains("document.querySelectorAll('[data-iterm-autofill]')"));
        // The license text ships beside the copies and is not a script.
        assert_eq!(crate::browser_js::resource("LICENSE-iTerm2"), None);
        let lic = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/resources/browser/LICENSE-iTerm2")).unwrap();
        assert!(lic.starts_with("GNU GENERAL PUBLIC LICENSE"));
    }

    #[test]
    fn detector_patch_fails_closed() {
        let src = raw("autofill-detector.js");
        assert!(detector_patched(src).is_ok());
        for target in [
            "const fieldButtons = new WeakMap();",
            "btn.addEventListener('click', e => {",
            "document.body.appendChild(btn);",
            "document.body.appendChild(meas);",
            "document.body.removeChild(meas);",
        ] {
            assert!(detector_patched(&src.replace(target, "")).is_err(), "{target}: a moved target fails closed");
            assert!(detector_patched(&format!("{src}\n{target}\n")).is_err(), "{target}: a repeated target fails closed");
        }
        // One of the four lookups goes missing: not a silent half patch.
        assert!(detector_patched(&src.replacen("document.querySelectorAll('[data-iterm-autofill]')", "x", 1)).is_err());
        assert!(detector_patched("").is_err());
        assert!(write_patched("").is_err());
        assert!(write_patched(&raw("autofill-write.js").replace("const fieldName = \"{{FIELD_NAME}}\";", "")).is_err());
    }

    #[test]
    fn scripts_follow_iterm2() {
        let d = detector_script("abc").unwrap();
        assert!(d.contains("const sessionSecret = \"abc\";"));
        assert!(d.contains(core_js()), "the include is resolved from the resource table");
        assert!(!d.contains("{{"));
        let f = fill_all_script("abc");
        assert!(f.contains("const sessionSecret = \"abc\";") && f.contains(core_js()) && !f.contains("{{"));
        assert!(detector_js().unwrap().contains("const handlerName = 'iTermAutofillHandler';"));
        assert!(fill_all_js().contains("\"Autofill not permitted on this protocol\""));
        assert!(core_js().contains("const fieldPatterns"));
        // The secret is the view's own and never page data: a secret holding a key is still one pass.
        assert!(!detector_script("{{INCLUDE:autofill-core.js}}").unwrap().is_empty());
    }

    #[test]
    fn write_call_keeps_page_text_out_of_the_script() {
        let hostile_id = "{{VALUE}}\"; alert(1); \"</script>\n";
        let c = write_call(hostile_id, "{{FIELD_NAME}}", "secret-v");
        assert!(!c.body.contains("{{") && !c.body.contains("secret-v") && !c.body.contains("alert"), "the script is a constant");
        assert!(c.body.starts_with("return "));
        assert!(c.body.contains("const fieldId = fieldIdArg;") && c.body.contains("const fieldName = fieldNameArg;") && c.body.contains("const value = valueArg;"));
        assert_eq!(c.args, json!({"fieldIdArg": hostile_id, "fieldNameArg": "{{FIELD_NAME}}", "valueArg": "secret-v"}));
        // Through the runner each value is one JSON literal and the value appears once.
        let js = crate::browser_js::call_js(&c.body, &c.args).unwrap();
        assert_eq!(js.matches("secret-v").count(), 1);
        assert!(js.contains("const valueArg = \"secret-v\";"));
        assert!(js.contains(&format!("const fieldIdArg = {};", crate::browser_js::js_string(hostile_id))));
        assert!(!js.contains("\n\"") && js.matches('\n').count() == c.body.matches('\n').count(), "a line break in page text is escaped");
        let e = write_call("", "", "");
        assert_eq!(e.args, json!({"fieldIdArg": "", "fieldNameArg": "", "valueArg": ""}));
    }

    #[test]
    fn prepare_follows_iterm2() {
        let card = Card { first_name: "Ann".into(), last_name: "Lee".into(), email: "a@b".into(), ..Card::default() };
        let fields = vec![
            Field { kind: "firstName".into(), id: Some("f".into()), name: None },
            Field { kind: "lastName".into(), id: Some(String::new()), name: Some("ln".into()) },
            Field { kind: "phone".into(), id: Some("p".into()), name: None },
            Field { kind: "fullName".into(), id: None, name: None },
            Field { kind: "nonsense".into(), id: Some("n".into()), name: None },
        ];
        assert_eq!(
            prepare(&fields, &card),
            vec![
                Fill { id: Some("f".into()), name: None, value: "Ann".into() },
                Fill { id: None, name: Some("ln".into()), value: "Lee".into() },
                Fill { id: None, name: None, value: "Ann Lee".into() },
            ]
        );
        let named = Card { full_name: "Dr. Ann Lee".into(), ..card.clone() };
        assert!(card_values(&named).contains(&("fullName", "Dr. Ann Lee".to_string())));
        let last_only = Card { last_name: "Lee".into(), ..Card::default() };
        assert_eq!(card_values(&last_only), vec![("lastName", "Lee".to_string()), ("fullName", "Lee".to_string())]);
        assert!(card_values(&Card::default()).is_empty() && Card::default().is_empty() && !card.is_empty());
        assert!(!format!("{:?}", Fill { id: None, name: None, value: "pw".into() }).contains("pw"));
        assert!(format!("{:?}", Fill { id: None, name: None, value: "pw".into() }).contains("<redacted>"));
        assert!(!format!("{card:?}").contains("Ann") && !format!("{card:?}").contains("a@b"));
        assert!(format!("{card:?}").contains("first_name"));
        let keys: Vec<_> = card_values(&Card { first_name: "a".into(), company: "c".into(), zip: "z".into(), ..Card::default() }).iter().map(|(k, _)| *k).collect();
        assert!(keys.iter().all(|k| FIELD_TYPES.contains(k)));
    }

    #[test]
    fn a_fill_is_wiped_when_dropped() {
        let mut f = Fill { id: None, name: None, value: "hunter2".into() };
        let ptr = f.value.as_ptr();
        wipe(&mut f.value);
        assert!(f.value.is_empty());
        // SAFETY: the buffer is still owned by `f` (clear keeps the capacity); its first bytes were zeroed.
        let bytes = unsafe { std::slice::from_raw_parts(ptr, 7) };
        assert_eq!(bytes, &[0u8; 7]);
        let mut c = Card { email: "a@b".into(), ..Card::default() };
        c.wipe();
        assert!(c.is_empty());
    }

    #[test]
    fn fill_all_outcome_follows_iterm2() {
        assert_eq!(fill_all_outcome(&json!({"success":true,"fieldsFound":3})), Some((true, 3)));
        assert_eq!(fill_all_outcome(&json!({"success":false,"error":"x"})), None);
        assert_eq!(fill_all_outcome(&json!("x")), None);
    }

    #[test]
    fn queue_follows_iterm2() {
        let mut q = Queue::new();
        assert!(!q.busy());
        assert_eq!(q.push(1), Some(1));
        assert!(q.busy());
        assert_eq!(q.push(2), None);
        assert_eq!(q.push(3), None);
        assert_eq!(q.finish(), Some(2));
        assert_eq!(q.finish(), Some(3));
        assert_eq!(q.finish(), None);
        assert!(!q.busy());
        assert_eq!(q.push(4), Some(4));
        assert_eq!(q.push(5), None);
        q.clear();
        assert!(!q.busy());
        assert_eq!(q.finish(), None);
        assert_eq!(q.push(6), Some(6));
    }

    #[test]
    fn queue_is_bounded() {
        let mut q = Queue::new();
        assert_eq!(q.push(0), Some(0));
        for i in 1..=MAX_QUEUE + 3 {
            assert_eq!(q.push(i), None);
        }
        assert_eq!(q.take_dropped(), 3, "the oldest are dropped and counted");
        assert_eq!(q.take_dropped(), 0);
        assert_eq!(q.finish(), Some(4));
    }
}

//! The browser password writer's pure model (milestone 13x7, Task 5): iTerm2's four password scripts
//! (copied byte for byte; the load-time patches here are exact and fail closed), the probe's
//! classification, the exact-origin test between a stored account and the page, and the per-pane
//! operation order. A password reaches a page only after the person picked a credential in Termurion's own
//! picker; the picker's caller binds the pick to the page's exact origin and navigation epoch
//! (`PwPage`), and every operation is checked against that binding again when it runs and once more
//! inside the page script. The password is a `pwm::Secret` from the source's reply until it has been
//! written into the call text, which is itself a `Secret` (`browser_js::call_js_secret`); no script
//! text holding it is a `String`, a format argument, a log line or a `Debug` output.
//!
//! iTerm2 cites: `PWR` Browser/PasswordManager/iTermBrowserPasswordWriter.swift, `PBJ` password-button.js,
//! `FPJ` focus-password.js, `PPJ` probe-password.js, `WPJ` write-password.js.

use serde_json::{Value, json};

use crate::browser_autofill::{MAX_TEXT, Queue, TRUSTED_CHECK, origin_of};
use crate::browser_js::{fill_template, resource, secret_ok};
use crate::browser_marks::patch_once;
use crate::pwm::Secret;

/// The typed argument that carries the text to write (a password, or a user name).
pub const TEXT_ARG: &str = "textArg";

/// What a refused password write tells the person (iTerm2 asks "Fill it anyway?"; Termurion never writes
/// a password into a field that is not a visible password field, Task 5 ruling).
pub const NOT_PASSWORD_FIELD: &str = "The focused field is not a visible password field, so Termurion did not fill it.";
/// A write that could not run because the page moved on since the pick.
pub const PAGE_CHANGED: &str = "The page changed, so Termurion did not fill it.";
/// A fill is refused on any page that is not https.
/// A stored account for another site is never typed into this page.
pub const WRONG_SITE: &str = "That password is for a different site, so Termurion did not fill it.";
pub const HTTPS_ONLY: &str = "Termurion fills passwords only on secure (https) pages.";

fn file(name: &str) -> &'static str {
    resource(name).expect("a copied password script is in the resource table")
}

/// The frame and page guard patched into `password-button.js`: no button on a page that is not https
/// (this also leaves every `iterm2-*` page, `about:blank` and `data:` without one) and none in an
/// iframe of another origin than the top page (a cross-origin parent throws on `window.top.location`).
const BUTTON_GUARD: &str = "'use strict';\n    // Termurion: no button on a page that is not https, nor in an iframe of another origin.\n    if (location.protocol !== 'https:') return;\n    try { if (window.top !== window && window.top.location.origin !== window.location.origin) return; } catch (_) { return; }";

/// The closed shadow root (13x5's lesson, as Task 1 gave the autofill button): the button lives in a
/// root only this world holds, under a host styled with `!important` inline rules; its label is
/// generated content (`::after`), never text, so `window.find` and select-all read nothing of it. A
/// click counts only when it is a person's (`isTrusted`) and the pointer's point holds that very button
/// (`elementFromPoint` on the root), or, for a keyboard activation, when the button has focus. The
/// autofill button is another closed root: its host is the one other max z-index element at the spot
/// (`tandemShift`), so this button steps aside by its own width, as iTerm2's does for a visible one.
const BUTTON_ROOT: &str = "let btn = null;\n    // Termurion: the button lives in a closed shadow root the page cannot reach; its label is generated content.\n    let tandemHost = null;\n    let tandemShadow = null;\n    function tandemRoot() {\n        if (!tandemShadow) {\n            tandemHost = document.createElement('div');\n            tandemHost.style.cssText = 'all: initial !important; display: block !important; position: absolute !important; top: 0 !important; left: 0 !important; width: 100% !important; height: 0 !important; overflow: visible !important; z-index: 2147483647 !important; pointer-events: none !important;';\n            tandemShadow = tandemHost.attachShadow({mode: 'closed'});\n            const rule = document.createElement('style');\n            rule.textContent = 'button { pointer-events: auto !important; } [data-tandem-glyph]::after { content: attr(data-tandem-glyph); }';\n            tandemShadow.appendChild(rule);\n        }\n        if (!tandemHost.isConnected && document.body) { document.body.appendChild(tandemHost); }\n        return tandemShadow;\n    }\n    function tandemReal(e, button) {\n        if (!e.isTrusted) return false;\n        const root = tandemRoot();\n        if (e.detail === 0) return root.activeElement === button;\n        return root.elementFromPoint(e.clientX, e.clientY) === button;\n    }\n    function tandemShift(samples, side) {\n        for (const {x, y} of samples) {\n            for (const el of document.elementsFromPoint(x, y)) {\n                if (el === tandemHost || el === activeField) continue;\n                if (parseInt(getComputedStyle(el).zIndex) === 2147483647) return side + 5;\n            }\n        }\n        return 0;\n    }";

/// `password-button.js` with Termurion's patches, every one exact once and fail closed (the copy in
/// `resources/` stays iTerm2's, byte for byte; a copy that changed makes no button script, never a
/// half-patched one). See `BUTTON_ROOT` and `BUTTON_GUARD`. The click handler acts only on
/// `event.isTrusted` (R13x67-2, an iTerm2 bug fixed) and a real press on the button, and then only
/// REQUESTS: the app opens the native password manager (Task 6); nothing fills without a pick there.
pub fn button_patched(src: &str) -> Result<String, String> {
    let s = patch_once(src, "'use strict';", BUTTON_GUARD)?;
    let s = patch_once(&s, "let btn = null;", BUTTON_ROOT)?;
    let s = patch_once(
        &s,
        "btn.addEventListener('click', e => {\n            e.preventDefault();",
        &format!("btn.addEventListener('click', e => {{\n            // Termurion: act only on a person's click, never a page script's (R13x67-2; iTerm2 bug).\n            {TRUSTED_CHECK}\n            if (!tandemReal(e, btn)) return;\n            e.preventDefault();"),
    )?;
    let s = patch_once(&s, "document.body.appendChild(btn);", "tandemRoot().appendChild(btn);")?;
    let s = patch_once(&s, "btn.textContent = emoji;", "btn.setAttribute('data-tandem-glyph', emoji);")?;
    let s = patch_once(&s, "meas.textContent = emoji;", "meas.setAttribute('data-tandem-glyph', emoji);")?;
    let s = patch_once(&s, "document.body.appendChild(meas);", "tandemRoot().appendChild(meas);")?;
    let s = patch_once(&s, "document.body.removeChild(meas);", "meas.remove();")?;
    patch_once(&s, "let newLeft = baseLeft;", "let newLeft = baseLeft - tandemShift(samples, side);")
}

/// The patched button script, or the reason it could not be made.
pub fn button_js() -> Result<String, String> {
    button_patched(file("password-button.js"))
}

/// The message handler the button posts to (`PMH:15`).
pub const HANDLER: &str = "iTermOpenPasswordManager";

/// The button script as injected (`PMH:31-35`): the view's secret in. Termurion's own hex, never page text.
/// None when a patch failed closed: no button, no request.
pub fn button_script(secret: &str) -> Option<String> {
    button_js().ok().map(|js| fill_template(&js, &[("SECRET", secret)]))
}

/// What a press asks for (`PMH:18-22`). A press only REQUESTS: the app opens the native manager and
/// nothing is typed without the person's pick there. No field name, account or secret is carried (the
/// page's `fieldName` is dropped unread; only the next password field's id, which the focus step
/// compares to nothing but its own page).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    ForPassword,
    ForUser,
    ForBoth { password_id: String },
}

/// `userContentController(_:didReceive:)` (`PMH:38-58`): the type picks the action; a wrong secret, a
/// missing or unknown type is none. A password field id that is not text is no id.
pub fn parse_action(body: &Value, secret: &str) -> Option<Action> {
    if !secret_ok(body, secret) {
        return None;
    }
    match body.get("type")?.as_str()? {
        "openPassword" => Some(Action::ForPassword),
        "openUser" => match body.get("nextPasswordFieldId").and_then(Value::as_str) {
            Some(id) if !id.is_empty() && id.len() <= MAX_TEXT => Some(Action::ForBoth { password_id: id.to_string() }),
            _ => Some(Action::ForUser),
        },
        _ => None,
    }
}

/// `probe-password.js` as iTerm2 wrote it (`PPJ`).
pub fn probe_js() -> &'static str {
    file("probe-password.js")
}

/// `focus-password.js` as iTerm2 wrote it (`FPJ`); `focus_patched` is the one that runs.
pub fn focus_js() -> &'static str {
    file("focus-password.js")
}

/// `write-password.js` as iTerm2 wrote it (`WPJ`); `write_patched` is the one that runs.
pub fn write_js() -> &'static str {
    file("write-password.js")
}

/// The guard every script starts with: only the main frame, and only on the exact origin the pick was
/// bound to (the host-side check can race a navigation; this one cannot).
const ORIGIN_GUARD: &str = "if (window.top !== window || window.location.origin !== originArg) { return false; }";

/// `write-password.js` with its three template keys turned into the names of typed arguments, the
/// origin guard first, and a stricter test for "visible" (zero-size and fully transparent fields are
/// refused; iTerm2 tests display, visibility and `offsetParent` only). Exact once each, fail closed.
pub(crate) fn write_patched(src: &str) -> Result<String, String> {
    let s = patch_once(src, "const s = {{STRING}};", &format!("{ORIGIN_GUARD}\n    const s = {TEXT_ARG};"))?;
    let s = patch_once(&s, "const requirePassword = {{REQUIRE_SECURE}};", "const requirePassword = requireArg;")?;
    let s = patch_once(&s, "const focusNextPw = {{FOCUS_NEXT_PW}};", "const focusNextPw = focusNextArg;")?;
    patch_once(
        &s,
        "style.visibility === 'hidden' || el.offsetParent === null) {",
        "style.visibility === 'hidden' || el.offsetParent === null\n        || parseFloat(style.opacity) === 0 || el.getBoundingClientRect().width < 1 || el.getBoundingClientRect().height < 1) {",
    )
}

/// `focus-password.js` with its id turned into a typed argument (iTerm2 pastes it bare, `FPJ:3`) and the
/// same origin guard.
pub(crate) fn focus_patched(src: &str) -> Result<String, String> {
    patch_once(src, "const prefocus = {{ID}};", &format!("{ORIGIN_GUARD}\n     const prefocus = idArg;"))
}

/// One script call: a constant body and its non-secret typed arguments.
pub struct Call {
    pub body: String,
    pub args: Value,
}

/// The write of `write_call`'s text: the password or user name itself is passed apart, to
/// `App::browser_js_call_secret` under the name `TEXT_ARG`.
pub fn write_call(origin: &str, require_password: bool, focus_next: bool) -> Call {
    let body = write_patched(write_js()).expect("the copied write script has its template keys");
    Call { body: format!("return {}", body.trim()), args: json!({"originArg": origin, "requireArg": require_password, "focusNextArg": focus_next}) }
}

/// The focus step of Enter Username & Password: the next password field by id, as a typed argument.
pub fn focus_call(origin: &str, id: &str) -> Call {
    let body = focus_patched(focus_js()).expect("the copied focus script has its template key");
    Call { body: format!("return {}", body.trim()), args: json!({"originArg": origin, "idArg": id}) }
}

/// The probe's call (no argument).
pub fn probe_call() -> Call {
    Call { body: format!("return {}", probe_js().trim()), args: json!({}) }
}

/// What the probe found at the focused element (`PWR:83-101`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Probe {
    NotAnInput,
    Unsafe,
    Safe,
}

/// iTerm2's rule (`PWR:90-99`): not found is not an input; a password field that is visible is safe;
/// anything else is unsafe.
pub fn probe_of(v: &Value) -> Probe {
    match v.get("found").and_then(Value::as_bool) {
        Some(true) => {}
        _ => return Probe::NotAnInput,
    }
    if v.get("isPassword").and_then(Value::as_bool) == Some(true) && v.get("visible").and_then(Value::as_bool) == Some(true) {
        Probe::Safe
    } else {
        Probe::Unsafe
    }
}

/// What a password write does with a probe's answer: only a safe field is written; the other two end
/// the operation, the second with the reason shown to the person.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Write,
    Nothing,
    Refuse(&'static str),
}

pub fn password_step(p: Probe) -> Step {
    match p {
        Probe::Safe => Step::Write,
        Probe::NotAnInput => Step::Nothing,
        Probe::Unsafe => Step::Refuse(NOT_PASSWORD_FIELD),
    }
}

// --- The page a pick is bound to ---

/// The exact origin (`https://host[:port]`, lowercase, default port dropped) and navigation epoch of
/// the main frame at the moment the person picked. https only: `new` answers None for anything else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PwPage {
    pub origin: String,
    pub epoch: u64,
}

impl PwPage {
    pub fn new(url: &str, epoch: u64) -> Option<PwPage> {
        origin_of(url).filter(|o| o.starts_with("https://")).map(|origin| PwPage { origin, epoch })
    }

    /// Whether the page now showing (`url`, `epoch`) is still the one the pick was made on.
    pub fn valid(&self, url: &str, epoch: u64) -> bool {
        self.epoch == epoch && self.origin.starts_with("https://") && origin_of(url).as_deref() == Some(self.origin.as_str())
    }
}

/// The origin an account name stands for: its first word, which must be exactly `https://host[:port]`
/// (an optional single trailing slash; Task 8's basic-auth names are `<origin> (<realm>)`), normalized
/// as `browser_autofill::origin_of` does. None for anything else: another scheme, a path, a query,
/// user information, a non-ASCII or empty host, text before the origin. The text is untrusted (a synced
/// or hand-typed source), so nothing here looks for a substring or a suffix.
pub fn account_origin(account: &str) -> Option<String> {
    let first = account.split(' ').next()?;
    if first.len() > MAX_TEXT || !first.is_ascii() || first.bytes().any(|b| b < 0x21 || b == 0x7f || b == b'\\') {
        return None;
    }
    let (scheme, rest) = first.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let tail = &rest[rest.find(['/', '?', '#']).unwrap_or(rest.len())..];
    if !(tail.is_empty() || tail == "/") {
        return None;
    }
    origin_of(first).filter(|o| o.starts_with("https://"))
}

/// Whether the stored account is for exactly the page's origin (Task 4's recorded to-do). A credential
/// for any other origin is never matched here; the picker lists it only as a manual choice.
pub fn account_matches(account: &str, page: &PwPage) -> bool {
    account_origin(account).is_some_and(|o| o == page.origin)
}

// --- The operations ---

/// One thing to type into the page. `Debug` never shows a value (`Secret`'s is redacted).
#[derive(Debug)]
pub enum Op {
    /// The password: probed first, written only into a visible password field.
    Password(Secret),
    /// The user name: written into the focused visible input, then the next password field takes focus.
    Username(Secret),
    /// Focus the password field with this id (Enter Username & Password's second step).
    Focus { id: String },
}

impl Op {
    /// A focus request, or None when the id is too long to be a field's.
    pub fn focus(id: &str) -> Option<Op> {
        (id.len() <= MAX_TEXT).then(|| Op::Focus { id: id.to_string() })
    }
}

/// An operation with the page it was picked on.
#[derive(Debug)]
pub struct Item {
    pub page: PwPage,
    pub op: Op,
}

/// One pane's operations, in order, one at a time (iTerm2's writer mutex, `PWR:11`). Dropping the queue
/// drops (and zeroes) every waiting secret.
pub type PwQueue = Queue<Item>;

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> PwPage {
        PwPage::new("https://example.test/login?x=1", 7).unwrap()
    }

    #[test]
    fn probe_follows_iterm2() {
        assert_eq!(probe_of(&json!({"found": false})), Probe::NotAnInput);
        assert_eq!(probe_of(&json!({"found": true, "isPassword": true, "visible": true})), Probe::Safe);
        assert_eq!(probe_of(&json!({"found": true, "isPassword": false, "visible": true})), Probe::Unsafe);
        assert_eq!(probe_of(&json!({"found": true, "isPassword": true, "visible": false})), Probe::Unsafe);
        assert_eq!(probe_of(&Value::Null), Probe::NotAnInput);
        assert_eq!(probe_of(&json!({})), Probe::NotAnInput);
        assert_eq!(password_step(Probe::Safe), Step::Write);
        assert_eq!(password_step(Probe::NotAnInput), Step::Nothing);
        assert_eq!(password_step(Probe::Unsafe), Step::Refuse(NOT_PASSWORD_FIELD));
    }

    #[test]
    fn password_scripts_follow_iterm2() {
        // The copies are iTerm2's bytes: the template keys are still there.
        assert!(write_js().contains("{{STRING}}") && write_js().contains("{{REQUIRE_SECURE}}") && write_js().contains("{{FOCUS_NEXT_PW}}"));
        assert!(focus_js().contains("{{ID}}"));
        assert!(probe_js().contains("isPassword: el.type === 'password'"));
        // The writer holds no text of the password: typed arguments, a constant body.
        let w = write_patched(write_js()).unwrap();
        assert!(w.contains("const s = textArg;") && w.contains("const requirePassword = requireArg;") && w.contains("const focusNextPw = focusNextArg;"));
        assert!(!w.contains("{{"), "no template key is left to fill");
        assert!(w.contains("window.top !== window") && w.contains("window.location.origin !== originArg"));
        assert!(w.find("originArg").unwrap() < w.find("document.activeElement").unwrap(), "the origin is the first test");
        assert!(w.contains("getBoundingClientRect().width < 1") && w.contains("parseFloat(style.opacity) === 0"));
        let f = focus_patched(focus_js()).unwrap();
        assert!(f.contains("const prefocus = idArg;") && !f.contains("{{") && f.contains("originArg"));
        // The same body whatever the values are; only the typed arguments differ.
        let a = write_call("https://a.test", true, false);
        let b = write_call("https://b.test", false, true);
        assert_eq!(a.body, b.body);
        assert_eq!(a.args, json!({"originArg": "https://a.test", "requireArg": true, "focusNextArg": false}));
        assert_eq!(focus_call("https://a.test", "pw\"x</script>{{ID}}").args["idArg"], "pw\"x</script>{{ID}}");
        assert!(!focus_call("https://a.test", "x").body.contains("pw\""), "an id never reaches the body");
        // Copied files carry no Termurion edit; the shipped button gets exactly one.
        for js in [focus_js(), probe_js(), write_js()] {
            assert!(!js.contains("Termurion:"));
        }
    }

    #[test]
    fn password_button_messages_follow_iterm2() {
        let m = |v: Value| parse_action(&v, "s");
        assert_eq!(m(json!({"type": "openPassword", "sessionSecret": "s"})), Some(Action::ForPassword));
        assert_eq!(m(json!({"type": "openUser", "sessionSecret": "s", "nextPasswordFieldId": "pw"})), Some(Action::ForBoth { password_id: "pw".into() }));
        assert_eq!(m(json!({"type": "openUser", "sessionSecret": "s", "nextPasswordFieldId": null})), Some(Action::ForUser));
        assert_eq!(m(json!({"type": "openUser", "sessionSecret": "s"})), Some(Action::ForUser));
        assert_eq!(m(json!({"type": "openUser", "sessionSecret": "s", "nextPasswordFieldId": 7})), Some(Action::ForUser));
        assert_eq!(m(json!({"type": "openUser", "sessionSecret": "s", "nextPasswordFieldId": "x".repeat(MAX_TEXT + 1)})), Some(Action::ForUser), "a hostile id is no id");
        assert_eq!(m(json!({"type": "openPassword", "sessionSecret": "t"})), None, "wrong secret");
        assert_eq!(m(json!({"type": "openPassword"})), None, "no secret");
        assert_eq!(m(json!({"sessionSecret": "s"})), None, "no type");
        assert_eq!(m(json!({"type": "x", "sessionSecret": "s"})), None);
        assert_eq!(m(json!({"type": 5, "sessionSecret": "s"})), None);
        assert_eq!(parse_action(&json!({"type": "openPassword", "sessionSecret": ""}), ""), None, "an empty secret is no secret");
        let js = button_script("s").unwrap();
        assert!(js.contains("const sessionSecret = \"s\";") && js.contains("const handlerName   = 'iTermOpenPasswordManager';"));
        assert_eq!(HANDLER, "iTermOpenPasswordManager");
        // The message carries no secret of Termurion's, no account and no user name: the request has no payload.
        assert!(!js.contains("{{"));
    }

    #[test]
    fn the_button_is_a_closed_trusted_request_only() {
        let b = button_js().unwrap();
        // Everything Termurion added is the patches: a copy that lost any one of them is caught below.
        assert_eq!(b.matches(TRUSTED_CHECK).count(), 1);
        let at = b.find("btn.addEventListener('click', e => {").unwrap();
        let body = &b[at..];
        let lines: Vec<&str> = body.lines().skip(1).map(str::trim).filter(|l| !l.starts_with("//")).collect();
        assert_eq!(lines[0], TRUSTED_CHECK, "first statement of the handler");
        assert_eq!(lines[1], "if (!tandemReal(e, btn)) return;", "then a real press on the button");
        assert_eq!(lines[2], "e.preventDefault();");
        // A closed shadow root; the button is never a child of the page.
        assert_eq!(b.matches("attachShadow({mode: 'closed'})").count(), 1);
        assert_eq!(b.matches("tandemRoot().appendChild(btn);").count(), 1);
        assert!(!b.contains("document.body.appendChild(btn)") && !b.contains("document.body.appendChild(meas)"));
        // The label is generated content: no text node holds it (window.find / select-all read none).
        assert!(!b.contains("textContent = emoji") && b.contains("[data-tandem-glyph]::after { content: attr(data-tandem-glyph); }"));
        assert!(b.contains("btn.setAttribute('data-tandem-glyph', emoji);") && b.contains("meas.setAttribute('data-tandem-glyph', emoji);"));
        // The press posts one thing: a type, the view's secret, a field name and the next password id.
        assert_eq!(b.matches("postMessage(").count(), 1);
        // https only, and no frame of another origin; the guard is the script's first statement.
        let g = b.find("location.protocol !== 'https:'").unwrap();
        assert!(g < b.find("let activeField").unwrap() && b.contains("window.top.location.origin !== window.location.origin"));
        // The bridge is Termurion's: no `window.webkit` once wrapped.
        let wrapped = crate::browser_js::wrap(&crate::browser_js::UserScript { id: "passwordManager", code: button_script("s").unwrap(), inject: crate::browser_js::Inject::DocumentEnd, main_frame_only: false, world: crate::browser_js::World::Client });
        assert!(!wrapped.contains("window.webkit"));
        // The copy is untouched.
        assert!(!resource("password-button.js").unwrap().contains("Termurion"));
    }

    /// Fail closed: a copy whose target moved (or doubled) makes no button script at all.
    #[test]
    fn the_button_patches_fail_closed() {
        let src = resource("password-button.js").unwrap();
        assert!(button_patched(src).is_ok());
        for target in ["'use strict';", "let btn = null;", "btn.addEventListener('click', e => {\n            e.preventDefault();", "document.body.appendChild(btn);", "btn.textContent = emoji;", "meas.textContent = emoji;", "document.body.appendChild(meas);", "document.body.removeChild(meas);", "let newLeft = baseLeft;"] {
            assert!(button_patched(&src.replace(target, "")).is_err(), "gone: {target}");
            assert!(button_patched(&format!("{src}\n{target}")).is_err(), "twice: {target}");
        }
        assert!(button_patched("nothing").is_err());
    }

    #[test]
    fn patches_fail_closed() {
        assert!(write_patched("const s = {{STRING}};").is_err());
        assert!(write_patched(&write_js().replace("const s = {{STRING}};", "const s = {{STRING}}; const s = {{STRING}};")).is_err());
        assert!(focus_patched("nothing").is_err());
    }

    #[test]
    fn a_pick_is_bound_to_one_exact_https_origin() {
        let p = page();
        assert_eq!(p.origin, "https://example.test");
        assert!(p.valid("https://EXAMPLE.test:443/other", 7), "same origin, other path");
        assert!(!p.valid("https://example.test/", 8), "a navigation since");
        assert!(!p.valid("https://example.test:8443/", 7), "another port");
        assert!(!p.valid("https://sub.example.test/", 7), "another host");
        assert!(!p.valid("https://example.test.evil.test/", 7));
        assert!(!p.valid("http://example.test/", 7), "http");
        assert!(!p.valid("https://example.test@evil.test/", 7), "user information");
        assert!(PwPage::new("http://example.test/", 1).is_none(), "never bound on http");
        assert!(PwPage::new("about:blank", 1).is_none());
        assert!(PwPage::new("iterm2-about:history", 1).is_none());
    }

    #[test]
    fn accounts_match_only_the_exact_origin() {
        let p = page();
        for ok in ["https://example.test", "https://example.test/", "https://EXAMPLE.test (Realm)", "https://example.test:443 (r)", "HTTPS://example.test"] {
            assert!(account_matches(ok, &p), "{ok}");
        }
        for bad in [
            "http://example.test",
            "example.test",
            "https://sub.example.test",
            "https://example.test.evil.test",
            "https://evilexample.test",
            "https://example.test:8443",
            "https://example.test/login",
            "https://example.test?x=1",
            "https://user@example.test",
            "https://example.test@evil.test",
            "https://evil.test/https://example.test",
            "see https://example.test",
            "https://example.test\\evil.test",
            "https://exampl\u{e9}.test",
            "https://",
            "",
            "ftp://example.test",
            "https://example.test\u{202e}",
        ] {
            assert!(!account_matches(bad, &p), "{bad:?}");
        }
        assert!(!account_matches("https://example.test", &PwPage { origin: "http://example.test".into(), epoch: 1 }), "an http page matches nothing");
    }

    #[test]
    fn operations_run_in_order_one_at_a_time() {
        let mut q: PwQueue = Queue::new();
        let item = |op| Item { page: page(), op };
        assert!(q.push(item(Op::Username(Secret::new("me".into())))).is_some(), "idle: starts now");
        assert!(q.push(item(Op::focus("pw").unwrap())).is_none());
        assert!(q.push(item(Op::Password(Secret::new("p".into())))).is_none());
        assert!(matches!(q.finish().unwrap().op, Op::Focus { .. }));
        assert!(matches!(q.finish().unwrap().op, Op::Password(_)));
        assert!(q.finish().is_none() && !q.busy());
        // A navigation drops what waits (and its secret) and the running one stays busy until it returns.
        assert!(q.push(item(Op::Username(Secret::new("me".into())))).is_some());
        assert!(q.push(item(Op::Password(Secret::new("p".into())))).is_none());
        q.drop_waiting();
        assert!(q.busy(), "the running operation's answer is still due");
        assert!(q.push(item(Op::focus("pw").unwrap())).is_none(), "nothing starts beside it");
        q.drop_waiting();
        assert!(q.finish().is_none() && !q.busy(), "the old page's password was dropped, not run");
        assert!(Op::focus(&"x".repeat(MAX_TEXT + 1)).is_none());
    }

    #[test]
    fn debug_never_shows_a_value() {
        let i = Item { page: page(), op: Op::Password(Secret::new("hunter2-secret".into())) };
        let j = Item { page: page(), op: Op::Username(Secret::new("me@example.test".into())) };
        for d in [format!("{i:?}"), format!("{j:?}")] {
            assert!(!d.contains("hunter2") && !d.contains("me@example"), "{d}");
        }
    }
}

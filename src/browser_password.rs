//! The browser password writer's pure model (milestone 13x7, Task 5): iTerm2's four password scripts
//! (copied byte for byte; the load-time patches here are exact and fail closed), the probe's
//! classification, the exact-origin test between a stored account and the page, and the per-pane
//! operation order. A password reaches a page only after the person picked a credential in Tandem's own
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
use crate::browser_js::resource;
use crate::browser_marks::patch_once;
use crate::pwm::Secret;

/// The typed argument that carries the text to write (a password, or a user name).
pub const TEXT_ARG: &str = "textArg";

/// What a refused password write tells the person (iTerm2 asks "Fill it anyway?"; Tandem never writes
/// a password into a field that is not a visible password field, Task 5 ruling).
pub const NOT_PASSWORD_FIELD: &str = "The focused field is not a visible password field, so Tandem did not fill it.";
/// A write that could not run because the page moved on since the pick.
pub const PAGE_CHANGED: &str = "The page changed, so Tandem did not fill it.";
/// A fill is refused on any page that is not https.
pub const HTTPS_ONLY: &str = "Tandem fills passwords only on secure (https) pages.";

fn file(name: &str) -> &'static str {
    resource(name).expect("a copied password script is in the resource table")
}

/// `password-button.js` with Tandem's one patch (R13x67-2): the click handler acts only on a person's
/// click. Exact once, fail closed (no button script is injected when the copy changed).
pub fn button_js() -> Result<String, String> {
    patch_once(
        file("password-button.js"),
        "btn.addEventListener('click', e => {\n            e.preventDefault();",
        &format!("btn.addEventListener('click', e => {{\n            // Tandem: act only on a person's click, never a page script's (R13x67-2; iTerm2 bug).\n            {TRUSTED_CHECK}\n            e.preventDefault();"),
    )
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
        // Copied files carry no Tandem edit; the shipped button gets exactly one.
        for js in [focus_js(), probe_js(), write_js()] {
            assert!(!js.contains("Tandem:"));
        }
    }

    #[test]
    fn the_button_acts_only_on_a_persons_click() {
        let b = button_js().unwrap();
        assert_eq!(b.matches(TRUSTED_CHECK).count(), 1);
        let at = b.find("btn.addEventListener('click', e => {").unwrap();
        let body = &b[at..];
        let first = body.lines().skip(1).map(str::trim).find(|l| !l.starts_with("//")).unwrap();
        assert_eq!(first, TRUSTED_CHECK, "first statement of the handler, before preventDefault");
        // Everything else is iTerm2's: removing the two inserted lines gives the copy back.
        let inserted = format!("            // Tandem: act only on a person's click, never a page script's (R13x67-2; iTerm2 bug).\n            {TRUSTED_CHECK}\n");
        assert_eq!(b.replacen(&inserted, "", 1), resource("password-button.js").unwrap());
        // Fail closed: a copy that changed makes no button.
        assert!(patch_once("nothing", "btn.addEventListener('click', e => {", "x").is_err());
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

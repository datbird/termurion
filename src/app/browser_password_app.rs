//! The app side of the browser password writer (milestone 13x7, Task 5). The caller (the browser
//! password manager, Task 6) binds the person's pick to the page with `browser_pw_page` and hands the
//! password over as a `Secret`; this file queues it, probes the focused field, writes it and drops it.
//!
//! Security: the write is a constant script in Tandem's isolated world with typed arguments
//! (`browser_password::write_call`), run through `browser_js_call_secret` (the call text is a `Secret`
//! wiped on drop). It runs in the main frame only (no frame-targeted runner is used here), only on an
//! https page whose exact origin and navigation epoch are the ones bound at pick time (checked when the
//! operation is queued, when it starts, after the probe, and inside the page script), and only into a
//! visible password field. A refused or dropped operation drops its secret. Nothing here logs a value,
//! a field id or an origin beyond the host; the entry points are `pub(super)` and are not reachable from
//! triggers, scripts or workgroups.

use serde_json::Value;

use super::browser_scripts_app::JsDone;
use super::*;
use crate::browser_password::{self as bp, Item, Op, PwPage, PwQueue, Step};
use crate::pwm::Secret;

/// One pane's password operations; they live in `BrowserView` and go with the view's world
/// (`browser_reset_world`), so no queued secret survives into a new page or a closed pane.
#[derive(Default)]
pub(super) struct PanePassword {
    pub(super) queue: PwQueue,
}

impl PanePassword {
    /// The page is new: every waiting operation (and its secret) is dropped.
    pub(super) fn reset(&mut self) {
        self.queue.clear();
    }
}

// The callers are Task 6's manager (Enter User Name, Enter Password, Enter Username & Password).
#[allow(dead_code)]
impl App {
    /// The main frame's URL and navigation epoch (`navigationCount`, `BM:1250`).
    fn pw_page_now(&self, pane: PaneId) -> Option<(String, u64)> {
        let io = self.panes.get(&pane)?.browser_io()?;
        Some((io.url.clone(), self.browser_views.get(&pane)?.marks_method.epoch()))
    }

    /// The navigation count of `pane` (bumped by each new main-frame navigation).
    pub(super) fn browser_nav_count(&self, pane: PaneId) -> u64 {
        self.pw_page_now(pane).map_or(0, |(_, e)| e)
    }

    /// The page a pick is bound to, taken at the moment of the pick; None (with the reason shown) on a
    /// page that is not https.
    pub(super) fn browser_pw_page(&mut self, pane: PaneId) -> Option<PwPage> {
        let (url, epoch) = self.pw_page_now(pane)?;
        let page = PwPage::new(&url, epoch);
        if page.is_none() {
            self.show_toast(bp::HTTPS_ONLY);
        }
        page
    }

    fn pw_still(&self, pane: PaneId, page: &PwPage) -> bool {
        self.pw_page_now(pane).is_some_and(|(url, epoch)| page.valid(&url, epoch))
    }

    /// Enter Password (`fillPassword`, `PWR:14-35`).
    pub(super) fn browser_fill_password(&mut self, pane: PaneId, page: PwPage, password: Secret) {
        self.pw_push(pane, Item { page, op: Op::Password(password) });
    }

    /// Enter User Name (`fillUsername`, `PWR:38-46`).
    pub(super) fn browser_fill_username(&mut self, pane: PaneId, page: PwPage, user: Secret) {
        self.pw_push(pane, Item { page, op: Op::Username(user) });
    }

    /// The focus step after a user name (`focus`, `PWR:48-64`): runs only on the page it was asked on.
    pub(super) fn browser_focus_password(&mut self, pane: PaneId, page: PwPage, id: &str) {
        if let Some(op) = Op::focus(id) {
            self.pw_push(pane, Item { page, op });
        }
    }

    fn pw_push(&mut self, pane: PaneId, item: Item) {
        if !self.pw_still(pane, &item.page) {
            self.show_toast(bp::PAGE_CHANGED);
            return;
        }
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        let start = bv.password.queue.push(item);
        if let Some(item) = start {
            self.pw_start(pane, item);
        }
    }

    /// The pane closed or its world reset: waiting operations (and their secrets) go.
    pub(super) fn browser_pw_pane_gone(&mut self, pane: PaneId) {
        if let Some(bv) = self.browser_views.get_mut(&pane) {
            bv.password.reset();
        }
    }

    /// The running operation ended: the next one starts.
    fn pw_next(&mut self, pane: PaneId) {
        let next = self.browser_views.get_mut(&pane).and_then(|bv| bv.password.queue.finish());
        if let Some(item) = next {
            self.pw_start(pane, item);
        }
    }

    /// Runs one operation if its page is still the one it was picked on, else drops it with its secret.
    fn pw_start(&mut self, pane: PaneId, item: Item) {
        if !self.pw_still(pane, &item.page) {
            self.show_toast(bp::PAGE_CHANGED);
            return self.pw_next(pane);
        }
        let Item { page, op } = item;
        match op {
            Op::Password(secret) => {
                let call = bp::probe_call();
                let done: JsDone = Box::new(move |app, pane, r| {
                    let found = r.map(|v| bp::probe_of(&v)).unwrap_or(bp::Probe::NotAnInput);
                    match bp::password_step(found) {
                        Step::Write => app.pw_write(pane, &page, &secret, true, false),
                        Step::Refuse(why) => {
                            app.show_toast(why);
                            app.pw_next(pane);
                        }
                        Step::Nothing => app.pw_next(pane),
                    }
                });
                self.browser_js_call(pane, &call.body, call.args, done);
            }
            Op::Username(secret) => self.pw_write(pane, &page, &secret, false, true),
            Op::Focus { id } => {
                let call = bp::focus_call(&page.origin, &id);
                let done: JsDone = Box::new(|app, pane, _| app.pw_next(pane));
                self.browser_js_call(pane, &call.body, call.args, done);
            }
        }
    }

    /// The one write (`PWR:112-131`): the page is checked again (the probe took a round trip), then the
    /// constant script runs with the text as its secret argument; the secret drops when the call returns.
    fn pw_write(&mut self, pane: PaneId, page: &PwPage, text: &Secret, require_password: bool, focus_next: bool) {
        if !self.pw_still(pane, page) {
            self.show_toast(bp::PAGE_CHANGED);
            return self.pw_next(pane);
        }
        let call = bp::write_call(&page.origin, require_password, focus_next);
        let password = require_password;
        let done: JsDone = Box::new(move |app, pane, r: Result<Value, String>| {
            if password && r.as_ref().ok().and_then(Value::as_bool) != Some(true) {
                app.show_toast(bp::NOT_PASSWORD_FIELD);
            }
            app.pw_next(pane);
        });
        self.browser_js_call_secret(pane, &call.body, call.args, bp::TEXT_ARG, text, done);
    }
}

#[cfg(test)]
mod tests {
    fn prod(src: &str) -> &str {
        &src[..src.find("#[cfg(test)]").unwrap_or(src.len())]
    }

    fn me() -> &'static str {
        prod(include_str!("browser_password_app.rs"))
    }

    fn between<'a>(s: &'a str, a: &str, b: &str) -> &'a str {
        let i = s.find(a).unwrap_or_else(|| panic!("missing {a}"));
        &s[i..i + s[i..].find(b).unwrap_or_else(|| panic!("missing {b}"))]
    }

    /// The password reaches the page through one call path: the secret runner, the constant write body,
    /// main frame only; never a frame runner, a format string, the clipboard or a log.
    #[test]
    fn the_writer_has_one_secret_path() {
        let src = me();
        assert_eq!(src.matches("browser_js_call_secret(").count(), 1, "one write site");
        assert_eq!(src.matches("bp::write_call(").count(), 1);
        for banned in ["browser_js_call_in_frame", "browser_eval_in_frame", "browser_eval(", "browser_page_call", "format!(", "rlog!", "dlog!", "println!", "eprintln!", "clipboard", "Command::new(", "execute_with(", "expose()", "duplicate()", "derive(Debug"] {
            assert!(!src.contains(banned), "{banned}");
        }
        // The secret goes into the runner by reference and is dropped with its closure.
        assert!(&src[src.find("fn pw_write").unwrap()..].contains("self.browser_js_call_secret(pane, &call.body, call.args, bp::TEXT_ARG, text, done)"));
    }

    /// Every operation is checked against the pick's page when queued, when started and before the
    /// write (the probe took a round trip), and the password needs the probe's Safe answer.
    #[test]
    fn every_step_checks_the_page() {
        let src = me();
        assert!(between(src, "fn pw_push", "/// The pane closed").contains("self.pw_still(pane, &item.page)"));
        assert!(between(src, "fn pw_start", "/// The one write").contains("self.pw_still(pane, &item.page)"));
        assert!(&src[src.find("fn pw_write").unwrap()..].contains("self.pw_still(pane, page)"));
        let start = between(src, "fn pw_start", "/// The one write");
        assert!(start.contains("Step::Write => app.pw_write(pane, &page, &secret, true, false)") && start.contains("Op::Username(secret) => self.pw_write(pane, &page, &secret, false, true)"));
        assert!(start.contains("Step::Refuse(why)") && start.contains("Step::Nothing"));
        // The bound page is only ever made from an https page.
        assert!(between(src, "pub(super) fn browser_pw_page", "fn pw_still").contains("PwPage::new(&url, epoch)"));
    }

    /// The entry points are not reachable from triggers, scripts or workgroups (P19), and the state
    /// follows the world and the pane.
    #[test]
    fn nothing_but_the_manager_can_call_the_writer() {
        for (name, src) in [("browser_triggers_app.rs", include_str!("browser_triggers_app.rs")), ("browser_scripts_app.rs", include_str!("browser_scripts_app.rs")), ("browser_smart_app.rs", include_str!("browser_smart_app.rs")), ("browser_input_app.rs", include_str!("browser_input_app.rs"))] {
            let p = prod(src);
            for f in ["browser_fill_password", "browser_fill_username", "browser_focus_password", "browser_pw_page"] {
                assert!(!p.contains(f), "{name} must not reach {f}");
            }
        }
        for f in ["fn browser_fill_password", "fn browser_fill_username", "fn browser_focus_password", "fn browser_pw_page", "fn browser_pw_pane_gone"] {
            let line = me().lines().find(|l| l.contains(f)).unwrap();
            assert!(line.contains("pub(super)"), "{line}");
        }
        let scripts = prod(include_str!("browser_scripts_app.rs"));
        assert!(between(scripts, "pub(super) fn browser_reset_world", "// --- Setup ---").contains("bv.password.reset();"));
        assert!(include_str!("mod.rs").contains("self.browser_pw_pane_gone(id);"));
    }
}

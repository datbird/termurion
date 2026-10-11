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
use crate::browser_js::{Routed, World};
use crate::browser_password::{self as bp, Item, Op, PwPage, PwQueue, Step};
use super::pwm_app::{PwmOpen, PwmTarget};
use crate::pwm::Secret;
use crate::pwm_source::Scope;

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

    /// A navigation started: the waiting operations (and their secrets) are the old page's and go now;
    /// the running one's answer is still due and finds its page gone (Task 5 review M3).
    pub(super) fn navigated(&mut self) {
        self.queue.drop_waiting();
    }
}

// The callers are Task 6's manager (Enter User Name, Enter Password, Enter Username & Password).
#[allow(dead_code)]
impl App {
    /// The navigation count of `pane` (bumped by each new main-frame navigation).
    pub(super) fn browser_nav_count(&self, pane: PaneId) -> u64 {
        self.page_bound_now(pane).map_or(0, |(_, e)| e)
    }

    /// Opens the browser password manager over `pane` (`openPasswordManagerForHost:forUser:didSendUserName:`,
    /// `PSB:113-124`): the account is the page's exact origin (what `account_matches` compares), `for_user`
    /// makes the default button send the user name, and a `focus_after_user` field id makes it send both,
    /// focusing that password field in between (while the page's navigation count is still this one).
    pub(super) fn browser_open_password_manager(&mut self, pane: Option<PaneId>, for_user: bool, focus_after_user: Option<String>) {
        let Some(pane) = pane.or_else(|| self.focused_id()).filter(|p| self.is_browser_pane(*p)) else { return };
        let account = self.page_bound_now(pane).and_then(|(url, epoch)| PwPage::new(&url, epoch)).map(|p| p.origin);
        let at = self.browser_nav_count(pane);
        self.open_password_manager_with(PwmOpen {
            account,
            pane: Some(pane),
            scope: Scope::Browser,
            for_user,
            send_both: focus_after_user.is_some(),
            target: PwmTarget::Session,
            focus_after_user: focus_after_user.map(|id| (id, at)),
            from_page: true,
        });
    }

    /// `iTermOpenPasswordManager`'s arm (`BM:1107-1135`): the isolated world's message with the view's
    /// secret, nothing else. A press only REQUESTS: it opens the native manager bound to the page's exact
    /// https origin and epoch (Task 6), where the person picks; nothing is typed from here. Nothing is
    /// asked of the store, so the button and this arm answer the same for 0 or N accounts. The origin is
    /// the main page's (iTerm2 uses `webView.url?.host` even for a frame's message); a frame's message
    /// never carries a field id to focus (the focus step is the main frame's).
    pub(super) fn browser_password_message(&mut self, pane: PaneId, r: &Routed) {
        if r.world != World::Client || !self.client_open(pane) {
            return;
        }
        let Some(secret) = self.browser_view_secret(pane) else { return };
        let Some(action) = bp::parse_action(&r.body, secret) else { return };
        // Only an https page (the same test as the pick's binding) and no toast: a page learns nothing.
        if !self.page_bound_now(pane).is_some_and(|(url, epoch)| PwPage::new(&url, epoch).is_some()) {
            return;
        }
        let (for_user, focus) = match action {
            bp::Action::ForPassword => (false, None),
            bp::Action::ForUser => (true, None),
            bp::Action::ForBoth { password_id } => (true, r.frame.is_none().then_some(password_id)),
        };
        self.browser_open_password_manager(Some(pane), for_user, focus);
    }

    /// The page a pick is bound to, taken at the moment of the pick; None (with the reason shown) on a
    /// page that is not https.
    pub(super) fn browser_pw_page(&mut self, pane: PaneId) -> Option<PwPage> {
        let (url, epoch) = self.page_bound_now(pane)?;
        let page = PwPage::new(&url, epoch);
        if page.is_none() {
            self.show_toast(bp::HTTPS_ONLY);
        }
        page
    }

    fn pw_still(&self, pane: PaneId, page: &PwPage) -> bool {
        self.page_bound_now(pane).is_some_and(|(url, epoch)| page.valid(&url, epoch))
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

    /// Task 7: the button's message arm. Client world only, the view's secret, an https page, the
    /// main frame's origin; it opens the native manager and types nothing; and it asks the store nothing,
    /// so the page cannot tell 0 accounts from N. The script is one isolated-world user script.
    #[test]
    fn password_button_opens_the_manager() {
        let src = me();
        let arm = between(src, "pub(super) fn browser_password_message", "/// The page a pick is bound to");
        assert!(arm.contains("World::Client") && arm.contains("self.client_open(pane)") && arm.contains("bp::parse_action(&r.body, secret)"));
        assert!(arm.contains("PwPage::new(&url, epoch).is_some()") && !arm.contains("show_toast"), "https only, silent to the page");
        assert!(arm.contains("bp::Action::ForPassword => (false, None)") && arm.contains("bp::Action::ForUser => (true, None)"));
        assert!(arm.contains("r.frame.is_none().then_some(password_id)"), "a frame's message never carries a focus id");
        for banned in ["browser_fill_", "browser_pw_page", "browser_js_call", "Work::", "accounts", "pwm_job", "format!(", "rlog!", "dlog!"] {
            assert!(!arm.contains(banned), "{banned}");
        }
        let opener = between(src, "pub(super) fn browser_open_password_manager", "/// The page a pick is bound to");
        assert!(opener.contains("from_page: true") && opener.contains("scope: Scope::Browser"));
        let scripts = prod(include_str!("browser_scripts_app.rs"));
        assert!(scripts.contains("h if h == crate::browser_password::HANDLER => self.browser_password_message(pane, &r),"));
        assert!(scripts.contains("UserScript { id: \"passwordManager\", code, inject: Inject::DocumentEnd, main_frame_only: false, world: World::Client }"));
        assert!(scripts.contains("crate::browser_password::button_script(secret)"), "inside the isolated-world block, none when the patches fail");
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
        // An allowlist (Task 5 review M1): of every file in src/app, only the writer and the manager
        // name the writer's entry points.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("app");
        let mut seen = 0;
        for e in std::fs::read_dir(&dir).unwrap() {
            let path = e.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if !name.ends_with(".rs") || name == "browser_password_app.rs" || name == "pwm_app.rs" {
                continue;
            }
            seen += 1;
            let src = std::fs::read_to_string(&path).unwrap();
            let p = prod(&src);
            for f in ["browser_fill_password", "browser_fill_username", "browser_focus_password", "browser_pw_page"] {
                assert!(!p.contains(f), "{name} must not reach {f}");
            }
        }
        assert!(seen > 50, "the scan read src/app ({seen} files)");
        for f in ["fn browser_fill_password", "fn browser_fill_username", "fn browser_focus_password", "fn browser_pw_page", "fn browser_pw_pane_gone"] {
            let line = me().lines().find(|l| l.contains(f)).unwrap();
            assert!(line.contains("pub(super)"), "{line}");
        }
        let scripts = prod(include_str!("browser_scripts_app.rs"));
        assert!(between(scripts, "pub(super) fn browser_reset_world", "// --- Setup ---").contains("bv.password.reset();"));
        assert!(include_str!("mod.rs").contains("self.browser_pw_pane_gone(id);"));
    }
}

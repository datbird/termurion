//! The app side of browser autofill (milestone 13x7): the contact card editor window and the fill
//! flow. A press in the page (or Session > Open Autocomplete) only REQUESTS. Nothing fills until the
//! person presses Fill in a native Tandem question (`pwm.confirms`, `ConfirmKind::Autofill`) that
//! names the page origin and each field type, and the fill is bound to that exact origin and
//! navigation epoch: a navigation or origin change before the answer, or before a queued field runs,
//! drops it. Never over http. Every write goes through `browser_autofill::write_call` in the main
//! frame only (a cross-origin iframe never gets one). Card values never reach the log or a banner.

use serde_json::json;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::browser_scripts_app::JsDone;
use super::*;
use crate::autofill_card_ui::{CardResult, CardWindow};
use crate::browser_autofill::{self, Bound, Field, Fill, Queue};
use crate::browser_js::{Routed, World};

/// A request that waits for the contact card (an empty card opens the editor once per page load).
pub(super) struct Pending {
    pub(super) origin: String,
    pub(super) epoch: u64,
    pub(super) fields: Vec<Field>,
}

/// One pane's autofill state: it lives in `BrowserView` and goes with the view's world
/// (`browser_reset_world`), so no fill, secret or wait survives into a new page.
#[derive(Default)]
pub(super) struct PaneAutofill {
    /// The fills waiting behind the running one (iTerm2's `AsyncMutex`, kept across requests).
    pub(super) queue: Queue<Bound>,
    pub(super) pending: Option<Pending>,
    /// The editor has opened on its own for this page load (an empty card asks once).
    pub(super) asked: bool,
}

impl PaneAutofill {
    /// The page is new: nothing queued, nothing waiting.
    pub(super) fn reset(&mut self) {
        self.queue.clear();
        self.pending = None;
        self.asked = false;
    }
}

/// What a person is asked: the fills (card values, wiped when dropped) and the page they are bound to.
pub(super) struct AutofillAsk {
    pub(super) origin: String,
    pub(super) epoch: u64,
    pub(super) fills: Vec<Fill>,
}

#[derive(Default)]
pub(super) struct AutofillState {
    pub(super) card: Option<CardWindow>,
    /// A fill found the card empty: the editor opens on the next pass (it needs the event loop).
    pub(super) want_card: bool,
}

impl App {
    /// Opens the contact card editor with the saved card, or brings the open one forward.
    pub(super) fn open_card_editor(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(w) = &self.autofill.card {
            w.focus();
            return;
        }
        let Some(g) = self.gpu.as_ref() else { return };
        let Some(base) = crate::webhost_win::local_appdata() else {
            self.banners.set("window", "Could not open the contact card: no local application data folder.".to_string());
            return;
        };
        let mut card = browser_autofill::load_card(&browser_autofill::card_path(&base));
        let opened = CardWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, card.clone());
        card.wipe();
        match opened {
            Ok(w) => {
                                self.autofill.card = Some(w);
            }
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// Events for the contact card editor. False for other windows. OK saves the card (a failure
    /// is a banner without values and leaves the editor open); Cancel or closing drops it.
    pub(super) fn autofill_card_event(&mut self, window: WindowId, e: &WindowEvent) -> bool {
        let Some(w) = self.autofill.card.as_mut().filter(|w| w.id() == window) else { return false };
        let answer = match e {
            WindowEvent::CloseRequested => Some(CardResult::Canceled),
            WindowEvent::RedrawRequested => match w.redraw() {
                Some(r) => Some(r),
                None => return true,
            },
            other => {
                w.event(other);
                return true;
            }
        };
        match answer {
            Some(CardResult::Saved(mut card)) => {
                let saved = match crate::webhost_win::local_appdata() {
                    Some(base) => browser_autofill::save_card(&browser_autofill::card_path(&base), &card),
                    None => Err("no local application data folder".to_string()),
                };
                card.wipe();
                match saved {
                    Ok(()) => {
                        self.autofill.card = None;
                        self.autofill_card_done(true);
                    }
                    Err(e) => self.banners.set("window", format!("Could not save the contact card: {e}")),
                }
            }
            _ => {
                self.autofill.card = None;
                self.autofill_card_done(false);
            }
        }
        true
    }

    /// The page a pane shows: its URL and navigation epoch (what a fill is bound to).
    fn autofill_page(&self, pane: PaneId) -> Option<(String, u64)> {
        let io = self.panes.get(&pane)?.browser_io()?;
        Some((io.url.clone(), self.browser_views.get(&pane)?.marks_method.epoch()))
    }

    /// Whether a fill asked on `origin` in `epoch` still belongs to `pane`'s page.
    fn autofill_bound(&self, pane: PaneId, origin: &str, epoch: u64) -> bool {
        self.autofill_page(pane).is_some_and(|(url, now)| browser_autofill::bound_valid(origin, epoch, &url, now))
    }

    /// `iTermAutofillHandler`'s arm: the isolated world's message with the view's secret, nothing else.
    pub(super) fn browser_autofill_message(&mut self, pane: PaneId, r: &Routed) {
        if r.world != World::Client || r.frame.is_some() || !self.client_open(pane) {
            return;
        }
        let Some(secret) = self.browser_view_secret(pane) else { return };
        let Some(fields) = browser_autofill::parse_request(&r.body, secret) else { return };
        self.browser_autofill_request(pane, fields);
    }

    /// `autofillRequest` (`BM:1985-2003`): logs the host and count, then asks the person. An https page
    /// only; one question at a time per pane; an empty card opens the editor and the request waits.
    pub(super) fn browser_autofill_request(&mut self, pane: PaneId, fields: Vec<Field>) {
        let Some((url, epoch)) = self.autofill_page(pane) else { return };
        let host = crate::dlog::origin(&url);
        crate::rlog!("Autofill requested for host: {} with {} fields", host, fields.len());
        if let Some(why) = browser_autofill::refusal(&url) {
            self.show_toast(why);
            return;
        }
        let Some(origin) = browser_autofill::origin_of(&url) else { return };
        if self.autofill_ask_open(pane) {
            return;
        }
        let Some(base) = crate::webhost_win::local_appdata() else { return };
        let mut card = browser_autofill::load_card(&browser_autofill::card_path(&base));
        if card.is_empty() {
            card.wipe();
            let ask_editor = match self.browser_views.get_mut(&pane) {
                Some(bv) => {
                    bv.autofill.pending = Some(Pending { origin, epoch, fields });
                    !std::mem::replace(&mut bv.autofill.asked, true)
                }
                None => false,
            };
            if ask_editor {
                self.autofill.want_card = true;
                self.redraw();
            }
            return;
        }
        self.autofill_ask(pane, origin, epoch, &fields, card);
    }

    /// Builds the question for the fields the card can fill and shows it; nothing to fill is logged.
    fn autofill_ask(&mut self, pane: PaneId, origin: String, epoch: u64, fields: &[Field], mut card: browser_autofill::Card) {
        let fills = browser_autofill::prepare(fields, &card);
        let kinds = browser_autofill::kinds_to_fill(fields, &card);
        card.wipe();
        if fills.is_empty() {
            crate::rlog!("No contact data available for autofill");
            return;
        }
        let text = browser_autofill::confirm_text(&origin, &kinds);
        self.pwm_ask_autofill(pane, text, AutofillAsk { origin, epoch, fills });
    }

    /// The editor closed: OK continues every waiting request that is still for its page, Cancel drops them.
    fn autofill_card_done(&mut self, saved: bool) {
        let waiting: Vec<(PaneId, Pending)> = self.browser_views.iter_mut().filter_map(|(p, bv)| bv.autofill.pending.take().map(|w| (*p, w))).collect();
        if !saved {
            return;
        }
        let Some(base) = crate::webhost_win::local_appdata() else { return };
        for (pane, w) in waiting {
            if !self.autofill_bound(pane, &w.origin, w.epoch) || self.autofill_ask_open(pane) {
                continue;
            }
            let card = browser_autofill::load_card(&browser_autofill::card_path(&base));
            self.autofill_ask(pane, w.origin, w.epoch, &w.fields, card);
        }
    }

    /// Fill was pressed: runs the fills if the page is still the one asked on.
    pub(super) fn autofill_confirmed(&mut self, pane: PaneId, ask: AutofillAsk) {
        if !self.autofill_bound(pane, &ask.origin, ask.epoch) {
            return;
        }
        self.browser_fill(pane, ask);
    }

    /// Pushes each fill through the pane's queue; one `browser_js_call` runs at a time (`AW:13-21`).
    fn browser_fill(&mut self, pane: PaneId, ask: AutofillAsk) {
        let AutofillAsk { origin, epoch, fills } = ask;
        for fill in fills {
            let Some(bv) = self.browser_views.get_mut(&pane) else { return };
            let start = bv.autofill.queue.push(Bound { origin: origin.clone(), epoch, fill });
            let dropped = bv.autofill.queue.take_dropped();
            if dropped > 0 {
                crate::rlog!("Autofill dropped {} queued fills", dropped);
            }
            if let Some(b) = start {
                self.autofill_start(pane, b);
            }
        }
    }

    /// Runs one queued fill, or skips it when its page is gone, then the next.
    fn autofill_start(&mut self, pane: PaneId, b: Bound) {
        if !self.autofill_bound(pane, &b.origin, b.epoch) {
            return self.autofill_next(pane);
        }
        let call = browser_autofill::write_call(b.fill.id.as_deref().unwrap_or(""), b.fill.name.as_deref().unwrap_or(""), &b.fill.value);
        drop(b);
        let done: JsDone = Box::new(|app, pane, _| app.autofill_next(pane));
        self.browser_js_call(pane, &call.body, call.args, done);
    }

    fn autofill_next(&mut self, pane: PaneId) {
        let next = self.browser_views.get_mut(&pane).and_then(|bv| bv.autofill.queue.finish());
        if let Some(b) = next {
            self.autofill_start(pane, b);
        }
    }

    /// Session > Open Autocomplete in a browser pane (`AH:77-102`): iTerm2's Fill All script in the
    /// isolated world. Its own request then arrives through the handler and is asked like any other.
    pub(super) fn browser_fill_all(&mut self, pane: PaneId) {
        if !self.client_open(pane) {
            return;
        }
        let Some(secret) = self.browser_view_secret(pane).map(str::to_string) else { return };
        let body = format!("return {}", browser_autofill::fill_all_script(&secret));
        let done: JsDone = Box::new(|_, _, r| match r.ok().as_ref().and_then(browser_autofill::fill_all_outcome) {
            Some((success, found)) => crate::rlog!("Autofill all: success {} fields {}", success, found.clamp(0, 1000)),
            None => crate::rlog!("Autofill all: no answer"),
        });
        self.browser_js_call(pane, &body, json!({}), done);
    }

    /// Opens what waited for the event loop (the card editor) and drops questions whose page moved on.
    pub(super) fn open_autofill_windows(&mut self, event_loop: &ActiveEventLoop) {
        for pane in self.autofill_ask_panes() {
            let page = self.autofill_page(pane);
            self.autofill_ask_drop(pane, false, |origin, epoch| page.as_ref().is_some_and(|(url, now)| browser_autofill::bound_valid(origin, epoch, url, *now)));
        }
        if std::mem::take(&mut self.autofill.want_card) {
            self.open_card_editor(event_loop);
        }
    }

    /// A pane closed: its questions, queue and waiting request go (their secrets wiped as they drop).
    pub(super) fn autofill_pane_gone(&mut self, pane: PaneId) {
        self.autofill_ask_drop(pane, true, |_, _| false);
        if let Some(bv) = self.browser_views.get_mut(&pane) {
            bv.autofill.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    fn prod(src: &str) -> &str {
        &src[..src.find("#[cfg(test)]").unwrap_or(src.len())]
    }

    fn me() -> &'static str {
        prod(include_str!("browser_autofill_app.rs"))
    }

    fn between<'a>(s: &'a str, a: &str, b: &str) -> &'a str {
        let i = s.find(a).unwrap_or_else(|| panic!("missing {a}"));
        &s[i..i + s[i..].find(b).unwrap_or_else(|| panic!("missing {b}"))]
    }

    /// An in-page press only requests (Task 1 review I1): the request arm asks in a native question and
    /// never writes; only the confirmed answer reaches the writer, which is Task 1's `write_call`.
    #[test]
    fn a_press_only_requests() {
        let src = me();
        let req = between(src, "pub(super) fn browser_autofill_request", "fn autofill_ask(");
        assert!(!req.contains("browser_fill(") && !req.contains("browser_js_call("), "the request arm never fills");
        let ask = between(src, "fn autofill_ask(", "fn autofill_card_done");
        assert!(ask.contains("pwm_ask_autofill(") && !ask.contains("browser_js_call("), "a native question comes first");
        let writers: Vec<_> = src.match_indices("write_call(").collect();
        assert_eq!(writers.len(), 1, "write_call is the only writer, once");
        assert!(between(src, "pub(super) fn autofill_confirmed", "/// Pushes each fill").contains("autofill_bound("), "the answer re-checks the page");
        let pwm = prod(include_str!("pwm_app.rs"));
        assert!(pwm.contains("ConfirmKind::Autofill(pane, ask) => {\n                if ok {\n                    self.autofill_confirmed(pane, ask);"), "only Fill runs it");
        assert!(pwm.contains("ConfirmWindow::choice(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, &c.heading, &c.text, pwm::TITLE_CANCEL, crate::browser_autofill::CONFIRM_FILL)"), "Enter answers Cancel");
    }

    /// The handler arm: isolated world only, main frame only, the view's secret, an https page.
    #[test]
    fn the_handler_arm_is_gated() {
        let arm = between(me(), "pub(super) fn browser_autofill_message", "/// `autofillRequest`");
        assert!(arm.contains("World::Client") && arm.contains("r.frame.is_some()") && arm.contains("self.client_open(pane)") && arm.contains("parse_request(&r.body, secret)"));
        let req = between(me(), "pub(super) fn browser_autofill_request", "fn autofill_ask(");
        assert!(req.contains("browser_autofill::refusal(&url)") && req.contains("self.autofill_ask_open(pane)"), "http refused, one question at a time");
        assert!(prod(include_str!("browser_scripts_app.rs")).contains("h if h == crate::browser_autofill::HANDLER => self.browser_autofill_message(pane, &r),"));
    }

    /// A fill is bound to the exact origin and epoch at request time; a queued fill is checked again
    /// when it runs; no frame-targeted runner is used (a cross-origin iframe never gets a fill).
    #[test]
    fn fills_are_bound_and_main_frame_only() {
        let src = me();
        assert!(between(src, "fn autofill_start", "fn autofill_next").contains("autofill_bound(pane, &b.origin, b.epoch)"));
        for banned in ["browser_js_call_in_frame", "browser_eval_in_frame", "browser_page_call", "Command::new(", "execute_with("] {
            assert!(!src.contains(banned), "{banned}");
        }
        assert!(between(src, "pub(super) fn open_autofill_windows", "/// A pane closed").contains("autofill_ask_drop(pane, false"), "a moved page closes the question");
    }

    /// The state goes with the view's world and the pane, and the question uses the ONE shared
    /// native-window guard (`prompt_ui::InputGuard` through `ConfirmWindow::guard_input`), once.
    #[test]
    fn state_follows_the_world_and_the_guard_has_one_site() {
        let scripts = prod(include_str!("browser_scripts_app.rs"));
        assert!(between(scripts, "pub(super) fn browser_reset_world", "// --- Setup ---").contains("bv.autofill.reset();"));
        assert!(include_str!("mod.rs").contains("self.autofill_pane_gone(id);"));
        let pwm = prod(include_str!("pwm_app.rs"));
        assert_eq!(pwm.matches("w.guard_input();").count(), 1, "one call site");
        assert!(!pwm.contains("native_input_guarded(") && !pwm.contains("input_ignored("), "no 500 ms guard of its own");
    }

    /// Merge with 13x6 (as R13x6-M2): the navigation epoch a fill is bound to moves only for a
    /// navigation that passed `nav_verdict` and the permission sync gate. A parked navigation is no
    /// navigation (no `Starting`, the epoch and the waiting request stay); its release is Tandem's
    /// own load, a new NavigationStarting, and moves the epoch then.
    #[test]
    fn the_epoch_moves_only_for_a_navigation_that_passed_the_gate() {
        let host = include_str!("../webhost_win.rs");
        let h = between(host, "fn starting_handler(", "fn register_navigation_rest(");
        let (verdict, gate, park, start) = (h.find("webhost::nav_verdict(").unwrap(), h.find("webhost::nav_gate(").unwrap(), h.find("Event::Parked {").unwrap(), h.find("i.poster.post(Event::Starting {").unwrap());
        assert!(verdict < gate && gate < park && park < start, "verdict, then the gate (park returns), then Starting");
        // The epoch moves in one place, the Starting arm, and the autofill reset is beside it.
        let app: String = ["browser_app.rs", "browser_marks_app.rs", "browser_perms_app.rs", "browser_restore_app.rs"].iter().map(|f| match *f {
            "browser_app.rs" => prod(include_str!("browser_app.rs")),
            "browser_marks_app.rs" => prod(include_str!("browser_marks_app.rs")),
            "browser_perms_app.rs" => prod(include_str!("browser_perms_app.rs")),
            _ => prod(include_str!("browser_restore_app.rs")),
        }).collect();
        assert_eq!(app.matches("marks_method.starting(").count(), 1);
        let arm = between(prod(include_str!("browser_marks_app.rs")), "Event::Starting { nav, redirect, user, .. } => {", "Event::DocumentMethod");
        assert!(arm.contains("bv.marks_method.starting(*nav);") && arm.contains("bv.autofill.pending = None;"));
        let parked = between(prod(include_str!("browser_perms_app.rs")), "pub(super) fn browser_navigation_parked(", "/// The toolbar's Reload");
        for banned in ["marks_method", "autofill", "password", "epoch"] {
            assert!(!parked.contains(banned), "a parked navigation leaves {banned} alone");
        }
        // The pure rule: a fill bound before a parked navigation still holds; after the released
        // navigation's Starting it does not.
        let mut m = crate::browser_marks::MethodState::default();
        m.starting(1);
        let bound = m.epoch();
        let url = "https://a.example/form";
        assert!(crate::browser_autofill::bound_valid("https://a.example", bound, url, m.epoch()), "parked: no Starting, the epoch holds");
        m.starting(2);
        assert!(!crate::browser_autofill::bound_valid("https://a.example", bound, url, m.epoch()), "released: its Starting moves the epoch");
    }

    /// Nothing in this file logs a value: logs carry the origin, counts and fixed text.
    #[test]
    fn fills_never_reach_the_log() {
        let src = me();
        for line in src.lines().filter(|l| l.contains("rlog!")) {
            assert!(!line.contains("value") && !line.contains("fill.") && !line.contains("card") && !line.contains("call"), "{line}");
        }
        assert!(src.contains("Autofill requested for host: {} with {} fields") && src.contains("No contact data available for autofill"));
    }

    /// Fill All runs through the one call path, in the isolated world.
    #[test]
    fn open_autocomplete_in_a_browser_pane() {
        let menu = include_str!("menu.rs");
        assert!(menu.contains("\"openAutocomplete:\" => match self.focused_id().filter(|p| self.is_browser_pane(*p)) {\n                Some(pane) => self.browser_fill_all(pane),\n                None => self.open_autocomplete(),"));
        let f = between(me(), "pub(super) fn browser_fill_all", "/// Opens what waited");
        assert!(f.contains("self.client_open(pane)") && f.contains("format!(\"return {}\", browser_autofill::fill_all_script(&secret))") && f.contains("self.browser_js_call("));
    }

    /// The detector is a Client-world, document-end, main-frame script with the view's secret.
    #[test]
    fn user_script_follows_iterm2() {
        let s = prod(include_str!("browser_scripts_app.rs"));
        let line = between(s, "crate::browser_autofill::detector_script(secret)", "scripts\n    }");
        assert!(line.contains("id: \"autofillHandler\"") && line.contains("inject: Inject::DocumentEnd") && line.contains("main_frame_only: true") && line.contains("world: World::Client"));
    }
}

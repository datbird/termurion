//! The find bar and the find menu on a browser pane (milestone 13x3, Task 4). The bar is Termurion's
//! own (`find_ui::FindState`, drawn in a strip above the page); the search is iTerm2's JS engine in
//! the isolated world, reached through `browser_js_call`, one command at a time. Everything a page
//! says arrives as a Client-world message and only the model reads it.

use super::find_ui::{FindState, Outcome};
use super::*;
use crate::browser_find::{self, Model, Update};
use crate::browser_js::{Routed, World};
use serde_json::{Value, json};

/// What the bar's key outcome does on a browser pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BarAct {
    Nothing,
    /// Find Next (`true`) or Find Previous: Return and Shift+Return are always forward here.
    Step { forward: bool },
    Close,
}

/// The bar's outcome for a browser pane: Tab (grow the match) and Alt+Return (paste the match) are
/// terminal text actions and do nothing; Return searches forward with or without Shift (`PS:9158`).
pub(super) fn bar_act(o: Outcome) -> BarAct {
    match o {
        Outcome::Step { .. } => BarAct::Step { forward: browser_find::enter_forward() },
        Outcome::Close => BarAct::Close,
        _ => BarAct::Nothing,
    }
}

/// The menu's Find Next and Find Previous after `SwapFindNextPrevious` (`PT:15329-15343`,
/// `FD:648-668`): true is `findNext`. It is the terminal's rule (`step_direction`), read the other
/// way: the terminal's "up" is the browser's `searchPrevious`.
pub(super) fn menu_forward(previous: bool, swap: bool) -> bool {
    !find_ui::step_direction(previous, swap)
}

/// The commands the bar's query starts: a new query is forced (`PB:480-487`), an empty one clears,
/// the owed global hide goes first.
pub(super) fn search_commands(model: &mut Model, query: &str, mode: crate::find::Mode, force: bool, secret: &str) -> Vec<Value> {
    model.start(query, browser_find::browser_mode(mode, query), force, secret)
}

/// Find Next or Previous: the owed hide first, the step only while a search is active.
pub(super) fn step_commands(model: &mut Model, forward: bool, secret: &str) -> Vec<Value> {
    if forward { model.next(secret) } else { model.previous(secret) }
}

/// Closing the bar clears the page (`PS:8856-8862`, `BVC:247-249`).
pub(super) fn close_commands(model: &mut Model, secret: &str) -> Vec<Value> {
    vec![model.clear(secret)]
}

/// The check a page answers before its global search starts (`FM:423-426`): no find script, no part.
pub(super) const GLOBAL_PROBE_JS: &str = "return typeof window.iTermCustomFind;";

/// True when the probe says the find script is there (`undefined` is a page without it).
pub(super) fn global_script_present(answer: &Result<Value, String>) -> bool {
    matches!(answer, Ok(Value::String(s)) if s == "object" || s == "function")
}

/// The commands a Find Globally search sends one page, the owed hide first (`FM:414-436`).
pub(super) fn global_commands(model: &mut Model, query: &str, mode: crate::find::Mode, secret: &str) -> Vec<Value> {
    model.global_start(query, browser_find::browser_mode(mode, query), secret)
}

/// Commands that reveal a picked match; nothing for an identifier the model would not hand back.
pub(super) fn reveal_commands(model: &mut Model, identifier: &Value, secret: &str) -> Vec<Value> {
    model.global_reveal(identifier, secret)
}

/// Opening Find with typing: the bar has the keyboard, the address field lets go.
pub(super) fn find_takes_keyboard(address_keys: &mut bool, f: &mut FindState) {
    *address_keys = false;
    f.typing = true;
}

/// The address field takes the keyboard: the find bar stops typing.
pub(super) fn address_takes_keyboard(address_keys: &mut bool, f: Option<&mut FindState>) {
    *address_keys = true;
    if let Some(f) = f {
        f.typing = false;
    }
}

/// The page took the keyboard back (a click, `Event::Focus(view, true)`): neither field types.
pub(super) fn page_takes_keyboard(address_keys: &mut bool, f: Option<&mut FindState>) {
    *address_keys = false;
    if let Some(f) = f {
        f.typing = false;
    }
}

/// Whether a focus change may hand the keyboard to the page: only when no Termurion field has it.
pub(super) fn page_may_focus(address_keys: bool, find_typing: bool) -> bool {
    !address_keys && !find_typing
}

/// Find URLs' banner for a pattern the search cannot take (13s's text).
pub(super) const BAD_URLS_PATTERN: &str = "Find URLs: invalid regular expression";

/// Find URLs on a page: the commands of the temporary search for `FindUrlsRegex`, or the banner.
/// The text goes to the page as it is (it runs it as a JavaScript RegExp); 13s's validator
/// (`find_urls_regex`) decides whether it is a pattern at all, and the model bounds its length.
pub(super) fn urls_commands(g: &Globals, model: &mut Model, secret: &str) -> Result<Vec<Value>, &'static str> {
    if super::labels_ui::find_urls_regex(g).is_none() {
        return Err(BAD_URLS_PATTERN);
    }
    model.temporary(&g.string("FindUrlsRegex"), secret).ok_or(BAD_URLS_PATTERN)
}

impl App {
    /// Whether pane `pane`'s find bar has the keyboard (a bar that is open and typing).
    pub(super) fn browser_find_typing(&self, pane: PaneId) -> bool {
        self.finds.get(&pane).is_some_and(|f| f.open && f.typing)
    }

    /// Whether the page may be given the keyboard now (no Termurion field has it).
    pub(super) fn browser_page_may_focus(&self, pane: PaneId) -> bool {
        page_may_focus(self.browser_views.get(&pane).is_some_and(|b| b.bar.keys), self.browser_find_typing(pane))
    }

    /// Opening the bar with typing (Find..., or the status bar Search Tool's field): the bar and not
    /// the page has the keyboard. `focus_window` (the one `SetFocus` the address field also uses)
    /// takes Win32 keyboard focus from the web view's child window; winit's `focus_window` only
    /// activates the top-level window (I3), so it is used first when the window is not in front.
    /// The page no longer holds the keyboard (`FocusStep::ReturnFromChild`'s bookkeeping).
    pub(super) fn browser_find_take_keyboard(&mut self, pane: PaneId) {
        if let (Some(bv), Some(f)) = (self.browser_views.get_mut(&pane), self.finds.get_mut(&pane)) {
            find_takes_keyboard(&mut bv.bar.keys, f);
            bv.bar.suggestions.clear();
            bv.bar.call = None;
        }
        let Some(hwnd) = self.hwnd() else { return };
        if crate::webhost_win::foreground_root() != Some(hwnd)
            && let Some(g) = self.gpu.as_ref() {
                g.window.focus_window();
            }
        crate::webhost_win::focus_window(hwnd);
        if self.web_focus_held.is_some() && self.web_focus_held == self.current_window() {
            self.web_focus_held = None;
            self.sync_web_keyboard();
        }
    }

    /// The address field takes the keyboard (`focus_url_bar`): the find bar stops typing.
    pub(super) fn browser_find_yield_to_address(&mut self, pane: PaneId) {
        if let Some(bv) = self.browser_views.get_mut(&pane) {
            address_takes_keyboard(&mut bv.bar.keys, self.finds.get_mut(&pane));
        }
    }

    /// The page has the keyboard (`Event::Focus(view, true)`).
    pub(super) fn browser_page_took_keyboard(&mut self, pane: PaneId) {
        if let Some(bv) = self.browser_views.get_mut(&pane) {
            page_takes_keyboard(&mut bv.bar.keys, self.finds.get_mut(&pane));
            bv.bar.suggestions.clear();
            bv.bar.call = None;
        }
    }

    /// The bar's query as a search: a fresh query is forced (`PB:480-487`). The page answers through
    /// `browser_find_message`; the bar's count comes from the model.
    pub(super) fn browser_find_search(&mut self, pane: PaneId, force: bool) {
        let mode = self.find_mode();
        let Some(query) = self.finds.get(&pane).map(|f| f.query.clone()) else { return };
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        let secret = bv.secret.clone();
        let cmds = search_commands(&mut bv.find, &query, mode, force, &secret);
        self.browser_find_send(pane, cmds);
        self.redraw();
    }

    /// Find Next (`forward`) or Find Previous. A search the delay still holds, or a remembered query
    /// with nothing active on the page (the bar was closed), is started instead.
    pub(super) fn browser_find_step(&mut self, pane: PaneId, forward: bool) {
        let pending = self.finds.get_mut(&pane).is_some_and(|f| std::mem::take(&mut f.pending));
        let remembered = self.finds.get(&pane).is_some_and(|f| !f.query.is_empty()) && self.browser_views.get(&pane).is_some_and(|b| b.find.active_term().is_none());
        if pending || remembered {
            return self.browser_find_search(pane, true);
        }
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        let secret = bv.secret.clone();
        let cmds = step_commands(&mut bv.find, forward, &secret);
        self.browser_find_send(pane, cmds);
    }

    /// Clears the page's highlights (the bar closed, or Clear): `clearFind`.
    pub(super) fn browser_find_clear(&mut self, pane: PaneId) {
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        let secret = bv.secret.clone();
        let cmds = close_commands(&mut bv.find, &secret);
        self.browser_find_send(pane, cmds);
    }

    /// The bar closed (Esc, or the Search Tool's close glyph): the page is cleared and gets the
    /// keyboard back (`takeFocus`, `PS:8850-8853`), unless the address field has it.
    pub(super) fn browser_find_closed(&mut self, pane: PaneId) {
        self.browser_find_clear(pane);
        if self.browser_page_may_focus(pane)
            && let Some(v) = self.browser_views.get(&pane).and_then(|b| b.view.as_ref()) {
                v.focus();
            }
        self.redraw();
    }

    /// Queues `commands` for the page, in order; each goes out when the one before it was answered
    /// (iTerm2's mutex, `FM:270`).
    pub(super) fn browser_find_send(&mut self, pane: PaneId, commands: Vec<Value>) {
        let first = match self.browser_views.get_mut(&pane) {
            Some(bv) => bv.find_out.push(commands),
            None => return,
        };
        if let Some(c) = first {
            self.browser_find_go(pane, c);
        }
    }

    /// One command, as data: the command is an argument of the call, never part of the script.
    fn browser_find_go(&mut self, pane: PaneId, command: Value) {
        self.browser_js_call(
            pane,
            browser_find::COMMAND_JS,
            json!({ "command": command }),
            Box::new(|app, pane, _answer| {
                let next = app.browser_views.get_mut(&pane).and_then(|bv| bv.find_out.answered());
                if let Some(c) = next {
                    app.browser_find_go(pane, c);
                }
            }),
        );
    }

    /// Find URLs on a browser pane (`PT:3872-3905`): the bar closes (its query stays), a temporary
    /// case-sensitive search for `FindUrlsRegex` runs, and when its full result is in the page shows
    /// the open shortcuts (`browser_find_message`). The page has the keyboard so find-nav's own key
    /// handler takes the letters and Esc. A shortcut's open is the page's `window.open`, which only
    /// 13x's gated `Event::NewWindow` path can turn into a tab: nothing here opens a URL.
    pub(super) fn browser_find_urls(&mut self, pane: PaneId) {
        if !self.client_open(pane) {
            self.banners.set("action", "Nothing to pick on screen.");
            return self.redraw();
        }
        if self.finds.get(&pane).is_some_and(|f| f.open) {
            if let Some(f) = self.finds.get_mut(&pane) {
                f.close();
            }
            self.browser_find_closed(pane);
        }
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        let secret = bv.secret.clone();
        match urls_commands(&self.globals, &mut bv.find, &secret) {
            Ok(cmds) => {
                self.browser_find_send(pane, cmds);
                if self.browser_page_may_focus(pane)
                    && let Some(v) = self.browser_views.get(&pane).and_then(|b| b.view.as_ref()) {
                        v.focus();
                    }
            }
            Err(banner) => {
                self.banners.set("action", banner);
            }
        }
        self.redraw();
    }

    /// Use Selection for Find (`BVC:778-791`): the page's selection, read by the one capped reader in
    /// Termurion's world, becomes the shared find string whatever the sync setting says.
    pub(super) fn browser_use_selection(&mut self, pane: PaneId) {
        self.browser_read_selection(
            pane,
            Box::new(|a, _, text| {
                let regex_mode = matches!(a.find_mode(), crate::find::Mode::RegexSensitive | crate::find::Mode::RegexInsensitive);
                if let Some(q) = browser_find::selection_query(&text, regex_mode) {
                    a.set_find_pasteboard_always(&q, None);
                    a.redraw();
                }
            }),
        );
    }

    /// Pick Result to Open (`BVC:1429-1435`): while a search is active, the open shortcuts go over
    /// its visible results and the page gets the keyboard. Nothing without a search
    /// (`BVC:1322-1323`).
    pub(super) fn browser_pick_result(&mut self, pane: PaneId) {
        if !self.client_open(pane) {
            return;
        }
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        if bv.find.active_term().is_none() {
            return;
        }
        let secret = bv.secret.clone();
        let cmds = bv.find.navigation_shortcuts("open", &secret);
        self.browser_find_send(pane, cmds);
        // The page gets the keyboard whatever has it now, as iTerm2 makes the web view first
        // responder (`BVC:1429-1435`): a typing find bar stops typing, the bar stays open.
        self.browser_page_took_keyboard(pane);
        if let Some(v) = self.browser_views.get(&pane).and_then(|b| b.view.as_ref()) {
            v.focus();
        }
    }

    /// Key action 36 on a browser pane (`PS:12119-12125`): a temporary case-sensitive regex search
    /// on the page, with no open shortcuts and the bar's query untouched.
    pub(super) fn browser_find_regex(&mut self, pane: PaneId, regex: &str) {
        if !self.client_open(pane) {
            self.banners.set("action", "No match.");
            return self.redraw();
        }
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        let secret = bv.secret.clone();
        match bv.find.temporary_search(regex, &secret) {
            Some(cmds) => self.browser_find_send(pane, cmds),
            None => self.banners.set("action", "Find Regular Expression: the pattern is empty or too long."),
        }
        self.redraw();
    }

    /// A Find Globally search reaches every browser pane (`GSEC:443-455`): a page with the find
    /// script (checked in the Termurion world, `FM:423-426`) clears and starts its global search; the
    /// others contribute nothing. An answer for an older search is dropped.
    pub(super) fn browser_global_start(&mut self, query: &str) {
        let generation = self.global_generation;
        let mode = self.global_mode();
        let panes: Vec<PaneId> = self.all_tab_panes().into_iter().flat_map(|(_, _, p)| p).filter(|p| self.is_browser_pane(*p) && self.client_open(*p)).collect();
        for pane in panes {
            let query = query.to_string();
            self.browser_js_call(
                pane,
                GLOBAL_PROBE_JS,
                json!({}),
                Box::new(move |app, pane, answer| {
                    if generation != app.global_generation || !global_script_present(&answer) {
                        return;
                    }
                    // A peer hidden while the probe ran is in no layout: it gets no find state (S4).
                    if !app.pane_in_a_layout(pane) {
                        return;
                    }
                    let Some(bv) = app.browser_views.get_mut(&pane) else { return };
                    bv.global_gen = generation;
                    let secret = bv.secret.clone();
                    let cmds = global_commands(&mut bv.find, &query, mode, &secret);
                    app.browser_find_send(pane, cmds);
                }),
            );
        }
    }

    /// The one batch of a page's global stream (`GSEC:457-470`): its rows join the list and the
    /// window is shown again. A batch for an older search, or with the window closed, is dropped.
    pub(super) fn browser_global_results(&mut self, pane: PaneId, results: Vec<browser_find::FindResult>) {
        if self.global_find.is_none() || self.browser_views.get(&pane).is_none_or(|b| b.global_gen != self.global_generation) {
            return;
        }
        let Some(title) = self.global_title(pane) else { return };
        self.global_web.retain(|h| h.pane != pane);
        self.global_web.extend(results.iter().map(|r| crate::global_find_ui::web_hit(pane, &title, r)));
        self.global_show();
    }

    /// A closed pane or a replaced page: its delivered Find Globally rows go and its stream
    /// no longer answers the current search; the window is shown again.
    pub(super) fn global_pane_gone(&mut self, pane: PaneId) {
        crate::global_find_ui::drop_pane_rows(&mut self.global_hits, &mut self.global_web, pane);
        if let Some(bv) = self.browser_views.get_mut(&pane) {
            bv.global_gen = 0;
        }
        if self.global_find.is_some() {
            self.global_show();
        }
    }

    /// The Find Globally window closed: nothing of its last search stays.
    pub(super) fn global_closed(&mut self) {
        self.global_find = None;
        self.global_hits.clear();
        self.global_web.clear();
        self.global_error = None;
    }

    /// Whether `pane` is in some window's layout (a hidden workgroup peer is in none).
    fn pane_in_a_layout(&self, pane: PaneId) -> bool {
        self.all_tab_panes().iter().any(|(_, _, panes)| panes.contains(&pane))
    }

    /// A picked browser match: the page shows its global results when the last search was the
    /// bar's, then reveals the match (`FM:438-450`).
    pub(super) fn browser_global_reveal(&mut self, pane: PaneId, identifier: &Value) {
        if !self.is_browser_pane(pane) || !self.client_open(pane) {
            return;
        }
        // A workgroup peer hidden since the search (13y1 seam S4) is in no layout: it gets no find
        // state, as `browser_global_start` never searched it.
        if !self.pane_in_a_layout(pane) {
            return;
        }
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        let secret = bv.secret.clone();
        let cmds = reveal_commands(&mut bv.find, identifier, &secret);
        self.browser_find_send(pane, cmds);
    }

    /// The find family from the menu on browser pane `id` (`browser_find::route`). False for Find
    /// Globally, which is its own window; every other command is taken.
    pub(super) fn browser_find_command(&mut self, id: PaneId, selector: &str, label: &str) -> bool {
        use browser_find::Route;
        let swap = self.globals.bool("SwapFindNextPrevious");
        match browser_find::route(selector, label) {
            Route::Open => {
                self.belt_yield_keys();
                self.finds.entry(id).or_default().open();
                self.load_find_pasteboard(id);
                if let Some(f) = self.finds.get_mut(&id).filter(|f| !f.query.is_empty()) {
                    f.set_query(&f.query.clone(), Instant::now());
                }
                self.browser_find_take_keyboard(id);
            }
            Route::Next => self.browser_find_step(id, menu_forward(false, swap)),
            Route::Previous => self.browser_find_step(id, menu_forward(true, swap)),
            Route::Clear => {
                if let Some(f) = self.finds.get_mut(&id) {
                    f.set_query("", Instant::now());
                    f.pending = false;
                }
                self.share_query(id);
                self.browser_find_clear(id);
            }
            Route::Jump => self.browser_jump_to_selection(id),
            Route::FindUrls => self.browser_find_urls(id),
            Route::UseSelection => self.browser_use_selection(id),
            Route::PickResult => self.browser_pick_result(id),
            Route::Global => return false,
            Route::Nothing => {}
        }
        self.redraw();
        true
    }

    /// One message of the find script (`FM:550-571`). Only the isolated world's, and only when that
    /// world was proven: a page-world message is dropped before it is read.
    pub(super) fn browser_find_message(&mut self, pane: PaneId, r: &Routed) {
        if r.world != World::Client || !self.client_open(pane) {
            return;
        }
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        let secret = bv.secret.clone();
        match bv.find.on_message(&r.body, &secret) {
            Update::Ignored => {}
            Update::Counts => self.redraw(),
            Update::Default { .. } => {
                bv.find.continue_find();
                // Find URLs: the full result of its temporary search brings the shortcuts.
                let shortcuts = bv.find.shortcuts_after_complete(&secret);
                if let Some(cmds) = shortcuts {
                    self.browser_find_send(pane, cmds);
                }
                self.redraw();
            }
            // Find Globally takes the one batch of the global stream (Task 6).
            Update::Global(results) => self.browser_global_results(pane, results),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser_find::{FindMode, Outbox};
    use crate::find::Mode;
    use find_ui::{BarKey, BarRoute};
    use std::time::{Duration, Instant};

    const SECRET: &str = "00112233445566778899aabbccddeeff";

    fn actions(cmds: &[Value]) -> Vec<String> {
        cmds.iter().map(|c| c["action"].as_str().unwrap_or("?").to_string()).collect()
    }

    fn model() -> Model {
        Model::new("11112222333344445555666677778888", "99990000aaaabbbbccccddddeeeeffff")
    }

    /// A bar on a browser pane, driven as the app drives it: keys through `route_key`, the bar's own
    /// state, and the model, with every command recorded as `browser_find_send` would take it.
    struct Sim {
        f: FindState,
        m: Model,
        out: Outbox,
        sent: Vec<Value>,
        t: Instant,
    }

    impl Sim {
        fn new() -> Sim {
            let mut f = FindState::default();
            f.open();
            f.typing = true;
            Sim { f, m: model(), out: Outbox::default(), sent: Vec::new(), t: Instant::now() }
        }

        fn send(&mut self, cmds: Vec<Value>) {
            // The page answers each command at once here.
            let mut next = self.out.push(cmds);
            while let Some(c) = next {
                self.sent.push(c);
                next = self.out.answered();
            }
        }

        fn key(&mut self, k: BarKey, swap: bool) {
            let o = self.f.key(k, self.t);
            match bar_act(o) {
                BarAct::Nothing => {}
                BarAct::Step { forward } => {
                    let cmds = step_commands(&mut self.m, forward, SECRET);
                    self.send(cmds);
                }
                BarAct::Close => {
                    let cmds = close_commands(&mut self.m, SECRET);
                    self.send(cmds);
                }
            }
            let _ = swap;
        }

        /// The delay elapsed: the due search runs.
        fn due(&mut self, mode: Mode) {
            let later = self.t + Duration::from_secs(5);
            if let Some(fresh) = self.f.take_due(later, Duration::from_secs(1)) {
                let q = self.f.query.clone();
                let cmds = search_commands(&mut self.m, &q, mode, fresh, SECRET);
                self.send(cmds);
            }
        }

        fn return_key(&mut self, shift: bool) {
            let mods = crate::mouse::Mods { shift, ..Default::default() };
            let BarRoute::Key(k) = find_ui::route_key(&winit::keyboard::Key::Named(winit::keyboard::NamedKey::Enter), None, mods) else { panic!("Return is a bar key") };
            self.key(k, false);
        }
    }

    /// Review Focus 5: the bar on a browser pane follows iTerm2.
    #[test]
    fn browser_find_uses_the_bar() {
        let mut s = Sim::new();
        s.key(BarKey::Text("A".into()), false);
        s.key(BarKey::Text("b".into()), false);
        assert!(s.sent.is_empty(), "nothing before the delay");
        s.due(Mode::SmartCase);
        assert_eq!(actions(&s.sent), ["startFind"]);
        assert_eq!(s.sent[0]["searchMode"], FindMode::CaseSensitive.raw(), "an uppercase letter makes smart case sensitive");
        assert_eq!(s.sent[0]["searchTerm"], "Ab");
        // The same query again, unforced, queues nothing.
        s.sent.clear();
        search_and_send(&mut s, false);
        assert!(s.sent.is_empty());
        // Return and Shift+Return both go forward (the browser rule).
        s.return_key(false);
        s.return_key(true);
        assert_eq!(actions(&s.sent), ["findNext", "findNext"]);
        // Tab and Alt+Return queue nothing.
        s.sent.clear();
        s.key(BarKey::Grow, false);
        s.key(BarKey::PasteMatch, false);
        assert!(s.sent.is_empty());
        // The menu: the swap key decides, as for terminals.
        assert_eq!(menu_forward(false, false), true, "Find Next, swap off: findNext");
        assert_eq!(menu_forward(false, true), false, "Find Next, swap on: findPrevious");
        assert_eq!(menu_forward(true, false), false, "Find Previous, swap off: findPrevious");
        assert_eq!(menu_forward(true, true), true, "Find Previous, swap on: findNext");
        let up = find_ui::step_direction(false, true);
        assert_eq!(!up, menu_forward(false, true), "the same rule the terminal's step uses");
        let cmds = step_commands(&mut s.m, menu_forward(false, true), SECRET);
        assert_eq!(actions(&cmds), ["findPrevious"]);
        let cmds = step_commands(&mut s.m, menu_forward(false, false), SECRET);
        assert_eq!(actions(&cmds), ["findNext"]);
        // Esc clears the page's highlights and closes the bar.
        s.sent.clear();
        s.key(BarKey::Close, false);
        assert_eq!(actions(&s.sent), ["clearFind"]);
        assert!(!s.f.open && !s.f.typing);
        // Nothing is active any more: a step queues nothing.
        assert!(step_commands(&mut s.m, true, SECRET).is_empty());
    }

    fn search_and_send(s: &mut Sim, force: bool) {
        let q = s.f.query.clone();
        let cmds = search_commands(&mut s.m, &q, Mode::SmartCase, force, SECRET);
        s.send(cmds);
    }

    #[test]
    fn modes_and_empty_queries() {
        let mut m = model();
        let c = search_commands(&mut m, "ab", Mode::SmartCase, true, SECRET);
        assert_eq!(c[0]["searchMode"], "caseInsensitive");
        let c = search_commands(&mut m, "ab", Mode::RegexSensitive, true, SECRET);
        assert_eq!(c[0]["searchMode"], "caseSensitiveRegex");
        // An empty query clears instead of searching.
        let c = search_commands(&mut m, "", Mode::SmartCase, true, SECRET);
        assert_eq!(actions(&c), ["clearFind"]);
    }

    /// Task 5 review: a bad Find URLs pattern shows its banner and does not hand the keyboard to the
    /// page; the focus follows only the commands that were sent.
    #[test]
    fn a_bad_find_urls_pattern_does_not_focus_the_page() {
        let me = prod(include_str!("browser_find_app.rs"));
        let f = between(me, "pub(super) fn browser_find_urls", "/// Use Selection for Find");
        let ok = between(f, "Ok(cmds) =>", "Err(banner)");
        assert!(ok.contains("self.browser_find_send(pane, cmds)") && ok.contains("browser_page_may_focus(pane)") && ok.contains(".focus()"));
        let err = &f[f.find("Err(banner)").unwrap()..];
        assert!(err.contains("self.banners.set(\"action\", banner)") && !err.contains(".focus()"), "no focus after the banner");
    }

    /// Pick Result to Open hands the page the keyboard even while the find bar types (final review
    /// I2b): the hand-over state is the one `browser_page_took_keyboard` makes, the bar stays open.
    #[test]
    fn pick_result_takes_the_keyboard_from_a_typing_bar() {
        let mut f = FindState::default();
        f.open();
        let mut keys = false;
        find_takes_keyboard(&mut keys, &mut f);
        assert!(!page_may_focus(keys, f.typing), "the gated path would leave the bar typing");
        page_takes_keyboard(&mut keys, Some(&mut f));
        assert!(f.open && !f.typing && !keys, "the bar stays open and stops typing");
        let me = prod(include_str!("browser_find_app.rs"));
        let pick = between(me, "pub(super) fn browser_pick_result", "/// Key action 36 on a browser pane");
        assert!(pick.contains("self.browser_page_took_keyboard(pane)") && pick.contains(".focus()") && !pick.contains("browser_page_may_focus") && !pick.contains("close()"));
        // Key action 36 goes to the page, not to the terminal's empty screen.
        let f = prod(include_str!("find_ui.rs"));
        let r = between(f, "pub(super) fn find_regex_in", "match find::find_all");
        assert!(r.contains("self.is_browser_pane(id)") && r.contains("self.browser_find_regex(id, regex)"));
        let reg = between(me, "pub(super) fn browser_find_regex", "/// A Find Globally search reaches every");
        assert!(reg.contains("temporary_search(") && !reg.contains("urls_commands"));
    }

    /// B6: the address field, the find bar and the page never type together.
    #[test]
    fn keyboard_has_one_owner() {
        let mut f = FindState::default();
        f.open();
        let mut keys = true;
        // Opening Find takes the keyboard from the address field.
        find_takes_keyboard(&mut keys, &mut f);
        assert!(f.typing && !keys);
        assert!(!page_may_focus(keys, f.typing), "a focus change must not steal the bar's keys");
        // Focusing the address field stops the bar typing.
        address_takes_keyboard(&mut keys, Some(&mut f));
        assert!(keys && !f.typing);
        assert!(!page_may_focus(keys, f.typing));
        // The same with no bar for the pane.
        address_takes_keyboard(&mut keys, None);
        assert!(keys);
        // A click in the page ends both.
        find_takes_keyboard(&mut keys, &mut f);
        page_takes_keyboard(&mut keys, Some(&mut f));
        assert!(!keys && !f.typing && f.open, "the bar stays open, typing stops");
        assert!(page_may_focus(keys, f.typing));
        // Never both at once, from any start.
        for (k0, t0) in [(false, false), (true, false), (false, true), (true, true)] {
            for step in 0..3 {
                let mut f = FindState::default();
                f.open();
                f.typing = t0;
                let mut keys = k0;
                match step {
                    0 => find_takes_keyboard(&mut keys, &mut f),
                    1 => address_takes_keyboard(&mut keys, Some(&mut f)),
                    _ => page_takes_keyboard(&mut keys, Some(&mut f)),
                }
                assert!(!(keys && f.typing), "step {step} from {k0} {t0}");
            }
        }
    }

    fn prod(src: &str) -> &str {
        &src[..src.find("#[cfg(test)]").unwrap()]
    }

    fn between<'a>(src: &'a str, from: &str, to: &str) -> &'a str {
        let a = src.find(from).unwrap_or_else(|| panic!("missing {from}"));
        let b = a + src[a..].find(to).unwrap_or_else(|| panic!("missing {to}"));
        &src[a..b]
    }


    /// Find URLs on a page (`PT:3872-3905`): the setting's text goes to the page as it is (the page
    /// runs it as a JavaScript RegExp), an invalid one sends nothing and shows 13s's banner, and
    /// `(?i)` is valid for the validator and sent all the same (it then finds nothing there, M4).
    #[test]
    fn find_urls_use_the_setting() {
        let mut g = Globals::empty(std::path::Path::new("unused.toml"));
        let default = "https?://([a-z0-9A-Z]+(:[a-zA-Z0-9]+)?@)?[a-z0-9A-Z\\-]+(\\.[a-z0-9A-Z\\-]+)*((:[0-9]+)?)(/[a-zA-Z0-9;:/\\.\\-_+%~?&amp;@=#\\(\\)]*)?";
        assert_eq!(g.string("FindUrlsRegex"), default, "AS:414-416, the literal &amp; kept");
        let mut m = model();
        let c = urls_commands(&g, &mut m, SECRET).expect("the default pattern searches");
        assert_eq!(c.len(), 1);
        assert_eq!((c[0]["action"].as_str(), c[0]["searchMode"].as_str(), c[0]["searchTerm"].as_str()), (Some("startFind"), Some("caseSensitiveRegex"), Some(default)));
        assert_eq!(c[0]["contextLength"], 0);
        for bad in ["(unclosed", ""] {
            g.set("FindUrlsRegex", toml::Value::String(bad.into()));
            let mut m = model();
            assert_eq!(urls_commands(&g, &mut m, SECRET), Err(BAD_URLS_PATTERN), "{bad:?}");
            assert!(m.active_term().is_none(), "nothing was started");
        }
        g.set("FindUrlsRegex", toml::Value::String("(?i)https?://\\S+".into()));
        let mut m = model();
        let c = urls_commands(&g, &mut m, SECRET).expect("the validator accepts the inline flag");
        assert_eq!(c[0]["searchTerm"], "(?i)https?://\\S+");
        // A pattern longer than the engine takes is refused here, not by the page.
        g.set("FindUrlsRegex", toml::Value::String(format!("a{}", "b?".repeat(600))));
        assert_eq!(urls_commands(&g, &mut model(), SECRET), Err(BAD_URLS_PATTERN));
    }

    /// Find URLs reaches the page one way, and opening a link is the page's `window.open`, which
    /// 13x's gated `Event::NewWindow` path handles: nothing here opens a URL or a window.
    #[test]
    fn find_urls_wiring_opens_nothing() {
        let me = prod(include_str!("browser_find_app.rs"));
        let urls = between(me, "pub(super) fn browser_find_urls", "/// The find family from the menu");
        for banned in ["open_url", "ShellExecute", "NewWindow", "open_browser", "new_tab", "execute_script", "format!("] {
            assert!(!urls.contains(banned), "Find URLs must not use {banned}");
        }
        assert!(urls.contains("urls_commands(") && urls.contains("self.browser_find_send(") && urls.contains("self.client_open(pane)"));
        assert!(urls.contains("self.browser_find_closed(pane)"), "the bar closes first, as iTerm2's does");
        let route = between(me, "Route::FindUrls =>", "Route::Global");
        assert!(route.contains("self.browser_find_urls(id)"));
        let msg = &me[me.find("pub(super) fn browser_find_message").unwrap()..];
        assert!(msg.contains("shortcuts_after_complete(") && msg.contains("self.browser_find_send("), "the shortcuts follow the complete result");
        let labels = prod(include_str!("labels_ui.rs"));
        let reveal = between(labels, "pub(super) fn reveal_shortcuts", "let Some(p) = self.panes.get(&id)");
        assert!(reveal.contains("self.is_browser_pane(id)") && reveal.contains("self.browser_find_urls(id)"), "a browser pane's request goes to the page");
    }

    /// B1: the find strip is cut inside the one layout owner, below the announcement bar; the scene
    /// draws the bar at the rectangle that function gives, the placed view and the hover anchor both
    /// take the view rectangle from `browser_web_rect`, so while the bar is open they agree.
    #[test]
    fn find_strip_has_one_owner() {
        let overlay = include_str!("browser_overlay_app.rs");
        let overlay = prod(overlay);
        let split = between(overlay, "pub(super) fn browser_split", "pub(super) fn browser_web_rect");
        assert!(split.contains("crate::browser_find::split(r, bottom, find_h)") && !split.contains("below_bar(") && !split.contains("+ 8.0"), "the pure split is the one cut, the height is the scene's");
        assert!(split.contains("browser_find_strip_open(id)") && split.contains("scene::find_bar_height(") && !split.contains("pane_chrome("));
        let web = between(overlay, "pub(super) fn browser_web_rect", "pub(super) fn browser_find_strip_open");
        assert!(web.contains("self.browser_split(id, r).1"));
        let shown = between(overlay, "fn browser_shown_rect", "// --- The hovered link");
        assert!(shown.contains("self.browser_web_rect(pane, r)"), "the hover anchor reads the placed view's rectangle");
        let app = prod(include_str!("browser_app.rs"));
        let place = between(app, "pub(super) fn place_browser_views", "fn show_browser_view");
        assert!(place.contains("self.browser_web_rect(id, r)"), "the view is placed in the same rectangle");
        let m = prod(include_str!("mod.rs"));
        assert!(m.contains("self.browser_split(id, rect).0") && m.contains("scene::find_strip(&mut frame, row)") && m.contains("scene::find_bar(&mut frame, &bar, row, self.pane_cell(id))"), "the scene draws the bar at the strip row");
        assert!(m.contains("&& !self.is_browser_pane(tab.focused)"), "the terminal's bar block skips a browser pane");
        // Only the strip's own call sites cut it: the status bar's Search Tool needs none.
        assert!(overlay.contains("self.bar_find_host(id).is_none()"));
    }

    /// B6: one hand-over for the keyboard. The bar takes it with the address field's own
    /// `focus_window` (never a second `SetFocus`), the page is not given focus while a Termurion field
    /// has it, and every path that gives a field the keyboard uses the exclusion functions.
    #[test]
    fn keyboard_hand_over_has_one_path() {
        let me = prod(include_str!("browser_find_app.rs"));
        let take = between(me, "pub(super) fn browser_find_take_keyboard", "pub(super) fn browser_find_yield_to_address");
        assert!(take.contains("crate::webhost_win::focus_window(hwnd)") && take.contains("find_takes_keyboard(&mut bv.bar.keys, f)"));
        assert!(take.contains("g.window.focus_window()"), "the window is activated first when it is not in front");
        assert!(take.contains("self.web_focus_held = None") && take.contains("self.sync_web_keyboard()"), "ReturnFromChild's bookkeeping");
        assert!(!me.contains("windows_sys") && !me.contains("SetFocus("), "no second SetFocus");
        let app = prod(include_str!("browser_app.rs"));
        let place = between(app, "pub(super) fn place_browser_views", "fn show_browser_view");
        assert!(place.contains("self.browser_page_may_focus(*p)") && !place.contains("b.bar.keys"), "a focus change never steals the bar's keys");
        assert!(app.contains("self.browser_page_may_focus(pane)"), "nor does a view made now");
        let focus = between(app, "Event::Focus(_, true) => {", "Event::NewWindow");
        assert!(focus.contains("self.browser_page_took_keyboard(pane)"));
        let url = between(app, "pub(super) fn focus_url_bar", "fn blur_url_bar");
        assert!(url.contains("self.browser_find_yield_to_address(pane)") && !url.contains("bar.keys = true"));
        let f = prod(include_str!("find_ui.rs"));
        let fieldf = between(f, "pub(super) fn bar_field_focus", "pub(super) fn bar_field_close");
        assert!(fieldf.contains("self.browser_find_take_keyboard(pane)"), "the Search Tool's field hands over too");
        let fieldc = between(f, "pub(super) fn bar_field_close", "pub(super) fn open_global_find");
        assert!(fieldc.contains("self.browser_find_closed(pane)"));
    }

    /// The bar, the menu and the page's answers reach the engine one way: data arguments to the main
    /// frame's Termurion world, a Client message read only by the model, nothing built into a script.
    #[test]
    fn find_wiring_follows_iterm2() {
        let me = prod(include_str!("browser_find_app.rs"));
        assert!(!me.contains("format!(") && !me.contains("execute_script") && !me.contains("browser_eval("), "no script text is built here");
        let go = between(me, "fn browser_find_go", "pub(super) fn browser_find_command");
        assert!(go.contains("self.browser_js_call(") && go.contains("browser_find::COMMAND_JS") && go.contains("json!({ \"command\": command })"));
        assert!(go.contains("find_out.answered()"), "the next goes when the previous answered");
        let msg = &me[me.find("pub(super) fn browser_find_message").unwrap()..];
        assert!(msg.find("World::Client").unwrap() < msg.find("on_message(").unwrap() && msg.contains("self.client_open(pane)"), "a page-world message is dropped before it is read");
        let router = prod(include_str!("browser_scripts_app.rs"));
        assert!(router.contains("h if h == crate::browser_find::HANDLER => self.browser_find_message(pane, &r)"));
        let reset = between(router, "pub(super) fn browser_reset_world", "// --- Setup ---");
        assert!(reset.contains("bv.find = crate::browser_find::fresh_model()") && reset.contains("bv.find_out.reset()"), "a new secret is a new search");
        // The bar's paths: terminal search and step become the browser's; Return is forward.
        let f = prod(include_str!("find_ui.rs"));
        let key = between(f, "pub(super) fn find_key", "pub(super) fn find_type_text");
        let one = between(f, "fn find_bar_outcome", "fn find_outcome");
        assert!(key.contains("self.find_bar_outcome(id, outcome, was_filter)") && one.contains("if self.is_browser_pane(id)") && one.contains("bar_act(outcome)") && one.contains("self.browser_find_closed(id)"));
        let search = between(f, "fn search_as", "fn refilter");
        assert!(search.contains("self.browser_find_search(id, fresh)"));
        let step = between(f, "pub(super) fn find_step", "fn find_grow");
        assert!(step.contains("self.browser_find_step(id, !up)"));
        let cmd = between(f, "pub(super) fn run_find_command", "match (selector, label)");
        assert!(cmd.contains("is_find_family(selector)") && cmd.contains("self.browser_find_command(id, selector, label)"));
        // Jump to Selection goes to the one owner; Filter and the text-view actions do nothing.
        assert!(me.contains("Route::Jump => self.browser_jump_to_selection(id)") && !me.contains("JUMP_JS"));
        assert!(me.contains("Route::Nothing => {}") && me.contains("Route::Global => return false"));
        // Use Selection for Find and Pick Result to Open (final review I2): the one reader, the
        // shared pasteboard path, and the shortcuts only with an active search.
        assert!(me.contains("Route::UseSelection => self.browser_use_selection(id)") && me.contains("Route::PickResult => self.browser_pick_result(id)"));
        let us = between(me, "pub(super) fn browser_use_selection", "pub(super) fn browser_pick_result");
        assert!(us.contains("self.browser_read_selection(") && us.contains("selection_query(") && us.contains("set_find_pasteboard_always("));
        let pick = between(me, "pub(super) fn browser_pick_result", "/// A Find Globally search reaches every");
        assert!(pick.contains("active_term().is_none()") && pick.contains("navigation_shortcuts(\"open\"") && pick.contains("self.browser_page_took_keyboard(pane)") && pick.contains(".focus()") && !pick.contains("browser_page_may_focus") && pick.contains("self.client_open(pane)"));
        assert!(prod(include_str!("browser_input_app.rs")).contains("pub(super) fn browser_read_selection"));
    }

    /// Find Globally on a browser pane (`GSEC:443-470`, `FM:414-450`): the search starts the page's
    /// global stream, an older answer is dropped, a pick shows the global results once and reveals.
    #[test]
    fn global_reveal_follows_iterm2() {
        // A page answers the probe: only a find script takes part.
        assert!(global_script_present(&Ok(json!("object"))) && global_script_present(&Ok(json!("function"))));
        assert!(!global_script_present(&Ok(json!("undefined"))) && !global_script_present(&Err("no".into())) && !global_script_present(&Ok(Value::Null)));
        // Search("rust") with the bar's search active: clear it, clear the global, start with 40 characters of context.
        let mut m = model();
        let cmds = global_commands(&mut m, "rust", Mode::default(), SECRET);
        assert_eq!(actions(&cmds), vec!["clearFind", "clearFind", "startFind"]);
        assert_eq!(cmds[0]["instanceId"], "main_11112222333344445555666677778888");
        assert_eq!(cmds[1]["instanceId"], "global_99990000aaaabbbbccccddddeeeeffff");
        assert_eq!(cmds[2]["searchTerm"], "rust");
        assert_eq!(cmds[2]["contextLength"], 40);
        assert!(cmds.iter().all(|c| c["sessionSecret"] == SECRET));
        // The second search of the stream does not clear the bar again.
        assert_eq!(actions(&global_commands(&mut m, "rusty", Mode::default(), SECRET)), vec!["clearFind", "startFind"]);
        // A pick when the last search was global sends the reveal alone; after a local search, showResults first.
        let id = json!({"index": 2});
        assert_eq!(actions(&reveal_commands(&mut m, &id, SECRET)), vec!["reveal"]);
        m.start("local", browser_find::FindMode::CaseInsensitive, true, SECRET);
        assert_eq!(actions(&reveal_commands(&mut m, &id, SECRET)), vec!["showResults", "reveal"]);
        assert_eq!(reveal_commands(&mut m, &json!("not an object"), SECRET).len(), 0, "a bad identifier sends nothing");
        // The wiring: generation guards, the window must be open, the pane is focused, no script text is built.
        let me = prod(include_str!("browser_find_app.rs"));
        let start = between(me, "pub(super) fn browser_global_start", "pub(super) fn browser_global_results");
        assert!(start.contains("generation != app.global_generation") && start.contains("global_script_present(&answer)") && start.contains("GLOBAL_PROBE_JS") && start.contains("self.client_open(*p)"));
        let results = between(me, "pub(super) fn browser_global_results", "pub(super) fn browser_global_reveal");
        assert!(results.contains("b.global_gen != self.global_generation") && results.contains("self.global_find.is_none()") && results.contains("web_hit("));
        let f = prod(include_str!("find_ui.rs"));
        let ev = between(f, "pub(super) fn global_find_event", "fn global_focus_pane");
        let search = between(f, "pub(super) fn global_search", "pub(super) fn global_title");
        assert!(search.contains("self.global_generation += 1") && search.contains("self.browser_global_start(q)") && ev.contains("GlobalEvent::PickWeb(id, identifier)"));
        let web = between(ev, "GlobalEvent::PickWeb(id, identifier)", "GlobalEvent::Pick(id, m)");
        assert!(web.contains("self.global_focus_pane(id)") && web.contains("self.browser_global_reveal(id, &identifier)"));
        // A closed pane or replaced page drops its rows; a closed window drops them all; the counter starts at 1.
        assert!(between(me, "pub(super) fn global_pane_gone", "pub(super) fn global_closed").contains("drop_pane_rows") && between(me, "pub(super) fn global_closed", "pub(super) fn browser_global_reveal").contains("self.global_error = None"));
        let scripts = prod(include_str!("browser_scripts_app.rs"));
        assert!(between(scripts, "pub(super) fn browser_reset_world", "// --- Setup ---").contains("self.global_pane_gone(pane)"));
        // A plain main-frame navigation drops them too (final review I1).
        let nav = between(scripts, "// A main-frame navigation began: every call still waiting is over", "self.browser_tip_clear(pane)");
        assert!(nav.contains("self.global_pane_gone(pane)"), "a navigated page keeps no Find Globally rows");
        let m = prod(include_str!("mod.rs"));
        assert!(between(m, "fn close_pane(&mut self", "self.single_use_ended(id)").contains("self.global_pane_gone(id)") && m.contains("global_generation: 1,"));
        assert!(ev.contains("GlobalEvent::Close => self.global_closed()") && f.contains("CloseRequested => self.global_closed()"));
        assert!(me.contains("Update::Global(results) => self.browser_global_results(pane, results)"));
    }

    /// A peer hidden while the global search ran gets no find state when its answer arrives.
    #[test]
    fn a_peer_hidden_mid_search_gets_no_find_state() {
        let me = prod(include_str!("browser_find_app.rs"));
        let start = between(me, "pub(super) fn browser_global_start", "pub(super) fn browser_global_results");
        let cb = start.find("Box::new(move |app, pane, answer|").expect("the callback");
        let guard = start[cb..].find("app.pane_in_a_layout(pane)").expect("the callback checks the layout");
        assert!(guard < start[cb..].find("global_commands(").unwrap(), "before any find state");
        assert!(start[cb..].find("bv.global_gen = generation").unwrap() > guard, "and before the pane joins the search");
    }

    #[test]
    fn a_picked_row_shows_its_hidden_peer() {
        let f = prod(include_str!("find_ui.rs"));
        let focus = between(f, "fn global_focus_pane", "/// The Find Globally list");
        assert!(focus.find("self.reveal_as_peer(id)").expect("the peer is shown") < focus.find("self.enter_pane_window(id)").unwrap(), "shown before its window and tab are found");
    }

    /// 13y1 merge seam S4: a hidden workgroup peer (in no layout) gets no find state. Find Globally
    /// searches the layouts only, and a row picked after a peer swap hid its pane reveals nothing.
    #[test]
    fn hidden_peers_get_no_find_state() {
        let me = prod(include_str!("browser_find_app.rs"));
        let start = between(me, "pub(super) fn browser_global_start", "pub(super) fn browser_global_results");
        assert!(start.contains("self.all_tab_panes()"), "the search reaches the layouts only");
        let reveal = between(me, "pub(super) fn browser_global_reveal", "pub(super) fn browser_find_command");
        let guard = reveal.find("self.pane_in_a_layout(pane)").expect("layout guard");
        assert!(guard < reveal.find("reveal_commands(").unwrap(), "the guard comes before any find state");
        let windows = prod(include_str!("windows_ui.rs"));
        assert!(between(windows, "pub(super) fn all_tab_panes", "\n    }\n").contains("layout.panes()"), "a hidden peer is in no layout");
    }

    /// 13x3 Task 9: a find for "world", then "/" from copy mode opens the find bar with the query
    /// kept (`CPH:480-483`, ShowFindPanel). The copy-mode key is the shared table's, the step is the
    /// one the app sends through `browser_find_command`, and Open reopens the bar on the same query.
    #[test]
    fn find_then_copy_mode() {
        use crate::browser_copymode::{Model as CopyModel, Step};
        let mut s = Sim::new();
        for c in "world".chars() {
            s.key(BarKey::Text(c.to_string()), false);
        }
        s.due(Mode::SmartCase);
        assert_eq!(actions(&s.sent), ["startFind"]);
        assert_eq!(s.sent[0]["searchTerm"], "world");
        // Esc-free hand-over: the bar stops typing when copy mode takes the keys (the page takes the keyboard).
        let mut keys = false;
        page_takes_keyboard(&mut keys, Some(&mut s.f));
        assert!(!s.f.typing && s.f.open);
        // "/" in copy mode is Find, and the app answers it with the Find... command.
        assert_eq!(CopyModel::default().key("/", SECRET), Step::Find);
        let apply = between(prod(include_str!("browser_copymode_app.rs")), "fn browser_copy_mode_apply", "\n}\n");
        assert!(apply.contains("Step::Find =>") && apply.contains("self.browser_find_command(pane, \"performFindPanelAction:\", \"Find...\")"));
        assert_eq!(browser_find::route("performFindPanelAction:", "Find..."), browser_find::Route::Open);
        // Open: the bar opens and takes the keyboard, the query is still "world" and is searched again.
        s.f.open();
        find_takes_keyboard(&mut keys, &mut s.f);
        let q = s.f.query.clone();
        s.f.set_query(&q, s.t);
        assert!(s.f.open && s.f.typing && !keys);
        assert_eq!(s.f.query, "world", "the query is kept");
        s.sent.clear();
        s.due(Mode::SmartCase);
        assert_eq!(actions(&s.sent), ["startFind"]);
        assert_eq!(s.sent[0]["searchTerm"], "world");
    }
}

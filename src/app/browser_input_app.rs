//! Keys in a browser pane (milestone 13x2, Task 6). A key reaches here only from the person: the
//! 13u keyboard hook and WebView2's `AcceleratorKeyPressed` (through `Event::Key`), never from a
//! page script event. The key gate (`browser_keys::GateState`, read inside the COM handler) decides
//! synchronously whether Tandem takes the key before the page sees it; this side repeats the route
//! and runs what iTerm2's browser would (`BAP`): scroll, send text into the focused field, extend
//! the selection, copy or paste, or hand the binding to the session. Bound text goes through
//! `Input.insertText` or iTerm2's insert-text.js, never to a pty (a browser pane has none).

use super::*;
use crate::browser_keys::{self, Do, Facts, GateState, KeyAct, Owner};
use super::browser_scripts_app::{JsDone, PageOpen};
use crate::browser_pointer::{self, Press, PointerAct};
use crate::browser_smart::WebMatch;
use crate::keyroute::Route;
use crate::pointer::{self, Down, PointerBinding, Up};
use crate::webhost;

/// What a Smart Selection answers with (`WebMatch` or none).
pub(super) type SmartDone = Box<dyn FnOnce(&mut App, PaneId, Option<WebMatch>)>;

/// What a pointer press, or the page's answer to one, asks that needs the event loop: the tick
/// that follows has it. Every URL goes through `browser_page_open` there.
pub(super) enum PageNext {
    Open(PaneId, String, PageOpen),
    /// A Smart Selection rule's first action on a match (`BWV:528-546`).
    Smart(PaneId, crate::smartsel::RuleAction, Vec<String>),
    /// The session's own pointer actions (tabs, windows, panes, profiles).
    Session(PaneId, PointerBinding),
}

/// Text sent or pasted into a page is bounded (the page's own paste limit).
const MAX_TEXT: usize = 1_048_576;

impl App {
    /// The gate pane `pane`'s view asks.
    #[allow(dead_code)] // the pointer gate of Task 7 and 13y1's peers reach it too
    pub(super) fn browser_key_gate(&self, pane: PaneId) -> webhost::KeyGate {
        self.browser_views.get(&pane).map(|b| b.gate.clone()).unwrap_or_default()
    }

    /// What the gate needs from this window (the focused-pane copy `with_route` makes, for the
    /// browser pane's own profile).
    fn browser_gate_state(&self, pane: PaneId) -> Option<GateState> {
        let bv = self.browser_views.get(&pane)?;
        let p = self.panes.get(&pane)?;
        let profile_map = self.store.get(&p.profile).and_then(|p| p.get("Keyboard Map")).and_then(|v| v.as_table().cloned()).unwrap_or_default();
        Some(GateState {
            profile_map,
            global_map: self.globals.table("GlobalKeyMap"),
            language_agnostic: self.globals.bool("LanguageAgnosticKeyBindings"),
            leader: crate::keystroke::parse(&self.globals.string("Leader")),
            leader_on: self.leader_on,
            switch: [self.globals.int("SwitchTabModifier"), self.globals.int("SwitchPaneModifier"), self.globals.int("SwitchWindowModifier")],
            bar: self.bars.get(&pane).and_then(|b| b.front()).cloned(),
            editing: bv.editing,
        })
    }

    /// Rebuilds the gate of every browser pane of this window (profile, key map, setting changes).
    pub(super) fn browser_gates_refresh(&mut self) {
        let panes: Vec<PaneId> = self.browser_views.keys().copied().filter(|p| self.panes.contains_key(p)).collect();
        for pane in panes {
            self.browser_gate_refresh(pane);
            self.browser_pointer_sync(pane);
        }
    }

    fn browser_gate_refresh(&mut self, pane: PaneId) {
        let Some(state) = self.browser_gate_state(pane) else { return };
        let sig = (state.editing, state.bar.as_ref().map(|b| b.id.clone()), state.leader_on);
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        bv.gate_sig = Some(sig);
        // The closure holds copies only: it runs inside the COM handler, with no App borrow.
        bv.gate.0.replace(webhost::KeyTable { take: Box::new(move |vk, mods| state.owner(vk, mods).taken()) });
    }

    /// Editing, the front bar and the Leader change without a settings change: the gate follows them.
    pub(super) fn browser_gates_sync(&mut self) {
        let stale: Vec<PaneId> = self
            .browser_views
            .iter()
            .filter(|(p, bv)| {
                self.panes.contains_key(*p) && {
                    let sig = (bv.editing, self.bars.get(*p).and_then(|b| b.front()).map(|b| b.id.clone()), self.leader_on);
                    bv.gate_sig.as_ref() != Some(&sig)
                }
            })
            .map(|(p, _)| *p)
            .collect();
        for pane in stale {
            self.browser_gate_refresh(pane);
        }
    }

    /// A key the page raised. `taken` is the gate's answer when the handler ran (the page never saw
    /// the key); the route is repeated here with the state of now, and a key the gate took that no
    /// longer routes to anything does nothing (P16).
    pub(super) fn browser_accel_key(&mut self, pane: PaneId, vk: u32, mods: u8, taken: bool, event_loop: &ActiveEventLoop) {
        // A key the page kept ends a pending Leader (`IA:552-572`); a modifier alone is not a key.
        if !taken && self.leader_on && !matches!(vk, 0x10 | 0x11 | 0x12 | 0x5B | 0x5C | 0xA0..=0xA5) {
            self.in_pane_window(pane, |a| {
                a.leader_on = false;
                a.refresh_route();
            });
        }
        // The gate's order, repeated: a bar's button first (its letter may be D), a person's binding
        // before the address keys, then 13x's accelerators and Win chords.
        let state = self.browser_gate_state(pane);
        let owner = state.as_ref().map(|s| s.owner(vk, mods));
        match owner {
            Some(Owner::Bar(i)) if taken => return self.browser_bar_press(pane, i, event_loop),
            Some(Owner::Binding) if taken && mods & browser_keys::WIN == 0 => {}
            _ => {
                if self.browser_page_key(pane, vk, mods, event_loop) {
                    return;
                }
            }
        }
        if !taken || mods & browser_keys::WIN != 0 {
            return;
        }
        let (Some(state), Some(chord)) = (state, browser_keys::chord_of(vk, mods)) else { return };
        match state.owner(vk, mods) {
            Owner::Bar(i) => self.browser_bar_press(pane, i, event_loop),
            Owner::Page | Owner::Address => {}
            Owner::Binding | Owner::Number | Owner::Leader => {
                let route = state.route(&chord);
                // What the key types in the person's layout (Copy or Send's character, `BAP:171`).
                let typed = browser_keys::layout_text(vk, mods);
                self.in_pane_window(pane, |a| a.browser_run_route(pane, route, &chord, typed.as_deref(), event_loop));
            }
        }
    }

    /// Alt+letter on the front announcement bar (as `bar_key`'s).
    fn browser_bar_press(&mut self, pane: PaneId, i: usize, event_loop: &ActiveEventLoop) {
        let Some(bar) = self.bars.get(&pane).and_then(|b| b.front()).cloned() else { return };
        let Some(button) = bar.buttons.get(i) else { return };
        if let Some(b) = self.bars.get_mut(&pane) {
            b.pop();
        }
        self.bar_answered(pane, &bar.id, Some(button.as_str()), event_loop);
        self.redraw();
    }

    /// `route_press` for a browser pane's key (the Terminal route is the page's, never here).
    fn browser_run_route(&mut self, pane: PaneId, route: Route, chord: &crate::menus::Chord, typed: Option<&str>, event_loop: &ActiveEventLoop) {
        let was_leader = std::mem::replace(&mut self.leader_on, false);
        match route {
            Route::LeaderOn => {
                self.leader_on = true;
                self.refresh_route();
            }
            Route::SelectWindow(d) => self.select_window_number(d),
            Route::SelectPane(d) => {
                if let Some(id) = self.tabs.get(self.active).and_then(|t| self.pane_by_number(t, d)) {
                    self.focus_quietly(self.active, id);
                }
            }
            Route::SelectTab(d) => {
                let i = if d == 9 { self.tabs.len().saturating_sub(1) } else { d as usize - 1 };
                if i < self.tabs.len() {
                    self.select_tab(i);
                }
            }
            Route::Binding(b) => self.browser_binding(pane, &b, chord, typed, event_loop),
            Route::Terminal => {}
        }
        if was_leader {
            self.refresh_route();
        }
    }

    /// One binding in a browser pane (`iTermBrowserKeyBindingActionPerformer.perform`).
    fn browser_binding(&mut self, pane: PaneId, b: &crate::keyaction::Binding, chord: &crate::menus::Chord, typed: Option<&str>, event_loop: &ActiveEventLoop) {
        let act = browser_keys::classify(b);
        // Set Profile only to a browser profile; anything else does nothing (`BAP:42-48`).
        if let KeyAct::SetProfile(guid) = &act {
            if self.store.get(guid).is_some_and(|p| self.is_browser_profile(p)) {
                self.set_profile_on(pane, guid);
            }
            return;
        }
        let own = browser_keys::own_char(chord, typed).unwrap_or_default();
        if act == KeyAct::CopyOrSend {
            // The page answers whether anything is selected (iTerm2's `actionPerformingHasSelection`).
            let script = crate::browser_js::resource("has-selection.js").unwrap_or_default();
            self.browser_eval_in_frame(
                pane,
                None,
                script,
                Box::new(move |a, p, r| {
                    let facts = Facts { has_selection: matches!(r, Ok(serde_json::Value::Bool(true))), clipboard_text: false };
                    for d in browser_keys::plan(&KeyAct::CopyOrSend, &own, &facts) {
                        a.browser_do_basic(p, d);
                    }
                }),
            );
            return;
        }
        let clipboard_text = act == KeyAct::PasteOrSend && self.clipboard_text().is_some_and(|t| !t.is_empty());
        for d in browser_keys::plan(&act, &own, &Facts { has_selection: false, clipboard_text }) {
            match d {
                Do::Session => {
                    if self.focused_id() == Some(pane) {
                        self.run_key_action(b, None, Some(chord), event_loop);
                    }
                }
                Do::CopyMode(keys) => self.browser_copy_mode_commands(pane, &keys),
                Do::Toggle(json) => self.toggle_setting(&json),
                // iTerm2 sends the snippet's value as it is (`BAP:122-127`).
                Do::Snippet(key) => {
                    if let Some(value) = self.snippets.find_key(&key).map(|s| s.value.clone()) {
                        self.browser_send_text(pane, &value);
                    }
                }
                Do::PasteSpecial { config, from_selection } => self.browser_paste_special(pane, &config, from_selection),
                other => self.browser_do_basic(pane, other),
            }
        }
    }

    /// The effects that need nothing but the pane (they also run from a page answer).
    fn browser_do_basic(&mut self, pane: PaneId, d: Do) {
        match d {
            Do::PageScript(s) => self.browser_eval(pane, &s, Box::new(|_, _, _| {})),
            Do::WorldScript(s) => self.browser_eval_in_frame(pane, None, &s, Box::new(|_, _, _| {})),
            Do::Insert(t) => self.browser_send_text(pane, &t),
            Do::Copy => self.browser_copy_selection(pane),
            Do::Paste => self.browser_paste(pane),
            _ => {}
        }
    }

    /// Scrolls the page (`WVA:19-38`).
    #[allow(dead_code)] // key actions reach it through `plan`; the pointer actions of Task 7 call it directly
    pub(super) fn browser_scroll(&mut self, pane: PaneId, s: browser_keys::Scroll) {
        self.browser_do_basic(pane, Do::PageScript(browser_keys::scroll_script(s).to_string()));
    }

    /// Moves one end of the page's selection (`WVA:379-433`), in Tandem's world.
    #[allow(dead_code)] // reached through `plan`
    pub(super) fn browser_extend(&mut self, pane: PaneId, start: bool, forward: bool, unit: i64) {
        if let Some(s) = browser_keys::extend_script(start, forward, unit) {
            self.browser_do_basic(pane, Do::WorldScript(s));
        }
    }

    /// Types `text` into the focused field of the page: insert-text.js first, and DevTools
    /// `Input.insertText` when the page reports it did not insert (`WVA:40-67`; Decision: no
    /// clipboard hack). Only a person's key or pointer binding calls this, never page script.
    pub(super) fn browser_send_text(&mut self, pane: PaneId, text: &str) {
        if text.is_empty() || text.len() > MAX_TEXT {
            return;
        }
        let raw = text.to_string();
        self.browser_eval(
            pane,
            &browser_keys::insert_script(text),
            Box::new(move |a, p, r| {
                if !matches!(r, Ok(serde_json::Value::Bool(true))) {
                    a.browser_insert_raw(p, &raw);
                }
            }),
        );
    }

    /// DevTools `Input.insertText` into whatever has the focus.
    fn browser_insert_raw(&mut self, pane: PaneId, text: &str) {
        self.browser_devtools(pane, "Input.insertText", serde_json::json!({ "text": text }), Box::new(|_, _, _| {}));
    }

    /// Copies the page's selection (Decision: its text, not the page's copy command, which needs a
    /// user gesture in Chromium). Read in Tandem's world, so the page cannot choose the text.
    pub(super) fn browser_copy_selection(&mut self, pane: PaneId) {
        self.browser_read_selection(pane, Box::new(|a, _, text| {
            if !text.is_empty() {
                a.set_clipboard_text(text.clone());
                a.remember_paste(&text);
            }
        }));
    }

    fn browser_read_selection(&mut self, pane: PaneId, then: Box<dyn FnOnce(&mut App, PaneId, String)>) {
        self.browser_eval_in_frame(
            pane,
            None,
            "window.getSelection().toString()",
            Box::new(move |a, p, r| {
                if let Ok(serde_json::Value::String(t)) = r {
                    then(a, p, crate::browser_js::store_selection(&t));
                }
            }),
        );
    }

    /// Pastes the person's clipboard into the page's focused field; the page is never given
    /// permission to read it.
    pub(super) fn browser_paste(&mut self, pane: PaneId) {
        if let Some(text) = self.clipboard_text().filter(|t| !t.is_empty() && t.len() <= MAX_TEXT) {
            self.browser_insert_raw(pane, &text);
        }
    }

    /// Paste... and Paste from Selection... (`BVC:1590-1600`): the paste configuration's
    /// transforms, then the text is inserted (a browser has no bracketed paste).
    fn browser_paste_special(&mut self, pane: PaneId, config: &str, from_selection: bool) {
        let config = config.to_string();
        if from_selection {
            self.browser_read_selection(pane, Box::new(move |a, p, text| a.browser_paste_text(p, &config, &text)));
        } else if let Some(text) = self.clipboard_text() {
            self.browser_paste_text(pane, &config, &text);
        }
    }

    fn browser_paste_text(&mut self, pane: PaneId, config: &str, text: &str) {
        if text.is_empty() {
            return;
        }
        let (mut job, _) = crate::paste::Config::decode(config).job(text);
        job.flags &= !crate::paste::BRACKET;
        let prefs = crate::paste::Prefs { escape_with_quotes: self.globals.bool("EscapeWithQuotes"), strip_zero_width: self.globals.bool("StripZeroWidthFormatCharactersOnPaste") };
        let out = crate::paste::sanitize(&job, &prefs);
        if !out.is_empty() && out.len() <= MAX_TEXT {
            self.browser_insert_raw(pane, &out);
        }
    }

    // --- Pointer bindings (Task 7) ---

    /// The table the gate script reads: the keys a binding claims, the fourth-click Smart Selection
    /// (always on, as iTerm2's `BWV:414-433`) and the focus preference.
    pub(super) fn browser_pointer_table(&self) -> (Vec<String>, bool, bool) {
        let keys = browser_pointer::gate_keys(&self.globals.table("PointerActions"), &self.pointer_prefs());
        (keys, true, self.globals.bool("FocusOnRightOrMiddleClick"))
    }

    /// Gives every live world the table of now when it changed (a map or preference edit; no reload).
    pub(super) fn browser_pointer_sync(&mut self, pane: PaneId) {
        if !self.client_open(pane) {
            return;
        }
        let (keys, four, focus) = self.browser_pointer_table();
        let sig = (keys.clone(), focus);
        if self.browser_views.get(&pane).is_none_or(|b| b.pointer_sig.as_ref() == Some(&sig)) {
            return;
        }
        if let Some(bv) = self.browser_views.get_mut(&pane) {
            bv.pointer_sig = Some(sig);
        }
        self.browser_world_update(pane, &browser_pointer::gate_update(&keys, four, focus));
    }

    /// A new Tandem world (a document that loaded) reads the table of now, not the one the script was
    /// registered with.
    pub(super) fn browser_pointer_push_new(&mut self, pane: PaneId, session: Option<&str>, json: &str) {
        if !self.client_open(pane) {
            return;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return };
        let c = &v["context"];
        let named = c["name"].as_str() == Some(crate::browser_world::WORLD_NAME) && c.pointer("/auxData/type").and_then(|t| t.as_str()) == Some("isolated");
        let Some(unique) = c["uniqueId"].as_str().filter(|_| named) else { return };
        let (keys, four, focus) = self.browser_pointer_table();
        let params = crate::browser_world::evaluate_params(unique, &browser_pointer::gate_update(&keys, four, focus));
        let session = session.map(str::to_string);
        self.devtools_send(pane, session.as_deref(), "Runtime.evaluate", &params, Box::new(|_, _, _| {}));
    }

    fn browser_rules(&self, pane: PaneId) -> Vec<crate::smartsel::Rule> {
        self.panes.get(&pane).and_then(|p| self.store.get(&p.profile)).map(crate::smartsel::rules).unwrap_or_default()
    }

    /// A press the gate script took (a trusted one: the script ignores every other). Down remembers a
    /// press a binding owns; its release runs the binding (`PointerController`).
    pub(super) fn browser_pointer(&mut self, pane: PaneId, press: Press) {
        let map = self.globals.table("PointerActions");
        let prefs = self.pointer_prefs();
        match press {
            Press::Down { button, clicks, mods, .. } => {
                let bound = pointer::on_down(button, clicks, mods, false, false, &map, &prefs) == Down::Bound;
                if let Some(bv) = self.browser_views.get_mut(&pane) {
                    bv.pointer_down = bound.then_some((button, clicks));
                }
            }
            Press::Up { button, clicks, mods, x, y } => {
                let Some(bv) = self.browser_views.get_mut(&pane) else { return };
                if bv.pointer_down.take() != Some((button, clicks)) {
                    return;
                }
                if let Up::Perform(b) = pointer::on_up(button, clicks, mods, false, false, &map, &prefs) {
                    self.browser_pointer_perform(pane, &b, (x, y));
                }
            }
            Press::SmartSelect { x, y } => {
                self.browser_smart_select(pane, (x, y), false, Box::new(|a, p, m| {
                    if m.is_some() && a.globals.bool("CopySelection") {
                        a.browser_copy_selection(p);
                    }
                }));
            }
            // Win-click: the best overall match is selected (iTerm2 matches with requireAction false,
            // `iTermBrowserWebView+Actions.swift:231`) and runs its rule's first action if it has one;
            // the selection is not copied (`WVA:254`).
            Press::SmartAction { x, y } => {
                self.browser_smart_select(pane, (x, y), false, Box::new(|a, p, m| {
                    let Some(m) = m else { return };
                    let action = crate::browser_smart::win_click_action(&a.browser_rules(p), &m);
                    if let Some(action) = action {
                        a.browser_next.push(PageNext::Smart(p, action, m.captures));
                    }
                }));
            }
            Press::Focus => {
                if self.globals.bool("FocusOnRightOrMiddleClick") {
                    self.in_pane_window(pane, |a| {
                        if let Some(t) = a.tabs.iter_mut().find(|t| t.layout.panes().contains(&pane)) {
                            t.focused = pane;
                        }
                        a.redraw();
                    });
                    if let Some(v) = self.browser_views.get(&pane).and_then(|b| b.view.as_ref()) {
                        v.focus();
                    }
                }
            }
        }
    }

    /// One binding of the pointer map, at the page point `js` (`PAP:13-173`).
    fn browser_pointer_perform(&mut self, pane: PaneId, b: &PointerBinding, js: (f64, f64)) {
        match browser_pointer::classify(&b.action) {
            PointerAct::Session => self.browser_next.push(PageNext::Session(pane, b.clone())),
            PointerAct::OpenLink { new_tab } => self.browser_link_at(pane, js, Box::new(move |a, p, r| {
                if let Some(url) = r.ok().and_then(|v| browser_pointer::link_of(&v)) {
                    a.browser_queue_open(p, &url, if new_tab { PageOpen::Tab } else { PageOpen::Here });
                }
            })),
            PointerAct::CopyLinkAddress => self.browser_link_at(pane, js, Box::new(|a, p, r| {
                if let Some(url) = r.ok().and_then(|v| browser_pointer::link_of(&v)) {
                    a.set_clipboard_text(url.clone());
                    a.remember_paste(&url);
                    a.browser_toast(p, crate::browser_tip::COPIED, crate::browser_tip::COPIED_SECS);
                }
            })),
            PointerAct::QuickLook => {
                let script = self.browser_point_script("urls-at-point.js", js, &[]);
                self.browser_eval_in_frame(pane, None, &script, Box::new(|a, p, r| {
                    if let Some(url) = r.ok().and_then(|v| browser_pointer::first_url_of(&v)) {
                        if crate::web_popover::quick_look_scheme_ok(&url) {
                            let at = crate::browser_tip::pointer_now().unwrap_or_else(|| a.pointer_screen());
                            a.browser_queue_open_gated(p, &url, PageOpen::QuickLook(at));
                        }
                    }
                }));
            }
            PointerAct::SmartSelect => {
                self.browser_smart_select(pane, js, false, Box::new(|a, p, m| {
                    if m.is_some() && a.globals.bool("CopySelection") {
                        a.browser_copy_selection(p);
                    }
                }));
            }
            PointerAct::ExtendSelection => {
                let script = self.browser_point_script("extend-selection-to-point.js", js, &[]);
                self.browser_eval_in_frame(pane, None, &script, Box::new(|_, _, _| {}));
            }
            PointerAct::ContextMenu => self.browser_context_menu_at(pane, js),
            PointerAct::PasteClipboard => self.browser_paste(pane),
            PointerAct::PasteSelection => self.browser_read_selection(pane, Box::new(|a, p, text| {
                if !text.is_empty() && text.len() <= MAX_TEXT {
                    a.browser_insert_raw(p, &text);
                }
            })),
            PointerAct::CopyOrPaste => {
                let script = crate::browser_js::resource("has-selection.js").unwrap_or_default();
                self.browser_eval_in_frame(pane, None, script, Box::new(|a, p, r| {
                    if matches!(r, Ok(serde_json::Value::Bool(true))) {
                        a.browser_copy_selection(p);
                    } else {
                        a.browser_paste(p);
                    }
                }));
            }
            PointerAct::SendText => {
                let text = if b.action == "kSendHexCodePointerAction" {
                    String::from_utf8(crate::keyaction::hex_bytes(&b.argument)).unwrap_or_default()
                } else {
                    crate::keyaction::expand(&b.argument, b.escaping())
                };
                self.browser_send_text(pane, &text);
            }
            PointerAct::Nothing => {}
        }
    }

    /// The right click iTerm2's Open Context Menu is (`BVC:1494-1500`): a trusted right press through
    /// DevTools at the point, so the page may intercept it and WebView2 shows its menu with Tandem's
    /// items (Task 3). Not done when the right button has a binding of its own (the gate would take it).
    fn browser_context_menu_at(&mut self, pane: PaneId, js: (f64, f64)) {
        let (keys, _, _) = self.browser_pointer_table();
        if keys.contains(&browser_pointer::js_press_key(1, 1, &pointer::Mods::default())) {
            return;
        }
        for kind in ["mousePressed", "mouseReleased"] {
            let params = serde_json::json!({ "type": kind, "x": js.0, "y": js.1, "button": "right", "clickCount": 1 });
            self.browser_devtools(pane, "Input.dispatchMouseEvent", params, Box::new(|_, _, _| {}));
        }
    }

    /// A point script (`{{X}}`, `{{Y}}` and `extra`), filled as iTerm2's loader does.
    fn browser_point_script(&self, name: &str, js: (f64, f64), extra: &[(&str, String)]) -> String {
        let (x, y) = (browser_pointer::num(js.0), browser_pointer::num(js.1));
        let mut subs: Vec<(&str, &str)> = vec![("X", &x), ("Y", &y)];
        subs.extend(extra.iter().map(|(k, v)| (*k, v.as_str())));
        crate::browser_js::fill_template(crate::browser_js::resource(name).unwrap_or_default(), &subs)
    }

    /// The link at a page point (`WVA:158-178`), in Tandem's world: the page cannot choose what the
    /// script finds by overriding `elementFromPoint`.
    pub(super) fn browser_link_at(&mut self, pane: PaneId, js: (f64, f64), done: JsDone) {
        let script = self.browser_point_script("open-link-at-point.js", js, &[]);
        self.browser_eval_in_frame(pane, None, &script, done);
    }

    /// Smart Selection at a page point (`WVA:201-259`, `:306-377`): the text around the point, the
    /// best rule match over it, and the selection of the match. `then` gets the match (None when the
    /// profile has no rules or nothing matched) after the selection is made. With `require_action`
    /// only rules that have an action count, and the caller copies nothing.
    pub(super) fn browser_smart_select(&mut self, pane: PaneId, js: (f64, f64), require_action: bool, then: SmartDone) {
        let rules = self.browser_rules(pane);
        if rules.is_empty() {
            return then(self, pane, None);
        }
        let radius = super::semantic_ui::radius(&self.globals).to_string();
        let script = self.browser_point_script("extract-text-at-point.js", js, &[("RADIUS", radius)]);
        self.browser_eval_in_frame(pane, None, &script, Box::new(move |a, p, r| {
            let text = r.ok().and_then(|v| Some((v.get("before")?.as_str()?.to_string(), v.get("after")?.as_str()?.to_string())));
            let found = text.and_then(|(before, after)| crate::browser_smart::first_match(&before, &after, &rules, require_action));
            let Some(m) = found else { return then(a, p, None) };
            let select = a.browser_point_script("select-text.js", js, &[("BEFORE_COUNT", m.before.to_string()), ("AFTER_COUNT", m.after.to_string())]);
            a.browser_eval_in_frame(p, None, &select, Box::new(move |a, p, _| then(a, p, Some(m))));
        }));
    }

    /// The page's selection scrolls to the middle of the view (`BVC:295-311`), in the page.
    pub(super) fn browser_jump_to_selection(&mut self, pane: PaneId) {
        const SCRIPT: &str = "(function() { try { const sel = window.getSelection(); if (sel.rangeCount === 0) return; const rect = sel.getRangeAt(0).getBoundingClientRect(); window.scrollTo({ top: window.scrollY + rect.top - window.innerHeight/2, behavior: 'smooth' }); } catch (e) { console.error(e); } })();";
        self.browser_eval(pane, SCRIPT, Box::new(|_, _, _| {}));
    }

    /// A page URL for the next tick, only when `nav_gate` says Navigate: a pointer click never opens a
    /// local file or runs anything. A network or script URL is a banner (the gate's refusal).
    fn browser_queue_open(&mut self, pane: PaneId, raw: &str, how: PageOpen) {
        match crate::browser_js::nav_gate(raw) {
            crate::browser_js::NavGate::Navigate(_) => self.browser_next.push(PageNext::Open(pane, raw.to_string(), how)),
            crate::browser_js::NavGate::Blocked(why) => {
                self.banners.set("action", why);
                self.redraw();
            }
            crate::browser_js::NavGate::Open(_) => {}
        }
    }

    fn browser_queue_open_gated(&mut self, pane: PaneId, raw: &str, how: PageOpen) {
        if matches!(crate::browser_js::nav_gate(raw), crate::browser_js::NavGate::Navigate(_)) {
            self.browser_next.push(PageNext::Open(pane, raw.to_string(), how));
        }
    }

    /// The tick that has the event loop: opens, Smart Selection actions and session actions.
    pub(super) fn browser_next_tick(&mut self, event_loop: &ActiveEventLoop) {
        for next in std::mem::take(&mut self.browser_next) {
            match next {
                PageNext::Open(pane, url, how) if self.browser_views.contains_key(&pane) => self.browser_page_open(pane, &url, how, event_loop),
                PageNext::Smart(pane, action, captures) if self.browser_views.contains_key(&pane) => self.browser_smart_action(pane, &action, &captures, event_loop),
                PageNext::Session(pane, b) if self.browser_views.contains_key(&pane) => {
                    self.in_pane_window(pane, |a| a.run_pointer_action(&b, pane, (0, 0), event_loop));
                }
                _ => {}
            }
        }
    }

    /// Key action 73 (copy mode): 13x3 Task 8 replaces this body.
    pub(super) fn browser_copy_mode_commands(&mut self, _pane: PaneId, _keys: &str) {}
}

impl App {
    // --- Broadcast between browser panes (Task 8) ---

    /// What the broadcast model and the panes say of `pane`, for `broadcast_peers`.
    fn browser_peer(&self, pane: PaneId) -> Option<browser_keys::Peer> {
        let io = self.panes.get(&pane)?.browser_io()?;
        let bv = self.browser_views.get(&pane)?;
        Some(browser_keys::Peer { browser: true, live: bv.view.is_some(), shown: bv.replay.shown, dev_null: io.dev_null })
    }

    /// Whether `sender`'s broadcast message is acted on: the pane is a broadcast source now, no
    /// replay is in flight to it, the last one ended a hold-off ago, and the message is not the
    /// report of an input the pane was sent (the echo guard). A replay that never answered does not
    /// count after a while.
    pub(super) fn browser_broadcast_accepts(&mut self, sender: PaneId, e: &browser_keys::BroadcastEvent) -> bool {
        let view = self.broadcast_view_in(true);
        let sends = self.is_browser_pane(sender) && self.broadcast.sends(&view, sender);
        let Some(bv) = self.browser_views.get_mut(&sender) else { return false };
        let since = bv.receiving_since.map(|t| t.elapsed());
        let stale = since.is_none_or(|d| d.as_secs() >= browser_keys::RECEIVING_STALE_SECS);
        let receiving = if stale { 0 } else { bv.receiving_broadcast };
        // The echo is consumed before the hold-off verdict (final review minor 5).
        browser_keys::broadcast_verdict(&mut bv.echo, e, Instant::now(), sends, receiving, since)
    }

    /// The panes that get `sender`'s browser input: the other browser panes of its broadcast domain
    /// (`PSB:398-407`), live, shown and of its privacy class.
    pub(super) fn browser_broadcast_targets(&self, sender: PaneId) -> Vec<PaneId> {
        let view = self.broadcast_view_in(true);
        let domain = self.broadcast.targets(&view, sender);
        browser_keys::broadcast_peers(&domain, sender, |p| self.browser_peer(p))
    }

    /// Types what the person typed in `sender` into the other browser panes of the broadcast set
    /// (`BM:882-899`). The replay goes to each target page's focused element through DevTools, which
    /// is trusted in the page, so the suppression is native (`receiving_broadcast`); the target's
    /// view is never focused and OS focus stays where the person is typing.
    pub(super) fn browser_broadcast(&mut self, sender: PaneId, e: browser_keys::BroadcastEvent) {
        let calls = browser_keys::replay_calls(&e);
        for target in self.browser_broadcast_targets(sender) {
            if let Some(bv) = self.browser_views.get_mut(&target) {
                bv.echo.note(&e, Instant::now());
            }
            self.browser_replay(sender, target, e.clone(), calls.clone());
        }
    }

    /// One event's replay on one target: the count goes up first, then the target asks whether its
    /// focused field holds a secret (then nothing is typed there), then the calls run in order; the
    /// count goes down when the last one answers, or when any step fails. A replay that types nothing
    /// (the guard refused, a step failed) forgets the echo it noted (final review minor 5).
    fn browser_replay(&mut self, sender: PaneId, target: PaneId, e: browser_keys::BroadcastEvent, calls: Vec<(&'static str, serde_json::Value)>) {
        let Some(bv) = self.browser_views.get_mut(&target) else { return };
        let stale = bv.receiving_since.is_none_or(|t| t.elapsed().as_secs() >= browser_keys::RECEIVING_STALE_SECS);
        if stale {
            bv.receiving_broadcast = 0;
        }
        bv.receiving_broadcast = browser_keys::receiving_raise(bv.receiving_broadcast);
        bv.receiving_since = Some(Instant::now());
        self.browser_js_call(
            target,
            &browser_keys::guard_body(),
            serde_json::Value::Null,
            Box::new(move |a, p, r| {
                if matches!(r, Ok(serde_json::Value::Bool(false))) {
                    a.browser_replay_step(sender, p, e, calls);
                } else {
                    a.browser_replay_forget(p, &e);
                    a.browser_replay_done(p);
                }
            }),
        );
    }

    /// The next call of a replay. A target that left `sender`'s broadcast set (or broadcast is off)
    /// gets no more of it (deferred minor 221).
    fn browser_replay_step(&mut self, sender: PaneId, pane: PaneId, e: browser_keys::BroadcastEvent, mut calls: Vec<(&'static str, serde_json::Value)>) {
        if calls.is_empty() || !self.browser_broadcast_targets(sender).contains(&pane) {
            return self.browser_replay_done(pane);
        }
        let (method, params) = calls.remove(0);
        self.browser_devtools(
            pane,
            method,
            params,
            Box::new(move |a, p, r| {
                if r.is_ok() {
                    a.browser_replay_step(sender, p, e, calls)
                } else {
                    a.browser_replay_forget(p, &e);
                    a.browser_replay_done(p)
                }
            }),
        );
    }

    /// The replay of `e` typed nothing in `pane`: no echo will come.
    fn browser_replay_forget(&mut self, pane: PaneId, e: &browser_keys::BroadcastEvent) {
        if let Some(bv) = self.browser_views.get_mut(&pane) {
            bv.echo.forget(e);
        }
    }

    fn browser_replay_done(&mut self, pane: PaneId) {
        if let Some(bv) = self.browser_views.get_mut(&pane) {
            bv.receiving_broadcast = browser_keys::receiving_lower(bv.receiving_broadcast);
            // The hold-off starts when the last replay ends.
            bv.receiving_since = Some(Instant::now());
        }
    }
}

#[cfg(test)]
mod tests {
    fn prod() -> &'static str {
        let src = include_str!("browser_input_app.rs");
        &src[..src.find("#[cfg(test)]").unwrap()]
    }

    /// Keys act as iTerm2's browser does: the gate is a copy (no App borrow), the route is repeated,
    /// each action goes through the existing path, text never reaches a pty.
    #[test]
    fn browser_keys_run_as_iterm2() {
        let p = prod();
        let gate = &p[p.find("fn browser_gate_refresh").unwrap()..p.find("fn browser_gates_sync").unwrap()];
        assert!(gate.contains("move |vk, mods| state.owner(vk, mods).taken()"), "the closure owns copies only");
        let key = &p[p.find("pub(super) fn browser_accel_key(").unwrap()..p.find("fn browser_bar_press").unwrap()];
        assert!(key.find("self.browser_page_key(").unwrap() < key.find("state.owner(").unwrap(), "13x's accelerators first");
        assert!(key.contains("mods & browser_keys::WIN != 0") && key.contains("!taken"), "Win chords and keys the page kept are not ours");
        let bind = &p[p.find("fn browser_binding").unwrap()..p.find("fn browser_do_basic").unwrap()];
        assert!(bind.contains("self.run_key_action(b, None, Some(chord), event_loop)") && bind.contains("self.focused_id() == Some(pane)"), "Session: the existing path");
        assert!(bind.contains("is_browser_profile(p)") && bind.contains("set_profile_on"), "Set Profile only to a browser profile");
        assert!(bind.contains("self.toggle_setting(&json)") && bind.contains("browser_copy_mode_commands"));
        assert!(bind.contains("has-selection.js") && bind.contains("browser_eval_in_frame"), "the selection is asked in Tandem's world");
        let send = &p[p.find("pub(super) fn browser_send_text").unwrap()..p.find("fn browser_insert_raw").unwrap()];
        assert!(send.contains("insert_script(text)") && send.contains("Bool(true)") && send.contains("browser_insert_raw"), "insert-text.js, then DevTools");
        assert!(!p.contains("send_to_focused") && !p.contains("execCommand"), "no pty, no execCommand");
        let paste = &p[p.find("fn browser_paste_text").unwrap()..p.find("pub(super) fn browser_copy_mode_commands").unwrap()];
        assert!(paste.contains("sanitize(&job, &prefs)") && paste.contains("flags &= !crate::paste::BRACKET"));
        let copy = &p[p.find("pub(super) fn browser_copy_selection").unwrap()..p.find("fn browser_read_selection").unwrap()];
        assert!(copy.contains("remember_paste") && copy.contains("set_clipboard_text"));
    }

    /// Only a person's key or binding sends text into a page: nothing from a page message reaches
    /// `browser_send_text`, and the gate is built from Tandem's maps.
    #[test]
    fn text_is_sent_only_for_a_person() {
        for (name, src) in [("browser_scripts_app.rs", include_str!("browser_scripts_app.rs")), ("browser_frames_app.rs", include_str!("browser_frames_app.rs")), ("browser_ctx_app.rs", include_str!("browser_ctx_app.rs"))] {
            let live = &src[..src.find("#[cfg(test)]").unwrap_or(src.len())];
            assert!(!live.contains("browser_send_text") && !live.contains("browser_accel_key("), "{name}: page messages never send text or keys");
        }
        let app = include_str!("browser_app.rs");
        let arm = &app[app.find("Event::Key { vk, mods, taken, .. }").unwrap()..];
        assert!(arm[..160].contains("self.browser_accel_key(pane, *vk, *mods, *taken, event_loop)"));
        let web = include_str!("../webhost_win.rs");
        let h = &web[web.find("AcceleratorKeyPressedEventHandler::create").unwrap()..web.find("fn modifier_bits").unwrap()];
        assert!(h.contains("gate.takes(vk, mods)") && h.contains("SetHandled(true)") && h.contains("taken });"), "the gate runs inside the handler");
        let refresh = include_str!("mod.rs");
        let r = &refresh[refresh.find("fn refresh_route").unwrap()..refresh.find("fn save_pending_settings").unwrap()];
        assert!(r.contains("self.browser_gates_refresh()"), "the gate follows the maps");
    }

    /// Pointer bindings act as iTerm2's browser does (`BWV`, `PAP`, `BVC`): a press reaches here only
    /// from the gate script's Client message (trusted events, the Ready gate, the view's secret), the
    /// release runs the binding, every URL goes through the one funnel, a program only through the
    /// Smart Selection path with its ask.
    #[test]
    fn pointer_runs_as_iterm2() {
        let p = prod();
        let at = |s: &str| p.find(s).unwrap_or_else(|| panic!("missing {s}"));
        let ptr = &p[at("pub(super) fn browser_pointer(")..at("fn browser_pointer_perform")];
        assert!(ptr.contains("pointer::on_down(") && ptr.contains("pointer::on_up("), "the map decides, as the terminal does");
        assert!(ptr.contains("bv.pointer_down.take() != Some((button, clicks))"), "a release without its press runs nothing");
        assert!(ptr.contains("Up::Perform(b)") && ptr.contains("self.globals.bool(\"FocusOnRightOrMiddleClick\")"));
        assert!(ptr.contains("globals.bool(\"CopySelection\")"), "Smart Selection copies only when that preference is on");
        let smart_action = &ptr[ptr.find("Press::SmartAction").unwrap()..ptr.find("Press::Focus").unwrap()];
        assert!(smart_action.contains("false, Box::new") && smart_action.contains("win_click_action(") && !smart_action.contains("browser_copy_selection"), "the Win-click selects the best match (requireAction false in iTerm2's matcher) and copies nothing");
        let perf = &p[at("fn browser_pointer_perform")..at("fn browser_context_menu_at")];
        assert!(perf.contains("PageNext::Session(pane, b.clone())"), "the session's actions run through run_pointer_action");
        assert!(perf.contains("PageOpen::Tab") && perf.contains("PageOpen::Here") && perf.contains("PageOpen::QuickLook(at)"));
        assert!(perf.contains("quick_look_scheme_ok(&url)") && perf.contains("browser_queue_open_gated"));
        assert!(perf.contains("COPIED") && perf.contains("COPIED_SECS") && perf.contains("remember_paste"));
        assert!(perf.contains("extend-selection-to-point.js") && perf.contains("urls-at-point.js"));
        let sel = &p[at("pub(super) fn browser_smart_select")..at("pub(super) fn browser_jump_to_selection")];
        assert!(sel.contains("extract-text-at-point.js") && sel.contains("\"RADIUS\"") && sel.contains("semantic_ui::radius(&self.globals)"));
        assert!(sel.contains("first_match(") && sel.contains("select-text.js") && sel.contains("BEFORE_COUNT") && sel.contains("AFTER_COUNT"));
        assert!(sel.find("extract-text-at-point.js").unwrap() < sel.find("select-text.js").unwrap());
        // The page's answers never open a URL by themselves: only `nav_gate` Navigate is queued, and
        // the tick opens through the funnel.
        let q = &p[at("fn browser_queue_open(")..at("pub(super) fn browser_next_tick")];
        assert!(q.contains("NavGate::Navigate(_) =>") && q.contains("NavGate::Open(_) => {}"), "a pointer click never opens a local file");
        let tick = &p[at("pub(super) fn browser_next_tick")..at("/// Key action 73")];
        assert!(tick.contains("self.browser_page_open(pane, &url, how, event_loop)") && tick.contains("self.browser_smart_action(") && tick.contains("run_pointer_action(&b, pane, (0, 0), event_loop)"));
        for banned in ["Command::new(", "execute_with(", "guard_open(", "shell_open(", "open_url_in_browser(", "navigate_tandem("] {
            assert!(!p.contains(banned), "{banned}");
        }
        // Jump to Selection scrolls the page (`BVC:295-311`); the find menu hands a browser pane to it.
        assert!(p.contains("window.scrollTo({ top: window.scrollY + rect.top - window.innerHeight/2, behavior: 'smooth' })"));
        assert!(include_str!("find_ui.rs").contains("(\"jumpToSelection:\", _) if self.is_browser_pane(id) => self.browser_jump_to_selection(id)"));
        // The router acts on a press only through the one gate of Client handlers.
        let scripts = include_str!("browser_scripts_app.rs");
        let on = &scripts[scripts.find("fn browser_always_on").unwrap()..scripts.find("pub(super) fn browser_page_open").unwrap()];
        assert!(on.contains("!open && !matches!(what, browser_js::AlwaysOn::Console") && on.contains("self.browser_pointer(pane, p)"));
        assert!(!scripts[..scripts.find("#[cfg(test)]").unwrap()].contains("browser_send_text"));
        // The table reaches live worlds when it changes and new worlds when they are made.
        assert!(include_str!("browser_scripts_app.rs").contains("self.browser_pointer_push_new(pane, session, json)"));
        assert!(p.contains("self.browser_pointer_sync(pane)") && p.contains("browser_world_update(pane, &browser_pointer::gate_update("));
    }

    /// Review I1 and m1: the app repeats the gate's order (a bar's button before the address keys, a
    /// person's binding before them), and a key the page kept disarms a pending Leader.
    #[test]
    fn the_app_follows_the_gates_order() {
        let p = prod();
        let key = &p[p.find("pub(super) fn browser_accel_key(").unwrap()..p.find("fn browser_bar_press").unwrap()];
        assert!(key.find("Owner::Bar(i)").unwrap() < key.find("self.browser_page_key(").unwrap(), "the bar first");
        assert!(key.contains("Owner::Binding"), "a binding beats the address keys");
        assert!(key.contains("self.leader_on") && key.contains("!taken"), "a kept key disarms the Leader");
        // Round 2 minor: a key `browser_page_key` consumes (F6, Ctrl+L) disarms it too, so the disarm is first.
        assert!(key.find("self.leader_on").unwrap() < key.find("self.browser_page_key(").unwrap(), "the Leader ends on any key the page kept");
    }

    /// Broadcast (Task 8): a message is acted on only when the pane sends now, the replay never
    /// focuses a view, only the person's own events (the script's) start it, and a target's focused
    /// secret field gets nothing.
    #[test]
    fn broadcast_reaches_other_browsers_only_natively() {
        let p = prod();
        let b = &p[p.find("// --- Broadcast between browser panes").unwrap()..];
        for banned in [".focus(", "focus_window", "set_visible", "set_focus", "Emulation.", "execute_script"] {
            assert!(!b.contains(banned), "replaying never moves focus: {banned}");
        }
        assert!(b.contains("broadcast_view_in(true)") && b.contains("self.broadcast.sends(&view, sender)") && b.contains("self.broadcast.targets(&view, sender)"));
        assert!(b.contains("browser_keys::guard_body()") && b.contains("Ok(serde_json::Value::Bool(false))"), "only a plain false lets a replay go");
        assert!(b.contains("receiving_raise") && b.contains("receiving_lower"));
        assert!(b.contains("echo.note(") && b.contains("browser_keys::broadcast_verdict(&mut bv.echo, e, Instant::now(), sends, receiving, since)"), "no ordering is relied on: hold-off and echo guard (review I1)");
        let scripts = include_str!("browser_scripts_app.rs");
        let router = &scripts[scripts.find("fn browser_always_on").unwrap()..scripts.find("pub(super) fn browser_page_open").unwrap()];
        assert!(router.contains("if self.browser_broadcast_accepts(pane, &e)"), "the router drops a broadcast message unless the pane sends");
        assert!(router.contains("if !open &&"), "and only the proven isolated world speaks");
        let js = include_str!("../browser_js.rs");
        assert!(js.contains("BROADCAST_HANDLER if secret_ok(&r.body, secret) && r.frame.is_none()"), "the view's secret, main frame");
    }

    /// Final review minor 5 and deferred minor 221: the echo is consumed before the hold-off verdict,
    /// a noted input that was never made is forgotten, and a queued replay stops when its target
    /// left the sender's broadcast set (or broadcast was turned off).
    #[test]
    fn replays_forget_what_they_never_typed() {
        let p = prod();
        let accepts = &p[p.find("pub(super) fn browser_broadcast_accepts").unwrap()..p.find("pub(super) fn browser_broadcast_targets").unwrap()];
        assert!(accepts.contains("browser_keys::broadcast_verdict(&mut bv.echo, e,") && !accepts.contains("browser_keys::broadcast_accepts("), "one verdict, echo first");
        let replay = &p[p.find("fn browser_replay(").unwrap()..p.find("fn browser_replay_done").unwrap()];
        assert!(replay.matches("a.browser_replay_forget(").count() >= 2, "a refused guard and a failed step forget the note");
        let step = &p[p.find("fn browser_replay_step").unwrap()..p.find("fn browser_replay_done").unwrap()];
        assert!(step.contains("self.browser_broadcast_targets(sender).contains(&pane)"), "membership is checked again before every call (221)");
    }
}

//! Keys in a browser pane (milestone 13x2, Task 6). A key reaches here only from the person: the
//! 13u keyboard hook and WebView2's `AcceleratorKeyPressed` (through `Event::Key`), never from a
//! page script event. The key gate (`browser_keys::GateState`, read inside the COM handler) decides
//! synchronously whether Tandem takes the key before the page sees it; this side repeats the route
//! and runs what iTerm2's browser would (`BAP`): scroll, send text into the focused field, extend
//! the selection, copy or paste, or hand the binding to the session. Bound text goes through
//! `Input.insertText` or iTerm2's insert-text.js, never to a pty (a browser pane has none).

use super::*;
use crate::browser_keys::{self, Do, Facts, GateState, KeyAct, Owner};
use crate::keyroute::Route;
use crate::webhost;

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
        // Win chords and the address field's accelerators stay first (13x).
        if self.browser_page_key(pane, vk, mods, event_loop) || !taken || mods & browser_keys::WIN != 0 {
            return;
        }
        let (Some(state), Some(chord)) = (self.browser_gate_state(pane), browser_keys::chord_of(vk, mods)) else { return };
        match state.owner(vk, mods) {
            Owner::Bar(i) => self.browser_bar_press(pane, i, event_loop),
            Owner::Page | Owner::Address => {}
            Owner::Binding | Owner::Number | Owner::Leader => {
                let route = state.route(&chord);
                self.in_pane_window(pane, |a| a.browser_run_route(pane, route, &chord, event_loop));
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
    fn browser_run_route(&mut self, pane: PaneId, route: Route, chord: &crate::menus::Chord, event_loop: &ActiveEventLoop) {
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
            Route::Binding(b) => self.browser_binding(pane, &b, chord, event_loop),
            Route::Terminal => {}
        }
        if was_leader {
            self.refresh_route();
        }
    }

    /// One binding in a browser pane (`iTermBrowserKeyBindingActionPerformer.perform`).
    fn browser_binding(&mut self, pane: PaneId, b: &crate::keyaction::Binding, chord: &crate::menus::Chord, event_loop: &ActiveEventLoop) {
        let act = browser_keys::classify(b);
        // Set Profile only to a browser profile; anything else does nothing (`BAP:42-48`).
        if let KeyAct::SetProfile(guid) = &act {
            if self.store.get(guid).is_some_and(|p| self.is_browser_profile(p)) {
                self.set_profile_on(pane, guid);
            }
            return;
        }
        let own = browser_keys::own_char(chord).unwrap_or_default();
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

    /// Key action 73 (copy mode): 13x3 Task 8 replaces this body.
    pub(super) fn browser_copy_mode_commands(&mut self, _pane: PaneId, _keys: &str) {}
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
}

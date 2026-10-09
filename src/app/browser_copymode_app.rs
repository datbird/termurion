//! Copy mode in a browser pane (milestone 13x3, Task 8). iTerm2's page cursor (`copy-mode.js`) runs
//! in Tandem's isolated world; this file turns the shared copy-mode keys into calls on it. Keys
//! arrive two ways and only two: the key gate (`Owner::CopyMode`, a key the page never saw) and the
//! world's own trusted-key listener (`browser_copymode::KEY_HANDLER`). A page's message, a page's
//! synthetic event or a view without a proven isolated world drives nothing.

use super::*;
use crate::browser_copymode::{self as cm, Model, Step};
use crate::browser_js::{Routed, World};
use serde_json::json;

impl App {
    /// Whether copy mode is on in browser pane `pane`.
    pub(super) fn is_browser_copy_mode(&self, pane: PaneId) -> bool {
        self.browser_views.get(&pane).is_some_and(|b| b.copy_mode.is_some())
    }

    /// Whether Edit > Copy Mode is offered on the focused pane: always on a terminal, on a browser
    /// only with a proven isolated world (the page-world fallback is gone, R13x23).
    pub(super) fn copy_mode_offered(&self) -> bool {
        match self.focused_id() {
            Some(id) if self.is_browser_pane(id) => self.client_open(id),
            _ => true,
        }
    }

    /// Edit > Copy Mode on a browser pane (`PS:1569-1583`).
    pub(super) fn browser_copy_mode_toggle(&mut self, pane: PaneId) {
        let on = !self.is_browser_copy_mode(pane);
        self.browser_copy_mode_set(pane, on);
    }

    /// Turns copy mode on or off (`BCH:25-46`): the page's cursor, the keys switch of every live
    /// Tandem world, the key gate, the view's focus (`BCH:101-110`) and the menu check.
    pub(super) fn browser_copy_mode_set(&mut self, pane: PaneId, on: bool) {
        let Some(secret) = self.browser_view_secret(pane).map(str::to_string) else { return };
        if on {
            if !self.client_open(pane) {
                return;
            }
            let Some(bv) = self.browser_views.get_mut(&pane) else { return };
            if bv.copy_mode.is_some() {
                return;
            }
            bv.copy_mode = Some(Model::default());
            self.belt_yield_keys();
            self.browser_js_call(pane, &cm::enable_js(&secret), json!({}), Box::new(|_, _, _| {}));
            self.browser_world_update(pane, &cm::keys_on_js(&secret, true));
            self.browser_page_took_keyboard(pane);
            if let Some(v) = self.browser_views.get(&pane).and_then(|b| b.view.as_ref()) {
                v.focus();
            }
        } else {
            let Some(bv) = self.browser_views.get_mut(&pane) else { return };
            if bv.copy_mode.take().is_none() {
                return;
            }
            self.browser_js_call(pane, &cm::disable_js(&secret), json!({}), Box::new(|_, _, _| {}));
            self.browser_world_update(pane, &cm::keys_on_js(&secret, false));
            if self.browser_page_may_focus(pane) {
                if let Some(v) = self.browser_views.get(&pane).and_then(|b| b.view.as_ref()) {
                    v.focus();
                }
            }
        }
        self.browser_gate_refresh(pane);
        self.sync_checks();
        self.redraw();
    }

    /// A Tandem world made while copy mode is on (an iframe that loaded mid-session) captures keys
    /// too: the switch is turned on in that one context (`unique id` from the DevTools event, as the
    /// pointer table is pushed).
    pub(super) fn browser_copy_mode_new_context(&mut self, pane: PaneId, session: Option<&str>, json: &str) {
        if !self.is_browser_copy_mode(pane) || !self.client_open(pane) {
            return;
        }
        let (Some(secret), Some(unique)) = (self.browser_view_secret(pane).map(str::to_string), super::browser_input_app::isolated_unique(json)) else { return };
        let params = crate::browser_world::evaluate_params(&unique, &cm::keys_on_js(&secret, true));
        self.devtools_send(pane, session, "Runtime.evaluate", &params, Box::new(|_, _, _| {}));
    }

    /// One `KEY_HANDLER` message: only the isolated world's, only with a proven world, only while copy
    /// mode is on; a frame's is the main frame's (focus inside an iframe). Dropped before it is read.
    pub(super) fn browser_copy_mode_key(&mut self, pane: PaneId, r: &Routed) {
        if r.world != World::Client || !self.client_open(pane) || !self.is_browser_copy_mode(pane) {
            return;
        }
        if let Some(name) = cm::key_of(&r.body).and_then(|(key, ctrl, alt)| cm::key_name(&key, ctrl, alt)) {
            self.browser_copy_mode_apply(pane, &name);
        }
    }

    /// A key the gate took for copy mode (`Owner::CopyMode`): the virtual key and modifiers become
    /// the same key names.
    pub(super) fn browser_copy_mode_vk(&mut self, pane: PaneId, vk: u32, mods: u8) {
        if !self.client_open(pane) || !self.is_browser_copy_mode(pane) {
            return;
        }
        if let Some(name) = cm::key_name_vk(vk, mods) {
            self.browser_copy_mode_apply(pane, &name);
        }
    }

    /// Key action 73 (`BCH:75-88`): the keys run with copy mode on, and it is restored to off
    /// afterwards. A parse error shows the terminal path's banner.
    pub(super) fn browser_copy_mode_commands(&mut self, pane: PaneId, keys: &str) {
        let keys = match crate::copymode::parse_vim_keys(keys) {
            Ok(k) => k,
            Err(e) => return self.banners.set("action", e),
        };
        let was_on = self.is_browser_copy_mode(pane);
        if !was_on {
            self.browser_copy_mode_set(pane, true);
        }
        for key in &keys {
            if !self.is_browser_copy_mode(pane) {
                break;
            }
            self.browser_copy_mode_apply(pane, key);
        }
        if !was_on {
            self.browser_copy_mode_set(pane, false);
        }
        self.redraw();
    }

    /// One named key through the model, then its step (`BCH:92-99`, `CPH:393-484`).
    fn browser_copy_mode_apply(&mut self, pane: PaneId, name: &str) {
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        let secret = bv.secret.clone();
        let Some(model) = bv.copy_mode.as_mut() else { return };
        match model.key(name, &secret) {
            Step::Nothing => {}
            Step::Script(js) => self.browser_js_call(pane, &js, json!({}), Box::new(|_, _, _| {})),
            // The one selection reader and clipboard path (1 MiB cap), then copy mode ends.
            Step::Copy => {
                self.browser_copy_selection(pane);
                self.browser_copy_mode_set(pane, false);
            }
            Step::Leave { stop_selecting } => {
                if stop_selecting {
                    self.browser_js_call(pane, cm::STOP_SELECTING_JS, json!({}), Box::new(|_, _, _| {}));
                }
                self.browser_copy_mode_set(pane, false);
            }
            Step::Find => {
                self.browser_find_command(pane, "performFindPanelAction:", "Find...");
            }
        }
        self.redraw();
    }
}

#[cfg(test)]
mod tests {
    fn prod(src: &str) -> &str {
        src.split("\n#[cfg(test)]").next().unwrap_or(src)
    }

    fn between<'a>(src: &'a str, from: &str, to: &str) -> &'a str {
        let a = src.find(from).unwrap_or_else(|| panic!("missing {from}"));
        let b = a + src[a..].find(to).unwrap_or_else(|| panic!("missing {to}"));
        &src[a..b]
    }

    /// Toggling: the page's cursor, the key switch of every live world, the gate, the focus and the
    /// menu check change together; without a proven isolated world nothing turns on.
    #[test]
    fn browser_copy_mode_toggles() {
        let me = prod(include_str!("browser_copymode_app.rs"));
        let set = between(me, "pub(super) fn browser_copy_mode_set", "pub(super) fn browser_copy_mode_new_context");
        assert!(set.contains("if !self.client_open(pane)") && set.find("if !self.client_open(pane)").unwrap() < set.find("Model::default()").unwrap(), "no isolated world, no copy mode");
        assert!(set.contains("cm::enable_js(&secret)") && set.contains("cm::disable_js(&secret)"));
        assert!(set.contains("self.browser_world_update(pane, &cm::keys_on_js(&secret, true))") && set.contains("self.browser_world_update(pane, &cm::keys_on_js(&secret, false))"));
        assert!(set.contains("v.focus()") && set.contains("self.browser_gate_refresh(pane)") && set.contains("self.sync_checks()") && set.contains("self.belt_yield_keys()"));
        assert!(!set.contains("execute_script") && !set.contains("browser_eval("), "only the Tandem world");
        // Every script the steps run goes through the world call, never the page.
        let apply = between(me, "fn browser_copy_mode_apply", "\n}\n");
        assert!(apply.contains("self.browser_js_call(pane, &js") && apply.contains("self.browser_copy_selection(pane)"), "Copy is the one selection reader and clipboard path");
        assert!(!apply.contains("arboard") && !apply.contains("clipboard =") && !apply.contains("SELECTION_JS") && !me.contains("format!("), "no second reader, no composed script here");
        assert!(apply.contains("Step::Leave { stop_selecting }") && apply.contains("STOP_SELECTING_JS") && apply.contains("self.browser_find_command(pane, \"performFindPanelAction:\", \"Find...\")"));
        // A new world mid-session gets the switch; the unique id, never the numeric one.
        let ctx = between(me, "pub(super) fn browser_copy_mode_new_context", "/// One `KEY_HANDLER` message");
        assert!(ctx.contains("isolated_unique(json)") && ctx.contains("self.client_open(pane)") && ctx.contains("\"Runtime.evaluate\""));
        // The menu: toggling, the check and the enable.
        let cm = prod(include_str!("copymode_ui.rs"));
        let run = between(cm, "pub(super) fn run_copymode_command", "pub(super) fn leave_copy_mode");
        assert!(run.contains("self.is_browser_pane(id)") && run.contains("self.browser_copy_mode_toggle(id)"));
        let cmds = between(cm, "pub(super) fn copy_mode_commands_in", "fn copy_mode_keys");
        assert!(cmds.contains("self.is_browser_pane(id)") && cmds.contains("self.browser_copy_mode_commands(id, keys)"));
        assert!(prod(include_str!("seltools_ui.rs")).contains("|| self.is_browser_copy_mode(id)"));
        assert!(prod(include_str!("menu.rs")).contains("c.set_enabled(self.copy_mode_offered())"));
        // key action 73 parses with the shared parser, runs with copy mode on and restores it.
        let cmds = between(me, "pub(super) fn browser_copy_mode_commands", "/// One named key");
        assert!(cmds.contains("parse_vim_keys(keys)") && cmds.contains("self.banners.set(\"action\", e)"));
        assert!(cmds.contains("let was_on") && cmds.contains("if !was_on {") && cmds.matches("self.browser_copy_mode_set(pane,").count() == 2);
    }

    /// A page cannot drive copy mode: only the isolated world's messages count, only with a proven
    /// world, only while it is on; the model reads typed fields, never the page's text as a script.
    #[test]
    fn page_cannot_hijack_copy_mode_wiring() {
        let me = prod(include_str!("browser_copymode_app.rs"));
        let key = between(me, "pub(super) fn browser_copy_mode_key", "/// A key the gate took");
        assert!(key.contains("r.world != World::Client") && key.contains("!self.client_open(pane)") && key.contains("!self.is_browser_copy_mode(pane)"));
        assert!(key.find("World::Client").unwrap() < key.find("key_of(").unwrap(), "a page-world message is dropped before it is read");
        let vk = between(me, "pub(super) fn browser_copy_mode_vk", "/// Key action 73");
        assert!(vk.contains("!self.client_open(pane)") && vk.contains("!self.is_browser_copy_mode(pane)"));
        // The router has the one arm; navigation starts and failures turn it off; a new secret resets it.
        let router = prod(include_str!("browser_scripts_app.rs"));
        assert!(router.contains("h if h == crate::browser_copymode::KEY_HANDLER => self.browser_copy_mode_key(pane, &r)"));
        let reset = between(router, "pub(super) fn browser_reset_world", "// --- Setup ---");
        assert!(reset.contains("bv.copy_mode = None"));
        let start = between(router, "Event::Starting { redirect: false, .. } =>", "Event::ScriptDone");
        assert!(start.contains("self.browser_copy_mode_set(pane, false)"));
        assert!(prod(include_str!("browser_app.rs")).contains("self.browser_copy_mode_set(pane, false)"));
        // Both scripts are registered only inside the isolated-world block.
        let scripts = between(router, "pub(super) fn browser_user_scripts", "pub(super) fn client_open");
        let at = scripts.find("browser_copymode::user_script(secret)").unwrap();
        assert!(at > scripts.find("if client_ok {").unwrap(), "no isolated world, no copy mode scripts");
        assert!(scripts.contains("browser_copymode::keys_script(secret)"));
        // The new world is told while copy mode is on.
        assert!(router.contains("self.browser_copy_mode_new_context(pane, session, json)"));
    }

    /// The gate takes copy mode's keys; the order test lives in `browser_keys`, this pins the wiring.
    #[test]
    fn the_gate_carries_copy_mode() {
        let input = prod(include_str!("browser_input_app.rs"));
        assert!(input.contains("copy_mode: bv.copy_mode.is_some()"));
        let key = between(input, "pub(super) fn browser_accel_key(", "fn browser_bar_press");
        assert!(key.contains("Owner::CopyMode => self.browser_copy_mode_vk(pane, vk, mods)"));
        let sync = between(input, "pub(super) fn browser_gates_sync", "/// A key the page raised");
        assert!(sync.contains("bv.copy_mode.is_some()"), "turning copy mode on or off rebuilds the gate");
    }
}

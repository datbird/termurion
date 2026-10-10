//! Browser triggers in a pane (milestone 13x5, Task 9): the document-start engine script, its messages,
//! the effects, and resending the list when the profile's browser triggers change. The decisions are
//! `browser_triggers`'s (pure); this applies them. The engine, highlight and hyperlink calls run in the
//! isolated world (`browser_js_call`); only Inject Javascript and Reload After Delay run in the page
//! world (`browser_page_call`), with no bridge and no secret (iTerm2's `BTH:134-138`).

use super::browser_scripts_app::log_js_failure;
use super::*;
use crate::browser_js::{self, Inject, Routed, UserScript, World};
use crate::browser_triggers::{self as bt, Effect, Message};
use crate::triggers::{self, Trigger};
use serde_json::json;

impl App {
    /// A pane's browser-action triggers as its profile stores them (the terminal list drops them).
    fn browser_trigger_list(&self, pane: PaneId) -> Vec<Trigger> {
        self.profile_of(pane).and_then(|p| p.get("Triggers")).map(|v| triggers::parse_list(&v)).unwrap_or_default()
    }

    /// Makes the pane's `Loaded` from its profile with a fresh secret (iTerm2's own trigger secret,
    /// `BTH:24-27`). Runs in `start_browser_pane`, before `browser_user_scripts` reads it.
    pub(super) fn browser_triggers_init(&mut self, pane: PaneId) {
        let list = self.browser_trigger_list(pane);
        let loaded = bt::load(&list, browser_js::new_secret());
        let held = bt::held_back(&loaded);
        if let Some(bv) = self.browser_views.get_mut(&pane) {
            bv.triggers = Some(loaded);
        }
        self.browser_triggers_tell(pane, held);
    }

    fn browser_triggers_tell(&mut self, pane: PaneId, held: usize) {
        if held > 0 {
            self.tell_once(pane, "browser-trigger-pattern", "A browser trigger's pattern can take very long to match, so it is not given to pages. Rewrite it without a repeated group that repeats inside.");
        }
    }

    /// The trigger script of a browser view (`BM:350-358`): always added, so an empty list costs one
    /// message per page. None without a `Loaded`, or when the script cannot be assembled (a patch that
    /// no longer applies: no engine at all).
    pub(super) fn browser_trigger_script(&self, pane: PaneId) -> Option<UserScript> {
        let l = self.browser_views.get(&pane)?.triggers.as_ref()?;
        match bt::user_script(&l.secret) {
            Ok(code) => Some(UserScript { id: bt::SCRIPT_ID, code, inject: Inject::DocumentStart, main_frame_only: true, world: World::Client }),
            Err(_) => {
                crate::rlog!("browser trigger script not registered");
                None
            }
        }
    }

    /// After a profile change (`refresh_panes`): a pane whose browser triggers changed gets a new list
    /// under the same secret (the document-start script holds it) and one `setTriggers`; any other
    /// profile edit sends nothing and keeps the identifiers, so in-flight matches still resolve
    /// (`BTH:34-36`, `:153-156`).
    pub(super) fn browser_triggers_refresh(&mut self) {
        let panes: Vec<PaneId> = self.browser_views.iter().filter(|(_, b)| b.triggers.is_some()).map(|(p, _)| *p).collect();
        for pane in panes {
            let list = self.browser_trigger_list(pane);
            let Some(old) = self.browser_views.get(&pane).and_then(|b| b.triggers.as_ref()) else { continue };
            if !bt::needs_reload(old, &list) {
                continue;
            }
            let loaded = bt::load(&list, old.secret.clone());
            let held = bt::held_back(&loaded);
            let epoch = self.browser_views.get(&pane).map_or(0, |b| b.marks_method.epoch());
            let Some(bv) = self.browser_views.get_mut(&pane) else { continue };
            let served = bv.trigger_gate.is_served();
            let call = bt::set_triggers_call(&loaded);
            bv.triggers = Some(loaded);
            self.browser_triggers_tell(pane, held);
            // Only a page that has asked has the engine; the next page asks for the new list itself.
            if served && self.client_open(pane) {
                if let Some(bv) = self.browser_views.get_mut(&pane) {
                    bv.trigger_gate.serve(epoch);
                }
                self.browser_js_call(pane, &call.body, call.args, Box::new(|_, _, r| {
                    if let Err(e) = r {
                        log_js_failure(&e);
                    }
                }));
            }
        }
    }

    /// One `iTerm2Trigger` message from a page. Heard only from the isolated world's main frame, from a
    /// proven world, never for one of Tandem's own pages; the secret is checked by `message_of`.
    pub(super) fn browser_trigger_message(&mut self, pane: PaneId, r: &Routed) {
        if !self.client_open(pane) {
            return;
        }
        let url = self.panes.get(&pane).and_then(|p| p.browser_io()).map(|io| io.url.clone()).unwrap_or_default();
        if !bt::accepted(r.world, r.frame, &r.source, &url) {
            return;
        }
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        let epoch = bv.marks_method.epoch();
        let Some(loaded) = bv.triggers.clone() else { return };
        match bt::message_of(&r.body, &loaded.secret) {
            Message::Bogus => {}
            Message::Request => {
                bv.trigger_gate.serve(epoch);
                let call = bt::set_triggers_call(&loaded);
                self.browser_js_call(pane, &call.body, call.args, Box::new(|_, _, r| {
                    if let Err(e) = r {
                        log_js_failure(&e);
                    }
                }));
            }
            Message::Matches(matches) => {
                // A match belongs to the page that asked: a navigation since makes it another page's.
                if !bv.trigger_gate.current(epoch) {
                    return;
                }
                let interpolated = self.profile_of(pane).is_some_and(|p| p.bool("Triggers Use Interpolated Strings"));
                for m in matches {
                    let Some((id, t)) = loaded.triggers.iter().find(|(i, _)| *i == m.identifier) else { continue };
                    // Only a trigger the person has on runs (imports of the risky actions stay off).
                    if !t.enabled {
                        continue;
                    }
                    let param = if bt::resolves_parameter(&t.action) {
                        let resolved = {
                            let screen = self.panes.get(&pane).map(|p| p.own());
                            let var = |n: &str| screen.and_then(|s| s.trigger_var(n, -1));
                            bt::resolve(&t.parameter, &bt::captures(&m), interpolated, &var)
                        };
                        match resolved {
                            Ok(v) => v,
                            Err(e) => {
                                self.tell_once(pane, &format!("parameter-{id}"), &format!("Error in Trigger Parameter: {e}"));
                                continue;
                            }
                        }
                    } else {
                        t.parameter.clone()
                    };
                    match bt::effect(t, &m, &param, &[]) {
                        Some(Effect::Stop) => break,
                        Some(e) => self.browser_trigger_effect(pane, e),
                        None => {}
                    }
                }
            }
        }
    }

    /// Runs one effect, within the pane's rate (B8). Highlight and Hyperlink call the engine in the
    /// isolated world with typed arguments; Inject (and Reload, which is Inject of its fixed script) runs
    /// in the page world. Reader Mode and the workgroup actions are Task 10's: until then they show the
    /// "not available yet" banner and do nothing.
    pub(super) fn browser_trigger_effect(&mut self, pane: PaneId, e: Effect) {
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        let now = Instant::now();
        if !bv.trigger_gate.effect_allowed(now) {
            return;
        }
        let done: super::browser_scripts_app::JsDone = Box::new(|_, _, r| {
            if let Err(e) = r {
                log_js_failure(&e);
            }
        });
        match e {
            Effect::Highlight { match_id, text, background } => {
                let c = bt::highlight_call(&match_id, text.as_deref(), background.as_deref());
                self.browser_js_call(pane, &c.body, c.args, done);
            }
            Effect::Hyperlink { match_id, url } => {
                let c = bt::hyperlink_call(&match_id, &url);
                self.browser_js_call(pane, &c.body, c.args, done);
            }
            Effect::Inject(script) => {
                if script.starts_with("function reloadAfter(") && !bv.trigger_gate.reload_allowed(now) {
                    return;
                }
                let body = format!("{}\nreturn null;", browser_js::iife(&script));
                self.browser_page_call(pane, &body, json!({}), done);
            }
            Effect::EnterReaderMode | Effect::EnterWorkgroup(_) | Effect::ExitWorkgroup => {
                self.tell_once(pane, "browser-trigger-later", "That trigger action is not available in Tandem yet.");
            }
            Effect::Stop => {}
        }
    }
}

#[cfg(test)]
mod tests {
    fn prod(src: &str) -> &str {
        &src[..src.find("#[cfg(test)]").unwrap_or(src.len())]
    }

    fn between<'a>(s: &'a str, from: &str, to: &str) -> &'a str {
        let a = s.find(from).unwrap_or_else(|| panic!("{from}"));
        &s[a..a + s[a..].find(to).unwrap_or_else(|| panic!("{to}"))]
    }

    /// Security (Review Focus 11, R13x45-1; B1 cannot build an `App`, so the shape is pinned): the
    /// trigger script is a Client document-start main-frame script holding only the trigger secret;
    /// every engine call is the isolated-world runner's; only Inject runs in the page world, with no
    /// bridge and no secret; the view secret and the marks secret appear nowhere here.
    #[test]
    fn trigger_scripts_use_the_right_world() {
        let me = prod(include_str!("browser_triggers_app.rs"));
        let script = between(me, "pub(super) fn browser_trigger_script", "\n    }\n");
        assert!(script.contains("bt::user_script(&l.secret)") && script.contains("world: World::Client") && script.contains("inject: Inject::DocumentStart") && script.contains("main_frame_only: true") && script.contains("id: bt::SCRIPT_ID"));
        assert!(!script.contains("bv.secret") && !script.contains("marks_page"));
        for bad in ["browser_eval", "execute_script", "chrome.webview", "dom_secret", "marks_page", "bv.secret", "browser_view_secret"] {
            assert!(!me.contains(bad), "{bad}");
        }
        let effect = between(me, "pub(super) fn browser_trigger_effect", "\n    }\n");
        let inject = between(effect, "Effect::Inject(script) =>", "Effect::EnterReaderMode");
        assert!(inject.contains("browser_page_call(") && !inject.contains("browser_js_call") && !inject.contains("secret"));
        for arm in ["Effect::Highlight", "Effect::Hyperlink"] {
            let a = between(effect, arm, "}\n            Effect::");
            assert!(a.contains("browser_js_call(") && !a.contains("browser_page_call"), "{arm}");
        }
        let msg = between(me, "pub(super) fn browser_trigger_message", "\n    }\n");
        assert!(msg.contains("self.client_open(pane)") && msg.contains("bt::accepted(r.world, r.frame, &r.source, &url)"), "the one gate, the world, the frame and the local-page rule");
        assert!(msg.find("bt::accepted").unwrap() < msg.find("message_of").unwrap());
        assert!(msg.contains("bv.trigger_gate.current(epoch)") && msg.contains("if !t.enabled"), "bound to the page that asked; off triggers never run");
        let request = between(msg, "Message::Request =>", "Message::Matches");
        assert!(request.contains("set_triggers_call(&loaded)") && request.contains("browser_js_call("));
        // The router and the registration.
        let router = prod(include_str!("browser_scripts_app.rs"));
        assert!(router.contains("crate::browser_triggers::MESSAGE_HANDLER => self.browser_trigger_message(pane, &r),"));
        let scripts = between(router, "pub(super) fn browser_user_scripts", "\n    }\n");
        assert!(scripts.contains("self.browser_trigger_script(pane)"));
        assert!(include_str!("browser_scripts_app.rs").contains("\"browser_reader_message\", \"browser_trigger_message\"]"), "listed in the router test");
        // Rotated and reset with the world; started with the pane.
        let sc = include_str!("browser_scripts_app.rs");
        assert_eq!(sc.matches("l.secret = browser_js::new_secret()").count(), 2);
        assert_eq!(sc.matches("bv.trigger_gate.reset()").count(), 2);
        assert!(prod(include_str!("browser_app.rs")).contains("self.browser_triggers_init(id);"));
        assert!(prod(include_str!("mod.rs")).contains("self.browser_triggers_refresh();"));
    }

    /// The effects the engine asks for are bounded and paced, and Make Hyperlink and the reload delay
    /// are judged before anything runs (the pure functions are tested in `browser_triggers`).
    #[test]
    fn effects_are_paced_and_judged() {
        let me = prod(include_str!("browser_triggers_app.rs"));
        let effect = between(me, "pub(super) fn browser_trigger_effect", "\n    }\n");
        assert!(effect.contains("trigger_gate.effect_allowed(now)") && effect.contains("reload_allowed(now)"));
        assert!(effect.find("effect_allowed").unwrap() < effect.find("browser_js_call").unwrap());
        let pure = prod(include_str!("../browser_triggers.rs"));
        assert!(pure.contains("triggers::hyperlink_url_ok(param)?") && pure.contains("delay_ok(param)?"));
    }

    /// The resend follows the profile's browser triggers only, and keeps the secret.
    #[test]
    fn the_list_is_resent_only_for_a_change() {
        let me = prod(include_str!("browser_triggers_app.rs"));
        let refresh = between(me, "pub(super) fn browser_triggers_refresh", "\n    }\n");
        assert!(refresh.contains("bt::needs_reload(old, &list)") && refresh.contains("bt::load(&list, old.secret.clone())"));
        assert!(refresh.find("needs_reload").unwrap() < refresh.find("browser_js_call").unwrap());
    }
}

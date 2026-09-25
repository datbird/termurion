//! The OSC 1337 keys that ask first, in the window: SetProfile, SetProfileProperty, and
//! SetBackgroundImageFile, with iTerm2's bars and stored answers.

use base64::Engine;

use super::*;
use crate::prompted::{self, Decision};

pub(super) const ASK_PROFILE: &str = "AskAboutChangingProfile";
pub(super) const ASK_PROPERTY: &str = "SetProfileProperty";
pub(super) const ASK_BACKGROUND: &str = "SetBackgroundImageFile";

fn b64_text(s: &str) -> Option<String> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(s.trim()).ok()?;
    String::from_utf8(bytes).ok()
}

impl App {
    fn global_list(&self, key: &str) -> Vec<String> {
        self.globals.get(key).and_then(|v| v.as_array().cloned()).unwrap_or_default().iter().filter_map(|v| v.as_str().map(str::to_string)).collect()
    }

    fn push_global_list(&mut self, key: &str, item: &str) {
        let mut list = self.global_list(key);
        if !list.iter().any(|x| x == item) {
            list.push(item.to_string());
        }
        self.globals.set(key, toml::Value::Array(list.into_iter().map(toml::Value::String).collect()));
        self.save_globals();
    }

    /// SetProfile: the named saved profile (exact; empty is the default) replaces the pane's.
    pub(super) fn set_profile_request(&mut self, pane: PaneId, name: String) {
        let prevent = self.globals.values.get("PreventEscapeSequenceFromChangingProfile").and_then(|v| v.as_bool());
        match prompted::set_profile_decision(prevent) {
            Decision::Deny => {}
            Decision::Ask => self.ask(
                pane,
                ASK_PROFILE,
                "A control sequence attempted to change the current profile. Allow this in the future?",
                &["Always _Allow", "Always _Deny"],
                false,
            ),
            Decision::Apply => {
                let guid = if name.is_empty() {
                    Some(self.store.default_guid.clone())
                } else {
                    self.store.profiles.iter().find(|p| p.name() == name).map(|p| p.guid())
                };
                if let Some(guid) = guid {
                    self.remarry(pane, &guid);
                }
            }
        }
    }

    /// SetProfileProperty: every pair must be legal, or none applies.
    pub(super) fn set_property_request(&mut self, pane: PaneId, pairs: Vec<(String, String)>) {
        let decoded: Vec<(String, serde_json::Value)> =
            pairs.into_iter().filter_map(|(k, v)| Some((k, serde_json::from_str(&b64_text(&v)?).ok()?))).collect();
        if decoded.is_empty() {
            return;
        }
        let Some(values): Option<Vec<(String, toml::Value)>> = decoded.iter().map(|(k, j)| Some((k.clone(), prompted::valid(k, j)?))).collect() else { return };
        let keys: Vec<String> = values.iter().map(|(k, _)| k.clone()).collect();
        let stored = |k: &str| self.globals.values.get(&format!("NoSyncSetProfileProperty_{k}")).and_then(|v| v.as_integer());
        match prompted::property_decision(&keys, stored) {
            Decision::Deny => {}
            Decision::Apply => self.apply_properties(pane, values),
            Decision::Ask => {
                self.pending_properties.insert(pane, values);
                self.ask(pane, ASK_PROPERTY, &prompted::property_text(&keys), &["_Allow Once", "Allow Always", "Deny Always"], false);
            }
        }
    }

    fn apply_properties(&mut self, pane: PaneId, values: Vec<(String, toml::Value)>) {
        let values: Vec<(String, Option<toml::Value>)> = values.into_iter().map(|(k, v)| (k, Some(v))).collect();
        self.set_session_values(pane, &values);
    }

    /// SetBackgroundImageFile: a path (empty removes the image) as the pane's own setting.
    pub(super) fn set_background_request(&mut self, pane: PaneId, value: String) {
        let Some(path) = b64_text(&value).map(|p| p.trim_end().to_string()) else { return };
        if !path.is_empty() && !std::path::Path::new(&path).exists() {
            return;
        }
        match prompted::background_decision(&path, &self.global_list("AlwaysAllowBackgroundImage"), &self.global_list("AlwaysDenyBackgroundImage")) {
            Decision::Deny => {}
            Decision::Apply => self.apply_background(pane, &path),
            Decision::Ask => {
                self.pending_background.insert(pane, path.clone());
                self.ask(pane, ASK_BACKGROUND, &prompted::background_text(&path), &["Yes", "Always", "Never"], false);
            }
        }
    }

    fn apply_background(&mut self, pane: PaneId, path: &str) {
        // Folder rotation ignores a single image, as iTerm2 does.
        if self.profile_of(pane).is_some_and(|p| p.int("Background Image Source Mode") == 1) {
            return;
        }
        let value = (!path.is_empty()).then(|| toml::Value::String(path.to_string()));
        self.set_session_values(pane, &[("Background Image Location".to_string(), value)]);
    }

    /// The answers to these bars. True when the bar was one of them.
    pub(super) fn prompted_bar(&mut self, pane: PaneId, id: &str, button: Option<&str>) -> bool {
        match (id, button) {
            (ASK_PROFILE, Some(b)) => {
                self.globals.set("PreventEscapeSequenceFromChangingProfile", toml::Value::Boolean(b.contains("Deny")));
                self.save_globals();
            }
            (ASK_PROPERTY, answer) => {
                let Some(values) = self.pending_properties.remove(&pane) else { return true };
                match answer {
                    Some("_Allow Once") => self.apply_properties(pane, values),
                    Some(b @ ("Allow Always" | "Deny Always")) => {
                        let n = if b == "Allow Always" { 0 } else { 1 };
                        for (k, _) in &values {
                            self.globals.set(&format!("NoSyncSetProfileProperty_{k}"), toml::Value::Integer(n));
                        }
                        self.save_globals();
                        if n == 0 {
                            self.apply_properties(pane, values);
                        }
                    }
                    _ => {}
                }
            }
            (ASK_BACKGROUND, answer) => {
                let Some(path) = self.pending_background.remove(&pane) else { return true };
                match answer {
                    Some("Yes") => self.apply_background(pane, &path),
                    Some("Always") => {
                        self.push_global_list("AlwaysAllowBackgroundImage", &path);
                        self.apply_background(pane, &path);
                    }
                    Some("Never") => self.push_global_list("AlwaysDenyBackgroundImage", &path),
                    _ => {}
                }
            }
            (ASK_PROFILE, None) => {}
            _ => return false,
        }
        true
    }

    /// A key press while the focused pane shows a bar: Alt+letter presses that button, and
    /// any other key dismisses a bar that is not important. True when the key was used.
    pub(super) fn bar_key(&mut self, ev: &KeyEvent, event_loop: &ActiveEventLoop) -> bool {
        let Some(pane) = self.focused_id() else { return false };
        let Some(bar) = self.bars.get(&pane).and_then(|b| b.front()).cloned() else { return false };
        // Modifier keys alone do nothing.
        if keystroke::key_of(&ev.logical_key, ev.text.as_deref()).is_none() {
            return false;
        }
        let pressed = if self.mods.alt {
            let letter = match &ev.key_without_modifiers() {
                winit::keyboard::Key::Character(c) => c.chars().next(),
                _ => None,
            };
            letter.and_then(|l| bar.shortcut(l))
        } else {
            None
        };
        if pressed.is_none() && bar.important {
            return false;
        }
        if let Some(b) = self.bars.get_mut(&pane) {
            b.pop();
        }
        self.bar_answered(pane, &bar.id, pressed.map(|i| bar.buttons[i].as_str()), event_loop);
        self.redraw();
        true
    }
}

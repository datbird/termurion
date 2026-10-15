//! Announcements on a session's bar: a refused window resize (Allow Once, Open Settings, Don't
//! Show This Again) and the offer to pretty-print JSON that was just copied (`PTYSession.m`).
//! The texts and rules are `announce`.

use super::*;
use crate::announce as an;
use crate::chrome::ResizePermission;

impl App {
    /// A program tried to resize or move the window and the profile refuses: the announcement asks
    /// (unless "Don't Show This Again" was chosen); a later request replaces the one waiting.
    pub(super) fn ask_to_resize(&mut self, pane: PaneId, permission: ResizePermission, frame: (f64, f64, f64, f64)) {
        let Some((id, text, key)) = an::resize_announcement(permission) else { return };
        if self.globals.bool(key) {
            return;
        }
        self.resize_pending.insert(pane, frame);
        self.ask(pane, id, text, &an::RESIZE_BUTTONS, false);
    }

    /// The answers of the announcements here. False for other bars.
    pub(super) fn announce_bar(&mut self, pane: PaneId, id: &str, button: Option<&str>, event_loop: &ActiveEventLoop) -> bool {
        match id {
            an::RESIZE_ID | an::RESIZE_UNFOCUSED_ID => {
                let frame = self.resize_pending.remove(&pane);
                match button {
                    Some("_Allow Once") => {
                        if let Some((x, y, w, h)) = frame {
                            self.apply_window_frame(x, y, w, h);
                        }
                    }
                    // The profile that refused (the saved one behind a private copy): its settings page.
                    Some("_Open Settings") => {
                        let guid = self.panes.get(&pane).map(|p| self.store.shared_guid(&p.profile));
                        self.edit_profiles(event_loop, guid);
                    }
                    Some(b) if b == an::RESIZE_BUTTONS[2] => {
                        let key = if id == an::RESIZE_ID { an::RESIZE_SUPPRESS_KEY } else { an::RESIZE_UNFOCUSED_SUPPRESS_KEY };
                        self.globals.set(key, toml::Value::Boolean(true));
                        self.save_globals();
                    }
                    _ => {}
                }
                true
            }
            an::JSON_ID => {
                if button == Some(an::JSON_BUTTONS[0]) {
                    self.json_try_now(pane);
                }
                true
            }
            _ => false,
        }
    }

    /// After a copy: the offer to pretty-print, once ever (`textViewShowJSONPromotion`).
    pub(super) fn json_promotion(&mut self, pane: PaneId, copied: &str) {
        if self.globals.bool(an::JSON_PROMOTED_KEY) || !an::worth_promoting(copied) {
            return;
        }
        self.globals.set(an::JSON_PROMOTED_KEY, toml::Value::Boolean(true));
        self.save_globals();
        self.ask(pane, an::JSON_ID, an::JSON_TEXT, &an::JSON_BUTTONS, false);
    }

    /// Try it Now: Replace with Pretty-Printed JSON on the selection, then where to find it again;
    /// a selection that no longer is JSON says so instead.
    fn json_try_now(&mut self, pane: PaneId) {
        let ok = self.selected_text(pane).is_some_and(|t| crate::seltools::pretty_json(&t).is_ok());
        if ok {
            self.replace_selection(pane, crate::seltools::pretty_json);
            self.ask_scripts(super::scripts_alerts::ask(an::JSON_DONE_HEADING, an::JSON_FEATURE_LOCATION, &[crate::api_auth::OK], None, crate::warnings::Kind::Persistent, super::scripts_alerts::Then::Nothing));
        } else {
            self.ask_scripts(super::scripts_alerts::ask("", an::JSON_SELECTION_CHANGED, &[crate::api_auth::OK], None, crate::warnings::Kind::Persistent, super::scripts_alerts::Then::Nothing));
        }
    }
}

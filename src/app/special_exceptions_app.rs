//! Manage Special Exceptions on the app side: the window (opened from a profile's Text tab) and
//! where its OK writes the table (`SE`, `PSV` "Manage Special Exceptions").

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::{App, Edit};
use crate::egui_host::EguiHost;
use crate::font_table::Config;
use crate::special_exceptions_ui::{Outcome, Window};
use crate::store::Store;

/// The profile value the window edits.
pub const KEY: &str = crate::font_table::KEY;

/// The open window and the profile it edits.
pub struct Open {
    host: EguiHost,
    win: Window,
    guid: String,
}

/// Writes `config` as the Special Font Config of profile `guid`: a saved profile takes it, a
/// session's private copy takes it as an override of its own. An empty table is stored as an
/// empty string, so a copy can override a saved profile's table with none. False when the
/// profile is gone.
pub fn store_config(store: &mut Store, guid: &str, config: &Config) -> bool {
    let text = if config.entries.is_empty() { String::new() } else { config.string() };
    if store.is_session(guid) {
        store.set_session(guid, &[(KEY.to_string(), Some(toml::Value::String(text)))]);
        return true;
    }
    let Some(profile) = store.get_mut(guid) else { return false };
    profile.set(KEY, toml::Value::String(text));
    true
}

impl App {
    /// The Text tab's "Manage Special Exceptions": opens the window on profile `guid`'s table
    /// (the session's own copy when it is a session's), or brings the open one forward.
    pub(super) fn open_special_exceptions(&mut self, guid: String, event_loop: &ActiveEventLoop) {
        if let Some(o) = self.special_exceptions.as_ref().filter(|o| o.guid == guid) {
            return o.host.focus();
        }
        let Some(profile) = self.store.get(&guid) else { return };
        // A table that does not parse (a hand-edited or API-set value) opens as an empty one.
        let config = Config::parse(&profile.string(KEY)).unwrap_or_else(|| Config::with_entries(Vec::new()));
        let Some(g) = self.gpu.as_ref() else { return };
        // The installed fonts are listed once, here.
        let fonts = g.renderer.installed_families();
        match EguiHost::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, crate::special_exceptions_ui::TITLE, 560.0, 440.0) {
            Ok(host) => self.special_exceptions = Some(Open { host, win: Window::open(config, fonts), guid }),
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// The window's events: OK writes the table to the profile, Cancel and closing drop the edits.
    /// False for other windows.
    pub(super) fn special_exceptions_event(&mut self, window: WindowId, event: &WindowEvent) -> bool {
        let Some(o) = self.special_exceptions.as_mut().filter(|o| o.host.id() == window) else { return false };
        let outcome = match event {
            WindowEvent::CloseRequested => Some(Outcome::Cancel),
            WindowEvent::RedrawRequested => {
                let mut out = None;
                let win = &mut o.win;
                o.host.paint(|ui| out = win.ui(ui));
                out
            }
            other => {
                o.host.event(other);
                None
            }
        };
        match outcome {
            None => {}
            Some(Outcome::Cancel) => self.special_exceptions = None,
            Some(Outcome::Ok(config)) => {
                if let Some(o) = self.special_exceptions.take() {
                    if store_config(&mut self.store, &o.guid, &config) {
                        self.apply_edit(Edit::Changed(o.guid));
                    }
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font_table::Entry;
    use crate::profile::Profile;

    fn table() -> Config {
        Config::with_entries(vec![Entry { start: 0xE000, count: 16, destination: None, font_name: "Consolas".into(), point_size: Some(12.0) }])
    }

    #[test]
    fn ok_writes_the_special_font_config() {
        let mut store = Store::fresh(std::path::Path::new("none.toml"));
        let p = Profile::new("Work");
        let saved = p.guid();
        store.add(p);
        // A saved profile takes the table as its own value.
        assert!(store_config(&mut store, &saved, &table()));
        assert_eq!(store.get(&saved).unwrap().string(KEY), table().string());
        // A session copy overrides it and the saved profile keeps its table.
        let copy = store.divorce(&saved);
        assert!(store_config(&mut store, &copy, &Config::with_entries(Vec::new())));
        assert_eq!(store.get(&copy).unwrap().string(KEY), "", "an empty table overrides the saved one");
        assert!(store.sessions.iter().any(|s| s.overridden.contains(KEY)));
        assert_eq!(store.get(&saved).unwrap().string(KEY), table().string());
        assert!(!store_config(&mut store, "gone", &table()));
    }

    /// The Text tab's button opens the window, and the window's OK reaches the profile.
    #[test]
    fn the_text_tab_button_opens_the_window() {
        let settings = include_str!("../settings_ui.rs");
        assert!(settings.contains("s.key == crate::font_table::KEY") && settings.contains("*special_exceptions = Some(selected.clone())"));
        let app = include_str!("mod.rs");
        assert!(app.contains("self.open_special_exceptions(guid, event_loop)") && app.contains("self.special_exceptions_event(id, &event)"));
        let schema = crate::schema::find(crate::font_table::KEY).expect("the key");
        assert_eq!((schema.tab.as_str(), schema.label.as_str()), ("Text", "Manage Special Exceptions"));
        assert!(include_str!("special_exceptions_app.rs").contains("self.apply_edit(Edit::Changed(o.guid))"));
    }
}

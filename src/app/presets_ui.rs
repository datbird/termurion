//! Loading color presets into sessions and profiles from outside the Colors tab: key action 40,
//! Open Quickly, and the escape code (`PS:12100-12116`, `PS:5570-5583`).

use super::*;

impl App {
    /// `setColorsFromPresetNamed:`: the session's own copy takes the preset's colors (a copy is
    /// made if it has none). False for an unknown name or a pane that is gone.
    pub(super) fn load_preset_into_session(&mut self, pane: PaneId, name: &str) -> bool {
        let Some(preset) = crate::color_presets::find(&self.globals, name) else { return false };
        if !self.panes.contains_key(&pane) {
            return false;
        }
        self.set_session_values(pane, &crate::color_presets::session_values(&preset));
        true
    }

    /// Key action 40 (`PS:12100-12116`): the session, then the profile behind it, with one save.
    pub(super) fn load_preset_action(&mut self, pane: PaneId, name: &str) {
        let Some(preset) = crate::color_presets::find(&self.globals, name) else { return };
        let Some(guid) = self.panes.get(&pane).map(|p| p.profile.clone()) else { return };
        let shared = self.store.shared_guid(&guid);
        let divorced = shared != guid;
        let (session, profile) = crate::color_presets::action_plan(divorced, self.store.get_mut(&shared).is_some());
        if session {
            self.load_preset_into_session(pane, name);
        }
        if profile {
            if let Some(p) = self.store.get_mut(&shared) {
                crate::color_presets::apply_whole(&preset, p);
            }
            self.store.merge_sessions();
            self.refresh_panes();
            self.save_due = Some(Instant::now() + SAVE_DELAY);
        }
    }

    /// Every preset's name, built-in and custom (at most 11 plus 256, sanitized).
    pub(super) fn preset_names(&self) -> Vec<String> {
        crate::color_presets::all_names(&self.globals)
    }

    /// SetColors `preset=Name` (B5): the preset's named colors become program-set colors on the
    /// pane (kept over the profile's, gone when the program ends) and its tab color the pane's.
    /// An unknown name does nothing.
    pub(super) fn escape_preset(&mut self, pane: PaneId, name: &str) {
        let Some(preset) = crate::color_presets::find(&self.globals, name) else { return };
        let Some(p) = self.panes.get_mut(&pane) else { return };
        let dark = p.screen.is_dark();
        p.screen.set_named_colors(&crate::color_presets::named_colors(&preset, dark));
        match crate::color_presets::tab_color(&preset, dark) {
            Some(c) => self.tab_colors.insert(pane, c),
            None => self.tab_colors.remove(&pane),
        };
        self.redraw();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn load_color_preset_action_follows_iterm2() {
        use crate::color_presets::action_plan;
        assert_eq!(action_plan(false, true), (true, true));
        assert_eq!(action_plan(true, true), (true, true), "a divorced session finds its backing profile by the original guid");
        assert_eq!(action_plan(true, false), (true, false));
        assert_eq!(crate::keyaction::status(40), crate::commands::Status::Done);
        assert_eq!(crate::keyaction::info(40).map(|a| a.arg), Some(crate::keyaction::Arg::ColorPreset));
        let b = crate::keyaction::Binding { action: 40, text: "Solarized".into(), label: String::new(), version: 2, escaping: None, apply_mode: 0 };
        assert_eq!(crate::settings_keys::action_summary(&b), "Load Color Preset \u{201c}Solarized\u{201d}");
    }
}

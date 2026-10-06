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
        let values = crate::color_presets::session_values(preset);
        self.set_session_values(pane, &values);
        true
    }

    /// Key action 40 (`PS:12100-12116`): the session, then the profile behind it, with one save.
    pub(super) fn load_preset_action(&mut self, pane: PaneId, name: &str) {
        // A key action, not per frame: one copy, so the session load below can borrow self.
        let Some(preset) = crate::color_presets::find(&self.globals, name).cloned() else { return };
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

    /// SetColors `preset=Name` (B5): the preset's named colors become program-set colors on the
    /// pane (kept over the profile's, gone when the program ends) and its tab color the pane's.
    /// An unknown name does nothing.
    pub(super) fn escape_preset(&mut self, pane: PaneId, name: &str) {
        let Some(preset) = crate::color_presets::find(&self.globals, name) else { return };
        let Some(p) = self.panes.get_mut(&pane) else { return };
        let dark = p.screen.is_dark();
        p.screen.apply_escape_preset(&preset);
        let current = self.tab_colors.get(&pane).copied();
        if let Some(c) = crate::color_presets::escape_tab_color(&preset, dark, current) {
            self.tab_colors.insert(pane, c);
        }
        self.redraw();
    }
}

#[cfg(test)]
mod tests {
    /// Merge seam S7 (13s): the command outline and shade read the pane's palette on every frame,
    /// so a preset loaded through the profile (key action 40, Open Quickly, the Colors menu) or the
    /// escape code (SetColors preset=) shows in the next frame's outline.
    #[test]
    fn command_outline_follows_a_loaded_preset() {
        use crate::cmdsel::{outline_colors, shade_color};
        let (_, preset) = crate::color_presets::built_in().iter().find(|(n, _)| n == "Solarized Dark").unwrap();
        // The escape-code path: the pane's own palette.
        let mut s = crate::screen::Screen::new(20, 4);
        let before = (outline_colors(s.palette.selection, s.palette.bg), shade_color(s.palette.bg, 0.25));
        s.apply_escape_preset(preset);
        let after = (outline_colors(s.palette.selection, s.palette.bg), shade_color(s.palette.bg, 0.25));
        assert_ne!(before.0, after.0, "the outline follows the preset's selection and background");
        assert_eq!(after.0[0], s.palette.selection);
        // The shade flips with the background's lightness: a light preset turns it black.
        let (_, light) = crate::color_presets::built_in().iter().find(|(n, _)| n == "Light Background").unwrap();
        s.apply_escape_preset(light);
        assert_ne!(after.1, shade_color(s.palette.bg, 0.25), "the shade follows the preset's background");
        // The profile path: refresh_panes rebuilds the palette from the profile.
        let mut profile = crate::profile::Profile::new("t");
        let old = crate::palette::Palette::from_profile(&profile, true);
        crate::color_presets::apply_whole(preset, &mut profile);
        let new = crate::palette::Palette::from_profile(&profile, true);
        assert_ne!(outline_colors(old.selection, old.bg), outline_colors(new.selection, new.bg));
        // The draw site reads the live palette, not a copy kept across frames.
        let src = include_str!("mod.rs");
        let draw = &src[src.find("let hl = scene::Highlights {").unwrap()..];
        let draw = &draw[..draw.find('\n').unwrap()];
        assert!(draw.contains("outline: crate::cmdsel::outline_colors(p.screen.palette.selection, p.screen.palette.bg)"), "{draw}");
        assert!(draw.contains("crate::cmdsel::shade_color(p.screen.palette.bg,"), "{draw}");
    }

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

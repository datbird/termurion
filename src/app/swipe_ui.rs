//! Horizontal swipes over a pane: tab switching and iTerm2's note about mouse reporting taking
//! the swipe (`PTYSession.m:13842`, `21689-21726`). See `crate::swipe_tabs`.

use super::*;
use crate::swipe_tabs as st;

fn clock() -> f64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

impl App {
    /// A horizontal wheel event no program took: a swipe between tabs.
    pub(super) fn swipe_tabs(&mut self, dx: f64, precise: bool) {
        let Some(step) = self.swipe.feed(clock(), dx, precise) else { return };
        if let Some(i) = st::target(self.active, self.tabs.len(), step) {
            self.select_tab(i);
        }
    }

    /// A horizontal wheel report went to the program in `pane`: the note, if it is due.
    pub(super) fn horizontal_scroll_note(&mut self, pane: PaneId) {
        let showing = self.bars.get(&pane).is_some_and(|b| b.has(st::BAR_ID));
        if st::note_due(true, self.tabs.len(), self.globals.bool(st::NEVER_KEY), showing) {
            self.ask(pane, st::BAR_ID, st::TEXT, &[st::SETTINGS], false);
        }
    }

    /// The note's answer: any answer, a close included, silences it for good; Settings also
    /// opens Settings at the horizontal reporting check box. False for other bars.
    pub(super) fn swipe_bar(&mut self, id: &str, button: Option<&str>, event_loop: &ActiveEventLoop) -> bool {
        if id != st::BAR_ID {
            return false;
        }
        if button == Some(st::SETTINGS) {
            self.open_settings_page(event_loop, crate::settings_ui::Page::Pointer);
            if let Some(s) = self.settings.as_mut() {
                s.reveal_setting(st::SETTING_KEY);
            }
        }
        self.globals.set(st::NEVER_KEY, toml::Value::Boolean(true));
        self.save_globals();
        true
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_wheel_path_switches_tabs_and_shows_the_note() {
        let ui = include_str!("mouse_ui.rs");
        assert!(ui.contains("self.horizontal_scroll_note(id)"), "a horizontal report shows the note");
        assert!(ui.contains("self.swipe_tabs(dx, ev.precise)"), "a horizontal wheel no program took swipes");
        assert!(include_str!("infobar_ui.rs").contains("self.swipe_bar(id, button, event_loop)"));
        assert!(include_str!("mod.rs").contains("swipe: crate::swipe_tabs::Swipe"));
    }
}

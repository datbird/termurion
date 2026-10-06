//! Opening the Profiles window's selection: several profiles as tabs, windows or splits, the
//! question above eight (P6), Close after opening, and "Edit Profiles\u{2026}"
//! (`iTermProfilesWindowController.m:213-440`).

use super::*;
use crate::settings_ui::{asks_before_opening, CLOSE_AFTER_OPENING, closes_after, open_plan, How, Step};


/// The question that asks before many sessions start (P6): its heading and text.
pub(super) fn open_many_text(count: usize) -> (String, &'static str) {
    (format!("Open {count} sessions?"), "Each profile starts a session of its own.")
}

impl App {
    /// Opens `guids` as `how`, asking first when there are more than `OPEN_ASK_ABOVE` (P6).
    pub(super) fn open_selected_profiles(&mut self, guids: Vec<String>, how: How) {
        if asks_before_opening(guids.len()) {
            let (heading, text) = open_many_text(guids.len());
            let ask = scripts_alerts::ask(&heading, text, &[crate::api_auth::OK, crate::api_auth::CANCEL], None, crate::warnings::Kind::Persistent, scripts_alerts::Then::OpenProfiles(guids, how));
            return self.ask_scripts(ask);
        }
        self.open_selected_now(guids, how);
    }

    /// The answered question's OK, or a small selection: each step goes through the usual
    /// spawn path (`new_tab`, `split`, `pending_windows`), then the Profiles window closes when
    /// its checkbox says so.
    pub(super) fn open_selected_now(&mut self, guids: Vec<String>, how: How) {
        let window_exists = self.current_window().is_some();
        for step in open_plan(&guids, how, window_exists) {
            match step {
                // A tab or split goes to the window the person used last.
                Step::Tab(g) => self.new_tab(Some(&g)),
                Step::Window(g) => self.pending_windows.push(windows_ui::NewWindow::Profile(Some(g))),
                Step::Split(side_by_side, g) => self.split(if side_by_side { Axis::Row } else { Axis::Column }, Some(&g)),
                Step::NewWindow(first, rest) => self.pending_windows.push(windows_ui::NewWindow::ProfilesIn(first, rest)),
            }
        }
        if closes_after(self.globals.bool(CLOSE_AFTER_OPENING), how) {
            self.profiles_win = None;
        }
    }

    /// "Edit Profiles\u{2026}": Settings on the selected profile's General tab, or on the Profiles
    /// page with none selected (`editBookmarks:`).
    pub(super) fn edit_profiles(&mut self, event_loop: &ActiveEventLoop, guid: Option<String>) {
        self.open_settings(event_loop, Mode::Settings);
        if let Some(s) = self.settings.as_mut() {
            s.show_profiles(guid.as_deref());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// More than eight profiles ask first (P6); closing the question is Cancel, never OK.
    #[test]
    fn opening_many_asks_a_persistent_question() {
        let (heading, _) = open_many_text(9);
        assert_eq!(heading, "Open 9 sessions?");
        let guids: Vec<String> = (0..9).map(|i| i.to_string()).collect();
        let ask = scripts_alerts::ask(&heading, "t", &[crate::api_auth::OK, crate::api_auth::CANCEL], None, crate::warnings::Kind::Persistent, scripts_alerts::Then::OpenProfiles(guids, How::Tab));
        assert_eq!(scripts_alerts::on_close(&ask.then, 1), scripts_alerts::Answer::Button(1));
        assert_eq!(ask.spec.cancel, Some(1));
    }
}

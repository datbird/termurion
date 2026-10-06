//! Opening the Profiles window's selection: several profiles as tabs, windows or splits, the
//! question above eight (P6), Close after opening, and "Edit Profiles\u{2026}"
//! (`iTermProfilesWindowController.m:213-440`).

use super::*;
use crate::settings_ui::{asks_before_opening, CLOSE_AFTER_OPENING, closes_after, open_plan, How, Opener, Step};


/// The question that asks before many sessions start (P6): its heading and text.
pub(super) fn open_many_text(count: usize) -> (String, &'static str) {
    (format!("Open {count} sessions?"), "Each profile starts a session of its own.")
}

/// `guids` as a crowd may open them: when more than one opens at once (Open All, a multiple
/// selection, a tag open) the Bonjour profiles (P5) and the dynamic ones (P8, a planted file
/// cannot fan out sessions) are left out; a single pick is the person's choice.
pub(super) fn crowd_guids(store: &crate::store::Store, guids: Vec<String>) -> Vec<String> {
    let guids = super::bonjour_ui::without_bonjour(store, guids);
    if guids.len() < 2 {
        return guids;
    }
    guids.into_iter().filter(|g| !store.get(g).is_some_and(|p| p.bool(crate::dynamic_profiles::DYNAMIC_KEY))).collect()
}

impl App {
    /// Opens `guids` as `how`, asking first when there are more than `OPEN_ASK_ABOVE` (P6).
    pub(super) fn open_selected_profiles(&mut self, guids: Vec<String>, how: How, from: Opener) {
        // Bonjour hosts and dynamic profiles never open in a crowd (P5, P8).
        let guids = crowd_guids(&self.store, guids);
        if guids.is_empty() {
            return;
        }
        if asks_before_opening(guids.len()) {
            let (heading, text) = open_many_text(guids.len());
            let ask = scripts_alerts::ask(&heading, text, &[crate::api_auth::OK, crate::api_auth::CANCEL], None, crate::warnings::Kind::Persistent, scripts_alerts::Then::OpenProfiles(guids, how, from));
            return self.ask_scripts(ask);
        }
        self.open_selected_now(guids, how, from);
    }

    /// The answered question's OK, or a small selection: each step goes through the usual
    /// spawn path (`new_tab`, `split`, `pending_windows`), then the Profiles window closes when
    /// its checkbox says so and the opening was its own.
    pub(super) fn open_selected_now(&mut self, guids: Vec<String>, how: How, from: Opener) {
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
        if closes_after(self.globals.bool(CLOSE_AFTER_OPENING), how, from) {
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

    /// P8: a dynamic profile never takes part in a multi-profile open, a single pick opens.
    #[test]
    fn a_crowd_leaves_dynamic_profiles_out() {
        let mut s = crate::store::Store::fresh(&std::env::temp_dir().join(format!("tandem-crowd-{}.toml", std::process::id())));
        let saved = s.profiles[0].guid();
        let mut d = crate::profile::Profile::new("dyn");
        d.set("Is Dynamic Profile", toml::Value::Boolean(true));
        let dynamic = d.guid();
        s.add(d);
        let other = crate::profile::Profile::new("other");
        let other_guid = other.guid();
        s.add(other);
        assert_eq!(crowd_guids(&s, vec![saved.clone(), dynamic.clone(), other_guid.clone()]), [saved.clone(), other_guid]);
        assert_eq!(crowd_guids(&s, vec![dynamic.clone()]), [dynamic.clone()], "a single pick is the person's choice");
    }

    /// More than eight profiles ask first (P6); closing the question is Cancel, never OK.
    #[test]
    fn opening_many_asks_a_persistent_question() {
        let (heading, _) = open_many_text(9);
        assert_eq!(heading, "Open 9 sessions?");
        let guids: Vec<String> = (0..9).map(|i| i.to_string()).collect();
        let ask = scripts_alerts::ask(&heading, "t", &[crate::api_auth::OK, crate::api_auth::CANCEL], None, crate::warnings::Kind::Persistent, scripts_alerts::Then::OpenProfiles(guids, How::Tab, Opener::ProfilesWindow));
        assert_eq!(scripts_alerts::on_close(&ask.then, 1), scripts_alerts::Answer::Button(1));
        assert_eq!(ask.spec.cancel, Some(1));
    }
}

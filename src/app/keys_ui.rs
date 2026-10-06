//! Force keyboard (`InputSourceForcer.swift`): the app side of `input_locale`. The layout is
//! forced while a window of this process is in front (the `app_active` edge, a process test, so
//! a move between a terminal and Settings changes nothing) and the person's comes back when
//! Tandem is no longer in front and at exit. `ActivateKeyboardLayout` runs here, on the UI thread.

use super::App;
use crate::input_locale::{self, Prefs, Step};

impl App {
    fn locale_prefs(&self) -> Prefs {
        Prefs::from_globals(&self.globals)
    }

    /// Carries out a step and keeps the panic restore and the seen layout current.
    fn locale_run(&mut self, step: Step) {
        if let Step::Activate(id) = step {
            input_locale::activate(&id);
        }
        input_locale::arm_restore(self.locale.to_restore());
        self.locale_seen = input_locale::current();
    }

    /// The app became active or resigned: `was` and `now` are "a window of Tandem is in front".
    pub(super) fn locale_focus(&mut self, was: bool, now: bool) {
        let Some(active) = input_locale::activation_change(was, now) else { return };
        let prefs = self.locale_prefs();
        let step = if active { self.locale.became_active(&prefs, &input_locale::current()) } else { self.locale.resigned(&prefs) };
        self.locale_run(step);
    }

    /// A setting changed.
    pub(super) fn locale_update(&mut self) {
        let prefs = self.locale_prefs();
        let step = self.locale.prefs_changed(&prefs);
        self.locale_run(step);
    }

    /// The person changed the layout while Tandem is in front: it is forced again, once. Runs with
    /// the event loop's own passes; nothing wakes for it.
    pub(super) fn locale_tick(&mut self) {
        if !self.locale.is_active() {
            return;
        }
        let prefs = self.locale_prefs();
        if !prefs.force || prefs.once || input_locale::current() == self.locale_seen {
            return;
        }
        let step = self.locale.layout_changed(&prefs);
        self.locale_run(step);
    }

    /// A new session that has the keyboard gets the layout once (once per session).
    pub(super) fn force_for_session(&mut self) {
        if !self.locale.is_active() || !self.focused {
            return;
        }
        let prefs = self.locale_prefs();
        if !(prefs.force && prefs.once) {
            return;
        }
        let Some(needs) = self.focused_pane().map(|p| p.needs_forced_layout) else { return };
        if !needs {
            return;
        }
        let (step, needs) = self.locale.session_focused(&prefs, needs);
        if let Some(p) = self.focused_pane() {
            p.needs_forced_layout = needs;
        }
        self.locale_run(step);
    }

    /// Tandem is ending: the person's layout comes back.
    pub(crate) fn locale_exit(&mut self) {
        let step = self.locale.exiting();
        self.locale_run(step);
    }
}

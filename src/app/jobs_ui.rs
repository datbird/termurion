//! The foreground job in the window: the job variables, titles, job triggers, and
//! Automatic Profile Switching react to it.

use super::*;

impl App {
    /// The watcher saw a job change (or a snapshot error).
    pub(super) fn jobs_changed(&mut self) {
        match self.jobs.error() {
            Some(e) => self.banners.set("jobs", format!("Could not read the running programs: {e}")),
            None => self.banners.clear("jobs"),
        }
        self.redraw();
    }
}

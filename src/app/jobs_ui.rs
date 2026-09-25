//! The foreground job in the window: the job variables, titles, job triggers, and
//! Automatic Profile Switching react to it.

use super::*;
use crate::jobtitle::{self, TitleParts};

impl App {
    /// The watcher saw a job change (or a snapshot error).
    pub(super) fn jobs_changed(&mut self) {
        match self.jobs.error() {
            Some(e) => self.banners.set("jobs", format!("Could not read the running programs: {e}")),
            None => self.banners.clear("jobs"),
        }
        let ids: Vec<PaneId> = self.panes.keys().copied().collect();
        for id in ids {
            let Some(pid) = self.panes.get(&id).and_then(|p| p.shell_pid()) else { continue };
            let now = self.jobs.job(pid);
            let Some(p) = self.panes.get_mut(&id) else { continue };
            let s = p.own_mut();
            if s.shell.job != now.job || s.shell.job_ancestors != now.ancestors {
                s.set_job(now.job, now.ancestors);
            }
        }
        self.redraw();
    }

    /// A session's title from its profile's Title Components.
    pub(super) fn session_title(&self, pane: PaneId) -> String {
        let Some(p) = self.panes.get(&pane) else { return "Shell".into() };
        let s = p.own();
        let profile = self.store.get(&p.profile);
        let components = profile.map_or(jobtitle::JOB as i64, |pr| pr.int("Title Components")).clamp(0, u32::MAX as i64) as u32;
        use alacritty_terminal::grid::Dimensions;
        let grid = s.term().grid();
        let parts = TitleParts {
            explicit: s.title().map(str::to_string),
            profile: profile.map(|pr| pr.name()).unwrap_or_default(),
            job: s.shell.job.as_ref().map(|j| j.title.clone()).unwrap_or_default(),
            cmdline: s.shell.job.as_ref().map(|j| j.cmdline.clone()).unwrap_or_default(),
            user: s.shell.user.clone().unwrap_or_default(),
            host: s.shell.host.clone().unwrap_or_default(),
            path: s.shell.cwd.clone().unwrap_or_default(),
            home: std::env::var("USERPROFILE").unwrap_or_default(),
            size: (grid.columns(), grid.screen_lines()),
        };
        jobtitle::title(components, &parts)
    }

    /// The window's title: a program's own window title as it is, else the session title.
    pub(super) fn window_title(&self, pane: PaneId) -> String {
        match self.panes.get(&pane).and_then(|p| p.own().title()) {
            Some(t) => t.to_string(),
            None => self.session_title(pane),
        }
    }
}

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
            if now.root.is_some() && s.shell.job_root != now.root {
                s.shell.job_root = now.root.clone();
            }
            if s.shell.job != now.job || s.shell.job_ancestors != now.ancestors {
                s.set_job(now.job, now.ancestors);
            }
        }
        // Job Started and Job Ended triggers leave their actions as requests.
        self.take_requests();
        self.run_automatic_profile_switching();
        self.redraw();
    }

    /// The Title Function's unique id when the session's Title Components is exactly Custom
    /// (`PS:22680-22692`): None when it is not Custom, Some(None) with no Title Function.
    pub(super) fn title_provider_id(&self, pane: PaneId) -> Option<Option<String>> {
        let profile = self.store.get(&self.panes.get(&pane)?.profile)?;
        (profile.int("Title Components") == i64::from(jobtitle::CUSTOM)).then(|| jobtitle::title_function(profile.get("Title Function").as_ref()).map(|(_, id)| id))
    }

    /// A session's title from its profile's Title Components; with Custom alone, its title
    /// provider's (`SNC:98-170`): "" with no Title Function, the ellipsis while no registered
    /// provider has its unique id, else the provider's last value (`script_titles`).
    pub(super) fn session_title(&self, pane: PaneId) -> String {
        match self.title_provider_id(pane) {
            Some(None) => return String::new(),
            Some(Some(id)) => {
                if !self.api_fns.title_providers().iter().any(|p| p.unique_id == id) {
                    return crate::script_eval::TITLE_UNREGISTERED.to_string();
                }
                return self.script_titles.get(&pane).filter(|t| t.unique_id == id).map(|t| t.value.clone()).unwrap_or_default();
            }
            None => {}
        }
        let Some(p) = self.panes.get(&pane) else { return "Shell".into() };
        let s = p.own();
        let profile = self.store.get(&p.profile);
        let components = profile.map_or(jobtitle::JOB as i64, |pr| pr.int("Title Components")).clamp(0, u32::MAX as i64) as u32;
        use alacritty_terminal::grid::Dimensions;
        let grid = s.term().grid();
        let mut parts = TitleParts {
            explicit: s.title().map(str::to_string),
            profile: profile.map(|pr| pr.name()).unwrap_or_default(),
            job: s.shell.job.as_ref().map(|j| j.title.clone()).unwrap_or_default(),
            cmdline: s.shell.job.as_ref().map(|j| j.cmdline.clone()).unwrap_or_default(),
            user: s.shell.user.clone().unwrap_or_default(),
            host: s.shell.host.clone().unwrap_or_default(),
            path: s.shell.cwd.clone().unwrap_or_default(),
            home: std::env::var("USERPROFILE").unwrap_or_default(),
            size: (grid.columns(), grid.screen_lines()),
            tmux_pane_title: String::new(),
        };
        // A tmux pane: what tmux's monitors report (the job is the command's name).
        if let Some(io) = p.tmux_io() {
            let info = io.info.borrow();
            parts.tmux_pane_title = info.title.clone();
            if !info.job.is_empty() {
                parts.job = info.job.clone();
                parts.cmdline = info.job.clone();
            }
            if parts.path.is_empty() {
                parts.path = info.cwd.clone();
                parts.home.clear();
            }
        }
        jobtitle::title(components, &parts)
    }

    /// The window's title: a program's own window title as it is, else the session title.
    pub(super) fn window_title(&self, pane: PaneId) -> String {
        if let Some(t) = self.tmux_titles(pane) {
            return t;
        }
        match self.panes.get(&pane).and_then(|p| p.own().title()) {
            Some(t) => t.to_string(),
            None => self.session_title(pane),
        }
    }
}

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
            // A framing session's job is on its remote, never ssh.exe.
            let remote = p.remote_job();
            let s = p.own_mut();
            if now.root.is_some() && s.shell.job_root != now.root {
                s.shell.job_root = now.root.clone();
            }
            let (job, ancestors) = pane_job(now, remote);
            if s.shell.job != job || s.shell.job_ancestors != ancestors {
                s.set_job(job, ancestors);
            }
        }
        // Job Started and Job Ended triggers leave their actions as requests.
        self.take_requests();
        self.run_automatic_profile_switching();
        self.claude_jobs_changed();
        self.redraw();
    }

    /// The Title Function's unique id when the session's Title Components is exactly Custom
    /// (`PS:22680-22692`): None when it is not Custom, Some(None) with no Title Function.
    pub(super) fn title_provider_id(&self, pane: PaneId) -> Option<Option<String>> {
        let profile = self.store.get(&self.panes.get(&pane)?.profile)?;
        (profile.int("Title Components") == i64::from(jobtitle::CUSTOM)).then(|| jobtitle::title_function(profile.get("Title Function").as_ref()).map(|(_, id)| id))
    }

    /// A session's title from its profile's Title Components; with Custom alone, its title
    /// provider's (`SNC:98-170`): "" with no Title Function, the last value evaluated for its
    /// unique id (`script_titles`; kept when the provider goes, until the next evaluation
    /// shows the ellipsis), else the ellipsis while no registered provider has the unique id.
    pub(super) fn session_title(&self, pane: PaneId) -> String {
        let provider = self.title_provider_id(pane);
        if let Some(provider) = &provider {
            let base = match provider {
                None => String::new(),
                Some(id) => match self.script_titles.get(&pane).filter(|t| t.unique_id == *id && !t.deps.is_empty()) {
                    Some(t) => t.value.clone(),
                    None if !self.api_fns.has_title_provider(id) => crate::script_eval::TITLE_UNREGISTERED.to_string(),
                    None => String::new(),
                },
            };
            // A tmux client session with a custom title wears the tmux title prefix (`SNC:368-387`).
            let prefixed = self.tmux_window_name_of(pane).and_then(|name| jobtitle::tmux_session_title(&self.globals.string("tmuxTitlePrefix"), Some(provider.as_deref()), &base, &name));
            return prefixed.unwrap_or(base);
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
        let built = jobtitle::title(components, &parts);
        // A tmux client session wears the tmux title prefix and its window's name, not the built-in
        // components (`SNC:381-382`); the gateway and the tab label paths keep their own titles.
        match self.tmux_window_name_of(pane) {
            Some(name) => jobtitle::tmux_session_title(&self.globals.string("tmuxTitlePrefix"), None, &built, &name).unwrap_or(built),
            None => built,
        }
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

/// The job and ancestors a session shows: a framing session's remote list wins over the
/// local watcher's (which would name ssh.exe); with none the local one stays (`PS:25574-25581`).
fn pane_job(local: crate::jobwatch::ShellJob, remote: Option<(Option<crate::proctree::Job>, Vec<String>)>) -> (Option<crate::proctree::Job>, Vec<String>) {
    match remote {
        Some(remote) => remote,
        None => (local.job, local.ancestors),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobwatch::ShellJob;
    use crate::proctree::Job;

    fn job(name: &str, pid: u32) -> Job {
        Job { name: name.into(), title: name.into(), cmdline: name.into(), pid }
    }

    /// No App harness exists: the built-in branch of `session_title` routes a tmux pane through
    /// `tmux_session_title(.., None, ..)`; `tmux_tab_label` and the gateway are not touched here.
    #[test]
    fn built_in_session_title_wears_the_tmux_prefix() {
        let src = include_str!("jobs_ui.rs");
        let body = src.split("#[cfg(test)]").next().unwrap();
        assert!(body.contains("jobtitle::tmux_session_title(&self.globals.string(\"tmuxTitlePrefix\"), None, &built, &name)"));
        assert!(!body.contains("tmux_tab_label"));
    }

    #[test]
    fn remote_jobs_follow_iterm2() {
        let local = || ShellJob { job: Some(job("ssh", 5)), ancestors: vec!["ssh".into()], root: Some("pwsh".into()) };
        let (j, a) = pane_job(local(), Some((Some(job("vim", 200)), vec!["vim".into()])));
        assert_eq!((j, a), (Some(job("vim", 200)), vec!["vim".to_string()]), "the remote value wins");
        // A remote list with no job is still the remote's answer: ssh.exe is not the job.
        let (j, a) = pane_job(local(), Some((None, Vec::new())));
        assert_eq!((j, a), (None, Vec::new()));
        let (j, a) = pane_job(local(), None);
        assert_eq!((j, a), (Some(job("ssh", 5)), vec!["ssh".to_string()]), "no remote list keeps the local one");
    }
}

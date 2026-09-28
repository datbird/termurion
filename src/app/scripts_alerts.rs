//! The Scripts features' alerts, one at a time: downloads to confirm, the slow-scan question
//! and the notices after them (iTerm2's `iTermWarning`s from the scripts menu controller and
//! the provisioner). An alert that is permanently silenceable remembers its answer under
//! iTerm2's keys and is answered at once while silenced.

use std::collections::VecDeque;

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::script_runtime_ui::After;
use super::App;
use crate::prompt_ui::{ConfirmResult, ConfirmWindow};
use crate::script_provision::Job;

/// One alert waiting to show.
pub(super) struct Ask {
    pub heading: String,
    pub text: String,
    /// Left to right; Enter presses the first and Escape the one labeled "Cancel".
    pub buttons: Vec<String>,
    /// iTerm2's identifier when the alert can be silenced for good.
    pub silence: Option<&'static str>,
    pub then: Then,
}

/// What an answer does.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Then {
    Nothing,
    /// A download consent: the first button (OK) submits the jobs in order, each with what
    /// follows it; any other answer (or the ask dropped) cancels them: silent, but what waited
    /// on them hears so (an import restores what it replaced).
    Consent(Vec<(Job, After)>),
    /// "Performance Issue": Stop (0) or Continue (1).
    SlowScan,
    /// "Cannot Run Script": OK (0) or Reveal (1) the folder.
    Malformed(std::path::PathBuf),
    /// "Rebuild Python Environment?": Rebuild (0) builds the container again, then launches
    /// it; Cancel (1) does nothing.
    Rebuild(crate::script_launch::Request),
    /// "Error running script": OK only.
    ErrorRunning,
    /// "Spaces Not Allowed": Use _ Instead of Space (0) goes on with the new name; Change Name
    /// (1) opens the save dialog again.
    Spaces(crate::script_new::Plan),
    /// "Open new script in %@?": OK (0) opens the script with its editor; Show in Explorer (1)
    /// reveals it and is never remembered.
    OpenNewScript(std::path::PathBuf),
    /// "Invalid Folder": OK opens the save dialog again (the panel stays open in iTerm2).
    InvalidFolder(crate::script_new::Plan),
    /// "Install Script Archive?" for an archive the scan found: OK (0) imports it and recycles
    /// it once installed; Cancel (1) does nothing; Move to Trash (2) recycles it.
    Archive(std::path::PathBuf),
    /// "Script Already Exists" for the import under way (the script's name): Replace Script
    /// (0) moves the old one aside and installs; Cancel (1) ends the import quietly.
    AlreadyInstalled(String),
    /// "Script Imported Successfully": OK (0), or Launch (1) the installed path.
    ImportDone(std::path::PathBuf),
    /// "Could Not Install Script": OK (0), or Try Again (1) with the same file.
    ImportFailed(std::path::PathBuf),
    /// Manage Dependencies' "Run this Command?": OK (0) runs the pip command in a window on
    /// the next loop pass; Cancel (1) does nothing.
    RunPip(Box<super::script_deps_app::RunPip>),
    /// "Confirm Python Downgrade" / "Confirm Python Upgrade" for a container and the version
    /// picked: OK (0) asks the download consent, then rebuilds; Cancel (1) keeps the version.
    VersionChange(std::path::PathBuf, String),
    /// "Load settings from URL? ...": Keep Local Changes (0), Disable Loading from URL (1) or
    /// Discard Local Changes (2), which loads the URL.
    PrefsUrlPrompt(String),
    /// "Failed to load settings from URL. ...": OK (0), Reveal in Settings (1), and after a
    /// timeout Try Again Without Timeout (2).
    PrefsFailedUrl { url: String, timed_out: bool },
    /// "Error Loading Settings": Don't Load Remote Settings (0) turns the setting off; Cancel
    /// (1) does nothing.
    PrefsLocationNotSet,
    /// "Settings at ... changed since Tandem started. Overwrite it?": Overwrite (0) or Discard
    /// Local Changes (1).
    PrefsOverwrite,
    /// "Settings have changed. Copy them to ...?" at quit: Copy (0) or Lose Changes (1), which
    /// is never remembered.
    PrefsQuit,
    /// "Copy local settings to custom folder now?": Copy (0) or Don't Copy (1).
    PrefsCopyNow,
}

impl Then {
    /// Whether choosing button `index` may be remembered when the alert is silenced
    /// (`doNotRememberLabels`).
    fn remembers(&self, index: usize) -> bool {
        !matches!(self, Then::OpenNewScript(_) | Then::PrefsQuit if index == 1)
    }
}

/// The remembered-choice keys of a silenceable warning: the silence flag and the chosen
/// button (`iTermWarning.m`'s `identifier` and `identifier_selection`).
pub(super) fn silence_keys(id: &str) -> (String, String) {
    (id.to_string(), format!("{id}_selection"))
}

/// The queue holds at most this many; later asks are dropped (a flood of the same question
/// must not open windows forever).
const LIMIT: usize = 16;

/// Queues `ask`, or gives it back when the queue is full.
fn push_capped(queue: &mut VecDeque<Ask>, ask: Ask) -> Option<Ask> {
    if queue.len() < LIMIT {
        queue.push_back(ask);
        None
    } else {
        Some(ask)
    }
}

impl App {
    /// Queues an alert; a silenced one answers its remembered choice at once.
    pub(super) fn ask_scripts(&mut self, ask: Ask) {
        if let Some(id) = ask.silence {
            let (flag, selection) = silence_keys(id);
            if self.globals.bool(&flag) {
                let index = usize::try_from(self.globals.int(&selection)).unwrap_or(0);
                return self.scripts_answered(ask.then, index);
            }
        }
        if let Some(dropped) = push_capped(&mut self.scripts_alerts, ask) {
            self.scripts_dropped(dropped.then);
        }
    }

    /// An ask that will never show: the slow-scan question answers Continue, so the scan
    /// worker does not wait for ever; any other is dropped.
    fn scripts_dropped(&mut self, then: Then) {
        match then {
            Then::SlowScan => self.scripts_answered(then, 1),
            // Canceled: what waits on a consent or on the replace question hears so.
            Then::Consent(jobs) => self.consent_declined(jobs),
            Then::AlreadyInstalled(_) => self.scripts_answered(then, 1),
            _ => {}
        }
    }

    /// Opens the next queued alert when none shows.
    pub(super) fn open_scripts_alerts(&mut self, event_loop: &ActiveEventLoop) {
        if self.scripts_alert.is_some() || self.scripts_alerts.is_empty() {
            return;
        }
        // After the last window has gone, the settings folder's quit question opens with the
        // GPU that window used.
        let gpu = self.gpu.as_ref().map(|g| (&g.instance, &g.adapter, &g.device, &g.queue)).or(self.prefs_gpu.as_ref().map(|(i, a, d, q)| (i, a, d, q)));
        let Some((instance, adapter, device, queue)) = gpu else { return };
        let Some(ask) = self.scripts_alerts.pop_front() else { return };
        let labels: Vec<&str> = ask.buttons.iter().map(String::as_str).collect();
        match ConfirmWindow::actions(event_loop, instance, adapter, device, queue, &ask.heading, &ask.text, &labels, ask.silence.is_some()) {
            Ok(w) => self.scripts_alert = Some((w, ask)),
            Err(e) => {
                self.banners.set("window", format!("Could not open the window: {e}"));
                self.scripts_dropped(ask.then);
            }
        }
    }

    /// Events for the showing alert. False for other windows.
    pub(super) fn scripts_alert_event(&mut self, window: WindowId, event: &WindowEvent) -> bool {
        let Some((w, ask)) = self.scripts_alert.as_mut().filter(|(w, _)| w.id() == window) else { return false };
        let answer = match event {
            // iTerm2's alerts cannot be closed; closing the slow-scan question goes on scanning
            // rather than pressing Stop (the first button, which closing would otherwise mean).
            WindowEvent::CloseRequested if ask.then == Then::SlowScan => ConfirmResult::Action { index: 1, suppress: false },
            WindowEvent::CloseRequested => w.close_answer(),
            WindowEvent::RedrawRequested => w.redraw(),
            other => {
                w.event(other);
                ConfirmResult::Open
            }
        };
        let (index, suppress) = match answer {
            ConfirmResult::Open => return true,
            ConfirmResult::Action { index, suppress } => (index, suppress),
            // An `actions` window answers only with an action.
            ConfirmResult::Ok { .. } | ConfirmResult::Cancel => return true,
        };
        let Some((_, ask)) = self.scripts_alert.take() else { return true };
        if let (true, Some(id)) = (suppress && ask.then.remembers(index), ask.silence) {
            let (flag, selection) = silence_keys(id);
            self.globals.set(&flag, toml::Value::Boolean(true));
            self.globals.set(&selection, toml::Value::Integer(index as i64));
            self.save_globals();
        }
        self.scripts_answered(ask.then, index);
        true
    }

    /// Acts on an answer: the button's place in the alert's buttons.
    pub(super) fn scripts_answered(&mut self, then: Then, index: usize) {
        match then {
            Then::Nothing | Then::ErrorRunning => {}
            Then::Malformed(path) => {
                if index == 1 {
                    self.reveal_path(&path);
                }
            }
            Then::Rebuild(req) => {
                if index == 0 {
                    self.rebuild_script(req);
                }
            }
            Then::Consent(jobs) => {
                if index == 0 {
                    self.provision_jobs(jobs);
                } else {
                    self.consent_declined(jobs);
                }
            }
            Then::Archive(path) => match index {
                0 => self.import_path(path, true),
                2 => super::scripts_app::recycle(&path),
                _ => {}
            },
            Then::AlreadyInstalled(name) => {
                if index == 0 {
                    self.import_replace(&name);
                } else {
                    self.import_finish(Ok(None));
                }
            }
            Then::ImportDone(path) => {
                if index == 1 {
                    self.launch_script(crate::script_launch::Request { path, args: Vec::new(), explicit: true });
                }
            }
            Then::ImportFailed(path) => {
                if index == 1 {
                    self.import_path(path, false);
                }
            }
            Then::Spaces(plan) => {
                if index == 0 {
                    self.spaces_replaced(plan);
                } else {
                    self.new_script_save(plan);
                }
            }
            Then::InvalidFolder(plan) => self.new_script_save(plan),
            Then::RunPip(run) => {
                if index == 0 {
                    self.deps_run_confirmed(*run);
                }
            }
            Then::VersionChange(container, version) => {
                if index == 0 {
                    self.deps_version_confirmed(container, version);
                } else {
                    self.deps_push();
                }
            }
            Then::OpenNewScript(path) => {
                if index == 0 {
                    self.edit_new_script(&path);
                } else {
                    self.reveal_path(&path);
                }
            }
            Then::PrefsUrlPrompt(_) | Then::PrefsFailedUrl { .. } | Then::PrefsLocationNotSet | Then::PrefsOverwrite | Then::PrefsQuit | Then::PrefsCopyNow => self.prefs_answered(then, index),
            Then::SlowScan => {
                let go_on = index == 1;
                if let Some(scanner) = &self.scripts_scan {
                    scanner.answer_slow(go_on);
                }
                if !go_on {
                    let ask = Ask {
                        heading: crate::scripts::DISABLED_HEADING.to_string(),
                        text: crate::scripts::DISABLED_TEXT.to_string(),
                        buttons: vec![crate::api_auth::OK.to_string()],
                        silence: None,
                        then: Then::Nothing,
                    };
                    self.ask_scripts(ask);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_keys_follow_iterm2() {
        assert_eq!(silence_keys("NoSyncX"), ("NoSyncX".to_string(), "NoSyncX_selection".to_string()));
    }

    #[test]
    fn show_in_explorer_is_never_remembered() {
        let open = Then::OpenNewScript(std::path::PathBuf::from(r"C:\S\a.py"));
        assert!(open.remembers(0));
        assert!(!open.remembers(1));
        assert!(Then::Nothing.remembers(1));
        // "Lose Changes" is never remembered (`RP:1688`).
        assert!(Then::PrefsQuit.remembers(0));
        assert!(!Then::PrefsQuit.remembers(1));
    }

    #[test]
    fn a_full_queue_gives_the_ask_back() {
        let ask = |then| Ask { heading: String::new(), text: String::new(), buttons: Vec::new(), silence: None, then };
        let mut q = VecDeque::new();
        for _ in 0..LIMIT {
            assert!(push_capped(&mut q, ask(Then::Nothing)).is_none());
        }
        assert_eq!(push_capped(&mut q, ask(Then::SlowScan)).map(|a| a.then), Some(Then::SlowScan));
        assert_eq!(q.len(), LIMIT);
    }
}

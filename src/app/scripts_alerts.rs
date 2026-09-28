//! The alert queue for iTerm2's `iTermWarning`s shown as windows, one at a time (first the
//! Scripts features' downloads to confirm, the slow-scan question and the notices after them).
//! A silenceable warning remembers its answer under iTerm2's keys and is answered at once while
//! silenced (`warnings`, `warnings_app`); "Always show alerts with remembered selections" shows
//! it instead, with the saved answer explained and "Permanently Forget Saved Selection".

use std::collections::VecDeque;

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::script_runtime_ui::After;
use super::App;
use crate::prompt_ui::{ConfirmResult, ConfirmWindow};
use crate::script_provision::Job;
use crate::warnings::{self, Kind, Preempt, Spec};

/// One alert waiting to show: the warning (its buttons left to right; Enter presses the first
/// and Escape the one labeled "Cancel") and what an answer does.
pub(super) struct Ask {
    pub spec: Spec,
    pub then: Then,
    /// Shown because "Always show alerts with remembered selections" is on: the saved button's
    /// label (iTerm2's `savedSelectionLabel`); the text explains it and "Permanently Forget
    /// Saved Selection" follows the buttons.
    remembered: Option<String>,
}

/// An alert: iTerm2's heading, text, button labels, identifier (None when it is never
/// silenced) and kind. The button labeled "Cancel" is never remembered, nor what `then` never
/// remembers.
pub(super) fn ask(heading: &str, text: &str, labels: &[&str], id: Option<&str>, kind: Kind, then: Then) -> Ask {
    let spec = Spec {
        id: id.map(str::to_string),
        kind,
        heading: heading.to_string(),
        text: text.to_string(),
        labels: labels.iter().map(|l| l.to_string()).collect(),
        cancel: labels.iter().position(|l| *l == crate::api_auth::CANCEL),
        never_remember: then.never_remember(),
    };
    Ask { spec, then, remembered: None }
}

/// How a closed alert answers (ruling R13x67-1): a button, or closed with none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Answer {
    Button(usize),
    // 13x6's permission alert is the first to close with no button.
    #[allow(dead_code)]
    Closed,
}

/// The answer when an alert is closed: `fallback` (the window's `close_answer`, which only
/// dismisses), except that closing the slow-scan question goes on scanning rather than
/// pressing Stop. iTerm2's alerts cannot be closed.
pub(super) fn on_close(then: &Then, fallback: usize) -> Answer {
    match then {
        Then::SlowScan => Answer::Button(1),
        _ => Answer::Button(fallback),
    }
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
    /// "Problem Exporting" after Export All Settings and Data failed: OK (0).
    ExportFailed,
    /// "Importing Settings and Data" for the archive picked: OK (0) stages it and restarts
    /// Tandem; Cancel (1) does nothing.
    ImportConfirm(std::path::PathBuf),
    /// "Erase All Settings and Data": Export First (0), Skip Export (1) or Cancel (2).
    EraseExportFirst,
    /// "Erase Everything?" or "Dry-Run Erase?": Cancel (0) or the action (1).
    EraseConfirm { dry_run: bool },
    /// A paste's question for the pane: the button runs what the paste's bar did (Paste, Paste
    /// Without Newline, Convert tabs to spaces, Advanced..., Cancel); dropped, the paste stops.
    Paste(crate::layout::PaneId),
    /// Closing tmux tabs or a window with them: the pane the question is about (its window is
    /// the one closed) and the question's identifier (`CLOSE_TAB`, `tmux_persist::close_win_id`);
    /// Hide, Kill, Detach tmux Session or Cancel.
    TmuxClose(crate::layout::PaneId, String),
    /// Turning broadcast input on, the menu selector: OK (0) turns it on; Cancel (1) does
    /// nothing.
    BroadcastInput(&'static str),
}

impl Then {
    /// The buttons never remembered when the alert is silenced (`doNotRememberLabels`): Show in
    /// Explorer after a new script, Lose Changes at quit (`RP:1688`), a malformed script's
    /// Reveal and a found archive's Cancel and Move to Trash (`iTermScriptsMenuController.m:567,964`).
    pub(super) fn never_remember(&self) -> Vec<usize> {
        match self {
            Then::OpenNewScript(_) | Then::PrefsQuit | Then::Malformed(_) => vec![1],
            Then::Archive(_) => vec![1, 2],
            _ => Vec::new(),
        }
    }
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
    /// Queues an alert; a silenced one answers its remembered choice at once (and is listed in
    /// Suppressed Alerts), or shows with it explained in the remembered mode.
    pub(super) fn ask_scripts(&mut self, mut ask: Ask) {
        match self.warning_preempt(&ask.spec) {
            Preempt::Answer(index) => return self.scripts_answered(ask.then, index),
            Preempt::Show => {}
            Preempt::ShowRemembered { saved_label } => ask.remembered = Some(saved_label),
        }
        if let Some(dropped) = push_capped(&mut self.scripts_alerts, ask) {
            self.scripts_dropped(dropped.then);
        }
    }

    /// An alert closed with no button (`Answer::Closed`): nothing is remembered or recorded, and
    /// the ask counts as dropped.
    pub(super) fn scripts_closed(&mut self, then: Then) {
        self.scripts_dropped(then);
    }

    /// An ask that will never show: the slow-scan question answers Continue, so the scan
    /// worker does not wait for ever; any other is dropped.
    fn scripts_dropped(&mut self, then: Then) {
        match then {
            Then::SlowScan => self.scripts_answered(then, 1),
            // Canceled: what waits on a consent or on the replace question hears so.
            Then::Consent(jobs) => self.consent_declined(jobs),
            Then::AlreadyInstalled(_) => self.scripts_answered(then, 1),
            Then::Paste(pane) => self.paste_answered(pane, None),
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
        let spec = &ask.spec;
        let labels: Vec<&str> = spec.labels.iter().map(String::as_str).collect();
        let heading = if spec.heading.is_empty() { warnings::DEFAULT_HEADING } else { spec.heading.as_str() };
        let text = match &ask.remembered {
            Some(saved) => warnings::remembered_text(&spec.text, saved),
            None => spec.text.clone(),
        };
        let (checkbox, shortcuts) = (warnings::checkbox_label(spec), warnings::shortcuts(&spec.labels));
        let underline = self.globals.bool("AlertsIndicateShortcuts");
        match ConfirmWindow::actions(event_loop, instance, adapter, device, queue, heading, &text, &labels, checkbox, &shortcuts, underline, ask.remembered.is_some()) {
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
            WindowEvent::CloseRequested => {
                let fallback = match w.close_answer() {
                    ConfirmResult::Action { index, .. } => index,
                    _ => 0,
                };
                match on_close(&ask.then, fallback) {
                    Answer::Button(index) => ConfirmResult::Action { index, suppress: false },
                    Answer::Closed => {
                        if let Some((_, ask)) = self.scripts_alert.take() {
                            self.scripts_closed(ask.then);
                        }
                        return true;
                    }
                }
            }
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
        let Some((_, mut ask)) = self.scripts_alert.take() else { return true };
        // "Permanently Forget Saved Selection": the saved answer goes and the same alert shows
        // again, next, without the remembered mode (`iTermWarning.m:526-541`).
        if ask.remembered.is_some() && index == ask.spec.labels.len() {
            if let Some(id) = ask.spec.id.as_deref() {
                warnings::clear_saved(&mut self.globals, id);
                self.save_globals();
            }
            ask.remembered = None;
            self.scripts_alerts.push_front(ask);
            return true;
        }
        self.warning_answered(&ask.spec, index, suppress);
        self.scripts_answered(ask.then, index);
        true
    }

    /// Acts on an answer: the button's place in the alert's buttons.
    pub(super) fn scripts_answered(&mut self, then: Then, index: usize) {
        match then {
            Then::Nothing | Then::ErrorRunning | Then::ExportFailed => {}
            Then::Paste(pane) => self.paste_answered(pane, Some(index)),
            Then::TmuxClose(anchor, id) => self.tmux_close_answered(anchor, &id, index),
            Then::BroadcastInput(selector) => {
                if index == 0 {
                    self.broadcast_run(selector, true);
                }
            }
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
            Then::ImportConfirm(file) => {
                if index == 0 {
                    self.import_confirmed(file);
                }
            }
            Then::EraseExportFirst => self.erase_export_answered(index),
            Then::EraseConfirm { dry_run } => {
                if index == 1 {
                    self.erase_confirmed(dry_run);
                }
            }
            Then::PrefsUrlPrompt(_) | Then::PrefsFailedUrl { .. } | Then::PrefsLocationNotSet | Then::PrefsOverwrite | Then::PrefsQuit | Then::PrefsCopyNow => self.prefs_answered(then, index),
            Then::SlowScan => {
                let go_on = index == 1;
                if let Some(scanner) = &self.scripts_scan {
                    scanner.answer_slow(go_on);
                }
                if !go_on {
                    let ask = ask(crate::scripts::DISABLED_HEADING, crate::scripts::DISABLED_TEXT, &[crate::api_auth::OK], None, Kind::Persistent, Then::Nothing);
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
    fn asks_follow_iterm2() {
        use crate::warnings::{selection_key, Kind};
        let a = ask("H", "T", &["OK", "Cancel"], Some("NoSyncX"), Kind::Permanent, Then::Nothing);
        assert_eq!(a.spec.cancel, Some(1));
        assert_eq!(a.spec.labels, ["OK", "Cancel"]);
        assert_eq!(a.spec.id.as_deref(), Some("NoSyncX"));
        assert_eq!((a.spec.heading.as_str(), a.spec.text.as_str(), a.spec.kind), ("H", "T", Kind::Permanent));
        assert!(a.spec.never_remember.is_empty());
        let a = ask("H", "T", &["OK", "Reveal"], None, Kind::Persistent, Then::Nothing);
        assert_eq!(a.spec.cancel, None);
        assert!(a.spec.never_remember.is_empty());
        assert_eq!(a.spec.id, None);
        // 11d's OpenNewScript rule: "Show in Explorer" is never remembered.
        let open = ask("H", "T", &["OK", "Show in Explorer"], Some("NoSyncX"), Kind::Permanent, Then::OpenNewScript(std::path::PathBuf::from(r"C:\S\a.py")));
        assert_eq!(open.spec.never_remember, [1]);
        assert!(open.spec.rememberable(0));
        assert!(!open.spec.rememberable(1));
        // "Lose Changes" is never remembered (`RP:1688`).
        assert_eq!(Then::PrefsQuit.never_remember(), [1]);
        assert!(Then::Nothing.never_remember().is_empty());
        assert_eq!(selection_key("NoSyncX"), "NoSyncX_selection");
    }

    #[test]
    fn close_result_follows_iterm2() {
        assert_eq!(on_close(&Then::SlowScan, 0), Answer::Button(1));
        assert_eq!(on_close(&Then::Nothing, 1), Answer::Button(1));
        assert_eq!(on_close(&Then::Archive(std::path::PathBuf::from(r"C:\a.zip")), 0), Answer::Button(0));
        assert_ne!(Answer::Closed, Answer::Button(0));
    }

    #[test]
    fn a_full_queue_gives_the_ask_back() {
        let ask = |then| ask("", "", &[], None, crate::warnings::Kind::Persistent, then);
        let mut q = VecDeque::new();
        for _ in 0..LIMIT {
            assert!(push_capped(&mut q, ask(Then::Nothing)).is_none());
        }
        assert_eq!(push_capped(&mut q, ask(Then::SlowScan)).map(|a| a.then), Some(Then::SlowScan));
        assert_eq!(q.len(), LIMIT);
    }
}

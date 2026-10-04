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
use crate::prompt_ui::ConfirmResult;
use crate::script_provision::Job;
use crate::warnings::{Kind, Preempt, Spec};

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
    Ask::new(spec, then)
}

impl Ask {
    /// An alert for a warning built in full: for the object API's warnings, whose "Cancel" is
    /// not the cancel button (restart session).
    pub(super) fn new(spec: Spec, then: Then) -> Ask {
        Ask { spec, then, remembered: None }
    }
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
/// pressing Stop, and closing a consent question with no Cancel button (a terminal-initiated
/// download or display, the large file question, copying settings to the custom folder,
/// overwriting settings that changed since launch) refuses it: its first button consents.
/// The one exception is the quit question, whose close copies (`fallback`): losing the
/// person's settings is worse. iTerm2's alerts cannot be closed, and neither of the quit
/// question's buttons has Escape.
pub(super) fn on_close(then: &Then, fallback: usize) -> Answer {
    match then {
        Then::SlowScan => Answer::Button(1),
        // Keep Both: a closed window never replaces a file (P9).
        Then::SshKeepOrReplace { .. } => Answer::Button(0),
        // Cancel: a closed window never uploads the person's files (P9).
        Then::SshUpload { .. } => Answer::Button(1),
        // No, Deny, Don't Copy and Discard Local Changes.
        Then::Download(..) | Then::BigDownload(..) | Then::Inline { .. } | Then::BigInline(..) | Then::PrefsCopyNow | Then::PrefsOverwrite => Answer::Button(1),
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
    /// Turning broadcast input on: the pane the question was asked from (its window and tab
    /// are the ones turned on) and the menu selector; OK (0) turns it on; Cancel (1) does
    /// nothing.
    BroadcastInput(crate::layout::PaneId, &'static str),
    /// "Restart session?" for the pane: OK (0) restarts it; Cancel (1) does nothing.
    Restart(crate::layout::PaneId),
    /// A terminal-initiated download from the pane, and whether its question was silenced
    /// when asked: Yes (0) saves it, after the large file question when that follows; No (1)
    /// drops it.
    Download(crate::layout::PaneId, crate::inline_file::Download, bool),
    /// "Allow Large File Download?" for the pane's download: Allow (0) saves it; Deny (1)
    /// drops it.
    BigDownload(crate::layout::PaneId, crate::inline_file::Download),
    /// "Allow Terminal-Initiated Display?" for the inline file the pane's output waits on (its
    /// number, `Screen::inline_serial`), its name and declared size, and whether the question
    /// was silenced when asked: Yes (0) shows it, after the large file question when that
    /// follows; No (1) drops it.
    Inline { pane: crate::layout::PaneId, serial: u64, name: String, size: usize, was_silenced: bool },
    /// "Allow Large File Download?" for a waiting inline file (pane, number): Allow (0) shows
    /// it; Deny (1) drops it.
    BigInline(crate::layout::PaneId, u64),
    /// Clear Buffer (`clear`) or Reset with broadcast, asked from `me` with the broadcast
    /// sessions `targets`: Clear All (0), Current Session Only (1) or Cancel (2).
    Broadcast { clear: bool, me: crate::layout::PaneId, targets: Vec<crate::layout::PaneId> },
    /// New Tab or New Window (`window`) from the tmux pane `anchor`: New tmux Tab or Window
    /// (0), Use Default Profile (1) or Cancel (2). `beside`: New Tab to the Right's tab (its
    /// id), which Use Default Profile's tab opens after.
    TmuxNew { anchor: crate::layout::PaneId, window: bool, beside: Option<u64> },
    /// The group close commands' "Close Tabs" for the tabs `ids` of `window`: Close (0) or
    /// Cancel (1).
    CloseTabs { window: winit::window::WindowId, ids: Vec<u64> },
    /// Upgrade's "It looks like you're not at a command prompt." for the pane: Run Installer
    /// Anyway (0) runs the installer there; Cancel (1) does nothing.
    InstallShellIntegration(crate::layout::PaneId),
    /// "Overwrite existing file?" for the finished SSH download `job` whose plain name is
    /// taken: Keep Both (0) or Replace (1, never remembered). Never dropped by the queue's cap
    /// (it holds a finished temp file and a job); closed or dropped it keeps both (P9).
    SshKeepOrReplace { job: u64 },
    /// "OK to copy ... to ...?" for an Alt-drop on `pane`: OK (0) uploads each file to the remote
    /// directory `to`; Cancel (1) does nothing. A closed window cancels and a dropped ask is
    /// dropped (P9: a close never sends local files). Never capped.
    SshUpload { pane: crate::layout::PaneId, files: Vec<std::path::PathBuf>, to: crate::ssh_files::ScpPath },
}

impl Then {
    /// The buttons never remembered when the alert is silenced (`doNotRememberLabels`): Show in
    /// Explorer after a new script, Lose Changes at quit (`RP:1688`), a malformed script's
    /// Reveal and a found archive's Cancel and Move to Trash (`iTermScriptsMenuController.m:567,964`).
    pub(super) fn never_remember(&self) -> Vec<usize> {
        match self {
            Then::OpenNewScript(_) | Then::PrefsQuit | Then::Malformed(_) => vec![1],
            Then::Archive(_) => vec![1, 2],
            // Replace would overwrite for every later file of that name.
            Then::SshKeepOrReplace { .. } => vec![1],
            _ => Vec::new(),
        }
    }

    /// An ask that output, a script or a folder scan raises, not the person's own action: only
    /// these count against the queue's cap, so a flood of them never drops a question the
    /// person asked for (a paste, Restart Session, Clear with broadcast, the quit question).
    fn capped(&self) -> bool {
        matches!(self, Then::Download(..) | Then::BigDownload(..) | Then::Inline { .. } | Then::BigInline(..) | Then::Nothing | Then::ErrorRunning | Then::Malformed(_) | Then::Archive(_))
    }
}

/// The queue holds at most this many capped asks (`Then::capped`); later ones are dropped (a
/// flood of the same question must not open windows forever). The person's own asks always
/// queue.
const LIMIT: usize = 16;

/// Whether an ask that `about` picks is showing or queued.
pub(super) fn waits(showing: Option<&Ask>, queued: &VecDeque<Ask>, about: impl Fn(&Then) -> bool) -> bool {
    queued.iter().any(|a| about(&a.then)) || showing.is_some_and(|a| about(&a.then))
}

/// Queues `ask`, or gives a capped one back when the queue holds `LIMIT` capped asks.
fn push_capped(queue: &mut VecDeque<Ask>, ask: Ask) -> Option<Ask> {
    if ask.then.capped() && queue.iter().filter(|a| a.then.capped()).count() >= LIMIT {
        return Some(ask);
    }
    queue.push_back(ask);
    None
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
            // The finished download keeps both files.
            Then::SshKeepOrReplace { .. } => self.scripts_answered(then, 0),
            Then::Paste(pane) => self.paste_answered(pane, None),
            // The pane's output waits on the answer: dropped, the file is not shown.
            Then::Inline { pane, serial, .. } | Then::BigInline(pane, serial) => self.inline_released(pane, serial, false),
            _ => {}
        }
    }

    /// Whether an ask that `about` picks is queued or showing.
    pub(super) fn alert_waits(&self, about: impl Fn(&Then) -> bool) -> bool {
        waits(self.scripts_alert.as_ref().map(|(_, a)| a), &self.scripts_alerts, about)
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
        let underline = self.globals.bool("AlertsIndicateShortcuts");
        match super::warnings_app::open_warning(event_loop, (instance, adapter, device, queue), &ask.spec, ask.remembered.as_deref(), underline) {
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
        if self.warning_window_answered(&ask.spec, ask.remembered.is_some(), index, suppress) {
            ask.remembered = None;
            self.scripts_alerts.push_front(ask);
            return true;
        }
        self.scripts_answered(ask.then, index);
        true
    }

    /// Acts on an answer: the button's place in the alert's buttons.
    pub(super) fn scripts_answered(&mut self, then: Then, index: usize) {
        match then {
            Then::Nothing | Then::ErrorRunning | Then::ExportFailed => {}
            Then::Paste(pane) => self.paste_answered(pane, Some(index)),
            Then::TmuxClose(anchor, id) => self.tmux_close_answered(anchor, &id, index),
            Then::BroadcastInput(anchor, selector) => {
                if index == 0 {
                    self.broadcast_granted(anchor, selector);
                }
            }
            Then::Restart(pane) => {
                if index == 0 {
                    self.restart_answered(pane);
                }
            }
            Then::Download(pane, d, was_silenced) => self.download_answered(pane, d, was_silenced, index),
            Then::Inline { pane, serial, name, size, was_silenced } => self.inline_answered(pane, serial, &name, size, was_silenced, index),
            Then::BigInline(pane, serial) => self.inline_released(pane, serial, index == 0),
            Then::BigDownload(_, d) => {
                if index == 0 {
                    self.save_download(d);
                }
            }
            Then::Broadcast { clear, me, targets } => self.clear_or_reset_answered(clear, me, super::broadcast_ui::broadcast_targets(index, me, targets)),
            Then::TmuxNew { anchor, window, beside } => self.tmux_new_answered(anchor, window, beside, index),
            Then::CloseTabs { window, ids } => self.close_tabs_answered(window, ids, index),
            Then::InstallShellIntegration(pane) => {
                if index == 0 {
                    self.install_shell_integration_in(pane);
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
            Then::SshKeepOrReplace { job } => self.ssh_keep_or_replace(job, index),
            Then::SshUpload { pane, files, to } => {
                if index == 0 {
                    self.ssh_upload_confirmed(pane, files, to);
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

    /// The broadcast input warning carries the pane it was asked from and the menu item, so
    /// the answer acts in that window and tab.
    #[test]
    fn broadcast_input_ask_carries_its_anchor() {
        let a = super::super::broadcast_ui::broadcast_input_ask("t", 7, "enableSendInputToAllPanes:");
        assert_eq!(a.then, Then::BroadcastInput(7, "enableSendInputToAllPanes:"));
    }

    #[test]
    fn close_result_follows_iterm2() {
        assert_eq!(on_close(&Then::SlowScan, 0), Answer::Button(1));
        assert_eq!(on_close(&Then::Nothing, 1), Answer::Button(1));
        assert_eq!(on_close(&Then::Archive(std::path::PathBuf::from(r"C:\a.zip")), 0), Answer::Button(0));
        assert_ne!(Answer::Closed, Answer::Button(0));
    }

    /// Closing a consent question with no Cancel button refuses it: its first button (Yes,
    /// Allow, Copy) would save a file the terminal sent, show one, or copy the settings.
    #[test]
    fn closing_a_consent_question_refuses_it() {
        use crate::prompt_ui::close_index;
        let d = || crate::inline_file::Download { name: "a.txt".into(), bytes: b"abc".to_vec() };
        let download = ask("", "", &["Yes", "No"], None, Kind::Permanent, Then::Download(3, d(), false));
        let big = ask("", "", &["Allow", "Deny"], None, Kind::Permanent, Then::BigDownload(3, d()));
        let inline = ask("", "", &["Yes", "No"], None, Kind::Permanent, Then::Inline { pane: 3, serial: 1, name: "a.png".into(), size: 5, was_silenced: false });
        let big_inline = ask("", "", &["Allow", "Deny"], None, Kind::Permanent, Then::BigInline(3, 1));
        let copy = ask("", "", &[crate::remote_prefs::COPY, crate::remote_prefs::DONT_COPY], None, Kind::Persistent, Then::PrefsCopyNow);
        // Discard Local Changes leaves the changed settings file alone.
        let overwrite = super::super::prefs_ui::overwrite_ask("C:\\s");
        for a in [download, big, inline, big_inline, copy, overwrite] {
            let labels: Vec<&str> = a.spec.labels.iter().map(String::as_str).collect();
            // The window's own close answer is the first button, which consents.
            assert_eq!(close_index(&labels), 0);
            assert_eq!(on_close(&a.then, close_index(&labels)), Answer::Button(1), "{:?}", a.spec.labels);
        }
    }

    /// The one exception: closing the quit question copies, since Lose Changes loses the
    /// person's settings.
    #[test]
    fn closing_the_quit_question_copies() {
        let labels = [crate::remote_prefs::COPY, crate::remote_prefs::LOSE_CHANGES];
        assert_eq!(on_close(&Then::PrefsQuit, crate::prompt_ui::close_index(&labels)), Answer::Button(0));
    }

    #[test]
    fn a_full_queue_gives_the_ask_back() {
        let ask = |then| ask("", "", &[], None, crate::warnings::Kind::Persistent, then);
        let mut q = VecDeque::new();
        for _ in 0..LIMIT {
            assert!(push_capped(&mut q, ask(Then::Nothing)).is_none());
        }
        assert_eq!(push_capped(&mut q, ask(Then::Nothing)).map(|a| a.then), Some(Then::Nothing));
        assert_eq!(q.len(), LIMIT);
    }

    /// Output-driven asks fill only their own share: the person's own questions still queue
    /// behind a flood of downloads.
    #[test]
    fn the_persons_questions_are_never_dropped() {
        let ask = |then| ask("", "", &[], None, crate::warnings::Kind::Persistent, then);
        let d = |pane| Then::Download(pane, crate::inline_file::Download { name: "a".into(), bytes: Vec::new() }, false);
        let mut q = VecDeque::new();
        for pane in 0..LIMIT as u64 {
            assert!(push_capped(&mut q, ask(d(pane))).is_none());
        }
        assert!(push_capped(&mut q, ask(d(99))).is_some(), "one download too many");
        for then in [Then::Paste(1), Then::Restart(1), Then::PrefsQuit, Then::Broadcast { clear: true, me: 1, targets: vec![2] }, Then::TmuxNew { anchor: 1, window: false, beside: Some(3) }, Then::SlowScan, Then::CloseTabs { window: winit::window::WindowId::from(1), ids: vec![2] }] {
            assert!(push_capped(&mut q, ask(then.clone())).is_none(), "{then:?}");
        }
        assert_eq!(q.len(), LIMIT + 7);
        assert!(push_capped(&mut q, ask(Then::Archive(std::path::PathBuf::from(r"C:\a.zip")))).is_some(), "a scan's asks are capped");
    }

    /// A finished SSH download's Keep Both or Replace question (P9, R14de-6): the queue's cap
    /// never drops it, a closed window or a dropped ask keeps both files, and Replace is never remembered.
    #[test]
    fn ssh_keep_or_replace_is_never_dropped() {
        use crate::warnings::Kind;
        let a = ask("H", "T", &["Keep Both", "Replace"], Some("NoSyncOverwriteOrReplaceFile"), Kind::Permanent, Then::SshKeepOrReplace { job: 7 });
        assert_eq!(a.spec.never_remember, [1]);
        assert!(a.spec.rememberable(0) && !a.spec.rememberable(1), "a remembered Replace would overwrite for every later file");
        assert_eq!(on_close(&a.then, 1), Answer::Button(0), "a closed window keeps both");
        let mut queue: VecDeque<Ask> = (0..LIMIT).map(|_| ask("H", "T", &["OK"], None, Kind::Persistent, Then::Nothing)).collect();
        assert!(push_capped(&mut queue, a).is_none());
        assert_eq!(queue.len(), LIMIT + 1);
        assert_eq!(queue.back().map(|q| &q.then), Some(&Then::SshKeepOrReplace { job: 7 }));
        // The cap still holds for every capped ask.
        assert!(push_capped(&mut queue, ask("H", "T", &["OK"], None, Kind::Persistent, Then::Nothing)).is_some());
    }

    /// An Alt-drop's "OK to copy" question (P9): a closed window is Cancel, never OK; the queue's
    /// cap never drops it; a dropped ask uploads nothing.
    #[test]
    fn ssh_upload_close_is_cancel() {
        use crate::warnings::Kind;
        let to = crate::ssh_files::ScpPath { path: "/home/u".into(), hostname: "h".into(), username: "u".into() };
        let then = Then::SshUpload { pane: 3, files: vec![std::path::PathBuf::from(r"C:\a.txt")], to };
        // The actions window's own close answer is the first button (OK): it must not reach us.
        assert_eq!(on_close(&then, 0), Answer::Button(1));
        assert_eq!(on_close(&then, 1), Answer::Button(1));
        let a = ask("", "OK to copy", &[crate::api_auth::OK, crate::api_auth::CANCEL], None, Kind::Persistent, then);
        assert_eq!(a.spec.cancel, Some(1), "Escape is Cancel");
        let mut queue: VecDeque<Ask> = (0..LIMIT).map(|_| ask("H", "T", &["OK"], None, Kind::Persistent, Then::Nothing)).collect();
        assert!(push_capped(&mut queue, a).is_none());
    }
}

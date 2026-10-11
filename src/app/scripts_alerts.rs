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
        // A site's permission question: a closed window is no button. The close index (Cancel, else
        // the first button) would be Allow (R13x67-1).
        Then::BrowserPermission(_) => Answer::Closed,
        // Sparkle's question: a closed window answers neither (nothing is stored, it asks again).
        Then::UpdatePermission | Then::TipPermission => Answer::Closed,
        // A closed error alert is no answer: nothing is revealed (P21).
        Then::DynamicProfilesError(_) => Answer::Closed,
        // Neither: a close never changes the person's locale.
        Then::LocaleMismatch { .. } => Answer::Closed,
        // A closed window never starts a log or makes a folder.
        Then::LogExists { .. } | Then::LogStyle { .. } => Answer::Closed,
        Then::CreateLogFolder { .. } => Answer::Button(1),
        // Keep Both: a closed window never replaces a file (P9).
        Then::SshKeepOrReplace { .. } => Answer::Button(0),
        // Cancel: a closed window never uploads the person's files (P9).
        Then::SshUpload { .. } | Then::BindScheme { .. } | Then::OpenProfiles(..) => Answer::Button(1),
        // Cancel: a closed window never runs an opened script or adds a preset (B6).
        Then::RunOpenedFile(_) | Then::AddDuplicateColorPreset(_) | Then::TriggerImportMarked(_) => Answer::Button(1),
        // Cancel: a closed window opens no link and downloads nothing.
        Then::OpenLink { .. } | Then::BrowserUpsell { .. } => Answer::Button(2),
        // Cancel (the third button): a closed window neither downloads nor opens a tab for a remote file.
        Then::SshViewOrDownload { .. } => Answer::Button(2),
        // Cancel: a closed window runs nothing and opens nothing a page's text named.
        Then::BrowserRun { .. } | Then::BrowserOpenFile { .. } => Answer::Button(1),
        // Cancel: a closed window clears nothing (Clear All Data from the settings page).
        Then::BrowserClearData { .. } => Answer::Button(1),
        // Cancel: a closed window approves nothing.
        Then::BrowserExtension { .. } => Answer::Button(1),
        // Cancel: a closed window deletes no bookmark.
        Then::BookmarkDelete { .. } => Answer::Button(1),
        // Cancel is the third button: a closed window never opens a profile either way (B6).
        Then::OpenRenamedProfile { .. } => Answer::Button(2),
        // Cancel: a closed window never uninstalls or goes on past a failed removal (P10).
        Then::ClaudeUninstall | Then::ClaudeHooksFailed => Answer::Button(1),
        // Leave Enabled: a closed window never turns the person's API off (P10).
        Then::ClaudeDisableApi => Answer::Button(0),
        // Not Now: a closed window never rewrites settings.json (P10).
        Then::ClaudeRepair => Answer::Button(1),
        // Cancel: a closed window never turns session cookies on.
        Then::ClaudeCookie => Answer::Button(1),
        // Keep It: a closed window never removes a workgroup the person edited.
        Then::ClaudeEditedWorkgroup => Answer::Button(0),
        // Cancel (the first button, the default): a closed window never starts a workgroup's commands.
        Then::EnterWorkgroup { .. } | Then::RestoreWorkgroup { .. } | Then::UsageReport { .. } => Answer::Button(0),
        // No, Deny, Don't Copy and Discard Local Changes.
        Then::Download(..) | Then::BigDownload(..) | Then::Inline { .. } | Then::BigInline(..) | Then::PrefsCopyNow | Then::PrefsOverwrite | Then::SessionHotkey { .. } => Answer::Button(1),
        _ => Answer::Button(fallback),
    }
}

/// What an answer does.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Then {
    Nothing,
    /// "Register a hotkey?" for a session hotkey a program supplied: Register (0) turns it on;
    /// Ignore (1) leaves it stored and inert. Register is never remembered.
    SessionHotkey { pane: crate::layout::PaneId, shortcut: crate::hotkey::Shortcut },
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
    /// The short-lived session warning: OK only. A checked box was remembered by the queue under
    /// the saved profile's key (the Settings row reads the same key); nothing more to do.
    ShortLived,
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
    /// "Shell Integration Setting Conflict": Turn On OSC 7 (0) sets Accept OSC 7, unless the
    /// question was silenced when it was raised (a remembered answer never flips it).
    TurnOnOsc7 { was_silenced: bool },
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
    /// "Warning" before Exclude from taskbar turns on: Exclude (0) turns it on and saves; Cancel (1) leaves it off.
    ExcludeFromTaskbar,
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
    /// "Allow Printing?" for the pane's escape-sequence print job: Allow (0), Disable
    /// Temporarily (1) or Disable Permanently (2); dropped, nothing prints.
    PrintGuard { pane: crate::layout::PaneId, job: crate::screen::PrintJob },
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
    Restart(crate::layout::PaneId, crate::session_end::AskedAbout),
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
    /// "Download or View File?" for the remote file `to` of `pane`'s host (`CD:1630-1675`): Download (0),
    /// View (1) opens a browser tab at the file's `iterm2-ssh:` URL, Cancel (2). Only View is
    /// remembered (a remembered Download would repeat a failed download without asking). A closed
    /// window cancels. Not capped: the person's own click.
    SshViewOrDownload { pane: crate::layout::PaneId, to: crate::ssh_files::ScpPath, mime: String },
    /// "Uninstall Claude Code Integration?": Uninstall (0) strips the hook; Cancel (1) does
    /// nothing. A closed window cancels (P10).
    ClaudeUninstall,
    /// "Couldn't Remove Hooks": Continue (0) goes on and clears the integration's flags; Cancel
    /// (1) stops. A closed window cancels (P10).
    ClaudeHooksFailed,
    /// Uninstall found a "Claude Code" workgroup that is not the one the installer adds.
    ClaudeEditedWorkgroup,
    /// "Disable the Python API?" after an uninstall: Leave Enabled (0) or Disable (1). A closed
    /// window leaves it enabled (P10).
    ClaudeDisableApi,
    /// "Claude Code Integration Looks Broken": Reinstall (0) opens the setup; Not Now (1)
    /// does nothing. A closed window is Not Now, and Reinstall is never remembered (P10).
    ClaudeRepair,
    /// "Enable Session Cookies?": Enable (0) turns them on; Cancel (1) leaves them as they are.
    /// A closed window cancels.
    ClaudeCookie,
    /// "Tandem is not the default handler for ssh. ...": OK (0) registers Tandem for the user
    /// and binds the scheme to the profile `guid` once Windows reports Tandem as the handler;
    /// Cancel (1) does nothing. A closed window cancels: a close never changes Windows.
    BindScheme { guid: String, scheme: String },
    /// "Open N sessions?" (P6): OK (0) opens the Profiles window's profiles as the way chosen;
    /// Cancel (1) does nothing. A closed window cancels.
    OpenProfiles(Vec<String>, crate::settings_ui::How, crate::settings_ui::Opener),
    /// "Wrong Encoding Detected" for the custom `locale` of profile `guid` under a UTF-8 profile:
    /// Change Locale (0) asks for another, Keep This Locale (1) keeps it, storing it when
    /// `from_settings` (a pick made in Settings). A closed window does neither.
    LocaleMismatch { guid: String, locale: String, from_settings: bool },
    /// "Logging Folder Not Found" for a local folder: OK (0) makes it and starts the log; Cancel
    /// (1) does nothing.
    CreateLogFolder { pane: crate::layout::PaneId, path: std::path::PathBuf, append: bool, style: crate::session_log::Style },
    /// "\"name\" already exists. Do you want to replace it or append to it?": Cancel (0), Replace
    /// (1) or Append (2), then the style question.
    LogExists { pane: crate::layout::PaneId, path: std::path::PathBuf },
    /// "Logging Style": Raw data (0), Plain text (1), HTML (2) or ASCIInema (3) starts the log.
    LogStyle { pane: crate::layout::PaneId, path: std::path::PathBuf, append: bool },
    /// "Session Logging Problem": OK only.
    LogProblem,
    /// "Debug Logging Enabled": OK starts the log (iTerm2 runs the alert modally, then starts), and
    /// so does a closed window. The person's own action, so never capped (P10).
    DebugLogStart,
    /// An alert the person asked for whose answer changes nothing ("Debug Logging Stopped", "Problem
    /// Saving Debug Log"): not capped, so a flood of capped asks never drops the one naming the file (P10).
    Acknowledged,
    /// "Check for updates automatically?": Check Automatically (0) or Don't Check (1). Never
    /// silenced; closed, nothing is stored.
    UpdatePermission,
    /// "See Tips of the Day?": Yes (0) or No (1). Closed, nothing is stored.
    TipPermission,
    /// "Dynamic Profiles Error": OK (0), View Log (1) shows the Dynamic Profiles console entry, and
    /// Reveal in Explorer (2, only with the file) selects the file of the first error.
    DynamicProfilesError(Option<std::path::PathBuf>),
    /// "OK to run ...?" for a script opened with Tandem (Task 2): OK (0) opens the session that
    /// runs it; Cancel (1) does nothing. A closed window cancels; another process raises it, so
    /// it is capped (B6).
    RunOpenedFile(crate::open_file::Launch),
    /// "Add duplicate color preset?" for an opened `.itermcolors`: Cancel (0) or Add it anyway (1).
    AddDuplicateColorPreset(std::path::PathBuf),
    /// "Profile Renamed" for an opened `.itermtab` (Task 6): the first button opens the profile by
    /// its guid (the one now named {new}), the second the profile that has the old name, Cancel
    /// nothing (`PD:44-61`, a faithful quirk). A closed window cancels; another process can raise it,
    /// so it is capped (B6).
    OpenRenamedProfile { by_guid: String, by_name: String },
    /// "This file was downloaded. Its triggers can run commands." for an opened `.it2triggers`
    /// (B1, 13v1 P21): OK (0) goes on to the profile chooser, Cancel (1) imports nothing. A closed
    /// window cancels; another process raises it, so it is capped.
    TriggerImportMarked(Vec<toml::Table>),
    /// "Open in Tandem?" for a web link a person clicked (`WS:384-410`): Use Default Browser (0)
    /// opens it with the system, Open in Tandem (1) in a browser pane, Cancel nothing. The person's
    /// own click, so it is not capped. `file` is the file wording; no route raises it today.
    OpenLink { url: String, style: crate::browser::Style, background: bool, pane: Option<crate::layout::PaneId>, file: bool },
    /// "WebView2 Runtime Required" (`GW:136-160`): Download (0) opens Microsoft's page, Use System
    /// Browser (1) opens the link, Cancel nothing. Not capped.
    BrowserUpsell { url: String },
    /// "Enter the workgroup ...?" for an imported workgroup, a trigger, or more than eight members
    /// (13y1): Cancel (0, the default) nothing, Enter (1, `ASK_ENTER`) enters the very workgroup the
    /// question showed. A trigger raises it, so then it is capped.
    EnterWorkgroup { workgroup: Box<crate::workgroup::Workgroup>, pane: crate::layout::PaneId, how: crate::workgroup_run::Mechanism, approved: crate::workgroup_run::Approved },
    /// A restored workgroup that is not the person's own, or whose commands changed since they were
    /// answered for (Task 9): `token` names the waiting restore.
    RestoreWorkgroup { token: u64, workgroup: Box<crate::workgroup::Workgroup>, approved: crate::workgroup_run::Approved },
    /// The AI Usage item's report alert (13y1 Task 5): Cancel (0, the default), Report Bug (1) copies
    /// the report and opens the issue form.
    UsageReport { title: String, text: String },
    /// "Run command?" for a Smart Selection action from a page (`PSB:300-317`): OK (0) queues the
    /// command, built already by the one cmd.exe builder, to run in a single-use window; Cancel (1)
    /// nothing. The person's own pick, so not capped.
    BrowserRun { pane: crate::layout::PaneId, command: String },
    /// "Open file?" for a Smart Selection action from a page (`PSB:335-346`): OK (0) opens it as
    /// `open_file_run` plans (a program is revealed, never run); Cancel (1) nothing.
    BrowserOpenFile { pane: crate::layout::PaneId, file: String },
    /// "Clear all website data?" for the settings page's Clear All Data (it also erases bookmarks and
    /// history): Clear (0) clears; Cancel (1) nothing. A page's button raised it, so the window is
    /// input-guarded and it is never remembered.
    BrowserClearData { pane: crate::layout::PaneId },
    /// "Enable Browser Extension?" (13x8 Task 10): Enable (0) records the approval of this folder's
    /// manifest (`hash`) for the pane's saved profile; Cancel (1) does nothing. Never remembered.
    BrowserExtension { pane: crate::layout::PaneId, folder: std::path::PathBuf, hash: String },
    /// "Are you sure you want to delete this bookmark?" from Edit Bookmark: Delete (0) removes it;
    /// Cancel (1) nothing. The person's own window raised it.
    BookmarkDelete { pane: crate::layout::PaneId, url: String },
    /// "Allow Notifications" (and the other permission questions) for the open ask `id`
    /// (`browser_perms_app::Asks`): Allow (0) or Block (1). A closed window, or an ask that never
    /// shows, blocks the request and saves nothing. A page raises it, so it is capped.
    BrowserPermission(u64),
}

impl Then {
    /// The buttons never remembered when the alert is silenced (`doNotRememberLabels`): Show in
    /// Explorer after a new script, Lose Changes at quit (`RP:1688`), a malformed script's
    /// Reveal and a found archive's Cancel and Move to Trash (`iTermScriptsMenuController.m:567,964`).
    pub(super) fn never_remember(&self) -> Vec<usize> {
        match self {
            Then::OpenNewScript(_) | Then::PrefsQuit | Then::Malformed(_) => vec![1],
            // View Log, and Reveal in Explorer when the file is offered: navigation, never remembered.
            Then::DynamicProfilesError(file) => if file.is_some() { vec![1, 2] } else { vec![1] },
            Then::Archive(_) => vec![1, 2],
            // Replace would overwrite for every later file of that name.
            Then::SshKeepOrReplace { .. } => vec![1],
            Then::SshViewOrDownload { .. } => vec![0, 2],
            // A remembered Reinstall would rewrite the person's settings.json unasked.
            Then::ClaudeRepair => vec![0],
            // A remembered Register would let every later program claim a system-wide hotkey.
            Then::SessionHotkey { .. } => vec![0],
            _ => Vec::new(),
        }
    }

    /// An ask that output, a script or a folder scan raises, not the person's own action: only
    /// these count against the queue's cap, so a flood of them never drops a question the
    /// person asked for (a paste, Restart Session, Clear with broadcast, the quit question).
    fn capped(&self) -> bool {
        matches!(self, Then::Download(..) | Then::BigDownload(..) | Then::Inline { .. } | Then::BigInline(..) | Then::SessionHotkey { .. } | Then::UpdatePermission | Then::TipPermission | Then::DynamicProfilesError(_) | Then::Nothing | Then::ErrorRunning | Then::Malformed(_) | Then::Archive(_) | Then::RunOpenedFile(_) | Then::AddDuplicateColorPreset(_) | Then::OpenRenamedProfile { .. } | Then::TriggerImportMarked(_) | Then::BrowserPermission(_))
            || matches!(self, Then::EnterWorkgroup { how: crate::workgroup_run::Mechanism::Trigger | crate::workgroup_run::Mechanism::BrowserTrigger, .. })
    }
}

/// How many ignored files a question that never shows stands for (the opened-file questions).
pub(super) fn dropped_open_files(then: &Then) -> usize {
    matches!(then, Then::RunOpenedFile(_) | Then::AddDuplicateColorPreset(_) | Then::OpenRenamedProfile { .. } | Then::TriggerImportMarked(_)) as usize
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
        // `scripts_dropped` blocks a `BrowserPermission` (nothing granted, nothing saved).
        self.scripts_dropped(then);
    }

    /// An ask that will never show: the slow-scan question answers Continue, so the scan
    /// worker does not wait for ever; any other is dropped.
    fn scripts_dropped(&mut self, then: Then) {
        let files = dropped_open_files(&then);
        if files > 0 {
            return self.open_files_ignored(files);
        }
        match then {
            Then::SlowScan => self.scripts_answered(then, 1),
            // Canceled: what waits on a consent or on the replace question hears so.
            Then::Consent(jobs) => self.consent_declined(jobs),
            Then::AlreadyInstalled(_) => self.scripts_answered(then, 1),
            // The finished download keeps both files.
            Then::SshKeepOrReplace { .. } => self.scripts_answered(then, 0),
            Then::Paste(pane) => self.paste_answered(pane, None),
            // The question could not open: a restored workgroup is not rebuilt unasked.
            Then::RestoreWorkgroup { token, workgroup, approved } => self.restore_answered(token, *workgroup, approved, false),
            Then::DynamicProfilesError(_) => self.dynamic_errors_pending = 0,
            // The request is blocked and nothing is saved: a WebView2 request never keeps its
            // deferral, and a closed window never grants.
            Then::BrowserPermission(id) => self.browser_permission_answered(id, None),
            // Clear All Data could not ask (the queue refused it, or the window never opened): that is a
            // Cancel, and the settings page's pending question is released (no dead button).
            Then::BrowserClearData { pane } => self.browser_clear_data_answered(pane, 1),
            Then::BrowserExtension { pane, folder, hash } => self.browser_extension_answered(pane, folder, hash, 1),
            Then::BookmarkDelete { pane, url } => self.bookmark_delete_answered(pane, &url, 1),
            // The pane's output waits on the answer: dropped, the file is not shown.
            Then::Inline { pane, serial, .. } | Then::BigInline(pane, serial) => self.inline_released(pane, serial, false),
            _ => {}
        }
    }

    /// Drops the queued asks that `about` picks (one already showing is left; its answer is stale).
    pub(super) fn alert_drop_queued(&mut self, about: impl Fn(&Then) -> bool) {
        self.scripts_alerts.retain(|a| !about(&a.then));
    }

    /// Whether an ask that `about` picks is queued or showing.
    pub(super) fn alert_waits(&self, about: impl Fn(&Then) -> bool) -> bool {
        waits(self.scripts_alert.as_ref().map(|(_, a)| a), &self.scripts_alerts, about)
    }

    /// Queues the not-default question for a scheme the profiles' menu chose.
    pub(super) fn ask_scheme(&mut self, a: crate::settings_ui::SchemeAsk) {
        let ask = ask(&a.text, &a.info, &[crate::api_auth::OK, crate::api_auth::CANCEL], None, Kind::Persistent, Then::BindScheme { guid: a.guid, scheme: a.scheme });
        self.ask_scripts(ask);
    }

    /// The question's OK: Windows is changed only here, only under HKCU, through the registry
    /// seam; the scheme is bound only when Tandem then is the handler (else Tandem's text).
    fn bind_scheme_confirmed(&mut self, guid: &str, scheme: &str) {
        if self.store.get(guid).is_none() {
            return self.banners.set("url-schemes", "That profile no longer exists, so nothing was registered.");
        }
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(e) => return self.banners.set("url-schemes", format!("Could not find Tandem's own path to register it for {scheme} links: {e}")),
        };
        let mut reg = crate::url_schemes::system();
        let mut table = self.globals.table(crate::url_schemes::HANDLERS_KEY);
        match crate::url_schemes::register_and_bind(&mut table, reg.as_mut(), scheme, guid, &exe) {
            Ok(()) => {
                self.globals.set(crate::url_schemes::HANDLERS_KEY, toml::Value::Table(table));
                self.save_globals();
            }
            Err(e) => {
                // Windows keeps the person's own choice (a UserChoice a program cannot set): Tandem's
                // page in Default apps is where they pick Tandem.
                let opened = e == crate::url_schemes::protected(scheme) && super::semantic_ui::shell_open(crate::url_schemes::DEFAULT_APPS_URI, false).is_err();
                self.banners.set("url-schemes", if opened { format!("{e} Windows could not open Default apps; open it from Settings.") } else { e })
            }
        }
    }

    /// Opens the next queued alert when none shows.
    pub(super) fn open_scripts_alerts(&mut self, event_loop: &ActiveEventLoop) {
        if self.scripts_alert.is_some() || self.scripts_alerts.is_empty() {
            return;
        }
        // A page's question whose pane was hidden meanwhile does not open: it parks and is asked
        // again when the pane is shown (final review M4).
        if let Some(Then::BrowserPermission(id)) = self.scripts_alerts.front().map(|a| &a.then) {
            let id = *id;
            if !self.browser_permission_may_open(id) {
                self.scripts_alerts.pop_front();
                return;
            }
        }
        // After the last window has gone, the settings folder's quit question opens with the
        // GPU that window used.
        let gpu = self.gpu.as_ref().map(|g| (&g.instance, &g.adapter, &g.device, &g.queue)).or(self.prefs_gpu.as_ref().map(|(i, a, d, q)| (i, a, d, q)));
        let Some((instance, adapter, device, queue)) = gpu else { return };
        let Some(ask) = self.scripts_alerts.pop_front() else { return };
        let underline = self.globals.bool("AlertsIndicateShortcuts");
        match super::warnings_app::open_warning(event_loop, (instance, adapter, device, queue), &ask.spec, ask.remembered.as_deref(), underline) {
            Ok(mut w) => {
                // The entry question's Enter waits until the whole body has been shown (13y1 review).
                if matches!(ask.then, Then::EnterWorkgroup { .. } | Then::RestoreWorkgroup { .. }) {
                    w.require_read(crate::workgroup_run::ASK_ENTER);
                }
                // A page raised it: no default button, input ignored for a moment after it appears
                // and after it regains focus (13x6 B1).
                if matches!(ask.then, Then::BrowserPermission(_) | Then::BrowserClearData { .. } | Then::BrowserExtension { .. }) {
                    w.guard_input();
                }
                // Enable waits until every permission has been shown (the list can be long).
                if matches!(ask.then, Then::BrowserExtension { .. }) {
                    w.require_read(0);
                }
                self.scripts_alert = Some((w, ask));
            }
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
            Then::SessionHotkey { pane, shortcut } => self.session_hotkey_answered(pane, shortcut, index),
            Then::UpdatePermission => self.update_permission_answered(index),
            Then::TipPermission => self.tip_permission_answered(index),
            Then::DynamicProfilesError(file) => {
                self.dynamic_errors_pending = 0;
                match (index, file) {
                    (1, _) => self.reveal_script_entry(crate::script_history::DYNAMIC_PROFILES_ID),
                    (2, Some(path)) => self.reveal_in_explorer(&path.display().to_string()),
                    _ => {}
                }
            }
            Then::BrowserPermission(id) => self.browser_permission_answered(id, Some(index)),
            Then::OpenLink { url, style, background, pane, file } => self.open_link_answered(&url, style, background, pane, file, index),
            Then::BrowserUpsell { url } => self.upsell_answered(&url, index),
            Then::BrowserRun { pane, command } => {
                if index == 0 {
                    self.browser_run_confirmed(pane, command);
                }
            }
            Then::BrowserClearData { pane } => self.browser_clear_data_answered(pane, index),
            Then::BrowserExtension { pane, folder, hash } => self.browser_extension_answered(pane, folder, hash, index),
            Then::BookmarkDelete { pane, url } => self.bookmark_delete_answered(pane, &url, index),
            Then::BrowserOpenFile { pane, file } => {
                if index == 0 {
                    self.browser_open_file_confirmed(pane, &file);
                }
            }
            Then::DebugLogStart => self.debug_log_start_answered(),
            Then::Nothing | Then::ErrorRunning | Then::ExportFailed | Then::ShortLived | Then::LogProblem | Then::Acknowledged => {}
            Then::CreateLogFolder { pane, path, append, style } => self.log_folder_answered(pane, path, append, style, index),
            Then::LogExists { pane, path } => self.log_exists_answered(pane, path, index),
            Then::LogStyle { pane, path, append } => self.log_style_answered(pane, path, append, index),
            Then::TurnOnOsc7 { was_silenced } => self.osc7_answered(was_silenced, index),
            Then::Paste(pane) => self.paste_answered(pane, Some(index)),
            Then::PrintGuard { pane, job } => self.print_guard_answered(pane, job, index),
            Then::TmuxClose(anchor, id) => self.tmux_close_answered(anchor, &id, index),
            Then::BroadcastInput(anchor, selector) => {
                if index == 0 {
                    self.broadcast_granted(anchor, selector);
                }
            }
            Then::Restart(pane, about) => {
                if index == 0 {
                    self.restart_answered(pane, &about);
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
            Then::ExcludeFromTaskbar => {
                if index == 0 {
                    self.exclude_from_taskbar_confirmed();
                }
            }
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
            Then::SshViewOrDownload { pane, to, .. } => self.ssh_view_answered(pane, &to, index),
            Then::LocaleMismatch { guid, locale, from_settings } => self.locale_mismatch_answered(guid, locale, from_settings, index),
            Then::OpenProfiles(guids, how, from) => {
                if index == 0 {
                    self.open_selected_now(guids, how, from);
                }
            }
            Then::BindScheme { guid, scheme } => {
                if index == 0 {
                    self.bind_scheme_confirmed(&guid, &scheme);
                }
            }
            Then::SshUpload { pane, files, to } => {
                if index == 0 {
                    self.ssh_upload_confirmed(pane, files, to);
                }
            }
            Then::ClaudeUninstall | Then::ClaudeHooksFailed | Then::ClaudeDisableApi | Then::ClaudeRepair | Then::ClaudeCookie | Then::ClaudeEditedWorkgroup => self.claude_answered(&then, index),
            Then::RunOpenedFile(launch) => {
                if index == 0 {
                    self.open_runs.push(launch);
                }
            }
            Then::AddDuplicateColorPreset(path) => {
                if index == 1 {
                    self.open_recheck(path);
                }
            }
            Then::OpenRenamedProfile { by_guid, by_name } => self.renamed_profile_answered(&by_guid, &by_name, index),
            Then::TriggerImportMarked(imported) => {
                if index == 0 {
                    self.trigger_import_queue(imported);
                }
            }
            Then::UsageReport { title, text } => {
                if index == 1 {
                    self.usage_report_answered(&title, &text);
                }
            }
            Then::EnterWorkgroup { workgroup, pane, how, approved } => {
                if index == crate::workgroup_run::ASK_ENTER {
                    self.enter_workgroup_now(*workgroup, pane, how, approved);
                } else if matches!(how, crate::workgroup_run::Mechanism::Trigger | crate::workgroup_run::Mechanism::BrowserTrigger) {
                    // Cancel: this trigger does not ask again in this session (not every 5 s).
                    self.workgroups.restoring.decline(pane, &workgroup.unique_identifier);
                }
            }
            Then::RestoreWorkgroup { token, workgroup, approved } => self.restore_answered(token, *workgroup, approved, index == crate::workgroup_run::ASK_ENTER),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The entry question's default (Return) is Cancel; Return, Escape and a closed window all cancel,
    /// and only the second button, by click or access key, enters.
    #[test]
    fn the_entry_question_defaults_to_cancel() {
        use crate::workgroup_run::{ASK_ENTER, ASK_LABELS};
        let then = || Then::EnterWorkgroup { workgroup: Box::new(crate::workgroup::Workgroup::new_empty("x")), pane: 1, how: crate::workgroup_run::Mechanism::Trigger, approved: Default::default() };
        let a = ask("H", "T", &ASK_LABELS, None, crate::warnings::Kind::Persistent, then());
        assert_eq!(a.spec.labels[0], crate::api_auth::CANCEL, "Return presses the first button");
        assert_eq!(a.spec.cancel, Some(0), "Escape presses it too");
        assert_eq!(a.spec.labels[ASK_ENTER], "Enter");
        assert_eq!(on_close(&then(), 7), Answer::Button(0), "a closed window cancels");
        assert!(include_str!("scripts_alerts.rs").contains("if index == crate::workgroup_run::ASK_ENTER {"));
        assert!(include_str!("scripts_alerts.rs").contains("w.require_read(crate::workgroup_run::ASK_ENTER);"), "Enter waits for the whole body");
    }

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

    /// Closing the Exclude warning is Cancel: the setting stays off.
    #[test]
    fn closing_the_exclude_warning_cancels() {
        let a = ask(crate::presence::EXCLUDE_HEADING, crate::presence::EXCLUDE_WARNING, &[crate::presence::EXCLUDE_ACTION, crate::presence::CANCEL], None, Kind::Persistent, Then::ExcludeFromTaskbar);
        let labels: Vec<&str> = a.spec.labels.iter().map(String::as_str).collect();
        assert_eq!(on_close(&a.then, crate::prompt_ui::close_index(&labels)), Answer::Button(1));
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
        for then in [Then::Paste(1), Then::Restart(1, Default::default()), Then::PrefsQuit, Then::Broadcast { clear: true, me: 1, targets: vec![2] }, Then::TmuxNew { anchor: 1, window: false, beside: Some(3) }, Then::SlowScan, Then::CloseTabs { window: winit::window::WindowId::from(1), ids: vec![2] }] {
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

    /// The four Claude Code questions (P10): closing one never takes the destructive or
    /// file-writing choice. The window's own close answer is the first button, which would
    /// uninstall, go on, disable and reinstall.
    #[test]
    fn closing_the_uninstall_question_never_uninstalls() {
        assert_eq!(on_close(&Then::ClaudeUninstall, 0), Answer::Button(1), "Cancel, not Uninstall");
    }

    #[test]
    fn closing_couldnt_remove_hooks_cancels() {
        assert_eq!(on_close(&Then::ClaudeHooksFailed, 0), Answer::Button(1), "Cancel, not Continue");
    }

    #[test]
    fn closing_the_edited_workgroup_question_keeps_it() {
        assert_eq!(on_close(&Then::ClaudeEditedWorkgroup, 1), Answer::Button(0), "Keep It, not Remove");
    }

    #[test]
    fn closing_disable_the_api_leaves_it_enabled() {
        assert_eq!(on_close(&Then::ClaudeDisableApi, 1), Answer::Button(0), "Leave Enabled, not Disable");
        assert_eq!(on_close(&Then::ClaudeDisableApi, 0), Answer::Button(0));
    }

    #[test]
    fn closing_the_cookie_question_cancels() {
        assert_eq!(on_close(&Then::ClaudeCookie, 0), Answer::Button(1), "Cancel, not Enable");
    }

    #[test]
    fn closing_the_repair_prompt_is_not_now_and_reinstall_is_never_remembered() {
        assert_eq!(on_close(&Then::ClaudeRepair, 0), Answer::Button(1), "Not Now, not Reinstall");
        let a = ask("H", "T", &["Reinstall", "Not Now"], Some(crate::claude_code::REPAIR_ID), Kind::Permanent, Then::ClaudeRepair);
        assert_eq!(a.spec.never_remember, [0]);
        assert!(!a.spec.rememberable(0) && a.spec.rememberable(1));
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
        // The not-default question: a closed window is Cancel, never OK, so nothing is registered.
        let bind = Then::BindScheme { guid: "G".into(), scheme: "ssh".into() };
        assert_eq!(on_close(&bind, 0), Answer::Button(1));
        assert_eq!(on_close(&bind, 1), Answer::Button(1));
        let a = ask("", "OK to copy", &[crate::api_auth::OK, crate::api_auth::CANCEL], None, Kind::Persistent, then);
        assert_eq!(a.spec.cancel, Some(1), "Escape is Cancel");
        let mut queue: VecDeque<Ask> = (0..LIMIT).map(|_| ask("H", "T", &["OK"], None, Kind::Persistent, Then::Nothing)).collect();
        assert!(push_capped(&mut queue, a).is_none());
    }

    /// The wrong-encoding warning (13p Task 3): a closed window changes nothing; its spec is
    /// iTerm2's, permanently silenceable under the profile's own identifier.
    #[test]
    fn locale_mismatch_warning_is_iterm2s() {
        let then = Then::LocaleMismatch { guid: "G".into(), locale: "C".into(), from_settings: true };
        assert_eq!(on_close(&then, 0), Answer::Closed);
        let a = ask(crate::locale_guess::MISMATCH_HEADING, crate::locale_guess::MISMATCH_TEXT, &[crate::locale_guess::CHANGE_LOCALE, crate::locale_guess::KEEP_LOCALE], Some(&crate::locale_guess::mismatch_id("G")), crate::warnings::Kind::Permanent, then);
        assert_eq!(a.spec.id.as_deref(), Some("NoSyncUTF8Mismatch_G"));
        assert_eq!(a.spec.cancel, None);
        assert!(a.spec.never_remember.is_empty());
        assert!(!a.then.capped());
    }

    /// B6: the questions another process can raise close as Cancel and count against the cap.
    #[test]
    fn closing_the_run_question_cancels() {
        let launch = crate::open_file::Launch { guid: "G".into(), dir: None, text: Some("x".into()), style: crate::open_file::Style::Tab, new_window: false };
        for then in [Then::RunOpenedFile(launch.clone()), Then::AddDuplicateColorPreset(std::path::PathBuf::from(r"C:.itermcolors"))] {
            assert_eq!(on_close(&then, 0), Answer::Button(1), "{then:?}");
            assert!(then.capped(), "{then:?}");
        }
    }

    #[test]
    fn a_flood_of_open_requests_is_capped() {
        let launch = crate::open_file::Launch { guid: "G".into(), dir: None, text: Some("x".into()), style: crate::open_file::Style::Tab, new_window: false };
        let mut queue = VecDeque::new();
        let mut dropped = 0;
        for _ in 0..40 {
            let a = ask("", "OK to run?", &["OK", "Cancel"], None, Kind::Persistent, Then::RunOpenedFile(launch.clone()));
            if push_capped(&mut queue, a).is_some() {
                dropped += 1;
            }
        }
        assert_eq!((queue.len(), dropped), (LIMIT, 40 - LIMIT));
    }

    /// Review fix 2: an "OK to run" (or duplicate-preset question) past the queue's cap is counted
    /// into the one "N files ignored" banner, never dropped silently; a closed one is a Cancel and
    /// is not counted.
    #[test]
    fn a_capped_open_question_is_counted_not_dropped_silently() {
        let launch = crate::open_file::Launch { guid: "G".into(), dir: None, text: Some("x".into()), style: crate::open_file::Style::Tab, new_window: false };
        let mut queue = VecDeque::new();
        let mut tally = crate::open_file::IgnoredCount::default();
        let t0 = std::time::Instant::now();
        let mut banner = String::new();
        for i in 0..(LIMIT + 3) {
            let then = if i % 2 == 0 { Then::RunOpenedFile(launch.clone()) } else { Then::AddDuplicateColorPreset(std::path::PathBuf::from(r"C:\a.itermcolors")) };
            if let Some(over) = push_capped(&mut queue, ask("", "x", &["OK", "Cancel"], None, Kind::Persistent, then)) {
                assert_eq!(dropped_open_files(&over.then), 1);
                banner = crate::open_file::ignored_banner(tally.add(t0, 1));
            }
        }
        assert_eq!(banner, "3 files ignored");
        assert_eq!(dropped_open_files(&Then::Nothing), 0);
        assert_eq!(dropped_open_files(&Then::SlowScan), 0);
        let src = include_str!("scripts_alerts.rs");
        let body = &src[src.find("fn scripts_dropped").unwrap()..];
        assert!(body.contains("dropped_open_files(&then)"), "scripts_dropped counts them");
    }

    #[test]
    fn closed_alert_is_no_button() {
        assert_eq!(on_close(&Then::BrowserPermission(3), 0), Answer::Closed);
        assert_eq!(on_close(&Then::BrowserPermission(3), 1), Answer::Closed, "the close index never leaks through");
        assert_eq!(on_close(&Then::SlowScan, 0), Answer::Button(1));
        assert_eq!(on_close(&Then::Nothing, 1), Answer::Button(1));
        assert_eq!(crate::prompt_ui::close_index(&["Allow", "Block"]), 0, "unchanged: the permission alert must not use it");
        assert!(Then::BrowserPermission(1).capped(), "a page cannot queue unlimited windows (B2)");
        assert!(Then::BrowserPermission(1).never_remember().is_empty());
        let src = include_str!("scripts_alerts.rs");
        assert!(src.contains("Then::BrowserPermission(id) => self.browser_permission_answered(id, None)"), "dropped and closed block");
        assert!(src.contains("w.guard_input()"), "the window is guarded");
    }
}

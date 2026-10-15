//! The Downloads and Uploads menus (`AppKit/iTermApplicationDelegate.m:724-755`,
//! `FileTransfer/FileTransferManager.m`): every terminal download, secure copy and RequestUpload
//! the app has started, listed in a top-level menu of its own with Clear All and, per transfer,
//! Stop, Show in Finder, Remove from List, Open and Get Info. The model is `crate::transfers`.

use muda::{IsMenuItem, MenuItem, PredefinedMenuItem, Submenu};

use super::*;
use crate::menus;
use crate::transfers::{self as tr, Act, Dir, Transfer};

/// How an OSC 1337 download ended.
pub(super) enum End {
    Saved(PathBuf),
    Failed(String),
    Cancelled,
}

fn slot(dir: Dir) -> usize {
    match dir {
        Dir::Download => 0,
        Dir::Upload => 1,
    }
}

/// The text of a menu title or item on Windows: Mac words mapped, "&" kept literal.
fn shown(label: &str) -> String {
    menus::shown_label(label).replace('&', "&&")
}

/// What the aborted-download bar is called, and what it asks (`NaggingController.m:329-340`).
pub(super) const ASK_ABORT_DOWNLOAD: &str = "AbortDownloadOnKeyPressAnnouncement";
pub(super) const ABORT_DOWNLOAD_TEXT: &str = "A file is being downloaded. Abort the download?";
/// `PTYSession.m:17006`, shown as a warning bar with no buttons.
pub(super) const ASK_OVERSIZE: &str = "FileTransferAbortedOversize";
pub(super) const OVERSIZE_TEXT: &str = "A file transfer was aborted for exceeding its declared size.";

/// `PTYSession.m:20644-20657`: another upload cannot start while one is going.
pub(super) const UPLOAD_BUSY_TITLE: &str = "Upload in Progress";
pub(super) const UPLOAD_BUSY_TEXT: &str = "Please wait for the current upload to complete or cancel it before starting another.";

impl App {
    /// "Upload in Progress": an OK-only alert (iTerm2 raises it for upload-and-paste; Termurion has the
    /// one upload a RequestUpload sends, so a second request while it runs meets it).
    pub(super) fn upload_in_progress_alert(&mut self) {
        let a = super::scripts_alerts::ask(UPLOAD_BUSY_TITLE, UPLOAD_BUSY_TEXT, &[crate::api_auth::OK], None, crate::warnings::Kind::Persistent, super::scripts_alerts::Then::Nothing);
        self.ask_scripts(a);
    }

    /// A transfer starts: it is listed, and its menu is added to the bar if it was not yet.
    pub(super) fn transfer_begin(&mut self, t: Transfer) -> u64 {
        let id = self.transfers.start(t);
        self.refresh_transfer_menus();
        id
    }

    /// An OSC 1337 download is over (it arrives whole, so its transfer is listed when it ends).
    pub(super) fn transfer_terminal_ended(&mut self, d: &crate::inline_file::Download, path: Option<PathBuf>, end: End) {
        let id = self.transfers.start(tr::terminal_download(Some(&d.name), path));
        self.transfers.progress(id, d.bytes.len() as u64, Some(d.bytes.len() as u64));
        match end {
            End::Saved(path) => self.transfers.finish(id, Some(path)),
            End::Failed(why) => self.transfers.fail(id, &why),
            End::Cancelled => self.transfers.cancelled(id),
        }
        self.refresh_transfer_menus();
    }

    /// A secure copy job began (its `Local` is in `ssh_locals`).
    pub(super) fn transfer_job_began(&mut self, job: u64) {
        let Some(l) = self.ssh_locals.get(&job) else { return };
        let dir = if l.upload { Dir::Upload } else { Dir::Download };
        let t = tr::secure_copy(dir, l.user.as_deref().unwrap_or(""), &l.host, &l.remote, tr::Source::Ssh(job));
        let t = Transfer { size: l.total, ..t };
        self.transfer_begin(t);
    }

    /// Bytes moved on a secure copy job, or its size became known.
    pub(super) fn transfer_job_progress(&mut self, job: u64) {
        let Some(l) = self.ssh_locals.get(&job) else { return };
        let (bytes, total) = (l.written, l.total);
        let Some(id) = self.transfers.find_source(tr::Source::Ssh(job)) else { return };
        self.transfers.progress(id, bytes, total);
        self.refresh_transfer_menus();
    }

    /// Whether the person stopped this job: it is Cancelling, so its end is no failure.
    pub(super) fn transfer_stopping(&self, job: u64) -> bool {
        self.transfers.find_source(tr::Source::Ssh(job)).and_then(|id| self.transfers.get(id)).is_some_and(|t| t.status == tr::Status::Cancelling)
    }

    /// A secure copy job ended: where a download arrived, or why it failed. A job the person
    /// stopped ends Cancelled.
    pub(super) fn transfer_job_ended(&mut self, job: u64, result: Result<Option<PathBuf>, String>) {
        let Some(id) = self.transfers.find_source(tr::Source::Ssh(job)) else { return };
        match result {
            Ok(path) => self.transfers.finish(id, path),
            Err(why) => self.transfers.fail(id, &why),
        }
        self.refresh_transfer_menus();
    }

    /// A RequestUpload began sending to `pane`.
    pub(super) fn transfer_upload_began(&mut self, pane: PaneId, label: &str, size: usize) -> u64 {
        self.transfer_begin(tr::pane_upload(label, pane, size as u64))
    }

    /// A RequestUpload's bytes went out.
    pub(super) fn transfer_upload_progress(&mut self, id: u64, sent: usize, shown_step: bool) {
        self.transfers.progress(id, sent as u64, None);
        if shown_step {
            self.refresh_transfer_menus();
        }
    }

    pub(super) fn transfer_upload_finished(&mut self, id: u64) {
        self.transfers.finish(id, None);
        self.refresh_transfer_menus();
    }

    pub(super) fn transfer_upload_cancelled(&mut self, id: u64) {
        self.transfers.cancelled(id);
        self.refresh_transfer_menus();
    }

    /// Builds each menu that has had a transfer and keeps its items current, in every window's
    /// bar. iTerm2 inserts the menu before Help (`AD:724-755`) and drops finished items after a day
    /// (`cleanUpMenus`; here the prune runs when the menu is refreshed).
    pub(super) fn refresh_transfer_menus(&mut self) {
        self.transfers.prune(Instant::now());
        for dir in [Dir::Download, Dir::Upload] {
            if !self.transfers.menu_shown(dir) {
                continue;
            }
            let rows = self.transfers.rows(dir);
            let (k, title) = (slot(dir), if dir == Dir::Download { tr::DOWNLOADS } else { tr::UPLOADS });
            for bar in self.menu_bars_mut() {
                if bar.transfer_menus[k].is_some() && bar.transfer_rows[k] == rows {
                    continue;
                }
                let sub = match bar.transfer_menus[k].clone() {
                    Some(s) => s,
                    None => {
                        let s = Submenu::new(shown(title), true);
                        let at = bar.menu.items().len().saturating_sub(1);
                        let _ = bar.menu.insert(&s, at);
                        bar.transfer_menus[k] = Some(s.clone());
                        s
                    }
                };
                while sub.remove_at(0).is_some() {}
                let _ = sub.append(&MenuItem::with_id(tr::clear_id(dir), shown(tr::CLEAR_ALL), true, None));
                let _ = sub.append(&PredefinedMenuItem::separator());
                for row in &rows {
                    let items: Vec<MenuItem> = row.subs.iter().map(|s| MenuItem::with_id(tr::item_id(row.id, s.act), shown(s.label), s.enabled, None)).collect();
                    let refs: Vec<&dyn IsMenuItem> = items.iter().map(|i| i as &dyn IsMenuItem).collect();
                    if let Ok(item) = Submenu::with_items(row.title.replace('&', "&&"), true, &refs) {
                        let _ = sub.append(&item);
                    }
                }
                bar.transfer_rows[k] = rows.clone();
            }
        }
    }

    /// A pick from a Downloads or Uploads menu. False for other ids.
    pub(super) fn transfer_picked(&mut self, id: &str) -> bool {
        if let Some(dir) = tr::parse_clear_id(id) {
            self.transfers.clear_all(dir);
            self.refresh_transfer_menus();
            return true;
        }
        let Some((tid, act)) = tr::parse_item_id(id) else { return false };
        let Some(t) = self.transfers.get(tid).cloned() else { return true };
        // A menu item that has gone stale since it was drawn does nothing.
        if !t.enabled(act) {
            return true;
        }
        match act {
            Act::Stop => self.transfer_stop(tid),
            Act::ShowInFinder => {
                if let Some(p) = &t.local_path {
                    self.reveal_path(p);
                }
            }
            Act::RemoveFromList => {
                self.transfers.remove(tid);
                self.refresh_transfer_menus();
            }
            Act::Open => {
                if let Some(p) = t.local_path.as_deref().and_then(crate::localfs::LocalPath::judge) {
                    let _ = crate::localfs::open_with_shell(&p, None, false);
                }
            }
            Act::GetInfo => {
                let a = super::scripts_alerts::ask(tr::SUMMARY_TITLE, &t.info(), &[crate::api_auth::OK], None, crate::warnings::Kind::Persistent, super::scripts_alerts::Then::Nothing);
                self.ask_scripts(a);
            }
        }
        true
    }

    /// Stop (`stop:`): the transfer is Cancelling while its work is ended, then Cancelled.
    fn transfer_stop(&mut self, id: u64) {
        let Some(source) = self.transfers.get(id).map(|t| t.source) else { return };
        if !self.transfers.stop(id) {
            return;
        }
        match source {
            tr::Source::Ssh(job) => {
                // The engine ends the job with this reason; `transfer_job_ended` sees Cancelling and
                // records Cancelled.
                let actions = self.ssh_transfers.fail(job, tr::TRANSFER_CANCELLED);
                self.ssh_transfer_apply(actions);
            }
            tr::Source::Pane(pane) => self.upload_abort(pane),
            tr::Source::None => self.transfers.cancelled(id),
        }
        self.refresh_transfer_menus();
    }

    /// Tells the person once about a file dropped for exceeding its declared size, in the pane it
    /// came to (`PTYSession.m:17005-17020`).
    pub(super) fn tell_oversize(&mut self) {
        // The abort question goes when the file is over (`didFinishDownload`).
        let idle: Vec<PaneId> = self.panes.iter().filter(|(_, p)| !p.own().receiving_file()).map(|(id, _)| *id).collect();
        for pane in idle {
            if let Some(bars) = self.bars.get_mut(&pane).filter(|b| b.has(ASK_ABORT_DOWNLOAD)) {
                bars.retain(|b| b.id != ASK_ABORT_DOWNLOAD);
            }
        }
        let panes: Vec<PaneId> = self.panes.iter_mut().filter_map(|(id, p)| p.own_mut().take_file_aborted().then_some(*id)).collect();
        for pane in panes {
            self.ask(pane, ASK_OVERSIZE, OVERSIZE_TEXT, &[], false);
        }
    }

    /// Ctrl+C while a file is arriving in the focused pane asks whether to abort it
    /// (`PTYSession.m:11536-11539`). The key still goes to the program.
    pub(super) fn download_key(&mut self, ev: &KeyEvent) {
        let Some(pane) = self.focused_id().filter(|id| self.panes.get(id).is_some_and(|p| p.own().receiving_file())) else { return };
        let is_c = matches!(&ev.key_without_modifiers(), winit::keyboard::Key::Character(c) if c.eq_ignore_ascii_case("c"));
        if self.mods.ctrl && is_c {
            self.ask(pane, ASK_ABORT_DOWNLOAD, ABORT_DOWNLOAD_TEXT, &["OK", "Cancel"], true);
        }
    }

    /// The abort bar answered: OK stops the file (`naggingControllerAbortDownload`).
    pub(super) fn abort_download_answered(&mut self, pane: PaneId, button: Option<&str>) {
        if button != Some("OK") {
            return;
        }
        if let Some(p) = self.panes.get_mut(&pane) {
            p.own_mut().abort_file();
        }
        let id = self.transfers.start(tr::terminal_download(None, None));
        self.transfers.cancelled(id);
        self.refresh_transfer_menus();
    }
}

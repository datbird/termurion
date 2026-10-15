//! The Downloads and Uploads menus' model (iTerm2's `FileTransferManager` and
//! `TransferrableFileMenuItemViewController`): every file transfer the app has started, its
//! status, and the items its submenu offers. Pure; the app side is `src/app/transfers_ui.rs`.
//!
//! iTerm2 keeps one controller per transfer and removes it by hand ("Remove from List", "Clear
//! All") or after a day (`cleanUpMenus`, a ten minute timer). Termurion prunes the old ones whenever
//! the menu is rebuilt instead, so no timer runs.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::layout::PaneId;

// Titles, verbatim from `AppKit/iTermApplicationDelegate.m:724-755` (the two menus and Clear All)
// and `FileTransfer/FileTransferManager.m:270-318` (the per-transfer submenu). "Finder" reads
// "Explorer" through `menus::shown_label` where the item is drawn.
pub const DOWNLOADS: &str = "Downloads";
pub const UPLOADS: &str = "Uploads";
pub const CLEAR_ALL: &str = "Clear All";
pub const STOP: &str = "Stop";
pub const SHOW_IN_FINDER: &str = "Show in Finder";
pub const REMOVE_FROM_LIST: &str = "Remove from List";
pub const OPEN: &str = "Open";
pub const GET_INFO: &str = "Get Info";

// The status line of an item (`TransferrableFileMenuItemViewController.m:106-160`).
pub const MSG_STARTING: &str = "Starting\u{2026}";
pub const MSG_DOWNLOADING: &str = "Downloading\u{2026}";
pub const MSG_UPLOADING: &str = "Uploading\u{2026}";
pub const MSG_FINISHED: &str = "Finished";
pub const MSG_FAILED: &str = "Failed";
pub const MSG_CANCELLING: &str = "Cancelling\u{2026}";
pub const MSG_CANCELLED: &str = "Cancelled";

/// Get Info's alert title (`:196-203`).
pub const SUMMARY_TITLE: &str = "File Transfer Summary";
/// What a download with no local path is called (`TerminalFile.m:130-136`).
pub const UNNAMED_FILE: &str = "Unnamed file";
/// A terminal download's subheading when it has no file name (`TerminalFile.m:138-140`).
pub const TERMINAL_DOWNLOAD: &str = "Terminal download";
/// A terminal download whose save dialog was closed (`TerminalFile.m:157-163`).
pub const CANCELED_ERROR: &str = "Canceled.";
/// A transfer the person stopped; the engine's reason for the job it ends.
pub const TRANSFER_CANCELLED: &str = "Transfer cancelled";

/// A finished transfer is dropped from the list after this long (`FileTransferManager.m:14-16`).
pub const KEEP_FINISHED: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Download,
    Upload,
}

/// `TransferrableFileStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Unstarted,
    Starting,
    Transferring,
    FinishedSuccessfully,
    FinishedWithError,
    Cancelling,
    Cancelled,
}

impl Status {
    /// What `removeFromList:` and Clear All accept (`FileTransferManager.m:244-262`,
    /// `TransferrableFileMenuItemViewController.m:64-69`).
    pub fn removable(self) -> bool {
        matches!(self, Status::FinishedSuccessfully | Status::FinishedWithError | Status::Cancelled)
    }

    /// Not over, and not on its way to being: what the old-item prune leaves alone
    /// (`FileTransferManager.m:60-70`).
    fn running(self) -> bool {
        matches!(self, Status::Starting | Status::Transferring | Status::Cancelling)
    }
}

/// What a transfer is made of, so that Stop can end it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// A job of the SSH transfer engine.
    Ssh(u64),
    /// A RequestUpload being typed into a pane.
    Pane(PaneId),
    /// Over already (an OSC 1337 download is saved whole).
    None,
}

#[derive(Clone, Debug)]
pub struct Transfer {
    pub id: u64,
    pub dir: Dir,
    pub source: Source,
    /// `shortName`: the file's name as the list shows it.
    pub short_name: String,
    /// `subheading` (`user@host:path` for secure copy).
    pub subheading: String,
    /// `displayName`, the first lines of Get Info.
    pub display_name: String,
    pub size: Option<u64>,
    pub bytes: u64,
    pub status: Status,
    pub local_path: Option<PathBuf>,
    /// `destination`: the temp file while a secure copy runs, the local path for a terminal download.
    pub destination: Option<String>,
    pub error: String,
    pub changed: Instant,
}

/// The action a submenu item runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Stop,
    ShowInFinder,
    RemoveFromList,
    Open,
    GetInfo,
}

impl Act {
    pub fn key(self) -> &'static str {
        match self {
            Act::Stop => "stop",
            Act::ShowInFinder => "show",
            Act::RemoveFromList => "remove",
            Act::Open => "open",
            Act::GetInfo => "info",
        }
    }

    fn from_key(key: &str) -> Option<Act> {
        [Act::Stop, Act::ShowInFinder, Act::RemoveFromList, Act::Open, Act::GetInfo].into_iter().find(|a| a.key() == key)
    }
}

/// One item of a transfer's submenu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sub {
    pub label: &'static str,
    pub act: Act,
    pub enabled: bool,
}

/// One transfer in a menu: its text and its submenu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: u64,
    pub title: String,
    pub subs: Vec<Sub>,
}

/// Menu ids.
pub const ITEM_ID: &str = "transfer:";
pub const CLEAR_ID: &str = "transfers-clear:";

pub fn item_id(id: u64, act: Act) -> String {
    format!("{ITEM_ID}{id}:{}", act.key())
}

/// A submenu id read back: the transfer and what to do.
pub fn parse_item_id(text: &str) -> Option<(u64, Act)> {
    let (id, key) = text.strip_prefix(ITEM_ID)?.split_once(':')?;
    Some((id.parse().ok()?, Act::from_key(key)?))
}

pub fn clear_id(dir: Dir) -> String {
    format!("{CLEAR_ID}{}", if dir == Dir::Download { "down" } else { "up" })
}

pub fn parse_clear_id(text: &str) -> Option<Dir> {
    match text.strip_prefix(CLEAR_ID)? {
        "down" => Some(Dir::Download),
        "up" => Some(Dir::Upload),
        _ => None,
    }
}

impl Transfer {
    /// `validateMenuItem:` (`TransferrableFileMenuItemViewController.m:50-82`).
    pub fn enabled(&self, act: Act) -> bool {
        match act {
            Act::Stop => matches!(self.status, Status::Starting | Status::Transferring),
            Act::ShowInFinder => self.local_path.is_some() && self.status == Status::FinishedSuccessfully,
            Act::RemoveFromList => self.status.removable(),
            Act::Open => self.status == Status::FinishedSuccessfully,
            Act::GetInfo => true,
        }
    }

    /// The submenu (`FileTransferManager.m:270-318`): Show in Finder and Open only for a download.
    pub fn subs(&self) -> Vec<Sub> {
        let download = self.dir == Dir::Download;
        let mut subs = vec![Sub { label: STOP, act: Act::Stop, enabled: self.enabled(Act::Stop) }];
        if download {
            subs.push(Sub { label: SHOW_IN_FINDER, act: Act::ShowInFinder, enabled: self.enabled(Act::ShowInFinder) });
        }
        subs.push(Sub { label: REMOVE_FROM_LIST, act: Act::RemoveFromList, enabled: self.enabled(Act::RemoveFromList) });
        if download {
            subs.push(Sub { label: OPEN, act: Act::Open, enabled: self.enabled(Act::Open) });
        }
        subs.push(Sub { label: GET_INFO, act: Act::GetInfo, enabled: true });
        subs
    }

    /// `update` (`:106-160`): the item's status message.
    pub fn message(&self) -> &'static str {
        match self.status {
            Status::Unstarted | Status::Starting => MSG_STARTING,
            Status::Transferring if self.dir == Dir::Download => MSG_DOWNLOADING,
            Status::Transferring => MSG_UPLOADING,
            Status::FinishedSuccessfully => MSG_FINISHED,
            Status::FinishedWithError => MSG_FAILED,
            Status::Cancelling => MSG_CANCELLING,
            Status::Cancelled => MSG_CANCELLED,
        }
    }

    /// The text of the item. iTerm2 draws a view (name, subheading, progress bar, message);
    /// a Windows menu item holds one line, so the pieces are joined (Decision).
    pub fn title(&self) -> String {
        let mut text = format!("{} - {}", self.short_name, self.message());
        if let (Status::Transferring, Some(size)) = (self.status, self.size.filter(|s| *s > 0)) {
            text.push_str(&format!(" {}%", (self.bytes.min(size) as u128 * 100 / size as u128) as u64));
        }
        text
    }

    /// `stringForStatus:` (`:162-186`).
    fn status_text(&self) -> String {
        match self.status {
            Status::Unstarted => "Unstarted".to_string(),
            Status::Starting => "Starting".to_string(),
            Status::Transferring => "Transferring".to_string(),
            Status::FinishedSuccessfully => "Finished".to_string(),
            Status::FinishedWithError => format!("Failed with error \u{201c}{}\u{201d}", self.error),
            Status::Cancelling => "Waiting to cancel".to_string(),
            Status::Cancelled => "Canceled by user".to_string(),
        }
    }

    /// Get Info's text (`getInfo:`, `:188-212`): "<name>\n\nStatus: <status>" and a destination or
    /// local path line.
    pub fn info(&self) -> String {
        let extra = if let Some(d) = &self.destination {
            format!("\nDestination: {d}")
        } else if let Some(p) = &self.local_path {
            format!("\nLocal path: {}", p.display())
        } else {
            String::new()
        };
        format!("{}\n\nStatus: {}{}", self.display_name, self.status_text(), extra)
    }
}

/// A terminal download (`TerminalFile.m`): named by its local path once there is one.
pub fn terminal_download(sent_name: Option<&str>, local_path: Option<PathBuf>) -> Transfer {
    let name = local_path.as_ref().and_then(|p| p.file_name()).map_or_else(|| UNNAMED_FILE.to_string(), |n| n.to_string_lossy().into_owned());
    let destination = local_path.as_ref().map(|p| p.display().to_string());
    Transfer {
        id: 0,
        dir: Dir::Download,
        source: Source::None,
        short_name: name.clone(),
        subheading: sent_name.map_or_else(|| TERMINAL_DOWNLOAD.to_string(), str::to_string),
        display_name: name,
        size: None,
        bytes: 0,
        status: Status::Starting,
        local_path,
        destination,
        error: String::new(),
        changed: Instant::now(),
    }
}

/// A secure copy (`SCPFile.m:153-163`): `user@host:path`.
pub fn secure_copy(dir: Dir, user: &str, host: &str, path: &str, source: Source) -> Transfer {
    let short = path.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string();
    Transfer {
        id: 0,
        dir,
        source,
        short_name: short,
        subheading: format!("{user}@{host}:{path}"),
        display_name: format!("Secure copy\nUser name: {user}\nHost: {host}\nFile: {path}"),
        size: None,
        bytes: 0,
        status: Status::Starting,
        local_path: None,
        destination: None,
        error: String::new(),
        changed: Instant::now(),
    }
}

/// A RequestUpload being sent to a pane (`TerminalFileUpload`): named for the files.
pub fn pane_upload(label: &str, pane: PaneId, size: u64) -> Transfer {
    Transfer {
        id: 0,
        dir: Dir::Upload,
        source: Source::Pane(pane),
        short_name: label.to_string(),
        subheading: label.to_string(),
        display_name: label.to_string(),
        size: Some(size),
        bytes: 0,
        status: Status::Starting,
        local_path: None,
        destination: None,
        error: String::new(),
        changed: Instant::now(),
    }
}

#[derive(Default)]
pub struct Transfers {
    items: Vec<Transfer>,
    next: u64,
    /// Whether each menu was ever needed: iTerm2 adds a menu to the bar the first time a transfer
    /// of its kind starts, and it stays (`downloadsMenu`, `uploadsMenu`).
    pub downloads_menu: bool,
    pub uploads_menu: bool,
}

impl Transfers {
    /// A transfer starts (`transferrableFileDidStartTransfer:`): it gets a number and its menu.
    pub fn start(&mut self, mut t: Transfer) -> u64 {
        self.next += 1;
        t.id = self.next;
        match t.dir {
            Dir::Download => self.downloads_menu = true,
            Dir::Upload => self.uploads_menu = true,
        }
        self.items.push(t);
        self.next
    }

    pub fn get(&self, id: u64) -> Option<&Transfer> {
        self.items.iter().find(|t| t.id == id)
    }

    fn get_mut(&mut self, id: u64) -> Option<&mut Transfer> {
        self.items.iter_mut().find(|t| t.id == id)
    }

    pub fn find_source(&self, source: Source) -> Option<u64> {
        self.items.iter().find(|t| t.source == source && !t.status.removable()).map(|t| t.id)
    }

    fn set_status(t: &mut Transfer, status: Status) {
        if t.status != status {
            t.status = status;
            t.changed = Instant::now();
        }
    }

    /// The size became known, or bytes moved (`transferrableFileProgressDidChange:`).
    pub fn progress(&mut self, id: u64, bytes: u64, size: Option<u64>) {
        let Some(t) = self.get_mut(id) else { return };
        if t.status == Status::Cancelling {
            return;
        }
        t.bytes = bytes;
        if size.is_some() {
            t.size = size;
        }
        Self::set_status(t, Status::Transferring);
    }

    /// Finished without an error; `local_path` is where a download arrived.
    pub fn finish(&mut self, id: u64, local_path: Option<PathBuf>) {
        let Some(t) = self.get_mut(id) else { return };
        if let Some(p) = local_path {
            t.destination = Some(p.display().to_string());
            t.short_name = p.file_name().map_or(t.short_name.clone(), |n| n.to_string_lossy().into_owned());
            t.local_path = Some(p);
        }
        if let Some(size) = t.size {
            t.bytes = size;
        }
        t.source = Source::None;
        Self::set_status(t, Status::FinishedSuccessfully);
    }

    /// Ended with an error (`didFailWithError:`); a transfer that was being stopped is Cancelled
    /// instead (`transferrableFileDidStopTransfer:`).
    pub fn fail(&mut self, id: u64, error: &str) {
        let Some(t) = self.get_mut(id) else { return };
        t.source = Source::None;
        if t.status == Status::Cancelling {
            Self::set_status(t, Status::Cancelled);
        } else {
            t.error = error.to_string();
            Self::set_status(t, Status::FinishedWithError);
        }
    }

    /// Stop was chosen (`transferrableFileWillStop:`): true when the transfer is running and now
    /// Cancelling, so the caller ends the real work.
    pub fn stop(&mut self, id: u64) -> bool {
        let Some(t) = self.get_mut(id) else { return false };
        if !t.enabled(Act::Stop) {
            return false;
        }
        Self::set_status(t, Status::Cancelling);
        true
    }

    /// The work ended at once (`transferrableFileDidStopTransfer:`).
    pub fn cancelled(&mut self, id: u64) {
        let Some(t) = self.get_mut(id) else { return };
        t.source = Source::None;
        Self::set_status(t, Status::Cancelled);
    }

    /// "Remove from List": only a transfer that is over goes.
    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.items.len();
        self.items.retain(|t| !(t.id == id && t.status.removable()));
        self.items.len() != before
    }

    /// "Clear All" (`removeAllDownloads`, `removeAllUploads`): the finished, failed and cancelled
    /// ones of one menu.
    pub fn clear_all(&mut self, dir: Dir) {
        self.items.retain(|t| !(t.dir == dir && t.status.removable()));
    }

    /// `cleanUpMenus`: finished items older than a day go.
    pub fn prune(&mut self, now: Instant) {
        self.items.retain(|t| t.status.running() || now.saturating_duration_since(t.changed) <= KEEP_FINISHED);
    }

    /// Whether any transfer of this kind is running (not over, not cancelled).
    pub fn active(&self, dir: Dir) -> bool {
        self.items.iter().any(|t| t.dir == dir && t.status.running())
    }

    /// The rows of one menu below "Clear All" and its line, oldest first.
    pub fn rows(&self, dir: Dir) -> Vec<Row> {
        self.items.iter().filter(|t| t.dir == dir).map(|t| Row { id: t.id, title: t.title(), subs: t.subs() }).collect()
    }

    /// Whether a menu has been added to the bar.
    pub fn menu_shown(&self, dir: Dir) -> bool {
        match dir {
            Dir::Download => self.downloads_menu,
            Dir::Upload => self.uploads_menu,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn download() -> Transfer {
        terminal_download(Some("a.txt"), Some(PathBuf::from("C:\\dl\\a.txt")))
    }

    /// `FileTransferManager.m:270-318`: a download's submenu is Stop, Show in Finder, Remove from
    /// List, Open, Get Info; an upload's has no Show in Finder and no Open.
    #[test]
    fn submenus_follow_iterm2() {
        let labels = |t: &Transfer| t.subs().iter().map(|s| s.label).collect::<Vec<_>>();
        assert_eq!(labels(&download()), ["Stop", "Show in Finder", "Remove from List", "Open", "Get Info"]);
        let up = secure_copy(Dir::Upload, "me", "host", "/tmp/a", Source::None);
        assert_eq!(labels(&up), ["Stop", "Remove from List", "Get Info"]);
    }

    /// `TransferrableFileMenuItemViewController.m:50-82`, state by state.
    #[test]
    fn items_enable_by_status() {
        let mut t = download();
        let enabled = |t: &Transfer| t.subs().iter().filter(|s| s.enabled).map(|s| s.label).collect::<Vec<_>>();
        t.status = Status::Starting;
        assert_eq!(enabled(&t), ["Stop", "Get Info"]);
        t.status = Status::Transferring;
        assert_eq!(enabled(&t), ["Stop", "Get Info"]);
        t.status = Status::FinishedSuccessfully;
        assert_eq!(enabled(&t), ["Show in Finder", "Remove from List", "Open", "Get Info"]);
        t.local_path = None;
        assert_eq!(enabled(&t), ["Remove from List", "Open", "Get Info"], "Show in Finder needs a local path");
        t.status = Status::FinishedWithError;
        assert_eq!(enabled(&t), ["Remove from List", "Get Info"]);
        t.status = Status::Cancelling;
        assert_eq!(enabled(&t), ["Get Info"]);
        t.status = Status::Cancelled;
        assert_eq!(enabled(&t), ["Remove from List", "Get Info"]);
    }

    /// `update` (`:106-160`).
    #[test]
    fn status_messages_follow_iterm2() {
        let mut t = download();
        let msgs: Vec<_> = [Status::Unstarted, Status::Starting, Status::Transferring, Status::FinishedSuccessfully, Status::FinishedWithError, Status::Cancelling, Status::Cancelled]
            .into_iter()
            .map(|s| {
                t.status = s;
                t.message()
            })
            .collect();
        assert_eq!(msgs, ["Starting\u{2026}", "Starting\u{2026}", "Downloading\u{2026}", "Finished", "Failed", "Cancelling\u{2026}", "Cancelled"]);
        let mut up = secure_copy(Dir::Upload, "u", "h", "/p", Source::None);
        up.status = Status::Transferring;
        assert_eq!(up.message(), "Uploading\u{2026}");
    }

    /// `getInfo:` (`:188-212`): the summary text for each status, with the destination line first.
    #[test]
    fn get_info_follows_iterm2() {
        let mut t = download();
        t.status = Status::FinishedSuccessfully;
        assert_eq!(t.info(), "a.txt\n\nStatus: Finished\nDestination: C:\\dl\\a.txt");
        t.status = Status::FinishedWithError;
        t.error = "No data received.".into();
        assert_eq!(t.info(), "a.txt\n\nStatus: Failed with error \u{201c}No data received.\u{201d}\nDestination: C:\\dl\\a.txt");
        t.destination = None;
        t.status = Status::Cancelled;
        assert_eq!(t.info(), "a.txt\n\nStatus: Canceled by user\nLocal path: C:\\dl\\a.txt");
        t.status = Status::Cancelling;
        assert!(t.info().contains("Status: Waiting to cancel"));
        let up = secure_copy(Dir::Upload, "me", "host", "/tmp/a", Source::None);
        assert_eq!(up.info(), "Secure copy\nUser name: me\nHost: host\nFile: /tmp/a\n\nStatus: Starting");
        assert_eq!(up.subheading, "me@host:/tmp/a");
        assert_eq!(up.short_name, "a");
        let none = terminal_download(None, None);
        assert_eq!((none.short_name.as_str(), none.subheading.as_str()), ("Unnamed file", "Terminal download"));
        assert_eq!(SUMMARY_TITLE, "File Transfer Summary");
    }

    #[test]
    fn a_menu_exists_after_its_first_transfer_and_clear_all_keeps_the_running() {
        let mut ts = Transfers::default();
        assert!(!ts.menu_shown(Dir::Download) && !ts.menu_shown(Dir::Upload));
        let done = ts.start(download());
        let running = ts.start(secure_copy(Dir::Download, "u", "h", "/p/b", Source::Ssh(7)));
        let up = ts.start(secure_copy(Dir::Upload, "u", "h", "/p/c", Source::Ssh(8)));
        assert!(ts.menu_shown(Dir::Download) && ts.menu_shown(Dir::Upload));
        ts.finish(done, None);
        ts.progress(running, 5, Some(10));
        ts.progress(up, 1, Some(2));
        ts.finish(up, None);
        ts.clear_all(Dir::Download);
        let ids: Vec<u64> = ts.rows(Dir::Download).iter().map(|r| r.id).collect();
        assert_eq!(ids, [running], "the finished download goes, the running one stays");
        assert_eq!(ts.rows(Dir::Upload).len(), 1, "Clear All of Downloads leaves Uploads");
        ts.clear_all(Dir::Upload);
        assert!(ts.rows(Dir::Upload).is_empty());
        assert!(ts.menu_shown(Dir::Upload), "the menu stays once added");
        assert!(!ts.remove(running), "a running transfer is not removed");
    }

    /// Stop moves a running transfer to Cancelling; when its work ends it is Cancelled, not Failed.
    #[test]
    fn stop_ends_cancelled() {
        let mut ts = Transfers::default();
        let id = ts.start(secure_copy(Dir::Download, "u", "h", "/p/b", Source::Ssh(3)));
        assert!(!ts.stop(99));
        ts.progress(id, 1, Some(9));
        assert_eq!(ts.find_source(Source::Ssh(3)), Some(id));
        assert!(ts.stop(id));
        assert!(!ts.stop(id), "already cancelling");
        ts.progress(id, 2, None);
        assert_eq!(ts.get(id).unwrap().status, Status::Cancelling, "progress does not undo it");
        ts.fail(id, TRANSFER_CANCELLED);
        assert_eq!(ts.get(id).unwrap().status, Status::Cancelled);
        assert!(ts.find_source(Source::Ssh(3)).is_none());
        let other = ts.start(secure_copy(Dir::Download, "u", "h", "/p/c", Source::Ssh(4)));
        ts.fail(other, "boom");
        let t = ts.get(other).unwrap();
        assert_eq!((t.status, t.error.as_str()), (Status::FinishedWithError, "boom"));
    }

    /// `cleanUpMenus`: a day after its last change a finished item is dropped; a running one stays.
    #[test]
    fn old_finished_items_are_pruned() {
        let mut ts = Transfers::default();
        let old = ts.start(download());
        let running = ts.start(download());
        ts.finish(old, None);
        ts.progress(running, 1, None);
        let later = Instant::now() + KEEP_FINISHED + Duration::from_secs(1);
        ts.prune(later);
        assert_eq!(ts.rows(Dir::Download).iter().map(|r| r.id).collect::<Vec<_>>(), [running]);
    }

    #[test]
    fn ids_round_trip() {
        for act in [Act::Stop, Act::ShowInFinder, Act::RemoveFromList, Act::Open, Act::GetInfo] {
            assert_eq!(parse_item_id(&item_id(12, act)), Some((12, act)));
        }
        assert_eq!(parse_item_id("transfer:x:stop"), None);
        assert_eq!(parse_clear_id(&clear_id(Dir::Upload)), Some(Dir::Upload));
        assert_eq!(parse_clear_id(&clear_id(Dir::Download)), Some(Dir::Download));
        assert_eq!(parse_clear_id("transfers-clear:?"), None);
    }

    #[test]
    fn titles_show_the_status_and_percent() {
        let mut t = secure_copy(Dir::Download, "u", "h", "/p/b.bin", Source::None);
        assert_eq!(t.title(), "b.bin - Starting\u{2026}");
        t.status = Status::Transferring;
        t.size = Some(200);
        t.bytes = 50;
        assert_eq!(t.title(), "b.bin - Downloading\u{2026} 25%");
    }
}

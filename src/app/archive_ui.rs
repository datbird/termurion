//! Session archives in the app (milestone 13q Task 8): Save Archive of Current Session, Archive
//! On Closure, Restore Archive and the recent archives (`PSW:1676-1738`, `PTV:1006-1022`,
//! `AMB:17-110`). The bytes are built on the UI thread; a short-lived worker writes them and
//! nothing here probes, stats or creates a folder (a network folder included). A restored
//! archive opens through the arrangement loader's gate (`open_arrangement_windows`, B10).

use super::*;
use crate::archive as ar;
use crate::arrangement::Value;

/// The items the Archives submenu keeps before the recents: Save, Restore and the separator.
const FIXED_ITEMS: usize = 3;

/// The shared list a worker reports into: the path and how its write went.
pub(super) type Results = Arc<Mutex<Vec<(String, Result<(), String>)>>>;

type Job = (PathBuf, Vec<u8>, Box<dyn FnOnce(Result<(), String>) + Send>);

/// The one archive writer: every archive goes through a single thread, so two writes of one file
/// never overlap (a window of same-named panes closes in one loop), and `pending` lets Quit wait
/// for the last one.
struct Writer {
    tx: std::sync::mpsc::Sender<Job>,
    pending: Arc<(Mutex<usize>, std::sync::Condvar)>,
}

fn writer() -> Option<&'static Writer> {
    static W: std::sync::OnceLock<Option<Writer>> = std::sync::OnceLock::new();
    W.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Job>();
        let pending: Arc<(Mutex<usize>, std::sync::Condvar)> = Default::default();
        let count = pending.clone();
        std::thread::Builder::new()
            .name("archive-write".into())
            .spawn(move || {
                for (path, bytes, done) in rx {
                    let result = std::fs::write(&path, &bytes).map_err(|e| e.to_string());
                    done(result);
                    let (n, cv) = &*count;
                    if let Ok(mut n) = n.lock() {
                        *n = n.saturating_sub(1);
                    }
                    cv.notify_all();
                }
            })
            .ok()
            .map(|_| Writer { tx, pending })
    })
    .as_ref()
}

/// Queues `bytes` for `path` on the writer thread and calls `done` there with the outcome. The
/// folder is never created or probed first (a share included); a writer that cannot start still
/// reports a failure.
pub(super) fn write_in_background(path: PathBuf, bytes: Vec<u8>, done: impl FnOnce(Result<(), String>) + Send + 'static) {
    let Some(w) = writer() else { return done(Err("the archive writer could not start".to_string())) };
    if let Ok(mut n) = w.pending.0.lock() {
        *n += 1;
    }
    if let Err(e) = w.tx.send((path, bytes, Box::new(done))) {
        if let Ok(mut n) = w.pending.0.lock() {
            *n = n.saturating_sub(1);
        }
        (e.0 .2)(Err("the archive writer stopped".to_string()));
    }
}

/// Waits until every queued archive is written, at most `wait` (Quit and the last close: the
/// process may end right after). True when nothing is left.
pub(super) fn wait_for_archives(wait: std::time::Duration) -> bool {
    let Some(w) = writer() else { return true };
    let deadline = Instant::now() + wait;
    let Ok(mut n) = w.pending.0.lock() else { return true };
    while *n > 0 {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        n = match w.pending.1.wait_timeout(n, left) {
            Ok((g, _)) => g,
            Err(_) => return true,
        };
    }
    true
}

/// The archive paths this run has written (or queued), so an automatic archive never reuses one.
fn written_paths() -> &'static Mutex<std::collections::HashSet<PathBuf>> {
    static P: std::sync::OnceLock<Mutex<std::collections::HashSet<PathBuf>>> = std::sync::OnceLock::new();
    P.get_or_init(Default::default)
}

/// Reads a file of at most `limit` bytes; a larger one is an error naming the limit.
fn read_bounded(path: &std::path::Path, limit: u64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    // The size comes from a stat first: a file that is too large is never read on the UI thread.
    let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if let Some(why) = ar::size_refusal(len).filter(|_| limit == ar::MAX_FILE_BYTES).or_else(|| (len > limit).then(|| format!("the file is larger than {limit} bytes"))) {
        return Err(why);
    }
    // `take` still guards against a file that grows between the stat and the read.
    let mut bytes = Vec::new();
    std::fs::File::open(path).map_err(|e| e.to_string())?.take(limit + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    match ar::size_refusal(bytes.len() as u64).filter(|_| limit == ar::MAX_FILE_BYTES).or_else(|| (bytes.len() as u64 > limit).then(|| format!("the file is larger than {limit} bytes"))) {
        Some(why) => Err(why),
        None => Ok(bytes),
    }
}

impl App {
    /// The recent archives, oldest first (`NoSyncRecentArchives`).
    fn recent_archives(&self) -> Vec<String> {
        self.globals.get(ar::RECENTS_KEY).and_then(|v| v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())).unwrap_or_default()
    }

    /// Builds the session's archive and hands it to the writer; its outcome comes back through
    /// `archive_tick`. Err only when there is nothing to archive.
    pub(super) fn save_archive_to(&mut self, id: PaneId, path: &std::path::Path) -> Result<(), String> {
        let window = self.snapshot_session(id).ok_or("there is no session to archive")?;
        let bytes = crate::arrangement::to_plist(&[window])?;
        let (results, proxy) = (self.archive_results.clone(), self.proxy.clone());
        let shown = path.display().to_string();
        write_in_background(path.to_path_buf(), bytes, move |r| {
            if let Ok(mut q) = results.lock() {
                q.push((shown, r));
            }
            if let Ok(p) = proxy.lock() {
                let _ = p.send_event(UserEvent::Wake);
            }
        });
        Ok(())
    }

    /// Shell > Archives > Save Archive of Current Session... (`PTV:1006-1022`).
    pub(super) fn save_archive_menu(&mut self) {
        let Some(id) = self.focused_id() else { return };
        let name = ar::save_name(&self.session_title(id));
        let Some(path) = rfd::FileDialog::new().set_title("Save Archive").set_file_name(name).add_filter("Archive", &[ar::EXTENSION]).save_file() else { return };
        if let Err(e) = self.save_archive_to(id, &path) {
            self.banners.set("arrangement", ar::failed_text(&path.display().to_string(), &e));
        }
        self.redraw();
    }

    /// Archive On Closure (`PS:4057-4065`, `PSW:1676-1695`): a pane that is not a tmux pane, not
    /// an archive (nor a loaded recording or kept-open pane) whose profile says so is written into
    /// "Archive Directory" as "<date and time> - <session name>.itermarchive"; nothing without a
    /// folder. A close kept for Undo Close archives too (Decision).
    pub(super) fn auto_archive(&mut self, id: PaneId) {
        let Some(p) = self.panes.get(&id) else { return };
        if p.tmux_io().is_some() || p.archived || p.dvr.read_only || p.hold_open {
            return;
        }
        let profile = self.store.get(&p.profile).unwrap_or_else(|| self.store.default_profile());
        if !profile.bool("Archive On Closure") {
            return;
        }
        let folder = profile.string("Archive Directory");
        if folder.is_empty() {
            return;
        }
        let name = ar::auto_file_name(&crate::session_log::now_parts().0, &self.session_title(id));
        let path = crate::session_log::join_archive_path(&folder, &name, &profile::home());
        // Two panes of one name closing in one second get two files.
        let path = match written_paths().lock() {
            Ok(mut taken) => {
                let unique = ar::unique_path(&path, &|p| taken.contains(p));
                taken.insert(unique.clone());
                unique
            }
            Err(_) => path,
        };
        if let Some(why) = ar::network_refusal(&folder, &path) {
            let _ = crate::notify::show_with(ar::FAILED_TITLE, &ar::failed_text(&path.display().to_string(), &why), false);
            return;
        }
        if !path.is_absolute() {
            let _ = crate::notify::show_with(ar::FAILED_TITLE, &ar::failed_text(&path.display().to_string(), "the archive folder is not a full path"), false);
            return;
        }
        if let Err(e) = self.save_archive_to(id, &path) {
            let _ = crate::notify::show_with(ar::FAILED_TITLE, &ar::failed_text(&path.display().to_string(), &e), false);
        }
    }

    /// Quit and the last window's close: the sessions that end with Tandem are archived when
    /// `all` (iTerm2 saves archives synchronously on app quit, `PTYSession.swift:1709`), then
    /// the exit waits, at most `wait`, for the writer.
    pub(super) fn finish_archives(&mut self, all: bool, wait: Duration) {
        if all {
            let ids: Vec<PaneId> = self.panes.keys().copied().collect();
            for id in ids {
                // The archive is built from the pane's own window (a parked window's frame and
                // title are its own).
                self.enter_pane_window(id);
                self.auto_archive(id);
            }
        }
        wait_for_archives(wait);
    }

    /// The writers' outcomes: a saved archive joins the recents, a failure is notified.
    pub(super) fn archive_tick(&mut self) {
        let done: Vec<(String, Result<(), String>)> = self.archive_results.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
        for (path, result) in done {
            match result {
                Ok(()) => {
                    let list = ar::add_recent(&self.recent_archives(), &path);
                    self.globals.set(ar::RECENTS_KEY, Value::Array(list.into_iter().map(Value::String).collect()));
                    self.save_globals();
                    self.menus_dirty = true;
                }
                Err(e) => {
                    let _ = crate::notify::show_with(ar::FAILED_TITLE, &ar::failed_text(&path, &e), false);
                }
            }
        }
    }

    /// Shell > Archives > Restore Archive... or a recent item: each window opens as a new
    /// window of sessions with no shell. A downloaded file asks first and its profiles pass the
    /// network filter, as a loaded arrangement does (B10).
    pub(super) fn restore_archive(&mut self, path: Option<PathBuf>) {
        let Some(path) = path.or_else(|| rfd::FileDialog::new().set_title("Restore Archive").add_filter("Archive", &[ar::EXTENSION]).pick_file()) else { return };
        let windows = match read_bounded(&path, ar::MAX_FILE_BYTES).and_then(|b| crate::arrangement::from_plist(&b)).and_then(|w| ar::count_refusal(&w).map_or(Ok(w), Err)) {
            Ok(w) => w,
            Err(e) => {
                self.banners.set("arrangement", format!("Could not restore {}: {e}", path.display()));
                return self.redraw();
            }
        };
        // Whatever the file says, its sessions have no shell; a hotkey window is not an archive's.
        let windows = windows.into_iter().filter(|w| crate::arrangement::hotkey_guid(w).is_none()).map(|w| if ar::is_archive(&w) { w } else { ar::archive_window(w, 80, 24) }).collect();
        self.open_arrangement_windows(&path, windows, "Restore Archive?");
    }

    /// Shell > Archives lists the recents after its separator, oldest first, titled by the file
    /// name without its extension (`AMB:53-63`).
    pub(super) fn refresh_archives_menu(&mut self) {
        let items = self.recent_archives();
        let Some(bar) = &mut self.menu_bar else { return };
        if bar.archive_items == items {
            return;
        }
        if let Some(sub) = &bar.archives {
            while sub.remove_at(FIXED_ITEMS).is_some() {}
            for (i, path) in items.iter().enumerate() {
                let _ = sub.append(&muda::MenuItem::with_id(format!("{}{i}", ar::MENU_ID), ar::recent_title(path).replace('&', "&&"), true, None));
            }
        }
        bar.archive_items = items;
    }

    /// A pick from the recent archives. False for other ids.
    pub(super) fn archive_picked(&mut self, id: &str) -> bool {
        let Some(i) = id.strip_prefix(ar::MENU_ID).and_then(|n| n.parse::<usize>().ok()) else { return false };
        if let Some(path) = self.recent_archives().get(i) {
            self.restore_archive(Some(PathBuf::from(path)));
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    fn production() -> &'static str {
        let m = include_str!("archive_ui.rs");
        m.split("#[cfg(test)]").next().unwrap()
    }

    fn body(src: &'static str, head: &str) -> &'static str {
        let from = src.find(head).unwrap_or_else(|| panic!("{head}"));
        let rest = &src[from..];
        rest.split("\n    }\n").next().unwrap()
    }

    /// One writer for every archive: queued writes of one path never mix, and a wait returns
    /// when the last is on disk.
    #[test]
    fn archives_are_written_one_at_a_time() {
        let dir = std::env::temp_dir().join(format!("tandem-archive-seq-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("same.itermarchive");
        let done = Arc::new(Mutex::new(0usize));
        let mut last = Vec::new();
        for n in 0..8u8 {
            last = vec![b'a' + n; (9 - n as usize) * 2_000_000];
            let d = done.clone();
            super::write_in_background(path.clone(), last.clone(), move |r| {
                assert!(r.is_ok());
                *d.lock().unwrap() += 1;
            });
        }
        assert!(super::wait_for_archives(std::time::Duration::from_secs(20)), "everything queued was written");
        assert_eq!(*done.lock().unwrap(), 8);
        assert_eq!(std::fs::read(&path).unwrap(), last, "the last queued write is the file, whole");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn archives_finish_before_the_process_ends() {
        let m = include_str!("mod.rs");
        let menu = include_str!("menu.rs");
        let quit = body(menu, "pub(super) fn quit_confirmed(");
        assert!(quit.contains("self.finish_archives(!keep, std::time::Duration::from_secs(5));"), "Quit archives the sessions that end with it (iTerm2 saves archives on app quit)");
        assert!(quit.find("self.finish_archives(").unwrap() < quit.find("self.leave_host(keep);").unwrap(), "before leave_host empties the panes");
        let last = body(m, "    fn close_pane(");
        assert!(last.find("self.finish_archives(false, std::time::Duration::from_secs(5));").unwrap() < last.find("self.leave_host(false);").unwrap(), "the last close waits for its archive");
        let fin = body(production(), "    pub(super) fn finish_archives(");
        assert!(fin.contains("wait_for_archives("));
    }

    #[test]
    fn restore_reads_a_bounded_file() {
        let restore = body(production(), "    pub(super) fn restore_archive(");
        assert!(restore.contains("read_bounded(") && restore.contains("count_refusal("), "size and counts are checked before anything opens");
        let dir = std::env::temp_dir().join(format!("tandem-archive-big-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.itermarchive");
        std::fs::write(&f, b"0123456789").unwrap();
        assert_eq!(super::read_bounded(&f, 10).unwrap(), b"0123456789");
        assert!(super::read_bounded(&f, 9).unwrap_err().contains("larger"), "a file past the limit is refused");
        // A file past the limit is refused by its size, before anything is read: one that cannot
        // be opened at all (another process holds it exclusively) still gets the size answer.
        {
            use std::os::windows::fs::OpenOptionsExt;
            let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(&f).unwrap();
            assert!(super::read_bounded(&f, 9).unwrap_err().contains("larger"), "the size is checked with a stat, not a read");
            drop(held);
        }
        let _ = std::fs::remove_dir_all(&dir);
        let arrange = include_str!("arrange_ui.rs");
        let shared = body(arrange, "    pub(super) fn open_arrangement_windows(");
        assert!(shared.contains("needs_question("), "several windows of an unmarked file ask first");
        let tab = body(arrange, "    pub(super) fn build_tab(");
        assert!(tab.contains("cap_lines(") && tab.contains("MAX_TEXT_LINES"), "a restored pane's text is capped");
        // Session restoration keeps an archived pane archived: saved with a mark, built with no shell.
        assert!(body(arrange, "    fn session_value(").contains("mark_archived(&mut v)"), "the saved state marks it");
        assert!(tab.contains("let archive = archive || crate::archive::session_archived(&session);"), "the launch builds it with no shell");
    }

    #[test]
    fn the_writer_reports_back_and_never_probes() {
        let dir = std::env::temp_dir().join(format!("tandem-archive-{}", std::process::id()));
        let missing = dir.join("no").join("such").join("a.itermarchive");
        let out: Arc<Mutex<Vec<Result<(), String>>>> = Default::default();
        let o = out.clone();
        super::write_in_background(missing.clone(), b"x".to_vec(), move |r| o.lock().unwrap().push(r));
        for _ in 0..200 {
            if !out.lock().unwrap().is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(out.lock().unwrap()[0].is_err(), "a missing folder is a failure, never created");
        assert!(!dir.exists(), "no folder was created");
        // Restore Archive stats the file the person picked (its size, before reading it); the
        // folders of the writer are never probed.
        let src = production().replace("std::fs::metadata(path)", "");
        for forbidden in ["create_dir", ".exists()", "is_dir()", "metadata(", "canonicalize"] {
            assert!(!src.contains(forbidden), "archive_ui never probes or creates a folder: {forbidden}");
        }
    }

    /// B10: the loader shared with Load Arrangement asks about a downloaded file, and the archive
    /// never reads a path before a dialog or the recents gave it.
    #[test]
    fn restore_archive_asks_for_a_downloaded_file() {
        let restore = body(production(), "    pub(super) fn restore_archive(");
        assert!(restore.contains("open_arrangement_windows("), "the shared gate");
        let arrange = include_str!("arrange_ui.rs");
        let shared = body(arrange, "    pub(super) fn open_arrangement_windows(");
        assert!(shared.contains("has_mark_of_the_web") && shared.contains("trust_text"));
        let load = body(arrange, "    pub(super) fn load_arrangement_file(");
        assert!(load.contains("open_arrangement_windows("), "Load Arrangement uses the same gate");
        let read = restore.find("read_bounded(").unwrap();
        let pick = restore.find("pick_file").unwrap();
        assert!(pick < read, "the dialog returns the path before anything reads it");
    }

    #[test]
    fn archives_are_wired_as_iterm2() {
        let m = include_str!("mod.rs");
        let close = body(m, "    fn close_pane(");
        assert!(close.trim_start_matches(|c| c != '\n').trim_start().starts_with("// ") || close.contains("self.auto_archive(id);"));
        let first = close.find("self.auto_archive(id);").unwrap();
        assert!(first < close.find("self.warn_short_lived(id);").unwrap(), "the archive is written first");
        let menu = include_str!("menu.rs");
        assert!(menu.contains("\"saveArchive:\" => self.save_archive_menu()") && menu.contains("\"restoreArchive:\" => self.restore_archive(None)"));
        assert!(crate::commands::DONE.contains(&"saveArchive:") && crate::commands::DONE.contains(&"restoreArchive:"));
        assert!(menu.contains("self.archive_picked(id)") && menu.contains("self.refresh_archives_menu();"));
        assert!(m.contains("self.archive_tick();"), "outcomes are collected every pass");
        assert!(include_str!("session_end_ui.rs").contains("archived: p.archived"), "an archive's close never warns of a short life");
    }
}

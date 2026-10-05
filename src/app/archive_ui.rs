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

/// Writes `bytes` to `path` on a short-lived thread and calls `done` there with the outcome. The
/// folder is never created or probed first (a share included); a thread that cannot start still
/// reports a failure.
pub(super) fn write_in_background(path: PathBuf, bytes: Vec<u8>, done: impl FnOnce(Result<(), String>) + Send + 'static) {
    let done = Arc::new(Mutex::new(Some(done)));
    let worker = done.clone();
    let spawned = std::thread::Builder::new().name("archive-write".into()).spawn(move || {
        let result = std::fs::write(&path, &bytes).map_err(|e| e.to_string());
        if let Some(d) = worker.lock().ok().and_then(|mut g| g.take()) {
            d(result);
        }
    });
    if let Err(e) = spawned {
        if let Some(d) = done.lock().ok().and_then(|mut g| g.take()) {
            d(Err(e.to_string()));
        }
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
        let path = crate::session_log::join_log_path(&folder, &name, &profile::home());
        if !path.is_absolute() {
            let _ = crate::notify::show_with(ar::FAILED_TITLE, &ar::failed_text(&path.display().to_string(), "the archive folder is not a full path"), false);
            return;
        }
        if let Err(e) = self.save_archive_to(id, &path) {
            let _ = crate::notify::show_with(ar::FAILED_TITLE, &ar::failed_text(&path.display().to_string(), &e), false);
        }
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
        let windows = match std::fs::read(&path).map_err(|e| e.to_string()).and_then(|b| crate::arrangement::from_plist(&b)) {
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
        let src = production();
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
        let read = restore.find("std::fs::read").unwrap();
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

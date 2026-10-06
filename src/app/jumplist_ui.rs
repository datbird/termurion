//! The Jump List and the recent profiles behind it (m13v1 Task 6): the taskbar button's Jump List
//! is iTerm2's dock menu, rebuilt from the profiles, the arrangements and the recents; a recent is
//! `%APPDATA%\Tandem\Recents\<uuid>.itermtab` (`ProfileDocument.swift`). Every Jump List entry
//! starts Tandem's own exe with arguments `launch::parse` accepts, so a click goes through the
//! launch pipe and its checks. The COM writer is injected into the pure rebuild loop; a test build
//! never writes the real Jump List.

use std::path::{Path, PathBuf};
use std::time::Instant;

use super::scripts_alerts::{ask, Then};
use super::windows_ui::NewWindow;
use super::App;
use crate::jumplist::{self, Recent, Recents, TabOpen};

/// `saveProfilesToRecentDocuments`'s key (13a's capitalized name).
pub(super) const KEY: &str = "SaveProfilesToRecentDocuments";
const BANNER: &str = "jump-list";
const LIST_FILE: &str = "recents.json";
/// The Jump List items the person removed (`jumplist::Removed`), next to the recents list.
const REMOVED_FILE: &str = "removed.json";

/// `%APPDATA%\Tandem\Recents`.
fn recents_dir() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("Tandem").join("Recents")
}

/// A list file of the folder, read at most one byte past its limit (so an oversized one is
/// refused by its loader); empty when it cannot be read.
fn read_list(dir: &Path, name: &str) -> String {
    use std::io::Read;
    let mut text = String::new();
    let read = std::fs::File::open(dir.join(name)).and_then(|f| f.take(jumplist::MAX_LIST_BYTES as u64 + 1).read_to_string(&mut text));
    if read.is_err() {
        return String::new();
    }
    text
}

/// The stored list: `Recents::load` keeps only what is valid.
fn load_recents(dir: &Path) -> Recents {
    Recents::load(&read_list(dir, LIST_FILE))
}

/// The saved removed items: `Removed::load` keeps only what is valid.
fn load_removed(dir: &Path) -> jumplist::Removed {
    jumplist::Removed::load(&read_list(dir, REMOVED_FILE))
}

fn store_removed(dir: &Path, removed: &jumplist::Removed) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(REMOVED_FILE), removed.save())
}

/// Writes the front entry's `.itermtab` file and the list.
fn store_recents(dir: &Path, recents: &Recents) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    if let Some(first) = recents.list.first() {
        std::fs::write(dir.join(&first.file), jumplist::tab_file_text(&first.guid, &first.name))?;
    }
    std::fs::write(dir.join(LIST_FILE), recents.save())
}

/// Deletes the `<uuid>.itermtab` files no entry names (a renamed profile's old file, a dropped
/// entry's); nothing else in the folder is touched.
fn delete_orphans(dir: &Path, recents: &Recents) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let names: Vec<String> = read.flatten().filter_map(|e| e.file_name().into_string().ok()).collect();
    for f in recents.orphans(&names) {
        let _ = std::fs::remove_file(dir.join(f));
    }
}

/// A `.itermtab` file's text: judged like every file another program names, and at most
/// `MAX_TAB_FILE` bytes (one more is read to see it was longer, which refuses it).
fn read_tab_file(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    crate::localfs::open(&crate::localfs::LocalPath::judge(path)?).ok()?.take(jumplist::MAX_TAB_FILE as u64 + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() > jumplist::MAX_TAB_FILE {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// The COM writer, except in a test build, which never writes the real Jump List.
#[cfg(all(windows, not(test)))]
fn write_list(m: &jumplist::Model, exe: &Path) -> Result<Vec<Vec<String>>, String> {
    crate::jumplist_win::commit(m, exe)
}

#[cfg(any(not(windows), test))]
fn write_list(_: &jumplist::Model, _: &Path) -> Result<Vec<Vec<String>>, String> {
    Ok(Vec::new())
}

#[cfg(all(windows, not(test)))]
fn clear_destinations() -> Result<(), String> {
    crate::jumplist_win::clear_recent_destinations()
}

#[cfg(any(not(windows), test))]
fn clear_destinations() -> Result<(), String> {
    Ok(())
}

impl App {
    /// The recents list, read on first use; with the key off at launch they are cleared
    /// (`AD:1340-1342`).
    fn recents_ready(&mut self) {
        if std::mem::replace(&mut self.jump_sync.started, true) {
            return;
        }
        let dir = recents_dir();
        self.recents = load_recents(&dir);
        self.jump_sync.removed = load_removed(&dir);
        if !self.globals.bool(KEY) {
            self.recents.clear();
            let _ = store_recents(&dir, &self.recents);
            delete_orphans(&dir, &self.recents);
        }
        self.jump_list_due = true;
    }

    /// The profiles in the Profiles menu's order, as (guid, name).
    fn jump_profiles(&self) -> Vec<(String, String)> {
        let rows = crate::profile_menu::menu_rows(&self.store.profiles);
        let tree = crate::profile_menu::tree(&rows);
        crate::profile_menu::open_all_guids(&tree, "").into_iter().filter_map(|g| rows.iter().find(|r| r.0 == g).map(|r| (r.0.clone(), r.1.clone()))).collect()
    }

    /// Rebuilds the Jump List when it is due, at most once a second; answers when to look again.
    pub(super) fn jump_list_tick(&mut self) -> Option<Instant> {
        self.gpu.as_ref()?;
        self.recents_ready();
        let key = self.globals.bool(KEY);
        if self.jump_sync.key_seen.replace(key).is_some_and(|k| k != key) {
            self.jump_list_due = true;
        }
        if !self.jump_list_due {
            return None;
        }
        let dir = recents_dir();
        let recents: Vec<Recent> = self.recents.list.iter().map(|r| Recent { file: dir.join(&r.file).to_string_lossy().into_owned(), ..r.clone() }).collect();
        let arrangements: Vec<String> = self.arrangements().names().into_iter().map(str::to_string).collect();
        let built = jumplist::model(&self.jump_profiles(), &self.store.default_guid, &arrangements, &recents, key);
        let Ok(exe) = std::env::current_exe() else {
            self.jump_list_due = false;
            return None;
        };
        let tick = self.jump_sync.tick(&mut self.jump_list_due, Instant::now(), &|| built.clone(), &mut |m| write_list(m, &exe));
        if let Some(e) = tick.banner {
            self.banners.set(BANNER, e);
            self.redraw();
        }
        if tick.save_removed {
            if let Err(e) = store_removed(&recents_dir(), &self.jump_sync.removed) {
                self.banners.set(BANNER, format!("Tandem could not save the removed Jump List items: {e}"));
                self.redraw();
            }
        }
        tick.wake
    }

    /// A session started with this saved profile (`SF:393-397`): with the key on it moves to the
    /// front of the recents. A name no link may show is not recorded.
    pub(super) fn note_recent(&mut self, guid: &str, name: &str) {
        if !self.globals.bool(KEY) || crate::pathnav::safe_text(name).is_none() || name.chars().count() > crate::launch::MAX_NAME {
            return;
        }
        self.recents_ready();
        if !self.recents.add(guid, name, &|| format!("{}.itermtab", uuid::Uuid::new_v4())) {
            return;
        }
        let dir = recents_dir();
        match store_recents(&dir, &self.recents) {
            Ok(()) => delete_orphans(&dir, &self.recents),
            Err(e) => {
                self.banners.set(BANNER, format!("Tandem could not save the recent profiles: {e}"));
                self.redraw();
            }
        }
        self.jump_list_due = true;
    }

    /// "Remove Recent Profiles from Dock Menu" (`AD:3616-3618`): Tandem's list and the files
    /// behind it go, and so do the destinations Windows keeps for Tandem; nothing else is touched.
    pub(super) fn remove_recent_profiles(&mut self) {
        self.recents_ready();
        self.recents.clear();
        let dir = recents_dir();
        let _ = store_recents(&dir, &self.recents);
        delete_orphans(&dir, &self.recents);
        if let Err(e) = clear_destinations() {
            self.banners.set(BANNER, e);
            self.redraw();
        }
        self.jump_list_due = true;
    }

    /// An opened `.itermtab` file (`ProfileDocument.open`): a tab of its profile in the current
    /// window (a window when there is none), after iTerm2's question when the profile was renamed
    /// and another one has the old name.
    pub(super) fn open_profile_tab(&mut self, path: &Path) {
        let shown = crate::inert_files::shown_name(&path.to_string_lossy());
        let Some((guid, name)) = read_tab_file(path).and_then(|t| jumplist::parse_tab_file(&t)).filter(|(g, n)| g.chars().count() <= crate::launch::MAX_GUID && n.chars().count() <= crate::launch::MAX_NAME) else {
            self.banners.set(BANNER, format!("Tandem could not read \u{201c}{shown}\u{201d} as a profile shortcut."));
            return self.redraw();
        };
        let profiles = &self.store.profiles;
        let name_of = |g: &str| profiles.iter().find(|p| p.guid() == g).map(|p| p.name());
        let guid_of = |n: &str| profiles.iter().find(|p| p.name() == n).map(|p| p.guid());
        match jumplist::tab_open(&guid, &name, &name_of, &guid_of) {
            TabOpen::Guid(g) => self.open_recent_profile(&g),
            TabOpen::Missing => {
                self.banners.set(BANNER, format!("The profile \u{201c}{}\u{201d} no longer exists.", crate::inert_files::shown_name(&name)));
                self.redraw();
            }
            TabOpen::Ask { by_guid, by_name, old, new } => {
                let (old, new) = (crate::inert_files::shown_name(&old), crate::inert_files::shown_name(&new));
                let labels = [jumplist::open_name(&old), jumplist::open_name(&new), crate::api_auth::CANCEL.to_string()];
                let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
                self.ask_scripts(ask(jumplist::RENAMED_HEADING, &jumplist::renamed_text(&old, &new), &labels, Some(jumplist::RENAMED_ID), crate::warnings::Kind::Permanent, Then::OpenRenamedProfile { by_guid, by_name }));
            }
        }
    }

    /// The renamed-profile question's answer: the first button opens by guid, the second by
    /// the old name, Cancel nothing (`PD:44-61`).
    pub(super) fn renamed_profile_answered(&mut self, by_guid: &str, by_name: &str, index: usize) {
        match index {
            0 => self.open_recent_profile(by_guid),
            1 => self.open_recent_profile(by_name),
            _ => {}
        }
    }

    /// A tab of this profile in the current window, a window when there is none
    /// (`launchBookmark:inTerminal:respectTabbingMode:NO`); a profile that is gone opens nothing.
    fn open_recent_profile(&mut self, guid: &str) {
        if !self.store.profiles.iter().any(|p| p.guid() == guid) {
            self.banners.set(BANNER, "That profile no longer exists.".to_string());
            return self.redraw();
        }
        if self.gpu.is_none() {
            return;
        }
        if let Some(w) = self.last_window {
            self.enter(w);
        }
        if self.tabs.is_empty() {
            self.pending_windows.push(NewWindow::Profile(Some(guid.to_string())));
            self.redraw();
        } else {
            self.add_tab(Some(guid));
        }
        self.bring_forward();
        self.redraw();
    }
}

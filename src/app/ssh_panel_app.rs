//! Shell > ssh > Download Files in the app (milestone 14d): the menu item, the remote file
//! panel's window, and what Open does: each chosen item downloads into a fresh folder of its
//! own under `%TEMP%` (P12), and the ones that arrive are revealed together.

use std::path::{Path, PathBuf};

use super::files_ui::shown_name;
use super::ssh_files_app::Waiter;
use super::*;
use crate::remote_file::RemoteFile;
use crate::ssh_args::Identity;
use crate::ssh_local::{self as local, Local, Place};
use crate::ssh_panel::{self as model, DoubleClick, Op, Panel, Request};
use crate::ssh_panel_ui::{PanelEvent, SshPanelWindow};

/// The prefix of a batch's folder in `%TEMP%` (P12; swept at launch with the other leftovers, P24).
pub(super) const BATCH_PREFIX: &str = "Tandem-";

/// The folder a Download Files batch writes into: `%TEMP%\Tandem-<UUID>`, new and empty, made
/// with `create_dir` (P12) so it never adopts a folder, a junction or a file that was already
/// there. A remote-chosen name therefore can never replace another program's file in `%TEMP%`.
pub(super) fn make_batch_folder(temp_root: &Path, uuid: &str) -> std::io::Result<PathBuf> {
    let dir = temp_root.join(format!("{BATCH_PREFIX}{uuid}"));
    std::fs::create_dir(&dir)?;
    Ok(dir)
}

/// Where a chosen item's finished copy goes: `<batch folder>\<cleaned name>` (R14de-3; the
/// name is one component, so the parent is always the folder). A remote folder lands in
/// `<folder>\<name>` and its own folder unpacks inside it, iTerm2's `<name>\<name>`. Two items
/// of one name end last-one-wins (R14de-4).
pub(super) fn batch_dest(folder: &Path, remote: &RemoteFile) -> PathBuf {
    folder.join(crate::inline_file::clean_name(&remote.name()))
}

/// Last one wins inside a batch folder (P12, R14de-4): an earlier item of the same name, a file
/// or a folder, is removed before the next one lands at `dest`, so the two never merge. A link
/// there is removed itself, never followed.
pub(super) fn clear_earlier(dest: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(dest) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
        Ok(m) if m.file_type().is_symlink() => std::fs::remove_dir(dest).or_else(|_| std::fs::remove_file(dest)),
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(dest),
        Ok(_) => std::fs::remove_file(dest),
    }
}

/// What Shell > ssh > Download Files must be set to: Some(enabled) when this window's bar does not
/// show `on` yet (`shown` None: never set since the bar was made), None when it already does.
pub(super) fn download_files_update(shown: Option<bool>, on: bool) -> Option<bool> {
    (shown != Some(on)).then_some(on)
}

/// One Download Files click's items still out, what arrived and what failed.
#[derive(Debug)]
pub(super) struct Batch {
    pub pending: usize,
    pub done: Vec<PathBuf>,
    pub failed: Vec<(String, String)>,
}

impl Batch {
    pub fn new(items: usize) -> Batch {
        Batch { pending: items, done: Vec::new(), failed: Vec::new() }
    }

    /// An item arrived at `path`. True when it was the last one out.
    pub fn arrived(&mut self, path: PathBuf) -> bool {
        self.done.push(path);
        self.pending = self.pending.saturating_sub(1);
        self.pending == 0
    }

    /// An item failed (`name` is the remote name, `why` the reason). True when it was the last one out.
    pub fn failed(&mut self, name: &str, why: &str) -> bool {
        self.failed.push((name.to_string(), why.to_string()));
        self.pending = self.pending.saturating_sub(1);
        self.pending == 0
    }
}

/// Shows the finished items together in Explorer: one path selected in its folder, several
/// selected together in their common folder (`OP:67-80`, with the arrived ones only).
#[cfg(windows)]
pub(super) fn reveal_all(paths: &[PathBuf]) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::UI::Shell::{ILCreateFromPathW, ILFindLastID, ILFree, SHOpenFolderAndSelectItems};
    let wide = |p: &Path| -> Vec<u16> { p.as_os_str().encode_wide().chain(std::iter::once(0)).collect() };
    match paths {
        [] => Ok(()),
        [one] => {
            // The same Explorer the Win-click's reveal runs, found through the system directory.
            let program = crate::script_provision::system_directory().and_then(|s| crate::semantic::explorer_in(&s)).ok_or("the Windows folder is unknown")?;
            let mut cmd = std::process::Command::new(program);
            cmd.raw_arg(format!("/select,{}", crate::semantic::cmd_quote(&one.to_string_lossy())));
            cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
        }
        many => {
            let folder = many[0].parent().ok_or("no folder")?;
            if many.iter().any(|p| p.parent() != Some(folder)) {
                return Err("the items are not in one folder".into());
            }
            // SAFETY: each list comes from ILCreateFromPathW and is freed once; the child ids
            // point into their lists, which outlive the call.
            unsafe {
                let parent = ILCreateFromPathW(wide(folder).as_ptr());
                if parent.is_null() {
                    return Err("the folder could not be found".into());
                }
                let lists: Vec<*mut _> = many.iter().map(|p| ILCreateFromPathW(wide(p).as_ptr())).filter(|l| !l.is_null()).collect();
                let children: Vec<*const _> = lists.iter().map(|l| ILFindLastID(*l) as *const _).collect();
                let mut hr = if children.is_empty() { -1 } else { SHOpenFolderAndSelectItems(parent, children.len() as u32, children.as_ptr(), 0) };
                // winit's drop target starts OLE on the UI thread; if COM is not up, start it once.
                if hr == 0x800401F0u32 as i32 {
                    windows_sys::Win32::System::Com::CoInitializeEx(std::ptr::null(), windows_sys::Win32::System::Com::COINIT_APARTMENTTHREADED as u32);
                    hr = SHOpenFolderAndSelectItems(parent, children.len() as u32, children.as_ptr(), 0);
                }
                for l in lists {
                    ILFree(l);
                }
                ILFree(parent);
                if hr < 0 { Err(format!("Explorer answered 0x{:08X}", hr as u32)) } else { Ok(()) }
            }
        }
    }
}

impl App {
    /// Download Files is enabled while any host frames (`TVS:953-955`: iTerm2's registry is not empty).
    pub(super) fn download_files_enabled(&self) -> bool {
        self.ssh_registry.any()
    }

    /// Lights or greys Shell > ssh > Download Files in the live window (P21), only when its bar
    /// shows otherwise. Called from every `tick_ssh` and on a host change.
    pub(super) fn refresh_download_files_menu(&mut self) {
        let Some(on) = download_files_update(self.download_files_shown, self.download_files_enabled()) else { return };
        let Some(item) = self.menu_bar.as_ref().and_then(|b| b.ssh_download_files.as_ref()) else { return };
        item.set_enabled(on);
        self.download_files_shown = Some(on);
    }

    /// The pane that answers for `id`: the first, in tab order, whose innermost conductor frames
    /// with that identity (the endpoint iTerm2's registry returns). Panes in other windows follow, by id (P16).
    pub(super) fn pane_for_identity(&self, id: &Identity) -> Option<PaneId> {
        let mut order: Vec<PaneId> = self.tabs.iter().flat_map(|t| t.layout.panes()).collect();
        let mut rest: Vec<PaneId> = self.panes.keys().copied().filter(|p| !order.contains(p)).collect();
        rest.sort_unstable();
        order.extend(rest);
        order.into_iter().find(|p| self.framing(*p).is_some_and(|c| c.identity() == *id))
    }

    /// The home directory the host's framer reported.
    fn panel_home(&self, id: &Identity) -> Option<String> {
        let pane = self.pane_for_identity(id)?;
        self.framing(pane)?.home_directory.clone()
    }

    /// The path saved for the host (`FP:1608-1670`), only under a key P15 allows.
    fn panel_saved(&self, id: &Identity) -> Option<String> {
        let key = model::saved_path_key_checked(id, &crate::ssh_args::local_user())?;
        crate::ssh_jumps::clean_directory(&self.globals.string(&key))
    }

    /// Shell > ssh > Download Files: opens the panel (one at a time; a second pick focuses it)
    /// on the connected hosts, the focused session's host first.
    pub(super) fn download_files(&mut self, event_loop: &ActiveEventLoop) {
        if let Some((w, _)) = &self.ssh_panel {
            w.focus();
            return;
        }
        let hosts = self.ssh_registry.identities();
        if hosts.is_empty() {
            return;
        }
        let Some(g) = self.gpu.as_ref() else { return };
        let window = match SshPanelWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue) {
            Ok(w) => w,
            Err(e) => return self.banners.set("window", format!("Could not open the window: {e}")),
        };
        let preferred = self.focused_id().and_then(|id| self.framing(id)).map(|c| c.identity());
        let mut panel = Panel::new(hosts, preferred);
        let requests = panel.start(&|h| self.panel_saved(h), &|h| self.panel_home(h));
        self.ssh_panel = Some((window, panel));
        self.ssh_panel_send(requests);
    }

    /// Sends what the panel asked: each file request to the pane that answers for its host, with
    /// the panel as its waiter. A host with no pane answers -1 at once. (Search is Task 8.)
    fn ssh_panel_send(&mut self, requests: Vec<Request>) {
        let mut work: std::collections::VecDeque<Request> = requests.into();
        while let Some(r) = work.pop_front() {
            let Request::File { generation, host, op, sub } = r else { continue };
            let sent = match self.pane_for_identity(&host) {
                Some(pane) => self.ssh_send(pane, sub, true, Waiter::Panel { generation, op: op.clone() }),
                None => false,
            };
            if !sent {
                work.extend(self.ssh_panel_apply(generation, &op, "", -1));
            }
        }
        if let Some((w, _)) = &self.ssh_panel {
            w.request_redraw();
        }
    }

    fn ssh_panel_apply(&mut self, generation: u64, op: &Op, content: &str, status: i32) -> Vec<Request> {
        self.ssh_panel.as_mut().map(|(_, p)| p.answered(generation, op, content, status)).unwrap_or_default()
    }

    /// An answer for the panel (its window may have closed meanwhile: then it is dropped).
    pub(super) fn ssh_panel_answered(&mut self, generation: u64, op: Op, content: String, status: i32) {
        let more = self.ssh_panel_apply(generation, &op, &content, status);
        self.ssh_panel_send(more);
    }

    /// The connected hosts changed while the panel shows (R14de-7).
    pub(super) fn ssh_panel_hosts_changed(&mut self) {
        let Some((w, mut p)) = self.ssh_panel.take() else { return };
        let hosts = self.ssh_registry.identities();
        let requests = p.hosts_changed(hosts, &|h| self.panel_home(h));
        w.request_redraw();
        self.ssh_panel = Some((w, p));
        self.ssh_panel_send(requests);
    }

    /// Events for the panel's window. False when `window` is not it.
    pub(super) fn ssh_panel_event(&mut self, window: WindowId, event: &WindowEvent) -> bool {
        let Some((w, _)) = self.ssh_panel.as_mut() else { return false };
        if w.id() != window {
            return false;
        }
        match event {
            WindowEvent::CloseRequested => self.ssh_panel_close(),
            WindowEvent::RedrawRequested => {
                let Some((w, p)) = self.ssh_panel.as_mut() else { return true };
                let e = w.redraw(p);
                self.ssh_panel_act(e);
            }
            WindowEvent::Resized(_) => {
                w.event(event);
                // The path is saved on resize too (`FP:1202`); it writes only when it changed.
                self.ssh_panel_save_path();
            }
            other => {
                w.event(other);
            }
        }
        true
    }

    /// Closes the panel, saving its path (`FP:220`).
    fn ssh_panel_close(&mut self) {
        self.ssh_panel_save_path();
        self.ssh_panel = None;
    }

    /// Saves the host's path in `Globals` under `saved_path_key` (a plain global, so it syncs
    /// where globals sync), only for a safe identity (P15): at most 32 keys, the oldest dropped.
    fn ssh_panel_save_path(&mut self) {
        let Some((_, p)) = self.ssh_panel.as_ref() else { return };
        let (Some(host), Some(path)) = (p.host.clone(), p.path.clone()) else { return };
        let Some(key) = model::saved_path_key_checked(&host, &crate::ssh_args::local_user()) else { return };
        let Some(path) = crate::ssh_jumps::clean_directory(&path).filter(|p| p.chars().count() <= 4096) else { return };
        let mut order: Vec<String> = self.globals.get(model::SAVED_KEYS_LIST).and_then(|v| v.as_array().cloned()).unwrap_or_default().into_iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
        let before = order.clone();
        let dropped = model::remember_saved_key(&mut order, &key);
        if order == before && self.globals.string(&key) == path {
            return;
        }
        for k in dropped {
            self.globals.unset(&k);
        }
        self.globals.set(&key, toml::Value::String(path));
        self.globals.set(model::SAVED_KEYS_LIST, toml::Value::Array(order.into_iter().map(toml::Value::String).collect()));
        self.save_globals();
    }

    /// Carries out what the person did in the panel.
    fn ssh_panel_act(&mut self, e: PanelEvent) {
        enum Then {
            Stay,
            Close,
            System,
            Open,
        }
        let Some((w, mut p)) = self.ssh_panel.take() else { return };
        let mut requests = Vec::new();
        let mut then = Then::Stay;
        let home = p.host.as_ref().and_then(|h| self.panel_home(h));
        match e {
            PanelEvent::None | PanelEvent::Search(_) => {}
            PanelEvent::Host(i) => {
                if let Some(h) = p.hosts.get(i).cloned() {
                    requests = p.select_host(&h, self.panel_saved(&h), self.panel_home(&h));
                }
            }
            PanelEvent::Back => requests = p.back(),
            PanelEvent::Forward => requests = p.forward(),
            PanelEvent::Up => requests = p.go_up(),
            PanelEvent::Home => requests = p.go_home(home.as_deref()),
            PanelEvent::Refresh => requests = p.refresh(),
            PanelEvent::ToggleHidden => requests = p.toggle_hidden(),
            PanelEvent::GoTo(path) => {
                if let Some(h) = p.host.clone() {
                    requests = p.navigate(&h, &path, true);
                }
            }
            PanelEvent::GoToTyped(text) => {
                if let Some(h) = p.host.clone() {
                    let path = p.expand_path(&text, home.as_deref());
                    requests = p.navigate(&h, &path, true);
                }
            }
            PanelEvent::Complete(text) => requests = p.completions(&text, home.as_deref()),
            PanelEvent::Sort(c) => p.sort_by(c),
            PanelEvent::Click { path, ctrl, shift } => p.click(&path, ctrl, shift),
            PanelEvent::DoubleClick(path) => match p.double_click(&path) {
                DoubleClick::Navigate(r) => requests = r,
                DoubleClick::Open => {
                    p.click(&path, false, false);
                    then = Then::Open;
                }
            },
            PanelEvent::SystemPanel => then = Then::System,
            PanelEvent::Open => then = Then::Open,
            PanelEvent::Cancel => then = Then::Close,
        }
        if matches!(then, Then::Open) && !p.open_enabled() {
            then = Then::Stay;
        }
        w.request_redraw();
        let chosen = p.chosen();
        let host = p.host.clone();
        self.ssh_panel = Some((w, p));
        match then {
            Then::Stay => self.ssh_panel_send(requests),
            Then::Close => self.ssh_panel_close(),
            Then::System => {
                self.ssh_panel_close();
                // The system panel's result reaches no handler in Download Files (iTerm2's, kept).
                let _ = rfd::FileDialog::new().pick_files();
            }
            Then::Open => {
                self.ssh_panel_close();
                if let Some(host) = host {
                    self.ssh_download_files(host, chosen);
                }
            }
        }
    }

    /// Open: each chosen item downloads, chunked, into a fresh batch folder under `%TEMP%` (P12);
    /// the ones that arrive are revealed together and each failure is notified (`FP:1828-1880`,
    /// `OP:220-237`). One host text is used for the whole transfer (P14).
    fn ssh_download_files(&mut self, host: Identity, items: Vec<RemoteFile>) {
        if items.is_empty() {
            return;
        }
        let host_text = if host.hostname.is_empty() { host.host.clone() } else { host.hostname.clone() };
        let id = self.ssh_next_file_id;
        self.ssh_next_file_id += 1;
        self.ssh_batches.insert(id, Batch::new(items.len()));
        let uuid = || uuid::Uuid::new_v4().to_string().to_uppercase();
        let folder = match make_batch_folder(&std::env::temp_dir(), &uuid()) {
            Ok(f) => f,
            Err(e) => {
                for f in &items {
                    self.ssh_batch_item_ended(id, &f.name(), Err(e.to_string()));
                }
                return;
            }
        };
        let pane = self.pane_for_identity(&host);
        for f in items {
            let name = f.name();
            // A host gone before its item starts.
            let Some(pane) = pane else {
                self.ssh_batch_item_ended(id, &name, Err(model::connection_lost(&shown_name(&host_text))));
                continue;
            };
            // A name the shell reads on listing is refused before the first fetch (P1).
            if let Some(why) = local::refused_name(&name) {
                self.ssh_batch_item_ended(id, &name, Err(why.to_string()));
                continue;
            }
            let dest = batch_dest(&folder, &f);
            let temp = folder.join(local::temp_name(&uuid()));
            if let Err(e) = std::fs::OpenOptions::new().write(true).create_new(true).open(&temp) {
                self.ssh_batch_item_ended(id, &name, Err(e.to_string()));
                continue;
            }
            let l = Local::new(Place::Temp { temp, dest, batch: id }, pane, &f.absolute_path, &host_text, host.username.as_deref());
            let (job, actions) = self.ssh_transfers.download_chunked(pane, &f.absolute_path, &host_text);
            self.ssh_locals.insert(job, l);
            self.ssh_transfer_apply(actions);
        }
    }

    /// An item of batch `batch` ended: `Ok` with where it arrived, or why it failed. The last
    /// one out reveals what arrived and notifies each failure.
    pub(super) fn ssh_batch_item_ended(&mut self, batch: u64, name: &str, result: Result<PathBuf, String>) {
        let Some(b) = self.ssh_batches.get_mut(&batch) else { return };
        let last = match result {
            Ok(path) => b.arrived(path),
            Err(why) => b.failed(name, &why),
        };
        if !last {
            return;
        }
        let Some(b) = self.ssh_batches.remove(&batch) else { return };
        for (name, why) in &b.failed {
            Self::ssh_notify(&local::download_failed(name), why);
        }
        // Two items of one name end in one place: shown once.
        let mut done: Vec<PathBuf> = Vec::new();
        for p in b.done {
            if !done.contains(&p) {
                done.push(p);
            }
        }
        if let Err(e) = reveal_all(&done) {
            self.banners.set("action", format!("Could not show the downloaded files: {}", shown_name(&e)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_file::Kind;

    fn remote(path: &str, folder: bool) -> RemoteFile {
        RemoteFile { kind: if folder { Kind::Folder } else { Kind::File { size: Some(1) } }, absolute_path: path.into(), permissions: None, parent_permissions: None, ctime: None, mtime: None }
    }

    #[test]
    fn download_files_batch_follows_iterm2() {
        let t = Path::new("T");
        assert_eq!(batch_dest(t, &remote("/h/a.txt", false)), t.join("a.txt"));
        assert_eq!(batch_dest(t, &remote("/h/d", true)), t.join("d"));
        // A remote name Windows cannot hold stays one component of the batch folder.
        for name in ["..", ".", "a\\..\\b", "x.txt:Zone.Identifier", "\\\\?\\C:\\x", "CON.txt", "com1.tar.gz", "CONIN$", "COM\u{b9}", "CON .txt", "name. "] {
            let d = batch_dest(t, &remote(&format!("/h/{name}"), false));
            assert_eq!(d.parent(), Some(t), "{name:?} gave {d:?}");
            assert_eq!(d.file_name().unwrap().to_string_lossy(), crate::inline_file::clean_name(name), "{name:?}");
        }
        // Two items of one name map to one place: the last one finished wins.
        assert_eq!(batch_dest(t, &remote("/a/x.txt", false)), batch_dest(t, &remote("/b/x.txt", false)));
        // A batch of three with one failure reveals the two that arrived and reports one.
        let mut b = Batch::new(3);
        assert!(!b.arrived(t.join("a")));
        assert!(!b.failed("b", "File not found"));
        assert!(b.arrived(t.join("c")));
        assert_eq!(b.done, [t.join("a"), t.join("c")]);
        assert_eq!(b.failed, [("b".to_string(), "File not found".to_string())]);
    }

    #[test]
    fn download_files_menu_follows_every_window() {
        // P21: each window's bar is set once from the registry and again only on a change.
        assert_eq!(download_files_update(None, false), Some(false));
        assert_eq!(download_files_update(None, true), Some(true));
        assert_eq!(download_files_update(Some(false), true), Some(true));
        assert_eq!(download_files_update(Some(true), false), Some(false));
        assert_eq!(download_files_update(Some(true), true), None);
        assert_eq!(download_files_update(Some(false), false), None);
        // The cache is per window and the tick refreshes the live one, so a window that becomes
        // live converges on its next tick; a new bar starts unset.
        let src = |f: &str| std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("app").join(f)).unwrap();
        assert!(src("windows_ui.rs").contains("swap(&mut self.download_files_shown, &mut s.download_files_shown)"), "download_files_shown must be in swap_live");
        let tick = src("ssh_app.rs");
        let body = &tick[tick.find("fn tick_ssh").unwrap()..];
        assert!(body.contains("self.refresh_download_files_menu()"), "tick_ssh must refresh Download Files");
        assert!(src("menu.rs").contains("self.download_files_shown = None;"), "a new bar must be set again");
    }

    #[test]
    fn later_item_of_one_name_replaces_the_earlier() {
        let root = std::env::temp_dir().join(format!("tandem-batch-last-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // A folder item, then a later folder item of the same name: only the later one's files.
        let d = batch_dest(&root, &remote("/a/d", true));
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("old.txt"), b"1").unwrap();
        std::fs::write(d.join("sub").join("old2.txt"), b"1").unwrap();
        clear_earlier(&d).unwrap();
        assert!(!d.exists(), "the earlier folder is gone before the later one lands");
        // A file item, then a folder item of the same name, and the reverse.
        let x = batch_dest(&root, &remote("/a/x", false));
        std::fs::write(&x, b"1").unwrap();
        clear_earlier(&x).unwrap();
        assert!(!x.exists());
        std::fs::create_dir(&x).unwrap();
        clear_earlier(&x).unwrap();
        assert!(!x.exists());
        // Nothing there is not an error.
        clear_earlier(&root.join("none")).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn batch_folder_is_fresh() {
        // P12: a new `Tandem-<UUID>` folder under the temp root, made with create_dir.
        let root = std::env::temp_dir().join(format!("tandem-batch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let f = make_batch_folder(&root, "U1").unwrap();
        assert_eq!(f, root.join("Tandem-U1"));
        assert!(f.is_dir() && std::fs::read_dir(&f).unwrap().next().is_none());
        assert_ne!(f, root, "never the temp root itself");
        // The items go inside it.
        assert_eq!(batch_dest(&f, &remote("/h/a.txt", false)).parent(), Some(f.as_path()));
        // One that exists already is never adopted: a folder (maybe a planted junction) or a file.
        assert!(make_batch_folder(&root, "U1").is_err());
        std::fs::write(root.join("Tandem-U2"), b"x").unwrap();
        assert!(make_batch_folder(&root, "U2").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}

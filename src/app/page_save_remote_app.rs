//! Save Page As to a folder on an SSH host, the app side (TWEAKS 7b). The pure parts are
//! `page_save_remote` (the plan, the upload as a state machine, the cleanup ledger), `save_stage` (the
//! private staging folder) and the folder picker mode of `ssh_panel`; this file connects them.
//!
//! The flow, with the owner of each step:
//! - `save_page_as` offers "Remote Folder..." only for a page of a session the person has (an
//!   `iterm2-ssh:` page whose host has a framing conductor), then the picker (the remote file panel
//!   in folder mode) chooses the host folder and one question confirms it;
//! - the page is saved into a private staging folder by the one local saver (`page_saver_app`), so
//!   its caps, names and create-new writes apply unchanged;
//! - the finished tree is planned (`page_save_remote::plan`) and uploaded through the session's
//!   file owner only (`ssh_files`: typed requests, base64 chunks, never a shell string): `stat` to
//!   pick a free folder name, `mkdir`, then each file through the transfer engine's upload (a hidden
//!   temp, then the move into place);
//! - every end (done, error, cancel, pane closed, connection lost) removes the staging folder and,
//!   when unfinished, only the paths this save made on the host.

use super::scripts_alerts::{ask, Then};
use super::*;
use crate::page_save_remote::{self as psr, Reply, Req, Upload};
use crate::page_saver;
use crate::ssh_args::Identity;
use crate::ssh_files::{FileSub, ScpPath};
use crate::ssh_page;
use crate::ssh_panel::Panel;
use crate::ssh_panel_ui::SshPanelWindow;
use super::files_ui::shown_name;
use crate::ssh_transfer::{Action, Outcome};
use crate::warnings::Kind;
use std::path::PathBuf;

pub(super) const WHERE_HEADING: &str = "Save Page As";
pub(super) const WHERE_TEXT: &str = "Where do you want to save this page?";
pub(super) const WHERE_REMOTE: &str = "Remote Folder\u{2026}";
pub(super) const WHERE_LOCAL: &str = "This Computer\u{2026}";
/// The banner of a running upload; it moves at most this often.
const BANNER: &str = "save-remote";
const BANNER_EVERY: std::time::Duration = std::time::Duration::from_millis(250);

/// What a request of the upload waits for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Step {
    Stat,
    Mkdir,
}

/// The folder picker while it shows: for which page, and on which session.
#[derive(Clone, Debug)]
pub(super) struct SavePick {
    pub pane: PaneId,
    pub url: String,
    pub epoch: u64,
    pub identity: Identity,
}

/// One remote save of a browser pane, from the staging save to the end of the upload.
pub(super) struct RemoteSave {
    id: u64,
    /// The session pane whose file owner takes the requests.
    conductor: PaneId,
    to: ScpPath,
    stage_root: PathBuf,
    page_dir: PathBuf,
    url: String,
    /// None while the page is still being saved into the staging folder.
    upload: Option<Upload>,
    /// The transfer engine's upload in flight.
    job: Option<u64>,
    shown: Option<std::time::Instant>,
}

impl App {
    /// The session pane and identity an `iterm2-ssh:` page belongs to, when that session still frames.
    pub(super) fn save_remote_session(&self, url: &str) -> Option<(PaneId, Identity)> {
        let (host, user, port, _) = ssh_page::parse(url)?;
        self.ssh_registry.identities().into_iter().filter(|id| ssh_page::identity_matches(id, &host, user.as_deref(), port)).find_map(|id| self.pane_for_identity(&id).map(|p| (p, id)))
    }

    /// Whether Save Page As offers a remote folder for this page.
    pub(super) fn save_remote_offered(&self, pane: PaneId, url: &str) -> bool {
        !self.page_save_remote.contains_key(&pane) && self.save_remote_session(url).is_some_and(|(conductor, id)| {
            let to = ScpPath { path: "/".into(), hostname: if id.hostname.is_empty() { id.host.clone() } else { id.hostname.clone() }, username: id.username.clone().unwrap_or_default() };
            self.transfer_pane(conductor, &to)
        })
    }

    /// The question where to save: the host or this computer.
    pub(super) fn save_where_ask(&mut self, pane: PaneId, url: String, epoch: u64) {
        let a = ask(WHERE_HEADING, WHERE_TEXT, &[WHERE_REMOTE, WHERE_LOCAL, crate::api_auth::CANCEL], None, Kind::Persistent, Then::SavePageWhere { pane, url, epoch });
        self.ask_scripts(a);
    }

    pub(super) fn save_where_answered(&mut self, pane: PaneId, url: String, epoch: u64, index: usize) {
        match index {
            0 => self.ssh_save_pick_pending = Some(SavePickWait { pane, url, epoch }),
            1 => self.save_page_local(pane, url, epoch),
            _ => {}
        }
        self.redraw();
    }

    /// The picker opens on the next tick (a window needs the event loop).
    pub(super) fn open_save_pick(&mut self, event_loop: &ActiveEventLoop) {
        let Some(wait) = self.ssh_save_pick_pending.take() else { return };
        if self.ssh_panel.is_some() {
            if let Some((w, _)) = &self.ssh_panel {
                w.focus();
            }
            return self.banners.set("action", "Close the open file panel first.".to_string());
        }
        let Some((conductor, identity)) = self.save_remote_session(&wait.url) else {
            return self.page_save_alert(&page_saver::save_failed_text(crate::ssh_files::FileError::ConnectionClosed.text()));
        };
        let Some(g) = self.gpu.as_ref() else { return };
        let window = match SshPanelWindow::open_titled(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, crate::ssh_panel::CHOOSE_TITLE) {
            Ok(w) => w,
            Err(e) => return self.banners.set("window", format!("Could not open the window: {e}")),
        };
        let cwd = self.panes.get(&conductor).and_then(|p| p.own().shell.cwd.clone()).and_then(|c| crate::ssh_jumps::clean_directory(&c));
        let mut panel = Panel::folder_picker(identity.clone());
        let requests = panel.start(&|h| cwd.clone().or_else(|| self.panel_saved(h)), &|h| self.panel_home(h));
        self.ssh_panel = Some((window, panel));
        self.ssh_save_pick = Some(SavePick { pane: wait.pane, url: wait.url, epoch: wait.epoch, identity });
        self.ssh_panel_send(requests);
    }

    /// Choose in the picker: the folder is judged, then one question confirms the host and folder.
    pub(super) fn save_remote_chosen(&mut self, pick: SavePick, folder: String) {
        let Some(to) = crate::ssh_files::folder_target(&folder, &pick.identity) else {
            return self.page_save_alert(&page_saver::save_failed_text("That is not a folder path Termurion can save to."));
        };
        if self.page_save_epoch(pick.pane) != pick.epoch {
            return self.page_save_alert(&page_saver::save_failed_text(page_saver::NAVIGATED_AWAY));
        }
        let Some((conductor, _)) = self.save_remote_session(&pick.url).filter(|(_, id)| *id == pick.identity) else {
            return self.page_save_alert(&page_saver::save_failed_text(crate::ssh_files::FileError::ConnectionClosed.text()));
        };
        if !self.transfer_pane(conductor, &to) {
            return self.page_save_alert(&page_saver::save_failed_text(crate::ssh_files::FileError::ConnectionClosed.text()));
        }
        // The path, host and user came from the remote: cleaned before the person reads them.
        let shown = ScpPath { path: shown_name(&to.path), hostname: shown_name(&to.hostname), username: shown_name(&to.username) };
        let text = psr::confirm_text(&shown_name(&psr::folder_base(&pick.url)), &shown.user_at_host(), &shown.path);
        let a = ask("", &text, &[crate::api_auth::OK, crate::api_auth::CANCEL], None, Kind::Persistent, Then::SaveRemote { pane: pick.pane, url: pick.url, epoch: pick.epoch, to });
        self.ask_scripts(a);
    }

    /// OK on the question: the page is saved into the staging folder first.
    pub(super) fn save_remote_confirmed(&mut self, pane: PaneId, url: String, epoch: u64, to: ScpPath) {
        if self.page_saves.contains_key(&pane) || self.page_save_remote.contains_key(&pane) {
            return;
        }
        let Some((conductor, _)) = self.save_remote_session(&url) else {
            return self.page_save_alert(&page_saver::save_failed_text(crate::ssh_files::FileError::ConnectionClosed.text()));
        };
        if !self.transfer_pane(conductor, &to) {
            return self.page_save_alert(&page_saver::save_failed_text(crate::ssh_files::FileError::ConnectionClosed.text()));
        }
        let uuid = uuid::Uuid::new_v4().to_string().to_uppercase();
        let stage_root = match crate::save_stage::make_root(&std::env::temp_dir(), &uuid) {
            Ok(r) => r,
            Err(e) => return self.page_save_alert(&page_saver::save_failed_text(&e.to_string())),
        };
        let page_dir = stage_root.join(psr::folder_base(&url));
        self.page_save_seq += 1;
        let id = self.page_save_seq;
        self.page_save_remote.insert(pane, RemoteSave { id, conductor, to, stage_root, page_dir: page_dir.clone(), url: url.clone(), upload: None, job: None, shown: None });
        self.banners.set(BANNER, "Saving the page...".to_string());
        self.start_page_save(pane, page_dir, url, epoch);
        // The staging save did not start (an alert says why): nothing is left behind.
        if !self.page_saves.contains_key(&pane) {
            self.save_remote_end(pane);
        }
    }

    /// Removes the staging folder (off the UI thread; retried, as the file worker may still be
    /// finishing a write) and forgets the save.
    fn save_remote_unstage(&mut self, pane: PaneId) {
        let Some(rs) = self.page_save_remote.remove(&pane) else { return };
        self.banners.clear(BANNER);
        let root = rs.stage_root;
        let _ = std::thread::Builder::new().name("save-unstage".into()).spawn(move || {
            let _ = crate::save_stage::remove_root(&root, 25, std::time::Duration::from_millis(400));
        });
    }

    /// The save is over (any way): the staging folder goes.
    fn save_remote_end(&mut self, pane: PaneId) {
        self.save_remote_unstage(pane);
    }

    /// The staging save failed (the saver's alert shows): the staging folder goes.
    pub(super) fn save_remote_staged_failed(&mut self, pane: PaneId) {
        self.save_remote_unstage(pane);
    }

    /// The person cancelled, or the pane is gone, or Save Page As was chosen again: an upload stops
    /// and removes what it made; a staging save loses its folder (the saver's own cancel ran first).
    pub(super) fn save_remote_cancel(&mut self, pane: PaneId) {
        let Some(rs) = self.page_save_remote.get_mut(&pane) else { return };
        match rs.upload.as_mut() {
            None => self.save_remote_unstage(pane),
            Some(up) => {
                let reqs = up.cancel(psr::CANCELED);
                self.save_remote_exec(pane, reqs);
            }
        }
    }

    /// The saver finished the staging tree (`Act::Reveal` for a remote save): plan it and upload.
    pub(super) fn save_remote_finish(&mut self, pane: PaneId) {
        let Some(rs) = self.page_save_remote.get(&pane) else { return };
        let (page_dir, url, to) = (rs.page_dir.clone(), rs.url.clone(), rs.to.clone());
        let planned = crate::save_stage::walk(&page_dir).and_then(|tree| psr::plan(&tree));
        let plan = match planned {
            Ok(p) => p,
            Err(why) => {
                self.save_remote_unstage(pane);
                return self.page_save_alert(&page_saver::save_failed_text(&why));
            }
        };
        let mut up = Upload::new(plan, &to.path, &psr::folder_base(&url));
        let reqs = up.start();
        if let Some(rs) = self.page_save_remote.get_mut(&pane) {
            rs.upload = Some(up);
        }
        self.save_remote_exec(pane, reqs);
    }

    /// Carries out what the upload asked. A request that cannot go out is answered at once (the
    /// owner is gone), which can end the save; so the answers are fed back from a queue.
    fn save_remote_exec(&mut self, pane: PaneId, reqs: Vec<Req>) {
        let mut work: std::collections::VecDeque<Req> = reqs.into();
        while let Some(req) = work.pop_front() {
            let Some(rs) = self.page_save_remote.get(&pane) else { return };
            let (conductor, id, page_dir) = (rs.conductor, rs.id, rs.page_dir.clone());
            let mut fed: Option<Reply> = None;
            match req {
                Req::Stat(path) => {
                    if !self.ssh_send(conductor, FileSub::Stat { path }, false, super::ssh_files_app::Waiter::SaveRemote { pane, id, step: Step::Stat }) {
                        fed = Some(Reply::Stat(-1));
                    }
                }
                Req::Mkdir(path) => {
                    if !self.ssh_send(conductor, FileSub::Mkdir { path }, false, super::ssh_files_app::Waiter::SaveRemote { pane, id, step: Step::Mkdir }) {
                        fed = Some(Reply::Made(-1));
                    }
                }
                Req::Put { rel, dest, size: _ } => {
                    fed = self.save_remote_put(pane, conductor, &page_dir, &rel, &dest);
                }
                Req::Abort => {
                    // The job stays in `ssh_save_jobs` until its Done comes, which answers nothing.
                    if let Some(job) = self.page_save_remote.get_mut(&pane).and_then(|r| r.job.take()) {
                        let actions = self.ssh_transfers.fail(job, psr::CANCELED);
                        self.ssh_transfer_apply(actions);
                    }
                }
                Req::Rm(path) => {
                    // A host that is gone cannot be cleaned; what it holds is only this save's partial copy.
                    self.ssh_send(conductor, FileSub::Rm { path, recursive: false }, false, super::ssh_files_app::Waiter::Ignore);
                }
                Req::Progress { done, total } => {
                    let due = self.page_save_remote.get(&pane).and_then(|r| r.shown).is_none_or(|t| t.elapsed() >= BANNER_EVERY);
                    if due || done == total {
                        if let Some(r) = self.page_save_remote.get_mut(&pane) {
                            r.shown = Some(std::time::Instant::now());
                        }
                        let host = shown_name(&self.page_save_remote.get(&pane).map(|r| r.to.hostname.clone()).unwrap_or_default());
                        self.banners.set(BANNER, format!("Saving the page to {host}: {done} of {total} files"));
                        self.redraw();
                    }
                }
                Req::Finished(result) => {
                    let to = self.page_save_remote.get(&pane).map(|r| r.to.clone());
                    self.save_remote_end(pane);
                    match result {
                        Ok(root) => {
                            let host = to.map(|t| t.user_at_host()).unwrap_or_default();
                            Self::ssh_notify("Saved the page", &format!("{host}:{root}"));
                        }
                        Err(why) if why == psr::CANCELED => {}
                        Err(why) => self.page_save_alert(&page_saver::save_failed_text(&why)),
                    }
                    self.redraw();
                    return;
                }
            }
            if let Some(reply) = fed {
                let more = self.page_save_remote.get_mut(&pane).and_then(|r| r.upload.as_mut()).map(|u| u.reply(reply)).unwrap_or_default();
                work.extend(more);
            }
        }
    }

    /// Starts the engine's upload of one staged file. A file that cannot be read or sent is the
    /// answer at once.
    fn save_remote_put(&mut self, pane: PaneId, conductor: PaneId, page_dir: &std::path::Path, rel: &str, dest: &str) -> Option<Reply> {
        let data = match crate::save_stage::read(page_dir, rel) {
            Ok(d) => d,
            Err(e) => return Some(Reply::Put(Err(format!("\"{}\" could not be read: {e}", shown_name(rel))))),
        };
        let uuid = uuid::Uuid::new_v4().to_string().to_uppercase();
        let (job, actions) = self.ssh_transfers.upload(conductor, dest, data, &uuid);
        self.ssh_save_jobs.insert(job, pane);
        if let Some(r) = self.page_save_remote.get_mut(&pane) {
            r.job = Some(job);
        }
        self.ssh_transfer_apply(actions);
        None
    }

    /// An action of an engine job that belongs to a remote save: its progress is the save's banner
    /// and its end is the answer to the upload. True when the action was taken here; a `Send`
    /// goes out like any other.
    pub(super) fn ssh_save_job(&mut self, action: &Action) -> bool {
        let job = match action {
            Action::Send { .. } | Action::Fire { .. } => return false,
            Action::Size { job, .. } | Action::Write { job, .. } | Action::Sent { job, .. } | Action::Unzip { job } | Action::Done { job, .. } => *job,
        };
        let Some(&pane) = self.ssh_save_jobs.get(&job) else { return false };
        if let Action::Done { result, .. } = action {
            self.ssh_save_jobs.remove(&job);
            // A job that is not the one in flight (a cancelled one ending) answers nothing.
            if self.page_save_remote.get(&pane).is_some_and(|r| r.job == Some(job)) {
                if let Some(r) = self.page_save_remote.get_mut(&pane) {
                    r.job = None;
                }
                let reply = Reply::Put(match result {
                    Ok(Outcome::Uploaded { remote_name }) => Ok(remote_name.clone()),
                    Ok(_) => Err(crate::ssh_files::GARBAGE.to_string()),
                    Err(why) => Err(why.clone()),
                });
                self.save_remote_feed(pane, reply);
            }
        }
        true
    }

    /// An answer of a request of the upload: `status` is the framer's. A reply of an earlier save
    /// of the pane (`id`) is dropped.
    pub(super) fn save_remote_replied(&mut self, pane: PaneId, id: u64, step: Step, status: i32) {
        if !self.page_save_remote.get(&pane).is_some_and(|r| r.id == id) {
            return;
        }
        self.save_remote_feed(pane, match step {
            Step::Stat => Reply::Stat(status),
            Step::Mkdir => Reply::Made(status),
        });
    }

    fn save_remote_feed(&mut self, pane: PaneId, reply: Reply) {
        let reqs = self.page_save_remote.get_mut(&pane).and_then(|r| r.upload.as_mut()).map(|u| u.reply(reply)).unwrap_or_default();
        self.save_remote_exec(pane, reqs);
    }
}

/// A picker waiting for the next tick to open.
#[derive(Clone, Debug)]
pub(super) struct SavePickWait {
    pub pane: PaneId,
    pub url: String,
    pub epoch: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prod(src: &'static str) -> &'static str {
        &src[..src.find("\n#[cfg(test)]\nmod tests").unwrap_or(src.len())]
    }

    /// Only a page of a session the person has offers a remote folder: the URL selects among the open
    /// sessions, and the pane's file owner must be able to take a transfer to that host.
    #[test]
    fn save_offers_remote_only_when_allowed() {
        for local in ["https://example.com/", "http://10.0.0.1/x", "file:///C:/a.html", "iterm2-about:settings", "iterm2-ssh://", "", "iterm2-ssh://bad host/x"] {
            assert!(ssh_page::parse(local).is_none() || local.is_empty(), "{local}");
        }
        assert!(ssh_page::parse("iterm2-ssh://host.example/home/u/a.html").is_some());
        let me = prod(include_str!("page_save_remote_app.rs"));
        let offered = &me[me.find("pub(super) fn save_remote_offered").unwrap()..me.find("/// The question where to save").unwrap()];
        assert!(offered.contains("self.save_remote_session(url)") && offered.contains("self.transfer_pane(conductor, &to)") && offered.contains("!self.page_save_remote.contains_key(&pane)"));
        let saver = prod(include_str!("page_saver_app.rs"));
        let menu = &saver[saver.find("pub(super) fn save_page_as").unwrap()..saver.find("pub(super) fn save_page_local").unwrap()];
        assert!(menu.find("self.save_remote_offered(pane, &url)").unwrap() < menu.find("self.save_page_local(pane, url, epoch);").unwrap(), "the local dialog is the default");
        // The local route is the one that was: the dialog and the start of the saver.
        let local = &saver[saver.find("pub(super) fn save_page_local").unwrap()..saver.find("pub(super) fn start_page_save").unwrap()];
        assert!(local.contains("crate::localfs::save_file(dialog)") && local.contains("self.start_page_save(pane, picked.into_path_buf(), url, epoch);"));
        // Both questions are cancels when closed, and are never remembered or capped as a flood.
        let alerts = prod(include_str!("scripts_alerts.rs"));
        assert!(alerts.contains("Then::SavePageWhere { .. } => Answer::Button(2)"));
        assert!(alerts.contains("Then::SshUpload { .. } | Then::SaveRemote { .. } | Then::BindScheme { .. }"));
        assert!(!alerts.contains("Then::SaveRemote { .. } | Then::SavePageWhere") && !alerts.contains("Then::SavePageWhere { .. } | Then::SaveRemote"));
    }

    /// The question names the difference, shows the host and folder cleaned, and OK is the only way on.
    #[test]
    fn confirm_names_the_difference() {
        let me = prod(include_str!("page_save_remote_app.rs"));
        let chosen = &me[me.find("pub(super) fn save_remote_chosen").unwrap()..me.find("/// OK on the question").unwrap()];
        assert!(chosen.contains("psr::confirm_text(") && chosen.contains("shown_name(&to.path)") && chosen.contains("shown_name(&to.hostname)") && chosen.contains("shown_name(&to.username)"));
        assert!(chosen.contains("crate::ssh_files::folder_target(&folder, &pick.identity)"), "the folder is judged by scp_path");
        assert!(chosen.contains("&[crate::api_auth::OK, crate::api_auth::CANCEL]"));
        let alerts = prod(include_str!("scripts_alerts.rs"));
        assert!(alerts.contains("Then::SaveRemote { pane, url, epoch, to } => {\n                if index == 0 {\n                    self.save_remote_confirmed(pane, url, epoch, to);"));
        let t = psr::confirm_text("example.com", "u@h", "/srv/pages");
        assert!(t.contains("web") && t.contains("will not mark") && t.contains("u@h:/srv/pages"));
    }

    /// Nothing the host is sent is a shell string, and the upload has one owner.
    #[test]
    fn remote_save_has_one_owner_and_no_shell() {
        let me = prod(include_str!("page_save_remote_app.rs"));
        for banned in ["Command::new(", "scp.exe", "ssh.exe", "std::process", "format!(\"rm", "format!(\"mkdir", "sh -c"] {
            assert!(!me.contains(banned), "{banned}");
        }
        assert!(me.contains("FileSub::Stat { path }") && me.contains("FileSub::Mkdir { path }") && me.contains("FileSub::Rm { path, recursive: false }"), "typed requests only, rm never recursive");
        assert!(!me.contains("recursive: true"));
        assert!(me.contains("self.ssh_transfers.upload(conductor, dest, data, &uuid)"));
        // The engine's actions of an upload are the save's: taken before the transfer panel's own handling.
        let files = prod(include_str!("ssh_files_app.rs"));
        let apply = &files[files.find("pub(super) fn ssh_transfer_apply").unwrap()..files.find("/// A download's own temp file").unwrap()];
        assert!(apply.find("self.ssh_page_job(&action)").unwrap() < apply.find("self.ssh_save_job(&action)").unwrap() && apply.find("self.ssh_save_job(&action)").unwrap() < apply.find("match action {").unwrap());
        assert!(files.contains("Waiter::SaveRemote { pane: view, id, step } => self.save_remote_replied(view, id, step, status)"));
        // A cancel reaches the upload from the menu item, the saver's cancel and a closed pane.
        let saver = prod(include_str!("page_saver_app.rs"));
        let cancel = &saver[saver.find("pub(super) fn page_save_cancel").unwrap()..];
        assert!(cancel[..cancel.find("\n    }\n").unwrap()].contains("self.save_remote_cancel(pane);"));
        assert!(saver.contains("self.page_save_remote.contains_key(&pane) {\n            self.page_save_cancel(pane);"));
        // The staging folder is removed at every end, and a save is never started from a stale id.
        assert!(me.contains("fn save_remote_unstage") && me.contains("crate::save_stage::remove_root("));
        assert!(me.contains("r.id == id"), "a reply of an earlier save is dropped");
    }
}

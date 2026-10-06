//! File requests over a pane's framing conductor (milestone 14d): each is sent with an id
//! Tandem chose, remembered with what is waiting for it, and routed when its reply comes
//! (`CD:1953-1960`, `CD:2273-2291`). Later tasks add the waiters that need the transfer
//! engine, the panel, suggestions and search; this is the one dispatch point (`ssh_route`).

use super::*;
use crate::conductor::{Command, Conductor, Handler};
use crate::ssh_files::{self, FileSub, ScpPath};
use crate::ssh_local::{self as local, Local, Place};
use crate::ssh_transfer::{has_room, Action, Outcome};
use super::files_ui::shown_name;
use super::ssh_panel_app::Landing;

/// What waits for one request's answer.
#[derive(Clone, Debug, PartialEq)]
#[allow(dead_code)]
pub(super) enum Waiter {
    /// Nobody: the answer is dropped (a fire-and-forget request, or one whose caller is gone).
    Ignore,
    /// A step of the transfer engine's job `job`, tagged `tag` (Task 3).
    Transfer { job: u64, tag: u64 },
    /// The remote file panel's request of generation `generation` (Task 7).
    Panel { generation: u64, op: crate::ssh_panel::Op },
    /// The Composer's suggestion fetch of `generation`, for the typed word `name` (Task 8).
    Suggest { generation: u64, inputs: crate::ssh_files::SuggestInputs, name: String },
    /// The panel's search start of `generation`: its answer is the search id (Task 8).
    SearchStart { generation: u64 },
    /// An acknowledgement of a result of search `id`: its answer frees a place in the window.
    SearchAck { id: String },
}

// Used by the transfer, panel, suggestion and search tasks that follow.
#[allow(dead_code)]
impl App {
    /// The pane's innermost conductor, only while its framer runs to answer (P7: framing and
    /// not unhooked; `Conductor::framing` stays true after a disconnect, and a request sent
    /// then would queue forever).
    pub(super) fn framing(&self, pane: PaneId) -> Option<&Conductor> {
        self.panes.get(&pane)?.conductor.as_deref().filter(|c| c.frames_keys())
    }

    pub(super) fn framing_mut(&mut self, pane: PaneId) -> Option<&mut Conductor> {
        self.panes.get_mut(&pane)?.conductor.as_deref_mut().filter(|c| c.frames_keys())
    }

    /// Sends a file request. False, with nothing sent and nothing remembered, when the pane
    /// has no framing conductor: the caller answers its own waiter (a -1 reply), or a job
    /// would wait forever.
    pub(super) fn ssh_send(&mut self, pane: PaneId, sub: FileSub, high: bool, waiter: Waiter) -> bool {
        let id = self.ssh_next_file_id;
        let Some(c) = self.framing_mut(pane) else { return false };
        c.send(Command::FramerFile(sub), Handler::File { id, lines: Vec::new() }, high);
        self.ssh_next_file_id += 1;
        self.ssh_waiters.insert((pane, id), waiter);
        // The write goes out through the effect loop.
        self.ssh_apply(pane);
        true
    }

    /// Cancels the queued requests `pred` matches (a request already written is not
    /// recalled). Each comes back as a -1 reply when its turn comes, and is routed like any other.
    pub(super) fn ssh_cancel_queued(&mut self, pane: PaneId, pred: &dyn Fn(&FileSub) -> bool) {
        let Some(c) = self.framing_mut(pane) else { return };
        c.cancel_enqueued(&|cmd| matches!(cmd, Command::FramerFile(sub) if pred(sub)));
        self.ssh_apply(pane);
    }

    /// Takes the answers the pane's conductors gave and routes them.
    pub(super) fn ssh_files_drain(&mut self, pane: PaneId) {
        let done = self.panes.get_mut(&pane).map(|p| p.take_ssh_files()).unwrap_or_default();
        for (id, content, status) in done {
            self.ssh_file_done(pane, id, content, status);
        }
    }

    /// An answer: its waiter is removed and told. An answer nobody waits for (a duplicate, or
    /// one for a request already failed) is ignored.
    pub(super) fn ssh_file_done(&mut self, pane: PaneId, id: u64, content: String, status: i32) {
        let Some(waiter) = self.ssh_waiters.remove(&(pane, id)) else { return };
        self.ssh_route(pane, waiter, content, status);
    }

    /// The one dispatch point: later tasks add their waiters here.
    pub(super) fn ssh_route(&mut self, pane: PaneId, waiter: Waiter, content: String, status: i32) {
        match waiter {
            Waiter::Ignore => {}
            Waiter::Transfer { job, tag } => {
                let actions = self.ssh_transfers.reply(job, tag, &content, status);
                self.ssh_transfer_apply(actions);
            }
            Waiter::Panel { generation, op } => self.ssh_panel_answered(generation, op, content, status),
            Waiter::Suggest { generation, inputs, name } => self.ssh_suggested(pane, generation, inputs, &name, &content, status),
            Waiter::SearchStart { generation } => self.ssh_search_started(pane, generation, &content, status),
            Waiter::SearchAck { id } => self.ssh_search_acked(pane, &id),
        }
    }

    /// Carries out the engine's actions: `Send` and `Fire` go to the conductor; the local side
    /// (`Size`, `Write`, `Sent`, `Unzip`, `Done`) is Task 4's and is only dropped here until it
    /// lands. `ssh_send` may route a reply before it returns (it runs the effect loop), so no
    /// borrow is held across it, and a send that finds no framing conductor is answered -1 at
    /// once or its job would wait forever.
    pub(super) fn ssh_transfer_apply(&mut self, actions: Vec<Action>) {
        let mut work: std::collections::VecDeque<Action> = actions.into();
        while let Some(action) = work.pop_front() {
            match action {
                Action::Send { job, tag, pane, sub, high } => {
                    // The job ended (a local failure, or a nested -1) earlier in this batch.
                    if !self.ssh_transfers.is_live(job) {
                        continue;
                    }
                    if !self.ssh_send(pane, sub, high, Waiter::Transfer { job, tag }) {
                        work.extend(self.ssh_transfers.reply(job, tag, "", -1));
                    }
                }
                Action::Fire { pane, sub } => {
                    self.ssh_send(pane, sub, false, Waiter::Ignore);
                }
                Action::Size { job, size } => {
                    if let Some(why) = self.ssh_size(job, size) {
                        work.extend(self.ssh_transfers.fail(job, &why));
                    }
                }
                Action::Write { job, bytes } => {
                    if !self.ssh_transfers.is_live(job) {
                        continue;
                    }
                    if let Err(why) = self.ssh_write(job, &bytes) {
                        work.extend(self.ssh_transfers.fail(job, &why));
                    }
                }
                Action::Sent { job, bytes } => self.ssh_sent(job, bytes),
                Action::Unzip { job } => {
                    if let Some(why) = self.ssh_unzip(job) {
                        work.extend(self.ssh_transfers.unzipped(job, Err(why)));
                    }
                }
                Action::Done { job, result } => self.ssh_transfer_done(job, result),
            }
        }
    }

    /// A download's own temp file is in this folder: where its room is measured.
    fn ssh_temp_dir(l: &Local) -> std::path::PathBuf {
        l.place.temp().parent().map(std::path::Path::to_path_buf).unwrap_or_default()
    }

    /// The first stat's size (P13: the last chance to refuse before a fetch): a file that
    /// would leave the drive under 1 GiB free fails "Not enough space on the drive". The
    /// reason when refused.
    fn ssh_size(&mut self, job: u64, size: Option<u64>) -> Option<String> {
        let l = self.ssh_locals.get_mut(&job)?;
        l.total = size;
        let size = size?;
        match local::free_space(&Self::ssh_temp_dir(l)) {
            Ok(free) if has_room(free, size) => None,
            Ok(_) => Some(local::NOT_ENOUGH_SPACE.to_string()),
            Err(e) => Some(e),
        }
    }

    /// Appends bytes to the job's own temp file (never the final name, R14de-4) and moves
    /// its banner. A stream with no claimed size is checked against the free space before each
    /// write. The reason when the write cannot go on.
    fn ssh_write(&mut self, job: u64, bytes: &[u8]) -> Result<(), String> {
        use std::io::Write;
        let Some(l) = self.ssh_locals.get_mut(&job) else { return Ok(()) };
        if l.total.is_none() {
            let free = local::free_space(&Self::ssh_temp_dir(l))?;
            if !has_room(free, bytes.len() as u64) {
                return Err(local::NOT_ENOUGH_SPACE.to_string());
            }
        }
        let mut f = crate::localfs::judge_io(l.place.temp()).and_then(|t| crate::localfs::open_custom(&t, |o| { o.append(true); })).map_err(|e| e.to_string())?;
        f.write_all(bytes).map_err(|e| e.to_string())?;
        self.ssh_progress(job, bytes.len() as u64);
        Ok(())
    }

    /// `n` more bytes moved: the banner follows a percent step of ten, or a megabyte when
    /// there is no total.
    fn ssh_progress(&mut self, job: u64, n: u64) {
        let Some(l) = self.ssh_locals.get_mut(&job) else { return };
        l.written += n;
        let step = l.percent().map_or(l.written >> 20, |p| p / 10);
        if l.shown == 0 || step > l.shown - 1 {
            l.shown = step + 1;
            let text = shown_name(&local::progress_text(l));
            self.banners.set("transfer", text);
            self.redraw();
        }
    }

    /// An upload's chunk was acknowledged (`Sent`): the banner moves.
    fn ssh_sent(&mut self, job: u64, bytes: u64) {
        self.ssh_progress(job, bytes);
    }

    /// Unzips a folder download's zip on a worker thread (P11: a remote-sized archive never
    /// freezes the UI); the answer comes through `ssh_unzip_tick`. It waits while an earlier
    /// item of its name is still landing (`Landings`). The reason when it cannot start.
    fn ssh_unzip(&mut self, job: u64) -> Option<String> {
        let l = self.ssh_locals.get(&job)?;
        let Place::Temp { dest, .. } = &l.place else { return Some("Internal error".to_string()) };
        let dest = dest.clone();
        if self.ssh_landings.claim(&dest, Landing::Unzip(job)).is_some() {
            self.ssh_unzip_start(job, dest);
        }
        None
    }

    /// Starts the unzip of `job`, which holds `dest`. False when the job is gone.
    fn ssh_unzip_start(&mut self, job: u64, dest: std::path::PathBuf) -> bool {
        let Some(l) = self.ssh_locals.get(&job) else { return false };
        let Place::Temp { temp, .. } = &l.place else { return false };
        let (temp, url) = (temp.clone(), l.url());
        let (tx, rx) = std::sync::mpsc::channel();
        let wake = self.notify.clone();
        let landed = dest.clone();
        std::thread::spawn(move || {
            // Last one wins in the batch folder: an earlier item of this name never merges in.
            let r = super::ssh_panel_app::clear_earlier(&dest).map_err(|e| e.to_string()).and_then(|()| local::unzip(&temp, &dest, &url));
            let _ = crate::localfs::judge_io(&temp).and_then(|t| crate::localfs::remove_file(&t));
            let _ = tx.send(r);
            wake();
        });
        self.ssh_unzips.push(crate::ssh_transfer::UnzipWait { job, dest: landed, rx });
        true
    }

    /// The landing at `dest` is done: the items waiting for it land in turn, until one unzips
    /// (it holds `dest` until its own answer) or none waits.
    fn ssh_land_next(&mut self, dest: &std::path::Path) {
        while let Some(next) = self.ssh_landings.next(dest) {
            match next {
                Landing::Unzip(job) => {
                    if self.ssh_unzip_start(job, dest.to_path_buf()) {
                        return;
                    }
                }
                Landing::File(job) => self.ssh_land_file(job),
            }
        }
    }

    /// Feeds the finished unzips to the engine.
    pub(super) fn ssh_unzip_tick(&mut self) {
        if self.ssh_unzips.is_empty() {
            return;
        }
        let results = crate::ssh_transfer::drain_unzips(&mut self.ssh_unzips);
        for (job, dest, r) in results {
            let actions = self.ssh_transfers.unzipped(job, r);
            self.ssh_transfer_apply(actions);
            self.ssh_land_next(&dest);
        }
    }

    /// A notification about a transfer; every remote string in it is cleaned first (P14).
    pub(super) fn ssh_notify(title: &str, body: &str) {
        let _ = crate::notify::show_with(&shown_name(title), &shown_name(body), false);
    }

    /// Win-click: downloads the remote file or folder `remote` of the pane's host into
    /// Downloads over the conductor (a folder arrives as `<name>.zip`, iTerm2's). The name is
    /// refused before the first fetch when it is a type the shell reads on listing (P1).
    pub(super) fn ssh_download(&mut self, pane: PaneId, remote: &str) {
        let Some(id) = self.framing(pane).map(|c| c.identity()) else { return };
        let host = if id.hostname.is_empty() { id.host.clone() } else { id.hostname.clone() };
        let mut l = Local::new(Place::Downloads { temp: std::path::PathBuf::new(), dir: std::path::PathBuf::new() }, pane, remote, &host, id.username.as_deref());
        let fail = |l: &Local, why: &str| Self::ssh_notify(&local::download_failed(&l.short_name), why);
        if let Some(why) = local::refused_name(&l.short_name) {
            return fail(&l, why);
        }
        let Some(dir) = self.downloads_folder() else { return fail(&l, local::NO_DOWNLOADS) };
        let temp = dir.join(local::temp_name(&uuid::Uuid::new_v4().to_string().to_uppercase()));
        if let Err(e) = crate::localfs::judge_io(&temp).and_then(|t| crate::localfs::open_custom(&t, |o| { o.write(true).create_new(true); })) {
            return fail(&l, &e.to_string());
        }
        l.place = Place::Downloads { temp, dir };
        let (job, actions) = self.ssh_transfers.download(pane, remote);
        self.ssh_locals.insert(job, l);
        self.ssh_transfer_apply(actions);
    }

    /// Download or view (`downloadOrView`, `CD:1630-1675`). Tandem has no in-app browser, so
    /// iTerm2's no-browser path applies: the file or folder downloads. The extension table is
    /// ported with a browser (Decision: with no browser the mime type is never consulted).
    pub(super) fn ssh_download_or_view(&mut self, pane: PaneId, to: &ScpPath) {
        // `Ask` is unreachable while `BROWSER_ALLOWED` is false: its View would open a browser
        // Tandem lacks, so both arms download.
        match ssh_files::download_or_view(None, ssh_files::BROWSER_ALLOWED) {
            ssh_files::ViewChoice::Download | ssh_files::ViewChoice::Ask { .. } => self.ssh_download(pane, &to.path),
        }
    }

    /// The Win-click's last phase (`UF:879-899`): the Smart Selection match at the click through
    /// `scpPathForFile`, downloaded when the pane's conductor can transfer. False when nothing
    /// was started (iTerm2 would use scp, which Tandem does not have).
    pub(super) fn secure_copy_at(&mut self, pane: PaneId, point: alacritty_terminal::index::Point) -> bool {
        let Some(c) = self.clicked(pane, point) else { return false };
        // The first component of the match (`components.firstObject`).
        let text = c.found.as_ref().and_then(|f| c.joined.text.get(f.range.clone()));
        self.secure_copy_text(pane, text)
    }

    /// Downloads the remote file `text` names, as the Win-click's last phase does. False when it
    /// names none this pane's conductor can fetch.
    pub(super) fn secure_copy_text(&mut self, pane: PaneId, text: Option<&str>) -> bool {
        let Some(to) = ssh_files::secure_copy_target(text, |t| self.scp_path_for(pane, t)) else { return false };
        if !self.transfer_pane(pane, &to) {
            return false;
        }
        self.ssh_download_or_view(pane, &to);
        true
    }

    /// The scp path of `file` for the pane's current host, user and directory (`scpPathForFile`;
    /// Tandem keeps no per-line host). `""` is the working directory itself. None when the pane
    /// reports no host, user or directory, or its host is this machine.
    pub(super) fn scp_path_for(&self, pane: PaneId, file: &str) -> Option<ScpPath> {
        let shell = &self.panes.get(&pane)?.own().shell;
        let local = shell.host.is_some() && self.shell_is_local(shell);
        let cwd = shell.cwd.as_deref().and_then(crate::ssh_jumps::clean_directory);
        ssh_files::scp_path(file, cwd.as_deref(), shell.host.as_deref(), shell.user.as_deref(), local)
    }

    /// Whether the pane's framing conductor can take a transfer to `to` (`canTransferFilesTo`).
    pub(super) fn transfer_pane(&self, pane: PaneId, to: &ScpPath) -> bool {
        self.framing(pane).is_some_and(|c| ssh_files::can_transfer(true, &c.identity(), c.discovered_hostname.as_deref(), to))
    }

    /// Alt-drop (`TV:5210-5271`): with Alt held over a session whose conductor can take the
    /// files, asks "OK to copy ... to ...?" and uploads on OK. False when it is not an upload
    /// (Alt up, no framing conductor, or a host the conductor is not): the paths paste as
    /// before (iTerm2 would use scp, which Tandem does not have).
    pub(super) fn ssh_drop(&mut self, pane: PaneId, files: Vec<String>) -> bool {
        // The Alt read, as everywhere (`GetKeyState(0x12)`).
        if unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x12) } >= 0 {
            return false;
        }
        self.ssh_drop_asked(pane, files)
    }

    /// The drop with Alt already known to be held.
    fn ssh_drop_asked(&mut self, pane: PaneId, files: Vec<String>) -> bool {
        let Some(to) = self.scp_path_for(pane, "") else { return false };
        if files.is_empty() || !self.transfer_pane(pane, &to) {
            return false;
        }
        // A network path is never read (P19): it is refused now, with the others left to ask.
        let (ok, refused): (Vec<String>, Vec<String>) = files.into_iter().partition(|f| ssh_files::upload_refused(f, None).is_none());
        for f in &refused {
            let name = std::path::Path::new(f).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            Self::ssh_notify(&local::upload_failed(&name), &ssh_files::upload_refused(f, None).unwrap_or_default());
        }
        if ok.is_empty() {
            return true;
        }
        // The path, host and user are what the remote reported: cleaned before the person reads them (P14).
        let shown = ScpPath { path: shown_name(&to.path), hostname: shown_name(&to.hostname), username: shown_name(&to.username) };
        let names: Vec<String> = ok.iter().map(|f| shown_name(f)).collect();
        let text = ssh_files::confirm_upload(&names, &shown);
        let then = super::scripts_alerts::Then::SshUpload { pane, files: ok.into_iter().map(std::path::PathBuf::from).collect(), to };
        let a = super::scripts_alerts::ask("", &text, &[crate::api_auth::OK, crate::api_auth::CANCEL], None, crate::warnings::Kind::Persistent, then);
        self.ask_scripts(a);
        true
    }

    /// OK on the drop question: each file is its own job, uploaded to `<to.path>/<file name>`
    /// (iTerm2 does not chain them).
    pub(super) fn ssh_upload_confirmed(&mut self, pane: PaneId, files: Vec<std::path::PathBuf>, to: ScpPath) {
        for f in files {
            let Some(name) = f.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
            let dest = ScpPath { path: crate::remote_file::posix_join(&to.path, &name), ..to.clone() };
            self.ssh_upload(pane, &f, &dest);
        }
    }

    /// Uploads the local file or folder `file` to the remote path `to.path`. A folder is
    /// packed into a gzipped tar sent to `<path>.tgz` (iTerm2's; not unpacked there). Refused,
    /// with a notification, when it is a network path, not a regular file or folder, or over
    /// 64 MiB (P19); the file is read only below that cap. A read error notifies at once.
    pub(super) fn ssh_upload(&mut self, pane: PaneId, file: &std::path::Path, to: &ScpPath) {
        use std::io::Read;
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let fail = |why: &str| Self::ssh_notify(&local::upload_failed(&name), why);
        if let Some(why) = ssh_files::upload_refused(&file.to_string_lossy(), None) {
            return fail(&why);
        }
        // The pane may have changed host or lost its conductor while the question showed.
        if !self.transfer_pane(pane, to) {
            return fail(crate::ssh_files::FileError::ConnectionClosed.text());
        }
        let Some(id) = self.framing(pane).map(|c| c.identity()) else { return };
        let host = if id.hostname.is_empty() { id.host.clone() } else { id.hostname.clone() };
        let (local_file, meta) = match crate::localfs::judge_io(file).and_then(|l| crate::localfs::lstat(&l).map(|m| (l, m))) {
            Ok(found) => found,
            Err(e) => return fail(&e.to_string()),
        };
        let too_big = || ssh_files::upload_refused("", Some(ssh_files::UPLOAD_MAX + 1)).unwrap_or_default();
        let (data, dest) = if meta.is_dir {
            match local::tree_over(file, ssh_files::UPLOAD_MAX) {
                Ok(false) => {}
                Ok(true) => return fail(&too_big()),
                Err(e) => return fail(&e.to_string()),
            }
            match crate::prompted::upload_archive(&[file.to_path_buf()]) {
                Ok(a) if a.len() as u64 <= ssh_files::UPLOAD_MAX => (a, format!("{}.tgz", to.path)),
                Ok(_) => return fail(&too_big()),
                Err(e) => return fail(&e),
            }
        } else if meta.is_file {
            if let Some(why) = ssh_files::upload_refused("", Some(meta.len)) {
                return fail(&why);
            }
            // Read whole, but never past the cap even if the file grew since the check.
            let mut data = Vec::new();
            let read = crate::localfs::open(&local_file).and_then(|f| f.take(ssh_files::UPLOAD_MAX + 1).read_to_end(&mut data));
            if let Err(e) = read {
                return fail(&e.to_string());
            }
            if data.len() as u64 > ssh_files::UPLOAD_MAX {
                return fail(&too_big());
            }
            (data, to.path.clone())
        } else {
            return fail("Not a regular file");
        };
        let mut l = Local::new(Place::Downloads { temp: std::path::PathBuf::new(), dir: std::path::PathBuf::new() }, pane, &dest, &host, id.username.as_deref());
        l.short_name = name.clone();
        l.upload = true;
        l.total = Some(data.len() as u64);
        let (job, actions) = self.ssh_transfers.upload(pane, &dest, data, &uuid::Uuid::new_v4().to_string().to_uppercase());
        self.ssh_locals.insert(job, l);
        self.ssh_transfer_apply(actions);
    }

    /// A job ended. A failure removes its temp and notifies with the reason; a download into
    /// Downloads asks Keep Both or Replace when the plain name is taken, then finishes.
    pub(super) fn ssh_transfer_done(&mut self, job: u64, result: Result<Outcome, String>) {
        self.banners.clear("transfer");
        if self.ssh_locals.get(&job).is_some_and(|l| l.upload) {
            let l = self.ssh_locals.remove(&job).unwrap();
            match result {
                Ok(_) => Self::ssh_notify(&local::upload_finished(&l.short_name), ""),
                Err(why) => Self::ssh_notify(&local::upload_failed(&l.short_name), &why),
            }
            return;
        }
        let Some(l) = self.ssh_locals.get_mut(&job) else { return };
        let outcome = match result {
            Ok(o) => o,
            Err(why) => {
                let l = self.ssh_locals.remove(&job).unwrap();
                let _ = crate::localfs::judge_io(l.place.temp()).and_then(|t| crate::localfs::remove_file(&t));
                // A Download Files item reports to its batch, which notifies once it is over.
                match &l.place {
                    Place::Temp { batch, .. } => self.ssh_batch_item_ended(*batch, &l.short_name, Err(why)),
                    Place::Downloads { .. } => Self::ssh_notify(&local::download_failed(&l.short_name), &why),
                }
                return;
            }
        };
        match &l.place {
            Place::Downloads { dir, .. } => {
                l.zip = outcome == Outcome::ZipOfFolder;
                let (_, plain) = local::final_destination(dir, &l.short_name, l.zip, &|p| crate::localfs::local_exists(p));
                match plain {
                    None => self.ssh_finish_download(job, None),
                    Some(plain) => {
                        let name = shown_name(&plain.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
                        let a = super::scripts_alerts::ask(local::OVERWRITE_HEADING, &local::keep_or_replace(&name), &[local::KEEP_BOTH, local::REPLACE], Some(local::KEEP_OR_REPLACE_ID), crate::warnings::Kind::Permanent, super::scripts_alerts::Then::SshKeepOrReplace { job });
                        // A silenced answer finishes at once, inside this call.
                        self.ask_scripts(a);
                    }
                }
            }
            // Download Files: a file is marked and moved into its place in the batch folder, an
            // earlier item of that name removed first (last one wins, R14de-4); a folder was
            // unzipped there already. The batch reports.
            // A file waits while an earlier item of its name is still unzipping there.
            Place::Temp { dest, .. } if outcome == Outcome::File => {
                let dest = dest.clone();
                if self.ssh_landings.claim(&dest, Landing::File(job)).is_some() {
                    self.ssh_land_file(job);
                    self.ssh_land_next(&dest);
                }
            }
            Place::Temp { .. } => {
                let l = self.ssh_locals.remove(&job).unwrap();
                let Place::Temp { temp, dest, batch } = &l.place else { return };
                let result = match outcome {
                    Outcome::Directory => Ok(dest.clone()),
                    _ => Err(crate::ssh_files::FileError::Internal.text().to_string()),
                };
                if let Err(why) = &result {
                    Self::ssh_note_unremoved(why, temp);
                }
                self.ssh_batch_item_ended(*batch, &l.short_name, result);
            }
        }
    }

    /// Moves a finished Download Files file item, which holds its place, into the batch folder.
    fn ssh_land_file(&mut self, job: u64) {
        let Some(l) = self.ssh_locals.remove(&job) else { return };
        let Place::Temp { temp, dest, batch } = &l.place else { return };
        let result = super::ssh_panel_app::clear_earlier(dest).map_err(|e| e.to_string()).and_then(|()| local::finish(temp, dest, &l.url()));
        if let Err(why) = &result {
            Self::ssh_note_unremoved(why, temp);
        }
        self.ssh_batch_item_ended(*batch, &l.short_name, result);
    }

    /// Keep Both (0) or Replace (1) answered for the finished download `job`. Keep Both
    /// recomputes the free name now (the numbered one may have been taken while the question waited).
    pub(super) fn ssh_keep_or_replace(&mut self, job: u64, index: usize) {
        self.ssh_finish_download(job, Some(index == 1));
    }

    /// Moves the finished download's temp into Downloads: `replace` None when the plain name is
    /// free, Some(true) onto the plain name, Some(false) onto a free numbered one.
    fn ssh_finish_download(&mut self, job: u64, replace: Option<bool>) {
        let Some(l) = self.ssh_locals.remove(&job) else { return };
        let Place::Downloads { temp, dir } = &l.place else { return };
        let (free, plain) = local::final_destination(dir, &l.short_name, l.zip, &|p| crate::localfs::local_exists(p));
        let dest = match (replace, plain) {
            (Some(true), Some(plain)) => plain,
            _ => free,
        };
        match local::finish(temp, &dest, &l.url()) {
            Ok(path) => Self::ssh_notify(&local::download_finished(&path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()), ""),
            Err(why) => {
                Self::ssh_note_unremoved(&why, temp);
                Self::ssh_notify(&local::download_failed(&l.short_name), &why);
            }
        }
    }

    /// A dangerous file that could not be removed: the person is told where it is.
    pub(super) fn ssh_note_unremoved(why: &str, temp: &std::path::Path) {
        if why == local::NOT_REMOVED {
            super::files_ui::message(&format!("{}\n\n{}", local::DANGER_HEADING, local::danger(&temp.to_string_lossy())));
        }
    }

    /// The Composer's remote suggestions (`CD:1864-1899`, `EP:184-209`): while the pane's
    /// conductor frames and its working directory is a POSIX path (or unknown), the request is
    /// built from the remote `PATH` and home, answered from the 5-second cache when it can, else
    /// sent high priority after canceling the queued older ones. False when the pane is not
    /// framing: the local listing runs (a framing pane with a drive-path folder gets none). The caller has set `waiting`.
    pub(super) fn ssh_composer_fetch(&mut self, pane: PaneId, generation: u64, line: &str, suffix: &str, name: &str, explicit: bool) -> bool {
        let Some(c) = self.framing(pane) else { return false };
        let reported = self.panes.get(&pane).and_then(|p| p.own().shell.cwd.clone());
        let cwd = match crate::ssh_suggest::framed_cwd(reported.as_deref()) {
            crate::ssh_suggest::FramedCwd::Unknown => None,
            crate::ssh_suggest::FramedCwd::Posix(d) => Some(d),
            // A drive path is no folder of the remote: answer empty, never list THIS PC's.
            crate::ssh_suggest::FramedCwd::Refused => {
                self.composer_found(pane, generation, Vec::new());
                return true;
            }
        };
        let inputs = crate::ssh_suggest::inputs(line, suffix, c.environment_variable("PATH"), cwd.as_deref(), c.home_directory.as_deref(), explicit);
        if let Some(values) = self.ssh_suggest_cache.entry(pane).or_default().get(&inputs, Instant::now()) {
            self.composer_found(pane, generation, crate::ssh_suggest::entries(name, &values));
            return true;
        }
        self.ssh_cancel_queued(pane, &FileSub::is_suggest);
        let waiter = Waiter::Suggest { generation, inputs: inputs.clone(), name: name.to_string() };
        if !self.ssh_send(pane, FileSub::Suggest(inputs), true, waiter) {
            self.composer_found(pane, generation, Vec::new());
        }
        true
    }

    /// A suggestion answer: status 0 is the JSON array (capped and cleaned, P18), anything else
    /// is empty. Only an answer the remote gave (status 0 or above) is cached: a canceled or
    /// failed request must not teach the cache that nothing matches. A stale generation is
    /// dropped by `composer_found`.
    fn ssh_suggested(&mut self, pane: PaneId, generation: u64, inputs: crate::ssh_files::SuggestInputs, name: &str, content: &str, status: i32) {
        let values = if status == 0 { crate::ssh_suggest::parse_limited(content, inputs.limit) } else { Vec::new() };
        if status >= 0 {
            self.ssh_suggest_cache.entry(pane).or_default().insert(inputs, values.clone(), Instant::now());
        }
        self.composer_found(pane, generation, crate::ssh_suggest::entries(name, &values));
    }

    /// Drops the suggestion answers that expired (`about_to_wait`, at most every 10 s).
    pub(super) fn ssh_suggest_purge(&mut self) {
        let now = Instant::now();
        if self.ssh_purge_at.is_some_and(|t| now < t) {
            return;
        }
        self.ssh_purge_at = Some(now + std::time::Duration::from_secs(10));
        for c in self.ssh_suggest_cache.values_mut() {
            c.purge(now);
        }
        self.ssh_suggest_cache.retain(|_, c| !c.is_empty());
    }

    /// Takes the `%notif search` texts of the pane and handles them.
    pub(super) fn ssh_search_drain(&mut self, pane: PaneId) {
        let texts = self.panes.get_mut(&pane).map(|p| p.take_ssh_search()).unwrap_or_default();
        for text in texts {
            self.ssh_search_notif(pane, &text);
        }
    }

    /// A search notification (`CD:2681-2698`): its result goes to the panel when its id is the
    /// current search's, and every one is acknowledged, or the remote stops after 16 (`F:1088`).
    /// The id the remote sent is acknowledged only when it is the current search's or one Tandem
    /// started (P18); a result past the panel's cap is acknowledged and dropped. A result that
    /// arrives with 16 acks of its search still out is dropped unacknowledged (and counted): a
    /// conforming framer never sends it, and a flood never grows the conductor's queue.
    pub(super) fn ssh_search_notif(&mut self, pane: PaneId, text: &str) {
        let Some((id, json)) = crate::ssh_suggest::search_notif(text) else { return };
        let current = self.ssh_search.as_ref().filter(|(p, i, _)| *p == pane && !i.is_empty());
        let generation = current.map(|(_, _, g)| *g);
        let current_id = current.map(|(_, i, _)| i.clone());
        let issued: Vec<String> = self.ssh_search_issued.iter().filter(|(p, _)| *p == pane).map(|(_, i)| i.clone()).collect();
        if !crate::ssh_suggest::ack_allowed(&id, current_id.as_deref(), &issued) {
            return;
        }
        if !self.ssh_search_acks.entry(pane).or_default().admit(&id) {
            return;
        }
        if let (Some(g), true) = (generation, current_id.as_deref() == Some(id.as_str())) {
            if let Ok(file) = crate::remote_file::RemoteFile::parse(&json) {
                if let Some((w, p)) = self.ssh_panel.as_mut() {
                    p.search_result(g, file);
                    w.request_redraw();
                }
            }
        }
        let waiter = Waiter::SearchAck { id: id.clone() };
        if !self.ssh_send(pane, FileSub::Search(crate::ssh_files::SearchSub::Ack { id: id.clone(), count: 1 }), false, waiter) {
            self.ssh_search_acked(pane, &id);
        }
    }

    /// An acknowledgement of search `id` was answered (or never sent): one place frees.
    fn ssh_search_acked(&mut self, pane: PaneId, id: &str) {
        if let Some(w) = self.ssh_search_acks.get_mut(&pane) {
            w.answered(id);
        }
    }

    /// The answer to a search start: the id. One for a search that was replaced or stopped
    /// meanwhile is stopped at once.
    fn ssh_search_started(&mut self, pane: PaneId, generation: u64, content: &str, status: i32) {
        let id = content.trim().to_string();
        let pending = matches!(&self.ssh_search, Some((p, i, g)) if *p == pane && i.is_empty() && *g == generation);
        if status != 0 || !crate::ssh_suggest::valid_search_id(&id) {
            if pending {
                self.ssh_search = None;
                let why = if status != 0 { ssh_files::check(status).err().map(|e| e.text().to_string()) } else { None }.unwrap_or_else(|| ssh_files::GARBAGE.to_string());
                if let Some((w, p)) = self.ssh_panel.as_mut() {
                    p.search_failed(generation, &why);
                    w.request_redraw();
                }
            }
            return;
        }
        self.ssh_search_issued.push((pane, id.clone()));
        if self.ssh_search_issued.len() > 16 {
            self.ssh_search_issued.remove(0);
        }
        if !pending {
            self.ssh_send(pane, FileSub::Search(crate::ssh_files::SearchSub::Stop { id }), false, Waiter::Ignore);
            return;
        }
        self.ssh_search = Some((pane, id.clone(), generation));
        if let Some((w, p)) = self.ssh_panel.as_mut() {
            p.search_started(generation, &id);
            w.request_redraw();
        }
    }

    /// Starts the panel's search on `pane` (one at a time: the running one stops first).
    pub(super) fn ssh_search_start(&mut self, pane: PaneId, generation: u64, query: String, base: String) {
        self.ssh_search_stop();
        self.ssh_search = Some((pane, String::new(), generation));
        let sub = FileSub::Search(crate::ssh_files::SearchSub::Start { query, base });
        if !self.ssh_send(pane, sub, true, Waiter::SearchStart { generation }) {
            self.ssh_search = None;
            if let Some((w, p)) = self.ssh_panel.as_mut() {
                p.search_failed(generation, ssh_files::FileError::ConnectionClosed.text());
                w.request_redraw();
            }
        }
    }

    /// Stops the running search: with its id known, a stop goes to the remote; one whose id has
    /// not come yet is stopped when it does (`ssh_search_started`).
    pub(super) fn ssh_search_stop(&mut self) {
        let Some((pane, id, _)) = self.ssh_search.take() else { return };
        if crate::ssh_suggest::valid_search_id(&id) {
            self.ssh_send(pane, FileSub::Search(crate::ssh_files::SearchSub::Stop { id }), false, Waiter::Ignore);
        }
    }

    /// Answers -1 to every waiter of `pane` (its conductor went or the pane did), and
    /// forgets them: a request is never left unanswered (P5).
    pub(super) fn ssh_files_pane_gone(&mut self, pane: PaneId) {
        self.ssh_suggest_cache.remove(&pane);
        self.ssh_search_issued.retain(|(p, _)| *p != pane);
        if self.ssh_search.as_ref().is_some_and(|(p, _, _)| *p == pane) {
            self.ssh_search = None;
        }
        let ids: Vec<u64> = self.ssh_waiters.keys().filter(|(p, _)| *p == pane).map(|(_, id)| *id).collect();
        for id in ids {
            self.ssh_file_done(pane, id, String::new(), -1);
        }
        self.ssh_search_acks.remove(&pane);
    }

    /// Fails the waiters of every pane that no longer has a framing conductor. The conductors
    /// answer what they hold themselves (a fail, an unhook and a pop tell their file
    /// requests); this catches what they could not.
    pub(super) fn ssh_files_settle(&mut self) {
        if self.ssh_waiters.is_empty() {
            return;
        }
        let mut gone: Vec<PaneId> = self.ssh_waiters.keys().map(|(p, _)| *p).filter(|p| self.framing(*p).is_none()).collect();
        gone.sort_unstable();
        gone.dedup();
        for pane in gone {
            self.ssh_files_pane_gone(pane);
        }
    }
}

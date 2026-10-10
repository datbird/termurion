//! Save Page As in a browser pane (milestone 13x4, Task 10): the dialog, the files and the alert.
//!
//! `Saving` is the whole flow with no `App` in it: it takes the answers of the page, of DevTools and
//! of the file worker, and answers `Act`s for the app to carry out. That keeps the flow testable
//! against the script seam and the real file system (an `App` cannot be built in a unit test). The
//! app side (`impl App` below) is thin: it runs the page world through `browser_page_call`, the
//! resource reads through `devtools_read` (the one door), and the files on a worker thread, so the
//! UI thread never writes.
//!
//! Rules this file keeps (Task 8 and 9 fixes):
//! - every file is written through `inert_files::write_new_checked` (create-new, the one dangerous
//!   type list, a strict Mark of the Web with the page's URL: a drive that cannot hold it fails the
//!   save with `inert_files::UNMARKABLE` and cleans up), every folder through `localfs::create_dir`;
//!   nothing overwrites;
//! - `Job::permits` is asked before EVERY step, and the pane's navigation epoch is bound at the start
//!   and checked before every step and every answer; an abort drops the rest;
//! - a resource is read only from the page's own loaded copy (`devtools_read`), the mapping and the
//!   saved-attribute script come from the job (`attributes_body`), names only from the sanitizer;
//! - the alert shows `inert_files::display_text` of the failure; nothing saved is opened or run
//!   (Explorer shows `index.html` selected, nothing more).

use super::scripts_alerts::{Then, ask};
use super::*;
use crate::page_saver::{self, Job, Step};
use crate::warnings::Kind;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

/// What a file worker does with one path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    MakeDir,
    Write,
    Remove,
    RemoveDir,
    /// The save is over unfinished (cancelled, or failed): the worker removes the folders IT made
    /// for this save, deepest first, each only if empty (`localfs::remove_dir`). A MakeDir whose
    /// answer had not reached the job when it ended is covered too (final review minor 3). Nothing
    /// is answered.
    Abandon,
}

/// One file operation for the worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FileOp {
    pub(super) kind: FileKind,
    pub(super) path: PathBuf,
    pub(super) bytes: Vec<u8>,
    /// The page the file came from: the Mark of the Web's `HostUrl`.
    pub(super) host_url: String,
}

/// What the worker answers: the operation's kind and path, and the error text if it failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDone {
    pub(super) kind: FileKind,
    pub(super) path: PathBuf,
    pub(super) err: Option<String>,
    /// The file was written but could not be marked (`inert_files::unmarkable`): the save fails.
    pub(super) unmarked: bool,
}

/// Carries out one operation. Runs on the worker thread (and in tests, directly). Every path is
/// judged (`judge_io`); a write is create-new through the door and strictly marked.
pub(super) fn run_op(op: &FileOp) -> FileDone {
    run_op_with(op, &crate::inert_files::mark_strict)
}

/// `run_op` with the marker given (a test forces a failure).
fn run_op_with(op: &FileOp, mark: &dyn Fn(&Path, Option<&str>) -> std::io::Result<()>) -> FileDone {
    let r: std::io::Result<()> = match op.kind {
        FileKind::MakeDir => crate::localfs::judge_io(&op.path).and_then(|p| crate::localfs::create_dir(&p)),
        FileKind::Write => crate::inert_files::write_new_checked_with(&op.path, &op.bytes, Some(&op.host_url), mark),
        FileKind::Remove => crate::localfs::judge_io(&op.path).and_then(|p| crate::localfs::remove_file(&p)),
        FileKind::RemoveDir | FileKind::Abandon => crate::localfs::judge_io(&op.path).and_then(|p| crate::localfs::remove_dir(&p)),
    };
    let unmarked = r.as_ref().err().is_some_and(crate::inert_files::unmarkable);
    FileDone { kind: op.kind, path: op.path.clone(), err: r.err().map(|e| e.to_string()), unmarked }
}

/// The file worker of one save: carries out each operation in order and hands each result to
/// `sink`, until the save's sender is dropped.
/// The folders the worker made (a create_dir that succeeded: never one that was there before) are
/// remembered for an `Abandon`.
pub(super) fn page_save_worker(rx: std::sync::mpsc::Receiver<FileOp>, sink: &dyn Fn(FileDone)) {
    let mut made: Vec<PathBuf> = Vec::new();
    for op in rx {
        if op.kind == FileKind::Abandon {
            for dir in made.drain(..).rev() {
                let _ = run_op(&FileOp { kind: FileKind::Abandon, path: dir, bytes: Vec::new(), host_url: String::new() });
            }
            continue;
        }
        let done = run_op(&op);
        if op.kind == FileKind::MakeDir && done.err.is_none() {
            made.push(op.path.clone());
        }
        sink(done);
    }
}

/// One thing for the app to do.
#[derive(Debug, PartialEq)]
pub(super) enum Act {
    /// Run this body in the page world (no bridge, no secret); answer `Saving::page_answer`.
    Page(String),
    /// `Page.getResourceTree`; answer `Saving::tree_answer`.
    Tree,
    /// `Page.getResourceContent`; answer `Saving::content_answer`.
    Content { frame: String, url: String },
    /// Hand to the file worker; its result is `Saving::file_done`.
    File(FileOp),
    /// Show this file selected in Explorer. The save is over.
    Reveal(PathBuf),
    /// The Save Failed alert's text. The save is over.
    Fail(String),
}

/// What the job waits for from the page.
#[derive(Debug, PartialEq)]
enum Waiting {
    Extract,
    Tree(String),
    Content(String),
    Attributes,
    Html,
}

pub(super) struct Saving {
    pub(super) job: Job,
    pub(super) folder: PathBuf,
    /// The page's URL when the save was chosen: every file's `HostUrl`.
    url: String,
    /// `Page.getResourceTree`, read once per save.
    pub(super) tree: Option<Value>,
    /// Which save this is, so a worker event of a cancelled save is ignored.
    pub(super) id: u64,
    /// The file worker's queue (None in tests, where the harness runs each operation itself).
    pub(super) worker: Option<std::sync::mpsc::Sender<FileOp>>,
    queue: VecDeque<Step>,
    waiting: Option<Waiting>,
    /// File operations handed out and not yet answered.
    outstanding: usize,
    budget: crate::devtools::Budget,
    /// The error of the file operation that failed the save (a MakeDir, `index.html`), for the
    /// alert; a skipped resource's error is never kept (Task 10 review minor 3).
    io_error: Option<String>,
    /// `Reveal` or `Fail` was answered.
    over: bool,
}

impl Saving {
    pub(super) fn new(folder: PathBuf, url: String, epoch: u64, id: u64) -> Saving {
        let mut job = Job::new(folder.clone());
        job.bind_epoch(epoch);
        Saving {
            job,
            folder,
            url,
            tree: None,
            id,
            worker: None,
            queue: VecDeque::new(),
            waiting: None,
            outstanding: 0,
            budget: crate::devtools::Budget::default(),
            io_error: None,
            over: false,
        }
    }

    pub(super) fn begin(&mut self, now: u64) -> Vec<Act> {
        let steps = self.job.start();
        self.run(steps, now)
    }

    /// What a cancel hands the file worker, in order (`App::page_save_cancel` sends exactly these).
    pub(super) fn cancel_ops(&mut self, now: u64) -> Vec<FileOp> {
        let mut ops: Vec<FileOp> = self.cancel(now).into_iter().filter_map(|a| if let Act::File(op) = a { Some(op) } else { None }).collect();
        ops.push(self.abandon_op());
        ops
    }

    /// The last operation of a save that ends unfinished: after the queued ones, the worker removes
    /// the empty folders it made.
    pub(super) fn abandon_op(&self) -> FileOp {
        FileOp { kind: FileKind::Abandon, path: self.folder.clone(), bytes: Vec::new(), host_url: String::new() }
    }

    /// The person cancelled, or the pane closed: stop, remove the partial output. No alert.
    pub(super) fn cancel(&mut self, now: u64) -> Vec<Act> {
        self.queue.clear();
        self.waiting = None;
        let steps = self.job.cancel();
        let acts = self.run(steps, now);
        self.over = true;
        acts.into_iter().filter(|a| !matches!(a, Act::Fail(_) | Act::Reveal(_))).collect()
    }

    fn run(&mut self, steps: Vec<Step>, now: u64) -> Vec<Act> {
        self.queue.extend(steps);
        self.pump(now)
    }

    /// A navigation since the start aborts the save; what the abort answers replaces the rest.
    /// True when it did (the caller pumps the abort and ignores the answer it was handed).
    fn guard(&mut self, now: u64) -> bool {
        let more = self.job.check_epoch(now);
        let aborted = !more.is_empty();
        if aborted {
            self.queue.clear();
            self.waiting = None;
            self.queue.extend(more);
        }
        aborted
    }

    /// Carries out queued steps in order until one waits for the page, a file barrier or the end.
    fn pump(&mut self, now: u64) -> Vec<Act> {
        let mut acts = Vec::new();
        while !self.over && self.waiting.is_none() {
            let _ = self.guard(now);
            let Some(step) = self.queue.front().cloned() else { break };
            if !self.job.permits(&step) {
                self.queue.pop_front();
                continue;
            }
            // The attributes and the end read what the files did: every file answer comes first.
            // A folder is made only after the one above it was: a failed save folder (it exists) must
            // not get a `resources` folder made inside it by a queued second operation.
            if matches!(step, Step::AddAttributes(_) | Step::Done(_) | Step::MakeDir(_)) && self.outstanding > 0 {
                break;
            }
            self.queue.pop_front();
            match step {
                Step::MakeDir(p) => acts.push(self.file(FileKind::MakeDir, p, Vec::new())),
                Step::Remove(p) => acts.push(self.file(FileKind::Remove, p, Vec::new())),
                Step::RemoveDir(p) => acts.push(self.file(FileKind::RemoveDir, p, Vec::new())),
                Step::Write { path, bytes } => {
                    // Only the folder itself and its resources folder are ever written into.
                    let parent = path.parent();
                    if parent == Some(self.folder.as_path()) || parent == Some(self.folder.join("resources").as_path()) {
                        acts.push(self.file(FileKind::Write, path, bytes));
                    } else {
                        let more = self.job.on_write_failed(&path);
                        self.queue.extend(more);
                    }
                }
                Step::Extract => {
                    self.waiting = Some(Waiting::Extract);
                    acts.push(Act::Page(page_saver::extract_body()));
                }
                Step::Fetch(url) => {
                    if self.tree.is_none() {
                        self.waiting = Some(Waiting::Tree(url));
                        acts.push(Act::Tree);
                    } else if let Some(a) = self.fetch(&url) {
                        acts.push(a);
                    }
                }
                // The job's current script, never the one carried by the step: a write that failed
                // since has left the mapping.
                Step::AddAttributes(_) => {
                    self.waiting = Some(Waiting::Attributes);
                    acts.push(Act::Page(self.job.attributes_body()));
                }
                Step::GetHtml => {
                    self.waiting = Some(Waiting::Html);
                    acts.push(Act::Page(page_saver::clone_body()));
                }
                Step::Done(p) => {
                    self.over = true;
                    acts.push(Act::Reveal(p));
                }
                Step::Failed(e) => {
                    self.over = true;
                    let text = match &self.io_error {
                        Some(io) => format!("{e}: {io}"),
                        None => e,
                    };
                    acts.push(Act::Fail(page_saver::save_failed_text(&crate::inert_files::display_text(&text, 400, false))));
                }
            }
        }
        acts
    }

    fn file(&mut self, kind: FileKind, path: PathBuf, bytes: Vec<u8>) -> Act {
        self.outstanding += 1;
        Act::File(FileOp { kind, path, bytes, host_url: self.url.clone() })
    }

    /// The content read for `url`: only a URL the tree lists is asked for; the job hears None for
    /// any other.
    fn fetch(&mut self, url: &str) -> Option<Act> {
        let frame = self.tree.as_ref().and_then(|t| crate::devtools::frame_for(t, url));
        match frame {
            Some(frame) => {
                self.waiting = Some(Waiting::Content(url.to_string()));
                Some(Act::Content { frame, url: url.to_string() })
            }
            None => {
                let more = self.job.on_resource(url, None, &|_| false);
                self.queue.extend(more);
                None
            }
        }
    }

    /// The page world answered (Extract, AddAttributes or GetHtml).
    pub(super) fn page_answer(&mut self, r: Result<Value, String>, now: u64) -> Vec<Act> {
        if self.guard(now) {
            return self.pump(now);
        }
        let more = match (self.waiting.take(), r) {
            (Some(Waiting::Extract), Ok(v)) => {
                let urls = v.as_array().map(|a| a.iter().filter_map(|u| u.as_str().map(str::to_string)).collect()).unwrap_or_default();
                self.job.on_urls(urls)
            }
            (Some(Waiting::Attributes), Ok(_)) => self.job.on_attributes_added(),
            (Some(Waiting::Html), Ok(v)) => self.job.on_html(v.as_str().map(str::to_string)),
            // A page-world exception's text is the page's to choose: never shown, and only its size
            // is logged (page text stays out of the log). The alert says Tandem's fixed text (Task 10
            // review minor 2).
            (Some(Waiting::Extract | Waiting::Attributes | Waiting::Html), Err(e)) => {
                crate::rlog!("save page as: a saver script failed (an error of {} bytes)", e.len());
                self.job.on_error(page_saver::FAILED_TO_GET_HTML.to_string())
            }
            (other, _) => {
                self.waiting = other;
                return Vec::new();
            }
        };
        self.run(more, now)
    }

    /// `Page.getResourceTree` answered; an error leaves every resource unlisted (skipped).
    pub(super) fn tree_answer(&mut self, r: Result<Value, String>, now: u64) -> Vec<Act> {
        if self.guard(now) {
            return self.pump(now);
        }
        let Some(Waiting::Tree(url)) = self.waiting.take() else { return Vec::new() };
        self.tree = Some(r.unwrap_or_else(|_| json!({})));
        let mut acts = Vec::new();
        if !self.over && self.job.permits(&Step::Fetch(url.clone())) {
            acts.extend(self.fetch(&url));
        }
        acts.extend(self.pump(now));
        acts
    }

    /// `Page.getResourceContent` answered: the bytes and the MIME of the page's own copy, or None.
    pub(super) fn content_answer(&mut self, r: Result<Value, String>, now: u64) -> Vec<Act> {
        if self.guard(now) {
            return self.pump(now);
        }
        let Some(Waiting::Content(url)) = self.waiting.take() else { return Vec::new() };
        let got = r.ok().and_then(|v| self.budget.take(&v).ok()).map(|bytes| {
            let mime = self.tree.as_ref().and_then(|t| crate::devtools::resource_mime(t, &url));
            (bytes, mime)
        });
        let more = self.job.on_resource(&url, got, &|_| false);
        self.run(more, now)
    }

    /// The worker finished one operation.
    pub(super) fn file_done(&mut self, d: FileDone, now: u64) -> Vec<Act> {
        self.outstanding = self.outstanding.saturating_sub(1);
        if d.unmarked {
            let more = self.job.on_unmarked(&d.path);
            self.queue.extend(more);
            return self.pump(now);
        }
        let (more, e) = match (d.err, d.kind) {
            (Some(e), FileKind::MakeDir) => (self.job.on_make_dir_failed(&d.path), Some(e)),
            (Some(e), FileKind::Write) => (self.job.on_write_failed(&d.path), Some(e)),
            (None, FileKind::MakeDir) => (self.job.on_made(&d.path), None),
            _ => (Vec::new(), None),
        };
        // Only the error of the operation that failed the save goes in the alert.
        if more.iter().any(|s| matches!(s, Step::Failed(_))) {
            self.io_error = e;
        }
        self.queue.extend(more);
        self.pump(now)
    }
}

impl App {
    /// Save Page As from the page context menu (`PSV:342-360`): a page with no URL does nothing; a
    /// save already running for the pane is cancelled instead (the way to cancel it). The person
    /// names the folder; it must not exist.
    pub(super) fn save_page_as(&mut self, pane: PaneId) {
        let url = self.panes.get(&pane).and_then(|p| p.browser_io()).map(|io| io.url.clone()).unwrap_or_default();
        if url.is_empty() {
            return;
        }
        if self.page_saves.contains_key(&pane) {
            self.page_save_cancel(pane);
            self.banners.set("action", "Save Page As cancelled.".to_string());
            return;
        }
        // The page is the one shown when the item was chosen: its epoch is bound before the modal
        // dialog and rechecked after it (Task 10 review minor 7).
        let epoch = self.page_save_epoch(pane);
        let dialog = rfd::FileDialog::new().set_file_name(page_saver::default_name(&url));
        let Some(picked) = crate::localfs::save_file(dialog) else { return };
        self.start_page_save(pane, picked.into_path_buf(), url, epoch);
    }

    /// The save of `pane`'s page `url` (shown at `epoch`, when the item was chosen) into the new
    /// folder `folder` (the dialog's pick): a page that moved since is not saved; the folder is
    /// judged; nothing is written before both.
    pub(super) fn start_page_save(&mut self, pane: PaneId, folder: PathBuf, url: String, epoch: u64) {
        if self.page_saves.contains_key(&pane) || epoch == u64::MAX {
            return;
        }
        if self.page_save_epoch(pane) != epoch {
            self.page_save_alert(&page_saver::save_failed_text(page_saver::NAVIGATED_AWAY));
            return;
        }
        if let Err(e) = page_saver::destination(&folder) {
            self.page_save_alert(&e);
            return;
        }
        self.page_save_seq += 1;
        let id = self.page_save_seq;
        let mut saving = Saving::new(folder, url, epoch, id);
        let (tx, rx) = std::sync::mpsc::channel::<FileOp>();
        let proxy = self.proxy.clone();
        let spawned = std::thread::Builder::new().name("page-save".into()).spawn(move || {
            page_save_worker(rx, &|done| {
                let _ = proxy.lock().map(|p| p.send_event(UserEvent::PageSaveFile { pane, id, done }));
            });
        });
        if let Err(e) = spawned {
            self.page_save_alert(&e.to_string());
            return;
        }
        saving.worker = Some(tx);
        self.page_saves.insert(pane, saving);
        self.page_save_do(pane, |s, now| s.begin(now));
    }

    fn page_save_epoch(&self, pane: PaneId) -> u64 {
        // A pane with no view cannot be on the page it started on: an epoch no save was bound to.
        self.browser_views.get(&pane).map_or(u64::MAX, |b| b.reader.epoch)
    }

    /// Runs `f` on the pane's save, then carries out what it answers.
    fn page_save_do(&mut self, pane: PaneId, f: impl FnOnce(&mut Saving, u64) -> Vec<Act>) {
        let now = self.page_save_epoch(pane);
        let Some(saving) = self.page_saves.get_mut(&pane) else { return };
        let id = saving.id;
        let acts = f(saving, now);
        self.page_save_acts(pane, id, acts);
    }

    fn page_save_acts(&mut self, pane: PaneId, id: u64, acts: Vec<Act>) {
        for act in acts {
            match act {
                Act::Page(body) => {
                    self.browser_page_call(pane, &body, json!({}), Box::new(move |app, pane, r| app.page_save_answer(pane, id, Answer::Page(r))));
                }
                Act::Tree => {
                    self.devtools_read(pane, "Page.getResourceTree", json!({}), Box::new(move |app, pane, r| app.page_save_answer(pane, id, Answer::Tree(r))));
                }
                Act::Content { frame, url } => {
                    let params = crate::devtools::resource_content_params(&frame, &url);
                    self.devtools_read(pane, "Page.getResourceContent", params, Box::new(move |app, pane, r| app.page_save_answer(pane, id, Answer::Content(r))));
                }
                Act::File(op) => {
                    let sent = self.page_saves.get(&pane).filter(|s| s.id == id).and_then(|s| s.worker.as_ref()).map(|w| w.send(op.clone()).is_ok());
                    if sent != Some(true) {
                        // The worker is gone: the operation failed.
                        let done = FileDone { kind: op.kind, path: op.path, err: Some("the file worker stopped".into()), unmarked: false };
                        self.page_save_file_done(pane, id, done);
                    }
                }
                Act::Reveal(path) => {
                    self.page_saves.remove(&pane);
                    self.reveal_in_explorer(&path.to_string_lossy());
                }
                Act::Fail(text) => {
                    // A folder made while the save failed, its answer still on the way, goes too.
                    if let Some(saving) = self.page_saves.remove(&pane) {
                        if let Some(w) = &saving.worker {
                            let _ = w.send(saving.abandon_op());
                        }
                    }
                    self.page_save_alert(&text);
                }
            }
        }
    }

    /// An answer of the page or DevTools, for save `id` (a cancelled save's answer is dropped).
    fn page_save_answer(&mut self, pane: PaneId, id: u64, a: Answer) {
        if !self.page_saves.get(&pane).is_some_and(|s| s.id == id) {
            return;
        }
        self.page_save_do(pane, move |s, now| match a {
            Answer::Page(r) => s.page_answer(r, now),
            Answer::Tree(r) => s.tree_answer(r, now),
            Answer::Content(r) => s.content_answer(r, now),
        });
    }

    /// The worker's result (`UserEvent::PageSaveFile`).
    pub(super) fn page_save_file_done(&mut self, pane: PaneId, id: u64, done: FileDone) {
        if !self.page_saves.get(&pane).is_some_and(|s| s.id == id) {
            return;
        }
        self.page_save_do(pane, move |s, now| s.file_done(done, now));
    }

    /// The person cancelled, or the pane is gone: the partial output is removed (the worker finishes
    /// the removals), nothing more is written, no alert.
    pub(super) fn page_save_cancel(&mut self, pane: PaneId) {
        let now = self.page_save_epoch(pane);
        let Some(mut saving) = self.page_saves.remove(&pane) else { return };
        let ops = saving.cancel_ops(now);
        if let Some(w) = &saving.worker {
            for op in ops {
                let _ = w.send(op);
            }
        }
    }

    /// "Save Failed": heading and text, one OK, nothing else.
    fn page_save_alert(&mut self, text: &str) {
        let text = crate::inert_files::display_text(text, 500, false);
        self.ask_scripts(ask(page_saver::SAVE_FAILED, &text, &["OK"], None, Kind::Persistent, Then::Nothing));
    }
}

enum Answer {
    Page(Result<Value, String>),
    Tree(Result<Value, String>),
    Content(Result<Value, String>),
}

#[cfg(test)]
mod tests {
    use super::super::browser_test_seam::{Runner, ScriptSeam};
    use super::*;
    use base64::Engine;

    /// The Explorer command line a finished save runs: the one reveal form of `reveal_in_explorer`
    /// (`/select,"<path>"` through `semantic::explorer_select_arg`).
    fn reveal_line(path: &Path) -> String {
        format!("explorer.exe {}", crate::semantic::explorer_select_arg(&path.to_string_lossy()).unwrap_or_default())
    }

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("tandem-psave-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    /// The app's part played in a test: page and DevTools answers come from the script seam's queue,
    /// the files from the real file system, one at a time and in order.
    struct Harness {
        s: Saving,
        seam: ScriptSeam,
        reveals: Vec<String>,
        fails: Vec<String>,
        epoch: u64,
        /// File operations carried out, in order.
        ops: Vec<(FileKind, PathBuf)>,
        /// Keep the first page-world call back (in `held`) so a test can act between the folders
        /// and the extract.
        hold: bool,
        held: Vec<Act>,
        /// Every mark fails, as on a drive without streams (FAT32, exFAT).
        unmarkable: bool,
        /// Keep every file write back (in `held`), as writes still queued in the worker.
        hold_writes: bool,
    }

    impl Harness {
        fn new(folder: PathBuf, answers: Vec<Result<Value, String>>) -> Harness {
            let mut seam = ScriptSeam::default();
            seam.answers.extend(answers);
            Harness { s: Saving::new(folder, "https://tandem.test/".into(), 7, 1), seam, reveals: Vec::new(), fails: Vec::new(), epoch: 7, ops: Vec::new(), hold: false, held: Vec::new(), unmarkable: false, hold_writes: false }
        }

        fn go(&mut self) {
            let acts = self.s.begin(self.epoch);
            self.exec(acts);
        }

        fn exec(&mut self, acts: Vec<Act>) {
            let mut todo: VecDeque<Act> = acts.into();
            while let Some(a) = todo.pop_front() {
                let now = self.epoch;
                let more = match a {
                    Act::Page(b) if self.hold => {
                        self.hold = false;
                        self.held.push(Act::Page(b));
                        Vec::new()
                    }
                    Act::Page(b) => {
                        let r = self.seam.run(1, Runner::Page, &b);
                        self.s.page_answer(r, now)
                    }
                    Act::Tree => {
                        let r = self.seam.run(1, Runner::DevTools, "Page.getResourceTree {}");
                        self.s.tree_answer(r, now)
                    }
                    Act::Content { frame, url } => {
                        let params = crate::devtools::resource_content_params(&frame, &url);
                        assert!(crate::devtools::refusal("Page.getResourceContent", &params).is_none());
                        let r = self.seam.run(1, Runner::DevTools, &format!("Page.getResourceContent {params}"));
                        self.s.content_answer(r, now)
                    }
                    Act::File(op) if self.hold_writes && op.kind == FileKind::Write => {
                        self.held.push(Act::File(op));
                        Vec::new()
                    }
                    Act::File(op) => {
                        self.ops.push((op.kind, op.path.clone()));
                        let d = if self.unmarkable { run_op_with(&op, &|_, _| Err(std::io::Error::other("no streams"))) } else { run_op(&op) };
                        self.s.file_done(d, now)
                    }
                    Act::Reveal(p) => {
                        self.reveals.push(reveal_line(&p));
                        Vec::new()
                    }
                    Act::Fail(t) => {
                        self.fails.push(t);
                        Vec::new()
                    }
                };
                todo.extend(more);
            }
        }

        fn calls(&self) -> Vec<String> {
            self.seam.calls.iter().map(|(_, r, t)| format!("{r:?} {}", t.chars().take(40).collect::<String>())).collect()
        }
    }

    fn tree() -> Value {
        json!({"frameTree": {"frame": {"id": "F1", "url": "https://tandem.test/", "mimeType": "text/html"},
            "resources": [{"url": "https://tandem.test/i.png", "mimeType": "image/png"}]}})
    }

    fn png() -> Value {
        json!({"content": base64::engine::general_purpose::STANDARD.encode([1u8, 2, 3, 4]), "base64Encoded": true})
    }

    fn listing(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
        v.sort();
        v
    }

    /// A complete save writes `index.html` and `resources\i.png` under the folder and nowhere else,
    /// reads each listed resource once, skips one the page did not load, and reveals `index.html`.
    #[test]
    fn save_flow_follows_iterm2() {
        let root = temp_root("flow");
        let folder = root.join("site");
        let urls = json!(["https://tandem.test/i.png", "https://tandem.test/gone.png"]);
        let mut h = Harness::new(folder.clone(), vec![Ok(urls), Ok(tree()), Ok(png()), Ok(Value::Null), Ok(json!("<html>hi</html>"))]);
        h.go();
        assert!(h.fails.is_empty(), "{:?}", h.fails);
        assert_eq!(listing(&root), vec!["site"], "nothing outside the chosen folder");
        assert_eq!(listing(&folder), vec!["index.html", "resources"]);
        assert_eq!(std::fs::read(folder.join("resources").join("i.png")).unwrap(), vec![1, 2, 3, 4]);
        assert_eq!(std::fs::read_to_string(folder.join("index.html")).unwrap(), "<html>hi</html>");
        assert_eq!(h.reveals, vec![format!("explorer.exe /select,\"{}\"", folder.join("index.html").display())]);
        // The three saver calls run in the page world; one tree read and one content read, for the
        // frame the tree names; the unlisted URL asked for nothing.
        let calls = h.calls();
        assert_eq!(calls.len(), 5, "{calls:?}");
        assert!(calls[0].starts_with("Page ") && calls[3].starts_with("Page ") && calls[4].starts_with("Page "));
        assert_eq!(calls[1], "DevTools Page.getResourceTree {}");
        assert!(h.seam.calls[2].2.contains(r#""frameId":"F1""#) && h.seam.calls[2].2.contains("https://tandem.test/i.png"));
        // No saver body holds a view secret (page world: no bridge, no secret).
        let secret = crate::browser_js::new_secret();
        for (_, r, body) in &h.seam.calls {
            assert!(!body.contains(&secret) && !body.contains("chrome.webview"), "{r:?}");
        }
        // Every written path's parent is the folder or its resources folder.
        for (k, p) in &h.ops {
            if *k == FileKind::Write {
                let parent = p.parent().unwrap();
                assert!(parent == folder || parent == folder.join("resources"), "{p:?}");
            }
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An existing folder fails the save as Save Failed with the file error, and nothing is written.
    #[test]
    fn an_existing_folder_fails_the_save() {
        let root = temp_root("exists");
        let folder = root.join("site");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("keep.txt"), "mine").unwrap();
        let mut h = Harness::new(folder.clone(), vec![Ok(json!([])), Ok(Value::Null), Ok(json!("<p>"))]);
        h.go();
        assert_eq!(h.fails.len(), 1, "{:?}", h.fails);
        assert!(h.fails[0].starts_with("Could not save the page: Failed to create save directory"), "{}", h.fails[0]);
        assert!(h.reveals.is_empty());
        assert_eq!(listing(&folder), vec!["keep.txt"], "an existing folder is never written into or removed");
        assert_eq!(std::fs::read_to_string(folder.join("keep.txt")).unwrap(), "mine");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A resource whose write fails is skipped and the save still reveals `index.html`; a failure of
    /// `index.html` itself is Save Failed and the partial output is gone.
    #[test]
    fn write_failures_follow_iterm2() {
        let root2 = temp_root("writes2");
        let folder2 = root2.join("site");
        let mut h2 = Harness::new(folder2.clone(), vec![Ok(json!(["https://tandem.test/i.png"])), Ok(tree()), Ok(png()), Ok(Value::Null), Ok(json!("<p>ok</p>"))]);
        h2.hold = true;
        h2.go();
        let extract = h2.held.pop().unwrap();
        // A folder named like the resource, under resources: the create-new write fails.
        std::fs::create_dir(folder2.join("resources").join("i.png")).unwrap();
        h2.exec(vec![extract]);
        assert!(h2.fails.is_empty(), "{:?}", h2.fails);
        assert_eq!(h2.reveals.len(), 1, "the save still reveals index.html");
        assert!(folder2.join("index.html").is_file());
        assert!(folder2.join("resources").join("i.png").is_dir(), "the resource name was a folder: skipped");
        let _ = std::fs::remove_dir_all(&root2);

        // index.html cannot be written (a folder of that name): Save Failed, and the files and
        // folders this job made are removed.
        let root3 = temp_root("writes3");
        let folder3 = root3.join("site");
        let mut h3 = Harness::new(folder3.clone(), vec![Ok(json!([])), Ok(Value::Null), Ok(json!("<p>ok</p>"))]);
        h3.hold = true;
        h3.go();
        let page = h3.held.pop().unwrap();
        std::fs::create_dir(folder3.join("index.html")).unwrap();
        h3.exec(vec![page]);
        assert_eq!(h3.fails.len(), 1, "{:?}", h3.fails);
        assert!(h3.fails[0].starts_with("Could not save the page: Failed to save file"), "{}", h3.fails[0]);
        assert!(h3.reveals.is_empty());
        assert_eq!(listing(&folder3), vec!["index.html"], "resources is gone; only the planted folder is left");
        assert!(folder3.join("index.html").is_dir(), "a folder the job did not make is never removed");
        let _ = std::fs::remove_dir_all(&root3);
    }

    /// A navigation mid-save aborts: nothing more is written, the output is removed, one alert.
    #[test]
    fn a_navigation_aborts_the_save() {
        let root = temp_root("nav");
        let folder = root.join("site");
        let mut h = Harness::new(folder.clone(), vec![Ok(json!(["https://tandem.test/i.png"])), Ok(tree()), Ok(png())]);
        h.hold = true;
        h.go();
        let extract = h.held.pop().unwrap();
        assert!(folder.join("resources").is_dir());
        h.epoch = 8;
        h.exec(vec![extract]);
        assert_eq!(h.fails.len(), 1);
        assert!(h.fails[0].contains("navigated away"), "{}", h.fails[0]);
        assert!(!folder.exists(), "the folders this save made are gone");
        assert!(h.reveals.is_empty());
        assert_eq!(h.seam.calls.len(), 1, "no resource read after the navigation");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The pane closing (or the person cancelling) drops the job: it removes its output and asks for
    /// nothing more, with no alert.
    #[test]
    fn cancel_writes_nothing_more() {
        let root = temp_root("cancel");
        let folder = root.join("site");
        let mut h = Harness::new(folder.clone(), vec![Ok(json!(["https://tandem.test/i.png"]))]);
        h.hold = true;
        h.go();
        let _extract = h.held.pop().unwrap();
        let acts = h.s.cancel(7);
        assert!(acts.iter().all(|a| matches!(a, Act::File(op) if matches!(op.kind, FileKind::Remove | FileKind::RemoveDir))), "{acts:?}");
        h.exec(acts);
        assert!(!folder.exists());
        assert!(h.s.over && h.fails.is_empty() && h.reveals.is_empty());
        // A late answer of the page does nothing.
        assert!(h.s.page_answer(Ok(json!(["https://tandem.test/i.png"])), 7).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A cancel that lands while writes are still queued in the worker (Task 10 review minor 6): the
    /// worker runs the queue in order, so each queued write runs before the removal the cancel
    /// sends after it, and nothing is left.
    #[test]
    fn cancel_with_writes_queued_leaves_nothing() {
        let root = temp_root("cancelq");
        let folder = root.join("site");
        let urls = json!(["https://tandem.test/i.png"]);
        let mut h = Harness::new(folder.clone(), vec![Ok(urls), Ok(tree()), Ok(png())]);
        h.hold_writes = true;
        h.go();
        assert!(matches!(h.held.as_slice(), [Act::File(FileOp { kind: FileKind::Write, .. })]), "{:?}", h.held);
        let cleanup = h.s.cancel(7);
        assert!(cleanup.iter().any(|a| matches!(a, Act::File(op) if op.kind == FileKind::Remove && op.path == folder.join("resources").join("i.png"))), "{cleanup:?}");
        // The worker's order: the queued write first, then the cleanup the cancel sent.
        h.hold_writes = false;
        let queued: Vec<Act> = h.held.drain(..).chain(cleanup).collect();
        h.exec(queued);
        assert!(!folder.exists(), "{:?}", h.ops);
        assert!(h.fails.is_empty() && h.reveals.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Final review minor 3: a cancel (or the pane closing) while the save folder is being made, before
    /// its answer, leaves no empty folder. The worker carries out the queued MakeDir, then exactly
    /// what `page_save_cancel` sends; the late answer goes nowhere (the save is gone).
    #[test]
    fn a_cancel_while_a_folder_is_made_leaves_no_folder() {
        let root = temp_root("cancelmk");
        let folder = root.join("site");
        let mut s = Saving::new(folder.clone(), "https://tandem.test/".into(), 7, 1);
        let first = s.begin(7);
        let made: Vec<FileOp> = first.into_iter().filter_map(|a| if let Act::File(op) = a { Some(op) } else { None }).collect();
        assert!(matches!(made.as_slice(), [FileOp { kind: FileKind::MakeDir, .. }]), "{made:?}");
        let (tx, rx) = std::sync::mpsc::channel::<FileOp>();
        for op in made.into_iter().chain(s.cancel_ops(7)) {
            tx.send(op).unwrap();
        }
        drop(tx);
        page_save_worker(rx, &|_| {});
        assert!(listing(&root).is_empty(), "the cancelled save left {:?}", listing(&root));
        // A folder that was there before is never removed: the same race against an existing one.
        std::fs::create_dir(&folder).unwrap();
        let mut s = Saving::new(folder.clone(), "https://tandem.test/".into(), 7, 2);
        let first = s.begin(7);
        let (tx, rx) = std::sync::mpsc::channel::<FileOp>();
        for a in first {
            if let Act::File(op) = a {
                tx.send(op).unwrap();
            }
        }
        for op in s.cancel_ops(7) {
            tx.send(op).unwrap();
        }
        drop(tx);
        page_save_worker(rx, &|_| {});
        assert!(folder.is_dir(), "an existing folder stays");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Final review minor 3, the failure path: the page's script fails while `resources` is being
    /// made (the MakeDir overlaps the extract). The job's cleanup cannot remove `site` (not empty
    /// yet) and the `resources` answer arrives after the save is gone; the app sends the abandon
    /// after the failure's operations, so neither folder is left. A file the worker could not
    /// remove keeps its folder (only empty folders go).
    #[test]
    fn a_failure_while_a_folder_is_made_leaves_no_folder() {
        let root = temp_root("failmk");
        let folder = root.join("site");
        let mut s = Saving::new(folder.clone(), "https://tandem.test/".into(), 7, 1);
        let ops = |acts: Vec<Act>| -> Vec<FileOp> { acts.into_iter().filter_map(|a| if let Act::File(op) = a { Some(op) } else { None }).collect() };
        // One worker for the whole save, as the app has: it makes `site` too.
        let (tx, rx) = std::sync::mpsc::channel::<FileOp>();
        let (dtx, drx) = std::sync::mpsc::channel::<FileDone>();
        let worker = std::thread::spawn(move || page_save_worker(rx, &|d| {
            let _ = dtx.send(d);
        }));
        let first = ops(s.begin(7));
        assert_eq!(first.len(), 1);
        tx.send(first[0].clone()).unwrap();
        let after = s.file_done(drx.recv().unwrap(), 7);
        assert!(after.iter().any(|a| matches!(a, Act::Page(_))), "the extract runs while resources is made: {after:?}");
        let resources = ops(after);
        assert!(matches!(resources.as_slice(), [FileOp { kind: FileKind::MakeDir, .. }]), "{resources:?}");
        // The MakeDir is in flight; its answer will find the save gone.
        tx.send(resources[0].clone()).unwrap();
        let failed = s.page_answer(Err("boom".into()), 7);
        assert!(failed.iter().any(|a| matches!(a, Act::Fail(_))), "{failed:?}");
        // The failure's cleanup, then the abandon the app sends at Act::Fail.
        for op in ops(failed).into_iter().chain([s.abandon_op()]) {
            tx.send(op).unwrap();
        }
        drop(tx);
        worker.join().unwrap();
        assert!(listing(&root).is_empty(), "the failed save left {:?}", listing(&root));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The app sends the abandon at both unfinished ends, after their operations, and never on a
    /// finished save.
    #[test]
    fn unfinished_saves_send_the_abandon() {
        let src = include_str!("page_saver_app.rs");
        let prod = &src[..src.find("\n#[cfg(test)]\nmod tests").unwrap()];
        let fail = &prod[prod.find("Act::Fail(text) => {").unwrap()..];
        let fail = &fail[..fail.find("self.page_save_alert(&text);").unwrap()];
        assert!(fail.contains("let _ = w.send(saving.abandon_op());"), "{fail}");
        let cancel = &prod[prod.find("pub(super) fn page_save_cancel").unwrap()..];
        let cancel = &cancel[..cancel.find("\n    }\n").unwrap()];
        assert!(cancel.contains("let ops = saving.cancel_ops(now);"));
        let reveal = &prod[prod.find("Act::Reveal(path) => {").unwrap()..prod.find("Act::Fail(text) => {").unwrap()];
        assert!(!reveal.contains("abandon"), "a finished save keeps its folders");
        let mut s = Saving::new(PathBuf::from("C:\\x\\site"), String::new(), 7, 1);
        let _ = s.begin(7);
        assert_eq!(s.cancel_ops(7).last().map(|o| o.kind), Some(FileKind::Abandon), "the abandon is the cancel's last operation");
    }

    /// The epoch is bound when the menu item is chosen, before the dialog (Task 10 review minor 7):
    /// a page that navigated while the dialog was open is not saved, and nothing is made.
    #[test]
    fn a_navigation_during_the_dialog_saves_nothing() {
        let src = include_str!("page_saver_app.rs");
        let prod = &src[..src.find("\n#[cfg(test)]\nmod tests").unwrap()];
        let menu = &prod[prod.find("pub(super) fn save_page_as").unwrap()..prod.find("pub(super) fn start_page_save").unwrap()];
        let bound = menu.find("let epoch = self.page_save_epoch(pane);").expect("the epoch is read in save_page_as");
        assert!(bound < menu.find("crate::localfs::save_file(dialog)").unwrap(), "bound before the dialog");
        assert!(menu.contains("self.start_page_save(pane, picked.into_path_buf(), url, epoch);"));
        let start = &prod[prod.find("pub(super) fn start_page_save").unwrap()..];
        let recheck = start.find("if self.page_save_epoch(pane) != epoch {").expect("rechecked after the dialog");
        assert!(recheck < start.find("Saving::new(").unwrap() && recheck < start.find("std::thread::Builder::new()").unwrap());
        // The job bound to the old epoch makes nothing once the page moved.
        let root = temp_root("dialognav");
        let folder = root.join("site");
        let mut h = Harness::new(folder.clone(), vec![]);
        h.epoch = 8;
        h.go();
        assert_eq!(h.fails, vec![page_saver::save_failed_text(page_saver::NAVIGATED_AWAY)]);
        assert!(!folder.exists() && h.ops.is_empty(), "{:?}", h.ops);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A saver script or a file name the page chose never reaches the alert unsanitized, and a
    /// resource the door refuses (a shortcut, a program) is skipped.
    #[test]
    fn the_alert_shows_sanitized_text_and_the_door_refuses_types() {
        let root = temp_root("door");
        let folder = root.join("site");
        let t = json!({"frameTree": {"frame": {"id": "F1", "url": "https://tandem.test/", "mimeType": "text/html"},
            "resources": [{"url": "https://tandem.test/a.url", "mimeType": "application/octet-stream"}, {"url": "https://tandem.test/desktop.ini", "mimeType": "text/plain"}, {"url": "https://tandem.test/s.scf", "mimeType": "text/plain"}]}});
        let body = json!({"content": "x", "base64Encoded": false});
        let urls = json!(["https://tandem.test/a.url", "https://tandem.test/desktop.ini", "https://tandem.test/s.scf"]);
        let mut h = Harness::new(folder.clone(), vec![Ok(urls), Ok(t), Ok(body.clone()), Ok(body.clone()), Ok(body), Ok(Value::Null), Ok(json!("<p>"))]);
        h.go();
        assert!(h.fails.is_empty(), "{:?}", h.fails);
        let saved = listing(&folder.join("resources"));
        for bad in &saved {
            assert!(!crate::inert_files::refused_write(bad), "{bad}");
        }
        assert!(!saved.iter().any(|n| n.ends_with(".url") || n.ends_with(".scf") || n == "desktop.ini"), "{saved:?}");
        let _ = std::fs::remove_dir_all(&root);
        // Hidden characters in a failure never reach the alert.
        let mut s = Saving::new(PathBuf::from("C:\\x"), String::new(), 1, 1);
        s.io_error = Some("bad \u{202e}name\u{1b}[31m".into());
        let acts = s.run(vec![Step::Failed("Failed to save file".into())], 1);
        assert_eq!(acts, vec![Act::Fail("Could not save the page: Failed to save file: bad name[31m".into())]);
    }

    /// A drive that cannot hold the Mark of the Web fails the save with a clear alert and cleans up;
    /// never a silent unmarked save (Task 10 review I1, controller ruling).
    #[test]
    fn an_unmarkable_drive_fails_the_save() {
        let root = temp_root("unmarked");
        let folder = root.join("site");
        let mut h = Harness::new(folder.clone(), vec![Ok(json!(["https://tandem.test/i.png"])), Ok(tree()), Ok(png()), Ok(Value::Null), Ok(json!("<p>ok</p>"))]);
        h.unmarkable = true;
        h.go();
        assert_eq!(h.fails, vec![page_saver::save_failed_text(crate::inert_files::UNMARKABLE)]);
        assert!(h.reveals.is_empty());
        assert_eq!(listing(&root), Vec::<String>::new(), "the save folder and everything in it are gone");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The alert names the failing operation's own error, never an earlier skipped resource's, and
    /// never page-authored text (Task 10 review minors 2 and 3).
    #[test]
    fn the_alert_names_the_right_cause() {
        let folder = PathBuf::from("C:\\x\\site");
        let skipped = |s: &mut Saving| {
            let d = FileDone { kind: FileKind::Write, path: folder.join("resources").join("i.png"), err: Some("resource error".into()), unmarked: false };
            assert!(s.file_done(d, 1).is_empty());
        };
        // A navigation after a skipped resource: the reason alone.
        let mut s = Saving::new(folder.clone(), String::new(), 1, 1);
        skipped(&mut s);
        assert_eq!(s.run(vec![Step::Failed(page_saver::NAVIGATED_AWAY.into())], 1), vec![Act::Fail(page_saver::save_failed_text(page_saver::NAVIGATED_AWAY))]);
        // index.html failing after a skipped resource: index.html's error.
        let mut s = Saving::new(folder.clone(), String::new(), 1, 1);
        skipped(&mut s);
        let d = FileDone { kind: FileKind::Write, path: folder.join("index.html"), err: Some("index error".into()), unmarked: false };
        let acts = s.file_done(d, 1);
        assert_eq!(acts.last(), Some(&Act::Fail(page_saver::save_failed_text("Failed to save file: index error"))), "{acts:?}");
        // A page-world exception (a hostile page can choose its message) shows Tandem's fixed text.
        let root = temp_root("pageerr");
        let mut h = Harness::new(root.join("site"), vec![Err("Uncaught Error: Your files are encrypted, call 555".into())]);
        h.go();
        assert_eq!(h.fails, vec![page_saver::save_failed_text(page_saver::FAILED_TO_GET_HTML)]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Every file the saver writes carries the Mark of the Web (real NTFS stream), with the page's URL.
    #[cfg(windows)]
    #[test]
    fn every_saved_file_is_marked() {
        let root = temp_root("zone");
        let folder = root.join("site");
        let mut h = Harness::new(folder.clone(), vec![Ok(json!(["https://tandem.test/i.png"])), Ok(tree()), Ok(png()), Ok(Value::Null), Ok(json!("<p>ok</p>"))]);
        h.go();
        assert_eq!(h.reveals.len(), 1);
        for f in [folder.join("index.html"), folder.join("resources").join("i.png")] {
            let stream = format!("{}:Zone.Identifier", f.display());
            let text = crate::localfs::read_to_string(&crate::localfs::judge_io(&stream).unwrap()).unwrap_or_else(|e| panic!("{f:?}: {e}"));
            assert_eq!(crate::inert_files::zone_of(&text), Some(3), "{f:?}");
            assert!(text.contains("HostUrl=https://tandem.test/\r\n"), "{text:?}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// B5: a page that lists 5000 resources is capped (the job's own cap), and the flow asks for no
    /// more than the cap.
    #[test]
    fn a_huge_url_list_is_capped() {
        let root = temp_root("cap");
        let folder = root.join("site");
        let urls: Vec<String> = (0..5000).map(|i| format!("https://tandem.test/r{i}.png")).collect();
        let resources: Vec<Value> = urls.iter().map(|u| json!({"url": u, "mimeType": "image/png"})).collect();
        let t = json!({"frameTree": {"frame": {"id": "F1", "url": "https://tandem.test/", "mimeType": "text/html"}, "resources": resources}});
        let mut answers = vec![Ok(json!(urls)), Ok(t)];
        for _ in 0..2000 {
            answers.push(Ok(json!({"content": "x", "base64Encoded": false})));
        }
        answers.push(Ok(Value::Null));
        answers.push(Ok(json!("<p>")));
        let mut h = Harness::new(folder.clone(), answers);
        h.go();
        let reads = h.seam.calls.iter().filter(|(_, r, t)| *r == Runner::DevTools && t.starts_with("Page.getResourceContent")).count();
        assert!(reads <= page_saver::MAX_RESOURCES, "{reads}");
        assert_eq!(h.reveals.len(), 1, "{:?}", h.fails);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The wiring: the dialog is the person's pick (no preset folder), the destination is judged
    /// before anything is written, the job runs through the worker, and the menu pick reaches it.
    #[test]
    fn save_page_as_is_wired_safely() {
        let src = include_str!("page_saver_app.rs");
        let prod = &src[..src.find("\n#[cfg(test)]\nmod tests").unwrap()];
        assert!(prod.contains("crate::localfs::save_file(dialog)") && !prod.contains("set_directory"), "no default folder");
        let start = &prod[prod.find("fn start_page_save").unwrap()..];
        assert!(start.find("page_saver::destination(&folder)").unwrap() < start.find("Saving::new(").unwrap());
        assert!(start.contains("std::thread::Builder::new()"));
        // No raw file system call, no process start, no shell open in this file's production code.
        for banned in ["std::fs::", "Command::new(", "open_with_shell(", "shell_open(", "open_url"] {
            assert!(!prod.contains(banned), "{banned}");
        }
        // One write door and the permits check before every step; the page runner is the page-world one.
        assert_eq!(prod.matches("write_new_checked_with(").count(), 1);
        assert!(prod.contains("self.job.permits(&step)") && prod.contains("self.job.check_epoch(now)") && prod.contains("self.job.attributes_body()"));
        assert!(prod.contains("self.browser_page_call(pane, &body, json!({})") && !prod.contains("browser_js_call("));
        assert!(prod.contains("self.devtools_read(pane, \"Page.getResourceTree\"") && prod.contains("self.devtools_read(pane, \"Page.getResourceContent\""));
        assert!(prod.contains("Kind::Persistent, Then::Nothing"));
        let ctx = include_str!("browser_ctx_app.rs");
        let ctx = &ctx[..ctx.find("#[cfg(test)]").unwrap()];
        assert!(ctx.contains("Item::SavePageAs =>") && ctx.contains("save_page: true") && ctx.contains("self.save_page_as(pane)"));
        // The dialog opens after the window closure, never inside it.
        assert!(ctx.find("self.save_page_as(pane)").unwrap() > ctx.find("self.in_pane_window(pane,").unwrap());
        // The one Explorer reveal, for the job's own Done path; nothing is opened or run.
        assert_eq!(prod.matches("self.reveal_in_explorer(").count(), 1);
        assert!(prod.contains("Act::Reveal(path) => {\n                    self.page_saves.remove(&pane);\n                    self.reveal_in_explorer(&path.to_string_lossy());"));
        let m = include_str!("mod.rs");
        assert!(m.contains("self.page_save_cancel(id);") && m.contains("UserEvent::PageSaveFile { pane, id, done }"));
    }
}

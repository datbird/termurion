//! git state on the status bar (milestone 9i, Task 5): a `GitPoller` per pane whose bar
//! has a git component, the worker threads that run git off the UI thread, and what the
//! component shows (`git_content`). The pure git model is `crate::git_state`; the runner is
//! `crate::git_run_win`.
//!
//! Threads. The UI thread only asks and drains a channel. One thread runs per path in
//! flight (a second request for the path joins it, `GPW:94-201`); at most `MAX_THREADS`
//! live at once, the rest wait in a queue. A thread still busy at its deadline (git is
//! killed there, but the checks before it can block on a dead mapped drive) is abandoned:
//! the UI reports the timeout itself, and the thread's late answer is ignored. It still
//! counts against the cap until it ends, so a flood of dead drives cannot grow threads.
//!
//! Citations use the milestone 9i brief's abbreviations (`GIT` =
//! `iTermStatusBarGitComponent.m`, `GP` = `iTermGitPoller.m`, `GPW` = `iTermGitPollWorker.m`,
//! `GSM` = `iTermGitStringMaker.m`).

use super::statusbar_content::{Content, MenuAt, is_this_machine, shown_path};
use super::statusbar_ui::{Click, StatusMenu};
use super::*;
use crate::broadcast::Write;
use crate::git_state::{self as gs, Fetched, GitCommand, GitPoller, GitRun, GitState, Lookup, PollGate, Polls, WorkerCache};
use crate::statusbar::Component;
use crate::statusbar_components::{self as sbc, Kind};
use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

/// Worker threads alive at once, abandoned ones included.
const MAX_THREADS: usize = 8;
/// How long past its deadline a thread may answer before the UI reports the timeout (the
/// runner kills git at the deadline; this is the time to hand the answer over).
const GRACE: Duration = Duration::from_millis(300);
const TIMEOUT_KEY: &str = "GitTimeout";
const SEARCH_PATH_KEY: &str = "GitSearchPath";
/// `gitTimeout`'s default (`ADV:534`).
const DEFAULT_TIMEOUT: f64 = 4.0;
/// The info bars standing in for iTerm2's git warnings and alert (headings dropped).
pub(super) const TIMEOUT_BAR: &str = "statusbar:git-timeout";
pub(super) const SEND_BAR: &str = "statusbar:git-send";
pub(super) const DEBUG_BAR: &str = "statusbar:git-debug";
/// A menu command refused because its folder is gone (Termurion's own; OK only).
pub(super) const GONE_BAR: &str = "statusbar:git-gone";
/// The git menu's item ids after `statusbar:`.
const MENU_ID: &str = "git:";
/// The wrapper a single-use window runs, beside `termurion.exe`.
const WRAPPER: &str = crate::git_wrapper::WRAPPER_EXE;

/// A session opened for a git menu command (`openSingleUseWindowWithCommand:`): the pane
/// whose component ran it, and whether it is short-lived (no Session Ended alert, no Undo
/// Close).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SingleUse {
    pub(super) owner: PaneId,
    pub(super) short_lived: bool,
    /// What the window does when its command ends. A git window follows no profile: iTerm2
    /// opens it with only `iTermSingleUseWindowOptionsShortLived`, so its end action is the
    /// default, No Action (`IC:2186-2188`): it stays and shows "Finished".
    pub(super) end_action: crate::session_end::EndAction,
}

/// The single-use record of a git menu command's window (R3: git windows stay at their end).
pub(super) fn git_single_use(owner: PaneId, short_lived: bool) -> SingleUse {
    SingleUse { owner, short_lived, end_action: crate::session_end::EndAction::NoAction }
}

/// The single-use record of a Run Command in Window pane (13r P20): never short-lived, owned by
/// the clicked pane, and its end follows the default profile's "After a session ends"
/// (`profile_end_value`, the window is of that profile). Being single-use, it is never restarted
/// (iTerm2's `isRestartable`): a restart would run the profile's shell, not the command.
pub(super) fn run_window_single_use(owner: PaneId, profile_end_value: i64) -> SingleUse {
    SingleUse { owner, short_lived: false, end_action: crate::session_end::EndAction::from_int(profile_end_value) }
}

/// What a git info bar's button does.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum GitBarAnswer {
    /// Store this `gitTimeout` and retry (`GP:77-84`).
    IncreaseTimeout(f64),
    /// Type the remote command.
    Send,
    /// Put the Debug Info on the clipboard.
    Copy,
    Nothing,
}

/// The answer a git info bar's button gives: the timeout warning's "Increase to Ns" (the
/// number it names), Send Command's OK, Debug Info's Copy; anything else does nothing.
pub(super) fn git_bar_answer(id: &str, button: &str) -> GitBarAnswer {
    match (id, button) {
        (TIMEOUT_BAR, b) => b.strip_prefix("Increase to ").and_then(|b| b.strip_suffix('s')).and_then(|n| n.parse::<f64>().ok()).map_or(GitBarAnswer::Nothing, GitBarAnswer::IncreaseTimeout),
        (SEND_BAR, "OK") => GitBarAnswer::Send,
        (DEBUG_BAR, "Copy") => GitBarAnswer::Copy,
        _ => GitBarAnswer::Nothing,
    }
}

/// A single-use window's command: `termurion-git.exe` in `exe_dir`, the git it runs, and the
/// command's arguments (a branch never read as an option).
pub(super) fn single_use_command(exe_dir: &Path, git: &Path, command: &GitCommand) -> String {
    crate::git_wrapper::command_line(&exe_dir.join(WRAPPER), git, &command.args())
}

/// The Debug Info text for a poller (`GIT:513-536`): its directory, cadence, enabled flag,
/// seconds since it last polled, its state, and the worker's `cache` line.
pub(super) fn git_debug_text(poller: &GitPoller, cache: &str, now: Instant) -> String {
    let since = poller.last_poll.map(|t| now.saturating_duration_since(t).as_secs_f64());
    gs::debug_text(poller.directory.as_deref(), poller.cadence, poller.enabled, since, poller.state.as_ref(), cache)
}

/// A text as an info bar shows it: one line, newlines as spaces.
pub(super) fn bar_text(text: &str) -> String {
    text.replace('\n', " ")
}

/// One pane's git component state (the first git component of its bar).
pub(super) struct GitPane {
    pub(super) poller: GitPoller,
    /// A menu command's running text (`"Pushing\u{2026}"`), while its window runs.
    pub(super) status: Option<&'static str>,
    /// The single-use window's pane running that command.
    pub(super) command_pane: Option<PaneId>,
    /// `lastCommand` when last looked at.
    pub(super) last_command: Option<String>,
    /// The component was placed on a bar on screen when last looked at.
    pub(super) was_visible: bool,
    /// Where the component was last clicked: the menu opens there once the recent
    /// branches arrive, and so does the Log popover.
    pub(super) menu_at: Option<MenuAt>,
    /// Waiting for the recent branches (a second click meanwhile is ignored).
    pub(super) waiting: bool,
    /// The state and directory when the menu was asked for (`iTermGitMenuItemContext`).
    pub(super) menu_state: Option<GitState>,
    pub(super) menu_dir: Option<String>,
    /// The open menu's commands, by item number.
    pub(super) picks: Vec<GitCommand>,
    /// The remote command Send Command's OK types.
    pub(super) send: Option<String>,
    /// The Debug Info text Copy puts on the clipboard.
    pub(super) debug: Option<String>,
    /// A Log run is on its way: another Log meanwhile is ignored, so one popover opens.
    pub(super) log_pending: bool,
}

/// An answer from the worker.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum GitReply {
    /// A poll of `path`.
    State { path: String, fetched: Fetched },
    /// A workgroup poll (`key` is its `cache_key`; `path` the directory polled).
    Workgroup { key: String, path: String, fetched: Fetched, launch: Option<gs::Launch> },
    /// The recent branches a pane's menu asked for.
    Branches { pane: PaneId, path: String, branches: Vec<String> },
    /// The Log command's output for a pane.
    Log { pane: PaneId, text: String },
}

/// What a thread runs; also the key that joins requests.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Job {
    State(String),
    /// A workgroup's poll: the files and stats too, against `base` (None is HEAD).
    Workgroup { path: String, base: Option<String> },
    Branches(PaneId, String),
    Log(PaneId, String),
}

/// What a thread got for its job.
enum Output {
    State(Fetched),
    /// A workgroup poll with the verdict a launch needs, made on this thread.
    Workgroup(Fetched, gs::Launch),
    Branches(Vec<String>),
    Log(String),
}

impl Job {
    /// The answer for a job that timed out or never ran: a timeout for a poll, nothing for
    /// the others.
    fn nothing(&self, timed_out: bool) -> Output {
        match self {
            Job::State(_) | Job::Workgroup { .. } => Output::State(Fetched { state: None, timed_out }),
            Job::Branches(..) => Output::Branches(Vec::new()),
            Job::Log(..) => Output::Log(String::new()),
        }
    }
}

/// A request waiting for a thread. `search` is `gitSearchPath`: the thread resolves the
/// program (`git_program` looks at the disk, which can block on a dead drive).
struct Queued {
    job: Job,
    search: String,
    timeout: Duration,
    asked: Instant,
}

/// A thread in flight: its id, when its answer is due, and a request that came after it
/// started and wants a fresh run once it ends.
struct Flight {
    id: u64,
    due: Instant,
    again: Option<(String, Duration)>,
}

/// A thread's answer.
struct Done {
    id: u64,
    job: Job,
    finished: Instant,
    output: Output,
}

/// What threads send: an answer, then (however the thread ends) that it ended.
#[allow(clippy::large_enum_variant)] // built once and moved rarely; boxing the variant adds an indirection for no gain
enum Msg {
    Done(Done),
    Ended(Job, u64),
}

/// The git worker: iTerm2's poll worker cache on the UI thread, and the threads.
pub(super) struct GitWorker {
    cache: WorkerCache,
    runner: Arc<dyn GitRun>,
    notify: Notify,
    tx: mpsc::Sender<Msg>,
    rx: mpsc::Receiver<Msg>,
    flights: HashMap<Job, Flight>,
    /// Threads past their deadline that have not ended, by job: the job keeps its one
    /// thread, and requests for it are answered "timed out" without another.
    abandoned: HashMap<Job, u64>,
    queue: VecDeque<Queued>,
    live: Arc<AtomicUsize>,
    next_id: u64,
    /// Answers ready without a thread (the cache's, a stuck job's, a thread that could not
    /// start).
    ready: Vec<GitReply>,
}

/// Says a thread ended, counts it down and wakes the loop, however it ends.
struct Alive {
    live: Arc<AtomicUsize>,
    notify: Notify,
    tx: mpsc::Sender<Msg>,
    job: Job,
    id: u64,
}

impl Drop for Alive {
    fn drop(&mut self) {
        // The message first: a count of 0 means every end is in the channel.
        let _ = self.tx.send(Msg::Ended(self.job.clone(), self.id));
        self.live.fetch_sub(1, Ordering::AcqRel);
        (self.notify)();
    }
}

impl GitWorker {
    pub(super) fn new(runner: Arc<dyn GitRun>, notify: Notify) -> GitWorker {
        let (tx, rx) = mpsc::channel();
        GitWorker {
            cache: WorkerCache::default(),
            runner,
            notify,
            tx,
            rx,
            flights: HashMap::new(),
            abandoned: HashMap::new(),
            queue: VecDeque::new(),
            live: Arc::new(AtomicUsize::new(0)),
            next_id: 0,
            ready: Vec::new(),
        }
    }

    /// A poll of `path` with the git `search` (`gitSearchPath`) names, answered from the
    /// cache, by joining the run in flight, or by a new run.
    pub(super) fn request(&mut self, path: String, search: String, timeout: Duration) {
        let now = Instant::now();
        match self.cache.lookup(&path, now) {
            Lookup::Cached(state) => self.ready.push(GitReply::State { path, fetched: Fetched { state, timed_out: false } }),
            Lookup::Joined => {}
            Lookup::Start => self.start(Job::State(path), search, timeout, now),
        }
    }

    /// A workgroup's poll of `path` against `base` with the files and the diff stats, through the
    /// same cache (the key says which) and threads.
    pub(super) fn request_workgroup(&mut self, path: String, base: Option<String>, search: String, timeout: Duration) {
        let now = Instant::now();
        let key = gs::cache_key(&path, base.as_deref(), true);
        match self.cache.lookup(&key, now) {
            Lookup::Cached(state) => self.ready.push(GitReply::Workgroup { key, path, fetched: Fetched { state, timed_out: false }, launch: None }),
            Lookup::Joined => {}
            Lookup::Start => self.start(Job::Workgroup { path, base }, search, timeout, now),
        }
    }

    /// The 8 most recently committed branches in `path`, for `pane`'s menu.
    pub(super) fn branches(&mut self, pane: PaneId, path: String, search: String, timeout: Duration) {
        self.start(Job::Branches(pane, path), search, timeout, Instant::now());
    }

    /// `git log` in `path`, for `pane`'s Log popover (5 s, `GIT:504`).
    pub(super) fn log(&mut self, pane: PaneId, path: String, search: String) {
        self.start(Job::Log(pane, path), search, gs::LOG_TIMEOUT, Instant::now());
    }

    /// Forgets `path`'s cached state and waiting requests (`GPW:161-199`).
    pub(super) fn invalidate(&mut self, path: &str) {
        self.cache.invalidate(path);
    }

    fn start(&mut self, job: Job, search: String, timeout: Duration, now: Instant) {
        if self.abandoned.contains_key(&job) {
            // Its thread is still stuck (a dead drive): a timeout, and no second thread.
            let reply = self.answer(&job, job.nothing(true), now);
            self.ready.push(reply);
            return;
        }
        if let Some(f) = self.flights.get_mut(&job) {
            // One thread per path: this one reruns when the one in flight ends.
            f.again = Some((search, timeout));
            return;
        }
        if self.queue.iter().any(|q| q.job == job) {
            return;
        }
        self.queue.push_back(Queued { job, search, timeout, asked: now });
        self.spawn_queued(now);
    }

    /// Starts queued runs while there is room.
    fn spawn_queued(&mut self, now: Instant) {
        while self.live.load(Ordering::Acquire) < MAX_THREADS {
            let Some(q) = self.queue.pop_front() else { return };
            self.spawn(q, now);
        }
    }

    fn spawn(&mut self, q: Queued, now: Instant) {
        let id = self.next_id;
        self.next_id += 1;
        // The deadline counts from the request: time spent queued is time spent waiting.
        let left = q.timeout.saturating_sub(now.saturating_duration_since(q.asked));
        self.live.fetch_add(1, Ordering::AcqRel);
        let alive = Alive { live: self.live.clone(), notify: self.notify.clone(), tx: self.tx.clone(), job: q.job.clone(), id };
        let (runner, tx, job) = (self.runner.clone(), self.tx.clone(), q.job.clone());
        let search = q.search;
        let spawned = std::thread::Builder::new().name("termurion-git".into()).spawn(move || {
            let _alive = alive;
            let git = gs::git_program(&search);
            let output = match (&job, git) {
                (Job::State(path), Some(git)) => Output::State(gs::fetch_state(&*runner, &git, Path::new(path), left)),
                (Job::Workgroup { path, base }, Some(git)) => {
                    let fetched = gs::fetch_state_with(&*runner, &git, Path::new(path), left, base.as_deref(), true);
                    let launch = gs::Launch { vet: gs::vet_repo(Path::new(path)), git: Some(git) };
                    Output::Workgroup(fetched, launch)
                }
                (Job::Branches(_, path), Some(git)) => Output::Branches(gs::recent_branches(&*runner, &git, Path::new(path), left)),
                (Job::Log(_, path), Some(git)) => Output::Log(gs::git_log(&*runner, &git, Path::new(path), left)),
                // No git to run (a network or linked `gitSearchPath`): no state.
                (job, None) => job.nothing(false),
            };
            let _ = tx.send(Msg::Done(Done { id, job, finished: Instant::now(), output }));
        });
        match spawned {
            Ok(_) => {
                self.flights.insert(q.job, Flight { id, due: now + left + GRACE, again: None });
            }
            // The closure (and its `Alive`) is dropped: the count is back. No thread means
            // no state, as for a folder git may not run in.
            Err(_) => {
                let reply = self.answer(&q.job, q.job.nothing(false), now);
                self.ready.push(reply);
            }
        }
    }

    /// The reply for a finished (or abandoned) job, through the cache for a poll.
    fn answer(&mut self, job: &Job, output: Output, finished: Instant) -> GitReply {
        match (job, output) {
            (Job::State(path), Output::State(fetched)) => GitReply::State { path: path.clone(), fetched: self.cache.finish(path, fetched, finished) },
            (Job::Workgroup { path, base }, Output::State(fetched)) => {
                let key = gs::cache_key(path, base.as_deref(), true);
                GitReply::Workgroup { fetched: self.cache.finish(&key, fetched, finished), key, path: path.clone(), launch: None }
            }
            (Job::Workgroup { path, base }, Output::Workgroup(fetched, launch)) => {
                let key = gs::cache_key(path, base.as_deref(), true);
                GitReply::Workgroup { fetched: self.cache.finish(&key, fetched, finished), key, path: path.clone(), launch: Some(launch) }
            }
            (Job::Branches(pane, path), Output::Branches(branches)) => GitReply::Branches { pane: *pane, path: path.clone(), branches },
            (Job::Log(pane, _), Output::Log(text)) => GitReply::Log { pane: *pane, text },
            // A thread answers its own job's kind.
            (job, _) => self.answer(job, job.nothing(false), finished),
        }
    }

    /// Every answer ready now: threads that finished, threads past their deadline (a
    /// timeout, the thread abandoned), and queued requests that waited past theirs.
    pub(super) fn take(&mut self) -> Vec<GitReply> {
        let now = Instant::now();
        let mut again = Vec::new();
        let mut out = Vec::new();
        while let Ok(msg) = self.rx.try_recv() {
            let d = match msg {
                Msg::Done(d) => d,
                Msg::Ended(job, id) => {
                    if self.abandoned.get(&job) == Some(&id) {
                        self.abandoned.remove(&job);
                    }
                    continue;
                }
            };
            // An abandoned thread's late answer is not its job's any more.
            if self.flights.get(&d.job).is_none_or(|f| f.id != d.id) {
                continue;
            }
            let flight = self.flights.remove(&d.job).expect("checked above");
            out.push(self.answer(&d.job, d.output, d.finished));
            again.extend(flight.again.map(|(search, timeout)| (d.job, search, timeout)));
        }
        let late: Vec<Job> = self.flights.iter().filter(|(_, f)| now >= f.due).map(|(j, _)| j.clone()).collect();
        for job in late {
            let flight = self.flights.remove(&job).expect("listed above");
            self.abandoned.insert(job.clone(), flight.id);
            out.push(self.answer(&job, job.nothing(true), now));
            again.extend(flight.again.map(|(search, timeout)| (job, search, timeout)));
        }
        let (waited, queued): (Vec<Queued>, Vec<Queued>) = self.queue.drain(..).partition(|q| now >= q.asked + q.timeout);
        self.queue = queued.into();
        for q in waited {
            out.push(self.answer(&q.job, q.job.nothing(true), now));
        }
        for (job, search, timeout) in again {
            self.start(job, search, timeout, now);
        }
        self.spawn_queued(now);
        out.append(&mut self.ready);
        out
    }

    /// When `take` next has an answer: now with one ready, else a flight's or a queued
    /// request's deadline. Finished threads wake the loop themselves.
    pub(super) fn next_due(&self) -> Option<Instant> {
        let ready = (!self.ready.is_empty()).then(Instant::now);
        let flights = self.flights.values().map(|f| f.due);
        let queued = self.queue.iter().map(|q| q.asked + q.timeout);
        ready.into_iter().chain(flights).chain(queued).min()
    }
}

/// What the git component shows (`GIT:136-297`, `GSM:292-302`): the timeout warning's two
/// variants, else the rendered string; the minimum is the text for branch `M`; Copy is the
/// branch; empty without a branch unless the warning shows. `enabled` is the poller's.
pub(super) fn git_content_of(state: Option<&GitState>, status: Option<&str>, enabled: bool, show_timeout: bool) -> Content {
    let ready = state.is_some() && enabled;
    let variants = if show_timeout { vec![gs::TIMED_OUT_SHORT.to_string(), gs::TIMED_OUT_ICON.to_string()] } else { gs::git_variants(state, status, ready) };
    let branch = state.map(|s| s.branch.clone()).unwrap_or_default();
    Content {
        min_text: gs::git_text(state, status, ready, "M"),
        empty: !show_timeout && branch.is_empty(),
        uncopyable: branch.is_empty(),
        copy: (!branch.is_empty()).then_some(branch),
        click: Click::Menu(Kind::Git),
        icon: sbc::icon(Kind::Git),
        tail: true,
        ..Content::text(variants)
    }
}

/// The first git component's Polling Interval knob, else its default.
fn cadence_knob(c: &Component) -> f64 {
    let v = c.knobs.get(sbc::GIT_POLLING_INTERVAL_KEY).or(sbc::default_knobs(Kind::Git).get(sbc::GIT_POLLING_INTERVAL_KEY)).cloned();
    v.and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).unwrap_or(2.0)
}

impl App {
    /// `gitTimeout` as a deadline.
    pub(super) fn git_timeout(&self) -> Duration {
        gs::timeout_of(self.git_timeout_setting())
    }

    /// `gitTimeout` as set.
    pub(super) fn git_timeout_setting(&self) -> f64 {
        self.globals.get(TIMEOUT_KEY).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).unwrap_or(DEFAULT_TIMEOUT)
    }

    /// `gitSearchPath`, which the worker threads resolve (never this thread: the check
    /// reads the disk).
    pub(super) fn git_search_path(&self) -> String {
        self.globals.string(SEARCH_PATH_KEY)
    }

    /// A pane's directory as the component uses it: the reported one (or its tmux pane's)
    /// in Windows form; None when unknown.
    pub(super) fn git_directory(&self, pane: PaneId) -> Option<String> {
        let p = self.panes.get(&pane)?;
        let tmux_cwd = || p.tmux_io().map(|io| io.info.borrow().cwd.clone());
        let raw = p.own().shell.cwd.clone().or_else(tmux_cwd)?;
        let path = shown_path(&raw);
        (!path.trim().is_empty()).then_some(path)
    }

    /// The panes whose bar is in a window: bars of each window's front tab, minimized or
    /// hidden ones too (`VC:355`: `statusBarComponentIsVisible:` is the bar's view being
    /// in a window and not hidden; a miniaturized window keeps its views).
    fn panes_with_bar_in_a_window(&self) -> HashSet<PaneId> {
        let fronts = self.tabs.get(self.active).into_iter().chain(self.parked_front_tabs_all());
        fronts.flat_map(|t| self.status_bars(t)).map(|b| b.pane).collect()
    }

    /// The git pollers: one per pane whose bar has a git component (dropped when the layout
    /// loses it), each fed its session's state, gate and bumps; their requests go to the
    /// worker, and its answers come back to every poller of that path, redrawing the bars
    /// whose component changed. When to come back.
    pub(super) fn tick_git(&mut self, now: Instant) -> Option<Instant> {
        let wanted: HashMap<PaneId, f64> = self
            .panes
            .keys()
            .filter(|&&id| self.shows_status_bar(id))
            .filter_map(|&id| self.bar_layout(id).0.components.iter().find(|c| c.class == sbc::GIT_CLASS).map(|c| (id, cadence_knob(c))))
            .collect();
        self.git.retain(|id, _| wanted.contains_key(id));
        let on_screen = self.panes_with_bar_in_a_window();
        let search = self.git_search_path();
        let timeout = self.git_timeout();
        let mut changed = false;
        for (&id, &cadence) in &wanted {
            let Some(p) = self.panes.get(&id) else { continue };
            let shell = &p.own().shell;
            let last_command = shell.history.last().map(|c| c.command.clone());
            let local = is_this_machine(shell.host.as_deref(), shell.machine_id.as_deref());
            let enabled = local || gs::remote_state(&shell.vars).is_some();
            let dir = self.git_directory(id);
            let bar_shown = on_screen.contains(&id);
            let gate = PollGate { visible: bar_shown, last_newline: p.last_newline, last_activity: self.activity.get(&id).and_then(|a| a.last_output()) };
            let gp = self.git.entry(id).or_insert_with(|| GitPane {
                poller: GitPoller::new(cadence, now),
                status: None,
                command_pane: None,
                last_command: last_command.clone(),
                was_visible: false,
                menu_at: None,
                waiting: false,
                menu_state: None,
                menu_dir: None,
                picks: Vec::new(),
                send: None,
                debug: None,
                log_pending: false,
            });
            gp.poller.set_cadence(cadence);
            changed |= gp.poller.set_enabled(enabled);
            let mut polls = vec![gp.poller.set_directory(dir, &gate, now)];
            // `statusBarComponentDidMoveToWindow`: the component came onto a bar on screen.
            let placed = bar_shown && self.git_placed.contains(&id);
            if placed && !gp.was_visible {
                polls.push(gp.poller.bump(&gate, now));
            }
            gp.was_visible = placed;
            // `bumpIfLastCommandWasGit` when `lastCommand` changes.
            if last_command != gp.last_command {
                if last_command.as_deref().is_some_and(gs::looks_like_git) {
                    polls.push(gp.poller.bump(&gate, now));
                }
                gp.last_command = last_command;
            }
            polls.push(gp.poller.tick(&gate, now));
            for polls in polls {
                self.git_worker_polls(polls, local, &search, timeout);
            }
        }
        let mut menus = Vec::new();
        for reply in self.git_worker.take() {
            match reply {
                GitReply::State { path, fetched } => {
                    for gp in self.git.values_mut() {
                        let before = (gp.poller.state.clone(), gp.poller.timed_out, gp.poller.has_fetched);
                        if gp.poller.did_poll(&path, fetched.clone()) && before != (gp.poller.state.clone(), gp.poller.timed_out, gp.poller.has_fetched) {
                            changed = true;
                        }
                    }
                }
                GitReply::Workgroup { key, path, fetched, launch } => {
                    if self.workgroup_git_answer(&key, &path, fetched, launch) {
                        changed = true;
                    }
                }
                // The menu waiting for them opens now.
                GitReply::Branches { pane, branches, .. } => {
                    if let Some(gp) = self.git.get_mut(&pane).filter(|g| g.waiting) {
                        gp.waiting = false;
                        menus.push((pane, branches));
                    }
                }
                GitReply::Log { pane, text } => {
                    if let Some(gp) = self.git.get_mut(&pane).filter(|g| g.log_pending) {
                        gp.log_pending = false;
                        self.git_log_ready = Some((pane, text));
                    }
                }
            }
        }
        for (pane, branches) in menus {
            self.open_git_menu(pane, branches);
        }
        if changed {
            self.redraw();
            self.redraw_all();
        }
        self.git.values().filter_map(|gp| gp.poller.next_due()).chain(self.git_worker.next_due()).min()
    }

    /// What a pane's git component shows now (`GSM:193-245`): this computer's state from
    /// the poller, a remote host's from its `user.git*` variables.
    pub(super) fn git_content(&self, pane: PaneId, _c: &Component) -> Content {
        let shell = self.panes.get(&pane).map(|p| &p.own().shell);
        let local = shell.is_none_or(|s| is_this_machine(s.host.as_deref(), s.machine_id.as_deref()));
        let gp = self.git.get(&pane);
        let enabled = gp.is_some_and(|g| g.poller.enabled);
        let state = if local { gp.and_then(|g| g.poller.state.clone()) } else { shell.and_then(|s| gs::remote_state(&s.vars)) };
        let show_timeout = gp.is_some_and(|g| gs::show_timeout(local, g.poller.enabled, g.poller.has_fetched, g.poller.timed_out));
        git_content_of(state.as_ref(), gp.and_then(|g| g.status), enabled, show_timeout)
    }
}

/// The click menu and its commands (`GIT:299-626`).
impl App {
    /// A poller's asks: the cache to invalidate, and (on this computer) the path to poll.
    fn git_worker_polls(&mut self, polls: Polls, local: bool, search: &str, timeout: Duration) {
        if let Some(path) = polls.invalidate {
            self.git_worker.invalidate(&path);
        }
        // A remote session's directory is not on this computer: its state comes from its
        // variables.
        if let (Some(path), true) = (polls.request, local) {
            self.git_worker.request(path, search.to_string(), timeout);
        }
    }

    /// The pane's session is on this computer.
    fn git_local(&self, pane: PaneId) -> bool {
        self.panes.get(&pane).is_none_or(|p| is_this_machine(p.own().shell.host.as_deref(), p.own().shell.machine_id.as_deref()))
    }

    /// What the component shows the state of: the poller's here, the variables' remotely.
    fn git_state_of(&self, pane: PaneId) -> Option<GitState> {
        if self.git_local(pane) {
            self.git.get(&pane).and_then(|g| g.poller.state.clone())
        } else {
            self.panes.get(&pane).and_then(|p| gs::remote_state(&p.own().shell.vars))
        }
    }

    /// Polls a pane's git state now (`bump`), or after the timeout was raised clears the
    /// warning and polls (`clearTimeoutAndRetry`, `retry`).
    fn git_bump(&mut self, pane: PaneId, retry: bool) {
        let now = Instant::now();
        let local = self.git_local(pane);
        let Some(p) = self.panes.get(&pane) else { return };
        let visible = self.panes_with_bar_in_a_window().contains(&pane);
        let gate = PollGate { visible, last_newline: p.last_newline, last_activity: self.activity.get(&pane).and_then(|a| a.last_output()) };
        let Some(gp) = self.git.get_mut(&pane) else { return };
        let polls = if retry { gp.poller.clear_timeout_and_retry(&gate, now) } else { gp.poller.bump(&gate, now) };
        let (search, timeout) = (self.git_search_path(), self.git_timeout());
        self.git_worker_polls(polls, local, &search, timeout);
        self.redraw_all();
    }

    /// The git component's click (`GIT:307-405`): a running command's Cancel and Reveal;
    /// the timeout warning; Show Debug Info alone without a branch; else the menu, once the
    /// recent branches arrive (none for a remote host). A click while they are on their way
    /// is ignored.
    pub(super) fn show_git_menu(&mut self, pane: PaneId, at: MenuAt) {
        let local = self.git_local(pane);
        let state = self.git_state_of(pane);
        let dir = self.git_directory(pane);
        let panes = &self.panes;
        let Some(gp) = self.git.get_mut(&pane) else { return };
        if gp.waiting {
            return;
        }
        gp.menu_at = Some(at);
        if gp.command_pane.is_some_and(|c| panes.contains_key(&c)) {
            let action = gs::cancel_title(gp.status.unwrap_or_default());
            let items = [(format!("{MENU_ID}cancel"), action), (format!("{MENU_ID}reveal"), "Reveal".to_string())];
            self.show_git_items(pane, at, items.into_iter().map(|(id, title)| Some((id, title, true))).collect());
            return;
        }
        if gs::show_timeout(local, gp.poller.enabled, gp.poller.has_fetched, gp.poller.timed_out) {
            let current = self.git_timeout_setting();
            let proposed = gs::proposed_timeout(current);
            let text = bar_text(&gs::timeout_message(dir.as_deref(), current, proposed));
            self.ask(pane, TIMEOUT_BAR, &text, &[&gs::increase_button(proposed), "Cancel"], false);
            return;
        }
        let Some(state) = state.filter(|s| !s.branch.is_empty()) else {
            self.show_git_items(pane, at, vec![Some((format!("{MENU_ID}debug"), "Show Debug Info".to_string(), true))]);
            return;
        };
        gp.menu_state = Some(state);
        gp.menu_dir = gp.poller.directory.clone().or(dir);
        gp.waiting = true;
        match gp.menu_dir.clone().filter(|_| local) {
            Some(path) => {
                let (search, timeout) = (self.git_search_path(), self.git_timeout());
                self.git_worker.branches(pane, path, search, timeout);
            }
            None => {
                gp.waiting = false;
                self.open_git_menu(pane, Vec::new());
            }
        }
    }

    /// The menu for the state asked about, with the recent `branches` (`GIT:343-404`).
    fn open_git_menu(&mut self, pane: PaneId, branches: Vec<String>) {
        let Some(gp) = self.git.get_mut(&pane) else { return };
        let (Some(state), Some(at)) = (gp.menu_state.clone(), gp.menu_at) else { return };
        let items = gs::git_menu(&state, &branches);
        gp.picks = items.iter().flatten().map(|i| i.command.clone()).collect();
        let mut n = 0;
        let rows = items
            .into_iter()
            .map(|i| {
                i.map(|i| {
                    n += 1;
                    (format!("{MENU_ID}{}", n - 1), i.title, i.enabled)
                })
            })
            .collect();
        // The branches can arrive while another window is in front.
        if self.enter_pane_window(pane) {
            self.show_git_items(pane, at, rows);
        }
    }

    /// Shows a git menu: (id after `statusbar:`, title, enabled) rows, `None` a separator.
    /// A disabled row shows but cannot be picked.
    fn show_git_items(&mut self, pane: PaneId, at: MenuAt, rows: Vec<Option<(String, String, bool)>>) {
        use muda::{Menu, MenuItem, PredefinedMenuItem};
        let menu = Menu::new();
        let results: Vec<_> = rows
            .into_iter()
            .map(|r| match r {
                Some((id, title, enabled)) => menu.append(&MenuItem::with_id(format!("statusbar:{id}"), title.replace('&', "&&"), enabled, None)),
                None => menu.append(&PredefinedMenuItem::separator()),
            })
            .collect();
        if let Some(Err(e)) = results.into_iter().find(Result::is_err) {
            self.banners.set("menu", format!("Status bar menu: {e}"));
        }
        self.status_menu = Some(StatusMenu::for_component(pane, sbc::GIT_CLASS, Vec::new(), None));
        self.show_menu_at(menu, at);
    }

    /// A pick from a git menu. True when `what` was one.
    pub(super) fn git_menu_picked(&mut self, what: &str, pane: PaneId, event_loop: &ActiveEventLoop) -> bool {
        let Some(what) = what.strip_prefix(MENU_ID) else { return false };
        let running = self.git.get(&pane).and_then(|g| g.command_pane).filter(|c| self.panes.contains_key(c));
        match what {
            // `killSession:`: closed without confirmation.
            "cancel" => {
                if let Some(c) = running {
                    self.close_pane(c, event_loop);
                }
            }
            // `[session reveal]` brings a buried session back first.
            "reveal" => match running {
                Some(c) if self.buried.contains(c) => self.disinter(c),
                Some(c) => self.reveal_pane(c),
                None => {}
            },
            "debug" => self.show_git_debug(pane),
            n => {
                let Some(command) = n.parse::<usize>().ok().and_then(|n| self.git.get(&pane)?.picks.get(n).cloned()) else { return true };
                match command {
                    GitCommand::Debug => self.show_git_debug(pane),
                    GitCommand::Log => self.git_log_command(pane),
                    _ => self.run_git_in_window(pane, &command, event_loop),
                }
            }
        }
        true
    }

    /// Show Debug Info (`GIT:513-536`) as an info bar with OK and Copy.
    fn show_git_debug(&mut self, pane: PaneId) {
        let now = Instant::now();
        let Some(gp) = self.git.get(&pane) else { return };
        let cache = self.git_worker.cache.debug_info(gp.poller.directory.as_deref().unwrap_or_default(), now);
        let text = git_debug_text(&gp.poller, &cache, now);
        if let Some(gp) = self.git.get_mut(&pane) {
            gp.debug = Some(text.clone());
        }
        self.ask(pane, DEBUG_BAR, &bar_text(&text), &["OK", "Copy"], false);
    }

    /// Send Command? (`GIT:577-594`): a remote host gets the command typed, after asking.
    fn git_send_prompt(&mut self, pane: PaneId, args: &[String]) {
        let command = gs::remote_command(args);
        let text = format!("Looks like you're sshed somewhere. OK to send the command \u{201C}{command}\u{201D}?");
        if let Some(gp) = self.git.get_mut(&pane) {
            gp.send = Some(command);
        }
        self.ask(pane, SEND_BAR, &bar_text(&text), &["OK", "Cancel"], false);
    }

    /// Log (`GIT:472-505`): here, `git log` through the worker into the popover; on a
    /// remote host, the send prompt.
    fn git_log_command(&mut self, pane: PaneId) {
        if !self.git_local(pane) {
            return self.git_send_prompt(pane, &GitCommand::Log.args());
        }
        let Some(gp) = self.git.get(&pane) else { return };
        // One Log per pane: while one runs, another is ignored (iTerm2 stops the older run
        // and starts again; either way one popover shows the log).
        if gp.log_pending {
            return;
        }
        // A popover already open goes.
        self.git_log = None;
        let dir = gp.menu_dir.clone().or_else(|| self.git_directory(pane));
        if let Some(dir) = dir {
            let search = self.git_search_path();
            if let Some(gp) = self.git.get_mut(&pane) {
                gp.log_pending = true;
            }
            self.git_worker.log(pane, dir, search);
        }
    }

    /// A menu command in a single-use window (`runGitInWindowWithArguments:`, `GIT:596-626`):
    /// nothing while a command runs; the send prompt on a remote host; else the status
    /// shows, and a new window of the default profile runs `termurion-git.exe` in the folder
    /// the menu was opened for, buried at once for Stash, Push, Pull and Check out.
    pub(super) fn run_git_in_window(&mut self, pane: PaneId, command: &GitCommand, event_loop: &ActiveEventLoop) {
        let Some(gp) = self.git.get(&pane) else { return };
        if gp.status.is_some() {
            return;
        }
        if !self.git_local(pane) {
            return self.git_send_prompt(pane, &command.args());
        }
        let Some(status) = command.status() else { return };
        // Never in a network folder, nor with a network git.
        let dir = gp.menu_dir.clone().or_else(|| self.git_directory(pane)).filter(|d| crate::prompted::local_path_ok(d));
        let Some(dir) = dir else {
            self.banners.set("git", "git runs only in folders on this computer.");
            return self.redraw();
        };
        // A folder gone since the menu opened: the window would start somewhere else, maybe
        // in another repository.
        if !crate::localfs::is_local_dir(Path::new(&dir)) {
            self.ask(pane, GONE_BAR, &format!("The folder {dir} no longer exists, so git did not run."), &["OK"], false);
            return;
        }
        // The text check only: the wrapper checks the disk (links, the file) itself, so a
        // dead drive cannot stall this thread.
        let Some(git) = gs::git_program_named(&self.git_search_path()) else {
            self.banners.set("git", "The folder searched for git is not on this computer.");
            return self.redraw();
        };
        let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) else { return };
        if let Some(gp) = self.git.get_mut(&pane) {
            gp.status = Some(status);
        }
        self.redraw_all();
        let new = self.open_single_use_window(event_loop, single_use_command(&exe_dir, &git, command), Some(dir));
        let Some(new) = new else {
            if let Some(gp) = self.git.get_mut(&pane) {
                gp.status = None;
            }
            return self.redraw_all();
        };
        self.single_use.insert(new, git_single_use(pane, true));
        if let Some(gp) = self.git.get_mut(&pane) {
            gp.command_pane = Some(new);
        }
        if command.buried() {
            self.bury_pane(new);
        }
        self.redraw_all();
    }

    /// A new window of the default profile that runs `command` instead of the profile's shell,
    /// in `dir` when one is given (a folder that is gone refuses the start, `start_pane_as`).
    /// The window's first pane, or None when the window could not be made. The caller
    /// registers the pane in `single_use` (it alone knows whether the pane is short-lived and
    /// who owns it); the overrides are set only around the window's creation, so no later
    /// pane inherits them and the profile's Initial Text never types over the command
    /// (`start_command_override` is what blanks it).
    pub(super) fn open_single_use_window(&mut self, event_loop: &ActiveEventLoop, command: String, dir: Option<String>) -> Option<PaneId> {
        self.start_command_override = Some(command);
        self.start_dir_override = dir.map(crate::arrangement::Folder::Forced);
        let made = self.open_window(event_loop, None);
        self.start_command_override = None;
        self.start_dir_override = None;
        made.ok().and_then(|_| self.tabs.get(self.active).map(|t| t.focused))
    }

    /// A git command's session ended (its completion, `GIT:628-631`): its owner's status clears
    /// and the owner polls. The window stays (R3), so its single-use record stays too: it is
    /// still a git window that shows "Finished" and never restarts. Once per session.
    pub(super) fn git_command_ended(&mut self, id: PaneId) {
        let Some(su) = self.single_use.get(&id).copied() else { return };
        if let Some(gp) = self.git.get_mut(&su.owner).filter(|g| g.command_pane == Some(id)) {
            gp.status = None;
            gp.command_pane = None;
        }
        self.git_bump(su.owner, false);
    }

    /// A single-use session closed: its command ends if it had not (the exit already did, once),
    /// and its record goes.
    pub(super) fn single_use_ended(&mut self, id: PaneId) {
        let pending = self.single_use.get(&id).is_some_and(|su| self.git.get(&su.owner).is_some_and(|g| g.command_pane == Some(id)));
        if pending {
            self.git_command_ended(id);
        }
        self.single_use.remove(&id);
    }

    /// A single-use session that must not leave an alert or an Undo Close.
    pub(super) fn short_lived(&self, id: PaneId) -> bool {
        self.single_use.get(&id).is_some_and(|s| s.short_lived)
    }

    /// The git info bars' answers. False for other bars.
    pub(super) fn git_bar(&mut self, pane: PaneId, id: &str, button: Option<&str>) -> bool {
        if ![TIMEOUT_BAR, SEND_BAR, DEBUG_BAR, GONE_BAR].contains(&id) {
            return false;
        }
        let answer = button.map_or(GitBarAnswer::Nothing, |b| git_bar_answer(id, b));
        // The bar's own pending text only: another git bar may still be waiting.
        let gp = self.git.get_mut(&pane);
        let (send, debug) = match (gp, id) {
            (Some(g), SEND_BAR) => (g.send.take(), None),
            (Some(g), DEBUG_BAR) => (None, g.debug.take()),
            _ => (None, None),
        };
        match answer {
            GitBarAnswer::IncreaseTimeout(t) => {
                self.globals.set(TIMEOUT_KEY, toml::Value::Float(t));
                self.save_globals();
                self.git_bump(pane, true);
            }
            // `writeTask:` with iTerm2's newline.
            GitBarAnswer::Send => {
                if let Some(command) = send {
                    self.user_write(pane, format!("{command}\n").as_bytes(), Write::SendText);
                }
            }
            GitBarAnswer::Copy => {
                if let Some(text) = debug {
                    self.copy_status_text(text);
                }
            }
            GitBarAnswer::Nothing => {}
        }
        true
    }

    /// Opens the Log popover whose text arrived, at the component it was asked from.
    pub(super) fn open_git_log(&mut self, event_loop: &ActiveEventLoop) {
        use crate::git_log_ui::{GitLogWindow, LogAt, LogFont};
        let Some((pane, text)) = self.git_log_ready.take() else { return };
        let Some(at) = self.git.get(&pane).and_then(|g| g.menu_at) else { return };
        if !self.enter_pane_window(pane) {
            return;
        }
        let dark = self.panes.get(&pane).is_some_and(|p| p.screen.is_dark());
        let Some(g) = self.gpu.as_ref() else { return };
        let origin = g.window.inner_position().unwrap_or_default();
        let place = LogAt { x: origin.x as f64 + at.x as f64, y: origin.y as f64 + at.y as f64, above: at.above };
        let font = LogFont { cell_w: g.renderer.cell.0 as f64, px: g.renderer.font_px(), scale: g.window.scale_factor(), dark };
        match GitLogWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, text, place, font) {
            Ok(w) => self.git_log = Some(w),
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// Handles an event for the Log popover. False when `window` is not it.
    pub(super) fn git_log_event(&mut self, window: winit::window::WindowId, event: &WindowEvent) -> bool {
        let Some(w) = self.git_log.as_mut().filter(|w| w.id() == window) else { return false };
        if !w.event(event) {
            self.git_log = None;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_state::RunResult;
    use std::sync::{Condvar, Mutex};

    /// A git that answers a clean `main` once released, and blocks until then; with
    /// `only`, it blocks only in folders whose name ends so (a dead drive).
    struct Gate {
        open: Mutex<bool>,
        cv: Condvar,
        only: Option<&'static str>,
    }

    impl GitRun for Gate {
        fn run(&self, _git: &Path, dir: &Path, args: &[String], _timeout: Duration, _cap: Option<usize>) -> RunResult {
            let mut open = self.open.lock().unwrap();
            let blocks = self.only.is_none_or(|o| dir.to_string_lossy().ends_with(o));
            while blocks && !*open {
                open = self.cv.wait(open).unwrap();
            }
            let out: &[u8] = if args.iter().any(|a| a == "status") { b"# branch.oid 0123\0# branch.head main\0" } else { b"" };
            RunResult::Exit { code: if out.is_empty() { 1 } else { 0 }, stdout: out.to_vec(), capped: false }
        }
    }

    #[test]
    fn git_windows_take_no_action_at_end() {
        use crate::session_end::{after_exit, EndAction, Exit, Message, Outcome};
        for short in [true, false] {
            assert_eq!(git_single_use(7, short).end_action, EndAction::NoAction);
        }
        let exit = Exit { action: git_single_use(7, true).end_action, restartable: true, short_lived_single_use: true };
        assert_eq!(after_exit(exit, false), (Message::Finished, Outcome::Finished { close: false }));
    }

    fn repos(name: &str, n: usize) -> Vec<String> {
        let root = std::env::temp_dir().join(format!("termurion-git-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        (0..n)
            .map(|i| {
                let d = root.join(format!("r{i}"));
                std::fs::create_dir_all(d.join(".git")).unwrap();
                d.to_string_lossy().into_owned()
            })
            .collect()
    }

    /// The single-use window runs the wrapper beside Termurion with the git and its arguments
    /// (a `-` branch as its ref); the info bars' buttons read as iTerm2's; Debug Info reads
    /// as `GIT:513-536`.
    #[test]
    fn single_use_windows_follow_iterm2() {
        let push = GitCommand::Push("main".into());
        assert_eq!(single_use_command(Path::new("C:\\Program Files\\Termurion"), Path::new("git"), &push), "\"C:\\Program Files\\Termurion\\termurion-git.exe\" git push origin main");
        assert_eq!(
            single_use_command(Path::new("C:\\T"), Path::new("C:\\Git\\cmd\\git.exe"), &GitCommand::Checkout("-x".into())),
            "C:\\T\\termurion-git.exe C:\\Git\\cmd\\git.exe checkout refs/heads/-x"
        );
        assert_eq!(single_use_command(Path::new("C:\\T"), Path::new("git"), &GitCommand::AddCommit), "C:\\T\\termurion-git.exe git commit -a");

        assert_eq!(git_bar_answer(TIMEOUT_BAR, "Increase to 8s"), GitBarAnswer::IncreaseTimeout(8.0));
        assert_eq!(git_bar_answer(TIMEOUT_BAR, &gs::increase_button(gs::proposed_timeout(1.3))), GitBarAnswer::IncreaseTimeout(3.3));
        assert_eq!(git_bar_answer(TIMEOUT_BAR, "Cancel"), GitBarAnswer::Nothing);
        assert_eq!(git_bar_answer(SEND_BAR, "OK"), GitBarAnswer::Send);
        assert_eq!(git_bar_answer(SEND_BAR, "Cancel"), GitBarAnswer::Nothing);
        assert_eq!(git_bar_answer(DEBUG_BAR, "Copy"), GitBarAnswer::Copy);
        assert_eq!(git_bar_answer(DEBUG_BAR, "OK"), GitBarAnswer::Nothing);
        assert_eq!(git_bar_answer("statusbar:error", "OK"), GitBarAnswer::Nothing);
        assert_eq!((TIMEOUT_BAR, SEND_BAR, DEBUG_BAR), ("statusbar:git-timeout", "statusbar:git-send", "statusbar:git-debug"));

        let now = Instant::now();
        let mut poller = GitPoller::new(2.0, now);
        poller.set_enabled(true);
        poller.directory = Some("C:\\r".into());
        poller.last_poll = Some(now - Duration::from_millis(1500));
        poller.state = Some(GitState { branch: "main".into(), ahead: "1".into(), dirty: true, adds: 2, ..Default::default() });
        let cache = WorkerCache::default();
        let text = git_debug_text(&poller, &cache.debug_info("C:\\r", now), now);
        assert_eq!(
            text,
            "Directory: C:\\r\nPolling cadence: 2 sec\nPolling enabled: Yes\nLast polled 1.5 seconds ago\n\
Repo state: dir=(null) xcode=(null) ahead=1 behind= branch=main dirty=1 adds=2 deletes=0 lines=+0/-0 files=+0/~0/-0\n\
Basic cache: none\nRich cache: none\nPending calls: 0\n"
        );
        assert_eq!(bar_text("a\nb\n"), "a b ", "an info bar shows newlines as spaces");
        assert_eq!(git_debug_text(&GitPoller::new(0.0, now), "", now), "Directory: (null)\nPolling cadence: 1 sec\nPolling enabled: No\nLast polled 0 seconds ago\nRepo state: (null)\n");
    }

    fn states(replies: &[GitReply]) -> Vec<(String, Fetched)> {
        replies.iter().filter_map(|r| if let GitReply::State { path, fetched } = r { Some((path.clone(), fetched.clone())) } else { None }).collect()
    }

    /// Requests for one path join the one thread; at most `MAX_THREADS` run, the rest
    /// queue; a thread stuck past its deadline is reported as a timeout by the UI side and
    /// its late answer is dropped; it counts against the cap until it ends. After that a
    /// run answers, and a second request within the TTL comes from the cache.
    #[test]
    fn worker_bounds_threads_and_abandons_stuck_ones() {
        let gate = Arc::new(Gate { open: Mutex::new(false), cv: Condvar::new(), only: None });
        let mut w = GitWorker::new(gate.clone(), Arc::new(|| {}));
        let paths = repos("worker", MAX_THREADS + 2);
        let git = String::new();
        let timeout = Duration::from_secs(1);
        w.request(paths[0].clone(), git.clone(), timeout);
        w.request(paths[0].clone(), git.clone(), timeout);
        assert_eq!(w.live.load(Ordering::Acquire), 1, "same-path requests join");
        for p in &paths[1..] {
            w.request(p.clone(), git.clone(), timeout);
        }
        assert_eq!(w.live.load(Ordering::Acquire), MAX_THREADS);
        assert_eq!(w.queue.len(), 2);
        assert!(w.take().is_empty(), "nothing is due yet");

        std::thread::sleep(timeout + GRACE + Duration::from_millis(100));
        let late = states(&w.take());
        assert_eq!(late.len(), paths.len(), "{late:?}");
        assert!(late.iter().all(|(_, f)| f.timed_out && f.state.is_none()));
        assert_eq!(w.live.load(Ordering::Acquire), MAX_THREADS, "stuck threads still count");
        assert!(w.flights.is_empty() && w.queue.is_empty());

        *gate.open.lock().unwrap() = true;
        gate.cv.notify_all();
        let end = Instant::now() + Duration::from_secs(5);
        while w.live.load(Ordering::Acquire) > 0 && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(w.live.load(Ordering::Acquire), 0);
        assert!(w.take().is_empty(), "late answers are dropped");

        w.request(paths[0].clone(), git.clone(), timeout);
        let end = Instant::now() + Duration::from_secs(5);
        let mut got = Vec::new();
        while got.is_empty() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10));
            got = states(&w.take());
        }
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].1.state.as_ref().map(|s| s.branch.as_str()), Some("main"));
        w.request(paths[0].clone(), git, timeout);
        assert_eq!(w.live.load(Ordering::Acquire), 0, "the cache answers without a thread");
        assert_eq!(states(&w.take()).len(), 1);
    }

    /// A path whose thread is stuck past its deadline keeps that one thread: requests for
    /// it answer "timed out" at once without another, healthy paths still get threads, and
    /// once the stuck thread ends the path runs again.
    #[test]
    fn a_stuck_path_never_takes_a_second_thread() {
        let gate = Arc::new(Gate { open: Mutex::new(false), cv: Condvar::new(), only: Some("r0") });
        let mut w = GitWorker::new(gate.clone(), Arc::new(|| {}));
        let paths = repos("stuck", 2);
        let timeout = Duration::from_secs(1);
        w.request(paths[0].clone(), String::new(), timeout);
        std::thread::sleep(timeout + GRACE + Duration::from_millis(100));
        assert_eq!(states(&w.take()).iter().map(|(_, f)| f.timed_out).collect::<Vec<_>>(), vec![true]);
        for _ in 0..3 {
            w.invalidate(&paths[0]);
            w.request(paths[0].clone(), String::new(), timeout);
            let got = states(&w.take());
            assert_eq!(got.len(), 1);
            assert!(got[0].1.timed_out, "{got:?}");
        }
        assert_eq!(w.live.load(Ordering::Acquire), 1, "one thread for the stuck path");

        w.request(paths[1].clone(), String::new(), timeout);
        assert_eq!(w.live.load(Ordering::Acquire), 2, "a healthy path gets its own thread");
        let end = Instant::now() + Duration::from_secs(5);
        let mut got = Vec::new();
        while got.is_empty() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10));
            got = states(&w.take());
        }
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].0.as_str(), got[0].1.state.as_ref().map(|s| s.branch.as_str())), (paths[1].as_str(), Some("main")));

        *gate.open.lock().unwrap() = true;
        gate.cv.notify_all();
        let end = Instant::now() + Duration::from_secs(5);
        while w.live.load(Ordering::Acquire) > 0 && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(w.take().is_empty(), "the stuck thread's late answer is dropped");
        w.invalidate(&paths[0]);
        w.request(paths[0].clone(), String::new(), timeout);
        assert_eq!(w.live.load(Ordering::Acquire), 1, "once it ended, the path runs again");
    }
}

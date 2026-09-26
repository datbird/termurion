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

use super::statusbar_content::{Content, MenuAt, is_localhost, shown_path};
use super::statusbar_ui::Click;
use super::*;
use crate::git_state::{self as gs, Fetched, GitPoller, GitRun, GitState, Lookup, PollGate, WorkerCache};
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
const GRACE: Duration = Duration::from_millis(200);
const TIMEOUT_KEY: &str = "gitTimeout";
const SEARCH_PATH_KEY: &str = "gitSearchPath";
/// `gitTimeout`'s default (`ADV:534`).
const DEFAULT_TIMEOUT: f64 = 4.0;

/// One pane's git component state (the first git component of its bar).
pub(super) struct GitPane {
    pub(super) poller: GitPoller,
    /// A menu command's running text (`"Pushing\u{2026}"`), while its window runs.
    pub(super) status: Option<&'static str>,
    /// The single-use window's pane running that command (Task 6).
    #[allow(dead_code)]
    pub(super) command_pane: Option<PaneId>,
    /// `lastCommand` when last looked at.
    pub(super) last_command: Option<String>,
    /// The component was placed on a bar on screen when last looked at.
    pub(super) was_visible: bool,
    /// Where the menu opens once the recent branches arrive (Task 6).
    #[allow(dead_code)]
    pub(super) menu_at: Option<MenuAt>,
}

/// An answer from the worker.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum GitReply {
    /// A poll of `path`.
    State { path: String, fetched: Fetched },
    /// The recent branches a pane's menu asked for.
    Branches { pane: PaneId, path: String, branches: Vec<String> },
}

/// What a thread runs; also the key that joins requests.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Job {
    State(String),
    Branches(PaneId, String),
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
    fetched: Fetched,
    branches: Vec<String>,
}

/// What threads send: an answer, then (however the thread ends) that it ended.
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

    /// The 8 most recently committed branches in `path`, for `pane`'s menu (Task 6).
    #[allow(dead_code)]
    pub(super) fn branches(&mut self, pane: PaneId, path: String, search: String, timeout: Duration) {
        self.start(Job::Branches(pane, path), search, timeout, Instant::now());
    }

    /// Forgets `path`'s cached state and waiting requests (`GPW:161-199`).
    pub(super) fn invalidate(&mut self, path: &str) {
        self.cache.invalidate(path);
    }

    fn start(&mut self, job: Job, search: String, timeout: Duration, now: Instant) {
        if self.abandoned.contains_key(&job) {
            // Its thread is still stuck (a dead drive): a timeout, and no second thread.
            let reply = self.answer(&job, Fetched { state: None, timed_out: true }, Vec::new(), now);
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
        let spawned = std::thread::Builder::new().name("tandem-git".into()).spawn(move || {
            let _alive = alive;
            let git = gs::git_program(&search);
            let (fetched, branches) = match (&job, git) {
                (Job::State(path), Some(git)) => (gs::fetch_state(&*runner, &git, Path::new(path), left), Vec::new()),
                (Job::Branches(_, path), Some(git)) => (Fetched::default(), gs::recent_branches(&*runner, &git, Path::new(path), left)),
                // No git to run (a network or linked `gitSearchPath`): no state.
                (_, None) => (Fetched::default(), Vec::new()),
            };
            let _ = tx.send(Msg::Done(Done { id, job, finished: Instant::now(), fetched, branches }));
        });
        match spawned {
            Ok(_) => {
                self.flights.insert(q.job, Flight { id, due: now + left + GRACE, again: None });
            }
            // The closure (and its `Alive`) is dropped: the count is back. No thread means
            // no state, as for a folder git may not run in.
            Err(_) => {
                let reply = self.answer(&q.job, Fetched::default(), Vec::new(), now);
                self.ready.push(reply);
            }
        }
    }

    /// The reply for a finished (or abandoned) job, through the cache for a poll.
    fn answer(&mut self, job: &Job, fetched: Fetched, branches: Vec<String>, finished: Instant) -> GitReply {
        match job {
            Job::State(path) => GitReply::State { path: path.clone(), fetched: self.cache.finish(path, fetched, finished) },
            Job::Branches(pane, path) => GitReply::Branches { pane: *pane, path: path.clone(), branches },
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
            out.push(self.answer(&d.job, d.fetched, d.branches, d.finished));
            again.extend(flight.again.map(|(search, timeout)| (d.job, search, timeout)));
        }
        let late: Vec<Job> = self.flights.iter().filter(|(_, f)| now >= f.due).map(|(j, _)| j.clone()).collect();
        for job in late {
            let flight = self.flights.remove(&job).expect("listed above");
            self.abandoned.insert(job.clone(), flight.id);
            out.push(self.answer(&job, Fetched { state: None, timed_out: true }, Vec::new(), now));
            again.extend(flight.again.map(|(search, timeout)| (job, search, timeout)));
        }
        let (waited, queued): (Vec<Queued>, Vec<Queued>) = self.queue.drain(..).partition(|q| now >= q.asked + q.timeout);
        self.queue = queued.into();
        for q in waited {
            out.push(self.answer(&q.job, Fetched { state: None, timed_out: true }, Vec::new(), now));
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
            let local = is_localhost(shell.host.as_deref());
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
                if let Some(path) = polls.invalidate {
                    self.git_worker.invalidate(&path);
                }
                // A remote session's directory is not on this computer: its state comes
                // from its variables.
                if let (Some(path), true) = (polls.request, local) {
                    self.git_worker.request(path, search.clone(), timeout);
                }
            }
        }
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
                // The menu (Task 6) opens with these.
                GitReply::Branches { .. } => {}
            }
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
        let local = is_localhost(shell.and_then(|s| s.host.as_deref()));
        let gp = self.git.get(&pane);
        let enabled = gp.is_some_and(|g| g.poller.enabled);
        let state = if local { gp.and_then(|g| g.poller.state.clone()) } else { shell.and_then(|s| gs::remote_state(&s.vars)) };
        let show_timeout = gp.is_some_and(|g| gs::show_timeout(local, g.poller.enabled, g.poller.has_fetched, g.poller.timed_out));
        git_content_of(state.as_ref(), gp.and_then(|g| g.status), enabled, show_timeout)
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

    fn repos(name: &str, n: usize) -> Vec<String> {
        let root = std::env::temp_dir().join(format!("tandem-git-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        (0..n)
            .map(|i| {
                let d = root.join(format!("r{i}"));
                std::fs::create_dir_all(d.join(".git")).unwrap();
                d.to_string_lossy().into_owned()
            })
            .collect()
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

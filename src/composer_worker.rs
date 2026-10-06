//! The Composer's worker (milestone 9l Task 6): the file and PATH listings behind its
//! suggestions and the command and file checks behind its highlighting, run off the UI
//! thread (as the git poller runs git). The filesystem and the environment are injected
//! (`Fs`), so the jobs are tested without touching the disk; `RealFs` is the real one.
//!
//! Nothing here runs a program, and nothing UNC-looking (`\\host`, `//host`, `\\?\UNC\...`)
//! is ever listed or probed (project invariant 5: probing one leaks the user's Windows
//! credentials to whatever `host` is).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::command_syntax::{self, Request};
use crate::layout::PaneId;
use crate::prompted::network_path;

/// Most entries read from one directory for a listing (a huge folder or a long PATH never
/// stalls the worker for long, and the popup shows at most 256 anyway).
pub const MAX_DIR_ENTRIES: usize = 4096;
/// How long a PATH directory's listing is reused before it is read again.
pub const PATH_TTL: Duration = Duration::from_secs(30);

/// What the jobs read of the machine.
pub trait Fs: Send + Sync {
    /// A directory's entries (name, is a directory), in any order.
    fn list(&self, dir: &Path) -> Vec<(String, bool)>;
    fn is_file(&self, path: &Path) -> bool;
    fn exists(&self, path: &Path) -> bool;
    /// An environment variable (PATH, PATHEXT, USERPROFILE), empty when unset.
    fn env(&self, name: &str) -> String;
}

/// The real filesystem and environment.
pub struct RealFs;

impl Fs for RealFs {
    fn list(&self, dir: &Path) -> Vec<(String, bool)> {
        let Some(Ok(entries)) = crate::localfs::LocalPath::judge(dir).map(|d| crate::localfs::entries_upto(&d, MAX_DIR_ENTRIES)) else { return Vec::new() };
        entries.into_iter().map(|e| (e.name.to_string_lossy().into_owned(), e.stat.is_dir)).collect()
    }

    fn is_file(&self, path: &Path) -> bool {
        crate::localfs::is_local_file(path)
    }

    fn exists(&self, path: &Path) -> bool {
        crate::localfs::local_exists(path)
    }

    fn env(&self, name: &str) -> String {
        std::env::var(name).unwrap_or_default()
    }
}

/// A job for the worker.
#[derive(Clone, Debug, PartialEq)]
pub enum Job {
    /// List `dirs` (each with whether it is a PATH directory, whose executables are wanted)
    /// for `pane`'s suggestion fetch `generation`.
    Files { pane: PaneId, generation: u64, dirs: Vec<(PathBuf, bool)> },
    /// One checker request for `pane`, a file argument relative to `cwd`.
    Check { pane: PaneId, request: Request, cwd: Option<PathBuf> },
}

/// A job's answer.
#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    Files { pane: PaneId, generation: u64, entries: Vec<(String, bool)> },
    /// None: the worker failed on it (it will never be answered; the caller calls
    /// `Checker::lost`).
    Check { pane: PaneId, request: Request, ok: Option<bool> },
}

/// Listings of PATH directories, with when each was read.
pub type Cache = HashMap<PathBuf, (Instant, Vec<(String, bool)>)>;

fn env_list(fs: &dyn Fs, name: &str) -> Vec<String> {
    fs.env(name).split(';').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

/// A directory's entries, at most `MAX_DIR_ENTRIES`; nothing for a UNC path.
fn list_dir(fs: &dyn Fs, dir: &Path) -> Vec<(String, bool)> {
    if network_path(&dir.to_string_lossy()) {
        return Vec::new();
    }
    let mut v = fs.list(dir);
    v.truncate(MAX_DIR_ENTRIES);
    v
}

/// The reply for a job the worker could not finish.
pub fn failed(job: &Job) -> Reply {
    match job {
        Job::Files { pane, generation, .. } => Reply::Files { pane: *pane, generation: *generation, entries: Vec::new() },
        Job::Check { pane, request, .. } => Reply::Check { pane: *pane, request: request.clone(), ok: None },
    }
}

/// One job. Listings: a PATH directory gives its executables (PATHEXT files, the extension
/// dropped) from `cache` while younger than `PATH_TTL`; any other directory everything; names
/// deduplicated ignoring case, first wins. Checks: a command is valid when a PATH/PATHEXT
/// candidate is a file (`FC:195-242`), a file argument when it exists (`~` is the home folder,
/// a relative one is under `cwd`, none without it).
pub fn run_job(job: &Job, cache: &mut Cache, fs: &dyn Fs, now: Instant) -> Reply {
    match job {
        Job::Files { pane, generation, dirs } => {
            let pathext = env_list(fs, "PATHEXT");
            let mut entries: Vec<(String, bool)> = Vec::new();
            let mut seen = HashSet::new();
            for (dir, on_path) in dirs {
                let listed = if *on_path {
                    match cache.get(dir).filter(|(t, _)| now.saturating_duration_since(*t) < PATH_TTL) {
                        Some((_, v)) => v.clone(),
                        None => {
                            let v: Vec<(String, bool)> = list_dir(fs, dir)
                                .into_iter()
                                .filter(|(_, is_dir)| !is_dir)
                                .filter_map(|(name, _)| {
                                    let lower = name.to_lowercase();
                                    let ext = pathext.iter().find(|e| lower.ends_with(&e.to_lowercase()))?;
                                    let cut = name.len().checked_sub(ext.len()).filter(|&c| name.is_char_boundary(c))?;
                                    Some((name[..cut].to_string(), false))
                                })
                                .collect();
                            cache.insert(dir.clone(), (now, v.clone()));
                            v
                        }
                    }
                } else {
                    list_dir(fs, dir)
                };
                for (name, is_dir) in listed {
                    if !name.is_empty() && seen.insert(name.to_lowercase()) {
                        entries.push((name, is_dir));
                    }
                }
            }
            Reply::Files { pane: *pane, generation: *generation, entries }
        }
        Job::Check { pane, request, cwd } => {
            let home = fs.env("USERPROFILE");
            let ok = match request {
                Request::Command(cmd) => {
                    let candidates = command_syntax::command_candidates(cmd, &env_list(fs, "PATH"), &env_list(fs, "PATHEXT"), &home);
                    candidates.iter().filter(|c| !network_path(c)).any(|c| fs.is_file(Path::new(c)))
                }
                Request::File(arg) => {
                    let p = if arg == "~" {
                        Some(PathBuf::from(&home))
                    } else if let Some(rest) = arg.strip_prefix("~/").or_else(|| arg.strip_prefix("~\\")) {
                        Some(Path::new(&home).join(rest))
                    } else if Path::new(arg).is_absolute() || arg.starts_with('\\') || arg.starts_with('/') || arg.as_bytes().get(1) == Some(&b':') {
                        Some(PathBuf::from(arg))
                    } else {
                        cwd.as_ref().map(|d| d.join(arg))
                    };
                    p.is_some_and(|p| !network_path(&p.to_string_lossy()) && fs.exists(&p))
                }
            };
            Reply::Check { pane: *pane, request: request.clone(), ok: Some(ok) }
        }
    }
}

/// The worker thread: started on first use, started again on the next job when it has
/// ended. Every job gets exactly one reply (a panicking one gets `failed`'s), after which
/// `notify` wakes the caller.
pub struct Worker {
    notify: Arc<dyn Fn() + Send + Sync>,
    fs: Arc<dyn Fs>,
    link: Option<(mpsc::Sender<Job>, mpsc::Receiver<Reply>)>,
}

impl Worker {
    pub fn new(notify: Arc<dyn Fn() + Send + Sync>, fs: Arc<dyn Fs>) -> Worker {
        Worker { notify, fs, link: None }
    }

    /// Hands a job to the thread; the job back when no thread takes it.
    pub fn send(&mut self, job: Job) -> Result<(), Job> {
        if self.link.is_none() {
            let (jtx, jrx) = mpsc::channel::<Job>();
            let (rtx, rrx) = mpsc::channel::<Reply>();
            let notify = self.notify.clone();
            let fs = self.fs.clone();
            let started = std::thread::Builder::new().name("composer".into()).spawn(move || {
                let mut cache = Cache::new();
                for job in jrx {
                    let reply = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_job(&job, &mut cache, fs.as_ref(), Instant::now()))).unwrap_or_else(|_| failed(&job));
                    if rtx.send(reply).is_err() {
                        break;
                    }
                    notify();
                }
            });
            if started.is_err() {
                return Err(job);
            }
            self.link = Some((jtx, rrx));
        }
        let Some((tx, _)) = &self.link else { return Err(job) };
        tx.send(job).map_err(|e| {
            self.link = None;
            e.0
        })
    }

    /// The replies that arrived since the last call.
    pub fn replies(&mut self) -> Vec<Reply> {
        let Some((_, rx)) = &self.link else { return Vec::new() };
        let mut out = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(r) => out.push(r),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.link = None;
                    break;
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A filesystem in memory: directories with entries, the files that exist, and the
    /// environment; counts listings, and records every path asked about.
    #[derive(Default)]
    struct FakeFs {
        dirs: HashMap<PathBuf, Vec<(String, bool)>>,
        files: HashSet<PathBuf>,
        env: HashMap<String, String>,
        lists: AtomicUsize,
        asked: Mutex<Vec<String>>,
        panic: bool,
    }

    impl Fs for FakeFs {
        fn list(&self, dir: &Path) -> Vec<(String, bool)> {
            assert!(!self.panic, "worker panic test");
            self.lists.fetch_add(1, Ordering::SeqCst);
            self.asked.lock().unwrap().push(dir.to_string_lossy().into_owned());
            self.dirs.get(dir).cloned().unwrap_or_default()
        }
        fn is_file(&self, path: &Path) -> bool {
            self.asked.lock().unwrap().push(path.to_string_lossy().into_owned());
            self.has(path)
        }
        fn exists(&self, path: &Path) -> bool {
            self.asked.lock().unwrap().push(path.to_string_lossy().into_owned());
            self.has(path) || self.dirs.contains_key(path)
        }
        fn env(&self, name: &str) -> String {
            self.env.get(name).cloned().unwrap_or_default()
        }
    }

    impl FakeFs {
        /// Windows file names ignore case.
        fn has(&self, path: &Path) -> bool {
            let want = path.to_string_lossy().to_lowercase();
            self.files.iter().any(|f| f.to_string_lossy().to_lowercase() == want)
        }
    }

    fn fake() -> FakeFs {
        let mut f = FakeFs::default();
        f.env.insert("PATHEXT".into(), ".EXE;.CMD".into());
        f.env.insert("PATH".into(), "C:\\bin;\\\\server\\share\\bin".into());
        f.env.insert("USERPROFILE".into(), "C:\\Users\\me".into());
        f.dirs.insert(PathBuf::from("C:\\bin"), vec![("git.exe".into(), false), ("Git.cmd".into(), false), ("notes.txt".into(), false), ("tools".into(), true)]);
        f.dirs.insert(PathBuf::from("C:\\work"), vec![("src".into(), true), ("README.md".into(), false), ("readme.md".into(), false)]);
        f.files.insert(PathBuf::from("C:\\bin\\git.exe"));
        f.files.insert(PathBuf::from("C:\\work\\README.md"));
        f
    }

    #[test]
    fn listings_follow_the_rules() {
        let fs = fake();
        let t0 = Instant::now();
        let mut cache = Cache::new();
        // A PATH directory: executables only, extension dropped, deduplicated ignoring case;
        // a UNC directory is never listed.
        let job = Job::Files { pane: 1, generation: 7, dirs: vec![(PathBuf::from("C:\\bin"), true), (PathBuf::from("\\\\server\\share\\bin"), true)] };
        assert_eq!(run_job(&job, &mut cache, &fs, t0), Reply::Files { pane: 1, generation: 7, entries: vec![("git".into(), false)] });
        assert!(fs.asked.lock().unwrap().iter().all(|p| !p.starts_with("\\\\")), "no UNC path touched");
        assert_eq!(fs.lists.load(Ordering::SeqCst), 1);
        // Reused from the cache while fresh, read again after PATH_TTL.
        run_job(&job, &mut cache, &fs, t0 + Duration::from_secs(1));
        assert_eq!(fs.lists.load(Ordering::SeqCst), 1);
        run_job(&job, &mut cache, &fs, t0 + PATH_TTL + Duration::from_secs(1));
        assert_eq!(fs.lists.load(Ordering::SeqCst), 2);
        // Any other directory: everything, first spelling wins.
        let job = Job::Files { pane: 1, generation: 8, dirs: vec![(PathBuf::from("C:\\work"), false)] };
        assert_eq!(
            run_job(&job, &mut cache, &fs, t0),
            Reply::Files { pane: 1, generation: 8, entries: vec![("src".into(), true), ("README.md".into(), false)] }
        );
    }

    #[test]
    fn listings_are_capped() {
        let mut fs = fake();
        let many: Vec<(String, bool)> = (0..MAX_DIR_ENTRIES + 50).map(|i| (format!("f{i}"), false)).collect();
        fs.dirs.insert(PathBuf::from("C:\\big"), many);
        let job = Job::Files { pane: 2, generation: 1, dirs: vec![(PathBuf::from("C:\\big"), false)] };
        let Reply::Files { entries, .. } = run_job(&job, &mut Cache::new(), &fs, Instant::now()) else { panic!() };
        assert_eq!(entries.len(), MAX_DIR_ENTRIES);
    }

    #[test]
    fn checks_follow_the_rules() {
        let fs = fake();
        let now = Instant::now();
        let check = |request: Request, cwd: Option<&str>| {
            let job = Job::Check { pane: 3, request: request.clone(), cwd: cwd.map(PathBuf::from) };
            match run_job(&job, &mut Cache::new(), &fs, now) {
                Reply::Check { ok, request: r, pane: 3 } if r == request => ok,
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(check(Request::Command("git".into()), None), Some(true));
        assert_eq!(check(Request::Command("nope".into()), None), Some(false));
        assert_eq!(check(Request::File("README.md".into()), Some("C:\\work")), Some(true));
        assert_eq!(check(Request::File("README.md".into()), None), Some(false), "relative with no directory");
        assert_eq!(check(Request::File("C:\\bin\\git.exe".into()), None), Some(true));
        assert_eq!(check(Request::File("x".into()), Some("\\\\server\\share")), Some(false), "a UNC session directory");
        assert!(fs.asked.lock().unwrap().iter().all(|p| !p.starts_with("\\\\")), "no UNC path touched");
        assert_eq!(failed(&Job::Check { pane: 3, request: Request::File("a".into()), cwd: None }), Reply::Check { pane: 3, request: Request::File("a".into()), ok: None });
    }

    #[test]
    fn the_worker_answers_every_job_once() {
        let woke = Arc::new(AtomicUsize::new(0));
        let w2 = woke.clone();
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            w2.fetch_add(1, Ordering::SeqCst);
        });
        let mut w = Worker::new(notify.clone(), Arc::new(fake()));
        let check = Job::Check { pane: 4, request: Request::Command("git".into()), cwd: None };
        assert!(w.send(check).is_ok());
        let mut got = Vec::new();
        for _ in 0..500 {
            got.extend(w.replies());
            if !got.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(got, vec![Reply::Check { pane: 4, request: Request::Command("git".into()), ok: Some(true) }]);
        assert_eq!(woke.load(Ordering::SeqCst), 1);
        // A job that panics still gets its one (failed) reply.
        let mut fs = fake();
        fs.panic = true;
        let mut w = Worker::new(notify, Arc::new(fs));
        assert!(w.send(Job::Files { pane: 5, generation: 2, dirs: vec![(PathBuf::from("C:\\work"), false)] }).is_ok());
        let mut got = Vec::new();
        for _ in 0..500 {
            got.extend(w.replies());
            if !got.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(got, vec![Reply::Files { pane: 5, generation: 2, entries: Vec::new() }]);
    }
}

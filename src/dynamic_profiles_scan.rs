//! Watches the Dynamic Profiles folder on a worker thread (`DP:252-265`): a reload request at
//! once, then one `latency` after the last change a subtree change notification saw (iTerm2's
//! coalescing latency). The worker sleeps in the kernel while nothing changes: it wakes the app
//! only when a reload is due, never on a timer (a folder that went missing is looked for every
//! `POLL` by the worker alone). The folder is resolved once at start; a change of the setting
//! needs a restart, as in iTerm2.

use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::Storage::FileSystem::{FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FindCloseChangeNotification, FindFirstChangeNotificationW, FindNextChangeNotification};
use windows_sys::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects};

use crate::pty::Notify;

/// A storm of changes (a sync tool rewriting the folder) never holds the reload off for longer
/// than this after the first change of the burst.
const MAX_DEFER: Duration = Duration::from_secs(5);
/// How often a folder that cannot be watched (missing, or removed while watched) is looked for.
const POLL: Duration = Duration::from_secs(5);

/// An auto-reset event that wakes the worker when the scanner goes.
struct Wake(HANDLE);

// SAFETY: an event is a process-wide kernel handle, usable from any thread.
unsafe impl Send for Wake {}
unsafe impl Sync for Wake {}

impl Drop for Wake {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// What the scanner and its worker share.
struct Shared {
    reload: AtomicBool,
    stop: AtomicBool,
    wake: Wake,
}

impl Shared {
    fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        unsafe { SetEvent(self.wake.0) };
    }
}

/// Every running watch, so the one process teardown can stop them all (`stop_all`).
static RUNNING: Mutex<Vec<Arc<Shared>>> = Mutex::new(Vec::new());

/// Stops every watch: the one process teardown (`teardown_process`) calls it. Safe to call twice.
pub fn stop_all() {
    stop_running(&|_| true);
}

fn stop_running(which: &dyn Fn(&Arc<Shared>) -> bool) {
    let mut gone = Vec::new();
    if let Ok(mut r) = RUNNING.lock() {
        let (stop, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut *r).into_iter().partition(|s| which(s));
        *r = keep;
        gone = stop;
    }
    for s in gone {
        s.stop();
    }
}

pub struct Scanner {
    folder: PathBuf,
    shared: Arc<Shared>,
    /// The worker thread; not joined. The tests watch it end.
    #[cfg_attr(not(test), allow(dead_code))]
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Scanner {
    /// Watches `folder`, asking for a reload at once and then `latency` after the last change.
    /// `notify` wakes the app when a reload is ready. A folder that the network judge refuses is
    /// not watched: one reload is asked for (which reports the refusal) and no thread starts.
    pub fn start(folder: PathBuf, latency: Duration, notify: Notify) -> Scanner {
        let shared = Arc::new(Shared { reload: AtomicBool::new(false), stop: AtomicBool::new(false), wake: Wake(unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) }) });
        if !crate::dynamic_profiles::system_local(&folder) {
            shared.reload.store(true, Ordering::SeqCst);
            notify();
            return Scanner { folder, shared, worker: None };
        }
        if let Ok(mut r) = RUNNING.lock() {
            r.push(shared.clone());
        }
        let (f, sh) = (folder.clone(), shared.clone());
        let worker = std::thread::spawn(move || run(f, latency, notify, sh));
        Scanner { folder, shared, worker: Some(worker) }
    }

    /// Whether a reload was asked for since the last call.
    pub fn take_reload(&self) -> bool {
        self.shared.reload.swap(false, Ordering::SeqCst)
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }
}

impl Drop for Scanner {
    fn drop(&mut self) {
        self.shared.stop();
        if let Ok(mut r) = RUNNING.lock() {
            r.retain(|s| !Arc::ptr_eq(s, &self.shared));
        }
    }
}

/// A change notification on the whole tree under `folder`, None when it cannot be watched.
fn watch(folder: &Path) -> Option<HANDLE> {
    let wide: Vec<u16> = folder.as_os_str().encode_wide().chain(Some(0)).collect();
    let h = unsafe { FindFirstChangeNotificationW(wide.as_ptr(), 1, FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_DIR_NAME) };
    (h != INVALID_HANDLE_VALUE).then_some(h)
}

fn run(folder: PathBuf, latency: Duration, notify: Notify, shared: Arc<Shared>) {
    let mut change = watch(&folder);
    // The reload at start.
    let mut due = Some(Instant::now());
    let mut first: Option<Instant> = None;
    loop {
        if shared.stop.load(Ordering::SeqCst) {
            break;
        }
        if due.is_some_and(|t| Instant::now() >= t) {
            due = None;
            first = None;
            shared.reload.store(true, Ordering::SeqCst);
            notify();
        }
        let mut timeout = due.map_or(INFINITE, |t| t.saturating_duration_since(Instant::now()).as_millis().max(1) as u32);
        if change.is_none() {
            timeout = timeout.min(POLL.as_millis() as u32);
        }
        let handles: Vec<HANDLE> = std::iter::once(shared.wake.0).chain(change).collect();
        let r = unsafe { WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, timeout) };
        if r == WAIT_OBJECT_0 {
            continue;
        } else if let (true, Some(h)) = (r == WAIT_OBJECT_0 + 1, change) {
            // A watch that cannot go on (its folder was removed) is dropped at once, or it would
            // stay signaled and spin; the reload then finds the folder missing.
            if unsafe { FindNextChangeNotification(h) } == 0 || !folder.is_dir() {
                unsafe { FindCloseChangeNotification(h) };
                change = None;
                due = Some(Instant::now());
            } else {
                let now = Instant::now();
                let began = *first.get_or_insert(now);
                due = Some((now + latency).min(began + MAX_DEFER));
            }
        } else if r == WAIT_TIMEOUT {
            if change.is_none() && folder.is_dir() {
                change = watch(&folder);
                if change.is_some() {
                    due = Some(Instant::now());
                }
            }
        } else {
            break;
        }
    }
    if let Some(h) = change {
        unsafe { FindCloseChangeNotification(h) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> TempDir {
            let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
            let dir = std::env::temp_dir().join(format!("tandem-dynscan-{tag}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn wait_for_reload(s: &Scanner, within: Duration) -> bool {
        let until = Instant::now() + within;
        while Instant::now() < until {
            if s.take_reload() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn scanner_sees_changes() {
        let tmp = TempDir::new("see");
        let s = Scanner::start(tmp.0.clone(), Duration::from_millis(100), Arc::new(|| {}));
        assert_eq!(s.folder(), tmp.0);
        // A reload request at once, once.
        assert!(wait_for_reload(&s, Duration::from_secs(2)), "no reload at start");
        assert!(!wait_for_reload(&s, Duration::from_millis(400)), "a reload with nothing changed");
        std::fs::write(tmp.0.join("a.json"), "{}").unwrap();
        assert!(wait_for_reload(&s, Duration::from_secs(2)), "a new file was not seen");
        // A change in a subfolder is seen too.
        std::fs::create_dir(tmp.0.join("sub")).unwrap();
        assert!(wait_for_reload(&s, Duration::from_secs(2)), "a new folder was not seen");
    }

    #[test]
    fn a_burst_of_changes_asks_for_one_reload() {
        let tmp = TempDir::new("burst");
        let s = Scanner::start(tmp.0.clone(), Duration::from_millis(200), Arc::new(|| {}));
        assert!(wait_for_reload(&s, Duration::from_secs(2)));
        for i in 0..40 {
            std::fs::write(tmp.0.join(format!("f{i}.json")), "{}").unwrap();
        }
        assert!(wait_for_reload(&s, Duration::from_secs(3)), "the burst was not seen");
        assert!(!wait_for_reload(&s, Duration::from_millis(600)), "the burst asked for a second reload");
    }

    #[test]
    fn the_worker_wakes_the_app_only_for_a_reload_and_stops_with_the_scanner() {
        let tmp = TempDir::new("wake");
        let wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let w = wakes.clone();
        let mut s = Scanner::start(tmp.0.clone(), Duration::from_millis(50), Arc::new(move || {
            w.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }));
        assert!(wait_for_reload(&s, Duration::from_secs(2)));
        let after_start = wakes.load(std::sync::atomic::Ordering::SeqCst);
        // Idle: nothing wakes the app.
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(wakes.load(std::sync::atomic::Ordering::SeqCst), after_start, "an idle watch woke the app");
        let worker = s.worker.take().unwrap();
        drop(s);
        let until = Instant::now() + Duration::from_secs(3);
        while !worker.is_finished() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(worker.is_finished(), "the worker did not stop with the scanner");
    }

    #[test]
    fn stop_all_stops_every_watch() {
        let tmp = TempDir::new("stop");
        let mut s = Scanner::start(tmp.0.clone(), Duration::from_millis(50), Arc::new(|| {}));
        assert!(wait_for_reload(&s, Duration::from_secs(2)));
        let worker = s.worker.take().unwrap();
        // The teardown's own stop, aimed at this watch only (the other tests' watches run on).
        let mine = s.shared.clone();
        stop_running(&|w| Arc::ptr_eq(w, &mine));
        let until = Instant::now() + Duration::from_secs(3);
        while !worker.is_finished() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(worker.is_finished(), "the process teardown did not stop the watch");
        drop(s);
    }
}

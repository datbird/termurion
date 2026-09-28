//! Watches the Scripts folder and scans it on a worker thread (`M:120-148`, `M:1669-1680`):
//! a scan at start, then one 500 ms after the last change a subtree change notification saw.
//! The slow-scan question goes to the app and the worker waits for its answer.

use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::Storage::FileSystem::{FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FindCloseChangeNotification, FindFirstChangeNotificationW, FindNextChangeNotification};
use windows_sys::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects};

use crate::pty::Notify;
use crate::scripts::{self, RealFs, ScanResult};

/// How long the folder must be quiet after a change before it is scanned again.
const SETTLE: Duration = Duration::from_millis(500);
/// How often a folder that cannot be watched (missing, or removed while watched) is looked
/// for, so the watch starts again when it comes back.
const POLL: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub enum ScanEvent {
    /// The scan passed its time limit: `answer_slow` says whether it goes on.
    Slow,
    Done(ScanResult),
}

enum Cmd {
    Answer(bool),
    Rescan,
    Hold(bool),
}

/// An auto-reset event that wakes the worker when a command is sent or the scanner goes.
struct Wake(HANDLE);

// SAFETY: an event is a process-wide kernel handle, usable from any thread.
unsafe impl Send for Wake {}
unsafe impl Sync for Wake {}

impl Drop for Wake {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

pub struct Scanner {
    folder: PathBuf,
    /// Taken in Drop, before the worker is woken, so the worker always sees it gone.
    commands: Option<Sender<Cmd>>,
    events: Receiver<ScanEvent>,
    wake: Arc<Wake>,
    /// Scans are held (a Python environment is being built in the folder).
    held: bool,
    /// The worker thread; not joined (a scan in progress finishes on its own). The tests
    /// watch it end.
    #[cfg_attr(not(test), allow(dead_code))]
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Scanner {
    /// Makes `folder` when it is missing (`M:131-132`), scans it at once and watches it.
    /// `notify` wakes the app when an event is ready. A folder reached through a link to a
    /// network path (`scripts::ancestors_stay_local`) is not made, watched or polled: the
    /// scanner gives one empty result and nothing more.
    pub fn start(folder: PathBuf, notify: Notify) -> Scanner {
        Scanner::start_with(folder, notify, &scripts::read_link)
    }

    /// `start`, reading links with `read_link`.
    fn start_with(folder: PathBuf, notify: Notify, read_link: &dyn Fn(&Path) -> Option<PathBuf>) -> Scanner {
        let (event_tx, events) = mpsc::channel();
        let wake = Arc::new(Wake(unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) }));
        if !scripts::ancestors_stay_local(&folder, read_link) {
            let _ = event_tx.send(ScanEvent::Done(ScanResult::default()));
            notify();
            return Scanner { folder, commands: None, events, wake, held: false, worker: None };
        }
        let _ = std::fs::create_dir_all(&folder);
        let (commands, command_rx) = mpsc::channel();
        let worker = Worker { folder: folder.clone(), commands: command_rx, events: event_tx, notify, wake: wake.clone(), disabled: false, held: false };
        let worker = std::thread::spawn(move || worker.run());
        Scanner { folder, commands: Some(commands), events, wake, held: false, worker: Some(worker) }
    }

    pub fn try_event(&self) -> Option<ScanEvent> {
        self.events.try_recv().ok()
    }

    /// The answer to `ScanEvent::Slow`: true is Continue, false is Stop, after which every
    /// scan is empty until a new scanner starts (iTerm2's `_disableEnumeration`).
    pub fn answer_slow(&self, go_on: bool) {
        self.send(Cmd::Answer(go_on));
    }

    /// Holds scans while a Python environment is built (its many file events would rescan
    /// again and again; iTerm2 skips them while provisioning, `M:1669-1680`); letting go scans
    /// once. Repeating the current state does nothing.
    pub fn hold(&mut self, on: bool) {
        if self.held != on {
            self.held = on;
            self.send(Cmd::Hold(on));
        }
    }

    /// Scans again now.
    pub fn rescan(&self) {
        self.send(Cmd::Rescan);
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    fn send(&self, cmd: Cmd) {
        if let Some(commands) = &self.commands {
            let _ = commands.send(cmd);
        }
        unsafe { SetEvent(self.wake.0) };
    }
}

impl Drop for Scanner {
    /// The worker sees its commands end and stops; a scan in progress finishes first. The
    /// channel closes before the wake, so a worker that wakes finds it closed.
    fn drop(&mut self) {
        self.commands = None;
        unsafe { SetEvent(self.wake.0) };
    }
}

/// A change notification on the whole tree under `folder`, None when it cannot be watched.
fn watch(folder: &Path) -> Option<HANDLE> {
    let wide: Vec<u16> = folder.as_os_str().encode_wide().chain(Some(0)).collect();
    let h = unsafe { FindFirstChangeNotificationW(wide.as_ptr(), 1, FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME) };
    (h != INVALID_HANDLE_VALUE).then_some(h)
}

struct Worker {
    folder: PathBuf,
    commands: Receiver<Cmd>,
    events: Sender<ScanEvent>,
    notify: Notify,
    wake: Arc<Wake>,
    disabled: bool,
    held: bool,
}

impl Worker {
    fn run(mut self) {
        // Without a watch (the folder is missing) the folder is looked for every POLL and
        // watched again when it is back.
        let mut change = watch(&self.folder);
        let mut due = Some(Instant::now());
        'run: loop {
            // A scan falling due while held waits for the hold to end, which scans anyway.
            if self.held && due.is_some() {
                due = None;
            }
            if due.is_some_and(|t| Instant::now() >= t) {
                due = None;
                match self.scan_once() {
                    None => break,
                    Some(again) => {
                        if again {
                            due = Some(Instant::now());
                            continue;
                        }
                    }
                }
            }
            let mut timeout = due.map_or(INFINITE, |t| t.saturating_duration_since(Instant::now()).as_millis() as u32);
            if change.is_none() {
                timeout = timeout.min(POLL.as_millis() as u32);
            }
            let handles: Vec<HANDLE> = std::iter::once(self.wake.0).chain(change).collect();
            let r = unsafe { WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, timeout) };
            if r == WAIT_OBJECT_0 {
                loop {
                    match self.commands.try_recv() {
                        Ok(Cmd::Rescan) => due = Some(Instant::now()),
                        Ok(Cmd::Hold(on)) => {
                            self.held = on;
                            if !on {
                                due = Some(Instant::now());
                            }
                        }
                        // An answer with no question waiting.
                        Ok(Cmd::Answer(_)) => {}
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => break 'run,
                    }
                }
            } else if let (true, Some(h)) = (r == WAIT_OBJECT_0 + 1, change) {
                // A watch that cannot go on (its folder was removed) is dropped at once, or
                // it would stay signaled and spin; the scan then finds the folder missing.
                if unsafe { FindNextChangeNotification(h) } == 0 || !self.folder.is_dir() {
                    unsafe { FindCloseChangeNotification(h) };
                    change = None;
                    due = Some(Instant::now());
                } else {
                    due = Some(Instant::now() + SETTLE);
                }
            } else if r == WAIT_TIMEOUT {
                if change.is_none() && self.folder.is_dir() {
                    change = watch(&self.folder);
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

    /// One scan, its result sent. None when the scanner is gone; else whether a rescan was
    /// asked for while the scan waited for an answer.
    fn scan_once(&mut self) -> Option<bool> {
        if self.disabled {
            self.send(ScanEvent::Done(ScanResult::default()));
            return Some(false);
        }
        let start = Instant::now();
        let (mut gone, mut again, mut held) = (false, false, self.held);
        let result = {
            let (events, notify, commands) = (&self.events, &self.notify, &self.commands);
            scripts::scan(&self.folder, &RealFs, &mut || start.elapsed(), &mut || {
                let _ = events.send(ScanEvent::Slow);
                notify();
                loop {
                    match commands.recv() {
                        Ok(Cmd::Answer(go_on)) => return go_on,
                        Ok(Cmd::Rescan) => again = true,
                        Ok(Cmd::Hold(on)) => {
                            held = on;
                            // Letting go asks for the scan the hold put off.
                            again |= !on;
                        }
                        Err(_) => {
                            gone = true;
                            return false;
                        }
                    }
                }
            })
        };
        self.held = held;
        if gone {
            return None;
        }
        if result.stopped {
            self.disabled = true;
        }
        self.send(ScanEvent::Done(result));
        Some(again)
    }

    fn send(&self, event: ScanEvent) {
        let _ = self.events.send(event);
        (self.notify)();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_for_done(s: &Scanner, want: &[&str]) -> Vec<String> {
        let until = Instant::now() + Duration::from_secs(6);
        let mut last = Vec::new();
        while Instant::now() < until {
            match s.try_event() {
                Some(ScanEvent::Done(r)) => {
                    last = scripts::all_scripts(&r.tree);
                    if last == want {
                        return last;
                    }
                }
                Some(ScanEvent::Slow) => s.answer_slow(true),
                None => std::thread::sleep(Duration::from_millis(20)),
            }
        }
        last
    }

    /// A fresh temp folder, removed when the test ends, passed or not.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> TempDir {
            let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
            let dir = std::env::temp_dir().join(format!("tandem-scripts-{tag}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The worker thread ends within 3 s of the scanner going.
    fn ends(mut s: Scanner) -> bool {
        let worker = s.worker.take().unwrap();
        drop(s);
        let until = Instant::now() + Duration::from_secs(3);
        while !worker.is_finished() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        worker.is_finished()
    }

    #[test]
    fn scanner_sees_changes() {
        let tmp = TempDir::new("scan");
        let dir = tmp.0.clone();
        std::fs::write(dir.join("a.py"), "").unwrap();
        let s = Scanner::start(dir.clone(), Arc::new(|| {}));
        assert_eq!(s.folder(), dir);
        assert_eq!(wait_for_done(&s, &["a.py"]), ["a.py"]);
        std::fs::write(dir.join("b.py"), "").unwrap();
        assert_eq!(wait_for_done(&s, &["a.py", "b.py"]), ["a.py", "b.py"]);
        assert!(ends(s), "the worker thread did not end");
    }

    #[test]
    fn scanner_refuses_a_folder_through_a_network_link() {
        let tmp = TempDir::new("unc");
        let dir = tmp.0.join("Scripts");
        // The folder above the Scripts folder is a link to a share.
        let above = tmp.0.clone();
        let read_link = move |p: &Path| (p == above).then(|| PathBuf::from(r"\\h\share"));
        let mut s = Scanner::start_with(dir.clone(), Arc::new(|| {}), &read_link);
        // Nothing is made, watched or polled: no worker, one empty result.
        assert!(s.worker.take().is_none(), "a worker started");
        assert!(!dir.exists(), "the folder was made");
        match s.try_event() {
            Some(ScanEvent::Done(r)) => assert_eq!(r, ScanResult::default()),
            other => panic!("{other:?}"),
        }
        assert!(s.try_event().is_none());
        // Its commands go nowhere.
        s.rescan();
        s.hold(true);
        assert!(s.try_event().is_none());
    }

    #[test]
    fn scanner_holds_while_provisioning() {
        let tmp = TempDir::new("hold");
        let dir = tmp.0.clone();
        std::fs::write(dir.join("a.py"), "").unwrap();
        let mut s = Scanner::start(dir.clone(), Arc::new(|| {}));
        assert_eq!(wait_for_done(&s, &["a.py"]), ["a.py"]);
        s.hold(true);
        std::fs::write(dir.join("b.py"), "").unwrap();
        // Past the settle time no scan comes while held.
        std::thread::sleep(SETTLE * 3);
        assert!(s.try_event().is_none(), "a scan came while held");
        // Letting go scans once and sees the change.
        s.hold(false);
        assert_eq!(wait_for_done(&s, &["a.py", "b.py"]), ["a.py", "b.py"]);
        assert!(ends(s), "the worker thread did not end");
    }

    #[test]
    fn scanner_survives_folder_removal() {
        let tmp = TempDir::new("gone");
        let dir = tmp.0.join("Scripts");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.py"), "").unwrap();
        let s = Scanner::start(dir.clone(), Arc::new(|| {}));
        assert_eq!(wait_for_done(&s, &["a.py"]), ["a.py"]);
        std::fs::remove_dir_all(&dir).unwrap();
        // The watch lets go of the removed folder and a scan finds it empty.
        assert!(wait_for_done(&s, &[]).is_empty());
        // Back again: once it can be made (the removal completes when the watch closes),
        // the poll watches it again and scans it, with no rescan asked for.
        let until = Instant::now() + Duration::from_secs(3);
        while std::fs::create_dir(&dir).is_err() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(50));
        }
        std::fs::write(dir.join("a.py"), "").unwrap();
        assert_eq!(wait_for_done(&s, &["a.py"]), ["a.py"]);
        // And the new watch sees changes.
        std::fs::write(dir.join("b.py"), "").unwrap();
        assert_eq!(wait_for_done(&s, &["a.py", "b.py"]), ["a.py", "b.py"]);
        assert!(ends(s), "the worker thread did not end");
    }
}

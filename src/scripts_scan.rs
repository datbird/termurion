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

#[derive(Debug)]
pub enum ScanEvent {
    /// The scan passed its time limit: `answer_slow` says whether it goes on.
    Slow,
    Done(ScanResult),
}

enum Cmd {
    Answer(bool),
    Rescan,
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
    commands: Sender<Cmd>,
    events: Receiver<ScanEvent>,
    wake: Arc<Wake>,
}

impl Scanner {
    /// Makes `folder` when it is missing (`M:131-132`), scans it at once and watches it.
    /// `notify` wakes the app when an event is ready.
    pub fn start(folder: PathBuf, notify: Notify) -> Scanner {
        let _ = std::fs::create_dir_all(&folder);
        let (commands, command_rx) = mpsc::channel();
        let (event_tx, events) = mpsc::channel();
        let wake = Arc::new(Wake(unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) }));
        let worker = Worker { folder: folder.clone(), commands: command_rx, events: event_tx, notify, wake: wake.clone(), disabled: false };
        std::thread::spawn(move || worker.run());
        Scanner { folder, commands, events, wake }
    }

    pub fn try_event(&self) -> Option<ScanEvent> {
        self.events.try_recv().ok()
    }

    /// The answer to `ScanEvent::Slow`: true is Continue, false is Stop, after which every
    /// scan is empty until a new scanner starts (iTerm2's `_disableEnumeration`).
    pub fn answer_slow(&self, go_on: bool) {
        self.send(Cmd::Answer(go_on));
    }

    /// Scans again now.
    pub fn rescan(&self) {
        self.send(Cmd::Rescan);
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    fn send(&self, cmd: Cmd) {
        let _ = self.commands.send(cmd);
        unsafe { SetEvent(self.wake.0) };
    }
}

impl Drop for Scanner {
    /// The worker sees its commands end and stops; a scan in progress finishes first.
    fn drop(&mut self) {
        unsafe { SetEvent(self.wake.0) };
    }
}

struct Worker {
    folder: PathBuf,
    commands: Receiver<Cmd>,
    events: Sender<ScanEvent>,
    notify: Notify,
    wake: Arc<Wake>,
    disabled: bool,
}

impl Worker {
    fn run(mut self) {
        let wide: Vec<u16> = self.folder.as_os_str().encode_wide().chain(Some(0)).collect();
        let change = unsafe { FindFirstChangeNotificationW(wide.as_ptr(), 1, FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME) };
        // Without a watch (the folder could not be made) only rescans scan.
        let mut handles = vec![self.wake.0];
        if change != INVALID_HANDLE_VALUE {
            handles.push(change);
        }
        let mut due = Some(Instant::now());
        'run: loop {
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
            let timeout = due.map_or(INFINITE, |t| t.saturating_duration_since(Instant::now()).as_millis() as u32);
            let r = unsafe { WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, timeout) };
            if r == WAIT_OBJECT_0 {
                loop {
                    match self.commands.try_recv() {
                        Ok(Cmd::Rescan) => due = Some(Instant::now()),
                        // An answer with no question waiting.
                        Ok(Cmd::Answer(_)) => {}
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => break 'run,
                    }
                }
            } else if r == WAIT_OBJECT_0 + 1 {
                unsafe { FindNextChangeNotification(change) };
                due = Some(Instant::now() + SETTLE);
            } else if r != WAIT_TIMEOUT {
                break;
            }
        }
        if change != INVALID_HANDLE_VALUE {
            unsafe { FindCloseChangeNotification(change) };
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
        let (mut gone, mut again) = (false, false);
        let result = {
            let (events, notify, commands) = (&self.events, &self.notify, &self.commands);
            scripts::scan(&self.folder, &RealFs, &mut || start.elapsed(), &mut || {
                let _ = events.send(ScanEvent::Slow);
                notify();
                loop {
                    match commands.recv() {
                        Ok(Cmd::Answer(go_on)) => return go_on,
                        Ok(Cmd::Rescan) => again = true,
                        Err(_) => {
                            gone = true;
                            return false;
                        }
                    }
                }
            })
        };
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
        let until = Instant::now() + Duration::from_secs(3);
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

    #[test]
    fn scanner_sees_changes() {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!("tandem-scripts-scan-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.py"), "").unwrap();
        let s = Scanner::start(dir.clone(), Arc::new(|| {}));
        assert_eq!(s.folder(), dir);
        assert_eq!(wait_for_done(&s, &["a.py"]), ["a.py"]);
        std::fs::write(dir.join("b.py"), "").unwrap();
        assert_eq!(wait_for_done(&s, &["a.py", "b.py"]), ["a.py", "b.py"]);
        drop(s);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

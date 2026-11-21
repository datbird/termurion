//! A child process on a pseudo terminal: ConPTY on Windows, a Unix pty elsewhere.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering, fence};

#[cfg(unix)]
use super::unix::pty as imp;
#[cfg(windows)]
use super::windows::pty as imp;

/// Called from background threads when a pty has new output or its child exits.
pub type Notify = Arc<dyn Fn() + Send + Sync>;

/// The result of a non-blocking read.
pub enum Read {
    Data(Vec<u8>),
    Empty,
    /// The output pipe closed. ConPTY closes it only after the pseudoconsole closes,
    /// so a child exit shows up in `exit_code`, not here.
    Closed,
}

/// The pseudo terminal's loaded library: ConPTY's functions on Windows, nothing to load on Unix.
#[cfg(windows)]
pub type PtyApi = super::windows::conpty::Api;
#[cfg(unix)]
pub use super::unix::pty::PtyApi;

pub use crate::ssh_transport::Transport;
pub use imp::{Pty, console_mode_unless, default_shell, inherits_term, spawn, spawn_notify, spawn_transport};

// Windows-only pieces (the loader, handoff, console attach) are reached through these re-exports.
#[cfg(windows)]
pub use super::windows::conpty;
#[cfg(windows)]
pub use super::windows::pty::{console_mode, from_handoff, spawn_flags};
#[cfg(windows)]
pub(crate) use super::windows::pty::{attached_unless, send_ctrl_event_attached};

/// Loads the pseudo terminal library the app spawns shells with. Windows takes the ConPTY shipped
/// beside the exe, else the copy built into Windows (the note says so for a banner); Unix has none to
/// load.
pub fn load_for_app(exe_dir: &std::path::Path) -> Result<(PtyApi, Option<String>), String> {
    imp::load_for_app(exe_dir)
}

/// Runs a process spawn with no other spawn of ours in flight where that matters (Unix: the gate
/// that keeps a freshly made descriptor from leaking into a child); Windows needs none.
pub fn spawn_gated<T>(f: impl FnOnce() -> T) -> T {
    imp::spawn_gated(f)
}

/// The loaded library has its pack (the default-terminal rule); never on Unix.
pub fn api_has_pack(api: &PtyApi) -> bool {
    imp::api_has_pack(api)
}

/// The default-terminal start rule: the shipped ConPTY has its pack (Windows only).
pub fn shipped_has_pack(exe_dir: &std::path::Path) -> bool {
    imp::shipped_has_pack(exe_dir)
}

/// The api the live tests spawn real shells with: the repo's shipped ConPTY on Windows.
#[cfg(test)]
pub fn test_api() -> PtyApi {
    imp::test_api()
}

/// Reads until `needle` appears, taking chunks from `next` (which waits up to the time it is
/// given). Ok with everything read on a hit, Err with everything read on timeout. Both backends'
/// `Pty::read_until` call it.
pub(crate) fn read_until_with(mut next: impl FnMut(std::time::Duration) -> Option<Vec<u8>>, needle: &[u8], wait: std::time::Duration) -> Result<Vec<u8>, Vec<u8>> {
    let end = std::time::Instant::now() + wait;
    let mut all = Vec::new();
    loop {
        if all.windows(needle.len()).any(|w| w == needle) {
            return Ok(all);
        }
        let left = end.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return Err(all);
        }
        if let Some(chunk) = next(left.min(std::time::Duration::from_millis(50))) {
            all.extend(chunk);
        }
    }
}

/// When a reader wakes its consumer. Off (local panes): after every chunk. On (the session
/// host): only on the first chunk since the consumer last found the output empty and said so
/// with `rearm`, so a flood costs one wake per drain, not one per 64 KiB read.
#[derive(Default)]
pub(crate) struct EdgeWake {
    on: AtomicBool,
    /// A wake went and the consumer has not drained since.
    signaled: AtomicBool,
}

impl EdgeWake {
    pub(crate) fn set_on(&self, on: bool) {
        self.on.store(on, Ordering::SeqCst);
    }

    /// The reader queued a chunk: should it notify?
    pub(crate) fn queued(&self) -> bool {
        if !self.on.load(Ordering::Acquire) {
            return true;
        }
        // With the fence in `rearm`: either this swap sees the consumer's false, or the
        // consumer's read after `rearm` sees this chunk.
        fence(Ordering::SeqCst);
        !self.signaled.swap(true, Ordering::SeqCst)
    }

    /// The consumer found nothing queued: the next chunk notifies. It must read once more after
    /// this, since a chunk queued just before it did not notify.
    pub(crate) fn rearm(&self) {
        self.signaled.store(false, Ordering::SeqCst);
        fence(Ordering::SeqCst);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::mpsc::{self, TryRecvError};
    use std::sync::{Condvar, Mutex};
    use std::time::Duration;

    /// Counts notifies; `wait` returns once the count passes `seen` or `most` passes.
    #[derive(Default)]
    pub(crate) struct Bell {
        n: Mutex<u64>,
        rung: Condvar,
    }

    impl Bell {
        pub(crate) fn ring(&self) {
            *self.n.lock().unwrap() += 1;
            self.rung.notify_all();
        }

        pub(crate) fn count(&self) -> u64 {
            *self.n.lock().unwrap()
        }

        pub(crate) fn wait(&self, seen: u64, most: Duration) -> u64 {
            let g = self.n.lock().unwrap();
            *self.rung.wait_timeout_while(g, most, |n| *n == seen).unwrap().0
        }
    }

    /// The reader and a pump's drain, rearm, read again. The writer sends bursts and waits for
    /// each to be taken, so every burst's last chunk is a chance to be lost; the consumer
    /// lingers between its empty read and the rearm, so the reader often lands right there. A
    /// lost chunk stalls the consumer for its full second.
    #[test]
    fn edge_wake_never_loses_a_chunk() {
        const BURSTS: u64 = 20_000;
        let total: u64 = (0..BURSTS).map(|b| 1 + b % 5).sum();
        let edge = Arc::new(EdgeWake::default());
        edge.set_on(true);
        let bell = Arc::new(Bell::default());
        let taken = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let (tx, rx) = mpsc::channel::<u64>();
        let (e2, b2, t2) = (edge.clone(), bell.clone(), taken.clone());
        let writer = std::thread::spawn(move || {
            let mut sent = 0;
            for b in 0..BURSTS {
                for _ in 0..1 + b % 5 {
                    tx.send(sent).unwrap();
                    sent += 1;
                    if e2.queued() {
                        b2.ring();
                    }
                }
                // A stalled consumer fails the test; this only keeps the thread from spinning on.
                let at = std::time::Instant::now();
                while t2.load(Ordering::Acquire) < sent && at.elapsed() < Duration::from_secs(3) {
                    std::thread::yield_now();
                }
            }
        });
        let take = |i: u64, next: &mut u64| {
            assert_eq!(i, *next);
            *next += 1;
            taken.store(*next, Ordering::Release);
        };
        let (mut next, mut seen) = (0u64, 0u64);
        while next < total {
            let now = bell.wait(seen, Duration::from_secs(1));
            if now == seen {
                assert!(rx.try_recv().is_err(), "no wake for a second with chunk {next} queued");
                continue;
            }
            seen = now;
            loop {
                match rx.try_recv() {
                    Ok(i) => take(i, &mut next),
                    Err(TryRecvError::Empty) => {
                        for _ in 0..next % 200 {
                            std::hint::spin_loop();
                        }
                        edge.rearm();
                        match rx.try_recv() {
                            Ok(i) => take(i, &mut next),
                            Err(_) => break,
                        }
                    }
                    Err(TryRecvError::Disconnected) => break,
                }
            }
        }
        writer.join().unwrap();
        assert!(bell.count() < total, "edge wakes are fewer than chunks: {}", bell.count());
    }

    #[test]
    fn edge_wake_off_notifies_every_chunk() {
        let edge = EdgeWake::default();
        assert!((0..10).all(|_| edge.queued()));
        edge.set_on(true);
        assert!(edge.queued());
        assert!(!edge.queued(), "one wake until the consumer rearms");
        edge.rearm();
        assert!(edge.queued());
    }
}

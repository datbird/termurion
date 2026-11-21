//! The byte budget between a pty reader thread and whoever takes its chunks. The reader stops
//! reading once `PTY_BUDGET` bytes wait unread, so the program blocks on the full pty (or pipe)
//! instead of piling its output up in memory, and reads again at `PTY_RESUME` or less.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// The reader stops reading at this many bytes queued.
pub const PTY_BUDGET: usize = 1 << 20;
/// It reads again at this many or less.
pub const PTY_RESUME: usize = 512 << 10;

pub struct Budget {
    /// Bytes queued and not yet taken, and closed (the pty is gone: never wait again).
    state: Mutex<(usize, bool)>,
    room: Condvar,
}

impl Budget {
    pub fn new() -> Arc<Budget> {
        Arc::new(Budget { state: Mutex::new((0, false)), room: Condvar::new() })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, (usize, bool)> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The reader queued `n` bytes.
    pub fn add(&self, n: usize) {
        self.lock().0 += n;
    }

    /// The consumer took `n` bytes.
    pub fn take(&self, n: usize) {
        let mut g = self.lock();
        g.0 = g.0.saturating_sub(n);
        if g.0 <= PTY_RESUME {
            self.room.notify_all();
        }
    }

    /// Returns at once below `PTY_BUDGET` or once closed. Otherwise waits until `PTY_RESUME` or
    /// less is queued, the budget closes, or `give_up` (asked every `every`) says so.
    pub fn wait_for_room(&self, every: Duration, mut give_up: impl FnMut() -> bool) {
        let mut g = self.lock();
        if g.0 < PTY_BUDGET {
            return;
        }
        while g.0 > PTY_RESUME && !g.1 {
            if give_up() {
                return;
            }
            g = self.room.wait_timeout(g, every).unwrap_or_else(|e| e.into_inner()).0;
        }
    }

    pub fn queued(&self) -> usize {
        self.lock().0
    }

    /// The pty is going: no wait holds the reader any more, now or later.
    pub fn close(&self) {
        self.lock().1 = true;
        self.room.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    const CHUNK: usize = 64 * 1024;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// A reader that adds a chunk per turn after `wait_for_room`, counting its turns.
    fn reader(b: Arc<Budget>, turns: Arc<std::sync::atomic::AtomicUsize>, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                b.wait_for_room(ms(100), || false);
                if stop.load(Ordering::Acquire) {
                    break;
                }
                b.add(CHUNK);
                turns.fetch_add(1, Ordering::AcqRel);
            }
        })
    }

    /// Waits until `turns` stops moving for 100 ms; returns its value.
    fn settled(turns: &std::sync::atomic::AtomicUsize) -> usize {
        let mut last = turns.load(Ordering::Acquire);
        loop {
            std::thread::sleep(ms(100));
            let now = turns.load(Ordering::Acquire);
            if now == last {
                return now;
            }
            last = now;
        }
    }

    #[test]
    fn the_reader_waits_at_the_budget_and_resumes_at_half() {
        let b = Budget::new();
        let turns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let t = reader(b.clone(), turns.clone(), stop.clone());
        let at = settled(&turns);
        assert!(b.queued() >= PTY_BUDGET, "it read up to the budget: {}", b.queued());
        assert_eq!(b.queued(), PTY_BUDGET, "and stopped there");
        b.take(b.queued() - (PTY_RESUME + 1));
        std::thread::sleep(ms(100));
        assert_eq!(turns.load(Ordering::Acquire), at, "still waiting one byte above the resume mark");
        b.take(1);
        let end = Instant::now() + ms(100);
        while turns.load(Ordering::Acquire) == at {
            assert!(Instant::now() < end, "it read again within 100 ms of the resume mark");
            std::thread::sleep(ms(2));
        }
        stop.store(true, Ordering::Release);
        b.close();
        t.join().unwrap();
    }

    #[test]
    fn close_ends_a_wait_for_good() {
        let b = Budget::new();
        b.add(PTY_BUDGET);
        let b2 = b.clone();
        let t = std::thread::spawn(move || b2.wait_for_room(ms(100), || false));
        std::thread::sleep(ms(100));
        assert!(!t.is_finished(), "it waits at the budget");
        let closed = Instant::now();
        b.close();
        t.join().unwrap();
        assert!(closed.elapsed() < ms(100), "close ends the wait at once: {:?}", closed.elapsed());
        b.add(10 * PTY_BUDGET);
        let at = Instant::now();
        b.wait_for_room(ms(100), || false);
        assert!(at.elapsed() < ms(50), "a closed budget never waits");
    }

    #[test]
    fn give_up_ends_a_wait() {
        let b = Budget::new();
        b.add(PTY_BUDGET);
        let start = Instant::now();
        b.wait_for_room(ms(10), || start.elapsed() >= ms(50));
        let took = start.elapsed();
        assert!(took >= ms(50) && took < ms(100), "{took:?}");
    }

    #[test]
    fn take_never_goes_below_zero() {
        let b = Budget::new();
        b.add(10);
        b.take(25);
        assert_eq!(b.queued(), 0);
        b.add(5);
        assert_eq!(b.queued(), 5);
    }
}

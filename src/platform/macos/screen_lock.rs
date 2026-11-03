//! The screen locking, as iTerm2 sees it (`staticScreenDidLock:`): the `com.apple.screenIsLocked`
//! distributed notification, observed once on the main thread. The observer only sets a flag and
//! wakes the loop; the app locks the password manager on its next pass.

use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::OnceLock;

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2_foundation::{NSDistributedNotificationCenter, NSNotification, NSOperationQueue, NSString};

use crate::platform::pty::Notify;

/// The notification the login window posts when the screen locks.
pub const LOCKED_NOTE: &str = "com.apple.screenIsLocked";

static LOCKED: AtomicBool = AtomicBool::new(false);
static WAKE: OnceLock<Notify> = OnceLock::new();
/// 1 once the observer is in place (it lives as long as the app).
static WATCHING: AtomicUsize = AtomicUsize::new(0);

/// The screen locked: remember it and wake the loop.
fn on_lock() {
    LOCKED.store(true, Ordering::Release);
    if let Some(w) = WAKE.get() {
        w();
    }
}

/// Starts watching for the screen lock, once. The handle is unused: the notification is app-wide.
/// Off the main thread nothing is set up, and `watched` stays 0 so the app asks again later.
pub fn watch(_view: isize, wake: Notify) {
    let _ = WAKE.set(wake);
    if WATCHING.load(Ordering::Acquire) != 0 || MainThreadMarker::new().is_none() {
        return;
    }
    let block = RcBlock::new(|_n: NonNull<NSNotification>| on_lock());
    let center = NSDistributedNotificationCenter::defaultCenter();
    // SAFETY: main thread; the block touches only atomics and the wake callback, and the main
    // queue runs it on the main thread. The returned token is kept alive for the app's life.
    let token = unsafe {
        center.addObserverForName_object_queue_usingBlock(Some(&NSString::from_str(LOCKED_NOTE)), None, Some(&NSOperationQueue::mainQueue()), &block)
    };
    std::mem::forget(token);
    WATCHING.store(1, Ordering::Release);
}

/// How many observers are set up: 0 or 1.
pub fn watched() -> usize {
    WATCHING.load(Ordering::Acquire)
}

/// Whether the screen locked since the last call.
pub fn take() -> bool {
    LOCKED.swap(false, Ordering::AcqRel)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lock is seen once, and the loop is woken for it.
    #[test]
    fn a_lock_is_taken_once_and_wakes_the_loop() {
        static WOKEN: AtomicUsize = AtomicUsize::new(0);
        let _ = WAKE.set(std::sync::Arc::new(|| {
            WOKEN.fetch_add(1, Ordering::SeqCst);
        }));
        let before = WOKEN.load(Ordering::SeqCst);
        take();
        on_lock();
        assert!(WOKEN.load(Ordering::SeqCst) > before);
        assert!(take());
        assert!(!take());
    }

    /// iTerm2's notification name, letter for letter.
    #[test]
    fn the_notification_is_iterm2s() {
        assert_eq!(LOCKED_NOTE, "com.apple.screenIsLocked");
    }
}

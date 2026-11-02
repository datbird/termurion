//! Unix has no end-of-session window message yet: the watch and the block reason do nothing, the
//! shutdown the app can ask about never arrives, and the quit question still runs on its own thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::super::logout::{OnEnd, QueryFn};

static QUESTION_PENDING: AtomicBool = AtomicBool::new(false);
static QUESTION_OPEN: AtomicBool = AtomicBool::new(false);

pub fn question_pending() -> bool {
    QUESTION_PENDING.load(Ordering::SeqCst)
}

pub fn begin_question() -> bool {
    !QUESTION_PENDING.swap(true, Ordering::SeqCst)
}

pub fn end_question() -> bool {
    QUESTION_PENDING.swap(false, Ordering::SeqCst)
}

/// Shows the quit question on a thread of its own; `answered` gets the answer.
pub fn ask_on_thread(ask: impl FnOnce() -> bool + Send + 'static, answered: impl FnOnce(bool) + Send + 'static) {
    QUESTION_OPEN.store(true, Ordering::SeqCst);
    let started = std::thread::Builder::new().name("quit-question".into()).spawn(move || {
        let ok = ask();
        QUESTION_OPEN.store(false, Ordering::SeqCst);
        answered(ok);
    });
    if let Err(e) = started {
        crate::host_base::log(&format!("shutdown: the quit question could not start: {e}"));
        QUESTION_OPEN.store(false, Ordering::SeqCst);
        end_question();
    }
}

pub fn question_open() -> bool {
    QUESTION_OPEN.load(Ordering::SeqCst)
}

/// Waits at most `limit` for the open question to end; nothing closes it from here.
pub fn close_question(limit: Duration) -> bool {
    let end = Instant::now() + limit;
    while question_open() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(10));
    }
    !question_open()
}

/// There is no shutdown block reason on Unix.
pub fn raise_block(_hwnd: isize) {}

pub fn destroy_block() {}

/// Nothing is asked of the app at a shutdown on Unix.
pub fn arm_query(_q: Option<QueryFn>) {}

/// No end-of-session message arrives on Unix, so nothing runs.
pub fn arm(_on_end: Option<OnEnd>) {}

/// No subclass on Unix.
pub fn watch(_hwnd: isize) {}

/// Hands the done flag to `post` and waits for it, at most `limit`. True when the app finished in time.
pub fn run_on_app(post: impl FnOnce(Arc<AtomicBool>), limit: Duration) -> bool {
    let done = Arc::new(AtomicBool::new(false));
    post(done.clone());
    let end = Instant::now() + limit;
    while !done.load(Ordering::Acquire) {
        if Instant::now() >= end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    true
}

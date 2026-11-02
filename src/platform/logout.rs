//! The end of the Windows session (logoff, restart, shut down): the window subclass that hears it, the
//! shutdown block reason, and the quit question shown on a thread of its own. The limits and the
//! `ask_or_end` rule are the same everywhere; Unix has no such message yet.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[cfg(unix)]
use super::unix::logout as imp;
#[cfg(windows)]
use super::windows::logout as imp;

pub use imp::{arm, arm_query, ask_on_thread, begin_question, close_question, destroy_block, end_question, question_open, question_pending, raise_block, run_on_app, watch};

/// The time a logoff gives the app thread to write out and end the shells (the flushes and the
/// kill wait are bounded inside it: `ARCHIVE_LIMIT` + 0.5 s of logs + `KILL_LIMIT`). With the
/// question's close and the busy-app fallback the whole session end stays under Windows' ~5 s
/// WM_ENDSESSION allowance (final review item 2): 0.5 s + 3.5 s + 0.5 s.
pub const LOGOFF_LIMIT: Duration = Duration::from_millis(3500);

/// The kill's wait when the app thread was too busy to do the write-out.
pub const FALLBACK_KILL_LIMIT: Duration = Duration::from_millis(500);

/// The write-out's wait for the archives.
pub const ARCHIVE_LIMIT: Duration = Duration::from_millis(1500);

/// The write-out's wait for the ended shells.
pub const KILL_LIMIT: Duration = Duration::from_secs(1);

pub type OnEnd = Arc<dyn Fn() + Send + Sync>;


/// What a WM_QUERYENDSESSION asks: true when Windows may end the session now.
pub type QueryFn = Arc<dyn Fn(isize) -> bool + Send + Sync>;

/// The time a shutdown query gives the app thread to decide (a timeout means "ask first").
pub const QUERY_LIMIT: Duration = Duration::from_secs(1);

/// Asks the app whether the session may end now: `post` hands over the done flag and the
/// answer flag (true = end now). An app that does not answer in `limit` counts as "ask".
pub fn ask_or_end(post: impl FnOnce(Arc<AtomicBool>, Arc<AtomicBool>), limit: Duration) -> bool {
    let answer = Arc::new(AtomicBool::new(false));
    let mine = answer.clone();
    run_on_app(move |done| post(done, mine), limit) && answer.load(Ordering::Acquire)
}

/// The longest a session end waits for the open question to close before the write-out.
pub const QUESTION_CLOSE_LIMIT: Duration = Duration::from_millis(500);


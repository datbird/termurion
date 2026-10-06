//! WM_QUERYENDSESSION and WM_ENDSESSION on a terminal window (`iTermController.m:1746-1751`): a logoff, restart or
//! shut down ends the hosted shells before Windows ends the process. A window subclass runs
//! the closure inside the window procedure, so the work is done before the process can end.
//! HWND_MESSAGE windows get no broadcast; the subclass goes on every terminal window, and the
//! closure it runs is one app-wide setting (the keys decide it, not the current window).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use std::collections::HashSet;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::Shutdown::ShutdownBlockReasonDestroy;
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{DispatchMessageW, MsgWaitForMultipleObjects, PeekMessageW, TranslateMessage, MSG, PM_REMOVE, QS_ALLINPUT, WM_ENDSESSION, WM_NCDESTROY, WM_QUERYENDSESSION};

const SUBCLASS_ID: usize = 0x7464_6C67;

/// The time a logoff gives the app thread to flush logs and archives and end the shells (the
/// flushes and the kill wait are bounded inside it: 2 s + 0.5 s + 2 s).
pub(super) const LOGOFF_LIMIT: Duration = Duration::from_secs(6);

type OnEnd = Arc<dyn Fn() + Send + Sync>;


/// What a WM_QUERYENDSESSION asks: true when Windows may end the session now.
type QueryFn = Arc<dyn Fn(isize) -> bool + Send + Sync>;

/// The time a shutdown query gives the app thread to decide (a timeout means "ask first").
pub(super) const QUERY_LIMIT: Duration = Duration::from_secs(1);

/// Asks the app whether the session may end now: `post` hands over the done flag and the
/// answer flag (true = end now). An app that does not answer in `limit` counts as "ask".
pub(super) fn ask_or_end(post: impl FnOnce(Arc<AtomicBool>, Arc<AtomicBool>), limit: Duration) -> bool {
    let answer = Arc::new(AtomicBool::new(false));
    let mine = answer.clone();
    run_on_app(move |done| post(done, mine), limit) && answer.load(Ordering::Acquire)
}

/// What a logoff runs: set while the keys ask for it, None otherwise. One for the whole app.
fn armed() -> &'static Mutex<Option<OnEnd>> {
    static A: OnceLock<Mutex<Option<OnEnd>>> = OnceLock::new();
    A.get_or_init(Default::default)
}

/// Every window carrying the subclass.
fn windows() -> &'static Mutex<HashSet<isize>> {
    static W: OnceLock<Mutex<HashSet<isize>>> = OnceLock::new();
    W.get_or_init(Default::default)
}

/// True from the first window's WM_ENDSESSION until a cancelled logoff: every window gets the
/// message, the work is done once.
static RAN: AtomicBool = AtomicBool::new(false);

/// What a shutdown query asks: set once by the app.
fn query() -> &'static Mutex<Option<QueryFn>> {
    static Q: OnceLock<Mutex<Option<QueryFn>>> = OnceLock::new();
    Q.get_or_init(Default::default)
}

/// Sets (or clears) what a shutdown query asks.
pub(super) fn arm_query(q: Option<QueryFn>) {
    if let Ok(mut s) = query().lock() {
        *s = q;
    }
}

/// Sets (or, with None, clears) what a logoff runs, for every window.
pub(super) fn arm(on_end: Option<OnEnd>) {
    RAN.store(false, Ordering::SeqCst);
    if let Ok(mut a) = armed().lock() {
        *a = on_end;
    }
}

/// Puts the subclass on `hwnd`, once per window.
pub(super) fn watch(hwnd: isize) {
    let Ok(mut w) = windows().lock() else { return };
    if w.contains(&hwnd) {
        return;
    }
    // SAFETY: `hwnd` is a window of this thread; the proc removes itself at WM_NCDESTROY.
    if unsafe { SetWindowSubclass(hwnd as HWND, Some(proc), SUBCLASS_ID, 0) } != 0 {
        w.insert(hwnd);
    }
}

/// How many windows are watched (the test reads it).
#[cfg(test)]
pub(super) fn watched() -> usize {
    windows().lock().map_or(0, |w| w.len())
}

/// Runs work on the app thread from inside the window procedure: `post` hands the flag to the
/// app (an event the app answers by doing the work and setting the flag), and this thread
/// handles its own messages until the flag is set, at most `limit`, so the event is delivered
/// while the window procedure waits. True when the app finished in time.
pub(super) fn run_on_app(post: impl FnOnce(Arc<AtomicBool>), limit: Duration) -> bool {
    let done = Arc::new(AtomicBool::new(false));
    post(done.clone());
    let end = Instant::now() + limit;
    while !done.load(Ordering::Acquire) {
        if Instant::now() >= end {
            return false;
        }
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        // SAFETY: the standard message pump of this thread.
        if unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        } else {
            unsafe { MsgWaitForMultipleObjects(0, std::ptr::null(), 0, 10, QS_ALLINPUT) };
        }
    }
    true
}

impl super::App {
    /// Keeps the app-wide end-of-session watch in step with the settings. The watch is always
    /// on (a shutdown asks as a quit would, and WM_ENDSESSION saves the state, Task 8); what it
    /// does to the shells follows the keys: with `KillSessionsOnLogout` and `runJobsInServers` on
    /// and a host running, WM_ENDSESSION also ends every hosted shell (`killRestorableSessions`,
    /// `IC:1463-1472`), otherwise the shells are left as Quit leaves them. A window only gets the
    /// subclass (`watch`, from where its display watch goes on).
    pub(super) fn tick_logout(&mut self) {
        let want = self.host.is_some() && crate::session_end::kill_on_logout(self.globals.bool("KillSessionsOnLogout"), self.globals.bool("runJobsInServers"), true);
        if want != self.logout_armed || !self.logout_set {
            self.logout_armed = want;
            self.logout_set = true;
            let host = if want { self.host.clone() } else { None };
            let proxy = self.proxy.clone();
            arm(Some(Arc::new(move || {
                let sent = |done| {
                    let _ = proxy.lock().map(|p| p.send_event(super::UserEvent::Logoff(done)));
                };
                if !run_on_app(sent, LOGOFF_LIMIT) {
                    // The app thread was busy: the shells still end when the keys say so.
                    if let Some(host) = &host {
                        host.kill_all_and_wait(Duration::from_secs(2));
                    }
                }
            }) as OnEnd));
        }
        if !self.query_set {
            self.query_set = true;
            let proxy = self.proxy.clone();
            arm_query(Some(Arc::new(move |hwnd| {
                let post = |done, answer| {
                    let _ = proxy.lock().map(|p| p.send_event(super::UserEvent::ShutdownQuery { done, end_now: answer, hwnd }));
                };
                ask_or_end(post, QUERY_LIMIT)
            })));
        }
    }

    /// The app's part of a session end (`UserEvent::Logoff`, the one end-of-session event): what
    /// Quit writes out (settings, state, notes, window frames, logs and archives), the block
    /// reason removed, then the shells end when the keys say so and are left as Quit leaves them
    /// otherwise; the whole of it bounded to fit Windows' end-session time limit. Nothing is asked.
    pub(super) fn logoff_now(&mut self, done: &AtomicBool) {
        self.save_pending_settings();
        self.save_state_for_quit();
        self.flush_notes();
        let keep = !self.logout_armed && self.keep_jobs_on_quit();
        self.save_all_window_frames();
        self.destroy_block_reason();
        if self.logout_armed {
            self.finish_archives(true, Duration::from_secs(2));
            self.close_all_logs();
            if let Some(host) = &self.host {
                host.kill_all_and_wait(Duration::from_secs(2));
            }
        } else {
            self.finish_archives(!keep, Duration::from_secs(2));
            self.close_all_logs();
            self.leave_host(keep);
        }
        super::teardown_process();
        done.store(true, Ordering::Release);
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    if msg == WM_QUERYENDSESSION {
        // A new attempt. The app decides (a pure read, bounded by `QUERY_LIMIT`); the question
        // itself is shown after this returns, never inside the procedure.
        RAN.store(false, Ordering::SeqCst);
        let q = query().lock().ok().and_then(|q| q.clone());
        if let Some(q) = q {
            return LRESULT::from(q(hwnd as isize));
        }
    } else if msg == WM_ENDSESSION && wparam != 0 {
        // Run it here: Windows may end the process as soon as this returns. The lock is not held
        // while it runs.
        let on_end = armed().lock().ok().and_then(|a| a.clone());
        if let Some(f) = on_end {
            if !RAN.swap(true, Ordering::SeqCst) {
                f();
            }
        }
    } else if msg == WM_ENDSESSION {
        // A cancelled logoff: the next one runs again, and no block reason stays standing.
        RAN.store(false, Ordering::SeqCst);
        // SAFETY: idempotent; fails harmlessly when this window has no reason standing.
        unsafe { ShutdownBlockReasonDestroy(hwnd) };
    } else if msg == WM_NCDESTROY {
        // SAFETY: as above; a reason never outlives its window.
        unsafe { ShutdownBlockReasonDestroy(hwnd) };
        // SAFETY: removing the subclass this proc was installed as.
        unsafe { RemoveWindowSubclass(hwnd, Some(proc), SUBCLASS_ID) };
        if let Ok(mut w) = windows().lock() {
            w.remove(&(hwnd as isize));
        }
    }
    // SAFETY: forwarding the message the system delivered, unchanged.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, PostMessageW, SendMessageW, HWND_MESSAGE};

    /// The watch is one global: tests that touch it run one at a time.
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static L: Mutex<()> = Mutex::new(());
        L.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn message_window() -> HWND {
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        unsafe { CreateWindowExW(0, class.as_ptr(), class.as_ptr(), 0, 0, 0, 0, 0, HWND_MESSAGE, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null()) }
    }

    fn counter() -> (Arc<AtomicUsize>, OnEnd) {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        (calls, Arc::new(move || {
            c.fetch_add(1, Ordering::SeqCst);
        }))
    }

    #[test]
    fn logout_watch_is_installed_once() {
        let _g = serial();
        let hwnd = message_window();
        assert!(!hwnd.is_null());
        let before = watched();
        let (calls, on_end) = counter();
        arm(Some(on_end));
        watch(hwnd as isize);
        watch(hwnd as isize);
        assert_eq!(watched(), before + 1, "a window is watched once");
        unsafe { SendMessageW(hwnd, WM_ENDSESSION, 1, 0) };
        assert_eq!(calls.load(Ordering::SeqCst), 1, "wParam nonzero ends the shells");
        unsafe { SendMessageW(hwnd, WM_ENDSESSION, 0, 0) };
        assert_eq!(calls.load(Ordering::SeqCst), 1, "wParam zero is a cancelled logoff");
        unsafe { SendMessageW(hwnd, WM_ENDSESSION, 1, 0) };
        assert_eq!(calls.load(Ordering::SeqCst), 2, "a logoff after a cancelled one runs again");
        arm(None);
        unsafe { SendMessageW(hwnd, WM_ENDSESSION, 0, 0) };
        unsafe { SendMessageW(hwnd, WM_ENDSESSION, 1, 0) };
        assert_eq!(calls.load(Ordering::SeqCst), 2, "a cleared watch does nothing");
        unsafe { DestroyWindow(hwnd) };
        assert_eq!(watched(), before, "the watch goes with the window");
    }

    /// The watch is app-wide: the keys turn it off for every window, not for the one that
    /// happened to be current, and several windows get the message but the work is done once.
    #[test]
    fn logout_key_off_stops_every_window() {
        let _g = serial();
        let (a, b) = (message_window(), message_window());
        assert!(!a.is_null() && !b.is_null());
        let (calls, on_end) = counter();
        arm(Some(on_end));
        watch(a as isize);
        watch(b as isize);
        unsafe {
            SendMessageW(a, WM_ENDSESSION, 1, 0);
            SendMessageW(b, WM_ENDSESSION, 1, 0);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1, "two windows, one logoff, one run");
        unsafe { SendMessageW(a, WM_ENDSESSION, 0, 0) };
        // The key goes off while window A is current.
        arm(None);
        unsafe { SendMessageW(b, WM_ENDSESSION, 1, 0) };
        assert_eq!(calls.load(Ordering::SeqCst), 1, "the key is off: no window ends the shells");
        unsafe {
            DestroyWindow(a);
            DestroyWindow(b);
        }
    }

    /// A logoff hands the work to the app and waits for it, handling messages meanwhile; what
    /// never finishes costs the limit, not more.
    #[test]
    fn a_logoff_waits_for_the_app_and_pumps_messages() {
        let _g = serial();
        let hwnd = message_window();
        let (calls, on_end) = counter();
        // The "app" answers from a message it is posted: only a pumping wait delivers it.
        let answered = Arc::new(AtomicBool::new(false));
        let flag = answered.clone();
        let finishing: OnEnd = Arc::new(move || {
            on_end();
            flag.store(true, Ordering::SeqCst);
        });
        arm(Some(finishing));
        watch(hwnd as isize);
        let ok = run_on_app(
            |done| {
                let post_done = answered.clone();
                std::thread::spawn(move || {
                    while !post_done.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    done.store(true, Ordering::Release);
                });
                unsafe { PostMessageW(hwnd, WM_ENDSESSION, 1, 0) };
            },
            Duration::from_secs(5),
        );
        assert!(ok, "the posted message was delivered while waiting, and the app finished");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let t = Instant::now();
        assert!(!run_on_app(|_| {}, Duration::from_millis(200)), "an app that never answers is reported");
        assert!(t.elapsed() < Duration::from_secs(2), "and the wait is bounded");
        arm(None);
        unsafe { DestroyWindow(hwnd) };
    }

    /// A shutdown query the app does not answer in time is a question, never a silent end.
    #[test]
    fn query_endsession_times_out_to_ask() {
        let t = Instant::now();
        assert!(!ask_or_end(|_done, _end| {}, Duration::from_millis(100)), "no answer: ask first");
        assert!(t.elapsed() < Duration::from_secs(2), "bounded");
        let say = |end: bool| {
            move |done: Arc<AtomicBool>, answer: Arc<AtomicBool>| {
                answer.store(end, Ordering::SeqCst);
                done.store(true, Ordering::Release);
            }
        };
        assert!(ask_or_end(say(true), Duration::from_secs(1)), "the app said end now");
        assert!(!ask_or_end(say(false), Duration::from_secs(1)), "the app said ask first");
        // The window procedure returns the answer (FALSE blocks the shutdown).
        let _g = serial();
        let hwnd = message_window();
        watch(hwnd as isize);
        arm_query(Some(Arc::new(|_| false)));
        assert_eq!(unsafe { SendMessageW(hwnd, WM_QUERYENDSESSION, 0, 0) }, 0, "ask first returns FALSE");
        arm_query(Some(Arc::new(|_| true)));
        assert_eq!(unsafe { SendMessageW(hwnd, WM_QUERYENDSESSION, 0, 0) }, 1, "end now returns TRUE");
        arm_query(None);
        assert_eq!(unsafe { SendMessageW(hwnd, WM_QUERYENDSESSION, 0, 0) }, 1, "no watch: Windows' default");
        unsafe { DestroyWindow(hwnd) };
    }

    /// B8: the resident window and a second window share the one subclass; one logoff, one run,
    /// and the query never runs the end-of-session work.
    #[test]
    fn logoff_runs_once_with_resident_and_a_second_window() {
        let _g = serial();
        let (resident, other) = (message_window(), message_window());
        let (calls, on_end) = counter();
        arm(Some(on_end));
        arm_query(Some(Arc::new(|_| true)));
        watch(resident as isize);
        watch(other as isize);
        unsafe {
            SendMessageW(resident, WM_QUERYENDSESSION, 0, 0);
            SendMessageW(other, WM_QUERYENDSESSION, 0, 0);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0, "a query ends nothing");
        unsafe {
            SendMessageW(resident, WM_ENDSESSION, 1, 0);
            SendMessageW(other, WM_ENDSESSION, 1, 0);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1, "one logoff, one run");
        // The app side has one end-of-session event: no second subclass, no ShutdownEnded.
        let whole = include_str!("mod.rs");
        assert!(!whole.contains("ShutdownEnded") && !whole.contains("install_shutdown_hook"), "one subclass, one Logoff event (B8)");
        arm(None);
        arm_query(None);
        unsafe {
            DestroyWindow(resident);
            DestroyWindow(other);
        }
    }

    /// Every window that is watched for the display is watched for the end of the session, the
    /// hidden resident one included (a source check: the watch no longer waits for the keys).
    #[test]
    fn every_window_gets_the_session_watch() {
        for (name, src) in [("mod.rs", include_str!("mod.rs")), ("transparency_ui.rs", include_str!("transparency_ui.rs")), ("windows_ui.rs", include_str!("windows_ui.rs"))] {
            let code = src;
            let displays = code.matches("display_watch::watch(").count();
            let sessions = code.matches("logout_ui::watch(").count();
            assert!(displays > 0 && sessions >= displays, "{name}: {displays} display watches, {sessions} session watches");
        }
    }

    /// The app's part of a logoff does what Quit does before it ends the shells, in that order
    /// (a source check: the handler needs a running app).
    #[test]
    fn a_logoff_writes_what_quit_writes_before_ending_the_shells() {
        let src = include_str!("logout_ui.rs").split("#[cfg(test)]\nmod tests").next().unwrap();
        let from = src.find("pub(super) fn logoff_now(").unwrap();
        let body = &src[from..];
        let (a, l, k) = (body.find("self.finish_archives(true").unwrap(), body.find("self.close_all_logs()").unwrap(), body.find("kill_all_and_wait").unwrap());
        assert!(a < l && l < k, "archives, then logs, then the kill, which is waited for");
        let whole = include_str!("mod.rs");
        let at = whole.find("UserEvent::Logoff(done) =>").expect("a Logoff arm in user_event");
        assert!(whole[at..].chars().take(200).collect::<String>().contains("logoff_now"), "UserEvent::Logoff calls logoff_now");
    }
}

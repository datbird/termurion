//! The app's part of a session end: what Quit writes out, then the shells. The window procedure,
//! the block reason and the quit question's thread are the platform's (`platform::logout`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::platform::logout::{arm, arm_query, ask_or_end, end_question, run_on_app, ARCHIVE_LIMIT, FALLBACK_KILL_LIMIT, KILL_LIMIT, LOGOFF_LIMIT, QUERY_LIMIT};

type OnEnd = Arc<dyn Fn() + Send + Sync>;

impl super::App {
    /// Keeps the app-wide end-of-session watch in step with the settings. The watch is always
    /// on (a shutdown asks as a quit would, and WM_ENDSESSION saves the state, Task 8); what it
    /// does to the shells follows the keys: with `KillSessionsOnLogout` and `runJobsInServers` on
    /// and a host running, WM_ENDSESSION also ends every hosted shell (`killRestorableSessions`,
    /// `IC:1463-1472`), otherwise the shells are left as Quit leaves them. A window only gets the
    /// subclass (`watch`, from where its display watch goes on).
    pub(super) fn tick_logout(&mut self) {
        let want = self.host.is_some() && crate::session_end::kill_on_logout(self.globals.bool("KillSessionsOnLogout"), self.globals.bool("RunJobsInServers"), true);
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
                        host.kill_all_and_wait(FALLBACK_KILL_LIMIT);
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
        end_question();
        self.destroy_block_reason();
        if self.logout_armed {
            self.finish_archives(true, ARCHIVE_LIMIT);
            self.close_all_logs();
            if let Some(host) = &self.host {
                host.kill_all_and_wait(KILL_LIMIT);
            }
        } else {
            self.finish_archives(!keep, ARCHIVE_LIMIT);
            self.close_all_logs();
            self.leave_host(keep);
        }
        super::teardown_process();
        done.store(true, Ordering::Release);
    }
}

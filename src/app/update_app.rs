//! Checking for updates in the app (M13v2 Task 2): the schedule, the permission question, the
//! Software Update window and what its buttons do. Notify only: nothing is downloaded.

use std::time::{Duration, Instant};

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::scripts_alerts::{ask, Ask, Then};
use super::App;
use crate::globals::Globals;
use crate::update::{self, FeedResult};
use crate::update_ui::{Choice, UpdateWindow, View};
use crate::warnings::Kind;

/// What a finished check shows (Review Focus 2, R13vw-1): None for nothing. No information (404,
/// 401) is silent in both kinds of check; a failure or an unparsable feed shows an error for a
/// manual check only; "up to date" and a newer running version are told to a manual check only;
/// an update is offered either way (the skipped one only to a manual check).
pub(super) fn decide(result: FeedResult, manual: bool, current: &str, skipped: Option<&str>, test: bool) -> Option<View> {
    match result {
        FeedResult::NoInfo => None,
        FeedResult::Error(_) => manual.then(|| View::Error(update::RETRIEVE_ERROR.to_string())),
        FeedResult::Body(body) => match update::parse_releases(&body) {
            Err(_) => manual.then(|| View::Error(update::PARSE_ERROR.to_string())),
            Ok(list) => match update::outcome(&list, test, current, skipped, manual) {
                update::Found::Update(r) => Some(View::Found(r)),
                update::Found::UpToDate { newest } => manual.then(|| View::UpToDate(newest, false)),
                update::Found::Newer { newest } => manual.then(|| View::UpToDate(newest, true)),
            },
        },
    }
}

/// A manual check that found no information closes its Checking window with nothing shown.
pub(super) fn closes_checking(result: &FeedResult, manual: bool) -> bool {
    manual && matches!(result, FeedResult::NoInfo)
}

/// What Learn More opens: the pinned releases page or a page under it, through the one guard.
pub(super) fn learn_more_target(url: &str) -> crate::semantic::Run {
    crate::semantic::guard_open(update::release_url(url).unwrap_or(update::RELEASE_PAGE), false)
}

/// Seconds until the next automatic check; None while automatic checks are not on (the raw
/// stored answer, so an unanswered question never checks).
pub(super) fn schedule_delay(g: &Globals, now: f64) -> Option<f64> {
    update::automatic_checks_on(g).then(|| update::delay_until_check(now, update::last_check_of(g), update::interval_of(g)))
}

/// A finished check, a failed one too (else a failing feed would re-check at every tick).
pub(super) fn store_check_time(g: &mut Globals, now: f64) {
    g.set(update::LAST_CHECK_KEY, toml::Value::Float(now));
}

/// Whether Termurion ran before (`SUHasLaunchedBefore`); this launch is marked either way.
pub(super) fn launched_before(g: &mut Globals) -> bool {
    let before = g.raw(update::LAUNCHED_KEY).and_then(|v| v.as_bool()) == Some(true);
    if !before {
        g.set(update::LAUNCHED_KEY, toml::Value::Boolean(true));
    }
    before
}

/// Sparkle's permission question: never silenced, closed with no answer.
pub(super) fn permission_ask() -> Ask {
    ask(update::PERMISSION_TITLE, &update::permission_text(), &[update::CHECK_AUTOMATICALLY, update::DONT_CHECK], None, Kind::Persistent, Then::UpdatePermission)
}

/// Whether a view waits instead of opening (the 13v1 merge): an automatic check's notice never
/// opens while Termurion is resident (only the notification area icon shows); it opens when a window
/// wakes. A manual check (the menu or the tray's Check For Updates) shows at once.
pub(super) fn notice_waits(resident: bool, manual: bool) -> bool {
    resident && !manual
}

/// A deadline as the idle loop sees it (the 13v1 merge, P12 of 13v1): a resident Termurion wakes for
/// no update or tip timer; the overdue one runs at the first pass after a window wakes.
pub(super) fn idle_due(due: Option<Instant>, resident: bool) -> Option<Instant> {
    due.filter(|_| !resident)
}

fn now_secs() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

impl App {
    fn update_checker_ready(&mut self) -> &crate::update_fetch_win::Checker {
        let notify = self.notify.clone();
        self.update_checker.get_or_insert_with(|| crate::update_fetch_win::Checker::start(Box::new(crate::update_fetch_win::CurlFetch), notify))
    }

    /// Once per launch, after the first window (`SUUpdater.m:193-240`): the permission question
    /// on the second launch, a schedule when checks are on, nothing on the first launch ever.
    pub(super) fn update_start(&mut self) {
        let before = launched_before(&mut self.globals);
        if !before {
            self.save_globals();
        }
        match update::start(update::automatic_of(&self.globals), before) {
            update::Start::Ask => self.ask_scripts(permission_ask()),
            update::Start::Schedule => self.update_schedule(),
            update::Start::Nothing => {}
        }
    }

    fn update_schedule(&mut self) {
        self.update_due = schedule_delay(&self.globals, now_secs()).map(|d| Instant::now() + Duration::from_secs_f64(d));
    }

    /// The permission question's answer: Check Automatically (0) or Don't Check (1).
    pub(super) fn update_permission_answered(&mut self, index: usize) {
        self.globals.set(update::AUTOMATIC_KEY, toml::Value::Boolean(index == 0));
        self.save_globals();
        self.update_schedule();
    }

    /// Check for Updates (the menu and the tray item) when `manual`: the window shows Checking
    /// at once. Otherwise the scheduled check, which shows nothing unless there is an update.
    pub(super) fn check_for_updates(&mut self, manual: bool) {
        if manual {
            if let Some(w) = self.update_window.as_ref().filter(|w| matches!(w.view(), View::Found(_))) {
                // An update is already on offer: that window comes forward.
                w.focus();
                return;
            }
            self.update_manual = true;
            self.update_show = Some(View::Checking);
            self.update_show_manual = true;
        }
        self.update_due = None;
        if !self.update_checker_ready().busy() {
            // Sparkle stores the time when the check starts (`SUUpdater.m:382`) and again when it
            // ends: a Cancel (or a quit) mid-check leaves the schedule pushed, not overdue.
            store_check_time(&mut self.globals, now_secs());
            self.save_globals();
            self.update_schedule();
            self.update_checker_ready().check();
        }
    }

    pub(super) fn update_tick(&mut self, event_loop: &ActiveEventLoop) {
        // A finished check.
        if let Some(fetched) = self.update_checker.as_ref().and_then(|c| c.try_result()) {
            let manual = std::mem::take(&mut self.update_manual);
            let result = update::classify(fetched);
            store_check_time(&mut self.globals, now_secs());
            self.save_globals();
            self.update_schedule();
            let skipped = update::skipped_of(&self.globals);
            let test = self.globals.bool("CheckTestRelease");
            let closes = closes_checking(&result, manual);
            match decide(result, manual, update::current_version(), skipped.as_deref(), test) {
                Some(view) => {
                    self.update_show = Some(view);
                    self.update_show_manual = manual;
                }
                None if closes || manual => {
                    self.update_window = None;
                    self.update_show = None;
                }
                None => {}
            }
        }
        // The schedule: a deadline, never a poll.
        if self.update_due.is_none() && self.update_checker.as_ref().is_none_or(|c| !c.busy()) {
            self.update_schedule();
        }
        // Never while resident: the overdue check runs when a window wakes.
        if !self.resident && self.update_due.is_some_and(|t| Instant::now() >= t) {
            self.update_due = None;
            if update::automatic_checks_on(&self.globals) {
                self.check_for_updates(false);
            }
        }
        // A view waiting to show (an automatic notice waits while resident).
        if self.update_window.is_none() && notice_waits(self.resident, self.update_show_manual) {
            return;
        }
        if let Some(view) = self.update_show.take() {
            if let Some(w) = self.update_window.as_mut() {
                w.set_view(view);
                w.focus();
            } else {
                let gpu = self.gpu.as_ref().map(|g| (&g.instance, &g.adapter, &g.device, &g.queue)).or(self.prefs_gpu.as_ref().map(|(i, a, d, q)| (i, a, d, q)));
                match gpu {
                    Some((i, a, d, q)) => match UpdateWindow::open(event_loop, i, a, d, q, view) {
                        Ok(w) => self.update_window = Some(w),
                        Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
                    },
                    None => self.update_show = Some(view),
                }
            }
        }
    }

    /// Events for the Software Update window. False for other windows.
    pub(super) fn update_window_event(&mut self, window: WindowId, event: &WindowEvent) -> bool {
        let Some(w) = self.update_window.as_mut().filter(|w| w.id() == window) else { return false };
        match event {
            WindowEvent::RedrawRequested => w.redraw(),
            other => {
                w.event(other);
            }
        }
        if let Some(choice) = w.take_choice() {
            let view = w.view().clone();
            self.update_choice(choice, view);
        }
        true
    }

    fn update_choice(&mut self, choice: Choice, view: View) {
        match (choice, view) {
            (Choice::Cancel, _) => {
                self.update_manual = false;
                if let Some(c) = &self.update_checker {
                    c.cancel();
                }
            }
            (Choice::Skip, View::Found(r)) => {
                self.globals.set(update::SKIPPED_KEY, toml::Value::String(r.version));
                self.save_globals();
            }
            (Choice::LearnMore, View::Found(r)) => self.learn_more(&r.url),
            _ => {}
        }
        self.update_window = None;
    }

    /// Learn More opens the release page and downloads nothing.
    fn learn_more(&mut self, url: &str) {
        match learn_more_target(url) {
            crate::semantic::Run::Open(target) => {
                if let Err(e) = super::semantic_ui::shell_open(&target, false) {
                    self.banners.set("action", format!("Could not open {target}: {e}"));
                }
            }
            crate::semantic::Run::Blocked(why) => self.banners.set("action", why),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    /// Final review 5: a manual check that decides nothing clears the pending Checking view too,
    /// so a GPU that arrives later does not open a Checking window with no check running.
    #[test]
    fn a_manual_check_that_decides_nothing_clears_the_checking_view() {
        let src = include_str!("update_app.rs");
        let arm = src.split("None if closes || manual =>").nth(1).expect("arm").split("None => {}").next().unwrap();
        assert!(arm.contains("self.update_show = None"), "{arm}");
    }

    use super::*;
    use crate::globals::Globals;
    use crate::update::{self, classify, FeedResult, Fetched};
    use crate::update_ui::View;
    use toml::Value;

    fn status(code: u16, body: &str) -> FeedResult {
        classify(Fetched::Status { code, body: body.into() })
    }

    const ONE: &str = r#"[{"tag_name":"v9.9.9","name":"Nine","body":"notes nine","html_url":"https://github.com/datbird/tandem/releases/tag/v9.9.9","prerelease":false,"draft":false}]"#;

    #[test]
    fn manual_and_background_outcomes() {
        let cur = "0.0.2";
        // 404 and 401 are no information: silent both ways, nothing shown, no "up to date".
        for r in [status(404, "{\"message\":\"Not Found\"}"), status(401, "{\"message\":\"Bad credentials\"}")] {
            assert_eq!(decide(r.clone(), true, cur, None, false), None);
            assert_eq!(decide(r.clone(), false, cur, None, false), None);
            assert!(closes_checking(&r, true), "a manual check's Checking window closes with nothing shown");
            assert!(!closes_checking(&r, false));
        }
        // Any other status or a transport failure: an error for a manual check only.
        let err = Some(View::Error(update::RETRIEVE_ERROR.into()));
        assert_eq!(decide(status(500, "oops"), true, cur, None, false), err);
        assert_eq!(decide(status(500, "oops"), false, cur, None, false), None);
        let transport = classify(Fetched::Transport("curl: (7) Failed to connect to api.github.com port 443".into()));
        assert_eq!(decide(transport.clone(), true, cur, None, false), err);
        assert_eq!(decide(transport, false, cur, None, false), None);
        assert!(!closes_checking(&status(500, "oops"), true), "an error is shown, not hidden");
        // Unparsable JSON.
        let parse = Some(View::Error(update::PARSE_ERROR.into()));
        assert_eq!(decide(status(200, "{"), true, cur, None, false), parse);
        assert_eq!(decide(status(200, "{"), false, cur, None, false), None);
        // A newer release is offered either way.
        for manual in [true, false] {
            match decide(status(200, ONE), manual, cur, None, false) {
                Some(View::Found(r)) => assert_eq!(r.version, "9.9.9"),
                other => panic!("{other:?}"),
            }
        }
        // The skipped version: offered by a manual check only.
        assert_eq!(decide(status(200, ONE), false, cur, Some("9.9.9"), false), None);
        assert!(matches!(decide(status(200, ONE), true, cur, Some("9.9.9"), false), Some(View::Found(_))));
        // Nothing newer.
        assert_eq!(decide(status(200, "[]"), true, cur, None, false), Some(View::UpToDate(cur.into(), false)));
        assert_eq!(decide(status(200, "[]"), false, cur, None, false), None);
        // Running a newer version than the newest release.
        assert_eq!(decide(status(200, ONE), true, "10.0.0", None, false), Some(View::UpToDate("9.9.9".into(), true)));
        assert_eq!(decide(status(200, ONE), false, "10.0.0", None, false), None);
        // A prerelease is offered only on the beta channel.
        let pre = ONE.replace("\"prerelease\":false", "\"prerelease\":true");
        assert_eq!(decide(status(200, &pre), true, cur, None, false), Some(View::UpToDate(cur.into(), false)));
        assert!(matches!(decide(status(200, &pre), true, cur, None, true), Some(View::Found(_))));
    }

    #[test]
    fn learn_more_opens_only_the_pinned_page() {
        use crate::semantic::Run;
        let page = update::RELEASE_PAGE;
        let opens = |url: &str| match learn_more_target(url) {
            Run::Open(u) => u,
            other => panic!("{url}: {other:?}"),
        };
        assert_eq!(opens(&format!("{page}/tag/v9.9.9")), format!("{page}/tag/v9.9.9"));
        for bad in ["https://github.com.evil.example/datbird/tandem/releases", "https://github.com@evil/..", "http://github.com/datbird/tandem/releases", "file:///C:/Windows/System32/calc.exe", "\\\\evil\\share\\x.exe", "https://github.com/datbird/tandem/releases/a b", "https://github.com/datbird/tandem/releases/%2e%2e/x", "ms-msdt:/id", ""] {
            assert_eq!(opens(bad), page, "{bad:?} opens the pinned page");
        }
    }

    #[test]
    fn schedule_follows_the_stored_answer() {
        let mut g = Globals::empty(std::path::Path::new("update-test.toml"));
        // Fresh settings: no answer, so nothing is scheduled although the row shows checked.
        assert_eq!(schedule_delay(&g, 1000.0), None);
        g.set(update::AUTOMATIC_KEY, Value::Boolean(false));
        assert_eq!(schedule_delay(&g, 1000.0), None);
        g.set(update::AUTOMATIC_KEY, Value::Boolean(true));
        assert_eq!(schedule_delay(&g, 1000.0), Some(0.0), "never checked: now");
        store_check_time(&mut g, 1000.0);
        assert_eq!(update::last_check_of(&g), Some(1000.0));
        assert_eq!(schedule_delay(&g, 1000.0 + 3600.0), Some(update::DEFAULT_INTERVAL - 3600.0));
        assert_eq!(schedule_delay(&g, 1000.0 + 200_000.0), Some(0.0), "overdue: at once");
    }

    #[test]
    fn the_question_shows_on_the_second_launch_only() {
        let mut g = Globals::empty(std::path::Path::new("update-test.toml"));
        assert!(!launched_before(&mut g), "the first launch ever");
        assert_eq!(g.raw(update::LAUNCHED_KEY).and_then(|v| v.as_bool()), Some(true), "and it is marked");
        assert!(launched_before(&mut g), "the second");
        assert_eq!(update::start(update::automatic_of(&g), true), update::Start::Ask);
    }

    #[test]
    fn the_permission_alert_is_never_silenced_and_closes_with_no_answer() {
        let a = permission_ask();
        assert_eq!(a.spec.id, None, "Sparkle's question is never silenceable");
        assert_eq!(a.spec.heading, update::PERMISSION_TITLE);
        assert_eq!(a.spec.labels, ["Check Automatically", "Don\u{2019}t Check"]);
        assert_eq!(super::super::scripts_alerts::on_close(&a.then, 0), super::super::scripts_alerts::Answer::Closed);
    }

    #[test]
    fn every_finished_check_stores_its_time_before_it_is_judged() {
        let src = include_str!("update_app.rs");
        let tick = src.split("fn update_tick").nth(1).and_then(|r| r.split("fn update_window_event").next()).unwrap();
        let stored = tick.find("store_check_time(").expect("the time is stored");
        assert!(stored < tick.find("decide(").unwrap(), "a failed or empty check is stored too");
        let choice = src.split("fn update_choice").nth(1).and_then(|r| r.split("fn learn_more").next()).unwrap();
        assert!(!choice.contains("store_check_time"), "Cancel itself stores nothing");
    }

    /// Sparkle stores the time when a check starts (`SUUpdater.m:382`) as well as when it ends, so
    /// a Cancel on an overdue check is not followed at once by the same request.
    #[test]
    fn a_check_stores_its_time_when_it_starts() {
        let src = include_str!("update_app.rs");
        let start = src.split("fn check_for_updates").nth(1).and_then(|r| r.split("fn update_tick").next()).unwrap();
        let stored = start.find("store_check_time(").expect("the time is stored at the start");
        assert!(stored < start.find(".check()").unwrap(), "stored before the request is sent");
    }

    /// The 13v1 merge (S8 d, 13v1 P12): an automatic check's notice waits while resident (a manual
    /// check from the tray shows at once), and a resident Termurion wakes for no update or tip timer.
    #[test]
    fn no_update_notice_or_timer_wake_while_resident() {
        assert!(notice_waits(true, false), "an automatic notice waits while resident");
        assert!(!notice_waits(true, true), "the tray's Check For Updates shows");
        assert!(!notice_waits(false, false));
        let soon = Some(Instant::now() + Duration::from_secs(5));
        assert_eq!(idle_due(soon, true), None, "no wake while resident");
        assert_eq!(idle_due(soon, false), soon);
        let src = include_str!("update_app.rs").split("#[cfg(test)]").next().unwrap().to_string();
        let tick = src.split("pub(super) fn update_tick(").nth(1).and_then(|r| r.split("\n    }\n").next()).unwrap();
        assert!(tick.contains("if !self.resident && self.update_due.is_some_and("), "no scheduled check while resident");
        let gate = tick.find("notice_waits(self.resident, self.update_show_manual)").expect("the notice gate");
        assert!(gate < tick.find("UpdateWindow::open(").unwrap(), "the gate comes before any window opens");
        let m = std::fs::read_to_string(format!("{}/src/app/mod.rs", env!("CARGO_MANIFEST_DIR"))).unwrap();
        assert!(m.contains("let (update_wake, tip_of_day_wake) = (update_app::idle_due(self.update_due, self.resident), update_app::idle_due(self.tip_due, self.resident));"));
        let wake = m.split("let wake = [").nth(1).and_then(|r| r.split("].into_iter()").next()).unwrap();
        assert!(wake.contains("update_wake, tip_of_day_wake"), "the wake array takes the resident-aware deadlines");
        assert!(!wake.contains(" self.update_due,") && !wake.contains(" self.tip_due,"), "no bare update or tip deadline in the wake array");
    }

}

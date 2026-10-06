//! The Tip of the Day in the app (M13v2 Task 4): the launch rules, the schedule, the card and
//! what its buttons do (`iTermTipController.m`, `iTermLaunchExperienceController.m`). One deadline
//! (`tip_due`), never a poll; the card never takes the focus when it opens.

use std::time::{Duration, Instant};

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::scripts_alerts::{ask, Ask, Then};
use super::App;
use crate::globals::Globals;
use crate::tip_ui::{self, Action, TipWindow, Work};
use crate::tips::{self, Launch, Tip, Try};
use crate::warnings::Kind;

pub(super) const PERMISSION_TITLE: &str = "See Tips of the Day?";
pub(super) const PERMISSION_TEXT: &str = "Tandem can show you a Tip of the Day message to help you learn about its many features. Are you interested?";

/// What the launch does about tips.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Step {
    /// Ask "See Tips of the Day?", then start.
    Ask,
    /// Start; no question may be asked.
    Start,
}

fn quell(g: &mut Globals, now: f64, days: f64) {
    g.set(tips::NEXT_ANNOYANCE_KEY, toml::Value::Float(now + days * tips::DAY));
}

fn number(g: &Globals, key: &str) -> Option<f64> {
    g.raw(key).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).filter(|v| v.is_finite())
}

/// The launch (`iTermLaunchExperienceController init`, `performStartupActivities`): counts the
/// launch, and on the first time the question could show, notes it and quells annoyances for two
/// days; asking quells for one.
pub(super) fn launch_step(g: &mut Globals, now: f64, automatic_checks_on: bool) -> Step {
    let count = g.raw(tips::RUN_COUNT_KEY).and_then(|v| v.as_integer()).unwrap_or(0).saturating_add(1);
    g.set(tips::RUN_COUNT_KEY, toml::Value::Integer(count));
    let quelled = number(g, tips::NEXT_ANNOYANCE_KEY).is_some_and(|until| until > now);
    let will_ask = tips::will_ask(tips::permission_of(g), g.bool(tips::DISABLED_KEY));
    match tips::launch_choice(count, quelled, will_ask, automatic_checks_on) {
        Launch::AskThenStart if g.raw(tips::ELIGIBILITY_KEY).is_none() => {
            g.set(tips::ELIGIBILITY_KEY, toml::Value::Float(now));
            quell(g, now, 2.0);
            Step::Start
        }
        Launch::AskThenStart => {
            quell(g, now, 1.0);
            Step::Ask
        }
        Launch::Start | Launch::Nothing => Step::Start,
    }
}

/// Seconds until the next try after a `Wait` or a shown tip: a full gap from now, as iTerm2's
/// `performSelector:afterDelay:` does, never `last + gap` (past when a card stays open).
pub(super) fn retry_in(between: f64) -> f64 {
    tips::between_clamped(between)
}

pub(super) fn permission_ask() -> Ask {
    ask(PERMISSION_TITLE, PERMISSION_TEXT, &["Yes", "No"], None, Kind::Persistent, Then::TipPermission)
}

fn now_secs() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

/// The work area of the monitor `hwnd` is on, else the first monitor's, else a common size.
fn work_area(hwnd: Option<isize>) -> Work {
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    if let Some(h) = hwnd {
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if unsafe { GetMonitorInfoW(MonitorFromWindow(h as _, MONITOR_DEFAULTTONEAREST), &mut info) } != 0 {
            let r = info.rcWork;
            return (r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32);
        }
    }
    super::requests_ui::work_areas().first().map_or((0.0, 0.0, 1920.0, 1040.0), |&(x, y, w, h)| (x as f32, y as f32, w as f32, h as f32))
}

impl App {
    fn tip_buttons(&self, tip: &Tip, expanded: bool) -> Vec<(&'static str, Action)> {
        let all = tips::all();
        let has_next = tips::next_key(all, Some(&tip.key), &[]).is_some();
        let has_prev = tips::prev_key(all, &tip.key).is_some();
        tip_ui::card_buttons(tip.url.is_some(), expanded, tips::frequency_high(tips::between_of(&self.globals)), self.globals.bool(tips::DISABLED_KEY), has_next, has_prev)
    }

    fn arm_tip(&mut self, secs: f64) {
        self.tip_due = Some(Instant::now() + Duration::from_secs_f64(secs.max(0.0)));
    }

    /// Once per launch, after the first window and the launch's own requests (`start`).
    pub(super) fn tips_launch(&mut self) {
        let automatic = crate::update::automatic_checks_on(&self.globals);
        let step = launch_step(&mut self.globals, now_secs(), automatic);
        self.save_globals();
        match step {
            Step::Ask => {
                self.ask_scripts(permission_ask());
                self.tips_start();
            }
            Step::Start => self.tips_start(),
        }
    }

    /// `startWithPermissionPromptAllowed:` after the question: with no permission yet, nothing
    /// starts (the answer starts it). A start waits a second for startup to settle.
    fn tips_start(&mut self) {
        if tips::will_ask(tips::permission_of(&self.globals), self.globals.bool(tips::DISABLED_KEY)) {
            return;
        }
        self.tips_started = true;
        self.arm_tip(1.0);
    }

    /// The question's answer: Yes (0) or No (1).
    pub(super) fn tip_permission_answered(&mut self, index: usize) {
        self.globals.set(tips::PERMISSION_KEY, toml::Value::Boolean(index == 0));
        self.save_globals();
        if index == 0 {
            self.tips_start();
        }
    }

    /// Tandem came to the front: try again soon (`applicationDidBecomeActive:`).
    pub(super) fn tips_became_active(&mut self) {
        if self.tips_started {
            self.arm_tip(0.0);
        }
    }

    pub(super) fn tips_tick(&mut self, event_loop: &ActiveEventLoop) {
        if self.tip_due.is_some_and(|t| Instant::now() >= t) {
            self.tip_due = None;
            self.try_tip(event_loop);
        }
    }

    /// `tryToShowTip` (`TC:136-160`).
    fn try_tip(&mut self, event_loop: &ActiveEventLoop) {
        let now = now_secs();
        let (last, between) = (tips::last_tip_time_of(&self.globals), tips::between_of(&self.globals));
        let showing = self.tip_window.is_some();
        let verdict = tips::try_show(tips::permission_of(&self.globals), self.globals.bool(tips::DISABLED_KEY), self.app_active, showing, tips::shown_recently(now, last, between));
        match verdict {
            // Not due to anyone now: the next activation tries again, so an idle Tandem is not woken.
            Try::Never => {}
            Try::Wait => self.arm_tip(retry_in(between)),
            Try::Show => {
                let seen = tips::seen_of(&self.globals);
                if let Some(key) = tips::next_key(tips::all(), None, &seen) {
                    self.show_tip(event_loop, &key);
                    self.arm_tip(retry_in(between));
                }
            }
        }
    }

    /// The menu item: the last tip seen, else the first (`showTip`, `TC:162-176`).
    pub(super) fn show_tip_from_menu(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(w) = &self.tip_window {
            w.focus();
            return;
        }
        let seen = tips::seen_of(&self.globals);
        if let Some(key) = tips::menu_key(tips::all(), &seen) {
            self.show_tip(event_loop, &key);
        }
    }

    /// `showTipForKey:`: opens the card and notes the time.
    fn show_tip(&mut self, event_loop: &ActiveEventLoop, key: &str) {
        let Some(tip) = tips::all().iter().find(|t| t.key == key).cloned() else { return };
        let gpu = self.gpu.as_ref().map(|g| (&g.instance, &g.adapter, &g.device, &g.queue)).or(self.prefs_gpu.as_ref().map(|(i, a, d, q)| (i, a, d, q)));
        let Some((i, a, d, q)) = gpu else { return };
        let work = work_area(self.hwnd());
        let offset = self.globals.raw(tips::OFFSET_KEY).and_then(|v| v.as_str()).and_then(tip_ui::parse_offset).unwrap_or((0.0, 0.0));
        let buttons = self.tip_buttons(&tip, false);
        match TipWindow::open(event_loop, i, a, d, q, &tip, buttons, work, offset) {
            Ok(w) => {
                self.tip_window = Some(w);
                self.globals.set(tips::LAST_TIP_KEY, toml::Value::Float(now_secs()));
                self.save_globals();
                self.sync_checks();
            }
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// Events for the card. False for other windows.
    pub(super) fn tip_window_event(&mut self, window: WindowId, event: &WindowEvent) -> bool {
        let Some(w) = self.tip_window.as_mut().filter(|w| w.id() == window) else { return false };
        match event {
            WindowEvent::RedrawRequested => w.redraw(),
            other => {
                w.event(other);
            }
        }
        let (action, moved) = (w.take_action(), w.moved_to());
        if let Some(offset) = moved {
            self.globals.set(tips::OFFSET_KEY, toml::Value::String(tip_ui::offset_string(offset)));
            self.save_globals();
        }
        if let Some(action) = action {
            self.tip_action(action);
        }
        true
    }

    /// The seen list with the card's tip last (`doNotShowCurrentTipAgain`).
    fn tip_seen(&mut self) {
        let Some(key) = self.tip_window.as_ref().map(|w| w.tip().key.clone()) else { return };
        let list = tips::seen(&tips::seen_of(&self.globals), &key);
        self.globals.set(tips::SEEN_KEY, toml::Value::Array(list.into_iter().map(toml::Value::String).collect()));
    }

    fn tip_close(&mut self) {
        self.tip_window = None;
        self.save_globals();
        self.sync_checks();
    }

    fn tip_refresh(&mut self) {
        let Some(w) = self.tip_window.as_ref() else { return };
        let buttons = self.tip_buttons(w.tip(), w.expanded());
        if let Some(w) = self.tip_window.as_mut() {
            w.set_buttons(buttons);
        }
    }

    fn tip_action(&mut self, action: Action) {
        let Some(tip) = self.tip_window.as_ref().map(|w| w.tip().clone()) else { return };
        match action {
            Action::LearnMore => {
                if let Some(url) = &tip.url {
                    self.tip_learn_more(url);
                }
                self.tip_seen();
                self.tip_close();
            }
            Action::Dismiss => {
                self.tip_seen();
                self.tip_close();
            }
            Action::ShowLater => self.tip_close(),
            Action::ToggleOptions => {
                if let Some(w) = self.tip_window.as_mut() {
                    w.toggle_expanded();
                }
                self.tip_refresh();
            }
            Action::Share => {
                if let Some(cb) = self.clipboard.as_mut() {
                    if let Err(e) = cb.set_text(tip_ui::share_text(&tip)) {
                        self.banners.set("action", format!("Could not copy the tip: {e}"));
                    }
                }
            }
            Action::ToggleFrequency => {
                let next = tips::toggled_frequency(tips::between_of(&self.globals));
                self.globals.set(tips::BETWEEN_KEY, toml::Value::Float(next));
                self.save_globals();
                self.tip_refresh();
            }
            Action::ToggleEnabled => {
                let disabled = self.globals.bool(tips::DISABLED_KEY);
                self.globals.set(tips::DISABLED_KEY, toml::Value::Boolean(!disabled));
                if disabled {
                    self.save_globals();
                    self.tip_refresh();
                } else {
                    self.tip_close();
                }
            }
            Action::Next | Action::Previous => {
                let all = tips::all();
                let key = if action == Action::Next { tips::next_key(all, Some(&tip.key), &[]) } else { tips::prev_key(all, &tip.key) };
                let Some(other) = key.and_then(|k| all.iter().find(|t| t.key == k)).cloned() else { return };
                self.tip_seen();
                let expanded = self.tip_window.as_ref().is_some_and(|w| w.expanded());
                let buttons = self.tip_buttons(&other, expanded);
                if let Some(w) = self.tip_window.as_mut() {
                    w.set_tip(&other, buttons);
                }
                self.globals.set(tips::LAST_TIP_KEY, toml::Value::Float(now_secs()));
                self.save_globals();
            }
        }
    }

    /// Learn More opens the tip's page through the one guard.
    fn tip_learn_more(&mut self, url: &str) {
        match crate::semantic::guard_open(url, false) {
            crate::semantic::Run::Open(target) => {
                if let Err(e) = super::semantic_ui::shell_open(&target, None, false) {
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
    use super::*;

    fn g() -> Globals {
        Globals::empty(std::path::Path::new("tips-test.toml"))
    }

    #[test]
    fn launch_follows_iterm2() {
        let mut g = g();
        let now = 1_000_000.0;
        // First launch: nothing asked yet, the first eligible moment is noted and quells two days.
        assert_eq!(launch_step(&mut g, now, true), Step::Start);
        assert_eq!(g.raw(tips::RUN_COUNT_KEY).and_then(|v| v.as_integer()), Some(1));
        assert_eq!(g.raw(tips::ELIGIBILITY_KEY).and_then(|v| v.as_float()), Some(now));
        assert_eq!(g.raw(tips::NEXT_ANNOYANCE_KEY).and_then(|v| v.as_float()), Some(now + 2.0 * tips::DAY));
        // Still quelled the next day: no question.
        assert_eq!(launch_step(&mut g, now + tips::DAY, true), Step::Start);
        // Past the quell: the question, and it quells for a day.
        let later = now + 3.0 * tips::DAY;
        assert_eq!(launch_step(&mut g, later, true), Step::Ask);
        assert_eq!(g.raw(tips::NEXT_ANNOYANCE_KEY).and_then(|v| v.as_float()), Some(later + tips::DAY));
        // Answered: the steady state starts with no question.
        g.set(tips::PERMISSION_KEY, toml::Value::Boolean(true));
        assert_eq!(launch_step(&mut g, later + 5.0 * tips::DAY, true), Step::Start);
    }

    #[test]
    fn the_update_question_takes_the_second_launch() {
        let mut g = g();
        g.set(tips::RUN_COUNT_KEY, toml::Value::Integer(1));
        // Run 2 with automatic checks off: Start (never asks) and no eligibility note.
        assert_eq!(launch_step(&mut g, 5.0, false), Step::Start);
        assert!(g.raw(tips::ELIGIBILITY_KEY).is_none());
    }

    /// A wait is always a full gap from now (`performSelector:afterDelay:`): a card left open
    /// for longer than the gap leaves `last + gap` in the past, and a deadline there would wake an
    /// idle app in a loop. Counts the wakes a simulated hour of waiting costs.
    #[test]
    fn retries_never_busy_loop() {
        let day = tips::DAY;
        assert_eq!(retry_in(day), day);
        assert_eq!(retry_in(f64::NAN), day);
        assert_eq!(retry_in(0.0), tips::MIN_BETWEEN);
        // A card open for three days with a one-day gap: every wait is a day, so 3 wakes, not a flood.
        let (mut now, mut wakes) = (0.0, 0);
        while now < 3.0 * day {
            now += retry_in(day);
            wakes += 1;
        }
        assert_eq!(wakes, 3);
        // And a short gap is the clamped minimum, never zero.
        assert!(retry_in(-5.0) >= tips::MIN_BETWEEN);
    }

    #[test]
    fn learn_more_goes_through_the_guard() {
        let src = include_str!("tips_app.rs");
        assert!(src.contains("crate::semantic::guard_open(url, false)"));
    }
}

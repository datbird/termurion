//! Trigger actions the window runs, the Instant-trigger timer, and Session > Triggers.

use std::time::{Duration, Instant};

use super::*;
use crate::trigger_eval::{TriggerPrefs, TriggerRequest};
use crate::triggers::{self, Trigger};

/// The least gap between two notification triggers in a pane, whatever
/// `UserNotificationTriggerRateLimit` says (B5/P18: a default of 0 is no flood guard).
const NOTIFY_FLOOR: Duration = Duration::from_millis(250);
const ALERT_BAR: &str = "trigger-alert";

/// `AlertTriggerRateLimit` and `UserNotificationTriggerRateLimit` (iTerm2: 1 s and 0 s), read
/// each time a trigger fires so a change applies at once.
pub(super) fn trigger_limits_from(globals: &crate::globals::Globals) -> (Duration, Duration) {
    let secs = |key: &str| crate::alerts::safe_secs(globals.number(key).unwrap_or(0.0), 0.0, 3600.0);
    (secs("AlertTriggerRateLimit"), secs("UserNotificationTriggerRateLimit"))
}

/// Whether a notification trigger may post now: its own limit, and the pane's floor.
fn notify_allowed(limits: &mut crate::triggers::RateLimit<(PaneId, usize)>, pane: PaneId, trigger: usize, now: Instant, limit: Duration) -> bool {
    // The trigger's own limit is asked first without recording, so a refusal there leaves the pane's floor unspent.
    limits.would_allow(&(pane, trigger), now, limit) && limits.allow((pane, usize::MAX), now, NOTIFY_FLOOR) && limits.allow((pane, trigger), now, limit)
}

/// A profile's triggers and switches.
pub(super) fn triggers_of(p: &crate::profile::Profile) -> (Vec<Trigger>, TriggerPrefs) {
    let list = p.get("Triggers").map(|v| triggers::parse_list(&v)).unwrap_or_default();
    let prefs = TriggerPrefs { interactive: p.bool("Enable Triggers in Interactive Apps"), interpolated: p.bool("Triggers Use Interpolated Strings"), ..TriggerPrefs::default() };
    (list, prefs)
}

/// The arguments cmd.exe gets for Run Command: the one builder, so a captured value cannot add a command.
fn command_args(cmd: &str) -> String {
    crate::semantic::cmd_args(cmd)
}

/// Runs a command for Run Command: hidden, through cmd.exe, output discarded.
fn start_command(cmd: &str) -> std::io::Result<std::process::Child> {
    let mut c = std::process::Command::new("cmd.exe");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.raw_arg(command_args(cmd));
    }
    c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c.spawn()
}

impl App {
    pub(super) fn trigger_request(&mut self, pane: PaneId, r: TriggerRequest) {
        // The action class only, never the matched text or what the action carries.
        let class = match &r {
            TriggerRequest::Alert { .. } => "alert",
            TriggerRequest::Notify { .. } => "notification",
            TriggerRequest::Bell => "bell",
            TriggerRequest::Bounce { .. } => "bounce",
            TriggerRequest::SendText(_) => "send",
            TriggerRequest::RunCommand(_) => "run",
            _ => "other",
        };
        crate::dlog!("trigger fired in pane {}: {}", pane, class);
        match r {
            TriggerRequest::Alert { trigger, text } => {
                let (alert_every, _) = trigger_limits_from(&self.globals);
                if self.trigger_limits.allow((pane, trigger), Instant::now(), alert_every) && self.ask(pane, ALERT_BAR, &text, &["OK", "Disable This Alert"], true) {
                    self.trigger_alerts.insert(pane, trigger);
                }
            }
            TriggerRequest::Notify { trigger, text } => {
                let (_, every) = trigger_limits_from(&self.globals);
                if notify_allowed(&mut self.trigger_limits, pane, trigger, Instant::now(), every) {
                    self.post_alert(pane, crate::alerts::AlertKind::Trigger(crate::alerts::safe_text(&text)));
                }
            }
            TriggerRequest::Bell => self.ring_bell(pane, 1),
            TriggerRequest::Bounce { until_active } => self.flash_taskbar(until_active),
            // The live session's own write: it reaches the shell during a replay too.
            TriggerRequest::SendText(text) => {
                if let Some(p) = self.panes.get_mut(&pane) {
                    p.live_input(text.as_bytes());
                }
            }
            TriggerRequest::RunCommand(cmd) => {
                if let Some(now) = self.trigger_pool.submit(cmd) {
                    self.run_trigger_command(pane, now);
                }
                let dropped = self.trigger_pool.take_dropped();
                if dropped > 0 {
                    self.tell_once(pane, "commands-dropped", "Too many Run Command triggers were waiting; the newest were not run.");
                }
            }
            TriggerRequest::SetTitle(title) => {
                if let Some(p) = self.panes.get_mut(&pane) {
                    p.own_mut().set_title(Some(title));
                }
                self.redraw();
            }
            TriggerRequest::TabStatus { status, dot, text } => {
                let word = match status.as_str() {
                    "work" => "Working",
                    "wait" => "Waiting",
                    "idle" => "Idle",
                    other => other,
                };
                let pairs = [("indicator", dot), ("status", word.to_string()), ("status-color", text)].map(|(k, v)| (k.to_string(), v));
                if let Some(p) = self.panes.get_mut(&pane) {
                    p.own_mut().apply_tab_status(&pairs);
                }
                self.redraw();
            }
            TriggerRequest::BufferInput(on) => {
                if let Some(p) = self.panes.get_mut(&pane) {
                    if on {
                        p.buffer.on = true;
                    } else {
                        let held = p.buffer.release();
                        p.live_input(&held);
                    }
                }
                self.redraw();
            }
            TriggerRequest::ProfileSetting { key, on } => {
                self.set_session_values(pane, &[(key, Some(toml::Value::Boolean(on)))]);
            }
            TriggerRequest::Coprocess { cmd, silent } => self.start_coprocess(pane, &cmd, silent, crate::coprocess::Entry::Trigger),
            TriggerRequest::Captured { needs_shell_integration } => self.capture_fired(pane, needs_shell_integration),
            // `triggerSideEffectOpenPasswordManagerToAccountName:`: opened on the next pass,
            // over the window of the session it matched in.
            TriggerRequest::OpenPasswordManager(value) => self.open_password_manager(crate::pwm_prompt::trigger_param(&value), Some(pane)),
            TriggerRequest::CapturedCleared => {
                self.tools_changed();
                self.redraw();
            }
            // `PS:25443-25452`: the session's scope with the trigger's values, no timeout.
            TriggerRequest::Invoke { invocation, captures, ranges, input, line } => {
                let frame = crate::script_eval::trigger_frame(&captures, &ranges, &input, line);
                let scope = super::script_ui::ScopeSpec::Session { pane, frame };
                self.invoke(&invocation, scope, crate::script_eval::Timeout::Never, true, super::script_ui::Origin::Report(super::script_ui::TRIGGER.into()));
            }
            TriggerRequest::ParameterError { trigger, text } => self.tell_once(pane, &format!("parameter-{trigger}"), &format!("Error in Trigger Parameter: {text}")),
            TriggerRequest::TooSlow => self.tell_once(pane, "too-slow", "Triggers are taking too long and were skipped for some output."),
            TriggerRequest::PatternTooSlow { trigger } => self.tell_once(pane, &format!("pattern-slow-{trigger}"), &format!("The pattern of trigger {} took too long to match, so the trigger was turned off for this session.", trigger + 1)),
            TriggerRequest::Unavailable { title, milestone, .. } => {
                let text = if milestone == 0 { format!("The trigger action \u{201c}{title}\u{201d} is not available in Tandem yet.") } else { format!("The trigger action \u{201c}{title}\u{201d} comes in milestone {milestone}.") };
                self.tell_once(pane, &title, &text);
            }
        }
    }

    /// A message shown once per pane and action.
    pub(super) fn tell_once(&mut self, pane: PaneId, key: &str, text: &str) {
        if self.trigger_told.insert((pane, key.to_string())) {
            self.ask(pane, &format!("trigger-told-{key}"), text, &["OK"], false);
        }
    }

    fn run_trigger_command(&mut self, pane: PaneId, cmd: String) {
        match start_command(&cmd) {
            Ok(child) => self.trigger_children.push((pane, cmd, child)),
            Err(e) => {
                self.ask(pane, "trigger-command", &format!("Run Command Trigger Failed: {cmd}: {e}"), &["OK"], false);
                if let Some(next) = self.trigger_pool.finished() {
                    self.run_trigger_command(pane, next);
                }
            }
        }
    }

    /// Bounce Dock Icon: the taskbar button flashes until Tandem is in front, or once.
    pub(super) fn flash_taskbar(&mut self, until_active: bool) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{FLASHW_ALL, FLASHW_TIMERNOFG, FLASHW_TRAY};
        if until_active { self.flash_taskbar_raw(FLASHW_TRAY | FLASHW_TIMERNOFG, 0) } else { self.flash_taskbar_raw(FLASHW_ALL, 1) }
    }

    /// The one `FlashWindowEx` call (B6): every taskbar flash (this trigger, RequestAttention)
    /// goes through the per-window floor; only `FLASHW_STOP` is never held back.
    pub(super) fn flash_taskbar_raw(&mut self, flags: u32, count: u32) {
        #[cfg(windows)]
        {
            use windows_sys::Win32::UI::WindowsAndMessaging::{FLASHW_STOP, FLASHWINFO, FlashWindowEx};
            let Some(hwnd) = self.hwnd() else { return };
            if flags != FLASHW_STOP && !self.bell_limiter.taskbar(hwnd, std::time::Instant::now()) {
                return;
            }
            let info = FLASHWINFO { cbSize: std::mem::size_of::<FLASHWINFO>() as u32, hwnd: hwnd as _, dwFlags: flags, uCount: count, dwTimeout: 0 };
            unsafe { FlashWindowEx(&info) };
        }
    }

    /// The bar answers for Show Alert: Disable This Alert turns that trigger off in this
    /// session only (iTerm2 does not save it). False for other bars.
    pub(super) fn trigger_bar(&mut self, pane: PaneId, id: &str, button: Option<&str>) -> bool {
        if id != ALERT_BAR {
            return false;
        }
        if let (Some("Disable This Alert"), Some(i)) = (button, self.trigger_alerts.remove(&pane)) {
            if let Some(p) = self.panes.get_mut(&pane) {
                p.own_mut().disable_trigger_here(i);
            }
        }
        true
    }

    /// Timer work: variables for interpolated strings, Instant triggers, and finished Run
    /// Command processes. When to come back.
    pub(super) fn tick_triggers(&mut self) -> Option<Instant> {
        self.handlers_finished();
        let now = Instant::now();
        let mut wake = None;
        let names: Vec<(PaneId, String, String)> = self
            .panes
            .iter()
            .map(|(id, p)| {
                let profile = self.store.get(&p.profile).map(|pr| pr.string("Name")).unwrap_or_default();
                let title = p.own().title().map(str::to_string).unwrap_or_else(|| profile.clone());
                (*id, title, profile)
            })
            .collect();
        for (id, title, profile) in names {
            let Some(p) = self.panes.get_mut(&id) else { continue };
            let s = p.own_mut();
            s.app_vars.insert("session.name".into(), title);
            s.app_vars.insert("session.profileName".into(), profile);
            let bells = s.bell_count.to_string();
            s.app_vars.insert("session.bellCount".into(), bells);
            if s.triggers.list.iter().any(|c| c.trigger.instant && c.trigger.enabled) {
                s.check_instant_triggers(now);
                wake = Some(now + Duration::from_millis(500));
            }
            // Idempotent mode rides the same timer (`performPeriodicTriggerCheck`).
            if s.idempotent_wake() {
                s.check_idempotent_triggers(now);
                wake = Some(wake.unwrap_or(now + Duration::from_millis(500)));
            }
            if s.has_timed_events() {
                s.check_event_timers(now);
                wake = Some(wake.map_or(now + Duration::from_secs(1), |w: Instant| w.min(now + Duration::from_secs(1))));
            }
        }
        self.take_requests();
        let mut done = Vec::new();
        self.trigger_children.retain_mut(|(pane, cmd, child)| match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    done.push((*pane, Some(format!("Run Command Trigger Failed: {cmd} ended with {status}."))));
                } else {
                    done.push((*pane, None));
                }
                false
            }
            Ok(None) => true,
            Err(e) => {
                done.push((*pane, Some(format!("Run Command Trigger Failed: {cmd}: {e}"))));
                false
            }
        });
        for (pane, failure) in done {
            if let Some(text) = failure {
                self.ask(pane, "trigger-command", &text, &["OK"], false);
            }
            if let Some(next) = self.trigger_pool.finished() {
                self.run_trigger_command(pane, next);
            }
        }
        if !self.trigger_children.is_empty() {
            wake = Some(wake.map_or(now + Duration::from_millis(250), |w: Instant| w.min(now + Duration::from_millis(250))));
        }
        wake
    }

    /// Session > Triggers commands. False for other selectors.
    pub(super) fn run_trigger_command_menu(&mut self, selector: &str, event_loop: &ActiveEventLoop) -> bool {
        let Some(id) = self.focused_id() else { return false };
        match selector {
            "addTrigger:" => {
                let text = self.panes.get(&id).and_then(|p| p.screen.selection_text()).unwrap_or_default();
                let Some(guid) = self.panes.get(&id).map(|p| self.store.shared_guid(&p.profile)) else { return true };
                if let Some(profile) = self.store.get_mut(&guid) {
                    let stored = profile.get("Triggers");
                    let mut list = stored.as_ref().map(triggers::parse_list).unwrap_or_default();
                    let line = text.lines().next().unwrap_or("").to_string();
                    list.push(Trigger { regex: triggers::escape_regex(&line), action: "BounceTrigger".into(), parameter: "0".into(), instant: false, enabled: true, name: line, job: String::new(), match_type: 0, extra: toml::Table::new() });
                    profile.set("Triggers", triggers::merge_list(stored.as_ref(), &list));
                    crate::settings_triggers::focus_last();
                }
                self.save_due = Some(Instant::now() + SAVE_DELAY);
                self.refresh_panes();
                self.open_settings(event_loop, Mode::Settings);
            }
            "editTriggers:" => {
                crate::settings_triggers::focus_first();
                self.open_settings(event_loop, Mode::Settings);
            }
            "toggleEnableTriggersInInteractiveApps:" => {
                let on = !self.panes.get(&id).is_some_and(|p| p.own().triggers.prefs.interactive);
                self.set_session_values(id, &[("Enable Triggers in Interactive Apps".into(), Some(toml::Value::Boolean(on)))]);
            }
            "toggleNotifyOnStatusChange:" => self.status_armed = !self.status_armed,
            "enableAllTriggers:" | "disableAllTriggers:" => {
                let on = selector == "enableAllTriggers:";
                // In place: entries Tandem cannot read, and every other key, stay as stored.
                if let Some(list) = self.profile_of(id).and_then(|p| p.get("Triggers")).and_then(|v| triggers::set_all_disabled(&v, !on)) {
                    self.set_session_values(id, &[("Triggers".into(), Some(list))]);
                }
            }
            _ => return false,
        }
        self.sync_checks();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T7 m3: a trigger refused by its own limit does not spend the pane's floor.
    #[test]
    fn a_refused_trigger_does_not_spend_the_pane_floor() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut l = crate::triggers::RateLimit::default();
        assert!(notify_allowed(&mut l, 1, 0, t0, Duration::from_secs(1)));
        assert!(!notify_allowed(&mut l, 1, 0, t0 + ms(300), Duration::from_secs(1)), "its own limit refuses");
        assert!(notify_allowed(&mut l, 1, 1, t0 + ms(400), Duration::from_secs(1)), "another trigger: the floor was last spent at t0, 400 ms ago");
    }

    #[test]
    fn trigger_limits_follow_iterm2() {
        let dir = std::env::temp_dir().join(format!("tandem-trigger-limits-{}", std::process::id()));
        let mut g = crate::globals::Globals::empty(&dir.join("settings.toml"));
        assert_eq!(trigger_limits_from(&g), (Duration::from_secs(1), Duration::ZERO));
        g.set("AlertTriggerRateLimit", toml::Value::Float(f64::NAN));
        g.set("UserNotificationTriggerRateLimit", toml::Value::Float(-4.0));
        let (a, n) = trigger_limits_from(&g);
        assert!(a <= Duration::from_secs(3600) && n == Duration::ZERO);
        let t0 = Instant::now();
        let mut l = crate::triggers::RateLimit::default();
        // Limit 0: the pane's floor still holds (P18), a quarter second later both post.
        assert!(notify_allowed(&mut l, 1, 0, t0, Duration::ZERO));
        assert!(!notify_allowed(&mut l, 1, 0, t0, Duration::ZERO), "two at once make one toast");
        assert!(notify_allowed(&mut l, 1, 0, t0 + Duration::from_millis(300), Duration::ZERO));
        // A 1 s limit on top: 300 ms later is still one.
        let mut l = crate::triggers::RateLimit::default();
        assert!(notify_allowed(&mut l, 1, 0, t0, Duration::from_secs(1)));
        assert!(!notify_allowed(&mut l, 1, 0, t0 + Duration::from_millis(300), Duration::from_secs(1)));
        assert!(notify_allowed(&mut l, 1, 0, t0 + Duration::from_millis(1100), Duration::from_secs(1)));
        // Another pane is its own.
        assert!(notify_allowed(&mut l, 2, 0, t0 + Duration::from_millis(1100), Duration::from_secs(1)));
        // An Alert keeps its per-trigger limit (the same table, its own key).
        let mut l = crate::triggers::RateLimit::default();
        assert!(l.allow((1, 5), t0, a) && !l.allow((1, 5), t0 + Duration::from_millis(500), a));
    }

    #[test]
    fn run_command_args_are_the_one_cmd_builder() {
        assert_eq!(command_args(r#"run "foo&calc""#), r#"/d /v:off /s /c "run "foo&calc"""#);
    }
}

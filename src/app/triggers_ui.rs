//! Trigger actions the window runs, the Instant-trigger timer, and Session > Triggers.

use std::time::{Duration, Instant};

use super::*;
use crate::trigger_eval::{TriggerPrefs, TriggerRequest};
use crate::triggers::{self, Trigger};

/// Show Alert: at most one per trigger per second (iTerm2's alertTriggerRateLimit).
const ALERT_EVERY: Duration = Duration::from_secs(1);
const ALERT_BAR: &str = "trigger-alert";

/// A profile's triggers and switches.
pub(super) fn triggers_of(p: &crate::profile::Profile) -> (Vec<Trigger>, TriggerPrefs) {
    let list = p.get("Triggers").map(|v| triggers::parse_list(&v)).unwrap_or_default();
    let prefs = TriggerPrefs { interactive: p.bool("Enable Triggers in Interactive Apps"), interpolated: p.bool("Triggers Use Interpolated Strings") };
    (list, prefs)
}

/// Runs a command for Run Command: hidden, through cmd.exe, output discarded.
fn start_command(cmd: &str) -> std::io::Result<std::process::Child> {
    let mut c = std::process::Command::new("cmd.exe");
    c.args(["/d", "/c", cmd]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
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
        match r {
            TriggerRequest::Alert { trigger, text } => {
                if self.trigger_limits.allow((pane, trigger), Instant::now(), ALERT_EVERY) && self.ask(pane, ALERT_BAR, &text, &["OK", "Disable This Alert"], true) {
                    self.trigger_alerts.insert(pane, trigger);
                }
            }
            TriggerRequest::Notify(text) => self.post_alert(pane, crate::alerts::AlertKind::Trigger(text)),
            TriggerRequest::Bell => self.ring_bell(pane),
            TriggerRequest::Bounce { until_active } => self.flash_taskbar(until_active),
            TriggerRequest::SendText(text) => {
                if let Some(p) = self.panes.get_mut(&pane) {
                    p.input(text.as_bytes());
                }
            }
            TriggerRequest::RunCommand(cmd) => {
                if let Some(now) = self.trigger_pool.submit(cmd) {
                    self.run_trigger_command(pane, now);
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
                        p.input(&held);
                    }
                }
                self.redraw();
            }
            TriggerRequest::ProfileSetting { key, on } => {
                self.set_session_values(pane, &[(key, Some(toml::Value::Boolean(on)))]);
            }
            TriggerRequest::Coprocess { cmd, silent } => self.start_coprocess(pane, &cmd, silent, crate::coprocess::Entry::Trigger),
            TriggerRequest::Captured { needs_shell_integration } => self.capture_fired(pane, needs_shell_integration),
            TriggerRequest::CapturedCleared => {
                self.tools_changed();
                self.redraw();
            }
            TriggerRequest::ParameterError { trigger, text } => self.tell_once(pane, &format!("parameter-{trigger}"), &format!("Error in Trigger Parameter: {text}")),
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
    fn flash_taskbar(&self, until_active: bool) {
        #[cfg(windows)]
        {
            use windows_sys::Win32::UI::WindowsAndMessaging::{FlashWindowEx, FLASHWINFO, FLASHW_ALL, FLASHW_TIMERNOFG, FLASHW_TRAY};
            let Some(hwnd) = self.hwnd() else { return };
            let info = FLASHWINFO {
                cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
                hwnd: hwnd as _,
                dwFlags: if until_active { FLASHW_TRAY | FLASHW_TIMERNOFG } else { FLASHW_ALL },
                uCount: if until_active { 0 } else { 1 },
                dwTimeout: 0,
            };
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
                let text = self.panes.get(&id).and_then(|p| p.screen.term().selection_to_string()).unwrap_or_default();
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
            "toggleNotifyOnStatusChange:" => self.notify_status_armed = !self.notify_status_armed,
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

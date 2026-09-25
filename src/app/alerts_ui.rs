//! Notifications in the window: which post, their texts, the click that reveals the session,
//! Session Ended, escape-sequence alerts, Notify on Status Change, and the idle code.

use std::time::{Duration, Instant};

use super::*;
use crate::alerts::{self, AlertKind, AlertPrefs};

/// Escape-sequence alerts: at most one a second per session.
const TERMINAL_EVERY: Duration = Duration::from_secs(1);
const STATUS_BAR: &str = "status-change";

impl App {
    fn alert_prefs(&self, pane: PaneId) -> AlertPrefs {
        let p = self.panes.get(&pane).and_then(|p| self.store.get(&p.profile));
        let b = |k: &str| p.is_some_and(|p| p.bool(k));
        AlertPrefs {
            enabled: b("BM Growl"),
            bell: b("Send Bell Alert"),
            idle: b("Send Idle Alert"),
            new_output: b("Send New Output Alert"),
            session_ended: b("Send Session Ended Alert"),
            terminal: b("Send Terminal Generated Alerts"),
            suppress_active: b("Suppress Alerts in Active Session"),
        }
    }

    /// A session's name (its title, else its profile's name) and its tab's number (from 1).
    pub(super) fn session_label(&self, pane: PaneId) -> (String, usize) {
        let name = self.panes.get(&pane).map(|p| p.own().title().map(str::to_string).unwrap_or_else(|| self.store.get(&p.profile).map(|pr| pr.string("Name")).unwrap_or_default())).unwrap_or_default();
        let tab = self.tabs.iter().position(|t| t.layout.panes().contains(&pane)).map_or(1, |i| i + 1);
        (name, tab)
    }

    /// Posts an alert when the profile and the user's view allow it.
    pub(super) fn post_alert(&mut self, pane: PaneId, kind: AlertKind) {
        let looking = self.looking_at(pane);
        if !alerts::should_post(&kind, self.alert_prefs(pane), looking, looking) {
            return;
        }
        let (name, tab) = self.session_label(pane);
        let (title, body) = alerts::text(&kind, &name, tab, self.globals.bool("simpleNotifications"), &self.globals.string("sessionEndMessageText"));
        match crate::notify::show_with(&title, &body, self.globals.bool("silentUserNotifications")) {
            Ok(()) => self.last_notified = Some(pane),
            Err(e) => self.banners.set("notify", format!("Could not show a notification: {e}")),
        }
    }

    /// OSC 9 and OSC 1337 Notification.
    pub(super) fn terminal_alert(&mut self, pane: PaneId, title: Option<String>, message: String) {
        if self.alert_limits.allow(pane, Instant::now(), TERMINAL_EVERY) {
            self.post_alert(pane, AlertKind::Terminal { title, message });
        }
    }

    /// A click on a notification: Tandem comes forward on the session it named.
    pub(super) fn reveal_notified(&mut self) {
        let Some(pane) = self.last_notified else { return };
        self.reveal_pane(pane);
    }

    /// Brings a pane to the front: its tab, the pane, the window.
    pub(super) fn reveal_pane(&mut self, pane: PaneId) {
        if let Some(i) = self.tabs.iter().position(|t| t.layout.panes().contains(&pane)) {
            self.active = i;
            self.tabs[i].focused = pane;
        }
        if let Some(g) = self.gpu.as_ref() {
            g.window.focus_window();
        }
        self.relayout();
    }

    /// Session Ended, for a pane whose process exited.
    pub(super) fn session_ended(&mut self, pane: PaneId) {
        self.post_alert(pane, AlertKind::SessionEnded);
    }

    /// Window > Notify on Status Change: armed, the next change of a session's status shows
    /// a bar, then it disarms.
    pub(super) fn tick_status_change(&mut self) {
        let now: Vec<(PaneId, Option<String>)> = self.panes.iter().map(|(id, p)| (*id, p.own().tab_status.status.clone())).collect();
        let mut changed = None;
        for (id, status) in &now {
            if let Some(before) = self.last_status.get(id) {
                if before != status && changed.is_none() {
                    changed = Some((*id, before.clone(), status.clone()));
                }
            }
        }
        self.last_status = now.into_iter().collect();
        if let (true, Some((pane, from, to))) = (self.notify_status_armed, changed) {
            self.notify_status_armed = false;
            let (name, _) = self.session_label(pane);
            let text = format!("Session status changed. {name} changed from \u{201c}{}\u{201d} to \u{201c}{}\u{201d}.", from.unwrap_or_else(|| "none".into()), to.unwrap_or_else(|| "none".into()));
            self.ask(pane, STATUS_BAR, &text, &["OK", "Reveal"], true);
            self.sync_checks();
        }
    }

    /// The status bar's answer. False for other bars.
    pub(super) fn status_bar(&mut self, pane: PaneId, id: &str, button: Option<&str>) -> bool {
        if id != STATUS_BAR {
            return false;
        }
        if button == Some("Reveal") {
            self.reveal_pane(pane);
        }
        true
    }

    /// "Periodically send an ASCII code when idle". When to come back.
    pub(super) fn tick_idle_codes(&mut self) -> Option<Instant> {
        let now = Instant::now();
        let mut wake: Option<Instant> = None;
        let due: Vec<(PaneId, u8, Duration)> = self
            .panes
            .iter()
            .filter_map(|(id, p)| {
                let pr = self.store.get(&p.profile)?;
                pr.bool("Send Code When Idle").then(|| (*id, pr.int("Idle Code").clamp(0, 255) as u8, Duration::from_secs_f64(pr.float("Idle Period").max(1.0))))
            })
            .collect();
        for (id, code, period) in due {
            let last_output = self.activity.get(&id).and_then(|a| a.last_output()).unwrap_or(self.started);
            let sent = self.idle_sent.get(&id).copied();
            if alerts::idle_code_due(last_output, sent, now, period) {
                if let Some(p) = self.panes.get_mut(&id) {
                    p.reply(&[code]);
                }
                self.idle_sent.insert(id, now);
            }
            let next = sent.map_or(last_output, |s| s.max(last_output)) + period;
            wake = Some(wake.map_or(next, |w| w.min(next)));
        }
        wake
    }
}

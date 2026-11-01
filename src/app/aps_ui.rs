//! Automatic Profile Switching in the window: each session's host, user, folder, and job
//! are scored against every profile's rules, and the session's profile follows.

use super::*;
use crate::aps::{Candidate, Facts};

pub(super) const LOG_KEY: &str = "NoSyncEnableAutomaticProfileSwitchingLogging";

/// This computer's full DNS name (iTerm2 starts sessions with the local host name, so host
/// rules work without shell integration).
pub(super) fn local_host() -> String {
    crate::platform::net::local_host()
}

impl App {
    fn rule_candidates(&self) -> Vec<Candidate> {
        self.store
            .profiles
            .iter()
            .map(|p| Candidate {
                guid: p.guid(),
                name: p.name(),
                rules: p.get("Bound Hosts").and_then(|v| v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())).unwrap_or_default(),
            })
            .collect()
    }

    /// Runs the rules for every session (a job, host, user, or folder may have changed; an
    /// unchanged session costs one comparison).
    pub(super) fn run_automatic_profile_switching(&mut self) {
        let candidates = self.rule_candidates();
        if candidates.iter().all(|c| c.rules.is_empty()) {
            return;
        }
        let ids: Vec<PaneId> = self.panes.keys().copied().collect();
        for id in ids {
            self.switch_if_rules_say(id, &candidates);
        }
    }

    fn switch_if_rules_say(&mut self, pane: PaneId, candidates: &[Candidate]) {
        let Some(p) = self.panes.get(&pane) else { return };
        let Some(profile) = self.store.get(&p.profile) else { return };
        if profile.bool("Prevent Automatic Profile Switching") {
            return;
        }
        let current = self.store.shared_guid(&p.profile);
        // The zoom to keep: measured against the original profile before the switch (`S:20830-20895`).
        let zoom = match (self.store.get(&p.profile), self.store.get(&current)) {
            (Some(now), Some(original)) => fonts_ui::zoom_delta(now, original, self.globals.bool("PreserveFontSizeOnAutomaticProfileSwitch")),
            _ => 0.0,
        };
        let s = &p.own().shell;
        let host = s.host.clone().unwrap_or_else(|| self.local_host.clone());
        let user = s.user.clone().unwrap_or_else(|| std::env::var("USERNAME").unwrap_or_default());
        let path = s.cwd.clone();
        let job = s.job.as_ref().map(|j| j.name.clone());
        let cmdline = s.job.as_ref().map(|j| j.cmdline.clone());
        let vars = self.variables(pane);
        // A rule expression that fails scores nothing; the failure is shown, not hidden.
        let failed = std::cell::RefCell::new(None);
        let expr = |e: &str| {
            let v = crate::interp::evaluate(&format!("\\({e})"), &|n| crate::prompted::variable(n.strip_prefix("session.").unwrap_or(n), &vars), &[]);
            match v.map(|v| v.trim().parse::<f64>().map_err(|_| format!("\u{201c}{v}\u{201d} is not a number"))) {
                Ok(Ok(n)) => n,
                Ok(Err(e2)) | Err(e2) => {
                    *failed.borrow_mut() = Some(format!("{{{e}}}: {e2}"));
                    f64::NEG_INFINITY
                }
            }
        };
        let facts = Facts { host: Some(&host), user: Some(&user), path: path.as_deref(), job: job.as_deref(), cmdline: cmdline.as_deref() };
        let switcher = self.aps.entry(pane).or_default();
        let decision = switcher.decide(&current, &facts, candidates, &expr);
        let stack = switcher.stack().join(" ");
        if let Some(e) = failed.into_inner() {
            self.banners.set("aps-expression", format!("An Automatic Profile Switching rule could not be scored: {e}"));
        }
        if self.globals.bool(LOG_KEY) {
            self.aps_log(pane, &format!("host={host} user={user} path={path:?} job={job:?} current={current} -> {decision:?}; stack: {stack}"));
        }
        let Some(guid) = decision else { return };
        let Some(name) = self.store.get(&guid).map(|p| p.name()) else {
            return self.banners.set("aps", format!("Can't switch to profile {guid}: it no longer exists."));
        };
        self.remarry(pane, &guid);
        // Re-applied after it, to the new profile's font, through the size change's own path.
        if zoom != 0.0 {
            self.write_font_size(pane, zoom);
            self.refresh_panes();
        }
        if self.globals.bool("ShowAutomaticProfileSwitchingBanner") {
            self.banners.set("aps", format!("Switched to profile \u{201c}{name}\u{201d}."));
        }
        self.redraw();
    }

    /// The Automatic Profile Switching debug log: the Script Console's "Automatic Profile
    /// Switching" entry, added when missing (`iTermAutomaticProfileSwitcher.m:20-35`).
    fn aps_log(&mut self, pane: PaneId, line: &str) {
        let title = self.session_title(pane);
        let stamp = self.history_stamp();
        self.script_history.ensure_aps(api_ui::unix_secs());
        self.script_history.add_log(crate::script_history::APS_ID, &crate::script_history::aps_line(&title, line), &stamp);
    }
}

//! Automatic Profile Switching in the window: each session's host, user, folder, and job
//! are scored against every profile's rules, and the session's profile follows.

use super::*;
use crate::aps::{Candidate, Facts, Switcher};

const LOG_KEY: &str = "NoSyncEnableAutomaticProfileSwitchingLogging";

/// This computer's full DNS name (iTerm2 starts sessions with the local host name, so host
/// rules work without shell integration).
pub(super) fn local_host() -> String {
    let mut buf = vec![0u16; 256];
    let mut len = buf.len() as u32;
    // ComputerNameDnsFullyQualified.
    let ok = unsafe { windows_sys::Win32::System::SystemInformation::GetComputerNameExW(3, buf.as_mut_ptr(), &mut len) };
    if ok == 0 {
        return std::env::var("COMPUTERNAME").unwrap_or_default();
    }
    String::from_utf16_lossy(&buf[..len as usize])
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
        let switcher = self.aps.entry(pane).or_insert_with(Switcher::default);
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
        if self.globals.bool("showAutomaticProfileSwitchingBanner") {
            self.banners.set("aps", format!("Switched to profile \u{201c}{name}\u{201d}."));
        }
        self.redraw();
    }

    /// The Automatic Profile Switching debug log (the Script Console comes in milestone 11):
    /// a file beside the settings.
    fn aps_log(&mut self, pane: PaneId, line: &str) {
        use std::io::Write;
        let path = crate::globals::default_path().with_file_name("automatic-profile-switching.log");
        let title = self.session_title(pane);
        let written = std::fs::OpenOptions::new().create(true).append(true).open(&path).and_then(|mut f| writeln!(f, "[{title} - session {pane}] {line}"));
        if let Err(e) = written {
            self.banners.set("aps", format!("Could not write {}: {e}", path.display()));
        }
    }
}

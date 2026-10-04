//! Session restoration in the window: the state saved at focus loss, on a debounce, and at
//! quit, and the windows rebuilt from it at launch.

use super::*;
use super::arrange_ui::Contents;
use crate::arrangement::Value;
use crate::restore_state::{self, State};

impl App {
    fn state_path() -> PathBuf {
        crate::globals::default_path().with_file_name("state")
    }

    /// A change a restored window would show: the next save comes after the quiet time.
    pub(super) fn mark_state_dirty(&mut self) {
        self.state_debounce.mark(Instant::now());
    }

    /// Saves every window with its contents when the debounce says so, or now with `force`.
    pub(super) fn save_state(&mut self, force: bool) {
        let now = Instant::now();
        if self.state_frozen || !(force || self.state_debounce.due(now)) {
            return;
        }
        let back = self.current_window();
        let windows = self.snapshot_all(self.restore_contents());
        if let Some(b) = back {
            self.enter(b);
        }
        let host = self.host_welcome.as_ref().filter(|_| self.host.is_some()).map(|w| w.instance.clone());
        self.write_state(&State { host, windows });
        self.state_debounce.written(now);
    }

    /// The last window closed: nothing comes back at the next launch (iTerm2 saves a state
    /// with no windows). Later saves are off.
    pub(super) fn save_empty_state(&mut self) {
        if !self.state_frozen {
            self.write_state(&State { host: None, windows: Vec::new() });
            self.state_frozen = true;
        }
    }

    /// Quit: the state as it is now, before any window closes; later saves are off.
    pub(super) fn save_state_for_quit(&mut self) {
        self.save_state(true);
        self.state_frozen = true;
    }

    /// Whether this Quit leaves the shells running in the host for the next launch.
    pub(super) fn keep_jobs_on_quit(&self) -> bool {
        let g = &self.globals;
        let system_policy = !g.bool("OpenArrangementAtStartup") && !g.bool("OpenNoWindowsAtStartup");
        self.host.is_some() && crate::close_rules::keep_jobs(true, g.bool("restoreWindowContents"), system_policy, g.bool("killJobsInServersOnQuit"))
    }

    /// Tandem is ending: with `keep`, every hosted shell stays in the host; otherwise the host
    /// ends them all. Either way the frames reach the host before Tandem exits.
    pub(super) fn leave_host(&mut self, keep: bool) {
        let Some(host) = self.host.take() else { return };
        if keep {
            for p in self.panes.values_mut() {
                p.detach();
            }
        } else {
            host.kill_all();
        }
        self.panes.clear();
        self.focus_order.clear();
        host.close();
    }

    fn restore_contents(&self) -> Contents {
        if self.globals.bool("restoreWindowContents") {
            Contents::Styled(self.globals.int("maxHistoryLinesToRestore").clamp(0, 10_000_000) as usize)
        } else {
            // No text, but the shells still come back to their panes after a crash.
            Contents::KeysOnly
        }
    }

    fn write_state(&mut self, s: &State) {
        let result = restore_state::encode(s).and_then(|bytes| {
            let path = Self::state_path();
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
            }
            restore_state::write_atomic(&path, &bytes)
        });
        match result {
            Ok(()) => self.banners.clear("state"),
            Err(e) => self.banners.set("state", format!("Could not save the windows for the next launch: {e}")),
        }
    }

    /// The windows saved at the last run, ready to rebuild: each restored session carries the
    /// banner line. None when nothing was saved; a bad file is kept as `state.bad` and a
    /// banner says why.
    pub(super) fn saved_windows(&mut self) -> Option<(Vec<Value>, Option<String>)> {
        let path = Self::state_path();
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => {
                self.banners.set("state", format!("Could not read the saved windows: {e}"));
                return None;
            }
        };
        let mut state = match restore_state::decode(&bytes) {
            Ok(s) => s,
            Err(e) => {
                let bad = path.with_file_name("state.bad");
                let kept = std::fs::rename(&path, &bad).map_or_else(|r| format!(" (it could not be moved aside: {r})"), |_| format!(" The file is kept as {}.", bad.display()));
                // Its own slot: a later successful save clears "state", not this.
                self.banners.set("state-bad", format!("The saved windows could not be restored: {e}.{kept}"));
                return None;
            }
        };
        if state.windows.is_empty() {
            return None;
        }
        // Only this launch-time read makes a saved chat reference adoptable.
        mark_adoptable(&mut state.windows);
        if self.globals.bool("showSessionRestoredBanner") {
            let line = restore_state::banner_text(&restore_date());
            for w in &mut state.windows {
                mark_banner(w, &line);
            }
        }
        Some((state.windows, state.host))
    }

    /// Matches the saved sessions against the host's running ones: a match is marked to take
    /// its shell back (no banner); ended sessions nobody claims are forgotten. Returns the
    /// running sessions nobody claims, as (id, pid).
    pub(super) fn plan_reattach(&mut self, windows: &mut [Value], saved_instance: Option<&str>) -> Vec<(u64, u32)> {
        let (Some(host), Some(welcome)) = (self.host.clone(), self.host_welcome.clone()) else { return Vec::new() };
        let mut sessions = Vec::new();
        for w in windows.iter_mut() {
            sessions_mut(w, &mut sessions);
        }
        let keys: Vec<Option<crate::reattach::HostKey>> = sessions.iter().map(|s| s.get(restore_state::HOST_KEY).and_then(parse_host_key)).collect();
        let plan = crate::reattach::plan(&keys, saved_instance, &welcome.instance, &welcome.sessions);
        for (i, id, since) in &plan.attach {
            let pid = keys[*i].as_ref().map_or(0, |k| k.pid);
            let mut t = toml::Table::new();
            t.insert("Id".into(), Value::Integer(*id as i64));
            t.insert("Pid".into(), Value::Integer(pid as i64));
            t.insert("Since".into(), Value::Integer(*since as i64));
            sessions[*i].insert(crate::arrangement::ATTACH_KEY.into(), Value::Table(t));
            // The job came back: no "Session Contents Restored" banner (iTerm2).
            sessions[*i].remove(crate::arrangement::BANNER_KEY);
        }
        for id in &plan.forget {
            host.forget(*id);
        }
        plan.orphans.iter().filter_map(|id| welcome.sessions.iter().find(|l| l.id == *id).map(|l| (l.id, l.pid))).collect()
    }

    /// Takes over a shell the host still runs, as a new pane. Its output from `since` follows
    /// the restored contents; when the host dropped part of it, the pane is repainted. The
    /// shell's saved `cookie` goes back in the jar as a Session cookie, so `ITERM2_COOKIE` in
    /// its environment still works (`PS:2243-2245`).
    pub(super) fn attach_pane(&mut self, rect: Rect, guid: Option<&str>, host_id: u64, pid: u32, since: u64, cookie: Option<String>) -> Result<PaneId, String> {
        let host = self.host.clone().ok_or("no session host is running")?;
        let (cols, rows) = self.grid_size(rect);
        let (pty, _from, dropped) = host.attach(host_id, pid, since, self.notify.clone())?;
        let id = self.next_id;
        self.next_id += 1;
        let p = self.profile(guid);
        let mut pane = Pane::attached(id, pty, cols, rows);
        if let Some(c) = &cookie {
            self.api_jar.lock().unwrap().add(c, crate::api_auth::CookieUse::Session);
        }
        pane.cookie = cookie;
        self.install_pane(id, pane, &p);
        if dropped {
            if let Some(pane) = self.panes.get_mut(&id) {
                // A visible mark where output is missing, then a repaint.
                let grays = restore_state::banner_grays(pane.screen.palette.bg);
                pane.screen.feed_restored("", None, Some(("Output was cut short while Tandem was closed.", grays)));
                pane.jiggle();
            }
        }
        Ok(id)
    }

    /// Host sessions nobody claimed: a tab each in this (new) window, the default profile,
    /// the whole output the host kept, a repaint, and iTerm2's notice.
    pub(super) fn adopt_orphans(&mut self, sessions: &[(u64, u32)]) {
        let area = self.content_area();
        for (host_id, pid) in sessions {
            match self.attach_pane(area, None, *host_id, *pid, 0, None) {
                Ok(id) => {
                    if let Some(p) = self.panes.get_mut(&id) {
                        p.jiggle();
                    }
                    self.tabs.push(Tab::new(self.next_tab_id, Layout::new(id), id));
                    self.next_tab_id += 1;
                }
                Err(e) => self.banners.set("reattach", format!("A running shell could not be taken back: {e}")),
            }
        }
        self.active = 0;
        if !self.tabs.is_empty() {
            self.banners.set("orphan", "This already-running session was restored but its contents were not saved.");
        }
        self.relayout();
    }
}

/// Every saved session's chat reference becomes adoptable (launch-time restoration only).
fn mark_adoptable(windows: &mut [Value]) {
    let mut sessions = Vec::new();
    for w in windows.iter_mut() {
        sessions_mut(w, &mut sessions);
    }
    for s in sessions {
        crate::arrangement::mark_adoptable(s);
    }
}

/// The session tables of a window table, in reading order.
fn sessions_mut<'a>(v: &'a mut Value, out: &mut Vec<&'a mut toml::Table>) {
    match v {
        Value::Table(t) => {
            if t.get("View Type").and_then(|x| x.as_str()) == Some("SessionView") {
                if let Some(Value::Table(s)) = t.get_mut("Session") {
                    out.push(s);
                }
                return;
            }
            for (_, x) in t.iter_mut() {
                sessions_mut(x, out);
            }
        }
        Value::Array(a) => {
            for x in a.iter_mut() {
                sessions_mut(x, out);
            }
        }
        _ => {}
    }
}

pub(super) fn host_key_value(id: u64, pid: u32, offset: u64) -> Value {
    let mut t = toml::Table::new();
    t.insert("Id".into(), Value::Integer(id as i64));
    t.insert("Pid".into(), Value::Integer(pid as i64));
    t.insert("Offset".into(), Value::Integer(offset as i64));
    Value::Table(t)
}

fn parse_host_key(v: &Value) -> Option<crate::reattach::HostKey> {
    let n = |k: &str| v.get(k).and_then(|x| x.as_integer()).filter(|i| *i >= 0);
    Some(crate::reattach::HostKey { id: n("Id")? as u64, pid: u32::try_from(n("Pid")?).ok()?, offset: n("Offset")? as u64 })
}

/// A session's saved cookie ("Reusable Cookie", beside `HOST_KEY`), when it is one Tandem could
/// have minted.
pub(super) fn saved_cookie(session: &Value) -> Option<String> {
    session.get(crate::session_env::REUSABLE_COOKIE_KEY).and_then(|v| v.as_str()).filter(|c| crate::session_env::is_cookie(c)).map(str::to_string)
}

/// The stable id a session saved for the launch's restoration, canonical, when it is one Tandem
/// could have minted (the marker `arrangement::mark_adoptable` sets at launch).
pub(super) fn stable_id_in(session: &Value) -> Option<String> {
    crate::arrangement::adoptable_stable_id(session)
}

/// A session marked to take its shell back: (host session id, pid, output offset).
pub(super) fn attach_key(session: &Value) -> Option<(u64, u32, u64)> {
    let t = session.get(crate::arrangement::ATTACH_KEY)?;
    let n = |k: &str| t.get(k).and_then(|x| x.as_integer()).filter(|i| *i >= 0);
    Some((n("Id")? as u64, u32::try_from(n("Pid")?).ok()?, n("Since")? as u64))
}

/// Every session with restored contents gets the banner line.
fn mark_banner(v: &mut Value, line: &str) {
    match v {
        Value::Table(t) => {
            if t.get("View Type").and_then(|x| x.as_str()) == Some("SessionView") {
                if let Some(Value::Table(s)) = t.get_mut("Session") {
                    if s.contains_key(crate::arrangement::STYLED_KEY) {
                        s.insert(crate::arrangement::BANNER_KEY.into(), Value::String(line.into()));
                    }
                }
                return;
            }
            for (_, x) in t.iter_mut() {
                mark_banner(x, line);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| mark_banner(x, line)),
        _ => {}
    }
}

/// Now, in the person's locale: the short date and the time without seconds.
fn restore_date() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let now = crate::stamps::local(secs);
    format!("{} at {}", crate::stamps::short_date(&now), crate::stamps::short_time(&now))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arrangement::{ADOPT_REF_KEY, SESSION_REF_KEY};

    #[test]
    fn a_launch_read_makes_saved_references_adoptable() {
        let session = |guid: &str| {
            let mut s = toml::Table::new();
            s.insert(SESSION_REF_KEY.into(), Value::String(guid.into()));
            let mut v = toml::Table::new();
            v.insert("View Type".into(), Value::String("SessionView".into()));
            v.insert("Session".into(), Value::Table(s));
            Value::Table(v)
        };
        let mut window = toml::Table::new();
        window.insert("Tabs".into(), Value::Array(vec![session("6F1C2A3B-0000-4000-8000-000000000001"), session("6F1C2A3B-0000-4000-8000-000000000002")]));
        let mut windows = [Value::Table(window)];
        mark_adoptable(&mut windows);
        let mut sessions = Vec::new();
        sessions_mut(&mut windows[0], &mut sessions);
        assert_eq!(sessions.len(), 2);
        for s in sessions {
            assert!(s.get(SESSION_REF_KEY).is_none() && s.get(ADOPT_REF_KEY).is_some());
        }
    }

    #[test]
    fn stable_id_survives_a_restoration_only() {
        use super::super::arrange_ui::{carry_ids, Contents};
        const ID: &str = "ptys_0123456789ABW";
        // Session restoration saves the id and the launch read makes it adoptable.
        let mut t = toml::Table::new();
        carry_ids(&mut t, Contents::Styled(100), "6F1C2A3B-0000-4000-8000-000000000001", ID);
        assert_eq!(t.get(crate::arrangement::STABLE_ID_KEY).and_then(|v| v.as_str()), Some(ID));
        let mut window = toml::Table::new();
        let mut view = toml::Table::new();
        view.insert("View Type".into(), Value::String("SessionView".into()));
        view.insert("Session".into(), Value::Table(t));
        window.insert("Tabs".into(), Value::Array(vec![Value::Table(view)]));
        let mut windows = [Value::Table(window)];
        mark_adoptable(&mut windows);
        let mut sessions = Vec::new();
        sessions_mut(&mut windows[0], &mut sessions);
        let back = Value::Table(sessions[0].clone());
        assert_eq!(stable_id_in(&back).as_deref(), Some(ID));
        // An arrangement, a file or a duplicate holds no id at all, with or without contents.
        for c in [Contents::None, Contents::Plain] {
            let mut t = toml::Table::new();
            carry_ids(&mut t, c, "6F1C2A3B-0000-4000-8000-000000000001", ID);
            assert!(t.is_empty(), "{c:?}");
        }
        // A saved value that is not a stable id gives None, so a fresh id is minted.
        let mut bad = toml::Table::new();
        bad.insert(crate::arrangement::ADOPT_STABLE_KEY.into(), Value::String("ptys_bad".into()));
        assert_eq!(stable_id_in(&Value::Table(bad)), None);
    }
}

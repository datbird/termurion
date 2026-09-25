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
        self.write_state(&State { host: None, windows });
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

    fn restore_contents(&self) -> Contents {
        if self.globals.bool("restoreWindowContents") {
            Contents::Styled(self.globals.int("maxHistoryLinesToRestore").clamp(0, 10_000_000) as usize)
        } else {
            Contents::None
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
    pub(super) fn saved_windows(&mut self) -> Option<Vec<Value>> {
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
                self.banners.set("state", format!("The saved windows could not be restored: {e}.{kept}"));
                return None;
            }
        };
        if state.windows.is_empty() {
            return None;
        }
        if self.globals.bool("showSessionRestoredBanner") {
            let line = restore_state::banner_text(&restore_date());
            for w in &mut state.windows {
                mark_banner(w, &line);
            }
        }
        Some(state.windows)
    }
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
    use windows_sys::Win32::Globalization::{DATE_SHORTDATE, GetDateFormatEx, GetTimeFormatEx, TIME_NOSECONDS};
    let mut date = [0u16; 128];
    let mut time = [0u16; 128];
    let (d, t) = unsafe {
        (
            GetDateFormatEx(std::ptr::null(), DATE_SHORTDATE, std::ptr::null(), std::ptr::null(), date.as_mut_ptr(), date.len() as i32, std::ptr::null()),
            GetTimeFormatEx(std::ptr::null(), TIME_NOSECONDS, std::ptr::null(), std::ptr::null(), time.as_mut_ptr(), time.len() as i32),
        )
    };
    let text = |buf: &[u16], n: i32| String::from_utf16_lossy(&buf[..(n.max(1) as usize - 1)]);
    format!("{} at {}", text(&date, d), text(&time, t))
}

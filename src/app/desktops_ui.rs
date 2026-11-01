//! Virtual desktops in the window (13g): the shell manager, and putting a restored window back
//! on the desktop it was saved on.

use super::*;
use crate::arrangement::Value;

impl App {
    /// The shell's virtual desktop manager, made on first use. None (logged once) when the
    /// session has no desktop shell, which turns every desktop feature off quietly (P7).
    pub(super) fn desktops(&mut self) -> Option<&crate::desktops::win::Manager> {
        if !self.desktops_tried {
            self.desktops_tried = true;
            self.desktops = crate::desktops::win::Manager::new();
            if self.desktops.is_none() {
                eprintln!("virtual desktop manager unavailable; desktop features are off");
            }
        }
        self.desktops.as_ref()
    }

    /// Restore windows to same desktops: a window restored at launch moves to the desktop its
    /// saved table names, when the setting is on and that desktop still exists. A failure is a
    /// log line only.
    pub(super) fn restore_desktop(&mut self, saved_window: &Value) {
        if !self.globals.bool(crate::desktops::RESTORE_KEY) {
            return;
        }
        let Some(saved) = crate::desktops::saved(saved_window) else { return };
        let Some(hwnd) = self.hwnd() else { return };
        let Some(m) = self.desktops() else { return };
        let Some(target) = crate::desktops::restore_target(Some(saved), &m.list()) else { return };
        if let Err(e) = m.move_window(hwnd, target) {
            eprintln!("could not move the window to its saved desktop: {e}");
        }
    }
}

impl App {
    /// A new window's profile Space (`iTermController.m:1052-1061`, `iTermSessionLauncher.m:729`):
    /// a numbered desktop moves the window there and activates it (Windows then switches, which
    /// stands in for iTerm2's Ctrl+number; UNVERIFIED, P18c: a window that stays on another
    /// desktop is left there without an error), All Desktops makes it follow, a missing desktop
    /// notifies once per run. Nothing happens without a desktop manager (P7).
    pub(super) fn apply_space(&mut self, space: i64) {
        use crate::desktops::{space_target, Target};
        if !crate::desktops::space_applies(self.hotkey.is_some(), space) || self.desktops().is_none() {
            return;
        }
        let list = self.desktops.as_ref().map(|m| m.list()).unwrap_or_default();
        match space_target(space, &list) {
            Target::Current => {}
            Target::All => {
                self.win_follow_desktop = true;
                self.follow_now();
            }
            Target::Desktop(id) => {
                // iTerm2 switches spaces only for windows the session launcher opens; a hotkey
                // window shows on the current desktop (`roll_in`).
                if self.hotkey.is_some() {
                    return;
                }
                let Some(hwnd) = self.hwnd() else { return };
                let moved = self.desktops.as_ref().is_some_and(|m| match m.move_window(hwnd, id) {
                    Ok(()) => true,
                    Err(e) => {
                        eprintln!("could not move the new window to desktop {space}: {e}");
                        false
                    }
                });
                if moved {
                    if let Some(w) = self.current_window() {
                        self.focus_terminal_window(w);
                    }
                    self.switch_recheck = Some((hwnd, space, Instant::now() + crate::desktops::SWITCH_RECHECK));
                }
            }
            Target::Missing(n) => {
                if !std::mem::replace(&mut self.space_complained, true) {
                    let _ = crate::platform::notify::show_with(&crate::desktops::cant_switch(n), crate::desktops::CANT_SWITCH_FIX, false);
                }
            }
        }
    }

    /// Moves the current window to the current desktop when it is on another (no activation).
    pub(super) fn follow_now(&mut self) {
        let Some(hwnd) = self.hwnd() else { return };
        let Some(m) = self.desktops() else { return };
        Self::follow(m, hwnd);
    }

    fn follow(m: &crate::desktops::win::Manager, hwnd: isize) {
        if m.on_current(hwnd) == Some(false)
            && let Some(now) = m.current()
                && let Err(e) = m.move_window(hwnd, now) {
                    eprintln!("could not follow the current desktop: {e}");
                }
    }

    /// After `SWITCH_RECHECK`: the window activated on its desktop should now be on the current
    /// one; when not, one log line per run (P18c: the window is left where it is). The time the
    /// check is next due.
    fn switch_recheck_tick(&mut self) -> Option<Instant> {
        let (hwnd, space, due) = self.switch_recheck?;
        if Instant::now() < due {
            return Some(due);
        }
        self.switch_recheck = None;
        let failed = self.desktops().is_some_and(|m| crate::desktops::switch_failed(m.on_current(hwnd)));
        if failed && !std::mem::replace(&mut self.switch_logged, true) {
            eprintln!("Windows did not switch to desktop {space} when its window was activated; the window stays there");
        }
        None
    }

    /// Checks every visible following window (current or parked) against the current desktop,
    /// at most once per `FOLLOW_EVERY`. The next time it needs to run, None when no window follows.
    pub(super) fn desktop_tick(&mut self) -> Option<Instant> {
        let recheck = self.switch_recheck_tick();
        let follow = self.follow_tick();
        [recheck, follow].into_iter().flatten().min()
    }

    fn follow_tick(&mut self) -> Option<Instant> {
        let visible = |g: &Gpu| g.window.is_visible() != Some(false);
        let mut hwnds: Vec<isize> = Vec::new();
        if self.win_follow_desktop {
            hwnds.extend(self.gpu.as_ref().filter(|g| visible(g)).and_then(|g| super::menu::hwnd_of(&g.window)));
        }
        for (_, s) in self.windows.parked() {
            if s.win_follow_desktop {
                hwnds.extend(s.gpu.as_ref().filter(|g| visible(g)).and_then(|g| super::menu::hwnd_of(&g.window)));
            }
        }
        if hwnds.is_empty() {
            self.desktop_due = None;
            return None;
        }
        let now = Instant::now();
        if self.desktop_due.is_some_and(|t| now < t) {
            return self.desktop_due;
        }
        self.desktop_due = Some(now + crate::desktops::FOLLOW_EVERY);
        if let Some(m) = self.desktops() {
            for h in hwnds {
                Self::follow(m, h);
            }
        }
        self.desktop_due
    }
}

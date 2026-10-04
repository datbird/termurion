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
        if space == 0 || self.desktops().is_none() {
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
                let Some(hwnd) = self.hwnd() else { return };
                let moved = self.desktops.as_ref().is_some_and(|m| match m.move_window(hwnd, id) {
                    Ok(()) => true,
                    Err(e) => {
                        eprintln!("could not move the new window to desktop {space}: {e}");
                        false
                    }
                });
                // A hotkey window is shown (and focused) by its own roll in.
                if moved && self.hotkey.is_none() {
                    if let Some(w) = self.current_window() {
                        self.focus_terminal_window(w);
                    }
                }
            }
            Target::Missing(n) => {
                if !std::mem::replace(&mut self.space_complained, true) {
                    let _ = crate::notify::show_with(&crate::desktops::cant_switch(n), crate::desktops::CANT_SWITCH_FIX, false);
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
        if m.on_current(hwnd) == Some(false) {
            if let Some(now) = m.current() {
                if let Err(e) = m.move_window(hwnd, now) {
                    eprintln!("could not follow the current desktop: {e}");
                }
            }
        }
    }

    /// Checks every visible following window (current or parked) against the current desktop,
    /// at most once per `FOLLOW_EVERY`. The next time it needs to run, None when no window follows.
    pub(super) fn desktop_tick(&mut self) -> Option<Instant> {
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

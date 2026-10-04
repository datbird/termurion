//! Leaving the taskbar and Alt+Tab (13g): applies `presence` to every window each loop pass.

use super::*;
use crate::presence::{self, Look};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongPtrW, SW_HIDE, SW_SHOWNA, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos, ShowWindow, WS_EX_TOOLWINDOW,
};

/// Whether the window's actual extended style carries the tool window bit.
fn has_tool_style(hwnd: isize) -> bool {
    unsafe { GetWindowLongPtrW(hwnd as HWND, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW != 0 }
}

/// Sets the tool window bit when it differs from `want`; a visible window with a taskbar
/// button is hidden and shown again so the taskbar drops or adds it (`SW_SHOWNA`: no focus
/// change). A hotkey window is never hidden by this (its focus loss would auto-hide it).
fn set_tool_style(window: &winit::window::Window, hwnd: isize, want: bool, hotkey: bool) {
    if has_tool_style(hwnd) == want {
        return;
    }
    unsafe {
        let style = GetWindowLongPtrW(hwnd as HWND, GWL_EXSTYLE);
        let style = if want { style | WS_EX_TOOLWINDOW as isize } else { style & !(WS_EX_TOOLWINDOW as isize) };
        SetWindowLongPtrW(hwnd as HWND, GWL_EXSTYLE, style);
        SetWindowPos(hwnd as HWND, std::ptr::null_mut(), 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        if presence::restyle_needs_reshow(hotkey, window.is_visible() != Some(false)) {
            ShowWindow(hwnd as HWND, SW_HIDE);
            ShowWindow(hwnd as HWND, SW_SHOWNA);
        }
    }
}

/// Applies `look` to a terminal window: winit's skip only on a change, the tool style against
/// the window's actual style every pass (winit rewrites the style on its own flag changes).
fn apply_look(window: &winit::window::Window, look: Look, seen: &mut Option<Look>, hotkey: bool) {
    use winit::platform::windows::WindowExtWindows;
    // A hotkey window is made without a taskbar button; a regular one with.
    let before = seen.map_or(hotkey, |l| l.skip_taskbar);
    if before != look.skip_taskbar {
        window.set_skip_taskbar(look.skip_taskbar);
    }
    if let Some(hwnd) = super::menu::hwnd_of(window) {
        set_tool_style(window, hwnd, look.tool_window, hotkey);
    }
    *seen = Some(look);
}

impl App {
    /// Each loop pass: whether Tandem is a UI element now, and every window's look. Returns
    /// the UI element value (Task 9's notification area icon follows it).
    pub(super) fn presence_tick(&mut self) -> bool {
        let windows = self.registry.order().len();
        let hotkeys = self.hotkey_windows().len();
        let now = presence::is_ui_element(self.globals.bool(presence::UI_ELEMENT_KEY), self.globals.bool(presence::REQUIRES_HOTKEYS_KEY), windows, hotkeys);
        self.ui_element_now = now;
        let excluded = self.globals.bool(presence::EXCLUDED_FROM_CYCLING_KEY);
        let look_of = |hotkey: Option<bool>, style: i64| presence::with_accessory(presence::window_look(now, hotkey, excluded), style == crate::frames::ACCESSORY);
        if let Some(g) = &self.gpu {
            let hotkey = self.hotkey.as_ref().map(|h| h.shown);
            apply_look(&g.window, look_of(hotkey, self.win_style), &mut self.look_seen, hotkey.is_some());
        }
        for (_, s) in self.windows.parked_mut() {
            let hotkey = s.hotkey.as_ref().map(|h| h.shown);
            if let Some(g) = &s.gpu {
                apply_look(&g.window, look_of(hotkey, s.win_style), &mut s.look_seen, hotkey.is_some());
            }
        }
        // Settings, panels and chat windows: the tool style alone (they keep winit's skip).
        for w in crate::egui_host::taskbar_windows() {
            if let Some(hwnd) = super::menu::hwnd_of(&w) {
                set_tool_style(&w, hwnd, now, false);
            }
        }
        now
    }

    /// Settings changed: turning Exclude on asks first (a persistent warning, `APV:246-283`);
    /// turning it off applies at once. `ui_element_confirmed` follows every applied value.
    pub(super) fn presence_globals_changed(&mut self) {
        let want = self.globals.bool(presence::UI_ELEMENT_KEY);
        if want && !self.ui_element_confirmed {
            // Back to off until the person answers.
            self.globals.set(presence::UI_ELEMENT_KEY, crate::arrangement::Value::Boolean(false));
            self.save_globals();
            if !self.alert_waits(|t| *t == scripts_alerts::Then::ExcludeFromTaskbar) {
                let ask = scripts_alerts::ask(presence::EXCLUDE_HEADING, presence::EXCLUDE_WARNING, &[presence::EXCLUDE_ACTION, presence::CANCEL], None, crate::warnings::Kind::Persistent, scripts_alerts::Then::ExcludeFromTaskbar);
                self.ask_scripts(ask);
            }
        } else {
            self.ui_element_confirmed = want;
        }
    }

    /// The warning's first button: the setting turns on and saves.
    pub(super) fn exclude_from_taskbar_confirmed(&mut self) {
        self.ui_element_confirmed = true;
        self.globals.set(presence::UI_ELEMENT_KEY, crate::arrangement::Value::Boolean(true));
        self.globals_changed(true);
    }
}

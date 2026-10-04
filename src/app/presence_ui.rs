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

/// The tray menu's own id for Bring All Windows to Front (the menu bar's item focuses only the
/// current window).
const TRAY_BRING_ALL_ID: &str = "tray-bring-all";

impl App {
    /// Follows `presence_tick`'s value: the notification area icon is added or removed on a change.
    pub(super) fn tray_tick(&mut self, ui_element_now: bool) {
        let want = presence::tray_wanted(ui_element_now, self.globals.bool(presence::STATUS_BAR_ICON_KEY));
        if want == self.tray_on {
            return;
        }
        match crate::notify::set_resident(want) {
            Ok(()) => self.tray_on = want,
            Err(e) => {
                self.tray_on = want;
                self.banners.set("tray", e);
            }
        }
    }

    /// Terminal windows: (id, Some(shown) for a hotkey window, on screen), current first.
    fn terminal_window_states(&self) -> Vec<(WindowId, Option<bool>, bool)> {
        let on_screen = |w: &winit::window::Window| w.is_visible() != Some(false) && w.is_minimized() != Some(true);
        let mut out = Vec::new();
        if let Some(g) = &self.gpu {
            out.push((g.window.id(), self.hotkey.as_ref().map(|h| h.shown), on_screen(&g.window)));
        }
        for (_, s) in self.windows.parked() {
            if let Some(g) = &s.gpu {
                out.push((g.window.id(), s.hotkey.as_ref().map(|h| h.shown), on_screen(&g.window)));
            }
        }
        out
    }

    /// A click on the notification area icon: a right click shows its menu, a left click is the
    /// dock click (`HKC:362-402`, `AD:946-952`).
    pub(super) fn tray_clicked(&mut self, right: bool, event_loop: &ActiveEventLoop) {
        if right {
            return self.show_tray_menu();
        }
        let states = self.terminal_window_states();
        let hotkeys = states.iter().filter(|(_, h, on)| *h == Some(true) && *on).count();
        let regular = states.iter().filter(|(_, h, on)| h.is_none() && *on).count();
        let profiles: Vec<(String, i64)> = self.store.profiles.iter().filter(|p| crate::hotkey::Prefs::from_profile(p).active()).map(|p| (p.guid(), p.int("HotKey Window Dock Click Action"))).collect();
        let back = self.current_window();
        match presence::dock_click(hotkeys, &profiles, regular) {
            presence::DockClick::HideHotkeyWindows => {
                for w in self.hotkey_windows() {
                    self.enter(w);
                    self.roll_out(true);
                }
                if let Some(b) = back {
                    self.enter(b);
                }
            }
            presence::DockClick::Show(guids) => {
                for g in guids {
                    self.show_hotkey_window(&g, 0);
                }
            }
            // The reopen macOS runs: the last window comes forward, or a new one opens.
            presence::DockClick::Unhandled => match self.last_window.filter(|w| states.iter().any(|(id, _, _)| id == w)) {
                Some(w) => self.focus_terminal_window(w),
                None if states.iter().all(|(_, _, on)| !on) => self.new_window(event_loop, None),
                None => {}
            },
        }
    }

    /// Bring All Windows to Front from the tray: every window, in registry order.
    fn tray_bring_all(&mut self) {
        let ids: Vec<WindowId> = self.registry.order().into_iter().filter_map(|k| self.window_id_of_key(k)).collect();
        for w in ids {
            self.focus_terminal_window(w);
        }
    }

    /// The icon's menu (`AD:1846-1880`) on the current terminal window (a cloaked hotkey window
    /// still has a handle; the message-only window cannot own a popup).
    fn show_tray_menu(&mut self) {
        use muda::{ContextMenu, Menu, MenuItem, PredefinedMenuItem, Submenu};
        let Some(hwnd) = self.hwnd() else { return };
        let menu = Menu::new();
        for (label, what) in presence::TRAY_ITEMS {
            let appended = match what {
                "buried" => {
                    let sub = Submenu::new(label, true);
                    let entries = self.buried_entries();
                    for (id, text) in &entries {
                        let _ = if id.0.is_empty() { sub.append(&PredefinedMenuItem::separator()) } else { sub.append(&MenuItem::with_id(id.clone(), text.replace('&', "&&"), true, None)) };
                    }
                    sub.set_enabled(!entries.is_empty());
                    menu.append(&sub)
                }
                "main" => menu.append(&self.main_menu_tree()),
                "arrangeInFront:" => menu.append(&MenuItem::with_id(TRAY_BRING_ALL_ID, label, true, None)),
                selector => match crate::menus::items().iter().find(|i| i.selector == selector) {
                    Some(item) => menu.append(&MenuItem::with_id(item.id(), label, Self::menu_enabled(item), None)),
                    None => continue,
                },
            };
            if let Err(e) = appended {
                self.banners.set("menu", format!("Notification area menu: {e}"));
            }
        }
        // A cloaked or background window still has to be foreground for the menu to close on a click away.
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(hwnd as HWND) };
        unsafe { menu.show_context_menu_for_hwnd(hwnd, None) };
        // The pick arrives after the menu closes, so the menu lives until the next one.
        self.popup_menu = Some(menu);
    }

    /// A pick from the tray's own items. False for other ids.
    pub(super) fn tray_picked(&mut self, id: &str) -> bool {
        if id != TRAY_BRING_ALL_ID {
            return false;
        }
        self.tray_bring_all();
        true
    }
}

//! Leaving the taskbar and Alt+Tab (13g): applies `presence` to every window each loop pass.

use super::*;
use crate::presence::{self, Look, Reshow};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetForegroundWindow, GetWindowLongPtrW, IsIconic, IsZoomed, SW_HIDE, SW_SHOW, SW_SHOWMAXIMIZED, SW_SHOWMINNOACTIVE, SW_SHOWNA, SetWindowLongPtrW, ShowWindow, WS_EX_TOOLWINDOW,
};

/// Whether the window's actual extended style carries the tool window bit.
fn has_tool_style(hwnd: isize) -> bool {
    unsafe { GetWindowLongPtrW(hwnd as HWND, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW != 0 }
}

/// Sets the tool window bit when it differs from `want`, inside `keep_client` so the client area
/// keeps its size across the caption change (P9). A visible window with a taskbar button is hidden
/// and shown again so the taskbar drops or adds it, in the state it was in (minimized, maximized,
/// focused). A hotkey window is never hidden by this (its focus loss would auto-hide it).
/// Returns whether the style changed.
fn set_tool_style(window: &winit::window::Window, hwnd: isize, want: bool, hotkey: bool) -> bool {
    if has_tool_style(hwnd) == want {
        return false;
    }
    super::caption_ui::keep_client(hwnd, || unsafe {
        let style = GetWindowLongPtrW(hwnd as HWND, GWL_EXSTYLE);
        let style = if want { style | WS_EX_TOOLWINDOW as isize } else { style & !(WS_EX_TOOLWINDOW as isize) };
        SetWindowLongPtrW(hwnd as HWND, GWL_EXSTYLE, style);
    });
    if presence::restyle_needs_reshow(hotkey, window.is_visible() != Some(false)) {
        unsafe {
            let minimized = IsIconic(hwnd as HWND) != 0;
            let maximized = IsZoomed(hwnd as HWND) != 0 && window.fullscreen().is_none();
            let focused = GetForegroundWindow() == hwnd as HWND;
            ShowWindow(hwnd as HWND, SW_HIDE);
            ShowWindow(
                hwnd as HWND,
                match presence::reshow_command(minimized, maximized, focused) {
                    Reshow::MinimizedNoActivate => SW_SHOWMINNOACTIVE,
                    Reshow::Maximized => SW_SHOWMAXIMIZED,
                    Reshow::Show => SW_SHOW,
                    Reshow::ShowNoActivate => SW_SHOWNA,
                },
            );
        }
    }
    true
}

/// Applies `look` to a terminal window: winit's skip only on a change, the tool style against
/// the window's actual style every pass (winit rewrites the style on its own flag changes).
fn apply_look(window: &winit::window::Window, look: Look, seen: &mut Option<Look>, hotkey: bool) -> bool {
    let mut changed = false;
    use winit::platform::windows::WindowExtWindows;
    // A hotkey window is made without a taskbar button; a regular one with.
    let before = seen.map_or(hotkey, |l| l.skip_taskbar);
    if before != look.skip_taskbar {
        window.set_skip_taskbar(look.skip_taskbar);
    }
    if let Some(hwnd) = super::menu::hwnd_of(window) {
        changed = set_tool_style(window, hwnd, look.tool_window, hotkey);
    }
    *seen = Some(look);
    changed
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
        let mut flipped = false;
        // A resident window stays hidden and off the taskbar: no look is applied to it.
        if let (Some(g), false) = (&self.gpu, self.resident) {
            let hotkey = self.hotkey.as_ref().map(|h| h.shown);
            flipped = apply_look(&g.window, look_of(hotkey, self.win_style), &mut self.look_seen, hotkey.is_some());
        }
        if flipped {
            // The caption math follows the style (13e): the frame is re-checked and the panes refit.
            self.refresh_caption_frame();
            self.relayout();
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
    /// A failed add is retried on a growing delay (`presence::tray_retry_delay`), woken through the
    /// pass's wake array, so a resident Tandem with no events still gets its icon once Explorer is
    /// up. Returns the next retry's due time. Every result is logged to host.log.
    pub(super) fn tray_tick(&mut self, ui_element_now: bool) -> Option<Instant> {
        let want = crate::resident::icon_wanted(presence::tray_wanted(ui_element_now, self.globals.bool(presence::STATUS_BAR_ICON_KEY)), self.resident);
        let now = Instant::now();
        if !presence::tray_attempt_due(want, self.tray_on, self.tray_failures, self.tray_due, now) {
            // Waiting for a retry's time, or nothing to do.
            return if presence::tray_changes(want, self.tray_on) { self.tray_due } else { None };
        }
        // A failure leaves the state as it was, so the next try is the retry (Explorer not up yet).
        let result = crate::notify::set_resident(want);
        self.tray_on = presence::tray_settle(want, self.tray_on, result.is_ok());
        let before = self.tray_failures;
        self.tray_failures = presence::tray_failures_after(self.tray_failures, result.is_ok());
        if presence::tray_enters_steady_retry(before, self.tray_failures) {
            crate::host::log(&format!("tray: {before} failures, retrying every 30 s while resident with no icon"));
        }
        let verb = if want { "add" } else { "remove" };
        match &result {
            Ok(()) => crate::host::log(&format!("tray: {verb} ok")),
            Err(e) => {
                crate::host::log(&format!("tray: {verb} failed ({e}), failures in a row {}", self.tray_failures));
                self.banners.set("tray", e.clone());
            }
        }
        self.tray_due = presence::tray_retry_delay(self.tray_failures).map(|d| now + d);
        self.tray_due
    }

    /// Explorer restarted and dropped every icon: add it again from a clean count.
    pub(super) fn taskbar_created(&mut self) {
        crate::host::log("tray: TaskbarCreated, adding the icon again");
        self.tray_on = false;
        self.tray_failures = 0;
        self.tray_due = None;
        self.tray_tick(false);
    }

    /// Terminal windows: (id, Some(shown) for a hotkey window, on screen), current first.
    pub(super) fn terminal_window_states(&self) -> Vec<(WindowId, Option<bool>, bool)> {
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
        self.dock_click_now(event_loop);
    }

    /// iTerm2's dock click (`AD:946-952`): the icon's left click, and a second bare start of
    /// `termurion.exe` (`launch::Request::Reopen`, which reaches here over the launch pipe).
    pub(super) fn dock_click_now(&mut self, event_loop: &ActiveEventLoop) {
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
            presence::DockClick::Unhandled => self.reopen(event_loop),
        }
    }

    /// Bring All Windows to Front from the tray: every window, in registry order.
    fn tray_bring_all(&mut self) {
        // arrangeInFront skips closed hotkey windows: a hidden one is not rolled in.
        let ids: Vec<WindowId> = self.registry.order().into_iter().filter(|k| !self.hidden_hotkey_key(*k)).filter_map(|k| self.window_id_of_key(k)).collect();
        for w in ids {
            self.focus_terminal_window(w);
        }
    }

    /// The icon's menu (`AD:1846-1880`) on the current terminal window (a cloaked hotkey window
    /// still has a handle), else on the notification window (a hidden top-level window), so it also
    /// opens with no terminal window made yet.
    fn show_tray_menu(&mut self) {
        use muda::{ContextMenu, Menu, MenuItem, PredefinedMenuItem, Submenu};
        let Some(hwnd) = presence::tray_menu_owner(self.hwnd(), crate::notify::window_handle()) else { return };
        let menu = Menu::new();
        // Every window first: one that left the taskbar and Alt+Tab (or was minimized) has no
        // other way back. A pick runs `window_list_picked`.
        let windows = super::menu::tray_window_items(&self.window_menu_entries(), self.last_window.map(super::windows_ui::key), self.globals.bool("IncludeShortcutInWindowsMenu"), self.globals.int("SwitchWindowModifier"));
        for (id, text) in &windows {
            let _ = menu.append(&MenuItem::with_id(id.clone(), text.replace('&', "&&"), true, None));
        }
        for (id, text) in presence::tray_panel_items(&Self::panel_windows()) {
            let _ = menu.append(&MenuItem::with_id(id, text.replace('&', "&&"), true, None));
        }
        if !windows.is_empty() || !Self::panel_windows().is_empty() {
            let _ = menu.append(&PredefinedMenuItem::separator());
        }
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
        let picked = unsafe { menu.show_context_menu_for_hwnd(hwnd, None) };
        crate::host::log(&format!("tray: menu on {hwnd:#x} (notification window {}), picked {picked}", Some(hwnd) == crate::notify::window_handle()));
        self.tray_pick = picked;
        // The pick arrives after the menu closes, so the menu lives until the next one.
        self.popup_menu = Some(menu);
    }

    /// Settings, chat and panel windows that left the taskbar: (window id, title).
    fn panel_windows() -> Vec<(u64, String)> {
        crate::egui_host::taskbar_windows().iter().map(|w| (u64::from(w.id()), w.title())).collect()
    }

    /// A pick from the tray's own items. False for other ids.
    pub(super) fn tray_picked(&mut self, id: &str) -> bool {
        if let Some(n) = presence::tray_panel_pick(id) {
            if let Some(w) = crate::egui_host::taskbar_windows().into_iter().find(|w| u64::from(w.id()) == n) {
                w.set_minimized(false);
                w.focus_window();
            }
            return true;
        }
        if id != TRAY_BRING_ALL_ID {
            return false;
        }
        self.tray_bring_all();
        true
    }
}

#[cfg(test)]
mod tests {
    /// Minor 5: tray "Bring All Windows to Front" skips hidden hotkey windows (arrangeInFront does).
    #[test]
    fn tray_bring_all_skips_hidden_hotkey_windows() {
        let src = include_str!("presence_ui.rs");
        let body = src.split("fn tray_bring_all").nth(1).expect("tray_bring_all");
        let body = body.split("\n    }\n").next().unwrap();
        assert!(body.contains("hidden_hotkey_key"), "a hidden hotkey window is not rolled in");
    }

    /// The style flip of a visible window keeps the client area (P9): it runs inside
    /// `caption_ui::keep_client`, and the current window re-checks its caption frame.
    #[test]
    fn style_flip_keeps_the_client_size() {
        let src = include_str!("presence_ui.rs");
        let flip = src.split("fn set_tool_style").nth(1).and_then(|r| r.split("\nfn apply_look").next()).expect("set_tool_style");
        assert!(flip.contains("keep_client("), "the flip runs inside keep_client");
        let tick = src.split("fn presence_tick").nth(1).and_then(|r| r.split("fn presence_globals_changed").next()).expect("presence_tick");
        assert!(tick.contains("refresh_caption_frame()"), "the current window re-checks its caption frame");
    }

    /// Every non-crash exit takes the process down through `teardown_process`: the keyboard hook,
    /// the person's layout and the icon (13u Task 5 review). The scan covers every exit site.
    #[test]
    fn every_exit_path_runs_the_one_teardown() {
        const CALL: &str = "teardown_process()";
        let m = include_str!("mod.rs");
        let after = m.split("event_loop.run_app(&mut app)").nth(1).expect("run_app");
        assert!(after[..400].contains(CALL), "torn down after the event loop ends");
        let hook = m.split("std::panic::set_hook").nth(1).and_then(|r| r.split("let event_loop").next()).expect("panic hook");
        assert!(hook.contains(CALL), "the panic hook tears down");
        let logoff = include_str!("logout_ui.rs").split("fn logoff_now").nth(1).and_then(|r| r.split("\n}").next()).expect("logoff_now");
        assert!(logoff.contains(CALL), "a logoff tears down");
        // Every std::process::exit in the app (outside tests) is preceded by the teardown.
        let mut sites = 0;
        let mut dirs = vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")];
        while let Some(d) = dirs.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let path = e.path();
                let name = path.file_name().unwrap().to_string_lossy().to_string();
                if path.is_dir() {
                    // bin/ holds separate programs with no hook of ours.
                    if name != "bin" {
                        dirs.push(path);
                    }
                    continue;
                }
                // The console tools (a probe run) never install the hook or change the layout.
                if !name.ends_with(".rs") || name == "util_console.rs" {
                    continue;
                }
                let src = std::fs::read_to_string(&path).unwrap();
                let code = src.split("#[cfg(test)]").next().unwrap();
                for (at, _) in code.match_indices("std::process::exit(") {
                    sites += 1;
                    let before = &code[at.saturating_sub(600)..at];
                    assert!(before.contains(CALL), "{name}: process::exit without the teardown before it");
                }
                assert!(!code.contains("process::abort("), "{name}: an abort skips the teardown");
            }
        }
        assert!(sites >= 1, "the scan found Erase All's exit");
    }

    /// Every non-crash exit removes the notification area icon (task 9 review 2), and the
    /// icon window hears the taskbar's broadcast (review 1), and a second start is the reopen.
    #[test]
    fn the_icon_is_removed_on_exit_and_survives_explorer() {
        let m = include_str!("mod.rs");
        let after = m.split("event_loop.run_app(&mut app)").nth(1).expect("run_app");
        assert!(after[..400].contains("teardown_process()"), "removed right after the event loop ends");
        let teardown = m.split("fn teardown_process").nth(1).and_then(|r| r.split("\n}").next()).expect("teardown_process");
        assert!(teardown.contains("notify::remove"), "the teardown removes the icon");
        assert!(teardown.contains("dynamic_profiles_scan::stop_all"), "the teardown stops the Dynamic Profiles watch");
        let relaunch = include_str!("settings_export_ui.rs");
        let tail = relaunch.split("self.leave_host(false);").nth(1).expect("relaunch exit");
        let before_exit = tail.split("std::process::exit(0)").next().unwrap();
        assert!(before_exit.contains("teardown_process()"), "removed before the relaunch exit");
        let n = include_str!("../notify.rs");
        let win = n.split("fn window()").nth(1).and_then(|r| r.split("\nfn base").next()).expect("window()");
        assert!(!win.contains("HWND_MESSAGE"), "a message-only window never hears TaskbarCreated");
        let run = m.split("pub fn run()").nth(1).and_then(|r| r.split("let event_loop").next()).expect("run");
        let after_wait = run.split("wait_for_pid(pid)").nth(1).expect("the wait comes first");
        assert!(after_wait.contains("forwardable_start(&request, wait_pid.is_some())") && after_wait.contains("hand_to_running(&request)"), "a second bare start hands its Reopen to the running Tandem, after the import wait");
        assert!(include_str!("launch_ui.rs").contains("Request::Reopen => self.dock_click_now"), "and the running Tandem acts as the dock click");
        assert!(!include_str!("../notify.rs").contains("forward_reopen"), "no second start channel beside the pipe");
    }
}

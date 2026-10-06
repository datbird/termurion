//! Staying resident with no window (m13v1 Task 7, R3): closing the last window keeps Tandem
//! running with its notification area icon unless "Quit when all windows are closed" is on
//! (`AD:915-942`). The last window is hidden, leaves the taskbar and keeps its GPU device; no shell
//! remains. Nothing here wakes the event loop while Tandem is idle.

use super::*;
use crate::resident::{self, AfterLast};

/// The setting keys this file reads (`MinRunningTime` and `OpenUntitledFile` are Advanced rows).
const QUIT_KEY: &str = "QuitWhenAllWindowsClosed";
const MIN_RUNNING_KEY: &str = "MinRunningTime";
const UNTITLED_KEY: &str = "OpenUntitledFile";

impl App {
    /// The person did something that counts against the grace period: a key in a session, Close,
    /// or closing a window themselves (`PTYTextView.m:7505-7509`, `PT:2728`, `PT:4775`).
    pub(super) fn note_interaction(&mut self) {
        self.user_interacted = true;
    }

    /// What closing the last window does now (`AD:915-942`), with the grace note logged once.
    pub(super) fn after_last_window_now(&mut self) -> AfterLast {
        let min = self.globals.number(MIN_RUNNING_KEY).unwrap_or(10.0);
        let ran = self.launched_at.elapsed();
        // The window that is closing is no terminal left (a hotkey window counts while it exists).
        let hotkeys = self.hotkey_windows().len().saturating_sub(usize::from(self.hotkey.is_some()));
        let decision = resident::after_last_window(false, hotkeys, self.user_interacted, ran, min, self.globals.bool(QUIT_KEY));
        let graced = !self.user_interacted && (ran.as_secs_f64() < min) && self.globals.bool(QUIT_KEY) && hotkeys == 0;
        if graced {
            crate::host::log(resident::grace_note());
        }
        decision
    }

    /// The last window's last tab closed and Tandem stays: the sessions end, the host stays up, the
    /// empty state is written without freezing later saves (B7), and the window is hidden.
    pub(super) fn go_resident_after_last(&mut self) {
        self.save_pending_settings();
        self.write_empty_state();
        self.flush_notes();
        self.clear_undo();
        if let Some(c) = self.undo_capture.as_mut() {
            for p in &mut c.panes {
                p.stop_log();
            }
        }
        self.finish_archives(false, std::time::Duration::from_secs(5));
        self.close_all_logs();
        self.end_sessions_keep_host();
        if self.save_due.is_some() {
            self.save_now();
        }
        self.go_resident();
    }

    /// Every hosted shell ends, on the same host that keeps serving the next window.
    fn end_sessions_keep_host(&mut self) {
        if let Some(host) = &self.host {
            host.kill_all_and_wait(std::time::Duration::from_secs(2));
        }
        self.panes.clear();
        self.focus_order.clear();
    }

    /// The last window becomes the resident one: its per-window state starts fresh on the same
    /// window and GPU, it leaves the window list, the taskbar and the screen.
    pub(super) fn go_resident(&mut self) {
        let Some(k) = self.windows.current() else { return };
        if self.resident {
            return;
        }
        self.save_window_frame();
        self.forget_window(k);
        let (Some(gpu), kind, menu) = (self.gpu.take(), self.win_alpha_surface, self.menu_bar.take()) else { return };
        let mut fresh = windows_ui::WinSlot::new(gpu);
        fresh.win_alpha_surface = kind;
        self.swap_live(&mut fresh);
        // The menu bar stays on the window handle until the next window builds its own.
        self.menu_bar = menu;
        self.last_window = self.current_window();
        self.resident = true;
        if let Some(g) = &self.gpu {
            use winit::platform::windows::WindowExtWindows;
            g.window.set_skip_taskbar(true);
            g.window.set_visible(false);
        }
    }

    /// The resident window is a window again: back in the window list and the taskbar (the caller
    /// shows it once it holds a tab). The state is not touched: saves ran all along (B7).
    pub(super) fn leave_resident(&mut self) {
        self.leave_resident_for(false);
    }

    /// `hotkey`: the window becomes a hotkey window, which has no taskbar button.
    pub(super) fn leave_resident_for(&mut self, hotkey: bool) {
        if !self.resident {
            return;
        }
        self.resident = false;
        if let Some(id) = self.current_window() {
            self.registry.open(windows_ui::key(id));
            self.refresh_window_list();
        }
        self.look_seen = None;
        if let Some(g) = &self.gpu {
            use winit::platform::windows::WindowExtWindows;
            g.window.set_skip_taskbar(hotkey);
        }
    }

    /// Shows the (former) resident window and brings it forward.
    pub(super) fn show_awake_window(&mut self) {
        if let Some(g) = &self.gpu {
            g.window.set_visible(true);
            g.window.set_minimized(false);
            g.window.focus_window();
        }
        self.relayout();
    }

    /// B11: a resident window that holds a tab (a link, a menu command, a script or the API made
    /// one) is a window again. Runs first in every loop pass, so no entry point leaves a tab in a
    /// hidden window. True when it woke.
    pub(super) fn wake_from_resident(&mut self) -> bool {
        if !self.resident || self.tabs.is_empty() {
            return false;
        }
        self.leave_resident();
        self.show_awake_window();
        true
    }

    /// iTerm2's reopen (`AD:944-951`, `AD:1132-1153`): with no window on screen and
    /// `OpenUntitledFile` on a window opens; with one, the last window comes forward.
    pub(super) fn reopen(&mut self, event_loop: &ActiveEventLoop) {
        let states = self.terminal_window_states();
        let visible = states.iter().filter(|(_, _, on)| *on).count();
        let open_untitled = self.globals.bool(UNTITLED_KEY);
        // A hidden resident window is never focused: it has nothing to show (B11).
        if self.resident {
            if resident::reopen_opens_window(open_untitled, visible) {
                self.new_window(event_loop, None);
            }
            return;
        }
        match crate::presence::reopen_target(self.last_window, &states) {
            crate::presence::Reopen::Focus(w) => self.focus_terminal_window(w),
            crate::presence::Reopen::NewWindow if resident::reopen_opens_window(open_untitled, visible) => self.new_window(event_loop, None),
            crate::presence::Reopen::NewWindow | crate::presence::Reopen::Nothing => {}
        }
    }
    /// WM_QUERYENDSESSION (`UserEvent::ShutdownQuery`, decided inside the window procedure's
    /// bounded wait): a shutdown asks exactly when a quit would (`AD:993-1036`, R3). `end_now` is
    /// set true when nothing needs asking. Otherwise Windows is told why it must wait (a block
    /// reason on the window that was asked, which may be the hidden resident one) and the question
    /// is posted to be shown after the procedure returned FALSE. Reads only; no dialog here.
    #[cfg(windows)]
    pub(super) fn shutdown_decide(&mut self, hwnd: isize, end_now: &std::sync::atomic::AtomicBool) {
        use std::sync::atomic::Ordering;
        let never = self.globals.bool("NeverBlockSystemShutdown");
        // A second window's query while the question is already standing: still held.
        if self.shutdown_block.is_some() {
            end_now.store(false, Ordering::Release);
            return;
        }
        let has_reasons = resident::reasons_collected(true, never) && !self.quit_reasons_now().is_empty();
        match resident::on_shutdown(never, has_reasons) {
            resident::ShutdownAnswer::EndNow => end_now.store(true, Ordering::Release),
            resident::ShutdownAnswer::AskFirst => {
                end_now.store(false, Ordering::Release);
                let reason: Vec<u16> = resident::BLOCK_REASON.encode_utf16().chain(Some(0)).collect();
                // SAFETY: `hwnd` is a top-level window of this process; `reason` is NUL-terminated.
                if unsafe { windows_sys::Win32::System::Shutdown::ShutdownBlockReasonCreate(hwnd as _, reason.as_ptr()) } != 0 {
                    self.shutdown_block = Some(hwnd);
                }
                let _ = self.proxy.lock().map(|p| p.send_event(UserEvent::ShutdownAsk));
            }
        }
    }

    /// Takes the block reason down (every path: OK, Cancel, the end of the session). Idempotent.
    pub(super) fn destroy_block_reason(&mut self) {
        #[cfg(windows)]
        if let Some(hwnd) = self.shutdown_block.take() {
            // SAFETY: the window the reason was created on; fails harmlessly if it is gone.
            unsafe { windows_sys::Win32::System::Shutdown::ShutdownBlockReasonDestroy(hwnd as _) };
        }
    }

    /// The quit question for a held shutdown (`UserEvent::ShutdownAsk`): OK takes the reason down
    /// and quits as Quit does, without asking again; Cancel takes it down and Tandem keeps running
    /// (Windows does not retry: the person starts the shutdown again, Decision).
    pub(super) fn shutdown_query(&mut self, event_loop: &ActiveEventLoop) {
        if self.confirm_close(close_ui::Closing::Shutdown) {
            self.destroy_block_reason();
            self.quit_confirmed(event_loop);
        } else {
            self.destroy_block_reason();
        }
    }
}

#[cfg(test)]
mod tests {
    fn production(src: &'static str) -> &'static str {
        src.split("\n#[cfg(test)]").next().unwrap()
    }

    /// The text of `fn name` up to the next method of its impl.
    fn body(src: &'static str, name: &str) -> &'static str {
        let head = format!("fn {name}(");
        let after = src.split(&head).nth(1).unwrap_or_else(|| panic!("{head}"));
        after.split("\n    pub(super) fn ").next().unwrap().split("\n    fn ").next().unwrap()
    }

    const OWN: &str = include_str!("resident_ui.rs");
    const MOD: &str = include_str!("mod.rs");
    const WINDOWS: &str = include_str!("windows_ui.rs");
    const PRESENCE: &str = include_str!("presence_ui.rs");

    /// B7: staying resident ends the shells on the SAME host and writes the empty state without
    /// freezing it, so the next window is hosted and saves normally.
    #[test]
    fn resident_keeps_the_host_and_state_saves() {
        let own = production(OWN);
        let go = body(own, "go_resident_after_last");
        assert!(go.contains("write_empty_state()") && go.contains("end_sessions_keep_host()") && go.contains("finish_archives(false") && go.contains("close_all_logs()"));
        for never in ["leave_host", "save_empty_state", "state_frozen", "save_state_for_quit", "event_loop"] {
            assert!(!go.contains(never), "the resident path never calls {never}");
        }
        let end = body(own, "end_sessions_keep_host");
        assert!(end.contains("kill_all_and_wait") && end.contains("&self.host"), "the host serves the next window");
        assert!(!end.contains(".take()") && !end.contains(".close()"), "the host is neither taken nor closed");
        let restore = production(include_str!("restore_ui.rs"));
        let write = body(restore, "write_empty_state");
        assert!(!write.contains("state_frozen = true"), "the resident empty state does not freeze later saves");
        assert!(body(restore, "save_empty_state").contains("write_empty_state()"));
        let leave = body(own, "leave_resident_for");
        assert!(!leave.contains("save_") && !leave.contains("state"), "leaving resident changes no state");
    }

    /// B11: every way a tab or a window is made while resident ends in one visible window: the
    /// window-making entries reuse the hidden window, a tab made by any route wakes it at the next
    /// pass, and a reopen never focuses a hidden empty window.
    #[test]
    fn resident_entry_points_wake_one_window() {
        let tick = body(production(MOD), "about_to_wait");
        let wake = tick.find("self.wake_from_resident();").expect("the pass wakes a resident window with a tab");
        assert!(wake < tick.find("self.sync_pane_activity();").unwrap() && wake < tick.find("self.tick_hotkeys()").unwrap(), "first, before any timer");
        let wake_body = body(production(OWN), "wake_from_resident");
        assert!(wake_body.contains("!self.resident || self.tabs.is_empty()"), "only a resident window that holds a tab wakes");
        let open = body(production(WINDOWS), "open_window_with");
        assert!(open.contains("let reused = self.resident;") && open.contains("leave_resident_for(hotkey)"), "a new window is the hidden one");
        assert!(open.contains("if reused && !hotkey") && open.contains("show_awake_window()"), "a regular window shows when it holds its tab");
        let reopen = body(production(OWN), "reopen");
        let resident_branch = reopen.split("if self.resident {").nth(1).unwrap().split("match crate::presence::reopen_target").next().unwrap();
        assert!(resident_branch.contains("return;") && !resident_branch.contains("reopen_target"), "a hidden window is never focused");
        let presence = production(PRESENCE);
        assert!(presence.contains("presence::DockClick::Unhandled => self.reopen(event_loop)") && !presence.contains("Reopen::Focus(w) => self.focus_terminal_window(w)"), "the tray's click is the one reopen");
    }

    /// `AD:1132-1153`: the icon's click opens a window only with `OpenUntitledFile` on, and only
    /// when none is on screen.
    #[test]
    fn tray_left_click_while_resident_opens_a_window_only_with_open_untitled() {
        assert!(!crate::resident::reopen_opens_window(false, 0));
        assert!(crate::resident::reopen_opens_window(true, 0));
        let reopen = body(production(OWN), "reopen");
        assert_eq!(reopen.matches("self.new_window(event_loop, None)").count(), 2, "both ways in");
        assert_eq!(reopen.matches("resident::reopen_opens_window(open_untitled, visible)").count(), 2, "each gated by the key and the visible count");
        assert!(reopen.contains("globals.bool(UNTITLED_KEY)") && OWN.contains("\"OpenUntitledFile\""));
    }

    /// P12: staying resident wakes nothing: this file sets no timer, no control flow and no due time,
    /// and the pass adds no wake source for it.
    #[test]
    fn resident_wake_array_is_empty() {
        let own = production(OWN);
        for never in ["WaitUntil", "set_control_flow", "Instant::now() +", "_due =", "_due:", "request_redraw", "redraw("] {
            assert!(!own.contains(never), "resident code must not schedule or draw: {never}");
        }
        let tick = body(production(MOD), "about_to_wait");
        let array = tick.split("let wake = [").nth(1).unwrap().split("].into_iter()").next().unwrap();
        assert!(!array.contains("resident"), "no resident source in the wake array");
        // A resident window gets no look applied and is never drawn.
        let presence = production(PRESENCE);
        assert!(presence.contains("(Some(g), false) = (&self.gpu, self.resident)"));
    }

    /// R3 (`AD:915-942`): the last window goes resident whatever kind it is while the key is off; a hotkey
    /// window as the last window (S5 reuses the resident window as one) is no exception. Hotkeys keep
    /// working while resident: the hook and the registered ids belong to the window that stays, hidden.
    #[test]
    fn resident_hide_takes_a_hotkey_window_too() {
        let close = body(production(MOD), "close_pane");
        let gate = close.split("iTerm2's rule for the last window").nth(1).unwrap().split("self.go_resident_after_last()").next().unwrap();
        assert!(gate.contains("self.windows.parked().next().is_none()") && gate.contains("after_last_window_now()"), "no other window");
        assert!(!gate.contains("self.hotkey"), "a hotkey window is a last window like any other");
        let go = body(production(OWN), "go_resident");
        assert!(!go.contains("self.hotkey"), "go_resident takes a hotkey window too (the fresh state clears it)");
        let after = body(production(OWN), "after_last_window_now");
        assert!(after.contains("self.hotkey.is_some()"), "the closing hotkey window is not a terminal left");
        for never in ["drop_current_window", "winkeys::", "register_hotkeys", "unregister", "close_current", "install_menus"] {
            assert!(!production(OWN).contains(never), "the resident window keeps its hook and hotkeys: {never}");
        }
        assert!(go.contains("set_visible(false)") && go.contains("set_skip_taskbar(true)"));
    }

    /// Every `tabs[self.active]` of the app is proven unreachable with no tab: only `split_as` (which
    /// returns at once without a focused tab) holds any, and the divider drag reads through `get_mut`.
    #[test]
    fn tab_indexing_is_guarded_while_resident() {
        let m = production(MOD);
        let split = body(m, "split_as");
        assert!(split.contains("let Some(target) = self.tabs.get(self.active).map(|t| t.focused) else { return };"));
        assert_eq!(m.matches("tabs[self.active]").count(), split.matches("tabs[self.active]").count(), "every index sits behind split_as's guard");
        assert!(m.contains("let Some(tab) = self.tabs.get_mut(self.active) else { return };"), "the divider drag is guarded");
        assert_eq!(production(include_str!("menu.rs")).matches("tabs[self.active]").count(), 0, "menu.rs indexes no tab");
    }

    /// R3 at launch: no window planned (and no hotkey window) is a hidden empty first window and a resident
    /// Tandem, not a window made "because nothing else would show".
    #[test]
    fn a_launch_with_no_window_starts_resident() {
        let run = body(production(MOD), "resumed");
        assert!(run.contains("let resident_start = self.start_handoff.is_none() && startup_hotkey.is_none()"));
        // iTerm2's launch path never reads openUntitledFile (AD:2463-2470): only the plan decides.
        assert!(run.contains("hotkey_profiles.is_empty() && !early.restore && !early.window;"), "only the launch plan decides");
        assert!(!run.contains("untitled") && !run.contains("OpenUntitledFile"), "OpenUntitledFile never decides a launch window");
        assert!(run.contains("if startup_hotkey.is_some() || resident_start {"), "the window is made hidden and off the taskbar");
        assert!(run.contains("None if resident_start => {}"), "no tab is made");
        assert!(run.contains("self.resident = true;"));
    }

    /// The icon is re-added after Explorer restarts (TaskbarCreated) while resident, and the 13g
    /// rule is joined with `resident`, never replaced.
    #[test]
    fn the_resident_icon_survives_explorer() {
        let notify = production(include_str!("../notify.rs"));
        assert!(notify.contains("RegisterWindowMessageW") && notify.contains("if RESIDENT.load(Ordering::SeqCst)") && notify.contains("add_resident(hwnd)"));
        assert!(production(PRESENCE).contains("crate::resident::icon_wanted(presence::tray_wanted("));
        assert!(production(PRESENCE).contains("crate::notify::set_resident(want)"));
    }

    /// A shutdown asks as a quit does, through the one subclass; the block reason stands only
    /// while there is something to ask and comes down on every path (a source check).
    #[test]
    fn a_shutdown_asks_through_the_block_reason_and_lets_it_go() {
        let src = include_str!("resident_ui.rs").split("#[cfg(test)]\nmod tests").next().unwrap();
        let decide = &src[src.find("fn shutdown_decide(").unwrap()..src.find("fn destroy_block_reason(").unwrap()];
        assert!(decide.contains("on_shutdown(never, has_reasons)") && decide.contains("quit_reasons_now()"), "the decision is the pure table over the quit reasons");
        let create = decide.find("ShutdownBlockReasonCreate").unwrap();
        assert!(create > decide.find("AskFirst").unwrap(), "a reason is made only for AskFirst");
        assert!(!decide.contains("confirm"), "no dialog inside the query");
        let ask = &src[src.find("fn shutdown_query(").unwrap()..];
        assert_eq!(ask.matches("destroy_block_reason()").count(), 2, "OK and Cancel both take it down");
        assert!(ask.contains("Closing::Shutdown") && ask.contains("quit_confirmed"), "OK quits without asking again");
        let logoff = include_str!("logout_ui.rs");
        assert!(logoff.contains("self.destroy_block_reason()"), "the session end takes it down");
    }
}

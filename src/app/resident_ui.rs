//! Staying resident with no window (m13v1 Task 7, R3): closing the last window keeps Termurion
//! running with its notification area icon unless "Quit when all windows are closed" is on
//! (`AD:915-942`). The last window is hidden, leaves the taskbar and keeps its GPU device; no shell
//! remains. Nothing here wakes the event loop while Termurion is idle.

use super::*;
use crate::resident::{self, AfterLast};

/// The setting keys this file reads (`MinRunningTime` and `OpenUntitledFile` are Advanced rows).
const QUIT_KEY: &str = "QuitWhenAllWindowsClosed";
const MIN_RUNNING_KEY: &str = "MinRunningTime";
const UNTITLED_KEY: &str = "OpenUntitledFile";

/// The banners of requests from outside that opened nothing (launch, files, the Jump List): raised
/// while resident they go to the notification area instead (`outside_note`), and the ones left from
/// before are cleared when a window wakes (final review item 10).
const OUTSIDE_KEYS: &[&str] = &["launch", "launch-ignored", "open-ignored", "open-file", "jump-list"];

/// How an entry point wakes a resident window (`wake_from_resident`).
pub(super) enum Wake<'a> {
    /// A new tab of this profile (None: the default).
    Window(Option<&'a str>),
    /// Tabs that exist already (a buried session restored): the hidden window opens with them.
    Tabs(Vec<Tab>),
    /// The caller makes its own tab in the awake window.
    InPlace,
}

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
        let decision = resident::after_last_window_where(self.windowless_allowed(hotkeys), decision);
        let graced = !self.user_interacted && (ran.as_secs_f64() < min) && self.globals.bool(QUIT_KEY) && hotkeys == 0;
        if graced && decision == AfterLast::StayResident {
            crate::host_base::log(resident::grace_note());
        }
        decision
    }

    /// THE one decision on being left with no window (C1, I1, I2): only while the icon can bring
    /// Termurion back, or one of `hotkeys` hotkey windows can. Closing the last window, burying the
    /// last session and every later pass (`resident_upkeep`) ask it. Windows and macOS always say yes.
    pub(super) fn windowless_allowed(&self, hotkeys: usize) -> bool {
        resident::may_be_windowless(crate::platform::notify::icon_state(), hotkeys)
    }

    /// Each pass while resident, after the icon's tick: the destroyed Wayland window is made again
    /// (unmapped, so the slot has a window as on the other systems: tool windows, launches and
    /// device recovery find it), and a Termurion nothing can reach any more (its icon failed or
    /// the panel left) makes a window again, or quits with no session left. It schedules no wake:
    /// the icon's loss arrives as an event (`UserEvent::TrayLost`), and a rescue whose window failed
    /// is tried again on a later pass, at most every 30 s.
    pub(super) fn resident_upkeep(&mut self, event_loop: &ActiveEventLoop) {
        if !self.resident {
            self.rescue_tried = None;
            return;
        }
        self.remake_if_gone(event_loop);
        let hotkeys = self.hotkey_windows().len();
        let panels = crate::egui_host::taskbar_windows().iter().any(|w| w.is_visible() != Some(false));
        let coming = !self.pending_windows.is_empty() || self.wake_owed;
        let icon = crate::platform::notify::icon_state();
        if !resident::stranded(true, icon, hotkeys, panels, coming) {
            self.rescue_tried = None;
            return;
        }
        if self.rescue_tried.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(30)) {
            return;
        }
        self.rescue_tried = Some(Instant::now());
        let how = resident::rescue(!self.tabs.is_empty(), self.buried.newest().is_some(), self.never_shown);
        crate::host_base::log(&format!("resident: nothing can bring Termurion back (icon {icon:?}), so {how:?}"));
        match how {
            resident::Rescue::WakeWithTabs => {
                self.wake_owed = true;
                self.pay_wake_owed(event_loop);
            }
            resident::Rescue::Disinter => {
                if let Some(id) = self.buried.newest() {
                    self.disinter(id);
                }
            }
            resident::Rescue::NewWindow => self.new_window(event_loop, None),
            resident::Rescue::Quit => self.quit_confirmed(event_loop),
        }
    }

    /// No window was ever made yet: requests from outside wait for the startup plan.
    pub(super) fn before_first_window(&self) -> bool {
        self.gpu.is_none() && self.resident_window_gone.is_none()
    }

    /// Wayland's destroyed resident window, made again when it is gone (a failure is logged and
    /// tried on the next pass that needs it).
    /// A failed remake is tried again at most every 30 s (review N10).
    pub(super) fn remake_if_gone(&mut self, event_loop: &ActiveEventLoop) {
        if !(self.resident && self.gpu.is_none() && self.resident_window_gone.is_some()) {
            return;
        }
        if self.remake_tried.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(30)) {
            return;
        }
        match self.remake_resident_window(event_loop) {
            Ok(()) => self.remake_tried = None,
            Err(e) => {
                self.remake_tried = Some(Instant::now());
                crate::host_base::log(&format!("resident: the window was not made again: {e}"));
            }
        }
    }

    /// The last window's last tab closed and Termurion stays: the sessions end, the host stays up, the
    /// empty state is written without freezing later saves (B7), and the window is hidden.
    pub(super) fn go_resident_after_last(&mut self) {
        self.save_pending_settings();
        self.write_empty_state();
        self.flush_notes();
        self.clear_undo();
        self.end_capture();
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
        // Checked before anything changes (item 12): a window without its GPU stays as it is.
        if self.resident || self.gpu.is_none() {
            return;
        }
        self.save_window_frame();
        // The window's hosted web views close with its state, not left in the slot that is dropped.
        if let Some(hwnd) = self.hwnd() {
            self.close_hosted_views(hwnd);
        }
        if self.win_style != crate::frames::NORMAL {
            self.apply_window_style(crate::frames::NORMAL, false);
        }
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
            // The window the next tab wakes is a plain one, not the last window's full screen,
            // maximized or Compact look (its slot starts fresh: style NORMAL).
            g.window.set_fullscreen(None);
            g.window.set_maximized(false);
            crate::platform::window_style::set_skip_taskbar(&g.window, true);
            g.window.set_visible(false);
        }
        // Wayland has no hide: the window goes, its device stays, and the next pass makes an
        // unmapped one in its place (`resident_upkeep`).
        if resident::resident_away(crate::platform::window_style::can_hide()) == resident::Away::Destroy {
            self.drop_resident_window();
        }
    }

    /// The resident window is a window again: back in the window list and the taskbar (the state is
    /// not touched: saves ran all along, B7).
    /// `hotkey`: the window becomes a hotkey window, which has no taskbar button.
    pub(super) fn leave_resident_for(&mut self, hotkey: bool) {
        if !self.resident {
            return;
        }
        self.resident = false;
        self.never_shown = false;
        self.remake_tried = None;
        self.pending_wake = None;
        // The screen reader's host comes back with the window (a hidden resident window has none:
        // `go_resident` dropped it with the window's registration); the next pass publishes.
        self.access_install_host();
        for key in OUTSIDE_KEYS {
            self.banners.clear(key);
        }
        if let Some(id) = self.current_window() {
            self.registry.open(windows_ui::key(id));
            self.refresh_window_list();
        }
        self.look_seen = None;
        if let Some(g) = &self.gpu {
            crate::platform::window_style::set_skip_taskbar(&g.window, hotkey);
        }
    }

    /// Shows the (former) resident window and brings it forward, with the activation token the
    /// click or the second start brought (GNOME raises a window only with one).
    pub(super) fn show_awake_window(&mut self) {
        let token = self.activation.take();
        if let Some(g) = &self.gpu {
            g.window.set_visible(true);
            g.window.set_minimized(false);
            if !matches!(crate::platform::window::activate(&g.window, token.as_deref()), Ok(true)) {
                g.window.focus_window();
            }
        }
        self.relayout();
    }

    /// B11, ONE function: every entry point that makes a tab while resident calls it FIRST. A tab of
    /// a profile (`Wake::Window`) is queued as a profile window, which `open_window_with` makes from the
    /// hidden one with all its steps (size, place, style, profile, transparency, space); anything else
    /// (`Wake::InPlace`: arrangement tabs, a recording, a buried session) wakes the window with the
    /// default profile's steps. True when the caller must stop (its tab is the window's to make).
    pub(super) fn wake_from_resident(&mut self, how: Wake) -> bool {
        if !self.resident {
            return false;
        }
        match how {
            Wake::Window(guid) => {
                // Every tab asked for before the window opens joins the one window (item 3).
                if windows_ui::queue_wake_tab(&mut self.pending_windows, &mut self.pending_wake, guid.map(str::to_string), &self.store.default_guid) {
                    let _ = self.proxy.lock().map(|p| p.send_event(UserEvent::Wake));
                }
                true
            }
            Wake::Tabs(tabs) => {
                self.pending_windows.push(windows_ui::NewWindow::Tabs(tabs));
                let _ = self.proxy.lock().map(|p| p.send_event(UserEvent::Wake));
                true
            }
            Wake::InPlace => {
                self.wake_in_place();
                false
            }
        }
    }

    /// The hidden window is a window again, with a fresh state, placed, sized, styled and shown as a
    /// new window with the default profile is (`open_window_with`'s steps for a window that is not
    /// an arrangement); the caller makes its tab next.
    fn wake_in_place(&mut self) {
        // No window to wake (Wayland destroyed it): the next Wake makes one and comes back here.
        if self.gpu.is_none() && self.resident_window_gone.is_some() {
            self.wake_owed = true;
            let _ = self.proxy.lock().map(|p| p.send_event(UserEvent::Wake));
            return;
        }
        self.leave_resident_for(false);
        let initial = crate::transparency::initial_use(None, self.store.default_profile());
        self.win_transparency = crate::transparency::WinTransparency::new(initial);
        self.apply_fonts();
        self.size_window(None);
        self.place_window();
        let (space, style) = (self.store.default_profile().int("Space"), self.store.default_profile().int("Window Type"));
        self.apply_space(space);
        if style != crate::frames::NORMAL {
            self.apply_window_style(style, false);
        }
        self.apply_window_profile(None);
        self.show_awake_window();
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
    pub(super) fn shutdown_decide(&mut self, hwnd: isize, end_now: &std::sync::atomic::AtomicBool) {
        use std::sync::atomic::Ordering;
        let never = self.globals.bool("NeverBlockSystemShutdown");
        // The question is already pending (a query that raced the window procedure's own check):
        // still held, and no second question (Task 8 M1).
        if crate::platform::logout::question_pending() {
            end_now.store(false, Ordering::Release);
            return;
        }
        let has_reasons = resident::reasons_collected(true, never) && !self.quit_reasons_now().is_empty();
        match resident::on_shutdown(never, has_reasons) {
            resident::ShutdownAnswer::EndNow => end_now.store(true, Ordering::Release),
            resident::ShutdownAnswer::AskFirst => {
                end_now.store(false, Ordering::Release);
                // Pending before anything is posted: every later query holds without asking.
                crate::platform::logout::begin_question();
                crate::platform::logout::raise_block(hwnd);
                let _ = self.proxy.lock().map(|p| p.send_event(UserEvent::ShutdownAsk));
            }
        }
    }

    /// Takes the block reason down (every path: OK, Cancel, the end of the session). Idempotent.
    pub(super) fn destroy_block_reason(&mut self) {
        crate::platform::logout::destroy_block();
    }

    /// The quit question for a held shutdown (`UserEvent::ShutdownAsk`), shown on a thread of its own
    /// so no handler is held while it is open (final review item 2): a session end meanwhile closes it
    /// and writes out. Nothing left to ask quits at once, as Quit does.
    pub(super) fn shutdown_query(&mut self, event_loop: &ActiveEventLoop) {
        if !crate::platform::logout::question_pending() {
            return;
        }
        let reasons = if resident::reasons_collected(true, self.globals.bool("NeverBlockSystemShutdown")) { self.quit_reasons_now() } else { Vec::new() };
        if reasons.is_empty() {
            return self.shutdown_answered(true, event_loop);
        }
        let keep = self.keep_jobs_on_quit();
        let proxy = self.proxy.clone();
        crate::platform::logout::ask_on_thread(move || close_ui::confirm_quit(None, true, &reasons, keep), move |ok| {
            let _ = proxy.lock().map(|p| p.send_event(UserEvent::ShutdownAnswer(ok)));
        });
    }

    /// The question's answer (`UserEvent::ShutdownAnswer`): OK takes the reason down and quits as Quit
    /// does, without asking again; Cancel takes it down and Termurion keeps running (Windows does not
    /// retry: the person starts the shutdown again, Decision). An answer after the session end wrote
    /// out (the question closed as Cancel) does nothing.
    pub(super) fn shutdown_answered(&mut self, ok: bool, event_loop: &ActiveEventLoop) {
        if !crate::platform::logout::end_question() {
            return;
        }
        self.destroy_block_reason();
        if ok {
            self.quit_confirmed(event_loop);
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

    /// B11, as ruled: ONE function wakes a resident window, and every entry point that makes a tab while
    /// resident (links, Jump List, launch, tray New Tab, API CreateTab, file opens, handoff, all through
    /// `add_tab`; arrangements as tabs, recordings and buried sessions on their own) calls it FIRST. It runs
    /// `open_window_with`'s steps on a fresh window state; nothing polls for it.
    #[test]
    fn every_tab_entry_wakes_a_resident_window_first() {
        let wake = body(production(OWN), "wake_from_resident");
        assert!(wake.contains("!self.resident") && wake.contains("queue_wake_tab(") && wake.contains("self.wake_in_place()"), "a profile tab opens through open_window_with, anything else wakes in place");
        let place = body(production(OWN), "wake_in_place");
        for step in ["leave_resident_for(false)", "WinTransparency::new", "apply_fonts()", "size_window(", "place_window()", "apply_space(", "apply_window_style(", "apply_window_profile(", "show_awake_window()"] {
            assert!(place.contains(step), "the in-place wake runs {step}");
        }
        let go = body(production(OWN), "go_resident");
        assert!(go.contains("set_fullscreen(None)") && go.contains("set_maximized(false)") && go.contains("frames::NORMAL"), "the hidden window leaves the last window's full screen, maximized or Compact look");
        assert!(!body(production(MOD), "about_to_wait").contains("wake_from_resident"), "nothing polls for the wake");
        for (src, name) in [(MOD, "add_tab"), (include_str!("arrange_ui.rs"), "restore_arrangement"), (include_str!("replay_ui.rs"), "import_recording"), (include_str!("bury_ui.rs"), "disinter")] {
            let b = body(production(src), name);
            let at = b.find("wake_from_resident(").unwrap_or_else(|| panic!("{name} wakes a resident window"));
            assert!(!b[..at].contains("self.tabs") && !b[..at].contains("start_pane") && !b[..at].contains("install_pane"), "{name} wakes before it makes anything");
        }
        let open = body(production(WINDOWS), "open_window_with");
        assert!(open.contains("let reused = self.resident;") && open.contains("leave_resident_for(hotkey)"), "a new window is the hidden one");
        let reopen = body(production(OWN), "reopen");
        let resident_branch = reopen.split("if self.resident {").nth(1).unwrap().split("match crate::presence::reopen_target").next().unwrap();
        assert!(resident_branch.contains("return;") && !resident_branch.contains("reopen_target"), "a hidden window is never focused");
        let presence = production(PRESENCE);
        assert!(presence.contains("presence::DockClick::Unhandled => self.reopen(event_loop)") && !presence.contains("Reopen::Focus(w) => self.focus_terminal_window(w)"), "the tray's click is the one reopen");
    }

    /// Review C1/I1/I2: ONE rule decides whether Termurion may be left with no window, and every way
    /// to that state asks it: the last close, the last bury, and each pass while resident (which
    /// also catches an icon that failed or went away later and brings a window back).
    #[test]
    fn every_way_to_no_window_asks_the_one_rule() {
        let own = production(OWN);
        assert!(body(own, "windowless_allowed").contains("resident::may_be_windowless(crate::platform::notify::icon_state(), hotkeys)"));
        let bury = body(production(include_str!("bury_ui.rs")), "bury_pane");
        let ask = bury.find("self.windowless_allowed(hotkeys)").expect("the last bury asks");
        assert!(ask < bury.find("self.buried.bury(").unwrap(), "before anything is buried");
        let upkeep = body(own, "resident_upkeep");
        for step in ["self.remake_if_gone(event_loop)", "resident::stranded(", "resident::rescue(", "self.disinter(id)", "self.quit_confirmed(event_loop)", "self.pay_wake_owed(event_loop)"] {
            assert!(upkeep.contains(step), "the upkeep runs {step}");
        }
        let m = production(MOD);
        assert!(body(m, "about_to_wait").contains("self.resident_upkeep(event_loop)"), "every pass");
        assert!(m.contains("UserEvent::TrayLost(why) => return self.tray_lost(why)"), "a lost icon wakes the loop");
        assert!(body(m, "render_frame").contains("if self.resident && !crate::platform::window_style::can_hide() {"), "Wayland's remade resident window is never drawn, so never mapped");
        // Requests from outside wait only for the startup plan, not for a gone resident window.
        assert!(body(production(include_str!("launch_ui.rs")), "receive_launch").contains("self.before_first_window()"));
        assert!(body(production(include_str!("ssh_url_app.rs")), "receive_url").contains("self.before_first_window()"));
        // I5: a failed remake buries the tabs again.
        let open = body(production(WINDOWS), "open_window_with");
        assert!(open.contains("if reused && self.gpu.is_none() {") && open.contains("self.rebury_tabs("));
    }

    /// Re-review round 2: a start that showed no window opens one instead of quitting (N3), a
    /// failed remake waits before the next try (N10), and an activation token no window took is
    /// dropped (N5).
    #[test]
    fn round_two_rescue_remake_and_tokens() {
        let own = production(OWN);
        let m = production(MOD);
        assert!(m.contains("if resident_start {\n                    self.resident = true;\n                    self.never_shown = true;"), "a resident start has shown no window");
        assert!(body(own, "leave_resident_for").contains("self.never_shown = false;"));
        let upkeep = body(own, "resident_upkeep");
        assert!(upkeep.contains("self.never_shown)") && upkeep.contains("resident::Rescue::NewWindow => self.new_window(event_loop, None)"));
        let remake = body(own, "remake_if_gone");
        let wait = remake.find("self.remake_tried.is_some_and(").expect("a failed remake waits");
        assert!(wait < remake.find("self.remake_resident_window(event_loop)").unwrap());
        assert!(m.contains("self.remake_tried = None;\n                self.remake_if_gone(event_loop);"), "a person's pick tries at once");
        let launch = production(include_str!("launch_ui.rs"));
        assert!(body(launch, "launch_request").contains("self.drop_unused_activation()"));
        assert!(body(launch, "drop_unused_activation").contains("self.pending_windows.is_empty() && !self.wake_owed"), "a window on its way keeps it");
        assert!(body(production(PRESENCE), "tray_clicked").contains("self.drop_unused_activation()"));
    }

    /// Wayland cannot hide a window: with a tray on the desktop the resident window is destroyed
    /// (its device kept) and made again by every wake; with none, the last close quits.
    #[test]
    fn a_resident_window_on_wayland_is_destroyed_and_made_again() {
        let after = body(production(OWN), "after_last_window_now");
        assert!(after.contains("after_last_window_where(self.windowless_allowed(hotkeys), decision)"));
        let go = body(production(OWN), "go_resident");
        let away = go.find("resident::Away::Destroy").expect("the destroy is decided by the pure rule");
        assert!(go.find("set_visible(false)").unwrap() < away && go[away..].contains("self.drop_resident_window()"), "after the hidden window's last steps");
        let w = production(WINDOWS);
        let drop_fn = body(w, "drop_resident_window");
        assert!(drop_fn.contains("self.prefs_gpu = Some(") && drop_fn.contains("menu::window_gone(&g.window)") && drop_fn.contains("self.resident_window_gone = Some("), "the device stays, the window is forgotten");
        let remake = body(w, "remake_resident_window");
        for step in ["create_window(", "init_gpu_from(", "self.rekey_window(old, window.id())", "self.install_menus()", "self.install_sizing()", "self.resident_window_gone = None"] {
            assert!(remake.contains(step), "the remade window gets {step}");
        }
        let open = body(w, "open_window_with");
        assert!(open.contains("if reused {\n                // Wayland destroyed the resident window: a new one takes its slot first.\n                break self.remake_resident_window(event_loop)"), "every new window from resident makes it first");
        let place = body(production(OWN), "wake_in_place");
        let owed = place.find("self.wake_owed = true").expect("an in-place wake with no window is owed");
        assert!(owed < place.find("self.leave_resident_for(false)").unwrap() && place[..owed].contains("self.gpu.is_none() && self.resident_window_gone.is_some()"));
        assert!(production(MOD).contains("UserEvent::Wake => {\n                self.pay_wake_owed(event_loop);"), "the Wake that follows pays it");
        let pay = body(w, "pay_wake_owed");
        assert!(pay.contains("remake_resident_window(event_loop)") && pay.contains("Wake::InPlace") && pay.contains("self.tabs.is_empty()"));
    }

    /// Final review item 3: three tabs asked for in one pass while resident (the Profiles window's Open
    /// in tabs, launch requests, links) make exactly ONE pending window holding the three profiles.
    #[test]
    fn several_wake_tabs_in_one_pass_make_one_window() {
        use super::super::windows_ui::{queue_wake_tab, NewWindow};
        use crate::settings_ui::Rest;
        let (mut pending, mut wake) = (Vec::new(), None);
        assert!(queue_wake_tab(&mut pending, &mut wake, Some("A".to_string()), "D"), "a window queued: wake the loop");
        assert!(matches!(pending.as_slice(), [NewWindow::Profile(Some(g))] if g == "A"), "one tab alone is a plain profile window");
        assert!(!queue_wake_tab(&mut pending, &mut wake, None, "D"), "joined: no second wake");
        assert!(!queue_wake_tab(&mut pending, &mut wake, Some("C".to_string()), "D"));
        assert_eq!(pending.len(), 1, "one window");
        assert!(matches!(&pending[0], NewWindow::ProfilesIn(f, Rest::Tabs(r)) if f == "A" && r == &["D".to_string(), "C".to_string()]), "three tabs, in order");
        assert_eq!(pending[0].queued_tabs(), 3);
        // A window queued before the wake is left alone.
        let (mut pending, mut wake) = (vec![NewWindow::Orphans(Vec::new())], None);
        queue_wake_tab(&mut pending, &mut wake, None, "D");
        queue_wake_tab(&mut pending, &mut wake, None, "D");
        assert_eq!(pending.len(), 2);
        assert!(matches!(&pending[1], NewWindow::ProfilesIn(f, Rest::Tabs(r)) if f == "D" && r.len() == 1));
        // A queue taken whole leaves the index pointing nowhere: the next tab queues a window again.
        pending.clear();
        assert!(queue_wake_tab(&mut pending, &mut wake, None, "D"), "a stale index never swallows a tab");
        // The wiring: the one wake function merges, the merge ends when the window opens, and the
        // callers that compare before and after count tabs.
        let wake_fn = body(production(OWN), "wake_from_resident");
        assert!(wake_fn.contains("if windows_ui::queue_wake_tab(&mut self.pending_windows, &mut self.pending_wake,"), "the wake merges, and wakes the loop for a new window");
        assert_eq!(production(MOD).matches("self.pending_wake = None;\n        for w in std::mem::take(&mut self.pending_windows)").count(), 1, "the drain forgets the index");
        assert!(body(production(OWN), "leave_resident_for").contains("self.pending_wake = None;"));
        let ssh = production(include_str!("ssh_url_app.rs"));
        assert!(ssh.contains("self.pending_tabs()") && !ssh.contains("self.pending_windows.len()"), "a second link in the pass keeps its session copy");
        assert!(!production(MOD).contains("self.pending_windows.len()"), "the add-tab button counts tabs");
        // N2: an as-tabs restore with nothing to restore never wakes (no flash, no frame saved).
        let restore = body(production(include_str!("arrange_ui.rs")), "restore_arrangement");
        let nothing = restore.find("if !has_tabs {").expect("nothing to restore is decided first");
        assert!(nothing < restore.find("wake_from_resident(").unwrap());
    }

    /// Final review item 10: a launch, file or Jump List request that opens nothing while resident tells
    /// the person through the notification area (no window would show a banner), and the refusal
    /// banners left from before are cleared when a window wakes.
    #[test]
    fn notes_while_resident_reach_the_person() {
        let note = body(production(include_str!("launch_ui.rs")), "outside_note");
        assert!(note.contains("if self.resident") && note.contains("crate::platform::notify::show_with(") && note.contains("self.banners.set(key, text)"));
        let launch = production(include_str!("launch_ui.rs")).replace(note, "");
        assert!(!launch.contains("self.banners.set("), "launch_ui: every banner goes through outside_note");
        assert_eq!(launch.matches("self.outside_note(").count(), 3, "ignored, arrangement gone, profile gone");
        let open = production(include_str!("open_file_ui.rs"));
        assert!(body(open, "open_files_ignored").contains("self.outside_note(") && body(open, "open_file_banner").contains("self.outside_note("));
        let jump = production(include_str!("jumplist_ui.rs"));
        for f in ["open_profile_tab", "open_recent_profile"] {
            let b = body(jump, f);
            assert!(!b.contains("self.banners.set(") && b.contains("self.outside_note("), "{f}");
        }
        let leave = body(production(OWN), "leave_resident_for");
        assert!(leave.contains("for key in OUTSIDE_KEYS") && leave.contains("self.banners.clear(key)"), "stale refusals go on wake");
    }

    /// Final review item 11 (iTerm2's bury, `PTYSession.m:15145`): with "Quit when all windows are
    /// closed" off, burying the last session hides the last window and Termurion stays resident with the
    /// session buried; with it on, the banner says why it stays. A buried session restored while
    /// resident comes back as the hidden window's tab.
    #[test]
    fn burying_the_last_session_goes_resident() {
        let bury = production(include_str!("bury_ui.rs"));
        let b = body(bury, "bury_pane");
        let gate = &b[b.find("if only && ").unwrap()..];
        assert!(gate.contains("self.globals.bool(QUIT_KEY)") && gate.contains("Termurion quits when its last window closes"), "the banner only when the key is on");
        assert!(b.contains("if only {\n                self.go_resident();"), "the last window goes resident with the session buried");
        let dis = body(bury, "disinter");
        assert!(dis.contains("Wake::Tabs(vec![tab])"), "while resident the session comes back as the hidden window's tab");
        let wake = body(production(OWN), "wake_from_resident");
        assert!(wake.contains("Wake::Tabs(tabs) =>") && wake.contains("NewWindow::Tabs(tabs)"));
        let help = include_str!("../../schema/global.toml");
        let quit = &help[help.find("key = \"QuitWhenAllWindowsClosed\"").unwrap()..];
        let quit = &quit[..quit.find("[[setting]]").unwrap()];
        assert!(quit.contains("OpenUntitledFile") || quit.contains("Open a new window when the icon is clicked"), "the click opens a window only with that setting");
    }

    /// Final review item 12: `go_resident` checks the GPU before anything changes, so a bail-out
    /// leaves the window as it was.
    #[test]
    fn go_resident_checks_before_it_changes_anything() {
        let go = body(production(OWN), "go_resident");
        let check = go.find("if self.resident || self.gpu.is_none() {").expect("checked first");
        assert!(check < go.find("self.save_window_frame()").unwrap() && check < go.find("self.forget_window(k)").unwrap());
    }

    /// The class guard: a new place that pushes a tab onto the window is looked at by a person. Each file
    /// below was checked: it only moves tabs between existing windows, restores into a window that is
    /// already awake, or is an entry point of the previous test.
    #[test]
    fn tab_making_sites_are_all_classified() {
        let known: [(&str, usize); 14] = [
            ("api_requests.rs", 3),
            ("arrange_ui.rs", 2),
            ("bury_ui.rs", 2),
            ("mod.rs", 1),
            ("pane_move_ui.rs", 1),
            ("replay_ui.rs", 1),
            ("restore_ui.rs", 1),
            ("tab_drag_ui.rs", 4),
            ("tab_groups_ui.rs", 1),
            ("tmux_tabs_ui.rs", 1),
            ("undo_ui.rs", 2),
            ("windows_ui.rs", 7),
            // A workgroup entry starts from a session in a tab of an awake window.
            ("workgroup_app.rs", 1),
            ("resident_ui.rs", 0),
        ];
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("app");
        let mut seen = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if !name.ends_with(".rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let code = text.split("\n#[cfg(test)]").next().unwrap();
            let n = ["self.tabs.push(", "self.tabs.insert(", "self.tabs.extend("].iter().map(|k| code.matches(k).count()).sum::<usize>();
            if n > 0 {
                seen.push((name, n));
            }
        }
        for (name, n) in &seen {
            let want = known.iter().find(|(k, _)| k == name).map(|(_, c)| *c);
            assert_eq!(want, Some(*n), "{name} makes tabs in {n} places: classify them (wake_from_resident first, or prove the window is awake)");
        }
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
        for never in ["drop_current_window", "platform::keys::", "register_hotkeys", "unregister", "close_current", "install_menus"] {
            assert!(!production(OWN).contains(never), "the resident window keeps its hook and hotkeys: {never}");
        }
        assert!(go.contains("set_visible(false)") && go.contains("set_skip_taskbar(&g.window, true)"));
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
    /// Termurion, not a window made "because nothing else would show".
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
        let notify = production(include_str!("../platform/windows/notify.rs"));
        assert!(notify.contains("RegisterWindowMessageW") && notify.contains("ON_TASKBAR.get()"), "TaskbarCreated reaches the app");
        let created = body(production(include_str!("presence_ui.rs")), "taskbar_created");
        assert!(created.contains("self.tray_on = false") && created.contains("self.tray_tick("), "the one owner adds it again");
        assert!(production(MOD).contains("UserEvent::TaskbarCreated => return self.taskbar_created()"));
        assert!(production(PRESENCE).contains("crate::resident::icon_wanted(presence::tray_wanted("));
        assert!(production(PRESENCE).contains("crate::platform::notify::set_resident(want)"));
    }

    /// Closing the last window by its close button, Close Tab or Close ends the undo capture with the
    /// sessions it holds (they end with the window, before the shells are killed): nothing dead or
    /// orphaned reaches the Undo Close stack, so the tray's Undo Close never restores them.
    #[test]
    fn closing_the_last_window_ends_the_undo_capture() {
        let go = body(production(OWN), "go_resident_after_last");
        assert!(go.contains("self.end_capture()") && !go.contains("stop_log"), "the capture ends through undo_ui");
        assert!(go.contains("self.clear_undo()"), "the stack is empty while resident");
        assert!(go.find("self.end_capture()").unwrap() < go.find("self.end_sessions_keep_host()").unwrap(), "before the shells are killed");
        let undo = body(production(include_str!("undo_ui.rs")), "end_capture");
        assert!(undo.contains("undo_capture.take()") && undo.contains("end_undone("), "the captured panes end for good");
    }

    /// A shutdown asks as a quit does, through the one subclass; the block reason stands only
    /// while there is something to ask and comes down on every path (a source check).
    #[test]
    fn a_shutdown_asks_through_the_block_reason_and_lets_it_go() {
        let src = include_str!("resident_ui.rs").split("#[cfg(test)]\nmod tests").next().unwrap();
        let decide = &src[src.find("fn shutdown_decide(").unwrap()..src.find("fn destroy_block_reason(").unwrap()];
        assert!(decide.contains("on_shutdown(never, has_reasons)") && decide.contains("quit_reasons_now()"), "the decision is the pure table over the quit reasons");
        let create = decide.find("raise_block(hwnd)").unwrap();
        assert!(create > decide.find("AskFirst").unwrap(), "a reason is made only for AskFirst");
        let pending = decide.find("begin_question()").unwrap();
        assert!(pending < decide.find("ShutdownAsk").unwrap(), "pending before the question is posted (M1)");
        assert!(decide.find("question_pending()").unwrap() < decide.find("quit_reasons_now()").unwrap(), "a pending question holds without deciding again");
        assert!(!decide.contains("confirm"), "no dialog inside the query");
        let ask = &src[src.find("fn shutdown_query(").unwrap()..src.find("fn shutdown_answered(").unwrap()];
        assert!(ask.contains("ask_on_thread(") && !ask.contains("confirm_close(") && !ask.contains("quit_confirmed"), "the question runs on its own thread, never inside the handler (item 2)");
        let answered = &src[src.find("fn shutdown_answered(").unwrap()..];
        let taken = answered.find("end_question()").unwrap();
        assert!(taken < answered.find("destroy_block_reason()").unwrap() && taken < answered.find("quit_confirmed").unwrap(), "OK and Cancel both take it down; a stale answer does nothing");
        let m = include_str!("mod.rs");
        assert!(m.contains("UserEvent::ShutdownAnswer(ok) => return self.shutdown_answered(ok, event_loop)"));
        let logoff = include_str!("logout_ui.rs");
        assert!(logoff.contains("self.destroy_block_reason()"), "the session end takes it down");
    }
}

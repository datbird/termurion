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
        self.close_hosted_views();
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
            use winit::platform::windows::WindowExtWindows;
            // The window the next tab wakes is a plain one, not the last window's full screen,
            // maximized or Compact look (its slot starts fresh: style NORMAL).
            g.window.set_fullscreen(None);
            g.window.set_maximized(false);
            g.window.set_skip_taskbar(true);
            g.window.set_visible(false);
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
    #[cfg(windows)]
    pub(super) fn shutdown_decide(&mut self, hwnd: isize, end_now: &std::sync::atomic::AtomicBool) {
        use std::sync::atomic::Ordering;
        let never = self.globals.bool("NeverBlockSystemShutdown");
        // The question is already pending (a query that raced the window procedure's own check):
        // still held, and no second question (Task 8 M1).
        if super::logout_ui::question_pending() {
            end_now.store(false, Ordering::Release);
            return;
        }
        let has_reasons = resident::reasons_collected(true, never) && !self.quit_reasons_now().is_empty();
        match resident::on_shutdown(never, has_reasons) {
            resident::ShutdownAnswer::EndNow => end_now.store(true, Ordering::Release),
            resident::ShutdownAnswer::AskFirst => {
                end_now.store(false, Ordering::Release);
                // Pending before anything is posted: every later query holds without asking.
                super::logout_ui::begin_question();
                super::logout_ui::raise_block(hwnd);
                let _ = self.proxy.lock().map(|p| p.send_event(UserEvent::ShutdownAsk));
            }
        }
    }

    /// Takes the block reason down (every path: OK, Cancel, the end of the session). Idempotent.
    pub(super) fn destroy_block_reason(&mut self) {
        #[cfg(windows)]
        super::logout_ui::destroy_block();
    }

    /// The quit question for a held shutdown (`UserEvent::ShutdownAsk`), shown on a thread of its own
    /// so no handler is held while it is open (final review item 2): a session end meanwhile closes it
    /// and writes out. Nothing left to ask quits at once, as Quit does.
    pub(super) fn shutdown_query(&mut self, event_loop: &ActiveEventLoop) {
        if !super::logout_ui::question_pending() {
            return;
        }
        let reasons = if resident::reasons_collected(true, self.globals.bool("NeverBlockSystemShutdown")) { self.quit_reasons_now() } else { Vec::new() };
        if reasons.is_empty() {
            return self.shutdown_answered(true, event_loop);
        }
        let text = crate::close_rules::quit_text(&reasons, self.keep_jobs_on_quit());
        let proxy = self.proxy.clone();
        super::logout_ui::ask_on_thread(move || close_ui::confirm_on_top("Quit Tandem?", &text), move |ok| {
            let _ = proxy.lock().map(|p| p.send_event(UserEvent::ShutdownAnswer(ok)));
        });
    }

    /// The question's answer (`UserEvent::ShutdownAnswer`): OK takes the reason down and quits as Quit
    /// does, without asking again; Cancel takes it down and Tandem keeps running (Windows does not
    /// retry: the person starts the shutdown again, Decision). An answer after the session end wrote
    /// out (the question closed as Cancel) does nothing.
    pub(super) fn shutdown_answered(&mut self, ok: bool, event_loop: &ActiveEventLoop) {
        if !super::logout_ui::end_question() {
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
        assert!(note.contains("if self.resident") && note.contains("crate::notify::show_with(") && note.contains("self.banners.set(key, text)"));
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
    /// closed" off, burying the last session hides the last window and Tandem stays resident with the
    /// session buried; with it on, the banner says why it stays. A buried session restored while
    /// resident comes back as the hidden window's tab.
    #[test]
    fn burying_the_last_session_goes_resident() {
        let bury = production(include_str!("bury_ui.rs"));
        let b = body(bury, "bury_pane");
        let gate = &b[b.find("if only && ").unwrap()..];
        assert!(gate.contains("self.globals.bool(QUIT_KEY)") && gate.contains("Tandem quits when its last window closes"), "the banner only when the key is on");
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
        let known: [(&str, usize); 13] = [
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

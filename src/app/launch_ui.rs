//! Requests from another `termurion.exe` (m13v1 Task 1): the command line a second start was given,
//! handed over the one launch pipe (`url_launch`). Every request is untrusted (another program of
//! the user may have sent it): a flood cap, a closed set of kinds, a profile or arrangement found
//! only in Tandem's own store, and none runs a command.

use std::time::Instant;

use winit::event_loop::ActiveEventLoop;

use super::App;
use crate::launch::Request;

impl App {
    /// A request from the pipe: capped like URLs are, queued until the first window exists.
    pub(super) fn receive_launch(&mut self, r: Request, event_loop: &ActiveEventLoop) {
        // Explorer opens many files as many processes: they gather in a short window and open
        // together (`open_file::Batch`, with its own bound and one "N files ignored" banner), so
        // the launch flood cap never drops them silently.
        if let Request::Open { paths } = r {
            self.open_batch.push(paths, Instant::now());
            return;
        }
        if !self.launch_flood.admit(Instant::now()) {
            return self.ignored_launch_banner();
        }
        if self.gpu.is_none() {
            if self.launch_queue.len() >= crate::url_launch::QUEUE_MAX {
                self.launch_flood.ignore();
                return self.ignored_launch_banner();
            }
            self.launch_queue.push(r);
            return;
        }
        self.launch_request(r, event_loop);
    }

    /// The requests that waited for the first window (this start's own, and any that arrived
    /// first), run after the startup plan.
    pub(super) fn open_launch_queue(&mut self, event_loop: &ActiveEventLoop) {
        for r in std::mem::take(&mut self.launch_queue) {
            self.launch_request(r, event_loop);
        }
        self.ignored_launch_banner();
    }

    fn ignored_launch_banner(&mut self) {
        let n = self.launch_flood.ignored();
        if n > 0 {
            self.outside_note("launch-ignored", crate::launch::ignored_banner(n));
        }
    }

    /// One request, now. `Open` gathers into `open_batch` (Task 2), `Url` (which arrives as `UserEvent::OpenUrl`) is
    /// `receive_url`, `Embedding` is Task 5: each does nothing here until it lands.
    pub(super) fn launch_request(&mut self, r: Request, event_loop: &ActiveEventLoop) {
        match r {
            Request::Reopen => self.dock_click_now(event_loop),
            Request::NewWindow { guid } => {
                if self.known_profile(guid.as_deref()) {
                    self.new_window(event_loop, guid.as_deref());
                }
            }
            Request::NewTab { guid } => {
                if !self.known_profile(guid.as_deref()) {
                    return;
                }
                if self.tabs.is_empty() {
                    return self.new_window(event_loop, guid.as_deref());
                }
                if let Some(w) = self.last_window {
                    self.enter(w);
                }
                self.new_tab(guid.as_deref());
                self.bring_forward();
                self.redraw();
            }
            Request::Arrangement { name } => {
                if self.arrangements().get(&name).is_none() {
                    return self.outside_note("launch", format!("The arrangement \u{201c}{}\u{201d} no longer exists.", crate::inert_files::shown_name(&name)));
                }
                if let Some(w) = self.last_window {
                    self.enter(w);
                }
                self.restore_arrangement(&name, false);
                self.bring_forward();
                self.redraw();
            }
            Request::Open { paths } => self.open_batch.push(paths, Instant::now()),
            Request::Url { .. } | Request::Embedding => {}
        }
    }

    /// A request from outside that opened nothing says why: a banner on the window, or while resident
    /// (no window shows) a notification, so it is neither lost nor stale when a window next opens.
    pub(super) fn outside_note(&mut self, key: &'static str, text: String) {
        if self.resident {
            if let Err(e) = crate::notify::show_with("Tandem", &text, true) {
                crate::host::log(&format!("{key}: {text} (no notification: {e})"));
            }
            return;
        }
        self.banners.set(key, text);
        self.redraw();
    }

    /// A profile named by a request exists in Tandem's own store (none named is the default
    /// profile); an unknown guid shows a banner and opens nothing, never the default silently (P22).
    fn known_profile(&mut self, guid: Option<&str>) -> bool {
        let Some(g) = guid else { return true };
        if self.store.get(g).is_some() {
            return true;
        }
        self.outside_note("launch", format!("The profile \u{201c}{}\u{201d} no longer exists.", crate::inert_files::shown_name(g)));
        false
    }
}

#[cfg(test)]
mod tests {
    /// The 13u startup a second Tandem must never reach: the keyboard hook, the global remap,
    /// the hotkeys (app, profile and session), the forced layout and the notification icon.
    const KEYS_STARTUP: &[&str] = &["winkeys::install", "set_remap_global", "set_hotkey_sink", "register_hotkeys", "input_locale::", "locale_focus", "notify::add", "notify::show"];

    /// `run()` up to the event loop: every way a forwarding or losing process ends lies in it.
    fn run_before_loop() -> &'static str {
        let m = include_str!("mod.rs");
        let run = m.split("pub fn run()").nth(1).expect("run");
        run.split("event_loop.run_app").next().expect("run_app")
    }

    /// 13u S4: a process that cannot bind the launch pipe hands its request over and ends before
    /// the event loop runs, so it never installs a second keyboard hook, hotkeys or tray icon.
    #[test]
    fn a_second_process_installs_no_hook() {
        let run = run_before_loop();
        let failed = run.split("launch: not listening").nth(1).and_then(|r| r.split("let wake = proxy.clone();").next()).expect("the bind failure branch");
        assert!(failed.contains("return;"), "the loser of the pipe ends");
        // Nothing of 13u's startup runs before the event loop, so every early return (a
        // forwarded launch, a lost pipe, a refused command line) leaves no hook, hotkey, forced
        // layout or icon behind.
        for call in KEYS_STARTUP {
            assert!(!run.contains(call), "{call} runs before the launch pipe decided this process may stay");
        }
        // The hook and the hotkeys come only from install_menus, which runs on the first window.
        let menu = include_str!("menu.rs");
        let live = menu.split("\n#[cfg(test)]").next().unwrap();
        let install = live.split("fn install_menus").nth(1).expect("install_menus");
        let body = install.split("\n    pub(super) fn ").next().unwrap();
        assert!(body.contains("winkeys::install") && body.contains("register_hotkeys"), "the hook and hotkeys are installed with the first window's menus");
        assert_eq!(live.matches("winkeys::install(").count(), 1, "one hook install site");
        let m = include_str!("mod.rs");
        let live_mod = m.split("\n#[cfg(test)]").next().unwrap();
        assert!(!live_mod.contains("winkeys::install("), "no hook install in mod.rs");
        // The listener is bound before the event loop runs, so the loser never reaches a window.
        assert!(run.contains("url_launch::listen("), "the pipe is bound before the first window");
    }

    /// 13u S6: a process that forwards its request and ends has nothing for the one process
    /// teardown to undo (13u's startup comes later, above), and the teardown runs after the
    /// event loop on every way it ends, so every 13v1 exit is covered by `teardown_process`.
    #[test]
    fn every_launch_exit_is_covered_by_the_one_teardown() {
        let run = run_before_loop();
        assert!(run.matches("AfterHand::Exit => return").count() >= 2, "the URL and the command-line forwards both end here");
        let m = include_str!("mod.rs");
        let after = m.split("event_loop.run_app").nth(1).expect("run_app");
        let tail = after.split("\nimpl App").next().unwrap();
        assert!(tail.contains("teardown_process();"), "the end of the event loop runs the one teardown");
        let teardown = m.split("pub(crate) fn teardown_process()").nth(1).and_then(|r| r.split("\n}").next()).expect("teardown_process");
        for part in ["winkeys::uninstall", "input_locale::", "notify::remove"] {
            assert!(teardown.contains(part), "the teardown undoes {part}");
        }
        // WM_ENDSESSION (13q's logout_ui, the one end-of-session path) runs it too.
        assert!(include_str!("logout_ui.rs").contains("teardown_process()"));
    }

    /// 13u S5: a session hotkey with no session (Tandem without a window, or every session gone)
    /// picks nothing, so it neither reveals a pane nor wakes a window.
    #[test]
    fn a_session_hotkey_with_no_session_does_nothing() {
        let none: &[crate::layout::PaneId] = &[];
        assert_eq!(crate::session_hotkeys::pick(none, None, none), None);
        let (first, background) = crate::session_hotkeys::focus_state(&[(false, None)]);
        assert_eq!(crate::session_hotkeys::pick(none, first, &background), None);
    }

    /// B1: the launch channel is the one URL pipe: no second pipe module, no second name.
    #[test]
    fn one_channel_serves_every_request() {
        assert!(!std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("instance_win.rs").exists());
        let pipe = include_str!("../winpipe.rs");
        assert!(!pipe.contains("tandem-app-"));
    }
}

//! Requests from another `tandem.exe` (m13v1 Task 1): the command line a second start was given,
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
            self.banners.set("launch-ignored", crate::launch::ignored_banner(n));
            self.redraw();
        }
    }

    /// One request, now. `Open` is Task 2, `Url` (which arrives as `UserEvent::OpenUrl`) is
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
                    self.banners.set("launch", format!("The arrangement \u{201c}{}\u{201d} no longer exists.", crate::inert_files::shown_name(&name)));
                    return self.redraw();
                }
                if let Some(w) = self.last_window {
                    self.enter(w);
                }
                self.restore_arrangement(&name, false);
                self.bring_forward();
                self.redraw();
            }
            Request::Open { .. } | Request::Url { .. } | Request::Embedding => {}
        }
    }

    /// A profile named by a request exists in Tandem's own store (none named is the default
    /// profile); an unknown guid shows a banner and opens nothing, never the default silently (P22).
    fn known_profile(&mut self, guid: Option<&str>) -> bool {
        let Some(g) = guid else { return true };
        if self.store.get(g).is_some() {
            return true;
        }
        self.banners.set("launch", format!("The profile \u{201c}{}\u{201d} no longer exists.", crate::inert_files::shown_name(g)));
        self.redraw();
        false
    }
}

#[cfg(test)]
mod tests {
    /// 13u S4: a process that cannot bind the launch pipe hands its request over and ends before
    /// the event loop runs, so it never installs a second keyboard hook, hotkeys or tray icon.
    #[test]
    fn a_second_process_installs_no_hook() {
        let m = include_str!("mod.rs");
        let run = m.split("pub fn run()").nth(1).expect("run");
        let failed = run.split("launch: not listening").nth(1).and_then(|r| r.split("let wake = proxy.clone();").next()).expect("the bind failure branch");
        assert!(failed.contains("return;"), "the loser of the pipe ends");
        assert!(!failed.contains("run_app"));
    }

    /// B1: the launch channel is the one URL pipe: no second pipe module, no second name.
    #[test]
    fn one_channel_serves_every_request() {
        assert!(!std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("instance_win.rs").exists());
        let pipe = include_str!("../winpipe.rs");
        assert!(!pipe.contains("tandem-app-"));
    }
}

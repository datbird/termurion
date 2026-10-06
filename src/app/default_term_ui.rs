//! Tandem as the Windows default terminal, the app side (m13v1 Task 5): the two menu items, the
//! handoff class while Tandem is the default, and the pane a handed-off console becomes. The
//! handoff itself is untrusted (`handoff_win`, `default_term`); by the time it reaches here it
//! has been vetted, and it is still refused while Tandem is closing.

use winit::application::ApplicationHandler;
use winit::event_loop::ActiveEventLoop;

use super::scripts_alerts::{ask, Then};
use super::*;
use crate::default_term as dt;
use crate::handoff_win::Handoff;
use crate::reg::{apply, HkcuRegistry, Registry};

impl App {
    /// Windows Terminal's folder of app aliases: where `wt.exe` is.
    fn windows_apps() -> Option<PathBuf> {
        std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Microsoft").join("WindowsApps"))
    }

    fn default_term_alert(&mut self, heading: &str, text: &str) {
        self.ask_scripts(ask(heading, text, &[crate::api_auth::OK], None, crate::warnings::Kind::Persistent, Then::Nothing));
    }

    /// Make Tandem Default Term: alerts and changes nothing without Windows Terminal or the pack;
    /// else saves the pair now (unless it is already Tandem's), writes the class and the pair,
    /// and registers the handoff class.
    pub(super) fn make_default_terminal(&mut self) {
        let wt = Self::windows_apps().is_some_and(|d| dt::console_host_present(&d));
        if let Some(text) = dt::make_blocked(wt, self.api.pack.is_some()) {
            let heading = if wt { dt::PACK_MISSING_HEADING } else { dt::NEEDS_WT_HEADING };
            return self.default_term_alert(heading, text);
        }
        let Ok(exe) = std::env::current_exe() else { return };
        let mut r = HkcuRegistry;
        let now = dt::current(&r);
        let result = (|| {
            if dt::should_save(&now) {
                apply(&mut r, &dt::save_entries(&now))?;
            }
            apply(&mut r, &dt::make_tandem_entries(&exe))
        })();
        match result {
            Ok(_) => self.banners.set("default-terminal", "Tandem is now the default terminal.".to_string()),
            Err(e) => self.banners.set("default-terminal", format!("Could not make Tandem the default terminal: {e}")),
        }
        self.sync_handoff_server();
        self.refresh_default_item();
        self.redraw();
    }

    /// Make Terminal Default Term: puts back the pair saved before (only while the pair is still
    /// Tandem's; one chosen since stays), removes Tandem's class and the saved pair, and revokes the server.
    pub(super) fn unmake_default_terminal(&mut self) {
        let mut r = HkcuRegistry;
        let now = dt::current(&r);
        if dt::is_tandem(&now) {
            let saved = dt::saved(&r);
            if let Err(e) = apply(&mut r, &dt::restore_entries(saved)) {
                self.banners.set("default-terminal", format!("Could not restore the default terminal: {e}"));
                return self.redraw();
            }
        }
        let _ = r.delete_tree(&dt::class_key());
        let _ = r.delete_tree(dt::SAVED_KEY);
        self.banners.set("default-terminal", "Windows decides the default terminal again.".to_string());
        self.sync_handoff_server();
        self.refresh_default_item();
        self.redraw();
    }

    /// The menu item follows the registry (`AD:481-482`).
    pub(super) fn refresh_default_item(&mut self) {
        let on = dt::make_enabled(&HkcuRegistry);
        for bar in self.menu_bars_mut() {
            if let Some(item) = &bar.make_default {
                item.set_enabled(on);
            }
        }
    }

    /// The handoff class is registered exactly while Tandem is the default and has the pack.
    pub(super) fn sync_handoff_server(&mut self) {
        let want = self.api.pack.is_some() && dt::is_tandem(&dt::current(&HkcuRegistry));
        match (want, self.handoff.is_some()) {
            (true, false) => {
                let (Some(pack), close) = (self.api.pack, self.api.close) else { return };
                let proxy = self.proxy.clone();
                let send = Box::new(move |h: Handoff| {
                    // A loop that is gone drops the event, and with it the handoff, which closes its handles.
                    let _ = proxy.lock().map(|p| p.send_event(UserEvent::Handoff(h)));
                });
                match crate::handoff_win::Server::register(send, pack, close) {
                    Ok(s) => self.handoff = Some(s),
                    Err(e) => {
                        crate::host::log(&format!("handoff: not registered: {e}"));
                        self.banners.set("handoff", "Tandem could not take terminal hand-offs from Windows.".to_string());
                    }
                }
            }
            (false, true) => self.handoff = None,
            _ => {}
        }
    }

    /// One banner for the calls the gate refused.
    pub(super) fn handoff_banner_tick(&mut self) {
        let n = self.handoff.as_ref().map_or(0, |s| s.take_ignored());
        if n > 0 {
            self.banners.set("handoff-ignored", dt::handoff_banner(n));
            self.redraw();
        }
    }

    /// A console arrived: a tab in the current window, a window when there is none (an
    /// `-Embedding` start makes its first window here). A closing Tandem refuses it; dropping
    /// the value closes what it owns.
    pub(super) fn handoff_arrived(&mut self, h: Handoff, event_loop: &ActiveEventLoop) {
        if event_loop.exiting() {
            return;
        }
        self.handoffs_seen += 1;
        if self.gpu.is_none() {
            if self.embedding.is_some() && self.start_handoff.is_none() {
                self.start_handoff = Some(h);
                self.embedding = None;
                self.resumed(event_loop);
                // The first window took it, or it could not start: nothing is kept.
                self.start_handoff = None;
            } else if self.handoff_queue.len() < dt::MAX_LIVE {
                self.handoff_queue.push(h);
            }
            return;
        }
        self.open_handoff(h, event_loop);
    }

    /// The handoffs that arrived before the first window.
    pub(super) fn open_handoff_queue(&mut self, event_loop: &ActiveEventLoop) {
        for h in std::mem::take(&mut self.handoff_queue) {
            self.open_handoff(h, event_loop);
        }
    }

    fn open_handoff(&mut self, h: Handoff, event_loop: &ActiveEventLoop) {
        self.start_handoff = Some(h);
        if self.tabs.is_empty() || self.last_window.is_none() || self.new_tab_opens_window(None) {
            if let Err(e) = self.open_window(event_loop, None) {
                self.banners.set("window", format!("Could not open a window: {e}"));
            }
        } else {
            if let Some(w) = self.last_window {
                self.enter(w);
            }
            self.add_tab(None);
        }
        // Not taken (a window or tab that did not start): the value closes its handles here.
        if self.start_handoff.take().is_some() {
            self.banners.set("handoff", "A console handed to Tandem could not be shown.".to_string());
        }
        self.bring_forward();
        self.redraw();
    }

    /// The pane a handed-off console becomes: its packed pseudoconsole, resized to the pane, titled
    /// with the startup title until the program sets one. It is never hosted in tandem-host.
    pub(super) fn pane_from_handoff(&mut self, id: PaneId, p: Profile, mut h: Handoff, cols: u16, rows: u16) -> Result<(Pane, Profile), String> {
        let title = std::mem::take(&mut h.title);
        let (hpc, input, output, client, live) = h.take();
        let mut pty = crate::pty::from_handoff(&self.api, hpc as _, input as _, output as _, client as _, self.notify.clone()).map_err(|e| format!("Could not take the handed-off console: {e}"))?;
        if let Some(l) = live {
            pty.hold(Box::new(l));
        }
        let _ = pty.resize(cols.max(2), rows.max(1));
        let mut pane = Pane::from_pty(id, pty, cols, rows);
        if !title.is_empty() {
            pane.screen.set_title(Some(title));
        }
        pane.profile = p.guid();
        Ok((pane, p))
    }
}

#[cfg(test)]
mod tests {
    /// The item that makes Tandem the default reads the registry, never writes it from a test:
    /// the two commands exist, are done, and every registry write goes through `reg`.
    #[test]
    fn the_two_items_are_wired() {
        for sel in ["makeDefaultTerminal:", "unmakeDefaultTerminal:"] {
            let item = crate::menus::items().iter().find(|i| i.selector == sel).expect(sel);
            assert_eq!(crate::commands::status(item), crate::commands::Status::Done, "{sel}");
        }
        let menu = include_str!("menu.rs");
        assert!(menu.contains("\"makeDefaultTerminal:\" => self.make_default_terminal()"));
        assert!(menu.contains("\"unmakeDefaultTerminal:\" => self.unmake_default_terminal()"));
        let me = include_str!("default_term_ui.rs");
        let live = me.split("\n#[cfg(test)]").next().unwrap();
        assert!(!live.contains("RegSetValue") && !live.contains("HKEY_"), "only reg::HkcuRegistry writes");
    }

    /// B9: the class is registered only while Tandem is the default and the pack exists, and the
    /// closing flag is raised by the one teardown.
    #[test]
    fn the_handoff_class_follows_the_default_and_the_pack() {
        let me = include_str!("default_term_ui.rs");
        let sync = me.split("fn sync_handoff_server").nth(1).unwrap().split("\n    }\n").next().unwrap();
        assert!(sync.contains("self.api.pack.is_some() && dt::is_tandem("));
        let m = include_str!("mod.rs");
        let teardown = m.split("pub(crate) fn teardown_process()").nth(1).unwrap().split("\n}").next().unwrap();
        assert!(teardown.contains("handoff_win::begin_closing()"));
        // S4: an -Embedding start that cannot take the pipe ends before any hook is installed.
        let run = m.split("pub fn run()").nth(1).unwrap().split("event_loop.run_app").next().unwrap();
        assert!(run.contains("probe_running()") && run.contains("if embedding {"));
    }
}

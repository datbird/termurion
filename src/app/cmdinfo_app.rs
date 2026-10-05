//! Command Info in the app (`TVA:616-645`, `CIV`): opening the window for a prompt's command
//! (Win-click on it) and what its buttons do. The command, the directory, the host and the user
//! are text a program wrote between its own prompt marks (B5): the buttons that type into the
//! pane (write, the snippet that is typed later) go through `cmdinfo::writable`, and the one
//! that opens the Composer through `cmdinfo::composer_text`; copies and the URL are clipboard
//! text only.

use super::*;
use crate::cmdinfo::{self, Info};
use crate::cmdinfo_ui::{Act, CmdInfoEvent, CmdInfoWindow};

/// The banner for a session that cannot take text (`13q`: an ended pane takes no input).
const ENDED: &str = "The session has ended.";

impl App {
    /// Win-click on a prompt's command: the Command Info window at the click (`at`, window
    /// pixels). A mark that is gone shows nothing.
    pub(super) fn show_command_info(&mut self, pane: PaneId, mark: u32, at: (f32, f32), event_loop: &ActiveEventLoop) {
        self.cmdinfo = None;
        let Some((info, started)) = self.command_info(pane, mark) else { return };
        let dark = self.dark();
        let work = self.monitor_rects().map(|(w, _)| w);
        let Some(g) = self.gpu.as_ref() else { return };
        let origin = g.window.inner_position().unwrap_or_default();
        let at = (f64::from(origin.x) + f64::from(at.0), f64::from(origin.y) + f64::from(at.1));
        match CmdInfoWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, pane, info, started, at, work, dark) {
            Ok(w) => self.cmdinfo = Some(w),
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// What changes while a command runs: its code, its end, and whether its program still runs
    /// (S5: an ended or restarted session leaves a mark with no code and no end, which is not
    /// running). None when the mark or the pane is gone.
    fn command_state(&self, pane: PaneId, mark: u32) -> Option<(Option<i32>, Option<std::time::SystemTime>, bool)> {
        let p = self.panes.get(&pane)?;
        let m = p.screen.command_record(mark)?;
        let running = p.exited.is_none() && !p.ended && m.code.is_none() && p.screen.command_is_running(mark);
        Some((m.code, m.end, running))
    }

    /// The window's fields for a command mark and its "Started at" text (`TVA:616-645`).
    pub(super) fn command_info(&self, pane: PaneId, mark: u32) -> Option<(Info, Option<String>)> {
        let p = self.panes.get(&pane)?;
        let m = p.screen.command_mark(mark)?;
        let (code, end, running) = self.command_state(pane, mark)?;
        let (output_bytes, output_lines) = cmdinfo::output_stats(p.screen.output_text(mark).as_deref());
        let local = !self.remote_text(pane) && m.host.as_deref().is_none_or(|h| self.host_is_this_machine(h));
        let info = Info {
            mark,
            command: m.command.clone(),
            directory: m.directory.clone(),
            host: m.host.clone(),
            user: m.user.clone(),
            code,
            start: m.start,
            end,
            output_bytes,
            output_lines,
            running,
            local,
        };
        Some((info, m.start.map(started_text)))
    }

    /// Ticks the open window: closes it with its pane or its mark, follows the command to its
    /// end, and asks for the next frame while it ticks (P15). The time the next one is due.
    pub(super) fn cmdinfo_tick(&mut self) -> Option<Instant> {
        let w = self.cmdinfo.as_ref()?;
        let (pane, mark) = (w.pane, w.mark());
        let Some((code, end, running)) = self.command_state(pane, mark) else {
            self.cmdinfo = None;
            return None;
        };
        let changed = {
            let info = self.cmdinfo.as_ref()?.info();
            (info.code, info.end, info.running) != (code, end, running)
        };
        // The output is read once more when the command has ended; never on every frame.
        let output = (changed && !running).then(|| {
            let text = self.panes.get(&pane).and_then(|p| p.screen.output_text(mark));
            cmdinfo::output_stats(text.as_deref())
        });
        let w = self.cmdinfo.as_mut()?;
        if changed {
            w.refresh(code, end, running, output);
        }
        w.tick(Instant::now())
    }

    /// A window event for Command Info. False for other windows.
    pub(super) fn cmdinfo_event(&mut self, window: winit::window::WindowId, event: &WindowEvent) -> bool {
        let Some(w) = self.cmdinfo.as_mut().filter(|w| w.id() == window) else { return false };
        match w.event(event) {
            CmdInfoEvent::None => {}
            CmdInfoEvent::Close => self.cmdinfo = None,
            CmdInfoEvent::Act(act) => self.cmdinfo_act(act),
        }
        self.redraw();
        true
    }

    /// What a button does.
    fn cmdinfo_act(&mut self, act: Act) {
        let Some(w) = self.cmdinfo.as_ref() else { return };
        let (pane, info) = (w.pane, w.info().clone());
        match act {
            Act::WriteCommand => self.cmdinfo_write(pane, &info.command),
            Act::WriteDirectory => match info.directory.as_deref() {
                Some(d) => self.cmdinfo_write(pane, d),
                None => {}
            },
            Act::CopyCommand => self.cmdinfo_copy(info.command),
            Act::CopyDirectory => {
                if let Some(d) = info.directory {
                    self.cmdinfo_copy(d);
                }
            }
            Act::CopyUrl => self.cmdinfo_copy(cmdinfo::command_url(&info.command, info.directory.as_deref(), info.host.as_deref(), info.user.as_deref(), info.local)),
            Act::CopyOutput => {
                let text = self.panes.get(&pane).and_then(|p| p.screen.output_text(info.mark)).unwrap_or_default();
                self.cmdinfo_copy(text);
            }
            Act::SelectOutput => {
                if let Some(r) = self.panes.get(&pane).and_then(|p| p.screen.output_range_of(info.mark)) {
                    self.select(pane, r);
                }
            }
            Act::AddSnippet => {
                self.cmdinfo_add_snippet(&info.command);
            }
            Act::Composer => {
                let shell = self.panes.get(&pane).and_then(|p| p.own().shell.shell.clone());
                match cmdinfo::composer_text(&info.command, info.directory.as_deref(), shell.as_deref()) {
                    Ok(text) => {
                        self.cmdinfo = None;
                        self.open_composer_with(pane, &text, crate::keyaction::Escaping::None);
                    }
                    Err(why) => self.banners.set("action", why),
                }
            }
        }
    }

    /// Types `text` into the pane through the guard (B5): a control character or a very long
    /// text is refused with a banner, an ended session says so (13q).
    fn cmdinfo_write(&mut self, pane: PaneId, text: &str) {
        let text = match cmdinfo::writable(text) {
            Ok(t) => t.to_string(),
            Err(why) => return self.banners.set("action", why),
        };
        let Some(p) = self.panes.get_mut(&pane) else { return };
        if p.exited.is_some() || p.ended {
            return self.banners.set("action", ENDED);
        }
        p.scroll(Scroll::Bottom);
        // Broadcast input applies, as `commandInfoSend` writes to the session.
        self.user_write(pane, text.as_bytes(), crate::broadcast::Write::SendText);
    }

    pub(super) fn cmdinfo_copy(&mut self, text: String) {
        if let Some(cb) = &mut self.clipboard {
            if let Err(e) = cb.set_text(text) {
                self.banners.set("clipboard", format!("Copy failed: {e}"));
            }
        }
    }

    /// "Add Command as Snippet" (`CIV:334-341`): a snippet titled and valued with the command,
    /// no tags, nothing escaped. The snippet is typed later by whoever picks it, so its text
    /// passes the same guard as a write.
    pub(super) fn cmdinfo_add_snippet(&mut self, command: &str) -> bool {
        let command = match cmdinfo::writable(command) {
            Ok(c) => c.to_string(),
            Err(why) => {
                self.banners.set("action", why);
                return false;
            }
        };
        let snippet = crate::snippets::Snippet {
            title: command.clone(),
            value: command,
            guid: uuid::Uuid::new_v4().to_string().to_uppercase(),
            tags: Vec::new(),
            escaping: crate::keyaction::Escaping::None,
            version: crate::snippets::VERSION,
        };
        self.snippets.snippets.push(snippet);
        self.save_snippets();
        if let Some(w) = self.cmdinfo.as_mut() {
            w.toast_snippet_added();
        }
        true
    }
}

/// "Started at <time> on <date>" for a command's start (`CIV:266-283`): the Windows user's
/// time format with seconds, the date as `MMM d, yyyy` with English month names (Decision).
fn started_text(start: std::time::SystemTime) -> String {
    let secs = start.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let parts = crate::stamps::local(secs);
    cmdinfo::started_text(&crate::stamps::long_time(&parts), &crate::stamps::month_day_year(&parts))
}

#[cfg(test)]
mod tests {
    const SRC: &str = include_str!("cmdinfo_app.rs");
    const SEMANTIC: &str = include_str!("semantic_ui.rs");

    fn code() -> &'static str {
        &SRC[..SRC.find("#[cfg(test)]").unwrap()]
    }

    /// B5: program text reaches the terminal, the snippets and the Composer only through the guard.
    #[test]
    fn text_reaches_the_terminal_only_through_the_guard() {
        let code = code();
        assert_eq!(code.matches("user_write(").count(), 1, "one write, in cmdinfo_write");
        assert!(!code.contains(".input("));
        let write = &code[code.find("fn cmdinfo_write").unwrap()..];
        let (guard, ended, send) = (write.find("cmdinfo::writable").unwrap(), write.find("ENDED").unwrap(), write.find("user_write(").unwrap());
        assert!(guard < ended && ended < send, "refuse the text, then an ended session, then write");
        assert!(code.find("composer_text(").unwrap() < code.find("open_composer_with(").unwrap());
        let snippet = &code[code.find("fn cmdinfo_add_snippet").unwrap()..];
        assert!(snippet.find("cmdinfo::writable").unwrap() < snippet.find("snippets.snippets.push").unwrap());
    }

    /// B5: Command Info opens in a pane whose text a remote host wrote; nothing else does.
    #[test]
    fn command_info_is_answered_before_the_remote_route() {
        let body = &SEMANTIC[SEMANTIC.find("fn open_target").unwrap()..];
        assert!(body.find("Target::CommandInfo").unwrap() < body.find("click_route(").unwrap());
        assert!(body.find("show_command_info(").unwrap() < body.find("click_route(").unwrap());
    }

    /// P15: the window asks for frames only while it ticks; the tick reads no buffer rows.
    #[test]
    fn the_tick_scans_no_rows() {
        let tick = &code()[code().find("fn cmdinfo_tick").unwrap()..code().find("fn cmdinfo_event").unwrap()];
        assert!(!tick.contains("command_mark("), "command_mark scans the buffer for its row");
        assert!(tick.contains("command_state("));
    }
}

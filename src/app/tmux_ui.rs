//! A pane that runs `tmux -CC` becomes a tmux gateway (iTerm2's TMUX_GATEWAY session): it
//! keeps its screen, prints the Command Menu and the gateway's messages, takes the menu's
//! keys instead of typing, and speaks tmux's control protocol through its pty.

use super::*;
use crate::tmux_gateway::{Event, Gateway, Tag};
use crate::tmux_handshake::{self, Handshake, Step};
use crate::tmux_hook::Out;
use crate::tmux_proto::Note;

/// iTerm2's banner (startTmuxMode), one message per line.
const BANNER: [&str; 8] = [
    "** tmux mode started **",
    "",
    "Command Menu",
    "----------------------------",
    "esc    Detach cleanly.",
    "  X    Force-quit tmux mode.",
    "  L    Toggle logging.",
    "  C    Run tmux command.",
];

pub(super) const SLOW_BAR: &str = "tmux-slow";
pub(super) const HUNG_BAR: &str = "tmux-hung";
const HUNG_KEY: &str = "NoSyncTmuxHung";

/// One gateway: its protocol state and what the session asked of it.
pub(super) struct TmuxGw {
    gw: Gateway,
    hs: Handshake,
    next_tag: Tag,
    /// Commands typed through C: their replies are printed.
    typed: std::collections::HashSet<Tag>,
    /// Force Detach? was offered for the command that waits.
    hung_asked: bool,
}

impl App {
    /// Runs what each pane's tmux hook found. True when anything happened.
    pub(super) fn tmux_pump(&mut self) -> bool {
        let ids: Vec<PaneId> = self.panes.iter().filter(|(_, p)| p.has_tmux()).map(|(id, _)| *id).collect();
        for id in &ids {
            let outs = self.panes.get_mut(id).map(|p| p.take_tmux()).unwrap_or_default();
            for out in outs {
                match out {
                    Out::Screen(bytes) => {
                        if let Some(p) = self.panes.get_mut(id) {
                            p.feed_output(&bytes);
                        }
                    }
                    Out::Enter => self.tmux_enter(*id),
                    Out::Line(line) | Out::Exit(line) => {
                        let Some(t) = self.tmux.get_mut(id) else { continue };
                        let events = t.gw.line(&line);
                        self.tmux_events(*id, events);
                    }
                }
            }
        }
        !ids.is_empty()
    }

    /// iTerm2's startTmuxMode.
    fn tmux_enter(&mut self, pane: PaneId) {
        if self.tmux.contains_key(&pane) {
            return;
        }
        let newline = if self.profile_of(pane).is_some_and(|p| p.bool("Tmux Newline")) { "\n" } else { "\r" };
        let Some(p) = self.panes.get_mut(&pane) else { return };
        // A bare ^C first: a fake start cannot make Tandem type a command into a shell.
        p.reply(&[3]);
        for line in BANNER {
            p.own_mut().print_message(line);
        }
        let mut gw = Gateway::new(newline);
        gw.tolerate_unrecognized = self.globals.bool("tolerateUnrecognizedTmuxCommands");
        self.tmux.insert(pane, TmuxGw { gw, hs: Handshake::default(), next_tag: tmux_handshake::FIRST_FREE_TAG, typed: Default::default(), hung_asked: false });
    }

    fn tmux_print(&mut self, pane: PaneId, text: &str) {
        if let Some(p) = self.panes.get_mut(&pane) {
            p.own_mut().print_message(text);
        }
    }

    fn tmux_events(&mut self, pane: PaneId, events: Vec<Event>) {
        for e in events {
            let Some(t) = self.tmux.get_mut(&pane) else { return };
            match e {
                Event::Print(line) => self.tmux_print(pane, &line),
                Event::Reply { tag, ok, text, .. } => {
                    if let Some(ask) = tmux_handshake::ask_of(tag) {
                        for step in t.hs.reply(ask, ok.then_some(text.as_str()), &mut t.gw) {
                            match step {
                                Step::Abort(title, text) => return self.tmux_abort(pane, &title, &text),
                                // Windows open from here in milestone 8b.
                                Step::VersionKnown => {}
                            }
                        }
                    } else if t.typed.remove(&tag) {
                        // iTerm2's printTmuxCommandOutputToScreen.
                        for line in text.split('\n') {
                            self.tmux_print(pane, &line.replace('\r', ""));
                        }
                    }
                }
                Event::Note(Note::SessionChanged { .. }) | Event::Initialized => t.hs.kick_off(&mut t.gw, Instant::now()),
                // Every other notification drives tmux windows (milestone 8b).
                Event::Note(_) => {}
                Event::Abort(msg) => return self.tmux_abort(pane, "tmux Reported a Problem", &msg),
                Event::InitialFailed(text) => self.tmux_print(pane, &format!("tmux failed with error: \u{201c}{text}\u{201d}")),
                Event::Exit => return self.tmux_disconnected(pane),
            }
        }
        self.tmux_flush(pane);
    }

    /// Writes what the gateway queued to the pane's pty (echoed while logging is on).
    fn tmux_flush(&mut self, pane: PaneId) {
        let Some(t) = self.tmux.get_mut(&pane) else { return };
        let bytes = t.gw.take_writes();
        if bytes.is_empty() {
            return;
        }
        let logging = t.gw.logging;
        let Some(p) = self.panes.get_mut(&pane) else { return };
        if logging {
            p.own_mut().print_message(&format!("> {}", String::from_utf8_lossy(&bytes).trim_end_matches(['\r', '\n'])));
        }
        p.reply(&bytes);
    }

    /// iTerm2's abortWithErrorMessage: the alert, `detach`, and tmux mode ends.
    fn tmux_abort(&mut self, pane: PaneId, title: &str, text: &str) {
        alert(self.hwnd(), title, text);
        if let Some(t) = self.tmux.get_mut(&pane) {
            t.gw.abort();
        }
        self.tmux_flush(pane);
        self.tmux_disconnected(pane);
    }

    /// iTerm2's tmuxHostDisconnected: "Detached", and the pane is an ordinary one again.
    fn tmux_disconnected(&mut self, pane: PaneId) {
        if self.tmux.remove(&pane).is_none() {
            return;
        }
        if let Some(p) = self.panes.get_mut(&pane) {
            p.own_mut().print_message("Detached");
            p.unhook_tmux();
        }
        self.bars.entry(pane).or_default().retain(|b| b.id != SLOW_BAR && b.id != HUNG_BAR);
        self.redraw();
    }

    pub(super) fn is_tmux_gateway(&self, pane: PaneId) -> bool {
        self.tmux.contains_key(&pane)
    }

    /// Typing in a gateway: the Command Menu's keys; everything else is dropped. A key
    /// counts by its first character, typed text by each one (iTerm2's keyDown and writeTask).
    pub(super) fn tmux_gateway_input(&mut self, pane: PaneId, bytes: &[u8], key: bool) {
        let text = String::from_utf8_lossy(bytes).into_owned();
        let chars: Vec<char> = if key { text.chars().take(1).collect() } else { text.chars().collect() };
        for c in chars {
            let Some(t) = self.tmux.get_mut(&pane) else { return };
            match c {
                '\u{1b}' => {
                    self.tmux_print(pane, "Detaching...");
                    let Some(t) = self.tmux.get_mut(&pane) else { return };
                    // A detach that tmux ignores: offer to let go (iTerm2's GW detach).
                    if t.gw.detach_sent() && t.gw.hung(Instant::now()) {
                        self.ask(pane, HUNG_BAR, "Force Detach? Tmux is not responding. Would you like to force detach?", &["Detach", "Cancel"], true);
                    } else {
                        t.gw.detach();
                        self.tmux_flush(pane);
                    }
                }
                'L' => {
                    t.gw.logging = !t.gw.logging;
                    let msg = if t.gw.logging { "tmux logging on" } else { "tmux logging off" };
                    self.tmux_print(pane, msg);
                }
                'C' => self.tmux_ask_command = Some(pane),
                'X' => {
                    self.tmux_print(pane, "Exiting tmux mode, but tmux client may still be running.");
                    if let Some(t) = self.tmux.get_mut(&pane) {
                        t.gw.force_disconnect();
                    }
                    self.tmux_disconnected(pane);
                }
                _ => {}
            }
        }
        self.redraw();
    }

    /// C's window: "Enter command to send tmux:".
    pub(super) fn open_tmux_command_prompt(&mut self, event_loop: &ActiveEventLoop) {
        let Some(pane) = self.tmux_ask_command.take() else { return };
        if let Some((w, _)) = &self.tmux_prompt {
            return w.focus();
        }
        let Some(g) = self.gpu.as_ref() else { return };
        match crate::prompt_ui::PromptWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, "tmux", "Enter command to send tmux:", "") {
            Ok(w) => self.tmux_prompt = Some((w, pane)),
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// Events for C's window. False when `window` is not it.
    pub(super) fn tmux_prompt_event(&mut self, window: winit::window::WindowId, event: &WindowEvent) -> bool {
        use crate::prompt_ui::PromptResult;
        let Some((w, pane)) = self.tmux_prompt.as_mut() else { return false };
        if w.id() != window {
            return false;
        }
        let pane = *pane;
        match event {
            WindowEvent::CloseRequested => self.tmux_prompt = None,
            WindowEvent::RedrawRequested => match w.redraw() {
                PromptResult::Open | PromptResult::Extra(_) => {}
                PromptResult::Cancel => self.tmux_prompt = None,
                PromptResult::Ok(command) => {
                    self.tmux_prompt = None;
                    if !command.is_empty() {
                        self.tmux_run_typed(pane, &command);
                    }
                }
            },
            other => {
                w.event(other);
            }
        }
        true
    }

    fn tmux_run_typed(&mut self, pane: PaneId, command: &str) {
        if !self.tmux.contains_key(&pane) {
            return;
        }
        self.tmux_print(pane, &format!("Run command \"{command}\""));
        let Some(t) = self.tmux.get_mut(&pane) else { return };
        let tag = t.next_tag;
        t.next_tag += 1;
        t.typed.insert(tag);
        t.gw.send(command, Some(tag), Default::default());
        self.tmux_flush(pane);
        self.redraw();
    }

    /// The gateways' timers: "Slow tmux Response" and "Force Detach?". Returns the next
    /// time to look.
    pub(super) fn tick_tmux(&mut self) -> Option<Instant> {
        let now = Instant::now();
        let silenced = self.globals.bool(HUNG_KEY);
        let mut slow = Vec::new();
        let mut hung = Vec::new();
        let mut next: Option<Instant> = None;
        for (id, t) in self.tmux.iter_mut() {
            if t.hs.phony_slow(now) && !silenced {
                slow.push(*id);
            }
            if !t.hung_asked && t.gw.hung(now) {
                t.hung_asked = true;
                hung.push(*id);
            }
            let due = [t.hs.phony_due(), (!t.hung_asked).then(|| t.gw.next_hung_check()).flatten()].into_iter().flatten().min();
            next = [next, due].into_iter().flatten().min();
        }
        for id in slow {
            self.ask(
                id,
                SLOW_BAR,
                "Slow tmux Response: It's taking a long time for tmux to respond. If this is a old or funky system it might expect newline rather than carriage return to end commands. You can adjust the line terminator used by tmux integration in Settings.",
                &["OK", "Reveal Setting", "Don't Show This Again"],
                true,
            );
        }
        for id in hung {
            self.ask(id, HUNG_BAR, "Force Detach? Tmux is not responding. Would you like to force detach?", &["Detach", "Cancel"], true);
        }
        next
    }

    /// The gateway bars' answers. True when the bar was one of these.
    pub(super) fn tmux_bar(&mut self, pane: PaneId, id: &str, button: Option<&str>, event_loop: &ActiveEventLoop) -> bool {
        match (id, button) {
            (SLOW_BAR, Some("Reveal Setting")) => {
                // The profile's "tmux uses newline (vs CRLF)" on its Terminal tab.
                let guid = self.panes.get(&pane).map(|p| p.profile.clone());
                self.open_settings(event_loop, Mode::Settings);
                if let (Some(s), Some(g)) = (self.settings.as_mut(), guid) {
                    s.select(&g);
                }
            }
            (SLOW_BAR, Some("Don't Show This Again")) => {
                self.globals.set(HUNG_KEY, toml::Value::Boolean(true));
                self.save_globals();
            }
            (SLOW_BAR, _) => {}
            (HUNG_BAR, Some("Detach")) => {
                if let Some(t) = self.tmux.get_mut(&pane) {
                    t.gw.force_disconnect();
                }
                self.tmux_disconnected(pane);
            }
            (HUNG_BAR, _) => {}
            _ => return false,
        }
        true
    }
}

/// An alert with OK (iTerm2's NSAlert with one button).
fn alert(owner: Option<isize>, title: &str, text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONWARNING, MB_OK, MessageBoxW};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let owner = owner.map_or(std::ptr::null_mut(), |h| h as *mut core::ffi::c_void);
    unsafe { MessageBoxW(owner, wide(text).as_ptr(), wide(title).as_ptr(), MB_OK | MB_ICONWARNING) };
}

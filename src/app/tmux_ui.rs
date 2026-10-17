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
    pub(super) gw: Gateway,
    pub(super) hs: Handshake,
    next_tag: Tag,
    /// Commands typed through C: their replies are printed.
    typed: std::collections::HashSet<Tag>,
    /// Force Detach? was offered for the command that waits.
    hung_asked: bool,
    /// iTerm2's clientName, fixed when tmux mode starts.
    pub(super) client: String,
    /// The tmux session: windows, panes, and the attach.
    pub(super) ctl: crate::tmux_controller::Controller,
    /// The tmux pane Termurion last told tmux (or tmux told Termurion) is selected.
    pub(super) selected: Option<u32>,
    /// When the pause monitor last ran.
    pub(super) pause_tick: Option<Instant>,
    /// AutoHideTmuxClientSession buried the gateway once already.
    pub(super) auto_buried: bool,
    /// The attach's windows opened: bury the gateway once they are windows.
    pub(super) bury_pending: bool,
    /// Each tmux window's tab size in cells, as Termurion last laid it out.
    pub(super) tab_sizes: HashMap<u32, (u32, u32)>,
    /// tmux pane id to (Termurion pane, tmux window id).
    pub(super) panes: HashMap<u32, (PaneId, u32)>,
    /// Each window's set-titles string, once tmux sent one.
    pub(super) window_titles: HashMap<u32, String>,
    /// Each window's name as tmux reports it (the prefix is not part of it).
    pub(super) window_names: HashMap<u32, String>,
    /// Tab titles the person gave tmux tabs (Edit Tab Title), by tmux window.
    pub(super) tab_overrides: HashMap<u32, String>,
}

/// Tags for commands typed through C, above the controller's.
const TYPED_TAGS: Tag = 1 << 40;

impl App {
    /// Runs what each pane's tmux hook found. True when anything happened.
    pub(super) fn tmux_pump(&mut self, event_loop: &ActiveEventLoop) -> bool {
        let ids: Vec<PaneId> = self.panes.iter().filter(|(_, p)| p.has_tmux()).map(|(id, _)| *id).collect();
        for id in &ids {
            let outs = self.panes.get_mut(id).map(|p| p.take_tmux()).unwrap_or_default();
            for out in outs {
                match out {
                    Out::Screen(bytes) => {
                        if let Some(p) = self.panes.get_mut(id) {
                            p.feed_output(&bytes);
                        }
                        self.api_output(&[*id]);
                    }
                    Out::Enter => self.tmux_enter(*id),
                    Out::Line(line) | Out::Exit(line) => {
                        let clock = self.started.elapsed().as_secs_f64();
                        let Some(t) = self.tmux.get_mut(id) else { continue };
                        t.ctl.set_clock(clock);
                        let events = t.gw.line(&line);
                        self.tmux_events(*id, events, event_loop);
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
        let title = self.session_title(pane);
        let Some(shell) = self.panes.get(&pane).map(|p| &p.own().shell) else { return };
        let name = crate::tmux_windows::client_name(shell.user.as_deref(), shell.host.as_deref(), &title);
        let Some(p) = self.panes.get_mut(&pane) else { return };
        p.gateway = true;
        // A bare ^C first: a fake start cannot make Termurion type a command into a shell.
        p.reply(&[3]);
        for line in BANNER {
            p.own_mut().print_message(line);
        }
        let client = (p.own().term().columns() as u32, p.own().term().screen_lines() as u32);
        let mut gw = Gateway::new(newline);
        gw.tolerate_unrecognized = self.globals.bool("TolerateUnrecognizedTmuxCommands");
        let mut ctl = crate::tmux_controller::Controller::new(tmux_handshake::FIRST_FREE_TAG);
        ctl.client = client;
        ctl.prefs = self.tmux_prefs(pane, client.1);
        crate::rlog!("tmux gateway started: pane {}", pane);
        self.tmux.insert(pane, TmuxGw { gw, hs: Handshake::default(), next_tag: TYPED_TAGS, typed: Default::default(), hung_asked: false, client: name, ctl, selected: None, pause_tick: None, auto_buried: false, bury_pending: false, window_titles: HashMap::new(), window_names: HashMap::new(), tab_overrides: HashMap::new(), tab_sizes: HashMap::new(), panes: HashMap::new() });
    }

    fn tmux_print(&mut self, pane: PaneId, text: &str) {
        if let Some(p) = self.panes.get_mut(&pane) {
            p.own_mut().print_message(text);
        }
    }

    /// The controller's preferences, read when the gateway starts.
    fn tmux_prefs(&self, gw: PaneId, rows: u32) -> crate::tmux_controller::Prefs {
        let p = self.profile_of(gw);
        let scrollback = p.map_or(1000, |p| p.int("Scrollback Lines"));
        let unlimited = p.is_some_and(|p| p.bool("Unlimited Scrollback"));
        crate::tmux_controller::Prefs {
            open_in: crate::tmux_windows::OpenIn::from_pref(self.globals.int("OpenTmuxWindowsIn")),
            limit: self.globals.int("TmuxDashboardLimit"),
            anonymous_location: self.globals.int("AnonymousTmuxWindowsOpenInCurrentWindow"),
            prevent_tab: p.is_some_and(|p| p.bool("Prevent Opening in a Tab")),
            max_history: crate::tmux_windows::max_history(rows, scrollback, unlimited),
            variable_sizes: self.globals.bool("TmuxVariableWindowSizesSupported"),
            pause_age: self.globals.int("TmuxPauseModeAgeLimit").max(1) as f64,
            unpause_automatically: self.globals.bool("TmuxUnpauseAutomatically"),
            warn_before_pausing: self.globals.bool("TmuxWarnBeforePausing") && !self.globals.bool("NoSyncDontWarnAboutTmuxPause"),
            poll_job: self.globals.bool("PollForTmuxForegroundJob"),
        }
    }

    pub(super) fn tmux_events(&mut self, pane: PaneId, events: Vec<Event>, event_loop: &ActiveEventLoop) {
        for e in events {
            let Some(t) = self.tmux.get_mut(&pane) else { return };
            match e {
                Event::Print(line) => self.tmux_print(pane, &line),
                Event::Reply { tag, ok, text, data } => {
                    // A Python API SendCommand: its output answers the request (`H:3620-3623`).
                    // A script's run_tmux_command: tmux's output, or "tmux error"
                    // (`PS:23730-23742`).
                    if let Some(token) = self.script_tmux.remove(&(pane, tag)) {
                        let r = if ok { Ok(crate::expr::Value::String(text)) } else { Err(crate::expr::Failure::plain(crate::builtin_fns::TMUX_ERROR)) };
                        self.api_rpc_done.push_back((token, r));
                        continue;
                    }
                    if let Some(waiting) = self.api_tmux_commands.remove(&(pane, tag)) {
                        let output = ok.then_some(text);
                        let payload = Some(crate::api_proto::pb::tmux_response::Payload::SendCommand(crate::api_proto::pb::tmux_response::SendCommand { output }));
                        self.api_tmux_answer(waiting, crate::api_proto::pb::TmuxResponse { status: None, payload });
                        continue;
                    }
                    if let Some(ask) = tmux_handshake::ask_of(tag) {
                        let steps = t.hs.reply(ask, ok.then_some(text.as_str()), &mut t.gw);
                        for step in steps {
                            match step {
                                Step::Abort(title, text) => return self.tmux_abort(pane, &title, &text),
                                Step::VersionKnown => {
                                    let Some(t) = self.tmux.get_mut(&pane) else { return };
                                    let (min, tab_bug, status) = (t.hs.min, t.hs.tab_bug, t.hs.status_bar);
                                    t.ctl.psmux = t.hs.psmux;
                                    t.ctl.version_known(min, tab_bug, status, &mut t.gw);
                                }
                            }
                        }
                        if ask == tmux_handshake::Ask::ServerPid {
                            // iTerm2's didLoadServerPID: a server whose process is here is local.
                            let Some(t) = self.tmux.get_mut(&pane) else { return };
                            let local = t.hs.server_local(&tmux_handshake::windows_process_name);
                            t.ctl.set_server_local(local, &mut t.gw);
                        }
                        if ask == tmux_handshake::Ask::SetTitles {
                            let Some(t) = self.tmux.get_mut(&pane) else { return };
                            let on = t.hs.set_titles;
                            t.ctl.set_titles_known(on, &mut t.gw);
                        }
                    } else if t.typed.remove(&tag) {
                        // iTerm2's printTmuxCommandOutputToScreen.
                        for line in text.split('\n') {
                            self.tmux_print(pane, &line.replace('\r', ""));
                        }
                    } else if let Some(actions) = t.ctl.reply(tag, ok, &text, &data, &mut t.gw) {
                        if !self.tmux_actions(pane, actions, event_loop) {
                            return;
                        }
                    }
                }
                Event::Initialized => t.hs.kick_off(&mut t.gw, Instant::now()),
                Event::Note(n) => {
                    // tmux 1.8 names the session before the first reply ends.
                    if matches!(n, Note::SessionChanged { .. }) {
                        t.hs.kick_off(&mut t.gw, Instant::now());
                    }
                    let actions = t.ctl.note(n, &mut t.gw);
                    if !self.tmux_actions(pane, actions, event_loop) {
                        return;
                    }
                }
                Event::Abort(msg) => return self.tmux_abort(pane, "tmux Reported a Problem", &msg),
                Event::InitialFailed(text) => self.tmux_print(pane, &format!("tmux failed with error: \u{201c}{text}\u{201d}")),
                Event::Exit => {
                    // tmux 3.2's wait-exit wants the newline the gateway queued.
                    self.tmux_flush(pane);
                    return self.tmux_disconnected(pane, false);
                }
                // psmux killed the attached session and sent no %exit: let go (which ends its client).
                Event::Gone => return self.tmux_disconnected(pane, true),
            }
        }
        self.tmux_flush(pane);
    }

    /// Writes what the gateway queued to the pane's pty (echoed while logging is on).
    pub(super) fn tmux_flush(&mut self, pane: PaneId) {
        let Some(t) = self.tmux.get_mut(&pane) else { return };
        let bytes = t.gw.take_writes();
        // A psmux detach: nothing to send, the client goes.
        if t.gw.take_left() {
            return self.tmux_disconnected(pane, true);
        }
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
    pub(super) fn tmux_abort(&mut self, pane: PaneId, title: &str, text: &str) {
        alert(self.hwnd(), title, text);
        if let Some(t) = self.tmux.get_mut(&pane) {
            t.gw.abort();
        }
        self.tmux_flush(pane);
        self.tmux_disconnected(pane, true);
    }

    /// iTerm2's tmuxHostDisconnected: "Detached", and the pane is an ordinary one again.
    /// `unhook`: tmux did not say `%exit` (X, an abort, Force Detach), so the hook is still
    /// reading lines. After `%exit` the hook has left already and may have started a new
    /// session read in the same batch.
    pub(super) fn tmux_disconnected(&mut self, pane: PaneId, unhook: bool) {
        if !self.tmux.contains_key(&pane) {
            return;
        }
        self.api_tmux_gone(pane);
        // psmux's client does not leave by itself when its gateway lets go: it is ended here.
        let psmux = self.tmux.get(&pane).is_some_and(|t| t.gw.psmux);
        // A buried gateway comes back first (iTerm2 restores it before anything else).
        if self.buried.contains(pane) {
            self.disinter(pane);
        }
        crate::rlog!("tmux gateway ended: pane {}", pane);
        self.tmux.remove(&pane);
        self.tmux_status.retain(|(g, _), _| *g != pane);
        self.tmux_end_panes(pane);
        self.tmux_forget_pids();
        if let Some(p) = self.panes.get_mut(&pane) {
            p.own_mut().print_message("Detached");
            p.gateway = false;
            if unhook {
                p.unhook_tmux();
                if psmux {
                    p.end_client();
                    p.seal_tmux();
                }
            }
        }
        self.bars.entry(pane).or_default().retain(|b| b.id != SLOW_BAR && b.id != HUNG_BAR);
        self.tmux_drop_double_bars(pane);
        self.redraw();
    }

    /// A tmux session's names: the gateway's `[prefix title client]`, a tmux pane's tab title.
    pub(super) fn tmux_titles(&self, pane: PaneId) -> Option<String> {
        let prefix = self.globals.string("TmuxTitlePrefix");
        if let Some(t) = self.tmux.get(&pane) {
            let base = self.panes.get(&pane).and_then(|p| p.own().title()).map(str::to_string).unwrap_or_else(|| self.session_title(pane));
            return Some(crate::tmux_windows::gateway_title(&prefix, &base, &t.client));
        }
        self.panes.get(&pane).and_then(|p| p.tmux_io())?;
        self.tabs.iter().chain(self.windows.parked().flat_map(|(_, s)| s.tabs.iter())).find(|t| t.layout.panes().contains(&pane)).and_then(|t| t.title.clone())
    }

    /// The client name of the tmux session that shows `pane`.
    pub(super) fn tmux_client_of(&self, pane: PaneId) -> Option<String> {
        let gw = self.panes.get(&pane)?.tmux_io()?.gateway;
        self.tmux.get(&gw).map(|t| t.client.clone())
    }

    /// The name of the tmux window that shows `pane`, as tmux reports it (None for a pane that is
    /// not a tmux client's). It is the server's text: control, bidirectional and zero-width
    /// characters are dropped and a long name is cut, as any name a remote sent.
    pub(super) fn tmux_window_name_of(&self, pane: PaneId) -> Option<String> {
        let io = self.panes.get(&pane)?.tmux_io()?;
        let t = self.tmux.get(&io.gateway)?;
        let window = t.panes.get(&io.pane)?.1;
        Some(crate::inert_files::shown_name(t.window_names.get(&window)?))
    }

    pub(super) fn tmux_gateways(&self) -> std::collections::BTreeSet<PaneId> {
        self.tmux.keys().copied().collect()
    }

    pub(super) fn is_tmux_gateway(&self, pane: PaneId) -> bool {
        self.tmux.contains_key(&pane)
    }

    /// Typing in a gateway: the Command Menu's keys; everything else is dropped. A key
    /// counts by its first character, typed text by each one (iTerm2's keyDown and writeTask).
    pub(super) fn tmux_gateway_input(&mut self, pane: PaneId, bytes: &[u8], key: bool) {
        for c in crate::tmux_gateway::menu_chars(bytes, key) {
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
                    self.tmux_disconnected(pane, true);
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

    /// A command for the Python API to the gateway `pane` (SendCommand with the default
    /// flags, `run_tmux_command` tolerating errors): sent with a tag of its own, whose output
    /// is not printed. The tag, or None when `pane` is no gateway.
    pub(super) fn tmux_api_send(&mut self, pane: PaneId, command: &str, flags: crate::tmux_gateway::Flags) -> Option<Tag> {
        let t = self.tmux.get_mut(&pane)?;
        let tag = t.next_tag;
        t.next_tag += 1;
        t.gw.send(command, Some(tag), flags);
        self.tmux_flush(pane);
        Some(tag)
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
                self.tmux_disconnected(pane, true);
            }
            (HUNG_BAR, _) => {}
            // The missing-program bars of New psmux Session and New tmux Session in WSL.
            (id, _) if id.starts_with(crate::tmux_launch::MISSING_BAR) => {}
            (super::tmux_tabs_ui::PLANE_BAR, Some("Why?")) => {
                if let Err(e) = super::semantic_ui::shell_open(crate::links::HELP_TMUX_22, false) {
                    self.banners.set("window", format!("Could not open the page: {e}"));
                }
            }
            (super::tmux_tabs_ui::PLANE_BAR, _) => {}
            (id, button) if self.tmux_close_bar(id, button) => self.tmux_finish_close(event_loop),
            (id, button) if self.tmux_pause_bar(id, button, event_loop) => {}
            _ => return false,
        }
        true
    }
}

/// An alert with OK (iTerm2's NSAlert with one button).
fn alert(owner: Option<isize>, title: &str, text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONWARNING, MB_OK};
    super::msgbox::show(owner, title, text, MB_OK | MB_ICONWARNING);
}

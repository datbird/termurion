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
    /// iTerm2's dcsID: names this control-mode stream, saved with a restorable gateway.
    pub(super) dcs_id: String,
    /// A restored gateway's writes are held until then (`enableWritesAfterDelay`).
    pub(super) writes_at: Option<Instant>,
    /// The client size was read before the restored gateway's tab existed: read it again.
    size_stale: bool,
}

/// Tags for commands typed through C, above the controller's.
const TYPED_TAGS: Tag = 1 << 40;

/// How long a restored gateway holds its writes (`enableWritesAfterDelay`, `TmuxGateway.m:1468-1473`).
pub(super) const RESTORE_WRITE_DELAY: Duration = Duration::from_secs(1);

impl TmuxGw {
    pub(super) fn new(newline: &str, tolerate_unrecognized: bool, client: String, ctl: crate::tmux_controller::Controller, dcs_id: String) -> TmuxGw {
        let mut gw = Gateway::new(newline);
        gw.tolerate_unrecognized = tolerate_unrecognized;
        TmuxGw { gw, hs: Handshake::default(), next_tag: TYPED_TAGS, typed: Default::default(), hung_asked: false, client, ctl, selected: None, pause_tick: None, auto_buried: false, bury_pending: false, window_titles: HashMap::new(), window_names: HashMap::new(), tab_overrides: HashMap::new(), tab_sizes: HashMap::new(), panes: HashMap::new(), dcs_id, writes_at: None, size_stale: false }
    }

    /// A restored gateway starts again (`PS:2109-2115`): the session it was attached to, then
    /// `kickOffTmuxForRestoration:YES` (`PS:11589-11604`) with its writes held for a second.
    /// Returns what goes to the pane at once, before anything else: the ^C (`PS:10850-10857`).
    pub(super) fn restore(&mut self, session: u32, name: &str, now: Instant) -> Vec<u8> {
        // Nothing is open yet, so its CloseAll has nothing to close; the windows wait for the version.
        let _ = self.ctl.session_changed(session, name, &mut self.gw);
        self.gw.restored_session(session, name);
        self.writes_at = Some(now + RESTORE_WRITE_DELAY);
        // Its tab is built after this: the size is read again before the windows open.
        self.size_stale = true;
        self.hs.kick_off(&mut self.gw, now);
        vec![3]
    }

    /// The restore's delay ran out: writes go from now on. True when it did just now.
    pub(super) fn writes_due(&mut self, now: Instant) -> bool {
        if !self.writes_at.is_some_and(|at| now >= at) {
            return false;
        }
        self.writes_at = None;
        self.gw.enable_writes();
        true
    }

    /// True once when the client size must be read again, now that the restored gateway's tab
    /// exists (iTerm2 asks `tmuxClientSize` only when it opens the windows, `TC:841-853`).
    pub(super) fn take_size_stale(&mut self) -> bool {
        std::mem::take(&mut self.size_stale)
    }

    /// A restored gateway at a shell: the held writes are dropped (`PS:11641`).
    pub(super) fn stop_restored(&mut self) {
        self.writes_at = None;
        self.gw.discard_held();
    }

    /// The gateway is going while its writes are still held: they go to the pane when the delay
    /// runs out, as iTerm2's delayed enableWrites still fires (`TmuxGateway.m:1468-1503`).
    pub(super) fn late_writes(&mut self) -> Option<(Instant, Vec<u8>)> {
        let at = self.writes_at.take()?;
        let bytes = self.gw.take_held();
        (!bytes.is_empty()).then_some((at, bytes))
    }

    /// A reply to one of the handshake's commands: the abort it asks for (title, text), if any.
    pub(super) fn handshake(&mut self, ask: tmux_handshake::Ask, text: Option<&str>) -> Option<(String, String)> {
        for step in self.hs.reply(ask, text, &mut self.gw) {
            match step {
                Step::Abort(title, text) => return Some((title, text)),
                Step::VersionKnown => {
                    let (min, tab_bug, status) = (self.hs.min, self.hs.tab_bug, self.hs.status_bar);
                    self.ctl.psmux = self.hs.psmux;
                    self.ctl.version_known(min, tab_bug, status, &mut self.gw);
                }
            }
        }
        None
    }
}

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
        let dcs_id = crate::pane::new_session_guid();
        if let Some(p) = self.panes.get_mut(&pane) {
            // A bare ^C first: a fake start cannot make Termurion type a command into a shell.
            p.reply(&[3]);
        }
        self.tmux_start(pane, dcs_id);
    }

    /// startTmuxMode after its ^C: the gateway, its banner and its controller.
    fn tmux_start(&mut self, pane: PaneId, dcs_id: String) {
        let newline = if self.profile_of(pane).is_some_and(|p| p.bool("Tmux Newline")) { "\n" } else { "\r" };
        let title = self.session_title(pane);
        let Some(shell) = self.panes.get(&pane).map(|p| &p.own().shell) else { return };
        let name = crate::tmux_windows::client_name(shell.user.as_deref(), shell.host.as_deref(), &title);
        let Some(p) = self.panes.get_mut(&pane) else { return };
        p.gateway = true;
        for line in BANNER {
            p.own_mut().print_message(line);
        }
        let own = (p.own().term().columns() as u32, p.own().term().screen_lines() as u32);
        let client = self.tmux_attach_size(pane, own);
        let mut ctl = crate::tmux_controller::Controller::new(tmux_handshake::FIRST_FREE_TAG);
        ctl.client = client;
        ctl.prefs = self.tmux_prefs(pane, client.1);
        crate::rlog!("tmux gateway started: pane {}", pane);
        let t = TmuxGw::new(newline, self.globals.bool("TolerateUnrecognizedTmuxCommands"), name, ctl, dcs_id);
        self.tmux.insert(pane, t);
    }

    /// The size the attach asks tmux for: a new tmux tab's (`tmux_windows::attach_size`). A tab in
    /// the gateway's window is the window's tab area in the tmux profile's cells; a gateway in a
    /// window that is not current keeps its own grid for that.
    fn tmux_attach_size(&mut self, gw: PaneId, own: (u32, u32)) -> (u32, u32) {
        let guid = self.tmux_profile_guid(gw);
        let p = self.profile(Some(&guid));
        let profile = (p.int("Columns").clamp(0, 9999) as u32, p.int("Rows").clamp(0, 9999) as u32);
        let tab = if self.tabs.iter().any(|t| t.layout.panes().contains(&gw)) {
            let cell = self.profile_cell(&p);
            // The tab's whole frame, as iTerm2's tab view (a status bar is inside a session).
            let (c, r) = self.grid_size_in(self.window_area(), cell, self.pane_room_for(gw, &p));
            (u32::from(c), u32::from(r))
        } else {
            own
        };
        crate::tmux_windows::attach_size(self.globals.bool("TmuxUsesDedicatedProfile"), crate::tmux_windows::OpenIn::from_pref(self.globals.int("OpenTmuxWindowsIn")), profile, tab)
    }

    /// A restored session that was a gateway, its job taken back: a gateway again, as in iTerm2
    /// (`PS:2109-2115`). The hook reads in recovery (`PS:2457-2466`), tmux mode starts with its ^C
    /// and banner, the controller takes the saved session, and the handshake waits a second. At a
    /// shell nothing runs tmux: it ends at once, the held writes dropped (`PS:11619-11645`).
    pub(super) fn restore_tmux_gateway(&mut self, pane: PaneId, saved: &crate::arrangement::SavedGateway) {
        let Some(p) = self.panes.get_mut(&pane) else { return };
        p.recover_tmux();
        let dcs_id = saved.dcs_id.clone().unwrap_or_else(crate::pane::new_session_guid);
        self.tmux_start(pane, dcs_id);
        let Some(t) = self.tmux.get_mut(&pane) else { return };
        let first = t.restore(saved.id, &saved.name, Instant::now());
        if let Some(p) = self.panes.get_mut(&pane) {
            p.reply(&first);
        }
        crate::rlog!("tmux gateway restored: pane {}", pane);
        let job = self.pane_job(pane);
        if self.tmux.get_mut(&pane).is_some_and(|t| stop_if_at_shell(t, job.as_deref())) {
            self.tmux_disconnected(pane, true);
        }
    }

    /// The name of the pane's foreground job, as `processNameIsAShell:` reads it.
    fn pane_job(&self, pane: PaneId) -> Option<String> {
        self.panes.get(&pane).and_then(|p| p.shell_pid()).and_then(crate::platform::proc::foreground_job).map(|j| j.name)
    }

    /// A restored gateway's tab exists by the time its writes go: its client size is read
    /// again, as iTerm2 reads it only when it opens the windows (`TC:841-853`).
    fn tmux_resize_restored(&mut self, pane: PaneId) {
        if !self.tmux.get_mut(&pane).is_some_and(|t| t.take_size_stale()) {
            return;
        }
        let Some(own) = self.panes.get(&pane).map(|p| (p.own().term().columns() as u32, p.own().term().screen_lines() as u32)) else { return };
        let client = self.tmux_attach_size(pane, own);
        let prefs = self.tmux_prefs(pane, client.1);
        if let Some(t) = self.tmux.get_mut(&pane) {
            t.ctl.client = client;
            t.ctl.prefs = prefs;
        }
    }

    pub(super) fn tmux_print(&mut self, pane: PaneId, text: &str) {
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
        // A reply may open the windows: a restored gateway's size is read first.
        self.tmux_resize_restored(pane);
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
                        if let Some((title, text)) = t.handshake(ask, ok.then_some(text.as_str())) {
                            return self.tmux_abort(pane, &title, &text);
                        }
                        if ask == tmux_handshake::Ask::ServerPid {
                            // iTerm2's didLoadServerPID: a server whose process is here is local.
                            // The process walk runs only when the rule needs it (never on Windows).
                            let shell = self.panes.get(&pane).and_then(|p| p.shell_pid());
                            let Some(t) = self.tmux.get_mut(&pane) else { return };
                            let local = t.hs.server_local(&tmux_handshake::process_name, &|| tmux_handshake::client_job(shell));
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
                    } else if let Some(actions) = t.ctl.reply(tag, ok, &text, &data, &mut t.gw)
                        && !self.tmux_actions(pane, actions, event_loop) {
                            return;
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
                // %exit, or a stream that is not tmux (then the hook lets go too, and psmux's
                // client is ended, so what follows is drawn: `PS:11746`).
                e @ (Event::Exit | Event::Lost) => {
                    let job = self.pane_job(pane);
                    let Some(t) = self.tmux.get_mut(&pane) else { return };
                    let (nl, unhook) = ending(t, &e, job.as_deref());
                    if let Some(nl) = nl {
                        if let Some(p) = self.panes.get_mut(&pane) {
                            p.reply(&nl);
                        }
                        // The restore's ^C may sit in the client's line, so that newline only ended
                        // it: look again, and send another only while the client still waits.
                        self.tmux_release.push((pane, Instant::now() + RELEASE_LOOK, nl, RELEASE_TRIES));
                    }
                    // tmux 3.2's wait-exit wants the newline the gateway queued.
                    self.tmux_flush(pane);
                    return self.tmux_disconnected(pane, unhook);
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
        // A restored gateway learns psmux from its first reply, so before that the job tells.
        let known = self.tmux.get(&pane).map(|t| (t.gw.psmux, t.hs.known));
        let job = if known.is_some_and(|(p, k)| !p && !k && unhook) { self.pane_job(pane) } else { None };
        let psmux = known.is_some_and(|(p, k)| is_psmux(p, k, job.as_deref()));
        // A buried gateway comes back first (iTerm2 restores it before anything else).
        if self.buried.contains(pane) {
            self.disinter(pane);
        }
        crate::rlog!("tmux gateway ended: pane {}", pane);
        if let Some(late) = self.tmux.remove(&pane).and_then(|mut t| t.late_writes()) {
            self.tmux_late.push((pane, late.0, late.1));
        }
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
        let mut due = Vec::new();
        for (id, t) in self.tmux.iter_mut() {
            if t.writes_due(now) {
                due.push(*id);
            }
            if t.hs.phony_slow(now) && !silenced {
                slow.push(*id);
            }
            if !t.hung_asked && t.gw.hung(now) {
                t.hung_asked = true;
                hung.push(*id);
            }
            let at = [t.writes_at, t.hs.phony_due(), (!t.hung_asked).then(|| t.gw.next_hung_check()).flatten()].into_iter().flatten().min();
            next = [next, at].into_iter().flatten().min();
        }
        for id in due {
            self.tmux_resize_restored(id);
            self.tmux_flush(id);
        }
        // A restored gateway that went before its second was up still writes then.
        for (pane, bytes) in due_late(&mut self.tmux_late, now) {
            if let Some(p) = self.panes.get_mut(&pane) {
                p.reply(&bytes);
            }
        }
        next = [next, self.tmux_late.iter().map(|(_, at, _)| *at).min()].into_iter().flatten().min();
        for (pane, nl, left) in due_release(&mut self.tmux_release, now) {
            let job = self.pane_job(pane);
            if !job.as_deref().is_some_and(is_tmux_client) {
                continue;
            }
            if let Some(p) = self.panes.get_mut(&pane) {
                p.reply(&nl);
            }
            if left > 0 {
                self.tmux_release.push((pane, now + RELEASE_LOOK, nl, left - 1));
            }
        }
        next = [next, self.tmux_release.iter().map(|(_, at, _, _)| *at).min()].into_iter().flatten().min();
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

/// Whether the gateway's end lets go of the hook: after `%exit` it has left already; a stream
/// that is not tmux, or psmux's silent end, still holds it (`PS:11746`).
fn unhooks(e: &Event) -> bool {
    !matches!(e, Event::Exit)
}

/// A restored gateway whose job is a shell runs no tmux: its held writes are dropped and it
/// ends (`PS:11625-11645`). True when it must end.
fn stop_if_at_shell(t: &mut TmuxGw, job: Option<&str>) -> bool {
    if !crate::session_state::is_shell(job) {
        return false;
    }
    t.stop_restored();
    true
}

/// How the gateway ends on `%exit` or on a stream that is not tmux: what goes to the pty first,
/// and whether the hook must be let go.
fn ending(t: &mut TmuxGw, e: &Event, job: Option<&str>) -> (Option<Vec<u8>>, bool) {
    let nl = if *e == Event::Exit { exited(t, job) } else { None };
    (nl, unhooks(e))
}

/// Whether the gateway talks to psmux: its first reply says so, and before that (a restored
/// gateway) the pane's job does.
fn is_psmux(said: bool, known: bool, job: Option<&str>) -> bool {
    said || (!known && job.is_some_and(|j| j.eq_ignore_ascii_case("psmux")))
}

/// `%exit` reached the gateway. One that never learned tmux's version (a restored gateway whose
/// server died) did not queue tmux 3.2's wait-exit newline, so its client may still wait for an
/// empty line and eat everything typed. The newline goes straight out while tmux is still the
/// pane's job, never into a shell. A restore's held handshake is dropped: once the client is
/// gone it would be typed into the shell.
fn exited(t: &mut TmuxGw, job: Option<&str>) -> Option<Vec<u8>> {
    if t.writes_at.take().is_some() {
        t.gw.discard_held();
    }
    (!t.hs.known && job.is_some_and(is_tmux_client)).then(|| t.gw.newline().as_bytes().to_vec())
}

/// A tmux client by its process name: tmux names its client process "tmux: client".
fn is_tmux_client(name: &str) -> bool {
    name == "tmux" || name.starts_with("tmux:")
}

/// How long after a wait-exit newline the pane's job is looked at again, and how many more
/// newlines may follow (the restore's ^C takes one).
const RELEASE_LOOK: Duration = Duration::from_millis(300);
const RELEASE_TRIES: u8 = 2;

/// The release looks that are due, taken from `list`: (pane, newline, tries left after this one).
fn due_release(list: &mut Vec<(PaneId, Instant, Vec<u8>, u8)>, now: Instant) -> Vec<(PaneId, Vec<u8>, u8)> {
    let (due, wait): (Vec<_>, Vec<_>) = std::mem::take(list).into_iter().partition(|(_, at, _, _)| now >= *at);
    *list = wait;
    due.into_iter().map(|(p, _, nl, left)| (p, nl, left.saturating_sub(1))).collect()
}

/// The late writes whose second is up, taken from `late`.
fn due_late(late: &mut Vec<(PaneId, Instant, Vec<u8>)>, now: Instant) -> Vec<(PaneId, Vec<u8>)> {
    let (due, wait): (Vec<_>, Vec<_>) = std::mem::take(late).into_iter().partition(|(_, at, _)| now >= *at);
    *late = wait;
    due.into_iter().map(|(pane, _, bytes)| (pane, bytes)).collect()
}

/// An alert with OK (iTerm2's NSAlert with one button).
fn alert(owner: Option<isize>, title: &str, text: &str) {
    crate::platform::dialog::message(owner, title, text, crate::platform::dialog::Kind::Warning);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux_controller::Controller;
    use std::time::Duration;

    fn restored(now: Instant) -> (TmuxGw, Vec<u8>) {
        let mut t = TmuxGw::new("\r", false, "client".into(), Controller::new(tmux_handshake::FIRST_FREE_TAG), "DCS-1".into());
        let first = t.restore(3, "tdef", now);
        (t, first)
    }

    /// What a fresh attach's kick-off writes (`PS:11589-11604`).
    fn kick_off_bytes() -> Vec<u8> {
        let mut g = Gateway::new("\r");
        g.enable_writes();
        Handshake::default().kick_off(&mut g, Instant::now());
        g.take_writes()
    }

    fn lines(t: &mut TmuxGw, lines: &[&str]) -> Vec<Event> {
        lines.iter().flat_map(|l| t.gw.line(l.as_bytes())).collect()
    }

    /// I2 and I3: a restored gateway writes ^C first (`PS:10850-10857`), knows its session
    /// (`PS:2112-2113`), holds its writes for a second (`TmuxGateway.m:1468-1473`), then sends
    /// the same handshake a fresh attach sends.
    #[test]
    fn a_restored_gateway_waits_a_second_then_shakes_hands() {
        let t0 = Instant::now();
        let (mut t, first) = restored(t0);
        assert_eq!(first, [3], "the ^C goes first, unconditionally");
        assert_eq!(t.ctl.session, Some((3, "tdef".to_string())));
        assert!(t.hs.kicked);
        assert!(t.gw.take_writes().is_empty(), "held");
        assert!(!t.writes_due(t0 + Duration::from_millis(999)));
        assert!(t.gw.take_writes().is_empty());
        assert!(t.writes_due(t0 + RESTORE_WRITE_DELAY));
        assert_eq!(t.gw.take_writes(), kick_off_bytes(), "the fresh attach's handshake, phony first");
        assert!(!t.writes_due(t0 + RESTORE_WRITE_DELAY * 2), "once");
    }

    /// I3: once the version is known the windows open from the server, as after a crash in
    /// iTerm2 (`TmuxController.m:580-589`, `823-838`).
    #[test]
    fn a_restored_gateway_opens_tmuxs_windows_once_the_version_is_known() {
        let t0 = Instant::now();
        let (mut t, _) = restored(t0);
        t.writes_due(t0 + RESTORE_WRITE_DELAY);
        t.gw.take_writes();
        assert_eq!(t.handshake(tmux_handshake::Ask::Version, Some("3.4")), None);
        let sent = String::from_utf8(t.gw.take_writes()).unwrap();
        assert!(sent.contains("show -v -q -t $3 @iterm2_size\r"), "{sent}");
        let mut t = TmuxGw::new("\r", false, "c".into(), Controller::new(tmux_handshake::FIRST_FREE_TAG), "D".into());
        t.restore(3, "tdef", t0);
        assert_eq!(t.handshake(tmux_handshake::Ask::AggressiveResize, Some("aggressive-resize on")).map(|a| a.0), Some("Unsupported tmux option".into()));
    }

    /// I1: the quit's `%exit` reaches a restored gateway before the version is known, so the
    /// gateway queues no newline (`TmuxGateway.m:978-980`, `826-831`). The client left with the
    /// quit, so the shell is the job: nothing is written, and the held handshake is dropped.
    #[test]
    fn the_quits_exit_on_a_restored_gateway_writes_no_newline() {
        let t0 = Instant::now();
        let (mut t, _) = restored(t0);
        let ev = lines(&mut t, &["%begin 1793828748 574 1", "%end 1793828748 574 1", "%exit"]);
        assert_eq!(ev.last(), Some(&Event::Exit), "{ev:?}");
        assert!(t.gw.take_writes().is_empty());
        assert_eq!(exited(&mut t, Some("zsh")), None);
        assert_eq!(t.late_writes(), None, "the handshake is not typed into the shell");
    }

    /// tmux-restore c: the server died while the app was gone, so the restored gateway gets
    /// `%exit` before any version. Its client sits in tmux 3.2's wait-exit as the pane's job: it
    /// gets the newline at once, and the held handshake is dropped, never typed into the shell.
    #[test]
    fn an_exit_before_the_version_releases_a_waiting_client_and_drops_the_held_writes() {
        let t0 = Instant::now();
        let (mut t, _) = restored(t0);
        let ev = lines(&mut t, &["%exit"]);
        assert_eq!(ev.last(), Some(&Event::Exit), "{ev:?}");
        assert_eq!(exited(&mut t, Some("tmux: client")), Some(b"\r".to_vec()), "the client still waits");
        assert_eq!(t.late_writes(), None, "nothing is typed a second later");
        assert!(!t.writes_due(t0 + RESTORE_WRITE_DELAY));
        assert!(t.gw.take_writes().is_empty());
        // At a shell the newline would be typed into it: none.
        let (mut t, _) = restored(t0);
        lines(&mut t, &["%exit"]);
        assert_eq!(exited(&mut t, Some("bash")), None);
        assert_eq!(exited(&mut t, None), None, "no job known: no newline");
        assert_eq!(t.late_writes(), None);
        // A known version queued its own newline (or needs none): nothing extra.
        let (mut t, _) = restored(t0);
        t.writes_due(t0 + RESTORE_WRITE_DELAY);
        t.handshake(tmux_handshake::Ask::Version, Some("3.4"));
        t.gw.take_writes();
        lines(&mut t, &["%exit"]);
        assert_eq!(exited(&mut t, Some("tmux")), None);
        assert_eq!(t.gw.take_writes(), b"\r", "the gateway's own wait-exit newline");
    }

    /// The restore's ^C sits in a waiting client's line, so its first newline only ends that
    /// line: the release looks again, each look once, at most RELEASE_TRIES times.
    #[test]
    fn a_release_looks_again_a_bounded_number_of_times() {
        let t0 = Instant::now();
        let mut list = vec![(7, t0 + RELEASE_LOOK, b"\r".to_vec(), RELEASE_TRIES)];
        assert!(due_release(&mut list, t0).is_empty());
        assert_eq!(due_release(&mut list, t0 + RELEASE_LOOK), vec![(7, b"\r".to_vec(), RELEASE_TRIES - 1)]);
        assert!(list.is_empty(), "taken once; the app puts it back only while the client waits");
    }

    /// m1: the Exit and Lost arm. `%exit` releases a waiting client and keeps the hook (it left
    /// already); a stream that is not tmux writes nothing and lets go of the hook.
    #[test]
    fn the_exit_and_lost_arm_writes_and_unhooks_as_it_must() {
        let t0 = Instant::now();
        let (mut t, _) = restored(t0);
        assert_eq!(ending(&mut t, &Event::Exit, Some("tmux: client")), (Some(b"\r".to_vec()), false));
        let (mut t, _) = restored(t0);
        assert_eq!(ending(&mut t, &Event::Exit, Some("tmux")).0, Some(b"\r".to_vec()));
        let (mut t, _) = restored(t0);
        assert_eq!(ending(&mut t, &Event::Lost, Some("tmux")), (None, true));
        assert!(t.writes_at.is_some(), "a lost stream keeps iTerm2's delayed writes");
    }

    /// m2: a restored psmux gateway that loses its stream before its first reply still ends
    /// psmux's client; a tmux one, or one whose version is known, goes by what tmux said.
    #[test]
    fn a_restored_psmux_gateway_is_known_by_its_job_before_its_first_reply() {
        assert!(is_psmux(false, false, Some("psmux")));
        assert!(!is_psmux(false, false, Some("tmux")));
        assert!(!is_psmux(false, true, Some("psmux")), "the reply said tmux");
        assert!(is_psmux(true, true, None));
        assert!(!is_psmux(false, false, None));
    }

    /// I4: a restored pane that is not talking to tmux prints iTerm2's words and lets go
    /// (`TmuxGateway.m:989-993`).
    #[test]
    fn a_restored_gateway_lets_go_of_a_stream_that_is_not_tmux() {
        let (mut t, _) = restored(Instant::now());
        let ev = lines(&mut t, &["Connection to host closed."]);
        assert_eq!(ev, vec![Event::Print("Unrecognized command from tmux. Did your ssh session die? The command was:".into()), Event::Print("Connection to host closed.".into()), Event::Lost]);
    }

    /// I-a and M-b: how each end lets go. A line that is not tmux unhooks, and so for psmux ends
    /// the client and seals the hook in `tmux_disconnected` as every other let-go does.
    #[test]
    fn each_end_lets_go_of_the_hook_as_it_must() {
        assert!(!unhooks(&Event::Exit), "after %exit the hook has left already");
        assert!(unhooks(&Event::Lost));
        assert!(unhooks(&Event::Gone));
        let mut g = Gateway::new("\r");
        g.psmux = true;
        let ev = g.line(b"not tmux");
        assert!(ev.last().is_some_and(unhooks), "{ev:?}");
    }

    /// M-b: the restore's checks the app runs. At a shell it ends with nothing written; under
    /// another job, or none known, it keeps tmux mode (`processNameIsAShell:`).
    #[test]
    fn a_restored_gateway_ends_only_at_a_shell() {
        let t0 = Instant::now();
        let (mut t, _) = restored(t0);
        assert!(!stop_if_at_shell(&mut t, Some("tmux")));
        assert!(!stop_if_at_shell(&mut t, None), "no job known: kept");
        assert!(t.writes_at.is_some());
        assert!(stop_if_at_shell(&mut t, Some("zsh")));
        assert_eq!((t.writes_at, t.late_writes()), (None, None), "nothing held, nothing late");
    }

    /// M-b: late writes go when their second is up, each once.
    #[test]
    fn late_writes_go_when_their_second_is_up() {
        let t0 = Instant::now();
        let mut late = vec![(1, t0 + RESTORE_WRITE_DELAY, b"a".to_vec()), (2, t0 + RESTORE_WRITE_DELAY * 2, b"b".to_vec())];
        assert!(due_late(&mut late, t0).is_empty());
        assert_eq!(due_late(&mut late, t0 + RESTORE_WRITE_DELAY), vec![(1, b"a".to_vec())]);
        assert_eq!(late.len(), 1);
        assert_eq!(due_late(&mut late, t0 + RESTORE_WRITE_DELAY * 2), vec![(2, b"b".to_vec())]);
        assert!(late.is_empty());
    }

    /// M-a: a restored gateway reads its client size again once, after its tab exists; a fresh
    /// one never needs to.
    #[test]
    fn a_restored_gateway_reads_its_size_again() {
        let mut fresh = TmuxGw::new("\r", false, "c".into(), Controller::new(tmux_handshake::FIRST_FREE_TAG), "D".into());
        assert!(!fresh.take_size_stale());
        let (mut t, _) = restored(Instant::now());
        assert!(t.take_size_stale());
        assert!(!t.take_size_stale(), "once");
    }

    /// At a shell nothing runs tmux: the held writes are dropped (`PS:11640-11642`).
    #[test]
    fn a_restored_gateway_at_a_shell_sends_nothing() {
        let t0 = Instant::now();
        let (mut t, _) = restored(t0);
        t.stop_restored();
        assert!(!t.writes_due(t0 + RESTORE_WRITE_DELAY));
        assert!(t.gw.take_writes().is_empty());
        assert_eq!(t.late_writes(), None);
    }
}

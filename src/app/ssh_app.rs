//! SSH integration in the app: the handshake's requests from a pane's output
//! (`TD:3955-4049`: the connecting state, the conductor bootstrap and EndSSH), and the
//! conductor hook (milestone 14b): what each pane's DCS 2000p hook found, the setup at hook
//! time (`PS:20118-20170`) and the conductor's effects.

use super::*;
use crate::conductor::{hook_banner, setup_for_hook, Conductor};
use crate::conductor_parser::{Init, Token};
use crate::screen::Request;
use crate::ssh_hook::Out;
use crate::ssh_session::{Hop, CONDUCTOR_READY};

/// iTerm2's ssh_config search order in this user's folders (`ssh_args::config_paths`), from
/// the environment only; a path that is relative, a network path, or reached through a link
/// or junction to one (`scripts::ancestors_stay_local`, which reads only the links) is left
/// out: a remote can start a hook without a token, so the load must never probe the network.
fn ssh_config_paths() -> Vec<std::path::PathBuf> {
    let var = |name: &str| std::env::var_os(name).map(std::path::PathBuf::from).unwrap_or_default();
    let paths = crate::ssh_args::config_paths(&var("APPDATA"), &var("USERPROFILE"), &var("ProgramData"));
    paths.into_iter().filter(|p| p.is_absolute() && crate::scripts::ancestors_stay_local(p, &crate::scripts::read_link)).collect()
}

impl App {
    /// Starts serving origin secrets on this user's pipe. A failure (another Tandem of this
    /// user holds the name) sets no banner: it2ssh then sends "none", as off macOS. The pipe
    /// name carries the user SID, so the message leaves it out.
    pub(super) fn ssh_secrets_start(&mut self) {
        match crate::ssh_secret_win::Server::start(self.ssh_secrets.clone()) {
            Ok(server) => self.ssh_secret_server = Some(server),
            Err(e) => {
                let e = match crate::winpipe::user_sid() {
                    Ok(sid) => e.replace(&sid, "<SID>"),
                    Err(_) => e,
                };
                eprintln!("Tandem: the SSH origin secret pipe did not start: {e}");
            }
        }
    }

    pub(super) fn ssh_request(&mut self, pane: PaneId, r: Request) {
        let Some(p) = self.panes.get_mut(&pane) else { return };
        match r {
            Request::SshWillBegin => p.ssh.begin(CONDUCTOR_READY),
            Request::SshSendConductor(h) => p.ssh_send_conductor(&h),
            // `PS:20253-20275` (`Pane::end_ssh`, which writes END_WRITE); the banner after
            // the write, as `PS:20269` then `TD:4043-4046`.
            Request::SshEnd(id) => {
                let banner = p.end_ssh(&id);
                debug_assert!(p.ssh_chain_in_step(), "the hops follow the conductors");
                if let Some(banner) = banner {
                    p.own_mut().append_banner(&banner);
                    self.redraw();
                }
                self.ssh_registry_drain(pane);
            }
            _ => {}
        }
    }

    /// Runs what each pane's SSH hook found, in order. Before `tmux_pump`: output released
    /// here can start tmux control mode. True when anything happened.
    pub(super) fn ssh_pump(&mut self) -> bool {
        let ids: Vec<PaneId> = self.panes.iter().filter(|(_, p)| p.has_ssh() || p.ssh_effects_pending()).map(|(id, _)| *id).collect();
        for id in &ids {
            self.ssh_pump_pane(*id);
        }
        // The remote jobs are read once however many panes and process blocks this pass held
        // (F22).
        self.ssh_jobs_changed(&ids);
        !ids.is_empty()
    }

    /// `jobs_changed` once when any of `panes` has a remote process list that changed.
    pub(super) fn ssh_jobs_changed(&mut self, panes: &[PaneId]) {
        let dirty = panes.iter().fold(false, |d, id| self.panes.get_mut(id).is_some_and(|p| p.take_jobs_dirty()) | d);
        if dirty {
            self.jobs_changed();
        }
    }

    /// Runs what one pane's SSH hook found, in order. A tmux integration pane is pumped inside
    /// `tmux_pump`, after `ssh_pump`, so its Output arm calls this at once: nothing else wakes
    /// the loop for a conductor waiting on Tandem's next command.
    pub(super) fn ssh_pump_pane(&mut self, pane: PaneId) {
        let outs = self.panes.get_mut(&pane).map(|p| p.take_ssh()).unwrap_or_default();
        let mut batch = crate::ssh_hook::Batch::default();
        for out in outs {
            if batch.drops(&out) {
                continue;
            }
            match out {
                Out::Screen(bytes) => {
                    if let Some(p) = self.panes.get_mut(&pane) {
                        p.feed_after_ssh(&bytes);
                    }
                    self.api_output(&[pane]);
                }
                // Only for the coprocess tee, which `Pane::hook_output` did at once.
                Out::Raw(_) => {}
                Out::Side { text, pid, channel, depth } => {
                    if let Some(c) = self.panes.get_mut(&pane).and_then(|p| p.conductor.as_mut()) {
                        c.handle_side(&text, pid, channel, depth);
                    }
                }
                Out::Ssh(Token::Init(init), depth) => {
                    let made = self.ssh_hooked(pane, init, depth);
                    batch.hooked(depth, made);
                }
                // By the depth of the parser that lost its output (a conductor that is gone
                // leaves it dropped), not always the innermost conductor.
                Out::Ssh(Token::Error(reason), depth) => {
                    if self.panes.get_mut(&pane).is_some_and(|p| p.ssh_error(depth, &reason)) {
                        self.redraw();
                    }
                }
                // Beyond iTerm2 (`PS:20172-20177` has no depth): see `unhook_conductor_at`.
                Out::Ssh(Token::Unhook, depth) => {
                    if let Some(p) = self.panes.get_mut(&pane) {
                        p.unhook_conductor_at(depth);
                    }
                }
                Out::Ssh(token, depth) => {
                    // A token with no conductor (its setup was refused) is dropped.
                    let Some(p) = self.panes.get_mut(&pane) else { continue };
                    let Some(c) = p.conductor.as_mut() else { continue };
                    match token {
                        Token::Begin(cmd) => c.handle_begin(&cmd, depth),
                        Token::Line(line) => c.handle_line(&line, depth),
                        Token::End { id: cmd, status, kind } => c.handle_end(&cmd, &kind, status, depth),
                        // Routed above by its depth.
                        Token::Unhook => {}
                        // Routed above by its depth.
                        Token::Error(_) => {}
                        Token::Terminate { pid, rc } => c.handle_terminate(pid, rc, depth),
                        Token::It2(text) => c.handle_it2(&text, depth),
                        // The hook turns output into `Out::Side` and `Out::Raw`.
                        Token::Init(_) | Token::Output { .. } => {}
                    }
                }
            }
            self.ssh_apply(pane);
        }
        // Keys handed to a conductor outside its tokens (a coprocess, a write routed to it).
        // The remote job is read by the caller (`ssh_jobs_changed`).
        self.ssh_apply(pane);
    }

    /// A conductor hooked (`TD:3852-3870`, then `PS:20118-20170` without jumps): the banner,
    /// the saved state, the reset, then a new conductor chained onto the pane's current one.
    /// Only when it takes its hook's depth there (`Pane::ssh_init_fits`); false when refused.
    fn ssh_hooked(&mut self, pane: PaneId, init: Init, depth: i32) -> bool {
        if !self.panes.get_mut(&pane).is_some_and(|p| p.ssh_init_fits(depth)) {
            return false;
        }
        let (cfg, inject) = match self.profile_of(pane) {
            Some(p) => (crate::ssh_config::from_profile(p), p.bool("Load Shell Integration Automatically")),
            None => (Default::default(), false),
        };
        let Some(p) = self.panes.get_mut(&pane) else { return false };
        p.own_mut().append_banner(&hook_banner(&init.sshargs));
        let saved = p.own().ssh_saved();
        // resetForSSH: before the conductor starts, so its replies are not held behind setup.
        p.own_mut().soft_reset();
        // The origin token is checked before anything the remote sent is parsed.
        let local = self.ssh_secrets.lock().unwrap().check(&init.token);
        let directory = if p.ssh.profile_ssh && p.conductor.is_none() { p.ssh.initial_directory.clone() } else { None };
        p.ssh.profile_ssh = false;
        let env = p.own().exfiltrated(None);
        let finder = crate::ssh_args::SshConfigs::load(&ssh_config_paths());
        let setup = setup_for_hook(&init, local, &env, &cfg.environment_variables_to_copy, directory, inject, &finder);
        let prefix = uuid::Uuid::new_v4().hyphenated().to_string().to_uppercase();
        let mut c = Conductor::new(setup, p.conductor.take(), &prefix);
        c.saved = Some(saved);
        c.uname_command = self.globals.string("unameCommand");
        if local {
            let home = std::env::var_os("USERPROFILE").map(std::path::PathBuf::from).unwrap_or_default();
            for (path, destination) in &cfg.files_to_copy {
                c.add_path(path, destination, &home);
            }
        }
        c.start();
        let hop = Hop { unique_id: c.setup.client_unique_id.clone(), location: c.identity().compact(&crate::ssh_args::local_user()) };
        p.conductor = Some(Box::new(c));
        p.ssh.chain.push(hop);
        debug_assert!(p.ssh_chain_in_step(), "the hops follow the conductors");
        debug_assert_eq!(p.conductor.as_ref().map(|c| c.depth()), Some(depth), "a conductor at its hook's depth");
        self.redraw();
        true
    }

    /// Applies what the pane's conductors asked for (`Pane::apply_ssh_effects`, which runs
    /// a failed conductor's abort, `PS:25587-25601`).
    pub(super) fn ssh_apply(&mut self, pane: PaneId) {
        if self.panes.get_mut(&pane).is_some_and(|p| p.apply_ssh_effects()) {
            self.redraw();
        }
        self.ssh_registry_drain(pane);
        self.ssh_answers_drain(pane);
    }

    /// Delivers what the pane's conductors answered to script calls (`get_time_offset`).
    pub(super) fn ssh_answers_drain(&mut self, pane: PaneId) {
        let answers = self.panes.get_mut(&pane).map(|p| p.take_ssh_answers()).unwrap_or_default();
        self.ssh_answers_deliver(answers);
    }

    fn ssh_answers_deliver(&mut self, answers: Vec<(u64, Result<crate::ssh_clock::Offset, String>)>) {
        for (token, result) in answers {
            let answer = result.map(|o| serde_json::json!({ "offset": o.offset, "tz": o.tz })).map_err(crate::expr::Failure::plain);
            self.api_rpc_done.push_back((token, answer));
        }
    }

    /// Applies the pane's changes to the connected hosts (`Registered` and `Deregistered` from
    /// its conductors, and every conductor that left the pane), then tells the hook once.
    pub(super) fn ssh_registry_drain(&mut self, pane: PaneId) {
        let changes = self.panes.get_mut(&pane).map(|p| p.take_ssh_registry()).unwrap_or_default();
        if self.ssh_registry.apply(changes) {
            self.ssh_hosts_changed();
        }
    }

    /// A pane that is dropped for good takes each of its conductors out of the connected hosts
    /// (removing an unknown guid is harmless). A pane kept for Undo Close keeps its entries
    /// until its Undo Close expires.
    pub(super) fn ssh_pane_dropped(&mut self, pane: &Pane) {
        let changes = pane.ssh_guids().into_iter().map(crate::ssh_registry::Change::Remove).collect::<Vec<_>>();
        if self.ssh_registry.apply(changes) {
            self.ssh_hosts_changed();
        }
        // A script call whose pane goes is answered with an error, not left waiting.
        let owed = pane.owed_answers().into_iter().map(|t| (t, Err("Failed to fetch time: the session ended".to_string()))).collect();
        self.ssh_answers_deliver(owed);
    }

    /// The connected hosts changed; 14d refreshes Download Files and its panel here.
    pub(super) fn ssh_hosts_changed(&mut self) {}

    /// The conductors' command timers: every pane's current conductor is ticked and its
    /// effects applied. Returns the earliest deadline left.
    pub(super) fn tick_ssh(&mut self) -> Option<Instant> {
        let now = Instant::now();
        let mut redraw = false;
        let mut next: Option<Instant> = None;
        let mut changes = Vec::new();
        let mut published = Vec::new();
        let mut answers = Vec::new();
        for (id, p) in self.panes.iter_mut().filter(|(_, p)| p.conductor.is_some()) {
            redraw |= p.tick_ssh(now);
            // The remote CPU values, once a second (F8: folded into this tick).
            if p.tick_procs(now) {
                published.push(*id);
            }
            changes.append(&mut p.take_ssh_registry());
            answers.append(&mut p.take_ssh_answers());
            next = [next, p.ssh_deadline(), p.procs_due()].into_iter().flatten().min();
        }
        if self.ssh_registry.apply(changes) {
            self.ssh_hosts_changed();
        }
        // The Clocks' server time: each session with a Clock in its bar asks when due (F8: this
        // is the one ssh tick, no timer of its own).
        let clocked: Vec<PaneId> = self.panes.iter().filter(|(_, p)| p.conductor.is_some() || p.ssh_clock.is_remote()).map(|(id, _)| *id).collect();
        for id in clocked {
            if !self.bar_has_clock(id) {
                continue;
            }
            let Some(p) = self.panes.get_mut(&id) else { continue };
            let host = p.own().shell.host.clone();
            p.ssh_clock_tick(now, host.as_deref());
            next = [next, p.clock_due()].into_iter().flatten().min();
            answers.extend(p.take_ssh_answers());
        }
        self.ssh_answers_deliver(answers);
        // A new remote CPU value redraws only a window whose bar shows it.
        redraw |= published.into_iter().any(|id| self.shows_status_bar(id));
        // A process list that changed or a conductor that left (the pane's job is read again).
        let dirty = self.panes.values_mut().fold(false, |d, p| p.take_jobs_dirty() | d);
        if dirty {
            self.jobs_changed();
        }
        if redraw {
            self.redraw();
        }
        next
    }
}

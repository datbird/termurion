//! SSH integration in the app: the handshake's requests from a pane's output
//! (`TD:3955-4049`: the connecting state, the conductor bootstrap and EndSSH), and the
//! conductor hook (milestone 14b): what each pane's DCS 2000p hook found, the setup at hook
//! time (`PS:20118-20170`) and the conductor's effects.

use super::*;
use crate::conductor::{hook_banner, setup_for_hook, Conductor};
use crate::conductor_parser::{Init, Token};
use crate::screen::Request;
use crate::ssh_hook::Out;
use crate::ssh_session::{conductor_bytes, Hop, CONDUCTOR_READY};

/// iTerm2's ssh_config search order in this user's folders (`ssh_args::config_paths`), from
/// the environment only; a folder that is unset, relative or a network path is left out.
fn ssh_config_paths() -> Vec<std::path::PathBuf> {
    let var = |name: &str| std::env::var_os(name).map(std::path::PathBuf::from).unwrap_or_default();
    let paths = crate::ssh_args::config_paths(&var("APPDATA"), &var("USERPROFILE"), &var("ProgramData"));
    paths.into_iter().filter(|p| p.is_absolute() && !is_network(p)).collect()
}

/// `\\host\share` or `//host/share`: never probed as a file.
fn is_network(p: &std::path::Path) -> bool {
    let s = p.to_string_lossy();
    s.starts_with(r"\\") || s.starts_with("//")
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
            Request::SshSendConductor(h) => {
                if let Some(bytes) = conductor_bytes(&h, CONDUCTOR_READY) {
                    p.write_no_broadcast(&bytes);
                }
            }
            // `PS:20253-20275` (`Pane::end_ssh`, which writes END_WRITE); the banner after
            // the write, as `PS:20269` then `TD:4043-4046`.
            Request::SshEnd(id) => {
                let banner = p.end_ssh(&id);
                debug_assert!(p.ssh_chain_in_step(), "the hops follow the conductors");
                if let Some(banner) = banner {
                    p.own_mut().append_banner(&banner);
                    self.redraw();
                }
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
        !ids.is_empty()
    }

    /// Runs what one pane's SSH hook found, in order. A tmux integration pane is pumped inside
    /// `tmux_pump`, after `ssh_pump`, so its Output arm calls this at once: nothing else wakes
    /// the loop for a conductor waiting on Tandem's next command.
    pub(super) fn ssh_pump_pane(&mut self, pane: PaneId) {
        let outs = self.panes.get_mut(&pane).map(|p| p.take_ssh()).unwrap_or_default();
        for out in outs {
            match out {
                Out::Screen(bytes) => {
                    if let Some(p) = self.panes.get_mut(&pane) {
                        p.feed_after_ssh(&bytes);
                    }
                    self.api_output(&[pane]);
                }
                Out::Ssh(Token::Init(init), _) => self.ssh_hooked(pane, init),
                Out::Ssh(token, depth) => {
                    // A token with no conductor (its setup was refused) is dropped.
                    let Some(p) = self.panes.get_mut(&pane) else { continue };
                    let Some(c) = p.conductor.as_mut() else { continue };
                    match token {
                        Token::Begin(cmd) => c.handle_begin(&cmd, depth),
                        Token::Line(line) => c.handle_line(&line, depth),
                        Token::End { id: cmd, status, kind } => c.handle_end(&cmd, &kind, status, depth),
                        // `PS:20172-20177`: the held writes go out raw.
                        Token::Unhook => {
                            c.handle_unhook();
                            let held = p.take_ssh_held();
                            p.write_no_broadcast(&held);
                        }
                        Token::Init(_) => {}
                    }
                }
            }
            self.ssh_apply(pane);
        }
        // Keys handed to a conductor outside its tokens (a coprocess, a write routed to it).
        self.ssh_apply(pane);
    }

    /// A conductor hooked (`TD:3852-3870`, then `PS:20118-20170` without jumps): the banner,
    /// the saved state, the reset, then a new conductor chained onto the pane's current one.
    fn ssh_hooked(&mut self, pane: PaneId, init: Init) {
        let (cfg, inject) = match self.profile_of(pane) {
            Some(p) => (crate::ssh_config::from_profile(p), p.bool("Load Shell Integration Automatically")),
            None => (Default::default(), false),
        };
        let Some(p) = self.panes.get_mut(&pane) else { return };
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
        self.redraw();
    }

    /// Applies what the pane's conductors asked for (`Pane::apply_ssh_effects`, which runs
    /// a failed conductor's abort, `PS:25587-25601`).
    pub(super) fn ssh_apply(&mut self, pane: PaneId) {
        if self.panes.get_mut(&pane).is_some_and(|p| p.apply_ssh_effects()) {
            self.redraw();
        }
    }

    /// The conductors' command timers: every pane's current conductor is ticked and its
    /// effects applied. Returns the earliest deadline left.
    pub(super) fn tick_ssh(&mut self) -> Option<Instant> {
        let now = Instant::now();
        let mut redraw = false;
        let mut next: Option<Instant> = None;
        for p in self.panes.values_mut().filter(|p| p.conductor.is_some()) {
            redraw |= p.tick_ssh(now);
            next = [next, p.ssh_deadline()].into_iter().flatten().min();
        }
        if redraw {
            self.redraw();
        }
        next
    }
}

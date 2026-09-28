//! SSH integration in the app: the handshake's requests from a pane's output
//! (`TD:3955-4049`: the connecting state, the conductor bootstrap and EndSSH), and the
//! conductor hook (milestone 14b): what each pane's DCS 2000p hook found, the setup at hook
//! time (`PS:20118-20170`) and the conductor's effects.

use super::*;
use crate::conductor::{hook_banner, setup_for_hook, Conductor, Effect};
use crate::conductor_parser::{Init, Token};
use crate::screen::Request;
use crate::ssh_hook::Out;
use crate::ssh_session::{conductor_bytes, Hop, CONDUCTOR_READY, END_WRITE};

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
            // The write before the banner, as `PS:20269` then `TD:4043-4046`.
            Request::SshEnd(id) => {
                if let Some(banner) = p.ssh.end_ssh(&id) {
                    p.write_no_broadcast(END_WRITE);
                    p.own_mut().append_banner(&banner);
                }
            }
            _ => {}
        }
    }

    /// Runs what each pane's SSH hook found, in order. Before `tmux_pump`: output released
    /// here can start tmux control mode. True when anything happened.
    pub(super) fn ssh_pump(&mut self) -> bool {
        let ids: Vec<PaneId> = self.panes.iter().filter(|(_, p)| p.has_ssh()).map(|(id, _)| *id).collect();
        for id in &ids {
            let outs = self.panes.get_mut(id).map(|p| p.take_ssh()).unwrap_or_default();
            for out in outs {
                match out {
                    Out::Screen(bytes) => {
                        if let Some(p) = self.panes.get_mut(id) {
                            p.feed_after_ssh(&bytes);
                        }
                        self.api_output(&[*id]);
                    }
                    Out::Ssh(Token::Init(init), _) => self.ssh_hooked(*id, init),
                    Out::Ssh(token, depth) => {
                        // A token with no conductor (its setup was refused) is dropped.
                        let Some(p) = self.panes.get_mut(id) else { continue };
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
                self.ssh_apply(*id);
            }
        }
        !ids.is_empty()
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
        self.redraw();
    }

    /// Applies what the pane's conductors asked for, until they ask nothing more.
    pub(super) fn ssh_apply(&mut self, pane: PaneId) {
        let mut redraw = false;
        loop {
            let Some(p) = self.panes.get_mut(&pane) else { return };
            let effects = p.conductor.as_mut().map(|c| c.take_effects()).unwrap_or_default();
            if effects.is_empty() {
                break;
            }
            for effect in effects {
                match effect {
                    Effect::Write(bytes) => p.write_no_broadcast(&bytes),
                    Effect::SessionWrite(bytes) => p.write_unless_held(&bytes),
                    Effect::Keys(bytes) => p.write_session(&bytes),
                    // `PS:25614-25617`: what was typed while connecting goes through sendKeys,
                    // ahead of later typing; its effects are applied by the next pass.
                    Effect::StopQueueingInput => {
                        let queued = p.ssh.stop_queueing();
                        if let Some(c) = p.conductor.as_mut() {
                            c.send_keys(&queued);
                        }
                    }
                    // Decision: Tandem sends no Initial Text anywhere yet; it goes here.
                    Effect::SendInitialText => {}
                    Effect::StateChanged => redraw = true,
                    Effect::Notice(text) => {
                        p.own_mut().append_banner(&text);
                        redraw = true;
                    }
                    // Task 9 replaces this with the full abort.
                    Effect::Abort(reason) => {
                        p.own_mut().print_message(&reason);
                        p.unhook_ssh();
                        redraw = true;
                    }
                }
            }
        }
        if redraw {
            self.redraw();
        }
    }
}

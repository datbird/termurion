//! The SSH integration handshake's requests from a pane's output (`TD:3955-4049`): the
//! connecting state and the conductor bootstrap, both held back until the conductor hook
//! exists (`ssh_session::CONDUCTOR_READY`), and EndSSH, which is not gated: with no hops
//! (all of 14a) it only ends connecting.

use super::*;
use crate::screen::Request;
use crate::ssh_session::{conductor_bytes, CONDUCTOR_READY, END_WRITE};

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
}

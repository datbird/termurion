//! The SSH integration handshake's requests from a pane's output (`TD:3955-4004`): the
//! connecting state and the conductor bootstrap, both held back until the conductor hook
//! exists (`ssh_session::CONDUCTOR_READY`).

use super::*;
use crate::screen::Request;
use crate::ssh_session::{conductor_bytes, CONDUCTOR_READY};

impl App {
    pub(super) fn ssh_request(&mut self, pane: PaneId, r: Request) {
        let Some(p) = self.panes.get_mut(&pane) else { return };
        match r {
            Request::SshWillBegin => p.ssh.begin(CONDUCTOR_READY),
            Request::SshSendConductor(h) => {
                if let Some(bytes) = conductor_bytes(&h, CONDUCTOR_READY) {
                    p.write_no_broadcast(&bytes);
                }
            }
            _ => {}
        }
    }
}

//! The default-terminal handoff on Windows, from `handoff_win`.

use super::super::pty::{Notify, Pty, PtyApi};

pub use crate::handoff_win::{Handoff, Server};

pub fn can_serve(api: &PtyApi) -> bool {
    api.pack.is_some()
}

pub fn register(send: Box<dyn Fn(Handoff) + Send + Sync>, api: &PtyApi) -> Result<Server, String> {
    let pack = api.pack.ok_or_else(|| "the pseudoconsole library cannot pack a console".to_string())?;
    Server::register(send, pack, api.close)
}

pub fn begin_closing() {
    crate::handoff_win::begin_closing()
}

pub fn into_pty(api: &PtyApi, mut h: Handoff, notify: Notify) -> Result<(Pty, String), String> {
    let title = std::mem::take(&mut h.title);
    let (hpc, input, output, client, live) = h.take();
    let mut pty = super::pty::from_handoff(api, hpc as _, input as _, output as _, client as _, notify)?;
    if let Some(l) = live {
        pty.hold(Box::new(l));
    }
    Ok((pty, title))
}

/// Startup: `server_repair` on the real registry. The recorded exe path is saved state, so it is
/// judged before it is probed: a path the judge refuses counts as still there.
pub fn repair_at_start() {
    let Ok(exe) = std::env::current_exe() else { return };
    let mut r = crate::reg::HkcuRegistry;
    let fix = crate::default_term::server_repair(&r, &exe, &|p| crate::localfs::judged(p) && !crate::localfs::local_exists(p));
    if !fix.is_empty()
        && let Err(e) = crate::reg::apply(&mut r, &fix) {
            crate::host_base::log(&format!("default terminal: could not repair the handoff class: {e}"));
        }
}

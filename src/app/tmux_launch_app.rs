//! Shell > tmux > New psmux Session and New tmux Session in WSL: a new tab runs the
//! control-mode client, or an info bar names the install command when the program is missing.

use super::*;
use crate::tmux_launch::{self, Found, Kind};

impl App {
    /// Starts a tmux session of `kind` in a new tab (or a new window, as New Tab would open).
    pub(super) fn start_tmux_session(&mut self, kind: Kind, event_loop: &ActiveEventLoop) {
        if let Found::No(text) = tmux_program_found(kind) {
            return self.tmux_program_missing(kind, &text);
        }
        self.start_command_override = Some(kind.command());
        if self.tabs.is_empty() || self.new_tab_opens_window(None) {
            let _ = self.open_window(event_loop, None);
        } else {
            self.new_tab_as(None, true);
        }
        self.start_command_override = None;
        self.redraw();
    }

    /// The info bar of a missing program, on the focused pane (a banner when there is none).
    fn tmux_program_missing(&mut self, kind: Kind, text: &str) {
        match self.focused_id() {
            Some(pane) => {
                self.ask(pane, kind.bar_id(), text, &["OK"], true);
            }
            None => self.banners.set("window", text.to_string()),
        }
    }
}

fn tmux_program_found(kind: Kind) -> Found {
    match kind {
        Kind::Psmux => {
            let path = std::env::var("PATH").unwrap_or_default();
            let ext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
            tmux_launch::psmux_found(&path, &ext, &|p| crate::localfs::is_local_file(p))
        }
        Kind::WslTmux => tmux_launch::wsl_found(wsl_has_tmux()),
    }
}

/// Runs `wsl.exe -e sh -c "command -v tmux"` for at most eight seconds (WSL may be starting).
fn wsl_has_tmux() -> Result<i32, bool> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let status = Command::new("wsl.exe").args(["-e", "sh", "-c", "command -v tmux"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).creation_flags(0x0800_0000).status();
        let _ = tx.send(status.map(|s| s.code().unwrap_or(-1)).map_err(|_| ()));
    });
    match rx.recv_timeout(std::time::Duration::from_secs(8)) {
        Ok(Ok(code)) => Ok(code),
        Ok(Err(())) => Err(false),
        Err(_) => Err(true),
    }
}

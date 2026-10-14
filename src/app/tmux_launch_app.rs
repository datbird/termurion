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
        Kind::WslTmux => {
            let root = std::env::var_os("SystemRoot").map_or_else(|| std::path::PathBuf::from("C:\\Windows"), std::path::PathBuf::from);
            tmux_launch::wsl_found(crate::localfs::is_local_file(&root.join("System32").join("wsl.exe")))
        }
    }
}

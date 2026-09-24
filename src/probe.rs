//! Drives real apps under ConPTY and checks that mouse input reaches them.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::conpty::Api;
use crate::mouse::{self, Action, Button, Event, Modes, Mods, Tracking};
use crate::pty::{self, Pty};
use crate::screen::Screen;

pub const COLS: u16 = 200;
pub const ROWS: u16 = 50;
pub const DISTRO: &str = "Ubuntu-Personal";

pub struct Session {
    pub pty: Pty,
    pub screen: Screen,
}

impl Session {
    pub fn start(api: &Api, cmdline: &str) -> Result<Session, String> {
        Ok(Session { pty: pty::spawn(api, cmdline, COLS, ROWS)?, screen: Screen::new(COLS as usize, ROWS as usize) })
    }

    /// Feeds output into the screen until `done` is true or time runs out. Writes
    /// the screen's query replies back to the app, so apps that ask do not hang.
    pub fn pump_until(&mut self, wait: Duration, mut done: impl FnMut(&Screen) -> bool) -> bool {
        let end = Instant::now() + wait;
        loop {
            if done(&self.screen) {
                return true;
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            if let Some(chunk) = self.pty.read_timeout(left.min(Duration::from_millis(50))) {
                let reply = self.screen.feed(&chunk);
                if !reply.is_empty() {
                    let _ = self.pty.write(&reply);
                }
            }
        }
    }

    pub fn send(&self, bytes: &[u8]) -> Result<(), String> {
        self.pty.write(bytes)
    }
}

pub struct Outcome {
    pub name: String,
    pub conpty: &'static str,
    pub pass: bool,
    pub detail: String,
}

type Scenario = Box<dyn Fn(&Api) -> Result<String, String>>;

/// Runs every scenario whose name contains `filter`. Ok(detail) passes, Err(detail) fails.
pub fn run_all(api: &Api, filter: &str) -> Vec<Outcome> {
    let mut list: Vec<(&str, Scenario)> = vec![("win-console", Box::new(win_console_mouse))];
    list.retain(|(name, _)| name.contains(filter));
    list.iter()
        .map(|(name, run)| {
            let result = run(api);
            Outcome {
                name: name.to_string(),
                conpty: api.source.label(),
                pass: result.is_ok(),
                detail: result.unwrap_or_else(|e| e),
            }
        })
        .collect()
}

pub fn wsl_path(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    match s.split_once(":/") {
        Some((drive, rest)) => format!("/mnt/{}/{rest}", drive.to_lowercase()),
        None => s,
    }
}

fn repo_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn press_release(s: &Session, modes: Modes, col: u32, row: u32) -> Result<(), String> {
    for action in [Action::Press, Action::Release] {
        let ev = Event { button: Button::Left, action, mods: Mods::default(), col, row };
        if let Some(bytes) = mouse::encode(modes, ev) {
            s.send(&bytes)?;
        }
    }
    Ok(())
}

/// A Windows console app reads mouse input with ReadConsoleInputW. ConPTY must ask
/// the terminal for mouse reports, then turn the reports into console events.
fn win_console_mouse(api: &Api) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?.with_file_name("mouse_echo_win.exe");
    if !exe.is_file() {
        return Err(format!("{} is not built. Run cargo build --bins first.", exe.display()));
    }
    let mut s = Session::start(api, &format!("\"{}\"", exe.display()))?;
    let asked = s.pump_until(Duration::from_secs(5), |sc| {
        sc.text().contains("READY") && sc.mouse_modes().tracking != Tracking::Off
    });
    if !s.screen.text().contains("READY") {
        return Err(format!("helper never printed READY. Screen: {:?}", s.screen.text().trim_end()));
    }
    let modes = s.screen.mouse_modes();
    // If ConPTY never asked, still send SGR clicks to learn whether they get through.
    let send_modes = if asked { modes } else { Modes { tracking: Tracking::Click, sgr: true } };
    press_release(&s, send_modes, 10, 5)?;
    let got = s.pump_until(Duration::from_secs(5), |sc| sc.text().contains("MOUSE x=10 y=5 buttons=1"));
    match (asked, got) {
        (true, true) => Ok(format!("modes {modes:?}")),
        (false, true) => Err("click arrived, but ConPTY never asked the terminal for mouse input".into()),
        (true, false) => Err(format!("ConPTY asked for {modes:?}, but the click never arrived")),
        (false, false) => Err("ConPTY never asked for mouse input, and the click never arrived".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wsl_path_maps_drive_letters() {
        assert_eq!(
            wsl_path(Path::new(r"C:\Users\datbi\src\tandem\probe\x.py")),
            "/mnt/c/Users/datbi/src/tandem/probe/x.py"
        );
    }
}

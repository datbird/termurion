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
    let sgr = |tracking| Modes { tracking, sgr: true };
    let legacy = Modes { tracking: Tracking::Click, sgr: false };
    let mut list: Vec<(&str, Scenario)> = vec![
        ("win-console", Box::new(win_console_mouse)),
        ("wsl-1000-sgr", Box::new(move |a| wsl_raw_mouse(a, &["1000", "1006"], sgr(Tracking::Click), 10, 5))),
        ("wsl-1002-sgr", Box::new(move |a| wsl_raw_mouse(a, &["1002", "1006"], sgr(Tracking::Drag), 10, 5))),
        ("wsl-1003-sgr", Box::new(move |a| wsl_raw_mouse(a, &["1003", "1006"], sgr(Tracking::Any), 10, 5))),
        ("wsl-1000-legacy", Box::new(move |a| wsl_raw_mouse(a, &["1000"], legacy, 10, 5))),
        // Legacy bytes past 127 are not valid UTF-8, and ConPTY turns them into U+FFFD.
        // The encoder must send nothing here.
        ("wsl-1000-legacy-col150", Box::new(move |a| wsl_raw_mouse(a, &["1000"], legacy, 150, 5))),
        ("vim-click", Box::new(vim_click)),
        ("tmux-wheel", Box::new(tmux_wheel)),
        ("htop", Box::new(htop_mouse)),
    ];
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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A raw WSL program turns on `dec` modes and prints what it receives. Every event
/// the encoder produces must arrive byte for byte.
fn wsl_raw_mouse(api: &Api, dec: &[&str], want: Modes, col: u32, row: u32) -> Result<String, String> {
    let script = wsl_path(&repo_dir().join("probe").join("mouse_echo.py"));
    let mut s = Session::start(api, &format!("wsl.exe -d {DISTRO} -e python3 {script} {}", dec.join(" ")))?;
    if !s.pump_until(Duration::from_secs(10), |sc| sc.text().contains("READY") && sc.mouse_modes() == want) {
        return Err(format!(
            "terminal saw modes {:?}, wanted {want:?}. Screen: {:?}",
            s.screen.mouse_modes(),
            s.screen.text().trim_end()
        ));
    }
    let none = Mods::default();
    let events = [
        Event { button: Button::Left, action: Action::Press, mods: none, col, row },
        Event { button: Button::Left, action: Action::Motion, mods: none, col: col + 1, row },
        Event { button: Button::None, action: Action::Motion, mods: none, col: col + 2, row },
        Event { button: Button::Left, action: Action::Release, mods: none, col: col + 1, row },
        Event { button: Button::WheelUp, action: Action::Press, mods: none, col, row },
    ];
    let mut expected = Vec::new();
    for ev in events {
        if let Some(bytes) = mouse::encode(want, ev) {
            s.send(&bytes)?;
            expected.push(format!("GOT {}", hex(&bytes)));
        }
    }
    if expected.is_empty() {
        s.send(b"q")?;
        return Ok("the encoder sent nothing, as ConPTY would corrupt these positions".into());
    }
    let all = s.pump_until(Duration::from_secs(5), |sc| {
        let text = sc.text();
        expected.iter().all(|e| text.contains(e.as_str()))
    });
    s.send(b"q")?;
    if all {
        return Ok(format!("{} events arrived intact", expected.len()));
    }
    let text = s.screen.text();
    let missing: Vec<&String> = expected.iter().filter(|e| !text.contains(e.as_str())).collect();
    let got: Vec<&str> = text.lines().map(str::trim_end).filter(|l| l.starts_with("GOT")).collect();
    Err(format!("missing {missing:?}; the app got {got:?}"))
}

/// Runs a command in WSL outside ConPTY and returns its stdout.
fn wsl(args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("wsl.exe")
        .args(["-d", DISTRO, "-e"])
        .args(args)
        .output()
        .map_err(|e| format!("wsl.exe: {e}"))?;
    if !out.status.success() {
        return Err(format!("wsl {args:?} failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A click in vim moves the cursor. vim then writes the cursor position to a file.
fn vim_click(api: &Api) -> Result<String, String> {
    wsl(&["rm", "-f", "/tmp/tandem_vim_pos"])?;
    let file = wsl_path(&repo_dir().join("probe").join("vim_target.txt"));
    let mut s = Session::start(
        api,
        &format!("wsl.exe -d {DISTRO} -e vim -u NONE -N -c \"set mouse=a ttymouse=sgr\" {file}"),
    )?;
    if !s.pump_until(Duration::from_secs(10), |sc| {
        sc.text().contains("line 01") && sc.mouse_modes().tracking != Tracking::Off
    }) {
        return Err(format!("vim never turned on mouse tracking; modes {:?}", s.screen.mouse_modes()));
    }
    let modes = s.screen.mouse_modes();
    // Column 4, row 2 (0-based) is line 3, column 5 in vim.
    press_release(&s, modes, 4, 2)?;
    std::thread::sleep(Duration::from_millis(300));
    s.send(b":call writefile([line('.') . ',' . col('.')], '/tmp/tandem_vim_pos')\r:qa!\r")?;
    s.pty.wait_exit(Duration::from_secs(5)).ok_or("vim did not exit")?;
    let pos = wsl(&["cat", "/tmp/tandem_vim_pos"])?;
    if pos.trim() == "3,5" {
        Ok(format!("cursor moved to 3,5; modes {modes:?}"))
    } else {
        Err(format!("cursor at {:?}, wanted 3,5; modes {modes:?}", pos.trim()))
    }
}

/// With tmux mouse on, a wheel-up over a pane enters copy mode.
fn tmux_wheel(api: &Api) -> Result<String, String> {
    // The probe uses its own tmux socket. No server running is the normal case here.
    let _ = wsl(&["tmux", "-L", "tandemprobe", "kill-server"]);
    let mut s = Session::start(
        api,
        &format!("wsl.exe -d {DISTRO} -e tmux -L tandemprobe -f /dev/null new-session -s probe ; set -g mouse on"),
    )?;
    let on = s.pump_until(Duration::from_secs(10), |sc| sc.mouse_modes().tracking != Tracking::Off);
    let modes = s.screen.mouse_modes();
    let result = if !on {
        Err(format!("tmux never turned on mouse tracking; modes {modes:?}"))
    } else {
        let ev = Event { button: Button::WheelUp, action: Action::Press, mods: Mods::default(), col: 10, row: 10 };
        s.send(&mouse::encode(modes, ev).ok_or("the encoder dropped the wheel event")?)?;
        let end = Instant::now() + Duration::from_secs(3);
        let mut in_mode = String::new();
        while Instant::now() < end {
            in_mode = wsl(&["tmux", "-L", "tandemprobe", "display", "-p", "-t", "probe", "#{pane_in_mode}"])?;
            if in_mode.trim() == "1" {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if in_mode.trim() == "1" {
            Ok(format!("wheel entered copy mode; modes {modes:?}"))
        } else {
            Err(format!("pane_in_mode is {:?} after the wheel; modes {modes:?}", in_mode.trim()))
        }
    };
    let _ = wsl(&["tmux", "-L", "tandemprobe", "kill-server"]);
    result
}

/// htop turns on mouse tracking when it starts.
fn htop_mouse(api: &Api) -> Result<String, String> {
    wsl(&["which", "htop"]).map_err(|_| "htop is not installed in WSL (Task 7 step 1)".to_string())?;
    let mut s = Session::start(api, &format!("wsl.exe -d {DISTRO} -e htop"))?;
    let on = s.pump_until(Duration::from_secs(10), |sc| sc.mouse_modes().tracking != Tracking::Off);
    let modes = s.screen.mouse_modes();
    s.send(b"q")?;
    s.pty.wait_exit(Duration::from_secs(5)).ok_or("htop did not exit on q")?;
    if on { Ok(format!("modes {modes:?}")) } else { Err("htop never turned on mouse tracking".into()) }
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
    fn hex_is_lowercase_pairs() {
        assert_eq!(hex(b"\x1b[<0;1;1M"), "1b5b3c303b313b314d");
    }

    #[test]
    fn wsl_path_maps_drive_letters() {
        assert_eq!(
            wsl_path(Path::new(r"C:\Users\datbi\src\tandem\probe\x.py")),
            "/mnt/c/Users/datbi/src/tandem/probe/x.py"
        );
    }
}

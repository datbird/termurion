//! Starting a tmux session from the Shell > tmux menu: psmux (native Windows tmux) or tmux
//! in WSL. Both run in a new tab as a control-mode client (`-CC`), attaching to the session
//! named `termurion` or making it. A program that is not installed gets an info bar with the
//! install command, never a tab that dies at once.

use std::path::{Path, PathBuf};

/// The session both menu items attach to or create.
pub const SESSION: &str = "termurion";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Psmux,
    WslTmux,
}

impl Kind {
    /// The command line of the new tab.
    pub fn command(self) -> String {
        match self {
            Kind::Psmux => format!("psmux.exe -CC new-session -A -s {SESSION}"),
            Kind::WslTmux => format!("wsl.exe -e tmux -CC new-session -A -s {SESSION}"),
        }
    }

    /// The command that installs what is missing.
    pub fn install_command(self) -> &'static str {
        match self {
            Kind::Psmux => "winget install psmux",
            Kind::WslTmux => "sudo apt install tmux",
        }
    }

    /// What the info bar says when the program is missing.
    pub fn missing_text(self) -> String {
        match self {
            Kind::Psmux => format!("psmux was not found. Install it with: {}", self.install_command()),
            Kind::WslTmux => format!("tmux was not found in WSL. Install it in your WSL distribution with: {}", self.install_command()),
        }
    }

    /// The info bar's id: one bar per kind.
    pub fn bar_id(self) -> &'static str {
        match self {
            Kind::Psmux => "tmux-missing-psmux",
            Kind::WslTmux => "tmux-missing-wsl",
        }
    }
}

/// The id prefix of both missing-program bars.
pub const MISSING_BAR: &str = "tmux-missing-";

/// The text when WSL itself is not there (`wsl.exe` cannot start).
pub const NO_WSL_TEXT: &str = "WSL was not found. Install it with: wsl --install";

/// The first file named `name` (a bare name, with `.exe` and the other PATHEXT endings tried
/// when it has no dot) in a folder of `path_var`. `exists` is the file check.
pub fn find_on_path(name: &str, path_var: &str, pathext: &str, exists: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    let endings: Vec<String> = if name.contains('.') {
        vec![String::new()]
    } else {
        std::iter::once(String::new()).chain(pathext.split(';').filter(|e| !e.is_empty()).map(|e| e.to_ascii_lowercase())).collect()
    };
    for dir in path_var.split(';').filter(|d| !d.is_empty()) {
        for e in &endings {
            let p = Path::new(dir.trim_matches('"')).join(format!("{name}{e}"));
            if exists(&p) {
                return Some(p);
            }
        }
    }
    None
}

/// What the check for the program found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    Yes,
    /// Missing: the info bar's text.
    No(String),
    /// The check did not finish in time (WSL starting up): start the tab anyway.
    Unknown,
}

/// psmux is found when `psmux.exe` is on PATH.
pub fn psmux_found(path_var: &str, pathext: &str, exists: &dyn Fn(&Path) -> bool) -> Found {
    match find_on_path("psmux.exe", path_var, pathext, exists) {
        Some(_) => Found::Yes,
        None => Found::No(Kind::Psmux.missing_text()),
    }
}

/// How `wsl.exe -e sh -c "command -v tmux"` ended: the exit code, `Err(false)` when `wsl.exe`
/// could not start, `Err(true)` when it did not finish in time.
pub fn wsl_found(result: Result<i32, bool>) -> Found {
    match result {
        Ok(0) => Found::Yes,
        Ok(_) => Found::No(Kind::WslTmux.missing_text()),
        Err(false) => Found::No(NO_WSL_TEXT.to_string()),
        Err(true) => Found::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_attach_or_create_the_termurion_session() {
        assert_eq!(Kind::Psmux.command(), "psmux.exe -CC new-session -A -s termurion");
        assert_eq!(Kind::WslTmux.command(), "wsl.exe -e tmux -CC new-session -A -s termurion");
    }

    #[test]
    fn missing_programs_name_the_install_command() {
        assert_eq!(Kind::Psmux.missing_text(), "psmux was not found. Install it with: winget install psmux");
        assert!(Kind::WslTmux.missing_text().ends_with("sudo apt install tmux"));
        assert_ne!(Kind::Psmux.bar_id(), Kind::WslTmux.bar_id());
        assert!(Kind::Psmux.bar_id().starts_with(MISSING_BAR) && Kind::WslTmux.bar_id().starts_with(MISSING_BAR));
    }

    #[test]
    fn path_search_tries_the_endings() {
        let have = |p: &Path| p == Path::new("C:\\tools").join("psmux.exe");
        assert_eq!(find_on_path("psmux", "C:\\bin;C:\\tools", ".COM;.EXE", &have), Some(Path::new("C:\\tools").join("psmux.exe")));
        assert_eq!(find_on_path("psmux.exe", "\"C:\\tools\";C:\\bin", ".EXE", &have), Some(Path::new("C:\\tools").join("psmux.exe")));
        assert_eq!(find_on_path("psmux", "C:\\bin;;", ".EXE", &have), None);
        assert_eq!(psmux_found("C:\\tools", ".EXE", &have), Found::Yes);
        assert_eq!(psmux_found("C:\\bin", ".EXE", &have), Found::No(Kind::Psmux.missing_text()));
        assert_eq!(psmux_found("", ".EXE", &have), Found::No(Kind::Psmux.missing_text()));
    }

    #[test]
    fn wsl_results_decide() {
        assert_eq!(wsl_found(Ok(0)), Found::Yes);
        assert_eq!(wsl_found(Ok(1)), Found::No(Kind::WslTmux.missing_text()));
        assert_eq!(wsl_found(Err(false)), Found::No(NO_WSL_TEXT.to_string()));
        assert_eq!(wsl_found(Err(true)), Found::Unknown);
    }
}

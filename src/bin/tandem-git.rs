//! The git menu's wrapper (milestone 9i): the Windows port of iTerm2's
//! `sources/iterm2_git_wrapper.sh`. A git menu command's single-use window runs
//! `tandem-git.exe <git> <arguments...>`: it prints the banner, runs git on this console,
//! and exits 0 when git succeeds. Otherwise it asks to be disinterred, prints the error, and
//! waits, so the window stays until it is closed. A console program (ConPTY runs it).

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use tandem::git_state as gs;
use tandem::git_wrapper;

fn main() {
    let mut argv = std::env::args_os().skip(1);
    let program = argv.next().map_or_else(|| PathBuf::from("git"), PathBuf::from);
    let raw: Vec<OsString> = argv.collect();
    let args: Vec<String> = raw.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    let cwd = std::env::current_dir().unwrap_or_default();
    let mut out = std::io::stdout();
    let _ = out.write_all(git_wrapper::banner(&cwd.to_string_lossy(), &args).as_bytes());
    let _ = out.flush();
    if run(&program, &cwd, &raw) {
        std::process::exit(0);
    }
    let _ = out.write_all(git_wrapper::failure(&args).as_bytes());
    let _ = out.flush();
    wait_forever();
}

/// Runs git on this console with the status bar's environment rules. Never in, or from, a
/// network path: `git` is found on PATH without looking in network folders, and any other
/// program must be on a local drive (Tandem vetted its `gitSearchPath` folder).
fn run(program: &Path, cwd: &Path, args: &[OsString]) -> bool {
    if !tandem::prompted::local_path_ok(&cwd.to_string_lossy()) {
        eprintln!("git does not run in a network folder.\r");
        return false;
    }
    let git = if program == Path::new("git") {
        gs::find_git(&std::env::var("PATH").unwrap_or_default(), &|p: &Path| p.is_file())
    } else {
        tandem::prompted::local_path_ok(&program.to_string_lossy()).then(|| program.to_path_buf())
    };
    let Some(git) = git else {
        eprintln!("git.exe was not found in a folder on this computer.\r");
        return false;
    };
    let mut cmd = std::process::Command::new(git);
    cmd.args(args).current_dir(cwd);
    for k in gs::GIT_ENV_REMOVED {
        cmd.env_remove(k);
    }
    for (k, v) in gs::GIT_ENV {
        cmd.env(k, v);
    }
    match cmd.status() {
        Ok(s) => s.success(),
        Err(e) => {
            eprintln!("{e}\r");
            false
        }
    }
}

/// `while read; do :; done` that never ends: reads what is typed, and sleeps once input
/// has ended.
fn wait_forever() -> ! {
    let mut stdin = std::io::stdin();
    let mut buf = [0u8; 4096];
    loop {
        match stdin.read(&mut buf) {
            Ok(n) if n > 0 => {}
            _ => std::thread::sleep(std::time::Duration::from_secs(3600)),
        }
    }
}

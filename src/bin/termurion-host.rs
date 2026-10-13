//! The session host: runs Tandem's shells so they outlive a crashed Tandem.
#![windows_subsystem = "windows"]

fn main() {
    #[cfg(windows)]
    termurion::dll::harden_process();
    #[cfg(windows)]
    std::process::exit(run());
}

#[cfg(windows)]
fn run() -> i32 {
    use termurion::conpty::{self, Source};
    use termurion::host;
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(std::path::PathBuf::from)).unwrap_or_default();
    let api = match conpty::load(&Source::Shipped(exe_dir)).or_else(|e| conpty::load(&Source::Inbox).map_err(|e2| format!("{e}; {e2}"))) {
        Ok(api) => api,
        Err(e) => {
            host::log(&format!("no ConPTY: {e}"));
            return 1;
        }
    };
    let name = match termurion::winpipe::host_pipe_name() {
        Ok(n) => n,
        Err(e) => {
            host::log(&e);
            return 1;
        }
    };
    match host::run(&name, api, host::Options::default()) {
        Ok(()) => 0,
        Err(e) => {
            host::log(&e);
            1
        }
    }
}

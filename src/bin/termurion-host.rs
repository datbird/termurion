//! The session host: runs Termurion's shells so they outlive a crashed Termurion.
#![windows_subsystem = "windows"]

fn main() {
    #[cfg(windows)]
    termurion::dll::harden_process();
    #[cfg(windows)]
    termurion::crashdump::install();
    #[cfg(windows)]
    std::process::exit(run());
}

#[cfg(windows)]
fn run() -> i32 {
    use termurion::conpty::{self, Source};
    use termurion::host;
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(std::path::PathBuf::from)).unwrap_or_default();
    let api = match conpty::load(&Source::Shipped(exe_dir.clone())).or_else(|e| conpty::load(&Source::Inbox).map_err(|e2| format!("{e}; {e2}"))) {
        Ok(api) => api,
        Err(e) => {
            host::log(&format!("no ConPTY: {e}"));
            return 1;
        }
    };
    let name = match termurion::platform::ipc::host_pipe_name() {
        Ok(n) => n,
        Err(e) => {
            host::log(&e);
            return 1;
        }
    };
    // A vanished Termurion is started again from this folder (`host_watch`).
    let app = exe_dir.join("termurion.exe");
    let opts = host::Options { reopen: app.is_file().then_some(app), ..host::Options::default() };
    match host::run(&name, api, opts) {
        Ok(()) => 0,
        Err(e) => {
            host::log(&e);
            1
        }
    }
}

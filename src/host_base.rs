//! The bits of the session host that every module may use: its log and its exe name. The host
//! itself (`host`, `host_client`) needs a pty, so it is not built everywhere.

/// The session host program beside the app; `start_host_process` starts it by this file name.
pub const HOST_EXE: &str = "termurion-host.exe";

/// `host.log` beside Termurion's settings.
pub fn log_path() -> std::path::PathBuf {
    crate::globals::default_path().with_file_name("host.log")
}

/// Appends a line to the host log (the host has no window and no console).
pub fn log(line: &str) {
    use std::io::Write;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log_path()) {
        let _ = writeln!(f, "{stamp} [{}] {line}", std::process::id());
    }
}

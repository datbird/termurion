//! `open` on macOS, `xdg-open` on Linux.

use std::process::{Command, Stdio};

#[cfg(target_os = "macos")]
const OPENER: &str = "open";
#[cfg(not(target_os = "macos"))]
const OPENER: &str = "xdg-open";

pub fn open_url(url: &str, _background: bool) -> Result<(), String> {
    // The URL is one argv entry, never a shell string.
    let mut child = Command::new(OPENER)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{OPENER}: {e}"))?;
    // xdg-open may sit until a fallback browser closes, so wait off the UI thread (no zombie left).
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

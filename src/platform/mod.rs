//! Per-OS code lives here. App modules call through this layer and never carry a cfg of their own.

use std::fmt;

#[cfg(windows)]
pub mod windows;
#[cfg(unix)]
pub mod unix;
#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;

pub mod a11y;
pub mod app_events;
pub mod approval_key;
pub mod browser;
pub mod caption;
pub mod chrome;
pub mod clipboard;
pub mod clock;
pub mod cmd_help;
pub mod coproc;
pub mod credentials;
pub mod dialog;
pub mod desktops;
pub mod display;
pub mod files;
pub mod handoff;
pub mod hotkey;
pub mod drag;
pub mod http;
pub mod ipc;
pub mod log_store;
pub mod input;
pub mod key_record;
pub mod keys;
pub mod locale;
pub mod mdns;
pub mod user_auth;
pub mod screen_lock;
pub mod watch;
pub mod metrics;
pub mod gitrun;
pub mod logout;
pub mod net;
pub mod paths;
pub mod notify;
pub mod print;
pub mod privacy;
pub mod provision;
pub mod proc;
pub mod proc_table;
pub mod pty;
pub mod quarantine;
pub mod registry;
pub mod shell_extras;
pub mod sizing;
pub mod shell_open;
pub mod shellfs;
pub mod ssh_secret;
pub mod sysinfo;
pub mod tcp_owner;
pub mod trust;
pub mod utilities;
pub mod update_fetch;
pub mod window;
pub mod window_style;

/// Name of the OS this build runs on, for messages.
#[cfg(windows)]
pub const OS_NAME: &str = "Windows";
#[cfg(target_os = "macos")]
pub const OS_NAME: &str = "macOS";
#[cfg(all(unix, not(target_os = "macos")))]
pub const OS_NAME: &str = "Linux";

/// True on Windows; for the odd value that only differs by OS (a test's expectation, a backend list).
pub const IS_WINDOWS: bool = cfg!(windows);

/// A test's value that differs by OS: the Windows string byte for byte, or the Unix one.
#[cfg(test)]
pub fn by_os<'a>(windows: &'a str, unix: &'a str) -> &'a str {
    if IS_WINDOWS { windows } else { unix }
}

/// A feature that has no implementation on this OS yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsupported {
    pub feature: &'static str,
}

impl fmt::Display for Unsupported {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} is not available on {} yet", self.feature, OS_NAME)
    }
}

impl std::error::Error for Unsupported {}

impl From<Unsupported> for String {
    fn from(u: Unsupported) -> String {
        u.to_string()
    }
}

/// Shorthand for a stub's return value.
pub fn unsupported<T>(feature: &'static str) -> Result<T, Unsupported> {
    Err(Unsupported { feature })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_names_feature_and_os() {
        assert_eq!(
            Unsupported { feature: "Notifications" }.to_string(),
            format!("Notifications is not available on {} yet", OS_NAME)
        );
    }

    #[test]
    fn unsupported_into_string() {
        let s = String::from(Unsupported { feature: "X" });
        assert!(s.contains("X is not available"));
    }
}

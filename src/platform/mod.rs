//! Per-OS code lives here. App modules call through this layer and never carry a cfg of their own.

use std::fmt;

#[cfg(windows)]
pub mod windows;
#[cfg(unix)]
pub mod unix;

pub mod a11y;
pub mod browser;
pub mod caption;
pub mod chrome;
pub mod clipboard;
pub mod clock;
pub mod coproc;
pub mod credentials;
pub mod dialog;
pub mod display;
pub mod http;
pub mod ipc;
pub mod input;
pub mod key_record;
pub mod keys;
pub mod locale;
pub mod net;
pub mod notify;
pub mod print;
pub mod proc;
pub mod pty;
pub mod shell_extras;
pub mod sizing;
pub mod shell_open;
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

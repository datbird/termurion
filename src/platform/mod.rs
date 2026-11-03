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
pub mod iterm2_prefs;
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

/// A Windows words and the Mac words that replace them, longest first. Only phrases that cannot be
/// read another way: "Windows" alone is also the app's windows, so it is never replaced by itself.
const MAC_WORDS: &[(&str, &str)] = &[
    ("Windows Credential Manager", "macOS Keychain"),
    ("Credential Manager", "Keychain"),
    ("Windows Hello (or your Windows password)", "Touch ID (or your password)"),
    ("Windows Settings", "System Settings"),
    ("Windows shutting down, restarting, or signing out", "macOS shutting down, restarting, or signing out"),
    ("Windows' own shortcuts", "the system's own shortcuts"),
    ("; Windows keeps three-finger gestures for itself", ""),
    ("Win (Command) key", "command (\u{2318}) key"),
    ("Alt (Option) key", "option (\u{2325}) key"),
    ("Left Win key", "Left command (\u{2318}) key"),
    ("Right Win key", "Right command (\u{2318}) key"),
    ("left Win key", "left command (\u{2318}) key"),
    ("right Win key", "right command (\u{2318}) key"),
    ("Left Ctrl key", "Left control (\u{2303}) key"),
    ("Right Ctrl key", "Right control (\u{2303}) key"),
    ("left Ctrl key", "left control (\u{2303}) key"),
    ("right Ctrl key", "right control (\u{2303}) key"),
    ("Left Alt key", "Left option (\u{2325}) key"),
    ("Right Alt key", "Right option (\u{2325}) key"),
    ("left Alt key", "left option (\u{2325}) key"),
    ("right Alt key", "right option (\u{2325}) key"),
    ("Alt key", "Option key"),
    ("Ctrl key", "Control key"),
    ("Left Win", "Left Command"),
    ("Right Win", "Right Command"),
    ("Left Alt", "Left Option"),
    ("Right Alt", "Right Option"),
    ("Shortcut key+Alt+Number", "Option+Command+Number"),
    ("Shortcut key+Number", "Command+Number"),
    ("Alt+Number", "Option+Number"),
    ("Ctrl+Number", "Control+Number"),
    ("Ctrl+Win+letter", "Control+Command+letter"),
    ("(Alt: in a new window)", "(Option: in a new window)"),
    ("Ctrl-Click", "Control-Click"),
    ("non-Win click", "non-Command click"),
    ("Win-click", "Command-click"),
    ("Hold Win to", "Hold Command to"),
    ("Win+Q", "\u{2318}Q"),
    ("Win+C", "\u{2318}C"),
    ("Win+1", "\u{2318}1"),
    ("Exclude from taskbar and Alt+Tab switcher", "Exclude from Dock and \u{2318}-Tab Application Switcher"),
    ("Exclude From Taskbar and Alt+Tab Switcher", "Exclude From Dock and \u{2318}-Tab Application Switcher"),
    ("Taskbar and Alt+Tab Switcher", "Dock and \u{2318}-Tab Application Switcher"),
    ("taskbar and Alt+Tab switcher", "Dock and \u{2318}-Tab application switcher"),
    ("Flash the taskbar button", "Bounce the Dock icon"),
    ("taskbar button\u{2019}s badge", "Dock icon\u{2019}s badge"),
    ("taskbar button\u{2019}s Jump List", "Dock icon\u{2019}s menu"),
    ("taskbar button", "Dock icon"),
    ("taskbar icon", "Dock icon"),
    ("Alt+Tab", "\u{2318}-Tab"),
    ("Taskbar", "Dock"),
    ("taskbar", "Dock"),
    ("Explorer", "Finder"),
];

/// A text written in Windows words, in the words of this OS: on macOS the Finder, the Keychain, the
/// Dock and the Mac's key names; Windows and Linux get it back unchanged, byte for byte.
pub fn native_words(text: &str) -> std::borrow::Cow<'_, str> {
    if cfg!(target_os = "macos") {
        std::borrow::Cow::Owned(mac_words(text))
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

/// `native_words` for macOS, whatever OS this is (so the table is tested everywhere).
pub fn mac_words(text: &str) -> String {
    let mut s = text.to_string();
    for (from, to) in MAC_WORDS {
        if s.contains(from) {
            s = s.replace(from, to);
        }
    }
    s
}

/// A test's shortcut text: the Windows and Linux form (Ctrl+Shift+1), or the Mac glyph form on
/// macOS, where Cmd is a key and the labels use the Mac's glyphs.
#[cfg(test)]
pub fn by_cmd<'a>(other: &'a str, mac: &'a str) -> &'a str {
    if keys::CMD_IS_A_KEY { mac } else { other }
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

#[cfg(test)]
mod words_tests {
    use super::*;

    #[test]
    fn mac_words_name_the_mac_things() {
        assert_eq!(mac_words("Reveal in Explorer"), "Reveal in Finder");
        assert_eq!(mac_words("Keys are stored securely in the Windows Credential Manager."), "Keys are stored securely in the macOS Keychain.");
        assert_eq!(mac_words("Left Win key:"), "Left command (\u{2318}) key:");
        assert_eq!(mac_words("Remap right Alt key to:"), "Remap right option (\u{2325}) key to:");
        assert_eq!(mac_words("Right Win"), "Right Command");
        assert_eq!(mac_words("Exclude From Taskbar and Alt+Tab Switcher"), "Exclude From Dock and \u{2318}-Tab Application Switcher");
        assert_eq!(mac_words("Flash the taskbar button when the bell rings"), "Bounce the Dock icon when the bell rings");
        // Windows alone is the app's windows: never replaced.
        assert_eq!(mac_words("Windows have square corners."), "Windows have square corners.");
        assert_eq!(mac_words("Restore windows to same desktops"), "Restore windows to same desktops");
    }

    #[test]
    fn native_words_leave_windows_and_linux_alone() {
        let text = "Reveal in Explorer, Left Win key, the taskbar";
        if cfg!(target_os = "macos") {
            assert_ne!(native_words(text), text);
        } else {
            assert_eq!(native_words(text), text);
        }
    }
}

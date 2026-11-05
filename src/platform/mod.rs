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

/// Windows words and the Linux ones, longest first: only the credential store has another name.
const LINUX_WORDS: &[(&str, &str)] = &[
    ("the Windows Credential Manager", "the Secret Service"),
    ("Windows Credential Manager", "the Secret Service"),
    ("the Credential Manager", "the Secret Service"),
    ("Credential Manager", "the Secret Service"),
];

/// A text written in Windows words, in the words of this OS: on macOS the Finder, the Keychain, the
/// Dock and the Mac's key names, on Linux the Secret Service; Windows gets it back unchanged.
pub fn native_words(text: &str) -> std::borrow::Cow<'_, str> {
    if cfg!(target_os = "macos") {
        std::borrow::Cow::Owned(mac_words(text))
    } else if cfg!(target_os = "linux") {
        std::borrow::Cow::Owned(linux_words(text))
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

/// `native_words` for Linux, whatever OS this is (so the table is tested everywhere).
pub fn linux_words(text: &str) -> String {
    let mut s = text.to_string();
    for (from, to) in LINUX_WORDS {
        if s.contains(from) {
            s = s.replace(from, to);
        }
    }
    s
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
    /// The feature is a plural noun ("Notifications are"), so the verb agrees.
    pub plural: bool,
}

impl fmt::Display for Unsupported {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} not available on {} yet", self.feature, if self.plural { "are" } else { "is" }, OS_NAME)
    }
}

impl std::error::Error for Unsupported {}

impl From<Unsupported> for String {
    fn from(u: Unsupported) -> String {
        u.to_string()
    }
}

/// Shorthand for a stub's return value (a singular feature).
pub fn unsupported<T>(feature: &'static str) -> Result<T, Unsupported> {
    Err(Unsupported { feature, plural: false })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_names_feature_and_os() {
        assert_eq!(
            Unsupported { feature: "Notifications", plural: true }.to_string(),
            format!("Notifications are not available on {} yet", OS_NAME)
        );
        assert_eq!(Unsupported { feature: "Printing", plural: false }.to_string(), format!("Printing is not available on {} yet", OS_NAME));
        assert_eq!(
            Unsupported { feature: "The command line utilities", plural: true }.to_string(),
            format!("The command line utilities are not available on {} yet", OS_NAME)
        );
    }

    /// Every feature named in the source reads with its own verb (no guess from a final "s").
    #[test]
    fn every_unsupported_text_has_its_own_verb() {
        let want = [
            "Bonjour discovery is", "Checking for updates is", "Coprocesses are", "Copy with styles as RTF is", "Disable App Nap is",
            "Download quarantine is", "Dynamic Profiles are", "Editing a file with its app is", "Finding the process on a connection is",
            "Finding the process on a port is", "Installing Python for scripts is", "Moving to the Trash is",
            "Notifications are", "Password manager command line tools are", "Password manager is", "Printing is", "Python API scripts are",
            "Reporting to Termurion is", "Running a command line is", "Running git is", "Saving the debug log is",
            "Saving the debug log's files is", "Settings from a URL are", "The color dialog is",
            "The command line utilities are", "The hotkey window on Wayland is", "The hotkey window without X11 is", "The registry is", "The resident icon is", "The terminal utilities are", "Verifying a signed script archive is", "Virtual desktops are", "X is",
        ];
        fn walk(dir: &std::path::Path, out: &mut Vec<String>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(std::fs::read_to_string(&p).unwrap());
                }
            }
        }
        let mut sources = Vec::new();
        walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut sources);
        let mut found = std::collections::BTreeSet::new();
        for src in &sources {
            for part in src.split("Unsupported { feature: \"").skip(1) {
                let Some((feature, rest)) = part.split_once('"') else { continue };
                let Some(plural) = rest.strip_prefix(", plural: ").and_then(|r| r.split(' ').next()) else { continue };
                let u = Unsupported { feature: Box::leak(feature.to_string().into_boxed_str()), plural: plural == "true" };
                found.insert(u.to_string().replace(&format!(" not available on {OS_NAME} yet"), ""));
            }
        }
        assert_eq!(found, want.iter().map(|s| s.to_string()).collect(), "an Unsupported text changed; check its verb");
        // The two the old guess got wrong, word for word.
        assert_eq!(Unsupported { feature: "Checking for updates", plural: false }.to_string(), format!("Checking for updates is not available on {OS_NAME} yet"));
        assert_eq!(Unsupported { feature: "Installing Python for scripts", plural: false }.to_string(), format!("Installing Python for scripts is not available on {OS_NAME} yet"));
    }

    #[test]
    fn unsupported_into_string() {
        let s = String::from(Unsupported { feature: "X", plural: false });
        assert!(s.contains("X is not available"));
    }
}

#[cfg(test)]
mod native_words_tests {
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
    fn linux_names_the_secret_service() {
        assert_eq!(linux_words("Keys are stored securely in the Windows Credential Manager."), "Keys are stored securely in the Secret Service.");
        assert_eq!(linux_words("it stores them in Windows Credential Manager, but"), "it stores them in the Secret Service, but");
        assert_eq!(linux_words("removed manually from Credential Manager if"), "removed manually from the Secret Service if");
        assert_eq!(linux_words("Windows Credential Manager"), "the Secret Service");
        // Windows alone is the app's windows, and the Mac words stay out of it.
        assert_eq!(linux_words("Windows have square corners. Reveal in Explorer"), "Windows have square corners. Reveal in Explorer");
        if cfg!(all(unix, not(target_os = "macos"))) {
            assert_eq!(native_words("the Windows Credential Manager"), "the Secret Service");
        }
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

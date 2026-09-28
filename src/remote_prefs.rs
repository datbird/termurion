//! "Load settings from a custom folder or URL" (iTerm2's `iTermRemotePreferences`, `RP`): where the
//! settings file is, which keys travel through it, the file itself, when the quit question runs, and
//! the curl arguments for a URL. The app side is `src/app/prefs_ui.rs`.
//!
//! The file is `tandem-settings.toml`: `default` (the default profile's Guid), `[[profile]]` tables in
//! `profiles.toml`'s form and a `[settings]` table in `settings.toml`'s form, holding syncable keys
//! only. iTerm2 keeps both in one plist; Tandem keeps them in two files and joins them here.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use toml::Value;

/// iTerm2's `<bundle id>.plist` (`RP:134-142`).
pub const PREFS_FILE: &str = "tandem-settings.toml";
/// `kPreferenceKeyLoadPrefsFromCustomFolder` (`PREF:52`).
pub const LOAD_KEY: &str = "LoadPrefsFromCustomFolder";
/// `kPreferenceKeyCustomFolder` (`PREF:67`).
pub const FOLDER_KEY: &str = "PrefsCustomFolder";
/// The quit question's silence flag and the Save changes popup's "have selection" (`PREF:62`).
pub const HAVE_SELECTION_KEY: &str = "NoSyncNeverRemindPrefsChangesLostForFile";
/// The Save changes popup's tag, and the silenced quit question's answer (`PREF:59`).
pub const SELECTION_KEY: &str = "NoSyncNeverRemindPrefsChangesLostForFile_selection";
/// The URL whose next load asks first (`RP:25`).
pub const PROMPT_URL_KEY: &str = "NoSyncPromptBeforeLoadingPrefsFromURL";
/// `noSyncDownloadPrefsTimeout` (`ASM:550`).
pub const TIMEOUT_KEY: &str = "NoSyncDownloadPrefsTimeout";

/// curl's exit code for "Operation timeout", NSURLErrorTimedOut's place (`RP:222`).
pub const CURL_TIMED_OUT: i32 = 28;

pub const FAILED_URL: &str = "Failed to load settings from URL. Falling back to local copy.";
pub const UNKNOWN_ERROR: &str = "unknown error";
pub const REVEAL_IN_SETTINGS: &str = "Reveal in Settings";
pub const TRY_AGAIN: &str = "Try Again Without Timeout";
pub const URL_PROMPT: &str = "Load settings from URL? Some changes were made to the local copy that will be lost.";
pub const KEEP_LOCAL: &str = "Keep Local Changes";
pub const DISABLE_URL: &str = "Disable Loading from URL";
pub const DISCARD_LOCAL: &str = "Discard Local Changes";
pub const ERROR_LOADING: &str = "Error Loading Settings";
pub const LOCATION_NOT_SET: &str =
    "You have enabled \u{201c}Load settings from a custom folder or URL\u{201d} in settings but the location is not set.";
pub const DONT_LOAD: &str = "Don\u{2019}t Load Remote Settings";
pub const FAILED_FOLDER: &str = "Failed to load settings from custom directory. Falling back to local copy.";
pub const OVERWRITE: &str = "Overwrite";
pub const CANNOT_COPY_URL: &str = "Settings cannot be copied to a URL.";
/// Tandem's: iTerm2 names its plist's path here.
pub const CANNOT_COPY_URL_DETAIL: &str =
    "To make it available, save the settings to a folder with Save Now and then manually copy tandem-settings.toml to your hosting provider.";
pub const FAILED_COPY: &str = "Failed to copy settings to custom directory.";
pub const COPY: &str = "Copy";
pub const LOSE_CHANGES: &str = "Lose Changes";
pub const COPY_NOW: &str = "Copy local settings to custom folder now?";
pub const DONT_COPY: &str = "Don\u{2019}t Copy";

// General > Settings rows (`XIB:8360-8478`).
pub const FOLDER_HINT: &str = "Enter path to folder with settings";
pub const BROWSE: &str = "Browse";
pub const SAVE_CHANGES: &str = "Save changes:";
pub const AUTOMATICALLY: &str = "Automatically";
pub const MANUALLY: &str = "Manually";
pub const WHEN_QUITTING: &str = "When Quitting";
pub const SAVE_NOW: &str = "Save Now";
/// iTerm2's xib has two spaces after "External".
pub const EXTERNAL_SETTINGS: &str = "External settings:";
pub const BACKUP_RESTORE: &str = "Backup & restore:";
pub const RESET: &str = "Reset:";
pub const EXPORT_ALL: &str = "Export All Settings and Data\u{2026}";
pub const IMPORT_ALL: &str = "Import All Settings and Data\u{2026}";
pub const ERASE_ALL: &str = "Erase All Settings and Data\u{2026}";

/// "HTTP request failed: %@" (`RP:209-232`), "unknown error" when curl said nothing.
pub fn http_failed(error: &str) -> String {
    format!("HTTP request failed: {}", if error.is_empty() { UNKNOWN_ERROR } else { error })
}

pub fn missing_file(file: &str) -> String {
    format!("Missing or malformed file at \u{201c}{file}\u{201d}")
}

pub fn remote_changed(file: &str) -> String {
    format!("Settings at {file} changed since Tandem started. Overwrite it?")
}

pub fn tried_to_copy(from: &str, to: &str) -> String {
    format!("Tried to copy {from} to {to}")
}

pub fn settings_changed(folder: &str) -> String {
    format!("Settings have changed. Copy them to {folder}?")
}

/// `stringIsUrlLike` (`NSStringITerm.m:1194-1196`).
pub fn is_url(text: &str) -> bool {
    text.starts_with("http://") || text.starts_with("https://")
}

/// A network or device path, never probed as a file (`scripts::is_network_path`).
pub fn is_unc(text: &str) -> bool {
    crate::scripts::is_network_path(text)
}

/// `expandedCustomFolderOrURL` (`RP:113-120`): a URL as is, else tilde-expanded.
pub fn expanded(text: &str, home: &Path) -> String {
    if is_url(text) {
        return text.to_string();
    }
    crate::scripts::expand_tilde(text, home).to_string_lossy().into_owned()
}

/// `prefsFilenameWithBaseDir:` (`RP:134-142`): the base itself when it names an existing `.toml`
/// file (iTerm2's `.plist`), else `tandem-settings.toml` inside it.
pub fn prefs_file(base: &Path, exists: &dyn Fn(&Path) -> bool) -> PathBuf {
    let named = base.extension().is_some_and(|e| e.eq_ignore_ascii_case("toml"));
    if named && exists(base) {
        return base.to_path_buf();
    }
    base.join(PREFS_FILE)
}

/// `remoteLocationIsValid` (`RP:315-328`): a URL with a host, or a writable folder. A network path,
/// or a folder reached through a link to one (`local_folder`), is never valid and never probed.
pub fn location_valid(text: &str, home: &Path, read_link: &dyn Fn(&Path) -> Option<PathBuf>, writable: &dyn Fn(&Path) -> bool) -> bool {
    if is_url(text) {
        return url_has_host(text);
    }
    if text.trim().is_empty() || is_unc(text) {
        return false;
    }
    let folder = expanded(text, home);
    local_folder(Path::new(&folder), read_link) && writable(Path::new(&folder))
}

/// Whether the settings folder (or its file) may be read, probed or written: not a network path,
/// and neither it nor any folder above it a link or junction whose chain reaches one
/// (`scripts::ancestors_stay_local`, as the Scripts folder and the export are checked). A mapped
/// drive letter stays allowed; a link is refused, since its text does not show that it leaves the
/// machine. Reads only the links.
pub fn local_folder(location: &Path, read_link: &dyn Fn(&Path) -> Option<PathBuf>) -> bool {
    !is_unc(&location.to_string_lossy()) && crate::scripts::ancestors_stay_local(location, read_link)
}

/// The authority between `//` and the path, less any user and port, is not empty.
fn url_has_host(url: &str) -> bool {
    let rest = url.split_once("://").map_or("", |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if host.starts_with('[') { host } else { host.split(':').next().unwrap_or("") };
    !host.trim().is_empty()
}

/// `directoryIsWritable:` (`NSFileManager+iTerm.m:401-418`): writes `.testwritable.<pid>` and
/// removes it. Never called for a URL; a network path answers false without touching it.
pub fn writable(dir: &Path) -> bool {
    let text = dir.to_string_lossy();
    if text.trim().is_empty() || is_unc(&text) {
        return false;
    }
    let probe = dir.join(format!(".testwritable.{}", std::process::id()));
    if std::fs::write(&probe, "test").is_err() {
        return false;
    }
    let _ = std::fs::remove_file(&probe);
    true
}

/// `iTermRemotePreferencesKeyIsSyncable` (`RP:144-175`): whether a key travels through the file.
pub fn syncable(key: &str) -> bool {
    const EXEMPT: [&str; 12] = [
        LOAD_KEY,
        FOLDER_KEY,
        "Secure Input",
        "moveToApplicationsFolderAlertSuppress",
        "iTerm Version",
        "CGFontRenderingFontSmoothingDisabled",
        "PreventEscapeSequenceFromChangingProfile",
        "PreventEscapeSequenceFromClearingHistory",
        "Coprocess MRU",
        "MetalCaptureEnabled",
        "MetalCaptureEnabledDate",
        "Snippets",
    ];
    !EXEMPT.contains(&key) && !["NS", "SU", "NoSync", "UK"].iter().any(|p| key.starts_with(p))
}

/// The settings a custom folder or URL holds: iTerm2's remote plist.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub settings: BTreeMap<String, Value>,
    pub default: String,
    pub profiles: Vec<BTreeMap<String, Value>>,
}

#[derive(Serialize, Deserialize)]
struct FileFormat {
    #[serde(default)]
    default: String,
    #[serde(default)]
    settings: BTreeMap<String, Value>,
    #[serde(default)]
    profile: Vec<BTreeMap<String, Value>>,
}

impl Snapshot {
    /// The local settings as the file holds them: syncable keys only.
    pub fn from_local(settings: &BTreeMap<String, Value>, default: &str, profiles: &[BTreeMap<String, Value>]) -> Snapshot {
        Snapshot {
            settings: settings.iter().filter(|(k, _)| syncable(k)).map(|(k, v)| (k.clone(), v.clone())).collect(),
            default: default.to_string(),
            profiles: profiles.to_vec(),
        }
    }

    pub fn to_toml(&self) -> String {
        let file = FileFormat { default: self.default.clone(), settings: self.settings.clone(), profile: self.profiles.clone() };
        toml::to_string(&file).unwrap_or_default()
    }

    /// The file's snapshot; None when it is malformed or holds nothing (iTerm2's nil or empty
    /// dictionary, `RP:1528`).
    pub fn parse(text: &str) -> Option<Snapshot> {
        let file: FileFormat = toml::from_str(text).ok()?;
        let snap = Snapshot { settings: file.settings, default: file.default, profiles: file.profile };
        (!snap.is_empty()).then_some(snap)
    }

    /// No settings and no profiles. A default Guid alone names nothing.
    pub fn is_empty(&self) -> bool {
        self.settings.is_empty() && self.profiles.is_empty()
    }
}

/// `RP:1531-1551`: every local syncable key goes, then the remote's syncable keys come.
pub fn apply(remote: &Snapshot, settings: &mut BTreeMap<String, Value>) {
    settings.retain(|k, _| !syncable(k));
    for (k, v) in &remote.settings {
        if syncable(k) {
            settings.insert(k.clone(), v.clone());
        }
    }
}

/// `localPrefsDifferFromSavedRemotePrefs` (`RP:1564-1592`): a syncable key, the default profile or a
/// profile differs. An empty saved snapshot differs from nothing, as in iTerm2.
pub fn differs(saved: &Snapshot, local: &Snapshot) -> bool {
    if saved.is_empty() {
        return false;
    }
    let synced = |s: &Snapshot| s.settings.iter().filter(|(k, _)| syncable(k)).map(|(k, v)| (k.clone(), v.clone())).collect::<BTreeMap<_, _>>();
    synced(saved) != synced(local) || saved.default != local.default || saved.profiles != local.profiles
}

/// `iTermPreferenceSavePrefsMode` (`iTermPreferences.h:74-78`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveMode {
    OnQuit,
    Never,
    Always,
}

impl SaveMode {
    pub fn tag(self) -> i64 {
        match self {
            SaveMode::OnQuit => 0,
            SaveMode::Never => 1,
            SaveMode::Always => 2,
        }
    }

    /// An unknown tag is Never, the default.
    pub fn from_tag(tag: i64) -> SaveMode {
        match tag {
            0 => SaveMode::OnQuit,
            2 => SaveMode::Always,
            _ => SaveMode::Never,
        }
    }
}

/// The popup's choice (`GP:2184-2192`): Never until a person picks one.
pub fn save_mode(have_selection: bool, selection: i64) -> SaveMode {
    if have_selection { SaveMode::from_tag(selection) } else { SaveMode::Never }
}

/// What quitting does with the custom folder (`RP:1646-1744`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuitStep {
    Nothing,
    /// The location goes in `NoSyncPromptBeforeLoadingPrefsFromURL` for the next launch's question.
    RememberUrl,
    SaveSilently,
    /// "Settings have changed. Copy them to ...?"; silenced, it answers with the popup's tag.
    Ask,
    ClearUrlPrompt,
}

/// `applicationWillTerminate`: `differs` is local against the snapshot saved at launch; `loaded`
/// is whether one was saved.
pub fn quit_step(enabled: bool, url: bool, differs: bool, mode: SaveMode, loaded: bool) -> QuitStep {
    if !enabled {
        QuitStep::Nothing
    } else if differs {
        if url {
            QuitStep::RememberUrl
        } else if mode == SaveMode::Always {
            QuitStep::SaveSilently
        } else {
            QuitStep::Ask
        }
    } else if loaded {
        QuitStep::ClearUrlPrompt
    } else {
        QuitStep::Nothing
    }
}

/// `RP:241-244`: the URL about to load is the one remembered at the last quit.
pub fn should_prompt_url(remembered: &str, location: &str) -> bool {
    !location.is_empty() && remembered == location
}

/// curl.exe's arguments for a URL load: fail on HTTP errors, follow redirects, http and https
/// only, and the timeout when one applies ("Try Again Without Timeout" passes None).
pub fn curl_args(url: &str, timeout: Option<f64>) -> Vec<String> {
    let mut args: Vec<String> = ["--fail", "--location", "--silent", "--show-error", "--proto", "=http,https"].map(String::from).to_vec();
    if let Some(t) = timeout {
        args.push("--max-time".into());
        args.push(format!("{t}"));
    }
    args.push(url.into());
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn map(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    #[test]
    fn locations_follow_iterm2() {
        assert!(is_url("https://x/y"));
        assert!(is_url("http://x"));
        assert!(!is_url("ftp://x"));
        assert!(!is_url("C:\\s"));
        assert!(is_unc(r"\\host\s"));
        assert!(is_unc("//host/s"));
        assert!(!is_unc(r"D:\s"));
        let home = Path::new(r"C:\Users\u");
        assert_eq!(expanded("~\\s", home), r"C:\Users\u\s");
        assert_eq!(expanded("https://h/~p", home), "https://h/~p");
        let none = |_: &Path| false;
        assert_eq!(prefs_file(Path::new(r"D:\s"), &none), Path::new(r"D:\s\tandem-settings.toml"));
        let mine = |p: &Path| p == Path::new(r"D:\s\mine.toml");
        assert_eq!(prefs_file(Path::new(r"D:\s\mine.toml"), &mine), Path::new(r"D:\s\mine.toml"));
        assert_eq!(prefs_file(Path::new(r"D:\s\mine.toml"), &none), Path::new(r"D:\s\mine.toml\tandem-settings.toml"));
        // Only a named .toml file counts (iTerm2's .plist).
        let txt = |p: &Path| p == Path::new(r"D:\s\mine.txt");
        assert_eq!(prefs_file(Path::new(r"D:\s\mine.txt"), &txt), Path::new(r"D:\s\mine.txt\tandem-settings.toml"));
        let always = |_: &Path| true;
        let no_link = |_: &Path| None;
        assert!(location_valid("https://h/p", home, &no_link, &none));
        assert!(!location_valid("https://", home, &no_link, &always));
        assert!(!location_valid("  ", home, &no_link, &always));
        let probed = Cell::new(false);
        let spy = |_: &Path| {
            probed.set(true);
            true
        };
        assert!(!location_valid(r"\\h\s", home, &no_link, &spy));
        assert!(!location_valid("//h/s", home, &no_link, &spy));
        assert!(!probed.get(), "a network path is never probed");
        assert!(location_valid("https://h/p", home, &no_link, &spy));
        assert!(!probed.get(), "a URL is never probed");
        let d = |p: &Path| p == Path::new(r"D:\s");
        assert!(location_valid("D:\\s", home, &no_link, &d));
        assert!(!location_valid("D:\\t", home, &no_link, &d));
        let h = |p: &Path| p == Path::new(r"C:\Users\u\s");
        assert!(location_valid("~\\s", home, &no_link, &h));
    }

    #[test]
    fn a_folder_linked_to_a_network_path_is_never_probed() {
        // The final review's Important 2 (invariant 5): D:\s is a link to \\h\s, or sits under
        // one, so it is neither probed, read nor written.
        let home = Path::new(r"C:\Users\u");
        let to_share = |p: &Path| (p == Path::new(r"D:\s")).then(|| PathBuf::from(r"\\h\s"));
        let never = |_: &Path| -> bool { panic!("a folder reached through a link to a network path is never probed") };
        assert!(!location_valid(r"D:\s", home, &to_share, &never));
        assert!(!location_valid(r"D:\s\sub", home, &to_share, &never), "a folder under the link");
        assert!(!location_valid(r"D:\s\mine.toml", home, &to_share, &never));
        assert!(!local_folder(Path::new(r"D:\s"), &to_share));
        assert!(!local_folder(Path::new(r"D:\s\tandem-settings.toml"), &to_share));
        assert!(!local_folder(Path::new(r"\\h\s"), &|_| None));
        // A link to a local folder, and a plain folder, stay usable.
        let to_local = |p: &Path| (p == Path::new(r"D:\s")).then(|| PathBuf::from(r"E:\t"));
        assert!(local_folder(Path::new(r"D:\s"), &to_local));
        assert!(local_folder(Path::new(r"D:\t"), &to_share));
        assert!(location_valid(r"D:\s", home, &to_local, &|p| p == Path::new(r"D:\s")));
    }

    #[test]
    fn writable_probes_and_cleans_up() {
        let dir = std::env::temp_dir().join(format!("tandem-remote-prefs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(writable(&dir));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "the probe file is removed");
        assert!(!writable(&dir.join("missing")));
        assert!(!writable(Path::new(" ")));
        assert!(!writable(Path::new(r"\\host\share")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn syncable_follows_iterm2() {
        for key in [
            "LoadPrefsFromCustomFolder",
            "PrefsCustomFolder",
            "Secure Input",
            "moveToApplicationsFolderAlertSuppress",
            "iTerm Version",
            "CGFontRenderingFontSmoothingDisabled",
            "PreventEscapeSequenceFromChangingProfile",
            "PreventEscapeSequenceFromClearingHistory",
            "Coprocess MRU",
            "MetalCaptureEnabled",
            "MetalCaptureEnabledDate",
            "Snippets",
            "NSWindow Frame",
            "SUEnableAutomaticChecks",
            "NoSyncX",
            "UKCrashReporterLastCrashReportDate",
        ] {
            assert!(!syncable(key), "{key}");
        }
        for key in ["FocusFollowsMouse", "tmuxTitlePrefix", "GlobalKeyMap"] {
            assert!(syncable(key), "{key}");
        }
    }

    #[test]
    fn snapshot_follows_iterm2() {
        let local = map(&[("FocusFollowsMouse", Value::Boolean(true)), ("NoSyncX", Value::Boolean(true)), (LOAD_KEY, Value::Boolean(true))]);
        let profiles = vec![map(&[("Guid", Value::String("g1".into())), ("Name", Value::String("Remote".into())), ("Columns", Value::Integer(100))])];
        let snap = Snapshot::from_local(&local, "g1", &profiles);
        assert_eq!(snap.settings, map(&[("FocusFollowsMouse", Value::Boolean(true))]));
        assert_eq!((snap.default.as_str(), &snap.profiles), ("g1", &profiles));
        assert_eq!(Snapshot::parse(&snap.to_toml()), Some(snap.clone()));
        assert_eq!(Snapshot::parse(""), None);
        assert_eq!(Snapshot::parse("not = [toml"), None);
        assert_eq!(Snapshot::parse("default = \"g1\"\n[settings]\n"), None);
        assert!(Snapshot::default().is_empty());
        assert!(!snap.is_empty());

        let remote = Snapshot { settings: map(&[("A", Value::Integer(1))]), ..Default::default() };
        let mut settings = map(&[("A", Value::Integer(2)), ("B", Value::Integer(3)), ("NoSyncX", Value::Boolean(true))]);
        apply(&remote, &mut settings);
        assert_eq!(settings, map(&[("A", Value::Integer(1)), ("NoSyncX", Value::Boolean(true))]));

        let base = Snapshot { settings: map(&[("A", Value::Integer(1))]), default: "g1".into(), profiles: profiles.clone() };
        let mut nosync = base.clone();
        nosync.settings.insert("NoSyncX".into(), Value::Boolean(true));
        assert!(!differs(&base, &nosync));
        assert!(!differs(&base, &base.clone()));
        let mut added = base.clone();
        added.settings.insert("B".into(), Value::Integer(2));
        assert!(differs(&base, &added));
        let mut removed = base.clone();
        removed.settings.clear();
        assert!(differs(&base, &removed));
        let mut changed = base.clone();
        changed.settings.insert("A".into(), Value::Integer(9));
        assert!(differs(&base, &changed));
        let mut profile = base.clone();
        profile.profiles[0].insert("Columns".into(), Value::Integer(80));
        assert!(differs(&base, &profile));
        let mut default = base.clone();
        default.default = "g2".into();
        assert!(differs(&base, &default));
        // Nothing was saved at launch: iTerm2 reports no difference (`RP:1573`).
        assert!(!differs(&Snapshot::default(), &added));
    }

    #[test]
    fn quit_step_follows_iterm2() {
        assert_eq!(save_mode(false, 2), SaveMode::Never);
        assert_eq!(save_mode(true, 0), SaveMode::OnQuit);
        assert_eq!(save_mode(true, 1), SaveMode::Never);
        assert_eq!(save_mode(true, 2), SaveMode::Always);
        for m in [SaveMode::OnQuit, SaveMode::Never, SaveMode::Always] {
            assert_eq!(SaveMode::from_tag(m.tag()), m);
        }
        assert_eq!((SaveMode::OnQuit.tag(), SaveMode::Never.tag(), SaveMode::Always.tag()), (0, 1, 2));
        for mode in [SaveMode::OnQuit, SaveMode::Never, SaveMode::Always] {
            for loaded in [false, true] {
                for url in [false, true] {
                    for d in [false, true] {
                        assert_eq!(quit_step(false, url, d, mode, loaded), QuitStep::Nothing);
                    }
                }
                assert_eq!(quit_step(true, true, true, mode, loaded), QuitStep::RememberUrl);
            }
            assert_eq!(quit_step(true, false, false, mode, true), QuitStep::ClearUrlPrompt);
            assert_eq!(quit_step(true, false, false, mode, false), QuitStep::Nothing);
        }
        assert_eq!(quit_step(true, false, true, SaveMode::Always, true), QuitStep::SaveSilently);
        assert_eq!(quit_step(true, false, true, SaveMode::Never, true), QuitStep::Ask);
        // The silenced question answers Copy by itself.
        assert_eq!(quit_step(true, false, true, SaveMode::OnQuit, true), QuitStep::Ask);
        assert!(should_prompt_url("https://h/p", "https://h/p"));
        assert!(!should_prompt_url("", "https://h/p"));
        assert!(!should_prompt_url("https://h/q", "https://h/p"));
    }

    #[test]
    fn curl_follows_the_rule() {
        assert_eq!(
            curl_args("https://h/p", Some(5.0)),
            ["--fail", "--location", "--silent", "--show-error", "--proto", "=http,https", "--max-time", "5", "https://h/p"]
        );
        assert_eq!(curl_args("https://h/p", None), ["--fail", "--location", "--silent", "--show-error", "--proto", "=http,https", "https://h/p"]);
        assert_eq!(curl_args("https://h/p", Some(0.5))[7], "0.5");
        assert_eq!(CURL_TIMED_OUT, 28);
    }

    #[test]
    fn texts_follow_iterm2() {
        assert_eq!(http_failed("boom"), "HTTP request failed: boom");
        assert_eq!(http_failed(""), "HTTP request failed: unknown error");
        assert_eq!(missing_file(r"D:\s\tandem-settings.toml"), "Missing or malformed file at \u{201c}D:\\s\\tandem-settings.toml\u{201d}");
        assert_eq!(remote_changed(r"D:\s"), "Settings at D:\\s changed since Tandem started. Overwrite it?");
        assert_eq!(tried_to_copy("a", "b"), "Tried to copy a to b");
        assert_eq!(settings_changed(r"D:\s"), "Settings have changed. Copy them to D:\\s?");
        assert_eq!(
            [
                FAILED_URL,
                UNKNOWN_ERROR,
                REVEAL_IN_SETTINGS,
                TRY_AGAIN,
                URL_PROMPT,
                KEEP_LOCAL,
                DISABLE_URL,
                DISCARD_LOCAL,
                ERROR_LOADING,
                LOCATION_NOT_SET,
                DONT_LOAD,
                FAILED_FOLDER,
                OVERWRITE,
                CANNOT_COPY_URL,
                CANNOT_COPY_URL_DETAIL,
                FAILED_COPY,
                COPY,
                LOSE_CHANGES,
                COPY_NOW,
                DONT_COPY,
                FOLDER_HINT,
                BROWSE,
                SAVE_CHANGES,
                AUTOMATICALLY,
                MANUALLY,
                WHEN_QUITTING,
                SAVE_NOW,
                EXTERNAL_SETTINGS,
                BACKUP_RESTORE,
                RESET,
                EXPORT_ALL,
                IMPORT_ALL,
                ERASE_ALL,
            ],
            [
                "Failed to load settings from URL. Falling back to local copy.",
                "unknown error",
                "Reveal in Settings",
                "Try Again Without Timeout",
                "Load settings from URL? Some changes were made to the local copy that will be lost.",
                "Keep Local Changes",
                "Disable Loading from URL",
                "Discard Local Changes",
                "Error Loading Settings",
                "You have enabled \u{201c}Load settings from a custom folder or URL\u{201d} in settings but the location is not set.",
                "Don\u{2019}t Load Remote Settings",
                "Failed to load settings from custom directory. Falling back to local copy.",
                "Overwrite",
                "Settings cannot be copied to a URL.",
                "To make it available, save the settings to a folder with Save Now and then manually copy tandem-settings.toml to your hosting provider.",
                "Failed to copy settings to custom directory.",
                "Copy",
                "Lose Changes",
                "Copy local settings to custom folder now?",
                "Don\u{2019}t Copy",
                "Enter path to folder with settings",
                "Browse",
                "Save changes:",
                "Automatically",
                "Manually",
                "When Quitting",
                "Save Now",
                "External settings:",
                "Backup & restore:",
                "Reset:",
                "Export All Settings and Data\u{2026}",
                "Import All Settings and Data\u{2026}",
                "Erase All Settings and Data\u{2026}",
            ]
        );
        assert_eq!(
            (PREFS_FILE, LOAD_KEY, FOLDER_KEY, HAVE_SELECTION_KEY, SELECTION_KEY, PROMPT_URL_KEY, TIMEOUT_KEY),
            (
                "tandem-settings.toml",
                "LoadPrefsFromCustomFolder",
                "PrefsCustomFolder",
                "NoSyncNeverRemindPrefsChangesLostForFile",
                "NoSyncNeverRemindPrefsChangesLostForFile_selection",
                "NoSyncPromptBeforeLoadingPrefsFromURL",
                "NoSyncDownloadPrefsTimeout"
            )
        );
    }
}

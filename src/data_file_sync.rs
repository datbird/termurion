//! Snippets, notes and icon customizations through the settings folder (milestone 13y2 Task 9a;
//! iTerm2's `iTermRemoteDataFileSync`, `Settings/iTermRemoteDataFileSync.swift`, and the
//! reconcile in `Settings/iTermRemotePreferences.m`). Pure: the allowlist, the signatures and
//! the decisions. The app side is `app::data_files_ui`.
//!
//! The files in the folder are untrusted input (the 13y1 Imported trust class): a name never
//! comes from the folder (only the four below are ever read or written), a file larger than
//! `MAX_FILE_BYTES` or not shaped like the file it claims to be is refused (`valid_content`),
//! and nothing in them runs.

use std::time::Duration;

use sha2::{Digest, Sha256};

/// The folder under the settings folder that holds the files (`iTermRemoteDataFileSync.swift:56`).
pub const DATA_DIR: &str = "Data";

/// iTerm2's `snippets.plist`, `notes.rtfd`, `graphic_colors.json` and `graphic_icons.json`, with
/// Tandem's own file names for the first two; each relative to `%APPDATA%\Termurion` locally and to
/// `<folder>\Data` remotely. Custom icon images are not synced.
pub const ALLOWLIST: [&str; 4] = ["snippets.json", "notes.txt", crate::tab_icons::COLORS_FILE, crate::tab_icons::ICONS_FILE];

/// How long a read of the folder may take before the launch goes on without it (`:51`).
pub const REMOTE_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// The largest file read from either side: a bigger one is not a snippet list, a note or an icon
/// map a person typed, and the read never goes past it.
pub const MAX_FILE_BYTES: u64 = 4 << 20;

/// The per-folder dictionaries in `settings.toml` (`NoSync`, so no settings file carries them),
/// keyed by the folder as typed, with iTerm2's names.
pub const CONSENT_KEY: &str = "NoSyncSyncDataFilesWithCustomFolder";
pub const BASELINE_KEY: &str = "NoSyncRemoteDataFileSyncBaselineSignatures";
pub const LAST_PUSHED_KEY: &str = "NoSyncRemoteDataFileSyncLastPushedLocal";
pub const DISCARD_PENDING_KEY: &str = "NoSyncRemoteDataFileSyncDiscardPending";

/// `RemotePreferences.SyncDataFilesPrompt` (`:798-801`); `%@` is the folder.
pub const CONSENT_TEXT: &str = "Tandem can also keep your snippets, global notes, and session icon customizations in the settings folder \u{201c}%@\u{201d} so they sync across machines. Notes and snippets can contain sensitive text such as passwords or tokens. Sync them too?";
pub const SYNC_TOO: &str = "Sync These Too";
pub const JUST_SETTINGS: &str = "Just Settings";
/// `RemotePreferences.DataFileConflictPrompt` (`:1368-1370`), "this Mac" as "this PC".
pub const CONFLICT_TEXT: &str = "Snippets, notes, or icon customizations changed both on this PC and in the settings folder \u{201c}%@\u{201d} since they were last in sync. Which copy would you like to keep?";
pub const USE_THIS_PC: &str = "Use This PC\u{2019}s";
pub const USE_FOLDER: &str = "Use Settings Folder\u{2019}s";
/// The quit question when settings and data both changed (`:1676`).
pub const COPY_BOTH_TEXT: &str = "Settings and your snippets, notes, or icon customizations have changed. Copy them to %@?";
/// The quit question when only the data changed (`:1719`).
pub const COPY_DATA_TEXT: &str = "Your snippets, notes, or icon customizations changed. Copy them to %@?";

/// A text with its `%@` replaced by the folder, as the person typed it, cleaned of anything a
/// shown name keeps out.
pub fn with_folder(text: &str, folder: &str) -> String {
    text.replacen("%@", &crate::inert_files::display_text(folder, 260, false), 1)
}

/// The hex SHA-256 over the allowlist in name order, each name framed with its presence and
/// length, so a missing file differs from an empty one and the order given does not matter
/// (`iTermRemoteDataFileSync.swift:283-420`). A name outside the allowlist is ignored.
pub fn signature(files: &[(&str, Option<Vec<u8>>)]) -> String {
    let mut hash = Sha256::new();
    for name in ALLOWLIST {
        let content = files.iter().find(|(n, _)| *n == name).and_then(|(_, c)| c.as_ref());
        hash.update((name.len() as u64).to_be_bytes());
        hash.update(name.as_bytes());
        match content {
            Some(bytes) => {
                hash.update([1u8]);
                hash.update((bytes.len() as u64).to_be_bytes());
                hash.update(bytes);
            }
            None => hash.update([0u8]),
        }
    }
    hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// The signature of no files at all: a side with this holds none of them.
pub fn empty_signature() -> String {
    signature(&[])
}

/// Whether bytes read from the folder (or from a file there) may become the local file `name`:
/// within the size cap and shaped like it. Snippets are a JSON object with a `snippets` list,
/// notes UTF-8 text, the icon and color maps JSON objects. Nothing here decodes an image.
pub fn valid_content(name: &str, bytes: &[u8]) -> bool {
    if bytes.len() as u64 > MAX_FILE_BYTES || !ALLOWLIST.contains(&name) {
        return false;
    }
    if name == "snippets.json" {
        return std::str::from_utf8(bytes).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok()).is_some_and(|v| v.get("snippets").is_some_and(serde_json::Value::is_array));
    }
    if name == "notes.txt" {
        return std::str::from_utf8(bytes).is_ok();
    }
    crate::tab_icons::is_map_file(bytes)
}

/// What the launch (or a Save Now) does about the files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reconcile {
    Nothing,
    /// The folder holds some of them and nothing was answered for it yet.
    AskConsent,
    /// The folder's copy replaces the local one.
    Pull,
    /// The local copy goes to the folder (a union: nothing there is deleted).
    Push,
    /// Both moved since they were last the same: ask which copy to keep.
    Conflict,
}

/// `reconcile` (`iTermRemotePreferences.m:776-812`, `iTermRemoteDataFileSync.swift:283-420`).
/// `consent` is the folder's recorded answer; signatures are `signature`'s; `baseline` is the
/// signature both sides last had.
pub fn reconcile(consent: Option<bool>, remote_has_any: bool, local: &str, remote: &str, baseline: Option<&str>) -> Reconcile {
    match consent {
        // iTerm2 skips the question only when neither side has a file (`RP:789-796`).
        None => return if remote_has_any || local != empty_signature() { Reconcile::AskConsent } else { Reconcile::Nothing },
        Some(false) => return Reconcile::Nothing,
        Some(true) => {}
    }
    if local == remote {
        return Reconcile::Nothing;
    }
    match baseline {
        None => {
            let local_has_any = local != empty_signature();
            match (local_has_any, remote_has_any) {
                (false, true) => Reconcile::Pull,
                (true, false) => Reconcile::Push,
                (true, true) => Reconcile::Conflict,
                (false, false) => Reconcile::Nothing,
            }
        }
        Some(b) => match (local != b, remote != b) {
            (false, true) => Reconcile::Pull,
            (true, false) => Reconcile::Push,
            (true, true) => Reconcile::Conflict,
            (false, false) => Reconcile::Nothing,
        },
    }
}

/// Whether a silent push may happen now (`:533-600`): consent given, a first reconcile done (a
/// baseline), the local copy changed since the baseline and is not what was last pushed, and the
/// folder still holds what the baseline says (otherwise the next reconcile decides).
pub fn push_allowed(consent: bool, baseline: Option<&str>, local: &str, last_pushed: Option<&str>, remote_now: &str) -> bool {
    let Some(baseline) = baseline else { return false };
    consent && local != baseline && last_pushed != Some(local) && remote_now == baseline
}

#[cfg(test)]
mod tests {
    use super::*;

    const L: &str = "local";
    const R: &str = "remote";

    fn some_local() -> String {
        signature(&[("notes.txt", Some(b"x".to_vec()))])
    }

    #[test]
    fn data_file_sync_follows_iterm2() {
        assert_eq!(reconcile(None, false, &empty_signature(), &empty_signature(), None), Reconcile::Nothing);
        // A machine with data and an empty folder is asked too (I1), and pushes once it agrees.
        assert_eq!(reconcile(None, false, &some_local(), &empty_signature(), None), Reconcile::AskConsent);
        assert_eq!(reconcile(Some(true), false, &some_local(), &empty_signature(), None), Reconcile::Push);
        assert_eq!(reconcile(None, true, L, R, None), Reconcile::AskConsent);
        assert_eq!(reconcile(Some(false), true, L, R, None), Reconcile::Nothing);
        // No baseline yet.
        assert_eq!(reconcile(Some(true), true, &empty_signature(), R, None), Reconcile::Pull);
        assert_eq!(reconcile(Some(true), false, &some_local(), &empty_signature(), None), Reconcile::Push);
        assert_eq!(reconcile(Some(true), true, &some_local(), R, None), Reconcile::Conflict);
        assert_eq!(reconcile(Some(true), true, R, R, None), Reconcile::Nothing);
        // With a baseline.
        assert_eq!(reconcile(Some(true), true, "B", R, Some("B")), Reconcile::Pull);
        assert_eq!(reconcile(Some(true), true, L, "B", Some("B")), Reconcile::Push);
        assert_eq!(reconcile(Some(true), true, L, R, Some("B")), Reconcile::Conflict);
        assert_eq!(reconcile(Some(true), true, "B", "B", Some("B")), Reconcile::Nothing);
        // Pushes.
        assert!(!push_allowed(true, None, L, None, R));
        assert!(!push_allowed(true, Some("B"), "B", None, "B"));
        assert!(!push_allowed(true, Some("B"), L, Some(L), "B"));
        assert!(!push_allowed(true, Some("B"), L, None, R));
        assert!(!push_allowed(false, Some("B"), L, None, "B"));
        assert!(push_allowed(true, Some("B"), L, None, "B"));
    }

    #[test]
    fn signature_frames_presence_and_ignores_order() {
        let missing = signature(&[("notes.txt", None)]);
        let empty = signature(&[("notes.txt", Some(Vec::new()))]);
        assert_ne!(missing, empty);
        let a = signature(&[("snippets.json", Some(b"a".to_vec())), ("notes.txt", Some(b"b".to_vec()))]);
        let b = signature(&[("notes.txt", Some(b"b".to_vec())), ("snippets.json", Some(b"a".to_vec()))]);
        assert_eq!(a, b);
        // A name outside the allowlist counts for nothing.
        assert_eq!(signature(&[("evil.exe", Some(b"x".to_vec()))]), empty_signature());
    }

    #[test]
    fn texts_are_iterm2s_with_the_replacements() {
        assert!(CONSENT_TEXT.starts_with("Tandem can also keep your snippets, global notes, and session icon customizations"));
        assert!(CONFLICT_TEXT.contains("this PC") && !CONFLICT_TEXT.contains("Mac"));
        assert_eq!(USE_THIS_PC, "Use This PC\u{2019}s");
        assert_eq!(USE_FOLDER, "Use Settings Folder\u{2019}s");
        assert_eq!(with_folder(COPY_DATA_TEXT, "D:\\sync"), "Your snippets, notes, or icon customizations changed. Copy them to D:\\sync?");
        // The folder is shown cleaned.
        assert_eq!(with_folder("%@", "a\u{202e}b"), "ab");
    }

    #[test]
    fn untrusted_content_is_refused() {
        assert!(valid_content("snippets.json", br#"{"snippets": []}"#));
        assert!(!valid_content("snippets.json", b"MZ\x90"));
        assert!(!valid_content("snippets.json", br#"{"snippets": 3}"#));
        assert!(valid_content("notes.txt", "hi".as_bytes()));
        assert!(!valid_content("notes.txt", &[0xff, 0xfe, 0x00]));
        assert!(valid_content(crate::tab_icons::ICONS_FILE, br#"{"git": ["git"]}"#));
        assert!(!valid_content(crate::tab_icons::ICONS_FILE, b"[1,2]"));
        assert!(!valid_content("..\\evil.json", b"{}"));
        assert!(!valid_content("notes.txt", &vec![b'a'; (MAX_FILE_BYTES + 1) as usize]));
    }
}

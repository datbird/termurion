//! Session archives (milestone 13q Task 8): the file names, the archive window's table, the
//! recent list and the failure text (`PSW:1676-1738`, `PTV:1006-1022`, `AMB:17-110`,
//! `PT:4636-4682`). Pure; the app side is `app/archive_ui.rs`.

use crate::arrangement::Value;
use crate::stamps::Parts;

pub const EXTENSION: &str = "itermarchive";
pub const ARCHIVE_KEY: &str = "Archive";
pub const RECENTS_KEY: &str = "NoSyncRecentArchives";
pub const MAX_RECENTS: usize = 10;
pub const RESTORED_BANNER: &str = "Archive Restored";
pub const MENU_ID: &str = "archive:";
/// The notification's title when an archive could not be written.
pub const FAILED_TITLE: &str = "Archiving failed";

/// What a session name with nothing in it is called (iTerm2's "iTerm2 Session", `PTV:1006-1022`).
const DEFAULT_SESSION: &str = "Tandem Session";

/// The longest session name taken, in UTF-16 units: the whole name stays under Windows' limit.
const MAX_SESSION_NAME: usize = 100;

/// A session name made safe for a file name: every character Windows forbids (and "/") becomes
/// "_", controls too, and the length is bounded. An empty result is the default name.
fn clean_session(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        let c = if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '_' } else { c };
        if out.encode_utf16().count() + c.len_utf16() > MAX_SESSION_NAME {
            break;
        }
        out.push(c);
    }
    let out = out.trim_end_matches(['.', ' ']).to_string();
    if out.is_empty() { DEFAULT_SESSION.to_string() } else { out }
}

/// "<date and time> - <session name>.itermarchive" (`PSW:1676-1695`): the date short ("9/27/26")
/// and the time medium ("3:04:05 PM") of en-US, spaces made "_", "/" made "-", "," removed, and
/// ":" made "." because Windows file names cannot hold it (Decision).
pub fn auto_file_name(p: &Parts, session_name: &str) -> String {
    let hour = match p.hour % 12 {
        0 => 12,
        h => h,
    };
    let half = if p.hour < 12 { "AM" } else { "PM" };
    let stamp = format!("{}-{}-{:02}_{}.{:02}.{:02}_{}", p.month, p.day, p.year % 100, hour, p.minute, p.second, half);
    crate::session_log::safe_file_name(&format!("{stamp} - {}.{EXTENSION}", clean_session(session_name)))
}

/// The save dialog's default name: "<session name>.itermarchive", else "Tandem Session"
/// (`PTV:1006-1022`).
pub fn save_name(session_name: &str) -> String {
    crate::session_log::safe_file_name(&format!("{}.{EXTENSION}", clean_session(session_name)))
}

/// The window table of an archive (`PT:4636-4682`): the saved window with the `Archive` table
/// holding the session's size, the selected tab 0, and no hotkey or hide-after-opening.
pub fn archive_window(mut window: Value, cols: u16, rows: u16) -> Value {
    if let Some(t) = window.as_table_mut() {
        let mut a = toml::Table::new();
        a.insert("columns".into(), Value::Integer(cols as i64));
        a.insert("rows".into(), Value::Integer(rows as i64));
        t.insert(ARCHIVE_KEY.into(), Value::Table(a));
        t.insert("Selected Tab Index".into(), Value::Integer(0));
        t.insert("Hide After Opening".into(), Value::Boolean(false));
        t.insert("Is Hotkey Window".into(), Value::Boolean(false));
    }
    window
}

/// Whether a window table is an archive's (`PT:4329-4335`).
pub fn is_archive(window: &Value) -> bool {
    window.get(ARCHIVE_KEY).is_some_and(|a| a.as_table().is_some())
}

/// The session key that marks an archived pane in a saved state (13q final fix 4): session
/// restoration brings it back as an archived pane (its text, no shell), never as a live shell
/// running the profile the file embeds.
pub const ARCHIVED_KEY: &str = "Archived";

/// Marks the session table of an archived pane.
pub fn mark_archived(session: &mut Value) {
    if let Some(t) = session.as_table_mut() {
        t.insert(ARCHIVED_KEY.into(), Value::Boolean(true));
    }
}

/// Whether a session table is of an archived pane.
pub fn session_archived(session: &Value) -> bool {
    session.get(ARCHIVED_KEY).and_then(|v| v.as_bool()).unwrap_or(false)
}

/// Why an automatic archive may not go to `path` (the folder joined to the name, "~" and an empty
/// folder being the home folder): a network or device path is refused unless the person typed it
/// in "Archive Directory" themselves, as an automatic log's folder is (13q final fix 25).
pub fn network_refusal(folder: &str, path: &std::path::Path) -> Option<String> {
    let typed = crate::start_dir::network_folder(folder) && !folder.contains("\\(");
    (crate::session_log::is_network_log_path(path) && !typed).then(|| "the archive folder is a network path".to_string())
}

/// The recent list with `path` appended (`AMB:43-51`: duplicates kept), the oldest dropped
/// past ten.
pub fn add_recent(list: &[String], path: &str) -> Vec<String> {
    let mut out: Vec<String> = list.to_vec();
    out.push(path.to_string());
    let extra = out.len().saturating_sub(MAX_RECENTS);
    out.drain(..extra);
    out
}

/// A recent item's title: the file name without its extension (`AMB:53-63`). Both separators
/// count, whatever the platform reading the list.
pub fn recent_title(path: &str) -> String {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match name.rfind('.') {
        Some(at) if at > 0 => name[..at].to_string(),
        _ => name.to_string(),
    }
}

/// The notification when archiving failed (`PSW`: "Archiving to %@ failed: %@").
pub fn failed_text(path: &str, error: &str) -> String {
    format!("Archiving to {path} failed: {error}")
}

/// The largest archive file Restore Archive reads (the 10,000,000-line save cap fits well
/// inside it); a bigger file is refused before it is read.
pub const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
/// The most windows a restored file may open, and the most tabs in one window.
pub const MAX_WINDOWS: usize = 64;
pub const MAX_TABS: usize = 64;
/// The most lines of styled text one restored pane takes (the save side's cap).
pub const MAX_TEXT_LINES: usize = 10_000_000;

/// Why a file of this size is refused, if it is.
pub fn size_refusal(len: u64) -> Option<String> {
    (len > MAX_FILE_BYTES).then(|| format!("the file is larger than {} MB", MAX_FILE_BYTES / (1024 * 1024)))
}

/// Why a file with these windows is refused, if it is: too many windows or tabs (the panes of a
/// tab are bounded by `arrangement::tree_of`).
pub fn count_refusal(windows: &[Value]) -> Option<String> {
    if windows.len() > MAX_WINDOWS {
        return Some(format!("the file holds more than {MAX_WINDOWS} windows"));
    }
    let tabs = |w: &Value| w.get("Tabs").and_then(|t| t.as_array()).map_or(0, Vec::len);
    windows.iter().any(|w| tabs(w) > MAX_TABS).then(|| format!("a window in the file holds more than {MAX_TABS} tabs"))
}

/// Whether opening the windows of a file asks first: a downloaded file always, and an archive
/// with no mark that opens more than one window. A Load Arrangement keeps its own trust rules.
pub fn needs_question(marked: bool, windows: usize, archive: bool) -> bool {
    marked || (archive && windows > 1)
}

/// The path an automatic archive is written to: `path` when this run has not written it yet,
/// else "name (2).itermarchive", "(3)" and so on (`taken` answers from this run's own list, never
/// from the disk, so a network folder is not probed).
pub fn unique_path(path: &std::path::Path, taken: &dyn Fn(&std::path::Path) -> bool) -> std::path::PathBuf {
    if !taken(path) {
        return path.to_path_buf();
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (2..)
        .map(|n| path.with_file_name(format!("{stem} ({n}){ext}")))
        .find(|p| !taken(p))
        .unwrap_or_else(|| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arrangement::{self, Value};

    fn parts(y: u16, mo: u16, d: u16, h: u16, mi: u16, s: u16) -> Parts {
        Parts { year: y, month: mo, day: d, weekday: 0, hour: h, minute: mi, second: s }
    }

    /// `PSW:1676-1695`: the date short and the time medium (en-US), spaces "_", "/" "-", "," gone;
    /// Windows cannot hold ":" so it is "." (Decision). `PTV:1006-1022`: the dialog's default.
    #[test]
    fn archive_names_follow_iterm2() {
        assert_eq!(auto_file_name(&parts(2026, 9, 27, 15, 4, 5), "bash"), "9-27-26_3.04.05_PM - bash.itermarchive");
        assert_eq!(auto_file_name(&parts(2026, 1, 5, 9, 8, 7), "x"), "1-5-26_9.08.07_AM - x.itermarchive");
        assert_eq!(auto_file_name(&parts(2026, 1, 5, 0, 8, 7), "x"), "1-5-26_12.08.07_AM - x.itermarchive", "midnight is 12 AM");
        assert_eq!(auto_file_name(&parts(2026, 1, 5, 12, 8, 7), "x"), "1-5-26_12.08.07_PM - x.itermarchive", "noon is 12 PM");
        assert_eq!(auto_file_name(&parts(2026, 9, 27, 15, 4, 5), "a/b:c"), "9-27-26_3.04.05_PM - a_b_c.itermarchive");
        assert_eq!(save_name("bash"), "bash.itermarchive");
        assert_eq!(save_name(""), "Tandem Session.itermarchive");
        assert_eq!(save_name("   "), "Tandem Session.itermarchive");
        assert_eq!(failed_text("C:\\a.itermarchive", "denied"), "Archiving to C:\\a.itermarchive failed: denied");
        assert_eq!(EXTENSION, "itermarchive");
        assert_eq!((ARCHIVE_KEY, RECENTS_KEY, MAX_RECENTS, RESTORED_BANNER, MENU_ID), ("Archive", "NoSyncRecentArchives", 10, "Archive Restored", "archive:"));
    }

    /// A name from a program's title can hold anything: no separator, device name or runnable
    /// ending survives into the file name, whatever the session was called.
    #[test]
    fn archive_names_stay_in_their_folder() {
        for name in ["..\\..\\evil", "../../evil", "CON", "a\u{7}b", "x*?\"<>|y", &"n".repeat(500)] {
            let f = auto_file_name(&parts(2026, 9, 27, 15, 4, 5), name);
            let s = save_name(name);
            for f in [&f, &s] {
                assert!(!f.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) && !f.chars().any(char::is_control), "{f:?}");
                assert!(f.ends_with(".itermarchive") && f.encode_utf16().count() <= 200, "{f:?}");
            }
        }
        assert_eq!(save_name("CON"), "_CON.itermarchive", "a device name as the whole stem");
    }

    /// A saved state keeps an archived pane archived: the launch builds it with no shell.
    #[test]
    fn an_archived_pane_stays_archived_through_a_saved_state() {
        let mut session = Value::Table(Default::default());
        assert!(!session_archived(&session), "an ordinary session");
        mark_archived(&mut session);
        assert!(session_archived(&session), "the mark is on the session table");
    }

    /// A network home folder gets no archive, a share the person typed does.
    #[test]
    fn archive_folders_follow_the_log_network_rule() {
        let home = std::path::Path::new(r"\\fileserver\home\u");
        for folder in ["", "~", "~\\arch", "~/arch"] {
            let path = crate::session_log::join_log_path(folder, "a.itermarchive", home);
            assert!(network_refusal(folder, &path).is_some(), "{folder:?}");
        }
        let typed = r"\\srv\arch";
        assert_eq!(network_refusal(typed, &crate::session_log::join_log_path(typed, "a.itermarchive", home)), None, "a share the person typed is theirs");
        let local = std::path::Path::new(r"C:\Users\u");
        assert_eq!(network_refusal("~", &crate::session_log::join_log_path("~", "a.itermarchive", local)), None);
    }

    #[test]
    fn archive_window_follows_iterm2() {
        let mut w = toml::Table::new();
        w.insert("Tabs".into(), Value::Array(vec![]));
        let w = Value::Table(w);
        assert!(!is_archive(&w));
        let a = archive_window(w.clone(), 80, 24);
        let archive = a.get(ARCHIVE_KEY).expect("an Archive table");
        assert_eq!(archive.get("columns").and_then(|v| v.as_integer()), Some(80));
        assert_eq!(archive.get("rows").and_then(|v| v.as_integer()), Some(24));
        assert_eq!(a.get("Selected Tab Index").and_then(|v| v.as_integer()), Some(0));
        assert_eq!(a.get("Hide After Opening").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(a.get("Is Hotkey Window").and_then(|v| v.as_bool()), Some(false));
        assert!(is_archive(&a));
        let back = arrangement::from_plist(&arrangement::to_plist(&[a]).unwrap()).unwrap();
        assert!(is_archive(&back[0]), "the plist round trip keeps it");
        assert_eq!(back[0].get(ARCHIVE_KEY).and_then(|a| a.get("columns")).and_then(|v| v.as_integer()), Some(80));
        assert_eq!(back[0].get("Selected Tab Index").and_then(|v| v.as_integer()), Some(0));
    }

    /// `AMB:43-63`: appended, duplicates kept, the oldest dropped past ten, titled by the file
    /// name without its extension.
    #[test]
    fn recents_follow_iterm2() {
        assert_eq!(add_recent(&[], "a"), vec!["a".to_string()]);
        let ten: Vec<String> = (0..10).map(|i| format!("p{i}")).collect();
        let eleven = add_recent(&ten, "new");
        assert_eq!(eleven.len(), 10);
        assert_eq!(eleven.first().map(String::as_str), Some("p1"), "the oldest (first) goes");
        assert_eq!(eleven.last().map(String::as_str), Some("new"));
        assert_eq!(add_recent(&["a".to_string(), "b".to_string()], "a"), vec!["a".to_string(), "b".to_string(), "a".to_string()], "iTerm2 keeps duplicates");
        assert_eq!(recent_title(r"C:\x\My Log.itermarchive"), "My Log");
        assert_eq!(recent_title("/tmp/a.b.itermarchive"), "a.b");
        assert_eq!(recent_title("plain"), "plain");
    }

    #[test]
    fn restore_is_bounded() {
        assert!(size_refusal(MAX_FILE_BYTES).is_none());
        assert!(size_refusal(MAX_FILE_BYTES + 1).unwrap().contains("512"), "the limit is named");
        let window = |tabs: usize| {
            let mut w = toml::Table::new();
            w.insert("Tabs".into(), Value::Array((0..tabs).map(|_| Value::Table(Default::default())).collect()));
            Value::Table(w)
        };
        assert!(count_refusal(&vec![window(1); MAX_WINDOWS]).is_none());
        assert!(count_refusal(&vec![window(1); MAX_WINDOWS + 1]).unwrap().contains("64 windows"));
        assert!(count_refusal(&[window(MAX_TABS)]).is_none());
        assert!(count_refusal(&[window(MAX_TABS + 1)]).unwrap().contains("64 tabs"));
        // A crafted file of 65 windows survives a plist round trip and is refused after it.
        let crafted = arrangement::to_plist(&vec![window(1); 65]).unwrap();
        assert!(count_refusal(&arrangement::from_plist(&crafted).unwrap()).is_some());
    }

    #[test]
    fn many_windows_ask_first() {
        assert!(needs_question(true, 1, true), "a downloaded file");
        assert!(needs_question(true, 0, false));
        assert!(!needs_question(false, 1, true), "one window of a local file opens without a question");
        assert!(needs_question(false, 2, true), "an unmarked archive that opens several windows asks");
        assert!(!needs_question(false, 2, false), "a Load Arrangement of an unmarked file asks nothing");
    }

    #[test]
    fn automatic_archives_never_share_a_file() {
        use std::path::{Path, PathBuf};
        let a = Path::new(r"D:rc\9-27-26_3.04.05_PM - bash.itermarchive");
        assert_eq!(unique_path(a, &|_| false), a);
        let first = a.to_path_buf();
        let second = PathBuf::from(r"D:rc\9-27-26_3.04.05_PM - bash (2).itermarchive");
        assert_eq!(unique_path(a, &|p| p == first), second);
        assert_eq!(unique_path(a, &|p| p == first || p == second), PathBuf::from(r"D:rc\9-27-26_3.04.05_PM - bash (3).itermarchive"));
    }
}

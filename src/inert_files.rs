//! Files Tandem writes from a name someone else chose (a chat attachment, a tool's created file)
//! and then shows in Explorer: Explorer reads some types just to list them (an icon location, a
//! library's folders), so a model-named `x.url` could make Windows contact a UNC host with the
//! person's credentials without anything being opened. One rule for every such writer: refuse
//! those types (and any name with an alternate stream or device form), and mark every file
//! written with the Mark of the Web.

use std::path::Path;

/// Extensions the shell parses on listing, lower case, with no dot.
const PARSED_ON_LISTING: [&str; 12] = ["url", "lnk", "website", "library-ms", "searchconnector-ms", "scf", "settingcontent-ms", "appref-ms", "theme", "themepack", "desktopthemepack", "deskthemepack"];

/// Whether writing a file with this final name is refused: its type is read by the shell on
/// listing, it is a `desktop.ini` (no other `.ini` is refused), a device name, or it holds `:`
/// (an alternate stream or a device form). Trailing dots and spaces are ignored as Windows
/// ignores them.
pub fn refused_on_listing(name: &str) -> bool {
    if name.contains(':') {
        return true;
    }
    let name = name.trim_end_matches([' ', '.']).to_ascii_lowercase();
    name == "desktop.ini" || is_device_name(&name) || Path::new(&name).extension().and_then(|e| e.to_str()).is_some_and(|e| PARSED_ON_LISTING.contains(&e))
}

/// `refused_on_listing` for a whole path a tool was given: its final component, a device name as
/// any folder (Windows cannot make one), and a `:` anywhere after an optional drive letter (an
/// alternate stream of a folder is as bad).
pub fn refused_path(path: &str) -> bool {
    let b = path.as_bytes();
    let rest = if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' { &path[2..] } else { path };
    let mut parts = path.split(['\\', '/']).rev();
    let last = parts.next().unwrap_or(path);
    rest.contains(':') || refused_on_listing(last) || parts.any(is_device_name)
}

/// Windows device names (`CON`, `PRN`, `AUX`, `NUL`, `CONIN$`, `CONOUT$`, `COM1` to `COM9`, `LPT1`
/// to `LPT9`), which no file may be called, with or without an extension, in any case.
pub fn is_device_name(name: &str) -> bool {
    // The one device-name rule (scripts::is_device_name and zip_check::device_name call it): the part
    // before the first dot, trailing spaces and colons ignored, as Windows reads it.
    let stem = name.split('.').next().unwrap_or("").trim_end_matches([' ', ':']).to_ascii_uppercase();
    // COM and LPT take the superscript digits too (Microsoft's "Naming Files").
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") || (stem.chars().count() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && matches!(stem.chars().nth(3), Some('1'..='9' | '\u{b9}' | '\u{b2}' | '\u{b3}')))
}

/// The mark Windows puts on a download (`ZoneId=3`, the Internet zone).
pub const ZONE_MARK: &str = "[ZoneTransfer]\r\nZoneId=3\r\n";

/// A character no shown name keeps: control, bidirectional formatting, line and paragraph
/// separator and zero-width characters (one place, so every label drops the same set).
pub fn is_hidden(c: char) -> bool {
    let bidi = matches!(c, '\u{61c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}');
    let unseen = matches!(c, '\u{2028}' | '\u{2029}' | '\u{180e}' | '\u{200b}'..='\u{200d}' | '\u{2060}'..='\u{2064}' | '\u{feff}');
    c.is_control() || bidi || unseen
}

/// THE sanitizer for text a program supplied and Tandem shows (badge text, alert text, host
/// names, and any later display of program text): `is_hidden` characters removed (a newline kept
/// when `keep_newline`), then at most `max` characters. Do not write another removal set.
pub fn display_text(s: &str, max: usize, keep_newline: bool) -> String {
    s.chars().filter(|&c| (keep_newline && c == '\n') || !is_hidden(c)).take(max).collect()
}

/// `name` without the characters no shown name keeps, and with no length cut (a command line
/// stays whole; `shown_name` also cuts).
pub fn clean_name(name: &str) -> String {
    name.chars().filter(|&c| !is_hidden(c)).collect()
}

/// A name the remote sent, as the person reads it (a notification, a question, a panel row):
/// control, bidirectional formatting, line and paragraph separator and zero-width characters
/// dropped, and at most `SHOWN_NAME` characters, the rest an ellipsis.
pub fn shown_name(name: &str) -> String {
    let clean: Vec<char> = clean_name(name).chars().collect();
    if clean.len() <= SHOWN_NAME {
        return clean.into_iter().collect();
    }
    clean[..SHOWN_NAME].iter().chain(['\u{2026}'].iter()).collect()
}

/// The longest name shown.
pub const SHOWN_NAME: usize = 200;

/// The Zone.Identifier text for a file that came from `host_url` (`HostUrl=` is left out when
/// there is none). A control character in the URL is dropped, so no URL can add a line: the
/// text always has its own lines and no others.
pub fn zone_text(host_url: Option<&str>) -> String {
    match host_url {
        Some(url) => format!("[ZoneTransfer]\r\nZoneId=3\r\nHostUrl={}\r\n", url.chars().filter(|c| !c.is_control()).collect::<String>()),
        None => ZONE_MARK.to_string(),
    }
}

/// The ZoneId of a Zone.Identifier stream's text (the Mark of the Web): the first `ZoneId=` line
/// of the `[ZoneTransfer]` section, None when there is none or it is not a number.
pub fn zone_of(text: &str) -> Option<u32> {
    let mut in_section = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = line.eq_ignore_ascii_case("[ZoneTransfer]");
        } else if in_section {
            if let Some(v) = line.strip_prefix("ZoneId=") {
                return v.trim().parse().ok();
            }
        }
    }
    None
}

/// Marks the existing file `path` as from the internet, and fails when it cannot: a missing
/// file (writing a stream would create it), or a drive that keeps no streams (FAT). For a
/// download that must not land unmarked; `write_marked` stays lenient for inline files (12d).
pub fn mark_strict(path: &Path, host_url: Option<&str>) -> std::io::Result<()> {
    if !crate::localfs::lstat(&crate::localfs::judge_io(path)?)?.is_file {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a file"));
    }
    crate::localfs::write(&zone_stream(path)?, zone_text(host_url))
}

/// `path`'s Zone.Identifier stream, judged.
fn zone_stream(path: &Path) -> std::io::Result<crate::localfs::LocalPath> {
    let mut stream = path.as_os_str().to_os_string();
    stream.push(":Zone.Identifier");
    crate::localfs::judge_io(std::path::PathBuf::from(stream))
}

/// Writes `bytes` to `path`, then the Zone.Identifier stream, so SmartScreen or Protected View
/// meets a file opened after it was revealed. A drive without streams (FAT) keeps the file
/// unmarked.
pub fn write_marked(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    crate::localfs::write(&crate::localfs::judge_io(path)?, bytes)?;
    mark(path);
    Ok(())
}

/// The Zone.Identifier stream alone, for a writer that wrote the file through its own handle
/// (a failure is ignored: a drive without streams keeps the file unmarked).
pub fn mark(path: &Path) {
    let _ = zone_stream(path).and_then(|s| crate::localfs::write(&s, ZONE_MARK));
}

/// `write_marked` for a name someone else chose that is not yet checked: refused with an error
/// when `refused_path` says so, so a caller with no name of its own cannot skip the rule.
pub fn write_checked(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if refused_path(&path.to_string_lossy()) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "files of this type are not allowed"));
    }
    write_marked(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_types_are_refused() {
        for bad in [
            "x.url", "X.URL", "a.lnk", "a.website", "a.library-ms", "a.searchConnector-ms", "a.scf", "a.settingcontent-ms", "a.appref-ms", "a.theme", "a.themepack", "a.desktopthemepack", "a.deskthemepack", "desktop.ini",
            "x.url.", "x.url ", "a.txt:evil", "a.txt::$DATA", "C:x", "Thumbs.db:x",
        ] {
            assert!(refused_on_listing(bad), "{bad:?}");
        }
        assert!(refused_path(r"D:\a\x.url") && refused_path("sub/x.lnk") && refused_path("a.txt:s") && refused_path(r"C:\d:s\x.txt"));
        assert!(!refused_path(r"D:\a\x.txt") && !refused_path("x.txt"));
        for fine in ["a.txt", "notes.md", "url.txt", "lnk", "a.pdf", "my.theme.txt", "inifile.txt", "a.json"] {
            assert!(!refused_on_listing(fine), "{fine:?}");
        }
    }

    #[test]
    fn device_names_are_refused_with_any_extension() {
        for bad in ["con", "CON", "con.txt", "Nul.", "aux.tar.gz", "PRN ", "COM1", "com9.log", "lpt1", "LPT9.x", "conin$", "CONOUT$", r"D:\a\NUL", "sub/con.txt"] {
            assert!(refused_path(bad), "{bad:?}");
        }
        for fine in ["console.txt", "com10", "COM0", "lpt", "a.con", "nulls.txt", "prn_x", "auxiliary"] {
            assert!(!refused_on_listing(fine), "{fine:?}");
            assert!(!is_device_name(fine), "{fine:?}");
        }
        assert!(is_device_name("con.txt") && is_device_name("LPT3") && is_device_name("NUL:"));
        // The superscript digits are devices too, and a device name as a folder is refused.
        for bad in ["COM\u{b9}", "com\u{b2}.txt", "COM\u{b3}", "LPT\u{b9}", "lpt\u{b2}", "LPT\u{b3}.log"] {
            assert!(is_device_name(bad) && refused_on_listing(bad), "{bad:?}");
        }
        for bad in [r"sub\con\x.txt", "aux/x.txt", r"D:\a\COM1\b.txt", r"C:\Users\Nul.d\a.txt", "C:\\x\\LPT\u{b9}\\a"] {
            assert!(refused_path(bad), "{bad:?}");
        }
        for fine in [r"C:\console\x.txt", r"C:\Users\nulls\a.txt", r"C:\a\com10\b.txt", "a.txt"] {
            assert!(!refused_path(fine), "{fine:?}");
        }
        assert!(!refused_on_listing("a.ini") && refused_on_listing("desktop.ini") && refused_on_listing("Desktop.INI."), "only desktop.ini among the ini files");
    }

    #[cfg(windows)]
    #[test]
    fn a_written_file_carries_the_mark() {
        let dir = std::env::temp_dir().join(format!("tandem-inert-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.txt");
        write_marked(&p, b"hi").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"hi");
        let zone = std::fs::read_to_string(format!("{}:Zone.Identifier", p.display())).unwrap();
        assert_eq!(zone, ZONE_MARK);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn zone_ids_are_read_from_the_stream() {
        assert_eq!(zone_of(ZONE_MARK), Some(3));
        assert_eq!(zone_of(&zone_text(Some("https://x/y"))), Some(3));
        assert_eq!(zone_of("[ZoneTransfer]\r\nZoneId=4\r\n"), Some(4));
        assert_eq!(zone_of("[ZoneTransfer]\r\nZoneId=2"), Some(2));
        assert_eq!(zone_of("[Other]\r\nZoneId=3\r\n"), None);
        assert_eq!(zone_of("ZoneId=3"), None);
        assert_eq!(zone_of("[ZoneTransfer]\r\nZoneId=x\r\n"), None);
        assert_eq!(zone_of(""), None);
    }

    #[test]
    fn zone_text_has_only_its_own_lines() {
        assert_eq!(zone_text(None), ZONE_MARK);
        assert_eq!(zone_text(Some("ssh://h/x")), "[ZoneTransfer]\r\nZoneId=3\r\nHostUrl=ssh://h/x\r\n");
        // A URL that carries line breaks cannot add a ZoneId line.
        let t = zone_text(Some("ssh://h/x\r\nZoneId=0"));
        assert_eq!(t.lines().count(), 3, "{t:?}");
        assert!(t.ends_with("\r\n") && !t.replace("\r\n", "").contains(['\r', '\n']));
        assert_eq!(t.lines().filter(|l| l.starts_with("ZoneId=")).count(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn a_strict_mark_fails_when_it_cannot_be_written() {
        let dir = std::env::temp_dir().join(format!("tandem-strict-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.txt");
        std::fs::write(&p, "x").unwrap();
        mark_strict(&p, Some("ssh://h/a.txt")).unwrap();
        assert_eq!(std::fs::read_to_string(format!("{}:Zone.Identifier", p.display())).unwrap(), zone_text(Some("ssh://h/a.txt")));
        // A missing file is an error, and the mark must not create it.
        let gone = dir.join("gone.txt");
        assert!(mark_strict(&gone, None).is_err());
        assert!(!gone.exists(), "writing the stream would have created the file");
        // A folder takes no mark here, nor does a file in a folder that is not there.
        assert!(mark_strict(&dir, None).is_err());
        assert!(mark_strict(&dir.join("no").join("b.txt"), None).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

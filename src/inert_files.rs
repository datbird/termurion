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

/// `refused_on_listing` for a whole path a tool was given: its final component, and a `:` anywhere
/// after an optional drive letter (an alternate stream of a folder is as bad).
pub fn refused_path(path: &str) -> bool {
    let b = path.as_bytes();
    let rest = if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' { &path[2..] } else { path };
    rest.contains(':') || refused_on_listing(path.rsplit(['\\', '/']).next().unwrap_or(path))
}

/// Windows device names (`CON`, `PRN`, `AUX`, `NUL`, `CONIN$`, `CONOUT$`, `COM1` to `COM9`, `LPT1`
/// to `LPT9`), which no file may be called, with or without an extension, in any case.
pub fn is_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").trim_end_matches([' ', '.']).to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && matches!(stem.as_bytes()[3], b'1'..=b'9'))
}

/// The mark Windows puts on a download (`ZoneId=3`, the Internet zone).
pub const ZONE_MARK: &str = "[ZoneTransfer]\r\nZoneId=3\r\n";

/// Writes `bytes` to `path`, then the Zone.Identifier stream, so SmartScreen or Protected View
/// meets a file opened after it was revealed. A drive without streams (FAT) keeps the file
/// unmarked.
pub fn write_marked(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)?;
    let mut stream = path.as_os_str().to_os_string();
    stream.push(":Zone.Identifier");
    let _ = std::fs::write(stream, ZONE_MARK);
    Ok(())
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
        assert!(is_device_name("con.txt") && is_device_name("LPT3"));
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
}

//! Files Tandem writes from a name someone else chose (a chat attachment, a tool's created file)
//! and then shows in Explorer: Explorer reads some types just to list them (an icon location, a
//! library's folders), so a model-named `x.url` could make Windows contact a UNC host with the
//! person's credentials without anything being opened. One rule for every such writer: refuse
//! those types (and any name with an alternate stream or device form), and mark every file
//! written with the Mark of the Web.

use std::path::Path;

/// THE list of dangerous file types (lower case, no dot), one place for every caller: the Windows
/// Attachment Manager's high-risk list, the types the shell runs, installs or follows on open, and
/// the mountable images. `true` marks a type the shell parses just to list it (an icon location, a
/// library's folders): those are refused even where the others may be written (a chat attachment,
/// a tool's file, an archive entry), since no click is needed. `semantic::runnable` reads it (a link
/// to any of them is revealed, never opened) and `write_checked` refuses all of them.
const DANGEROUS: &[(&str, bool)] = &[
    ("url", true), ("lnk", true), ("website", true), ("library-ms", true), ("searchconnector-ms", true), ("scf", true),
    ("settingcontent-ms", true), ("appref-ms", true), ("theme", true), ("themepack", true), ("desktopthemepack", true),
    ("deskthemepack", true),
    ("exe", false), ("com", false), ("scr", false), ("pif", false), ("bat", false), ("cmd", false), ("btm", false),
    ("ps1", false), ("psm1", false), ("psd1", false), ("ps1xml", false), ("ps2", false), ("ps2xml", false), ("psc1", false),
    ("psc2", false), ("msh", false), ("msh1", false), ("msh2", false), ("mshxml", false), ("msh1xml", false), ("msh2xml", false),
    ("vbs", false), ("vbe", false), ("vb", false), ("js", false), ("jse", false), ("wsf", false), ("wsh", false), ("ws", false),
    ("wsc", false), ("sct", false), ("hta", false), ("msi", false), ("msp", false), ("mst", false), ("msu", false),
    ("appx", false), ("appxbundle", false), ("msix", false), ("msixbundle", false), ("appinstaller", false), ("inf", false),
    ("ins", false), ("isp", false), ("reg", false), ("cpl", false), ("msc", false), ("jar", false), ("jnlp", false),
    ("application", false), ("xbap", false), ("gadget", false), ("search-ms", false), ("diagcab", false), ("chm", false),
    ("hlp", false), ("xll", false), ("xnk", false), ("mcf", false), ("shb", false), ("shs", false), ("py", false), ("pyw", false),
    ("pyc", false), ("pyz", false), ("pyzw", false), ("ade", false), ("adp", false), ("app", false), ("asp", false), ("bas", false),
    ("cer", false), ("crt", false), ("csh", false), ("ksh", false), ("fxp", false), ("its", false), ("mad", false), ("maf", false),
    ("mag", false), ("mam", false), ("maq", false), ("mar", false), ("mas", false), ("mat", false), ("mau", false), ("mav", false),
    ("maw", false), ("mda", false), ("mdb", false), ("mde", false), ("mdt", false), ("mdw", false), ("mdz", false), ("ops", false),
    ("pcd", false), ("prf", false), ("prg", false), ("pst", false), ("tmp", false), ("vsmacros", false), ("vss", false),
    ("vst", false), ("vsw", false), ("iso", false), ("img", false), ("vhd", false), ("vhdx", false),
];

/// A path's final name as Windows opens it: THE normalizer `extension`, `high_risk` (the write
/// door), `semantic::runnable` and every open-or-reveal decision share (findings r2 N1; the same
/// forms `settings_export::aliased` refuses in a name). A `\\?\` or `\\.\` prefix and a leading
/// drive (`C:`) are dropped; the final component is cut at its first `:` (an NTFS stream:
/// `a.exe::$DATA` and `a.exe:` open `a.exe`); trailing dots and spaces go, as Windows drops them;
/// the case is folded one character for one, as NTFS folds it. `stream` is true when any `:`
/// remains past the drive, in any component: such a path is dangerous whatever its type (fail
/// closed).
pub struct Opened {
    pub name: String,
    pub stream: bool,
}

/// `path`'s final name as Windows opens it, and whether it names a stream (see `Opened`).
pub fn opened_name(path: &str) -> Opened {
    let mut rest = path;
    for prefix in [r"\\?\", r"\\.\", "//?/", "//./"] {
        if let Some(r) = rest.strip_prefix(prefix) {
            rest = r;
            break;
        }
    }
    let b = rest.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        rest = &rest[2..];
    }
    let last = rest.rsplit(['\\', '/']).next().unwrap_or(rest);
    let last = last.split(':').next().unwrap_or(last).trim_end_matches([' ', '.']);
    let name = last
        .chars()
        .map(|c| {
            let mut up = c.to_uppercase();
            match (up.next(), up.next()) {
                (Some(u), None) => u,
                _ => c,
            }
        })
        .collect::<String>()
        .to_ascii_lowercase();
    Opened { name, stream: rest.contains(':') }
}

/// The type of a name as Windows reads it (`PathFindExtension`) of the name `opened_name` gives:
/// the text after its last dot, a leading dot included (`.scf` is an SCF file), folded to lower
/// case. None when there is no dot.
pub fn extension(name: &str) -> Option<String> {
    opened_name(name).name.rsplit_once('.').map(|(_, e)| e.to_string())
}

/// Whether the type `ext` (lower case, no dot) is on the one dangerous list.
pub fn dangerous_extension(ext: &str) -> bool {
    DANGEROUS.iter().any(|(e, _)| *e == ext)
}

/// Whether this name's type is on the one dangerous list (run, installed, followed or parsed by
/// the shell, or on the machine's PATHEXT), or it names a stream (`opened_name`: fail closed).
pub fn high_risk(name: &str) -> bool {
    high_risk_with(name, std::env::var("PATHEXT").ok().as_deref())
}

/// `high_risk` with the PATHEXT text given: a type the machine runs by name (`.RB` added by a
/// Ruby install) is high risk too, for the writer as for `semantic::runnable` (findings r2 M5).
pub fn high_risk_with(name: &str, pathext: Option<&str>) -> bool {
    let on_pathext = |e: &str| pathext.is_some_and(|p| p.split(';').map(|x| x.trim().trim_start_matches('.')).any(|x| !x.is_empty() && x.eq_ignore_ascii_case(e)));
    opened_name(name).stream || extension(name).is_some_and(|e| dangerous_extension(&e) || on_pathext(&e))
}

/// What `neutralized` appends: a type with no shell action whose first three characters are
/// inert too, so the 8.3 short name (`SETUP~1.DOW`) keeps no type either (findings r2 M1; the
/// form browsers use for a download in progress).
pub const NEUTRAL_SUFFIX: &str = ".download";

/// The name a file someone else named is kept under: a high-risk type gets `NEUTRAL_SUFFIX`
/// (`setup.exe` is kept as `setup.exe.download`, bytes and all, and never runs from a double
/// click); any other name is unchanged. Trailing dots and spaces are dropped first, as Windows
/// drops them.
pub fn neutralized(name: &str) -> String {
    neutralized_with(name, std::env::var("PATHEXT").ok().as_deref())
}

/// `neutralized` with the PATHEXT text given.
pub fn neutralized_with(name: &str, pathext: Option<&str>) -> String {
    if high_risk_with(name, pathext) { format!("{}{NEUTRAL_SUFFIX}", name.trim_end_matches([' ', '.'])) } else { name.to_string() }
}

/// Whether writing a file with this final name is refused: its type is read by the shell on
/// listing, it is a `desktop.ini` (no other `.ini` is refused), a device name, or it holds `:`
/// (an alternate stream or a device form). Trailing dots and spaces are ignored as Windows
/// ignores them, and the type is read as Windows reads it (`extension`).
pub fn refused_on_listing(name: &str) -> bool {
    if name.contains(':') {
        return true;
    }
    let name = name.trim_end_matches([' ', '.']).to_ascii_lowercase();
    name == "desktop.ini" || is_device_name(&name) || extension(&name).is_some_and(|e| DANGEROUS.iter().any(|(d, listed)| *listed && *d == e))
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

/// Windows device names (`CON`, `PRN`, `AUX`, `NUL`, `CONIN$`, `CONOUT$`, `COM0` to `COM9`, `LPT0`
/// to `LPT9`, and COM/LPT with a superscript 1, 2 or 3), which no file may be called, with or without an extension, in any case.
pub fn is_device_name(name: &str) -> bool {
    // The one device-name rule (scripts::is_device_name and zip_check::device_name call it): the part
    // before the first dot, trailing spaces and colons ignored, as Windows reads it.
    let stem = name.split('.').next().unwrap_or("").trim_end_matches([' ', ':']).to_ascii_uppercase();
    // COM and LPT take 0 and the superscript digits too (Microsoft's "Naming Files").
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") || (stem.chars().count() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && matches!(stem.chars().nth(3), Some('0'..='9' | '\u{b9}' | '\u{b2}' | '\u{b3}')))
}

/// The mark Windows puts on a download (`ZoneId=3`, the Internet zone).
pub const ZONE_MARK: &str = "[ZoneTransfer]\r\nZoneId=3\r\n";

/// A character no shown name keeps: control, bidirectional formatting, line and paragraph
/// separator and zero-width characters (one place, so every label drops the same set). The 13w x
/// 13x merge folded the browser's set in: the soft hyphen, the prepended concatenation marks
/// (U+0600-0605, U+06DD, U+070F), every U+2060-206F format character, the interlinear
/// annotation marks, the musical formatting controls and the tag characters.
pub fn is_hidden(c: char) -> bool {
    let bidi = matches!(c, '\u{61c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}');
    let unseen = matches!(c, '\u{2028}' | '\u{2029}' | '\u{180e}' | '\u{200b}'..='\u{200d}' | '\u{2060}'..='\u{206f}' | '\u{feff}');
    let format = matches!(c, '\u{ad}' | '\u{600}'..='\u{605}' | '\u{6dd}' | '\u{70f}' | '\u{fff9}'..='\u{fffb}' | '\u{1d173}'..='\u{1d17a}' | '\u{e0000}'..='\u{e007f}');
    // The whole Unicode format class, from the crate's table, never a hand list (the list above
    // stays for the characters that are not Cf: line and paragraph separators, the interlinear marks).
    let cf = unicode_general_category::get_general_category(c) == unicode_general_category::GeneralCategory::Format;
    c.is_control() || bidi || unseen || format || cf
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

/// The door's rule for a whole path (`write_checked`): `refused_path`, or a final name of a
/// high-risk type. A caller that asks before writing (a question, a save job) asks this.
pub fn refused_write(path: &str) -> bool {
    refused_path(path) || high_risk(path)
}

/// `write_marked` for a name someone else chose that is not yet checked: refused with an error
/// when `refused_path` says so or the final name is `high_risk`, so a caller with no name of its
/// own cannot skip the rule (a caller that keeps such a download names it `neutralized` first).
pub fn write_checked(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if refused_write(&path.to_string_lossy()) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "files of this type are not allowed"));
    }
    write_marked(path, bytes)
}

/// `write_checked` that never replaces a file: an existing name (a file, a folder, a link) is an
/// `AlreadyExists` error and nothing is written (`CREATE_NEW` does not follow a link). The Save Page
/// As job writes only this way (Task 9 review I1). The file is marked as from the internet.
pub fn write_new_checked(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if refused_write(&path.to_string_lossy()) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "files of this type are not allowed"));
    }
    let judged = crate::localfs::judge_io(path)?;
    let mut f = crate::localfs::open_custom(&judged, |o| {
        o.write(true).create_new(true);
    })?;
    f.write_all(bytes)?;
    drop(f);
    mark(path);
    Ok(())
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

    /// Windows reads a type from the last dot, a leading dot included (`PathFindExtension`), so a
    /// name that is only a dot and an extension is that type (review I2).
    #[test]
    fn leading_dot_names_keep_their_type() {
        for bad in [".scf", ".URL", ".lnk", ".library-ms", ".settingcontent-ms", ".url.", ".lnk "] {
            assert!(refused_on_listing(bad), "{bad:?}");
        }
        assert!(refused_path(r"D:\a\.url") && refused_path("sub/.scf"));
        for fine in [".bashrc", ".gitignore", ".txt"] {
            assert!(!refused_on_listing(fine), "{fine:?}");
        }
    }

    /// The door refuses every high-risk type, not only the listing ones (review I2, controller
    /// ruling): a remote program's `x.exe` is never written as `x.exe` by any caller.
    #[test]
    fn the_door_refuses_high_risk_types() {
        let dir = std::env::temp_dir().join(format!("tandem-door-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for bad in [
            "x.exe", ".exe", "a.pdf.exe", "x.EXE.", "x.ps1 ", "x.com", "x.scr", "x.pif", "x.bat", "x.cmd", "x.psm1", "x.psd1", "x.ps1xml", "x.vbs", "x.vbe", "x.js", "x.jse", "x.wsf",
            "x.wsh", "x.wsc", "x.hta", "x.msi", "x.msp", "x.mst", "x.inf", "x.ins", "x.isp", "x.reg", "x.cpl", "x.msc", "x.jar", "x.application", "x.gadget", "x.search-ms", "x.diagcab",
            "x.chm", "x.hlp", "x.xll", "x.url", ".scf",
        ] {
            let r = write_checked(&dir.join(bad), b"MZ");
            assert!(r.is_err(), "{bad:?} was written");
        }
        for fine in ["a.txt", "a.log", "notes.md", "export.zip", "a.json", "x.exe.download", "a.pdf", ".bashrc"] {
            write_checked(&dir.join(fine), b"ok").unwrap_or_else(|e| panic!("{fine:?}: {e}"));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The door reads PATHEXT as `semantic::runnable` does (findings r2 M5): a type the machine
    /// runs by name is high risk for the writer too.
    #[test]
    fn the_door_reads_pathext() {
        let pathext = Some(".COM;.EXE;.BAT;.RB;.Pl ;tcl");
        for bad in ["x.rb", "X.RB", "a.pl", "b.tcl", "x.rb. ", r"C:\d\x.Rb"] {
            assert!(high_risk_with(bad, pathext), "{bad:?}");
            assert_eq!(neutralized_with(bad, pathext), format!("{}.download", bad.trim_end_matches(['.', ' '])), "{bad:?}");
        }
        for fine in ["x.rb", "a.pl"] {
            assert!(!high_risk_with(fine, None) && !high_risk_with(fine, Some("")), "{fine:?}");
        }
        assert!(!high_risk_with("x.txt", pathext) && !high_risk_with("x.rbx", pathext));
        assert!(high_risk_with("x.exe", None), "the list holds without PATHEXT");
    }

    /// A neutralized name ends in `.download` (findings r2 M1): its 8.3 short name is `X~1.DOW`,
    /// never `X~1.EXE` as `x.exe_` gave, and no dangerous type starts with `dow`.
    #[test]
    fn neutral_names_stay_neutral_as_short_names() {
        assert_eq!(neutralized("setup.exe"), "setup.exe.download");
        assert_eq!(neutralized("x.LNK. ."), "x.LNK.download");
        assert_eq!(neutralized("notes.txt"), "notes.txt");
        for (e, _) in DANGEROUS {
            let n = neutralized(&format!("a.{e}"));
            assert_eq!(n, format!("a.{e}.download"));
            assert!(!high_risk(&n) && !refused_on_listing(&n), "{n}");
            let short = &extension(&n).unwrap()[..3];
            assert!(!DANGEROUS.iter().any(|(d, _)| d.get(..3) == Some(short) || *d == short), "{n}: short {short}");
        }
    }

    #[test]
    fn device_names_are_refused_with_any_extension() {
        for bad in ["con", "CON", "con.txt", "Nul.", "aux.tar.gz", "PRN ", "COM1", "com9.log", "lpt1", "LPT9.x", "conin$", "CONOUT$", r"D:\a\NUL", "sub/con.txt"] {
            assert!(refused_path(bad), "{bad:?}");
        }
        for fine in ["console.txt", "com10", "lpt", "a.con", "nulls.txt", "prn_x", "auxiliary", "COM\u{b9}0", "LPT00"] {
            assert!(!refused_on_listing(fine), "{fine:?}");
            assert!(!is_device_name(fine), "{fine:?}");
        }
        assert!(is_device_name("con.txt") && is_device_name("LPT3") && is_device_name("NUL:"));
        // The superscript digits are devices too, and a device name as a folder is refused.
        // COM0 and LPT0 too (Microsoft's "Naming Files"; Task 8 review I3: the one device rule).
        for bad in ["COM0", "lpt0.txt", "Com0 .log", "COM\u{b9}", "com\u{b2}.txt", "COM\u{b3}", "LPT\u{b9}", "lpt\u{b2}", "LPT\u{b3}.log"] {
            assert!(is_device_name(bad) && refused_on_listing(bad), "{bad:?}");
            assert!(crate::settings_export::aliased(bad) && crate::page_saver::sanitize(bad).starts_with('_'), "{bad:?}");
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

    /// Every Unicode format character (General_Category Cf) is hidden, not a hand list of them: the
    /// ones a list misses (Arabic marks above, Kaithi, Egyptian hieroglyph controls, shorthand
    /// controls) can disguise a command as well as a bidi override.
    #[test]
    fn every_format_character_is_hidden() {
        for c in ['\u{890}', '\u{891}', '\u{8e2}', '\u{110bd}', '\u{110cd}', '\u{13430}', '\u{1343f}', '\u{1bca0}', '\u{1bca3}', '\u{61c}', '\u{180e}', '\u{202e}', '\u{2066}'] {
            assert!(is_hidden(c), "{:#x}", c as u32);
            assert_eq!(display_text(&format!("a{c}b"), 10, false), "ab", "{:#x}", c as u32);
        }
        // Cf lives in planes 0, 1 and 14 only.
        for c in (0..=0x1ffffu32).chain(0xe0000..=0xe0fff).filter_map(char::from_u32) {
            if unicode_general_category::get_general_category(c) == unicode_general_category::GeneralCategory::Format {
                assert!(is_hidden(c), "Cf {:#x}", c as u32);
            }
        }
        assert!(!is_hidden('a') && !is_hidden(' ') && !is_hidden('\u{e9}'));
    }

    /// Create-new: an existing file is kept as it was, an existing folder fails, the door's refusals
    /// hold, and a written file carries the Mark of the Web (the real stream, on Windows).
    #[test]
    fn write_new_checked_never_overwrites_and_marks() {
        let dir = std::env::temp_dir().join(format!("tandem-new-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.txt");
        write_new_checked(&f, b"one").unwrap();
        let e = write_new_checked(&f, b"two").unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&f).unwrap(), b"one");
        std::fs::create_dir(dir.join("d.png")).unwrap();
        assert!(write_new_checked(&dir.join("d.png"), b"x").is_err());
        for bad in ["x.url", "desktop.ini", "a.scf", "x.exe", "a.txt:s"] {
            assert!(write_new_checked(&dir.join(bad), b"x").is_err(), "{bad}");
            assert!(!dir.join(bad).exists() || bad == "a.txt:s", "{bad}");
        }
        #[cfg(windows)]
        {
            let stream = format!("{}:Zone.Identifier", f.display());
            let text = crate::localfs::read_to_string(&crate::localfs::judge_io(&stream).unwrap()).expect("a Zone.Identifier stream");
            assert_eq!(zone_of(&text), Some(3), "{text:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

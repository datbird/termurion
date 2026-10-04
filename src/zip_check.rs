//! A zip archive's safety check before anything extracts it: the file must be a zip (its
//! first bytes a local header or an empty archive's end record), and every entry of its
//! central directory must name a plain file or folder inside the folder it is extracted
//! into. Pure: it reads bytes only, so the Scripts import (milestone 11d) and later SSH file
//! transfers can share it.

/// Why an archive was refused.
pub fn refused(reason: &str) -> String {
    format!("The archive is not safe to extract: {reason}.")
}

const LOCAL: u32 = 0x0403_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const END: u32 = 0x0605_4b50;
const END64: u32 = 0x0606_4b50;
const LOCATOR64: u32 = 0x0706_4b50;
/// The ZIP64 extended information extra field.
const EXTRA_ZIP64: u16 = 0x0001;
/// PKWARE's Unix extra field, which can carry a link's target (`APPNOTE` 4.5.7).
const EXTRA_UNIX: u16 = 0x000d;
/// Hosts whose external attributes hold a Unix mode in their high half.
const HOST_UNIX: u8 = 3;
const HOST_OSX: u8 = 19;
const S_IFMT: u32 = 0o170_000;
const S_IFREG: u32 = 0o100_000;
const S_IFDIR: u32 = 0o040_000;
/// `FILE_ATTRIBUTE_REPARSE_POINT` in a DOS host's attributes.
const DOS_REPARSE: u32 = 0x400;

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

fn corrupt() -> String {
    refused("its directory is damaged")
}

/// Whether one part of a path is a Windows device name (CON, PRN, AUX, NUL, CONIN$,
/// CONOUT$, COM0-9 and LPT0-9 with the superscript digits too), with any extension: the
/// part before its first dot, trailing spaces ignored, as Windows reads it.
pub fn device_name(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or("").trim_end_matches(' ').to_uppercase();
    if ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&stem.as_str()) {
        return true;
    }
    let digits = |c: char| c.is_ascii_digit() || matches!(c, '\u{b9}' | '\u{b2}' | '\u{b3}');
    for prefix in ["COM", "LPT"] {
        if let Some(rest) = stem.strip_prefix(prefix) {
            let mut chars = rest.chars();
            if chars.next().is_some_and(digits) && chars.next().is_none() {
                return true;
            }
        }
    }
    false
}

/// Why one part of a path is refused as a file or folder name on Windows: empty, "." or
/// "..", a device name, a ":" (a drive or an alternate data stream), a control character,
/// or a trailing dot or space (Windows drops them, so the name would reach another item).
pub fn bad_part(part: &str) -> Option<&'static str> {
    if part.is_empty() {
        return Some("an empty name");
    }
    if part == "." || part == ".." {
        return Some("a \u{201c}.\u{201d} or \u{201c}..\u{201d} name");
    }
    if part.contains(':') {
        return Some("a \u{201c}:\u{201d} in a name");
    }
    if part.chars().any(|c| c.is_control()) {
        return Some("a control character in a name");
    }
    if part.ends_with('.') || part.ends_with(' ') {
        return Some("a name ending in a dot or a space");
    }
    if device_name(part) {
        return Some("a device name");
    }
    None
}

/// Why an entry's name is refused: absolute, with a backslash, or any part `bad_part`
/// refuses. A folder's name ends in "/".
fn bad_name(name: &str) -> Option<&'static str> {
    if name.starts_with('/') {
        return Some("an absolute path");
    }
    if name.contains('\\') {
        return Some("a backslash in a name");
    }
    let trimmed = name.strip_suffix('/').unwrap_or(name);
    trimmed.split('/').find_map(bad_part)
}

/// An extra field block's fields, or None when it is damaged.
fn extra_fields(extra: &[u8]) -> Option<Vec<(u16, &[u8])>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < extra.len() {
        let id = u16_at(extra, at)?;
        let size = usize::from(u16_at(extra, at + 2)?);
        let data = extra.get(at + 4..at + 4 + size)?;
        out.push((id, data));
        at += 4 + size;
    }
    Some(out)
}

/// Where the central directory starts, how long it is and how many entries it has, from the
/// end record (and its ZIP64 record when one is needed).
fn directory(b: &[u8]) -> Result<(usize, usize, u64), String> {
    // The end record is the last 22 bytes plus a comment of up to 65535.
    let lowest = b.len().saturating_sub(22 + 0xffff);
    let end = (lowest..=b.len().saturating_sub(22)).rev().find(|&i| u32_at(b, i) == Some(END)).ok_or_else(corrupt)?;
    let (disk, cd_disk) = (u16_at(b, end + 4).ok_or_else(corrupt)?, u16_at(b, end + 6).ok_or_else(corrupt)?);
    let mut count = u64::from(u16_at(b, end + 10).ok_or_else(corrupt)?);
    let mut size = u64::from(u32_at(b, end + 12).ok_or_else(corrupt)?);
    let mut offset = u64::from(u32_at(b, end + 16).ok_or_else(corrupt)?);
    if disk != 0 || cd_disk != 0 {
        return Err(refused("it spans several disks"));
    }
    if count == 0xffff || size == 0xffff_ffff || offset == 0xffff_ffff {
        let locator = end.checked_sub(20).filter(|&l| u32_at(b, l) == Some(LOCATOR64)).ok_or_else(corrupt)?;
        let at = usize::try_from(u64_at(b, locator + 8).ok_or_else(corrupt)?).map_err(|_| corrupt())?;
        if u32_at(b, at) != Some(END64) {
            return Err(corrupt());
        }
        count = u64_at(b, at + 32).ok_or_else(corrupt)?;
        size = u64_at(b, at + 40).ok_or_else(corrupt)?;
        offset = u64_at(b, at + 48).ok_or_else(corrupt)?;
    }
    let (size, offset) = (usize::try_from(size).map_err(|_| corrupt())?, usize::try_from(offset).map_err(|_| corrupt())?);
    if offset.checked_add(size).is_none_or(|e| e > b.len()) {
        return Err(corrupt());
    }
    Ok((offset, size, count))
}

/// Refuses an entry whose extra fields are damaged or hold PKWARE's Unix field (a link).
fn check_extra(extra: &[u8]) -> Result<Vec<(u16, &[u8])>, String> {
    let fields = extra_fields(extra).ok_or_else(corrupt)?;
    if fields.iter().any(|(id, _)| *id == EXTRA_UNIX) {
        return Err(refused("an entry is a link"));
    }
    Ok(fields)
}

/// The most a folder zip may hold.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub entries: u64,
    pub bytes: u64,
}

impl Limits {
    /// 100000 entries and 2 GiB uncompressed in all (P4): a remote chose the archive.
    pub const DEFAULT: Limits = Limits { entries: 100_000, bytes: 2 << 30 };
}

/// Checks the zip `b` before it is extracted: its first bytes are a local header or an empty
/// archive's end record, and each central directory entry names a plain file or folder
/// (no link, device or other special file) inside the extraction folder, with the same name
/// in its local header. The reason when refused.
pub fn check(b: &[u8]) -> Result<(), String> {
    check_with(b, Limits::DEFAULT)
}

/// `check` with other caps on the entry count and the total uncompressed size the directory
/// claims (the sizes of the data are not trusted: the directory is read, nothing is expanded).
pub fn check_with(b: &[u8], limits: Limits) -> Result<(), String> {
    let first = u32_at(b, 0);
    if first != Some(LOCAL) && first != Some(END) {
        return Err(refused("it is not a zip archive"));
    }
    let (offset, size, count) = directory(b)?;
    if count > limits.entries {
        return Err(refused("it holds too many entries"));
    }
    let mut at = offset;
    let mut seen = 0u64;
    let mut total = 0u64;
    while at < offset + size {
        if u32_at(b, at) != Some(CENTRAL) {
            return Err(corrupt());
        }
        let made_by = u16_at(b, at + 4).ok_or_else(corrupt)?;
        let usize32 = u32_at(b, at + 24).ok_or_else(corrupt)?;
        let csize32 = u32_at(b, at + 20).ok_or_else(corrupt)?;
        let name_len = usize::from(u16_at(b, at + 28).ok_or_else(corrupt)?);
        let extra_len = usize::from(u16_at(b, at + 30).ok_or_else(corrupt)?);
        let comment_len = usize::from(u16_at(b, at + 32).ok_or_else(corrupt)?);
        let disk = u16_at(b, at + 34).ok_or_else(corrupt)?;
        let external = u32_at(b, at + 38).ok_or_else(corrupt)?;
        let local32 = u32_at(b, at + 42).ok_or_else(corrupt)?;
        let name = b.get(at + 46..at + 46 + name_len).ok_or_else(corrupt)?;
        let extra = b.get(at + 46 + name_len..at + 46 + name_len + extra_len).ok_or_else(corrupt)?;
        if disk != 0 {
            return Err(refused("it spans several disks"));
        }
        // A name is UTF-8 and says so: a tool would read other bytes as another code page.
        let flags = u16_at(b, at + 8).ok_or_else(corrupt)?;
        let Ok(text) = std::str::from_utf8(name) else {
            return Err(refused(&format!("\u{201c}{}\u{201d} is not a UTF-8 name", String::from_utf8_lossy(name).escape_debug())));
        };
        if !text.is_ascii() && flags & 0x800 == 0 {
            return Err(refused(&format!("\u{201c}{}\u{201d} is not marked as a UTF-8 name", text.escape_debug())));
        }
        if let Some(why) = bad_name(text) {
            return Err(refused(&format!("\u{201c}{}\u{201d} has {why}", text.escape_debug())));
        }
        // The types Explorer reads on listing (an icon location can reach a UNC host).
        if text.strip_suffix('/').unwrap_or(text).split('/').any(crate::inert_files::refused_on_listing) {
            return Err(refused(&format!("\u{201c}{}\u{201d} is a type Explorer reads when it lists a folder", text.escape_debug())));
        }
        let host = (made_by >> 8) as u8;
        if host == HOST_UNIX || host == HOST_OSX {
            let kind = (external >> 16) & S_IFMT;
            if kind != S_IFREG && kind != S_IFDIR {
                return Err(refused(&format!("\u{201c}{text}\u{201d} is a link or a special file")));
            }
        } else if external & DOS_REPARSE != 0 {
            return Err(refused(&format!("\u{201c}{text}\u{201d} is a link")));
        }
        let fields = check_extra(extra)?;
        // The local header's offset, from the ZIP64 field when it overflowed (its values come
        // in order: sizes first, each only when its 32-bit field overflowed).
        let mut local = u64::from(local32);
        if local32 == 0xffff_ffff {
            let data = fields.iter().find(|(id, _)| *id == EXTRA_ZIP64).map(|(_, d)| *d).ok_or_else(corrupt)?;
            let skip = 8 * (usize::from(usize32 == 0xffff_ffff) + usize::from(csize32 == 0xffff_ffff));
            local = u64_at(data, skip).ok_or_else(corrupt)?;
        }
        let claimed = if usize32 == 0xffff_ffff { fields.iter().find(|(id, _)| *id == EXTRA_ZIP64).and_then(|(_, d)| u64_at(d, 0)).ok_or_else(corrupt)? } else { u64::from(usize32) };
        total = total.saturating_add(claimed);
        if total > limits.bytes {
            return Err(refused("it would expand to too much data"));
        }
        let local = usize::try_from(local).map_err(|_| corrupt())?;
        if u32_at(b, local) != Some(LOCAL) {
            return Err(corrupt());
        }
        let local_name_len = usize::from(u16_at(b, local + 26).ok_or_else(corrupt)?);
        let local_extra_len = usize::from(u16_at(b, local + 28).ok_or_else(corrupt)?);
        if b.get(local + 30..local + 30 + local_name_len) != Some(name) {
            return Err(refused(&format!("\u{201c}{text}\u{201d} has another name in its local header")));
        }
        check_extra(b.get(local + 30 + local_name_len..local + 30 + local_name_len + local_extra_len).ok_or_else(corrupt)?)?;
        seen += 1;
        at += 46 + name_len + extra_len + comment_len;
    }
    if seen != count {
        return Err(corrupt());
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// One entry of a hand-built zip.
    pub(crate) struct Entry {
        pub name: Vec<u8>,
        pub data: Vec<u8>,
        pub made_by: u16,
        pub external: u32,
        pub extra: Vec<u8>,
        /// The local header's name when it differs.
        pub local_name: Option<Vec<u8>>,
        /// General purpose flags (bit 11, 0x800: the name is UTF-8).
        pub flags: u16,
        /// The uncompressed size the central directory claims, when it is not the data's.
        pub claimed: Option<u32>,
    }

    /// The UTF-8 name flag a tool sets for a name with bytes above ASCII.
    fn utf8_flag(name: &str) -> u16 {
        if name.is_ascii() { 0 } else { 0x800 }
    }

    /// A regular file entry from a Unix host (mode 0644).
    pub(crate) fn file(name: &str, data: &str) -> Entry {
        Entry { name: name.as_bytes().to_vec(), data: data.as_bytes().to_vec(), made_by: 0x031e, external: (S_IFREG | 0o644) << 16, extra: Vec::new(), local_name: None, flags: utf8_flag(name), claimed: None }
    }

    /// A folder entry from a Unix host.
    pub(crate) fn dir(name: &str) -> Entry {
        Entry { name: name.as_bytes().to_vec(), data: Vec::new(), made_by: 0x031e, external: (S_IFDIR | 0o755) << 16, extra: Vec::new(), local_name: None, flags: utf8_flag(name), claimed: None }
    }

    /// The zip CRC-32 (tar.exe checks it).
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }

    /// A stored (uncompressed) zip of `entries`.
    pub(crate) fn zip(entries: &[Entry]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for e in entries {
            let offset = out.len() as u32;
            let local_name = e.local_name.as_ref().unwrap_or(&e.name);
            let crc = crc32(&e.data);
            out.extend(LOCAL.to_le_bytes());
            out.extend([20u16, e.flags, 0, 0, 0].iter().flat_map(|v| v.to_le_bytes()));
            out.extend(crc.to_le_bytes());
            out.extend((e.data.len() as u32).to_le_bytes());
            out.extend((e.data.len() as u32).to_le_bytes());
            out.extend((local_name.len() as u16).to_le_bytes());
            out.extend((e.extra.len() as u16).to_le_bytes());
            out.extend(local_name);
            out.extend(&e.extra);
            out.extend(&e.data);
            central.extend(CENTRAL.to_le_bytes());
            central.extend([e.made_by, 20, e.flags, 0, 0, 0].iter().flat_map(|v| v.to_le_bytes()));
            central.extend(crc.to_le_bytes());
            central.extend((e.data.len() as u32).to_le_bytes());
            central.extend(e.claimed.unwrap_or(e.data.len() as u32).to_le_bytes());
            central.extend((e.name.len() as u16).to_le_bytes());
            central.extend((e.extra.len() as u16).to_le_bytes());
            central.extend([0u16, 0, 0].iter().flat_map(|v| v.to_le_bytes()));
            central.extend(e.external.to_le_bytes());
            central.extend(offset.to_le_bytes());
            central.extend(&e.name);
            central.extend(&e.extra);
        }
        let cd_offset = out.len() as u32;
        out.extend(&central);
        out.extend(END.to_le_bytes());
        out.extend([0u16, 0, entries.len() as u16, entries.len() as u16].iter().flat_map(|v| v.to_le_bytes()));
        out.extend((central.len() as u32).to_le_bytes());
        out.extend(cd_offset.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out
    }

    /// A valid full-environment export: `tool/setup.cfg` and `tool/tool/tool.py`.
    pub(crate) fn valid() -> Vec<Entry> {
        vec![dir("tool/"), file("tool/setup.cfg", "[metadata]\nname=tool\n"), dir("tool/tool/"), file("tool/tool/tool.py", "print(1)\n")]
    }

    #[test]
    fn zip_check_refuses_unsafe_archives() {
        assert_eq!(check(&zip(&valid())), Ok(()));
        // An empty archive is a zip (its layout is refused later).
        assert_eq!(check(&zip(&[])), Ok(()));

        // Not a zip: a tar, an mtree text, a 7z.
        for other in [b"ustar\0\0\0\0\0".to_vec(), b"#mtree\n./x type=file contents=/etc/passwd\n".to_vec(), b"7z\xbc\xaf\x27\x1c".to_vec(), Vec::new()] {
            assert!(check(&other).is_err(), "{other:?}");
        }

        // Hostile names.
        for name in ["../x.py", "tool/../../x.py", "C:/x.py", "/x.py", "tool/tool/a:b", "tool/CON", "tool/aux.py", "tool/Com1.txt", "tool/LPT\u{b9}", "tool/CONIN$", "tool/conout$.x", "tool\\x.py", "tool/x.py.", "tool/x.py ", "tool/a\u{1}b", "tool//x.py", "tool/./x.py", "tool/nul .txt"] {
            let mut entries = valid();
            entries.push(file(name, "x"));
            assert!(check(&zip(&entries)).is_err(), "{name}");
        }
        // Names that only look close are fine.
        for name in ["tool/tool/console.py", "tool/tool/COM10.py", "tool/tool/.hidden", "tool/tool/a.b.c"] {
            let mut entries = valid();
            entries.push(file(name, "x"));
            assert_eq!(check(&zip(&entries)), Ok(()), "{name}");
        }

        // A Unix symbolic link, a device, a fifo; a DOS reparse point; the Unix extra field.
        for kind in [0o120_000u32, 0o020_000, 0o010_000] {
            let mut entries = valid();
            entries.push(Entry { external: (kind | 0o777) << 16, ..file("tool/tool/link", "/etc") });
            assert!(check(&zip(&entries)).is_err(), "{kind:o}");
        }
        let mut entries = valid();
        entries.push(Entry { made_by: 0x0014, external: 0x400 | 0x20, ..file("tool/tool/link", "") });
        assert!(check(&zip(&entries)).is_err());
        let mut entries = valid();
        entries.push(Entry { made_by: 0x0014, external: 0x20, ..file("tool/tool/dos.txt", "") });
        assert_eq!(check(&zip(&entries)), Ok(()), "a plain DOS file");
        let mut entries = valid();
        entries.push(Entry { extra: [EXTRA_UNIX.to_le_bytes(), 4u16.to_le_bytes()].concat().into_iter().chain(*b"/etc").collect(), ..file("tool/tool/hard", "") });
        assert!(check(&zip(&entries)).is_err());

        // A local header naming another place than the directory.
        let mut entries = valid();
        entries.push(Entry { local_name: Some(b"../evil.py".to_vec()), ..file("tool/tool/ok.py", "") });
        assert!(check(&zip(&entries)).is_err());

        // A damaged directory.
        let mut bytes = zip(&valid());
        let n = bytes.len();
        bytes.truncate(n - 3);
        assert!(check(&bytes).is_err());
    }

    /// A folder `d` of entries as a remote folder zip would hold.
    fn d_with(more: Entry) -> Vec<u8> {
        zip(&[dir("d/"), file("d/a.txt", "a"), more])
    }

    #[test]
    fn remote_zips_refuse_hostile_entries() {
        // Accepted: plain names, a UTF-8 name with its flag, DOS attributes, an empty archive.
        assert_eq!(check(&zip(&[dir("d/"), file("d/a.txt", "a"), dir("d/s/"), file("d/s/b.txt", "b"), file("d/\u{e9}.txt", "e")])), Ok(()));
        for dos in [0x10u32, 0x20] {
            assert_eq!(check(&d_with(Entry { made_by: 0x0014, external: dos, ..file("d/dos", "") })), Ok(()), "{dos:#x}");
        }
        assert_eq!(check(&zip(&[])), Ok(()));
        // Refused: every name Windows would read as another place, a device or a stream.
        let names = [
            "../x", "d/../x", "d/./x", "d//x", "/x", "//srv/share/x", "\\\\srv\\x", "d\\x", "C:/x", "C:x", "d/a:b", "x.txt:Zone.Identifier", "d/aux.c", "d/COM1.txt", "d/CONIN$", "d/COM\u{b9}.txt", "d/CON .txt", "d/a\u{1}b", "d/a\tb", "d/a\u{7f}",
            "d/x.", "d/x ", "d./x",
            // The types Explorer parses on listing (P1), as a file and as a folder.
            "d/x.url", "d/x.lnk", "d/desktop.ini", "d/x.library-ms", "d/x.theme", "d/X.URL", "d/a.url/b.txt",
        ];
        for name in names {
            assert!(check(&d_with(file(name, "x"))).is_err(), "{name:?}");
        }
        // A Unix link, a FIFO, a mode with no file type bits, a hard link (PKWARE extra field).
        for mode in [0o120_777u32, 0o010_644, 0o000_644] {
            assert!(check(&d_with(Entry { external: mode << 16, ..file("d/l", "/etc/passwd") })).is_err(), "{mode:o}");
        }
        let hard = [EXTRA_UNIX.to_le_bytes(), 8u16.to_le_bytes()].concat().into_iter().chain(*b"d/a.txt").chain([0]).collect();
        assert!(check(&d_with(Entry { extra: hard, ..file("d/h", "") })).is_err());
        // Names that are not UTF-8: a byte above ASCII with the UTF-8 bit clear, invalid UTF-8 with it set.
        assert!(check(&d_with(Entry { name: b"d/\xe9.txt".to_vec(), flags: 0, ..file("d/x", "") })).is_err());
        assert!(check(&d_with(Entry { name: b"d/\xe9.txt".to_vec(), flags: 0x800, ..file("d/x", "") })).is_err());
        assert!(check(&d_with(Entry { name: "d/\u{e9}.txt".as_bytes().to_vec(), flags: 0, ..file("d/x", "") })).is_err(), "valid UTF-8 without the bit is read as another code page by tar");
        // A local header naming another place than the directory.
        assert!(check(&d_with(Entry { local_name: Some(b"../a".to_vec()), ..file("d/a", "") })).is_err());
        // The end record counting differently than the directory.
        let mut bytes = zip(&[dir("d/"), file("d/a.txt", "a")]);
        let end = bytes.len() - 22;
        bytes[end + 10] = 5;
        assert!(check(&bytes).is_err());
        // Not a zip at all.
        assert!(check(b"no end record here, not a zip at all").is_err());
        // A ZIP64 end record that counts differently than the directory.
        let mut bytes = zip(&[dir("d/"), file("d/a.txt", "a")]);
        let end = bytes.len() - 22;
        let (cd_size, cd_offset) = (u32::from_le_bytes(bytes[end + 12..end + 16].try_into().unwrap()), u32::from_le_bytes(bytes[end + 16..end + 20].try_into().unwrap()));
        bytes.truncate(end);
        let at = bytes.len() as u64;
        bytes.extend(END64.to_le_bytes());
        bytes.extend(44u64.to_le_bytes());
        bytes.extend([45u16, 45].iter().flat_map(|v| v.to_le_bytes()));
        bytes.extend([0u32, 0].iter().flat_map(|v| v.to_le_bytes()));
        bytes.extend([7u64, 7, u64::from(cd_size), u64::from(cd_offset)].iter().flat_map(|v| v.to_le_bytes()));
        bytes.extend(LOCATOR64.to_le_bytes());
        bytes.extend(0u32.to_le_bytes());
        bytes.extend(at.to_le_bytes());
        bytes.extend(1u32.to_le_bytes());
        bytes.extend(END.to_le_bytes());
        bytes.extend([0u16, 0, 0xffff, 0xffff].iter().flat_map(|v| v.to_le_bytes()));
        bytes.extend([0xffff_ffffu32, 0xffff_ffff].iter().flat_map(|v| v.to_le_bytes()));
        bytes.extend(0u16.to_le_bytes());
        assert!(check(&bytes).is_err(), "zip64 says 7 entries, the directory holds 2");
    }

    #[test]
    fn remote_zips_have_a_size_cap() {
        let two = zip(&[file("d/a.txt", "12345"), file("d/b.txt", "12345")]);
        let tight = Limits { entries: 1, bytes: 1 << 20 };
        assert!(check_with(&two, tight).is_err(), "two entries over a cap of one");
        assert!(check_with(&two, Limits { entries: 2, bytes: 9 }).is_err(), "ten bytes over a cap of nine");
        assert_eq!(check_with(&two, Limits { entries: 2, bytes: 10 }), Ok(()));
        // A directory that claims more than the cap, whatever the data holds (also through ZIP64).
        let big = Entry { claimed: Some(0x7000_0000), ..file("d/big", "x") };
        assert!(check(&zip(&[big, Entry { claimed: Some(0x7000_0000), ..file("d/big2", "x") }])).is_err(), "about 3.5 GiB claimed");
        let extra: Vec<u8> = [EXTRA_ZIP64.to_le_bytes(), 8u16.to_le_bytes()].concat().into_iter().chain((3u64 << 30).to_le_bytes()).collect();
        assert!(check(&zip(&[Entry { claimed: Some(0xffff_ffff), extra, ..file("d/z", "x") }])).is_err(), "3 GiB in the ZIP64 field");
        assert_eq!(check(&zip(&[Entry { claimed: Some(0x1000_0000), ..file("d/ok", "x") }])), Ok(()));
    }

    #[test]
    fn device_names_follow_windows() {
        for d in ["CON", "con", "Con.txt", "PRN", "AUX", "NUL", "COM0", "COM9", "LPT1", "COM\u{b2}", "LPT\u{b3}", "CONIN$", "CONOUT$", "NUL .txt", "aux.tar.gz"] {
            assert!(device_name(d), "{d}");
        }
        for n in ["CONSOLE", "COM10", "LPT", "xCON", "nul_", "com1x"] {
            assert!(!device_name(n), "{n}");
        }
        assert_eq!(bad_part("tool"), None);
        assert!(bad_part("tool.").is_some() && bad_part("a:b").is_some() && bad_part("CON").is_some() && bad_part("").is_some());
    }
}

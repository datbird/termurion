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

/// Checks the zip `b` before it is extracted: its first bytes are a local header or an empty
/// archive's end record, and each central directory entry names a plain file or folder
/// (no link, device or other special file) inside the extraction folder, with the same name
/// in its local header. The reason when refused.
pub fn check(b: &[u8]) -> Result<(), String> {
    let first = u32_at(b, 0);
    if first != Some(LOCAL) && first != Some(END) {
        return Err(refused("it is not a zip archive"));
    }
    let (offset, size, count) = directory(b)?;
    let mut at = offset;
    let mut seen = 0u64;
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
        let text = String::from_utf8_lossy(name);
        if let Some(why) = bad_name(&text) {
            return Err(refused(&format!("\u{201c}{}\u{201d} has {why}", text.escape_debug())));
        }
        let host = (made_by >> 8) as u8;
        if host == HOST_UNIX || host == HOST_OSX {
            let kind = (external >> 16) & S_IFMT;
            if kind != 0 && kind != S_IFREG && kind != S_IFDIR {
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
    }

    /// A regular file entry from a Unix host (mode 0644).
    pub(crate) fn file(name: &str, data: &str) -> Entry {
        Entry { name: name.as_bytes().to_vec(), data: data.as_bytes().to_vec(), made_by: 0x031e, external: (S_IFREG | 0o644) << 16, extra: Vec::new(), local_name: None }
    }

    /// A folder entry from a Unix host.
    pub(crate) fn dir(name: &str) -> Entry {
        Entry { name: name.as_bytes().to_vec(), data: Vec::new(), made_by: 0x031e, external: (S_IFDIR | 0o755) << 16, extra: Vec::new(), local_name: None }
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
            out.extend([20u16, 0, 0, 0, 0].iter().flat_map(|v| v.to_le_bytes()));
            out.extend(crc.to_le_bytes());
            out.extend((e.data.len() as u32).to_le_bytes());
            out.extend((e.data.len() as u32).to_le_bytes());
            out.extend((local_name.len() as u16).to_le_bytes());
            out.extend((e.extra.len() as u16).to_le_bytes());
            out.extend(local_name);
            out.extend(&e.extra);
            out.extend(&e.data);
            central.extend(CENTRAL.to_le_bytes());
            central.extend([e.made_by, 20, 0, 0, 0, 0].iter().flat_map(|v| v.to_le_bytes()));
            central.extend(crc.to_le_bytes());
            central.extend((e.data.len() as u32).to_le_bytes());
            central.extend((e.data.len() as u32).to_le_bytes());
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

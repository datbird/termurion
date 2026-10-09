//! An archive's safety check and the parse that extracts it: the one rule every extraction in
//! Tandem uses. A zip (its first bytes a local header or an empty archive's end record, every
//! entry of its central directory) or a gzip tar (every header, `check_tgz`) must name only plain
//! files and folders inside the folder it is extracted into, refused whole otherwise. The walk
//! that checks an archive is the walk `extract` writes from (final findings r2: one parser over
//! one set of bytes; no other program reads the archive). Pure: it reads bytes only, so the
//! Scripts import (milestone 11d), SSH file transfers and Import All share it.

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
/// Info-ZIP's Unicode Path and Unicode Comment fields: libarchive (tar.exe) takes the entry's
/// name from the first when its CRC matches the main name, a name `check` never saw.
const EXTRA_UNICODE_PATH: u16 = 0x7075;
const EXTRA_UNICODE_COMMENT: u16 = 0x6375;
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
/// CONOUT$, COM1-9 and LPT1-9 with the superscript digits too), with any extension: the
/// part before its first dot, trailing spaces ignored, as Windows reads it.
pub fn device_name(part: &str) -> bool {
    crate::inert_files::is_device_name(part)
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

/// The name rule of every entry, zip or tar: `bad_name`, the types Explorer reads on listing (an
/// icon location can reach a UNC host), and any part `more` refuses (Import All: the 8.3 short
/// names, `settings_export::aliased`).
fn check_name(text: &str, more: &dyn Fn(&str) -> bool) -> Result<(), String> {
    if let Some(why) = bad_name(text) {
        return Err(refused(&format!("\u{201c}{}\u{201d} has {why}", text.escape_debug())));
    }
    let parts = || text.strip_suffix('/').unwrap_or(text).split('/');
    if parts().any(crate::inert_files::refused_on_listing) {
        return Err(refused(&format!("\u{201c}{}\u{201d} is a type Explorer reads when it lists a folder", text.escape_debug())));
    }
    if parts().any(more) {
        return Err(refused(&format!("\u{201c}{}\u{201d} has a name Windows reads as another item", text.escape_debug())));
    }
    Ok(())
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
    if fields.iter().any(|(id, _)| *id == EXTRA_UNICODE_PATH || *id == EXTRA_UNICODE_COMMENT) {
        return Err(refused("an entry carries a second name"));
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
    check_total(b, limits).map(|_| ())
}

/// `check_with`, giving the total uncompressed size the directory claims. The extraction
/// (`walk_zip` with a visitor) writes exactly that size per entry, or fails.
pub fn check_total(b: &[u8], limits: Limits) -> Result<u64, String> {
    walk_zip(b, limits, None)
}

/// What a walk hands its visitor for each checked entry: the name (a folder's ends in "/"), the
/// data for a file (None for a folder) and the size the entry claims.
pub type Visit<'a> = &'a mut dyn FnMut(&str, Option<&mut dyn std::io::Read>, u64) -> Result<(), String>;

/// A file's data, read only to exactly its claimed size with its CRC-32: any other length or sum
/// fails the read.
struct Exact<R> {
    inner: R,
    want: u64,
    got: u64,
    crc: flate2::Crc,
    want_crc: u32,
}

impl<R: std::io::Read> std::io::Read for Exact<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n == 0 {
            if self.got != self.want || self.crc.sum() != self.want_crc {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "an entry's data is not what its directory says"));
            }
            return Ok(0);
        }
        self.got += n as u64;
        if self.got > self.want {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "an entry expands past its size"));
        }
        self.crc.update(&buf[..n]);
        Ok(n)
    }
}

/// THE zip walk: `check_total`'s every rule, and with `visit`, each checked entry handed over in
/// directory order (stored or deflated data only, never encrypted). Nothing is visited until the
/// whole directory passed.
pub fn walk_zip(b: &[u8], limits: Limits, visit: Option<Visit>) -> Result<u64, String> {
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
    // (name, folder, data offset, compressed size, size, method, crc)
    let mut found: Vec<(String, bool, usize, usize, u64, u16, u32)> = Vec::new();
    while at < offset + size {
        if u32_at(b, at) != Some(CENTRAL) {
            return Err(corrupt());
        }
        let made_by = u16_at(b, at + 4).ok_or_else(corrupt)?;
        let method = u16_at(b, at + 10).ok_or_else(corrupt)?;
        let crc = u32_at(b, at + 16).ok_or_else(corrupt)?;
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
        check_name(text, &|_| false)?;
        let host = (made_by >> 8) as u8;
        if host == HOST_UNIX || host == HOST_OSX {
            let kind = (external >> 16) & S_IFMT;
            if kind != S_IFREG && kind != S_IFDIR {
                return Err(refused(&format!("\u{201c}{text}\u{201d} is a link or a special file")));
            }
        } else if external & DOS_REPARSE != 0 {
            return Err(refused(&format!("\u{201c}{text}\u{201d} is a link")));
        }
        if flags & 1 != 0 {
            return Err(refused(&format!("\u{201c}{}\u{201d} is encrypted", text.escape_debug())));
        }
        if method != 0 && method != 8 {
            return Err(refused(&format!("\u{201c}{}\u{201d} is compressed in a way Tandem does not read", text.escape_debug())));
        }
        let fields = check_extra(extra)?;
        // The ZIP64 field's values come in order, each only when its 32-bit field overflowed: the
        // size, the compressed size, the local header's offset.
        let zip64 = || fields.iter().find(|(id, _)| *id == EXTRA_ZIP64).map(|(_, d)| *d).ok_or_else(corrupt);
        let (big_size, big_csize) = (usize32 == 0xffff_ffff, csize32 == 0xffff_ffff);
        let claimed = if big_size { u64_at(zip64()?, 0).ok_or_else(corrupt)? } else { u64::from(usize32) };
        let csize = if big_csize { u64_at(zip64()?, 8 * usize::from(big_size)).ok_or_else(corrupt)? } else { u64::from(csize32) };
        let mut local = u64::from(local32);
        if local32 == 0xffff_ffff {
            local = u64_at(zip64()?, 8 * (usize::from(big_size) + usize::from(big_csize))).ok_or_else(corrupt)?;
        }
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
        let data = local + 30 + local_name_len + local_extra_len;
        let csize = usize::try_from(csize).map_err(|_| corrupt())?;
        if data.checked_add(csize).is_none_or(|e| e > b.len()) {
            return Err(corrupt());
        }
        let folder = text.ends_with('/');
        if (folder && claimed != 0) || (method == 0 && csize as u64 != claimed) {
            return Err(refused(&format!("\u{201c}{}\u{201d} has data that does not fit its entry", text.escape_debug())));
        }
        found.push((text.to_string(), folder, data, csize, claimed, method, crc));
        seen += 1;
        at += 46 + name_len + extra_len + comment_len;
    }
    if seen != count {
        return Err(corrupt());
    }
    if let Some(visit) = visit {
        for (name, folder, data, csize, claimed, method, crc) in found {
            let raw = &b[data..data + csize];
            if folder {
                visit(&name, None, 0)?;
                continue;
            }
            let inner: Box<dyn std::io::Read + '_> = if method == 0 { Box::new(raw) } else { Box::new(std::io::Read::take(flate2::read::DeflateDecoder::new(raw), claimed + 1)) };
            let mut data = Exact { inner, want: claimed, got: 0, crc: flate2::Crc::new(), want_crc: crc };
            visit(&name, Some(&mut data), claimed)?;
        }
    }
    Ok(total)
}

/// The pax keys a tar entry may carry: what bsdtar writes for a plain file or folder (a long or
/// non-ASCII name, times, owners). Any other key is refused: `linkpath` makes a file a link,
/// `size` moves where the next header is, `hdrcharset` changes how the name is read, and the
/// sparse, xattr, ACL and flag keys write more than the data.
const PAX_KEYS: [&str; 9] = ["path", "mtime", "atime", "ctime", "uid", "gid", "uname", "gname", "LIBARCHIVE.creationtime"];
/// The most a long name or pax header may hold.
const MAX_META: u64 = 64 << 10;

/// Whether a header's number field is plain octal as both readers of the format agree on it
/// (libarchive's `validate_number_field`, stricter): optional leading spaces, octal digits, then only
/// spaces and NULs. No sign, no other white space, no base-256 (final findings r2 1a: the tar crate
/// trims "+" and "\n", libarchive reads them as 0). `digits` when the field may not be empty.
fn plain_octal(field: &[u8], digits: bool) -> bool {
    let start = field.iter().position(|&b| b != b' ').unwrap_or(field.len());
    let end = start + field[start..].iter().position(|b| !(b'0'..=b'7').contains(b)).unwrap_or(field.len() - start);
    (!digits || end > start) && field[end..].iter().all(|&b| b == b' ' || b == 0)
}

/// Checks the gzip tar read from `r` (Task 9 findings r3; Windows' bsdtar refuses no hard link,
/// R14de-1): every header is ustar or GNU with a good checksum and plain octal numbers and is a
/// regular file, a folder (no data), a pax header or a GNU long name; no entry links anywhere (no
/// link name, no `linkpath`); every name (pax `path`, else the long name, else the header's, never
/// two) passes the one name rule (`check_name`, with `more` for the caller's own parts); every
/// header is where every reader puts it (the gzip members are all read, no pax `size`, a folder
/// carries no data, a file's name does not end in "/"); and the entry count and the total size stay
/// within `limits`. The total size when accepted.
pub fn check_tgz(r: impl std::io::Read, limits: Limits, more: &dyn Fn(&str) -> bool) -> Result<u64, String> {
    walk_tgz(r, limits, more, &mut |_, _, _| Ok(()))
}

/// THE gzip tar walk: `check_tgz`'s every rule, each accepted entry handed to `visit` as it is read
/// (gzip only, never another filter guessed; the tar crate's parse only). An extraction walks the
/// same bytes twice, checking first (`extract`), so nothing is written from an archive that fails.
pub fn walk_tgz(r: impl std::io::Read, limits: Limits, more: &dyn Fn(&str) -> bool, visit: Visit) -> Result<u64, String> {
    use std::io::Read;
    let bad = || refused("it is not a gzip tar or it is damaged");
    let mut archive = tar::Archive::new(flate2::read::MultiGzDecoder::new(r));
    let mut entries = archive.entries().map_err(|_| bad())?.raw(true);
    let (mut path, mut long): (Option<Vec<u8>>, Option<Vec<u8>>) = (None, None);
    let (mut pax, mut seen, mut total) = (false, 0u64, 0u64);
    for entry in entries.by_ref() {
        let mut entry = entry.map_err(|_| bad())?;
        let h = entry.header().clone();
        if h.as_ustar().is_none() && h.as_gnu().is_none() {
            return Err(refused("a header is neither ustar nor GNU"));
        }
        let raw = h.as_bytes();
        let numbers = [(100..108, false), (108..116, false), (116..124, false), (124..136, true), (136..148, false), (148..156, true)];
        if !numbers.into_iter().all(|(at, digits)| plain_octal(&raw[at], digits)) {
            return Err(refused("a header holds a number that is not plain octal"));
        }
        let size = h.entry_size().map_err(|_| bad())?;
        let kind = h.entry_type();
        if h.link_name_bytes().is_some_and(|l| !l.is_empty()) {
            return Err(refused("an entry is a link"));
        }
        if kind.is_pax_local_extensions() || kind.is_gnu_longname() {
            if size > MAX_META {
                return Err(bad());
            }
            let mut body = Vec::new();
            entry.read_to_end(&mut body).map_err(|_| bad())?;
            if kind.is_gnu_longname() {
                if long.is_some() {
                    return Err(bad());
                }
                long = Some(body.strip_suffix(&[0]).unwrap_or(&body).to_vec());
            } else {
                if pax {
                    return Err(bad());
                }
                pax = true;
                for field in tar::PaxExtensions::new(&body) {
                    let field = field.map_err(|_| bad())?;
                    let key = field.key().map_err(|_| bad())?;
                    if key == "linkpath" {
                        return Err(refused("an entry is a link"));
                    }
                    if !PAX_KEYS.contains(&key) {
                        return Err(refused(&format!("an entry carries the pax key \u{201c}{}\u{201d}", key.escape_debug())));
                    }
                    if key == "path" {
                        if path.is_some() {
                            return Err(bad());
                        }
                        path = Some(field.value_bytes().to_vec());
                    }
                }
            }
            continue;
        }
        let name = match (path.take(), long.take()) {
            (Some(_), Some(_)) => return Err(refused("an entry has two names")),
            (Some(n), None) | (None, Some(n)) => n,
            (None, None) => h.path_bytes().into_owned(),
        };
        pax = false;
        let Ok(text) = std::str::from_utf8(&name) else {
            return Err(refused(&format!("\u{201c}{}\u{201d} is not a UTF-8 name", String::from_utf8_lossy(&name).escape_debug())));
        };
        if kind != tar::EntryType::Regular && kind != tar::EntryType::Directory {
            return Err(refused(&format!("\u{201c}{}\u{201d} is a link or a special file", text.escape_debug())));
        }
        check_name(text, more)?;
        let folder = kind == tar::EntryType::Directory;
        if (folder && size != 0) || (!folder && text.ends_with('/')) {
            return Err(refused(&format!("\u{201c}{}\u{201d} is a folder and a file at once", text.escape_debug())));
        }
        seen += 1;
        if seen > limits.entries {
            return Err(refused("it holds too many entries"));
        }
        total = total.saturating_add(size);
        if total > limits.bytes {
            return Err(refused("it would expand to too much data"));
        }
        // A folder's name ends in "/" for the visitor, whatever the header wrote.
        let shown = if folder && !text.ends_with('/') { format!("{text}/") } else { text.to_string() };
        visit(&shown, (!folder).then_some(&mut entry as &mut dyn Read), size)?;
    }
    if pax || path.is_some() || long.is_some() {
        return Err(refused("a header describes an entry that never comes"));
    }
    Ok(total)
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

    /// One tar entry as raw blocks: a ustar header of type `kind` naming `name` (any bytes, nothing
    /// cleaned) and linking to `link`, then `data` padded to a block.
    pub(crate) fn tar_entry(name: &str, kind: u8, data: &[u8], link: &str) -> Vec<u8> {
        let mut h = tar::Header::new_ustar();
        let u = h.as_ustar_mut().unwrap();
        u.name[..name.len()].copy_from_slice(name.as_bytes());
        u.linkname[..link.len()].copy_from_slice(link.as_bytes());
        u.typeflag = [kind];
        h.set_mode(0o644);
        h.set_size(data.len() as u64);
        h.set_mtime(0);
        h.set_cksum();
        let mut out = h.as_bytes().to_vec();
        out.extend(data);
        out.resize(out.len().div_ceil(512) * 512, 0);
        out
    }

    /// A pax extended header ('x') carrying `records` for the entry after it.
    pub(crate) fn pax(records: &[(&str, &str)]) -> Vec<u8> {
        let mut body = Vec::new();
        for (k, v) in records {
            let rest = format!(" {k}={v}\n");
            let mut len = rest.len() + 1;
            while format!("{len}").len() + rest.len() != len {
                len += 1;
            }
            body.extend(format!("{len}{rest}").into_bytes());
        }
        tar_entry("PaxHeader/x", b'x', &body, "")
    }

    /// `blocks` with the end-of-archive mark, gzipped.
    pub(crate) fn tgz(blocks: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(blocks).unwrap();
        gz.write_all(&[0u8; 1024]).unwrap();
        gz.finish().unwrap()
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
        // A claim under the cap passes the cap; a stored entry whose data is not that size is
        // refused for its data (the extraction writes exactly the claim, final findings r2).
        assert!(check(&zip(&[Entry { claimed: Some(0x1000_0000), ..file("d/ok", "x") }])).unwrap_err().contains("does not fit its entry"));
    }

    #[test]
    fn device_names_follow_windows() {
        for d in ["CON", "con", "Con.txt", "PRN", "AUX", "NUL", "COM9", "LPT1", "COM\u{b2}", "LPT\u{b3}", "CONIN$", "CONOUT$", "NUL .txt", "aux.tar.gz"] {
            assert!(device_name(d), "{d}");
        }
        for n in ["CONSOLE", "COM0", "COM10", "LPT", "xCON", "nul_", "com1x"] {
            assert!(!device_name(n), "{n}");
        }
        assert_eq!(bad_part("tool"), None);
        assert!(bad_part("tool.").is_some() && bad_part("a:b").is_some() && bad_part("CON").is_some() && bad_part("").is_some());
    }

    /// Task 9 findings r3: the gzip tar check refuses every entry that is not a plain file or
    /// folder, every link however it is spelled, every name the zip rule refuses, and every
    /// header two readers could place differently.
    #[test]
    fn tgz_check_refuses_unsafe_entries() {
        let none = |_: &str| false;
        let ok = |blocks: &[u8]| check_tgz(&tgz(blocks)[..], Limits::DEFAULT, &none);
        let base = [tar_entry("app-support/", b'5', b"", ""), tar_entry("app-support/a.txt", b'0', b"abc", ""), tar_entry("app-support/old.txt", 0, b"de", "")].concat();
        assert_eq!(ok(&base), Ok(5));
        let long = "d/".to_string() + &"n".repeat(150) + "\u{e9}.txt";
        assert_eq!(ok(&[pax(&[("path", long.as_str()), ("mtime", "1.5"), ("LIBARCHIVE.creationtime", "1")]), tar_entry("d/short", b'0', b"x", "")].concat()), Ok(1), "a pax path");
        let gnu_long = |name: &str| {
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::GNULongName);
            h.set_size(name.len() as u64 + 1);
            h.as_gnu_mut().unwrap().name[..13].copy_from_slice(b"././@LongLink");
            h.set_cksum();
            let mut out = h.as_bytes().to_vec();
            out.extend(format!("{name}\0").as_bytes());
            out.resize(out.len().div_ceil(512) * 512, 0);
            out.extend(tar_entry("d/short", b'0', b"x", ""));
            out
        };
        let gnu_blocks = gnu_long(&long);
        assert_eq!(ok(&gnu_blocks), Ok(1), "a GNU long name");
        assert!(ok(&gnu_long(&format!("../{long}"))).is_err(), "the long name is the one checked");
        assert!(ok(&[pax(&[("path", "../x")]), tar_entry("d/short", b'0', b"x", "")].concat()).is_err(), "the pax path is the one checked");
        assert!(ok(&[pax(&[("path", "d/a")]), gnu_blocks.clone()].concat()).is_err(), "two names for one entry");
        assert!(ok(&[pax(&[("mtime", "1")]), pax(&[("mtime", "1")]), tar_entry("d/a", b'0', b"", "")].concat()).is_err(), "two pax headers");
        assert!(ok(&pax(&[("mtime", "1")])).is_err(), "a pax header with no entry");

        // Every type that is not a file or a folder: hard link, symlink, devices, fifo, contiguous,
        // GNU long link, pax global, sparse, multi-volume, dump dir, vendor letters.
        for kind in [b'1', b'2', b'3', b'4', b'6', b'7', b'K', b'g', b'S', b'M', b'D', b'V', b'N', b'A', b'X'] {
            assert!(ok(&[base.clone(), tar_entry("app-support/z", kind, b"", "")].concat()).is_err(), "{}", kind as char);
        }
        // A link name on any entry, in the header or as pax linkpath, data or not.
        for (name, kind, data, link) in [("d/a", b'0', &b"x"[..], "C:/t"), ("d/", b'5', &b""[..], "../../t"), ("d/a", b'1', &b"PWNED"[..], "../../t"), ("d/a", b'2', &b""[..], "/etc")] {
            assert!(ok(&tar_entry(name, kind, data, link)).is_err(), "{name} {link}");
        }
        for key in ["linkpath", "size", "hdrcharset", "GNU.sparse.size", "SCHILY.xattr.x", "LIBARCHIVE.xattr.x", "SCHILY.fflags", "SCHILY.acl.access", "comment"] {
            let refused = ok(&[pax(&[(key, "1")]), tar_entry("d/a", b'0', b"x", "")].concat());
            assert!(refused.is_err(), "{key}");
        }
        // The zip's name rule, and the caller's own parts.
        for name in ["/x", "../x", "d/../x", "C:/x", "d/a:b", "d\\x", "d/CON", "d/x.", "d/x ", "d/a\u{1}b", "d//x", "d/./x", "d/x.lnk", "d/desktop.ini"] {
            assert!(ok(&tar_entry(name, b'0', b"x", "")).is_err(), "{name:?}");
        }
        assert!(check_tgz(&tgz(&tar_entry("d/SETTIN~1.TOM", b'0', b"x", ""))[..], Limits::DEFAULT, &crate::settings_export::aliased).is_err());
        assert_eq!(ok(&tar_entry("d/SETTIN~1.TOM", b'0', b"x", "")), Ok(1), "the short-name rule is the caller's");
        let mut latin = tar_entry("d/e", b'0', b"x", "");
        latin[2] = 0xe9;
        let mut h = tar::Header::new_old();
        h.as_mut_bytes().copy_from_slice(&latin[..512]);
        h.set_cksum();
        latin[..512].copy_from_slice(h.as_bytes());
        assert!(ok(&latin).is_err(), "a name that is not UTF-8");
        // Headers placed differently by another reader: a folder with data, a file named as a
        // folder, a header that is neither ustar nor GNU, a bad checksum.
        assert!(ok(&tar_entry("d/", b'5', b"PWNED", "")).is_err());
        assert!(ok(&tar_entry("d/", b'0', b"PWNED", "")).is_err());
        let mut v7 = tar_entry("d/a", b'0', b"x", "");
        v7[257..265].copy_from_slice(&[0; 8]);
        assert!(ok(&v7).is_err(), "an old v7 header");
        let mut sum = tar_entry("d/a", b'0', b"x", "");
        sum[0] = b'e';
        assert!(ok(&sum).is_err());
        // A second gzip member is read too (every gzip reader takes them all).
        let mut two = {
            use std::io::Write;
            let mut first = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            first.write_all(&tar_entry("d/a", b'0', b"x", "")).unwrap();
            first.finish().unwrap()
        };
        two.extend(tgz(&tar_entry("d/l", b'1', b"", "../../t")));
        assert!(check_tgz(&two[..], Limits::DEFAULT, &none).is_err(), "a link in the second gzip member");
        // Not a gzip tar.
        for other in [b"not an archive".to_vec(), zip(&valid())] {
            assert!(check_tgz(&other[..], Limits::DEFAULT, &none).is_err());
        }
        // The caps.
        assert!(check_tgz(&tgz(&base)[..], Limits { entries: 2, bytes: 1 << 20 }, &none).is_err());
        assert!(check_tgz(&tgz(&base)[..], Limits { entries: 3, bytes: 4 }, &none).is_err());
    }

    /// Final findings r2 1(a): a number field the tar crate reads and libarchive reads as 0 (a "+",
    /// a leading newline or other white space past the space, base-256) is refused, in the size, the
    /// checksum and every other number; plain octal with space or NUL padding passes.
    #[test]
    fn tgz_numbers_are_plain_octal() {
        let none = |_: &str| false;
        let seal = |b: &mut Vec<u8>, sum: &dyn Fn(u32) -> String| {
            b[148..156].copy_from_slice(b"        ");
            let total: u32 = b[..512].iter().map(|&x| u32::from(x)).sum();
            let text = sum(total);
            b[148..156].fill(0);
            b[148..148 + text.len()].copy_from_slice(text.as_bytes());
        };
        let plain = |s: u32| format!("{s:06o}\0 ");
        let with = |at: std::ops::Range<usize>, field: &[u8], sum: &dyn Fn(u32) -> String| {
            let mut b = tar_entry("d/a", b'0', &[b'x'; 512], "");
            b[at].copy_from_slice(field);
            seal(&mut b, sum);
            check_tgz(&tgz(&[tar_entry("d/", b'5', b"", ""), b].concat())[..], Limits::DEFAULT, &none)
        };
        assert_eq!(with(124..136, b"00000001000\0", &plain), Ok(512));
        assert_eq!(with(124..136, b"   1000    \0", &plain), Ok(512), "leading spaces, space padding");
        for size in [&b"+0000001000\0"[..], b"\n0000001000\0", b"\x0b0000001000\0", b"\t0000001000\0", b"0000001000x\0", b"\x80\0\0\0\0\0\0\0\0\0\x02\0", b"\0\0\0\0\0\0\0\0\0\0\0\0"] {
            assert!(with(124..136, size, &plain).is_err(), "size {size:?}");
        }
        assert!(with(124..136, b"00000001000\0", &|s| format!("+0{s:05o}\0")).is_err(), "a signed checksum");
        assert!(with(124..136, b"00000001000\0", &|s| format!("\n{s:06o}\0")).is_err(), "a checksum after a newline");
        assert!(with(100..108, b"+000644\0", &plain).is_err(), "a signed mode");
        assert!(with(136..148, b"\r0000000000\0", &plain).is_err(), "an mtime after a carriage return");
    }

    /// libarchive replaces an entry's name with the Info-ZIP Unicode Path field (0x7075) when its
    /// CRC matches: a name `check` never saw. Both Unicode fields are refused like 0x000d.
    #[test]
    fn unicode_name_fields_are_refused() {
        for id in [0x7075u16, 0x6375] {
            // version 1, crc32 of "x", name "CON"
            let field: Vec<u8> = [id.to_le_bytes(), 8u16.to_le_bytes()].concat().into_iter().chain([1, 0, 0, 0, 0]).chain(*b"CON").collect();
            assert!(check(&zip(&[dir("d/"), Entry { extra: field, ..file("d/a.txt", "a") }])).is_err(), "{id:#x}");
        }
        assert!(check(&zip(&[dir("d/"), file("d/a.txt", "a")])).is_ok());
    }
}

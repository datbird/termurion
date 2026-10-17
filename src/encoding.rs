//! Character encodings and Unicode normalization (milestone 13o). A session whose profile names
//! an encoding other than UTF-8 gets its output decoded to UTF-8 before the screen sees it, and
//! its typed text and text replies encoded the other way. Windows code pages stand in for
//! iTerm2's NSStringEncodings (no new crate); the system calls sit behind `CodePage` so the
//! stream logic is tested with a fake. Normalization is applied where text leaves the grid.
//! Pure but for `WinCodePage` and `NormalizeString`.

use std::borrow::Cow;

/// UTF-8 is iTerm2's NSUTF8StringEncoding (4): no conversion.
pub const UTF8: i64 = 4;

/// The code page for UTF-16 (little endian), which the Win32 conversion calls reject.
pub const UTF16: u32 = 1200;

/// Every encoding the profile popup lists: Foundation's raw value, iTerm2's name for it and the
/// Windows code page standing in (0 for UTF-8, which needs none).
pub const CHOICES: &[(i64, &str, u32)] = &[
    (1, "Western (ASCII)", 20127),
    (4, "Unicode (UTF-8)", 0),
    (5, "Western (ISO Latin 1)", 28591),
    (9, "Central European (ISO Latin 2)", 28592),
    (10, "Unicode (UTF-16)", 1200),
    (12, "Western (Windows Latin 1, CP1252)", 1252),
    (30, "Western (Mac OS Roman)", 10000),
    (3, "Japanese (EUC)", 20932),
    (8, "Japanese (Shift JIS)", 932),
    (21, "Japanese (ISO 2022-JP)", 50220),
    (11, "Cyrillic (Windows)", 1251),
    (13, "Greek (Windows)", 1253),
    (14, "Turkish (Windows)", 1254),
    (15, "Central European (Windows Latin 2)", 1250),
    (0x8000_0632, "Chinese (GB 18030)", 54936),
    (0x8000_0A03, "Chinese (Big 5)", 950),
    (0x8000_0940, "Korean (EUC)", 51949),
    (0x8000_0A02, "Cyrillic (KOI8-R)", 20866),
];

/// iTerm2's NSStringEncoding to a Windows code page. None for UTF-8 (no conversion) and for a
/// value the table does not know.
pub fn code_page(ns_encoding: i64) -> Option<u32> {
    CHOICES.iter().find(|(v, _, _)| *v == ns_encoding).map(|c| c.2).filter(|&cp| cp != 0)
}

/// What the system's conversions do for a code page. `Decoder` and `encode_input` hold the
/// stream logic and call these for the characters.
pub trait CodePage {
    /// The text of whole characters in `cp`; an undecodable byte becomes U+FFFD.
    fn decode(&self, cp: u32, bytes: &[u8]) -> String;
    /// The bytes of `text` in `cp`; a character the code page lacks becomes its default.
    fn encode(&self, cp: u32, text: &str) -> Vec<u8>;
    /// Whether `b` starts a two-byte (or longer) character in `cp`.
    fn lead_byte(&self, cp: u32, b: u8) -> bool;
}

pub fn utf16le_decode(bytes: &[u8]) -> String {
    char::decode_utf16(bytes.as_chunks::<2>().0.iter().map(|p| u16::from_le_bytes([p[0], p[1]]))).map(|r| r.unwrap_or('\u{fffd}')).collect()
}

pub fn utf16le_encode(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

/// The longest run handed to one decode call.
const RUN_MAX: usize = 64 * 1024;

/// A session's byte stream to UTF-8, one chunk at a time. A character cut by a read waits (at
/// most three bytes) for the next one; bytes below 0x80 pass unchanged in every code page but
/// UTF-16, so escape sequences reach the parser as sent.
#[derive(Clone, Debug)]
pub struct Decoder {
    cp: u32,
    held: Vec<u8>,
}

impl Decoder {
    pub fn new(cp: u32) -> Decoder {
        Decoder { cp, held: Vec::new() }
    }

    pub fn code_page(&self) -> u32 {
        self.cp
    }

    /// `bytes` as UTF-8. UTF-16 pairs the bytes; every other code page decodes runs of
    /// non-ASCII bytes, with a lead byte kept for the next read when its second byte has not
    /// come. A lead followed by a byte that cannot continue it is replaced by U+FFFD.
    pub fn feed(&mut self, bytes: &[u8], sys: &dyn CodePage) -> Vec<u8> {
        let mut data = std::mem::take(&mut self.held);
        data.extend_from_slice(bytes);
        if self.cp == UTF16 {
            return self.feed_utf16(data, sys);
        }
        let cp = self.cp;
        let mut out = Vec::with_capacity(data.len());
        let mut run: Vec<u8> = Vec::new();
        let flush = |run: &mut Vec<u8>, out: &mut Vec<u8>| {
            if !run.is_empty() {
                out.extend_from_slice(sys.decode(cp, run).as_bytes());
                run.clear();
            }
        };
        let mut i = 0;
        while i < data.len() {
            let b = data[i];
            if b < 0x80 {
                flush(&mut run, &mut out);
                out.push(b);
                i += 1;
            } else if sys.lead_byte(cp, b) {
                let Some(&second) = data.get(i + 1) else {
                    self.held = data[i..].to_vec();
                    break;
                };
                if second < min_trail(cp) {
                    flush(&mut run, &mut out);
                    out.extend_from_slice("\u{fffd}".as_bytes());
                    i += 1;
                    continue;
                }
                // Every later trail byte is checked, so a control byte is never swallowed.
                if let Some(at) = bad_trail(cp, &data[i..]) {
                    flush(&mut run, &mut out);
                    out.extend_from_slice("\u{fffd}".as_bytes());
                    i += at;
                    continue;
                }
                let len = sequence_len(cp, b, second);
                if i + len > data.len() {
                    self.held = data[i..].to_vec();
                    break;
                }
                run.extend_from_slice(&data[i..i + len]);
                i += len;
            } else {
                run.push(b);
                i += 1;
            }
            if run.len() >= RUN_MAX {
                flush(&mut run, &mut out);
            }
        }
        flush(&mut run, &mut out);
        out
    }

    /// UTF-16 little endian: whole units, and a high surrogate waits for its low one.
    fn feed_utf16(&mut self, data: Vec<u8>, sys: &dyn CodePage) -> Vec<u8> {
        let mut n = data.len() & !1;
        if n >= 2 && (0xd800..0xdc00).contains(&u16::from_le_bytes([data[n - 2], data[n - 1]])) {
            n -= 2;
        }
        self.held = data[n..].to_vec();
        sys.decode(UTF16, &data[..n]).into_bytes()
    }
}

/// The lowest byte that can follow a lead byte: the double-byte pages start their trail bytes at
/// 0x40, GB 18030's four-byte form at 0x30. A control byte or a space never continues one, so
/// an escape sequence is never swallowed.
fn min_trail(cp: u32) -> u8 {
    if cp == 54936 { 0x30 } else { 0x40 }
}

/// The offset of the first byte past the second that cannot continue the character starting
/// `seq[0]` (GB 18030's third byte is 0x81 to 0xFE and its fourth a digit; EUC-JP's 0x8F form
/// takes a byte of 0xA1 or more third), among the bytes present.
fn bad_trail(cp: u32, seq: &[u8]) -> Option<usize> {
    let len = sequence_len(cp, seq[0], *seq.get(1)?);
    (2..len.min(seq.len())).find(|&j| {
        let b = seq[j];
        match (cp, j) {
            (54936, 2) => !(0x81..=0xfe).contains(&b),
            (54936, _) => !(0x30..=0x39).contains(&b),
            _ => b < 0xa1,
        }
    })
}

/// How many bytes the character starting `lead, second` takes: GB 18030's four-byte form,
/// EUC-JP's three-byte 0x8F form, else two.
fn sequence_len(cp: u32, lead: u8, second: u8) -> usize {
    match cp {
        54936 if (0x30..=0x39).contains(&second) => 4,
        20932 | 51949 if lead == 0x8f => 3,
        _ => 2,
    }
}

/// `text` converted whole to the session's bytes: the answerback, which iTerm2 sends in
/// `self.encoding` (`VT100Terminal.m:1951`), ASCII included.
pub fn encode_whole(cp: u32, text: &str, sys: &dyn CodePage) -> Vec<u8> {
    if cp == 0 {
        return text.as_bytes().to_vec();
    }
    sys.encode(cp, text)
}

/// `text` as the session's bytes. UTF-8 (`cp` 0) is the text itself.
pub fn encode_input(cp: u32, text: &str, sys: &dyn CodePage) -> Vec<u8> {
    if cp == 0 {
        return text.as_bytes().to_vec();
    }
    sys.encode(cp, text)
}

/// Bytes bound for the session, with the text in them encoded: what is typed or pasted (the
/// answerback is encoded where it is set, and replies leave raw). Bytes below 0x80 and anything
/// that is not UTF-8 pass unchanged in a byte code page, so reports and escape sequences are
/// never touched; the three raw bytes of a legacy mouse report (`CSI M`) are never read as text.
/// In UTF-16 every run of text is converted, ASCII too (a stream of two-byte units must not mix
/// with single bytes), while escape sequences and C0 controls leave raw.
pub fn encode_stream(cp: u32, bytes: &[u8], sys: &dyn CodePage) -> Vec<u8> {
    if cp == 0 || (bytes.is_ascii() && cp != UTF16) {
        return bytes.to_vec();
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let rest = &bytes[at..];
        // A legacy mouse report: `ESC [ M` and its three bytes pass raw.
        if rest.starts_with(b"\x1b[M") {
            let n = rest.len().min(6);
            out.extend_from_slice(&rest[..n]);
            at += n;
            continue;
        }
        let stop = rest.windows(3).position(|w| w == b"\x1b[M").unwrap_or(rest.len());
        for chunk in rest[..stop].utf8_chunks() {
            // Only the runs of non-ASCII characters are converted: ASCII, and so every escape
            // sequence, leaves as it is, also in UTF-16 (iTerm2 sends keys and reports raw).
            let valid = chunk.valid();
            let mut rest = valid;
            if cp == UTF16 {
                utf16_text_runs(valid, &mut out, sys);
                out.extend_from_slice(chunk.invalid());
                continue;
            }
            while !rest.is_empty() {
                let ascii = rest.starts_with(|c: char| c.is_ascii());
                let n = rest.find(|c: char| c.is_ascii() != ascii).unwrap_or(rest.len());
                if ascii {
                    out.extend_from_slice(rest[..n].as_bytes());
                } else {
                    out.extend_from_slice(&sys.encode(cp, &rest[..n]));
                }
                rest = &rest[n..];
            }
            out.extend_from_slice(chunk.invalid());
        }
        at += stop;
    }
    out
}

/// `text` for a UTF-16 session: each run between escape sequences and C0 controls (and DEL) is
/// converted whole; the sequences and controls are copied. A sequence is `ESC [ ... final`,
/// `ESC O x` or `ESC` and one more character.
fn utf16_text_runs(text: &str, out: &mut Vec<u8>, sys: &dyn CodePage) {
    let mut rest = text;
    while !rest.is_empty() {
        let control = |c: char| c < ' ' || c == '\u{7f}';
        let n = rest.find(control).unwrap_or(rest.len());
        if n > 0 {
            out.extend_from_slice(&sys.encode(UTF16, &rest[..n]));
            rest = &rest[n..];
            continue;
        }
        // At a control: ESC takes its whole sequence, any other control is one byte.
        let mut chars = rest.char_indices();
        let (_, first) = chars.next().expect("not empty");
        let mut end = first.len_utf8();
        if first == '\x1b' {
            match chars.next() {
                Some((i, '[')) => {
                    end = i + 1;
                    for (j, c) in chars {
                        end = j + c.len_utf8();
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some((i, c)) => end = i + c.len_utf8(),
                None => {}
            }
            // SS3: ESC O and the one character after it.
            if rest[..end].ends_with("\x1bO")
                && let Some(c) = rest[end..].chars().next() {
                    end += c.len_utf8();
                }
        }
        out.extend_from_slice(rest[..end].as_bytes());
        rest = &rest[end..];
    }
}

/// The Windows conversions.
#[cfg(windows)]
pub struct WinCodePage;

#[cfg(windows)]
impl CodePage for WinCodePage {
    fn decode(&self, cp: u32, bytes: &[u8]) -> String {
        use windows_sys::Win32::Globalization::MultiByteToWideChar;
        if cp == UTF16 {
            return utf16le_decode(bytes);
        }
        let Ok(len) = i32::try_from(bytes.len()) else { return String::from_utf8_lossy(bytes).into_owned() };
        if len == 0 {
            return String::new();
        }
        // SAFETY: the pointers and lengths are of live buffers; the first call only measures.
        unsafe {
            let n = MultiByteToWideChar(cp, 0, bytes.as_ptr(), len, std::ptr::null_mut(), 0);
            if n <= 0 {
                return bytes.iter().map(|&b| if b < 0x80 { b as char } else { '\u{fffd}' }).collect();
            }
            let mut wide = vec![0u16; n as usize];
            let got = MultiByteToWideChar(cp, 0, bytes.as_ptr(), len, wide.as_mut_ptr(), n);
            wide.truncate(got.max(0) as usize);
            String::from_utf16_lossy(&wide)
        }
    }

    fn encode(&self, cp: u32, text: &str) -> Vec<u8> {
        use windows_sys::Win32::Globalization::WideCharToMultiByte;
        if cp == UTF16 {
            return utf16le_encode(text);
        }
        let wide: Vec<u16> = text.encode_utf16().collect();
        let Ok(len) = i32::try_from(wide.len()) else { return text.as_bytes().to_vec() };
        if len == 0 {
            return Vec::new();
        }
        // No best fit (`WC_NO_BEST_FIT_CHARS`): a character the page lacks becomes its default,
        // never a lookalike (a fullwidth quote must not turn into `"` in a paste). Windows takes
        // no flags for ISO 2022, GB 18030, UTF-7, UTF-8 and the other symbolic pages.
        let flags = if matches!(cp, 50220..=50229 | 52936 | 54936 | 57002..=57011 | 65000 | 65001 | 42) { 0 } else { 0x400 };
        // SAFETY: as in `decode`. No default character is passed: some code pages refuse one.
        unsafe {
            let n = WideCharToMultiByte(cp, flags, wide.as_ptr(), len, std::ptr::null_mut(), 0, std::ptr::null(), std::ptr::null_mut());
            if n <= 0 {
                return text.as_bytes().to_vec();
            }
            let mut out = vec![0u8; n as usize];
            let got = WideCharToMultiByte(cp, flags, wide.as_ptr(), len, out.as_mut_ptr(), n, std::ptr::null(), std::ptr::null_mut());
            out.truncate(got.max(0) as usize);
            out
        }
    }

    fn lead_byte(&self, cp: u32, b: u8) -> bool {
        match cp {
            // Windows names no lead bytes for the EUC and GB 18030 pages.
            20932 | 51949 => b == 0x8e || b == 0x8f || b >= 0xa1,
            54936 => (0x81..=0xfe).contains(&b),
            // The other Japanese, Chinese and Korean pages Windows knows.
            _ => unsafe { windows_sys::Win32::Globalization::IsDBCSLeadByteEx(cp, b) != 0 },
        }
    }
}

/// Where the system has no code pages (the pure code builds elsewhere): UTF-8 as is.
#[cfg(not(windows))]
pub struct NoCodePage;

#[cfg(not(windows))]
impl CodePage for NoCodePage {
    fn decode(&self, _cp: u32, bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    fn encode(&self, _cp: u32, text: &str) -> Vec<u8> {
        text.as_bytes().to_vec()
    }

    fn lead_byte(&self, _cp: u32, _b: u8) -> bool {
        false
    }
}

/// The conversions of this system.
pub fn system() -> &'static dyn CodePage {
    #[cfg(windows)]
    {
        &WinCodePage
    }
    #[cfg(not(windows))]
    {
        &NoCodePage
    }
}

/// The profile's Unicode Normalization Form. HFS+ (3) is a macOS file-system form and does
/// nothing on Windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    None,
    Nfc,
    Nfd,
}

impl Form {
    pub fn from_profile(v: i64) -> Form {
        match v {
            1 => Form::Nfc,
            2 => Form::Nfd,
            _ => Form::None,
        }
    }
}

/// `text` in the form. ASCII is the same in every form and is never copied.
pub fn normalize(text: &str, form: Form) -> Cow<'_, str> {
    if form == Form::None || text.is_ascii() {
        return Cow::Borrowed(text);
    }
    normalize_system(text, form)
}

#[cfg(windows)]
fn normalize_system(text: &str, form: Form) -> Cow<'_, str> {
    use windows_sys::Win32::Globalization::{NormalizationC, NormalizationD, NormalizeString};
    let wide: Vec<u16> = text.encode_utf16().collect();
    let Ok(len) = i32::try_from(wide.len()) else { return Cow::Borrowed(text) };
    let norm = if form == Form::Nfc { NormalizationC } else { NormalizationD };
    // SAFETY: the buffers are live for the calls; the first call asks for an estimate, and a
    // negative answer is the size the second needs. A few rounds at most, then the text as it was.
    unsafe {
        let mut size = NormalizeString(norm, wide.as_ptr(), len, std::ptr::null_mut(), 0);
        for _ in 0..4 {
            if size <= 0 {
                break;
            }
            let mut out = vec![0u16; size as usize];
            let got = NormalizeString(norm, wide.as_ptr(), len, out.as_mut_ptr(), size);
            if got > 0 {
                out.truncate(got as usize);
                return Cow::Owned(String::from_utf16_lossy(&out));
            }
            size = -got;
        }
    }
    Cow::Borrowed(text)
}

#[cfg(not(windows))]
fn normalize_system(text: &str, _form: Form) -> Cow<'_, str> {
    Cow::Borrowed(text)
}

/// `normalize` in place.
pub fn normalize_in_place(text: &mut String, form: Form) {
    if let Cow::Owned(n) = normalize(text, form) {
        *text = n;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Latin-1 for 28591; for 932 a two-byte page where 0x81 to 0x9F lead and a pair is the
    /// character `0x3000 + 0x100 * (lead - 0x80) + trail`; 1200 as the real one.
    struct FakeCodePage;

    fn pair(lead: u8, trail: u8) -> char {
        char::from_u32(0x3000 + 0x100 * (lead as u32 - 0x80) + trail as u32).unwrap()
    }

    impl CodePage for FakeCodePage {
        fn decode(&self, cp: u32, bytes: &[u8]) -> String {
            match cp {
                1200 => utf16le_decode(bytes),
                932 => {
                    let mut out = String::new();
                    let mut i = 0;
                    while i < bytes.len() {
                        if self.lead_byte(cp, bytes[i]) && i + 1 < bytes.len() {
                            out.push(pair(bytes[i], bytes[i + 1]));
                            i += 2;
                        } else {
                            out.push(if bytes[i] < 0x80 { bytes[i] as char } else { '\u{fffd}' });
                            i += 1;
                        }
                    }
                    out
                }
                _ => bytes.iter().map(|&b| b as char).collect(),
            }
        }

        fn encode(&self, cp: u32, text: &str) -> Vec<u8> {
            match cp {
                1200 => utf16le_encode(text),
                _ => text.chars().map(|c| if (c as u32) < 256 { c as u32 as u8 } else { b'?' }).collect(),
            }
        }

        fn lead_byte(&self, cp: u32, b: u8) -> bool {
            cp == 932 && (0x81..=0x9f).contains(&b)
        }
    }

    #[test]
    fn encodings_follow_iterm2() {
        assert_eq!(code_page(4), None);
        assert_eq!(code_page(5), Some(28591));
        assert_eq!(code_page(12), Some(1252));
        assert_eq!(code_page(8), Some(932));
        assert_eq!(code_page(3), Some(20932));
        assert_eq!(code_page(10), Some(1200));
        assert_eq!(code_page(9999), None);
        assert_eq!(code_page(21), Some(50220));
        assert_eq!(code_page(0x8000_0632), Some(54936));
        assert_eq!(Decoder::new(28591).feed(b"caf\xe9", &FakeCodePage), "caf\u{e9}".as_bytes());
        assert_eq!(encode_input(28591, "\u{e9}", &FakeCodePage), vec![0xe9]);
        assert_eq!(encode_input(0, "\u{e9}", &FakeCodePage), "\u{e9}".as_bytes());
    }

    #[test]
    fn split_characters_decode_once() {
        let fake = FakeCodePage;
        let mut d = Decoder::new(932);
        assert_eq!(d.feed(b"a\x82", &fake), b"a");
        let want = format!("{}b", pair(0x82, 0xa0));
        assert_eq!(d.feed(b"\xa0b", &fake), want.as_bytes());
        // Escape sequences and every byte below 0x80 pass unchanged, in both kinds of page.
        let esc = b"\x1b[31mhi\x1b[0m\r\n";
        assert_eq!(Decoder::new(932).feed(esc, &fake), esc);
        assert_eq!(Decoder::new(28591).feed(esc, &fake), esc);
        // UTF-16 pairs bytes: a unit cut in two, and a surrogate pair cut between its units.
        let mut u = Decoder::new(1200);
        assert_eq!(u.feed(b"a", &fake), b"");
        assert_eq!(u.feed(b"\x00b\x00", &fake), b"ab");
        let smile = utf16le_encode("\u{1f600}");
        assert_eq!(u.feed(&smile[..2], &fake), b"");
        assert_eq!(u.feed(&smile[2..], &fake), "\u{1f600}".as_bytes());
        // An unpaired high surrogate becomes U+FFFD, never a panic.
        assert_eq!(u.feed(&[0x3d, 0xd8, b'a', 0], &fake), "\u{fffd}a".as_bytes());
    }

    #[test]
    fn decoder_is_bounded() {
        let fake = FakeCodePage;
        // A lead followed by a control byte is no pair: the lead is replaced, the control passes.
        assert_eq!(Decoder::new(932).feed(b"\x82\x1bX", &fake), "\u{fffd}\x1bX".as_bytes());
        // A lead alone at the end waits, and the next read's text after it is still seen.
        let mut d = Decoder::new(932);
        assert_eq!(d.feed(b"\x82", &fake), b"");
        assert_eq!(d.feed(b"\x1b", &fake), "\u{fffd}\x1b".as_bytes());
        // Hostile input: no panic, the hold never passes three bytes, the output stays bounded.
        let junk: Vec<u8> = (0..300_000u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
        for cp in [932, 28591, 1200] {
            let mut d = Decoder::new(cp);
            let out: usize = junk.chunks(997).map(|c| d.feed(c, &fake).len()).sum();
            assert!(out <= junk.len() * 3, "{cp}: {out}");
            assert!(d.held.len() <= 3);
        }
    }

    #[test]
    fn typed_text_is_encoded_and_reports_are_not() {
        let fake = FakeCodePage;
        assert_eq!(encode_stream(28591, b"ls \xc3\xa9\r", &fake), b"ls \xe9\r");
        // ASCII, escape sequences and bytes that are not UTF-8 pass untouched.
        assert_eq!(encode_stream(28591, b"\x1b[1;2R", &fake), b"\x1b[1;2R");
        assert_eq!(encode_stream(28591, b"\xff\xfe", &fake), b"\xff\xfe");
        // The three raw bytes of a legacy mouse report are never text, even when they look like it.
        assert_eq!(encode_stream(28591, b"\x1b[M \xc3\xa9", &fake), b"\x1b[M \xc3\xa9");
        // UTF-16 sessions get everything as UTF-16.
        // The text of a UTF-16 session is UTF-16 whole (ASCII too, or the stream is misaligned);
        // escape sequences and C0 controls leave raw (iTerm2 sends keys and reports raw).
        assert_eq!(encode_stream(1200, b"a\xc3\xa9", &fake), vec![0x61, 0, 0xe9, 0]);
        assert_eq!(encode_stream(1200, b"ls\r", &fake), vec![b'l', 0, b's', 0, b'\r']);
        assert_eq!(encode_stream(1200, b"\x1b[1;2R", &fake), b"\x1b[1;2R");
        assert_eq!(encode_stream(1200, b"a\x1b[Ab\x1bOPc\t", &fake), vec![b'a', 0, 0x1b, b'[', b'A', b'b', 0, 0x1b, b'O', b'P', b'c', 0, b'\t']);
        assert_eq!(encode_stream(1200, b"\x1b[M a\xc3\xa9", &fake), b"\x1b[M a\xc3\xa9", "a legacy mouse report is raw to its end");
        assert_eq!(encode_stream(0, b"\xc3\xa9", &fake), b"\xc3\xa9");
    }

    #[test]
    fn choices_match_the_table_and_the_schema() {
        for (v, name, cp) in CHOICES {
            assert_eq!(code_page(*v).unwrap_or(0), *cp, "{name}");
        }
        let schema = crate::schema::profile();
        let s = schema.iter().find(|s| s.key == "Character Encoding").unwrap();
        let mut from_schema: Vec<(i64, String)> = s.choices.iter().map(|c| (c.value.as_integer().unwrap(), c.label.clone())).collect();
        let mut from_table: Vec<(i64, String)> = CHOICES.iter().map(|(v, n, _)| (*v, n.to_string())).collect();
        from_schema.sort();
        from_table.sort();
        assert_eq!(from_schema, from_table);
    }

    #[test]
    fn form_follows_the_profile() {
        assert_eq!(Form::from_profile(0), Form::None);
        assert_eq!(Form::from_profile(1), Form::Nfc);
        assert_eq!(Form::from_profile(2), Form::Nfd);
        assert_eq!(Form::from_profile(3), Form::None);
        assert_eq!(normalize("e\u{301}", Form::None), "e\u{301}");
    }

    #[cfg(windows)]
    #[test]
    fn normalize_follows_windows() {
        assert_eq!(normalize("e\u{301}", Form::Nfc), "\u{e9}");
        assert_eq!(normalize("\u{e9}", Form::Nfd), "e\u{301}");
        assert_eq!(normalize("e\u{301}", Form::None), "e\u{301}");
        assert!(matches!(normalize("plain", Form::Nfc), Cow::Borrowed(_)));
        let long = "e\u{301}".repeat(5000);
        assert_eq!(normalize(&long, Form::Nfc), "\u{e9}".repeat(5000));
    }

    #[cfg(windows)]
    #[test]
    fn code_pages_round_trip() {
        let w = WinCodePage;
        assert_eq!(w.decode(932, b"\x82\xa0"), "\u{3042}");
        assert_eq!(w.encode(932, "\u{3042}"), vec![0x82, 0xa0]);
        assert_eq!(w.decode(1200, b"a\x00"), "a");
        assert_eq!(w.encode(1200, "a"), vec![0x61, 0x00]);
        assert!(w.lead_byte(932, 0x82));
        assert!(!w.lead_byte(932, b'a'));
        assert!(!w.lead_byte(1252, 0x82));
        assert_eq!(w.decode(1252, b"caf\xe9"), "caf\u{e9}");
        assert_eq!(w.decode(1252, b""), "");
        // The real decoder over the real code page, a character split across reads.
        let mut d = Decoder::new(932);
        assert_eq!(d.feed(b"x\x82", &w), b"x");
        assert_eq!(d.feed(b"\xa0y", &w), "\u{3042}y".as_bytes());
        // A character the code page lacks encodes as its default, never a panic.
        assert_eq!(w.encode(1252, "\u{3042}"), vec![b'?']);
    }

    /// Fix round 1 (Important 1): escape sequences stay raw in a UTF-16 session; only text with
    /// non-ASCII characters is converted.
    #[test]
    fn utf16_keeps_escape_sequences_raw() {
        let fake = FakeCodePage;
        assert_eq!(encode_stream(1200, b"\x1b[A", &fake), b"\x1b[A");
        assert_eq!(encode_stream(1200, b"\x1b[1;1R", &fake), b"\x1b[1;1R");
        assert_eq!(encode_stream(1200, "\u{e9}".as_bytes(), &fake), vec![0xe9, 0]);
        // The answerback is text and is converted whole.
        assert_eq!(encode_whole(1200, "hi", &fake), vec![b'h', 0, b'i', 0]);
        assert_eq!(encode_whole(0, "h\u{e9}", &fake), "h\u{e9}".as_bytes());
        assert_eq!(encode_whole(28591, "h\u{e9}", &fake), b"h\xe9");
    }

    /// Fix round 1 (Minor 2): every trail byte is checked, so a control byte after the lead of a
    /// GB 18030 or EUC-JP three-byte form is never handed to the conversion.
    #[cfg(windows)]
    #[test]
    fn later_trail_bytes_are_checked() {
        let mut d = Decoder::new(54936);
        assert_eq!(d.feed(b"\x81\x30\x1b[0m", system()), "\u{fffd}\x1b[0m".as_bytes());
        assert_eq!(d.feed(b"\x81\x30\x81\x1b[0m", system()), "\u{fffd}\x1b[0m".as_bytes());
        // A whole four-byte character still decodes, also across reads.
        assert_eq!(d.feed(b"\x81\x30\x81\x30", system()), "\u{80}".as_bytes());
        assert!(d.feed(b"\x81\x30\x81", system()).is_empty());
        assert_eq!(d.feed(b"\x30x", system()), "\u{80}x".as_bytes());
        let mut e = Decoder::new(51949);
        assert_eq!(e.feed(b"\x8f\xa1\x1b[0m", system()), "\u{fffd}\x1b[0m".as_bytes());
    }

    /// Fix round 1 (Minor 1, security): a character the page lacks never becomes a similar one
    /// (a fullwidth quote must not become `"` in a paste); it is the page's default.
    #[cfg(windows)]
    #[test]
    fn no_best_fit_characters() {
        assert_eq!(system().encode(1252, "\u{ff02}\u{ff3c}"), b"??");
        assert_eq!(system().encode(932, "\u{a5}"), b"?");
        assert_eq!(system().encode(1252, "caf\u{e9}"), b"caf\xe9");
        assert_eq!(system().encode(932, "\u{3042}"), b"\x82\xa0");
        // Pages that take no flags still convert.
        assert_eq!(system().encode(65001, "\u{ff02}"), "\u{ff02}".as_bytes());
        assert_eq!(system().encode(54936, "\u{ff02}\u{4e2d}").len(), 4);
    }
}

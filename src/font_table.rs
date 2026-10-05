//! Special Font Config: per-Unicode-range font overrides (iTerm2's `FontTable.swift`, `FT`, and
//! the editor's checks in `SpecialExceptionsWindowController.swift`). A profile stores the table
//! as JSON text under "Special Font Config"; a face with a table picks the font per character
//! and may remap the code point it draws. The value comes from a profile, an imported file or
//! the API, so `Config::parse` is bounded and total (13n preflight P6): no entry list longer than
//! `MAX_ENTRIES`, every number inside the Unicode range, no overlap (so `lookup` is a binary
//! search), and a size the renderer can draw.

use std::ops::Range;

use serde::{Deserialize, Serialize};

/// The profile key that stores the table (iTerm2's `Special Font Config`).
pub const KEY: &str = "Special Font Config";
/// `FontTable.unicodeLimit`: one past the highest code point.
pub const UNICODE_LIMIT: i64 = 0x110000;
/// `FontTable.Config.latestKnownVersion`.
pub const LATEST_VERSION: i64 = 2;
/// ASCII never takes a special exception (`asciiLimit` in `FT`, the editor's `.ascii` check).
pub const ASCII_LIMIT: i64 = 0x80;
/// The most entries a table may hold (P6).
pub const MAX_ENTRIES: usize = 512;
/// The longest JSON text `Config::parse` reads.
pub const MAX_TEXT: usize = 256 * 1024;
/// The longest font name an entry may carry.
pub const MAX_FONT_NAME: usize = 256;
/// The size range an entry's point size is kept in: the range `Profile::font` draws (P2).
pub const MIN_POINT_SIZE: f64 = 4.0;
pub const MAX_POINT_SIZE: f64 = 128.0;

/// The editor's text for each refusal (`SpecialExceptions.Preview*`, Tandem's wording).
pub const ASCII_ERROR: &str = "Start must be at least U+80. ASCII doesn\u{2019}t support special exceptions.";
pub const INVALID_RANGE: &str = "Invalid range.";
pub const LIMIT_TOO_LARGE: &str = "End is higher than U+110000, the maximum Unicode code point.";
pub const RANGE_TAKEN: &str = "Range includes an already-assigned code point.";
pub const INVALID_DESTINATION: &str = "Invalid destination";
pub const NO_FONT: &str = "No font selected.";
pub const TRUNCATED: &str = " [truncated]";
pub const IMPORT_MALFORMED: &str = "This file is not well formed. Is it from a newer version of Tandem?";
pub const IMPORT_NEWER: &str = "This file is from a newer version of Tandem and cannot be loaded.";
pub const IMPORT_HEADING: &str = "Problem Importing Special Exceptions";
pub const NERD_DISABLED: &str = "Downloading fonts is not available in Tandem.";

/// The message for fonts an imported file names that are not installed.
pub fn import_missing(fonts: &[String]) -> String {
    format!("You must install the following fonts to use the exceptions in this file:\n\n{}", fonts.join("\n"))
}

/// The preview's text is cut here (`combined.length >= 1024 * 10`).
pub const PREVIEW_LIMIT: usize = 1024 * 10;
/// Code points the preview skips (`disallowed` in `updatePreview`).
pub const PREVIEW_SKIPPED: [u32; 8] = [9, 10, 13, 0xad, 0x200e, 0x200f, 0x200b, 0x200c];

/// The range presets under "Add Preset...", in iTerm2's menu order (sorted by title).
pub const PRESETS: &[(&str, i64, i64)] = &[
    ("Han (CJK Unified Ideographs)", 0x4E00, 0x9FFF),
    ("Hiragana/Katakana", 0x3040, 0x30FF),
    ("Hangul Syllables", 0xAC00, 0xD7AF),
    ("Arabic", 0x0600, 0x06FF),
    ("Cyrillic", 0x0400, 0x04FF),
    ("Greek (Greek and Coptic)", 0x0370, 0x03FF),
    ("Private Use Area", 0xE000, 0xF8FF),
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// The first code point in the font.
    pub start: i64,
    /// The length of the run.
    pub count: i64,
    /// With a destination, the code points `destination..destination + count` the program sends
    /// draw `start..start + count` of the font.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub destination: Option<i64>,
    #[serde(rename = "fontName")]
    pub font_name: String,
    #[serde(rename = "pointSize", skip_serializing_if = "Option::is_none", default)]
    pub point_size: Option<f64>,
}

impl Entry {
    /// The code points the program sends (`range`: `start - delta ..< start - delta + count`).
    pub fn range(&self) -> Range<i64> {
        let first = self.destination.unwrap_or(self.start);
        first..first.saturating_add(self.count)
    }

    /// What to add to a code point the program sent to get the one drawn (`start - destination`).
    pub fn delta(&self) -> Option<i64> {
        self.destination.map(|d| self.start.saturating_sub(d))
    }

    /// The entry with its point size grown, when it has one (`byAddingPointSize`).
    fn grown(&self, delta: f64) -> Entry {
        Entry { point_size: self.point_size.map(|p| clamp_size(p + delta)), ..self.clone() }
    }

    /// Whether every number is inside its bounds (P6).
    fn valid(&self) -> bool {
        let in_range = |v: i64| (0..=UNICODE_LIMIT).contains(&v);
        let tty = self.range();
        in_range(self.start)
            && self.count >= 1
            && self.start.saturating_add(self.count) <= UNICODE_LIMIT
            && self.destination.is_none_or(|d| in_range(d) && d.saturating_add(self.count) <= UNICODE_LIMIT)
            && tty.start >= ASCII_LIMIT
            && self.point_size.is_none_or(f64::is_finite)
            && !self.font_name.is_empty()
            && self.font_name.chars().count() <= MAX_FONT_NAME
            && !self.font_name.chars().any(char::is_control)
    }
}

/// A point size in the range the renderer draws.
fn clamp_size(p: f64) -> f64 {
    p.clamp(MIN_POINT_SIZE, MAX_POINT_SIZE)
}

fn latest() -> i64 {
    LATEST_VERSION
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub entries: Vec<Entry>,
    #[serde(default = "latest")]
    pub version: i64,
}

impl Config {
    /// The table `s` holds, entries sorted by the code points the program sends. None for an
    /// empty string, malformed JSON, or any value outside its bounds (P6).
    pub fn parse(s: &str) -> Option<Config> {
        if s.is_empty() || s.len() > MAX_TEXT {
            return None;
        }
        let mut config: Config = serde_json::from_str(s).ok()?;
        if config.entries.len() > MAX_ENTRIES || config.entries.iter().any(|e| !e.valid()) {
            return None;
        }
        for e in &mut config.entries {
            e.point_size = e.point_size.map(clamp_size);
        }
        config.entries.sort_by_key(|e| e.range().start);
        // Neighbors may not overlap: `lookup` is a binary search.
        if config.entries.windows(2).any(|w| w[0].range().end > w[1].range().start) {
            return None;
        }
        Some(config)
    }

    /// The JSON text a profile stores (`stringValue`).
    pub fn string(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// The index of the entry that draws code point `c` and the code point it draws.
    pub fn find(&self, c: u32) -> Option<(usize, u32)> {
        let c = i64::from(c);
        let i = self.entries.partition_point(|e| e.range().end <= c);
        let e = self.entries.get(i).filter(|e| e.range().contains(&c))?;
        let drawn = u32::try_from(c + e.delta().unwrap_or(0)).ok()?;
        Some((i, drawn))
    }

    /// The entry that draws code point `c`, and the code point it draws.
    pub fn lookup(&self, c: u32) -> Option<(&Entry, u32)> {
        self.find(c).map(|(i, drawn)| (&self.entries[i], drawn))
    }

    /// The table with every sized entry's point size grown by `delta` (`byAddingPointSize`).
    pub fn grown(&self, delta: f64) -> Config {
        Config { entries: self.entries.iter().map(|e| e.grown(delta)).collect(), version: self.version }
    }

    /// The same table with entries kept sorted: what the editor builds after an edit.
    pub fn with_entries(entries: Vec<Entry>) -> Config {
        let mut config = Config { entries, version: LATEST_VERSION };
        config.entries.sort_by_key(|e| e.range().start);
        config
    }
}

/// A stored value grown by `delta` points (`Make Text Bigger` and `Smaller` with Size Changes
/// Affect Profile, and the session's own copy). An empty or unusable value stays as it is.
pub fn grown_value(value: &str, delta: f64) -> String {
    match Config::parse(value) {
        Some(c) if c.entries.iter().any(|e| e.point_size.is_some()) => c.grown(delta).string(),
        _ => value.to_string(),
    }
}

/// The normalized text of a stored value: what a face keys on, so two spellings of one table
/// are one face. None for no table (empty, malformed or out of bounds, or no entries).
pub fn normalized(value: &str) -> Option<String> {
    Config::parse(value).filter(|c| !c.entries.is_empty()).map(|c| c.string())
}

/// Reads an imported file (`importString`): the table it holds, or the text of what is wrong. A
/// file of a newer version is refused, and so is one that names a font `installed` does not know
/// (each missing font once, in file order).
pub fn import(text: &str, installed: &dyn Fn(&str) -> bool) -> Result<Config, String> {
    let config = Config::parse(text).ok_or_else(|| IMPORT_MALFORMED.to_string())?;
    if config.version > LATEST_VERSION {
        return Err(IMPORT_NEWER.to_string());
    }
    let mut missing: Vec<String> = Vec::new();
    for e in &config.entries {
        if !installed(&e.font_name) && !missing.contains(&e.font_name) {
            missing.push(e.font_name.clone());
        }
    }
    if missing.is_empty() { Ok(config) } else { Err(import_missing(&missing)) }
}

/// What the editor's fields say (`CheckedRange`).
#[derive(Clone, Debug, PartialEq)]
pub enum Check {
    Ascii,
    Incomplete,
    Inverted,
    LimitTooLarge,
    Taken,
    InvalidDestination,
    Valid,
}

/// The editor's checks, in iTerm2's order: the field text is `start` to `end` (code points of
/// the font) and `destination` (the code points the program sends, when asked for); `others` are
/// the entries already in the table (without the one being edited).
pub fn check(start: Option<i64>, end: Option<i64>, destination: Option<i64>, others: &[Entry]) -> Check {
    let (Some(start), Some(end)) = (start, end) else { return Check::Incomplete };
    let count = end.saturating_sub(start).saturating_add(1);
    let begin = destination.unwrap_or(start);
    if begin < ASCII_LIMIT {
        return Check::Ascii;
    }
    if count <= 0 {
        return Check::Inverted;
    }
    if begin.saturating_add(count) > UNICODE_LIMIT {
        return Check::LimitTooLarge;
    }
    let last = begin + count;
    if others.iter().any(|e| {
        let r = e.range();
        r.start < last && begin < r.end
    }) {
        return Check::Taken;
    }
    // `destinationIsValid`: the destination run must end below the limit.
    if destination.is_some() && begin + count >= UNICODE_LIMIT {
        return Check::InvalidDestination;
    }
    Check::Valid
}

/// The text a refusal shows in the preview; None for `Incomplete` (the preview is empty) and
/// `Valid`.
pub fn check_text(check: &Check) -> Option<&'static str> {
    match check {
        Check::Ascii => Some(ASCII_ERROR),
        Check::Inverted => Some(INVALID_RANGE),
        Check::LimitTooLarge => Some(LIMIT_TOO_LARGE),
        Check::Taken => Some(RANGE_TAKEN),
        Check::InvalidDestination => Some(INVALID_DESTINATION),
        Check::Incomplete | Check::Valid => None,
    }
}

/// A code point as typed: `U+` and `u+` and `0x` prefixes are hex, anything else decimal
/// (`parseUnicode`).
pub fn parse_code_point(s: &str) -> Option<i64> {
    for prefix in ["U+", "u+", "0x"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            return i64::from_str_radix(rest, 16).ok();
        }
    }
    s.parse::<i64>().ok()
}

/// A code point as the editor shows it (`formatUnicode`): "U+" and lower-case hex.
pub fn format_code_point(v: i64) -> String {
    if v < 0 { v.to_string() } else { format!("U+{v:x}") }
}

/// The text of the preview for a valid range in a font: every code point of the run except the
/// ones that draw as nothing or break the line, up to `PREVIEW_LIMIT` characters, then
/// `TRUNCATED`. The drawn code points are the font's (`start..start + count`).
pub fn preview_text(start: i64, count: i64) -> String {
    let mut out = String::new();
    let mut length = 0;
    let end = start.saturating_add(count).min(UNICODE_LIMIT);
    for i in start.max(0)..end {
        if length >= PREVIEW_LIMIT {
            out.push_str(TRUNCATED);
            break;
        }
        let Some(c) = u32::try_from(i).ok().and_then(char::from_u32) else { continue };
        if PREVIEW_SKIPPED.contains(&(c as u32)) {
            continue;
        }
        out.push(c);
        length += c.len_utf16();
    }
    out
}

/// The row text of an entry in the table: the code points the program sends, and the font.
pub fn row_range_text(e: &Entry) -> String {
    let r = e.range();
    format!("{} to {}", format_code_point(r.start), format_code_point(r.end - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(start: i64, count: i64, destination: Option<i64>, font: &str, size: Option<f64>) -> Entry {
        Entry { start, count, destination, font_name: font.into(), point_size: size }
    }

    #[test]
    fn font_table_follows_iterm2() {
        let c = Config::parse(r#"{"entries":[{"start":57344,"count":6400,"fontName":"Symbols Nerd Font Mono"}],"version":2}"#).expect("parses");
        assert_eq!(c.entries.len(), 1);
        assert_eq!(c.entries[0].range(), 0xE000..0xF900);
        let (e, drawn) = c.lookup(0xE0B0).expect("entry");
        assert_eq!((e.font_name.as_str(), drawn), ("Symbols Nerd Font Mono", 0xE0B0));
        assert!(c.lookup(0xDFFF).is_none() && c.lookup(0xF900).is_none() && c.lookup(0x41).is_none());

        // A destination: the program sends 0x10000.., the font draws 0x41...
        let c = Config::with_entries(vec![entry(0x41, 26, Some(0x10000), "Consolas", None)]);
        assert_eq!(c.entries[0].delta(), Some(0x41 - 0x10000));
        assert_eq!(c.entries[0].range(), 0x10000..0x1001A);
        assert_eq!(c.lookup(0x10000).map(|(_, d)| d), Some(0x41));
        assert_eq!(c.lookup(0x10005).map(|(_, d)| d), Some(0x46));
        assert!(c.lookup(0x1001A).is_none() && c.lookup(0x41).is_none());

        // The text round-trips, in iTerm2's field order.
        let c = Config::with_entries(vec![entry(0x4E00, 100, None, "Meiryo", Some(14.0)), entry(0x41, 26, Some(0x10000), "Consolas", None)]);
        let text = c.string();
        assert_eq!(Config::parse(&text), Some(c.clone()));
        assert!(text.starts_with(r#"{"entries":[{"start":19968,"count":100,"fontName":"Meiryo","pointSize":14.0}"#), "{text}");
        assert!(text.ends_with(r#""version":2}"#));
        assert!(Config::parse("").is_none() && Config::parse("{").is_none() && Config::parse("[]").is_none());

        // Entries sort by the code points the program sends, not by the font's.
        let sorted = Config::parse(&Config::with_entries(vec![entry(0x4E00, 2, None, "A", None), entry(0x300, 2, Some(0x200), "B", None)]).string()).unwrap();
        assert_eq!(sorted.entries.iter().map(|e| e.range().start).collect::<Vec<_>>(), vec![0x200, 0x4E00]);

        // Growing adds to sized entries only.
        let g = c.grown(1.0);
        assert_eq!(g.entries.iter().map(|e| e.point_size).collect::<Vec<_>>(), vec![Some(15.0), None]);
        assert_eq!(g.grown(-2.0).entries[0].point_size, Some(13.0));
        assert_eq!(c.grown(1.0).entries[1].font_name, "Consolas");
        // The stored value grows through its text; an unsized, empty or bad value stays.
        assert!(grown_value(&text, 2.0).contains("16.0"));
        assert_eq!(grown_value("", 1.0), "");
        assert_eq!(grown_value("not json", 1.0), "not json");
        let unsized_table = Config::with_entries(vec![entry(0x4E00, 2, None, "A", None)]).string();
        assert_eq!(grown_value(&unsized_table, 3.0), unsized_table);
    }

    #[test]
    fn a_table_that_asks_too_much_is_refused() {
        let one = |e: &str| format!(r#"{{"entries":[{e}],"version":2}}"#);
        let ok = |e: &str| Config::parse(&one(e));
        assert!(ok(r#"{"start":4096,"count":16,"fontName":"A"}"#).is_some());
        // Bad ranges: negative, empty, past the limit, overflowing, ASCII.
        assert!(ok(r#"{"start":-1,"count":16,"fontName":"A"}"#).is_none());
        assert!(ok(r#"{"start":4096,"count":0,"fontName":"A"}"#).is_none());
        assert!(ok(r#"{"start":4096,"count":9223372036854775807,"fontName":"A"}"#).is_none());
        assert!(ok(r#"{"start":1114000,"count":200,"fontName":"A"}"#).is_none());
        assert!(ok(r#"{"start":9223372036854775807,"count":1,"fontName":"A"}"#).is_none());
        assert!(ok(r#"{"start":4096,"count":16,"destination":9223372036854775800,"fontName":"A"}"#).is_none());
        assert!(ok(r#"{"start":4096,"count":16,"destination":-5,"fontName":"A"}"#).is_none());
        assert!(ok(r#"{"start":65,"count":26,"fontName":"A"}"#).is_none());
        assert!(ok(r#"{"start":4096,"count":16,"destination":65,"fontName":"A"}"#).is_none());
        // No font, a control character, a huge name.
        assert!(ok(r#"{"start":4096,"count":16,"fontName":""}"#).is_none());
        assert!(ok(r#"{"start":4096,"count":16,"fontName":"A\u0007"}"#).is_none());
        assert!(ok(&format!(r#"{{"start":4096,"count":16,"fontName":"{}"}}"#, "x".repeat(300))).is_none());
        // A size is kept in the drawn range.
        assert_eq!(ok(r#"{"start":4096,"count":16,"fontName":"A","pointSize":100000000}"#).unwrap().entries[0].point_size, Some(128.0));
        assert_eq!(ok(r#"{"start":4096,"count":16,"fontName":"A","pointSize":0.5}"#).unwrap().entries[0].point_size, Some(4.0));
        // Overlap makes `lookup` ambiguous.
        assert!(ok(r#"{"start":4096,"count":16,"fontName":"A"},{"start":4100,"count":16,"fontName":"B"}"#).is_none());
        assert!(ok(r#"{"start":4096,"count":16,"fontName":"A"},{"start":4112,"count":16,"fontName":"B"}"#).is_some());
        // Too many entries, too much text.
        let many = |n: i64| (0..n).map(|i| format!(r#"{{"start":{},"count":1,"fontName":"A"}}"#, 0x100 + i)).collect::<Vec<_>>().join(",");
        assert!(ok(&many(MAX_ENTRIES as i64)).is_some());
        assert!(ok(&many(MAX_ENTRIES as i64 + 1)).is_none());
        assert!(Config::parse(&"a".repeat(MAX_TEXT + 1)).is_none());
        // A missing version reads as the latest.
        assert_eq!(Config::parse(r#"{"entries":[]}"#).unwrap().version, LATEST_VERSION);
        // Lookups on a full table are a binary search and stay right at every edge.
        let big = Config::parse(&one(&many(MAX_ENTRIES as i64))).unwrap();
        assert_eq!(big.find(0x100).map(|(i, d)| (i, d)), Some((0, 0x100)));
        assert_eq!(big.find(0x100 + 511).map(|(i, _)| i), Some(511));
        assert!(big.find(0x100 + 512).is_none() && big.find(0xFF).is_none());
        // The face key's text: one spelling for one table, none for no usable table.
        assert_eq!(normalized(" "), None);
        assert_eq!(normalized(r#"{"entries":[],"version":2}"#), None);
        assert_eq!(normalized(&one(r#"{"fontName":"A","count":16,"start":4096}"#)), normalized(&one(r#"{"start":4096,"count":16,"fontName":"A"}"#)));
        assert!(normalized(&one(r#"{"start":4096,"count":16,"fontName":"A"}"#)).is_some());
    }

    #[test]
    fn editor_checks_follow_iterm2() {
        assert_eq!(check(Some(0x41), Some(0x50), None, &[]), Check::Ascii);
        assert_eq!(check(Some(0x100), None, None, &[]), Check::Incomplete);
        assert_eq!(check(None, Some(0x100), None, &[]), Check::Incomplete);
        assert_eq!(check(Some(0x200), Some(0x100), None, &[]), Check::Inverted);
        assert_eq!(check(Some(0x100), Some(0x110001), None, &[]), Check::LimitTooLarge);
        assert_eq!(check(Some(0x100), Some(0x1FF), None, &[]), Check::Valid);
        // A range overlapping an entry already in the table is taken (by the code points the program sends).
        let others = [entry(0x150, 16, None, "A", None), entry(0x41, 4, Some(0x1000), "B", None)];
        assert_eq!(check(Some(0x100), Some(0x1FF), None, &others), Check::Taken);
        assert_eq!(check(Some(0x41), Some(0x44), Some(0x1002), &others), Check::Taken);
        assert_eq!(check(Some(0x41), Some(0x44), Some(0x1004), &others), Check::Valid);
        assert_eq!(check(Some(0x100), Some(0x14F), None, &others), Check::Valid);
        // ASCII is judged by where the program sends the code points.
        assert_eq!(check(Some(0x4E00), Some(0x4E10), Some(0x41), &[]), Check::Ascii);
        assert_eq!(check(Some(0x41), Some(0x50), Some(0x4E00), &[]), Check::Valid);
        // The order: ascii, inverted, limit, taken.
        assert_eq!(check(Some(0x50), Some(0x40), None, &[]), Check::Ascii);
        assert_eq!(check(Some(0x100), Some(0x110001), None, &others), Check::LimitTooLarge);
        // A destination run that reaches the limit is invalid; huge values never overflow.
        assert_eq!(check(Some(0x100), Some(0x10F), Some(0x10FFF0), &[]), Check::InvalidDestination);
        assert_eq!(check(Some(0x100), Some(0x10F), Some(0x10FFEF), &[]), Check::Valid);
        assert_eq!(check(Some(0x100), Some(0x10F), Some(0x10FFF1), &[]), Check::LimitTooLarge);
        assert_eq!(check(Some(i64::MIN), Some(i64::MAX), None, &[]), Check::Ascii);
        assert_eq!(check(Some(0x100), Some(i64::MAX), None, &[]), Check::LimitTooLarge);
        assert_eq!(check(Some(i64::MAX), Some(i64::MIN), Some(i64::MAX), &[]), Check::Inverted);

        assert_eq!(parse_code_point("U+e0b0"), Some(0xE0B0));
        assert_eq!(parse_code_point("u+E0B0"), Some(0xE0B0));
        assert_eq!(parse_code_point("0x41"), Some(0x41));
        assert_eq!(parse_code_point("65"), Some(65));
        assert_eq!(parse_code_point(""), None);
        assert_eq!(parse_code_point("U+"), None);
        assert_eq!(parse_code_point("U+zz"), None);
        assert_eq!(parse_code_point("U+ffffffffffffffffffff"), None);
        assert_eq!(format_code_point(0xE0B0), "U+e0b0");
        assert_eq!(PRESETS[0], ("Han (CJK Unified Ideographs)", 0x4E00, 0x9FFF));
        assert_eq!(PRESETS.len(), 7);
        assert_eq!(PRESETS[6], ("Private Use Area", 0xE000, 0xF8FF));

        assert_eq!(check_text(&Check::Ascii), Some("Start must be at least U+80. ASCII doesn\u{2019}t support special exceptions."));
        assert_eq!(check_text(&Check::Inverted), Some("Invalid range."));
        assert_eq!(check_text(&Check::LimitTooLarge), Some("End is higher than U+110000, the maximum Unicode code point."));
        assert_eq!(check_text(&Check::Taken), Some("Range includes an already-assigned code point."));
        assert_eq!(check_text(&Check::InvalidDestination), Some("Invalid destination"));
        assert_eq!(check_text(&Check::Incomplete), None);
        assert_eq!(import_missing(&["A".into(), "B".into()]), "You must install the following fonts to use the exceptions in this file:\n\nA\nB");
    }

    #[test]
    fn the_preview_skips_what_iterm2_skips_and_stops_where_it_does() {
        assert_eq!(preview_text(0x41, 3), "ABC");
        // Tab, newline, carriage return, soft hyphen and the zero width marks draw nothing.
        assert_eq!(preview_text(8, 8), "\u{8}\u{b}\u{c}\u{e}\u{f}");
        assert_eq!(preview_text(0xAC, 3), "\u{ac}\u{ae}");
        // Surrogates are not scalars.
        assert_eq!(preview_text(0xD7FF, 3), "\u{d7ff}");
        let long = preview_text(0x4E00, 0x5000);
        assert!(long.ends_with(TRUNCATED) && long.chars().count() <= PREVIEW_LIMIT + TRUNCATED.len());
        assert_eq!(preview_text(0x10FFFF, i64::MAX).chars().count(), 1);
        assert_eq!(row_range_text(&entry(0x41, 26, Some(0x10000), "A", None)), "U+10000 to U+10019");
    }
}

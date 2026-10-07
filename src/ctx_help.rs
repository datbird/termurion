//! The selection helpers of iTerm2's context menu (milestone 13x2, Task 4): conversions of the
//! selected text (`iTermContextMenuUtilities.m`, "CMU"), as titles and what picking one copies.
//! Pure: no web view and no App. The text is a page's, so it is bounded here (`menu_helpers`) and
//! every title is made inert where the menu is built (`browser_ctx::entries`).

use base64::Engine;

/// The most helper items a menu holds (P19).
pub const MAX_ITEMS: usize = 20;
/// The longest Base64-decoded copy offered (P19).
pub const MAX_DECODED: usize = 1 << 20;
/// Smart Selection actions are offered for a selection shorter than this (`CMH:39`).
pub const MAX_CUSTOM_ACTIONS_TEXT: usize = 400;
/// Synonyms, the color and the Base64 preview are offered for a trimmed selection shorter than this.
const MAX_SHORT: usize = 200;
/// Numeric conversions and the Base64 copies are offered for a selection shorter than this.
const MAX_LONG: usize = 1_000_000;

/// What picking a helper copies.
#[derive(Clone, Debug, PartialEq)]
pub enum Copy {
    Text(String),
    Bytes(Vec<u8>),
}

/// One helper item: its title, what it copies (none: the item does nothing), and a color to show as a
/// swatch beside the title.
#[derive(Clone, Debug, PartialEq)]
pub struct Helper {
    pub title: String,
    pub copy: Option<Copy>,
    pub color: Option<(u8, u8, u8)>,
}

fn helper(title: String, copy: Option<Copy>) -> Helper {
    Helper { title, copy, color: None }
}

fn pair(title: String, copy: String) -> Option<Helper> {
    Some(helper(title, Some(Copy::Text(copy))))
}

/// The text a copy puts on the clipboard: bytes decode as UTF-8, losing what is not (`BWV:943-945`).
pub fn copy_text(c: &Copy) -> String {
    match c {
        Copy::Text(t) => t.clone(),
        Copy::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
    }
}

/// `NSNumberFormatterDecimalStyle`'s grouping, "," in threes (Decision: not localized). Takes digits
/// with an optional minus sign and fraction; only the whole part is grouped.
pub fn group_thousands(digits: &str) -> String {
    let (sign, rest) = match digits.strip_prefix('-') {
        Some(r) => ("-", r),
        None => ("", digits),
    };
    let (whole, frac) = match rest.split_once('.') {
        Some((w, f)) => (w, Some(f)),
        None => (rest, None),
    };
    let mut out = String::from(sign);
    for (i, c) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if let Some(f) = frac {
        out.push('.');
        out.push_str(f);
    }
    out
}

/// `SIT:159-185`: bytes under 1024, else IEC prefixes with two decimals rounded down, "\u{2248}" when
/// inexact.
pub fn human_size(value: u64) -> String {
    if value < 1024 {
        return format!("{value} bytes");
    }
    let mut num = value;
    let mut pow = 0;
    let mut exact = true;
    while num >= 1024 * 1024 {
        pow += 1;
        if num % 1024 != 0 {
            exact = false;
        }
        num /= 1024;
    }
    if 100 * num % 1024 != 0 {
        exact = false;
    }
    num = 100 * num / 1024;
    const PREFIXES: [&str; 6] = ["Ki", "Mi", "Gi", "Ti", "Pi", "Ei"];
    format!("{}{}.{:02} {}", if exact { "" } else { "\u{2248}" }, num / 100, num % 100, PREFIXES[pow])
}

/// NSScanner's `scanHexLongLong` from a string: skips white space and one optional "0x", then needs
/// at least one hex digit; stops at the first character that is not one.
fn scan_hex(s: &str) -> Option<u64> {
    let s = s.trim_start();
    let s = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
    let digits: String = s.chars().take_while(char::is_ascii_hexdigit).collect();
    if digits.is_empty() {
        return None;
    }
    // At most 16 digits fit; the caller bounds the length, and NSScanner saturates.
    Some(u64::from_str_radix(&digits, 16).unwrap_or(u64::MAX))
}

/// `CMU:40-165`.
pub fn hex_or_decimal(text: &str) -> Option<Helper> {
    let units = text.encode_utf16().count();
    let (value, dec_to_hex, is32, must_be_positive): (u64, bool, bool, bool);
    if text.starts_with("0x") && units <= 18 {
        value = scan_hex(&text[2..])?;
        dec_to_hex = false;
        is32 = units <= 10;
        must_be_positive = false;
    } else {
        let t = text.trim();
        let (negative, digits) = match t.strip_prefix('-') {
            Some(d) => (true, d),
            None => (false, t),
        };
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) || digits.len() > 40 {
            return None;
        }
        let magnitude: i128 = digits.parse().ok()?;
        let v = if negative { -magnitude } else { magnitude };
        if v > i128::from(u64::MAX) || v < i128::from(i64::MIN) {
            return None;
        }
        dec_to_hex = true;
        must_be_positive = v > 0;
        is32 = if v > 0 { v < 2_147_483_648 } else { v >= -2_147_483_648 };
        value = if v < 0 { v as i64 as u64 } else { v as u64 };
    }
    let size = if value >= 1024 { format!(" ({})", human_size(value)) } else { String::new() };
    if is32 {
        let int = value as u32 as i32;
        let dec = group_thousands(&int.to_string());
        if dec_to_hex {
            let size = if int < 0 { "" } else { size.as_str() };
            let converted = format!("0x{:x}{size}", int as u32);
            pair(format!("{dec} = {converted}"), converted)
        } else if int >= 0 {
            pair(format!("0x{int:x} = {dec}{size}"), format!("{dec}{size}"))
        } else {
            let unsigned = group_thousands(&(int as u32).to_string());
            pair(format!("0x{:x} = {dec} or {unsigned}{size}", int as u32), dec)
        }
    } else {
        let signed = value as i64;
        let negative = !must_be_positive && signed < 0;
        let dec = group_thousands(&if negative { signed.to_string() } else { value.to_string() });
        if dec_to_hex {
            let size = if negative { "" } else { size.as_str() };
            let converted = format!("0x{value:x}{size}");
            pair(format!("{dec} = 0x{value:x}{size}"), converted)
        } else if signed >= 0 {
            pair(format!("0x{value:x} = {dec}{size}"), format!("{dec}{size}"))
        } else {
            let unsigned = group_thousands(&value.to_string());
            pair(format!("0x{value:x} = {dec} or {unsigned}{size}"), dec)
        }
    }
}

/// A decimal number as NSDecimalNumber prints it: at most 38 significant digits and an exponent in
/// -128..=127 (else not a number), no trailing fraction zeros.
fn decimal_text(negative: bool, whole: &str, frac: &str, exp: i64) -> Option<String> {
    let all = format!("{whole}{frac}");
    let trimmed = all.trim_start_matches('0');
    if trimmed.is_empty() {
        return if all.is_empty() { None } else { Some("0".to_string()) };
    }
    let lead = (all.len() - trimmed.len()) as i64;
    let mut digits = trimmed.to_string();
    digits.truncate(38);
    let point = whole.len() as i64 + exp - lead;
    let mantissa_exp = point - digits.len() as i64;
    if !(-128..=127).contains(&mantissa_exp) {
        return None;
    }
    let digits = digits.trim_end_matches('0').to_string();
    let digits = if digits.is_empty() { "0".to_string() } else { digits };
    let n = digits.len() as i64;
    let body = if point <= 0 {
        format!("0.{}{}", "0".repeat((-point) as usize), digits)
    } else if point >= n {
        format!("{}{}", digits, "0".repeat((point - n) as usize))
    } else {
        format!("{}.{}", &digits[..point as usize], &digits[point as usize..])
    };
    Some(format!("{}{}", if negative { "-" } else { "" }, group_thousands(&body)))
}

/// `CMU:167-186`.
pub fn scientific(text: &str) -> Option<Helper> {
    let re = regex::Regex::new(r"^(-?)(0|[1-9][0-9]*)?(\.[0-9]+)?[eE]([+\-]?[0-9]+)$").ok()?;
    let c = re.captures(text)?;
    let exp: i64 = c.get(4)?.as_str().parse().ok()?;
    let frac = c.get(3).map_or("", |m| &m.as_str()[1..]);
    let formatted = decimal_text(!c[1].is_empty(), c.get(2).map_or("", |m| m.as_str()), frac, exp)?;
    pair(format!("{text} = {formatted}"), formatted)
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// Days from 1970-01-01 to a civil date (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date of a day count from 1970-01-01.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// "yyyy MMM d HH:mm:ss[.SSS] zone" (Decision: iTerm2's localized template, fixed).
fn format_date(secs: i64, millis: Option<u32>, tz_offset_minutes: i32, tz_name: &str) -> String {
    let local = secs + i64::from(tz_offset_minutes) * 60;
    let (y, m, d) = civil_from_days(local.div_euclid(86_400));
    let t = local.rem_euclid(86_400);
    let frac = millis.map_or(String::new(), |ms| format!(".{ms:03}"));
    format!("{y} {} {d} {:02}:{:02}:{:02}{frac} {tz_name}", MONTHS[(m - 1) as usize], t / 3600, t % 3600 / 60, t % 60)
}

/// iTerm2's four Unix forms (`CMU:274-305`): whole seconds, milliseconds, microseconds, and seconds
/// with a fraction; each starts with 1. The fraction is in [0, 1).
fn unix_form(text: &str) -> Option<(i64, f64)> {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !text.starts_with('1') {
        return None;
    }
    if let Some((w, f)) = text.split_once('.') {
        return (w.len() == 10 && digits(w) && digits(f)).then(|| (w.parse().unwrap_or(0), format!("0.{f}").parse().unwrap_or(0.0)));
    }
    if !digits(text) {
        return None;
    }
    let n: i64 = text.parse().ok()?;
    match text.len() {
        10 => Some((n, 0.0)),
        13 => Some((n / 1000, (n % 1000) as f64 / 1000.0)),
        16 => Some((n / 1_000_000, (n % 1_000_000) as f64 / 1_000_000.0)),
        _ => None,
    }
}

/// A zone as the RFC forms write it (`zzz`): UTC, GMT, Z, "GMT-05:00", "+0100", or a US zone name.
fn zone_minutes(z: &str) -> Option<i32> {
    let named = match z {
        "UTC" | "GMT" | "UT" | "Z" => Some(0),
        "EST" => Some(-300),
        "EDT" => Some(-240),
        "CST" => Some(-360),
        "CDT" => Some(-300),
        "MST" => Some(-420),
        "MDT" => Some(-360),
        "PST" => Some(-480),
        "PDT" => Some(-420),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    let rest = z.strip_prefix("GMT").unwrap_or(z);
    let (sign, d) = match rest.as_bytes().first()? {
        b'+' => (1, &rest[1..]),
        b'-' => (-1, &rest[1..]),
        _ => return None,
    };
    let d = d.replace(':', "");
    if !d.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (h, m) = match d.len() {
        1 | 2 => (d.parse::<i32>().ok()?, 0),
        4 => (d[..2].parse::<i32>().ok()?, d[2..].parse::<i32>().ok()?),
        _ => return None,
    };
    (h < 24 && m < 60).then_some(sign * (h * 60 + m))
}

/// The date of civil fields as seconds since 1970 in UTC, or None for an impossible date.
fn civil_secs(y: i64, mo: i64, d: i64, h: i64, mi: i64, s: i64) -> Option<i64> {
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let dim = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if !(1..=12).contains(&mo) || d < 1 || d > dim[(mo - 1) as usize] || h > 23 || mi > 59 || s > 59 {
        return None;
    }
    Some(days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + s)
}

/// `CMU:307-332`: the eight UTC forms (RFC 822 and 850-like dates, and ISO with a Z or z), as
/// seconds and milliseconds. A trailing Z is UTC (iTerm2 reads it in the local zone, a bug).
fn utc_form(text: &str) -> Option<(i64, Option<u32>)> {
    let day = "(?:Mon|Tue|Wed|Thu|Fri|Sat|Sun)";
    let month = format!("({})", MONTHS.join("|"));
    let month_of = |s: &str| MONTHS.iter().position(|m| *m == s).map(|i| i as i64 + 1);
    let n = |s: &str| s.parse::<i64>().ok();
    let re1 = regex::Regex::new(&format!(r"^{day}, ([0-9]{{1,2}}) {month} ([0-9]{{4}}) ([0-9]{{2}}):([0-9]{{2}}):([0-9]{{2}}) (\S+)$")).ok()?;
    if let Some(c) = re1.captures(text) {
        let secs = civil_secs(n(&c[3])?, month_of(&c[2])?, n(&c[1])?, n(&c[4])?, n(&c[5])?, n(&c[6])?)?;
        return Some((secs - i64::from(zone_minutes(&c[7])?) * 60, None));
    }
    let re2 = regex::Regex::new(&format!(r"^{day} {month} ([0-9]{{1,2}}) ([0-9]{{2}}):([0-9]{{2}}):([0-9]{{2}}) (\S+) ([0-9]{{4}})$")).ok()?;
    if let Some(c) = re2.captures(text) {
        let secs = civil_secs(n(&c[7])?, month_of(&c[1])?, n(&c[2])?, n(&c[3])?, n(&c[4])?, n(&c[5])?)?;
        return Some((secs - i64::from(zone_minutes(&c[6])?) * 60, None));
    }
    let iso = regex::Regex::new(r"^([0-9]{4})-([0-9]{2})-([0-9]{2})([Tt])([0-9]{2}):([0-9]{2})(?::([0-9]{2})(?:\.([0-9]{3}))?)?([Zz])$").ok()?;
    let c = iso.captures(text)?;
    // The T and the Z agree in case, as iTerm2's formats do; a fraction needs seconds.
    if (&c[4] == "T") != (&c[9] == "Z") {
        return None;
    }
    let secs = civil_secs(n(&c[1])?, n(&c[2])?, n(&c[3])?, n(&c[5])?, n(&c[6])?, c.get(7).map_or(Some(0), |m| n(m.as_str()))?)?;
    Some((secs, c.get(8).and_then(|m| m.as_str().parse().ok())))
}

/// `CMU:188-218`. `tz_offset_minutes` and `tz_name` are the zone the date shows in.
pub fn timestamp(text: &str, tz_offset_minutes: i32, tz_name: &str) -> Option<Helper> {
    if let Some((secs, frac)) = unix_form(text) {
        let millis = (frac > 0.001).then(|| ((frac * 1000.0).round() as u32).min(999));
        let shown = format_date(secs, millis, tz_offset_minutes, tz_name);
        return pair(shown.clone(), shown);
    }
    let (secs, millis) = utc_form(text)?;
    let number = match millis {
        Some(ms) if ms > 0 => format!("{secs}.{ms:03}").trim_end_matches('0').to_string(),
        _ => secs.to_string(),
    };
    pair(format!("Unix timestamp {number}"), number)
}

/// Whether `text` is exactly one extended grapheme cluster (an approximation: a base, then marks,
/// variation selectors, skin tones, tags and ZWJ joined parts; a flag is two regional indicators;
/// CR LF is one).
fn one_grapheme(text: &str) -> bool {
    use unicode_width::UnicodeWidthChar;
    let mut it = text.chars().peekable();
    let Some(first) = it.next() else { return false };
    if first == '\r' {
        return it.next().is_none_or(|c| c == '\n') && it.next().is_none();
    }
    let ri = |c: char| ('\u{1f1e6}'..='\u{1f1ff}').contains(&c);
    if ri(first) {
        if it.peek().is_some_and(|c| ri(*c)) {
            it.next();
        }
    }
    while let Some(c) = it.next() {
        let joins = c == '\u{200d}';
        let extend = matches!(c, '\u{fe00}'..='\u{fe0f}' | '\u{1f3fb}'..='\u{1f3ff}' | '\u{e0020}'..='\u{e007f}' | '\u{200c}') || (!c.is_control() && c.width() == Some(0));
        if joins {
            // A joiner takes the next character with it.
            if it.next().is_none() {
                return true;
            }
        } else if !extend {
            return false;
        }
    }
    true
}

/// `CMU:220-272`.
pub fn utf8_help(text: &str) -> Option<Helper> {
    if text.is_empty() || !one_grapheme(text) {
        return None;
    }
    if text.len() == 1 && text.is_ascii() {
        return None;
    }
    let bytes: Vec<String> = text.bytes().map(|b| format!("0x{b:02x}")).collect();
    let codes: Vec<String> = text.chars().filter(|c| *c != '\u{feff}').map(|c| format!("U+{:04x}", c as u32)).collect();
    let utf8 = bytes.join(" ");
    pair(format!("\u{201c}{text}\u{201d} = {} = {utf8} (UTF-8)", codes.join(" ")), utf8)
}

/// The zone the dates show in: this machine's, as an offset east of UTC in minutes and a short name.
#[cfg(windows)]
pub fn local_zone() -> (i32, String) {
    use windows_sys::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    let mut info: TIME_ZONE_INFORMATION = unsafe { std::mem::zeroed() };
    let id = unsafe { GetTimeZoneInformation(&mut info) };
    let daylight = id == 2;
    let bias = info.Bias + if daylight { info.DaylightBias } else { info.StandardBias };
    let name = if daylight { &info.DaylightName } else { &info.StandardName };
    let end = name.iter().position(|c| *c == 0).unwrap_or(name.len());
    let long = String::from_utf16_lossy(&name[..end]);
    // "Central Daylight Time" is shown as its initials; a name of one word is kept as it is.
    let words: Vec<&str> = long.split_whitespace().collect();
    let short = if words.len() > 1 { words.iter().filter_map(|w| w.chars().next()).collect::<String>() } else { long.clone() };
    (-bias, crate::inert_files::display_text(&short, 16, false))
}

#[cfg(not(windows))]
pub fn local_zone() -> (i32, String) {
    (0, "UTC".to_string())
}

/// `CMU:15-38`: hex or decimal, scientific notation, timestamp, UTF-8, in that order, with dates in
/// the given zone.
pub fn synonyms_in(text: &str, tz_offset_minutes: i32, tz_name: &str) -> Vec<Helper> {
    [hex_or_decimal(text), scientific(text), timestamp(text, tz_offset_minutes, tz_name), utf8_help(text)].into_iter().flatten().collect()
}

/// `synonyms_in` in this machine's zone.
pub fn synonyms(text: &str) -> Vec<Helper> {
    let (offset, name) = local_zone();
    synonyms_in(text, offset, &name)
}

/// `CMU:336-363`: `#rrggbb` only; the title is the text (a menu item needs a label).
pub fn color(text: &str) -> Option<Helper> {
    let hex = text.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let part = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some(Helper { title: text.to_string(), copy: None, color: Some((part(0)?, part(2)?, part(4)?)) })
}

/// `NSS:1005-1013`: only Base64 characters, or only URL-safe ones, white space allowed.
pub fn may_be_base64(text: &str) -> bool {
    let standard = |c: char| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=') || c.is_whitespace();
    let url_safe = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '=') || c.is_whitespace();
    text.chars().all(standard) || text.chars().all(url_safe)
}

/// Standard Base64, else URL-safe, with padding required and line breaks and white space ignored
/// (Decision: iTerm2's decoder takes an unpadded run, which would offer a decode of any word).
fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.is_empty() {
        return None;
    }
    base64::engine::general_purpose::STANDARD.decode(&compact).or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(&compact)).ok().filter(|b| !b.is_empty())
}

/// `NSD:385-421`: UTF-8 text with control characters escaped, unless it is all white space or not
/// UTF-8, then hex bytes.
pub fn friendly(bytes: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(bytes) {
        if s.chars().any(|c| !c.is_whitespace()) {
            let mut out = String::new();
            for c in s.chars() {
                match c {
                    '\r' => out.push_str("\\r"),
                    '\n' => out.push_str("\\n"),
                    '\t' => out.push_str("\\t"),
                    c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02X}", c as u32)),
                    c => out.push(c),
                }
            }
            return out;
        }
    }
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

/// `NSS:1640-1665`: line breaks shown as the return arrow, cut to `max` characters with an ellipsis.
pub fn ellipsize(text: &str, max: usize) -> String {
    let flat: Vec<char> = text.chars().map(|c| if c == '\r' || c == '\n' { '\u{21a9}' } else { c }).collect();
    let blank = |c: &char| matches!(*c, ' ' | '\t' | '\u{a0}') || (c.is_whitespace() && !matches!(*c, '\r' | '\n'));
    let trim = |v: &[char]| -> String {
        let a = v.iter().position(|c| !blank(c)).unwrap_or(v.len());
        let b = v.iter().rposition(|c| !blank(c)).map_or(a, |i| i + 1);
        v[a..b].iter().collect()
    };
    if flat.len() <= max {
        return trim(&flat);
    }
    let Some(first) = flat.iter().position(|c| !c.is_whitespace()) else { return String::new() };
    let length = flat.len() - first;
    let (prefix, truncated) = if length < max { (&flat[first..], false) } else { (&flat[first..first + max - 1], true) };
    let mut out = trim(prefix);
    if truncated {
        out.push('\u{2026}');
    }
    out
}

/// `NSS:674-688`: whitespace-separated groups of hex digits, each of an even length, as bytes.
pub fn hex_bytes(text: &str) -> Option<Vec<u8>> {
    if text.is_empty() || !text.chars().all(|c| c.is_ascii_hexdigit() || c.is_whitespace()) {
        return None;
    }
    let mut out = Vec::new();
    for group in text.split(char::is_whitespace) {
        if group.len() % 2 != 0 {
            return None;
        }
        for pair in group.as_bytes().chunks(2) {
            out.push(u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?);
        }
    }
    Some(out)
}

/// `CMU:365-383`.
pub fn base64_preview(text: &str) -> Option<Helper> {
    if text.chars().count() <= 3 || !may_be_base64(text) {
        return None;
    }
    let decoded = decode_base64(text)?;
    Some(helper(format!("Base64: {}", ellipsize(&friendly(&decoded), 20)), Some(Copy::Bytes(decoded))))
}

/// `CMU:421-481`.
pub fn numeric_conversions(text: &str) -> Vec<Helper> {
    let Some(data) = hex_bytes(text) else { return Vec::new() };
    if data.len() <= 1 {
        return Vec::new();
    }
    let num = |t: String, v: String| helper(t, Some(Copy::Text(v)));
    let mut out = Vec::new();
    match data.len() {
        4 => {
            let b: [u8; 4] = data[..4].try_into().unwrap_or([0; 4]);
            let (be, le) = (u32::from_be_bytes(b), u32::from_le_bytes(b));
            out.push(num(format!("Big-Endian int32: {be}"), be.to_string()));
            out.push(num(format!("Little-Endian int32: {le}"), le.to_string()));
        }
        8 => {
            let b: [u8; 8] = data[..8].try_into().unwrap_or([0; 8]);
            let (be, le) = (u64::from_be_bytes(b), u64::from_le_bytes(b));
            out.push(num(format!("Big-Endian int64: {be}"), be.to_string()));
            out.push(num(format!("Little-Endian int64: {le}"), le.to_string()));
        }
        n if n < 100 => {
            if let Ok(s) = String::from_utf8(data.clone()) {
                out.push(num(format!("{n} UTF-8 bytes: {s}"), s));
            }
        }
        _ => {}
    }
    if out.is_empty() && data.len() > 4 {
        out.push(helper(format!("{} hex bytes", data.len()), None));
    }
    out
}

/// `CMU:483-515`: decoded when it decodes (at most `MAX_DECODED`), and always the encoded text of
/// the unstripped selection in 76-character lines separated by CR LF.
pub fn copy_base64(unstripped: &str) -> Vec<Helper> {
    let mut out = Vec::new();
    if may_be_base64(unstripped) {
        if let Some(decoded) = decode_base64(unstripped).filter(|d| d.len() <= MAX_DECODED) {
            out.push(helper("Copy Base64-Decoded".to_string(), Some(Copy::Bytes(decoded))));
        }
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(unstripped.as_bytes());
    let lines: Vec<&str> = encoded.as_bytes().chunks(76).filter_map(|c| std::str::from_utf8(c).ok()).collect();
    out.push(helper("Copy Base64-Encoded".to_string(), Some(Copy::Text(lines.join("\r\n")))));
    out
}

/// `BCM:72-99`: what the menu shows for a selection (as the page gave it, white space and all), in
/// `tz_offset_minutes` and `tz_name` for dates. At most `MAX_ITEMS` items.
pub fn menu_helpers_in(unstripped: &str, tz_offset_minutes: i32, tz_name: &str) -> Vec<Helper> {
    let mut out = Vec::new();
    if unstripped.is_empty() {
        return out;
    }
    let trimmed = unstripped.trim();
    let count = trimmed.chars().count();
    if count < MAX_SHORT {
        out.extend(synonyms_in(trimmed, tz_offset_minutes, tz_name));
        out.extend(color(trimmed));
        out.extend(base64_preview(trimmed));
    }
    if count < MAX_LONG {
        out.extend(numeric_conversions(trimmed));
        out.extend(copy_base64(unstripped));
    }
    out.truncate(MAX_ITEMS);
    out
}

/// `menu_helpers_in` in this machine's zone.
pub fn menu_helpers(unstripped: &str) -> Vec<Helper> {
    let (offset, name) = local_zone();
    menu_helpers_in(unstripped, offset, &name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn help(h: Option<Helper>) -> (String, Option<Copy>) {
        let h = h.expect("a helper");
        (h.title, h.copy)
    }
    fn text(s: &str) -> Option<Copy> {
        Some(Copy::Text(s.to_string()))
    }

    /// `CMU:15-38` and every conversion's text (Review Focus 3).
    #[test]
    fn synonyms_follow_iterm2() {
        assert_eq!(help(hex_or_decimal("255")), ("255 = 0xff".into(), text("0xff")));
        assert_eq!(help(hex_or_decimal("0x400")), ("0x400 = 1,024 (1.00 Ki)".into(), text("1,024 (1.00 Ki)")));
        // The size suffix is only for 1024 and up.
        assert_eq!(help(hex_or_decimal("1023")), ("1,023 = 0x3ff".into(), text("0x3ff")));
        assert_eq!(help(hex_or_decimal("0xff")), ("0xff = 255".into(), text("255")));
        // The brief says -1 is accepted (`CMU:111-119` handles a negative); the source's isNumeric
        // would refuse the minus sign. The brief wins here (recorded).
        assert_eq!(help(hex_or_decimal("-1")), ("-1 = 0xffffffff".into(), text("0xffffffff")));
        // 0xffffffff is 4294967295: 3.99 Gi by SIT:159-185's arithmetic (the brief said 4.00).
        assert_eq!(help(hex_or_decimal("0xffffffff")), ("0xffffffff = -1 or 4,294,967,295 (\u{2248}3.99 Gi)".into(), text("-1")));
        assert_eq!(help(hex_or_decimal("4294967296")), ("4,294,967,296 = 0x100000000 (4.00 Gi)".into(), text("0x100000000 (4.00 Gi)")));
        assert_eq!(help(hex_or_decimal("0x100000000")), ("0x100000000 = 4,294,967,296 (4.00 Gi)".into(), text("4,294,967,296 (4.00 Gi)")));
        assert_eq!(help(hex_or_decimal("0xffffffffffffffff")), ("0xffffffffffffffff = -1 or 18,446,744,073,709,551,615 (\u{2248}15.99 Ei)".into(), text("-1")));
        assert_eq!(help(hex_or_decimal("-4294967296")).0, "-4,294,967,296 = 0xffffffff00000000");
        assert_eq!(hex_or_decimal("18446744073709551616"), None);
        assert_eq!(hex_or_decimal("-9223372036854775809"), None);
        assert_eq!(hex_or_decimal("0x"), None);
        assert_eq!(hex_or_decimal("12a"), None);
        assert_eq!(hex_or_decimal("0x12345678901234567"), None, "past 18 characters is not hex");
        assert_eq!(hex_or_decimal(""), None);
        assert_eq!(hex_or_decimal("1.5"), None);

        assert_eq!(help(scientific("1e3")), ("1e3 = 1,000".into(), text("1,000")));
        assert_eq!(help(scientific("1.5E-2")), ("1.5E-2 = 0.015".into(), text("0.015")));
        assert_eq!(help(scientific("-2.5e2")), ("-2.5e2 = -250".into(), text("-250")));
        assert_eq!(scientific("e3"), None);
        assert_eq!(scientific("1e"), None);
        assert_eq!(scientific("12"), None);
        assert_eq!(scientific("1e999"), None, "past NSDecimalNumber's exponent range");

        assert_eq!(help(timestamp("1700000000", 0, "UTC")), ("2023 Nov 14 22:13:20 UTC".into(), text("2023 Nov 14 22:13:20 UTC")));
        assert!(help(timestamp("1700000000123", 0, "UTC")).0.contains(".123"));
        assert_eq!(help(timestamp("1700000000123", 0, "UTC")).0, "2023 Nov 14 22:13:20.123 UTC");
        assert_eq!(help(timestamp("1700000000", -300, "CDT")).0, "2023 Nov 14 17:13:20 CDT");
        assert_eq!(help(timestamp("1700000000.5", 0, "UTC")).0, "2023 Nov 14 22:13:20.500 UTC");
        assert_eq!(help(timestamp("1700000000000123", 0, "UTC")).0, "2023 Nov 14 22:13:20 UTC", "microseconds under a millisecond show none");
        assert_eq!(help(timestamp("2023-11-14T22:13:20Z", 0, "UTC")), ("Unix timestamp 1700000000".into(), text("1700000000")));
        // A trailing Z is UTC whatever the local zone (iTerm2 reads it as local time, a bug; recorded).
        assert_eq!(help(timestamp("2023-11-14T22:13:20Z", -300, "CDT")).1, text("1700000000"));
        assert_eq!(help(timestamp("2023-11-14T22:13:20.250Z", 0, "UTC")), ("Unix timestamp 1700000000.25".into(), text("1700000000.25")));
        assert_eq!(help(timestamp("2023-11-14t22:13z", 0, "UTC")).1, text("1699999980"));
        assert_eq!(help(timestamp("Tue, 14 Nov 2023 22:13:20 GMT", 0, "UTC")).1, text("1700000000"));
        assert_eq!(help(timestamp("Tue Nov 14 17:13:20 -0500 2023", 0, "UTC")).1, text("1700000000"));
        assert_eq!(timestamp("2700000000", 0, "UTC"), None, "the ^1 rule");
        assert_eq!(timestamp("1700000000x", 0, "UTC"), None);
        assert_eq!(timestamp("2023-13-45T99:99:99Z", 0, "UTC"), None);
        assert_eq!(timestamp("hello", 0, "UTC"), None);

        assert_eq!(help(utf8_help("\u{e9}")), ("\u{201c}\u{e9}\u{201d} = U+00e9 = 0xc3 0xa9 (UTF-8)".into(), text("0xc3 0xa9")));
        assert_eq!(utf8_help("a"), None);
        assert_eq!(utf8_help("ab"), None);
        assert_eq!(utf8_help(""), None);
        // One grapheme of several scalars: a letter and a combining accent, a ZWJ family, a flag.
        assert_eq!(help(utf8_help("e\u{301}")).0, "\u{201c}e\u{301}\u{201d} = U+0065 U+0301 = 0x65 0xcc 0x81 (UTF-8)");
        assert!(utf8_help("\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}").is_some());
        assert!(utf8_help("\u{1f1fa}\u{1f1f8}").is_some());
        assert_eq!(utf8_help("\u{1f1fa}\u{1f1f8}\u{1f1fa}"), None);
        // A byte order mark is left out of the code points (it is one cluster with a letter before it).
        assert_eq!(help(utf8_help("e\u{feff}")).0, "\u{201c}e\u{feff}\u{201d} = U+0065 = 0x65 0xef 0xbb 0xbf (UTF-8)");
        assert_eq!(utf8_help("\u{feff}\u{e9}"), None);

        let titles: Vec<String> = synonyms_in("255", 0, "UTC").into_iter().map(|h| h.title).collect();
        assert_eq!(titles, vec!["255 = 0xff".to_string()]);
        assert_eq!(synonyms_in("\u{e9}", 0, "UTC").len(), 1);
        assert!(synonyms_in("hello", 0, "UTC").is_empty());
        // The order: hex or decimal, scientific notation, timestamp, UTF-8.
        let both = synonyms_in("1700000000", 0, "UTC");
        assert_eq!(both.len(), 2);
        assert!(both[0].title.starts_with("1,700,000,000 = 0x"));
        assert_eq!(both[1].title, "2023 Nov 14 22:13:20 UTC");
    }

    #[test]
    fn human_size_follows_iterm2() {
        assert_eq!(human_size(0), "0 bytes");
        assert_eq!(human_size(1023), "1023 bytes");
        assert_eq!(human_size(1024), "1.00 Ki");
        assert_eq!(human_size(1536), "1.50 Ki");
        assert_eq!(human_size(1025), "\u{2248}1.00 Ki");
        assert_eq!(human_size(1048576), "1.00 Mi");
        assert_eq!(human_size(1048577), "\u{2248}1.00 Mi");
        assert_eq!(human_size(u64::MAX), "\u{2248}15.99 Ei");
        assert_eq!(group_thousands("1234567"), "1,234,567");
        assert_eq!(group_thousands("-1234"), "-1,234");
        assert_eq!(group_thousands("123"), "123");
        assert_eq!(group_thousands("1234.5678"), "1,234.5678");
    }

    #[test]
    fn numeric_conversions_follow_iterm2() {
        let pair = |v: Vec<Helper>| v.into_iter().map(|h| (h.title, h.copy)).collect::<Vec<_>>();
        assert_eq!(pair(numeric_conversions("00 00 00 01")), vec![("Big-Endian int32: 1".into(), text("1")), ("Little-Endian int32: 16777216".into(), text("16777216"))]);
        assert_eq!(pair(numeric_conversions("0000 0000 0000 0001")), vec![("Big-Endian int64: 1".into(), text("1")), ("Little-Endian int64: 72057594037927936".into(), text("72057594037927936"))]);
        assert_eq!(pair(numeric_conversions("68 69")), vec![("2 UTF-8 bytes: hi".into(), text("hi"))]);
        assert_eq!(pair(numeric_conversions("ff ff ff ff ff")), vec![("5 hex bytes".into(), None)]);
        assert!(numeric_conversions("0").is_empty(), "an odd group");
        assert!(numeric_conversions("zz").is_empty());
        assert!(numeric_conversions("41").is_empty(), "one byte is not enough (CMU:425)");
        assert!(numeric_conversions("ff fe").is_empty(), "two bytes that are not UTF-8 and not over four");
        assert!(numeric_conversions("").is_empty());
        assert_eq!(hex_bytes("de ad\nbe ef"), Some(vec![0xde, 0xad, 0xbe, 0xef]));
        assert_eq!(hex_bytes("dead beef0 1"), None);
        // Over 99 bytes that are text: no UTF-8 item, but a hex count.
        let long = vec!["41"; 120].join(" ");
        assert_eq!(pair(numeric_conversions(&long)), vec![("120 hex bytes".into(), None)]);
    }

    #[test]
    fn base64_items_follow_iterm2() {
        assert_eq!(help(base64_preview("aGVsbG8=")), ("Base64: hello".into(), Some(Copy::Bytes(b"hello".to_vec()))));
        assert_eq!(base64_preview("abc"), None, "the length must exceed 3 (CMU:366)");
        assert_eq!(help(base64_preview("aGk_")).1, Some(Copy::Bytes(b"hi?".to_vec())));
        assert_eq!(base64_preview("not base64!"), None);
        // The preview is the friendly form, cut at 20 characters.
        let long = crate::seltools::base64_encode(&"x".repeat(60));
        assert_eq!(help(base64_preview(&long)).0, format!("Base64: {}\u{2026}", "x".repeat(19)));
        // Not text: hex bytes.
        assert_eq!(help(base64_preview("//79/A==")).0, "Base64: FF FE FD FC");

        let titles = |v: Vec<Helper>| v.into_iter().map(|h| h.title).collect::<Vec<_>>();
        assert_eq!(titles(copy_base64("hi")), vec!["Copy Base64-Encoded".to_string()]);
        assert_eq!(copy_base64("hi")[0].copy, text("aGk="));
        assert_eq!(titles(copy_base64("aGk=")), vec!["Copy Base64-Decoded".to_string(), "Copy Base64-Encoded".to_string()]);
        assert_eq!(copy_base64("aGk=")[0].copy, Some(Copy::Bytes(b"hi".to_vec())));
        // The encoded copy wraps at 76 characters with CR LF.
        let enc = copy_base64(&"a".repeat(100));
        let Some(Copy::Text(t)) = &enc.last().unwrap().copy else { panic!("text") };
        let lines: Vec<&str> = t.split("\r\n").collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), 76);
        assert_eq!(lines[1].len(), 60, "the rest");

        assert_eq!(help(color("#ff8000")).0, "#ff8000");
        assert_eq!(color("#ff8000").unwrap().color, Some((255, 128, 0)));
        assert_eq!(color("#FF8000").unwrap().color, Some((255, 128, 0)));
        assert_eq!(color("#ff80"), None);
        assert_eq!(color("ff8000"), None);
        assert_eq!(color("#ff80000"), None);
        assert_eq!(color("#gg8000"), None);
    }

    #[test]
    fn text_tools_follow_iterm2() {
        assert_eq!(ellipsize("a\nb", 20), "a\u{21a9}b");
        assert_eq!(ellipsize("a\r\nb", 20), "a\u{21a9}\u{21a9}b");
        let t = "abcdefghijklmnopqrstuvwxyz0123";
        assert_eq!(ellipsize(t, 20), format!("{}\u{2026}", &t[..19]));
        assert_eq!(ellipsize("  hi  ", 20), "hi");
        assert_eq!(ellipsize("       ", 3), "");
        assert!(may_be_base64("aGk="));
        assert!(may_be_base64("a-k_"));
        assert!(!may_be_base64("a+k_"), "mixed alphabets");
        assert!(!may_be_base64("a b!"));
        assert!(may_be_base64(""));
        assert_eq!(friendly(b"a\r\n\tb"), "a\\r\\n\\tb");
        assert_eq!(friendly(b"a\x01"), "a\\x01");
        assert_eq!(friendly(&[0xff, 0x00]), "FF 00");
        assert_eq!(friendly(b"  \n"), "20 20 0A", "all white space is shown as hex");
    }

    /// `BCM:72-99`: 200 and 1,000,000 are the limits.
    #[test]
    fn menu_helpers_follow_iterm2() {
        let titles = |s: &str| menu_helpers(s).into_iter().map(|h| h.title).collect::<Vec<_>>();
        let t = titles(" 255 ");
        assert_eq!(t[0], "255 = 0xff");
        assert!(t.contains(&"Copy Base64-Encoded".to_string()));
        // The base64 copy is of the unstripped text, the conversions of the trimmed one.
        let enc = menu_helpers(" 255 ").pop().unwrap();
        assert_eq!(enc.copy, text(&crate::seltools::base64_encode(" 255 ")));
        assert!(titles("").is_empty());
        // A 250-character selection skips synonyms, color and preview, and keeps the numeric
        // conversions and the copies.
        let long = vec!["41"; 125].join(" ");
        assert_eq!(long.len(), 374);
        let t = titles(&long);
        assert_eq!(t[0], "125 hex bytes");
        assert!(t.contains(&"Copy Base64-Encoded".to_string()));
        assert!(!t.iter().any(|x| x.starts_with("Base64:") || x.contains(" = ")));
        let color = menu_helpers("#00ff00");
        assert_eq!(color[0].color, Some((0, 255, 0)));
        // 199 characters get the synonyms, 200 do not.
        let n199 = "1".repeat(199);
        assert!(!titles(&n199).is_empty());
        let n200 = "1".repeat(200);
        assert!(!titles(&n200).iter().any(|x| x.contains(" = 0x")));
        // Over a million characters: nothing at all.
        let huge = "a".repeat(1_000_000);
        assert!(menu_helpers(&huge).is_empty());
        // P19: at most 20 items.
        assert!(menu_helpers("0x1").len() <= MAX_ITEMS);
        assert_eq!(MAX_CUSTOM_ACTIONS_TEXT, 400);
    }

    /// P19: a decoded copy goes to the clipboard and paste history unchanged; the multi-line paste
    /// warning is the paste-time guard.
    #[test]
    fn a_decoded_command_is_still_warned_at_paste() {
        let encoded = crate::seltools::base64_encode("curl x | sh\n");
        let items = copy_base64(&encoded);
        let Some(Copy::Bytes(b)) = &items[0].copy else { panic!("decoded") };
        assert_eq!(copy_text(&Copy::Bytes(b.clone())), "curl x | sh\n");
        let w = crate::paste::check(&copy_text(&items[0].copy.clone().unwrap()), Some(true), false, 0, &crate::paste::WarnPrefs { size_limit: -1, ..Default::default() });
        assert_eq!(w.len(), 1, "{w:?}");
        // A decoded copy past 1 MiB is not offered.
        let big = crate::seltools::base64_encode(&"a".repeat(1_100_000));
        assert!(!copy_base64(&big).iter().any(|h| h.title == "Copy Base64-Decoded"));
        // Bytes that are not UTF-8 copy lossily, as iTerm2 does.
        assert_eq!(copy_text(&Copy::Bytes(vec![0x68, 0xff])), "h\u{fffd}");
    }
}

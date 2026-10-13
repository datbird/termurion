//! iTerm2's status bar text machinery (milestone 9h, Task 2): every content component's
//! width variants (narrowest to widest is not the order; iTerm2's own construction order
//! is kept, since only width matters for `pick`), the widest-fitting pick, localhost
//! detection, and a TR35 date pattern formatter. Pure text: no Win32, no `Renderer`. Task 3
//! measures each variant's width with `Renderer::text_width` and calls `pick`.
//!
//! Citations use `.superpowers/research/m9-facts.md` section 2's abbreviations (`VAR` =
//! `iTermStatusBarVariableBaseComponent.m`, `ACT` = `iTermStatusBarActionComponent.m`,
//! `JOB` = `iTermStatusBarJobComponent.m`, `TXT` = `iTermStatusBarTextComponent.m`) plus
//! `NSStringITerm.m` (`byTruncatingComposedCharactersInCenter:`, cited as `STR`) and
//! `NSHost+iTerm.m` (`it_hostnameIsThisMachine:`, cited as `HOST`). Where this file's
//! behavior disagrees with the plan's prose, the source (re-read directly for this task)
//! wins per the implementer rules; see the report for the one place that happened
//! (`hostname_variants_follow_iterm2`, a one-label name longer than one character).

/// The widest variant whose width fits `available`, both rounded up first so a variant
/// exactly as wide as a fractional `available` still counts as fitting (`TXT:178-229`:
/// "draw = widest with `ceil(w) <= ceil(available)`, else empty"). `""` when none fit. On a
/// tie, the FIRST widest-fitting variant wins, matching `maxWithBlock:`'s own tie-break
/// (`NSArray+iTerm.m:531-544`: it replaces its running max only on a strictly-greater
/// comparison, so an equal later element never displaces an earlier one); `Iterator::
/// max_by` does the opposite (keeps the LAST maximum on a tie), so this is a manual scan
/// rather than that combinator.
pub fn pick(widths: &[(String, f32)], available: f32) -> String {
    let available_ceil = available.ceil();
    let mut best: Option<&(String, f32)> = None;
    for entry in widths.iter().filter(|(_, w)| w.ceil() <= available_ceil) {
        match best {
            Some(b) if entry.1 <= b.1 => {}
            _ => best = Some(entry),
        }
    }
    best.map(|(s, _)| s.clone()).unwrap_or_default()
}

/// The narrowest variant's width (`TXT:178-229`: "minimum = narrowest's"). `f32::INFINITY`
/// for no variants; the caller never has an empty list for a component that draws at all.
pub fn narrowest(widths: &[(String, f32)]) -> f32 {
    widths.iter().map(|(_, w)| *w).fold(f32::INFINITY, f32::min)
}

/// The widest variant's width (`TXT:178-229`: "preferred = widest variant's width").
pub fn widest(widths: &[(String, f32)]) -> f32 {
    widths.iter().map(|(_, w)| *w).fold(f32::NEG_INFINITY, f32::max)
}

/// `byTruncatingComposedCharactersInCenter:` (`STR:1876-1890`). `numberToOmit` composed
/// characters removed from the center, replaced with a single U+2026; `None` when `s` is
/// too short to omit that many and still leave two characters on each side of the ellipsis
/// (`length < numberToOmit + 2`). Termurion uses `char`s where iTerm2 uses composed characters
/// (grapheme clusters); every test string here is single-scalar ASCII, so they agree.
fn truncate_center(s: &str, number_to_omit: usize) -> Option<String> {
    let chars: Vec<char> = s.chars().collect();
    let length = chars.len();
    if length < number_to_omit + 2 {
        return None;
    }
    let prefix_len = (length - number_to_omit) / 2;
    let suffix_len = length - number_to_omit - prefix_len;
    let prefix: String = chars[..prefix_len].iter().collect();
    let suffix: String = chars[length - suffix_len..].iter().collect();
    Some(format!("{prefix}\u{2026}{suffix}"))
}

/// The shared `variantsOf:` (`VAR:102-125`), common to every `iTermStatusBarVariableBase
/// Component` subclass (Host Name, User Name via its own override, Current Directory).
/// `compress` is that subclass's `stringByCompressingString:`; the base class's own
/// returns `nil` unconditionally, so a caller with no compression (User Name) never
/// reaches this function at all (it hardcodes its own `stringVariants`, see `user_min_text`
/// below).
///
/// Kept bug (9c ruling): the center-truncate section always splits the WHOLE string on
/// `.`, never on the caller's own separator, so a Directory path with a literal `.` in it
/// gets Host-style center forms of everything before that dot, path separators and all.
fn variants_of(s: &str, compress: impl Fn(&str) -> Option<String>) -> Vec<String> {
    if s.is_empty() {
        return Vec::new();
    }
    let mut result = Vec::new();

    let dot_parts: Vec<&str> = s.split('.').collect();
    if dot_parts.len() > 1 {
        let localname = dot_parts[0];
        let local_len = localname.chars().count();
        if local_len > 1 {
            let mut n = 2usize;
            while n < local_len.saturating_sub(2) {
                if let Some(t) = truncate_center(localname, n) {
                    result.push(t);
                }
                n += 1;
            }
            result.push(localname.to_string());
        }
    }

    let mut current: Option<String> = Some(s.to_string());
    let mut prev_len: Option<usize> = None;
    loop {
        let keep_going = match (&current, prev_len) {
            (None, _) => false,
            (Some(_), None) => true,
            (Some(c), Some(p)) => c.chars().count() < p,
        };
        if !keep_going {
            break;
        }
        let c = current.take().unwrap();
        prev_len = Some(c.chars().count());
        let next = compress(&c);
        result.push(c);
        current = next;
    }
    result
}

/// `iTermStatusBarHostnameComponent`'s `stringByCompressingString:` (`VAR:~300-315`):
/// splits on `.`, finds the LAST label longer than one character (searching from the end,
/// `NSEnumerationReverse`), and replaces it with just its first character (no ellipsis).
fn host_compress(source: &str) -> Option<String> {
    let parts: Vec<&str> = source.split('.').collect();
    for idx in (0..parts.len()).rev() {
        let part = parts[idx];
        let Some(first) = part.chars().next() else { continue };
        let first_str = first.to_string();
        if first_str != part {
            let mut owned: Vec<String> = parts.iter().map(|p| p.to_string()).collect();
            owned[idx] = first_str;
            return Some(owned.join("."));
        }
    }
    None
}

/// Host Name's width variants (`iTermStatusBarHostnameComponent`, `VAR:~250-320`): the
/// shared center-truncate-then-compress ladder over the raw hostname. The `abbreviate-
/// localhost` knob is a straight one-variant override on top of this and is not part of
/// this pure module (it needs `is_local_host`'s live-machine answer, which the caller has
/// and this module does not).
pub fn host_variants(name: &str) -> Vec<String> {
    variants_of(name, host_compress)
}

/// `iTermStatusBarWorkingDirectoryComponent`'s `stringByCompressingString:` (`VAR:~330-
/// 345`): splits on the path separator, finds the FIRST component longer than one
/// character (forward search, unlike Host's reverse one), and replaces it with just its
/// first character. iTerm2 always splits on `/`; Termurion's paths are backslash, so this
/// splits on `\` when the string has one and falls back to `/` otherwise (a raw display
/// string built from a forward-slash path, e.g. one already using `/`, still compresses).
fn dir_compress(source: &str) -> Option<String> {
    let sep = if source.contains('\\') { '\\' } else { '/' };
    let parts: Vec<&str> = source.split(sep).collect();
    for (idx, part) in parts.iter().enumerate() {
        let Some(first) = part.chars().next() else { continue };
        let first_str = first.to_string();
        if first_str != *part {
            let mut owned: Vec<String> = parts.iter().map(|p| p.to_string()).collect();
            owned[idx] = first_str;
            return Some(owned.join(&sep.to_string()));
        }
    }
    None
}

/// Current Directory's width variants over an already home-substituted display string
/// (`display_dir`'s output). See `variants_of` for the kept dot-splitting bug shared with
/// Host Name.
pub fn dir_variants(display: &str) -> Vec<String> {
    variants_of(display, dir_compress)
}

/// `transformedValue:` (`VAR:~250`): a leading `home` prefix becomes `~`, a plain string
/// prefix so a remote path under the same-looking prefix is abbreviated too. Windows adds
/// a case-insensitive compare (the resume note's correction over the research; macOS paths
/// are case-sensitive, `%USERPROFILE%` is not) so a drive-letter or user-name case
/// mismatch still abbreviates. The replacement keeps the raw suffix's own casing.
///
/// Compares by character, not by byte length: `home.len()` in bytes is not necessarily a
/// char boundary in `raw` (a `char::to_lowercase()` case pair can differ in UTF-8 width,
/// e.g. `\u{1e9e}` (3 bytes) folding to `\u{df}` (2 bytes)), so slicing `raw` at that byte
/// offset can land mid-character and panic. Each `home` character is matched against the
/// corresponding `raw` character by comparing their own lowercased forms (which may
/// themselves differ in length without upsetting the pairing), and the cut point in `raw`
/// is found through `char_indices`, which only ever returns character-boundary offsets.
pub fn display_dir(raw: &str, home: &str) -> String {
    if home.is_empty() {
        return raw.to_string();
    }
    let fold = |c: char| -> String {
        let c = if c == '/' { '\\' } else { c };
        c.to_lowercase().collect()
    };
    let home_chars: Vec<char> = home.chars().collect();
    let raw_chars: Vec<char> = raw.chars().collect();
    if raw_chars.len() < home_chars.len() {
        return raw.to_string();
    }
    let prefix_matches = raw_chars[..home_chars.len()].iter().zip(home_chars.iter()).all(|(&r, &h)| fold(r) == fold(h));
    if !prefix_matches {
        return raw.to_string();
    }
    let cut = raw.char_indices().nth(home_chars.len()).map(|(i, _)| i).unwrap_or(raw.len());
    format!("~{}", &raw[cut..])
}

/// `iTermStatusBarJobComponent.stringVariants` and `variantsOfChain:` (`JOB:85-149`):
/// deepest job first, then each ancestor added on the right, joined with `" \u{25C2} "`.
/// `chain` is empty before the session has reported a job at all (`_cached == nil`,
/// `JOB:88-91`: "don't show a placeholder prior to the initial value"), which this module
/// represents as a single empty-string variant so the component still has something to
/// measure and never draws a placeholder. A non-empty chain whose entries are all empty
/// names is the "process cache raced the pid" case (`JOB:92-99`): a single U+2026 stands
/// in so the component is not hidden.
pub fn job_variants(chain: &[String]) -> Vec<String> {
    if chain.is_empty() {
        return vec![String::new()];
    }
    if chain.iter().all(|s| s.is_empty()) {
        return vec!["\u{2026}".to_string()];
    }
    (0..chain.len()).map(|i| chain[..=i].join(" \u{25C2} ")).collect()
}

/// Custom Action's width variants (`ACT:24-40`). The research read RegexKitLite's
/// `enumerateStringsSeparatedByRegex` as visiting only the FULL title when it ends in
/// whitespace; re-reading the source, it visits every word between runs of horizontal
/// whitespace (`\h+`) and the block appends the title up through the end of each word in
/// turn, so the variants are every word prefix, ending with the full title regardless of
/// trailing whitespace. Horizontal whitespace here is space and tab (`\h` excludes the
/// line breaks `\v` covers).
pub fn action_variants(title: &str) -> Vec<String> {
    let chars: Vec<char> = title.chars().collect();
    let mut result = Vec::new();
    let mut in_word = false;
    for (idx, &c) in chars.iter().enumerate() {
        if c == ' ' || c == '\t' {
            if in_word {
                result.push(chars[..idx].iter().collect());
                in_word = false;
            }
        } else {
            in_word = true;
        }
    }
    if in_word {
        result.push(chars.iter().collect());
    }
    result
}

/// The tmux left status component's width variants (`iTermTmuxStatusBarMonitor.m`,
/// `iTermStatusBarLayout+tmux.m`; Resume notes): `value` is tmux's already-sanitized
/// `#{T:status-left}` text with its `" | "`-joined segments; each variant drops one more
/// trailing segment, narrowest last.
pub fn tmux_left_variants(value: &str) -> Vec<String> {
    let parts: Vec<&str> = value.split(" | ").collect();
    (0..parts.len()).map(|drop| parts[..parts.len() - drop].join(" | ")).collect()
}

/// The tmux right status component's width variants: the mirror of `tmux_left_variants`,
/// dropping leading segments instead of trailing ones (the caller right-aligns the result;
/// this module only produces the candidate strings).
pub fn tmux_right_variants(value: &str) -> Vec<String> {
    let parts: Vec<&str> = value.split(" | ").collect();
    (0..parts.len()).map(|start| parts[start..].join(" | ")).collect()
}

/// User Name's minimum-width candidate (`VAR:234-247`: "always shows the full name;
/// minimum = min(width of full, width of first character + U+2026)"). User Name never
/// shortens its DRAWN text (kept bug, 9c ruling): this is only ever used to compute the
/// component's minimum width, never as something `pick` can choose to draw; the caller
/// measures both `full` and this candidate and keeps the narrower width, but always draws
/// `full` when there is room for anything at all.
pub fn user_min_text(full: &str) -> String {
    match full.chars().next() {
        Some(c) => format!("{c}\u{2026}"),
        None => String::new(),
    }
}

/// NSTextField's tail truncation (`NSLineBreakByTruncatingTail`, `ATXT:271-278`): the text
/// whole when it fits `room`, else its longest prefix that fits with U+2026 after it, else
/// `""`. `measure` gives a text's width in the same unit as `room`.
pub fn tail_fit(text: &str, room: f32, measure: &mut dyn FnMut(&str) -> f32) -> String {
    if measure(text) <= room {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let cut = |n: usize| chars[..n].iter().collect::<String>() + "\u{2026}";
    if measure("\u{2026}") > room {
        return String::new();
    }
    // The widest prefix that fits: widths only grow with the prefix, so a binary search.
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if measure(&cut(mid)) <= room {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    cut(lo)
}

/// `NSHost.it_hostnameIsThisMachine:`'s exact-match rule (`HOST:16-27, 123-135`), minus the
/// live-lookup and remembered-name parts: this module has no Win32, so it takes the
/// candidate local names (localhost, the computer name, `<name>.local`, ...) as a
/// parameter instead of gathering them itself. Case-insensitive exact match only; no
/// short-label leniency, matching the source's own comment about why.
pub fn is_local_host(host: &str, local_names: &[String]) -> bool {
    if host.is_empty() {
        return false;
    }
    local_names.iter().any(|n| n.eq_ignore_ascii_case(host))
}

/// A civil (broken-down) date and time, timezone-naive: the caller has already applied
/// whatever timezone the Clock component's SSH knobs (unused until milestone 14) or the
/// local clock call for. `weekday` is 0 for Sunday, matching TR35's own `E` field, not
/// `chrono`'s Monday-first convention (there is no `chrono` dependency here to match
/// anyway).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Civil {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub weekday: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub millis: u32,
}

impl Civil {
    /// The civil time (UTC calendar, no zone applied) of `secs` since 1970-01-01 and `millis`.
    /// Days are converted with the proleptic Gregorian rules; a value too large for the year to
    /// fit clamps rather than wraps.
    pub fn from_unix(secs: i64, millis: u32) -> Civil {
        let days = secs.div_euclid(86_400);
        let rest = secs.rem_euclid(86_400) as u32;
        // Howard Hinnant's civil_from_days, on i128 so no input overflows.
        let z = i128::from(days) + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
        let year = (yoe + era * 400 + i128::from(month <= 2)).clamp(i128::from(i32::MIN), i128::from(i32::MAX)) as i32;
        Civil {
            year,
            month,
            day,
            weekday: (days + 4).rem_euclid(7) as u32,
            hour: rest / 3600,
            minute: rest % 3600 / 60,
            second: rest % 60,
            millis: millis.min(999),
        }
    }
}

const MONTH_ABBR: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const MONTH_FULL: [&str; 12] =
    ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const WEEKDAY_ABBR: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const WEEKDAY_FULL: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

/// One run of a repeated TR35 pattern letter, rendered against `civil`. Month, weekday and
/// am/pm names are English regardless of the Clock's `localize` knob (Task 3 note): this
/// formatter renders a concrete pattern, whether that pattern came from the knob's literal
/// text or from `udatpg_getBestPattern`'s locale-aware template on the Windows side; this
/// module has no locale data to render the names themselves in anything else. An
/// unrecognized letter (any ASCII letter this function does not special-case) passes
/// through literally, `count` copies of it, rather than panicking on an unsupported knob.
fn field_text(letter: char, count: usize, civil: &Civil) -> String {
    match letter {
        'y' if count == 2 => {
            let v = civil.year.rem_euclid(100);
            format!("{v:02}")
        }
        'y' => format!("{:0width$}", civil.year, width = count),
        'M' if count >= 4 => MONTH_FULL[(civil.month.saturating_sub(1) as usize).min(11)].to_string(),
        'M' if count == 3 => MONTH_ABBR[(civil.month.saturating_sub(1) as usize).min(11)].to_string(),
        'M' => format!("{:0width$}", civil.month, width = count),
        'd' => format!("{:0width$}", civil.day, width = count),
        'E' if count >= 4 => WEEKDAY_FULL[(civil.weekday as usize).min(6)].to_string(),
        'E' => WEEKDAY_ABBR[(civil.weekday as usize).min(6)].to_string(),
        'H' => format!("{:0width$}", civil.hour, width = count),
        'h' => {
            let h12 = civil.hour % 12;
            format!("{:0width$}", if h12 == 0 { 12 } else { h12 }, width = count)
        }
        'm' => format!("{:0width$}", civil.minute, width = count),
        's' => format!("{:0width$}", civil.second, width = count),
        'a' => if civil.hour < 12 { "AM" } else { "PM" }.to_string(),
        other => other.to_string().repeat(count),
    }
}

/// A TR35 (Unicode date pattern) formatter, run against `civil`: enough of the standard
/// (letter-run fields, single-quoted literal text, `''` for a literal quote) to render the
/// Clock component's `format` knob, whether typed directly (localize off) or produced by
/// `udatpg_getBestPattern` from the knob's template (localize on, Task 3's job on the
/// Windows side; this function does not pick a pattern, only renders one).
pub fn format_tr35(pattern: &str, civil: &Civil) -> String {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            if i + 1 < chars.len() && chars[i + 1] == '\'' {
                out.push('\'');
                i += 2;
                continue;
            }
            i += 1;
            while i < chars.len() && chars[i] != '\'' {
                out.push(chars[i]);
                i += 1;
            }
            if i < chars.len() {
                i += 1;
            }
            continue;
        }
        if c.is_ascii_alphabetic() {
            let start = i;
            while i < chars.len() && chars[i] == c {
                i += 1;
            }
            out.push_str(&field_text(c, i - start, civil));
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// NSTextField truncates the tail: the prefix that fits beside U+2026, the whole text
    /// when it fits, nothing when not even U+2026 fits.
    #[test]
    fn tail_fit_follows_nstextfield() {
        let mut ten = |t: &str| t.chars().count() as f32 * 10.0;
        assert_eq!(tail_fit("feature/login", 60.0, &mut ten), "featu\u{2026}");
        assert_eq!(tail_fit("feature/login", 200.0, &mut ten), "feature/login");
        assert_eq!(tail_fit("feature/login", 130.0, &mut ten), "feature/login", "exactly as wide fits");
        assert_eq!(tail_fit("feature/login", 5.0, &mut ten), "");
        assert_eq!(tail_fit("feature/login", 10.0, &mut ten), "\u{2026}");
        assert_eq!(tail_fit("", 0.0, &mut ten), "");
    }

    #[test]
    fn widest_fitting_variant_wins() {
        let widths = [("a".to_string(), 40.0f32), ("b".to_string(), 80.0), ("c".to_string(), 120.0)];
        assert_eq!(pick(&widths, 100.0), "b");
        // ceil on both sides: a variant exactly at the narrowest ceiling above `available`
        // still counts as fitting.
        assert_eq!(pick(&widths, 80.4), "b");
        assert_eq!(pick(&widths, 30.0), "");
        assert_eq!(narrowest(&widths), 40.0);
        assert_eq!(widest(&widths), 120.0);

        // A tie at the widest-fitting width keeps the FIRST such variant
        // (`NSArray+iTerm.m:531-544`'s `maxWithBlock:` only replaces on strictly-greater).
        let tied = [("first".to_string(), 80.0f32), ("second".to_string(), 80.0), ("too-wide".to_string(), 120.0)];
        assert_eq!(pick(&tied, 100.0), "first");
    }

    #[test]
    fn hostname_variants_follow_iterm2() {
        assert_eq!(
            host_variants("foobar.example.com"),
            vec!["fo\u{2026}ar", "f\u{2026}ar", "foobar", "foobar.example.com", "foobar.example.c", "foobar.e.c", "f.e.c",]
        );
        assert_eq!(host_variants(""), Vec::<String>::new());
        // A one-label name that is itself a single character never reaches the
        // compress step at all (nothing shorter to produce): just itself.
        assert_eq!(host_variants("x"), vec!["x"]);
        // Disagreement with the plan's prose, resolved per the implementer rules (source
        // wins): a one-label name LONGER than one character still runs `host_compress`
        // once, since that override has no guard on the label count (`VAR:~300-315`), so
        // it is not "only itself". The Resume notes' own correction agrees ("compresses
        // the LAST label that is longer than one character") since the only label here is
        // both first and last.
        assert_eq!(host_variants("foobar"), vec!["foobar", "f"]);
    }

    #[test]
    fn directory_variants_follow_iterm2() {
        let home = "C:\\Users\\rb";
        let raw = "C:\\Users\\rb\\src\\iterm2\\sources";
        let display = display_dir(raw, home);
        assert_eq!(display, "~\\src\\iterm2\\sources");
        assert_eq!(
            dir_variants(&display),
            vec!["~\\src\\iterm2\\sources", "~\\s\\iterm2\\sources", "~\\s\\i\\sources", "~\\s\\i\\s",]
        );

        // A raw path with a dot in a component: the shared base's center-truncate section
        // (kept bug) fires on the whole string split by `.`, path separators and all,
        // ahead of Directory's own `/`- or `\`-compress ladder.
        let dotty = display_dir("C:\\Users\\rb\\abcdef.gh\\sources", home);
        assert_eq!(dotty, "~\\abcdef.gh\\sources");
        assert_eq!(
            dir_variants(&dotty),
            vec![
                "~\\a\u{2026}def",
                "~\\\u{2026}def",
                "~\\\u{2026}ef",
                "~\u{2026}ef",
                "~\\abcdef",
                "~\\abcdef.gh\\sources",
                "~\\a\\sources",
                "~\\a\\s",
            ]
        );

        // A forward-slash path (no backslash present) splits on `/` instead.
        assert_eq!(
            dir_variants("~/src/iterm2/sources"),
            vec!["~/src/iterm2/sources", "~/s/iterm2/sources", "~/s/i/sources", "~/s/i/s",]
        );

        // The home-prefix compare is case-insensitive (Windows correction over the
        // research); the raw suffix keeps its own casing.
        assert_eq!(display_dir("C:\\USERS\\RB\\Src", home), "~\\Src");
        // No prefix match: unchanged.
        assert_eq!(display_dir("D:\\other", home), "D:\\other");

        // Non-ASCII, case-insensitive: e with acute (U+00E9, 2 bytes) case-folds against
        // its capital (U+00C9, also 2 bytes), so this one happens to land on a char
        // boundary either way, but it is still real non-ASCII prefix-fold coverage.
        let accented_home = "C:\\Users\\Jos\u{00e9}";
        assert_eq!(display_dir("C:\\users\\JOS\u{00c9}\\src", accented_home), "~\\src");

        // A case pair whose UTF-8 widths differ: capital sharp S (U+1E9E, 3 bytes) folds to
        // lowercase sharp S (U+00DF, 2 bytes). `home`'s byte length lands on the LAST byte
        // of `raw`'s 3-byte character here (bytes 12..15 in `raw`, home.len() == 14): a
        // byte-offset `raw.split_at(home.len())` would panic mid-character. The char-based
        // comparison must not panic and must still recognize the fold.
        let sharp_s_home = "C:\\Users\\Wei\u{df}"; // "...Weiß"
        assert_eq!(sharp_s_home.len(), 14);
        let sharp_s_raw = "C:\\users\\WEI\u{1e9e}\\src"; // "...WEIẞ\src"
        assert_eq!(display_dir(sharp_s_raw, sharp_s_home), "~\\src");
    }

    #[test]
    fn job_variants_follow_iterm2() {
        assert_eq!(job_variants(&["vim".to_string(), "bash".to_string()]), vec!["vim", "vim \u{25C2} bash"]);
        assert_eq!(job_variants(&[]), vec![""]);
        assert_eq!(job_variants(&["".to_string(), "".to_string()]), vec!["\u{2026}"]);
    }

    #[test]
    fn action_title_variants_follow_iterm2() {
        assert_eq!(action_variants("Run my job"), vec!["Run", "Run my", "Run my job"]);
        assert_eq!(action_variants("Run"), vec!["Run"]);
    }

    #[test]
    fn tmux_variants_follow_iterm2() {
        assert_eq!(tmux_left_variants("a | b | c"), vec!["a | b | c", "a | b", "a"]);
        assert_eq!(tmux_right_variants("a | b | c"), vec!["a | b | c", "b | c", "c"]);
    }

    #[test]
    fn clock_formats_like_tr35() {
        let c = Civil { year: 2026, month: 10, day: 4, weekday: 0, hour: 15, minute: 7, second: 9, millis: 0 };
        assert_eq!(format_tr35("M/dd h:mm", &c), "10/04 3:07");
        assert_eq!(format_tr35("yyyy-MM-dd HH:mm:ss", &c), "2026-10-04 15:07:09");
        assert_eq!(format_tr35("EEE MMM d", &c), "Sun Oct 4");
        assert_eq!(format_tr35("h:mm a", &c), "3:07 PM");
        assert_eq!(format_tr35("'at' h", &c), "at 3");
        assert_eq!(format_tr35("''", &c), "'");
    }

    #[test]
    fn localhost_is_this_machine() {
        let names = vec!["localhost".to_string(), "MyComputer".to_string(), "MyComputer.local".to_string()];
        assert!(is_local_host("localhost", &names));
        assert!(is_local_host("mycomputer", &names));
        assert!(is_local_host("MYCOMPUTER.LOCAL", &names));
        assert!(!is_local_host("othermachine", &names));
        assert!(!is_local_host("", &names));
    }

    #[test]
    fn civil_from_unix_follows_the_calendar() {
        let c = Civil::from_unix(0, 0);
        assert_eq!((c.year, c.month, c.day, c.weekday, c.hour, c.minute, c.second, c.millis), (1970, 1, 1, 4, 0, 0, 0, 0));
        let c = Civil::from_unix(951782400, 250);
        assert_eq!((c.year, c.month, c.day, c.weekday, c.hour, c.minute, c.second, c.millis), (2000, 2, 29, 2, 0, 0, 0, 250));
        let c = Civil::from_unix(-1, 0);
        assert_eq!((c.year, c.month, c.day, c.weekday, c.hour, c.minute, c.second), (1969, 12, 31, 3, 23, 59, 59));
        // A hostile value never panics.
        let _ = Civil::from_unix(i64::MAX, 0);
        let _ = Civil::from_unix(i64::MIN, 999);
    }
}

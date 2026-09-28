//! iTerm2's paste pipeline (iTermPasteHelper, PasteEvent): the transforms in iTerm2's order,
//! bracketing, speeds and chunks, the warnings, and the paste configuration that key and
//! pointer actions carry.

use base64::Engine;

/// iTerm2's iTermPasteFlags.
pub const ESCAPE: u32 = 1 << 0;
pub const NEWLINES: u32 = 1 << 3;
pub const CONTROLS: u32 = 1 << 4;
pub const BRACKET: u32 = 1 << 5;
pub const BASE64: u32 = 1 << 6;
pub const PUNCTUATION: u32 = 1 << 7;
/// Wait for the shell prompt before each line.
pub const COMMANDS: u32 = 1 << 8;
pub const REMOVE_NEWLINES: u32 = 1 << 9;
pub const REGEX: u32 = 1 << 10;
pub const NO_WARNINGS: u32 = 1 << 11;
/// Tandem's: a plain paste (the menu, keys, clicks, drops), the only kind the tabs question
/// is for (iTerm2 asks it in pasteString:flags: only).
pub const PLAIN: u32 = 1 << 12;

/// What happens to tabs (iTerm2's iTermTabTransformTags).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tabs {
    Keep,
    Spaces(usize),
    /// Ctrl+V (LNEXT) before each tab.
    CtrlV,
}

/// One paste: the text and how it is sent.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub text: String,
    pub flags: u32,
    pub tabs: Tabs,
    pub regex: String,
    pub substitution: String,
}

impl Job {
    pub fn new(text: &str, flags: u32) -> Job {
        Job { text: text.to_string(), flags, tabs: Tabs::Keep, regex: String::new(), substitution: String::new() }
    }
}

/// Settings the transforms read (EscapeWithQuotes, StripZeroWidthFormatCharactersOnPaste).
#[derive(Clone, Copy, Debug, Default)]
pub struct Prefs {
    pub escape_with_quotes: bool,
    pub strip_zero_width: bool,
}

/// iTerm2's shell characters, in its order (a backslash goes before each).
const SHELL: &str = "\\ ()\"&'!$<>;|*?[]#`\t{}^+=@~";

/// iTerm2's sanitizePasteEvent: every transform the flags ask for, in iTerm2's order.
pub fn sanitize(job: &Job, prefs: &Prefs) -> String {
    let mut t = job.text.clone();
    // 1. Newlines: removed, or each CRLF and LF becomes CR.
    if job.flags & REMOVE_NEWLINES != 0 {
        t.retain(|c| c != '\r' && c != '\n');
    } else if job.flags & NEWLINES != 0 {
        t = t.replace("\r\n", "\r").replace('\n', "\r");
    }
    // 2. Unicode quotes and dashes become ASCII.
    if job.flags & PUNCTUATION != 0 {
        t = t
            .chars()
            .map(|c| match c {
                '\u{201c}'..='\u{201f}' | '\u{301d}'..='\u{301f}' | '\u{ff02}' => '"',
                '\u{2018}'..='\u{201b}' | '\u{ff07}' => '\'',
                '\u{2010}'..='\u{2015}' | '\u{207b}' | '\u{208b}' | '\u{2212}' | '\u{2e3a}' | '\u{2e3b}' | '\u{fe58}' | '\u{fe63}' | '\u{ff0d}' | '\u{ad}' => '-',
                c => c,
            })
            .collect();
    }
    // 3. C0 controls but tab, LF, FF, and CR (before the tab step can add ^V).
    if job.flags & CONTROLS != 0 {
        t.retain(|c| !((c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\x0c' | '\r')));
    }
    // 4. Zero-width format characters (StripZeroWidthFormatCharactersOnPaste).
    if prefs.strip_zero_width {
        t.retain(|c| !matches!(c, '\u{200b}'..='\u{200f}' | '\u{ad}' | '\u{2060}' | '\u{feff}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{61c}'));
    }
    // 5. Tabs.
    match job.tabs {
        Tabs::Keep => {}
        Tabs::Spaces(n) => t = t.replace('\t', &" ".repeat(n)),
        Tabs::CtrlV => t = t.replace('\t', "\x16\t"),
    }
    // 6. Shell escaping (newlines are not escaped).
    if job.flags & ESCAPE != 0 {
        t = if job.tabs == Tabs::CtrlV {
            backslashed(&t, |c| c != '\t')
        } else if prefs.escape_with_quotes {
            if t.chars().any(|c| SHELL.contains(c)) { format!("'{}'", t.replace('\'', "'\\''")) } else { t }
        } else {
            backslashed(&t, |_| true)
        };
    }
    // 7. Regex substitution (a bad expression changes nothing).
    if job.flags & REGEX != 0 && !job.regex.is_empty() {
        if let Ok(re) = fancy_regex::Regex::new(&job.regex) {
            if let Ok(out) = re.try_replacen(&t, 0, expand_vim(&job.substitution).as_str()) {
                t = out.into_owned();
            }
        }
    }
    // 8. Base64.
    if job.flags & BASE64 != 0 {
        t = base64_lines(t.as_bytes());
    }
    t
}

/// A backslash before each shell character that `which` keeps.
fn backslashed(t: &str, which: impl Fn(char) -> bool) -> String {
    let mut out = String::with_capacity(t.len());
    for c in t.chars() {
        if SHELL.contains(c) && which(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// ESC [200~ ... ESC [201~; `per_line`: each line on its own, its newline outside.
pub fn bracket(text: &str, per_line: bool) -> String {
    // No ESC inside, so the text cannot end its own bracket (Tandem keeps this guard).
    let wrap = |s: &str| format!("\x1b[200~{}\x1b[201~", s.replace('\x1b', ""));
    if !per_line {
        return wrap(text);
    }
    let mut out = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        let end = rest.find(['\r', '\n']).unwrap_or(rest.len());
        let nl = if rest[end..].starts_with("\r\n") { 2 } else if end < rest.len() { 1 } else { 0 };
        out.push_str(&wrap(&rest[..end]));
        out.push_str(&rest[end..end + nl]);
        rest = &rest[end + nl..];
    }
    out
}

/// Base64 in 77-character lines, each ending in CR (iTerm2's).
pub fn base64_lines(bytes: &[u8]) -> String {
    let b = base64::engine::general_purpose::STANDARD.encode(bytes);
    b.as_bytes().chunks(77).map(|c| format!("{}\r", std::str::from_utf8(c).unwrap_or(""))).collect()
}

/// iTerm2's stringByExpandingVimSpecialCharacters for a substitution.
pub fn expand_vim(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let hex = |from: usize, max: usize| -> (u32, usize) {
        let digits: String = c[from..].iter().take(max).take_while(|d| d.is_ascii_hexdigit()).collect();
        (u32::from_str_radix(&digits, 16).unwrap_or(0), digits.len())
    };
    while i < c.len() {
        if c[i] != '\\' || i + 1 >= c.len() {
            out.push(c[i]);
            i += 1;
            continue;
        }
        let n = c[i + 1];
        match n {
            '0'..='7' => {
                let digits: String = c[i + 1..].iter().take(3).take_while(|d| ('0'..='7').contains(*d)).collect();
                out.extend(char::from_u32(u32::from_str_radix(&digits, 8).unwrap_or(0)));
                i += 1 + digits.len();
            }
            'x' | 'u' => {
                let (v, len) = hex(i + 2, if n == 'x' { 2 } else { 4 });
                if len == 0 {
                    out.push('\\');
                    out.push(n);
                } else {
                    out.extend(char::from_u32(v));
                }
                i += 2 + len;
            }
            '<' if c.len() >= i + 6 && c[i + 3] == '-' && c[i + 5] == '>' && matches!(c[i + 2], 'C' | 'M') => {
                let k = c[i + 4];
                if c[i + 2] == 'C' {
                    out.extend(char::from_u32(k as u32 & 0x1f));
                } else {
                    out.push('\x1b');
                    out.push(k);
                }
                i += 6;
            }
            _ => {
                match n {
                    'b' => out.push('\x08'),
                    'e' => out.push('\x1b'),
                    'f' => out.push('\x0c'),
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    '\\' => out.push('\\'),
                    '"' => out.push('"'),
                    other => {
                        out.push('\\');
                        out.push(other);
                    }
                }
                i += 2;
            }
        }
    }
    out
}

/// How fast a paste goes: UTF-16 units per write and seconds between writes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Speed {
    pub units: usize,
    pub delay: f64,
}

/// iTerm2's effective defaults (QuickPaste*, SlowPaste*).
pub const QUICK: Speed = Speed { units: 768, delay: 0.01 };
pub const SLOW: Speed = Speed { units: 16, delay: 0.125 };

/// The longest wait between writes a setting may ask for, in seconds.
pub const MAX_DELAY: f64 = 60.0;

/// The speed a paste setting pair gives (0 or less means iTerm2's default `d`).
/// A delay that is not a finite positive number is the default; one above `MAX_DELAY` is
/// capped, so it always converts to a `Duration`.
pub fn speed_from(units: i64, delay: Option<f64>, d: Speed) -> Speed {
    let delay = delay.filter(|t| t.is_finite() && *t > 0.0).map_or(d.delay, |t| t.min(MAX_DELAY));
    Speed { units: if units > 0 { units as usize } else { d.units }, delay }
}

/// Paste Faster (1.5) and Paste Slower (0.66): more per write, less wait, within bounds.
pub fn faster(s: Speed, factor: f64) -> Speed {
    let units = ((s.units as f64 * factor) as usize).clamp(1, 1_048_576);
    Speed { units, delay: (s.delay / factor).clamp(0.001, 10.0) }
}

/// A byte count as iTerm2 shows sizes: "12 bytes", "1 byte", "75.0 KB", "1.2 MB".
pub fn format_bytes(n: f64) -> String {
    if n < 1024.0 {
        let n = n.round() as u64;
        format!("{n} byte{}", if n == 1 { "" } else { "s" })
    } else if n < 1024.0 * 1024.0 {
        format!("{:.1} KB", n / 1024.0)
    } else {
        format!("{:.1} MB", n / (1024.0 * 1024.0))
    }
}

/// "Pasting at up to 75.0 KB/sec".
pub fn rate_text(s: Speed) -> String {
    format!("Pasting at up to {}/sec", format_bytes(s.units as f64 / s.delay.max(1e-9)))
}

/// The end (exclusive) of the next write from `at`: `units` long, never ending inside a
/// surrogate pair, and ending after the first newline when pasting waits for prompts.
pub fn next_chunk(text: &[u16], at: usize, units: usize, block_at_newline: bool) -> usize {
    let mut end = (at + units.max(1)).min(text.len());
    if end < text.len() && (0xd800..0xdc00).contains(&text[end - 1]) {
        end += 1;
    }
    if block_at_newline {
        if let Some(i) = text[at..end].iter().position(|c| *c == b'\n' as u16 || *c == b'\r' as u16) {
            end = at + i + 1;
        }
    }
    end
}

/// iTerm2 shows its progress bar when a paste takes over 3 s, or waits for prompts.
pub fn shows_progress(len: usize, s: Speed, commands: bool) -> bool {
    commands || s.delay * len as f64 / s.units.max(1) as f64 + (len / 10_000) as f64 > 3.0
}

/// The settings the warnings read. Whether a warning is silenced is not among them: a
/// silenced warning still comes, and its remembered answer answers it (`warnings::preempt`).
#[derive(Clone, Copy, Debug, Default)]
pub struct WarnPrefs {
    /// PromptForPasteWhenNotAtPrompt.
    pub when_not_at_prompt: bool,
    /// SuppressMultilinePasteWarningWhenPastingOneLineWithTerminalNewline.
    pub skip_one_line_newline: bool,
    /// SuppressMultilinePasteWarningWhenNotAtShellPrompt.
    pub skip_not_at_prompt: bool,
    /// AlwaysWarnBeforePastingOverSize (-1: off).
    pub size_limit: i64,
}

/// A question before a paste.
#[derive(Clone, Debug, PartialEq)]
pub enum Warning {
    Tabs,
    Size(usize),
    Lines { n: usize, at_prompt: bool },
    OneLine { at_prompt: bool },
}

impl Warning {
    pub fn text(&self) -> String {
        match self {
            Warning::Tabs => "You're about to paste a string with tabs.".into(),
            Warning::Size(n) => format!("OK to paste {n} characters?"),
            Warning::Lines { n, at_prompt: true } => format!("OK to paste {n} lines at shell prompt?"),
            Warning::Lines { n, at_prompt: false } => format!("OK to paste {n} lines?"),
            Warning::OneLine { at_prompt: true } => "OK to paste one line ending in a newline at shell prompt?".into(),
            Warning::OneLine { at_prompt: false } => "OK to paste one line ending in a newline?".into(),
        }
    }

    /// iTerm2's heading over the question.
    pub fn heading(&self) -> &'static str {
        match self {
            Warning::Tabs => "Paste Tabs",
            Warning::Size(_) => "Paste Limit Exceeded",
            Warning::Lines { .. } | Warning::OneLine { .. } => "Confirm Multi-Line Paste",
        }
    }

    pub fn buttons(&self) -> &'static [&'static str] {
        match self {
            Warning::Tabs => &["OK", "Cancel", "Convert tabs to spaces", "Advanced..."],
            Warning::Size(_) => &["OK", "Cancel", "Advanced..."],
            Warning::Lines { .. } => &["Paste", "Cancel", "Advanced..."],
            Warning::OneLine { .. } => &["Paste", "Paste Without Newline", "Cancel", "Advanced..."],
        }
    }

    /// The warning with `heading` and `text`: its identifier and kind (the size question is
    /// persistent, `iTermPasteHelper.m:758-759`; the rest permanently silenceable, `:865-867`,
    /// `:910-911`) and its Cancel button, never remembered.
    pub fn spec(&self, heading: &str, text: &str) -> crate::warnings::Spec {
        use crate::warnings::Kind;
        let (id, kind) = match self {
            Warning::Tabs => ("AboutToPasteTabsWithCancel", Kind::Permanent),
            Warning::Size(_) => ("NoSyncPasteOverCharacterLimitWarning", Kind::Persistent),
            Warning::OneLine { at_prompt: true } => ("NoSyncDoNotWarnBeforePastingOneLineEndingInNewlineAtShellPrompt", Kind::Permanent),
            Warning::Lines { .. } | Warning::OneLine { .. } => ("NoSyncDoNotWarnBeforeMultilinePaste", Kind::Permanent),
        };
        let labels = self.buttons();
        crate::warnings::Spec {
            id: Some(id.to_string()),
            kind,
            heading: heading.to_string(),
            text: text.to_string(),
            labels: labels.iter().map(|l| l.to_string()).collect(),
            cancel: labels.iter().position(|l| *l == "Cancel"),
            never_remember: Vec::new(),
        }
    }
}

/// The questions a paste asks, in iTerm2's order (tabs, size, then lines).
/// `at_prompt`: None without shell integration.
pub fn check(text: &str, at_prompt: Option<bool>, bracketed: bool, flags: u32, p: &WarnPrefs) -> Vec<Warning> {
    let mut out = Vec::new();
    if text.is_empty() || flags & (COMMANDS | NO_WARNINGS) != 0 {
        return out;
    }
    if flags & PLAIN != 0 && !bracketed && text.contains('\t') {
        out.push(Warning::Tabs);
    }
    let n = text.encode_utf16().count();
    if p.size_limit >= 0 && n as i64 > p.size_limit {
        out.push(Warning::Size(n));
    }
    if flags & REMOVE_NEWLINES != 0 || !text.contains(['\r', '\n']) {
        return out;
    }
    let only_last = text.find(['\r', '\n']) == Some(text.len() - 1);
    let at = at_prompt == Some(true);
    if (p.skip_one_line_newline && only_last) || (p.skip_not_at_prompt && !at) {
        return out;
    }
    // iTerm2's componentsSeparatedByRegex drops trailing empty lines.
    let mut lines: Vec<&str> = text.split("\r\n").flat_map(|l| l.split(['\r', '\n'])).collect();
    while lines.last() == Some(&"") {
        lines.pop();
    }
    let w = if lines.len() > 1 { Warning::Lines { n: lines.len(), at_prompt: at } } else { Warning::OneLine { at_prompt: at } };
    // Not at the prompt, only with PromptForPasteWhenNotAtPrompt.
    if at || p.when_not_at_prompt {
        out.push(w);
    }
    out
}

/// A paste configuration that key and pointer actions carry (iTerm2's JSON).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config {
    pub chunk_size: i64,
    pub delay: f64,
    pub tab_stop_size: i64,
    pub tab_transform: i64,
    pub convert_newlines: bool,
    pub remove_newlines: bool,
    pub convert_punctuation: bool,
    pub escape_for_shell: bool,
    pub remove_controls: bool,
    pub bracket_allowed: bool,
    pub base64: bool,
    pub use_regex: bool,
    pub regex: String,
    pub substitution: String,
    pub wait_for_prompts: bool,
}

impl Config {
    /// Missing keys read as 0, false, or empty (iTerm2's).
    pub fn decode(json: &str) -> Config {
        // An empty or unreadable setting is a new action in iTerm2's editor.
        let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
            return Config { convert_newlines: true, convert_punctuation: true, remove_controls: true, bracket_allowed: true, ..Default::default() };
        };
        let b = |k: &str| v.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
        let i = |k: &str| v.get(k).and_then(|x| x.as_i64().or_else(|| x.as_f64().map(|f| f as i64))).unwrap_or(0);
        let st = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        Config {
            chunk_size: i("ChunkSize"),
            delay: v.get("Delay").and_then(|x| x.as_f64()).unwrap_or(0.0),
            tab_stop_size: i("TabStopSize"),
            tab_transform: i("TabTransform"),
            convert_newlines: b("ConvertNewlines"),
            remove_newlines: b("RemoveNewlines"),
            convert_punctuation: b("ConvertUnicodePunctuation"),
            escape_for_shell: b("EscapeForShell"),
            remove_controls: b("RemoveControls"),
            bracket_allowed: b("BracketAllowed"),
            base64: b("Base64"),
            use_regex: b("UseRegexSubstitution"),
            regex: st("Regex"),
            substitution: st("Substitution"),
            wait_for_prompts: b("WaitForPrompts"),
        }
    }

    pub fn encode(&self) -> String {
        serde_json::json!({
            "ChunkSize": self.chunk_size,
            "Delay": self.delay,
            "TabStopSize": self.tab_stop_size,
            "TabTransform": self.tab_transform,
            "ConvertNewlines": self.convert_newlines,
            "RemoveNewlines": self.remove_newlines,
            "ConvertUnicodePunctuation": self.convert_punctuation,
            "EscapeForShell": self.escape_for_shell,
            "RemoveControls": self.remove_controls,
            "BracketAllowed": self.bracket_allowed,
            "Base64": self.base64,
            "UseRegexSubstitution": self.use_regex,
            "Regex": self.regex,
            "Substitution": self.substitution,
            "WaitForPrompts": self.wait_for_prompts,
        })
        .to_string()
    }

    /// The paste this configuration makes of `text`, and its speed.
    pub fn job(&self, text: &str) -> (Job, Speed) {
        let pairs = [
            (self.convert_newlines, NEWLINES),
            (self.remove_newlines, REMOVE_NEWLINES),
            (self.escape_for_shell, ESCAPE),
            (self.remove_controls, CONTROLS),
            (self.bracket_allowed, BRACKET),
            (self.base64, BASE64),
            (self.wait_for_prompts, COMMANDS),
            (self.convert_punctuation, PUNCTUATION),
            (self.use_regex, REGEX),
        ];
        let flags = pairs.iter().filter(|(on, _)| *on).fold(0, |f, (_, bit)| f | bit);
        let tabs = match self.tab_transform {
            1 => Tabs::Spaces(self.tab_stop_size.max(0) as usize),
            2 => Tabs::CtrlV,
            _ => Tabs::Keep,
        };
        let job = Job { text: text.to_string(), flags, tabs, regex: self.regex.clone(), substitution: self.substitution.clone() };
        (job, Speed { units: self.chunk_size.max(0) as usize, delay: self.delay })
    }

    /// The summary in the key and pointer lists.
    pub fn summary(&self) -> String {
        let mut p: Vec<String> = Vec::new();
        if self.base64 {
            p.push("Base64".into());
        }
        if self.wait_for_prompts {
            p.push("WaitForPrompts".into());
        }
        match self.tab_transform {
            1 => p.push(format!("Tabs->{} spcs", self.tab_stop_size)),
            2 => p.push("^V+Tab".into()),
            _ => {}
        }
        if self.delay > 1.0 {
            p.push("V.Slow".into());
        } else if self.delay > 0.25 {
            p.push("Slow".into());
        } else if self.chunk_size > 100_000 {
            p.push("HugeChunks".into());
        }
        if self.escape_for_shell {
            p.push("\\Escape".into());
        }
        if !self.remove_controls {
            p.push("UnsafeControls".into());
        }
        if !self.bracket_allowed {
            p.push("BracketingOff".into());
        }
        if !self.convert_newlines {
            p.push("NoCRLFConversion".into());
        }
        if self.remove_newlines {
            p.push("RemoveNewlines".into());
        }
        if self.convert_punctuation {
            p.push("ConvertPunctuation".into());
        }
        if self.use_regex {
            p.push(format!("s/{}/{}/g", self.regex, self.substitution));
        }
        p.join(", ")
    }
}

/// One pane's pastes: the one being written and the ones waiting (iTerm2 queues a paste
/// that arrives while another is still going).
#[derive(Default)]
pub struct Queue {
    running: Option<Running>,
    waiting: std::collections::VecDeque<(Vec<u16>, Speed, bool)>,
}

struct Running {
    units: Vec<u16>,
    at: usize,
    speed: Speed,
    /// Wait for the shell prompt after each line.
    commands: bool,
    /// Blocked until more prompts than this have been seen.
    wait_for: Option<usize>,
    next: f64,
}

impl Queue {
    pub fn push(&mut self, text: &str, speed: Speed, commands: bool) {
        self.waiting.push_back((text.encode_utf16().collect(), speed, commands));
    }

    /// The next write when it is due (`now` in seconds; `prompts`: prompts seen so far).
    pub fn tick(&mut self, now: f64, prompts: usize) -> Option<String> {
        if self.running.is_none() {
            let (units, speed, commands) = self.waiting.pop_front()?;
            self.running = Some(Running { units, at: 0, speed, commands, wait_for: None, next: now });
        }
        let r = self.running.as_mut()?;
        if r.wait_for.is_some_and(|n| prompts <= n) || now < r.next {
            return None;
        }
        r.wait_for = None;
        let end = next_chunk(&r.units, r.at, r.speed.units, r.commands);
        let chunk = String::from_utf16_lossy(&r.units[r.at..end]);
        r.at = end;
        r.next = now + r.speed.delay;
        if r.commands && chunk.ends_with(['\r', '\n']) {
            r.wait_for = Some(prompts);
        }
        if r.at >= r.units.len() {
            self.running = None;
        }
        Some(chunk)
    }

    /// Cancel: the paste being written stops; the next one starts.
    pub fn cancel(&mut self) {
        self.running = None;
    }

    /// (sent, total) of the paste being written.
    pub fn progress(&self) -> Option<(usize, usize)> {
        self.running.as_ref().map(|r| (r.at, r.units.len()))
    }

    /// When the next write is due (None: nothing to write, or waiting for a prompt).
    pub fn next_due(&self) -> Option<f64> {
        match &self.running {
            Some(r) if r.wait_for.is_some() => None,
            Some(r) => Some(r.next),
            None => (!self.waiting.is_empty()).then_some(0.0),
        }
    }

    pub fn busy(&self) -> bool {
        self.running.is_some() || !self.waiting.is_empty()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn settings_speeds_stay_finite() {
        use super::{QUICK, Speed, speed_from};
        assert_eq!(speed_from(0, None, QUICK), QUICK);
        assert_eq!(speed_from(100, Some(0.5), QUICK), Speed { units: 100, delay: 0.5 });
        assert_eq!(speed_from(-3, Some(-1.0), QUICK), QUICK);
        for bad in [f64::INFINITY, f64::NAN, f64::NEG_INFINITY] {
            assert_eq!(speed_from(100, Some(bad), QUICK), Speed { units: 100, delay: QUICK.delay }, "{bad}");
        }
        assert_eq!(speed_from(100, Some(1e30), QUICK).delay, super::MAX_DELAY, "a huge delay is capped");
        assert_eq!(super::MAX_DELAY, 60.0);
        // Every speed converts to a Duration without a panic.
        for t in [1e30, f64::INFINITY, 0.25] {
            let _ = std::time::Duration::from_secs_f64(speed_from(1, Some(t), QUICK).delay);
        }
    }

    use super::*;

    fn run(text: &str, flags: u32) -> String {
        sanitize(&Job::new(text, flags), &Prefs::default())
    }

    #[test]
    fn transforms_follow_iterm2() {
        assert_eq!(run("a\r\nb\nc\rd", NEWLINES), "a\rb\rc\rd");
        assert_eq!(run("a\r\nb\nc", NEWLINES | REMOVE_NEWLINES), "abc", "removing wins");
        assert_eq!(run("\u{201c}hi\u{201d} \u{2018}x\u{2019} a\u{2013}b\u{2014}c\u{ff0d}", PUNCTUATION), "\"hi\" 'x' a-b-c-");
        assert_eq!(run("a\x1b[1m\x16\t\x0c\x7f\u{85}", CONTROLS), "a[1m\t\x0c\x7f\u{85}", "C0 but tab, LF, FF, CR; DEL and C1 stay");
        assert_eq!(run("a\u{200c}b", 0), "a\u{200c}b");
        assert_eq!(sanitize(&Job::new("a\u{200c}\u{feff}b", 0), &Prefs { strip_zero_width: true, ..Default::default() }), "ab");
        let tabbed = |t: Tabs, flags: u32| sanitize(&Job { tabs: t, ..Job::new("a\tb c", flags) }, &Prefs::default());
        assert_eq!(tabbed(Tabs::Spaces(2), 0), "a  b c");
        assert_eq!(tabbed(Tabs::CtrlV, CONTROLS), "a\x16\tb c", "controls go before the tab step adds ^V");
        assert_eq!(tabbed(Tabs::CtrlV, ESCAPE), "a\x16\tb\\ c", "Ctrl+V tabs are not escaped");
        assert_eq!(run("a b'c\\", ESCAPE), "a\\ b\\'c\\\\");
        assert_eq!(run("x\ty", ESCAPE), "x\\\ty");
        let quoted = |t: &str| sanitize(&Job::new(t, ESCAPE), &Prefs { escape_with_quotes: true, ..Default::default() });
        assert_eq!(quoted("a b'c\\"), "'a b'\\''c\\'");
        assert_eq!(quoted("plain"), "plain");
        let re = |r: &str, s: &str| sanitize(&Job { regex: r.into(), substitution: s.into(), ..Job::new("foo bar", REGEX) }, &Prefs::default());
        assert_eq!(re("(o+)", "<$1>"), "f<oo> bar");
        assert_eq!(re("\\s", "\\t"), "foo\tbar", "vim escapes in the substitution");
        assert_eq!(re("(", "x"), "foo bar", "a bad expression changes nothing");
        assert_eq!(re("", "x"), "foo bar");
        assert_eq!(run("hi", BASE64), "aGk=\r");
        assert_eq!(run("a\nb", NEWLINES | BASE64), "YQ1i\r", "base64 is last");
    }

    #[test]
    fn base64_lines_are_77_wide() {
        let out = base64_lines(&[7u8; 100]);
        let lines: Vec<&str> = out.split('\r').collect();
        assert_eq!(lines[0].len(), 77);
        assert_eq!(lines.last(), Some(&""), "the last line ends in CR too");
        assert_eq!(lines.len(), 3);
        assert_eq!(base64_lines(b""), "");
    }

    #[test]
    fn vim_escapes_expand() {
        assert_eq!(expand_vim("\\101\\x41\\u00e9\\e\\n\\t\\\\\\\""), "AA\u{e9}\x1b\n\t\\\"");
        assert_eq!(expand_vim("\\7x"), "\x07x");
        assert_eq!(expand_vim("\\<C-c>\\<M-x>"), "\x03\x1bx");
        assert_eq!(expand_vim("\\q plain"), "\\q plain");
    }

    #[test]
    fn brackets_wrap_lines() {
        assert_eq!(bracket("ls\rpwd", false), "\x1b[200~ls\rpwd\x1b[201~");
        assert_eq!(bracket("a\x1b[201~b", false), "\x1b[200~a[201~b\x1b[201~", "no ESC inside: a paste cannot end its bracket");
        assert_eq!(bracket("ls\rpwd\r", true), "\x1b[200~ls\x1b[201~\r\x1b[200~pwd\x1b[201~\r");
        assert_eq!(bracket("a\r\nb", true), "\x1b[200~a\x1b[201~\r\n\x1b[200~b\x1b[201~");
    }

    #[test]
    fn speeds_follow_iterm2() {
        let f = faster(QUICK, 1.5);
        assert_eq!(f.units, 1152);
        assert!((f.delay - 0.01 / 1.5).abs() < 1e-12);
        let sl = faster(QUICK, 0.66);
        assert_eq!(sl.units, 506, "truncated");
        assert_eq!(faster(Speed { units: 1_000_000, delay: 0.001 }, 1.5), Speed { units: 1_048_576, delay: 0.001 }, "bounds");
        assert_eq!(faster(Speed { units: 1, delay: 9.0 }, 0.66), Speed { units: 1, delay: 10.0 });
        assert_eq!(rate_text(QUICK), "Pasting at up to 75.0 KB/sec");
        assert!(shows_progress(1_000_000, QUICK, false), "13 s");
        assert!(!shows_progress(10_000, QUICK, false));
        assert!(shows_progress(5, QUICK, true), "waiting for prompts always shows it");
    }

    #[test]
    fn chunks_never_split_surrogates() {
        let t: Vec<u16> = "ab\u{1f600}cd".encode_utf16().collect();
        assert_eq!(next_chunk(&t, 0, 3, false), 4, "the pair stays whole");
        assert_eq!(next_chunk(&t, 0, 2, false), 2);
        assert_eq!(next_chunk(&t, 4, 100, false), t.len());
        let lines: Vec<u16> = "ls\rpwd\r".encode_utf16().collect();
        assert_eq!(next_chunk(&lines, 0, 100, true), 3, "one line when waiting for prompts");
        assert_eq!(next_chunk(&lines, 3, 100, true), 7);
    }

    #[test]
    fn warnings_follow_iterm2() {
        let p = WarnPrefs { size_limit: -1, ..Default::default() };
        assert_eq!(check("abc\n", Some(true), true, NEWLINES, &p), vec![Warning::OneLine { at_prompt: true }]);
        assert_eq!(Warning::OneLine { at_prompt: true }.text(), "OK to paste one line ending in a newline at shell prompt?");
        assert_eq!(Warning::OneLine { at_prompt: true }.buttons(), &["Paste", "Paste Without Newline", "Cancel", "Advanced..."]);
        assert_eq!(check("a\nb\n", Some(true), true, 0, &p), vec![Warning::Lines { n: 2, at_prompt: true }]);
        assert_eq!(check("a\n\nb", Some(true), true, 0, &p), vec![Warning::Lines { n: 3, at_prompt: true }]);
        assert_eq!(Warning::Lines { n: 2, at_prompt: true }.text(), "OK to paste 2 lines at shell prompt?");
        assert!(check("a\nb", Some(false), true, 0, &p).is_empty(), "not at a prompt: only with PromptForPasteWhenNotAtPrompt");
        assert!(check("a\nb", None, true, 0, &p).is_empty(), "no shell integration: not at a prompt");
        let q = WarnPrefs { when_not_at_prompt: true, ..p };
        assert_eq!(check("a\nb", Some(false), true, 0, &q), vec![Warning::Lines { n: 2, at_prompt: false }]);
        assert!(check("a\nb", Some(true), true, NO_WARNINGS, &p).is_empty());
        assert!(check("a\nb", Some(true), true, COMMANDS, &p).is_empty());
        assert!(check("a\nb", Some(true), true, REMOVE_NEWLINES, &p).is_empty());
        assert!(check("ab\n", Some(true), true, 0, &WarnPrefs { skip_one_line_newline: true, ..p }).is_empty());
        assert_eq!(check("a\tb", Some(false), false, PLAIN, &p), vec![Warning::Tabs], "tabs with bracketing off");
        assert!(check("a\tb", Some(false), true, PLAIN, &p).is_empty(), "bracketing on: tabs go as they are");
        assert!(check("a\tb", Some(false), false, 0, &p).is_empty(), "a configured paste has its own tab rule");
        assert_eq!(Warning::Tabs.text(), "You're about to paste a string with tabs.");
        assert_eq!(check("abcdef", None, true, 0, &WarnPrefs { size_limit: 3, ..p }), vec![Warning::Size(6)]);
        assert_eq!(Warning::Size(6).text(), "OK to paste 6 characters?");
    }

    #[test]
    fn paste_specs_follow_iterm2() {
        use crate::warnings::Kind;
        let spec = |w: Warning| w.spec(w.heading(), &w.text());
        let tabs = spec(Warning::Tabs);
        assert_eq!((tabs.id.as_deref(), tabs.kind, tabs.cancel), (Some("AboutToPasteTabsWithCancel"), Kind::Permanent, Some(1)));
        assert_eq!((tabs.heading.as_str(), tabs.text.as_str()), ("Paste Tabs", "You're about to paste a string with tabs."));
        assert_eq!(tabs.labels, ["OK", "Cancel", "Convert tabs to spaces", "Advanced..."]);
        let size = spec(Warning::Size(6));
        assert_eq!((size.id.as_deref(), size.kind), (Some("NoSyncPasteOverCharacterLimitWarning"), Kind::Persistent), "never silenced");
        let lines = spec(Warning::Lines { n: 2, at_prompt: true });
        assert_eq!((lines.id.as_deref(), lines.kind, lines.cancel, lines.never_remember.as_slice()), (Some("NoSyncDoNotWarnBeforeMultilinePaste"), Kind::Permanent, Some(1), &[][..]));
        let one = spec(Warning::OneLine { at_prompt: true });
        assert_eq!((one.id.as_deref(), one.kind, one.cancel), (Some("NoSyncDoNotWarnBeforePastingOneLineEndingInNewlineAtShellPrompt"), Kind::Permanent, Some(2)));
        assert_eq!(spec(Warning::OneLine { at_prompt: false }).id.as_deref(), Some("NoSyncDoNotWarnBeforeMultilinePaste"));
        // A silenced warning still comes: its remembered answer is the preempt's.
        let p = WarnPrefs { size_limit: -1, ..Default::default() };
        assert_eq!(check("a\nb", Some(true), true, 0, &p), vec![Warning::Lines { n: 2, at_prompt: true }]);
    }

    #[test]
    fn configs_decode_like_iterm2() {
        let c = Config::decode(r#"{"Base64":true,"TabTransform":1,"TabStopSize":2,"Delay":0.5}"#);
        assert_eq!((c.base64, c.tab_transform, c.tab_stop_size, c.chunk_size, c.convert_newlines), (true, 1, 2, 0, false), "missing keys read as 0 and false");
        assert_eq!(Config::decode(&c.encode()), c);
        assert_eq!(c.summary(), "Base64, Tabs->2 spcs, Slow, UnsafeControls, BracketingOff, NoCRLFConversion");
        let (job, speed) = c.job("a\tb");
        assert_eq!((job.flags, job.tabs, speed), (BASE64, Tabs::Spaces(2), Speed { units: 0, delay: 0.5 }));
        let full = Config { convert_newlines: true, remove_controls: true, bracket_allowed: true, escape_for_shell: true, use_regex: true, regex: "a".into(), substitution: "b".into(), tab_transform: 2, ..Default::default() };
        assert_eq!(full.summary(), "^V+Tab, \\Escape, s/a/b/g");
        assert_eq!(full.job("x").0.flags, NEWLINES | CONTROLS | BRACKET | ESCAPE | REGEX);
        let fresh = Config::decode("");
        assert!(fresh.convert_newlines && fresh.convert_punctuation && fresh.remove_controls && fresh.bracket_allowed, "an empty setting is iTerm2's new action");
        assert!(!fresh.base64 && !fresh.escape_for_shell && !fresh.wait_for_prompts);
        assert_eq!(Config::decode("not json"), fresh);
    }

    #[test]
    fn paste_jobs_queue_in_order() {
        let mut q = Queue::default();
        q.push("abcd", Speed { units: 2, delay: 1.0 }, false);
        q.push("xy", Speed { units: 2, delay: 1.0 }, false);
        assert_eq!(q.tick(0.0, 0).as_deref(), Some("ab"));
        assert_eq!(q.progress(), Some((2, 4)));
        assert_eq!(q.tick(0.5, 0), None, "a second between writes");
        assert_eq!(q.next_due(), Some(1.0));
        assert_eq!(q.tick(1.0, 0).as_deref(), Some("cd"));
        assert_eq!(q.tick(1.0, 0).as_deref(), Some("xy"), "the next paste starts");
        assert!(!q.busy());
        q.push("ls\rpwd\r", Speed { units: 100, delay: 0.0 }, true);
        assert_eq!(q.tick(5.0, 3).as_deref(), Some("ls\r"));
        assert_eq!(q.tick(5.0, 3), None, "waits for the next prompt");
        assert_eq!(q.next_due(), None);
        assert_eq!(q.tick(5.0, 4).as_deref(), Some("pwd\r"));
        q.push("long text", Speed { units: 1, delay: 1.0 }, false);
        q.push("next", Speed { units: 10, delay: 1.0 }, false);
        q.tick(9.0, 0);
        q.cancel();
        assert_eq!(q.tick(9.0, 0).as_deref(), Some("next"), "Cancel starts the next one");
    }
}

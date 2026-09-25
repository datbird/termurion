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
    let wrap = |s: &str| format!("\x1b[200~{s}\x1b[201~");
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

#[cfg(test)]
mod tests {
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
        assert_eq!(bracket("ls\rpwd\r", true), "\x1b[200~ls\x1b[201~\r\x1b[200~pwd\x1b[201~\r");
        assert_eq!(bracket("a\r\nb", true), "\x1b[200~a\x1b[201~\r\n\x1b[200~b\x1b[201~");
    }
}

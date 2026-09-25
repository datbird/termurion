//! iTerm2's Advanced Paste window (iTermPasteSpecialWindowController and
//! iTermPasteSpecialViewController): the form's state, which controls work for the text,
//! the preview, the job it pastes, and the statistics, duration, and speed slider texts.

use crate::paste::{self, Job, Speed, Tabs};

/// The remembered settings (iTerm2's keys, their defaults).
#[derive(Clone, Debug, PartialEq)]
pub struct Defaults {
    pub chunk: usize,
    pub delay: f64,
    pub spaces: usize,
    /// 0 paste with tabs, 1 convert to spaces, 2 escape with ^V.
    pub tab_transform: i64,
    pub escape: bool,
    pub punctuation: bool,
    pub dos_newlines: bool,
    pub remove_controls: bool,
    pub bracket: bool,
    pub use_regex: bool,
    pub regex: String,
    pub substitution: String,
}

impl Default for Defaults {
    fn default() -> Defaults {
        Defaults { chunk: 1024, delay: 0.01, spaces: 4, tab_transform: 0, escape: false, punctuation: false, dos_newlines: true, remove_controls: true, bracket: true, use_regex: false, regex: String::new(), substitution: String::new() }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Form {
    pub tab_transform: i64,
    pub spaces: usize,
    pub escape: bool,
    pub convert_newlines: bool,
    pub remove_newlines: bool,
    pub base64: bool,
    pub punctuation: bool,
    pub use_regex: bool,
    pub regex: String,
    pub substitution: String,
    pub remove_controls: bool,
    pub bracket: bool,
    pub wait: bool,
    pub chunk: usize,
    pub delay: f64,
}

/// Which controls work for this text (the rest are dimmed).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Enabled {
    pub tabs: bool,
    pub spaces: bool,
    pub escape: bool,
    pub convert_newlines: bool,
    pub remove_newlines: bool,
    pub punctuation: bool,
    pub regex_fields: bool,
    pub remove_controls: bool,
    pub bracket: bool,
    pub wait: bool,
}

/// Where the program is: bracketed paste on, shell integration, at a prompt.
#[derive(Clone, Copy, Debug, Default)]
pub struct Session {
    pub bracketed: bool,
    pub shell_integration: bool,
    pub at_prompt: bool,
    /// AdvancedPasteWaitsForPromptByDefault.
    pub wait_default: bool,
}

impl Form {
    pub fn new(text: &str, d: &Defaults, s: Session) -> Form {
        Form {
            tab_transform: d.tab_transform,
            spaces: d.spaces,
            escape: d.escape,
            convert_newlines: text.contains('\n') && d.dos_newlines,
            remove_newlines: false,
            base64: false,
            punctuation: has_punctuation(text) && d.punctuation,
            use_regex: d.use_regex,
            regex: d.regex.clone(),
            substitution: d.substitution.clone(),
            remove_controls: has_unsafe(text) && d.remove_controls,
            bracket: s.bracketed && d.bracket,
            wait: s.at_prompt && s.shell_integration && s.wait_default,
            chunk: d.chunk,
            delay: d.delay,
        }
    }

    pub fn enabled(&self, text: &str, s: Session) -> Enabled {
        let tabs = text.contains('\t');
        Enabled {
            tabs,
            spaces: tabs && self.tab_transform == 1,
            escape: text.chars().any(|c| "\\ ()\"&'!$<>;|*?[]#`\t{}^+=@~".contains(c)),
            convert_newlines: text.contains('\n') && !self.remove_newlines,
            remove_newlines: text.contains(['\n', '\r']),
            punctuation: has_punctuation(text),
            regex_fields: self.use_regex,
            remove_controls: has_unsafe(text),
            bracket: s.bracketed,
            wait: s.shell_integration,
        }
    }

    /// The text shown and edited: newlines, controls, punctuation, escaping, regex, and
    /// base64 applied (tabs change at paste time).
    pub fn preview(&self, text: &str) -> String {
        let bits = [
            (self.convert_newlines, paste::NEWLINES),
            (self.remove_newlines, paste::REMOVE_NEWLINES),
            (self.remove_controls, paste::CONTROLS),
            (self.punctuation, paste::PUNCTUATION),
            (self.escape, paste::ESCAPE),
            (self.base64, paste::BASE64),
            (self.use_regex, paste::REGEX),
        ];
        let flags = bits.iter().filter(|(on, _)| *on).fold(0, |f, (_, b)| f | b);
        let job = Job { regex: self.regex.clone(), substitution: self.substitution.clone(), ..Job::new(text, flags) };
        paste::sanitize(&job, &paste::Prefs::default())
    }

    /// What Paste sends: the (edited) preview, with the paste-time steps.
    pub fn job(&self, preview: &str) -> (Job, Speed) {
        let bits = [
            (self.convert_newlines, paste::NEWLINES),
            (self.remove_newlines, paste::REMOVE_NEWLINES),
            (self.remove_controls, paste::CONTROLS),
            (self.punctuation, paste::PUNCTUATION),
            (self.bracket, paste::BRACKET),
            (self.wait, paste::COMMANDS),
        ];
        let flags = bits.iter().filter(|(on, _)| *on).fold(0, |f, (_, b)| f | b);
        let tabs = match self.tab_transform {
            1 => Tabs::Spaces(self.spaces),
            2 => Tabs::CtrlV,
            _ => Tabs::Keep,
        };
        (Job { tabs, ..Job::new(preview, flags) }, Speed { units: self.chunk.max(1), delay: self.delay })
    }

    /// The settings to remember: each only when its control was enabled.
    pub fn remember(&self, e: &Enabled, d: &mut Defaults) {
        if e.tabs {
            d.tab_transform = self.tab_transform;
            if self.tab_transform == 1 {
                d.spaces = self.spaces;
            }
        }
        if e.escape {
            d.escape = self.escape;
        }
        if e.punctuation {
            d.punctuation = self.punctuation;
        }
        if e.convert_newlines {
            d.dos_newlines = self.convert_newlines;
        }
        if e.remove_controls {
            d.remove_controls = self.remove_controls;
        }
        if e.bracket {
            d.bracket = self.bracket;
        }
        d.use_regex = self.use_regex;
        d.regex = self.regex.clone();
        d.substitution = self.substitution.clone();
        d.chunk = self.chunk;
        d.delay = self.delay;
    }
}

/// "12 bytes in 3 lines."
pub fn stats(preview: &str) -> String {
    let bytes = preview.encode_utf16().count();
    // iTerm2's numberOfLines: CR, LF, CRLF, and the Unicode line ends each end a line.
    let mut lines = 0;
    let mut chars = preview.chars().peekable();
    let mut open = false;
    while let Some(c) = chars.next() {
        open = true;
        if matches!(c, '\n' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}') {
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            lines += 1;
            open = false;
        }
    }
    lines += usize::from(open);
    let s = |n: usize| if n == 1 { "" } else { "s" };
    format!("{bytes} byte{} in {lines} line{}.", s(bytes), s(lines))
}

/// iTerm2's "Minimum duration" text.
pub fn duration(len: usize, s: Speed) -> String {
    let chunks = len / s.units.max(1);
    let mut d = chunks.saturating_sub(1) as f64 * s.delay + len as f64 / 2_000_000.0;
    if d > 1.0 {
        d = d.ceil();
    }
    if d < 0.01 {
        return "Instant".into();
    }
    // iTerm2's descriptionForDuration: two fraction digits at most, thousands grouped.
    let (n, unit) = if d < 1.0 { (d * 1000.0, "ms") } else if d < 60.0 { (d, "sec") } else { (d / 60.0, "min") };
    format!("{} {unit}", decimal(n))
}

/// A number with at most two fraction digits and grouped thousands ("1,234.5").
fn decimal(n: f64) -> String {
    let text = format!("{:.2}", (n * 100.0).round() / 100.0);
    let (whole, frac) = text.split_once('.').unwrap_or((&text, ""));
    let frac = frac.trim_end_matches('0');
    let digits: Vec<char> = whole.chars().collect();
    let mut grouped = String::new();
    for (i, c) in digits.iter().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(*c);
    }
    if frac.is_empty() { grouped } else { format!("{grouped}.{frac}") }
}

/// The chunk slider (0..1, logarithmic from 1 byte to 1 MB) and the delay slider (0..1,
/// logarithmic from 1 ms to 10 s).
pub fn slider_to_units(v: f32) -> usize {
    2f64.powf(20.0 * v.clamp(0.0, 1.0) as f64).round() as usize
}

pub fn units_to_slider(u: usize) -> f32 {
    ((u.max(1) as f64).log2() / 20.0).clamp(0.0, 1.0) as f32
}

pub fn slider_to_delay(v: f32) -> f64 {
    10f64.powf(4.0 * v.clamp(0.0, 1.0) as f64 - 3.0)
}

pub fn delay_to_slider(d: f64) -> f32 {
    ((d.max(0.001).log10() + 3.0) / 4.0).clamp(0.0, 1.0) as f32
}

fn has_unsafe(text: &str) -> bool {
    text.chars().any(|c| (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\x0c' | '\r'))
}

fn has_punctuation(text: &str) -> bool {
    text.chars().any(|c| matches!(c, '\u{201c}'..='\u{201f}' | '\u{301d}'..='\u{301f}' | '\u{ff02}' | '\u{2018}'..='\u{201b}' | '\u{ff07}' | '\u{2010}'..='\u{2015}' | '\u{207b}' | '\u{208b}' | '\u{2212}' | '\u{2e3a}' | '\u{2e3b}' | '\u{fe58}' | '\u{fe63}' | '\u{ff0d}' | '\u{ad}'))
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: Session = Session { bracketed: true, shell_integration: true, at_prompt: true, wait_default: true };

    #[test]
    fn advanced_paste_follows_iterm2() {
        let d = Defaults::default();
        let f = Form::new("ls -l\n", &d, AT);
        let e = f.enabled("ls -l\n", AT);
        assert!(!e.tabs && !e.spaces, "no tabs in the text");
        assert!(e.convert_newlines && f.convert_newlines && !f.remove_newlines);
        assert!(e.escape, "a space can be escaped");
        assert!(f.bracket && f.wait, "bracketed, and at a prompt with shell integration");
        assert!(!f.remove_controls, "nothing unsafe to remove");
        let mut r = Form { use_regex: true, regex: "l$".into(), substitution: "la".into(), ..f.clone() };
        r.convert_newlines = false;
        assert_eq!(r.preview("ls -l"), "ls -la");
        let (job, speed) = r.job("ls -la");
        assert_eq!(job.text, "ls -la");
        assert_eq!(job.flags & (paste::REGEX | paste::ESCAPE | paste::BASE64), 0, "applied in the preview already");
        assert_ne!(job.flags & paste::BRACKET, 0);
        assert_ne!(job.flags & paste::COMMANDS, 0);
        assert_eq!(speed, Speed { units: 1024, delay: 0.01 });
        let tabbed = Form::new("a\tb", &Defaults { tab_transform: 1, ..d.clone() }, AT);
        assert!(tabbed.enabled("a\tb", AT).spaces);
        assert_eq!(tabbed.preview("a\tb"), "a\tb", "tabs change at paste time");
        assert_eq!(tabbed.job("a\tb").0.tabs, Tabs::Spaces(4));
        let b64 = Form { base64: true, ..Form::new("hi", &d, AT) };
        assert_eq!(b64.preview("hi"), "aGk=\r");
        let plain = Form::new("x", &d, Session::default());
        assert!(!plain.bracket && !plain.wait && !plain.enabled("x", Session::default()).wait);
        let mut saved = Defaults::default();
        Form { escape: true, ..tabbed.clone() }.remember(&tabbed.enabled("ab", AT), &mut saved);
        assert_eq!((saved.tab_transform, saved.escape), (0, false), "controls that were off are not remembered");
        Form { escape: true, ..tabbed.clone() }.remember(&tabbed.enabled("a\tb", AT), &mut saved);
        assert_eq!((saved.tab_transform, saved.escape), (1, true));
        assert_eq!(stats("ab\ncd"), "5 bytes in 2 lines.");
        assert_eq!(stats("a"), "1 byte in 1 line.");
        assert_eq!(duration(100_000, Speed { units: 1024, delay: 0.01 }), "2 sec", "0.96 s + 0.05 s at 2 MB/s, rounded up");
        assert_eq!(duration(60_000, Speed { units: 1024, delay: 0.01 }), "600 ms");
        assert_eq!(stats("a\rb\r"), "4 bytes in 2 lines.", "CR ends a line too");
        assert_eq!(stats("a\r\nb"), "4 bytes in 2 lines.");
        assert_eq!(duration(10, Speed { units: 1024, delay: 0.01 }), "Instant");
        assert_eq!(duration(100_000, Speed { units: 16, delay: 0.125 }), "13.03 min", "iTerm2 switches to minutes at 60 s");
        assert_eq!((slider_to_units(0.0), slider_to_units(1.0)), (1, 1_048_576));
        assert!((units_to_slider(1024) - 0.5).abs() < 1e-6);
        assert!((slider_to_delay(0.0) - 0.001).abs() < 1e-9 && (slider_to_delay(1.0) - 10.0).abs() < 1e-9);
        assert!((delay_to_slider(0.1) - 0.5).abs() < 1e-6);
    }
}

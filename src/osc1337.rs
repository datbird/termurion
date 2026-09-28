//! Value grammars and replies for the OSC 1337 keys that are not files, marks, or the
//! clipboard: CursorShape, HighlightCursorLine, RequestAttention, ReportCellSize, the window
//! and screen frames, UnicodeVersion, SetColors, SetKeyLabel and its stack, Capabilities, Env.

use alacritty_terminal::vte::ansi::{CursorShape, Rgb};

/// NSString's `boolValue`, which iTerm2 uses: after spaces and an optional sign and zeros,
/// Y, y, T, t, or a digit 1 to 9 is true.
pub fn parse_bool(s: &str) -> bool {
    let t = s.trim_start();
    let t = t.strip_prefix(['+', '-']).unwrap_or(t);
    let t = t.trim_start_matches('0');
    t.chars().next().is_some_and(|c| matches!(c, 'Y' | 'y' | 'T' | 't' | '1'..='9'))
}

/// `CursorShape=N`: 0 block, 1 bar, 2 underline; a non-number counts as 0; others ignored.
pub fn cursor_shape(value: &str) -> Option<CursorShape> {
    match value.trim().parse::<i64>().unwrap_or(0) {
        0 => Some(CursorShape::Block),
        1 => Some(CursorShape::Beam),
        2 => Some(CursorShape::Underline),
        _ => None,
    }
}

/// iTerm2's `%0.2f` with trailing zeros (and a trailing point) removed.
/// How deep the UnicodeVersion and key-label stacks go, and how many labels a pane keeps:
/// past these the oldest go, so output cannot grow them without end.
const STACK_MAX: usize = 64;
const LABELS_MAX: usize = 64;

pub fn number(n: f64) -> String {
    let s = format!("{n:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".to_string() } else { s.to_string() }
}

/// RequestAttention's values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attention {
    Fireworks,
    /// Flash the taskbar button once.
    Once,
    /// Flash the pane.
    Flash,
    /// Flash the taskbar button until Tandem comes to the front.
    Start,
    /// Stop flashing.
    Stop,
}

pub fn attention(value: &str) -> Attention {
    match value {
        "fireworks" => Attention::Fireworks,
        "once" => Attention::Once,
        "flash" => Attention::Flash,
        v if parse_bool(v) => Attention::Start,
        _ => Attention::Stop,
    }
}

/// A color from SetColors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    Rgb(Rgb),
    /// `i:N`: palette entry N.
    Index(u8),
}

/// `RRGGBB` or `RGB`, with an optional `srgb:`, `rgb:`, or `p3:` prefix (p3 treated as
/// sRGB), or `i:N`.
pub fn color(s: &str) -> Option<Color> {
    if let Some(n) = s.strip_prefix("i:") {
        if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        return n.parse().ok().map(Color::Index);
    }
    let hex = ["srgb:", "rgb:", "p3:"].iter().find_map(|p| s.strip_prefix(p)).unwrap_or(s);
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let v = |t: &str| u8::from_str_radix(t, 16).ok();
    let rgb = match hex.len() {
        6 => Rgb { r: v(&hex[0..2])?, g: v(&hex[2..4])?, b: v(&hex[4..6])? },
        3 => {
            let d = |i: usize| v(&hex[i..i + 1]).map(|x| x * 17);
            Rgb { r: d(0)?, g: d(1)?, b: d(2)? }
        }
        _ => return None,
    };
    Some(Color::Rgb(rgb))
}

/// `SetColors=name=color,name=color`: the pairs, empty names or values skipped.
pub fn set_colors(value: &str) -> Vec<(String, String)> {
    value
        .split(',')
        .filter_map(|pair| pair.split_once('='))
        .filter(|(n, v)| !n.is_empty() && !v.is_empty())
        .map(|(n, v)| (n.to_string(), v.to_string()))
        .collect()
}

/// UnicodeVersion's state: the pane's version (0 for the profile's) and the push stack.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Unicode {
    pub version: u8,
    stack: Vec<(String, u8)>,
}

impl Unicode {
    /// `N`, `push [label]`, or `pop [label]`.
    pub fn apply(&mut self, value: &str) {
        let (word, label) = value.split_once(' ').unwrap_or((value, ""));
        match word {
            "push" => {
                if self.stack.len() >= STACK_MAX {
                    self.stack.remove(0);
                }
                self.stack.push((label.to_string(), self.version));
            }
            "pop" => {
                while let Some((l, v)) = self.stack.pop() {
                    if label.is_empty() || l == label {
                        self.version = v;
                        break;
                    }
                }
            }
            n => match n.trim().parse::<u8>() {
                Ok(v @ (0 | 8 | 9)) => self.version = v,
                _ => {}
            },
        }
    }
}

/// SetKeyLabel's labels and the Push/PopKeyLabels stack. Windows has no Touch Bar, so
/// they are kept, not shown.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyLabels {
    pub current: std::collections::BTreeMap<String, String>,
    stack: Vec<(String, std::collections::BTreeMap<String, String>)>,
}

impl KeyLabels {
    /// `key=label` (an empty label removes it).
    pub fn set(&mut self, value: &str) {
        let Some((key, label)) = value.split_once('=') else { return };
        if key.is_empty() {
            return;
        }
        if label.is_empty() {
            self.current.remove(key);
        } else if self.current.len() < LABELS_MAX || self.current.contains_key(key) {
            self.current.insert(key.to_string(), label.to_string());
        }
    }

    /// Saves the labels; they are cleared unless the name starts with `.`.
    pub fn push(&mut self, name: &str) {
        if self.stack.len() >= STACK_MAX {
            self.stack.remove(0);
        }
        self.stack.push((name.to_string(), self.current.clone()));
        if !name.starts_with('.') {
            self.current.clear();
        }
    }

    /// Pops to the entry with this name (or the top one for an empty name).
    pub fn pop(&mut self, name: &str) {
        while let Some((n, labels)) = self.stack.pop() {
            if name.is_empty() || n == name {
                self.current = labels;
                break;
            }
        }
    }
}

/// What Tandem can do, for the Capabilities report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Features {
    pub clipboard_writable: bool,
    pub ambiguous_wide: bool,
    pub unicode_version: u8,
}

/// A profile's "Unicode Version" as a session starts with it: 8, else 9.
pub fn profile_unicode(value: Option<i64>) -> u8 {
    if value == Some(8) { 8 } else { 9 }
}

/// A session's features, iTerm2's one `capabilities` behind both `TERM_FEATURES` and the
/// `Capabilities` reply (`PS:3349`, `:18361-18386`): the clipboard setting, ambiguous width
/// narrow, and the session's Unicode version (the one a `UnicodeVersion` sequence set, else
/// the profile's).
pub fn session_features(clipboard_writable: bool, set_version: u8, profile_version: u8) -> Features {
    Features { clipboard_writable, ambiguous_wide: false, unicode_version: if set_version == 0 { profile_version } else { set_version } }
}

/// `Capabilities=` codes in iTerm2's order, for Tandem's features.
pub fn capabilities(f: Features) -> String {
    let mut out = String::from("T3");
    if f.clipboard_writable {
        out += "Cw";
    }
    // Mouse, DECSCUSR 0 to 6, Unicode.
    out += "MSc7U";
    if f.ambiguous_wide {
        out += "Aw";
    }
    out += &format!("Uw{}", f.unicode_version);
    // Title stack and title set, bracketed paste, focus, strikethrough, synchronized
    // output, hyperlinks, sixel, files. Not yet: DECSLRM, overline, notifications, progress.
    out += "Ts3BFGsSyHSxF";
    out
}

/// A rectangle in Windows virtual-screen pixels (y down) as AppKit points: y up from the
/// bottom of the primary monitor, divided by the scale.
pub fn to_points(x: f64, y: f64, w: f64, h: f64, primary_height_px: f64, scale: f64) -> (f64, f64, f64, f64) {
    (x / scale, (primary_height_px - y - h) / scale, w / scale, h / scale)
}

/// The reverse of `to_points`.
pub fn from_points(x: f64, y: f64, w: f64, h: f64, primary_height_px: f64, scale: f64) -> (f64, f64, f64, f64) {
    let (w, h) = (w * scale, h * scale);
    (x * scale, primary_height_px - y * scale - h, w, h)
}

/// What `Env=` does to the screen's exfiltrated environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnvUpdate {
    /// Not a `report=all` request: the stored environment stays.
    Ignore,
    /// Replaces the stored environment wholesale; None after a failed decode, as iTerm2
    /// assigns nil.
    Set(Option<Vec<(String, String)>>),
}

/// iTerm2's `keyValuePair`: split at the first `=`; without one, the value is empty.
pub fn split_pair(s: &str) -> (String, String) {
    match s.split_once('=') {
        Some((k, v)) => (k.to_string(), v.to_string()),
        None => (s.to_string(), String::new()),
    }
}

/// `Env=<params>:<base64>`, as iTerm2's `terminalDidExfiltrateEnvironment` reads it: the
/// params are split on `;` and must hold `report=all`; the payload is decoded (CR and LF
/// dropped, padding optional) and split into `NAME=value` lines on every newline
/// character, so `\r\n` gives an empty line between.
pub fn parse_env(value: &str) -> EnvUpdate {
    let Some((params, payload)) = value.split_once(':') else { return EnvUpdate::Ignore };
    if !params.split(';').map(split_pair).any(|(k, v)| k == "report" && v == "all") {
        return EnvUpdate::Ignore;
    }
    let Some(text) = decode_env(payload) else { return EnvUpdate::Set(None) };
    let newline = |c: char| matches!(c, '\u{a}'..='\u{d}' | '\u{85}' | '\u{2028}' | '\u{2029}');
    EnvUpdate::Set(Some(text.split(newline).map(split_pair).collect()))
}

/// The payload's text: None when it is empty, not base64, or not UTF-8.
fn decode_env(payload: &str) -> Option<String> {
    use base64::Engine;
    let compact: String = payload.chars().filter(|c| !matches!(c, '\r' | '\n')).collect();
    let e = &base64::engine::general_purpose::STANDARD;
    let bytes = e.decode(&compact).ok().or_else(|| base64::engine::general_purpose::STANDARD_NO_PAD.decode(compact.trim_end_matches('=')).ok())?;
    if bytes.is_empty() {
        return None;
    }
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::{Request, Screen};
    use alacritty_terminal::grid::Dimensions;

    #[test]
    fn cursor_shape_and_clear_scrollback() {
        assert_eq!(cursor_shape("1"), Some(CursorShape::Beam));
        assert_eq!(cursor_shape("2"), Some(CursorShape::Underline));
        assert_eq!(cursor_shape("x"), Some(CursorShape::Block), "not a number counts as 0");
        assert_eq!(cursor_shape("7"), None);
        let mut s = Screen::with_scrollback(10, 3, 100);
        s.feed(b"\x1b]1337;CursorShape=1\x07");
        assert_eq!(s.term().cursor_style().shape, CursorShape::Beam);
        for i in 0..10 {
            s.feed(format!("line{i}\r\n").as_bytes());
        }
        s.feed(b"$ \x1b]1337;ClearScrollback\x07");
        assert_eq!(s.term().grid().history_size(), 0);
        let row0: String = (0..10).map(|c| s.term().grid()[alacritty_terminal::index::Line(0)][alacritty_terminal::index::Column(c)].c).collect();
        assert_eq!(row0.trim_end(), "$", "the prompt line stays");
    }

    #[test]
    fn set_colors_parse() {
        assert_eq!(set_colors("fg=ff0000,bg=0f0,,x=,=1"), vec![("fg".to_string(), "ff0000".to_string()), ("bg".to_string(), "0f0".to_string())]);
        assert_eq!(color("ff8000"), Some(Color::Rgb(Rgb { r: 255, g: 128, b: 0 })));
        assert_eq!(color("0f0"), Some(Color::Rgb(Rgb { r: 0, g: 255, b: 0 })));
        assert_eq!(color("p3:000080"), Some(Color::Rgb(Rgb { r: 0, g: 0, b: 128 })));
        assert_eq!(color("srgb:010203"), Some(Color::Rgb(Rgb { r: 1, g: 2, b: 3 })));
        assert_eq!(color("i:12"), Some(Color::Index(12)));
        assert_eq!(color("i:300"), None);
        assert_eq!(color("zzz"), None);
        let mut s = Screen::new(10, 3);
        s.feed(b"\x1b]1337;SetColors=bg=102030,red=ff0000,selbg=i:4\x07");
        assert_eq!(s.palette.bg, Rgb { r: 0x10, g: 0x20, b: 0x30 });
        assert_eq!(s.palette.ansi[1], Rgb { r: 255, g: 0, b: 0 });
        assert_eq!(s.palette.selection, s.palette.ansi[4]);
        // A profile change keeps what the program set.
        s.set_palette(crate::palette::Palette::default());
        assert_eq!(s.palette.bg, Rgb { r: 0x10, g: 0x20, b: 0x30 });
    }

    #[test]
    fn unicode_version_stack() {
        let mut u = Unicode::default();
        u.apply("9");
        assert_eq!(u.version, 9);
        u.apply("7");
        assert_eq!(u.version, 9, "only 8 or 9");
        u.apply("push outer");
        u.apply("8");
        u.apply("push inner");
        u.apply("0");
        assert_eq!(u.version, 0);
        u.apply("pop outer");
        assert_eq!(u.version, 9, "pops to the labeled entry");
        u.apply("pop");
        assert_eq!(u.version, 9, "empty stack: nothing");
        u.apply("push");
        u.apply("8");
        u.apply("pop");
        assert_eq!(u.version, 9);
    }

    #[test]
    fn key_label_stack() {
        let mut k = KeyLabels::default();
        k.set("F1=Help");
        k.set("status=Busy");
        k.push("vim");
        assert!(k.current.is_empty(), "a push clears");
        k.set("F2=Save");
        k.push(".keep");
        assert_eq!(k.current.get("F2").map(String::as_str), Some("Save"), "a name starting with . keeps them");
        k.pop("vim");
        assert_eq!(k.current.get("F1").map(String::as_str), Some("Help"));
        k.set("F1=");
        assert!(!k.current.contains_key("F1"), "an empty label removes");
        k.set("=x");
        assert!(!k.current.contains_key(""));
    }

    #[test]
    fn one_unicode_version_per_session() {
        assert_eq!(profile_unicode(None), 9);
        assert_eq!(profile_unicode(Some(8)), 8);
        assert_eq!(profile_unicode(Some(9)), 9);
        assert_eq!(profile_unicode(Some(12)), 9, "only iTerm2's two versions");
        assert_eq!(session_features(true, 0, 8), Features { clipboard_writable: true, ambiguous_wide: false, unicode_version: 8 }, "the profile's until a sequence sets one");
        assert_eq!(session_features(false, 9, 8).unicode_version, 9, "a set version wins");
        assert_eq!(session_features(false, 8, 9).unicode_version, 8);
        assert_eq!(session_features(false, 0, 9).unicode_version, 9);
    }

    #[test]
    fn capabilities_follow_features() {
        let f = Features { clipboard_writable: true, ambiguous_wide: false, unicode_version: 9 };
        assert_eq!(capabilities(f), "T3CwMSc7UUw9Ts3BFGsSyHSxF");
        let f = Features { clipboard_writable: false, ambiguous_wide: true, unicode_version: 8 };
        assert_eq!(capabilities(f), "T3MSc7UAwUw8Ts3BFGsSyHSxF");
        let mut s = Screen::new(10, 3);
        assert!(String::from_utf8(s.feed(b"\x1b]1337;Capabilities\x07")).unwrap().starts_with("\x1b]1337;Capabilities=T3"));
        // The reply and TERM_FEATURES agree: the profile's version until a sequence sets one.
        s.profile_unicode = 8;
        s.clipboard_allowed = true;
        let want = |v: u8| format!("\x1b]1337;Capabilities={}\x07", capabilities(session_features(true, v, 8)));
        assert_eq!(String::from_utf8(s.feed(b"\x1b]1337;Capabilities\x07")).unwrap(), want(0));
        assert!(want(0).contains("Uw8"));
        s.feed(b"\x1b]1337;UnicodeVersion=9\x07");
        assert_eq!(String::from_utf8(s.feed(b"\x1b]1337;Capabilities\x07")).unwrap(), want(9));
        s.trusted = false;
        assert!(s.feed(b"\x1b]1337;Capabilities\x07").is_empty(), "needs trust");
    }

    #[test]
    fn number_format_matches_iterm2() {
        assert_eq!(number(17.0), "17");
        assert_eq!(number(7.5), "7.5");
        assert_eq!(number(2.0), "2");
        assert_eq!(number(8.333333), "8.33");
        assert_eq!(number(0.0), "0");
        assert_eq!(number(-12.25), "-12.25");
        assert!(parse_bool("yes") && parse_bool("True") && parse_bool("1") && parse_bool("  +02"));
        assert!(!parse_bool("no") && !parse_bool("0") && !parse_bool("false") && !parse_bool(""));
    }

    #[test]
    fn frames_flip_to_appkit_points() {
        // A 1600x900 window at (100, 50) on a 2160 px tall primary monitor at 200%.
        assert_eq!(to_points(100.0, 50.0, 1600.0, 900.0, 2160.0, 2.0), (50.0, 605.0, 800.0, 450.0));
        assert_eq!(from_points(50.0, 605.0, 800.0, 450.0, 2160.0, 2.0), (100.0, 50.0, 1600.0, 900.0));
    }

    #[test]
    fn osc133_e_to_h_are_ignored() {
        let mut s = Screen::new(10, 3);
        let reply = s.feed(b"\x1b]133;E;1\x07a\x1b]133;F;1\x07\x1b]133;G;50;x\x07\x1b]133;H;QUIT\x07b");
        assert!(reply.is_empty());
        assert!(s.take_requests().is_empty());
        let row: String = (0..10).map(|c| s.term().grid()[alacritty_terminal::index::Line(0)][alacritty_terminal::index::Column(c)].c).collect();
        assert_eq!(row.trim_end(), "ab");
    }

    #[test]
    fn requests_reach_the_app() {
        let mut s = Screen::new(10, 3);
        s.feed(b"\x1b]1337;HighlightCursorLine\x07\x1b]1337;HighlightCursorLine=no\x07");
        s.feed(b"\x1b]1337;RequestAttention=once\x07\x1b]1337;RequestAttention=fireworks\x07\x1b]1337;RequestAttention=yes\x07\x1b]1337;RequestAttention=no\x07\x1b]1337;RequestAttention=flash\x07");
        s.feed(b"\x1b]1337;StealFocus\x07\x1b]1337;ReportCellSize\x07\x1b]1337;ReportWindowFrame\x07\x1b]1337;ReportScreenFrames\x07");
        s.feed(b"\x1b]1337;SetWindowFrame=1;2;3;4\x07\x1b]1337;SetWindowFrame=1;2;3\x07\x1b]1337;SetColors=tab=ff0000\x07\x1b]1337;SetColors=tab=default\x07");
        assert_eq!(
            s.take_requests(),
            vec![
                Request::CursorGuide(true),
                Request::CursorGuide(false),
                Request::Attention(Attention::Once),
                Request::Attention(Attention::Fireworks),
                Request::Attention(Attention::Start),
                Request::Attention(Attention::Stop),
                Request::Attention(Attention::Flash),
                Request::StealFocus,
                Request::ReportCellSize,
                Request::ReportWindowFrame,
                Request::ReportScreenFrames,
                Request::SetWindowFrame(1.0, 2.0, 3.0, 4.0),
                Request::TabColor(Some(Rgb { r: 255, g: 0, b: 0 })),
                Request::TabColor(None),
            ]
        );
        s.trusted = false;
        s.feed(b"\x1b]1337;StealFocus\x07\x1b]1337;ReportScreenFrames\x07\x1b]1337;Env=report=all:QT1i\x07");
        assert_eq!(s.take_requests(), vec![], "trusted only");
        assert_eq!(s.env, None, "Env is trusted only");
        s.trusted = true;
        s.feed(b"\x1b]1337;Env=report=all:QT1i\x07\x1b]1337;ExecFailed\x07\x1b]1337;UnicodeVersion=8\x07\x1b]1337;SetKeyLabel=F1=Go\x07");
        assert_eq!(s.env, Some(vec![("A".into(), "b".into())]));
        assert!(s.exec_failed);
        assert_eq!(s.unicode.version, 8);
        assert_eq!(s.key_labels.current.get("F1").map(String::as_str), Some("Go"));
        s.feed(b"\x1b]1337;Env=report=all:!!!!\x07");
        assert_eq!(s.env, None, "a failed decode assigns nil, as iTerm2 does");
    }

    fn set(pairs: &[(&str, &str)]) -> EnvUpdate {
        EnvUpdate::Set(Some(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()))
    }

    fn rows(s: &Screen) -> Vec<String> {
        let g = s.term().grid();
        (0..g.screen_lines() as i32).map(|l| s.line_text(l)).collect()
    }

    #[test]
    fn handshake_requests_follow_iterm2() {
        use crate::ssh_handshake::{Handshake, FUTURE, OUT_OF_DATE};
        let mut s = Screen::new(80, 24);
        s.trusted = false;
        s.feed(b"\x1b]1337;it2ssh=a b c d\x07");
        assert_eq!(s.take_requests(), vec![Request::SshWillBegin]);
        s.feed(b"\x1b]1337;SendConductor=v=3\x07");
        let h = Handshake { token: "a".into(), unique_id: "b".into(), encoded_ba: "c".into(), ssh_args: "d".into() };
        assert_eq!(s.take_requests(), vec![Request::SshSendConductor(h)]);
        s.feed(b"\x1b]1337;SendConductor=v=3\x07");
        assert!(s.take_requests().is_empty(), "the flags were taken");
        s.feed(b"\x1b]1337;it2ssh=a b c\x07");
        assert!(s.take_requests().is_empty(), "too few flags");
        s.feed(b"\x1b]1337;SendConductor=v=3\x07");
        assert!(s.take_requests().is_empty(), "no flags");
        assert!(rows(&s).iter().all(|r| r.trim().is_empty()), "nothing shown so far");
        s.feed(b"\x1b]1337;it2ssh=a b c d\x07\x1b]1337;SendConductor=v=2\x07");
        assert_eq!(s.take_requests(), vec![Request::SshWillBegin]);
        assert!(rows(&s)[..2].iter().any(|r| r.trim_end() == OUT_OF_DATE));
        s.feed(b"\x1b]1337;SendConductor=v=3\x07");
        assert!(s.take_requests().is_empty(), "a banner clears the flags");
        s.feed(b"\x1b]1337;it2ssh=a b c d\x07\x1b]1337;SendConductor=v=4\x07");
        assert_eq!(s.take_requests(), vec![Request::SshWillBegin]);
        assert!(rows(&s).iter().any(|r| r.trim_end() == FUTURE));
    }

    #[test]
    fn end_ssh_request_follows_iterm2() {
        let mut s = Screen::new(80, 24);
        s.trusted = false;
        s.feed(b"\x1b]1337;EndSSH=123\x07");
        assert!(s.take_requests().is_empty(), "EndSSH needs trust");
        s.trusted = true;
        s.feed(b"\x1b]1337;EndSSH=123\x07");
        assert_eq!(s.take_requests(), vec![Request::SshEnd("123".into())]);
        s.feed(b"\x1b]1337;EndSSH=\x07");
        assert!(s.take_requests().is_empty(), "an empty id is ignored");
        s.feed(b"\x1b]1337;it2ssh=a b c d\x07\x1b]1337;EndSSH=1\x07\x1b]1337;SendConductor=v=3\x07");
        assert_eq!(s.take_requests(), vec![Request::SshWillBegin, Request::SshEnd("1".into())], "EndSSH clears the flags");
    }

    #[test]
    fn banner_inside_dispatch_is_safe() {
        let mut s = Screen::new(80, 24);
        s.feed(b"\x1b]1337;it2ssh=a b c d\x07\x1b]1337;SendConductor=v=2\x07\x1b]1337;CurrentDir=/tm");
        s.feed(b"p\x07");
        assert!(rows(&s).iter().any(|r| r.trim_end() == crate::ssh_handshake::OUT_OF_DATE));
        assert_eq!(s.shell.cwd.as_deref(), Some("/tmp"), "the OSC split around the banner survived");
        assert!(s.requests.contains(&Request::SshWillBegin));
    }

    #[test]
    fn env_follows_iterm2() {
        assert_eq!(
            parse_env("report=all:SE9NRT0vVXNlcnMveApBPWI9YwpOT0VRCg=="),
            set(&[("HOME", "/Users/x"), ("A", "b=c"), ("NOEQ", ""), ("", "")])
        );
        assert_eq!(parse_env("report=all:QT0xDQpCPTI="), set(&[("A", "1"), ("", ""), ("B", "2")]));
        assert_eq!(parse_env("report=all:QT0x4oCoQj0yC0M9Mw=="), set(&[("A", "1"), ("B", "2"), ("C", "3")]));
        assert_eq!(parse_env("x=1;report=all;flag:QT0x"), set(&[("A", "1")]));
        assert_eq!(parse_env("report=all:QT0\r\nx"), set(&[("A", "1")]));
        for v in ["report=some:QT0x", "report:QT0x", "report=all", "QT0x"] {
            assert_eq!(parse_env(v), EnvUpdate::Ignore, "{v}");
        }
        for v in ["report=all:", "report=all:!!!!", "report=all:/w=="] {
            assert_eq!(parse_env(v), EnvUpdate::Set(None), "{v}");
        }
    }

    #[test]
    fn split_pair_follows_iterm2() {
        assert_eq!(split_pair("a=b=c"), ("a".to_string(), "b=c".to_string()));
        assert_eq!(split_pair("x"), ("x".to_string(), String::new()));
        assert_eq!(split_pair("=v"), (String::new(), "v".to_string()));
    }
    #[test]
    fn stacks_and_labels_are_bounded() {
        let mut k = KeyLabels::default();
        for i in 0..1000 {
            k.set(&format!("F{i}=x"));
        }
        assert!(k.current.len() <= 64, "{}", k.current.len());
        for i in 0..1000 {
            k.push(&format!(".n{i}"));
        }
        assert!(k.stack.len() <= 64);
        let mut u = Unicode::default();
        for _ in 0..1000 {
            u.apply("push x");
        }
        assert!(u.stack.len() <= 64);
    }
}

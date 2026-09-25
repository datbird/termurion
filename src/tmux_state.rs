//! A tmux pane's terminal state (iTerm2's TmuxStateParser and setTmuxState), and the bytes
//! that rebuild a pane from tmux's captures: history, the other screen, modes, cursor, and
//! the output tmux had not finished sending.

/// iTerm2's list-panes format: `key=#{key}` joined by tabs.
pub const KEYS: [&str; 21] = [
    "pane_id",
    "alternate_on",
    "alternate_saved_x",
    "alternate_saved_y",
    "cursor_x",
    "cursor_y",
    "scroll_region_upper",
    "scroll_region_lower",
    "pane_tabs",
    "cursor_flag",
    "insert_flag",
    "keypad_cursor_flag",
    "keypad_flag",
    "wrap_flag",
    "mouse_standard_flag",
    "mouse_button_flag",
    "mouse_any_flag",
    "mouse_utf8_flag",
    "mouse_sgr_flag",
    "bracket_paste_flag",
    "pane_key_mode",
];

pub fn format() -> String {
    KEYS.iter().map(|k| format!("{k}=#{{{k}}}")).collect::<Vec<_>>().join("\t")
}

/// One pane's line, as key and value text. A missing key reads as 0, as `[nil intValue]`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PaneState {
    values: Vec<(String, String)>,
}

impl PaneState {
    fn get(&self, key: &str) -> Option<&str> {
        self.values.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// NSString intValue: leading spaces, a sign, digits; anything else is 0.
    fn int(&self, key: &str) -> i64 {
        let Some(v) = self.get(key) else { return 0 };
        let v = v.trim_start();
        let (neg, v) = match v.as_bytes().first() {
            Some(b'-') => (true, &v[1..]),
            Some(b'+') => (false, &v[1..]),
            _ => (false, v),
        };
        let n: i64 = v.bytes().take_while(u8::is_ascii_digit).fold(0i64, |n, d| n.saturating_mul(10).saturating_add((d - b'0') as i64));
        if neg { -n } else { n }
    }

    fn flag(&self, key: &str) -> Option<bool> {
        self.get(key).map(|_| self.int(key) != 0)
    }

    pub fn pane(&self) -> Option<i64> {
        self.get("pane_id").map(|v| match v.strip_prefix('%') {
            Some(n) if !n.is_empty() => PaneState { values: vec![("n".into(), n.into())] }.int("n"),
            _ => -1,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

fn parse_line(line: &str, tab_bug: bool) -> PaneState {
    let mut fields: Vec<&str> = line.split('\t').collect();
    if fields.len() == 1 && tab_bug {
        fields = line.split("\\t").collect();
    }
    let values = fields.into_iter().filter_map(|kv| kv.split_once('=')).filter(|(k, _)| !k.is_empty()).map(|(k, v)| (k.to_string(), v.to_string())).collect();
    PaneState { values }
}

/// The line for `pane` in a list-panes reply, or an empty state.
pub fn parse(reply: &str, pane: u32, tab_bug: bool) -> PaneState {
    reply.split('\n').map(|l| parse_line(l, tab_bug)).find(|s| s.pane() == Some(pane as i64)).unwrap_or_default()
}

/// `capture-pane -p -P -C`: only octal escapes of control codes are decoded (tmux escapes
/// control characters but not backslashes), as iTerm2's getPendingOutputResponse.
pub fn pending_bytes(capture: &[u8]) -> Vec<u8> {
    let octal = |b: &[u8]| -> Option<u8> {
        let mut v = 0u32;
        for &c in b.get(..3)? {
            if !(b'0'..=b'7').contains(&c) {
                return None;
            }
            v = v * 8 + (c - b'0') as u32;
        }
        Some(v as u8).filter(|_| v < 0x20)
    };
    let mut out = Vec::with_capacity(capture.len());
    let mut i = 0;
    while i < capture.len() {
        let c = capture[i];
        if c == b'\\' && capture.len() >= i + 4 {
            if let Some(v) = octal(&capture[i + 1..]) {
                out.push(v);
                i += 4;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Capture lines as terminal output: CRLF between lines, attributes reset at the end.
fn lines_bytes(lines: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if i > 0 {
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(l);
    }
    out.extend_from_slice(b"\x1b[0m");
    out
}

fn split_lines(capture: &[u8]) -> Vec<&[u8]> {
    if capture.is_empty() {
        return Vec::new();
    }
    capture.split(|b| *b == b'\n').collect()
}

/// The bytes that rebuild a pane of `rows` rows: `primary` is `capture-pane -peqJ` (history
/// and the current screen), `alternate` is the same with `-a` (tmux's saved grid). With the
/// alternate screen on, tmux's current screen is the alternate one and its saved grid is
/// the primary one, so they trade places (iTerm2 swaps the grids).
pub fn restore_bytes(state: &PaneState, primary: &[u8], alternate: &[u8], rows: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let alt_on = state.int("alternate_on") != 0;
    let cup = |x: i64, y: i64| format!("\x1b[{};{}H", y.max(0) + 1, x.max(0) + 1).into_bytes();
    if alt_on {
        let current = split_lines(primary);
        let cut = current.len().saturating_sub(rows);
        let (history, alt_screen) = current.split_at(cut);
        let saved = split_lines(alternate);
        out.extend(lines_bytes(&[history, &saved[..]].concat()));
        // The primary cursor goes where the alternate screen saved it.
        if state.get("alternate_saved_x").is_some() && state.get("alternate_saved_y").is_some() {
            out.extend(cup(state.int("alternate_saved_x"), state.int("alternate_saved_y")));
        }
        // ?1049h keeps the cursor where it is: the alternate screen starts at home.
        out.extend_from_slice(b"\x1b[?1049h\x1b[H");
        out.extend(lines_bytes(alt_screen));
    } else {
        out.extend(lines_bytes(&split_lines(primary)));
    }
    if state.is_empty() {
        return out;
    }
    // Scroll region first: DECSTBM homes the cursor.
    let (top, bottom) = (state.int("scroll_region_upper"), state.int("scroll_region_lower"));
    out.extend(format!("\x1b[{};{}r", top + 1, bottom + 1).into_bytes());
    // Tab stops: tmux's, then every 8 columns past the last (iTerm2 fills to 1000).
    out.extend_from_slice(b"\x1b[3g");
    let mut tabs: Vec<i64> = state.get("pane_tabs").map(|v| v.split(',').map(|s| PaneState { values: vec![("n".into(), s.into())] }.int("n")).collect()).unwrap_or_default();
    let max = tabs.iter().copied().max().unwrap_or(0);
    tabs.extend((0..1000).step_by(8).filter(|i| *i > max));
    for t in tabs {
        out.extend(cup(t, 0));
        out.extend_from_slice(b"\x1bH");
    }
    let mode = |on: bool, n: &str| format!("\x1b[?{n}{}", if on { 'h' } else { 'l' }).into_bytes();
    if let Some(on) = state.flag("cursor_flag") {
        out.extend(mode(on, "25"));
    }
    out.extend(format!("\x1b[4{}", if state.int("insert_flag") != 0 { 'h' } else { 'l' }).into_bytes());
    out.extend(mode(state.int("keypad_cursor_flag") != 0, "1"));
    out.extend_from_slice(if state.int("keypad_flag") != 0 { b"\x1b=" } else { b"\x1b>" });
    if let Some(on) = state.flag("wrap_flag") {
        out.extend(mode(on, "7"));
    }
    // One mouse mode: standard, then button, then any (iTerm2's final pass).
    for n in ["1000", "1002", "1003"] {
        out.extend(mode(false, n));
    }
    if state.int("mouse_standard_flag") != 0 {
        out.extend(mode(true, "1000"));
    } else if state.int("mouse_button_flag") != 0 {
        out.extend(mode(true, "1002"));
    } else if state.int("mouse_any_flag") != 0 {
        out.extend(mode(true, "1003"));
    }
    out.extend(mode(false, "1005"));
    out.extend(mode(false, "1006"));
    if state.int("mouse_sgr_flag") != 0 {
        out.extend(mode(true, "1006"));
    } else if state.int("mouse_utf8_flag") != 0 {
        out.extend(mode(true, "1005"));
    }
    out.extend(mode(state.int("bracket_paste_flag") != 0, "2004"));
    let key_mode = match state.get("pane_key_mode") {
        Some("Ext 1") => Some(1),
        Some("Ext 2") => Some(2),
        Some("VT10x") => Some(0),
        _ => None,
    };
    if let Some(n) = key_mode {
        out.extend(format!("\x1b[>4;{n}m").into_bytes());
    }
    out.extend(cup(state.int("cursor_x"), state.int("cursor_y")));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::Screen;
    use alacritty_terminal::index::{Column, Line};
    use alacritty_terminal::term::TermMode;

    fn line(pairs: &[(&str, &str)]) -> String {
        pairs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("\t")
    }

    #[test]
    fn pane_state_parses() {
        assert!(format().starts_with("pane_id=#{pane_id}\talternate_on=#{alternate_on}\t"));
        assert!(format().ends_with("bracket_paste_flag=#{bracket_paste_flag}\tpane_key_mode=#{pane_key_mode}"));
        let reply = format!("{}\n{}", line(&[("pane_id", "%3"), ("cursor_x", "4")]), line(&[("pane_id", "%7"), ("cursor_x", "9")]));
        let s = parse(&reply, 7, false);
        assert_eq!((s.pane(), s.int("cursor_x")), (Some(7), 9));
        assert!(parse(&reply, 8, false).is_empty(), "no line for the pane");
        let bug = "pane_id=%2\\tcursor_y=5";
        assert_eq!(parse(bug, 2, true).int("cursor_y"), 5, "tmux next-3.4 printed \\t");
        assert_eq!(parse(bug, 2, false).int("cursor_y"), 0, "without the workaround it is one field; intValue still reads the pane");
        assert_eq!(parse("pane_id=%1\t=x\tcursor_x=12abc", 1, false).int("cursor_x"), 12, "intValue reads the leading digits");
    }

    #[test]
    fn pane_state_restores_modes() {
        let state = parse(
            &line(&[
                ("pane_id", "%1"),
                ("alternate_on", "0"),
                ("cursor_x", "3"),
                ("cursor_y", "1"),
                ("scroll_region_upper", "0"),
                ("scroll_region_lower", "3"),
                ("pane_tabs", "4,10"),
                ("cursor_flag", "0"),
                ("keypad_cursor_flag", "1"),
                ("mouse_button_flag", "1"),
                ("mouse_sgr_flag", "1"),
                ("bracket_paste_flag", "1"),
                ("wrap_flag", "1"),
            ]),
            1,
            false,
        );
        let mut s = Screen::new(20, 4);
        s.feed(&restore_bytes(&state, b"one\ntwo\n\x1b[1mbold", b"", 4));
        let mode = *s.term().mode();
        assert!(mode.contains(TermMode::APP_CURSOR));
        assert!(mode.contains(TermMode::MOUSE_DRAG) && mode.contains(TermMode::SGR_MOUSE));
        assert!(mode.contains(TermMode::BRACKETED_PASTE));
        assert!(!mode.contains(TermMode::SHOW_CURSOR));
        let g = s.term().grid();
        assert_eq!((g.cursor.point.line.0, g.cursor.point.column.0), (1, 3));
        assert_eq!(g[Line(1)][Column(0)].c, 't');
        // The alternate screen: tmux's current capture is history plus that screen.
        let alt = parse(&line(&[("pane_id", "%1"), ("alternate_on", "1"), ("alternate_saved_x", "2"), ("alternate_saved_y", "0"), ("cursor_x", "0"), ("cursor_y", "0")]), 1, false);
        let mut s = Screen::new(20, 2);
        s.feed(&restore_bytes(&alt, b"old\nVIM1\nVIM2", b"prompt\n", 2));
        assert!(s.term().mode().contains(TermMode::ALT_SCREEN));
        assert_eq!(s.term().grid()[Line(0)][Column(0)].c, 'V', "the alternate screen shows");
        s.feed(b"\x1b[?1049l");
        assert_eq!(s.term().grid()[Line(0)][Column(0)].c, 'p', "the primary screen is tmux's saved grid");
        assert_eq!(s.term().grid().cursor.point.column.0, 2, "its cursor is the saved one");
    }

    #[test]
    fn pending_output_keeps_only_controls() {
        assert_eq!(pending_bytes(b"a\\033[1mb\\\\c\\101"), b"a\x1b[1mb\\\\c\\101", "only control codes decode");
        assert_eq!(pending_bytes(b"x\\01"), b"x\\01", "too short");
        assert_eq!(pending_bytes(b"\\015\\012"), b"\r\n");
    }
}

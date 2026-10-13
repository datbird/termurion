//! The reports iTerm2 answers that alacritty does not: XTVERSION, iTerm2's version query
//! (`CSI 1337 n`), DECRQSS, and XTGETTCAP. Device attributes by emulation level are
//! answered in alacritty_terminal (patch 8).

use alacritty_terminal::grid::Dimensions;

use crate::screen::Screen;

/// Termurion's version, as XTVERSION reports it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The iTerm2 version `CSI 1337 n` reports: the iTerm2 line Termurion follows. iTerm2 answers
/// with its version uppercased (`VT100Output.m:922-927`), so it holds no "n" and does not
/// start with "0"; it2check compares it with the script's ASCII order.
pub const ITERM2_VERSION: &str = "3.7.0";

/// At most this many XTGETTCAP keys are answered in one query.
const MAX_CAPS: usize = 64;

/// A query the scanner caught.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Query {
    /// `CSI > q` or `CSI > 0 q`.
    XtVersion,
    /// `CSI 1337 n`.
    ITerm2Version,
    /// `DCS $ q Pt ST`.
    Decrqss(Vec<u8>),
    /// `DCS + q Pt ST`.
    XtGetTcap(Vec<u8>),
    /// `CSI ? 996 n`: is the background dark or light?
    ColorScheme,
    /// `CSI ? 2031 h` or `l`: report color scheme changes, or stop.
    ColorSchemeReports(bool),
}

/// The color scheme report: `CSI ? 997 ; 1 n` for dark, `; 2 n` for light.
pub fn color_scheme(dark: bool) -> Vec<u8> {
    format!("\x1b[?997;{}n", if dark { 1 } else { 2 }).into_bytes()
}

/// The answer to a query, or nothing.
pub fn answer(screen: &Screen, q: &Query) -> Vec<u8> {
    match q {
        Query::XtVersion => format!("\x1bP>|Termurion {VERSION}\x1b\\"),
        Query::ITerm2Version => format!("\x1b[ITERM2 {ITERM2_VERSION}n"),
        Query::Decrqss(pt) => match decrqss(screen, pt) {
            Some(p) => format!("\x1bP1$r{p}\x1b\\"),
            None => "\x1bP0$r\x1b\\".to_string(),
        },
        Query::XtGetTcap(keys) => match xtgettcap(keys, screen.term_name()) {
            Some(p) => format!("\x1bP1+r{p}\x1b\\"),
            None => "\x1bP0+r\x1b\\".to_string(),
        },
        Query::ColorScheme => return color_scheme(screen.is_dark()),
        Query::ColorSchemeReports(_) => return Vec::new(),
    }
    .into_bytes()
}

/// DECRQSS: the setting's current value followed by its name, or None for a setting
/// Termurion does not report.
fn decrqss(screen: &Screen, pt: &[u8]) -> Option<String> {
    let term = screen.term();
    Some(match pt {
        b"m" => format!("{}m", sgr(&term.grid().cursor.template)),
        b"\"p" => match screen.level() / 100 {
            1 => "61\"p".to_string(),
            n => format!("6{n};1\"p"),
        },
        b" q" => {
            use alacritty_terminal::vte::ansi::CursorShape;
            let style = term.cursor_style();
            let n = match style.shape {
                CursorShape::Underline => 3,
                CursorShape::Beam => 5,
                _ => 1,
            } + if style.blinking { 0 } else { 1 };
            format!("{n} q")
        }
        b"\"q" => "0\"q".to_string(),
        b"r" => {
            let r = term.scroll_region();
            format!("{};{}r", r.start.0 + 1, r.end.0)
        }
        b"s" => format!("1;{}s", term.columns()),
        b"t" => format!("{}t", term.screen_lines()),
        _ => return None,
    })
}

/// The SGR parameters that recreate a cell's look, starting with 0.
fn sgr(cell: &alacritty_terminal::term::cell::Cell) -> String {
    use alacritty_terminal::term::cell::Flags;
    let mut out = vec!["0".to_string()];
    for (flag, code) in [
        (Flags::BOLD, "1"),
        (Flags::DIM, "2"),
        (Flags::ITALIC, "3"),
        (Flags::UNDERLINE, "4"),
        (Flags::DOUBLE_UNDERLINE, "4:2"),
        (Flags::UNDERCURL, "4:3"),
        (Flags::DOTTED_UNDERLINE, "4:4"),
        (Flags::DASHED_UNDERLINE, "4:5"),
        (Flags::BLINK, "5"),
        (Flags::INVERSE, "7"),
        (Flags::HIDDEN, "8"),
        (Flags::STRIKEOUT, "9"),
    ] {
        if cell.flags.contains(flag) {
            out.push(code.to_string());
        }
    }
    out.extend(color(cell.fg, 30));
    out.extend(color(cell.bg, 40));
    out.join(";")
}

/// SGR parameters for a color; `base` is 30 (foreground) or 40 (background).
fn color(c: alacritty_terminal::vte::ansi::Color, base: u16) -> Option<String> {
    use alacritty_terminal::vte::ansi::{Color, NamedColor};
    Some(match c {
        Color::Named(n) => {
            let i = n as usize;
            match i {
                0..=7 => (base + i as u16).to_string(),
                8..=15 => (base + 60 + i as u16 - 8).to_string(),
                _ if matches!(n, NamedColor::Foreground | NamedColor::Background) => return None,
                _ => return None,
            }
        }
        Color::Indexed(i) => format!("{};5;{i}", base + 8),
        Color::Spec(rgb) => format!("{};2;{};{};{}", base + 8, rgb.r, rgb.g, rgb.b),
    })
}

/// XTGETTCAP: `key=value` pairs (hex) for every key asked, or None when any is unknown.
fn xtgettcap(keys: &[u8], term_name: &str) -> Option<String> {
    let mut out = Vec::new();
    for (n, key) in keys.split(|b| *b == b';').enumerate() {
        if n >= MAX_CAPS {
            return None;
        }
        let name = String::from_utf8(unhex(key)?).ok()?;
        let value = match name.as_str() {
            "TN" | "name" => term_name,
            "Co" | "colors" => "256",
            "RGB" => "8/8/8",
            _ => return None,
        };
        out.push(format!("{}={}", hex(name.as_bytes()), hex(value.as_bytes())));
    }
    Some(out.join(";"))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02X}")).collect()
}

fn unhex(s: &[u8]) -> Option<Vec<u8>> {
    if s.is_empty() || s.len() % 2 != 0 {
        return None;
    }
    s.chunks(2).map(|p| u8::from_str_radix(std::str::from_utf8(p).ok()?, 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use crate::screen::Screen;

    fn ask(s: &mut Screen, q: &[u8]) -> String {
        String::from_utf8(s.feed(q)).unwrap()
    }

    #[test]
    fn device_attributes_follow_the_level() {
        let mut s = Screen::new(80, 24);
        assert_eq!(ask(&mut s, b"\x1b[c"), "\x1b[?64;1;2;4;6;17;18;21;22;52c");
        assert_eq!(ask(&mut s, b"\x1b[>c"), "\x1b[>64;2500;0c");
        assert_eq!(ask(&mut s, b"\x1b[=c"), "\x1bP!|6954726D\x1b\\");
        s.set_level(100);
        assert_eq!(ask(&mut s, b"\x1b[c"), "\x1b[?1;2c");
        assert_eq!(ask(&mut s, b"\x1b[>c"), "\x1b[>0;2500;0c");
        assert_eq!(ask(&mut s, b"\x1b[=c"), "", "DA3 needs VT400");
        s.set_level(200);
        assert_eq!(ask(&mut s, b"\x1b[c"), "\x1b[?62;1;2;4;6;22;28c");
        s.set_level(300);
        assert_eq!(ask(&mut s, b"\x1b[>c"), "\x1b[>32;2500;0c");
        s.set_level(400);
        assert_eq!(ask(&mut s, b"\x1b[>0c"), "\x1b[>41;2500;0c");
    }

    #[test]
    fn decrqss_cursor_follows_permission() {
        use crate::screen::{Look, Screen};
        let look = |allow| Look { cursor_blink: false, blink_change: allow, ..Look::default() };
        let mut s = Screen::with_look(80, 24, crate::palette::Palette::default(), look(false));
        s.feed(b"\x1b[1 q");
        assert_eq!(ask(&mut s, b"\x1bP$q q\x1b\\"), "\x1bP1$r2 q\x1b\\");
        assert_eq!(ask(&mut s, b"\x1b[?12$p"), "\x1b[?12;2$y");
        let mut s = Screen::with_look(80, 24, crate::palette::Palette::default(), look(true));
        s.feed(b"\x1b[1 q");
        assert_eq!(ask(&mut s, b"\x1bP$q q\x1b\\"), "\x1bP1$r1 q\x1b\\");
        assert_eq!(ask(&mut s, b"\x1b[?12$p"), "\x1b[?12;1$y");
    }

    #[test]
    fn xtversion_and_1337n() {
        let mut s = Screen::new(80, 24);
        let v = super::VERSION;
        assert_eq!(ask(&mut s, b"\x1b[>q"), format!("\x1bP>|Termurion {v}\x1b\\"));
        assert_eq!(ask(&mut s, b"\x1b[>0q"), format!("\x1bP>|Termurion {v}\x1b\\"));
        assert_eq!(ask(&mut s, b"\x1b[1337n"), "\x1b[ITERM2 3.7.0n");
        // Split across reads.
        assert_eq!(ask(&mut s, b"\x1b[13"), "");
        assert_eq!(ask(&mut s, b"37n"), "\x1b[ITERM2 3.7.0n");
    }

    #[test]
    fn decrqss_reports_settings() {
        let mut s = Screen::new(80, 24);
        assert_eq!(ask(&mut s, b"\x1bP$qm\x1b\\"), "\x1bP1$r0m\x1b\\");
        s.feed(b"\x1b[1;4;31;48;5;200m");
        assert_eq!(ask(&mut s, b"\x1bP$qm\x1b\\"), "\x1bP1$r0;1;4;31;48;5;200m\x1b\\");
        s.feed(b"\x1b[0;38;2;1;2;3m");
        assert_eq!(ask(&mut s, b"\x1bP$qm\x1b\\"), "\x1bP1$r0;38;2;1;2;3m\x1b\\");
        s.feed(b"\x1b[0;5;7m");
        assert_eq!(ask(&mut s, b"\x1bP$qm\x1b\\"), "\x1bP1$r0;5;7m\x1b\\");
        s.feed(b"\x1b[5;10r");
        assert_eq!(ask(&mut s, b"\x1bP$qr\x1b\\"), "\x1bP1$r5;10r\x1b\\");
        s.feed(b"\x1b[4 q");
        assert_eq!(ask(&mut s, b"\x1bP$q q\x1b\\"), "\x1bP1$r4 q\x1b\\");
        assert_eq!(ask(&mut s, b"\x1bP$q\"p\x1b\\"), "\x1bP1$r65;1\"p\x1b\\");
        assert_eq!(ask(&mut s, b"\x1bP$q\"q\x1b\\"), "\x1bP1$r0\"q\x1b\\");
        assert_eq!(ask(&mut s, b"\x1bP$qs\x1b\\"), "\x1bP1$r1;80s\x1b\\");
        assert_eq!(ask(&mut s, b"\x1bP$qt\x1b\\"), "\x1bP1$r24t\x1b\\");
        s.set_level(100);
        assert_eq!(ask(&mut s, b"\x1bP$q\"p\x1b\\"), "\x1bP1$r61\"p\x1b\\");
    }

    #[test]
    fn xtgettcap_answers_known_keys() {
        let mut s = Screen::new(80, 24);
        assert_eq!(ask(&mut s, b"\x1bP+q544e;436f\x1b\\"), "\x1bP1+r544E=787465726D2D323536636F6C6F72;436F=323536\x1b\\");
        assert_eq!(ask(&mut s, b"\x1bP+q524742\x1b\\"), "\x1bP1+r524742=382F382F38\x1b\\");
        assert_eq!(ask(&mut s, b"\x1bP+q6e616d65;636f6c6f7273\x1b\\"), "\x1bP1+r6E616D65=787465726D2D323536636F6C6F72;636F6C6F7273=323536\x1b\\");
        // The profile's Terminal Type is what TN and name report (`T:3307-3318`).
        s.set_term_name("xterm-kitty");
        assert_eq!(s.term_name(), "xterm-kitty");
        assert_eq!(ask(&mut s, b"\x1bP+q544e\x1b\\"), format!("\x1bP1+r544E={}\x1b\\", super::hex(b"xterm-kitty")));
        assert_eq!(ask(&mut s, b"\x1bP+q6e616d65\x1b\\"), format!("\x1bP1+r6E616D65={}\x1b\\", super::hex(b"xterm-kitty")));
    }

    #[test]
    fn bad_queries_reply_errors() {
        let mut s = Screen::new(80, 24);
        assert_eq!(ask(&mut s, b"\x1bP$qzz\x1b\\"), "\x1bP0$r\x1b\\");
        assert_eq!(ask(&mut s, b"\x1bP$q\x1b\\"), "\x1bP0$r\x1b\\");
        assert_eq!(ask(&mut s, b"\x1bP+q4a4a\x1b\\"), "\x1bP0+r\x1b\\", "unknown key");
        assert_eq!(ask(&mut s, b"\x1bP+qZZ\x1b\\"), "\x1bP0+r\x1b\\", "not hex");
        assert_eq!(ask(&mut s, b"\x1bP+q544\x1b\\"), "\x1bP0+r\x1b\\", "odd length");
        assert_eq!(ask(&mut s, b"\x1bP+qc3\x1b\\"), "\x1bP0+r\x1b\\", "not UTF-8");
        let mut huge = b"\x1bP+q".to_vec();
        for _ in 0..100_000 {
            huge.extend_from_slice(b"544e;");
        }
        huge.extend_from_slice(b"\x1b\\");
        assert_eq!(ask(&mut s, &huge), "\x1bP0+r\x1b\\", "too many keys");
        assert_eq!(ask(&mut s, b"\x1b[>99999q"), "", "XTVERSION takes only 0");
        assert_eq!(ask(&mut s, b"\x1b[>99999999999999999999999q"), "");
        let mut long = b"\x1b[".to_vec();
        long.extend(std::iter::repeat_n(b'1', 100_000));
        long.push(b'n');
        assert_eq!(ask(&mut s, &long), "");
    }
}

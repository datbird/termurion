//! iTerm2's `colorFromHexString:` and the color math pane title bars need.

use crate::palette::{rgb, Rgb};

/// `NSColor colorFromHexString:`: "#rgb", "#rrggbb" or "#rrrrggggbbbb", in sRGB, or after
/// "p3" in Display P3 (so "p3#rrrrggggbbbb"). A partial scan such as "#12345z" is refused
/// (`NSScanner` would accept it; Decision), as is anything with surrounding spaces.
pub fn parse(word: &str) -> Option<Rgb> {
    let (p3, hash) = match word.strip_prefix("p3") {
        Some(rest) if rest.starts_with('#') => (true, rest),
        _ => (false, word),
    };
    let digits = hash.strip_prefix('#')?;
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    // Components as fractions of the whole (`getHashColorRed:green:blue:` gives 16 bits).
    let part = |s: &str, max: f64| u32::from_str_radix(s, 16).ok().map(|v| v as f64 / max);
    let (r, g, b) = match digits.len() {
        3 => {
            let d = |i: usize| part(&digits[i..i + 1].repeat(2), 255.0);
            (d(0)?, d(1)?, d(2)?)
        }
        6 => (part(&digits[0..2], 255.0)?, part(&digits[2..4], 255.0)?, part(&digits[4..6], 255.0)?),
        12 => (part(&digits[0..4], 65535.0)?, part(&digits[4..8], 65535.0)?, part(&digits[8..12], 65535.0)?),
        _ => return None,
    };
    let (r, g, b) = if p3 { crate::import::p3_to_srgb(r, g, b) } else { (r, g, b) };
    let to8 = |c: f64| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
    Some(rgb(to8(r), to8(g), to8(b)))
}

/// `it_colorByDimmingByAmount:` (basis 0.15): the same mix toward gray 0.15 as the tab themes.
pub fn dim_by(c: Rgb, amount: f32) -> Rgb {
    crate::tab_theme::toward_gray(c, amount, 0.15)
}

/// A gray of this level (0 to 1) in sRGB.
pub fn white(level: f32) -> Rgb {
    let v = (level * 255.0).round().clamp(0.0, 255.0) as u8;
    rgb(v, v, v)
}

/// `getHue:saturation:brightness:`: hue 0 to 1, saturation, brightness.
pub fn to_hsb(c: Rgb) -> (f32, f32, f32) {
    let (r, g, b) = (f32::from(c.r) / 255.0, f32::from(c.g) / 255.0, f32::from(c.b) / 255.0);
    let max = r.max(g).max(b);
    let d = max - r.min(g).min(b);
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h / 6.0, if max == 0.0 { 0.0 } else { d / max }, max)
}

/// `colorWithHue:saturation:brightness:alpha:` as bytes, through `statusbar::hsb_to_rgb`.
pub fn from_hsb(h: f32, s: f32, b: f32) -> Rgb {
    let (r, g, bl) = crate::statusbar::hsb_to_rgb(h, s, b);
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    rgb(byte(r), byte(g), byte(bl))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::rgb;

    #[test]
    fn hex_colors_follow_iterm2() {
        assert_eq!(parse("#336699"), Some(rgb(0x33, 0x66, 0x99)));
        assert_eq!(parse("#abc"), Some(rgb(0xaa, 0xbb, 0xcc)));
        assert_eq!(parse("#ffff80800000"), Some(rgb(255, 128, 0)));
        // p3# goes through the one converter the tab colors use (P3 ruling).
        assert_eq!(parse("p3#336699"), Some(rgb(27, 104, 157)));
        for bad in ["", "336699", "#33669", "#33669g", " #336699", "#33669900"] {
            assert_eq!(parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn color_math_follows_iterm2() {
        let c = rgb(0x33, 0x66, 0x99);
        assert_eq!(dim_by(c, 0.3), rgb(47, 83, 119));
        assert_eq!(dim_by(c, 0.0), c);
        assert_eq!(white(0.27), rgb(69, 69, 69));
        assert_eq!(white(0.7), rgb(179, 179, 179));
        assert_eq!(to_hsb(rgb(255, 0, 0)), (0.0, 1.0, 1.0));
        assert_eq!(from_hsb(0.0, 0.75, 0.3), rgb(77, 19, 19));
        assert_eq!(from_hsb(0.0, 0.5, 1.0), rgb(255, 128, 128));
        let (h, s, b) = to_hsb(c);
        assert_eq!(from_hsb(h, s, b), c);
    }
}

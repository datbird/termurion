//! Maps terminal colors to RGB, starting from iTerm2's default colors.

use alacritty_terminal::term::cell::Flags;
pub use alacritty_terminal::vte::ansi::Rgb;
use alacritty_terminal::vte::ansi::{Color, NamedColor};

pub const fn rgb(r: u8, g: u8, b: u8) -> Rgb {
    Rgb { r, g, b }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    pub ansi: [Rgb; 16],
    pub fg: Rgb,
    pub bg: Rgb,
    pub bold: Rgb,
    pub cursor: Rgb,
    pub cursor_text: Rgb,
    pub selection: Rgb,
    /// None keeps a selected cell's own text color, iTerm2's default.
    pub selected_text: Option<Rgb>,
    /// iTerm2's "Use bright colors for bold text".
    pub bright_bold: bool,
    /// Opacity of faint (dim) text over its background.
    pub faint_alpha: f32,
}

impl Default for Palette {
    /// iTerm2's stock default profile colors, from the schema.
    fn default() -> Self {
        Palette::from_profile(&crate::profile::Profile::default(), true)
    }
}

fn dim(c: Rgb) -> Rgb {
    let d = |v: u8| (v as u16 * 2 / 3) as u8;
    rgb(d(c.r), d(c.g), d(c.b))
}

impl Palette {
    /// The palette a profile describes. `dark` picks the Dark variants when the
    /// profile keeps separate Light and Dark colors.
    pub fn from_profile(p: &crate::profile::Profile, dark: bool) -> Palette {
        let c = |k: &str| p.color_for(k, dark);
        let mut ansi = [rgb(0, 0, 0); 16];
        for (i, slot) in ansi.iter_mut().enumerate() {
            *slot = c(&format!("Ansi {i} Color"));
        }
        let fg = c("Foreground Color");
        Palette {
            ansi,
            fg,
            bg: c("Background Color"),
            bold: if p.bool("Use Bright Bold") { c("Bold Color") } else { fg },
            cursor: c("Cursor Color"),
            cursor_text: c("Cursor Text Color"),
            selection: c("Selection Color"),
            selected_text: p.bool("Use Selected Text Color").then(|| c("Selected Text Color")),
            bright_bold: p.bool("Brighten Bold Text"),
            faint_alpha: p.float("Faint Text Alpha") as f32,
        }
    }

    /// A 256-color index: 16 ANSI colors, a 6x6x6 cube, then 24 grays.
    pub fn indexed(&self, i: u8) -> Rgb {
        match i {
            0..=15 => self.ansi[i as usize],
            16..=231 => {
                let i = i - 16;
                let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                rgb(level(i / 36), level(i / 6 % 6), level(i % 6))
            }
            _ => {
                let v = 8 + (i - 232) * 10;
                rgb(v, v, v)
            }
        }
    }

    fn named(&self, n: NamedColor) -> Rgb {
        let i = n as usize;
        match n {
            NamedColor::Foreground => self.fg,
            NamedColor::Background => self.bg,
            NamedColor::Cursor => self.cursor,
            NamedColor::BrightForeground => self.bold,
            NamedColor::DimForeground => dim(self.fg),
            _ if i < 16 => self.ansi[i],
            _ => dim(self.ansi[i - NamedColor::DimBlack as usize]),
        }
    }

    pub fn color(&self, c: Color) -> Rgb {
        match c {
            Color::Spec(rgb) => rgb,
            Color::Indexed(i) => self.indexed(i),
            Color::Named(n) => self.named(n),
        }
    }

    /// The (foreground, background) a cell draws with.
    pub fn cell(&self, fg: Color, bg: Color, flags: Flags) -> (Rgb, Rgb) {
        let bold = flags.contains(Flags::BOLD);
        let mut f = match fg {
            Color::Named(NamedColor::Foreground) if bold => self.bold,
            Color::Named(n) if bold && self.bright_bold && (n as usize) < 8 => self.ansi[n as usize + 8],
            Color::Indexed(i) if bold && self.bright_bold && i < 8 => self.ansi[i as usize + 8],
            c => self.color(c),
        };
        let mut b = self.color(bg);
        if flags.contains(Flags::DIM) {
            let a = self.faint_alpha;
            let mix = |f: u8, b: u8| (f as f32 * a + b as f32 * (1.0 - a)).round() as u8;
            f = rgb(mix(f.r, b.r), mix(f.g, b.g), mix(f.b, b.b));
        }
        if flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut f, &mut b);
        }
        if flags.contains(Flags::HIDDEN) {
            f = b;
        }
        (f, b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_red_is_itermish() {
        assert_eq!(Palette::default().color(Color::Named(NamedColor::Red)), rgb(0xb4, 0x3c, 0x2a));
    }

    #[test]
    fn palette_reads_the_profile() {
        let mut p = crate::profile::Profile::default();
        // The stock profile keeps separate Light and Dark colors; test the single set.
        p.set("Use Separate Colors for Light and Dark Mode", toml::Value::Boolean(false));
        p.set("Ansi 4 Color", crate::profile::color_value(rgb(1, 2, 3)));
        p.set("Brighten Bold Text", toml::Value::Boolean(false));
        let pal = Palette::from_profile(&p, true);
        assert_eq!(pal.ansi[4], rgb(1, 2, 3));
        assert!(!pal.bright_bold);
        assert!(pal.selected_text.is_some());
        p.set("Use Selected Text Color", toml::Value::Boolean(false));
        assert_eq!(Palette::from_profile(&p, true).selected_text, None);
    }

    #[test]
    fn faint_text_uses_the_profile_alpha() {
        let mut p = crate::profile::Profile::default();
        p.set("Faint Text Alpha", toml::Value::Float(0.5));
        let pal = Palette::from_profile(&p, true);
        let (fg, _) = pal.cell(Color::Named(NamedColor::Foreground), Color::Named(NamedColor::Background), Flags::DIM);
        let half = |f: u8, b: u8| (f as f32 * 0.5 + b as f32 * 0.5).round() as u8;
        assert_eq!(fg, rgb(half(pal.fg.r, pal.bg.r), half(pal.fg.g, pal.bg.g), half(pal.fg.b, pal.bg.b)));
    }

    #[test]
    fn indexed_cube_and_grays() {
        let p = Palette::default();
        assert_eq!(p.indexed(16), rgb(0, 0, 0));
        assert_eq!(p.indexed(196), rgb(255, 0, 0));
        assert_eq!(p.indexed(231), rgb(255, 255, 255));
        assert_eq!(p.indexed(232), rgb(8, 8, 8));
        assert_eq!(p.indexed(255), rgb(238, 238, 238));
    }

    #[test]
    fn dim_named_is_two_thirds() {
        let p = Palette::default();
        let red = p.ansi[1];
        let d = |v: u8| (v as u16 * 2 / 3) as u8;
        assert_eq!(p.color(Color::Named(NamedColor::DimRed)), rgb(d(red.r), d(red.g), d(red.b)));
    }

    #[test]
    fn bold_uses_bright_colors() {
        let p = Palette::default();
        let (fg, _) = p.cell(Color::Named(NamedColor::Red), Color::Named(NamedColor::Background), Flags::BOLD);
        assert_eq!(fg, p.ansi[9]);
        let (fg, _) = p.cell(Color::Named(NamedColor::Foreground), Color::Named(NamedColor::Background), Flags::BOLD);
        assert_eq!(fg, p.bold);
    }

    #[test]
    fn inverse_swaps_and_hidden_hides() {
        let p = Palette::default();
        let (fg, bg) = p.cell(Color::Named(NamedColor::Foreground), Color::Named(NamedColor::Background), Flags::INVERSE);
        assert_eq!((fg, bg), (p.bg, p.fg));
        let (fg, bg) = p.cell(Color::Named(NamedColor::Foreground), Color::Named(NamedColor::Background), Flags::HIDDEN);
        assert_eq!(fg, bg);
    }

    #[test]
    fn spec_colors_pass_through() {
        assert_eq!(Palette::default().color(Color::Spec(rgb(1, 2, 3))), rgb(1, 2, 3));
    }
}

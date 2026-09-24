//! Maps terminal colors to RGB, starting from iTerm2's default colors.

use alacritty_terminal::term::cell::Flags;
pub use alacritty_terminal::vte::ansi::Rgb;
use alacritty_terminal::vte::ansi::{Color, NamedColor};

pub const fn rgb(r: u8, g: u8, b: u8) -> Rgb {
    Rgb { r, g, b }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    pub ansi: [Rgb; 16],
    pub fg: Rgb,
    pub bg: Rgb,
    pub bold: Rgb,
    pub cursor: Rgb,
    pub cursor_text: Rgb,
    pub selection: Rgb,
    pub selected_text: Rgb,
    /// iTerm2's "Use bright colors for bold text".
    pub bright_bold: bool,
}

impl Default for Palette {
    /// iTerm2's default profile colors.
    fn default() -> Self {
        Palette {
            ansi: [
                rgb(0x00, 0x00, 0x00), rgb(0xc9, 0x1b, 0x00), rgb(0x00, 0xc2, 0x00), rgb(0xc7, 0xc4, 0x00),
                rgb(0x02, 0x25, 0xc7), rgb(0xca, 0x30, 0xc7), rgb(0x00, 0xc5, 0xc7), rgb(0xc7, 0xc7, 0xc7),
                rgb(0x67, 0x67, 0x67), rgb(0xff, 0x6d, 0x67), rgb(0x5f, 0xf9, 0x67), rgb(0xfe, 0xfb, 0x67),
                rgb(0x68, 0x71, 0xff), rgb(0xff, 0x76, 0xff), rgb(0x5f, 0xfd, 0xff), rgb(0xff, 0xfe, 0xfe),
            ],
            fg: rgb(0xc7, 0xc7, 0xc7),
            bg: rgb(0x00, 0x00, 0x00),
            bold: rgb(0xff, 0xff, 0xff),
            cursor: rgb(0xc7, 0xc7, 0xc7),
            cursor_text: rgb(0x00, 0x00, 0x00),
            selection: rgb(0xc1, 0xde, 0xff),
            selected_text: rgb(0x00, 0x00, 0x00),
            bright_bold: true,
        }
    }
}

fn dim(c: Rgb) -> Rgb {
    let d = |v: u8| (v as u16 * 2 / 3) as u8;
    rgb(d(c.r), d(c.g), d(c.b))
}

impl Palette {
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
        if flags.contains(Flags::DIM) {
            f = dim(f);
        }
        let mut b = self.color(bg);
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
        assert_eq!(Palette::default().color(Color::Named(NamedColor::Red)), rgb(0xc9, 0x1b, 0x00));
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
        assert_eq!(Palette::default().color(Color::Named(NamedColor::DimRed)), rgb(0x86, 0x12, 0x00));
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

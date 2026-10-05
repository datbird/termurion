//! Box and block drawing character sets (iTerm2's `iTermBoxDrawingBezierCurveFactory`).

/// True for iTerm2's `blockDrawingCharacters`: block elements, shades, quadrants, the corner
/// triangles, the Powerline separators and the sextants. Minimum contrast leaves them alone
/// because they sit beside empty cells whose background should match.
pub fn is_block(c: char) -> bool {
    matches!(u32::from(c),
        0x2580..=0x259F
        | 0x25E2..=0x25E5
        | 0x25F8..=0x25FA
        | 0x25FF
        // Powerline (https://github.com/ryanoasis/powerline-extra-symbols).
        | 0xE0B0 | 0xE0B2 | 0xE0B4 | 0xE0B6 | 0xE0B8 | 0xE0BA | 0xE0BC | 0xE0BE
        | 0xE0C0 | 0xE0C2 | 0xE0C8 | 0xE0CA | 0xE0D1 | 0xE0D2 | 0xE0D4 | 0xE0D6 | 0xE0D7
        // The 60 block sextants.
        | 0x1FB00..=0x1FB3B)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_set_follows_iterm2() {
        for c in ['\u{2580}', '\u{2588}', '\u{258F}', '\u{2590}', '\u{2591}', '\u{2593}', '\u{2594}', '\u{2595}', '\u{2596}', '\u{259F}', '\u{25E2}', '\u{25E5}', '\u{25F8}', '\u{25FA}', '\u{25FF}', '\u{E0B0}', '\u{E0B2}', '\u{E0D7}', '\u{1FB00}', '\u{1FB3B}'] {
            assert!(is_block(c), "{c:?}");
        }
        // Box lines, the powerline glyphs outside the list, the gap in the triangles, and the
        // sextant neighbors are not blocks.
        for c in ['\u{2500}', '\u{2502}', '\u{257F}', '\u{25E1}', '\u{25E6}', '\u{25FB}', '\u{25FE}', '\u{E0B1}', '\u{E0B3}', '\u{E0D8}', '\u{1FAFF}', '\u{1FB3C}', 'a', ' '] {
            assert!(!is_block(c), "{c:?}");
        }
    }
}

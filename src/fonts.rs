//! Font rules shared by the renderer and the app: the cell a font gets from its metrics.
//! Pure; the renderer reads the metrics from the font and calls these.

/// A font's vertical metrics and the advance of "W", in pixels at the face size.
/// `descent` is positive below the baseline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub ascent: f32,
    pub descent: f32,
    pub leading: f32,
    pub advance_w: f32,
}

/// Whether `family` uses the generous rounding: the lower-cased family contains any entry of the
/// comma-separated `list` (`FontsForGenerousRounding`). Entries are neither trimmed nor dropped,
/// so an empty entry matches every family (`FontSizeEstimator.m:64-72`).
pub fn generous(family: &str, list: &str) -> bool {
    let family = family.to_lowercase();
    list.split(',').any(|entry| family.contains(entry))
}

/// The cell for `m`, and the spacing-free cell the cursor is measured against, as `(cell, base)`.
pub fn cell_size(m: Metrics, generous: bool, h_spacing: f32, v_spacing: f32) -> ((f32, f32), (f32, f32)) {
    // iTerm2 takes ceil(ascender) - floor(descender) with a negative descender when generous,
    // else ascender - descender; then adds the leading (`PTYTextView.m:227-233`). The width stays
    // the unrounded advance (a rounded one drifts from the shaped text along a row).
    let line = if generous { m.ascent.ceil() + m.descent.ceil() } else { m.ascent + m.descent };
    let base_h = (line + m.leading).ceil().max(1.0);
    let w = m.advance_w.max(1.0);
    ((w * h_spacing, (base_h * v_spacing).ceil().max(1.0)), (w, base_h))
}

/// The baseline of a row `cell_h` tall, from the row's top. cosmic-text centers the glyphs'
/// ascent and descent in the line, so the extra height splits evenly above and below.
pub fn baseline(m: Metrics, cell_h: f32) -> f32 {
    (cell_h - (m.ascent + m.descent)) / 2.0 + m.ascent
}

#[cfg(test)]
mod tests {
    use super::*;

    const M: Metrics = Metrics { ascent: 12.4, descent: 3.2, leading: 0.0, advance_w: 7.6 };

    fn cell(m: Metrics, g: bool, h: f32, v: f32) -> (f32, f32) {
        cell_size(m, g, h, v).0
    }

    #[test]
    fn line_height_follows_iterm2() {
        assert_eq!(cell(M, false, 1.0, 1.0), (7.6, 16.0));
        assert_eq!(cell(M, true, 1.0, 1.0), (7.6, 17.0));
        let w = cell(M, false, 1.5, 1.0).0;
        assert!((w - 11.4).abs() < 1e-4, "{w}");
        assert_eq!(cell(M, false, 1.0, 1.25).1, 20.0);
        assert_eq!(cell(Metrics { leading: 1.0, ..M }, false, 1.0, 1.0).1, 17.0);
        // The spacing-free cell ignores both spacings and keeps the rounding rule.
        let (c, base) = cell_size(M, true, 1.5, 1.25);
        assert_eq!(base, (7.6, 17.0));
        assert_eq!(c.1, (17.0f32 * 1.25).ceil());
        // At least one pixel each way.
        assert_eq!(cell_size(Metrics { ascent: 0.0, descent: 0.0, leading: 0.0, advance_w: 0.0 }, false, 1.0, 1.0).1, (1.0, 1.0));
    }

    #[test]
    fn generous_follows_iterm2() {
        assert!(generous("Consolas", "consolas"));
        assert!(!generous("Cascadia Mono", "consolas"));
        assert!(!generous("Fira Code", "code ,consolas"));
        assert!(generous("Fira Code", "fira,consolas"));
        assert!(!generous("Consolas", "Consolas"));
        assert!(generous("x", ""));
        assert!(generous("x", "consolas,"));
    }

    #[test]
    fn baseline_centers_the_glyphs() {
        assert!((baseline(M, 16.0) - 12.6).abs() < 1e-4);
        assert!((baseline(M, 17.0) - 13.1).abs() < 1e-4);
    }
}

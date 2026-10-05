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

/// A weight on NSFontManager's scale (`PTYFontInfo.m` `boldVersionOfFont:` counts in these
/// classes): CSS 100 is 2, 200 is 3, 300 is 4, 400 is 5, 500 is 6, 600 is 8, 700 is 9, 800 is 10
/// and 900 is 11; a weight between two classes takes the lower one.
pub fn ns_weight(css: u16) -> i32 {
    match css {
        0..=199 => 2,
        200..=299 => 3,
        300..=399 => 4,
        400..=499 => 5,
        500..=599 => 6,
        600..=699 => 8,
        700..=799 => 9,
        800..=899 => 10,
        _ => 11,
    }
}

/// Every weight a variable face covers: the 100-steps inside `min..=max`.
pub fn range_weights(min: u16, max: u16) -> Vec<u16> {
    (1..=9).map(|n| n * 100).filter(|w| *w >= min && *w <= max).collect()
}

/// The weight the "regular" face of a family has: the one nearest 400, the lighter on a tie.
pub fn regular_weight(available: &[u16]) -> Option<u16> {
    available.iter().copied().min_by_key(|w| ((*w as i32 - 400).abs(), *w))
}

/// The bold weight for a regular face (`boldVersionOfFont:`): the lightest of `available` whose
/// class is at least the regular's class plus `min_difference`
/// (`MinimumWeightDifferenceForBoldFont`). None means no face is heavy enough and the regular
/// face is double-struck.
pub fn bold_weight(regular_css: u16, available: &[u16], min_difference: i32) -> Option<u16> {
    let need = ns_weight(regular_css) + min_difference;
    available.iter().copied().filter(|w| ns_weight(*w) >= need).min()
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

    #[test]
    fn bold_face_follows_iterm2() {
        assert_eq!(bold_weight(400, &[400, 700], 4), Some(700));
        // 600 is class 8, below 5 + 4.
        assert_eq!(bold_weight(400, &[400, 500, 600], 4), None);
        assert_eq!(bold_weight(400, &[400, 600], 3), Some(600));
        assert_eq!(bold_weight(300, &[300, 400, 700], 4), Some(700));
        assert_eq!(bold_weight(500, &[500, 700, 800], 4), Some(800));
        // The lightest weight that is heavy enough, whatever the order.
        assert_eq!(bold_weight(400, &[900, 700, 400], 4), Some(700));
        assert_eq!(bold_weight(400, &[], 4), None);
        assert_eq!(ns_weight(650), 8);
        for (css, ns) in [(100, 2), (200, 3), (300, 4), (400, 5), (500, 6), (600, 8), (700, 9), (800, 10), (900, 11), (450, 5), (950, 11)] {
            assert_eq!(ns_weight(css), ns, "css {css}");
        }
    }

    #[test]
    fn weights_of_a_variable_face_cover_its_range() {
        assert_eq!(range_weights(300, 700), vec![300, 400, 500, 600, 700]);
        assert_eq!(range_weights(350, 650), vec![400, 500, 600]);
        assert_eq!(range_weights(700, 300), Vec::<u16>::new());
    }

    #[test]
    fn the_regular_face_is_the_one_nearest_400() {
        assert_eq!(regular_weight(&[300, 400, 700]), Some(400));
        assert_eq!(regular_weight(&[700, 300]), Some(300));
        assert_eq!(regular_weight(&[500, 300]), Some(300), "equal distance takes the lighter");
        assert_eq!(regular_weight(&[]), None);
    }
}

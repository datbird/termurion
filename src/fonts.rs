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

/// Whether a font's PostScript name is on iTerm2's ligature blacklist (`PTYFontInfo.m`
/// `it_fontIsOnLigatureBlacklist`): system fonts that never have ligatures. Prefix match, with
/// `MenloNF` and any name containing "Ligaturized" exempt.
pub fn blacklisted(postscript: &str) -> bool {
    const LIST: [&str; 9] = ["AndaleMono", "Courier", "LetterGothicStd", "Monaco", "OCRAStd", "OratorStd", "Osaka", "PTMono", "SFMono"];
    if postscript.starts_with("MenloNF") || postscript.contains("Ligaturized") {
        return false;
    }
    LIST.iter().any(|prefix| postscript.starts_with(prefix))
}

/// The ligature level of a font (`it_ligatureLevel`): 0 blacklisted, 2 with contextual
/// alternates (`EnableContextualAlternates`), else 1.
pub fn ligature_level(blacklisted: bool, calt: bool) -> u8 {
    if blacklisted {
        0
    } else if calt {
        2
    } else {
        1
    }
}

/// The OpenType features that make `level` happen: 0 turns `liga`, `clig`, `calt` and `dlig` off,
/// 1 turns `liga` and `clig` on and `calt` off, 2 turns `liga`, `clig` and `calt` on.
pub fn features(level: u8) -> glyphon::cosmic_text::FontFeatures {
    use glyphon::cosmic_text::{FeatureTag as T, FontFeatures};
    let mut f = FontFeatures::new();
    match level {
        0 => {
            f.disable(T::STANDARD_LIGATURES).disable(T::CONTEXTUAL_LIGATURES).disable(T::CONTEXTUAL_ALTERNATES).disable(T::DISCRETIONARY_LIGATURES);
        }
        1 => {
            f.enable(T::STANDARD_LIGATURES).enable(T::CONTEXTUAL_LIGATURES).disable(T::CONTEXTUAL_ALTERNATES);
        }
        _ => {
            f.enable(T::STANDARD_LIGATURES).enable(T::CONTEXTUAL_LIGATURES).enable(T::CONTEXTUAL_ALTERNATES);
        }
    }
    f
}

/// What a pane's profile and the font say about ligatures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LigatureRules {
    /// The profile's ASCII Ligatures.
    pub ascii: bool,
    /// The profile's Non-ASCII Ligatures.
    pub non_ascii: bool,
    /// `PreferSpeedToFullLigatureSupport`.
    pub prefer_speed: bool,
    /// The font's `ligature_level`.
    pub level: u8,
}

/// The ligature level of a run (`iTermAttributedStringBuilder.m:780-792`, with R3: nothing forms
/// unless the profile asks for that kind of text).
pub fn span_level(text_is_ascii: bool, fast_path: bool, r: LigatureRules) -> u8 {
    if text_is_ascii {
        // With speed preferred, the characters iTerm2 draws without shaping never join.
        if !r.ascii || (r.prefer_speed && fast_path) {
            return 0;
        }
    } else if !r.non_ascii {
        return 0;
    }
    r.level
}

/// Whether iTerm2 draws `c` without shaping when preferring speed (`iTermCharacterSupportsFastPath`).
pub fn fast_path(c: char, ascii_ligatures: bool) -> bool {
    if !c.is_ascii() {
        return false;
    }
    // With ASCII ligatures available only letters, digits and space skip shaping.
    !ascii_ligatures || c.is_ascii_alphanumeric() || c == ' '
}

/// How a text mask is drawn (`iTermTextDrawingHelperShouldAntiAlias`, then thin strokes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaskMode {
    Smooth,
    Aliased,
    Thin,
}

/// The `CacheKey` flag bit of an aliased span (cosmic-text keeps bits it does not know).
pub const FLAG_ALIASED: u32 = 1 << 8;
/// The `CacheKey` flag bit of a thin-stroke span.
pub const FLAG_THIN: u32 = 1 << 9;

impl MaskMode {
    /// The code a `scene::Span` carries: 0 smooth, 1 aliased, 2 thin.
    pub fn code(self) -> u8 {
        match self {
            MaskMode::Smooth => 0,
            MaskMode::Aliased => 1,
            MaskMode::Thin => 2,
        }
    }

    pub fn from_code(code: u8) -> MaskMode {
        match code {
            1 => MaskMode::Aliased,
            2 => MaskMode::Thin,
            _ => MaskMode::Smooth,
        }
    }

    /// The glyph cache key bits of a span in this mode: an aliased span also places whole pixels
    /// (`PIXEL_FONT`) and keeps hinting on; a thin span has its own bit; a smooth one none.
    pub fn flags(self) -> u32 {
        const PIXEL_FONT: u32 = 4;
        match self {
            MaskMode::Smooth => 0,
            MaskMode::Aliased => FLAG_ALIASED | PIXEL_FONT,
            MaskMode::Thin => FLAG_THIN,
        }
    }

    /// The mode a glyph's `CacheKey` flag bits ask for.
    pub fn of_flags(flags: u32) -> MaskMode {
        if flags & FLAG_ALIASED != 0 {
            MaskMode::Aliased
        } else if flags & FLAG_THIN != 0 {
            MaskMode::Thin
        } else {
            MaskMode::Smooth
        }
    }

    /// Whether the text is anti-aliased (smooth and thin are; aliased is not).
    pub fn anti_aliased(self) -> bool {
        self != MaskMode::Aliased
    }
}

/// The mask mode of a run of text (`iTermTextDrawingHelperShouldAntiAlias`: Retina with
/// `ForceAntialiasingOnRetina` is always anti-aliased; without a Non-ASCII Font, or for plain
/// ASCII, the ASCII switch decides, else the Non-ASCII switch), and thin strokes when it is
/// anti-aliased.
pub fn mask_mode(ascii: bool, use_non_ascii_font: bool, ascii_aa: bool, non_ascii_aa: bool, retina: bool, force_aa_on_retina: bool, thin: bool) -> MaskMode {
    let aa = if retina && force_aa_on_retina {
        true
    } else if !use_non_ascii_font || ascii {
        ascii_aa
    } else {
        non_ascii_aa
    };
    match (aa, thin) {
        (false, _) => MaskMode::Aliased,
        (true, true) => MaskMode::Thin,
        (true, false) => MaskMode::Smooth,
    }
}

/// Whether the profile's Thin Strokes setting thins the text (`iTermThinStrokesSetting`): 0 never,
/// 1 on Retina and dark backgrounds, 2 on dark backgrounds, 3 always, 4 on Retina.
pub fn thin_strokes(setting: i64, retina: bool, dark_background: bool) -> bool {
    match setting {
        1 => retina && dark_background,
        2 => dark_background,
        3 => true,
        4 => retina,
        _ => false,
    }
}

/// Rewrites a glyph's alpha mask for `mode`: aliased text has no partial coverage (0 below 128,
/// else 255), thin strokes square the coverage (`a * a / 255`, rounded), smooth text is unchanged.
pub fn filter_mask(mode: MaskMode, alpha: &mut [u8]) {
    match mode {
        MaskMode::Smooth => {}
        MaskMode::Aliased => alpha.iter_mut().for_each(|a| *a = if *a < 128 { 0 } else { 255 }),
        MaskMode::Thin => alpha.iter_mut().for_each(|a| *a = ((*a as u32 * *a as u32 + 127) / 255) as u8),
    }
}

/// The profile's and the window's anti-aliasing inputs for a pane (`mask_mode`'s, with the
/// per-character `ascii` and the pane's `dark` background left to the caller).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaskRules {
    pub use_non_ascii_font: bool,
    pub ascii_aa: bool,
    pub non_ascii_aa: bool,
    pub retina: bool,
    pub force_aa_on_retina: bool,
    /// The profile's Thin Strokes setting.
    pub thin: i64,
}

impl MaskRules {
    /// The stock profile at 1x: both switches on, Thin Strokes on Retina only.
    pub const STOCK: MaskRules = MaskRules { use_non_ascii_font: true, ascii_aa: true, non_ascii_aa: true, retina: false, force_aa_on_retina: false, thin: 4 };

    pub fn mode(&self, ascii: bool, dark_background: bool) -> MaskMode {
        mask_mode(ascii, self.use_non_ascii_font, self.ascii_aa, self.non_ascii_aa, self.retina, self.force_aa_on_retina, thin_strokes(self.thin, self.retina, dark_background))
    }
}

/// How far the double strike of bold text moves right, in physical pixels
/// (`iTermTextDrawingHelper.m:2340`, `:2661`): anti-aliased text shifts by `antiAliasedShift`
/// (0.5 point on a Retina-class display, else 0: the second copy darkens the strokes in place),
/// aliased text by one pixel.
pub fn double_strike_shift(mode: MaskMode, scale: f32) -> f32 {
    if !mode.anti_aliased() {
        1.0
    } else if scale >= 2.0 {
        0.5 * scale
    } else {
        0.0
    }
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
    fn ligature_features_follow_iterm2() {
        assert!(blacklisted("Monaco"));
        assert!(!blacklisted("MenloNF-Regular"));
        assert!(!blacklisted("MonacoLigaturizedNFM"));
        assert!(blacklisted("SFMono-Regular"));
        assert!(!blacklisted("CascadiaCode-Regular"));
        assert!(blacklisted("Courier"));
        assert!(blacklisted("CourierNewPSMT"));
        assert!(!blacklisted("Menlo-Regular"));
        assert_eq!(ligature_level(false, true), 2);
        assert_eq!(ligature_level(false, false), 1);
        assert_eq!(ligature_level(true, true), 0);
        assert_eq!(ligature_level(true, false), 0);
        use glyphon::cosmic_text::FeatureTag as T;
        let value = |level: u8, tag: T| features(level).features.iter().rev().find(|f| f.tag == tag).map(|f| f.value);
        for tag in [T::STANDARD_LIGATURES, T::CONTEXTUAL_LIGATURES, T::CONTEXTUAL_ALTERNATES, T::DISCRETIONARY_LIGATURES] {
            assert_eq!(value(0, tag), Some(0), "level 0 turns {tag:?} off");
        }
        assert_eq!((value(1, T::STANDARD_LIGATURES), value(1, T::CONTEXTUAL_LIGATURES), value(1, T::CONTEXTUAL_ALTERNATES)), (Some(1), Some(1), Some(0)));
        assert_eq!((value(2, T::STANDARD_LIGATURES), value(2, T::CONTEXTUAL_LIGATURES), value(2, T::CONTEXTUAL_ALTERNATES)), (Some(1), Some(1), Some(1)));
    }

    #[test]
    fn ligature_runs_follow_iterm2() {
        assert!(fast_path('a', true));
        assert!(fast_path('7', true));
        assert!(fast_path(' ', true));
        assert!(!fast_path('-', true));
        assert!(fast_path('-', false));
        assert!(!fast_path('\u{e9}', true));
        assert!(!fast_path('\u{e9}', false));
        let r = |ascii, non_ascii, prefer_speed| LigatureRules { ascii, non_ascii, prefer_speed, level: 2 };
        // R3: the stock profile draws no ligatures, whatever the run.
        for (is_ascii, fast) in [(true, true), (true, false), (false, false)] {
            assert_eq!(span_level(is_ascii, fast, r(false, false, true)), 0);
            assert_eq!(span_level(is_ascii, fast, r(false, false, false)), 0);
        }
        // ASCII Ligatures on, speed preferred: punctuation ligates, letters do not.
        assert_eq!(span_level(true, fast_path('-', true), r(true, false, true)), 2);
        assert_eq!(span_level(true, fast_path('a', true), r(true, false, true)), 0);
        // Not preferring speed: both ligate.
        assert_eq!(span_level(true, fast_path('-', true), r(true, false, false)), 2);
        assert_eq!(span_level(true, fast_path('a', true), r(true, false, false)), 2);
        // A non-ASCII run is 0 unless Non-ASCII Ligatures.
        assert_eq!(span_level(false, false, r(true, false, true)), 0);
        assert_eq!(span_level(false, false, r(true, false, false)), 0);
        assert_eq!(span_level(false, false, r(false, true, true)), 2);
        // A font at level 1 or 0 passes its level through.
        assert_eq!(span_level(true, false, LigatureRules { level: 1, ..r(true, false, true) }), 1);
        assert_eq!(span_level(true, false, LigatureRules { level: 0, ..r(true, false, true) }), 0);
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

    #[test]
    fn antialias_follows_iterm2() {
        assert_eq!(mask_mode(true, true, false, true, false, false, false), MaskMode::Aliased);
        assert_eq!(mask_mode(true, true, false, true, true, true, false), MaskMode::Smooth);
        // Retina alone does not force it.
        assert_eq!(mask_mode(true, true, false, true, true, false, false), MaskMode::Aliased);
        // Non-ASCII text follows its own switch.
        assert_eq!(mask_mode(false, true, false, true, false, false, false), MaskMode::Smooth);
        // Without a Non-ASCII Font every character follows the ASCII switch.
        assert_eq!(mask_mode(false, false, true, false, false, false, false), MaskMode::Smooth);
        assert_eq!(mask_mode(false, false, false, true, false, false, false), MaskMode::Aliased);
        // Thin strokes need anti-aliased text.
        assert_eq!(mask_mode(true, true, true, true, false, false, true), MaskMode::Thin);
        assert_eq!(mask_mode(true, true, false, true, false, false, true), MaskMode::Aliased);
        assert!(thin_strokes(4, true, false));
        assert!(!thin_strokes(4, false, true));
        assert!(thin_strokes(1, true, true));
        assert!(!thin_strokes(1, true, false));
        assert!(thin_strokes(2, false, true));
        assert!(!thin_strokes(0, true, true));
        assert!(thin_strokes(3, false, false));
        let mut a = [0u8, 127, 128, 255];
        filter_mask(MaskMode::Aliased, &mut a);
        assert_eq!(a, [0, 0, 255, 255]);
        let mut t = [255u8, 128, 0];
        filter_mask(MaskMode::Thin, &mut t);
        assert_eq!(t, [255, 64, 0]);
        let mut s = [1u8, 99, 200];
        filter_mask(MaskMode::Smooth, &mut s);
        assert_eq!(s, [1, 99, 200]);
    }

    #[test]
    fn mask_modes_round_trip_through_codes_and_flags() {
        for m in [MaskMode::Smooth, MaskMode::Aliased, MaskMode::Thin] {
            assert_eq!(MaskMode::from_code(m.code()), m);
            assert_eq!(MaskMode::of_flags(m.flags()), m);
        }
        // An aliased span places whole pixels and keeps hinting on (DISABLE_HINTING is bit 1).
        assert_eq!(MaskMode::Aliased.flags(), FLAG_ALIASED | 4);
        assert_eq!(MaskMode::Aliased.flags() & 2, 0);
        assert_eq!(MaskMode::Thin.flags(), FLAG_THIN);
        assert_eq!(MaskMode::Smooth.flags(), 0);
        // The stock profile at 1x is smooth; thin strokes need Retina.
        assert_eq!(MaskRules::STOCK.mode(true, true), MaskMode::Smooth);
        assert_eq!(MaskRules { retina: true, ..MaskRules::STOCK }.mode(true, false), MaskMode::Thin);
    }

    #[test]
    fn double_strike_shift_follows_iterm2() {
        // Aliased text: one pixel. Anti-aliased: 0.5 point on a Retina-class scale, else none.
        assert_eq!(double_strike_shift(MaskMode::Aliased, 1.0), 1.0);
        assert_eq!(double_strike_shift(MaskMode::Aliased, 2.0), 1.0);
        assert_eq!(double_strike_shift(MaskMode::Smooth, 1.0), 0.0);
        assert_eq!(double_strike_shift(MaskMode::Thin, 1.5), 0.0);
        assert_eq!(double_strike_shift(MaskMode::Smooth, 2.0), 1.0);
        assert_eq!(double_strike_shift(MaskMode::Thin, 3.0), 1.5);
    }
}

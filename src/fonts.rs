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
    nearest_weight(available, 400)
}

/// The weight of `available` nearest `target`, the lighter on a tie: the face a profile's font asks
/// for when its family has no face at exactly that weight.
pub fn nearest_weight(available: &[u16], target: u16) -> Option<u16> {
    available.iter().copied().min_by_key(|w| ((*w as i32 - target as i32).abs(), *w))
}

/// The weight a PostScript name's style word asks for ("CascadiaMono-Light" is 300). A name with no
/// style word, or one this does not know, is regular.
pub fn weight_from_style_word(postscript: &str) -> u16 {
    let word = postscript.rsplit_once('-').map_or("", |(_, w)| w).to_lowercase();
    match word.trim_end_matches("italic").trim_end_matches("oblique") {
        "thin" | "hairline" => 100,
        "extralight" | "ultralight" => 200,
        "light" => 300,
        "medium" => 500,
        "semibold" | "demibold" => 600,
        "bold" => 700,
        "extrabold" | "ultrabold" | "heavy" => 800,
        "black" => 900,
        _ => 400,
    }
}

/// What the font menu calls a weight.
pub fn weight_name(css: u16) -> String {
    match css {
        100 => "Thin".into(),
        200 => "ExtraLight".into(),
        300 => "Light".into(),
        400 => "Regular".into(),
        500 => "Medium".into(),
        600 => "SemiBold".into(),
        700 => "Bold".into(),
        800 => "ExtraBold".into(),
        900 => "Black".into(),
        w => format!("Weight {w}"),
    }
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

/// One table of an OpenType font file: `index` picks the font of a collection. Reads only the
/// table directory.
pub fn sfnt_table<'a>(data: &'a [u8], index: u32, tag: &[u8; 4]) -> Option<&'a [u8]> {
    let be16 = |o: usize| data.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
    let be32 = |o: usize| data.get(o..o + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    let base = if data.get(0..4)? == b"ttcf" { be32(12 + 4 * index as usize)? as usize } else { 0 };
    for i in 0..be16(base + 4)? as usize {
        let rec = base + 12 + 16 * i;
        if data.get(rec..rec + 4)? == tag {
            let (off, len) = (be32(rec + 8)? as usize, be32(rec + 12)? as usize);
            return data.get(off..off.checked_add(len)?);
        }
    }
    None
}

/// Whether a font's `gasp` table asks for smoothed (anti-aliased) drawing at `ppem` pixels per em:
/// the first range whose largest size reaches `ppem` has the gray-scale or symmetric-smoothing
/// bit. This is what Settings' "(recommended for this font)" reads on Windows, where there is no
/// CoreText `CTFontShouldAntiAlias`.
pub fn gasp_smooths(table: &[u8], ppem: u16) -> bool {
    let be16 = |o: usize| table.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
    let n = be16(2).unwrap_or(0) as usize;
    for i in 0..n {
        let (Some(max), Some(behavior)) = (be16(4 + 4 * i), be16(6 + 4 * i)) else { return false };
        if ppem <= max {
            return behavior & 0x0002 != 0 || behavior & 0x0008 != 0;
        }
    }
    false
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

/// The OpenType features a font's menu choices add to `level`'s: with Contextual Alternates turned
/// off, the level that would turn `calt` on turns it off instead; the stylistic sets that are on
/// (AAT selector 2n is OpenType `ssNN`, `n` from 1 to 20) are enabled for text that shapes.
pub fn features_with(level: u8, chosen: &crate::profile::FontFeatures) -> glyphon::cosmic_text::FontFeatures {
    use glyphon::cosmic_text::FeatureTag;
    let mut f = features(if chosen.calt_off && level == 2 { 1 } else { level });
    if level > 0 {
        for selector in &chosen.stylistic {
            let n = selector / 2;
            if (1..=20).contains(&n) {
                f.enable(FeatureTag::new(&[b's', b's', b'0' + (n / 10) as u8, b'0' + (n % 10) as u8]));
            }
        }
    }
    f
}

/// The stylistic sets a font offers, as its menu lists them: (the AAT selector, 2 for `ss01`, and
/// the set's name), by name. The name is the one the font gives the set (the `name` table entry
/// its GSUB feature points to), else "Stylistic Set N".
pub fn stylistic_sets(data: &[u8], index: u32) -> Vec<(u32, String)> {
    let be16 = |t: &[u8], o: usize| t.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
    let Some(gsub) = sfnt_table(data, index, b"GSUB") else { return Vec::new() };
    let names = sfnt_table(data, index, b"name");
    let Some(list) = be16(gsub, 6).map(|o| o as usize) else { return Vec::new() };
    let mut out: Vec<(u32, String)> = Vec::new();
    for i in 0..be16(gsub, list).unwrap_or(0) as usize {
        let rec = list + 2 + 6 * i;
        let Some(tag) = gsub.get(rec..rec + 4) else { break };
        let n = match tag {
            [b's', b's', a @ b'0'..=b'9', b @ b'0'..=b'9'] => u32::from(a - b'0') * 10 + u32::from(b - b'0'),
            _ => continue,
        };
        if !(1..=20).contains(&n) || out.iter().any(|(s, _)| *s == n * 2) {
            continue;
        }
        let feature = list + be16(gsub, rec + 4).unwrap_or(0) as usize;
        // FeatureParams for a stylistic set: version 0, then the name id of its UI name.
        let name = be16(gsub, feature).filter(|p| *p != 0).and_then(|p| be16(gsub, feature + p as usize + 2)).and_then(|id| names.and_then(|t| name_string(t, id)));
        out.push((n * 2, name.unwrap_or_else(|| format!("Stylistic Set {n}"))));
    }
    out.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    out
}

/// Name id `id` of a `name` table, English if it has one: a Windows (UTF-16) record, else a Mac Roman one.
fn name_string(table: &[u8], id: u16) -> Option<String> {
    let be16 = |o: usize| table.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
    let (count, strings) = (be16(2)? as usize, be16(4)? as usize);
    let mut found: Option<(u8, String)> = None;
    for i in 0..count {
        let rec = 6 + 12 * i;
        if be16(rec + 6)? != id {
            continue;
        }
        let (platform, language, len, off) = (be16(rec)?, be16(rec + 4)?, be16(rec + 8)? as usize, be16(rec + 10)? as usize);
        let raw = table.get(strings + off..strings + off + len)?;
        let (rank, text) = match platform {
            3 | 0 => (if language == 0x0409 { 0 } else { 2 }, String::from_utf16_lossy(&raw.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect::<Vec<_>>())),
            1 => (if language == 0 { 1 } else { 3 }, raw.iter().map(|b| *b as char).collect()),
            _ => continue,
        };
        if found.as_ref().is_none_or(|(r, _)| rank < *r) {
            found = Some((rank, text));
        }
    }
    found.map(|(_, t)| t).filter(|t| !t.trim().is_empty())
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
    /// A font file holding only `tables`, enough for `sfnt_table`.
    fn sfnt(tables: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut out = vec![0, 1, 0, 0];
        out.extend((tables.len() as u16).to_be_bytes());
        out.extend([0u8; 6]);
        let mut offset = 12 + 16 * tables.len();
        for (tag, body) in tables {
            out.extend(*tag);
            out.extend([0u8; 4]);
            out.extend((offset as u32).to_be_bytes());
            out.extend((body.len() as u32).to_be_bytes());
            offset += body.len();
        }
        for (_, body) in tables {
            out.extend(body);
        }
        out
    }

    #[test]
    fn stylistic_sets_are_listed_by_name_with_their_selectors() {
        let be = |v: &[u16]| v.iter().flat_map(|x| x.to_be_bytes()).collect::<Vec<u8>>();
        // GSUB: a feature list of ss02 (named by name id 256) and ss01 (unnamed), a liga that is no set.
        let mut gsub = be(&[1, 0, 0, 10, 0]);
        let mut list = be(&[3]);
        list.extend(b"ss02");
        list.extend(be(&[20]));
        list.extend(b"ss01");
        list.extend(be(&[28]));
        list.extend(b"liga");
        list.extend(be(&[32]));
        list.extend(be(&[4, 0, 0, 256])); // ss02: params 4 bytes on, no lookups, params: version, name id
        list.extend(be(&[0, 0])); // ss01: no params
        list.extend(be(&[0, 0])); // liga
        gsub.extend(list);
        let text: Vec<u8> = "Open four".encode_utf16().flat_map(|u| u.to_be_bytes()).collect();
        let mut name = be(&[0, 1, 18, 3, 1, 0x0409, 256, text.len() as u16, 0]);
        name.extend(&text);
        let font = sfnt(&[(b"GSUB", gsub), (b"name", name)]);
        assert_eq!(stylistic_sets(&font, 0), vec![(4, "Open four".to_string()), (2, "Stylistic Set 1".to_string())]);
        assert!(stylistic_sets(&sfnt(&[(b"name", Vec::new())]), 0).is_empty(), "no GSUB, no sets");
        assert!(stylistic_sets(&[1, 2, 3], 0).is_empty(), "not a font");
    }

    #[test]
    fn font_choices_add_to_the_levels_features() {
        use crate::profile::FontFeatures;
        use glyphon::cosmic_text::FeatureTag;
        let none = FontFeatures::default();
        assert_eq!(features_with(2, &none).features.len(), features(2).features.len());
        let sets = FontFeatures { stylistic: vec![2, 42], calt_off: false };
        let f = features_with(2, &sets);
        assert!(f.features.iter().any(|x| x.tag == FeatureTag::new(b"ss01")), "selector 2 is ss01");
        assert!(!f.features.iter().any(|x| x.tag.as_bytes().starts_with(b"ss") && x.tag != FeatureTag::new(b"ss01")), "selector 42 is past ss20");
        assert!(features_with(0, &sets).features.iter().all(|x| x.tag != FeatureTag::new(b"ss01")), "a level that does not shape takes no set");
        let off = FontFeatures { stylistic: Vec::new(), calt_off: true };
        let calt = |f: &glyphon::cosmic_text::FontFeatures| f.features.iter().find(|x| x.tag == FeatureTag::CONTEXTUAL_ALTERNATES).map(|x| x.value);
        assert_eq!(calt(&features_with(2, &none)), Some(1));
        assert_eq!(calt(&features_with(2, &off)), Some(0), "Contextual Alternates turned off in the font menu");
    }

    #[test]
    fn a_postscript_name_asks_for_its_weight() {
        assert_eq!(weight_from_style_word("CascadiaMono-Light"), 300);
        assert_eq!(weight_from_style_word("CascadiaMono-SemiBold"), 600);
        assert_eq!(weight_from_style_word("CascadiaMono-BoldItalic"), 700);
        assert_eq!(weight_from_style_word("Consolas"), 400);
        assert_eq!(weight_from_style_word("Foo-Regular"), 400);
        assert_eq!(nearest_weight(&[300, 500], 400), Some(300), "the lighter wins a tie");
        assert_eq!(nearest_weight(&[300, 700], 600), Some(700));
        assert_eq!(nearest_weight(&[], 400), None);
        assert_eq!(weight_name(300), "Light");
        assert_eq!(weight_name(450), "Weight 450");
    }

    #[test]
    fn gasp_decides_the_antialiasing_hint() {
        // version 1, two ranges: grid-fit only up to 8 ppem, gray-scale and grid-fit beyond.
        let table = [0, 1, 0, 2, 0, 8, 0, 1, 0xFF, 0xFF, 0, 3];
        assert!(!gasp_smooths(&table, 8));
        assert!(gasp_smooths(&table, 9));
        assert!(!gasp_smooths(&[], 12), "no table, no hint");
    }

    #[test]
    fn a_font_file_table_is_found_by_its_tag() {
        // A one-table sfnt: the directory says "gasp" is 4 bytes at offset 28.
        let mut f = vec![0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        f.extend_from_slice(b"gasp");
        f.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 28, 0, 0, 0, 4]);
        f.extend_from_slice(&[1, 2, 3, 4]);
        assert_eq!(sfnt_table(&f, 0, b"gasp"), Some(&[1u8, 2, 3, 4][..]));
        assert_eq!(sfnt_table(&f, 0, b"cmap"), None);
    }

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

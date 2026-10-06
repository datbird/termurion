//! iTerm2's color presets (`iTermColorPresets.m`, `ProfilesColorsPreferencesViewController.m`): the
//! built-in list from `ColorPresets.plist`, custom presets in the global "Custom Color Presets",
//! `.itermcolors` files, the apply rule with light and dark modes, the current colors and the
//! check-mark match. Pure.
//!
//! Presets are untrusted data (a file, a stored dictionary from an imported settings file): every
//! reader goes through one sanitize, which keeps only the preset color keys, finite components in
//! 0 to 1 and a known color space, and the lists are bounded.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;
use std::sync::OnceLock;

use toml::Value;

use crate::globals::Globals;
use crate::profile::Profile;

/// A preset's color as the plist has it (raw components and color space); a missing alpha is 1.
#[derive(Clone, Debug, PartialEq)]
pub struct PresetColor {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: Option<f64>,
    pub space: Option<String>,
}

pub type Preset = BTreeMap<String, PresetColor>;

pub const LIGHT: &str = " (Light)";
pub const DARK: &str = " (Dark)";

/// iTerm2's `colorKeysWithModes:` order without modes.
pub const COLOR_KEYS: [&str; 29] = [
    "Ansi 0 Color",
    "Ansi 1 Color",
    "Ansi 2 Color",
    "Ansi 3 Color",
    "Ansi 4 Color",
    "Ansi 5 Color",
    "Ansi 6 Color",
    "Ansi 7 Color",
    "Ansi 8 Color",
    "Ansi 9 Color",
    "Ansi 10 Color",
    "Ansi 11 Color",
    "Ansi 12 Color",
    "Ansi 13 Color",
    "Ansi 14 Color",
    "Ansi 15 Color",
    "Foreground Color",
    "Background Color",
    "Bold Color",
    "Link Color",
    "Match Background Color",
    "Selection Color",
    "Selected Text Color",
    "Cursor Color",
    "Cursor Text Color",
    "Tab Color",
    "Underline Color",
    "Cursor Guide Color",
    "Badge Color",
];

/// A preset file longer than this is refused (a real one is about 10 KiB).
pub const MAX_ITERMCOLORS: usize = 1 << 20;
/// At most this many custom presets are read (extra ones stay stored and unseen).
pub const MAX_CUSTOM: usize = 256;
/// A preset name's longest length, in characters.
pub const MAX_NAME: usize = 64;
/// A preset file holds one dictionary per color, so more containers than this is not a preset.
/// It also bounds the parser's recursion depth.
const MAX_CONTAINERS: usize = 512;

const SEPARATE: &str = "Use Separate Colors for Light and Dark Mode";
const CUSTOM_KEY: &str = "Custom Color Presets";
/// The colors a preset may lack: a missing one is removed, not defaulted (`CP:295-299`).
const OPTIONAL: [&str; 3] = ["Tab Color", "Match Background Color", "Underline Color"];
const SPACES: [&str; 3] = ["sRGB", "P3", "Calibrated"];

/// Which of light and dark mode an operation reads or writes (`iTermColorPresetMode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Modes {
    pub light: bool,
    pub dark: bool,
}

impl Modes {
    pub const NONE: Modes = Modes { light: false, dark: false };
    pub const LIGHT_ONLY: Modes = Modes { light: true, dark: false };
    pub const DARK_ONLY: Modes = Modes { light: false, dark: true };
    pub const BOTH: Modes = Modes { light: true, dark: true };
}

pub fn keys_with_modes(modes: bool) -> Vec<String> {
    if !modes {
        return COLOR_KEYS.iter().map(|k| k.to_string()).collect();
    }
    COLOR_KEYS.iter().flat_map(|k| [format!("{k}{LIGHT}"), format!("{k}{DARK}")]).collect()
}

/// A key's base name and its mode suffix ("" for none).
fn split(key: &str) -> (&str, &'static str) {
    if let Some(base) = key.strip_suffix(LIGHT) {
        (base, LIGHT)
    } else if let Some(base) = key.strip_suffix(DARK) {
        (base, DARK)
    } else {
        (key, "")
    }
}

fn is_preset_key(key: &str) -> bool {
    COLOR_KEYS.contains(&split(key).0)
}

/// The 87 names a preset may hold: the 29 base keys and their 58 mode keys.
fn preset_keys() -> impl Iterator<Item = String> {
    keys_with_modes(false).into_iter().chain(keys_with_modes(true))
}

fn make_color(r: f64, g: f64, b: f64, a: Option<f64>, space: Option<&str>) -> Option<PresetColor> {
    if ![r, g, b].iter().all(|c| c.is_finite() && (0.0..=1.0).contains(c)) {
        return None;
    }
    let a = match a {
        Some(a) if a.is_finite() => Some(a.clamp(0.0, 1.0)),
        Some(_) => return None,
        None => None,
    };
    let space = space.filter(|s| SPACES.contains(s)).map(str::to_string);
    Some(PresetColor { r, g, b, a, space })
}

fn plist_number(v: &plist::Value) -> Option<f64> {
    match v {
        plist::Value::Real(f) => Some(*f),
        plist::Value::Integer(i) => i.as_signed().map(|i| i as f64),
        // Some presets (Tango) write their components as strings.
        plist::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn color_from_plist(v: &plist::Value) -> Option<PresetColor> {
    let d = v.as_dictionary()?;
    let n = |k: &str| d.get(k).and_then(plist_number);
    let alpha = match d.get("Alpha Component") {
        Some(v) => Some(plist_number(v)?),
        None => None,
    };
    make_color(n("Red Component")?, n("Green Component")?, n("Blue Component")?, alpha, d.get("Color Space").and_then(|v| v.as_string()))
}

fn preset_from_plist(d: &plist::Dictionary) -> Option<Preset> {
    let mut p = Preset::new();
    for key in preset_keys() {
        if let Some(c) = d.get(&key).and_then(color_from_plist) {
            p.insert(key, c);
        }
    }
    (!p.is_empty()).then_some(p)
}

fn number(v: &Value) -> Option<f64> {
    v.as_float().or_else(|| v.as_integer().map(|i| i as f64))
}

/// A color in profile form, `{r, g, b, a}` (and an optional "space" in a stored preset).
fn color_from_value(v: &Value) -> Option<PresetColor> {
    let t = v.as_table()?;
    let n = |k: &str| t.get(k).and_then(number);
    let alpha = match t.get("a") {
        Some(v) => Some(number(v)?),
        None => None,
    };
    make_color(n("r")?, n("g")?, n("b")?, alpha, t.get("space").and_then(|v| v.as_str()))
}

fn value_of(c: &PresetColor) -> Value {
    let mut t = toml::Table::new();
    for (k, v) in [("r", c.r), ("g", c.g), ("b", c.b)] {
        t.insert(k.into(), Value::Float(v));
    }
    if let Some(a) = c.a {
        t.insert("a".into(), Value::Float(a));
    }
    if let Some(s) = &c.space {
        t.insert("space".into(), Value::String(s.clone()));
    }
    Value::Table(t)
}

/// The one reader of a stored preset (a table of color tables): only the preset color keys, each a
/// color with finite components in 0 to 1 (alpha clamped, absent is 1) and a known color space.
/// None when nothing usable is left.
pub fn sanitize(raw: &Value) -> Option<Preset> {
    let t = raw.as_table()?;
    let mut p = Preset::new();
    for key in preset_keys() {
        if let Some(c) = t.get(&key).and_then(color_from_value) {
            p.insert(key, c);
        }
    }
    (!p.is_empty()).then_some(p)
}

/// A name that may be shown: 1 to 64 characters, none a control, bidi or zero-width character.
fn name_ok(name: &str) -> bool {
    let hostile = |c: char| {
        c.is_control()
            || matches!(c, '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
    };
    let n = name.chars().count();
    (1..=MAX_NAME).contains(&n) && !name.chars().any(hostile)
}

pub fn built_in() -> &'static [(String, Preset)] {
    static BUILT_IN: OnceLock<Vec<(String, Preset)>> = OnceLock::new();
    BUILT_IN.get_or_init(|| {
        let root = plist::Value::from_reader(std::io::Cursor::new(&include_bytes!("../resources/ColorPresets.plist")[..]))
            .expect("resources/ColorPresets.plist must parse");
        let mut list: Vec<(String, Preset)> = root
            .as_dictionary()
            .expect("ColorPresets.plist is a dictionary")
            .iter()
            .filter_map(|(name, v)| Some((name.clone(), preset_from_plist(v.as_dictionary()?)?)))
            .collect();
        list.sort_by(|a, b| a.0.cmp(&b.0));
        list
    })
}

/// The custom presets, sorted by name: sanitized, at most 256, hostile names hidden.
pub fn custom(g: &Globals) -> Vec<(String, Preset)> {
    let mut list: Vec<(String, Preset)> = g
        .table(CUSTOM_KEY)
        .iter()
        .filter(|(name, _)| name_ok(name))
        .filter_map(|(name, v)| Some((name.clone(), sanitize(v)?)))
        .take(MAX_CUSTOM)
        .collect();
    list.sort_by(|a, b| a.0.cmp(&b.0));
    list
}

/// `presetWithName:`: built-ins first, then custom presets.
pub fn find(g: &Globals, name: &str) -> Option<Preset> {
    if let Some((_, p)) = built_in().iter().find(|(n, _)| n == name) {
        return Some(p.clone());
    }
    custom(g).into_iter().find(|(n, _)| n == name).map(|(_, p)| p)
}

/// `allColorPresets`: built-in and custom names, a clash listed once, sorted.
pub fn all_names(g: &Globals) -> Vec<String> {
    let mut names: BTreeSet<String> = built_in().iter().map(|(n, _)| n.clone()).collect();
    names.extend(custom(g).into_iter().map(|(n, _)| n));
    names.into_iter().collect()
}

/// A preset has light and dark colors when it has "Foreground Color (Light)".
pub fn has_modes(p: &Preset) -> bool {
    p.contains_key(&format!("Foreground Color{LIGHT}"))
}

/// `iTermColorPresetGet`: a mode-less preset's base key; else the mode key, then the base key.
pub fn color_of(p: &Preset, base: &str, dark: bool) -> Option<PresetColor> {
    if !has_modes(p) {
        return p.get(base).cloned();
    }
    let key = format!("{base}{}", if dark { DARK } else { LIGHT });
    p.get(&key).or_else(|| p.get(base)).cloned()
}

/// The profile's default for a color key, as iTerm2 reads a missing one ("(Dark)" and "(Light)"
/// variants have their own defaults).
fn schema_default(key: &str) -> Option<Value> {
    let (base, suffix) = split(key);
    let s = crate::schema::find(base)?;
    match suffix {
        DARK => s.default_dark.clone().or_else(|| s.default.clone()),
        LIGHT => s.default_light.clone().or_else(|| s.default.clone()),
        _ => s.default.clone(),
    }
}

/// What a preset lacking `key` gives it when applied: the default, but nothing for the optional
/// colors, which are removed.
fn fallback(key: &str) -> Option<Value> {
    if OPTIONAL.contains(&split(key).0) { None } else { schema_default(key) }
}

/// The preset's value for a key as a profile value, else `fallback`.
fn preset_value(p: &Preset, key: &str) -> Option<Value> {
    match p.get(key) {
        Some(c) => Some(crate::import::preset_color_to_value(c)),
        None => fallback(key),
    }
}

/// `addColorPresetNamed:toProfile:from:to:updateUseModes:`.
pub fn apply(p: &Preset, profile: &mut Profile, from: Modes, to: Modes, update_use_modes: bool) {
    let uses_modes = has_modes(p);
    let mut from = from;
    if !uses_modes {
        // A mode-less preset has no modes to read from.
        from = Modes::NONE;
    }
    // The suffixes of the preset's keys that are ignored.
    let skip: Vec<&str> = if !uses_modes || from == Modes::NONE {
        vec![LIGHT, DARK]
    } else if from == Modes::LIGHT_ONLY {
        vec!["", DARK]
    } else if from == Modes::DARK_ONLY {
        vec!["", LIGHT]
    } else {
        vec![""]
    };
    // The suffixes written to.
    let mut set: Vec<&str> = Vec::new();
    if to == Modes::NONE {
        set.push("");
    }
    if to.light {
        set.push(LIGHT);
    }
    if to.dark {
        set.push(DARK);
    }
    for key in keys_with_modes(uses_modes) {
        let (base, suffix) = split(&key);
        if skip.contains(&suffix) {
            continue;
        }
        let value = preset_value(p, &key);
        for s in &set {
            // iTerm2 fans every key out to every written suffix: with both modes read into both,
            // the later (dark) key's value is the one left in each.
            let target = format!("{base}{s}");
            match &value {
                Some(v) => profile.set(&target, v.clone()),
                None => {
                    profile.values.remove(&target);
                }
            }
        }
    }
    if update_use_modes {
        profile.set(SEPARATE, Value::Boolean(to != Modes::NONE));
    }
}

/// `addColorPresetNamed:toProfile:`: both modes when the preset has them, else none.
pub fn apply_whole(p: &Preset, profile: &mut Profile) {
    let modes = if has_modes(p) { Modes::BOTH } else { Modes::NONE };
    apply(p, profile, modes, modes, true);
}

/// `setColorsFromPresetNamed:` as session values: every color key of the preset's key set, then the
/// separate-colors switch. None removes the key.
pub fn session_values(p: &Preset) -> Vec<(String, Option<Value>)> {
    let uses_modes = has_modes(p);
    let mut out: Vec<(String, Option<Value>)> = keys_with_modes(uses_modes).into_iter().map(|k| {
        let v = preset_value(p, &k);
        (k, v)
    }).collect();
    out.push((SEPARATE.to_string(), Some(Value::Boolean(uses_modes))));
    out
}

/// The profile's value for a color key: its own, else the default.
fn profile_value(profile: &Profile, key: &str) -> Option<Value> {
    profile.values.get(key).cloned().or_else(|| schema_default(key))
}

/// `presetDictionaryForCurrentColors`: the shown colors under the base keys (the shown mode with
/// separate colors), and with separate colors both mode keys too.
pub fn current(profile: &Profile, editing_dark: bool) -> Preset {
    let separate = profile.bool(SEPARATE);
    let mut p = Preset::new();
    let suffix = if editing_dark { DARK } else { LIGHT };
    for base in COLOR_KEYS {
        let key = if separate { format!("{base}{suffix}") } else { base.to_string() };
        if let Some(c) = profile_value(profile, &key).as_ref().and_then(profile_color) {
            p.insert(base.to_string(), c);
        }
    }
    if separate {
        for key in keys_with_modes(true) {
            if let Some(c) = profile_value(profile, &key).as_ref().and_then(profile_color) {
                p.insert(key, c);
            }
        }
    }
    p
}

/// A profile color (sRGB floats) as a preset color.
fn profile_color(v: &Value) -> Option<PresetColor> {
    let mut c = color_from_value(v)?;
    c.a = Some(c.a.unwrap_or(1.0));
    c.space = Some("sRGB".into());
    Some(c)
}

fn same_color(a: &Value, b: &Value) -> bool {
    match (color_from_value(a), color_from_value(b)) {
        (Some(x), Some(y)) => {
            let close = |p: f64, q: f64| (p - q).abs() < 1e-6;
            close(x.r, y.r) && close(x.g, y.g) && close(x.b, y.b) && close(x.a.unwrap_or(1.0), y.a.unwrap_or(1.0))
        }
        _ => a == b,
    }
}

fn same_value(a: &Option<Value>, b: &Option<Value>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => same_color(a, b),
        _ => false,
    }
}

/// `currentColorsEqualPreset:modal:dark:`: `None` checks both modes, `Some(dark)` one mode.
pub fn matches(p: &Preset, profile: &Profile, modal: Option<bool>) -> bool {
    let uses_modes = has_modes(p);
    let separate = profile.bool(SEPARATE);
    let amended = |base: &str, modes: bool, dark: bool| {
        if modes { format!("{base}{}", if dark { DARK } else { LIGHT }) } else { base.to_string() }
    };
    match modal {
        None => {
            if uses_modes != separate {
                return false;
            }
            keys_with_modes(uses_modes).iter().all(|k| same_value(&preset_value_for_match(p, k), &profile_value(profile, k)))
        }
        Some(dark) => COLOR_KEYS.iter().all(|base| {
            let pk = amended(base, uses_modes, dark);
            let rk = amended(base, separate, dark);
            same_value(&preset_value_for_match(p, &pk), &profile_value(profile, &rk))
        }),
    }
}

/// A preset's value for the check mark: its color, else the default (iTerm2 reads a preset's
/// missing key as the profile default, optional colors included).
fn preset_value_for_match(p: &Preset, key: &str) -> Option<Value> {
    match p.get(key) {
        Some(c) => Some(crate::import::preset_color_to_value(c)),
        None => schema_default(key),
    }
}

fn oklab_of(c: &PresetColor) -> crate::oklab::OkLab {
    let v = crate::import::preset_color_to_value(c);
    let t = v.as_table();
    let byte = |k: &str| (t.and_then(|t| t.get(k)).and_then(number).unwrap_or(0.0).clamp(0.0, 1.0) * 255.0).round() as u8;
    crate::oklab::from_srgb(crate::palette::Rgb { r: byte("r"), g: byte("g"), b: byte("b") })
}

/// `differenceBetweenPreset:andPreset:usingKeys:` over the second preset's keys: 1 for a key either
/// lacks, else the perceptual distance (Decision: Euclidean distance in OKLab).
pub fn difference(a: &Preset, b: &Preset) -> f64 {
    b.iter()
        .map(|(key, bc)| match a.get(key) {
            Some(ac) => crate::oklab::distance(oklab_of(ac), oklab_of(bc)),
            None => 1.0,
        })
        .sum()
}

/// A `.itermcolors` file (an XML plist whose root is a dictionary of colors). Binary plists and
/// anything over `MAX_ITERMCOLORS` bytes or with more than 512 containers are refused, so a
/// hostile file cannot cost more than a bounded parse.
pub fn read_itermcolors(bytes: &[u8]) -> Option<Preset> {
    if bytes.len() > MAX_ITERMCOLORS || bytes.starts_with(b"bplist") {
        return None;
    }
    let containers = bytes.windows(5).filter(|w| *w == b"<dict" || *w == b"<arra").count();
    if containers > MAX_CONTAINERS {
        return None;
    }
    let root = plist::Value::from_reader(std::io::Cursor::new(bytes)).ok()?;
    preset_from_plist(root.as_dictionary()?)
}

/// `read_itermcolors` over a file, reading at most one byte past the limit.
pub fn read_file(path: &Path) -> Option<Preset> {
    let mut bytes = Vec::new();
    std::fs::File::open(path).ok()?.take(MAX_ITERMCOLORS as u64 + 1).read_to_end(&mut bytes).ok()?;
    read_itermcolors(&bytes)
}

/// The preset as an XML plist, keys sorted (what iTerm2's Export writes).
pub fn write_itermcolors(p: &Preset) -> Vec<u8> {
    let mut root = plist::Dictionary::new();
    for (key, c) in p {
        let mut d = plist::Dictionary::new();
        if let Some(a) = c.a {
            d.insert("Alpha Component".into(), plist::Value::Real(a));
        }
        d.insert("Blue Component".into(), plist::Value::Real(c.b));
        if let Some(s) = &c.space {
            d.insert("Color Space".into(), plist::Value::String(s.clone()));
        }
        d.insert("Green Component".into(), plist::Value::Real(c.g));
        d.insert("Red Component".into(), plist::Value::Real(c.r));
        root.insert(key.clone(), plist::Value::Dictionary(d));
    }
    let mut out = Vec::new();
    plist::Value::Dictionary(root).to_writer_xml(&mut out).expect("writing to a Vec cannot fail");
    out
}

/// `presetNameFromFilename:`: the file's name without its last extension (either slash).
pub fn name_from_file(path: &Path) -> String {
    let text = path.to_string_lossy();
    let name = text.rsplit(['/', '\\']).next().unwrap_or("");
    match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem.to_string(),
        _ => name.to_string(),
    }
}

fn same_preset(a: &Preset, b: &Preset) -> bool {
    a.len() == b.len()
        && a.iter().all(|(k, x)| {
            b.get(k).is_some_and(|y| {
                let close = |p: f64, q: f64| (p - q).abs() < 1e-9;
                close(x.r, y.r) && close(x.g, y.g) && close(x.b, y.b) && close(x.a.unwrap_or(1.0), y.a.unwrap_or(1.0)) && x.space == y.space
            })
        })
}

/// `nameOfPresetsEqualTo:`: a custom preset equal to `p`.
pub fn duplicate_of(g: &Globals, p: &Preset) -> Option<String> {
    custom(g).into_iter().find(|(_, c)| same_preset(c, p)).map(|(n, _)| n)
}

/// `addColorPreset:withColors:`: stores the preset; a name already taken becomes "Name (2)", "(3)"...
/// Returns the name used, or an error text when `MAX_CUSTOM` presets are stored. The name is
/// cleaned to what `custom` will show.
pub fn add_custom(g: &mut Globals, name: &str, p: Preset) -> Result<String, String> {
    let mut base: String = name.chars().filter(|c| name_ok(&c.to_string())).take(MAX_NAME - 6).collect();
    if base.is_empty() {
        base = "Untitled".into();
    }
    let mut stored = g.table(CUSTOM_KEY);
    if stored.len() >= MAX_CUSTOM {
        return Err(format!("At most {MAX_CUSTOM} custom color presets can be kept."));
    }
    let mut used = base.clone();
    let mut i = 1;
    while stored.contains_key(&used) {
        i += 1;
        used = format!("{base} ({i})");
    }
    let t: toml::Table = p.iter().filter(|(k, _)| is_preset_key(k)).map(|(k, c)| (k.clone(), value_of(c))).collect();
    stored.insert(used.clone(), Value::Table(t));
    g.set(CUSTOM_KEY, Value::Table(stored));
    Ok(used)
}

pub fn delete_custom(g: &mut Globals, name: &str) {
    let mut stored = g.table(CUSTOM_KEY);
    stored.remove(name);
    g.set(CUSTOM_KEY, Value::Table(stored));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::Profile;
    use std::path::Path;
    use toml::Value;

    fn globals() -> Globals {
        Globals::empty(Path::new("color-presets-test-unused.toml"))
    }

    fn named(name: &str) -> Preset {
        built_in().iter().find(|(n, _)| n == name).unwrap_or_else(|| panic!("{name}")).1.clone()
    }

    fn color(r: f64, g: f64, b: f64) -> PresetColor {
        PresetColor { r, g, b, a: Some(1.0), space: Some("sRGB".into()) }
    }

    fn table(r: f64, g: f64, b: f64) -> Value {
        let mut t = toml::Table::new();
        for (k, v) in [("r", r), ("g", g), ("b", b), ("a", 1.0)] {
            t.insert(k.into(), Value::Float(v));
        }
        Value::Table(t)
    }

    #[test]
    fn built_in_presets_follow_iterm2() {
        let names: Vec<&str> = built_in().iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            ["Dark Background", "High Contrast", "Light Background", "Pastel (Dark Background)", "Regular", "Smoooooth", "Solarized", "Solarized Dark", "Solarized Light", "Tango Dark", "Tango Light"]
        );
        for (name, p) in built_in() {
            assert_eq!(has_modes(p), ["High Contrast", "Regular", "Solarized"].contains(&name.as_str()), "{name}");
        }
        let bg = &named("Solarized Dark")["Background Color"];
        assert_eq!((bg.r, bg.g, bg.b), (0.0, 0.16862745098039217, 0.21176470588235294));
        assert_eq!(bg.space.as_deref(), Some("sRGB"));
        // Tango Dark is Calibrated (no space), and its components are written as strings.
        let red = &named("Tango Dark")["Ansi 1 Color"];
        assert_eq!((red.r, red.g, red.b, red.space.as_deref()), (0.8, 0.0, 0.0, None));
        assert_eq!(COLOR_KEYS.len(), 29);
        assert_eq!(keys_with_modes(false).len(), 29);
        assert_eq!(keys_with_modes(true).len(), 58);
        assert_eq!(keys_with_modes(true)[0], "Ansi 0 Color (Light)");
        assert_eq!(keys_with_modes(true)[1], "Ansi 0 Color (Dark)");
        assert_eq!((COLOR_KEYS[0], COLOR_KEYS[16], COLOR_KEYS[28]), ("Ansi 0 Color", "Foreground Color", "Badge Color"));
    }

    #[test]
    fn apply_follows_iterm2() {
        let mut profile = Profile::new("t");
        profile.set("Tab Color", color_value_of(0.5, 0.5, 0.5));
        let dark = named("Solarized Dark");
        apply_whole(&dark, &mut profile);
        assert_eq!(profile.values["Ansi 0 Color"], crate::import::preset_color_to_value(&dark["Ansi 0 Color"]));
        assert_eq!(profile.values["Use Separate Colors for Light and Dark Mode"], Value::Boolean(false));
        assert!(!profile.values.contains_key("Tab Color"));

        let solarized = named("Solarized");
        apply_whole(&solarized, &mut profile);
        for suffix in [LIGHT, DARK] {
            let key = format!("Ansi 0 Color{suffix}");
            assert_eq!(profile.values[&key], crate::import::preset_color_to_value(&solarized[&key]), "{key}");
        }
        assert_eq!(profile.values["Use Separate Colors for Light and Dark Mode"], Value::Boolean(true));
        // iTerm2's loop fans every key out to every written suffix, so reading both modes into
        // both leaves the dark value in each (the Colors tab avoids it by loading one mode at a time).
        assert_ne!(solarized["Foreground Color (Light)"], solarized["Foreground Color (Dark)"]);
        let dark_fg = crate::import::preset_color_to_value(&solarized["Foreground Color (Dark)"]);
        assert_eq!(profile.values["Foreground Color (Light)"], dark_fg);
        assert_eq!(profile.values["Foreground Color (Dark)"], dark_fg);

        let mut only_dark = Profile::new("t");
        let before = only_dark.values.clone();
        apply(&solarized, &mut only_dark, Modes::DARK_ONLY, Modes::DARK_ONLY, false);
        let changed: Vec<&String> = only_dark.values.keys().filter(|k| before.get(*k) != only_dark.values.get(*k)).collect();
        assert!(!changed.is_empty());
        assert!(changed.iter().all(|k| k.ends_with(DARK)), "{changed:?}");
        assert_eq!(only_dark.values["Foreground Color (Dark)"], crate::import::preset_color_to_value(&solarized["Foreground Color (Dark)"]));

        let tango = named("Tango Dark");
        let mut mode_less = Profile::new("t");
        apply(&tango, &mut mode_less, Modes::LIGHT_ONLY, Modes::NONE, false);
        assert_eq!(mode_less.values["Ansi 1 Color"], crate::import::preset_color_to_value(&tango["Ansi 1 Color"]));
        assert_eq!(mode_less.values["Use Separate Colors for Light and Dark Mode"], Profile::new("t").values["Use Separate Colors for Light and Dark Mode"]);

        let values = session_values(&solarized);
        assert_eq!(values.iter().filter(|(k, _)| k != "Use Separate Colors for Light and Dark Mode").count(), 58);
        assert_eq!(values.last().unwrap(), &("Use Separate Colors for Light and Dark Mode".to_string(), Some(Value::Boolean(true))));
        // Solarized has no tab, match or underline colors: those three are removed, in both modes.
        let removed: Vec<&str> = values.iter().filter(|(_, v)| v.is_none()).map(|(k, _)| k.as_str()).collect();
        assert_eq!(removed.len(), 6, "{removed:?}");
        assert!(removed.iter().all(|k| k.starts_with("Tab Color") || k.starts_with("Match Background Color") || k.starts_with("Underline Color")));
        // A mode-less preset: the 29 base keys, a lacking Link Color falls back to the default.
        let plain = session_values(&dark);
        assert_eq!(plain.len(), 30);
        let link = plain.iter().find(|(k, _)| k == "Link Color").unwrap();
        assert!(link.1.is_some());
        assert_eq!(plain.last().unwrap().1, Some(Value::Boolean(false)));
    }

    fn color_value_of(r: f64, g: f64, b: f64) -> Value {
        table(r, g, b)
    }

    #[test]
    fn custom_presets_follow_iterm2() {
        let mut g = globals();
        let profile = Profile::new("t");
        let p = current(&profile, false);
        assert_eq!(add_custom(&mut g, "Mine", p.clone()).unwrap(), "Mine");
        assert_eq!(add_custom(&mut g, "Mine", p.clone()).unwrap(), "Mine (2)");
        assert_eq!(add_custom(&mut g, "Mine", p.clone()).unwrap(), "Mine (3)");
        assert_eq!(duplicate_of(&g, &p).as_deref(), Some("Mine"));
        delete_custom(&mut g, "Mine (2)");
        let names: Vec<String> = custom(&g).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["Mine", "Mine (3)"]);
        let mut other = p.clone();
        other.insert("Foreground Color".into(), color(0.1, 0.2, 0.3));
        assert_eq!(duplicate_of(&g, &other), None);
        // Built-ins win a name clash, and the merged list names a clash once.
        add_custom(&mut g, "Regular", other).unwrap();
        assert_eq!(find(&g, "Regular"), Some(named("Regular")));
        assert_eq!(all_names(&g).iter().filter(|n| *n == "Regular").count(), 1);
        assert_eq!(find(&g, "Mine"), Some(p));
        assert_eq!(find(&g, "Nope"), None);
        let all = all_names(&g);
        assert_eq!(all.len(), 11 + 2);
    }

    #[test]
    fn itermcolors_round_trip() {
        let mut profile = Profile::new("t");
        profile.set("Use Separate Colors for Light and Dark Mode", Value::Boolean(false));
        let now = current(&profile, false);
        assert_eq!(read_itermcolors(&write_itermcolors(&now)), Some(now.clone()));
        assert!(matches(&now, &profile, None));
        profile.set("Ansi 3 Color", table(0.9, 0.1, 0.2));
        assert!(!matches(&now, &profile, None));
        // A file whose root is not a dictionary.
        let array = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><array/></plist>";
        assert_eq!(read_itermcolors(array), None);
        assert_eq!(read_itermcolors(b"not a plist"), None);
        assert_eq!(name_from_file(Path::new(r"C:\x\My Theme.itermcolors")), "My Theme");
        assert_eq!(name_from_file(Path::new("/x/Solar.v2.itermcolors")), "Solar.v2");
    }

    #[test]
    fn current_and_matches_follow_iterm2() {
        let mut profile = Profile::new("t");
        let solarized = named("Solarized");
        // As the Colors tab loads a preset with modes: light into light, then dark into dark.
        apply(&solarized, &mut profile, Modes::LIGHT_ONLY, Modes::LIGHT_ONLY, true);
        apply(&solarized, &mut profile, Modes::DARK_ONLY, Modes::DARK_ONLY, true);
        let shown_dark = current(&profile, true);
        assert_eq!(shown_dark["Foreground Color"].r, solarized["Foreground Color (Dark)"].r);
        assert_eq!(current(&profile, false)["Foreground Color"].r, solarized["Foreground Color (Light)"].r);
        assert!(has_modes(&shown_dark));
        assert!(matches(&solarized, &profile, None));
        // A mode-less preset never matches both modes of a separate-colors profile, but it matches
        // one mode it was loaded into.
        assert!(!matches(&named("Solarized Dark"), &profile, None));
        apply(&named("Solarized Dark"), &mut profile, Modes::NONE, Modes::DARK_ONLY, false);
        apply(&named("Solarized Light"), &mut profile, Modes::NONE, Modes::LIGHT_ONLY, false);
        assert!(matches(&named("Solarized Dark"), &profile, Some(true)));
        assert!(matches(&named("Solarized Light"), &profile, Some(false)));
        assert!(!matches(&named("Solarized Light"), &profile, Some(true)));
        assert!(!matches(&named("Solarized"), &profile, None));
        let shown_dark = current(&profile, true);
        // Exported with separate colors, the current dictionary survives a file.
        assert_eq!(read_itermcolors(&write_itermcolors(&shown_dark)), Some(shown_dark));
    }

    #[test]
    fn add_custom_stops_at_the_cap() {
        let mut g = globals();
        let p = current(&Profile::new("t"), false);
        for i in 0..MAX_CUSTOM {
            add_custom(&mut g, &format!("P{i}"), p.clone()).unwrap();
        }
        assert!(add_custom(&mut g, "One more", p.clone()).is_err());
        assert_eq!(g.table(CUSTOM_KEY).len(), MAX_CUSTOM);
    }

    #[test]
    fn color_of_follows_iterm2() {
        let s = named("Solarized");
        assert_eq!(color_of(&s, "Foreground Color", true), s.get("Foreground Color (Dark)").cloned());
        assert_eq!(color_of(&s, "Foreground Color", false), s.get("Foreground Color (Light)").cloned());
        let d = named("Solarized Dark");
        assert_eq!(color_of(&d, "Foreground Color", true), d.get("Foreground Color").cloned());
        assert_eq!(color_of(&d, "Tab Color", true), None);
    }

    #[test]
    fn difference_follows_iterm2() {
        let a = named("Solarized Dark");
        assert_eq!(difference(&a, &a), 0.0);
        let mut b = a.clone();
        b.insert("Foreground Color".into(), color(1.0, 0.0, 0.0));
        let d = difference(&a, &b);
        assert!(d > 0.1 && d < 1.0, "{d}");
        // A key the first lacks counts 1 (the second's keys are walked).
        let mut c = a.clone();
        c.remove("Foreground Color");
        assert_eq!(difference(&c, &a), 1.0);
    }

    fn itermcolors_with(body: &str) -> Vec<u8> {
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict>{body}</dict></plist>").into_bytes()
    }

    fn entry(key: &str, r: &str, g: &str, b: &str) -> String {
        format!("<key>{key}</key><dict><key>Red Component</key><real>{r}</real><key>Green Component</key><real>{g}</real><key>Blue Component</key><real>{b}</real></dict>")
    }

    #[test]
    fn itermcolors_untrusted_file_is_bounded() {
        // One byte over the limit is refused; the limit itself is not a size problem.
        let mut big = itermcolors_with(&entry("Ansi 0 Color", "0.1", "0.2", "0.3"));
        big.resize(MAX_ITERMCOLORS + 1, b' ');
        assert_eq!(read_itermcolors(&big), None);
        // Non-finite and out-of-range components drop the key; an unknown key is dropped.
        let body = [
            entry("Ansi 0 Color", "0.1", "0.2", "0.3"),
            entry("Ansi 1 Color", "2.0", "0.2", "0.3"),
            entry("Ansi 2 Color", "NaN", "0.2", "0.3"),
            entry("Ansi 3 Color", "inf", "0.2", "0.3"),
            entry("Command", "0.5", "0.5", "0.5"),
        ]
        .concat();
        let p = read_itermcolors(&itermcolors_with(&body)).unwrap();
        assert_eq!(p.keys().map(String::as_str).collect::<Vec<_>>(), ["Ansi 0 Color"]);
        // A file with nothing usable is not a preset.
        assert_eq!(read_itermcolors(&itermcolors_with(&entry("Nope", "0.1", "0.2", "0.3"))), None);
        // Deeply nested containers are refused before the parser sees them.
        let deep = format!("{}{}", "<dict><key>a</key>".repeat(5000), "<dict/>").into_bytes();
        let mut file = b"<?xml version=\"1.0\"?><plist version=\"1.0\">".to_vec();
        file.extend(deep);
        file.extend("</dict>".repeat(5000).bytes());
        file.extend(b"</plist>");
        assert_eq!(read_itermcolors(&file), None);
        // An unknown color space is dropped, a known one kept.
        let spaced = "<key>Ansi 0 Color</key><dict><key>Red Component</key><real>0.1</real><key>Green Component</key><real>0.2</real><key>Blue Component</key><real>0.3</real><key>Color Space</key><string>Bogus</string></dict>";
        assert_eq!(read_itermcolors(&itermcolors_with(spaced)).unwrap()["Ansi 0 Color"].space, None);
    }

    #[test]
    fn custom_presets_from_an_imported_plist_are_sanitized() {
        let mut g = globals();
        let mut stored = toml::Table::new();
        for i in 0..300 {
            let mut p = toml::Table::new();
            p.insert("Ansi 0 Color".into(), table(0.1, 0.2, 0.3));
            stored.insert(format!("P{i:03}"), Value::Table(p));
        }
        let mut hostile = toml::Table::new();
        hostile.insert("Ansi 0 Color".into(), table(f64::NAN, 0.2, 0.3));
        hostile.insert("Ansi 1 Color".into(), table(0.1, 2.0, 0.3));
        hostile.insert("Command".into(), Value::String("calc.exe".into()));
        hostile.insert("Foreground Color".into(), table(0.5, 0.5, 0.5));
        stored.insert("A hostile".into(), Value::Table(hostile));
        stored.insert("bad\u{202e}name".into(), Value::Table([("Ansi 0 Color".to_string(), table(0.1, 0.1, 0.1))].into_iter().collect()));
        stored.insert("not a table".into(), Value::Integer(3));
        stored.insert("x".repeat(65), Value::Table([("Ansi 0 Color".to_string(), table(0.1, 0.1, 0.1))].into_iter().collect()));
        g.values.insert("Custom Color Presets".into(), Value::Table(stored));
        let list = custom(&g);
        assert_eq!(list.len(), 256);
        assert!(list.iter().all(|(n, _)| !n.contains('\u{202e}') && n.chars().count() <= 64));
        let hostile = find(&g, "A hostile").unwrap();
        assert_eq!(hostile.keys().map(String::as_str).collect::<Vec<_>>(), ["Foreground Color"]);
        assert_eq!(find(&g, "bad\u{202e}name"), None);
        // Alpha is clamped, and a name that is not a table or has no usable key is hidden.
        let mut a = toml::Table::new();
        let mut c = table(0.1, 0.2, 0.3);
        c.as_table_mut().unwrap().insert("a".into(), Value::Float(7.0));
        a.insert("Badge Color".into(), c);
        assert_eq!(sanitize(&Value::Table(a)).unwrap()["Badge Color"].a, Some(1.0));
        assert_eq!(sanitize(&Value::Integer(1)), None);
        assert_eq!(sanitize(&Value::Table(toml::Table::new())), None);
        // The stored form reads back through the same path: no space, profile-form floats.
        let mut g2 = globals();
        let p = current(&Profile::new("t"), false);
        add_custom(&mut g2, "Mine", p.clone()).unwrap();
        assert_eq!(find(&g2, "Mine"), Some(p));
    }

    #[test]
    fn read_file_is_bounded() {
        let dir = std::env::temp_dir().join(format!("tandem-cp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ok = dir.join("ok.itermcolors");
        std::fs::write(&ok, itermcolors_with(&entry("Ansi 0 Color", "0.1", "0.2", "0.3"))).unwrap();
        assert!(read_file(&ok).is_some());
        let big = dir.join("big.itermcolors");
        let mut bytes = itermcolors_with(&entry("Ansi 0 Color", "0.1", "0.2", "0.3"));
        bytes.resize(MAX_ITERMCOLORS + 1, b' ');
        std::fs::write(&big, bytes).unwrap();
        assert!(read_file(&big).is_none());
        assert!(read_file(&dir.join("missing.itermcolors")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

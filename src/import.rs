//! Imports iTerm2 profiles: the full settings plist (com.googlecode.iterm2.plist)
//! or a Dynamic Profiles JSON file. This is a one-time copy, not a sync.

use std::path::Path;

use toml::Value;

use crate::profile::Profile;

/// Weight and style words iTerm2's PostScript names carry after the family.
const STYLE_WORDS: [&str; 13] =
    ["Regular", "Book", "Medium", "Bold", "Italic", "BoldItalic", "Light", "Thin", "SemiBold", "ExtraBold", "Black", "Oblique", "Normal"];

/// "MesloLGS-NF-Regular" becomes "MesloLGS NF". A trailing style word is dropped,
/// and the other dashes become spaces.
pub fn family_from_postscript(name: &str) -> String {
    let mut parts: Vec<&str> = name.split('-').collect();
    if parts.len() > 1 && STYLE_WORDS.contains(parts.last().unwrap()) {
        parts.pop();
    }
    parts.join(" ")
}

/// Converts a Display P3 color to sRGB (both gamma encoded, 0 to 1), clipped to the sRGB gamut.
pub fn p3_to_srgb(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let lin = |c: f64| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
    let enc = |c: f64| {
        let c = c.clamp(0.0, 1.0);
        if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
    };
    let (r, g, b) = (lin(r), lin(g), lin(b));
    // Linear Display P3 to linear sRGB. Each row sums to 1, so grays stay gray.
    let sr = 1.2249401 * r - 0.2249404 * g;
    let sg = -0.0420569 * r + 1.0420571 * g;
    let sb = -0.0196376 * r - 0.0786361 * g + 1.0982735 * b;
    let round = |v: f64| (v * 1e6).round() / 1e6;
    (round(enc(sr)), round(enc(sg)), round(enc(sb)))
}

/// Converts an sRGB color to Display P3 (both gamma encoded, 0 to 1). sRGB lies inside P3, so
/// nothing clips; the inverse of `p3_to_srgb` for those colors.
pub fn srgb_to_p3(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let lin = |c: f64| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
    let enc = |c: f64| {
        let c = c.clamp(0.0, 1.0);
        if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
    };
    let (r, g, b) = (lin(r), lin(g), lin(b));
    // Linear sRGB to linear Display P3: the inverse of `p3_to_srgb`'s matrix, rows summing to 1.
    let pr = 0.822462 * r + 0.177538 * g;
    let pg = 0.0331942 * r + 0.9668057 * g;
    let pb = 0.0170827 * r + 0.0723975 * g + 0.91052 * b;
    (enc(pr), enc(pg), enc(pb))
}

pub(crate) fn color_table(get: impl Fn(&str) -> Option<f64>, space: Option<&str>) -> Option<Value> {
    let (mut r, mut g, mut b) = (get("Red Component")?, get("Green Component")?, get("Blue Component")?);
    if space == Some("P3") {
        (r, g, b) = p3_to_srgb(r, g, b);
    }
    let mut t = toml::Table::new();
    t.insert("r".into(), Value::Float(r));
    t.insert("g".into(), Value::Float(g));
    t.insert("b".into(), Value::Float(b));
    t.insert("a".into(), Value::Float(get("Alpha Component").unwrap_or(1.0)));
    Some(Value::Table(t))
}

/// A preset color as a profile value: `{r, g, b, a}` in sRGB (a P3 color converted, a missing alpha 1).
pub(crate) fn preset_color_to_value(c: &crate::color_presets::PresetColor) -> Value {
    let get = |k: &str| match k {
        "Red Component" => Some(c.r),
        "Green Component" => Some(c.g),
        "Blue Component" => Some(c.b),
        _ => c.a,
    };
    color_table(get, c.space.as_deref()).expect("a preset color has all three components")
}

pub(crate) fn from_plist(v: &plist::Value) -> Option<Value> {
    from_plist_at(v, 0)
}

/// Nesting deeper than any real file is dropped (a crafted file cannot exhaust the stack).
const MAX_NESTING: usize = 64;

fn from_plist_at(v: &plist::Value, depth: usize) -> Option<Value> {
    if depth > MAX_NESTING {
        return None;
    }
    let from_plist = |v: &plist::Value| from_plist_at(v, depth + 1);
    Some(match v {
        plist::Value::Boolean(b) => Value::Boolean(*b),
        plist::Value::Integer(i) => Value::Integer(i.as_signed().unwrap_or(i64::MAX)),
        plist::Value::Real(f) => Value::Float(*f),
        plist::Value::String(s) => Value::String(s.clone()),
        plist::Value::Date(d) => Value::String(d.to_xml_format()),
        plist::Value::Array(a) => Value::Array(a.iter().filter_map(from_plist).collect()),
        plist::Value::Dictionary(d) if d.contains_key("Red Component") => {
            let get = |k: &str| d.get(k).and_then(|v| v.as_real().or_else(|| v.as_signed_integer().map(|i| i as f64)));
            return color_table(get, d.get("Color Space").and_then(|v| v.as_string()));
        }
        plist::Value::Dictionary(d) => Value::Table(d.iter().filter_map(|(k, v)| Some((k.clone(), from_plist(v)?))).collect()),
        // Data blobs (background images) and UIDs have no place in a TOML profile.
        _ => return None,
    })
}

pub(crate) fn from_json(v: &serde_json::Value) -> Option<Value> {
    Some(match v {
        serde_json::Value::Bool(b) => Value::Boolean(*b),
        serde_json::Value::Number(n) => n.as_i64().map(Value::Integer).or_else(|| n.as_f64().map(Value::Float))?,
        serde_json::Value::String(s) => Value::String(s.clone()),
        serde_json::Value::Array(a) => Value::Array(a.iter().filter_map(from_json).collect()),
        serde_json::Value::Object(o) if o.contains_key("Red Component") => {
            let get = |k: &str| o.get(k).and_then(|v| v.as_f64());
            return color_table(get, o.get("Color Space").and_then(|v| v.as_str()));
        }
        serde_json::Value::Object(o) => Value::Table(o.iter().filter_map(|(k, v)| Some((k.clone(), from_json(v)?))).collect()),
        serde_json::Value::Null => return None,
    })
}


pub fn from_plist_bytes(bytes: &[u8]) -> Result<Vec<Profile>, String> {
    let root = plist::Value::from_reader(std::io::Cursor::new(bytes)).map_err(|e| format!("not an iTerm2 settings file: {e}"))?;
    let bookmarks = root
        .as_dictionary()
        .and_then(|d| d.get("New Bookmarks"))
        .and_then(|v| v.as_array())
        .ok_or("no New Bookmarks (profiles) in this file")?;
    Ok(unique(bookmarks
        .iter()
        .filter_map(|b| match from_plist(b) {
            Some(Value::Table(t)) => Some(scrubbed(t)),
            _ => None,
        })
        .collect::<Vec<_>>()))
}

/// An imported profile: a start folder naming a network path is emptied (it did not come from
/// the person typing it in Settings; `start_dir::scrub_value`).
fn scrubbed(t: toml::Table) -> Profile {
    let mut values: std::collections::BTreeMap<String, Value> = t.into_iter().collect();
    crate::start_dir::scrub_map(&mut values);
    Profile::from_values(values)
}

/// Imported profiles may lack a Guid or repeat one; give each a unique Guid.
fn unique(mut list: Vec<Profile>) -> Vec<Profile> {
    crate::store::ensure_unique_guids(&mut list);
    list
}

pub fn from_dynamic_json(text: &str) -> Result<Vec<Profile>, String> {
    let root: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("not a Dynamic Profiles file: {e}"))?;
    let list = root.get("Profiles").and_then(|v| v.as_array()).ok_or("no Profiles list in this file")?;
    Ok(unique(list
        .iter()
        .filter_map(|b| match from_json(b) {
            Some(Value::Table(t)) => Some(scrubbed(t)),
            _ => None,
        })
        .collect::<Vec<_>>()))
}

/// The user-defaults key iTerm2 stores an Advanced setting under: its name with the first
/// letter capitalized (`stringByCapitalizingFirstLetter`), "tmuxTitlePrefix" as "TmuxTitlePrefix".
pub fn defaults_key(key: &str) -> String {
    let mut chars = key.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The key with its first letter lower-case: how Termurion spelled an Advanced key before the
/// 13y2 rename sweep, which a settings plist written by that Termurion may still hold.
fn lower_first(key: &str) -> String {
    let mut chars = key.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The global settings (iTerm2 user defaults) in a com.googlecode.iterm2.plist, for the
/// keys Termurion's global schema knows. Other keys are left out. A schema key spelled with a
/// lower-case first letter is also found under iTerm2's capitalized defaults key, and a
/// capitalized schema key under its lower-case spelling (Termurion before the 13y2 rename); the
/// schema spelling wins when a file holds both. The settings that stand between a person and an AI
/// request (`ai_settings::guards_consent`) never come, even from a hand-edited plist: a person
/// opts in again on Windows (Review Focus 3, ruling I5).
pub fn globals_from_file(path: &Path) -> Result<Vec<(String, Value)>, String> {
    let root = plist_file(path)?;
    let dict = root.as_dictionary().ok_or_else(|| format!("{}: not a settings plist", path.display()))?;
    Ok(globals_from_dict(dict))
}

/// The global keys a settings plist may carry for Termurion: the schema's, less the consent
/// switches and the workgroup keys, which never come from a file.
pub fn readable_globals() -> impl Iterator<Item = &'static str> {
    crate::schema::global()
        .iter()
        .filter(|s| !crate::ai_settings::guards_consent(&s.key))
        .filter(|s| !crate::workgroup::is_workgroup_key(&s.key))
        .map(|s| s.key.as_str())
}

/// The spellings a settings plist may hold a global key under, the schema's first.
pub fn spellings(key: &str) -> [String; 3] {
    [key.to_string(), defaults_key(key), lower_first(key)]
}

/// `globals_from_file` for a plist already read.
pub fn globals_from_dict(dict: &plist::Dictionary) -> Vec<(String, Value)> {
    readable_globals()
        .filter_map(|key| {
            let v = spellings(key).iter().find_map(|k| dict.get(k))?;
            Some((key.to_string(), from_plist(v)?))
        })
        .collect()
}

/// The legacy iTerm2 keys an old settings plist may carry (`PREF:145`, `:174-176`, `:216`),
/// read so the import can migrate them as iTerm2 does at launch. They are consumed, not stored.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Legacy {
    /// `Control`: the remap both Control keys fall back to (`PREF:1221-1263`).
    pub control: Option<i64>,
    /// `HotKeyTogglesWindow`: the app hotkey opens a dedicated window.
    pub toggles_window: bool,
    /// `HotKeyBookmark`: that window's profile.
    pub profile_guid: Option<String>,
    /// `HotkeyAutoHides` (absent is true, `PREF:875`).
    pub auto_hides: bool,
    /// `HotkeyMigratedFromSingleToMulti`: the migration already ran.
    pub migrated: bool,
    /// `DockIconTogglesWindow` (the Advanced setting dockIconTogglesWindow, `ASM:471`).
    pub dock_icon_toggles: bool,
}

/// The legacy keys in a com.googlecode.iterm2.plist, with iTerm2's defaults for those absent.
pub fn legacy_from_file(path: &Path) -> Result<Legacy, String> {
    let root = plist_file(path)?;
    let dict = root.as_dictionary().ok_or_else(|| format!("{}: not a settings plist", path.display()))?;
    Ok(legacy_from_dict(dict))
}

/// `legacy_from_file` for a plist already read.
pub fn legacy_from_dict(dict: &plist::Dictionary) -> Legacy {
    let value = |k: &str| dict.get(k).and_then(from_plist);
    let int = |v: &Value| v.as_integer().or_else(|| v.as_float().map(|f| f as i64)).or_else(|| v.as_bool().map(i64::from));
    let bool = |k: &str, absent: bool| value(k).map_or(absent, |v| int(&v).is_some_and(|i| i != 0));
    Legacy {
        control: value("Control").as_ref().and_then(int),
        toggles_window: bool("HotKeyTogglesWindow", false),
        profile_guid: value("HotKeyBookmark").and_then(|v| v.as_str().map(str::to_string)),
        auto_hides: bool("HotkeyAutoHides", true),
        migrated: bool("HotkeyMigratedFromSingleToMulti", false),
        dock_icon_toggles: bool("DockIconTogglesWindow", false),
    }
}

/// iTerm2's legacy right Control tag (`kPreferencesModifierTagLegacyRightControl`), which acts
/// as right Control (`iTermModifierRemapper.m:56-58`, `:122-123`).
const LEGACY_RIGHT_CONTROL: i64 = 1;
/// The Control remap choice "Right Control" (`schema/global.toml` LeftControl, RightControl).
const RIGHT_CONTROL: i64 = 12;

/// Applies the legacy keys to an import's global values and profiles, as iTerm2 does:
/// `Control` fills each Control remap the file does not set (`PREF:1221-1263`), and, once
/// (`HKM:34-73`), a single hotkey window (`Hotkey` and `HotKeyTogglesWindow`) moves to the
/// profile `HotKeyBookmark` names (`HKM:252-278`) and the app hotkey is cleared; the migrated
/// flag is set either way. A dynamic hotkey profile and the children warning are not migrated
/// (Termurion's dynamic profiles import once).
pub fn migrate_legacy(values: &mut Vec<(String, Value)>, legacy: &Legacy, profiles: &mut [Profile]) {
    fn get<'a>(values: &'a [(String, Value)], key: &str) -> Option<&'a Value> {
        values.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    fn put(values: &mut Vec<(String, Value)>, key: &str, v: Value) {
        values.retain(|(k, _)| k != key);
        values.push((key.to_string(), v));
    }
    let number = |values: &[(String, Value)], key: &str| get(values, key).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)));
    if let Some(control) = legacy.control {
        let remap = if control == LEGACY_RIGHT_CONTROL { RIGHT_CONTROL } else { control };
        for key in ["LeftControl", "RightControl"] {
            if get(values, key).is_none() {
                put(values, key, Value::Integer(remap));
            }
        }
    }
    if legacy.migrated {
        return;
    }
    let enabled = get(values, "Hotkey").and_then(Value::as_bool).unwrap_or(false);
    if enabled && legacy.toggles_window {
        let code = number(values, "HotkeyCode").unwrap_or(0.0) as i64;
        // A unichar: the character's low 16 bits; 0 (or a lone surrogate) is no character.
        let character = number(values, "HotkeyChar").unwrap_or(0.0) as i64 as u16;
        let characters = if character == 0 { String::new() } else { char::from_u32(u32::from(character)).map(String::from).unwrap_or_default() };
        let modifiers = number(values, "HotkeyModifiers").unwrap_or(0.0) as i64;
        let duration = number(values, "HotkeyTermAnimationDuration")
            .or_else(|| crate::schema::find_global("HotkeyTermAnimationDuration").and_then(|s| s.default.as_ref()).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))))
            .unwrap_or(0.0);
        // iTermHotKeyDockPreferenceShowIfNoOtherWindowsOpen, else DoNotShow.
        let dock = if legacy.dock_icon_toggles { 2 } else { 0 };
        if let Some(p) = legacy.profile_guid.as_deref().and_then(|g| profiles.iter_mut().find(|p| p.guid() == g)) {
            p.set("Has Hotkey", Value::Boolean(true));
            p.set("HotKey Key Code", Value::Integer(code));
            p.set("HotKey Characters", Value::String(characters.clone()));
            p.set("HotKey Characters Ignoring Modifiers", Value::String(characters));
            p.set("HotKey Modifier Flags", Value::Integer(modifiers));
            p.set("HotKey Window AutoHides", Value::Boolean(legacy.auto_hides));
            p.set("HotKey Window Reopens On Activation", Value::Boolean(false));
            p.set("HotKey Window Animates", Value::Boolean(duration > 0.0));
            p.set("HotKey Window Floats", Value::Boolean(false));
            p.set("HotKey Window Dock Click Action", Value::Integer(dock));
        }
        // "Erase the legacy prefs so the keystroke doesn't show up as the global app toggle."
        put(values, "Hotkey", Value::Boolean(false));
        put(values, "HotkeyCode", Value::Integer(0));
        put(values, "HotkeyChar", Value::Integer(0));
    }
    put(values, "HotkeyMigratedFromSingleToMulti", Value::Boolean(true));
}

/// A settings file's bytes: the path comes from the person or an imported folder, so it is judged
/// before it is read.
fn read_judged(path: &Path) -> Result<Vec<u8>, String> {
    crate::localfs::judge_io(path).and_then(|p| crate::localfs::read(&p)).map_err(|e| format!("{}: {e}", path.display()))
}

/// A plist file, judged before it is read.
fn plist_file(path: &Path) -> Result<plist::Value, String> {
    plist::Value::from_reader(std::io::Cursor::new(read_judged(path)?)).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn from_file(path: &Path) -> Result<Vec<Profile>, String> {
    let bytes = read_judged(path)?;
    match path.extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref() {
        Some("json") => from_dynamic_json(&String::from_utf8_lossy(&bytes)),
        _ => from_plist_bytes(&bytes),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn srgb_to_p3_follows_p3_to_srgb() {
        // sRGB red is less saturated in P3; gray stays gray; the two conversions invert each other.
        let (r, g, b) = srgb_to_p3(1.0, 0.0, 0.0);
        assert!((r - 0.9175).abs() < 2e-3 && (g - 0.2003).abs() < 2e-3 && (b - 0.1387).abs() < 2e-3, "{r} {g} {b}");
        let (r, g, b) = srgb_to_p3(0.5, 0.5, 0.5);
        assert!((r - 0.5).abs() < 1e-4 && (g - 0.5).abs() < 1e-4 && (b - 0.5).abs() < 1e-4);
        for c in [(0.2, 0.7, 0.4), (1.0, 1.0, 0.0), (0.03, 0.9, 0.99)] {
            let (r, g, b) = srgb_to_p3(c.0, c.1, c.2);
            let back = p3_to_srgb(r, g, b);
            assert!((back.0 - c.0).abs() < 1e-3 && (back.1 - c.1).abs() < 1e-3 && (back.2 - c.2).abs() < 1e-3, "{c:?} {back:?}");
        }
    }

    use super::*;
    use crate::palette::rgb;

    /// 13y2 Task 9b: an Advanced setting imports under its iTerm2 spelling or a lower-case-first one.
    #[test]
    fn plist_keys_match_either_spelling() {
        for (name, spelling) in [("cap", "DisableTopRightIndicators"), ("low", "disableTopRightIndicators")] { // old key on purpose
            let path = std::env::temp_dir().join(format!("termurion-import-spell-{name}-{}.plist", std::process::id()));
            std::fs::write(&path, format!(r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>{spelling}</key><true/></dict></plist>"#)).unwrap();
            let got = globals_from_file(&path).unwrap();
            let _ = std::fs::remove_file(&path);
            assert!(got.iter().any(|(k, v)| k == "DisableTopRightIndicators" && v.as_bool() == Some(true)), "{spelling}: {got:?}");
        }
    }

    #[test]
    fn imports_global_keys_only() {
        let path = std::env::temp_dir().join(format!("termurion-import-globals-{}.plist", std::process::id()));
        std::fs::write(&path, r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
  <key>GlobalKeyMap</key><dict><key>0x61-0x100000</key><dict><key>Action</key><integer>13</integer><key>Text</key><string></string></dict></dict>
  <key>LeftCommand</key><integer>11</integer>
  <key>NSWindow Frame</key><string>0 0 10 10</string>
</dict></plist>"#).unwrap();
        let mut got = globals_from_file(&path).unwrap();
        got.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(got.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), ["GlobalKeyMap", "LeftCommand"]);
        assert_eq!(got[1].1.as_integer(), Some(11));
        assert_eq!(got[0].1["0x61-0x100000"]["Action"].as_integer(), Some(13));
    }

    /// `PREF:1221-1263` (Left and Right Control fall back to `Control`; tag 1, the legacy right
    /// Control, acts as right Control, `iTermModifierRemapper.m:56-58`, `:122-123`) and
    /// `HKM:34-73`, `:252-278` (the single hotkey window moves to its profile once).
    #[test]
    fn legacy_keys_follow_iterm2() {
        let int = |i: i64| Value::Integer(i);
        let get = |values: &[(String, Value)], k: &str| values.iter().rev().find(|(x, _)| x == k).map(|(_, v)| v.clone());
        // Control fills both remaps only when they are absent.
        let mut values = vec![];
        migrate_legacy(&mut values, &Legacy { control: Some(1), ..Default::default() }, &mut []);
        assert_eq!(get(&values, "LeftControl"), Some(int(12)));
        assert_eq!(get(&values, "RightControl"), Some(int(12)));
        let mut values = vec![];
        migrate_legacy(&mut values, &Legacy { control: Some(2), ..Default::default() }, &mut []);
        assert_eq!(get(&values, "LeftControl"), Some(int(2)));
        assert_eq!(get(&values, "RightControl"), Some(int(2)));
        let mut values = vec![("LeftControl".to_string(), int(11))];
        migrate_legacy(&mut values, &Legacy { control: Some(1), ..Default::default() }, &mut []);
        assert_eq!(get(&values, "LeftControl"), Some(int(11)));
        assert_eq!(get(&values, "RightControl"), Some(int(12)));
        assert_eq!(values.iter().filter(|(k, _)| k == "LeftControl").count(), 1);
        let mut values = vec![];
        migrate_legacy(&mut values, &Legacy { migrated: true, ..Default::default() }, &mut []);
        assert!(get(&values, "LeftControl").is_none() && get(&values, "RightControl").is_none());

        // The single hotkey window moves to its profile.
        let hotkey = || vec![("Hotkey".to_string(), Value::Boolean(true)), ("HotkeyCode".to_string(), int(50)), ("HotkeyChar".to_string(), int(96)), ("HotkeyModifiers".to_string(), int(1048576))];
        let legacy = Legacy { toggles_window: true, profile_guid: Some("G".into()), auto_hides: false, migrated: false, dock_icon_toggles: true, ..Default::default() };
        let profile = |guid: &str| {
            let mut p = Profile::default();
            p.set("Guid", Value::String(guid.into()));
            p
        };
        let mut values = hotkey();
        let mut profiles = [profile("G"), profile("H")];
        migrate_legacy(&mut values, &legacy, &mut profiles);
        let g = &profiles[0];
        assert_eq!(g.values.get("Has Hotkey"), Some(&Value::Boolean(true)));
        assert_eq!(g.values.get("HotKey Key Code"), Some(&int(50)));
        assert_eq!(g.values.get("HotKey Characters"), Some(&Value::String("`".into())));
        assert_eq!(g.values.get("HotKey Characters Ignoring Modifiers"), Some(&Value::String("`".into())));
        assert_eq!(g.values.get("HotKey Modifier Flags"), Some(&int(1048576)));
        assert_eq!(g.values.get("HotKey Window AutoHides"), Some(&Value::Boolean(false)));
        assert_eq!(g.values.get("HotKey Window Reopens On Activation"), Some(&Value::Boolean(false)));
        assert_eq!(g.values.get("HotKey Window Animates"), Some(&Value::Boolean(true)), "the default 0.25 is above 0");
        assert_eq!(g.values.get("HotKey Window Floats"), Some(&Value::Boolean(false)));
        assert_eq!(g.values.get("HotKey Window Dock Click Action"), Some(&int(2)));
        assert!(!profiles[1].values.contains_key("Has Hotkey"), "only the named profile");
        assert_eq!(get(&values, "Hotkey"), Some(Value::Boolean(false)));
        assert_eq!(get(&values, "HotkeyCode"), Some(int(0)));
        assert_eq!(get(&values, "HotkeyChar"), Some(int(0)));
        assert_eq!(get(&values, "HotkeyMigratedFromSingleToMulti"), Some(Value::Boolean(true)));
        let mut values = hotkey();
        values.push(("HotkeyTermAnimationDuration".to_string(), Value::Float(0.0)));
        let mut profiles = [profile("G")];
        migrate_legacy(&mut values, &Legacy { dock_icon_toggles: false, ..legacy.clone() }, &mut profiles);
        assert_eq!(profiles[0].values.get("HotKey Window Animates"), Some(&Value::Boolean(false)));
        assert_eq!(profiles[0].values.get("HotKey Window Dock Click Action"), Some(&int(0)));
        // A character of 0 is an empty text.
        let mut values = hotkey();
        values.retain(|(k, _)| k != "HotkeyChar");
        let mut profiles = [profile("G")];
        migrate_legacy(&mut values, &legacy, &mut profiles);
        assert_eq!(profiles[0].values.get("HotKey Characters"), Some(&Value::String(String::new())));

        // A guid naming no imported profile still clears the app hotkey and sets the flag.
        let mut values = hotkey();
        let mut profiles = [profile("H")];
        migrate_legacy(&mut values, &legacy, &mut profiles);
        assert!(!profiles[0].values.contains_key("Has Hotkey"));
        assert_eq!(get(&values, "Hotkey"), Some(Value::Boolean(false)));
        assert_eq!(get(&values, "HotkeyCode"), Some(int(0)));
        assert_eq!(get(&values, "HotkeyChar"), Some(int(0)));
        assert_eq!(get(&values, "HotkeyMigratedFromSingleToMulti"), Some(Value::Boolean(true)));
        // Already migrated: nothing but the Control remaps.
        let mut values = hotkey();
        let mut profiles = [profile("G")];
        migrate_legacy(&mut values, &Legacy { migrated: true, control: Some(2), ..legacy.clone() }, &mut profiles);
        let mut want = hotkey();
        want.push(("LeftControl".to_string(), int(2)));
        want.push(("RightControl".to_string(), int(2)));
        assert_eq!(values, want);
        assert!(!profiles[0].values.contains_key("Has Hotkey"));
        // Not a dedicated window: no profile changes and the app hotkey stays; the flag is set.
        let mut values = hotkey();
        let mut profiles = [profile("G")];
        migrate_legacy(&mut values, &Legacy { toggles_window: false, ..legacy.clone() }, &mut profiles);
        assert!(!profiles[0].values.contains_key("Has Hotkey"));
        assert_eq!(get(&values, "Hotkey"), Some(Value::Boolean(true)));
        assert_eq!(get(&values, "HotkeyCode"), Some(int(50)));
        assert_eq!(get(&values, "HotkeyMigratedFromSingleToMulti"), Some(Value::Boolean(true)));
        // The app hotkey off: nothing moves; the flag is set.
        let mut values = vec![("Hotkey".to_string(), Value::Boolean(false))];
        let mut profiles = [profile("G")];
        migrate_legacy(&mut values, &legacy, &mut profiles);
        assert!(!profiles[0].values.contains_key("Has Hotkey"));
        assert_eq!(get(&values, "HotkeyMigratedFromSingleToMulti"), Some(Value::Boolean(true)));

        // The flag's row (`PREF:216`, default NO `PREF:914`).
        let row = crate::schema::find_global("HotkeyMigratedFromSingleToMulti").expect("HotkeyMigratedFromSingleToMulti");
        assert_eq!((row.tab.as_str(), row.section.as_str(), row.kind), ("Stored", "Hotkey", crate::schema::Kind::Bool));
        assert_eq!(row.default, Some(Value::Boolean(false)));
    }

    /// `PREF:145`, `:174-176`, `:216`, defaults `PREF:848`, `:873-875`, `:914`.
    #[test]
    fn legacy_keys_read_from_the_plist() {
        let path = std::env::temp_dir().join(format!("termurion-import-legacy-{}.plist", std::process::id()));
        std::fs::write(&path, r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
  <key>Control</key><integer>2</integer>
  <key>HotKeyTogglesWindow</key><true/>
  <key>HotKeyBookmark</key><string>G</string>
  <key>DockIconTogglesWindow</key><true/>
</dict></plist>"#).unwrap();
        let got = legacy_from_file(&path).unwrap();
        assert_eq!(got, Legacy { control: Some(2), toggles_window: true, profile_guid: Some("G".into()), auto_hides: true, migrated: false, dock_icon_toggles: true });
        std::fs::write(&path, r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
  <key>HotkeyAutoHides</key><false/>
  <key>HotkeyMigratedFromSingleToMulti</key><true/>
</dict></plist>"#).unwrap();
        let got = legacy_from_file(&path).unwrap();
        assert_eq!(got, Legacy { auto_hides: false, migrated: true, ..Default::default() });
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn import_matches_iterm2_defaults_keys() {
        assert_eq!(defaults_key("TmuxTitlePrefix"), "TmuxTitlePrefix");
        assert_eq!(defaults_key("NoSyncDisableOpenURL"), "NoSyncDisableOpenURL");
        assert_eq!(defaults_key(""), "");
        let dir = std::env::temp_dir();
        let path = dir.join(format!("termurion-import-defaults-keys-{}.plist", std::process::id()));
        std::fs::write(&path, r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
  <key>TmuxTitlePrefix</key><string>x </string>
  <key>NoSyncDisableOpenURL</key><true/>
</dict></plist>"#).unwrap();
        let got = globals_from_file(&path).unwrap();
        assert!(got.contains(&("TmuxTitlePrefix".to_string(), Value::String("x ".into()))), "{got:?}");
        assert!(got.contains(&("NoSyncDisableOpenURL".to_string(), Value::Boolean(true))), "{got:?}");
        // Both spellings present: iTerm2's (the schema key) wins.
        let both = dir.join(format!("termurion-import-defaults-keys-both-{}.plist", std::process::id()));
        std::fs::write(&both, r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
  <key>tmuxTitlePrefix</key><string>lower</string>
  <key>TmuxTitlePrefix</key><string>exact</string>
</dict></plist>"#).unwrap();
        let got = globals_from_file(&both).unwrap();
        assert_eq!(got, [("TmuxTitlePrefix".to_string(), Value::String("exact".into()))]);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&both);
    }

    /// 13y1 trust ruling: a plist never brings the workgroups, the Imported marks or the latches
    /// (iTerm2 stores the workgroups as binary data, which the import drops; a crafted string
    /// value must not get past it either). The task that converts them sets the mark.
    #[test]
    fn a_plist_never_brings_workgroup_keys() {
        let path = std::env::temp_dir().join(format!("termurion-import-wg-{}.plist", std::process::id()));
        std::fs::write(&path, r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
  <key>Workgroups</key><string>[]</string>
  <key>WorkgroupsImported</key><string>[]</string>
  <key>NoSyncWorkgroupShortcutsBackfilled</key><true/>
  <key>TmuxTitlePrefix</key><string>x </string>
</dict></plist>"#).unwrap();
        let got = globals_from_file(&path).unwrap();
        assert_eq!(got.len(), 1, "{got:?}");
        let _ = std::fs::remove_file(&path);
    }

    /// Controller ruling I5: Import from iTerm2, with 13a's capitalized defaults keys and its
    /// legacy key migration, brings none of the AI consent switches (`ai_settings::guards_consent`).
    #[test]
    fn legacy_import_never_brings_the_ai_consent_switches() {
        use crate::ai_settings as ai;
        let path = std::env::temp_dir().join(format!("termurion-import-ai-legacy-{}.plist", std::process::id()));
        let mut body = String::from("<key>Control</key><integer>2</integer><key>FocusFollowsMouse</key><true/>");
        for key in [ai::ADMIN, ai::ENABLE_AI, ai::AI_COMPLETIONS] {
            body += &format!("<key>{}</key><{}/>", defaults_key(key), if key == ai::ADMIN { "false" } else { "true" });
        }
        for (key, _) in ai::PERMISSIONS {
            body += &format!("<key>{}</key><integer>1</integer>", defaults_key(key));
        }
        std::fs::write(&path, format!(r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>{body}</dict></plist>"#)).unwrap();
        let mut values = globals_from_file(&path).unwrap();
        migrate_legacy(&mut values, &legacy_from_file(&path).unwrap(), &mut []);
        let _ = std::fs::remove_file(&path);
        let keys: Vec<&str> = values.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"FocusFollowsMouse") && keys.contains(&"LeftControl"), "{keys:?}");
        for key in keys {
            assert!(!ai::guards_consent(key), "{key} was imported");
        }
    }

    #[test]
    fn ai_consent_is_never_imported() {
        let path = std::env::temp_dir().join(format!("termurion-import-ai-{}.plist", std::process::id()));
        std::fs::write(&path, r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
  <key>EnableAI</key><true/>
  <key>AICompletions</key><true/>
  <key>NoSyncAIRequestConsent</key><array><string>api.openai.com</string></array>
  <key>Enable AI</key><true/>
  <key>AIVendor</key><integer>4</integer>
  <key>AIManualModelConfigurations</key><array><dict>
    <key>name</key><string>m</string>
    <key>url</key><string>https://my.host/v1/messages</string>
    <key>api</key><integer>7</integer>
    <key>streaming</key><true/>
  </dict></array>
</dict></plist>"#).unwrap();
        let mut got = globals_from_file(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        got.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(got.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), ["AIManualModelConfigurations", "AIVendor"]);
        let config = &got[0].1[0];
        assert_eq!(config["api"].as_integer(), Some(7));
        assert_eq!(config["streaming"].as_bool(), Some(true));
        for key in ["EnableAI", "AICompletions", "NoSyncAIRequestConsent", "NoSyncAutoProvideConsent"] {
            assert!(crate::ai_settings::guards_consent(key), "{key}");
        }
    }

    const PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>New Bookmarks</key>
  <array>
    <dict>
      <key>Name</key><string>Work</string>
      <key>Guid</key><string>ABC-123</string>
      <key>Columns</key><integer>132</integer>
      <key>Use Bright Bold</key><false/>
      <key>Normal Font</key><string>MesloLGS-NF-Regular 13</string>
      <key>Some Future Key</key><string>kept</string>
      <key>Background Image Data</key><data>AAEC</data>
      <key>Ansi 1 Color</key>
      <dict>
        <key>Color Space</key><string>sRGB</string>
        <key>Red Component</key><real>1</real>
        <key>Green Component</key><real>0.5</real>
        <key>Blue Component</key><real>0</real>
        <key>Alpha Component</key><real>1</real>
      </dict>
      <key>Ansi 2 Color</key>
      <dict>
        <key>Color Space</key><string>P3</string>
        <key>Red Component</key><real>0.5</real>
        <key>Green Component</key><real>0.5</real>
        <key>Blue Component</key><real>0.5</real>
      </dict>
    </dict>
  </array>
</dict>
</plist>"#;

    fn work() -> Profile {
        from_plist_bytes(PLIST.as_bytes()).unwrap().remove(0)
    }

    #[test]
    fn imports_bookmarks() {
        let p = work();
        assert_eq!((p.name().as_str(), p.guid().as_str()), ("Work", "ABC-123"));
        assert_eq!(p.int("Columns"), 132);
        assert!(!p.bool("Use Bright Bold"));
        assert_eq!(p.color("Ansi 1 Color"), rgb(255, 128, 0));
    }

    #[test]
    fn postscript_font_names_become_family_names() {
        // Import keeps iTerm2's exact name; the renderer resolves it against installed fonts.
        assert_eq!(work().font("Normal Font").family, "MesloLGS-NF-Regular");
        assert_eq!(family_from_postscript("JetBrainsMono-BoldItalic"), "JetBrainsMono");
        assert_eq!(family_from_postscript("Monaco"), "Monaco");
        assert_eq!(family_from_postscript("FiraCode-Retina"), "FiraCode Retina");
        assert_eq!(family_from_postscript("JetBrainsMono-Bold"), "JetBrainsMono");
    }

    #[test]
    fn unknown_keys_are_kept_and_data_is_skipped() {
        let p = work();
        assert_eq!(p.string("Some Future Key"), "kept");
        assert!(!p.values.contains_key("Background Image Data"));
    }

    #[test]
    fn p3_colors_are_converted() {
        // A neutral gray is the same in P3 and sRGB.
        assert_eq!(work().color("Ansi 2 Color"), rgb(128, 128, 128));
        let pure = p3_to_srgb(0.0, 1.0, 0.0);
        assert!(pure.0 == 0.0 && pure.1 == 1.0 && pure.2 == 0.0, "{pure:?}");
    }

    #[test]
    fn imported_plain_colors_reach_the_palette() {
        // Profiles from before iTerm2 3.5 have no Light/Dark keys. iTerm2 then reads the
        // plain colors, so the palette must too.
        let json = r#"{"Profiles":[{"Name":"Sol","Guid":"S-1","Foreground Color":{"Red Component":0.5,"Green Component":0.6,"Blue Component":0.7}}]}"#;
        let p = from_dynamic_json(json).unwrap().remove(0);
        let pal = crate::palette::Palette::from_profile(&p, true);
        assert_eq!(pal.fg, rgb(128, 153, 179));
    }

    #[test]
    fn imported_profiles_without_a_guid_get_one() {
        let json = r#"{"Profiles":[{"Name":"NoId"},{"Name":"AlsoNoId"}]}"#;
        let list = from_dynamic_json(json).unwrap();
        assert!(list.iter().all(|p| !p.guid().is_empty()));
        assert_ne!(list[0].guid(), list[1].guid());
    }

    #[test]
    fn imports_dynamic_profiles_json() {
        let json = r#"{"Profiles":[{"Name":"Dyn","Guid":"D-1","Rows":40,"Foreground Color":{"Red Component":0,"Green Component":1,"Blue Component":0}}]}"#;
        let p = from_dynamic_json(json).unwrap().remove(0);
        assert_eq!((p.name().as_str(), p.int("Rows")), ("Dyn", 40));
        assert_eq!(p.color("Foreground Color"), rgb(0, 255, 0));
    }

    #[test]
    fn a_plist_without_profiles_is_an_error() {
        let empty = r#"<?xml version="1.0"?><plist version="1.0"><dict></dict></plist>"#;
        assert!(from_plist_bytes(empty.as_bytes()).unwrap_err().contains("New Bookmarks"));
    }

    /// An imported profile never carries a network start folder (it was not typed in Settings).
    #[test]
    fn imported_profiles_lose_network_folders() {
        let json = r#"{"Profiles":[{"Name":"X","Guid":"g","Custom Directory":"Yes","Working Directory":" \\\\evil\\share","AWDS Tab Directory":"//evil/s","AWDS Pane Directory":"D:\\ok"}]}"#;
        let got = from_dynamic_json(json).unwrap();
        let v = |k: &str| got[0].string(k);
        assert_eq!((v("Working Directory"), v("AWDS Tab Directory"), v("AWDS Pane Directory")), (String::new(), String::new(), "D:\\ok".to_string()));
        let plist = r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>New Bookmarks</key><array><dict>
<key>Name</key><string>P</string><key>Guid</key><string>g2</string>
<key>Working Directory</key><string>	\\evil\share</string><key>AWDS Window Directory</key><string>"//evil/s"</string></dict></array></dict></plist>"#;
        let got = from_plist_bytes(plist.as_bytes()).unwrap();
        assert_eq!((got[0].string("Working Directory"), got[0].string("AWDS Window Directory")), (String::new(), String::new()));
    }

    /// Task 4 ruling: the same for every path key, in both import formats.
    #[test]
    fn imported_profiles_lose_every_network_path() {
        let json = r#"{"Profiles":[{"Name":"X","Guid":"g","Log Directory":"\\\\evil\\logs","Archive Directory":" //evil/a","Custom Icon Path":"\\\\evil\\i.png","Background Image Location":"\\\\evil\\b.png","Background Image Folder Location":"//evil/pics","SSH":{"pathToSSH":"\\\\evil\\ssh.exe","filesToCopy":[["\\\\evil\\k","~"],["D:\\ok","~"]]}}]}"#;
        let got = from_dynamic_json(json).unwrap();
        for k in ["Log Directory", "Archive Directory", "Custom Icon Path", "Background Image Location", "Background Image Folder Location"] {
            assert_eq!(got[0].string(k), "", "{k}");
        }
        let ssh = crate::ssh_config::from_profile(&got[0]);
        assert_eq!((ssh.path_to_ssh, ssh.files_to_copy), (None, vec![("D:\\ok".to_string(), "~".to_string())]));
        let plist = r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>New Bookmarks</key><array><dict>
<key>Name</key><string>P</string><key>Guid</key><string>g2</string>
<key>Log Directory</key><string>\\evil\logs</string><key>Custom Icon Path</key><string>D:\i.png</string></dict></array></dict></plist>"#;
        let got = from_plist_bytes(plist.as_bytes()).unwrap();
        assert_eq!((got[0].string("Log Directory"), got[0].string("Custom Icon Path")), (String::new(), "D:\\i.png".to_string()));
    }

    /// An imported profile never carries the dynamic marks: only the Dynamic Profiles loader sets
    /// them (an iTerm2 export holds them for its dynamic profiles).
    #[test]
    fn imports_strip_the_dynamic_marks() {
        let marks = |p: &Profile| ["Is Dynamic Profile", "Rewritable", "Dynamic Profile Parent Name", "Dynamic Profile Parent GUID", "Dynamic Profile Filename"].iter().any(|k| p.values.contains_key(*k));
        let json = r#"{"Profiles": [{"Guid": "G", "Name": "d", "Is Dynamic Profile": true, "Rewritable": true, "Dynamic Profile Parent Name": "P", "Dynamic Profile Parent GUID": "Q", "Dynamic Profile Filename": "x.json", "Columns": 90}]}"#;
        let got = from_dynamic_json(json).unwrap();
        assert!(!marks(&got[0]), "{:?}", got[0].values);
        assert_eq!(got[0].int("Columns"), 90);
        let xml = r#"<?xml version="1.0"?><plist version="1.0"><dict><key>New Bookmarks</key><array><dict><key>Guid</key><string>G</string><key>Name</key><string>d</string><key>Is Dynamic Profile</key><true/><key>Rewritable</key><true/></dict></array></dict></plist>"#;
        let got = from_plist_bytes(xml.as_bytes()).unwrap();
        assert!(!marks(&got[0]), "{:?}", got[0].values);
    }
}

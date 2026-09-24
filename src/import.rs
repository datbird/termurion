//! Imports iTerm2 profiles: the full settings plist (com.googlecode.iterm2.plist)
//! or a Dynamic Profiles JSON file. This is a one-time copy, not a sync.

use std::path::Path;

use toml::Value;

use crate::profile::Profile;

/// Weight and style words iTerm2's PostScript names carry after the family.
const STYLE_WORDS: [&str; 12] =
    ["Regular", "Book", "Medium", "Bold", "Italic", "Light", "Thin", "SemiBold", "ExtraBold", "Black", "Oblique", "Normal"];

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

fn color_table(get: impl Fn(&str) -> Option<f64>, space: Option<&str>) -> Option<Value> {
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

fn from_plist(v: &plist::Value) -> Option<Value> {
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

fn from_json(v: &serde_json::Value) -> Option<Value> {
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

/// Fonts arrive as PostScript names. Store them as family names Windows can find.
fn fix_fonts(mut p: Profile) -> Profile {
    for key in ["Normal Font", "Non Ascii Font"] {
        if let Some(Value::String(s)) = p.values.get(key).cloned() {
            let spec = crate::profile::parse_font(&s);
            p.set(key, Value::String(format!("{} {}", family_from_postscript(&spec.family), spec.size_pt)));
        }
    }
    p
}

pub fn from_plist_bytes(bytes: &[u8]) -> Result<Vec<Profile>, String> {
    let root = plist::Value::from_reader(std::io::Cursor::new(bytes)).map_err(|e| format!("not an iTerm2 settings file: {e}"))?;
    let bookmarks = root
        .as_dictionary()
        .and_then(|d| d.get("New Bookmarks"))
        .and_then(|v| v.as_array())
        .ok_or("no New Bookmarks (profiles) in this file")?;
    Ok(bookmarks
        .iter()
        .filter_map(|b| match from_plist(b) {
            Some(Value::Table(t)) => Some(fix_fonts(Profile { values: t.into_iter().collect() })),
            _ => None,
        })
        .collect::<Vec<_>>())
    .map(unique)
}

/// Imported profiles may lack a Guid or repeat one; give each a unique Guid.
fn unique(mut list: Vec<Profile>) -> Vec<Profile> {
    crate::store::ensure_unique_guids(&mut list);
    list
}

pub fn from_dynamic_json(text: &str) -> Result<Vec<Profile>, String> {
    let root: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("not a Dynamic Profiles file: {e}"))?;
    let list = root.get("Profiles").and_then(|v| v.as_array()).ok_or("no Profiles list in this file")?;
    Ok(list
        .iter()
        .filter_map(|b| match from_json(b) {
            Some(Value::Table(t)) => Some(fix_fonts(Profile { values: t.into_iter().collect() })),
            _ => None,
        })
        .collect::<Vec<_>>())
    .map(unique)
}

pub fn from_file(path: &Path) -> Result<Vec<Profile>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    match path.extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref() {
        Some("json") => from_dynamic_json(&String::from_utf8_lossy(&bytes)),
        _ => from_plist_bytes(&bytes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::rgb;

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
        assert_eq!(work().font("Normal Font").family, "MesloLGS NF");
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
}

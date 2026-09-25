//! A profile: iTerm2 keys mapped to values. A key with no value reads the schema default.

use std::collections::BTreeMap;

use toml::Value;

use crate::palette::{Rgb, rgb};
use crate::schema;

pub const GUID: &str = "Guid";
pub const NAME: &str = "Name";
/// The Windows stand-in for iTerm2's default Monaco 12.
pub const WINDOWS_FONT: &str = "Cascadia Mono 12";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Profile {
    pub values: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FontSpec {
    pub family: String,
    pub size_pt: f32,
}

/// Parses iTerm2's font form, "<family> <size>". A missing size reads as 12.
pub fn parse_font(s: &str) -> FontSpec {
    let s = s.trim();
    match s.rsplit_once(' ') {
        Some((family, size)) if size.parse::<f32>().is_ok() => {
            FontSpec { family: family.trim().to_string(), size_pt: size.parse().unwrap() }
        }
        _ => FontSpec { family: s.to_string(), size_pt: 12.0 },
    }
}

pub fn color_value(c: Rgb) -> Value {
    let mut t = toml::Table::new();
    t.insert("r".into(), Value::Float(c.r as f64 / 255.0));
    t.insert("g".into(), Value::Float(c.g as f64 / 255.0));
    t.insert("b".into(), Value::Float(c.b as f64 / 255.0));
    t.insert("a".into(), Value::Float(1.0));
    Value::Table(t)
}

fn to_rgb(v: &Value) -> Option<Rgb> {
    let t = v.as_table()?;
    let c = |k: &str| t.get(k).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)));
    let byte = |f: f64| (f.clamp(0.0, 1.0) * 255.0).round() as u8;
    Some(rgb(byte(c("r")?), byte(c("g")?), byte(c("b")?)))
}

impl Profile {
    /// The values iTerm2 gives a new profile (its DefaultBookmark), with no Guid or name.
    /// `Profile::default()` is an empty profile: every key reads iTerm2's missing-key value,
    /// which is how an imported profile's absent keys behave.
    pub fn stock() -> Profile {
        let mut p = Profile::default();
        for s in schema::profile() {
            if let Some(v) = &s.seed {
                p.set(&s.key, v.clone());
            }
            if let Some(v) = &s.seed_light {
                p.set(&format!("{} (Light)", s.key), v.clone());
            }
            if let Some(v) = &s.seed_dark {
                p.set(&format!("{} (Dark)", s.key), v.clone());
            }
        }
        p
    }

    pub fn new(name: &str) -> Profile {
        let mut p = Profile::stock();
        p.set(GUID, Value::String(uuid::Uuid::new_v4().to_string().to_uppercase()));
        p.set(NAME, Value::String(name.to_string()));
        p.set("Normal Font", Value::String(WINDOWS_FONT.into()));
        p.set("Non Ascii Font", Value::String(WINDOWS_FONT.into()));
        p
    }

    /// The profile's own value, or the schema default.
    pub fn get(&self, key: &str) -> Option<Value> {
        self.values.get(key).cloned().or_else(|| schema::find(key).and_then(|s| s.default.clone()))
    }

    pub fn set(&mut self, key: &str, value: Value) {
        self.values.insert(key.to_string(), value);
    }

    pub fn bool(&self, key: &str) -> bool {
        self.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
    }

    pub fn int(&self, key: &str) -> i64 {
        self.get(key).and_then(|v| v.as_integer().or_else(|| v.as_float().map(|f| f as i64))).unwrap_or(0)
    }

    pub fn float(&self, key: &str) -> f64 {
        self.get(key).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).unwrap_or(0.0)
    }

    pub fn string(&self, key: &str) -> String {
        self.get(key).and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
    }

    pub fn color(&self, key: &str) -> Rgb {
        self.get(key).as_ref().and_then(to_rgb).unwrap_or(rgb(0, 0, 0))
    }

    /// A color that may have Light and Dark variants ("<key> (Dark)").
    pub fn color_for(&self, key: &str, dark: bool) -> Rgb {
        if self.bool("Use Separate Colors for Light and Dark Mode") {
            let variant = format!("{key} ({})", if dark { "Dark" } else { "Light" });
            let stock = schema::find(key).and_then(|s| if dark { s.default_dark.as_ref() } else { s.default_light.as_ref() });
            if let Some(c) = self.values.get(&variant).or(stock).and_then(to_rgb) {
                return c;
            }
        }
        self.color(key)
    }

    /// A color's alpha (the badge color keeps one), with Light and Dark variants like
    /// `color_for`; 1 when the color has none.
    pub fn alpha_for(&self, key: &str, dark: bool) -> f32 {
        let a = |v: &Value| v.as_table().and_then(|t| t.get("a").or_else(|| t.get("Alpha Component"))).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)));
        if self.bool("Use Separate Colors for Light and Dark Mode") {
            let variant = format!("{key} ({})", if dark { "Dark" } else { "Light" });
            let stock = schema::find(key).and_then(|s| if dark { s.default_dark.as_ref() } else { s.default_light.as_ref() });
            if let Some(v) = self.values.get(&variant).or(stock).and_then(a) {
                return v as f32;
            }
        }
        self.get(key).as_ref().and_then(a).map_or(1.0, |v| v as f32)
    }

    /// A font, with its size kept between 4 and 128 points.
    pub fn font(&self, key: &str) -> FontSpec {
        let mut f = parse_font(&self.string(key));
        f.size_pt = f.size_pt.clamp(4.0, 128.0);
        f
    }

    /// A bool that may have Light and Dark variants, read like `color_for`.
    pub fn bool_for(&self, key: &str, dark: bool) -> bool {
        if self.bool("Use Separate Colors for Light and Dark Mode") {
            let variant = format!("{key} ({})", if dark { "Dark" } else { "Light" });
            let stock = schema::find(key).and_then(|s| if dark { s.default_dark.as_ref() } else { s.default_light.as_ref() });
            if let Some(b) = self.values.get(&variant).or(stock).and_then(|v| v.as_bool()) {
                return b;
            }
        }
        self.bool(key)
    }

    /// Gives this profile a new Guid.
    pub fn renew_guid(&mut self) {
        self.set(GUID, Value::String(uuid::Uuid::new_v4().to_string().to_uppercase()));
    }

    pub fn guid(&self) -> String {
        self.string(GUID)
    }

    pub fn name(&self) -> String {
        self.string(NAME)
    }
}

/// The command line a profile starts. "Login Shell" means the Windows default shell.
pub fn command_line(p: &Profile, default_shell: &str) -> String {
    match p.string("Custom Command").as_str() {
        "Yes" | "Custom Shell" if !p.string("Command").trim().is_empty() => p.string("Command"),
        _ => default_shell.to_string(),
    }
}

/// The directory a session starts in, and a warning when the profile's directory is
/// unusable. Home is %USERPROFILE%; "~" expands to it. A directory that does not exist
/// (an imported macOS path, for example) falls back to home, as iTerm2 does.
pub fn start_dir(p: &Profile) -> (std::path::PathBuf, Option<String>) {
    start_dir_with(p, None)
}

/// A reported directory as a Windows path: "/C:/Users/rob" (OSC 7 URL form) or
/// "C:\Users\rob". None for a path that is not a Windows path (a remote Unix one).
pub fn local_path(reported: &str) -> Option<std::path::PathBuf> {
    let s = reported.strip_prefix('/').filter(|r| r.as_bytes().get(1) == Some(&b':')).unwrap_or(reported);
    let b = s.as_bytes();
    (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':').then(|| std::path::PathBuf::from(s.replace('/', "\\")))
}

/// `start_dir`, with the directory of the session a new one comes from ("Reuse previous
/// session's directory"). A missing or unusable directory falls back to home.
pub fn start_dir_with(p: &Profile, previous: Option<&std::path::Path>) -> (std::path::PathBuf, Option<String>) {
    let home = std::env::var_os("USERPROFILE").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    if p.string("Custom Directory") == "Recycle" {
        return (previous.filter(|d| d.is_dir()).map(|d| d.to_path_buf()).unwrap_or(home), None);
    }
    let wanted = p.string("Working Directory");
    if p.string("Custom Directory") != "Yes" || wanted.trim().is_empty() {
        return (home, None);
    }
    let dir = match wanted.trim().strip_prefix('~') {
        Some(rest) => home.join(rest.trim_start_matches(['/', '\\'])),
        None => std::path::PathBuf::from(wanted.trim()),
    };
    if dir.is_dir() {
        (dir, None)
    } else {
        (home, Some(format!("Start directory {wanted} does not exist; using your home folder.")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::rgb;

    #[test]
    fn recycle_uses_the_previous_directory() {
        let mut p = Profile::default();
        p.set("Custom Directory", toml::Value::String("Recycle".into()));
        let dir = std::env::temp_dir();
        assert_eq!(start_dir_with(&p, Some(&dir)).0, dir);
    }

    #[test]
    fn recycle_falls_back_home() {
        let mut p = Profile::default();
        p.set("Custom Directory", toml::Value::String("Recycle".into()));
        let home = start_dir(&Profile::default()).0;
        assert_eq!(start_dir_with(&p, None).0, home);
        assert_eq!(start_dir_with(&p, Some(std::path::Path::new("Z:\\no\\such\\dir"))).0, home);
    }

    #[test]
    fn url_paths_become_windows_paths() {
        assert_eq!(local_path("/C:/Users/rob"), Some(std::path::PathBuf::from("C:\\Users\\rob")));
        assert_eq!(local_path("C:\\Users\\rob"), Some(std::path::PathBuf::from("C:\\Users\\rob")));
        assert_eq!(local_path("/home/rob"), None);
    }

    #[test]
    fn unset_keys_read_the_schema_default() {
        let p = Profile::default();
        assert_eq!(p.int("Columns"), 80);
        assert_eq!(p.int("Rows"), 25);
        assert!(p.bool("Use Bright Bold"));
        assert_eq!(p.int("Cursor Type"), 2);
    }

    #[test]
    fn set_values_win() {
        let mut p = Profile::default();
        p.set("Columns", toml::Value::Integer(120));
        assert_eq!(p.int("Columns"), 120);
    }

    #[test]
    fn colors_round_trip() {
        let mut p = Profile::default();
        p.set("Ansi 1 Color", color_value(rgb(0x12, 0x34, 0x56)));
        assert_eq!(p.color("Ansi 1 Color"), rgb(0x12, 0x34, 0x56));
    }

    #[test]
    fn light_and_dark_colors() {
        let mut p = Profile::stock();
        p.set("Foreground Color", color_value(rgb(1, 1, 1)));
        p.set("Foreground Color (Dark)", color_value(rgb(9, 9, 9)));
        // iTerm2's stock profile keeps separate Light and Dark colors, so switch that off first.
        p.set("Use Separate Colors for Light and Dark Mode", toml::Value::Boolean(false));
        assert_eq!(p.color_for("Foreground Color", true), rgb(1, 1, 1));
        p.set("Use Separate Colors for Light and Dark Mode", toml::Value::Boolean(true));
        assert_eq!(p.color_for("Foreground Color", true), rgb(9, 9, 9));
        // With separate colors on, an unset variant reads iTerm2's stock Light value, not the plain key.
        assert_eq!(p.color_for("Foreground Color", false), rgb(16, 16, 16));
    }

    #[test]
    fn command_line_follows_custom_command() {
        let mut p = Profile::default();
        assert_eq!(command_line(&p, "pwsh.exe"), "pwsh.exe");
        p.set("Custom Command", toml::Value::String("Yes".into()));
        p.set("Command", toml::Value::String("wsl.exe -d Ubuntu".into()));
        assert_eq!(command_line(&p, "pwsh.exe"), "wsl.exe -d Ubuntu");
    }

    #[test]
    fn start_dir_follows_custom_directory() {
        let mut p = Profile::default();
        p.set("Custom Directory", toml::Value::String("Yes".into()));
        let dir = std::env::temp_dir();
        p.set("Working Directory", toml::Value::String(dir.to_string_lossy().into_owned()));
        assert_eq!(start_dir(&p), (dir, None));
    }

    #[test]
    fn a_missing_key_reads_iterm2s_fallback_not_the_new_profile_seed() {
        // iTerm2 reads NO for a missing "Use Separate Colors" key, though new profiles start with YES.
        assert!(!Profile::default().bool("Use Separate Colors for Light and Dark Mode"));
        assert!(Profile::stock().bool("Use Separate Colors for Light and Dark Mode"));
        assert!(Profile::new("x").bool("Use Separate Colors for Light and Dark Mode"));
    }

    #[test]
    fn stock_dark_colors_come_from_the_schema() {
        // iTerm2's stock profile keeps separate colors; its Dark background is 0.081 0.099 0.121.
        let p = Profile::stock();
        assert_eq!(p.color_for("Background Color", true), rgb(21, 25, 31));
        assert_eq!(p.color_for("Background Color", false), rgb(250, 250, 250));
    }

    #[test]
    fn fonts_parse_family_and_size() {
        assert_eq!(parse_font("MesloLGS NF 13"), FontSpec { family: "MesloLGS NF".into(), size_pt: 13.0 });
        assert_eq!(parse_font("Cascadia Mono 12.5"), FontSpec { family: "Cascadia Mono".into(), size_pt: 12.5 });
        assert_eq!(parse_font("Consolas"), FontSpec { family: "Consolas".into(), size_pt: 12.0 });
    }

    #[test]
    fn font_sizes_stay_in_a_usable_range() {
        // A half-typed size ("Cascadia Mono 1") must not shrink the font to nothing.
        let mut p = Profile::default();
        p.set("Normal Font", toml::Value::String("Cascadia Mono 1".into()));
        assert_eq!(p.font("Normal Font").size_pt, 4.0);
        p.set("Normal Font", toml::Value::String("Cascadia Mono 999".into()));
        assert_eq!(p.font("Normal Font").size_pt, 128.0);
    }

    #[test]
    fn start_dir_is_home_by_default_and_expands_tilde() {
        let home = std::path::PathBuf::from(std::env::var_os("USERPROFILE").unwrap());
        let mut p = Profile::default();
        assert_eq!(start_dir(&p), (home.clone(), None));
        p.set("Custom Directory", toml::Value::String("Yes".into()));
        p.set("Working Directory", toml::Value::String("~".into()));
        assert_eq!(start_dir(&p), (home.clone(), None));
    }

    #[test]
    fn a_missing_start_dir_falls_back_to_home_with_a_warning() {
        let home = std::path::PathBuf::from(std::env::var_os("USERPROFILE").unwrap());
        let mut p = Profile::default();
        p.set("Custom Directory", toml::Value::String("Yes".into()));
        p.set("Working Directory", toml::Value::String("/Users/robert/src".into()));
        let (dir, warning) = start_dir(&p);
        assert_eq!(dir, home);
        assert!(warning.unwrap().contains("/Users/robert/src"));
    }

    #[test]
    fn bools_with_light_and_dark_variants() {
        let mut p = Profile::stock();
        p.set("Brighten Bold Text (Dark)", toml::Value::Boolean(false));
        assert!(!p.bool_for("Brighten Bold Text", true));
        assert!(!crate::palette::Palette::from_profile(&p, true).bright_bold);
    }

    #[test]
    fn new_profiles_get_an_id_and_windows_fonts() {
        let a = Profile::new("A");
        let b = Profile::new("B");
        assert_ne!(a.guid(), b.guid());
        assert_eq!(a.name(), "A");
        assert_eq!(a.font("Normal Font").family, "Cascadia Mono");
    }
}

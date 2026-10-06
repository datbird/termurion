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

/// `value` ("<family> <size>") with its size grown by `delta` points (`NSFont+iTerm.m` `fontWithSize:`
/// takes the integer part of size plus delta). Kept between 4 and 128, the range `Profile::font`
/// draws, so no step is hidden (iTerm2 keeps 2 to 200).
pub fn grow_font_value(value: &str, delta: f32) -> String {
    let f = parse_font(value);
    // The size is first seen as drawn (4 to 128), as `Profile::font` reads it.
    let size = (f.size_pt.clamp(4.0, 128.0) + delta).trunc().clamp(4.0, 128.0);
    format!("{} {}", f.family, size as u32)
}

/// How many points `current` is bigger than `original` (what an Automatic Profile Switch keeps).
pub fn font_delta(current: &str, original: &str) -> f32 {
    parse_font(current).size_pt - parse_font(original).size_pt
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

/// The profile's tag list.
const TAGS_KEY: &str = "Tags";

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

    /// The TERM the profile reports. iTerm2 seeds "xterm-256color" into every profile it makes,
    /// so a profile with no stored type (one made before Tandem had the key) reads that, not the
    /// schema's "xterm" fallback.
    pub fn terminal_type(&self) -> String {
        match self.values.get("Terminal Type") {
            Some(v) => v.as_str().unwrap_or_default().to_string(),
            None => "xterm-256color".to_string(),
        }
    }

    /// A profile from a stored or imported table.
    /// A profile from a stored or imported table; its "Bindings" is cleaned (`bindings::clean`).
    pub fn from_values(mut values: BTreeMap<String, Value>) -> Profile {
        if let Some(b) = values.remove(crate::bindings::KEY).and_then(crate::bindings::clean) {
            values.insert(crate::bindings::KEY.to_string(), b);
        }
        if let Some(t) = values.remove(TAGS_KEY) {
            if let Some(t) = crate::profile_list::clean_tags(t) {
                values.insert(TAGS_KEY.to_string(), t);
            }
        }
        Profile { values }
    }

    /// The one write: the Settings window, the API, an arrangement's session copy and the
    /// escape codes all land here. "Bindings" is cleaned once, so every reader sees at most
    /// `bindings::MAX_TABLE` bindable entries, and "Tags" the same way (`profile_list::clean_tags`).
    pub fn set(&mut self, key: &str, value: Value) {
        if key == crate::bindings::KEY {
            match crate::bindings::clean(value) {
                Some(b) => self.values.insert(key.to_string(), b),
                None => self.values.remove(key),
            };
            return;
        }
        if key == TAGS_KEY {
            match crate::profile_list::clean_tags(value) {
                Some(t) => self.values.insert(key.to_string(), t),
                None => self.values.remove(key),
            };
            return;
        }
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

    /// `string` borrowed: the profile's own text or the schema default's, no copy (a font table
    /// can be 256 KiB, and a frame reads it for every pane).
    pub fn str_ref(&self, key: &str) -> &str {
        let own = self.values.get(key);
        let stock = || schema::find(key).and_then(|s| s.default.as_ref());
        own.or_else(stock).and_then(|v| v.as_str()).unwrap_or("")
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

    /// A number that may have Light and Dark variants, read like `bool_for`.
    pub fn float_for(&self, key: &str, dark: bool) -> f64 {
        if self.bool("Use Separate Colors for Light and Dark Mode") {
            let variant = format!("{key} ({})", if dark { "Dark" } else { "Light" });
            let stock = schema::find(key).and_then(|s| if dark { s.default_dark.as_ref() } else { s.default_light.as_ref() });
            if let Some(f) = self.values.get(&variant).or(stock).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))) {
                return f;
            }
        }
        self.float(key)
    }

    /// The tab's color (Use Tab Color and Tab Color, for the mode); None without one.
    pub fn tab_color(&self, dark: bool) -> Option<Rgb> {
        if !self.bool_for("Use Tab Color", dark) {
            return None;
        }
        let variant = format!("Tab Color ({})", if dark { "Dark" } else { "Light" });
        let separate = self.bool("Use Separate Colors for Light and Dark Mode");
        separate.then(|| self.values.get(&variant)).flatten().or_else(|| self.values.get("Tab Color")).and_then(to_rgb)
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

/// Whether the profile's command type is SSH (`Custom Command` "SSH").
pub fn is_ssh(p: &Profile) -> bool {
    p.string("Custom Command") == "SSH"
}

/// Whether an SSH profile runs through Tandem's `it2ssh` (`AB:893-926`): SSH with a non-blank
/// Command, SSH Integration on, and `it2ssh` usable.
pub fn ssh_integrated(p: &Profile, it2ssh: Option<&std::path::Path>) -> bool {
    is_ssh(p) && !p.string("Command").trim().is_empty() && crate::ssh_config::from_profile(p).ssh_integration && it2ssh.is_some()
}

/// The remote folder an SSH profile's session starts in: the custom Working Directory, only
/// when it is a POSIX path (starts with `/` or `~`); a Windows path cannot name a remote
/// folder (iTerm2 sends the local `PWD`, `PS:20129-20132`).
pub fn ssh_initial_directory(p: &Profile) -> Option<String> {
    if p.string("Custom Directory") != "Yes" {
        return None;
    }
    let wanted = p.string("Working Directory");
    let wanted = wanted.trim();
    wanted.starts_with(['/', '~']).then(|| wanted.to_string())
}

/// The command line a profile starts. "Login Shell" means the Windows default shell. SSH runs
/// `ssh <Command>`, or `it2ssh.exe <Command>` with SSH Integration (`AB:862-926`); the joined
/// text is trimmed, and a blank Command runs the shell.
pub fn command_line(p: &Profile, default_shell: &str, it2ssh: Option<&std::path::Path>) -> String {
    match p.string("Custom Command").as_str() {
        "Yes" | "Custom Shell" if !p.string("Command").trim().is_empty() => p.string("Command"),
        "SSH" if !p.string("Command").trim().is_empty() => {
            let program = match it2ssh.filter(|_| ssh_integrated(p, it2ssh)) {
                Some(exe) => format!("\"{}\"", exe.display()),
                None => "ssh".to_string(),
            };
            format!("{program} {}", p.string("Command")).trim().to_string()
        }
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

/// A saved folder as the Windows path a shell starts in: `local_path`'s, or "~" or a path
/// under it in the home folder (iTerm2 standardizes the path, expanding the tilde, before it
/// checks it, `iTermSessionFactory.m:346-354`). None for any other path ("~user", WSL, MSYS,
/// relative).
pub fn local_folder(saved: &str) -> Option<std::path::PathBuf> {
    let Some(rest) = saved.strip_prefix('~') else { return local_path(saved) };
    if !rest.is_empty() && !rest.starts_with(['/', '\\']) {
        return None;
    }
    let rest = rest.trim_start_matches(['/', '\\']);
    Some(if rest.is_empty() { home() } else { home().join(rest.replace('/', "\\")) })
}

/// The home folder a session starts in: %USERPROFILE%.
pub fn home() -> std::path::PathBuf {
    std::env::var_os("USERPROFILE").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

/// `start_dir`, with the directory of the session a new one comes from ("Reuse previous
/// session's directory"). A missing or unusable directory falls back to home.
pub fn start_dir_with(p: &Profile, previous: Option<&std::path::Path>) -> (std::path::PathBuf, Option<String>) {
    start_dir_probing(p, previous, &|d| d.is_dir())
}

/// `start_dir_with`, with the local folder check given (tests record what it is asked). A
/// tab's directory (`start_dir::resolve`), without interpolation: the app expands for a session.
pub fn start_dir_probing(p: &Profile, previous: Option<&std::path::Path>, is_dir: &dyn Fn(&std::path::Path) -> bool) -> (std::path::PathBuf, Option<String>) {
    crate::start_dir::resolve(&crate::start_dir::mode(p, crate::start_dir::Object::Tab), previous, &home(), &|t| t.to_string(), is_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::rgb;

    /// `NSFont+iTerm.m:32-47` with Tandem's drawn range (pre-flight P2: 4 to 128, not 2 to 200).
    #[test]
    fn grown_fonts_follow_iterm2() {
        assert_eq!(grow_font_value("Consolas 12", 1.0), "Consolas 13");
        assert_eq!(grow_font_value("Consolas 12.5", 1.0), "Consolas 13");
        assert_eq!(grow_font_value("Consolas 12.5", -1.0), "Consolas 11");
        assert_eq!(grow_font_value("Cascadia Mono 12", 2.0), "Cascadia Mono 14");
        assert_eq!(grow_font_value("Consolas 4", -1.0), "Consolas 4");
        assert_eq!(grow_font_value("Consolas 128", 1.0), "Consolas 128");
        // A stored size outside the drawn range is first seen as drawn, so a step always acts.
        assert_eq!(grow_font_value("Consolas 2", 1.0), "Consolas 5");
        assert_eq!(grow_font_value("Consolas 200", -1.0), "Consolas 127");
        assert_eq!(font_delta("Consolas 14", "Consolas 12"), 2.0);
        assert_eq!(font_delta("Consolas 10", "Consolas 12.5"), -2.5);
    }

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
        assert_eq!(local_folder("/D:/x"), Some(std::path::PathBuf::from("D:\\x")));
        assert_eq!(local_folder("~/www/a"), Some(home().join("www\\a")));
        assert_eq!(local_folder("~\\www"), Some(home().join("www")));
        assert_eq!(local_folder("~"), Some(home()));
        assert_eq!(local_folder("~rob/www"), None);
        assert_eq!(local_folder("/home/rob"), None);
        assert_eq!(local_folder("src"), None);
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
    fn float_for_reads_the_variant() {
        let mut p = Profile::stock();
        p.set("Use Separate Colors for Light and Dark Mode", toml::Value::Boolean(true));
        p.set("Cursor Boost (Dark)", toml::Value::Float(0.4));
        p.set("Cursor Boost (Light)", toml::Value::Float(0.1));
        assert_eq!(p.float_for("Cursor Boost", true), 0.4);
        assert_eq!(p.float_for("Cursor Boost", false), 0.1);
        p.set("Use Separate Colors for Light and Dark Mode", toml::Value::Boolean(false));
        p.set("Cursor Boost", toml::Value::Float(0.3));
        assert_eq!(p.float_for("Cursor Boost", true), 0.3);
        assert_eq!(p.float_for("Cursor Boost", false), 0.3);
    }

    #[test]
    fn minimum_contrast_reads_the_dark_variant() {
        let mut p = Profile::stock();
        p.set("Use Separate Colors for Light and Dark Mode", toml::Value::Boolean(true));
        p.set("Minimum Contrast (Dark)", toml::Value::Float(0.6));
        p.set("Minimum Contrast (Light)", toml::Value::Float(0.2));
        assert_eq!(p.float_for("Minimum Contrast", true), 0.6);
        assert_eq!(p.float_for("Minimum Contrast", false), 0.2);
    }

    #[test]
    fn ssh_command_line_follows_iterm2() {
        let u = std::path::Path::new(r"C:\T\utilities\it2ssh.exe");
        let s = |v: &str| toml::Value::String(v.into());
        let mut p = Profile::default();
        p.set("Custom Command", s("SSH"));
        p.set("Command", s("user@host -p 2222"));
        assert!(is_ssh(&p));
        assert_eq!(command_line(&p, "pwsh.exe", Some(u)), "\"C:\\T\\utilities\\it2ssh.exe\" user@host -p 2222");
        assert!(ssh_integrated(&p, Some(u)));
        assert!(!ssh_integrated(&p, None));
        assert_eq!(command_line(&p, "pwsh.exe", None), "ssh user@host -p 2222");
        let mut off = toml::Table::new();
        off.insert("sshIntegration".into(), toml::Value::Boolean(false));
        let mut q = p.clone();
        q.set("SSH", toml::Value::Table(off));
        assert_eq!(command_line(&q, "pwsh.exe", Some(u)), "ssh user@host -p 2222");
        assert!(!ssh_integrated(&q, Some(u)));
        p.set("Command", s("  host \n"));
        assert_eq!(command_line(&p, "pwsh.exe", None), "ssh   host", "the joined string is trimmed, as iTerm2 trims");
        p.set("Command", s("   "));
        assert_eq!(command_line(&p, "pwsh.exe", Some(u)), "pwsh.exe");
        assert!(!ssh_integrated(&p, Some(u)), "a blank Command runs the shell");
        p.set("Custom Command", s("Yes"));
        p.set("Command", s("x"));
        assert!(!is_ssh(&p));
        assert_eq!(command_line(&p, "pwsh.exe", Some(u)), "x");
        p.set("Custom Command", s("No"));
        assert_eq!(command_line(&p, "pwsh.exe", Some(u)), "pwsh.exe");
    }

    #[test]
    fn ssh_initial_directory_follows_windows() {
        let s = |v: &str| toml::Value::String(v.into());
        let mut p = Profile::default();
        p.set("Custom Directory", s("Yes"));
        p.set("Working Directory", s("/srv/app"));
        assert_eq!(ssh_initial_directory(&p).as_deref(), Some("/srv/app"));
        p.set("Working Directory", s("~/proj"));
        assert_eq!(ssh_initial_directory(&p).as_deref(), Some("~/proj"));
        p.set("Working Directory", s("C:\\work"));
        assert_eq!(ssh_initial_directory(&p), None, "a Windows path cannot name a remote folder");
        p.set("Working Directory", s("/srv/app"));
        p.set("Custom Directory", s("No"));
        assert_eq!(ssh_initial_directory(&p), None);
        p.set("Custom Directory", s("Recycle"));
        assert_eq!(ssh_initial_directory(&p), None);
    }

    #[test]
    fn command_line_follows_custom_command() {
        let mut p = Profile::default();
        assert_eq!(command_line(&p, "pwsh.exe", None), "pwsh.exe");
        p.set("Custom Command", toml::Value::String("Yes".into()));
        p.set("Command", toml::Value::String("wsl.exe -d Ubuntu".into()));
        assert_eq!(command_line(&p, "pwsh.exe", None), "wsl.exe -d Ubuntu");
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
    fn profile_tab_colors_follow_the_mode() {
        assert_eq!(Profile::stock().tab_color(false), None, "off in the stock profile");
        let mut p = Profile::default();
        assert_eq!(p.tab_color(false), None, "off by default");
        p.set("Use Tab Color", toml::Value::Boolean(true));
        assert_eq!(p.tab_color(false), None, "on, but no color picked");
        p.set("Tab Color", color_value(rgb(10, 20, 30)));
        assert_eq!(p.tab_color(false), Some(rgb(10, 20, 30)));
        p.set("Use Separate Colors for Light and Dark Mode", toml::Value::Boolean(true));
        p.set("Use Tab Color (Dark)", toml::Value::Boolean(true));
        p.set("Tab Color (Dark)", color_value(rgb(1, 2, 3)));
        assert_eq!(p.tab_color(true), Some(rgb(1, 2, 3)));
        p.set("Use Tab Color (Light)", toml::Value::Boolean(false));
        assert_eq!(p.tab_color(false), None, "light mode has it off");
    }

    #[test]
    fn new_profiles_get_an_id_and_windows_fonts() {
        let a = Profile::new("A");
        let b = Profile::new("B");
        assert_ne!(a.guid(), b.guid());
        assert_eq!(a.name(), "A");
        assert_eq!(a.font("Normal Font").family, "Cascadia Mono");
    }

    #[test]
    fn stock_terminal_type_is_xterm_256color() {
        assert_eq!(Profile::stock().string("Terminal Type"), "xterm-256color");
    }

    /// Profiles made before the key existed have none stored: they keep reporting
    /// xterm-256color (iTerm2's seeded value), and a stored value, even "xterm", wins.
    #[test]
    fn a_profile_without_a_terminal_type_reports_xterm_256color() {
        let mut p = Profile::stock();
        p.values.remove("Terminal Type");
        assert_eq!(p.terminal_type(), "xterm-256color");
        // The editor reads `get`, which must say the same thing as the TERM that is reported.
        assert_eq!(p.get("Terminal Type"), Some(Value::String("xterm-256color".into())));
        p.set("Terminal Type", Value::String("xterm".into()));
        assert_eq!(p.terminal_type(), "xterm");
        p.set("Terminal Type", Value::String(String::new()));
        assert_eq!(p.terminal_type(), "", "a stored empty type is reported as it is");
    }
}

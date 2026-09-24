# Tandem Milestone 2: Profiles and the Settings Window

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** iTerm2 profiles in Tandem: every profile setting exists, persists, and shows in a settings window laid out like iTerm2's, and the settings that shape the terminal (colors, fonts, spacing, cursor, scrollback, window size, command, directory) take effect live.

**Architecture:** A profile is iTerm2's own model: a map from iTerm2 key to value, falling back to the defaults in `schema/profile.toml` (242 keys). `schema.rs` loads the schema, `profile.rs` reads typed values, `store.rs` saves and loads `%APPDATA%\Tandem\profiles.toml`, and `import.rs` reads an iTerm2 plist or Dynamic Profiles JSON. `palette.rs` and the renderer take their values from a profile. `settings_ui.rs` is an egui window whose controls come from the schema.

**Tech Stack:** as milestone 1, plus `serde` 1, `toml` 1, `plist` 1, `serde_json` 1, `uuid` 1 (v4), `rfd` 0.17, `egui`/`egui-winit`/`egui-wgpu` 0.36.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md` (goal revised 2026-09-24: full iTerm2 parity). Inventory: `docs/parity/01-profile-settings.md`. Roadmap: `docs/parity/00-roadmap.md`.

## Global Constraints

- Everything from the milestone 0 and 1 plans still holds.
- Profile keys are iTerm2's plist key strings, unchanged, so import is a copy.
- The saved file keeps only values that differ from the schema default (plus `Guid` and `Name`).
- A bad settings file never silently resets: the app keeps the last good profiles and shows a banner.
- Windows platform defaults: a new profile uses `Cascadia Mono 12` for `Normal Font` and `Non Ascii Font`, because iTerm2's `Monaco 12` does not ship with Windows.
- Settings with no Windows meaning (`macos_only = true` in the schema) show in the window, disabled, with the reason.
- Row status in `docs/parity/01-profile-settings.md` is updated at the end: `have` when the setting persists and takes effect, `partial` when it persists and shows but its effect comes in a later milestone.

## Review Focus

1. A settings file that is hand-edited into invalid TOML. Expect: the app keeps working with the last good profiles and shows the parse error. Test: Task 3 `a_corrupt_file_is_an_error_not_a_reset`.
2. An iTerm2 export with keys Tandem does not know, or data blobs. Expect: unknown keys are kept, data blobs are skipped, nothing panics. Test: Task 4 `unknown_keys_are_kept_and_data_is_skipped`.
3. A font name that is an iTerm2 PostScript name (`MesloLGS-NF-Regular 13`). Expect: it becomes the family name Windows knows (`MesloLGS NF`). Test: Task 4 `postscript_font_names_become_family_names`.
4. Deleting the last profile, or the default one. Expect: the last profile cannot be deleted; deleting the default makes another one the default. Test: Task 3 `the_last_profile_cannot_be_deleted`.
5. A color stored in Display P3. Expect: it is converted to sRGB, not used raw. Test: Task 4 `p3_colors_are_converted`.

---

### Task 1: Schema loader

**Files:**
- Create: `src/schema.rs`
- Modify: `Cargo.toml` (add `serde = { version = "1", features = ["derive"] }`, `toml = "1"`), `src/lib.rs` (add `pub mod schema;`)

**Interfaces:**
- Produces (in `tandem::schema`):
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)] pub enum Kind { Bool, Int, Float, String, Color, Enum, Font, List, Dict, Data }`
  - `#[derive(Clone, Debug, Deserialize)] pub struct Choice { pub value: toml::Value, pub label: String }`
  - `#[derive(Clone, Debug, Deserialize)] pub struct Setting { pub key, pub tab, pub section, pub label: String, pub kind: Kind, pub default: Option<toml::Value>, pub min: Option<toml::Value>, pub max: Option<toml::Value>, pub choices: Vec<Choice>, pub help: String, pub light_dark: bool, pub macos_only: bool }` with `min_f64(&self) -> Option<f64>` and `max_f64(&self) -> Option<f64>`
  - `pub const TABS: [&str; 8]`
  - `pub fn profile() -> &'static [Setting]`, `pub fn find(key: &str) -> Option<&'static Setting>`

- [ ] **Step 1: Write the failing tests**

`src/schema.rs`:

```rust
//! The iTerm2 profile schema: every profile key with its type, default, label, and tab.
//! The data lives in schema/profile.toml, built from iTerm2's source.

#[cfg(test)]
mod tests {
    use super::*;
    use toml::Value;

    #[test]
    fn loads_every_setting() {
        assert!(profile().len() >= 242, "{}", profile().len());
    }

    #[test]
    fn every_tab_has_settings() {
        for tab in TABS {
            assert!(profile().iter().any(|s| s.tab == tab), "no settings in {tab}");
        }
        assert!(profile().iter().all(|s| TABS.contains(&s.tab.as_str())), "a setting names an unknown tab");
    }

    #[test]
    fn known_keys_are_present() {
        for key in ["Ansi 0 Color", "Ansi 15 Color", "Foreground Color", "Normal Font", "Cursor Type", "Columns", "Rows", "Scrollback Lines", "Command", "Custom Command", "Working Directory", "Option Key Sends", "Triggers", "Smart Selection Rules"] {
            assert!(find(key).is_some(), "{key} missing");
        }
    }

    #[test]
    fn defaults_match_their_kind() {
        for s in profile() {
            let Some(d) = &s.default else { continue };
            let ok = match s.kind {
                Kind::Bool => d.is_bool(),
                Kind::Int => d.is_integer(),
                Kind::Float => d.is_float() || d.is_integer(),
                Kind::String | Kind::Font => d.is_str(),
                Kind::Color => d.as_table().is_some_and(|t| ["r", "g", "b"].iter().all(|c| t.get(*c).is_some_and(|v| v.is_float() || v.is_integer()))),
                Kind::Enum => s.choices.is_empty() || s.choices.iter().any(|c| &c.value == d),
                Kind::List => d.is_array(),
                Kind::Dict => d.is_table(),
                Kind::Data => true,
            };
            assert!(ok, "{}: default {d:?} does not fit {:?}", s.key, s.kind);
        }
        let _ = Value::Boolean(true);
    }

    #[test]
    fn keys_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for s in profile() {
            assert!(seen.insert(s.key.as_str()), "duplicate key {}", s.key);
        }
    }
}
```

Add to `Cargo.toml` `[dependencies]`: `serde = { version = "1", features = ["derive"] }` and `toml = "1"`. Add `pub mod schema;` to `src/lib.rs`.

- [ ] **Step 2: Run the tests to see them fail**

Run: `scripts/xt test --lib schema`
Expected: compile errors (`profile`, `find`, `Kind` not found).

- [ ] **Step 3: Write the loader**

Put above the tests in `src/schema.rs`:

```rust
use std::sync::OnceLock;

use serde::Deserialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Bool,
    Int,
    Float,
    String,
    Color,
    Enum,
    Font,
    List,
    Dict,
    Data,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Choice {
    pub value: toml::Value,
    pub label: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Setting {
    pub key: String,
    pub tab: String,
    #[serde(default)]
    pub section: String,
    pub label: String,
    #[serde(rename = "type")]
    pub kind: Kind,
    pub default: Option<toml::Value>,
    pub min: Option<toml::Value>,
    pub max: Option<toml::Value>,
    #[serde(default)]
    pub choices: Vec<Choice>,
    #[serde(default)]
    pub help: String,
    #[serde(default)]
    pub light_dark: bool,
    #[serde(default)]
    pub macos_only: bool,
}

fn number(v: &toml::Value) -> Option<f64> {
    v.as_float().or_else(|| v.as_integer().map(|i| i as f64))
}

impl Setting {
    pub fn min_f64(&self) -> Option<f64> {
        self.min.as_ref().and_then(number)
    }

    pub fn max_f64(&self) -> Option<f64> {
        self.max.as_ref().and_then(number)
    }
}

#[derive(Deserialize)]
struct File {
    setting: Vec<Setting>,
}

/// The iTerm2 profile editor's tabs, in its order.
pub const TABS: [&str; 8] = ["General", "Colors", "Text", "Window", "Terminal", "Session", "Keys", "Advanced"];

pub fn profile() -> &'static [Setting] {
    static SETTINGS: OnceLock<Vec<Setting>> = OnceLock::new();
    SETTINGS.get_or_init(|| {
        toml::from_str::<File>(include_str!("../schema/profile.toml"))
            .expect("schema/profile.toml must parse; the schema tests check it")
            .setting
    })
}

pub fn find(key: &str) -> Option<&'static Setting> {
    profile().iter().find(|s| s.key == key)
}
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `scripts/xt test --lib schema`
Expected: `5 passed`. If `defaults_match_their_kind` or `keys_are_unique` fails, the fault is in the schema data: fix the entry in `schema/profile.toml` against iTerm2's source and ledger it.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/schema.rs src/lib.rs schema/profile.toml
git commit -m "Load the iTerm2 profile schema"
git push
```

---

### Task 2: Profile values

**Files:**
- Create: `src/profile.rs`
- Modify: `Cargo.toml` (add `uuid = { version = "1", features = ["v4"] }`), `src/lib.rs` (add `pub mod profile;`)

**Interfaces:**
- Consumes: `schema::{find, profile}`, `palette::{Rgb, rgb}`.
- Produces (in `tandem::profile`):
  - `#[derive(Clone, Debug, Default, PartialEq)] pub struct Profile { pub values: BTreeMap<String, toml::Value> }`
  - `#[derive(Clone, Debug, PartialEq)] pub struct FontSpec { pub family: String, pub size_pt: f32 }`
  - `Profile::new(name: &str) -> Profile` (new Guid, Windows font defaults)
  - `get(&self, key) -> Option<toml::Value>`, `set(&mut self, key, value: toml::Value)`, `bool`, `int -> i64`, `float -> f64`, `string -> String`, `color(&self, key) -> Rgb`, `color_for(&self, key, dark: bool) -> Rgb`, `font(&self, key) -> FontSpec`, `guid() -> String`, `name() -> String`
  - `pub fn color_value(c: Rgb) -> toml::Value`
  - `pub fn parse_font(s: &str) -> FontSpec`

- [ ] **Step 1: Write the failing tests**

`src/profile.rs`:

```rust
//! A profile: iTerm2 keys mapped to values. A key with no value reads the schema default.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::rgb;

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
        let mut p = Profile::default();
        p.set("Foreground Color", color_value(rgb(1, 1, 1)));
        p.set("Foreground Color (Dark)", color_value(rgb(9, 9, 9)));
        assert_eq!(p.color_for("Foreground Color", true), rgb(1, 1, 1));
        p.set("Use Separate Colors for Light and Dark Mode", toml::Value::Boolean(true));
        assert_eq!(p.color_for("Foreground Color", true), rgb(9, 9, 9));
        assert_eq!(p.color_for("Foreground Color", false), rgb(1, 1, 1));
    }

    #[test]
    fn fonts_parse_family_and_size() {
        assert_eq!(parse_font("MesloLGS NF 13"), FontSpec { family: "MesloLGS NF".into(), size_pt: 13.0 });
        assert_eq!(parse_font("Cascadia Mono 12.5"), FontSpec { family: "Cascadia Mono".into(), size_pt: 12.5 });
        assert_eq!(parse_font("Consolas"), FontSpec { family: "Consolas".into(), size_pt: 12.0 });
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
```

Add `uuid = { version = "1", features = ["v4"] }` to `Cargo.toml` and `pub mod profile;` to `src/lib.rs`.

- [ ] **Step 2: Run the tests to see them fail**

Run: `scripts/xt test --lib profile`
Expected: compile errors (`Profile` not found).

- [ ] **Step 3: Write the profile**

Put above the tests in `src/profile.rs`:

```rust
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
    pub fn new(name: &str) -> Profile {
        let mut p = Profile::default();
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
            if let Some(c) = self.values.get(&variant).and_then(to_rgb) {
                return c;
            }
        }
        self.color(key)
    }

    pub fn font(&self, key: &str) -> FontSpec {
        parse_font(&self.string(key))
    }

    pub fn guid(&self) -> String {
        self.string(GUID)
    }

    pub fn name(&self) -> String {
        self.string(NAME)
    }
}
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `scripts/xt test --lib profile`
Expected: `6 passed`.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/profile.rs src/lib.rs
git commit -m "Add profiles backed by the iTerm2 schema"
git push
```

---

### Task 3: Profile store

**Files:**
- Create: `src/store.rs`
- Modify: `src/lib.rs` (add `pub mod store;`)

**Interfaces:**
- Consumes: `profile::{Profile, GUID, NAME}`, `schema::find`.
- Produces (in `tandem::store`):
  - `pub struct Store { pub profiles: Vec<Profile>, pub default_guid: String, path: PathBuf }`
  - `Store::load(path: &Path) -> Result<Store, String>` (a missing file gives one profile, "Default")
  - `Store::save(&self) -> Result<(), String>` (non-default values only; write then rename)
  - `default_profile(&self) -> &Profile`, `get(&self, guid) -> Option<&Profile>`, `get_mut(&mut self, guid) -> Option<&mut Profile>`, `add(&mut self, Profile)`, `duplicate(&mut self, guid) -> Option<String>`, `delete(&mut self, guid) -> bool`
  - `pub fn default_path() -> PathBuf` (`%APPDATA%\Tandem\profiles.toml`)

- [ ] **Step 1: Write the failing tests**

`src/store.rs`:

```rust
//! Saves and loads profiles in one TOML file. Only values that differ from the
//! schema defaults are written, so the file stays small and readable.

#[cfg(test)]
mod tests {
    use super::*;
    use toml::Value;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tandem-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("profiles.toml")
    }

    #[test]
    fn a_missing_file_gives_one_default_profile() {
        let s = Store::load(&scratch("missing")).unwrap();
        assert_eq!(s.profiles.len(), 1);
        assert_eq!(s.default_profile().name(), "Default");
        assert_eq!(s.default_guid, s.profiles[0].guid());
    }

    #[test]
    fn save_and_load_round_trip() {
        let path = scratch("roundtrip");
        let mut s = Store::load(&path).unwrap();
        let guid = s.default_guid.clone();
        s.get_mut(&guid).unwrap().set("Columns", Value::Integer(132));
        s.save().unwrap();
        let t = Store::load(&path).unwrap();
        assert_eq!(t.get(&guid).unwrap().int("Columns"), 132);
        assert_eq!(t.default_guid, guid);
    }

    #[test]
    fn saved_files_omit_defaults() {
        let path = scratch("omit");
        let mut s = Store::load(&path).unwrap();
        let guid = s.default_guid.clone();
        s.get_mut(&guid).unwrap().set("Rows", Value::Integer(25));
        s.save().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("Rows"), "{text}");
        assert!(text.contains("Guid"), "{text}");
    }

    #[test]
    fn a_corrupt_file_is_an_error_not_a_reset() {
        let path = scratch("corrupt");
        std::fs::write(&path, "this is [not toml").unwrap();
        let err = Store::load(&path).err().unwrap();
        assert!(err.contains("profiles.toml"), "{err}");
    }

    #[test]
    fn the_last_profile_cannot_be_deleted() {
        let mut s = Store::load(&scratch("delete")).unwrap();
        let first = s.default_guid.clone();
        assert!(!s.delete(&first));
        let second = s.duplicate(&first).unwrap();
        assert!(s.delete(&first));
        assert_eq!(s.default_guid, second);
        assert_eq!(s.profiles.len(), 1);
    }
}
```

Add `pub mod store;` to `src/lib.rs`.

- [ ] **Step 2: Run the tests to see them fail**

Run: `scripts/xt test --lib store`
Expected: compile errors (`Store` not found).

- [ ] **Step 3: Write the store**

Put above the tests in `src/store.rs`:

```rust
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use toml::Value;

use crate::profile::{GUID, NAME, Profile};
use crate::schema;

pub struct Store {
    pub profiles: Vec<Profile>,
    pub default_guid: String,
    path: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct FileFormat {
    default: String,
    profile: Vec<BTreeMap<String, Value>>,
}

pub fn default_path() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("Tandem").join("profiles.toml")
}

impl Store {
    pub fn load(path: &Path) -> Result<Store, String> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let p = Profile::new("Default");
                return Ok(Store { default_guid: p.guid(), profiles: vec![p], path: path.to_path_buf() });
            }
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let file: FileFormat = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let profiles: Vec<Profile> = file.profile.into_iter().map(|values| Profile { values }).collect();
        if profiles.is_empty() {
            return Err(format!("{}: no profiles", path.display()));
        }
        let default_guid = if profiles.iter().any(|p| p.guid() == file.default) { file.default } else { profiles[0].guid() };
        Ok(Store { profiles, default_guid, path: path.to_path_buf() })
    }

    pub fn save(&self) -> Result<(), String> {
        let profile = self
            .profiles
            .iter()
            .map(|p| {
                p.values
                    .iter()
                    .filter(|(k, v)| {
                        k.as_str() == GUID || k.as_str() == NAME || schema::find(k).and_then(|s| s.default.as_ref()) != Some(*v)
                    })
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .collect();
        let text = toml::to_string(&FileFormat { default: self.default_guid.clone(), profile })
            .map_err(|e| format!("saving profiles: {e}"))?;
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        // Write a new file, then rename it over the old one, so a crash never leaves half a file.
        let tmp = self.path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| format!("{}: {e}", self.path.display()))
    }

    pub fn default_profile(&self) -> &Profile {
        self.get(&self.default_guid).unwrap_or(&self.profiles[0])
    }

    pub fn get(&self, guid: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.guid() == guid)
    }

    pub fn get_mut(&mut self, guid: &str) -> Option<&mut Profile> {
        self.profiles.iter_mut().find(|p| p.guid() == guid)
    }

    pub fn add(&mut self, profile: Profile) {
        self.profiles.push(profile);
    }

    /// Copies a profile under a new Guid and "<name> copy". Returns the new Guid.
    pub fn duplicate(&mut self, guid: &str) -> Option<String> {
        let mut copy = self.get(guid)?.clone();
        let new_guid = uuid::Uuid::new_v4().to_string().to_uppercase();
        copy.set(GUID, Value::String(new_guid.clone()));
        copy.set(NAME, Value::String(format!("{} copy", copy.name())));
        self.profiles.push(copy);
        Some(new_guid)
    }

    /// Deletes a profile. The last profile cannot be deleted. Deleting the default
    /// makes the first remaining profile the default.
    pub fn delete(&mut self, guid: &str) -> bool {
        if self.profiles.len() < 2 {
            return false;
        }
        let before = self.profiles.len();
        self.profiles.retain(|p| p.guid() != guid);
        if self.default_guid == guid {
            self.default_guid = self.profiles[0].guid();
        }
        self.profiles.len() < before
    }
}
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `scripts/xt test --lib store`
Expected: `5 passed`.

- [ ] **Step 5: Commit**

```bash
git add src/store.rs src/lib.rs
git commit -m "Save and load profiles, keeping only non-default values"
git push
```

---

### Task 4: iTerm2 import

**Files:**
- Create: `src/import.rs`
- Modify: `Cargo.toml` (add `plist = "1"`, `serde_json = "1"`), `src/lib.rs` (add `pub mod import;`)

**Interfaces:**
- Consumes: `profile::{Profile, parse_font}`.
- Produces (in `tandem::import`):
  - `pub fn from_plist_bytes(bytes: &[u8]) -> Result<Vec<Profile>, String>` (reads `New Bookmarks` from a full iTerm2 settings plist)
  - `pub fn from_dynamic_json(text: &str) -> Result<Vec<Profile>, String>` (`{"Profiles": [...]}`)
  - `pub fn from_file(path: &Path) -> Result<Vec<Profile>, String>` (picks by extension: `.plist` or `.json`)
  - `pub fn family_from_postscript(name: &str) -> String`

- [ ] **Step 1: Write the failing tests**

`src/import.rs`:

```rust
//! Imports iTerm2 profiles: the full settings plist (com.googlecode.iterm2.plist)
//! or a Dynamic Profiles JSON file. This is a one-time copy, not a sync.

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
```

Add `plist = "1"` and `serde_json = "1"` to `Cargo.toml`; add `pub mod import;` to `src/lib.rs`.

- [ ] **Step 2: Run the tests to see them fail**

Run: `scripts/xt test --lib import`
Expected: compile errors (`from_plist_bytes` not found).

- [ ] **Step 3: Write the importer**

Put above the tests in `src/import.rs`:

```rust
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
        .collect())
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
        .collect())
}

pub fn from_file(path: &Path) -> Result<Vec<Profile>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    match path.extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref() {
        Some("json") => from_dynamic_json(&String::from_utf8_lossy(&bytes)),
        _ => from_plist_bytes(&bytes),
    }
}
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `scripts/xt test --lib import`
Expected: `6 passed`.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/import.rs src/lib.rs
git commit -m "Import iTerm2 profiles from a settings plist or Dynamic Profiles JSON"
git push
```

---

### Task 5: Profiles drive the terminal

**Files:**
- Modify: `src/palette.rs`, `src/screen.rs`, `src/scene.rs`, `src/render.rs`, `src/pty.rs`, `src/pane.rs`, `src/app.rs`

**Interfaces:**
- Consumes: `profile::Profile`.
- Produces:
  - `Palette::from_profile(p: &Profile, dark: bool) -> Palette`; `Palette::default()` is now `from_profile(&Profile::default(), true)`; `Palette.selected_text` becomes `Option<Rgb>` (None keeps the cell's own color, iTerm2's default); new field `faint_alpha: f32` used for DIM instead of 2/3.
  - `pub struct Look { pub cursor_shape: CursorShape, pub cursor_blink: bool, pub scrollback: usize }` in `screen.rs`, `Look::from_profile(&Profile)`, `Screen::with_look(cols, rows, palette, look)`, `Screen::set_palette`, `Screen::set_look`.
  - `scene::pane(frame, screen, rect, cell, focused, cursor_on: bool)` (blink phase); `Span` gains `pub alt_font: bool` (true for non-ASCII pieces).
  - `Renderer::set_fonts(&mut self, normal: &str, non_ascii: Option<&str>, px: f32, h_spacing: f32, v_spacing: f32)`.
  - `pty::spawn_notify(api, cmdline, cwd: Option<&Path>, cols, rows, notify)` (new `cwd`); `Pane::start(id, api, cmdline, cwd, cols, rows, notify)`; `Pane.profile: String` (Guid).
  - `pub fn command_line(p: &Profile, default_shell: &str) -> String` and `pub fn working_dir(p: &Profile) -> Option<PathBuf>` in `profile.rs`.

- [ ] **Step 1: Write the failing tests**

Add to `src/palette.rs` tests (and change `named_red_is_itermish` to expect the schema's stock default `rgb(0xb4, 0x3c, 0x2a)`, since the palette now comes from iTerm2's real defaults):

```rust
    #[test]
    fn palette_reads_the_profile() {
        let mut p = crate::profile::Profile::default();
        p.set("Ansi 4 Color", crate::profile::color_value(rgb(1, 2, 3)));
        p.set("Brighten Bold Text", toml::Value::Boolean(false));
        let pal = Palette::from_profile(&p, true);
        assert_eq!(pal.ansi[4], rgb(1, 2, 3));
        assert!(!pal.bright_bold);
        assert_eq!(pal.selected_text, None);
        p.set("Use Selected Text Color", toml::Value::Boolean(true));
        assert!(Palette::from_profile(&p, true).selected_text.is_some());
    }

    #[test]
    fn faint_text_uses_the_profile_alpha() {
        let mut p = crate::profile::Profile::default();
        p.set("Faint Text Alpha", toml::Value::Float(0.5));
        let pal = Palette::from_profile(&p, true);
        let (fg, _) = pal.cell(Color::Named(NamedColor::Foreground), Color::Named(NamedColor::Background), Flags::DIM);
        assert_eq!(fg, rgb(pal.fg.r / 2 + pal.bg.r / 2, pal.fg.g / 2 + pal.bg.g / 2, pal.fg.b / 2 + pal.bg.b / 2));
    }
```

Add to `src/profile.rs` tests:

```rust
    #[test]
    fn command_line_follows_custom_command() {
        let mut p = Profile::default();
        assert_eq!(command_line(&p, "pwsh.exe"), "pwsh.exe");
        p.set("Custom Command", toml::Value::String("Yes".into()));
        p.set("Command", toml::Value::String("wsl.exe -d Ubuntu".into()));
        assert_eq!(command_line(&p, "pwsh.exe"), "wsl.exe -d Ubuntu");
    }

    #[test]
    fn working_dir_follows_custom_directory() {
        let mut p = Profile::default();
        assert_eq!(working_dir(&p), None);
        p.set("Custom Directory", toml::Value::String("Yes".into()));
        p.set("Working Directory", toml::Value::String("C:\\Work".into()));
        assert_eq!(working_dir(&p), Some(std::path::PathBuf::from("C:\\Work")));
    }
```

Add to `src/screen.rs` tests:

```rust
    #[test]
    fn look_follows_the_profile() {
        let mut p = crate::profile::Profile::default();
        p.set("Cursor Type", toml::Value::Integer(1));
        p.set("Blinking Cursor", toml::Value::Boolean(true));
        p.set("Scrollback Lines", toml::Value::Integer(5000));
        let look = Look::from_profile(&p);
        assert_eq!((look.cursor_shape, look.cursor_blink, look.scrollback), (CursorShape::Beam, true, 5000));
        p.set("Unlimited Scrollback", toml::Value::Boolean(true));
        assert_eq!(Look::from_profile(&p).scrollback, 100_000);
    }

    #[test]
    fn the_profile_cursor_shape_is_used() {
        let mut p = crate::profile::Profile::default();
        p.set("Cursor Type", toml::Value::Integer(0));
        let s = Screen::with_look(80, 24, Palette::default(), Look::from_profile(&p));
        assert_eq!(s.term().renderable_content().cursor.shape, CursorShape::Underline);
    }
```

Add to `src/scene.rs` tests:

```rust
    #[test]
    fn the_cursor_blinks_off() {
        let s = screen(b"ab");
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, false);
        assert!(!f.quads.iter().any(|q| q.color == rgba(s.palette.cursor, 1.0)));
    }

    #[test]
    fn non_ascii_pieces_use_the_alternate_font() {
        let s = screen("a\u{4e2d}".as_bytes());
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, true);
        let wide = f.rows.iter().flat_map(|r| &r.spans).find(|sp| sp.text == "\u{4e2d}").unwrap();
        assert!(wide.alt_font);
        let a = f.rows.iter().flat_map(|r| &r.spans).find(|sp| sp.text == "a").unwrap();
        assert!(!a.alt_font);
    }
```

(Update every existing `scene::pane(...)` call in the scene tests to pass `true` as the new last argument, and every `Span { .. }` literal to add `alt_font: false`.)

Add to `src/pty.rs` tests:

```rust
    #[test]
    fn the_child_starts_in_the_given_directory() {
        let dir = std::env::temp_dir();
        let p = spawn_notify(&shipped(), "cmd.exe /c cd", Some(&dir), 200, 24, Arc::new(|| {})).unwrap();
        let want = dir.to_string_lossy().trim_end_matches('\\').to_string();
        p.read_until(want.as_bytes(), secs(10)).unwrap_or_else(|got| panic!("{:?}", text(&got)));
    }
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `scripts/xt test --lib`
Expected: compile errors for `from_profile`, `Look`, `with_look`, `command_line`, `working_dir`, the new `pane` and `spawn_notify` arguments, and `alt_font`.

- [ ] **Step 3: Write the code**

`src/palette.rs`: change `selected_text: Rgb` to `selected_text: Option<Rgb>`, add `faint_alpha: f32`, and add:

```rust
impl Palette {
    /// The palette a profile describes. `dark` picks the Dark variants when the
    /// profile keeps separate Light and Dark colors.
    pub fn from_profile(p: &crate::profile::Profile, dark: bool) -> Palette {
        let c = |k: &str| p.color_for(k, dark);
        let mut ansi = [rgb(0, 0, 0); 16];
        for (i, slot) in ansi.iter_mut().enumerate() {
            *slot = c(&format!("Ansi {i} Color"));
        }
        let fg = c("Foreground Color");
        Palette {
            ansi,
            fg,
            bg: c("Background Color"),
            bold: if p.bool("Use Bright Bold") { c("Bold Color") } else { fg },
            cursor: c("Cursor Color"),
            cursor_text: c("Cursor Text Color"),
            selection: c("Selection Color"),
            selected_text: p.bool("Use Selected Text Color").then(|| c("Selected Text Color")),
            bright_bold: p.bool("Brighten Bold Text"),
            faint_alpha: p.float("Faint Text Alpha") as f32,
        }
    }
}
```

Replace `impl Default for Palette` with `Palette::from_profile(&crate::profile::Profile::default(), true)`. In `Palette::cell`, replace the DIM step with a blend toward the background by `faint_alpha`:

```rust
        if flags.contains(Flags::DIM) {
            let bg = self.color(bg);
            let mix = |f: u8, b: u8| (f as f32 * self.faint_alpha + b as f32 * (1.0 - self.faint_alpha)).round() as u8;
            f = rgb(mix(f.r, bg.r), mix(f.g, bg.g), mix(f.b, bg.b));
        }
```

(The named `Dim*` colors keep the 2/3 rule, which is how iTerm2 derives them.)

`src/profile.rs`, add:

```rust
/// The command line a profile starts. "Login Shell" means the Windows default shell.
pub fn command_line(p: &Profile, default_shell: &str) -> String {
    match p.string("Custom Command").as_str() {
        "Yes" | "Custom Shell" if !p.string("Command").trim().is_empty() => p.string("Command"),
        _ => default_shell.to_string(),
    }
}

/// The starting directory, or None for the user's home (iTerm2's default).
pub fn working_dir(p: &Profile) -> Option<std::path::PathBuf> {
    match p.string("Custom Directory").as_str() {
        "Yes" if !p.string("Working Directory").trim().is_empty() => Some(p.string("Working Directory").into()),
        _ => None,
    }
}
```

`src/screen.rs`: add

```rust
use alacritty_terminal::vte::ansi::{CursorShape, CursorStyle};

/// The profile settings the terminal core needs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub cursor_shape: CursorShape,
    pub cursor_blink: bool,
    pub scrollback: usize,
}

impl Look {
    pub fn from_profile(p: &crate::profile::Profile) -> Look {
        let cursor_shape = match p.int("Cursor Type") {
            0 => CursorShape::Underline,
            1 => CursorShape::Beam,
            _ => CursorShape::Block,
        };
        // alacritty_terminal keeps at most 100,000 lines; that is Tandem's "unlimited".
        let scrollback = if p.bool("Unlimited Scrollback") { 100_000 } else { p.int("Scrollback Lines").clamp(0, 100_000) as usize };
        Look { cursor_shape, cursor_blink: p.bool("Blinking Cursor"), scrollback }
    }
}

impl Default for Look {
    fn default() -> Self {
        Look::from_profile(&crate::profile::Profile::default())
    }
}

fn config(look: Look) -> Config {
    Config {
        scrolling_history: look.scrollback,
        default_cursor_style: CursorStyle { shape: look.cursor_shape, blinking: look.cursor_blink },
        ..Config::default()
    }
}
```

Change `Screen::new(cols, rows)` to call `Screen::with_look(cols, rows, Palette::default(), Look::default())`, add `with_look` that builds the `Term` with `config(look)` and stores the palette, and add:

```rust
    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
    }

    pub fn set_look(&mut self, look: Look) {
        self.term.set_options(config(look));
    }
```

`src/scene.rs`: add `pub alt_font: bool` to `Span`; set it to `!ascii` where pieces are built (and `false` in `tab_bar` and `banner`). Add a `cursor_on: bool` parameter to `pane`, and skip the cursor (both the block colors and the overlay shapes) when it is false. Selected text uses `palette.selected_text.unwrap_or(fg)`.

`src/render.rs`: replace the single `family` field with `family: String`, `alt_family: Option<String>`, `h_spacing: f32`, `v_spacing: f32`, and add:

```rust
    /// Sets the fonts and size. Spacing multiplies the cell width and height, as iTerm2's
    /// horizontal and vertical spacing do.
    pub fn set_fonts(&mut self, normal: &str, non_ascii: Option<&str>, px: f32, h_spacing: f32, v_spacing: f32) {
        self.family = normal.to_string();
        self.alt_family = non_ascii.map(str::to_string);
        self.h_spacing = h_spacing;
        self.v_spacing = v_spacing;
        self.set_font_px(px);
    }
```

In `set_font_px`, multiply the measured width by `h_spacing` and the line height by `v_spacing`. In `draw`, a span with `alt_font` uses `alt_family` when it is set. Also compare `alt_font` in the per-row cache (it is part of `Span`, so the `==` already covers it).

`src/pty.rs`: add `cwd: Option<&Path>` after `cmdline` in `spawn_notify` and `start`, and pass it to `CreateProcessW` as `lpCurrentDirectory` (a wide, null-terminated string, or null). `spawn` passes `None`.

`src/pane.rs`: `Pane::start(id, api, cmdline, cwd: Option<&Path>, cols, rows, notify)` passes `cwd` through; add `pub profile: String` (the Guid), set by the app after start.

`src/app.rs`: load the `Store` at startup (`store::default_path()`). On an error, keep a default in-memory store and set the banner to the error. `start_pane` uses the default profile: `command_line`, `working_dir`, `Palette::from_profile`, `Look::from_profile`. It sets `pane.profile` too. The renderer's fonts come from the default profile: `Normal Font`, `Non Ascii Font` when `Use Non-ASCII Font` is on, `Horizontal Spacing`, and `Vertical Spacing`, with px = pt * 4 / 3 * scale. The first window size comes from `Columns` and `Rows`. A 530 ms timer (`ControlFlow::WaitUntil`) flips a blink phase and redraws only when the focused pane's look blinks.

- [ ] **Step 4: Run the tests to see them pass**

Run: `scripts/xt test --lib` and `scripts/xt build --release --bin tandem`
Expected: all tests pass; the build has no warnings.

- [ ] **Step 5: Commit**

```bash
git add -A src
git commit -m "Drive colors, fonts, cursor, scrollback, command, and directory from the profile"
git push
```

---

### Task 6: The settings window

**Files:**
- Create: `src/settings_ui.rs`
- Modify: `Cargo.toml` (add `egui = "0.36"`, `egui-winit = "0.36"`, `egui-wgpu = "0.36"`, `rfd = "0.17"`), `src/lib.rs`, `src/keys.rs`, `src/app.rs`

**Interfaces:**
- Consumes: `schema`, `profile`, `store::Store`, `import::from_file`.
- Produces (in `tandem::settings_ui`):
  - `pub fn sections(tab: &str) -> Vec<(String, Vec<&'static Setting>)>` (settings of a tab grouped by section, in schema order)
  - `pub fn rgb_of(v: &toml::Value) -> [u8; 3]` and `pub fn value_of_rgb(c: [u8; 3]) -> toml::Value`
  - `pub struct SettingsWindow` with `open(event_loop, instance, device, queue) -> Result<Self, String>`, `id() -> WindowId`, `event(&mut self, &WindowEvent) -> bool` (true = redraw), `redraw(&mut self, store: &mut Store) -> Edit`
  - `pub enum Edit { None, Changed(String /* guid */), Imported(usize), Error(String) }`
  - `keys::Action::OpenSettings` on Ctrl+Shift+, (Cmd+,)

- [ ] **Step 1: Write the failing tests**

`src/settings_ui.rs`:

```rust
//! The settings window, laid out like iTerm2's profile editor: a profile list on the
//! left, the eight tabs across the top, and one control per schema setting.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tab_has_sections_in_schema_order() {
        for tab in crate::schema::TABS {
            let s = sections(tab);
            assert!(!s.is_empty(), "{tab}");
            let keys: Vec<&str> = s.iter().flat_map(|(_, v)| v.iter().map(|x| x.key.as_str())).collect();
            let schema: Vec<&str> = crate::schema::profile().iter().filter(|x| x.tab == tab).map(|x| x.key.as_str()).collect();
            let mut sorted = schema.clone();
            sorted.retain(|k| keys.contains(k));
            assert_eq!(keys.len(), schema.len(), "{tab} lost settings");
        }
    }

    #[test]
    fn colors_convert_both_ways() {
        let v = value_of_rgb([255, 128, 0]);
        assert_eq!(rgb_of(&v), [255, 128, 0]);
    }

    #[test]
    fn cmd_comma_opens_settings() {
        use crate::keys::{Action, Key, shortcut};
        use crate::mouse::Mods;
        assert_eq!(shortcut(&Key::Text(",".into()), Mods { ctrl: true, shift: true, alt: false }), Some(Action::OpenSettings));
    }
}
```

Add the four crates to `Cargo.toml`, `#[cfg(windows)] pub mod settings_ui;` to `src/lib.rs`, and `OpenSettings` to `keys::Action` (arm: `(_, ",") if cmd => Action::OpenSettings`).

- [ ] **Step 2: Run the tests to see them fail**

Run: `scripts/xt test --lib settings_ui`
Expected: compile errors (`sections`, `rgb_of` not found).

- [ ] **Step 3: Write the settings window**

Put above the tests in `src/settings_ui.rs`:

```rust
use std::sync::Arc;

use toml::Value;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

use crate::profile::{self, Profile};
use crate::schema::{self, Kind, Setting};
use crate::store::Store;

pub fn sections(tab: &str) -> Vec<(String, Vec<&'static Setting>)> {
    let mut out: Vec<(String, Vec<&'static Setting>)> = Vec::new();
    for s in schema::profile().iter().filter(|s| s.tab == tab) {
        match out.iter_mut().find(|(name, _)| *name == s.section) {
            Some((_, list)) => list.push(s),
            None => out.push((s.section.clone(), vec![s])),
        }
    }
    out
}

pub fn rgb_of(v: &Value) -> [u8; 3] {
    let t = v.as_table();
    let c = |k: &str| {
        let f = t.and_then(|t| t.get(k)).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).unwrap_or(0.0);
        (f.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    [c("r"), c("g"), c("b")]
}

pub fn value_of_rgb(c: [u8; 3]) -> Value {
    profile::color_value(crate::palette::rgb(c[0], c[1], c[2]))
}

pub enum Edit {
    None,
    Changed(String),
    Imported(usize),
    Error(String),
}

pub struct SettingsWindow {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    ctx: egui::Context,
    state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    selected: Option<String>,
    tab: usize,
    window: Arc<Window>,
}

impl SettingsWindow {
    pub fn open(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Self, String> {
        let attrs = Window::default_attributes().with_title("Tandem Settings").with_inner_size(winit::dpi::LogicalSize::new(980.0, 720.0));
        let window = Arc::new(event_loop.create_window(attrs).map_err(|e| e.to_string())?);
        let surface = instance.create_surface(window.clone()).map_err(|e| e.to_string())?;
        let caps = surface.get_capabilities(adapter);
        let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0]);
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(device, &config);
        let ctx = egui::Context::default();
        let state = egui_winit::State::new(ctx.clone(), egui::ViewportId::ROOT, &window, Some(window.scale_factor() as f32), None, None);
        let renderer = egui_wgpu::Renderer::new(device, format, egui_wgpu::RendererOptions::default());
        Ok(SettingsWindow { surface, config, device: device.clone(), queue: queue.clone(), ctx, state, renderer, selected: None, tab: 0, window })
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    /// Feeds a window event to egui. Returns true when the window should redraw.
    pub fn event(&mut self, event: &WindowEvent) -> bool {
        if let WindowEvent::Resized(s) = event {
            if s.width > 0 && s.height > 0 {
                self.config.width = s.width;
                self.config.height = s.height;
                self.surface.configure(&self.device, &self.config);
            }
        }
        let r = self.state.on_window_event(&self.window, event);
        if r.repaint {
            self.window.request_redraw();
        }
        r.repaint
    }

    pub fn redraw(&mut self, store: &mut Store) -> Edit {
        let input = self.state.take_egui_input(&self.window);
        let mut edit = Edit::None;
        let selected = self.selected.get_or_insert_with(|| store.default_guid.clone()).clone();
        let mut tab = self.tab;
        let mut pick = None;
        let out = self.ctx.run_ui(input, |ui| {
            egui::Panel::left("profiles").default_size(220.0).show(ui, |ui| {
                ui.heading("Profiles");
                for p in &store.profiles {
                    let star = if p.guid() == store.default_guid { " (default)" } else { "" };
                    if ui.selectable_label(p.guid() == selected, format!("{}{star}", p.name())).clicked() {
                        pick = Some(p.guid());
                    }
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("+").on_hover_text("New profile").clicked() {
                        let p = Profile::new("New Profile");
                        pick = Some(p.guid());
                        store.add(p);
                        edit = Edit::Changed(String::new());
                    }
                    if ui.button("Duplicate").clicked() {
                        pick = store.duplicate(&selected);
                        edit = Edit::Changed(String::new());
                    }
                    if ui.button("-").on_hover_text("Delete profile").clicked() && store.delete(&selected) {
                        pick = Some(store.default_guid.clone());
                        edit = Edit::Changed(String::new());
                    }
                });
                if ui.button("Set as Default").clicked() {
                    store.default_guid = selected.clone();
                    edit = Edit::Changed(selected.clone());
                }
                if ui.button("Import from iTerm2...").clicked() {
                    if let Some(path) = rfd::FileDialog::new().add_filter("iTerm2 settings or Dynamic Profiles", &["plist", "json"]).pick_file() {
                        edit = match crate::import::from_file(&path) {
                            Ok(list) => {
                                let n = list.len();
                                for p in list {
                                    store.profiles.retain(|q| q.guid() != p.guid());
                                    store.add(p);
                                }
                                Edit::Imported(n)
                            }
                            Err(e) => Edit::Error(e),
                        };
                    }
                }
            });
            egui::CentralPanel::default().show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (i, name) in schema::TABS.iter().enumerate() {
                        if ui.selectable_label(tab == i, *name).clicked() {
                            tab = i;
                        }
                    }
                });
                ui.separator();
                let Some(profile) = store.get_mut(&selected) else { return };
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (section, list) in sections(schema::TABS[tab]) {
                        if !section.is_empty() {
                            ui.add_space(8.0);
                            ui.strong(section);
                        }
                        for s in list {
                            if control(ui, profile, s) {
                                edit = Edit::Changed(selected.clone());
                            }
                        }
                    }
                });
            });
        });
        self.tab = tab;
        if let Some(g) = pick {
            self.selected = Some(g);
        }
        self.state.handle_platform_output(&self.window, out.platform_output);
        let jobs = self.ctx.tessellate(out.shapes, out.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: [self.config.width, self.config.height], pixels_per_point: out.pixels_per_point };
        for (id, delta) in &out.textures_delta.set {
            self.renderer.update_texture(&self.device, &self.queue, *id, delta);
        }
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) => f,
            _ => {
                self.surface.configure(&self.device, &self.config);
                self.window.request_redraw();
                return edit;
            }
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("settings") });
        let extra = self.renderer.update_buffers(&self.device, &self.queue, &mut encoder, &jobs, &screen);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("settings"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut pass = pass.forget_lifetime();
            self.renderer.render(&mut pass, &jobs, &screen);
        }
        self.queue.submit(extra.into_iter().chain(Some(encoder.finish())));
        self.queue.present(frame);
        for id in &out.textures_delta.free {
            self.renderer.free_texture(id);
        }
        edit
    }
}

/// One control for one setting. Returns true when the user changed it.
fn control(ui: &mut egui::Ui, profile: &mut Profile, s: &Setting) -> bool {
    let current = profile.get(&s.key);
    let mut changed = None;
    ui.horizontal(|ui| {
        ui.add_enabled_ui(!s.macos_only, |ui| {
            match s.kind {
                Kind::Bool => {
                    let mut b = current.as_ref().and_then(|v| v.as_bool()).unwrap_or(false);
                    if ui.checkbox(&mut b, &s.label).changed() {
                        changed = Some(Value::Boolean(b));
                    }
                }
                Kind::Int => {
                    ui.label(&s.label);
                    let mut i = current.as_ref().and_then(|v| v.as_integer()).unwrap_or(0);
                    let mut w = egui::DragValue::new(&mut i);
                    if let (Some(a), Some(b)) = (s.min_f64(), s.max_f64()) {
                        w = w.range(a as i64..=b as i64);
                    }
                    if ui.add(w).changed() {
                        changed = Some(Value::Integer(i));
                    }
                }
                Kind::Float => {
                    ui.label(&s.label);
                    let mut f = current.as_ref().and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).unwrap_or(0.0);
                    let r = match (s.min_f64(), s.max_f64()) {
                        (Some(a), Some(b)) => ui.add(egui::Slider::new(&mut f, a..=b)),
                        _ => ui.add(egui::DragValue::new(&mut f).speed(0.01)),
                    };
                    if r.changed() {
                        changed = Some(Value::Float(f));
                    }
                }
                Kind::String | Kind::Font => {
                    ui.label(&s.label);
                    let mut text = current.as_ref().and_then(|v| v.as_str()).unwrap_or("").to_string();
                    if ui.text_edit_singleline(&mut text).changed() {
                        changed = Some(Value::String(text));
                    }
                }
                Kind::Color => {
                    ui.label(&s.label);
                    let mut c = current.as_ref().map(rgb_of).unwrap_or([0, 0, 0]);
                    if ui.color_edit_button_srgb(&mut c).changed() {
                        changed = Some(value_of_rgb(c));
                    }
                }
                Kind::Enum => {
                    ui.label(&s.label);
                    let now = current.clone();
                    let shown = s.choices.iter().find(|c| Some(&c.value) == now.as_ref()).map_or("".to_string(), |c| c.label.clone());
                    egui::ComboBox::from_id_salt(&s.key).selected_text(shown).show_ui(ui, |ui| {
                        for c in &s.choices {
                            if ui.selectable_label(Some(&c.value) == now.as_ref(), &c.label).clicked() {
                                changed = Some(c.value.clone());
                            }
                        }
                    });
                }
                Kind::List | Kind::Dict | Kind::Data => {
                    let n = match &current {
                        Some(Value::Array(a)) => a.len(),
                        Some(Value::Table(t)) => t.len(),
                        _ => 0,
                    };
                    ui.label(format!("{}: {n} item(s)", s.label)).on_hover_text("This editor arrives with its feature's milestone.");
                }
            }
        })
        .response
        .on_hover_text(if s.macos_only { format!("{} (macOS only, no Windows equivalent)", s.help) } else { s.help.clone() });
    });
    match changed {
        Some(v) => {
            profile.set(&s.key, v);
            true
        }
        None => false,
    }
}
```

`src/app.rs`: keep the wgpu `Adapter` in `Gpu`. Add `settings: Option<SettingsWindow>` and the `Store` to `App`. `Action::OpenSettings` opens the window (or focuses it). `window_event` routes events for the settings window's id to it; a `RedrawRequested` there calls `redraw(&mut self.store)`. On `Edit::Changed` or `Edit::Imported`: `store.save()` (errors go to the banner), then for every pane whose `profile` matches, apply `Palette::from_profile` and `Look::from_profile`; if the default profile changed, update the renderer fonts and relayout. `Edit::Error` goes to the banner. Closing the settings window drops it.

- [ ] **Step 4: Run the tests to see them pass**

Run: `scripts/xt test --lib` and `scripts/xt build --release --bin tandem`
Expected: all pass, clean build. Fix API differences against egui 0.36 as rulings.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/settings_ui.rs src/lib.rs src/keys.rs src/app.rs
git commit -m "Add the settings window with a control for every profile setting"
git push
```

---

### Task 7: Profiles window, new tab with a profile, and parity status

**Files:**
- Modify: `src/keys.rs`, `src/settings_ui.rs`, `src/app.rs`, `docs/parity/01-profile-settings.md`, `docs/parity/00-roadmap.md`

**Interfaces:**
- Produces: `keys::Action::OpenProfiles` on Ctrl+Shift+O (Cmd+O). `SettingsWindow` gains a `Mode { Settings, Profiles }`; in `Profiles` mode it shows only the list with "New Tab", "New Window", "Split Right", and "Split Down" buttons, returned as `Edit::Open(guid, How)`. `App::start_pane` takes the profile Guid.

- [ ] **Step 1: Write the failing test**

Add to `src/keys.rs` tests:

```rust
    #[test]
    fn cmd_o_opens_profiles() {
        assert_eq!(shortcut(&t("o"), CMD), Some(Action::OpenProfiles));
    }
```

- [ ] **Step 2: Run it to see it fail**

Run: `scripts/xt test --lib keys`
Expected: compile error (`OpenProfiles` not found).

- [ ] **Step 3: Write the code**

Add `OpenProfiles` to `Action` with the arm `(_, "o") if cmd => Action::OpenProfiles`. Add the mode and the `Edit::Open(String, How)` variant, where `pub enum How { Tab, Window, SplitRight, SplitDown }`. In the app, `How::Window` opens a new tab for milestone 2, and the ruling notes that separate windows arrive in milestone 7. `start_pane(rect, guid)` looks the profile up in the store, falling back to the default profile when the Guid is gone.

- [ ] **Step 4: Update the parity inventory**

For every row in `docs/parity/01-profile-settings.md` whose key the schema holds:
- `have` if it persists and takes effect (colors, fonts, spacing, cursor type and blink, scrollback, columns and rows for the first window, command, working directory, name, faint text alpha, bright bold, selected text color).
- `partial` if it persists and shows in the settings window but its effect comes later (note the milestone number in the Windows note column).
Recount the totals at the bottom of the file and in `docs/parity/00-roadmap.md`.

- [ ] **Step 5: Run everything and commit**

Run: `scripts/xt test --lib` and `scripts/xt build --release --bin tandem`

```bash
git add -A src docs/parity
git commit -m "Add the profiles window and open tabs and splits with any profile"
git push
```

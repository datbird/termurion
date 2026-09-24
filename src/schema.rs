//! The iTerm2 profile schema: every profile key with its type, default, label, and tab.
//! The data lives in schema/profile.toml, built from iTerm2's source.

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
    /// iTerm2's values for a missing "(Light)" or "(Dark)" variant key.
    pub default_light: Option<toml::Value>,
    pub default_dark: Option<toml::Value>,
    /// What a new profile starts with, when it differs from `default` (iTerm2's
    /// DefaultBookmark.plist). `default` is what iTerm2 reads for a missing key.
    pub seed: Option<toml::Value>,
    pub seed_light: Option<toml::Value>,
    pub seed_dark: Option<toml::Value>,
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

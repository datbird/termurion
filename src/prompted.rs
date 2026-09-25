//! The OSC 1337 keys that ask first: what each request means, whether the stored answers
//! allow, deny, or ask, and the texts of iTerm2's bars.

use toml::Value;

use crate::schema::{self, Kind};

/// What to do with a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Apply,
    Deny,
    Ask,
}

/// A JSON value from SetProfileProperty as a profile value, if it is legal for the key:
/// the key must be a profile setting and the value of its type (colors as iTerm2's
/// component dictionaries, enums one of their values).
pub fn valid(key: &str, json: &serde_json::Value) -> Option<Value> {
    use serde_json::Value as J;
    if key == crate::profile::GUID || key == crate::session_profile::ORIGINAL_GUID {
        return None;
    }
    let s = schema::find(key)?;
    Some(match (&s.kind, json) {
        (Kind::Bool, J::Bool(b)) => Value::Boolean(*b),
        (Kind::Int, J::Number(n)) => Value::Integer(n.as_i64()?),
        (Kind::Float, J::Number(n)) => Value::Float(n.as_f64()?),
        (Kind::String | Kind::Font, J::String(t)) => Value::String(t.clone()),
        (Kind::Enum, J::Number(n)) => {
            let v = Value::Integer(n.as_i64()?);
            if !s.choices.is_empty() && !s.choices.iter().any(|c| c.value == v) {
                return None;
            }
            v
        }
        (Kind::Enum, J::String(t)) => {
            let v = Value::String(t.clone());
            if !s.choices.is_empty() && !s.choices.iter().any(|c| c.value == v) {
                return None;
            }
            v
        }
        (Kind::Color, J::Object(o)) => {
            let part = |name: &str, default: Option<f64>| -> Option<f64> {
                let v = match o.get(name) {
                    Some(v) => v.as_f64()?,
                    None => default?,
                };
                (0.0..=1.0).contains(&v).then_some(v)
            };
            let mut t = toml::Table::new();
            t.insert("r".into(), Value::Float(part("Red Component", None)?));
            t.insert("g".into(), Value::Float(part("Green Component", None)?));
            t.insert("b".into(), Value::Float(part("Blue Component", None)?));
            t.insert("a".into(), Value::Float(part("Alpha Component", Some(1.0))?));
            Value::Table(t)
        }
        _ => return None,
    })
}

/// SetProfileProperty's permission from each key's stored answer
/// (`NoSyncSetProfileProperty_<key>`: 0 always allow, 1 always deny).
pub fn property_decision(keys: &[String], stored: impl Fn(&str) -> Option<i64>) -> Decision {
    let answers: Vec<Option<i64>> = keys.iter().map(|k| stored(k)).collect();
    if answers.contains(&Some(1)) {
        Decision::Deny
    } else if answers.iter().all(|a| *a == Some(0)) {
        Decision::Apply
    } else {
        Decision::Ask
    }
}

/// SetProfileProperty's bar text.
pub fn property_text(keys: &[String]) -> String {
    let name = |k: &String| schema::find(k).map_or_else(|| k.clone(), |s| s.label.clone());
    match keys {
        [one] => format!("An app tried to change the profile property {}.", name(one)),
        many => format!("An app tried to change multiple profile properties: {}.", many.iter().map(name).collect::<Vec<_>>().join(", ")),
    }
}

/// SetBackgroundImageFile's permission from the exact-path lists (deny wins).
pub fn background_decision(path: &str, allow: &[String], deny: &[String]) -> Decision {
    if deny.iter().any(|p| p == path) {
        Decision::Deny
    } else if allow.iter().any(|p| p == path) {
        Decision::Apply
    } else {
        Decision::Ask
    }
}

/// SetBackgroundImageFile's bar text.
pub fn background_text(path: &str) -> String {
    if path.is_empty() {
        "Remove background image?".to_string()
    } else {
        format!("Set background image to \u{201c}{path}\u{201d}?")
    }
}

/// SetProfile's permission from `PreventEscapeSequenceFromChangingProfile` (unset asks,
/// and the request that asked is dropped).
pub fn set_profile_decision(prevent: Option<bool>) -> Decision {
    match prevent {
        None => Decision::Ask,
        Some(true) => Decision::Deny,
        Some(false) => Decision::Apply,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn values_follow_schema_types() {
        assert_eq!(valid("Blend", &json!(0.3)), Some(Value::Float(0.3)));
        assert_eq!(valid("Blend", &json!(1)), Some(Value::Float(1.0)), "an integer is a number too");
        assert_eq!(valid("Blend", &json!("x")), None);
        assert_eq!(valid("Use Cursor Guide", &json!(true)), Some(Value::Boolean(true)));
        assert_eq!(valid("Use Cursor Guide", &json!(1)), None);
        assert_eq!(valid("Cursor Type", &json!(1)), Some(Value::Integer(1)));
        assert_eq!(valid("Cursor Type", &json!(7)), None, "not one of the enum's values");
        assert_eq!(valid("Scrollback Lines", &json!(500)), Some(Value::Integer(500)));
        assert_eq!(valid("Scrollback Lines", &json!(1.5)), None);
        assert_eq!(valid("Name", &json!("x")), Some(Value::String("x".into())));
        let c = valid("Ansi 1 Color", &json!({"Red Component": 1.0, "Green Component": 0.5, "Blue Component": 0, "Color Space": "sRGB"})).unwrap();
        let t = c.as_table().unwrap();
        assert_eq!((t["r"].as_float(), t["g"].as_float(), t["b"].as_float(), t["a"].as_float()), (Some(1.0), Some(0.5), Some(0.0), Some(1.0)));
        assert_eq!(valid("Ansi 1 Color", &json!({"Red Component": 2.0, "Green Component": 0, "Blue Component": 0})), None, "out of range");
        assert_eq!(valid("No Such Key", &json!(1)), None);
        assert_eq!(valid("Guid", &json!("x")), None, "a profile's identity is not a property");
    }

    #[test]
    fn one_illegal_key_rejects_all() {
        let pairs = [("Blend", json!(0.2)), ("Cursor Type", json!(9))];
        let checked: Option<Vec<Value>> = pairs.iter().map(|(k, v)| valid(k, v)).collect();
        assert!(checked.is_none());
    }

    #[test]
    fn property_permissions() {
        let keys = vec!["Blend".to_string(), "Cursor Type".to_string()];
        assert_eq!(property_decision(&keys, |_| None), Decision::Ask);
        assert_eq!(property_decision(&keys, |_| Some(0)), Decision::Apply);
        assert_eq!(property_decision(&keys, |k| if k == "Blend" { Some(0) } else { None }), Decision::Ask);
        assert_eq!(property_decision(&keys, |k| if k == "Blend" { Some(0) } else { Some(1) }), Decision::Deny, "any deny drops it all");
        assert_eq!(property_text(&keys[..1]), "An app tried to change the profile property Blending.");
        assert_eq!(property_text(&keys), "An app tried to change multiple profile properties: Blending, Cursor style.");
    }

    #[test]
    fn set_profile_needs_permission() {
        assert_eq!(set_profile_decision(None), Decision::Ask);
        assert_eq!(set_profile_decision(Some(false)), Decision::Apply);
        assert_eq!(set_profile_decision(Some(true)), Decision::Deny);
    }

    #[test]
    fn background_image_file_lists() {
        let allow = vec!["C:\\a.png".to_string(), String::new()];
        let deny = vec!["C:\\a.png".to_string(), "C:\\b.png".to_string()];
        assert_eq!(background_decision("C:\\a.png", &allow, &deny), Decision::Deny, "deny wins");
        assert_eq!(background_decision("", &allow, &[]), Decision::Apply, "removing can be always allowed");
        assert_eq!(background_decision("C:\\c.png", &allow, &deny), Decision::Ask);
        assert_eq!(background_text("C:\\c.png"), "Set background image to \u{201c}C:\\c.png\u{201d}?");
        assert_eq!(background_text(""), "Remove background image?");
    }
}

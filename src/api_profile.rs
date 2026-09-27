//! The Python API's profile requests (GetProfileProperty `PS:22595-22615`, SetProfileProperty
//! `H:2245-2352`, ListProfiles `H:3383-3426`) and Preferences (`H:3802-3881`): the keys a
//! profile answers with, the JSON iTerm2 writes each value as, the assignments a request
//! makes and which of them are legal, so the app only reads and changes its store.

use serde_json::{json, Map, Value as Json};
use toml::Value;

use crate::api_proto::pb;
use crate::profile::{Profile, GUID};
use crate::schema::{self, Kind, Setting};
use crate::session_profile::ORIGINAL_GUID;
use pb::preferences_response::result::set_preference_result::Status as PrefStatus;

/// The suffixes of a color setting's light and dark variants, which iTerm2 lists as keys of
/// their own (`defaultValueMap`).
const VARIANTS: [&str; 2] = [" (Light)", " (Dark)"];

/// iTerm2's keys without a default (`keysWithoutDefaultValues`) that Tandem's schema does not
/// list; they answer only when the profile has them.
const NO_DEFAULT: [&str; 8] = [
    GUID,
    ORIGINAL_GUID,
    "Special Font Config",
    "Dynamic Profile Parent Name",
    "Dynamic Profile Parent GUID",
    "Dynamic Profile Filename",
    "Rewritable",
    "Is Dynamic Profile",
];

/// The setting a key belongs to, and for a light or dark variant its default.
fn setting_of(key: &str) -> Option<(&'static Setting, Option<Value>)> {
    if let Some(s) = schema::find(key) {
        return Some((s, s.default.clone()));
    }
    let (i, base) = VARIANTS.iter().enumerate().find_map(|(i, v)| Some((i, key.strip_suffix(v)?)))?;
    let s = schema::find(base).filter(|s| s.light_dark)?;
    let stock = if i == 0 { &s.default_light } else { &s.default_dark };
    Some((s, stock.clone().or_else(|| s.default.clone())))
}

/// Every key iTerm2 answers with when none are asked (`allKeys`): the schema's, each light
/// and dark variant after its setting, then the keys without defaults.
fn all_keys() -> Vec<String> {
    let mut keys = Vec::new();
    for s in schema::profile() {
        keys.push(s.key.clone());
        if s.light_dark {
            keys.extend(VARIANTS.iter().map(|v| format!("{}{v}", s.key)));
        }
    }
    keys.extend(NO_DEFAULT.iter().map(|k| k.to_string()));
    keys
}

/// The profile's own value, else the key's default (`objectForKey:inProfile:`).
fn value_of(p: &Profile, key: &str) -> Option<Value> {
    p.values.get(key).cloned().or_else(|| setting_of(key).and_then(|(_, d)| d))
}

/// A number as NSJSONSerialization writes it: whole values without a fraction.
fn number(f: f64) -> Option<Json> {
    if !f.is_finite() {
        return None;
    }
    Some(if f.fract() == 0.0 && f.abs() < 9.0e15 { json!(f as i64) } else { json!(f) })
}

/// A value as JSON; None for a number JSON cannot hold.
fn to_json(v: &Value) -> Option<Json> {
    Some(match v {
        Value::Boolean(b) => Json::Bool(*b),
        Value::Integer(i) => json!(*i),
        Value::Float(f) => number(*f)?,
        Value::String(s) => Json::String(s.clone()),
        Value::Datetime(d) => Json::String(d.to_string()),
        Value::Array(a) => Json::Array(a.iter().map(to_json).collect::<Option<_>>()?),
        Value::Table(t) => Json::Object(t.iter().map(|(k, v)| Some((k.clone(), to_json(v)?))).collect::<Option<Map<_, _>>>()?),
    })
}

/// A color as iTerm2 stores it (`srgbDictionaryValue`).
fn color_json(v: &Value) -> Option<Json> {
    let t = v.as_table()?;
    let c = |k: &str| t.get(k).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)));
    Some(json!({
        "Color Space": "sRGB",
        "Red Component": number(c("r")?)?,
        "Green Component": number(c("g")?)?,
        "Blue Component": number(c("b")?)?,
        "Alpha Component": number(c("a").unwrap_or(1.0))?,
    }))
}

/// JSON text as `it_jsonStringForObject:` writes it: a top-level boolean is its NSNumber's
/// `stringValue` ("1" or "0"), and slashes are escaped as NSJSONSerialization escapes them.
fn json_text(j: &Json) -> String {
    match j {
        Json::Bool(b) => if *b { "1" } else { "0" }.to_string(),
        j => j.to_string().replace('/', "\\/"),
    }
}

/// A profile value as the JSON iTerm2 answers with (`jsonEncodedValueForKey:inProfile:`):
/// colors as their component dictionaries, other values as they are.
pub fn json_of(key: &str, v: &Value) -> Option<String> {
    let j = match setting_of(key) {
        Some((s, _)) if s.kind == Kind::Color => color_json(v).or_else(|| to_json(v))?,
        _ => to_json(v)?,
    };
    Some(json_text(&j))
}

/// The properties a GetProfileProperty or ListProfiles answer carries: the keys asked (all
/// of them when none are), each that has a value (`PS:22595-22615`, `H:3405-3421`).
pub fn properties(p: &Profile, keys: &[String]) -> Vec<pb::ProfileProperty> {
    let all;
    let keys = if keys.is_empty() {
        all = all_keys();
        &all
    } else {
        keys
    };
    keys.iter()
        .filter_map(|k| {
            let json = json_of(k, &value_of(p, k)?)?;
            Some(pb::ProfileProperty { key: Some(k.clone()), json_value: Some(json) })
        })
        .collect()
}

/// A SetProfileProperty request's assignments with their JSON parsed (fragments allowed):
/// the deprecated key and value when both are present, else the assignments (`H:2310-2318`);
/// None when any value does not parse (`H:2320-2341`).
pub fn assignments(r: &pb::SetProfilePropertyRequest) -> Option<Vec<(String, Json)>> {
    let pairs: Vec<(String, &str)> = match (&r.key, &r.json_value) {
        (Some(k), Some(j)) => vec![(k.clone(), j.as_str())],
        _ => r.assignments.iter().map(|a| (a.key.clone().unwrap_or_default(), a.json_value.as_deref().unwrap_or_default())).collect(),
    };
    pairs.into_iter().map(|(k, j)| Some((k, serde_json::from_str(j).ok()?))).collect()
}

/// The assignments as profile values when every one is legal for its key (`valueIsLegal`,
/// Tandem's `prompted::valid`: a profile setting of the right type, never the Guid).
pub fn legal(a: &[(String, Json)]) -> Option<Vec<(String, Value)>> {
    a.iter().map(|(k, j)| Some((k.clone(), crate::prompted::valid(k, j)?))).collect()
}

/// ListProfiles' answer (`H:3383-3426`): every profile, only those with one of `guids` when
/// any are given, each with the `properties` asked (all when none are).
pub fn list_profiles(profiles: &[Profile], properties: &[String], guids: &[String]) -> pb::ListProfilesResponse {
    let profiles = profiles
        .iter()
        .filter(|p| guids.is_empty() || guids.contains(&p.guid()))
        .map(|p| pb::list_profiles_response::Profile { properties: self::properties(p, properties) })
        .collect();
    pb::ListProfilesResponse { profiles }
}

/// A GetPreference answer's JSON (`H:3839-3845`): "null" when the key has no value, as
/// `it_jsonStringForObject:` writes nil; None for a number JSON cannot hold.
pub fn preference_json(v: Option<&Value>) -> Option<String> {
    match v {
        None => Some("null".into()),
        Some(v) => Some(json_text(&to_json(v)?)),
    }
}

/// True when a JSON null is anywhere in `j` (not plist-safe, `it_isSafeForPlist`).
fn has_null(j: &Json) -> bool {
    match j {
        Json::Null => true,
        Json::Array(a) => a.iter().any(has_null),
        Json::Object(o) => o.values().any(has_null),
        _ => false,
    }
}

/// A SetPreference value (`H:3847-3864`): BAD_JSON when it does not parse (fragments
/// allowed), INVALID_VALUE when a null is anywhere in it.
pub fn preference_value(json: &str) -> Result<Value, PrefStatus> {
    let j: Json = serde_json::from_str(json).map_err(|_| PrefStatus::BadJson)?;
    if has_null(&j) {
        return Err(PrefStatus::InvalidValue);
    }
    crate::import::from_json(&j).ok_or(PrefStatus::InvalidValue)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema;
    use pb::preferences_response::result::set_preference_result::Status as PStatus;
    use pb::set_profile_property_request::Assignment;
    use serde_json::json;

    fn table(pairs: &[(&str, Value)]) -> Value {
        Value::Table(pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
    }

    fn parsed(s: &Option<String>) -> serde_json::Value {
        serde_json::from_str(s.as_deref().expect("some JSON")).expect("valid JSON")
    }

    fn keys(props: &[pb::ProfileProperty]) -> Vec<String> {
        props.iter().map(|p| p.key.clone().unwrap_or_default()).collect()
    }

    fn named(name: &str, guid: &str) -> Profile {
        let mut p = Profile::stock();
        p.set("Name", Value::String(name.into()));
        p.set("Guid", Value::String(guid.into()));
        p
    }

    fn assign(key: &str, json: &str) -> Assignment {
        Assignment { key: Some(key.into()), json_value: Some(json.into()) }
    }

    #[test]
    fn profile_json_follows_iterm2() {
        let fg = table(&[("r", Value::Float(1.0)), ("g", Value::Float(0.5)), ("b", Value::Float(0.0)), ("a", Value::Float(1.0))]);
        let j = parsed(&json_of("Foreground Color", &fg));
        let o = j.as_object().expect("a color is a dictionary");
        let comp = |k: &str| o.get(k).and_then(|v| v.as_f64());
        assert_eq!(comp("Red Component"), Some(1.0));
        assert_eq!(comp("Green Component"), Some(0.5));
        assert_eq!(comp("Blue Component"), Some(0.0));
        assert_eq!(comp("Alpha Component"), Some(1.0));
        assert_eq!(o.get("Color Space"), Some(&json!("sRGB")));
        assert_eq!(o.len(), 5);
        // A color variant is a color too.
        assert!(parsed(&json_of("Foreground Color (Dark)", &fg)).get("Red Component").is_some());
        assert_eq!(json_of("Columns", &Value::Integer(80)).as_deref(), Some("80"));
        assert_eq!(json_of("Name", &Value::String("x".into())).as_deref(), Some("\"x\""));
        // A top-level boolean is an NSNumber's stringValue (`it_jsonStringForObject`).
        assert_eq!(json_of("Blinking Cursor", &Value::Boolean(true)).as_deref(), Some("1"));
        assert_eq!(json_of("Blinking Cursor", &Value::Boolean(false)).as_deref(), Some("0"));
        // Whole numbers lose their fraction, slashes are escaped, as NSJSONSerialization writes.
        assert_eq!(json_of("Transparency", &Value::Float(1.0)).as_deref(), Some("1"));
        assert_eq!(json_of("Transparency", &Value::Float(0.25)).as_deref(), Some("0.25"));
        assert_eq!(json_of("Working Directory", &Value::String("C:/x".into())).as_deref(), Some("\"C:\\/x\""));
        let nested = table(&[("a", table(&[("b", Value::Integer(1)), ("c", Value::Boolean(true))]))]);
        assert_eq!(parsed(&json_of("Keyboard Map", &nested)), json!({"a": {"b": 1, "c": true}}));

        // No keys asked: every key that has a value, in schema order, each light and dark
        // variant after its key, then the keys without defaults (the Guid).
        let stock = Profile::stock();
        let all = properties(&stock, &[]);
        let mut want = Vec::new();
        for s in schema::profile() {
            if stock.get(&s.key).is_some() {
                want.push(s.key.clone());
            }
            if s.light_dark {
                for (suffix, stock_default) in [(" (Light)", &s.default_light), (" (Dark)", &s.default_dark)] {
                    let key = format!("{}{suffix}", s.key);
                    if stock.values.contains_key(&key) || stock_default.is_some() || s.default.is_some() {
                        want.push(key);
                    }
                }
            }
        }
        let got = keys(&all);
        let schema_part: Vec<String> = got.iter().filter(|k| want.contains(k)).cloned().collect();
        assert_eq!(schema_part, want);
        assert!(!got.contains(&"Guid".to_string()), "a stock profile has no Guid");
        let with_guid = properties(&named("A", "GA"), &[]);
        assert_eq!(with_guid.last().map(|p| (p.key.clone(), p.json_value.clone())), Some((Some("Guid".into()), Some("\"GA\"".into()))));
        assert!(all.iter().all(|p| p.json_value.is_some()));

        let mut p = Profile::stock();
        p.set("Columns", Value::Integer(80));
        let some = properties(&p, &["Columns".into(), "Nope".into()]);
        assert_eq!(some, vec![pb::ProfileProperty { key: Some("Columns".into()), json_value: Some("80".into()) }]);
    }

    #[test]
    fn set_profile_follows_iterm2() {
        // The deprecated single pair wins when both halves are present (`H:2310-2318`).
        let old = pb::SetProfilePropertyRequest { key: Some("Columns".into()), json_value: Some("100".into()), assignments: vec![assign("Rows", "5")], ..Default::default() };
        assert_eq!(assignments(&old), Some(vec![("Columns".into(), json!(100))]));
        let two = pb::SetProfilePropertyRequest { assignments: vec![assign("Columns", "100"), assign("Name", "\"x\"")], ..Default::default() };
        assert_eq!(assignments(&two), Some(vec![("Columns".into(), json!(100)), ("Name".into(), json!("x"))]));
        // Only the key: the assignments are used.
        let half = pb::SetProfilePropertyRequest { key: Some("Columns".into()), assignments: vec![assign("Rows", "5")], ..Default::default() };
        assert_eq!(assignments(&half), Some(vec![("Rows".into(), json!(5))]));
        let bad = pb::SetProfilePropertyRequest { assignments: vec![assign("Columns", "100"), assign("Rows", "{bad")], ..Default::default() };
        assert_eq!(assignments(&bad), None);
        assert_eq!(assignments(&pb::SetProfilePropertyRequest::default()), Some(vec![]));

        assert_eq!(legal(&[("Columns".into(), json!("x"))]), None);
        assert_eq!(legal(&[("Guid".into(), json!("g"))]), None);
        assert_eq!(legal(&[("Nope".into(), json!(1))]), None);
        assert_eq!(legal(&[("Columns".into(), json!(100))]), Some(vec![("Columns".into(), Value::Integer(100))]));
        assert_eq!(legal(&[("Columns".into(), json!(100)), ("Rows".into(), json!(null))]), None);
    }

    #[test]
    fn list_profiles_follows_iterm2() {
        let list = [named("A", "GA"), named("B", "GB")];
        let all = list_profiles(&list, &[], &[]);
        assert_eq!(all.profiles.len(), 2);
        assert_eq!(all.profiles[0].properties, properties(&list[0], &[]));
        assert_eq!(all.profiles[1].properties, properties(&list[1], &[]));
        let b = list_profiles(&list, &[], &["GB".into()]);
        assert_eq!(b.profiles.len(), 1);
        assert_eq!(b.profiles[0].properties, properties(&list[1], &[]));
        assert!(list_profiles(&list, &[], &["Z".into()]).profiles.is_empty());
        let names = list_profiles(&list, &["Name".into()], &[]);
        let got: Vec<_> = names.profiles.iter().map(|p| p.properties.clone()).collect();
        assert_eq!(
            got,
            vec![
                vec![pb::ProfileProperty { key: Some("Name".into()), json_value: Some("\"A\"".into()) }],
                vec![pb::ProfileProperty { key: Some("Name".into()), json_value: Some("\"B\"".into()) }],
            ]
        );
    }

    #[test]
    fn preferences_follow_iterm2() {
        // No value is JSON null, not an unset field (`it_jsonStringForObject:` of nil).
        assert_eq!(preference_json(None).as_deref(), Some("null"));
        assert_eq!(preference_json(Some(&Value::Integer(3))).as_deref(), Some("3"));
        assert_eq!(preference_json(Some(&Value::Boolean(true))).as_deref(), Some("1"));
        assert_eq!(preference_json(Some(&Value::String("a/b".into()))).as_deref(), Some("\"a\\/b\""));
        assert_eq!(preference_value("{bad"), Err(PStatus::BadJson));
        assert_eq!(preference_value(""), Err(PStatus::BadJson));
        assert_eq!(preference_value("null"), Err(PStatus::InvalidValue));
        assert_eq!(preference_value("[1, null]"), Err(PStatus::InvalidValue));
        assert_eq!(preference_value("{\"a\": null}"), Err(PStatus::InvalidValue));
        assert_eq!(preference_value("true"), Ok(Value::Boolean(true)));
        assert_eq!(preference_value("[1, \"x\"]"), Ok(Value::Array(vec![Value::Integer(1), Value::String("x".into())])));
    }
}

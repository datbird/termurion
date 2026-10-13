//! Expression bindings on color wells and checkboxes (`PP:245-290`, `PP:512-561`,
//! `PBV:351-387`, `EBV`): the profile's "Bindings" dictionary, the value conversions and the
//! dialog's texts. Pure; the evaluation per session is `app/bindings_ui.rs`.

use std::collections::BTreeMap;

use crate::profile::Profile;

/// The profile key (`KEY_BINDINGS`).
pub const KEY: &str = "Bindings";
/// A profile reads at most this many bindings (an untrusted profile can carry any number).
pub const MAX_BINDINGS: usize = 32;
/// An expression longer than this is not read.
pub const MAX_EXPRESSION: usize = 1024;

pub const MENU_BIND: &str = "Bind to Expression";
pub const MENU_EDIT: &str = "Edit Expression Binding";
pub const MENU_REMOVE: &str = "Remove Expression Binding";
pub const TITLE: &str = "Bind Expression to Setting";
pub const INFO: &str = "Enter expression to bind to this setting, or leave empty to clear the binding.";
pub const COLOR_EXAMPLE: &str = "bgColor";
pub const BOOL_EXAMPLE: &str = "user.mySetting";
pub const COLOR_HELP: &str = "Colors can be specified as 3- or 6-digit hex strings such as \"#f8a\" like in HTML. By default it uses the sRGB color space. To use P3 instead, place p3 before the # as in: \"p3#f8a\". While you can use a string literal here, typically this would come from a string-valued variable or function.";
pub const BOOL_HELP: &str = "Checkboxes accept numbers (0 is false and any other number is true), strings containing a number, or strings like \"true\" and \"false\". While you can use a numeric literal here, typically this would come from a number-valued variable or function.";

/// What a "Bindings" entry holds (`PP:245-290`): a plain expression the person wrote, or a
/// dictionary `{expression, owner = "palette"}` an escape code's palette binding wrote.
#[derive(Clone, Debug, PartialEq)]
pub enum BindingValue {
    Plain(String),
    Palette(String),
}

impl BindingValue {
    pub fn from_value(v: &toml::Value) -> Option<BindingValue> {
        match v {
            toml::Value::String(s) => Some(BindingValue::Plain(s.clone())),
            toml::Value::Table(t) => {
                let expression = t.get(EXPRESSION)?.as_str()?.to_string();
                let palette = t.get(OWNER).and_then(|o| o.as_str()) == Some(PALETTE);
                Some(if palette { BindingValue::Palette(expression) } else { BindingValue::Plain(expression) })
            }
            _ => None,
        }
    }

    pub fn expression(&self) -> &str {
        match self {
            BindingValue::Plain(s) | BindingValue::Palette(s) => s,
        }
    }
}

const EXPRESSION: &str = "expression";
const OWNER: &str = "owner";
const PALETTE: &str = "palette";

fn table(profile: &Profile) -> Option<&toml::Table> {
    profile.values.get(KEY)?.as_table()
}

/// Every well-formed entry of the profile's "Bindings" (an expression over `MAX_EXPRESSION`
/// characters is not read).
pub fn get(profile: &Profile) -> BTreeMap<String, BindingValue> {
    let Some(t) = table(profile) else { return BTreeMap::new() };
    t.iter().filter_map(|(k, v)| Some((k.clone(), BindingValue::from_value(v)?))).filter(|(_, b)| b.expression().chars().count() <= MAX_EXPRESSION).collect()
}

/// A color key's base, the key without its " (Light)" or " (Dark)" (`baseColorKeyForKey:`).
pub fn base_key(key: &str) -> &str {
    key.strip_suffix(" (Light)").or_else(|| key.strip_suffix(" (Dark)")).unwrap_or(key)
}

fn variants(base: &str) -> [String; 3] {
    [base.to_string(), format!("{base} (Light)"), format!("{base} (Dark)")]
}

pub fn is_color_key(key: &str) -> bool {
    crate::schema::find(base_key(key)).is_some_and(|s| s.kind == crate::schema::Kind::Color)
}

/// Sets (or, with None or an empty text, removes) the binding of `key` (`setBinding:forKey:`);
/// a color key first drops the palette-owned entries of its base key. Entries this module
/// ignores stay as they are.
pub fn set(profile: &mut Profile, key: &str, expression: Option<&str>) {
    if is_color_key(key) {
        drop_palette_in(profile, base_key(key));
    }
    let expression = expression.filter(|e| !e.is_empty());
    let mut t = match profile.values.remove(KEY) {
        Some(toml::Value::Table(t)) => t,
        _ => toml::Table::new(),
    };
    match expression {
        Some(e) => {
            t.insert(key.to_string(), toml::Value::String(e.to_string()));
        }
        None => {
            t.remove(key);
        }
    }
    if !t.is_empty() {
        profile.values.insert(KEY.to_string(), toml::Value::Table(t));
    }
}

/// Removes the palette-owned entries of `base` and its Light and Dark variants
/// (`bindings:byRemovingPaletteBindingsForBaseKey:`); whether any went.
pub fn drop_palette(bindings: &mut BTreeMap<String, BindingValue>, base: &str) -> bool {
    let mut any = false;
    for k in variants(base) {
        if matches!(bindings.get(&k), Some(BindingValue::Palette(_))) {
            bindings.remove(&k);
            any = true;
        }
    }
    any
}

/// `drop_palette` on the profile's stored dictionary (other entries untouched).
pub fn drop_palette_in(profile: &mut Profile, base: &str) -> bool {
    let Some(t) = table(profile) else { return false };
    let drop: Vec<String> = variants(base).into_iter().filter(|k| t.get(k).is_some_and(|v| matches!(BindingValue::from_value(v), Some(BindingValue::Palette(_))))).collect();
    if drop.is_empty() {
        return false;
    }
    if let Some(toml::Value::Table(t)) = profile.values.get_mut(KEY) {
        for k in &drop {
            t.remove(k);
        }
        if t.is_empty() {
            profile.values.remove(KEY);
        }
    }
    true
}

/// A concrete color picked for `key` by the person (the Settings well or wheel) drops the
/// palette-owned binding of its base key, or the binding's next answer would overwrite the pick
/// (`clearPaletteBindingForColorKey:`, `PCP:555-569`); a user-authored binding stays. Not for
/// propagating writes (`PS:8384-8394`). Whether anything went.
pub fn drop_for_write(profile: &mut Profile, key: &str) -> bool {
    is_color_key(key) && drop_palette_in(profile, base_key(key))
}

/// Whether a binding may name `key`: the color wells and checkboxes (the schema's color and
/// checkbox rows and their Light and Dark variants), never a path, command, Guid or Name key
/// (a profile's Bindings can come from an import or a script).
pub fn bindable(key: &str) -> bool {
    use crate::schema::Kind;
    use std::sync::OnceLock;
    static KEYS: OnceLock<std::collections::HashSet<String>> = OnceLock::new();
    let keys = KEYS.get_or_init(|| {
        let mut set = std::collections::HashSet::new();
        for s in crate::schema::profile().iter().filter(|s| matches!(s.kind, Kind::Color | Kind::Bool)) {
            let excluded = matches!(s.key.as_str(), "Rewritable" | "Is Dynamic Profile") || s.key == crate::profile::GUID || s.key == crate::profile::NAME || s.key == crate::session_profile::ORIGINAL_GUID || crate::start_dir::SCRUBBED_KEYS.contains(&s.key.as_str());
            if excluded {
                continue;
            }
            set.insert(s.key.clone());
            if s.light_dark {
                set.insert(format!("{} (Light)", s.key));
                set.insert(format!("{} (Dark)", s.key));
            }
        }
        set
    });
    keys.contains(key)
}

/// The bindings the app evaluates: bindable keys only, at most `MAX_BINDINGS`, as (key,
/// expression).
pub fn bound(profile: &Profile) -> Vec<(String, String)> {
    match table(profile) {
        Some(t) => bound_from(t.iter()),
        None => Vec::new(),
    }
}

/// `bound` over any walk of entries. Pulls entries one at a time and stops at `MAX_BINDINGS`;
/// only an entry that passes the cheap key check is cloned.
pub fn bound_from<'a>(entries: impl Iterator<Item = (&'a String, &'a toml::Value)>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (k, v) in entries {
        if out.len() >= MAX_BINDINGS {
            break;
        }
        if !bindable(k) {
            continue;
        }
        if let Some(b) = BindingValue::from_value(v).filter(|b| b.expression().chars().count() <= MAX_EXPRESSION) {
            out.push((k.clone(), b.expression().to_string()));
        }
    }
    out
}

/// The expression bound to `key`, borrowed from the stored table.
pub fn expression_of<'a>(profile: &'a Profile, key: &str) -> Option<&'a str> {
    match table(profile)?.get(key)? {
        toml::Value::String(s) => Some(s),
        toml::Value::Table(t) => t.get(EXPRESSION)?.as_str(),
        _ => None,
    }
}

/// Whether the profile binds `key` (a bindable key with a readable expression): one lookup, not
/// the whole table.
pub fn is_bound(profile: &Profile, key: &str) -> bool {
    bindable(key) && table(profile).and_then(|t| t.get(key)).and_then(BindingValue::from_value).is_some_and(|b| b.expression().chars().count() <= MAX_EXPRESSION)
}

/// The most entries a profile's "Bindings" table keeps. Nothing legitimate is near it
/// (`MAX_BINDINGS` are read).
pub const MAX_TABLE: usize = 256;

/// The cleaned form of an incoming "Bindings" value, made at the one write point
/// (`Profile::set`, `Profile::from_values`) so nothing downstream sees an unbounded or
/// unbindable table: bindable keys with a readable expression only, at most `MAX_TABLE` of them
/// (one hash lookup per entry). None for a value that is not a table or has nothing left.
pub fn clean(value: toml::Value) -> Option<toml::Value> {
    let toml::Value::Table(t) = value else { return None };
    let kept: toml::Table = t
        .into_iter()
        .filter(|(k, v)| k.len() <= 128 && bindable(k) && BindingValue::from_value(v).is_some_and(|b| b.expression().chars().count() <= MAX_EXPRESSION))
        .take(MAX_TABLE)
        .collect();
    (!kept.is_empty()).then_some(toml::Value::Table(kept))
}

/// iTerm2's descriptions of the mode color keys, for the failed-color log line
/// (`descriptionForKey:`, `PP:617-700`).
fn description(key: &str) -> Option<String> {
    let mode = if key.ends_with(" (Light)") { "light" } else if key.ends_with(" (Dark)") { "dark" } else { return None };
    let base = base_key(key);
    let phrase = match base {
        "Foreground Color" => "Default text color",
        "Background Color" => "Terminal background color",
        "Bold Color" => "Color for bold text",
        "Link Color" => "Color for clickable links",
        "Match Background Color" | "Match Color" => "Highlight color for search matches",
        "Selection Color" => "Background color of selected text",
        "Selected Text Color" => "Text color of selected text",
        "Cursor Color" => "Cursor fill color",
        "Cursor Text Color" => "Color of text under the cursor",
        "IME Cursor Color" => "IME composition cursor color",
        "Cursor Guide Color" => "Cursor Guide color",
        "Badge Color" => "Color of the badge overlay",
        "Active Pane Border Color" => "Border color for active pane",
        "Tab Color" => "Tab Color",
        "Underline Color" => "Color for underlined text",
        _ => {
            let n: usize = base.strip_prefix("Ansi ")?.strip_suffix(" Color")?.parse().ok()?;
            const NAMES: [&str; 8] = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];
            let name = NAMES.get(n % 8)?;
            return (n < 16).then(|| format!("ANSI {}{name} color in {mode} mode", if n >= 8 { "bright " } else { "" }));
        }
    };
    Some(format!("{phrase} in {mode} mode"))
}

fn class_name(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) | serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "dictionary",
    }
}

fn describe(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// A bound key's value as a color (`PP:512-545`). A string of 3 or 6 hex digits after "#"
/// (sRGB) or after "p3#" (Display P3; 12 digits are the 16-bit form), with two (sRGB) or four
/// (P3) alpha digits after the full form, kept only for the badge and cursor guide colors
/// (`colorKeyHonorsAlpha:`, a prefix match). Err is iTerm2's log line for a value that is not
/// a string (U+2757 without its variation selector), empty for a string that is no color.
pub fn color_from(value: &serde_json::Value, key: &str) -> Result<toml::Value, String> {
    let Some(text) = value.as_str() else {
        if value.is_null() {
            return Err(String::new());
        }
        let shown = description(key).unwrap_or_else(|| key.to_string());
        return Err(format!("\u{2757} The bound expression for {shown} is a color. It expected a string like #ff8844 but got the value \u{201c}{}\u{201d} of type \u{201c}{}\u{201d} instead.\n", describe(value), class_name(value)));
    };
    let (p3, rest) = match text.strip_prefix("p3#") {
        Some(r) => (true, r),
        None => (false, text.strip_prefix('#').ok_or_else(String::new)?),
    };
    if !rest.is_ascii() || !rest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(String::new());
    }
    let honors_alpha = key.starts_with("Badge Color") || key.starts_with("Cursor Guide Color");
    let hex = |s: &str| u32::from_str_radix(s, 16).unwrap_or(0);
    let mut alpha = 1.0;
    let rgb = match (p3, rest.len()) {
        (false, 8) => {
            if honors_alpha {
                alpha = f64::from(hex(&rest[6..])) / 255.0;
            }
            &rest[..6]
        }
        (true, 16) => {
            if honors_alpha {
                alpha = f64::from(hex(&rest[12..])) / 65535.0;
            }
            &rest[..12]
        }
        (_, 3 | 6 | 12) => rest,
        _ => return Err(String::new()),
    };
    let c: Vec<f64> = match rgb.len() {
        3 => rgb.chars().map(|d| f64::from(hex(&format!("{d}{d}"))) / 255.0).collect(),
        6 => (0..3).map(|i| f64::from(hex(&rgb[i * 2..i * 2 + 2])) / 255.0).collect(),
        12 if p3 => (0..3).map(|i| f64::from(hex(&rgb[i * 4..i * 4 + 4])) / 65535.0).collect(),
        _ => return Err(String::new()),
    };
    let get = |k: &str| match k {
        "Red Component" => Some(c[0]),
        "Green Component" => Some(c[1]),
        "Blue Component" => Some(c[2]),
        _ => Some(alpha),
    };
    crate::import::color_table(get, p3.then_some("P3")).ok_or_else(String::new)
}

/// A bound key's value as a checkbox (`PP:547-561`): a number is true unless 0; a string is
/// NSString's `boolValue` (Y, y, T, t or a nonzero digit first, after spaces, a sign and
/// zeros). Null and anything else: None.
pub fn bool_from(value: &serde_json::Value) -> Option<bool> {
    match value {
        serde_json::Value::Bool(b) => Some(*b),
        serde_json::Value::Number(n) => Some(n.as_f64().is_some_and(|f| f != 0.0)),
        serde_json::Value::String(s) => {
            let t = s.trim_start_matches([' ', '\t', '\n', '\r']);
            let t = t.strip_prefix(['+', '-']).unwrap_or(t);
            let t = t.trim_start_matches('0');
            Some(matches!(t.chars().next(), Some('Y' | 'y' | 'T' | 't' | '1'..='9')))
        }
        _ => None,
    }
}

/// The help a bindable key's type shows (`typeHelpForKey:`).
pub fn type_help(key: &str) -> Option<&'static str> {
    use crate::schema::Kind;
    match crate::schema::find(base_key(key)).filter(|s| key == s.key || s.light_dark)?.kind {
        Kind::Color => Some(COLOR_HELP),
        Kind::Bool => Some(BOOL_HELP),
        _ => None,
    }
}

/// The dialog's background help (`EBV:131-168`), with its example variable and environment
/// variable; "iTerm2" is "Termurion". Plain text: the markdown's headings and code are kept as
/// lines.
pub fn background_help(example_user_var: &str, example_env_var: &str) -> String {
    format!(
        "Background\nBinding a setting to an expression lets you change settings programmatically.\n\nTermurion tracks a collection of \u{201c}Variables\u{201d} for each session. You can learn more about them in Scripting Fundamentals (https://iterm2.com/documentation-scripting-fundamentals.html).\n\nTypically this feature is used by binding a setting to a user-defined variable.\n\nExample\nThe easiest way to set a user-defined variable is to install shell integration and then define a iterm2_print_user_vars function. Here's an example using bash:\n\niterm2_print_user_vars() {{\n  iterm2_set_user_var {example_user_var} $(echo ${example_env_var})\n}}\n\nThis runs each time the shell prompt is printed. The example sets a user-defined variable to the value of the environment variable {example_env_var}.\n\nThe appropriate expression to bind this example would be user.{example_user_var}. All user-defined variables go in the user scope.\n\nDebugging\nYou can view variables in the Inspector (Scripts > Manage > Console and then click Inspector)."
    )
}

/// The whole help the "?" button shows: the key's type help, then the background.
pub fn help_text(key: &str) -> String {
    let (user, env) = if is_color_key(key) { (COLOR_EXAMPLE, "BACKGROUND_COLOR") } else { (BOOL_EXAMPLE, "MY_SETTING") };
    let for_this = type_help(key).map(|h| format!("For This Setting\n{h}\n\n")).unwrap_or_default();
    format!("{for_this}{}", background_help(user, env))
}

/// The dialog field's placeholder (`EBV:84`).
pub fn placeholder(key: &str) -> String {
    format!("Expression (e.g., {})", if is_color_key(key) { COLOR_EXAMPLE } else { BOOL_EXAMPLE })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tbl(expression: &str, owner: &str) -> toml::Value {
        let mut t = toml::Table::new();
        t.insert("expression".into(), toml::Value::String(expression.into()));
        t.insert("owner".into(), toml::Value::String(owner.into()));
        toml::Value::Table(t)
    }

    /// S4 (13t x 13u): a binding can come from an import or a script, and a bound Session Hotkey
    /// would register a system-wide hotkey with no consent (13u B3), as a bound Shortcut would
    /// claim a Ctrl+Win chord: neither is bindable, and a table naming them evaluates nothing.
    #[test]
    fn bindings_never_name_a_hotkey() {
        for key in ["Session Hotkey", "Shortcut", "HotKey Key Code", "HotKey Modifier Flags", "HotKey Characters"] {
            assert!(!bindable(key), "{key}");
        }
        // The hotkey window checkboxes are bindable checkboxes, but a binding writes the session's
        // own copy (`set_session_values`) and hotkey windows register from the saved profiles
        // only, so a binding cannot register one.
        let prefs = include_str!("app/hotkey_ui.rs").split("fn hotkey_prefs(").nth(1).and_then(|r| r.split("\n    }").next()).expect("hotkey_prefs");
        assert!(prefs.contains("self.store.profiles.iter()") && !prefs.contains("sessions"));
        let (hk, sc) = ("Session Hotkey".to_string(), "Shortcut".to_string());
        let v = tbl("\"x\"", "user");
        assert!(bound_from([(&hk, &v), (&sc, &v)].into_iter()).is_empty());
    }

    fn with(entries: &[(&str, toml::Value)]) -> Profile {
        let mut p = Profile::default();
        let mut t = toml::Table::new();
        for (k, v) in entries {
            t.insert((*k).into(), v.clone());
        }
        p.set(KEY, toml::Value::Table(t));
        p
    }

    fn s(x: &str) -> toml::Value {
        toml::Value::String(x.into())
    }

    fn f(v: &toml::Value, k: &str) -> f64 {
        v.get(k).and_then(|x| x.as_float()).unwrap()
    }

    #[test]
    fn binding_values_follow_iterm2() {
        assert_eq!(BindingValue::from_value(&s("user.fg")), Some(BindingValue::Plain("user.fg".into())));
        assert_eq!(BindingValue::from_value(&tbl("x", "palette")), Some(BindingValue::Palette("x".into())));
        // Any other owner, or a table with no expression, is not a palette binding.
        assert_eq!(BindingValue::from_value(&tbl("x", "someone")), Some(BindingValue::Plain("x".into())));
        assert_eq!(BindingValue::from_value(&toml::Value::Integer(3)), None);
        assert_eq!(BindingValue::from_value(&toml::Value::Table(toml::Table::new())), None);
        assert_eq!(BindingValue::Palette("x".into()).expression(), "x");

        let mut p = Profile::default();
        set(&mut p, "Foreground Color", Some("user.fg"));
        assert_eq!(p.values.get(KEY).and_then(|v| v.get("Foreground Color")), Some(&s("user.fg")));
        set(&mut p, "Foreground Color", None);
        assert!(get(&p).is_empty());
        set(&mut p, "Foreground Color", Some("user.fg"));
        set(&mut p, "Foreground Color", Some(""));
        assert!(get(&p).is_empty());

        // A color binding first drops the palette-owned siblings of its base key and keeps the
        // plain ones (`setBinding:forKey:`).
        let mut p = with(&[("Tab Color (Dark)", tbl("x", "palette")), ("Tab Color (Light)", s("z")), ("Foreground Color", tbl("w", "palette"))]);
        set(&mut p, "Tab Color", Some("y"));
        let b = get(&p);
        assert_eq!(b.get("Tab Color"), Some(&BindingValue::Plain("y".into())));
        assert_eq!(b.get("Tab Color (Light)"), Some(&BindingValue::Plain("z".into())));
        assert!(!b.contains_key("Tab Color (Dark)"));
        assert_eq!(b.get("Foreground Color"), Some(&BindingValue::Palette("w".into())));

        let mut m = get(&with(&[("Tab Color (Dark)", tbl("x", "palette")), ("Tab Color", s("q"))]));
        assert!(drop_palette(&mut m, "Tab Color"));
        assert!(!drop_palette(&mut m, "Tab Color"));
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn color_conversion_follows_iterm2() {
        let c = color_from(&json!("#f8a"), "Foreground Color").unwrap();
        assert_eq!((f(&c, "r"), f(&c, "a")), (1.0, 1.0));
        assert!((f(&c, "g") - f64::from(0x88) / 255.0).abs() < 1e-9);
        assert!((f(&c, "b") - f64::from(0xaa) / 255.0).abs() < 1e-9);
        let c = color_from(&json!("#ff8844"), "Foreground Color").unwrap();
        assert!((f(&c, "g") - f64::from(0x88) / 255.0).abs() < 1e-9);
        assert!((f(&c, "b") - f64::from(0x44) / 255.0).abs() < 1e-9);
        // P3 goes through the importer's table: P3 red is outside sRGB.
        let p3 = color_from(&json!("p3#ff0000"), "Foreground Color").unwrap();
        let expect = crate::import::color_table(|k| Some(if k == "Red Component" { 1.0 } else { 0.0 }), Some("P3")).unwrap();
        assert_eq!(f(&p3, "r"), f(&expect, "r"));
        assert_eq!(f(&p3, "g"), f(&expect, "g"));
        // Alpha is kept only for the badge and cursor guide, and their mode keys.
        let alpha = |key: &str| f(&color_from(&json!("#ff884480"), key).unwrap(), "a");
        assert_eq!(alpha("Foreground Color"), 1.0);
        assert!((alpha("Badge Color") - f64::from(0x80) / 255.0).abs() < 1e-9);
        assert!((alpha("Badge Color (Dark)") - f64::from(0x80) / 255.0).abs() < 1e-9);
        assert!((alpha("Cursor Guide Color (Light)") - f64::from(0x80) / 255.0).abs() < 1e-9);
        // A string that is no color logs nothing.
        assert_eq!(color_from(&json!("nope"), "Foreground Color"), Err(String::new()));
        assert_eq!(color_from(&json!("#12"), "Foreground Color"), Err(String::new()));
        assert_eq!(
            color_from(&json!(5), "Foreground Color"),
            Err("\u{2757}\u{fe0f} The bound expression for Foreground Color is a color. It expected a string like #ff8844 but got the value \u{201c}5\u{201d} of type \u{201c}number\u{201d} instead.\n".replace('\u{fe0f}', ""))
        );
        // A mode key is described as iTerm2 does.
        assert!(color_from(&json!(true), "Foreground Color (Dark)").unwrap_err().contains("Default text color in dark mode"));
    }

    #[test]
    fn bool_conversion_follows_iterm2() {
        for (v, want) in [
            (json!(0), Some(false)),
            (json!(2), Some(true)),
            (json!(-1), Some(true)),
            (json!(0.0), Some(false)),
            (json!(true), Some(true)),
            (json!(false), Some(false)),
            (json!("1"), Some(true)),
            (json!("0"), Some(false)),
            (json!("true"), Some(true)),
            (json!("false"), Some(false)),
            (json!("yes"), Some(true)),
            (json!("no"), Some(false)),
            (json!("  Y"), Some(true)),
            (json!("007"), Some(true)),
            (json!("-5"), Some(true)),
            (json!(""), Some(false)),
            (json!(null), None),
            (json!([1]), None),
        ] {
            assert_eq!(bool_from(&v), want, "{v}");
        }
    }

    #[test]
    fn type_help_follows_iterm2() {
        assert_eq!(type_help("Foreground Color"), Some(COLOR_HELP));
        assert_eq!(type_help("Tab Color (Dark)"), Some(COLOR_HELP));
        assert_eq!(type_help("Blur"), Some(BOOL_HELP));
        assert_eq!(type_help("Columns"), None);
    }

    #[test]
    fn bindings_ignore_unbindable_keys() {
        let p = with(&[
            ("Foreground Color", s("user.fg")),
            ("Blur", s("user.blur")),
            ("Command", s("user.cmd")),
            ("Initial Text", s("user.t")),
            ("Working Directory", s("user.d")),
            ("Guid", s("user.g")),
            ("Name", s("user.n")),
            (KEY, s("user.b")),
            ("Triggers", s("user.t")),
            ("Tab Color (Dark)", tbl("user.tab", "palette")),
            ("No Such Key", s("user.x")),
            ("Ansi 1 Color", toml::Value::Integer(3)),
        ]);
        let b = bound(&p);
        let keys: Vec<&str> = b.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["Blur", "Foreground Color", "Tab Color (Dark)"]);
        assert!(b.iter().any(|(k, e)| k == "Tab Color (Dark)" && e == "user.tab"));
        // The one write point drops what a binding may never name.
        assert_eq!(get(&p).len(), 3);
    }

    #[test]
    fn bindings_are_bounded() {
        let many: Vec<(String, toml::Value)> = (0..16).map(|i| (format!("Ansi {i} Color"), s("user.x"))).chain((0..16).map(|i| (format!("Ansi {i} Color (Dark)"), s("user.x")))).chain([("Foreground Color".to_string(), s("user.x")), ("Blur".to_string(), s("user.x"))]).collect();
        let refs: Vec<(&str, toml::Value)> = many.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        assert_eq!(bound(&with(&refs)).len(), MAX_BINDINGS);
        let long = "x".repeat(MAX_EXPRESSION + 1);
        assert!(bound(&with(&[("Foreground Color", s(&long))])).is_empty());
        assert_eq!(bound(&with(&[("Foreground Color", s(&"x".repeat(MAX_EXPRESSION)))])).len(), 1);
    }

    #[test]
    fn a_picked_color_drops_only_the_palette_binding() {
        let mut p = with(&[("Foreground Color (Dark)", tbl("i:3", "palette")), ("Foreground Color", s("user.fg")), ("Blur", s("user.b"))]);
        assert!(!drop_for_write(&mut p, "Blur"), "not a color");
        assert!(drop_for_write(&mut p, "Foreground Color"), "the base key drops its mode variants too");
        let keys: Vec<String> = get(&p).into_keys().collect();
        assert_eq!(keys, ["Blur", "Foreground Color"], "a user-authored binding stays");
        assert!(!drop_for_write(&mut p, "Foreground Color"));
    }

    #[test]
    fn the_bindings_walk_is_bounded() {
        // A hostile table: 48 good entries, then 100k more. The walk stops at the limit.
        let good: Vec<(String, toml::Value)> = (0..16).map(|i| format!("Ansi {i} Color")).chain((0..16).map(|i| format!("Ansi {i} Color (Dark)"))).chain((0..16).map(|i| format!("Ansi {i} Color (Light)"))).map(|k| (k, s("user.x"))).collect();
        let rest: Vec<(String, toml::Value)> = (0..100_000).map(|i| (format!("junk {i}"), s("user.x"))).collect();
        let all: Vec<(String, toml::Value)> = good.into_iter().chain(rest).collect();
        let pulled = std::cell::Cell::new(0usize);
        let walk = all.iter().map(|(k, v)| {
            pulled.set(pulled.get() + 1);
            (k, v)
        });
        let b = bound_from(walk);
        assert_eq!(b.len(), MAX_BINDINGS);
        assert!(pulled.get() <= MAX_BINDINGS + 1, "pulled {}", pulled.get());
        // The same table through a profile.
        let refs: Vec<(&str, toml::Value)> = all.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        assert_eq!(bound(&with(&refs)).len(), MAX_BINDINGS);
    }

    fn junk_first_table() -> toml::Value {
        let mut t = toml::Table::new();
        for i in 0..100_000 {
            t.insert(format!("Aaa junk {i}"), s("user.x"));
        }
        t.insert("Foreground Color".into(), s("user.fg"));
        t.insert("Blur".into(), s("user.b"));
        toml::Value::Table(t)
    }

    /// Walks the stored table the way a pass does and says how many entries it pulled.
    fn pass_work(p: &Profile) -> (usize, usize) {
        let pulled = std::cell::Cell::new(0usize);
        let walk = table(p).into_iter().flat_map(|t| t.iter()).map(|(k, v)| {
            pulled.set(pulled.get() + 1);
            (k, v)
        });
        let n = bound_from(walk).len();
        (pulled.get(), n)
    }

    /// Every write funnel stores a cleaned table: the API, an arrangement's session write, an
    /// import or a file load all end in `Profile::set` or `Profile::from_values`.
    #[test]
    fn every_write_funnel_stores_a_cleaned_table() {
        let mut api = Profile::default();
        api.set(KEY, junk_first_table());
        let mut session = crate::session_profile::divorce(&Profile::new("S"), "g".into());
        session.set(&[(KEY.to_string(), Some(junk_first_table()))], &Profile::new("S"));
        let loaded = Profile::from_values(BTreeMap::from([(KEY.to_string(), junk_first_table()), ("Name".to_string(), s("kept"))]));
        for (what, p) in [("API set", &api), ("session set", &session.profile), ("from_values", &loaded)] {
            let t = table(p).unwrap_or_else(|| panic!("{what}: table"));
            assert!(t.len() <= MAX_TABLE, "{what}: {}", t.len());
            assert!(t.keys().all(|k| bindable(k)), "{what}: only bindable keys are stored");
            assert!(t.contains_key("Foreground Color") && t.contains_key("Blur"), "{what}");
            let (pulled, n) = pass_work(p);
            assert_eq!(n, 2, "{what}");
            assert!(pulled <= MAX_BINDINGS + 1, "{what}: a pass pulled {pulled} entries");
        }
        assert_eq!(loaded.string("Name"), "kept");
        // A non-table value is not stored; a short expression table is kept as it is.
        let mut p = Profile::default();
        p.set(KEY, s("not a table"));
        assert!(table(&p).is_none());
        p.set(KEY, toml::Value::Table(toml::Table::from_iter([("Blur".to_string(), s("user.b"))])));
        assert_eq!(table(&p).unwrap().len(), 1);
    }

    /// Reading one binding borrows the table (no copy of every entry), and a color write that
    /// drops nothing leaves the table alone.
    #[test]
    fn readers_borrow_the_table() {
        let p = with(&[("Foreground Color", s("user.fg")), ("Blur", s("user.b"))]);
        assert_eq!(expression_of(&p, "Blur"), Some("user.b"));
        assert_eq!(expression_of(&p, "Cursor Color"), None);
        let mut q = p.clone();
        assert!(!drop_palette_in(&mut q, "Foreground Color"));
        assert_eq!(q, p);
    }
}

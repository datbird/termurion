//! `.it2triggers` files: iTerm2's trigger export and import (`TriggerController.m`). Pure: the
//! text of a file in, canonical trigger tables out, and back. An imported file is untrusted, so
//! every bound is checked here and a file over one is refused whole.

use serde_json::Value as Json;
use toml::{Table, Value};

/// The extension of an exported trigger file (`TriggerController.m` save panel).
pub const EXTENSION: &str = "it2triggers";

/// What a trigger file may hold: a trigger file never needs more, and an imported one is untrusted
/// (B1). A file over any bound is refused whole, as iTerm2 refuses a file with one bad element.
pub const MAX_FILE: usize = 1024 * 1024;
pub const MAX_TRIGGERS: usize = 256;
pub const MAX_REGEX: usize = 4096;
pub const MAX_PARAM: usize = 64 * 1024;
pub const MAX_NAME: usize = 256;
pub const MAX_EVENT_KEYS: usize = 64;
pub const MAX_EVENT_DEPTH: usize = 4;
/// How much of a regex or parameter an import list shows (B7: a long parameter must not hide its tail).
const SHOWN: usize = 120;

pub const IMPORT_FAILED_HEADING: &str = "Import Failed";
pub const EXPORT_FAILED_HEADING: &str = "Export Failed";
pub const IMPORT_FAILED_ID: &str = "NoSyncImportTriggersFailed";
pub const MALFORMED_ID: &str = "NoSyncTriggerEncodingError";
pub const WRITE_FAILED_ID: &str = "NoSyncTriggerWritingError";
pub const IMPORT_TITLE: &str = "Import\u{2026}";
pub const EXPORT_TITLE: &str = "Export to File";

pub fn load_error_text(f: &str, e: &str) -> String {
    format!("While loading {f}: {e}")
}

pub fn parse_error_text(f: &str, e: &str) -> String {
    format!("While parsing {f}: {e}")
}

pub fn malformed_text(f: &str) -> String {
    format!("Malformed file at {f}")
}

pub fn save_error_text(name: &str, e: &str) -> String {
    format!("Error saving to {name}: {e}")
}

/// A refused file: iTerm2 shows it as a persistent warning (`id` has no silence choice).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportError {
    pub heading: &'static str,
    pub text: String,
    pub identifier: &'static str,
}

/// 13v1's question for a trigger file that came from the internet (Mark of the Web, P21).
pub const DOWNLOADED_TEXT: &str = "This file was downloaded. Its triggers can run commands.";

/// The identifier of the persistent warning an alert with this heading and text stands for.
pub fn identifier_for(heading: &str, text: &str) -> &'static str {
    if heading == EXPORT_FAILED_HEADING {
        WRITE_FAILED_ID
    } else if text.starts_with("Malformed file at ") {
        MALFORMED_ID
    } else {
        IMPORT_FAILED_ID
    }
}

fn malformed(f: &str) -> ImportError {
    ImportError { heading: IMPORT_FAILED_HEADING, text: malformed_text(f), identifier: MALFORMED_ID }
}

fn text_ok(v: &Value, max_bytes: usize) -> bool {
    match v {
        Value::String(s) => s.len() <= max_bytes && !s.contains('\0'),
        Value::Array(a) => a.iter().all(|e| text_ok(e, max_bytes)),
        Value::Table(t) => t.iter().all(|(k, e)| !k.contains('\0') && text_ok(e, max_bytes)),
        _ => true,
    }
}

fn chars_within(t: &Table, key: &str, max: usize) -> bool {
    t.get(key).and_then(Value::as_str).is_none_or(|s| s.chars().count() <= max)
}

fn event_params_ok(v: &Value, depth: usize) -> bool {
    match v {
        Value::Table(t) => depth <= MAX_EVENT_DEPTH && t.len() <= MAX_EVENT_KEYS && t.values().all(|e| event_params_ok(e, depth + 1)),
        Value::Array(a) => depth <= MAX_EVENT_DEPTH && a.len() <= MAX_EVENT_KEYS && a.iter().all(|e| event_params_ok(e, depth + 1)),
        _ => true,
    }
}

/// Whether a raw trigger is inside every bound (applied before it is sanitized and again after).
fn within_bounds(t: &Table) -> bool {
    chars_within(t, "regex", MAX_REGEX)
        && chars_within(t, "contentregex", MAX_REGEX)
        && ["name", "job", "provenance"].iter().all(|k| chars_within(t, k, MAX_NAME))
        && t.get("parameter").is_none_or(|p| text_ok(p, MAX_PARAM))
        && t.iter().all(|(k, v)| k == "parameter" || text_ok(v, MAX_PARAM))
        && t.get("eventParams").is_none_or(|e| event_params_ok(e, 1))
}

/// One trigger as iTerm2's own import stores it: `triggerFromUntrustedDict:` then
/// `dictionaryValue` (`Trigger.m:68-97`, `:521-533`), then the one write point's caps
/// (`triggers::clean`). None when the action names no trigger class, or a bound is exceeded.
pub fn canonical(t: &Table) -> Option<Table> {
    if !within_bounds(t) {
        return None;
    }
    let j = crate::api_profile::sanitized_trigger(t)?;
    let c = crate::import::from_json(&j)?.as_table().cloned()?;
    if !within_bounds(&c) {
        return None;
    }
    let kept = crate::triggers::clean(&Value::Array(vec![Value::Table(c.clone())]));
    (kept.as_array().is_some_and(|a| a.len() == 1)).then_some(c)
}

/// `TriggerController.m:1139-1169`: one compact JSON array of the triggers, each in its
/// canonical form (a trigger that has none is left out). iTerm2's JSON writer escapes `/`.
pub fn export_text(stored: &[Table]) -> String {
    let list: Vec<Json> = stored.iter().filter(|t| canonical(t).is_some()).filter_map(crate::api_profile::sanitized_trigger).collect();
    Json::Array(list).to_string().replace('/', "\\/")
}

/// `TriggerController.m:580-656`: the triggers of a file's text, canonical and in file order.
/// `filename` is only for the messages. A bad file is refused whole.
pub fn parse(text: &str, filename: &str) -> Result<Vec<Table>, ImportError> {
    if text.len() > MAX_FILE {
        return Err(malformed(filename));
    }
    let root: Json = serde_json::from_str(text).map_err(|e| ImportError { heading: IMPORT_FAILED_HEADING, text: parse_error_text(filename, &e.to_string()), identifier: IMPORT_FAILED_ID })?;
    let elements = match root {
        Json::Object(_) => vec![root],
        Json::Array(a) => a,
        _ => return Err(malformed(filename)),
    };
    if elements.len() > MAX_TRIGGERS {
        return Err(malformed(filename));
    }
    elements
        .iter()
        .map(|e| match e {
            Json::Object(_) => crate::import::from_json(e).and_then(|v| v.as_table().and_then(canonical)).ok_or_else(|| malformed(filename)),
            _ => Err(malformed(filename)),
        })
        .collect()
}

/// A trigger with its `disabled` flag left out: what two triggers are compared by, so a person
/// who enabled an imported command trigger is not asked again.
fn identity(t: &Table) -> Option<Table> {
    let mut c = canonical(t)?;
    c.remove("disabled");
    Some(c)
}

/// `TriggerController.m:717-735`: the stored list as it is, then each imported trigger whose
/// canonical form no stored trigger has, in order (imported triggers are not compared with each
/// other). A trigger that acts on the machine is stored turned off (B7).
pub fn add_triggers(existing: Option<&Value>, imported: &[Table]) -> Value {
    let mut list: Vec<Value> = existing.and_then(Value::as_array).cloned().unwrap_or_default();
    let have: Vec<Table> = list.iter().filter_map(Value::as_table).filter_map(identity).collect();
    for t in imported {
        if identity(t).is_some_and(|i| have.contains(&i)) {
            continue;
        }
        let mut t = t.clone();
        if t.get("action").and_then(Value::as_str).is_some_and(crate::triggers::acts_on_machine) {
            t.insert("disabled".into(), Value::Boolean(true));
        }
        list.push(Value::Table(t));
    }
    Value::Array(list)
}

/// How many of these triggers import turned off.
pub fn machine_count(imported: &[Table]) -> usize {
    imported.iter().filter(|t| t.get("action").and_then(Value::as_str).is_some_and(crate::triggers::acts_on_machine)).count()
}

/// `NSStringITerm.m:2988-3027` over `it_unsafeForDisplayCharacters`: LF as `\n`, ESC as `\e`, any
/// other control, bidi, zero-width or invisible character as `<U+XXXX>`.
pub fn sanitized_for_display(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let u = c as u32;
        let unsafe_char = matches!(u, 0x00..=0x1F | 0x7F | 0x80..=0x9F | 0x202A..=0x202E | 0x2066..=0x2069 | 0x200B | 0x200C | 0x200D | 0xFEFF | 0xAD | 0x2060 | 0x2064);
        match (unsafe_char, c) {
            (false, _) => out.push(c),
            (true, '\n') => out.push_str("\\n"),
            (true, '\u{1b}') => out.push_str("\\e"),
            (true, _) if u > 0xFFFF => out.push_str(&format!("<U+{u:06X}>")),
            (true, _) => out.push_str(&format!("<U+{u:04X}>")),
        }
    }
    out
}

/// `s` cut to `SHOWN` characters with an ellipsis.
fn shortened(s: &str) -> String {
    if s.chars().count() <= SHOWN {
        return s.to_string();
    }
    s.chars().take(SHOWN).chain(['\u{2026}']).collect()
}

fn param(t: &Table) -> &str {
    t.get("parameter").and_then(Value::as_str).unwrap_or("")
}

fn hex(c: Option<alacritty_terminal::vte::ansi::Rgb>) -> String {
    c.map_or_else(|| "(no color)".to_string(), |c| format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b))
}

/// Each trigger class's own `-description` (`grep -n "description" sources/Triggers/*`), with the
/// shown parameter shortened. A class with none is described by its catalog title.
pub fn action_description(t: &Table) -> String {
    let action = t.get("action").and_then(Value::as_str).unwrap_or("");
    let p = shortened(param(t));
    let quoted = |label: &str| format!("{label} \u{201c}{p}\u{201d}");
    match action {
        "BellTrigger" => "Ring Bell".into(),
        "AlertTrigger" => quoted("Show alert"),
        "AnnotateTrigger" => quoted("Annotate as as"),
        "BounceTrigger" => format!("Bounce taskbar button {}", if p.trim() == "1" { "once" } else { "until focused" }),
        "iTermHyperlinkTrigger" => quoted("Make Hyperlink with URL"),
        "iTermRPCTrigger" => quoted("Invoke Script Function"),
        "iTermSetTitleTrigger" => quoted("Set Title to"),
        "CaptureTrigger" if p.is_empty() => "Capture Output".into(),
        "CaptureTrigger" => format!("Capture output, running \u{201c}{p}\u{201d} on double-click"),
        "CoprocessTrigger" | "MuteCoprocessTrigger" => quoted("Run Coprocess"),
        "ScriptTrigger" => quoted("Run Command"),
        "SetDirectoryTrigger" => quoted("Report Directory as"),
        "SetHostnameTrigger" => quoted("Report User & Host as"),
        "SendTextTrigger" => quoted("Send text"),
        "iTermUserNotificationTrigger" | "GrowlTrigger" => quoted("Post Notification"),
        "iTermInjectTrigger" => quoted("Inject Data"),
        "iTermSGRTrigger" => quoted("Change Style"),
        "iTermFoldTrigger" => format!("Fold to {p}"),
        "iTermSetNamedMarkTrigger" => format!("Set Named Mark to {p}"),
        "iTermBufferInputTrigger" => if p.trim() == "1" { "Stop Buffering Input" } else { "Buffer Input" }.into(),
        "MarkTrigger" => format!("Set Mark and {} scrolling", if p.trim() == "1" { "stop" } else { "continue" }),
        "PasswordTrigger" if p.is_empty() => "Open Password Manager".into(),
        "PasswordTrigger" => format!("Open Password Manager to \u{201c}{p}\u{201d}"),
        "HighlightTrigger" | "iTermHighlightLineTrigger" => {
            let (fg, bg) = crate::trigger_eval::highlight_colors(param(t));
            if action == "HighlightTrigger" {
                format!("Highlight text {} over {}", hex(fg), hex(bg))
            } else {
                format!("Highlight Line with {} over {}", hex(fg), hex(bg))
            }
        }
        "iTermSetUserVariableTrigger" => match param(t).split_once('\u{1}') {
            Some((name, value)) => format!("Set User Variable \u{201c}{}\u{201d} to \u{201c}{}\u{201d}", shortened(name), shortened(value)),
            None => quoted("Set User Variable"),
        },
        "iTermSetProfileBooleanTrigger" => match crate::settings_triggers::split_setting(param(t)) {
            (key, on) if !key.is_empty() => {
                let label = crate::schema::find(&key).map_or(key.clone(), |s| s.label.clone());
                format!("Set \u{201c}{}\u{201d} to {}", shortened(&label), if on { "On" } else { "Off" })
            }
            _ => "Set Profile Setting".into(),
        },
        other => crate::triggers::action_info(other).map_or_else(|| other.to_string(), |a| a.title.to_string()),
    }
}

/// `TriggerController.m:670-680`: "On \u{201c}regex\u{201d}, description[ instantly]", for one
/// line of the profile chooser.
pub fn import_description(t: &Table) -> String {
    let regex = shortened(t.get("regex").and_then(Value::as_str).unwrap_or(""));
    let partial = t.get("partial").and_then(Value::as_bool).unwrap_or(false);
    let line = format!("On \u{201c}{regex}\u{201d}, {}{}", action_description(t), if partial { " instantly" } else { "" });
    sanitized_for_display(&line)
}

/// `TriggerController.m:681-693`: the chooser's text. At most ten lines, then "and n more"; B7 adds
/// how many of them import turned off.
pub fn select_profiles_text(imported: &[Table]) -> String {
    let mut lines: Vec<String> = imported.iter().take(10).map(|t| format!("\u{2022} {}", import_description(t))).collect();
    if imported.len() > 10 {
        lines.push(format!("\u{2026}and {} more", imported.len() - 10));
    }
    let mut text = format!("Select the profiles into which these triggers should be imported:\n\n{}", lines.join("\n"));
    let off = machine_count(imported);
    if off > 0 {
        text.push_str(&format!("\n\n{off} of these triggers run commands or type text; they are imported turned off."));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::from_json;
    use serde_json::json;

    fn table(j: serde_json::Value) -> toml::Table {
        from_json(&j).and_then(|v| v.as_table().cloned()).expect("a table")
    }

    fn highlight() -> toml::Table {
        table(json!({"action": "HighlightTrigger", "regex": "err", "parameter": "{#ff0000,}", "partial": false, "disabled": false, "matchType": 0}))
    }

    fn alert(regex: &str, param: &str) -> toml::Table {
        table(json!({"action": "AlertTrigger", "regex": regex, "parameter": param}))
    }

    #[test]
    fn trigger_file_round_trips() {
        let stored = vec![highlight()];
        assert_eq!(export_text(&stored), r##"[{"action":"HighlightTrigger","regex":"err","contentregex":"","matchType":0,"parameter":"{#ff0000,}","partial":false,"disabled":false}]"##);
        let two = vec![highlight(), table(json!({"action": "AlertTrigger", "regex": "done", "parameter": "hi", "partial": true, "name": "n", "extra": 5}))];
        let parsed = parse(&export_text(&two), "t.it2triggers").expect("parses");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[1].get("name").and_then(toml::Value::as_str), Some("n"));
        assert!(!parsed[1].contains_key("extra"));
        assert_eq!(export_text(&parsed), export_text(&two), "export then import is a fixed point");
        let one = parse(r#"{"action":"AlertTrigger","regex":"x","parameter":"y"}"#, "t.it2triggers").expect("one object");
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].get("contentregex").and_then(toml::Value::as_str), Some(""));
        assert_eq!(one[0].get("partial").and_then(toml::Value::as_bool), Some(false));
        assert_eq!(one[0].get("disabled").and_then(toml::Value::as_bool), Some(false));
        assert_eq!(one[0].get("matchType").and_then(toml::Value::as_integer), Some(0));
    }

    #[test]
    fn malformed_trigger_files_follow_iterm2() {
        let f = "C:\\t\\a.it2triggers";
        let e = parse("{", f).unwrap_err();
        assert_eq!((e.heading, e.identifier), ("Import Failed", IMPORT_FAILED_ID));
        assert!(e.text.starts_with("While parsing C:\\t\\a.it2triggers: "), "{}", e.text);
        let malformed = ImportError { heading: "Import Failed", text: "Malformed file at C:\\t\\a.it2triggers".into(), identifier: MALFORMED_ID };
        for bad in ["5", "[1]", r#"[{"regex":"x"}]"#, r#"[{"action":"NoSuchTrigger","regex":"x"}]"#, r#"[{"action":"AlertTrigger","regex":"a"}, 7]"#] {
            assert_eq!(parse(bad, f), Err(malformed.clone()), "{bad}");
        }
        assert_eq!(parse("[]", f), Ok(Vec::new()));
        assert_eq!(load_error_text("a", "denied"), "While loading a: denied");
    }

    #[test]
    fn import_merges_as_iterm2() {
        let (a, b, c) = (alert("a", "1"), alert("b", "2"), alert("c", "3"));
        let mut b2 = b.clone();
        b2.insert("performance".into(), toml::Value::Table(toml::Table::new()));
        let stored = toml::Value::Array(vec![a.clone().into(), b.clone().into()]);
        let merged = add_triggers(Some(&stored), &[b2, c.clone()]);
        assert_eq!(merged, toml::Value::Array(vec![a.into(), b.into(), c.clone().into()]));
        assert_eq!(add_triggers(None, &[c.clone(), c.clone()]), toml::Value::Array(vec![c.clone().into(), c.into()]));
    }

    #[test]
    fn import_texts_follow_iterm2() {
        assert_eq!(sanitized_for_display("a\u{1b}b\nc\u{200b}d\u{1F600}"), "a\\eb\\nc<U+200B>d\u{1F600}");
        assert_eq!(sanitized_for_display("\u{7}"), "<U+0007>");
        let foo = alert("foo", "hi");
        assert_eq!(import_description(&foo), "On \u{201c}foo\u{201d}, Show alert \u{201c}hi\u{201d}");
        let mut partial = foo.clone();
        partial.insert("partial".into(), true.into());
        assert_eq!(import_description(&partial), "On \u{201c}foo\u{201d}, Show alert \u{201c}hi\u{201d} instantly");
        let twelve = vec![foo.clone(); 12];
        let text = select_profiles_text(&twelve);
        assert_eq!(text.lines().filter(|l| l.starts_with('\u{2022}')).count(), 10);
        assert!(text.ends_with("\n\u{2026}and 2 more"), "{text}");
        assert_eq!(select_profiles_text(&[foo]), "Select the profiles into which these triggers should be imported:\n\n\u{2022} On \u{201c}foo\u{201d}, Show alert \u{201c}hi\u{201d}");
        assert_eq!(save_error_text("t.it2triggers", "full"), "Error saving to t.it2triggers: full");
        assert_eq!(EXTENSION, "it2triggers");
    }

    #[test]
    fn trigger_file_bounds_refuse_whole() {
        let f = "t.it2triggers";
        let ok = |j: serde_json::Value| parse(&j.to_string(), f).is_ok();
        let a = |k: &str, v: serde_json::Value| {
            let mut o = json!({"action": "AlertTrigger", "regex": "x", "parameter": "y"});
            o[k] = v;
            json!([o])
        };
        assert!(ok(a("regex", json!("r".repeat(MAX_REGEX)))));
        assert!(!ok(a("regex", json!("r".repeat(MAX_REGEX + 1)))));
        assert!(ok(a("contentregex", json!("r".repeat(MAX_REGEX)))));
        assert!(!ok(a("contentregex", json!("r".repeat(MAX_REGEX + 1)))));
        assert!(ok(a("parameter", json!("p".repeat(MAX_PARAM)))));
        assert!(!ok(a("parameter", json!("p".repeat(MAX_PARAM + 1)))));
        for k in ["name", "job", "provenance"] {
            assert!(ok(a(k, json!("n".repeat(MAX_NAME)))), "{k}");
            assert!(!ok(a(k, json!("n".repeat(MAX_NAME + 1)))), "{k}");
        }
        assert!(!ok(a("name", json!("a\u{0}b"))));
        assert!(!ok(a("parameter", json!("a\u{0}b"))));
        let keys = |n: usize| serde_json::Value::Object((0..n).map(|i| (format!("k{i}"), json!(1))).collect());
        assert!(ok(a("eventParams", keys(MAX_EVENT_KEYS))));
        assert!(!ok(a("eventParams", keys(MAX_EVENT_KEYS + 1))));
        let deep = |d: usize| (0..d).fold(json!(1), |inner, _| json!({"k": inner}));
        assert!(ok(a("eventParams", deep(MAX_EVENT_DEPTH))));
        assert!(!ok(a("eventParams", deep(MAX_EVENT_DEPTH + 1))));
        let many = |n: usize| serde_json::Value::Array((0..n).map(|_| json!({"action": "BellTrigger", "regex": "x"})).collect());
        assert!(ok(many(MAX_TRIGGERS)));
        assert!(!ok(many(MAX_TRIGGERS + 1)));
        assert!(parse(&" ".repeat(MAX_FILE + 1), f).is_err(), "a file over the cap is refused before parsing");
        // The bounds hold in `canonical` too, for a table that did not come through `parse`.
        let mut t = alert("x", "y");
        t.insert("regex".into(), "r".repeat(MAX_REGEX + 1).into());
        assert_eq!(canonical(&t), None);
    }

    #[test]
    fn machine_triggers_import_disabled() {
        for class in ["ScriptTrigger", "CoprocessTrigger", "MuteCoprocessTrigger", "CaptureTrigger", "SendTextTrigger", "PasswordTrigger", "iTermRPCTrigger", "iTermSetProfileBooleanTrigger"] {
            assert!(crate::triggers::acts_on_machine(class), "{class}");
            let t = table(json!({"action": class, "regex": "x", "parameter": "p", "disabled": false}));
            let merged = add_triggers(None, &[canonical(&t).unwrap()]);
            let first = merged.as_array().unwrap()[0].as_table().unwrap();
            assert_eq!(first.get("disabled").and_then(toml::Value::as_bool), Some(true), "{class}");
        }
        for class in ["HighlightTrigger", "AlertTrigger", "BellTrigger", "iTermUserNotificationTrigger", "MarkTrigger"] {
            assert!(!crate::triggers::acts_on_machine(class), "{class}");
        }
        let t = canonical(&table(json!({"action": "AlertTrigger", "regex": "x", "parameter": "p", "disabled": false}))).unwrap();
        assert_eq!(add_triggers(None, std::slice::from_ref(&t)).as_array().unwrap()[0].as_table().unwrap().get("disabled").and_then(toml::Value::as_bool), Some(false));
        // The chooser says how many are imported turned off, and shortens what it shows.
        let run = canonical(&table(json!({"action": "ScriptTrigger", "regex": "x", "parameter": format!("echo ok{}& calc", " ".repeat(500))}))).unwrap();
        let text = select_profiles_text(&[run.clone(), t]);
        assert!(text.ends_with("\n\n1 of these triggers run commands or type text; they are imported turned off."), "{text}");
        assert!(text.contains('\u{2026}') && !text.contains("calc"), "{text}");
    }

    #[test]
    fn import_twice_adds_once_even_when_disabled() {
        let run = canonical(&table(json!({"action": "SendTextTrigger", "regex": "x", "parameter": "ls\\n"}))).unwrap();
        let once = add_triggers(None, std::slice::from_ref(&run));
        let twice = add_triggers(Some(&once), std::slice::from_ref(&run));
        assert_eq!(twice.as_array().unwrap().len(), 1);
        // A person who enabled it is not asked again either.
        let mut enabled = once.as_array().unwrap()[0].as_table().unwrap().clone();
        enabled.insert("disabled".into(), false.into());
        let stored = toml::Value::Array(vec![enabled.into()]);
        assert_eq!(add_triggers(Some(&stored), std::slice::from_ref(&run)).as_array().unwrap().len(), 1);
    }

    #[test]
    fn descriptions_follow_the_trigger_classes() {
        let d = |j: serde_json::Value| action_description(&table(j));
        assert_eq!(d(json!({"action": "BellTrigger"})), "Ring Bell");
        assert_eq!(d(json!({"action": "BounceTrigger", "parameter": "0"})), "Bounce taskbar button until focused");
        assert_eq!(d(json!({"action": "BounceTrigger", "parameter": "1"})), "Bounce taskbar button once");
        assert_eq!(d(json!({"action": "HighlightTrigger", "parameter": "{#ff0000,}"})), "Highlight text #ff0000 over (no color)");
        assert_eq!(d(json!({"action": "MarkTrigger", "parameter": "1"})), "Set Mark and stop scrolling");
        assert_eq!(d(json!({"action": "CaptureTrigger", "parameter": ""})), "Capture Output");
        assert_eq!(d(json!({"action": "ScriptTrigger", "parameter": "x"})), "Run Command \u{201c}x\u{201d}");
        assert_eq!(d(json!({"action": "StopTrigger"})), "Stop Processing Triggers");
        assert_eq!(d(json!({"action": "iTermSetTabStatusTrigger"})), "Set Tab Status\u{2026}");
    }
}

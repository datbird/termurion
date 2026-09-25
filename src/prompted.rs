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
        // A program may only point the background image at a local file.
        (Kind::String, J::String(t)) if matches!(key, "Background Image Location" | "Background Image Folder Location") && !t.is_empty() && !local_path_ok(t) => return None,
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

/// A path from a program that Tandem may touch: a local absolute path (`C:\...`). Network
/// and device paths (`\\server\share`, `//host`, `\\?\`) are refused before any file is
/// opened, since opening one sends the user's Windows credentials to that server.
pub fn local_path_ok(path: &str) -> bool {
    let b = path.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
}

/// A ReportVariable name Tandem will look up or ask about: letters, digits, `_`, and `.`.
pub fn variable_name_ok(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.')
}

/// OpenURL's value, `<ignored>:<base64 url>`: the URL, when it has a scheme.
pub fn open_url(value: &str) -> Option<String> {
    use base64::Engine;
    let (_, b64) = value.split_once(':')?;
    let url = String::from_utf8(base64::engine::general_purpose::STANDARD.decode(b64.trim()).ok()?).ok()?;
    let (scheme, rest) = url.split_once(':')?;
    // Web and mail links only: a file, a drive path, or another scheme could run something.
    let ok = matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https" | "ftp" | "mailto") && !rest.is_empty();
    ok.then_some(url)
}

/// The host part of a URL (without user, port, path), as iTerm2 compares it.
pub fn url_host(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("://")?;
    let auth = rest.split(['/', '?', '#']).next()?;
    let auth = auth.rsplit_once('@').map_or(auth, |(_, h)| h);
    let host = if let Some(v6) = auth.strip_prefix('[') {
        v6.split(']').next()?
    } else {
        auth.rsplit_once(':').filter(|(_, port)| port.bytes().all(|b| b.is_ascii_digit())).map_or(auth, |(h, _)| h)
    };
    (!host.is_empty()).then(|| host.to_string())
}

/// The stored key that always allows OpenURL for a host: `OpenURL` + hex of its bytes.
pub fn host_key(host: &str) -> String {
    let hex: String = host.bytes().map(|b| format!("{b:02x}")).collect();
    format!("OpenURL{hex}")
}

/// ReportVariable's permission from `NoSyncVariablesToReport` (`allow:name` and
/// `deny:name`, comma separated; deny wins).
pub fn variable_decision(name: &str, list: &str) -> Decision {
    let entries: Vec<&str> = list.split(',').map(str::trim).filter(|e| !e.is_empty()).collect();
    if entries.iter().any(|e| e.strip_prefix("deny:") == Some(name)) {
        Decision::Deny
    } else if entries.iter().any(|e| e.strip_prefix("allow:") == Some(name)) {
        Decision::Apply
    } else {
        Decision::Ask
    }
}

/// ReportVariable's answer: the value in base64 (empty for none), ended by BEL.
pub fn variable_reply(value: Option<&str>) -> Vec<u8> {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(value.unwrap_or(""));
    format!("\x1b]1337;ReportVariable={b64}\x07").into_bytes()
}

/// What ReportVariable can report about a session.
#[derive(Clone, Debug, Default)]
pub struct Vars {
    pub session: std::collections::BTreeMap<&'static str, String>,
    pub user: std::collections::BTreeMap<String, String>,
    pub tab: std::collections::BTreeMap<&'static str, String>,
    pub window: std::collections::BTreeMap<&'static str, String>,
    pub app: std::collections::BTreeMap<&'static str, String>,
}

/// A variable by iTerm2's name in the session scope: bare session names, `user.`,
/// `tab.`, `tab.window.`, `iterm2.`. Anything else (`session.name` too) is None.
pub fn variable(name: &str, vars: &Vars) -> Option<String> {
    if let Some(n) = name.strip_prefix("user.") {
        return vars.user.get(n).cloned();
    }
    if let Some(n) = name.strip_prefix("tab.window.") {
        return vars.window.get(n).cloned();
    }
    if let Some(n) = name.strip_prefix("tab.") {
        return vars.tab.get(n).cloned();
    }
    if let Some(n) = name.strip_prefix("iterm2.") {
        return vars.app.get(n).cloned();
    }
    if name.contains('.') {
        return None;
    }
    vars.session.get(name).cloned()
}

/// RequestUpload's parameters: Ok when the format is tgz (or not given).
pub fn upload_format_ok(params: &str) -> bool {
    params.split(';').filter_map(|p| p.split_once('=')).find(|(k, _)| *k == "format").is_none_or(|(_, v)| v == "tgz")
}

/// RequestUpload's answers before the archive: OK, and cancel (^C, iTerm2's choice over
/// the protocol's `abort`, which a report must not hold).
pub const UPLOAD_OK: &[u8] = b"ok\n";
pub const UPLOAD_CANCEL: &[u8] = b"\x03";

/// A gzipped tar of `paths` with names relative to their lowest common folder (one folder
/// alone: relative to its parent).
pub fn upload_archive(paths: &[std::path::PathBuf]) -> Result<Vec<u8>, String> {
    use std::path::{Path, PathBuf};
    if paths.is_empty() {
        return Err("nothing chosen".into());
    }
    // Files count as their folder; folders as themselves.
    let homes: Vec<PathBuf> = paths.iter().map(|p| if p.is_dir() { p.clone() } else { p.parent().map(Path::to_path_buf).unwrap_or_default() }).collect();
    let base = if paths.len() == 1 && paths[0].is_dir() {
        paths[0].parent().map(Path::to_path_buf).unwrap_or_default()
    } else {
        let mut base = homes[0].clone();
        for h in &homes[1..] {
            while !h.starts_with(&base) {
                if !base.pop() {
                    break;
                }
            }
        }
        base
    };
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut tar = tar::Builder::new(gz);
    tar.follow_symlinks(false);
    for p in paths {
        let rel = p.strip_prefix(&base).map_err(|_| format!("{} is outside {}", p.display(), base.display()))?;
        let name = Path::new(".").join(rel);
        let result = if p.is_dir() { tar.append_dir_all(&name, p) } else { tar.append_path_with_name(p, &name) };
        result.map_err(|e| format!("{}: {e}", p.display()))?;
    }
    let gz = tar.into_inner().map_err(|e| e.to_string())?;
    gz.finish().map_err(|e| e.to_string())
}

/// The archive as sent: base64 in 76-column lines each ending CR, then CR CR.
pub fn upload_lines(archive: &[u8]) -> Vec<u8> {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(archive);
    let mut out = Vec::with_capacity(b64.len() + b64.len() / 76 + 3);
    for line in b64.as_bytes().chunks(76) {
        out.extend_from_slice(line);
        out.push(b'\r');
    }
    out.extend_from_slice(b"\r\r");
    out
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
    #[test]
    fn open_url_parse_and_hosts() {
        use base64::Engine;
        let b = |u: &str| base64::engine::general_purpose::STANDARD.encode(u);
        assert_eq!(open_url(&format!("x=1:{}", b("https://Example.com:8080/p?q#f"))).as_deref(), Some("https://Example.com:8080/p?q#f"));
        assert_eq!(open_url(&format!(":{}", b("mailto:a@b.c"))).as_deref(), Some("mailto:a@b.c"));
        assert_eq!(open_url(&b("https://x.io")), None, "a colon is required");
        assert_eq!(open_url(&format!(":{}", b("no scheme here"))), None);
        assert_eq!(open_url(":!!!"), None);
        assert_eq!(url_host("https://user:pw@Example.com:8080/p").as_deref(), Some("Example.com"));
        assert_eq!(url_host("http://[::1]:80/").as_deref(), Some("::1"));
        assert_eq!(url_host("mailto:a@b.c"), None);
        assert_eq!(host_key("Example.com"), "OpenURL4578616d706c652e636f6d");
    }

    #[test]
    fn report_variable_replies() {
        let mut v = Vars::default();
        v.session.insert("name", "zsh".into());
        v.session.insert("columns", "80".into());
        v.user.insert("git_branch".into(), "main".into());
        v.tab.insert("title", "T".into());
        v.window.insert("id", "1".into());
        v.app.insert("pid", "42".into());
        assert_eq!(variable("name", &v).as_deref(), Some("zsh"));
        assert_eq!(variable("user.git_branch", &v).as_deref(), Some("main"));
        assert_eq!(variable("tab.title", &v).as_deref(), Some("T"));
        assert_eq!(variable("tab.window.id", &v).as_deref(), Some("1"));
        assert_eq!(variable("iterm2.pid", &v).as_deref(), Some("42"));
        assert_eq!(variable("session.name", &v), None, "not a name in this scope");
        assert_eq!(variable("nope", &v), None);
        assert_eq!(variable_reply(Some("zsh")), b"\x1b]1337;ReportVariable=enNo\x07");
        assert_eq!(variable_reply(None), b"\x1b]1337;ReportVariable=\x07");
    }

    #[test]
    fn variable_permissions() {
        assert_eq!(variable_decision("name", ""), Decision::Ask);
        assert_eq!(variable_decision("name", "allow:name,deny:rows"), Decision::Apply);
        assert_eq!(variable_decision("rows", "allow:name,deny:rows"), Decision::Deny);
        assert_eq!(variable_decision("name", "allow:name, ,deny:name"), Decision::Deny, "deny wins");
        assert_eq!(variable_decision("nam", "allow:name"), Decision::Ask, "whole names only");
    }

    #[test]
    fn upload_archive_and_lines() {
        let dir = std::env::temp_dir().join(format!("tandem-up-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), b"alpha").unwrap();
        std::fs::write(dir.join("sub").join("b.txt"), b"beta").unwrap();
        let archive = upload_archive(&[dir.join("a.txt"), dir.join("sub")]).unwrap();
        let mut names: Vec<String> = tar::Archive::new(flate2::read::GzDecoder::new(&archive[..]))
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().to_string_lossy().replace('\\', "/"))
            .collect();
        names.sort();
        // iTerm2's tar writes `./a.txt`; the tar crate writes `a.txt`. Both extract alike.
        assert_eq!(names, vec!["a.txt", "sub/", "sub/b.txt"]);
        let only = upload_archive(&[dir.join("sub")]).unwrap();
        let first: Vec<String> = tar::Archive::new(flate2::read::GzDecoder::new(&only[..])).entries().unwrap().map(|e| e.unwrap().path().unwrap().to_string_lossy().replace('\\', "/")).collect();
        assert!(first.contains(&"sub/b.txt".to_string()), "one folder: relative to its parent: {first:?}");
        let lines = upload_lines(&[7u8; 100]);
        let text = String::from_utf8(lines).unwrap();
        let parts: Vec<&str> = text.split('\r').collect();
        assert_eq!(parts[0].len(), 76);
        assert!(text.ends_with("\r\r\r"), "the last line's CR, then CR CR");
        assert!(!text.contains('\n'));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn upload_answers() {
        assert!(upload_format_ok("format=tgz;version=dGFy"));
        assert!(upload_format_ok(""), "no format given");
        assert!(!upload_format_ok("format=zip"));
        assert_eq!(UPLOAD_CANCEL, b"\x03");
        assert_eq!(UPLOAD_OK, b"ok\n");
    }
    #[test]
    fn network_paths_are_refused() {
        assert!(local_path_ok("C:\\Users\\me\\bg.png"));
        assert!(local_path_ok("d:/pics/x.jpg"));
        assert!(!local_path_ok("\\\\attacker.example\\s\\x.png"));
        assert!(!local_path_ok("//attacker.example/s/x.png"));
        assert!(!local_path_ok("\\\\?\\C:\\x.png"));
        assert!(!local_path_ok("\\\\.\\pipe\\x"));
        assert!(!local_path_ok("relative\\x.png"));
        assert!(!local_path_ok("\\rooted-no-drive.png"));
        assert!(!local_path_ok("C:relative.png"));
        let unc = serde_json::json!("\\\\attacker\\s\\x.png");
        assert_eq!(valid("Background Image Location", &unc), None);
        assert_eq!(valid("Background Image Folder Location", &unc), None);
        assert!(valid("Background Image Location", &serde_json::json!("C:\\x.png")).is_some());
        assert!(crate::bgimage::load(std::path::Path::new("\\\\127.0.0.1\\nope\\x.png")).is_none());
    }

    #[test]
    fn odd_variable_names_are_refused() {
        assert!(variable_name_ok("user.git_branch"));
        assert!(variable_name_ok("columns"));
        assert!(!variable_name_ok("foo,allow:selection"));
        assert!(!variable_name_ok("a b"));
        assert!(!variable_name_ok(""));
        assert!(!variable_name_ok("x:y"));
    }

    #[test]
    fn open_url_takes_web_and_mail_only() {
        use base64::Engine;
        let b = |u: &str| format!(":{}", base64::engine::general_purpose::STANDARD.encode(u));
        assert!(open_url(&b("https://x.io")).is_some());
        assert!(open_url(&b("ftp://x.io/f")).is_some());
        assert!(open_url(&b("mailto:a@b.c")).is_some());
        assert_eq!(open_url(&b("C:\\Users\\me\\x.xll")), None);
        assert_eq!(open_url(&b("file://localhost/C:/x.xll")), None);
        assert_eq!(open_url(&b("ms-settings:privacy")), None);
    }
}

//! Instant Replay recordings: the `.itr` file Session > Log > Export Recording writes and
//! Import Recording reads (`Codecs/iTermRecordingCodec.m`, `RC`). iTerm2's file is a
//! gzipped NSKeyedArchiver of `{ dvr, profile, version: 4 }`; Termurion's is the gzipped JSON
//! of the same dictionary, with the recorder as `Dvr::to_json` writes it.

use std::io::{Read, Write};

use serde_json::{Map, Value};

use crate::dvr::Dvr;

/// The file's extension (`RC:25,149`).
pub const EXTENSION: &str = "itr";
/// The save dialog's file name (`RC:148`).
pub const DEFAULT_NAME: &str = "Recording";
/// The outer version (`RC:175-181`): 2 added line metadata, 3 moved URL codes, 4 changed
/// the private use area of wide characters.
pub const VERSION: i64 = 4;
/// The save dialog's filter name (Windows needs one; macOS shows none).
pub const FILTER: &str = "Instant Replay recording";

/// Profile keys an exported recording leaves out (`RC:158-173`, key names from
/// `ITAddressBookMgr.h`): private information the recording does not show.
pub const PRIVATE_KEYS: &[&str] = &[
    "Name",
    "Command",
    "Working Directory",
    "Automatically Log",
    "Archive On Closure",
    "Archive Directory",
    "Plain Text Logging",
    "Description",
    "Initial Text",
    "Tags",
    "Title Components",
    "Original Guid",
    "AWDS Window Directory",
    "AWDS Tab Directory",
    "AWDS Pane Directory",
    "Log Directory",
    "Log Filename Format",
    "Show Status Bar",
    "Status Bar Layout",
    "Has Hotkey",
    "Triggers",
    "Triggers Use Interpolated Strings",
    "Smart Selection Actions Use Interpolated Strings",
    "Smart Selection Rules",
    "Semantic History",
    "Bound Hosts",
    "Dynamic Profile Parent Name",
    "Dynamic Profile Parent GUID",
    "Dynamic Profile Filename",
    "Rewritable",
    "Is Dynamic Profile",
    "Bindings",
];

/// A write failure's heading; the error text is the message (`RC:189-197`).
pub const SAVE_FAILED: &str = "The recording could not be saved.";
/// No recorder to write (`RC:199-204`): the menu command during a replay.
pub const ENCODE_FAILED: &str = "Error encoding recording.";
/// `iTermWarning`'s heading when none is given (`iTermWarning.m:330`).
pub const WARNING: &str = "Warning";

/// Why a file could not be loaded, in the order `RC:34-127` checks.
#[derive(Debug, PartialEq)]
pub enum LoadError {
    /// The file could not be read; the error text.
    Read(String),
    /// iTerm2's `inflateInit2` failure (`RC:48-56`), out of memory only. Unreachable with
    /// flate2: a stream that does not inflate goes on to the parse, as iTerm2's
    /// `gunzippedData` returns its buffer whatever `inflate` returned
    /// (`NSData+GZIP.m:103-126`), and fails there as `Unarchive`.
    Decompress,
    /// Not a JSON dictionary.
    Unarchive,
    /// The outer version is not 1 to 4.
    Version,
    /// No recorder or no profile.
    Missing,
    /// The recorder does not load (`Dvr::from_json`).
    Corrupt,
}

/// The profile as a recording carries it: the private keys gone and a new Guid, so it
/// never matches an existing profile (`RC:156-176`).
pub fn redact(profile: &Map<String, Value>, new_guid: &str) -> Map<String, Value> {
    let mut p = profile.clone();
    for k in PRIVATE_KEYS {
        p.remove(*k);
    }
    p.insert(crate::profile::GUID.to_string(), Value::String(new_guid.to_string()));
    p
}

/// The profile keys a recording may set: the ones that only change how it is drawn, as
/// iTerm2 allows (`iTermRecordingProfileSanitizer`). Colors are allowed too, with their
/// Light and Dark variants. A recording comes from anyone, so every other key is dropped.
pub const DRAWING_KEYS: &[&str] = &[
    "Use Separate Colors for Light and Dark Mode",
    "Use Bright Bold",
    "Brighten Bold Text",
    "Harmonize 256 Colors",
    "Use Selected Text Color",
    "Use Underline Color",
    "Use Cursor Guide",
    "Use Tab Color",
    "Smart Cursor Color",
    "Minimum Contrast",
    "Faint Text Alpha",
    "Cursor Boost",
    "HDR Cursor",
    // iTerm2 also allows "HDR Cursor Brightness"; Termurion has no such key yet (the parity plan adds it).
    "Use Active Pane Border",
    "Normal Font",
    "Non Ascii Font",
    "Use Non-ASCII Font",
    "Special Font Config",
    "Horizontal Spacing",
    "Vertical Spacing",
    "Use Bold Font",
    "Use Italic Font",
    "Thin Strokes",
    "ASCII Anti Aliased",
    "Non-ASCII Anti Aliased",
    "ASCII Ligatures",
    "Non-ASCII Ligatures",
    "Draw Powerline Glyphs",
    "Cursor Type",
    "Blinking Cursor",
    "Cursor Shadow",
    "Cursor Hidden When Unfocused",
    "Blink Allowed",
    "Cursor Smooth Slide",
    "Cursor Smooth Blink",
    "Cursor Blink Fade In Duration",
    "Cursor Blink Fade Out Duration",
    "Cursor Blink Fade In Curve",
    "Cursor Blink Fade Out Curve",
    "Cursor Blink Visible Dwell",
    "Cursor Blink Hidden Dwell",
    "Animate Movement",
    "Animate Movement Only in Interactive Apps",
    "Transparency",
    "Initial Use Transparency",
    "Blur",
    "Blur Radius",
    "Only The Default BG Color Uses Transparency",
    "Ambiguous Double Width",
    "Unicode Normalization",
    "Unicode Version",
    "Show Mark Indicators",
    "Use Theme Mark Colors",
    "Timestamps Style",
    "Timestamps Visible",
    "Show Offscreen Command line",
    "Show Offscreen Command line for Current Command",
    "Enable Progress Bars",
    "Progress Bar Height",
    "Progress Bar Color Scheme",
    "Reduce Flicker",
    "Rows",
    "Columns",
    "Scrollback Lines",
    "Unlimited Scrollback",
    // Termurion's own, drawing only. Badge Text is left out (it is an expression).
    "Badge Font",
    "Badge Top Margin",
    "Badge Right Margin",
    "Badge Max Width",
    "Badge Max Height",
    // The effect switches are plain bools. Render Plugins is a list of names that must match a
    // plugin already installed (built in or in the user's folder): the file can carry no shader
    // source or path (`sanitized_profile` accepts only a list of strings).
    "Render Plugins",
    "Effects Claude Enter",
    "Effects Claude Done",
    "Effects Claude Line Color",
    "Effects Claude Live Lines",
    "Effects Claude Prompt",
    "Effects Signals",
    "Effects Jumps",
    "Effects Idle",
];

/// A number held to its key's schema range, as the settings window holds it (a spacing of 0 or a
/// million columns would otherwise size the replay pane from the file's say-so).
fn in_range(key: &str, v: toml::Value) -> toml::Value {
    let Some(s) = crate::schema::find(key) else { return v };
    let (lo, hi) = (s.min_f64().unwrap_or(f64::MIN), s.max_f64().unwrap_or(f64::MAX));
    match v {
        toml::Value::Integer(i) if s.min_f64().is_some() || s.max_f64().is_some() => toml::Value::Integer(i.clamp(lo.max(i64::MIN as f64) as i64, hi.min(i64::MAX as f64) as i64)),
        toml::Value::Float(f) => toml::Value::Float(f.clamp(lo, hi)),
        v => v,
    }
}

/// What an imported recording's profile may carry (`iTermRecordingProfileSanitizer`): only the
/// drawing keys, each with a value legal for its key, plus the deprecated keys that still
/// change drawing, converted to their replacements. Everything else is dropped, so a crafted
/// file can never bring triggers, a command or any other active key.
pub fn sanitized_profile(untrusted: &Map<String, Value>) -> Vec<(String, toml::Value)> {
    let mut out: Vec<(String, toml::Value)> = Vec::new();
    for (key, json) in untrusted {
        let base = crate::bindings::base_key(key);
        if !(crate::bindings::is_color_key(key) || DRAWING_KEYS.contains(&base)) {
            continue;
        }
        if base == crate::render_fx::PROFILE_KEY {
            if let Some(names) = json.as_array().and_then(|a| a.iter().map(|n| n.as_str().map(|n| toml::Value::String(n.into()))).collect::<Option<Vec<_>>>()) {
                out.push((key.clone(), toml::Value::Array(names)));
            }
        } else if let Some(v) = crate::prompted::valid(base, json) {
            out.push((key.clone(), in_range(base, v)));
        }
    }
    let has = |out: &[(String, toml::Value)], k: &str| out.iter().any(|(n, _)| n == k);
    if !has(&out, "Unicode Normalization")
        && let Some(on) = untrusted.get("Use HFS Plus Mapping").and_then(Value::as_bool)
    {
        out.push(("Unicode Normalization".into(), toml::Value::Integer(if on { 3 } else { 0 })));
    }
    if !has(&out, "Use Bold Font")
        && let Some(off) = untrusted.get("Disable Bold").and_then(Value::as_bool)
    {
        out.push(("Use Bold Font".into(), toml::Value::Boolean(!off)));
    }
    out
}

/// The file's bytes: `gzip({ dvr, profile, version: 4 })` (`RC:177-185`).
pub fn encode(dvr: Value, profile: Map<String, Value>) -> Vec<u8> {
    let mut dict = Map::new();
    dict.insert("dvr".into(), dvr);
    dict.insert("profile".into(), Value::Object(profile));
    dict.insert("version".into(), Value::from(VERSION));
    let json = serde_json::to_vec(&Value::Object(dict)).unwrap_or_default();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    // Writing to a Vec cannot fail.
    let _ = gz.write_all(&json);
    gz.finish().unwrap_or_default()
}

/// `gunzippedData` (`NSData+GZIP.m:88-126`): bytes that do not start with gzip's magic
/// are taken as they are; a stream that does not inflate gives what inflated before
/// the error (iTerm2 returns its buffer whatever `inflate` returned), which the parse
/// then refuses.
fn gunzip(bytes: &[u8]) -> Vec<u8> {
    if bytes.len() < 2 || bytes[0] != 0x1f || bytes[1] != 0x8b {
        return bytes.to_vec();
    }
    let mut out = Vec::new();
    // On an error `out` keeps what was read so far.
    let _ = flate2::read::GzDecoder::new(bytes).read_to_end(&mut out);
    out
}

/// Every check of `RC:47-100` but the recorder's own load.
fn parts(bytes: &[u8]) -> Result<(Value, Map<String, Value>), LoadError> {
    let data = gunzip(bytes);
    let Ok(Value::Object(mut dict)) = serde_json::from_slice::<Value>(&data) else { return Err(LoadError::Unarchive) };
    if !dict.get("version").and_then(Value::as_i64).is_some_and(|v| (1..=VERSION).contains(&v)) {
        return Err(LoadError::Version);
    }
    let dvr = dict.remove("dvr").filter(|v| !v.is_null());
    let profile = match dict.remove("profile") {
        Some(Value::Object(p)) => Some(p),
        _ => None,
    };
    match (dvr, profile) {
        (Some(dvr), Some(profile)) => Ok((dvr, profile)),
        _ => Err(LoadError::Missing),
    }
}

/// A file's recorder dictionary and profile, after every check `RC:34-127` makes.
pub fn decode(bytes: &[u8]) -> Result<(Value, Map<String, Value>), LoadError> {
    let (dvr, profile) = parts(bytes)?;
    if Dvr::from_json(&dvr).is_none() {
        return Err(LoadError::Corrupt);
    }
    Ok((dvr, profile))
}

/// `decode` with the recorder loaded (read-only), for Import Recording.
pub fn load(bytes: &[u8]) -> Result<(Dvr, Map<String, Value>), LoadError> {
    let (dvr, profile) = parts(bytes)?;
    let dvr = Dvr::from_json(&dvr).ok_or(LoadError::Corrupt)?;
    Ok((dvr, profile))
}

/// A load failure's warning (`RC:36-120`), each with OK only: its heading and its text.
/// "iTerm2" in the text reads "Termurion".
pub fn warning(e: &LoadError) -> (&'static str, String) {
    const CANT_LOAD: &str = "Can\u{2019}t Load Recording";
    match e {
        LoadError::Read(text) => {
            let text = if text.is_empty() { "Unknown error".to_string() } else { text.clone() };
            ("Could not read the file: its envelope was malformed.", text)
        }
        LoadError::Decompress => (WARNING, "Could not read the file: decompression failed.".into()),
        LoadError::Unarchive => (WARNING, "Could not read the file: unarchiving decompressed data failed.".into()),
        LoadError::Version => (CANT_LOAD, "This recording is from a newer version of Termurion and cannot be replayed in this version.".into()),
        LoadError::Missing => (CANT_LOAD, "This recording could not be loaded because it is missing critical information.".into()),
        LoadError::Corrupt => ("Error Loading Recording", "The recording could not be loaded. It might be corrupted.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dvr::{Decoder, key_bytes};
    use alacritty_terminal::grid::Row;
    use alacritty_terminal::index::Column;
    use alacritty_terminal::term::cell::Cell;
    use serde_json::json;

    fn row(cols: usize, ch: char) -> Row<Cell> {
        let mut r: Row<Cell> = Row::new(cols);
        for x in 0..cols {
            r[Column(x)].c = ch;
        }
        r
    }

    fn gz(bytes: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(bytes).unwrap();
        e.finish().unwrap()
    }

    /// Every frame's rows and info, rebuilt by a decoder.
    fn frames(d: &Dvr) -> Vec<(Vec<Row<Cell>>, crate::dvr::Info)> {
        let mut dec = Decoder::default();
        let mut out = Vec::new();
        if dec.seek(d, 0) {
            loop {
                out.push((dec.rows.clone(), dec.info.unwrap()));
                if !dec.next(d) {
                    break;
                }
            }
        }
        out
    }

    #[test]
    fn recording_file_follows_iterm2() {
        // A three-frame recorder: a key frame, a key frame (the first eligible), a diff.
        let (cols, rows) = (6, 6);
        let a: Vec<Row<Cell>> = (0..rows).map(|y| row(cols, (b'a' + y as u8) as char)).collect();
        let mut b = a.clone();
        b[2] = row(cols, '\u{e9}');
        let mut clean = vec![true; rows];
        clean[2] = false;
        let mut d = Dvr::new(key_bytes(cols, rows) * 50);
        d.append(&a, &[], (1, 0), 100);
        d.append(&a, &vec![true; rows], (2, 1), 200);
        d.append(&b, &clean, (3, 2), 300);
        let mut never = || -> i64 { panic!("the whole range is not restamped") };
        let dvr = d.to_json(100, 300, &mut never);
        assert_eq!(dvr["version"], json!(4));
        assert_eq!(dvr["buffer"]["index"]["2"]["info"]["frameType"], json!(1), "a diff frame stays a diff");

        let mut profile = Map::new();
        for k in PRIVATE_KEYS {
            profile.insert(k.to_string(), json!("secret"));
        }
        profile.insert("Guid".into(), json!("OLD"));
        profile.insert("Columns".into(), json!(80));
        let red = redact(&profile, "NEW");
        for k in PRIVATE_KEYS {
            assert!(!red.contains_key(*k), "{k} is removed");
        }
        assert_eq!(red.get("Guid"), Some(&json!("NEW")));
        assert_eq!(red.get("Columns"), Some(&json!(80)));
        for k in ["Name", "Command", "Working Directory", "Initial Text", "Triggers", "Bindings", "Archive On Closure", "Plain Text Logging", "Rewritable", "Is Dynamic Profile"] {
            assert!(PRIVATE_KEYS.contains(&k), "{k} is one of iTerm2's private keys");
        }
        assert_eq!(PRIVATE_KEYS.len(), 32);

        let bytes = encode(dvr, red.clone());
        assert_eq!(&bytes[..2], &[0x1f, 0x8b], "gzipped");
        let (back, p) = decode(&bytes).unwrap();
        assert_eq!(p, red);
        let loaded = Dvr::from_json(&back).unwrap();
        assert!(loaded.read_only);
        assert!(!loaded.can_clear(false), "a loaded recording is never cleared");
        assert_eq!(loaded.timestamps(), vec![100, 200, 300]);
        assert_eq!(frames(&loaded), frames(&d), "the same rows, cursors and kinds");
        assert_eq!(loaded.ring, d.ring, "the ring as it was");
        let (l2, _) = load(&bytes).unwrap();
        assert_eq!(frames(&l2), frames(&d));
        let mut loaded = loaded;
        assert_eq!(loaded.append(&a, &[], (0, 0), 400), crate::dvr::Appended::ReadOnly, "never records");

        // A part is restamped key frames.
        let mut t = 999;
        let mut now = || {
            t += 1;
            t
        };
        let part = Dvr::from_json(&d.to_json(200, 300, &mut now)).unwrap();
        assert_eq!(part.timestamps(), vec![1000, 1001]);
        assert!(frames(&part).iter().all(|(_, i)| i.kind == crate::dvr::Kind::Key), "every copied frame is a key frame");
        assert_eq!(frames(&part).last().unwrap().0, b);

        // An empty recorder writes a valid, empty recording (iTerm2's DVR is never
        // `empty`, so Export always runs).
        let empty = Dvr::new(1000);
        let e = Dvr::from_json(&empty.to_json(0, 0, &mut never)).unwrap();
        assert!(e.is_empty());
        assert_eq!(e.last_timestamp(), 0);

        // Load failures, in iTerm2's order.
        let file = |v: Value| gz(&serde_json::to_vec(&v).unwrap());
        let good = decode(&bytes).unwrap().0;
        assert_eq!(decode(&[0x1f, 0x8b, 1, 2, 3]), Err(LoadError::Unarchive), "a broken gzip stream is parsed as far as it inflated");
        assert_eq!(decode(&bytes[..bytes.len() / 2]), Err(LoadError::Unarchive), "a truncated recording");
        assert_eq!(decode(b"x"), Err(LoadError::Unarchive), "bytes that are not gzip are read as they are");
        assert_eq!(decode(&gz(b"x")), Err(LoadError::Unarchive));
        assert_eq!(decode(&gz(b"[1]")), Err(LoadError::Unarchive), "not a dictionary");
        assert_eq!(decode(&file(json!({"version": 5, "dvr": good.clone(), "profile": {}}))), Err(LoadError::Version));
        assert_eq!(decode(&file(json!({"dvr": good.clone(), "profile": {}}))), Err(LoadError::Version));
        assert_eq!(decode(&file(json!({"version": 4, "profile": {}}))), Err(LoadError::Missing));
        assert_eq!(decode(&file(json!({"version": 4, "dvr": good.clone()}))), Err(LoadError::Missing));
        let mut no_index = good.clone();
        no_index["buffer"].as_object_mut().unwrap().remove("index");
        let e = Dvr::from_json(&no_index).expect("no index loads as an empty recording");
        assert!(e.is_empty() && e.read_only);
        let mut bad = good.clone();
        bad["capacity"] = json!(0);
        assert_eq!(decode(&file(json!({"version": 4, "dvr": bad, "profile": {}}))), Err(LoadError::Corrupt));
        let mut bad = good.clone();
        bad["version"] = json!(5);
        assert_eq!(decode(&file(json!({"version": 4, "dvr": bad, "profile": {}}))), Err(LoadError::Corrupt), "the inner version");
        let mut bad = good.clone();
        bad["buffer"]["index"]["1"]["lines"] = json!([]);
        assert_eq!(decode(&file(json!({"version": 4, "dvr": bad, "profile": {}}))), Err(LoadError::Corrupt), "rows that do not match the size");
        assert!(decode(&serde_json::to_vec(&json!({"version": 1, "dvr": good, "profile": {}})).unwrap()).is_ok(), "versions 1 to 4, plain bytes too");

        assert_eq!(warning(&LoadError::Version), ("Can\u{2019}t Load Recording", "This recording is from a newer version of Termurion and cannot be replayed in this version.".to_string()));
        assert_eq!(warning(&LoadError::Corrupt), ("Error Loading Recording", "The recording could not be loaded. It might be corrupted.".to_string()));
        assert_eq!(warning(&LoadError::Read("e".into())), ("Could not read the file: its envelope was malformed.", "e".to_string()));
        assert_eq!(warning(&LoadError::Read(String::new())).1, "Unknown error");
        assert_eq!(warning(&LoadError::Missing), ("Can\u{2019}t Load Recording", "This recording could not be loaded because it is missing critical information.".to_string()));
        assert_eq!(warning(&LoadError::Decompress), ("Warning", "Could not read the file: decompression failed.".to_string()));
        assert_eq!(warning(&LoadError::Unarchive), ("Warning", "Could not read the file: unarchiving decompressed data failed.".to_string()));
        assert_eq!((EXTENSION, DEFAULT_NAME, VERSION), ("itr", "Recording", 4));
        assert_eq!(SAVE_FAILED, "The recording could not be saved.");
        assert_eq!(ENCODE_FAILED, "Error encoding recording.");
    }

    fn red() -> Value {
        json!({"Red Component": 1.0, "Green Component": 0.0, "Blue Component": 0.0, "Alpha Component": 1.0})
    }

    fn keys(profile: &Map<String, Value>) -> Vec<String> {
        sanitized_profile(profile).into_iter().map(|(k, _)| k).collect()
    }

    fn one(k: &str) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert(k.to_string(), json!(1));
        m
    }

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn import_drops_every_active_key() {
        let hostile = obj(json!({
            "Triggers": [{"action": "ScriptTrigger", "parameter": "open -a Calculator", "regex": ""}],
            "Triggers Use Interpolated Strings": true,
            "Command": "open -a Calculator",
            "Custom Command": "Yes",
            "Initial Text": "open -a Calculator",
            "Smart Selection Rules": [{"regex": ".*"}],
            "Semantic History": {"action": "command", "text": "x"},
            "Keyboard Map": {"0x61-0x0": {"Action": 35, "Text": "x"}},
            "Bindings": {"Foreground Color": "x"},
            "Answerback String": "x",
            "Badge": "x",
            "Send Code When Idle": true,
            "Working Directory": "/",
            "Log Directory": "/tmp",
            "Background Image Location": "/etc/passwd",
            "Some Future Key": "x",
            "Normal Font": "Menlo-Regular 14",
            "Foreground Color": red(),
            "Background Color (Dark)": red(),
            "Transparency": 0.25,
            "Cursor Type": 1,
            "Columns": 80,
        }));
        let mut got = keys(&hostile);
        got.sort();
        assert_eq!(got, ["Background Color (Dark)", "Columns", "Cursor Type", "Foreground Color", "Normal Font", "Transparency"]);
    }

    #[test]
    fn import_keeps_the_values_of_drawing_keys() {
        let got: std::collections::HashMap<_, _> =
            sanitized_profile(&obj(json!({"Normal Font": "Menlo-Regular 14", "Foreground Color": red(), "Minimum Contrast": 0.5}))).into_iter().collect();
        assert_eq!(got["Normal Font"], toml::Value::String("Menlo-Regular 14".into()));
        assert_eq!(got["Minimum Contrast"], toml::Value::Float(0.5));
        assert_eq!(got["Foreground Color"].get("r"), Some(&toml::Value::Float(1.0)));
    }

    #[test]
    fn import_keeps_termurions_own_drawing_keys() {
        let got: std::collections::HashMap<_, _> = sanitized_profile(&obj(json!({
            "Badge Font": "Menlo-Bold 20",
            "Badge Top Margin": 10.0,
            "Badge Right Margin": 12.0,
            "Badge Max Width": 0.5,
            "Badge Max Height": 0.2,
            "Badge Text": "run something",
            "Badge Color": red(),
            "Effects Idle": false,
            "Effects Claude Done": true,
            "Render Plugins": ["cursor_blaze", "tft"],
        })))
        .into_iter()
        .collect();
        let mut names: Vec<_> = got.keys().map(String::as_str).collect();
        names.sort();
        assert_eq!(names, ["Badge Color", "Badge Font", "Badge Max Height", "Badge Max Width", "Badge Right Margin", "Badge Top Margin", "Effects Claude Done", "Effects Idle", "Render Plugins"]);
        assert_eq!(got["Render Plugins"], toml::Value::Array(vec!["cursor_blaze".into(), "tft".into()]));
    }

    #[test]
    fn import_render_plugins_are_names_only() {
        // Only a list of names (which must match an installed plugin): never a shader, a table or a number.
        for bad in [json!("x"), json!(1), json!([1]), json!([{"shader": "void main(){}"}]), json!([["a"]]), json!(["a", 2])] {
            assert!(keys(&obj(json!({ "Render Plugins": bad }))).is_empty(), "{bad}");
        }
    }

    #[test]
    fn import_drops_an_allowed_key_with_an_illegal_value() {
        assert!(keys(&obj(json!({"Transparency": "not a number", "Normal Font": {"not": "a string"}}))).is_empty());
    }

    #[test]
    fn import_migrates_the_deprecated_drawing_keys() {
        let got: std::collections::HashMap<_, _> = sanitized_profile(&obj(json!({"Use HFS Plus Mapping": true, "Disable Bold": true}))).into_iter().collect();
        assert_eq!(got["Unicode Normalization"], toml::Value::Integer(3));
        assert_eq!(got["Use Bold Font"], toml::Value::Boolean(false));
        assert!(!got.contains_key("Use HFS Plus Mapping") && !got.contains_key("Disable Bold"));
        // A modern key wins over a deprecated one.
        let got: std::collections::HashMap<_, _> =
            sanitized_profile(&obj(json!({"Use HFS Plus Mapping": true, "Unicode Normalization": 1, "Disable Bold": true, "Use Bold Font": true}))).into_iter().collect();
        assert_eq!(got["Unicode Normalization"], toml::Value::Integer(1));
        assert_eq!(got["Use Bold Font"], toml::Value::Boolean(true));
    }

    #[test]
    fn import_numbers_obey_the_schema_range() {
        let got: std::collections::HashMap<_, _> = sanitized_profile(&obj(json!({
            "Horizontal Spacing": 0.0,
            "Vertical Spacing": 1e-30,
            "Columns": 1_000_000_000,
            "Rows": -5,
            "Scrollback Lines": 1_000_000_000_000_i64,
            "Blur Radius": 1e300,
            "Minimum Contrast": 0.5,
            "Transparency": 7.5,
        })))
        .into_iter()
        .collect();
        use toml::Value::{Float, Integer};
        // Out of range clamps, as the settings window does; in range stays.
        assert_eq!(got["Horizontal Spacing"], Float(0.5), "a zero spacing cannot make a zero-width cell");
        assert_eq!(got["Vertical Spacing"], Float(0.5));
        assert_eq!(got["Columns"], Integer(1250), "a huge grid is never allocated");
        assert_eq!(got["Rows"], Integer(1));
        assert_eq!(got["Scrollback Lines"], Integer(10_000_000));
        assert_eq!(got["Blur Radius"], Float(64.0));
        assert_eq!(got["Minimum Contrast"], Float(0.5));
        assert_eq!(got["Transparency"], Float(1.0));
    }

    #[test]
    fn import_decides_every_profile_key() {
        // Like iTerm2: a key added to the schema must be allowed or ruled out on purpose.
        let denied: Vec<&str> = include_str!("../schema/profile.toml")
            .lines()
            .filter_map(|l| l.strip_prefix("key = \"")?.strip_suffix('"'))
            .filter(|k| !crate::bindings::is_color_key(k) && !DRAWING_KEYS.contains(k))
            .collect();
        for k in &denied {
            assert!(!keys(&one(k)).iter().any(|g| g == k), "{k} must not import");
        }
        for k in DRAWING_KEYS {
            assert!(crate::schema::find(k).is_some(), "{k} is not a profile key");
        }
    }
}

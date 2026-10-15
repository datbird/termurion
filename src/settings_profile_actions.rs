//! The Profiles list's "Other Actions\u{2026}" menu: Set as Default, Duplicate Profile, Bulk Copy from
//! Selected Profile\u{2026} (and its sheet), Save Profile as JSON\u{2026}, Save All Profiles as JSON\u{2026} and
//! Import JSON Profiles\u{2026} (`ProfilePreferencesViewController.m`, `BulkCopyProfilePreferencesWindowController.m`).

use serde_json::Value as Json;
use toml::Value;

use crate::profile::Profile;
use crate::schema;
use crate::store::Store;

pub const OTHER_ACTIONS: &str = "Other Actions\u{2026}";
pub const SET_AS_DEFAULT: &str = "Set as Default";
pub const DUPLICATE_PROFILE: &str = "Duplicate Profile";
pub const BULK_COPY: &str = "Bulk Copy from Selected Profile\u{2026}";
pub const SAVE_JSON: &str = "Save Profile as JSON\u{2026}";
pub const SAVE_ALL_JSON: &str = "Save All Profiles as JSON\u{2026}";
pub const IMPORT_JSON: &str = "Import JSON Profiles\u{2026}";

/// The Bulk Copy sheet's strings (`BulkCopyProfilePreferences.xib`).
pub const TO_THESE: &str = "To these profiles:";
pub const COPY: &str = "Copy";
pub const CANCEL: &str = "Cancel";
/// The sheet's categories in its order. iTerm2 also has Web; Termurion's Browser rows are on General.
pub const CATEGORIES: [&str; 7] = ["Colors", "Text", "Window", "Terminal", "Keys", "Session", "Advanced"];

/// The label above the checkboxes (`BulkCopy.CopyFromProfileLabel`).
pub fn copy_label(name: &str) -> String {
    format!("Copy these settings from profile \u{201c}{name}\u{201d}:")
}

/// The keys one category copies (`keysForBulkCopy`): its tab's rows, but not a hotkey (a copy would
/// make a second profile claim the same hotkey window), and, when the source keeps separate
/// light and dark colors, the Colors rows as their two variants (`shouldAmendKey:`).
pub fn keys_for(category: &str, source_separate: bool) -> Vec<String> {
    let mut keys = Vec::new();
    for s in schema::profile().iter().filter(|s| s.tab == category) {
        if s.key == "Has Hotkey" || s.key == "Session Hotkey" || s.key.starts_with("HotKey ") || s.key == "Bindings" {
            continue;
        }
        let plain = matches!(s.key.as_str(), "Use Separate Colors for Light and Dark Mode" | "Use Theme Mark Colors");
        if category == "Colors" && source_separate && !plain {
            keys.push(format!("{} (Light)", s.key));
            keys.push(format!("{} (Dark)", s.key));
        } else {
            keys.push(s.key.clone());
        }
    }
    keys
}

/// Copies the checked categories of profile `source` onto each of `dests` (`performBulkCopy:`): a
/// key the source lacks is removed from the destination. Returns how many profiles were written.
pub fn bulk_copy(store: &mut Store, source: &str, dests: &[String], categories: &[&str]) -> usize {
    let Some(src) = store.get(source).cloned() else { return 0 };
    let separate = src.bool("Use Separate Colors for Light and Dark Mode");
    let keys: Vec<String> = categories.iter().flat_map(|c| keys_for(c, separate)).collect();
    let mut written = 0;
    for d in dests {
        if d == source {
            continue;
        }
        let Some(p) = store.get_mut(d) else { continue };
        for k in &keys {
            match src.values.get(k) {
                Some(v) => p.set(k, v.clone()),
                None => {
                    p.remove(k);
                }
            }
        }
        written += 1;
    }
    written
}

/// A profile as JSON (`jsonForProfile:`), pretty printed.
pub fn profile_json(p: &Profile) -> String {
    serde_json::to_string_pretty(&Json::Object(profile_object(p))).unwrap_or_default()
}

fn profile_object(p: &Profile) -> serde_json::Map<String, Json> {
    p.values.iter().filter_map(|(k, v)| Some((k.clone(), serde_json::from_str(&crate::api_profile::json_of(k, v)?).ok()?))).collect()
}

/// Every profile as `{"Profiles": [...]}` (`jsonForAllProfilesWithErrorCount:`).
pub fn all_json(list: &[Profile]) -> String {
    let profiles: Vec<Json> = list.iter().map(|p| Json::Object(profile_object(p))).collect();
    let mut root = serde_json::Map::new();
    root.insert("Profiles".into(), Json::Array(profiles));
    serde_json::to_string_pretty(&Json::Object(root)).unwrap_or_default()
}

/// The profiles in one JSON file (`tryToImportJSONProfileFromURL:`): a profile object, or an
/// object whose "Profiles" is a list of them. A Guid already taken (`taken`) is replaced by a fresh
/// one. The errors are iTerm2's wording.
pub fn import_json(text: &str, taken: &dyn Fn(&str) -> bool) -> Result<Vec<Profile>, String> {
    let root: Json = serde_json::from_str(text).map_err(|_| "The data couldn\u{2019}t be read because it isn\u{2019}t in the correct format.".to_string())?;
    let Json::Object(dict) = &root else { return Err("Invalid JSON: does not have a top-level dictionary.".into()) };
    let one = |obj: &serde_json::Map<String, Json>| -> Option<Profile> {
        let Some(Value::Table(t)) = crate::import::from_json(&Json::Object(obj.clone())) else { return None };
        let mut p = Profile::from_values(t.into_iter().collect());
        if taken(&p.guid()) {
            p.renew_guid();
        }
        Some(p)
    };
    let named = |obj: &serde_json::Map<String, Json>| obj.get("Name").is_some_and(Json::is_string) && obj.get("Guid").is_some_and(Json::is_string);
    if named(dict) {
        return one(dict).map(|p| vec![p]).ok_or_else(|| "Invalid JSON: missing Name or Guid key in top-level dictionary.".to_string());
    }
    let Some(Json::Array(list)) = dict.get("Profiles") else { return Err("Invalid JSON: missing Name or Guid key in top-level dictionary.".into()) };
    let mut out: Vec<Profile> = Vec::new();
    for (i, item) in list.iter().enumerate() {
        let Json::Object(obj) = item else { return Err("Invalid JSON: not an array of dictionaries.".into()) };
        if !named(obj) {
            return Err(format!("Invalid JSON: missing Name or Guid key in profile number {}.", i + 1));
        }
        let Some(p) = one(obj) else { return Err(format!("Invalid JSON: missing Name or Guid key in profile number {}.", i + 1)) };
        out.push(p);
    }
    // Two profiles of one file may share a Guid, and one may match an earlier one of the file.
    let mut seen: Vec<String> = Vec::new();
    for p in &mut out {
        if seen.contains(&p.guid()) {
            p.renew_guid();
        }
        seen.push(p.guid());
    }
    Ok(out)
}

/// The failure alert's heading and text for a file that did not import (`ProfilePreferences.ImportFailedFormat`).
pub fn import_failed(path: &str, error: &str) -> (String, String) {
    ("Could not Import Profile".to_string(), format!("Import from {path} failed: {error}"))
}

/// The text of the alert for a file that could not be written (`ProfilePreferences.CouldntSaveFormat`).
pub fn save_failed(file: &str, host: &str, error: &str) -> String {
    format!("Couldn\u{2019}t save to \u{201c}{file}\u{201d} on {host}: {error}")
}

/// A file name for a profile's JSON: the name with the characters Windows refuses in a file name replaced.
pub fn file_name_for(profile_name: &str) -> String {
    let cleaned: String = profile_name.chars().map(|c| if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() { '_' } else { c }).collect();
    let cleaned = cleaned.trim().trim_end_matches('.').to_string();
    if cleaned.is_empty() { "UnknownProfile".to_string() } else { cleaned }
}

/// The largest JSON file an import reads.
pub const MAX_JSON: usize = 16 * 1024 * 1024;

/// Asks where to save and writes `text` there (`saveProfileAsJSON:`, `saveAllProfilesAsJSON:`), as a
/// new file readable by this user alone moved over the target (`settings_triggers::write_export`). A
/// saved file is shown in Explorer through `reveal`; a failure is the alert iTerm2 shows.
pub fn save_json(default_name: &str, text: &str, reveal: &mut Option<String>) -> crate::settings_ui::Edit {
    use crate::settings_ui::Edit;
    let name = match default_name.strip_suffix(".json") {
        Some(stem) => format!("{}.json", file_name_for(stem)),
        None => file_name_for(default_name),
    };
    let dialog = rfd::FileDialog::new().set_directory(crate::profile::home()).set_file_name(name).add_filter("JSON", &["json"]);
    let Some(path) = crate::localfs::save_file(dialog) else { return Edit::None };
    match crate::settings_triggers::write_export(&path, text) {
        Ok(()) => {
            *reveal = Some(path.path().to_string_lossy().into_owned());
            Edit::None
        }
        Err(e) => {
            let file = crate::inert_files::shown_name(&path.path().file_name().map_or_else(String::new, |f| f.to_string_lossy().into_owned()));
            let host = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "this computer".to_string());
            Edit::Alert { heading: "Error".to_string(), text: save_failed(&file, &host, &e.to_string()) }
        }
    }
}

/// A JSON file the person picked, at most `MAX_JSON` bytes, as UTF-8.
fn read_json(p: &crate::localfs::LocalPath) -> Result<String, String> {
    use std::io::Read;
    let file = crate::localfs::open(p).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(MAX_JSON as u64 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_JSON {
        return Err("the file is too large".to_string());
    }
    String::from_utf8(bytes).map_err(|_| "the file is not valid UTF-8".to_string())
}

/// Asks for JSON files and imports each (`importJSONProfiles:`); a file that fails queues its alert
/// (`import_failed`), and the others still import.
pub fn import_json_files(store: &mut Store, alerts: &mut std::collections::VecDeque<(String, String)>) -> crate::settings_ui::Edit {
    use crate::settings_ui::Edit;
    let dialog = rfd::FileDialog::new().set_directory(crate::profile::home()).add_filter("JSON", &["json"]);
    let mut imported = 0;
    for file in crate::localfs::pick_files(dialog) {
        let shown = crate::inert_files::shown_name(&file.path().to_string_lossy());
        let text = match read_json(&file) {
            Ok(t) => t,
            Err(e) => {
                alerts.push_back(import_failed(&shown, &e));
                continue;
            }
        };
        match import_json(&text, &|g| store.get(g).is_some()) {
            Ok(list) => {
                imported += list.len();
                for p in list {
                    store.add(p);
                }
            }
            Err(e) => alerts.push_back(import_failed(&shown, &e)),
        }
    }
    if imported > 0 { Edit::Imported(imported) } else { Edit::None }
}

/// The Bulk Copy sheet while it is open.
pub struct BulkCopy {
    /// The profile the settings come from.
    pub source: String,
    checks: [bool; 7],
    list: crate::profile_list_ui::ListState,
}

/// What the person did with the sheet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BulkOutcome {
    Open,
    Cancel,
    /// Copy: the destinations and the checked categories.
    Copy(Vec<String>, Vec<&'static str>),
}

impl BulkCopy {
    pub fn new(source: &str) -> BulkCopy {
        BulkCopy { source: source.to_string(), checks: [false; 7], list: Default::default() }
    }

    /// The checked categories, in the sheet's order.
    pub fn checked(&self) -> Vec<&'static str> {
        CATEGORIES.iter().zip(self.checks).filter(|(_, on)| *on).map(|(c, _)| *c).collect()
    }
}

/// The sheet (`BulkCopyProfilePreferences.xib`): the label, one checkbox per category, "To these
/// profiles:" with a multi-select profile list, and Copy and Cancel.
pub fn show(ctx: &egui::Context, sheet: &mut BulkCopy, rows: &[(String, String, Vec<String>, String)], default_guid: &str) -> BulkOutcome {
    let mut out = BulkOutcome::Open;
    let name = rows.iter().find(|r| r.0 == sheet.source).map(|r| r.1.clone()).unwrap_or_default();
    egui::Window::new("Bulk Copy").title_bar(false).collapsible(false).resizable(false).default_width(480.0).show(ctx, |ui| {
        ui.label(copy_label(&crate::profile_list::display_text(&name)));
        ui.horizontal_wrapped(|ui| {
            for (i, c) in CATEGORIES.iter().enumerate() {
                ui.checkbox(&mut sheet.checks[i], *c);
            }
        });
        ui.add_space(6.0);
        ui.label(TO_THESE);
        crate::profile_list_ui::profile_list(ui, &mut sheet.list, rows, default_guid, true, None);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button(COPY).clicked() {
                out = BulkOutcome::Copy(sheet.list.selection.clone(), sheet.checked());
            }
            if ui.button(CANCEL).clicked() {
                out = BulkOutcome::Cancel;
            }
        });
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with(names: &[&str]) -> Store {
        let mut s = Store::fresh(std::path::Path::new("bulk-copy-test.toml"));
        s.profiles.clear();
        for n in names {
            s.add(Profile::new(n));
        }
        s.default_guid = s.profiles[0].guid();
        s
    }

    #[test]
    fn bulk_copy_writes_the_checked_categories_and_removes_what_the_source_lacks() {
        let mut store = store_with(&["Src", "Dst", "Other"]);
        let guids: Vec<String> = store.profiles.iter().map(Profile::guid).collect();
        store.get_mut(&guids[0]).unwrap().set("Scrollback Lines", Value::Integer(777));
        store.get_mut(&guids[1]).unwrap().set("Silence Bell", Value::Boolean(true));
        store.get_mut(&guids[1]).unwrap().set("Columns", Value::Integer(99));
        // The source has no stored Silence Bell, so the destination's goes with the Terminal category.
        store.get_mut(&guids[0]).unwrap().remove("Silence Bell");
        let n = bulk_copy(&mut store, &guids[0], &[guids[1].clone(), guids[0].clone()], &["Terminal"]);
        assert_eq!(n, 1, "the source itself is skipped");
        assert_eq!(store.get(&guids[1]).unwrap().int("Scrollback Lines"), 777);
        assert!(!store.get(&guids[1]).unwrap().values.contains_key("Silence Bell"));
        assert_eq!(store.get(&guids[1]).unwrap().int("Columns"), 99, "Window was not checked");
        assert_eq!(store.get(&guids[2]).unwrap().int("Scrollback Lines"), Profile::new("x").int("Scrollback Lines"), "an unselected profile is left alone");
    }

    #[test]
    fn colors_copy_as_light_and_dark_variants_when_the_source_has_them() {
        let plain = keys_for("Colors", false);
        let split = keys_for("Colors", true);
        assert!(plain.contains(&"Foreground Color".to_string()));
        assert!(split.contains(&"Foreground Color (Light)".to_string()) && split.contains(&"Foreground Color (Dark)".to_string()));
        assert!(!split.contains(&"Foreground Color".to_string()));
        assert!(split.contains(&"Use Separate Colors for Light and Dark Mode".to_string()), "the switch itself has no variants");
    }

    #[test]
    fn hotkeys_are_never_bulk_copied() {
        assert!(keys_for("Keys", false).iter().all(|k| !k.starts_with("HotKey ") && k != "Has Hotkey"));
        assert!(keys_for("Keys", false).contains(&"Keyboard Map".to_string()));
    }

    #[test]
    fn json_round_trips_a_profile_and_a_list() {
        let mut p = Profile::new("Round");
        p.set("Scrollback Lines", Value::Integer(1234));
        let one = import_json(&profile_json(&p), &|_| false).unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].name(), "Round");
        assert_eq!(one[0].int("Scrollback Lines"), 1234);
        assert_eq!(one[0].guid(), p.guid());
        let q = Profile::new("Second");
        let many = import_json(&all_json(&[p.clone(), q.clone()]), &|g| g == p.guid()).unwrap();
        assert_eq!(many.iter().map(Profile::name).collect::<Vec<_>>(), ["Round", "Second"]);
        assert_ne!(many[0].guid(), p.guid(), "a Guid already in use is replaced");
        assert_eq!(many[1].guid(), q.guid());
    }

    #[test]
    fn json_errors_are_iterms() {
        let none = |_: &str| false;
        assert_eq!(import_json("[1]", &none).unwrap_err(), "Invalid JSON: does not have a top-level dictionary.");
        assert_eq!(import_json("{\"a\": 1}", &none).unwrap_err(), "Invalid JSON: missing Name or Guid key in top-level dictionary.");
        assert_eq!(import_json("{\"Profiles\": [1]}", &none).unwrap_err(), "Invalid JSON: not an array of dictionaries.");
        assert_eq!(import_json("{\"Profiles\": [{\"Name\": \"a\", \"Guid\": \"1\"}, {\"Name\": \"b\"}]}", &none).unwrap_err(), "Invalid JSON: missing Name or Guid key in profile number 2.");
        assert!(import_json("not json", &none).is_err());
        assert_eq!(import_failed("C:\\p.json", "x"), ("Could not Import Profile".to_string(), "Import from C:\\p.json failed: x".to_string()));
    }

    #[test]
    fn a_profile_name_makes_a_file_name() {
        assert_eq!(file_name_for("Work: prod/east"), "Work_ prod_east");
        assert_eq!(file_name_for("  "), "UnknownProfile");
        assert_eq!(file_name_for("plain"), "plain");
    }

    #[test]
    fn the_sheet_label_names_the_source() {
        assert_eq!(copy_label("Default"), "Copy these settings from profile \u{201c}Default\u{201d}:");
    }
}

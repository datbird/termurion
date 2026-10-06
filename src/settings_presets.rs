//! The Colors tab's color presets (`ProfilesColorsPreferencesViewController.m`): the presets
//! menu with its check marks, the hover preview, the "Update Which Modes?" questions, loading a
//! preset into the edited mode, Import, Export, Save As, Delete and the gallery link, and the
//! "Editing:" selector's state. The model is `color_presets`; the drawing is `colors_header` and
//! `pump_dialogs`.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use toml::Value;

use crate::color_presets::{self as cp, Modes, Preset};
use crate::globals::Globals;
use crate::profile::Profile;
use crate::warnings::{Kind, Spec};

const SEPARATE: &str = "Use Separate Colors for Light and Dark Mode";

/// `kColorGalleryURL`: opened in the default browser through the settings window's `open_url`.
pub const GALLERY_URL: &str = "https://www.iterm2.com/colorgallery";
/// A hover this long previews a preset (`menu:willHighlightItem:`).
pub const PREVIEW_DELAY: Duration = Duration::from_millis(400);
/// A multi-pick in Import stops here (B3).
pub const MAX_IMPORT_FILES: usize = 32;
/// "Similar" Save As names stop above this difference (`mostSimilarCustomPresetName`).
pub const SIMILAR_LIMIT: f64 = 5.0;

pub const IMPORT_FAILED_HEADING: &str = "Import Failed.";
pub const IMPORT_FAILED_TEXT: &str = "The selected file could not be read or did not contain a valid color scheme.";
pub const DUPLICATE_HEADING: &str = "Add duplicate color preset?";
pub const ADD_ANYWAY: &str = "Add it anyway";
pub const SAVE_FAILED_HEADING: &str = "Save Failed.";
pub const SAVE_TITLE: &str = "Save Color Preset";
pub const SAVE_PROMPT: &str = "Select preset name";
pub const DELETE_PROMPT: &str = "Select a preset to delete:";
pub const NO_DELETABLE_HEADING: &str = "No deletable color presets.";
pub const NO_DELETABLE_TEXT: &str = "You cannot erase the built-in presets and no custom presets have been imported.";
pub const UPDATE_WHICH_HEADING: &str = "Update Which Modes?";
pub const HAS_MODES_TEXT: &str = "This preset has colors for both light mode and dark mode.";
pub const LACKS_MODES_TEXT: &str = "This preset does not have separate colors for light mode and dark mode.";
pub const HAS_MODES_ID: &str = "NoSyncUpdateWhichModes_PresetHasModes";
pub const LACKS_MODES_ID: &str = "NoSyncUpdateWhichModes_PresetLacksModes";

/// "The color preset \u{201c}%@\u{201d} is the same as..." (`ColorPresets.DuplicateInformative`).
pub fn duplicate_text(name: &str) -> String {
    format!("The color preset \u{201c}{name}\u{201d} is the same as the preset you're trying to add. Really add it?")
}

/// "Could not save to %@".
pub fn could_not_save_text(path: &str) -> String {
    format!("Could not save to {path}")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Off,
    On,
    Mixed,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuEntry {
    Preset { name: String, title: String, mark: Mark },
    Separator,
    Import,
    Export,
    SaveAs,
    Delete,
    Gallery,
}

impl MenuEntry {
    /// The title of a command row.
    pub fn command_title(&self) -> Option<&'static str> {
        match self {
            MenuEntry::Import => Some("Import\u{2026}"),
            MenuEntry::Export => Some("Export\u{2026}"),
            MenuEntry::SaveAs => Some("Save As\u{2026}"),
            MenuEntry::Delete => Some("Delete Preset\u{2026}"),
            MenuEntry::Gallery => Some("Visit Online Gallery"),
            _ => None,
        }
    }
}

/// Every preset by name, a custom one winning a clash (`allColorPresets`), borrowed.
fn all_presets(g: &Globals) -> BTreeMap<&str, &Preset> {
    let mut all: BTreeMap<&str, &Preset> = cp::built_in().iter().map(|(n, p)| (n.as_str(), p)).collect();
    all.extend(cp::custom(g).iter().map(|(n, p)| (n.as_str(), p)));
    all
}

/// `addColorPresetsInDict:toMenu:`: presets with modes first (the yin-yang mark), a separator,
/// then the rest, each sorted by name.
fn section(presets: &[(String, Preset)]) -> Vec<(String, String)> {
    let mut sorted: Vec<&(String, Preset)> = presets.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out: Vec<(String, String)> = Vec::new();
    let mut modal = 0;
    for (name, p) in sorted.iter().filter(|(_, p)| cp::has_modes(p)).map(|x| (&x.0, &x.1)) {
        let _ = p;
        out.push((name.clone(), format!("\u{262f} {name}")));
        modal += 1;
    }
    if modal > 0 {
        out.push((String::new(), String::new()));
    }
    for (name, p) in sorted.iter().map(|x| (&x.0, &x.1)) {
        if !cp::has_modes(p) {
            out.push((name.clone(), name.clone()));
        }
    }
    out
}

/// `rebuildColorPresetsMenu` and `popupButtonWillPopUp:` (the check marks).
pub fn menu(g: &Globals, profile: &Profile, _editing_dark: bool) -> Vec<MenuEntry> {
    let all = all_presets(g);
    let mut entries: Vec<MenuEntry> = Vec::new();
    let add = |entries: &mut Vec<MenuEntry>, rows: Vec<(String, String)>| {
        for (name, title) in rows {
            if name.is_empty() {
                entries.push(MenuEntry::Separator);
            } else {
                entries.push(MenuEntry::Preset { name, title, mark: Mark::Off });
            }
        }
    };
    add(&mut entries, section(cp::built_in()));
    let custom = cp::custom(g);
    if !custom.is_empty() {
        entries.push(MenuEntry::Separator);
        add(&mut entries, section(custom));
    }
    // The check marks: the first exact match is On; with separate colors and no exact match, the
    // presets equal in one mode are Mixed.
    let separate = profile.bool(SEPARATE);
    let mut found = false;
    for e in entries.iter_mut() {
        if let MenuEntry::Preset { name, mark, .. } = e {
            let Some(p) = all.get(name.as_str()) else { continue };
            if !found && cp::matches(p, profile, None) {
                *mark = Mark::On;
                found = true;
            }
        }
    }
    if !found && separate {
        for e in entries.iter_mut() {
            if let MenuEntry::Preset { name, mark, .. } = e {
                let Some(p) = all.get(name.as_str()) else { continue };
                if cp::matches(p, profile, Some(true)) || cp::matches(p, profile, Some(false)) {
                    *mark = Mark::Mixed;
                }
            }
        }
    }
    entries.push(MenuEntry::Separator);
    entries.extend([MenuEntry::Import, MenuEntry::Export, MenuEntry::SaveAs, MenuEntry::Delete, MenuEntry::Gallery]);
    entries
}

/// Which of the two questions a load asks (`shouldUpdateBothModesWhenLoadingPreset:`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Question {
    None,
    HasModes,
    LacksModes,
}

pub fn question(preset_has_modes: bool, separate: bool) -> Question {
    match (preset_has_modes, separate) {
        (true, true) => Question::HasModes,
        (false, true) => Question::LacksModes,
        _ => Question::None,
    }
}

/// Whether a load that asks nothing updates both modes: a preset with modes into a profile
/// without separate colors does.
pub fn both_without_asking(preset_has_modes: bool) -> bool {
    preset_has_modes
}

/// The question as a settings warning (P21: `Kind::Persistent`, never remembered). The second
/// button names the edited mode.
pub fn question_spec(q: Question, editing_dark: bool) -> Option<Spec> {
    let (id, text) = match q {
        Question::None => return None,
        Question::HasModes => (HAS_MODES_ID, HAS_MODES_TEXT),
        Question::LacksModes => (LACKS_MODES_ID, LACKS_MODES_TEXT),
    };
    let only = if editing_dark { "Update Dark Mode Only" } else { "Update Light Mode Only" };
    Some(Spec {
        id: Some(id.to_string()),
        kind: Kind::Persistent,
        heading: UPDATE_WHICH_HEADING.to_string(),
        text: text.to_string(),
        labels: vec!["Update Both Modes".to_string(), only.to_string()],
        cancel: None,
        never_remember: Vec::new(),
    })
}

/// Whether a warning id is one of the two questions.
pub fn is_question_id(id: Option<&str>) -> bool {
    matches!(id, Some(HAS_MODES_ID) | Some(LACKS_MODES_ID))
}

/// `loadColorPresetWithName:prompt:` after the question: both modes load light into light then
/// dark into dark and turn separate colors on; with separate colors on the edited mode loads;
/// otherwise the system mode's colors go into the base keys.
pub fn load(p: &Preset, profile: &mut Profile, both: bool, editing_dark: bool, system_dark: bool) {
    let mode = |dark: bool| if dark { Modes::DARK_ONLY } else { Modes::LIGHT_ONLY };
    if both {
        cp::apply(p, profile, Modes::LIGHT_ONLY, Modes::LIGHT_ONLY, false);
        cp::apply(p, profile, Modes::DARK_ONLY, Modes::DARK_ONLY, false);
        profile.set(SEPARATE, Value::Boolean(true));
        return;
    }
    if profile.bool(SEPARATE) {
        cp::apply(p, profile, mode(editing_dark), mode(editing_dark), false);
    } else {
        cp::apply(p, profile, mode(system_dark), Modes::NONE, false);
    }
}

/// `mostSimilarCustomPresetName`: the custom preset with the least difference from the current
/// colors, None above `SIMILAR_LIMIT`. The pair is (current, custom), as iTerm2 passes it.
pub fn most_similar(g: &Globals, current: &Preset) -> Option<String> {
    // `custom` is sorted by name.
    let mut best: Option<(&str, f64)> = None;
    for (name, p) in cp::custom(g) {
        let d = cp::difference(current, p);
        if best.as_ref().is_none_or(|(_, b)| d < *b) {
            best = Some((name, d));
        }
    }
    best.filter(|(_, d)| *d <= SIMILAR_LIMIT).map(|(n, _)| n.to_string())
}

/// The Editing selector is enabled only with separate colors (`PCP:370-384`).
pub fn editing_enabled(separate: bool) -> bool {
    separate
}

/// The "Editing:" selector's state: it starts from Windows' mode and follows a change of it until
/// the person picks one (P13).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Editing {
    pub dark: bool,
    picked: bool,
}

impl Editing {
    pub fn new(system_dark: bool) -> Editing {
        Editing { dark: system_dark, picked: false }
    }
    pub fn follow(&mut self, system_dark: bool) {
        if !self.picked {
            self.dark = system_dark;
        }
    }
    pub fn pick(&mut self, dark: bool) {
        self.dark = dark;
        self.picked = true;
    }
}

/// Every key a preset load can change, with its value now: restoring puts them back exactly.
fn snapshot(profile: &Profile) -> Vec<(String, Option<Value>)> {
    let mut keys = cp::keys_with_modes(false);
    keys.extend(cp::keys_with_modes(true));
    keys.push(SEPARATE.to_string());
    keys.into_iter().map(|k| {
        let v = profile.values.get(&k).cloned();
        (k, v)
    }).collect()
}

/// A preview in progress: the colors as they were (`_savedColors`), when it began and the preset
/// shown. It lives in the window, never in the store's saved file (B2): only `Edit::Preview` is
/// returned while it shows.
#[derive(Clone, Debug)]
pub struct Preview {
    pub saved: Preset,
    pub since: Instant,
    pub name: String,
    guid: String,
    values: Vec<(String, Option<Value>)>,
}

impl Preview {
    pub fn begin(profile: &Profile, guid: &str, editing_dark: bool, name: &str) -> Preview {
        Preview { saved: cp::current(profile, editing_dark), since: Instant::now(), name: name.to_string(), guid: guid.to_string(), values: snapshot(profile) }
    }

    pub fn guid(&self) -> &str {
        &self.guid
    }

    /// `restoreColors`.
    pub fn restore(&self, profile: &mut Profile) {
        for (k, v) in &self.values {
            match v {
                Some(v) => profile.set(k, v.clone()),
                None => {
                    profile.values.remove(k);
                }
            }
        }
    }
}

/// What a click on a preset row does.
#[derive(Debug, PartialEq)]
pub enum Click {
    /// The preset loaded without asking.
    Loaded,
    /// Ask which modes first (the settings window shows the warning).
    Ask(Spec),
    /// No such preset.
    Missing,
}

/// A click on a preset row: loads it, or names the question to ask, remembering what to load.
pub fn click(st: &mut State, name: &str, g: &Globals, profile: &mut Profile, system_dark: bool) -> Click {
    let Some(p) = all_presets(g).get(name).copied() else { return Click::Missing };
    let has = cp::has_modes(p);
    let q = question(has, profile.bool(SEPARATE));
    match question_spec(q, st.editing.dark) {
        None => {
            load(p, profile, both_without_asking(has), st.editing.dark, system_dark);
            Click::Loaded
        }
        Some(spec) => {
            st.pending = Some((name.to_string(), has));
            Click::Ask(spec)
        }
    }
}

/// A dialog the colors tab shows over the window.
#[derive(Clone, Debug)]
enum Dialog {
    Alert { heading: String, text: String },
    Duplicate { name: String, original: String, preset: Preset },
    SaveAs { name: String, names: Vec<String> },
    Delete { names: Vec<String>, chosen: usize },
}

/// What the colors tab remembers.
#[derive(Debug)]
pub struct State {
    pub editing: Editing,
    /// A live preview: restored when the menu closes, the window closes or the app quits.
    pub preview: Option<Preview>,
    /// The profile whose colors header was drawn this frame (set by `colors_header`, taken by
    /// `strand_check`).
    pub shown: Option<String>,
    /// The row hovered and since when (`_timer`).
    hover: Option<(String, Instant)>,
    /// The menu as it was when it opened (`popupButtonWillPopUp:` marks once).
    entries: Option<Vec<MenuEntry>>,
    /// The preset a question is about: (name, has modes). P21.
    pub pending: Option<(String, bool)>,
    dialog: Option<Dialog>,
    queue: VecDeque<(String, Preset)>,
    /// A file to show in Explorer; the app takes it after each frame.
    pub reveal: Option<String>,
}

impl State {
    pub fn new(system_dark: bool) -> State {
        State { editing: Editing::new(system_dark), preview: None, shown: None, hover: None, entries: None, pending: None, dialog: None, queue: VecDeque::new(), reveal: None }
    }

    /// Puts a live preview's colors back (B2). True when the profile changed.
    pub fn restore(&mut self, store: &mut crate::store::Store) -> bool {
        self.hover = None;
        let Some(pre) = self.preview.take() else { return false };
        if let Some(p) = store.get_mut(pre.guid()) {
            pre.restore(p);
        }
        true
    }

    /// After a frame (B2): a preview whose header was not drawn for its profile (the tab, page or
    /// profile changed, the menu is gone) is put back. True when a profile changed.
    pub fn strand_check(&mut self, store: &mut crate::store::Store) -> bool {
        let shown = self.shown.take();
        match &self.preview {
            Some(pre) if shown.as_deref() != Some(pre.guid()) => self.restore(store),
            _ => false,
        }
    }

    /// Whether a dialog is showing.
    pub fn busy(&self) -> bool {
        self.dialog.is_some() || !self.queue.is_empty()
    }
}

/// What the colors header did this frame.
#[derive(Default)]
pub struct Header {
    /// The profile's colors changed without being committed (a preview or its restore).
    pub previewed: bool,
    /// A preset row was clicked.
    pub clicked: Option<String>,
    pub import: bool,
    pub export: bool,
    pub save_as: bool,
    pub delete: bool,
    pub gallery: bool,
    /// The Editing combo was changed.
    pub picked: bool,
}

/// The row above the color wells: "Editing:" with its combo (enabled with separate colors) and the
/// "Color Presets\u{2026}" menu. The preview timer runs while a preset row is hovered.
pub fn colors_header(ui: &mut egui::Ui, st: &mut State, g: &Globals, profile: &mut Profile, guid: &str, system_dark: bool) -> Header {
    let mut out = Header::default();
    st.shown = Some(guid.to_string());
    let separate = profile.bool(SEPARATE);
    ui.horizontal(|ui| {
        ui.label("Editing:");
        ui.add_enabled_ui(editing_enabled(separate), |ui| {
            let mut dark = st.editing.dark;
            egui::ComboBox::from_id_salt("editing-mode").selected_text(if dark { "Dark Mode" } else { "Light Mode" }).show_ui(ui, |ui| {
                ui.selectable_value(&mut dark, false, "Light Mode");
                ui.selectable_value(&mut dark, true, "Dark Mode");
            });
            if dark != st.editing.dark {
                st.editing.pick(dark);
                out.picked = true;
            }
        });
        let editing_dark = st.editing.dark;
        let mut hovered: Option<Option<String>> = None;
        let button = ui.menu_button("Color Presets\u{2026}", |ui| {
            ui.set_min_width(220.0);
            let entries = st.entries.get_or_insert_with(|| menu(g, profile, editing_dark)).clone();
            for e in &entries {
                match e {
                    MenuEntry::Separator => {
                        ui.separator();
                    }
                    MenuEntry::Preset { name, title, mark } => {
                        let prefix = match mark {
                            Mark::On => "\u{2713} ",
                            Mark::Mixed => "\u{2212} ",
                            Mark::Off => "    ",
                        };
                        let r = ui.button(format!("{prefix}{title}"));
                        if r.hovered() {
                            hovered = Some(Some(name.clone()));
                        }
                        if r.clicked() {
                            out.clicked = Some(name.clone());
                            ui.close();
                        }
                    }
                    cmd => {
                        let r = ui.button(cmd.command_title().unwrap_or(""));
                        if r.hovered() {
                            hovered = Some(None);
                        }
                        if r.clicked() {
                            match cmd {
                                MenuEntry::Import => out.import = true,
                                MenuEntry::Export => out.export = true,
                                MenuEntry::SaveAs => out.save_as = true,
                                MenuEntry::Delete => out.delete = true,
                                _ => out.gallery = true,
                            }
                            ui.close();
                        }
                    }
                }
            }
        });
        let open = button.inner.is_some();
        let now = Instant::now();
        match hovered {
            Some(Some(name)) => {
                if st.hover.as_ref().is_none_or(|(n, _)| *n != name) {
                    st.hover = Some((name, now));
                }
            }
            // A command row: no preview, the colors go back (`willHighlightItem:`).
            Some(None) => {
                st.hover = None;
                if let Some(pre) = st.preview.take() {
                    pre.restore(profile);
                    out.previewed = true;
                }
            }
            None => {}
        }
        if open {
            if let Some((name, since)) = st.hover.clone() {
                let waited = now.duration_since(since);
                if waited < PREVIEW_DELAY {
                    ui.ctx().request_repaint_after(PREVIEW_DELAY - waited);
                } else if st.preview.as_ref().is_none_or(|p| p.name != name) {
                    if let Some(p) = all_presets(g).get(name.as_str()).copied() {
                        let pre = st.preview.take().unwrap_or_else(|| Preview::begin(profile, guid, editing_dark, &name));
                        // The preview loads without asking, as `loadColorPresetWithName:prompt:NO`.
                        pre.restore(profile);
                        load(p, profile, false, editing_dark, system_dark);
                        st.preview = Some(Preview { name, ..pre });
                        out.previewed = true;
                    }
                }
            }
        }
        // A click ends the preview first: the load starts from the saved colors.
        if out.clicked.is_some() || !open {
            st.hover = None;
            st.entries = None;
            if let Some(pre) = st.preview.take() {
                pre.restore(profile);
                out.previewed = true;
            }
        }
    });
    out
}

/// Import's file dialog: several `.itermcolors`, at most `MAX_IMPORT_FILES`. Each is read bounded
/// and sanitized; a failure shows "Import Failed.", a duplicate asks, the rest are added.
pub fn begin_import(st: &mut State) {
    let Some(paths) = rfd::FileDialog::new().add_filter("Color presets", &["itermcolors"]).pick_files() else { return };
    for path in paths.into_iter().take(MAX_IMPORT_FILES) {
        match cp::read_file(&path) {
            Some(p) => st.queue.push_back((cp::name_from_file(&path), p)),
            None => {
                st.dialog = Some(Dialog::Alert { heading: IMPORT_FAILED_HEADING.into(), text: IMPORT_FAILED_TEXT.into() });
                break;
            }
        }
    }
}

/// Export: saves the current colors to a picked file and reveals it in Explorer.
pub fn begin_export(st: &mut State, profile: &Profile) {
    let Some(path) = rfd::FileDialog::new().add_filter("Color presets", &["itermcolors"]).set_file_name("Colors.itermcolors").save_file() else { return };
    let bytes = cp::write_itermcolors(&cp::current(profile, st.editing.dark));
    match std::fs::write(&path, bytes) {
        Ok(()) => st.reveal = Some(path.to_string_lossy().into_owned()),
        Err(_) => st.dialog = Some(Dialog::Alert { heading: SAVE_FAILED_HEADING.into(), text: could_not_save_text(&path.to_string_lossy()) }),
    }
}

/// Save As: asks for a name, prefilled with the most similar custom preset's.
pub fn begin_save_as(st: &mut State, g: &Globals, profile: &Profile) {
    let mut names: Vec<String> = cp::custom(g).iter().map(|(n, _)| n.clone()).collect();
    names.sort();
    let name = most_similar(g, &cp::current(profile, st.editing.dark)).unwrap_or_default();
    st.dialog = Some(Dialog::SaveAs { name, names });
}

/// Delete Preset: a chooser, or the "No deletable color presets." alert.
pub fn begin_delete(st: &mut State, g: &Globals) {
    let mut names: Vec<String> = cp::custom(g).iter().map(|(n, _)| n.clone()).collect();
    names.sort();
    st.dialog = Some(if names.is_empty() {
        Dialog::Alert { heading: NO_DELETABLE_HEADING.into(), text: NO_DELETABLE_TEXT.into() }
    } else {
        Dialog::Delete { names, chosen: 0 }
    });
}

/// Shows the dialog in front (or starts the next queued import) and carries out its answer.
/// True when the stored presets changed (the settings are saved).
pub fn pump_dialogs(ctx: &egui::Context, st: &mut State, g: &mut Globals, profile: &Profile) -> bool {
    let mut changed = false;
    // The next import waits for the one before it to be answered.
    while st.dialog.is_none() {
        let Some((name, preset)) = st.queue.pop_front() else { break };
        match cp::duplicate_of(g, &preset) {
            Some(dup) => st.dialog = Some(Dialog::Duplicate { name: dup, original: name, preset }),
            None => match cp::add_custom(g, &name, preset) {
                Ok(_) => changed = true,
                Err(e) => {
                    st.queue.clear();
                    st.dialog = Some(Dialog::Alert { heading: IMPORT_FAILED_HEADING.into(), text: e });
                }
            },
        }
    }
    let Some(dialog) = st.dialog.clone() else { return changed };
    let mut done = false;
    let mut next: Option<Dialog> = None;
    egui::Modal::new(egui::Id::new("color-preset-dialog")).show(ctx, |ui| {
        ui.set_max_width(420.0);
        match dialog {
            Dialog::Alert { heading, text } => {
                ui.strong(heading);
                ui.add(egui::Label::new(text).wrap());
                if ui.button("OK").clicked() {
                    done = true;
                }
            }
            Dialog::Duplicate { name, original, preset } => {
                ui.strong(DUPLICATE_HEADING);
                ui.add(egui::Label::new(duplicate_text(&name)).wrap());
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        done = true;
                    }
                    if ui.button(ADD_ANYWAY).clicked() {
                        done = true;
                        match cp::add_custom(g, &original, preset.clone()) {
                            Ok(_) => changed = true,
                            Err(e) => {
                                st.queue.clear();
                                next = Some(Dialog::Alert { heading: IMPORT_FAILED_HEADING.into(), text: e });
                            }
                        }
                    }
                });
            }
            Dialog::SaveAs { mut name, names } => {
                ui.strong(SAVE_TITLE);
                ui.label(SAVE_PROMPT);
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut name).char_limit(cp::MAX_NAME).desired_width(240.0));
                    egui::ComboBox::from_id_salt("save-preset-names").selected_text("").show_ui(ui, |ui| {
                        for n in &names {
                            if ui.selectable_label(*n == name, n).clicked() {
                                name = n.clone();
                            }
                        }
                    });
                });
                let mut keep = true;
                ui.horizontal(|ui| {
                    if ui.add_enabled(!name.trim().is_empty(), egui::Button::new("OK")).clicked() {
                        keep = false;
                        let saved = save_preset(g, profile, &name, st.editing.dark);
                        match saved {
                            Ok(()) => changed = true,
                            Err(e) => next = Some(Dialog::Alert { heading: SAVE_FAILED_HEADING.into(), text: e }),
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        keep = false;
                    }
                });
                if keep {
                    next = Some(Dialog::SaveAs { name, names });
                }
                done = !keep;
            }
            Dialog::Delete { names, mut chosen } => {
                ui.strong(DELETE_PROMPT);
                egui::ComboBox::from_id_salt("delete-preset-names").selected_text(names.get(chosen).cloned().unwrap_or_default()).show_ui(ui, |ui| {
                    for (i, n) in names.iter().enumerate() {
                        ui.selectable_value(&mut chosen, i, n);
                    }
                });
                let mut keep = true;
                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() {
                        keep = false;
                        if let Some(n) = names.get(chosen) {
                            cp::delete_custom(g, n);
                            changed = true;
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        keep = false;
                    }
                });
                if keep {
                    next = Some(Dialog::Delete { names, chosen });
                }
                done = !keep;
            }
        }
    });
    if done || next.is_some() {
        st.dialog = next;
    }
    changed
}

/// Save As: a same-named custom preset is deleted first, then the current colors are added under
/// the name (`reallySaveColorPresetWithName:`).
pub fn save_preset(g: &mut Globals, profile: &Profile, name: &str, editing_dark: bool) -> Result<(), String> {
    let current = cp::current(profile, editing_dark);
    let clean = cp::clean_name(name);
    if cp::custom(g).iter().any(|(n, _)| *n == clean) {
        cp::delete_custom(g, &clean);
    }
    cp::add_custom(g, name, current).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::Profile;

    fn globals() -> Globals {
        Globals::empty(std::path::Path::new("settings-presets-test.toml"))
    }

    fn named(name: &str) -> Preset {
        cp::built_in().iter().find(|(n, _)| n == name).unwrap().1.clone()
    }

    fn titles(entries: &[MenuEntry]) -> Vec<String> {
        entries
            .iter()
            .map(|e| match e {
                MenuEntry::Preset { title, .. } => title.clone(),
                MenuEntry::Separator => "-".into(),
                other => other.command_title().unwrap().to_string(),
            })
            .collect()
    }

    #[test]
    fn preset_menu_follows_iterm2() {
        let g = globals();
        let t = titles(&menu(&g, &Profile::new("t"), false));
        let head: Vec<&str> = t.iter().take(5).map(String::as_str).collect();
        assert_eq!(head, ["\u{262f} High Contrast", "\u{262f} Regular", "\u{262f} Solarized", "-", "Dark Background"]);
        assert_eq!(t.len(), 3 + 1 + 8 + 1 + 5);
        assert_eq!(t[t.len() - 7], "Tango Light");
        assert_eq!(t[t.len() - 6..], ["-", "Import\u{2026}", "Export\u{2026}", "Save As\u{2026}", "Delete Preset\u{2026}", "Visit Online Gallery"]);
        let mut sorted: Vec<&String> = t[4..t.len() - 6].iter().collect();
        let before = sorted.clone();
        sorted.sort();
        assert_eq!(sorted, before);

        let mut g = globals();
        cp::add_custom(&mut g, "Mine", named("Tango Dark")).unwrap();
        let t = titles(&menu(&g, &Profile::new("t"), false));
        let n = t.len();
        assert_eq!(t[n - 9..n - 6], ["Tango Light", "-", "Mine"]);
        assert_eq!(t[n - 6], "-");
        assert_eq!(t[n - 5], "Import\u{2026}");
    }

    fn marks(g: &Globals, profile: &Profile) -> Vec<(String, Mark)> {
        menu(g, profile, false)
            .into_iter()
            .filter_map(|e| match e {
                MenuEntry::Preset { name, mark, .. } => Some((name, mark)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn check_marks_follow_iterm2() {
        let g = globals();
        let mut profile = Profile::new("t");
        cp::apply_whole(&named("Tango Dark"), &mut profile);
        let m = marks(&g, &profile);
        assert_eq!(m.iter().filter(|(_, k)| *k == Mark::On).collect::<Vec<_>>(), [&("Tango Dark".to_string(), Mark::On)]);
        assert!(m.iter().all(|(_, k)| *k != Mark::Mixed));

        // Separate colors with only the dark mode equal to Tango Dark: Mixed.
        let mut p = Profile::new("t");
        cp::apply(&named("Tango Dark"), &mut p, Modes::NONE, Modes::DARK_ONLY, true);
        let m = marks(&g, &p);
        assert!(m.iter().any(|(n, k)| n == "Tango Dark" && *k == Mark::Mixed));
        assert!(m.iter().all(|(_, k)| *k != Mark::On));

        // A changed color marks nothing.
        profile.set("Ansi 3 Color", toml::Value::Table(toml::Table::from_iter([("r".into(), 0.9.into()), ("g".into(), 0.1.into()), ("b".into(), 0.2.into()), ("a".into(), 1.0.into())])));
        assert!(marks(&g, &profile).iter().all(|(_, k)| *k == Mark::Off));
    }

    #[test]
    fn mode_questions_follow_iterm2() {
        assert_eq!(question(true, false), Question::None);
        assert_eq!(question(true, true), Question::HasModes);
        assert_eq!(question(false, true), Question::LacksModes);
        assert_eq!(question(false, false), Question::None);
        let spec = question_spec(Question::HasModes, true).unwrap();
        assert_eq!((spec.id.as_deref(), spec.kind), (Some("NoSyncUpdateWhichModes_PresetHasModes"), Kind::Persistent));
        assert_eq!(spec.labels, ["Update Both Modes", "Update Dark Mode Only"]);
        assert_eq!(spec.heading, "Update Which Modes?");
        assert_eq!(question_spec(Question::LacksModes, false).unwrap().labels[1], "Update Light Mode Only");
        assert_eq!(question_spec(Question::None, false), None);

        let solarized = named("Solarized");
        let dark_fg = crate::import::preset_color_to_value(&solarized["Foreground Color (Dark)"]);
        let light_fg = crate::import::preset_color_to_value(&solarized["Foreground Color (Light)"]);
        // Both modes: each mode gets its own colors and separate colors turn on.
        let mut p = Profile::new("t");
        load(&solarized, &mut p, true, false, false);
        assert_eq!(p.values[SEPARATE], Value::Boolean(true));
        assert_eq!(p.values["Foreground Color (Light)"], light_fg);
        assert_eq!(p.values["Foreground Color (Dark)"], dark_fg);
        // Separate on, not both: only the edited mode.
        let mut p = Profile::new("t");
        p.set(SEPARATE, Value::Boolean(true));
        let before = p.values.clone();
        load(&solarized, &mut p, false, true, false);
        assert_eq!(p.values["Foreground Color (Dark)"], dark_fg);
        assert!(p.values.iter().filter(|(k, v)| before.get(*k) != Some(*v)).all(|(k, _)| k.ends_with(cp::DARK)));
        // Separate off: the system's mode into the base keys.
        let mut p = Profile::new("t");
        p.set(SEPARATE, Value::Boolean(false));
        load(&solarized, &mut p, false, false, true);
        assert_eq!(p.values["Foreground Color"], dark_fg);
        assert_eq!(p.values[SEPARATE], Value::Boolean(false));
        assert_ne!(p.values.get("Foreground Color (Light)"), Some(&light_fg));
    }

    #[test]
    fn similar_name_follows_iterm2() {
        let mut g = globals();
        let profile = Profile::new("t");
        let current = cp::current(&profile, false);
        cp::add_custom(&mut g, "A", current.clone()).unwrap();
        let mut far = current.clone();
        for c in far.values_mut() {
            c.r = 1.0 - c.r;
            c.g = 1.0 - c.g;
            c.b = 1.0 - c.b;
        }
        cp::add_custom(&mut g, "B", far.clone()).unwrap();
        assert_eq!(most_similar(&g, &current), Some("A".to_string()));
        let mut only_b = globals();
        cp::add_custom(&mut only_b, "B", far).unwrap();
        assert!(cp::difference(&current, &cp::custom(&only_b)[0].1) > SIMILAR_LIMIT);
        assert_eq!(most_similar(&only_b, &current), None);
        assert_eq!(most_similar(&globals(), &current), None);
    }

    #[test]
    fn import_texts_follow_iterm2() {
        assert_eq!(IMPORT_FAILED_HEADING, "Import Failed.");
        assert_eq!(IMPORT_FAILED_TEXT, "The selected file could not be read or did not contain a valid color scheme.");
        assert_eq!(DUPLICATE_HEADING, "Add duplicate color preset?");
        assert_eq!(ADD_ANYWAY, "Add it anyway");
        assert_eq!(duplicate_text("Mine"), "The color preset \u{201c}Mine\u{201d} is the same as the preset you're trying to add. Really add it?");
        assert_eq!(could_not_save_text("C:\\x.itermcolors"), "Could not save to C:\\x.itermcolors");
        assert_eq!(GALLERY_URL, "https://www.iterm2.com/colorgallery");
        assert_eq!(PREVIEW_DELAY, Duration::from_millis(400));
    }

    #[test]
    fn editing_selector_follows_iterm2() {
        assert!(!editing_enabled(false));
        assert!(editing_enabled(true));
        // Opened in dark mode: starts dark; follows Windows until a pick.
        let mut st = State::new(true);
        assert!(st.editing.dark);
        st.editing.follow(false);
        assert!(!st.editing.dark);
        st.editing.pick(true);
        st.editing.follow(false);
        assert!(st.editing.dark);
    }

    #[test]
    fn closing_settings_restores_a_preview() {
        let mut store = crate::store::Store::fresh(&std::env::temp_dir().join(format!("tandem-presets-{}.toml", std::process::id())));
        let guid = store.default_guid.clone();
        let before = store.get(&guid).unwrap().values.clone();
        let mut st = State::new(false);
        let p = store.get_mut(&guid).unwrap();
        st.preview = Some(Preview::begin(p, &guid, false, "Solarized"));
        load(&named("Tango Dark"), p, false, false, false);
        assert_ne!(store.get(&guid).unwrap().values, before);
        assert!(st.restore(&mut store));
        assert_eq!(store.get(&guid).unwrap().values, before);
        assert!(!st.restore(&mut store));
    }

    #[test]
    fn a_preview_is_put_back_when_its_header_is_gone() {
        let mut store = crate::store::Store::fresh(&std::env::temp_dir().join(format!("tandem-presets-strand-{}.toml", std::process::id())));
        let guid = store.default_guid.clone();
        let before = store.get(&guid).unwrap().values.clone();
        let mut st = State::new(false);
        let p = store.get_mut(&guid).unwrap();
        st.preview = Some(Preview::begin(p, &guid, false, "Solarized"));
        load(&named("Tango Dark"), p, false, false, false);
        // The Colors header drew for the profile this frame: the preview stays.
        st.shown = Some(guid.clone());
        assert!(!st.strand_check(&mut store));
        assert!(st.preview.is_some());
        assert_ne!(store.get(&guid).unwrap().values, before);
        // The next frame the person is on another tab (no header): the colors go back.
        assert!(st.strand_check(&mut store));
        assert!(st.preview.is_none());
        assert_eq!(store.get(&guid).unwrap().values, before);
    }

    #[test]
    fn save_as_replaces_a_same_named_preset() {
        let mut g = globals();
        let mut profile = Profile::new("t");
        save_preset(&mut g, &profile, "Mine", false).unwrap();
        cp::apply_whole(&named("Tango Dark"), &mut profile);
        save_preset(&mut g, &profile, "Mine", false).unwrap();
        let custom = cp::custom(&g);
        assert_eq!(custom.len(), 1);
        assert_eq!(custom[0].0, "Mine");
        assert_eq!(custom[0].1["Ansi 1 Color"], cp::current(&profile, false)["Ansi 1 Color"]);
    }
}

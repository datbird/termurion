//! The key and pointer binding editors of the settings window: iTerm2's Keys and
//! Pointer panes and the profile Key Mappings table.

use toml::Value;

use crate::commands::Status;
use crate::globals::{Globals, keymaps};
use crate::keyaction::{self, Arg, Binding, Escaping};
use crate::keystroke::{self, Keystroke};
use crate::mouse::Mods;
use crate::pointer::{self, PointerBinding};
use crate::profile::Profile;
use crate::schema::{self, Kind, Setting};

thread_local! {
    /// The snippets Send Snippet can pick and names; the settings window sets them each
    /// frame from the snippets file.
    static SNIPPETS: std::cell::RefCell<Vec<crate::snippets::Snippet>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The snippets the editors offer (the settings window calls this each frame).
pub fn set_snippets(list: &[crate::snippets::Snippet]) {
    SNIPPETS.with(|s| *s.borrow_mut() = list.to_vec());
}

thread_local! {
    /// "Show indicators for non-default values"; the settings window sets it each frame.
    static INDICATE_NON_DEFAULT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether controls mark a non-default value (the settings window calls this each frame from
/// `NoSyncHideDefaultValuedSettings`, `PP:1111-1121`).
pub fn set_indicate_non_default(on: bool) {
    INDICATE_NON_DEFAULT.with(|c| c.set(on));
}

pub fn indicate_non_default() -> bool {
    INDICATE_NON_DEFAULT.with(std::cell::Cell::get)
}

/// The note under General > Experimental's right-to-left checkbox (`XIB:8743`).
pub const EXPERIMENTAL_NOTE: &str = "This setting also enables ligature support in the GPU renderer.";

/// The banner shown when "Prefer integrated to discrete GPU" changes (`GP:2089`).
pub const GPU_RESTART: &str = "You must restart Tandem for this change to take effect.";

/// The right-to-left checkbox's tooltip: iTerm2's xib carries this unrelated text on it
/// (`XIB:8720`), kept as iTerm2 shows it.
pub const RTL_TOOLTIP: &str = "Buried sessions appear under Session > Buried Sessions. When this setting is enabled, window sizes are restored when attaching to an existing tmux session.";

/// The settings panes' tolerance for numbers (`iTermPreferencesBaseViewController.m:287-292`:
/// "otherwise colors don't compare equal when they ought to").
const INDICATOR_EPSILON: f64 = 0.001;

/// `it_hasZeroValue`: 0 (or false), an empty text, list or table.
fn has_zero_value(v: &Value) -> bool {
    match v {
        Value::Integer(i) => *i == 0,
        Value::Float(f) => *f == 0.0,
        Value::Boolean(b) => !b,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Table(t) => t.is_empty(),
        Value::Datetime(_) => false,
    }
}

/// `isApproximatelyEqualToObject:epsilon:` (`NSObject+iTerm.m:25-105`): numbers within the
/// epsilon, tables with the same keys and lists of the same length element by element.
fn approximately_equal(a: &Value, b: &Value) -> bool {
    let number = |v: &Value| v.as_float().or_else(|| v.as_integer().map(|i| i as f64));
    match (a, b) {
        (Value::Table(x), Value::Table(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| approximately_equal(v, w))),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(v, w)| approximately_equal(v, w)),
        _ => match (number(a), number(b)) {
            (Some(x), Some(y)) => (x - y).abs() < INDICATOR_EPSILON,
            _ => a == b,
        },
    }
}

/// `isNullablyEqualToObject:epsilon:` (`NSObject+iTerm.m:168-179`): approximately equal, or
/// one missing and the other zero or empty.
pub fn nullably_equal(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => approximately_equal(x, y),
        (None, Some(v)) | (Some(v), None) => has_zero_value(v),
    }
}

/// Whether a global control shows its default (`PBVC:287-292`): the value (the stored one, else
/// the schema default) nullably equals the default.
pub fn global_is_default(g: &Globals, s: &Setting) -> bool {
    nullably_equal(g.get(&s.key).as_ref(), s.default.as_ref())
}

/// The non-default indicator (`NonDefaultIndicator.swift`): a 2-point bar in the accent color
/// the full height of `control`, 4 points left of it; hidden with a disabled control
/// (`NDI:67-78`).
pub fn non_default_bar(ui: &egui::Ui, control: egui::Rect, enabled: bool) {
    if !enabled {
        return;
    }
    let bar = egui::Rect::from_x_y_ranges(control.left() - 4.0..=control.left() - 2.0, control.y_range());
    // The bar may sit left of the scroll area's content edge.
    let painter = ui.painter().clone().with_clip_rect(ui.clip_rect().expand2(egui::vec2(6.0, 0.0)));
    painter.rect_filled(bar, 0.0, ui.visuals().selection.bg_fill);
}

/// Send Snippet's name for a stored key.
fn snippet_name(key: &str) -> String {
    SNIPPETS.with(|s| crate::user_actions::snippet_action_name(s.borrow().iter().find(|x| x.matches_key(key)).map(|x| x.title.as_str())))
}

/// The Delete sends ^H checkbox's binding: Backspace (0x7f, kVK_Delete) to Send ^H Backspace.
const BACKSPACE_KEY: &str = "0x7f-0x0-0x33";

fn later_note(status: Status) -> String {
    match status {
        Status::Later(m) => format!(" (milestone {m})"),
        _ => String::new(),
    }
}

/// One line for an action in a table: its name, and its parameter when it has one, after
/// the apply mode's words.
pub fn action_summary(b: &Binding) -> String {
    let base = if b.action == 63 { snippet_name(&b.text) } else { action_name(b) };
    crate::user_actions::with_apply_mode(&base, b.apply_mode)
}

fn action_name(b: &Binding) -> String {
    let name = keyaction::info(b.action).map_or_else(|| format!("Action {}", b.action), |a| a.name.clone());
    let arg = keyaction::info(b.action).map_or(Arg::None, |a| a.arg);
    match arg {
        Arg::None => name,
        // Compose...: `Compose \u{201c}<text>\u{201d}`, the whole text (`iTermKeyBindingAction.m:291-292`).
        _ if b.action == 64 => format!("Compose \u{201c}{}\u{201d}", b.text),
        Arg::Sequence => format!("{name} ({} steps)", keyaction::sequence(&b.text).len()),
        _ if b.text.is_empty() => name,
        // Load Color Preset: `Load Color Preset \u{201c}<name>\u{201d}` (`iTermKeyBindingAction.m:341-342`).
        Arg::ColorPreset => format!("Load Color Preset \u{201c}{}\u{201d}", b.text),
        // Paste... and Paste from Selection...: the configuration's summary (iTerm2's).
        _ if matches!(b.action, 41 | 42) => match crate::paste::Config::decode(&b.text).summary() {
            s if s.is_empty() => name,
            s => format!("{name}: {s}"),
        },
        _ => {
            let text: String = b.text.chars().take(40).collect();
            format!("{name}: {text}")
        }
    }
}

/// The rows of a key binding table: (stored key, keystroke label, action), by label.
pub fn rows(map: &toml::Table) -> Vec<(String, String, String)> {
    let mut out: Vec<(String, String, String)> = map
        .iter()
        .map(|(k, v)| {
            let label = keystroke::parse(k).map_or_else(|| k.clone(), |s| s.label());
            let action = Binding::from_value(v).map_or_else(|| "(unreadable)".to_string(), |b| action_summary(&b));
            (k.clone(), label, action)
        })
        .collect();
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

/// The rows of the pointer bindings table: (stored key, click label, action), by label.
pub fn pointer_rows(map: &toml::Table) -> Vec<(String, String, String)> {
    let mut out: Vec<(String, String, String)> = map
        .iter()
        .map(|(k, v)| {
            let action = PointerBinding::from_value(v).map_or_else(|| "(unreadable)".to_string(), |b| {
                let name = pointer::name(&b.action).to_string();
                if b.argument.is_empty() { name } else { format!("{name}: {}", b.argument.chars().take(40).collect::<String>()) }
            });
            (k.clone(), pointer::key_label(k), action)
        })
        .collect();
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

/// Loads a preset: it replaces the map, or its keys are added over the map's.
pub fn apply_preset(map: &toml::Table, preset: &toml::Table, replace: bool) -> toml::Table {
    let mut out = if replace { toml::Table::new() } else { map.clone() };
    for (k, v) in preset {
        out.insert(k.clone(), v.clone());
    }
    out
}

fn backspace_keys(map: &toml::Table) -> Vec<String> {
    let bs = Keystroke::new(0x7f, 0, Some(0x33), 0x7f);
    map.keys().filter(|k| keystroke::parse(k).is_some_and(|s| s.same_key(&bs))).cloned().collect()
}

pub fn delete_sends_ctrl_h(map: &toml::Table) -> bool {
    backspace_keys(map).iter().any(|k| Binding::from_value(&map[k]).is_some_and(|b| b.action == 16))
}

pub fn set_delete_sends_ctrl_h(map: &mut toml::Table, on: bool) {
    for k in backspace_keys(map) {
        map.remove(&k);
    }
    if on {
        map.insert(BACKSPACE_KEY.into(), Binding::new(16, "").to_value());
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Global,
    Profile,
}

pub struct KeyEditor {
    pub target: Target,
    /// The row being edited; None for a new binding.
    pub old_key: Option<String>,
    pub stroke: Option<Keystroke>,
    pub binding: Binding,
    pub recording: bool,
}

pub struct PointerEditor {
    pub old_key: Option<String>,
    pub button: u8,
    pub clicks: u8,
    pub mods: Mods,
    pub binding: PointerBinding,
}

/// Which hotkey recorder is recording.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeySlot {
    Primary,
    Alternate(usize),
    App,
    /// Settings > Keys > Hotkey's Open Quickly Hotkey.
    OpenQuickly,
}

/// The binding editors' state, kept by the settings window.
#[derive(Default)]
pub struct KeysState {
    /// A hotkey recorder is recording, and what it recorded.
    pub hotkey_recording: Option<HotkeySlot>,
    pub hotkey_recorded: Option<(HotkeySlot, Keystroke)>,
    pub selected: Option<String>,
    pub profile_selected: Option<String>,
    pub pointer_selected: Option<String>,
    pub key_editor: Option<KeyEditor>,
    pub pointer_editor: Option<PointerEditor>,
    pub leader_recording: bool,
    pub leader_recorded: Option<Keystroke>,
    /// The Leader, so the recorder can take "Leader, then a key" as one keystroke.
    pub leader: Option<Keystroke>,
    /// The recorder has seen the Leader and waits for the key after it.
    leader_seen: bool,
    /// A warning an editor raised, for the settings window to show (`SettingsWindow::sheet`).
    pub ask: Option<SheetWarning>,
    /// A binding whose save waits for the override warning's OK.
    held: Option<HeldKey>,
}

/// A saved binding held for a warning: the editor and the profile Guid it belongs to (None for
/// the global map).
struct HeldKey {
    editor: KeyEditor,
    profile: Option<String>,
}

impl KeysState {
    /// A key press should be recorded, not typed.
    pub fn recording(&self) -> bool {
        self.leader_recording || self.hotkey_recording.is_some() || self.key_editor.as_ref().is_some_and(|e| e.recording)
    }

    pub fn deliver(&mut self, k: Keystroke) {
        if let Some(slot) = self.hotkey_recording.take() {
            // Modifiers alone do not make a hotkey; keep recording.
            if k.keycode.is_some_and(|c| matches!(c, 0x37 | 0x38 | 0x3A | 0x3B | 0x3C | 0x3D | 0x3E | 0x36)) {
                self.hotkey_recording = Some(slot);
                return;
            }
            self.hotkey_recorded = Some((slot, k));
            return;
        }
        if self.leader_recording {
            self.leader_recording = false;
            self.leader_recorded = Some(k);
        } else if let Some(e) = self.key_editor.as_mut().filter(|e| e.recording) {
            if !self.leader_seen && self.leader.as_ref().is_some_and(|l| l.same_key(&k)) {
                self.leader_seen = true;
                return;
            }
            let mut k = k;
            if std::mem::take(&mut self.leader_seen) {
                k.flags |= keystroke::LEADER;
            }
            e.recording = false;
            e.stroke = Some(k);
        }
    }
}

pub enum TableAction {
    None,
    Add,
    Edit(String),
    Remove(String),
}

/// A bindings table with +, -, and Edit. Double-click edits a row.
pub fn table(ui: &mut egui::Ui, salt: &str, rows: &[(String, String, String)], selected: &mut Option<String>) -> TableAction {
    let mut out = TableAction::None;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        egui::ScrollArea::vertical().id_salt(salt).max_height(240.0).auto_shrink([false, true]).show(ui, |ui| {
            egui::Grid::new(salt).num_columns(2).striped(true).min_col_width(180.0).show(ui, |ui| {
                ui.strong("Keyboard Shortcut / Click");
                ui.strong("Action");
                ui.end_row();
                for (key, label, action) in rows {
                    let on = selected.as_deref() == Some(key.as_str());
                    let r = ui.selectable_label(on, label);
                    if r.clicked() {
                        *selected = Some(key.clone());
                    }
                    if r.double_clicked() {
                        out = TableAction::Edit(key.clone());
                    }
                    ui.label(action);
                    ui.end_row();
                }
            });
        });
    });
    ui.horizontal(|ui| {
        if ui.button("+").on_hover_text("Add a binding").clicked() {
            out = TableAction::Add;
        }
        if ui.add_enabled(selected.is_some(), egui::Button::new("-")).on_hover_text("Remove the selected binding").clicked() {
            out = TableAction::Remove(selected.clone().unwrap_or_default());
        }
        if ui.add_enabled(selected.is_some(), egui::Button::new("Edit...")).clicked() {
            out = TableAction::Edit(selected.clone().unwrap_or_default());
        }
    });
    out
}

pub(crate) fn action_combo(ui: &mut egui::Ui, salt: &str, action: &mut i64, allow_sequence: bool) -> bool {
    let mut changed = false;
    let current = keyaction::info(*action).map_or("Select Action...".to_string(), |a| format!("{}{}", a.name, later_note(keyaction::status(*action))));
    egui::ComboBox::from_id_salt(salt).width(320.0).height(420.0).selected_text(current).show_ui(ui, |ui| {
        for (group, codes) in keyaction::groups() {
            ui.label(egui::RichText::new(group).strong());
            for code in codes {
                if code == 66 && !allow_sequence {
                    continue;
                }
                let Some(a) = keyaction::info(code) else { continue };
                let text = format!("    {}{}", a.name, later_note(keyaction::status(code)));
                if ui.selectable_label(*action == code, text).clicked() {
                    *action = code;
                    changed = true;
                }
            }
        }
    });
    changed
}

fn profile_combo(ui: &mut egui::Ui, salt: &str, guid: &mut String, profiles: &[(String, String)]) -> bool {
    let mut changed = false;
    let shown = profiles.iter().find(|(g, _)| g == guid).map_or("Select a profile", |(_, n)| n.as_str()).to_string();
    egui::ComboBox::from_id_salt(salt).selected_text(shown).show_ui(ui, |ui| {
        for (g, n) in profiles {
            if ui.selectable_label(g == guid, n).clicked() {
                *guid = g.clone();
                changed = true;
            }
        }
    });
    changed
}

/// The color presets a Load Color Preset binding can name (`preset_names()`).
fn preset_combo(ui: &mut egui::Ui, salt: &str, name: &mut String) -> bool {
    let mut changed = false;
    let shown = if name.is_empty() { "Select a preset".to_string() } else { name.clone() };
    egui::ComboBox::from_id_salt(salt).width(240.0).height(420.0).selected_text(shown).show_ui(ui, |ui| {
        for n in crate::color_presets::remembered_names() {
            if ui.selectable_label(*name == n, &n).clicked() {
                *name = n;
                changed = true;
            }
        }
    });
    changed
}

/// Menu items a binding can select, with their title and id.
fn menu_items() -> Vec<(&'static str, String)> {
    crate::menus::items().iter().filter(|i| !i.separator && !i.submenu && !i.selector.is_empty()).map(|i| (i.label(), i.id())).collect()
}

/// `pointer`: pointer bindings store "identifier\ntitle"; key bindings "title\nidentifier".
fn menu_combo(ui: &mut egui::Ui, salt: &str, text: &mut String, pointer: bool) -> bool {
    let mut changed = false;
    let shown = text.split('\n').find(|p| !p.contains(" > ")).unwrap_or(text.as_str()).to_string();
    egui::ComboBox::from_id_salt(salt).width(320.0).height(420.0).selected_text(shown).show_ui(ui, |ui| {
        for (title, id) in menu_items() {
            let value = if pointer { format!("{id}\n{title}") } else { format!("{title}\n{id}") };
            if ui.selectable_label(*text == value, &id).clicked() {
                *text = value;
                changed = true;
            }
        }
    });
    changed
}

/// Boolean settings Toggle Setting can flip: every global and profile checkbox except the AI
/// consent switches (`ai_settings::toggle_allowed`).
fn toggle_choices() -> Vec<(String, bool, String)> {
    let bools = |list: &'static [Setting], is_profile: bool| {
        list.iter()
            .filter(|s| s.kind == Kind::Bool && !s.macos_only && crate::ai_settings::toggle_allowed(&s.key))
            .map(move |s| (s.key.clone(), is_profile, s.label.clone()))
    };
    bools(schema::global(), false).chain(bools(schema::profile(), true)).collect()
}

fn setting_combo(ui: &mut egui::Ui, salt: &str, text: &mut String) -> bool {
    let mut changed = false;
    let now: Option<serde_json::Value> = serde_json::from_str(text).ok();
    let key = now.as_ref().and_then(|v| v.get("key")).and_then(|k| k.as_str()).unwrap_or("").to_string();
    let shown = if key.is_empty() { "Select a setting".to_string() } else { key.clone() };
    egui::ComboBox::from_id_salt(salt).width(320.0).height(420.0).selected_text(shown).show_ui(ui, |ui| {
        for (k, is_profile, label) in toggle_choices() {
            let where_ = if is_profile { "Profile" } else { "Global" };
            if ui.selectable_label(k == key, format!("{where_}: {label}")).clicked() {
                *text = serde_json::json!({ "key": k, "isProfile": is_profile, "label": label }).to_string();
                changed = true;
            }
        }
    });
    changed
}

const ESCAPINGS: [(Escaping, &str); 5] = [
    (Escaping::None, "No escaping"),
    (Escaping::Compatibility, "Compatibility (\\n \\e \\a \\t)"),
    (Escaping::Common, "Common (\\n \\r \\t \\e \\a \\b \\\\ \\xHH)"),
    (Escaping::Vim, "vim special characters"),
    (Escaping::VimAndCompatibility, "vim, then Compatibility"),
];

/// The parameter controls for a key binding's action.
/// Send Snippet's picker: the snippets sorted by title, case-insensitively.
fn snippet_combo(ui: &mut egui::Ui, salt: &str, key: &mut String) -> bool {
    let mut list = SNIPPETS.with(|s| s.borrow().clone());
    list.sort_by_key(|s| s.display_title().to_lowercase());
    let shown = list.iter().find(|s| s.matches_key(key)).map_or("Select a snippet".to_string(), |s| s.display_title());
    let mut changed = false;
    egui::ComboBox::from_id_salt(salt).width(320.0).selected_text(shown).show_ui(ui, |ui| {
        for s in &list {
            if ui.selectable_label(s.matches_key(key), s.display_title()).clicked() {
                *key = s.action_key();
                changed = true;
            }
        }
    });
    changed
}

/// The apply mode popup, for the actions that have one.
pub(crate) fn apply_mode_combo(ui: &mut egui::Ui, salt: &str, b: &mut Binding) -> bool {
    if !crate::user_actions::shows_apply_mode(b.action) {
        return false;
    }
    let mut changed = false;
    let shown = crate::user_actions::APPLY_MODES.iter().find(|(m, _)| *m == b.apply_mode).map_or("", |(_, n)| n);
    egui::ComboBox::from_id_salt(salt).width(260.0).selected_text(shown).show_ui(ui, |ui| {
        for (m, name) in crate::user_actions::APPLY_MODES {
            if ui.selectable_label(m == b.apply_mode, name).clicked() {
                b.apply_mode = m;
                changed = true;
            }
        }
    });
    changed
}

/// The parameter field's placeholder for an action (`iTermEditKeyActionWindowController.m
/// :98-101`), when it has one.
pub fn param_placeholder(action: i64) -> Option<&'static str> {
    match action {
        64 => Some("Text for composer"),
        _ => None,
    }
}

pub(crate) fn param_editor(ui: &mut egui::Ui, salt: &str, b: &mut Binding, profiles: &[(String, String)]) -> bool {
    let arg = keyaction::info(b.action).map_or(Arg::None, |a| a.arg);
    let mut changed = false;
    if b.action == 63 {
        return snippet_combo(ui, &format!("{salt}-snippet"), &mut b.text);
    }
    match arg {
        Arg::None => {}
        Arg::Text => {
            changed |= ui.text_edit_singleline(&mut b.text).changed();
            let now = b.escaping();
            let shown = ESCAPINGS.iter().find(|(e, _)| *e == now).map_or("", |(_, n)| n);
            egui::ComboBox::from_id_salt(format!("{salt}-esc")).selected_text(shown).show_ui(ui, |ui| {
                for (e, name) in ESCAPINGS {
                    if ui.selectable_label(e == now, name).clicked() {
                        b.version = 2;
                        b.escaping = Some(e.code());
                        changed = true;
                    }
                }
            });
        }
        Arg::ColorPreset => changed |= preset_combo(ui, &format!("{salt}-preset"), &mut b.text),
        Arg::VimText | Arg::Other => {
            let edit = egui::TextEdit::singleline(&mut b.text).hint_text(param_placeholder(b.action).unwrap_or(""));
            changed |= ui.add(edit).changed();
        }
        Arg::Hex => {
            changed |= ui.text_edit_singleline(&mut b.text).changed();
            ui.label("Hex codes, for example 0x1b 0x5b");
        }
        Arg::Escape => {
            ui.label("Esc +");
            changed |= ui.text_edit_singleline(&mut b.text).changed();
        }
        Arg::Profile => changed |= profile_combo(ui, &format!("{salt}-profile"), &mut b.text, profiles),
        Arg::MenuItem => changed |= menu_combo(ui, &format!("{salt}-menu"), &mut b.text, false),
        Arg::Setting => changed |= setting_combo(ui, &format!("{salt}-setting"), &mut b.text),
        Arg::Sequence => changed |= sequence_editor(ui, salt, b, profiles),
    }
    changed
}

fn sequence_editor(ui: &mut egui::Ui, salt: &str, b: &mut Binding, profiles: &[(String, String)]) -> bool {
    let mut steps = keyaction::sequence(&b.text);
    let mut changed = false;
    let mut remove = None;
    let mut up = None;
    for (i, step) in steps.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.label(format!("{}.", i + 1));
            changed |= action_combo(ui, &format!("{salt}-seq{i}"), &mut step.action, false);
            changed |= param_editor(ui, &format!("{salt}-seq{i}-p"), step, profiles);
            if ui.small_button("Up").clicked() && i > 0 {
                up = Some(i);
            }
            if ui.small_button("Remove").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = up {
        steps.swap(i, i - 1);
        changed = true;
    }
    if let Some(i) = remove {
        steps.remove(i);
        changed = true;
    }
    if ui.button("Add Action").clicked() {
        steps.push(Binding::new(-1, ""));
        changed = true;
    }
    if changed {
        b.text = keyaction::sequence_text(&steps);
    }
    changed
}

pub enum Outcome {
    Open,
    Cancel,
    Save,
}

/// The Edit Key Binding window.
pub fn key_editor(ctx: &egui::Context, e: &mut KeyEditor, profiles: &[(String, String)]) -> Outcome {
    let mut out = Outcome::Open;
    egui::Window::new("Edit Key Binding").collapsible(false).resizable(true).default_width(560.0).show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label("Keyboard Shortcut:");
            let text = if e.recording {
                "Press the keys...".to_string()
            } else {
                e.stroke.as_ref().map_or("Click to record".to_string(), Keystroke::label)
            };
            if ui.button(text).clicked() {
                e.recording = !e.recording;
            }
        });
        ui.horizontal(|ui| {
            ui.label("Action:");
            if action_combo(ui, "key-action", &mut e.binding.action, true) {
                e.binding.text.clear();
                if !crate::user_actions::shows_apply_mode(e.binding.action) {
                    e.binding.apply_mode = 0;
                }
            }
        });
        param_editor(ui, "key-param", &mut e.binding, profiles);
        apply_mode_combo(ui, "key-apply", &mut e.binding);
        ui.separator();
        ui.horizontal(|ui| {
            let ok = e.stroke.is_some() && e.binding.action != -1 && !e.recording;
            if ui.add_enabled(ok, egui::Button::new("OK")).clicked() {
                out = Outcome::Save;
            }
            if ui.button("Cancel").clicked() {
                out = Outcome::Cancel;
            }
        });
    });
    out
}

const BUTTONS: [&str; 8] = ["Left button", "Right button", "Middle button", "Button #4", "Button #5", "Button #6", "Button #7", "Button #8"];
const CLICKS: [&str; 4] = ["single click", "double click", "triple click", "quad click"];

/// The Edit Pointer Binding window.
pub fn pointer_editor(ctx: &egui::Context, e: &mut PointerEditor, profiles: &[(String, String)]) -> Outcome {
    let mut out = Outcome::Open;
    egui::Window::new("Edit Pointer Binding").collapsible(false).default_width(520.0).show(ctx, |ui| {
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("ptr-button").selected_text(BUTTONS[e.button.min(7) as usize]).show_ui(ui, |ui| {
                for (i, name) in BUTTONS.iter().enumerate() {
                    ui.selectable_value(&mut e.button, i as u8, *name);
                }
            });
            egui::ComboBox::from_id_salt("ptr-clicks").selected_text(CLICKS[(e.clicks.clamp(1, 4) - 1) as usize]).show_ui(ui, |ui| {
                for (i, name) in CLICKS.iter().enumerate() {
                    ui.selectable_value(&mut e.clicks, i as u8 + 1, *name);
                }
            });
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut e.mods.cmd, "Win");
            ui.checkbox(&mut e.mods.alt, "Alt");
            ui.checkbox(&mut e.mods.shift, "Shift");
            ui.checkbox(&mut e.mods.ctrl, "Ctrl");
        });
        ui.add_enabled(false, egui::Label::new("Trackpad gestures (three-finger tap and swipes, Force Touch) are macOS only."));
        ui.horizontal(|ui| {
            ui.label("Action:");
            let shown = format!("{}{}", pointer::name(&e.binding.action), later_note(pointer::status(&e.binding)));
            egui::ComboBox::from_id_salt("ptr-action").width(300.0).height(420.0).selected_text(shown).show_ui(ui, |ui| {
                for (action, name, _) in pointer::ACTIONS {
                    let note = later_note(pointer::status(&PointerBinding::new(action, "")));
                    if ui.selectable_label(e.binding.action == action, format!("{name}{note}")).clicked() {
                        e.binding.action = action.to_string();
                        e.binding.argument.clear();
                    }
                }
            });
        });
        match pointer::arg(&e.binding.action) {
            Arg::Profile => {
                profile_combo(ui, "ptr-profile", &mut e.binding.argument, profiles);
            }
            Arg::MenuItem => {
                menu_combo(ui, "ptr-menu", &mut e.binding.argument, true);
            }
            Arg::None => {}
            _ => {
                ui.text_edit_singleline(&mut e.binding.argument);
            }
        }
        ui.separator();
        ui.horizontal(|ui| {
            if ui.add_enabled(!e.binding.action.is_empty(), egui::Button::new("OK")).clicked() {
                out = Outcome::Save;
            }
            if ui.button("Cancel").clicked() {
                out = Outcome::Cancel;
            }
        });
    });
    out
}

/// What a bool row's checkbox shows for the stored value: the opposite for an
/// inverted row. Storing the checkbox back applies the same rule.
pub fn checkbox_shows(stored: bool, inverted: bool) -> bool {
    stored != inverted
}

/// A control for one global setting (checkbox, menu, or text).
pub fn global_control(ui: &mut egui::Ui, g: &mut Globals, s: &Setting, enabled: bool) -> bool {
    let mut changed = false;
    ui.add_enabled_ui(enabled && !s.macos_only, |ui| {
        let control = match s.kind {
            Kind::Bool => {
                let mut b = checkbox_shows(g.bool(&s.key), s.inverted);
                let r = ui.checkbox(&mut b, &s.label);
                if r.changed() {
                    g.set(&s.key, Value::Boolean(checkbox_shows(b, s.inverted)));
                    changed = true;
                }
                Some(r.rect)
            }
            Kind::Enum => {
                ui.horizontal(|ui| {
                    ui.label(&s.label);
                    let now = g.get(&s.key);
                    let shown = s.choices.iter().find(|c| Some(&c.value) == now.as_ref()).map_or(String::new(), |c| c.label.clone());
                    egui::ComboBox::from_id_salt(&s.key)
                        .selected_text(shown)
                        .show_ui(ui, |ui| {
                            for c in &s.choices {
                                if ui.selectable_label(Some(&c.value) == now.as_ref(), &c.label).clicked() {
                                    g.set(&s.key, c.value.clone());
                                    changed = true;
                                }
                            }
                        })
                        .response
                        .rect
                })
                .inner
                .into()
            }
            // A ranged float is a slider with two decimals (the Dimming amount, `APV:351-374`).
            Kind::Float if s.min_f64().is_some() && s.max_f64().is_some() => {
                let (lo, hi) = (s.min_f64().unwrap_or(0.0), s.max_f64().unwrap_or(1.0));
                ui.horizontal(|ui| {
                    ui.label(&s.label);
                    let mut v = g.number(&s.key).unwrap_or(lo).clamp(lo, hi);
                    let r = ui.add(egui::Slider::new(&mut v, lo..=hi).fixed_decimals(2));
                    if r.changed() {
                        g.set(&s.key, Value::Float(v));
                        changed = true;
                    }
                    r.rect
                })
                .inner
                .into()
            }
            // The margins are drag values; iTerm2 bounds neither (13l). Other Int rows stay as
            // they were (undrawn or drawn elsewhere): a later milestone gives them a control.
            Kind::Int if matches!(s.key.as_str(), "TerminalMargin" | "TerminalVMargin") => {
                ui.horizontal(|ui| {
                    ui.label(&s.label);
                    let mut v = g.int(&s.key);
                    let drag = egui::DragValue::new(&mut v);
                    let r = ui.add(match (s.min_f64(), s.max_f64()) {
                        (Some(lo), Some(hi)) => drag.range(lo as i64..=hi as i64),
                        _ => drag,
                    });
                    if r.changed() {
                        g.set(&s.key, Value::Integer(v));
                        changed = true;
                    }
                    r.rect
                })
                .inner
                .into()
            }
            _ => None,
        };
        if let Some(rect) = control.filter(|_| indicate_non_default() && !global_is_default(g, s)) {
            non_default_bar(ui, rect, ui.is_enabled());
        }
    })
    .response
    .on_hover_text(if s.macos_only { format!("{} (macOS only)", s.help) } else { s.help.clone() });
    changed
}

fn globals_in(section: &str) -> Vec<&'static Setting> {
    schema::global().iter().filter(|s| s.section == section).collect()
}

/// iTerm2's Keys pane.
/// `key_maps` are every profile's Key Mappings, for the possible override warning.
pub fn keys_page(ui: &mut egui::Ui, g: &mut Globals, st: &mut KeysState, profiles: &[(String, String)], key_maps: &[toml::Table]) -> bool {
    let mut changed = false;
    if let Some(k) = st.leader_recorded.take() {
        g.set("Leader", Value::String(k.serialized()));
        changed = true;
    }
    st.leader = keystroke::parse(&g.string("Leader"));
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.strong("Key Bindings");
        let mut map = g.table("GlobalKeyMap");
        match table(ui, "global-keys", &rows(&map), &mut st.selected) {
            TableAction::Add => st.key_editor = Some(KeyEditor { target: Target::Global, old_key: None, stroke: None, binding: Binding::new(-1, ""), recording: false }),
            TableAction::Edit(k) => st.key_editor = editor_for(&map, &k, Target::Global),
            TableAction::Remove(k) => {
                map.remove(&k);
                st.selected = None;
                g.set("GlobalKeyMap", Value::Table(map.clone()));
                changed = true;
            }
            TableAction::None => {}
        }
        ui.horizontal(|ui| {
            if ui.button("Presets: Factory Defaults").on_hover_text("Replaces the global key bindings with iTerm2's factory set.").clicked() {
                g.set("GlobalKeyMap", Value::Table(keymaps::factory_global().clone()));
                changed = true;
            }
        });
        ui.horizontal(|ui| {
            ui.label("Leader:");
            let leader = keystroke::parse(&g.string("Leader"));
            let text = if st.leader_recording { "Press the keys...".to_string() } else { leader.as_ref().map_or("Click to record".to_string(), Keystroke::label) };
            let button = ui.button(text);
            if button.clicked() {
                st.leader_recording = !st.leader_recording;
            }
            // Marked while a Leader is set (`KeysPreferencesViewController.m:116`).
            if indicate_non_default() && leader.is_some() {
                non_default_bar(ui, button.rect, true);
            }
            if leader.is_some() && ui.button("Clear").clicked() {
                g.set("Leader", Value::String(String::new()));
                changed = true;
            }
        });
        for s in globals_in("Key Bindings").into_iter().filter(|s| s.kind == Kind::Bool) {
            changed |= global_control(ui, g, s, true);
        }
        ui.add_space(8.0);
        ui.strong("Navigation Shortcuts");
        for s in globals_in("Navigation Shortcuts") {
            changed |= global_control(ui, g, s, true);
        }
        ui.add_space(8.0);
        ui.strong("Remap Modifiers");
        for s in globals_in("Remap Modifiers") {
            changed |= global_control(ui, g, s, true);
        }
        if ui.button("Reset to Defaults").clicked() {
            for s in globals_in("Remap Modifiers").into_iter().filter(|s| s.kind == Kind::Enum) {
                if let Some(d) = s.default.clone() {
                    g.set(&s.key, d);
                }
            }
            changed = true;
        }
    });
    if st.key_editor.as_ref().is_some_and(|e| e.target == Target::Global) {
        let mut map = g.table("GlobalKeyMap");
        if finish_key_editor(ui.ctx(), st, &mut map, None, &|stroke| any_profile_maps(&key_maps.iter().collect::<Vec<_>>(), stroke), profiles) {
            g.set("GlobalKeyMap", Value::Table(map));
            changed = true;
        }
    }
    changed
}

fn editor_for(map: &toml::Table, key: &str, target: Target) -> Option<KeyEditor> {
    let binding = Binding::from_value(map.get(key)?)?;
    Some(KeyEditor { target, old_key: Some(key.to_string()), stroke: keystroke::parse(key), binding, recording: false })
}

/// Shows the key editor; on OK writes the binding into `map`. True when `map` changed.
/// `overrides` tells whether the new key would override, or be overridden by, another map's
/// binding: then the save is held for iTerm2's warning (`PK:639-643`, `KP:763-766`), and the
/// settings window saves it with `held_answered`. `profile` is the Guid of the profile `map`
/// belongs to, None for the global map.
fn finish_key_editor(ctx: &egui::Context, st: &mut KeysState, map: &mut toml::Table, profile: Option<String>, overrides: &dyn Fn(&Keystroke) -> bool, profiles: &[(String, String)]) -> bool {
    let Some(e) = st.key_editor.as_mut() else { return false };
    match key_editor(ctx, e, profiles) {
        Outcome::Open => false,
        Outcome::Cancel => {
            st.key_editor = None;
            false
        }
        Outcome::Save => {
            let e = st.key_editor.take().unwrap();
            if e.stroke.as_ref().is_some_and(overrides) {
                let spec = if e.target == Target::Global { possible_override_spec() } else { override_spec() };
                st.ask = Some(SheetWarning::new(spec, SheetThen::SaveKey));
                st.held = Some(HeldKey { editor: e, profile });
                ctx.request_repaint();
                return false;
            }
            save_binding(st, map, e)
        }
    }
}

/// Writes an edited binding into `map`, replacing the row edited and any binding for the same
/// key stored in another form. True when `map` changed.
fn save_binding(st: &mut KeysState, map: &mut toml::Table, e: KeyEditor) -> bool {
    if let Some(old) = &e.old_key {
        map.remove(old);
    }
    let Some(stroke) = e.stroke else { return false };
    // Replace any binding for the same key stored in another form.
    let same: Vec<String> = map.keys().filter(|k| keystroke::parse(k).is_some_and(|s| s.same_key(&stroke))).cloned().collect();
    for k in same {
        map.remove(&k);
    }
    let key = stroke.serialized();
    map.insert(key.clone(), e.binding.to_value());
    if e.target == Target::Global {
        st.selected = Some(key);
    } else {
        st.profile_selected = Some(key);
    }
    true
}

/// What a held binding's save changed.
#[derive(Debug, PartialEq, Eq)]
pub enum Saved {
    Nothing,
    Global,
    /// The profile with this Guid.
    Profile(String),
}

/// The override warning answered with button `index`: OK saves the held binding, Cancel drops
/// it without saving (iTerm2 returns before writing).
pub fn held_answered(st: &mut KeysState, index: usize, g: &mut Globals, store: &mut crate::store::Store) -> Saved {
    let Some(held) = st.held.take() else { return Saved::Nothing };
    if index != 0 {
        return Saved::Nothing;
    }
    match held.profile {
        None => {
            let mut map = g.table("GlobalKeyMap");
            if !save_binding(st, &mut map, held.editor) {
                return Saved::Nothing;
            }
            g.set("GlobalKeyMap", Value::Table(map));
            Saved::Global
        }
        Some(guid) => {
            let Some(p) = store.get_mut(&guid) else { return Saved::Nothing };
            let mut map = p.get("Keyboard Map").and_then(|v| v.as_table().cloned()).unwrap_or_default();
            if !save_binding(st, &mut map, held.editor) {
                return Saved::Nothing;
            }
            p.set("Keyboard Map", Value::Table(map));
            Saved::Profile(guid)
        }
    }
}

/// `haveGlobalKeyMappingForKeystroke:`: the global map `map` has a binding for this key.
pub fn profile_overrides(map: &toml::Table, stroke: &Keystroke) -> bool {
    map.keys().any(|k| keystroke::parse(k).is_some_and(|s| s.same_key(stroke)))
}

/// `anyProfileHasMappingForKeystroke:`: some profile's Key Mappings bind this key.
pub fn any_profile_maps(profiles: &[&toml::Table], stroke: &Keystroke) -> bool {
    profiles.iter().any(|m| profile_overrides(m, stroke))
}

/// A settings warning (`iTermWarning` on the settings window, 13b Task 6): no heading, so
/// iTerm2's "Warning" shows; OK, and Cancel when given, the cancel button.
fn settings_spec(id: &str, text: String, labels: &[&str]) -> crate::warnings::Spec {
    crate::warnings::Spec {
        id: Some(id.to_string()),
        kind: crate::warnings::Kind::Permanent,
        heading: String::new(),
        text,
        labels: labels.iter().map(|l| l.to_string()).collect(),
        cancel: labels.iter().position(|l| *l == crate::api_auth::CANCEL),
        never_remember: Vec::new(),
    }
}

/// Choosing Meta for an Option key (`PK:727-736`).
pub fn meta_spec() -> crate::warnings::Spec {
    let text = "You have chosen to have an option key act as Meta. This option is useful for backward compatibility with older systems. The \"Esc+\" option is recommended for most users.";
    settings_spec("NeverWarnAboutMeta", text.into(), &[crate::api_auth::OK])
}

/// A profile binding for a key the global map binds (`PK:712-725`).
pub fn override_spec() -> crate::warnings::Spec {
    let text = "The keyboard shortcut you have set for this profile will take precedence over an existing shortcut for the same key combination in a global shortcut.";
    settings_spec("NeverWarnAboutOverrides", text.into(), &[crate::api_auth::OK, crate::api_auth::CANCEL])
}

/// A global binding for a key some profile binds (`KP:704-719`).
pub fn possible_override_spec() -> crate::warnings::Spec {
    let text = "The global keyboard shortcut you have set is overridden by at least one profile. Check your profiles\u{2019} keyboard settings if it doesn't work as expected.";
    settings_spec("NeverWarnAboutPossibleOverrides", text.into(), &[crate::api_auth::OK, crate::api_auth::CANCEL])
}

/// Copy to Profile's question (`PP:780-791`); 13t's Copy to Profile asks it.
pub fn replace_profile_spec(name: &str) -> crate::warnings::Spec {
    let text = format!("Replace profile \u{201c}{name}\u{201d} with the current session's settings?");
    settings_spec("NoSyncReplaceProfileWarning", text, &["Replace", crate::api_auth::CANCEL])
}

/// What a settings warning's answer does.
#[derive(Clone, Debug, PartialEq)]
pub enum SheetThen {
    Nothing,
    /// Saves the held binding on OK (`held_answered`).
    SaveKey,
    /// Create a Dedicated Hotkey Window's question, with the hotkey profiles as (Guid, name).
    CreateHotkey(Vec<(String, String)>),
}

/// A warning shown over the settings window, with its checkbox's state.
#[derive(Clone, Debug, PartialEq)]
pub struct SheetWarning {
    pub spec: crate::warnings::Spec,
    pub then: SheetThen,
    suppress: bool,
}

impl SheetWarning {
    pub fn new(spec: crate::warnings::Spec, then: SheetThen) -> SheetWarning {
        SheetWarning { spec, then, suppress: false }
    }
}

/// A settings warning's answer: button `index` (the spec's labels' length is "Permanently
/// Forget Saved Selection", after which the warning shows again), `automatic` when it was
/// answered from a remembered choice without showing.
#[derive(Clone, Debug, PartialEq)]
pub struct SheetAnswer {
    pub spec: crate::warnings::Spec,
    pub index: usize,
    pub automatic: bool,
}

/// The no-drawing half of `sheet_warning`: a remembered answer, when `preempt:` gives one.
pub fn sheet_preempt(g: &Globals, w: &SheetWarning, show_remembered: bool, now: f64) -> Option<SheetAnswer> {
    match crate::warnings::preempt(&w.spec, g, now, show_remembered) {
        crate::warnings::Preempt::Answer(index) => Some(SheetAnswer { spec: w.spec.clone(), index, automatic: true }),
        _ => None,
    }
}

/// Shows a warning over the settings window, as iTerm2's sheet: modal, the heading, the text
/// (with the remembered explanation and "Permanently Forget Saved Selection" when "Always show
/// alerts with remembered selections" shows a remembered one), iTerm2's checkbox and the
/// buttons; Enter is the first button, Escape the Cancel button and Alt plus a button's letter
/// that button. A remembered answer returns at once without drawing. A clicked button's
/// answer is remembered per the checkbox; Forget clears it.
pub fn sheet_warning(ctx: &egui::Context, g: &mut Globals, w: &mut SheetWarning, show_remembered: bool) -> Option<SheetAnswer> {
    use crate::warnings;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    if let Some(answer) = sheet_preempt(g, w, show_remembered, now) {
        return Some(answer);
    }
    let remembered = match warnings::preempt(&w.spec, g, now, show_remembered) {
        warnings::Preempt::ShowRemembered { saved_label } => Some(saved_label),
        _ => None,
    };
    let text = remembered.as_deref().map_or_else(|| w.spec.text.clone(), |saved| warnings::remembered_text(&w.spec.text, saved));
    let mut chosen = None;
    egui::Modal::new(egui::Id::new("settings-warning")).show(ctx, |ui| {
        ui.set_max_width(420.0);
        ui.strong(w.spec.shown_heading());
        ui.add_space(4.0);
        ui.add(egui::Label::new(text.as_str()).wrap());
        if let Some(label) = warnings::checkbox_label(&w.spec) {
            ui.add_space(6.0);
            ui.checkbox(&mut w.suppress, label);
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            for (i, label) in w.spec.labels.iter().enumerate() {
                if ui.button(label.as_str()).clicked() {
                    chosen = Some(i);
                }
            }
            if remembered.is_some() && ui.button(warnings::FORGET).clicked() {
                chosen = Some(w.spec.labels.len());
            }
        });
    });
    let keys = ctx.input_mut(|i| {
        if i.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
            return Some(0);
        }
        if let Some(c) = w.spec.cancel.filter(|_| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            return Some(c);
        }
        warnings::shortcuts(&w.spec.labels).iter().enumerate().find_map(|(n, s)| {
            let key = egui::Key::from_name(&s.as_ref()?.1.to_ascii_uppercase().to_string())?;
            i.consume_key(egui::Modifiers::ALT, key).then_some(n)
        })
    });
    let index = chosen.or(keys)?;
    if index == w.spec.labels.len() {
        if let Some(id) = w.spec.id.as_deref() {
            warnings::clear_saved(g, id);
        }
    } else {
        warnings::remember(&w.spec, g, index, w.suppress, now);
    }
    Some(SheetAnswer { spec: w.spec.clone(), index, automatic: false })
}

/// Whether a global row takes input: the Python API's authentication popup only while the API
/// is on (`GP:2055-2057`), the custom scripts folder only while its checkbox is on
/// (`GP:2152-2154`), the settings folder only while "Load settings from a custom folder or URL"
/// is on (`GP:2210-2212`); every other row always.
pub fn row_enabled(g: &Globals, key: &str) -> bool {
    row_enabled_with(g, key, crate::power::both_gpu_kinds_known())
}

/// `row_enabled` with the one fact the settings window cannot read: "Prefer integrated" is live
/// only when both kinds of GPU exist (`iTermAdvancedGPUSettingsViewController.m:25-34`).
pub fn row_enabled_with(g: &Globals, key: &str, both_gpu_kinds: bool) -> bool {
    match key {
        "preferIntegratedGPU" => both_gpu_kinds,
        "API Authentication Method" => g.bool("EnableAPIServer"),
        "CustomScriptsFolder" => g.bool("UseCustomScriptsFolder"),
        crate::remote_prefs::FOLDER_KEY => g.bool(crate::remote_prefs::LOAD_KEY),
        // The bar can hide in full screen: Show off, or hidden with one tab (`APV:436-441`).
        "FlashTabBarInFullscreen" => !g.bool("ShowFullScreenTabBar") || g.bool("HideTab"),
        // The second title bar row follows the first (`APV:228-231`).
        "ShowPaneTitlesEvenIfOnlyOnePane" => g.bool("ShowPaneTitles"),
        // Enabled only under the system restore policy (`GP:3496-3506`).
        "RestoreWindowsToSameSpaces" => crate::desktops::restore_row_enabled(g.bool("OpenArrangementAtStartup"), g.bool("OpenNoWindowsAtStartup")),
        // Narrows the first (`APV:285-290`).
        crate::presence::REQUIRES_HOTKEYS_KEY => g.bool(crate::presence::UI_ELEMENT_KEY),
        // The amount and Only Text apply while either dimming switch is on (`PT:12402-12422`, `SV:1497-1508`).
        "DimOnlyText" | "SplitPaneDimmingAmount" => g.bool("DimInactiveSplitPanes") || g.bool("DimBackgroundWindows"),
        "PreserveWindowSizeToKeepOnScreenWhenTabBarVisibilityChanges" => g.bool("PreserveWindowSizeWhenTabBarVisibilityChanges"),
        _ => true,
    }
}

/// The key under which a window's egui memory notes that `add_icon_font` found the font.
fn icon_font_id() -> egui::Id {
    egui::Id::new("tandem-settings-icons")
}

pub(crate) fn icon_family() -> egui::FontFamily {
    egui::FontFamily::Name("icons".into())
}

/// Gives the window the icon font (Segoe Fluent Icons on Windows 11, Segoe MDL2 Assets on
/// Windows 10, same code points) for the custom scripts folder's mark. Called before the
/// first frame; without the font the mark is not drawn.
pub fn add_icon_font(ctx: &egui::Context) {
    use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
    let dir = std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into())).join("Fonts");
    let Some(bytes) = ["SegoeIcons.ttf", "segmdl2.ttf"].iter().find_map(|f| std::fs::read(dir.join(f)).ok()) else { return };
    ctx.add_font(FontInsert::new("tandem-icons", egui::FontData::from_owned(bytes), vec![InsertFontFamily { family: icon_family(), priority: FontPriority::Highest }]));
    ctx.data_mut(|d| d.insert_temp(icon_font_id(), true));
}

/// A glyph of the icon font, or None when `add_icon_font` did not find it.
pub fn icon_text(ctx: &egui::Context, glyph: char) -> Option<egui::RichText> {
    has_icon_font(ctx).then(|| egui::RichText::new(glyph.to_string()).family(icon_family()))
}

/// Whether `add_icon_font` found the icon font (without it the "icons" family is not defined
/// and must not be drawn with).
pub fn has_icon_font(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(icon_font_id())).unwrap_or(false)
}

/// Where a window's egui memory notes a custom scripts folder typed and not yet applied.
fn folder_pending_id() -> egui::Id {
    egui::Id::new("tandem-scripts-folder-pending")
}

/// Where a window's egui memory keeps the last folder text checked and whether it was valid.
fn folder_valid_id() -> egui::Id {
    egui::Id::new("tandem-scripts-folder-valid")
}

/// Whether the custom scripts folder's text was typed and not yet applied (iTerm2's
/// `_customScriptsFolderDidChange`), clearing the note. The Settings window applies it when it
/// closes or turns to another page (`GP:3417-3434`).
pub fn take_folder_pending(ctx: &egui::Context) -> bool {
    ctx.data_mut(|d| d.remove_temp::<bool>(folder_pending_id())).unwrap_or(false)
}

/// Notes the custom scripts folder's text as typed and not yet applied.
pub fn note_folder_pending(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(folder_pending_id(), true));
}

/// `chooseCustomScriptsFolder`: the folder picker; a picked folder becomes the text, which is
/// returned.
fn choose_scripts_folder(g: &mut Globals) -> Option<String> {
    let dir = rfd::FileDialog::new().pick_folder()?;
    let text = dir.display().to_string();
    g.set("CustomScriptsFolder", Value::String(text.clone()));
    Some(text)
}

/// The custom scripts folder's row (`GP:2137-2163`, `GP:4204-4219`): the text field and
/// Browse, both enabled with the checkbox, and the mark saying whether the text is a usable
/// folder (checked when the text changes, not on every frame). The text is stored as it is
/// typed; "editing ended" is true when the field lets go of the focus or Browse picked a
/// folder (iTerm2 posts on end of editing, since a partial path would scan a whole drive).
/// Text typed without an end is noted for `take_folder_pending`. Returns (changed, editing
/// ended).
pub fn custom_scripts_folder_row(ui: &mut egui::Ui, g: &mut Globals, home: &std::path::Path) -> (bool, bool) {
    let (mut changed, mut ended) = (false, false);
    let on = g.bool("UseCustomScriptsFolder");
    let mut text = g.string("CustomScriptsFolder");
    let setting = schema::find_global("CustomScriptsFolder");
    ui.horizontal(|ui| {
        ui.add_enabled_ui(on, |ui| {
            let field = ui.text_edit_singleline(&mut text);
            if field.changed() {
                g.set("CustomScriptsFolder", Value::String(text.clone()));
                changed = true;
            }
            if field.lost_focus() {
                ended = true;
            }
            if ui.button("Browse").clicked() {
                if let Some(picked) = choose_scripts_folder(g) {
                    text = picked;
                    changed = true;
                    ended = true;
                }
            }
        });
        if ended {
            ui.ctx().data_mut(|d| d.remove_temp::<bool>(folder_pending_id()));
        } else if changed {
            note_folder_pending(ui.ctx());
        }
        let cached = ui.ctx().data(|d| d.get_temp::<(String, bool)>(folder_valid_id()));
        let valid = match cached {
            Some((checked, valid)) if checked == text => valid,
            _ => {
                let valid = crate::scripts::custom_folder_valid(&text, home, &crate::scripts::RealFs);
                ui.ctx().data_mut(|d| d.insert_temp(folder_valid_id(), (text.clone(), valid)));
                valid
            }
        };
        let have_font = has_icon_font(ui.ctx());
        if let (Some((glyph, alpha)), true) = (crate::scripts::folder_glyph(on, &text, valid), have_font) {
            let color = ui.visuals().text_color().gamma_multiply(alpha);
            ui.label(egui::RichText::new(glyph.to_string()).family(icon_family()).color(color)).on_hover_text(setting.map_or("", |s| s.label.as_str()));
        }
    })
    .response
    .on_hover_text(setting.map_or("", |s| s.help.as_str()));
    (changed, ended)
}

/// Rows a schema page never draws: the Advanced rows that need Apple Intelligence, which is
/// not available on Windows (ruling 7). Ruling 12e-4 also keeps them out of the schema.
pub fn row_hidden(key: &str) -> bool {
    matches!(key, "aiGeneratedTabTitles" | "logAITabTitleCorpus")
}

/// The global rows a schema page draws, in schema order: the tab's rows without the General
/// tab's "AI" section (`settings_ai` draws it) and without hidden rows.
pub fn page_rows(tab: &str) -> Vec<&'static Setting> {
    schema::global().iter().filter(|s| s.tab == tab && !(tab == "General" && s.section == "AI") && !row_hidden(&s.key)).collect()
}

/// A pane that shows only the global settings the schema has for it so far, by section
/// (General and Appearance). Returns whether a setting changed, and what the custom settings
/// folder's rows asked for.
pub fn schema_page(ui: &mut egui::Ui, g: &mut Globals, tab: &str) -> (bool, PrefsAction) {
    schema_page_with(ui, g, tab, &mut |_, _| false)
}

/// `schema_page` with more drawn after its last section (General's AI section): `extra`
/// gets the globals the scroll area already holds, and its answer joins the result.
pub fn schema_page_with(ui: &mut egui::Ui, g: &mut Globals, tab: &str, extra: &mut dyn FnMut(&mut egui::Ui, &mut Globals) -> bool) -> (bool, PrefsAction) {
    let mut changed = false;
    let mut prefs = PrefsAction::None;
    let home = std::env::var_os("USERPROFILE").map(std::path::PathBuf::from).unwrap_or_default();
    let rows = page_rows(tab);
    egui::ScrollArea::vertical().show(ui, |ui| {
        let mut sections: Vec<&str> = Vec::new();
        for s in &rows {
            if !sections.contains(&s.section.as_str()) {
                sections.push(&s.section);
            }
        }
        for section in sections {
            ui.strong(section);
            for s in rows.iter().copied().filter(|s| s.section == section) {
                if s.key == "CustomScriptsFolder" {
                    // The typed text is saved and applied when editing ends, not per key.
                    let (_, ended) = custom_scripts_folder_row(ui, g, &home);
                    changed |= ended;
                    continue;
                }
                // The custom settings folder's rows draw its text field with the checkbox.
                if s.key == crate::remote_prefs::LOAD_KEY {
                    prefs = prefs_rows(ui, g, &home);
                    changed |= matches!(prefs, PrefsAction::Changed | PrefsAction::CopyNow | PrefsAction::SaveNow);
                    continue;
                }
                if s.key == crate::remote_prefs::FOLDER_KEY {
                    continue;
                }
                let enabled = row_enabled(g, &s.key);
                let was_on = g.bool(&s.key);
                let c = global_control(ui, g, s, enabled);
                changed |= c;
                // Preferring the other kind of GPU takes a restart (`GP:2089`); the banner stays for the window.
                if s.key == "preferIntegratedGPU" {
                    let id = egui::Id::new("tandem-gpu-restart");
                    if c {
                        ui.ctx().data_mut(|d| d.insert_temp(id, true));
                    }
                    if ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false) {
                        ui.label(egui::RichText::new(GPU_RESTART).small().weak());
                    }
                }
                if s.key == "Bidi" {
                    ui.label(egui::RichText::new(EXPERIMENTAL_NOTE).small().weak());
                }
                // Turned on with no folder named: the folder picker opens (`GP:4247-4259`).
                if c && s.key == "UseCustomScriptsFolder" && !was_on && g.bool(&s.key) && g.string("CustomScriptsFolder").is_empty() && choose_scripts_folder(g).is_some() {
                    ui.ctx().data_mut(|d| d.remove_temp::<bool>(folder_pending_id()));
                }
            }
            ui.add_space(8.0);
        }
        changed |= extra(ui, g);
    });
    (changed, prefs)
}

/// What General > Settings' external settings, backup and reset rows asked for: a setting
/// changed; the folder was just picked with the checkbox turned on, so "Copy local settings to
/// custom folder now?" asks (`GP:4262-4292`); Save Now was pressed; or one of the Export All,
/// Import All and Erase All buttons (`GP:4084-4096`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrefsAction {
    None,
    Changed,
    CopyNow,
    SaveNow,
    ExportAll,
    ImportAll,
    EraseAll,
}

/// Where a window's egui memory keeps the last settings location checked and whether it was
/// valid (the check writes a probe file, so it runs when the text changes, not every frame).
fn prefs_valid_id() -> egui::Id {
    egui::Id::new("tandem-prefs-folder-valid")
}

/// Whether the Save changes popup, its label and Save Now take input (`GP:4239-4244`): the
/// setting is on and the location is a valid folder (never a URL).
pub fn prefs_save_enabled(
    g: &Globals,
    home: &std::path::Path,
    read_link: &dyn Fn(&std::path::Path) -> Option<std::path::PathBuf>,
    writable: &dyn Fn(&std::path::Path) -> bool,
) -> bool {
    use crate::remote_prefs as rp;
    let text = g.string(rp::FOLDER_KEY);
    g.bool(rp::LOAD_KEY) && !rp::is_url(&rp::expanded(&text, home)) && rp::location_valid(&text, home, read_link, writable)
}

/// `choosePrefsCustomFolder`: the folder picker; a picked folder becomes the location, which
/// is returned.
fn choose_prefs_folder(g: &mut Globals) -> Option<String> {
    let dir = rfd::FileDialog::new().pick_folder()?;
    let text = dir.display().to_string();
    g.set(crate::remote_prefs::FOLDER_KEY, Value::String(text.clone()));
    Some(text)
}

/// General > Settings' labeled rows (`XIB:8360-8480`): "External settings:" with the custom
/// folder rows, "Backup & restore:" with Export All and Import All one above the other, and
/// "Reset:" with Erase All.
pub fn prefs_rows(ui: &mut egui::Ui, g: &mut Globals, home: &std::path::Path) -> PrefsAction {
    use crate::remote_prefs as rp;
    let mut action = PrefsAction::None;
    egui::Grid::new("tandem-prefs-rows").num_columns(2).show(ui, |ui| {
        ui.label(rp::EXTERNAL_SETTINGS);
        ui.vertical(|ui| action = custom_folder_rows(ui, g, home));
        ui.end_row();
        ui.label(rp::BACKUP_RESTORE);
        ui.vertical(|ui| {
            if ui.button(rp::EXPORT_ALL).clicked() {
                action = PrefsAction::ExportAll;
            }
            if ui.button(rp::IMPORT_ALL).clicked() {
                action = PrefsAction::ImportAll;
            }
        });
        ui.end_row();
        ui.label(rp::RESET);
        if ui.button(rp::ERASE_ALL).clicked() {
            action = PrefsAction::EraseAll;
        }
        ui.end_row();
    });
    action
}

/// The custom folder rows (`GP:2167-2218`, `GP:4221-4292`, `XIB:8360-8440`): the checkbox; the
/// location field and Browse, both enabled with it, and the mark saying whether the location
/// is usable; "Save changes:" with its popup and Save Now, enabled for an on, valid folder.
/// Turning the checkbox on with no location opens the folder picker, and a usable pick offers
/// to copy the local settings there.
fn custom_folder_rows(ui: &mut egui::Ui, g: &mut Globals, home: &std::path::Path) -> PrefsAction {
    use crate::remote_prefs::{self as rp, SaveMode};
    let mut action = PrefsAction::None;
    let was_on = g.bool(rp::LOAD_KEY);
    if let Some(s) = schema::find_global(rp::LOAD_KEY) {
        if global_control(ui, g, s, true) {
            action = PrefsAction::Changed;
        }
    }
    let on = g.bool(rp::LOAD_KEY);
    if on && !was_on && g.string(rp::FOLDER_KEY).is_empty() && choose_prefs_folder(g).is_some() && rp::location_valid(&g.string(rp::FOLDER_KEY), home, &crate::scripts::read_link, &rp::writable) {
        action = PrefsAction::CopyNow;
    }
    let mut text = g.string(rp::FOLDER_KEY);
    let setting = schema::find_global(rp::FOLDER_KEY);
    let mut valid = false;
    ui.horizontal(|ui| {
        ui.add_enabled_ui(on, |ui| {
            let field = ui.add(egui::TextEdit::singleline(&mut text).hint_text(rp::FOLDER_HINT));
            if field.changed() {
                g.set(rp::FOLDER_KEY, Value::String(text.clone()));
                action = action.max_changed();
            }
            if setting.is_some_and(|s| indicate_non_default() && !global_is_default(g, s)) {
                non_default_bar(ui, field.rect, ui.is_enabled());
            }
            if ui.button(rp::BROWSE).clicked() {
                if let Some(picked) = choose_prefs_folder(g) {
                    text = picked;
                    action = action.max_changed();
                }
            }
        });
        let cached = ui.ctx().data(|d| d.get_temp::<(String, bool)>(prefs_valid_id()));
        valid = match cached {
            Some((checked, valid)) if checked == text => valid,
            _ => {
                let valid = rp::location_valid(&text, home, &crate::scripts::read_link, &rp::writable);
                ui.ctx().data_mut(|d| d.insert_temp(prefs_valid_id(), (text.clone(), valid)));
                valid
            }
        };
        if let (Some((glyph, alpha)), true) = (crate::scripts::folder_glyph(on, &text, valid), has_icon_font(ui.ctx())) {
            let color = ui.visuals().text_color().gamma_multiply(alpha);
            ui.label(egui::RichText::new(glyph.to_string()).family(icon_family()).color(color));
        }
    })
    .response
    .on_hover_text(setting.map_or("", |s| s.help.as_str()));
    // The folder's probe (and its link check) answered above; a URL or a network path is never probed.
    let enabled = prefs_save_enabled(g, home, &|_| None, &|_| valid);
    ui.add_enabled_ui(enabled, |ui| {
        ui.horizontal(|ui| {
            ui.label(rp::SAVE_CHANGES);
            let mode = rp::save_mode(g.bool(rp::HAVE_SELECTION_KEY), g.int(rp::SELECTION_KEY));
            let choices = [(rp::AUTOMATICALLY, SaveMode::Always), (rp::MANUALLY, SaveMode::Never), (rp::WHEN_QUITTING, SaveMode::OnQuit)];
            let shown = choices.iter().find(|(_, m)| *m == mode).map_or("", |(label, _)| label);
            let combo = egui::ComboBox::from_id_salt(rp::SELECTION_KEY).selected_text(shown).show_ui(ui, |ui| {
                for (label, m) in choices {
                    if ui.selectable_label(m == mode, label).clicked() {
                        // A pick is the popup's "have selection" (`GP:2179-2187`).
                        g.set(rp::HAVE_SELECTION_KEY, Value::Boolean(true));
                        g.set(rp::SELECTION_KEY, Value::Integer(m.tag()));
                        action = action.max_changed();
                    }
                }
            });
            if schema::find_global(rp::SELECTION_KEY).is_some_and(|s| indicate_non_default() && !global_is_default(g, s)) {
                non_default_bar(ui, combo.response.rect, ui.is_enabled());
            }
            if ui.button(rp::SAVE_NOW).clicked() {
                action = PrefsAction::SaveNow;
            }
        });
    });
    action
}

impl PrefsAction {
    /// Changed, unless something more was asked for.
    fn max_changed(self) -> PrefsAction {
        if self == PrefsAction::None { PrefsAction::Changed } else { self }
    }
}

/// iTerm2's Pointer pane.
pub fn pointer_page(ui: &mut egui::Ui, g: &mut Globals, st: &mut KeysState, profiles: &[(String, String)]) -> bool {
    let mut changed = false;
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.strong("General");
        for s in globals_in("General").into_iter().filter(|s| s.tab == "Pointer") {
            changed |= global_control(ui, g, s, true);
        }
        ui.add_space(8.0);
        ui.strong("Selection");
        for s in globals_in("Selection").into_iter().filter(|s| s.tab == "Pointer") {
            changed |= global_control(ui, g, s, true);
        }
        ui.add_space(8.0);
        ui.strong("Bindings");
        let mut map = g.table("PointerActions");
        match table(ui, "pointer", &pointer_rows(&map), &mut st.pointer_selected) {
            TableAction::Add => {
                st.pointer_editor = Some(PointerEditor { old_key: None, button: 2, clicks: 1, mods: Mods::default(), binding: PointerBinding::new("kIgnoreAction", "") })
            }
            TableAction::Edit(k) => {
                if let (Some((button, clicks, mods)), Some(binding)) = (pointer::parse_key(&k), map.get(&k).and_then(PointerBinding::from_value)) {
                    st.pointer_editor = Some(PointerEditor { old_key: Some(k), button, clicks, mods, binding });
                }
            }
            TableAction::Remove(k) => {
                map.remove(&k);
                st.pointer_selected = None;
                g.set("PointerActions", Value::Table(map.clone()));
                changed = true;
            }
            TableAction::None => {}
        }
    });
    if let Some(e) = st.pointer_editor.as_mut() {
        match pointer_editor(ui.ctx(), e, profiles) {
            Outcome::Open => {}
            Outcome::Cancel => st.pointer_editor = None,
            Outcome::Save => {
                let e = st.pointer_editor.take().unwrap();
                let mut map = g.table("PointerActions");
                if let Some(old) = &e.old_key {
                    map.remove(old);
                }
                let key = pointer::key(e.button, e.clicks, e.mods);
                map.insert(key.clone(), e.binding.to_value());
                st.pointer_selected = Some(key);
                g.set("PointerActions", Value::Table(map));
                changed = true;
            }
        }
    }
    changed
}

/// The profile Keys tab's Key Mappings block: the table, presets, and Delete sends ^H.
/// `global` is the global key map, for the override warning.
pub fn profile_key_mappings(ui: &mut egui::Ui, p: &mut Profile, st: &mut KeysState, profiles: &[(String, String)], global: &toml::Table) -> bool {
    let mut changed = false;
    let mut map = p.get("Keyboard Map").and_then(|v| v.as_table().cloned()).unwrap_or_default();
    ui.add_space(8.0);
    ui.strong("Key Mappings");
    match table(ui, "profile-keys", &rows(&map), &mut st.profile_selected) {
        TableAction::Add => st.key_editor = Some(KeyEditor { target: Target::Profile, old_key: None, stroke: None, binding: Binding::new(-1, ""), recording: false }),
        TableAction::Edit(k) => st.key_editor = editor_for(&map, &k, Target::Profile),
        TableAction::Remove(k) => {
            map.remove(&k);
            st.profile_selected = None;
            changed = true;
        }
        TableAction::None => {}
    }
    ui.horizontal(|ui| {
        ui.label("Presets:");
        for (name, preset) in keymaps::presets() {
            ui.menu_button(name.as_str(), |ui| {
                if ui.button("Replace all mappings").clicked() {
                    map = apply_preset(&map, preset, true);
                    changed = true;
                    ui.close();
                }
                if ui.button("Add to mappings").clicked() {
                    map = apply_preset(&map, preset, false);
                    changed = true;
                    ui.close();
                }
            });
        }
    });
    let mut ctrl_h = delete_sends_ctrl_h(&map);
    let check = ui.checkbox(&mut ctrl_h, "Delete sends ^H");
    if check.changed() {
        set_delete_sends_ctrl_h(&mut map, ctrl_h);
        changed = true;
    }
    // Marked while on (`ProfilesKeysPreferencesViewController.m:549`).
    if indicate_non_default() && ctrl_h {
        non_default_bar(ui, check.rect, ui.is_enabled());
    }
    if st.key_editor.as_ref().is_some_and(|e| e.target == Target::Profile) && finish_key_editor(ui.ctx(), st, &mut map, Some(p.guid()), &|stroke| profile_overrides(global, stroke), profiles) {
        changed = true;
    }
    if changed {
        p.set("Keyboard Map", Value::Table(map));
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 13j: the Advanced > Mouse rows and the three Experimental Features rows, iTerm2's text
    /// with "Tandem" for "iTerm2"; the label is the first line of the help.
    #[test]
    fn mouse_rows_follow_iterm2() {
        use crate::schema::Kind;
        let up = "Scroll wheel up sends the specified text when in alternate screen mode.\nThe value should use Vim syntax, such as \\e for escape.";
        let down = up.replace("wheel up", "wheel down");
        let rows: Vec<(&str, &str, Kind, Value, String)> = vec![
            ("AlternateMouseScrollStringForUp", "Mouse", Kind::String, Value::String(String::new()), up.into()),
            ("AlternateMouseScrollStringForDown", "Mouse", Kind::String, Value::String(String::new()), down),
            ("AlternateMouseScroll", "Mouse", Kind::Bool, Value::Boolean(false), "Scroll wheel sends arrow keys when in alternate screen mode.".into()),
            ("PinchToChangeFontSizeDisabled", "Mouse", Kind::Bool, Value::Boolean(false), "Disable changing font size in response to a pinch gesture.".into()),
            ("UseSystemCursorWhenPossible", "Mouse", Kind::Bool, Value::Boolean(true), "Use system cursor icons when possible.".into()),
            ("AlwaysAcceptFirstMouse", "Mouse", Kind::Bool, Value::Boolean(true), "Always accept first mouse event on terminal windows.\nThis means clicks will work the same when Tandem is active as when it\u{2019}s inactive. Note that there is a separate setting for mouse reporting on first mouse.".into()),
            ("AutodetectMouseReportingStuck", "Mouse", Kind::Bool, Value::Boolean(true), "Automatically detect when mouse reporting got stuck on?\nThis watches for parts of mouse reporting control sequences being printed to the screen".into()),
            ("ReportOnFirstMouse", "Mouse", Kind::Bool, Value::Boolean(false), "Send mouse reports when you click in a window that isn't already frontmouse?".into()),
            ("DoubleReportScrollWheel", "Mouse", Kind::Bool, Value::Boolean(false), "Double-report scroll wheel events to work around tmux scrolling bug.".into()),
            ("StealKeyFocus", "Mouse", Kind::Bool, Value::Boolean(false), "When Focus Follows Mouse is enabled, steal key focus even when inactive.".into()),
            ("AggressiveFocusFollowsMouse", "Mouse", Kind::Bool, Value::Boolean(false), "When Focus Follows Mouse is enabled, activate the window under the cursor when Tandem becomes active?".into()),
            ("SensitiveScrollWheel", "Mouse", Kind::Bool, Value::Boolean(false), "Scroll on any scroll wheel movement, no matter how small?".into()),
            ("ScrollWheelAcceleration", "Mouse", Kind::Float, Value::Float(1.0), "Speed up scroll gestures by this factor.".into()),
            ("NaturalScrollingAffectsHorizontalMouseReporting", "Mouse", Kind::Bool, Value::Boolean(false), "Horizontal scrolling is reversed when \u{201c}Natural scrolling\u{201d} is enabled in Windows Settings".into()),
            ("HorizontalScrollingSensitivity", "Mouse", Kind::Float, Value::Float(0.1), "Sensitivity of mouse wheel for horizontal scrolling.\nUse 0 to disable. Value should be between 0 and 1. Changes to this setting only affect new sessions.".into()),
            ("ProportionalScrollWheelReporting", "Experimental Features", Kind::Bool, Value::Boolean(true), "Report multiple mouse scroll events when scrolling quickly?".into()),
            ("UseModernScrollWheelAccumulator", "Experimental Features", Kind::Bool, Value::Boolean(true), "Use modern scroll wheel accumulator.\nThis should support wheel mice better and feel more natural.".into()),
            ("FastTrackpad", "Experimental Features", Kind::Bool, Value::Boolean(true), "Trackpad scrolls fast?\nSet to No for legacy scrolling speed.".into()),
        ];
        assert_eq!(rows.len(), 18);
        for (key, section, kind, default, help) in rows {
            let s = schema::find_global(key).unwrap_or_else(|| panic!("{key} is missing"));
            assert_eq!(s.tab, "Advanced", "{key} tab");
            assert_eq!(s.section, section, "{key} section");
            assert_eq!(s.kind, kind, "{key} kind");
            assert_eq!(s.default.as_ref(), Some(&default), "{key} default");
            assert_eq!(s.help, help, "{key} help");
            assert_eq!(s.label, help.lines().next().unwrap(), "{key} label");
            // The Advanced pane must draw it with the right control (preflight P9).
            let want = match kind {
                Kind::Bool => crate::advanced_pane::RowKind::Bool,
                Kind::Float => crate::advanced_pane::RowKind::Float,
                _ => crate::advanced_pane::RowKind::Text,
            };
            let row = crate::advanced_pane::rows().into_iter().find(|r| r.key == key).unwrap_or_else(|| panic!("{key} has no pane row"));
            assert_eq!(row.kind, want, "{key} row kind");
        }
    }

    #[test]
    fn paste_configurations_show_their_summary() {
        let b = Binding::new(41, r#"{"Base64":true,"RemoveControls":true,"BracketAllowed":true,"ConvertNewlines":true}"#);
        assert_eq!(action_summary(&b), "Paste...: Base64");
    }

    #[test]
    fn a_preset_can_replace_or_merge() {
        let mut map: toml::Table = toml::from_str("\"0x61-0x100000\" = { Action = 13 }").unwrap();
        let nat = &crate::globals::keymaps::presets()[0].1;
        assert_eq!(apply_preset(&map, nat, true).len(), 8);
        map = apply_preset(&map, nat, false);
        assert_eq!(map.len(), 9);
    }

    #[test]
    fn delete_sends_ctrl_h_is_a_binding() {
        let mut map = toml::Table::new();
        set_delete_sends_ctrl_h(&mut map, true);
        assert_eq!(map["0x7f-0x0-0x33"]["Action"].as_integer(), Some(16));
        assert!(delete_sends_ctrl_h(&map));
        set_delete_sends_ctrl_h(&mut map, false);
        assert!(map.is_empty());
        // A backspace binding stored in the legacy form counts too.
        let old: toml::Table = toml::from_str("\"0x7f-0x0\" = { Action = 16 }").unwrap();
        assert!(delete_sends_ctrl_h(&old));
    }

    #[test]
    fn toggle_setting_never_offers_ai_consent() {
        let keys: Vec<String> = toggle_choices().into_iter().map(|(k, _, _)| k).collect();
        for key in ["GenerativeAIAllowed", "EnableAI", "AICompletions"] {
            assert!(!keys.iter().any(|k| k == key), "{key}");
        }
        // Other AI checkboxes and ordinary settings are still offered.
        assert!(keys.iter().any(|k| k == "AI Safety Check"));
        assert!(keys.iter().any(|k| k == "FocusFollowsMouse"));
    }

    #[test]
    fn rows_show_labels() {
        let map: toml::Table = toml::from_str("\"0x9-0x40000\" = { Action = 32, Text = \"\" }").unwrap();
        assert_eq!(rows(&map), [("0x9-0x40000".to_string(), "Ctrl+Tab".to_string(), "Cycle Tabs Forward".to_string())]);
        let hex: toml::Table = toml::from_str("\"0x7f-0x80000\" = { Action = 11, Text = \"0x1b 0x7f\" }").unwrap();
        assert_eq!(rows(&hex)[0].2, "Send Hex Code: 0x1b 0x7f");
    }

    #[test]
    fn the_recorder_takes_a_leader_sequence() {
        let ctrl_b = Keystroke::new(0x62, keystroke::CONTROL, None, 0x62);
        let mut st = KeysState { leader: Some(ctrl_b.clone()), ..KeysState::default() };
        st.key_editor = Some(KeyEditor { target: Target::Global, old_key: None, stroke: None, binding: Binding::new(-1, ""), recording: true });
        st.deliver(ctrl_b);
        assert!(st.recording());
        st.deliver(Keystroke::new(0x73, 0, None, 0x73));
        let e = st.key_editor.as_ref().unwrap();
        assert!(!e.recording);
        assert_eq!(e.stroke.as_ref().unwrap().label(), "Leader, S");
    }

    #[test]
    fn api_rows_follow_iterm2() {
        use crate::api_auth::{ALLOW_ALL_APPS, AUTH_METHOD_LABEL, REQUIRE_COOKIE};
        let enable = schema::find_global("EnableAPIServer").unwrap();
        assert_eq!((enable.tab.as_str(), enable.section.as_str(), enable.label.as_str()), ("General", "Python API", "Enable Python API"));
        assert_eq!(enable.kind, Kind::Bool);
        assert_eq!(enable.default, Some(Value::Boolean(false)));
        let auth = schema::find_global("API Authentication Method").unwrap();
        assert_eq!((auth.tab.as_str(), auth.section.as_str(), auth.label.as_str()), ("General", "Python API", AUTH_METHOD_LABEL));
        assert_eq!(auth.kind, Kind::Enum);
        let choices: Vec<(Value, &str)> = auth.choices.iter().map(|c| (c.value.clone(), c.label.as_str())).collect();
        assert_eq!(choices, [(Value::Integer(0), REQUIRE_COOKIE), (Value::Integer(1), ALLOW_ALL_APPS)]);
        assert_eq!(auth.default, Some(Value::Integer(0)));
        let cookie = schema::find_global("setCookie").unwrap();
        assert_eq!((cookie.tab.as_str(), cookie.section.as_str()), ("Advanced", "Scripting"));
        assert_eq!(cookie.kind, Kind::Bool);
        assert_eq!(cookie.default, Some(Value::Boolean(false)));
        assert_eq!(cookie.label, "Set ITERM2_COOKIE environment variable, allowing Python scripts to be launched without confirmation?");
        assert_eq!(
            cookie.help,
            "Set ITERM2_COOKIE environment variable, allowing Python scripts to be launched without confirmation?\nThis will only affect sessions created after changing this setting."
        );
        let cap = schema::find_global("maximumBytesToProvideToPythonAPI").unwrap();
        assert_eq!((cap.tab.as_str(), cap.section.as_str()), ("Advanced", "General"));
        assert_eq!(cap.kind, Kind::Int);
        assert_eq!(cap.default, Some(Value::Integer(100)));
        assert_eq!(cap.label, "Maximum number of bytes of selection to provide to Python API.");
        assert_eq!(cap.help, "Maximum number of bytes of selection to provide to Python API.\nA large value here can cause performance issues when you have a big selection.");
        let lax = schema::find_global("laxNilPolicyInInterpolatedStrings").unwrap();
        assert_eq!((lax.tab.as_str(), lax.section.as_str()), ("Advanced", "Scripting"));
        assert_eq!(lax.kind, Kind::Bool);
        assert_eq!(lax.default, Some(Value::Boolean(true)));
        // The popup is enabled only while the API is on; every other row always.
        let mut g = Globals::empty(std::path::Path::new("api-rows-test.toml"));
        assert!(!row_enabled(&g, "API Authentication Method"));
        assert!(row_enabled(&g, "EnableAPIServer"));
        assert!(row_enabled(&g, "setCookie"));
        g.set("EnableAPIServer", Value::Boolean(true));
        assert!(row_enabled(&g, "API Authentication Method"));
        assert!(row_enabled(&g, "IRMemory"));
    }

    #[test]
    fn pane_title_rows_follow_iterm2() {
        let row = |key: &str| schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
        let link = "\nShould be a web-style color, #rrggbb. You can also use p3#rrggbb for p3 color space.";
        let color = |label: &str| (label.to_string(), format!("{label}{link}"));
        let mut rows: Vec<(&str, &str, &str, Kind, Value, String, Option<String>)> = vec![
            ("ShowPaneTitles", "Appearance", "Panes", Kind::Bool, Value::Boolean(true), "Show per-pane title bars".into(), None),
            ("ShowPaneTitlesEvenIfOnlyOnePane", "Appearance", "Panes", Kind::Bool, Value::Boolean(false), "\u{2026}even if there is only one pane".into(), None),
            (
                "PaneTitleBarBackgroundColor", "Advanced", "Windows", Kind::String, Value::String(String::new()),
                "Custom background color for pane title bars. Leave empty to use default color.".into(),
                Some("Custom background color for pane title bars. Leave empty to use default color.\nThis should be an HTML-style color, like #aabbcc.".into()),
            ),
            (
                "PaneTitleBarTextColor", "Advanced", "Windows", Kind::String, Value::String(String::new()),
                "Custom text color for pane title bars. Leave empty to use default color.".into(),
                Some("Custom text color for pane title bars. Leave empty to use default color.\nThis should be an HTML-style color, like #aabbcc.".into()),
            ),
            (
                "RequireOptionToDragSplitPaneTitleBar", "Advanced", "Mouse", Kind::Bool, Value::Boolean(false),
                "Require Alt to be pressed to drag a split pane by its title bar?".into(),
                Some("Require Alt to be pressed to drag a split pane by its title bar?\nThis helps prevent accidental drags.".into()),
            ),
        ];
        for (key, default, what) in [
            ("SplitPaneSourceFillColor", "#008000", "Color of source pane fill when moving/swapping a split pane."),
            ("SplitPaneSourceBorderColor", "#ffffff", "Color of source pane outer border when moving/swapping a split pane."),
            ("SplitPaneSourceInnerBorderColor", "#000000", "Color of source pane inner border when moving/swapping a split pane."),
            ("SplitPaneTargetDropFillColor", "#800000", "Color of drop target fill when dragging a split pane."),
            ("SplitPaneTargetDropBorderColor", "#ffffff", "Color of drop target outer border when dragging a split pane."),
            ("SplitPaneTargetDropInnerBorderColor", "#000000", "Color of drop target inner border when dragging a split pane."),
        ] {
            let (label, help) = color(what);
            rows.push((key, "Advanced", "Tabs", Kind::String, Value::String(default.into()), label, Some(help)));
        }
        assert_eq!(rows.len(), 11);
        for (key, tab, section, kind, default, label, help) in rows {
            let s = row(key);
            assert_eq!((s.tab.as_str(), s.section.as_str(), s.label.as_str()), (tab, section, label.as_str()), "{key}");
            assert_eq!(s.kind, kind, "{key}");
            assert_eq!(s.default, Some(default), "{key}");
            if let Some(help) = help {
                assert_eq!(s.help, help, "{key}");
            }
        }
        // Ruling R3: iTerm2's default is on (`iTermPreferences.m:827`).
        // The second row follows the first (`APV:228-231`).
        let mut g = Globals::empty(std::path::Path::new("pane-title-rows-test.toml"));
        assert_eq!(g.bool("ShowPaneTitles"), true);
        g.set("ShowPaneTitles", Value::Boolean(false));
        assert!(!row_enabled(&g, "ShowPaneTitlesEvenIfOnlyOnePane"));
        g.set("ShowPaneTitles", Value::Boolean(true));
        assert!(row_enabled(&g, "ShowPaneTitlesEvenIfOnlyOnePane"));
    }

    #[test]
    fn prefs_rows_follow_iterm2() {
        let row = |key: &str| schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
        for (key, tab, section, kind, default, label) in [
            ("LoadPrefsFromCustomFolder", "General", "Settings", Kind::Bool, Value::Boolean(false), Some("Load settings from a custom folder or URL")),
            ("PrefsCustomFolder", "General", "Settings", Kind::String, Value::String(String::new()), Some("Custom folder")),
            ("NoSyncNeverRemindPrefsChangesLostForFile_selection", "Stored", "Settings", Kind::Int, Value::Integer(0), None),
            ("NoSyncPromptBeforeLoadingPrefsFromURL", "Stored", "Settings", Kind::String, Value::String(String::new()), None),
            (
                "NoSyncNeverRemindPrefsChangesLostForFile",
                "Advanced",
                "Warnings",
                Kind::Bool,
                Value::Boolean(false),
                Some("Suppress changed-setting warning when settings are loaded from a custom folder."),
            ),
            (
                "NoSyncNeverRemindPrefsChangesLostForUrl",
                "Advanced",
                "Warnings",
                Kind::Bool,
                Value::Boolean(false),
                Some("Suppress changed-setting warning when settings are loaded from a URL."),
            ),
            ("NoSyncDownloadPrefsTimeout", "Advanced", "General", Kind::Float, Value::Float(5.0), Some("Timeout for downloading settings")),
        ] {
            let s = row(key);
            assert_eq!((s.tab.as_str(), s.section.as_str()), (tab, section), "{key}");
            assert_eq!(s.kind, kind, "{key}");
            assert_eq!(s.default, Some(default), "{key}");
            if let Some(label) = label {
                assert_eq!(s.label, label, "{key}");
                if tab == "Advanced" {
                    assert_eq!(s.help, label, "{key}");
                }
            }
        }
        assert_eq!(
            row("PrefsCustomFolder").help,
            "The folder (or http:// or https:// URL) holding tandem-settings.toml. Network paths are not supported."
        );
        // The rows sit before "Save copy/paste and command history to disk" (Task 8's order).
        let general: Vec<&str> = schema::global().iter().filter(|s| s.tab == "General" && s.section == "Settings").map(|s| s.key.as_str()).collect();
        assert_eq!(&general[..3], ["LoadPrefsFromCustomFolder", "PrefsCustomFolder", "SavePasteHistory"]);
    }

    /// m13i Task 1: the pane navigation rows (`AS:311`, `:510`, `:667`, `:715`, `:716`, `:881`).
    #[test]
    fn navigation_rows_follow_iterm2() {
        for (key, section, kind, default, label) in [
            ("NavigatePanesInReadingOrder", "Tabs", Kind::Bool, Value::Boolean(true), "Next Pane and Previous Pane commands use reading order, not the time of last use."),
            ("WrapFocus", "General", Kind::Bool, Value::Boolean(true), "Should split pane navigation by direction wrap around?"),
            ("FocusNewSplitPaneWithFocusFollowsMouse", "Session", Kind::Bool, Value::Boolean(true), "When focus follows mouse is enabled, should new split panes automatically be focused?"),
            ("SplitPaneColor", "Windows", Kind::String, Value::String(String::new()), "Custom color for split pane dividers. Leave empty to use default color."),
            ("SplitPaneDividerWidth", "Windows", Kind::Float, Value::Float(0.0), "Custom width for split pane dividers in points (1-5). Set to 0 to use default width."),
            ("UseDivorcedProfileToSplit", "Experimental Features", Kind::Bool, Value::Boolean(true), "When splitting a pane, use the profile with local modifications, not the backing profile."),
        ] {
            let s = schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!((s.tab.as_str(), s.section.as_str(), s.kind), ("Advanced", section, kind), "{key}");
            assert_eq!(s.default, Some(default), "{key}");
            assert_eq!(s.label, label, "{key}");
            let help = if key == "SplitPaneColor" { format!("{label}\nThis should be an HTML-style color, like #aabbcc.") } else { label.to_string() };
            assert_eq!(s.help, help, "{key}");
            assert!(page_rows("Advanced").iter().any(|r| r.key == key), "{key} is shown");
        }
    }

    /// m13i Task 1: the Appearance pane's Dimming box (`APV:351-374`, `PREF:134-142`).
    #[test]
    fn dimming_rows_follow_iterm2() {
        for (key, kind, default, label) in [
            ("DimInactiveSplitPanes", Kind::Bool, Value::Boolean(true), "Dim inactive split panes"),
            ("DimOnlyText", Kind::Bool, Value::Boolean(false), "Dimming affects only text, not background."),
            ("SplitPaneDimmingAmount", Kind::Float, Value::Float(0.4), "Dimming amount:"),
            ("DimBackgroundWindows", Kind::Bool, Value::Boolean(false), "Dim background windows"),
        ] {
            let s = schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!((s.tab.as_str(), s.section.as_str(), s.kind), ("Appearance", "Dimming", kind), "{key}");
            assert_eq!(s.default, Some(default), "{key}");
            assert_eq!(s.label, label, "{key}");
        }
        // iTerm2's xib order: the slider, then the three checkboxes.
        let order: Vec<&str> = page_rows("Appearance").iter().filter(|s| s.section == "Dimming").map(|s| s.key.as_str()).collect();
        assert_eq!(order, ["SplitPaneDimmingAmount", "DimInactiveSplitPanes", "DimBackgroundWindows", "DimOnlyText"]);
        let amount = schema::find_global("SplitPaneDimmingAmount").unwrap();
        assert_eq!((amount.min_f64(), amount.max_f64()), (Some(0.01), Some(0.6)));
        assert_eq!(amount.help, "Inactive sessions get dimmed to help find the active session.");
        let mut g = Globals::empty(std::path::Path::new("dimming-rows-test.toml"));
        g.set("DimInactiveSplitPanes", Value::Boolean(false));
        g.set("DimBackgroundWindows", Value::Boolean(false));
        for key in ["DimOnlyText", "SplitPaneDimmingAmount"] {
            assert!(!row_enabled(&g, key), "{key} off with both switches off");
            g.set("DimInactiveSplitPanes", Value::Boolean(true));
            assert!(row_enabled(&g, key), "{key} on with the panes switch");
            g.set("DimInactiveSplitPanes", Value::Boolean(false));
            g.set("DimBackgroundWindows", Value::Boolean(true));
            assert!(row_enabled(&g, key), "{key} on with the windows switch");
            g.set("DimBackgroundWindows", Value::Boolean(false));
        }
        assert!(row_enabled(&g, "DimInactiveSplitPanes") && row_enabled(&g, "DimBackgroundWindows"));
    }

    #[test]
    fn export_rows_follow_iterm2() {
        let s = schema::find_global("RevealExportedSettingsAndData").expect("row");
        assert_eq!((s.tab.as_str(), s.section.as_str(), s.kind), ("Advanced", "General", Kind::Bool));
        assert_eq!(s.default, Some(Value::Boolean(true)));
        let text = "Reveal file created by \u{201c}Export All Settings and Data\u{201d} in Explorer upon completion";
        assert_eq!((s.label.as_str(), s.help.as_str()), (text, text));
    }

    #[test]
    fn warning_rows_follow_iterm2() {
        let s = schema::find_global("AlertsIndicateShortcuts").expect("row");
        assert_eq!((s.tab.as_str(), s.section.as_str(), s.kind), ("Advanced", "General", Kind::Bool));
        assert_eq!(s.default, Some(Value::Boolean(false)));
        let label = "Buttons in modal alerts indicate keyboard shortcuts.";
        assert_eq!(s.label, label);
        assert_eq!(s.help, format!("{label}\nDo you miss Windows 95? I do."));
        let c = schema::find_global(crate::warnings::CATALOG_KEY).expect("row");
        assert_eq!((c.tab.as_str(), c.section.as_str(), c.kind), ("Stored", "Warnings", Kind::Dict));
        assert_eq!(c.default, Some(Value::Table(toml::Table::new())));
    }

    /// The confirmations' rows (`ASM:504,668,785,787`).
    #[test]
    fn confirmation_rows_follow_iterm2() {
        let rows = [
            ("NoSyncSuppressRestartSessionConfirmationAlert", "Session", Some(false), "Suppress restart session confirmation alert.", "Suppress restart session confirmation alert.\nDon't ask for a confirmation when manually restarting a session."),
            ("NoSyncSuppressDownloadConfirmation", "General", Some(false), "Suppress confirmation of terminal-initiated downloads?", "Suppress confirmation of terminal-initiated downloads?"),
            ("NoSyncClearAllBroadcast", "Warnings", None, "Send Clear Session to all broadcasted-to sessions?", "Send Clear Session to all broadcasted-to sessions?"),
            ("NoSyncConfirmRemoveAnnotation", "Warnings", Some(false), "Suppress confirmation to remove annotation?", "Suppress confirmation to remove annotation?"),
        ];
        for (key, section, default, label, help) in rows {
            let s = schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!((s.tab.as_str(), s.section.as_str(), s.kind), ("Advanced", section, Kind::Bool), "{key}");
            assert_eq!(s.default, default.map(Value::Boolean), "{key}");
            assert_eq!((s.label.as_str(), s.help.as_str()), (label, help), "{key}");
        }
        assert!(schema::find_global("NoSyncClearAllBroadcast").unwrap().optional_bool());
    }

    /// The arrangement announcements' rows (`ASM:779-781`).
    #[test]
    fn arrangement_rows_follow_iterm2() {
        let rows = [
            ("NoSyncSuppressMissingProfileInArrangementWarning", "Suppress the notification that a restored session\u{2019}s profile no longer exists?"),
            ("NoSyncSuppressBadPWDInArrangementWarning", "Suppress the notification that a saved arrangement has a bad initial working directory."),
        ];
        for (key, text) in rows {
            let s = schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!((s.tab.as_str(), s.section.as_str(), s.kind), ("Advanced", "Warnings", Kind::Bool), "{key}");
            assert_eq!(s.default, Some(Value::Boolean(false)), "{key}");
            assert_eq!((s.label.as_str(), s.help.as_str()), (text, text), "{key}");
        }
    }

    #[test]
    fn erase_row_follows_iterm2() {
        let s = schema::find_global("DryRunEraseAllSettingsAndData").expect("row");
        assert_eq!((s.tab.as_str(), s.section.as_str(), s.kind), ("Advanced", "General", Kind::Bool));
        assert_eq!(s.default, Some(Value::Boolean(false)));
        let label = "Dry-run \u{201c}Erase All Settings and Data\u{201d}.";
        assert_eq!(s.label, label);
        assert_eq!(
            s.help,
            format!("{label}\nWhen enabled, the Erase All Settings and Data button logs what it would delete (to the debug output) instead of actually deleting anything, and does not quit. Intended for testing the feature.")
        );
    }

    #[test]
    fn prefs_row_enabling_follows_iterm2() {
        use crate::remote_prefs::{FOLDER_KEY, LOAD_KEY};
        let home = std::path::Path::new(r"C:\Users\u");
        let mut g = Globals::empty(std::path::Path::new("prefs-rows-test.toml"));
        assert!(!row_enabled(&g, FOLDER_KEY));
        g.set(LOAD_KEY, Value::Boolean(true));
        assert!(row_enabled(&g, FOLDER_KEY));
        let writable = |p: &std::path::Path| p == std::path::Path::new(r"D:\s");
        g.set(FOLDER_KEY, Value::String("https://h/p".into()));
        assert!(!prefs_save_enabled(&g, home, &|_| None, &writable), "a URL is never saved to");
        g.set(FOLDER_KEY, Value::String(r"D:\s".into()));
        assert!(prefs_save_enabled(&g, home, &|_| None, &writable));
        let to_share = |p: &std::path::Path| (p == std::path::Path::new(r"D:\s")).then(|| std::path::PathBuf::from(r"\\h\s"));
        assert!(!prefs_save_enabled(&g, home, &to_share, &writable), "a folder linked to a network path");
        g.set(FOLDER_KEY, Value::String(r"D:\t".into()));
        assert!(!prefs_save_enabled(&g, home, &|_| None, &writable), "a folder that cannot be written");
        g.set(FOLDER_KEY, Value::String(r"D:\s".into()));
        g.set(LOAD_KEY, Value::Boolean(false));
        assert!(!prefs_save_enabled(&g, home, &|_| None, &writable), "off");
        assert!(!row_enabled(&g, FOLDER_KEY));
    }

    #[test]
    fn restore_spaces_row_follows_windows() {
        let s = schema::find_global("RestoreWindowsToSameSpaces").unwrap();
        assert_eq!((s.tab.as_str(), s.section.as_str(), s.label.as_str()), ("General", "Startup", "Restore windows to same desktops"));
        assert_eq!(s.kind, Kind::Bool);
        assert_eq!(s.default, Some(Value::Boolean(false)));
        let mut g = Globals::empty(std::path::Path::new("restore-spaces-test.toml"));
        for (arr, none) in [(false, false), (true, false), (false, true), (true, true)] {
            g.set("OpenArrangementAtStartup", Value::Boolean(arr));
            g.set("OpenNoWindowsAtStartup", Value::Boolean(none));
            assert_eq!(row_enabled(&g, "RestoreWindowsToSameSpaces"), crate::desktops::restore_row_enabled(arr, none));
        }
    }

    #[test]
    fn scripts_folder_rows_follow_iterm2() {
        let use_custom = schema::find_global("UseCustomScriptsFolder").unwrap();
        assert_eq!((use_custom.tab.as_str(), use_custom.section.as_str(), use_custom.label.as_str()), ("General", "Python API", "Custom Python API Scripts Folder"));
        assert_eq!(use_custom.kind, Kind::Bool);
        assert_eq!(use_custom.default, Some(Value::Boolean(false)));
        let custom = schema::find_global("CustomScriptsFolder").unwrap();
        assert_eq!((custom.tab.as_str(), custom.section.as_str(), custom.label.as_str()), ("General", "Python API", "Custom folder for Python API scripts"));
        assert_eq!(custom.kind, Kind::String);
        assert_eq!(custom.default, Some(Value::String(String::new())));
        let help = "Tandem looks for Python API scripts in this folder instead of %APPDATA%\\Tandem\\Scripts. It must exist and have no spaces in its path. Network paths are not supported.";
        assert_eq!((use_custom.help.as_str(), custom.help.as_str()), (help, help));
        // The field follows the checkbox (`GP:2152-2154`).
        let mut g = Globals::empty(std::path::Path::new("scripts-rows-test.toml"));
        assert!(!row_enabled(&g, "CustomScriptsFolder"));
        assert!(row_enabled(&g, "UseCustomScriptsFolder"));
        g.set("UseCustomScriptsFolder", Value::Boolean(true));
        assert!(row_enabled(&g, "CustomScriptsFolder"));
    }

    #[test]
    fn generous_row_follows_iterm2() {
        let row = schema::find_global("FontsForGenerousRounding").unwrap();
        assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", "Experimental Features"));
        assert_eq!(row.kind, Kind::String);
        assert_eq!(row.default, Some(Value::String("consolas".into())));
        assert_eq!(row.label, "List of fonts to use alternate rounding algorithm for line height calculation.");
        assert!(row.help.starts_with(&row.label) && row.help.contains("Terminal.app\u{2019}s behavior"), "{}", row.help);
    }

    #[test]
    fn bold_row_follows_iterm2() {
        let row = schema::find_global("MinimumWeightDifferenceForBoldFont").unwrap();
        assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", "Drawing"));
        assert_eq!(row.kind, Kind::Int);
        assert_eq!(row.default, Some(Value::Integer(4)));
        assert_eq!(row.label, "Minimum weight difference between regular and bold font.");
        assert!(row.help.starts_with(&row.label) && row.help.contains("double-struck with a small offset."), "{}", row.help);
    }

    #[test]
    fn ligature_rows_follow_iterm2() {
        for (key, label) in [
            ("PreferSpeedToFullLigatureSupport", "Improves drawing performance at the expense of disallowing alphanumeric characters to belong to ligatures."),
            ("EnableContextualAlternates", "Enable contextual alternates (calt) for ligature-capable fonts."),
        ] {
            let row = schema::find_global(key).unwrap_or_else(|| panic!("{key} has a row"));
            assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", "Drawing"), "{key}");
            assert_eq!(row.kind, Kind::Bool, "{key}");
            assert_eq!(row.default, Some(Value::Boolean(true)), "{key}");
            assert_eq!(row.label, label, "{key}");
            assert!(row.help.starts_with(label), "{key}: {}", row.help);
        }
        let calt = schema::find_global("EnableContextualAlternates").unwrap();
        assert!(calt.help.contains("Many coding fonts such as Monaspace, Iosevka, and Cascadia Code require this for their ligatures to work."), "{}", calt.help);
    }

    #[test]
    fn box_drawing_row_follows_iterm2() {
        let row = schema::find_global("DisableCustomBoxDrawing").expect("a row");
        assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", "Drawing"));
        assert_eq!(row.kind, Kind::Bool);
        assert_eq!(row.default, Some(Value::Boolean(false)));
        assert_eq!(row.label, "Use your typeface\u{2019}s box-drawing characters instead of Tandem\u{2019}s custom drawing code.");
        assert_eq!(row.help, format!("{}\nYou must restart Tandem after changing this setting.", row.label));
    }

    #[test]
    fn powerline_rows_follow_iterm2() {
        let wide = schema::find_global("MakeSomePowerlineSymbolsWide").expect("a row");
        assert_eq!((wide.tab.as_str(), wide.section.as_str()), ("Advanced", "Drawing"));
        assert_eq!(wide.kind, Kind::Bool);
        assert_eq!(wide.default, Some(Value::Boolean(true)));
        assert_eq!(wide.label, "Draw certain Powerline symbols double-width?");
        assert_eq!(wide.help, "Draw certain Powerline symbols double-width?\nThis matches how most \u{201c}nerd\u{201d} fonts render them, but your favorite one might not do this. An example code point that is affected is U+E0B8.");
        let ext = schema::find_global("SupportPowerlineExtendedSymbols").expect("a row");
        assert_eq!((ext.tab.as_str(), ext.section.as_str()), ("Advanced", "Drawing"));
        assert_eq!(ext.kind, Kind::Bool);
        assert_eq!(ext.default, Some(Value::Boolean(true)));
        assert_eq!(ext.label, "Include extended symbols when drawing Powerline natively.");
        assert_eq!(ext.help, ext.label);
    }

    #[test]
    fn anti_alias_rows_follow_iterm2() {
        let row = schema::find_global("ForceAntialiasingOnRetina").expect("a row");
        assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", "Drawing"));
        assert_eq!(row.kind, Kind::Bool);
        assert_eq!(row.default, Some(Value::Boolean(false)));
        assert_eq!(row.label, "Force text to be anti-aliased on Retina displays.");
        assert!(row.help.starts_with(&row.label) && row.help.ends_with("which sometimes looks better."), "{}", row.help);
        // The stock profile seeds both anti-alias switches on and Thin Strokes at Retina only.
        let stock = crate::profile::Profile::stock();
        assert!(stock.bool("ASCII Anti Aliased") && stock.bool("Non-ASCII Anti Aliased"));
        assert_eq!(stock.int("Thin Strokes"), 4);
    }

    #[test]
    fn scrollbar_tick_rows_follow_iterm2() {
        let locations = "Show search result and prompt locations in scroll bar.\nChanging this setting will not affect existing sessions until you restart.";
        let marks = "Show prompt locations in scroll bar?\nChanging this setting will not affect existing sessions until you restart. You must also enable \u{201c}Show search result and prompt locations in scroll bar\u{201d} for this to take effect.";
        for (key, help) in [("ShowLocationsInScrollbar", locations), ("ShowMarksInScrollbar", marks)] {
            let row = schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", "Experimental Features"), "{key}");
            assert_eq!((row.kind, row.default.clone()), (Kind::Bool, Some(Value::Boolean(true))), "{key}");
            assert_eq!(row.help.as_str(), help, "{key}");
            assert_eq!(row.label.as_str(), help.lines().next().unwrap(), "{key}");
        }
    }

    #[test]
    fn hide_scrollbar_row_follows_iterm2() {
        let row = schema::find_global("HideScrollbar").unwrap();
        assert_eq!((row.tab.as_str(), row.section.as_str()), ("Appearance", "Windows"));
        assert_eq!(row.kind, Kind::Bool);
        assert_eq!(row.default, Some(Value::Boolean(false)));
        assert_eq!((row.label.as_str(), row.help.as_str()), ("Hide scrollbars", "Hide scrollbars"));
    }

    #[test]
    fn line_style_marks_row_follows_iterm2() {
        let row = schema::find_global("AlwaysUseLineStyleMarks").unwrap();
        assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", "Drawing"));
        assert_eq!(row.kind, Kind::Bool);
        assert_eq!(row.default, Some(Value::Boolean(false)));
        assert_eq!((row.label.as_str(), row.help.as_str()), ("Always use line-style marks?", "Always use line-style marks?"));
    }

    #[test]
    fn bidi_rows_follow_iterm2() {
        use crate::bidi::Rules;
        let detect = "Auto-detect paragraph writing direction based on the first strong directional character?\nRequires right-to-left text support to be enabled.";
        let justify = "Right-justify lines in paragraphs with base writing direction of right-to-left?\nRequires BOTH \u{201c}right-to-left text support\u{201d} and \u{201c}auto-detect paragraph writing detection\u{201d} to be enabled.";
        let alt = "Do not reorder right-to-left text in the alternate screen?\nFull-screen apps (editors and some CLIs) position their own text and may already order it for display, so reordering it again fights their layout. Turn this on to leave alternate-screen text unreordered. Requires right-to-left text support.";
        for (key, default, help) in [("DetectParagraphDirection", false, detect), ("RightJustifyRTLLines", true, justify), ("DisableBidiInAlternateScreen", false, alt)] {
            let row = schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", "Experimental Features"), "{key}");
            assert_eq!((row.kind, row.default.clone()), (Kind::Bool, Some(Value::Boolean(default))), "{key}");
            assert_eq!(row.help.as_str(), help, "{key}");
            assert_eq!(row.label.as_str(), help.lines().next().unwrap(), "{key}");
        }
        // The rules read the four rows, the General one included.
        let mut g = Globals::empty(&std::env::temp_dir().join("tandem-bidi-rows.toml"));
        assert_eq!(Rules::from_globals(&g), Rules { enabled: true, detect_direction: false, right_justify: true, off_in_alternate: false });
        g.set("Bidi", Value::Boolean(false));
        g.set("DetectParagraphDirection", Value::Boolean(true));
        g.set("RightJustifyRTLLines", Value::Boolean(false));
        g.set("DisableBidiInAlternateScreen", Value::Boolean(true));
        assert_eq!(Rules::from_globals(&g), Rules { enabled: false, detect_direction: true, right_justify: false, off_in_alternate: true });
    }

    #[test]
    fn color_space_rows_follow_iterm2() {
        let p3 = "Use P3 as default color space? If No, sRGB will be used.";
        let row = schema::find_global("P3").unwrap();
        assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", "Terminal"));
        assert_eq!((row.kind, row.default.clone()), (Kind::Bool, Some(Value::Boolean(true))));
        assert_eq!((row.label.as_str(), row.help.as_str()), (p3, p3));
        let hdr = "Allow HDR effects\nWhen enabled, Tandem may render some UI effects, such as the selected tab's outline, brighter than white on an HDR-capable display. Off by default because forcing the display into extended dynamic range can cause hangs on some systems.";
        let row = schema::find_global("AllowHDR").unwrap();
        assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", "Drawing"));
        assert_eq!((row.kind, row.default.clone()), (Kind::Bool, Some(Value::Boolean(false))));
        assert_eq!((row.label.as_str(), row.help.as_str()), ("Allow HDR effects", hdr));
    }

    /// 13p's eight Advanced rows (`AS:409`, `429`, `450`, `487`, `488`, `498`, `517`, `855`):
    /// tab, section, kind, default, label and help as iTerm2 words them; `DoNotSetCtype` is
    /// inert in iTerm2 and so here (ruling R2).
    #[test]
    fn session_start_rows_follow_iterm2() {
        use crate::advanced_pane::RowKind;
        let ctype = "Never set the CTYPE environment variable.";
        let colorfgbg = "Use fallback for COLORFGBG if no exact match found?";
        let terminfo = "Set $TERMINFO_DIRS to add modern terminal features to xterm-like $TERMs?";
        let battery = "Prevent system idle sleep on battery too, for profiles with \u{201c}Prevent sleep\u{201d} enabled?";
        let idle = "Time in seconds before a session is considered idle.";
        let jiggle = "Redraw the screen after the Clear Buffer menu item is selected.";
        let lcctype = "Value to set LC_CTYPE to if the machine\u{2018}s combination of country and language are not supported.";
        let fast = "Enable low-latency updates of the current foreground job";
        let rows: [(&str, &str, Kind, Value, &str, String); 8] = [
            ("DoNotSetCtype", "Terminal", Kind::Bool, Value::Boolean(false), ctype, ctype.to_string()),
            ("UseColorfgbgFallback", "Terminal", Kind::Bool, Value::Boolean(true), colorfgbg, format!("{colorfgbg}\nThe COLORFGBG variable indicates the ANSI colors that match the foreground and background colors. If no colors match and this setting is enabled, then the variable will be set to 15;0 to indicate a dark background or 0;15 to indicate a light background.")),
            ("ShouldSetTerminfoDirs", "Terminal", Kind::Bool, Value::Boolean(true), terminfo, format!("{terminfo}\nTandem ships with extended terminfo capabilities for common TERMs (xterm, xterm-new, and xterm-256color). For example, it enables undercurl. New sessions get created with a TERMINFO_DIRS that make the customized TERMINFOs take precedence over the system defaults.")),
            ("PreventSleepOnBatteryToo", "General", Kind::Bool, Value::Boolean(false), battery, format!("{battery}\nBy default the \u{201c}Prevent sleep\u{201d} profile setting only prevents idle sleep while connected to power. Enable this to also prevent idle sleep on battery.")),
            ("IdleTimeSeconds", "General", Kind::Float, Value::Float(2.0), idle, format!("{idle}\nUsed for updating icons and activity indicator in tabs.")),
            ("JiggleTTYSizeOnClearBuffer", "General", Kind::Bool, Value::Boolean(false), jiggle, format!("{jiggle}\nWhen enabled, the TTY size is briefly changed after clearing the buffer to cause the shell or current app to redraw.")),
            ("FallbackLCCType", "General", Kind::String, Value::String(String::new()), lcctype, format!("{lcctype}\nIf unset, the encoding (e.g., UTF-8) will be used.")),
            ("FastForegroundJobUpdates", "Experimental Features", Kind::Bool, Value::Boolean(true), fast, fast.to_string()),
        ];
        let advanced = crate::advanced_pane::rows();
        for (key, section, kind, default, label, help) in rows {
            let row = schema::find_global(key).unwrap_or_else(|| panic!("{key} is not in the schema"));
            assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", section), "{key}");
            assert_eq!(row.kind, kind, "{key}");
            assert_eq!(row.default, Some(default), "{key}");
            assert_eq!((row.label.as_str(), row.help.as_str()), (label, help.as_str()), "{key}");
            // The row draws through the Advanced pane with the kind its type gives (P1).
            let drawn = advanced.iter().find(|r| r.key == key).unwrap_or_else(|| panic!("{key} is not drawn"));
            let want = match kind {
                Kind::Float => RowKind::Float,
                Kind::String => RowKind::Text,
                _ => RowKind::Bool,
            };
            assert_eq!(drawn.kind, want, "{key}");
        }
    }

    #[test]
    fn scripting_rows_follow_iterm2() {
        for (key, default, help) in [
            ("pythonRuntimeDownloadURL", "https://iterm2.com/downloads/pyenv/manifest.json", "URL to check for new versions of the Python scripting runtime."),
            (
                "uvManifestDownloadURL",
                "https://iterm2.com/downloads/uv/manifest.json",
                "URL of the uv download manifest (used when the Python runtime is provisioned with uv).",
            ),
            (
                "pythonRuntimeBetaDownloadURL",
                "https://iterm2.com/downloads/pyenv/betamanifest.json",
                "URL to check for new Beta versions of the Python scripting runtime.",
            ),
        ] {
            let row = schema::find_global(key).unwrap();
            assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", "Scripting"), "{key}");
            assert_eq!(row.kind, Kind::String, "{key}");
            assert_eq!(row.default, Some(Value::String(default.to_string())), "{key}");
            assert_eq!((row.label.as_str(), row.help.as_str()), (help, help), "{key}");
        }
        let uv_label = "Use uv to provision the Python runtime for API scripts?";
        let app_label = "Set IT2_APP_PATH environment variable on new sessions, pointing at this Tandem build\u{2019}s app bundle?";
        for (key, default, label, help) in [
            (
                "pythonRuntimeUsesUV",
                true,
                uv_label,
                format!("{uv_label}\nWhen enabled, Tandem uses uv to download Python and build per-script environments instead of the bundled Python runtime. Scripts already provisioned continue to run regardless of this setting; it only affects creating, importing, and migrating scripts."),
            ),
            (
                "setIT2AppPath",
                false,
                app_label,
                format!("{app_label}\nThis disambiguates the AppleScript target when multiple Tandem builds with the same bundle identifier are running, so that the \u{201c}it2\u{201d} CLI requests its cookie from this Tandem instance. Intended for Tandem developers. Only affects sessions created after changing this setting."),
            ),
        ] {
            let row = schema::find_global(key).unwrap();
            assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", "Scripting"), "{key}");
            assert_eq!(row.kind, Kind::Bool, "{key}");
            assert_eq!(row.default, Some(Value::Boolean(default)), "{key}");
            assert_eq!((row.label.as_str(), row.help.as_str()), (label, help.as_str()), "{key}");
        }
    }

    #[test]
    fn plumbed_rows_follow_iterm2() {
        let row = |key: &str| schema::find_global(key).unwrap_or_else(|| panic!("{key} missing"));
        let vars = row("NoSyncVariablesToReport");
        assert_eq!((vars.tab.as_str(), vars.section.as_str(), vars.kind), ("Advanced", "Terminal", Kind::String));
        assert_eq!(vars.default, Some(Value::String(String::new())));
        assert_eq!(vars.label, "Variables to report via control sequence");
        assert_eq!(
            vars.help,
            "Variables to report via control sequence\nThis is a comma-delimited list of variables that can be reported with the OSC 1337 ReportVariable=name control sequence. Each variable name must be prefixed with \u{201c}allow:\u{201d} or \u{201c}deny:\u{201d}."
        );
        let prevent = row("PreventEscapeSequenceFromChangingProfile");
        assert_eq!((prevent.tab.as_str(), prevent.section.as_str(), prevent.kind), ("Advanced", "Terminal", Kind::Bool));
        assert_eq!(prevent.default, None);
        let text = "Prevent control sequences from changing the current profile?";
        assert_eq!((prevent.label.as_str(), prevent.help.as_str()), (text, text));
        for (key, text) in [
            ("NoSyncSuppressClipboardAccessDeniedWarning", "Suppress the notification that the terminal attempted to access the clipboard but it was denied?"),
            ("NoSyncDisableOpenURL", "Disable control sequence to open URLs?"),
        ] {
            let r = row(key);
            assert_eq!((r.tab.as_str(), r.section.as_str(), r.kind), ("Advanced", "Warnings", Kind::Bool), "{key}");
            assert_eq!(r.default, Some(Value::Boolean(false)), "{key}");
            assert_eq!((r.label.as_str(), r.help.as_str()), (text, text), "{key}");
        }
        let base = row("PreferredBaseDir");
        assert_eq!((base.tab.as_str(), base.section.as_str(), base.kind), ("Advanced", "General", Kind::String));
        assert_eq!(base.default, Some(Value::String(String::new())));
        let label = "Folder for config files. There must not be a space in the path.";
        assert_eq!(base.label, label);
        assert_eq!(base.help, format!("{label}\nIf empty, then %APPDATA%\\Tandem will be the default location."));
    }

    /// `PP:1096-1121`, `PBVC:902-909`: "Show indicators for non-default values" is
    /// `NoSyncHideDefaultValuedSettings` (`PREF:252`, default NO `PREF:780`); a control is
    /// default when its value is nullably and approximately (0.001) equal to the default.
    #[test]
    fn indicators_follow_iterm2() {
        let s = schema::find_global("NoSyncHideDefaultValuedSettings").expect("NoSyncHideDefaultValuedSettings");
        assert_eq!((s.tab.as_str(), s.section.as_str(), s.kind), ("Stored", "Settings", Kind::Bool));
        assert_eq!(s.default, Some(Value::Boolean(false)));
        assert_eq!(s.label, crate::settings_ui::SHOW_INDICATORS);
        let mut g = Globals::empty(std::path::Path::new("unused-settings.toml"));
        let ffm = schema::find_global("FocusFollowsMouse").unwrap();
        assert!(global_is_default(&g, ffm));
        g.set("FocusFollowsMouse", Value::Boolean(true));
        assert!(!global_is_default(&g, ffm));
        g.set("FocusFollowsMouse", Value::Boolean(false));
        assert!(!g.values.contains_key("FocusFollowsMouse"), "the file drops a default");
        assert!(global_is_default(&g, ffm));
        // Numbers: within 0.001 of the default is default (`isNullablyEqualToObject:epsilon:0.001`).
        let height = schema::find_global("statusBarHeight").unwrap();
        g.values.insert("statusBarHeight".into(), Value::Float(21.0005));
        assert!(global_is_default(&g, height));
        g.values.insert("statusBarHeight".into(), Value::Integer(21));
        assert!(global_is_default(&g, height));
        g.values.insert("statusBarHeight".into(), Value::Float(21.01));
        assert!(!global_is_default(&g, height));
        // An optional bool has no default: a stored No or Yes is not default, unset is.
        let prevent = schema::find_global("PreventEscapeSequenceFromChangingProfile").unwrap();
        assert!(global_is_default(&g, prevent));
        g.values.insert(prevent.key.clone(), Value::Boolean(true));
        assert!(!global_is_default(&g, prevent));
        // A missing value equals a zero, empty or false one (`it_hasZeroValue`).
        assert!(nullably_equal(None, Some(&Value::Integer(0))));
        assert!(nullably_equal(Some(&Value::String(String::new())), None));
        assert!(!nullably_equal(Some(&Value::String("x".into())), None));
        let color = |r: f64| Value::Table(toml::toml! { r = r g = 0.5 b = 0.5 a = 1.0 });
        assert!(nullably_equal(Some(&color(0.2)), Some(&color(0.2009))));
        assert!(!nullably_equal(Some(&color(0.2)), Some(&color(0.21))));
        set_indicate_non_default(true);
        assert!(indicate_non_default());
        set_indicate_non_default(false);
        assert!(!indicate_non_default());
    }

    /// `AS:695`, `AS:670`, `iTermPreferences.m:219`: the text size rows (Task 8).
    #[test]
    fn size_rows_follow_iterm2() {
        let label = "Should growing or shrinking the font in a session that's broadcasting input affect all session that broadcast input?";
        let row = schema::find_global("FontChangeAffectsBroadcastingSessions").expect("FontChangeAffectsBroadcastingSessions");
        assert_eq!((row.tab.as_str(), row.section.as_str(), row.kind), ("Advanced", "Windows", Kind::Bool));
        assert_eq!(row.default, Some(Value::Boolean(false)));
        // The help is the whole DEFINE text (`AS:695`, with no closing period); the label its first line.
        assert_eq!((row.label.as_str(), row.help.as_str()), (label, format!("{label}\nThis only applies to changing the font size with Make Text Bigger, Make Text Normal Size, and Make Text Smaller").as_str()));
        let help = "Preserve font zoom level when Automatic Profile Switching changes profiles.";
        let row = schema::find_global("preserveFontSizeOnAutomaticProfileSwitch").expect("preserveFontSizeOnAutomaticProfileSwitch");
        assert_eq!((row.tab.as_str(), row.section.as_str(), row.kind), ("Advanced", "Session", Kind::Bool));
        assert_eq!(row.default, Some(Value::Boolean(true)));
        assert_eq!((row.label.as_str(), row.help.as_str()), (help, help));
        let row = schema::find_global("Size Changes Affect Profile").expect("Size Changes Affect Profile");
        assert_eq!((row.tab.as_str(), row.kind), (crate::advanced_pane::STORED_TAB, Kind::Bool));
        assert_eq!(row.default, Some(Value::Boolean(false)));
    }

    /// `XIB:8708-8750`, `GP:2559-2566`, `PREF:105-106`, `PREF:757-758`: General > Experimental.
    #[test]
    fn experimental_rows_follow_iterm2() {
        let bidi = schema::find_global("Bidi").expect("Bidi");
        assert_eq!((bidi.tab.as_str(), bidi.section.as_str(), bidi.kind), ("General", "Experimental", Kind::Bool));
        assert_eq!(bidi.default, Some(Value::Boolean(true)));
        assert_eq!(bidi.label, "Enable support for right-to-left scripts");
        assert_eq!(bidi.help, RTL_TOOLTIP);
        let ssh = schema::find_global("SshIntegrationForURLs").expect("SshIntegrationForURLs");
        assert_eq!((ssh.tab.as_str(), ssh.section.as_str(), ssh.kind), ("General", "Experimental", Kind::Bool));
        assert_eq!(ssh.default, Some(Value::Boolean(false)));
        assert_eq!(ssh.label, "Use SSH integration for ssh: URLs");
        assert_eq!(EXPERIMENTAL_NOTE, "This setting also enables ligature support in the GPU renderer.");
        assert_eq!(RTL_TOOLTIP, "Buried sessions appear under Session > Buried Sessions. When this setting is enabled, window sizes are restored when attaching to an existing tmux session.");
        // Experimental is the General pane's last section, as in iTerm2's xib.
        let last = schema::global().iter().filter(|s| s.tab == "General").last().unwrap();
        assert_eq!(last.section, "Experimental");
    }

    /// m14e Task 4: the Advanced row that turns the key mode restore on.
    #[test]
    fn restore_key_mode_row_follows_iterm2() {
        let row = schema::find_global("restoreKeyModeAutomaticallyOnHostChange").expect("restoreKeyModeAutomaticallyOnHostChange");
        assert_eq!((row.tab.as_str(), row.section.as_str(), row.kind), ("Advanced", "Experimental Features", Kind::Bool));
        assert_eq!(row.default, Some(Value::Boolean(true)));
        assert_eq!(row.label, "Automatically restore keyboard mode when an ssh session ends?");
        assert!(page_rows("Advanced").iter().any(|s| s.key == "restoreKeyModeAutomaticallyOnHostChange"));
    }

    /// `SL:562-640`, `AS:444`, `AS:746`, `AS:880`: the ssh: URL rows. `SshIntegrationForURLs` is
    /// General > Experimental (tested above, built by 13a); the other three are Advanced rows.
    #[test]
    fn ssh_url_rows_follow_iterm2() {
        let ssh = schema::find_global("SshIntegrationForURLs").expect("SshIntegrationForURLs");
        assert_eq!((ssh.tab.as_str(), ssh.section.as_str(), ssh.kind), ("General", "Experimental", Kind::Bool));
        assert_eq!(ssh.default, Some(Value::Boolean(false)));
        assert!(page_rows("General").iter().any(|s| s.key == "SshIntegrationForURLs" && s.section == "Experimental"));
        for (key, section, kind, default, label, help) in [
            ("sshSchemePath", "Terminal", Kind::String, Value::String("ssh".into()), "Command to run when handling an ssh:// URL.", None),
            (
                "validCharactersInSSHUserNames",
                "SSH Integration",
                Kind::String,
                Value::String("_-+.".into()),
                "Valid characters in SSH user names",
                Some("Valid characters in SSH user names\nAlphanumerics are always valid. This specifies other valid characters."),
            ),
            (
                "sshURLsSupportPath",
                "Experimental Features",
                Kind::Bool,
                Value::Boolean(true),
                "SSH URLs respect the path.",
                Some("SSH URLs respect the path.\nThey run the command: ssh -t \"cd $$PATH$$; exec \\$SHELL -l\""),
            ),
        ] {
            let row = schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!((row.tab.as_str(), row.section.as_str(), row.kind), ("Advanced", section, kind), "{key}");
            assert_eq!(row.default, Some(default), "{key}");
            assert_eq!(row.label, label, "{key}");
            if let Some(help) = help {
                assert_eq!(row.help, help, "{key}");
            }
            assert!(page_rows("Advanced").iter().any(|s| s.key == key), "{key}");
        }
        // Each key is declared once.
        for key in ["SshIntegrationForURLs", "sshSchemePath", "validCharactersInSSHUserNames", "sshURLsSupportPath"] {
            assert_eq!(schema::global().iter().filter(|s| s.key == key).count(), 1, "{key}");
        }
    }

    #[test]
    fn utilities_rows_follow_iterm2() {
        let lc = "Set LC_TERMINAL=iTerm2.";
        for (key, label, help) in [
            ("addUtilitiesToPATH", "Add path to Tandem utilities to $PATH for new sessions?", "Add path to Tandem utilities to $PATH for new sessions?".to_string()),
            ("shouldSetLCTerminal", lc, format!("{lc}\nopenssh and mosh pass this to hosts you connect to. It communicates the current terminal emulator. This is useful for enabling terminal emulator-specific features.")),
        ] {
            let row = schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", "Experimental Features"), "{key}");
            assert_eq!(row.kind, Kind::Bool, "{key}");
            assert_eq!(row.default, Some(Value::Boolean(true)), "{key}");
            assert_eq!((row.label.as_str(), row.help.as_str()), (label, help.as_str()), "{key}");
            assert!(Globals::empty(std::path::Path::new("utilities-row-test.toml")).bool(key), "{key} on until turned off");
        }
    }

    #[test]
    fn codecierge_rows_follow_iterm2() {
        let ghost = "You operate a terminal emulator for me. My goal is $GOAL. $CONTEXT. Ask clarifying questions as needed and run commands using the execute function when you're ready. When I've reached my goal, remind me to click the End Task button.";
        let regular = "You help a me in a terminal emulator. My goal is $GOAL. $CONTEXT. Start by suggesting a command. Don't overwhelm me with too much information: just go one step at a time. When I've reached my goal, remind me to click the End Task button.";
        let second = "$GOAL and $CONTEXT are replaced with the user-specified goal and info about the running environment, respectively.";
        for (key, kind, default, label, help) in [
            ("codeciergeGhostRidingPrompt", Kind::String, Value::String(ghost.into()), "Prompt to send to LLM for Codecierge when it is able to execute commands automatically.", Some(second)),
            ("codeciergeCommandWarningCount", Kind::Int, Value::Integer(10), "After this many commands, make codecierge confirm that you intend to keep it enabled.", None),
            ("codeciergeRegularPrompt", Kind::String, Value::String(regular.into()), "Prompt to send to LLM for Codecierge when it is NOT able to execute commands automatically.", Some(second)),
        ] {
            let row = schema::find_global(key).unwrap_or_else(|| panic!("{key} missing"));
            // P18: iTerm2's section is "Experimental Features" (SECTION_EXPERIMENTAL).
            assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", "Experimental Features"), "{key}");
            assert_eq!(row.kind, kind, "{key}");
            assert_eq!(row.default.as_ref(), Some(&default), "{key}");
            assert_eq!(row.label, label, "{key}");
            let want = help.map_or_else(|| label.to_string(), |h| format!("{label}\n{h}"));
            assert_eq!(row.help, want, "{key}");
        }
        let g = Globals::empty(std::path::Path::new("codecierge-rows-test.toml"));
        assert_eq!(g.int("codeciergeCommandWarningCount"), 10);
        assert_eq!(g.string("codeciergeGhostRidingPrompt"), ghost);
    }

    #[test]
    fn software_update_row_follows_iterm2() {
        let key = "NotifyOnlyForCriticalShellIntegrationUpdates";
        let row = schema::find_global(key).expect("row");
        assert_eq!((row.tab.as_str(), row.section.as_str(), row.kind), ("General", "Software Update", Kind::Bool));
        assert_eq!(row.default, Some(Value::Boolean(true)));
        assert_eq!(row.label, "Ignore non-critical shell integration updates");
        assert!(Globals::empty(std::path::Path::new("software-update-row-test.toml")).bool(key), "on until turned off");
    }

    #[test]
    fn upload_row_follows_iterm2() {
        let row = schema::find_global("accelerateUploads").unwrap();
        assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", "Experimental Features"));
        assert_eq!(row.kind, Kind::Bool);
        assert_eq!(row.default, Some(Value::Boolean(true)));
        assert_eq!(row.label, "Make uploads with it2ul really fast.");
        assert_eq!(row.help, "Make uploads with it2ul really fast.");
        assert!(Globals::empty(std::path::Path::new("upload-row-test.toml")).bool("accelerateUploads"), "on until turned off");
    }

    #[test]
    fn tab_appearance_rows_follow_iterm2() {
        for (key, default, inverted, label) in [
            ("HideTab", true, true, "Show tab bar even when there is only one tab"),
            ("HideTabNumber", false, true, "Show tab numbers"),
            ("TabsHaveCloseButton", true, false, "Tabs have close buttons"),
            ("HideActivityIndicator", false, true, "Show activity indicator"),
            ("StretchTabsToFillBar", true, false, "Stretch tabs to fill bar"),
            ("HTMLTabTitles", false, false, "Support basic HTML tags in tab titles"),
            ("ScrollableSideTabBar", true, false, "Tab bar scrolls when tabs don\u{2019}t fit"),
        ] {
            let row = schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!((row.tab.as_str(), row.section.as_str()), ("Appearance", "Tabs"), "{key}");
            assert_eq!(row.kind, Kind::Bool, "{key}");
            assert_eq!(row.default, Some(Value::Boolean(default)), "{key}");
            assert_eq!(row.inverted, inverted, "{key}");
            assert_eq!(row.label, label, "{key}");
        }
        // The one row with an iTerm2 tooltip (`PreferencePanel.xib`, "Support basic HTML tags") uses it verbatim.
        assert_eq!(
            schema::find_global("HTMLTabTitles").unwrap().help,
            "The following tags are recognized: <u>, <i>, and <b>. You can also use &lt; and &amp;."
        );
        // An inverted checkbox shows the opposite of the stored bool (`kPreferenceInfoTypeInvertedCheckbox`).
        assert!(checkbox_shows(true, false));
        assert!(!checkbox_shows(false, false));
        assert!(!checkbox_shows(true, true));
        assert!(checkbox_shows(false, true));
    }

    #[test]
    fn darkness_rows_follow_iterm2() {
        for (key, kind, default, label) in [
            ("LightModeInactiveTabDarkness", Kind::Float, Value::Float(0.07), "Darkness (in [0\u{2026}1]) for non-selected tabs in non-Minimal theme in light mode."),
            ("DarkModeInactiveTabDarkness", Kind::Float, Value::Float(0.5), "Darkness (in [0\u{2026}1]) for non-selected tabs in non-Minimal theme in dark mode."),
            ("HighVisibility", Kind::Bool, Value::Boolean(true), "High Contrast modes maximize visibility."),
        ] {
            let row = schema::find_global(key).unwrap();
            assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", "Tabs"), "{key}");
            assert_eq!(row.kind, kind, "{key}");
            assert_eq!(row.default, Some(default), "{key}");
            assert_eq!(row.label, label, "{key}");
        }
    }

    #[test]
    fn colored_rows_follow_iterm2() {
        for (key, label) in [
            ("ColoredSelectedTabOutlineStrength", "How prominent should the outline around the selected tab be drawn when there are colored tabs in a window?"),
            ("ColoredUnselectedTabTextProminence", "How prominent should the text in a non-selected tab be when there are colored tabs in a window?"),
        ] {
            let row = schema::find_global(key).unwrap();
            assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", "Tabs"), "{key}");
            assert_eq!(row.kind, Kind::Float, "{key}");
            assert_eq!(row.default, Some(Value::Float(0.5)), "{key}");
            assert_eq!(row.label, label, "{key}");
        }
    }

    #[test]
    fn divider_row_follows_iterm2() {
        let row = schema::find_global("MinimalSplitPaneDividerProminence").unwrap();
        assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", "Tabs"));
        assert_eq!(row.kind, Kind::Float);
        assert_eq!(row.default, Some(Value::Float(0.15)));
        assert_eq!(row.label, "In the Minimal theme, how prominent should split pane dividers be?");
    }

    #[test]
    fn compact_rows_follow_iterm2() {
        for (key, section, default, label) in [
            ("CompactMinimalTabBarHeight", "Tabs", 38.0, "Tab bar height (points) for the Minimal theme."),
            ("ExtraSpaceBeforeCompactTopTabBar", "Windows", 0.0, "Amount of extra space (in points) between stoplight buttons and inline tab bar."),
            ("CompactTabBarStoplightButtonsWidth", "Windows", 75.0, "Width (in points) reserved for the window buttons (close/minimize/zoom) before the inline tab bar."),
        ] {
            let row = schema::find_global(key).unwrap();
            assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", section), "{key}");
            assert_eq!(row.kind, Kind::Float, "{key}");
            assert_eq!(row.default, Some(Value::Float(default)), "{key}");
            assert_eq!(row.label, label, "{key}");
            assert!(section != "Windows" || row.help.contains("On Windows"), "{key}: the help adds the Windows note");
        }
    }

    #[test]
    fn window_name_row_follows_iterm2() {
        let row = schema::find_global("ShowWindowNameBesideTabs").unwrap();
        assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", "Windows"));
        assert_eq!(row.kind, Kind::Enum);
        assert_eq!(row.default, Some(Value::Integer(1)));
        assert_eq!(row.label, "Show the window name beside the tabs in the Compact and Minimal themes?");
        let choices: Vec<(Value, &str)> = row.choices.iter().map(|c| (c.value.clone(), c.label.as_str())).collect();
        assert_eq!(choices, [(Value::Integer(0), "Never"), (Value::Integer(1), "When a custom name is set"), (Value::Integer(2), "Always")]);
    }

    #[test]
    fn drag_region_rows_follow_iterm2() {
        for (key, label, default) in [
            ("MinimalEdgeDragSize", "In the Minimal theme, you can move the window by dragging starting in a region on the edge near the window border. This gives the size of that region.", Value::Float(12.0)),
            ("CompactEdgeDragSize", "In the Compact theme, you can move the window by dragging starting in a region on the edge near the window border. This gives the size of that region.", Value::Float(10.0)),
            ("ConvertTabDragToWindowDragForSolitaryTabInCompactOrMinimalTheme", "In the Minimal and Compact themes when there is a single tab and the tab bar is visible, should dragging the tab bar move the window?", Value::Boolean(true)),
        ] {
            let row = schema::find_global(key).unwrap();
            assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", "Tabs"), "{key}");
            assert_eq!(row.kind, if matches!(default, Value::Boolean(_)) { Kind::Bool } else { Kind::Float }, "{key}");
            assert_eq!(row.default, Some(default), "{key}");
            assert_eq!(row.label, label, "{key}");
        }
    }

    #[test]
    fn minimal_rows_follow_iterm2() {
        for (key, label, default) in [
            ("MinimalTabStyleBackgroundColorDifference", "In the Minimal theme, how different should the background color of the selected tab be from the others?", Value::Float(0.05)),
            ("MinimalTabStyleOutlineStrength", "In the Minimal theme, how prominent should the tab outline be?", Value::Float(0.2)),
            ("MinimalTextLegibilityAdjustment", "How much should contrast be increased for text in tabs in the Minimal theme?", Value::Float(1.0)),
            ("MinimalTabStyleTreatLeftInsetAsPartOfFirstTab", "In the Minimal theme, should the area left of the tab bar be treated as part of the first tab?", Value::Boolean(false)),
            ("MinimalSelectedTabUnderlineProminence", "Prominence of selected tab underline indicator in the Minimal theme when there is at least one colored tab.", Value::Float(1.0)),
            ("MinimalDeslectedColoredTabAlpha", "Alpha value for tab color for non-selected colored tabs in the Minimal theme.", Value::Float(0.5)),
        ] {
            let row = schema::find_global(key).unwrap();
            assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", "Tabs"), "{key}");
            assert_eq!(row.kind, if matches!(default, Value::Boolean(_)) { Kind::Bool } else { Kind::Float }, "{key}");
            assert_eq!(row.default, Some(default), "{key}");
            assert_eq!(row.label, label, "{key}");
        }
    }

    #[test]
    fn theme_row_follows_iterm2() {
        let row = schema::find_global("TabStyleWithAutomaticOption").unwrap();
        assert_eq!((row.tab.as_str(), row.section.as_str()), ("Appearance", "General"));
        assert_eq!(row.kind, Kind::Enum);
        assert_eq!(row.default, Some(Value::Integer(4)));
        assert_eq!(row.label, "Theme:");
        let choices: Vec<(Value, &str)> = row.choices.iter().map(|c| (c.value.clone(), c.label.as_str())).collect();
        assert_eq!(
            choices,
            [
                (Value::Integer(4), "Regular"),
                (Value::Integer(5), "Minimal"),
                (Value::Integer(6), "Compact"),
                (Value::Integer(0), "Light"),
                (Value::Integer(1), "Dark"),
                (Value::Integer(2), "Light (High Contrast)"),
                (Value::Integer(3), "Dark (High Contrast)"),
            ]
        );
        // The first row of Appearance > General, before the tab bar location and the status bar.
        let general: Vec<&str> = schema::global().iter().filter(|s| s.tab == "Appearance" && s.section == "General").map(|s| s.key.as_str()).collect();
        assert_eq!(general.first(), Some(&"TabStyleWithAutomaticOption"));
        let pos = |k: &str| general.iter().position(|g| *g == k).unwrap();
        assert!(pos("TabStyleWithAutomaticOption") < pos("TabViewType") && pos("TabViewType") < pos("StatusBarPosition"));
    }

    #[test]
    fn tab_position_rows_follow_iterm2() {
        let row = schema::find_global("TabViewType").unwrap();
        assert_eq!((row.tab.as_str(), row.section.as_str()), ("Appearance", "General"));
        assert_eq!(row.kind, Kind::Enum);
        assert_eq!(row.default, Some(Value::Integer(0)));
        assert_eq!(row.label, "Tab bar location:");
        let choices: Vec<(Value, &str)> = row.choices.iter().map(|c| (c.value.clone(), c.label.as_str())).collect();
        // iTerm2's "Right" carries a zero-width space (`PreferencePanel.xib`, tag 3).
        assert_eq!(choices, [(Value::Integer(0), "Top"), (Value::Integer(1), "Bottom"), (Value::Integer(2), "Left"), (Value::Integer(3), "Right\u{200b}")]);
        let row = schema::find_global("LeftTabBarWidth").unwrap();
        assert_eq!((row.tab.as_str(), row.section.as_str()), ("Appearance", "Tabs"));
        assert_eq!(row.kind, Kind::Float);
        assert_eq!(row.default, Some(Value::Float(150.0)));
        assert_eq!(row.label, "Left (side) tab bar width");
    }

    #[test]
    fn transparency_globals_follow_iterm2() {
        let s = schema::find_global("DisableFullscreenTransparency").unwrap();
        assert_eq!((s.tab.as_str(), s.section.as_str(), s.kind), ("Appearance", "Windows", Kind::Bool));
        assert_eq!(s.default, Some(Value::Boolean(false)));
        assert_eq!(s.label, "Disable transparency for non-native fullscreen windows by default");
        let s = schema::find_global("DisableTransparencyForKeyWindow").unwrap();
        assert_eq!((s.tab.as_str(), s.section.as_str(), s.kind), (crate::advanced_pane::STORED_TAB, "Transparency", Kind::Bool));
        assert_eq!(s.default, Some(Value::Boolean(false)));
    }

    #[test]
    fn full_screen_rows_follow_iterm2() {
        for (key, default, label) in [
            ("ShowFullScreenTabBar", true, "Show tab bar in fullscreen"),
            ("FlashTabBarInFullscreen", false, "Flash tab bar when switching tabs in fullscreen"),
        ] {
            let row = schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!((row.tab.as_str(), row.section.as_str()), ("Appearance", "Tabs"), "{key}");
            assert_eq!(row.kind, Kind::Bool, "{key}");
            assert_eq!(row.default, Some(Value::Boolean(default)), "{key}");
            assert!(!row.inverted, "{key}");
            assert_eq!(row.label, label, "{key}");
        }
        for (key, default, label, help) in [
            (
                "TabAutoShowHoldTime",
                1.0,
                "How long in seconds to show tabs in fullscreen.",
                "How long in seconds to show tabs in fullscreen.\nThe tab bar appears briefly in fullscreen when the number of tabs changes or you switch tabs. This setting gives the time in seconds for it to remain visible.",
            ),
            (
                "TabFlashAnimationDuration",
                0.25,
                "Animation duration for fade in/out animation of tabs in full screen, in seconds.",
                "Animation duration for fade in/out animation of tabs in full screen, in seconds.",
            ),
        ] {
            let row = schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", "Tabs"), "{key}");
            assert_eq!(row.kind, Kind::Float, "{key}");
            assert_eq!(row.default, Some(Value::Float(default)), "{key}");
            assert_eq!((row.label.as_str(), row.help.as_str()), (label, help), "{key}");
        }
        // The flash box is enabled when the bar can hide in full screen: Show off, or the bar
        // hidden with one tab (`APV:436-441`). The defaults have HideTab on.
        let mut g = Globals::empty(std::path::Path::new("full-screen-rows-test.toml"));
        assert!(row_enabled(&g, "FlashTabBarInFullscreen"));
        g.set("HideTab", Value::Boolean(false));
        assert!(!row_enabled(&g, "FlashTabBarInFullscreen"), "Show on and HideTab off");
        g.set("ShowFullScreenTabBar", Value::Boolean(false));
        assert!(row_enabled(&g, "FlashTabBarInFullscreen"), "Show off");
        assert!(row_enabled(&g, "ShowFullScreenTabBar"));
    }

    #[test]
    fn drag_rows_follow_iterm2() {
        for (key, label) in [
            ("AllowDragOfTabIntoNewWindow", "Allow a tab to be dragged and dropped outside any existing tab bar to create a new window."),
            ("AllowDragOnAddTabButton", "Allow dragging window by the Add Tab button in the tab bar?"),
        ] {
            let row = schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", "Tabs"), "{key}");
            assert_eq!(row.kind, Kind::Bool, "{key}");
            assert_eq!(row.default, Some(Value::Boolean(true)), "{key}");
            assert!(!row.inverted, "{key}");
            assert_eq!((row.label.as_str(), row.help.as_str()), (label, label), "{key}");
        }
        let row = schema::find_global("MinimumTabDragDistance").expect("MinimumTabDragDistance");
        assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", "Tabs"));
        assert_eq!(row.kind, Kind::Int);
        assert_eq!(row.default, Some(Value::Integer(10)));
        assert_eq!(row.label, "How far must the mouse move before a tab drag is initiated?");
        assert!(row.help.ends_with("You must restart Tandem after changing this setting for it to take effect."), "{}", row.help);
    }

    #[test]
    fn pinned_width_row_follows_iterm2() {
        let row = schema::find_global("PinnedTabWidth").expect("PinnedTabWidth");
        assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", "Tabs"));
        assert_eq!(row.kind, Kind::Int);
        assert_eq!(row.default, Some(Value::Integer(64)));
        assert_eq!((row.label.as_str(), row.help.as_str()), ("Width of pinned tabs in pixels.", "Width of pinned tabs in pixels."));
    }

    #[test]
    fn icon_row_follows_iterm2() {
        let row = schema::find_global("DefaultIconsUsingLetters").expect("DefaultIconsUsingLetters");
        assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", "Tabs"));
        assert_eq!(row.kind, Kind::Bool);
        assert_eq!(row.default, Some(Value::Boolean(true)));
        assert_eq!(row.label, "Use the running command's first letter as the tab's default icon if there isn't a built in one.");
        assert_eq!(row.help, "Use the running command's first letter as the tab's default icon if there isn't a built in one.\nThis takes effect when tabs are configured to use built-in icons.");
    }

    #[test]
    fn duplicate_row_follows_iterm2() {
        let row = schema::find_global("DuplicatedTabsOpenAdjacentToOriginal").expect("DuplicatedTabsOpenAdjacentToOriginal");
        assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", "Tabs"));
        assert_eq!(row.kind, Kind::Bool);
        assert_eq!(row.default, Some(Value::Boolean(true)));
        assert_eq!(row.label, "When you duplicate a tab, should the copy open immediately to the right of the original tab?");
        assert_eq!(row.help, "When you duplicate a tab, should the copy open immediately to the right of the original tab?\nWhen disabled, duplicated tabs follow the general \u{201c}New tabs are added\u{201d} placement setting instead.");
    }

    #[test]
    fn preserve_rows_follow_iterm2() {
        for (key, label) in [
            ("PreserveWindowSizeWhenTabBarVisibilityChanges", "Preserve window size when tab bar shows or hides"),
            ("PreserveWindowSizeToKeepOnScreenWhenTabBarVisibilityChanges", "\u{2026}but only if it would cause the window to go off-screen"),
        ] {
            let row = schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!((row.tab.as_str(), row.section.as_str()), ("Appearance", "Tabs"), "{key}");
            assert_eq!(row.kind, Kind::Bool, "{key}");
            assert_eq!(row.default, Some(Value::Boolean(false)), "{key}");
            assert!(!row.inverted, "{key}");
            assert_eq!(row.label, label, "{key}");
        }
        // The second box is enabled by the first (`APV:184-192`).
        let mut g = Globals::empty(std::path::Path::new("preserve-rows-test.toml"));
        assert!(!row_enabled(&g, "PreserveWindowSizeToKeepOnScreenWhenTabBarVisibilityChanges"));
        g.set("PreserveWindowSizeWhenTabBarVisibilityChanges", Value::Boolean(true));
        assert!(row_enabled(&g, "PreserveWindowSizeToKeepOnScreenWhenTabBarVisibilityChanges"));
        assert!(row_enabled(&g, "PreserveWindowSizeWhenTabBarVisibilityChanges"));
    }

    #[test]
    fn tab_advanced_rows_follow_iterm2() {
        let b = Value::Boolean;
        let i = Value::Integer;
        let f = Value::Float;
        let plain = |s: &str| (s.to_string(), s.to_string());
        let more = |label: &str, rest: &str| (label.to_string(), format!("{label}\n{rest}"));
        for (key, kind, default, (label, help)) in [
            ("UseUnevenTabs", Kind::Bool, b(false), plain("Uneven tab widths allowed.")),
            ("MinTabWidth", Kind::Int, i(75), plain("Minimum tab width when using uneven tab widths.")),
            ("MinCompactTabWidth", Kind::Int, i(60), plain("Minimum tab width when using uneven tab widths for compact tabs.")),
            ("OptimumTabWidth", Kind::Int, i(175), plain("Preferred tab width when tabs are equally sized.")),
            (
                "ScrollableTabWidth",
                Kind::Int,
                i(120),
                more("Tab width when the tab bar is scrollable.", "Used as the fixed width when tabs are equally sized, and as the minimum width when uneven tab widths are allowed."),
            ),
            (
                "TabTitlesUseSmartTruncation",
                Kind::Bool,
                b(true),
                more(
                    "Use \u{201c}smart truncation\u{201d} for tab titles.",
                    "If a tab\u{2018}s title is too long to fit, ellipsize the start of the title if more tabs have unique suffixes than prefixes in a given window.",
                ),
            ),
            ("MiddleClickClosesTab", Kind::Bool, b(true), plain("Should middle-click on a tab in the tab bar close the tab?")),
            (
                "DefaultTabBarHeight",
                Kind::Float,
                f(24.0),
                more(
                    "Default tab bar height",
                    "This does not affect the minimal or compact themes. On macOS 26 Tahoe-style tabs, the top (horizontal) tab bar uses the system height and ignores this setting; left/right (vertical) tabs use max(26, this value) for each tab\u{2019}s height.",
                ),
            ),
            ("DoubleClickTabToEdit", Kind::Bool, b(true), plain("Should double-clicking a tab open a window to edit its title?")),
            (
                "MinimumTabLabelWidth",
                Kind::Float,
                f(35.0),
                more("Minimum width for tab labels.", "The activity/bell icon will be hidden when the space for the label drops below this size (in points)"),
            ),
            ("DrawBottomLineForHorizontalTabBar", Kind::Bool, b(true), plain("Draw bottom line for horizontal tabbar in Regular, Dark and Light theme?")),
            ("DisableTabBarTooltips", Kind::Bool, b(false), plain("Disable tab bar tooltips?")),
            (
                "UseCustomTabBarFontSize",
                Kind::Bool,
                b(false),
                more("Use custom font size for tab labels?", "See also advanced setting \u{201c}Custom tab label font size\u{201d}."),
            ),
            (
                "CustomTabBarFontSize",
                Kind::Float,
                f(11.0),
                more("Custom tab label font size", "For this to take effect, turn on \u{201c}Use custom font size for tab labels?\u{201d}."),
            ),
            ("SelectsTabsOnMouseDown", Kind::Bool, b(true), more("Select tabs on mouse-down?", "Changing this setting will not affect existing windows.")),
            (
                "TabColorMenuOptions",
                Kind::String,
                Value::String("#fb6b62 #f6ac47 #f0dc4f #b5d749 #5fa3f8 #c18ed9 #787878".into()),
                more(
                    "Colors for tab color menu item.",
                    "Space delimited strings like #rrggbb or #rgb in sRGB color space. If the P3 color space is available, you can use strings like: color(p3 1 0.5 0.25)",
                ),
            ),
            ("RemoveAddTabButton", Kind::Bool, b(false), plain("Remove the \u{201c}new tab\u{201d} button from horizontal tab bars?")),
            (
                "AddTabButtonUsesCurrentProfile",
                Kind::Bool,
                b(false),
                more(
                    "Should the \u{201c}new tab\u{201d} button in the tab bar use the current session\u{2019}s profile?",
                    "When enabled, clicking the \u{201c}+\u{201d} button opens a new tab with the current session\u{2019}s profile instead of the default profile.",
                ),
            ),
            ("TabCloseButtonsAlwaysVisible", Kind::Bool, b(false), plain("Should tab close buttons always remain visible?")),
        ] {
            let row = schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", "Tabs"), "{key}");
            assert_eq!(row.kind, kind, "{key}");
            assert_eq!(row.default, Some(default), "{key}");
            assert!(!row.inverted, "{key}");
            assert_eq!((row.label.as_str(), row.help.as_str()), (label.as_str(), help.as_str()), "{key}");
        }
    }

    #[test]
    fn pointer_rows_show_labels() {
        let rows = pointer_rows(crate::globals::keymaps::factory_pointer());
        assert!(rows.contains(&("Button,1,1,,".to_string(), "Right button, single click".to_string(), "Open Context Menu".to_string())));
    }

    /// Settings > General > AI rows: key, kind, default, label, help after the label.
    fn ai_rows() -> Vec<(&'static str, Kind, Value, &'static str, &'static str)> {
        const GENERAL: &str = "Set by Settings > General > AI.";
        const MODEL: &str = "Set by the AI Model popup and the model editor.";
        const LEGACY: &str = "Legacy; copied once into each manual model.";
        let b = Value::Boolean;
        let i = Value::Integer;
        let s = |t: &str| Value::String(t.to_string());
        vec![
            (
                "EnableAI",
                Kind::Bool,
                b(false),
                "Enable generative AI features",
                "No data is sent to third parties without your explicit permission. You will always be prompted before data is shared.",
            ),
            (
                "AICompletions",
                Kind::Bool,
                b(false),
                "Offer AI command suggestion in Composer & Auto Composer",
                "When enabled, everything you type in the Composer will be shared with your AI provider.",
            ),
            ("AIVendor", Kind::Int, i(2), "AI vendor", MODEL),
            ("UseRecommendedAIModel", Kind::Bool, b(true), "Use the vendor's recommended model", MODEL),
            ("AiModel", Kind::String, s("gpt-4o-mini"), "AI model", MODEL),
            ("AiMaxTokens", Kind::Int, i(128000), "Context tokens", MODEL),
            ("AiResponseMaxTokens", Kind::Int, i(8192), "Max response tokens", MODEL),
            ("AI Timeout", Kind::Int, i(1200), "AI timeout", GENERAL),
            ("AitermURL", Kind::String, s(""), "AI endpoint URL", MODEL),
            ("AitermUseLegacyAPI", Kind::Bool, b(false), "Use the legacy API (deprecated)", GENERAL),
            ("AITermAPI", Kind::Int, i(2), "AI API", MODEL),
            ("AIFeatureHostedFileSearch", Kind::Bool, b(false), "Hosted file search", MODEL),
            ("AIFeatureHostedCodeInterpeter", Kind::Bool, b(false), "Hosted code interpreter", MODEL),
            ("AIFeatureHostedWebSearch", Kind::Bool, b(false), "Hosted web search", MODEL),
            ("AIFeatureFunctionCalling", Kind::Bool, b(false), "Function calling", MODEL),
            ("AIFeatureStreamingResponses", Kind::Bool, b(false), "Streaming responses", MODEL),
            ("AIVectorStore", Kind::Int, i(0), "Vector store", MODEL),
            ("AIManualModelConfigurations", Kind::List, Value::Array(Vec::new()), "Manual AI Models", GENERAL),
            ("AIEconomyModelName", Kind::String, s(""), "Economy model", GENERAL),
            ("AIOllamaRegularModel", Kind::String, s(""), "Regular:", GENERAL),
            ("AIOllamaEconomyModel", Kind::String, s(""), "Budget:", GENERAL),
            ("AI Safety Check", Kind::Bool, b(false), "Use AI to check safety of commands before running them", GENERAL),
            ("AICustomHeadersEnabled", Kind::Bool, b(false), "Custom headers (legacy)", LEGACY),
            ("AICustomHeaders", Kind::List, Value::Array(Vec::new()), "Custom header list (legacy)", LEGACY),
            ("AIZeroDataRetention", Kind::Bool, b(false), "Enable \u{201c}zero data retention\u{201d} (send whole conversation every time)", GENERAL),
            ("AIModelUpdatesEnabled", Kind::Bool, b(false), "Automatically update available AI models from live online database", GENERAL),
        ]
    }

    #[test]
    fn ai_rows_follow_iterm2() {
        for (key, kind, default, label, rest) in ai_rows() {
            let row = schema::find_global(key).unwrap_or_else(|| panic!("{key} missing"));
            assert_eq!((row.tab.as_str(), row.section.as_str()), ("General", "AI"), "{key}");
            assert_eq!(row.kind, kind, "{key}");
            assert_eq!(row.default.as_ref(), Some(&default), "{key}");
            assert_eq!(row.label, label, "{key}");
            assert_eq!(row.help, format!("{label}\n{rest}"), "{key}");
        }
        for (key, label) in crate::ai_settings::PERMISSIONS {
            let row = schema::find_global(key).unwrap_or_else(|| panic!("{key} missing"));
            assert_eq!((row.tab.as_str(), row.section.as_str()), ("General", "AI"), "{key}");
            assert_eq!(row.kind, Kind::Enum, "{key}");
            assert_eq!(row.default, Some(Value::Integer(0)), "{key}");
            assert_eq!(row.label, label, "{key}");
            assert_eq!(row.help, format!("{label}\nSet by Settings > General > AI."), "{key}");
            let allow = if key == "AIPermissionCheckTerminalState" { "Provide Automatically" } else { "Allow Without Asking" };
            let choices: Vec<(Value, &str)> = row.choices.iter().map(|c| (c.value.clone(), c.label.as_str())).collect();
            assert_eq!(choices, [(Value::Integer(0), "Ask Each Time"), (Value::Integer(1), allow), (Value::Integer(2), "Never Allow")], "{key}");
        }
        let g = Globals::empty(std::path::Path::new("ai-rows-test.toml"));
        assert_eq!(g.int("AIVendor"), 2);
        assert!(g.bool("UseRecommendedAIModel"));
        assert_eq!(g.string("AiModel"), "gpt-4o-mini");
        assert_eq!(g.int("AI Timeout"), 1200);
        assert_eq!(g.int("AITermAPI"), 2);
        assert!(!g.bool("EnableAI"));
    }

    #[test]
    fn advanced_ai_rows_follow_iterm2() {
        let s = |t: &str| Value::String(t.to_string());
        let rows: Vec<(&str, &str, Kind, Value, String)> = vec![
            ("llmPlatform", "General", Kind::String, s("OpenAI"), "LLM Platform.\nLegal values are: OpenAI, Azure, Gemini. This determines the format of requests and responses.".into()),
            (
                "aiModelCatalogURL",
                "General",
                Kind::String,
                s("https://iterm2.com/downloads/ai/manifest.json"),
                "URL to check for updates to the AI model catalog.\nThe catalog defines the built-in AI models (context window, capabilities, and the recommended model per vendor). Tandem ships its catalog with each release and never downloads a newer one, so this URL is not used.".into(),
            ),
            (
                "aiChatVerboseConsoleLogging",
                "Debugging",
                Kind::Bool,
                Value::Boolean(false),
                "Log AI chat traffic to the system console.\nEmits per-turn user / agent / tool entries so you can trace exactly what the agent received and produced. Useful for debugging tool dispatch and history translation. Off by default; turn on only while reproducing an issue.".into(),
            ),
            (
                "aiChatRawWireLogging",
                "Debugging",
                Kind::Bool,
                Value::Boolean(false),
                "Log raw AI API requests and responses to disk.\nWrites every byte sent to and received from the AI vendor (request headers, body, streaming chunks, final response, errors, and timing) to %APPDATA%\\Tandem\\AIChatWire\\. One log file per app launch; rotate or delete it yourself when done. Every header value except known non-secret ones (such as Content-Type, Accept, User-Agent and anthropic-version), and keys that appear in URLs or bodies, are replaced by fingerprints (the first 8 and last 4 characters); prompt and response content is logged verbatim. Off by default; turn on only while reproducing an issue and delete the files when finished.".into(),
            ),
            (
                "unameCommand",
                "SSH Integration",
                Kind::String,
                s("uname -s"),
                "Command used to set the `uname` variable.\nThe main use of this is to use in the AI prompt so the language model knows the operating system.".into(),
            ),
            (
                "aiModernModelPrefixes",
                "Experimental Features",
                Kind::String,
                s("gpt-"),
                "AI model name substrings that use the modern 'completions' API.\nSubstrings should be space-delimited. This handles OpenRouter-style names like 'openai/gpt-4o'. Note that o1 models use a different API and have hard-coded behavior.".into(),
            ),
            ("aiProxy", "Experimental Features", Kind::String, s(""), "Host and port for proxy for AI requests.\ni.e., example.com:8080".into()),
            (
                "ollamaNumCtx",
                "Experimental Features",
                Kind::Int,
                Value::Integer(0),
                "Ollama context window size (num_ctx) for native /api/chat requests.\nOllama defaults num_ctx to a small value (around 4096) regardless of the model, silently truncating long prompts. 0 means size it automatically per request, capped at the model\u{2019}s context window. A positive value overrides that and is sent verbatim (larger values use more memory).".into(),
            ),
            (
                "ollamaKeepAlive",
                "Experimental Features",
                Kind::String,
                s("30m"),
                "Ollama keep_alive for native /api/chat requests.\nHow long Ollama keeps the model resident after a request, e.g. 30m, 1h, or 0 to unload immediately. Empty means omit the field and use Ollama\u{2019}s own default.".into(),
            ),
        ];
        for (key, section, kind, default, help) in rows {
            let row = schema::find_global(key).unwrap_or_else(|| panic!("{key} missing"));
            assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", section), "{key}");
            assert_eq!(row.kind, kind, "{key}");
            assert_eq!(row.default.as_ref(), Some(&default), "{key}");
            assert_eq!(row.label, help.split('\n').next().unwrap(), "{key}");
            assert_eq!(row.help, help, "{key}");
        }
        // The Apple-only rows are left out of the schema (ruling 12e-4).
        assert!(schema::find_global("aiGeneratedTabTitles").is_none());
        assert!(schema::find_global("logAITabTitleCorpus").is_none());
        let admin = schema::find_global("GenerativeAIAllowed").unwrap();
        assert_eq!((admin.tab.as_str(), admin.section.as_str()), ("Advanced", "General"));
        assert_eq!(admin.kind, Kind::Bool);
        assert_eq!(admin.default, Some(Value::Boolean(true)));
        assert_eq!(admin.label, "Allow the use of large language model APIs?");
        assert_eq!(
            admin.help,
            "Allow the use of large language model APIs?\nThe purpose of this setting is to make it easy for managed environments to disable the use of LLMs. The user defaults key is `GenerativeAIAllowed`."
        );
    }

    #[test]
    fn page_rows_leave_ai_to_its_section() {
        let general = page_rows("General");
        assert!(general.iter().all(|s| s.section != "AI"));
        assert!(general.iter().any(|s| s.key == "EnableAPIServer"));
        assert!(general.iter().all(|s| s.tab == "General"));
        let advanced = page_rows("Advanced");
        let has = |key: &str| advanced.iter().any(|s| s.key == key);
        assert!(has("GenerativeAIAllowed"));
        assert!(has("unameCommand"));
        assert!(!has("aiGeneratedTabTitles"));
        assert!(!has("logAITabTitleCorpus"));
        assert!(row_hidden("aiGeneratedTabTitles"));
        assert!(row_hidden("logAITabTitleCorpus"));
        assert!(!row_hidden("aiProxy"));
        // Schema order is kept.
        let order: Vec<&str> = schema::global().iter().filter(|s| s.tab == "Advanced" && !row_hidden(&s.key)).map(|s| s.key.as_str()).collect();
        assert_eq!(advanced.iter().map(|s| s.key.as_str()).collect::<Vec<_>>(), order);
    }

    /// The settings warnings' specs (`PK:712-736`, `KP:704-719`, `PP:780-791`): texts, buttons,
    /// identifiers and kinds; none has a heading, so each shows iTerm2's "Warning".
    #[test]
    fn key_warnings_follow_iterm2() {
        use crate::warnings::{Kind::Permanent, checkbox_label};
        let meta = meta_spec();
        assert_eq!(meta.text, "You have chosen to have an option key act as Meta. This option is useful for backward compatibility with older systems. The \"Esc+\" option is recommended for most users.");
        assert_eq!((meta.labels.clone(), meta.cancel, meta.id.as_deref(), meta.kind), (vec!["OK".to_string()], None, Some("NeverWarnAboutMeta"), Permanent));
        assert_eq!(checkbox_label(&meta), Some("Suppress this message permanently"));
        let o = override_spec();
        assert_eq!(o.text, "The keyboard shortcut you have set for this profile will take precedence over an existing shortcut for the same key combination in a global shortcut.");
        assert_eq!((o.labels.clone(), o.cancel, o.id.as_deref(), o.kind), (vec!["OK".to_string(), "Cancel".into()], Some(1), Some("NeverWarnAboutOverrides"), Permanent));
        assert_eq!(checkbox_label(&o), Some("Suppress this message permanently"));
        let p = possible_override_spec();
        assert_eq!(p.text, "The global keyboard shortcut you have set is overridden by at least one profile. Check your profiles\u{2019} keyboard settings if it doesn't work as expected.");
        assert_eq!((p.labels.clone(), p.cancel, p.id.as_deref(), p.kind), (vec!["OK".to_string(), "Cancel".into()], Some(1), Some("NeverWarnAboutPossibleOverrides"), Permanent));
        let r = replace_profile_spec("Work");
        assert_eq!(r.text, "Replace profile \u{201c}Work\u{201d} with the current session's settings?");
        assert_eq!((r.labels.clone(), r.cancel, r.id.as_deref(), r.kind), (vec!["Replace".to_string(), "Cancel".into()], Some(1), Some("NoSyncReplaceProfileWarning"), Permanent));
        for spec in [&meta, &o, &p, &r] {
            assert_eq!(spec.shown_heading(), crate::warnings::DEFAULT_HEADING, "{:?}", spec.id);
            assert!(spec.never_remember.is_empty());
        }
    }

    /// `haveGlobalKeyMappingForKeystroke:` and `anyProfileHasMappingForKeystroke:`: a binding
    /// for the same key, in whatever form it is stored.
    #[test]
    fn override_detection_follows_iterm2() {
        let ctrl_a = Keystroke::new(0x61, keystroke::CONTROL, Some(0x0), 0x1);
        let ctrl_b = Keystroke::new(0x62, keystroke::CONTROL, Some(0xb), 0x2);
        let global: toml::Table = toml::from_str("\"0x61-0x40000-0x0\" = { Action = 13 }").unwrap();
        assert!(profile_overrides(&global, &ctrl_a));
        assert!(!profile_overrides(&global, &ctrl_b));
        // The legacy form without a keycode is the same key.
        let legacy: toml::Table = toml::from_str("\"0x61-0x40000\" = { Action = 13 }").unwrap();
        assert!(profile_overrides(&legacy, &ctrl_a));
        assert!(any_profile_maps(&[&toml::Table::new(), &global], &ctrl_a));
        assert!(!any_profile_maps(&[&global], &ctrl_b));
        assert!(!any_profile_maps(&[], &ctrl_a));
    }

    /// `preempt:` for a warning in the settings window: a remembered answer comes back at once,
    /// marked automatic (the app lists it in Suppressed Alerts); anything else draws the sheet.
    #[test]
    fn sheet_answers_are_recorded() {
        let mut g = Globals::empty(std::path::Path::new("sheet-test.toml"));
        let w = SheetWarning::new(override_spec(), SheetThen::SaveKey);
        assert_eq!(sheet_preempt(&g, &w, false, 1000.0), None);
        g.set("NeverWarnAboutOverrides", Value::Boolean(true));
        g.set("NeverWarnAboutOverrides_selection", Value::Integer(0));
        assert_eq!(sheet_preempt(&g, &w, false, 1000.0), Some(SheetAnswer { spec: override_spec(), index: 0, automatic: true }));
        // "Always show alerts with remembered selections": the sheet draws.
        assert_eq!(sheet_preempt(&g, &w, true, 1000.0), None);
        // A remembered Cancel is never replayed.
        g.set("NeverWarnAboutOverrides_selection", Value::Integer(1));
        assert_eq!(sheet_preempt(&g, &w, false, 1000.0), None);
    }

    /// The settings warnings' Advanced rows (`ASM:752-755,774`).
    #[test]
    fn key_warning_rows_follow_iterm2() {
        let meta = "Suppress a warning when Alt Key Acts as Meta is enabled in Settings > Profiles > Keys.";
        let rows = [
            ("NeverWarnAboutMeta", meta),
            ("NeverWarnAboutOverrides", "Suppress a warning about a change to a Profile key setting that overrides a global setting."),
            ("NeverWarnAboutPossibleOverrides", "Suppress a warning about a change to a global key that's overridden by a Profile."),
            ("NoSyncReplaceProfileWarning", "Suppress warning about copying a session's settings over a Profile"),
        ];
        for (key, text) in rows {
            let s = schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!((s.tab.as_str(), s.section.as_str(), s.kind), ("Advanced", "Warnings", Kind::Bool), "{key}");
            assert_eq!(s.default, Some(Value::Boolean(false)), "{key}");
            assert_eq!((s.label.as_str(), s.help.as_str()), (text, text), "{key}");
        }
    }

    /// The host change offers' rows are optional bools (`ASM:775-777`).
    #[test]
    fn host_rows_follow_iterm2() {
        let rows = [
            ("NoSyncTurnOffFocusReportingOnHostChange", "Always turn off focus reporting when host changes?"),
            ("NoSyncTurnOffMouseReportingOnHostChange", "Always turn off mouse reporting when host changes?"),
            ("NoSyncTurnOffBracketedPasteOnHostChange", "Always turn off paste bracketing when host changes?"),
        ];
        for (key, text) in rows {
            let s = schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!((s.tab.as_str(), s.section.as_str(), s.kind), ("Advanced", "Warnings", Kind::Bool), "{key}");
            assert_eq!(s.default, None, "{key}");
            assert!(s.optional_bool(), "{key}");
            assert_eq!((s.label.as_str(), s.help.as_str()), (text, text), "{key}");
        }
    }

    /// The frustration offers' rows (`ASM:782-783`), Cmd-C read as Win+C.
    #[test]
    fn frustration_rows_follow_iterm2() {
        let rows = [
            ("NoSyncNeverAskAboutMouseReportingFrustration", "Suppress the notification asking if you want to disable mouse reporting that is shown after a drag followed by Win+C when mouse reporting is on?"),
            ("NoSyncNeverAskAboutDEC2048Frustration", "Suppress the notification asking if you want to disable resize reporting"),
        ];
        for (key, text) in rows {
            let s = schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!((s.tab.as_str(), s.section.as_str(), s.kind), ("Advanced", "Warnings", Kind::Bool), "{key}");
            assert_eq!(s.default, Some(Value::Boolean(false)), "{key}");
            assert_eq!((s.label.as_str(), s.help.as_str()), (text, text), "{key}");
        }
    }

    /// The window title rows: General's check box (`GPV:2321`, `XIB:8249`) and the advanced
    /// setting (`AS:705`).
    #[test]
    fn title_rows_follow_iterm2() {
        let s = schema::find_global("SeparateWindowTitlePerTab").expect("SeparateWindowTitlePerTab");
        assert_eq!((s.tab.as_str(), s.section.as_str(), s.kind), ("General", "Window", Kind::Bool));
        assert_eq!(s.default, Some(Value::Boolean(true)));
        assert_eq!(s.label, "Each tab may have its own window title");
        let s = schema::find_global("ShowWindowTitleWhenTabBarInvisible").expect("ShowWindowTitleWhenTabBarInvisible");
        assert_eq!((s.tab.as_str(), s.section.as_str(), s.kind), ("Advanced", "Windows", Kind::Bool));
        assert_eq!(s.default, Some(Value::Boolean(true)));
        assert_eq!(s.label, "Show window title when the tab bar is not visible?");
        assert_eq!(s.help, "Show window title when the tab bar is not visible?\nWhen disabled, the tab's title will be shown where the window title would normally go.");
    }

    /// The window follows a font size change: General's check box (`XIB:8221-8224`, on by
    /// default, `P:733`).
    #[test]
    fn font_window_rows_follow_iterm2() {
        let s = schema::find_global("AdjustWindowForFontSizeChange").expect("AdjustWindowForFontSizeChange");
        assert_eq!((s.tab.as_str(), s.section.as_str(), s.kind), ("General", "Window", Kind::Bool));
        assert_eq!(s.default, Some(Value::Boolean(true)));
        assert_eq!(s.label, "Adjust window when changing font size");
        assert_eq!(s.help, "If enabled, changes to font size will try to preserve session dimensions.");
    }

    /// Zoom's vertical-only preference (`P:734`, `XIB:8213`).
    #[test]
    fn max_vertically_row_follows_iterm2() {
        let s = schema::find_global("MaxVertically").expect("MaxVertically");
        assert_eq!((s.tab.as_str(), s.section.as_str(), s.kind), ("General", "Window", Kind::Bool));
        assert_eq!(s.default, Some(Value::Boolean(false)));
        assert_eq!(s.label, "Zoom maximizes vertically only");
    }

    /// The window number's advanced settings (`AS:710-711`).
    #[test]
    fn number_rows_follow_iterm2() {
        for (key, label) in [("UseShortcutAccessoryViewController", "Show window number in titlebar accessory?"), ("IncludeShortcutInWindowsMenu", "Include keyboard shortcut for windows in Window menu?")] {
            let s = schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!((s.tab.as_str(), s.section.as_str(), s.kind), ("Advanced", "Windows", Kind::Bool), "{key}");
            assert_eq!(s.default, Some(Value::Boolean(true)), "{key}");
            assert_eq!((s.label.as_str(), s.help.as_str()), (label, label), "{key}");
        }
    }

    /// The border rows: Appearance's check box (`XIB:8935-8939`) and the advanced settings
    /// (`AS:707-709`, `:719`).
    #[test]
    fn border_rows_follow_iterm2() {
        let s = schema::find_global("UseBorder").expect("UseBorder");
        assert_eq!((s.tab.as_str(), s.section.as_str(), s.kind), ("Appearance", "Windows", Kind::Bool));
        assert_eq!(s.default, Some(Value::Boolean(false)));
        assert_eq!(s.label, "Heavy border around windows in light mode");
        let rows = [
            ("WindowBorderColor", Kind::String, Value::String(String::new()), "Border color for focused windows."),
            ("WindowBorderColorUnfocused", Kind::String, Value::String(String::new()), "Border color for unfocused windows."),
            ("BordersOnlyInLightMode", Kind::Bool, Value::Boolean(true), "Opaque windows have a border only in light mode."),
            ("SquareWindowCorners", Kind::Bool, Value::Boolean(false), "Windows have square corners."),
        ];
        for (key, kind, default, label) in rows {
            let s = schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!((s.tab.as_str(), s.section.as_str(), s.kind), ("Advanced", "Windows", kind), "{key}");
            assert_eq!(s.default, Some(default), "{key}");
            assert_eq!(s.label, label, "{key}");
            assert!(s.help.starts_with(&format!("{label}\n")), "{key}");
        }
        let s = schema::find_global("SquareWindowCorners").expect("SquareWindowCorners");
        assert!(s.help.ends_with("You must restart Tandem after changing this setting for it to take effect."));
    }

    /// The resize settings (`AS:692`, `:720`).
    #[test]
    fn resize_rows_follow_iterm2() {
        let rows = [
            ("DisableWindowSizeSnap", false, "Terminal windows resize smoothly.", "Terminal windows resize smoothly.\nDisables snapping to character grid. Holding Control will temporarily disable snap-to-grid."),
            ("AllowLiveResize", true, "Allow window resizing by dragging edges and corners?", "Allow window resizing by dragging edges and corners?"),
        ];
        for (key, default, label, help) in rows {
            let s = schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!((s.tab.as_str(), s.section.as_str(), s.kind), ("Advanced", "Windows", Kind::Bool), "{key}");
            assert_eq!(s.default, Some(Value::Boolean(default)), "{key}");
            assert_eq!((s.label.as_str(), s.help.as_str()), (label, help), "{key}");
        }
    }

    /// The full screen menu bar and the division line: Appearance's check boxes
    /// (`XIB:8785`, `:8986`), both on by default (`P:830`, `:843`).
    #[test]
    fn full_screen_chrome_rows_follow_iterm2() {
        let rows = [("HideMenuBarInFullscreen", "Auto-hide menu bar in non-native fullscreen"), ("EnableDivisionView", "Show line under title bar when the tab bar is not visible")];
        for (key, label) in rows {
            let s = schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!((s.tab.as_str(), s.section.as_str(), s.kind), ("Appearance", "Windows", Kind::Bool), "{key}");
            assert_eq!(s.default, Some(Value::Boolean(true)), "{key}");
            assert_eq!(s.label, label, "{key}");
        }
    }

    /// `AS:485`, `AS:920`: the display change Advanced rows.
    #[test]
    fn display_rows_follow_iterm2() {
        let d = schema::find_global("UpdateScreenParamsDelay").expect("UpdateScreenParamsDelay");
        assert_eq!((d.tab.as_str(), d.section.as_str(), d.kind), ("Advanced", "General", Kind::Float));
        assert_eq!(d.default, Some(Value::Float(1.0)));
        assert_eq!(d.label, "Delay after changing number of screens/resolution until refresh (seconds).");
        let c = schema::find_global("ChaseAnchoredScreen").expect("ChaseAnchoredScreen");
        assert_eq!((c.tab.as_str(), c.section.as_str(), c.kind), ("Advanced", "Experimental Features", Kind::Bool));
        assert_eq!(c.default, Some(Value::Boolean(false)));
        assert_eq!(c.label, "Aggressively keep windows on the screen they were configured to be on");
    }

    /// `APV:285-290`, `AS:520`: the presence rows.
    #[test]
    fn presence_rows_follow_iterm2() {
        let rows = [
            ("HideFromDockAndAppSwitcher", "Appearance", "Exclude from taskbar and Alt+Tab switcher"),
            ("UIElementRequiresHotkeys", "Appearance", "\u{2026}but only if all windows are hotkey windows"),
            ("HotkeyWindowsExcludedFromCycling", "Advanced", "Hotkey windows are excluded from Cycle Through Windows."),
        ];
        for (key, tab, label) in rows {
            let s = schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!((s.tab.as_str(), s.section.as_str(), s.kind), (tab, "General", Kind::Bool), "{key}");
            assert_eq!(s.default, Some(Value::Boolean(false)), "{key}");
            assert_eq!(s.label, label, "{key}");
        }
        let mut g = Globals::empty(std::path::Path::new("presence-rows-test.toml"));
        assert!(!row_enabled(&g, "UIElementRequiresHotkeys"));
        g.set("HideFromDockAndAppSwitcher", Value::Boolean(true));
        assert!(row_enabled(&g, "UIElementRequiresHotkeys"));
        assert!(row_enabled(&g, "HideFromDockAndAppSwitcher"));
    }

    /// `AS:508`, Mac words replaced.
    #[test]
    fn status_bar_icon_row_follows_iterm2() {
        let s = schema::find_global("StatusBarIcon").expect("StatusBarIcon");
        assert_eq!((s.tab.as_str(), s.section.as_str(), s.kind), ("Advanced", "General", Kind::Bool));
        assert_eq!(s.default, Some(Value::Boolean(true)));
        assert_eq!(s.label, "Add notification area icon when excluded from taskbar?");
        assert_eq!(s.help, "Add notification area icon when excluded from taskbar?\nWhen you turn on \u{201c}Exclude from taskbar and Alt+Tab switcher\u{201d} a notification area icon is added so you can switch the setting back off. Disable this to remove the icon. Doing so makes it very hard to get to Settings. You must restart Tandem after changing this setting.");
    }

    #[test]
    fn cursor_rows_follow_iterm2() {
        use crate::advanced_pane::{self, RowKind};
        let lead = |s: &str| s.split('\n').next().unwrap().to_string();
        let thr = |what: &str, side: &str| {
            format!("Threshold for Smart Cursor Color for {what} (0 to 1).\n0 means the cursor\u{2019}s {side} color will always be the cell\u{2019}s {}, while 1 means it will always be black or white.", if what == "background color" { "text color" } else { "background color" })
        };
        let rows: [(&str, &str, Kind, Value, String); 12] = [
            ("TimeBetweenBlinks", "Terminal", Kind::Float, Value::Float(0.5), "Cursor blink speed (seconds).".into()),
            ("SmartCursorColorBgThreshold", "Terminal", Kind::Float, Value::Float(0.5), thr("background color", "background")),
            ("SmartCursorColorFgThreshold", "Terminal", Kind::Float, Value::Float(0.75), thr("text color", "text")),
            ("FullHeightCursor", "Terminal", Kind::Bool, Value::Boolean(false), "Cursor occupies line spacing area.\nIf lines have more than 100% vertical spacing and this setting is enabled the bottom of the cursor will be aligned to the bottom of the spacing area.".into()),
            ("UnderlineCursorOffset", "Terminal", Kind::Float, Value::Float(0.0), "Vertical offset for underline cursor.\nPositive values move it up, negative values move it down.".into()),
            ("VerticalBarCursorWidth", "Terminal", Kind::Float, Value::Float(1.0), "Width of vertical bar cursor.".into()),
            ("DrawOutlineAroundCursor", "Drawing", Kind::Bool, Value::Boolean(false), "Draw outline around underline and vertical bar cursors using background color.".into()),
            ("UnderlineCursorHeight", "Drawing", Kind::Float, Value::Float(2.0), "Thickness of underline cursor.".into()),
            ("CursorAnimationMinDistance", "Drawing", Kind::Float, Value::Float(150.0), "When animated cursor movement is enabled, only perform animation if the cursor moves at least this distance in points.".into()),
            ("CursorSmearAnimationDuration", "Drawing", Kind::Float, Value::Float(0.1), "Duration in seconds for the \u{201c}Animate large movements\u{201d} cursor animation.".into()),
            ("CursorSlideAnimationDuration", "Drawing", Kind::Float, Value::Float(0.05), "Duration in seconds for smooth cursor slide animation.\nThis affects the \"Smooth slide\" cursor movement option.".into()),
            ("CursorSlideAnimationMaxCells", "Drawing", Kind::Int, Value::Integer(1), "Maximum horizontal distance in cells for smooth cursor slide animation.\nCursor movements larger than this will not animate.".into()),
        ];
        let pane = advanced_pane::rows();
        for (key, section, kind, default, help) in rows {
            let row = schema::find_global(key).unwrap_or_else(|| panic!("{key} missing"));
            assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", section), "{key}");
            assert_eq!(row.kind, kind, "{key}");
            assert_eq!(row.default, Some(default), "{key}");
            assert_eq!((row.label.clone(), row.help.clone()), (lead(&help), help.clone()), "{key}");
            // The Advanced pane draws the row with the matching editor (no global_control arm).
            let want = match kind {
                Kind::Bool => RowKind::Bool,
                Kind::Int => RowKind::Int,
                _ => RowKind::Float,
            };
            let drawn = pane.iter().find(|r| r.key == key).unwrap_or_else(|| panic!("{key} not in the pane"));
            assert_eq!((drawn.category, &drawn.kind), (section, &want), "{key}");
        }
    }

    /// 13l: the Advanced rows for frames and underlines (`AS:563-595`, `AS:479`, `AS:850`): tab, section, kind,
    /// default, label (the first line of the help) and help, iTerm2's text with "Tandem" for "iTerm2".
    #[test]
    fn drawing_rows_follow_iterm2() {
        use crate::advanced_pane::RowKind;
        use crate::schema::Kind;
        let t = Value::Boolean(true);
        let f = Value::Boolean(false);
        let rows: Vec<(&str, &str, Kind, Value, &str)> = vec![
            ("UseAdaptiveFrameRate", "Drawing", Kind::Bool, t.clone(), "Use adaptive framerate.\nWhen throughput is low, the screen will update at 60 frames per second. When throughput is higher, it will drop to a configurable rate (15 fps by default)."),
            ("DisableAdaptiveFrameRateInInteractiveApps", "Drawing", Kind::Bool, t.clone(), "Disable adaptive framerate in interactive apps.\nTurn off adaptive frame rate while in alternate screen mode for more consistent refresh rate. This works even if alternate screen mode is disabled."),
            ("MetalSlowFrameRate", "Drawing", Kind::Float, Value::Float(30.0), "When adaptive framerate is enabled and using the Metal renderer, refresh at this rate during high throughput conditions (FPS)."),
            ("ActiveUpdateCadence", "Drawing", Kind::Float, Value::Float(60.0), "Maximum frame rate (FPS) when adaptive framerate is disabled.\nNote: this is doubled on ARM Macs for displays that support at least 120hz. Modifications to this setting will not affect existing sessions."),
            ("AdaptiveFrameRateThroughputThreshold", "Drawing", Kind::Int, Value::Integer(10000), "Throughput threshold for adaptive frame rate.\nIf more than this many bytes per second are received, use the lower frame rate of 30 fps."),
            ("MaximumFrameRate", "Drawing", Kind::Float, Value::Float(60.0), "Frame rate (FPS) when adaptive framerate is enabled and throughput is low but not 0."),
            ("LowPowerModeFrameRate", "Drawing", Kind::Float, Value::Float(30.0), "Maximum frame rate (FPS) when low power mode is enabled."),
            ("UseLowPowerGPUWhenUnplugged", "Drawing", Kind::Bool, f.clone(), "Metal renderer uses integrated GPU when not connected to power?\nFor this to be effective you must disable \u{201c}Disable Metal renderer when not connected to power\u{201d}."),
            ("UnderlineHyperlinks", "Drawing", Kind::Bool, t.clone(), "Underline OSC 8 hyperlinks"),
            ("SolidUnderlines", "Drawing", Kind::Bool, f.clone(), "Use solid underlines?\nWhen disabled, underlines break near text that would intersect them."),
            ("UseMultiPassUnderlineRenderer", "Drawing", Kind::Bool, t.clone(), "Use multi-pass underline renderer?\nFixes underline gaps with ligature fonts. Requires restart."),
            ("ShowMetalFPSmeter", "Drawing", Kind::Bool, f.clone(), "Show FPS meter\nRequires Metal renderer"),
            ("MetalRedrawPeriod", "Drawing", Kind::Float, Value::Float(0.5), "GPU renderer redraws at least this often, in seconds.\nThis is to work around a problem where the GPU renderer encounters a lot of latency when drawing for the first time after a short period of inactivity. Set this to a big number to render it ineffectual."),
            ("DisableAppNap", "General", Kind::Bool, f.clone(), "Disable App Nap.\nChange effective after restarting Tandem."),
            ("ExtendBackgroundColorIntoMargins", "Experimental Features", Kind::Bool, t.clone(), "Extend background color into margins in alternate screen mode?"),
        ];
        assert_eq!(rows.len(), 15);
        let pane = crate::advanced_pane::rows();
        let g = Globals::empty(std::path::Path::new("drawing-rows-test.toml"));
        for (key, section, kind, default, help) in rows {
            let s = schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!((s.tab.as_str(), s.section.as_str()), ("Advanced", section), "{key}");
            assert_eq!(s.kind, kind, "{key}");
            assert_eq!(s.default, Some(default), "{key}");
            assert_eq!(s.help, help, "{key}");
            assert_eq!(s.label, help.split('\n').next().unwrap(), "{key}");
            let want = match kind {
                Kind::Bool => RowKind::Bool,
                Kind::Int => RowKind::Int,
                _ => RowKind::Float,
            };
            let drawn = pane.iter().find(|r| r.key == key).unwrap_or_else(|| panic!("{key} not in the pane"));
            assert_eq!((drawn.category, &drawn.kind), (section, &want), "{key}");
            // iTerm2 enables every Advanced row; none has a rule here.
            assert!(row_enabled(&g, key), "{key}");
        }
    }

    /// 13l: General > "GPU Renderer" (`GP:2060-2110`): Maximize throughput and the Advanced GPU sheet's rows.
    #[test]
    fn prefer_integrated_row_needs_two_gpus() {
        let g = Globals::empty(std::path::Path::new("gpu-rows-test.toml"));
        assert!(!row_enabled_with(&g, "preferIntegratedGPU", false));
        assert!(row_enabled_with(&g, "preferIntegratedGPU", true));
        assert!(row_enabled_with(&g, "metalMaximizeThroughput", false));
    }

    #[test]
    fn gpu_rows_follow_iterm2() {
        use crate::schema::Kind;
        let inert = "\nTandem always draws with the GPU; stored for settings you bring from iTerm2.";
        let rows = [
            ("metalMaximizeThroughput", "Maximize throughput at the cost of higher latency", String::new()),
            ("disableMetalWhenUnplugged", "Disable GPU renderer when disconnected from power", inert.to_string()),
            ("disableMetalInLowPowerMode", "Disable GPU renderer in low power mode", inert.to_string()),
            ("preferIntegratedGPU", "Prefer integrated to discrete GPU", String::new()),
        ];
        for (key, label, note) in rows {
            let s = schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!((s.tab.as_str(), s.section.as_str()), ("General", "GPU Renderer"), "{key}");
            assert_eq!(s.kind, Kind::Bool, "{key}");
            assert_eq!(s.default, Some(Value::Boolean(true)), "{key}");
            assert_eq!(s.label, label, "{key}");
            assert_eq!(s.help, format!("{label}{note}"), "{key}");
        }
        assert_eq!(GPU_RESTART, "You must restart Tandem for this change to take effect.");
    }

    /// 13l: Appearance > Panes margins (`P:112-113`, in points).
    #[test]
    fn margin_rows_follow_iterm2() {
        use crate::schema::Kind;
        let rows = [
            ("TerminalMargin", 5, "Side margins", "Space between the terminal text and the left and right edges of a pane, in points."),
            ("TerminalVMargin", 2, "Top & bottom margins", "Space between the terminal text and the top and bottom edges of a pane, in points."),
        ];
        for (key, default, label, help) in rows {
            let s = schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!((s.tab.as_str(), s.section.as_str()), ("Appearance", "Panes"), "{key}");
            assert_eq!(s.kind, Kind::Int, "{key}");
            assert_eq!(s.default, Some(Value::Integer(default)), "{key}");
            assert_eq!((s.label.as_str(), s.help.as_str()), (label, help), "{key}");
            assert!(s.min.is_none() && s.max.is_none(), "{key} is unbounded, as iTerm2's");
        }
    }

    /// 13l P5: an Int row draws a drag value for its stored integer.
    #[test]
    fn int_rows_draw_a_control() {
        let ctx = egui::Context::default();
        let mut g = Globals::empty(std::path::Path::new("int-row-test.toml"));
        let s = schema::find_global("TerminalMargin").unwrap();
        let mut drawn = 0.0;
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let before = ui.min_rect().height();
            global_control(ui, &mut g, s, true);
            drawn = ui.min_rect().height() - before;
        });
        out.textures_delta.clear();
        assert!(drawn > 0.0, "the Int row drew nothing");
        assert_eq!(g.int("TerminalMargin"), 5);
    }

    /// Final review 5: only the margin rows draw a drag value; an Int row such as IRMemory
    /// (bounded 0..=1000) is not given an unbounded one.
    #[test]
    fn other_int_rows_draw_no_unbounded_drag_value() {
        let ctx = egui::Context::default();
        let mut g = Globals::empty(std::path::Path::new("int-row-test.toml"));
        let s = schema::find_global("IRMemory").unwrap();
        assert_eq!(s.kind, Kind::Int);
        let mut drawn = 0.0;
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let before = ui.min_rect().height();
            global_control(ui, &mut g, s, true);
            drawn = ui.min_rect().height() - before;
        });
        out.textures_delta.clear();
        assert_eq!(drawn, 0.0, "IRMemory drew an Int control");
    }

    #[test]
    fn terminal_rows_follow_iterm2() {
        let focus = "Apps may turn on Focus Reporting.";
        let osc7 = "Accept OSC 7 to set username, hostname, and path. Tandem shell integration now reports this information using OSC 7, so turning this off also disables shell integration\u{2019}s username, hostname, and directory tracking.";
        let tab = "Default tab stop width for new sessions.";
        let xterm = "xterm version for secondary device attributes (SDA).";
        let color = "Report 16-bit color values to OSC 4 and 10 through 19.";
        let depth = "Maximum number of chunks to buffer.";
        let rows: [(&str, &str, Kind, Value, &str, String); 6] = [
            ("FocusReportingEnabled", "Terminal", Kind::Bool, Value::Boolean(true), focus, format!("{focus}\nFocus reporting causes Tandem to send an escape sequence when a session gains or loses focus. It can cause problems when an ssh session dies unexpectedly because it gets left on, so some users prefer to disable it.")),
            ("AcceptOSC7", "Terminal", Kind::Bool, Value::Boolean(true), osc7, osc7.to_string()),
            ("DefaultTabStopWidth", "Terminal", Kind::Int, Value::Integer(8), tab, format!("{tab}\nNote: this will break drawing in emacs and other apps.")),
            ("XtermVersion", "Terminal", Kind::Int, Value::Integer(2500), xterm, format!("{xterm}\nIncreasing this number enables more features in apps but may break things. Use 95 to recover pre-3.4.10 behavior.")),
            ("OscColorReport16Bits", "Experimental Features", Kind::Bool, Value::Boolean(true), color, format!("{color}\nWorks around a bug in older vim where they could not properly parse 8-bit values.")),
            ("BufferDepth", "Terminal", Kind::Int, Value::Integer(40), depth, format!("{depth}\nIn general, these chunks are 1024 bytes. A larger value increases buffer bloat but\u{2014}up to a limit\u{2014}can improve performance in the fast path of ASCII text.")),
        ];
        for (key, section, kind, default, label, help) in rows {
            let row = schema::find_global(key).unwrap_or_else(|| panic!("{key} missing"));
            assert_eq!((row.tab.as_str(), row.section.as_str(), row.kind), ("Advanced", section, kind), "{key}");
            assert_eq!(row.default, Some(default), "{key}");
            assert_eq!((row.label.as_str(), row.help.as_str()), (label, help.as_str()), "{key}");
        }
    }

    #[test]
    fn save_rows_follow_iterm2() {
        let clear = "Save scroll buffer when clearing screen.";
        let above = "Save the screen to scrollback when an app moves the cursor above the prompt.";
        let rows: [(&str, &str, String); 2] = [
            ("SaveScrollBufferWhenClearing", clear, format!("{clear}\nWhen enabled, saves the current screen into scroll back buffer instead of clearing it.")),
            ("SaveScrollbackWhenCursorMovesAbovePrompt", above, format!("{above}\nSome full-screen programs that don\u{2019}t use the alternate screen buffer repaint by moving the cursor to the top of the screen and overwriting whatever was already there, destroying content you can still see. When enabled, Tandem first scrolls the visible screen into history so that content isn\u{2019}t lost. Requires shell integration.")),
        ];
        for (key, label, help) in rows {
            let row = schema::find_global(key).unwrap_or_else(|| panic!("{key} missing"));
            assert_eq!((row.tab.as_str(), row.section.as_str(), row.kind), ("Advanced", "General", Kind::Bool), "{key}");
            assert_eq!(row.default, Some(Value::Boolean(true)), "{key}");
            assert_eq!((row.label.as_str(), row.help.as_str()), (label, help.as_str()), "{key}");
        }
    }

    /// 13o Task 7: Tabs wrap around (`AS:443`).
    #[test]
    fn tabs_wrap_row_follows_iterm2() {
        let label = "Tabs wrap around to the next line.";
        let row = schema::find_global("TabsWrapAround").expect("TabsWrapAround missing");
        assert_eq!((row.tab.as_str(), row.section.as_str(), row.kind), ("Advanced", "Terminal", Kind::Bool));
        assert_eq!(row.default, Some(Value::Boolean(false)));
        assert_eq!(row.label, label);
        assert_eq!(row.help, format!("{label}\nThis is useful for preserving tabs for later copying to the pasteboard. It breaks backward compatibility and may cause layout problems with programs that don\u{2019}t expect this behavior."));
    }

    #[test]
    fn stream_rows_follow_iterm2() {
        let url = "Maximum length for OSC 8 URLs";
        let rep = "Enable support for REP (Repeat previous character) escape sequence?";
        let sgr = "Reset colors at shell prompt?";
        let rows: [(&str, &str, Kind, Value, &str, String); 3] = [
            ("MaxURLLength", "Terminal", Kind::Int, Value::Integer(2_097_152), url, url.to_string()),
            ("SupportREPCode", "Experimental Features", Kind::Bool, Value::Boolean(true), rep, rep.to_string()),
            ("ResetSGROnPrompt", "Experimental Features", Kind::Bool, Value::Boolean(true), sgr, format!("{sgr}\nUses shell integration to detect a shell prompt and, if enabled, resets colors to their defaults.")),
        ];
        for (key, section, kind, default, label, help) in rows {
            let row = schema::find_global(key).unwrap_or_else(|| panic!("{key} missing"));
            assert_eq!((row.tab.as_str(), row.section.as_str(), row.kind), ("Advanced", section, kind), "{key}");
            assert_eq!(row.default, Some(default), "{key}");
            assert_eq!((row.label.as_str(), row.help.as_str()), (label, help.as_str()), "{key}");
        }
    }

    /// 13o Task 6: the DECRQCRA row (`AS:493`).
    #[test]
    fn decrqcra_row_follows_iterm2() {
        let row = schema::find_global("NoSyncDisableDECRQCRA").expect("row");
        assert_eq!((row.tab.as_str(), row.section.as_str(), row.kind), ("Advanced", "General", Kind::Bool));
        assert_eq!(row.default, Some(Value::Boolean(true)));
        assert_eq!(row.label, "Disable DECRQCRA?");
        assert_eq!(row.help, "Disable DECRQCRA?\nThis control sequence allows an app running in the terminal to read its contents.");
    }

    /// 13o Task 5: the character-width rows.
    #[test]
    fn width_rows_follow_iterm2() {
        let zw = "Zero-Width Space (U+200B) advances cursor?";
        let a = "Support variation selector 16 making emoji fullwidth in all modes?";
        let b = "Support variation selector 16 making emoji fullwidth outside of alternate screen mode?";
        let fw = "Lone regional indicators render full-width.";
        let ag = "Detect base unicode characters with lookup table.";
        let rows: [(&str, &str, bool, &str, String); 5] = [
            ("ZeroWidthSpaceAdvancesCursor", "Terminal", true, zw, format!("{zw}\nWhile a zero-width space should not advance the cursor per the Unicode spec, both Terminal.app and Konsole do this, and Weechat depends on it. You must restart Tandem after changing this setting.")),
            ("Vs16Supported", "Experimental Features", false, a, a.to_string()),
            ("Vs16SupportedInPrimaryScreen", "Experimental Features", true, b, b.to_string()),
            ("FullWidthFlags", "Experimental Features", true, fw, format!("{fw}\nA complete flag is always two columns, which is the width wcwidth reports for the pair.")),
            ("AggressiveBaseCharacterDetection", "Experimental Features", true, ag, format!("{ag}\nThe system\u{2019}s algorithm for segmenting composed characters makes bad choices, such as for Tamil. Enable this to reduce text overlapping.")),
        ];
        for (key, section, default, label, help) in rows {
            let row = schema::find_global(key).unwrap_or_else(|| panic!("{key} missing"));
            assert_eq!((row.tab.as_str(), row.section.as_str(), row.kind), ("Advanced", section, Kind::Bool), "{key}");
            assert_eq!(row.default, Some(Value::Boolean(default)), "{key}");
            assert_eq!((row.label.as_str(), row.help.as_str()), (label, help.as_str()), "{key}");
        }
        let uv = schema::find("Unicode Version").unwrap();
        let choices: Vec<(Value, &str)> = uv.choices.iter().map(|c| (c.value.clone(), c.label.as_str())).collect();
        assert_eq!(choices, [(Value::Integer(8), "Unicode 8"), (Value::Integer(9), "Unicode 9+")]);
        assert_eq!(uv.default, Some(Value::Integer(9)));
        // The rules read them under iTerm2's names.
        let mut g = Globals::empty(std::path::Path::new("width-rows-test.toml"));
        let d = crate::emu::Rules::from_globals(&g);
        assert!(d.zwsp_advances && !d.vs16_all && d.vs16_primary && d.full_width_flags);
        g.set("Vs16Supported", Value::Boolean(true));
        g.set("FullWidthFlags", Value::Boolean(false));
        let r = crate::emu::Rules::from_globals(&g);
        assert!(r.vs16_all && !r.full_width_flags);
    }

    /// 13o Task 3: the CSI 3 J row.
    #[test]
    fn clear_history_row_follows_iterm2() {
        let label = "Prevent CSI 3 J from clearing scrollback history?";
        let row = schema::find_global("PreventEscapeSequenceFromClearingHistory").expect("row missing");
        assert_eq!((row.tab.as_str(), row.section.as_str(), row.kind), ("Advanced", "Terminal", Kind::Bool));
        assert_eq!(row.default, None);
        assert_eq!(row.label, label);
        assert_eq!(row.help, format!("{label}\nThis is also known as the terminfo E3 capability."));
    }
    #[test]
    fn semantic_rows_follow_iterm2() {
        use crate::advanced_pane::{rows, RowKind};
        enum D {
            B(bool),
            S(&'static str),
            I(i64),
        }
        // (key, section, default, help); the label is the help's first line.
        let table: Vec<(&str, &str, D, String)> = vec![
            ("CmdClickWhenInactiveInvokesSemanticHistory", "Mouse", D::B(false), "Win-click in an active pane while Tandem isn't the active app invokes Semantic History.\nBy default, Tandem respects the OS standard that Win-click in an app that doesn't have keyboard focus behaves like a non-Win click that does not raise the window.".into()),
            ("EnableUnderlineSemanticHistoryOnCmdHover", "Mouse", D::B(true), "Underline Semantic History-selectable items under the cursor while holding Win?".into()),
            ("EnableCmdClickPromptForShowCommandInfo", "Mouse", D::B(true), "Win-click in the prompt shows the Command Info window".into()),
            ("ShowButtonsForSelectedCommand", "General", D::B(true), "Show buttons at the top of a selected command?\nWhen you select a command by single clicking the command line or its output, buttons are shown on the top right with options like Copy and Share. Note that this setting only applies when marks are not \u{201c}line style\u{201d} such as when using Auto Composer.".into()),
            ("ShowURLPreviewForSemanticHistory", "Drawing", D::B(true), "Show URL preview on Win-hover for semantic history matches.".into()),
            ("IgnoreHardNewlinesInURLs", "Semantic History", D::B(false), "Ignore hard newlines for the purposes of locating URLs and file names for Semantic History.\nIf a hard newline occurs at the end of a line then Win-click will not see it all unless this setting is turned on. This is useful for some interactive applications. Turning this on will remove newlines from the \\3 and \\4 substitutions.".into()),
            ("URLCharacterSet", "Semantic History", D::S(".?\\/:;$%=&_-,+~#@!*'(\u{ff08})\u{ff09}|[]"), "Non-alphanumeric characters considered part of a URL or file name for Semantic History.\nLetters and numbers are always considered part of the URL. These non-alphanumeric characters are used in addition for the purposes of figuring out where a URL begins and ends. You must restart Tandem for changes to this setting to take effect.".into()),
            ("URLCharacterSetExclusions", "Semantic History", D::S("\u{ac}"), "Characters never considered part of a URL.\nThe characters in this string may never occur in a URL; when one is seen that delineates the beginning or end of a URL.".into()),
            ("FilenameCharacterSet", "Semantic History", D::S("$@"), "Non-alphanumeric characters considered part of a filename. Note that URL characters (a separate advanced setting) are also allowed.\nLetters and numbers are always considered part of the URL. These non-alphanumeric characters are used in addition for the purposes of figuring out where a file begins and ends. You must restart Tandem for changes to this setting to take effect.".into()),
            ("MaxSemanticHistoryPrefixOrSuffix", "Semantic History", D::I(2000), "Maximum number of bytes of text before and after click location to take into account.\nThis also limits the size of the \\3 and \\4 substitutions.".into()),
            ("PathsToIgnore", "Semantic History", D::S(""), "Paths to ignore for Semantic History.\nSeparate paths with a comma. Any file under one of these paths will not be openable with Semantic History. It is wise to add network file systems to this list, since they can be very slow.".into()),
            ("ShowYellowMarkForJobStoppedBySignal", "Semantic History", D::B(true), "Use a yellow for a Shell Integration prompt mark when the job is stopped by a signal.".into()),
            ("ConservativeURLGuessing", "Semantic History", D::B(false), "URLs must contain a scheme?\nEnable this to reduce the number of false positives that semantic history thinks are a URL".into()),
            ("TrailingPunctuationMarks", "Semantic History", D::S("!?\u{2026})].\"';:,"), "Characters to ignore at the end of a URL".into()),
            ("DefaultURLScheme", "Semantic History", D::S("https"), "Default URL scheme.\nThis is applied to hostnames that are not a single word.".into()),
            ("UrlHandlerCommand", "Semantic History", D::S(""), "Command to run when opening a clicked URL, instead of asking the OS to open it.\nWhen non-empty, this command runs as a silent background task whenever you click a URL (including OSC 8 hyperlinks and auto-detected URLs) that Tandem would otherwise hand to the system. Tandem splits it into a program and its arguments and starts the program directly, without a shell. This is an interpolated string; use \\(url) for the URL, which is always passed as one argument of its own (added at the end when \\(url) is absent), and \\(background) for whether a background open was requested (1 if you held Alt while clicking, otherwise 0). Any other variable is passed as one argument of its own (or part of one) and can never name the program; a variable holding a quote, a control character, a trailing backslash or one of & | < > ^ % ! ( ) ` is refused. The command is responsible for opening the URL however it likes (e.g., \u{201c}explorer.exe \\(url)\u{201d} to open it with the default browser). cmd.exe, conhost, forfiles, wt, sh, zsh, rundll32, regsvr32, msiexec and certutil cannot be URL handlers (they hand their arguments to another program); a .cmd or .bat handler, or an interpreter such as PowerShell, is not given URLs containing % ^ & | < > \" or control characters. When empty, URLs open normally. Tandem-internal schemes (iterm2:), URL schemes bound to a profile, and Semantic History file links are not affected.".into()),
            ("RestrictSemanticHistoryPrefixAndSuffixToLogicalWindow", "Semantic History", D::B(true), "Respect soft boundaries for computing the prefix and suffix text passed to semantic history commands?\nDue to a long-standing bug this did not used to be respected. Soft boundaries were always respected for deciding what text was clicked on, but the prefix and suffix did not. If you have a semantic history command that depends on the bug, you can switch this off to get the pre-3.1.2 behavior.".into()),
            ("EnableSemanticHistoryOnNetworkMounts", "Semantic History", D::B(false), "Enable semantic history for network-mounted filesystems?\nOnly enable this if you know your network-mounted filesystems are really fast and reliable. A slow filesystem may cause the entire Tandem app to hang.".into()),
            ("PrioritizeSmartSelectionActions", "Semantic History", D::B(false), "Check smart selection actions before existing files on Win-click?".into()),
            ("DisableSmartSelectionActionsOnClick", "Semantic History", D::B(false), "Disable performing the first matching Smart Selection\u{2019}s Action on Win-click?".into()),
        ];
        assert_eq!(table.len(), 20);
        let advanced = rows();
        for (key, section, default, help) in &table {
            let row = schema::find_global(key).unwrap_or_else(|| panic!("{key} has no row"));
            assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", *section), "{key}");
            let label = help.lines().next().unwrap();
            assert_eq!((row.label.as_str(), row.help.as_str()), (label, help.as_str()), "{key}");
            let (kind, value, rk) = match default {
                D::B(b) => (Kind::Bool, Value::Boolean(*b), RowKind::Bool),
                D::S(s) => (Kind::String, Value::String(s.to_string()), RowKind::Text),
                D::I(n) => (Kind::Int, Value::Integer(*n), RowKind::Int),
            };
            assert_eq!(row.kind, kind, "{key}");
            assert_eq!(row.default, Some(value), "{key}");
            let shown = advanced.iter().find(|r| r.key == *key).unwrap_or_else(|| panic!("{key} is not in the Advanced pane"));
            assert_eq!(shown.kind, rk, "{key}");
        }
        let nav = schema::find("Prompt Path Click Opens Navigator").unwrap();
        assert_eq!(nav.label, "Click on a path in a shell prompt to open Navigator");
        assert_eq!(nav.help, "Whether clicking path in prompt opens file navigator");
    }

    /// 13q Task 1 (`AS:661-682`, `AS:733`, `AS:769`, `AS:853-896`): the session-end Advanced
    /// rows with iTerm2's sections, kinds, defaults and texts; the label is the help's first line.
    #[test]
    fn session_rows_follow_iterm2() {
        let auto_log = "\\(creationTimeString).\\(profileName).\\(termid).\\(iterm2.pid).\\(autoLogId).log";
        let divider = "Character to use for divider lines in session end messages. Leave empty to use secure default (BrokenPipeDivider image). Examples: \u{2501} \u{2500} \u{2550} \u{254c} \u{2504}";
        let tmux = "Close tmux windows after detaching?\nThis only takes effect when \u{201c}Settings > Profiles > Session > After a session ends\u{201d} is set to \u{201c}No Action\u{201d}.";
        let b = |v: bool| Some(Value::Boolean(v));
        let t = |v: &str| Some(Value::String(v.to_string()));
        let rows: Vec<(&str, &str, Kind, Option<Value>, String)> = vec![
            ("SuppressRestartAnnouncement", "Session", Kind::Bool, b(false), "Suppress the Restart Session offer.\nWhen a session terminates, it will offer to restart itself. Turn this on to suppress the offer permanently.".into()),
            ("AutoLogFormat", "Session", Kind::String, t(auto_log), "Format for automatic session log filenames.\nSee the Badges documentation for supported substitutions.".into()),
            ("AutologAppends", "Session", Kind::Bool, b(true), "Automatic session logging appends to existing files.\nWhen set to No, the file will be overwritten instead.".into()),
            ("LogTimestampFormat", "Session", Kind::String, t("yyyy-MM-dd hh.mm.ss.SSS"), "Format for log timestamps. See Unicode TR 35-31 for syntax.\nYou must restart Tandem for changes to this setting to take effect.".into()),
            ("LogTimestampsWithPlainText", "Session", Kind::Bool, b(true), "When logging plain text, include timestamps for each line?".into()),
            ("sessionEndMessageText", "Session", Kind::String, t(""), "Text displayed when a session ends. Leave empty to use default.".into()),
            ("SessionEndMessageDividerCharacter", "Session", Kind::String, t(""), divider.into()),
            ("SessionRestartedMessageText", "Session", Kind::String, t("Session Restarted"), "Text displayed when a session is restarted.".into()),
            ("SessionFinishedMessageText", "Session", Kind::String, t("Finished"), "Text displayed when a short-lived session finishes.".into()),
            ("TmuxWindowsShouldCloseAfterDetach", "Tmux Integration", Kind::Bool, None, tmux.into()),
            ("ShortLivedSessionDuration", "Warnings", Kind::Float, Some(Value::Float(3.0)), "Warn about short-lived sessions that live less than this many seconds.".into()),
            ("KillSessionsOnLogout", "Experimental Features", Kind::Bool, b(false), "Kill sessions on logout.\nA possible fix for issue 4147.".into()),
            ("Multiserver", "Experimental Features", Kind::Bool, b(true), "Enable multi-server daemon.\nA new implementation of session restoration that combines daemon processes.".into()),
            ("UseNewContentFormat", "Experimental Features", Kind::Bool, b(true), "Save unlimited amount of window contents.\nThis is going to be slow unless you enable SQLite-based window restoration too.".into()),
        ];
        for (key, section, kind, default, help) in rows {
            let row = schema::find_global(key).unwrap_or_else(|| panic!("{key} missing"));
            assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", section), "{key}");
            assert_eq!(row.kind, kind, "{key}");
            assert_eq!(row.default, default, "{key}");
            assert_eq!(row.label, help.lines().next().unwrap(), "{key}");
            assert_eq!(row.help, help, "{key}");
        }
        assert!(schema::find_global("TmuxWindowsShouldCloseAfterDetach").unwrap().optional_bool());
        for key in ["SuppressRestartAnnouncement", "KillSessionsOnLogout"] {
            assert!(!schema::find_global(key).unwrap().optional_bool(), "{key}");
        }
        // The text moved to Session once (13a); nothing of it is left in Notifications.
        assert_eq!(schema::global().iter().filter(|s| s.key == "sessionEndMessageText").count(), 1);
        assert_ne!(schema::find_global("sessionEndMessageText").unwrap().section, "Notifications");
        let smart = schema::find_global("smartLoggingWithAutoComposer").unwrap();
        assert_eq!((smart.tab.as_str(), smart.kind, smart.default.clone()), ("Advanced", Kind::Bool, b(false)));
    }
}

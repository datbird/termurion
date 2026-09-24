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

/// The Delete sends ^H checkbox's binding: Backspace (0x7f, kVK_Delete) to Send ^H Backspace.
const BACKSPACE_KEY: &str = "0x7f-0x0-0x33";

fn later_note(status: Status) -> String {
    match status {
        Status::Later(m) => format!(" (milestone {m})"),
        _ => String::new(),
    }
}

/// One line for an action in a table: its name, and its parameter when it has one.
pub fn action_summary(b: &Binding) -> String {
    let name = keyaction::info(b.action).map_or_else(|| format!("Action {}", b.action), |a| a.name.clone());
    let arg = keyaction::info(b.action).map_or(Arg::None, |a| a.arg);
    match arg {
        Arg::None => name,
        Arg::Sequence => format!("{name} ({} steps)", keyaction::sequence(&b.text).len()),
        _ if b.text.is_empty() => name,
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

/// The binding editors' state, kept by the settings window.
#[derive(Default)]
pub struct KeysState {
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
}

impl KeysState {
    /// A key press should be recorded, not typed.
    pub fn recording(&self) -> bool {
        self.leader_recording || self.key_editor.as_ref().is_some_and(|e| e.recording)
    }

    pub fn deliver(&mut self, k: Keystroke) {
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

fn action_combo(ui: &mut egui::Ui, salt: &str, action: &mut i64, allow_sequence: bool) -> bool {
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

/// Boolean settings Toggle Setting can flip: every global and profile checkbox.
fn toggle_choices() -> Vec<(String, bool, String)> {
    let bools = |list: &'static [Setting], is_profile: bool| {
        list.iter().filter(|s| s.kind == Kind::Bool && !s.macos_only).map(move |s| (s.key.clone(), is_profile, s.label.clone()))
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
fn param_editor(ui: &mut egui::Ui, salt: &str, b: &mut Binding, profiles: &[(String, String)]) -> bool {
    let arg = keyaction::info(b.action).map_or(Arg::None, |a| a.arg);
    let mut changed = false;
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
        Arg::VimText | Arg::Other => changed |= ui.text_edit_singleline(&mut b.text).changed(),
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
            }
        });
        param_editor(ui, "key-param", &mut e.binding, profiles);
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

/// A control for one global setting (checkbox, menu, or text).
pub fn global_control(ui: &mut egui::Ui, g: &mut Globals, s: &Setting, enabled: bool) -> bool {
    let mut changed = false;
    ui.add_enabled_ui(enabled && !s.macos_only, |ui| {
        match s.kind {
            Kind::Bool => {
                let mut b = g.bool(&s.key);
                if ui.checkbox(&mut b, &s.label).changed() {
                    g.set(&s.key, Value::Boolean(b));
                    changed = true;
                }
            }
            Kind::Enum => {
                ui.horizontal(|ui| {
                    ui.label(&s.label);
                    let now = g.get(&s.key);
                    let shown = s.choices.iter().find(|c| Some(&c.value) == now.as_ref()).map_or(String::new(), |c| c.label.clone());
                    egui::ComboBox::from_id_salt(&s.key).selected_text(shown).show_ui(ui, |ui| {
                        for c in &s.choices {
                            if ui.selectable_label(Some(&c.value) == now.as_ref(), &c.label).clicked() {
                                g.set(&s.key, c.value.clone());
                                changed = true;
                            }
                        }
                    });
                });
            }
            _ => {}
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
pub fn keys_page(ui: &mut egui::Ui, g: &mut Globals, st: &mut KeysState, profiles: &[(String, String)]) -> bool {
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
            if ui.button(text).clicked() {
                st.leader_recording = !st.leader_recording;
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
        if finish_key_editor(ui.ctx(), st, &mut map, profiles) {
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
fn finish_key_editor(ctx: &egui::Context, st: &mut KeysState, map: &mut toml::Table, profiles: &[(String, String)]) -> bool {
    let Some(e) = st.key_editor.as_mut() else { return false };
    match key_editor(ctx, e, profiles) {
        Outcome::Open => false,
        Outcome::Cancel => {
            st.key_editor = None;
            false
        }
        Outcome::Save => {
            let e = st.key_editor.take().unwrap();
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
    }
}

/// iTerm2's Pointer pane.
pub fn pointer_page(ui: &mut egui::Ui, g: &mut Globals, st: &mut KeysState, profiles: &[(String, String)]) -> bool {
    let mut changed = false;
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.strong("General");
        for s in globals_in("General").into_iter().filter(|s| s.tab == "Pointer") {
            // Semantic History arrives in milestone 4.
            changed |= global_control(ui, g, s, s.key != "CommandSelection");
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
pub fn profile_key_mappings(ui: &mut egui::Ui, p: &mut Profile, st: &mut KeysState, profiles: &[(String, String)]) -> bool {
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
    if ui.checkbox(&mut ctrl_h, "Delete sends ^H").changed() {
        set_delete_sends_ctrl_h(&mut map, ctrl_h);
        changed = true;
    }
    if st.key_editor.as_ref().is_some_and(|e| e.target == Target::Profile) && finish_key_editor(ui.ctx(), st, &mut map, profiles) {
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
    fn pointer_rows_show_labels() {
        let rows = pointer_rows(crate::globals::keymaps::factory_pointer());
        assert!(rows.contains(&("Button,1,1,,".to_string(), "Right button, single click".to_string(), "Open Context Menu".to_string())));
    }
}

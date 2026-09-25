//! The hotkey window settings: Profiles > Keys' hotkey row, the HotKey Window Preferences
//! sheet, Additional Hotkeys, and the Hotkey part of Settings > Keys (Create a Dedicated
//! Hotkey Window..., App Hotkey), with iTerm2's texts.

use std::collections::HashMap;

use toml::Value;

use crate::globals::Globals;
use crate::hotkey::{self, Prefs, Shortcut};
use crate::keystroke::{self, Keystroke};
use crate::profile::Profile;
use crate::settings_keys::{HotkeySlot, KeysState};
use crate::store::Store;

/// The sheet's settings, written to the profile only on OK.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub primary: Option<Shortcut>,
    pub tap_on: bool,
    pub tap: u8,
    pub alternates: Vec<Option<Shortcut>>,
    pub pinned: bool,
    pub reopens: bool,
    pub animates: bool,
    pub floats: bool,
    pub dock: i64,
}

impl Model {
    pub fn from_profile(p: &Profile) -> Model {
        let f = Prefs::from_profile(p);
        Model {
            primary: f.primary,
            tap_on: f.double_tap.is_some(),
            tap: p.int("HotKey Modifier Activation").clamp(0, 3) as u8,
            alternates: f.alternates.into_iter().map(Some).collect(),
            pinned: !f.auto_hides,
            reopens: f.reopens,
            animates: f.animates,
            floats: f.floats,
            dock: p.int("HotKey Window Dock Click Action"),
        }
    }

    /// A new hotkey window's model (iTerm2: auto-hide and animate on, the rest off).
    pub fn fresh() -> Model {
        Model { primary: None, tap_on: false, tap: 0, alternates: Vec::new(), pinned: false, reopens: false, animates: true, floats: false, dock: 0 }
    }

    pub fn assigned(&self) -> bool {
        self.primary.is_some() || self.tap_on || self.alternates.iter().any(Option::is_some)
    }

    fn prefs(&self) -> Prefs {
        Prefs { has_hotkey: true, primary: self.primary.clone(), double_tap: self.tap_on.then_some(self.tap), alternates: self.alternates.iter().flatten().cloned().collect(), auto_hides: !self.pinned, reopens: self.reopens, animates: self.animates, floats: self.floats }
    }

    /// The profile keys the sheet writes.
    pub fn to_table(&self) -> toml::Table {
        let mut t = toml::Table::new();
        let p = self.primary.clone().unwrap_or(Shortcut { keycode: 0, flags: 0, chars: String::new(), chars_ignoring: String::new() });
        t.insert("Has Hotkey".into(), Value::Boolean(self.assigned()));
        t.insert("HotKey Key Code".into(), Value::Integer(p.keycode as i64));
        t.insert("HotKey Modifier Flags".into(), Value::Integer(p.flags as i64));
        t.insert("HotKey Characters".into(), Value::String(p.chars));
        t.insert("HotKey Characters Ignoring Modifiers".into(), Value::String(p.chars_ignoring));
        t.insert("HotKey Activated By Modifier".into(), Value::Boolean(self.tap_on));
        t.insert("HotKey Modifier Activation".into(), Value::Integer(self.tap as i64));
        t.insert("HotKey Alternate Shortcuts".into(), Value::Array(self.alternates.iter().flatten().map(Shortcut::to_value).collect()));
        t.insert("HotKey Window AutoHides".into(), Value::Boolean(!self.pinned));
        t.insert("HotKey Window Reopens On Activation".into(), Value::Boolean(self.reopens));
        t.insert("HotKey Window Animates".into(), Value::Boolean(self.animates));
        t.insert("HotKey Window Floats".into(), Value::Boolean(self.floats));
        t.insert("HotKey Window Dock Click Action".into(), Value::Integer(self.dock));
        t
    }
}

/// A recorded keystroke as a stored shortcut.
pub fn shortcut_of(k: &Keystroke) -> Option<Shortcut> {
    let ch = char::from_u32(k.ch)?;
    let typed = char::from_u32(k.modified).unwrap_or(ch);
    Some(Shortcut { keycode: k.keycode?, flags: k.flags & (keystroke::SHIFT | keystroke::CONTROL | keystroke::OPTION | keystroke::COMMAND), chars: typed.to_string(), chars_ignoring: ch.to_string() })
}

/// "Win+Alt+Space".
pub fn label(s: &Shortcut) -> String {
    let ch = s.chars_ignoring.chars().next().map_or(0, |c| c as u32);
    Keystroke::new(ch, s.flags, Some(s.keycode), ch).label()
}

pub struct Sheet {
    /// The profile it edits; None when Create a Dedicated Hotkey Window... makes a new one.
    pub guid: Option<String>,
    pub model: Model,
    /// A shortcut that smells accidental, waiting for OK.
    confirm: Option<(HotkeySlot, Shortcut)>,
    additional: bool,
    selected_alternate: Option<usize>,
}

impl Sheet {
    pub fn edit(guid: &str, p: &Profile) -> Sheet {
        Sheet { guid: Some(guid.to_string()), model: Model::from_profile(p), confirm: None, additional: false, selected_alternate: None }
    }

    pub fn create() -> Sheet {
        Sheet { guid: None, model: Model::fresh(), confirm: None, additional: false, selected_alternate: None }
    }
}

/// The hotkey settings' state, kept by the settings window.
#[derive(Default)]
pub struct HotkeyUi {
    pub sheet: Option<Sheet>,
    /// Registration failures by profile guid ("App Hotkey" for the app hotkey).
    pub errors: HashMap<String, String>,
    /// Create a Dedicated Hotkey Window...: the question when hotkey profiles exist.
    create_warning: Option<Vec<(String, String)>>,
    /// The name of the profile just made, for the success message.
    created: Option<String>,
}

enum Outcome {
    Open,
    Cancel,
    Ok,
}

const WARN: egui::Color32 = egui::Color32::from_rgb(0xD0, 0x8A, 0x00);

fn recorder(ui: &mut egui::Ui, keys: &mut KeysState, slot: HotkeySlot, current: Option<&Shortcut>, enabled: bool) -> bool {
    let mut cleared = false;
    ui.horizontal(|ui| {
        let text = if keys.hotkey_recording == Some(slot) { "Recording".to_string() } else { current.map_or("Click to Set".to_string(), label) };
        if ui.add_enabled(enabled, egui::Button::new(text).min_size(egui::vec2(160.0, 0.0))).clicked() {
            keys.hotkey_recording = if keys.hotkey_recording == Some(slot) { None } else { Some(slot) };
        }
        if current.is_some() && ui.add_enabled(enabled, egui::Button::new("x")).on_hover_text("Clear").clicked() {
            cleared = true;
        }
    });
    cleared
}

fn assign(model: &mut Model, slot: HotkeySlot, s: Option<Shortcut>) {
    match slot {
        HotkeySlot::Primary => {
            model.primary = s;
            // Clearing the primary shortcut promotes the first additional one (iTerm2).
            if model.primary.is_none() {
                if let Some(i) = model.alternates.iter().position(Option::is_some) {
                    model.primary = model.alternates.remove(i);
                }
            }
        }
        HotkeySlot::Alternate(i) => {
            if let Some(a) = model.alternates.get_mut(i) {
                *a = s;
            }
        }
        HotkeySlot::App => {}
    }
}

/// The HotKey Window Preferences sheet.
fn sheet(ctx: &egui::Context, sh: &mut Sheet, keys: &mut KeysState, others: &[(String, Prefs)], error: Option<&str>) -> Outcome {
    if let Some((slot, k)) = keys.hotkey_recorded.take() {
        if let Some(s) = shortcut_of(&k) {
            if hotkey::smells_accidental(&s) {
                sh.confirm = Some((slot, s));
            } else {
                assign(&mut sh.model, slot, Some(s));
            }
        }
    }
    let mut out = Outcome::Open;
    let me = sh.guid.clone().unwrap_or_default();
    let (chord_taken, tap_taken) = hotkey::conflicts(&me, &sh.model.prefs(), others);
    egui::Window::new("HotKey Window Preferences").collapsible(false).resizable(false).default_width(460.0).show(ctx, |ui| {
        let m = &mut sh.model;
        ui.label(egui::RichText::new(if sh.guid.is_some() {
            "When you press the hotkey bound to this profile a dedicated window will show or hide. Each profile with a hotkey controls its own Hotkey Window."
        } else {
            "This panel helps you configure a new profile that will be bound to a keystroke you assign. Pressing the hotkey (even when Tandem is not active) will toggle a special window."
        }).small());
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("Hotkey:");
            if recorder(ui, keys, HotkeySlot::Primary, m.primary.as_ref(), true) {
                assign(m, HotkeySlot::Primary, None);
            }
        });
        if chord_taken {
            ui.colored_label(WARN, "Warning: Another profile uses this shortcut");
        }
        if let Some(e) = error {
            ui.colored_label(WARN, format!("Warning: {e}"));
        }
        let any = m.assigned();
        ui.horizontal(|ui| {
            ui.checkbox(&mut m.tap_on, "Double-tap key:");
            ui.add_enabled_ui(m.tap_on, |ui| {
                egui::ComboBox::from_id_salt("hotkey-tap").selected_text(["Control", "Shift", "Alt", "Win"][m.tap as usize % 4]).show_ui(ui, |ui| {
                    for (i, name) in ["Control", "Shift", "Alt", "Win"].iter().enumerate() {
                        ui.selectable_value(&mut m.tap, i as u8, *name);
                    }
                });
            });
        });
        if tap_taken {
            ui.colored_label(WARN, "Warning: Another profile uses this double-tap");
        }
        ui.add_enabled_ui(any, |ui| {
            ui.checkbox(&mut m.pinned, "Pin hotkey window (stays open on loss of keyboard focus)");
            ui.add_enabled(!m.pinned, egui::Checkbox::new(&mut m.reopens, "Automatically reopen on app reactivation")).on_hover_text("Note: the window is reopened only if it was closed because another app became active.");
            ui.checkbox(&mut m.animates, "Animate showing and hiding");
            ui.checkbox(&mut m.floats, "Floating window");
            ui.label(egui::RichText::new("Floating windows stay above other windows.").small());
            ui.label("On Dock icon click:");
            ui.add_enabled_ui(false, |ui| {
                for (v, text) in [(0, "Don't do anything special"), (1, "Show this Hotkey Window"), (2, "Show this Hotkey Window if no other window is open")] {
                    ui.radio_value(&mut m.dock, v, text);
                }
            })
            .response
            .on_disabled_hover_text("Windows has no Dock; this setting is kept for profiles shared with iTerm2.");
        });
        ui.horizontal(|ui| {
            if ui.add_enabled(m.primary.is_some(), egui::Button::new("Additional Hotkeys...")).clicked() {
                sh.additional = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Cancel").clicked() {
                    out = Outcome::Cancel;
                }
                if ui.button("OK").clicked() {
                    out = Outcome::Ok;
                }
            });
        });
    });
    if sh.additional {
        let mut open = true;
        egui::Window::new("Additional Hotkeys").collapsible(false).resizable(true).min_width(306.0).show(ctx, |ui| {
            let others_chords: Vec<(u16, u64)> = others.iter().filter(|(g, p)| *g != me && p.active()).flat_map(|(_, p)| p.shortcuts().into_iter().map(|s| (s.keycode, s.flags)).collect::<Vec<_>>()).collect();
            let mut clear = None;
            for i in 0..sh.model.alternates.len() {
                ui.horizontal(|ui| {
                    if ui.selectable_label(sh.selected_alternate == Some(i), format!("{}", i + 1)).clicked() {
                        sh.selected_alternate = Some(i);
                    }
                    let cur = sh.model.alternates[i].clone();
                    if recorder(ui, keys, HotkeySlot::Alternate(i), cur.as_ref(), true) {
                        clear = Some(i);
                    }
                    if cur.as_ref().is_some_and(|s| others_chords.contains(&(s.keycode, s.flags))) {
                        ui.colored_label(WARN, "Warning: another profile uses it");
                    }
                });
            }
            if let Some(i) = clear {
                sh.model.alternates[i] = None;
            }
            ui.horizontal(|ui| {
                if ui.button("+").clicked() {
                    sh.model.alternates.push(None);
                }
                if ui.add_enabled(sh.selected_alternate.is_some(), egui::Button::new("-")).clicked() {
                    if let Some(i) = sh.selected_alternate.take() {
                        if i < sh.model.alternates.len() {
                            sh.model.alternates.remove(i);
                        }
                    }
                }
                if ui.button("OK").clicked() {
                    open = false;
                }
            });
        });
        if !open {
            sh.additional = false;
            sh.model.alternates.retain(Option::is_some);
        }
    }
    if let Some((slot, s)) = sh.confirm.clone() {
        let (heading, text) = hotkey::confirm_text(&label(&s));
        egui::Window::new(heading).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.label(text);
            ui.horizontal(|ui| {
                if ui.button("OK").clicked() {
                    assign(&mut sh.model, slot, Some(s.clone()));
                    sh.confirm = None;
                }
                if ui.button("Cancel").clicked() {
                    sh.confirm = None;
                }
            });
        });
    }
    out
}

/// Profiles > Keys: "Hotkey window:" with its checkbox and Configure Hotkey Window. True
/// when the profile changed.
pub fn profile_row(ui: &mut egui::Ui, guid: &str, p: &mut Profile, st: &mut HotkeyUi, keys: &mut KeysState, others: &[(String, Prefs)]) -> bool {
    let mut changed = false;
    let mut on = p.bool("Has Hotkey");
    ui.horizontal(|ui| {
        ui.label("Hotkey window:");
        if ui.checkbox(&mut on, "A hotkey opens a dedicated window with this profile").changed() {
            // Turning it on with no hotkey yet opens the sheet instead (iTerm2).
            if on && !Prefs::from_profile(p).assigned() {
                st.sheet = Some(Sheet::edit(guid, p));
            } else {
                p.set("Has Hotkey", Value::Boolean(on));
                changed = true;
            }
        }
        if ui.add_enabled(on, egui::Button::new("Configure Hotkey Window")).clicked() {
            st.sheet = Some(Sheet::edit(guid, p));
        }
    });
    if let Some(e) = st.errors.get(guid).filter(|_| on) {
        ui.colored_label(WARN, format!("Warning: {e}"));
    }
    if st.sheet.as_ref().is_some_and(|s| s.guid.as_deref() == Some(guid)) {
        let error = st.errors.get(guid).cloned();
        let sh = st.sheet.as_mut().expect("checked above");
        match sheet(ui.ctx(), sh, keys, others, error.as_deref()) {
            Outcome::Open => {}
            Outcome::Cancel => st.sheet = None,
            Outcome::Ok => {
                for (k, v) in sh.model.to_table() {
                    p.set(&k, v);
                }
                st.sheet = None;
                changed = true;
            }
        }
    }
    changed
}

/// What the Keys page's hotkey part changed.
pub enum KeysEdit {
    None,
    Globals,
    /// A new profile was added (its guid).
    Profile(String),
}

/// Settings > Keys > Hotkey: Create a Dedicated Hotkey Window..., and App Hotkey.
pub fn keys_section(ui: &mut egui::Ui, g: &mut Globals, store: &mut Store, st: &mut HotkeyUi, keys: &mut KeysState) -> KeysEdit {
    let mut edit = KeysEdit::None;
    let others: Vec<(String, Prefs)> = store.profiles.iter().map(|p| (p.guid(), Prefs::from_profile(p))).collect();
    ui.add_space(8.0);
    ui.strong("Hotkey");
    ui.horizontal(|ui| {
        ui.label("Hotkey Window:");
        if ui.button("Create a Dedicated Hotkey Window...").clicked() {
            let existing: Vec<(String, String)> = store.profiles.iter().filter(|p| Prefs::from_profile(p).active()).map(|p| (p.guid(), p.name())).collect();
            if existing.is_empty() || g.bool("NoSyncSuppressAddAnotherHotkeyProfileWarning") {
                st.sheet = Some(Sheet::create());
            } else {
                st.create_warning = Some(existing);
            }
        }
    });
    // App Hotkey: Show/hide all windows.
    if let Some((HotkeySlot::App, k)) = keys.hotkey_recorded.clone() {
        keys.hotkey_recorded = None;
        if let Some(s) = shortcut_of(&k) {
            g.set("HotkeyCode", Value::Integer(s.keycode as i64));
            g.set("HotkeyModifiers", Value::Integer(s.flags as i64));
            g.set("HotkeyChar", Value::Integer(s.chars_ignoring.chars().next().map_or(0, |c| c as i64)));
            edit = KeysEdit::Globals;
        }
    }
    let mut on = g.bool("Hotkey");
    ui.horizontal(|ui| {
        ui.label("App Hotkey:");
        if ui.checkbox(&mut on, "Show/hide all windows with a system-wide hotkey").changed() {
            g.set("Hotkey", Value::Boolean(on));
            // Turned on with no key: Alt+Space (iTerm2's Option+Space).
            if on && g.int("HotkeyChar") == 0 {
                g.set("HotkeyCode", Value::Integer(0x31));
                g.set("HotkeyModifiers", Value::Integer(keystroke::OPTION as i64));
                g.set("HotkeyChar", Value::Integer(' ' as i64));
            }
            edit = KeysEdit::Globals;
        }
    });
    let ch = g.int("HotkeyChar");
    let current = (ch > 0).then(|| {
        let c = char::from_u32(ch as u32).unwrap_or(' ').to_string();
        Shortcut { keycode: g.int("HotkeyCode").clamp(0, u16::MAX as i64) as u16, flags: g.int("HotkeyModifiers").max(0) as u64, chars: c.clone(), chars_ignoring: c }
    });
    ui.horizontal(|ui| {
        ui.add_space(96.0);
        if recorder(ui, keys, HotkeySlot::App, current.as_ref(), on) {
            // Clearing the shortcut turns the app hotkey off (iTerm2).
            g.set("Hotkey", Value::Boolean(false));
            g.set("HotkeyChar", Value::Integer(0));
            edit = KeysEdit::Globals;
        }
    });
    if on {
        if let Some(c) = &current {
            if others.iter().any(|(_, p)| p.active() && p.shortcuts().iter().any(|s| (s.keycode, s.flags) == (c.keycode, c.flags))) {
                ui.colored_label(WARN, "Warning: A profile also uses this shortcut");
            }
        }
        if let Some(e) = st.errors.get("App Hotkey") {
            ui.colored_label(WARN, format!("Warning: {e}"));
        }
    }
    // The question before another hotkey profile.
    if let Some(existing) = st.create_warning.clone() {
        let names: Vec<String> = existing.iter().map(|(_, n)| n.clone()).collect();
        let (title, text, buttons) = hotkey::create_warning(&names);
        egui::Window::new(title).collapsible(false).resizable(false).show(ui.ctx(), |ui| {
            ui.label(text);
            let mut silence = g.bool("NoSyncSuppressAddAnotherHotkeyProfileWarning");
            if ui.checkbox(&mut silence, "Do not ask again").changed() {
                g.set("NoSyncSuppressAddAnotherHotkeyProfileWarning", Value::Boolean(silence));
                edit = KeysEdit::Globals;
            }
            ui.horizontal(|ui| {
                for b in buttons {
                    if ui.button(b).clicked() {
                        st.create_warning = None;
                        match b {
                            "OK" => st.sheet = Some(Sheet::create()),
                            "Configure Existing Profile" => {
                                if let Some(p) = store.get(&existing[0].0) {
                                    st.sheet = Some(Sheet::edit(&existing[0].0, p));
                                }
                            }
                            _ => {}
                        }
                    }
                }
            });
        });
    }
    // The sheet, for a new profile or an existing one opened from here.
    if st.sheet.is_some() {
        let guid = st.sheet.as_ref().and_then(|s| s.guid.clone());
        let error = guid.as_ref().and_then(|g| st.errors.get(g).cloned());
        let sh = st.sheet.as_mut().expect("checked above");
        match sheet(ui.ctx(), sh, keys, &others, error.as_deref()) {
            Outcome::Open => {}
            Outcome::Cancel => st.sheet = None,
            Outcome::Ok => {
                let table = sh.model.to_table();
                let assigned = sh.model.assigned();
                st.sheet = None;
                match guid {
                    Some(g) => {
                        if let Some(p) = store.get_mut(&g) {
                            for (k, v) in table {
                                p.set(&k, v);
                            }
                            edit = KeysEdit::Profile(g);
                        }
                    }
                    // OK with no hotkey does nothing (iTerm2).
                    None if !assigned => {}
                    None => {
                        let names: Vec<String> = store.profiles.iter().map(|p| p.name()).collect();
                        let new_guid = uuid::Uuid::new_v4().to_string().to_uppercase();
                        let p = hotkey::dedicated_profile(store.default_profile(), &names, &table, &new_guid);
                        st.created = Some(p.name());
                        store.add(p);
                        edit = KeysEdit::Profile(new_guid);
                    }
                }
            }
        }
    }
    if let Some(name) = st.created.clone() {
        egui::Window::new("Hotkey Window Successfully Configured").collapsible(false).resizable(false).show(ui.ctx(), |ui| {
            ui.label(format!("A new profile called \u{201c}{name}\u{201d} was created for you. It is tuned to work well for the Hotkey Window feature and it can be customized in the Profiles tab."));
            if ui.button("OK").clicked() {
                st.created = None;
            }
        });
    }
    edit
}

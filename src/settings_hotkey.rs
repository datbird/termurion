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
            tap: crate::hotkey::tap_key(p.int("HotKey Modifier Activation")),
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
    /// The name of the profile just made, for the success message.
    created: Option<String>,
    /// An Open Quickly hotkey that smells accidental, waiting for OK.
    quickly_confirm: Option<Shortcut>,
}

/// Create a Dedicated Hotkey Window...'s question when hotkey profiles named `names` exist
/// (`KP:385-418`): persistent, so always asked; Cancel is the last button.
pub fn create_warning_spec(names: &[String]) -> crate::warnings::Spec {
    let (heading, text, labels) = hotkey::create_warning(names);
    crate::warnings::Spec {
        id: Some("NoSyncSuppressAddAnotherHotkeyProfileWarning".into()),
        kind: crate::warnings::Kind::Persistent,
        heading,
        text,
        labels: labels.iter().map(|l| l.to_string()).collect(),
        cancel: labels.iter().position(|l| *l == crate::api_auth::CANCEL),
        never_remember: Vec::new(),
    }
}

/// The question before another hotkey profile answered with button `index`: OK opens the
/// sheet for a new profile, Configure Existing Profile (only with one hotkey profile) the
/// sheet for it, Cancel nothing.
pub fn create_answered(st: &mut HotkeyUi, store: &Store, existing: &[(String, String)], index: usize) {
    match (index, existing) {
        (0, _) => st.sheet = Some(Sheet::create()),
        (1, [(guid, _)]) => {
            if let Some(p) = store.get(guid) {
                st.sheet = Some(Sheet::edit(guid, p));
            }
        }
        _ => {}
    }
}

/// The Open Quickly hotkey's help (`PreferencePanel.xib:9797`).
pub const OPEN_QUICKLY_HELP: &str = "When set, the Open Quickly window can be opened even when another app is active.";

/// Stores the Open Quickly hotkey, or zeros for none (clearing unregisters it,
/// `KeysPreferencesViewController.m:675-683`).
fn set_quickly_hotkey(g: &mut Globals, s: Option<&Shortcut>) {
    g.set("OpenQuicklyHotkeyCode", Value::Integer(s.map_or(0, |s| s.keycode as i64)));
    g.set("OpenQuicklyHotkeyModifiers", Value::Integer(s.map_or(0, |s| s.flags as i64)));
    g.set("OpenQuicklyHotkeyChar", Value::Integer(s.and_then(|s| s.chars_ignoring.chars().next()).map_or(0, |c| c as i64)));
}

/// The Open Quickly hotkey as stored, when one is set (a key code or a character).
pub fn quickly_hotkey(g: &Globals) -> Option<Shortcut> {
    let (code, ch) = (g.int("OpenQuicklyHotkeyCode"), g.int("OpenQuicklyHotkeyChar"));
    if code == 0 && ch == 0 {
        return None;
    }
    let text = char::from_u32(ch as u32).filter(|_| ch > 0).map(String::from).unwrap_or_default();
    Some(Shortcut { keycode: code.clamp(0, u16::MAX as i64) as u16, flags: g.int("OpenQuicklyHotkeyModifiers").max(0) as u64, chars: text.clone(), chars_ignoring: text })
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
            keys.win_note = false;
        }
        if keys.hotkey_recording == Some(slot) && keys.win_note {
            ui.weak(crate::settings_keys::WIN_NOTE);
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
        HotkeySlot::App | HotkeySlot::OpenQuickly | HotkeySlot::Session | HotkeySlot::Workgroup => {}
    }
}

/// The pane the Session Hotkey row writes: the one Settings was opened to edit (Edit Session),
/// while the profile that session uses (`session_profile`, its shared Guid) is the one selected;
/// no row otherwise (`PreferencePanel` shows it only in a session's own editor, and another
/// profile picked in the list is not that session's).
pub fn session_row_pane(edit_session: Option<crate::layout::PaneId>, selected: &str, session_profile: Option<&str>) -> Option<crate::layout::PaneId> {
    edit_session.filter(|_| session_profile == Some(selected))
}

/// A set or cleared session hotkey as the edit the app takes: the pane's own session value.
pub fn session_edit(pane: crate::layout::PaneId, s: Option<Shortcut>) -> crate::settings_ui::Edit {
    crate::settings_ui::Edit::SessionHotkey(pane, s)
}

/// The note for a stored session hotkey that is not the chord the person allowed.
pub const STALE_NOTE: &str = "Not registered: this is not the hotkey you allowed; record it here to turn it on";

/// The note under the Session Hotkey row: a value that waits for the person's answer is not
/// registered (never shown as live), nor is one the person answered Ignore or one that is not the
/// allowed chord, and a refused chord says why.
pub fn session_row_note(has_value: bool, waiting: bool, refused: Option<crate::session_hotkeys::NotLive>, error: Option<&str>) -> Option<String> {
    use crate::session_hotkeys::NotLive;
    if has_value && waiting {
        return Some("Not registered: waiting for your answer to the question".to_string());
    }
    match refused.filter(|_| has_value) {
        Some(NotLive::Ignored) => return Some("Not registered: you chose Ignore".to_string()),
        Some(NotLive::Stale) => return Some(STALE_NOTE.to_string()),
        None => {}
    }
    error.map(|e| format!("Warning: {e}"))
}

/// The Session Hotkey row: a recorder for `pane`'s hotkey, with the app's warning when the chord
/// is not registered. The edit when the person recorded or cleared one.
pub fn session_row(ui: &mut egui::Ui, pane: crate::layout::PaneId, current: Option<&Shortcut>, waiting: bool, refused: Option<crate::session_hotkeys::NotLive>, st: &HotkeyUi, keys: &mut KeysState) -> Option<crate::settings_ui::Edit> {
    let mut out = None;
    if let Some((HotkeySlot::Session, k)) = keys.hotkey_recorded.clone() {
        keys.hotkey_recorded = None;
        if let Some(s) = shortcut_of(&k) {
            out = Some(session_edit(pane, Some(s)));
        }
    }
    ui.horizontal(|ui| {
        ui.label("Session hotkey:");
        if recorder(ui, keys, HotkeySlot::Session, current, true) {
            out = Some(session_edit(pane, None));
        }
    });
    if let Some(e) = session_row_note(current.is_some(), waiting, refused, st.errors.get(&crate::session_hotkeys::error_key(pane)).map(String::as_str)) {
        ui.colored_label(WARN, e);
    }
    out
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
            "This panel helps you configure a new profile that will be bound to a keystroke you assign. Pressing the hotkey (even when Termurion is not active) will toggle a special window."
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
                egui::ComboBox::from_id_salt("hotkey-tap").selected_text(["Control", "Shift", "Alt"][m.tap as usize % 3]).show_ui(ui, |ui| {
                    for (i, name) in ["Control", "Shift", "Alt"].iter().enumerate() {
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
            ui.label("On notification area icon click:");
            for (v, text) in [(0, "Don't do anything special"), (1, "Show this Hotkey Window"), (2, "Show this Hotkey Window if no other window is open")] {
                ui.radio_value(&mut m.dock, v, text);
            }
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
    // A recorder left armed would eat the next key in Settings.
    if !matches!(out, Outcome::Open) {
        keys.hotkey_recording = None;
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
        let check = ui.checkbox(&mut on, "A hotkey opens a dedicated window with this profile");
        if check.changed() {
            // Turning it on with no hotkey yet opens the sheet instead (iTerm2).
            if on && !Prefs::from_profile(p).assigned() {
                st.sheet = Some(Sheet::edit(guid, p));
            } else {
                p.set("Has Hotkey", Value::Boolean(on));
                changed = true;
            }
        }
        // `ProfilesKeysPreferencesViewController.m:196-198`: marked while Has Hotkey is on.
        let marked = crate::schema::find("Has Hotkey").is_some_and(|s| !crate::settings_ui::profile_is_default(p, s, "Has Hotkey"));
        if crate::settings_keys::indicate_non_default() && marked {
            crate::settings_keys::non_default_bar(ui, check.rect, ui.is_enabled());
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

/// Settings > Keys > Hotkey: Create a Dedicated Hotkey Window..., and App Hotkey. The question
/// before another hotkey profile goes to `keys.ask` (the settings window shows it and calls
/// `create_answered`).
pub fn keys_section(ui: &mut egui::Ui, g: &mut Globals, store: &mut Store, st: &mut HotkeyUi, keys: &mut KeysState) -> KeysEdit {
    let mut edit = KeysEdit::None;
    let others: Vec<(String, Prefs)> = store.profiles.iter().map(|p| (p.guid(), Prefs::from_profile(p))).collect();
    ui.add_space(8.0);
    ui.strong("Hotkey");
    ui.horizontal(|ui| {
        ui.label("Hotkey Window:");
        if ui.button("Create a Dedicated Hotkey Window...").clicked() {
            let existing: Vec<(String, String)> = store.profiles.iter().filter(|p| Prefs::from_profile(p).active()).map(|p| (p.guid(), p.name())).collect();
            if existing.is_empty() {
                st.sheet = Some(Sheet::create());
            } else {
                let names: Vec<String> = existing.iter().map(|(_, n)| n.clone()).collect();
                keys.ask = Some(crate::settings_keys::SheetWarning::new(create_warning_spec(&names), crate::settings_keys::SheetThen::CreateHotkey(existing)));
                ui.ctx().request_repaint();
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
    // Open Quickly Hotkey: no leader, and a shortcut that smells accidental asks first.
    if let Some((HotkeySlot::OpenQuickly, k)) = keys.hotkey_recorded.clone() {
        keys.hotkey_recorded = None;
        if let Some(s) = shortcut_of(&k) {
            if hotkey::smells_accidental(&s) {
                st.quickly_confirm = Some(s);
            } else {
                set_quickly_hotkey(g, Some(&s));
                edit = KeysEdit::Globals;
            }
        }
    }
    let quickly = quickly_hotkey(g);
    ui.horizontal(|ui| {
        ui.label("Open Quickly Hotkey:");
        if recorder(ui, keys, HotkeySlot::OpenQuickly, quickly.as_ref(), true) {
            set_quickly_hotkey(g, None);
            edit = KeysEdit::Globals;
        }
    });
    ui.horizontal(|ui| {
        ui.add_space(96.0);
        ui.weak(OPEN_QUICKLY_HELP);
    });
    if quickly.is_some() {
        if let Some(e) = st.errors.get("Open Quickly Hotkey") {
            ui.colored_label(WARN, format!("Warning: {e}"));
        }
    }
    if let Some(s) = st.quickly_confirm.clone() {
        let (heading, text) = hotkey::confirm_text_for(&label(&s), true);
        egui::Window::new(heading).id(egui::Id::new("open-quickly-confirm")).collapsible(false).resizable(false).show(ui.ctx(), |ui| {
            ui.label(text);
            ui.horizontal(|ui| {
                if ui.button("OK").clicked() {
                    set_quickly_hotkey(g, Some(&s));
                    st.quickly_confirm = None;
                    edit = KeysEdit::Globals;
                }
                if ui.button("Cancel").clicked() {
                    st.quickly_confirm = None;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_row_marks_a_waiting_value() {
        assert_eq!(session_row_note(true, true, None, None).as_deref(), Some("Not registered: waiting for your answer to the question"));
        assert_eq!(session_row_note(true, true, None, Some("Another hotkey uses this shortcut")).as_deref(), Some("Not registered: waiting for your answer to the question"), "waiting comes first");
        assert_eq!(session_row_note(true, false, None, Some("Windows keeps this shortcut for itself")).as_deref(), Some("Warning: Windows keeps this shortcut for itself"));
        assert_eq!(session_row_note(true, false, None, None), None);
        assert_eq!(session_row_note(false, false, None, None), None);
    }

    #[test]
    fn a_refused_value_shows_as_not_registered() {
        use crate::session_hotkeys::NotLive;
        // Answered Ignore (or refused by a tmux decision): stored, not live, and the row says so.
        assert_eq!(session_row_note(true, false, Some(NotLive::Ignored), None).as_deref(), Some("Not registered: you chose Ignore"));
        assert_eq!(session_row_note(true, false, Some(NotLive::Ignored), Some("Windows keeps this shortcut for itself")).as_deref(), Some("Not registered: you chose Ignore"));
        assert_eq!(session_row_note(false, false, Some(NotLive::Ignored), None), None, "no value, no note");
        // A stored value another writer changed: not the registered one.
        assert_eq!(session_row_note(true, false, Some(NotLive::Stale), None).as_deref(), Some(STALE_NOTE));
    }

    #[test]
    fn the_session_row_shows_only_under_the_edited_sessions_profile() {
        // Edit Session shows the row for its pane while the session's profile is selected.
        assert_eq!(session_row_pane(Some(7), "g1", Some("g1")), Some(7));
        // Another profile selected in the list: no row (its value is not this session's).
        assert_eq!(session_row_pane(Some(7), "g2", Some("g1")), None);
        // Settings alone has none.
        assert_eq!(session_row_pane(None, "g1", Some("g1")), None);
        assert_eq!(session_row_pane(Some(7), "g1", None), None);
    }

    #[test]
    fn open_quickly_hotkey_stores_like_iterm2() {
        let mut g = Globals::empty(std::path::Path::new("globals-test.toml"));
        assert_eq!(quickly_hotkey(&g), None, "all zeros is unbound");
        let s = Shortcut { keycode: 0x1F, flags: keystroke::OPTION, chars: "o".into(), chars_ignoring: "o".into() };
        set_quickly_hotkey(&mut g, Some(&s));
        assert_eq!((g.int("OpenQuicklyHotkeyCode"), g.int("OpenQuicklyHotkeyChar"), g.int("OpenQuicklyHotkeyModifiers")), (0x1F, 'o' as i64, keystroke::OPTION as i64));
        assert_eq!(quickly_hotkey(&g), Some(s));
        // Clearing stores zeros, which unregisters it.
        set_quickly_hotkey(&mut g, None);
        assert_eq!((g.int("OpenQuicklyHotkeyCode"), g.int("OpenQuicklyHotkeyChar"), g.int("OpenQuicklyHotkeyModifiers")), (0, 0, 0));
        assert_eq!(quickly_hotkey(&g), None);
    }

    /// The question before another hotkey profile (`KP:385-418`): persistent, so never
    /// answered for the person and without a checkbox; Cancel is the last button.
    #[test]
    fn create_warning_follows_iterm2() {
        let one = create_warning_spec(&["Quake".to_string()]);
        assert_eq!((one.heading.as_str(), one.text.as_str()), ("Add Another Hotkey Window Profile?", "You already have a Profile with a Hotkey Window named \u{201c}Quake\u{201d}."));
        assert_eq!((one.labels.clone(), one.cancel), (vec!["OK".to_string(), "Configure Existing Profile".into(), "Cancel".into()], Some(2)));
        assert_eq!((one.id.as_deref(), one.kind), (Some("NoSyncSuppressAddAnotherHotkeyProfileWarning"), crate::warnings::Kind::Persistent));
        assert_eq!(crate::warnings::checkbox_label(&one), None);
        let two = create_warning_spec(&["A".to_string(), "B".into()]);
        assert_eq!((two.labels.clone(), two.cancel), (vec!["OK".to_string(), "Cancel".into()], Some(1)));
        // A stored flag (Termurion's old "Do not ask again") no longer skips it.
        let mut g = Globals::empty(std::path::Path::new("hotkey-warning-test.toml"));
        g.set("NoSyncSuppressAddAnotherHotkeyProfileWarning", Value::Boolean(true));
        let w = crate::settings_keys::SheetWarning::new(one, crate::settings_keys::SheetThen::CreateHotkey(vec![("G".into(), "Quake".into())]));
        assert_eq!(crate::settings_keys::sheet_preempt(&g, &w, false, 0.0), None);
    }

    /// The dock preference is the notification area icon's left click on Windows.
    #[test]
    fn dock_preference_follows_windows() {
        let s = crate::schema::find("HotKey Window Dock Click Action").expect("the dock key");
        assert!(!s.macos_only);
        assert_eq!(s.label, "On notification area icon click:");
        let values: Vec<Value> = s.choices.iter().map(|c| c.value.clone()).collect();
        assert_eq!(values, vec![Value::Integer(0), Value::Integer(1), Value::Integer(2)]);
    }
}

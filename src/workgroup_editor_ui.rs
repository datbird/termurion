//! Settings > Arrangements > Workgroups: the editor drawn in egui (`iTermWorkgroupsEditingViewController`,
//! `iTermWorkgroupDetailViewController` and `iTermWorkgroupSessionDetailViewController`). The model
//! is `workgroup_edit`; every change goes through one `commit`, which checks the limits the write
//! point applies, takes an undo snapshot and writes "Workgroups" through `Globals::set`
//! (`Model::save`). Commands typed here are stored as typed and never run.

use std::collections::{HashMap, HashSet};

use crate::globals::Globals;
use crate::settings_keys::{HotkeySlot, KeysState};
use crate::workgroup::{self as wg, Kind, Mode, Model, Orientation, SessionConfig, Shortcut, Side, ToolbarItem, Workgroup};
use crate::workgroup_edit as ed;

/// Which shortcut the recorder is waiting for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rec {
    Peer,
    Back,
    Forward,
    NavReload,
    ItemReload,
}

/// What an undo step keeps: the list and where each workgroup came from, so a workgroup that
/// comes back (delete then Undo, Undo then Redo after a reload) keeps its mark.
#[derive(Clone, Debug, PartialEq)]
struct Snapshot {
    list: Vec<Workgroup>,
    marks: std::collections::BTreeMap<String, wg::Source>,
}

/// The editor's state between frames.
#[derive(Default)]
pub struct EditorState {
    selected: Option<String>,
    session: Option<String>,
    item: Option<usize>,
    undo: ed::Undo<Snapshot>,
    model: Option<Model>,
    /// What the model was loaded from (the list and the Imported marks).
    cache: (String, String),
    what_is: bool,
    rec: Option<Rec>,
    notice: Option<String>,
    bufs: HashMap<String, String>,
    drag: Option<usize>,
    /// Profiles whose command is the browser: their sessions show a URL row.
    pub browser: HashSet<String>,
    /// Dynamic Profiles: marked in the profile popup (a workgroup over several of them asks).
    pub dynamic: HashSet<String>,
}

enum Cmd {
    AddWorkgroup,
    AddPreset(String),
    RemoveWorkgroup(String),
    Edit { wg: Workgroup, action: &'static str, coalesce: bool },
    Trust(String),
    Undo,
    Redo,
}

fn key_of(g: &Globals) -> (String, String) {
    (g.string(wg::KEY), g.string(wg::KEY_IMPORTED))
}

fn with_session(w: &Workgroup, id: &str, f: impl FnOnce(&mut SessionConfig)) -> Workgroup {
    let mut out = w.clone();
    if let Some(s) = out.sessions.iter_mut().find(|s| s.unique_identifier == id) {
        f(s);
    }
    ed::enforce_mode_switcher(&mut out);
    out
}

fn shortcut_label(s: &Shortcut) -> String {
    crate::keystroke::parse(&s.serialized).map_or_else(|| s.serialized.clone(), |k| k.label())
}

fn workgroup_title(w: &Workgroup) -> String {
    wg::display_name(w).to_string()
}

/// A single-line text field that keeps what is being typed while it has the focus, so a value the
/// write point would refuse mid-typing (a half-typed address) does not fight the person. Returns
/// the text when it changed.
fn text_field(ui: &mut egui::Ui, st: &mut EditorState, key: &str, stored: &str, max: usize, hint: &str, mono: bool) -> Option<String> {
    let id = ui.id().with(key);
    let focused = ui.memory(|m| m.has_focus(id));
    let buf = st.bufs.entry(key.to_string()).or_default();
    if !focused {
        *buf = stored.to_string();
    }
    let mut edit = egui::TextEdit::singleline(buf).id(id).char_limit(max).desired_width(f32::INFINITY).hint_text(hint);
    if mono {
        edit = edit.font(egui::TextStyle::Monospace);
    }
    let r = ui.add(edit);
    r.changed().then(|| buf.clone())
}

/// A number field that applies when it loses the focus (iTerm2 applies spacer and usage fields at
/// the end of editing); a value that is not a number puts the old one back.
fn number_field(ui: &mut egui::Ui, st: &mut EditorState, key: &str, stored: &str) -> Option<f64> {
    let id = ui.id().with(key);
    let focused = ui.memory(|m| m.has_focus(id));
    let buf = st.bufs.entry(key.to_string()).or_default();
    if !focused && !ui.memory(|m| m.had_focus_last_frame(id)) {
        *buf = stored.to_string();
    }
    let r = ui.add(egui::TextEdit::singleline(buf).id(id).desired_width(80.0).char_limit(16));
    if r.lost_focus() {
        let v = buf.trim().parse::<f64>().ok().filter(|v| v.is_finite());
        if v.is_none() {
            *buf = stored.to_string();
        }
        return v;
    }
    None
}

/// The shortcut button; true when it was cleared. A recorded key arrives through the keys state.
fn recorder(ui: &mut egui::Ui, keys: &mut KeysState, st: &mut EditorState, target: Rec, current: Option<&Shortcut>) -> bool {
    let mut cleared = false;
    ui.horizontal(|ui| {
        let on = st.rec == Some(target) && keys.hotkey_recording == Some(HotkeySlot::Workgroup);
        let text = if on { "Recording".to_string() } else { current.map_or("Click to Set".to_string(), shortcut_label) };
        if ui.add(egui::Button::new(text).min_size(egui::vec2(160.0, 0.0))).clicked() {
            if on {
                keys.hotkey_recording = None;
                st.rec = None;
            } else {
                keys.hotkey_recording = Some(HotkeySlot::Workgroup);
                st.rec = Some(target);
            }
        }
        if current.is_some() && ui.button("x").on_hover_text("Clear").clicked() {
            cleared = true;
        }
    });
    cleared
}

/// The key the recorder delivered and the row it was recorded for. The key is read before the
/// row is forgotten: `deliver` ends the recording in the same step, so a check of "still
/// recording" first would throw every recorded key away.
fn recorded_key(keys: &mut KeysState, st: &mut EditorState) -> Option<(Rec, crate::keystroke::Keystroke)> {
    let got = match keys.hotkey_recorded.clone() {
        Some((HotkeySlot::Workgroup, k)) => {
            keys.hotkey_recorded = None;
            st.rec.take().map(|r| (r, k))
        }
        _ => None,
    };
    if keys.hotkey_recording != Some(HotkeySlot::Workgroup) {
        st.rec = None;
    }
    got
}

/// The editor. `profiles` are (guid, name). True when "Workgroups" changed.
pub fn page(ui: &mut egui::Ui, g: &mut Globals, profiles: &[(String, String)], keys: &mut KeysState, st: &mut EditorState) -> bool {
    let key = key_of(g);
    if st.model.is_none() || st.cache != key {
        st.model = Some(Model::load(g).0);
        st.cache = key;
    }
    let mut model = st.model.take().unwrap_or_default();
    let mut cmd: Option<Cmd> = None;

    // The recorder delivered a key.
    if let Some((rec, k)) = recorded_key(keys, st)
        && let (Some(w), Some(sid)) = (st.selected.as_deref().and_then(|id| model.get(id)), st.session.clone()) {
            let new = Some(Shortcut { serialized: k.serialized() });
            cmd = Some(shortcut_cmd(w, &sid, rec, new, st.item));
        }

    // Undo and redo, when no text field has the focus (a field has its own).
    if ui.memory(|m| m.focused()).is_none() {
        let redo = ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Z) || i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y));
        let undo = !redo && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z));
        if redo {
            cmd = Some(Cmd::Redo);
        } else if undo {
            cmd = Some(Cmd::Undo);
        }
        st.bufs.clear();
    }

    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(220.0);
            left_column(ui, &model, st, &mut cmd);
        });
        ui.separator();
        ui.vertical(|ui| detail(ui, &model, profiles, keys, st, &mut cmd));
    });
    what_is_window(ui.ctx(), st);

    let changed = apply(cmd, &mut model, g, st);
    st.model = Some(model);
    if changed {
        st.cache = key_of(g);
    }
    changed
}

fn shortcut_cmd(w: &Workgroup, sid: &str, rec: Rec, new: Option<Shortcut>, item: Option<usize>) -> Cmd {
    let (action, edited) = match rec {
        Rec::Peer => (ed::CHANGE_PEER_SHORTCUT, with_session(w, sid, |s| s.peer_switch_shortcut = new.clone())),
        _ => (
            ed::CHANGE_SHORTCUT,
            with_session(w, sid, |s| {
                let Some(it) = item.and_then(|i| s.toolbar_items.get_mut(i)) else { return };
                match (it, rec) {
                    (ToolbarItem::Navigation(n), Rec::Back) => n.back = new.clone(),
                    (ToolbarItem::Navigation(n), Rec::Forward) => n.forward = new.clone(),
                    (ToolbarItem::Navigation(n), Rec::NavReload) => n.reload = new.clone(),
                    (ToolbarItem::Reload(r), Rec::ItemReload) => *r = new.clone(),
                    _ => {}
                }
            }),
        ),
    };
    Cmd::Edit { wg: edited, action, coalesce: false }
}

fn left_column(ui: &mut egui::Ui, model: &Model, st: &mut EditorState, cmd: &mut Option<Cmd>) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        egui::ScrollArea::vertical().id_salt("wg-list").max_height(260.0).min_scrolled_height(160.0).auto_shrink([false, false]).show(ui, |ui| {
            ui.set_min_width(190.0);
            for w in &model.workgroups {
                let on = st.selected.as_deref() == Some(w.unique_identifier.as_str());
                let mut text = workgroup_title(w);
                if !model.source_of(&w.unique_identifier).is_person() {
                    text.push_str("  (imported)");
                }
                if ui.selectable_label(on, text).clicked() && !on {
                    st.selected = Some(w.unique_identifier.clone());
                    st.session = None;
                    st.item = None;
                }
            }
        });
    });
    ui.horizontal(|ui| {
        if ui.button("+").on_hover_text("New workgroup").clicked() {
            *cmd = Some(Cmd::AddWorkgroup);
        }
        let sel = st.selected.clone().filter(|id| model.get(id).is_some());
        if ui.add_enabled(sel.is_some(), egui::Button::new("\u{2212}")).on_hover_text("Delete").clicked()
            && let Some(id) = sel {
                *cmd = Some(Cmd::RemoveWorkgroup(id));
            }
        ui.menu_button(ed::ADD_PRESET, |ui| {
            for (id, title) in wg::presets::all() {
                if ui.button(title).clicked() {
                    *cmd = Some(Cmd::AddPreset(id.to_string()));
                    ui.close();
                }
            }
        });
    });
    ui.horizontal(|ui| {
        if ui.add_enabled(st.undo.undo_name().is_some(), egui::Button::new("Undo")).on_hover_text(st.undo.undo_name().map_or(String::new(), |n| format!("Undo {n}"))).clicked() {
            *cmd = Some(Cmd::Undo);
        }
        if ui.add_enabled(st.undo.redo_name().is_some(), egui::Button::new("Redo")).on_hover_text(st.undo.redo_name().map_or(String::new(), |n| format!("Redo {n}"))).clicked() {
            *cmd = Some(Cmd::Redo);
        }
    });
}

fn what_is_window(ctx: &egui::Context, st: &mut EditorState) {
    let mut open = st.what_is;
    egui::Window::new(ed::WHAT_IS).open(&mut open).collapsible(false).resizable(false).default_width(360.0).show(ctx, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for (text, bold) in ed::plain_runs(ed::WHAT_IS_TEXT) {
                ui.label(if bold { egui::RichText::new(text).strong() } else { egui::RichText::new(text) });
            }
        });
    });
    st.what_is = open;
}

fn detail(ui: &mut egui::Ui, model: &Model, profiles: &[(String, String)], keys: &mut KeysState, st: &mut EditorState, cmd: &mut Option<Cmd>) {
    let Some(w) = st.selected.as_deref().and_then(|id| model.get(id)) else {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.weak(ed::EMPTY_STATE);
            if ui.button(ed::WHAT_IS).clicked() {
                st.what_is = true;
            }
        });
        return;
    };
    if let Some(n) = &st.notice {
        ui.colored_label(egui::Color32::from_rgb(0xD0, 0x8A, 0x00), n.as_str());
    }
    if !model.source_of(&w.unique_identifier).is_person() {
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(egui::Color32::from_rgb(0xD0, 0x8A, 0x00), "This workgroup was imported. Termurion asks before it runs its commands.");
            if ui.button("Trust this workgroup").on_hover_text("You have read its commands and want them to run without asking.").clicked() {
                *cmd = Some(Cmd::Trust(w.unique_identifier.clone()));
            }
        });
    }
    ui.horizontal(|ui| {
        if let Some(name) = text_field(ui, st, &format!("{}:name", w.unique_identifier), &w.name, 128, ed::NAME_PLACEHOLDER, false) {
            let mut n = w.clone();
            n.name = name;
            *cmd = Some(Cmd::Edit { wg: n, action: ed::RENAME_WORKGROUP, coalesce: true });
        }
    });
    if ui.small_button("?").on_hover_text(ed::WHAT_IS).clicked() {
        st.what_is = true;
    }

    // The session tree.
    let rows = ed::tree_rows(w);
    if st.session.as_deref().is_none_or(|id| w.session(id).is_none()) {
        st.session = rows.first().map(|r| r.2.clone());
        st.item = None;
    }
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        for (depth, label, id) in &rows {
            ui.horizontal(|ui| {
                ui.add_space(14.0 * *depth as f32);
                let on = st.session.as_deref() == Some(id.as_str());
                if ui.selectable_label(on, label).clicked() && !on {
                    st.session = Some(id.clone());
                    st.item = None;
                    st.rec = None;
                }
            });
        }
    });
    let sel = st.session.clone().and_then(|id| w.session(&id));
    ui.horizontal(|ui| {
        let can = ed::can_add(w);
        ui.add_enabled_ui(sel.is_some() && can, |ui| {
            ui.menu_button("+ Add", |ui| {
                if let Some(s) = sel {
                    for (title, enabled) in ed::add_menu_in(w, s) {
                        if ui.add_enabled(enabled, egui::Button::new(title)).clicked() {
                            let kind = title.trim_start_matches("Add ").to_lowercase();
                            *cmd = Some(Cmd::Edit { wg: ed::add_child(w, &s.unique_identifier, &kind), action: ed::ADD_SESSION, coalesce: false });
                            ui.close();
                        }
                    }
                }
            });
        });
        let removable = sel.is_some_and(|s| s.kind != Kind::Root);
        if ui.add_enabled(removable, egui::Button::new("\u{2212}")).on_hover_text("Remove the session and everything under it").clicked()
            && let Some(s) = sel {
                *cmd = Some(Cmd::Edit { wg: ed::remove_session(w, &s.unique_identifier), action: ed::REMOVE_SESSION, coalesce: false });
            }
        if !can {
            ui.weak("A workgroup holds at most 32 sessions.");
        }
    });
    ui.separator();
    let Some(s) = sel else {
        ui.weak(ed::NO_SESSION);
        return;
    };
    session_detail(ui, w, s, profiles, keys, st, cmd);
}

fn session_detail(ui: &mut egui::Ui, w: &Workgroup, s: &SessionConfig, profiles: &[(String, String)], keys: &mut KeysState, st: &mut EditorState, cmd: &mut Option<Cmd>) {
    let sid = s.unique_identifier.clone();
    let browser = s.profile_guid.as_ref().is_some_and(|g| st.browser.contains(g));
    let fields = ed::session_fields_in(w, s, browser);
    let edit = |wg: Workgroup, action: &'static str, coalesce: bool| Cmd::Edit { wg, action, coalesce };
    egui::Grid::new(format!("wg-session-{sid}")).num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        for f in &fields {
            match f {
                ed::Field::Profile => {
                    ui.label("Profile:");
                    let shown = s.profile_guid.as_ref().and_then(|g| profiles.iter().find(|p| &p.0 == g)).map_or("Default", |p| p.1.as_str()).to_string();
                    egui::ComboBox::from_id_salt(format!("wg-profile-{sid}")).selected_text(shown).show_ui(ui, |ui| {
                        if ui.selectable_label(s.profile_guid.is_none(), "Default").clicked() {
                            *cmd = Some(edit(with_session(w, &sid, |s| s.profile_guid = None), ed::CHANGE_PROFILE, false));
                        }
                        for (guid, name) in profiles {
                            let label = if st.dynamic.contains(guid) { format!("{name} (dynamic)") } else { name.clone() };
                            if ui.selectable_label(s.profile_guid.as_ref() == Some(guid), label).clicked() {
                                *cmd = Some(edit(with_session(w, &sid, |s| s.profile_guid = Some(guid.clone())), ed::CHANGE_PROFILE, false));
                            }
                        }
                    });
                }
                ed::Field::Mode => {
                    ui.label("Mode:");
                    egui::ComboBox::from_id_salt(format!("wg-mode-{sid}")).selected_text(s.mode.title()).show_ui(ui, |ui| {
                        for m in [Mode::Regular, Mode::CodeReview, Mode::Diff] {
                            if ui.selectable_label(s.mode == m, m.title()).clicked() && s.mode != m {
                                *cmd = Some(edit(with_session(w, &sid, |s| s.mode = m), ed::CHANGE_MODE, false));
                            }
                        }
                    });
                }
                ed::Field::Command => {
                    ui.label("Command:");
                    if let Some(t) = text_field(ui, st, &format!("{sid}:command"), &s.command, 4096, "", true) {
                        *cmd = Some(edit(with_session(w, &sid, |s| s.command = t), ed::CHANGE_COMMAND, true));
                    }
                }
                ed::Field::FileCommand => {
                    ui.label("File command:");
                    if let Some(t) = text_field(ui, st, &format!("{sid}:filecommand"), &s.per_file_command, 4096, "", true) {
                        *cmd = Some(edit(with_session(w, &sid, |s| s.per_file_command = t), ed::CHANGE_FILE_COMMAND, true));
                    }
                }
                ed::Field::Url => {
                    ui.label("URL:");
                    if let Some(t) = text_field(ui, st, &format!("{sid}:url"), &s.url_string, 2048, "", false) {
                        *cmd = Some(edit(with_session(w, &sid, |s| s.url_string = t), ed::CHANGE_URL, true));
                    }
                }
                ed::Field::Name => {
                    ui.label("Name:");
                    let key = format!("{sid}:name");
                    if let Some(t) = text_field(ui, st, &key, &s.display_name, 128, "", false) {
                        *cmd = Some(edit(with_session(w, &sid, |s| s.display_name = t), ed::RENAME_PEER, true));
                    }
                    // A peer cannot end up unlabelled: a blank name becomes a fresh animal (`WSDV:1316-1340`).
                    let id = ui.id().with(&key);
                    if s.kind == Kind::Peer && s.display_name.trim().is_empty() && !ui.memory(|m| m.has_focus(id)) {
                        let taken: std::collections::BTreeSet<String> = w.sessions.iter().filter(|o| o.unique_identifier != sid && !o.display_name.is_empty()).map(|o| o.display_name.clone()).collect();
                        let name = ed::animal_name(&taken);
                        *cmd = Some(edit(with_session(w, &sid, |s| s.display_name = name), ed::RENAME_PEER, false));
                    }
                }
                ed::Field::Shortcut => {
                    ui.label("Shortcut:");
                    if recorder(ui, keys, st, Rec::Peer, s.peer_switch_shortcut.as_ref()) {
                        *cmd = Some(shortcut_cmd(w, &sid, Rec::Peer, None, None));
                    }
                }
                ed::Field::Split => {
                    ui.label("Split:");
                    if let Kind::Split(sp) = &s.kind {
                        ui.horizontal(|ui| {
                            for (o, name) in [(Orientation::Vertical, "Vertical"), (Orientation::Horizontal, "Horizontal")] {
                                if ui.selectable_label(sp.orientation == o, name).clicked() && sp.orientation != o {
                                    *cmd = Some(edit(with_session(w, &sid, |s| if let Kind::Split(x) = &mut s.kind { x.orientation = o }), ed::CHANGE_ORIENTATION, false));
                                }
                            }
                        });
                    }
                }
                ed::Field::Side => {
                    ui.label("Side:");
                    if let Kind::Split(sp) = &s.kind {
                        let (a, b) = if sp.orientation == Orientation::Vertical { ("Left", "Right") } else { ("Top", "Bottom") };
                        ui.horizontal(|ui| {
                            for (side, name) in [(Side::LeadingOrTop, a), (Side::TrailingOrBottom, b)] {
                                if ui.selectable_label(sp.side == side, name).clicked() && sp.side != side {
                                    *cmd = Some(edit(with_session(w, &sid, |s| if let Kind::Split(x) = &mut s.kind { x.side = side }), ed::CHANGE_SIDE, false));
                                }
                            }
                        });
                    }
                }
                ed::Field::Location => {
                    ui.label("Location:");
                    if let Kind::Split(sp) = &s.kind {
                        let mut loc = sp.location.clamp(0.1, 0.9);
                        ui.horizontal(|ui| {
                            if ui.add(egui::Slider::new(&mut loc, 0.1..=0.9).show_value(false)).changed() {
                                *cmd = Some(edit(with_session(w, &sid, |s| if let Kind::Split(x) = &mut s.kind { x.location = loc }), ed::CHANGE_SPLIT_LOCATION, true));
                            }
                            ui.label(format!("{}%", (loc * 100.0).round() as i64));
                        });
                    }
                }
                ed::Field::ToolbarItems => {
                    ui.label("Toolbar Items:");
                    ui.vertical(|ui| toolbar_items(ui, w, s, keys, st, cmd));
                }
            }
            ui.end_row();
        }
    });
}

fn toolbar_items(ui: &mut egui::Ui, w: &Workgroup, s: &SessionConfig, keys: &mut KeysState, st: &mut EditorState, cmd: &mut Option<Cmd>) {
    let sid = s.unique_identifier.clone();
    let edit = |wg: Workgroup, action: &'static str, coalesce: bool| Cmd::Edit { wg, action, coalesce };
    if st.item.is_some_and(|i| i >= s.toolbar_items.len()) {
        st.item = None;
    }
    let mut rects: Vec<egui::Rect> = Vec::new();
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_min_width(260.0);
        if s.toolbar_items.is_empty() {
            ui.weak("None");
        }
        for (i, item) in s.toolbar_items.iter().enumerate() {
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 20.0), egui::Sense::click_and_drag());
            let on = st.item == Some(i);
            if on {
                ui.painter().rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
            } else if resp.hovered() {
                ui.painter().rect_filled(rect, 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
            }
            ui.painter().text(rect.left_center() + egui::vec2(4.0, 0.0), egui::Align2::LEFT_CENTER, ed::item_label(item), egui::FontId::proportional(13.0), ui.visuals().text_color());
            if resp.clicked() || resp.drag_started() {
                st.item = Some(i);
                st.rec = None;
            }
            if resp.drag_started() {
                st.drag = Some(i);
            }
            rects.push(rect);
        }
    });
    // Drag to reorder: the row follows the pointer's slot on release (`WSDV:1385-1455`).
    if let Some(from) = st.drag {
        if let Some(p) = ui.input(|i| i.pointer.latest_pos()) {
            let to = rects.iter().position(|r| p.y < r.center().y).unwrap_or(rects.len()).min(rects.len());
            if let Some(r) = rects.get(to.min(rects.len().saturating_sub(1))) {
                let y = if to >= rects.len() { r.bottom() } else { r.top() };
                ui.painter().hline(r.x_range(), y, egui::Stroke::new(2.0, ui.visuals().selection.stroke.color));
            }
            if ui.input(|i| i.pointer.any_released()) {
                st.drag = None;
                let dest = if to > from { to - 1 } else { to };
                if dest != from && from < s.toolbar_items.len() {
                    *cmd = Some(edit(
                        with_session(w, &sid, |s| {
                            let it = s.toolbar_items.remove(from);
                            s.toolbar_items.insert(dest.min(s.toolbar_items.len()), it);
                        }),
                        ed::REORDER_ITEM,
                        false,
                    ));
                    st.item = Some(dest);
                }
            }
        } else {
            st.drag = None;
        }
        if !ui.input(|i| i.pointer.any_down()) {
            st.drag = None;
        }
    }
    ui.horizontal(|ui| {
        ui.menu_button("+", |ui| {
            for name in ed::offered_items(s) {
                if ui.button(name).clicked() {
                    if let Some(item) = ed::default_item(name) {
                        let at = st.item.map_or(s.toolbar_items.len(), |i| i + 1).min(s.toolbar_items.len());
                        *cmd = Some(edit(with_session(w, &sid, |s| s.toolbar_items.insert(at, item)), ed::ADD_ITEM, false));
                        st.item = Some(at);
                    }
                    ui.close();
                }
            }
        });
        let removable = st.item.is_some_and(|i| i < s.toolbar_items.len() && !ed::is_required_switcher(w, s, i));
        if ui.add_enabled(removable, egui::Button::new("\u{2212}")).on_hover_text("Remove the item").clicked()
            && let Some(i) = st.item {
                *cmd = Some(edit(with_session(w, &sid, |s| drop(s.toolbar_items.remove(i))), ed::REMOVE_ITEM, false));
                st.item = None;
            }
    });
    let Some(i) = st.item else { return };
    let Some(item) = s.toolbar_items.get(i) else { return };
    match item {
        ToolbarItem::Spacer { min_width, max_width } => {
            egui::Grid::new(format!("wg-spacer-{sid}-{i}")).num_columns(2).show(ui, |ui| {
                ui.label("Min width:");
                let a = number_field(ui, st, &format!("{sid}:{i}:min"), &ed::format_width(*min_width));
                ui.end_row();
                ui.label("Max width:");
                let b = number_field(ui, st, &format!("{sid}:{i}:max"), &ed::format_width(*max_width));
                ui.end_row();
                if a.is_some() || b.is_some() {
                    let lo = a.unwrap_or(*min_width);
                    let hi = b.unwrap_or(*max_width).max(lo);
                    *cmd = Some(edit(with_session(w, &sid, |s| s.toolbar_items[i] = ToolbarItem::Spacer { min_width: lo, max_width: hi }), ed::CHANGE_SPACER, false));
                }
            });
        }
        ToolbarItem::Navigation(n) => {
            egui::Grid::new(format!("wg-nav-{sid}-{i}")).num_columns(2).show(ui, |ui| {
                for (label, rec, cur) in [("Back:", Rec::Back, &n.back), ("Forward:", Rec::Forward, &n.forward), ("Reload:", Rec::NavReload, &n.reload)] {
                    ui.label(label);
                    if recorder(ui, keys, st, rec, cur.as_ref()) {
                        *cmd = Some(shortcut_cmd(w, &sid, rec, None, Some(i)));
                    }
                    ui.end_row();
                }
            });
        }
        ToolbarItem::Reload(r) => {
            egui::Grid::new(format!("wg-reload-{sid}-{i}")).num_columns(2).show(ui, |ui| {
                ui.label("Reload:");
                if recorder(ui, keys, st, Rec::ItemReload, r.as_ref()) {
                    *cmd = Some(shortcut_cmd(w, &sid, Rec::ItemReload, None, Some(i)));
                }
                ui.end_row();
            });
        }
        ToolbarItem::Usage { provider, command, interval_seconds } => {
            egui::Grid::new(format!("wg-usage-{sid}-{i}")).num_columns(2).show(ui, |ui| {
                ui.label("Provider:");
                egui::ComboBox::from_id_salt(format!("wg-usage-provider-{sid}-{i}")).selected_text(provider.display_name()).show_ui(ui, |ui| {
                    // One provider today; choosing it again changes nothing.
                    let _ = ui.selectable_label(true, provider.display_name());
                });
                ui.end_row();
                ui.label("Command:");
                let cmd_text = text_field(ui, st, &format!("{sid}:{i}:usagecmd"), command, 1024, "Default (bundled script)", true);
                ui.end_row();
                ui.label("Every (sec):");
                let every = number_field(ui, st, &format!("{sid}:{i}:every"), &ed::format_width(*interval_seconds));
                ui.end_row();
                if cmd_text.is_some() || every.is_some() {
                    let provider = *provider;
                    let typing = cmd_text.is_some();
                    let c = cmd_text.unwrap_or_else(|| command.clone());
                    let e = every.map_or(*interval_seconds, |v| v.max(ed::MIN_USAGE_INTERVAL));
                    *cmd = Some(edit(with_session(w, &sid, |s| s.toolbar_items[i] = ToolbarItem::Usage { provider, command: c, interval_seconds: e }), ed::CHANGE_USAGE, typing));
                }
            });
        }
        _ => {}
    }
}

/// Applies one command: the limits first, then the undo snapshot, then the write.
fn apply(cmd: Option<Cmd>, model: &mut Model, g: &mut Globals, st: &mut EditorState) -> bool {
    let Some(cmd) = cmd else { return false };
    let before = model.workgroups.clone();
    let snapshot = Snapshot { list: before.clone(), marks: model.marks() };
    let mut marks = snapshot.marks.clone();
    let mut list = before.clone();
    let (action, coalesce): (&'static str, bool) = match cmd {
        Cmd::AddWorkgroup => {
            let w = Workgroup::new_empty(ed::UNTITLED_WORKGROUP);
            st.selected = Some(w.unique_identifier.clone());
            st.session = None;
            marks.insert(w.unique_identifier.clone(), wg::Source::Person);
            list.push(w);
            (ed::CHANGE_WORKGROUPS, false)
        }
        Cmd::AddPreset(id) => {
            let Some(w) = wg::presets::build(&id) else { return false };
            st.selected = Some(w.unique_identifier.clone());
            st.session = None;
            marks.insert(w.unique_identifier.clone(), wg::Source::Person);
            list.push(w);
            (ed::CHANGE_WORKGROUPS, false)
        }
        Cmd::RemoveWorkgroup(id) => {
            list.retain(|w| w.unique_identifier != id);
            st.selected = None;
            (ed::CHANGE_WORKGROUPS, false)
        }
        Cmd::Edit { wg: new, action, coalesce } => {
            let Some(at) = list.iter().position(|w| w.unique_identifier == new.unique_identifier) else { return false };
            if action == ed::ADD_SESSION && new.sessions.len() > list[at].sessions.len() {
                st.session = new.sessions.last().map(|s| s.unique_identifier.clone());
                st.item = None;
            }
            if action == ed::REMOVE_SESSION {
                st.session = None;
            }
            list[at] = new;
            (action, coalesce)
        }
        Cmd::Trust(id) => {
            model.trust(&id);
            model.save(g);
            st.notice = None;
            return true;
        }
        Cmd::Undo => {
            let Some(back) = st.undo.undo(snapshot) else { return false };
            model.set_all_marked(back.list, &back.marks);
            model.save(g);
            st.notice = None;
            return true;
        }
        Cmd::Redo => {
            let Some(fwd) = st.undo.redo(snapshot) else { return false };
            model.set_all_marked(fwd.list, &fwd.marks);
            model.save(g);
            st.notice = None;
            return true;
        }
    };
    if list == before {
        return false;
    }
    // The write point drops or trims what breaks a limit: refuse the edit and say why.
    if let Some(bad) = list.iter().find(|w| !before.contains(w) && wg::check(w).is_err()) {
        st.notice = Some(match wg::check(bad) {
            Err(e) => format!("That change was not kept: {e}."),
            Ok(()) => String::new(),
        });
        return false;
    }
    st.notice = None;
    if coalesce {
        st.undo.push_coalesced(snapshot, action);
    } else {
        st.undo.push(snapshot, action);
    }
    model.set_all_marked(list, &marks);
    model.save(g);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn globals() -> Globals {
        Globals::empty(std::path::Path::new("workgroup-editor-test-settings.toml"))
    }

    fn edited(w: &Workgroup, f: impl FnOnce(&mut Workgroup)) -> Cmd {
        let mut n = w.clone();
        f(&mut n);
        Cmd::Edit { wg: n, action: ed::RENAME_WORKGROUP, coalesce: false }
    }

    #[test]
    fn every_edit_goes_through_the_one_write_point() {
        let mut g = globals();
        let mut st = EditorState::default();
        let mut model = Model::default();
        assert!(apply(Some(Cmd::AddWorkgroup), &mut model, &mut g, &mut st));
        assert_eq!(model.workgroups.len(), 1);
        assert_eq!(model.workgroups[0].name, "Untitled Workgroup");
        assert_eq!(wg::decode(&g.string(wg::KEY)).unwrap(), model.workgroups, "stored through Globals::set");
        // A rename is stored as typed; a command is stored as typed and never run.
        let w = model.workgroups[0].clone();
        assert!(apply(Some(edited(&w, |w| w.name = "Mine".into())), &mut model, &mut g, &mut st));
        let root = w.sessions[0].unique_identifier.clone();
        let w = model.workgroups[0].clone();
        assert!(apply(Some(Cmd::Edit { wg: with_session(&w, &root, |s| s.command = "rm -rf ~ & calc".into()), action: ed::CHANGE_COMMAND, coalesce: true }), &mut model, &mut g, &mut st));
        assert_eq!(wg::decode(&g.string(wg::KEY)).unwrap()[0].sessions[0].command, "rm -rf ~ & calc");
        // A control character in a command is refused: nothing is stored and undo does not grow.
        let depth = st.undo.stack.len();
        let w = model.workgroups[0].clone();
        assert!(!apply(Some(Cmd::Edit { wg: with_session(&w, &root, |s| s.command = "a\nb".into()), action: ed::CHANGE_COMMAND, coalesce: true }), &mut model, &mut g, &mut st));
        assert_eq!(st.undo.stack.len(), depth);
        assert!(st.notice.is_some());
        assert_eq!(model.workgroups[0].sessions[0].command, "rm -rf ~ & calc");
        // Undo and redo walk the snapshots.
        assert!(apply(Some(Cmd::Undo), &mut model, &mut g, &mut st));
        assert_eq!(model.workgroups[0].sessions[0].command, "");
        assert!(apply(Some(Cmd::Redo), &mut model, &mut g, &mut st));
        assert_eq!(model.workgroups[0].sessions[0].command, "rm -rf ~ & calc");
    }

    #[test]
    fn an_imported_workgroup_stays_imported_through_edits_until_trusted() {
        let mut g = globals();
        let mut st = EditorState::default();
        let mut model = Model::default();
        let w = Workgroup::new_empty("theirs");
        let id = w.unique_identifier.clone();
        model.add_from(w.clone(), wg::Source::Import);
        model.save(&mut g);
        assert!(apply(Some(edited(&w, |w| w.name = "renamed".into())), &mut model, &mut g, &mut st));
        assert_eq!(model.source_of(&id), wg::Source::Import);
        assert_eq!(Model::load(&g).0.source_of(&id), wg::Source::Import, "the mark is stored");
        assert!(apply(Some(Cmd::Undo), &mut model, &mut g, &mut st));
        assert_eq!(Model::load(&g).0.source_of(&id), wg::Source::Import, "undo does not launder it");
        assert!(apply(Some(Cmd::Trust(id.clone())), &mut model, &mut g, &mut st));
        assert_eq!(Model::load(&g).0.source_of(&id), wg::Source::Person);
        // A workgroup the editor makes is the person's.
        assert!(apply(Some(Cmd::AddPreset("codingAgentPlusDiff".into())), &mut model, &mut g, &mut st));
        let preset = model.workgroups.last().unwrap().unique_identifier.clone();
        assert_eq!(Model::load(&g).0.source_of(&preset), wg::Source::Person);
    }

    #[test]
    fn adding_and_removing_sessions_select_sensibly() {
        let mut g = globals();
        let mut st = EditorState::default();
        let mut model = Model::default();
        assert!(apply(Some(Cmd::AddWorkgroup), &mut model, &mut g, &mut st));
        let w = model.workgroups[0].clone();
        let root = w.sessions[0].unique_identifier.clone();
        assert!(apply(Some(Cmd::Edit { wg: ed::add_child(&w, &root, "peer"), action: ed::ADD_SESSION, coalesce: false }), &mut model, &mut g, &mut st));
        assert_eq!(st.session.as_deref(), Some(model.workgroups[0].sessions[1].unique_identifier.as_str()));
        assert_eq!(model.workgroups[0].sessions[1].display_name, "Fox");
        assert_eq!(st.undo.undo_name(), Some("Add Session"));
    }

    #[test]
    fn undo_and_redo_never_launder_an_import() {
        let mut g = globals();
        let mut st = EditorState::default();
        let mut model = Model::default();
        let w = Workgroup::new_empty("theirs");
        let id = w.unique_identifier.clone();
        model.add_from(w, wg::Source::Import);
        model.save(&mut g);
        // Delete, then Undo.
        assert!(apply(Some(Cmd::RemoveWorkgroup(id.clone())), &mut model, &mut g, &mut st));
        assert!(apply(Some(Cmd::Undo), &mut model, &mut g, &mut st));
        assert_eq!(model.source_of(&id), wg::Source::Import);
        assert_eq!(Model::load(&g).0.source_of(&id), wg::Source::Import, "stored marked");
        // Delete, Undo, Undo again past it, then Redo, across a reload of the model.
        assert!(apply(Some(Cmd::RemoveWorkgroup(id.clone())), &mut model, &mut g, &mut st));
        assert!(apply(Some(Cmd::Undo), &mut model, &mut g, &mut st));
        assert!(apply(Some(Cmd::Redo), &mut model, &mut g, &mut st));
        assert!(model.get(&id).is_none());
        model = Model::load(&g).0;
        assert!(apply(Some(Cmd::Undo), &mut model, &mut g, &mut st));
        assert_eq!(model.source_of(&id), wg::Source::Import, "a reloaded model: the snapshot carries the mark");
        assert_eq!(Model::load(&g).0.source_of(&id), wg::Source::Import);
        // The person's own workgroup, added and undone and redone, stays the person's.
        assert!(apply(Some(Cmd::AddWorkgroup), &mut model, &mut g, &mut st));
        let mine = model.workgroups.last().unwrap().unique_identifier.clone();
        assert!(apply(Some(Cmd::Undo), &mut model, &mut g, &mut st));
        assert!(apply(Some(Cmd::Redo), &mut model, &mut g, &mut st));
        assert_eq!(model.source_of(&mine), wg::Source::Person);
        // Only Trust makes it the person's.
        assert!(apply(Some(Cmd::Trust(id.clone())), &mut model, &mut g, &mut st));
        assert_eq!(model.source_of(&id), wg::Source::Person);
    }

    #[test]
    fn a_recorded_shortcut_is_kept() {
        let mut keys = KeysState::default();
        let mut st = EditorState::default();
        // The person clicked a recorder: the row is remembered and the page records.
        keys.hotkey_recording = Some(HotkeySlot::Workgroup);
        st.rec = Some(Rec::Peer);
        // Nothing delivered yet: the row stays while it records.
        assert!(recorded_key(&mut keys, &mut st).is_none());
        assert!(st.rec.is_some());
        // A key arrives: `deliver` ends the recording in the same step.
        keys.deliver(crate::keystroke::parse("0x72-0x100000-0xf").unwrap());
        assert_eq!(keys.hotkey_recording, None);
        let (rec, k) = recorded_key(&mut keys, &mut st).expect("the recorded key reaches the row");
        assert!(matches!(rec, Rec::Peer));
        assert_eq!(k.serialized(), "0x72-0x100000-0xf");
        assert!(keys.hotkey_recorded.is_none() && st.rec.is_none());
        // A key recorded for another slot is left alone.
        keys.hotkey_recorded = Some((HotkeySlot::Session, crate::keystroke::parse("0x72-0x100000-0xf").unwrap()));
        assert!(recorded_key(&mut keys, &mut st).is_none());
        assert!(keys.hotkey_recorded.is_some());
    }
}

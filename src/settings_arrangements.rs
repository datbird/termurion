//! Settings > Arrangements: the saved arrangements (the default marked), rename, delete,
//! Set Default, a preview, and Change Profile in Arrangement.

use crate::arrangement::{self, Value};
use crate::arrangements::{self, Store};
use crate::globals::Globals;
use crate::layout::{Axis, Tree};

/// The page's two sub-tabs (`PreferencePanel.xib:10244`, `:10343`).
pub fn tab_titles() -> [&'static str; 2] {
    ["Arrangements", "Workgroups"]
}

/// The page's own state between frames.
#[derive(Default)]
pub struct State {
    selected: Option<String>,
    rename: String,
    profile: Option<String>,
    /// The sub-tab shown: 0 Arrangements, 1 Workgroups.
    tab: usize,
    workgroups: crate::workgroup_editor_ui::EditorState,
}

fn load(g: &Globals) -> Store {
    Store::from_values(g.get(arrangements::ARRANGEMENTS_KEY).as_ref(), g.get(arrangements::DEFAULT_KEY).as_ref())
}

fn put(g: &mut Globals, s: &Store) {
    let (all, default) = s.to_values();
    g.set(arrangements::ARRANGEMENTS_KEY, all.unwrap_or_else(|| Value::Table(Default::default())));
    g.set(arrangements::DEFAULT_KEY, default.unwrap_or_else(|| Value::String(String::new())));
}

/// Draws a tab's panes as boxes inside `r`.
fn draw_tree(p: &egui::Painter, t: &Tree<Value>, r: egui::Rect, stroke: egui::Stroke) {
    match t {
        Tree::Leaf(_) => {
            p.rect_stroke(r.shrink(1.0), 2.0, stroke, egui::StrokeKind::Inside);
        }
        Tree::Split { axis, ratio, a, b } => {
            let (ra, rb) = match axis {
                Axis::Row => {
                    let x = r.left() + r.width() * ratio;
                    (egui::Rect::from_min_max(r.min, egui::pos2(x, r.bottom())), egui::Rect::from_min_max(egui::pos2(x, r.top()), r.max))
                }
                Axis::Column => {
                    let y = r.top() + r.height() * ratio;
                    (egui::Rect::from_min_max(r.min, egui::pos2(r.right(), y)), egui::Rect::from_min_max(egui::pos2(r.left(), y), r.max))
                }
            };
            draw_tree(p, a, ra, stroke);
            draw_tree(p, b, rb, stroke);
        }
    }
}

/// The page. `profiles` are (guid, name, values). True when the arrangements or the workgroups
/// changed.
pub fn page(ui: &mut egui::Ui, g: &mut Globals, profiles: &[(String, String, toml::Table)], keys: &mut crate::settings_keys::KeysState, st: &mut State) -> bool {
    ui.horizontal(|ui| {
        for (i, title) in tab_titles().iter().enumerate() {
            ui.selectable_value(&mut st.tab, i, *title);
        }
    });
    ui.separator();
    if st.tab == 1 {
        let list: Vec<(String, String)> = profiles.iter().map(|p| (p.0.clone(), p.1.clone())).collect();
        st.workgroups.browser = profiles.iter().filter(|p| p.2.get("Custom Command").and_then(|v| v.as_str()).is_some_and(crate::browser::is_browser)).map(|p| p.0.clone()).collect();
        st.workgroups.dynamic = profiles.iter().filter(|p| p.2.get(crate::dynamic_profiles::DYNAMIC_KEY).and_then(|v| v.as_bool()).unwrap_or(false)).map(|p| p.0.clone()).collect();
        return crate::workgroup_editor_ui::page(ui, g, &list, keys, &mut st.workgroups);
    }
    arrangements_page(ui, g, profiles, st)
}

fn arrangements_page(ui: &mut egui::Ui, g: &mut Globals, profiles: &[(String, String, toml::Table)], st: &mut State) -> bool {
    let mut store = load(g);
    let mut changed = false;
    ui.heading("Arrangements");
    let names: Vec<String> = store.names().iter().map(|s| s.to_string()).collect();
    if names.is_empty() {
        ui.label("No saved arrangements. Window > Arrangements > Save Window Arrangement saves one.");
        return false;
    }
    let default = store.default_name().map(str::to_string);
    egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
        for n in &names {
            let label = if Some(n) == default.as_ref() { format!("{n}    (default)") } else { n.clone() };
            if ui.selectable_label(st.selected.as_ref() == Some(n), label).clicked() {
                st.selected = Some(n.clone());
                st.rename = n.clone();
            }
        }
    });
    let Some(sel) = st.selected.clone().filter(|s| names.contains(s)) else { return false };
    ui.horizontal(|ui| {
        if ui.button("\u{2212}").on_hover_text("Delete").clicked() {
            store.delete(&sel);
            st.selected = None;
            changed = true;
        }
        if ui.button("Set Default").clicked() {
            store.set_default(&sel);
            changed = true;
        }
        crate::settings_keys::field(ui, egui::TextEdit::singleline(&mut st.rename).desired_width(220.0));
        if ui.button("Rename").clicked() && st.rename != sel
            && store.rename(&sel, &st.rename) {
                st.selected = Some(arrangements::clean_name(&st.rename));
                changed = true;
            }
    });
    ui.horizontal(|ui| {
        let shown = st.profile.as_ref().and_then(|g| profiles.iter().find(|p| &p.0 == g)).map_or("Choose a profile", |p| p.1.as_str()).to_string();
        egui::ComboBox::from_id_salt("arr-profile").selected_text(shown).show_ui(ui, |ui| {
            for (guid, name, _) in profiles {
                if ui.selectable_label(st.profile.as_ref() == Some(guid), name).clicked() {
                    st.profile = Some(guid.clone());
                }
            }
        });
        let pick = st.profile.as_ref().and_then(|g| profiles.iter().find(|p| &p.0 == g));
        if ui.add_enabled(pick.is_some(), egui::Button::new("Change Profile in Arrangement")).clicked()
            && let (Some(p), Some(windows)) = (pick, store.get(&sel).cloned()) {
                let mut windows = windows;
                arrangement::change_profile(&mut windows, &p.2);
                store.replace(&sel, windows);
                changed = true;
            }
    });
    // A preview: each window as a box, with its selected tab's panes.
    if let Some(windows) = store.get(&sel) {
        let (resp, painter) = ui.allocate_painter(egui::vec2(ui.available_width(), 160.0), egui::Sense::hover());
        let area = resp.rect;
        let stroke = egui::Stroke::new(1.0, ui.visuals().text_color());
        let n = windows.len().max(1) as f32;
        for (i, w) in windows.iter().enumerate() {
            let cell = egui::Rect::from_min_size(egui::pos2(area.left() + i as f32 * area.width() / n, area.top()), egui::vec2(area.width() / n - 8.0, area.height()));
            painter.rect_stroke(cell, 3.0, stroke, egui::StrokeKind::Inside);
            let tabs = w.get("Tabs").and_then(|t| t.as_array()).cloned().unwrap_or_default();
            painter.text(cell.left_top() + egui::vec2(4.0, 2.0), egui::Align2::LEFT_TOP, format!("{} tab(s)", tabs.len()), egui::FontId::proportional(11.0), ui.visuals().text_color());
            let selected = arrangement::window_facts(w).selected_tab.min(tabs.len().saturating_sub(1));
            if let Some(tree) = tabs.get(selected).and_then(|t| t.get("Root")).and_then(|r| arrangement::tree_of(r).ok()) {
                draw_tree(&painter, &tree, cell.shrink2(egui::vec2(4.0, 16.0)).translate(egui::vec2(0.0, 8.0)), stroke);
            }
            // A window with its toolbelt: a white strip, 10 percent of the width, framed in
            // black at the right (`PT:3525-3534`).
            if crate::toolbelt::read_arrangement(w).has {
                let tw = (cell.width() * 0.1).floor();
                let strip = egui::Rect::from_min_size(egui::pos2(cell.right() - tw, cell.top()), egui::vec2((tw - 1.0).max(1.0), cell.height()));
                painter.rect_filled(strip, 0.0, egui::Color32::WHITE);
                painter.rect_stroke(strip, 0.0, egui::Stroke::new(1.0, egui::Color32::BLACK), egui::StrokeKind::Inside);
            }
        }
    }
    if changed {
        put(g, &store);
    }
    changed
}

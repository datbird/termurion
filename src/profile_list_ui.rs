//! The profile list: a search field, a "Tags >" button, the tag sidebar and the rows, shared by
//! the Settings window and the Profiles window (`ProfileListView.m`, `ProfileTagsView.m`).

use crate::profile_list::{arrow, follow_filter, click_selection, click_tags, display_text, filter, tag_query, TagTreeCache};

/// The list's state; the window that owns the list keeps it.
#[derive(Default)]
pub struct ListState {
    pub query: String,
    pub tags_open: bool,
    pub selected_tags: Vec<String>,
    pub selection: Vec<String>,
    /// The sidebar's tree, rebuilt only when the tags change.
    pub tree: TagTreeCache,
}

/// What the person did in the list this frame.
#[derive(Clone, Debug, PartialEq)]
pub enum ListEvent {
    None,
    /// A row was clicked (the clicked guid; `selection` holds the whole selection).
    Picked(String),
    /// A row was double-clicked.
    Activated(String),
    /// A row was right-clicked: the selection the menu acts on.
    Context(Vec<String>),
    /// A context menu item was chosen (its label); the selection holds what it acts on.
    MenuItem(&'static str),
}

/// The list's height.
const LIST_HEIGHT: f32 = 300.0;
/// The tag sidebar's width.
const TAGS_WIDTH: f32 = 110.0;

/// `rows` are (guid, name, tags, command). A star marks the default profile.
/// `menu` gives the context menu's items for a selection of that many rows (None: no menu).
pub fn profile_list(ui: &mut egui::Ui, st: &mut ListState, rows: &[(String, String, Vec<String>, String)], default_guid: &str, multi: bool, menu: Option<&dyn Fn(usize) -> Vec<&'static str>>) -> ListEvent {
    let mut event = ListEvent::None;
    let focus_id = ui.id().with("profile-list-search");
    let mut field_focused = false;
    let mut enter_pressed = false;
    ui.horizontal(|ui| {
        let r = ui.add(egui::TextEdit::singleline(&mut st.query).id(focus_id).hint_text("Search").desired_width((ui.available_width() - 70.0).max(40.0)));
        // The search field has focus when the list opens (iTerm2's first responder).
        let first = ui.data_mut(|d| d.get_temp::<bool>(focus_id).is_none());
        if first {
            ui.data_mut(|d| d.insert_temp(focus_id, true));
            r.request_focus();
        }
        field_focused = r.has_focus();
        // Enter in the search field opens the selection (`PLV:980-990`): with several rows it needs a selected one.
        enter_pressed = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if enter_pressed {
            r.request_focus();
        }
        if ui.button(if st.tags_open { "< Tags" } else { "Tags >" }).clicked() {
            st.tags_open = !st.tags_open;
        }
    });
    let shown_idx = filter(rows, &st.query);
    let mut shown_idx = shown_idx;
    let mut shown: Vec<String> = shown_idx.iter().map(|&i| rows[i].0.clone()).collect();
    // Esc clears a typed query (`PLV:972-979`); the arrow keys move the selection from the field
    // (`PLV:188`).
    if field_focused {
        if !st.query.is_empty() && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            st.query.clear();
            st.selected_tags.clear();
            shown_idx = filter(rows, &st.query);
            shown = shown_idx.iter().map(|&i| rows[i].0.clone()).collect();
        }
        for (key, down) in [(egui::Key::ArrowDown, true), (egui::Key::ArrowUp, false)] {
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key)) {
                st.selection = arrow(&st.selection, &shown, down);
                if let Some(g) = st.selection.first() {
                    event = ListEvent::Picked(g.clone());
                }
            }
        }
    }
    // The selection follows the filter (`updateResultsForSearch`, `PLV:1004-1012`).
    let followed = follow_filter(&st.selection, &shown);
    if followed != st.selection {
        st.selection = followed;
        if let Some(g) = st.selection.first() {
            event = ListEvent::Picked(g.clone());
        }
    }
    if enter_pressed && (shown.len() == 1 || !st.selection.is_empty()) {
        if let Some(g) = st.selection.first() {
            event = ListEvent::Activated(g.clone());
        }
    }
    let mods = ui.input(|i| i.modifiers);
    ui.horizontal_top(|ui| {
        if st.tags_open {
            ui.allocate_ui(egui::vec2(TAGS_WIDTH, LIST_HEIGHT), |ui| {
                ui.set_width(TAGS_WIDTH);
                ui.label(egui::RichText::new("Tag Name").strong());
                egui::ScrollArea::vertical().id_salt("profile-tags").max_height(LIST_HEIGHT - 20.0).show(ui, |ui| {
                    // The tree is built when some tag changes, not every frame; a click is applied
                    // after the loop, which borrows the tree.
                    let mut clicked = None;
                    for (label, path) in st.tree.for_profiles(rows.iter().map(|r| &r.2)) {
                        if ui.selectable_label(st.selected_tags.contains(path), label).clicked() {
                            clicked = Some(path.clone());
                        }
                    }
                    if let Some(path) = clicked {
                        st.selected_tags = click_tags(&st.selected_tags, &path, mods.ctrl || mods.command);
                        st.query = tag_query(&st.selected_tags);
                    }
                });
            });
            ui.separator();
        }
        egui::ScrollArea::vertical().id_salt("profile-rows").max_height(LIST_HEIGHT).auto_shrink([false, false]).show(ui, |ui| {
            let width = ui.available_width();
            for &i in &shown_idx {
                let (guid, name, tags, _) = &rows[i];
                let star = if guid == default_guid { "\u{2605} " } else { "" };
                let selected = st.selection.contains(guid);
                // The tags sit on a second, smaller line under the name (`PLV:657-671`).
                let font = egui::TextStyle::Body.resolve(ui.style());
                let small = egui::FontId::new(font.size * 0.8, font.family.clone());
                let line = ui.text_style_height(&egui::TextStyle::Body);
                let joined = tags.iter().map(|t| display_text(t)).collect::<Vec<_>>().join(", ");
                let height = if joined.is_empty() { line + 4.0 } else { line * 1.8 + 4.0 };
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
                if selected {
                    ui.painter().rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
                } else if resp.hovered() {
                    ui.painter().rect_filled(rect, 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
                }
                let text = if selected { ui.visuals().selection.stroke.color } else { ui.visuals().text_color() };
                let name_y = if joined.is_empty() { rect.center().y } else { rect.top() + 2.0 + line * 0.5 };
                ui.painter().with_clip_rect(rect).text(egui::pos2(rect.left() + 4.0, name_y), egui::Align2::LEFT_CENTER, format!("{star}{}", display_text(name)), font, text);
                if !joined.is_empty() {
                    let dim = if selected { text } else { ui.visuals().weak_text_color() };
                    ui.painter().with_clip_rect(rect).text(egui::pos2(rect.left() + 4.0, rect.top() + 2.0 + line * 1.4), egui::Align2::LEFT_CENTER, joined, small, dim);
                }
                if resp.clicked() {
                    st.selection = click_selection(&st.selection, &shown, guid, mods.ctrl || mods.command, mods.shift, multi);
                    event = ListEvent::Picked(guid.clone());
                }
                if resp.double_clicked() {
                    event = ListEvent::Activated(guid.clone());
                }
                if resp.secondary_clicked() {
                    if !selected {
                        st.selection = vec![guid.clone()];
                    }
                    event = ListEvent::Context(st.selection.clone());
                }
                if let Some(menu) = menu {
                    let count = st.selection.len();
                    resp.context_menu(|ui| {
                        for label in menu(count) {
                            if ui.button(label).clicked() {
                                event = ListEvent::MenuItem(label);
                                ui.close();
                            }
                        }
                    });
                }
            }
        });
    });
    event
}

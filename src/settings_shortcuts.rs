//! Settings > Shortcuts: iTerm2's Actions and Snippets tabs, their editors, and the
//! `.it2actions` and `.it2snippets` files.

use crate::globals::Globals;
use crate::keyaction::{Binding, Escaping};
use crate::snippets::{self, Snippet, Store};
use crate::user_actions::{self, UserAction};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Actions,
    Snippets,
}

/// What an edit on the page changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Changed {
    None,
    Actions,
    Snippets,
}

/// The Edit Snippet sheet: which snippet (None: a new one at `insert_at`) and its fields.
struct SnippetEditor {
    index: Option<usize>,
    insert_at: usize,
    snippet: Snippet,
    tags: String,
    help: bool,
}

struct ActionEditor {
    index: Option<usize>,
    binding: Binding,
    title: String,
}

/// The page's own state between frames.
#[derive(Default)]
pub struct State {
    tab: Tab,
    search: String,
    snippets_selected: Vec<usize>,
    actions_selected: Vec<usize>,
    snippet_editor: Option<SnippetEditor>,
    action_editor: Option<ActionEditor>,
    drag: Option<usize>,
    /// An alert: (heading, text).
    alert: Option<(String, String)>,
    search_help: bool,
}

/// The snippet editor's escaping choices, with iTerm2's names.
const SNIPPET_ESCAPINGS: [(Escaping, &str); 3] = [(Escaping::None, "Unescaped Literal Text"), (Escaping::Compatibility, "Backward Compatibility Escaping"), (Escaping::Common, "C-Style Backslash Escaping")];

const ESCAPING_HELP: &str = "C-Style Backslash Escaping supports: \\a (bell), \\b (backspace), \\e (escape), \\n (newline), \\r (carriage return), \\t (tab), \\\\ (backslash), and \\x followed by two hex digits giving a single byte of UTF-8.\n\nUnescaped Literal Text does not have any special characters.\n\nBackward Compatibility Escaping, which is not recommended for new snippets, supports: \\n (newline), \\e (escape), \\a (bell), and \\t (tab).";

const SEARCH_HELP: &str = "Terms in the search query are used to prefix search snippet titles, content, and tags.\n\nYou can use the following operators to restrict what attributes of snippets a term searches:\n * tag: to search only tags.\n * title: to search only snippet titles.\n * text: to search only the text of a snippet.\nFor example, tag:linux.\n\nTo search only for snippets that do not match a term, use the - operator. For example, -linux or -tag:linux.\n\nYou can use | as logical OR. For example, linux|bsd or tag:linux|tag:bsd.";

fn clipboard_text() -> Option<String> {
    arboard::Clipboard::new().and_then(|mut c| c.get_text()).ok()
}

/// Rows picked with a click: Ctrl adds or removes one, a plain click picks one.
fn pick(selected: &mut Vec<usize>, row: usize, ctrl: bool) {
    if ctrl {
        match selected.iter().position(|&r| r == row) {
            Some(i) => {
                selected.remove(i);
            }
            None => selected.push(row),
        }
    } else {
        *selected = vec![row];
    }
}

fn read_file(path: &std::path::Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("While loading {}: {e}", path.display()))
}

/// The page. `profiles` are (guid, name) for the profile actions.
pub fn page(ui: &mut egui::Ui, g: &mut Globals, store: &mut Store, st: &mut State, profiles: &[(String, String)]) -> Changed {
    ui.heading("Shortcuts");
    ui.horizontal(|ui| {
        ui.selectable_value(&mut st.tab, Tab::Actions, "Actions");
        ui.selectable_value(&mut st.tab, Tab::Snippets, "Snippets");
    });
    ui.separator();
    let mut changed = match st.tab {
        Tab::Actions => actions_tab(ui, g, st, profiles),
        Tab::Snippets => snippets_tab(ui, store, st),
    };
    let ctx = ui.ctx().clone();
    if let Some(e) = &mut st.snippet_editor {
        match snippet_sheet(&ctx, e) {
            Some(true) => {
                let e = st.snippet_editor.take().unwrap();
                let mut s = e.snippet;
                s.tags = e.tags.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect();
                s.version = snippets::VERSION;
                match e.index {
                    Some(i) if i < store.snippets.len() => store.snippets[i] = s,
                    _ => {
                        let at = e.insert_at.min(store.snippets.len());
                        store.snippets.insert(at, s);
                        st.snippets_selected = vec![at];
                    }
                }
                changed = Changed::Snippets;
            }
            Some(false) => st.snippet_editor = None,
            None => {}
        }
    }
    if let Some(e) = &mut st.action_editor {
        match action_sheet(&ctx, e, profiles) {
            Some(true) => {
                let e = st.action_editor.take().unwrap();
                let mut list = user_actions::load(g.get("Actions").as_ref());
                let escaping = e.binding.escaping();
                let a = UserAction { action: e.binding.action, title: e.title, parameter: e.binding.text, escaping, apply_mode: e.binding.apply_mode, version: user_actions::VERSION };
                match e.index {
                    Some(i) if i < list.len() => list[i] = a,
                    _ => list.push(a),
                }
                g.set("Actions", user_actions::store(&list));
                changed = Changed::Actions;
            }
            Some(false) => st.action_editor = None,
            None => {}
        }
    }
    if let Some((heading, text)) = st.alert.clone() {
        egui::Window::new(heading).collapsible(false).resizable(false).show(&ctx, |ui| {
            ui.label(text);
            if ui.button("OK").clicked() {
                st.alert = None;
            }
        });
    }
    for (open, title, text) in [(&mut st.search_help, "Search", SEARCH_HELP)] {
        if *open {
            egui::Window::new(title).collapsible(false).open(open).show(&ctx, |ui| {
                ui.label(text);
            });
        }
    }
    changed
}

fn tag_pill(ui: &egui::Ui, painter: &egui::Painter, at: egui::Pos2, tag: &str) -> f32 {
    let font = egui::FontId::proportional(10.5);
    let galley = painter.layout_no_wrap(tag.to_string(), font, egui::Color32::WHITE);
    let size = galley.size() + egui::vec2(8.0, 2.0);
    let rect = egui::Rect::from_min_size(at + egui::vec2(0.0, (ui.spacing().interact_size.y - size.y) / 2.0), size);
    painter.rect_filled(rect, 4.0, egui::Color32::from_rgb(247, 120, 26));
    painter.galley(rect.min + egui::vec2(4.0, 1.0), galley, egui::Color32::WHITE);
    size.x + 4.0
}

fn snippets_tab(ui: &mut egui::Ui, store: &mut Store, st: &mut State) -> Changed {
    let mut changed = Changed::None;
    ui.horizontal(|ui| {
        ui.label("Search:");
        ui.add(egui::TextEdit::singleline(&mut st.search).desired_width(300.0));
        if ui.button("?").on_hover_text("Search help").clicked() {
            st.search_help = true;
        }
    });
    let shown = snippets::search(&store.snippets, &st.search, &[]);
    st.snippets_selected.retain(|r| shown.contains(r));
    let ctrl = ui.input(|i| i.modifiers.command);
    let mut rects: Vec<(usize, egui::Rect)> = Vec::new();
    let mut edit_row = None;
    let mut menu: Option<(&'static str, usize)> = None;
    egui::ScrollArea::vertical().max_height(320.0).id_salt("snippets-table").show(ui, |ui| {
        let w = ui.available_width();
        ui.horizontal(|ui| {
            ui.add_sized([w * 0.45, 16.0], egui::Label::new(egui::RichText::new("Title").strong()));
            ui.label(egui::RichText::new("Text").strong());
        });
        for &i in &shown {
            let s = &store.snippets[i];
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, ui.spacing().interact_size.y), egui::Sense::click_and_drag());
            let painter = ui.painter_at(rect);
            if st.snippets_selected.contains(&i) {
                painter.rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
            } else if resp.hovered() {
                painter.rect_filled(rect, 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
            }
            let color = ui.visuals().text_color();
            let mut x = rect.left() + 4.0;
            for t in &s.tags {
                x += tag_pill(ui, &painter, egui::pos2(x, rect.top()), t);
            }
            painter.text(egui::pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, &s.title, egui::FontId::proportional(13.0), color);
            painter.text(egui::pos2(rect.left() + w * 0.45 + 8.0, rect.center().y), egui::Align2::LEFT_CENTER, snippets::ellipsize(&s.value, 60), egui::FontId::monospace(12.0), color);
            if resp.clicked() {
                pick(&mut st.snippets_selected, i, ctrl);
            }
            if resp.double_clicked() {
                edit_row = Some(i);
            }
            // Reordering, only when every snippet shows (a filtered list has no true place).
            if resp.drag_started() && st.search.is_empty() {
                st.drag = Some(i);
            }
            resp.context_menu(|ui| {
                for label in ["Duplicate", "Delete", "Add Above", "Add Below", "Edit"] {
                    if ui.button(label).clicked() {
                        menu = Some((label, i));
                        ui.close();
                    }
                }
            });
            rects.push((i, rect));
        }
    });
    if let Some(from) = st.drag {
        if let Some(p) = ui.ctx().pointer_interact_pos() {
            if let Some(&(_, r)) = rects.iter().find(|(_, r)| r.y_range().contains(p.y)) {
                ui.painter().hline(r.x_range(), if p.y < r.center().y { r.top() } else { r.bottom() }, egui::Stroke::new(2.0, ui.visuals().selection.stroke.color));
            }
        }
        if ui.input(|i| i.pointer.any_released()) {
            st.drag = None;
            let to = ui.ctx().pointer_interact_pos().and_then(|p| rects.iter().find(|(_, r)| r.y_range().contains(p.y)).map(|&(i, r)| if p.y < r.center().y { i } else { i + 1 }));
            if let Some(to) = to {
                let s = store.snippets.remove(from);
                let at = if to > from { to - 1 } else { to }.min(store.snippets.len());
                store.snippets.insert(at, s);
                st.snippets_selected = vec![at];
                if at != from {
                    changed = Changed::Snippets;
                }
            }
        }
    }
    let one = (st.snippets_selected.len() == 1).then(|| st.snippets_selected[0]);
    ui.horizontal(|ui| {
        if ui.add_enabled(!st.snippets_selected.is_empty(), egui::Button::new("\u{2212}")).on_hover_text("Remove").clicked() {
            menu = Some(("Delete", usize::MAX));
        }
        if ui.button("+").on_hover_text("Add").clicked() {
            menu = Some(("Add", usize::MAX));
        }
        if ui.add_enabled(one.is_some(), egui::Button::new("Edit")).clicked() {
            edit_row = one;
        }
        if ui.add_enabled(!st.snippets_selected.is_empty(), egui::Button::new("Export...")).clicked() {
            let picked: Vec<&Snippet> = shown.iter().filter(|i| st.snippets_selected.contains(i)).map(|&i| &store.snippets[i]).collect();
            if let Some(path) = rfd::FileDialog::new().add_filter("Snippets", &["it2snippets"]).set_file_name("snippets.it2snippets").save_file() {
                if let Err(e) = std::fs::write(&path, snippets::export(&picked)) {
                    st.alert = Some(("Export Failed".into(), format!("Error saving to {}: {e}", path.display())));
                }
            }
        }
        if ui.button("Import...").clicked() {
            for path in rfd::FileDialog::new().add_filter("Snippets", &["it2snippets"]).pick_files().unwrap_or_default() {
                let parsed = read_file(&path).and_then(|t| match serde_json::from_str::<serde_json::Value>(&t) {
                    Err(e) => Err(format!("While parsing {}: {e}", path.display())),
                    Ok(_) => snippets::import(&t).map_err(|_| format!("Malformed file at {}", path.display())),
                });
                match parsed {
                    Ok(list) => {
                        store.snippets.extend(list);
                        changed = Changed::Snippets;
                    }
                    Err(e) => st.alert = Some(("Import Failed".into(), e)),
                }
            }
        }
    });
    let no_typing = !ui.ctx().egui_wants_keyboard_input();
    if no_typing && st.snippet_editor.is_none() && ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)) && !st.snippets_selected.is_empty() {
        menu = Some(("Delete", usize::MAX));
    }
    if let Some(i) = edit_row {
        let s = store.snippets[i].clone();
        st.snippet_editor = Some(SnippetEditor { index: Some(i), insert_at: i, tags: s.tags.join(", "), snippet: s, help: false });
    }
    match menu {
        Some(("Duplicate", row)) => {
            let rows = if st.snippets_selected.contains(&row) { st.snippets_selected.clone() } else { vec![row] };
            let mut rows = rows;
            rows.sort_unstable();
            let at = rows.last().map_or(0, |r| r + 1);
            let copies: Vec<Snippet> = rows.iter().map(|&r| Snippet { guid: uuid::Uuid::new_v4().to_string().to_uppercase(), version: snippets::VERSION, ..store.snippets[r].clone() }).collect();
            let n = copies.len();
            for (k, c) in copies.into_iter().enumerate() {
                store.snippets.insert(at + k, c);
            }
            st.snippets_selected = (at..at + n).collect();
            changed = Changed::Snippets;
        }
        Some(("Delete", row)) => {
            let mut rows = if row == usize::MAX || st.snippets_selected.contains(&row) { st.snippets_selected.clone() } else { vec![row] };
            rows.sort_unstable();
            for r in rows.into_iter().rev() {
                if r < store.snippets.len() {
                    store.snippets.remove(r);
                }
            }
            st.snippets_selected.clear();
            changed = Changed::Snippets;
        }
        Some((label @ ("Add Above" | "Add Below" | "Add"), row)) => {
            let insert_at = match label {
                "Add Above" => row,
                "Add Below" => row + 1,
                _ => store.snippets.len(),
            };
            st.snippet_editor = Some(SnippetEditor { index: None, insert_at, snippet: snippets::new_snippet(clipboard_text().as_deref()), tags: String::new(), help: false });
        }
        Some(("Edit", row)) => {
            let s = store.snippets[row].clone();
            st.snippet_editor = Some(SnippetEditor { index: Some(row), insert_at: row, tags: s.tags.join(", "), snippet: s, help: false });
        }
        _ => {}
    }
    changed
}

/// The Edit Snippet sheet: Some(true) for OK, Some(false) for Cancel.
fn snippet_sheet(ctx: &egui::Context, e: &mut SnippetEditor) -> Option<bool> {
    let mut out = None;
    egui::Window::new("Edit Snippet").collapsible(false).resizable(true).default_width(520.0).show(ctx, |ui| {
        egui::Grid::new("snippet-grid").num_columns(2).show(ui, |ui| {
            ui.label("Title:");
            ui.add(egui::TextEdit::singleline(&mut e.snippet.title).desired_width(400.0));
            ui.end_row();
            ui.label("Tags:");
            ui.add(egui::TextEdit::singleline(&mut e.tags).desired_width(400.0).hint_text("Comma-separated tags"));
            ui.end_row();
            ui.label("");
            ui.label(egui::RichText::new("Use / in tag names to create folders in Edit > Snippets menu").small());
            ui.end_row();
            ui.label("Value:");
            ui.add(egui::TextEdit::multiline(&mut e.snippet.value).desired_width(400.0).desired_rows(8).code_editor());
            ui.end_row();
        });
        ui.horizontal(|ui| {
            let now = e.snippet.escaping;
            let shown = SNIPPET_ESCAPINGS.iter().find(|(x, _)| *x == now).map_or("Vim Escaping", |(_, n)| n);
            egui::ComboBox::from_id_salt("snippet-escaping").width(260.0).selected_text(shown).show_ui(ui, |ui| {
                for (x, name) in SNIPPET_ESCAPINGS {
                    if ui.selectable_label(x == now, name).clicked() {
                        e.snippet.escaping = x;
                    }
                }
            });
            if ui.button("?").clicked() {
                e.help = !e.help;
            }
        });
        if e.help {
            ui.label(ESCAPING_HELP);
        }
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("OK").clicked() {
                out = Some(true);
            }
            if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                out = Some(false);
            }
        });
    });
    out
}

fn actions_tab(ui: &mut egui::Ui, g: &mut Globals, st: &mut State, _profiles: &[(String, String)]) -> Changed {
    let mut list = user_actions::load(g.get("Actions").as_ref());
    let mut changed = Changed::None;
    st.actions_selected.retain(|&r| r < list.len());
    let ctrl = ui.input(|i| i.modifiers.command);
    let mut edit_row = None;
    egui::ScrollArea::vertical().max_height(320.0).id_salt("actions-table").show(ui, |ui| {
        let w = ui.available_width();
        ui.horizontal(|ui| {
            ui.add_sized([w * 0.4, 16.0], egui::Label::new(egui::RichText::new("Title").strong()));
            ui.label(egui::RichText::new("Action").strong());
        });
        for (i, a) in list.iter().enumerate() {
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, ui.spacing().interact_size.y), egui::Sense::click());
            let painter = ui.painter_at(rect);
            if st.actions_selected.contains(&i) {
                painter.rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
            } else if resp.hovered() {
                painter.rect_filled(rect, 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
            }
            let color = ui.visuals().text_color();
            painter.text(egui::pos2(rect.left() + 4.0, rect.center().y), egui::Align2::LEFT_CENTER, &a.title, egui::FontId::proportional(13.0), color);
            painter.text(egui::pos2(rect.left() + w * 0.4 + 8.0, rect.center().y), egui::Align2::LEFT_CENTER, crate::settings_keys::action_summary(&a.binding()), egui::FontId::proportional(13.0), color);
            if resp.clicked() {
                pick(&mut st.actions_selected, i, ctrl);
            }
            if resp.double_clicked() {
                edit_row = Some(i);
            }
        }
    });
    let mut remove = false;
    let one = (st.actions_selected.len() == 1).then(|| st.actions_selected[0]);
    ui.horizontal(|ui| {
        remove = ui.add_enabled(!st.actions_selected.is_empty(), egui::Button::new("\u{2212}")).on_hover_text("Remove").clicked();
        if ui.button("+").on_hover_text("Add").clicked() {
            let b = Binding { action: 12, text: String::new(), label: String::new(), version: user_actions::VERSION, escaping: Some(Escaping::Common.code()), apply_mode: 0 };
            st.action_editor = Some(ActionEditor { index: None, binding: b, title: "Untitled Action".into() });
        }
        if ui.add_enabled(one.is_some(), egui::Button::new("Edit")).clicked() {
            edit_row = one;
        }
        if ui.button("Import...").clicked() {
            for path in rfd::FileDialog::new().add_filter("Actions", &["it2actions"]).pick_files().unwrap_or_default() {
                let parsed = read_file(&path).and_then(|t| match serde_json::from_str::<serde_json::Value>(&t) {
                    Err(e) => Err(format!("While parsing {}: {e}", path.display())),
                    Ok(_) => user_actions::import(&t).map_err(|_| format!("Malformed file at {}", path.display())),
                });
                match parsed {
                    Ok(more) => {
                        list.extend(more);
                        g.set("Actions", user_actions::store(&list));
                        changed = Changed::Actions;
                    }
                    Err(e) => st.alert = Some(("Import Failed".into(), e)),
                }
            }
        }
        if ui.add_enabled(!st.actions_selected.is_empty(), egui::Button::new("Export...")).clicked() {
            let picked: Vec<&UserAction> = (0..list.len()).filter(|i| st.actions_selected.contains(i)).map(|i| &list[i]).collect();
            if let Some(path) = rfd::FileDialog::new().add_filter("Actions", &["it2actions"]).set_file_name("actions.it2actions").save_file() {
                if let Err(e) = std::fs::write(&path, user_actions::export(&picked)) {
                    st.alert = Some(("Export Failed".into(), format!("Error saving to {}: {e}", path.display())));
                }
            }
        }
    });
    let no_typing = !ui.ctx().egui_wants_keyboard_input();
    if no_typing && st.action_editor.is_none() && ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)) && !st.actions_selected.is_empty() {
        remove = true;
    }
    if remove {
        let mut rows = std::mem::take(&mut st.actions_selected);
        rows.sort_unstable();
        for r in rows.into_iter().rev() {
            list.remove(r);
        }
        g.set("Actions", user_actions::store(&list));
        changed = Changed::Actions;
    }
    if let Some(i) = edit_row {
        let a = &list[i];
        // The action keeps its own escaping (iTerm2's editor resets it to C-style).
        st.action_editor = Some(ActionEditor { index: Some(i), binding: a.binding(), title: a.title.clone() });
    }
    changed
}

/// The Edit Action sheet: Some(true) for OK, Some(false) for Cancel.
fn action_sheet(ctx: &egui::Context, e: &mut ActionEditor, profiles: &[(String, String)]) -> Option<bool> {
    let mut out = None;
    egui::Window::new("Edit Action").collapsible(false).resizable(true).default_width(560.0).show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label("Title:");
            ui.add(egui::TextEdit::singleline(&mut e.title).desired_width(380.0));
        });
        ui.horizontal(|ui| {
            ui.label("Action:");
            if crate::settings_keys::action_combo(ui, "useraction-action", &mut e.binding.action, false) {
                e.binding.text.clear();
                if !user_actions::shows_apply_mode(e.binding.action) {
                    e.binding.apply_mode = 0;
                }
            }
        });
        crate::settings_keys::param_editor(ui, "useraction-param", &mut e.binding, profiles);
        crate::settings_keys::apply_mode_combo(ui, "useraction-apply", &mut e.binding);
        ui.separator();
        ui.horizontal(|ui| {
            if ui.add_enabled(e.binding.action != -1, egui::Button::new("OK")).clicked() {
                out = Some(true);
            }
            if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                out = Some(false);
            }
        });
    });
    out
}

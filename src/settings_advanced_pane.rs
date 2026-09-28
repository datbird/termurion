//! Settings > Advanced drawn in egui (`iTermAdvancedSettingsViewController.m`).

use std::sync::OnceLock;

use toml::Value;

use crate::advanced_pane::{self, Item, Row, RowKind};
use crate::globals::Globals;
use crate::settings_keys::row_enabled;

/// The pane's state across frames.
#[derive(Default)]
pub struct AdvancedState {
    /// The search field's text.
    pub query: String,
    /// "Show only non-default values".
    pub only_non_default: bool,
    /// The key and text of the text field being edited: its row stays under the non-default
    /// filter, and its text is shown as typed, until editing ends.
    editing: Option<(String, String)>,
}

/// The rows in iTerm2's order, built once.
fn all_rows() -> &'static [Row] {
    static ROWS: OnceLock<Vec<Row>> = OnceLock::new();
    ROWS.get_or_init(advanced_pane::rows)
}

/// The value the No/Yes popup stores for its item (`ASVC:193-222`).
pub fn bool_write(index: usize) -> Value {
    Value::Boolean(index == 1)
}

/// The Unspecified/No/Yes popup (`ASVC:233-266`): Unspecified removes the key, No and Yes store
/// false and true.
pub fn tristate_write(g: &mut Globals, key: &str, index: usize) {
    match index {
        0 => g.unset(key),
        i => g.set(key, Value::Boolean(i == 2)),
    }
}

/// An enum popup (`ASVC:268-291`): the chosen option's value is stored, and the default leaves
/// the file.
pub fn enum_write(g: &mut Globals, row: &Row, index: usize) {
    if let RowKind::Enum(choices) = &row.kind {
        if let Some((value, _)) = choices.get(index) {
            g.set(row.key, value.clone());
        }
    }
}

/// A text field's text as stored on each keystroke (`ASVC:578-621`): integers as
/// `integerValue`, floats as `doubleValue`, strings as typed; a value equal to the default
/// leaves the file.
pub fn text_write(g: &mut Globals, row: &Row, text: &str) {
    let default = row.default.as_ref().and_then(|d| d.as_float().or_else(|| d.as_integer().map(|i| i as f64)));
    match row.kind {
        RowKind::Int => g.set(row.key, Value::Integer(advanced_pane::parse_int(text))),
        RowKind::Float => {
            let v = advanced_pane::parse_float(text);
            // The schema may write a float's default as an integer.
            if default == Some(v) {
                g.unset(row.key);
            } else {
                g.set(row.key, Value::Float(v));
            }
        }
        _ => g.set(row.key, Value::String(text.to_string())),
    }
}

/// A number or string as its text field shows it.
fn shown_text(g: &Globals, row: &Row) -> String {
    match (&row.kind, g.get(row.key)) {
        (RowKind::Float, Some(v)) => v.as_float().or_else(|| v.as_integer().map(|i| i as f64)).map_or(String::new(), advanced_pane::format_float),
        (RowKind::Int, Some(v)) => v.as_integer().map_or(String::new(), |i| format!("{i}")),
        (_, Some(v)) => v.as_str().unwrap_or_default().to_string(),
        (_, None) => String::new(),
    }
}

/// A popup of `items` showing `selected`; returns the picked item.
fn popup(ui: &mut egui::Ui, key: &str, selected: Option<usize>, items: &[&str]) -> Option<usize> {
    let mut picked = None;
    let shown = selected.and_then(|i| items.get(i)).copied().unwrap_or("");
    egui::ComboBox::from_id_salt(("advanced", key)).selected_text(shown).show_ui(ui, |ui| {
        for (i, item) in items.iter().enumerate() {
            if ui.selectable_label(selected == Some(i), *item).clicked() {
                picked = Some(i);
            }
        }
    });
    picked
}

/// The value cell of one row. Returns true when the change should be saved.
fn value_cell(ui: &mut egui::Ui, g: &mut Globals, st: &mut AdvancedState, row: &Row) -> bool {
    match &row.kind {
        RowKind::Bool => {
            let picked = popup(ui, row.key, Some(usize::from(g.bool(row.key))), &advanced_pane::NO_YES);
            picked.map(|i| g.set(row.key, bool_write(i))).is_some()
        }
        RowKind::OptionalBool => {
            let picked = popup(ui, row.key, Some(advanced_pane::tristate_index(g.optional_bool(row.key))), &advanced_pane::TRISTATE);
            picked.map(|i| tristate_write(g, row.key, i)).is_some()
        }
        RowKind::Enum(choices) => {
            let now = g.get(row.key);
            let selected = choices.iter().position(|(v, _)| Some(v) == now.as_ref());
            let labels: Vec<&str> = choices.iter().map(|(_, l)| l.as_str()).collect();
            let picked = popup(ui, row.key, selected, &labels);
            picked.map(|i| enum_write(g, row, i)).is_some()
        }
        RowKind::Int | RowKind::Float | RowKind::Text => {
            let editing = st.editing.as_ref().filter(|(k, _)| k == row.key).map(|(_, t)| t.clone());
            let was_editing = editing.is_some();
            let mut text = editing.unwrap_or_else(|| shown_text(g, row));
            let field = ui.add(egui::TextEdit::singleline(&mut text).id_salt(("advanced", row.key)).desired_width(ui.available_width()));
            if field.changed() {
                text_write(g, row, &text);
            }
            if field.has_focus() {
                st.editing = Some((row.key.to_string(), text));
                false
            } else if field.lost_focus() || was_editing {
                // Editing ended (or the field lost its focus unseen, as when the page turned):
                // the file is written and the text re-formatted from the stored value. A field
                // that took the focus earlier in this frame keeps its note.
                if st.editing.as_ref().is_some_and(|(k, _)| k == row.key) {
                    st.editing = None;
                }
                true
            } else {
                false
            }
        }
    }
}

/// The Advanced pane: the search field and "Show only non-default values" above a list of
/// every Advanced setting under its category (`ASVC`, `XIB:10672-10767`). Returns true when a
/// change should be saved.
pub fn page(ui: &mut egui::Ui, g: &mut Globals, st: &mut AdvancedState) -> bool {
    let mut changed = false;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.checkbox(&mut st.only_non_default, advanced_pane::ONLY_NON_DEFAULT).on_hover_text(advanced_pane::ONLY_NON_DEFAULT_TIP);
        ui.add(egui::TextEdit::singleline(&mut st.query).hint_text(advanced_pane::SEARCH_HINT).desired_width(ui.available_width()));
    });
    let rows = all_rows();
    let keep = st.editing.as_ref().map(|(k, _)| k.clone());
    let items = advanced_pane::filtered(rows, &st.query, st.only_non_default, &|k| g.values.get(k).cloned(), keep.as_deref());
    egui::ScrollArea::vertical().show(ui, |ui| {
        let label_width = (ui.available_width() * 0.65).max(200.0);
        let setting_row = |ui: &mut egui::Ui, label: &mut dyn FnMut(&mut egui::Ui), value: &mut dyn FnMut(&mut egui::Ui)| {
            ui.horizontal(|ui| {
                ui.set_min_height(30.0);
                ui.allocate_ui_with_layout(egui::vec2(label_width, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.set_width(label_width);
                    label(ui);
                });
                value(ui);
            });
        };
        setting_row(ui, &mut |ui| _ = ui.strong(advanced_pane::SETTING_COLUMN), &mut |ui| _ = ui.strong(advanced_pane::VALUE_COLUMN));
        for (n, item) in items.into_iter().enumerate() {
            // Striped as the table's rows; the stripe goes under the row once its height is known.
            let stripe = ui.painter().add(egui::Shape::Noop);
            let top = ui.cursor().top();
            match item {
                // A group row (`ASVC:709-713`): the header spans the whole row.
                Item::Header(category) => {
                    ui.vertical(|ui| {
                        ui.set_min_height(30.0);
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new(category).strong().size(20.0));
                    });
                }
                Item::Setting(i) => {
                    let row = &rows[i];
                    let enabled = row_enabled(g, row.key) && !row.macos_only;
                    let hover = if row.macos_only { format!("{} (macOS only)", row.description) } else { row.description.to_string() };
                    setting_row(
                        ui,
                        &mut |ui| {
                            ui.add(egui::Label::new(row.label).wrap());
                            if let Some(subtitle) = row.subtitle {
                                let weak = ui.visuals().weak_text_color();
                                ui.add(egui::Label::new(egui::RichText::new(subtitle).size(11.0).color(weak)).wrap());
                            }
                        },
                        &mut |ui| {
                            let cell = ui.add_enabled_ui(enabled, |ui| value_cell(ui, g, st, row));
                            changed |= cell.inner;
                            cell.response.on_hover_text(hover.as_str());
                        },
                    );
                }
            }
            if n % 2 == 1 {
                let rect = egui::Rect::from_x_y_ranges(ui.max_rect().x_range(), top..=ui.min_rect().bottom());
                ui.painter().set(stripe, egui::Shape::rect_filled(rect, 0.0, ui.visuals().faint_bg_color));
            }
        }
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::advanced_pane;
    use toml::Value;

    fn globals() -> Globals {
        Globals::empty(std::path::Path::new("unused-settings.toml"))
    }

    fn row(key: &str) -> advanced_pane::Row {
        advanced_pane::rows().into_iter().find(|r| r.key == key).unwrap_or_else(|| panic!("{key}"))
    }

    #[test]
    fn tristate_writes_follow_iterm2() {
        let key = "PreventEscapeSequenceFromChangingProfile";
        let mut g = globals();
        tristate_write(&mut g, key, 2);
        assert_eq!(g.values.get(key), Some(&Value::Boolean(true)));
        tristate_write(&mut g, key, 1);
        assert_eq!(g.values.get(key), Some(&Value::Boolean(false)));
        tristate_write(&mut g, key, 0);
        assert!(!g.values.contains_key(key));
        assert_eq!(g.optional_bool(key), None);
        assert_eq!(bool_write(0), Value::Boolean(false));
        assert_eq!(bool_write(1), Value::Boolean(true));
    }

    #[test]
    fn text_writes_follow_iterm2() {
        let mut g = globals();
        let height = row("statusBarHeight");
        text_write(&mut g, &height, "22.5");
        assert_eq!(g.values.get("statusBarHeight"), Some(&Value::Float(22.5)));
        text_write(&mut g, &height, "21");
        assert!(!g.values.contains_key("statusBarHeight"));
        text_write(&mut g, &row("maximumBytesToProvideToPythonAPI"), "12abc");
        assert_eq!(g.values.get("maximumBytesToProvideToPythonAPI"), Some(&Value::Integer(12)));
        let prefix = row("tmuxTitlePrefix");
        text_write(&mut g, &prefix, "> ");
        assert_eq!(g.values.get("tmuxTitlePrefix"), Some(&Value::String("> ".into())));
        let default = prefix.default.as_ref().and_then(Value::as_str).unwrap().to_string();
        text_write(&mut g, &prefix, &default);
        assert!(!g.values.contains_key("tmuxTitlePrefix"));
    }

    #[test]
    fn enum_writes_follow_iterm2() {
        let mut g = globals();
        let key = "anonymousTmuxWindowsOpenInCurrentWindow";
        let tmux = row(key);
        enum_write(&mut g, &tmux, 0);
        assert_eq!(g.values.get(key), Some(&Value::Integer(0)));
        let advanced_pane::RowKind::Enum(choices) = &tmux.kind else { panic!("{key}") };
        let default = choices.iter().position(|(v, _)| Some(v) == tmux.default.as_ref()).unwrap();
        enum_write(&mut g, &tmux, default);
        assert!(!g.values.contains_key(key));
    }
}

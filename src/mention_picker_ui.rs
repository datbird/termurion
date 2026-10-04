//! The `@` session picker's list (`CW:985-1034`), an `egui::Area` drawn inside a host window's
//! `Ui`: the Cockpit's command bar and the chat window's input. The host owns the keys (Up,
//! Down, Return, Escape) and the text; this draws the rows and reports a click.

use crate::mention::{Chip, Picker};

/// Rows shown before the list scrolls.
const MAX_ROWS: usize = 10;
/// A row's height and the room each level of depth indents.
const ROW_H: f32 = 22.0;
const INDENT: f32 = 14.0;
const WIDTH: f32 = 280.0;

/// The glyph beside an item's kind: terminal for a session, as the chat token draws it.
fn kind_glyph(chip: &Chip) -> char {
    match chip {
        Chip::Window { .. } => crate::cockpit::WINDOW_GLYPH,
        Chip::Tab { .. } => crate::cockpit::TAB_GLYPH,
        Chip::Session { .. } => crate::cockpit::SESSION_GLYPH,
    }
}

fn list(ui: &mut egui::Ui, picker: &Picker) -> Option<usize> {
    let mut clicked = None;
    let icons = crate::settings_keys::has_icon_font(ui.ctx());
    ui.set_width(WIDTH);
    egui::ScrollArea::vertical().max_height(ROW_H * MAX_ROWS as f32).auto_shrink([false, true]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for (i, item) in picker.items.iter().enumerate() {
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_H), egui::Sense::click());
            let selected = i == picker.selected;
            if selected {
                ui.painter().rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
                ui.scroll_to_rect(rect, None);
            } else if resp.hovered() {
                ui.painter().rect_filled(rect, 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
            }
            let color = if selected { ui.visuals().selection.stroke.color } else { ui.visuals().text_color() };
            let mut x = rect.min.x + 6.0 + f32::from(item.depth) * INDENT;
            let mid = rect.center().y;
            if icons {
                let font = egui::FontId::new(13.0, crate::settings_keys::icon_family());
                let r = ui.painter().text(egui::pos2(x, mid), egui::Align2::LEFT_CENTER, kind_glyph(&item.chip).to_string(), font, color);
                x = r.max.x + 6.0;
            }
            let clip = ui.painter().with_clip_rect(rect);
            clip.text(egui::pos2(x, mid), egui::Align2::LEFT_CENTER, &item.label, egui::TextStyle::Body.resolve(ui.style()), color);
            if resp.clicked() {
                clicked = Some(i);
            }
        }
    });
    clicked
}

/// The list with its top left at `anchor` (under the field); the index of a clicked item.
pub fn show(ui: &mut egui::Ui, picker: &Picker, anchor: egui::Pos2) -> Option<usize> {
    if picker.items.is_empty() {
        return None;
    }
    let area = egui::Area::new(egui::Id::new("mention_picker")).order(egui::Order::Foreground).fixed_pos(anchor);
    area.show(ui.ctx(), |ui| egui::Frame::popup(ui.style()).show(ui, |ui| list(ui, picker)).inner).inner
}

/// The same list with its bottom left at `anchor`, for a field at the foot of its window.
pub fn show_above(ui: &mut egui::Ui, picker: &Picker, anchor: egui::Pos2) -> Option<usize> {
    if picker.items.is_empty() {
        return None;
    }
    let area = egui::Area::new(egui::Id::new("mention_picker")).order(egui::Order::Foreground).pivot(egui::Align2::LEFT_BOTTOM).fixed_pos(anchor);
    area.show(ui.ctx(), |ui| egui::Frame::popup(ui.style()).show(ui, |ui| list(ui, picker)).inner).inner
}

/// The chips chosen so far as a strip of rounded labels, each with a small close button; the
/// index of the chip whose button was pressed.
pub fn chip_strip(ui: &mut egui::Ui, chips: &[(Chip, String)]) -> Option<usize> {
    let mut closed = None;
    let icons = crate::settings_keys::has_icon_font(ui.ctx());
    ui.horizontal_wrapped(|ui| {
        for (i, (chip, label)) in chips.iter().enumerate() {
            let fill = ui.visuals().widgets.inactive.weak_bg_fill;
            egui::Frame::new().fill(fill).corner_radius(10.0).inner_margin(egui::Margin::symmetric(8, 2)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    if icons {
                        ui.label(egui::RichText::new(kind_glyph(chip).to_string()).family(crate::settings_keys::icon_family()).size(12.0));
                    }
                    ui.label(label);
                    let close = ui.add(egui::Button::new("x").frame(false).small()).on_hover_text("Remove");
                    if close.clicked() {
                        closed = Some(i);
                    }
                });
            });
        }
    });
    closed
}

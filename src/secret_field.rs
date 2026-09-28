//! A password field that edits a `pwm::Secret` in place (egui's `TextEdit` keeps plain copies
//! of its text in its undo history). The password manager's panels and the AI key dialogs use it.

use crate::pwm::Secret;

/// A password field that edits a `Secret` in place: typing and pasting append (a pasted line
/// break is dropped, as in a single-line secure field), Backspace deletes the last character
/// (Ctrl+Backspace all of it), and it shows a bullet per character, or the text itself when
/// `shown`. It never copies to the clipboard. It publishes no IME output, so egui-winit turns
/// the IME off while it has focus (a secure field's secure input does the same): nothing is
/// composed in plain text, and keys arrive as `Event::Text`.
pub fn secret_field(ui: &mut egui::Ui, id: egui::Id, s: &mut Secret, enabled: bool, hint: Option<&str>, width: f32, shown: bool) -> egui::Response {
    let h = ui.spacing().interact_size.y;
    let (_, rect) = ui.allocate_space(egui::vec2(width, h));
    let resp = ui.interact(rect, id, if enabled { egui::Sense::click() } else { egui::Sense::hover() });
    if resp.clicked() {
        resp.request_focus();
    }
    let focused = enabled && resp.has_focus();
    if focused {
        ui.input(|i| {
            for e in &i.events {
                match e {
                    egui::Event::Text(t) | egui::Event::Paste(t) => s.push_line(t),
                    egui::Event::Key { key: egui::Key::Backspace, pressed: true, modifiers, .. } => {
                        if modifiers.command {
                            s.clear();
                        } else {
                            s.pop();
                        }
                    }
                    _ => {}
                }
            }
        });
    }
    let v = ui.visuals();
    let stroke = if focused { v.selection.stroke } else { v.widgets.inactive.bg_stroke };
    let painter = ui.painter_at(rect);
    painter.rect(rect, 2.0, v.extreme_bg_color, stroke, egui::StrokeKind::Inside);
    let font = egui::TextStyle::Body.resolve(ui.style());
    let at = rect.left_center() + egui::vec2(4.0, 0.0);
    let end = if s.is_empty() {
        if let Some(hint) = hint {
            painter.text(at, egui::Align2::LEFT_CENTER, hint, font.clone(), v.weak_text_color());
        }
        at.x
    } else {
        let color = if enabled { v.text_color() } else { v.weak_text_color() };
        let r = if shown {
            painter.text(at, egui::Align2::LEFT_CENTER, s.expose(), font.clone(), color)
        } else {
            painter.text(at, egui::Align2::LEFT_CENTER, "\u{2022}".repeat(s.expose().chars().count()), font.clone(), color)
        };
        r.right()
    };
    if focused {
        painter.vline(end + 1.0, rect.shrink(4.0).y_range(), egui::Stroke::new(1.0, v.text_color()));
    }
    resp
}

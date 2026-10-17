//! The autofill contact card editor (milestone 13x7, Task 2): iTerm2's contact card as a small
//! window with twelve single-line fields in a two-column grid, OK (Enter) and Cancel (Escape).
//! The card is personal data: no value is logged or put in a banner, the values shown are the
//! card's own run through `display_text` (hidden characters out, `MAX_VALUE` cut), and the window
//! wipes its copy when it is dropped.

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::browser_autofill::{CARD_LABELS, CARD_TEXT, CARD_TITLE, Card, MAX_VALUE};
use crate::egui_host::EguiHost;

/// What the editor answered. `Debug` shows no value.
#[derive(PartialEq)]
#[allow(clippy::large_enum_variant)] // built once and moved rarely; boxing the variant adds an indirection for no gain
pub enum CardResult {
    Saved(Card),
    Canceled,
}

impl std::fmt::Debug for CardResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CardResult::Saved(_) => f.write_str("Saved(<card>)"),
            CardResult::Canceled => f.write_str("Canceled"),
        }
    }
}

/// The twelve (label, value) rows the window draws, in `CARD_LABELS` order, each value as shown.
pub fn rows(card: &Card) -> Vec<(&'static str, String)> {
    let mut c = card.clone();
    let values: Vec<String> = c.fields_mut().into_iter().map(|v| crate::inert_files::display_text(v, MAX_VALUE, false)).collect();
    c.wipe();
    CARD_LABELS.iter().map(|(_, label)| *label).zip(values).collect()
}

pub struct CardWindow {
    host: EguiHost,
    card: Card,
    /// The first field takes the focus on the first frame.
    focus: bool,
    /// The frame it first drew in, where Return and Escape do nothing (12a's modal key guard).
    opened: Option<u64>,
}

impl Drop for CardWindow {
    fn drop(&mut self) {
        self.card.wipe();
    }
}

impl CardWindow {
    pub fn open(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue, card: Card) -> Result<CardWindow, String> {
        let host = EguiHost::open(event_loop, instance, adapter, device, queue, CARD_TITLE, 460.0, 470.0)?;
        // What the editor holds is what it shows: bounded, with no hidden character.
        Ok(CardWindow { host, card: card.bounded(), focus: true, opened: None })
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    pub fn focus(&self) {
        self.host.focus();
    }

    pub fn event(&mut self, e: &WindowEvent) -> bool {
        self.host.event(e)
    }

    /// Draws a frame; the answer when OK or Cancel was chosen.
    pub fn redraw(&mut self) -> Option<CardResult> {
        let mut ok = false;
        let mut cancel = false;
        let (card, focus, opened) = (&mut self.card, &mut self.focus, &mut self.opened);
        self.host.paint(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.label(CARD_TEXT);
                ui.add_space(8.0);
                let ids: Vec<egui::Id> = (0..CARD_LABELS.len()).map(|i| egui::Id::new(("autofill-card", i))).collect();
                egui::Grid::new("autofill-card-grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    for (i, (field, (_, label))) in card.fields_mut().into_iter().zip(CARD_LABELS.iter()).enumerate() {
                        ui.label(*label);
                        ui.add(egui::TextEdit::singleline(field).id(ids[i]).char_limit(MAX_VALUE).desired_width(280.0));
                        ui.end_row();
                    }
                });
                if std::mem::take(focus) {
                    ui.memory_mut(|m| m.request_focus(ids[0]));
                }
                ui.add_space(10.0);
                let first = *opened.get_or_insert_with(|| ui.ctx().cumulative_frame_nr());
                let (enter, escape) = crate::settings_ai::modal_keys(ui, first, &ids);
                ui.horizontal(|ui| {
                    ok = ui.add(egui::Button::new("OK").min_size(egui::vec2(76.0, 24.0))).clicked() || enter;
                    cancel = ui.add(egui::Button::new("Cancel").min_size(egui::vec2(76.0, 24.0))).clicked() || escape;
                });
            });
        });
        if ok {
            Some(CardResult::Saved(self.card.bounded()))
        } else if cancel {
            Some(CardResult::Canceled)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_labels_render() {
        let card = Card { first_name: "Al\u{202e}ice".into(), zip: "9".repeat(MAX_VALUE + 9), ..Card::default() };
        let r = rows(&card);
        assert_eq!(r.len(), 12);
        assert!(r.iter().map(|(l, _)| *l).eq(CARD_LABELS.iter().map(|(_, l)| *l)));
        assert_eq!(r[0], ("First name", "Alice".to_string()), "hidden characters are not shown");
        assert_eq!(r[9].1.chars().count(), MAX_VALUE);
        assert!(r[1].1.is_empty());
        assert_eq!(format!("{:?}", CardResult::Saved(card)), "Saved(<card>)");
    }
}

//! The app side of browser autofill (milestone 13x7): the contact card editor window. Task 3
//! extends `AutofillState`. Card values never reach the log or a banner.

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::App;
use crate::autofill_card_ui::{CardResult, CardWindow};
use crate::browser_autofill;

#[derive(Default)]
pub(super) struct AutofillState {
    pub(super) card: Option<CardWindow>,
    /// The editor has opened on its own once this run (an empty card asks once).
    pub(super) card_asked: bool,
}

impl App {
    /// Opens the contact card editor with the saved card, or brings the open one forward.
    #[allow(dead_code)] // Task 3's fill flow and the menu call it
    pub(super) fn open_card_editor(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(w) = &self.autofill.card {
            w.focus();
            return;
        }
        let Some(g) = self.gpu.as_ref() else { return };
        let Some(base) = crate::webhost_win::local_appdata() else {
            self.banners.set("window", "Could not open the contact card: no local application data folder.".to_string());
            return;
        };
        let mut card = browser_autofill::load_card(&browser_autofill::card_path(&base));
        let opened = CardWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, card.clone());
        card.wipe();
        match opened {
            Ok(w) => {
                self.autofill.card_asked = true;
                self.autofill.card = Some(w);
            }
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// Events for the contact card editor. False for other windows. OK saves the card (a failure
    /// is a banner without values and leaves the editor open); Cancel or closing drops it.
    pub(super) fn autofill_card_event(&mut self, window: WindowId, e: &WindowEvent) -> bool {
        let Some(w) = self.autofill.card.as_mut().filter(|w| w.id() == window) else { return false };
        let answer = match e {
            WindowEvent::CloseRequested => Some(CardResult::Canceled),
            WindowEvent::RedrawRequested => match w.redraw() {
                Some(r) => Some(r),
                None => return true,
            },
            other => {
                w.event(other);
                return true;
            }
        };
        match answer {
            Some(CardResult::Saved(mut card)) => {
                let saved = match crate::webhost_win::local_appdata() {
                    Some(base) => browser_autofill::save_card(&browser_autofill::card_path(&base), &card),
                    None => Err("no local application data folder".to_string()),
                };
                card.wipe();
                match saved {
                    Ok(()) => self.autofill.card = None,
                    Err(e) => self.banners.set("window", format!("Could not save the contact card: {e}")),
                }
            }
            _ => self.autofill.card = None,
        }
        true
    }
}

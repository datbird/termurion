//! What output asks of the clipboard: writes under Applications in terminal may access
//! clipboard, reads under iTerm2's never / always / ask policy, both answered in the pane.

use super::*;
use crate::clipboard_osc::{self, ReadPolicy};
use crate::screen::Request;

/// The bars this file asks with.
pub(super) const DENIED: &str = "clipboard-denied";
pub(super) const READ: &str = "clipboard-read";

impl App {
    /// Each pane's trust and clipboard claim follow the global settings.
    pub(super) fn sync_output_policy(&mut self) {
        let trusted = !self.globals.bool("DisablePotentiallyInsecureEscapeSequences");
        let chosen = self.globals.values.contains_key("AllowClipboardAccess");
        let allowed = self.globals.bool("AllowClipboardAccess");
        let advertise = clipboard_osc::advertise_52(
            self.globals.bool("AllowClipboardAccess"),
            chosen,
            self.globals.bool("NoSyncSuppressClipboardAccessDeniedWarning"),
        );
        for p in self.panes.values_mut() {
            let s = p.own_mut();
            s.trusted = trusted;
            s.clipboard_allowed = allowed;
            s.set_clipboard_advertised(advertise);
        }
    }

    /// Acts on what each pane's output asked for.
    pub(super) fn take_requests(&mut self) {
        let asked: Vec<(PaneId, Request)> =
            self.panes.iter_mut().flat_map(|(id, p)| p.own_mut().take_requests().into_iter().map(|r| (*id, r)).collect::<Vec<_>>()).collect();
        for (pane, r) in asked {
            match r {
                Request::ClipboardWrite(text) => self.clipboard_write(pane, text),
                Request::ClipboardRead(pc) => self.clipboard_read(pane, pc),
                other => self.window_request(pane, other),
            }
        }
    }

    fn clipboard_write(&mut self, pane: PaneId, text: String) {
        if !self.globals.bool("AllowClipboardAccess") {
            if !self.globals.bool("NoSyncSuppressClipboardAccessDeniedWarning") {
                self.ask(
                    pane,
                    DENIED,
                    "The terminal attempted to access the clipboard but it was denied. Enable clipboard access in Settings > General > Selection.",
                    &["Open Settings", "Don't Show This Again"],
                );
            }
            return;
        }
        if let Some(cb) = &mut self.clipboard {
            if let Err(e) = cb.set_text(text) {
                self.banners.set("clipboard", format!("Copy failed: {e}"));
            }
        }
    }

    fn clipboard_read(&mut self, pane: PaneId, pc: String) {
        match clipboard_osc::read_policy(self.globals.bool("NoSyncNeverAllowPaste"), self.globals.bool("NoSyncAllowPaste")) {
            ReadPolicy::Never => {}
            ReadPolicy::Always => self.report_clipboard(pane, &pc),
            ReadPolicy::Ask => {
                self.pending_reads.insert(pane, pc);
                self.ask(pane, READ, "Share clipboard contents with app in terminal?", &["Just Once", "Always", "Never"]);
            }
        }
    }

    /// Sends the clipboard's text to the program that asked.
    fn report_clipboard(&mut self, pane: PaneId, pc: &str) {
        let text = match self.clipboard.as_mut().map(|cb| cb.get_text()) {
            Some(Ok(t)) => t,
            // An empty or non-text clipboard reports as empty text.
            Some(Err(arboard::Error::ContentNotAvailable)) => String::new(),
            Some(Err(e)) => {
                self.banners.set("clipboard", format!("Could not read the clipboard: {e}"));
                return;
            }
            None => return,
        };
        if let Some(p) = self.panes.get_mut(&pane) {
            p.input(&clipboard_osc::read_reply(pc, &text));
        }
        self.show_toast("Clipboard contents reported");
    }

    /// The clipboard bars' answers. True when the bar was one of these.
    pub(super) fn clipboard_bar(&mut self, pane: PaneId, id: &str, button: Option<&str>, event_loop: &ActiveEventLoop) -> bool {
        match (id, button) {
            (DENIED, Some("Open Settings")) => {
                self.open_settings(event_loop, Mode::Settings);
                if let Some(s) = self.settings.as_mut() {
                    s.show_page(crate::settings_ui::Page::General);
                }
            }
            (DENIED, Some(_)) => {
                self.globals.set("NoSyncSuppressClipboardAccessDeniedWarning", toml::Value::Boolean(true));
                self.save_globals();
            }
            (READ, answer) => {
                let pc = self.pending_reads.remove(&pane);
                match answer {
                    Some("Just Once") => {
                        if let Some(pc) = pc {
                            self.report_clipboard(pane, &pc);
                        }
                    }
                    Some("Always") => {
                        if files_ui::ask("Really Enable Clipboard Reporting?\n\nPrograms in the terminal will be able to read the clipboard without asking.") {
                            self.globals.set("NoSyncAllowPaste", toml::Value::Boolean(true));
                            self.save_globals();
                            if let Some(pc) = pc {
                                self.report_clipboard(pane, &pc);
                            }
                        }
                    }
                    Some("Never") => {
                        self.globals.set("NoSyncNeverAllowPaste", toml::Value::Boolean(true));
                        self.save_globals();
                    }
                    _ => {}
                }
            }
            _ => return false,
        }
        true
    }
}

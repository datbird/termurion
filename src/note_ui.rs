//! A small window with one editable note: an annotation's text, or the session note.

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::egui_host::EguiHost;

/// The delete button's accessibility description (`PTYNoteView.m:61`).
pub const DELETE_ACCESSIBILITY: &str = "Delete annotation";

pub enum NoteEvent {
    None,
    /// The text changed.
    Changed(String),
    /// The Delete button (annotations only), before any question: the app answers it at once
    /// when the question is silenced, else calls `confirm_delete`.
    RequestDelete,
    /// Delete confirmed with OK, the checkbox `suppress` checked or not.
    Delete { suppress: bool },
    Close,
}

pub struct NoteWindow {
    host: EguiHost,
    text: String,
    /// Offers Delete, with iTerm2's confirmation.
    deletable: bool,
    /// The inline question shows.
    confirming: bool,
    /// Its checkbox.
    suppress: bool,
    first: bool,
}

/// "Really remove annotation?" (`noteDidRequestRemoval:`, `TV:7690-7704`): heading "Confirm",
/// OK or Cancel, permanently silenceable.
pub fn remove_spec() -> crate::warnings::Spec {
    use crate::api_auth::{CANCEL, OK};
    crate::warnings::Spec {
        id: Some("NoSyncConfirmRemoveAnnotation".to_string()),
        kind: crate::warnings::Kind::Permanent,
        heading: "Confirm".to_string(),
        text: "Really remove annotation?".to_string(),
        labels: vec![OK.to_string(), CANCEL.to_string()],
        cancel: Some(1),
        never_remember: Vec::new(),
    }
}

impl NoteWindow {
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        title: &str,
        text: &str,
        deletable: bool,
    ) -> Result<NoteWindow, String> {
        let host = EguiHost::open(event_loop, instance, adapter, device, queue, title, 320.0, 200.0)?;
        Ok(NoteWindow { host, text: text.into(), deletable, confirming: false, suppress: false, first: true })
    }

    /// Shows the inline question for Delete, its checkbox unchecked.
    pub fn confirm_delete(&mut self) {
        self.confirming = true;
        self.suppress = false;
        self.host.request_redraw();
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    pub fn focus(&self) {
        self.host.focus();
    }

    pub fn event(&mut self, event: &WindowEvent) -> bool {
        self.host.event(event)
    }

    /// The note changed elsewhere (the Notes tool edits the same session note): the window
    /// shows it.
    pub fn set_text(&mut self, text: &str) {
        if self.text != text {
            self.text = text.to_string();
            self.host.request_redraw();
        }
    }

    pub fn redraw(&mut self) -> NoteEvent {
        let mut out = NoteEvent::None;
        let (text, deletable, confirming, suppress, first) = (&mut self.text, self.deletable, &mut self.confirming, &mut self.suppress, &mut self.first);
        let spec = remove_spec();
        self.host.paint(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                if deletable {
                    ui.horizontal(|ui| {
                        if *confirming {
                            ui.label(&spec.text);
                            if ui.button(&spec.labels[0]).clicked() {
                                out = NoteEvent::Delete { suppress: *suppress };
                            }
                            if ui.button(&spec.labels[1]).clicked() {
                                *confirming = false;
                            }
                        } else {
                            let r = ui.button("Delete");
                            r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, DELETE_ACCESSIBILITY));
                            if r.clicked() {
                                out = NoteEvent::RequestDelete;
                            }
                        }
                    });
                    if *confirming
                        && let Some(label) = crate::warnings::checkbox_label(&spec) {
                            ui.checkbox(suppress, label);
                        }
                }
                let r = ui.add_sized(ui.available_size(), egui::TextEdit::multiline(text));
                if *first {
                    r.request_focus();
                    *first = false;
                }
                if r.changed() {
                    out = NoteEvent::Changed(text.clone());
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    out = NoteEvent::Close;
                }
            });
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use crate::warnings::{Kind, checkbox_label};

    /// `noteDidRequestRemoval:` (`TV:7690-7704`).
    #[test]
    fn annotation_removal_follows_iterm2() {
        let s = super::remove_spec();
        assert_eq!((s.heading.as_str(), s.text.as_str()), ("Confirm", "Really remove annotation?"));
        assert_eq!(s.labels, ["OK", "Cancel"]);
        assert_eq!(s.id.as_deref(), Some("NoSyncConfirmRemoveAnnotation"));
        assert_eq!((s.kind, s.cancel), (Kind::Permanent, Some(1)));
        assert_eq!(checkbox_label(&s), Some("Suppress this message permanently"));
    }
}

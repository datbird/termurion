//! A small window with one editable note: an annotation's text, or the session note.

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::egui_host::EguiHost;

pub enum NoteEvent {
    None,
    /// The text changed.
    Changed(String),
    /// Delete confirmed (annotations only).
    Delete,
    Close,
}

pub struct NoteWindow {
    host: EguiHost,
    text: String,
    /// Offers Delete, with iTerm2's confirmation.
    deletable: bool,
    confirming: bool,
    first: bool,
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
        Ok(NoteWindow { host, text: text.into(), deletable, confirming: false, first: true })
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
        let (text, deletable, confirming, first) = (&mut self.text, self.deletable, &mut self.confirming, &mut self.first);
        self.host.paint(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                if deletable {
                    ui.horizontal(|ui| {
                        if *confirming {
                            ui.label("Really remove annotation?");
                            if ui.button("Remove").clicked() {
                                out = NoteEvent::Delete;
                            }
                            if ui.button("Cancel").clicked() {
                                *confirming = false;
                            }
                        } else if ui.button("Delete").clicked() {
                            *confirming = true;
                        }
                    });
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

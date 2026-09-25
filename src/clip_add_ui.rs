//! iTerm2's "New Clipping" sheet: a title and a detail, Cancel and Add.

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::egui_host::EguiHost;

pub enum AddEvent {
    None,
    /// Add: (title, detail).
    Add(String, String),
    Cancel,
}

pub struct AddClipWindow {
    host: EguiHost,
    title: String,
    detail: String,
    first: bool,
}

impl AddClipWindow {
    pub fn open(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<AddClipWindow, String> {
        let host = EguiHost::open(event_loop, instance, adapter, device, queue, "New Clipping", 420.0, 290.0)?;
        Ok(AddClipWindow { host, title: String::new(), detail: String::new(), first: true })
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

    pub fn redraw(&mut self) -> AddEvent {
        let mut out = AddEvent::None;
        let (title, detail, first) = (&mut self.title, &mut self.detail, &mut self.first);
        self.host.paint(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.label("Title:");
                let t = ui.add(egui::TextEdit::singleline(title).desired_width(f32::INFINITY));
                if *first {
                    t.request_focus();
                    *first = false;
                }
                // Return in the title adds (Add is the default button).
                if t.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    out = AddEvent::Add(title.clone(), detail.clone());
                }
                ui.label("Detail:");
                let room = ui.available_height() - 36.0;
                let d = egui::ScrollArea::vertical().max_height(room.max(40.0)).show(ui, |ui| ui.add_sized([ui.available_width(), room.max(40.0)], egui::TextEdit::multiline(detail))).inner;
                // Shift+Return in the detail adds too.
                if d.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.shift) {
                    let text = detail.trim_end_matches(['\n', '\r']).to_string();
                    out = AddEvent::Add(title.clone(), text);
                }
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        out = AddEvent::Cancel;
                    }
                    if ui.button("Add").clicked() {
                        out = AddEvent::Add(title.clone(), detail.clone());
                    }
                });
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    out = AddEvent::Cancel;
                }
            });
        });
        out
    }
}

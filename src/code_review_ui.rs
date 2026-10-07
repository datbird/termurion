//! The Code Review prompt windows (milestone 13y1, Task 7): the manager (iTerm2's "Code Review Prompts"
//! window, `CRM`) and the "Name this prompt" sheet (`CRV` `saveAsNewMenuItem`), both egui windows like
//! the other small tool windows. They hold their own copy of the list and report each change; the app
//! stores it through one write point (`code_review::bounded`).

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::code_review as cr;
use crate::code_review::SavedPrompt;
use crate::egui_host::EguiHost;

/// What the manager reports for a frame.
pub enum ManagerEvent {
    None,
    /// The list changed (an add, a remove or an edit): store it.
    Changed(Vec<SavedPrompt>),
    /// A prompt was selected: its uuid is the last selected one (`CRM` `loadDetailFromSelection`).
    Selected(String),
}

pub struct ManagerWindow {
    host: EguiHost,
    prompts: Vec<SavedPrompt>,
    selected: Option<usize>,
    /// Focus the name field of a prompt that was just added.
    focus_name: bool,
}

impl ManagerWindow {
    pub fn open(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue, prompts: Vec<SavedPrompt>, selected_uuid: Option<&str>) -> Result<ManagerWindow, String> {
        let host = EguiHost::open(event_loop, instance, adapter, device, queue, cr::MANAGER_TITLE, 720.0, 460.0)?;
        let selected = selected_uuid.and_then(|u| prompts.iter().position(|p| p.uuid == u)).or_else(|| (!prompts.is_empty()).then_some(0));
        Ok(ManagerWindow { host, prompts, selected, focus_name: false })
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

    /// The list changed elsewhere (Save Current as New): shown now, the selection kept by uuid.
    pub fn reload(&mut self, prompts: Vec<SavedPrompt>) {
        let keep = self.selected.and_then(|i| self.prompts.get(i)).map(|p| p.uuid.clone());
        self.selected = keep.and_then(|u| prompts.iter().position(|p| p.uuid == u)).or_else(|| (!prompts.is_empty()).then_some(0));
        self.prompts = prompts;
    }

    pub fn redraw(&mut self) -> ManagerEvent {
        let mut out = ManagerEvent::None;
        let (prompts, selected, focus_name) = (&mut self.prompts, &mut self.selected, &mut self.focus_name);
        self.host.paint(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.columns(2, |cols| {
                    // Left: the names, then + and -.
                    egui::ScrollArea::vertical().max_height(cols[0].available_height() - 36.0).show(&mut cols[0], |ui| {
                        for (i, p) in prompts.iter().enumerate() {
                            let shown = if p.name.trim().is_empty() { cr::UNTITLED } else { p.name.as_str() };
                            if ui.selectable_label(*selected == Some(i), shown).clicked() && *selected != Some(i) {
                                *selected = Some(i);
                                out = ManagerEvent::Selected(p.uuid.clone());
                            }
                        }
                    });
                    cols[0].horizontal(|ui| {
                        if ui.button("+").clicked() && prompts.len() < cr::MAX_PROMPTS {
                            let taken: Vec<String> = prompts.iter().map(|p| p.name.clone()).collect();
                            prompts.push(SavedPrompt { uuid: cr::fresh_uuid(), name: cr::unique_name(cr::NEW_PROMPT, &taken), text: String::new() });
                            *selected = Some(prompts.len() - 1);
                            *focus_name = true;
                            out = ManagerEvent::Changed(prompts.clone());
                        }
                        let can_remove = selected.is_some();
                        if ui.add_enabled(can_remove, egui::Button::new("\u{2212}")).clicked() {
                            if let Some(i) = selected.take().filter(|i| *i < prompts.len()) {
                                prompts.remove(i);
                                *selected = (!prompts.is_empty()).then(|| i.min(prompts.len() - 1));
                                out = ManagerEvent::Changed(prompts.clone());
                            }
                        }
                    });
                    // Right: the name and the prompt, or the placeholder.
                    let ui = &mut cols[1];
                    match selected.and_then(|i| prompts.get_mut(i)) {
                        None => {
                            ui.label(cr::EMPTY_PLACEHOLDER);
                        }
                        Some(p) => {
                            ui.label(cr::NAME_LABEL);
                            let name = ui.add(egui::TextEdit::singleline(&mut p.name).hint_text(cr::UNTITLED).desired_width(f32::INFINITY));
                            if std::mem::take(focus_name) {
                                name.request_focus();
                            }
                            let name_changed = name.changed();
                            ui.label(cr::PROMPT_LABEL);
                            let room = (ui.available_height() - 4.0).max(60.0);
                            let body = egui::ScrollArea::vertical().max_height(room).show(ui, |ui| ui.add_sized([ui.available_width(), room], egui::TextEdit::multiline(&mut p.text))).inner;
                            if p.text.len() > cr::MAX_TEXT {
                                p.text.truncate(cr::MAX_TEXT);
                                while !p.text.is_char_boundary(p.text.len()) {
                                    p.text.pop();
                                }
                            }
                            if name_changed || body.changed() {
                                out = ManagerEvent::Changed(prompts.clone());
                            }
                        }
                    }
                });
            });
        });
        out
    }
}

/// What the name sheet reports.
pub enum NameEvent {
    None,
    Save(String),
    Cancel,
}

/// "Name this prompt": the field, Save and Cancel (`CRV` `saveAsNewMenuItem`).
pub struct NameWindow {
    host: EguiHost,
    name: String,
    first: bool,
}

impl NameWindow {
    pub fn open(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<NameWindow, String> {
        let host = EguiHost::open(event_loop, instance, adapter, device, queue, cr::SAVE_TITLE, 380.0, 170.0)?;
        Ok(NameWindow { host, name: String::new(), first: true })
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

    pub fn redraw(&mut self) -> NameEvent {
        let mut out = NameEvent::None;
        let (name, first) = (&mut self.name, &mut self.first);
        self.host.paint(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.strong(cr::SAVE_TITLE);
                ui.label(cr::SAVE_TEXT);
                let field = ui.add(egui::TextEdit::singleline(name).hint_text(cr::NAME_PLACEHOLDER).desired_width(f32::INFINITY));
                if std::mem::take(first) {
                    field.request_focus();
                }
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let ready = !name.trim().is_empty();
                ui.horizontal(|ui| {
                    if ui.button(crate::api_auth::CANCEL).clicked() {
                        out = NameEvent::Cancel;
                    }
                    if (ui.add_enabled(ready, egui::Button::new("Save")).clicked() || enter) && ready {
                        out = NameEvent::Save(name.trim().to_string());
                    }
                });
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    out = NameEvent::Cancel;
                }
            });
        });
        out
    }
}

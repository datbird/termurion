//! Engage Artificial Intelligence's two windows (iTerm2's alert sheet, `PS:21117-21209`, and the
//! please-wait window, `PW:84-116`), as egui windows because Windows has no sheet.

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::ai_engage::{self, Row};
use crate::egui_host::EguiHost;

/// What the dialog answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngageResult {
    Open,
    Cancel,
    Send { text: String, skip_next: bool },
}

pub struct EngageWindow {
    host: EguiHost,
    rows: Vec<Row>,
    text: String,
    skip: bool,
    /// The field takes the focus on the first frame.
    focus: bool,
    /// The frame it first drew in, where Escape does nothing (the key that opened it).
    opened: Option<u64>,
}

impl EngageWindow {
    pub fn open(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue, ask: &ai_engage::Ask) -> Result<EngageWindow, String> {
        let host = EguiHost::open(event_loop, instance, adapter, device, queue, "Engage Artificial Intelligence", f64::from(ai_engage::FIELD_W) + 40.0, f64::from(ai_engage::FIELD_H) + 150.0)?;
        crate::settings_keys::add_icon_font(host.ctx());
        Ok(EngageWindow { host, rows: ai_engage::dialog_rows(ask), text: ask.prefill.clone(), skip: false, focus: true, opened: None })
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

    pub fn redraw(&mut self) -> EngageResult {
        let (mut ok, mut cancel) = (false, false);
        let (rows, text, skip, focus, opened) = (&self.rows, &mut self.text, &mut self.skip, &mut self.focus, &mut self.opened);
        self.host.paint(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let first = *opened.get_or_insert_with(|| ui.ctx().cumulative_frame_nr());
                // Shift+Enter presses OK; taken before the field so Enter alone still inserts a line.
                ok = ui.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter));
                cancel = ui.input(|i| i.key_pressed(egui::Key::Escape)) && ui.ctx().cumulative_frame_nr() != first;
                for row in rows {
                    match row {
                        Row::Text(t) => {
                            ui.add(egui::Label::new(*t).wrap());
                            ui.add_space(6.0);
                        }
                        Row::Field(_) => {
                            let id = egui::Id::new("ai-engage-field");
                            egui::ScrollArea::vertical().max_height(ai_engage::FIELD_H).show(ui, |ui| {
                                ui.add(egui::TextEdit::multiline(text).id(id).desired_width(ai_engage::FIELD_W).desired_rows(10).min_size(egui::vec2(ai_engage::FIELD_W, ai_engage::FIELD_H)));
                            });
                            if std::mem::take(focus) {
                                ui.memory_mut(|m| m.request_focus(id));
                            }
                            ui.add_space(6.0);
                        }
                        Row::Check(label) => {
                            ui.checkbox(skip, *label);
                        }
                        Row::Reason(r) => {
                            ui.horizontal(|ui| {
                                if let Some(g) = crate::settings_keys::icon_text(ui.ctx(), crate::scripts::WARNING_GLYPH) {
                                    ui.label(g.size(11.0));
                                }
                                ui.label(egui::RichText::new(*r).size(11.0));
                            });
                        }
                        Row::Buttons(names) => {
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                for n in names {
                                    let clicked = ui.add(egui::Button::new(*n).min_size(egui::vec2(76.0, 24.0))).clicked();
                                    match *n {
                                        "OK" => ok |= clicked,
                                        _ => cancel |= clicked,
                                    }
                                }
                            });
                        }
                    }
                }
            });
        });
        if ok {
            EngageResult::Send { text: self.text.clone(), skip_next: self.skip && self.rows.iter().any(|r| matches!(r, Row::Check(_))) }
        } else if cancel {
            EngageResult::Cancel
        } else {
            EngageResult::Open
        }
    }
}

/// What the wait window answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitResult {
    Open,
    Cancel,
}

/// "Thinking..." with an indeterminate spinner and Cancel, titled "Termurion".
pub struct WaitWindow {
    host: EguiHost,
    message: String,
    opened: Option<u64>,
}

impl WaitWindow {
    pub fn open(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue, message: &str) -> Result<WaitWindow, String> {
        let host = EguiHost::open(event_loop, instance, adapter, device, queue, "Termurion", 300.0, 110.0)?;
        Ok(WaitWindow { host, message: message.to_string(), opened: None })
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

    pub fn redraw(&mut self) -> WaitResult {
        let mut cancel = false;
        let (message, opened) = (&self.message, &mut self.opened);
        self.host.paint(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let first = *opened.get_or_insert_with(|| ui.ctx().cumulative_frame_nr());
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new());
                    ui.label(message.as_str());
                });
                ui.add_space(10.0);
                cancel = ui.add(egui::Button::new("Cancel").min_size(egui::vec2(76.0, 24.0))).clicked() || (ui.input(|i| i.key_pressed(egui::Key::Escape)) && ui.ctx().cumulative_frame_nr() != first);
                // The spinner animates, so ask for the next frame.
                ui.ctx().request_repaint();
            });
        });
        if cancel {
            WaitResult::Cancel
        } else {
            WaitResult::Open
        }
    }
}

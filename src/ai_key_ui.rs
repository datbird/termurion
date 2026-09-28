//! The one-vendor API key prompt (iTerm2's registration sheet,
//! `AITerm/AITermControllerRegistrationHelper.swift:40-51`; ruling 12e-1): a small window with
//! the key dialog's heading and text, one masked field, OK (Enter) and Cancel (Escape). The
//! typed key is a `Secret` from the first key press until the window answers or is dropped.

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::ai_settings::Vendor;
use crate::egui_host::EguiHost;
use crate::pwm::Secret;
use crate::settings_ai::{key_prompt, KEYS_TEXT};

/// What the prompt answered. `Ok` carries the typed key as typed; the caller decides whether it
/// is blank.
#[derive(Debug)]
pub enum KeyResult {
    Open,
    Cancel,
    Ok(Secret),
}

pub struct KeyWindow {
    host: EguiHost,
    hint: String,
    key: Secret,
    /// The field takes the focus on the first frame.
    focus: bool,
}

/// The field's width, as the key dialog's (`G:3556-3561`: 620 less the 104-point label column).
const FIELD_WIDTH: f32 = 516.0;

impl KeyWindow {
    pub fn open(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue, v: Vendor) -> Result<KeyWindow, String> {
        let (heading, hint) = key_prompt(v);
        let host = EguiHost::open(event_loop, instance, adapter, device, queue, &heading, f64::from(FIELD_WIDTH) + 40.0, 130.0)?;
        Ok(KeyWindow { host, hint, key: Secret::new(String::new()), focus: true })
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

    pub fn redraw(&mut self) -> KeyResult {
        let mut ok = false;
        let mut cancel = false;
        let (hint, key, focus) = (&self.hint, &mut self.key, &mut self.focus);
        self.host.paint(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.label(KEYS_TEXT);
                ui.add_space(8.0);
                let id = egui::Id::new("ai-key-prompt");
                crate::secret_field::secret_field(ui, id, key, true, Some(hint), FIELD_WIDTH, false);
                if std::mem::take(focus) {
                    ui.memory_mut(|m| m.request_focus(id));
                }
                ui.add_space(10.0);
                let (enter, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter) && i.modifiers.is_none(), i.key_pressed(egui::Key::Escape)));
                ui.horizontal(|ui| {
                    ok = ui.add(egui::Button::new("OK").min_size(egui::vec2(76.0, 24.0))).clicked() || enter;
                    cancel = ui.add(egui::Button::new("Cancel").min_size(egui::vec2(76.0, 24.0))).clicked() || escape;
                });
            });
        });
        if ok {
            KeyResult::Ok(std::mem::replace(&mut self.key, Secret::new(String::new())))
        } else if cancel {
            KeyResult::Cancel
        } else {
            KeyResult::Open
        }
    }
}

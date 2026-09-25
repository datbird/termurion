//! A small window that asks for one line of text (Set Named Mark's "Enter Mark Name").

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::egui_host::EguiHost;

/// "2026-09-24 08:05:03": iTerm2's default mark name.
pub fn stamp(y: u16, mo: u16, d: u16, h: u16, mi: u16, s: u16) -> String {
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
}

/// The local time: year, month, day, hour, minute, second.
pub fn local_time() -> (u16, u16, u16, u16, u16, u16) {
    let mut t: windows_sys::Win32::Foundation::SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t) };
    (t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

/// The local time as a mark name.
pub fn local_stamp() -> String {
    let mut t: windows_sys::Win32::Foundation::SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t) };
    stamp(t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

pub enum PromptResult {
    Open,
    Cancel,
    Ok(String),
}

pub struct PromptWindow {
    host: EguiHost,
    label: String,
    text: String,
    first: bool,
}

impl PromptWindow {
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        title: &str,
        label: &str,
        initial: &str,
    ) -> Result<PromptWindow, String> {
        let host = EguiHost::open(event_loop, instance, adapter, device, queue, title, 420.0, 130.0)?;
        Ok(PromptWindow { host, label: label.into(), text: initial.into(), first: true })
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

    /// Draws the window. Enter or OK gives the text; Escape or Cancel gives up.
    pub fn redraw(&mut self) -> PromptResult {
        let mut result = PromptResult::Open;
        let (label, text, first) = (&self.label, &mut self.text, &mut self.first);
        self.host.paint(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.label(label.as_str());
                let r = ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY));
                if *first {
                    r.request_focus();
                    *first = false;
                }
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() || enter {
                        result = PromptResult::Ok(text.clone());
                    }
                    if ui.button("Cancel").clicked() || escape {
                        result = PromptResult::Cancel;
                    }
                });
            });
        });
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_named_mark_defaults_to_the_time() {
        assert_eq!(stamp(2026, 9, 24, 8, 5, 3), "2026-09-24 08:05:03");
    }
}

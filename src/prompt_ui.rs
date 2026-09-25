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
    /// Earlier entries to pick from (Run Coprocess's history).
    choices: Vec<String>,
    /// A checkbox: its label, its state, and the entries it starts checked for.
    check: Option<(String, bool, Vec<String>)>,
    /// OK with blank text beeps and keeps the window open.
    refuse_blank: bool,
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
        Ok(PromptWindow { host, label: label.into(), text: initial.into(), first: true, choices: Vec::new(), check: None, refuse_blank: false })
    }

    /// Earlier entries, offered in a list under the text.
    pub fn with_choices(mut self, choices: Vec<String>) -> PromptWindow {
        self.choices = choices;
        self
    }

    /// A checkbox that starts checked when the text is one of `checked_for`, and follows
    /// the text as it changes.
    pub fn with_check(mut self, label: &str, checked_for: Vec<String>) -> PromptWindow {
        let on = checked_for.iter().any(|c| *c == self.text);
        self.check = Some((label.into(), on, checked_for));
        self
    }

    pub fn refuse_blank(mut self) -> PromptWindow {
        self.refuse_blank = true;
        self
    }

    /// The checkbox's state.
    pub fn checked(&self) -> bool {
        self.check.as_ref().is_some_and(|c| c.1)
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
        let (label, text, first, choices, check, refuse_blank) = (&self.label, &mut self.text, &mut self.first, &self.choices, &mut self.check, self.refuse_blank);
        self.host.paint(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.label(label.as_str());
                let before = text.clone();
                let r = ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY));
                if *first {
                    r.request_focus();
                    *first = false;
                }
                if !choices.is_empty() {
                    egui::ComboBox::from_id_salt("choices").selected_text("History").width(ui.available_width()).show_ui(ui, |ui| {
                        for c in choices {
                            if ui.selectable_label(false, c.as_str()).clicked() {
                                *text = c.clone();
                            }
                        }
                    });
                }
                if let Some((label, on, checked_for)) = check.as_mut() {
                    if *text != before {
                        *on = checked_for.iter().any(|c| c == text);
                    }
                    ui.checkbox(on, label.as_str());
                }
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() || enter {
                        if refuse_blank && text.trim().is_empty() {
                            unsafe { windows_sys::Win32::System::Diagnostics::Debug::MessageBeep(windows_sys::Win32::UI::WindowsAndMessaging::MB_OK) };
                        } else {
                            result = PromptResult::Ok(text.clone());
                        }
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

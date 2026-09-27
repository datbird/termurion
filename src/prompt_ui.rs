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
    /// The extra button (Save to File with Contents...).
    Extra(String),
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
    /// A button between OK and Cancel.
    extra: Option<String>,
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
        Ok(PromptWindow { host, label: label.into(), text: initial.into(), first: true, choices: Vec::new(), check: None, refuse_blank: false, extra: None })
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

    pub fn with_extra(mut self, label: &str) -> PromptWindow {
        self.extra = Some(label.into());
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
        let (label, text, first, choices, check, refuse_blank, extra) = (&self.label, &mut self.text, &mut self.first, &self.choices, &mut self.check, self.refuse_blank, &self.extra);
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
                    if let Some(x) = extra {
                        if ui.button(x.as_str()).clicked() {
                            result = PromptResult::Extra(text.clone());
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

/// What a `ConfirmWindow` answered.
pub enum ConfirmResult {
    Open,
    Cancel,
    /// OK, and whether "Suppress this message permanently" was checked.
    Ok { suppress: bool },
    /// A button of an `actions` window, by its place in the labels, and the checkbox's state.
    Action { index: usize, suppress: bool },
}

/// The buttons Enter and Escape press in an `actions` window: Enter the first, Escape the one
/// labeled "Cancel", if any (`NSAlert`'s key equivalents, which `iTermWarning` keeps).
pub fn action_keys(labels: &[&str]) -> (usize, Option<usize>) {
    (0, labels.iter().position(|l| *l == "Cancel"))
}

/// The button closing an `actions` window answers: Escape's (Cancel), else the first (OK),
/// which only dismisses. iTerm2's alerts cannot be closed, so closing never acts.
pub fn close_index(labels: &[&str]) -> usize {
    action_keys(labels).1.unwrap_or(0)
}

/// An OK and Cancel question with iTerm2's permanently silenceable checkbox
/// (`iTermWarning.m:400-406,517-520`: "Suppress this message permanently", remembered only on
/// OK). Enter is OK and Escape Cancel.
pub struct ConfirmWindow {
    host: EguiHost,
    text: String,
    suppress: bool,
    /// The "Suppress this message permanently" checkbox shows.
    checkbox: bool,
    /// The button that answers `Ok`, and the one that answers `Cancel` (none for a warning).
    ok: String,
    cancel: Option<String>,
    /// Enter answers the cancel button: an iTerm2 warning whose first action is Cancel makes
    /// it the default button.
    enter_cancels: bool,
    /// An `actions` window's buttons, left to right.
    actions: Vec<String>,
}

impl ConfirmWindow {
    /// The checkbox's label (`iTermWarning.m:400-406`).
    pub const SUPPRESS: &'static str = "Suppress this message permanently";

    #[allow(clippy::too_many_arguments)]
    pub fn open(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        heading: &str,
        text: &str,
    ) -> Result<ConfirmWindow, String> {
        let host = EguiHost::open(event_loop, instance, adapter, device, queue, heading, 420.0, 130.0)?;
        Ok(ConfirmWindow { host, text: text.into(), suppress: false, checkbox: true, ok: "OK".into(), cancel: Some("Cancel".into()), enter_cancels: false, actions: Vec::new() })
    }

    /// A warning that cannot be silenced: the text and an OK button (Enter; Escape closes
    /// it too).
    #[allow(clippy::too_many_arguments)]
    pub fn warning(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        heading: &str,
        text: &str,
    ) -> Result<ConfirmWindow, String> {
        let mut w = ConfirmWindow::open(event_loop, instance, adapter, device, queue, heading, text)?;
        w.checkbox = false;
        w.cancel = None;
        Ok(w)
    }

    /// A warning that cannot be silenced with two actions in iTerm2's order: `first` is the
    /// default button (Enter) and answers `Cancel`, as does Escape; `second` answers `Ok`
    /// ("Not all sessions at password prompt", the password prompt check).
    #[allow(clippy::too_many_arguments)]
    pub fn choice(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        heading: &str,
        text: &str,
        first: &str,
        second: &str,
    ) -> Result<ConfirmWindow, String> {
        let mut w = ConfirmWindow::open(event_loop, instance, adapter, device, queue, heading, text)?;
        w.checkbox = false;
        w.ok = second.into();
        w.cancel = Some(first.into());
        w.enter_cancels = true;
        Ok(w)
    }

    /// A notice with OK only that can be silenced for good (`kiTermWarningTypePermanentlySilenceable`
    /// with one action): OK answers `Ok` with the checkbox's state; Escape or closing it `Cancel`.
    #[allow(clippy::too_many_arguments)]
    pub fn notice(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        heading: &str,
        text: &str,
    ) -> Result<ConfirmWindow, String> {
        let mut w = ConfirmWindow::open(event_loop, instance, adapter, device, queue, heading, text)?;
        w.cancel = None;
        Ok(w)
    }

    /// An `iTermWarning` with any number of actions: the text, one button per label, left to
    /// right in the labels' order (iTerm2's), and "Suppress this message permanently" only when
    /// `silenceable`. A button answers `Action` with its index; Enter and Escape press the
    /// buttons `action_keys` names.
    #[allow(clippy::too_many_arguments)]
    pub fn actions(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        heading: &str,
        text: &str,
        labels: &[&str],
        silenceable: bool,
    ) -> Result<ConfirmWindow, String> {
        let mut w = ConfirmWindow::open(event_loop, instance, adapter, device, queue, heading, text)?;
        w.checkbox = silenceable;
        w.cancel = None;
        w.actions = labels.iter().map(|l| l.to_string()).collect();
        Ok(w)
    }

    /// The answer when the window is closed: Cancel, or for an `actions` window `close_index`'s
    /// button, with the checkbox not remembered.
    pub fn close_answer(&self) -> ConfirmResult {
        if self.actions.is_empty() {
            return ConfirmResult::Cancel;
        }
        let labels: Vec<&str> = self.actions.iter().map(String::as_str).collect();
        ConfirmResult::Action { index: close_index(&labels), suppress: false }
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

    pub fn redraw(&mut self) -> ConfirmResult {
        let mut result = ConfirmResult::Open;
        let (text, suppress, checkbox, ok, cancel, enter_cancels, actions) = (&self.text, &mut self.suppress, self.checkbox, &self.ok, &self.cancel, self.enter_cancels, &self.actions);
        self.host.paint(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.label(text.as_str());
                ui.add_space(6.0);
                if checkbox {
                    ui.checkbox(suppress, Self::SUPPRESS);
                    ui.add_space(6.0);
                }
                let (enter, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
                ui.horizontal(|ui| {
                    if !actions.is_empty() {
                        let labels: Vec<&str> = actions.iter().map(String::as_str).collect();
                        let (enter_index, escape_index) = action_keys(&labels);
                        for (index, label) in labels.iter().enumerate() {
                            if ui.button(*label).clicked() {
                                result = ConfirmResult::Action { index, suppress: *suppress };
                            }
                        }
                        if enter {
                            result = ConfirmResult::Action { index: enter_index, suppress: *suppress };
                        } else if let (true, Some(index)) = (escape, escape_index) {
                            result = ConfirmResult::Action { index, suppress: *suppress };
                        }
                        return;
                    }
                    if enter_cancels {
                        if let Some(c) = cancel {
                            if ui.button(c.as_str()).clicked() || enter {
                                result = ConfirmResult::Cancel;
                            }
                        }
                        if ui.button(ok.as_str()).clicked() {
                            result = ConfirmResult::Ok { suppress: *suppress };
                        }
                        if escape {
                            result = ConfirmResult::Cancel;
                        }
                        return;
                    }
                    if ui.button(ok.as_str()).clicked() || enter {
                        result = ConfirmResult::Ok { suppress: *suppress };
                    }
                    if cancel.as_ref().is_some_and(|c| ui.button(c.as_str()).clicked()) || escape {
                        result = ConfirmResult::Cancel;
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
    fn action_keys_follow_iterm2() {
        assert_eq!(action_keys(&["OK", "Cancel", "More Info", "Always"]), (0, Some(1)));
        assert_eq!(action_keys(&["OK", "Reveal Preference"]), (0, None));
        assert_eq!(action_keys(&["OK"]), (0, None));
    }

    #[test]
    fn closing_is_harmless() {
        // Escape's Cancel, else the first (OK) button, which never acts.
        assert_eq!(close_index(&["OK", "Cancel", "More Info", "Always"]), 1);
        assert_eq!(close_index(&["OK", "Reveal Preference"]), 0);
        assert_eq!(close_index(&["OK", "Show in Explorer"]), 0);
        assert_eq!(close_index(&["OK"]), 0);
    }

    #[test]
    fn a_named_mark_defaults_to_the_time() {
        assert_eq!(stamp(2026, 9, 24, 8, 5, 3), "2026-09-24 08:05:03");
    }
}
